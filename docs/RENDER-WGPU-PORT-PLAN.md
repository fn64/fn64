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
| updated | 2026-08-16 |
| program state | **IN PROGRESS** |
| execution wave | **ACCEL-A -- port spine and evidence in parallel** |
| active milestones | **M0 authority/baseline, M2 GPU feasibility, and M3 raw-DPC vertical slice (all IN PROGRESS)** |
| active slices | M0.3 native baseline; A0.3 is infrastructure-ready but blocked on an RT64-produced observable; M2.4 shader-artifact qualification; M3.2 bounded raw-DPC decode/state planning |
| active ownership | `F/xhigh` integration lead; `F/xhigh` shader provenance/review; `F/xhigh` raw-DPC decoder/state lead; lower-cost fixture and repetition lanes only after typed interfaces freeze |
| last completed result | local `main` contains M1.2's cross-crate effect ownership, M2.3's executed Metal submission/coverage evidence, and M3.1's reviewed typed wgpu submission spine (`39ea1cbc`) |
| next concrete decision | accept or repair M2.4's source-build/validator mechanism, then dispatch M3.2a bounded raw-DPC decoding before TMEM, production ABI wiring, VI, or surface ownership |
| evidence blockers | no matched private-game RT64 baseline exists; public RT64 does not provide a usable tracing implementation; HLSL artifact provenance and Windows DXC/utf8conv closure remain unresolved; these block claims or platform closure, not independent port work |
| verification claim | authority/inventory, render IR, M1.2, M2.1-M2.3, and M3.1 retain their recorded bars; A0.3's all-pending evidence mechanism passed 10/10 but closes no parity row; broad RDP, VI/surface, renderer parity, and performance remain unproven |

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

**Outcome:** M0.3 is in progress on an unintegrated measurement slice. Its
proposed schema permits an honest `development` report with explicit
unavailable metrics while reserving summaries and comparisons for
`comparison_ready` reports. That slice is not an authority on `main` until its
checker, bounded JSONL trace, headless route, and baseline cohort pass review
and merge. Raw advances must retain committed VI fields and guest cycles
instead of pretending every advance is one field.

**Inventory decision:** existing fn64 CPU/frame-census observations are
usable only with their exact boundaries. The native RT64 rolling timer names
are not schema definitions: some measure inter-call cadence, some include
fence waits, and the native GPU interval excludes VI and presentation. Copy,
queue-wait, allocation, shader/PSO, full GPU-pass, total VRAM, and physical
presentation observations remain partial or missing and are the M0.3 work
queue. Missing and unarmed channels cannot appear as zero-cost work.

**Route decision:** the M0.3 benchmark route is a new headless branch in
`fn64-shell`, immediately after `Shell::boot` and before the `EventLoop`, using
`Shell::pump_one_frame`. It reuses the established RT64 device and excludes a
second `pixels`/wgpu compositor. The route owns a deterministic neutral
boot/title-v2 identity: boot-complete start, the already-admitted owned ROM,
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

**Implemented route, not yet an honest comparison:** `fn64-abi` exposes an
owned raw frame-census measurement snapshot. `fn64-certification` can build a
typed control report and an explicit-partial instrumented development report.
The RT64 adapter queries its active graphics API without present capture. The
headless shell path executes `Shell::pump_one_frame` outside the winit/pixels
compositor, and `tools/run_rt64_render_baseline.py` launches five
counterbalanced control/instrumented pairs as fresh processes. Semantic and
command-level FullSync roots now accumulate at committed ABI boundaries;
native-framebuffer and post-VI hashes come from one capture-only VI advance
after the timed horizon. Its move-only token unregisters native capture work on
take, failure, explicit cancellation, or drop. Program/build/host/GPU/display
identity is now build-issued or observed. A control run can become
`comparison_ready` only after the post-create verifier
completes and both horizon endpoint observations are available and unchanged.
The window begins only after warmup and atomically rebases wall, guest,
counter, graphics, and sample state. Instrumented runs remain `development`
until all required native
channels and trace evidence exist. No real matched baseline has been recorded.

The C-RecompiledFuncs shell build now emits a path-free receipt over the exact
generated-code and section-bridge archives, a separately domain-separated
dispatch-source closure, Cargo target/profile/features, the effective
`rustc -vV`, and fn64 Git HEAD/clean state. Those fields replace request
placeholders before publication. The content-free build emits no receipt, and
the Rust-recompiled lane fails measurement admission until its generator issues
canonical native-program and dispatch identities; hashing an arbitrary source-
tree walk would not be an honest substitute. This mechanism passed content-free
unit tests but has not been exercised with private linked game inputs in this
session.

**Verification frontier:** the full no-feature `fn64-abi` library suite passed
once after the post-warmup observation and per-call FullSync corrections (388
passed, 7 ignored). The focused renderer-measurement group passed 10/10 clean
process runs; the correctness-root and resumable-FullSync groups also passed
10/10 before the final measurement-only overflow guard. RT64 feature tests
passed earlier (67/67).
The complete 60-test shell binary suite passed 10/10 consecutive runs. The
one-shot census tests passed 20/20 consecutive runs and name the closed
two-caller erase interleaving at the transition. The current 38-case
measurement checker passed 10/10 separate processes. The eight-test runner
suite passed 10/10; every run includes a self-test that launches ten unique,
counterbalanced child processes. The forced linked-route compile passed once,
but used content-free build inputs and is syntax/integration evidence only.
This closes the current deterministic schema/runner and local route receipt,
not the private linked-build or real matched-baseline gates.

**Closed correctness review:** the post-VI digest includes renderer-owned
extension rows, observed device atoms are revalidated, and the runner requires
all five controls to form one correctness/identity cohort. The typed boot
consumes the admitted ROM bytes once, uses blank in-memory SRAM and fixed route
policy, never constructs host input/config/overlay state, and cannot fall back
to the reference renderer. Pre/post one-minute load and thermal/power
conditions are collector-observed immediately around the rebased horizon;
unavailable coarse conditions force development tier. The independent Python
validator mirrors positive-field, nonzero-graphics, classification, ordinal,
bounded atom, condition-enum, and metric-state rules. `armed_not_reached`
metrics are accepted only with zero values.

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

**Next action:** review and integrate the trace wire, generated port inventory,
and semantic-IR spine. Then dispatch disjoint raw decode/TMEM, framebuffer/VI,
and GPU capability slices. In parallel, exercise the C-program receipt, native
identity query, and v2 neutral route when a clean private linked build is
available; do not label RT64 GUI timer rings as the new spans.

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
| M3 | raw-DPC vertical slice | PLANNED | `F/xhigh`; isolated `I/high` modules | real LLE commands reach native target, FullSync, VI, and visible surface |
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
| M3.2 | decode the first admitted raw-DPC command subset into persistent typed RDP/TMEM state and upload only journal-declared ranges; unknown commands trap with stream/offset identity | `src/raw_dpc`, `src/state`, decoder fixtures |
| M3.3 | own native color/depth resources, exact shader-compute coverage, bounded guest writeback, minimal VI, and headless capture for the first real captured workload | `src/targets`, `src/raster`, `src/vi`, shaders |
| M3.4 | replace the shell's CPU-RGBA/pixels path with direct surface and overlay composition, including resize/loss handling, while leaving ABI scheduling and guest-memory authority unchanged | shell/backend integration paths, serialized after M3.3 |

M3.1 may use one small reviewed WGSL fixture so M2.4 does not idle the spine.
M3.3's admitted RT64 shader corpus, and every broader M4 shader claim, require
M2.4's source/tool/artifact receipts. Synthetic success advances mechanism
only; the milestone remains open until M3.3 replays a captured workload and
M3.4 removes the second presentation stack.

### M4 -- base RDP and framebuffer correctness

**Goal:** close the native-resolution console pixel and memory contract.

**Deliverables:** triangle, rectangle, fill, and copy cycles; combiner, blender,
coverage, depth, dither, TLUT, filtering, tile/TMEM semantics; framebuffer
identity, overlap, copy, reinterpretation, hidden bits, and native writeback;
resumable raw chunks and bounded guest commits.

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
3. **M0.3 -- capture the matched baseline (IN PROGRESS).** The route and
   snapshot/report/API-identity seams and fresh-process runner are present.
   Observation-derived correctness, workload, program/build, host, GPU, and
   display identities, typed neutral boot, exact horizon boundary, and
   post-horizon capture are present; validate them in a clean linked private
   route, then run five independent unprofiled repetitions plus
   counterbalanced instrumented controls on the declared route. No matched
   baseline exists.
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
12. **M2.4 -- qualify the HLSL artifact producer (READY).** Use an isolated,
    pinned [official DXC source](https://github.com/microsoft/DirectXShaderCompiler)
    build and its documented
    [SPIR-V path](https://github.com/microsoft/DirectXShaderCompiler/blob/main/docs/SPIR-V.rst)
    to compile the admitted RT64 HLSL corpus to checked artifacts plus
    source/tool/flags/include/output receipts. The fn64 runtime and ordinary
    build consume only accepted artifacts through wgpu; they do not acquire
    DXC, CMake, or the upstream unqualified `dxc-bin`.
13. **M3.1 -- build the first raw-DPC replay spine (INTEGRATED).** Drive the
    smallest decode-to-headless-frame path before broad opcode or
    feature-family ports, consuming the accepted M1.2 lifecycle and M2.3
    submission evidence.
14. **M3.2 -- decode bounded raw-DPC state (READY).** Replace the fixed M3.1
    fixture parser with a transaction-local decoder for no-op, fill-cycle
    other-mode, color-image, fill-color, rectangle, and FullSync commands.
    Produce an exact resource plan and staged state delta without publishing
    durable renderer state or crossing into ABI, VI, or surface policy.

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
10/10 receipt predates M0.3's corrected development/comparison-ready handling;
the amended 38-case checker subsequently passed a fresh 10/10 separate-process
receipt during M0.3. The current 60-test shell route passed 10/10, the exact
one-shot census boundary passed 20/20 for its named race interleaving, and the
eight-test runner suite passed 10/10 with ten unique child processes per run.
M0.3 still owns all matched native baseline numbers and the missing
instrumentation.

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
