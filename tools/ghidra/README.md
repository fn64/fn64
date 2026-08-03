# Ghidra T3 conformance spike

This directory contains a bounded, synthetic-only headless Ghidra experiment.
It does not load a ROM, use N64LoaderWV as a mapping authority, or establish
function ownership. It proves that the production adapter shape proposed in
`docs/DISCOVER-TOOLCHAIN.md` is executable:

N64LoaderWV is separately approved for interactive and headless first-contact
candidate work. This fixture deliberately keeps stock raw import as the
deterministic, bank-isolated production baseline and does not test the loader
extension.

- each bank is imported into a separate disposable project with raw
  `MIPS:BE:64:64-32addr:o32` bytes at an fn64-supplied VA;
- banks A and B deliberately occupy the same VA but call different functions;
- `unseeded` receives only the synthetic proven entry, while `seeded` receives
  an additional snapshot-derived entry and records that parent lineage;
- the post-script emits `function_entry`, contiguous `function_extent`, and
  entry-associated `function_body_range` candidates through
  `fn64.tool-adapter` schema v2; discontiguous gaps are never widened into a
  continuous extent; and
- `gate_tool_jsonl` passes the result through the real Rust parser and verifies
  completion and the canonical claim digest.

Run it with:

```sh
tools/ghidra/run-conformance.sh
```

For the deterministic validation bar, use a fresh mode-0700 series root; each
attempt and its logs are retained:

```sh
FN64_GHIDRA_SERIES_WORK="$OUT_OF_REPO_SERIES" \
tools/ghidra/run-conformance-series.sh 10
```

Override `GHIDRA_HEADLESS`, `GHIDRA_JAVA_HOME`, or `FN64_GHIDRA_WORK` for
another installation. The work directory must remain outside the repository.
The checked-in `.hex` inputs are handwritten MIPS instructions; all binaries,
projects, logs, and JSONL are materialized under `/private/tmp` by default.

This is not the full Decomp Pack exporter. The next bounded expansion should
export bank-qualified basic blocks, direct and indirect xrefs, switch records,
data objects, prototypes, decompiler types, and stack-frame candidates into the
canonical graph. Those artifacts are valuable inputs to a Decomp Pack even
when they cannot prove exact ownership. Exact owner derivation and recompiler
admission remain separate Rust proof stages; seeded Ghidra descendants cannot
corroborate their own seeds.

## N64LoaderWV first-contact lane

The approved loader fork is
[`fn64/N64LoaderWV`](https://github.com/fn64/N64LoaderWV), based on the
`jeremyw` fork and pinned to
commit `e484f187f2aab869e5e808e1cfcec23e2d4779c7`. Newly built artifacts are
candidates. Automated first-contact and A/B runs accept only the exact receipt
and extension digests pinned by `n64loaderwv-artifact-policy.json`; a
self-consistent caller-written receipt is not source provenance. The
conformance runner rejects a checkout
whose `origin` and origin-tracking refs do not satisfy the checked-in source
policy, builds only the approved immutable commit and tree, and records the
repository, policy, commit, tree, source archive, and extension in its receipt.

### ROM identity

Build the path-free identity helper once, then pass its absolute executable
path to first-contact runs:

```sh
cargo build -p fn64-discover --bin rom_identity
FN64_ROM_IDENTITY="$PWD/target/debug/rom_identity"
export FN64_ROM_IDENTITY
```

`rom_identity ROM` accepts z64, n64, or v64 input and emits one
`fn64.rom-identity` schema-version-1 JSON object containing the normalized ROM
SHA-256, source byte order, byte length, and entry point. The first-contact
runner requires this helper and keys its per-ROM workspace by the normalized
digest. It does not substitute the raw-file digest for byte-swapped input.
The helper rejects symlinks, non-regular files, and inputs larger than 128 MiB
before allocation.

### Pinned synthetic conformance

Use a clean Ghidra installation without an install-wide N64LoaderWV extension:

```sh
N64LOADERWV_CHECKOUT="$LOADER_CHECKOUT" \
N64LOADERWV_COMMIT=e484f187f2aab869e5e808e1cfcec23e2d4779c7 \
FN64_GHIDRA_WORK="$OUT_OF_REPO_WORK" \
GHIDRA_INSTALL_DIR="$GHIDRA" \
GHIDRA_JAVA_HOME="$JDK" \
tools/ghidra/run-n64loaderwv-conformance.sh
```

All named paths must be absolute. `GHIDRA_HEADLESS` may override the headless
launcher. `FN64_GATE_TOOL_JSONL` may name a prebuilt absolute Rust gate;
otherwise the script runs it through Cargo. The script archives exactly the
pinned commit, builds and checks the extension, verifies the extracted tree,
imports its synthetic fixture, proves the resolved loader class came from that
JAR, passes the resulting `fn64.tool-adapter` schema-version-1 JSONL through the
Rust gate, and writes a candidate `fn64.n64loaderwv-conformance.v2` receipt. Its attempt
directory also retains a digest-pinned extension ZIP for reuse. The immutable
archive is built under its `N64LoaderWV` project identity, and Gradle, its Java
verification processes, and headless Ghidra receive attempt-local home,
settings, cache, and temporary directories. This proves the loader/build/export
shape on the synthetic fixture but does not approve a new ZIP. Review its
retained evidence and update the checked-in artifact policy before downstream
use. Loader conformance has not yet exercised production snapshot-bound
ingestion.

A 2026-07-29 rebuild reproduced the loader class bytes but not the extension
or JAR digests because Ghidra's generated help files embed the build time. Do
not promote a rebuilt ZIP until the pending fork-side reproducible-help
normalization is reviewed and committed.

For consecutive clean runs, use a fresh mode-0700 series root outside Git:

```sh
FN64_GHIDRA_SERIES_WORK="$OUT_OF_REPO_SERIES" \
N64LOADERWV_CHECKOUT="$LOADER_CHECKOUT" \
N64LOADERWV_COMMIT=e484f187f2aab869e5e808e1cfcec23e2d4779c7 \
GHIDRA_INSTALL_DIR="$GHIDRA" GHIDRA_JAVA_HOME="$JDK" \
tools/ghidra/run-n64loaderwv-conformance-series.sh 10
```

The series runner accepts 1 through 100 attempts, stops at the first failure,
and writes a path-free aggregate receipt plus the exact digest of every
attempt receipt. Each attempt retains its own build, analysis, gate logs, and
memory-guard evidence.

### Review-only first contact

Reuse a conformance-built ZIP only with the receipt that binds it to the fn64
source policy. The workspace must already exist outside the repository:

```sh
FN64_ROM_IDENTITY="$ROM_IDENTITY" \
GHIDRA_INSTALL_DIR="$GHIDRA" \
GHIDRA_JAVA_HOME="$JDK" \
tools/ghidra/run-n64loaderwv-first-contact.sh \
    "$ROM" "$WORKSPACE" "$EXTENSION_ZIP" "$CONFORMANCE_RECEIPT" "$RDRAM"
```

All paths above must be absolute. `GHIDRA_HEADLESS` may override the launcher,
and the final RDRAM argument may be omitted; when present it must be exactly 4
or 8 MiB. The receipt and ZIP must exactly match the checked-in artifact-policy
digests and the `fn64.n64loaderwv-conformance.v2` source-policy fields. The
runner derives provenance from that approved pair; it does not accept an
independent caller-supplied commit or fabricated self-consistent receipt. It
installs the verified ZIP only under the attempt's isolated settings, verifies
the complete extracted tree and loader classpath, proves the resolved runtime
class/JAR identity, copies and replays both inputs, and retains the Ghidra
project for GUI review.
It prints the project, logs, inventory, and receipt locations.

The path-free outputs are a
`fn64.n64loaderwv-review-inventory.v2` JSON inventory marked
`candidate_only: true` and a `fn64.n64loaderwv-first-contact.receipt.v3` text
receipt. They contain digests, versions, counts, memory-block candidates, and
function body ranges, plus a count of root-reachable candidates. Each function also records whether Ghidra reached it
from the loader's named resident roots (`ramMain`, `bootMain`, or `pifMain`);
that reachability is a candidate-quality signal, not ownership proof. These
are review artifacts, not production-ingest authority.

On 2026-07-29 the approved artifact completed ten consecutive guarded Banjo
first-contact runs with identical install/runtime identities and receipt
digests. The live loader was `ghidra.GhidraClassLoader`; its private JAR and
class identities are recorded by the local receipts, not retained or rechecked
by repository tests.

Both N64LoaderWV scripts bound the process tree to a 1 GiB JVM/build heap, a
2 GiB RSS ceiling, at least 40 percent free memory, and 180 seconds wall time;
Ghidra analysis is limited to 120 seconds and one CPU. They also isolate the
subprocess environment, home, settings, cache, and temporary directories.
They do **not** provide an OS network sandbox, an OS filesystem sandbox, disk
quotas, or per-file size quotas. Run them only in an environment whose external
network and filesystem access policy is appropriate for the inputs and tools.
Every VW launcher also requires the selected `analyzeHeadless` to belong to
the selected Ghidra distribution; an override from another installation is
rejected before its ambient extensions can run.

### Isolated GUI profile

Launch interactive Ghidra with the same approved fork and artifact policy as
the automated lanes:

```sh
mkdir -m 700 "$OUT_OF_REPO_PROFILE_ROOT"
GHIDRA_INSTALL_DIR="$GHIDRA" GHIDRA_JAVA_HOME="$JDK" \
tools/ghidra/run-n64loaderwv-gui.sh \
    "$OUT_OF_REPO_PROFILE_ROOT" "$EXTENSION_ZIP" "$CONFORMANCE_RECEIPT"
```

Add `--prepare-only` after the receipt to verify and materialize the profile
without opening a GUI.

The runner verifies the pinned receipt/ZIP pair, requires `ghidraRun` to
belong to the selected distribution, and materializes a digest-named Ghidra
home. Every launch rechecks the complete extracted tree byte-for-byte against
the copied approved ZIP and scans the distribution/profile for any competing
`N64LoaderWVLoader` class. It launches with an
isolated settings, cache, temporary, and user-home path and a 1 GiB Java heap.
The profile is persistent for interactive work but must remain outside Git;
Ghidra settings and project history can contain game-derived paths. Reusing
the command revalidates the installed artifact before launch. This is the VW
ROM-ingest and human-review lane; fn64's independently materialized raw-bank
mapping remains the snapshot-bound authority.

### Snapshot loader A/B

`run-snapshot-loader-ab.sh` compares BinaryLoader and the approved fork over
the same fn64-materialized bank in separate projects. The first Banjo-Kazooie
Rev 1 boot-bank run on 2026-07-29 found 61 common exact-body functions and four
VW-only starts (114 additional body words), with no Binary-only start. Its
pre-analysis inventory shows only `0x80000400` was a loader-created external
entry point; the other three starts appeared after analysis under VW's memory
map. All four already existed in fn64's snapshot ledger, so this is a useful
loader differential, not new coverage or an answer-key true-positive grade.
The runtime-identity wiring was exercised once on the same Banjo boot bank on
2026-07-29: the live JAR/class digests matched the approved artifact. This A/B
runner change has not yet met the ten-run deterministic validation bar.

Keep this lane separate from the raw-bank production lane above. ROMs, RDRAM
dumps, projects, exports, and logs are game-derived: keep them in the per-ROM
out-of-tree workspace and never commit them.

A flat-ROM or RDRAM import may emit candidate diagnostics, but it must not be
ingested into a snapshot-bound `ToolClaimSetV1` merely because the loader
assigned virtual addresses. Production ingestion requires an independently
constructed fn64 bank identity and matching discovery-snapshot lineage,
including the normalized-ROM, bank-byte, and mapping digests. Translate each
claim through that exact bank input; overlays that reuse one VA remain separate
bank runs.

### Computed-flow candidates

`Fn64ExportComputedFlows.java` exports Ghidra-recovered `jr`, `jalr`, and
switch targets through `fn64.tool-adapter` schema v3 under the distinct
`control_flow_candidates` role. Every claim is bank-qualified and carries
`completeness: unknown`; an empty target vector preserves an independently
decoded site without pretending Ghidra proved it targetless. Ordinary
`jr $ra` returns are excluded, out-of-bank references remain diagnostics, and
the Rust adapter rejects unsorted, duplicate, unaligned, wrong-bank, or
pre-v3 computed-flow claims.

The exporter also has an opt-in raw-word audit mode for bytes that Ghidra left
undefined: add `-Dfn64.rawIndirectCandidates=true` to the isolated JVM options.
This mode can recover omitted register-indirect encodings, but it deliberately
emits broad candidate noise and is never enabled by the production snapshot
runner or treated as ownership evidence.

Build the strict gate, then run the handwritten positive/negative fixture:

```sh
cargo build -p fn64-discover --bin gate_tool_jsonl
GHIDRA_INSTALL_DIR="$GHIDRA" GHIDRA_JAVA_HOME="$JDK" \
FN64_GHIDRA_WORK="$OUT_OF_REPO_WORK" \
tools/ghidra/run-computed-flow-conformance.sh
```

The fixture contains constant `jr`/`jalr`, a three-target switch, an
unresolved `jr $a0`, and ordinary returns. Ten consecutive guarded Ghidra
12.1.2 runs on 2026-07-29 produced the same path-free receipt identity. The
private receipt is not retained or rechecked by repository tests.
Ghidra reports terminal constant `jr` thunks as calls, so `ghidra_via_call`
is provider semantics rather than ISA authority.

`compare_computed_flows` independently verifies the snapshot, materialized
bank, provider schema/lineage, and raw MIPS `jr`/`jalr` encoding before
writing a candidate-only differential. It freezes and revalidates a
`ToolClaimSetV1` internally but never changes native CFG or facts. Its first
OoT boot-bank run matched fn64's one exhaustive target exactly, found both
native-open sites with no recovered targets, and reported seven Ghidra-only
sites. Five of those sites lie in answer-key function `__osDevMgrMain` and two
lie in `__osException`; the former includes a six-target switch at
`0x8000390c`. This is evidence that Ghidra can expose code outside fn64's
current reachable closure, not authority to seed those functions.

For first-contact review, `join-review-computed-flows.py` joins a review
inventory to a schema-v3 computed-flow stream by function body ranges. Its
output remains candidate-only and reports disconnected sites and targets
without mutating native CFG or ownership facts.

## Snapshot-bound production ingestion

Build `ingest_tool_claims`, then run it under the memory guard with an absolute,
canonical, out-of-Git workspace and an output directory already inside that
workspace:

```sh
cargo build -p fn64-discover --bin ingest_tool_claims
scripts/memory-guard.zsh target/debug/ingest_tool_claims \
    "$SNAPSHOT" "$REQUEST" "$WORKSPACE" "$WORKSPACE/tool-claims.json"
```

The request is strict `fn64.tool-ingest-request` schema version 1:

```json
{
  "schema": "fn64.tool-ingest-request",
  "schema_version": 1,
  "runs": [{
    "bank": "bank-id",
    "jsonl": "provider.jsonl",
    "tool": {
      "name": "ghidra-headless-unseeded",
      "version": "12.1.2",
      "build_sha256": "64-hex tool-manifest digest"
    },
    "tool_artifact_manifest": "tool-manifest.json",
    "role": "function_boundary_candidates",
    "lineage_artifacts": [
      {"role": "tool_configuration", "path": "configuration.json"},
      {"role": "evidence_manifest", "path": "evidence.json"}
    ]
  }]
}
```

`tool_artifact_manifest` is itself strict JSON. Its byte digest must equal the
request's `tool.build_sha256`, and ingestion streams and verifies every listed
regular file rather than trusting the manifest's assertions:

```json
{
  "schema": "fn64.tool-artifact-manifest",
  "schema_version": 1,
  "tool_name": "ghidra-headless-unseeded",
  "tool_version": "12.1.2",
  "artifacts": [
    {"path": "artifacts/analyzeHeadless", "byte_length": 1234,
     "sha256": "64-hex file digest"}
  ]
}
```

Artifact paths are nonempty, strictly sorted relative paths beside the
manifest; absolute paths, `..`, symlinks, duplicate paths, length drift, and
digest drift are rejected.

Relative artifact paths resolve beside the request. The command independently
hashes the tool and lineage manifests, derives the exact bank identity and
discovery-snapshot lineage from the schema-v2 `ProgramSnapshotV1`, applies
bounded intake and aggregate-candidate limits, validates the frozen sidecar,
and publishes without overwriting an existing output. The loader first-contact
workflow does not produce this request; use the snapshot-bank path below.

If a function entry lies in the selected bank but its Ghidra body crosses the
bank boundary, the exporter retains the entry candidate, omits the invalid
single-bank extent, and records a `cross_bank_function_body:<pc>` warning in the
summary. Multi-bank composition must resolve that warning before an extent can
be promoted.

### Snapshot-bank runner

The caller first materializes one bank through fn64's snapshot pipeline. Build
the strict helpers, then run both Ghidra experiment modes over those exact
bytes:

```sh
cargo build -p fn64-discover --bin stage_snapshot_bank --bin ingest_tool_claims
FN64_STAGE_SNAPSHOT_BANK="$PWD/target/debug/stage_snapshot_bank" \
FN64_INGEST_TOOL_CLAIMS="$PWD/target/debug/ingest_tool_claims" \
GHIDRA_INSTALL_DIR="$GHIDRA" GHIDRA_JAVA_HOME="$JDK" \
tools/ghidra/run-snapshot-bank.sh \
    "$SNAPSHOT" "$BANK" "$MATERIALIZED_BANK" "$WORKSPACE" \
    "$PROVEN_BASE_SEED" "$SNAPSHOT_SEED"
```

All file and helper paths must be absolute; the workspace must be canonical,
caller-owned, mode 0700, and outside Git. `stage_snapshot_bank` rejects wrong
bank bytes, geometry, or seeds before publication. The runner uses separate
guarded BinaryLoader projects for unseeded and seeded modes, then invokes
the caller-supplied `ingest_tool_claims` to produce one candidate-only
sidecar. The runner copies the runner, manifest scanner, memory guard, stage,
and ingest helpers into the private attempt before use. A compact
orchestration manifest records their sizes and hashes, and each generated
mode tool manifest binds that manifest without counting multi-megabyte helper
binaries against the strict ingest artifact budget. A final source-and-copy
recheck rejects replacement during the run. This establishes which ingest
helper produced the sidecar; it does not make an arbitrary caller-supplied
helper trusted. The receipt binds
both configs, both provider streams, tool/evidence manifests, the complete
Ghidra distribution inventory, and all six memory-guard traces.

The runner owns exactly one foreground guarded phase at a time. On HUP, INT,
or TERM it forwards termination to that exact memory-guard process and waits
for the guard to finish cleaning its isolated process group before exiting or
allowing another launch. Interrupted attempts retain their logs, memory trace,
and a path-free `diagnostics/runner-interruption.json`; they do not publish a
successful bank receipt.

The paired seeded-plus-unseeded comparison above is the default. For a faster
candidate refresh that needs only stock Ghidra discovery, pass
`--unseeded-only` before `PROGRAM_SNAPSHOT`:

```sh
tools/ghidra/run-snapshot-bank.sh --unseeded-only \
    "$SNAPSHOT" "$BANK" "$MATERIALIZED_BANK" "$WORKSPACE" \
    "$PROVEN_BASE_SEED"
```

This explicit fast mode runs and ingests only the unseeded provider. It does
not accept or validate a snapshot seed: `stage_snapshot_bank --base-only`
validates only the required proven-owner base seed. Its schema-version-2 bank
evidence and final receipt use typed
`seeds: {"mode":"base_only","base_seed":...}` objects with no
`snapshot_seed`; the unseeded Ghidra config carries `snapshot_seed: null`.
Paired mode keeps its two-seed CLI and records
`seeds: {"mode":"paired","base_seed":...,"snapshot_seed":...}`. The fast
mode does not create seeded config, provider, guard, or tool-manifest
artifacts and does not complete the paired comparison. The receipt records
`execution_mode: "unseeded-only"`, `paired_comparison_complete: false`, and
only the completed unseeded hashes. Default receipts instead record
`execution_mode: "paired"`, both completed modes, and
`paired_comparison_complete: true`. Both modes remain candidate-only; neither
makes Ghidra claims authoritative.

Banks without a proven owner use the distinct no-seed candidate path:

```sh
tools/ghidra/run-snapshot-bank.sh --discovery-only \
    "$SNAPSHOT" "$BANK" "$MATERIALIZED_BANK" "$WORKSPACE"
```

For a complete producer workspace, `run-snapshot-workspace.py` is the bounded
sequential scheduler. Give it the private directory containing
`snapshot-workspace.json` and a separate mode-0700 output directory (empty for
a new queue, or the same validated queue directory when resuming). It
preflights every bank before launching Ghidra, runs exactly one bank runner at
a time, chooses discovery-only or unseeded base-only execution from the typed
producer seed object, and never runs the paired second analysis. Immutable
numbered attempts make an interrupted queue resumable; resume rehashes its
inputs, retained runner receipts, logs, Ghidra inventory, and transitive tool
artifacts before accepting prior work. The immutable request also binds the
queue script itself, so changed validation semantics require a fresh queue
workspace instead of silently resuming an older request.

```sh
tools/ghidra/run-snapshot-workspace.py \
    "$SNAPSHOT_WORKSPACE" "$QUEUE_WORKSPACE"
```

The default queue ceilings are 64 new launches, six hours, three attempts per
bank, eight ordinary failures, 16 MiB per stdout/stderr log, and 512 MiB per
attempt, with at least 2 GiB free disk. HUP, INT, or TERM stops further
scheduling and lets the active certified bank runner perform its bounded
cleanup. A terminal `queue-receipt.json` is published last only after every
bank has a validated candidate result. `candidate_queue_complete` means the
external candidate pass finished; it is not ROM recompilation completeness or
a completed paired comparison.

Before analysis, the runner hashes every regular non-symlink file beneath the
Ghidra installation into one canonical, path-free inventory artifact. It
rejects symlinks, special files, and any directory-traversal or file-read
error, then repeats the full content scan after analysis and fails if the
distribution differs. The content-addressed cache deduplicates an identical
inventory artifact, but every run still rehashes all distribution files; file
metadata is never accepted as a cache shortcut. The strict ingest verifies
the inventory artifact's length and SHA-256 through the tool manifest. The
runner's two fail-closed scans, rather than ingest, verify the individual
Ghidra files and JARs named inside that inventory.
The queue accepts the runner workspace only when it contains one attempt plus
the exact private `.fn64-ghidra-distribution-manifests` cache. Resume verifies
that the cache has one mode-0600 file named by, and byte-identical to, the
receipt-bound distribution-manifest digest; extra entries and symlinks fail.

## Measured conformance

On 2026-07-28, Ghidra 12.1.2 and OpenJDK 21 completed ten consecutive guarded
clean runs through `run-conformance-series.sh` using the preceding schema-v1
exporter. Each run created three isolated projects, verified the bytes actually
mapped by Ghidra, carried snapshot lineage in seeded and unseeded modes, passed
the Rust JSONL gate, and deleted its projects. Each v1 stream had exactly one
SHA-256 value across all ten runs:

- bank A seeded: `062377050bbabfd7ad34e8f968608cee83e859342a8af22c5ff3fe88d9b6bc08`
- bank A unseeded: `9beaec498da4c35af821ea662dec1e46a8b532f3084ca248e0a7330eba51e4e6`
- bank B unseeded: `748246baa3b4bd9fadc3466a6926f3156894c7eb636666e3bcc9661f47099461`

The series evidence is retained outside the repository. Observed per-Ghidra
process-tree peaks stayed below 500 MiB in the preceding smoke run; every
series process remained subject to the 2 GiB/free-memory/time kill policy.
These hashes do not certify the schema-v2 discontiguous-body exporter; its
production retry and conformance series must record new evidence.
`Fn64ExportCandidates.java` keeps its historical function-boundary output by
default. Setting `FN64_GHIDRA_EXECUTABLE_RANGES=1` additionally emits
candidate-only `executable_range` claims for execute-permission memory blocks,
clipped to the requested bank interval. These ranges are evidence for image
discovery, not proof that every byte executes.
