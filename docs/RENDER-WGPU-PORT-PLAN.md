# RT64-to-Rust renderer program

This is the canonical implementation plan **and live status entry point** for
replacing fn64's quarantined C++ RT64 integration with an idiomatic Rust
renderer. The milestone contracts below are deliberately stable. Every working
session updates the status capsule and handoff near the top of this file rather
than starting another plan or appending a chronological diary; git history is
the archive.

Crate name: **`fn64-render-wgpu`** (parallel to `fn64-render-rt64`, exactly
as `fn64-cpu-runtime` parallels the N64Recomp adapter).

The product target is a completely Rust-owned fn64 render pipeline: no
project-owned C++, C ABI shim, CMake build, or foreign-pointer lifetime. Native
`wgpu` still reaches Metal, Vulkan, and D3D12 through platform bindings, so
"all Rust" describes fn64's source, ownership model, and build graph rather
than claiming that an operating-system graphics driver contains no FFI.

The governing principle is **faithful behavior, idiomatic Rust structure**.
Port allowed MIT RT64 behavior and fn64's pinned source overlays; do not
transliterate RT64's pointer graph, mutable `State`, thread topology, or plume
RHI. Known RT64 defects are not compatibility requirements. Those cases use
the Rust reference lane, public libultra manuals, public hardware evidence, or
an admitted differential fixture as authority. GPL runtime implementation and
m2c remain excluded by `AGENTS.md`.

Permitted RT64 algorithms are ported faithfully and differential-gated per
module, except for named RT64 defects whose authority is hardware or a
permitted reference. Rust types, ownership, memory layout, and GPU APIs may
modernize the mechanism without silently changing the required observable.

The required model/effort, delegation, path-ownership, review, and escalation
protocol is [`RT64-PORT-ORCHESTRATION.md`](RT64-PORT-ORCHESTRATION.md). It
uses lower-cost delegates where a deterministic oracle makes that safe, while
reserving authority, parity, performance, and integration claims for the lead.

## Live status capsule

| field | current value |
|---|---|
| updated | 2026-08-16 (production triangle draw: real decoded RawTriangle admitted into the sealed plan and drawn) |
| program state | **IN PROGRESS** |
| execution wave | **ACCEL-A -- port spine and evidence in parallel** |
| active milestones | **M0 authority/baseline, M2 shader/tool feasibility, M3 raw-DPC vertical slice, and M4 base RDP/TMEM correctness (all IN PROGRESS)** |
| active slices | M2.5.2 typed wgpu-ingestion assessment; M4.2a transactional physical TMEM state; T3 physical-successor follow-on; A0 workflow frontier maintenance; M4 combiner fragment-register wiring (`combiner_inputs_from_fragment_registers` derives `CombinerInputs.env_color`/`prim_color`/`prim_lod_frac` from decoded `Color4`/`PrimColor`/`PrimLod`; CPU-oracle-only, no draw-path wiring) |
| active ownership | `F/xhigh` integration lead; `F/xhigh` M2.5.2 assessment owner; `F/xhigh` M4.2a physical-state owner; independent reviewers remain serialized from writers |
| last completed result | Production triangle draw closes the RawTriangle sealed-plan admission gap (`fn64-rt64-production-triangle-draw-card.md`): `render_ir.rs` gains a third `RawDpcSemanticCommandRef::Triangle(&RdpTriangleCommand)` variant, `RdpTriangleCommand{location, raw_words, vertices: [NeutralTriangleVertex; 3]}` (deliberately no `before`/`after` -- a draw event pushes zero `ResourceAccess`, verified against `ExactRawDpcPlanWriter::finish`'s access-ordering contract having zero coupling to `self.commands`), and `ExactRawDpcPlanWriter::push_triangle`. `production_adapter.rs` gains a variable-width raw-word slicer (`triangle_command_raw_words`, keyed off `raw_rdp_command_width` -- cannot reuse `tmem_command_raw_words`'s fixed-2-word assumption) and a `RawTriangle` push arm, removing it from the catch-all rejection arm (which now holds only `NoOp`/`FillRectangle`/`FullSync`). Each triangle is decoded against the `OtherMode` current AT ITS OWN STREAM POSITION: a local `Option<OtherMode>` tracked forward through the push loop, seeded from `decoded.base_state.other_mode()` (the durable state a caller held before this exact decode -- `production_adapter` is a descendant module of `raw_dpc`, so this no-modifier field is already readable with zero new accessor and zero signature change) and updated again on every in-plan `SetOtherMode`. A triangle walked with no `OtherMode` established at all -- neither carried in nor set earlier in this plan -- is a loud, typed `TriangleBeforeAnyOtherMode` rejection, not a silent `(0,0)` default (this value feeds `texture_perspective()` into `decode_triangle_vertices` and changes decoded geometry). New `raw_dpc/triangle_draw_data.rs`: a plain `NeutralTriangleVertex -> RasterVertex` field adapter, and `TriangleDrawStateCollector` (an `ExactRawDpcPlanVisitor`, same shape as `production.rs`'s own `PlanCollector`) that snapshots the `SetOtherMode`/`SetCombine` state current at each triangle's own stream position onto that triangle -- per-triangle, not a single whole-plan-final value, so a later state change cannot retroactively apply to an earlier draw; a triangle snapshotted before its own state is a loud, indexed `MissingTriangleDrawState` naming which triangle. `targets/triangle_pipeline.rs` gains `TrianglePipelineRenderer::submit_admitted_triangle`, a thin call site feeding real retrieved vertices/state into the existing `submit_triangle` entry point -- no pipeline construction restructuring. |
| next concrete decision | connect typed fragment registers, texture-gen output to the combiner/blender/depth inputs for a textured/blended slice; wire a real command-stream dispatcher that calls T1's `push_decoded_raw_dpc` and M4.3.4's `execute_fill_rectangle` from the same walk; multi-triangle/real draw-command-stream/frame assembly (this slice's `TriangleDrawStateCollector` already returns a `Vec` of per-triangle draws, so multi-triangle retrieval has no known blocker, only untested); a parallel lane (`260d95e8`-style follow-on) consuming this slice's coordinator-reachable plan for zero-TMEM triangle-only execution completion; continue the typed all-56 wgpu-ingestion assessment |
| evidence blockers | no matched private-game RT64 performance baseline exists; ShaderNonUniform SPIR-V remains blocked by Naga 30; texture sampling, blend, alpha compare, coverage-write, and multi-triangle batching are not yet composed into the triangle draw path; production DPC dispatch still does not consume the owned-read path or call M4.3.4/T1 from a shared per-command walk |
| verification claim | Production triangle draw passed 10 consecutive clean `cargo test -p fn64-render-wgpu --lib` runs (925/925 each, default features) and 10 consecutive clean runs with `--features host-gpu-tests` (932/932 each, real Metal execution evidence for the end-to-end host-GPU test), scoped clippy (`--no-deps --all-targets`, both feature sets) clean, explicit-file rustfmt (edition 2021) clean on every touched file, `lint-docs.py` clean against the pre-existing 5-error `origin/main` baseline (0 new errors -- `docs/BASE-RENDERER-BEHAVIOR-MATRIX.md` regenerated via `tools/check_base_renderer_matrix.py --write-doc` to absorb this slice's `crates/fn64-render/src/lib.rs` +1-line shift, verified as a pure line-anchor update, no content change), and one bounded independent adversarial review (caught and fixed across three passes during implementation: a triangle-decode-time `OtherMode` ordering gap, corrected from a silent `(0,0)` default to a loud typed rejection; a retrieval-time reintroduction of the same ordering bug one layer downstream, corrected from a single whole-plan-final state value to per-triangle stream-position snapshots; a missing cross-submission durable-state seed, closed via the zero-signature-change `decoded.base_state` read rather than a wider public API; final independent review pass found zero further issues across five focus areas). Unified RDP pure-state admission (`71ab8b4e`) and the real wgpu vertex+fragment triangle pipeline (`d6250fcc`), the two dependencies this slice built on, each separately passed their own recorded final-source gates. M2.5.1 has an independently verified 56/56 conditional reference-valid corpus but no runtime/parity claim. |

The canonical per-ticket status, blockers, owners, branches, and verification
counts are in [`RT64-PORT-DASHBOARD.md`](RT64-PORT-DASHBOARD.md), generated
from `rt64-port-status.json`. The capsule summarizes that ledger; it does not
replace or independently override it.

Allowed milestone states are `PLANNED`, `READY`, `IN PROGRESS`, `BLOCKED`, and
`COMPLETE`. `COMPLETE` means every exit gate is recorded with evidence; code
existing or one test passing is not completion. Several milestones may be
`IN PROGRESS` only under the accelerated track below: their contracts and
writable paths must be disjoint, a downstream slice cannot claim an upstream
gate, and the integration lead remains the sole owner of cross-milestone
decisions. An incomplete evidence milestone blocks parity, performance, and
cutover claims; it does not block source translation that preserves the pinned
oracle as a differential fallback.

## Accelerated execution track

The port is a source-translation and ownership-migration program first. M0 is
no longer a serial prerequisite for writing M1-M5 code. The pinned C++ oracle
stays runnable while independent Rust slices advance; evidence closes beside
the port at the earliest layer that can locate a divergence.

The accelerated critical path is:

```text
machine inventory + generated task cards
              |
              v
typed IR and move-only lifecycle spine
              |
       +------+------------------+
       |                         |
       v                         v
raw-DPC/TMEM/framebuffer     GPU capability and pass-graph probes
       |                         |
       +------------+------------+
                    v
          first Rust-native frame
                    |
        parallel opcode/GBI/VI/backend families
                    |
             parity and cutover gates
```

Execution rules:

1. **Port and evidence proceed together.** M0.3 retains one lane; lack of a
   private receipt cannot idle semantic, decoder, fixture, or GPU-probe work.
2. **Prefer mechanical preservation before redesign.** Preserve allowed RT64
   algorithms, HLSL, constants, layouts, and ordering. Refactor architecture
   only where Rust ownership requires it or a measurement proves value.
3. **One vertical spine before breadth.** The first production target is an
   owned raw-DPC stream through TMEM/framebuffer, raster work, minimal VI, and
   headless capture. No layer-complete detour may postpone that frame.
4. **Generate the denominator.** A checked inventory assigns every admitted
   RT64 source/module/symbol to a task card, milestone, owner, port state, and
   evidence state. Hand-maintained “roughly complete” lists are not authority.
5. **Replay offline.** Privacy-safe oracle records drive millisecond semantic,
   TMEM, framebuffer-operation, shader-key, and image-hash differentials. Full
   game boots are integration gates, not the inner development loop.
6. **Keep shaders off the translation critical path.** Existing permitted HLSL
   remains the initial shader authority and is compiled to backend artifacts;
   shader-language modernization follows parity.
7. **Scale by exclusive paths.** The current four-thread wave uses one lead
   plus three writers. An 8-10-thread worktree wave may add disjoint decoder,
   framebuffer, Metal, Vulkan, D3D12, fixture, and review lanes; shared-crate
   contracts and `fn64-abi` remain serialized.
8. **Reliability counts are merge gates.** Focused tests run in the fast loop;
   the 10-run deterministic and 20-run concurrency bars run when a slice is
   ready to claim, not after every edit.

### Backend-neutral parity ladder

Migration is driven by one conformance fixture contract with multiple backend
delegates, not separate RT64 and Rust test suites. Each required feature row
declares its authority, observable layers, RT64 expectation, Rust expectation,
and evidence state. The existing FFI adapter may satisfy `RT64_PASS` only after
the fixture proves it exercised the intended qualified behavior; the Rust
delegate begins `RUST_PENDING` and must satisfy the identical contract before
promotion to `RUST_PASS`.

Allowed row states are `RT64_PASS`, `RT64_DIVERGES`,
`RT64_PUBLICLY_UNAVAILABLE`, `RUST_PENDING`, `RUST_PASS`, and
`RUST_BOUNDED_QUALIFICATION`. A known RT64 defect uses hardware/reference
authority and records `RT64_DIVERGES`; it is never baked into the Rust target.
Unavailable FFI observability is explicit and cannot masquerade as a passing
or skipped test.

The harness compares the earliest discriminating layer available: admitted
commands and state, FullSync timeline, TMEM bytes/hash, resource journal and
guest-memory effects, shader parameters, framebuffer native/high targets, VI,
and finally post-VI pixels. Main CI runs every already-closed Rust row and a
denominator checker that forbids regression or new implicit skips. A separate
port-progress command exits nonzero while any required `RUST_PENDING` row
remains. Each small feature branch names the exact rows it closes and must turn
them into executed passes before merge. M10 cutover requires zero required
pending rows; a permanently red ordinary CI suite is not used as status.

The conformance suite is split by responsibility. A renderer-neutral fixture
crate owns bounded inputs, expected observable layers, row IDs, and result
receipts; it owns no RT64 handles, wgpu objects, emulator scheduling, or guest
memory. `fn64-render-rt64` supplies the feature-gated FFI delegate and the Rust
renderer supplies a separate delegate for the same contract. `fn64-abi` may
publish only committed guest facts needed by a fixture, while certification
validates and renders receipts without inventing renderer facts.

A delegate pass is proof-bearing. Its receipt binds the fixture digest,
delegate/build identity, qualified RT64 source identity when applicable,
workload and FullSync lineage, and the earliest observable that distinguishes
the behavior under test. Every assertion mechanism has a negative mutation
test showing that a wrong byte, event, ordering, or identity is rejected. A
successful function return, a backend label, or a final-pixel match alone
cannot close a stronger TMEM, framebuffer-coherence, timing, or memory-effect
row. The initial harness branch must demonstrate three cases end to end: one
qualified RT64 pass, one declared RT64 divergence or public unavailability,
and one Rust-pending row whose progress gate fails without making ordinary CI
permanently red.

Aggressive planning targets, measured as continuous active execution rather
than calendar promises, are: first Rust-native frame in 2-4 days, a playable
primary-platform renderer in 7-10 days, primary-platform parity in 10-20 days,
and public cross-backend feature parity in 3-5 weeks. These are scheduling
targets, not evidence claims; the recorded milestone gates remain authoritative.

### Resume here in a new session

1. Read `AGENTS.md`, `README.md`, `docs/DESIGN.md`, then this file completely.
2. Run `git status --short`. Preserve unrelated and user-owned changes.
3. Read the live status capsule, milestone table, pending decisions, and last
   session handoff. Do not rediscover completed audits unless the recorded
   source identity has changed.
4. Claim one bounded slice from an active accelerated lane. Record its owner,
   exact paths, baseline, and exit evidence in the handoff before editing code.
5. Work the slice loop below. The lead may keep independent lanes full, but one
   writer does not abandon or broaden its slice merely because it is difficult.
6. Before ending, update the capsule, milestone row, evidence, and next exact
   action. If blocked, record the failing invariant and what was ruled out.

## Last session handoff

**Accelerated wave:** ACCEL-A started on 2026-08-15. M0.3 retains one
non-blocking evidence lane while implementation advances. The first parallel
writers own the bounded trace wire, a generated RT64 source/task inventory,
and the new GPU-independent `fn64-render-ir` ownership spine. No result from
those active writers is integrated or verified merely because it is listed
here; the lead must review each diff and execute its stated gate.

**Integrated since that handoff:** the dual-pin authority/inventory gate,
wgpu 30 Metal capability baseline, and repaired render-IR ownership spine are
on local `main` as `34e00cf5`, `978981b2`, and `9879c7e0`. M1 is not complete:
ABI-issued provenance, backend-held completion evidence, and guest-memory
commit authority are the active M1.2 frontier. Metal M2.2 now proves the
selected adapter's integer/TMEM, binding-array, fractional blend, explicit
format-I/O, and invalid-reinterpretation behavior. Exact submission ownership,
timestamp validity, and the shader-compute N64 coverage path remain M2.3 work.

**Outcome:** M0.3 remains open. An unintegrated schema/checker slice defines an
honest `development` report with explicit unavailable metrics and exact
cross-field validation. It does not implement a bounded JSONL trace, raw
sample ABI, shell emitter, headless executor, or baseline collector. The only
declared route cannot observe physical presentation, so the checker must reject
every current attempt to claim `comparison_ready`. Raw advances will need to
retain committed VI fields and guest cycles instead of pretending every
advance is one field.

**Inventory decision:** existing fn64 CPU/frame-census observations are
usable only with their exact boundaries. The native RT64 rolling timer names
are not schema definitions: some measure inter-call cadence, some include
fence waits, and the native GPU interval excludes VI and presentation. Copy,
queue-wait, allocation, shader/PSO, full GPU-pass, total VRAM, and physical
presentation observations remain partial or missing and are the M0.3 work
queue. Missing and unarmed channels cannot appear as zero-cost work.

**Route decision:** the first M0.3 benchmark route is specified as a future
headless branch in `fn64-shell`, immediately after `Shell::boot` and before the
`EventLoop`, using `Shell::pump_one_frame`. No such measurement branch or JSON
emitter exists in the current tree. The proposed route reuses the established
RT64 device and excludes a second `pixels`/wgpu compositor. It must own a
deterministic neutral boot/title-v2 identity: boot-complete start, the
already-admitted owned ROM,
blank in-memory SRAM, literal neutral controller state, no ambient frontend,
an empty controller-command stream, a graphics-submit warmup threshold, and
an exact retained VI-advance horizon. All inherited `FN64_`, `OOT_`, and
`RSP_TRACE_` settings plus named loader/GPU selectors are rejected before the
route installs its own gates.
The request may select the numeric thresholds but cannot rename their meaning;
the collector verifies the census values at publication. Capture is one-shot
and outside the measured horizon because its readback/wait perturbs timing.
Each final repetition is a fresh process; five counterbalanced control/
instrumented pairs are required.

**Implementation frontier:** `fn64-abi` currently exposes only its aggregate
frame-census snapshot; there is no raw per-advance snapshot, post-warmup rebase,
or report-emission ABI. No shell or certification path emits this schema and
`tools/run_rt64_render_baseline.py` deliberately has no `--run` mode. The
checker validates exact nested keys and types, finite minima, sample order,
horizon/census and derived-latency equality, metric state/value shapes,
and path-free strings. V1 deliberately defers cohort acceptance because it
does not encode pair/repetition ordinals or requested/observed horizon and
workload boundaries; a false two-report gate is not retained. It does not
create a synthetic `comparison_ready` fixture. No private linked build was
exercised and no real matched baseline has been recorded.

The next implementation slice must add the Rust raw-sample ABI and shell JSON
emitter together, issue canonical program/build/GPU identity rather than
accepting request placeholders, and add a presentation-capable route before a
comparison cohort can exist. The runner and native measurement gates remain
separate work after that seam is reviewed.

**Delegation:** the integration lead/reviewer is `F/xhigh` (currently GPT-5.6
Sol, with an Opus-class equivalent acceptable). Active writers are `I/high`
for the certification trace wire, `M/high` for deterministic inventory/tooling,
and `I/xhigh` for `fn64-render-ir`. Their writable paths are exclusive;
`crates/fn64-abi/` remains a single-writer path. The lead alone reviews shared
contracts, native observers, shell integration, identity derivation, parity,
and performance conclusions. See
[`RT64-PORT-ORCHESTRATION.md`](RT64-PORT-ORCHESTRATION.md).
Claude Sonnet is an approved `I/high` or `P/high` option for future bounded
implementation and independent-review lanes when the session's orchestration
interface exposes it; no Claude worker is active in this session's currently
advertised GPT-only collaborator pool.

**Next action:** independently review and integrate the closed M0.3 schema and
checker, then implement its raw-sample ABI and shell emitter as a distinct
ticket before authoring a native runner. Renderer implementation proceeds in
parallel; do not label RT64 GUI timer rings as the new spans.

**Worktree note:** at plan creation, unrelated shell/frontend/example changes
were already present. They were not modified by this planning work and remain
outside the renderer-port scope.

## Non-negotiable program constraints

- Keep `ReferenceBackend` as a pure-Rust, GPU-free behavioral oracle. It is not
  retired when the new renderer works.
- Keep `Rt64Backend` available as a pinned A/B oracle until M10 cutover clears.
- Treat "matches RT64" and "correctly diverges from a known RT64 defect" as
  different evidence claims.
- Preserve content-addressed microcode admission, ordered self-load
  generations, persistent DMEM/IMEM, exact FullSync evidence, and loud traps
  for unsupported behavior.
- Do not retain borrowed RDRAM on another thread. Asynchrony requires owned
  workload data and explicit guest-commit tickets.
- No game content, ROM bytes, recompiled-game output, or private trace payload
  enters git.
- Derived RT64 modules retain their MIT provenance and notices. Fn64's
  MIT/Apache project license does not erase an upstream MIT-only obligation.
- Behavior changes update their docs in the same commit and run
  `scripts/lint-docs.py`.
- Deterministic fixes require 10 consecutive clean runs. Concurrency,
  scheduling, and coherence fixes require at least 20, with the exact closed
  interleaving stated at the fix site.

## Scope and authorities

Full parity means both phases in `docs/RT64-GAP-REGISTER.md` section D:

1. A hardware-faithful base renderer: RDP state and commands, TMEM, raster,
   framebuffer ownership/readback/writeback/reinterpretation, RSP/GBI, and VI.
2. Every available public RT64 behavior: higher resolution/downsample,
   widescreen, latency modes, HFR interpolation, Extended GBI, texture
   replacement and streaming, debugger/history, render-to-RAM, and the three
   native host APIs.

The generated public inventory currently records 19 closed and 7 open claims
for the native RT64 lane in `docs/RT64-PUBLIC-FEATURE-INVENTORY.md`. It is the
feature denominator, not proof that the Rust backend inherits those closures.
Every row must be re-earned.

Path tracing, scripting, model replacement, and emulator-plugin integration are
not available behavior in the public RT64 source at the reviewed pin. They are
post-parity fn64 extensions, not items that may substitute for an open parity
row. Ray and path tracing have their own M12 contract because hybrid effects
are broadly applicable to semantic 3D workloads while full path tracing needs
material, light, object, and environment information that arbitrary raw RDP
commands do not carry. Rice-compatible runtime lookup is in scope; copying the
separate GPL Rice hasher implementation is not.

Use three explicitly labeled authorities:

| claim | authority |
|---|---|
| preserve intentional RT64 feature behavior | frozen RT64 plus fn64 overlays |
| establish console behavior | Rust reference lane, public manuals/hardware evidence, admitted fixtures |
| establish ABI and task handoff | `cargo nextest run -p fn64-abi` and shared renderer contract tests |
| establish performance | matched A/B workload, settings, adapter, driver, image quality, and buffering |

`scripts/lane-parity.sh` is subject to its callable-body authority precondition.
Its observation mode is not an accuracy oracle. Fn64 has no reference-runtime
savestate-transplant differential; no milestone may cite one.

## Why the current boundary must change

The existing `RenderBackend` is a good one-way isolation seam, but its task and
raw-RDP calls lend the complete mutable RDRAM image synchronously. The RT64
adapter snapshots guest memory for task rollback; captured raw commands copy
the physical image into a synthetic 24-bit image and copy it back; presentation
must drain work before the borrowed pointer expires. See
`crates/fn64-render/src/lib.rs`,
`crates/fn64-render-rt64/src/transaction.rs`, and
`crates/fn64-abi/src/task_dispatch/rsp_commit.rs`.

The native RT64 surface is also not the shell's visible surface. The shell
currently rereads the native RDRAM framebuffer, converts RGBA5551 on the CPU,
and uploads it into a second wgpu presentation stack. Porting raster code while
preserving that topology would retain redundant synchronization, conversion,
upload, and prevent the visible window from being the authoritative enhanced
post-VI output.

The seam may evolve additively, but the final hot path must not be constrained
to synchronous whole-memory borrowing.

## Target architecture

```text
fn64-render
  lifecycle, admission, settings, presentation, compatibility trait
          |
          v
fn64-render-ir
  typed addresses and state, immutable workloads, resource journals
       |                                      |
       v                                      v
fn64-render-reference                   fn64-render-wgpu
software oracle                         frontend + render owner
                                                   |
                         +-------------------------+------------------+
                         v                         v                  v
                    GPU pass graph          coherence manager   compositor
                    and pipelines           and guest commits   and surface
```

The proposed crate names describe the target and do not exist until their
creation milestone lands. `fn64-render-ir` owns only stable, GPU-independent
semantics: physical address/range newtypes, admission identities, RSP/RDP/TMEM/
framebuffer/VI values, ordered operations, and immutable workload packets.
It must not contain wgpu handles, host presentation policy, or replacement-pack
I/O.

Responsibility is a dependency rule, not merely a directory convention:

| owner | authoritative responsibility | forbidden dependency/policy |
|---|---|---|
| `fn64-abi` | committed guest task/raw-DPC events, exact FullSync order, bounded guest-memory access epochs, VI lifecycle | RT64 types, GPU handles, shaders, TMEM/framebuffer rendering policy |
| `fn64-render-ir` | immutable renderer-neutral values, journals, replay identities, move-only work/ticket states | emulator scheduling, mutable guest ownership, backend/API choices, presentation policy |
| renderer frontend/backend | command interpretation, TMEM/framebuffer/RSP/RDP mechanisms, resource and GPU execution policy | guest scheduler authority or direct mutation outside an IR-declared commit |
| shell/certification | backend selection, surface orchestration, workload/environment identity, evidence publication | semantic fabrication, renderer internals, or correctness events inferred after the fact |

Any slice crossing one of these rows must expose the smallest typed value at
the owner boundary. It must not move the upstream owner's policy into the
downstream crate for convenience. Review rejects reverse dependencies and
backend-specific fields in `fn64-abi` or `fn64-render-ir` even when they would
make a single vertical slice shorter.

The steady-state dataflow is:

```text
borrow guest memory briefly
  -> exact admission and bounded decode
  -> immutable WorkloadPacket plus read/write journal
  -> bounded SPSC submission queue
  -> renderer-owned frame and upload arena
  -> explicitly ordered GPU passes
  -> completion ticket
  -> bounded guest commit only when visibility requires it
  -> VI/postprocess/direct surface presentation
```

Move-only states should make `Decoded -> Submitted -> GpuComplete ->
GuestCommitted` ownership unrepresentable out of order. Initially preserve
synchronous guest-visible FullSync writeback. Range states such as `CpuDirty`,
`GpuDirty`, and `SharedClean` arrive only in M9 after every CPU and DMA read
path has an audited observation mechanism.

### Contracts required before the renderer grows

- An owned workload packet containing exactly the bytes needed after the
  RDRAM borrow ends.
- A separate immutable raw command stream for DRAM and XBUS, with temporal
  chunk, `CMD_END`, interrupt, and FullSync identity.
- Submission, GPU-completion, and guest-commit tickets tied to exact effects.
- A direct presentation resource with workload/present provenance and overlay
  composition on one GPU device.
- A typed control channel covering user, enhancement, emulator, and
  replacement policy after backend registration.
- Explicit resource access declarations for framebuffer transitions, copies,
  reinterpretation, VI, capture, and readback.
- Bounded two-frame low-latency and three-frame throughput queues. No unbounded
  work or texture channels.

## Milestone ledger

| ID | milestone | state | minimum orchestration profile | exit headline |
|---|---|---|---|---|
| M0 | authority, evidence, and baseline | **IN PROGRESS** | `F/xhigh`; `M`/`P` only behind a deterministic oracle | immutable port manifest and matched correctness/performance baseline |
| M1 | semantic IR and renderer seam v2 | **IN PROGRESS** | `F/xhigh`; `P` contract/fixture audits | owned packets/tickets work through the reference and compatibility paths |
| M2 | cross-backend wgpu feasibility | **IN PROGRESS** | `F/high`; three independent `P/high` probes | hard GPU requirements proven or a bounded fallback decision recorded |
| M3 | raw-DPC vertical slice | **IN PROGRESS** | `F/xhigh`; isolated `I/high` modules | real LLE commands reach native target, FullSync, VI, and visible surface |
| M4 | base RDP and framebuffer correctness | PLANNED | `F/xhigh`; isolated `I/high` opcode families | full native RDP/framebuffer matrix closes |
| M5 | GBI and deferred RSP | PLANNED | `F/xhigh`; isolated `I/high` ucode families | admitted HLE families match their declared authorities |
| M6 | allocation-free asynchronous performance spine | PLANNED | `F/xhigh`; isolated `I/high` modules and `M` receipts | hot-path structural budgets hold and matched A/B is faster |
| M7 | base-renderer certification | PLANNED | `F/xhigh`; `P` analysis and `M` receipt checks | behavior matrix and private full-ROM gates close without broadened admission |
| M8 | complete RT64 feature parity | PLANNED | `F/high` per family; `I/high` implementation lanes | every required public feature-inventory row closes for Rust |
| M9 | typed CPU/GPU coherence optimization | PLANNED | `F/xhigh`; isolated `I/high` observer/fuzz work | deferred bounded readback is proven for all guest observers |
| M10 | platform certification and cutover | PLANNED | `F/xhigh`; `P` platform and `M` packaging lanes | Rust is default; C++ is certification-only, then removable |
| M11 | post-parity modernization | PLANNED | `F/high`; `I/high` feature lanes | extensions ship without weakening reference behavior |
| M12 | hybrid ray tracing and authored path tracing | PLANNED | `F/xhigh`; `P/high` research and isolated `I/high` paths | capable 3D workloads gain optional traced lighting with exact raster fallback |

## Milestone contracts

### M0 -- authority, evidence, and baseline

**Goal:** freeze exactly what is being ported and how improvement is measured.

**Deliverables:** an exact RT64/source-overlay/dependency/license manifest; a
decision on the nine-commit upstream delta; an inventory mapping every fn64
overlay to a Rust milestone and evidence row; a renderer trace schema; matched
RT64 CPU/GPU timing, allocation, copy/upload/readback, queue, shader, memory,
and presentation baselines.

**Exit gate:** a clean checkout can reconstruct the same authority identity;
the feature inventory and gap register name that identity; at least five
independent unprofiled performance repetitions establish distributions on the
declared reference route; instrumentation overhead is separately measured; all
private inputs are identified by digest but absent from git.

### M1 -- semantic IR and renderer seam v2

**Goal:** make asynchronous ownership and exact effects expressible before GPU
implementation decisions leak upward.

**Deliverables:** typed semantic values; immutable workload and raw-command
packets; read/write journals; submission/completion/commit tickets; direct
presentation and complete controls; an adapter preserving the existing atomic
trait; shared fixtures consumable by reference, RT64 capture, and future wgpu.

**Exit gate:** ABI tests stay green; packets retain no guest borrow; unsupported
states trap with identity and context; exact FullSync/interrupt ordering is
tested; a packet can be recorded and deterministically replayed through the
reference path; no GPU dependency enters the semantic crate.

### M2 -- cross-backend wgpu feasibility

**Goal:** retire existential GPU risks before committing the full port.

**Deliverables:** focused Metal, Vulkan, and D3D12 probes for integer shader
semantics, dual-source blending or exact fallback, texture-array/nonuniform
access, required formats/layouts, TMEM compute decode, framebuffer readback and
aliasing, direct surface composition, timestamps, and always-ready ubershaders.

**Exit gate:** each hard requirement is `portable`, `bounded native fallback`,
or `blocked`, with captures and adapter/driver identity. A blocker produces a
design decision before M3. WebGPU is a separate capability tier and cannot
weaken native correctness.

### M3 -- raw-DPC vertical slice

**Goal:** render production-relevant LLE work without the synthetic whole-RDRAM
command image or second presentation stack.

**Deliverables:** persistent raw RDP/TMEM state; owned command buffers; native
color/depth targets; exact FullSync; minimal VI; headless capture; direct shell
surface and overlay composition.

**Exit gate:** real captured command workloads replay deterministically through
reference/RT64/Rust observations; native framebuffer and post-VI hashes satisfy
the declared authority; ordinary presentation performs no CPU RGBA conversion
or GPU readback; no full-RDRAM staging copy occurs on the normal route.

M3 is dispatched as dependency-safe vertical slices, not one renderer-sized
branch:

| slice | exclusive outcome | writable owner |
|---|---|---|
| M3.1 | create `fn64-render-wgpu`, consume the merged render IR and exact submission completion, prewarm a bounded headless wgpu device, and execute one receipt-bearing fill/FullSync fixture without ABI or shell policy | crate root plus `src/device`, `src/lifecycle` |
| M3.2 | **INTEGRATED:** consume one submission ticket to decode the first bounded raw-DPC command subset into a transaction-local typed RDP state delta and exact resource plan; preserve M3.1 wire identity and trap unsupported/state-invalid input with full source identity; no durable state, upload, GPU, ABI, VI, parity, or performance claim | `src/raw_dpc`, `src/state`, decoder fixtures |
| M3.3 | own native color/depth resources, exact shader-compute coverage, bounded guest writeback, minimal VI, and headless capture for the first real captured workload | `src/targets`, `src/raster`, `src/vi`, shaders |
| M3.4 | replace the shell's CPU-RGBA/pixels path with direct surface and overlay composition, including resize/loss handling, while leaving ABI scheduling and guest-memory authority unchanged | shell/backend integration paths, serialized after M3.3 |

M3.1 may use one small reviewed WGSL fixture so shader qualification does not
idle the spine. M3.3's admitted RT64 shader corpus, and every broader M4 shader
claim, require M2.5's complete source/tool/artifact receipts. Synthetic success
advances mechanism
only; the milestone remains open until M3.3 replays a captured workload and
M3.4 removes the second presentation stack.

### M4 -- base RDP and framebuffer correctness

**Goal:** close the native-resolution console pixel and memory contract.

**Deliverables:** triangle, rectangle, fill, and copy cycles; combiner, blender,
coverage, depth, dither, TLUT, filtering, tile/TMEM semantics; framebuffer
identity, overlap, copy, reinterpretation, hidden bits, and native writeback;
resumable raw chunks and bounded guest commits. M4.0 first establishes the
two-phase deferred guest-read contract: a renderer-selected exact
`TmemLoadSource` plan, ABI-owned bounded logical-byte capture, and packet/
record/replay identity binding without a full-RDRAM snapshot.

**Exit gate:** the base RDP and framebuffer matrices close with exact or
explicitly bounded evidence; known RT64 divergences have separate hardware/
reference fixtures; no unsupported opcode or state silently continues.

### M5 -- GBI and deferred RSP

**Goal:** move admitted graphics tasks from LLE fallback to the typed Rust
frontend without broadening what fn64 claims to understand.

**Deliverables:** F3DEX2 first; F3DZEX2 only after its outstanding point-light
contract is characterized; S2DEX/S2DEX2; remaining required F3D/F3DEX/L3DEX
families; matrices, lighting, clipping, texgen, vertex modification, and
ordered self-load generation behavior; measured CPU-versus-GPU RSP choice.

**Exit gate:** content-addressed admission is unchanged or narrower; every
family has decode/state/geometry differentials and task-level fixtures;
unknown microcode and opcodes return a typed `NeedsLle` or loud trap rather
than partial rendering.

### M6 -- allocation-free asynchronous performance spine

**Goal:** make structural efficiency the default before micro-optimizing
kernels.

**Deliverables:** one renderer owner thread; bounded SPSC work; two/three frame
arenas; coalesced incremental uploads; GPU-resident framebuffer sampling;
explicit pass ordering; ready ubershader family; bounded background pipeline
specialization and warm manifests; bounded texture staging and residency;
per-stage telemetry.

**Exit gate:** after warm-up, task preparation, submission, render, and present
perform zero steady-state heap allocation and zero critical-thread pipeline
creation; ordinary present has zero readback; full-RDRAM copy/upload counters
remain zero; queues stay bounded; an apples-to-apples A/B improves the target
distribution without changing quality or latency policy.

### M7 -- base-renderer certification

**Goal:** turn module-level correctness into an honest renderer claim.

**Deliverables:** complete base behavior matrix, OoT eye gates, synthetic and
private full-ROM traces, exact guest-memory side-effect comparisons, device
loss/resize recovery for the reference platform, and release evidence schema.

**Exit gate:** every base row is exact or carries an owner-approved bounded
qualification; deterministic gates meet 10 consecutive clean runs and all
concurrency gates meet 20+; no visual-only observation is promoted to a memory
or timing claim.

### M8 -- complete RT64 feature parity

**Goal:** re-earn the full available RT64 feature denominator in Rust.

**Deliverables:** higher resolution/downsample; arbitrary aspect and 2D
correction; Console/SkipBuffering/PresentEarly policies; HFR workload matching
and interpolation; Extended GBI; DDS and compatible replacement lookup;
bounded asynchronous streaming and VRAM budgets; debugger/history/replay;
native render-to-RAM; full live/recreate settings behavior.

**Exit gate:** every required row in the public inventory has Rust-specific
evidence; asynchronous texture and shader work never stalls the critical path;
temporal matching is structurally compared by IDs, transforms, rejection
reasons, and weights rather than final pixels alone.

### M9 -- typed CPU/GPU coherence optimization

**Goal:** remove conservative FullSync writeback only after every observer can
participate safely.

**Deliverables:** audited CPU, DMA, VI, and renderer read/write observation;
typed range epochs; CPU invalidation of overlapping GPU targets; GPU-to-GPU
framebuffer sampling; bounded readback on a guest read of GPU-dirty memory;
fence-tied resource reclamation.

**Exit gate:** no generated, DMA, or presentation read bypasses coherence;
randomized overlapping-access differentials pass; the exact race interleavings
are documented; 20+ clean concurrency runs pass; measurement proves the added
complexity improves the target route.

### M10 -- platform certification and cutover

**Goal:** make the Rust renderer the shipping default without turning API
initialization into a false support claim.

**Deliverables:** Metal/macOS, Vulkan/Linux, and Vulkan-or-D3D12/Windows 10/11
certification across declared vendor classes; resize, suspend/resume, device
loss, presentation-mode, and adapter-change gates; release packaging without a
C++ toolchain or external RT64 checkout.

**Exit gate:** all required platform rows close; the Rust backend is the shell
default for one release cycle while RT64 remains an optional oracle; rollback
criteria are recorded; only then may the RT64 build dependency and ABI shim be
removed.

### M11 -- post-parity modernization

**Goal:** add modern features without contaminating the reference contract.

**Candidates:** HDR/scRGB, VRR-aware pacing, dynamic resolution, motion vectors,
TAA and spatial/temporal upscaling, optional optical-flow interpolation,
transactional pack hot reload, WebGPU, versioned material/model replacement,
and deterministic replay tooling. The stable scene identities, motion data,
materials, and GPU residency built here are prerequisites for M12.

**Exit gate:** each feature is explicitly non-authoritative where appropriate,
can be disabled to recover reference output, has bounded resource behavior,
and cannot substitute for a regression in M0-M10 evidence.

### M12 -- hybrid ray tracing and authored path tracing

**Goal:** add modern traced lighting where the workload exposes enough scene
meaning, without pretending that every N64 command stream contains a world
model or making ray tracing part of emulation correctness.

The capability ladder is explicit:

1. **Universal raster:** every supported workload retains the faithful M0-M10
   renderer and can recover its exact reference output with tracing disabled.
2. **Automatic hybrid:** admitted semantic 3D workloads may add ray-traced
   shadows, ambient occlusion, and selected reflections from decoded geometry,
   transforms, lights, stable draw identities, and inferred material classes.
3. **Annotated hybrid:** Extended GBI or enhancement metadata may supply object
   identity, physical materials, emissive surfaces, reflection policy, and
   off-screen/environment geometry.
4. **Authored path tracing:** title-specific packs may define the complete
   material, light, environment, and temporal contracts needed for global
   illumination and progressive path-traced presentation.

Sprite-heavy, pre-rendered-background, framebuffer-effect-heavy, and unknown-
microcode games remain valid raster workloads. Raw RDP triangles are sufficient
for faithful rasterization but generally lack the model-space transforms,
object continuity, lights, and material intent needed for automatic world-space
tracing. The renderer must never invent those semantics and then claim general
support.

**Deliverables:** capability-gated acceleration-structure ownership; stable
mesh/instance/material/light identities derived from the semantic IR; cached
static BLAS and bounded dynamic BLAS/TLAS updates; hybrid shadows/AO/reflections
first; motion-aware denoising and temporal accumulation; replacement-pack and
Extended-GBI material/light metadata; an authored full-path mode; per-effect
quality, memory, build-time, and frame-time budgets.

At plan review on 2026-08-15, the official
[wgpu ray-tracing specification](https://github.com/gfx-rs/wgpu/blob/trunk/docs/api-specs/ray_tracing.md)
marked its public ray-query and ray-tracing pipeline surface experimental. M12
must re-audit the then-current wgpu API and Metal/Vulkan/D3D12 capability matrix
before choosing portable wgpu, a bounded native Rust path, or deferral.
Experimental support is never enabled silently on a release route.

**Exit gate:** tracing-off output re-enters and passes the applicable M7-M10
reference gates; unsupported workloads fall back loudly and predictably to
raster rather than losing effects; automatic hybrid claims have semantic scene
evidence; authored modes bind pack and ROM identities; acceleration-structure
work and denoising stay within declared CPU/GPU/VRAM budgets; deterministic and
concurrency changes meet their 10/20-run bars.

## The implementation slice loop

Every slice, whether written by one agent or several, follows the same loop:

1. **Claim:** name one outcome, owner, exact paths, authority, non-goals, and
   exit gate. Partition write ownership before parallel work begins.
2. **Baseline:** reproduce the current passing behavior or failing invariant.
   Record the exact command, identities, settings, and result before editing.
3. **Model:** put ownership, address bounds, state transition, or queue
   exclusivity in types before writing the mechanism.
4. **Implement:** land the smallest end-to-end change that can satisfy the
   gate. Do not hide a missing feature behind a guard, no-op, or fallback.
5. **Fast loop:** run focused unit/fixture tests while editing. A fast loop is
   diagnostic, not release evidence.
6. **Differential loop:** compare semantic IR, TMEM bytes, operations,
   framebuffer effects, shader inputs, post-VI pixels, and timing identities at
   the earliest layer that can locate a divergence.
7. **Reliability loop:** run the required 10 deterministic or 20+ concurrency
   repetitions from clean processes. Record counts and failures, not only the
   final green output.
8. **Performance loop:** pre-register the expected mechanism and metric; run
   matched unprofiled A/B repetitions; measure instrumentation overhead; report
   p50/p95/p99/p99.9 and copied bytes/waits, not a context-free FPS number.
9. **Close:** update behavior docs, run `scripts/lint-docs.py`, review staged
   scope for game content, and update this status capsule and handoff.
10. **Improve:** add one bounded retrospective item to the ticket: friction,
    cause, reusable prevention, and expected time saved. Apply only a small
    checker/template/fixture/ownership improvement immediately; queue anything
    larger. Start the next slice by confirming the prior prevention is active,
    so mistakes are not rediscovered and stale process is removed rather than
    accumulated.

Each integrated slice is a small branch carrying exactly one ticket, its
dependency/base, automated gate output, clean-run counts, and an independent
GPT or Sonnet review. The lead re-runs the gate and merges to `main` only when
green; dependent worktrees rebase/sync immediately. A draft PR is optional.
Types and deterministic automation justify low ceremony, not absent review.
The current historically dirty shared tree is decomposed into clean branches
rather than committed wholesale.

Do not start the next slice with a red gate. If a gate cannot close, record the
precise frontier and mark the slice `BLOCKED`; a measured dead end is preferable
to changing the claim.

### Performance acceptance budgets

The eventual steady-state structural budgets are:

- zero full-RDRAM copies or uploads on the normal render route;
- zero ordinary-present GPU readbacks;
- zero shader/pipeline creation on critical threads after initialization;
- zero steady-state heap allocation in prepare, submit, render, and present;
- no visible shader miss because an initialized ubershader always exists;
- bounded two-frame low-latency or three-frame throughput queues; and
- explicit CPU prepare, queue, GPU, VI, present, and input-to-present
  distributions.

On a declared reference machine, the aspirational 60 Hz gate is p99 below
13.3 ms and p99.9 below 16.667 ms; 120 Hz HFR is p99.9 below 8.333 ms when the
workload permits it. These are not current claims. They become release gates
only after M0 freezes the workload, image quality, adapter, driver, buffering,
and instrumentation policy.

## Workstream and agent ownership

Four workstreams may proceed, but one integration owner controls the active
milestone:

| workstream | owns | must not own simultaneously |
|---|---|---|
| authority/evidence | pin, overlays, licenses, traces, matrices, release receipts | renderer implementation paths |
| semantic/frontend | IR, admission, GBI/RSP/RDP decode, packets, guest commits | wgpu resources and presentation |
| GPU/render | WGSL, pipelines, pass graph, targets, VI, compositor | ABI scheduling and guest-memory internals |
| integration/performance | shell wiring, benchmarks, platform gates, regression synthesis | overlapping feature implementation |

Prefer parallel agents for read-heavy audits, fixture analysis, test execution,
and non-overlapping modules. One agent writes a given crate or contract at a
time. The integration owner resolves design questions before dependent agents
encode different answers. Review agents do not silently edit a writer's paths.
The full dispatch roles, current model/effort recommendations, capability
fallbacks, path ownership, and escalation conditions are load-bearing in
[`RT64-PORT-ORCHESTRATION.md`](RT64-PORT-ORCHESTRATION.md); neither a cheaper
delegate nor an implementation writer may close a parity or speed claim.

## Decision log

Accepted decisions:

- **D1:** port RT64 behavior, not its C++ architecture.
- **D2:** use wgpu as the portable default; add a native Rust escape hatch only
  after M2 proves a material correctness or performance limitation.
- **D3:** add a GPU-independent immutable semantic IR shared by fixtures and
  renderer implementations.
- **D4:** make raw DPC the first production vertical slice because fn64 already
  has an accuracy LLE RSP path and current OoT rendering relies heavily on it.
- **D5:** preserve RT64 and the reference backend as distinct authorities until
  cutover.
- **D6:** converge on one GPU device/surface/compositor path.
- **D7:** retain conservative FullSync writeback until M9 proves complete
  read-observation and coherence.
- **D8:** keep an initialized ubershader on the no-stall path; specialization is
  asynchronous and optional for correctness.
- **D9:** treat hybrid ray tracing as broadly available only when semantic 3D
  scene data exists; keep universal raster fallback and require authored
  metadata for full path tracing.
- **D10:** use a dual-pin authority: `f0728a2` remains the qualified executable
  oracle while `5473732` is the accepted Rust-port source until its explicitly
  listed native requalification gates close.

Pending decisions, owned by their milestones:

- **P2 / M2:** is portable wgpu sufficient for exact blending, descriptor
  access, format/layout, timing, and presentation requirements?
- **P3 / M4:** hardware raster only, or a later tile-compute exact-raster
  experiment?
- **P4 / M5:** which RSP work is measurably better on GPU than CPU?
- **P5 / M9:** what runtime/codegen mechanism observes every CPU and DMA read of
  GPU-owned ranges?
- **P6 / M12:** is the then-current portable wgpu ray-tracing surface mature
  across Metal, Vulkan, and D3D12, or does tracing need a bounded native Rust
  capability path?

## Immediate slice queue

The accelerated wave keeps dependency-safe work active in parallel:

1. **M0.1 -- freeze the authority manifest (COMPLETE).** Reconciled the nine
   upstream commits and all exact-source overlays, repeated the dependency and
   license audit, and recorded the dual-pin decision in a checked manifest.
2. **M0.2 -- define the trace and benchmark schema (COMPLETE).** Includes semantic events,
   memory effects, GPU timestamps, allocations, bytes, waits, shader keys,
   queue depth, VRAM, present provenance, and all environment identities.
3. **M0.3 -- capture the matched baseline (IN PROGRESS).** The unintegrated
   schema/checker mechanism is under review. Raw snapshot, rebase, emitter,
   native runner, presentation-capable route, and all matched measurements are
   absent. Implement those seams before running five independent unprofiled
   repetitions plus counterbalanced instrumented controls. No matched baseline
   exists.
4. **A0.1 -- generate the source/task denominator (INTEGRATED).** The dual-pin
   inventory covers 276 admitted files / 48.065 KLOC and every authority gate.
5. **A0.2 -- workflow dashboard (INTEGRATED).** The strict canonical
   ticket ledger and deterministic terminal/Markdown/HTML renderer passed its
   final 50-test suite in 10/10 clean processes.
6. **A0.3 -- backend-neutral parity ladder (BLOCKED).** The exact all-pending
   24+26 denominator and Rust-owned fail-closed verifier are implemented; a
   display-independent RT64-produced observable is still required before any
   RT64 row can be promoted to pass or divergence.
   **A0.4 is READY:** qualify `feature::deferred-frame-history` first using
   RT64's retained completed-Workload snapshot on a controlled hidden Metal
   surface. Ignore fn64 preflight FullSync, perform no present/VI call, and
   bind the exact oracle runner/source/build plus actual RDRAM effects across
   ten checker-owned fresh processes.
7. **M1.1 -- land the seam-v2 ownership spine (INTEGRATED).** The bounded,
   GPU-independent types and records are merged; this does not place runtime
   authorities or execute a backend.
8. **M1.2 -- place and exercise IR authorities (INTEGRATED).** The synthetic
   DRAM/RDRAM slice now carries a validated capture through distinct queue,
   disposable reference-backend effect, and ABI guest-commit owners. It does
   not yet migrate production dispatch or receipt persistent/XBUS/TMEM state.
9. **M2.1 -- pin the Metal capability baseline (INTEGRATED).** wgpu 30 on the
   M5 Pro advertised the candidate hard set and passed exact GPU copy/readback;
   advertisements are not semantic proof.
10. **M2.2 -- execute Metal semantics/formats (INTEGRATED).** Exact host runs
    cover integer and TMEM-like operations, binding-array indexing,
    fractional dual-source/manual blend, explicit format conversion/I/O, and
    invalid reinterpret rejection.
11. **M2.3 -- execute Metal submission/coverage (INTEGRATED).** Exact host
    receipts prove single-queue completion, timestamp validity after
    application pipeline prewarm, and the shader-compute eight-sample mask
    primitive. Hardware MSAA stays a separately labeled, non-authoritative
    enhancement, and full RDP coverage remains M4 work.
12. **M2.4 -- qualify the HLSL artifact mechanism (INTEGRATED,
    `2458be28`).** Use an isolated,
    pinned [official DXC source](https://github.com/microsoft/DirectXShaderCompiler)
    build and its documented
    [SPIR-V path](https://github.com/microsoft/DirectXShaderCompiler/blob/main/docs/SPIR-V.rst)
    to define the checked 60-call/56-SPIR-V denominator, source/license/build
    receipts, immutable shader snapshot, and strict standalone wgpu validator.
    The mechanism passed its 10/10 reliability bar and independent review. No
    complete official DXC build or 56-artifact corpus was produced, so this
    integration closes mechanism review only.
13. **M3.1 -- build the first raw-DPC replay spine (INTEGRATED).** Drive the
    smallest decode-to-headless-frame path before broad opcode or
    feature-family ports, consuming the accepted M1.2 lifecycle and M2.3
    submission evidence.
14. **M3.2 -- decode bounded raw-DPC state (INTEGRATED, `92ea1d2d`).** The
    transaction-local decoder consumes one submission ticket and admits the
    bounded no-op, fill-cycle other-mode, color-image, fill-color, rectangle,
    and FullSync subset. It enforces exact queue/transaction succession,
    preserves M3.1's exact eight-word fixture, and produces an exact resource
    plan plus staged state delta. Its 30-test CPU suite passed 10/10; it makes
    no GPU, parity, ABI, TMEM, persistent-target, VI/surface, or performance
    claim.
15. **M2.5 -- split the official shader corpus gate (BLOCKED UMBRELLA; CHILDREN RUNNING/READY).**
    The complete official DXC and wgpu-validator builds succeeded, but the
    first artifact failed closed because RT64's required nonuniform descriptor
    indexing emits `ShaderNonUniform`, which Naga 30's strict SPIR-V frontend
    does not support. Weakening validation or stripping the decoration would
    lose semantics. M2.5.1 therefore owns the DXC-plus-`spirv-val`
    reference-valid corpus and capability inventory; M2.5.2 owns an exact
    typed wgpu-ingestion assessment; M2.5.3 owns the separately checked
    runtime corpus, using owned WGSL/Naga IR and bounded feature fallbacks
    where reference SPIR-V is not ingestible. None of these claims substitutes
    for another. M2.5.1's additive receipt/validator mechanism is integrated
    at `8765d9b0` after independent review and a 10/10 hostile gate. The exact
    v3 `spirv-val` build/smoke and all 56 reference rows are now independently
    verified; M2.5.2 still must grade wgpu ingestion, and this reference-valid
    result makes no adapter, pipeline, runtime, parity, or performance claim.
16. **M4.0 -- own deferred guest reads (INTEGRATED, `d8c0d4b1`; 10/10 VALIDATED).** The
    renderer preflights an exact ordered plan from RDRAM `TmemLoadSource`
    journal operations; the ABI captures only those ranges in N64 logical byte
    order; packet finalization and content-silent v3 record/replay bind the
    exact read-set identity. The synthetic cross-crate proof retains no RDRAM
    pointer or borrow and performs no full-memory snapshot. This does not yet
    migrate production dispatch, populate TMEM, upload to a GPU, or advance a
    parity/performance row. Record v3 is an intentional wire break: v2 magic,
    version, workload identity, and integrity-domain records are rejected, and
    the cross-language replay golden is rebound to the v3 encoding. Retained
    v2 evidence requires its pinned v2 verifier. On base `e150ff70`, the full
    `fn64-render-ir`/`fn64-render`/`fn64-abi` library suites (541 passed, 7
    ignored), four move-only doctests, and the cross-language replay golden
    passed together in 10/10 consecutive runs after independent rereview.
17. **M4.1 -- decode typed texture/TMEM wire state (INTEGRATED, `71af4c96`).** Decode and
    transactionally stage `SetTextureImage`, `SetTile`, `SetTileSize`,
    `LoadSync`, `LoadBlock`, `LoadTile`, and `LoadTLUT` with exact command and
    source identities plus M4.0 `TmemLoadSource` read plans. This slice does
    not move TMEM bytes, decode textures, upload to a GPU, migrate production
    dispatch, or claim parity/performance. Independent review closed address
    width, provenance, cross-layout latch, detached-plan, LoadBlock-bound, and
    TLUT-admission defects; the 95-unit/nine-doctest suite passed 10/10.
18. **M4.2 -- execute physical TMEM loads (RUNNING; M4.2.0 INTEGRATED, `75e9025f`).**
    Exact source and canonical physical-destination journal fragments, full
    64-bit transfer-word validity, and starting-row fixtures are frozen. The
    active M4.2a lane now owns a 4 KiB typed byte/validity/epoch state engine;
    after its review, parallel `LoadTile` and
    `LoadBlock` executors, and one frontier integration that consumes M4.0
    reads, receipts every device-local effect, and publishes durable state only
    after guest commit. Public hardware authority overrides RT64's known
    source-size, starting-row, invalid-command, and host-layout shortcuts.
19. **M4.3 -- load TLUT and decode committed textures (RUNNING; M4.3.2,
    M4.3.3a, M4.3.3b, M4.3.3c, and M4.3.3d INTEGRATED,
    `aec0ae1b`/`1f0a3213`/`fcd2b16a`/`94e16f4e`/`b07fc375`;
    M4.3.3e THIS CHANGE).**
    Keep load semantics, physical TMEM, packed texture decode, sampling, and
    cache identity separate. `LoadTLUT` writes quadricated 16-bit entries into
    high-half TMEM; the CPU oracle and owned WGSL then cover RGBA16/32,
    CI4/CI8 plus RGBA16/IA16 palettes, IA4/8/16, and I4/8 only after complete
    validity-footprint checks. YUV remains blocked on its distinct
    `SetConvert`/filter/combiner contract, and cache publication keeps old
    generations isolated across overlapping TMEM/TLUT writes.
    M4.3.3b is deliberately pure-value only: it decodes OtherMode high bits
    15:14 into disabled/RGBA16/IA16 modes with reserved encoding 1 rejected,
    extracts high-nibble-first CI4 values, normalizes CI4 palette+nibble and
    CI8 byte indices, aliases disabled CI to I8, and returns typed enabled-TLUT
    lookups at `0x800 + index * 8`. A caller-supplied big-endian 16-bit entry
    reuses M4.3.3a's RGBA16/IA16 conversion. This slice does not read physical
    TMEM or claim validity/epoch/generation/snapshot, footprint, addressing,
    sampling/filtering, cache, GPU, production, parity, or performance. This
    pure-value slice leaves state/generation binding and the exact
    quadricated validity footprint to the physical reader below.
    M4.3.3c adds that physical reader over durable `PhysicalTmemState` only.
    The caller supplies an already-addressed integer column/row plus explicit
    first-row parity; the reader applies line stride, wrapping, odd-row XOR4,
    packed CI4 nibble selection, big-endian direct values, and RGBA32 low/high
    bank reads without inferring parity or performing sampling. It preflights
    through the M4.3.3a/b pure decoders before any read, requires complete
    validity while allowing byte touch generations to predate the current
    durable generation, and returns the decoded color with one captured state
    identity/generation. Enabled CI4/CI8 is restricted to canonical low-half
    sources and absolute `0x800 + index * 8` lookup. The Programming Manual
    section 13.8 partial-CI8 example places indices 40..=69 at absolute TMEM
    words 296..=325 (`256 + index`), rather than rebasing index 40 onto word
    256; the committed fixture exercises that same placement. Requiring all
    eight quadricated bytes to be valid and their four big-endian 16-bit lanes
    equal is this reader's admitted conservative canonical subset, not a claim
    that partial or unequal words are unsampleable on hardware. Which lane the
    RDP samples in those cases remains deferred to a sample-lane hardware
    measurement. This slice adds no coordinate normalization,
    sampling/filtering/LOD, YUV, CI16/CI32, cache, WGSL/GPU, production
    dispatch, parity, or performance claim.
    M4.3.3d adds an allocation-free typed CPU point-addressing layer over that
    reader. An already-quantized signed S10.5 coordinate is shifted using the
    tile's four-bit encoding, reduced by the exact S10.2 low endpoint in
    integer five-fraction-bit space, floored with Euclidean division, and
    addressed clamp-before-mirror/mask; mask zero implies clamp and a required
    reversed clamp extent rejects before any physical read. Composition with
    `read_committed_texel` preserves its exact state/generation identity and
    typed failures. First-row parity remains explicit caller input: the
    reference lane's render-ULT derivation and pinned RT64 raw-TMEM shader's
    relative-row derivation do not settle load/render-tile aliasing on
    hardware. Partial/unequal TLUT banks remain rejected, and float/perspective
    coordinate conversion, copy-cycle addressing, filter selection/lane
    behavior, LOD, YUV, cache, WGSL/GPU, raster integration, production
    dispatch, parity, and performance remain outside this slice.
    M4.3.3e reuses M4.3.3d's exact integer decomposition to expose the 2x2
    cell containing a point: post-shift/post-origin five-bit fractions plus
    independently clamp/mirror/mask-addressed upper-left, lower-left,
    upper-right, and lower-right corners. An optional all-four committed
    gather delegates every corner to `read_committed_texel`, preserving
    explicit parity, per-corner TLUT resolution/errors, and equal immutable
    snapshot identities. The cell geometry follows the public Programming
    Manual sections “TF: Texture Filter” and “Sampling Overview”; Chapter
    13.7 “Texture Level of Detail” supplies the five-fraction-bit grid.
    This is not a filtered-color result: three-nearest corner selection and
    its validity footprint, diagonal/tie behavior, average triggering/output
    rounding, filter accumulator width, reciprocal quantization, copy mode,
    LOD/YUV/cache/WGSL/GPU work, and performance remain outside the slice. It
    adds no production-DPC integration; primitive, rectangle, or triangle
    decode; combiner, coverage, depth, blend, target, or VI behavior;
    derivatives, detail/sharpen, or two-cycle selection; full-ROM
    qualification; or RT64 pixel parity. It claims no visual/silicon parity.
    M2.5.3a's direct-texel WGSL mechanism and runtime shader-corpus
    documentation at `7e13b87c` remain unchanged.
    M4.3.3f ports the RDP three-nearest triangular bilerp as a pure function
    over `CommittedTextureCell`: `filter_three_nearest_committed_cell` remaps
    the cell's stored `[UpperLeft, LowerLeft, UpperRight, LowerRight]` order
    to `fn64-render-reference`'s `filter_three_nearest_s10_5` formula order
    before applying the same fixed-point arithmetic (lower-left triangle when
    `sf + tf <= 32`, otherwise upper-right; round-to-nearest, clamp to `u8`).
    The accumulator width and tie-break rule remain a preserved convention,
    not a verified hardware fact, per the reference lane's own comment. A
    same-repo Rust-to-Rust differential drives the arithmetic against the
    reference lane's literal 262,144-case sweep plus a TMEM-address-grounded
    fixture at the `sf + tf == 32` boundary. It does not select which filter
    mode applies, wire the filtered texel into the crate's pure one-cycle/
    two-cycle color combiner (that seam is not connected to this decoder's
    texel output), drive per-pixel UV/gather from a rasterizer, or claim
    RT64 pixel/visual/silicon parity or performance.
    M2.5.3b adds a second owned WGSL component, `three_nearest_filter`,
    differentially gated directly against
    `fn64-render-reference::filter_three_nearest_s10_5` over a duplicated
    262,144-case fixture (crate visibility keeps the reference function out
    of a cross-crate call); it is independent of both M4.3.3e and M4.3.3f's
    CPU port (each differentials against the same reference oracle rather
    than against each other), adds no raster integration, and remains
    `NotQualified`/`NativeUnverified`. See `crates/fn64-render-wgpu/README.md`.
20. **T0 -- production raw-DPC sealed session/authority seam, v11 interface
    freeze (INTEGRATED).** The production-dispatch migration card's first
    ticket, rebuilt to the v11 interface freeze's minimal sealed/session
    design after an independent adversarial review found the original pass
    not implementation-ready (`docs/DESIGN.md`'s "Production raw-DPC seam
    (T0)" section has the full type-level narrative). `new_raw_dpc_roles()`
    splits one lifecycle into ABI-owned `RawDpcAbiSession` (queue,
    guest-commit authority, retirement ledger) and backend-owned
    `RawDpcBackendAuthority` (paired completion authority, entering the
    registered backend at concrete construction -- there is no object-safe
    install method). `RawDpcBackendAuthority::begin_plan` *consumes* a
    `RawDpcPlanRequest` by value (so one stamped request cannot mint two
    writers) and traps before any plan field can be written if the request's
    stamp does not match. The resulting private-field `ExactRawDpcPlanWriter`
    is push-only (`push_tmem_load`/`push_state`/
    `push_command_decode_access`); its sole `finish(journal)` first proves
    the writer's accumulated access list equals `journal`'s ordered access
    list exactly (count/order/identity -- rejecting missing, extra,
    reordered, or mutated accesses, each with a dedicated hostile test), then
    derives the plan's source/journal identity from the writer's own request
    and that same journal, and returns the sealed `PlannedRawDpcSubmission`
    -- no public constructor exists for it, the neutral
    `ExactValidatedRawDpcPlan`, an owned semantic-command builder enum, or
    any bare ticket type.
    `RawDpcAbiSession::finalize_and_submit` owns queue readiness and ticket
    issuance entirely inside the session, so a `DecodedTicket`/
    `SubmittedTicket` never escapes; it returns only the sealed
    `BoundSubmittedRawDpc` -- the session still records a diagnostic
    `RawDpcRetirementHandle` in its own ledger, but does not hand a second
    copy back to the caller (same-module tests inspect the ledger directly).
    Neither `PlannedRawDpcSubmission` nor `BoundSubmittedRawDpc` exposes a
    plan-visiting getter; `BoundSubmittedRawDpc::execution_view` is the sole
    route, paired-authority-checked and statically dispatched through a
    generic `<V: RawDpcExecutionView<PV>>` parameter -- never `&mut dyn
    Visitor` -- so ABI gets identity/plan facts by borrow, never a
    plan-extraction surface it can retain.
    `BoundSubmittedRawDpc::into_backend_prepared` is the sole unseal route: it
    validates the exact paired authority queue identity before moving any
    field (a mismatch loudly traps; a rejected/dropped value exposes no
    parts), then issues the `GpuCompleteTicket` internally through the paired
    authority so an independently supplied ticket can never enter, yielding
    `BackendPreparedRawDpc` with no physical-state field of any kind --
    `stage()`/`submission()` facts only, no plan-visiting method, no
    `complete()` getter.
    **`RawDpcCoordinator<P>` replaces the earlier digest-identity design.** An
    earlier draft threaded a `RawDpcReadyPhysicalIdentity` (a bare, publicly
    constructible content digest) through every typestate and had the
    terminal publish step compare a caller-supplied copy against it.
    Independent review correctly rejected this: identity equality is not
    proof a physical mutation happened, so any caller could echo a matching
    digest back without performing it. The fix moves physical-state
    *ownership* into `fn64-render`:
    `RawDpcBackendAuthority::into_coordinator<P>(self, initial: P) ->
    RawDpcCoordinator<P>` consumes the paired authority into a coordinator
    generic over the backend's own physical state type `P` (a plain owned
    value, never a callback/trait object), double-buffered as
    `[Option<P>; 2]` plus an `active: u8` index; `physical()` always returns
    the currently-published slot.
    `RawDpcCoordinator::complete_execution(&mut self, BoundSubmittedRawDpc,
    BackendEffectReport, next_physical: P) -> Result<BackendPreparedRawDpc,
    ValidationError>` wraps `into_backend_prepared` and overwrites the
    coordinator's *inactive* slot with `next_physical` -- dropping whatever
    `P` used to live there -- entirely inside this ordinary fallible method,
    before any publication exists; a colocated test tracks a droppable `P`'s
    destructor across two calls to prove this timing directly. Two slots,
    not `mem::replace`-ing the active one: replacing active in place would
    run the old active `P`'s destructor at the exact instant a new candidate
    becomes current, which would put an arbitrary, unaudited `Drop` inside
    what must otherwise be commit's Drop-free straight line.
    `complete_execution` also records private `(queue, submission, inactive
    slot index)` metadata for the ordinal, consumed exactly once by
    `RawDpcAbiSession::seal_publication(GuestCommittedRawDpc,
    fn64_runtime::device::ReadyDpcFabricCommit<'a>) ->
    Result<ReadyRawDpcCommitCapsule<'a>, ValidationError>` -- v11's exact
    signature, no documented deviation this time -- which seals against the
    concrete T2 fabric-ready value and advances retirement to
    `FabricPrepare`. It validates only what the session alone owns
    (`committed`'s queue); full authority/submission/ready-slot validation is
    deliberately deferred to
    `RawDpcCoordinator::prepare_publication(&mut self,
    ReadyRawDpcCommitCapsule<'a>) -> ReadyPublication<'_, 'a, P>`, the one
    place backend-owned physical state actually lives: it looks up and
    consumes the private ready-slot metadata `complete_execution` recorded --
    queue, submission, and a private `Rc::clone` of the exact retirement
    slot `complete_execution` observed, checked via `Rc::ptr_eq` against the
    capsule's own retirement (the strongest of the three checks) -- traps if
    any disagree, then advances retirement to `PhysicalPrepare` and only
    then constructs a `ReadyPublication` -- by which point every check has
    already passed and the stage is already correct. That private
    retirement-slot clone lives on the coordinator's own ready-slot record,
    not on the capsule: an earlier draft exposed a public
    `ReadyRawDpcCommitCapsule::retirement_handle()` accessor so a caller
    could reap an abandoned/rejected candidate's slot itself, but that let
    any caller reach the same observation surface from outside the module;
    it is removed now that the coordinator keeps its own private clone for
    exactly that purpose. `ReadyPublication::commit(self) ->
    CommittedRawDpcOutcome` is the sole terminal step and the only method
    anywhere returning `CommittedRawDpcOutcome`: it flips `coordinator.active`
    to the already-checked slot (the first, and only, durable physical
    move), commits the fabric transition infallibly, and unconditionally
    disarms retirement as `Published` -- `commit` performs no stage advance
    of its own (retirement is already `PhysicalPrepare` by the time `commit`
    runs), so no callback, allocation, lookup, `assert`, `Result`, `stage`
    write, or `Drop` of `P` runs after the flip. `ReadyRawDpcCommitCapsule`
    itself exposes no bare public route to `Published`; a colocated
    source-shape test asserts by name that its `impl` block has zero methods
    returning `CommittedRawDpcOutcome` and that `ReadyPublication::commit` is
    the sole one in the module. Dropping an unconsumed `ReadyPublication`
    (which borrows, not owns, the coordinator, so its own `Drop` runs no
    code and never touches `active`) or the capsule it wraps cancels via the
    capsule's own inherited `Drop`: rolls back the fabric commit, records
    exactly one `Rejected` -- at `PhysicalPrepare` if `prepare_publication`
    already ran, or `FabricPrepare` if the capsule was dropped before ever
    reaching `prepare_publication`.
    The neutral `TmemLoadSemantics`/`TmemStateCommand` DTOs carry the
    complete materialized load contract T3 needs -- `RawDpcCommandLocation`,
    `TmemLoadEpoch`, exact `TmemLoadKind` geometry (Block coords/DXT,
    Tile/TLUT bounds/count), transfer layout/rows/logical+padding byte
    accounting, and source/destination identities -- so a physical executor
    never has to reread or redecode raw command bytes. That plan is bound to
    each submission's *captured* source identity, never live device state:
    a public STATUS-mode command may legitimately change XBUS selection
    while an admitted DPC transaction is still in flight, and
    `fn64-runtime`'s `commit_dpc_submission` deliberately preserves the
    pending submission's captured source rather than re-reading live XBUS;
    neither `seal_publication` nor `prepare_publication`/`commit` reads any
    live DPC/XBUS register as a validation gate, confirmed by direct
    inspection.
    `fn64-render-ir`'s `SubmissionQueue::try_ready_submission` adds the
    fallible, nonmutating ordinal-capacity check that returns
    `ReadySubmissionQueue<'_>`, followed by infallible issuance; that generic
    primitive remains available but cannot by itself produce a
    `BoundSubmittedRawDpc` -- only the session's own `finalize_and_submit`
    can. Every issued ordinal's `SubmittedRawDpcRetirement` has no `Clone`
    impl, so the exact same shared slot moves by value through
    `BoundSubmittedRawDpc` -> `BackendPreparedRawDpc` -> `GuestCommittedRawDpc`
    -> `ReadyRawDpcCommitCapsule` (proven via `Rc::ptr_eq` in a colocated
    test) -- this exact-once guarantee is scoped to submissions that entered
    through `RawDpcAbiSession`, not a universal invariant over every
    `SubmittedTicket` in the process (the legacy public
    `DecodedTicket`/`TicketAuthoritySet` queue APIs remain callable and
    intentionally untracked by this ledger, exactly as under v10). Every
    sealed wrapper exposes no owned-field getter, proven by compile-fail
    doctests and a colocated source-shape sweep that fails if
    `Any`/`TypeId`/downcast/`FnOnce`-callback machinery, `mem::forget`, or
    `ManuallyDrop` -- including a generic trait standing in for a concrete
    authority, fabric-commit, or physical-state type -- is ever added.
    `RenderBackend` has exactly four object-safe raw-DPC methods:
    `raw_dpc_ir_capability`, `plan_raw_dpc`, `execute_raw_dpc`, and
    `publish_raw_dpc(ReadyRawDpcCommitCapsule<'_>) -> CommittedRawDpcOutcome`,
    the object-safe shape unchanged from earlier drafts (only what a
    conforming backend does inside it changed). The first three have loud,
    named-error defaults; `publish_raw_dpc` has no `Result` in its signature
    (matching v11 exactly), so its default instead drops the capsule
    (cancelling the fabric commit, recording `Rejected`, never `Published`)
    and panics -- deliberately unreachable in practice, since a capsule
    cannot exist unless `execute_raw_dpc` already succeeded against a real,
    capable backend, and this keeps every existing `RenderBackend`
    implementor across the workspace unrelated to raw-DPC production
    compiling without a forced fourth override. A real raw-DPC-capable
    backend instead stores a `RawDpcCoordinator<P>` and implements
    `publish_raw_dpc` as exactly
    `self.coordinator.prepare_publication(publication).commit()`. There is
    deliberately no `install_raw_dpc_backend_authority` method, and the
    legacy `process_rdp_commands` path is unchanged. The call into any of
    these four through `dyn RenderBackend` is documented as the sole dynamic
    dispatch in the raw-DPC production path -- everything from identity
    validation through the terminal state transition, including
    `ReadyPublication::commit`'s fixed consuming body, is monomorphic Rust, a
    property that holds only because `fn64-render`/`fn64-render-wgpu` do not
    (and per this design must never) depend on `fn64-abi`, closing a proven
    reentrancy hazard through `fn64-abi`'s `with_host` gateway.
    This slice implements the complete sealed session/writer/typestate/
    coordinator/capsule chain through `publish_raw_dpc`'s object-safe shape,
    sealed against the concrete `fn64_runtime::device::ReadyDpcFabricCommit`
    T2 landed. It implements no decoder (T1) and instantiates no real
    backend's `RawDpcCoordinator<P>` with a concrete `wgpu` physical state
    type (T3, which also owns `PendingTmemTransaction::into_physical_successor`
    and proposed logical RDP state/durable before-after identity -- T0
    provides only the generic coordinator mechanism, not a backend state
    payload).
    `fn64-render`'s full unit/doctest suite and `fn64-render-ir`'s full suite
    passed together; see the T0 freeze report for the exact command lines,
    the per-item audit
    disposition, and the scoped blocker.

21. **M4.3.4 -- execute fill-cycle `FillRectangle` against a color target
    (INTEGRATED).** Closes the exact gap the M4.3.3f/T1 audit named: decoded
    RDP `FillRectangle` (opcode `0x36`) was validated by `raw_dpc::plan_fill`
    into a resource-journal entry but never executed against a target, and
    neither raw-DPC production seam (T1's `push_decoded_raw_dpc`, TMEM-only
    by frozen v11 scope) admits it. New `targets/fill.rs` adds a CPU-side
    executor -- not a GPU/wgpu compute pipeline, deliberately: it produces
    the identical `CompletedColorTargetWrite`/`DeviceColorBytes` domain the
    M3.3c GPU raster path also produces, and the two compose at that seam by
    construction, so this slice adds no new pipeline/shader surface for a
    single fill-cycle rectangle write.
    Scope is exactly `CycleType::Fill` x RGBA16/RGBA32, matching what
    `plan_fill` already validates; `Copy`/`OneCycle`/`TwoCycle` fill and
    `Index8` stay unimplemented (need the crate's pure combiner wired into
    this draw path, which it is not, or are out of the reference lane's own
    guaranteed-result contract).
    **Coordinate provenance.** `raw_dpc::plan_fill`'s `>>2` and
    `fn64-render-reference`'s `draw_fill_rectangle` `.ceil()/.floor()` are
    reconciled, not chosen-over: both extract the identical wire field
    (`(word >> 12) & 0x0fff`, confirmed at `raw_dpc/mod.rs:909-912` and
    `fn64-render-reference/src/gbi/stream.rs:1087,1313,1337`) and the same
    `/4` scale to a whole-integer-or-fractional quarter-pixel coordinate.
    `plan_fill` rejects any nonzero low two bits before its `>>2` runs, so
    that shift is exact integer division with nothing left to round;
    `.ceil()/.floor()` is the reference lane's *general* rule for the
    fractional case this slice does not admit. In the domain both lanes
    share (already-whole-pixel edges), they compute the same integer pixel
    range -- see `targets/fill.rs`'s module doc for the full derivation.
    This rests on the two in-repo sources agreeing on one identical bit
    extraction, not on a freshly re-read Programming Manual page: the exact
    section number is not independently reconfirmed here, named as a loud
    nonclaim in the module doc rather than a silent shrug.
    **Z/framebuffer bypass hazard.** `docs/BASE-RENDERER-BEHAVIOR-MATRIX.md`'s
    `rdp-command-state-order` row grades fill-cycle `G_FILLRECT`'s
    Z_CMP/Z_UPD/IM_RD bypass-hazard rejection `exact_public` -- this slice
    adds the check (`targets/fill.rs`'s `require_safe_fill_cycle_bypass`)
    rather than deferring it, porting `fn64-render-reference/src/raster/
    blend.rs:13-21`'s check (itself citing *Nintendo 64 Functions Reference*
    `gDPFillRectangle`/`gDPSetCycleType`) onto `OtherMode`'s existing wire
    words at the same bit positions.
    **Resident sub-rectangle admission.** `targets::CandidateColorTarget::
    admit_completed_initialization` previously rejected every non-full-extent
    completion outright, for both brand-new and already-resident targets
    (`TargetError::PartialResidentUpdateUnsupported`, now removed as
    unreachable). The type machinery already distinguished the two cases
    (`predecessor: Option<TargetGeneration>` on `CandidateColorTarget`); this
    slice only changes the resident branch's behavior, keeping the identical
    full-extent requirement for a brand-new target (nothing else could prove
    every byte of a target with no prior generation). A resident sub-
    rectangle write is admitted once its `DeviceColorBytes` buffer still
    covers the target's full extent -- the pre-existing byte-length check
    already enforced that shape; `execute_fill_rectangle` is the real
    producer, read-modify-writing the prior generation's full buffer with
    only the claimed rectangle's rows patched. An independent adversarial
    review caught the resulting caller-discipline gap directly: nothing
    stopped a caller from omitting the prior generation's bytes for an
    already-resident candidate, which would have silently zero-filled every
    untouched row instead of preserving it. `execute_fill_rectangle` now
    rejects `resident_bytes: None` for a `predecessor.is_some()` candidate
    with `FillExecutionError::MissingResidentBytes` rather than assuming
    zero content -- a loud trap, not the silent shrug AGENTS.md forbids.
    Fixtures: an exhaustive 65,536 x 5-seed RGBA16 differential and a full
    256-case RGBA32 differential against an inline-duplicated,
    independently-written oracle of `draw_fill_rectangle`'s fill-cycle
    branch; hand-computed unit fixtures for the 5-bit expansion and
    RGBA32 alpha/coverage-byte unpack; a real end-to-end test decoding
    genuine raw-DPC command words through `decode_raw_dpc` and executing the
    resulting `FillRectangle` against a real `CandidateColorTarget`, byte-
    exact; targeted characterization for literal single-pixel/degenerate
    rectangles, fractional-edge and reversed-coordinate rejection,
    out-of-bounds rejection, resident-byte-length mismatch rejection, and
    nonmutation on every rejection path (the resident target's bytes and
    generation are asserted unchanged after an out-of-bounds attempt).
    It does not implement Copy/OneCycle/TwoCycle fill, texture rectangles,
    triangles, a combiner, blend, coverage, or depth; it writes no GPU
    buffer and drives no wgpu pipeline (CPU-side `DeviceColorBytes` write
    only); it establishes no VI, presentation, or full-frame path; it claims
    no visual/silicon parity or performance. `targets/raster.rs`'s M3.3c
    fixture demo, T0's sealed coordinator, T1's TMEM-only production
    adapter, T3's `PendingTmemTransaction::into_physical_successor`, and the
    M4.3.3f three-nearest filter are all unchanged.

22. **T4 -- real ABI raw-DPC ingress (INTEGRATED).** Wires all three concrete
    production raw-DPC producers in `fn64-abi` -- sp_dp DRAM (`sp_dp.rs`),
    MMIO DRAM/XBUS (`pi/mmio.rs`, both sources, via the shared
    `dispatch_dpc_submission` seam), and RSP XBUS (the coalesced
    pending-submission loop inside `dispatch_lle_task`,
    `task_dispatch/rsp_commit.rs`) -- through T3 Phase B's
    `plan_raw_dpc -> finalize_and_submit -> execute_raw_dpc ->
    commit_zero_guest_writes -> seal_publication -> publish_raw_dpc`
    conveyor, conditionally on a registered `RawDpcAbiSession`
    (`fn64_abi::set_raw_dpc_session`/`clear_raw_dpc_session`, a new
    thread-local slot alongside the pre-existing `RENDER_BACKEND` one). No
    session registered -- the default, and what `Rt64Backend` always uses --
    is the byte-for-byte unchanged legacy atomic `process_rdp_commands`
    path; the routing decision is made once, before
    `LiveDpcTransaction::new` runs, at all three call sites, so a submission
    is never claimed by one path and abandoned to the other. See
    `docs/DESIGN.md`'s "T4 -- real ABI raw-DPC ingress" section (right after
    T3 Phase A/B) for the full type-level narrative, including the guest-
    read-byte-sourcing argument (live RDRAM for both DRAM and XBUS captures)
    and the `single_source_probe_journal` fix this slice made to already-
    shipped T3 Phase B code (its command-decode probe access previously
    always declared an RDRAM region regardless of submission source, which
    made `plan_raw_dpc` reject every genuinely XBUS-sourced capture).
    `fn64-abi` never depends on a concrete backend crate to do this --
    `RawDpcAbiSession` is a `fn64-render` type -- so `fn64-render-wgpu` is a
    dev-dependency only, mirroring the existing `fn64-render-reference`
    dev-dependency pattern; no shell in this workspace constructs a
    `WgpuBackend` or calls `set_raw_dpc_session` in production. Tests:
    `crates/fn64-abi/src/task_dispatch/tests/raw_dpc_session_integration.rs`
    (10 tests) drive the real producer entry points end to end against a
    real 8 MiB RDRAM allocation and a real `DeviceFabric` admission --
    including a real, hand-encoded tiny RSP interpreter program (COP0
    `mtc0` writes to DPC_START/DPC_STATUS/DPC_END) that reaches the actual
    `dispatch_lle_task` pending-loop for the RSP-XBUS producer, not a
    `dispatch_dpc_submission(Dmem)` surrogate -- covering: per-producer
    session routing and legacy fallback when no session is registered,
    FullSync rejection through the real producer seam, drop/cancel leaving
    no pending fabric transaction, joint ordinal/fabric-state publication
    across two independent submissions through the same session, exact
    XBUS source-byte preservation (submission identity SHA-256), and a
    mismatched backend/session registration (two independently constructed
    `WgpuBackend::try_new()` pairs, backend from one and session from the
    other) trapping loudly via `RawDpcBackendAuthority::begin_plan`'s own
    paired-queue assertion before any fabric mutation. Plus one new
    `fn64-render-wgpu` characterization test
    (`plan_raw_dpc_accepts_a_genuinely_xbus_sourced_capture`) proving the
    probe-journal fix directly. Scope, nonclaims unchanged from T3 Phase B:
    TMEM-only, no-FullSync, no-guest-write, headless only; no visible
    presentation, no raster/combiner/blend parity, no native GPU testing, no
    `Rt64Backend` migration (it keeps the legacy path unconditionally, since
    it never implements the three object-safe raw-DPC production methods).

### M0 evidence ledger

**M0.1 -- COMPLETE, 2026-08-15.**

- `tools/check_rt64_port_authority.py --rt64-dir <old> --port-dir <candidate>`:
  10/10 consecutive clean runs against the real clean Git trees.
- Candidate native `cargo check -p fn64-render-rt64 --features rt64`: 10/10
  consecutive runs rejected with exit 101 and the exact active-oracle identity
  error. This is the expected negative result, not a failed test run.
- Old-oracle native `cargo check -p fn64-render-rt64 --features rt64`: clean.
- `cargo nextest run -p fn64-render-rt64`: 33/33 passed.
- `scripts/lint-docs.py --verbose`: clean after generated-doc refresh.

No renderer output, timing, or platform qualification changed in M0.1. The
candidate's required native evidence remains open in the authority report.

**M0.2 -- COMPLETE, 2026-08-15.**

- `tools/check_rt64_render_measurement.py`: 10/10 consecutive clean process
  runs; every process executed 20 positive/mutation validation checks.
- `python3 -m py_compile tools/check_rt64_render_measurement.py`: clean.
- `scripts/lint-docs.py --verbose`: clean with the generated measurement
  report checked against its canonical JSON definition.
- `git diff --check`: clean.

M0.2 establishes measurement comparability, not a speed result. Its historical
10/10 receipt predates M0.3's development/comparison-ready distinction and
does not prove a raw-sample ABI, shell emitter, native runner, or baseline.
Those mechanisms are absent from the current tree. M0.3 still owns all matched
native baseline numbers and the missing instrumentation.

## End-of-session handoff template

Replace the handoff near the top with a concise instance of this template:

```text
Outcome:
  What changed, stated as an evidence-bounded result.

Active milestone and slice:
  ID, state, owner, and exact paths.

Delegation:
  Lead/reviewer profile, model/effort recommendation, delegated lanes and
  their candidate-versus-claim status, and every serialized path.

Baseline / authority:
  Source identities, command, settings, and pre-change result.

Verification:
  Commands, clean-run counts, differential result, performance A/B, and
  instrumentation caveats. Say "not verified" where appropriate.

Frontier:
  First failing invariant or next unchecked dependency; what was ruled out.

Next exact action:
  One command or edit target that advances the same slice.

Worktree:
  Relevant changes plus unrelated pre-existing changes that must be preserved.
```
