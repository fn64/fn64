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
