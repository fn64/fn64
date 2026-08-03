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
| No Mercy (NW4E) | running | | | |
| WCW/nWo Revenge | **PASS** exit 0 | 3 | 25,057 | was `InvalidResidentSplit`; fixed by the resident-tail clamp below |
| WCW vs nWo World Tour | **PASS** exit 0 | 3 | 25,375 | same fix, same commit |

Reading trap worth keeping: per-bank `unsupported` lines can be nonzero and
still compose to zero (WM2000's boot bank reports 3, `recovered_overlay_2`
reports 8, HEADLINE is 0) because a destination unmapped in one bank is
resident in another. The HEADLINE is the verdict.

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
