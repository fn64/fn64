# RT64 texture-LOD scale enhancement evidence

Status: causal synthetic-HLE behavior proven on the certified Metal host; the
broader platform, recognized-game, and base-renderer silicon rows remain open.

## Public mechanism

Pinned MIT RT64 exposes
[`EnhancementConfiguration::TextureLOD::scale`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/common/rt64_enhancement_configuration.h#L40-L45).
For a draw using public `G_TL_LOD`, its workload builder sets the shader's
`upscaleLOD` flag only when that live setting or the Extended-GBI override is
active ([`rt64_state.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_state.cpp#L977-L984)).
The raster shader otherwise multiplies the LOD derivative by the framebuffer
resolution scale; the enabled flag retains scale 1.0
([`RasterPS.hlsl`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/shaders/RasterPS.hlsl#L126-L131)).
This is an optional host enhancement, not compiled game state.

The fixture uses only public F3DEX2/RDP operations: two RGBA16 mip tiles,
`G_TEXTURE` with one level after primitive tile zero, `G_TL_LOD`, and one
textured triangle at manual 2x resolution. The base tile is solid red and the
lower mip is solid green. Its texture derivatives straddle the exact
factor-of-two policy boundary: hardware-relative scaling selects the green
lower mip, while display-relative scaling selects the red base mip.

## Measured result

[`rt64_texture_lod_scale_behavior.rs`](../crates/fn64-render-rt64/examples/rt64_texture_lod_scale_behavior.rs)
creates one pinned Metal backend and switches `textureLOD.scale` off, on, then
off again through the live enhancement-policy path. Every phase submits the
same hand-authored, non-ROM display list through the non-default
`synthetic-f3dex2-evidence` transport and captures exact post-VI BGRA8 bytes.
Normal `process_task` must return `NeedsLle` before and after that interval, so
the evidence-only dialect substitution cannot leak into production microcode
recognition.

The first submission has a separate stable 290-green-pixel warm-up identity.
The gate binds that SHA-256 (`a9f09450...`) explicitly, then takes the
subsequent stable 259-pixel off/on/off sequence; it does not mistake the first
submission's different footprint for the enhancement effect.

Ten consecutive fresh processes passed on 2026-07-20 using clean pinned RT64
on macOS 26.5 arm64 / Apple M5 Pro:

| Phase | Active-policy SHA-256 | Post-VI SHA-256 | Exact color |
|---|---|---|---|
| scale off | `7a426cc2a30f5b5f16bf356996e65591934bf617363411898bc8d72b5558baa5` | `254d73f02da9dfed4700f700b6af553d41ef7f4e680a793eaacaf2ae04b0e22c` | 259 green pixels, zero red |
| scale on | `25ac93b536bcfc3b7b07094106d44d5f2cf5ee988931fb44b5985b30beb6fc3b` | `cd42bc830ce59f02afd8734cc2d8b6dec6f0dd459f23b33473cfaec88203b2f6` | 259 red pixels, zero green |
| scale off restored | exact first-phase identities | exact first-phase identity | exact first-phase colors |

This closes the local texture-LOD control/effect gap. It does not prove N64
silicon LOD rounding, recognize a game microcode, certify S2DEX controls, or
close any cross-platform or full-ROM row.
