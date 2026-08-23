# Task 30: diagnose the WM2000 perf regression (READ-ONLY, bisect-style)

## Signal
The owner reports WM2000 (all-fn64 rs+wgpu lane) performance was **better last
night** and something regressed since. Find WHAT commit/change regressed the
drawn-frame time and by how much. READ-ONLY diagnosis — do NOT fix, propose the fix.

## Measured anchors
- This session Task 22 measured **49.1 ms/drawn frame (render field 43.24 ms + off
  5.83)**, ~20.4 fps, 1.47x over the 33.3 ms 30Hz budget — on the CURRENT HEAD.
- "Last night better" means an EARLIER commit was faster. The regression window is
  the commits below (only render/execute/shell-path commits can affect runtime perf;
  docs/tests/parity-corpus commits CANNOT — ignore them).

## Prime suspects (commits that touch the live render/execute/shell path, newest→oldest)
Bisect around these — they are the only ones that can move runtime perf:
1. **fcd48b7c** feat: bind z-image + wire DEPTH TEST — this added PER-PIXEL
   z-compare/z-update into `raster_triangle` (crates/fn64-render-wgpu/src/targets/raw_triangle.rs),
   the ~483 ns/px hot loop. **TOP SUSPECT**: if WM2000 triangles now execute a depth
   path (SetOtherModes z_compare/z_update, or a z-image bound) they didn't before,
   every covered pixel pays for it. Check: does WM2000 actually set z bits / bind a
   z-image? If the depth code runs unconditionally (even when z disabled) that's the
   regression.
2. **435dbbab** fix: coverage CLR_ON_CVG write path — changed the per-pixel coverage
   write decision (targets/texrect.rs). Check if it added work on WM2000's common path.
3. **1d8c0d11** fix: LoadBlock DxT — mark_block_footprint_valid back-fill; per-LOAD
   not per-pixel, but check it isn't invoked hot.
4. **c8ba2cb5** TexRectFlip — adds a flipped_axes bool + coordinates_at branch in the
   texrect per-pixel loop; a new per-pixel branch. Check the non-flip path didn't slow.
5. **9ffa4e69** — tests only (microbench), NO production change; should be inert. Verify.
6. **5120e619** present black for blanked VI, **924effda** seal executor on exiting,
   **c6eacd65** VI_STATUS via fabric field — shell/present path; less likely per-frame
   hot but check present() cost.

## Method
- Use `git log`/`git diff` to inspect each suspect's ACTUAL diff on the runtime path.
- Prefer the **headless raster microbench** for a clean per-pixel A/B without the GUI:
  `texture_plane_raster_microbench` (crates/fn64-render-wgpu/src/targets/raw_triangle/tests.rs:1181,
  #[ignore], `cargo test -p fn64-render-wgpu ... -- --ignored --nocapture`). Build at
  HEAD, measure ns/px; then `git stash`-free: build at a suspect's PARENT commit
  (checkout the file or the commit in a SCRATCH copy — do NOT disturb the shared
  worktree's HEAD or other sessions), measure, compare. Isolate the regressor.
  (The full windowed pump census is the ground truth but GUI-blocked for sandboxed
  agents; the microbench catches a per-pixel raster regression cleanly.)
- Anchor to fn64-perf-method discipline: numbers renderer-tagged (wgpu+rs), read
  shares/ns-per-pixel, unprofiled.

## Deliverable
The regressing commit (or commits), the mechanism (what per-pixel/per-frame work it
added on WM2000's path), the measured delta (before/after ns/px or ms/frame), and a
proposed fix direction (e.g. gate the depth path on z_compare_en/z_update_en actually
set, so z-disabled triangles pay nothing). Do NOT implement — propose. If it turns out
NOT to be a code regression (e.g. measurement variance / different lane last night),
say so with evidence.

## Constraints
- READ-ONLY. No production changes to the shared worktree. If you need to build an
  older commit, do it in a scratch checkout/copy — never move the shared worktree HEAD
  or touch other sessions' work.
- Report to `.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-30-report.md`.
