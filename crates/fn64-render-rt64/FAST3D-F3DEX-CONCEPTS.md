# Fast3D / F3DEX / F3DEX2 Wire Concepts

> Clean-room concept spec derived from Nintendo's public `gbi.h`, the public
> Fast3D/F3DEX function manuals, and Programming Manual Chapter 25. No GPL
> runtime implementation was consulted.

## 1. Identity selects the wire family

The three polygon microcodes are source-compatible through GBI macros but are
not binary-compatible. The public introductory manual explicitly states that
the F3DEX-series GBI and ordinary Fast3D GBI differ at the binary level. fn64
therefore associates each complete 4 KiB task-text SHA-256 with one of
`GeometryWireFamily::Fast3d`, `F3dex`, or `F3dex2`. A command byte never selects
the family, and one digest cannot be registered under two families.

`ReferenceBackend::with_geometry_ucode_sha256` and
`with_geometry_ucode_text` admit an exact identity and family together.
`supported_ucodes()` reports only the corresponding backend-neutral
`UcodeId::Fast3d`, `F3dex`, and `F3dex2` entries. Unknown task text returns
`NeedsLle` before live task state changes. The RT64 adapter's existing public
configuration remains explicitly F3DEX2-only.

This boundary is necessary even for opcode `0x01`: in the base and F3DEX
envelopes it is `G_MTX`, while in F3DEX2 it is `G_VTX`. The regression suite
uses one word that is simultaneously a valid legacy 64-byte projection-matrix
DMA and a valid F3DEX2 16-vertex load, proving that opcode inspection cannot
resolve the collision.

## 2. Published command layouts

The public [`gbi.h`](https://ultra64.ca/files/documentation/online-manuals/man/header/gbi.htm)
defines these relevant layouts:

| Operation | Fast3D | F3DEX | F3DEX2 |
|---|---|---|---|
| Vertex cache | 16 entries | 32 entries | 32 entries |
| `G_VTX` | `0x04`; parameter `((n-1)<<4)|v0`, length `16*n` | `0x04`; parameter `v0*2`, length `(n<<10)|(16*n-1)` | `0x01`; `n` in bits 19:12, exclusive end `v0+n` in bits 7:1 |
| `G_TRI1` | `0xbf`; `w1={flag,v0*10,v1*10,v2*10}` | `0xbf`; `w1` contains three `v*2` bytes, cyclically reordered for flat shade | `0x05`; the same `v*2` bytes occupy `w0[23:0]`, `w1=0` |
| Two triangles | not published for base GBI | `0xb1`; one `v*2` triple per word | `0x06`; one `v*2` triple per word |
| Quadrangle | no fully specified base-envelope opcode in this header | current F3DEX macro lowers to `G_TRI2` | `0x07`, decoded as the published two-triangle split |
| Cull | `0xbe`; start and exclusive end are 40-byte vertex-record offsets | `0xbe`; inclusive endpoints use `v*2` | `0x03`; inclusive endpoints use `v*2` |
| Branch on Z | not published for Fast3D | `0xb0`, preceded by `G_RDPHALF_1=0xb4` | `0x04`, preceded by `G_RDPHALF_1=0xe1` |
| Modify vertex | `G_MOVEWORD/G_MW_POINTS`, offset `v*40+where` | `0xb2`, index `v*2` | `0x02`, index `v*2` |
| Texture rectangle continuation | `G_RDPHALF_1/2=0xb4/0xb3` | `G_RDPHALF_1/2=0xb4/0xb3` | `G_RDPHALF_1/2=0xe1/0xf1` |

The [public F3DEX overview](https://ultra64.ca/files/documentation/online-manuals/man-v5-1/pro-man/pro25/25-01.htm)
describes F3DEX as Fast3D extended to a 32-entry cache with two-triangle
support. Current F3DEX emulates the removed historical quadrangle command with
that two-triangle form. fn64 does not invent a Fast3D quadrangle opcode from
the historical feature name; an undocumented byte remains a named trap.

All admitted triangle forms normalize to the existing typed triangle
operation. The flat-shade selector is represented by the published cyclic
vertex ordering, not retained as an invented side channel. Equivalent Fast3D,
F3DEX, and F3DEX2 display lists are tested to produce identical framebuffer
bytes.

## 3. Matrix, state, and line differences

Fast3D and F3DEX share the older matrix flags: projection `0x01`, load `0x02`,
push `0x04`. F3DEX2 uses projection `0x04`, load `0x02`, push `0x01` and XORs
the push bit in its DMA2 envelope. Legacy 64-byte matrix commands normalize
those flags before using the existing matrix stacks.

The old `G_MOVEWORD=0xbc` packs `offset<<8 | index`; F3DEX2 `0xdb` packs
`index<<16 | offset`. Segment, fog, clip, perspective normalization, vertex
modification, light count, and light-color writes are converted from their
published legacy offsets. The old light count
`0x80000000 | ((n+1)*32)` and 32-byte light-color stride become the F3DEX2
decoder's equivalent 24-byte forms. Legacy `G_MOVEMEM=0x03` viewport,
look-at, and 16-byte light destinations likewise normalize to their published
F3DEX2 offsets. Four-part legacy force-matrix insertion and other unmodeled
move destinations remain loud rather than fabricating partial state.

Legacy geometry bits differ too: smooth shading is `0x00000200`, cull front/
back are `0x00001000/0x00002000`; F3DEX2 uses
`0x00200000` and `0x00000200/0x00000400`. The normalizer translates only the
public modeled bits. F3DEX's optional clipping-policy bit remains loud because
the current typed geometry state always performs clipping and cannot honestly
represent that toggle.

The public header retains Fast3D's `G_LINE3D=0xb5` form with flag plus `v*10`
endpoints, so it normalizes into the established typed line mechanism. The
public [`gSPLine3D`](https://ultra64.ca/files/documentation/online-manuals/man/n64man/gsp/gSPLine3D.html)
manual says F3DEX-era `v*2` line commands execute only after loading L3DEX or
L3DEX2; F3DEX polygon-family opcode `0xb5` therefore traps with guidance to
load L3DEX instead of silently drawing.

## 4. Self-load and remaining frontier

The public [`gSPLoadUcode`](https://ultra64.ca/files/documentation/online-manuals/functions_reference_manual_2.0i/gsp/gSPLoadUcode.html)
manual allows F3DEX/L3DEX self-load but not Fast3D. It also states that legacy
loads initialize RSP segments, viewport, geometry, matrices, and display-list
link state. fn64 applies that legacy reset only at root-list depth; a load in a
called list traps because its reset link cannot return. F3DEX2 retains its
separate documented maintained-state behavior. In both cases an unadmitted
resulting text digest aborts speculative HLE and requests whole-phase LLE.

The public header exposes a raw two-word `gDPTextureRectangle` form and a
display-list-safe `gSPTextureRectangle` form whose coefficient word is wrapped
in the family-specific `G_RDPHALF_1/2` commands above. The decoder accepts both
documented forms, with the wrapper selected only by the admitted task digest;
wrong-family and malformed wrappers trap. Exhaustive vectors cover every
admitted geometry/line family, normal and flipped commands, both forms, and
signed coefficient boundaries.

Exact microcode fixed-point transforms, clipping subdivision and rounding, and
the unpublished pre-current dedicated quadrangle binary remain outside this
bounded tranche.
F3DLX/F3DLX.Rej's separately typed bounded support and precision frontier are
specified in [`F3DLX-CONCEPTS.md`](F3DLX-CONCEPTS.md). Unknown forms are not
inferred from neighboring encodings.
