# Runtime/render integration handoff

Status: paused at a clean checkpoint on 2026-07-24. This file records delivery
state, not a parity claim. The remaining hardware and full-ROM gaps in
`UNIVERSAL-RUNTIME-PLAN.md`, `BASE-RENDERER-BEHAVIOR-MATRIX.md`, and
`RT64-GAP-REGISTER.md` remain open.

## Authoritative checkpoint

- Worktree: `/private/tmp/fn64-runtime-render-integration-20260724`
- Branch: `integration/runtime-render-parity-20260724`
- Clean HEAD: `20dae5d` (`fix(recomp): fail closed on new COP0 decodes`)
- Remote comparison at handoff: 28 commits ahead of and one commit behind
  `origin/main`; fetched main is `6596566`.
- Do not infer that the isolated v28 or WM2000 commits below are present here.

The integrated stack includes reviewed DPC transaction scheduling, AI timing,
physical FGR/FR behavior, exact COP1 environment work, bounded RT64 S2DEX2
object rectangles, the reviewed audio evidence/HLE substrate with live-IMEM
LLE remaining release authority, and reviewed COP0 authority across the
arbitrary-PC lanes. The final COP0 commits are:

- `1e4e011` — enforce COP0 authority across composed lanes;
- `20dae5d` — fail closed if a future recognized COP0 decode lacks authority
  classification.

Independent COP0 review reported no P0/P1/P2 after `20dae5d`. Explicit COP0
residuals are Reserved Instruction/malformed encoding behavior, unpredictable
control transfers in delay slots, the whole-function precise ERET/exception
boundary, and silicon-exact Random timing.

## Evidence at this checkpoint

After audio and COP0 integration, the authoritative branch passed:

- recompiler plus ABI composition: 604/604 tests, 8 skipped;
- full public workspace: 2452/2452 tests, 8 skipped;
- strict all-target `fn64-recomp-rs` clippy;
- base-renderer matrix, RT64 feature inventory, macOS certification, and
  platform certification generators/checkers;
- documentation lint: 287 references across 62 documents; and
- `git diff --check`.

The RT64 S2DEX2 native gate separately retained 10/10 fresh Metal processes
with exact RDRAM SHA-256 `dd1694195986db0ca633c44727c0bf23f76e3feb1810b19f3b8799b6efab9c6a`
and post-VI SHA-256 `394924cd4165863fbb78e503486bcba6291f8994931beb08d8d666a114b79bef`.
These focused results do not close the broader renderer or full-ROM claims.

## Isolated work ready for review

### Release evidence v28

- Worktree: `/private/tmp/fn64-v28-substrate-port`
- Branch: `integration/v28-substrate-port-20260724`
- Clean commits: `73d21f4` followed by `13168ad`.

`73d21f4` forward-ports report schema v28, DeviceState v15, canonical 24-bit
DPC counter evidence, and the exact synthetic-native fingerprint mechanism
without importing the donor's older COP1 implementation. Its original review
found a caller-forgeable synthetic capability and an implicit platform-bound
golden. `13168ad` addresses both: no general verified capability escapes the
specialized synthetic operation, and the sole checked golden is explicitly
bound to aarch64-apple-darwin plus the exact two compiler-produced archive
hashes and combined native-program identity. The fix has focused unit and one
exact-ten native parent run, but it intentionally stopped before a fresh
independent re-review, full composed gates, or integration.

Resume by independently reviewing `13168ad`, then cherry-pick `73d21f4` and
`13168ad` onto the authoritative branch. Expect documentation conflicts in
`BASE-RENDERER-BEHAVIOR-MATRIX.md` and `UNIVERSAL-RUNTIME-PLAN.md`; preserve
the current COP0 row, preserve the v28 evidence changes, and regenerate rather
than choosing either stale generated side wholesale.

### WM2000 voice-map-intervention-free gate

- Donor gate: `18a7f82`.
- Review correction: `4292998` on
  `integration/wm2000-v28-gate-reviewfix-20260724`.

The gate arms unsupported journaling before guest execution, requires an exact
scheduled VI edge and authoritative post-VI RT64 capture, binds both linked
native archives, and fails on skipped/early termination or the diagnostic
voice-map mutation. `4292998` narrows all claims and the scenario name to
`voice-map-intervention-free`; the harness still applies its documented
`osGetTime` and optional fallthrough transforms. Integrate these only after
the v28 substrate. No private WM2000 ROM run, exact-ten series, or removal of
all harness transforms has been verified.

### DPC STATUS counter commands

The CPU/device path still omits public DPC STATUS counter-clear commands
0x0040/0x0080/0x0100/0x0200. Pending-renderer cancellation also restores a
whole DPC register snapshot, which can erase a later mode command or resurrect
a cleared counter. Do not cherry-pick donor `ac237a4` wholesale: it combines
unrelated older AI/VI work. Port only the DPC slice after v28, preserving
`DpcCounter24` and the typed scheduling authority. Because this closes an
ordered cancellation interleaving, its focused cancellation tests require at
least 20 consecutive clean runs under `AGENTS.md`; deterministic selective
clear and ABI convergence tests require 10.

## Resume order

1. Re-review `13168ad`; fix any P0/P1/P2 before integration.
2. Cherry-pick `73d21f4 13168ad`, resolve generated docs, and rerun composed
   runtime/boot/RT64/workspace/clippy/generator gates.
3. Cherry-pick `18a7f82 4292998`; run public WM/boot-harness tests. Keep private
   ROM evidence explicitly unverified unless an admitted run is actually made.
4. Port and independently review the focused DPC counter-clear/cancellation
   lane with the 20-run interleaving bar.
5. Rebase with merge topology preserved onto `origin/main` `6596566`, rerun
   affected ABI dispatch and full composed gates, then update the draft PR.
6. Only after this checkpoint merges should broad parallel residual work
   resume.

The project goal remains unfulfilled until representative current-schema
full-ROM series retain zero unsupported events and deterministic framebuffer,
audio, device, and memory digests; RT64 pixels are independently validated;
and the documented hardware-exact residuals are either closed or explicitly
bounded by retained evidence.
