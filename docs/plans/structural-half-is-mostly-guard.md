# The "structural half" is mostly guard, and three of its four targets are misattributed

Written 2026-08-07. A wave was dispatched to attack the ~50% of WM2000 runtime
that `perf-method.md` classified as **structural** — "being an N64" — on the
premise that the guard had been optimized as far as it goes and the remaining
cost was irreducible emulation work.

**Three of the four named structural targets are not structural.** Two are the
correctness guard wearing a different symbol name, one was already fixed and its
cost is now zero, and the fourth is real but small. This does not mean the
guard is cheap; it means the *categorization* that made the structural half look
like a separate 50% was wrong, and work aimed at it would have been aimed at
nothing.

No code changed. The finding is the deliverable, plus one new tool.

## The categorization that was wrong

`perf-method.md` carried this split, which the wave brief inherited:

| share | component | claimed class |
|---|---|---|
| 12.5% | device fabric — PI/SI/VI/AI | **structural** |
| 11% | `RdramView::read_u8` | **structural** |
| 11% | `with_executor` dispatch | **structural** |
| 11% | per-instruction address translation | **structural** |

Every one of those rows came from **self time alone**. Self time says what is
executing; it does not say *why it was called*, and for these four frames the
"why" is the whole answer. `recomps/wm2000/scripts/wm2000_callers.py` (added by this wave)
resolves the frame above the leaf and settles it.

## Target by target

### `RdramView::read_u8` — 12.62% self, and 92.7% of it is the guard

Fresh 3-run profile, 3.82 G weighted samples, quiet machine:

```
Callers of 'RdramView::read_u8'   self: 481,622,754 (12.62%)
  82.76%  ...CanonicalExecutableMutationStateV1::read_snapshot
   9.97%  receipts::watched_bytes_sha256
   3.95%  receipts::BootstrapImportTransactionV1::commit
```

`read_snapshot` is the mutation journal's baseline snapshot. `watched_bytes_sha256`
and `BootstrapImportTransactionV1::commit` are bootstrap validation. **Not one
significant caller is a guest load.** The brief described this frame as "guest
memory reads through a lane-XOR-3 swizzle; every guest load pays it" — that is
not what the profile says. Guest loads go through
`fn64_recomp_rs::runtime::host::Rdram`, which appears separately and far lower
(`backing_offset` 1.39%, `try_store_w_translated` 0.97%).

Optimizing the swizzle would therefore have optimized the guard, been measured
against the guard, and been filed under "structural." The 11% was already
counted in the 34% journal/digest row — **the two halves of the published split
overlap.**

Note also `watched_bytes_sha256` (`receipts.rs:1263`) hashes **one byte at a
time** (`hasher.update([view.read_u8(..)])`) over the whole watched range — the
exact anti-pattern `copy_logical_bytes` was written to remove. It is worth
fixing on cleanliness grounds, but all three of its call sites are
bootstrap/validation, so it lands in the ~15% one-time boot segment and not in
steady state. Do not expect a steady-state win from it.

### `with_executor` — 11.10% self, and it is called once per step

```
Callers of 'with_executor'   inclusive: 71.59%   self: 11.10%
  +1  100.00%  fn64_abi::host::run_one_step
  +2  100.00%  wm2000_block_boot::main
```

The brief described this as "scheduler dispatch, a `thread_local` + `RefCell`
borrow on every access… called extremely often. Is the borrow avoidable on hot
paths?"

263 `with_executor` call sites exist in `fn64-abi`. **One of them accounts for
100.00% of on-stack samples**, and it is `run_one_step` (`host.rs:334`), which
calls it exactly **once per scheduling step** — 19,523 times in the route. The
other 262 shim call sites are statistically invisible.

The 11.10% self time is not borrow overhead. `run_one_step`'s `with_executor`
closure *contains `exec.run_one_step()`* — the entire guest execution, including
the coroutine resume. The self time is the context-switch trampoline into the
guest stack, which carries no line info and so lands on the enclosing frame.
The inclusive figure makes this unambiguous: **71.59% inclusive against 11.10%
self** is a frame that spends its time in its callee.

Eliminating the `RefCell` borrow here would remove one borrow per 16.75 µs step.
It is not a lever. This is rule 2 of `perf-method.md` (self time = count minus
immediate children) reappearing in a new costume: the row was read as if the
11% were the borrow, when the borrow is a rounding error inside it.

### Device fabric — measured at exactly zero

`FN64_DEVICE_ADVANCE_CENSUS=1` on the deep route:

```
[device-census] no samples
```

Not "small" — **zero**. Every `advance_device_time` call exits at the
`advance_clock_if_idle` fast path (`pi/timing.rs:256`), which a prior wave added
precisely because the slow path was 38% of the lane. The census that would count
slow-path entries records none. The 12.5% row is stale and should be struck.

### Per-instruction translation — real, and small

`backing_offset` 1.39% + `try_store_w_translated` 0.97% +
`verify_precompiled_instruction_word` 0.95% + `advance_cop0_random` 0.71%
≈ **4.0%**, not 11%. Already `#[inline]` with a two-compare hot path
(`may_be_mmio`). This is the only one of the four that is genuinely
per-instruction and genuinely structural, and it is a quarter of its billing.

## What the profile actually says

3 runs, 3,817,695,861 weighted samples, quiet machine, deep route
(`FN64_ABSENT_N64DD=1 FN64_BLOCK_MAX_STEPS=19523 FN64_MPROTECT_BARRIER=1`):

| SELF% | symbol |
|------:|--------|
| 22.66% | `sha2::sha256::aarch64::compress` |
| 12.62% | `RdramView::read_u8` — 92.7% of it guard callers |
| 11.10% | `with_executor` — resume trampoline, 1 call/step |
| 8.34% | `changed_ranges_from_view` |
| 3.58% | `_platform_memcmp` |
| 3.38% | `_platform_memmove` |
| 2.83% | `read_snapshot` |
| 1.39% | `Rdram::backing_offset` |
| 1.32% | `watched_bytes_sha256` (bootstrap) |
| 0.76% | `wm2000_block_shard_03::runner_02::run_02` (guest code) |

Re-classified by *cause* rather than by symbol, steady state is still
overwhelmingly the correctness guard. **The structural half, as a distinct 50%,
does not exist.** What exists is roughly 4% per-instruction translation, a few
percent of genuine device/scheduler work, and a guard that is larger than the
34%+8%+5% the old split credited it with, because part of it was filed under
`read_u8`.

## Why this matters for the 60fps goal

The hardened goal is a worst-case frame-latency bound. Two measurements bear on
it directly.

### The guard's per-boundary work is already bounded and small

`FN64_MPROTECT_BARRIER_STATS=1 FN64_MPROTECT_BARRIER_SYSCALLS=1`:

```
[mprotect-barrier] boundaries=91446 served=91445 (100.00%) fell_back=1 (0.00%)
                   clean=68615 (75.03%) mean_dirty_pages_per_served=0.2532
[mprotect-syscalls] total=46494 (27.7ms) reprotect=23246 (17.5ms) fault=23246 (10.2ms)
```

The spiky failure mode to fear was the full-region fallback: `matches_storage`
(`recompiled/mod.rs:1114`) compares the entire 1 MiB watched region when the
barrier cannot answer. **It happens once in 91,446 boundaries.** The barrier
serves 100.00% of them, at a mean of 0.25 dirty pages each. The digest does
*not* scale with watched bytes per boundary in practice; the brief's concern
that "the digest scales with watched bytes, not with what changed" is the right
shape of worry but the barrier already closed it.

The residual unbounded item is the **one** fallback, plus the 23,246
reprotect/fault syscall pairs — amortized at 754 ns and 437 ns, spread evenly,
not bunched per frame.

### The route does not contain a frame in the 60fps sense

This is the more important caveat, and it invalidates any p50/p99/max frame-time
number taken from this route.

```
sim_time=13990253   vi_interrupts=8   gfx_submits=0   audio_submits=7
executor_ms=327.057 calls=19523  (16.75 µs/step)
```

- **`gfx_submits=0`.** The route renders nothing. There is no frame pipeline to
  measure the latency of.
- **8 VI interrupts** over 298 ms of guest virtual time = a 37.3 ms field
  period. The *guest* is producing fields at ~27 Hz during boot, not 60 Hz.
- Wall 327 ms / 8 fields = 40.88 ms per field = **2.45x the 16.67 ms budget**,
  which is where the "2.4x" reconciles. But wall/virtual is only **1.096x**.

Those two ratios differ because the denominator differs: 2.45x measures against
a 60 Hz *target*, 1.096x measures against what the guest actually asked for.
Quoting either without the other misleads. Neither is a frame-time distribution,
because this route has no frames.

**A p99 frame-time bound cannot be measured on `wm2000-block-boot`.** It needs
a route that reaches sustained rendering — `gfx_submits > 0` over many fields.
Until such a route exists, "guaranteed 60fps" has no test, and any frame-latency
claim about fn64 is unfalsifiable. That is the single highest-value gap for the
hardened goal, and it is a harness gap, not an optimization gap.

## Tooling added

`recomps/wm2000/scripts/wm2000_callers.py` — caller attribution for an `xctrace` cpu-profile
export. Same two hard-won rules as `wm2000_self_time.py` (leaf is the innermost
frame; each run is slid with its own `load-addr`), plus a `--selftest` that
pins the caller rule on a synthetic export. `--leaves` reproduces the self-time
histogram as a cross-check; `--symbol X --depth N` walks outward from the
innermost occurrence of `X`.

Every claim in this document that begins "the caller is" came from it, and none
of them could have been read off a self-time table.

## For `perf-method.md`'s dead-ends list

- **`with_executor`'s `RefCell` borrow.** Not a lever. One call per scheduling
  step from `run_one_step`; the 11% self time is the coroutine resume inside its
  closure (71.59% inclusive), not the borrow.
- **`RdramView::read_u8`'s swizzle, as a guest-load cost.** Not a guest-load
  cost. 92.7% of its samples come from the mutation journal and bootstrap
  validation. Optimize it as guard work or not at all.
- **The device fabric.** Already zero — `FN64_DEVICE_ADVANCE_CENSUS` reports no
  samples on the deep route.
