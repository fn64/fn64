# fn64 design

Status: pre-alpha, design phase. This document is the load-bearing spec
`AGENTS.md` requires agents read before touching code. Every claim below
cites its source per the clean-room protocol: our own boot-ladder evidence
(`aki-recomp/docs/BOOT-LADDER-PLAYBOOK.md`, `aki-recomp/games/NWXE/profile.toml`
rung comments), the mechanically-extracted ABI surface
(`aki-recomp/runtime/ABI-SURFACE.md` / `abi_surface.json`), and the public
libultra manual. No GPL runtime implementation code was read to write this.

## 1. Crate layout

```
fn64-runtime   core: scheduler, OSMesgQueue, timers, PI/SI/VI/AI plumbing, rdram model, overlays
fn64-abi       the extern "C" surface recompiled code links against
fn64-boot-harness shared generated-section bridge/registration and ABI-sized rdram allocation
fn64-shell     the executable: window, input, audio out, ROM/RecompiledFuncs intake
fn64-render-ir GPU/runtime-independent render semantics, bounded replay records, and move-only effect ownership
fn64-render-conformance Rust-decoded replay fixtures and private-authority evaluation
fn64-render    backend-neutral render seam, exact microcode admission, and diagnostic raw-DPC inspection
fn64-render-reference deterministic pure-Rust ReferenceBackend
fn64-render-rt64 FFI bridge to RT64 (C++)
fn64-render-wgpu pure-Rust wgpu backend; bounded M3.1 lifecycle fixture today
fn64-certification executable behavioral evidence gates over the public renderer seams
fn64-cpu-runtime  linked typed execution runtime for generated VR4300 Rust runners
fn64-cpu-runtime-codegen  build-side Rust emitter and whole-ROM driver (§1.1's `rs` lane)
fn64-recomp  N64Recomp adapter for the comparison lane
fn64-audio     RSP audio ucode execution
fn64-diff      the first-divergence comparator, pure/no-I/O (§4's comparator lane)
fn64-timing-trace producer-neutral typed device-timing wire and DeviceFabric capture adapter
fn64-discover  ROM discovery: symbol/section metadata without a decomp (Phase D)
```

(`fn64-rt64` is this doc's older name for `fn64-render-rt64`; the crate is
`fn64-render-rt64` and §1's later prose still uses the short form.)

Dependency direction is strictly one-way:

```
fn64-shell ──depends on──> fn64-abi ──depends on──> fn64-runtime
    │                                                    ^
    └──────────────────depends on───────────────────────┘
    └──depends on──> fn64-boot-harness ──depends on──> fn64-abi + fn64-runtime
    ├──depends on──> fn64-render-reference ──depends on──> fn64-render ──depends on──> fn64-runtime + fn64-render-ir
    ├──depends on──> fn64-render-rt64 ────────depends on──> fn64-render
    └──depends on──> fn64-timing-trace ───────depends on──> fn64-runtime
fn64-render-wgpu ──depends on──> fn64-render-ir + wgpu
fn64-certification ──depends on──> fn64-render + fn64-render-reference + fn64-render-rt64 + fn64-runtime
fn64-render-ir (GPU/runtime independent; has no workspace dependencies)
fn64-render-conformance ──depends on──> fn64-render-ir
```

The first render-IR consumer is intentionally a synthetic raw-DPC integration
test, not a replacement for production DPC dispatch. After the ABI-side test
owner validates an exact owned DRAM command capture, `fn64-render` translates
it into one ephemeral `WorkloadPacket`/`DecodedTicket`. Three different owners
then hold the `SubmissionQueue`, reference-renderer
`BackendCompletionAuthority`, and ABI-side `GuestCommitAuthority`. The ABI
guest owner captures an exact-submission preimage while retaining an exclusive
borrow of that live allocation; moving its one immutable snapshot produces a
distinct transaction state that alone can commit. The reference adapter
installs each immutable stream only into that call's shadow image, executes
under transaction-local diagnostic isolation, receipts the exact declared
RDRAM effects, and discards its cloned backend even on success because this
first slice does not yet receipt persistent RDP/TMEM/hidden-bit state. Only a
completion matching the queue, submitted identity and ordinal, transaction
ordinal, byte length, and full live-memory preimage can obtain the guest
receipt and copy bytes back. Rejection or a dropped completion therefore
leaves live RDRAM, the backend template, process-global observations, and
diagnostic files unchanged.

`WorkloadRecord` is content-silent replay data and may be derived before
commit; that construction is not an architectural publication claim. Durable
semantic publication uses `CommittedSemanticWorkloadRecord`, whose private
construction requires a `GuestCommittedTicket`. Architectural raw-DPC
observation publication remains outside this synthetic slice and must acquire
the same committed authority when it is added. The IR ticket API currently
represents cancellation by consuming and dropping the ticket; it does not yet
have an explicit cancellation outcome type.

Production `dispatch_dpc_submission` remains on the compatible
`RenderBackend` atomic path. Its eventual migration order is renderer-staged
completion, `LiveDpcTransaction::commit`, committed semantic publication and
guest-effect application, then DP completion scheduling. The synthetic test
proves the authority placement and rollback mechanism only; it does not yet
prove that production scheduling order.

M3.1 adds `fn64-render-wgpu` as the first native GPU consumer of that ownership
model (`docs/RENDER-WGPU-PORT-PLAN.md`, M3.1). It is not wired into production
dispatch. Its only admitted packet is one synthetic 2x2 RGBA fill with an exact
`CMD_END -> FullSync -> DP interrupt` observation sequence and one 16-byte
journal-declared color-framebuffer effect. Its journal is exactly ordered as
operation-zero command read then operation-one framebuffer write; additional,
reordered, or renumbered accesses are not part of this fixture. Render-IR owns
the canonical effect-byte digest used by both the pre-existing M1.2 guest
staging adapter and wgpu, so identical bytes cannot acquire backend-specific
receipt identities. The backend exclusively borrows its prewarmed headless
device while one move-only `SubmittedTicket` is in flight, binds that semantic
identity to wgpu's opaque `SubmissionIndex`, and retains its paired
`BackendCompletionAuthority`. It issues a receipt only after the exact indexed
wait shape characterized by the M2.3 native probe, callback observation,
bounded map/readback, and exact byte comparison. This is lifecycle mechanism
evidence only: it does not claim general RDP decode, VI, presentation, RT64
parity, or performance.

M4.0 replaces the next whole-memory temptation with an owned deferred-read
boundary before broader TMEM work begins. Renderer preflight projects every
RDRAM `TmemLoadSource` access from the exact ordered `ResourceJournal` into a
move-only, renderer-neutral plan; it neither borrows nor reads RDRAM. The ABI
memory owner then bounds that plan against the installed physical layout and
copies only the named half-open ranges through fn64's canonical N64Recomp
byte-lane mapping into N64-logical-order owned bytes. Finalization consumes
the plan and capture and requires exact access index, operation ID, resource,
range, order, byte length, and content digest before it can construct a
`WorkloadPacket`. Runtime content identity and read-set comparison use
xxh3-128; the content-silent v3 replay encoding still carries stable per-read
SHA-256 digests, computed only when encoding and checked during cold replay.
The packet identity and replay record bind the resulting read-set identity and
per-read digests; replay must supply the same owned bytes again. A missing,
extra, reordered, overlapping substitute,
layout alias, short/long capture, or digest mutation therefore fails before a
packet or record can be retained. No renderer type retains an RDRAM pointer or
borrow, and the normal capture path allocates only the sum of declared reads,
not an eight-MiB snapshot.

Version 3 is an intentional wire break: its magic, version, workload identity,
and integrity domain all changed together, and the decoder rejects v2 records
rather than treating a record without a guest-read identity as an empty plan.
The cross-language replay golden is mechanically rebound to the v3 bytes;
retained v2 evidence remains interpretable only through its pinned v2
verifier, never through this decoder.

This first slice is an executable synthetic cross-crate mechanism proof. It
does not migrate production DPC dispatch, decode texture commands, populate
TMEM, prove a GPU upload, change guest scheduling, or establish RT64 parity or
performance. `fn64-abi` owns only bounds, byte-lane translation, and capture;
the renderer remains the sole owner of which semantic reads are required.
Provenance: fn64's resource-journal contract above and the N64Recomp-generated
`MEM_*` storage mapping documented in this file's RDRAM section; no reference
runtime implementation was consulted.

**Production raw-DPC seam (T0, `fn64-render`/`fn64-render-ir`).** The
production-dispatch migration card (`docs/RENDER-WGPU-PORT-PLAN.md`) freezes
one transactional, TMEM-only, no-FullSync, no-guest-write vertical slice
driven through the real ABI raw-DPC ingress, as a minimal sealed/session
design (v11 interface freeze, superseding an earlier broader
public-constructor sketch and a generic-capsule sketch, both rejected for
widening the public forge surface).

Two role objects split one lifecycle: `new_raw_dpc_roles() -> (RawDpcAbiSession,
RawDpcBackendAuthority)`. `RawDpcAbiSession` (ABI-owned) holds the submission
queue, the guest-commit authority, and a diagnostic retirement ledger.
`RawDpcBackendAuthority` (backend-owned) holds the paired completion
authority; it enters the registered backend at concrete construction, not
through an object-safe `RenderBackend` method -- pairing is a one-time,
backend-concrete construction fact, not a per-call production operation. The
session stamps one `OwnedRawDpcCapture` with its own queue identity into a
`RawDpcPlanRequest`; the backend's sole route to a plan-writing capability is
`RawDpcBackendAuthority::begin_plan(request)`, which *consumes* the request
by value (one stamped request cannot mint two writers or two plans) and
traps immediately -- before any plan field can be written -- if the request
was not stamped by the paired queue.

The returned `ExactRawDpcPlanWriter` is a private-field, push-only handle
(`push_tmem_load`, `push_state`, `push_command_decode_access`). Its sole
`finish(journal)` first proves the writer's own accumulated access list
equals `journal`'s ordered access list exactly -- same count, same order,
same `ResourceAccess` identity -- rejecting a missing, extra, reordered, or
mutated access with a named `ValidationError` before any plan exists; it
then derives both the source identity and journal identity from the
writer's own request capture and that same journal (never from a
caller-supplied identity parameter that could disagree with what was
actually pushed), builds the matching preflight internally, and returns the
sealed `PlannedRawDpcSubmission`. There is no public constructor for
`PlannedRawDpcSubmission`, `ExactValidatedRawDpcPlan`, an owned
semantic-command builder enum, or any bare ticket type; the only route onward
is `RawDpcAbiSession::finalize_and_submit`, which owns queue readiness and
ticket issuance entirely inside the session so a bare `DecodedTicket`/
`SubmittedTicket` never escapes to a caller, returning only the sealed
`BoundSubmittedRawDpc` -- the session still records a diagnostic
`RawDpcRetirementHandle` into its own ledger for this ordinal, but does not
hand a second copy of it back to the caller; same-module test code inspects
the ledger directly instead. Neither `PlannedRawDpcSubmission` nor
`BoundSubmittedRawDpc` exposes a plan-visiting method to a generic caller:
`BoundSubmittedRawDpc::execution_view` is the sole route, and it is
paired-authority-checked, nonextracting, and statically dispatched (a
generic `<V: RawDpcExecutionView<PV>>` parameter, never `&mut dyn Visitor`)
rather than a getter -- ABI never gets a plan-extraction surface it can hold
onto past the call.

`BoundSubmittedRawDpc::into_backend_prepared(&mut RawDpcBackendAuthority,
BackendEffectReport)` is the sole unseal route: it validates the exact paired
authority queue identity before moving any field (a mismatch loudly traps;
the still-sealed value then drops normally, recording exactly one `Rejected`
and exposing no parts), then consumes the submitted ticket internally via the
paired authority's own `issue`, so the resulting `GpuCompleteTicket` can only
ever be the one this exact submission produced. It carries no physical-state
field or identity of any kind -- the backend's own physical candidate lives
entirely in `RawDpcCoordinator`'s double-buffered slots (below), never in a
value this method returns. The result, `BackendPreparedRawDpc`, retains the
GPU-completion ticket and exposes only `stage()`/`submission()` facts -- no
plan-visiting method, no `complete()` getter.

**`RawDpcCoordinator<P>` -- the physical-state ownership fix.** An earlier
draft threaded a `RawDpcReadyPhysicalIdentity` (a bare, publicly constructible
content-digest value) through every typestate and had the terminal publish
step compare a caller-supplied copy against it. Independent review correctly
rejected that shape: identity equality alone is not proof that a physical
mutation happened, so any caller could echo a matching digest back without
ever performing it. The fix moves physical-state *ownership*, not just its
identity, into `fn64-render`: `RawDpcBackendAuthority::into_coordinator<P>(self,
initial: P) -> RawDpcCoordinator<P>` consumes the paired authority into a
coordinator generic over the backend's own physical state type `P` (a plain
owned value -- for a real backend, its concrete `wgpu`-side TMEM/texture
state -- never a callback or trait object). The coordinator owns `P` in two
slots (`[Option<P>; 2]`) plus an `active: u8` index; `physical()` always
returns the currently-published slot.

**Why two slots, not `mem::replace`.** `P` is an arbitrary backend type whose
`Drop` this module does not control. Replacing the *active* slot in place
would run the old active `P`'s destructor at the exact instant a new
candidate becomes current -- precisely the moment
`ReadyPublication::commit`'s straight-line body must be Drop-free. With two
slots, `RawDpcCoordinator::complete_execution(&mut self, BoundSubmittedRawDpc,
BackendEffectReport, next_physical: P) -> Result<BackendPreparedRawDpc,
ValidationError>` -- the coordinator's own wrapper around
`into_backend_prepared` -- overwrites the currently-*inactive* slot with
`next_physical`, dropping whatever `P` used to occupy it right there, inside
this ordinary fallible method, before any publication exists. `commit` later
only ever flips the `active` index, an integer write; a colocated test
(`old_inactive_slot_physical_state_drops_during_complete_execution_not_during_commit`)
proves this by tracking exactly when a droppable `P`'s destructor runs across
two successive `complete_execution` calls on the same never-flipped
coordinator. `complete_execution` also records private `(queue, submission,
inactive slot index)` metadata for this ordinal, consumed exactly once by the
matching `prepare_publication` call.

`RawDpcAbiSession::commit_zero_guest_writes` is unchanged in shape from
before this fix (card v10 section 1 point 2): it *consumes*
`BackendPreparedRawDpc` and returns the sealed `GuestCommittedRawDpc`, moving
plan and retirement together (no physical-state field to carry, since none
ever entered `BackendPreparedRawDpc`). Before issuing the guest-commit
receipt it hands `GuestCommitEffectReport::try_new` an empty write list
against the prepared ticket's own packet, which re-derives that packet's
actual `guest_write_accesses()` and requires an exact-length match -- a
defensive re-check independent of what this method is named, proven by a
colocated test that hand-builds a ticket with a real guest-visible write
(bypassing the normal writer path, which can never produce one) and confirms
this re-check rejects it.

`RawDpcAbiSession::seal_publication(GuestCommittedRawDpc,
fn64_runtime::device::ReadyDpcFabricCommit<'a>) ->
Result<ReadyRawDpcCommitCapsule<'a>, ValidationError>` -- v11's exact
signature -- seals a guest-committed submission against the concrete,
backend-retained T2 fabric-ready value and advances retirement to
`FabricPrepare`, the boundary where a real fabric commit is in hand but no
physical mutation has happened yet. It validates only what this session alone
owns (`committed`'s queue against the session's own); full
authority/submission/ready-slot validation is deliberately deferred to
`RawDpcCoordinator::prepare_publication`, the one place backend-owned
physical state is actually available.

`RawDpcCoordinator::prepare_publication(&mut self, ReadyRawDpcCommitCapsule<'a>)
-> ReadyPublication<'_, 'a, P>` is where v11's "validate authority, queue,
submission, proposal, and ready-slot identity while durable state is
unchanged" actually happens: it looks up (and consumes) the private ready-slot
metadata `complete_execution` recorded for this exact submission -- queue,
submission, and (the strongest of the three) a private `Arc::clone` of the
exact retirement slot `complete_execution` observed, checked via
`Arc::ptr_eq` against the capsule's own retirement -- and traps if any
disagree (no legitimate `complete_execution` call preceded this
`prepare_publication` call, the queue disagrees, or this capsule is not the
one the ready slot was actually prepared for). Only then does it advance
retirement to `PhysicalPrepare` and construct a `ReadyPublication` borrowing
the coordinator, holding the matched slot index and the capsule together.
The `PhysicalPrepare` advance happens here, inside `prepare_publication`,
*before* a `ReadyPublication` is ever returned -- not inside `commit` --
so that stage is observable the instant a caller holds one, whether or not
`commit` is ever called. That private retirement-slot clone (kept on the
coordinator's own ready-slot record, never on the capsule) is also what
lets a coordinator notice and reap an abandoned/rejected candidate's slot
without a public capsule-side observation accessor a caller could otherwise
reach the same state through -- an earlier draft exposed a public
`ReadyRawDpcCommitCapsule::retirement_handle()` for exactly that purpose;
it is removed, since nothing outside this module needs it once the
coordinator holds its own private clone. By the time a `ReadyPublication`
exists, every check has already passed -- there is nothing left to
validate.

`ReadyPublication::commit(self) -> CommittedRawDpcOutcome` is the sole
terminal step, and the only method anywhere that can produce
`CommittedRawDpcOutcome`: it flips `coordinator.active` to the already-checked
slot index (the first, and only, durable physical move), commits the inner
fabric transition infallibly, and unconditionally disarms retirement as
`Published`. `commit` performs no stage advance of its own -- retirement is
already at `PhysicalPrepare` by the time `commit` runs, set by
`prepare_publication` before this value existed -- so no callback, trait
object, allocation, lookup, `assert`, `Result`, `stage` write, or `Drop` of
`P` runs after the flip. `ReadyRawDpcCommitCapsule`
itself exposes **no bare public route to `Published`** -- no `commit`, no
other `CommittedRawDpcOutcome`-returning method -- closing the exact
fabric-only-terminal-route hazard the digest-based design had; a colocated
source-shape test asserts by name that the capsule's own `impl` block has
zero methods returning `CommittedRawDpcOutcome` and that
`ReadyPublication::commit` is the sole one anywhere in the module. Dropping
an unconsumed `ReadyPublication` (or the capsule it wraps, before
`prepare_publication`) cancels: `ReadyPublication` itself borrows rather than
owns the coordinator, so its `Drop` runs no code of its own and `active`
stays untouched; the capsule's own `Drop` -- inherited from every earlier
typestate -- rolls back the inner fabric commit and records exactly one
`Rejected` at `FabricPrepare`.

Every issued ordinal owns a `SubmittedRawDpcRetirement`: a pre-created shared
`Arc<AtomicU8>` terminal slot, also retained by the
diagnostic `RawDpcRetirementHandle`. This type has no `Clone` impl and this
module's own source-shape sweep forbids `mem::forget`/`ManuallyDrop`, so the
*same* retirement -- and therefore the same shared slot -- moves by value
from `BoundSubmittedRawDpc` through `BackendPreparedRawDpc` into
`GuestCommittedRawDpc` and on into `ReadyRawDpcCommitCapsule`; a colocated
test proves the slot is the exact same `Arc` allocation (`Arc::ptr_eq`) across
that whole chain. Drop performs only "if empty, set `Rejected {stage,
submission}`"; it allocates nothing, takes no `RefCell` borrow, and cannot
panic during unwind. This exact-once guarantee is scoped to submissions that
entered through `RawDpcAbiSession`: `fn64-render-ir`'s public,
admission-agnostic `DecodedTicket::new` and
`TicketAuthoritySet::submit`/`SubmissionQueue::submit` remain callable
outside the session (v11 explicitly keeps them public), and a raw-DPC ticket
minted that way is intentionally outside this ledger, exactly as under v10.

No `Any`/`TypeId`/downcast/`FnOnce` callback exists anywhere in this seam --
in particular, no generic trait stands in for a concrete backend authority,
fabric-commit, or physical-state type (`RawDpcCoordinator<P>`/
`ReadyPublication<P>` are generic over a plain owned `P`, never a `dyn`
anything); the colocated source-shape sweep enforces this too. `RenderBackend`
has exactly four object-safe raw-DPC methods: `raw_dpc_ir_capability`,
`plan_raw_dpc(RawDpcPlanRequest) -> Result<PlannedRawDpcSubmission,
RenderError>`, `execute_raw_dpc(BoundSubmittedRawDpc) ->
Result<BackendPreparedRawDpc, RenderError>`, and
`publish_raw_dpc(ReadyRawDpcCommitCapsule<'_>) -> CommittedRawDpcOutcome` --
the last with no `Result` in its signature, matching v11 exactly, and its
own object-safe shape unchanged from earlier drafts (only what a conforming
backend does *inside* it changed). The first three have loud, named-error
defaults; `publish_raw_dpc`'s default instead drops the capsule (cancelling
its fabric commit and recording `Rejected`, never `Published`) and panics,
since there is no `Result` arm available to report "unsupported" and this
default is architecturally unreachable in practice -- a capsule cannot exist
unless `execute_raw_dpc` already succeeded against a real, capable backend.
This keeps every existing `RenderBackend` implementor across the workspace
unrelated to raw-DPC production (test mocks, other backends) compiling
without adding a fourth required method to each of them. A real
raw-DPC-capable backend instead stores a `RawDpcCoordinator<P>` (`P` = its own
physical state type) and implements `publish_raw_dpc` as exactly
`self.coordinator.prepare_publication(publication).commit()`. There is
deliberately no `install_raw_dpc_backend_authority` object-safe method, since
v11 moves that pairing to concrete backend construction (now: the same
construction site that calls `into_coordinator`). The call into any of these
four methods through `dyn RenderBackend` is the sole dynamic dispatch in the
raw-DPC production path; everything from validating authority/queue/
submission/ready-slot identity through the terminal state transition,
including `ReadyPublication::commit`'s fixed consuming body, is monomorphic
Rust with no further vtable call. This holds only because `fn64-render`/
`fn64-render-wgpu` do not, and must never, depend on `fn64-abi` -- a
load-bearing reentrancy guarantee (not a hygiene preference) against
`fn64-abi`'s `with_host`/`with_executor` gateway, which has already produced
a real nested-reentry panic through an analogous path.

This T0 slice defines the neutral vocabulary, sealed session/writer/typestate
seam, the generic physical-state coordinator, and the real terminal
capsule/publish transition, sealed against the concrete
`fn64_runtime::device::ReadyDpcFabricCommit` T2 landed. That capsule's
readiness check binds to the plan's *captured* source identity, never live
device state: a public STATUS-mode command may legitimately change XBUS
selection while an already-admitted DPC transaction is still in flight, and
`fn64-runtime`'s `commit_dpc_submission` deliberately preserves the pending
submission's captured source rather than re-reading live XBUS -- a
live-XBUS-equality gate at publish time would be a correctness bug, not a
stricter check; neither `seal_publication` nor `prepare_publication`/`commit`
reads any live DPC/XBUS register as a validation gate. T0 does not implement
`fn64-render-wgpu`'s decoder (T1) or instantiate a real backend's
`RawDpcCoordinator<P>` with `P` = concrete `wgpu` physical state (T3, which
also owns `PendingTmemTransaction::into_physical_successor` and proposed
logical RDP state/durable before-after identity -- T0 provides only the
generic coordinator mechanism, not a backend state payload or its own
generation-tracking logic); see the migration card's ticket DAG and T0's
freeze report for the exact scoped boundary.

**T3 Phase A/B -- the concrete `wgpu` backend.** T3 Phase A adds
`PendingTmemTransaction::into_physical_successor(&self, base: &PhysicalTmemState,
effects: &fn64_render_ir::BackendEffectReport) -> Result<PhysicalTmemState,
PhysicalTmemError>`: the exact `next_physical` shape
`RawDpcCoordinator::complete_execution` needs, produced without touching
`base` and never durably published until a later `commit` flips a
coordinator's active slot to it. It runs the identical three ordinal checks
`PhysicalTmemPublicationAuthority::publish` runs
(`CrossStatePublication`/`StaleBaseGeneration`/`StaleLoadEpoch`), then a new
`BackendEffectMismatch` check that the backend report's declared writes
exactly match this transaction's own proposed effects. The move-only pending
transaction's only constructor establishes its internal projection/digest
consistency and its fields stay private and immutable, so this successor path
does not re-hash the same projected bytes. `FN64_REVALIDATE_SEALED_TMEM=1`
restores that redundant audit for diagnostic runs; it does not relax any
base-state or externally supplied backend-effect check.

T3 Phase B adds `fn64-render-wgpu`'s `production` module: a concrete
`WgpuBackend` owning `fn64_render::RawDpcCoordinator<PhysicalTmemState>`,
obtained exactly once at construction via `RawDpcBackendAuthority::
into_coordinator`. `plan_raw_dpc` decodes a `RawDpcPlanRequest`'s capture
through T1's real decoder (`crate::decode_raw_dpc`) and T1's push loop
(`crate::raw_dpc::push_decoded_raw_dpc`) into a sealed
`PlannedRawDpcSubmission`, using the same two-pass journal probe T1's own
tests use (decode once against a throwaway single-source journal, read the
real access list back off `RawDpcDecodeError::JournalMismatch::expected`,
decode again for real) since the exact journal `ExactRawDpcPlanWriter::finish`
requires is not knowable before a first decode attempt. `WgpuBackend` also
carries a durable `RdpState`, updated via each successful plan's own
`RdpStateDelta`, so a submission's `SetTile`/`SetTextureImage`/`SyncLoad`
state depends correctly on what an earlier submission staged rather than
re-decoding from a fresh default every time.

`execute_raw_dpc` reaches plan contents exclusively through
`BoundSubmittedRawDpc`'s authority-scoped, nonextracting `execution_view` --
never a bare `SubmittedTicket` or the private decoder's own
`BoundTmemTransfer`. This exposed one genuine seam gap: `PhysicalTmemState`'s
existing `stage_transfer` (and its `PhysicalTmemPacketTransaction` chaining
counterpart) is hard-typed to that decoder-owned pair, which a production
`execute_raw_dpc` implementation cannot obtain by design. T3 Phase B adds an
additive, crate-private neutral counterpart --
`PhysicalTmemState::stage_neutral_transfer`/`PhysicalTmemPacketTransaction::
stage_neutral_transfer_next` -- that performs the identical checks
(destination-access/physical-word coverage via the same
`validate_physical_plan`, epoch ordering, cross-state/generation binding)
against `fn64_render::TmemLoadSemantics`'s neutral fields (which mirror the
private decoder types field-for-field) instead. It reuses
`PhysicalTmemState`'s existing `stage_word`/`finish_load`/`into_pending`
unchanged, and reuses (via widened `pub(crate)` visibility, not
reimplementation) the exact same tested LoadBlock/LoadTile/LoadTLUT
byte-to-physical-lane mapping functions the decoder-typed executors already
use -- LoadBlock and LoadTile share one identical mapping (both are pure
linear/split-bank fragment placement with no quadrication), so only one copy
is reused for both. `publish_raw_dpc` is implemented as exactly
`self.coordinator.prepare_publication(publication).commit()`, matching v11's
frozen shape: one non-`Result`, callback-free terminal path that flips the
physical slot, commits the concrete fabric transition, and records
`Published` together.

Scope, matching the T3 ticket DAG and card v11 exactly: TMEM-only,
no-FullSync, no-guest-write raw-DPC execution/publication, headless only. No
visible presentation, no raster parity, no native GPU testing.
`WgpuBackend::process_task`/`present` are honest, named `RenderError`
rejections rather than invented gfx-task/presentation behavior -- the landed
`RenderBackend` trait requires both as non-defaulted methods, but this slice
proves only the raw-DPC production seam. T3 Phase B does not itself wire any
ABI producer to this seam; that is T4, below.

**T4 -- real ABI raw-DPC ingress (`fn64-abi`).** Wires all three concrete
production raw-DPC producers -- sp_dp DRAM (`sp_dp.rs`), MMIO DRAM/XBUS
(`pi/mmio.rs`, both sources), and RSP XBUS (the coalesced pending-submission
loop inside `dispatch_lle_task`, `task_dispatch/rsp_commit.rs`) -- through
the T3 `plan_raw_dpc -> finalize_and_submit -> execute_raw_dpc ->
commit_zero_guest_writes -> seal_publication -> publish_raw_dpc` conveyor,
conditionally: only when a `RawDpcAbiSession` is registered
(`set_raw_dpc_session`, paired at construction with a concrete backend via
`fn64_render::new_raw_dpc_roles`, exactly as `RawDpcBackendAuthority::
into_coordinator`'s own doc comment requires). No session registered (the
default, and what `Rt64Backend` always uses) is byte-for-byte the pre-T4
legacy atomic `process_rdp_commands` path, unchanged.

`fn64-abi` never depends on a concrete backend crate to do this --
`RawDpcAbiSession` is a `fn64-render` type, so the ABI layer stays
backend-agnostic per this document's crate-layout rule; only a shell or test
harness ever names `WgpuBackend` and registers the paired session. The
routing decision (`session_registered`) is checked before
`LiveDpcTransaction::new` runs at every one of the three call sites, so a
submission is never claimed by the T4 path and then abandoned back to the
legacy path -- either it is taken by the session path in full (through to
`publish_raw_dpc`) or the legacy path owns it from the start, never both.

Guest-read bytes are sourced from live RDRAM for both DRAM- and
XBUS-sourced captures: every admitted TMEM load's source access is
`RdramResource::Buffer` regardless of which bus carried the command stream
(`raw_dpc::production_adapter`'s push loop; XBUS changes only where RDP
*command words* come from, never where `LoadBlock`/`LoadTile`/`LoadTLUT`
read texel data). `OwnedRawDpcCapture` preserves the exact original
source/range/bytes with no synthetic staging suffix, unlike the legacy
`dispatch_captured_raw_rdp` path. `plan_raw_dpc` rejects `FullSync` and any
command outside the admitted TMEM/state subset loudly (a named panic, never
a silent downgrade to the legacy path); `commit_zero_guest_writes`
independently re-rejects any guest-visible write. Fixed this slice: T3
Phase B's `single_source_probe_journal` always declared its command-decode
access as an RDRAM `RawCommands` region, which `fn64-render-ir`'s
one-to-one command-read validation rejects for a genuinely XBUS-sourced
stream -- every XBUS producer would have panicked on its first
`plan_raw_dpc` call once a T4 session was registered. Now branches on
`submission.source()` (`ResourceRegion::RspDmem(DmemRange)` for XBUS).

Nonclaims, unchanged from T3 Phase B: TMEM-only (no raster/combiner/blend),
no visible presentation, no native GPU testing, no RT64/Rt64Backend
migration (that backend keeps the legacy path unconditionally, since it
never implements `plan_raw_dpc`/`execute_raw_dpc`/`publish_raw_dpc`), and
no shell wiring -- no shell in this workspace constructs a `WgpuBackend` or
calls `set_raw_dpc_session` in production; that registration is exercised
only by `fn64-abi`'s own dev-dependency test suite
(`task_dispatch::tests::raw_dpc_session_integration`).

`fn64-runtime` depends on nothing else in this workspace. It is pure, safe
Rust: the scheduler, message-queue semantics, timer wheel, rdram buffer
ownership, and the diagnostic/watch hooks. It has no knowledge that it is
being called from generated C, and no knowledge of RT64. This is what makes
it independently testable (unit tests drive scheduler/queue invariants with
no ABI or graphics involved) and is also the reuse seam: any future
recompiler backend (not just N64Recomp's C) links against the same core.

`fn64-abi` depends on `fn64-runtime` only. It is the thin, mechanically-
checkable translation layer: every `#[no_mangle] extern "C"` symbol
generated `RecompiledFuncs/*.c` calls (`recomp.h` dispatch helpers and the
`_recomp` shim inventory) lives here,
each one a direct call into an `fn64-runtime` API. This crate is deliberately
"dumb" -- if a function's `fn64-runtime` counterpart already exists, its
`fn64-abi` wrapper is a signature-and-marshalling adapter, not a place new
policy gets invented. Reviewing `fn64-abi` in isolation should answer "does
this match ABI-SURFACE.md" without needing runtime-internals knowledge.

`fn64-boot-harness` depends on `fn64-abi` and `fn64-runtime`. It owns the
game-agnostic generated-C boot boundary shared by `fn64-shell` and the
headless boot examples: the clean-room `recomp_overlays.inl` bridge, its
`fn64_register_func` callback and section accumulator, registration-order
adapter, generated `recomp_entrypoint` declaration, and the one RDRAM
allocation sized for physical RDRAM plus the raw MMIO/non-RDRAM window. Which
sections begin resident and all input/save/render/audio policy remain local
to each harness because those choices differ by game and host.
The allocation API requires a typed `TvType` (`Pal`/`Ntsc`/`Mpal`) and installs
the IPL-owned `osTvType`, `osRomBase`, and `osResetType` globals before thread 0
runs. New process allocations explicitly select the public cold-reset value;
`osRomBase` receives the cartridge's canonical KSEG1 base. A zero-filled buffer
is not valid console boot state: generated initialization reads these globals
before any ABI shim can repair them.
The same seam reproduces IPL3's initial DMA—one MiB from cartridge ROM
`0x1000` to RDRAM `0x400`—so translated CPU execution and hardware DMA
consumers such as rspboot observe one resident boot image. Once game policy
selects the registered sections that begin resident, the harness also copies
each section's declared ROM range to its static RDRAM range. That geometry
comes directly from the generated section table; overlay sections remain
unloaded until the game requests their DMA.

The ABI task loader retains a typed CPU-side image for each immutable
`OSTask::ucode_boot` range and re-DMAs that image at every `osSpTaskLoad`.
This represents the public R4300/RSP cache-coherency contract at the seam where
generated CPU accesses and physical device DMA otherwise share one host
allocation: a CIC/custom task may write its response over rspboot's physical
DRAM bytes while the CPU cache still owns the boot text used by the next task.
The device write remains visible in RDRAM; only the subsequent boot-code DMA
uses the retained CPU image.

`fn64-shell` depends on `fn64-abi`, `fn64-runtime`, `fn64-timing-trace`, and
`fn64-rt64`. It owns
the parts every recompiled game needs but that aren't part of the libultra
ABI surface itself: windowing, input device polling, audio output backend,
loading a user's own locally-recompiled ROM output (per `README.md`'s "no
game content in this repo" rule -- the shell is where a user's own build
artifacts get linked/loaded, never anything checked into fn64).

For bounded differential timing runs, `FN64_DEVICE_TIMING_TRACE` selects an
absolute, not-yet-existing JSONL output path and
`FN64_DEVICE_TIMING_TRACE_ID` supplies the nonempty identity shared with the
reference run. `FN64_DEVICE_TRACE_SCOPE` optionally selects a unique subset
of `pi,ai,si,sp,vi,mi`; omitted selects all six. The shell parses this
configuration once at boot and seals the runtime's already-cycle-stamped
device trace before ABI teardown. It uses create-new output and fails loudly
rather than overwriting evidence. The wire's first retained event is cycle
zero, so independently launched producers compare relative device timing
without claiming a shared boot-observation instant. The trace contains event
metadata only, never framebuffer, PCM, ROM, or other game bytes.

`gate_timing_diff` compares two fully ingested instances of that wire without
depending on either producer's implementation. A verdict requires the same
opaque trace identity, distinct producer labels, equal schema, clock basis,
and observation scope, nonempty event evidence, and two completed envelopes.
Event kind, device, and applicable payload align strictly by position; the
first mismatch is the first semantic divergence, with no guessed
resynchronization. Cycle bands account for both timestamp quanta: a possible
delta wholly inside the band agrees, one wholly outside diverges, and an
interval that straddles the band is refused as ambiguous. Missing end records
are rejected by ingest and declared aborted captures are refused, so a shared
truncated prefix cannot become agreement. Producer labels and the caller's
opaque trace identity establish comparison pairing, not independent proof of
ROM, input, or reference-emulator provenance.

The shell's audio backend keeps two clocks and two queue views explicit. AI
DMA buffers arrive at the exact typed `VI_CLOCK / (DACRATE + 1)` rational;
the floored rate returned by `osAiSetFrequency` remains guest ABI and telemetry
metadata rather than resampling authority. cpal may run at a different device
rate and resamples at that boundary without accumulating an integer-Hz
truncation. `AI_LEN`
reports only the current emulated DMA. The host output prebuffer is separate,
retains the bounded ring used to absorb callback jitter, and starts only when
one accepted payload is also the active hardware DMA. A second queued but
CONTROL-gated or FIFO-waiting payload cannot start host playback. An idle
enabled AI starts its first accepted DMA immediately in emulated time.
Letting host depth leak into `AI_LEN` would make guest buffer sizing depend on
host latency rather than N64 hardware state. The host ring allocates its full
250 ms bound before playback and keeps the producer's drop-oldest policy. The
realtime callback never waits for the producer lock: a contended pull becomes
counted silence, preserving callback deadlines without changing DMA progress.
Stream health reports those contention slots separately from all other
host-inserted silence and counts producer-dropped slots. A busy lock does not
prove whether the ring held PCM, so the counters deliberately do not label the
remaining silence as an exact empty-ring population.
AI FIFO admission and DAC start are separate typed events. Each accepted
buffer receives a monotonic `AiDmaId`; an idle enabled FIFO reports its start
with admission, a CONTROL-gated buffer reports the later enable edge, and a
second-slot buffer reports the exact completion-cycle promotion. PCM is still
copied at admission, before guest RDRAM can change. Every admitted DMA also
carries a frame-zero presentation marker through the stateful resampler and
host ring. When cpal consumes that marker, its callback timestamp predicts the
host playback `Instant`; a bounded atomic slot joins that observation to the
matching typed `AiDmaStarted` instant. Start-first and callback-first are both
valid and neither can publish a half-anchor. Underrun, contention, eviction,
retiming, or stream error invalidates the current continuity generation.
The backend reports that generation even while it has no complete anchor, so a
joined presentation diagnostic can discard the prior correlation immediately
and wait for a later DMA to establish a complete anchor in the new generation.

The shell maps exact scheduled VI instants through one fixed emulated-cycle to
host-wall epoch rather than accumulating rounded field durations or rebasing
after slow work. Audio presentation anchors are correlation measurements only:
callback-inserted silence or host buffering must not feed back into VI or guest
pace. The callback never advances or retimes the executor, AI, VI, a timer, or
any guest-visible clock.

The opt-in
`FN64_AV_SYNC_PROBE` selects the first above-threshold stereo frame after a
configurable quiet interval (`FN64_AV_SYNC_QUIET_MS`, with
`FN64_AV_SYNC_THRESHOLD` selecting the sample magnitude), carries that
identity and source-frame position
through the stateful sinc filter, and records the exact callback slot that
crosses it. The callback converts cpal's predicted callback-to-playback stream
timestamp plus that intra-buffer frame offset into one wall `Instant` without
locking. Ring eviction and live DACRATE retiming are explicit invalidation
flags; neither silently produces a phase claim. This landmark diagnoses audio
production/start/delivery. The title-neutral video half is selected with
`FN64_AV_SYNC_VIDEO_HASH` and an optional one-based
`FN64_AV_SYNC_VIDEO_OCCURRENCE`. It counts only new renderer-owned
presentations, never expose redraws, and settles only after the corresponding
window submission succeeds. The result binds the RGBA hash, explicit source
or post-VI stage, and stage-specific presentation generation to the exact typed
VI-edge cycle carried by that renderer request and to the post-submit wall
`Instant`. `fn64.host-presentation.v10` serializes that stage, a neutral
`presentation_generation`, and the renderer-batch and admission-keyed guest-task
observations described below. Renderer-worker records carry scheduled thread
CPU duration when the host exposes that clock; wall minus thread CPU is labeled
non-CPU wall and does not by itself distinguish blocking from preemption. When
`FN64_AV_SYNC_CUE_ID` supplies an opaque
experiment identity, v10
also records the exact audio and video halves and emits their rational guest
cycle and signed host-time pair only if the callback's audio-continuity
generation is still current. It requires both exact probes; the runtime does
not infer correspondence from nearest timestamps. Schema v10 retains v9's
distinction between the
host stream's successful preactivation return from the first active DMA's
payload queue, emulated start, guest-authorized PCM-delivery activation, and
first delivery callback. Both v8 and v9 remain readable for existing traces;
v8 treats its first-DMA `play` return as the delivery boundary. Schemas before
v8 are rejected rather than silently
treated as complete: v1's `source_generation` cannot describe a post-VI Wgpu
field, v2 has no renderer-batch record contract, v3 has no exact-cue authority,
v4 has no execution-mechanism identity, and the independently developed v5
dialects each omit either per-admission lifecycle or exact-cue/startup authority.
Schema v6 has no renderer-worker CPU clock, so it cannot separate CPU-consuming
worker time from non-CPU wall time. Schema v7 has no per-callback underrun
timestamp or distinct renderer-scanout and window-submission spans, so it cannot
attribute lost audio continuity to the host owner active at that instant.
Schema v8 added those host-only observations. The realtime callback publishes
only content-free counters through a bounded allocation-free queue: empty and
short rings retain their exact pre-drain depth, while producer contention uses
a null depth because the callback could not inspect the ring. Queue loss is an
explicit record and makes attribution incomplete. Schema v9 also carries a
bounded calibration stream: each producer admission names its typed AI DMA id,
exact resampled sample-slot count, post-push ring depth, negotiated host rate,
and channel count, while the callback publishes only its first requested slot
count and later geometry changes. The
realtime producer uses `try_lock` and reports explicit loss rather than
allocating, waiting, or changing delivery. These records permit timestamp
correlation of an underrun with the preceding and following DMA admissions
without exposing PCM or making host buffering guest clock authority. Five
mutually exclusive host
activity labels (`waiting`, `guest_step`, `device_advance`, `vi_scanout`, and
`window_present`) diagnose ownership; they are not N64 states and never feed a
guest clock. One renderer VI operation span retains both source and post-VI
generation/availability outcomes because one backend call produces them and
at most one can be ready. Exact retrace/stage/generation identities join a
ready outcome to its successful window span and existing presented-field
record. Ready fields superseded by normal redraw suppression remain explicit
unsubmitted observations rather than malformed joins. The shell drains VI
observations after every pump, independent of redraw submission, so a long
suppressed interval cannot overflow the bounded producer.
Once
both halves settle, the shell
reports signed video-minus-audio deltas in guest cycles and host milliseconds;
a dropped or retimed audio landmark stays labeled and cannot silently become a
phase claim. Choosing corresponding cues and comparing them with an external
reference remain experiment inputs rather than title knowledge in the runtime.
The exact FNV RGBA identity is a lazy per-presentation authority. The frame
tripwire, frame-dump filename, enabled presentation trace, unsettled video-sync
probe, nonuniform first-frame log, and 60-swap heartbeat demand that same
cached value; ordinary intervening fields do not scan every RGBA byte merely
to discard an identity no consumer requested.
`FN64_AV_SYNC_FRAME_DUMP` may name a diagnostic-only directory outside the
repository; the shell writes its latest cached prior presentation when that
audio landmark settles. The next redraw has not occurred at this polling
point, so this is a visual observation aid, not the nearest-field claim or an
exact VI cue binding, and game-derived pixels remain forbidden from git.
Guest-order AI sample decoding reuses one thread-owned buffer across
submissions, and disabled audio diagnostics are launch-time cached values, so
the ordinary DMA path neither reallocates its sample vector nor scans the
process environment at buffer cadence.
That split is also enforced by the Rust seam: `AiSamplePeriod` carries the
device clock and DAC divisor without loss, guest and host whole-Hz rates remain
distinct nonzero compatibility types, `GuestPcm16` proves complete interleaved
frames, and
guest sample slots, host sample slots, host frames, and guest DMA bytes cannot
be passed interchangeably. Raw integers exist only at device, cpal, atomic,
and C-ABI edges where their representation is required.

`fn64-rt64` depends on `fn64-render`, which owns the backend-neutral task,
microcode-admission, runtime-policy, and raw-DPC completion seams, and on
`fn64-runtime` for persistent RSP memory and device value types. It is the ONLY
crate in the workspace permitted to contain C++ or call into RT64's C++ API.
Rationale, three reasons:

1. **License and language boundary are the same boundary.** RT64 is MIT but
   C++; keeping all `cxx`/`bindgen`/raw-FFI surface in one crate means a
   `cargo geiger`-style or manual audit of "where is this workspace not
   memory-safe Rust" has exactly one crate to look at, not a foreign-function
   call site sprinkled through the runtime.
2. **The gfx task handoff is explicitly an open question, not a settled
   contract** (`ABI-SURFACE.md` section (e): "the gfx task handoff signature
   that RT64 consumes is NOT visible from generated RecompiledFuncs C in this
   snapshot for either game -- no direct osSpTaskLoad/osSpTaskStartGo
   `_recomp` call site found... this is a real gap, not a resolved ABI
   point"). Quarantining the unresolved seam in its own crate means the
   uncertainty doesn't leak into `fn64-runtime`'s otherwise well-specified
   scheduler/queue model; when the real call shape is observed (a profile.toml
   rename reaching that call site), only `fn64-rt64` and the `fn64-abi` glue
   need to change.
3. **Independent buildability.** A contributor working on scheduler
   correctness should never need a C++ toolchain or RT64 checked out. Only
   building `fn64-shell` (which needs real graphics output) pulls in
   `fn64-rt64`; `cargo test -p fn64-runtime -p fn64-abi` stays pure-Rust and
   fast in CI.

#### License boundary: what the RT64 wrap is allowed to link (LIVE CONSTRAINT)

fn64's linked binary stays MIT/Apache-clean **only because the RT64 build is
scoped to its render/HLE path.** This is a standing build constraint, not a
one-time finding — re-check it whenever the RT64 pin moves or the wrapper's
CMake scope changes.

**The rule: build RT64 as a static lib for its render/HLE target only, and do
NOT enable the mupen64plus plugin target.** `mupen64plus-core` is **GPLv2**
(source: `third_party/rt64/src/contrib/mupen64plus-core/LICENSES`, "licensed
under the GNU General Public License version 2"). It is not linked today:
RT64 puts only `.../mupen64plus-core/src/api` on the *include path*
(`third_party/rt64/CMakeLists.txt:421`) to consume the mupen plugin ABI
headers (`m64p_*` descriptor types), and no RT64 source outside `contrib/`
`#include`s an `m64p` header in the evaluated tree. That include exists for
RT64's own future emulator-plugin build — a feature its `README.md:6` says is
"not available in this repository yet." Enabling that target is what would
pull GPL into fn64's binary.

Everything else RT64 links is permissive, audited 2026-07-16 against the
`no-mercy-recompiled/third_party/rt64` @ `f0728a2` checkout: RT64 itself MIT
(`third_party/rt64/LICENSE`); `plume` GPU abstraction + `re-spirv` MIT
(`src/contrib/plume/LICENSE`, `src/contrib/re-spirv/LICENSE`);
`imgui`/`implot`/`im3d`/`hlslpp`/`VulkanMemoryAllocator`/`stb`/`ddspp` MIT or
public-domain; `xxHash`/`zstd` BSD; `nativefiledialog-extended` Zlib;
`spirv-cross` Apache-2.0; `dxc` LLVM/Apache-with-exception and build-time only
(a shader compiler binary, not linked into the runtime —
`third_party/rt64/CMakeLists.txt:39-61`).

The same one-crate quarantine that bounds the unsafe audit bounds this license
audit: there is exactly one crate to check.

**Provenance note (clean-room):** RT64 is HLE "directly reverse engineered by
observing console behavior" (its `README.md`) — it studied *hardware output*,
not copyrighted game code. That is different in kind from the matching
decompilations this project rejects, and consuming it as an MIT dependency does
not touch fn64's own from-ROM-bytes provenance for game code. `raster.rs:17-24`
already cites RT64's MIT `shared/rt64_color_combiner.h` as its algorithm
source; reading MIT RT64 is an allowed source under AGENTS.md, GPL runtime
internals are not.

No longer planned-only: `fn64-cpu-runtime-codegen` emits typed Rust against the
linked `fn64-cpu-runtime` execution runtime. Generated shard crates depend only
on that runtime package, so an emitter-only edit cannot invalidate their
normal Cargo dependency edge. `fn64-recomp` remains the N64Recomp comparison
adapter. Together they provide the second lane below.

### 1.0 The outer boundary: fn64 owns its toolchain

The rules above govern crate-to-crate concerns. They say nothing about the
boundary between fn64 and everything outside it, and that omission has a
scar: a legacy sibling checkout (`aki-recomp`) became load-bearing without
violating a single written rule — it is not a crate, so dependency direction
never caught it; it is not C++, so the quarantine never caught it. It was
found 2026-07-17 and is being cut (ROADMAP Phase H). The rule that would have
prevented it:

**Everything needed to build and run fn64 lives in fn64, except a user's own
game content.** Exactly one class of input is legitimately out-of-tree — ROMs
and anything ROM-derived, which the no-game-content rule bars from git
forever. Everything else — recompiler configs, upstream MIT headers, tooling,
metadata — is either owned here, vendored here, or generated here.

Corollaries, each earned the hard way:

- **A path to another project is not a dependency mechanism.** If fn64 needs
  an artifact, vendor it, submodule it, or generate it. Reaching into a
  sibling working directory couples fn64 to one machine's layout and, worse,
  to another project's lifetime.
- **Out-of-tree inputs are named and declared, never defaulted to someone's
  home directory.** A default path that only resolves on the author's machine
  is a silent shrug: it works for exactly one person and fails or — worse —
  silently reads something stale for everyone else. (`native-emit.sh` did
  exactly this: it hashed a stale driver into a cache key when the repo-local
  and `CARGO_TARGET_DIR` copies diverged.)
- **Test/gate fixtures obey this too.** A gate whose inputs are compile-time
  `const` paths into a personal directory (`fn64-discover`'s `gate_*.rs`,
  ROADMAP H3) produces numbers exactly one person can reproduce. Unreproducible
  evidence is not evidence — see AGENTS.md's validation bars.

#### Private release execution is a typed authority boundary

Private admission and private execution are deliberately separate. Current
admission schema `fn64.private-input-admission.v7` validates local
ownership/provenance policy and content-addresses the
ROM, recompiled output, microcode pair, native host entry image, typed
program-build receipt, arguments, environment, fixed cycle, and expected
execution source. It also binds a retail-cartridge or public-homebrew class to
class-specific ROM provenance; the header cannot prove that class. Retained v6
manifests remain strictly read-only verifiable and cannot select v7's
F3DZEX2-characterization purpose or raw-window roles. The emitted
`fn64.private-release-run-contract.v3` is an
integrity wire, not a signature:
any caller can recompute a self-hash. Production runner APIs therefore accept
only an opaque `VerifiedPrivateReleaseRunContract`. Its loader runs the typed
in-process Rust policy over stable-captured bytes and replays the complete
v7/v6 manifest/readiness/receipt/contract mapping. Files are measured through
one no-follow descriptor or Windows handle, with object identity and metadata
checked before and after hashing and the path chain checked afterward. Python
remains a producer and differential oracle, not production loader authority. A
separate constructor
is confined by exact byte identities and typed fields to fn64's fixed non-game
`synthetic_mechanism` fixture and current test executable; arbitrary relabelled
input cannot authorize a capability.

The capability owns one exact-ten process series. It clears ambient
environment state, copies the verified native ELF/Mach-O/PE bytes to a
create-new executable beside the original, launches only that isolated stage,
sets the ROM and release tuple itself, derives ten event identities from an
OS-random nonce plus contract/child/ordinal/output context, validates each
durable report/journal pair before continuing, and persists a canonical
receipt only after all ten agree semantically. Script launchers and known
loader/interpreter/plugin injection variables are rejected. Input paths and
output directories remain private, non-symlink, and outside git (or explicitly
ignored).

The exact-stage boundary is local and single-owner: staged files are random,
create-new, read-only, and rehashed, but a malicious same-UID process capable
of chmod plus pathname replacement between verification and OS open/spawn is
outside scope. Renaming an external ancestor and restoring it with the same
identity remains outside the admission boundary as well.

Rehashing an admitted microcode or recompiled file proves only that it did not
change, not that the child consumed it. Admission schema v7 therefore requires
`fn64.release-program-build-receipt.v1` for `full_rom` and `combined`. The
receipt binds the exact child entry image and recomputes the declared execution
source from one typed lane: canonically labeled exact linked archives for a
native program, the generated typed-observed-function identity wire, or the
typed-block pack plus its expected live program identity. The private v3
contract binds the receipt itself, requires exactly one receipt lane input to
equal the admitted `recompiled` artifact, and requires both the declared and
recomputed source to equal the report source. The runner revalidates these
files before the series, before each child, after the final child, and during
retained-series verification. This is exact identity co-binding, not proof that
the child was compiled or linked from the lane inputs; that stronger claim
requires a trusted build/link record or external attestation.

Runtime task-start identity is separate from program-input identity. At the authoritative
graphics-task start, the ABI hashes the exact logical RDRAM bytes named by the
original task's microcode-data address and length and records that identity in
the same recognition event as the live 4 KiB IMEM digest and recognized
family. That family comes only from the selected backend's exact text/data-pair
catalog; text-only HLE recognition cannot populate release evidence. Overlay
recognition pairs each replacement IMEM generation with that
same original data identity; a yielded resume never promotes the rewritten
yield-buffer pointer to admitted microcode data. One typed lifecycle permits
`Running -> ResumeAuthorized -> ResumeLoaded -> Running`; ordinary completion
retires `Running`, and each authorization is load-consumed exactly once. Every production report in
the exact-ten series must contain at least one individual recognized event whose text SHA,
data length, and data SHA equal the admitted pair. Current report schema
`fn64.release-gate.v30` also freezes the install-once audio-task execution
policy and admits only execution of the live RSP image through `LleAccuracy`;
the
`fn64.rsp-rdp-observations.v2` wire bind those fields.

This mechanism makes a correctly formed production contract launchable; it is
not representative-ROM evidence by itself. Representative private NTSC
full-ROM exact-ten series for reference and RT64 LLE/post-VI completed under
schema v22 and were independently reverified locally on 2026-07-22. Both
series are historical under schema v29 and require regeneration. They bind
their then-current boundary-owned observations and the compiled unsupported-
instrumentation identity. A retained public synthetic identified-native XBUS
series binds the same denominator without acquiring private-ROM authority.
That series and its target-named fingerprint are schema-v28 historical
evidence and require v29 regeneration. Its
specialized runner operation exposes only a self-hashed receipt; repository
acceptance is instead anchored solely by the exact target-named macOS arm64
semantic fingerprint, including both build-produced archive hashes. Compiler,
SDK, or target drift fails closed until a separately reviewed golden exists.
Their combined incomplete matrix accepted all 30 reports, satisfied 12 of 162
requirements, and retained 150 explicit gaps. Self-hashed receipts are
retained integrity evidence, not transferable process attestation, and the
synthetic result cannot be promoted into private-ROM evidence.

Representative matrix verification preserves the same capability boundary.
Report-only matrix v5 verification never awards a ROM-class requirement from
the report's host-supplied label. Its private-series path accepts only an
opaque capability produced by jointly revalidating the policy-admitted v3
contract, exact-ten receipt, retained reports/journals, raw ROM, runner image,
and bound inputs. It exact-matches the v29 semantic report and ordered run-event
identities, and retains a canonical `fn64.verified-rom-class-authority.v1`
inside verified-matrix v18. The retained
self-hash proves canonical integrity, not signer identity or transferable
process provenance.

The current local 2026-07-22 v5 assessment jointly revalidated both private
representative series plus the public synthetic XBUS series as 30 reports and
retained the full 162-requirement denominator. It satisfied 12 assignments and
left 150 explicit; incomplete matrix verification did not discard or relabel
the missing requirements.

#### Instruction-exact savestate transplant is NOT REPRESENTABLE here (negative result, 2026-07-14)

This is an architecture fact about the runtime's shape, kept here because the
code that discovered it has been deleted (see below) and a future session must
not re-derive it the expensive way — or, worse, re-add a mupen64plus savestate
parser believing it closes a gap. It does not. There is no gap; there is a
representability wall.

fn64 (like N64Recomp itself) compiles each MIPS function to one native
function. `SectionRegistry::resolve` (`fn64-runtime/src/overlay.rs`) matches
only a vram address that is an EXACT function-entry offset, **by design**:
`LOOKUP_FUNC`'s only real call shape is a whole-function indirect call.
A savestate's saved PC lands wherever an instruction happened to be
executing — essentially never exactly at a function's first instruction.

Therefore true instruction-exact transplant ("resume at PC") is **not merely
unimplemented — it is not representable by a recompiler-shaped runtime at
all**, without either:

- (a) sub-function-granularity call targets, which N64Recomp's own codegen
  does not produce; or
- (b) a bytecode/threaded-interpreter fallback for the remainder of the
  interrupted function.

The deleted code was honest about this rather than faking it: its
`resolve_entry_point` reported the ENCLOSING function (nearest registered
function whose vram range contains the resume PC) plus the offset into it,
rather than silently pretending an exact resume had happened. Starting the
enclosing function from its own top is a materially different — and for that
invocation, near-certainly incorrect — execution.

Consequence for the comparator lane (§4): the unit of comparison against a
reference runtime can only be a **checkpoint PC reached by whole-function
execution**, never a single MIPS instruction. `fn64-diff` is scoped to exactly
that comparison and nothing more.

This is a statement about the current function-granularity lane, not a permanent
limit on fn64. `UNIVERSAL-RUNTIME-PLAN.md` defines the bank-qualified
arbitrary-PC block lane that removes the representability wall. Until its U1
gate passes, savestate resume remains unrepresentable and no tool may claim
otherwise. The working-tree sparse emitter now compiles a real digest-verified
N64 bank without decoding holes. `BlockProgram` atomically registers the owned
`CodeBank` with the generated callable and rechecks a sparse entry before
invocation; emitted code supplies the bank-bound registration helper. The live
executor now has an explicit `boot_thread0_block_program` lane that owns the
registered program for thread 0 and spawned OSThreads. Generated instruction
checkpoints suspend to the executor, which charges their instruction count to
virtual time and services device deadlines before another block can run. The
reset context may arrive with Status.FR set, but a libultra-created OSThread
enters the FR=0 paired-register view; the typed creation seam clears FR instead
of inheriting that reset-only view while retaining its other modeled Status
fields. This matches the fixed saved-SR shape in N64Recomp-generated
`osCreateThread` and keeps paired-double construction architectural. The
thread-0 boundary requires a canonical `fn64.boot-context.v1` value instead of
inventing a zeroed IPL3 handoff. That value binds the normalized ROM, complete
IPL3 digest, header-derived TV standard, and entry PC; restores all GPRs,
HI/LO, and modeled CP0 state; and seeds the executor's Count, Compare, and
captured IP7 latch before the coroutine exists. ROM, TV, and entry mismatches
fail loudly. `tools/mupen-trace/mupen_trace.c` creates the out-of-tree value
through the public black-box debugger boundary at the pause immediately before
the ROM-header entry executes. Its wire retains every raw CP0 slot, including
slots the runtime cannot yet execute. The producer is implemented and
syntax-checked. A timeout recovery race initially queued a second debugger step
between retirement and callback publication, losing a pre-window pause and
moving captured Count by 66 ticks. The producer now owns exactly one
outstanding step and traps on callback stall. Twenty consecutive NWXE
black-box captures reached the same 5,079,153-instruction pre-window horizon
and were byte-identical. The private capture report retains both content
digests out of tree; this document does not freeze ungated digest literals. A
real fn64 first-entry comparison is
now wired at the generated-runner boundary: the captured NWXE context matched
all GPRs, HI/LO, and every modeled CP0 field before instruction one. The sparse
resident pack's checked word access now routes aligned translated MMIO through
the same live device hook as the whole-function lane before testing RDRAM
backing, closing the SI-status fault at `0x80038268` / `0xffffffffa4800018`.
The NWXE pack also proves `__osSiDeviceBusy` by its exact six-word semantic
body, records the unique matched address in the generated artifact, and gives
that address static host-call precedence through the typed adapter. An unknown
or ambiguous signature fails the build instead of becoming an address-only
override. A first differential target snapshot disproved the apparent TLB
frontier: three independent black-box runs reached `0x80036f10` after exactly
261,748 retired window instructions with `$t8 = 0xffffffff80048860`, while
fn64 had loaded `0x60880480`, its exact byte reversal. The block example had
copied flat big-endian ROM bytes directly into native-word RDRAM storage; it now
uses the same logical IPL3 DMA materialization as every other boot harness.
After that correction, ten consecutive fn64 runs first stopped identically at
the honest sparse-pack miss for newly created thread entry `0x800004d0`.
Scenario AOT admission now consumes at least three byte-identical,
normalized-ROM-bound black-box traces and unions their exact bank-generation
PCs with the statically proven sparse bank. This does not promote those PCs to
function-owner roots and does not claim scenario exhaustiveness. The public
debugger exposes a branch and its delay slot as one step, so the producer no
longer emits its former executed-PC exhaustiveness claim; pack admission reads
and adds each observed control word's architecturally inseparable delay slot
from the same normalized ROM mapping. Three regenerated traces were
byte-identical. For the bounded NWXE trace, 1,929 distinct pause PCs plus 289
required delay-slot words produce a 90-span, 2,517-word bank, including
`0x800004d0`. The first ten consecutive fn64 runs passed that entry and stopped
at a separate runtime-behavior fault: a guest load at `0x8002a8d8` addressed
the cartridge window `0xffffffffb0000000`. The typed raw-word seam now maps
canonical KSEG0/KSEG1 PI-domain-1 address-2 reads to the same installed,
read-only ROM source used by PI DMA; noncanonical aliases remain rejected.
Ten consecutive corrected runs pass that access and stop identically at the
next runtime frontier, where VI scanout construction sees a partially
programmed register image (`H_START` decodes to `0..0`) at retrace. An
env-gated fn64 register trace proves these are raw guest writes rather than a
queued host `OSViMode`: the guest fills V timing and scales, leaves H_START
zero, then enables VI status. An independent public-debugger MMIO observation
records the same values and no H_START transition. Its status transition can
fall on either side of one adjacent debugger pause, so that diagnostic is not
an instruction-exact timing oracle. A zero H or V interval now remains an
inactive retained VI image; nonzero malformed intervals still trap. Ten
consecutive corrected runs pass the former VI assertion and stop identically
at the separate `present_render_backend: no render backend registered`
frontier, with no earlier `AotMiss`. The live dispatcher includes current
Status, Cause, EPC, and BadVAddr in a loud non-architectural gap; those fields
are diagnostic context, not a claim that the gap committed an exception.
The block example now registers the reference renderer and recompiles the
complete one-MiB IPL3-resident image: all 262,144 aligned words are present,
including words which decode to architectural RI behavior. A monolithic unit
produced roughly 267 MiB of Rust and exceeded the two-minute compile gate. The
pack therefore owns sixteen content-addressed 64 KiB bank crates. Each bank is
still one immutable generation, but its callable is statically partitioned
into sixteen 4 KiB subrunners; cross-subrunner control leaves through the
ordinary typed resolver and never decodes at runtime. This changed the full
`cargo check -j4` measurement to 62.67 seconds and the native debug build to
107.56 seconds; an unchanged rebuild took 0.06 seconds. The debug binary was
295 MiB. Three 400,000-step black-box traces, boot contexts, and completed
general-exception-vector captures were byte-identical. The CPU-produced
`0x80000180` preamble is a separate digest-checked generation rather than
being folded into the resident bank. Its four admitted words are compared
directly on each matching entry; only a mismatch hashes the live bytes to
produce the same expected/actual `AotMiss` digest evidence. This keeps image
identity fail-closed without paying SHA-256 setup on every exception.
The production pack no longer assumes that this is the only capturable vector
image. Each reproducible capture group is validated into a deterministic
external-AOT catalog: it must cover at least one of the six modeled exception
entries, its first observed fetch must be one of those entries, its range must
not overlap another capture or any immutable ROM-backed shard, and its
content-derived 64-bit bank ID must be collision-free. Generated lookup,
digest gates, runners, and `CodeBank` registration iterate that catalog. The
current evidence still owns only `0x80000180`; multi-image support is capacity
for future allowed captures, not a claim that the other five vectors closed.

Dense execution passed the former sparse-PC and renderer frontiers without an
`AotMiss`. It exposed two runtime-only harness defects: missing typed SRAM
registration, then eager RSP executable-write publication reborrowing the
live `BlockProgram` from inside its own AOT MMIO instruction. NWXE now receives
an in-memory `SramBanked` device. RSP writes only mark the generated runner's
typed executable-write boundary; the outer live owner publishes after the
runner returns and releases its program borrow, before another guest
instruction executes. The apparent next destination at `0x800e1b90` was not
indirect: typed `jr`/`jalr` provenance identified the direct `jal` at
`0x80000884`. The active resident page made that target a preceding return's
delay-slot NOP under the first page-wide identity scheme. That scheme was the
bug: the capture's first `0x790` bytes are mutable data while the fetched
suffix beginning at `0x800e1b90` is byte-identical to resident ROM
`0xe2790..0xe3000`. Its `first_executed_pc` was caller-supplied, not observed.
The trace-derived page generation has been removed, and ordinary build
coverage now comes only from the complete resident image plus four
mechanically recovered overlay recipes. Dense generated runners verify the
exact instruction word immediately before execution. A non-likely control
transfer also verifies its delay word before any branch effect; a likely
branch verifies that word only on the taken path. Thus a neighboring data
write cannot retire valid code, while a changed fetched instruction returns
typed `ImageChanged` with zero retired instructions. The closed catalog then
matches complete ROM-recovered overlay candidates and retries the same PC
under the selected immutable bank. Unknown content remains a loud `AotMiss`,
with no runtime translation or interpreter fallback. Focused compile-and-run
gates cover mutable neighbors, changed instructions, annulled and taken likely
delays, shard-end lookahead, and cross-shard fallthrough. Corrected two-million-
and ten-million-step idle-boot runs execute resident AOT without an earlier
`AotMiss` or false image change, but neither requests a recovered gameplay
overlay. A deterministic controller scenario with an independent black-box
trace, followed by ten-run validation, remains outstanding. The block
harness's opt-in
`FN64_BLOCK_PC_TRACE` diagnostic requires that minimum budget and writes the
bank-qualified retired-PC stream reconstructed from typed runner-entry counts;
it does not instrument or alter generated instruction semantics. The companion
`FN64_BLOCK_HOST_TRACE` stream records typed host-call enter/exit anchors with
thread, target, resume, GPR, HI/LO, and modeled CP0 state so differential
normalization never guesses a substituted guest-function extent.
Both histories remain complete by default. The long-running exploratory
harness suppresses a history only when its corresponding trace environment
variable is absent, preventing tens of millions of diagnostic entries from
turning a bounded probe into unbounded memory growth. A digest-bound
controller schedule advances by successful per-port controller-read ordinal,
not instruction count. The current resident scenario reaches zero such reads,
so controller replay is implemented but cannot yet steer this timeline.

Later typed host discovery and timing work supersedes that resident-only
frontier. Public-manual structural recognizers now bind `osSetTimer` and the
four SP-task operations, controller initialization completes one operation,
and repeated VI H/V timing writes no longer reset the running beam epoch.
Ten consecutive current non-exploratory runs enter mechanically recovered
overlay generation `0x5DEA0D1723E94993` at step 19,523 and
`sim_time=13990253`, with a guarded peak of 134 MiB. The retained schedule is
still unused because no standard-controller read occurs. A 100,000-step
continuation completes overlay0 and services 130 audio tasks but submits no
graphics task and enters no later overlay generation; that post-overlay
application/graphics progression was the next runtime frontier at that point.
Created OSThreads now enter with the generated `osCreateThread` FR=0 contract,
which lets the current 65,000-step scenario submit fourteen graphics tasks.
Recognized graphics LLE is included in phase timing separately from its HLE
preflight. Native profiling showed its raw-DPC framebuffer commit dominated the
old run: the reference renderer therefore stores the fixed 8 MiB RDRAM hidden
coverage domain as a lazy 16 MiB packed dense sidecar rather than a hash map.
The standalone diagnostic harness applies `opt-level=2` only to that handwritten
renderer; generated shards retain their bounded low-memory profiles. The
measured executor window fell from 49.584 s to 16.592 s without changing the
step, simulation-time, device/task, or render-error observations.

Reserved words in a dense bank now emit the architectural RI
exception (ExcCode 10), including
precise EPC/BD in a delay slot, rather than failing Rust code generation. The
OoT Rust host can now explicitly select an out-of-tree generated pack source,
hash-bind and preflight its entry/runner identities, and install it without a
whole-function guest fallback. Missing pack input fails the build. The current
OoT `recompile_rom` generator still emits only the whole-function crate, so no
real OoT pack artifact exists to exercise that host path yet. Arbitrary-PC
codegen emits direct boundaries for a supplied static host-JAL inventory and a
distinct `ResolveCall` for dynamic JAL/JALR targets. The live resolver types
those as either an installed host function or a bank-qualified guest target;
ordinary dense and sparse arms share one typed post-step boundary decision.
The emitted arm still owns the architectural operation, CP0 Random advance,
retirement count, and transfer. The shared helper only preserves the ordered
choice of a committed executable write before a local budget checkpoint; at
an artifact edge it drains a write but leaves transfer classification to the
generated arm. This removes repeated exit construction from generated HIR
without moving instruction semantics into the runtime. The helper is
deliberately non-inlined so LLVM cannot recreate that body at every generated
site. Its per-ordinary-instruction host call is an explicit runtime tradeoff:
source shrink alone is not a performance result, and retention requires both a
worst-shard compile/RSS comparison and an unchanged execution oracle with an
acceptable runtime measurement.
generated `jr`/`jalr` recognizes only an explicit thread-return target. A
spawned OSThread uses the synthetic sentinel installed with its entry context;
thread 0 instead uses the exact `$ra` in the validated header-handoff
`BootContext`, because a normal ROM-bootstrap return targets IPL3/SP memory
outside the game AOT pack.
`BootContext v1` deliberately does not contain the 32-entry TLB, and restoring
it does not invent that state. Boot-TLB analysis therefore retains
`InitialTlbStateUnproven`: it correlates path-invariant indexed writes and
transfer-time EntryHi/ASID, uses the runtime's typed non-panicking diagnostic
translator, and intersects only independently proven physical backing. This
view remains diagnostic; it does not mint a `RomMapping` or alter cold ROM-only
authority. A disposable ares `b80f67d3` candidate capture reached the exact
header entry for the cold-panel GoldenEye and Perfect Dark inputs, but each
successful-TLBWI/TLBWR-since-power mask was `0x00000000`. All 32
captured values therefore came from ares's zero-reset policy and cannot supply
hardware authority. Production translation still traps loudly on invalid
PageMask encodings and undefined multiple matches.
`fn64-cpu-runtime` keeps the interpreter and `dynamic_mips` fallback behind the
`dev-interpreter` feature. The final WM host selects `production-aot`, which
implies `aot-runtime` and is compile-time incompatible with `dev-interpreter`;
its standalone workspace explicitly uses Cargo resolver 2 so build-tool
features remain separate from the linked target graph. Dense-pack manifests
disable default features and select `aot-runtime`. An admitted mapped
generation without a callable returns a named `MissingAotEntry`/`AotMiss`
fault. Before a precompiled generation catalog becomes live, every shard must
name an installed generated bank with exactly one matching contiguous range.
This prevents activation of a catalog-only bank from retrying an `UnknownBank`
without progress. The interpreter API is absent from the production build,
while development/oracle tests keep the default feature for differential
coverage.

The `static-micro-op.v1` format separates immutable instruction data from a
shared executor using one
canonical eight-byte record: expected raw word, an exhaustive stable opcode,
decoder-derived flags, and a zero reserved byte. The raw word remains the
live-image verification and operand authority; the opcode cannot silently
drift because its match covers every decoded `Instruction` variant without a
wildcard, including an explicit reserved-instruction tag. The codegen-side
artifact retains bank-qualified ordered span geometry and validates alignment,
overlap, counts, digest, records, truncation, and trailing bytes. Separate
source receipts bind the packer and the record mapping plus shared decoder;
the runtime owns the sole artifact parser and codegen validation delegates to
it.
Exporting that API still changes the codegen
crate's `lib.rs`, which the existing generated-emitter receipt hashes, so
prepared typed-Rust artifacts correctly invalidate once.

The experimental static-micro-op executor still cannot grant `production-aot`
authority. Every admitted non-control word now enters the lane-neutral
`execute_straight_word` kernel using the exact raw word; the kernel decodes
that word internally, so a caller cannot pair one verified word with another
decoded operation. BEQ/BEQL control pairs and their narrow delay slice remain
local to this executor; other control-shaped words fail loudly.

The dense-runner differential compares the exact `BlockRun`, complete
`RecompContextEvidenceSnapshotV1`, full RDRAM, and ordered MMIO events. Its
fixtures cover straight integer, memory, executable-write, direct-MMIO,
mapped/TLB data-alias, COP0 Random/Count, ERET, COP1, prior-retirement fault,
live-word mismatch, branch-likely annulment, and V2 lookahead behavior. This
proves integration equivalence between the two runners, not an independent ISA
oracle: both intentionally share semantic/runtime helpers. Instruction
live-word verification still uses the direct-RDRAM dense helper, so mapped/TLB
instruction-fetch identity, the remaining control/delay families, and
host-call transfer boundaries remain promotion gates.

V1/V2 executor receipts remain frozen. V3 additionally binds `fpu.rs` and the
complete manifest/lib/decoder/execution/runtime/pack/executor/semantic source
set. Generated-runner runtime receipt V2 likewise adds `fpu.rs`; static
execution receipt V2 distinguishes `dynamic-mapped-runtime` from the broader
development interpreter.

A control-shaped record is not rejected merely because the preceding record
also has a delay slot. The second record remains a valid arbitrary direct entry;
only executing the first pair reaches the architecturally unpredictable nested
control, where the experimental executor returns its loud typed unsupported
boundary. Artifact admission still requires every control to have a delay
record. V2 closes the distinct package-boundary shape while preserving every
V1 API: a span header explicitly tags zero or one appended delay-only record.
The executor can reach it only from the final owned control, live-verifies it
at `end`, and excludes it from direct entry, owned instruction counts, and the
bank identity derived from owned words. This represents
`wm2000-block-overlay-3-shard-03`'s final branch at `0x80121b8c` and affine
lookahead at `0x80121b90` without turning the latter into owned code. The
content-silent V2 WM profile admits all 35 packages: 516,688 owned instructions
in 4,135,951 bytes; the 67-byte V1 delta is 35 one-byte span tags plus four
eight-byte lookahead records. Ten consecutive real-ROM profiles returned the
same package count, owned-instruction count, byte count, and inventory digest.

The authority-capable catalog lane is now separate from that legacy live
builder. `CatalogResolverInstallV1` consumes the canonical block program and
exact host-function inventory; its boot API accepts no resolver callback or
ambient host lookup. `CatalogGenerationInstallV1` additionally consumes a
closed generation inventory whose complete invalidation VA ranges are tiled by
explicit physical-RDRAM spans. The spans support noncontiguous page mappings,
are bounded by the 8 MiB device, and drive both physical-byte digest selection
and the executable-write denominator. Active generation ownership is resolved
before static code. An inactive owned target becomes a typed activation
boundary; generation shard banks are excluded from ordinary static fallback,
and an unclaimed static span may not intersect generation ownership. CPU and
host/DMA writes retire every active segment of each affected image before the
next resolution. The live evidence includes the complete immutable inventory,
backing map, current active segments, pending physical writes, and the
canonical executable-mutation journal. The recompiler boundary assigns every
write event one of the fixed eight writer channels; producer-less notification
is not an API. Device commits retain PI/SI/SP identity through `DmaMemory`.

The physically backed catalog also exposes a thread-local diagnostic observer
after a complete live physical-image digest has matched one admitted generation
and that generation has been published active. Its event records the requested
PC, selected generation and entry, matched image digest, whether publication
was new, and retired generations. Failed and compatibility/unbacked activation
paths emit nothing. The callback runs after publication and must not panic or
recursively activate through ambient host state. This is trace-local runtime
observability, not a durable receipt: it neither binds a catalog definition nor
proves static reachability, catalog completeness, unobserved paths, or writer
closure. Any retained diagnostic must join it to the installed catalog's
canonical-definition identity. Runner entry remains a separate later event.

The compatibility resolver constructor still accepts an arbitrary
`HostFunctionCatalogV1`, but its evidence is explicitly non-authoritative.
The authority-capable constructor instead consumes an opaque ABI-issued host
catalog: callers provide only aligned guest target PCs plus stable named shim
IDs, while fn64-abi privately selects the safe-Rust callable and a conservative
writer-effect declaration. Its receipt hash is bound into both resolver and
writer-program model identity. This semantic addition advances those evidence
domains to `fn64.catalog-resolver-install.v2` and
`fn64.canonical-writer-program-model.v2`; retained V1 names describe the old
target-only model. A caller-supplied function pointer or effect
label cannot manufacture that authority. This is a prerequisite for, not a
claim of, SI or any other non-bootstrap writer-channel completion.

Generated-runner source identity has a deliberately weaker boundary. The WM
Cargo build now hashes the exact checked-in root manifest/lock/build/adapter
sources, the shared shard build/lib sources and all 35 shard manifests. Each
linked bank binds its complete code words/geometry, generated composite-source
digest, subrunner count, and adapter role. `fn64-cpu-runtime-codegen` issues the
exact emitter-source receipt, while `fn64-cpu-runtime` separately issues the
linked execution/runtime receipt plus its feature receipt. That runtime
receipt includes `generated_support.rs`, the documented typed cold boundary
used by emitted runners for shared synchronous-fault construction and
retirement. The lower
`GeneratedRunnerSourceAttestationV2` binds that pointer-free projection while
explicitly treating the emitter identity as externally measured.
It is not callable semantics authority: safe Rust cannot prove that an opaque
function pointer from a separately compiled crate came from those source
bytes, and the public fields can be paired with another callable. Therefore
the generic and source-attested catalog constructors expose no receipt that SI
completion may consume. `fn64-boot-harness` now owns the separate frozen Cargo
build and exact compiler-artifact selection. Its selected WM child has one
fixed identity argument which constructs the canonical block program without
installing devices or starting guest execution, emits one deny-unknown
protocol envelope sorted by bank, and exits. The envelope binds the exact
manifest/lock, measured source domains, source-attestation fields, production
feature receipt, and every runner's source/code/geometry/composition/role.
Only the parent verifier combines that child report with its selected binary
and source measurements to mint the move-only build capability; invoking the
child mode directly remains non-authoritative. The selected WM child now also
has a fixed Bootstrap/Import audit argument which branches immediately after
canonical boot has produced the ABI's move-only bootstrap completion receipt,
before controller, history, watchdog, or guest scheduling setup. It consumes
that receipt once and emits one nonce-bound, deny-unknown line containing a
pointer-free projection of sequence-zero bootstrap publication. The parent
uses the same bounded, environment-cleared launcher as the device audits,
revalidates the retained binary and private inputs around every launch, and
requires exactly ten distinct nonces with identical nonce-excluded semantics.
The resulting move-only outer series binds the selected build to the ABI
bootstrap receipt, ROM, resolver, generation catalog, journal root, and final
watched bytes. A direct ABI receipt has no denominator completion API; only a
verifier-owned writer-audit bundle can project this selected-build series.
Neither the child report nor copied series evidence is
authority, and no private exact-ten Bootstrap series has yet been run.

Captured CPU-produced executable images are immutable precompiled generations,
not initially resident static banks. The WM catalog registers every such image
and its direct-RDRAM physical backing before bootstrap validation. Zero bytes in
that reserved range therefore mean the future image has not yet been produced;
an ordinary unreserved static bank must still match its admitted words at
bootstrap. This keeps bootstrap validation, executable-write invalidation, and
later digest-selected activation on one generation mechanism.

To avoid rebuilding the very large generated runner once per channel, the
boot harness also exposes a move-only writer-audit session. It retains one
`VerifiedGeneratedRunnerBuildV1`, can independently run Bootstrap, SI, and SP
exact-ten series at most once each, preserves already-established evidence if
a later channel fails, and seals any nonempty subset into a non-cloneable
bundle. The bundle bitmap, common build/binary/private-input identities, each
nested series authority, cross-channel build/program identity, and canonical
writer-program model are hash-bound and revalidated. It exposes evidence but
never the selected path or staged private inputs. This is a runtime-audit and
profiling optimization; the bundle itself grants no denominator credit until
an explicit denominator API consumes the relevant move-only authority.
Existing consume-one-build SI/SP entry points remain compatibility wrappers
over the same borrowed-build implementations.

The private writer-audit CLI may publish fixed path-free progress at the
verified-build and channel-series boundaries. These wall-clock observations
are flushed for operator feedback and may be retained beside private
diagnostics, but they are neither serialized into nor hashed by any build,
series, bundle, denominator, or scorecard receipt. Losing the diagnostic
consumer cannot create, suppress, or alter authority; the exact-ten session
and its move-only bundle remain the only completion path.

Writer children may emit ordinary runtime diagnostics before their authority
envelope. Transport is source-bound by the v5 selected build, capped at 16 MiB
per stdout/stderr stream, and accepts exactly one expected protocol-prefixed
report of at most 1 MiB. Other report prefixes, zero reports, and duplicate
reports fail closed. Only the extracted line enters the unchanged strict
nonce/build-bound semantic parser; diagnostic bytes never enter authority.
Watchdog failures remain fixed at 600 seconds and retain only bounded private
tails plus exact byte counts and full-output digests.

The first complete v5 selected build finished in 854,608 ms. Its all-channel
session retained one exact-ten CPU series and a one-of-eight partial bundle.
Bootstrap run zero produced 8,214,477 diagnostic bytes before its report and
hit the former 1 MiB transport cap; Host ABI, PI, RDP renderer, RSP, SI, and SP
each hit the unchanged 600-second watchdog. No complete writer-audit or
scorecard receipt resulted, and those seven transport/liveness failures do not
establish seven semantic channel gaps.

The runtime issuer and selected-build verifier consume the same exported
`GENERATED_RUNNER_SOURCE_BINDING_DOMAIN_V2` constant. The verifier still
reconstructs every ordered field independently, but the protocol version is
not duplicated as a string literal: an earlier cold full build exposed that
the issuer had advanced to V2 while the verifier still hashed the V1 domain.
That drift now fails at compile-time name resolution or at the ordinary
field-level recomputation tests instead of after a private cold build.

SI completion additionally
requires the SI-specific channel constructor described below. The boot harness
now consumes the selected-build capability in a fixed SI child mode and
verifier-owned exact-ten series. Each launch receives a fresh nonce and only
the retained staged ROM, BootContext, and capture paths; the parent revalidates
those inputs and the binary around each watchdog-bounded launch, accepts one
content-silent report line, and requires all ten reports to be semantically
identical except for distinct nonces. The resulting move-only capability binds
the build authority, private-input and binary identities, program
model/resolver/catalog, sealed journal, watched state, and SI transition
receipt. No private series has yet been run, so no production denominator has
received SI credit. The writer denominator still accepts this move-only
capability through `complete_si`: it revalidates the series authority hash,
requires an exact canonical writer-program-model digest match and an open SI
row, and stores a private SI receipt bound to the series authority. The
selected-build writer-audit bundle is the atomic path when it represents SI
alongside Bootstrap and/or SP. Copied public reports or series evidence remain
inadmissible. The ABI supplies the
inner runtime-state prerequisite for that owner: a private validator
requires an ABI-issued host catalog, the production-AOT feature lane, retained
and balanced SI start/byte-commit/busy-clear/interrupt/notification
transitions including at least one PIF-to-RDRAM commit, no pending device or
ABI SI owner, and a sealed quiescent mutation journal whose expected bytes
still equal owned RDRAM. One successful take yields a move-only, non-serde
receipt bound to the exact writer-program model, resolver, host-catalog
receipt, terminal journal root, watched digest, and SI transition digest. It
deliberately carries no generated-build authority and no writer-denominator
API accepts it directly. The fixed verifier-owned child mode disables and
re-enables device-trace retention immediately before its bounded SI audit
window; otherwise an earlier unrelated PIF-to-RDRAM transaction could satisfy
the prerequisite's minimum exercised-path condition. The validator rejects a
non-monotonic `(cycle, sequence)` stream. It also retains the count of journal
declarations attributed to `SiDma`; that count may be zero because declarations
are clipped to executable backing and ordinary PIF controller buffers are not
code. Structural path totality must come from the typed gateway plus the
future selected-build series, never from observing an executable intersection.
That trait is sealed to runtime-owned implementations; the canonical ABI raw
allocation crosses it only through a call-scoped `ProcessDmaMemory` which
bounds the allocation, performs the canonical lane mapping, and invokes its
borrowed attribution callback after commit.
The SP-DMA channel now has the same ABI-local prerequisite boundary without
claiming channel completion. Its begin API first requires the canonical owned
bootstrap, ABI-issued host catalog, production-AOT lane, sealed/unpoisoned
journal, and no pending physical, attributed, host, child, device-SP, or
ABI-RSP work. It then clears and re-enables device-trace retention and returns
a move-only epoch token bound to the exact writer-program model. The take API
accepts only that live epoch, checks exact watched bytes, and validates the
public RSP guide's double-buffered lifecycle: `SpDmaStarted`, optional
`SpDmaQueued`, matching `SpDmaBytesCommitted`, immediate start of the exact
queued request before busy can clear, and terminal `SpDmaBusyCleared`, with at
least one `RspToRdram` commit. Raw SP DMA has no MI interrupt or OS
notification to invent; busy-clear is its terminal publication. The receipt
binds transition counts/digest, journal root, watched digest, resolver, build,
and host-catalog authority, is one-successful-take, and remains inadmissible to
the writer denominator. The selected WM runner now exposes a fixed SP child
mode which arms this typed epoch immediately before the same bounded canonical
guest/device scheduling loop used for the SI audit. Only SP DMA initiated by
the admitted guest can populate that fresh trace; the child retries transient
pending/no-transition/no-RSP-to-RDRAM states, but traps malformed transition
order or any other invariant failure. After the real writeback lifecycle and
all SP device/task/ABI owners drain, it consumes the ABI receipt once and emits
one nonce-bound deny-unknown report. The boot-harness parent consumes the
selected-build capability and reuses SI's environment-cleared, staged-input,
bounded-output, watchdog, and pre/post-validation process mechanism. Ten
distinct OS-random nonces with identical nonce-excluded semantics mint a
move-only SP-series prerequisite. No private SP series has been run, and no
production denominator has received SP credit. The writer denominator still
accepts this move-only capability through `complete_sp`: it revalidates the SP
series authority hash, requires an exact canonical writer-program-model digest
match and an open SP row, and stores a private SP receipt bound to the series
authority. The selected-build writer-audit bundle is the atomic path when it
represents SP alongside Bootstrap and/or SI. Copied reports, copied series
evidence, and the ABI-local SP prerequisite remain inadmissible. This
ABI-owned epoch improves on the SI prerequisite's
current outer-verifier freshness obligation; SI is not silently reinterpreted
and still requires its selected child to clear/re-enable retention immediately
before its bounded audit. Provenance: public *RSP Programmer's Guide*, SP DMA
register and double-buffering descriptions, plus fn64's typed device trace.
The PI-DMA channel now has the same bounded ABI-local prerequisite, without an
outer selected-build series or denominator completion claim. Its begin API
requires the validated owned bootstrap, ABI-issued host catalog,
production-AOT lane, sealed quiescent journal, no active device request or
queued ABI completion owner, and a clear PI interrupt line. It then clears and
re-enables retained device history and returns a move-only, process-unique
epoch bound to the exact writer-program model. The take API accepts only that
live epoch, rejects either pending PI owner, rechecks the journal and watched
bytes, and validates each typed lifecycle in order: matching `PiDmaStarted`,
`PiBytesCommitted`, `PiBusyCleared`, PI interrupt publication when the line was
not already asserted, and matching `NotificationReady(PiDmaComplete)`. The
validator also tracks interrupt acknowledgements, so a serialized request may
correctly complete without a second raise event while the first interrupt
remains asserted; no missing transition is invented. At least one
device-to-RDRAM commit is mandatory. The one-successful-take, non-serializable
receipt binds the transition counts/digest, terminal journal root, watched
digest, resolver, production feature receipt, ABI host catalog, and writer
program model. A data-only DMA can exercise the typed producer while its
executable-clipped journal declaration count remains zero. Copied evidence is
not authority, and no selected-child PI exact-ten series or production PI
credit exists yet. Provenance: public libultra PI manager/DMA documentation,
public PI register behavior, and fn64's typed device trace.
The RSP-execution/HLE-writeback channel has a path-total ABI-local prerequisite
over the installed production policies. Interpreter publications append exact
physical half-open ranges and their `RspInterpreterOwner` (task address plus
admission generation, or a generation-bearing raw kick). The translated-audio
HLE callback runs inside the canonical nested writer and classifies its BREAK
result before publishing trace success. A successful callback binds its exact
owner and every resulting executable-journal sequence; a non-BREAK callback
records its sequences as rejected and permanently invalidates that epoch, so a
caught unwind cannot let a later audit absorb speculative executable changes.
Empty successful callbacks remain visible as typed lifecycle boundaries but do
not satisfy the receipt's writeback requirement. Duplicate, missing, wrong-
channel, or rejected sequences fail closed. A process-unique move-only epoch
supersedes all earlier observations. Successful take requires a production-AOT
canonical owner, validated bootstrap and ABI host catalog, sealed journal and
matching watched bytes, no device task, loaded/yielded lineage, interpreter
owner, HLE continuation, host transaction, child writer, or pending executable
write. The one-take receipt binds those authorities, the RSP journal-declaration
count, exact interpreter ranges, and exact translated-HLE publication sequence.
Graphics optimized HLE writes belong to the separately audited RDP-renderer
channel. The verified-audio transactional adapter remains test-only and is not
an installed production policy or a trace source; no receipt claims otherwise.
The selected runner now chooses graphics `LleAccuracy` for its RSP audit,
arms a fresh epoch immediately before guest/device scheduling, and retries
only live device/ABI ownership or the absence of a typed writeback. Its strict
nonce-bound report projects the ABI receipt and recomputes the production
build, program model, catalog, journal, watched image, and trace identities.
Ten distinct nonces with identical nonce-excluded semantics mint a move-only
series and an RSP bit in the one-build audit bundle. A data-only interpreter
writeback may exercise the typed producer while the executable-clipped journal
count remains zero; copied report bytes are not authority. This remains
structural: no private ten-run RSP series has executed, and the selected WM
scenario exercises the interpreter policy rather than independently exercising
the translated-audio callback.
Provenance: public *RSP Programmer's Guide* task/DRAM-DMA protocol and fn64's
typed interpreter-owner and mutation-journal machinery.
The CPU-instruction-store channel now has a corresponding ABI-local fresh
window, but not an outer selected-build series. Its begin API requires the
validated owned bootstrap, ABI-issued host catalog, production-AOT lane, and a
sealed, unpoisoned, quiescent mutation owner. The move-only epoch is
process-unique and bound to the exact writer-program model; arming a new epoch
supersedes the old one. While armed, the existing post-commit typed RDRAM store
observer retains each CPU physical range in order. Successful take requires at
least one valid in-RDRAM store, a second quiescent boundary, unchanged watched
bytes, and a self-consistent canonical journal, then consumes the sole live
epoch and returns a non-serializable receipt binding the trace digest, journal,
resolver, feature receipt, ABI host catalog, and writer-program model. A CPU
store outside executable backing legitimately exercises the typed path while
adding no clipped journal declaration. The exported notification gateway also
serves generated-C adapters, so this inner receipt deliberately does not prove
which separately compiled body caused the store. No selected WM child,
exact-ten build-owned series, writer-audit bundle projection, private run, or
denominator completion exists for this channel yet. Provenance: MIPS III store
semantics, fn64's typed `Rdram` store gateways, and the canonical mutation
journal described above.
The Host-ABI channel now carries its ABI-local fresh transaction prerequisite
through a selected-build series, but has no production completion claim. Its
begin API requires the
validated owned bootstrap, production-AOT lane, ABI-issued stable-shim
catalog, and a sealed, unpoisoned, quiescent mutation owner. A process-unique,
move-only epoch arms tracing inside that exact owner. Each subsequent
catalog-owned host call retains its target, resume key, thread, per-thread
LIFO begin/finish lifecycle, ordering boundaries, and the exact HostAbi
journal sequences committed at those boundaries. Successful take requires a
balanced lifecycle, a second quiescent watched-byte check, and at least one
actual executable-journal commit attributed to `HostAbi`; merely invoking a
shim whose conservative effect set says it *may* write cannot satisfy the
writer prerequisite. The non-serializable receipt binds the fresh lifecycle
digest, initial and terminal journal positions, journal root, watched digest,
resolver, feature receipt, ABI host catalog, and writer program model. A
replacement epoch supersedes the prior token, and one successful take consumes
the sole authority. Compatibility catalogs remain executable but are
deliberately inadmissible because caller-supplied raw function pointers do not
prove stable-shim identity or total writer effects. The selected runner arms
this epoch immediately before guest scheduling and must exercise a real
HostAbi executable write through the enumerated ABI catalog. Its strict
nonce-bound report is recomputed by the parent, and ten distinct, semantically
identical launches become one move-only series that can join the one-build
audit bundle. This remains structural evidence: no private ten-run series has
been executed, and model-total coverage of all mutable host surfaces remains
required for production denominator credit. Provenance: fn64's
canonical host transaction and executable mutation journal described below;
no reference-runtime implementation was consulted.
Canonical renderer task calls and validated raw-DPC shadow publications are
similarly bracketed by an executable-only preimage. Changed logical physical
bytes are coalesced and reported as `RdpRenderer` before the guest can resume;
ordinary framebuffer changes outside the executable union are not retained.
The channel now also has an ABI-local fresh publication prerequisite. A
process-unique move-only epoch is armed only while the canonical production-
AOT owner has no live RSP task, fabric DPC transaction, pending DP completion,
HLE continuation, loaded task, task lineage, interpreter owner, host frame, or
child writer. Successful HLE `Complete`/`Yielded`/`Continue` chunks and
fabric-committed raw-DPC transactions are the only lifecycle marks. A
`NeedsLle` preflight still traverses the mutation bracket, but it cannot count
as a renderer publication and any executable write invalidates the epoch. Take
requires a second
quiescent boundary, at least one marked publication, exact agreement between
the publication windows and every `RdpRenderer` journal sequence since arm,
and unchanged watched bytes. Its non-serializable receipt binds that trace,
the journal root and positions, final watched digest, resolver, ABI-issued host
catalog, feature receipt, and canonical writer model. The selected runner now
arms that epoch immediately before guest/device scheduling. Its child retries
only live task/device/renderer quiescence and `NoRendererPublications`; an
invalid trace remains a loud failure, so a `NeedsLle` preflight cannot be
relabelled as a publication. The strict nonce-bound report additionally
requires at least one actual `RdpRenderer` executable-journal entry and
declaration; a committed framebuffer-only publication is insufficient. The
parent recomputes the ABI receipt, selected build and program identities, then
requires ten distinct nonces with byte-identical nonce-excluded semantics
before minting a move-only series. That series can join the same one-build
audit bundle as the other represented channels. This remains structural
evidence: no private ten-run RDP series has been executed and model-total
renderer coverage remains open.
Catalog-owned host calls form per-guest-thread LIFO parent transactions. The
owner commits the current `HostAbi` prefix before coroutine suspension and
before every synchronous RSP/RDP child enters, commits each child batch
immediately, retains the parent frame across a yield, and commits the residual
`HostAbi` suffix when the host call returns. The hash-chained batches therefore
preserve `HostAbi -> child/device -> HostAbi` execution order even when every
writer changes the same executable byte. Open frames and pending writes are
transient quiescence diagnostics, not part of the committed journal root.
This closes that attribution interleaving only for the enumerated canonical
gateways. Compatibility raw-pointer APIs, other noncanonical callers,
model-total coverage, and the selected-build completion authority remain
structural channel-closure blockers. Provenance is fn64's canonical mutation
journal, fabric-owned DPC transaction, and public `RenderBackend` commit
contract; no reference-runtime implementation was consulted.
Bootstrap/Import's ABI receipt is only an inner prerequisite. A move-only
verifier-owned selected-build bundle may retain Bootstrap, CPU, HostAbi, PI,
RDP-renderer, RSP-execution-writeback, SI, and SP series from one exact build.
All eight fixed writer channels now have a bundle projection. The current
denominator accepts the other seven; RSP denominator admission remains a
separate integration step. Those are structural authorities only: without the
private exact-ten runs, every corresponding production row remains open.

Stage B's ROM-independent build boundary is specified in
[`WM-PREPARED-SHARDS.md`](WM-PREPARED-SHARDS.md). Its std-only per-shard
materializer and strict v2 synthetic format exist, but remain inactive: all 35
manifests still use the current shared ROM-driven build script. A future
activation uses the new one-shot producer, which shares the exact source
generator with that legacy build and atomically renames a complete synced tree
from outside the repository. Its explicit source identities remain claims.
The root manifest cross-binds all package sidecars and artifacts, while each
materializer watches only its package's stable sidecar and two sources; a
root-claim-only retry with byte-identical package files atomically replaces
only the authority manifest at the same root. This preserves every watched
path, but no zero-shard invalidation claim is made until Cargo compiler-artifact
evidence exists. Stable-root artifact reconciliation is likewise serialized:
an update marker makes materializers fail closed, changed sources commit before
their package sidecar, root authority commits last, and a same-target rerun
recovers an interrupted prefix. Concurrent producer/Cargo execution carries no
authority.
Generated-build v3 now implements that inactive cold authority: it owns the
frozen producer graph and staged binary, independently validates the exact
private projection, remeasures it across Cargo and the identity child, and
retains it in the move-only build capability. Its explicit
`legacy_with_prepared_candidate` mode keeps the selected binary's legacy Cargo
source attestation authoritative and does not claim that prepared sources were
compiled. The future `prepared_consumed` mode is derived only from an exact
all-35 manifest inventory and switches the source domain to
`prepared_build.rs` plus `materializer.rs`. Manifest claims and warm
materialization are not authority, and there is no fallback from the prepared
path to discovery/codegen because that would recreate the invalidation edge.
Real-ROM byte parity and guarded cold/warm activation benchmarks remain gates.

At first dispatch the owner snapshots the union of every generation's physical
backing, not only active code. After an attributed write it verifies that every
changed byte lies within a declared range, invalidates intersecting active
generations, and hash-chains the exact channel/ranges, before/after digest, and
retired identities. It byte-reconciles the expected backing again before every
dispatch, so a remaining raw-pointer escape traps before stale static code can
execute. This runtime reconciliation does not itself close the writer
denominator: raw mutation visibility must still be sealed structurally before
the seven remaining completion receipts can be minted. The older
callback/builder APIs remain executable for compatibility but cannot populate
this canonical evidence.

`ExecutableRegion` now owns one active immutable generation and atomically
retires the previous `CodeBank` plus runner on same-range replacement. The ABI
registers equal-length physical/virtual executable spans and observes typed CPU
stores, generated-C direct RDRAM stores, and device DMA writes through one
post-commit range seam. At the next host boundary it snapshots architectural
byte order, builds and publishes the replacement pair, retires the old pair,
and re-resolves interrupt, checkpoint, host-resume, and spawned-thread entries
through the active mapping before executing. The ordinary post-commit write
observer remains notification-only. A live program owner separately installs
the typed `GuestWriteBoundaryObserver`, which reports `ExecutableChanged` only
for a proven active-region overlap. Generated AOT and interpreted runners
consume that mark after one straight instruction or after the full indivisible
branch/delay pair. `BlockExit::ExecutableWrite` preserves source-bank lineage
and the resume PC; its resolve-call and fault variants carry unresolved targets
out of generation A so the live owner can publish B before resolving a call,
resume, or exception vector. Non-overlapping stores continue normally. The
project-owned contiguous/sparse runner, emitted/interpreted delay-slot,
deferred-continuation, and live stale-sentinel gates define this contract and
prevent A's post-store sentinel from running. This is not a hardware-cache
claim. Cache tags, silicon self-modifying-code rules, page-granular ownership,
automatic executable detection, and a real translator/pack plus boot
registration remain open, so this does not make real-ROM transplant/resume
available by itself.

`BlockProgram::dispatch` therefore has two explicit exception ownership modes.
The ordinary API vectors architectural faults inside the guest program. The
live host-scheduled lane asks dispatch to expose faults to its owner instead.
For an `OSThread` registered through the typed host scheduler, that owner
commits precise CP0 exception state, optionally posts libultra's registered
BREAK/FAULT event, and parks the current coroutine. It must not execute the raw
guest exception dispatcher, because host-bound thread creation and queues make
the typed executor the sole scheduler authority. The build discovers the
guest current-thread global independently from the agreeing null-thread paths
of `osGetThreadPri` and `osSetThreadPri`; the per-resume seam mirrors the
selected `OSThread*` there for guest-visible state without admitting a second
scheduler. In the canonical catalog lane this scheduler-owned publication is
reconciled, attributed to the HostAbi writer channel, invalidated, and committed
before the coroutine resumes. It is not represented as a catalog host-call
frame because no guest target/resume pair exists; the current host-call-only
completion receipt therefore remains open until a later schema admits this
distinct typed scheduler boundary.

Operational A/B capture may also install one process-wide exact
charged-instruction limit. The canonical owner clamps each subsequent dispatch
slice to the remaining work. A final straight instruction may execute with a
one-instruction budget; a branch and its delay slot still require two together,
and return a typed indivisible-unit error before either instruction mutates
state when only one remains. The operational publication v2 comparison digest
validates but excludes the most recent slice charge, because AOT and dynamic
execution may partition identical cumulative work differently. It also omits
the per-context dispatch-entry mirrors of Count, Compare, and the host-driven
RCP/timer Cause lines: the required executor digest owns Count, odd phase,
Compare, and timer state, while the required device/ABI digests own RCP state.
Pending Count/Compare writes are rejected rather than omitted. The v2 CPU
digest still binds context-owned CPU state, including Random and every other
Cause bit; its equality is meaningful only beside equal executor, device, and
ABI component digests. The continuation digest still binds cumulative charge,
pending exit, and prepared continuation. These controls change checkpoint
scheduling and comparison only; they are not guest state, program identity, or
static execution authority.

The WM operational A/B lane withholds the installed canonical-entry
`ExecutionKey` once, not a generated shard or an arbitrary catalog member.
Both binaries install the same complete static catalog and retain the same
canonical program and resolver-install identities. The dynamic-enabled owner
validates that the selected key equals the installed entry, then applies one
operational-only redirect at the unified dispatch seam. The guard clears only
after that dynamic attempt charges positive work; ordinary static budgets and
executable-mutation reconciliation then resume without changing the outer
interrupt boundary. The v2 telemetry proof belongs to that individual attempt:
it names the entry, requires positive `charged_instructions`, and requires zero
`unsupported_exits`. Aggregate identity totals cannot prove the selected
attempt.
Immediately before the first unified dispatch, the catalog boot seam also
requires the dispatch PC to equal the validated `BootContext` entry and the
restored CPU projection to match that context. The ordinary AOT lane repeats
that check at its first generated-bank call. Exact-entry withholding does not:
its first static call is the post-instruction resume, so the per-attempt dynamic
telemetry is the applicable first-entry evidence.
`FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY=1` enables this mode;
`fn64.wm2000.dynamic-withheld-telemetry.v2` records the selected bank/PC,
selection basis, per-attempt result, dynamic identities, and the unchanged
program and resolver-install identities. The identity line and comparator bind
`resolver_install_sha256` alongside the program digest. This replaces
whole-shard withholding, which
could miss an unvisited shard and therefore fail to exercise the mechanism.
One 100,000-instruction real-ROM v3 diagnostic reached 100,001 charged
instructions in both lanes. The exact withheld key
`(0x81bf2e27273b27db, 0x80000400)` executed dynamically once for one instruction
with zero unsupported exits. Logical RDRAM, CPU, device, executor, ABI-host,
continuation, scheduler steps, and simulation time matched. Both publication
diagnostics reported the same pending `ExecutableWrite`, five-instruction last
charge, cumulative charge 100,001, and no prepared continuation. This single
operational diagnostic is not a ten-run parity claim.

Provenance of the removal: `crates/fn64-diff` once carried a subprocess client
for the *faki-tools* `oracle` CLI plus a mupen64plus `.m64p` savestate parser,
to drive this transplant path. Both were removed 2026-07-17 — the oracle client
because a client for another project's command line is precisely what §1.0
above forbids, and the savestate parser because the path it fed cannot work,
for the reason stated here. The historical run they produced is preserved in
`crates/fn64-diff/docs/2026-07-14-first-divergence-report.md`.

### 1.1 The two lanes: how the game arrives, and what draws it

Two independent switches select a build configuration. They are orthogonal,
and a symptom is only diagnosable once you know which lane produced it — the
same visual artifact means different things in each.

**Recomp lane — `FN64_RECOMP=c|rs`.** Which *form the game arrives in*:

- `c` (default): N64Recomp's emitted `RecompiledFuncs/*.c`, compiled and
  linked as before.
- `rs`: `fn64-cpu-runtime` emits the whole ROM as a typed-Rust crate
  (`recompile_rom`), linked directly.

The intended experiment is the same recompiled semantics in two forms, but
that is a precondition to prove, not an assumption the framebuffer comparison
may make. The current legacy OoT C corpus contains callable empty bodies that
the Rust driver recompiles. `scripts/lane-parity.sh` now compares the generated
body inventories first and rejects semantic authority when they differ; only
its explicit `--observe` mode will compare framebuffers under that admitted
limitation. The executable contract, current counts, and residual blind spots
are in `PARITY-METHOD.md`.

This is a *different axis* from §4's A/B, which swaps which **runtime**
implements the `_recomp` surface under one identical generated-C game; this
swaps the **game's form** under an identical runtime. §4's `nm`-based
completeness gate applies to the `c` lane's archive; the `rs` lane resolves the
same host surface through Rust linkage instead.

The native lane's official build preparation inserts one call to
`fn64_c_recompiled_function_enter` as the first statement of every generated
`RECOMP_FUNC` body. This location is intentional: `get_function` is only a
resolver, and its result can be cached, compared, or discarded without ever
being called, while ordinary generated C-to-C calls bypass it entirely. The
in-body hook therefore records both direct and indirect successful entries.
`fn64-abi` translates the callable pointer through the already-registered
generated section table and retains only `(section index, function offset,
link VRAM, guest cycle)` in exact entry order; probes and resolution misses do
not append. Installing a ROM or the process RDRAM clears the append-only
history. This authority is bounded to generated sources passed through fn64's
preparation function: a third-party native build that bypasses that pass has
no universal ABI call boundary and cannot claim complete entry observation.

The typed-Rust whole-function lane uses the equivalent mechanism at its own
single body template. `emit_function_resolved` writes
`notify_function_entry` before the local PC dispatcher in every emitted
callable, so root, direct sibling/tail, and lookup-resolved guest entries share
one boundary while host overrides and lookup misses remain excluded. A current
generated module exports `FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA`; the ABI
accepts authoritative installation only when the host passes that marker plus
a stable artifact identity. OoT derives the identity from a canonical,
path-independent wire over the exact emitter manifest contract and every
regular generated file under `src/`. Only the validated machine-local runtime
path is normalized; extra targets, features, dependencies, build scripts, and
symlinks are rejected. A stale or handwritten callable table therefore cannot
silently claim a complete stream. The committed-VI release boundary freezes
the exact `(cycle, artifact, link VRAM, symbol)` order and schema v29 binds its
ordered and canonical unique/count digests as `typed_observed_function`.

The same boundary freezes a separate ABI-owned RSP/RDP observation stream.
Before task mutation the ABI hashes the raw RDRAM prefixes required by pinned
MIT RT64's F3DZEX2 identity rows. For each graphics LLE generation, it also
hashes the complete live 4 KiB IMEM image and asks the registered backend only
for exact catalog recognition. The pinned identity wins agreement or absence;
a contradictory backend label traps. Neither source can choose the digest or
execution policy. Successful IMEM
replacement and DRAM/XBUS DPC commits enter the same ordered history. This is
release observation, not future-affecting DeviceState, so ROM installation
clears it and report schema `fn64.release-gate.v30` binds it independently.
Each microcode recognition entry also binds the original task data address,
exact logical byte length, and SHA-256 in the
`fn64.rsp-rdp-observations.v2` wire.

The emitted crate is out-of-tree, game-derived material, and per `AGENTS.md`
it must never enter git or the main workspace graph. That constraint — not
taste — is why the rs lane builds through **standalone manifests carrying
their own `[workspace]`** (`recomps/wm2000/packages/oot-boot/rs/Cargo.toml`,
`crates/fn64-shell/rs/Cargo.toml`) that reuse the sibling `src/main.rs` and
`build.rs` rather than duplicating them. A gitignored `recompiled` symlink is
refreshed from `RECOMP_RS_DIR` before Cargo resolves the graph. A standalone
manifest is the seam that keeps game-derived code out of the workspace; if
you find yourself adding the emitted crate as a normal path dependency, that
is the rule you are about to break.

**Render backend — `FN64_RENDER=reference|rt64`** (feature `rt64`):
`ReferenceBackend` is the pure-Rust, headless CI/seam-test backend and A/B
oracle; RT64 (§1's `fn64-render-rt64`) is the faithful lane. **Keep both — the
oracle is not obsolete once RT64 works.** ReferenceBackend is what keeps
`cargo test` GPU-free (RT64 needs Vulkan/D3D12/Metal, so headless CI would
otherwise need a GPU or software Vulkan), and it is the differential oracle the
wgpu port is gated against (ROADMAP P2).

An `OSTask` boundary is an RSP scheduling boundary, not an RDP reset. The
reference backend therefore owns one persistent RDP decode state shared by its
admitted F3DEX2 and raw-DPC entry paths: other mode, combiner/constants,
scissor/fill, texture-image/tile/TLUT registers, and physical TMEM survive.
Only the F3DEX2-owned `G_TEXTURE` enable/tile/scale selection is rebuilt per
task. Keeping this ownership in the backend prevents a new decoder invocation
from manufacturing reset registers or a white texture. `G_SETCIMG` is part of
that persistent RDP state: production F3DEX2/raw color writes require it and
never infer it from the independent VI scanout/`output_addr` state. Persistence
retains the register, not a private copy of memory: the selected image is
re-imported from RDRAM at each task boundary to observe intervening CPU/device
writes.

Operational detail (emit caching, the shared target dir, the `./oot` loop)
lives in `FAST-LOOP.md`; this section is only the shape and the why.

## 2. Threading model

### The invariant this model exists to enforce

**Exactly one game thread runs at a time.** This is not an optimization
choice, it is dictated by the ABI: `recomp_context` (per
`ABI-SURFACE.md` section (b), from `recomp.h`, MIT) is a plain mutable
struct of MIPS register state with no synchronization of its own, and
every `RECOMP_FUNC` receives `uint8_t* rdram` -- one shared, unsynchronized
byte buffer -- as raw pointer, not behind any lock. Real N64 hardware ran
one CPU; the recompiled C was generated assuming exactly that. A host
implementation that lets two "logical" OSThreads' recompiled C actually
execute concurrently on two host threads is not parallelizing a
parallel-safe program -- it is inventing a race the original program never
had and the ABI gives no tools to guard against.

### The evidence this is a real, not theoretical, failure mode

`aki-recomp/games/NWXE/profile.toml`'s rung 18 / 18b writeup (boot-ladder,
2026-07-14) is the definitive case study, cited here as our own evidence
(not GPL code -- we read our own debugger output, not vendor source):

- The crash: `EXC_BAD_ACCESS` inside `thread_queue_pop`, dereferencing a
  popped queue head that a caller-side `!thread_queue_empty()` guard had
  just certified non-empty, "with nothing else executing in THIS thread" --
  i.e. the queue's own head vanished between check and pop.
- Diagnosis ruled out the obvious suspects one at a time, with a hardware
  watchpoint as the actual tie-breaker (not a guess): four separately-named
  SI-manager candidate functions were individually cleared by full disasm
  read; a scheduler-wide `recursive_mutex` closing the check-then-pop TOCTOU
  was landed and confirmed live in the compiled binary (disassembly showed
  real `lock()`/`unlock()` bracketing) -- and the crash reproduced **20/20**
  at the identical site anyway, proving the mutex closed a real but
  different bug, not this one.
  - Prior rung's WCW_WATCH_ADDR-based diagnosis was *inconclusive/misleading*
    on this exact question (the same rdram address is reused earlier in boot
    by an unrelated function, and separately, `dladdr`'s `fn=` attribution
    on a large float-heavy function was shown to be an artifact of clang
    tail-merging near-identical slow-path stubs -- "do not trust
    WCW_WATCH_ADDR's fn= naming at face value... without cross-checking
    against a real hw watchpoint").
  - The eventual ground truth came from a **late-armed** real hardware
    watchpoint (armed only after the specific queue's creation, conditioned
    on the exact mq address) -- an env-var watch armed from process start
    could not isolate the actual writer among address reuse and other noise.
  - Final root cause identified: the field transitions via a **genuinely
    concurrent OTHER game thread's own recompiled MIPS code** executing
    `osSendMesg`'s blocking-insert path on the shared queue struct, touching
    raw rdram bytes with **no lock the scheduler API can see at all** --
    "it cannot stop two 'game' host threads from both executing arbitrary
    recompiled code that touches shared rdram bytes with no lock at all,
    which is the deeper version of the disease this rung's dispatch
    described."
- The explicit refusal on record: a "silently treat a low/implausible
  pointer as empty" guard was **drafted and reverted** -- "that would
  convert a hard, honest crash into silently losing a blocked thread
  forever."

The mechanism this whole rung exposes is upstream architectural, not a
one-off bug: giving every `OSThread` its own real host `std::thread` and
relying on a signal-then-return handoff (a semaphore signal without waiting
for the signaled thread to actually park) that has **no lock anywhere**
around `running_queue` or any `OSMesgQueue`'s blocked lists — so a second
"game" thread's recompiled MIPS code can be mid-instruction on shared rdram
at the same moment the first thread believes it has exclusive access. This
class of bug is exactly what the threading model below must make
structurally impossible, not merely less likely.

### OSMesgQueue's other invariant, independently confirmed (rung 12)

A second, independent piece of evidence about what the *data structures*
themselves assume, cited because it directly informs the `MesgQueue` design
in §3: rung 12 (`profile.toml`) found that leaving `osCreateMesgQueue`
un-named (its body still raw recompiled MIPS) meant every queue's
`blocked_on_recv`/`blocked_on_send` fields got initialized to a ROM
sentinel struct's address (`D_80048860`, a hardware dummy tail node with
`next=0, priority=-1`) instead of a real null. Runtime code that tested
"is anything blocked" via `*queue == NULLPTR` was always false against that
sentinel, so every send/recv treated it as a real blocked thread, and its
own `next` field (word `0`, reread as an in-rdram address) created a
self-loop that permanently corrupted the run queue's walk. **Lesson coded
into the design**: `osCreateMesgQueue`'s reset is not "zero some bytes," it
is "establish the empty-queue invariant these fields are load-bearing for,"
and that reset must be a single, non-bypassable constructor path — not
something any caller can reach around by writing raw fields (see the newtype
design in §2's `MesgQueue` below, and the `blocked-list ownership` point).

### Options evaluated

**(a) OS-thread-per-`OSThread` with a single-runnable baton.** One host
`std::thread`/`std::thread`-equivalent per `OSThread`, gated by a shared
token/mutex+condvar such that only the token holder may execute recompiled
code; `pause_self`/scheduler handoff releases the token and blocks on a
condvar until re-granted. This is architecturally what the reference runtime
already does (per the rung-18 evidence: "4 separate real host OS threads all
named 'game' alive simultaneously... this runtime gives every OSThread a
genuine std::thread, not a coroutine") — and rung 18/18b is the direct
demonstration of why it's fragile: the "single-runnable" property is an
*invariant maintained by convention across every call site that touches
scheduler state*, not a property the type system enforces. Every one of
`thread_queue_pop`/`insert`/`remove`/`schedule_running_thread` becomes a
place a missing lock (or a lock that's present but held over the wrong
window, per the fix that landed and still didn't close rung 18) reopens the
race, and — the harder problem — real game rdram touched by recompiled code
running on a second live host thread is *never* inside any of those guarded
functions, so no scheduler-level lock, however carefully placed, can close
it. Real preemption at the OS-thread level exists here even though the
model is trying to emulate a single core; correctness rests entirely on
every yield point being disciplined, forever.

**(b) Single executor + stackful coroutines (e.g. `corosensei`).** One real
host thread executes all game logic; each `OSThread` is a stackful coroutine
(its own machine stack, switched to and from cooperatively). "Only one game
thread runs at a time" stops being a discipline every future contributor
must maintain across N call sites and becomes **physically true** — there is
exactly one native call stack live in guest code at any instant, because
there is exactly one native thread executing it. A yield
(`pause_self`/blocking `osRecvMesg`/timer wait/scheduler switch) is a
`coroutine.yield()` back to the executor's scheduling loop, which picks the
next runnable coroutine per libultra priority rules and resumes it — all on
the same host thread, so "resume coroutine B" and "coroutine A's last write
to rdram" have a trivial happens-before relationship (sequential program
order on one thread), not a cross-thread visibility question requiring a
lock or atomic at all. `recomp_context`'s per-thread MIPS register state
naturally becomes coroutine-local (each coroutine owns its own
`recomp_context`, no shared mutable state to race on); the shared `rdram`
buffer is still shared, but now the only way two writes to it can interleave
is a yield point *the coroutine itself chose* (an explicit
`pause_self`/blocking-syscall boundary that the recompiled C emits), never
an arbitrary instruction boundary an OS scheduler picked. This makes the
rung-18 failure mode — "a second thread's recompiled code touches shared
rdram with no lock the scheduler can see" — **unrepresentable**: there is no
second thread.

The Rust recompiler lane uses the same model. Generated functions own
a safe `fn(&mut fn64_cpu_runtime::RecompContext, &mut Rdram)` ABI, while
`fn64-abi::recompiled` is the single adapter at the already-unsafe C host-shim
boundary. It marshals GPR/HI/LO/COP0 status into the legacy host context,
calls the existing queue/DMA/VI/thread shim, then copies architectural state
back. The adapter snapshots Status.BEV and rejects any shim transition before
copy-back, so an admitted legacy-C call cannot silently select the bootstrap
exception-vector family. `osCreateThread` constructs a recompiled context inside the same
`GameThread` coroutine; it does not create another executor, RDRAM image, or
host thread. Its child Status inherits the caller while clearing FR, so BEV
closure for a spawned thread depends on proving the caller's Status sources;
once those sources are closed, this is an inductive preservation edge rather
than a new blocker. The generated module also exports section `(ROM, static VRAM,
size)` geometry. The existing DMA load registry records relocated heap bases,
and host-first lookup maps a relocated callback back to its static typed
function entry. Thus rs and C lanes share scheduling, peripherals, and
memory ownership without pretending their register structs are layout-
compatible.

**(c) async (Rust `Future`s / an async runtime).** Model each `OSThread` as
an `async fn`, yielding at `.await` points, driven by a single-threaded
executor (e.g. a `LocalSet` / current-thread runtime). Shares (b)'s core
correctness property (one poller, one logical thread of control at a time)
but the ergonomic fit is poor for this specific workload: recompiled `C`
calls into `fn64-abi` are ordinary synchronous function calls with a
fixed `(rdram, ctx)` signature (per every extern surface entry in
`ABI-SURFACE.md`) — there is no natural `.await` point inside a
`RECOMP_FUNC` because the recompiled code was never rewritten to be async,
and retrofitting yield points would mean either (i) polling from inside a
non-async C call via a hand-rolled waker dance at every `pause_self`/blocking
call site (recreating stackful-coroutine mechanics on top of a strictly
worse primitive for this — Rust's stackless coroutines require the yield
point to be a syntactic `.await`, which recompiled C's call graph doesn't
have), or (ii) running each `OSThread`'s entire body as a blocking task on
a dedicated thread anyway, which collapses back into option (a)'s hazards.
Async's real strength — cheap concurrency for I/O-bound, deeply nested
call graphs with natural suspend points — doesn't match "run a fixed MIPS
call graph that suspends only at a handful of libultra API boundaries."

### Recommendation: (b), single executor + stackful coroutines

This is the load-bearing choice. Reasoning, mapped to the specific seams the
task calls out:

#### Clock and event authority

The executor owns one monotonic hardware-time domain measured in 93.75 MHz CPU
master cycles. VI, AI, PI, SI, RSP/RDP, CP0 Count, and timer deadlines are
derived from that deterministic time; the core never reads host wall time.
The shell may wait against wall time to avoid presenting emulated work early,
but it does not advance an independent audio, video, or game clock.
`EmulatedInstant` represents a position on this monotonic timeline, while
`Cycles` represents only a duration; the types permit instant-plus-duration
and instant-minus-instant, but not instant-plus-instant or an implicit wall
conversion. The executor clock, OS-timer deadlines, device-fabric clock and
event heap, VI epoch, AI start/deadline state, and device trace timestamps use
that distinction internally. Stable evidence encoders continue to write their
numeric cycle values, preserving the versioned wire while preventing runtime
code from adding two positions or treating a duration as a deadline.
The shell retains one immutable wall epoch and projects each exact VI deadline
from its absolute cycle position. It does not add rounded field durations,
replace the epoch when host work misses a deadline, or recalibrate from cpal
playback observations. Complete cpal anchors measure the host presentation
phase of the deterministic clock; they are never pace inputs.
Before that epoch can be established, the graphical shell submits one
content-neutral black field through the same pixels texture upload,
presentation shader, viewport, queue, and surface path used by ordinary
fields. This host-only prewarm cannot advance guest time or create a VI
observation. It prevents lazy host pipeline construction from becoming debt on
the first guest VI deadline; subsequent real stalls remain debt and the
immutable mapping repays them without changing the emulated clock rate.
The shell likewise starts the host cpal stream before establishing that epoch,
while an explicit inactive delivery gate makes every early callback emit
content-neutral silence without touching queued guest PCM, underrun health, or
continuity. The first authoritative AI DMA publishes its start and presentation
metadata before a Release activates delivery; the realtime callback must
Acquire that state before it can inspect the ring. Host device activation is
therefore setup, while guest AI start remains the sole authority that admits
tracked PCM to playback.
The cpal adapter uses the device's default callback geometry. Schema v9 records
the first callback size and every observed change together with each tracked
DMA's resampled payload and resulting ring depth. These are host measurements,
not N64 clock parameters. The adapter does not invent a guest-independent
padding span or use callback observations to retime emulated AI.

`FN64_PRESENTATION_TRACE` plus a unique `FN64_PRESENTATION_TRACE_ID` writes a
separate bounded JSONL stream at clean exit. It correlates continuity changes,
complete AI DMA playback anchors, and successfully presented VI fields using
both typed emulated cycles and nanoseconds from one host epoch. This host-only
stream is intentionally not part of `fn64-timing-trace`: a callback timestamp
or window-present timestamp cannot become deterministic device evidence.
Schema v8 retains each admitted guest task under the content-free key
`(task_offset, admission_generation)`. Creation occurs only after StartGo has
retained the admitted task lineage, so a record cannot claim an unretained
header. A resumable HLE graphics task moves its pending record with the owned
renderer continuation. Yield terminates that admission as `yielded`; the next
StartGo creates a new record for its new generation and names only the prior
generation in `resumed_from_admission_generation`. Completion and clean process
exit consume the record exactly once. Translated and LLE audio complete
synchronously with an RDP state of `not_applicable`; translated HLE graphics
uses `unavailable` because the backend exposes no executed CPU/compute member
census at that seam. Diagnostic routes are likewise explicit `unavailable`,
never relabeled as measured work.

LLE graphics moves its pending guest-task record into the owned raw-DPC batch.
The record completes only when that batch completes, using the backend's actual
CPU/compute member counts, queue ID, host-thread lane, and typed architectural
join reason. A clean exit may instead consume it as
`abandoned_at_process_exit` with unavailable RDP evidence; tracing does not poll
the worker or publish its writes. The shell validates every queued task by exact
batch ID and rejects duplicate task keys, invalid resume generations,
audio/RDP-applicability mismatches, or queue/thread/coherence combinations that
cannot arise from the ownership graph. Task type is the architectural
graphics/audio/other class only: no title, ROM digest, microcode digest,
function address, command bytes, pixels, or samples enter this stream.

Schema v10 records each dispatched production raw-DPC task batch as
one bounded host observation. A completed batch carries the sequential dispatch
cycle/host sample, exact publication and completion cycles, worker execution span, typed
architectural join cause and
complete request/return span, and the emulation-thread staged-write, commit,
copyback, and publication durations. It also names the CPU dispatch authority,
the interpreted RSP route that creates this batch, the backend's actual
CPU/compute member counts after admission, and the emulation or persistent-RDP
host thread. The monotonic batch ID is also the raw-DPC queue identity. A
content-free `member_timings` array preserves each member's ordinal, decimal
DPC transaction identity, structural command/wire/triangle/rectangle/sync
counts, and every interpreted DP_END byte boundary and RSP step. Triangle
counts are positional raw opcodes `0x08..0x0f`; a synthetic boundary carries
JSON `null` instead of inventing an RSP step. No command bytes, addresses,
pixels, samples, or game identity enter this array. A
compatibility backend reports the RDP lane as unavailable rather than
laundering a plan into execution evidence. The join cause is the first typed
host consumer that forces architectural coherence; it is not a claim about an
internal GPU readback. If process exit finds the one allowed
batch still pending, the terminal path consumes only its diagnostic metadata
and emits `render_batch_incomplete` with its dispatch identity and
`process_exit_before_completion`; it does not poll or resume the worker,
publish guest writes, or complete a device event for tracing.
`vi_visibility`, `later_graphics`, `dmem_dependency`, and the combined latter
two are the complete join-cause set. These timestamps flow back through the
worker's owned completion message and are drained on the same shell/emulation
OS thread that enabled the trace; no worker logs or reads a host-global trace
configuration. Enabling the trace adds clock reads at task-batch boundaries,
so instrumented absolute timings require a matched uninstrumented control.
None of these observations can create a guest deadline, complete DP, or change
which architectural barrier settles the renderer.
For a transactional batch that reaches FullSync, v10 separately binds the
typed schedule receipt `(scheduled_cycle, deadline_cycle)` to the real
DeviceFabric `RcpTaskComplete(Dp)` notification. A clean notification emits
`render_batch_dp_completion` with `completion_cycle == deadline_cycle`; clean
process exit instead emits `render_batch_dp_incomplete` without advancing the
device for tracing. Both records join by monotonic batch ID and are distinct
from renderer-worker completion: host readiness publishes the batch but is
not DP timing authority. A batch without FullSync legitimately has no DP
terminal record. These records expose the current policy for measurement;
they neither justify nor select its deadline.
Schema v8 additionally brackets backend VI scanout separately from window
composition/submission and samples the current host activity in each callback
that inserts silence. Its callback-to-emulation transport is bounded and
allocation-free on the realtime producer; any dropped diagnostic record is
serialized rather than converted into a false zero-underrun or complete-
attribution claim. At terminal process exit the shell destroys the host audio
producer before the final probe drain and trace seal, closing the callback-
after-drain interleaving without blocking the realtime callback during normal
operation. These spans measure host API extents, not physical display scanout
or speaker output.
`scripts/summarize-presentation-trace.py` compares the host-minus-emulated-time
offset of each successfully presented field with the nearest complete audio
DMA playback anchor. Its residual measures the relative host phase at those
two API boundaries, not physical display scanout or speaker output latency.
Over the common emulated-cycle interval it also fits each host projection
independently and reports video-versus-audio rate in parts per million plus
phase drift in milliseconds per minute. A fixed offset and a continuing pace
error are therefore distinct observations; neither estimate feeds scheduling.
An optional opaque `FN64_AV_SYNC_CUE_ID` requires the audio and video probes
and adds one explicit exact-cue result. Its audio half names the DMA, source
frame offset, start cycle, programmed DAC rate, AI clock, predicted callback
playback instant, and continuity generation; its video half names the exact
hash occurrence, stage, presentation generation, VI edge, and successful
present return. The pair is absent—and the summarizer reports an invalid
measurement—after a drop, retime, missing half, or continuity change. The
summarizer recomputes both rational guest-cycle and signed host-time deltas
from the two halves and rejects any identity or arithmetic mismatch. Cue
semantics and reference correspondence remain external experiment inputs.
The same report summarizes worker duration, guest overlap before its join,
architectural join wait, and emulation-thread finish phases. GPU query spans
remain backend-local until they can carry the same task-batch identity without
introducing a queue wait into the ordinary path.
`vi_visibility` identifies why an architectural join occurred; it is not a
foreign key to one `vi_present` record or presentation generation. The report
therefore aggregates renderer spans and does not attribute a batch to an exact
presented field.

Libultra `OSTime` is a distinct typed domain. The public `osGetTime` and Timer
Manager manuals define it at the CP0 Count rate, one tick per two CPU master
cycles. `OsTime` therefore derives from the monotonic clock with exact integer
division, and `osSetTime` changes only a wrapping bias. It cannot move hardware
time, Count, or an already-armed deadline. `osSetTimer` converts its `OSTime`
durations back to master cycles exactly once at the ABI boundary; overflow is
a named trap. This separation prevents a title's game clock from advancing at
twice its documented rate while VI and AI remain correctly configured.

The host asks one deadline projection for the next runnable HLE continuation,
device-fabric event, or OS timer. Both idle host advancement and translated
instruction checkpoints walk every intervening combined deadline while the
guest coroutine is suspended. Each boundary advances the monotonic clock and
CP0/clock-driven state, commits hardware effects and notifications, then
delivers equal-cycle OS timers. Only after the requested target and all of its
same-cycle work are committed may the single executor choose the next guest
coroutine. A translated checkpoint is still the minimum interruptible codegen
unit: the scheduler preserves exact intermediate event times inside that unit
but cannot resume another coroutine before the checkpoint. Repeating timers
re-arm from their prior deadline and therefore retain phase and every elapsed
expiration rather than shifting to `now + interval`. Rendering may execute on
an owned worker, but VI
publication and all RDRAM, audio-production, queue, timer, and scheduler
authority remain on the emulation thread. This preserves hardware ordering
without making a renderer or audio callback another source of emulated time.

- **`pause_self` / yield sites.** Each libultra call that can block or
  voluntarily yield (`pause_self` itself — 3 call sites in NWXE, 2 in NW4E
  per `ABI-SURFACE.md`'s dispatch-helper table; `osRecvMesg_recomp` when the
  queue is empty; a blocking `osSendMesg_recomp` when the queue is full,
  the exact path rung 18b root-caused) becomes a single `yield_now()`-style
  call into the executor from inside the current coroutine. The executor's
  resume logic picks the next runnable `OSThread` by the same priority rule
  libultra specifies (see `osCreateThread`/`osSetThreadPri`'s semantics —
  highest-priority runnable thread runs) and resumes its coroutine, which
  is the *only* place execution physically transfers between "threads." No
  call site anywhere else in the runtime can accidentally run two
  `OSThread`s' recompiled code concurrently, because there is exactly one
  coroutine ever resumed.
- **VI/timer event delivery.** VI retrace and timer expiry are host-side
  events (real wall-clock/vsync driven), not guest compute — they must be
  able to interrupt/wake a blocked coroutine (e.g. a thread parked on
  `osRecvMesg` from `OS_EVENT_VI`) without themselves being a second
  "runnable game thread." Model them as executor-level scheduling inputs:
  the host VI/timer driver (in `fn64-runtime`, no coroutine of its own)
  posts to the target `OSMesgQueue`/marks the target coroutine runnable and
  returns; the *executor's* next resume decision (still made from the single
  active coroutine's yield point, or from the top-level scheduling loop
  between coroutine turns) is what actually runs the woken thread's code.
  This mirrors real hardware exactly: a VI interrupt on real N64 doesn't
  execute game code itself, it posts a message and returns to whatever the
  CPU was doing; libultra's own scheduler decides what runs next.
- **SI/PI completion messages.** Same shape: DMA completion is host-driven
  (a real disk/cart read finishing, or in fn64's case a host-file-backed
  ROM read finishing), and the correct model is "post the completion
  message to the registered `OSMesgQueue`, let the next coroutine-resume
  decision (not a new host thread) pick up the woken thread." This is
  exactly the shape `ultramodern::send_si_message`/`dequeue_external_messages`
  is evidenced to have in the rung-18b writeup — an external (non-coroutine)
  message source feeding the same queue machinery a blocking `osSendMesg`
  from guest code feeds — the design difference is only that in fn64 there
  is no second real thread that could race the queue mutation, because the
  actual mutation of "make thread X runnable" is executor-owned state
  touched only between coroutine resumes.

  The live device implementation now makes that ordering structural. Its
  RCP/MI authority exists from `HostState` construction and is not optional or
  coupled to cartridge ROM load order; a separate `rom_installed` invariant
  keeps PI DMA's missing-content path loud.
  `DeviceFabric` owns PI's sole in-flight hardware request and guest-cycle
  deadline. An ABI-side FIFO models the PI manager's accepted managed work:
  `osEPiStartDma` requests submitted while that hardware slot is occupied wait
  in order and still return success, while raw PI starts retain loud busy
  behavior. This distinction is load-bearing: exposing hardware `PiBusy` to a
  second managed caller made OoT's DmaMgr report a multi-chunk load complete
  after only its first chunk. Managed EPI, raw PI APIs, and typed-Rust PI
  register writes otherwise converge on the same fabric. Public
  `OSPiHandle` state is not an opaque host token: one ABI decoder validates
  its public type/domain/timing/base fields, including the uncached KSEG1
  base-address form shown by Chapter 27's SRAM acquisition example, publishes
  timing through that fabric's raw PI registers, and applies the documented
  KSEG1 `baseAddress | devAddr` rule for managed DMA, raw DMA, and programmed
  I/O before converting the result at the physical PI boundary.
  The public runtime request carries a typed device-relative
  `PiDeviceAddress::{RomOffset,SramOffset}`; storage never infers a device from
  an offset threshold. Raw MMIO alone retains the physical `PI_CART_ADDR`
  latch for register readback and snapshots, then decodes Domain 1 Address 2
  or Domain 2 Address 2 when a length write triggers the transfer. Admission
  checks the complete half-open device range without wrapping at either PI
  window boundary; typed-start failures leave every readable latch and trace
  unchanged, while an explicitly programmed raw CART latch remains readable
  after a rejected length trigger. SRAM chip addressing inside an admitted
  Domain 2 range still masks the installed power-of-two part's real address
  lines and may wrap once; a transfer longer than the whole part traps. In the
  public PI register convention, `PI_WR_LEN` starts device-to-RDRAM and
  `PI_RD_LEN` starts RDRAM-to-device; a ROM write is a loud typed rejection.
  The managed API reports that read-only rejection at submission, whereas the
  raw ABI traps at its length-register boundary; that public API distinction is
  retained rather than relabeled as timed transfer parity.
  The supported Game Pak ROM and SRAM physical spaces normalize into the
  fabric's one storage authority. A malformed handle or a documented bulk/64DD space without
  attached storage records `abi.pi.epi-handle` before trapping; it cannot be
  guessed from pointer inequality or routed into zero-filled ROM bytes.
  `osCartRomInit` and `osLeoDiskInit` write the same public handle layout
  from typed domain state. Provenance: public libultra
  Programming Manual Chapter 27, “EPI Manager” and “SRAM,” plus the public
  `osEPiStartDma` and `osEPiRawStartDma` function pages.
  At the deadline it writes the process's one RDRAM allocation, clears PI
  busy, raises MI PI pending, and only then returns an executor notification.
  `advance_virtual_time` injects that notification before it returns. The
  translated checkpoint path also suspends first, advances executor time, and
  commits the fabric in `fn64-abi::run_one_step` before any later resume. The
  ordinary ROM installation seeds Domain 1 latency, pulse width, page size,
  and release from the normalized cartridge header, then schedules completion
  with the transfer geometry and programmed domain registers. The public
  Programming Manual Chapter 27 defines those controls but not an exact
  completion equation; `RcpPiTiming` independently restates the formula from
  ares' ISC-licensed PI model at commit
  `e4217366cf01f963441a9664197c36430400e70d` and converts its RCP clocks to
  fn64's 93.75 MHz CPU-cycle domain. This is deterministic, reference-derived
  compatibility evidence, not silicon-trace certification. Synthetic hosts
  can still request an explicit `FixedPiTiming` policy without changing PI
  state or event ordering. The same fabric owns AI's complete guest
  register latches and two-slot FIFO; shim calls and raw register writes do not
  retain a second DAC-rate, source-address, or control authority.
  It derives deterministic drain deadlines from stereo-frame count, the
  93.75 MHz CPU clock, and the exact public `VI_CLOCK / (DACRATE + 1)`
  rational, with one final ceiling; the integer rate returned by
  `osAiSetFrequency` remains ABI/backend metadata rather than a device-clock
  input. The shim computes its divisor with exact integer round-to-nearest and
  admits fn64's bounded 132..=16384 range without mutating state on rejection.
  Typed starts reject unaligned or out-of-field DRAM/LEN values and 24-bit
  range overflow; raw register writes apply their public masks first.
  Per public `rcp.h`, retiring the current slot while a next slot exists raises
  MI AI and returns OS_EVENT_AI after promotion makes FIFO FULL transition
  1-to-0. A lone/final BUSY transition does not fabricate that edge. This is
  guest-time ordering, not a claim of hardware-verified AI bus timing. DACRATE
  and BITRATE writes while either FIFO slot is occupied fail with named faults
  because their active-transfer hardware behavior is not yet admitted. Its DAC
  divisor uses the same IPL-selected NTSC/PAL/MPAL video clock as VI. Exact AI bus
  clock-domain phase, per-edge `AI_LEN` decrement timing, other interrupt
  causes, hardware counter edge behavior, and native-C instruction-interior
  observation remain open. SP, SI, VI, PI,
  AI, and DP pending bits and masks are
  one level-sensitive gate. Typed raw writes apply the acknowledgement commands
  documented by the public `rcp.h` register definitions: SP status bits 3/4,
  any VI_CURRENT/AI_STATUS/SI_STATUS write, and MI mode bit 11 for DP. In the
  same fabric, SI owns persistent 64-byte PIF RAM and schedules distinct
  DRAM-to-PIF command and PIF-to-DRAM response transfers. Completion order is
  `PIF/RDRAM bytes -> SI idle -> MI SI -> OS_EVENT_SI`; the current one-cycle
  deadline is an explicit policy because the public register definitions do
  not supply a transfer-cycle formula. Above that physical PIF authority, the
  ABI owns the public libultra Controller Manager lifecycle and polling prefix:
  `osContInit` initializes once with four channels, while a later
  `osContSetCh(ch)` limits high-level query/read copies and `osPfsIsPlug` to
  ports `0..ch`, leaving query/read caller storage beyond that prefix untouched.
  A pre-init `osContSetCh` retains the four-channel default, and a count above
  `MAXCONTROLLERS` traps. Controller initialization now validates the supplied
  initialized, exclusively idle `OS_EVENT_SI` queue, encodes all four query
  channels into the fabric-owned PIF RAM, blocks internally, and publishes its
  bit pattern/status only after the timed SI completion wakes it. Subsequent
  query/read starts validate the supplied event target and encode the manager's
  current channel prefix into that same device-owned packet. Their getters
  decode only the completed PIF RAM image: input changes after completion and
  later `osContSetCh` calls cannot rewrite or expand an already-finished poll.
  The fixed SI compatibility latency remains channel-independent because the
  public manual gives only approximate per-channel savings, not an exact cycle
  formula. Device evidence retains the exact pending request and complete PIF
  command/response bytes without a host pointer. As elsewhere in the native-C
  lane, `osContInit`'s suspended coroutine continuation (including its guest
  output destinations) remains outside portable executor evidence; no broader
  continuation claim follows from the device transaction. `osPfsIsPlug`
  validates that its caller-created
  queue is the live `OS_EVENT_SI` target and is exclusively idle: queued
  messages and either blocked-receiver or blocked-sender role reject the call
  loudly, so an older waiter cannot steal its completion. It then enters the
  same timed `ControllerQuery` fabric. A typed transaction owns the caller
  thread, exact queue/message route, registered-RDRAM result address, and
  latched Pak bitmap across both hardware-pending and completion-posted phases.
  This makes the future output fixed-cycle evidence rather than an invisible
  coroutine-stack local. Completion posts directly through that captured route;
  the coroutine consumes the matching transaction and writes the bitmap only
  after byte commit, SI-idle, MI-SI, and completion-message order are established.
  A busy SI start returns the older public function page's
  failure value `1` without touching the output; the later 5.2 page instead
  documents `-1`, so version-specific return-code parity remains bounded by
  the title's linked libultra revision.
  The manager policy is future-affecting ABI evidence; raw packets retain their
  explicit channel addressing and both paths observe the same `PifModel` port
  identities and input. Provenance: public libultra Programming Manual Chapter
  26, “Controller Manager,” plus the public `osContSetCh` function page. Raw
  controller query/read commands are implemented; other raw PIF device commands
  remain loud gaps. The manual also describes approximate polling-time savings
  as channels are removed, but does not provide a transfer-cycle formula. The
  fabric's explicit one-cycle SI compatibility policy therefore does not yet
  vary by controller count; channel-count-dependent `osContSetCh` timing
  parity remains unverified. The fabric also
  owns the RSP's persistent 4 KiB DMEM/IMEM, PC, status, atomic semaphore, and
  double-buffered SP DMA. DMA forces public 64-bit alignment, decodes
  length/count/skip rows, commits at an eight-setup-cycle plus one-cycle-per-
  64-bit-beat deterministic deadline, and increments an IMEM generation only
  after commit. RSP execution admits the installed physical RDRAM (8 MiB in
  the current console profile) plus only the static-storage ranges of overlays
  the registry proves are loaded. The latter are a static-recompiler seam:
  generated overlay code retains absolute link-time data pointers (for example
  ConsoleLogo's `G_MOVEMEM` pointer `0x80800920`) where the console's overlay
  relocation would have rewritten the instruction, and PI already mirrors the
  loaded image at that explicit static alias for CPU access. The admitted
  extent is the union of registered text geometry and bytes actually committed
  by the PI static-image mirror, so trailing overlay data is included without
  blessing the unused gap before the next section. The rest of the
  larger host allocation—including raw RCP/cartridge windows and unloaded
  overlay space—remains invisible to SP. Every rectangular DMA row must fit
  wholly inside one merged admitted range and otherwise traps with its
  descriptor and first invalid row; zero-filled host address space cannot turn
  a corrupt SP pointer into a silent transfer. The
  Scalar DMEM halfword/word accesses walk architectural big-endian bytes at
  the complete 12-bit effective address, including unaligned and bank-wrapped
  accesses; native-word backing is only a storage representation, never an
  unaligned host integer view. The public `osSpTaskLoad` sequence copies all 64 OSTask bytes
  to DMEM `0xfc0`, aligned rspboot bytes to IMEM zero, resets PC, and clears a
  preceding task's SIG0/SIG1 yield handshake. `osSpTaskYield` writes the public
  `SP_SET_YIELD`/SIG0 command and returns immediately. After SP completion,
  `osSpTaskYielded` observes SIG1; a real acknowledgement sets
  `OS_TASK_YIELDED` and replaces `ucode_data`/`ucode_data_size` with the task's
  yield-buffer fields, while normal completion returns zero and leaves the
  task untouched. These semantics come from the public *RSP Programmer's
  Guide*, "Task Yielding", the `rcp.h` SP signal definitions, and the
  `osSpTaskYielded` manual page. The query never dispatches a backend or ucode,
  preventing a completed task from running twice. A renderer returning
  `FrameStatus::Yielded` drives the same SIG1 state and schedules SP completion
  without a premature DP completion; missing or failed renderer operations all
  pass through one loud gate rather than synthetic-completing. Reloading the
  rewritten task then calls the backend with `OS_TASK_YIELDED` and the saved
  data range, providing cooperative HLE resume. A second, typed continuation
  protocol lets a capable backend return an opaque token only after committing
  a real chunk. The fabric keeps SP busy without inventing a deadline; the next
  host scheduling boundary checks SIG0 before consuming that token. A hit
  moves the sole token to `Suspended`, sets SIG1, and schedules SP completion;
  reload/start validates the same task address and public yield-buffer rewrite
  before consuming it exactly once. The ABI never serializes or reconstructs
  backend-local stacks. Known graphics/audio admission is
  classified by image shape. An ordinary boot-overlay task runs admitted
  rspboot through its real scalar interpreter until control first reaches an
  IMEM range installed by read DMA. A direct task whose physical
  `ucode_boot == ucode` and whose aligned boot copy covers the complete ucode
  is already at ucode PC zero after `osSpTaskLoad`; it enters HLE there, or
  starts accuracy LLE from the live admitted image, without misinterpreting
  the ucode's terminal BREAK as a failed rspboot handoff. Equal pointers with
  an incomplete copy remain on the boot-overlay path and trap loudly rather
  than admitting truncated content. RDRAM DMA writes, DMEM, the final IMEM
  generation, SP status, and ucode entry PC commit before the HLE backend
  represents the loaded-ucode phase; BREAK, DPC submission, or a bounded
  failure before an ordinary rspboot handoff traps loudly. Exact HLE calls consume
  the public task contract, while a transactional LLE fallback carries a typed
  snapshot of all non-memory RSP state from rspboot into the interpreter.
  Audio's next migration seam can instead acquire the exact Running task
  generation as a non-cloneable `InFlight` owner before copying any state,
  then execute pure owned rspboot once and fork its proven entry into HLE and
  LLE lanes. The reference with no deferred DPC submission retains the pre-boot 8 MiB/RSP baseline,
  exact boot-plus-ucode write intent, ordered IMEM generations, final LLE
  machine state, and measured phase work without publishing an intermediate
  boot state. It carries no commit authority and is not selected by live
  policy until a concrete audio-family HLE executor compares exactly; the
  current memory-command characterization and missing DSP arithmetic keep that
  frontier loud.
  Graphics microcode selection is an explicit host policy:
  `HleOptimized` preserves the interactive compatibility path and its exact-
  digest transactional fallback, while `LleAccuracy` always continues the
  loaded graphics ucode from that same rspboot snapshot through the interpreter
  and exposes only its raw DPC submissions to the renderer. The generic
  `set_render_backend` entry point intentionally defaults to `HleOptimized`;
  release/parity harnesses must opt into `LleAccuracy` through the typed
  registration API, so an accuracy claim cannot depend on an ambient flag or
  silently change unrelated callers. Unknown
  and custom tasks execute from that persistent image through the clean-room
  scalar/vector interpreter: IMEM DMA replaces a generation and resumes at the
  saved PC; BREAK commits DMEM, RDRAM DMA writes, status, and DRAM/XBUS DPC
  submissions before the guest resumes. Each submission owns exactly one
  source-typed command image: logical XBUS bytes
  or canonical RDRAM words captured at CMD_END, never independently mutable
  copies of both. Deferred dispatch consumes and coalesces those owned images;
  it does not reread a command range after later RSP writes can change it.
  The release-evidence encoder derives the historical XBUS word image directly
  from those owned logical bytes while serializing, preserving its established
  wire schema without retaining a second mutable command representation.
  The interactive all-Rust shell registers `WgpuBackend` through one persistent
  owned raw-DPC worker boundary. The backend moves to that worker for each
  batch and returns on completion; the host thread itself is reused rather
  than created and joined per batch. Accuracy LLE still executes the loaded
  RSP program on the one emulation thread. Once BREAK has committed its
  DMEM/IMEM and DMA writes, the ABI activates the first reserved DPC range
  (making the modeled RDP busy), moves only sealed raw-DPC tickets and backend
  state to the worker, and schedules SP completion independently. This matches
  the public RCP programming model's separate SP and DP processors: the scheduler may run a
  later audio task after SP completes while the RDP continues rasterizing.
  It does not permit two guest OSThreads or two RSP tasks to execute together.
  If that audio task finishes while a DPC transaction is live, the fabric
  commits its independent SP register image only when the interpreter echoed
  every DPC register exactly; a DPC mutation still rejects as busy, while the
  live transaction retains its registers and rollback authority. The standard
  RSP boot itself waits when the live DP source is DMEM and DMA remains busy.
  Before entering the synchronous interpreter, the host represents that exact
  dependency by joining the typed DP owner instead of burning the interpreter's
  instruction bound in the boot's DPC-status polling loop.

  Renderer completion returns the backend and prepared tickets to the
  emulation thread. That thread alone replays guest-ordered CPU halfword
  observations, validates and copies render-target payloads, commits DPC
  registers/physical state in reservation order, and schedules DP FullSync.
  VI is the visibility barrier: if a field reaches its scanout deadline while
  the worker is outstanding, the host joins and publishes before VI borrows
  RDRAM. A join may schedule DP FullSync at the fabric's earlier current-time
  successor after the coordinator has selected the VI boundary. The
  coordinator therefore recomputes and restarts at that new minimum before
  advancing the executor; DP cannot be delivered at the later VI cycle merely
  because the worker completed during settlement. The worker never holds a
  live RDRAM pointer or device-fabric borrow.
  Worker readiness is not itself a scheduler deadline and generic host
  boundaries do not poll it. Publication occurs only at a typed architectural
  barrier: VI visibility or a later graphics/DMEM dependency. Both join
  regardless of whether the worker had already finished or finishes while
  waiting, so OS thread/GPU wall timing cannot select the emulated DP cycle.
  Guest quiescence is not an RCP observation and therefore does not force a
  join. Terminal process teardown joins the worker to recover its owned backend
  but deliberately abandons its unpublished result.
  `RenderBackend::deferred_non_rdp_write16_disposition` is capability-gated so
  a future backend with a synchronous hidden-bit sidecar cannot be threaded by
  accident; WGPU currently declares `NoRustHiddenSidecar` and deferred writes
  are replayed before publication or reuse.

  This is ordering fidelity, not cycle accuracy. The barrier that demands a
  result is not a modeled RDP completion deadline, and the model still lacks
  silicon CURRENT/counter progression,
  FIFO capacity, FREEZE/FLUSH during an outstanding batch, and multiple queued
  graphics batches. Audio/SP work can overlap the outstanding batch, while a
  later graphics task applies DPC backpressure before renderer-backed
  microcode identification; it neither overtakes the batch nor fabricates
  queue capacity. These nonclaims preserve an extension point for a later
  evidence-derived RDP timing model without coupling renderer throughput to
  guest cycles.
  Every renderer task and DRAM-backed
  raw-DPC entry receives an 8 MiB physical-RDRAM
  view. Registration must cover that complete device, including its final
  byte, while the generated-code allocation's appended MMIO/non-RDRAM backing is
  never exposed or transactionally cloned. Captured XBUS/LLE command words use
  a synthetic suffix and only the physical prefix is copied back, but RDP
  commands can address that suffix during execution. The transaction image is
  retained as thread-owned scratch after completion and completely overwritten
  before its next admission; reuse changes allocation churn, not rollback or
  evidence authority. Exact RT64 LLE captured-
  DPC execution therefore remains a release residual until the native seam
  accepts a separate command buffer and enforces physical-memory bounds. One
  fabric-owned DPC register file and typed
  pending transaction retain START, END, CURRENT, STATUS, source (RDRAM or
  DMEM), range, and ownership token until the renderer commits or cancels it;
  raw MMIO, LLE, and shim submissions cannot bypass that state. The ordinary synchronous DPC model treats
  `START == END` as the public empty-FIFO initialization and emits only each
  newly exposed `[CURRENT, END)` span, advancing `CURRENT` after consumption;
  repeated `END` writes cannot replay an already-rendered prefix. Exact
  hardware DPC counters and latency, FREEZE/FLUSH interaction, subword register
  access, native execution paused mid-transaction, and silicon bus behavior
  remain open. An additive phase-A scheduling seam can represent future
  evidence-derived progress without changing that production model: runtime-owned
  transaction/quantum/cursor types stop at an exact external-work barrier and
  accept only the matching acknowledgment, while the ABI owns any renderer
  continuation. Its schedules are explicit inputs used by deterministic synthetic
  tests; they grant no RDP-cycle, intermediate-CURRENT, counter, FREEZE, or FLUSH
  authority. An additive runtime-only two-stage form names command-ingested and
  effects-visible barriers on the monotonic `EmulatedInstant` timeline. The
  caller supplies the complete ordered barrier list, including same-cycle
  order. Runtime privately mints a move-only receipt for each due barrier and
  accepts it through distinct commit or failure transitions, retaining
  separate ingested and visible cursors. This is only a transactional type seam: it is
  not wired to ABI or device production, does not establish that either named
  stage matches silicon, and derives no deadline, chunking, CURRENT, counter,
  FREEZE, FLUSH, interrupt, or visibility policy.
  The production atomic path represents its existing single
  synchronous backend call as one identity-only acknowledgment through the
  same validator before shadow publication; this changes no timing, device,
  interrupt, rollback, or digest authority and does not select chunking.
  Existing backends remain on the atomic path. Graphics HLE preflight is
  transactional and content-addressed: selecting an HLE decode mode admits no
  content. Both HLE backends return `NeedsLle` when the task-entry IMEM digest
  is unregistered; the reference renderer additionally decodes admitted tasks
  against cloned RDRAM/RSP state and rejects an unadmitted `G_LOAD_UCODE`
  generation. The clone is discarded and the complete ucode phase runs from
  untouched post-rspboot memory and scalar/VU/SP/DMA/DPC state through that
  interpreter. DRAM and staged XBUS
  DPC ranges then reach either the Rust raw renderer or RT64's bounded LLE RDP
  entry with the submission boundary's explicit VI output address; no raw path
  infers it from a preceding HLE call. Each successful backend operation also
  returns typed `Reached`/`NotReached` FullSync evidence. HLE derives it from
  the admitted display-list operation stream; raw DRAM and staged XBUS ranges
  use the backend-neutral `fn64-render` inspector to walk exact public command
  widths, so a triangle coefficient that resembles opcode `0xe9` cannot
  fabricate completion. `Unidentified` is accepted only
  as a backend's pre-operation state and traps if a successful operation leaves
  it unresolved. This implements the public RDP Programming Manual's
  Sync Full command-to-DP-interrupt relationship without treating every
  graphics task or every DPC range as if it had reached FullSync. This avoids an
  impossible fabricated mid-HLE scalar/VU transplant while preserving BREAK
  and DRAM/XBUS DPC effects. The scheduler now supports actual mid-HLE SIG0
  preemption at backend-declared committed chunk boundaries. The pure-Rust
  `ReferenceBackend` now owns its decoded operation stream, active color/depth
  targets, primitive-depth registers, dirty state, and cumulative FullSync
  evidence in a typed checkpoint. It commits one `RenderOp` to RDRAM per call,
  returns a fresh opaque token while operations remain, removes that token
  before executing the next operation, and rejects stale, mismatched, or
  overlapping task ownership by name. Its atomic `process_task` entry exposes
  no continuation or guest interleaving point, so it executes the same ordered
  operations in one call and commits at color/depth target changes, FullSync,
  and task completion rather than rewriting a dirty full image after every
  operation. Atomic-vs-chunked tests require identical final RDRAM,
  framebuffer, and FullSync evidence. RT64 remains
  `Atomic` because its public native task call exposes no resumable
  continuation. Completion no longer wakes
  the scheduler from inside `osSpTaskStartGo`: the fabric schedules SP at the
  measured pre-ucode instruction count (zero for a direct image) plus one HLE
  policy cycle. It schedules the later DP event only when that operation's
  evidence says FullSync was reached. A raw CPU/RSP DPC FullSync schedules DP
  without fabricating an SP event. The DP deadline remains one cycle after the
  SP deadline, or one cycle after a raw synchronous submission, preserving
  deterministic ordering while making no hardware-timing claim. Hardware-
  derived RSP/RDP latency remains a prerequisite for exact timing. Native RT64
  chunking still requires an upstream-owned checkpoint representation; a
  yield-buffer image cannot reconstruct an arbitrary host call stack or
  renderer-local state.
  Exact task-entry and self-loaded microcode admission is likewise owned by
  `fn64-render`: catalogs bind the complete IMEM SHA-256 to an explicit public
  wire family, while release recognition can additionally bind the exact data
  image identity. Backends consume that shared mechanism rather than carrying
  independent digest maps. The admission rule follows the public GBI family
  boundaries; it does not infer compatibility from a task header or colliding
  opcode byte. The RT64 transactional preflight additionally freezes an
  immutable shared `TaskAdmissionPlan`: task entry is generation zero and
  every admitted `G_LOAD_UCODE` follows in executed order with physical
  addresses, complete text/data identities, and behavior-bearing microcode
  identity. F3DZEX2 cannot be represented there as a broad family: plan-v2
  requires its classified 2.06H, 2.08I, or 2.08J variant and hashes that tag.
  Duplicate
  addresses and `A -> B -> A` generations are deliberately retained. The
  native adapter consumes that plan at pinned RT64's pre-cache
  `loadUCodeGBI` boundary, compares the live raw recognition windows, forces
  recognition for every generation, and preserves the old active GBI through
  the self-load flush before applying the admitted replacement. Unknown or
  incompatible generations return typed `NeedsLle` before live interpreter
  mutation. Missing, extra, reordered, or changed generations after execution
  begins poison the native context and fail loudly.
  The native schema-v2 generation wire mirrors that variant in
  `expected_detail`; preflight requires NoN for all three F3DZEX2 rows and the
  variant-specific point-lighting capability. Its immutable raw pool remains
  the I/J discriminator because pinned RT64 exposes the same native flags for
  those two variants. This typed plumbing does not itself admit the F3DZEX2
  command decoder. `GeometryUcodeProfile`, backed only by the typed admission
  identity, now survives the shared inspector and reference decode state plus
  catalog-admitted self-load transitions. All three typed F3DZEX2 profiles
  apply the bounded NoN near-admission policy, while ordinary F3DEX2 retains
  its near gate and side/far clip codes keep the existing raster-clipping
  handoff. Exact clipping remains a separate trace frontier. Until point-light
  behavior and exact F3DZEX2 raw-pair self-load resolution pass their separate
  gates, the production catalog continues to select LLE.
  A non-default `f3dzex2-characterization-evidence` feature exposes one
  explicitly evidence-named RT64 method outside `RenderBackend`. It accepts no
  caller-selected identity: the exact raw task-entry pair selects 2.06H,
  2.08I, or 2.08J, the logical plan is derived from live RDRAM/IMEM, and the
  existing native schema, context-poisoning, and full RDRAM/RSP rollback
  transaction execute the entry generation. The result retains the native
  workload counter before and after execution. The repository-owned v1 suite
  expands eight fixed policy rows into two public controls plus all six
  point-light hypotheses at each 16/24/32-byte candidate transfer width. Each
  subcase receives fresh RDRAM/RSP/native state, exactly one FullSync, guarded
  synthetic inputs, and a subsequent present whose workload identity must
  equal the task's final counter. Candidate, knockout, and adaptive refinement
  vectors set `G_LIGHTING | G_POINT_LIGHTING`; the directional control sets
  only `G_LIGHTING`, and the lighting-disabled control sets neither bit.
  Admission fixes the suite; private manifests cannot select a variant,
  commands, cases, or expected results. The runner obtains its two raw windows
  only from the boot harness's typed in-process Rust loader. That loader
  revalidates current v7 scope, exact-matches the supplied readiness bytes to
  its canonical derivation, and returns the bytes captured while each window
  was read and hashed through one stable no-follow descriptor or Windows
  handle. Python remains a producer and differential oracle, never loader
  authority. This remains characterization transport only, currently
  entry-only and not yet exercised with an admitted private point-light pair;
  it neither opens production HLE nor supplies the missing wire/arithmetic
  evidence.
  Internally, the transactional inspector and reference decoder now share the
  pinned BranchW control-flow rule: opcode `0x04` selects bits 1..7, validates
  a loaded finite transformed W, compares it strictly with `float(u32 w1)`,
  and on a taken/forced path resolves persistent HALF_1 before applying the
  24-bit eight-byte command mask. Equality falls through and F3DEX2 retains
  its distinct BranchZ packing/comparison. This mechanism is testable before
  admission. The variant profile now governs the bounded NoN slice; point-light
  wire activation, layout, arithmetic, and rounding remain unimplemented and
  must be characterized without treating the capability flag as behavior.
  Native RT64 task submission returns a schema-checked result containing the
  plan identity, planned/observed generation counts, typed disposition and
  rejected generation, entry GBI availability, pre/post workload IDs, and
  initial/final microcode addresses. A complete result must exhaust the exact
  ordered plan. The adapter takes the native context out of its reusable slot
  and snapshots the complete physical RDRAM plus persistent RSP memory before
  crossing FFI. Only a schema/plan/count-validated completion commits that
  guest-memory transaction and returns the context. A valid preflight
  `NeedsLle` returns the context only after byte-for-byte proof that neither
  guest-memory resource changed. Every other native failure restores both
  resources, destroys the unrollbackable context, and clears its active
  release identity. Raw RDP execution applies the same rule to RDRAM. RT64's
  synchronous queue joins and call-scoped alias restoration are what make the
  rollback occur after the last possible foreign access, never concurrently
  with one.
  Pinned RT64 advances the workload ID only from `State::fullSync`, so the
  delta is typed native completion evidence and must agree with transactional
  public-command inspection. The address pair is diagnostic, not admission
  authority. A focused backend-neutral walker in `fn64-render` now owns the
  ordered entry/self-load plan, activation-time raw recognition windows, and
  exact FullSync count over immutable inputs. RT64 production task submission
  consumes that result directly and a structural test forbids calls into the
  reference decoder or its `RenderOp` stream. The reference renderer can
  therefore be extracted without leaving geometry-decode policy in the native
  adapter.
  The reference rasterizer owns one deterministic, explicitly seedable
  per-fragment noise stream. Every covered one/two-cycle fragment consumes one
  typed eight-bit sample before combiner/alpha/depth rejection; combiner
  NOISE and `G_AC_DITHER` use the byte, while RGB and alpha Noise use its low
  three bits. This implements the public Programming Manual's common random
  per-pixel routing and frame-varying behavior without substituting an ordered
  screen mask. SplitMix64 is a reproducible host policy for reference digests,
  not a claim about the manual's unpublished silicon generator, seed, or exact
  cycle advancement.
  The native RT64 path has a narrower exact-source overlay at the pinned MIT
  revision. It creates one typed fragment-noise sample from one
  `nextRandUint` result: combiner NOISE and `G_AC_DITHER` consume the same
  low-24-bit unit float, while `G_AD_NOISE` consumes the low three bits. The
  overlay applies the public `G_AD_PATTERN`, `G_AD_NOTPATTERN`, and
  `G_AD_NOISE` selectors only to combiner alpha after alpha compare/coverage
  rejection and immediately before blending; `G_AC_DITHER` therefore remains
  a separate earlier decision over the unmodified alpha even though its Noise
  selector shares the sample. The
  existing clean pinned-Metal synthetic raw-DPC fixture binds exact 16x16
  RGBA16 Pattern, InversePattern, Noise, and Disabled output digests, exact
  ordered 4x4 tiles, live Noise, and same-context reproduction for the
  deterministic selectors. A paired live combiner-NOISE/`G_AC_DITHER` phase
  accepts exactly 146 pixels and every survivor is grayscale at or below the
  primitive half-alpha cutoff, binding the shared route on pinned Metal. The
  complete twelve-phase, seven-repeat transcript was identical in 10/10 fresh
  native processes; its ordinary G_AC, shared control, and shared G_AC digests
  are respectively `1493e7af74f80caff7a0c645b0f522ec347ce38a198237ab3cbd802394e0c793`,
  `0268d9c2410c25067f144983829a5a091525f357e2981fc53f25e3d2c054da7f`,
  and `70289db3267cb703e806ee9ba86635ec651aab0ec56f434db1cf7988cbb34251`.
  The overlay does not alter shade, fog, or coverage alpha and does not repair
  RT64's framebuffer-wide/deferred RGB-dither selection. The reference lane
  instead exposes one full eight-bit sample to `G_AC_DITHER`; sharing topology
  does not establish identical threshold quantization or native/reference
  random-stream parity. No claim follows for the hardware random generator,
  seed or advancement, ordered matrices or ties, internal fixed-point
  precision, non-Metal APIs, MSAA, or representative full-ROM reach.
  VI is scheduled in this fabric rather than asserted after an executor ticker
  fires. Its 14-word raw register image is shared with typed MMIO;
  `VI_CURRENT` is derived from the programmed `VI_V_SYNC`: progressive output
  exposes the public even half-line sequence,
  while `VI_STATUS.SERRATE` alternates even/odd fields and the sampled low bit.
  Equality with `VI_INTR` raises the common MI source, and any `VI_CURRENT`
  write acknowledges it without replacing the sampled line.
  `osViSetMode` decodes the public `OSViMode` structure into that same image,
  retaining both five-word field register sets. Each interrupt selects the
  set matching live field parity, and its origin is added to the queued
  framebuffer base rather than misread as an absolute address.
  `osViSetSpecialFeatures` consumes the public `u32` ON/OFF command pairs—not
  a pointer—and composes gamma, gamma-dither, divot, and bit-16 dither-filter
  changes with the queued control image before the same interrupt latch.
  `osViSetXScale` and `osViSetYScale` validate their public ranges and multiply
  the mode's low 12-bit 2.10 coefficient while retaining its subpixel offset;
  a later mode call resets earlier overrides, while later scale calls compose
  into the queued mode.
  `osViGetCurrentLine`, `osViGetCurrentField`, `osViGetStatus`, and
  `osViGetCurrentMode` query that live state; a queued mode does not become
  current until the interrupt latch.
  Device advancement stops at each due deadline rather than collecting
  multiple field notifications at the final requested cycle. At each VI
  interrupt, pending mode/scales/blanking/framebuffer state becomes
  current before the general OS_EVENT_VI target or `osViSetEvent` target can
  wake. The general event fires every field; the VI-manager target honors its
  public nonzero `retraceCount` divisor independently. Framebuffer,
  black/unblack, and special-feature transitions become visible only after
  that latch. Every field triggers the renderer, including unchanged
  progressive register images: field cadence and the retrace-cycle noise seed
  are scanout inputs in their own right. One `ViScanoutRegisters` value
  snapshots all fourteen live words after field selection; it crosses the
  renderer boundary atomically with `ViPresentation`, so origin, source width,
  timing, active H/V window, X/Y scale, STATUS filters, event cycle, and sampled
  field cannot drift independently even when one checkpoint jump spans multiple
  fields. A jointly
  zero H/V window stays an inactive live register image rather than selecting
  backend compatibility geometry. The Rust
  reference backend keeps its RDP image separate from VI scanout, presents
  black without erasing that image, implements the public `osViFade` 10-bit
  interpolation of its first two rows, implements `osViRepeatLine`, and
  restores the unmodified source when each effect is disabled. It also applies
  the public square-root gamma transfer, the three-horizontal-sample median
  divot correction at partial-coverage silhouettes, and RGBA16 dither
  restoration's signed comparison against the available 3x3 neighbors. The
  exact implemented arithmetic and the boundary between public mechanism,
  deterministic host policy, bounded hardware-unverified coverage
  AA/resampling, and post-DAC analog behavior are recorded in `VI-FILTERS.md`.
  Its post-VI allocation uses the public H start/end pixel extent and V
  start/end half-line extent independently of the RDP source dimensions; the
  same coordinate generators implement filtered modes and mode-3 replication.
  Presentation receives a move-only, retrace-scoped physical-RDRAM read
  capability together with that register image. Integrated execution creates
  the capability from the one registered process allocation only while the
  guest coroutine is suspended, without manufacturing a Rust slice that would
  alias the typed recompiler's dormant mutable view. The deterministic
  reference path rereads the exact live 24-bit origin and effective 12-bit
  stride on every field, decodes RGBA16 or RGBA32 in the generated-code storage
  layout, and never substitutes its resident RDP framebuffer. Its checked fetch
  envelope includes the vertical resampling sample and the largest active
  restoration/coverage-AA row halo; an out-of-bounds footprint or an odd
  RGBA16 origin is a named error. Inactive and blank images do not fetch source
  bytes. Source decoding, hidden-coverage inference, the full VI pipeline, and
  presentation state commit transactionally, leaving both the previous
  presented image and the resident RDP framebuffer unchanged on failure.
  RT64 receives the same current physical allocation and live VI origin/effective
  stride for each presentation. Its Rust boundary consumes `fn64-render`'s
  typed programmed footprint and validates only the rows selected by public
  coordinate arithmetic; the reference-only filter halo is not presented as
  evidence about RT64's internal or silicon bus fetches.
  Those mechanisms follow the
  public VI manual and the clean-room hardware descriptions in
  [US 6,166,748](https://patents.google.com/patent/US6166748A/en) and
  [US 5,699,079](https://patents.google.com/patent/US5699079A/en).
  Gamma dither stochastically rounds the final video value to the documented
  seven bits using a coordinate/channel hash keyed by the exact retrace guest
  cycle. The patent specifies fresh random low-bit noise but does not publish
  its generator or seed, so this is an explicit deterministic emulation policy,
  not a claim that fn64 reproduces the silicon's random stream. These two
  public functions are beyond the canonical NMR inventory but are exported
  for general N64 software in both C and Rust-recompiler host-call lanes.
  Enabling black with an effective Y scale other than the manual-required 1.0
  traps loudly, as do blacking while fade/repeat is active and enabling fade
  and repeat together. The RT64 adapter sends the complete register image
  through its quarantined C boundary and preserves the guest's origin so
  pinned RT64 applies its own one-row/odd-serrated-two-row framebuffer lookup
  normalization exactly once. Precompensating that value in fn64 cancels the
  native lookup and forces RT64's 1x scratch-upload path. The adapter retains
  the complete image when later HLE/raw submissions or resizes refresh address
  aliases. Backend-only compatibility callers have no live VI origin and name
  the color-image base, so only that explicit state synthesizes RT64's inverse
  lookup bias.
  A scoped foreign binding installs the call's RDRAM pointer in RT64 Core and
  State, waits both workload and presentation queues idle—including exception
  exits—and restores placeholder aliases before the Rust capability ends.
  Standalone backend-geometry compatibility remains available for behavior
  fixtures, but the backend records that authority and refuses to emit a
  fixed-cycle release capture until a complete live-register presentation has
  succeeded. A successful capture also binds the exact active digital output
  height from that same `ViPresentation` to its complete renderer-owned pixel
  storage. The active height is validated as nonzero and no greater than the
  stored height; native filter-extension rows remain in release evidence even
  when an interactive presenter excludes them from the visible prefix.
  Black still disables pixel type, repeat-line uses zero Y scale, and fade uses
  zero Y scale plus the 10-bit Y subpixel offset without discarding the retained
  image. The no-device adapter capture proves the first and post-submission
  24-word RT64 images are identical. A live pinned-Metal gate now observes
  twenty complete register phases over one workload at nondefault 8x6 active
  geometry: off-state restorations are byte-identical, gamma and 1.5x X/Y
  scale causally change exact pixels, and every present identity advances.
  Gamma dither, coverage-gated horizontal divot, and full-coverage RGBA16
  dither restoration are causal and restorable in the native VI shader. The
  divot gate proves that three full-coverage control rows stay unchanged while
  exactly twelve eligible pixels in the otherwise identical non-full rows
  change to the exact componentwise median over RT64's modulo-eight
  framebuffer-alpha coverage estimate. The restoration gate applies the
  shared signed available-neighbor 3x3 formula: exactly eighteen eligible
  full-coverage pixels change, all twenty-four non-full pixels and six flat
  full-coverage controls stay byte-identical, and alpha is preserved. This
  restoration claim is limited to clean pinned Metal with nearest host
  filtering, native scale, progressive scanout, and the synthetic RGBA16
  fixture. Managed-target per-pixel dither history and complete coverage,
  linear and anti-aliased-pixel-scaling filtering, enhancement resolution,
  MSAA/downsample behavior, D3D12, Vulkan, and representative full-ROM
  presentation remain uncertified. A separate eleven-phase pinned-Metal
  fixture distinguishes supplied hardware mode 0 from compatibility-only
  `Unspecified` at the native callback; a separate adapter-capture integration
  test proves the Rust/C/C++ wire distinction. The fixture applies the public
  Figure-11 AA arithmetic to deliberately generated RT64-managed codes 1-6
  with opaque code-7 controls, and proves modes 0/1 equal an independent
  per-code CPU oracle while modes 2/3 restore the baseline. Its divot oracle
  reconstructs the declared RDP source before projection; AA, divot, and
  AA-before-divot each change the six target pixels causally. This result is
  bounded to pinned Metal's nearest, progressive, synthetic RGBA16 path under
  the original-aspect (4:3) presentation policy. Pinned RT64 aliases managed
  7/8 and clamped 8/8 at code 7; natural/imported hidden coverage, code-0/save
  semantics, insufficient neighborhoods, wider sampling lattices, silicon,
  and analog parity remain explicitly bounded. A typed IPL television
  standard is the common VI/AI clock authority. Before a mode exists, VI
  retains the public television-standard clock but does not manufacture edges
  while the mode registers remain at reset. `VI_INTR` resets to the public
  0x3ff default;
  once H_SYNC and V_SYNC are nonzero, their public terminal-counted
  line/half-line units derive the next
  guest-cycle field interval from that standard's video clock after expanding
  each stored total by one. Hosts query the live interval at
  every injection point, so a latched mode changes the next deadline. The live
  shell converts that exact guest-cycle interval to its next `WaitUntil`
  deadline; it does not replace the programmed cadence with a nominal 60 Hz
  constant. VI animation and the DAC therefore retain the same television-clock
  authority even when a title programs a non-nominal NTSC mode or selects PAL.
  This formula is clean-room derived from public register definitions and the
  N64 Timing Reference section 5.1.1's U.S. Patent 6,331,856 sheets 46--47
  register diagrams; it has not yet been checked against a hardware timing
  trace. Exact VI random-stream
  identity, broader native coverage/filter-lattice certification, and
  physical-console filter capture remain open.
  Per VR4300 User's Manual chapters 3 and 6, the arbitrary-PC block lane's canonical 32-bit instruction-fetch boundary
  keeps the architectural virtual PC used by branch/J/JAL, EPC, and Cause.BD
  separate from the admitted `InstructionWordIdentity { BankId, physical
  address }`. KSEG0/KSEG1 select PA directly; KUSEG/KSSEG/KSEG3 use the
  recorded PageMask, ASID/global, and valid state. AOT translations contain
  exactly one straight word or one branch plus a separately translated slot,
  and the interpreter constructs the same unit from the physical catalog, so
  cross-page nonadjacent mappings and remaps cannot borrow virtual adjacency
  or stale bytes. Its typed unit source also admits the canonical
  `0xffff_fffc -> 0` slot wrap without relaxing the ordinary code catalog's
  non-wrapping span invariant. This is selected inside the ordinary `BlockProgram::run` and
  `dispatch` contract whenever a destination bank owns physical code; exact
  mapped AOT entries override that bank's mapped-interpreter fallback without
  creating another dispatcher. Canonical `BlockProgram` evidence binds all
  physical spans/words and every mapped entry's exact `BankId`/PA sequence,
  preflight-expected words, and generated artifact identity, independent of
  native pointers and registration order. Mapped-interpreter destination
  observations honestly retain no
  generated artifact and are operational/differential-only, not fixed-cycle
  release evidence under schema v29; artifact-identified mapped AOT retains its
  real artifact and is eligible, while compatibility AOT without one is not.
  Refill and invalid fetch faults retain exact EPC/BD, BadVAddr, Context/EntryHi,
  and refill/common vector selection. The legacy whole-function boundary,
  64-bit instruction-PC/catalog identity, and translated physical device
  routing remain loud. Data-side translation is wider: Status.KSU plus UX/SX/KX
  classify the documented XUSEG/XKSSEG/XKUSEG, XSSEG, XKPHYS, and XKSEG ranges;
  mapped spaces compare EntryHi.Region plus VA[39:13], while XKPHYS requires
  VA[58:32]=0 before using PA[31:0]. Width/privilege violations become typed
  AdEL/AdES. Extended refill entry preserves full BadVAddr and updates Context,
  XContext, and EntryHi before selecting the first-level XTLB vector; nested
  exceptions retain the common-vector rule.
  In the block
  lane, raw MI mask commands and RCP completion drive CPU IP2; the next
  instruction boundary applies the
  Status IE/IM/EXL/ERL gate, commits Cause/EPC/EXL, and resolves the BEV-selected
  handler through the active code mapping. The same boundary synchronizes the
  executor-owned half-rate Count/Compare clock: equality latches CPU IP7 and a
  handler's MTC0 Compare acknowledges it before ERET can resume. Generated-C
  translation units compile as C++ solely for `fn64_mmio_proxy.h`: its lvalue
  proxy maps zero- or sign-extended KSEG0/KSEG1 RDRAM aliases onto one
  low-29-bit physical prefix, preserves the `^2`/`^3` byte lanes, and routes
  canonical KSEG1 RCP plus KSEG0/KSEG1 PIF `MEM_W` accesses through the same
  raw handlers as the typed block lane. KUSEG, KSEG2/3, and noncanonical
  64-bit aliases never acquire implicit TLB behavior. The wrapper also
  replaces the vendor header's pre-expanded LD/SD and unaligned helpers so
  every width uses that boundary; non-word RCP/PIF operations and partial
  SWL/SWR selectors trap before a device read or write. Because N64Recomp's C
  permits `goto` to cross an initialized
  scalar declaration while C++ rejects it, the shared build boundary copies
  each generated translation unit into Cargo's `OUT_DIR`, supplies the uniform
  recompiled-function prototype for calls omitted from generated `funcs.h`, and
  splits only the exact `gpr jr_addend_<hex> = value;` shape into a declaration
  plus assignment at the same program point. The missing-prototype set is
  derived from each generated input rather than game names baked into fn64.
  The proxy's C++ `RECOMP_FUNC` keeps C linkage plus weak/noinline attributes
  but omits N64Recomp's C-specific `extern inline` spelling, whose different
  C++ semantics can suppress every externally linkable generated body.
  The out-of-tree source stays untouched and no derived game code enters git.
  Subword RCP access traps loudly. This closes
  split register authority, but not the function lane's inability to suspend
  inside one generated function; tight timed-device polling still requires
  block-lane checkpoints. Both boot lanes pass the allocation length with its
  pointer through the public `register_process_rdram` seam (also invoked by
  `boot_thread0`); the executor, timed DMA
  paths, and RSP HLE/LLE task runners therefore share one explicit bounds
  authority. Re-registering the identical pointer/length is idempotent;
  replacing a live allocation traps because retained device/task authority may
  still name the original bytes. Raw-MMIO interception ends at the public RCP/SI boundary
  `0xA4900000`; cartridge-domain KSEG1 addresses at `0xA5000000` and above
  remain ordinary generated-code backing rather than being misdecoded as
  registers. The C proxy and typed Rust lane share one classifier: physical
  RDRAM aliases use the common 8 MiB prefix, while other canonical KSEG0/KSEG1
  addresses use N64Recomp's sparse `low32(address) - 0x80000000` offset and
  succeed only when the host supplied that complete range. This compatibility
  backing is not evidence that a cartridge-domain device is attached; in
  particular, completing the cart-only `osDriveRomInit` probe does not claim
  mounted 64DD IPL-ROM storage or DMA support.
- **Why rung-18-class races become unrepresentable, precisely.** Rung 18's
  actual root cause was not "the mutex was in the wrong place" — a mutex
  *was* added at exactly the TOCTOU the original hypothesis named, verified
  present in the compiled binary, and the crash reproduced unchanged 20/20.
  The real cause was a second genuinely-concurrent host thread executing
  recompiled MIPS code that touches shared rdram through no queue API at
  all — a category of write no scheduler-level lock can intercept, because
  it doesn't go through the scheduler. A stackful-coroutine, single-executor
  model removes the precondition entirely: there is no second host thread
  ever executing recompiled code, so there is no "genuinely concurrent write
  to shared rdram bytes with no lock the scheduler can see" to have in the
  first place. The invariant "exactly one game thread runs at a time" is not
  maintained by discipline at N call sites (as in (a)) — it is a physical
  fact about how many native call stacks exist, enforced by the executor
  loop itself, at exactly one place in the codebase.

### `OSMesgQueue` semantics, designed from the libultra manual + rung evidence

Modeled as (all in `fn64-runtime`, no `unsafe`, no direct field access from
`fn64-abi`):

```rust
/// Owns the invariant osCreateMesgQueue is documented (libultra manual,
/// "Message Manager") and rung 12 proved load-bearing: a freshly-created
/// queue's blocked lists are EMPTY, full stop -- never a stale/sentinel
/// value, never partially constructed. The only way to get a MesgQueue is
/// through this constructor; there is no path that produces one with a
/// non-empty blocked list, matching the ROM's own real osCreateMesgQueue
/// semantics (zero both fields) and closing off the rung-12 failure mode
/// (a caller writing raw struct bytes and leaving a sentinel/garbage
/// pointer where the runtime's "is anything blocked" check expects None)
/// by construction: there is no raw-write path in this API at all.
pub struct MesgQueue {
    buffer: Box<[Mesg]>,      // count-capacity ring buffer (osCreateMesgQueue's `msg`/`count` args)
    valid_count: usize,       // validCount: how many slots currently hold a real message
    first: usize,             // ring index of the oldest valid message
    blocked_on_recv: BlockedList,  // OSThreads parked in osRecvMesg on an empty queue
    blocked_on_send: BlockedSenderList, // OSThreads + message + head/tail operation
}
```

- **Blocked-list ownership.** `BlockedList` is not a raw pointer/sentinel
  (the exact shape rung 12 found corrupting the run queue) — it is an
  `Option<CoroutineId>` chain owned exclusively by the executor's scheduler
  module, never touched by `fn64-abi` shim code directly. A shim
  (`osRecvMesg_recomp`, `osSendMesg_recomp`) calls a `fn64-runtime` method
  (`MesgQueue::try_recv`/`try_send` returning `Blocked` or `Delivered`); only
  the executor's yield/resume machinery ever mutates which coroutine is on
  a `BlockedList`. This means the field can never observe the rung-12 state
  (a queue whose blocked list "contains" a foreign, non-thread ROM address)
  because nothing outside this module's constructor and the executor's
  single mutation path can write it at all — there is no `unsafe`, no raw
  pointer cast, and no second writer to race.
- **What `osCreateMesgQueue` resets (rung 12).** `MesgQueue::new(buffer,
  count)` is the only constructor; it always produces `valid_count: 0,
  first: 0, blocked_on_recv: None, blocked_on_send: None`. There is
  structurally no way to observe a freshly-created queue with a non-empty
  blocked list, which is exactly the invariant rung 12 found the real ROM's
  `osCreateMesgQueue` establishes and found catastrophic when skipped
  (a queue whose fields still held whatever raw bytes were there before,
  interpreted by the empty-check as "something is blocked").
- **Send/recv as coroutine yield points, not thread ops.** `try_send`/
  `try_recv` return an enum (`Delivered(Mesg)` or `WouldBlock`); the
  `fn64-abi` shim, on `WouldBlock`, registers the current coroutine on the
  appropriate `BlockedList` and yields to the executor — this is
  `osSendMesg`'s blocking path, the exact one rung 18b root-caused as the
  actual (and previously un-suspected) source of the concurrent write. In
  this design that "concurrent write" cannot happen: registering on
  `BlockedList` and yielding are two steps of one sequential function running
  on the single executor thread, with no other coroutine able to observe or
  mutate the queue in between (nothing else is running).
- **Blocked operation identity and lifecycle.** A blocked sender retains a
  typed head/tail placement with its thread, block-time priority, and message;
  blocked receivers likewise retain block-time priority. Waiters wake in
  descending priority with FIFO ties, so a delayed
  `osJamMesg` commit cannot become an ordinary tail `osSendMesg` when another
  thread frees space. `osStopThread` and `osDestroyThread` sweep every queue's
  sender and receiver roles before changing thread state. Thus the later
  receive/event interleaving cannot rediscover a stale waiter and revive a
  stopped or destroyed coroutine.
- **Event queue registration (`osSetEventMesg`, VI/SI/PI sources).**
  Modeled as a small `EventTable: HashMap<OsEvent, (QueueHandle, Mesg)>` in
  `fn64-runtime`, populated by `osSetEventMesg_recomp`. VI/timer/SI/PI
  completion (host-driven, §2's yield-sites discussion) posts through this
  table by calling the *same* `MesgQueue` API a blocking guest `osSendMesg`
  would use — one code path, one invariant, whether the sender is "guest
  code" or "the host VI driver," closing the asymmetry that made rung 18b's
  external-vs-game-code distinction a source of confusion in the reference
  runtime (its `dequeue_external_messages` was a structurally separate path
  from `do_send`, per the profile.toml writeup, and telling which one was
  responsible for a given mutation was part of what made that rung hard).

### Implementation notes (wave 2/3, 2026-07-14): what building it taught us

This design's recommendation (option (b), `corosensei`) is implemented as
specified — no deviation from the chosen crate or the core "one host
thread, stackful coroutines, priority-ordered run queue" shape. Three
things the implementation surfaced that this doc didn't originally spell
out, recorded here honestly per `AGENTS.md`'s "mark revisions honestly":

- **Process exit is an explicit terminal executor state, not coroutine
  unwinding.** A bounded host can legitimately stop while guest threads are
  suspended inside generated C and an `extern "C"` blocking shim. The public
  `corosensei::Coroutine` contract force-unwinds a suspended stack from
  `Drop`; Rust cannot unwind that payload through those non-unwind FFI frames.
  `fn64_abi::prepare_process_exit` therefore validates that no guest owns the
  run token, abandons any committed HLE renderer-continuation token without
  resuming it, drops renderer/audio backends while registered RDRAM is live,
  normally drops never-started and completed coroutines, and intentionally
  forgets only started/unfinished coroutine objects for the kernel to reclaim
  at process termination. It then clears every saved yielder/RDRAM pointer
  and changes the TLS owner from `Active(Executor)` to
  `PreparedForProcessExit`; subsequent executor access traps. This is neither
  guest `osDestroyThread` nor an in-process reset facility. The shell invokes
  that seal from winit's irreversible `ApplicationHandler::exiting` boundary,
  not only after `run_app` returns: on macOS `applicationWillTerminate:` can
  begin Apple TLS destruction before control reaches the statement after
  `run_app`, and an executor still live there force-unwinds guest stacks across
  their non-unwind FFI frames. The bounded-census `process::exit` path seals
  directly because it does not dispatch winit's exit callback. The child-process
  ABI regressions block inside the real `osRecvMesg_recomp` and stop after a
  resumable backend returns `Continue`; each seals the runtime, returns
  through ordinary Rust teardown, and requires exit status zero. Provenance:
  the public corosensei `Coroutine::drop`/`force_unwind`
  contract and Rust's non-unwind `extern "C"` ABI rule; no reference-runtime
  implementation behavior is used.

- **`Yield`/`Resume` needed a `may_block` field, not just two "will
  definitely block" variants.** The original sketch modeled
  `BlockOnRecv`/`BlockOnSend` as always-blocking suspend points, with the
  `fn64-abi` shim expected to pre-check via an `Executor` method (e.g.
  `send_mesg`/`recv_mesg`) whether blocking was actually needed before
  deciding to yield. That pre-check is exactly what caused the bug below,
  so the real shape unifies `OS_MESG_BLOCK`/`OS_MESG_NOBLOCK` into ONE
  suspend point per operation: `Yield::BlockOnRecv { mq_addr, may_block }`/
  `Yield::BlockOnSend { mq_addr, msg, may_block, jam }`. The executor's
  `handle_yield` (the only place that safely holds `&mut Executor` at this
  point) does the check-then-deliver-or-block-or-drop logic uniformly; a
  new `Resume::WouldBlock` variant carries the `OS_MESG_NOBLOCK`-on-
  unready-queue outcome back to a coroutine that yielded with
  `may_block: false`, which never gets parked on any blocked list. This is
  a strictly more precise version of the same design intent (§2's
  "Send/recv as coroutine yield points, not thread ops"), not a course
  reversal.
- **First resume and blocked-send intent are explicit state.** `osStartThread`
  installs `Resume::Start` only until `GameThread` records its first resume;
  a previously resumed coroutine can never receive `Start` again. A blocked
  send stores `SendPlacement::Head` or `Tail`, and
  queue-owned waiter removal clears both sender and receiver roles for thread
  stop/destruction. These types preserve the operation across every scheduler
  interleaving rather than asking the eventual wake site to reconstruct it.
  The host shim also yields after making the target runnable. This closes the
  caller-starts-higher-priority-target interleaving: the executor's ordered run
  queue chooses the target before the caller can retire its next guest
  instruction; a non-outranking target simply causes the caller to be chosen
  again.
- **A real reentrancy bug, caught by this crate's own tests, in exactly the
  shape the pre-check above created.** `fn64-abi`'s coroutine bodies run
  physically nested inside `Executor::run_one_step`'s call to
  `GameThread::resume` — which itself runs inside whatever outer call
  (`run_one_step`/`run_to_idle`) invoked it. A coroutine body that called
  back into a `RefCell<Executor>`-guarded accessor (to pre-check "would
  this send block?") hit a live "RefCell already borrowed" panic on the
  very first such call, not a theoretical race: the outer borrow was still
  open on the same call stack. The fix (previous bullet, plus `fn64-abi`
  never touching its `EXECUTOR` thread-local from inside a coroutine body
  at all — even "which thread am I" is answered from a second thread-local
  populated alongside the active `Yielder`, never by asking the executor)
  is now load-bearing, commented at the fix site in both crates. This is
  the same *category* of bug rung 18 was — a hidden caller reaching state
  through an API that looked like a safe accessor — just caught by a type
  (`RefCell`'s dynamic borrow check) instead of a debugger, and inside this
  project's own new code rather than the reference runtime's.
- **`osCreateThread`'s real entry-point dispatch is a separate, larger
  piece of work than "wire the thread-lifecycle shim."** Calling the
  actual recompiled function a new `OSThread` should run requires the
  overlay/`get_function` lookup table (§1's `FuncEntry`/`SectionTableEntry`,
  wave 3's last listed item) which doesn't exist yet — `osCreateThread_recomp`/
  `osStartThread_recomp` are implemented as loud, named `unimplemented!()`s
  for exactly that missing piece (per `AGENTS.md`), not silently-succeeding
  stubs. Every other piece of thread/queue/timer machinery those two shims
  would eventually drive (`Executor::create_thread`/`start_thread`/
  `set_thread_pri`, the whole blocking send/recv/wake path) is implemented
  and tested for real, exercised end-to-end by this crate's own test
  harness standing in for the not-yet-written trampoline (see
  `fn64-abi/src/lib.rs`'s `tests::spawn_test_thread`).

### `Executor`/`Peripherals` module split (structure wave, 2026-07-14)

`fn64-runtime::executor::Executor` had grown into holding both its actual
job (run queue, `MesgQueue` registrations, timers, the `event_table`, and
the single `inject_event` door — the scheduling state §2's threading model
is about) AND host-side hardware-model state for three peripherals that
have nothing to do with the single-runnable-coroutine invariant: VI
(mode/y-scale/framebuffer-swap/retrace-ticker), SI/PIF (controller-probe
response shape), and RSP (task-header capture/counting). Every VI/SI/RSP
method lived directly in `impl Executor`, touching private `Executor`
fields (`vi`, `retrace`, `pif`, `tasks`) — a reviewer auditing "does this
change threaten the single-runnable-thread guarantee" had to read past
`osViSetMode`/`PifModel::query_response`-adjacent code to find the actual
scheduling logic, and vice versa.

**The fix**: a new `fn64_runtime::peripherals::Peripherals` struct now owns
those four fields and every method that only touches them
(`vi()`/`vi_set_*`/`vi_swap_buffer`/`arm_retrace`/`advance_retrace`,
`pif()`, `task_log()`/`submit_task`). Hardware RSP memory/register/DMA state
now lives separately in `DeviceFabric`; `Peripherals` retains only the OS-facing
task log. `Executor` holds exactly one
`peripherals: Peripherals` field and re-exposes the same public method
names as one-line delegations, so **no caller outside this crate changed**
— `fn64-abi`'s `with_executor(|exec| exec.vi_set_mode(...))`-shaped call
sites are byte-identical before and after this split; only where the
implementation lives moved.

Two things deliberately did NOT move to `Peripherals`, on purpose, not by
oversight:

- **`event_table`** (the `osSetEventMesg`-populated `OS_EVENT_*` →
  `(queue, msg)` table) stays on `Executor`. It is genuinely shared
  scheduling machinery — a guest `osSetEventMesg` registration and the VI
  retrace ticker's `OS_EVENT_VI` lookup both go through it, and
  `inject_event`'s `ExternalEvent::OsEvent` arm has no notion of which
  peripheral "owns" a given event code. Moving it into `Peripherals` would
  just relocate the god-object problem one file over instead of resolving
  it.
- **Trace recording** (`TraceLog`/`sim_time`) also stays on `Executor`.
  `Peripherals::vi_swap_buffer`/`submit_task` return the plain data
  (framebuffer address; task kind) the old single-body versions used to
  feed straight into `self.trace.record(...)` — `Executor`'s thin wrappers
  do that recording themselves, since `sim_time` is the executor's virtual
  clock, not a peripheral's own state.

This was a pure structural move: every `Peripherals` method's body is
character-for-character what used to be the matching `Executor` method's
body (see `peripherals.rs`'s module doc for the full mapping); no behavior,
field default, or trace-event shape changed. The existing test suite
(`fn64-runtime`'s unit tests, `rung_regressions.rs`, `fn64-abi`'s unit
tests) passes unchanged in both count and behavior — this is the gate a
pure-refactor claim like this one has to clear, not merely "it compiles."

### `ReentrantCell` audit verdict (structure wave, 2026-07-14)

The wave 2/3 implementation notes above record a real reentrancy bug fixed
by replacing `fn64-abi`'s `EXECUTOR: RefCell<Executor>` with
`EXECUTOR: ReentrantCell<Executor>`. This wave's task: is that cell still
earning its keep now that `Yield`/`Resume` (§2, `thread.rs`) already make
one whole class of reentrancy a compile-time non-issue, or was it only ever
papering over something the type system should be asked to catch instead?

**Verdict: still needed, and it guards a genuinely different hazard than
the one `Yield`/`Resume` closes — not a residual instance of the same one.**

- **What `Yield`/`Resume` + `RunToken` already prove, at compile time**: no
  second `GameThread::resume` can ever be invoked while a first is on the
  stack. `RunToken` is non-`Copy`, privately constructed, and
  `Executor::run_one_step` is the only place that both issues one and calls
  `resume` with it (`thread.rs`'s `RunToken` doc comment) — this is a
  *scheduling* reentrancy guarantee about resumes specifically.
- **What `ReentrantCell` guards, which is not a resume at all**: a
  coroutine body, once resumed and running as ordinary synchronous Rust
  code (no suspend, no yield), is free to call any `_recomp` shim as a
  plain nested function call — and several real, common shims
  (`osCreateThread_recomp`, `osSetEventMesg_recomp`, every VI setter,
  `osSetTimer_recomp`, etc.) themselves call `with_executor`. Since the
  OUTER `with_executor` call (`fn64-abi`'s own `run_one_step`/`run_to_idle`
  helpers, which wrap `Executor::run_one_step`/`run_to_idle`) is still
  nominally on the stack when this happens, the inner call is a **second,
  nested `with_executor` invocation while the first is still open** — not
  two threads, not two resumes, just an ordinary call stack `Yield`/`Resume`
  have no vocabulary for, because there is no suspend point here for either
  type to govern. `fn64-abi/src/lib.rs`'s
  `a_running_threads_own_body_can_call_os_create_thread_recomp_without_reentrancy_panic`
  test is the regression test for exactly this shape, reproducing what
  `recomps/wm2000/packages/wm2000-boot`'s boot harness hit for real on its very first
  `osCreateThread` call.
- **Why this is memory-safe despite looking like `&mut` aliasing**: the
  outer `with_executor` closure does not read or write `Executor` state
  again until the inner, nested call returns — the two "live" `&mut`
  references are simultaneously in scope on the call stack but never
  simultaneously dereferenced. A plain `RefCell` cannot express that
  distinction (its borrow tracking is purely dynamic/stack-blind: a second
  `borrow_mut()` panics the instant it happens, regardless of whether the
  first borrow is actually being touched concurrently) — which is exactly
  the "already borrowed" panic that surfaced this bug for real.
- **Why this can't be pushed into the type system the way `Yield`/`Resume`
  were**: doing so would require making "a coroutine body calls another
  shim" itself a suspend point — i.e. a stackless/async redesign where
  every shim call is an awaited yield the executor's loop mediates.
  §2 already evaluated and rejected async for this exact workload
  (recompiled C's call graph has no natural `.await` points; forcing one
  in would mean hand-rolling the same suspend machinery on a worse
  primitive, or collapsing back to option (a)'s per-OS-thread hazards).
  Short of that redesign, this residual case is a property of ordinary
  synchronous Rust call stacks, not something a coroutine-yield type can
  see.
- **What this wave DID do, per the task's option (a)**: confirmed
  `with_executor` (`fn64-abi/src/lib.rs`) is already, structurally, the ONE
  gateway — `EXECUTOR` is a private `thread_local` with no other accessor
  anywhere in the crate, so every one of the ~30 `Executor`-touching call
  sites (every `_recomp` shim, every host-facing helper, every test) already
  funnels through it; there was no second, looser path to close. What was
  missing was the audit itself living at that gateway: `with_executor`'s doc
  comment now states precisely which reentrancy shape the type system
  already closes, which dynamic shape survives, and why, so a future reader
  doesn't have to re-derive this from the bug history to trust the cell is
  still doing real work and not just historical caution left in place.

`ReentrantCell` is not removed. It is not a second, redundant guard next to
`Yield`/`Resume` — it is the only mechanism that can cover this particular
shape at all, given the design this project already committed to (single
executor, stackful coroutines, synchronous shim calls). Removing it would
not be "relying on the type system instead" — it would just reintroduce the
exact panic `recomps/wm2000/packages/wm2000-boot` hit, with no compile-time replacement
available under this architecture.

## 3. Memory model

### rdram buffer ownership

Physical RDRAM is one stable 8 MiB allocation for the canonical typed-Rust
lane. A validated bootstrap/import transaction owns it before boot, admits
only typed ROM publications, binds the executable baseline to the exact ROM,
resolver, generation catalog, and watched-byte digest, then moves the sealed
allocation into `fn64-abi`'s `HostState` for the complete guest lifetime. The
initial publications become sequence zero of the executable-mutation journal;
there is no mutable slice or raw-pointer escape between validation and
installation. Bootstrap commit validates every initially resident,
unreserved direct-RDRAM static bank and every unreserved physical-code bank,
not only the entry bank. Generation shard banks are excluded from that
word-for-word pass because they are mutually exclusive alternatives; their
initial physical images instead pass a whole-catalog validator. Zero bytes are
an unloaded image; every nonzero image byte must be covered by at least one
complete exact catalog digest. The receipt binds the canonically ordered
matching generation IDs, and install revalidates them before taking ownership.
The ABI has a validator-owned move-only completion constructor for this exact
bootstrap evidence, and the boot harness now adds the selected-build exact-ten
outer prerequisite described above. The production row remains open until a
private series is actually run and the denominator consumes the intended
move-only outer authority; copied report data is never a substitute.

The generated-C compatibility lane still uses
`fn64-boot-harness::new_rdram(TvType)`, whose same stable allocation extends to
the legacy `0x2900_0000` sparse MMIO mirror because generated pointer macros
cannot be intercepted. That roughly 656 MiB compatibility extent is not N64
RDRAM and is not carried into the canonical block lane: typed-Rust loads and
stores route RCP MMIO through the registered hooks. `fn64-runtime::Rdram` owns
the corresponding layout in isolated core tests and runtime-only
configurations. Every consumer — `fn64-abi` shims, the executor, and render
task marshalling — borrows the one installed allocation; no consumer makes a
translated framebuffer/DMA copy and later treats it as RDRAM. Raw compatibility
boot rejects a canonical executable backing whose physical end exceeds the
supplied allocation before installing any live owner.

Local test runs use `scripts/guarded-cargo-test.zsh`; nextest runs use
`scripts/guarded-nextest.zsh`. Both combine single-job Cargo compilation with
serialized test execution, a sampled 4 GiB process-group ceiling, and a
sampled 40% system-free floor. `nextest -j1` serializes test processes but does
not serialize Cargo compilation, so the wrapper also fixes
`CARGO_BUILD_JOBS=1`. The guard owns one dedicated
macOS session until its exact PGID is empty, including surviving reparented
descendants, and signals only that group. These one-second observations are a
safety guard rather than an OS hard memory limit; transient overshoot remains
possible between samples. `cargo test -j1` alone does not serialize tests and
therefore is not the safe feedback loop for tests that own process RDRAM.

### The `MEM_*` accessor contract

`ABI-SURFACE.md` section (c) gives the exact, byte-cited semantics
(`refs/N64RecompSource` codegen, MIT, cited there) that any Rust-side
helper touching rdram from outside generated C (diagnostics, watch hooks,
save-state code) must reproduce exactly:

| Accessor | Width | Byte-lane XOR | Sign |
|---|---|---|---|
| `MEM_W` | i32 | none (word-aligned) | sign-extended |
| `MEM_H` | i16 | `offset ^ 2` | sign-extended |
| `MEM_B` | i8 | `offset ^ 3` | sign-extended |
| `MEM_HU` | u16 | `offset ^ 2` | zero-extended |
| `MEM_BU` | u8 | `offset ^ 3` | zero-extended |

The byte-lane XOR is real, load-bearing big-endian behavior (N64 MIPS is
big-endian; host RDRAM storage is native-endian by 32-bit word, so sub-word
access corrects the lane) — not a bug to "simplify away." The N64Recomp ABI
shape requires a little-endian host; `rdram.rs` rejects other targets at
compile time instead of pretending the native-endian dereferences plus XORs
are portable there.

`fn64-runtime` is the sole owner of the mapping:

- `RdramView` / `RdramViewMut` borrow a sized storage slice and accept only
  logical `RdramAddr`s. Host adapters, framebuffer conversion, diagnostics,
  and device bulk copies use these safe views.
- `RdramPtr` is the deliberately unsafe form for `_recomp` shims whose C ABI
  supplies a raw pointer but no length. It centralizes the same mapping while
  making the missing bounds proof explicit at construction/access.
- Owning `Rdram` methods delegate to the views; DMA, controller structs,
  audio PCM, both framebuffer capture paths, and the ReferenceBackend writer
  therefore exercise one implementation.

`scripts/lint-rdram-layout.py` sweeps production Rust for a hand-written
`^2`/`^3`, raw indexed RDRAM write, or raw-pointer RDRAM write outside
`rdram.rs`. Its self-test includes the former flat-big-endian framebuffer
writer, so the regression shape is mechanically rejected before a live boot.

### `RdramAddr` newtype

```rust
/// An N64 vram/kseg0 address as MIPS code computes it -- i.e. a 32-bit
/// value that may arrive already sign-extended to 64 bits in a `gpr`
/// (recomp_context's register fields are uint64_t, per ABI-SURFACE.md
/// section (b): "gpr is uint64_t; MIPS registers r0..r31 are all 64-bit
/// even though most recompiled ops operate via ADD32/SUB32/S32 32-bit-
/// truncating wrappers"). Constructing one performs the SAME translation
/// math the generated MEM_* macros perform (section (c): subtract the
/// full 64-bit sign-extended KSEG0 base 0xFFFFFFFF80000000, not the naive
/// 32-bit 0x80000000) so a value arriving as either a plain 32-bit vram
/// or its 64-bit sign-extended gpr form lands on the identical rdram-
/// relative byte offset -- this ambiguity is exactly what a hand-rolled
/// `addr - 0x80000000` at a second call site would get wrong for half of
/// its inputs.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct RdramAddr(u32); // stored as the resolved rdram-relative byte offset

impl RdramAddr {
    pub fn from_gpr(reg: u64) -> Self { /* replicates MEM_* base math, tested against
                                            both a plain-32-bit and sign-extended-64-bit
                                            input per ABI-SURFACE.md (c) */ }
}
```

Every layout-aware RDRAM API in `fn64-runtime` takes `RdramAddr`, never a bare
`u32`/`u64`; only allocation sizing and raw-storage construction operate on
host integers/slices. This is the "types before audits" rule from `AGENTS.md`
applied directly: an invariant (correct KSEG0 translation) that could be
silently gotten wrong at any of dozens of call sites is instead computed once,
in one constructor, and every other call site's type signature makes bypassing it
impossible.

### First-class watch/diagnostic hooks

Rung 18/18b is the direct design brief here: the reference runtime's
`WCW_WATCH_ADDR` env-var hook was shown to be **misleading** on the exact
question fn64 needs diagnostics to answer reliably — "who wrote this rdram
address" — for two independently-confirmed reasons in that writeup:

1. **Attribution via `dladdr`/return-address unslide is unreliable under
   compiler inlining/tail-merging.** The rung's own cross-check found a
   watch hit reported as belonging to `func_800E6178` (an unrelated
   trig/waveform routine) that was "very likely an artifact of clang
   tail-merging many near-identical slow-path stubs into a shared block" —
   i.e. the reported call site was a real address, just not a meaningful
   one for "which logical function did this." The rung's own conclusion:
   "do not trust WCW_WATCH_ADDR's fn= naming at face value... without
   cross-checking against a real hw watchpoint."
2. **A watch armed at process start can't distinguish reused-address
   history from the event actually being investigated** — the same rdram
   address the rung cared about had been written earlier in boot, for an
   unrelated purpose, by a totally different function; an always-on watch
   conflates both.

fn64's diagnostic model is designed to make both of these non-issues,
in `fn64-runtime` (not bolted on later, and not env-var-gated production
code with debug-only side doors — per `AGENTS.md`'s "no silent shrugs" and
this crate's testability goal):

- **A global monotonic sequence counter**, incremented on every rdram
  mutation that flows through `Rdram::write_*` (i.e. every write any
  `fn64-abi` shim or the executor itself performs — there is exactly one
  write path per §3.1, so there is exactly one place to increment). Every
  watch/log record carries this sequence number, which turns "is this
  address's write history from the window I care about, or stale reuse
  from earlier in boot" (problem 2 above) into a trivial range filter on
  the log, not a late-arm-the-watchpoint dance done by hand in lldb each
  time.
- **Reliable attribution by construction, not by unslide-and-guess.** Every
  write that goes through `Rdram::write_*` is called from a specific,
  already-known Rust call site — the `fn64-abi` shim function, or the
  specific executor/scheduler method, that invoked it. A watch hook records
  that call site directly (a `&'static str` function name baked in at the
  call site, or `#[track_caller]`'s `Location`) — this is categorically
  different from the reference runtime's approach of reconstructing "which
  function was this" from a raw return address via `dladdr` after the fact,
  which is exactly the step clang's tail-merging was shown to corrupt.
  There is no unslide-and-bisect step for fn64's hook to get wrong, because
  the caller identity was never lost in the first place.
- **Late-arming as a first-class query, not an lldb incantation.** The
  rung's eventual ground truth came from "a hardware watchpoint... armed
  right after the conditional breakpoint on `osCreateMesgQueue(mq_==...)`
  fires, i.e. genuinely late-armed." fn64 exposes this as an ordinary API —
  `Rdram::watch(addr, from_sequence: Option<u64>)` — so "start watching this
  address, but only care about writes after event N" is a query against the
  sequence-numbered log, not a hand-run debugger recipe that has to be
  redone from scratch for the next investigation.

The renderer consumes one narrower typed event from that same write boundary:
`NonRdpWrite16` carries the canonical physical halfword and the exact value
after a CPU store commits. N64Recomp-generated C reaches it through the
build-wide `MEM_H` lvalue proxy, while typed-Rust output reaches it through
`Rdram::store_h`; KSEG0 and KSEG1 stores update the same visible physical
bytes before that event, and neither path suppresses a same-value assignment.
Word and unaligned-word stores publish one aligned four-byte range; SD/SDL/SDR
publish one eight-byte range only after both native words are coherent. The host
multiplexes Rust executable-region invalidation and renderer notification
without entering the executor. That `WriteObserver` remains notification-only;
the block lane installs a distinct `GuestWriteBoundaryObserver` whose typed
result marks a proven post-commit overlap with the live executable-region map.
The checked arbitrary-PC lane also exposes a thread-local `ReadObserver` for
exact-invocation dependency certificates. It reports successful physical-RDRAM
backing reads after translation, conservatively widening merge loads to their
aligned four- or eight-byte backing range. It excludes MMIO, failed accesses,
instruction fetches, host snapshots, and whole-function generated runners; it
is evidence for a bounded arbitrary-PC execution, not a lane-wide read trace.
Runners consume the mark only after the current straight instruction or full
branch/delay pair, so renderer observation neither chooses nor delays the
execution boundary. Programming Manual 15.5.6 is the behavioral source for
the renderer side only: only the documented 16-bit visible-LSB replication is
modeled here; byte and word stores remain range notifications without inferred
hidden-bit effects. A backend must explicitly return whether it applied a
Rust-owned sidecar, so native RT64's separate ownership cannot become a silent
parity claim. Raw RCP and PIF registers remain word-only in both lanes:
subword, doubleword, and partial unaligned stores trap before any device side
effect.

## 4. A/B migration: link-time swap over identical `RecompiledFuncs`

### The core mechanism

Both the reference runtime and fn64 link against the byte-identical
`libRecompiledFuncs.a` that N64Recomp emits for a given game/profile — per
`README.md`: "Both runtimes link the *identical* recompiled code, so every
fn64 behavior gets A/B'd against reality before the swap." The swap is a
**link-time choice of which library provides the `_recomp`/`recomp.h`
extern surface** (`ABI-SURFACE.md` section (a)'s full inventory) that the
same, unmodified `RecompiledFuncs/*.c` object files call into — nothing
about the recompiled game code changes between the two configurations, only
which implementation of `osCreateThread_recomp`/`osRecvMesg_recomp`/etc. the
linker resolves those undefined symbols against. This is exactly the
`nm`-based "truly-external undefined symbol" completeness gate
`ABI-SURFACE.md` already runs per game/archive — the same gate doubles as
the A/B build's correctness precondition (if the symbol set fn64-abi
exports isn't a superset of what a given game's archive needs, the swap
fails to link, loudly, before ever running).

### Shared event-trace format

The machine-readable fixed-cycle digest and minimum-scenario closure format
built on this trace is specified in `docs/RELEASE-GATE.md`. Its current live
ledger proves typed observations and absence of a reached loud trap only for
the exercised scenario; it is not full-runtime semantic closure. The digest deliberately
excludes the process-global diagnostic sequence number from timing digests:
the event order is retained, while unrelated tests or earlier tracing in the
same process cannot perturb release evidence.

DMA closure does not use the executor trace's legacy unqualified `Dma`
variant. `DeviceFabric` already owns a second typed transition trace at the
actual device boundary; the ABI copies it without translation. Schema v12
domain-separates and hashes PI/SI/AI/raw-SP start plus commit/completion events
and synchronous `SpTaskAdmitted`, binds each path's serialized observation
count into the report SHA, and hashes the complete future-affecting state of
the modeled `DeviceFabric`: its internal memories, queues, event ordering,
timing policy, and cartridge-save/programming state. DeviceState v10 extends
that projection with every AI latch and DPC register plus the complete pending
DPC transaction/source, so equal visible BUSY bits cannot hide distinct
DACRATE or START/range futures. It also hashes the
executor-owned PIF identities/input/rumble and all four retained Controller
Pak, Transfer Pak, and VRU slots; their complete authoritative storage,
semantic metadata, mapper/RTC/timing state; high-level VI/retrace state; and
the ABI manager's pending PI/SI delivery and VI-latch metadata. DeviceState v9
added the owner-local executor control and complete modeled ABI HostState
projections described below. Retained report schema v22 and DeviceState v9
artifacts are historical only; they cannot satisfy current v30 verification.

Device transition retention remains enabled by default and is required for
that release evidence. Long exploratory runs may explicitly disable retention;
`DeviceFabric` then releases the retained vector but continues updating a
constant-space typed summary of total transitions and PI/SI/AI/SP/task/VI
counts. The summary is progress telemetry only: it cannot replace the ordered
trace in a release digest. The WM2000 block harness uses this mode only under
`FN64_BLOCK_PROGRESS_ONLY`, with `FN64_BLOCK_DEVICE_TRACE=1` as the explicit
opt-in when full diagnostic history is needed.
Committed RSP/RDP observations follow a separate typed retention policy because
they are release evidence rather than device-trace diagnostics. Every ROM load
starts in `CompleteEvidence`, retaining the exact ordered payloads required by
the release gate. An interactive host may then select
`InteractiveConstantSpace`; the ABI clears retained payloads, continues a
monotonic total count, and loudly rejects any later full-history request from
that ROM lifetime. Loading a new ROM restores complete retention. Thus an
interactive session cannot grow this observation vector without bound, while a
certification consumer cannot accidentally digest a truncated history.
DeviceState v11 binds the audio-task execution policy and translated artifact
identity. DeviceState v12 additionally binds DPC CLOCK, BUFBUSY, PIPEBUSY, and
TMEM. DeviceState v13 binds the ABI-owned RSP interpreter continuation:
distinct exact/compatibility/unavailable/in-flight lifecycle tags, complete
scalar and vector state, SP/DPC registers, and ordered pending DPC submissions.
DeviceState v14 additionally binds each loaded/lineage/owner admission
generation and the next process-monotonic generation so task-address reuse
cannot alias a prior commit authority.
DeviceState v15 narrows DPC CLOCK, BUFBUSY, PIPEBUSY, and TMEM to their public
24-bit domains at the runtime import boundary and rejects a noncanonical value
at release encoding. It does not claim modeled counter increments or close the
still-open STATUS counter-clear/transaction interleavings.
DeviceState v16 distinguishes pending PI ROM and SRAM requests with a typed
device-relative address. Live timing v2 applies the same identity to PI DMA
rows, so equal offsets in different devices cannot alias in either channel.
Fixed-cycle report construction admits only
`AudioTaskExecutionPolicy::LleAccuracy`; translated callbacks cannot prove a
match to live IMEM, and diagnostic skip is explicitly non-release.
Pointer identity is excluded while the one-process-RDRAM
invariant, buffer length, and guest-visible delivery fields are retained.
MBC3 powered-off persistence keeps this boundary deterministic: the host
explicitly injects sidecar checkpoint/resume timestamps, restore materializes
their elapsed interval into the live RTC/guest-cycle phase once, and the
runtime discards the timestamps. Evidence therefore binds the resulting
future-visible RTC/phase but no host wall-clock value. The sidecar is a
versioned fn64 host format bound to the exact Game Boy ROM SHA-256 and public
timer+battery cartridge type; Pan Docs supplies the hardware RTC/oscillator/
battery semantics, not the file format or Unix-time policy.
`SectionRegistry::evidence_snapshot` provides the corresponding typed overlay
projection: registration-order section geometry and function offset/ROM-size
metadata, canonical sorted residency/runtime-load/static-storage maps, and the
exact in-flight static-mirror cursor. Its derived lookup cache and native
function-pointer bits are intentionally absent. This runtime projection alone
does not prove callable-body identity; the program-owning ABI aggregate must
bind that identity before a release schema can claim complete program state.
The typed-Rust program owner now has that separate owner-local projection. A
function-lane install must supply a stable 256-bit identity for the actual
generated native artifact; compatibility installs still run, but evidence
capture traps loudly while they remain unidentified. A block-lane snapshot
sorts every bank/span, retains every instruction word, and derives a
domain-separated SHA-256 over that image plus the caller-supplied artifact
identity of each generated bank runner. Code words alone are not treated as
proof of runner semantics. The live ABI projection additionally requires
stable artifact identities for the entry/transfer dispatch implementation and
each registered dynamic builder, then binds those identities, the instruction
budget, sorted physical/virtual executable-region geometry, active bank and
generation counters, and the canonical union of pending executable-write
ranges. Compatibility installs without those identities still execute, but
their evidence capture traps. Runner, resolver, builder, lookup, and native
pointer values are excluded. Schema v12 aggregates this projection at the
committed VI edge when the boot harness is built with `recomp-rs`; a stable
no-typed-program tag remains explicit for C/default builds and is not callable
body identity for the legacy C archive.
`Executor::control_evidence_snapshot` supplies the owner-local scheduler
projection for the same aggregate: RDRAM registration presence and length
(never its host pointer), canonical thread/queue/event maps, exact runnable and
waiter priority/FIFO-tie order (including each cached block-time priority),
pending resume payloads, stable timer firing/tie order, the
active run-token owner, virtual time, and CP0 Count/Compare/IP7 state. Snapshot
construction first validates that runnable IDs are unique and match runnable
thread state, queue waiters match blocked state, and pending resumes belong to
runnable queued threads. Diagnostic traces and native coroutine stacks/
continuations are excluded. Consequently two executors paused at different
opaque native continuation points can have equal control snapshots; this is a
fixed-cycle evidence projection for aggregation, explicitly not a whole-
executor savestate or a claim that native continuation state is portable.
Schema v12 aggregates this control projection with the raw device and ABI-owner
snapshots at the committed VI edge. The same opaque boundary freezes the
supported host target, exact four-port PIF identities, closed cartridge-save
configuration, graphics execution policy, and renderer self-report. The live
gate performs no later ambient query to construct those fields. Compatibility
save/backend registrations remain runnable but are rejected as unidentified;
RT64 evidence also binds its authoritative build identity, active settings
digest, and whether an enabled nonempty replacement-pack set was active.
Only byte-commit/completion variants and the
post-`osSpTaskLoad` admission boundary satisfy their narrowly named closure
paths. This keeps an accepted or queued request distinct from bytes that
became observable, and it does not use synchronous task loading to claim raw
timed SP DMA. VI interrupts remain VI events rather than being relabeled DMA.

Both runtimes, when built with tracing enabled, emit the same structured
  event stream so a diff tool never has to reconcile two different logging
formats:

```rust
pub struct TraceEvent {
    pub seq: u64,          // the global sequence counter from §3 (fn64 side);
                            // reference-runtime side assigns the same role
                            // to its own monotonic counter at emission time
    pub sim_time: u64,     // 93.75 MHz CPU master cycles, not wall clock or OSTime
    pub kind: TraceKind,
}

pub enum TraceKind {
    ThreadSwitch { from: ThreadId, to: ThreadId, reason: SwitchReason },
    QueueOp { queue: RdramAddr, op: QueueOpKind, thread: ThreadId }, // send/recv/block/wake
    Dma { direction: DmaDirection, dram: RdramAddr, device: PiDeviceAddress, len: u32 },
    TaskSubmit { task_kind: TaskKind, ucode: u32 }, // RSP gfx/audio StartGo handoff
}
```

The complete `TaskLog` records synchronous `osSpTaskLoad` admission, while
`TaskSubmit` is emitted only after `osSpTaskStartGo` consumes that admitted
token. A later Load can replace an unstarted task image without fabricating an
execution trace or satisfying an RSP task-execution closure path.

Each event names *what changed*, not implementation-internal state, so it's
comparable across two structurally different implementations (OS-thread
model vs. coroutine model) — a `ThreadSwitch` event is meaningful whether
the "thread" underneath is a host `std::thread` being parked or a
coroutine being suspended; the comparator (below) only ever needs the
logical event stream, never runtime internals from either side.

### Comparator plan

A standalone tool (`fn64-shell`'s `--trace-compare` mode, or a small
separate binary once the format stabilizes) ingests two `TraceEvent`
streams — one from the reference runtime, one from fn64 — for the same
boot/input sequence, and asserts:

1. **Same `QueueOp` sequence per queue address** (modulo interleaving from
   `ThreadSwitch` ordering that both models are free to make differently
   as long as delivery order per queue is preserved — libultra's own
   message-queue contract is FIFO per queue, not a global total order).
2. **Same `Dma`/`TaskSubmit` sequence and payload sizes** — this is the
   direct differential-testing mechanism `AGENTS.md` requires ("Runtime
   behavior changes emit the shared event trace and get diffed against the
   reference runtime over identical recompiled code").
3. A structured diff report (first divergence: sequence number, event kind,
   both sides' payloads) — not a pass/fail bit; per this project's own
   verification-contract precedent (`CLAUDE.md`'s "never a fuzzy/bbox/partial
   match"), a diff that silently drops mismatched-but-similar events is
   worse than one that fails loud.

### Milestones

- **M1 — boot-to-idle parity.** fn64, linked against a real game's
  `RecompiledFuncs`, reaches the same idle/attract-mode depth the reference
  runtime's boot ladder has already validated (the playbook's rung
  progression is the existence proof this depth is reachable at all) —
  trace-compared clean, no divergence, for the deterministic (non-input)
  portion of boot.
- **M2 — current-rung parity.** fn64 reaches whatever rung the reference
  runtime's `profile.toml` most recently closed (today: past rung 18's
  scheduler_mutex fix, at the still-open TOCTOU-adjacent frontier) — i.e.
  fn64 is never the lagging system; its bring-up is paced by and validated
  against the reference's own hard-won ladder, not a separate one climbed
  from scratch.
- **M3 — full swap + shell rewrite + relicense.** fn64-shell replaces the
  reference runtime's own executable/windowing/input entirely; the GPL-3.0
  scaffold (`aki-recomp`'s vendored/forked runtime) is retired from the
  product's runtime dependency graph (it remains, permanently, the
  differential-testing oracle in CI, never the shipping runtime); the
  shipping artifact is MIT OR Apache-2.0 end to end, matching `README.md`'s
  license goal.

## 5. Work packages, sized in waves

Sequenced by dependency; items in the same wave parallelize (independent
files/crates, no shared state):

**Wave 1 — scaffolding (this doc's own deliverable).**
- Workspace skeleton, `fn64-abi`'s first representative symbols, C smoke
  test. (Parallelizes trivially against nothing — it's the prerequisite for
  every later wave.)

**Wave 2 — `fn64-runtime` core types (parallel sub-tasks, no shared state).**
**DONE (2026-07-14).**
- `Rdram` + `MEM_*`-equivalent accessors + `RdramAddr` (§3). Landed wave 1.
- `MesgQueue` + `BlockedList` + `EventTable` (§2) — `mesgqueue.rs` (landed
  wave 1) + `executor.rs`'s `event_table` field.
- The executor/coroutine scheduler (§2) — `executor.rs`'s `Executor`,
  priority-ordered run queue, `thread.rs`'s `GameThread`/`RunToken`/
  `Yield`/`Resume`. Rung regression suite (`rung_12_*`/`rung_14_*`/
  `rung_18_*` + ping-pong/full-queue-block/timer-ordering property tests)
  in `fn64-runtime/tests/rung_regressions.rs`.
- Timer wheel (`osSetTimer`/`osStopTimer` semantics, VI-tick-driven) —
  `timer.rs`'s `TimerWheel`, driven by `Executor::advance_time`'s virtual
  clock (no wall-clock in core, per this doc's requirement).
- Differential-trace scaffolding (`trace.rs`'s `TraceEvent`/`TraceKind`/
  global sequence counter, §4) landed alongside the executor rather than
  deferred to wave 6, since every executor event needed a place to record
  to from day one.
- See "Implementation notes (wave 2/3)" above this section for what
  building it taught us (the `may_block`/`Resume::WouldBlock` unification;
  a real ABI-layer reentrancy bug and its fix).

**Wave 3 — `fn64-abi` surface, by ABI-SURFACE.md's own grouping (parallel
per group once wave 2's matching runtime API exists).**
- `recomp.h` dispatch helpers: `pause_self`/`switch_error`/`do_break`/
  `get_function` **DONE** (M1 wave, 2026-07-14). This wave discovered and
  fixed a real signature mismatch from the prior wave's implementation:
  `pause_self` is `void pause_self(uint8_t *rdram)` (ONE argument, no
  `ctx`), `switch_error`/`do_break` take no `rdram`/`ctx` at all, and
  `recomp_context` is the REAL 32-gpr/32-fpr/hi/lo/f_odd/status_reg struct,
  not the 9-field subset a prior wave modeled — verified directly against
  `aki-recomp/games/NWXE/RecompiledFuncs/recomp.h` (N64Recomp's own
  MIT-licensed generated/vendored header) and real call sites, not
  re-derived from `ABI-SURFACE.md`'s prose alone. `get_function` is backed
  by the new `fn64-runtime::overlay::SectionRegistry` (§1's long-deferred
  overlay/`get_function` lookup table, built this wave — see below).
  The legacy corpus had no `cop0_status_*` call site per `ABI-SURFACE.md`; the
  arbitrary-PC lane now owns typed Status/Cause/EPC state because precise
  exceptions and interrupts require it independently of shim reachability.
- Thread lifecycle shims: `osCreateThread_recomp`/`osStartThread_recomp`
  **DONE** (M1 wave) — real dispatch via `SectionRegistry::resolve`, no
  longer `unimplemented!()`. `osSetThreadPri_recomp` **DONE** (prior wave,
  no dispatch-gap blocker). `osGetThreadPri`/`osGetThreadId` not yet
  reached.
- Message-queue shims: `osCreateMesgQueue_recomp`/`osSendMesg_recomp`/
  `osRecvMesg_recomp`/`osSetEventMesg_recomp`/`osSetTimer_recomp` **DONE**.
  `osJamMesg`/`osStopTimer_recomp` not yet reached.
- PI/SI/EPI DMA shims: `osCreatePiManager_recomp`/`osCartRomInit_recomp`/
  `osEPiStartDma_recomp`/`osVirtualToPhysical_recomp`/`osSetIntMask_recomp`/
  `osInitialize_recomp`/`osAiSetFrequency_recomp` **DONE** (M1 wave), backed
  by the new `fn64-runtime::rom` module (`RomStorage` trait, `PiDma`,
  `InMemoryRom`) — see §3's new "The PI/ROM seam" subsection.
  `__osSiRawStartDma_recomp`/`osSpTaskYielded_recomp` are loud, named
  `unimplemented!()`s (no real PIF-controller/RSP-task-execution model
  exists yet; see their doc comments in `fn64-abi/src/lib.rs` for why a
  silently-succeeding stub would be worse). `osEPiStartDma_recomp`'s
  `OSIoMesg` field-offset assumptions are flagged NOT YET byte-verified
  against a real ROM struct-init call site — honest "not verified," not a
  false "done," per `AGENTS.md`.
- VI/AI shims: `osAiSetFrequency_recomp` **DONE**. The `osVi*` family
  (`osViSetMode`/`osViSetSpecialFeatures`/`osViSetYScale`/`osViSwapBuffer`/
  `osViBlack`) are loud, named `unimplemented!()`s (T2 per
  `aki-recomp/runtime/M1-WORKLIST.md` — needed for the boot chain to
  complete, but no display/VI-hardware backend exists in this crate yet;
  that's `fn64-shell`'s wave-5 windowing piece). Implemented from the
  union (not either game's current subset) per this section's original
  guidance.
- `recomp_overlays.inl` consumption **DONE** (M1 wave):
  `fn64-runtime::overlay::SectionRegistry` (`Section`/`FuncEntry`, §1's
  shapes) resolves `get_function`'s `vram -> recomp_func_t*` lookup,
  correctly modeling NWXE's REAL bank-switch overlap (sections 2/5 and 3/4
  both declare the same `ram_addr` range in the actual
  `recomp_overlays.inl` — verified by reading the generated file directly)
  via an explicit `loaded: HashSet<SectionIndex>` rather than a flat
  address map, so only the currently-PI-mapped bank's functions resolve.

**M1 gate (2026-07-14): WM2000 (NWXE) `RecompiledFuncs` links clean against
`fn64-abi`.** Per `aki-recomp/runtime/M1-WORKLIST.md`'s 23-symbol undefined
set (16 T1 + 7 T2): all 51 `RecompiledFuncs/*.c` files recompiled fresh from
source, archived, and trial-linked (`-force_load` + a stub `main`, the same
method `M1-WORKLIST.md` used to derive the 23-symbol set) against a
release build of `fn64-abi` — **zero undefined symbols remain** beyond
ordinary libc/pthread/dyld/Rust-runtime symbols (confirmed via `nm -u` on
the linked binary, grepped for any `recomp`/`os*`/`switch_error`/`do_break`/
`get_function`-shaped name: none found). T1 symbols are real, tested
implementations; T2 VI-family symbols are loud named traps by design (no
display backend exists yet), which is sufficient for THIS gate (a clean
*link*, not a clean *boot to idle* — that's M1's "boot-to-idle parity"
milestone in §4, separate and not yet attempted).

**M1 boot-host attempt (2026-07-14): `recomps/wm2000/packages/wm2000-boot`, first real boot
run against the linked archive.** Per the task's own scope (a headless boot
host taking `RECOMPILED_DIR`/`RECOMP_H_DIR`/`ROM` env vars, zero game content
in-repo — `recomps/wm2000/packages/wm2000-boot/build.rs` and the shared
`crates/fn64-boot-harness/bridge/section_bridge.c`): this is
the FIRST time the M1-linked archive was actually RUN, not just linked, and
it surfaced four real, load-bearing bugs the trial-link gate above could not
have caught (a clean link says nothing about correct runtime behavior):

1. **`fn64-abi`'s `EXECUTOR` reentrancy.** A plain `RefCell<Executor>`
   panicked ("already borrowed") the moment ANY non-blocking `_recomp` shim
   (e.g. `osCreateThread_recomp`) ran as part of `Executor::run_one_step`'s
   own coroutine resume — not a rare edge case, the NORMAL path for a
   running thread creating another thread. Fixed via `ReentrantCell`, a
   documented, single-thread-only interior-mutability wrapper (see its doc
   comment in `fn64-abi/src/lib.rs` for the full soundness argument); a new
   regression test drives the exact nested shape.
2. **`osStartThread`/`osSetThreadPri`/`osGetThreadPri` were keyed on the
   wrong identity.** A prior wave's doc comment asserted real call sites
   pass the same `OSId` to `osStartThread` that `osCreateThread` received —
   real disassembly (`funcs_0.c` asm 0x800004AC-0x800004B8) disproves this:
   both calls pass the SAME `OSThread*` handle, never the `OSId` a second
   time, and `osSetThreadPri(t=NULL, pri)` means "the calling thread," a
   documented libultra convention. Fixed via `HostState::thread_handles` (an
   `OSThread* -> OSId` map populated by `osCreateThread_recomp`) and
   `resolve_thread_arg`'s null-means-self handling.
3. **`osCreateThread_recomp` never seeded the new thread's stack pointer.**
   `entry_ctx.r29` was left zeroed; the real `sp` argument (stack-passed,
   per `osCreateThread`'s documented signature) was read but discarded. Any
   real thread entry point touching its own stack (i.e. every one) crashed
   immediately. Fixed by seeding `entry_ctx.r29` with the real `sp` value.
4. **`MEM_W`/`MEM_H`/`MEM_HU` are NATIVE-endian, not big-endian.** The
   single most consequential correction: `fn64-runtime::Rdram`'s word/
   halfword accessors and `fn64-abi`'s `read_stack_word` all used
   `from_be_bytes`/`to_be_bytes`, based on a prior wave's mistranscription
   of `ABI-SURFACE.md` section (c)'s prose summary. The generated `recomp.h`
   macro itself (quoted directly, MIT) is `*(int32_t*)(rdram + ...)` — a
   PLAIN NATIVE POINTER DEREFERENCE. The `^2`/`^3` byte-lane XOR on
   sub-word accessors exists BECAUSE the backing store is native-endian
   (little-endian on every real fn64 host); it corrects sub-word addressing
   relative to that, and would be pointless if the store were actually
   big-endian. First caught when a spawned thread's own real stack pointer
   came back exactly byte-swapped. Fixed throughout `Rdram`'s accessors and
   every `fn64-abi` call site that hand-rolled the same assumption
   (`osRecvMesg_recomp`, `read_os_task_header`, several tests).
5. **`osEPiStartDma_recomp`'s `dramAddr`/`retQueue` fields need KSEG0
   translation, and a sibling double-translation bug.** `dramAddr`/
   `retQueue` are raw vram POINTERS the game computed normally — they need
   `RdramAddr::from_gpr`'s translation like any other vram value, not
   `RdramAddr::from_offset` (no translation, silently wrong). Separately,
   the OTHER `OSIoMesg` fields were being read via `read_stack_word`, which
   itself re-applies the KSEG0 subtraction to an already-resolved
   `mb_addr.offset()` — a double subtraction producing garbage. Fixed via a
   new sibling helper (`read_offset_word`, takes an already-resolved
   offset, never re-translates) plus correcting the two vram-pointer fields
   to `from_gpr`.

**Result, honestly reported:** boot now progresses far past every prior
milestone — thread 0 (`recomp_entrypoint`) runs its real body, spawns and
starts a second real thread with a correctly-seeded stack, that thread
(id 6) runs real recompiled code three call-levels deep
(`func_800222D8` → `func_80003720` → `func_80000660`) into a REAL
`osEPiStartDma_recomp` PI-DMA call that completes without crashing. Boot
then reaches a state that runs for tens of seconds of wall-clock CPU time
inside a single `Executor::run_one_step` call with no crash and no log
output — i.e. the recompiled code is executing a real (long or unbounded)
recompiled loop inside `func_800004D0` that this milestone's stubs never
observed to terminate, most likely because our SI/PIF or PI-DMA completion
model isn't yet posting whatever the game's own poll loop is waiting for.
**Not a false "boot to idle"**: this is the honestly-reported frontier —
three `TraceEvent`s recorded, VI retrace never reached (no `osViSetMode`
call observed before the stall), zero framebuffer swaps, zero RSP tasks
submitted. `fn64-abi`'s 4 real bugs above are fixed and regression-tested;
the stall itself is a new, not-yet-root-caused frontier for the next wave,
not something papered over. The out-of-tree `wm2000_audio.cpp` (RSPRecomp's
own generated audio ucode) could not be linked at all in this wave: RSPRecomp's
codegen template unconditionally emits `#include "librecomp/rsp.hpp"`, which
lives under `N64ModernRuntime`'s GPL-3.0-licensed tree (verified: that repo's
top-level `COPYING` is GPL-3.0; `librecomp/` is not under the MIT-carved-out
`N64Recomp/` subdirectory) — a real, load-bearing clean-room blocker, not
routed around. The audio task-dispatch plumbing now owned by
`osSpTaskStartGo_recomp` now executes admitted live audio-task IMEM through
fn64's clean-room RSP interpreter. Optional translated callbacks carry an exact
artifact identity but are not release authority for arbitrary live IMEM.

**Wave 4 — `fn64-rt64` bridge (parallelizes against wave 3, converges at
the RSP task boundary).**
- RSP audio-ucode task submission (the one RESOLVED boundary per
  `ABI-SURFACE.md` (e): `games/NWXE/rsp/wm2000_audio.toml`'s byte-verified
  `text_offset`/`text_address`/entry points).
- Gfx task handoff — explicitly blocked on real evidence per §1's rationale
  (3): do not guess the shape; wait for a profile.toml rename wave to reach
  an `osSpTaskLoad`/`osSpTaskStartGo` call site, then extract the real
  signature the same mechanical way `ABI-SURFACE.md` extracted everything
  else, before writing this wave's code.

**Wave 5 — `fn64-shell` (depends on wave 3 substantially complete).**
- Window/input/audio-out backend selection.
- ROM/`RecompiledFuncs` intake (user supplies their own recompiler output —
  no game content ships in this repo, ever).

**Wave 6 — differential harness (parallelizes against waves 2-5 once each
lands its first behavior; grows incrementally, never "done" as a single
wave).**
- `TraceEvent`/`TraceKind` types + emission call sites (§4).
- Comparator tool.
- CI wiring: boot a pinned game/profile under both runtimes, diff the trace,
  fail loud on first divergence.

## 6. Provenance appendix

Every source consulted while writing this document, and what it licensed us
to claim:

| Source | License / kind | What it informed |
|---|---|---|
| `aki-recomp/docs/BOOT-LADDER-PLAYBOOK.md` | our own method doc | §2's decision-tree framing, validation-bar language, tool-to-question map |
| `aki-recomp/games/NWXE/profile.toml` rung 12 comment block | our own debugger/disasm evidence trail | §2 and §3's `MesgQueue`/`osCreateMesgQueue` reset invariant |
| `aki-recomp/games/NWXE/profile.toml` rung 18 / 18 follow-up / 18 follow-up #2 comment blocks | our own lldb + hardware-watchpoint evidence trail | §2's threading-model case study; §3's watch/diagnostic-hook design (what failed and why) |
| `aki-recomp/runtime/ABI-SURFACE.md` + `runtime/abi_surface.json` | mechanically extracted from N64Recomp-generated C (both games) + `recomp.h`/`symbol_lists.cpp` (MIT) + `librecomp/include/librecomp/sections.h` (public interface header, ABI only) | §1's crate boundaries and Wave 3's symbol grouping; §3's `recomp_context`/`MEM_*` semantics; §4's link-time-swap/`nm`-completeness mechanism |
| `fn64/README.md`, `fn64/AGENTS.md`, `fn64/CONTRIBUTING.md` | our own project docs | Crate names (final, per README's table), validation bars, clean-room protocol, licensing goal |
| `aki-recomp/AGENTS.md`, `aki-recomp/PINS.md` | our own project docs | Cross-repo context: which repo is the behavioral-spec source, pinned reference commit hygiene |
| Public libultra manual (message-manager / thread-manager sections; general knowledge of `osCreateMesgQueue`/`osSendMesg`/`osRecvMesg`/`osSetEventMesg`/`osCreateThread`/`osSetThreadPri` semantics — priority-based scheduling, FIFO per-queue delivery, blocking vs. non-blocking send) | public documentation | §2's `OSMesgQueue` semantics, priority-based resume ordering |

Explicitly NOT consulted, per the clean-room protocol in `AGENTS.md`:
`vendor/N64ModernRuntime/**/*.cpp,*.hpp` (ultramodern/librecomp
implementation bodies) — every claim about the reference runtime's actual
behavior above is sourced from our own black-box observation (lldb
backtraces, hardware watchpoints, disassembly of the compiled binary, the
mechanically-extracted ABI surface), recorded in `aki-recomp`'s own
evidence trail, never from reading its GPL implementation source.
