# Task 25: two-cycle TEXEL1 combiner gap — scoping

VERDICT: refusal is the wgpu texrect path rejecting Texel1 unconditionally; fix is
MEDIUM (~150-250 LOC, second-tile addressing genuinely absent). NOT WM2000-blocking
— any-ROM breadth.

(Report authored by the read-only Explore agent, which lacked a Write tool; saved
by the orchestrator.)

## Refusal location
`crates/fn64-render-wgpu/src/targets/texrect.rs:1518-1519` —
`validate_combiner_program_for` raises
`TexrectExecutionError::UnsupportedColorInput{input: Texel1}` (alpha twin
:1544-1545). Root cause: `ADMITTED_COLOR_INPUTS` (texrect.rs:1247-1257) omits
Texel1/Texel1Alpha; `ADMITTED_ALPHA_INPUTS` (:1264-1271) omits Texel1.
`TexrectTileBinding` (:1093) carries ONE descriptor; the single texel fetch is
texrect.rs:1865-1890 (one sample_point -> tex_val0), tex_val1 left `[0.0;4]` at :1645.

## Divergence vs reference
The reference refuses Texel1 ONLY when `texture1.is_none()` (validate.rs:479-483).
The parity runner STAGES a second tile (`stage_and_declare_two_tiles`,
parity-runner.rs:4169-4185, tile 1 at tmem word 8), so reference/RT64/angrylion
render but wgpu refuses unconditionally.

## Fix size: MEDIUM (~150-250 LOC)
Second-tile addressing is genuinely ABSENT on the wgpu texrect path
(single-descriptor binding). Need:
1. A second `TexrectTileBinding` for tile+1
2. A second `sample_point` call feeding tex_val1 (single-texel path is a clean template)
3. Add Texel1/Texel1Alpha to `ADMITTED_*` gated on tile+1 present
4. Wire `run_two_cycle` (already exists, combiner.rs:1021) into the executor

## WM2000-blocking: NO
Any-ROM breadth. WM2000's own two-cycle fog (gen-two-cycle-wm2000-fog-program)
passes; census shows 0 two-cycle texrects of 2,520 in the WM2000 boot window
(docs/RT64-LANE-DIVERGENCES.md:542). Confirmed.

## Overlap with Task 24 (LOD)
`gen-two-cycle-lod-fraction-gap` fails for TWO reasons: LodFraction not in
`ADMITTED_COLOR_INPUTS` (Task 24's LOD_FRACTION supply) AND its Texel1/second-tile
fetch (this task). Fixes touch the same file (texrect.rs `ADMITTED_*` + the
lod_fraction=0.0 hardcode at triangle_pipeline_fragment.wgsl:233) but disjoint
selectors — coordinate the `ADMITTED_COLOR_INPUTS` edit to avoid a merge collision.
If both are dispatched, do them SERIALLY (same file) or as one combined fix.

Related: [[rdp-untested-surface-map]], [[rt64-ported-modules-are-inert]].
