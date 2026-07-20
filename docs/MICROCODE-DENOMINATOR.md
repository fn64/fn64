# Admitted Microcode Denominator

> Clean-room audit derived from Nintendo's public `gbi.h`/`gs2dex.h`, the
> public Fast3D, F3DEX, L3DEX, and S2DEX manuals, and fn64's typed decoder
> tests. No GPL runtime implementation was consulted.

## Geometry and line families

Geometry HLE is admitted by the SHA-256 of one complete 4 KiB IMEM text image.
The catalog distinguishes Fast3D, F3DEX, F3DLX, F3DLX.Rej, F3DEX2,
F3DEX2.NoN, F3DEX2.Rej, F3DLX2.Rej, L3DEX, and L3DEX2. Opcode bytes do not
select among those families. F3DZEX2 remains named but unadmitted because the
allowed public sources do not specify its continuation and branch wire.

Release recognition is exact but diagnostic/optimization evidence only. For
every graphics LLE generation, the ABI independently hashes the complete live
4 KiB IMEM image; the registered backend may attach a family from its runtime
catalog, but cannot choose the digest or replace LLE execution. Report schema
v16 binds those recognition events in order with committed IMEM replacements
and DRAM/XBUS DPC command digests. It rejects decreasing event cycles,
decreasing global IMEM generations, a replacement that does not strictly
advance the generation, or different digests assigned to one generation.

Matrix v5 does not trust the backend's family label for public-microcode
credit. It independently adjudicates each reported text digest through the
immutable, project-owned certified-public-microcode catalog v1, and rejects a
backend label that contradicts that adjudication. Catalog v1 is currently
empty pending allowed-source digest provenance, so no v13 matrix can yet
satisfy any of the twelve public-microcode requirements. Unknown digests,
F3DZEX2, and declared readiness families likewise receive no denominator
credit.

The public command-class denominator for the admitted polygon and line
families is represented. Remaining gaps are behavioral precision rather than
unknown ordinary opcode classes: exact transform/clipping/line-edge
fixed-point behavior, the unpublished historical Fast3D quadrangle wire, and
F3DLX/F3DLX2.Rej transformed-vertex pixel rounding. These cases trap where an
exact public rule is unavailable.

The public header defines two texture-rectangle continuations. The raw RDP
`gDPTextureRectangle` form appends one 64-bit coefficient word. The normal
display-list `gSPTextureRectangle` form instead emits family-specific
`G_RDPHALF_1` and `G_RDPHALF_2` commands: `0xb4/0xb3` for the legacy envelope
and `0xe1/0xf1` for the modern envelope. fn64 now decodes both forms through
one typed continuation mechanism selected by admitted task identity. The
exhaustive regression matrix covers all ten admitted families, normal and
flipped rectangles, both continuation forms, and signed 16-bit boundary
fields. Wrong-family, malformed, and truncated envelopes trap by family and
command name.

## Object families

S2DEX and S2DEX2 are separately digest-admitted because their public command
bytes collide. Their published object/background opcode classes are present:
rectangle, matrix-relative rectangle, rotating sprite, texture block/tile/TLUT
loads and compound load/draw commands, object matrix/submatrix, render mode,
background copy/scale, segment/status writes, conditional lists, and end.
Rotating sprites retain the public no-LOD tile pair when their ordinary
two-polygon RDP lowering selects TEXEL1, across both wire families and both
standalone and compound sprite commands; a missing tile 1 remains loud.

That opcode-class inventory does not imply unrestricted object arithmetic.
Integer point-sampled Copy backgrounds implement the public row-major
horizontal/vertical loop through one typed load/draw partitioner shared by both
wire families and load selectors. A separate fixed-point planner implements
point-sampled scaled wrapping, fractional horizontal origins, and distinct
integer `imageYorig` without importing Copy's partition assumptions. Vertical
subpixel or filtered scaled backgrounds; NOTXCLAMP footprints that leave the
source image; WIDEN with filter/flip/Copy; Average combined with WIDEN,
NOTXCLAMP, Copy, rotating polygons, or other unpublished neighbour footprints;
and historical XLU/AA revisions remain loud. Public material describes their
intent without providing enough intermediate rounding, scaled partition, or
spill rules for an exact implementation.
[`S2DEX-CONCEPTS.md`](../crates/fn64-render-rt64/S2DEX-CONCEPTS.md) is the
executable-support boundary.

## Next admissible work

Do not add a new named family from a game-era label or neighboring opcode
layout. The next microcode work must begin with either a complete allowed
public wire specification or clean-room black-box vectors that distinguish the
missing behavior. Until then, exact fixed-point and object-composition vectors
have higher value than another speculative family alias.
