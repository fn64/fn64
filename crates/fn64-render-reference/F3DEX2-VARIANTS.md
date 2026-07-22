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

## 4. F3DZEX2 remains an explicit unadmitted identity

The public Nintendo materials above do not specify an F3DZEX2 command
envelope. Public
[`gbi.h`](https://ultra64.ca/files/documentation/online-manuals/man/header/gbi.htm)
defines the common `F3DEX_GBI_2` envelope but leaves `G_SPECIAL_1`,
`G_SPECIAL_2`, and `G_SPECIAL_3` reserved. The allowed MIT N64Recomp source
transports and recompiles RSP/game code; it does not define the proprietary
continuation and branch command semantics needed to decode this family.

Pinned MIT RT64 does publish a software-parity definition in
`src/gbi/rt64_gbi_f3dzex2.cpp` and identifies three NoN variants in
`src/gbi/rt64_gbi.cpp`: F3DZEX2 replaces opcode `0x04` with `BranchW` and the
2.08I/J variants enable point lighting. That is an allowed RT64 parity source,
not a Nintendo specification or hardware-exact oracle. fn64 binds those raw
text/data fingerprints only to backend-neutral identity; it has not validated
all family-specific operations independently, so HLE remains unadmitted.

Ordered HLE admission nevertheless retains the behavior-bearing identity
explicitly. `TaskAdmissionUcode::F3dzex2` requires one of the three classified
variants; a broad `UcodeId::F3dzex2` cannot construct an executable task plan.
The canonical plan-v2 identity and native schema-v2 wire bind variant tags 1,
2, and 3 respectively. Native preflight additionally requires NoN for all
three, point lighting disabled for 2.06H, and point lighting enabled for
2.08I/J. The raw text/data pool remains the authority that distinguishes I
from J because pinned RT64 exposes identical native behavior flags for those
two rows. This closes identity collapse only; it does not open production
admission.

The common BranchW control-flow delta is now implemented independently in the
backend-neutral transactional inspector and deterministic reference decoder.
Both retain the transformed homogeneous W at `G_VTX`, decode the seven-bit
slot from bits 1..7, apply pinned RT64's strict `W < float(u32 threshold)`
condition (including integer-to-float rounding), validate the vertex even for
a forced branch, require persistent HALF_1 only on a taken path, and
resolve/align the segmented tail target. Adversarial tests keep opcode `0x04`
as ordinary BranchZ under F3DEX2, exercise loaded slot 126 and unloaded slot
127, preserve W across screen-coordinate modification, and trap non-finite W
or an invalid target extent. This is pinned RT64 software behavior, not a
hardware-exact BranchW claim.

fn64 names `GeometryWireFamily::F3dzex2` and `UcodeId::F3dzex2` so the frontier
cannot be mistaken for ordinary F3DEX2. Catalog admission still traps with the
missing-evidence requirement. `GeometryUcodeProfile` now carries the exact
behavior-bearing admission identity through the shared inspector and reference
decode state, including catalog-admitted self-load transitions, without
allowing a broad F3DZEX2 family to construct a profile. All three typed
F3DZEX2 variants apply the same bounded NoN near-admission policy already used
for public F3DEX2.NoN, while ordinary F3DEX2 retains its near rejection. Tests
also prove that side/far clip codes remain the same raster-clipping handoff;
this is not a claim of exact polygon subdivision, fixed-point clipping, or
silicon boundary behavior.

The internal decoder therefore covers the tested BranchW and bounded NoN
slices, but it does not reinterpret the remaining reserved `G_SPECIAL_*`
opcodes, alias an F3DZEX2 digest to F3DEX2, or claim point-light behavior from
the variant capability flag alone. An opt-in RT64 characterization transport
now derives the exact variant and logical entry generation from locally
supplied raw recognition windows, then reuses the production native
plan/result validation and guest-memory rollback boundary. It is intentionally
entry-only, is not reachable through `RenderBackend`, and has not yet produced
a private native point-light vector. Closing the RT64-parity HLE boundary still
requires running that black-box characterization and implementing the measured
2.08I/J point-light records and arithmetic, exact raw-pair resolution across
F3DZEX2 self-loads, native pixel differentials, and repeated representative
full-task evidence. A hardware-exact or Nintendo-public claim separately
requires hardware/black-box evidence or a public specification.

The backend-neutral identity classifier hashes the larger raw task text/data
prefixes with pinned RT64's XXH3 rows before rspboot or LLE mutates state. A
matching intersection supplies `UcodeId::F3dzex2`; a contradictory backend
pair label traps. This independently binds the family written to recognition
evidence, but it is still software-parity identity rather than HLE admission.
The task executes through the RSP interpreter, and the identity earns no
public-microcode coverage credit.
