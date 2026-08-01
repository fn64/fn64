# Fast loop: cache what's deterministic, share the build

Two tooling levers cut per-job time (measured 2026-07-16):

## Mechanical discovery feedback

For a previously built `fn64-discover` binary, use the compact feedback path
before requesting a full JSON artifact. It runs the same selected mechanical
strategy and trace fold, but deliberately skips the byte-granular ledger and
the full fact-log serialization. The wrapper compares a content-free summary
receipt across runs, enforces a per-run wall-clock bound, and emits only a
path-free timing receipt:

```sh
scripts/profile-discovery-loop.zsh \
  --bin /path/to/fn64-discover \
  --rom /path/to/local/game.z64 \
  --runs 3 --max-seconds 120
```

`--bin` can instead be supplied through `FN64_DISCOVER_BIN`. Optional evidence
and trace files are passed with `--evidence` and repeated `--trace`. The
underlying `fn64-discover --summary` output is observation-only: it contains a
normalized-ROM digest, strategy outcomes, coverage, and a self-hash, but no
paths, ROM bytes, fact log, ledger, or admission authority. Use the normal
`--out facts.json` route whenever downstream work needs the complete artifact.
The automatic strategy pass normalizes the ROM once, evaluates each mechanical
Phase-2 recovery once, then runs Phase-3 candidate harvest only on the selected
database. Strategy outcomes and the selected fact log remain deterministic;
losing strategies no longer pay the harvest cost merely to report their
mapping counts.

### Cold function-training loop

Build a sealed ROM-only training workspace once, then reuse its validated
identity for attribution experiments:

```sh
mkdir -m 700 /absolute/private/new-cold-workspace
cargo run -p fn64-discover --bin produce_snapshot_workspace -- \
  --training /absolute/private/game.z64 /absolute/private/new-cold-workspace

cargo run -p fn64-discover --bin validate_training_workspace -- \
  /absolute/private/new-cold-workspace

mkdir -m 700 /absolute/private/new-attribution-workspace
cargo run -p fn64-discover --bin attribute_known_functions -- \
  /absolute/private/new-cold-workspace \
  /absolute/private/dump.toml \
  EXPECTED_NORMALIZED_ROM_SHA256 EXPECTED_DUMP_SHA256 \
  /absolute/private/new-attribution-workspace
```

The output directory must be a new private workspace outside Git. The producer
publishes the schema-v4 workspace envelope with snapshot wire v6, a v3
address-space-aware candidate receipt, cold provider-set algorithm v3, and no
answer key. Candidate receipt v3 adds the typed
`SemanticCallableArgument` detector derived during composed authority closure;
it does not turn other structural candidates into authority. Run
`validate_training_workspace` before giving any later process a
label path; it streams the candidate receipt and each bank once and prints only
counts, elapsed time, and binding digests. A 2026-07-30 cached OoT sample
produced the pre-handler-provider schema-v3 baseline of 471 banks and a roughly
106 MiB workspace in about 46 seconds at 448 MiB peak RSS. The historical
snapshot-v4 handler-provider run retained the same 471 banks, peaked at 469 MiB, and its
cached key-free validation visited 5,721,664 bank bytes, 115,434 facts, and
13,196 candidates in 1.04 seconds. Reuse the sealed
workspace for scoring and miss-cluster iteration instead of rerunning ROM
discovery. The attribution command validates the entire cold workspace before
opening the digest-pinned label file, writes one create-new bounded report, and
uses `candidate_matched` rather than `found`: an exact cold candidate is not
yet proof of a function owner or extent.

The 2026-07-30 handler-table increment was also applied key-free to held-out
SM64 before its digest-pinned key was admitted: it added 30 exact bodies from
37 new candidates (two interiors and five outside) at 296 MiB producer peak
RSS. This is the minimum standard for adopting a learned detector: a target
key cannot influence the application artifacts, and the report must retain
both the gain and the newly introduced candidate errors.

The subsequent bounded prologue-prefix refinement recovered seven additional
OoT bodies and two held-out SM64 bodies with no matched-to-missed transition.
It also removed 99 and 33 interior candidates respectively by relocating the
same prologue evidence to an independently structural boundary; it did not
change proof state. Strict digest-pinned A/B artifacts retain the private
evidence; repository tests do not retain or recheck those artifacts.

Snapshot v5 then publishes typed semantic callback/thread entry claims that
the earlier closure retained only as roots. A fresh key-free OoT application
recovered one additional body and reduced `ProvenCodeNoEntry` from three to
two without another candidate error; held-out SM64 had no status or population
change. The focused semantic recovery passed ten consecutive clean runs. This
is a narrow authority repair rather than a reason to continue optimizing
candidate-entry recall ahead of executable-source and writer closure.

Do not use `gate_decomp_functions` as the cold producer: that historical gate
derives `code_end` from the answer key before it discovers candidates. It
remains useful for regression comparisons. For generalization claims, train on
the labeled corpus, freeze the mechanism, and validate it on a separately
sealed held-out ROM; the application run itself receives no target key or
target-specific constants.

For a repeatable corpus fold, use `scripts/cold-training-fold.py`. Its three
create-new phases are `prepare`, `freeze`, and `grade-heldout`; `--help` lists
the required manifest and digest pins. `prepare` creates and validates every
cold workspace, but admits labels only for the declared training IDs.
`freeze` binds a canonical mechanism receipt to that training receipt.
`grade-heldout` accepts the held-out label only through a separate,
freeze-bound admission receipt. The receipts bind stable private copies of the
ROMs, labels, and exact tool executables; subprocesses have bounded output and
run in isolated process groups owned and sampled directly by the orchestrator.
Each child group defaults to a 2,048 MiB aggregate-RSS ceiling and a 40%
system-free floor, sampled immediately after launch and then every 1,000 ms;
sampling failures, either threshold, or any same-group
descendant left after a successful leader exit kill the exact group and fail
the phase. `prepare` accepts `--max-rss-mib` and `--min-free-percent`, records
the selected limits in its receipt, and `freeze` carries them forward.
`grade-heldout` must use exactly those frozen values, so a later invocation
cannot silently loosen the envelope. The RSS/free signals are sampled rather
than kernel-enforced, so short overshoot between samples remains possible.
Before either attribution receipt is accepted, the frozen attribution
executable reparses the staged cold workspace and staged answer key, rebuilds
the exact report, and rejects any typed, relational, counter, or byte mismatch.
It returns only a bounded path-free validation summary containing the exact
report-byte digest; the orchestrator compares that digest with a stable,
streaming post-validation hash instead of parsing the potentially 128 MiB
report again. Final directory publication uses macOS exclusive rename and
fails if any destination appears before the rename completes.
Prepare and grade publish by atomic directory rename. This proves only
`held_out_key_admitted_by_orchestrator=false` through freeze; it is not a claim
that some process outside the orchestrator could not read the key. Exercise
the no-ROM adversarial harness with:

```sh
python3 scripts/test-cold-training-fold.py
```

### Rank the next discovery mechanism

After producing a digest-pinned attribution report, derive a bounded local
opportunity ranking without rerunning discovery:

```sh
python3 scripts/mechanism-opportunity-ranking.py single \
  --report /absolute/private/known-function-attribution.json \
  --expected-report-sha256 REPORT_SHA256 \
  --evidence-id oot --family zelda \
  --output /absolute/private/new-opportunity-ranking.json
```

The create-new, mode-0600 artifact is answer-derived diagnostic evidence for
training the *next* mechanism only. It cannot feed discovery on the current or
evaluated ROM. It ranks opaque ROM-local section/bank clusters by distinct
bodies and declared function bytes, retains full totals under `--top`, and
reports physically contiguous missed runs and matched-function bracketing.
Candidate proximity is explicitly an unbanked addressed approximation because
the addressed candidate identity has no bank. Single-report validation checks the
pinned file digest, strict ASCII schema/geometry/totals, and the report's
serde-compatible canonical digest, but does not reconstruct the report from
the cold workspace; that boundary is recorded in the output. Exercise it with:

```sh
python3 scripts/test-mechanism-opportunity-ranking.py
```

### Compare two attribution experiments

When an experiment changes a frozen mechanism, compare its two independently
produced, digest-pinned reports before promoting the result:

```sh
python3 scripts/mechanism-opportunity-ranking.py ab \
  --baseline-report /absolute/private/baseline-known-function-attribution.json \
  --expected-baseline-report-sha256 BASELINE_REPORT_SHA256 \
  --followup-report /absolute/private/followup-known-function-attribution.json \
  --expected-followup-report-sha256 FOLLOWUP_REPORT_SHA256 \
  --output /absolute/private/new-attribution-ab.json
```

The mode reuses strict report validation for both inputs, then rejects a ROM,
answer-key, answer-denominator, or non-marker body-denominator mismatch. Its
create-new mode-0600 JSON stays outside Git and contains only binding digests,
baseline/follow-up totals and deltas, body-status transition counts,
candidate-status deltas, and opaque detector population/addition counts. It
never publishes function names, addresses, source paths, or raw detector
labels. The report remains answer-derived diagnostic evidence, not discovery
input for either compared ROM.

The A/B v2 receipt records each input envelope and candidate-receipt identity.
Same-schema comparisons are labeled as such. An exact historical V1 baseline
may be compared with a current V2 report, but the result is explicitly
`cross_schema_unprojected_total_delta`: it measures the total observed change
and cannot by itself attribute that change to one mechanism or authorize
discovery.

### Discovery code test tiers

Use the smallest test artifact that covers the edit. A nextest expression still
asks Cargo to prepare every test target in the package; for `fn64-discover`
that currently means 134 targets even when the expression selects one test.
The supported loops are therefore:

```sh
# Unit/data-flow iteration: builds and runs only the library test binary.
scripts/guarded-cargo-test.zsh -p fn64-discover --lib FILTER

# One integration-test binary.
scripts/guarded-cargo-test.zsh -p fn64-discover --test TEST_TARGET FILTER

# Milestone/package gate: prepares and runs the complete nextest inventory.
scripts/guarded-nextest.zsh -p fn64-discover
```

Both wrappers serialize Cargo compilation explicitly and execute inside a
sampled 4 GiB process-group/40%-free-memory envelope. The test wrapper also
serializes libtest. The nextest wrapper binds `CARGO_BUILD_JOBS=1` because
nextest's own `-j1` controls test processes, not Cargo compiler concurrency.

## 1. `scripts/native-emit.sh` — cache the deterministic whole-ROM emit
`recompile_rom` emits a BIT-IDENTICAL 139MB Rust-recompiled crate for the same
(ROM + oot.toml + recompiler binary). Re-emitting per job wastes ~2-4s + 133MB.
`native-emit.sh` content-addresses the emit (hash of those 3 inputs) into
`/tmp/fn64-emit-cache/<hash>` and reuses it. Measured: **~2-4s emit -> 0.13s
cache hit.** Use it instead of calling recompile_rom directly:
```
./scripts/native-emit.sh --selftest
./scripts/native-emit.sh --dry-run
RECOMP_RS_DIR="$(./scripts/native-emit.sh)"   # emits on miss, instant on hit
```
The driver build and cache-miss emitter are separate 2048 MiB/40%-free guarded
process groups; the Cargo build is fixed at `-j1`. A failed guarded build or
emit exits loudly and cannot produce a cache-hit claim.

## 2. Shared `CARGO_TARGET_DIR` — reuse compiled deps across worktrees
Every worktree cold-compiles the same deps into its own `target/` (GBs, minutes).
A shared target dir lets jobs reuse each other's builds. Measured: a fresh
worktree building `fn64-runtime` with the shared target = **0.05s (full reuse)**
vs a cold compile. Set it for every job build:
```
export CARGO_TARGET_DIR=/tmp/fn64-shared-target
```
The boot and shell manifests now select live-IMEM LLE without translated-ucode
dependencies, so their lockfiles do not cross-pin a sibling fn64-audio checkout.

## The combined fast rs-boot loop
```
export FN64_GAME_DIR=/path/to/your/rom-derived/workspace  # no default: set it
export CARGO_TARGET_DIR=/tmp/fn64-shared-target
export RECOMP_RS_DIR="$(./scripts/native-emit.sh)"
export FN64_RECOMP=rs
# build via examples/oot-boot/rs/Cargo.toml (crate emit compiles in parallel,
# incremental after the modcrate fix) -> run ./oot
```

## Late gameplay state/task differential

`OOT_STATE_TRACE=1` revalidates the live `Play_Main` allocation on every swap
and reports control, player, and generated-C-grounded `RoomContext` load state.
To compare selected RT64 task indices without committing game data, add:

```sh
export FN64_GFX_TASK_DUMP=4149,4289
export FN64_GFX_TASK_DUMP_DIR="$PWD/.eyegate/r6"
```

Each report contains the `OSTask`, independent reference triangle count, full
F3DEX2 command walk, resolved segment/DL targets, and bounded content
fingerprints. `.eyegate/` is evidence-only and remains untracked.
Dispatch every rs-lane job with these exports; it reuses the cached
emit + the shared compiled deps instead of redoing both.

## RDRAM layout, writer topology, and live timing loops

The recurring native-word/guest-byte layout class has a sub-second structural
gate. Run both modes when touching any host/device/RDRAM boundary:

```sh
scripts/lint-rdram-layout.py --selftest
scripts/lint-rdram-layout.py
scripts/lint-writer-channel-topology.py --selftest
scripts/lint-writer-channel-topology.py
scripts/lint-compiler-memory-safety.py --selftest
scripts/lint-compiler-memory-safety.py
scripts/lint-wm-shard-dependencies.py --selftest
scripts/lint-wm-shard-dependencies.py
```

The writer-topology sweep locks the sealed `DmaMemory` implementation
denominator, the sole SI/SP device write sites, and the canonical ABI's exact
PI/SI/SP producer-to-notification mapping. It is intentionally structural: it
does not complete a writer channel because the installed program model still
does not bind host-function identities and effects strongly enough to prove
that every reachable device path uses the canonical adapter.

The WM shard sweep resolves every production-root dependency path to its leaf
package and requires exact agreement with the shared generator and prepared
materializer inventories. Its negative fixtures reject both a missing
resident-tail package and the obsolete pre-split `wm2000-block-shard-15`
identity without running Cargo or reading a ROM.

For a cold, compiler-side measurement of one generated WM2000 shard without
touching accumulated Cargo targets, use:

```sh
scripts/profile-wm2000-shard.zsh --selftest
scripts/profile-wm2000-shard.zsh --dry-run
ROM=/path/to/local/NWXE.z64 scripts/profile-wm2000-shard.zsh
```

The default is the historically worst overlay shard. The real run uses a fresh
retained target, one Cargo job, and the common 2 GiB/40%-free guard defaults.
Its summary is path-free and reports cold graph-plus-shard wall time, peak RSS,
generated source shape, and rlib size; the retained sanitized log contains the
finer build-script phase timings. The JSON also reports exact semantic-body
slot totals at the current 2 KiB subrunner boundary and the candidate 64 KiB
artifact boundary, so block-reuse ideas can be rejected before changing or
compiling an emitter. It never removes old targets or prints the ROM pathname.

Static-frontier and current-scorecard host binaries default dev debug info to
line tables (`CARGO_PROFILE_DEV_DEBUG=1`). Full debuginfo was measured at a
2,160 MiB process-tree sample and was terminated by the 2 GiB guard; line
tables completed the identical static producer at a 1,414 MiB peak. This does
not change generated guest code or receipt content, and an explicit caller
value still overrides the feedback-loop default.

The compiler-memory sweep fixes ordinary scoped production build/profile
defaults at 2048 MiB aggregate process-group RSS and a 40% system-free floor,
and rejects an unguarded Cargo compiler command in those entrypoints. Generic
local wrappers still accept explicit `FN64_GUARD_MAX_RSS_MIB` and
`FN64_GUARD_MIN_FREE_PERCENT` overrides; generated-build authority does not.
Its v5 contract binds a 4096 MiB/40% envelope. The sweep also covers
`native-emit.sh` and `lane-parity.sh`: their emitter, authority-test, C-lane,
and Rust-lane phases are independently guarded and remain sequential. Use
their `--selftest` and `--dry-run` modes for a no-ROM/no-Cargo contract check.
The WM selected-build exception is exactly two Cargo jobs; its verifier clears
the ambient environment and binds the explicit CLI/environment job count into
the v5 build authority.

The emitted-runner compile-and-run tests select their dev-interpreter rlib by
an artifact marker stored in a feature-gated source module. Keep the literal
out of cfg-disabled items in `lib.rs`: rustc metadata can retain those tokens
in an AOT-only rlib and make a mixed-feature target choose the wrong artifact.
The separate module makes ordinary shared targets reliable again; an isolated
target remains the cleanest feature-graph diagnostic.

The supported private acquisition path for one reproducible executable-image
group is likewise bounded and serialized:

```sh
scripts/capture-wm-executable-image-group.zsh --selftest
scripts/capture-wm-executable-image-group.zsh \
  --producer /absolute/path/to/mupen_trace \
  --core /absolute/path/to/libmupen64plus.dylib \
  --rsp /absolute/path/to/rsp-plugin.dylib \
  --rom /absolute/private/path/NWXE.z64 \
  --out-dir /absolute/private/path/new-vector-captures \
  --group-name FN64_EXECUTABLE_IMAGE_GENERAL_EXCEPTION \
  --image-id general-exception-preamble \
  --capture-pc 0x80000180 --first-pc 0x80000180 \
  --start 0x80000180 --word-count 4 \
  --steps 400000 --timeout-seconds 600
```

It defaults to three independent runs, the canonical minimum. Each producer
owns a fresh 2048 MiB/40%-free guarded process group and receives only the
allowlisted public-debugger capture environment under `env -i`. A zero exit
without both `image.json` and `boot-context.json` is still failure. After all
runs, a guarded `cargo run -j1` invokes the canonical
`parse_reproducible_executable_image_group` parser and binds the result to the
requested ROM, group, image identity, and geometry. Success prints one
path-free receipt; paths, captured words, producer output, and diagnostics stay
inside the caller-owned mode-0700 directory. Supply `--runs N` for more than
three observations. The output directory must be absent and outside this
repository.

### Build and compare one exactly withheld WM entry

Build the pure-AOT and `dynamic-withheld` hosts from one private input set with
two guarded, lane-isolated Cargo targets under one reusable private root:

```sh
ROM=/absolute/private/NWXE.z64 \
FN64_BOOT_CONTEXT=/absolute/private/boot-context.json \
FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGES \
FN64_EXECUTABLE_IMAGES=/absolute/private/image-1.json:/absolute/private/image-2.json:/absolute/private/image-3.json \
scripts/build-wm2000-withheld-pair.zsh \
  /absolute/private/new-wm-withheld-build
```

The destination must not exist and must remain outside the repository. The
builder checks both feature graphs, then runs the AOT and dynamic `--locked`
builds serially under a 4,096 MiB/40%-free,
one-hour-per-lane default guard. It retains distinct executables, sanitized
logs, per-lane guard JSONL, and a path-free build-local receipt. The receipt
binds private-input, capture-group, manifest, lock, guard, checker, and binary
digests, but deliberately carries no release or static-discovery authority.
The targets live at `cargo-target/aot` and
`cargo-target/dynamic-withheld`. Keeping their Cargo fingerprints separate
prevents either feature graph from evicting the other's freshness state.
Receipt v4 omits the former standalone dynamic `cargo check`: the retained
dynamic build compiles the same source and feature graph and additionally links
the executable, while avoiding a dynamic-to-AOT-to-dynamic Cargo feature flip.
The comparator accepts retained v3 receipts under their exact legacy field set
and v4 receipts under the smaller exact field set; fields cannot be mixed
between versions.

The retained attempt-17 v4 pair, seeded from the completed attempt-16 Cargo
cache, built AOT in 33 seconds at 755 MiB peak tree RSS and dynamic-withheld in
32 seconds at 751 MiB. Its receipt is
`/private/tmp/fn64-wm-exact-entry-pair-20260731-17/receipt.json`; this is one
private build measurement, not a repeat-count performance result.

Persistent-cache attempt 22 populated both lane caches in 1,354 seconds AOT
and 1,335 seconds dynamic-withheld, peaking at 1,480 and 1,482 MiB. With no
source or input change, attempt 25 then completed the same builds in one and
two seconds, emitted zero shard `Compiling` lines, and produced byte-identical
binaries. Its receipt is
`/private/tmp/fn64-wm-exact-entry-pair-20260731-25/receipt.json`. These are one
cold pair and one warm pair, not repeat-count performance evidence.

After an interrupted or failed attempt, seed a new immutable attempt from its
retained private Cargo target root:

```sh
FN64_WM_PAIR_CARGO_CACHE_SEED=/absolute/private/prior-attempt/cargo-target \
ROM=/absolute/private/NWXE.z64 \
FN64_BOOT_CONTEXT=/absolute/private/boot-context.json \
FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGES \
FN64_EXECUTABLE_IMAGES=/absolute/private/image-1.json:/absolute/private/image-2.json:/absolute/private/image-3.json \
scripts/build-wm2000-withheld-pair.zsh \
  /absolute/private/new-wm-withheld-build
```

The builder APFS-clones each lane into the matching new lane and still executes
both feature gates and both builds. A legacy flat target is cloned into both
lanes so older attempts remain usable. The seed is recorded only as
caller-provided, untrusted acceleration; its path and contents carry no receipt
authority.

For repeated local iteration, keep the lane targets at stable absolute paths
instead of relocating a copied target on every attempt:

```sh
mkdir -m 700 /absolute/private/wm-pair-cache
FN64_WM_PAIR_CARGO_CACHE_ROOT=/absolute/private/wm-pair-cache \
ROM=/absolute/private/NWXE.z64 \
FN64_BOOT_CONTEXT=/absolute/private/boot-context.json \
FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGES \
FN64_EXECUTABLE_IMAGES=/absolute/private/image-1.json:/absolute/private/image-2.json:/absolute/private/image-3.json \
scripts/build-wm2000-withheld-pair.zsh \
  /absolute/private/new-wm-withheld-build
```

The cache root must be an existing non-symlink directory outside the
repository. The builder takes an atomic cache-root lock, builds directly in
its `aot` and `dynamic-withheld` lanes, then APFS-clones both into the fresh
result's `cargo-target` snapshot. An interrupted run retains progress at the
same cache paths. The persistent cache is untrusted acceleration: both Cargo
commands still execute, only newly retained binaries enter the comparison, and
the cache path is redacted from logs and receipts. Do not set the persistent
root and `FN64_WM_PAIR_CARGO_CACHE_SEED` together. The lock is released before
failure-log sanitization so an early guard failure cannot strand it.

Run the operational comparison with a deterministic controller schedule:

```sh
ROM=/absolute/private/NWXE.z64 \
FN64_BOOT_CONTEXT=/absolute/private/boot-context.json \
FN64_WM_PAIR_RECEIPT=/absolute/private/new-wm-withheld-build/receipt.json \
FN64_WM_AOT_BINARY=/absolute/private/new-wm-withheld-build/wm2000-block-boot.aot \
FN64_WM_DYNAMIC_BINARY=/absolute/private/new-wm-withheld-build/wm2000-block-boot.dynamic-withheld \
scripts/wm2000-withheld-rdram-diff.zsh \
  /absolute/private/controller-schedule.json 100000 2000000
```

Both binaries retain the complete static catalog and must report the same
canonical program and resolver-install identities. The wrapper leaves the AOT
lane unchanged and sets `FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY=1` only for the
dynamic lane. That
token selects the already validated installed entry `ExecutionKey`; no other
catalog member is accepted. The operational unified dispatcher redirects that
entry to dynamic mapped execution once, without removing a bank or changing
static program identity. The guard clears only after that attempt charges
positive dynamic work; normal static dispatch budgets and executable-mutation
reconciliation then resume. The dynamic lane must reach the baseline's exact
global guest-work count and report
one per-attempt proof naming the installed entry with
`charged_instructions > 0` and `unsupported_exits == 0`, report zero dropped
identities, and match full logical RDRAM plus canonical device, executor, and
ABI-host digests. Telemetry schema
`fn64.wm2000.dynamic-withheld-telemetry.v2` records the selected `bank` and
`pc`, selection basis, that one attempt, and unchanged program/resolver
identities. Aggregate dynamic totals remain diagnostics and cannot prove that
the selected attempt charged work. The program-identity line and comparator
bind `resolver_install_sha256` as well as the canonical program identity. CPU
and continuation digests are compared only
when both lanes publish the same complete non-opaque thread set; otherwise the
comparison records `null` for those fields and does not call the result full
machine state. Publication digest v2 makes that operational comparison
partition-invariant by excluding only the validated most-recent slice charge;
it still binds cumulative work, CPU state, pending exit, and prepared
continuation. A one-instruction final slice may retire a straight instruction,
while an indivisible branch/delay pair fails loudly and produces no comparison
receipt. Exercise both wrappers without private inputs using
`scripts/test-build-wm2000-withheld-pair.zsh` and
`scripts/test-wm2000-withheld-rdram-diff.zsh`. The current exact-entry builder
and comparator wrapper contracts passed 10/10 consecutive runs after the
schema migration on 2026-07-31. The canonical ABI
publication and boot-harness digest suites independently passed the
deterministic bar.
One real-ROM v3 diagnostic reached 100,001 charged instructions in both lanes.
The exact withheld key `(0x81bf2e27273b27db, 0x80000400)` executed dynamically
once for one instruction with zero unsupported exits. RDRAM, CPU, device,
executor, ABI-host, continuation, scheduler steps, and simulation time matched.
Both publication diagnostics reported the same pending `ExecutableWrite`,
five-instruction last charge, cumulative charge 100,001, and no prepared
continuation. Evidence is retained at
`/private/tmp/fn64-wm-exact-entry-diff-20260731-25/{comparison.json,dynamic-telemetry.json,aot.log,dynamic.log}`.
This is one operational diagnostic only, not a ten-run parity claim.

For a ROM-bearing static-recompilation checkpoint, aggregate the gate-owned
receipts without rerunning discovery or Cargo:

```sh
scripts/static-recomp-scorecard.py \
  --closure-audit /private/tmp/fn64-score/nwxe.closure-audit-v3.json \
  --source-frontier /private/tmp/fn64-score/source.json \
  --writer-denominator /private/tmp/fn64-score/writers.json \
  --evidence-label historical
```

The scorecard is a read-only diagnostic aggregator. It rejects unknown schemas,
malformed fixed denominators, inconsistent headline counts, and receipts whose
ROM or program-model digests do not bind to one another. It cannot mint or
restore an authority. Existing receipt schemas do not bind the Git worktree,
so `--evidence-label current` additionally requires the explicit
`--ack-current-is-caller-attested` acknowledgement and remains labeled as a
caller assertion. Use `--format json` for comparisons. Static progress stays
multi-axis: zero `unsupported`, zero `dynamic_mips` for pure static execution,
a source/catalog frontier without known open findings, and eight completed
writer channels are reported separately rather than collapsed into a weighted
percentage. A verifier-owned writer-audit bundle can retain selected-build
series for all eight fixed channels:
Bootstrap/CPU/HostAbi/PI/RDP-renderer/RSP/SI/SP. The denominator atomically
accepts exactly the represented subset, including all eight rows when every
model-bound series is present. RSP audit mode selects LLE and arms immediately
before scheduling; a data-only writeback may prove the typed producer without
fabricating an executable-byte change. No private exact-ten series has run, so
these remain structural capabilities rather than production completion
evidence.

The retained outcome-8 checkpoint is a useful shape check for this loop: its
caller-attested current scorecard reports 1,823 unique destination VAs (1,810
`block_aot` / 7,240 bytes, 13 `dynamic_mips` / 12 bytes, 0 `unsupported`). The
dynamic bucket contains 3 concrete destinations and 10 indirect sites; the
concrete VAs are `0x8010211c`, `0x8013b744`, and `0x8013c3c0`.

Relative to outcome 6, the denominator contracted by 515 destinations, AOT by
503 destinations / 2,012 bytes, and dynamic by 12 destinations because
unsound ambiguous-generation authority was removed. The dynamic reduction is
a denominator artifact, not 12 coverage wins. Outcome 6's direct sources
comprised boot (2,800), overlay 2 (552), and overlay 3 (215); outcome 8 retains
only the boot source closure. The six previously unique targets are absent,
not closed. Thirteen prior concrete dynamic destinations disappeared with
those source closures, `0x8010211c` remains, and `0x8013b744` plus `0x8013c3c0`
newly appear as dynamic. Soundness improved, but admitted coverage regressed
pending typed activation-compatibility authority.

The sibling transfer inventory is 2,800 entries (2,660 guest, 137 host, 3
open), with 12 closed indirect sites. The source receipt differs from outcome
6 because its authority-derived inventory changed; the writer receipt remains
byte-identical. Compare the retained JSON sets rather than subtracting only
headlines.

The 2026-07-30 delay-entry-alias regeneration leaves outcome 8's scoreboard
identical: 1,810 `block_aot`, 13 `dynamic_mips`, and 0 `unsupported`. That is
the expected result while overlapping-generation activation remains outside
the authority set. It does improve the proof frontier at `0x8013b744`: the
overlay-2 block proof now preserves the predecessor pair starting at
`0x8013b740` instead of inventing a four-byte block at the delay entry, and the
spurious owner-missing/partition-ambiguity row for that entry disappears. The
destination remains dynamic for the narrower, honest
`entry_not_authoritative` reason.

The catalog-bound transfer slice now replaces overlapping-VA fanout with a
move-only capability that binds the normalized ROM, dense manifest, diagnostic
topology, backed runtime catalog definition, exact source generation/bank/site,
transfer kind, destination, and selected target generation. A target
generation can be excluded only by an exact conflicting physical byte while
the source generation's complete invalidation interval has catalog backing;
the control/delay pair must also be proven not to write catalog-backed memory.
Composition rechecks the complete identity before consuming the capability.
Calls confer callable authority; direct jumps confer reachability only. Zero
compatible generations is a typed activation miss, while multiple compatible
generations remain ambiguous. Geometry states and runtime observations cannot
construct this authority. Its bounded fixed point is wired into
`gate_wm2000_recompile`: it consumes the shared ROM-derived backed catalog,
emits path-free typed dispositions and authority totals, and feeds the
resulting validated snapshots directly to block-pack emission and the
scorecard. The WM build uses the same catalog constructor; runtime
reconstruction must match its canonical digest before external captured images
are appended. The eight fixed-point tests and all thirteen gate tests each pass
10/10 guarded runs.

A read-only characterization against the caller-owned NWXE image on 2026-07-30
measured the three outcome-8 concrete transfers. `0x800e1bcc` is a `jal` to
`0x8013b744` and selects exactly `recovered_overlay_2`. `0x800f1de4` is a
direct `j` (not a call) to `0x8010211c` and has zero compatible catalog
generations, so it is a typed activation miss and creates no target
reachability. `0x800e1bb4` is a `jal` to `0x8013c3c0`, but its delay word at
`0x800e1bb8` is `sh`; the current proof therefore rejects it as a possible
catalog-backed write. That third edge remains unauthorized unless a later
address proof shows the exact store cannot touch catalog backing. This closes
one callable edge and classifies one guaranteed miss; it does not claim loader
reachability, all-path coverage, or authority for the third edge.

The first newly wired ROM-bearing run on 2026-07-31 reached two rounds with one
authorized target root, one activation miss, nine ambiguous selections, and
seven rejected requests. Admitting overlay 2 expanded the scorecard from the
prior 1,810 `block_aot` / 13 `dynamic_mips` / 0 `unsupported` denominator to
2,047 / 19 / 0; covered AOT bytes increased from 7,240 to 8,188. The six added
dynamic destinations are newly exposed by the larger authority closure. The
result remains caller-attested and incomplete: the source frontier has 16
direct-transfer blockers, six open exception vectors, and all eight writer
channels open.

Those values are a unique-VA diagnostic denominator, not ROM/code/path
coverage and not generation closure. The outcome-8 artifacts stay in the
caller-owned private output directory outside git; do not copy their path into
docs or logs. The `current` label remains explicitly caller-attested because
the receipt schema does not bind the worktree.

A denominator containing `writer_audit_bundle` rows is inadmissible to the
scorecard by itself. Supply its sibling `writer-audit.json` with
`--writer-audit`; aggregation then requires all eight rows to be complete from
one bundle and binds the audit to the exact denominator bytes, normalized ROM,
program model, bundle schema, and bundle authority. It also requires the frozen
series/build contract: 10 runs per channel, 8 channels, completion bitmap 255,
and the 4096 MiB/40%-free build guard. A missing or mismatched companion fails
closed rather than presenting copied denominator rows as completed evidence.

To regenerate the three receipts against one current WM2000 ROM and aggregate
them in order, use the guarded wrapper. It requires absolute readable ROM and
ROM-bound BootContext paths plus an explicit absent output directory outside
the repository:

```sh
export FN64_DISCOVER_NWXE_ROM=/absolute/path/to/NWXE.z64
export FN64_BOOT_CONTEXT=/absolute/path/to/boot-context.json
scripts/current-static-scorecard.zsh \
  --output /private/tmp/fn64-score-current
```

The wrapper refuses an existing output directory rather than guessing whether
partial private evidence is safe to resume. It creates the directory mode
0700, retains producer diagnostics and the fixed receipts there, and runs the
canonical WM static-frontier producer once under the existing memory guard
with one Cargo job. That producer emits the closure audit, source frontier,
and open writer denominator from one exact discovery/composition snapshot;
the wrapper then prints only the path-free caller-attested scorecard. It clears
unrelated ROM, dump, executable-image capture, and optional block-source
emission variables. The current label still does not bind the Git worktree;
the wrapper supplies the scorecard's explicit caller-attestation
acknowledgement and does not convert that assertion into authority. Preview
the redacted stage list without creating the directory with `--dry-run`; run
the ROM-free fake-command coverage with `--selftest`.

The default wrapper mode intentionally retains the fast, open writer
denominator. To replace that input for the final aggregation with verifier-
owned completion evidence for all eight writer channels, opt into the much
more expensive selected-build audit and declare every reproducible executable-
image group explicitly. Each group needs at least three absolute readable
captures; names use the `FN64_EXECUTABLE_IMAGE_*` namespace:

```sh
scripts/current-static-scorecard.zsh \
  --full-writer-audit \
  --image-group FN64_EXECUTABLE_IMAGE_MAIN /private/a.json /private/b.json /private/c.json \
  --max-build-seconds 7200 \
  --output /private/tmp/fn64-score-current-full
```

Full mode still runs the one-pass static producer first. It then launches the
feature-gated `run_wm_writer_audit` CLI through the memory guard. The verifier
owns an exact two-job selected build, binds that job count into its v5 build
authority digest and the v3 path-free audit receipt, requires a new private
`full-writer-audit` subdirectory, and aggregates
against that CLI's completed `writers.json` and co-produced
`writer-audit.json`. The static producer's open
denominator remains retained alongside it for diagnosis. Any missing group,
failed build/series, missing fixed receipt, duplicate group name, or existing
audit directory stops before aggregation. The ordered image-group names and
capture paths are projected from the command line into both the static
frontier and the selected-build audit; ambient capture selectors are still
removed, so both stages validate the same explicit groups against the same
ROM. Because the static-frontier wire is a Unix path list, capture paths
containing `:` are rejected before either stage starts. Omitting
`--max-build-seconds` uses the CLI's 7200-second build ceiling; accepted values
are 2400 through 7200.

The wrapper streams only fixed, path-free `writer-progress` records from that
CLI and retains the same records in `writer-progress.log`; child output and
failure diagnostics remain private. Progress reports the verified-build
boundary and each exact-ten channel-series start, completion, or failure with
wall time. A channel failure does not discard the expensive selected build or
prevent the other independent series from running. The CLI exits unsuccessfully
after attempting all eight, retains bounded per-channel error diagnostics in
`full-writer-audit/diagnostics`, and writes
`partial-writer-audit.json`. When at least one series succeeds, the in-process
move-only partial bundle is consumed into `partial-writers.json`: successful
rows are complete projections of that one validated bundle and failed rows
remain open against the common program model. The partial audit file contains
only diagnostic hash references and binds the partial denominator bytes; it is
not a serialized capability. If every series fails, no bundle or program model
exists and no partial denominator is minted. A failed run never creates the
complete `writers.json` or `writer-audit.json`, and the scorecard rejects a
partial denominator without the complete companion as well as the distinct
partial audit schema. Timings are observational and do not enter build,
series, bundle, or scorecard authority.
Full mode has three numbered stages; inventory-only mode retains two. The
verified build still compiles from a fresh target, so a build start without an
immediate completion record is expected compiler work rather than evidence
that a writer series has begun.

The v4 two-job selected-build attempt compiled all 35 shards and the root, then
the guard terminated the process group at 2,050 MiB against its 2,048 MiB cap;
system free memory was still 77%. It therefore produced no verified-build,
writer-audit, or scorecard receipt. An earlier full graph measurement peaked at
3,194 MiB. The v5 protocol retains exactly two jobs but raises its single fixed
owned-build envelope to 4,096 MiB/40%; the outer full-audit wrapper uses the
same ceiling so it cannot preempt the nested authority owner.

The first complete v5 selected build finished in 854,608 ms and produced its
verified-build authority. The subsequent all-channel attempt retained one
successful exact-ten CPU series (23,932 ms) and a one-of-eight partial bundle;
it did not produce a complete writer audit or scorecard. Bootstrap run zero
reached its semantic report after emitting 8,214,477 bytes of ordinary runtime
diagnostics, exceeding the former 1 MiB transport cap. Host ABI, PI, RDP
renderer, RSP, SI, and SP each reached the unchanged 600-second child watchdog
without a completion receipt. Those are seven exact failures, not seven
unsupported-channel conclusions. The transport now remains bounded at 16 MiB,
extracts exactly one protocol-prefixed report capped at 1 MiB, and sends only
that envelope to the unchanged nonce/build-bound semantic parser. Timeout and
over-limit diagnostics retain byte counts, full-output digests, and bounded
tails in the private diagnostic path.

For the packed-lane size gate, skip rustc and profile the exact recovered WM
inventory directly:

```sh
cargo run --quiet \
  --manifest-path examples/wm2000-prepared-shard-producer/Cargo.toml \
  --bin fn64-wm-static-micro-op-profile -- \
  --rom /absolute/private/path/NWXE.z64
```

The probe prints only a fixed schema, package/instruction/byte counts, the
12 MiB ceiling, and an inventory digest; no path, address, instruction word, or
ROM byte is emitted. It shares the shard resolver but does not construct the
legacy generated Rust. The current V2 complete-WM result is 516,688 owned
records and 4,135,951 bytes; ten consecutive real-ROM runs returned the
identical digest and counts. V2's optional delay-only span lookahead admits
all 35 packages without creating another owned entry. This is a representation
and feedback-loop gate, not execution or `production-aot` authority.

The live shell heartbeat is also a bounded timing probe now. Alongside the
decoded-frame hash it reports windowed retrace Hz and interval, guest pump,
and present median/p95. A slow `pump` with a cheap `present` is an executor or
renderer-work budget problem; a slow `present` is the window/GPU blit path.
The cumulative `retrace_hz` remains useful for long-run drift but must not be
used to locate a phase transition by itself.
The same heartbeat reports cumulative and per-window callback underrun samples,
AI submission count, current-DMA `AI_LEN`, guest/device rates, and host frame
depth. For live audio acceptance, wait past startup/title and require stable
depth with neither an overflow warning nor new per-window underruns; a single
point-in-time depth or a nonzero-sample count is not an audible-health test.

To split synthesis/AI decoding from host resampling, set
`FN64_DUMP_AUDIO_STREAM_PCM=/tmp/fn64-guest.pcm`. It records at most 12 seconds
of consecutive pre-resample stereo `s16le` plus a `.meta` sidecar. Convert the
local evidence without committing it:

```sh
ffmpeg -f s16le -ar 32006 -ac 2 -i /tmp/fn64-guest.pcm \
  -ar 48000 /tmp/fn64-guest-hq.wav
```

If the WAV is clean while live output buzzes, the host resampler is the fault;
if both buzz, continue upstream at AI decoding/RSP synthesis.

For task-level RSP replay, set `FN64_DUMP_AUDIO_TASK=/tmp/fn64-task.rdram`.
By default this captures the first submitted audio task; use one-based
`FN64_DUMP_AUDIO_TASK_INDEX=N` to capture the task aligned with a later audible
event. Capture occurs at the common task-kick boundary before either translated
or live-image LLE execution, so both policies expose the same immutable task
input. The sidecar records `task_offset`, `task_index`, and `rdram_len`.
Pair it with `FN64_TRACE_AI_BUFFERS=1` when checking whether the AI consumes the
same RDRAM address/length range the replayed task's `A_SAVEBUFF` commands
produced.

For generated-vs-interpreter RSP checks:

```sh
RSP_TRACE_WRITE_RDRAM=/tmp/interp.rdram \
  cargo run -p fn64-audio --bin rsp_trace -- \
  --task-dump /tmp/fn64-task.rdram /tmp/fn64-task.meta 2000000

RSP_TRACE_WRITE_RDRAM=/tmp/generated.rdram \
  cargo run --manifest-path examples/oot-boot/audio-ucode/Cargo.toml \
  --bin replay_task -- /tmp/fn64-task.rdram /tmp/fn64-task.meta

cmp /tmp/interp.rdram /tmp/generated.rdram
```

`RSP_TRACE_DMA=1` logs every RSP DMA read/write with decoded length and a source
checksum in both paths. Use it to find the first divergent hardware seam before
looking at instruction traces. `RSP_TRACE_DMA_LIMIT=N` bounds the process-wide
DMA stream to its first `N` operations. `RSP_TRACE_DMA_WORDS=N` adds the first
`N` native-storage words from each read source; keep it bounded because command
buffers are game data and the diagnostic can be large.

`RSP_TRACE_EXEC=1` logs every interpreter PC and raw instruction word.
`RSP_TRACE_EXEC_LIMIT=N` bounds that process-wide stream to the first `N`
instructions. The trace is intentionally verbose and disabled by default.
`RSP_TRACE_EXEC_GPRS=9,11,13` adds the named scalar-register values to each
emitted instruction record; indices are decimal and comma-separated.
`RSP_TRACE_CP0=1` logs RSP-side CP0 writes, including the scalar values which
program SP DMA and DPC registers.
`RSP_TRACE_DPC_WORDS=N` prints the first `N` logical command words from each
completed LLE DPC range before it is submitted to the renderer.
`RSP_TRACE_RDRAM_WORDS=OFFSET:COUNT` prints native-storage words from one
hexadecimal RDRAM offset when an LLE task begins; `COUNT` is decimal.
`RSP_TRACE_DMEM_WORDS=OFFSET:COUNT` prints big-endian logical DMEM words at
task admission and after every completed IMEM overlay DMA, tagged with the
overlay generation and resumed PC.
`RSP_TRACE_DMEM_WRITES=OFFSET:COUNT` watches a hexadecimal DMEM offset and a
decimal byte count, logging scalar/vector writes which overlap that logical
range. Pair it with `RSP_TRACE_DMA=1`, since bulk SP DMA uses the backing image
directly and is reported by the DMA trace instead.
`FN64_TRACE_PI_DMA=1` logs every managed/raw PI request at the shared timing
boundary with direction, cartridge address, RDRAM destination, and length.

`FN64_DISCOVER_REPORT_PROJECTION_STATS=1` prints the bank count, aggregate
bank-indexed fact rows, compact serialized fact bytes, and truly global rows
per bank after snapshot composition's projection preflight. It does not change
composition limits or authority; use it to distinguish fact-scope growth from
CFG/owner work.

For crackle that survives stable ring depth and zero underruns, capture the
post-resample stream too:

```sh
FN64_DUMP_AUDIO_OUTPUT_STREAM_PCM=/tmp/fn64-output.pcm
ffmpeg -f s16le -ar 48000 -ac 2 -i /tmp/fn64-output.pcm /tmp/fn64-output.wav
```

The heartbeat also reports `late_callbacks` and `max_callback_gap_us`. A clean
pre/post PCM pair with late callbacks points at host output delivery; crackle
already present in the pre-resample file points upstream at AI decoding/RSP
synthesis.
