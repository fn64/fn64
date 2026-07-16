# OoT task-249 render-artifact diagnosis

Status: root-caused on `fix/render-artifact` (2026-07-16); selectively ported
after the RDP shading pipeline landed. The diagnostic branch intentionally
shipped no scene-specific render workaround.

## Result

The red/black Hyrule Field artifact was localized to depth policy, not to a
second projection error or an all-black texture decode. At the time of the
experiment, fn64 ignored `G_SETOTHERMODE_L/H` while comparing and updating
depth for every F3DEX2 triangle. Bypassing that hardwired policy exposed the
moon, field, road, and foreground geometry in the same screen regions as the
black-box emulator oracle.

Current main now decodes and snapshots other-mode, alpha compare, combiner,
blender, and texture-format state. The remaining depth integration is exact,
not heuristic: the raster path must use each triangle's `Z_CMP` and `Z_UPD`
bits independently. `OOT_NO_DEPTH` and per-task clears remain diagnostic tools,
not fixes.

## Oracle and decisive A/B

The C-file boot used OoT-generated `RecompiledFuncs`, clean MIT N64Recomp
headers, and the decompressed OoT NTSC 1.0 ROM outside git. With
`OOT_MAX_SWAPS=250 OOT_SKIP_AUDIO_UCODE=1`, graphics task 249 emitted 900
triangles. `OOT_RENDER_DUMP_LIMIT=260` includes that task in the PNG capture.

The black-box Mupen64Plus frame 250 showed a dark Hyrule Field/title-demo view
with the moon near the upper-right and field geometry across the lower half
(`/tmp/oot-reference-frame250.png`, SHA-256
`e65c1e86d2e7b2df00bcb54acfc305f20503b5a1d385c718afa6d8768786e210`). The
pre-other-mode fn64 task-249 frame had red side/background regions and a nearly
black center (`/tmp/fn64-oot-task249-baseline.png`, SHA-256
`808533fe319f6fc2801d37e6b99589caf05711cee1ea599d20e42fb16fa42bd1`).

Clearing prior-task color/depth history left task 249 uniformly red. Disabling
texture also left it uniformly red. With the same clear and `OOT_NO_DEPTH=1`,
the oracle-positioned geometry appeared
(`/tmp/fn64-oot-task249-clear-no-depth.png`, SHA-256
`753c85d3c7c183b26bb658f24035a8e68b47a361b398a1ca5b84ff976bc9e283`).

## Wire and implementation evidence

- Public `gbi.h:498-503` assigns render mode to other-mode low beginning at
  bit 3; `gbi.h:593-609` defines independent `Z_CMP` (`0x10`) and `Z_UPD`
  (`0x20`) bits. OoT alternates setup lists with and without z-buffering
  (`refs/oot-decomp/src/code/z_rcp.c:24-43`).
- MIT RT64 updates masked other-mode state
  (`third_party/rt64/src/gbi/rt64_gbi_f3dex2.cpp:24-33`,
  `hle/rt64_rsp.cpp:1026-1037`) and derives compare/write independently from
  `zCmp()` and `zUpd()` (`shared/rt64_other_mode.h:82-99`,
  `render/rt64_raster_shader.cpp:220-232,309-319`).
- The old fn64 path recognized `0xE2/0xE3` only by name and then routed every
  triangle through an unconditional compare-and-update path. Current main
  carries the decoded bits per triangle; the fragment write policy is the
  remaining seam.
- Task 249 sampled 132,001 texels, 126,913 non-black. OoT also selects
  incompatible combine modes in adjacent setup lists (`z_rcp.c:10-95`), so
  hardwired `texel * shade` was a separate fidelity failure. The integrated
  combiner replaces that old color-source path.

## Hypotheses falsified or bracketed

| Candidate | Task-249 evidence | Conclusion |
|---|---|---|
| Vertex color vs signed normal | 1,832/2,164 vertices took lighting; zero computed lit colors were black. Raw CN produced the expected rainbow-normal diagnostic, while lighting produced coherent blue/gray shading. | Not the red/black mask. |
| `G_VTX` | Public packing and MIT RT64 use the same `n`, `v0+n`, and 7-bit triangle indices. The no-depth A/B produced recognizable geometry. | Ruled out for this artifact. |
| S/T / texture | 96.1% of sampled texels were non-black, and the no-depth frame visibly contained field texture. | Not the black mask; TMEM fidelity remains partial. |
| Segment / DL addressing | Task 249 saw 48 segment writes, 103 DL calls, and 9 branches. Raising the local depth cap recovered only six near-plane-rejected candidates and left the default PNG unchanged. | Not this artifact. |
| Backface / near cull | Disabling backface culling left the masked frame unchanged; disabling the near gate restored wrong-side giant polygons. | Existing gates were not hiding the scene. |

## Separate localized decode defect

The investigation also found a real `G_MOVEMEM` light-slot off-by-one. Public
`gbi.h:2867-2911` emits `gSPLight(LIGHT_1)` at byte offset 48, encoded as six
eight-byte units. MIT RT64 reserves indices 0 and 1 for look-at vectors and
stores the light at `offset / 24 - 2` (`rt64_gbi_f3dex2.cpp:36-55`). The old
`offset / 24 - 1` mapping shifted every light one slot high. Regression
`movemem_light_1_maps_to_directional_slot_zero` locks wire value 6 to slot 0.

That correction changed the exposed diagnostic from green-biased lighting
(SHA-256 `d1f4fb8f80029720528ef480ca1a111b9284ef0c0066fbc006ddbff7d82bb7a0`)
to coherent blue/gray lighting
(SHA-256 `1e7e04e791137caf7fdfaaa8c95a6270a16380404707e1625e098e0d2f8530f0`).
It is a genuine decode fix, but not the depth-policy fix.
