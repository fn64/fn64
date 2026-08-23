# Task 18: LoadBlock (0x33) DxT row-advance for row >= 1 (RGBA16 + CI8)

## The gap (proven by fan-out Pass 1)
6 loadblock-deep cases REFUSE in wgpu:
`gen-loadblock-deep-ci8-dxt-fractional-triangle`,
`gen-loadblock-deep-ci8-dxt400-triangle`, `gen-loadblock-deep-ci8-dxt800-triangle`,
`gen-loadblock-deep-rgba16-dxt-fractional-triangle`,
`gen-loadblock-deep-rgba16-dxt400-triangle`,
`gen-loadblock-deep-rgba16-dxt800-triangle`. These exercise LoadBlock's DxT
row-advance (the per-row TMEM stride a single LoadTile can't catch), sampled by a
TEXTURED TRIANGLE (not texrect, to avoid the texrect-coverage confound). RT64 and
angrylion render them; wgpu refuses.

## Background: LoadBlock DxT
LoadBlock (0x33) loads a linear run of texels into TMEM. The DxT field is the
fixed-point increment (1.11 format) added to the T (row) accumulator per texel;
when the accumulator crosses an integer boundary the load advances to the next
TMEM row. For row >= 1, DxT determines when the row wraps. dxt400 / dxt800 /
fractional are three DxT values that place the row boundary at different texel
counts. The wgpu LoadBlock path apparently only handles the row-0 / single-row
case and refuses (or mis-loads) when DxT drives a row >= 1.

## What to do
1. Read the runner's loadblock-deep builders in
   `crates/fn64-render-conformance/src/bin/fn64-render-conformance-parity-runner.rs`
   for the exact DxT values and texel counts.
2. Trace where wgpu declines: build the runner (below), `FN64_GENERATE=1`, find
   the refusal for these 6 cases. LoadBlock lives around
   `crates/fn64-render-wgpu/src/tmem/wire.rs` (the TMEM load-word logic) and its
   callers.
3. Implement the DxT row-advance so texels past the first row land at the correct
   TMEM addresses (per the 1.11 DxT accumulator + row wrap). Handle RGBA16 (16bpp)
   and CI8 (8bpp) source strides. Respect TMEM padded-word loads (RDP copies whole
   64-bit words — see the padded-word behavior already in wire.rs).
4. Reference for CORRECT behavior = RT64 + angrylion; ground truth = angrylion.

## Verify (REQUIRED — build and run)
Build:
```
FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 cargo build -p fn64-render-conformance --features parity-runner --bin fn64-render-conformance-parity-runner --offline
```
Triage:
```
FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 FN64_GENERATE=1 target/debug/fn64-render-conformance-parity-runner > lb.json
```
The 6 loadblock-deep cases must go from wgpu-refused to
`wgpu_vs_angrylion_diff_pixels: 0`. If a case can't reach 0-diff, report it as a
residual with the diff count + first-diff pixel — do NOT suppress it. Note: the
fractional-DxT cases may legitimately differ if fn64's model can't represent the
partial tail — if so, document it precisely (this connects to the known
tmem-padded-word-loads behavior), don't force a wrong match.
Gate (must PASS):
```
FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 target/debug/fn64-render-conformance-parity-runner > gate.json
python3 scripts/check_rt64_parity.py < gate.json
```
Add a wgpu-crate unit test pinning the DxT row-advance (a texel that must land on
row 1 lands there) so a reverted stride fails a test.

## Constraints
- You are in an ISOLATED worktree — work here only.
- Surgical: implement in the existing TMEM load path; do not restructure.
- angrylion oracle is external/MAME — reference OUTPUT only. Binary:
  /Users/jer/Code/angrylion-oracle/oracle (runner calls it, fail-open).
- Commit on your worktree branch (do NOT push):
  `fix(render-wgpu): LoadBlock (0x33) DxT row-advance for row >= 1 (RGBA16/CI8)`
- Report to `.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-18-report.md`: where the
  refusal was, the DxT impl, before/after on all 6 cases, gate result, unit test,
  commit hash. Return the commit hash + a one-line summary.
