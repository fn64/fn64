# Task 25: scope the two-cycle TEXEL1 combiner gap (read-only scoping)

## The gap
These REFUSE in wgpu (RT64 + angrylion render them):
- `gen-two-cycle-texel0-combined-texel1-select` — cycle 0 forms Combined:=Texel0;
  cycle 1 blends in Texel1 (dual-tile / second-texel fetch)
- `gen-two-cycle-texel1-direct` — cycle 1 reads Texel1 directly
- `gen-two-cycle-lod-fraction-gap` — cycle 1 reads LodFraction as RGB multiplier
  (overlaps Task 24's LOD scope — coordinate: this case's TEXEL1/second-tile part
  is yours, the LOD_FRACTION supply is Task 24's)

Common thread: the SECOND texel (Texel1) / second-tile fetch in two-cycle mode.
WM2000's own two-cycle usage (`gen-two-cycle-wm2000-fog-program`) already PASSES,
so this is any-ROM breadth, not WM2000-blocking — CONFIRM that framing.

## Investigate (READ-ONLY)
1. Read the three cases in the parity runner for their exact combine words and
   tile setup (do they stage two tiles? what's the second tile descriptor?).
2. Trace where wgpu refuses. The combiner + texel-fetch path:
   `crates/fn64-render-wgpu/src/` — the two-cycle combiner setup, the sampler
   (`tmem/sample.rs`), and wherever Texel1 / second-tile selection would be
   decoded. Find the exact refusal (unimplemented Texel1 fetch? a combiner-input
   match arm that rejects TEXEL1?). Cite file:line.
3. Scope the fix: what does implementing Texel1 fetch require — a second
   tile-descriptor resolution + a second `sample_point` at the adjacent tile, fed
   into the cycle-1 combiner? Is the machinery mostly there (single-texel path is
   the template) or is second-tile addressing absent? Estimate size.
4. Note the overlap with Task 24 (LOD) so the eventual fixes don't collide.

## Deliverable
A scoping verdict: where the refusal is, what the fix touches, its size, and
confirmation this is any-ROM breadth vs WM2000-blocking. READ-ONLY — no code
changes, no worktree.

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-25-report.md` + concise summary.
