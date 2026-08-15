# The mutation-journal performance wave: review and profile

Adversarial review of the seven correctness-sensitive changes in
`git diff 61a6adf..HEAD`, plus the first working self-time profile of
`recomps/wm2000/packages/wm2000-block-boot`.

Reviewed 2026-08-07 on Apple Silicon, branch `fix/overlay-stride-aliases`.
Each of the seven was written and verified by a different agent in isolation;
none had been reviewed against the others. They all touch the same commit path.

The seven:

| commit | change |
|---|---|
| `2d1741e` | define the v2 page-tree watched-bytes digest |
| `ce7bd9e` | read the incremental root on the commit path |
| `4ac07b0` | compare watched bytes in place on the commit path |
| `a8933a6` | break blocks only on writes backing a resident generation |
| `8a1f3ad` | reach HOST without panicking from the guest store path |
| `6e6ab3c` | take the view on the device-time and host-memory commits |
| `0dcb065` | compare against the view at the HostAbi boundaries too |

## Verdict

**These changes are sound.** No silently weakened detection was found. Every
"we can answer this cheaply" shortcut falls back conservatively, the byte-lane
swizzle is correct at every offset, the three baseline mirrors cannot drift,
and the v2 root is a pure function of the watched bytes.

Two defects were found. Neither is a live correctness bug, both are latent:
one permissive fallback that diverges from the function it mirrors (S1), and
one unsealed-state fallback that would assert rather than degrade (S3). Both
are cheap to fix.

The profile, however, contradicts the premise that motivated the wave's last
stage. See [Part 2](#part-2--the-profile).

---

## Part 1 — adversarial review

### S1 (medium, latent). `is_ok_and` makes a missing backing mean "not resident"

`crates/fn64-abi/src/recompiled/live_program.rs:2566-2579`

```rust
backings
    .binary_search_by_key(&generation, |backing| backing.generation())
    .is_ok_and(|index| { /* span intersection */ })
```

The doc comment states this "mirrors `invalidate_physical_write`
(`fn64-recomp-rs` `generation/mod.rs:1292`)". It does not mirror it in the
one case that matters. That function treats a missing backing as an invariant
violation:

`crates/fn64-recomp-rs/src/generation/mod.rs:1307-1310`
```rust
let backing = &self.backings[self
    .backings
    .binary_search_by_key(&generation.id, |backing| backing.generation)
    .expect("active generation has no validated physical backing")];
```

`is_ok_and` returns `false` on `Err` — "no resident generation is backed by
these bytes" — which is the **permissive** direction, and the only permissive
default in the whole guard path. Every other unanswerable case in this wave
resolves to "assume resident" (see the audit below). If an active generation
ever lacked a backing, this would let a write to live executable bytes chain
the block instead of breaking it: stale translated code executes.

It is latent, not live. `BackedPrecompiledGenerationCatalogV1::new`
(`generation/mod.rs:936-945`) rejects any backing whose generation is unknown,
and every active segment's generation is registered, so the search cannot
currently fail. But the invariant is enforced in a *different crate* from the
new caller, and the mirrored function defends it with `expect` precisely
because it is load-bearing.

**Fix**: match the mirrored function — `.map_or(true, |index| …)`, or an
`expect` with the same message. The conservative default costs nothing:
this branch is unreachable under the current invariant.

### S2 (medium). The HostAbi boundary scans the whole watched region twice

`crates/fn64-abi/src/recompiled/live_program.rs:2218` and `:2231`

```rust
let changed = match view.and_then(|view| state.borrow().changed_ranges_from_view(view)) {
    …
};                                                          // scan #1
let first_new_entry = state.borrow().entries.len();
for (physical_start, physical_end) in changed { … notify … }
match view {
    Some(view) => {
        self.invalidate_pending_physical_writes_from_view(view, &mut read_physical_byte);
    }                                                       // scan #2, via matches_view
```

`changed_ranges_from_view` scans the full 1 MiB region to build the notify
list. `invalidate_pending_physical_writes_from_view` then immediately scans it
again — first through `matches_view` (`:2427`), and if that fails, a third time
through `changed_ranges_from_view` (`:2449`).

This is not a correctness problem; it is the single largest remaining cost in
the profile. Measured attribution of `memcmp` self time by call path:

| self% | samples | path |
|---|---|---|
| 13.10% | 355 | `reconcile_before_dispatch > matches_view` |
| 4.46% | 121 | `write_guest_physical > invalidate_… > changed_ranges_from_view` |
| 3.73% | 101 | `flush_host_abi_transaction > invalidate_… > matches_view` |
| 3.62% | 98 | `checkpoint_…_before_suspend > invalidate_… > matches_view` |
| 3.58% | 97 | `checkpoint_…_before_suspend > changed_ranges_from_view` |
| 3.47% | 94 | `flush_host_abi_transaction > changed_ranges_from_view` |

The last four rows are the same two entry points each paying for two full
scans. `flush_host_abi_transaction`: 3.47% + 3.73%. `checkpoint_before_suspend`:
3.58% + 3.62%. Roughly **7.1% of total runtime** is the second scan of a
region the caller has already scanned and found unchanged.

### S3 (low, latent). The unsealed fallback asserts rather than degrading

`crates/fn64-abi/src/recompiled/execution.rs:714-721`

```rust
let changed = state.borrow().changed_ranges_from_view(&view);
let changed = match changed {
    Some(changed) => changed,
    None => {
        let snapshot = state.borrow().read_snapshot_from_view(&view);
        state.borrow().current_changed_ranges(&snapshot)
    }
};
```

`changed_ranges_from_view` returns `None` for **two** distinct reasons
(`live_program.rs:376-386`): an unmapped watched byte, *and* an unsealed state.
The comment names only the first ("`None` means an unmapped watched byte; the
copying path below then raises the panic that owes").

On the unsealed path the fallback does not raise the panic that owes. It calls
`current_changed_ranges`, whose first act is
`assert_eq!(range.expected.len(), current.len())` (`live_program.rs:498`) —
and pre-seal `expected` is empty while `read_snapshot_from_view` always returns
full-length buffers. That is a length-mismatch assert, not the diagnostic an
unmapped byte is supposed to produce.

Guarded by `is_canonical()` (a live transaction exists, which implies sealing),
so it is not currently reachable. The predecessor `matches_view` form had the
same shape — it returned `false` when unsealed and fell into the same path — so
this is inherited, not introduced. Worth an explicit `sealed` check or a
corrected comment.

### Verified sound: the byte-lane swizzle

This was the highest-risk item — a lane error reports "unchanged" for changed
memory. It is correct.

The claim under test, `changed_ranges_into` (`recompiled/mod.rs:773-777`):

```rust
for lane in 0..4 {
    if live[at + 3 - lane] != mirror[at + 3 - lane] {
        push(out, head + at + lane);
    }
}
```

I modelled `copy_logical_bytes`' mapping (logical byte `n` at storage `(ps+n)^3`,
`rdram.rs:337-339`), `set_expected`'s mirror construction (`mod.rs:598-611`),
and this lane walk, then brute-forced every combination of
`physical_start ∈ [0,8)` and `len ∈ [1,40)`, checking that storage index
`(ps+head)+at+(3-lane)` equals `(ps + (head+at+lane)) ^ 3` for every word and
lane:

```
lane/mirror checks=5328 mismatches=0
```

Head and tail (≤3 bytes each) stay on the per-byte `read_u8` path
(`mod.rs:727-738`, `:784-795`), the same rule `copy_logical_bytes` applies, so
an unaligned range cannot be decided by a different rule than the copy would.
Runs coalesce across the head/body and body/tail seams through one shared
`push` sink, and the `first` guard (`mod.rs:719`) correctly prevents a run from
an earlier watched range absorbing a byte from this one.

The `apply_changed_from_view` word-widening (`mod.rs:848-854`) — the trickiest
arithmetic in the diff, because a partial word cannot be reversed in isolation
— was brute-forced over every `(physical_start, len, lo, hi)` combination:

```
widening checks=346898 violations=0
```

Every widened span stays within `[head, head+body]`, is word-aligned relative
to `head`, and covers the body portion of the changed range.

This is also covered empirically:
`changed_ranges_from_view_matches_the_copying_path`
(`tests/mutation_state.rs:93-231`) is a randomized differential test across 9
layouts — unaligned start, unaligned end, both, sub-word entirely inside one
storage word, a range shorter than the 3-byte head, and a multi-range set —
64 rounds each, asserting the fast and copying paths agree on the changed
ranges, `expected`, `expected_storage_order`, `expected_page_digests`, the
watched root, and the journal root. It also churns bytes *outside* every
watched range each round to prove they never influence the answer.

### Verified sound: mirror coherence

`expected`, `expected_storage_order` and `expected_page_digests` have exactly
two writers, and both update all three together:

- `set_expected` (`mod.rs:598-611`) — calls `refresh_page_digests` first (while
  both baselines still exist, so the dirty set comes from a `memcmp` against the
  old bytes), then rewrites both byte mirrors.
- `apply_changed_from_view` (`mod.rs:813-862`) — rewrites `expected` over the
  changed spans via `copy_logical_bytes`, rebuilds the storage mirror over the
  same spans widened to whole words, then calls `refresh_page_digests_over`.

No path updates one without the others. `seal_with` (`live_program.rs:467-493`)
routes through `set_expected` and only then reads `digest_expected()`, so the
seal-time root is computed from freshly refreshed pages. `adopt_from_view`
(`live_program.rs:769-787`) and `commit_changed` (`live_program.rs:924-931`)
both adopt *before* reading the incremental root, which is the required order.

The dirty-set derivation is conservative in both directions, which is the
property that matters: a page is skipped only when its bytes are proven equal
by a `memcmp` at the moment of update — never from a writer's declaration,
never from a dirty flag. Equal bytes have an equal digest by definition.

### Verified sound: page-tree determinism

The v2 root depends only on the range bounds and the page digests, in range
order and page order (`receipts.rs:1298-1319`). The leaf binds the schema, page
size, both range bounds, the page index, and the actual byte length
(`receipts.rs:1276-1291`) — so a short final page cannot be confused with a
zero-padded full one, and a page digest is not reusable at another position.
The range count and each range's page count are hashed, so no regrouping can
collide.

`page_tree_root_is_independent_of_incremental_history`
(`tests/mutation_state.rs:839-989`) covers the real failure modes, and it is
better than its name suggests. It asserts three distinct things per round:
the incremental root equals `digest_snapshot` computed from scratch *before*
the commit adopts (so it cannot be reading the cache); the incremental root
equals a **freshly sealed state with no history at all**; and after restoring
the original bytes, the root returns to its original value — proving the page
cache holds no residue of the path taken. The edit list deliberately hits first
byte of a page, last byte of a page, a span straddling a page boundary, a span
covering exactly one whole page, the short final page, and re-dirtying an
already-dirty page. It also verifies the journal actually chained
(`after_sha256 == next before_sha256`), so the history was long-lived rather
than a sequence of independent seals.

The one thing it does not cover is the *view* path — it drives
`commit_snapshot`, not `commit_from_view`. That gap is closed by
`changed_ranges_from_view_matches_the_copying_path`, which asserts
`expected_page_digests` and `expected_sha256` agree between the two paths every
round. Between them the coverage is adequate.

`digest_snapshot` was correctly kept cache-free (`live_program.rs:404-441`) —
validation hashes an all-zero snapshot to reconstruct the sealed-from-zero
`before_sha256`, and a cached form would be wrong for that caller.

### Verified sound: re-entrancy

`try_with_host` (`lib.rs:1824-1826`) is the only new `RefCell` interaction on
the guest store path, and it is the correct treatment: `advance_device_time_step`
holds `with_host` open across device writes, and those writes reach the
executable-write boundary observer, so a plain `with_host` there is a nested
`borrow_mut` and a hard abort.

The second borrow in that chain, `generations.try_borrow()`
(`live_program.rs:2545`), is also fallible and also resolves conservatively.

I checked for other collision sites: `try_with_host` has exactly one non-test
caller (`snapshots.rs:1045`), and no other `with_host` call sits on a
guest-store path — the ~20 in `snapshots.rs` are all host-call entry points,
which by construction are not reached from inside a `with_host` closure.

### Verified sound: the attribution/boundary split

The split held. `record_executable_and_renderer_write` (`snapshots.rs:960-980`)
is untouched by this diff and still pushes to
`PENDING_ATTRIBUTED_EXECUTABLE_WRITES` on the **wide** predicate — bare
intersection with `EXECUTABLE_WRITE_RANGES`, no residency test. Only
`classify_live_executable_write` (`snapshots.rs:1019-1052`) narrowed, and it
feeds the block-boundary decision alone.

This is the exact conflation that produced the documented
`events=0 declarations=0` bug, and the diff both preserves the separation and
documents why at `snapshots.rs:983-989`.

### Verified sound: the un-resident safety argument

The claim at `snapshots.rs:1000-1013` is that you cannot write bytes while
nothing is resident, then activate a generation over them and execute stale
code. I verified it against the source rather than the comment.

`activate_for_fetch_with_digest` (`generation/mod.rs:799-821`) computes
`live_digest` from live memory and compares against `expected_sha256` for
**every** containing candidate, unconditionally, in a loop that runs to
completion before anything else. `already_active` is computed only after that
loop and after all the miss returns (`generation/mod.rs:846`). So a later
activation over bytes changed earlier re-digests the changed bytes and returns
`AotMiss`/`NoGenerationMatched` rather than activating. Confirmed.

`guest_write_token` would be the way to cache past this, and it genuinely has
no non-test consumers — only `lib.rs:397` (the re-export) and
`runtime/tests.rs`. No activation path bypasses the digest.

### The conservative-fallback audit

Every "cannot tell" in the new guard path, and which way it resolves:

| site | condition | resolves to | safe? |
|---|---|---|---|
| `snapshots.rs:1048` | HOST borrowed / no program / catalog borrowed | `unwrap_or(true)` → assume resident → break block | yes |
| `live_program.rs:2545` | catalog already borrowed | `None` → propagates to the above | yes |
| `mod.rs:742`, `:730`, `:787` | storage out of range | `false` → copying path raises the owed panic | yes |
| `live_program.rs:381` | state not sealed | `None` → copying path | yes (but see S3) |
| `mod.rs:637` | page count or length changed | rehash every page | yes |
| **`live_program.rs:2570`** | **active generation has no backing** | **`false` → not resident → chain block** | **no — S1** |

One permissive default out of six, and it is S1.

### Test suite

`cargo test -p fn64-abi --lib` — **287 passed, 0 failed, 7 ignored**.

---

## Part 2 — the profile

### Method

Added `[profile.release.package.wm2000-block-boot] debug = 1` to
`recomps/wm2000/packages/wm2000-block-boot/Cargo.toml`. The `debug = false` at `:94-99` is
about the *generated shard crates* and stays false for them; the root package
inheriting it is why `sample` attributed 99.99% to `wm2000_block_boot::main`.
Cost of the change: the relink took **10.2s** and the binary grew 53 KB
(88,756,128 → 88,809,232), with 72 debug-map entries for the root package. The
`-C force-frame-pointers=yes` fallback was not needed.

16 runs sampled at 1 ms, aggregated to **2,710 samples**. Self time computed as
**inclusive count minus the sum of immediate children**, walking the indent
tree — `sample`'s leading integer is inclusive, and reading it as self time is
the error that caused three failed optimizations here before.

### The baseline in the brief no longer holds

The brief states 1.269s. Measured, 8 consecutive runs:

```
272.3  271.4  270.1  270.1  270.2  271.7  274.5  272.9   ms wall
```

**271 ms** (0.25 s CPU), same guest work — 1,461,883 instructions, 874 steps,
identical `logical_rdram_sha256`. That is **4.7x faster than the stated
baseline**, so 1.269s predates at least one commit in the range.

Restated: 1,461,883 instr / 0.271 s = **5.39 M guest instr/sec**, or **17.4x
slower than the N64's 93.75 MHz** — not 81x.

The A/B control confirms where it came from. With the resident-generation
boundary disabled:

```
FN64_DISABLE_RESIDENT_BOUNDARY=1  →  17172, 19129, 17487 ms
```

**63x.** `a8933a6` is by far the largest single win in the wave, and the kill
switch it shipped with is what made this measurable.

### Self time by category

2,710 samples. Sums to 100%.

| SELF% | n | category |
|---|---|---|
| **57.56%** | 1560 | **`memcmp` — mutation-guard region scans** |
| 8.52% | 231 | fn64-recomp-rs (dispatch, store, translate) |
| 6.72% | 182 | fn64-abi (HLE, pi, task dispatch) |
| 5.39% | 146 | mutation-guard logic (non-`memcmp`) |
| 4.87% | 132 | `memmove`/`memset` (snapshot + mirror copies) |
| 4.46% | 121 | allocator |
| 4.17% | 113 | fn64-runtime (rdram, executor) |
| 3.32% | 90 | sha2 (page and root digests) |
| **2.51%** | 68 | **GUEST CODE (recompiled shards)** |
| 2.47% | 67 | other (libsystem, TLS, dyld) |

Top individual frames by self time:

| SELF% | self | incl | symbol |
|---|---|---|---|
| 57.53% | 1559 | 1559 | `_platform_memcmp` |
| 4.94% | 134 | 2393 | `fn64_abi::with_executor` |
| 3.91% | 106 | 106 | `_platform_memmove` |
| 3.65% | 99 | 99 | `fn64_runtime::rdram::PhysicalRdramRead::read_u8` |
| 3.28% | 89 | 89 | `sha2::sha256::aarch64::compress` |
| 1.99% | 54 | 75 | `_xzm_free` |
| 1.48% | 40 | 310 | `Rdram::try_store_w_translated` |
| 1.40% | 38 | 50 | `record_executable_and_renderer_write` |
| 1.37% | 37 | 209 | `wm2000_block_shard_03::runner_02::run_02` |
| 1.29% | 35 | 35 | `Rdram::backing_offset` |

### The one number

**Recompiled guest code is 2.51% of self time — not the ~12% predicted.**

Inclusive of everything called beneath a shard entry point (host callbacks,
store observers, the guard work those trigger) it is 23.21%, but the shard
code's own instructions are 2.51%.

The prediction assumed overhead fell 233x while guest code stayed put. Overhead
did fall — but the mutation guard did not become a small constant, it became a
*whole-region `memcmp` on every dispatch boundary*, which is O(watched bytes)
rather than O(bytes written). At 1 MiB per boundary that scan alone is 57.56%
of runtime.

The practical reading is the opposite of the one the brief anticipated: codegen
is **not** yet the path to real-time. Guest code has ~40x of headroom above it
before it constrains anything. The remaining runtime work is worth roughly
**8-10x**, and it is nearly all in the guard.

---

## Part 3 — ranked opportunities

Ranked by expected gain against risk. All figures are measured self time from
the profile above, not estimates from reading.

### 1. Stop scanning the region twice at the HostAbi boundaries — ~7%, low risk

**Mechanism.** `flush_host_abi_transaction_inner` computes `changed` at
`live_program.rs:2218`, then calls
`invalidate_pending_physical_writes_from_view` at `:2231`, which recomputes the
same thing via `matches_view` (`:2427`) and possibly a third time
(`:2449`). Pass the already-computed `changed` down instead. Same for
`checkpoint_catalog_host_transaction_before_suspend`.

**Gain.** 3.73% + 3.62% (the redundant `matches_view` calls) ≈ **7.1%**.

**What would break.** The two scans are not currently at the same instant —
`notify_host_abi_write` runs between them (`:2226-2228`), and
`invalidate_pending_physical_writes_inner` drains
`PENDING_EXECUTABLE_WRITES`/`PENDING_ATTRIBUTED_EXECUTABLE_WRITES` at
`:2335-2338`. Reusing the earlier `changed` assumes nothing mutated watched
RDRAM in between. That holds only if `notify_host_abi_write` never writes guest
memory — verify before doing this, and keep the second scan behind a
`debug_assert` comparing it against the reused list.

### 2. Make the dispatch reconcile incremental — up to ~45%, medium risk

**Mechanism.** `reconcile_before_dispatch > matches_view` alone is **13.10%**,
and it is one entry point among several all doing the same O(1 MiB) scan. The
guard is O(watched bytes) per boundary when the information it needs is
O(bytes written). The page digests introduced by `2d1741e` are already the right
data structure — the region is divided into 370 pages and each has a maintained
digest. A dirty-page set maintained by the store observer would turn the scan
into "rescan only pages some write touched".

**Gain.** Most of the 57.56%. If typical boundaries touch a handful of pages,
the scan cost falls by roughly the page-count ratio.

**What would break.** This is exactly the reverted optimization documented at
`live_program.rs:2375-2385` — gating on "does some queued write intersect a
watched range" is **wrong**, because the write queue does not enumerate every
mutation of watched memory. At least one path reaches RDRAM without passing
through `record_executable_and_renderer_write`, and skipping on that assumption
resurfaced as the `0x0009b0b3` panic.

The distinction that makes this version viable is the one already drawn at
`:2416-2424`: asking *RDRAM itself* is safe, asking the *write queue* is not.
So a dirty-page scheme must be driven by something that cannot miss a write —
hardware dirty bits, an mprotect scheme, or a cheap per-page checksum — not by
the declaration queue. **Do not attempt the queue-driven version.** Given that
history, this needs a full route under `FN64_DISABLE_RESIDENT_BOUNDARY` A/B
before it is trusted.

#### Resolved 2026-08-07: the page tree cannot narrow the scan. Lever closed.

The proposal was to let the v2 page digests identify candidate pages and run
the byte scan only inside them. The appeal is real — the region is 370 pages
and the measured change rate is **1.005 page rehashes per commit**
(`checkpoint-digest-cost.md:481`), so the scan does ~368x more work than the
change requires.

**It does not work, and the reason is not the write queue.** It fails on
arithmetic that holds regardless of soundness.

`expected_page_digests` is a **cache of the baseline**, not an observation of
live RDRAM. All five call sites of `watched_page_digest_v2` hash either
`self.expected` (`mod.rs:642`, `:657`, `:895`) or a caller-supplied snapshot
(`live_program.rs:425`). **Not one hashes live RDRAM.** The stored digests
therefore say nothing about what RDRAM currently holds; they describe what the
baseline holds, which is precisely the side of the comparison that is already
in hand and never the side that needs reading.

To make a page digest answer "did live RDRAM change", it must be recomputed
from live RDRAM — and SHA-256 must read **every byte of the page to produce
it**. Detecting change across 370 pages by digest means hashing all 1,513,056
bytes. The `memcmp` it would replace reads *the same 1,513,056 bytes* and stops
early on the first difference. Digesting is strictly worse: same reads, plus
SHA-256 compression per byte instead of a vectorized 32-byte-per-cycle compare.

The dirty-page set is not free-standing information either. `refresh_page_digests`
derives it at `mod.rs:654` with `if self.expected[lo..hi] == bytes[lo..hi]` —
a full-region `memcmp` — and `refresh_page_digests_over` (`:876`) takes it from
`changed`, which came from the scan. **The page tree's 1.005 rehashes/commit is
a saving on hashing, achieved by consuming a comparison someone else already
paid for. It is downstream of the scan, so it cannot replace the scan.**

That is why the 57.56% is not addressable this way: the guard's cost is the
*read* of the watched region, and every candidate substitute (digest, checksum,
Merkle path) must perform that same read to be trustworthy. Only a mechanism
that learns of writes **without reading** — hardware dirty bits or `mprotect`
— could break the O(watched bytes) floor, and both are out of scope for a
correctness guard that must not miss a write.

Left on the table deliberately. The guard has caught two real corruptions.

### 3. Stop cloning the whole program on every watched store — ~4%, low risk

**Mechanism.** `classify_live_executable_write` (`snapshots.rs:1045`) does
`try_with_host(|host| host.canonical_recompiled_program.clone())` on **every
store into the watched region**. `CanonicalLiveBlockProgramV1` is `#[derive(Clone)]`
(`mod.rs:385`) and most fields are `Rc`, but `bootstrap_evidence:
Option<BootstrapOrImportValidationEvidenceV1>` (`mod.rs:418`) is a by-value
field that deep-clones — visible in the profile as
`BootstrapOrImportValidationEvidenceV1::clone` calling into
`_xzm_xzone_malloc`, and matched by the `drop_in_place` and `_xzm_free` on the
way out.

**Gain.** `CanonicalLiveBlockProgramV1::clone` is **4.17% inclusive** (113
samples), and it accounts for much of the 4.46% allocator category. Realistic
gain **~4%**.

**Fix.** Do the residency test inside the `try_with_host` closure against a
borrow, rather than cloning the program out to call a method on it. Or wrap
`bootstrap_evidence` in `Rc`.

**What would break.** Doing the test inside the closure means `HOST` is held
while `generations.try_borrow()` runs. That is a *narrower* window than today
(the clone already runs under the same borrow), but it must stay `try_borrow`.

### Measured outcome of items 1 and 3 (landed 2026-08-07)

8 runs each, same guest work (1,461,883 instructions), complete run output
`diff`-identical to the baseline in every lane — `sim_time=1492883`,
`logical_rdram_sha256=3514f2a1…`, and every device counter.

| state | ms | M instr/s | vs 93.75 MHz |
|---|---|---|---|
| baseline | 271.1 | 5.39 | 17.4x |
| + single HostAbi scan | 259.5 | 5.63 | 16.6x |
| + no program clone | 241.3 | 6.06 | 15.5x |

**11.0% total.** The clone (18.2 ms) paid more than the double scan (11.6 ms),
inverting the profile's ranking — the double scan's predicted ~7% was measured
against `flush_host_abi_transaction`'s two rows only, but the reuse also
removes the `matches_view` short-circuit scan on the `checkpoint` path, while
the clone's 4.17% inclusive understated the allocator traffic it caused.

Item 2 (incremental reconcile) is closed as not viable — see above.

### 4. `target-cpu=native` in isolation — unknown, low risk

The brief's earlier test changed three variables at once
(`codegen-units=1` + `lto="thin"` + `target-cpu=native`), took 9m13s to build,
and came out 2.3x slower — unattributable.

I started an isolated `target-cpu=native` build (RUSTFLAGS only, separate
target dir) but it did not finish within this session's budget; a cold build of
the shard catalog is the 40+ minute case the Cargo comments warn about.

**Given the profile, deprioritize this.** 57.56% of runtime is `memcmp` inside
libsystem and 2.51% is guest code — neither is materially affected by the
host's `-mcpu`. The upside is bounded by the ~20% that is handwritten Rust, and
the earlier 2.3x regression suggests the shard catalog reacts badly to codegen
changes. Items 1-3 are strictly better uses of the same time.

### Not worth doing

- **sha2 (3.32%)** — **RETRACTED. This number was wrong and this conclusion
  sent later waves at the wrong target.** Re-measured with `xctrace`/PMU
  sampling and ground-truth ASLR load addresses, SHA-256 is **26% of steady
  state** — the single largest cost in the run — of which 20 points are
  `digest_expected` re-hashing all 256 page digests of the 1 MiB watched region
  on every commit. The `sample`-based figure here could not see it because the
  release build inlines the dispatch loop and `sample` reports inclusive counts
  without raw addresses. See `resolvable-self-time-profile.md` for the method,
  the two silent traps that produce confident wrong profiles, and the corrected
  category split.
- **Guest codegen (2.51%)** — the ~12% figure the brief expected would have
  capped runtime work at 8x and made codegen the path forward. At 2.51% it does
  not. Revisit once the guard is fixed and guest code is a double-digit share.

---

## Reproducing

```
cd recomps/wm2000/packages/wm2000-block-boot
source ../../.claude/local.env
export ROM="$FN64_DISCOVER_NWXE_ROM"
C=~/Code/aki-recomp/captures; G="$C/wm-general-exception-images"
export FN64_EXECUTABLE_IMAGES="$G/run-1/image.json:$G/run-2/image.json:$G/run-3/image.json"
export FN64_BOOT_CONTEXT="$C/wm2000-boot-context.json"
export FN64_ABSENT_N64DD=1 FN64_BLOCK_MAX_STEPS=40000000 FN64_BLOCK_MIN_GUEST_INSTRUCTIONS=1461877
cargo build --release --bin wm2000-block-boot -j 6
```

`FN64_EXECUTABLE_IMAGES` and `FN64_BOOT_CONTEXT` are needed at **build** time as
well as run time — `build.rs:312` asserts on the former.

The run is only ~271 ms, so a single `sample` yields ~150 samples. Aggregate
across runs: launch the binary, `sample $! 5 1 -f out-N.txt -mayDie`, repeat,
then sum self time (inclusive minus immediate children) across the files.

The A/B control for the resident boundary is
`FN64_DISABLE_RESIDENT_BOUNDARY=1`.
