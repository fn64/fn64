# Task 15: implement TexRectFlip (0x25) in the wgpu backend

## The defect (confirmed by Track B parity)

fn64's wgpu backend REFUSES `G_TEXRECTFLIP` (opcode 0x25) while RT64 AND the
bit-accurate angrylion oracle both render it and agree pixel-for-pixel. This is
a clean, isolated fn64 rendering gap. TexRectFlip is a TextureRectangle with S
and T axes SWAPPED (the rect's texture coordinates step along the transposed
axis) — i.e. `DsDx` applies to the T axis and `DtDy` to the S axis.

## Where it lives

- `crates/fn64-render-wgpu/src/raw_dpc/mod.rs:84` — G_TEXRECTFLIP (0x25) named.
- `crates/fn64-render-wgpu/src/raw_dpc/production_adapter.rs:13,124,151` — the
  adapter already mentions `TextureRectangleFlip` "admitted as two
  RdpTriangleCommand pushes each", so the admission machinery partly exists.
- `crates/fn64-render-wgpu/src/rt64_rdp_state.rs:255-287` — decodes both
  G_TEXRECT (0xe4) and G_TEXRECTFLIP (0xe5 in GBI / 0x25 raw) to command id 2.
- The REFUSAL is somewhere in the flip-specific coordinate/span path. Trace from
  the adapter to where the flip bit causes a refusal or an unimplemented branch.
  The existing non-flip TextureRectangle path is your template — the ONLY
  difference is the S/T axis swap in the per-pixel texture-coordinate stepping.

## What to do

1. Trace the current refusal: run the parity runner's `gen-texrect-flip` case (or
   the hand `textured-rect-flip-point-sampled` case) and find exactly where wgpu
   declines it. `FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64
   cargo build -p fn64-render-conformance --features parity-runner --bin
   fn64-render-conformance-parity-runner --offline`, then run it.
2. Implement the flip: reuse the TextureRectangle rasterization but swap the S/T
   coordinate stepping (DsDx<->DtDy axis assignment) for the flip opcode. The
   reference for correct behavior is what RT64 and angrylion produce — the parity
   runner already compares against both.
3. Verify: the flip case must go from "wgpu refused" to wgpu == RT64 == angrylion
   byte-identical. Use the standalone oracle (/Users/jer/Code/angrylion-oracle/
   oracle) if you need a direct reference render.
4. Mutation check: describe what reverting the fix does (refusal returns).

## Constraints
- Work in the worktree /Users/jer/Code/fn64/.claude/worktrees/wm2000-playable.
- Surgical: implement the flip in the existing texrect path; do not restructure
  production.rs.
- The angrylion oracle is external/MAME — reference its OUTPUT only, never link it.
- Commit (branch worktree-wm2000-playable, do NOT push):
  `feat(render-wgpu): implement TexRectFlip (0x25) texture-coordinate axis swap`.

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-15-report.md`: where the
refusal was, the fix, the before/after parity result on the flip case (refused ->
byte-identical to RT64+angrylion), mutation/revert behavior, commit hash.
