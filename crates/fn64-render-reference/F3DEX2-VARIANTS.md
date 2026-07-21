# F3DEX2 Variant Behavioral Concepts

> Clean-room concept spec derived only from Nintendo's public Programming
> Manual Chapter 25, public `gbi.h`, and public `gSPVertex`/`gSPClipRatio`
> manuals. No GPL runtime implementation was consulted.

## 1. Digest identity selects the variant

The public
[`F3DEX2 microcode chapter`](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro25/25-04.html)
distinguishes F3DEX2, F3DEX2.NoN, F3DEX2.Rej, and F3DLX2.Rej even where their
GBI command words are shared. fn64 therefore associates an exact 4 KiB text
SHA-256 with a distinct `GeometryWireFamily`. Opcode bytes never choose a
variant, and one digest cannot be registered under two families.

The backend-neutral seam reports `UcodeId::F3dex2`, `F3dex2NoN`,
`F3dex2Rej`, or `F3dlx2Rej` only after that exact identity is admitted. These
are Rust reference-lane claims; the RT64 adapter keeps its separate F3DEX2-only
admission boundary.

## 2. Public cache, load, and initial-state differences

Ordinary F3DEX2 and F3DEX2.NoN retain the public 32-entry vertex cache. The
public
[`gSPVertex` manual](https://ultra64.ca/files/documentation/online-manuals/man/n64man/gsp/gSPVertex.html)
gives both modern Rej variants a 64-entry cache and permits one command to load
1--64 vertices. The decoder validates the modern `G_VTX` exclusive-end wire
against the digest-selected capacity. A single 64-vertex load and high-slot
triangle execute under F3DEX2.Rej/F3DLX2.Rej and trap under ordinary F3DEX2.

Chapter 25 states that F3DEX2 changed the initial clipping ratio from one to
two. Every admitted F3DEX2 variant therefore begins at `FRUSTRATIO_2`; the
public per-side `gSPClipRatio` writes retain their existing typed behavior.
Modern Rej also permits front or both face culling under the modern geometry
mode envelope; the legacy Rej restriction does not leak across digest
identities.

## 3. NoN and Rej admission policies

F3DEX2.NoN disables near-plane clipping while retaining the other public clip
planes. fn64 currently models that distinction at its bounded reference
triangle admission gate: an otherwise valid triangle is not rejected merely
because a vertex crosses the near plane. Exact polygon subdivision and
silicon fixed-point clipping on the remaining planes are pre-existing trace
frontiers, so this is not a claim of complete clipper equivalence.

F3DEX2.Rej and F3DLX2.Rej replace clipping with whole-triangle rejection. The
public chapter and
[`gSPClipRatio` manual](https://ultra64.ca/files/documentation/online-manuals/functions_reference_manual_2.0i/gsp/gSPClipRatio.html)
specify the X/Y reject box, far-plane rejection, no near-plane rejection, and
initial ratio two. fn64 applies those tests in homogeneous coordinates before
emitting a typed triangle. One vertex outside X, Y, or far rejects the whole
triangle; a near-plane crossing alone does not.

F3DEX2.Rej retains the existing subpixel transform path. Chapter 25 says
F3DLX2.Rej omits subpixel vertex calculations but does not publish the exact
fixed-point rounding at negative or boundary coordinates. An F3DLX2.Rej
`G_VTX` that requires transformation therefore traps by name rather than
substituting host rounding. Raw screen-coordinate fixtures still prove its
wire, 64-entry cache, rejection policy, and raster handoff.

## 4. F3DZEX2 remains an explicit unsupported identity

The allowed public Nintendo materials above do not specify an F3DZEX2 command
envelope. Public
[`gbi.h`](https://ultra64.ca/files/documentation/online-manuals/man/header/gbi.htm)
defines the common `F3DEX_GBI_2` envelope but leaves `G_SPECIAL_1`,
`G_SPECIAL_2`, and `G_SPECIAL_3` reserved. The allowed MIT N64Recomp source
transports and recompiles RSP/game code; it does not define the proprietary
continuation and branch command semantics needed to decode this family.

fn64 names `GeometryWireFamily::F3dzex2` and `UcodeId::F3dzex2` so the frontier
cannot be mistaken for ordinary F3DEX2. Catalog admission, command decode, and
state initialization all trap with the missing-evidence requirement. In
particular, fn64 does not reinterpret a reserved `G_SPECIAL_*` opcode, alias an
F3DZEX2 digest to F3DEX2, or claim support from game-era naming alone. Closing
this boundary requires an allowed complete wire specification or a clean-room
hardware/black-box trace that distinguishes the family-specific commands.
