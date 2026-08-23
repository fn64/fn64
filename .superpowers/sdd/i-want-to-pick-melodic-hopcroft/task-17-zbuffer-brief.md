# Task 17: implement Z-buffer raw-DPC binding (SetZImage 0xfe + SetMaskImage 0x3e)

## The gap (proven by fan-out Pass 1)
All 6 zbuffer parity cases REFUSE in wgpu: `gen-zbuffer-compare-disabled`,
`gen-zbuffer-farther-loses`, `gen-zbuffer-nearer-wins`,
`gen-zbuffer-source-sel-pixel-wins`, `gen-zbuffer-update-disabled`,
`gen-zbuffer-setmaskimage-binds-z-image`. RT64 and angrylion both render them.
The wgpu raw-DPC decoder has no binding for the z-image commands, so any
z-compared/z-updated draw is refused as "outside the subset". Entirely
unimplemented opcode pair.

## Opcodes
- **SetZImage (0xfe)** binds the depth buffer base address (like SetColorImage
  0xff binds the color image). Address field mirrors SetColorImage.
- **SetMaskImage (0x3e)** — the runner's zbuffer slice names SetMaskImage(0x3e)
  as the z-image binder. Read
  `crates/fn64-render-conformance/src/bin/fn64-render-conformance-parity-runner.rs`
  (the zbuffer builders near the other `gen_*`) for the EXACT command words each
  of the 6 cases emits; `gen-zbuffer-setmaskimage-binds-z-image` is the direct probe.

## What to do
1. Trace where wgpu refuses: build the runner (below), `FN64_GENERATE=1`, find
   where the 6 zbuffer cases decline. Likely an unhandled opcode in
   `crates/fn64-render-wgpu/src/raw_dpc/`.
2. Implement the z-image binding: decode 0xfe (and/or 0x3e per the cases) to
   record the z-buffer base address + format in raw-DPC state, mirroring
   SetColorImage binding. Wire z-compare / z-update into the CPU triangle raster
   path (`crates/fn64-render-wgpu/src/targets/raw_triangle.rs` — where
   guest-visible pixels are produced) so overlapping triangles at different depths
   resolve per SetOtherModes `z_compare_en` / `z_update_en` / `z_source_sel`.
3. Reference for CORRECT behavior = RT64 + angrylion (the runner 3-way compares);
   ground truth = angrylion (bit-accurate).

## Verify (REQUIRED — build and run)
Build:
```
FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 cargo build -p fn64-render-conformance --features parity-runner --bin fn64-render-conformance-parity-runner --offline
```
Triage (write to a temp file, parse with python3):
```
FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 FN64_GENERATE=1 target/debug/fn64-render-conformance-parity-runner > z.json
```
The 6 zbuffer cases must go from wgpu-refused to `wgpu_vs_angrylion_diff_pixels: 0`.
If a case can't reach 0-diff, report it as a residual with the diff count and
first-diff pixel — do NOT suppress it.
Also run the gate (must PASS):
```
FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 target/debug/fn64-render-conformance-parity-runner > gate.json
python3 scripts/check_rt64_parity.py < gate.json
```
Add a wgpu-crate unit test pinning the z-compare decision (nearer wins, farther
loses) so a reverted depth test fails a test.

## Constraints
- You are in an ISOLATED worktree — work here only, never touch other worktrees.
- Surgical: implement in existing raw_dpc/raster paths; do not restructure production.rs.
- angrylion oracle is external/MAME — reference its OUTPUT only, never link it. External
  oracle binary: /Users/jer/Code/angrylion-oracle/oracle (the runner calls it, fail-open).
- Commit on your worktree branch (do NOT push):
  `feat(render-wgpu): bind z-image (SetZImage 0xfe/SetMaskImage 0x3e) and wire depth test`
- Report to `.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-17-report.md`: where the
  refusal was, the binding + depth-test impl, before/after on all 6 cases, gate result,
  unit test, commit hash. Return the commit hash + a one-line summary.
