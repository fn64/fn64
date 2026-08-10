# Discovery pipeline improvement handoff

Status: execution plan for a fresh session

Canonical architecture and measurements: [`../DISCOVER-PLAN.md`](../DISCOVER-PLAN.md)

Scope: improve ROM discovery recall, semantic usefulness, and usability without
weakening fn64's proof boundary

## Goal

Move `fn64-discover` from a promising ROM-first recompiler front end toward a
repeatable workflow that helps a port builder answer three questions:

1. What did fn64 prove?
2. What remains unresolved, and what evidence would resolve it?
3. What can execute now through `exact_aot`, `block_aot`, or an explicit
   `dynamic_mips` fallback?

This plan does not make “universal discovery” a milestone. Every phase must
improve a named denominator while preserving explicit unknowns and the
`wrong == 0` admission posture.

## Read before changing code

Follow the repository `AGENTS.md` read order. For this work, then read:

- [`../DISCOVER-DESIGN.md`](../DISCOVER-DESIGN.md)
- [`../DISCOVER-PLAN.md`](../DISCOVER-PLAN.md), especially “Current measured
  baseline,” “Multi-view analysis,” and the experiment impact ledger
- [`../DISCOVER-OWNER-PROOF.md`](../DISCOVER-OWNER-PROOF.md)
- `crates/fn64-discover/src/cfg.rs`
- `crates/fn64-discover/src/resolve/mod.rs`
- `crates/fn64-discover/src/tool_claims.rs`
- `crates/fn64-discover/src/candidate_corroboration.rs`
- `crates/fn64-discover/src/candidate_relation_report.rs`
- `crates/fn64-discover/src/spimdisasm_adapter.rs`
- `crates/fn64-discover/src/spimdisasm_reference.rs`
- `crates/fn64-discover/src/coverage.rs`

The clean-room restrictions remain in force. In particular, do not install,
invoke, read, vendor, or adapt m2c. External permissive tools remain candidate
providers, never behavioral authorities.

## Current frontier to preserve

The canonical plan records these baselines. Re-measure before citing them as
current after code changes.

- A nine-ROM, five-engine cold panel completed in about 118–119 seconds with
  an observed process-group peak around 578 MB. Eight ROMs had zero statically
  unsupported destinations; Banjo-Kazooie retained one.
- Cold rebuilds can prove and round-trip large code regions, including every
  measured region in the retained Majora's Mask and Clay Fighter runs, while
  the cold rebuild snapshots still claim no exact historical function owners.
  Code classification and function reconstruction are different denominators.
- The candidate-only spimdisasm grade measured 91.99% precision / 97.64%
  recall for entries and 80.53% exact extents on its recorded corpus case.
- The retained Ghidra experiment found 952 words beyond the native Banjo boot
  baseline, but analyzer completeness is unknown and no native authority was
  created.
- The relocation proxy grade reported 50.4% of 280 recovered references
  misclassified.
- Open indirect transfers, runtime-built tables, loader diversity,
  compressed materialization, and production-wide `dynamic_mips` coverage
  remain the main arbitrary-ROM blockers.

## Non-goals

- Do not replace fn64's decoder, fact database, owner proof, or grading with an
  external tool.
- Do not promote a candidate because two heuristic providers agree.
- Do not treat executed PCs as exhaustive reachability evidence.
- Do not use a byte-exact assembly round trip as proof of a historical
  function boundary.
- Do not build a general decompiler inside fn64. Export evidence to established
  analyst tools instead.
- Do not mix RSP, RDP, or runtime-fidelity completion into static CPU discovery
  metrics.

## Recommended first session: candidate impact and explanation

This is the first implementation slice. Do not start the trace producer or
dynamic fallback expansion in the same change.

### Outcome

Given one validated discovery snapshot and one or more validated external-tool
claim sets, emit a deterministic report that separates:

- claims already explained by native proven reachability;
- claims overlapping native candidate/supported evidence;
- new candidate entries and extents;
- extra CFG blocks and words reached only after candidate seeding;
- conflicts, invalid geometry, and unchanged open indirect sites;
- the native evidence class required before each candidate could advance.

The report must not mutate `FactDb`, owner proof, a Recompiler Pack, or an
execution-closure classification.

### Likely implementation surface

- Extend `candidate_relation_report.rs` rather than creating a second relation
  model.
- Reuse the move-only validation capability in
  `candidate_corroboration.rs`.
- Reuse `candidate_cfg_probe.rs` for bounded exploratory reachability.
- Add a small CLI or gate under `crates/fn64-discover/src/bin/` that renders
  stable text plus optional canonical JSON.
- Keep answer-key grading in a gate binary. Production reporting must never
  open an answer key.

### Required tests

- A tool-only entry stays candidate-only.
- A native proven call is reported as native corroboration, not provider
  promotion.
- Two agreeing tool sources still do not create native authority.
- Same-VA claims in different banks remain isolated.
- Wrong snapshot, bank, geometry, digest, schema, or provider lineage fails
  closed.
- Duplicate, overlapping, and out-of-bank extents remain typed failures.
- Rendering is byte-identical across ten consecutive runs.

### Done gate

Record before/after provider deltas on at least one held-out bank:

```text
native_reached_entries
provider_entries
already_explained
new_candidate_entries
candidate_seeded_blocks
candidate_seeded_words
new_open_indirect_sites
invalid_or_conflicting_claims
authority_promotions=0
```

The slice is done only when the report is useful for choosing the next native
corroboration mechanism and `authority_promotions` is mechanically zero.

## Phase 2: contributor-facing frontier report

Turn existing typed blockers into an ordered work queue. Add an `explain`
surface only after its contents come from canonical receipts rather than
re-parsing log text.

For every unresolved item, report:

- normalized ROM identity without exposing a path;
- bank and PC/range;
- current evidence and proof state;
- blocker kind;
- downstream destinations or bytes held back;
- the admissible evidence class that could resolve it;
- the gate that would verify the change.

Rank by impact, not confidence: unsupported destinations first, then blockers
holding back the most reached code, then candidate-only recall opportunities.
Keep a stable machine-readable schema and derive human text from it.

Done means a new contributor can select one blocker without reading the whole
discovery plan, and the report never recommends a forbidden silent fallback.

## Phase 3: repeatable trace producer

Build the missing producer for the existing trace ingestion schema.

Required observations:

- bank-qualified executed PCs;
- direct and indirect transfer source/target pairs;
- PI DMA source, destination, length, and activation order;
- runtime table writes relevant to callbacks or dispatch;
- scenario, frame/cycle horizon, natural-versus-forced label, and executable
  identity.

Start with one scripted black-box scenario. Retain raw private-ROM traces
outside git and commit only content-free schemas, synthetic fixtures, and
permitted receipts.

Trace evidence proves existence, not completeness. A target observed once may
become a supported callable candidate; it cannot close an indirect target set
without an independent finite-domain proof.

Done means ten deterministic scenario runs produce schema-valid observations,
the same normalized identities, and a stable folded fact summary. Any
concurrency-sensitive producer fix requires twenty clean runs.

## Phase 4: indirect-flow improvements

Use trace evidence to guide, not replace, static analysis:

1. Add context-sensitive bounded value sets where caller identity changes a
   callee's return or indirect targets.
2. Add proof-carrying return/noreturn summaries so fake fallthrough does not
   inflate reachability.
3. Generalize compiler-specific jump-table and callback-registration
   recognizers after a compiler-family classifier exists.
4. Grade every new resolver on near-miss data/code cases and open-target
   negatives.

Every resolver returns `Exhaustive`, `Bounded`, or `Open`. Only `Exhaustive`
may feed CFG closure. Track precision, recall, newly reached words, exact-owner
delta, and wrong-owner delta separately.

## Phase 5: relocation recovery

Improve the recorded relocation baseline through differential evidence:

- compare one overlay materialized at two load addresses; or
- compare matched code across ROM revisions;
- identify values that move by the exact load delta;
- reconstruct and validate HI/LO pairs;
- bind source word, relocation kind, addend, target bank/object, and both input
  identities into the receipt.

Grade relocation kind, target, and addend separately. A symbol-key proxy is
not a complete relocation oracle, so retain the proxy limitation in the
result.

Do not adopt the mechanism unless the misclassification rate falls on a
held-out case without weakening function-entry or owner `wrong == 0` gates.

## Phase 6: Decomp Pack and tool exports

Emit an analyst-oriented view without strengthening the Recompiler Pack:

- matching assembly;
- proven and candidate symbols with provenance;
- xrefs and relocations;
- typed data objects, strings, floats, and pointer tables where established;
- function prototypes and stack-frame candidates;
- generated Splat configuration and Ghidra import material.

Round-trip imported analyst changes as digest-bound candidate evidence. Human
or tool edits never become runtime authority merely because they make a clean
decompilation project.

Done means a clean generated project opens in at least one supported analyst
tool, preserves bank identity for same-VA overlays, and reassembles all emitted
authoritative bytes exactly.

## Phase 7: loader and compressed-materialization breadth

Generalize beyond currently recognized loader families:

1. Correlate cache operations, PI DMA, wrapper arguments, and execution traces.
2. Identify the ROM's own decompressor as code through ordinary fn64 evidence.
3. Execute it in the instrumented MIPS lane.
4. Bind source bytes, decompressor identity, input state, output bytes,
   destination mapping, and deterministic replay into a materialization
   receipt.
5. Admit a bank only when the transform reproduces its bytes and mapping.

Use a blind holdout from a loader family not used to write the recognizer.
Report boot-only models loudly; zero code-like residue in a boot-only model is
not whole-ROM coverage.

## Phase 8: production `dynamic_mips` closure

Finish the explicit fallback so static uncertainty is runnable and observable:

- cover remaining CPU instruction and exception classes, including the named
  FPU/COP0/exception frontier;
- route every admitted dynamic destination through the live executor;
- retain bank, PC, backing identity, fallback reason, and execution counts;
- feed repeated executions back into trace-guided AOT candidates;
- preserve holes, bad mappings, and unsupported device behavior as loud
  faults.

The release gate remains zero `unsupported` destinations. A nonzero
`dynamic_mips` count is allowed only when every destination is backed,
instrumented, and reported; it is not a silent success path.

## Cross-phase engineering improvements

Apply these when measurements show they matter:

- Content-address and cache immutable per-bank analysis.
- Parallelize independent banks while retaining deterministic sorted output.
- Separate discovery time, generated-code compilation time, and execution
  time in reports.
- Expand the blind corpus across engine, compiler, loader, compression, and
  overlay families.
- Add public permissively licensed homebrew/system-test fixtures so more of the
  breadth claim can be independently reproduced without private ROMs.
- Correct stale design-status prose in the same commit when implementation
  has overtaken it. In particular, recursive entrypoint traversal is
  implemented even though one dated paragraph in `DISCOVER-DESIGN.md` still
  calls it design-only.

## Validation for every implementation slice

Run the narrow tests first, then the repository doc sweep:

```sh
cargo test -p fn64-discover
python3 scripts/lint-docs.py
git diff --check
```

Run the relevant real-ROM or held-out gate only when its required private
inputs are available. Report a skipped private gate as not verified, never as
passing. Deterministic fixes require ten consecutive clean runs. Concurrency
fixes require twenty or more and a comment naming the closed interleaving.

Each commit records:

- exact input/gate and algorithm version;
- before/after denominators;
- whether facts remained candidate-only or gained native authority;
- wrong-result count;
- run count;
- unresolved frontier after the change.

## Fresh-session kickoff prompt

Use this to begin the recommended first slice:

> Read `AGENTS.md`, `README.md`, `docs/DESIGN.md`, and
> `docs/plans/discover-pipeline-improvements.md` in that order, then inspect
> the candidate/tool files named by the plan. Implement only “Recommended
> first session: candidate impact and explanation.” Preserve the clean-room
> protocol and candidate-only external-tool boundary. Before editing, report
> the existing relation/probe APIs and the smallest missing mechanism. Add
> deterministic synthetic tests, run the fn64-discover test suite and doc
> linter, and report private-ROM gates as unverified if their inputs are not
> available.
