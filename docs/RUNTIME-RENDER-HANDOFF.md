# Runtime/render integration handoff

Status: the prior resume order (v28 substrate, WM2000 gate, DPC counter-clear
slice, rebase, PR update) is complete as of 2026-07-24. This file records
delivery state, not a parity claim. The remaining hardware and full-ROM gaps in
`UNIVERSAL-RUNTIME-PLAN.md`, `BASE-RENDERER-BEHAVIOR-MATRIX.md`, and
`RT64-GAP-REGISTER.md` remain open.

## Authoritative checkpoint

- Worktree: `/private/tmp/fn64-runtime-render-integration-20260724`
- Branch: `integration/runtime-render-parity-20260724`
- Rebased onto `origin/main` (`a96fa51`) with merge topology preserved; the
  branch is now zero commits behind and ahead only by its own integration work.
- Draft PR #88 is `MERGEABLE` and its body reflects this checkpoint.

The integrated stack includes reviewed DPC transaction scheduling, AI timing,
physical FGR/FR behavior, exact COP1 environment work, bounded RT64 S2DEX2
object rectangles, the reviewed audio evidence/HLE substrate with live-IMEM
LLE remaining release authority, reviewed COP0 authority across the
arbitrary-PC lanes, the v28 release-evidence substrate, the WM2000
voice-map-intervention-free gate, and the DPC STATUS counter-clear slice.

## What the completed resume order delivered

- **v28 release-evidence substrate** cherry-picked and independently
  re-reviewed clean (no P0/P1/P2). The general verified private-release
  capability stays confined to production admission; the sole checked golden is
  bound to aarch64-apple-darwin plus both build-produced archive hashes and the
  combined native identity.
- **WM2000 voice-map-intervention-free gate** cherry-picked and reviewed clean.
  The scenario name does not overclaim a transform-free run; the harness still
  applies its documented `osGetTime` and optional fallthrough transforms.
- **DPC STATUS counter-clear slice** ported from donor `ac237a4` (AI FULL and
  VI scale work excluded) onto the existing `DpcCounter24` typed authority.
  Adds the four counter-clear commands (0x0040/0x0080/0x0100/0x0200 →
  tmem/pipe/cmd/clock), fixes the cancellation-ownership bug so cancellation
  restores only start/end/current/status (never resurrecting a cleared counter
  or erasing an interleaved mode command), and routes raw DPC counter reads
  (0xA410_0010..001C) to the live device. Independently reviewed clean.
- **clippy** — the recomp-only FPR bit accessors are gated behind `recomp-rs`
  (closing the previously documented `fn64-abi` dead_code blocker), and PR
  #87's ICF-folding test shim is cast through `*const ()` for clippy 1.96's
  `function-casts-as-integer`.

## Evidence at this checkpoint

- Full workspace: 2474/2474 tests pass across 98 suites, zero failures.
- DPC determinism under `AGENTS.md`: cancellation tests 20/20 consecutive fresh
  runs; selective-clear and shim/raw-MMIO convergence 10/10.
- Strict clippy clean on the composed gate — `fn64-cpu-runtime` all-targets plus
  `fn64-abi`, `fn64-runtime`, `fn64-boot-harness`, `fn64-render-rt64`, both
  featureless and with `--features recomp-rs`, `-D warnings`.
- Base-renderer matrix, RT64 macOS/platform certification, and RT64 feature
  inventory generators/checkers clean; docs lint clean; `git diff --check`
  clean.
- Rebase conflict resolution regenerated generated docs rather than choosing a
  stale side: this branch's COP1 and S2DEX2 evidence is preserved while
  `origin/main`'s function-lane report schema bump (v23 to v27) is adopted.

## Known inherited items

Two pre-existing clippy 1.96 lints exist on `origin/main` outside the composed
gate's scope and are intentionally not patched on this branch, since they live
in unrelated crates: `fn64-discover/src/boundaries.rs` (`unnecessary-sort-by`)
and shared `build_support.rs` dead code surfaced only by `fn64-shell`'s build
script (from PR #89). A full-workspace `-D warnings` clippy reports these until
they are addressed upstream.

## Resume order

1. Decide whether to close the two inherited full-workspace clippy lints
   (either upstream on `origin/main` or on this branch) before merge.
2. Move draft PR #88 to ready only once a representative current-schema
   full-ROM series and independent RT64 pixel validation exist; until then the
   draft state correctly signals a delivery checkpoint, not a parity claim.
3. Resume broad parallel residual work only after this checkpoint merges.

The project goal remains unfulfilled until representative current-schema
full-ROM series retain zero unsupported events and deterministic framebuffer,
audio, device, and memory digests; RT64 pixels are independently validated; and
the documented hardware-exact residuals are either closed or explicitly bounded
by retained evidence.
