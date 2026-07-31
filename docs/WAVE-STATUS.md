# In-flight wave status

This is an inventory of the uncommitted working tree captured on 2026-07-31.
It contains 285 status entries: 143 modified, four deleted, and 138 untracked.
The tracked diff spans 147 files with 53,639 insertions and 11,305 deletions;
untracked file contents are not included in those diff totals. No build or test
was run while preparing this inventory.

“Verified” below applies only where the named current-wave gate met the
`AGENTS.md` count: 10 consecutive clean runs for deterministic behavior or 20+
for a concurrency fix. A single full-suite pass is recorded as such but is not
promoted to that bar. Private artifacts are evidence trails, not files eligible
for commit.

## Recompiler split, arbitrary-PC execution, and static micro-ops

**FILES:** `Cargo.toml`, `Cargo.lock`, `crates/fn64-recomp-rs/**`, the new
`crates/fn64-recomp-rs-codegen/**`, and their portions of
`docs/{DESIGN.md,DISCOVER-PLAN.md,RECOMP-RS-COVERAGE.md}`. The deleted
`fn64-recomp-rs/src/{emit.rs,module.rs,bin/recompile_rom.rs}` are represented in
the new codegen crate; runtime execution, fetch, generation, semantic, and
static-micro-op modules remain in `fn64-recomp-rs`.

**STATE:** Half-done. Build-side decoding and emission are split from the
linked runtime, and the five-bank WM pack emits and compiles. Static micro-ops
remain experimental: straight instructions share the semantic kernel, while
only narrow `BEQ`/`BEQL` delay-pair forms are implemented. **Exact next step:**
close mapped/TLB fetch identity, the remaining control/delay families, and
host-call transfer boundaries in the static-micro-op differential.

**VERIFICATION:** The recorded full `fn64-recomp-rs` run passed 370 tests with
one skipped. Budget-related generated-runner, semantic-kernel, and micro-op
regressions passed 10 consecutive runs. The WM static-micro-op profile passed
10/10 for 35 packages, 516,688 instructions, and 4,135,951 packed bytes
([DESIGN.md:593](DESIGN.md#L593)). No OoT or SM64 emitted-pack boot gate ran.

**FRONTIER:** The micro-op executor describes itself as experimental and not a
replacement for the production runner
([static_micro_op_exec.rs:1](../crates/fn64-recomp-rs/src/static_micro_op_exec.rs#L1)).
The OoT host can admit a pack, but no real OoT pack has been produced
([DESIGN.md:535](DESIGN.md#L535)). The recorded 4.14 MiB packed representation
reduces generated representation size; it does not prove the missing fetch or
transfer semantics.

**COMMIT SAFETY:** Not safe alone in its current form. Root manifests, ABI
features, boot-harness development dependencies, WM build scripts, and all
generated shard crates consume the split.

## Dense block pack, generation catalog, and fixed-point discovery

**FILES:** `crates/fn64-discover/src/{block_pack.rs,block_proof.rs,cfg.rs,closure.rs,facts.rs,grade_candidates.rs,owner_proof.rs,partition.rs,resolve.rs,snapshot.rs,dense_aot_pack.rs,generation_topology.rs,runtime_generation_catalog.rs,catalog_transfer_fixed_point.rs}`;
`crates/fn64-discover/src/bin/{gate_b2.rs,gate_closure.rs,gate_wm2000_recompile.rs}`;
the related discovery tests; `scripts/{wm2000-static-frontier.zsh,current-static-scorecard.zsh,static-recomp-scorecard.py}`;
and the corresponding discovery, scorecard, and fast-loop documentation.

**STATE:** Half-done. The fixed point emits, compiles, and probes five banks.
The exact-entry A/B reached the same 100,001-instruction horizon and its v3
comparison matched RDRAM, owner components, CPU, continuation, scheduler steps,
simulation time, and the per-thread publication diagnostic. **Exact next
step:** run nine more frozen real comparisons before calling the scheduler fix
deterministic.

**VERIFICATION:** The 30-test delay suite and generated-runner direct-entry
regression each passed 10/10
([DISCOVER-PLAN.md:2240](DISCOVER-PLAN.md#L2240)). Eight fixed-point tests and
13 WM gate tests each passed 10/10
([DISCOVER-PLAN.md:1330](DISCOVER-PLAN.md#L1330)). One ROM-bearing regeneration
reported 2,047 `block_aot`, 19 `dynamic_mips`, and zero unsupported outcomes,
but no repeat count is recorded for it. No current full `fn64-discover` suite
is evidenced after the latest exact-entry and checkpoint edits.

**FRONTIER:** The regeneration classified the 19 dynamic destinations as one
authorized, one activation miss, nine ambiguous, and seven rejected. Its
source side retains 16 direct-transfer blockers, six exception vectors, and
all eight writer channels open
([DISCOVER-PLAN.md:1337](DISCOVER-PLAN.md#L1337)). The delay-slot `sh` at
`0x800e1bb8` lacks proof that its address is outside catalog backing
([DISCOVER-PLAN.md:1351](DISCOVER-PLAN.md#L1351)). The retained diagnostic
scorecard at
`/private/tmp/fn64-score-catalog-fp-20260731-1/scorecard.json` is
caller-attested and not worktree-bound; it reports 8,188/8,224 destination
bytes AOT (99.562257%), 12 closed indirect sites, 16 open direct transfers,
six open vectors, 14 open writer classes, 52 open CPU word stores, three
conditional stores, and 42 unclassified cache sites.

**COMMIT SAFETY:** Not safe alone. `gate_wm2000_recompile.rs` and `snapshot.rs`
couple catalog topology, PI recovery, source/writer closure, the recompiler
split, ABI admission, the boot harness, and the WM generated workspace.

## Semantic callback roots and known-ROM training

**FILES:** `crates/fn64-discover/src/{callback_flow.rs,snapshot_inputs.rs,snapshot_workspace.rs,workspace_artifacts.rs,missed_function_attribution.rs,candidate_relation_report.rs,snapshot.rs,pi_dma.rs,banks.rs}`;
the new workspace, staging, attribution, validation, and ROM-identity binaries;
their tests; `reference/{corpus-invocations.md,oot-ntsc-1.0-entry-args.toml}`;
the deletion of `reference/mm-n64-us-entry-args.toml`; and
`scripts/{cold-training-fold.py,test-cold-training-fold.py,mechanism-opportunity-ranking.py,test-mechanism-opportunity-ranking.py}`.

**STATE:** Half-done. `osCreateThread` and the composed MM Fault callback
registry are mechanically recovered; the sealed workspace, streaming
validator, and late-key attribution paths exist. **Exact next step:** complete
one current-schema prepare, freeze, held-out-grade fold and retain its receipts.

**VERIFICATION:** Focused semantic recovery passed 10 consecutive runs
([FAST-LOOP.md:88](FAST-LOOP.md#L88)). Cached private OoT and held-out SM64
measurements are recorded, but their A/B artifacts are explicitly not retained
or rechecked. No run count is recorded for `test-cold-training-fold.py`, and no
current six-ROM or full-fold 10-run result was found.

**FRONTIER:** Remaining OoT `Fault_AddClient` and `_Printf` roots are explicitly
not derived automatically
([oot-ntsc-1.0-entry-args.toml:1](../crates/fn64-discover/reference/oot-ntsc-1.0-entry-args.toml#L1));
the AKI donor still covers callbacks loaded through caller-provided object
fields ([corpus-invocations.md:81](../crates/fn64-discover/reference/corpus-invocations.md#L81)).
`callback_flow.rs` gives no authority after merged disagreement, arithmetic,
or non-stack loads. Workspace claims remain candidate-level and cannot mint
owner or extent authority ([DISCOVER-STORAGE.md:228](DISCOVER-STORAGE.md#L228)).

**COMMIT SAFETY:** Not safe alone. Snapshot schema v5, candidate schema v3,
callback/PI slicing, the producer and validator CLIs, reference-input removal,
and the shared explicit Cargo binary registry must agree.

## Stock Ghidra candidate and computed-flow bridge

**FILES:** `tools/ghidra/Fn64ExportCandidates.java`, the new computed-flow and
loader-comparison Java scripts and fixture, snapshot-bank/workspace runners,
their Python and shell tests, `crates/fn64-discover/src/{tool_adapter.rs,tool_claims.rs,candidate_corroboration.rs,candidate_cfg_probe.rs,candidate_relation_report.rs,spimdisasm_reference.rs}`,
the related ingestion/comparison binaries and tests, and
`tools/ghidra/README.md` plus `docs/DISCOVER-TOOLCHAIN.md`.

**STATE:** Half-done. Candidate-only receipt ingestion and computed-flow schema
exist. **Exact next step:** run the current schema-v2 exporter conformance
series 10 times.

**VERIFICATION:** The old schema-v1 exporter and raw-bank T3 path each have a
10-run record, but the Ghidra README states those hashes do not certify the
current schema-v2 discontiguous-body exporter
([README.md:471](../tools/ghidra/README.md#L471)). The synthetic computed-flow
fixture passed 10 guarded runs
([DISCOVER-TOOLCHAIN.md:675](DISCOVER-TOOLCHAIN.md#L675)). Only one real OoT
computed-flow comparison ran: all three native sites appeared, the one
exhaustive target matched exactly, two sites remained targetless, and seven
sites were Ghidra-only. No current snapshot-bank/workspace 10-run result exists.

**FRONTIER:** Containing-function entry authority and native resolver replay
remain open ([DISCOVER-TOOLCHAIN.md:683](DISCOVER-TOOLCHAIN.md#L683)). The
candidate export still lacks block, direct-reference, switch/data,
prototype/type, and frame completeness
([tools/ghidra/README.md:44](../tools/ghidra/README.md#L44)). Ghidra claims
cannot directly create a `FactDb` root or owner.

**COMMIT SAFETY:** The computed-flow slice can be separated with its fixture,
comparator, and tests. The broader snapshot bridge is entangled with Rust wire
schemas, ingestion, staging, the explicit Cargo binary registry, and its docs.

## N64LoaderWV candidate bridge and fork tooling

**FILES:** `tools/ghidra/Fn64ExportN64LoaderCandidates.java`,
`Fn64VerifyN64LoaderRuntime.java`, the new `run-n64loaderwv-*`,
`run-snapshot-loader-ab*`, comparison/grading/provenance/install/GUI scripts
and tests, and the N64LoaderWV sections of `tools/ghidra/README.md` and
`docs/DISCOVER-TOOLCHAIN.md`. The checked-in source/artifact policy JSON files
are relevant unchanged inputs.

**STATE:** Half-done. First-contact and one snapshot-bound A/B exist, but the
current untracked provenance/install/GUI/A/B paths are not certified.
**Exact next step:** reconcile the approved policy pins with the README and
toolchain pin before another artifact is admitted.

**VERIFICATION:** Approved-artifact Banjo first contact passed 10 guarded runs
([DISCOVER-TOOLCHAIN.md:631](DISCOVER-TOOLCHAIN.md#L631)). Snapshot loader A/B
ran once, below the deterministic bar. It found four VW-only starts and 114
words, all already present in fn64's ledger; no new coverage and no Banjo
answer-key grade were established
([DISCOVER-TOOLCHAIN.md:643](DISCOVER-TOOLCHAIN.md#L643)). No retained run
evidence was found for the new provenance, install, GUI, or A/B tests.

**FRONTIER:** The policy JSON approves source commit `eea9b4c7…`, tree
`79c239…`, and ZIP `097487…`, while modified prose still instructs pinning
`e484f187…`. Analyzer completeness remains unknown, and independent native
corroboration of unmatched candidates remains open.

**COMMIT SAFETY:** Not safe as one commit. Policy/prose reconciliation,
provenance/install tooling, GUI launcher, and snapshot A/B are separable review
units, but each unit must keep the candidate schema and receipt checks paired.

## PI/EPI recovery and Mupen trace ingestion

**FILES:** `crates/fn64-discover/src/{pi_dma.rs,trace.rs,banks.rs,harvest.rs,overlay_regions.rs,overlay_recipe.rs,source_closure.rs}`;
`crates/fn64-discover/src/bin/{headless_bridge.rs,validate_executable_image_group.rs}`;
PI and trace tests; `tools/mupen-trace/**`; and
`scripts/{run-black-box-trace.zsh,capture-wm-executable-image-group.zsh,test-run-black-box-trace.zsh,test-capture-wm-executable-image-group.zsh}`.

**STATE:** Half-done. Static slicers, typed trace folding, boot context, CPU
snapshots, executable-image records, optional input, fast-forward, and VI watch
paths exist. **Exact next step:** update the producer's evidence claim to the
non-exhaustive stepping contract and run fresh producer/ingestion gates against
that schema.

**VERIFICATION:** Historical evidence records three byte-identical 500,000-step
producer runs and `gate_trace` at 10/10
([DISCOVER-PLAN.md:1734](DISCOVER-PLAN.md#L1734)). A breakpoint/watchpoint driver
ran only 4/4, below the bar. A later debugger follow-up records 10 runs. These
runs do not certify the substantially changed producer. Current Mupen source,
input-plugin, classifier, capture scripts, and trace/PI integration are not
verified against this tree.

**FRONTIER:** Static PI candidates do not prove call reachability, DMA
completion, or handle-to-ROM mapping
([pi_dma.rs:20](../crates/fn64-discover/src/pi_dma.rs#L20)). The public debugger
advances a branch and delay slot atomically, so its observed PCs are not an
exhaustive executed-PC stream; current README/source say this while historical
prose still claims exhaustiveness. Interpreter stepping blocks deep capture,
and full-speed GE/PD capture has been nondeterministic.

**COMMIT SAFETY:** Not safe alone. Mupen producer/schema and Rust ingestion must
stay paired; PI facts also feed bank discovery, source closure, snapshot
composition, executable-image validation, and runtime DMA attribution.

## Canonical execution, executable writers, and selected-build audits

**FILES:** `crates/fn64-abi/{Cargo.toml,src/{dispatch.rs,host.rs,lib.rs,pi.rs,recompiled.rs,si.rs,sp_dp.rs,task_dispatch.rs,thread.rs},tests/c_smoke/smoke.c}`;
new `crates/fn64-boot-harness/src/generated_runner_build.rs` and
`precompiled_admission.rs`; modified `release_gate.rs`; WM writer-audit modes;
`crates/fn64-discover/src/{source_closure.rs,writer_denominator.rs,transfer_scan.rs,external_aot.rs,host_bindings.rs}`;
the writer-audit binary; and writer/scorecard scripts and docs.

**STATE:** Half-done. Canonical resolver/generation installs, mutation journal,
fixed eight-channel denominator, exact-ten selected-build protocols, exact
instruction budgets, and checkpoint publication exist. **Exact next step:**
rerun the all-channel selected-build audit after the 16 MiB transport change
and retain each channel's exact result.

**VERIFICATION:** One current-wave `fn64-abi` suite run passed 418 tests with
seven skipped. Four focused checkpoint/scheduler/dynamic tests passed 10
consecutive runs, and the scheduler-mirror HostAbi regression passed 10/10
([BOOT-NOTES-WM2000.md:1465](BOOT-NOTES-WM2000.md#L1465)). Historical v5
selected-build evidence records one exact-ten CPU series and a 1/8 bundle
([DESIGN.md:757](DESIGN.md#L757)); other modified docs say no private CPU series
ran. Because the sources changed afterward and the records conflict, CPU
writer verification for this working tree is unresolved. Bootstrap exceeded
the old 1 MiB cap; HostAbi, PI, RDP, RSP, SI, and SP timed out. No current
complete writer audit exists. The ABI full-suite run is one run, not 10.

**FRONTIER:** All eight rows remain open in the current diagnostic scorecard.
Broad raw pointers, mutable slices, noncanonical renderer/ABI paths, and
model-total SI authority remain unproved. Scheduler-owned
`__osRunningThread` publication is not representable by the host-call-only
HostAbi completion schema ([DESIGN.md:1080](DESIGN.md#L1080)). The structural
writer topology does not prove that every reachable path uses it.

**COMMIT SAFETY:** Not safe alone. Writer authority spans discovery, ABI,
runtime DMA/device paths, boot-harness build verification, WM audit children,
and the scorecard scripts. The 15,992-line addition in `recompiled.rs` mixes
catalog execution, dynamic fallback, publications, and every writer channel.

## Boot context, deterministic input, and WM scenario execution

**FILES:** new `crates/fn64-boot-harness/src/{boot_context.rs,controller_input_schedule.rs}`;
`crates/fn64-boot-harness/src/{lib.rs,release_gate.rs}`;
`examples/{oot-boot,wm2000-block-boot}/**`;
`crates/fn64-shell/src/{gamepad.rs,input_map.rs,overlay.rs}`; and
`scripts/{wm2000-route-probe.zsh,wm2000-route-series.zsh,wm2000-scenario-gate.zsh,test-wm2000-scenario-gate.zsh}`.

**STATE:** Half-done. ROM/TV-bound boot context, controller-read-ordinal
schedules, publication digest v2, and a bounded three-generation WM scenario
exist. **Exact next step:** generate and boot an OoT block pack with its bound
boot context.

**VERIFICATION:** The current boot-harness suite passed 281 tests once;
publication-v2 focused tests passed 10/10. The WM route series passed 10/10
with byte-identical evidence
([BOOT-NOTES-WM2000.md:1629](BOOT-NOTES-WM2000.md#L1629)); the authoritative
100,000-step scenario gate also passed 10/10
([BOOT-NOTES-WM2000.md:1731](BOOT-NOTES-WM2000.md#L1731)). No current OoT
real-pack boot ran.

**FRONTIER:** The scenario proves only its scheduled path and three entered
generations, not source or path closure. Five modeled exception vectors remain
unowned, and the OoT host has no real emitted pack. A 320,000-step route ruled
out simple repeated confirmation as an observed fourth-generation route, not
the fourth generation itself
([BOOT-NOTES-WM2000.md:1753](BOOT-NOTES-WM2000.md#L1753)).

**COMMIT SAFETY:** The parser and schedule types could be extracted with their
tests, but the current API signatures and publication wires are entangled with
ABI block boot, OoT/WM hosts, and scenario scripts.

## Runtime DMA, VI, and reference-renderer work

**FILES:** `crates/fn64-runtime/src/{device.rs,executor.rs,lib.rs,overlay.rs,rom.rs}`;
`crates/fn64-render-reference/src/backend.rs`, `crates/fn64-render/src/lib.rs`,
`crates/fn64-render-rt64/**`, the ABI PI/SI/SP/task-dispatch files, and the
renderer behavior/inventory docs.

**STATE:** Half-done. DMA adapters are sealed and channel-typed; device tracing
can retain constant-space counters; the reference renderer uses a lazy dense
hidden-state sidecar and paired framebuffer writes; VI field-epoch handling
changed. **Exact next step:** run the renderer/runtime regression set 10
consecutive times against unchanged task outputs.

**VERIFICATION:** The full reference-renderer suite passed 455 unit tests plus
six replay/snapshot tests once
([BOOT-NOTES-WM2000.md:1625](BOOT-NOTES-WM2000.md#L1625)). The guarded WM route
then passed 10/10 with identical semantic evidence, but it does not isolate
every renderer/runtime edit. Unit coverage exists for DMA channel identity,
post-commit notification, bounds preflight, and disabled-trace summaries; no
10-run count was found for those tests. No current exact-ten PI/SI/SP writer
audit completed.

**FRONTIER:** The base-renderer matrix remains 4/24 exact and no full-ROM
renderer parity gate is closed. Structural DMA topology does not prove that all
reachable device paths use the canonical adapters. The dense-sidecar timing
gain and unchanged scenario counters are performance evidence, not complete
render parity.

**COMMIT SAFETY:** The reference-renderer subset is mechanically separable but
not ready to commit because its behavior changes lack a dedicated 10-run gate.
DMA changes are entangled with ABI mutation ownership and selected-build writer
evidence. RT64 and shell edits should not be bundled as proof of the reference
renderer behavior.

## WM shard generation, prepared sources, and feedback tooling

**FILES:** `examples/wm2000-block-boot/{Cargo.toml,Cargo.lock,build.rs,src/main.rs}`;
new `examples/wm2000-block-shards/**` and
`examples/wm2000-prepared-shard-producer/**`; `docs/WM-PREPARED-SHARDS.md`;
`tools/profile_generated_shards.py`; and the new guarded-build, memory,
profiling, target-inventory, shard-lint, prepared-parity, prepared-audit, and
invalidation-benchmark scripts. The exact-entry pair builder and comparator
scripts are accounted for in the next thrust.

**STATE:** Half-done. The legacy 35-shard ROM-driven build is active; the
producer, materializer, prepared tree, and verifier candidate are inactive.
Every shard manifest still points at `../build.rs`. **Exact next step:** run 10
fresh real-ROM prepared-producer versus legacy byte-parity comparisons before
changing any shard manifest.

**VERIFICATION:** `scripts/test-memory-guard.zsh` passed 10/10 shell-only runs
([DISCOVER-PLAN.md:839](DISCOVER-PLAN.md#L839)). The exception-helper 26-test
bank-runner gate passed 10/10
([DISCOVER-PLAN.md:935](DISCOVER-PLAN.md#L935)). The static-micro-op profile
passed 10/10. One cold 35-shard build compiled but failed build-identity
validation. Prepared parity, invalidation benchmark, and a
`prepared_consumed` selected build did not run.

**FRONTIER:** No compiler-artifact evidence supports zero- or one-shard
invalidation, and no real-ROM prepared-consumed authority exists
([WM-PREPARED-SHARDS.md:140](WM-PREPARED-SHARDS.md#L140)). Fixed-size chunk
deduplication was ruled out: the complete prepared tree had no repeated 2 KiB
chunks, and measured semantic sharing was only 5.6–7.3%. The retained build
loop remains approximately 40 minutes in the selected one-job configuration.

**COMMIT SAFETY:** Generic memory-guard helpers can be committed independently
with their tests. Prepared activation cannot: producer, materializer, root
consumer, verifier schema, all 35 manifests, and their docs must change
atomically. `crates/fn64-discover/Cargo.toml` is also a cross-wave choke point:
it disables autobins, registers every binary explicitly, and supplies shared
dependencies for unrelated new CLIs.

## Exact-entry dynamic A/B and publication diagnostics

**FILES:** the dynamic-withheld and publication portions of
`crates/fn64-abi/src/recompiled.rs`,
`crates/fn64-boot-harness/src/release_gate.rs`,
`examples/wm2000-block-boot/src/main.rs`, and
`scripts/{build-wm2000-withheld-pair.zsh,test-build-wm2000-withheld-pair.zsh,wm2000-withheld-rdram-diff.zsh,test-wm2000-withheld-rdram-diff.zsh}`;
plus the corresponding design, fast-loop, and WM boot notes.

**STATE:** Half-done. One canonical static `(bank, PC)` can be withheld for one
attempt, dynamic work is charged per attempt, and telemetry v2 binds the
program and resolver-install identities. Publication digest v2 ignores only
valid slice partitioning. The unified dispatcher now publishes executable
writes before resolving their continuation, and one real-ROM v3 diagnostic
matched at the exact horizon. **Exact next step:** run the package-wide ABI
suite against this tree. That step subsequently passed; the exact remaining
step is nine more frozen real comparisons before calling the scheduler fix
deterministic.

**VERIFICATION:** Four focused ABI tests passed 10/10, and the publication-v2
focused gate passed 10/10. The current pair-builder and comparator wrapper
self-tests passed 10/10 together after the schema migration. The current
`fn64-abi` suite passed 418 tests with seven skipped once; the current
`fn64-boot-harness` suite passed 281 tests once. The dynamic mapped-source
receipt inventory test passed 10/10. The earlier whole-shard wrapper form
passed 10/10 but does not certify exact-entry selection
([FAST-LOOP.md:458](FAST-LOOP.md#L458)). The real-ROM exact-entry comparison ran
once; this does not meet the deterministic bar. Pair attempt 17 produced a v4
receipt after seeding from the completed attempt-16 Cargo cache. AOT built in
33 seconds at 755 MiB peak tree RSS and dynamic-withheld in 32 seconds at 751
MiB. The pair receipt is
`/private/tmp/fn64-wm-exact-entry-pair-20260731-17/receipt.json`.
The focused unified-dispatch suite passed 20/20 after the executable-write
publication fix. The lane-isolated builder and v3 comparator contracts passed
together 10/10 after the early-failure lock cleanup reorder. Persistent-cache
attempt 22 populated AOT/dynamic in
1,354/1,335 seconds at 1,480/1,482 MiB peak RSS. Unchanged attempt 25 took
1/2 seconds, emitted zero shard compile lines, and produced byte-identical
binaries. The real v3 comparison ran once; it does not meet the deterministic
bar. The package-wide ABI suite passed 286/286 tests with seven skipped once
after this fix; this is not a ten-run claim.
`scripts/lint-docs.py` was run once and failed four gates: an asserted but
ungated content hash at `docs/DISCOVER-PLAN.md:58`, the NMR surface check's
30-second timeout, stale `docs/BASE-RENDERER-BEHAVIOR-MATRIX.md`, and the
unsupported-recorder bypass at `crates/fn64-recomp-rs/src/semantic.rs:241`.

**FRONTIER:** Prior whole-shard runs at
`/private/tmp/fn64-wm-withheld-diff-catalog-fp-20260731-4.10uqUL` and
`/private/tmp/fn64-wm-withheld-diff-catalog-fp-20260731-5.uVLUEs` reached
100,001 charged instructions and matched RDRAM, device, executor, and ABI-host
digests, but neither withheld shard was entered. CPU and continuation digests
differed, so those runs are not dynamic-execution parity evidence
([BOOT-NOTES-WM2000.md:1476](BOOT-NOTES-WM2000.md#L1476)).
The attempt-25 exact-entry diagnostic at
`/private/tmp/fn64-wm-exact-entry-diff-20260731-25` reached 100,001 charged
instructions in both lanes. The exact withheld key
`81bf2e27273b27db:80000400` executed dynamically once for one instruction with
zero unsupported exits. Logical RDRAM, CPU, device, executor, ABI-host, and
simulation time matched, as did continuation and 33,333 scheduler steps. Both
publication diagnostics reported the same pending `ExecutableWrite`,
five-instruction last charge, cumulative charge 100,001, and no prepared
continuation. The evidence trail is
`comparison.json`, `dynamic-telemetry.json`, `aot.log`, and `dynamic.log` in
that directory. This single diagnostic is not ten-run parity evidence.

**COMMIT SAFETY:** Not safe alone in the present diff. ABI redirect semantics,
boot-harness wire v2, WM telemetry, build receipt, and comparator schema must
land together; the same large source files also contain writer-audit work.

## Content-consumer discriminator experiment

**FILES:** The abandoned result is recorded in the modified
`docs/DISCOVER-PLAN.md`; its candidate-only implementation remains unwired and
is not modified in this working tree.

**STATE:** Abandoned. The experiment identified only 2/10 open words correctly.

**VERIFICATION:** The 2/10 characterization is recorded at
[DISCOVER-PLAN.md:2090](DISCOVER-PLAN.md#L2090). No qualifying 10-run result is
recorded, and no current test was run for this inventory.

**FRONTIER:** Eight false pointer classifications were the
`__osExceptionPreamble` idiom, while the positive code signal duplicated
`cfg.rs`. This ruled out the discriminator as an authoritative promotion
mechanism; the candidate-only module remains unwired.

**COMMIT SAFETY:** The documentation of the negative result is independently
committable. It must not be bundled as an enabled discovery mechanism.

## Summary

| Thrust | State | Verified? | Blocking? | Safe to commit alone? |
| --- | --- | --- | --- | --- |
| Recompiler split / arbitrary-PC / micro-ops | Half-done | Partial: focused gates 10/10; full suite once | Yes | No |
| Dense pack / catalog / fixed point | Half-done | Partial: mechanism gates 10/10; ROM regeneration once | Yes | No |
| Semantic roots / known-ROM training | Half-done | Partial: focused recovery 10/10 | Yes | No |
| Stock Ghidra / computed flow | Half-done | Partial: computed fixture 10/10; current exporter unverified | Yes | Computed-flow slice only |
| N64LoaderWV bridge | Half-done | Partial: first contact 10/10; A/B once | Yes | Only after splitting paired units |
| PI/EPI / Mupen trace | Half-done | No for current producer | Yes | No |
| Canonical writers / selected-build audit | Half-done | No current complete audit; CPU record conflicts | Yes | No |
| Boot context / input / WM scenario | Half-done | Partial: WM scenario 10/10; OoT not run | Yes | Parser/schedule subset only |
| Runtime DMA / VI / renderer | Half-done | No dedicated 10-run current gate | Yes | No |
| WM shards / prepared sources / feedback | Half-done | Partial: guard and focused gates 10/10; prepared path unverified | Yes | Memory guard only |
| Exact-entry dynamic A/B | Half-done | Partial: focused contracts 10/10; real A/B not run | Yes | No |
| Content-consumer discriminator | Abandoned | No; 2/10 characterization | No | Documentation only |
