# A resolvable self-time profile, and where the 430 ms actually goes

Supersedes the profile in `perf-review-and-profile.md` Part 2. That one was
taken with `sample` and concluded **"sha2 (3.32%) — not worth doing"**. Measured
properly, SHA-256 is **26% of steady state** and the single largest cost in the
run. The old number was not a small error; it pointed the last several waves at
the wrong target.

## Why `sample` could not answer this, and what can

`sample` has three properties that make it unusable here:

1. it symbolicates with its own logic and emits no raw addresses, so the
   inlined dispatch loop collapses onto `run_one_step`;
2. its 1 ms floor yields ~420 samples for a 420 ms run;
3. its leading integer is *inclusive*, and reading it as self time is the
   documented cause of three failed optimizations in this repo.

`xctrace` with the **CPU Profiler** template solves all three. It samples on the
`CORE_ACTIVE_CYCLE` PMU counter (every 1M cycles, ~1,500 samples/run) and its
`cpu-profile` table exports, **per row**, a full backtrace with absolute frame
addresses, a real cycle `weight`, and — critically — a `<binary load-addr=...>`
attribute giving the exact ASLR load address.

```
xctrace record --template "CPU Profiler" --output /tmp/wm.trace \
  --target-stdout /dev/null --launch -- ./target/release/wm2000-block-boot

xctrace export --input /tmp/wm.trace \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="cpu-profile"]' \
  --output /tmp/wm-cpu.xml
```

`dsymutil` produces the dSYM in **0.23 s / 18 MB** (the prior agent's report is
accurate), and `atos -o <dSYM> -l 0x100000000` then recovers the full inlined
frame chain with `file:line`. 157,848 `DW_TAG_inlined_subroutine` DIEs are
present; the existing `debug = 1` settings are sufficient and no
`-C force-frame-pointers=yes` was needed.

### Two traps that produce confident, wrong profiles

Both of these bit me before the numbers below stabilized, and both are silent.

**Do not use the `kdebug-counters-with-pmi-sample` table's stacks.** Its
`text-addresses` are *shared fragments* — 312 distinct fragments backing 1,506
rows — referenced by many rows at once. Attributing them per row manufactures
identical fake call chains; it produced a caller list where the top four entries
all had exactly 1,065 samples and showed `run_00` calling a btree insert calling
`draw_texture_rectangle`. Only that table's `text-address` (the leaf PC) is
per-row. The `cpu-profile` table's backtraces *are* per-row and are what this
profile uses.

**Do not infer the ASLR slide.** I tried three statistical fits (cluster
minimum, `__TEXT`-bounds containment, symbol-start proximity). All three
disagreed with each other and with ground truth. The symbol-proximity fit scored
88% and was still off by 0x3c000, which yielded a top-15 list led by
`PathBuf::__set_extension` (10.9%) and float formatting `dragon.rs` (~25%) —
in a loop that formats nothing. Bounds-checking against `nm` output is also
useless here: `nm`'s highest symbol is `0x104c972a4` but `__TEXT` really ends at
`0x1052fc000`, because the generated shards are overwhelmingly unnamed.
The `load-addr` attribute in the export is ground truth; use it.

Runs are individually slid, so each trace must be converted with its own
`load-addr` before merging.

## The measurement

5 runs, 7,532 samples, 7.44 G cycles, deep route
(`FN64_ABSENT_N64DD=1 FN64_BLOCK_MAX_STEPS=19523 FN64_MPROTECT_BARRIER=1`),
baseline 420 ms.

Boot/one-time setup (`parse_boot_context`, ROM load, bootstrap import) is
**15.5%** of the run and is excluded below; the remaining **84.5%** is the
steady-state loop.

### Top self time, steady state

| share | frame |
|------:|-------|
| 26.14% | `sha2::sha256::aarch64::compress` |
| 11.07% | `fn64_runtime::rdram::RdramView::read_u8` — `rdram.rs:338` |
| 11.02% | `fn64_abi::with_executor` — `lib.rs:1811` |
| 8.03% | `CanonicalExecutableMutationStateV1::changed_ranges_from_view` — `live_program.rs:432` |
| 6.26% | `_platform_memmove` |
| 4.11% | `_platform_memcmp` |
| 2.08% | `…::read_snapshot` — `live_program.rs:455` |
| 1.32% | `Rdram::backing_offset` — `host.rs:667` |
| 1.19% | `snapshots::record_executable_and_renderer_write` |
| 1.14% | `_tlv_get_addr` |
| 1.06% | `Rdram::try_store_w_translated` — `host.rs:1461` |
| 1.00% | `verify_precompiled_instruction_word` |
| 0.86% | `pi::mmio::write_live_device_mmio` |
| 0.83% | `snapshots::classify_live_executable_write` |
| 0.81% | `WatchedExecutableBytesV1::set_expected` — `mod.rs:608` |
| 0.81% | `RecompContext::advance_cop0_random` |

### The category split that decides strategy

Inclusive, as a share of steady state:

| share | category |
|------:|----------|
| **30.82%** | `commit_from_view` — per-boundary digest + diff |
| **24.15%** | `changed_ranges_from_view` / `read_snapshot` — per-boundary scan |
| 12.50% | device timing advance |
| 8.48% | `translate` / `backing_offset` / `try_store` — **per instruction** |
| 7.23% | `adopt_changed_from_view` |
| 2.96% | `record_executable_and_renderer_write` — per store |
| 2.01% | `verify_precompiled_instruction_word` — per instruction |
| 0.81% | `advance_cop0_random` — per instruction |
| **2.86%** | **guest recompiled code (leaf)** |

**This is per-boundary work, not per-instruction work.** Commit and scan
together are ~55% of steady state; everything charged per guest instruction is
~11%. The lever is *fewer boundaries*, or a cheaper boundary — not codegen.

### Guest code is still 2.86%

The coordinator's arithmetic holds, and if anything understates the case. The
recompiled MIPS is a rounding error in this profile; the remaining ~97% is
apparatus. Runtime optimization is emphatically **not** finished.

## Where the SHA-256 goes

Breaking the 26.22% of steady state spent in `sha2` down by purpose:

| share of steady | site |
|----------------:|------|
| **20.23%** | `digest_expected` |
| 6.80% | `watched_page_digest_v2` |
| 1.00% | `canonical_mutation_entry_root` |
| 0.99% | `set_expected` |

`digest_expected` (`live_program.rs:511`) is the *incremental* root. Its comment
is accurate that it hashes 32 bytes per page rather than every watched byte —
but the watched region is the 1 MiB boot bank and
`CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2` is 4096 (`recompiled/mod.rs:481`), so it
is **256 page digests = 8 KiB of SHA-256 on every commit that changed anything**,
regardless of whether one byte changed or a thousand. The root is recomputed
from scratch each time because `watched_root_digest_v2` is a flat hash over all
page digests in order.

## The boundary count, and the 62% that change nothing

The existing census (`FN64_MPROTECT_CENSUS=1`, already in-tree at
`snapshots.rs:967`) on the deep route:

```
boundaries=49910  distinct_pages_total=26376  mean_pages_per_boundary=0.5285
     0 page(s):  31088 boundaries (62.29%)
     1 page(s):  11472 boundaries (22.99%)
     2 page(s):   7256 boundaries (14.54%)
    3+ page(s):     94 boundaries ( 0.19%)
```

**49,910 boundaries in a 420 ms run**, and **62% of them touch zero pages.**

`commit_changed` (`live_program.rs:961`) already returns early when
`declarations.is_empty() && changed.is_empty()`. The expensive case is the one
where a writer *declares* a store whose bytes are unchanged: `changed` is empty,
so `after_sha256` correctly short-circuits to `before_sha256` at `:987` — but
the boundary still pays the full `changed_ranges_from_view` scan to *discover*
that nothing changed, and still allocates and appends a journal entry with a
`canonical_mutation_entry_root` hash.

## What this implies (not yet attempted)

Ranked by measured size, all per-boundary:

1. **Make the root digest incremental over pages.** 20% of steady state is
   re-hashing 256 unchanged page digests to fold in one changed page. A tree
   (rather than a flat hash) over page digests would make the update
   `O(log pages)`. This changes the digest value, so it is a v3 schema and a
   certified-identity change — deliberate, not incidental.
2. **Fewer boundaries.** 49,910 for 19,523 steps is ~2.6 boundaries per step.
   The resident-generation predicate reportedly reaches 519-block slices when
   nothing forces a break; the census histogram says 62% of boundaries had
   nothing to report at all.
3. **Skip the journal entry when nothing changed.** ~1% in
   `canonical_mutation_entry_root` plus the allocation traffic behind
   `_platform_memmove` (6.26%) and `_xzm_free`.

`verify_precompiled_instruction_word` measures 1.00% here, consistent with the
prior finding that it is not worth pursuing — and it is unfixable at that layer
regardless.

## Reproducing

Environment as `perf-review-and-profile.md` "Reproducing", plus
`FN64_BLOCK_MAX_STEPS=19523 FN64_MPROTECT_BARRIER=1`. Then:

```
dsymutil target/release/wm2000-block-boot -o /tmp/wm2000bb.dSYM   # 0.23s
xctrace record --template "CPU Profiler" --output /tmp/wm.trace \
  --target-stdout /dev/null --launch -- ./target/release/wm2000-block-boot
xctrace export --input /tmp/wm.trace \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="cpu-profile"]' \
  --output /tmp/wm-cpu.xml
```

Then per row: read `<binary load-addr>` for the main image, subtract
`load-addr - 0x100000000` from every main-image frame `addr`, and resolve with
`atos -o /tmp/wm2000bb.dSYM -l 0x100000000`. Self time is the **leaf frame
only**, weighted by the row's `cycle-weight`.

## The 440 ms vs 775 ms discrepancy: machine load, not drift

A v3-migration agent flagged ~335 ms unexplained between its runs (~775 ms) and
the profile this document was taken from (~440 ms) — same route, same env, same
build config. It was right to stop and flag it rather than optimize past it:
that gap is six times the win it had just measured.

**Cause: fifteen concurrent `rustc` processes at ~90% CPU each**, load average
16.16. A second agent was rebuilding the 32 shard crates — an ~11 minute job
triggered by any edit to `crates/fn64-recomp-rs`. The benchmark was competing
with a full shard rebuild.

Reproduced: the same binary and invocation that measured 430 ms on an idle
machine measures 732-749 ms under that load. Nothing regressed.

### The rule this needs

**Absolute timings are only comparable on a quiet machine.** Before quoting one,
check `uptime` and count heavy processes; if load exceeds ~2, the number
describes contention rather than the code.

Interleaved A/B pairs survive this — the v3 agent's paired design measured a
trustworthy 54.9 ms delta *through* the contention, which is exactly what
pairing is for. But the **ratio** to hardware does not survive, and that agent
correctly declined to quote one.

Practical consequence: **do not run a `fn64-recomp-rs`-editing agent concurrently
with a benchmarking agent.** `fn64-abi` edits rebuild only the harness (~25 s);
`fn64-recomp-rs` edits rebuild 32 crates and saturate the machine.
