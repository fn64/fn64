# RT64 Rust-port orchestration

This document turns the renderer-port plan into a durable dispatch protocol.
It supplements, rather than replaces, the milestone contracts in
[`RENDER-WGPU-PORT-PLAN.md`](RENDER-WGPU-PORT-PLAN.md), the general wave rules
in [`DELEGATION.md`](DELEGATION.md), and the clean-room contract in
[`../AGENTS.md`](../AGENTS.md). The port is allowed to use parallel agents to
increase throughput; it is not allowed to use parallel agents to weaken an
evidence claim.

## Operating rule

One integration lead owns the active milestone's claim and merges its work.
Delegates own bounded artifacts or read-only evidence only. The lead reviews
every diff, re-runs the relevant gates, resolves design conflicts, and writes
the durable handoff. No delegate, regardless of model, may self-certify parity,
performance, license provenance, a race fix, or a visual/eye gate.

`AGENTS.md` remains the controlling rule: use only clean-room-allowed sources;
do not read GPL runtime code or m2c; trap unsupported behavior loudly; retain
no game content or private traces; and meet the 10-run deterministic and
20-run concurrency bars. A task card cannot relax these requirements.

The accelerated track deliberately permits several dependency-safe milestones
to execute together. “One lead” means one owner of shared contracts and final
claims, not one active worker. M0 evidence, M1 semantic ownership, M2 backend
probes, mechanical inventory, and isolated replay tooling should occupy
separate lanes whenever their paths do not overlap. An open evidence gate
prevents a parity/performance/cutover claim; it does not idle translation work.

### Concurrency profiles

| profile | threads | allocation | use |
|---|---:|---|---|
| `S4` | 4 total | lead + three bounded writers | current shared-worktree minimum; reserve paths strictly |
| `S8` | 8 total | lead, semantic, raw/TMEM, framebuffer/VI, GPU, fixtures, tooling, review | preferred sustained port wave in isolated worktrees |
| `S10` | 10 total | `S8` plus independently owned platform backend and validation lanes | temporary breadth wave after the shared contracts freeze |

Adding writers past `S10` is not presumed faster: shared API review, GPU test
hardware, and integration become the bottleneck. Use cheaper parallel agents
for generated work and validation, not competing architectural edits.

## Model and effort profiles

The names below are current recommendations, not a provider lock-in. If a
named model is unavailable, select a model with the stated capability and keep
the same review/evidence boundary. Effort is part of the task's required
resource, not a reward for task size.

The 2026-08-15 assignments follow OpenAI's current model guidance: Sol is the
frontier tier, Terra balances capability and cost, and Luna is the efficient
high-volume tier. Effort starts at `medium`, rises to `high` or `xhigh` only
for a measured quality need, and does not default to the maximum merely
because it exists. Re-evaluate these recommendations against representative
port task cards when the model family changes:
<https://developers.openai.com/api/docs/guides/latest-model>.

| profile | current recommendation | capability fallback | permitted work |
|---|---|---|---|
| `F` frontier lead | GPT-5.6 Sol at `xhigh` (or `high` for a settled contract) | an Opus-class or equivalent frontier reasoning model | architecture, clean-room and authority decisions, invariants, concurrency arguments, integration, adversarial review, final claims |
| `I` isolated implementer | GPT-5.6 Sol at `high` | strong coding model able to execute tests and reason about ownership | a bounded implementation behind an already-written contract and tests |
| `P` probe/research | GPT-5.6 Terra at `medium` or `high` | capable mid-tier coding/research model | independent source mapping, one-API feasibility probes, fixture analysis, non-overlapping test scaffolding |
| `M` mechanical delegate | GPT-5.6 Luna at `low` or `medium` | fast lower-cost model with a deterministic checker | inventory extraction, citation harvesting, generated-doc refresh, validator mutations, focused test repetition, path-scoped lint repair |

Claude Sonnet agents are approved when the active orchestration environment
actually exposes them. Record the exact advertised model/version in the task
card; “Sonnet” alone is not a reproducible identity. Their default placement is
`I/high` for a bounded implementation behind a frozen public contract, or
`P/high` for an independent adversarial review, backend probe, fixture audit,
or test synthesis lane. They are particularly useful for disjoint RDP opcode,
TMEM, framebuffer, VI, shader, and platform modules. Do not assign a Sonnet
lane simultaneous ownership of `fn64-abi`, a shared IR contract, parity or
performance interpretation, clean-room authority, or final integration. Those
remain `F` decisions unless a future recorded evaluation promotes the exact
model to the frontier profile.

Model availability is checked at dispatch time. A model named in this plan but
absent from the session's spawn interface is an approved future option, not an
active worker. Never relabel a GPT worker as Claude (or the reverse) in the
handoff merely because their profile is equivalent.

Use `M` aggressively for tasks whose output is rejected or accepted by a
deterministic oracle. Do not give it an ambiguous behavioral question and then
mistake a plausible answer for evidence. An `M` or `P` finding becomes a
candidate fact until an `F` or `I` owner checks the source, scope, and gate.

## Milestone staffing matrix

Each row states the *minimum* lead profile for an implementation change.
Independent review must be at least as capable as the implementation tier,
and all final milestone closure is `F`.

| milestone | lead / effort | safe parallel delegate lanes | serialized or lead-only decisions |
|---|---|---|---|
| M0 authority and baseline | `F/xhigh` | `M`: manifest/doc drift and repeated checker runs; `P`: route/source inventory and per-channel mapping | authority identity, measurement denominator, workload route, report interpretation, performance claim |
| M1 semantic IR and seam | `F/xhigh` | `P`: contract-test alternatives and fixture mapping; `M`: API inventory and docs | packet ownership, guest-borrow boundary, ticket state machine, public seam shape |
| M2 wgpu feasibility | `F/high` | three independent `P/high` API probes (Metal, Vulkan, D3D12); `M`: capability-matrix extraction | portable/fallback/blocked classification and architecture commitment |
| M3 raw-DPC slice | `F/xhigh` | `I/high`: separate owned-command or VI fixture modules; `P`: trace comparison tooling | FullSync, interrupt, guest-commit, direct-presentation and lifetime semantics |
| M4 RDP/framebuffer | `F/xhigh` | `I/high`: disjoint opcode/fixture families; `P`: hardware/manual evidence map; `M`: matrix bookkeeping | arithmetic/coverage authority, framebuffer coherence, divergence classification |
| M5 GBI/deferred RSP | `F/xhigh` | `I/high`: one microcode family per isolated path; `P`: command/fixture audit; `M`: generated inventory | admission boundary, self-load ordering, CPU-versus-GPU policy |
| M6 performance spine | `F/xhigh` | `I/high`: non-overlapping queue/arena/telemetry modules; `M`: repeated benchmark execution and receipt validation | thread ownership, boundedness, allocation accounting, causal performance conclusion |
| M7 certification | `F/xhigh` | `P`: fixture/minimizer analysis; `M`: matrix and receipt validation | release evidence, exact/bounded qualification, eye-gate interpretation |
| M8 feature parity | `F/high` per feature family | `I/high`: one non-overlapping feature family; `P`: public-control mapping; `M`: inventory/docs | denominator changes, live/recreate semantics, cross-feature regressions |
| M9 coherence optimization | `F/xhigh` | `I/high`: isolated observer/fuzz module; `M`: repeat harnesses | every CPU/DMA/VI observation, epochs, races and commit order |
| M10 platform/cutover | `F/xhigh` | `P`: per-platform bring-up, `M`: packaging and receipt checks | support claim, default flip, rollback, removal of C++ oracle |
| M11 modernization | `F/high` | `I/high` per optional feature; `M`: documentation and fixture inventories | reference-mode isolation and resource policy |
| M12 ray/path tracing | `F/xhigh` | `P/high`: API and capability research; `I/high`: isolated acceleration or denoiser path; `M`: metadata inventories | applicability classification, raster fallback, authored-pack semantics |

## Ownership and dispatch

Before dispatch, the lead writes a task card with one outcome, authority,
non-goals, exact writable paths, required profile, baseline command, and exit
gate. The card must state whether the result is a candidate observation or a
claimable result. A delegate reports the command output and the first failing
invariant; it does not silently broaden scope.

Before cutting a clean integration branch, run a consumer-closure preflight:
search every new manifest/schema/module name in the dirty source slice and
list the exact build files, dependency declarations, generated artifacts, and
lint hooks required for the slice to compile and validate from `main`. Record
that `integration_paths` closure in the ticket. A checker discovering an
unlisted consumer blocks the transplant before files are copied; the lead then
either widens the coherent ticket once or creates an explicit prerequisite.
This prevents repeated one-file-at-a-time scope expansion during integration.

Every behavior ticket also names its parity-ladder row IDs. The implementation
delegate may add fixtures and backend adapters, but only the lead may promote a
row from `RUST_PENDING` after confirming the test executed the declared
observable and authority. Review rejects `ignore`, capability-based silent
skip, fixture-only success, or an RT64 pixel match used to close a stronger
memory/timing contract. A merge may reduce the pending denominator; it may not
rename, delete, or weaken rows to look complete.

Every card is also a status ticket. Its durable fields are `id`, `milestone`,
`objective`, `profile`, exact `model`, `owner`, `authority`, `dependencies`,
`writable_paths`, `non_goals`, `baseline`, `exit_gate`, `state`, `findings`,
`verification`, optional `external_issue`, `blocker`, and `next_action`.
Allowed execution states are
`READY`, `RUNNING`, `READY_FOR_REVIEW`, `BLOCKED`, `REJECTED`, and
`INTEGRATED`. A delegate may return only `READY_FOR_REVIEW` or `BLOCKED`;
only the integration lead may set `INTEGRATED` after independently checking
the diff and exit gate. `BLOCKED` names the first failing invariant, evidence,
what was ruled out, and the smallest next action. Status is copied into this
plan's live capsule/handoff before a session ends so a later session never has
to infer progress from an abandoned process or dirty directory.

Paths are exclusive during a wave. In particular, `crates/fn64-abi/` is a
serialized chokepoint, as required by `DELEGATION.md`. The active renderer
split is:

| owner lane | normal writable paths | may not concurrently change |
|---|---|---|
| authority/evidence | `docs/rt64-*`, `tools/check_rt64_*`, certification fixtures/receipts | renderer behavior or shared ABI semantics |
| semantic/frontend | `crates/fn64-render*` semantic contracts, IR, RSP/GBI decode | wgpu resource lifecycle or shell presentation |
| GPU/render | Rust GPU backend, WGSL, targets, VI, compositor | ABI scheduling and guest-memory ownership |
| integration/performance | `crates/fn64-shell/`, benchmark runner, platform receipts | overlapping feature implementation |
| ABI chokepoint | `crates/fn64-abi/` | any other `fn64-abi` writer |

The lane owning a fact also owns when that fact becomes authoritative. ABI
publishes committed emulator events; the neutral IR represents them; renderer
lanes interpret and execute them; integration/certification selects routes and
publishes evidence. A downstream lane may request a typed field from its owner,
but may not infer it later, duplicate its state machine, or pull backend policy
upstream. In particular, RT64/wgpu types, shader policy, framebuffer algorithms,
and presentation heuristics never enter `fn64-abi`, while emulator scheduling
and mutable guest-memory ownership never enter `fn64-render-ir`.

Read-only audits may fan out freely. Writers merge only after the lead has
reviewed the patch against the task card; reviewers do not edit a writer's
paths. A dirty user-owned path is excluded unless the owner explicitly assigns
it, even if a seemingly small change would help.

External Claude/Sonnet dispatch is additionally fail-closed: use an isolated
worktree, `--safe-mode`, an empty strict MCP configuration, no Chrome/web/
subagent tools, no session persistence, a cleared private-input environment,
an explicit cost budget and Bash allowlist, and a non-interactive permission
mode that permits edits only inside that worktree. The lead rejects any diff
outside `writable_paths`; agents cannot commit, push, merge, read external
private paths, or certify their own result. The broad interactive permissions
in `.claude/settings.local.json` are not an unattended-dispatch policy and
must never be inherited by a port ticket.

## Required orchestration loop

1. **Plan and partition (`F`).** Choose the next dependency-ready slice,
   assign profiles, reserve paths, and write task cards.
2. **Fan out evidence.** Use `M` for deterministic extraction/checking and
   `P` for independent audits or probes. Require source locations and commands
   in every response.
3. **Implement (`I` or `F`).** One writer changes one path set behind the
   already-decided contract. Fast tests are diagnostic only.
4. **Adversarial review (`F`).** Check clean-room provenance, diff scope,
   type-level invariants, error behavior, and whether the executed test
   actually proves the stated result.
5. **Integrate (`F`).** Resolve competing findings, run the relevant
   differential and reliability loops from clean processes, and merge one
   small evidence-cited ticket branch to `main`. Record its base/dependency,
   automated checks, and independent review, then immediately rebase/sync all
   dependent worktrees. A PR is optional; never accumulate unrelated
   dirty-tree work into a convenience merge.
6. **Record and close (`F`).** Update the live plan capsule, evidence ledger,
   and handoff with actual counts. Close a linked GitHub issue only after the
   branch is merged and those exit-gate facts are recorded. A blocked or
   rejected ticket leaves its issue open with the precise frontier; a
   superseded issue closes with the replacement ticket/issue link. A failed or
   incomplete result never becomes a lower-quality green claim.
7. **Improve (all, lead-owned).** Spend at most five minutes recording one
   avoidable friction, repeated mistake, or wasted wait from the ticket. Name
   its cause and either make one small reusable mechanism change (checker,
   task-card field, fixture, cached command, or ownership rule) or explicitly
   record `no_change`. Carry the resulting rule into the next ticket and remove
   obsolete steps when evidence makes them redundant. Improvement work may not
   consume more than ten percent of a slice or delay its next dependency-ready
   task; larger ideas enter the queue instead of becoming process theater.

## Escalate to the frontier lead immediately

- An allowed-source, licensing, provenance, or RT64-pin interpretation is
  uncertain.
- A delegate proposes a new public type, unsafe boundary, queue/thread model,
  fallback, or a way to suppress an error.
- Two probes disagree, a differential diverges, an identity field cannot be
  independently derived, or a benchmark route requires a capture/readback that
  perturbs the horizon.
- The change touches `fn64-abi`, shared memory, FullSync, guest commit,
  presentation ordering, a borrow crossing a thread, or a race/interleaving.
- A test passes fewer than its stated reliability bar, a performance result has
  unmatched controls, or an eye gate needs a human decision.

The escalation output is a compact frontier: source/trace, invariant, commands
run, what is ruled out, affected paths, and the decision needed. This is the
handoff unit for a new session.

## Claim and handoff format

Every active slice adds the following fields to the plan handoff:

```text
Delegation:
  lead: profile/model/effort; reviewer: profile/model/effort
  delegates: task, profile, read/write paths, and candidate-vs-claim status
  serialized paths: ...

Evidence:
  provenance/authority; exact commands; clean-process run counts;
  differential scope; performance pairing and instrumentation caveat

Decision frontier:
  the one question reserved for the lead, or "none".
```

This records why a lower-cost delegate was safe to use and prevents a later
session from treating its report as an unreviewed parity or speed result.
