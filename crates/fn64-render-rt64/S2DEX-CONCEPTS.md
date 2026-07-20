# S2DEX Behavioral Concepts

> Clean-room concept spec derived from Nintendo's public `gs2dex.h`, the
> public *S2DEX Microcode* manual, and the existing typed RDP operation model
> in this crate. No GPL runtime implementation was consulted.

## 1. Admission and family boundary

S2DEX is a separate GBI family. In the public `F3DEX_GBI_2` header its opcode
`0x01` means `G_OBJ_RECTANGLE`, while F3DEX2 assigns the same byte to `G_VTX`.
The renderer therefore never guesses the family from an opcode or decodes an
S2DEX task as F3DEX2.

The same public `gs2dex.h` assigns legacy S2DEX and S2DEX2 different,
colliding bytes:
legacy `0x01` is `G_BG_1CYC`, while S2DEX2 `0x01` is `G_OBJ_RECTANGLE`.
Legacy `gMoveWd` also uses opcode `0xbc` with `offset<<8|index`, while S2DEX2
uses opcode `0xdb` with `index<<16|offset`. Payload structures and the status
and segment equations are shared. The admission catalog associates every exact
text digest with `S2dexWireFamily::S2dex` or `S2dexWireFamily::S2dex2`, decodes
the family-specific opcode and move-word envelope, then feeds the same typed
payload/state mechanisms. An unknown digest returns `NeedsLle`; one digest
cannot be registered as both families. The decoder never guesses a family from
a colliding command byte.

`ReferenceBackend::with_s2dex()` selects the S2DEX decoder, but it does not
admit arbitrary IMEM. Each complete logical 4 KiB task-entry text image must
also be registered by SHA-256 and wire family through
`with_s2dex_ucode_sha256_for` or, for a synthetic fixture,
`with_s2dex_ucode_text_for`. The older methods remain explicitly S2DEX2. An
unlisted image returns `FrameStatus::NeedsLle` before task effects are
committed.

The backend-neutral seam reports `UcodeId::S2dex` and `UcodeId::S2dex2`
separately. `supported_ucodes()` returns exactly the wire families represented
by the backend's admitted digest catalog, rather than an experimental catch-all
tag.

## 2. Implemented object rectangle and texture-load slice

The public [`gs2dex.h`](https://ultra64.ca/files/documentation/online-manuals/man/header/gs2dex.htm)
defines the F3DEX_GBI_2 command as `G_OBJ_RECTANGLE = 0x01` and emits it with
`gDma0p(command, pointer, 0)`. The second word points to the public 24-byte
`uObjSprite` structure. The decoder accepts physical, KSEG0, KSEG1, and public
16-entry segmented pointers established by preceding `gSPSegment` writes.

The decoder reads every published field in command order:

| Offset | Field | Fixed-point / meaning |
|---:|---|---|
| 0 | `objX` | signed s10.2 screen X |
| 2 | `scaleW` | unsigned u5.10 S step |
| 4 | `imageW` | unsigned u10.5 texture width |
| 6 | `paddingX` | reserved, required zero |
| 8 | `objY` | signed s10.2 screen Y |
| 10 | `scaleH` | unsigned u5.10 T step |
| 12 | `imageH` | unsigned u10.5 texture height |
| 14 | `paddingY` | reserved, required zero |
| 16 | `imageStride` | TMEM row stride in 64-bit words |
| 18 | `imageAdrs` | TMEM origin in 64-bit words |
| 20–23 | format, size, palette, flags | public texture selectors |

The public [S2DEX sprite manual, section 4.2.3](https://ultra64.ca/files/documentation/online-manuals/man-v5-1/ucode/s2dex/04.htm)
states that the RSP converts this structure to an RDP texture rectangle. fn64
does exactly that at its typed seam: it programs render tile zero from the
sprite's TMEM fields, snapshots the persistent RDP other-mode/combiner/blender/
scissor state, and emits the existing `RenderOp::TextureRectangle`. The shared
rectangle executor supplies clipping, sampling, blending, and color-image
write-back; this slice does not add a parallel sprite rasterizer.

Supported behavior:

- non-rotating `G_OBJ_RECTANGLE`;
- public 24-byte `G_OBJ_MOVEMEM` `uObjMtx` loads and 8-byte `uObjSubMtx`
  updates;
- matrix-relative non-rotating `G_OBJ_RECTANGLE_R`;
- rotating `G_OBJ_SPRITE` using a full object matrix;
- normal, S-flipped, T-flipped, and S+T-flipped images through the public
  `G_OBJ_FLAG_FLIPS` / `G_OBJ_FLAG_FLIPT` bits;
- positive, whole-texel image dimensions;
- 1:1 or scaled, point-filtered one-cycle/two-cycle drawing, bilinear
  filtering when `G_OBJRM_BILERP` matches the RDP filter, and the public
  four-sample Average box filter without bilerp correction, including exact
  inward `G_OBJRM_SHRINKSIZE_1` / `G_OBJRM_SHRINKSIZE_2` correction on
  non-rotating rectangle paths and `G_OBJRM_NOTXCLAMP` when every addressed
  four-texel cell is proven wholly inside the source image;
- point-filtered non-rotating rectangles with `G_OBJRM_NOTXCLAMP`, including
  exact inward `G_OBJRM_SHRINKSIZE_1` / `G_OBJRM_SHRINKSIZE_2` and unflipped
  `G_OBJRM_WIDEN` combinations when every emitted point sample is proven inside
  the source image;
- copy-cycle drawing with the public no-X-scaling restriction;
- a texture already loaded in persistent TMEM or loaded by the object commands
  below;
- `G_ENDDL` with zero reserved fields.

The public header gives `gSPObjLoadTxtr` a 24-byte `uObjTxtr` payload and gives
the three compound commands a 48-byte `uObjTxSprite` payload. The decoder now
accepts all three documented texture structures:

- `G_OBJLT_TXTRBLOCK`: `(tsize + 1)` 64-bit words, destination `tmem`, and
  `tline` DXT stepping;
- `G_OBJLT_TXTRTILE`: `twidth`/`theight` macro outputs, destination `tmem`,
  and row-preserving loads;
- `G_OBJLT_TLUT`: `pnum + 1` RGBA16 entries beginning at high-TMEM `phead`.

Each is lowered through the existing raw-RDP `SETTIMG`/load-tile/TMEM
mechanism rather than a second texture store. Image alignment, physical RDRAM
ranges, TMEM bounds, macro field shapes, TLUT bounds, the reserved TLUT zero,
and public status IDs are checked before any state commits. Image sources are
strictly limited to the console's physical 8 MiB even when the host slice also
contains generated-C MMIO alias backing. Raw-RDP lowering rebases only the
admitted logical image bytes into one reusable 4,136-byte task-local scratch,
followed by its synthetic command tail. It therefore never copies the prefix
before a near-end image and preserves CI4 nibble order, native-word TLUT tails,
LoadBlock DXT row exchange, and LoadTile rows through the existing RDP loader.

Programming Manual section 4.5.2 specifies four status words and the exact
cache test `(Status[sid] & mask) == flag`; a miss updates the word to
`(Status[sid] & ~mask) | (flag & mask)`. The decoder applies that equation
with `sid` in `{0,4,8,12}`. Section 4.6.2 defines `G_OBJ_LDTX_RECT` as
`G_OBJ_LOADTXTR` followed by `G_OBJ_RECTANGLE`; fn64 applies the load to the
task-local RDP state before lowering the rectangle, then returns the established
`RenderOp::TextureRectangle`.

The decoder reads `gMoveWd` through the admitted family's public envelope:
legacy S2DEX uses `0xbc` plus `offset<<8|index`; S2DEX2 uses `0xdb` plus
`index<<16|offset`. `G_MW_SEGMENT` writes one of sixteen task-local 24-bit
bases at aligned offsets `segment*4`; later addresses resolve as
`base[segment] + low24(pointer)`. The same resolver is used for standalone and
compound object payloads, matrices, object texture images, background
payloads/images, and split conditional-list targets. KSEG0 and KSEG1 pointers
retain their public low-24 physical interpretation. Address addition overflow,
non-public segment bytes, final misalignment, and physical RDRAM escape remain
named traps.

`gs2dex.h` defines `G_MW_GENSTAT = 0x08` and `gSPSetStatus(sid,val)` through
each family's move-word form. fn64 accepts exactly status IDs `{0,4,8,12}` and
replaces the selected task-local word before subsequent texture-cache or
conditional-list tests. Segment and status mutations remain speculative with
the rest of the S2DEX task.

Manual sections 4.2.2, 4.2.4, and 4.3 define the object matrix wire fields and
the matrix-relative rectangle equations. The decoder accepts exactly the public
`gDma1p` forms `(parameter,length)=(0,23)` for `uObjMtx` and `(2,7)` for
`uObjSubMtx`. A full load replaces `{A,B,C,D,X,Y,BaseScaleX,BaseScaleY}`; a
sub-matrix changes only `{X,Y,BaseScaleX,BaseScaleY}`. `G_OBJ_RECTANGLE_R` uses
the documented position and group-scale transform. `G_OBJ_LDTX_RECT_R` follows
its public load-then-matrix-relative-draw sequence on the same speculative
state.

Manual section 4.2.5 defines rotating sprite coordinates as
`x'=A*x+B*y+X`, `y'=C*x+D*y+Y`, and states that the four-sided result is made
from two polygons. fn64 retains the s15.16 A/B/C/D values, transforms the four
s10.2 sprite corners, and emits two existing textured `RenderOp::Triangle`
operations sharing one diagonal. Texture coordinates cover the public
`imageW`/`imageH` texel extent, and the ordinary triangle rasterizer supplies
coverage, interpolation, combiner, blender, and scissor behavior.

Manual sections 4.2.3 and 4.2.5 assign `imageFlags` independently of the
rectangle or matrix path. Non-rotating rectangles reverse the relevant signed
S5.10 gradient and begin at the last texel center. Rotating sprites instead
reverse the S/T coordinates at all four transformed corners, so both triangles
retain one continuous flipped image across their shared diagonal.

`G_OBJ_SPRITE` requires a preceding full `uObjMtx`; a sub-matrix alone cannot
invent A/B/C/D. `G_OBJ_LDTX_SPRITE` applies its texture load and two-triangle
draw on task-local state, so a rejected draw or later command commits neither.
The admitted polygon path is currently point-filtered one-cycle/two-cycle and
preserves the public no-LOD tile pair for a TEXEL1-selecting two-cycle
combiner. Section 4.2.5 defines the rotating object as two polygons and gives
it the same texture settings as `G_OBJ_RECTANGLE`; Programming Manual chapter
12.6 defines TEXEL1 as the texture-map output from tile+1. Both S2DEX wire
families, standalone `G_OBJ_SPRITE`, and compound `G_OBJ_LDTX_SPRITE` feed the
same immutable two-tile triangle snapshot. A TEXEL1 program without an
initialized tile 1 remains a named loud error instead of aliasing tile 0.
Copy/fill cycles and depth still trap until their S2DEX-specific primitive
policy is represented.

S2DEX Microcode manual section 4.4.1, “gSPObjRenderMode”, defines
`G_OBJ_RENDERMODE` as task-local RSP correction state. fn64 retains clamp
policy, Point-or-Average versus Bilinear correction, composable shrink/widen
perimeter policy, and the two current-header ignored edge flags as typed state
rather than unrelated booleans. `G_OBJRM_BILERP` must match the RDP Bilinear
selector. The typed rectangle sampler already uses integer coordinates as
texel centers, which is the result of the RSP's documented half-texel bilerp
correction. Triangle interpolation is evaluated at screen-pixel centers, so
rotating sprites apply the equivalent `-0.5` S/T vertex correction.

The public
[`gDPSetTextureFilter` manual](https://ultra64.ca/files/documentation/online-manuals/man-v5-2/allman52/n64man/gdp/gDPSetTextureFilter.htm)
defines Average as an equal average of the four surrounding texels. It has no
S2DEX correction flag. Average therefore uses the existing shared RDP box
sampler with the Point-or-Average object state and ordinary perimeter clamp.
Average filtering and inward shrink remain separately typed stages. On
non-rotating rectangles, exact `G_OBJRM_SHRINKSIZE_1` and
`G_OBJRM_SHRINKSIZE_2` compose with Average across both wire families,
ordinary and matrix-relative commands, standalone and compound commands, and
all S/T flip combinations. The footprint validation distinguishes fully
interior four-texel cells from a positive-edge cell whose one-past neighbour
maps to the final texel through the retained public perimeter clamp. This is
not a claim that every raw neighbour is interior. Average combined with
`G_OBJRM_NOTXCLAMP` is admitted only where a typed monotonic-endpoint proof
establishes that both neighbours on both axes remain inside the source image
for every emitted sample. In that domain the public clamp switch is
observationally irrelevant; an out-of-domain footprint still traps. Average
combined with `G_OBJRM_BILERP`, `G_OBJRM_WIDEN`, Copy cycle, or a rotating
polygon remains loud because those neighbour or pixel-center rules are not
published. Reserved filters and mismatched Point/Bilinear state also trap.

The same “gSPObjRenderMode” section states that `G_OBJRM_SHRINKSIZE_1` removes
0.5 texel from each perimeter edge and `G_OBJRM_SHRINKSIZE_2` removes 1.0
texel, while the upper-left screen coordinate does not move. For perimeter
amount `p`, fn64 therefore samples the source domain `[p, image-p]` and reduces
only the positive screen extent by `2p/scale`. The equation is shared by
ordinary, matrix-relative, rotating, and compound object paths; the Average
composition is deliberately narrower and excludes rotating polygons. Because
the public input screen domain is s10.2, a rectangle shrink that does not land
on an exact quarter pixel remains loud instead of choosing an unpublished
rounding rule. The two shrink bits are mutually exclusive exactly as required
by the header. Copy-cycle shrink also remains loud because the public rectangle
section says Copy does not support subpixel processing.

The public object-mode manual gives `G_OBJRM_WIDEN` an exact 3/8-texel
expansion in the positive S and T directions. fn64 expands only the lower-right
screen edge and positive texture endpoint. It admits the operation when
`3/8 * 1024/scale` is exactly representable in the destination s10.2 screen
domain; otherwise it traps instead of choosing an RSP rounding rule. The
manual permits render-mode flags to be ORed and names only SHRINKSIZE_1 plus
SHRINKSIZE_2 as mutually exclusive. fn64 therefore composes shrink and widen
as independent typed corrections: source domain `[p,image-p+3/8]`, with the
positive screen extent reduced by `2p/scale` then expanded by
`(3/8)/scale`. Each correction must independently land on a quarter pixel;
unknown intermediate rounding cannot be hidden by a net cancellation. The
same mechanism feeds ordinary, matrix-relative, rotating, compound, S2DEX,
and S2DEX2 paths. Widen in Copy mode or with filtering/flips remains loud
because those modes require unpublished perimeter or positive-edge rules.

`G_OBJRM_NOTXCLAMP` disables the RSP's texture-perimeter clamp. The public
manual states the intent but not the tile-mask/TMEM spill sequence. fn64 admits
the flag for non-polygon Point or Average paths whose actual neighbour sets
need no spill rule. This includes exact inward `G_OBJRM_SHRINKSIZE_1` and
`G_OBJRM_SHRINKSIZE_2` for Point and safe-interior Average sampling, plus
unflipped Point WIDEN with or without either shrink:
section 4.4.1 permits the flags to be ORed and defines each perimeter amount.
Flag composability is not itself the admission proof. A typed emitted raster
sequence records its nonzero signed gradient, proves the sequence is monotonic,
and requires its first and last point texels to remain inside
`[0,imageW) x [0,imageH)`. Exact WIDEN geometry can therefore still reject when
it emits a one-past sample; for example, an aligned four-texel image at scale
512 rejects without shrink, while its exact SHRINKSIZE+WIDEN compositions stay
inside. Exhaustive tests enumerate every actual admitted sample for both wire
families, ordinary/matrix-relative and standalone/compound paths, exact scales,
and BaseScale 512/1024/2048; the non-WIDEN Point and safe-interior Average
classes additionally cover all flips. The Average proof exhausts quarter-texel
starts, signed gradients, screen starts, extents, and image widths against every
emitted four-texel cell, then exercises both wire families and every ordinary,
matrix-relative, standalone, and compound rectangle path. Enabling or disabling
clamp is observably identical for every admitted case and no spill rule is
invented. Bilinear, flipped-WIDEN, Copy-subpixel, rotating, or any out-of-domain
use remains loud until allowed evidence defines its addressing or edge
selection.

The current public `gs2dex.h` revision marks `G_OBJRM_XLU` and
`G_OBJRM_ANTIALIAS` as ignored. The decoder retains both requests in typed
task-local state but deliberately produces the same operation and framebuffer
as mode zero. Historical release notes describe older XLU/AA behavior and then
state that release 1.04 made both unnecessary; an older revision would require
its own digest-associated behavior identity before those bits could acquire an
effect.

Manual section 4.7.2 and the public `gs2dex.h` macros define conditional lists
as an adjacent `G_RDPHALF_0` / `G_SELECT_DL` pair. The decoder reconstructs the
split 32-bit target, validates status IDs `{0,4,8,12}`, and applies the exact
`(Status[sid] & mask) == flag` test. A false result updates the masked status
bits and either calls through the public 18-entry F3DEX_GBI_2 return stack or
tail-branches for `G_DL_NOPUSH`; a true result skips the target. Missing or
non-adjacent halves, invalid push selectors, misaligned/unresolved targets, and
stack overflow trap before live RDP state commits.

This HLE slice admits matrix-relative values only when fixed-point division,
multiplication, corner extents, and A/B/C/D transforms are exactly representable
in the destination quarter-pixel or u5.10 fields. Values requiring an
unpublished intermediate rounding rule trap with the axis and operands;
silicon-vector evidence must establish that rule before the decoder broadens
the accepted set.

The display-list decode, status words, and RDP-state update are speculative.
A malformed or unsupported later command leaves live state unchanged rather
than partially admitting a task that the HLE decoder could not complete.

## 3. Background strip loading

Public manual sections 4.1.1–4.1.3 define the shared 40-byte `uObjBg` union,
Copy-cycle `G_BG_COPY`, and scaled one-cycle `G_BG_1CYC`. Both commands are
implemented for positive whole-pixel dimensions and point filtering. Copy
requires integer origins. Scale admits the public
u10.5 fractional `imageX` and s10.2 fractional `frameX`, while `imageY` and
`frameY` remain whole-pixel because section 4.1.3 permits only horizontal
subpixel movement. Both modes implement the public row-major horizontal and
vertical closed loop. Horizontal `G_BG_FLAG_FLIPS` is represented by a
negative S gradient. Copy requires Copy cycle; Scale requires OneCycle.

Both public `imageLoad` selectors are retained. `G_BGLT_LOADBLOCK` loads full
source rows with public DXT stepping; `G_BGLT_LOADTILE` loads only the admitted
source rectangle. The decoder lowers each transfer through the existing raw
RDP `SETTIMG`/`SETTILE`/`LOADSYNC`/load mechanism, snapshots the resulting TMEM
tile, and emits an existing `RenderOp::TextureRectangle` before loading the
next strip. TMEM capacity is 512 64-bit words, or 256 for CI so high TMEM stays
available to the TLUT. A final partial-height strip emits its own remainder
rectangle rather than dropping lines.

Copy backgrounds use one typed integer-window partitioner for both S2DEX wire
families and both load selectors. Public section 4.1.2 defines the right-edge
successor `(imageW-1,n) -> (0,n+1)` and describes horizontally and vertically
looped closed areas. The partitioner applies that row-major topology exactly,
splitting only at source row/image edges or the initialized TMEM row capacity,
then gives the same validated slice to the loader and rectangle emitter.
Horizontal flip reverses S within those slices. Exhaustive small-domain vectors
cover every image/frame size, origin, flip state, and row-capacity choice; full
load/draw vectors cover legacy/current wires and LoadBlock/LoadTile. A separate
end-to-end matrix covers both wire families and both selectors on a real
six-row TMEM strip followed by a two-row remainder, asserting the exact two
rectangle bounds and pixels at every full/remainder boundary. This is evidence
for the already-shared partition mechanism, not a new arithmetic admission.
The caller must still submit an origin already wrapped inside the image, and
the transfer frame may not exceed the image, matching the manual's public
operating conditions.

Scaled point backgrounds use a separate typed fixed-point planner rather than
reusing Copy's one-texel partition assumptions. It retains u10.5 `imageX`,
u5.10 S/T gradients, the row carry produced when scaled S crosses `imageW`,
and the vertically wrapped sample identity in integers until rectangle
emission. Each zero-neighbour point slice is one output row and never crosses
a source-row edge, so its TMEM partition cannot create a filter seam. A
168,000-configuration exhaustive sweep reconstructs every emitted sample and
compares it with the direct fixed-point row-major equation. End-to-end vectors
cover both wire families, both loaders, normal/flip, fractional `imageX`, both
wrap axes, and distinct negative/equal/positive `imageYorig` values.

`imageYorig` is a separate signed s20.5 type, not an alias for `imageY`.
Section 4.1.3 explicitly requires callers to update those fields independently
while scrolling. Integer-valued distinct origins are therefore admitted and
retained. They are observationally neutral for the point footprint because a
sample has no neighbouring texels and every emitted slice is independently
exact. Fractional `imageYorig` remains loud because the public manual does not
publish its sub-plane boundary rounding.

All strips reuse one 8,240-byte task-local scratch. Each independently loaded
transfer stages only its admitted source rectangle at logical row zero, then
appends the five raw-RDP commands. LoadBlock is a one-dimensional transfer;
its low coordinate is S, so source-Y parity must not be encoded there. DXT
retains row exchange within a multi-row staged strip. A background image near
the physical 8 MiB boundary therefore has the same bounded staging cost as the
same strip near address zero.

`G_BG_COPY` validates all six CPU-derived `guS2DInitBg` fields against the
public `GS_PIX2TMEM` and `GS_CALC_DXT` formulas. Stale or zero-capacity fields
trap by command name. Image rows must satisfy the documented eight-byte
alignment, source coordinates must fit RDP tile fields, and the entire image
must remain inside physical 8 MiB RDRAM even when the host slice includes the
generated-C MMIO alias window. `G_BG_1CYC` also enforces the public physical
image-address floor of `0x1000`.

The current exact subset traps subpixel Copy origins, vertical scaled-subpixel
origins, fractional `imageYorig`, and bilinear/average-filter RSP correction.
Those cases need their public partition equations or silicon rounding evidence
represented before admission; they never silently clamp or approximate.

The background investigation distinguishes public intent from an executable
lowering rule. Section 4.1.3 publishes the point-sample scale and closed-loop
topology, and how callers maintain `imageYorig` independently while scrolling.
It also states that `imageYorig` is the scale and sub-plane division origin,
but does not publish the filtered strip-boundary equation that consumes it.
The manual does not specify bilinear neighbour texels, wrap-side filtered TMEM
partition, or RSP edge rounding. Admitting that footprint through ordinary
modulo/clamp or a generic `-0.5` shift would therefore guess the very seam
behavior `imageYorig` exists to correct.

## 4. Loud frontier

The remaining object-mode frontier is evidence-blocked rather than a local
unsupported-bit checklist. Filtered, rotating, flipped-WIDEN, or otherwise
out-of-domain NOTXCLAMP; WIDEN requiring sub-quarter-pixel rounding or combined
with flips/filtering; Average combined with WIDEN, Copy, rotating-polygon
correction, or an out-of-domain NOTXCLAMP footprint; and historical
revision-specific XLU/AA effects all trap with the command and missing
arithmetic. The public object manual states intent and perimeter amounts but
does not publish neighbour selection, fixed-point rounding, or rotated-edge
ownership for those combinations. Copy-mode subpixel perimeter processing is
not an implementation target: section 4.2.3 explicitly says Copy mode does not
support bilinear interpolation or subpixel processing and does not guarantee
proper operation. Applying those shapes as ordinary RDP state would omit or
invent the RSP's documented object correction.

The source boundary is explicit:

| Combination | What the public source settles | What it does not settle | Disposition |
|---|---|---|---|
| Average + safe-interior `NOTXCLAMP` | `gDPSetTextureFilter` defines the four surrounding texels; section 4.4.1 says `NOTXCLAMP` disables perimeter clamp | nothing observable when all four neighbours are interior | implemented by the typed endpoint proof |
| Average + out-of-domain `NOTXCLAMP` | the same two independent intents | tile-mask/TMEM spill addressing for an unclamped neighbour | hardware trace required; trap |
| Average or Bilinear + WIDEN | section 4.4.1 gives the 3/8-texel positive expansion and the half-texel Bilinear correction; the filter manual gives each filter's neighbour count | correction order, positive-edge neighbour ownership, and filtered endpoint command arithmetic | hardware trace required; trap |
| flipped WIDEN | object flags define S/T reversal; WIDEN names positive S/T | which screen edge receives the expansion after reversal and the resulting texture start/end pair | hardware trace required; trap |
| rotating Average/WIDEN | section 4.2.5 gives the affine corner transform and two-polygon shape | filter correction placement, shared-diagonal/perimeter ownership, and rotated outward-edge rounding | hardware trace required; trap |
| Copy + perimeter/subpixel correction | section 4.2.3 says Copy supports neither Bilinear nor subpixel processing and does not guarantee proper operation | no supported composition exists | permanently reject; not an implementation omission |
| inexact screen-quarter correction | public structures define s10.2 screen fields and the perimeter amount | RSP intermediate division and tie/negative rounding when the result is not an exact quarter pixel | hardware trace required; trap |
| vertical background subpixel motion | section 4.1.3 explicitly permits subpixel motion only horizontally | no supported vertical form exists | permanently reject; not an implementation omission |
| filtered/fractional-`imageYorig` background | section 4.1.3 identifies `imageYorig` as scale and sub-plane division origin and explains caller updates | filtered neighbour loads, wrap-side strip partition, and fractional boundary rounding | hardware trace required; trap |

The focused load tests use only synthetic bytes. They prove that block DXT row
exchange, tile row layout, and CI4 plus TLUT loading reach the same TMEM sampler
used by ordinary RDP commands. They are not a claim about unpublished RSP
timing. For backgrounds, section 4.1.3 allows horizontal subpixel motion but
explicitly excludes vertical subpixel motion, while `imageYorig` is also the
private sub-plane division origin. It publishes neither the filtered neighbour
partition nor the strip-boundary rounding equation. Filtered scaled
partitioning and fractional `imageYorig` therefore require hardware traces;
vertical subpixel movement remains rejected by the public contract. The next
local work is full-ROM reachability evidence, not guessed perimeter arithmetic.
