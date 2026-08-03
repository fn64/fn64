# AKI recompile certification — measured 2026-08-03

`gate_rom_recompile` is generic: one input (`FN64_DISCOVER_ROM`), no boot
harness, no answer key, no per-game constants. It had never been run against
any AKI title — WM2000's certification came from `gate_wm2000_recompile`,
which is hardcoded to that one game.

What it proves: discovery finds banks cold, every proven code word is packed
with digest-bound block geometry, emitted as Rust, compiled by a real
`rustc`, run, and probed at arbitrary guest PCs, with **every branch/jump/call
destination either recompiled ahead-of-time or covered by an instrumented
interpreter fallback** (`unsupported=0`). What it does NOT prove: a booting
game. RSP audio and RDP graphics are separate subsystems and the gate never
consults host bindings.

| title | result | banks | blocks | detail |
|---|---|---|---|---|
| WrestleMania 2000 (NWXE) | **PASS** exit 0 | 5 | 43,032 | exact_aot=110 block_aot=1937 dynamic_mips=19 |
| Virtual Pro Wrestling 2 | **PASS** exit 0 | 5 | 49,329 | first-ever attempt, cold |
| No Mercy (NW4E) | **PASS** exit 0 | 6 | 57,284 | exact_aot=0 block_aot=1820 dynamic_mips=11; was a non-terminating walk, fixed below |
| WCW/nWo Revenge | **PASS** exit 0 | 3 | 25,057 | was `InvalidResidentSplit`; fixed by the resident-tail clamp below |
| WCW vs nWo World Tour | **PASS** exit 0 | 3 | 25,375 | same fix, same commit |

Reading trap worth keeping: per-bank `unsupported` lines can be nonzero and
still compose to zero (WM2000's boot bank reports 3, `recovered_overlay_2`
reports 8, HEADLINE is 0) because a destination unmapped in one bank is
resident in another. The HEADLINE is the verdict.

## No Mercy's blocker: a non-terminating walk, not a slow one

No Mercy was killed at 71 minutes without reaching emission. The natural
reading — 6 banks vs WM2000's 5, so something superlinear in bank count — was
wrong. **It was not slow. It never terminated.**

`sample(1)` on the live process attributed 6807 of 6807 samples to a single
leaf, `partition::same_bank_overlaps`, called from `prove_exact_owners_inner`
*before* the per-root assessment loop it was assumed to be stuck in. (An
earlier profile stopped one frame short and read `prove_exact_owners_inner`
as the hot function; the leaf is what identifies the bug.)

`same_bank_overlaps` walks a chain of CFG blocks across each owner's extent:

```rust
let mut pc = owner.root_va;
while pc < owner.extent_end {
    ...
    let Some(b) = blocks_by_start.get(&pc) else { break };
    pc = b.end_va;              // never advances when end_va <= pc
}
```

**Zero-length blocks are a legitimate `Cfg` shape, not corruption.** `cfg.rs`
constructs one whenever a root lands in fenced data: "when the block's own
start is fenced it becomes a zero-instruction fence block," with
`end_va == start_va` (pinned by `data_fence_stops_descent_before_the_fenced_word`).
A `RanOffEnd` block at the image edge can have the same shape. When such a
block starts inside an owner's extent, `pc = b.end_va` leaves `pc` unchanged,
the `while pc < extent_end` guard stays true, and the loop spins at 100% CPU
forever. No limit, timeout, or round budget bounds it, because it never
returns to any code that has one.

WM2000 and the three other AKI titles simply have no fenced root inside an
owner extent, which is why four of five certified and the fifth hung — the
distinguishing input is one block shape, not ROM size.

**Worth noticing in the result, not hiding:** No Mercy certifies with
`exact_aot=0` — every one of its 1,820 recompiled destinations is `BlockAot`.
That is a legitimate `unsupported=0` (the emitter "does not need or trust
function boundaries," and `BlockAot` covers a destination exactly as `ExactAot`
does), but it means zero of its destinations landed on a proven exact function
entry, where WM2000 gets 110. Certification does not depend on that number;
boundary recall reporting does, so do not read this PASS as evidence that
exact-owner proof works well on this ROM.

**The fix** stops the walk at a block that covers no bytes, which is the same
disposition the loop already gave the address-has-no-block case immediately
above it: the extent past such a block is not walkable coverage, so it yields
no overlap evidence either way. The clause only ever ends a walk earlier than
before, and only in the case where the previous behavior was to not end at
all, so no overlap pair that used to be reported can be lost.

Alongside it, `prove_exact_owners_inner` now indexes its per-bank evidence
once instead of rescanning the whole `FactDb` for every candidate root:
`proven_executable_ranges` was recomputed per root, and `validate_incoming` /
`validate_indirects` each walked every fact per root (the latter per CFG block
per root). `BankFactIndex` is a reindexing of exactly the facts those scans
matched, so the blocker sets are unchanged; only the cost is, from
`O(roots × facts)` to `O(facts + roots × hits)`. This was found while
profiling and is a real cost reduction, but it is not what unblocked No
Mercy — a faster loop that never terminates still never terminates.

## The shared blocker: `InvalidResidentSplit` — diagnosed and fixed

Revenge and World Tour failed identically, before emission, in
`build_generation_topology_v1`. Both are the two-overlay swap-pair games M1b
recovered (both images at one VA). The failure was in composing a generation
topology from that geometry, not in discovery — their overlays ARE recovered
and graded (Revenge: 745/1020 exact, wrong=0).

**Which clause, measured.** Of the four-clause guard, only
`invalidation_end < resident.load_end` tripped. Alignment and
split-inside-the-resident-bank were all satisfied:

| | resident | split | overlay union end |
|---|---|---|---|
| WM2000 | `[0x80000400,0x80100400)` | `0x800e1b90` | `0x80171a60` (past end) |
| Revenge | `[0x80000400,0x80100400)` | `0x80090000` | `0x800fafa0` (21,600 short) |
| World Tour | `[0x80000400,0x80100400)` | `0x80090000` | `0x800f8af0` (30,992 short) |

**The ASSUMPTION was wrong, not the data.** The guard required overlays to
overwrite the resident bank all the way to `resident.load_end`. That end is
not a discovered code extent: for every ROM this path admits it is
`entry - ipl3_delta + BOOT_COPY_SIZE`, the fixed 1 MiB IPL3 boot copy
(`banks/mod.rs`). Nothing obliges a game's overlays to reach a hardware
constant. WM2000's happen to; the swap-pair titles' do not. The recipes' own
`bss_end` values are internally consistent and were not mis-derived.

**The fix** (`e5e7d39`) clamps the resident-tail image to
`min(resident.load_end, union_end)` instead of requiring the union to cover
it. The trailing resident span no overlay writes becomes immutable — the same
status as the pre-split prefix — rather than being folded into a generation
whose invalidation could not contain it. That last part is the real
soundness content: the runtime rejects `invalidation < image`
(`PrecompiledGeneration::new` → `InvalidationDoesNotContainImage`), so the
old rule was not protecting an invariant, it was working around one. The
clamp only ever shrinks the tail image, so no byte becomes tail-owned that
the old rule did not already grant; the surviving clauses still reject a
split outside the resident bank, and a degenerate empty tail now returns a
precise `EmptyResidentTail` rather than the blanket error.

## What jessetbh's pipeline tells us about the answer keys

Researched from the local GPL checkouts (process and formats only; no code
copied). `WCWSyms` is a single-commit build artifact: `gen_symbols.py` is a
**regex scraper over splat's disassembly text**, transcribing whatever
splat/spimdisasm decided about boundaries. There is **no verification pass** —
no byte-level round-trip, no matching build. Symbol correctness is validated
only by "does the game crash at that call site," so any wrong boundary that
never crashes is never caught.

Three error classes fn64 should expect in the grading oracle:

1. **Sizes overshoot.** When spimdisasm emits no explicit size, `gen_symbols.py`
   falls back to `next_function_vram - this_vram`, folding trailing alignment
   padding into the preceding function. Treat dump.toml sizes as upper bounds.
2. **Tail-call-via-`j` mis-splits.** splat splits a single function in two when
   it tail-calls a shared exit sequence with bare `j`. Their own
   `func_80018C24` needed a hand-written size override to fix. So some of
   fn64's `interior_entries` may be fn64 being right.
3. **Colliding-address names are pipeline artifacts.** Two overlay sections
   loading at the same VA produce duplicate names disambiguated by a
   first-seen-wins rule invented by the scraper, not by the binary. The
   "canonical" name at such an address is arbitrary.

**fn64's byte-exact rebuild is a stronger check than anything in that
pipeline.** That is worth stating plainly rather than treating the key as
ground truth.

## The hand-configured checklist = what "fully automatic" must cover

Enumerated from their per-game configs. fn64 derives some of these already;
the list is the honest scope of the remaining problem:

1. Entrypoint, ROM<->VRAM section mapping, section sizes — fn64 derives.
2. Overlay geometry: count, shared vs exclusive VA ranges, loader descriptor
   format — fn64 derives (M1/M1b), and this is where Revenge/World Tour now
   fail downstream.
3. Function boundaries and sizes — fn64 derives, at 76-79% recall.
4. **libultra/OS function identification — the largest hand-effort category
   in their work, and fn64 has no mechanism for it.** Their own cross-game
   fingerprint transfer got only 3/46 between two closely related titles
   because IDO/libultra versions differ. Their actual method was
   crash-driven forensics: call-graph position, MMIO addresses touched,
   constant literals (PI magic `0x22222222`, PIF delay `0x165A0BB`), struct
   field-write patterns.
5. Which identified functions must NOT be substituted (the `gu*`/`sinf` trap:
   naming them breaks the build because no host shim exists).
6. Save type (Controller Pak vs cart SRAM) — behavioral, not static.
7. Unrecompilable-opcode stubs — mechanical (cop0/cache/eret/tlb scan), and
   the one category they automated.
8. Cooperative-yield scheduler patches — the SAME idle-thread deadlock
   recurred in both AKI games with the same one-instruction fix, which
   suggests a detectable pattern rather than a per-game surprise.

## Categories that genuinely cannot be recompiled straight

From their stub lists. Categories 1, 2 and 6 are opcode-detectable; category
3 is the dangerous one, because the code recompiles *silently wrong*:

1. Privileged CPU instructions (mfc0/mtc0, tlbwi/tlbp, eret).
2. COP2 glue in CPU code.
3. **Hardware MMIO drivers** — no privileged opcodes, so a recompiler happily
   translates them, and the result reads/writes RDRAM at the register's
   numeric offset instead of the peripheral. Invisible to static opcode
   scanning; surfaces only as garbage or an access violation.
4. Thread/scheduler internals reading runtime-owned globals.
5. Hand-written assembly whose boundaries the disassembler mis-segments.
6. IDO soft-float helpers using MIPS III FPU instructions.
7. RSP microcode — outside CPU recompilation entirely.

## Secondary goal: corpus-wide certification (2026-08-03, first measurement)

`gate_rom_recompile` is ROM-agnostic, so the AKI result raises an obvious
question nobody had measured: how much of the 287-ROM corpus certifies?

First sample, six ROMs in corpus order, cold, no configuration:

| ROM | result |
|---|---|
| 007 GoldenEye (Europe) | **PASS** `unsupported=0` |
| 007 The World Is Not Enough | **PASS** `unsupported=0` |
| 1080 TenEighty Snowboarding | **PASS** `unsupported=0` |
| AeroGauge (USA) | **PASS** `unsupported=0` |
| Air Boarder 64 (Europe) | FAIL `SourceFieldsChanged { record: 0 }` |
| All Star Tennis 99 (USA) | **PASS** `unsupported=0` |

**Five of six certify cold, including GoldenEye** — a title with a well-known
reputation for resisting static analysis. The machinery generalizes past the
AKI family without per-game work.

Context for the denominator: 41 of 287 corpus ROMs recover overlay geometry;
the other 246 are mostly single-bank and were never expected to need it. So
"has overlay geometry" is not a prerequisite for certification, and the
addressable set is much larger than the overlay count suggests.

The one failure is a specific, nameable blocker in overlay-recipe recovery
(`SourceFieldsChanged`), not a general limitation. A wider batch is running.

### Wider corpus batch — the failure class is singular

A second batch (ROMs 7-40 in corpus order) is still running, but the first
completed results already name one shared blocker rather than many:

| ROM | result |
|---|---|
| Armorines - Project S.W.A.R.M. | `unsupported=1` — `0x00292e00:OutsideAllMappings` |
| Army Men - Sarge's Heroes | `unsupported=1` — `0x801f9930:OutsideAllMappings` |
| Army Men - Sarge's Heroes 2 | `unsupported=1` — `0x80214690:OutsideAllMappings` |

Each reaches the scoreboard and emits successfully; each has **exactly one**
destination landing at an address no discovered bank covers. Two are the same
engine, so this is likely a family-shaped geometry gap (a bank the descriptor
search does not recover) rather than three unrelated defects.

That is a far better failure mode than a structural one: the addressable work
is "recover one more mapping", and a single fix plausibly certifies several
titles at once -- the same shape as the `InvalidResidentSplit` fix that
certified Revenge and World Tour together.

## The `0xbfc0...` frontier: measured, still open, and correctly so

Ran the canonical receipt (`gate_wm2000_recompile` with
`FN64_DENSE_MANIFEST_ONLY=1`, receipt `sha256=4cd5e0fe...`,
byte-identical across two runs). Of the five admission conditions:

| # | condition | status |
|---|---|---|
| 1 | initial BootContext Status clear | FAILS — no private capture on this machine |
| 2 | dense/external Status scans + value proofs close | FAILS — boot `value_open=4 unclassified=2` |
| 3 | exact 15 host symbols and effects | **PASSES** — all `unique_structural_semantic_match` |
| 4 | normal-vector handlers have scanned owners | FAILS — all six vectors open, `external_images=0` |
| 5 | writer/DMA/transfer closure | FAILS — 14 open writer classes, 52 open stores |

**The four open Status value proofs are genuine limits, not defects.** Decoded
from ROM bytes (BEV is bit 22): `0x8002a2ac` and `0x80036fb0` mask with
`0xffff00ff`, so BEV sits in the *preserved* half and is carried from
runtime-mutable memory; `0x8002a2c8` is `or $t0,$a0` against an unconstrained
argument, where known-zero correctly collapses; `0x800376d0` is a bare
`mtc0 $a0,$12`. Closing them needs interprocedural argument closure -- proving
the callable-entry set and every caller's `$a0`. The abstract interpreter is
not leaking soundness: `read_static_word` zeroes both known-bit masks on every
load-image read, so ROM bytes can never masquerade as runtime invariants.

**The instructive one:** `0x800367ac` decodes as `mfc0 $k1,$12; and $k1,~3;
mtc0` -- a textbook BEV-preserving RMW that would close trivially if
classified. It is unclassified only because the CFG never reaches it: the boot
bank seeds the ROM header entry point alone, giving **27 proven roots / 197
reachable blocks against 262,144 aligned words**. Promoting a candidate word
to proven code to pass this condition is precisely the bar-lowering the
frontier exists to prevent, so it stays open.

Conditions 1 and 4 fail on *absent private inputs* rather than evidence
limits; supplying a validated BootContext and external image captures would
likely close them. Conditions 2 and 5 would still block on the 27-root CFG,
so the frontier stays open regardless.
