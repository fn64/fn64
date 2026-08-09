# fn64: position and plan (2026-08-09, updated after the predecode fix)

**Goal: all five AKI titles 100% playable through fn64, discovery → runtime →
render. Position: 1 of 5 playable — and that title is now PAST its 30fps
floor: 32.1 fps measured with 6.5% margin. Revenge boots and renders; a
longer-route re-run is in flight past the 0x07 wall.**

## The frame-rate ledger (all rt64, unprofiled, byte-identical 8/8)

| step | per-field mean | drawn frame | fps |
|---|---:|---:|---:|
| baseline (pre mirror fix) | — | ~69.8 ms | 14.3 |
| mirror fix `8109435` (one line) | 17.25 | 34.49 | 29.0 |
| **RSP predecode `456c920`** | **15.58** | **31.15** | **32.1 ✅ 30fps + 6.5% margin** |

Predecode delta −1.67 ms/field (predicted −2.5–2.8 from sample shares — shares
inflate, as pre-registered; target met regardless). p95 also fell 28.1 → 24.4.

**60fps tier (per-game conversion): render field must fit 16.667 ms.** Now at
~23.0 → **−6.3 ms to go.** Named leads: memmove 11.1% (attributed:
staging-dominated per the workflow), guest-runtime wrapper ~2x its payload,
sha digests ~5%, mprotect+barrier ~9%. All defect-shaped. Plus the standing
constraint: nothing may key on "every second field renders."

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

## Standing constraint: native rate now, no ceiling later (owner, 2026-08-09)

30 Hz titles render **faithfully at 30 fps** — that is the guaranteed floor.
But nothing in the architecture may **block** a per-game higher rate later:

- The emulation core is already rate-agnostic: VI fields run at hardware 60 Hz
  and the guest decides when it renders. The render/non-render bimodality is
  guest behaviour, not an emulator assumption. Keep it that way — no
  optimization may key on "every second field renders."
- The only 30-ism found in a sweep is the shell's `FN64_FRAME_PACE_MS`
  *default* (33.3 ms, env-overridable, 0 disables). Make it per-title when the
  shell grows config; it blocks nothing today.
- **60 fps conversion is a per-title, clean-room patch layer** (decoupling or
  doubling the game's 30 Hz update, as recomp-community 60fps mods do) plus an
  emulator budget of **16.667 ms per render field** (−9.0 ms from today, −6.4
  post-predecode). The profile says the remaining cost is defect-shaped
  (wrapper 2x its payload, prepare 2x execute, memmove under attribution),
  not intrinsic — so the budget is credible, and the patch layer is the part
  fn64 does not have yet.
