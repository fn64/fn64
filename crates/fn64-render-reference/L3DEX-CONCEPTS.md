# L3DEX / L3DEX2 Behavioral Concepts

> Clean-room concept spec derived from Nintendo's public `gbi.h`, the public
> `gSPLine3D` manual page, and Chapter 25 of the public N64 Programming Manual.
> No GPL runtime implementation was consulted.

## 1. Digest-typed family boundary

L3DEX and L3DEX2 are line microcodes with different public command envelopes.
L3DEX uses the older F3DEX layout; L3DEX2 uses `F3DEX_GBI_2`. Their opcode
bytes collide with each other and with polygon microcodes, so a command byte
cannot identify the family. In particular, `0x01` is legacy `G_MTX` but modern
`G_VTX`, and modern `0x05` is the `G_TRI1` form that line microcode documents
as a no-op while F3DEX2 uses it to emit a triangle.

The backend-neutral seam therefore names `UcodeId::L3dex` and
`UcodeId::L3dex2` separately. `GeometryUcodeCatalog` associates every admitted
SHA-256 text identity with exactly one `GeometryWireFamily`; registering one
digest under two families traps. `ReferenceBackend::with_geometry_ucode_sha256`
and `with_geometry_ucode_text` select the shared geometry decoder and preserve
that explicit family identity. An unknown task-entry image returns
`FrameStatus::NeedsLle` before task effects commit. A public `G_LOAD_UCODE`
continues HLE only when its resulting text digest is also in the catalog, and
switches the command envelope to that digest's registered family.

The RT64 adapter remains limited to its separately admitted F3DEX2 identities;
this Rust-reference support does not advertise L3DEX support on an upstream
path whose line dispatch remains incomplete.

## 2. Public line forms

The public [`gbi.h`](https://ultra64.ca/files/documentation/online-manuals/man/header/gbi.htm)
defines these exact encodings:

| Family | `G_VTX` | `G_LINE3D` | `G_ENDDL` |
|---|---:|---:|---:|
| L3DEX / F3DEX envelope | `0x04` | `0xb5` | `0xb8` |
| L3DEX2 / F3DEX_GBI_2 envelope | `0x01` | `0x08` | `0xdf` |

L3DEX packs the vertex destination as `v0 * 2` and its DMA length/count as
`(n << 10) | (sizeof(Vtx) * n - 1)`. L3DEX2 packs `n` in `w0[19:12]` and the
exclusive destination end `(v0 + n)` in `w0[7:1]`. Both line forms encode
their two cache slots as `v * 2`, followed by an eight-bit width parameter.
L3DEX places that payload in `w1`; L3DEX2 places it in the low 24 bits of
`w0` and reserves `w1` as zero.

The public [`gSPLine3D`](https://ultra64.ca/files/documentation/online-manuals/functions_reference_manual_2.0i/gsp/gSPLine3D.html)
contract gives the rendered width as `1.5 + wd * 0.5` pixels and encodes the
flat-shade selector by swapping the two endpoints. The decoder validates those
wire fields, normalizes both envelopes into one typed `RenderOp::Line`, and
uses the established line raster path for homogeneous clipping, shade/texture
attributes, scissor, sample coverage, blending, and read-only depth. The line
footprint retains the public eight-sample checkerboard identity. A partial
pixel evaluates smooth shade, perspective texture coordinates, and Z at the
same typed covered-sample point as the triangle paths; a full pixel remains at
pixel center. Equivalent legacy and modern command streams are regression-
tested to produce identical masks, selected points, typed endpoints, and
framebuffer bytes.

## 3. Admitted adjacent F3DEX forms

Legacy normalization is deliberately limited to layouts published by the
same header: 64-byte matrix DMA with the old projection/load/push flag values,
the 16-byte viewport `G_MOVEMEM`, vertex DMA, display-list call/branch,
clear/set geometry mode, end, other-mode ranges, texture selection, move-word,
modelview pop, and the public no-op. Published RDP command bytes `0xe4..=0xff`
retain their existing RDP mechanisms. These translations change only the
command envelope; matrix, vertex, segment, triangle-independent line, and RDP
state continue through the existing exact mechanisms.

Chapter 25 of the public Programming Manual describes L3DEX2 as the replacement
for L3DEX and documents the 32-entry vertex cache. The implementation therefore
accepts only cache slots `0..=31`. The `gSPLine3D` manual states that triangle
commands are not supported by line microcode: the published `G_TRI1` wire form
is validated and consumed as a no-op. `G_TRI2`, `G_QUAD`, reserved fields, odd
`v * 2` encodings, undocumented legacy commands, and out-of-range DMA shapes
trap with the family, opcode, and original command words. No opcode guessing or
silent polygon fallback exists.

## 4. Remaining fidelity frontier

The typed line primitive preserves the public width, endpoint order, clipping,
texture, combiner, blender, and depth contracts. Programming Manual Chapter
15.4 establishes that Z is subpixel-corrected onto the primitive, but does not
publish the representative-sample lookup or correction arithmetic. Lines
therefore share the reference renderer's explicit nearest-covered,
stable-order bounded policy; this is not a silicon centroid claim. Exact
microcode fixed-point transform rounding and the silicon line-edge coefficient
generator still require hardware-trace evidence. The public modern F3DEX2 Rej
variants are separately digest-typed as described in
[`F3DEX2-VARIANTS.md`](F3DEX2-VARIANTS.md). F3DZEX variants and other
game-specific microcodes remain unadmitted and take LLE or trap by name.
