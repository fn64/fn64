# RT64 S2DEX2 object-rectangle evidence

This evidence covers one public compound S2DEX2 command, not the S2DEX family
as a whole. The clean pinned RT64 source at
`f0728a2520d5aa735886240de3fee75cc805f6d6` loads the texture prefix for
`G_OBJ_LDTX_RECT` and then asserts before drawing. fn64 stages a generated copy
of that exact MIT source, guarded by full-file SHA-256
`7c8e779092eb7e2ddc8794694c0c04176a3e29958dc288952d577750673acc3f`.
The external checkout remains untouched and must report `GitClean`.

## Bounded contract

The overlay admits only:

- the active S2DEX2 GBI family and its `G_OBJ_LDTX_RECT` wire command;
- the exact public `G_OBJLT_TXTRBLOCK` type `0x00001033`, public structure ID
  offsets 0/4/8/12, and a descriptor whose `tsize`, `tmem`, `tline`, stride,
  extents, TMEM span, public four-bit segment encoding, resolved
  8-byte-aligned block source, and complete physical-RDRAM span all agree
  before mutation;
- the exact public 48-byte compound DMA encoding (`w0` low 24 bits `0x2f`),
  with public four-bit segment encoding, resolved 8-byte alignment, and its
  complete physical-RDRAM span checked before the first byte is read or the
  persistent structure buffer can expose a stale tail;
- axis-aligned, 1:1, whole-texel rectangles;
- point filtering in one-cycle mode with tile LOD, clamp detail (neither detail
  nor sharpen), no TLUT, no texture perspective, no S/T flips, palette zero,
  and object render mode zero;
- public RDP tile stride/address ranges.

Validation precedes the texture load. Both the texture prefix and sprite tail
are copied into initialized, trivially-copyable local public-layout structures
with `memcpy`; compile-time lifetime, size, and `offsetof` checks guard the
decoded fields without dereferencing a typed object in byte storage.
Legacy S2DEX wire admission, standalone
rectangles, sprites, matrix-relative rectangles, scaling, filtering, flips,
tile/TLUT loads, other formats, and object render-mode corrections retain a
named `G_OBJ_LDTX_RECT unsupported by bounded fn64 slice` failure instead of a
silent approximation.

The fixed-point structure fields and compound load-then-draw order come from
the public `gs2dex.h` interface and S2DEX manual sections 4.2.3 and 4.6.2, as
already cited and implemented independently in
`crates/fn64-render-reference/S2DEX-CONCEPTS.md`.

## Causal gate

`rt64_s2dex_object_rect_behavior` uses no ROM bytes or private input. In every
fresh process it:

1. proves the synthetic S2DEX transport cannot bypass production digest
   admission (`process_task` returns `NeedsLle` for an unknown image);
2. runs `G_OBJ_LOADTXTR` alone and proves the guarded target remains all zero;
3. runs the compound load/draw with an asymmetric 4x2 texture at nonzero screen
   origin and captures downstream load/workload telemetry;
4. independently encodes the equivalent raw RDP texture load, render tile, and
   texture rectangle into a separate guarded target;
5. requires byte-identical RDRAM targets and byte-identical Metal post-VI BGRA8
   captures; and
6. requires exact named rejection for an S flip, an upper-bit lookalike block
   type, inconsistent `tsize`, unaligned/out-of-RDRAM/non-public-segment block
   sources, unaligned/out-of-RDRAM/non-public-segment compound sources, a short
   DMA with a deliberately stale valid sprite tail, bilerp, texture LOD,
   sharpen, detail, TLUT, texture perspective, and the legacy S2DEX `0xC3`
   transport;
7. binds the successful path to exact workload IDs 2/3, present IDs 1/2,
   framebuffer-pair, sync-pair, valid-tile, CPU-upload, load, distinct/base/
   offset-source, and triangle multiplicities plus exact route and content
   digests; and
8. after every rejection, runs the fused path again and requires exact
   workload/present IDs 4/3, route/content identities, target/post-VI digests,
   and production `NeedsLle` admission.

`G_TP_NONE` is included because pinned RT64's texture sampler branches on
`textPersp`; `G_TL_TILE`, `G_TD_CLAMP`, and `G_TT_NONE` similarly exclude its
LOD/detail/TLUT branches. Conversion modes are not part of this bounded mode
gate because the admitted tile is RGBA rather than YUV. Other combiner/blender
state remains the independently encoded raw-RDP control's responsibility.

The stable exact outputs are:

- RDRAM target SHA-256:
  `dd1694195986db0ca633c44727c0bf23f76e3feb1810b19f3b8799b6efab9c6a`;
- post-VI BGRA8 SHA-256:
  `394924cd4165863fbb78e503486bcba6291f8994931beb08d8d666a114b79bef`.

The generated adapter identity for this run is
`fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-dither-filter:v1+vi-divot:v1+vi-silhouette-aa:v1+vi-retrace-cadence:v1+rdp-alpha-dither:v1+rdp-shared-fragment-noise:v1+s2dex-object-rect:v3`,
with exact adapter-source SHA-256
`6ec2849acf1b4d129f290f0f1dee996140bf16048494a15a8aa44298fd751ed5`.

On 2026-07-24 in America/New_York the v3 gate passed 10/10 fresh processes on
the recorded macOS 26.5 arm64 Apple M5 Pro host. Every process retained exact
workloads 2/3/4, presents 1/2/3, route digest `74388d653ac3227f`, final content
digest `28cb374eedfe64b3`, adapter and output hashes above, and all named
negatives. The processes used the clean exact pin and Metal
`metal-bgra8-unorm` release identity, with ten seconds between processes to
match the macOS certification runner's hidden-window teardown discipline.

This removes one native command hard stop and advances the open
`base-rendering-accuracy` inventory row. It does not close that row, certify a
representative S2DEX ROM, or establish silicon behavior outside the bounded
public contract.
