# fn64: position and plan (2026-08-09, end of session)

**Goal: all five AKI titles 100% playable through fn64, discovery → runtime →
render. Position: 1 of 5 playable; that title's emulation ceiling is 1.16 ms
from its 30fps budget; four titles CPU-recompile.**

52 commits this session. Everything below is measured, with digests and logs.

## Where each number stands — use these, not older figures

| | value | provenance |
|---|---|---|
| WM2000 emulation ceiling (rt64, headless) | **17.25 ms/field → 34.49 ms/frame → 29.0 fps** | two unprofiled reps, 0.17% apart, byte-identical, `renderer: rt64` in-log |
| 30fps budget | 33.333 ms/frame | **gap: 1.16 ms** |
| mirror fix (`8109435`) | 8.75 → 0.18 ms/field | −20% on the reference lane, confirmed unprofiled |
| reference-lane figures (27.96 ms, 17.9 fps, 22.6 ms gap) | **wrong lane** — software rasterizer | `render-benchmark.zsh` never set `FN64_RENDER`; now echoes `renderer:` |
| windowed / presented frame time | **UNMEASURED** | the shell adds present cost, "never less" (`render-benchmark.zsh:87`) |

**The two claims are different and only the first is supported:** the
*emulation ceiling* is 29.0 fps; whether the *game the owner plays* reaches it
depends on presentation cost, which nobody has measured post-fix.

## The plan, in order

### 1. Measure the windowed lane post-fix (hours, decisive)
`wm2000-shell` has never been measured since the mirror fix. The owner's last
session showed p50 ~50 ms windowed — but that predates `8109435` and was on a
binary carrying the 9 ms mirror defect. **One instrumented windowed session
answers whether the played game is at ~29 fps or still stuttering**, and if the
latter, present cost is the entire remaining problem and it is shell-side, not
emulation. The owner must drive it (or an input schedule must replay a route);
the measurement itself is the heartbeat that already exists.

### 2. Close the last 1.16 ms if the ceiling itself needs it (days)
Post-fix decomposition (perturbation-corrected): rasterization 8.30, RSP 5.09,
guest code 8.23, invalidate 1.68, staging 1.45, audio 0.98. **Four rows are
individually ≥ the 1.16 ms gap.** No architecture work required. Closed lines:
copyback narrowing (99.49% dead bytes, still wrong direction), RSP micro-opt,
depth-copy elimination, mirror gating, instruction budgeting.

### 3. Second playable title: author an input schedule for Revenge (human, days)
Revenge is five of six: 15/15 bindings, recompiles (0 unsupported of 1,749),
title-specific shard tree, boot context (`d8c097f8…`), validated exception
images. **The schedule is genuinely human** — WM2000's is 124 lines of menu
navigation, every screen evidenced by a committed frame dump. No Mercy is at
the same point; Revenge's smaller ROM makes it the cheaper first attempt.
Everything mechanical now exists: `--title` generation, inventory selection,
digest-recorded artifacts.

### 4. VPW2 bookkeeping (hours)
15/15 bindings, recompiles (0 of 4,648). Needs an answer key and
`FN64_DISCOVER_*` env entries so regressions can be graded. Bookkeeping, not
engineering.

### 5. World Tour: a scoped project, not a fix (weeks; decide before starting)
3/15 bindings; 7/7 skeleton-different code; a 1996 libultra generation. The
per-symbol matrix and drift sweep are in `wcw-host-binding-recognizers.md`.
Decide whether 5-of-5 justifies a parallel recognizer set before anyone starts.

## Standing hazards (all bitten this session, all documented in the skill)

- **State the renderer with every number** — two readers measured the wrong
  lane in one day, one of them having just read the warning.
- Shipped figures are **unprofiled means**, never profiled numbers or p50s.
- Every per-title artifact carries its **ROM digest** — four variant collisions
  in one session, all from title-string filing.
- One owner per run; a frozen log is not a dead process; `pgrep -x` not `-f`.
- Three pre-existing test failures predate this session; the suite was never
  green. `emit.rs:1121`'s stale label list is wire-format for a receipt digest
  — fixing it is a versioning decision.

## What was delivered this session, for the record

ROM-free releasable builds (owner-verified, byte-identical, rebuild
reproducible). Four of five titles CPU-recompiling — Revenge unblocked from
"bounded out of the lane" by two behavioural recognizer fixes. The mirror fix:
−20% shipped, from one line. FN64_PROFILE: one command, counter tree enforced,
five historical instrument failures now caught by construction. The corrected
target: 29.0 fps ceiling against a 30fps goal, not 54% of a field to remove.
