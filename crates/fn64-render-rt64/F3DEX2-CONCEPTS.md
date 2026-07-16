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
- **Priority:** the vertex + matrix + triangle path (this is what produces
  *visible geometry*). Texture and color-combiner handling are marked
  **phase-2** and can be stubbed for a first recognizable flat-shaded frame.

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
| `G_MW_FOG` | `0x08` | fog multiplier/offset (phase-2). |
| `G_MW_LIGHTCOL` | `0x0A` | a light's color (phase-2). |
| `G_MW_PERSPNORM` | `0x0E` | perspective-normalization scalar (a small integer scale hint; can be ignored for a first frame). |
| `G_MW_FORCEMTX` | `0x0C` | force the MVP recompute flag (F3DEX2-specific; ignore for first frame). |

For a flat-shaded first frame, **only `G_MW_SEGMENT` matters**; the rest can
be acknowledged-and-skipped.

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
- Lights (`G_MV_LIGHT`) are phase-2.

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
| `G_SHADING_SMOOTH` | `0x00200000` | Gouraud (smooth) vs flat shading |

**For a first frame:** you can ignore lighting/fog/shade-smooth. The one bit
worth honoring early is **back-face culling** (`G_CULL_BACK`), because without
it, back faces overpaint front faces and models look inside-out/garbled. Cull
by the sign of the screen-space triangle's signed area (winding); which sign
is "back" depends on the N64's convention (Y is down in screen space — see
3.5), so if culling removes the wrong half, flip the test.

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

Behavior:

- **Projection** (`PROJECTION` set): replace (or, rarely, multiply) the single
  projection matrix. OoT loads projection once per view via `guPerspective`
  output. Push/pop does not apply to projection.
- **Modelview** (`PROJECTION` clear):
  - If `PUSH`: save the current top modelview onto the stack first.
  - If `LOAD`: replace the current modelview with the loaded matrix.
  - Else (`MUL`): `modelview = modelview · loaded` (concatenate — this is how
    a hierarchy of local transforms is built, e.g. a limb relative to a body).
- After any change, **recompute MVP = projection · modelview** (or apply them
  in sequence at vertex time). This cached product is what `G_VTX` uses.

### 3.3 `G_POPMTX` — pop the modelview stack (opcode `0xD8`)

Restores a previously pushed modelview. **F3DEX2 layout:** `w1` holds the
number of matrices to pop **× 64** (each stack entry is a 64-byte matrix), so
`count = w1 >> 6`. Pop that many entries and set the modelview to the last one
popped, then recompute MVP. (The modelview stack is the classic
push-a-transform / draw-children / pop pattern; the projection matrix has no
stack.)

The F3DEX2 modelview stack holds **up to 18** matrices.

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
  ×4 encoding), translate ≈ (160, 120) center (`vtrans ≈ (640, 480)`). If no
  viewport has been loaded yet, a **320×240, origin-center default** (scale
  160/120, translate 160/120) is a reasonable stand-in and matches OoT's
  actual full-screen view closely enough to get recognizable geometry.
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
  of all three edges. Hardware uses a specific tie-break (top-left-style rule)
  and sub-pixel coverage for antialiasing; a first-pass renderer can use the
  standard **edge-function (barycentric) test**: for edges `e0,e1,e2`
  evaluated at pixel center, the pixel is inside when all three edge functions
  share the sign of the triangle's signed area. Sample at pixel-center
  (`x+0.5, y+0.5`). This is exactly the textbook Pineda edge-function fill and
  is not N64-specific.
- **Attribute interpolation:** compute barycentric weights from the edge
  functions and interpolate vertex color (and, phase-2, S/T and depth)
  perspective-*in*correctly for a first frame (screen-linear). True
  perspective-correct interpolation divides attributes by `w` per-pixel;
  defer that to phase-2 (it mainly matters for textures at glancing angles).

### 4.2 Degenerate / cull handling

- **Zero-area** (collinear) triangles: skip (they contribute nothing).
- **Back-face culling** (if `G_CULL_BACK` is set, 2.4): drop triangles whose
  screen-space signed area has the "back" sign. Do this *before* the fill loop.

### 4.3 Z-buffer compare

- OoT uses a **16-bit z-buffer** (`G_SETZIMG` points at it; `G_ZBUFFER`
  geometry-mode bit + the render mode's z-compare/z-update bits enable it).
- Per covered pixel: compute the interpolated depth, compare against the
  stored depth (default **less-than passes**, nearer wins), write color + new
  depth on pass. For a first frame a simple `f32` depth buffer with
  less-than-passes is adequate — you just need consistent occlusion so far
  geometry doesn't overwrite near geometry.
- If you skip the z-buffer entirely, draw order artifacts appear (later
  triangles overwrite earlier regardless of depth). Acceptable for a very
  first "is anything recognizable" frame, but the z-buffer is the first thing
  to add after triangles show up.

### 4.4 The framebuffer format — RGBA5551, 16-bit, 320×240

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
  z-buffer address in `w1`.

---

## 5. Texture / combiner (phase-2 — summarize + stub)

None of this is needed for a first flat-shaded frame; a decoder should
**acknowledge-and-skip** each of these (loudly, by name) and render geometry
from vertex color only. Captured here so phase-2 knows the contract.

### 5.1 Texture image + tile + TMEM load

- **`G_SETTIMG`** (`0xFD`, RDP): sets the source texture image in RDRAM —
  format `field(w0,21,3)` (RGBA/CI/IA/I), size `field(w0,19,2)` (4/8/16/32
  bpp), width `field(w0,0,12)+1`, address `w1` (segmented). Just a pointer +
  format latch; no data moves yet.
- **`G_SETTILE`** (`0xF5`, RDP): defines a **tile descriptor** — where in the
  RDP's 4 KiB **TMEM** the texture lives, its format/size, line stride, palette
  index, and the S/T clamp/mirror/wrap + mask/shift parameters. Up to 8 tiles.
- **`G_LOADBLOCK`** (`0xF3`) / **`G_LOADTILE`** (`0xF4`): DMA texel data from
  the `G_SETTIMG` image into TMEM for a tile. `G_LOADBLOCK` loads a linear run
  (fast, whole texture); `G_LOADTILE` loads a rectangular sub-region.
  `G_SETTILESIZE` (`0xF2`) sets the tile's active S/T extent.
- **`G_LOADTLUT`** (`0xF0`): load a color palette into TMEM for CI (color-
  indexed) textures.
- To *sample*: take the vertex S/T (S10.5 fixed-point from the `Vtx`), apply
  the tile's shift/mask/clamp, address TMEM, decode the texel per the tile
  format. OoT textures are commonly 4-bit/8-bit CI and 16-bit RGBA5551/IA.

### 5.2 Color combiner + `G_TEXTURE`

- **`G_SETCOMBINE`** (`0xFC`, RDP): programs the **color combiner** — a
  two-cycle formula `(A - B) * C + D` for RGB and separately for alpha, where
  A/B/C/D are selected from a fixed menu of inputs (combined/texel0/texel1/
  primitive/shade/env colors, etc.). This is *the* op that decides whether a
  pixel's color comes from the texture, the vertex/shade color, a flat prim
  color, or a blend. **With `G_SETCOMBINE` skipped, use vertex/shade color
  directly** — which is exactly the flat-shaded first-frame behavior.
- **`G_TEXTURE`** (`0xD7`, RSP): enables texturing and sets the S/T
  coordinate scale applied to vertex texcoords, plus which tile + mip level.
  Fields: on-bit `field(w0,1,7)`, mip level `field(w0,11,3)`, tile
  `field(w0,8,3)`, S scale `field(w1,16,16)`, T scale `field(w1,0,16)` (both
  U0.16). Skip for flat-shaded; needed to place textures correctly in phase-2.
- **`G_SETOTHERMODE_H/L`** (`0xE3`/`0xE2`): the RDP "other modes" — render
  mode (blend/z-compare/AA/cvg), texture filter, cycle type (1-cycle/2-cycle/
  copy/fill), etc. The **render mode** (in the low word) is what actually
  enables z-compare and alpha blending. The renderer now decodes the minimum
  cycle/blender subset described in 5.3; the remaining othermode fields belong
  to the fuller othermode pass.
- **`G_SETPRIMCOLOR`/`G_SETENVCOLOR`/`G_SETFOGCOLOR`/`G_SETFILLCOLOR`/
  `G_SETBLENDCOLOR`** (`0xFA`/`0xFB`/`0xF8`/`0xF7`/`0xF9`): flat color
  registers fed to the combiner/blender. Blend and fog colors are now decoded
  for 5.3; primitive/environment/fill colors remain combiner work.
- **Sync ops** `G_RDPLOADSYNC`/`G_RDPPIPESYNC`/`G_RDPTILESYNC`/
  `G_RDPFULLSYNC` (`0xE6`-`0xE9`) and `G_NOOP`/`G_SPNOOP`
  (`0x00`/`0xE0`): pipeline hazard barriers / no-ops. **Always safe to skip**
  in an HLE decoder — they exist to serialize the real RDP pipeline and have
  no effect on the decoded geometry.

### 5.3 Othermode + framebuffer blender

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
means coverage alpha, not the stored RGBA byte; because coverage is not yet
emulated, it is treated as full coverage, matching RT64 lines 351-357.

The blender consumes the alpha produced by the color combiner. Until the
separate `G_SETCOMBINE`/primitive/environment-color work lands, this renderer's
"combined alpha" remains the existing shade × texel approximation. The OOTU
C-lane trace through 1,000 VI swaps produced no fractional combined-alpha
fragments, so the equation and selector wiring are fixture-verified here but
live OoT translucency remains visually blocked on that combiner input.

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

Everything else (`G_TEXTURE`, `G_SETTILE`, `G_SETCOMBINE`, `G_SETTIMG`,
`G_LOAD*`, `G_SETOTHERMODE_*`, `G_SETPRIMCOLOR`, all sync ops, lighting)
can be **acknowledged-and-skipped** for the first frame — geometry renders
flat-shaded from vertex color.

---

## 7. Cross-reference: what `crates/fn64-render-rt64/src/gbi.rs` already names

`gbi.rs` already declares these opcode constants and has handler scaffolding.
This maps each to the section above and flags where the current handler logic
needs attention against the F3DEX2 contract:

| `gbi.rs` const | byte | spec § | handler note |
|----------------|------|--------|--------------|
| `G_VTX` | `0x01` | 2.1 | The F3DEX2 `decode_display_list_f3dex2` path reads `n = field(w0,10,6)` and `v0 = field(w0,16,8)/2`. **Per the F3DEX2 contract, `n = field(w0,12,8)` and `v0 = field(w0,1,7) - n`.** Verify against a real OoT DL; the `/2` form is F3DEX-shaped and can misplace vertices. (Failure risk #2.) |
| `G_TRI1` | `0x05` | 2.2 | Current path extracts three 8-bit bytes and `/2`. The F3DEX2 layout is three **7-bit** fields at bits 17/9/1; each is already the slot. The `/2`-of-a-byte form and the 7-bit-field form agree only if the byte's low bit is 0 — validate. |
| `G_TRI2` | `0x06` | 2.3 | Two triangles: tri A in `w0`, tri B in `w1`, same field positions. |
| `G_QUAD` | `0x07` | 2.3 | Decode identically to `G_TRI2`. |
| `G_TEXTURE` | `0xD7` | 5.2 | Phase-2. Currently skipped (correct for flat-shaded). |
| `G_POPMTX` | `0xD8` | 3.3 | `count = w1 >> 6`; pop that many modelview entries. |
| `G_GEOMETRYMODE` | `0xD9` | 2.4 | Currently skipped. To get culling, apply `mode = (mode & field(w0,0,24)) | w1` and honor `G_CULL_BACK`. |
| `G_MTX` | `0xDA` | 3.2 | Un-XOR the push bit (`params = field(w0,0,8) ^ 0x01`), then PROJECTION=`0x04`/LOAD=`0x02`/PUSH=`0x01`. Confirm the transpose-on-read matches the multiply convention (3.1). |
| `G_MOVEWORD` | `0xDB` | 1.3 | `G_MW_SEGMENT=0x06`: seg = `field(w0,2,4)` (= `offset/4`), base = `w1 & 0x00FF_FFFF`. |
| `G_MOVEMEM` | `0xDC` | 1.4 | Currently skipped → viewport falls back to a 320×240 default. To honor OoT's real viewport, handle `G_MV_VIEWPORT` (index `field(w0,0,8)==8`) and parse the `Vp` (÷4). |
| `G_DL` | `0xDE` | 1.1 | Push flag = `field(w0,16,8)` bit `0x01`. **Do not push/pop the matrix stack across `G_DL`** — only the return address is saved; matrix state is intentionally global (see 1.1). |
| `G_ENDDL` | `0xDF` | 1.2 | Return to caller (pop DL stack) or stop. |

The `opcode_name` table in `gbi.rs` additionally names `G_NOOP`, `G_SPNOOP`,
`G_RDPHALF_1/2`, `G_SETOTHERMODE_L/H`, the four sync ops, `G_LOADBLOCK`,
`G_SETTILE`, `G_SETCOMBINE`, `G_SETTIMG` — all correctly in the
"acknowledge-and-skip for a flat-shaded frame" bucket (sections 5).

**Not yet named in `gbi.rs`** but part of the F3DEX2 map, worth adding as
named skips so coverage isn't overstated: `G_MODIFYVTX` (`0x02`),
`G_CULLDL` (`0x03`), `G_BRANCH_Z` (`0x04`), `G_LINE3D` (`0x08`),
`G_LOAD_UCODE` (`0xDD`), `G_SPECIAL_1` (`0xD5`), `G_DMA_IO` (`0xD6`), and the
RDP framebuffer ops `G_SETCIMG` (`0xFF`), `G_SETZIMG` (`0xFE`),
`G_SETSCISSOR` (`0xED`).

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

Everything else acknowledged-and-skipped → flat-shaded from vertex color.

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
