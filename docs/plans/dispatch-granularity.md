# Why a dispatch advances only 3 guest cycles

Investigation record. Answers the question left open by
`docs/plans/checkpoint-digest-cost.md`: with the SHA-256 removed entirely the
run would still be ~5,700x slower than hardware, because a dispatch advances
only 3 guest cycles. This document establishes what actually ends a dispatch.

Measured 2026-08-06 on WM2000, `examples/wm2000-block-boot`, Apple Silicon,
branch `fix/overlay-stride-aliases`.

## Headline

**The slice does not end at a block boundary, and it does not end on budget.
It ends because the guest stored to RDRAM.**

This holds at 60,000 steps (100% of slices) and past the boot BSS clear at
200,000 steps (99.8% of slices). It is structural, not a boot artifact.

A census over the 60,000-step benchmark:

```
[dispatch-census] slices=60000 instructions=180000 blocks=89999
                  instructions_per_slice=3.000 blocks_per_slice=1.500
                  instructions_per_block=2.000
[dispatch-census] slice_exit={"ExecutableWrite": 60000}
[dispatch-census] slice_instruction_histogram 1:30000 5:30000
[dispatch-census] slice_block_histogram        1:30001 2:29999
```

A per-block census run alongside it (since removed, see *Reproducing*)
resolved the inner turns:

```
block_instruction_histogram 1:59999 4:29999 5:1
block_exit=[("ExecutableWrite", 60000), ("Transfer", 29999)]
```

**100% of slices terminate on `BlockExit::ExecutableWrite`.** Not
`Checkpoint`, not budget exhaustion, not a host call, not a message queue.
Zero slices ended for any other reason.

And they terminate at exactly **two** guest PCs, splitting the 60,000 evenly:

```
[dispatch-census] site ExecutableWrite pc=0x80000414 count=30000
[dispatch-census] site ExecutableWrite pc=0x80000418 count=30000
```

### What that code is

Decoding the boot bank at those addresses (ROM offset `0x1000` maps to VA
`0x80000400`):

```
80000400  lui   $t0, 0x8005
80000404  addiu $t0, $t0, -0x4b40    # t0 = 0x8004b4c0   destination
80000408  lui   $t1, 0x0006
8000040c  addiu $t1, $t1, 0x5ed0     # t1 = 0x00065ed0   byte count
80000410  sw    $zero, 0($t0)        #   store 1
80000414  sw    $zero, 4($t0)        #   store 2
80000418  addi  $t0, $t0, 8
8000041c  addi  $t1, $t1, -8
80000420  bnez  $t1, 0x80000410
80000424  nop
```

This is the boot stub's **BSS clear**: a 6-instruction loop zeroing 0x65ED0
bytes (~417 KB) two words per iteration, ~52,000 iterations. It is not
self-modifying code by any reading — it is `memset` on uninitialized data.

The two `sw $zero` land inside the watched megabyte, so each one sets the
executable-write boundary and ends a slice. The 6-instruction loop is therefore
served as a 1-instruction slice and a 5-instruction slice, which is exactly the
`slice_instruction_histogram 1:30000 5:30000` above. The exit PC is the
*resume* address, so the store at `0x80000410` reports as `0x80000414` and the
store at `0x80000414` reports as `0x80000418`.

This is the cleanest possible demonstration of the problem: the single hottest
loop in the boot path pays a full scheduler round trip per store, for stores
that cannot possibly invalidate translated code.

`sim_time=180000` and 36.7s, matching the clean baseline exactly — the census
is observation-only.

## The mechanism, end to end

A guest store to RDRAM ends the block it is in, unconditionally, whenever the
store lands anywhere inside the watched executable region.

1. Every 4-byte CPU store to backed RDRAM calls `notify_cpu_instruction_store`
   — `crates/fn64-recomp-rs/src/runtime/host.rs:730` (`store_backed_word`;
   the 1-byte and 8-byte forms are at `:947` and `:1051`).
2. That calls `request_guest_write_boundary`
   (`crates/fn64-recomp-rs/src/runtime/host.rs:531`).
3. Which consults the installed boundary observer. In the live ABI that is
   `classify_live_executable_write`
   (`crates/fn64-abi/src/recompiled/snapshots.rs:983`), which returns
   `GuestWriteBoundary::ExecutableChanged` iff the store's byte range
   intersects any entry of `EXECUTABLE_WRITE_RANGES`.
4. On `ExecutableChanged`, `EXECUTABLE_WRITE_BOUNDARY` is set
   (`crates/fn64-recomp-rs/src/runtime/host.rs:541`).
5. The runner consumes it at the next architectural instruction boundary and
   converts the exit into `BlockExit::ExecutableWrite` — interpreter at
   `crates/fn64-recomp-rs/src/semantic/mod.rs:619-627`, and generically via
   `finalize_executable_write_exit`
   (`crates/fn64-recomp-rs/src/execution/mod.rs:804`).
6. The chaining loop in `dispatch_with_exception_vectoring` explicitly refuses
   to chain past it and returns the slice
   (`crates/fn64-recomp-rs/src/execution/program.rs:1511-1524`).
7. The outer runner then publishes a checkpoint and suspends the coroutine
   (`crates/fn64-abi/src/recompiled/runners.rs:1103-1114`).

### What `EXECUTABLE_WRITE_RANGES` contains

The watched set is installed from the mutation state's own watched ranges at
`crates/fn64-abi/src/recompiled/execution.rs:190-194`, and on WM2000 that is
**the whole 1 MiB boot bank** — the same region the SHA-256 hashes. The
comments at `crates/fn64-abi/src/recompiled/live_program.rs:349`, `:401`,
`:445` and `:2077` all name it as such.

This is the crux. The watched region is not "the executable bytes"; it is the
entire megabyte the IPL3 boot DMA delivered. Guest code, guest data, guest BSS
and guest stack all live inside it. So **an ordinary store to an ordinary
variable is indistinguishable from self-modifying code** and pays the full
self-modification boundary: end the block, publish a checkpoint, suspend the
coroutine, round-trip the scheduler.

WM2000's steady-state loop stores once per iteration. That is why every single
slice ends this way.

## Answers to the four questions

### 1. Why does a dispatch end after ~3 cycles when its budget is 4096?

It never consults the budget. The slice ends at the first guest store into the
watched megabyte, which for this workload is every ~3 instructions. The budget
is checked at `crates/fn64-recomp-rs/src/execution/program.rs:1432-1439` and
that branch is never taken — consistent with the already-recorded negative
result that 4096 vs 65536 produced byte-identical `sim_time` and wall time.

Note this also corrects a reasonable prior reading of the yield census. All
60,000 yields are `Yield::InstructionCheckpoint`, but that is the *yield*
type, not the *exit* type: `runners.rs:1103-1114` publishes a checkpoint and
yields `InstructionCheckpoint` for **any** exit that made progress, including
`ExecutableWrite`. The yield census could not distinguish them. The exit census
can, and says the exit is `ExecutableWrite` 60,000 times out of 60,000.

### 2. Inherent or incidental?

**Incidental, and specifically case (b): something forces a checkpoint far more
often than block boundaries require.**

The evidence that it is not inherent:

- Blocks are not the limit. Mean block length is 2.0 instructions but the
  chaining loop already follows `Transfer` without a round trip
  (`program.rs:1554-1557`); it recorded 29,999 chained `Transfer` exits. The
  machinery for multi-block slices exists and works.
- The boundary is not architectural. A store to a guest *data* address is not
  self-modifying code. It ends the block only because the watched range is the
  whole boot bank rather than the bytes that are actually executable.
- The system already distinguishes these cases elsewhere. The generation
  catalog tracks which physical spans back which executable generations
  (`live_program.rs:2266-2320`, `invalidate_pending_physical_writes_inner`),
  and `PENDING_EXECUTABLE_WRITES` vs `PENDING_ATTRIBUTED_EXECUTABLE_WRITES`
  (`snapshots.rs:960-981`) already separates "some write happened" from "a
  write intersecting an executable range happened". The precision exists; the
  *boundary decision* just does not use it.

What IS inherent, and must not be weakened:

- A store that genuinely changes executable bytes must end the block before
  the next instruction is fetched. The comment at `host.rs:538-541` names the
  exact interleaving this closes.
- The checkpoint is the only thing that advances virtual time. `advance_time`
  (`crates/fn64-runtime/src/executor/mod.rs:1032-1074`) moves `sim_time`, steps
  CP0 Count, fires the timer wheel and delivers VI retraces; `run_one_step`
  then commits the device fabric at that timestamp
  (`crates/fn64-abi/src/host.rs:320-325`). Fewer checkpoints means coarser
  device/timer granularity, which is a semantic change, not a free win.

### 3. What is the actual mean block length?

**Boot phase (60,000 steps): 2.0 instructions per block; 3.0 per slice; 1.5
blocks per slice.**

**Past the BSS clear (200,000 steps): 3.398 per block; 7.163 per slice; 2.108
blocks per slice.**

Use the second set for any steady-state projection; the first is a boot-memset
floor. Both are far below the 4096 budget, and in both cases the terminating
exit is `ExecutableWrite` essentially always (100% and 99.8%).

In the boot phase the distribution is bimodal and tiny: blocks are 1
instruction (59,999 of 89,999) or 4 instructions (29,999); slices are 1
instruction (30,000) or 5 instructions (30,000). That is the 6-instruction BSS
loop being cut at each of its two stores.

Past BSS the distribution gains a long tail — slices of 410, 411, 412 and 519
blocks appear — but the mode stays low because most guest code stores
frequently.

For scale: at 60,000 steps the guest retires 180,000 instructions in 36.7s
across 60,000 scheduler round trips. That is **one full scheduler round trip,
checkpoint publication, mutation-journal reconcile and device-fabric commit
per 3 guest instructions** (per ~7 past boot).

### 4. What would it take to execute N blocks per dispatch?

The chaining loop already exists and already executes N blocks per dispatch
when exits permit it. The question is really: what would let a slice continue
past a guest store?

**The targeted change: narrow the boundary predicate, not the journal.**

`classify_live_executable_write` (`snapshots.rs:983-996`) currently answers
"did this store touch the watched megabyte". The boundary only needs to answer
"did this store change bytes that back a *currently resident executable
generation*". Those are different questions and the second is strictly
narrower. A store to guest data inside the boot bank would then return
`Continue`, the block would not end, and the slice would chain to its budget.

What this would need, and what each part risks:

1. **A resident-code range set distinct from the watched set.** The watched set
   must stay as-is: it is what the mutation journal and every receipt digest is
   bound to (`live_program.rs:942`, `:954`). Only the *boundary predicate*
   would consult the narrower set. Getting the narrower set wrong in the
   permissive direction is a correctness bug — a genuine self-modifying store
   would be missed and stale translated code would execute. This is the one
   place that must be proven, not assumed.
2. **Attribution must not change.** `record_executable_and_renderer_write`
   (`snapshots.rs:960`) must keep pushing to `PENDING_EXECUTABLE_WRITES` and
   `PENDING_ATTRIBUTED_EXECUTABLE_WRITES` on the current, wider test.
   Attribution is what the journal's undeclared-write guard checks
   (`live_program.rs:885-896`); narrowing that would weaken a guard.
3. **Time granularity changes, even though the `sim_time` total does not.**
   This distinction matters and is easy to get wrong. `advance_time` is called
   with the slice's retired-instruction count and simply accumulates it
   (`executor/mod.rs:1642-1650`), so **total `sim_time` is exactly total
   instructions retired regardless of how they are grouped into slices** —
   180,000 instructions gives `sim_time=180000` whether that is 60,000 slices
   of 3 or 60 slices of 3,000. What changes is the *step size*: CP0 Count, the
   timer wheel and VI retrace delivery are evaluated at each checkpoint
   (`executor/mod.rs:1032-1074`), so coarser slices mean a timer that should
   have fired mid-slice now fires at the slice end. Interleaving with other
   runnable threads coarsens the same way.

   That is a real semantic change, but a narrower one than "every certified
   value moves". It would perturb any expected-closure value sensitive to
   *when* a device event landed, and `charged_instructions` is per-slice and
   part of the publication digest
   (`crates/fn64-boot-harness/src/release_gate/publication.rs:297-340`), so
   publication digests do change. Whether the run's *observable outcome*
   changes is an empirical question this investigation did not answer, and it
   is the question any implementation attempt must answer first — with the
   single-threaded WM2000 boot route it may well not, which would make the
   change far cheaper than the digest migration.

Point 3 is why this is not a drop-in optimization: it is observationally
equivalent in what the guest computes, not necessarily in when devices observe
it. But unlike the digest migration, it does **not** inherently redefine a
hashed quantity — it changes a scheduling grain. Those are different costs, and
the cheaper one has not been measured yet.

**Shortcuts that do NOT work, and what each violates:**

| shortcut | what it violates |
|---|---|
| Skip the checkpoint when nothing changed | The checkpoint is not only a journal commit; it is the sole advance of virtual time (`executor/mod.rs:1650`). Skipping it stops the clock. |
| Chain past `ExecutableWrite` in the slice loop | Exactly the interleaving `host.rs:538-541` closes: the store commits, then a later translated instruction from the pre-store image executes. The refusal at `program.rs:1511-1524` is load-bearing. |
| Raise the instruction budget | Already falsified: 4096 vs 65536 is byte-identical. The budget is never the binding constraint. |
| Batch checkpoints and publish every Nth | `publish_checkpoint` writes a per-thread map (`live_program.rs:1026-1047`) that is overwritten, so no record is lost — but the yield that follows it is what advances time, so batching still moves the timeline. |
| Disable the mutation journal | `FN64_FAST_MUTATION_JOURNAL=1` already exists as the iteration lane and does not change granularity: it skips the *comparison* (`live_program.rs:2093`), not the boundary observer, which is installed unconditionally at `execution.rs:195`. Confirmed empirically — the 200,000-step run above used it and still ended 99.8% of slices on `ExecutableWrite`. |

## What this means for the digest decision

Direct answer to the question this document was gating.

**Both factors are the same root cause: the watched region is the entire 1 MiB
boot bank.**

- The digest cost is 70% of self time because each commit hashes that megabyte.
- The 3-cycle dispatch exists because every store into that megabyte ends a
  block.

They are not two independent 5,700x and 3x problems to be attacked separately.
Narrowing what is watched — or splitting "what the journal is bound to" from
"what forces an execution boundary" — addresses both. Conversely, the page-tree
digest migration attacks only the first: it makes each commit cheap but leaves
60,000 commits, 60,000 scheduler round trips and 60,000 device-fabric
advances in place for 180,000 guest instructions.

So the honest framing for sequencing:

- The digest migration remains worth doing and remains the largest *single*
  measured line item — 70% of self time is real.
- It will not get near hardware speed on its own, and the residual it leaves is
  not a diffuse constant factor. It is one specific, identified predicate:
  `classify_live_executable_write` testing against the whole boot bank.
- The two changes are **not** equally expensive. The digest migration
  necessarily redefines a hashed quantity, which is why it needs a versioned
  schema and a full receipt regeneration. The boundary narrowing does not
  redefine any hash — it changes when checkpoints occur. Its blast radius is
  publication digests and any timing-sensitive expected-closure value, which
  may be small on this single-threaded route. **Nobody has measured that yet.**

The actionable finding: **measure the boundary narrowing before committing to
the digest migration's scope.** A one-line change to the predicate in
`classify_live_executable_write`, run against the existing gates, answers
empirically how much of the certified surface actually moves. If it turns out
to be small, it is a much cheaper win than the migration and changes what the
migration needs to carry. If it turns out to be large, the two should share one
regeneration rather than forcing two.

The BSS-clear attribution makes the upside concrete. The destination range is
`[0x8004b4c0, 0x800b1390)` — entirely data, and entirely inside the watched
bank. A predicate that excluded it would let that loop chain to its 4096
budget instead of ending every 1-2 instructions, with the guest computing
bit-identical results.

The 200,000-step run supplies the existence proof that this pays off generally:
slices of 410-519 blocks already occur wherever the guest happens not to
store. That is the behaviour the predicate is suppressing everywhere else.

### Scoping: the 60,000-step benchmark is entirely inside this one loop

The BSS clear needs 52,186 iterations × 6 instructions = **313,116
instructions and ~104,372 slices**. The standard benchmark stops at 180,000
instructions and 60,000 slices, so **it never finishes the BSS clear** — the
two-PC census confirms every slice end in that run is one of those two stores.

So "3 guest cycles per dispatch" characterizes the boot memset specifically.
Running past it changes the numbers but not the conclusion.

### Past the BSS clear: 200,000 steps

```
[dispatch-census] slices=199751 instructions=1430877 blocks=421144
                  instructions_per_slice=7.163 blocks_per_slice=2.108
                  instructions_per_block=3.398
[dispatch-census] slice_exit={"Checkpoint": 177, "ExecutableWrite": 199292,
                              "ExecutableWriteResolveCall": 9,
                              "HostCall": 272, "ThreadReturn": 1}
[dispatch-census] site ExecutableWrite pc=0x80027154 count=93596
[dispatch-census] site ExecutableWrite pc=0x80000414 count=52186
[dispatch-census] site ExecutableWrite pc=0x80000418 count=52186
```

(`sim_time=1461877`, `thread0_dead=true`, 110s with
`FN64_FAST_MUTATION_JOURNAL=1`.)

The BSS clear completes — 52,186 at each PC, exactly the predicted iteration
count — and a different hot store at `0x80027154` takes over for nearly half
the remaining run.

What this settles:

- **Granularity improves ~2.4x once real code runs**: 7.163 instructions per
  slice, 3.398 per block. So the 3.0 figure is a boot-phase floor, and any
  steady-state speedup projection should use ~7, not ~3.
- **The mechanism is not a boot artifact.** `ExecutableWrite` still ends
  **199,292 of 199,751 slices — 99.8%**. Budget exhaustion accounts for 177.
  Store-forced slice ends dominate real game code just as thoroughly.
- **Chaining works and is being suppressed.** The `slice_block_histogram` tail
  contains slices of 410, 411, 412 and 519 blocks — where the guest happens
  not to store, the loop chains hundreds of blocks into one dispatch. That is
  direct evidence the ceiling is the store predicate and nothing else.

That experiment is cheap — the census in this document is confined to
`fn64-abi`, so the iteration loop is a single-crate rebuild — and it is the
obvious next step. This investigation deliberately stopped short of it: the
brief was analysis, and changing the predicate is a correctness-sensitive
change that deserves its own task with its own verification.

## Reproducing

The census is gated on `FN64_DISPATCH_CENSUS` and is observation-only —
`sim_time` is byte-identical with and without it.

```
cd examples/wm2000-block-boot
source ../../.claude/local.env
export ROM="$FN64_DISCOVER_NWXE_ROM"
C=~/Code/aki-recomp/captures; G="$C/wm-general-exception-images"
export FN64_EXECUTABLE_IMAGES="$G/run-1/image.json:$G/run-2/image.json:$G/run-3/image.json"
export FN64_BOOT_CONTEXT="$C/wm2000-boot-context.json"
export FN64_ABSENT_N64DD=1 FN64_BLOCK_MAX_STEPS=60000 FN64_DISPATCH_CENSUS=1
time ./target/release/wm2000-block-boot 2>&1 | grep -E 'census|done: steps'
```

For the past-BSS numbers, raise the step count and use the fast journal lane so
it finishes in ~110s instead of ~10 minutes (the journal does not affect
granularity — see the shortcuts table):

```
FN64_BLOCK_MAX_STEPS=200000 FN64_FAST_MUTATION_JOURNAL=1 \
  ./target/release/wm2000-block-boot 2>&1 | grep -E 'census|done: steps'
```

Census implementation: `dispatch_census` in
`crates/fn64-abi/src/recompiled/runners.rs`, recorded at the static runner's
dispatch site. It is deliberately confined to `fn64-abi`. `DispatchRun`
already carries `{ exit, instructions, blocks }`
(`crates/fn64-recomp-rs/src/execution/mod.rs:1035-1039`), so the per-block mean
is derivable from the slice census alone and no instrumentation inside
`fn64-recomp-rs` is required.

Two reasons that matters. `fn64-recomp-rs` is a dependency of all 32 generated
shard crates, so editing it rebuilds all of them. More importantly,
`src/lib.rs` and `src/execution/program.rs` are both members of
`DYNAMIC_MAPPED_EXECUTION_LIBRARY_SOURCES_V1`
(`crates/fn64-recomp-rs/src/lib.rs:91-154`), which is hashed into the dynamic
mapped execution source identity — instrumenting them would have changed a
certified identity value. An earlier draft of this census did exactly that and
was reverted.

## Note on the dynamic lane

The default benchmark runs the **static** catalog path
(`run_catalog_block_program`, `runners.rs:1006`). The dynamic mapped lane is
behind the non-default `dynamic-withheld` feature
(`examples/wm2000-block-boot/Cargo.toml:29-32`) and was not exercised here.

Worth recording for whoever touches that lane: its unit is far smaller still.
`DynamicMappedUnitCatalogV1::activate_and_run_with_memory_port`
(`crates/fn64-recomp-rs/src/fetch.rs:495-545`) snapshots **one instruction, or
one branch/delay pair**, hashes it into a unit identity, admits a synthetic
bank through a `BTreeMap`, and runs it — asserted at
`crates/fn64-recomp-rs/src/semantic/mod.rs:481-484`. A straight instruction
then falls out of its own one-word unit and returns `ResolveTransfer`
(`semantic/mod.rs:639-645`), so that lane re-snapshots and re-hashes per guest
instruction. If it ever becomes the default, this analysis needs redoing
against it.
