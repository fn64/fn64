# Where fn64 stands, and the four things left

Written 2026-08-09 at the end of a long session. **The standing goal — all five
AKI titles 100% playable through discovery, runtime and render — is 1 of 5, and
that title is not at 60fps.** This file is the handoff: what is true, what is
scoped, and what to do first.

## The five titles

| title | state | what it needs |
|---|---|---|
| WrestleMania 2000 | **boots and renders**, 1.94x budget | the 60fps work below |
| No Mercy | 15/15 host bindings on the **on-disk ROM**; build can express it | executable-image PCs, input schedule |
| VPW2 | 15/15 host bindings | an answer key; no `FN64_DISCOVER_*` entry |
| WCW/nWo Revenge | **15/15 bindings, CPU-recompiles** (0 unsupported of 1749) | shard tree, boot context, executable-image PCs, input schedule |
| WCW vs nWo World Tour | **1 of 9**, 7/7 skeleton-different | support for a 1996 libultra generation |

## The four remaining projects, in the order I would take them

### 1. ~~Revenge's two recognizers~~ — DONE 2026-08-09 (`10a73c7`, `7fa28f5`)
Both predicates were pinning compiler artifacts rather than ABI: a hardcoded
register in one, a hardcoded stack frame size in the other. Revenge inlines
less, so its frame is 24 bytes not 32 and every o32 stack-argument offset
shifted. Made frame-relative; **Revenge went 7/9 → 15/15 and now
CPU-recompiles with `unsupported_destinations = 0` of 1,749.**

**Four of five AKI titles now CPU-recompile.** Revenge needs the same bring-up
as No Mercy from here: shard tree, boot context, executable-image PCs, input
schedule. **That bring-up — for either title — is now the shortest path to a
second playable game**, and it is item 2.

### 2. No Mercy's remaining bring-up — the only path to a second playable title
Everything mechanical is done: 15/15 confirmed on the actual ROM, the topology
generator produces its shape (21 overlay shards — **generate it, do not
transcribe the stale 24**), and the build can select a non-WM2000 inventory.
What is left is genuinely human-gated: locating the executable-image PCs and
authoring an input schedule. See `docs/plans/second-aki-title-scoping.md`.

### 3. 60fps — an architecture question, not an optimization pass
Render field on RT64 is **36.5 ms against 16.667**. You must remove **19.8 ms,
54% of the field**, and **no single line exceeds 0.67x budget**:

    guest code 9.79 | mirror 9.01 | RSP 5.76 | rasterization 4.00
    invalidate 2.04 | staging memcpy 1.77

There is no bottleneck. Every single-target attempt this session came back null
or misdirected. The two candidates large enough to matter are architectural:
the **8 MiB-per-submission RDRAM round-trip**, and the **mirror's coupling to
the write barrier** (proven safe to gate, proven to catch nothing, and gating
it measured *null* because reading the dirty set is what lets the barrier take
a free early-out). **Nobody has asked whether the mirror can be made cheaper
rather than removed.**

Use `FN64_PROFILE=1`. One command, one report, the counter tree enforced.

### 4. World Tour — a project, not a fix
7 of 7 routines are different code, flat at 0 matches through 35% opcode drift.
This is a 1996 launch title against 1998 siblings. Supporting it means a second
libultra generation in the recognizer set. Worth knowing the size before
anyone starts.

## Three things to check before trusting the tree

1. **Three pre-existing test failures** predate this session. The suite was
   never green. `the_spread_rows_...` in `frame_census` (a test-construction
   artifact, diagnosed), `precompiled_admission::...miss_evidence` (a real
   `first_diff_offset` semantics gap), and
   `prepared_tree::...deterministic_and_idempotent` (33 vs 36).
2. **`generated_runner_build/build.rs:1121`** hardcodes `src/emit.rs` in a
   label list that is **wire format for a receipt digest**. `emit.rs` became
   `emit/` in the #119 wave. Fixing it changes the receipt version — a
   deliberate decision, not a typo repair.
3. **`examples/wm2000-block-boot/src/shell.rs` is uncommitted** and belongs to
   the owner: present-mode, frame-pacing, and the audio-heartbeat fix.

## What this session established, so it is not re-derived

- **fn64 can ship builds with no copyrighted ROM content.** Words come from the
  user's ROM at runtime via emitted geometry; verified zero ROM words against a
  control carrying 126, guest byte-identical 8 of 8, and the binary boots and
  renders. `b86fc95`.
- **The 60fps target is graphics, and graphics is RDP** — but 79% of the
  reference lane's RDP was an artifact of the software rasterizer, not the
  renderer the owner runs. Measure on RT64. `ab0b9be`.
- **An infinitely fast renderer still misses 60fps**: host-side alone is 1.29x
  budget. `0014aae`.
- **Two blocked titles are two different problems**, and the record's "0
  candidates, bounded out of the lane" was wrong about both. `9dbecc0`.
- **The method rules are in the loadable skill** (`.claude/skills/fn64-perf-method/`),
  including a section on running the work rather than the experiment — five
  sequencing rules, each earned by a specific cost this session.
