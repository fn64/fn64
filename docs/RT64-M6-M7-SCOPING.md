# M6 and M7 architecture scoping

Scope: **M6 (allocation-free asynchronous performance spine)** and **M7
(base-renderer certification)** only. M9, M10, and M12 are out of scope here.

Purpose: make M6 and M7 *decidable*. Three prior planners declined to write
tickets for them, correctly — the generated per-file cards in
`docs/rt64-port-inventory.json` are literal-port cards, and neither milestone
is a literal-port problem. This document lays out what each gate demands, what
already exists, what the gap is, and **which decisions the owner must make
before a normal planner can write cards**. It decides nothing.

Base of analysis: worktree `/private/tmp/fn64-lane50`, detached HEAD
`854d2867` ("render-wgpu: port RT64 FbReinterpret format kernels"), clean tree.

Authority: pinned MIT RT64 `5473732a822a4423b5696e7cb18fecc425a59875` (port)
and `f0728a2520d5aa735886240de3fee75cc805f6d6` (oracle), per
`docs/rt64-port-authority.json`. No GPL runtime code was read or cited.

---

## Contents

- [Cross-cutting findings](#cross-cutting-findings)
- [M6 — allocation-free asynchronous performance spine](#m6--allocation-free-asynchronous-performance-spine)
- [M7 — base-renderer certification](#m7--base-renderer-certification)
- [Evidence-blocked versus decision-blocked](#evidence-blocked-versus-decision-blocked)
- [Candidate first cards](#candidate-first-cards)
- [Vague or contradictory items in the plan](#vague-or-contradictory-items-in-the-plan)

---

## Cross-cutting findings

Four facts bear on both milestones and are established once here.

### CC-1. The generated inventory cards are the wrong shape for M6

`docs/rt64-port-inventory.json` assigns 14 files (1,613 lines) to M6 and
**zero files to M7**. Milestone distribution across all 276 admitted files:

| M1 | M3 | M4 | M5 | M6 | M8 | M10 | M11 | M12 |
|---:|---:|---:|---:|---:|---:|----:|----:|----:|
| 5 | 8 | 81 | 44 | **14** | 101 | 10 | 10 | 3 |

Every M6 card generated for those 14 files has the same generated body, e.g.
for `src/render/rt64_buffer_uploader.cpp`:

> outcome: "Port the admitted behavior represented by
> src/render/rt64_buffer_uploader.cpp into an owned Rust module without
> widening behavior claims."
> writable_paths: the inventory's generated proposal for this still-unported
> source, `rt64_buffer_uploader_cpp.rs` under `crates/fn64-render-wgpu/src/`.
> Quoted as a proposal, not a live path — the file does not exist yet, and
> naming it as one would trip `scripts/lint-docs.py`'s dangling-reference
> check.
> exit_gate: "The M6 behavior fixture for src/render/rt64_buffer_uploader.cpp
> passes its declared differential and required 10/20-run reliability bar."

One Rust module per C++ file, named after the C++ file. That is exactly what
**D1** forbids ("port RT64 behavior, not its C++ architecture") for a milestone
whose subject *is* architecture: thread topology, queue depth, arena lifetime,
and allocation accounting are properties of the whole renderer, not of any one
file. A per-file differential also cannot test the M6 exit gate, which is
stated over the *aggregate* steady state ("zero steady-state heap allocation in
prepare, submit, render, and present").

This is a finding, not a criticism of the generator: the generator is a
denominator tool and says so ("Hand-maintained 'roughly complete' lists are not
authority", plan execution rule 4). But it means **the M6 inventory cards
should not be dispatched as written**, and the owner should decide whether they
are retired, re-scoped, or left as a source denominator only (decision **D-M6-1**).

### CC-2. M6 splits cleanly into a portable half and a plume-gated half

Reading the dependency edges the inventory already records:

| file | lines | dependency | portable today? |
|---|---:|---|---|
| `src/common/rt64_timer.h` | 21 | *(none)* | yes |
| `src/common/rt64_timer.cpp` | 74 | `rt64_timer.h` | yes |
| `src/common/rt64_elapsed_timer.h` | 19 | `rt64_timer.h` | yes |
| `src/common/rt64_elapsed_timer.cpp` | 29 | `rt64_elapsed_timer.h` | yes |
| `src/common/rt64_profiling_timer.h` | 35 | `rt64_elapsed_timer.h` | yes |
| `src/common/rt64_profiling_timer.cpp` | 88 | `rt64_profiling_timer.h` | yes |
| `src/render/rt64_render_worker.h` | 31 | `rt64_plume.h` | **no — RHI** |
| `src/render/rt64_render_worker.cpp` | 45 | `rt64_render_worker.h` | **no — RHI** |
| `src/render/rt64_buffer_uploader.h` | 69 | `rt64_plume.h`, `rt64_render_worker.h` | **no — RHI** |
| `src/render/rt64_buffer_uploader.cpp` | 175 | `rt64_thread.h`, `…_uploader.h` | **no — RHI** |
| `src/render/rt64_descriptor_sets.h` | 677 | `rt64_plume.h`, `rt64_sampler_library.h` | **no — RHI** |
| `src/render/rt64_shader_compiler.h` | 41 | `rt64_plume.h` | **no — RHI + DXC** |
| `src/render/rt64_shader_compiler.cpp` | 127 | `rt64_common.h`, `…_compiler.h` | **no — RHI + DXC** |
| `src/render/rt64_raster_shader_cache.cpp` | 182 | `rt64_thread.h`, `…_cache.h` | **no — authority-gated** |

**266 of 1,613 M6 lines (16.5%) are plume-free.** The remaining 1,347 lines all
route through `rt64_plume.h` — RT64's RHI abstraction, which the plan
explicitly refuses to transliterate ("do not transliterate RT64's pointer
graph, mutable `State`, thread topology, or plume RHI",
`docs/RENDER-WGPU-PORT-PLAN.md:21-23`). wgpu *is* fn64's RHI; porting plume on
top of wgpu would be two abstraction layers for one job.

Inference (basis: the dependency graph above plus the plan's plume prohibition):
**the 8 plume-gated M6 files will never be ported as files.** Their *behavior*
— coalesced uploads, a worker thread, descriptor-set shape, a raster shader
cache — must be re-derived against wgpu's own resource model. Whether the
inventory should record that as `not-started` (implying future port) or a new
state such as `superseded-by-architecture` is decision **D-M6-1**.

### CC-3. This host has a real GPU; docs claiming otherwise are stale

`docs/RENDER-WGPU-PORT-PLAN.md:47` (the live status capsule) states:

> "this environment has no GPU adapter for any backend, matching every
> pre-existing `host-gpu-tests` test's identical typed `NoAdapter` failure"

Measured on this host at HEAD `854d2867`:

```
cargo test -p fn64-render-wgpu --lib --features host-gpu-tests
test result: ok. 3312 passed; 0 failed; 3 ignored; 0 measured
```

including `targets::triangle_pipeline::tests::host_gpu_tests::
required_host_rasterizes_covering_triangle_and_matches_combiner_and_depth_oracles`
and `sealed_plan_end_to_end::required_host_draws_a_real_admitted_triangle_matching_the_combiner_oracle`
— tests that name `required_host` precisely because they fail closed without an
adapter. The capsule's own line 51 already contradicts line 47 ("all confirmed
10 consecutive clean runs against a real Metal adapter"). Line 47's claim is
stale and should not be used to scope either milestone.

**Consequence:** any M6 or M7 work the plan deferred on "no adapter" grounds is
*not* blocked. Notably, the Slice B "required physical-Metal nonzero-row
differential" named in the capsule's `next concrete decision` is runnable here.

### CC-4. `lint-docs.py` baseline is 5 on this branch, not 4

```
$ python3 scripts/lint-docs.py
lint-docs: 5 error(s)
  docs/RT64-RUNTIME-SHADER-CORPUS.md:25,27,28,29: asserts content hash … that no test checks
  RT64-PORT-INVENTORY.md: rt64-port-inventory: src/shaders/FbReinterpretCS.hlsl:
    ported_as drift from mechanical SHA-256 citation scan
```

Four are the known `RT64-RUNTIME-SHADER-CORPUS.md` hash errors. The fifth is
new at HEAD `854d2867` — the FbReinterpret port commit left the inventory's
`ported_as` field out of sync with the SHA-256 citation scan. It is pre-existing
relative to this document (verified against a clean tree with `git status`
empty) and is an inventory-regeneration fix, not a doc-authoring fix. **This
document adds zero new errors and asserts no content hash.**

---

## M6 — allocation-free asynchronous performance spine

### 1. What the exit gate actually demands

Quoted from `docs/RENDER-WGPU-PORT-PLAN.md:583-585`:

> **Exit gate:** after warm-up, task preparation, submission, render, and
> present perform zero steady-state heap allocation and zero critical-thread
> pipeline creation; ordinary present has zero readback; full-RDRAM copy/upload
> counters remain zero; queues stay bounded; an apples-to-apples A/B improves
> the target distribution without changing quality or latency policy.

Ledger headline (`:453`): "hot-path structural budgets hold and matched A/B is
faster."

Deliverables (`:575-579`): "one renderer owner thread; bounded SPSC work;
two/three frame arenas; coalesced incremental uploads; GPU-resident framebuffer
sampling; explicit pass ordering; ready ubershader family; bounded background
pipeline specialization and warm manifests; bounded texture staging and
residency; per-stage telemetry."

Restated as things that must be **true and measurable**:

| # | demand | measurable as |
|---|---|---|
| G1 | zero steady-state heap allocation in prepare/submit/render/present | a counting global allocator, or an allocation counter in the measurement report (`allocation_bytes` metric already exists in the schema) reading 0 across a post-warmup window |
| G2 | zero critical-thread pipeline creation | `shader_pso_compile` metric (already in the schema) reading 0 post-warmup |
| G3 | ordinary present has zero readback | `copy_upload_readback_bytes` reading 0 on the present route |
| G4 | full-RDRAM copy/upload counters remain zero | same counter, RDRAM-scoped |
| G5 | queues stay bounded | a declared depth (plan says two-frame low-latency / three-frame throughput, `:760`) with a test that the queue refuses rather than grows |
| G6 | matched A/B improves the target distribution | two five-repetition unprofiled cohorts, p50/p95/p99/p99.9, on one frozen route |

G1–G5 are **structural** and testable without a game. G6 is **comparative** and
needs a baseline.

The plan also names aspirational numeric budgets at `:764-768` — "p99 below
13.3 ms and p99.9 below 16.667 ms" for 60 Hz — and immediately disclaims them:
"These are not current claims. They become release gates only after M0 freezes
the workload, image quality, adapter, driver, buffering, and instrumentation
policy." They are therefore **not** part of the M6 exit gate. Do not let a card
adopt them.

### 2. What already exists

#### Landed and directly useful

- **A prewarmed pipeline at construction.**
  `crates/fn64-render-wgpu/src/targets/triangle_pipeline.rs:699-1016`:
  `UninitializedTrianglePipeline::request()` creates the shader modules
  (`:749,754`), ten fixed buffers (`:759-813`), the bind-group layout (`:819`),
  a prewarm bind group (`:954`, labelled `fn64-triangle-pipeline-prewarm-bind-group`),
  and the render pipelines (`:1016`) — all before any draw. `targets/raster.rs:3`
  makes the same claim for the raster path ("owns one prewarmed raster pipeline").
  This is real G2 groundwork: pipeline creation is already off the draw path.

- **A real measurement wire schema, mechanically checked.**
  `docs/rt64-render-measurement-schema.json` + `tools/run_rt64_render_baseline.py`
  (966 lines). Its metric denominator already contains exactly the counters
  G1–G4 need: `allocation_bytes`, `shader_pso_compile`,
  `copy_upload_readback_bytes`, `queue_wait`, `gpu_interval`, `full_gpu_pass`,
  `total_vram_bytes`, `physical_presentation`, plus `cpu_field_latency` as a
  distribution. `--selftest` passes on this host: "3 development reports
  accepted, 83 hostile cases rejected, weighted mean checked, cohort deferred,
  comparison-ready fabrication rejected, generated doc exact".

- **A bounded-capacity discipline in the IR.** `crates/fn64-render-ir/src/`
  declares hard caps: `MAX_COMMAND_CHUNKS = 4096` and
  `MAX_RAW_STREAM_BYTES = RDP_PHYSICAL_ADDRESS_BYTES` (`command.rs:17-18`),
  `MAX_RESOURCE_ACCESSES = 16_384` and `MAX_DECLARED_RESOURCE_BYTES = 256 MiB`
  (`journal.rs:7-8`), `MAX_WORKLOAD_RECORD_BYTES = 8 MiB` (`record.rs:15`).
  These are *size* caps that fail closed, which is the right reflex — but note
  they are not the *depth* bound G5 asks for (see gap W5).

- **An ordinal-capacity check that is nonmutating and fallible.**
  `SubmissionQueue::try_ready_submission` returning `ReadySubmissionQueue<'_>`
  followed by infallible issuance (plan `:1193-1197`). This is exactly the
  shape a bounded SPSC handoff wants at the boundary.

- **A frame-census distribution engine.** `crates/fn64-abi/src/frame_census.rs`
  (3,614 lines) already computes `FrameDistribution` with `wall_ms_per_field`
  (`:789`), `guest_field_hz` (`:815`), `holds_60fps` (`:783`),
  `wall_versus_virtual` (`:800`), and a `Periodicity` model (`:1126`). This is
  a working per-field latency instrument, already in the tree.

#### Landed and *counter*-useful (i.e. it establishes the gap concretely)

- **The production draw path allocates per draw and reads back
  unconditionally.** `targets/triangle_pipeline.rs`'s `submit_triangles`
  creates, **per submission**: a vertex buffer and nine uniform/storage buffers
  *per draw* (`:1375-1473`, inside `for fixture in fixtures`), then a color
  texture (`:1503`), depth texture (`:1518`), status texture (`:1533`), and
  three readback buffers (`:1549,1555,1564`); then it issues three
  `copy_texture_to_buffer` calls (`:1755,1776,1797`) and blocks on
  `device.poll(wgpu::PollType::Wait { … })` (`:1867`) plus a `map_async`
  (`:1947`) and a second blocking poll (`:1951`).

  The per-draw duplication is *deliberate and correct* for the current path —
  the code says so at `:1342-1350`: "mid-render-pass `queue.write_buffer` calls
  are not safe against a buffer already bound by an in-flight pass, so per-draw
  uniforms must be distinct resources written before the pass opens". This is
  not sloppiness; it is the absence of the frame arena M6 exists to add.

  The readback is also correct for what this path *is*: a headless-capture
  path. `RenderBackend::present` is `Err(…"presentation is out of scope")`
  (`production.rs:1200-1207`), so there is no "ordinary present" yet for G3 to
  be measured against.

- **`plan_raw_dpc` decodes the capture twice, on every submission.**
  `production.rs:1230-1250` (documented at `:1345-1362`): it builds a throwaway
  `single_source_probe_journal`, decodes once, reads the real access list off
  the `RawDpcDecodeError::JournalMismatch { expected, .. }` error path, rebuilds
  a `ResourceJournal`, then decodes again for real. The comment is candid:
  "that journal is not knowable ahead of decoding … so this mirrors T1's own
  test harness's two-pass probe". Every submission pays a full extra decode plus
  a `Vec` allocation, and the *error* path is load-bearing control flow.

- Allocation-shaped calls (`to_vec`/`Vec::new`/`to_string`/`format!`/`clone`/
  `vec![`) counted by grep: **89 in `production.rs`**, **71 in
  `raw_dpc/production_adapter.rs`**. Not all are hot, but no path in either
  file is currently allocation-audited.

- **No renderer owner thread exists.** No `std::thread::spawn` appears in any
  non-test file of `fn64-render-wgpu`, `fn64-render`, or `fn64-render-ir`
  (grep over `src/`, excluding `tests.rs`, returns only test files:
  `shader_manifest.rs`, `alpha_compare.rs`, `rt64_texture_map_lru.rs`,
  `device/mod.rs`, and four `tests.rs` modules). The renderer is entirely
  synchronous today; `create` "blocks on
  `UninitializedTrianglePipeline::request()`" by its own doc
  (`production.rs:1163-1172`).

#### Absent

- **No matched baseline, and the schema currently forbids one.**
  `docs/RT64-RENDER-MEASUREMENT.md:8-19` states report production is
  "**unavailable**" and lists four missing seams (fn64-abi raw per-advance
  snapshot; post-warmup measurement-window rebase; fn64-shell path-free JSON
  emitter; native metric instruments and a presentation-capable route).
  `tools/run_rt64_render_baseline.py` has no `--run` action (its argument group
  is exactly `--validate-schema`, `--print-doc`, `--check-doc`,
  `--validate-report`, `--selftest`).

  More sharply, the schema is **self-limiting**: `:28-31` — "The sole declared
  route, `headless_pump_one_frame`, has no physical presentation. Its
  `physical_presentation` metric must therefore be `unavailable`, which
  mechanically prevents `comparison_ready`." The checker enforces it at
  `run_rt64_render_baseline.py:560`. So **no report producible under schema v1
  can ever satisfy G6**; reaching `comparison_ready` requires a schema change,
  not just a run.

  Cohort validation is also deferred: `:68-74` — v1 "does not encode
  pair/repetition ordinals or both requested and observed horizon/workload
  boundaries … A later schema must require exactly five pairs, explicit
  ordinals, alternating control/instrumented and instrumented/control order".

- **M0.3 is RUNNING with reliability NOT MET.** From
  `docs/rt64-port-status.json`, ticket M0.3: `cargo nextest run -p fn64-abi
  --no-default-features` at **1/10** required deterministic runs (the second
  command, `tools/check_rt64_render_measurement.py`, is 10/10). Its own
  findings record: "No real matched baseline has been recorded; the private
  linked-build and native-identity gates remain open" and, importantly, the
  scoping call: "It blocks the performance/parity claim, not independent
  implementation, so the ticket remains RUNNING rather than BLOCKED."

  Note also that M0.3's `writable_paths` include `crates/fn64-certification`
  and `crates/fn64-shell`, and its branch is `feat/egui-settings-drawer-hud`.
  On **this** base, `crates/fn64-certification/src/lib.rs` contains a single
  line (`pub mod f3dzex2_point_light;`) and no measurement-report module, and
  grep for `RawMeasurementSnapshot`/`measurement_snapshot`/`raw_measurement`
  across `fn64-abi`, `fn64-certification`, and `fn64-shell` returns nothing.
  **M0.3's landed half is not on the port branch.** Any M6 card that assumes
  it must first state how the two branches reconcile.

**What M0.3 blocks, precisely:** it blocks **G6 only**. G1–G5 are structural
properties of fn64's own code, provable with fn64-internal instruments against
a synthetic workload, with no RT64 comparison and no private ROM. Treating M0.3
as blocking all of M6 is the conflation this document was asked to separate.

### 3. The gap, as work items

| # | work item | depends on |
|---|---|---|
| W1 | Port the six plume-free timer files (266 lines) as owned Rust: monotonic timestamp, delta-microseconds, the precise-sleep estimator, the elapsed timer, and the ring-buffer profiling timer. | nothing |
| W2 | Choose and install an allocation accounting mechanism (counting allocator or equivalent) that a test can read across a declared window. | D-M6-3 |
| W3 | Eliminate `plan_raw_dpc_inner`'s double decode by making the resource journal derivable before decode, or by making the first decode's journal reusable. | D-M6-4 |
| W4 | Introduce a frame arena (two- or three-deep) owning the per-draw uniform/vertex buffers currently created per submission, so `submit_triangles` rents rather than creates. | D-M6-2, D-M6-5 |
| W5 | Declare the queue *depth* bound (distinct from the existing size caps) and make over-depth a typed refusal. | D-M6-5 |
| W6 | Decide and implement the renderer thread topology; today the renderer is fully synchronous and reentrant through `dyn RenderBackend`. | D-M6-2 |
| W7 | Separate the capture readback from the render path so a future present route can be readback-free, keeping capture available under a test-only route. | D-M6-6 |
| W8 | Extend the measurement schema to a presentation-capable route and a five-pair counterbalanced cohort, so a `comparison_ready` report is representable. | D-M6-7, M0.3 |
| W9 | Re-derive the behavior of the eight plume-gated files against wgpu's resource model (buffer uploader → staging belt; render worker → owner thread; descriptor sets → bind-group layouts; shader compiler / raster shader cache → pipeline specialization + warm manifest). | D-M6-1, D-M6-2 |

W1–W3 are unblocked today. W4–W7 are decision-blocked. W8 is both
decision-blocked and evidence-blocked.

### 4. Decisions the owner must make

There is currently **no `P` (pending decision) entry for M6** in the plan's
decision log (`:838-849` lists P2/M2, P3/M4, P4/M5, P5/M9, P6/M12 — M6 and M7
are absent). That absence is itself the reason no cards exist. The following
should become the M6 pending-decision set.

---

#### D-M6-1 — What is the fate of the eight plume-gated M6 inventory files?

**Context:** 1,347 of M6's 1,613 lines depend on `rt64_plume.h`, which the plan
forbids transliterating (`:21-23`). The inventory records them `not-started`,
which reads as "will be ported".

**Options**

| option | cost | forecloses |
|---|---|---|
| **A. Port plume-shaped behavior onto wgpu, keep the file cards** | Highest. `rt64_descriptor_sets.h` alone is 677 lines of RHI descriptor shape that has no wgpu analogue as written; the mapping is a redesign wearing a port card's clothes. | Nothing technically, but it spends the port budget on a translation the plan already rejected. |
| **B. Mark them `superseded-by-architecture` and write behavior cards instead** | Requires an inventory schema change (a new `port_state`) and a re-run of `tools/rt64_port_inventory.py`; the burndown denominator drops by 1,347 lines, which will read as regression on the dashboard unless labelled. | Nothing. The behavior still has to be delivered, just under cards that name outcomes rather than files. |
| **C. Leave them `not-started` and never dispatch them** | Zero effort now. | Honesty. The dashboard keeps promising a port that will not happen, and the next planner rediscovers this. |

**Recommendation: B.** The plan's own execution rule 4 says the inventory
"assigns every admitted RT64 source/module/symbol to a task card, milestone,
owner, port state, and evidence state" — it is a *denominator*, and a
denominator that misstates what will happen is the failure mode this project
has already hit once (the "266 bogus writable_paths" repair recorded in the
SDD ledger's Lane0 entry). Option C's cost is paid by every future reader.
Option A's cost is paid once and is large. B makes the truth mechanical.

**Note:** B requires deciding what the burndown percentage *means* after the
change. Recommend reporting two numbers (lines ported / lines superseded)
rather than silently shrinking the denominator.

---

#### D-M6-2 — What is the renderer's thread topology?

**Context:** the plan's deliverable is "one renderer owner thread; bounded SPSC
work" (`:575-576`). Today the renderer is fully synchronous: no
`thread::spawn` outside tests, and `create` blocks on the adapter request
(`production.rs:1163-1172`). Crucially, T0's design closed a *reentrancy
hazard* by forbidding `fn64-render`/`fn64-render-wgpu` from depending on
`fn64-abi` (plan `:1233-1236`) — a property any threading design must preserve.

**Options**

| option | cost | forecloses |
|---|---|---|
| **A. Owner thread + bounded SPSC, as the plan states** | A real ownership redesign: the sealed T0 typestate chain (`RawDpcCoordinator<P>` double-buffered `[Option<P>; 2]`, `ReadyPublication::commit`'s Drop-free straight line) currently assumes single-threaded `&mut self`. Moving it across a queue means deciding which half lives on which side. | Nothing structurally; it is the plan's own target. |
| **B. Stay synchronous; pursue G1–G5 without a thread** | Much cheaper. Allocation-freedom, bounded queues, arena reuse, and readback removal are all achievable single-threaded. | The *asynchronous* half of the milestone name, and any latency win that comes from overlapping CPU prepare with GPU execute. It also forecloses the plan's "bounded two-frame low-latency and three-frame throughput queues" (`:760`), which only mean something across a thread boundary. |
| **C. Async-by-completion rather than async-by-thread** (keep one thread, make GPU completion non-blocking by removing the `PollType::Wait`) | Moderate. Requires the capture/present split (D-M6-6) first, since the blocking poll exists to serve readback. | Less than B: it recovers GPU/CPU overlap without a second thread, but still leaves CPU prepare serialized behind submit. |

**Recommendation: C first, then reassess A.** B abandons the milestone's stated
purpose. A is correct eventually but is the single largest redesign in M6, and
committing to it before the readback is off the render path means designing the
thread boundary around a path that is about to change shape. C is a strict
subset of A's work (removing the blocking wait is required for A anyway),
produces a measurable result on its own, and does not foreclose A.

This is the decision I am least confident recommending: the argument for A-now
is that retrofitting a thread boundary through the sealed T0 typestates later
may be harder than designing for it now. The owner is better placed to weigh
that, since it depends on how settled T0's shape is considered to be.

---

#### D-M6-3 — How is "zero steady-state heap allocation" to be *proven*?

**Context:** G1 is the milestone's headline structural claim and nothing in the
tree measures it. The measurement schema has an `allocation_bytes` metric
(`docs/RT64-RENDER-MEASUREMENT.md:53`) but no producer.

**Options**

| option | cost | forecloses |
|---|---|---|
| **A. A counting global allocator behind a test-only feature** | Low. A `#[global_allocator]` wrapper counting alloc/dealloc, enabled by a cargo feature, read by a test that asserts zero delta across a post-warmup window. | Nothing. Caveat: a global allocator is process-wide, so the test must run in a fresh process (the project already does this routinely — the 10/10 fresh-process bar). |
| **B. Instrument the specific hot functions with a scoped counter** | Medium, and it can only prove what it instruments — a new allocation in an uninstrumented callee passes. | The strongest form of the claim. G1 says "prepare, submit, render, and present perform zero allocation", which is a statement about everything reachable, not about annotated lines. |
| **C. Static analysis / no-alloc types (e.g. forbidding `Vec` in the hot module)** | High and brittle; Rust has no stable `#[no_alloc]`. | Practicality. |

**Recommendation: A.** It is the only option that can actually discharge the
claim as written, it is cheap, and its one weakness (process-global scope) is
already handled by the project's fresh-process discipline. B is a useful
*supplement* for locating an offender once A says the total is nonzero.

---

#### D-M6-4 — Is the double decode in `plan_raw_dpc_inner` fixed, or is it accepted and measured?

**Context:** every production submission decodes its capture twice and derives
the real journal from an error variant (`production.rs:1230-1250`, documented
at `:1345-1362`). This is the single clearest steady-state inefficiency in the
production path, and it predates M6.

**Options**

| option | cost | forecloses |
|---|---|---|
| **A. Make the journal derivable before decode** (a preflight pass that computes accesses without producing commands) | Medium; a third code path over the same wire bytes, which must be proven to agree with the decoder or it becomes a second source of truth. | Nothing, if the agreement is enforced by the existing `finish(journal)` equality check, which already "proves the writer's accumulated access list equals `journal`'s ordered access list exactly" (plan `:1080-1084`). That check makes a disagreeing preflight fail loudly. |
| **B. Make the first decode's output reusable** (decode once into a buffer, then seal) | Medium-low; changes `ExactRawDpcPlanWriter`'s consumption order but not its guarantees. | Possibly the writer's push-only discipline, depending on how it is done. |
| **C. Accept it; measure whether it matters before spending anything** | Near zero now. | Nothing — and per `perf-measure-before-dispatching`, this is the disciplined default. |

**Recommendation: C, then A if measured.** This project's own recorded lesson
is that byte counts and obvious-looking duplication are frequently not the
bottleneck. A decode of a bounded command capture may be microseconds. **But**
note that C requires an instrument, so C is not free — it is gated on D-M6-3's
mechanism plus a per-stage timer (which W1's `ProfilingTimer` port would
supply). That is a virtuous ordering: W1 → D-M6-3 → measure → then decide.

---

#### D-M6-5 — What are the queue *depth* bounds, and where is the boundary?

**Context:** the plan says "bounded two-frame low-latency and three-frame
throughput queues. No unbounded work or texture channels." (`:441-442`, echoed
at `:760`). The tree has size caps (`MAX_COMMAND_CHUNKS`, `MAX_RESOURCE_ACCESSES`,
`MAX_WORKLOAD_RECORD_BYTES`) but no frame-depth bound, because there is no
frame pipeline yet.

**Options**

| option | cost | forecloses |
|---|---|---|
| **A. Fix 2/3 as the plan states, selectable by a latency-vs-throughput policy** | Low to state, but meaningless until D-M6-2 creates a boundary to bound. | Nothing. |
| **B. Make depth a declared parameter with a tested refusal, value chosen later** | Lowest. The *typed refusal* is the load-bearing part; the number can be tuned. | Nothing. |
| **C. Defer entirely until the thread topology lands** | Zero. | Nothing, but it leaves W4's arena depth undetermined, and the arena is wanted before the thread (see D-M6-2 rec C). |

**Recommendation: B.** The arena in W4 needs *a* depth to be built at all, and
the invariant worth testing is "over-depth refuses loudly", not "the number is
2". Choosing B lets W4 start before D-M6-2 resolves.

---

#### D-M6-6 — Does the capture readback stay on the render path?

**Context:** `submit_triangles` unconditionally copies color, depth, and status
to readback buffers and blocks (`triangle_pipeline.rs:1755-1797, 1867, 1947-1951`).
G3 says "ordinary present has zero readback". There is currently no present at
all (`production.rs:1200-1207` returns an error), so today the readback *is*
the output — it is how every GPU test observes a pixel.

**Options**

| option | cost | forecloses |
|---|---|---|
| **A. Split: a capture route that reads back (tests, headless) and a render route that does not** | Medium. Every one of the ~17 host-GPU tests observes through readback; they keep the capture route, so they do not churn. The render route needs a consumer, which is presentation — not yet built. | Nothing. This is the shape G3 implies. |
| **B. Keep readback until presentation exists, then remove it** | Zero now. | Sequencing: it makes G3 depend on presentation, which is an M3.4/M8 concern, so M6 cannot close until an unrelated milestone does. |
| **C. Make readback opt-in per submission now** | Low. A flag on the submission; capture tests set it. | Nothing, and it is a strictly smaller version of A. |

**Recommendation: C as the first step toward A.** It is small, it is testable
today (assert a no-readback submission issues zero `copy_texture_to_buffer`
calls), and it converts G3 from "blocked on presentation" into "measurable on
the route that will become present". It also makes the *current* dependency
honest: G3 as literally worded ("ordinary present") cannot be closed until an
ordinary present exists, and the owner should know that G3 is presentation-gated
regardless of which option is picked.

---

#### D-M6-7 — Does the measurement schema get a v2 now, or after the route exists?

**Context:** schema v1 mechanically forbids `comparison_ready`
(`docs/RT64-RENDER-MEASUREMENT.md:28-31`; enforced at
`run_rt64_render_baseline.py:560`) and defers cohort validation (`:68-74`).
G6 is unreachable under v1 by construction.

**Options**

| option | cost | forecloses |
|---|---|---|
| **A. Write schema v2 now** (presentation-capable route, five-pair ordinals, alternating counterbalance) | Medium. Risk: designing a wire for a route that does not exist yet invites a v3. | Nothing, but likely wastes a revision. |
| **B. Build the presentation-capable route first, then v2 from what it actually emits** | Higher sequencing cost; G6 stays unreachable longer. | Nothing. |
| **C. Split: land the cohort/ordinal half of v2 now (which does not depend on the route), defer the presentation half** | Low. The five-pair counterbalanced structure is already fully specified in prose at `:68-74`; encoding it is mechanical. | Nothing. |

**Recommendation: C.** The cohort structure is route-independent and already
written down in enough detail to implement; the presentation metric is not.
Splitting also means the A/B *methodology* is checkable before there is
anything to compare, which is the right order — a counterbalanced-cohort
checker that nobody can yet feed is still a gate that will hold when they can.

---

### 5. Evidence-blocked versus decision-blocked (M6)

| item | blocked on | note |
|---|---|---|
| G1 zero-allocation | **decision** (D-M6-3) | provable with fn64-internal instruments, synthetic workload, no ROM |
| G2 zero critical-thread pipeline creation | **decision** (D-M6-2 defines "critical thread") | prewarm already exists; the claim needs a thread to be critical |
| G3 zero present readback | **decision** (D-M6-6) *and* structurally gated on presentation existing at all | presentation is not an M6 deliverable |
| G4 zero full-RDRAM copies | **neither — arguably already true** | the T4 entry (plan `:1378-1428`) claims live-RDRAM captures with no full-memory snapshot, and M4.0 (`:937-950`) states "performs no full-memory snapshot". Needs a counter to *prove*, not new work to achieve. |
| G5 bounded queues | **decision** (D-M6-5) | |
| G6 matched A/B faster | **evidence** (no baseline) **and decision** (D-M6-7 schema) **and** M0.3's private linked build | genuinely hard-blocked |
| W9 plume-gated behavior | **decision** (D-M6-1, D-M6-2) | |

**The important separation:** five of six exit-gate clauses are
decision-blocked or already-nearly-true. Only G6 is evidence-blocked. The plan
and dashboard currently present M6 as uniformly gated behind M0.3's baseline;
that is true of one clause out of six.

---

## M7 — base-renderer certification

### 1. What the exit gate actually demands

Quoted from `docs/RENDER-WGPU-PORT-PLAN.md:595-598`:

> **Exit gate:** every base row is exact or carries an owner-approved bounded
> qualification; deterministic gates meet 10 consecutive clean runs and all
> concurrency gates meet 20+; no visual-only observation is promoted to a
> memory or timing claim.

Ledger headline (`:454`): "behavior matrix and private full-ROM gates close
without broadened admission."

Deliverables (`:591-593`): "complete base behavior matrix, OoT eye gates,
synthetic and private full-ROM traces, exact guest-memory side-effect
comparisons, device loss/resize recovery for the reference platform, and
release evidence schema."

Restated:

| # | demand | measurable as |
|---|---|---|
| H1 | every base row exact **or** owner-approved bounded | `docs/BASE-RENDERER-BEHAVIOR-MATRIX.md` with no row lacking either status |
| H2 | 10 consecutive clean deterministic runs | the project's standing bar |
| H3 | 20+ concurrency runs | ditto |
| H4 | no visual-only observation promoted to memory/timing | a review property, checkable by the receipt schema (`fn64-render-conformance` receipts already bind "the earliest observable that distinguishes the behavior under test") |
| H5 | private full-ROM gates close | `full-rom-zero-unsupported` row reaching exact |
| H6 | OoT eye gates | human visual comparison — `docs/DELEGATION.md:67` records eye-gates as **never delegable to any agent tier** |

### 2. What already exists

- **A frozen, mechanically validated 24-row denominator.**
  `docs/base-renderer-behavior-matrix.json` → `docs/BASE-RENDERER-BEHAVIOR-MATRIX.md`
  via `tools/check_base_renderer_matrix.py`. Current grading:

  | exactness | count |
  |---|---:|
  | `exact_public` | 4 |
  | `bounded_reference` | 18 |
  | `missing` | 2 |

  Blocker classes: hardware trace 19, full-ROM 8, allowed specification 1,
  implementation 2. Four rows carry **no** blockers and are already exact:
  `raw-dpc-command-envelope`, `rdp-command-state-order`, `depth-memory-encoding`,
  `tmem-load-layout-formats`.

  The validator is strict and real: it "rejects denominator shrinkage, unknown
  categories or statuses, stale evidence paths/needles, implemented rows
  without test evidence, non-exact rows without blockers, loss of hardware/
  full-ROM blockers, a closed parent claim, and generated-doc drift"
  (`BASE-RENDERER-BEHAVIOR-MATRIX.md:65-68`).

- **A parity ladder that mirrors it and passes its checker.**
  `docs/rt64-port-parity.json`: 50 required rows — 24 `base_renderer` +
  26 `rt64_public_feature`. `python3 tools/check_rt64_port_parity.py` on this
  base: "clean (50 required rows; 50 Rust pending; 49 RT64 observations
  pending)". Exactly one row is `RT64_PASS`; every row is `RUST_PENDING`.

- **A conformance crate with the right shape.** `crates/fn64-render-conformance`
  owns fixtures, row IDs, and receipts; its contract declares the nine-layer
  `observable_order` (`admitted_commands_state` → `full_sync_timeline` →
  `tmem_bytes` → `resource_journal_guest_memory_effects` → `shader_parameters`
  → `framebuffer_native` → `framebuffer_high` → `vi` → `post_vi_pixels`),
  three `delegate_kinds` (`rt64`, `rust_port`, `reference`), and
  `"rt64_observation_policy": "required_for_every_row"`. Three binaries exist:
  a test runner, a verifier, and an RT64 deferred-history runner.

- **A real GPU on this host** (see CC-3), which is a materially better
  certification position than the plan's stale text assumes.

- **3,312 passing tests in `fn64-render-wgpu` alone**, including host-GPU
  differentials against CPU oracles at real pixels.

### 3. The gap — and the finding that dominates it

**Zero of the 24 base rows cite the wgpu lane.** Counting evidence-path crates
across all 194 evidence entries in `docs/base-renderer-behavior-matrix.json`:

| crate | citations |
|---|---:|
| `fn64-render-reference` | 110 |
| `tools` | 28 |
| `docs` | 17 |
| `fn64-abi` | 9 |
| `fn64-render-rt64` | 7 |
| `fn64-render` | 7 |
| `fn64-certification` | 7 |
| `fn64-boot-harness` | 6 |
| `fn64-audio`, `fn64-cpu-runtime`, `fn64-runtime` | 1 each |
| **`fn64-render-wgpu`** | **0** |

The matrix in its current form certifies the **reference lane and the RT64
adapter**. It is not, today, a statement about the Rust wgpu renderer at all.
The parity ladder agrees and is honest about it: all 50 rows are `RUST_PENDING`
with `rust_evidence.availability: "unimplemented"` and the reason "The Rust
renderer delegate does not exist yet."

So "complete the base behavior matrix" as an M7 deliverable is ambiguous in a
way that changes the milestone's size by an order of magnitude — see D-M7-1.

**Second structural finding: the validator forbids closure.**
`tools/check_base_renderer_matrix.py:184`:

```python
require(any(item["exactness"] != "exact_public" for item in items),
        "base accuracy cannot be closed while this matrix is the open denominator")
```

plus `:182-183` requiring at least one `hardware_trace` and at least one
`full_rom` blocker to remain, plus `validate_claim_guard` (`:188-200`) requiring
`base-rendering-accuracy` to have `status == "open"` in the public feature
inventory. And `EXPECTED_IDS` (`:27`) freezes the 24 row IDs, so a wgpu-lane row
cannot be added without editing the tool.

Read literally, **the tool that owns the M7 denominator asserts the denominator
cannot close.** This is defensible as a guard against a premature claim, but it
means H1 as written is currently unachievable by construction, and closing M7
requires deliberately changing that tool. That should be an owner decision, not
a card-writer's incidental edit (D-M7-3).

**Third: 19 of 24 rows are hardware-trace-blocked.** The blocker text is
specific and repeated: silicon accumulator widths, negative-product rounding,
clamp points, coverage representative-sample lookup, reciprocal-to-S10.5
rounding, the VI gamma ROM and random generator, the dither matrices. These
need a physical N64 and a capture rig. No amount of Rust closes them. The M7
gate's escape hatch is "**or** carries an owner-approved bounded qualification"
— which means, in practice, M7 closes by the owner approving ~19 bounded
qualifications, not by the rows becoming exact. That is a legitimate reading of
the gate, and it should be made explicit (D-M7-2), because it converts M7 from
an engineering milestone into largely a **judgment** milestone.

**Fourth: `full-rom-zero-unsupported` is `missing` and needs private ROMs.**
Its blocker records that the historical schema-v22 assessment "canonically
retained 12 satisfied and 150 missing FullParityV1 assignments; current v30
requires regeneration". `docs/PRIVATE-INPUT-ADMISSION.md` confirms the private
manifest "must never enter git" and "must remain in `/private/tmp` or another
path outside the repository". This row is hard evidence-blocked on ROM access
plus a v29 regeneration campaign.

Gap as work items:

| # | work item | depends on |
|---|---|---|
| X1 | Decide what the matrix certifies (reference lane vs wgpu lane vs both) and, if wgpu, extend the schema/tool to carry a per-row lane. | D-M7-1, D-M7-3 |
| X2 | Implement the `rust_port` conformance delegate so any parity row can be exercised at all. | D-M7-1 |
| X3 | For the 4 blocker-free rows, add wgpu-lane evidence (these are the rows where the wgpu crate plausibly already has the tests, unlabelled). | X1, X2 |
| X4 | Produce owner-approved bounded qualifications for the 19 hardware-trace rows, or fund a capture program. | D-M7-2 |
| X5 | Regenerate the private full-ROM series under schema v29. | private ROM access; M0.3-adjacent |
| X6 | Device-loss / resize recovery for the reference platform (named in M7's deliverables, and nothing in `fn64-render-wgpu` implements it — `resize` is `fn(&mut self, _w, _h) {}`, `production.rs:1209`). | nothing — this one is unblocked |
| X7 | Define what "OoT eye gates" means for M7 specifically: which scenes, what pass criterion, who looks. | D-M7-4 |

### 4. Decisions the owner must make

Again, **no `P` entry for M7 exists** in the plan's decision log. Proposed set:

---

#### D-M7-1 — Does the base behavior matrix certify the reference lane, the wgpu lane, or both per row?

**Context:** 0/194 evidence citations reference `fn64-render-wgpu`. The parity
ladder's 24 base rows are all `RUST_PENDING` with delegate "does not exist yet".
M7's name is "**base-renderer** certification"; the plan's product target is
"a completely Rust-owned fn64 render pipeline" (`:13`).

**Options**

| option | cost | forecloses |
|---|---|---|
| **A. The matrix certifies the reference lane; the parity ladder separately certifies wgpu** | Lowest — this is the status quo, made explicit. Two artifacts, two denominators, one shared row vocabulary (already true: parity row IDs are `base::<matrix-id>`). | Nothing, but it means M7's exit gate ("every base row is exact or bounded") is satisfiable **without the Rust renderer passing anything**, which cannot be the intent given the milestone's name. |
| **B. The matrix gains a per-row lane dimension; a row is exact only when both lanes are** | Highest. Requires a schema change, a tool change, and 24 rows × 1 new lane of evidence — much of which does not exist and some of which (VI analog, F3DZEX2 point light) the wgpu lane is nowhere near. | Nothing, but it makes M7 the largest milestone in the program. |
| **C. The matrix stays reference-lane; M7's gate is re-stated over the *parity ladder's* 24 base rows instead** | Medium. No matrix change; the parity ladder already exists, already has a checker, already distinguishes `RUST_PENDING`/`RUST_PASS`/`RUST_BOUNDED_QUALIFICATION`, and already forbids implicit skips ("A separate port-progress command exits nonzero while any required `RUST_PENDING` row remains", `:147-148`). | Nothing obvious. It uses the artifact that was *built for this*. |

**Recommendation: C.** The parity ladder is already the backend-neutral
conformance contract the plan designed for exactly this question (`:124-169`),
its states already include `RUST_BOUNDED_QUALIFICATION` (which is precisely the
M7 gate's "owner-approved bounded qualification"), and its checker already
passes. Option A is the status quo and reads as certification theatre for a
Rust-renderer milestone. Option B duplicates the ladder inside the matrix.

Under C, M7's deliverable "complete base behavior matrix" should be re-worded
to "close the 24 base rows of the parity ladder", and the behavior matrix
becomes an input (the row definitions and blocker classes) rather than the gate.

**This is the highest-value decision in this document.** Every M7 card's size
depends on it, and it cannot be inferred from the plan text, which uses
"behavior matrix" for the artifact and "base row" for the unit without saying
which artifact owns the rows.

---

#### D-M7-2 — Are the 19 hardware-trace rows closed by owner-approved bounded qualification, or is a hardware capture program funded?

**Context:** 19 of 24 rows carry `hardware_trace` blockers naming unpublished
silicon behavior (accumulator widths, rounding ties, the VI gamma ROM, the
dither RNG). The gate permits "an owner-approved bounded qualification".
`docs/RDP-SILICON-VECTORS.md` and `docs/VI-ANALOG-CAPTURE-PROGRAM.md` already
specify capture programs in detail; the matrix's `next_action` fields
repeatedly say "run … from reset on hardware … pass repeated-capture
consensus".

**Options**

| option | cost | forecloses |
|---|---|---|
| **A. Owner approves bounded qualifications for all 19** | Owner time (19 judgments), and each qualification must be written down precisely enough that a later reader knows what was *not* claimed. Cheap in engineering, expensive in care. | Nothing permanently — a later capture can promote a bounded row to exact. It does mean fn64 ships with 19 documented approximations. |
| **B. Fund the capture program first** | Very high: physical console, capture hardware, ten power-cycle cohorts per vector, independent extraction review. The docs estimate this in reset-isolated cohorts across four separate producers. | Schedule. M7 would not close this year. |
| **C. Split — capture the few rows where the approximation is most load-bearing, qualify the rest** | Medium; requires ranking which approximations matter most, which is itself a judgment. | Nothing. |

**Recommendation: A, with C as a named follow-on.** The gate explicitly
contemplates bounded qualification, the matrix already records each boundary
with unusual precision (each row's blocker text is specific about *what* is
unknown), and this project's stated discipline is that a bounded claim honestly
labelled beats an unbounded one. Blocking a certification milestone on physical
silicon captures would make M7 permanently open, which serves nobody.

The care required is real, though: 19 approvals is a lot of judgment to issue
in one sitting, and the failure mode is rubber-stamping. Suggest the owner
batch them by category (microcode / combiner / blender / coverage / texture /
VI) rather than row by row, since the blocker text repeats within categories.

---

#### D-M7-3 — Does `check_base_renderer_matrix.py`'s no-close guard get relaxed, and by whom?

**Context:** `:182-184` and `validate_claim_guard` make it impossible for the
matrix or the `base-rendering-accuracy` claim to be marked closed. That is a
deliberate ratchet.

**Options**

| option | cost | forecloses |
|---|---|---|
| **A. Leave it; M7 closes against the parity ladder instead** | Zero. Consistent with D-M7-1 rec C. | Nothing. |
| **B. Relax the guard when the owner approves closure** | Low mechanically, but it removes the ratchet that has been protecting the claim. Once removed, nothing prevents a future agent from closing it. | The guard's whole purpose. |
| **C. Replace the hard guard with an explicit owner token** (closure permitted only when a named field records who approved it and when) | Low-medium. Keeps the ratchet, adds a legitimate key. | Nothing. |

**Recommendation: A if D-M7-1 lands on C; otherwise C.** Never B — an
unconditional relaxation of a ratchet in a program whose own docs record a red
gate shipping for a day is a bad trade. Note this guard is *why* the previous
planners could not write an M7 closure card even if they had wanted to.

---

#### D-M7-4 — What is an "OoT eye gate" for M7, concretely?

**Context:** the phrase appears in the plan exactly once (`:591`) and is
defined nowhere. The project *does* have an eye-gate practice:
`docs/ROADMAP.md:97` records "R3 eye-gate PASSED — scope: title/attract camera
ONLY", `:100` records "R3b outdoor gameplay eye-gate" as still open, and
`docs/DELEGATION.md:67` records eye-gates as "**Never delegate to any tier**
(the user's, batched)". `docs/OOT-STATUS.md:9-12` states the visual contract:
"verified by looking at the actual PNG side-by-side with the emulator. No agent
self-certifies a render."

**Options**

| option | cost | forecloses |
|---|---|---|
| **A. Reuse the R3/R3b scopes** (title/attract, then outdoor gameplay) | Low, and it inherits an established pass criterion. | Nothing. Note R3b is recorded as blocked on the capture route, not on rendering. |
| **B. Define a new M7-specific scene set** | Medium; needs a rationale for each scene. | Nothing, but it discards a precedent that already worked once. |
| **C. Drop eye gates from M7 and rely on pixel differentials** | Zero engineering, but it contradicts `OOT-STATUS.md`'s explicit contract and this project's recorded experience that "both audio and projection bugs this project hit were unit-green but end-to-end broken". | The one check that has historically caught end-to-end breakage. |

**Recommendation: A.** It reuses a working precedent and inherits R3b's known
blocker (capture route) as a named, already-understood dependency rather than a
new unknown.

Whichever is chosen, cards must record that this step is **owner-executed and
non-delegable**, so no planner schedules it to an agent.

---

### 5. Evidence-blocked versus decision-blocked (M7)

| item | blocked on | note |
|---|---|---|
| H1 for the 4 blocker-free rows, wgpu lane | **decision** (D-M7-1) | the wgpu crate likely already has the tests; they are unlabelled |
| H1 for the 19 hardware-trace rows | **decision** (D-M7-2) if bounded qualification; **evidence** (physical console) if exact | the gate permits either — this is the fork |
| H1 for `vi-aa-resampling-analog` | **evidence** (analog capture) *and* implementation (no DAC model) | `missing`; the hardest row |
| H5 `full-rom-zero-unsupported` | **evidence** (private ROMs) + a v29 regeneration campaign | no ROM bytes may enter git; manifests live outside the repo |
| H2/H3 run bars | **neither** | standing project practice, mechanically runnable |
| H4 no visual→memory promotion | **decision** (a review rule) | the receipt schema already binds "the earliest observable" |
| H6 OoT eye gates | **decision** (D-M7-4) then **owner time** | non-delegable |
| X2 `rust_port` delegate | **decision** (D-M7-1) then unblocked engineering | nothing external blocks it |
| X6 device-loss / resize recovery | **neither — fully unblocked** | `resize` is an empty stub at `production.rs:1209` |

**The important separation:** M7's *certification bookkeeping* is
decision-blocked; only two of its demands (the analog VI row and the private
full-ROM row) are genuinely evidence-blocked. The plan's phrase "private
full-ROM gates close" makes M7 sound uniformly ROM-gated; one row of 24 is.

---

## Candidate first cards

Each is the smallest real work that can start immediately regardless of how the
open decisions land.

### M6 first card — port the six plume-free RT64 timer files

**Why it survives every decision above.** It touches no thread topology
(D-M6-2), no arena (D-M6-5), no readback (D-M6-6), no schema (D-M6-7). It is
unaffected by D-M6-1 because these six are the files everyone agrees *are*
portable. And it produces the instrument D-M6-4's "measure first" recommendation
requires.

**Scope:** `src/common/rt64_timer.{h,cpp}`, `rt64_elapsed_timer.{h,cpp}`,
`rt64_profiling_timer.{h,cpp}` from the pinned port commit — 266 lines total,
zero `rt64_plume.h` dependencies (per the inventory's own dependency edges).

**What is actually in them** (read from the pinned source at
`5473732a…:src/common/`, and non-trivial enough to be worth a differential):

- `Timer::deltaMicroseconds` — a `duration_cast` truncation, so it truncates
  toward zero including for negative deltas. Worth a fixture.
- `Timer::preciseSleepUntil` — a Welford online mean/variance estimator over
  observed 100 µs sleep durations, with a two-standard-deviation
  upper estimate (`estimateStddevCount = 2.0`), a `fixedSleepUpperBound` of
  2 ms above which a sample is excluded from the statistics, `thread_local`
  accumulators seeded at count 1, and a final busy-spin. The *estimator* is
  pure arithmetic and exactly differentiable against a hand-computed sequence;
  the *sleeping* is not, and must be a nonclaim.
- `ProfilingTimer` — a fixed-size ring buffer (`history`, `historyIndex`),
  an accumulator, and `average()`. Pure.

**Recommended card shape** (behavior card, not a per-file port card, per CC-1):
one owned Rust module exposing a monotonic timestamp type, a delta helper, the
sleep-duration estimator as a **pure function over a sample sequence**, an
elapsed timer, and a ring-buffer profiling timer. Differential fixtures: the
Welford sequence against hand-computed values including the
`fixedSleepUpperBound` exclusion branch and the `count == 1` seed; the ring
buffer's wrap and `average()` at wrap boundaries; `deltaMicroseconds`
truncation at negative and sub-microsecond deltas.

**Explicit nonclaims:** ports no plume-dependent file; makes no allocation,
threading, queue-depth, or performance claim; the sleep *behavior* (as opposed
to the estimator arithmetic) is host-scheduler-dependent and is not gated.

**Why this is the right first card and not merely the easiest:** M6's stated
deliverable list ends with "per-stage telemetry", and D-M6-3/D-M6-4 both
recommend measuring before changing anything. `ProfilingTimer` is that
instrument, and it is the only M6 deliverable that is simultaneously
portable-as-written and prerequisite to the milestone's own method.

### M7 first card — device-loss and resize recovery for the reference platform

**Why it survives every decision above.** It is named directly in M7's
deliverables (`:592-593`, "device loss/resize recovery for the reference
platform"), it is not a matrix row, so D-M7-1/2/3 do not touch it, and it is
not an eye gate, so D-M7-4 does not touch it. It needs no ROM, no hardware
capture, and no baseline.

**Current state:** `RenderBackend::resize` in
`crates/fn64-render-wgpu/src/production.rs:1209` is `fn resize(&mut self, _w:
u32, _h: u32) {}` — an empty stub. There is no device-loss handling anywhere in
the crate. Meanwhile `create` is documented as "a repeated call is a full
reset: `triangle_pipeline`/`triangle_target_extent` are always replaced
together, from a fresh device request, never partially"
(`production.rs:1163-1172`) — which is already most of a recovery primitive,
and it is already exercised by the host-GPU test
`required_host_atomic_pipeline_and_extent_replacement` (per the capsule's
verification list).

**Recommended card shape:** make resize a real, typed operation over the
existing atomic-replacement primitive, with a loud refusal for a resize that
arrives while a submission is in flight (the sealed T0 typestates make "in
flight" representable, so this can be a type-level refusal rather than a
runtime flag). Add device-loss detection on the same path. Gate with host-GPU
tests on the real Metal adapter available here (CC-3): resize between
submissions preserves subsequent draw correctness; resize during a pending
publication refuses by name; a simulated device loss re-creates rather than
half-updates.

**Explicit nonclaims:** certifies no behavior-matrix row; makes no presentation
claim (there is still no present route); is scoped to the reference platform
(macOS/Metal) only; proves nothing about Vulkan or D3D12 (that is M10).

**A candidate second card, if one is wanted:** implement the `rust_port`
conformance delegate for the four blocker-free rows. It is genuinely small and
high-value, but it is **not** decision-independent — it presumes D-M7-1 lands
on B or C. Listed here so it is not lost, not proposed as the first card.

---

## Vague or contradictory items in the plan

Recorded because the brief asked for them; each is a finding the owner needs,
not a request to change the plan.

1. **"complete base behavior matrix" (M7 deliverable, `:591`) does not say
   which lane.** 0 of 194 matrix evidence citations reference the wgpu crate.
   The deliverable is satisfiable today by the reference lane alone, which
   cannot be the intent of a milestone named "base-renderer certification".
   → D-M7-1.

2. **The matrix's own validator forbids the matrix from closing**
   (`tools/check_base_renderer_matrix.py:182-184`, `validate_claim_guard`).
   M7's H1 is therefore unachievable without editing the gate that defines it.
   → D-M7-3.

3. **"OoT eye gates" (`:591`) is defined nowhere.** The phrase occurs once in
   the entire repo. A practice exists (ROADMAP R3/R3b, DELEGATION.md:67,
   OOT-STATUS.md's visual contract) but is not bound to M7. → D-M7-4.

4. **M6's G3 ("ordinary present has zero readback") presumes a present route
   that does not exist.** `RenderBackend::present` returns
   `Err(…"presentation is out of scope")` (`production.rs:1200-1207`). G3 is
   presentation-gated regardless of any M6 decision, and presentation is not an
   M6 deliverable. → D-M6-6 and a plan note.

5. **The measurement schema mechanically forbids the report M6's G6 requires.**
   `docs/RT64-RENDER-MEASUREMENT.md:28-31` and
   `run_rt64_render_baseline.py:560`. M0.3's next-action reads as "run the
   repetitions", but no producible v1 report can reach `comparison_ready`.
   → D-M6-7.

6. **The live status capsule contradicts itself about GPU availability.**
   Line 47: "this environment has no GPU adapter for any backend". Line 51:
   "confirmed 10 consecutive clean runs against a real Metal adapter". Measured:
   3,312/3,312 pass with `--features host-gpu-tests` on this host. Line 47 is
   stale. → CC-3.

7. **M6's inventory cards are literal-port cards for an architecture
   milestone**, contradicting D1. 1,347 of 1,613 M6 lines route through
   `rt64_plume.h`, which the plan forbids transliterating (`:21-23`). → CC-1,
   D-M6-1.

8. **Neither M6 nor M7 has a `P` entry in the decision log** (`:838-849` lists
   P2/M2, P3/M4, P4/M5, P5/M9, P6/M12). Both milestones' contracts imply
   architecture choices, and the plan's own convention is that such choices are
   "owned by their milestones" as pending decisions. The absence of a P entry
   is a plausible mechanical cause of the three prior planners' correct refusal
   to ticket them.

9. **`lint-docs.py` is at 5 errors on this base, not the 4 recorded as the
   baseline.** The fifth (`RT64-PORT-INVENTORY.md: … FbReinterpretCS.hlsl:
   ported_as drift`) is pre-existing at HEAD `854d2867` and is fixed by
   regenerating the inventory, not by editing a doc. → CC-4.

10. **An open merge blocker from the SDD ledger's final review is not fixed on
    this base.** The ledger's final review returned "NOT READY — 1 Important":
    a `FillRectangle + RawTriangle` packet is silently admitted and runs two
    disjoint render paths. The ruling was "fix before merge with a named
    `MixedFillAndTrianglePacket` rejection beside the existing
    `MixedFillAndTmemLoadPacket`, and move the pending-token store after the
    triangle draw." On HEAD `854d2867`, `MixedFillAndTmemLoadPacket` exists at
    `production.rs:990,1052,1721,1842` and **`MixedFillAndTrianglePacket` does
    not exist**; `stage_and_report` (`:1713-1725`) checks fills-vs-loads only,
    and `execute_raw_dpc` stores `pending_fill_publication` (`:1266`) before
    `draw_admitted_triangles` (`:1269`). Out of M6/M7 scope, reported because
    it is a known-open blocker sitting on the branch this scoping targets.

---

## Method note

Everything above was read directly in `/private/tmp/fn64-lane50` at HEAD
`854d2867` or in the pinned RT64 port checkout
`5473732a822a4423b5696e7cb18fecc425a59875`. Counts (3,312 tests; 194 evidence
citations; 0 wgpu citations; 89/71 allocation-shaped calls; 266/1,613 M6 lines)
were measured, not carried forward from another document. Where I inferred
rather than observed — the fate of the plume-gated files, the likely size of
the double-decode cost — it is labelled inferred and the basis is given. No
file other than this one was modified.
