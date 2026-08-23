# Task 22: WM2000 post-fix perf + on-screen verification (read-only / measurement)

## Why
This session landed 4 wgpu fixes (TexRectFlip, z-image+depth, LoadBlock DxT,
coverage CLR_ON_CVG) — all gate-verified but NOT yet checked against a running
WM2000 ROM. The original acceptance bars are: (1) no visible rendering defects,
(2) frame rate at the 30Hz budget (33.3ms/drawn frame). Re-establish where those
stand post-fix.

## What to measure (headless-measurable parts — do these)
Run the all-fn64 stack: `FN64_RECOMP=rs FN64_RENDER=wgpu`. The runner script is
`scripts/play-wm2000.sh`. For bounded perf measurement use the pump census env:
`FN64_PUMP_CENSUS=1 FN64_PUMP_CENSUS_WARMUP=300 FN64_PUMP_CENSUS_PUMPS=1200`.
Consult the `/fn64-perf-method` skill conventions and memory
[[wm2000-gameplay-perf-baseline]], [[wm2000-wgpu-perf-attribution]],
[[sample-cannot-see-guest-coroutines]] (use phase counters, NOT leaf profiles).

1. **Attract-mode drawn-frame rate.** WM2000 renders every other field, so pair
   adjacent pumps: two-field total = one drawn frame; compare to 33.3ms.
2. **Phase attribution** of any per-field overage (FN64_PHASE_TIMING /
   FN64_EXECUTOR_SPLIT / FN64_PROFILE) — is gfx_lle_rdp still ~90% of overage?
3. Report: drawn frames/s, ms/drawn-frame, % over/under budget, top cost center.
   Compare against the known baseline (memory says 52.79 ms/field = 3.17x budget
   for gameplay; attract ~30ms mean — CONFIRM or update these numbers).

## Visual correctness (do what's measurable; flag the rest)
- The genuinely on-screen visual check needs the live GUI window and is a HUMAN
  spot-check (memory [[shell-interactive-probe.md]]). Do NOT claim visual
  correctness you cannot measure.
- What you CAN do: if there is any frame-capture / PNG-dump path (grep for a
  screenshot/capture env or flag in the shell), capture logo/menu frames and
  check for the known artifacts (boot static — already fixed; AKI/THQ logo yellow
  block / black bar; horizontally-duplicated menu text). Report what the captures
  show. If no capture path exists, say so and list exactly what a human should
  eyeball.
- Note whether any of the 4 fixes' opcodes (TexRectFlip 0x25, z-image, LoadBlock
  DxT, CLR_ON_CVG coverage) actually fire in a WM2000 capture — i.e. does the ROM
  exercise what we fixed? Grep the census/capture data.

## Constraints
- READ-ONLY: measure and report, no code changes, no git worktree.
- Bounded runs only (don't launch an unbounded GUI loop that never exits — use
  the census PUMPS cap or a short warmup).
- If `play-wm2000.sh` needs a ROM path / interactive login you can't provide,
  report exactly what's missing rather than guessing.

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-22-report.md`: the drawn-frame
rate (attract, + in-match if reachable), phase attribution, whether the 4 fixed
opcodes fire in WM2000, any measurable visual findings, and a precise list of what
still needs a human on-screen glance. Return a concise summary.
