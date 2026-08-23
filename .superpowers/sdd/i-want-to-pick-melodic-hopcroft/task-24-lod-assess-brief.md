# Task 24: assess the LOD/mipmap path — dead code or wireable? (read-only scoping)

## The gap
Fan-out Pass 2 added LOD cases; these REFUSE in wgpu:
`gen-lod-fraction-combiner-enabled`, `gen-lod-fraction-combiner-disabled`,
`gen-two-cycle-lod-fraction-gap` (LOD_FRACTION as a two-cycle combiner input with
texture_lod_en). RT64 + angrylion render them.

## Known context
`crates/fn64-render-wgpu/src/texture_lod.rs` exists with `LodTileIndices`,
`LodSelection`, `compute_lod`, `hlsl_clamp_i32` — but the build warns these are
`never constructed` / `never used`, i.e. the LOD module may be DEAD CODE (ported
but unwired), similar to the [[rt64-ported-modules-are-inert]] pattern.

## Investigate (READ-ONLY)
1. Read `texture_lod.rs` fully. Is `compute_lod` a complete, correct LOD
   implementation, or a stub? What does it compute (LOD level + fraction from
   texture derivatives / tile descriptors)?
2. Is it wired to ANYTHING? Grep for referrers to `compute_lod`, `LodSelection`,
   `LodTileIndices` across the crate. If zero live referrers, it's inert.
3. Where WOULD it wire in? Trace the two-cycle combiner path and the textured
   triangle sampler (`tmem/sample.rs`, the combiner setup) — where is
   LOD_FRACTION supposed to be supplied as a combiner input, and where is the
   mip-level tile selection supposed to happen? Identify the exact call sites a
   fix would touch.
4. Scope the fix: is this "wire up existing correct code" (small) or "the ported
   code is incomplete/wrong and needs real work" (large)? Estimate honestly.
5. WM2000 relevance: does WM2000 use LOD/mipmapping? Grep the census/capture data
   for texture_lod_en set or LOD_FRACTION combiner use. (Its two-cycle fog program
   already passes without LOD, so LOD is likely any-ROM breadth, not WM2000-blocking
   — CONFIRM.)

## Deliverable
A scoping verdict: dead-code-vs-live, the fix's size and exact touch points, and
WM2000 relevance. Enough for me to decide whether to dispatch the fix. READ-ONLY —
no code changes, no worktree.

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-24-report.md` + concise summary.
