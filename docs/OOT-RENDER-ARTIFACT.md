# OoT task-249 render-artifact diagnosis

Status: root-caused 2026-07-16. This is a diagnosis of the current reference
renderer, not a claim that the RDP state layer is implemented.

## Result

The open red/black Hyrule Field artifact is caused by the unbuilt RDP state
layer, specifically by ignoring `G_SETOTHERMODE_L/H` while applying depth
compare/update to every F3DEX2 triangle. The depth comparison itself remains
correct. The defect is that fn64 does not use the display list's `Z_CMP` and
`Z_UPD` state to decide whether a draw compares or writes depth.

This finding rules out a second projection fix and does not justify shipping
`OOT_NO_DEPTH`, clearing the framebuffer per task, or any other scene-specific
heuristic. The proper next job is an RDP state model that decodes othermode,
combiner, fill/clear, and color-image state and carries the applicable state on
each draw.

## Oracle and target frame

The C-file boot was built through `examples/oot-boot/native/Cargo.toml` without
`FN64_NATIVE_RECOMP`, using the OoT-generated `RecompiledFuncs`, the clean MIT
N64Recomp headers, and the decompressed OoT NTSC 1.0 ROM outside git. With
`OOT_MAX_SWAPS=250 OOT_SKIP_AUDIO_UCODE=1`, graphics task 249 contains 900
emitted triangles. `OOT_RENDER_DUMP_LIMIT=260` is required to dump that exact
task; the prior bound of 240 stopped at task 240.

The black-box Mupen64Plus oracle's frame 250 is a dark Hyrule Field/title-demo
view with the moon near the upper-right and field geometry across the lower
half (`/tmp/oot-reference-frame250.png`, SHA-256
`e65c1e86d2e7b2df00bcb54acfc305f20503b5a1d385c718afa6d8768786e210`).
The default fn64 task-249 frame has red side/background regions and a nearly
black center (`/tmp/fn64-oot-task249-baseline.png`, SHA-256
`808533fe319f6fc2801d37e6b99589caf05711cee1ea599d20e42fb16fa42bd1`).

For the decisive A/B, `OOT_RENDER_CLEAR_EACH_TASK=1` removes prior-task color
and depth history. The task remains uniformly red. Disabling texture also
remains uniformly red. With the same clear but `OOT_NO_DEPTH=1`, the moon,
field, road, and foreground geometry immediately appear in their oracle-like
screen locations (`/tmp/fn64-oot-task249-clear-no-depth.png`, SHA-256
`753c85d3c7c183b26bb658f24035a8e68b47a361b398a1ca5b84ff976bc9e283`).
This switch is diagnostic only; it is deliberately not the fix.

## Byte and implementation evidence

- Public `gbi.h:498-503` assigns render mode to othermode low beginning at
  bit 3; `gbi.h:593-609` defines separate `Z_CMP` (`0x10`) and `Z_UPD`
  (`0x20`) bits. OoT actually emits materially different modes: setup 1 has
  no `G_ZBUFFER` and uses `G_RM_AA_OPA_SURF2`, while setup 2 enables
  `G_ZBUFFER` and `G_RM_AA_ZB_OPA_SURF{,2}`
  (`refs/oot-decomp/src/code/z_rcp.c:24-43`).
- The MIT RT64 F3DEX2 decoder updates masked othermode state
  (`third_party/rt64/src/gbi/rt64_gbi_f3dex2.cpp:24-33` and
  `hle/rt64_rsp.cpp:1026-1037`). Its render pipeline independently derives
  depth compare and write from `zCmp()` and `zUpd()`
  (`shared/rt64_other_mode.h:82-99`,
  `render/rt64_raster_shader.cpp:220-232,309-319`).
- fn64 recognizes `0xE2/0xE3` only by name and falls through to
  `skip_opcode`; then `lib.rs:238-255` routes every default F3DEX2 triangle to
  `draw_triangle_culled`, whose `raster.rs:184-201` path always enables its
  depth test. This exactly predicts the A/B result.

The current hardwired `texel * shade` path is a second, independent fidelity
failure. Task 249 emits 890 textured triangles and samples 132,001 texels,
126,913 of which are non-black, so the black-looking frame is not explained
by an all-black texture decode. OoT selects many incompatible combine modes in
adjacent setup display lists (`z_rcp.c:10-95`), including shade-only,
primitive-times-shade, texture-times-primitive, and decal; fn64 ignores those
commands and cannot reproduce the oracle colors yet.

## Hypotheses falsified or bracketed

| Candidate | Task-249 evidence | Conclusion |
|---|---|---|
| Vertex color vs signed normal | 1,832/2,164 vertices take the lighting path; zero computed lit colors are black. Forcing raw CN yields the expected rainbow-normal frame once depth masking is neutralized, while normal lighting produces coherent blue/gray shading. Public `Vtx_t`/`Vtx_tn` layouts are `gbi.h:1010-1031`; RT64's signed `/127` and light/raw selection are `RSPProcessCS.hlsl:57-61,95-119`. | Selector is working; not the red/black artifact. |
| `G_VTX` | Public packing is `gbi.h:2113-2139`; MIT RT64 extracts the same `n`, `v0+n`, and three 7-bit triangle indices at `rt64_gbi_f3dex2.cpp:138-153`. The no-depth A/B produces recognizable oracle-positioned field geometry. | Ruled out for this artifact. |
| S/T / texture | fn64's `raw * scale / 32` matches RT64's `raw * scale / (65536*32)` where the scale is the raw 16-bit `G_TEXTURE` field (`rt64_rsp.cpp:727-733`). 96.1% of sampled task-249 texels are non-black, and the no-depth textured frame visibly contains field texture. | Not the cause of the black mask; TMEM fidelity remains partial. |
| Segment / display-list addressing | Task 249 observes 48 segment writes, 103 DL calls, and 9 branches. Raising the local depth cap from 10 to 64 recovers only six candidate triangles; all six fail the same near-plane gate and the default PNG is byte-identical. | Not this artifact. The cap should separately become F3DEX2's real 18-entry return stack. |
| Backface / near cull | Disabling backface culling removes 323 cull decisions but leaves the masked default PNG byte-identical; in the visible no-depth A/B it exposes an inside-out diagonal face. Disabling the near gate restores 406 behind-camera triangles and covers the frame with giant wrong-side polygons. | Existing gates are not over-culling the missing visible scene. Proper near clipping remains future work. |

## Separate localized decode defect

The investigation also found a real F3DEX2 light MOVE_MEM slot off-by-one.
Public `gbi.h:2867-2911` emits `gSPLight(LIGHT_1)` at byte offset
`1*24+24 = 48`, encoded as six eight-byte units. MIT RT64 divides the byte
offset by 24, reserves indices 0 and 1 for look-at vectors, and stores a light
at `index-2` (`rt64_gbi_f3dex2.cpp:36-55`). fn64 formerly used
`offset/24-1`, shifting every light one slot high. The corrected helper maps
wire value 6 to directional slot 0, and
`movemem_light_1_maps_to_directional_slot_zero` fails against the old mapping.

This correction changes the exposed diagnostic task from the old green-biased
lighting (`/tmp/fn64-oot-task249-clear-no-depth-no-texture-legacy-light.png`,
SHA-256 `d1f4fb8f80029720528ef480ca1a111b9284ef0c0066fbc006ddbff7d82bb7a0`)
to the coherent blue/gray result
(`/tmp/fn64-oot-task249-clear-no-depth-no-texture.png`, SHA-256
`1e7e04e791137caf7fdfaaa8c95a6270a16380404707e1625e098e0d2f8530f0`).
The default target frame remains masked and byte-identical, so this is not
misrepresented as the artifact fix.
