# Corpus operations: stage receipts and dashboard

Status: proposed implementation plan for K08 / T3

## Purpose

fn64 has useful discovery, rebuild, CPU-recompile, boot, renderer, and
title-specific evidence, but no single current per-ROM outcome. This plan
defines one without making a dashboard into a source of truth or treating a
successful early stage as gameplay or fidelity evidence.

The corpus-operation system answers, for each exact normalized ROM image:

1. What was attempted, against which fn64 candidate and toolchain?
2. What is the highest stage whose evidence is currently valid?
3. What exact loud frontier prevented the next stage?
4. Which failure mechanisms, resource limits, runtime gaps, or renderer gaps
   block the most titles from advancing?

It implements K08's public, content-free compatibility-stage matrix and T3's
requirement that discovery, pack, compile, boot, and sustained play each
publish a typed result or an exact loud frontier.

## Non-goals

- It does not put ROMs, game-derived source, screenshots, traces, input
  schedules, local paths, or generated game output in Git.
- It does not replace the existing discovery snapshot/receipt contracts or
  loosen their proof boundary.
- It does not infer that `unsupported=0`, a presented field, or an automated
  route proves a whole-ROM, playability, or fidelity claim.
- It does not schedule arbitrary title-specific work automatically. Workers
  execute a frozen campaign; promotion policy remains explicit.

## Authority and storage

Receipts, not the dashboard, are authoritative. A private campaign directory
contains immutable inputs and append-only results:

```text
campaign/
  manifest.json                 # frozen ROM identities and policy
  receipts/<rom-sha>/<attempt>/ # one or more stage receipts
  artifacts/<rom-sha>/<attempt>/ # private logs, generated output, traces
  dashboard.json                # derived, reproducible view
  dashboard.html                # derived convenience view
```

`manifest.json` names only opaque `rom_id`s and normalized SHA-256 values;
the local machine separately maps those identities to private ROM paths. A
campaign is bound to an fn64 revision, dirty-state identity, toolchain,
features, runner policy, and resource policy. A changed input starts a new
campaign rather than silently replacing an old result.

The checked-in implementation owns schemas, validators, fixture receipts,
the aggregator, and dashboard renderer. Only content-free, validated summary
reports may be published. Existing reports remain historical evidence until
they are imported as receipts that pass this plan's validation.

## Stages and claims

Stages are ordered but not interchangeable. A later receipt is valid only if
it binds the required predecessor identities.

| Stage | Required result | Supports this claim | Does not support |
|---|---|---|---|
| `discover` | normalized identity; validated snapshot/summary; mapped and proven-code measures; open frontier | fn64 observed this ROM's structural discovery result | full-code or function recovery |
| `pack` | validated recompiler-pack identity and bank/geometry accounting | the stated proven material is eligible for emission | compilation or execution |
| `recompile` | emitted-runner identity; real compiler success; bounded probe; exact/block/dynamic/unsupported totals | the stated recovered CPU model compiled and passed its probe | boot, whole-game closure, graphics, or audio |
| `boot` | ROM-bound context/build; bounded launch; ordered named milestones | the declared boot scenario reached those milestones | interactivity or playability |
| `interactive` | deterministic input schedule; controllable-state checkpoint; video, input, and declared audio checks | the stated scenario reached an interactive state | a usable external play-test build or fidelity |
| `playtest_ready` | reproducible launch bundle, controls, known limits, smoke route, watchdog/diagnostics, and no unreported loud frontier on that route | another person can launch and test the declared scenario | broad compatibility or fidelity certification |
| `fidelity` | named reference/provenance, synchronized checkpoints, comparison metrics, and renderer/runtime scope | agreement in exactly the measured dimensions and scenarios | universal faithfulness |

`pack` is retained as a distinct stage even when the first implementation runs
it inside the CPU gate: its identity must remain visible so discovery and
emission failures do not share one opaque `recompile` status.

## Canonical receipt envelope

Every stage writes canonical JSON with this envelope. Stage-specific payloads
are nested under `result`; tools must reject unknown schema major versions,
missing predecessor identities, malformed digests, and self-contradictory
counts.

```json
{
  "schema": "fn64.corpus-stage-receipt.v1",
  "receipt_id": "sha256:...",
  "campaign_id": "fn64-corpus-pilot-v1",
  "attempt_id": "discover-0001",
  "stage": "discover",
  "rom": { "id": "pilot-001", "normalized_sha256": "..." },
  "candidate": {
    "git_commit": "...",
    "worktree_sha256": "...",
    "toolchain": "rustc ...",
    "features": ["..."],
    "runner_sha256": "..."
  },
  "predecessors": [],
  "policy": {
    "wall_time_ms": 0,
    "peak_rss_bytes": 0,
    "disk_bytes": 0,
    "retry_class": "none"
  },
  "outcome": {
    "kind": "passed",
    "frontier": null,
    "exit": { "code": 0, "signal": null }
  },
  "result": {},
  "artifacts": [{ "kind": "stdout", "sha256": "...", "visibility": "private" }],
  "started_at": "2026-09-13T00:00:00Z",
  "finished_at": "2026-09-13T00:00:00Z"
}
```

`candidate.runner_sha256` is required only once a stage invokes a generated
runner or shell. The envelope never stores a path or raw artifact; artifact
digests let a private operator retrieve and verify the retained material.

### Outcome vocabulary

Exactly one outcome applies:

- `passed` — this stage's narrow stated condition holds.
- `frontier` — fn64 deliberately refused an unproven or unsupported condition;
  `frontier` supplies a stable machine-readable kind and location.
- `resource_limit` — the frozen time, RSS, disk, or worker limit stopped a
  correctly launched job; this is not a product failure or a pass.
- `invalid_input` — identity, format, or prerequisite contract failed.
- `infrastructure_failure` — worker/tool invocation failed before a
  trustworthy stage result; only this class receives automatic retry.

The taxonomy is closed per schema version. A tool must not turn an unfamiliar
diagnostic string into `frontier`; it emits `infrastructure_failure` until a
typed classifier is added and tested.

### Required stage payloads

`discover.result` records physical/materialized/mapped/proven-code measures,
qualified owner and candidate measures when applicable, open-frontier counts
by kind, snapshot identity, and explicit denominator definitions.

`pack.result` records input snapshot identity, included/excluded banks and
bytes, block/word counts, geometry validation result, pack identity, and every
exclusion with a typed reason.

`recompile.result` records emitted source bytes, compile units, compile and
probe completion, exact-AOT bytes, block-AOT bytes, dynamic destinations,
unsupported destinations, total destinations, generated-runner identity, and
the pack identity. These are coverage of the stated pack, never a ROM-wide
denominator unless the pack proves that scope separately.

`boot.result` records build and boot-context identities, renderer mode,
ordered milestones, watchdog horizon, guest progress, graphics/audio/input
activation observations, named traps/fallbacks, and the route/config identity.

`interactive.result` adds a digest-bound input schedule, controllable-state
checkpoint, video/input/audio observations, duration, progress identity, and
the specific unsupported/trap census during the scenario.

`playtest_ready.result` adds a launch-bundle identity, controls/config schema,
known-limit identifiers, smoke-route receipt, diagnostic-bundle schema, and
the declared test surface. It must name its renderer and enhancement modes.

`fidelity.result` adds reference identity and provenance, synchronization
points, comparison dimensions and thresholds, observed differences, and an
explicit scope statement. A screenshot hash alone is insufficient.

## Aggregation and current status

The aggregator is deterministic and has no ROM access. Given one campaign
manifest and its receipts it must:

1. validate every envelope, stage payload, digest format, and predecessor edge;
2. select one attempt per `(rom, stage, candidate, policy)` using a manifest
   ordering rule: newest *valid* completed receipt wins, ties are an error;
3. refuse a passed child whose selected predecessor is absent, invalid, or
   mismatched;
4. derive the highest contiguous passed stage, never the highest stage name
   present;
5. retain all non-selected attempts in an attempt history rather than deleting
   failures;
6. calculate every numerator and denominator from selected receipts, and
   publish count reconciliation checks.

The dashboard labels a result **stale** when its candidate or policy differs
from the active campaign. Stale evidence remains inspectable but contributes
to neither current pass counts nor the next-stage queue.

For each ROM the status is one of `not_run`, a contiguous highest passed
stage, or `blocked_at_<stage>`. `resource_limit` and
`infrastructure_failure` are visible separately from technical frontiers;
they never become “unsupported.”

## Dashboard

The first dashboard is static HTML plus canonical JSON, generated locally and
safe to attach to a private campaign report. It has no ROM access and embeds
no private log bodies. It provides:

1. **Stage matrix:** one row per ROM identity; stage badges; highest valid
   stage; current blocker; elapsed/RSS/disk; candidate and policy identity.
2. **Progress summary:** counts and rates for every stage transition, with the
   eligible denominator printed beside each rate. “190 recompile passes” is
   never displayed without “of which eligible set.”
3. **Frontier clusters:** stable failure kind plus optional classifier version,
   affected ROM count, first blocked stage, and links to receipt IDs. Cluster
   by typed evidence, not shared human log text.
4. **Resource view:** p50/p95/max wall time, RSS, disk, emitted size, and
   compile units by stage and outcome; this drives scheduling limits.
5. **Runtime/WGPU view:** boot and later-stage blockers grouped by device,
   audio, input, storage, RSP, RDP, VI, presentation, or renderer mode. It
   shows stage advancement evidence, not inferred root cause.
6. **Play-test queue:** only titles whose selected receipts pass
   `interactive`; displays missing admission requirements for the next stage.
7. **Evidence drawer:** receipt identity, exact claim/nonclaim, predecessor
   chain, artifact digests, tool command class, and repeat count.

The JSON renderer and HTML renderer consume the same validated in-memory
model. Tests must prove that a missing predecessor, altered digest, unknown
outcome, duplicate winner, or inconsistent count makes generation fail rather
than producing an optimistic dashboard.

## Scheduling and escalation

The campaign runner is deliberately simple: discovery workers fan out broadly;
compile workers receive isolated output directories and a measured RSS/disk
budget; boot/render workers have smaller concurrency and a watchdog. A
content-addressed stage key includes ROM digest, candidate identity, stage,
policy, and predecessor receipts, so valid work is reusable and stale work is
detectable.

Normal failure handling needs no model. The coordinator groups receipts by
typed frontier. A low-cost classifier may propose a new diagnostic grouping,
but cannot change a receipt's outcome. Escalate architectural or research
review only for a novel/high-impact cluster, contradictory evidence, or a
proposed change to runtime/WGPU/discovery proof boundaries.

## Implementation slices

1. Define Rust/JSON schemas, canonical encoding, validation, and fixtures for
   `discover`, `pack`, and `recompile`; adapt the existing
   `fn64.rom-recompile-report.v1` rather than parsing terminal output.
2. Add the deterministic campaign aggregator, JSON summary, reconciliation
   tests, and a small static HTML stage matrix.
3. Run a 12–20 image stratified private pilot, preserving every typed outcome
   and measuring resource budgets before choosing full-corpus concurrency.
4. Add `boot` receipts and a curated boot panel. Do not invent title-specific
   input schedules merely to inflate a later-stage count.
5. Add `interactive` and `playtest_ready` only after two or three real titles
   expose the actual launcher, controls, diagnostics, and known-limit contract.
6. Add fidelity receipts after the runtime/WGPU comparison contract supplies
   a named reference and scenario-specific denominator.

Each slice must update the public compatibility matrix and keep raw
ROM-derived artifacts private. A dashboard is release evidence only after its
campaign is rerun against the candidate it names.

