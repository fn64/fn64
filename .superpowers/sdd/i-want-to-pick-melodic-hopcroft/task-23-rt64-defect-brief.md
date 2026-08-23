# Task 23: characterize the rt64-hle-defect (gen-blend-aa-sloped-edge) — read-only

## The finding
Fan-out Pass 2 surfaced ONE `rt64-hle-defect` case: `gen-blend-aa-sloped-edge`.
Classification `rt64-hle-defect` means wgpu matches bit-accurate angrylion but
RT64 (the C++ HLE oracle) does NOT — i.e. RT64 is wrong, wgpu is right. This is
the mirror image of the shared-ported-bug cases.

## Investigate (READ-ONLY)
1. Read the case in the parity runner
   (`crates/fn64-render-conformance/src/bin/fn64-render-conformance-parity-runner.rs`)
   — the builder + intent for `gen-blend-aa-sloped-edge`. It is an antialiased
   partial-coverage edge on a SLOPED triangle (nonzero dXHdy) with a
   coverage-driven blend. Get the exact SetOtherModes / blend word and geometry.
2. Run the parity runner (FN64_GENERATE=1, or FN64_ONLY=blend-aa) and read the
   3-way result: confirm wgpu==angrylion and RT64 diverges; capture the exact
   diff — how many pixels, where (which edge pixels), and the pixel values
   (wgpu/angrylion vs RT64). Build first:
   `FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 cargo build -p fn64-render-conformance --features parity-runner --bin fn64-render-conformance-parity-runner --offline`
   (full runner may stall in Metal init — kill+rerun or use FN64_ONLY; macOS has no `timeout`).
3. Characterize the RT64 bug: WHAT does RT64 do wrong on a sloped AA edge? Likely
   candidates — coverage computation on a sloped span, edge-pixel coverage
   quantization, or the coverage→blend B-factor. Read RT64's HLE source (under
   $FN64_RT64_DIR / the RT64 tree) for the AA/coverage edge path and state the
   precise divergence. Cross-check against memory [[rt64-fn64-disagreements]],
   [[rt64-port-discrepancy-map]], [[fillrect-cycle-edge-rules]].
4. Confirm wgpu is genuinely correct here (matches angrylion), not accidentally
   matching — is there a hand-derived expectation, or only angrylion? Note the
   admissibility caveat (angrylion is clean-room-excluded as SOLE authority per
   [[angrylion-mame-license-blocks-oracle]]) — but here wgpu agreeing with
   angrylion AGAINST RT64 is still a strong signal RT64 has a bug.

## Deliverable
Is this a real RT64 HLE defect? If yes: the precise mechanism, the affected
pixels/values, whether it's backportable to RT64 upstream, and whether it belongs
on the RT64 issue-review card ([[rt64-issue-review-card]]). If wgpu's correctness
can't be admissibly confirmed, say what would confirm it. READ-ONLY — no code
changes, no worktree.

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-23-report.md` + concise summary.
