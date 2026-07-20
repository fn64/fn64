# F3DLX / F3DLX.Rej Behavioral Concepts

> Clean-room concept spec derived only from Nintendo's public `gbi.h`, public
> F3DEX-package function manuals, and Programming Manual Chapter 25. No GPL
> runtime implementation was consulted.

## 1. Digest identity selects execution policy

The public
[`gspF3DLP.Rej` package page](https://ultra64.ca/files/documentation/online-manuals/man/n64man/ucode/gspF3DLP.Rej.html)
states that F3DEX, F3DLX, F3DLX.Rej, and F3DLP.Rej are binary-compatible at
the GBI level. Binary compatibility does not make their execution behavior
identical: F3DLX changes vertex precision and clipping control, while
F3DLX.Rej replaces clipping with reject-box processing.

fn64 therefore associates each complete 4 KiB text SHA-256 with a distinct
`GeometryWireFamily::F3dlx` or `F3dlxRej`. The backend-neutral seam reports
`UcodeId::F3dlx` and `F3dlxRej` only for identities explicitly admitted with
those families. A digest registered under another family cannot be reused,
and no opcode guesses the execution policy. The RT64 adapter remains limited
to its separately admitted F3DEX2 identities.

## 2. Shared wire, different cache bounds

Both families normalize the published F3DEX-envelope matrix, move, vertex,
triangle, cull, branch, state, RDP, and load commands into the existing typed
geometry/RDP mechanisms. F3DLX uses the ordinary 32-entry F3DEX cache.

The public
[`gSPVertex` manual](https://ultra64.ca/files/documentation/online-manuals/man/n64man/gsp/gSPVertex.html)
gives F3DLX.Rej a 64-entry cache while retaining the legacy limit of 1--32
vertices per `G_VTX`. Multiple loads can therefore populate slots 32--63.
Triangle, modify-vertex, cull, and branch indices keep the same public `v*2`
wire representation and are validated against the digest-selected capacity.
An identical high-slot stream traps as F3DEX and executes as F3DLX.Rej.

The public package contract permits both families to self-load. They use the
legacy reset rule: RSP geometry, matrices, viewport, segments, and display-list
links reset, while independent RDP registers and TMEM remain live. A load from
a called list remains a named trap because the reset link cannot return.

## 3. Clipping and rejection

The public
[`gSPSetGeometryMode` manual](https://ultra64.ca/files/documentation/online-manuals/man-v5-2/allman52/n64man/gsp/gSPSetGeometryMode.htm)
limits the `G_CLIPPING` toggle to F3DLX/F3DLX.NoN and says it begins enabled.
fn64 retains that bit only for F3DLX; the same bit is loud under F3DEX and
F3DLX.Rej. The public
[`Introduction to N64` microcode summary](https://ultra64.ca/files/documentation/online-manuals/man/kantan/step2/4-3.html)
marks front/both culling unsupported for F3DLX as well, while the later
function page explicitly calls out Rej. fn64 conservatively leaves front/both
culling loud for both legacy F3DLX variants until that public-version conflict
can be tied to exact text identities.

F3DLX.Rej admits a triangle only when every transformed vertex lies within
the reject box. The public package page specifies X/Y rejection, far-plane
rejection, and deliberately no near-plane rejection. The public
[`gSPClipRatio` manual](https://ultra64.ca/files/documentation/online-manuals/functions_reference_manual_2.0i/gsp/gSPClipRatio.html)
defines `FRUSTRATIO_2` as the initial Rej value and permits ratios 2--6.
fn64 applies the retained per-side ratios in homogeneous coordinates before
emitting the typed triangle. One vertex outside X, Y, or the far plane rejects
the complete triangle; a vertex beyond the near plane does not do so merely
because of Rej policy.

## 4. Precision boundary remains loud

[Programming Manual Chapter 25](https://ultra64.ca/files/documentation/online-manuals/man-v5-1/pro-man/pro25/25-01.htm)
states that F3DLX simplifies F3DEX's subpixel vertex calculation to pixel
precision, but the public materials do not specify the exact fixed-point
rounding at negative or boundary coordinates. fn64 does not substitute a host
`floor`, truncation, or rounding guess. An F3DLX/F3DLX.Rej `G_VTX` with an
active transform traps at that named precision frontier. Raw screen-coordinate
fixtures still exercise the exact command envelope, cache limits, family
identity, triangle split, and raster mechanisms without claiming that missing
transform arithmetic.

The later F3DLX2.Rej family has its own modern command envelope, 64-vertex
single-load behavior, and the same unresolved pixel-precision arithmetic. See
[`F3DEX2-VARIANTS.md`](F3DEX2-VARIANTS.md); the two identities are never
interchanged merely because both use rejection and omit subpixel calculations.

## 5. Quadrangle evidence boundary

The public
[`gSP1Quadrangle` manual](https://ultra64.ca/files/documentation/online-manuals/functions_reference_manual_2.0i/gsp/gSP1Quadrangle.html)
specifies the current macro as two triangles. The public
[`gbi.h`](https://ultra64.ca/files/documentation/online-manuals/man/header/gbi.htm)
fully defines its F3DEX `G_TRI2` split and the F3DEX2 `G_QUAD` equivalent;
those two forms produce byte-identical raster output in the regression suite.

Chapter 25 also says F3DEX version 1.21 and earlier had a dedicated
quadrangle command that was later removed. The allowed public header does not
publish a dedicated base Fast3D opcode or a complete historical layout. fn64
therefore does not infer one from the later `G_TRI2`/`G_QUAD` encodings:
opcode `0xb1` under Fast3D remains a named unsupported-command trap. A dated
public header or hardware trace that completely specifies that wire form is
required to move this frontier.
