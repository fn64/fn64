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

**60fps tier (per-game conversion): render field must fit 16.667 ms.**
CORRECTED 2026-08-09 (the −6.3/23.0 figures were stale): corrected render
field is **25.65 ms** (`rt64-on-the-block-lane.md:489`), and the gap to the
30fps ceiling is **−1.15 ms** (−2.99 for the 5% margin). The four "defect
leads" were re-audited: mprotect+barrier is OFF by default (premise false),
the wrapper 2x was the already-fixed mirror boundary, sha roots were closed
by the v3 Merkle migration. What actually remains, measured:
- **`FN64_WM_SHARD_VERIFY_LIVE_WORDS=0`: −3.50 ms predicted (−2.2
  realistic)** — ablation-measured, already gated in build.rs, no emitter
  change. ALONE closes the gap. Defence-in-depth removal (the belt-and-braces
  detector for bypass writers), adjacent to the 0x0009b0b3 lesson → needs the
  FULL byte-identity verification set before shipping, not a partial one.
- rt64 `NativeRdramRollback` copies all 8 MiB per submission (2.818/field,
  −0.4 to −1.4 ms) — likely redundant with the ABI staging contract.
- RDP-stream digest per-word updates + double image hash (−0.2 to −0.5 ms).
  The independent unbounded `rsp_rdp_observations` memory bug now has an
  explicit typed interactive constant-space mode; complete ordered retention
  remains the default and remains mandatory for certification/release evidence.
Plus the standing constraint: nothing may key on "every second field
renders."

## The plan, in order

### 1. Measure the windowed lane post-fix — DONE 2026-08-09, still over budget
Owner played `wm2000-shell` (rt64, AutoNoVsync, 1280x960, ~63k frames):
**frame_interval p50 46–51 ms (~20 fps), p95 67–106, p99 spikes 200–296 ms.**
`pump_ms` is bimodal — p50 flips between ~16 ms and ~48 ms across heartbeat
windows — so the cost is split between the emulation pump and the present
path, not one owner. Caveats binding the next measurement: this binary ran
with `verify_live_words` ON (worth ~7 ms/drawn frame; build the next shell
with `FN64_WM_SHARD_VERIFY_LIVE_WORDS=0` after the A/B clears it), and
interactive sessions must select constant-space RSP/RDP observation retention
after loading the ROM; the certification lane continues retaining complete
ordered evidence. Block lane simultaneously measures
15.58 ms/field — the gap between 31.15 ms core and ~48 ms presented is the
shell-side + verify budget to reclaim.

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
