# F3DEX2 / RDP Behavioral Concepts

> Clean-room concept spec: F3DEX2/RDP behavioral semantics from the public
> GBI contract + N64 hardware model; no code copied from any implementation.

This document is a behavioral reference for the F3DEX2 graphics microcode as
used by *The Legend of Zelda: Ocarina of Time* (NTSC 1.0). It describes **what
each display-list command does to N64 RSP/RDP state** and **the transform +
rasterization math that turns loaded vertices into on-screen triangles**, so
that `crates/fn64-render-rt64/src/gbi.rs` (the DL decoder) and `raster.rs`
(the software rasterizer) can be filled in correctly.

Everything here is described from the hardware/GBI *contract* (command
encodings, fixed-point formats, the N64 memory map, RDP pixel math). Those are
facts, not any one implementation's expression.

- **Facts described:** opcode bytes, argument bit layouts, fixed-point
  matrix/vertex formats, the transform pipeline, segmented addressing, RDP
  edge/coverage/z math, RGBA5551 framebuffer packing.
- **Priority:** the vertex + matrix + triangle path produces visible geometry;
  the implemented texture/color-combiner layer makes that geometry carry the
  material colors selected by OoT's display lists.

Section 0 gives the two orienting facts (word format + memory map); sections
1-6 follow the task's structure; section 7 is the cross-reference to what
`gbi.rs` already names; section 8 is the minimal-command-set + top risks.

---

## 0. Orientation: the command word, and the N64 memory model

### 0.1 The `Gfx` command word

A display list is a packed array of **8-byte commands**, each two big-endian
32-bit words. Call them `w0` (bytes 0-3) and `w1` (bytes 4-7). Both are read
big-endian out of RDRAM.

```
w0 = [ opcode:8 | ...payload... :24 ]
w1 = [ ...payload... :32 ]
```

- The **opcode is the top byte of `w0`** (`opcode = w0 >> 24`).
- The remaining 24 bits of `w0` plus all of `w1` carry per-opcode arguments.
- **F3DEX2 opcodes fall in two ranges:** the RSP geometry ops use *low*
  bytes `0x00`-`0x08` (e.g. `G_VTX=0x01`, `G_TRI1=0x05`), and the RSP state /
  RDP-passthrough ops use *high* bytes `0xD5`-`0xFF` (e.g. `G_MTX=0xDA`,
  `G_ENDDL=0xDF`). This split is a defining difference from the older F3DEX
  (where geometry ops were high bytes too); getting the byte values right is
  the whole ballgame for even reaching the geometry.

**Bit-field convention used below.** For a word `W`, write `field(W, shift,
width)` for "the `width`-bit unsigned field whose least-significant bit is at
`shift`", i.e. `(W >> shift) & ((1 << width) - 1)`. The public GBI packs each
opcode's arguments at fixed `(shift, width)` positions; those positions are
what an implementation must match, not any particular extraction helper.

### 0.2 The N64 memory map / segmented addressing (critical)

The RSP does not see 32-bit CPU virtual addresses. It sees:

1. **Physical RDRAM offsets.** RDRAM is 4 MiB (8 MiB with the Expansion Pak).
   A "physical address" here is just an index into the `rdram: &[u8]` slice.
2. **CPU KSEG addresses.** OoT builds pointers as `0x80xxxxxx` (KSEG0,
   cached). To turn one into an RDRAM offset, **mask off the top bits and keep
   the low 24-ish bits** (`& 0x00FFFFFF` is the practical mask for a 4 MiB
   image; `& 0x03FFFFFF` for 8 MiB). Never dereference `0x80xxxxxx` as a slice
   index.
3. **Segmented addresses.** The RSP keeps a **16-entry segment base table**
   (`segments[0..16]`, each a physical RDRAM base). A "segmented pointer" is
   `0xSSoooooo` where `SS` is the segment number in the top byte and the low
   **24 bits** are the offset within that segment. Resolution is:

   ```
   seg    = (ptr >> 24) & 0x0F        // segment number (0-15)
   offset = ptr & 0x00FF_FFFF         // 24-bit offset
   physical = segments[seg] + offset  // index into rdram
   ```

   **Segment 0 is conventionally the identity/physical segment** (base 0), so
   an unset segment resolves to its own low-24-bit offset — which is why a
   raw physical pointer with a `0x00` top byte "just works". OoT sets up
   segments for scene/room/object data at load time via `G_MOVEWORD` /
   `G_MW_SEGMENT` (see 1.3). **Every pointer argument that names RDRAM data
   (vertex arrays, matrices, nested DLs, texture images) is segmented and
   must go through this resolution.** This is failure risk #1 (section 8).

---

## 1. Display-list execution model

The RSP walks the command array sequentially, advancing 8 bytes per command,
dispatching on the opcode byte, until an `G_ENDDL` pops the call stack empty.

### 1.1 `G_DL` — call/branch into another display list (opcode `0xDE`)

Invokes a nested display list (the DL "call"/"jump" instruction).

| field | location | meaning |
|-------|----------|---------|
| push flag | `field(w0, 16, 8)` (in practice bit `0x01` of that byte) | `0` = **push** (call): save current DL pointer on the return stack, then jump. Non-zero (`G_DL_NOPUSH`) = **branch/tail**: jump without saving (the current DL does not resume). |
| target | `w1` | segmented address of the child DL (resolve via 0.2). |

- **Call stack.** The RSP maintains a small DL return-address stack (**18
  entries deep in F3DEX2**; the older number "10" is F3DEX/F3D — F3DEX2
  raised it). A pushing `G_DL` pushes the address of the *next* command;
  `G_ENDDL` pops it. A non-pushing `G_DL` replaces the current stream and
  never returns to the caller.
- **State is NOT saved/restored across `G_DL`.** The matrix stack, segment
  table, geometry mode, vertex cache, etc. are global RSP state; a child DL
  sees whatever the parent left and can mutate it for the parent. The only
  thing the call stack holds is the *return command pointer*. (A decoder that
  push/pops the matrix on `G_DL` to "be safe" is modeling something the
  hardware does not do — matrix save/restore is exclusively `G_MTX` push +
  `G_POPMTX`. See failure risk #3-adjacent in section 8.)
- A bound on recursion depth is a sane guard against a corrupt/cyclic DL, but
  the real limit is the return-stack depth (18), not an arbitrary number.

### 1.2 `G_ENDDL` — end / return (opcode `0xDF`)

Stops the current command stream. If the DL return stack is non-empty, pop and
resume at the saved address (return to caller). If empty, the whole DL is
done. Payload words are ignored.

### 1.3 `G_MOVEWORD` — write one 32-bit word into RSP state (opcode `0xDB`)

A general "poke a word of DMEM/state" command. The **index** selects which
state region; the **offset** selects the word within it.

| field | location | meaning |
|-------|----------|---------|
| index | `field(w0, 16, 8)` | which state block (see table below) |
| offset | `field(w0, 0, 16)` | byte offset within that block |
| data | `w1` | the 32-bit value to write |

Indices relevant here (F3DEX2 values):

| index name | value | effect |
|------------|-------|--------|
| `G_MW_SEGMENT` | `0x06` | **Set a segment base.** Segment number = `offset / 4` (equivalently `field(w0, 2, 4)`); base = `w1` (mask to a physical RDRAM offset, `w1 & 0x00FF_FFFF`). Writes `segments[seg]`. **This is the command that makes segmented addressing work** — it must be handled before any vertex/DL load that uses that segment. |
| `G_MW_NUMLIGHT` | `0x02` | number of active lights (lighting; phase-2). |
| `G_MW_FOG` | `0x08` | Pack signed 16-bit multiplier `fm` and offset `fo`. With `G_FOG` enabled, vertex shade alpha becomes `clamp(ndc_z * fm + fo, 0, 255)`; source vertex alpha is ignored. |
| `G_MW_LIGHTCOL` | `0x0A` | Update a light's RGB without changing its direction. F3DEX2 uses a 24-byte slot stride and word offsets 0/4 for the primary/copied colors; `gSPLightColor` emits both writes with identical RGBA data and ignores alpha. |
| `G_MW_CLIP` | `0x04` | `gSPClipRatio` writes negative X/Y ratios 1..6 at offsets `0x04`/`0x0C` and their signed-negative positive-side partners at `0x14`/`0x1C`. These expand primitive clipping independently of `G_CULLDL`. |
| `G_MW_PERSPNORM` | `0x0E` | retain the public unsigned `.16` perspective-divide normalization scale at offset zero. Any nonzero value is mathematically neutral in the float reference divide; explicit zero makes geometry non-renderable. |
| `G_MW_FORCEMTX` | `0x0C` | second half of `gSPForceMatrix`: offset zero and marker `0x00010000` activate the previously DMA'd concatenated matrix. |

All public F3DEX2 segment, clip-ratio, light-count/color, fog, force-matrix,
and perspective-normalization destinations are implemented. Non-public/raw
subindices remain loud malformed-command frontiers.

### 1.4 `G_MOVEMEM` — DMA a block into RSP state (opcode `0xDC`)

Loads a *multi-word* block (viewport, light, matrix) from RDRAM into RSP
state. F3DEX2 layout:

| field | location | meaning |
|-------|----------|---------|
| index | `field(w0, 0, 8)` | which block |
| size/offset hint | `field(w0, 8, 8)` | packed length/dest offset (for lights, selects which light) |
| source | `w1` | segmented address of the block |

Index values (F3DEX2): `G_MV_VIEWPORT = 8`, `G_MV_LIGHT = 10`,
`G_MV_MATRIX = 14` (a forced/absolute matrix load), `G_MV_MMTX/PMTX` etc.

- **`G_MV_VIEWPORT`** is the one that matters for correct screen mapping (see
  section 3.5). It points at a **`Vp` struct**: 8 × s16 = `vscale[4]` then
  `vtrans[4]` (x, y, z, w each). Both scale and translate are stored in a
  **fixed-point where the value is pre-multiplied by 4** (the classic N64
  "quarter-pixel" viewport encoding) — divide by 4 to get pixel units. See
  3.5.
- **`G_MV_LIGHT` slot encoding:** the offset field is in eight-byte units.
  Public offsets `0 * 24` and `1 * 24` load the X and Y screen-space
  directions from the two `Light_t`-shaped entries emitted by `gSPLookAt`.
  Their signed direction bytes are at offsets 8..10; the color bytes are
  placeholders.
  `gSPLight(light, n)` targets byte offset `n * 24 + 24`; DMEM 24-byte indices
  0 and 1 are the look-at vectors, so `LIGHT_1` (offset 48, wire value 6)
  maps to light slot 0, `LIGHT_2` maps to slot 1, and so on. In other words,
  the light slot is `offset_bytes / 24 - 2`, not `- 1`.
- **`G_MV_MATRIX`** is the first half of public F3DEX2 `gSPForceMatrix`.
  It loads one complete 64-byte `Mtx` at destination offset zero as an
  already-concatenated model/projection transform. The following
  `G_MW_FORCEMTX` marker activates it; neither underlying matrix stack is
  changed.

The decoder validates both the encoded lengths and the complete source ranges
before changing state. A viewport, LookAt, light, or forced-matrix DMA that
would run past RDRAM traps by public command name instead of retaining the
previous value. `G_MV_POINT`, the raw model/projection matrix indices, and any
other non-public destination trap with index and wire words; they are not
acknowledged skips.

### 1.5 `G_DMA_IO` — debug I/DMEM transfer (opcode `0xD6`)

The public F3DEX2 `gSPDma_io` macro packs a transfer direction, an eight-byte
unit RSP address, a 12-bit `size - 1`, and a segmented DRAM address. Its
`gSPDmaRead` and `gSPDmaWrite` wrappers use flags zero and one respectively.
The SGI *Nintendo 64 RSP Programmer's Guide*, chapter 4 tables 4-1 and 4-6,
defines those hardware directions as READ = DRAM -> I/DMEM and WRITE =
I/DMEM -> DRAM. The same chapter requires eight-byte-aligned addresses,
eight-byte-multiple lengths, at most 4 KiB, without crossing the selected
4 KiB RSP bank.

`execute_display_list_f3dex2_ops` applies the transfer immediately against
fn64's one persistent `RspMemory` and logical-byte `RdramView` mapping. This
ordering is load-bearing: a WRITE may replace the next display-list command,
and the next decoder iteration observes the replacement. The read-only
inspection helper traps on `G_DMA_IO` because it has neither authority nor a
persistent RSP memory image; it cannot truthfully simulate the command.

Sources: public
[`gbi.h`](https://ultra64.ca/files/documentation/online-manuals/man/header/gbi.htm)
and SGI
[*Nintendo 64 RSP Programmer's Guide*, chapter 4](https://ultra64.ca/files/documentation/silicon-graphics/SGI_Nintendo_64_RSP_Programmers_Guide.pdf).

### 1.6 `G_LOAD_UCODE` — ordered F3DEX2 self-load (opcode `0xDD`)

The public `gSPLoadUcodeEx` macro is a compound command. A preceding
`G_RDPHALF_1` carries the physical microcode-data address; `G_LOAD_UCODE`
carries `data_size - 1` in its low 16 bits and the physical text address in
its second word. Public `OSTask` guidance fixes the text size at
`SP_UCODE_SIZE`, one complete 4 KiB IMEM bank. The RSP Programmer's Guide
states that the compiled microcode data section is loaded at the beginning of
DMEM. fn64 therefore copies exactly the declared data prefix to DMEM and the
4 KiB text image to IMEM through typed `RspMemory`, using logical RDRAM bytes
and the hardware's 64-bit DMA granularity. Physical ucode addresses are not
resolved through the display-list segment table.

The transfer is immediate in the ordered execution path. A following
`G_DMA_IO` observes the new bank contents, and an IMEM write advances the
persistent generation exactly once. Missing staging, reserved payload bits,
unaligned addresses, non-eight-byte data sizes, bank overflow, and RDRAM
bounds all trap by command name. The read-only inspection helper traps because
it cannot truthfully mutate the console's persistent RSP memories.

`ReferenceBackend::with_f3dex2` selects a decoder but admits no content. The
rspboot-populated task-entry IMEM and every self-loaded 4 KiB text image are
identified by exact SHA-256; only a digest registered through
`with_f3dex2_ucode_sha256` may enter or remain in the F3DEX2 HLE lane. An
unadmitted entry returns `FrameStatus::NeedsLle` before decode. An unadmitted
self-load stops at the load boundary before the following command can be
decoded under the wrong GBI. The reference backend performs admitted decode
against cloned RDRAM/RSP state. On rejection it discards the clone and the
runtime replays the whole ucode phase from untouched post-rspboot state through
the scalar/vector interpreter. This preserves real register evolution,
DMA/overlay order, BREAK, and DPC submissions without inventing a mid-HLE
scalar/VU snapshot. RT64 applies the same entry admission; its raw-RDP C ABI
then consumes the LLE-produced bounded command ranges without GBI recognition.

F3DEX2 self-load does **not** use the older F3DEX reset contract. Its public
release notes give an explicit maintained-state list: display-list stack,
matrix stack, modelview and projection matrices, segment table, scissor,
other mode, perspective normalization, and viewport survive. The combined MP
matrix, geometry mode, lights, vertex cache, fog factors, texture selection,
clip ratio, and compound-command staging reset. Independent RDP registers and
TMEM remain live. A nested-list test proves that execution returns to the
caller after self-load, as required by the F3DEX2 maintained display-list
stack.

Sources: public
[`gbi.h`](https://ultra64.ca/files/documentation/online-manuals/man/header/gbi.htm),
the public
[F3DEX2 release notes](https://ultra64.ca/files/documentation/online-manuals/man-v5-2/allman52/ucode/f3dex2/f3dex2.htm),
the public libultra
[`OSTask` documentation](https://ultra64.ca/files/documentation/online-manuals/functions_reference_manual_2.0i/os/OSTask.html),
and SGI
[*Nintendo 64 RSP Programmer's Guide*, "DMEM Organization and Usage"](https://ultra64.ca/files/documentation/silicon-graphics/SGI_Nintendo_64_RSP_Programmers_Guide.pdf).

---

## 2. Vertex + geometry commands

### 2.1 `G_VTX` — load transformed vertices into the vertex cache (opcode `0x01`)

Loads `n` vertices from an RDRAM array, transforms each by the current
model→projection matrix, and stores the results into the **32-entry vertex
cache** (indices 0-31) starting at a destination slot.

**F3DEX2 argument layout** (this differs from F3DEX/F3D — get it right):

| field | location | meaning |
|-------|----------|---------|
| `n` (count) | `field(w0, 12, 8)` | number of vertices to load (1-32) |
| end index | `field(w0, 1, 7)` | the cache index **just past** the last loaded vertex, i.e. `v0 + n`. So the destination start slot is `v0 = field(w0, 1, 7) - n`. |
| source | `w1` | segmented address of the vertex array |

> Note the F3DEX2 encoding stores `(v0+n)` and `n`, *not* `v0` directly. Recover
> `v0 = end - n`. (This is a real divergence from the SDK's F3DEX branch, which
> is why a naive `((w0>>16)&0xFF)/2` reader — an F3DEX-shaped formula — can
> place vertices in the wrong cache slots. See failure risk #2, section 8.)

The decoder rejects a zero/over-32 count, an end index smaller than the count,
a destination beyond slot 31, or a source range that cannot hold all `n`
records before mutating any cache entry. Triangle commands likewise trap when
any packed index exceeds slot 31; malformed geometry cannot disappear as an
apparently valid cull.

**Per-vertex wire format — `Vtx_t`, 16 bytes, big-endian:**

| offset | type | field | meaning |
|--------|------|-------|---------|
| 0 | s16 | `ob[0]` (x) | model-space X |
| 2 | s16 | `ob[1]` (y) | model-space Y |
| 4 | s16 | `ob[2]` (z) | model-space Z |
| 6 | u16 | `flag` | usually 0; not needed for geometry |
| 8 | s16 | `tc[0]` (s) | texture S coordinate, **S10.5 fixed point** (phase-2) |
| 10 | s16 | `tc[1]` (t) | texture T coordinate, S10.5 (phase-2) |
| 12 | u8×4 | `cn[0..4]` | **either** RGBA vertex color (shading OFF) **or** XYZ normal (bytes 0-2, signed) + alpha (byte 3), when lighting is ON |

- The `cn` bytes are the **same 4 bytes** interpreted two ways depending on
  the `G_LIGHTING` geometry-mode bit (2.4). With lighting **off** (the simple
  first-frame case) they are a literal RGBA vertex color — use them directly
  as the flat/Gouraud vertex color. With lighting **on** they are a signed
  normal that must be lit against the light set; **for a first recognizable
  frame, treat them as color regardless** (accept that lit surfaces look
  wrong-colored — geometry is still correct).
- x/y/z are **model space**; they must be run through the transform pipeline
  (section 3) at load time. The N64 transforms at `G_VTX` time and caches the
  *screen/clip-space* result; `G_TRI*` only indexes the cache.

### 2.2 `G_TRI1` — one triangle (opcode `0x05`)

Draws one triangle from three vertex-cache indices.

**F3DEX2 layout:** the three indices are packed as three 7-bit fields in `w0`:

| field | location |
|-------|----------|
| index 0 | `field(w0, 17, 7)` |
| index 1 | `field(w0, 9, 7)` |
| index 2 | `field(w0, 1, 7)` |

Each field is **already the cache slot** (0-31). (In the SDK *macro* the index
is written as `v*2` into a byte, so an alternate correct reader is "extract a
byte and divide by 2"; but the clean way is the 7-bit-field-at-odd-bit
extraction above, which yields the slot directly. Both must agree — mixing an
8-bit-byte reader with a 7-bit-field layout is a bug source.)

Winding order (v0→v1→v2) determines the front face for back-face culling
(2.4). All three vertices come from the already-transformed cache.

### 2.3 `G_TRI2` / `G_QUAD` — two triangles (opcodes `0x06` / `0x07`)

Draws **two** triangles in one command (used to pack a quad or a triangle
strip pair). Triangle A's three indices are in `w0` (same 7-bit-field layout
as `G_TRI1`); triangle B's three indices are in `w1` at the **same bit
positions** (`field(w1, 17, 7)`, `field(w1, 9, 7)`, `field(w1, 1, 7)`).

`G_QUAD` is encoded identically to `G_TRI2` at the microcode level — two
triangles sharing an edge. Decode it exactly like `G_TRI2`.

### 2.4 Geometry mode — `G_GEOMETRYMODE` (opcode `0xD9`)

F3DEX2 collapses the older `G_SETGEOMETRYMODE`/`G_CLEARGEOMETRYMODE` pair into
**one** command that clears some bits and sets others atomically:

| field | location | meaning |
|-------|----------|---------|
| clear mask | `field(w0, 0, 24)` | bits to **clear** — but note this is stored **inverted**: the wire value is `~clearbits` (the AND mask). Effect: `mode = (mode & w0_low24) | w1`. |
| set mask | `w1` | bits to **set** |

So the update is `geometry_mode = (geometry_mode & field(w0,0,24)) | w1`.

Relevant geometry-mode bits (F3DEX2 values):

| bit name | value | meaning for the render |
|----------|-------|--------|
| `G_ZBUFFER` | `0x00000001` | enable z-buffering |
| `G_SHADE` | `0x00000004` | enable shading (interpolate vertex color) |
| `G_CULL_FRONT` | `0x00000200` | cull front-facing triangles |
| `G_CULL_BACK` | `0x00000400` | cull back-facing triangles (the common case) |
| `G_CULL_BOTH` | `0x00000600` | cull both (draws nothing) |
| `G_FOG` | `0x00010000` | fog (phase-2) |
| `G_LIGHTING` | `0x00020000` | enable lighting → `cn` = normal, not color (phase-2) |
| `G_TEXTURE_GEN` | `0x00040000` | generate S/T by projecting the vertex normal onto the loaded LookAt X/Y directions |
| `G_TEXTURE_GEN_LINEAR` | `0x00080000` | use the inverse-cosine mapping when combined with `G_TEXTURE_GEN` |
| `G_SHADING_SMOOTH` | `0x00200000` | Gouraud (smooth) vs flat shading |

**For a first frame:** you can ignore lighting/fog/shade-smooth. The one bit
worth honoring early is **back-face culling** (`G_CULL_BACK`), because without
it, back faces overpaint front faces and models look inside-out/garbled. Cull
by the sign of the screen-space triangle's signed area (winding); which sign
is "back" depends on the N64's convention (Y is down in screen space — see
3.5), so if culling removes the wrong half, flip the test.

#### Automatic texture-coordinate generation

The public Programming Manual, Chapter 11.7.5, requires `G_LIGHTING` together
with `G_TEXTURE_GEN`: the `cn[0..3]` bytes are therefore a signed normalized
XYZ normal plus unchanged alpha. The reference lane transforms the two loaded
LookAt directions into the vertex's model space, normalizes them, and projects
the normalized vertex normal onto each axis.

- Regular/spherical mode maps projection `p` from `[-1,+1]` to `[0,scale]`:
  `generated = (p + 1) / 2 * scale`.
- With `G_TEXTURE_GEN_LINEAR`, it uses
  `generated = acos(clamp(p,-1,+1)) / pi * scale`.

Here `scale` is the corresponding S/T value supplied by `gSPTexture`. The
manual's worked form `gSPTexture(tex_max << 6, ...)` therefore maps the regular
`+1` endpoint (and the linear `-1` endpoint) to `tex_max`. Generated values
replace the explicit S10.5 `Vtx.tc` fields at vertex-load time and then follow
the same perspective-correct primitive interpolation and RDP sampler path.
Using texture generation without lighting, or before both LookAt DMAs, traps
by the public state/command name rather than treating normal bytes as color or
inventing axes. The exact F3DEX2 reciprocal/trigonometric lookup and fixed-point
rounding remain a hardware-trace frontier.

Sources: public libultra
[`gbi.h`](https://ultra64.ca/files/documentation/online-manuals/man/header/gbi.htm)
(`G_MVO_LOOKATX/Y`, `gSPLookAtX/Y`, `gdSPDefLookAt`) and Programming Manual
[11.7, "Vertex Lighting State"](https://ultra64.ca/files/documentation/online-manuals/man-v5-1/pro-man/pro11/11-07.htm).

### 2.5 Clip-volume culling and depth branches — `G_CULLDL` / `G_BRANCH_Z`

F3DEX2 retains six homogeneous clip-plane flags for every transformed vertex:
`x < -w`, `x > w`, `y < -w`, `y > w`, `z < -w`, and `z > w`. These flags
drive display-list control flow independently of triangle face culling.

- **`G_CULLDL` (`0x03`)** receives an inclusive cache range encoded as
  `vstart * 2` and `vend * 2`. AND the clip flags across the selected vertices.
  If any flag remains set, every vertex lies beyond the same clip plane, so
  terminate the current display list exactly as `G_ENDDL` would. A culled
  child therefore returns to its caller; a culled root ends the task. The
  public `gSPCullDisplayList` contract states that this uses retained clipping
  codes and is unaffected by the clip ratio.
- **`G_BRANCH_Z` (`0x04`)** is the second half of the public
  `gSPBranchLessZraw` compound command. A preceding `G_RDPHALF_1` stages the
  segmented target, while `G_BRANCH_Z` identifies one vertex through redundant
  `vtx * 5` and `vtx * 2` fields and carries the raw unsigned 16.16 screen-Z
  threshold in `w1`. Tail-branch to the staged target, without pushing a return
  address, when the retained vertex screen Z is less than or equal to that
  threshold. `G_MODIFYVTX` screen-Z writes must therefore remain exact rather
  than round-tripping through a host float.

These encodings and comparisons follow the public `gSPCullDisplayList` manual
page and the public F3DEX2 `gbi.h` macros for `gsSPCullDisplayList` and
`gsSPBranchLessZraw`.

### 2.6 Lines — `G_LINE3D` (opcode `0x08`)

The public F3DEX2/L3DEX packing stores `v0 * 2`, `v1 * 2`, and an unsigned
width parameter in the three payload bytes of `w0`; `w1` is zero. The public
macros express the flat-shade flag by swapping the encoded endpoints, making
the first endpoint the selected flat color. `gSPLine3D` uses width parameter
zero for a 1.5-pixel line; `gSPLineW3D` defines the total width as
`1.5 + width_parameter / 2` pixels.

The reference lane emits a typed line operation rather than manufacturing a
triangle during decode. It clips transformed endpoints and their color,
texture, and depth attributes against all six homogeneous clip planes, using
the active per-side `gSPClipRatio` for X/Y while retaining ordinary Z planes, applies
the active scissor, evaluates the public eight coverage samples over the
commanded width, supports flat or smooth shade and perspective texture
interpolation, and runs the normal combiner/blender. Line depth is read-only:
Z comparison can reject the fragment, but a line never updates the depth
image. Exact microcode-generated degenerate-polygon edge coefficients and
subpixel endpoint convention remain a hardware-trace differential frontier.

---

## 3. Transform pipeline (the core of visible geometry)

A model-space vertex becomes a screen pixel via:

```
model --[modelview M]--> world/eye --[projection P]--> clip
      --[perspective divide /w]--> NDC --[viewport]--> screen(x,y) + depth(z)
```

F3DEX2 keeps a **modelview matrix stack** and a **single projection matrix**,
and precomputes their product (the "MVP") which is what `G_VTX` actually
applies.

### 3.1 The fixed-point matrix format (`Mtx`) — 64 bytes, split int/frac

An N64 `Mtx` is a 4×4 matrix of **s15.16 fixed-point** values, but stored in a
non-obvious **split layout** that must be reassembled correctly:

- **64 bytes total = two 32-byte halves.**
- **First 32 bytes:** the 16 elements' **signed integer parts**, each a
  big-endian **s16**, in row-major order (`m[0][0], m[0][1], ... m[3][3]`).
- **Second 32 bytes:** the 16 elements' **fractional parts**, each a
  big-endian **u16**, same row-major order.
- The real value of element `k` (0-15) is:

  ```
  value = (i32(int16[k]) << 16 | u16(frac[k])) as i32, then / 65536.0
  ```

  i.e. reinterpret `(int_part, frac_part)` as one 32-bit fixed-point number
  and divide by 2^16. **Do not** compute `int + frac/65536` as two separate
  float adds when `int` is negative — that double-counts the sign. Combine the
  halves as a single 32-bit two's-complement quantity *first*, then scale.
  (This is failure risk #3.)

**Storage/multiplication convention.** The N64 multiplies **row-vector ×
matrix** (`v' = v · M`), with the matrix stored row-major for that product. If
your transform code multiplies **matrix × column-vector** (`v' = M · v`), you
must **transpose on read** so the two conventions agree. Getting this wrong
mirrors/rotates the whole scene about a diagonal. (Related to failure risk
#3.)

### 3.2 `G_MTX` — load/multiply a matrix (opcode `0xDA`)

Loads a matrix from RDRAM onto the projection or modelview, optionally
pushing the modelview stack first.

**F3DEX2 layout** (note the **inverted param byte** — F3DEX2 XORs the params
with the push bit on the wire):

| field | location | meaning |
|-------|----------|---------|
| params | `field(w0, 0, 8)` **XOR `G_MTX_PUSH`** | the caller's params; the wire byte has the push bit flipped, so `params = field(w0,0,8) ^ 0x01` to recover them |
| source | `w1` | segmented address of the 64-byte `Mtx` |

Param bits (F3DEX2 values — **different from F3D!**):

| param | F3DEX2 value | meaning |
|-------|--------------|---------|
| `G_MTX_PROJECTION` | `0x04` | target the projection matrix (else modelview) |
| `G_MTX_LOAD` | `0x02` | **replace** the target (else **multiply/concat** onto it) |
| `G_MTX_PUSH` | `0x01` | push the modelview stack before loading (save for later `G_POPMTX`) |

> The F3D microcode uses `PROJECTION=0x01, LOAD=0x02, PUSH=0x04` — reversed
> bit meanings. This spec and OoT are F3DEX2, so use the `0x04/0x02/0x01` set.

The wire decoder also requires the public zero destination offset, exact
64-byte DMA length, only these three parameter bits, and a complete 64-byte
RDRAM source before changing either matrix stack.

Behavior:

- **Projection** (`PROJECTION` set): replace (or, rarely, multiply) the single
  projection matrix. OoT loads projection once per view via `guPerspective`
  output. Push/pop does not apply to projection.
- **Modelview** (`PROJECTION` clear):
  - If `PUSH`: save the current top modelview onto the stack first.
  - If `LOAD`: replace the current modelview with the loaded matrix.
  - Else (`MUL`): `modelview = modelview · loaded` (concatenate — this is how
    a hierarchy of local transforms is built, e.g. a limb relative to a body).
- After any change, **recompute MVP = modelview · projection** in the N64's
  row-vector convention (or apply them
  in sequence at vertex time). This cached product is what `G_VTX` uses.

### 3.3 `G_POPMTX` — pop the modelview stack (opcode `0xD8`)

Restores a previously pushed modelview. **F3DEX2 layout:** `w1` holds the
number of matrices to pop **× 64** (each stack entry is a 64-byte matrix), so
`count = w1 >> 6`. Pop that many entries and set the modelview to the last one
popped, then recompute MVP. (The modelview stack is the classic
push-a-transform / draw-children / pop pattern; the projection matrix has no
stack.)

The F3DEX2 modelview stack holds **up to 18** matrices.

#### `gSPForceMatrix` — replace the concatenated transform

Public F3DEX2 emits this as exactly two commands. `G_MOVEMEM G_MV_MATRIX`
stages one 64-byte fixed-point matrix; `G_MOVEWORD G_MW_FORCEMTX` at offset
zero with `0x00010000` makes it the matrix used by later vertex transforms.
This bypasses multiplication and leaves the modelview/projection stacks
untouched. The next ordinary `G_MTX` or `G_POPMTX` rebuilds the concatenated
matrix from those stacks and supersedes the forced value. A modelview load
with no explicit projection uses identity projection rather than dropping the
modelview transform. Missing/malformed halves trap with both public command
names.

Sources: public libultra
[`gbi.h`](https://ultra64.ca/files/documentation/online-manuals/man/header/gbi.htm)
(`gSPForceMatrix`, `G_MV_MATRIX`, `G_MW_FORCEMTX`) and the public
[`gSPForceMatrix` function index description](https://ultra64.ca/files/documentation/online-manuals/functions_reference_manual_2.0i/gsp/gsp.html).

#### `gSPPerspNormalize` — finite-precision divide scale

`G_MOVEWORD G_MW_PERSPNORM` stores one unsigned `.16` scale at offset zero.
The RSP scales transformed coordinates and W together before perspective
division to maximize its limited divider precision. Those factors cancel in
the float reference lane for every nonzero value; the exact fixed-point
precision improvement remains a hardware-trace item. An explicitly programmed
zero leaves the divide degenerate, so vertices retain nonpositive W and no
triangle or line is emitted. The value survives F3DEX2 `G_LOAD_UCODE`, matching
the public maintained-state list.

Source: public libultra
[`gSPPerspNormalize`](https://ultra64.ca/files/documentation/online-manuals/functions_reference_manual_2.0i/gsp/gSPPerspNormalize.html)
and Programming Manual Chapter 25, "F3DEX2 Microcode."

#### `gSPClipRatio` — expanded primitive clip rectangle

The macro emits four `G_MOVEWORD G_MW_CLIP` writes. Negative-side X/Y fields
carry `FRUSTRATIO_1..6` directly; positive-side fields carry the corresponding
signed negative halfword. The state is retained per side in command order but
resets on F3DEX2 self-load because it is absent from the public exhaustive
maintained-state list. It expands the primitive clipping planes to
`x = ±ratio*w` and `y = ±ratio*w`; Z remains `±w`. The reference line path
uses those planes directly. High-level triangles are rasterized once and then
scissored, which is mathematically equivalent inside the visible rectangle;
exact microcode subdivision and fixed-point boundary rounding remain a
hardware-trace frontier. `G_CULLDL` deliberately continues to use ordinary
`±w` retained codes, as required by its public contract.

Source: public libultra
[`gSPClipRatio`](https://ultra64.ca/files/documentation/online-manuals/functions_reference_manual_2.0i/gsp/gSPClipRatio.html)
and the four `G_MWO_CLIP_*` encodings in public `gbi.h`.

### 3.4 The transform math, per vertex

At `G_VTX` load time, for each source vertex `(x, y, z)` (model space):

1. **To clip space:** `clip = MVP · [x, y, z, 1]` → `(cx, cy, cz, cw)`. (Or,
   with row-vector convention, `[x,y,z,1] · MVP`.)
2. **Perspective divide:** `ndc = (cx/cw, cy/cw, cz/cw)`. This `/cw` is what
   gives perspective foreshortening; **skipping it (or dividing by a wrong /
   near-zero `w`) collapses or explodes the geometry.** Guard `cw ≈ 0`
   (vertices on/behind the eye plane) — real hardware clips against the view
   frustum first; a first-pass renderer can clamp/skip degenerate `w`. This is
   failure risk relating to the perspective divide (section 8).
3. **Viewport map** (3.5): NDC ∈ [-1, 1] → pixel coordinates.
4. Cache the resulting screen `(x, y)`, a depth value, and the vertex color.

Clipping against the frustum (near/far/sides) is done in hardware before the
divide; for a first frame you can skip real clipping and rely on the
rasterizer's scissor/bounds check, accepting artifacts on triangles that cross
the near plane.

### 3.5 Viewport → screen mapping

The viewport (`Vp`, loaded via `G_MOVEMEM`/`G_MV_VIEWPORT`, see 1.4) holds
`vscale` and `vtrans`, each 4× s16 in **quarter-pixel** units (divide by 4).
Map NDC to screen:

```
screen_x = ndc_x * (vscale.x / 4) + (vtrans.x / 4)
screen_y = ndc_y * (vscale.y / 4) + (vtrans.y / 4)   // see Y-flip note
depth    = ndc_z * (vscale.z / 4) + (vtrans.z / 4)
```

- **Y axis:** N64 screen space is **top-down** (y increases downward), while
  NDC +Y is up. The sign is absorbed by `vscale.y` being **negative** in the
  real viewport, *or* you flip Y explicitly. Do one, not both. If the image is
  upside-down, this is why.
- **OoT's viewport** for the main 3D view is centered on a **320×240**
  framebuffer: scale ≈ (160, 120) in pixels (so `vscale ≈ (640, 480)` in the
  ×4 encoding), translate ≈ (160, 120) center (`vtrans ≈ (640, 480)`). The
  decoder requires that DMA before transformed `G_VTX`: no viewport means a
  named trap, because a host-sized 320×240 stand-in fabricates RSP state and
  makes the same display list depend on the runtime's output surface.
- **Depth** maps to the z-buffer range. OoT uses a 16-bit z-buffer; the exact
  N64 z encoding is non-linear (a piecewise "float"-ish format), but for a
  software rasterizer a **linear NDC-z / plain `1/w`-style depth compare** is
  fine for a first frame — you just need *a* consistent monotonic depth to get
  correct occlusion (section 4.3).

---

## 4. Rasterization essentials (first flat-shaded proof)

Once section 3 has produced screen-space triangles (three `(x, y, depth,
rgba)` vertices), the RDP rasterizes them. For a software proof you do **not**
need bit-exact RDP edge rules — you need filled triangles with correct
coverage and depth.

### 4.1 The RDP triangle / edge-coverage model (conceptual)

- The RDP rasterizes a triangle as **three edges** described by
  edge-slope/coefficient data (the "edge coefficients" the RSP sends per
  triangle). Conceptually it walks scanlines top-to-bottom, and for each
  scanline computes the left/right span from the edge equations, then fills
  the covered pixels.
- **Coverage / fill rule:** a pixel is inside iff it is on the interior side
  of all three edges. The raw RDP path evaluates the public 4×4 checkerboard
  mask's eight selected subpixel centers against the commanded major/minor
  edges and retains their complete typed eight-bit identity mask until the
  fragment boundary. The higher-level F3DEX2 triangle and line paths evaluate
  those same eight selected centers and return the same mask type; only the
  shared framebuffer path derives its zero-through-eight storage count.
  Exact-boundary samples use a winding-
  independent top-left rule, so adjacent high-level triangles assign a shared
  edge once, matching the raw span walker's public left-inclusive/right-
  exclusive ownership. Edge vectors prove a vertical half-pixel mask of
  `0x55`, a one-sample corner mask of `0x01`, and complementary diagonal
  masks `0xaf`/`0x50`. Exhaustive raw vectors cover all 675 axis-aligned edge
  rectangles on the eighth/quarter-pixel wire grids and all 225 quarter-pixel
  scissor rectangles within one pixel. Full coverage evaluates attributes at
  pixel center (`x+0.5, y+0.5`). For partial coverage, one shared typed policy
  chooses the actually covered checkerboard sample nearest the center, with
  sample-array order breaking distance ties, and raw/HLE shade, texture, and Z
  all use that point. This satisfies Programming Manual 15.4's public
  on-primitive Z requirement and avoids extrapolating a narrow edge from an
  uncovered center. The selector is a bounded host policy, not a claimed RDP
  centroid: allowed public sources do not publish the representative lookup,
  tie rule, correction accumulator width, or rounding. Exhaustive vectors
  cover all 255 nonzero identity masks and sample-sensitive raw/HLE slopes and
  complementary shared edges.
- **Attribute interpolation:** compute barycentric weights from the edge
  functions. Vertex color and the current approximate depth remain
  screen-linear. Texture coordinates are perspective-correct: interpolate
  `S/w`, `T/w`, and `1/w`, then divide the first two results by the third.
  This matches RT64's raster path, which preserves homogeneous `w`
  (`shaders/RasterVS.hlsl:21,36-38`) and emits UV as ordinary TEXCOORD with
  the default perspective interpolation
  (`render/rt64_raster_shader.cpp:254-260,280-286`).
- **Raw coefficient arithmetic:** raw triangle X, slope, shade, texture, and Z
  terms remain signed 16.16 integers through span and plane evaluation. The
  eight coverage sample positions are exact odd eighths, and Table 12's XH/XM
  reference is the scanline preceding YH while XL is referenced at YM. Host
  floating point is now used only after S/T/W evaluation for the final texture
  ratio. The hardware accumulator's narrower internal truncation points are
  not specified by the public command format and remain differential work.
- **Scissor:** `G_SETSCISSOR` (`0xED`) packs unsigned 12-bit upper-left X/Y in
  `w0[23:12]/w0[11:0]` and lower-right X/Y in the same fields of `w1`; all
  four are quarter-pixels (`ultra64/gbi.h:4819-4826`). The lower-right edge
  is exclusive (OoT `src/code/PreRender.c:137` passes stored inclusive
  `lrx + 1, lry + 1`). Snapshot the current rectangle on each emitted
  triangle and intersect its pixel-center raster bounds with `[ul, lr)`.
  RT64 stores the fixed rectangle in `hle/rt64_rdp.cpp:974-980` and performs
  the triangle/scissor intersection in `hle/rt64_rsp.cpp:1140-1154`.

### 4.2 Degenerate / cull handling

- **Zero-area** (collinear) triangles: skip (they contribute nothing).
- **Back-face culling** (if `G_CULL_BACK` is set, 2.4): drop triangles whose
  screen-space signed area has the "back" sign. Do this *before* the fill loop.

### 4.3 Z-buffer compare

- OoT uses a **16-bit z-buffer** (`G_SETZIMG` points at it; `G_ZBUFFER`
  geometry-mode bit + the render mode's z-compare/z-update bits enable it).
- Per covered pixel: compute the interpolated depth. `Z_CMP` controls whether
  the fragment compares against stored depth; `Z_UPD` independently controls
  whether a passing fragment replaces stored depth. The reference framebuffer
  models all four compare/update combinations rather than forcing both on for
  every triangle.
- Raw RDP Z is converted from its 16.16 command plane to the documented
  unsigned 15.3 working range, where near is zero and far is `G_MAXZ`. The
  Chapter 16 exponent/mantissa + split DeltaZ memory codec is implemented and
  exhaustively checked. Passing `Z_UPD` fragments and depth-directed fills
  commit CPU-visible compressed halfwords plus the two hidden DeltaZ bits;
  selecting a depth image reloads both, and the hidden pair is owned by its
  physical RDRAM halfword so it survives task and image switches. Raw triangle
  DeltaZ follows Chapter 15 Equation 4 (`|dZ/dx| + |dZ/dy|`). Stored DeltaZ
  uses Equation 10's most-significant-set-bit index and expands back to its
  power-of-two floor for comparison. Equations 5-9 supply typed `Farther`,
  `Nearer`, `In Front`, and maximum-Z relations. Opaque/interpenetrating modes
  admit front or depth-correlated fragments, translucent mode requires strict
  in-front depth, and decal mode admits only correlated fragments over a
  non-clear depth sample. When actual pixel coverage plus memory coverage
  exceeds eight, opaque mode now uses Equation 9's strict `In Front` result
  instead of the DeltaZ correlation window, as specified by Programming
  Manual Chapter 15, "Opaque Surface Antialiased Z-Buffer Algorithm."
  Chapter 15 separately requires a coverage-wrap-selected adjustment for
  interpenetration mode but does not publish its arithmetic. One typed routing
  decision therefore preserves ordinary non-wrap interpenetration and traps by
  `ZMODE_INTER` when wrap selects the unsupported adjustment, rather than
  silently aliasing that case to opaque correlation. The adjustment remains a
  hardware-vector frontier.
- `G_ZS_PRIM` selects the persistent `G_SETPRIMDEPTH` registers for triangles
  and texture rectangles. Per the public libultra `gDPSetPrimDepth` function
  reference and Programming Manual Chapter 15, "Z Calculation," the 16-bit
  primitive Z contributes its low 15 integer bits plus three zero fractional
  bits, while primitive DeltaZ contributes all 16 bits. Both are compressed
  through the same persistent Z-image path as stepped triangle depth.
- If you skip the z-buffer entirely, draw order artifacts appear (later
  triangles overwrite earlier regardless of depth). Acceptable for a very
  first "is anything recognizable" frame, but the z-buffer is the first thing
  to add after triangles show up.

### 4.4 Public framebuffer layouts and OoT's RGBA5551 target

- The RDP memory interface has three legal color-image layouts: a size-defined
  8-bit index/intensity byte, RGBA16, and RGBA32. Programming Manual
  [Chapter 15.5, "Color Image Format"](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro15/15-05.html)
  defines their byte layouts, while
  [Chapter 14.6, "Color Index Frame Buffer"](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro14/14-06.html)
  requires an undereferenced CI8 texture for direct index copies. The
  reference backend classifies these as one `ColorImageLayout`, imports and
  commits each exact logical byte layout, supports same-address
  reinterpretation, and copies undereferenced CI8 indices to the 8-bit target.
  Copy source and destination layouts must match; unsupported raw format/size
  combinations fail by name. Programming Manual 15.5.4 gives RGBA16 copy
  alpha special semantics: with `G_AC_THRESHOLD` or `G_AC_DITHER`, its one
  alpha bit is a direct write enable and never enters the eight-bit
  blend-alpha comparator. An alpha-zero texel is therefore rejected even when
  blend alpha is zero; with `G_AC_NONE`, the same texel is copied normally.

- OoT's color framebuffer (`G_SETCIMG`) is **16-bit RGBA5551**: 5 bits red,
  5 green, 5 blue, **1 bit alpha/coverage**, packed big-endian per pixel:

  ```
  pixel16 = (R5 << 11) | (G5 << 6) | (B5 << 1) | A1
  ```

  where `R5 = R8 >> 3`, etc. (top 5 bits of each 8-bit channel).
- Dimensions: **320 × 240** for the main view (OoT is 320×240 NTSC; a
  low-res/interlaced mode exists but the HLE target is 320×240).
- A software rasterizer can render into an **RGBA8888** working buffer for
  convenience (as the current `raster.rs` reference does) and only pack to
  RGBA5551 when it needs to *match the real framebuffer byte layout* (e.g. to
  write back into `rdram` at the `G_SETCIMG` address, or to diff against a
  captured frame). For a first eyes-on proof, RGBA8888 → PNG is fine; the
  RGBA5551 packing is required only when writing the N64-native framebuffer.
- `G_SETCIMG` (opcode `0xFF`, an RDP op) carries: format `field(w0,21,3)`,
  size `field(w0,19,2)` (size `2` = 16-bit), width `field(w0,0,12)+1`, and
  `w1` = segmented framebuffer address. `G_SETZIMG` (`0xFE`) carries just the
  z-buffer address in `w1`. The reference lane retains `G_SETCIMG` across
  tasks and requires it before production F3DEX2/raw color writes. It does
  not infer the RDP target from the VI scanout or host `output_addr`.

---

## 5. Texture / combiner

The reference backend implements the common OoT texture and color-combiner
path described here. Physical TMEM layout and command-ordered load/render-tile
reinterpretation are modeled. Independent low/high 10.2 fractions retain
subtexel tile bounds while their integer parts select the inclusive source
span, and source-sized transfers may use a different load-descriptor size,
including the public RGBA32-through-16-bit load form. The noise input has a
typed, deterministic host stream; reproducing the unpublished silicon noise
generator and silicon-internal fixed-point/filter precision remain later
fidelity work.
Unsupported cases trap by name. K4/K5, the YUV conversion table, distinct
TEXEL1, and public mip/detail/sharpen LOD selection are modeled.

These are persistent RDP registers, not per-`OSTask` decoder scratch. The
reference backend carries the texture-image latch, tile descriptors, TLUT,
physical TMEM, scissor, other mode, combiner/key/convert state, constant
colors, and fill color across both F3DEX2 tasks and raw DPC submissions.
F3DEX2's `G_TEXTURE` enable/tile/scale is RSP state and is reset at the next
task boundary. If a new task enables it before any task has initialized TMEM,
the primitive traps by `G_TEXTURE` and tile instead of substituting white.

### 5.1 Texture image + tile + TMEM load

- **`G_SETTIMG`** (`0xFD`, RDP): sets the source texture image in RDRAM —
  format `field(w0,21,3)` (RGBA/YUV/CI/IA/I), size `field(w0,19,2)` (4/8/16/32
  bpp), width `field(w0,0,12)+1`, address `w1` (segmented). Just a pointer +
  format latch; no data moves yet.
- **`G_SETTILE`** (`0xF5`, RDP): defines a **tile descriptor** — where in the
  RDP's 4 KiB **TMEM** the texture lives, its format/size, line stride, palette
  index, and the S/T clamp/mirror/wrap + mask/shift parameters. Up to 8 tiles.
- **`G_LOADBLOCK`** (`0xF3`) / **`G_LOADTILE`** (`0xF4`): DMA texel data from
  the `G_SETTIMG` image into TMEM for a tile. `G_LOADBLOCK` loads a linear run
  (fast, whole texture); `G_LOADTILE` loads a rectangular sub-region. Its
  ULS/ULT source origin is addressed using the `G_SETTIMG` image width as the
  row stride; it is not a packed stand-alone rectangle. `G_SETTILESIZE`
  (`0xF2`) sets the tile's active S/T extent, whose ULS/ULT origin must be
  removed when mapping vertex S/T into the copied tile.
- **`G_LOADTLUT`** (`0xF0`): load a color palette into TMEM for CI (color-
  indexed) textures. The public `gDPLoadTLUTCmd` wire layout stores
  `count - 1` directly in bits 14..23; unlike S/T fields, this count has no
  quarter-texel scaling. Each RGBA16 or IA16 entry is quadricated across the
  four high-TMEM banks before a CI sample selects it.
- The backend retains the complete 4 KiB physical image and a per-bit
  initialization mask. `G_SETTILE`'s nine-bit TMEM base and line stride drive
  addressing; `G_LOADTILE` pads rows to 64-bit words, while `G_LOADBLOCK`
  applies its 1.11 `dxt` accumulator and line skip. Odd rows exchange the two
  32-bit longs in each word. RGBA32 stores RG in low TMEM and BA in high TMEM;
  YUV stores shared UV pairs low and the two Y samples high. These layouts and
  transfers use `G_SETTIMG`'s source size even when the load tile carries a
  different size. Each low/high edge's integer part selects the inclusive
  source span while its raw fractional quarter remains in SL/TL/SH/TH as the
  render origin; low and high fractions need not match.
  These rules and
  the load/render-tile separation follow Programming Manual
  [13.8, Texture Memory](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro13/13-08.html),
  [13.9, Texture Loading](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro13/13-09.html),
  and SGI *RDP Command Summary* Tables 7-10. A primitive snapshots TMEM with
  its descriptors, so later loads cannot retroactively alter queued work.
- To *sample*: take the vertex S/T (S10.5 fixed-point from the `Vtx`), apply
  the tile's public shift encoding (0=no shift, 1..10=right shift, 11..15=
  left shift by 5..1), then its clamp/mirror/mask address transform, address
  TMEM, and decode the texel per the tile format. Mask zero implies clamp;
  nonzero masks pass the low `mask` bits, and mirror inverts those bits when
  the next bit is set. The shared triangle/rectangle sampler implements this
  sequence against physical TMEM, including addresses beyond the render
  tile's clamp extent. For a nonzero mask with clamp clear, the decoded axis
  domain is `1 << mask`; the unused `G_SETTILESIZE` clamp bound therefore does
  not reject a wrapped tile merely because that bound is inverted or below
  its origin. Fetching bits that no load initialized traps with the physical
  byte address. OoT textures are commonly 4-bit/8-bit CI and 16-bit
  RGBA5551/IA.
- The shared sampler materializes the public post-perspective S10.5 grid as a
  typed signed 16-bit value and traps outside Programming Manual 13.11's
  -1024..+1023.99 valid input range. A distinct wide host accumulator begins
  only after the public tile shift, because left-shift encodings expand a
  valid input's integer magnitude; its width is not a silicon-register claim.
  Shift, origin subtraction, negative texel/fraction decomposition,
  wrap/mirror/clamp inputs, and three-nearest weights therefore use exact
  1/32-texel arithmetic instead of recomputing those boundaries through host
  float. Interpolated host coordinates floor onto that grid at one named
  bounded conversion boundary. Programming Manual 13.7 establishes the five
  fraction bits and explains that left shifts consume them; it does not
  establish reciprocal-to-grid rounding or the filter accumulator/tie rule.

### 5.2 YUV conversion

- **`G_SETCONVERT`** (`0xEC`, RDP) stores K0..K5 as six signed nine-bit
  fields split across its two words. YUV16 source data is byte-interleaved
  Y0/U/Y1/V, so adjacent luma samples share U and V. K0..K3 feed the texture
  filter's YUV-to-RGB stage; K4/K5 are independently selectable inputs to the
  color combiner's second stage. These layouts and equations are from the
  public SGI [*RDP Command Summary*, Table 28 and texture-filter section](https://ultra64.ca/files/documentation/silicon-graphics/SGI_RDP_Command_Summary.pdf).
- Other-mode `G_TC_CONV` point-samples then converts, `G_TC_FILTCONV` filters
  then converts, and `G_TC_FILT` filters without conversion, following public
  Programming Manual [Chapter 12.5](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro12/12-05.html).
  The shared raw/F3DEX2 triangle and rectangle sampler implements those three
  modes and rejects reserved encodings. Its integer coefficients and
  clamping are deterministic, but exact silicon accumulator width and
  negative-product rounding still need hardware traces.

### 5.3 Texture LOD, detail, and sharpen

Programming Manual [Chapter 13.7, "Texture Level of Detail"](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro13/13-07.html)
defines the tile pair and fraction used for mipmapping, detail texturing, and
sharpening. The reference path implements those public tables as one shared
sampler used by texture rectangles, high-level F3DEX2 triangles, and raw RDP
texture triangles:

- `G_TEXTURE` supplies the primitive tile and maximum mip level for F3DEX2
  work; raw RDP triangles carry the corresponding three-bit tile and level in
  their edge command. `G_SETPRIMCOLOR` supplies the eight-bit minimum LOD and
  primitive LOD fraction independently.
- Each emitted primitive owns an immutable snapshot of all eight decoded
  tiles. Later `G_SETTIMG`/`G_LOAD*` commands therefore cannot retroactively
  change an already-emitted primitive's mip chain. Tile addition wraps modulo
  eight, including primitive tile 7.
- LOD derives from the difference between the current perspective-corrected
  S/T coordinate and its adjacent +X/+Y coordinates. Clamp mode uses the
  finest tile twice while magnifying; detail mode chooses the documented
  base/+1 or +1/+2 pair and applies minimum LOD; sharpen mode uses a negative
  fraction while magnifying. The selected fraction feeds both RGB and alpha
  `LOD_FRACTION` combiner inputs.
- Every selected mip must have a decoded `G_LOADBLOCK`/`G_LOADTILE` image.
  Missing selected tiles and a no-LOD combiner `TEXEL1` without tile+1 trap by
  tile/source name; neither aliases silently to TEXEL0.

The present deterministic magnitude is the maximum absolute S/T derivative
component. Chapter 13.7 specifies adjacent-coordinate differences and its
selection examples, but not enough silicon detail to prove that norm or the
internal rounding width. Exact reciprocal-to-S10.5 conversion, derivative/LOD
quantization, output boundary rounding, and filter accumulator behavior remain
hardware-trace work; this is not claimed bit-exact.

### 5.4 Color combiner + `G_TEXTURE`

- **`G_SETCOMBINE`** (`0xFC`, RDP): programs the **color combiner** — a
  two-cycle formula `(A - B) * C + D` for RGB and separately for alpha, where
  A/B/C/D are selected from a fixed menu of inputs (combined/texel0/texel1/
  primitive/shade/env colors, etc.). This is *the* op that decides whether a
  pixel's color comes from the texture, the vertex/shade color, a flat prim
  color, or a blend. The wire packing is the public `gbi.h`
  `GCCc0w0`/`GCCc1w0`/`GCCc0w1`/`GCCc1w1` layout: cycle-0 and cycle-1 fields
  are interleaved across both command words. Selector values are also
  position-dependent (A/B/C/D do not share one numeric table), so decode to
  semantic sources before evaluating. `gbi.rs` snapshots both cycles onto
  each emitted triangle; `raster.rs` evaluates cycle 0 then cycle 1, carrying
  COMBINED between them. This covers OoT's modulate, decal/replace,
  primitive-tint, environment-blend, shade-only, PASS2, and `*2` presets.
  The public OoT decomp demonstrates the covered mix directly in
  `src/code/z_rcp.c:27-225` (shade, decal, modulate, primitive tint, PASS2)
  and `src/overlays/gamestates/ovl_file_choose/z_file_choose.c:282`
  (primitive/environment blend).
- **`G_TEXTURE`** (`0xD7`, RSP): enables texturing and sets the S/T
  coordinate scale applied to vertex texcoords, plus which tile + mip level.
  Fields: on-bit `field(w0,1,7)`, mip level `field(w0,11,3)`, tile
  `field(w0,8,3)`, S scale `field(w1,16,16)`, T scale `field(w1,0,16)` (both
  U0.16). The current decoder applies these fields to its selected TMEM tile.
- **`G_SETOTHERMODE_H/L`** (`0xE3`/`0xE2`): the RDP "other modes" — render
  mode (blend/z-compare/AA/cvg), texture filter, cycle type (1-cycle/2-cycle/
  copy/fill), etc. F3DEX2 encodes a partial update as `length-1` in the low
  byte of `w0` and `32-shift-length` in bits 8..15; the decoder reconstructs
  the selected H/L mask, updates the persistent words, and snapshots them on
  every emitted triangle. Typed accessors expose the public `G_MDSFT_*`,
  `CVG_DST_*`, `ZMODE_*`, and `GBL_c1`/`GBL_c2` fields. The public encoding is
  `include/ultra64/gbi.h:497-627,3353-3369`; the matching RT64 field layout
  and partial-update structure are `shared/rt64_other_mode.h:14-101` and
  `hle/rt64_rsp.cpp:1026-1037`.
- **Alpha compare** is selected by low bits 0..1: `G_AC_NONE=0`,
  `G_AC_THRESHOLD=1`, and `G_AC_DITHER=3` (`gbi.h:500,584-587`). Threshold
  mode discards post-combiner fragments below `G_SETBLENDCOLOR.a`; rejected
  fragments update neither color nor depth. Programming Manual 15.5.4 defines
  dither compare as a hardware-generated pseudo-random threshold, independent
  of the ordered RGB/alpha dither matrices. The reference rasterizer compares
  against its typed per-fragment noise byte rather than substituting an ordered
  Bayer threshold. Its deterministic seedable stream preserves reproducible
  digests but does not claim the unpublished silicon polynomial. OoT's setup display list selects threshold comparison at
  `src/code/z_rcp.c:815-818` and programs blend alpha 8 at `z_rcp.c:824-835`.
  RT64 performs the comparison after combiner alpha in
  `shaders/RasterPS.hlsl:184,204-211`.
- **RGB/alpha dither** is selected by other-mode-high bits 6..7 and 4..5.
  Programming Manual 15.5.1 establishes screen-registered magic-square/Bayer
  routing, frame-varying long-period noise, alpha pattern/inverse/noise
  routing, and three-low-bit addition before RGB truncation. It does not
  publish the long-period noise generator. The reference rasterizer implements
  the screen-registered RGB MagicSquare/Bayer tables and alpha Pattern/
  InversePattern routing for one/two-cycle writes to its supported RGBA16 and
  RGBA32 color-image layouts. RGB Noise and alpha Noise consume the low three
  bits of the same per-fragment byte used by combiner NOISE and
  `G_AC_DITHER`; exact silicon stream identity remains hardware-trace work.
  The disabled memory path uses the known exact `>> 3` truncation for RGBA16
  RGB and RGBA32's five-bit memory alpha. Copy and fill cycles bypass the
  blender and are unaffected.
- **`G_FILLRECT`** (`0xF6`) snapshots the RDP state at its command position.
  Fill cycle uses the raw fill register and inclusive lower-right rule.
  One/two-cycle rectangles use exclusive lower-right bounds and the supported
  combiner, alpha-compare, primitive-depth, framebuffer-blender, ordered-
  dither, coverage, and color-write path. A rectangle whose combiner requires
  texture, shade, LOD, or an unavailable prior COMBINED value traps by
  source name; copy-cycle `G_FILLRECT` remains a loud gap.
- **`G_SETPRIMCOLOR`/`G_SETENVCOLOR`/`G_SETFOGCOLOR`/`G_SETFILLCOLOR`/
  `G_SETBLENDCOLOR`** (`0xFA`/`0xFB`/`0xF8`/`0xF7`/`0xF9`): flat color
  registers fed to the combiner/blender. Primitive and environment RGBA are
  decoded now; `G_SETPRIMCOLOR`'s low-byte primitive-LOD fraction is retained
  as a combiner input. Blend and fog RGBA are decoded for the framebuffer
  blender; fill color drives format-correct fill-cycle writes. All remain
  selected across task boundaries until another RDP command replaces them.
- **Sync ops** `G_RDPLOADSYNC`/`G_RDPPIPESYNC`/`G_RDPTILESYNC`/
  `G_RDPFULLSYNC` (`0xE6`-`0xE9`) and `G_NOOP`/`G_SPNOOP`
  (`0x00`/`0xE0`): pipeline hazard barriers / no-ops. The three hazard-only
  barriers are explicit validated commands in the atomic HLE path;
  `G_RDPFULLSYNC` remains an ordered operation boundary. `G_NOOP` accepts the
  public `gDPNoOpTag` second word, while `G_SPNOOP` requires both reserved
  payloads to be zero. Raw RDP Load/Pipe/Tile/Full Sync accepts an arbitrary
  second word because SGI *RDP Command Summary* assigns only the opcode field;
  the atomic F3DEX2 macro path still validates its reserved second word as
  zero. None falls through an unknown-opcode path.

### 5.4 Chroma key

`G_SETKEYR` (`0xEB`) and `G_SETKEYGB` (`0xEA`) persist three channel centers,
eight-bit scales, and twelve-bit 4.8 widths. `G_CK_KEY` enables the public
two-stage path: the combiner computes `(pixel - CENTER) * SCALE`, then alpha
fixup computes each channel as `clamp(width - abs(key prime), 0, 1)` and uses
their minimum as fragment alpha. A width greater than 1.0 disables that
channel. The wire fields, formula, and two-cycle constraint come from SGI
[*RDP Command Summary*, Tables 29-30](https://ultra64.ca/files/documentation/silicon-graphics/SGI_RDP_Command_Summary.pdf)
and Programming Manual
[Chapter 12.6](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro12/12-06.html).
Raw and F3DEX2 streams share the typed state and alpha-fixup path; an
end-to-end raw command gate proves the resulting alpha reaches alpha compare.
Exact internal arithmetic precision still requires hardware traces.

### 5.5 Othermode + framebuffer blender

The RDP framebuffer blender is selected by the two othermode words. OoT uses
both the full `G_RDPSETOTHERMODE` command (`0xEF`) and F3DEX2's partial
`G_SETOTHERMODE_L/H` commands (`0xE2`/`0xE3`):

- `G_RDPSETOTHERMODE` replaces the high 24-bit word from `w0[23:0]` and the
  full low word from `w1` (`gbi.h` lines 3697-3737).
- A partial setter stores `size-1` in `w0[7:0]` and
  `32-logical_shift-size` in `w0[15:8]` (`gbi.h` lines 3353-3369). Decode
  those fields back to a logical bit range and patch only that range.
- Cycle type is high-othermode bits 20-21: one cycle, two cycles, copy, or
  fill (`gbi.h` lines 519 and 527-531). Copy/fill bypass the blender.
- Low-othermode bits 31-16 hold two four-selector tuples. `GBL_c1` places
  `(P,A,M,B)` at shifts `(30,26,22,18)` and `GBL_c2` at
  `(28,24,20,16)` (`gbi.h` lines 612-627). Each active cycle evaluates:

  ```text
  color = P * A + M * B
  ```

  `P`/`M` can select combined (the incoming fragment in cycle 1, the prior
  blender result in cycle 2), framebuffer memory, blend color, or fog color.
  `A` can select combined, fog, shade alpha, or zero; `B` can select one minus
  A, framebuffer alpha, one, or zero. `FORCE_BL` is low bit `0x4000`
  (`gbi.h` lines 593-622). Without it the final blender cycle is a P-input
  pass; in two-cycle mode cycle 1 can still perform fog before that pass.

The standard translucent-surface tuple is
`IN*A_IN + MEM*(1-A)`, so a half-alpha fragment retains half of the existing
framebuffer instead of replacing it. The software rasterizer snapshots this
minimal state per triangle because display lists may change othermode between
draws. Constant blend/fog colors come from `G_SETBLENDCOLOR`/`G_SETFOGCOLOR`
(`gbi.h` lines 3640-3656). RT64 independently models the same raw othermode
shadow, selector ordering, final-cycle bypass, and sequential cycle handoff
(`shared/rt64_other_mode.h` lines 14-52;
`shared/rt64_blender.h` lines 45-81 and 366-504). RT64 represents a selected
framebuffer term as a source color plus a separate blend alpha (lines
414-424), then its graphics pipeline composites with `SRC1_ALPHA` /
`INV_SRC1_ALPHA` (`render/rt64_raster_shader.cpp` lines 332-339). This
software framebuffer performs that last composite directly. `G_BL_A_MEM`
means memory coverage alpha, not the stored RGBA byte. The software
framebuffer now retains the actual 1..8 count and supplies that normalized
value to this selector. `CVG_DST_CLAMP/WRAP/FULL/SAVE`, `CLR_ON_CVG`,
`CVG_X_ALPHA`, and `ALPHA_CVG_SEL` feed the same typed coverage path. RGBA16
stores `coverage - 1`: its high bit occupies the visible word's LSB and its
low two bits share the physical-address hidden-bit sidecar with depth DeltaZ.
RGBA32 instead stores those three bits in the alpha byte's MSBs and the
five-bit memory alpha in its LSBs, so it does not consume the hidden sidecar.
The disabled-dither store truncates eight-bit components to those five-bit
fields; it does not round to the nearest bit-replicated display value.
An observed CPU-visible word change reconstructs both hidden bits from the new
LSB per Programming Manual 15.5.6. A CPU rewrite of the *same* visible value
cannot yet be observed without moving this sidecar into the runtime RDRAM
write path. Programming Manual 15.5.4/15.7 and SGI *RDP Command Summary*
Table 20 prove the coverage/alpha selector topology and normalized product;
15.5 also states that blender alpha muxes have five-bit resolution. None of
those sources publishes the coverage multiplier width, whether unity is
encoded with a `/255` correction or binary `/256`, the product quantizer/tie
rule, or the conversion from one-through-eight coverage to the selected alpha
code. The current reference policy remains nearest `/255` and normalized-u8.
Focused fixtures bracket values that distinguish nearest/truncate and
`/255`/`/256`, while the silicon-vector inventory specifies a threshold sweep
that can recover the selected alpha without blender arithmetic. Exact
alpha-times-coverage rounding, the high-level covered-subpixel selector and
correction arithmetic, and fixed-width raw edge accumulators remain hardware-
differential frontiers.

The blender consumes the alpha produced by the color combiner. Both programmed
combiner cycles, primitive/environment registers, shade, texels, LOD, chroma
key, and conversion constants feed that result. The hardware NOISE source is
retained as a distinct selector but traps when evaluated: the public manual
specifies a long-period value that varies between frames but not its generator
state, so substituting zero would fabricate stable black input. The OOTU
C-lane trace through 1,000 VI swaps produced no fractional combined-alpha
fragments; selector wiring is fixture-verified, while live translucency still
needs a visual differential.

---

## 6. OoT specifics (the minimal command set for a recognizable frame)

### 6.1 What OoT's draw path emits

OoT builds its frame through two main display-list buffers appended to the
`GraphicsContext`: **`POLY_OPA_DISP`** (opaque polygons) and
**`POLY_XLU_DISP`** (translucent), plus `OVERLAY_DISP` (HUD) and `WORK_DISP`.
The `gSPxxx`/`gDPxxx` macros in the decomp's `gfx.h` append to these. For the
3D world, the dominant commands are exactly the geometry/transform set:

- **View setup** (`z_view.c` / `View_Apply*`): loads the **projection**
  (`guPerspective` → `G_MTX` PROJECTION) and the **viewport**
  (`G_MOVEMEM`/`G_MV_VIEWPORT`), sets the scissor (`G_SETSCISSOR`), and the
  render mode (`G_SETOTHERMODE_*`).
- **Per-object/limb** (skeleton draw, `z_skelanime.c`; actor draws): a stack
  of `G_MTX` (modelview, MUL/LOAD + PUSH) around `G_DL` calls into each limb's
  mesh DL, balanced by `G_POPMTX`.
- **Per-mesh**: `G_VTX` (load a batch of ≤32 verts) then a run of
  `G_TRI1`/`G_TRI2` referencing them; interleaved with `G_TEXTURE` /
  `G_SETTILE` / `G_LOADBLOCK` / `G_SETCOMBINE` for textured surfaces
  (skippable for flat-shaded).
- **Segment setup**: `G_MOVEWORD`/`G_MW_SEGMENT` establishes the scene/room/
  object/display-buffer segments before the draws that reference them.

### 6.2 The render target OoT sets

- Framebuffer: **320×240, RGBA5551** (`G_SETCIMG` size=16-bit).
- Z-buffer: **320×240, 16-bit** (`G_SETZIMG`).
- Viewport: full-screen centered (scale ≈160/120 px, section 3.5).
- Projection: perspective (`guPerspective`), FOV ~60°, with OoT's near/far.

### 6.3 Minimal decode set for a recognizable OoT frame

To get *recognizable geometry* (correct silhouettes, correct positions, flat
vertex-colored), a decoder must correctly handle:

1. `G_MOVEWORD` / **`G_MW_SEGMENT`** — segment table (or nothing resolves).
2. `G_MTX` (projection + modelview, LOAD/MUL/PUSH) + `G_POPMTX`.
3. `G_MOVEMEM` / **`G_MV_VIEWPORT`** (or fall back to a 320×240 default).
4. `G_VTX` (F3DEX2 count/end-index layout + the 16-byte `Vtx`).
5. `G_TRI1` / `G_TRI2` / `G_QUAD` (7-bit index fields).
6. `G_DL` (push/branch) + `G_ENDDL` (return).
7. `G_GEOMETRYMODE` — enough to honor **back-face culling**.

That list is the historical first-frame minimum. The reference backend now
also implements texture/lighting, RDP other-mode and alpha compare, and the
color-combiner, framebuffer, depth, scissor, texture-rectangle, raw-triangle,
  and line paths described above. Remaining unsupported encodings trap as
  enumerated in section 7.

---

## 7. Cross-reference: what `crates/fn64-render-rt64/src/gbi.rs` already names

`gbi.rs` already declares these opcode constants and has handler scaffolding.
This maps each to the section above and flags where the current handler logic
needs attention against the F3DEX2 contract:

| `gbi.rs` const | byte | spec § | handler note |
|----------------|------|--------|--------------|
| `G_VTX` | `0x01` | 2.1 | Implemented with `n = field(w0,12,8)` and `v0 = field(w0,1,7) - n`; transformed vertices retain screen values, raw screen Z, six clip codes, and homogeneous position for later control flow/clipping. |
| `G_MODIFYVTX` | `0x02` | 2.1 | Implemented for all four public final-cache destinations: RGBA bytes, signed S10.5 ST, signed S13.2 screen XY, and unsigned 16.16 screen Z. The packed slot is decoded from `vtx*2`; malformed slots/destinations trap by name. |
| `G_TRI1` | `0x05` | 2.2 | Implemented with three 7-bit cache-slot fields at bits 17/9/1; no legacy byte `/2` reinterpretation. |
| `G_TRI2` | `0x06` | 2.3 | Two triangles: tri A in `w0`, tri B in `w1`, same field positions. |
| `G_QUAD` | `0x07` | 2.3 | Decode identically to `G_TRI2`. |
| `G_LINE3D` | `0x08` | 2.6 | Implemented as a typed clipped line with public width, shade/texture attributes, scissor, coverage, blender, and read-only depth behavior. |
| `G_TEXTURE` | `0xD7` | 5.2 | Decodes enable, tile, and S/T scales. |
| `G_SETOTHERMODE_L/H` | `0xE2`/`0xE3` | 5.2 | Masked F3DEX2 H/L updates are retained in typed render state and snapshotted per primitive. Threshold and pseudo-random dither alpha compare are implemented; reserved mode 2 traps by name. The deterministic seedable noise stream is a reference policy, not a silicon-sequence claim. |
| `G_SETCOMBINE` | `0xFC` | 5.2 | Decodes both cycles' position-specific RGB/alpha selectors and snapshots them per triangle. |
| `G_CULLDL` | `0x03` | 2.5 | Implemented with retained six-plane clip codes and inclusive `v*2` cache bounds; a common outside plane ends only the current display list. |
| `G_BRANCH_Z` | `0x04` | 2.5 | Implemented as the conditional tail half of `G_RDPHALF_1` + `G_BRANCH_Z`, using retained unsigned 16.16 screen Z. |
| `G_POPMTX` | `0xD8` | 3.3 | Implemented: require a nonzero multiple of 64 and pop exactly `w1 / 64` modelview entries. |
| `G_GEOMETRYMODE` | `0xD9` | 2.4 | Implemented state update; cull, lighting, fog, automatic regular/linear texture generation, Z-buffer, shade, and smooth-shading consumers use the retained bits where their corresponding render mechanisms exist. |
| `G_MTX` | `0xDA` | 3.2 | Un-XOR the push bit (`params = field(w0,0,8) ^ 0x01`), then PROJECTION=`0x04`/LOAD=`0x02`/PUSH=`0x01`. Confirm the transpose-on-read matches the multiply convention (3.1). |
| `G_MOVEWORD` | `0xDB` | 1.3 | All public F3DEX2 destinations are implemented: segment, four clip-ratio sides, active-light-count, both light-color copies, fog, force-matrix activation, and perspective normalization. Non-public/raw indices are malformed-command frontiers. |
| `G_MOVEMEM` | `0xDC` | 1.4 | Viewport, both LookAt directions, directional/ambient lights, and the 64-byte force-matrix DMA are implemented. Non-public point/raw matrix subindices are named traps. |
| `G_DMA_IO` | `0xD6` | 1.5 | Implemented against persistent DMEM/IMEM and logical RDRAM bytes in command order; alignment, size, bank, and RDRAM bounds trap by name. |
| `G_LOAD_UCODE` | `0xDD` | 1.6 | Loads the declared data prefix and fixed 4 KiB text image into persistent DMEM/IMEM in command order, applies the public F3DEX2 maintained-state contract, and validates the compound wire form. |
| `G_DL` | `0xDE` | 1.1 | Push flag = `field(w0,16,8)` bit `0x01`. **Do not push/pop the matrix stack across `G_DL`** — only the return address is saved; matrix state is intentionally global (see 1.1). |
| `G_ENDDL` | `0xDF` | 1.2 | Return to caller (pop DL stack) or stop. |
| `G_SETSCISSOR` | `0xED` | 4.1 | Decode all four 12-bit quarter-pixel edges, snapshot the rect per triangle, and clip raster bounds to its exclusive lower-right edge. |

The `opcode_name` table in `gbi.rs` additionally names `G_NOOP`, `G_SPNOOP`,
`G_RDPHALF_1/2`, `G_SETOTHERMODE_L/H`, the four sync ops, `G_LOADBLOCK`,
`G_SETTILE`, `G_SETCOMBINE`, and `G_SETTIMG`. Public `G_NOOP`/`G_SPNOOP`
and the hazard sync commands are explicit validated handlers. The texture, combiner,
other-mode, framebuffer-image, synchronization, DMA, and self-load commands
required by the reference executor are handled as described above.

There is no remaining rate-limited opcode skip path. `G_SPECIAL_1/2/3`
(`0xD5`/`0xD4`/`0xD3`) are reserved by the public header and trap with the
command words. Unsupported `G_MOVEWORD`/`G_MOVEMEM` indices and any
unrecognized opcode likewise trap with index/address context. A new public
microcode contract must provide semantics before one of those encodings can
become an effectful handler.

---

## 8. Summary: minimal set + top failure risks

### Minimal command set for a first recognizable flat-shaded OoT frame

1. `G_MOVEWORD` / `G_MW_SEGMENT` (segment base table)
2. `G_MTX` (projection + modelview, LOAD/MUL/PUSH) and `G_POPMTX`
3. `G_MOVEMEM` / `G_MV_VIEWPORT` (or a 320×240 centered default)
4. `G_VTX` (F3DEX2 `n = field(w0,12,8)`, `v0 = field(w0,1,7) - n`; 16-byte `Vtx`)
5. `G_TRI1` / `G_TRI2` / `G_QUAD` (three 7-bit index fields at bits 17/9/1)
6. `G_DL` (push/branch) and `G_ENDDL` (return)
7. `G_GEOMETRYMODE` — only enough to honor `G_CULL_BACK`
8. `G_SETSCISSOR` — quarter-pixel `[upper-left, lower-right)` raster clip

Unsupported encodings trap rather than being acknowledged-and-skipped;
decoded texture, lighting, other-mode, combiner, blender, and scissor state
feed the raster path above.

### Top 3 things most likely to be wrong/incomplete in a naive rasterizer

1. **Segmented-address resolution.** Every vertex/matrix/DL/framebuffer
   pointer is a `0xSSoooooo` segmented address that must be resolved through
   the `G_MW_SEGMENT`-populated 16-entry base table (segment 0 = identity).
   Forgetting this, or masking the wrong number of bits (24 for a 4 MiB image),
   makes every load read garbage — no geometry at all. Handle `G_MW_SEGMENT`
   *before* the first load that needs it.

2. **The F3DEX2 `G_VTX` / `G_TRI` bit-field layout (vs F3DEX).** F3DEX2 packs
   `G_VTX` as `n = field(w0,12,8)`, end-index `field(w0,1,7)` (so
   `v0 = end - n`), and triangle indices as **7-bit** fields at bits 17/9/1
   (already slots). A reader copied from an F3DEX/SDK-macro shape
   (`((w0>>16)&0xFF)/2`, byte-and-divide-by-2) misplaces vertices and mis-reads
   indices — the geometry loads into the wrong cache slots and triangles
   reference the wrong vertices. This is the single most likely silent
   correctness bug and the one to test first against a real OoT DL.

3. **The `Mtx` fixed-point split + the perspective divide.** (a) The 64-byte
   `Mtx` stores all 16 integer parts (s16) first, then all 16 fractional
   parts (u16); reassemble each element as **one** `(int<<16)|frac`
   two's-complement 32-bit value then `/65536` — computing `int + frac/65536`
   as separate floats mis-signs negative elements. (b) After `MVP · v`, you
   **must** perspective-divide by `w` (`ndc = clip.xyz / clip.w`), guarding
   `w ≈ 0`; skipping it flattens the scene, and a wrong transpose/multiply
   convention mirrors it. Also honor the N64 **top-down Y** in the viewport
   (flip Y exactly once) or the frame renders upside-down.
```
