# WM2000 playability blocker — hypothesis ledger

Goal: WM2000 recompile 100% playable through the fn64 pipeline (discovery →
runtime → render). This file is the working record for the blockers in front
of that.

**Correction (2026-08-06).** An earlier revision of this file said "Current
state: `gfx_submits=0`; no display list is ever submitted." That was an
artifact of the repro command, not a rendering blocker. The command omitted
`FN64_CONTROLLER_SCHEDULE`, so every controller read returned
`ContInput::default()` and the guest never left the audio-driven attract
sequence -- it legitimately never builds a display list without input.
`docs/BOOT-NOTES-WM2000.md` records the SAME harness binary reaching 175
graphics tasks at 100k steps (:1643), 638 at 240k (:1725), and 1,707 at 420k
(:1823) whenever a schedule is supplied. Rendering is not the blocker; run
scheduled routes via `scripts/wm2000-route-probe.zsh`.

The "732 VI swaps / uniform fill / the fade cover never lifts" note quoted in
earlier planning is from a DIFFERENT harness (`examples/wm2000-boot`, the
aki-recomp C lane), where the game had already reached gfx task #1241. It is
not evidence about this lane.

Rule for this file: a hypothesis leaves the OPEN table only with a measurement
attached. Five hypotheses died twice across sessions because the evidence lived
in transcripts instead of here.

## The blocker

```
unjournaled executable mutation changed physical RDRAM [0x0009b0b3, 0x0009b0b4)
expected=Some(0) live=Some(1)
journal_entries=104420        # identical every run — fully deterministic
```

Reached at step ~1,183,304 after entering a second overlay at
[0x8011c900,0x801226f0). Prior to the baseline fix the route died at 421,717.

## Writes to 0x0009b0b3 — measured, both seams instrumented

| # | Writer | Value | Attributed? |
|---|--------|-------|-------------|
| 1 | `write_logical_bytes [0x400,+0x100000)` (boot publication) | ROM byte `0x10` | yes |
| 2 | `write_u8 [0x9b0b3,+0x1)` (boot publication) | — | yes |
| 3 | `store_backed_word [0x9b0b0,+0x4)` (guest CPU store) | `0x0` | yes — `seq=81661 CpuInstructionStore` |
| 4 | **`mirror_queue_to_rdram [0x9b0b0,+0x4)` ×2** — FOUND | `0x1` | **no — the blocker** |

Writer 3 legitimately explains `expected=0`: the guest zeroed the word, the
store was declared, the baseline advanced.

## Root cause (CONFIRMED by measurement)

`Executor::mirror_queue_to_rdram` (`crates/fn64-runtime/src/executor/mod.rs:666`)
mirrors guest `OSMesgQueue` fields into RDRAM with raw
`std::ptr::copy_nonoverlapping`, bypassing every view type and every
`notify_*_write`. WM2000 has a queue at guest `0x8009b0b0`; a `validCount` of 1
writes native `01 00 00 00` at storage offset `0x9b0b0`, and since storage
offset `o` is logical byte `o^3`, the `01` lands at logical `0x0009b0b3` --
inside a watched executable range.

Proven with a temporary probe: `FN64_WATCH_WRITE=0x9b0b3` printed
`mirror_queue_to_rdram [0x0009b0b0,+0x4) covers 0x0009b0b3` twice.

Note this is a swizzle effect, but in the *writer*, not in the snapshot/baseline
comparison -- the latter remains dead as a hypothesis.

The repair is attribution, not suppression: the mirror is a legitimate host
write that must declare itself on the `HostAbi` channel, exactly as fn64-abi's
sibling scheduler running-thread mirror already does
(`recompiled/execution.rs:698-710`). `fn64-runtime` cannot call the recompiler
crate in production (dev-only, deliberately one-way, `Cargo.toml:18-23`), so the
host installs a callback.

In-tree corroboration written before the cause was known
(`live_program.rs:2049-2052`): *"at least one path reaches RDRAM without passing
through `record_executable_and_renderer_write`."* This is that path.

### Instrumentation coverage (why #4 is invisible)

Two unrelated types are both named `Rdram`, and until this session only one was
watched:

- `fn64_runtime::Rdram` + its views — watched by `watch_raw_write`
  (`crates/fn64-runtime/src/rdram.rs:464`).
- `fn64_recomp_rs::runtime::host::Rdram` (`runtime/host.rs:20`) — what
  recompiled guest code stores through; writes `self.mem[..]` directly. Now
  watched by `watch_guest_store` at `store_backed_word`/`store_h`/`store_b`/
  `store_d`.

Known remaining gaps: `Rdram::as_mut_slice()` (`host.rs:574`, documented
in-tree as "an UNATTRIBUTED write path"; sole non-test caller
`recompiled/runners.rs:1331` hands a raw pointer to a generated C shim),
`RdramViewMut::write_u16` (`rdram.rs:593`), `RdramPtr::write_u32`
(`rdram.rs:512`), `RdramPtr::write_u16` (`rdram.rs:553`),
`fn64_runtime::Rdram::write_bytes` (`rdram.rs:757`), and any DMA path writing
through `ProcessDmaMemory` or a raw pointer.

## Dead hypotheses — do not re-propose without new evidence

| Hypothesis | How it died |
|---|---|
| Byte-lane swizzle mismatch between snapshot and baseline | ROM word at 0x9b0b0 is `a4 45 00 10`; lane-XOR-3 gives `10 00 45 a4`. Neither order yields 0x00 or 0x01. Also killed earlier in `a2d1982`/`ba0af45`. |
| `FN64_FAST_MUTATION_JOURNAL` gate skipping a baseline-advancing read | Flag is unset in these runs. |
| Device-advance empty `RdramViewMut` (site 1) | Fixed in `121a8cf`/`8aaf654`. Note the fix commit's stated mechanism is itself wrong: an empty view cannot silently zero — `RdramView::range` (`rdram.rs:294-299`) asserts. It changed behavior via device-advance timing. |
| Device-advance empty `RdramViewMut` (site 2, `pi/timing.rs:390`) | Patched it to describe the real allocation: broke 13 tests AND left the panic byte-identical. Reverted. That empty view is legitimate. |
| `expected` seeded before publication ("sealed too early") | `FN64_BASELINE_PROBE` in `121a8cf` measured `expected[0x9b0b3]=Some(16)` = 0x10, correct at boot. Superseded. |
| A second RDRAM allocation (`expected` and `live` reading different memory) | `boot_thread0_validated_catalog_generation_program_v1` (`execution.rs:1323-1325`) *moves* `validated.storage` into the process RDRAM. Same buffer. |
| Overlapping watched ranges consulting different range objects | `CanonicalExecutableMutationStateV1::new` (`live_program.rs:44-49`) asserts `physical_start > previous_end`. |
| `covering_declarations=2` in the panic proves the delta was declared | That filter (`live_program.rs:539-550`) scans the ENTIRE journal history; `seq=81661` of 104420 is an old acceptance. Misleading as written. |

## Status: RESOLVED in `0a13c34`

H1 (a DMA/device writeback) and H2 (`as_mut_slice()` into a C shim) were both
wrong; the probe named `mirror_queue_to_rdram` directly, so neither needed
elimination on its own merits.

The fix is a pre-write publisher rather than a post-write notifier: the
ordering-boundary assertion requires a child writer's bytes to be declared AND
committed before any host transaction reaches a boundary, which notifying after
the bytes are visible cannot satisfy. `recompiled/host_memory.rs:64`
(`write_guest_physical`) already packages that, so it installs as a bare fn
pointer. The three fields are contiguous and every call site rewrites all
three, so one 12-byte declaration replaces three.

A/B verified against a rebuilt baseline: without the fix, exit 134 at
0x0009b0b3, and the mirror is absent from that panic's `covering_declarations`;
with it, exit 0. 687/687, 401/401, `grade-all.sh` wrong=0 on all five.

## Open

- **Where does the route terminate now?** With
  `FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1` it runs past the 10-minute mark without
  aborting, where it previously died at ~1,183,304. Endpoint not yet recorded.
  Note a run WITHOUT that flag stops at ~421,692 with `thread0_dead=true` by
  design at overlay entry -- not a regression, just a different stop condition.
- **Next blockers are scoped, not started:** see
  `rdram-write-attribution-audit.md` (the remaining unattributed writers and
  the one structural change that would end this bug class) and
  `per-title-shard-generation.md` / the title-generic boot lane (what the other
  four AKI titles need in order to boot at all).

## Reproduce

```
cd examples/wm2000-block-boot
export ROM="$FN64_DISCOVER_NWXE_ROM"
C=~/Code/aki-recomp/captures; G="$C/wm-general-exception-images"
export FN64_EXECUTABLE_IMAGES="$G/run-1/image.json:$G/run-2/image.json:$G/run-3/image.json"
export FN64_BOOT_CONTEXT="$C/wm2000-boot-context.json"
export FN64_ABSENT_N64DD=1 FN64_BLOCK_MAX_STEPS=1300000
export FN64_WATCH_WRITE=0x9b0b3          # add FN64_WATCH_WRITE_BACKTRACE=1 for stacks
./target/release/wm2000-block-boot
```

Deterministic — one pass suffices.

## 2026-08-06: WM2000 renders and takes input

The first measured run in this repository showing graphics, audio, and
controller input live at once, from committed tooling.

```
[wm2000-block-boot] controller input_edge port=0 read=90 buttons=0x1000
  stick=(0, 0) step=2412333 sim_time=328071095
  gfx_submits=88 audio_submits=191
  generations=[16226209253856221389, 17518568401266107605]
[wm2000-block-boot] controller input_edge port=0 read=100 buttons=0x0000
  step=2528050 sim_time=359624297 gfx_submits=98 audio_submits=210
```

`buttons=0x1000` is START at controller read 90 -- exactly what the route
recipe annotates as "Title screen: START to enter the main menu". Graphics
submits climb with the route, `render_error=None`, no panic.

Program identity `b26d98af4aaaab86...`. Reproduce:

```
cd examples/wm2000-block-boot
source ../../.claude/local.env
export ROM="$FN64_DISCOVER_NWXE_ROM"
C=~/Code/aki-recomp/captures; G="$C/wm-general-exception-images"
export FN64_EXECUTABLE_IMAGES="$G/run-1/image.json:$G/run-2/image.json:$G/run-3/image.json"
export FN64_BOOT_CONTEXT="$C/wm2000-boot-context.json"
export FN64_ABSENT_N64DD=1 FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1
export FN64_CONTROLLER_SCHEDULE=../../reference/wm2000-routes/entrance-to-match.schedule
export FN64_BLOCK_MAX_STEPS=12000000
./target/release/wm2000-block-boot
```

**Budget at least 25 minutes of wall clock before the first graphics submit.**
Two investigations today mistook a too-short run for a rendering failure. At
~1,450 steps/s the first overlay entry is ~5 min in and the first submits
~25 min in; a 420,000-step run covers 8.9 NTSC fields and legitimately shows
`gfx_submits=0`.

What unblocked it was `a7a50fe` -- the overlay digest extent. Nothing about the
render path changed.

### Still open

- **Throughput: the checkpoint digest.** A step advances **3 sim cycles**
  (`sim_time=180000` for `steps=60000`, clean HEAD), so the gap to hardware is
  ~31,000x. A frame-pointer profile at 200k steps attributes **70.30% of self
  time to `sha2::sha256::aarch64::compress`**, against **0.06% for the
  recompiled guest code** and 0.03% for `advance_device_time`. The stack is
  `run_catalog_block_program -> commit_snapshot -> digest_snapshot -> sha2`:
  every commit that changed anything re-hashes the full 1.14 MiB watched
  region.

  Corroborated independently from the other end: a census of
  `Executor::handle_yield` over the 60k benchmark reports
  `{"InstructionCheckpoint": 60000}` -- **100% of slice ends are checkpoint
  publications**, never budget exhaustion, device access, or shims. This also
  explains why raising `FN64_BLOCK_INSTRUCTION_BUDGET` from 4096 to 65536
  produced byte-identical `sim_time` and wall time: the slice never ends on
  budget.

  Not fixable as a perf change. `expected_sha256` feeds `journal_root_sha256`
  and is cross-checked against `watched_bytes_sha256`
  (`recompiled/receipts.rs:1252`) across the receipt chain and the gates, so a
  page-tree digest would change every certified evidence value in the project,
  including the byte-exact rebuild proofs. That is a certification decision --
  see `docs/plans/checkpoint-digest-cost.md`.

  Three hypotheses were falsified by measurement before the profile settled it,
  all recorded here so they are not re-proposed: the journal snapshot modelled
  as ~100% of runtime (it is ~20% at 60k); the per-dispatch scheduler mirror at
  `host.rs:312` (gating it out entirely: 38s -> 37s, ~3%); and a larger
  per-dispatch instruction budget (no effect at all).
- **A live window.** The display lists are produced; that they reach a window
  has never been shown.

## 2026-08-06: WM2000 renders its copyright screen, legibly

Frame dumps from the certified dense-AOT block program show the recompiled
game drawing real, readable content -- not a uniform fill.

`FN64_RENDER_DUMP_DIR` (wiring at `examples/wm2000-block-boot/src/main.rs:800-828`)
produced 40 PNGs at **480x240**, the game's real VI width rather than the 320
default. Frames 0-7 are a monotonic fade whose dominant background steps by
exactly `0x21` per frame (`e7 c6 a5 84 63 42 21 00`), and the foreground is
WM2000's copyright screen:

> (c)1999 Asmik Ace Entertainment / AKI
> WRESTLEMANIA 2000
> (c)1999 World Wrestling Federation Entertainment, Inc.
> ... Gangrel created by White Wolf, Inc. ... Licensed by Nintendo

Two frames are retained as evidence in `reference/wm2000-frames/`.

One reading correction worth recording: an automated decode of these frames
reported "96.23% black with a 4339-pixel static white set riding above the
fade", and read that set as a featureless overlay. Those 4,339 pixels are the
**text glyphs**. A pixel-statistics summary could not distinguish "static
overlay" from "legible typography"; looking at the image could. Prefer viewing
a frame over summarizing it.

Every frame logs `NON-CLEAR (0 tris)`: this content is fills and rectangles,
not rasterized geometry, which is correct for a copyright screen. Triangle
geometry remains unproven -- it belongs to menus and the match, deeper in the
route.

### The live window

A purpose-built `wm2000-shell` binary (2nd `[[bin]]` of
`examples/wm2000-block-boot`, `src/shell.rs`) runs the SAME certified program
as the headless gate via `construct_catalog_program`. Confirmed open at
1280x960, titled "fn64 -- WM2000 (dense AOT block program)", reference renderer
registered, cpal audio live, `first-entry BootContext matches exactly`, and no
panic on gfx dispatch. Launch it with `nohup` -- a foreground launch was
SIGTERM'd at 12 minutes.

RT64 does not apply to this lane, correcting an earlier assumption:
`examples/wm2000-block-boot/Cargo.toml:40-41` depends only on
`fn64-render`/`fn64-render-reference`. RT64 is wired into `crates/fn64-shell`,
the function lane, which boots a linked whole-ROM crate; WM2000 is a dense-AOT
shard catalog. Different contracts, not variants -- the reference backend is
the correct choice here, not a fallback, and the recorded RT64 speedup does not
transfer.

The `task_dispatch.rs:295` gfx-dispatch blocker recorded in earlier notes is
already fixed. That file is now a module directory, and the KSEG0->physical
mask is covered by
`crates/fn64-abi/src/task_dispatch/tests/dispatch_a.rs:970`, which cites the
original panic and asserts `0x8038ce30 -> 0x0038ce30`.


## 2026-08-06: deepest verified run -- 12M steps, clean exit

The scheduled `entrance-to-match` route ran to its full 12,000,000-step budget
and exited cleanly. No panic, no `unjournaled` mutation, no `AotMiss`.

```
done: steps=12000000 sim_time=1912427205 thread0_dead=true
gfx_submits=694 audio_submits=1120
process exit prepared: threads=10 detached_coroutines=9
```

It reached controller **read 560 of 1400** -- through the copyright screen,
the Exhibition and match-type menus, the rules page, the Decision page that
commits match setup, and into the entrance/versus presentation. Three recovered
generations stayed resident throughout; the catalogued fourth
(`3068194456377681093`) was still not entered, consistent with the note at
`docs/BOOT-NOTES-WM2000.md` that no retained route recipe reaches it.

**It stopped on the step budget, not on a fault and not at the end of the
route.** The remaining 840 controller reads need roughly 3x the budget, which
at current throughput is several more hours. Depth is now purely a throughput
question -- see `docs/plans/dispatch-granularity.md`.


## 2026-08-06: the v2 page-tree digest -- 2.9x, independently reproduced

The checkpoint digest migration landed. `digest_snapshot` no longer hashes the
whole watched region on every commit; each range is partitioned into 4096-byte
pages with a per-page SHA-256 leaf and a root over the leaves, so a commit
rehashes only the pages whose bytes changed. Measured: **1.005 page rehashes per
commit**.

| | before | after |
|---|---|---|
| 60k benchmark | ~36.5s | **12-13s** |
| throughput | 1,644 steps/s | **4,800 steps/s** |
| 200k route | 107.5s | 43.2s |
| SHA-256 self time | 70.30% | 2.3% |
| `sim_time` (60k) | 180000 | 180000 |

Verified independently of the implementing agent: 694/694 abi+runtime,
401/401 recomp-rs, `grade-all.sh` wrong=0 on all five, and a live route running
clean with zero `unjournaled`/`AotMiss` failures.

**The expected certification cost did not exist.** This migration was gated
behind three other investigations on the belief that it would force receipt-chain
regeneration across gates, fixtures and docs. An exhaustive search found **zero
hardcoded digest literals** over watched executable memory: `fn64-abi` owns the
chain and computes it end to end, so no fixture, gate, test or reference edit was
required. The blast radius was estimated from architecture rather than from
grep, and that misestimate cost real sequencing time.

`watched_bytes_sha256` deliberately stays v1 and flat. It is the independent
bootstrap cross-check, runs once, and is not hot -- making it a second page tree
would have the two agree by construction rather than by evidence.

### What this buys in practice

The 12M-step route that took ~2h20m now takes ~48 min, and the full route to
controller read 1400 drops from ~5.1 h to ~1.7 h -- from impractical to
runnable.

### The bottleneck moved rather than vanished

Post-migration self time: `memcmp` 2757, `memmove` 1456,
`current_changed_ranges` 1029, `copy_logical_bytes` 920, `set_expected` 915,
`sha2` 173. That is the 1.44 MiB copied out of RDRAM per commit purely so the
comparison has a contiguous buffer -- addressable by comparing in place, which
redefines no hashed quantity.


## 2026-08-06: cumulative 7.9x, and the profile is now flat

Two optimizations landed in sequence. The 60k benchmark, independently
re-measured after each:

| stage | 60k | sim_time |
|---|---|---|
| session start | ~36.5s | 180000 |
| v2 page-tree digest | 12-13s | 180000 |
| in-place watched comparison | **4.6s** | 180000 |

**7.9x cumulative**, with `sim_time` byte-identical at every stage and progress
counters byte-identical at 200k.

The second change removed the per-commit snapshot copy. Its only consumers were
a comparison against `expected` and a baseline update needing just the differing
bytes, so both now read RDRAM in place through the pre-reversed storage-order
mirror; only changed ranges are materialized, and only the pages they touch are
rehashed.

Profile confirmation on a live route, sampled from the running binary rather
than taken on report: `memcmp` 2757 -> **22** samples, `memmove` 1456 -> 11,
`current_changed_ranges` 1029 -> 11, `sha2` -> 8. The snapshot machinery that
was ~52% of self time is now marginal. There is no longer a single dominant
hotspot in the runtime.

Practical effect: the full route to controller read 1400 goes from ~5.1 h at
session start to roughly 40 min.

### Still open: commit frequency

100% of slices still end on `BlockExit::ExecutableWrite` at ~7.163 instructions
per scheduler round-trip, while the census shows slices reaching 519 blocks
wherever the guest does not store. The resident-generation predicate is the
remaining lever and its safety argument has been verified:
`activate_for_fetch_with_digest` re-digests live memory unconditionally BEFORE
consulting `self.active`, so a generation activated later over bytes written
earlier cannot execute stale code -- it re-digests and returns `AotMiss`.
`guest_write_token` has no non-test consumers, so nothing short-circuits that.


## 2026-08-06: the resident-generation boundary -- the store-forced dispatch is gone

`classify_live_executable_write` asked only "does this write land in the watched
executable region" -- a region covering every byte any generation could EVER
back. It now also asks whether any **currently resident** generation is backed
by those bytes.

A go/no-go probe run before any code was written: **0 of 199,588 watched stores
touched a resident generation.** Every one of them was forcing a scheduler
round-trip for nothing.

| | before | after |
|---|---|---|
| 1,461,877 guest instructions | 18.29s | **1.27s** (14.4x, re-measured independently) |
| slices for that work | 199,751 | **625** |
| instructions per slice | 7.163 | **2339.0** |
| `ExecutableWrite` exit share | 99.8% | **0** |

Remaining exits are `Checkpoint` (budget) and `HostCall` -- genuine boundaries.

**Why this is safe, verified rather than assumed.** The digest loop in
`activate_for_fetch_with_digest` (`generation/mod.rs:799-821`) runs over all
containing candidates BEFORE the `already_active` check at :846, so a generation
activated later over bytes changed while nothing was resident re-digests live
memory and returns `AotMiss` rather than executing stale code.
`guest_write_token` has only test consumers, so no activation path
short-circuits that digest. Attribution stays wide at `snapshots.rs:970`; only
the boundary at `:986` narrowed.

The predicate fails safe: when residency cannot be determined -- `HOST` already
borrowed, no canonical program, catalog borrowed -- it returns `true` and breaks
the block. `unwrap_or(true)`, not `unwrap_or(false)`.

**The re-entrancy hazard was real.** `advance_device_time_step` holds
`with_host` open across a device write that reaches the boundary observer; the
first build aborted with "RefCell already borrowed". Fixed with a
non-panicking `try_with_host`.

### A measurement correction that invalidates earlier step counts

**`FN64_BLOCK_MAX_STEPS` bounds SLICES, not instructions.** With slices ~326x
longer, a "60,000 step" run now executes 2,670,280 guest instructions instead of
180,000. Every step-count comparison in this document predating this change
measures a different amount of guest work on either side, including the "60k
benchmark = 4.6s" figure.

**Pin `FN64_BLOCK_MIN_GUEST_INSTRUCTIONS` for any A/B from here.** The 14.4x
above was measured that way, toggling `FN64_DISABLE_RESIDENT_BOUNDARY` within a
single binary.

Determinism: every device counter byte-identical at the deepest reachable state
(`device_trace=15389 pi_started=3823 sp_tasks=7 rcp_completed=7
vi_interrupts=8`). Only `trace` differs -- it counts scheduler round trips, the
quantity this change eliminates.


## 2026-08-06: 233x faster this session -- 19,000x from hardware down to 81x

Measured on pinned guest work (1,461,877 instructions in 1.269s):
**1,151,991 guest instructions/sec against the N64's 93.75 MHz = 81x slower
than hardware.** At session start the same measurement was ~19,000x.

Three changes, each verified independently of the agent that wrote it, each
leaving `sim_time` and the device counters byte-identical:

| change | effect |
|---|---|
| v2 page-tree checkpoint digest | SHA-256 70.3% -> 2.3% of self time |
| in-place watched-byte comparison | `memcmp` 2757 -> 22 samples; snapshot copy eliminated |
| resident-generation boundary | `ExecutableWrite` exits 99.8% -> 0; instructions/slice 7.2 -> 2339 |

**The profile is now flat.** Sampled on a live route: `memcmp` 24,
`current_changed_ranges` 12, `sha2` 9, `memmove` 9, `advance_device_time` 7,
`run_one_step` 6, and the new `classify_live_executable_write` predicate 2.
Nothing dominates; there is no next obvious lever of the kind the previous three
were.

What that buys: a 60-second gameplay segment (~5.6B guest instructions) now
takes ~81 minutes rather than ~10 days. Real-time is 81x away rather than four
orders of magnitude, which makes it an engineering target rather than an
aspiration.

None of this touched the recompiled guest code, which was 0.06% of self time
throughout. Every win came from runtime bookkeeping around it.


## 2026-08-07: 16x from hardware, and the guest code is still invisible

Measured on pinned guest work (1,461,877 instructions in 0.278s warm):
**5,847,508 guest instructions/sec = 16.0x slower than the N64's 93.75 MHz.**
Session start was ~19,000x.

The remaining cost was `read_snapshot` (`live_program.rs:393`) at **74.9% of
self time** -- it materializes the whole 1 MiB watched region through a per-byte
`FnMut(u32) -> u8` closure. The word-wise alternative already existed and was
documented; three call sites had simply never been converted, all with the RDRAM
allocation already in hand:

- `execution.rs:469` (`advance_device_time`) and `host_memory.rs:110`
  (`write_guest_physical`), via a new `commit_with_optional_view`
- `flush_host_abi_transaction`, which built a snapshot solely to feed
  `current_changed_ranges`
- `execution.rs:524` (`checkpoint_catalog_host_transaction_before_suspend`)

Equivalence is byte-identical, verified by `diff` of complete run output across
both lanes -- `sim_time`, `scheduler_steps`, RDRAM SHA-256, and every device
counter. `trace` unchanged confirms the change removed work *per boundary*
rather than removing boundaries.

### A projection of mine that was wrong

I predicted the recompiled guest code would now be ~12% of runtime, reasoning
that it was 0.06% before a 233x overhead reduction and would therefore emerge as
dominant. **It does not appear in the profile at all (<0.2%).** The reasoning
failed because it assumed guest work per boundary held constant while overhead
fell; instead the wins scale with boundary count, and guest work per boundary
rose alongside. We are still in the runtime-bookkeeping project, and codegen
remains irrelevant until guest code is visible at all.

Also settled: `codegen-units=16` on the shards gives a 9-minute build for 10%,
and `codegen-units=1` + `lto=thin` + `target-cpu=native` together measured 2.3x
SLOWER. Codegen tuning is not the lever.

### Where the remaining cost is

**94% is `_platform_memcmp`** -- the correctness scan, running at memory
bandwidth. 1 MiB `memcmp` measures 19.4 us / 54 GB/s on this machine, and the
deep route's ~19,523 boundaries x ~3 scans x 19.4 us is the same order as its
total runtime. The scan is already one `memcmp` per range with a chunked
fallback; there is no constant factor left in it.

Further gains require **reducing scan volume, not scan cost**: fewer boundaries,
or a watched region smaller than the 1 MiB boot bank. The latter is the "narrow
code map" already closed by measurement (WM2000 zeroes its own loaded code
image), so boundary count is the honest next lever.


## 2026-08-07: 15.5x, and the O(watched bytes) floor is proven

Two more levers landed (`e68af57`, `6cd4980`): the HostAbi boundaries scanned
the watched region twice, and `classify_live_executable_write` deep-cloned the
whole program on every watched store. 271.1 ms -> 240.7 ms; re-measured
independently at **229 ms = 6,065,880 guest instructions/sec = 15.5x slower than
hardware.** Complete run output `diff`-identical in every lane.

Note the profile mis-ranked these: it predicted 7% for the double scan and 4%
for the clone; measured, the clone paid 7.0% and the double scan 4.3%.

### The incremental reconcile is not viable -- proven, not abandoned

The largest remaining lever (57.6% of runtime, the watched-region scan) cannot
be made incremental. The argument is arithmetic, not soundness, so it would have
failed even with a perfect soundness story:

`expected_page_digests` is **a cache of the baseline, not an observation of live
RDRAM.** All five call sites of `watched_page_digest_v2` hash either
`self.expected` (`mod.rs:642`, `:657`, `:895`) or a caller-supplied snapshot
(`live_program.rs:425`). None hashes live RDRAM. To make a page digest answer
"did live RDRAM change", it must be recomputed from live RDRAM -- and SHA-256
reads every byte of the page to do so. That is the same 1,513,056 bytes the
`memcmp` reads, with per-byte compression instead of a vectorized compare and
without `memcmp`'s early exit. **Strictly worse.**

The 1.005 page-rehashes-per-commit figure is a saving on *hashing*, obtained by
consuming a comparison the scan already paid for (`refresh_page_digests` derives
its dirty set at `mod.rs:654` with a full-region `memcmp`). It is downstream of
the scan and cannot replace it.

**The general form:** the guard's cost IS the read of the watched region, and
every candidate substitute -- digest, checksum, Merkle path -- must perform that
read to be trustworthy. Only a mechanism that learns of writes *without reading*
could break the O(watched bytes) floor: hardware dirty bits, or `mprotect`
write-protection with a fault handler.

That is the honest boundary of software-only optimization here.


## 2026-08-07: the `mprotect` write barrier — measured, and it is favourable

The previous section named `mprotect` write-protection as one of only two
mechanisms that could break the O(watched bytes) floor, and left it unmeasured.
It is now measured. **The arithmetic works**, which was not the expected
outcome — the prior was that Mach fault delivery would be too slow.

Everything below is measured on this machine (Apple M5 Pro, 16384-byte pages,
93 pages over the 1,513,056-byte watched region). The microbenchmark is
standalone and links nothing from this repository.

### The two costs

| quantity | measured |
|---|---|
| `memcmp`, whole region, all-equal (the scan being replaced) | **26,525 ns** (57.0 GB/s) |
| whole-region `mprotect` protect+unprotect, no fault | 2,253 ns |
| single-page `mprotect` protect+unprotect, no fault | 597 ns |
| protect whole region + take **one** write fault + re-arm that page | **3,541 ns** |
| marginal cost per additional fault (least squares over n=1..32) | 2,935 ns |
| fixed cost per boundary (same fit) | 740 ns |

A fault is therefore **~7.5x cheaper than the scan it replaces** at one fault
per boundary, and the two cross over at **~9 distinct pages written per
boundary**:

| faults/boundary | ns/boundary | vs 26,525 ns scan |
|---|---|---|
| 1 | 3,541 | 7.49x cheaper |
| 2 | 6,523 | 4.07x cheaper |
| 4 | 12,571 | 2.11x cheaper |
| 8 | 23,616 | 1.12x cheaper |
| **16** | 48,865 | **1.84x MORE** |
| 93 | 276,684 | 10.43x MORE |

So the entire question reduces to one empirical number that had never been
measured: **how many distinct 16 KiB pages does the guest write between two
dispatch boundaries?**

### The page census — the number that decides it

Instrumented at `record_executable_and_renderer_write` (every observed guest
write) and closed at `matches_view` (exactly one scan = one boundary), behind
`FN64_MPROTECT_CENSUS=1`. Inert when unset — `sim_time`, `steps` and wall time
are unchanged with the probe compiled in and disabled (2.90s both ways).

Deep scheduled route, `entrance-to-match`, 400,000 steps:

```
boundaries=840169  distinct_pages_total=568514  mean_pages_per_boundary=0.6767
     0 page(s):     457790 boundaries (54.49%)
     1 page(s):     280938 boundaries (33.44%)
     2 page(s):      66088 boundaries ( 7.87%)
     3 page(s):      17081 boundaries ( 2.03%)
     4 page(s):       7464 boundaries ( 0.89%)
   >=5 page(s):       10799 boundaries ( 1.29%)
```

**Mean 0.68 pages per boundary. 54.5% of boundaries write no watched page at
all, 98.7% write four or fewer, and only 0.054% exceed the 9-page break-even.**
Reproduced on the shorter pinned workload (49,910 boundaries, mean 0.53), so
the shape is not an artifact of one route.

The 54.5% zero-page case is the important one: those boundaries pay a full
26,525 ns scan today to discover that nothing changed, and would pay only the
re-arm under a barrier. This is the same "nothing changed is the overwhelmingly
common answer" observation that motivated `matches_view`, now quantified — and
it is precisely the case a read-based guard can never exploit, because it must
read to learn it.

### Projection

Weighting the measured per-fault costs by the measured distribution:

| | |
|---|---|
| mprotect cost per boundary | **2,726 ns** |
| scan cost per boundary | 26,525 ns |
| component speedup | **9.7x** |

`memcmp` is **1292 of ~1400 leaf samples (93%)** in a live `sample` of the
running binary, so with Amdahl applied the projected whole-run speedup is
**~6.0x**, which would move WM2000 from 15.5x slower than hardware to roughly
**2.6x**. That is the largest single lever identified in this document.

### Why this does not contradict the "incremental reconcile is impossible" proof

It does not attempt to. That proof is about *software* substitutes: a digest,
checksum or Merkle path must READ the region to be trustworthy, so it cannot
beat the read it replaces. `mprotect` is not a substitute computation — it is
the hardware MMU reporting writes as they happen, with no read at all. The
proof explicitly named it as one of the two escapes and this measures it.

### Obstacles to integration — assessed, not fought through

Reported rather than solved, per the brief. None is arithmetic-fatal; together
they are a substantial piece of engineering.

1. **RDRAM is a `Box<[u8]>`** (`crates/fn64-abi/src/host.rs:105`,
   `install_owned_process_rdram`) — malloc'd, so not page-aligned and not
   legally `mprotect`-able. It would have to become an `mmap`'d, page-aligned
   allocation, which touches the boot/publication path and the "RDRAM ownership
   moves into the runtime" contract.

2. **The watched region is unaligned** — `[0x400, 0x171a60)`, neither end on a
   16 KiB boundary, and the region is the whole boot bank, so it contains guest
   *data* as well as code. Whole-page protection therefore covers bytes outside
   the watched set. **This is already priced in**: the census counts distinct
   pages touched by *every* observed guest write, not only writes to code, so
   the 0.68 mean is the spurious-fault-inclusive number.

3. **Signal-handler safety.** The handler runs on the guest store path under
   `corosensei` coroutine stacks. It must be async-signal-safe: no allocation,
   no `RefCell`, no libc beyond the `mprotect` syscall. The benchmark handler
   meets that bar (two relaxed atomics and one `mprotect`) and took 325,500
   faults without incident, including with `SA_ONSTACK`.

   The open question was fault delivery onto a coroutine stack rather than a
   normal thread stack, since a handler that faults or deadlocks there is a hard
   crash with no diagnostic. **Settled directly, and it is clear:**
   `reference/mprotect-bench/coroutine-fault.rs` takes 2,000 write faults from
   inside a real `corosensei` coroutine across 20 suspend/resume cycles, with
   `SA_ONSTACK` set, asserting after each that the store actually landed once
   the handler re-armed the page. All 2,000 delivered; no lost stores, no
   corruption of the stack switch. This was the obstacle most likely to
   invalidate the approach outright, and it does not.

4. **Unattributed writers are the real correctness gap, and it cuts the safe
   way.** The census only sees writes routed through `set_write_observer`. The
   ledger already documents paths that bypass it (`Rdram::as_mut_slice()` into a
   C shim, `dma_write_bytes`, raw-pointer DMA). Those are *undercounted by the
   census* but would still **fault** under a barrier — the MMU does not care
   which Rust function issued the store. So the barrier detects a **superset**
   of what the census saw, and the projection is conservative on correctness
   while being mildly optimistic on cost (bulk DMA would fault once per page
   touched; a host writer can unprotect around a known bulk write instead).

5. **Equivalence bar.** A barrier reports *which pages* were written, not
   *which bytes changed*. A store that rewrites a byte with its existing value
   faults but changes nothing, so the barrier's dirty set is a superset of the
   scan's changed set. To stay a true substitute the changed-byte set must still
   be derived by comparing the faulted pages against the baseline — 16 KiB per
   dirty page instead of 1.44 MiB per boundary. That preserves the guard exactly
   and is where the 9.7x comes from; it is not a weakening.

6. Not yet checked: interaction with `FN64_WATCH_WRITE` and the debugger.

### A measurement trap worth recording

The first version of the probe printed its report from
`examples/wm2000-block-boot/src/main.rs`, and that **changed the canonical
program identity** -- `34712877...` became `57165dc1...`. Not a bug in the
probe: `build.rs:794` reads `src/main.rs` verbatim into
`DISPATCH_SOURCE_SHA256`, which feeds `ProgramArtifactIdentity` and therefore
the whole receipt chain. *Any* edit to that file, including a comment, changes
the identity of the program under measurement.

It was initially misdiagnosed as a stale `OUT_DIR` -- a forced `build.rs` rerun
reproduced the new digest, which looked like confirmation until a clean rebuild
from the same state returned the old one. The A/B, not the single observation,
is what settled it.

The fix was to print from `fn64-abi` via `atexit` instead, leaving the harness
source untouched. With that, the instrumented binary's complete run output is
`diff`-identical to the clean baseline -- program identity, `sim_time`, RDRAM
SHA-256 and every device counter -- with the census compiled in and disabled.
**Instrument the library, never `wm2000-block-boot/src/main.rs`.**

### Status

**Measured and favourable; not implemented.** The go/no-go number the design
hinged on — faults per boundary — is 0.68 against a break-even of 9, an ~13x
margin, so the lever exists. The probe is committed behind
`FN64_MPROTECT_CENSUS` so the number can be re-derived on any route.

Verification carried out: 568/568 tests across `fn64-abi` and `fn64-runtime`,
and complete run output `diff`-identical to the clean baseline with the probe
compiled in and disabled (2.90s both ways, `sim_time=13990253`,
`steps=19523`).

Reproduce the census (add `FN64_CONTROLLER_SCHEDULE` and a larger
`FN64_BLOCK_MAX_STEPS` for the deep route):

```
cd examples/wm2000-block-boot
source ../../.claude/local.env
export ROM="$FN64_DISCOVER_NWXE_ROM"
C=~/Code/aki-recomp/captures; G="$C/wm-general-exception-images"
export FN64_EXECUTABLE_IMAGES="$G/run-1/image.json:$G/run-2/image.json:$G/run-3/image.json"
export FN64_BOOT_CONTEXT="$C/wm2000-boot-context.json"
export FN64_ABSENT_N64DD=1 FN64_BLOCK_MAX_STEPS=1300000
FN64_MPROTECT_CENSUS=1 ./target/release/wm2000-block-boot
```

The standalone microbenchmark is retained at `reference/mprotect-bench/`
(`main.rs` = the headline costs, `stress.rs` = the faults-per-boundary sweep
that locates the break-even). It links nothing from this repository; build it
with the included `Cargo.toml.txt`.

### The next step, if this is taken up

The obstacle most likely to invalidate the approach outright, signal delivery
onto a `corosensei` coroutine stack, has been tested and cleared, so what
remains is invasive but not in doubt.

The expensive, structural piece is obstacle 1: moving RDRAM from a malloc'd
`Box<[u8]>` to a page-aligned `mmap`. Everything else depends on it, and it
touches the boot/publication path and the RDRAM-ownership contract. Obstacle 5
(deriving the changed-byte set by comparing only the faulted pages) is what
keeps the guard exactly as strong as it is today and should be designed
alongside it, not after.

Worth stating plainly: this is a multi-day structural change, not a patch. What
the measurement establishes is that it is worth attempting, roughly 6x to ~2.6x
of hardware, not that it is easy.

Recorded honestly: the brief that commissioned this expected "the fault costs
more than the scan" and eight hypotheses had died before it. This one did not.


## 2026-08-07: the entrance presentation does not advance -- a real boot blocker

A scheduled route ran **13h44m at 100% CPU** and never progressed past
controller **read 600**, holding at `step=653736 sim_time=1944932808
gfx_submits=704 audio_submits=1139` with three resident generations.

It is **not deadlocked**. Sampling the hung process shows active execution --
`run_one_step`, `advance_device_time`, `deliver_or_enqueue`, `osRecvMesg_recomp`
-- so the guest is running a loop that never satisfies its exit condition.

The route recipe anticipated this case. Its annotation at read 640 calls the
long idle tail *"the cheapest discriminator"*: if the entrance were a timed
cutscene it would advance with no input, and if it were an input gate the
scripted presses would clear it. **Neither happened**, so the entrance
presentation is waiting on something the route does not supply and time does not
deliver.

**The hang is byte-for-byte deterministic.** Two independent runs -- different
binaries, different budgets, hours apart -- both stopped at exactly
`read=600 step=653736 sim_time=1944932808`. That rules out a race, a timing
window, or host nondeterminism: the guest reaches the same state and makes the
same decision every time. Whatever it waits on is missing identically on each
run, which is the easiest kind of bug to chase and means any fix is verifiable
by a single reproduction.

Candidates, none yet tested:
- an RSP/RDP completion the presentation waits on (7 sp_tasks, all audio, and
  `rcp_completed=7` have been static since early boot)
- a VI retrace count the presentation blocks on
- a save/Controller Pak probe (`save_ops=0` for the whole run)
- a controller read pattern the schedule does not reproduce

This is now the top **playability** blocker, distinct from throughput. The
perf work took the route from hours to minutes per attempt, which makes this
tractable to investigate -- each hypothesis is now a minutes-long experiment
rather than an overnight one.


## 2026-08-07: the mprotect barrier's aliasing hazard, raised and closed

A parallel trace of every RDRAM writer surfaced a hazard the barrier's
feasibility study had not covered: `execution.rs:1158` (identically `:1576`,
`:1680`, and four sites in `runners.rs`) creates a `&mut [u8]` over the WHOLE
RDRAM allocation once inside the coroutine body and holds it across the entire
guest run -- every yield, every host shim, every device tick. A write barrier
would trap stores made through a reference that stays live across the fault.

The existing `reference/mprotect-bench/coroutine-fault.rs` did NOT cover this:
its faulting store is `ptr::write_volatile` on a raw pointer, so its 2,000-fault
result said nothing about a long-lived borrow.

`reference/mprotect-bench/borrowed-fault.rs` closes the gap by reproducing the
real shape: one whole-region `&mut [u8]` created at coroutine entry and live
across all faults and suspends; stores as safe bounds-checked indexing
(`mem[i] = v`, which is literally what `Rdram::store_b` compiles to) rather than
volatile; readback through the same borrow; an aliasing raw-pointer write to the
same bytes while that `&mut` is live; suspends taken with the barrier both armed
and disarmed; and an independent whole-region mirror compared byte-for-byte.

**Reproduced independently: 4,000/4,000 faults delivered, mirror
byte-identical.**

The correctness argument deliberately rests on the MMU property alone -- *a byte
of a `PROT_READ` page cannot change without a fault* -- and never on `&mut`
uniqueness. `mprotect` changes page permissions; it creates no reference and
invalidates no provenance, and the fault is transparent because the store
retires after the handler returns.

### Pre-existing aliasing debt, recorded not fixed

Two live `&mut [u8]` over the same allocation is aliasing-UB-adjacent under
Stacked/Tree Borrows **today**, independent of any barrier -- Miri would reject
it. The writers involved include `host_memory.rs`, `pi/timing.rs:444`,
`rsp_commit.rs:87`, `rsp_phase.rs:773` and `executor/mod.rs:734`. Addressing it
means refactoring `Rdram<'a>`'s ~40 accessors onto raw pointers, a separate and
much larger job.

### One structural fact that simplifies the barrier

Single OS thread, enforced by construction: `RunToken` is a ZST capability whose
sole producer is `Executor::run_one_step`, so two coroutines executing
concurrently "has no expressible call site" (`thread.rs:1-19`). `Executor`,
`HostState`, `ACTIVE_RDRAM`, `WRITE_OBSERVER` and the page-epoch tables are all
`thread_local!`; coroutine switches change the stack, not the thread. The only
non-test spawn near the guest is the watchdog, which never touches RDRAM.


## 2026-08-07: `guest_write_token` must NOT be wired into activation

I proposed caching activation digests against `guest_write_token`, on the
premise that it is page-epoch based and therefore observes RDRAM writes
independently of the declaration path. **That premise is false, and acting on
it would have silently deleted a fail-closed guard.**

`mark_guest_write_pages` (`fn64-recomp-rs/src/runtime/host.rs:353`) has exactly
two callers: `:466` inside `notify_attributed_guest_write` -- the common body of
every `notify_*` gateway -- and `:516` inside `notify_cpu_instruction_store16`.
There is no mprotect hook and no hardware dirty bit. **The page epoch is bumped
only when a writer voluntarily declares**, which is the identical trigger as the
write queue. The token re-encodes the queue; it does not observe memory.

Undeclared writers therefore leave the token unchanged while mutating an image:
`Rdram::as_mut_slice` (`runtime/host.rs:605`, self-documented as "an
UNATTRIBUTED write path", live caller `runners.rs:1331`, a C shim that writes
wherever the guest points it), the raw `copy_nonoverlapping`/`RdramPtr` writes at
`pi/timing.rs:1064`/`:1096`, `mesgqueue.rs:148`, and ~25 sites across
`pfs`/`gbpak`/`voice`/`si`. The attribution audit's "covered" verdicts do not
transfer: mechanism 2 declares bytes at a later flush boundary, which is too
late for a cache consulted at fetch.

The consequence would be: a C shim mutates a byte of a generation's image, no
epoch bumps, the cache reports "verified", and the guest executes stale
translated code with no digest and no error.

**The tree already says so, and I missed it.** `docs/plans/dispatch-granularity.md:570`
states the safety argument as: `guest_write_token` "has **no non-test
consumers**, so no activation path bypasses the digest." The zero-consumer
property is a cited premise of a written safety argument, not an oversight
awaiting a fix.

Also corrected: `generation/mod.rs` IS a certified source
(`fn64-recomp-rs/src/lib.rs:128`). The 27-entry
`DYNAMIC_MAPPED_EXECUTION_LIBRARY_SOURCES_V1` list is essentially all of that
crate's `src`, so there is no digest-neutral file in `fn64-recomp-rs`.

### The real fix targets the retirement loop, not the digest

The measured cost is structural rather than cryptographic. Generations
`17518568401266107605` `[0x8011c900,0x801226f0)` and `5227338575556428217`
`[0x8011c900,0x80161460)` **share a start address**, so every activation of one
retires the other and the next fetch re-hashes the full image -- 419,861 times,
66.8 GB.

Making activation not retire a generation whose image is byte-identical and
still resident removes the re-hash **while every activation still digests**. No
weakened verification, no soundness argument about writers required.

## 2026-08-07: the reference renderer profiled and optimized -- 12.3% faster

The reference renderer had never been profiled. A subsystem `sample` of a live
route put `fn64_render` second only to `fn64_abi` and about 3x the recompiled
guest code, so it was measured properly and optimized. Output is byte-identical.

### The benchmark this needed, because the standard one renders nothing

The documented A/B (`FN64_BLOCK_MAX_STEPS=40000000
FN64_BLOCK_MIN_GUEST_INSTRUCTIONS=1461877`, ~229 ms) reports
`gfx_submits=0 sp_tasks=0`. **It exercises zero rendering** and cannot measure
this subsystem at all; a renderer change of any size moves it not at all.

The isolated benchmark is `examples/xbus_replay` against a captured stream:

```
FN64_XBUS_STREAM_DUMP_DIR=/tmp/fn64-xbus FN64_XBUS_STREAM_DUMP_SKIP=20 \
FN64_XBUS_STREAM_DUMP_RDRAM=20 ./target/release/wm2000-block-boot   # capture
FN64_XBUS_REPLAY_REPEAT=60 ./target/release/examples/xbus_replay \
    /tmp/fn64-xbus/xbus-0020.bin /tmp/out /tmp/fn64-xbus/rdram-0020.bin
```

Real captured RDP commands over real captured RDRAM, no guest execution in the
sample. `FN64_XBUS_REPLAY_REPEAT` (added here) loops the replay and prints
`best_render_ms` plus an FNV-1a framebuffer digest, so a change's speed and its
byte-exactness are read off the same line. It asserts the digest is stable
across iterations, so a nondeterministic renderer fails the harness itself.

### Profile: texture sampling is half the renderer, the combiner is not

`sample` of 400 replays, render-attributable samples only:

```
texture sampling   48.6%     <- read_tlut alone (220) > the whole combiner
color combiner      8.5%
blender             5.5%
```

### What was wrong, and what it cost

1. **TLUT decode was recomputed per texel** (`6cbfa17`). A CI texel's color is
   a pure function of `(index, texture_lut)`, both immutable for a
   `TmemTexture`'s life, yet each fetch redid two masked byte reads, two
   validity asserts, and a format conversion -- four times per bilinear pixel.
   A lazily-filled 256-entry memo removed `read_tlut` from the profile
   entirely: render samples 1260 -> 1031 (**-18%**). Lazy, not eager, because
   `G_LOADTLUT` need not fill all 256 entries and an eager pass would trap on
   entries the primitive never samples.

2. **The per-texel TMEM accessors were out-of-line** (`59a9ce7`). `read_byte`
   runs up to four times per texel and sixteen per bilinear pixel; its body was
   dominated by panic formatting. Splitting the failure arm into a `#[cold]`
   helper and inlining the accessors: 8.60 -> 8.16 ms.

3. **Loop-invariant work in four pixel loops** (`7c8cea0`) -- `uses_texel1`
   rescanning eight combiner sources per pixel, per-pixel `TextureDerivatives`
   reconstruction, and a `getenv` per covered pixel in the triangle path.
   **Not measurable on this workload** (7.914 -> 7.865 ms, inside the noise);
   kept as an unconditionally correct hoist, recorded as unmeasured.

Interleaved A/B, six alternating rounds against `c6d9ecc` to cancel load drift:
**9.037 -> 7.926 ms, 12.3% faster**, identical digest every round.

### Do not re-try: precomputing the combiner's constant inputs

`evaluate_combiner` converts primitive, environment, k4/k5, key center/scale,
and prim_lod_fraction to float per pixel, all derived from primitive-constant
state. This looks like an obvious ~14-divisions-per-pixel win. **It is not.**
Replacing every one of those with a literal -- an upper bound no real
implementation can beat -- measured 8.73 ms against the honest 7.93 ms, i.e.
*slower*. LLVM already hoists them; the measured cost is the combiner's
data-dependent branching, not its arithmetic. Measure this ceiling before
building the plumbing, as was done here.

### Equivalence evidence

- 40 route frames dumped via `FN64_RENDER_DUMP_DIR` before and after:
  **all 40 byte-identical** by SHA-256.
- Both committed `reference/wm2000-frames/` PNGs reproduce exactly.
- Framebuffer digest `d0dbebc71bdf5264` unchanged across all three commits.
- 462/462 render-reference (including the exact-panic-text TMEM test),
  108/108 render + certification, 704/704 abi + runtime, `grade-all.sh`
  wrong=0 on all five.


## 2026-08-07: the `mprotect` write barrier — implemented, 4.5x on the deep route

The barrier is built, behind `FN64_MPROTECT_BARRIER=1`, together with the
page-aligned RDRAM it required. The guard is not weakened, every gate passes,
and this is the largest single lever the project has landed.

### Results

| route | scan lane | barrier lane | speedup |
|---|---|---|---|
| deep (`entrance-to-match`, 19,523 steps) | **2.98 s** | **0.66 s** | **4.5x** |
| pinned (1,461,877 guest instructions) | 220 ms | 140 ms | 1.57x whole-run |

Measured interleaved -- one scan run, one barrier run, alternating, same binary
-- rather than as two blocks. That is not fastidiousness: the first version of
this measurement was taken as two blocks while another agent's renderer
optimisations were landing in the shared tree between them, and the drift went
straight into the number.

The two routes disagree because the pinned lane is 44% fixed startup — ROM
load, shard resolution, catalog install — which the barrier does not touch.
The deep route is the honest measure of the mechanism: it runs 19,523 scheduler
steps against the pinned lane's 874, so the boundary work dominates.

Equivalence is `diff` of complete run output on BOTH routes — program identity
`34712877`, `sim_time=13990253`, RDRAM SHA-256, `steps=19523`, every device
counter. Only ASLR addresses and the process-unique thread id in the harness's
terminal backtrace differ. 704/704 abi+runtime, 401/401 recomp-rs, 1069/1069
discover, `grade-all` wrong=0 on all five.

### The number that decided it, measured rather than projected

`FN64_MPROTECT_BARRIER_STATS=1`, pinned route:

```
boundaries=3286 served=3285 (99.97%) fell_back=1 (0.03%)
clean=2622 (79.79%) mean_dirty_pages_per_served=0.2274
```

**80% of boundaries are now settled without reading a single byte** — the
barrier says no page in the watched region faulted, and that is the whole
answer. The census predicted 54.5% zero-page boundaries; the realised figure is
higher because it counts pages faulted inside one armed window rather than
every observed write across a route.

An intermediate version served only 77%, and the missing 23% was a single
missing arm: the "nothing to attribute and the bytes still match" early return
in `invalidate_pending_physical_writes_inner` exited without re-arming, leaving
the barrier down until some later boundary happened to arm it. With this design
the arming sites, not the comparison, are where the performance lives.

Note these counters are pinned-route only. On the deep route the harness ends
in a non-unwinding abort, so the `atexit` hook that prints them never runs.

### A fabricated result, and how it was caught

The first report of this work claimed 4.9x, and it was wrong. Two independent
mistakes compounded:

1. **The gate read an empty value as ON.** `requested()` was
   `var_os(..).is_some_and(|v| v != "0")`, and `FN64_MPROTECT_BARRIER=` -- set
   but empty, which is exactly how a shell writes the off lane in an inline
   `env` assignment -- satisfies `v != "0"`. Both lanes of the A/B were the
   barrier lane.
2. **The lanes were measured as two blocks, not interleaved**, while another
   agent's renderer optimisations were landing in the shared tree between them.
   So the "scan lane" number came from a slower binary for reasons unrelated to
   the barrier.

Together those produced a 3.57 s "scan" against a 0.73 s "barrier" that were
the same configuration measured 20 minutes apart. The tell was visible in the
profile and went unread: `__mprotect` and `_sigtramp` appeared in the lane that
was supposed to have no barrier.

Caught by the coordinator re-running the A/B independently and getting 704 ms
against 710 ms -- no speedup at all. The correct answer, gate fixed and lanes
interleaved, is 4.5x.

Two durable fixes: `env_flag` is now the single reader for all three of this
module's flags, and absent/empty/`0` all mean off, so no spelling of "off" can
mean "on"; `only_affirmative_env_values_enable_a_flag` pins it. And every
timing in this section is interleaved.

**The general lesson, which is not about this gate.** A performance claim needs
a check that fails loudly when the two lanes are secretly the same lane. Here
that check existed and was free -- the served/fell-back counters, and the
profile -- and simply was not consulted against the lane that was supposed to
be off.

### The two bugs, because they are the interesting part

Both weakened the guard, both were invisible on the pinned route, and both were
caught only by running the deep route.

**A real missed mutation.** `dirty_spans` began as a pure read, with the
capture left to the boundary's entry point. But
`reconcile_matched_before_dispatch` (`live_program.rs:2159`, `:2194`) and
`flush_host_abi_transaction` (`execution.rs:714`) reach the comparison without
passing through `invalidate_pending_physical_writes_inner`, so nothing had
captured for them. They read an empty leftover set and concluded "no page was
written, therefore nothing changed" while the fault handler held the pages that
said otherwise:

```
unjournaled executable mutation changed physical RDRAM
[0x00086090, 0x00086094) before canonical static dispatch
```

The guard caught the barrier, which is the right way round — but a barrier the
guard has to catch is not a substitute for the guard.

**Stale pages read as changes.** Returning from a boundary without re-arming
left the recorded set accumulating into the next boundary, which reported
~2,000 spurious changed ranges and tripped the attribution assertion.

### The design property that came out of them

Both bugs are the same shape: a call site that had to remember something. The
fix was not to add the missing call sites — there are many boundary paths and
no reliable way to enumerate them by inspection — but to make forgetting safe:

- **The read and the close are one operation.** `dirty_spans` disarms itself,
  so a caller cannot obtain a set predating its own question, and a new
  comparison site inherits the property without knowing it exists.
- **The set is consuming.** A missed arm yields `None` and a full scan, rather
  than a set describing an older window whose complement is wrongly treated as
  proven-unchanged.

The invariant, stated once: **missing an arm costs a scan; missing a disarm
would be unsound, so no caller is trusted to disarm.** That asymmetry is what
makes the integration verifiable by inspection of two functions rather than of
every boundary path — and the 77% → 99.97% fix shows the cost of a missed arm
is exactly what the invariant promises, a scan and nothing worse.

### The guard is not weakened

The dirty page set is a **superset** of the changed byte set, and the
byte-level comparison still runs — over the dirty pages instead of the whole
region. The guard's answer is unchanged; only the bytes read to reach it fall.

- The barrier arms only at a boundary that has just PROVEN the region equals
  the baseline (one caller, `arm_barrier_over_clean_region`).
- A byte of an `mprotect(PROT_READ)` page cannot change without a write fault.
  That is the MMU's guarantee, not this code's.
- So every byte the scan could find changed lies in some page the handler
  recorded.

Strict superset — a store rewriting a byte with its own value faults and
changes nothing, and a fault marks a whole 16 KiB page — and both errors are in
the safe direction.

This also covers the writers no declaration path sees. `as_mut_slice`, the DMA
paths, the RSP and renderer whole-allocation slices, and raw `RdramPtr` stores
all bypass `set_write_observer`, and every one of them faults: the MMU does not
care which Rust function issued the store. The barrier does not consult the
declaration path at all, which is why it detects a superset of what the census
could see.

`barrier_restricted_changed_ranges_match_the_full_scan` asserts the restricted
comparison names exactly the bytes the full scan names, over randomized
contents and change patterns, at every alignment, for three dirty-set shapes
(tight, page-widened, whole-region). It found a real bug on its first run: two
spans landing in the same storage word each widened to cover it, so the word
was visited twice and its differing bytes emitted twice. Fixed by widening and
merging in one pass (`word_align_spans`).

### The aliasing question, settled separately

The real coroutine body holds one `&mut [u8]` over the whole allocation across
the entire guest run (`execution.rs:1680-1690`). The earlier
`coroutine-fault.rs` did **not** cover that shape — its faulting store was
`ptr::write_volatile` on a raw pointer. `borrowed-fault.rs` does: whole-region
`&mut` live across 4,000 faults and 70 suspend/resume cycles, stores as safe
bounds-checked indexing, non-volatile readback through the same borrow, an
aliasing raw write while the `&mut` is live, and an independent mirror compared
byte-for-byte. All delivered, mirror identical, release and debug.

That rules out the practical failure mode — the optimizer hoisting or caching
around a store it believes cannot trap. It does not make the program
Stacked-Borrows-clean: two live `&mut [u8]` over this allocation already exist
independent of any barrier, and Miri would reject that today. The correctness
argument above therefore never uses `&mut` uniqueness, only the MMU property.
`mprotect` creates no reference and invalidates no provenance.

### Fallbacks

Every case the barrier cannot cover runs the scan that exists today: not
requested, allocation not page-aligned, `mprotect` refused, dirty set overflow
past 512 pages, a fault outside the region, poisoned mutation state, process
exit. There is no configuration in which a boundary is decided by neither.

### Where the remaining cost is

With the barrier on, the deep route is 0.73 s. `memcmp` is no longer the
profile's centre of mass; `__mprotect` and `_sigtramp` are now visible, which
is the direct confirmation that faults are being taken and pages re-armed.

The next candidates, in descending expected value:

1. **Re-arm per page rather than whole-region.** `arm` `mprotect`s the entire
   1.44 MiB span every boundary; the microbenchmark put whole-region protect at
   2,253 ns against 597 ns for a single page. At 0.23 dirty pages per served
   boundary, re-protecting only what faulted recovers most of that gap, and it
   is the single largest remaining item.
2. **SHA-256.** Already 38 leaf samples with the barrier on and rising as a
   share; the separate finding that the entrance hashes 66.8 GB is the same
   cost seen from the other side.
3. Codegen remains irrelevant until guest code is visible in a profile at all.

## 2026-08-07: the post-barrier profile, corrected — I read inclusive as self

I dispatched three optimization targets off a profile that was **cumulative
(inclusive) time, not self time**. In `sample` output a frame's count includes
everything it called. All three targets were artifacts:

- `live_program::_` (42) is not a symbol. It is the demangled prefix of
  `_$LT$impl…CanonicalLiveBlockProgramV1$GT$`, an inclusive total across
  `invalidate_pending_physical_writes_inner`, `reconcile_before_dispatch`,
  `flush_host_abi_transaction` and `arm_barrier_over_clean_region` — mostly the
  barrier calls beneath it.
- `verify_precompiled_instruction_word` (15) never appears in a self-time
  profile at all. **All** generated shard code sums to 8 of 214 self samples.
- The three address-translation functions (24) are ~6 self samples together.

This is the exact error the project's own history warns about, and it cost an
agent a full investigation cycle to unwind.

**Corrected self time** (two independent samples, 12 s and 6 s, in agreement):

```
109  __mprotect                 50.9%
 34  sha2::sha256::compress
 23  _sigtramp                  (fault delivery)
 14  changed_ranges_from_view
  6  _platform_memmove
  4  try_store_w_translated
```

### The barrier is now the bottleneck it created

~316 ms of the 620 ms deep route is the `mprotect` syscall itself: ~182,892
calls (arm + disarm across 91,446 boundaries) at ~1.7 us. With `_sigtramp` and
the SHA-256 re-rooting, **the barrier's own machinery is ~78% of remaining
runtime.** It traded a 1.5 MB `memcmp` for two syscalls per boundary — a 4.5x
win that is now the thing to attack.

The lever is **syscall volume**, not per-instruction work:
- `clean=68615` — **75% of boundaries have zero dirty pages** and still pay arm
  + disarm.
- Staying armed across a clean boundary skips both syscalls, but correctness
  rests on `dirty_spans` being consuming; see the stale-window bug documented at
  `write_barrier.rs:888`.
- `mean_dirty_pages_per_served=0.2532` — narrower protected spans or batched
  re-arm cut syscalls directly.

### Target 1 is dead three ways, not one

`verify_precompiled_instruction_word` cannot be gated on the barrier even if it
were worth it: `fn64-recomp-rs` does not depend on `fn64-abi` (the dependency
runs the other way), so the verify cannot ask whether the barrier is armed.
`verify_live_words: true` is baked into the shards at generation
(`examples/wm2000-block-shards/build.rs:270`), so making it conditional would
mean a runtime flag test per instruction. And the sets differ: the barrier's
span is the page-widened executable ranges bound at seal, while the verify
covers whatever PC executes, and the barrier legitimately degrades to `Unknown`.
