---
name: fn64-verify
description: Canonical full-workspace verification in a clean-target pinned worktree, and push-on-green. Use before every push; mandatory when diagnosing an apparent test regression.
---

# fn64-verify

The shared checkout's `target/` accumulates rlibs from feature-varying
builds (parallel agents, feature experiments). The fn64-recomp-rs
differential tests (`interp_differential`, `mapped_fetch`) compile emitted
code against an rlib found in `target/debug/deps` — in a polluted target
dir they FAIL FALSELY and look exactly like a real regression. A two-hour
bisect once chased this ghost. The canonical verdict comes only from a
clean-target worktree.

## One-time setup (survives sessions)

```sh
git worktree add --detach <SCRATCH>/verify-tip HEAD
```

`<SCRATCH>` is any per-session tmp dir (e.g. `$CLAUDE_JOB_DIR/tmp`). Keep a
dedicated `CARGO_TARGET_DIR` beside it that ONLY ever sees default-feature
workspace builds.

## The gate loop (per wave of commits)

```sh
tip=$(git -C <REPO> rev-parse --short HEAD)
git -C <SCRATCH>/verify-tip checkout --detach "$tip"
cd <SCRATCH>/verify-tip && CARGO_TARGET_DIR=<SCRATCH>/verify-target \
  cargo nextest run --workspace
# green (baseline: 3175 passed, 10 skipped as of 2026-08) -> push:
git -C <REPO> push origin <branch>
```

Rules:
- Never diagnose a differential-test failure from the shared tree; re-run
  in the pinned worktree first. Shared-tree pass + clean-tree fail = real.
  Shared-tree fail + clean-tree pass = stale-rlib ghost.
- The gate tests COMMITTED state. Uncommitted work in the shared tree is
  invisible to it — commit first, gate second.
- If the tip moved while the suite ran, the push carries ungated commits;
  gate the new tip next wave rather than pretending they were covered.
- After touching fn64-render-reference or moving fn64-abi shims, also run
  the doc regeneration couplings (see rust-module-split skill, step 5) or
  CI's doc-drift lint will fail even though nextest is green.
