# Raw triangle -> guest RDRAM: design record

## Facts verified by reading (worktree /private/tmp/fn64-triwb @ 89090522)

1. `plan_render_target_rows` (raw_dpc/mod.rs:1701) ALREADY emits per-row disjoint
   ResourceAccess + a FillAccessSpan from a `RenderTargetRectangle`. Shared by
   plan_fill and plan_texture_rectangle. Full-width -> 1 access; partial-width ->
   N accesses strided by image width. The brief's "declare per-row" requirement is
   already satisfied by this helper. It is generic over the rectangle.

2. Decoder arm 0x08..=0x0f (raw_dpc/mod.rs:1389-1399) decodes RawTriangle and
   pushes the command only. No planner call. Confirmed.
   production_adapter.rs:995 sets `texrect_accesses: None` with the comment
   "A RawTriangle pushes zero accesses, so there is no span to carry".

3. validate_effects (fn64-render-ir/src/ticket.rs:650-669): equal length check
   THEN positional `*expected != actual.access`. So declared accesses need real
   CompletedWrites, one per access, in order.

4. **The guest-visible path is a CPU rasterizer, not the GPU.**
   `execute_texture_rectangle` (targets/texrect.rs:1214) is a scalar
   `for row { for column { sample_point -> combine -> blend_and_write_pixel } }`
   loop over a `Vec<u8>`. No wgpu. It returns CompletedColorTargetWrite.
   `stage_color_commands` (production.rs:3393) composes N of these into one
   accumulated buffer and calls `fill_completed_writes` once at the end.

5. **`draw_admitted_triangles` (production.rs:387) needs a real GPU.** It starts
   with `self.triangle_pipeline.as_mut().ok_or(TriangleDrawBeforeCreate)?`. So
   `triangle_draw_output` does not exist at all in the default-feature suite.

## Consequence: the briefed fork is not the real fork

The brief offered (a) read the GPU raster back and commit it as guest writes, or
(b) raster directly into the ColorTargetRegistry colour target.

Both presuppose that a GPU raster result exists to be routed. It does not exist
in the configuration that the 8499-test default suite runs, because
`draw_admitted_triangles` refuses without a device. So:

- (a) readback-and-copy is only reachable under `--features host-gpu-tests` on a
  real adapter. Its correctness could then only ever be asserted as a
  one-adapter claim, and every default-feature test of it would be vacuous.
  It also adds a per-draw GPU stall, which the brief itself flags.
- (b) "raster directly into the colour target" is the same thing said differently
  when the target lives in guest RDRAM: the ColorTargetRegistry's device_bytes
  ARE a CPU Vec<u8>. So (b) collapses into "produce the triangle's pixels on the
  CPU, the way a texrect already does".

The real fork is therefore:
  (i)  route the GPU raster into the journal  [needs a GPU, one-adapter only]
  (ii) give the raw triangle the SAME CPU rasterizer seam the texrect uses
       [device-free, testable in the default suite, reuses the proven path]

## Chosen: (ii)

Reasons, correctness first:
- The journal cannot lie. Declared accesses need matching CompletedWrite content
  at the same positions. The only content producer that exists without a GPU is
  the CPU loop in texrect.rs. Choosing (i) makes the journal's truth conditional
  on an adapter being present.
- The byte-lane hazard is already handled correctly by the existing path
  (DeviceColorBytes -> fill_completed_writes -> copy_committed_guest_writes).
  Reusing it inherits that correctness rather than re-deriving it.
- present() stays untouched: the triangle's pixels land in the exact
  ColorTargetRegistry buffer present already accepts.

Cost of (i), stated plainly: a per-draw readback stall on every triangle, plus a
journal whose declared writes can only be satisfied when a GPU adapter exists,
which would make the default-feature suite unable to test the guest-visible
behaviour at all.

## Sizing of (ii), measured by reading

The chosen design has two halves, and they are very different sizes.

### Half A -- the journal (declared writes). Decode-time. Small.
Per-scanline covered X range is pure arithmetic over the triangle's own edge
coefficients (yh/ym/yl, xh/xm/xl, dxhdy/dxmdy/dxldy, lft), all already decoded by
`RawTriangle` (raw_dpc/triangle.rs:266-306). The reference lane's
`raw_span_edges_at_y_eighth` (fn64-render-reference/src/raster/mod.rs:303) plus
`draw_raw_rdp_triangle_impl`'s min_x/max_x derivation (raster/draw.rs:820-866) is
the whole algorithm, ~60 lines.
NOT reusable directly: fn64-render-reference is a DEV-dependency of
fn64-render-wgpu (Cargo.toml:37) and every one of these fns is `pub(super)`.
Promoting it to a production dependency would put the software reference
renderer back into the all-Rust stack, which is exactly what the goal excludes.
So this must be written fresh in-crate. `Q16_ONE`/`fixed_mul_ratio`/`ceil_ratio`
do not exist anywhere in fn64-render-wgpu -- checked.

### Half B -- the content (real CompletedWrite bytes). Large.
validate_effects needs a real write per declared access. Producing one means
per-pixel: coverage mask (4x2 subsamples), shade plane interpolation, texture
s/t/w plane interpolation + perspective divide, LOD, the combiner, the blender,
depth. The reference's version is raster/draw.rs:805-1000+, backed by its own
oracle suite. The texrect CPU loop (targets/texrect.rs:1288-1370) supplies only
the tail of that (combine_one_texel + blend_and_write_pixel) and takes its s/t
from a linear TexrectDraw, which a triangle does not have.

### Honest conclusion on scope
Half A is a real, self-contained, testable deliverable that removes the
structural blocker named in the brief ("a RawTriangle pushes NO ResourceAccess
at all"). Half B is a software triangle rasterizer -- a port-sized effort, not a
lane-sized one, and it is the actual remaining distance to pixels on screen.
Landing Half A alone WITHOUT Half B would make the journal WORSE, not better:
declared accesses with no CompletedWrite to satisfy them fail validate_effects'
length check and would break every currently-passing raw-triangle packet.
So Half A must not be landed as a production decoder change on its own.

## Confirmations from a deep read of the texrect path

- `fill_completed_writes` (production.rs:4059) maps physical->buffer with
  `start = range.start() - key.address()`; `len = range.len()`. It slices the
  FULL-EXTENT device buffer. It does NOT check that the declared run matches
  what the raster loop touched -- it will happily slice bytes the loop never
  wrote. So declaring per-row accesses for a triangle over a buffer whose
  triangle pixels were never written yields CompletedWrites with REAL digests
  of STALE bytes. That passes validate_effects and copy_committed_guest_writes
  and writes the resident's old content back over guest RDRAM. This is a
  silent-wrong-answer path, worse than the current honest gap.
- `CompletedWrite::try_from_bytes` (fn64-render-ir/src/ticket.rs:315) enforces
  `byte_count == access.region().declared_bytes()`.
- `copy_committed_guest_writes` (fn64-abi/src/task_dispatch/rsp_commit.rs:1402)
  re-derives every digest from the payload before writing any byte, and writes
  via `RdramViewMut::write_logical_bytes` -- the correct XOR3 logical path. The
  byte-lane hazard is handled here and is inherited for free by any producer
  that goes through DeviceColorBytes.
- `DeviceColorBytes` is the whole framebuffer, row-major, tightly packed,
  big-endian per RGBA16 pixel, indexed `(y*extent.width()+x)*bpp`.

## A latent defect found while reading (inert today)

`RawTriangle::right_major()` (raw_dpc/triangle.rs:258) reads w0 bit 23 and
names it "right-major (flip)". `fn64-render-reference` reads the SAME bit
(gbi/entries.rs:596) and names it `left_major`. The reference carries an
explicit comment (raster/mod.rs:335-343) that it previously had this polarity
inverted, and that the inversion made "every real triangle's span come back
right < left = empty -- raw RDP geometry decoded but never rasterized a single
pixel", corrected against WM2000's own live stream.

`right_major()` currently has ZERO consumers in fn64-render-wgpu -- checked by
grep -- so the disagreement is inert. It becomes load-bearing the instant span
derivation lands, and it is exactly the bug that produces "geometry decodes but
nothing rasterizes". Recorded rather than changed: with no consumer there is no
behaviour to test, and renaming on inference alone is what the project's
evidence rules forbid.

## Option (a) fails on CORRECTNESS, not merely cost -- three independent reasons

Verified by reading the triangle pipeline end to end:

1. **Wrong pixel format.** `TriangleDrawOutput.color_rgba8` is
   `wgpu::TextureFormat::Rgba8Unorm` readback (triangle_pipeline.rs:144, struct
   at :1959). The guest framebuffer WM2000 programs is RGBA16 (5/5/5/1). A
   readback-and-copy would have to requantize 8888->5551, which is a lossy
   re-derivation of a value the RDP itself defines. That is inventing content.

2. **Wrong dimensions.** `triangle_target_extent` comes from `RenderConfig`
   (production.rs:311-316, "an identity mapping"), NOT from `SetColorImage`.
   The colour target's extent comes from `SetColorImage`'s own width
   (production.rs:3634-3638). These are two different sizes from two different
   sources; there is no guarantee they match, and nothing checks it. Copying
   one into the other is a stride bug waiting to happen -- exactly the class the
   brief's byte-lane hazard warns about.

3. **No adapter, no path.** `draw_admitted_triangles` returns
   `TriangleDrawBeforeCreate` when `triangle_pipeline` is `None`
   (production.rs:393-398). Confirmed: there is ZERO ungated test in the crate
   that rasters a raw triangle; every one is `#[cfg(feature="host-gpu-tests")]`.
   So under option (a) the entire guest-visible behaviour would be untestable in
   the 8499-test default suite, and every claim about it would be one-adapter.

Reason 3 alone makes option (a) unable to satisfy the brief's own evidence rule
("CPU-oracle only unless you actually run the Metal adapter").

## MEASURED: declaring a triangle write with no content producer breaks the suite

Probe (applied, measured, then reverted): the `0x08..=0x0f` arm was patched to
call the existing `plan_render_target_rows` with a one-row RenderTargetRectangle
whenever a compatible `SetColorImage` was staged. Nothing else changed.

`cargo nextest run -p fn64-render-wgpu --offline`
  BEFORE probe: 4743 passed / 3 skipped / 0 failed
  AFTER  probe: 4739 passed / 3 skipped / **7 FAILED**

Failing tests:
  production::tests::a_fill_composed_with_a_raw_triangle_is_still_refused_by_name
  production::tests::a_texrect_composed_with_a_trailing_raw_triangle_executes
  production::tests::compositions_this_slice_does_not_admit_still_fail_by_name
  production::tests::execute_raw_dpc_rejects_a_mixed_fill_and_triangle_packet
  production::tests::the_mixed_fixture_really_carries_a_texrect_and_a_raw_triangle
  raw_dpc::tests::base_edge_triangle_frames_exactly_against_a_following_full_sync
  raw_dpc::tests::fully_populated_triangle_frames_exactly_against_a_following_full_sync

Measured failure mode, one level EARLIER than predicted:
  "raw-DPC plan seal failed: raw-DPC plan writer accumulated access count is 6;
   exact journal requires 7"

I had predicted `MergedWriteUnclaimed`. The real gate is
`ExactRawDpcPlanWriter::finish`'s access-count check, which fires at plan-seal
time before execution is reached at all. Recording the correction: my prediction
named the right class (declared-without-content) and the wrong guard.

This is the empirical proof that Half A cannot land without Half B.

## The lft/right_major bit has no RT64 ground truth

Checked the pinned oracle checkout
(/Users/jer/Code/no-mercy-recompiled/third_party/rt64 @ f0728a2): RT64 does not
decode raw RDP triangle edge coefficients at all. It is an HLE renderer -- its
`drawRect`/`drawTexRect` `flip` parameter is the TEXRECT flip bit (rt64_rdp.cpp:1159,
1316), a different field entirely. Grep for bit-23 triangle decode across
rt64_rdp.cpp and rt64_rsp.cpp: no hits.

So the only evidence for this bit's polarity in the repo is
`fn64-render-reference`'s, which is EMPIRICAL: its comment
(raster/mod.rs:335-343) says the reading was corrected against WM2000's live
title-scene stream (task #783), where "its rect-split tris carry lft=1 with a
constant XH on the left and XM/XL marching right", and that the previous
inverted reading "made every real triangle's span come back right < left =
empty".

That is real evidence, from the target ROM, for the semantic the reference uses.
It contradicts the NAME `fn64-render-wgpu` gives the same bit. Since the wgpu
accessor has zero consumers, nothing is wrong today and there is no behaviour to
write a failing test against. Flagging, not changing.

## Mutation testing of the arm I am KEEPING (present's refusal + scanout)

Baselines measured on this worktree, both matching the brief:
  cargo nextest run --workspace --offline                       -> 8499 passed / 13 skipped
  cargo nextest run -p fn64-render-wgpu --features host-gpu-tests --offline
                                                                -> 4787 passed /  3 skipped
  cargo nextest run -p fn64-render-wgpu --offline                -> 4746 passed /  3 skipped

**M1** -- in `present` (production.rs:1970), drop the assignment
`self.presented_field = Some(field)` while still performing the scanout, so a
present silently keeps the previously presented field.

  focused (-p fn64-render-wgpu):  4746 passed / 0 failed  -> **SURVIVED**
  workspace:                      8498 passed / 1 failed  -> **KILLED**
  killing test: fn64-abi task_dispatch::tests::raw_dpc_session_integration::
                an_admitted_fill_presents_through_the_real_vi_retrace_path

Two findings:
1. The mutant IS killed, so the scanout-and-present arm is genuinely constrained
   by a test -- but only by an integration test in a DIFFERENT crate.
2. `fn64-render-wgpu`'s own 4746 tests do not constrain it at all. Anyone
   iterating with the focused suite -- which is the natural loop when working in
   this crate -- would see a total present regression stay green. This is the
   brief's "verify both configurations" hazard, found in a third configuration
   it did not name (focused vs workspace, not default vs host-gpu-tests).

**M2** -- in `plan_render_target_rows` (raw_dpc/mod.rs:1722), force
`planned_rows = 1` so a partial-width rectangle collapses into ONE range
spanning every row, over-declaring the untouched inter-row bytes. This is
precisely the defect the brief names ("collapsing them into one span was
measured to falsely claim 95% of a range as written").

  focused (-p fn64-render-wgpu):  4722 passed / **24 FAILED** -> **KILLED**
  Named killers include:
    raw_dpc::tests::non_contiguous_rectangle_rows_have_one_exact_access_each
    production::tests::the_composed_texrect_fixture_declares_the_hand_derived_rows
    production::tests::execute_raw_dpc_admits_a_partial_width_fill_end_to_end

The per-row declaration rule is genuinely well-guarded, including by a
hand-derived expectation. Contrast with M1: the two arms I am keeping are NOT
equally protected. The journal's per-row truth is strongly tested; the present
path's field write is not tested in-crate at all.

Both mutants reverted; both baselines re-confirmed clean.

---

# Lane `lane/tri-cpu-raster` (worktree /private/tmp/fn64-tri-cpu, from 272bf781)

Baseline re-measured on this worktree (the doc above was written at an older
commit, so its 4746 is stale):
  cargo nextest run -p fn64-render-wgpu --offline -> 4755 passed / 3 skipped

## Plan, taking the prior lane's chosen design (ii) as given

Land Half A and Half B **in one commit**, per the prior lane's measured proof
that Half A alone breaks plan seal. Narrowest first: flat-shaded, opaque,
untextured, no depth, RGBA16 target.

Seam-by-seam, all three touched together:
1. decoder `raw_dpc/mod.rs` 0x08..=0x0f -- derive covered rows from the
   triangle's own edge coefficients, call the EXISTING
   `plan_render_target_rows` once per covered row, record a span.
2. adapter `raw_dpc/production_adapter.rs` -- bind that span and push the
   decoder's own access slice before `push_triangle`, exactly as the texrect
   arm already does. Replaces the `texrect_accesses: None` line.
3. executor -- a new in-crate CPU rasterizer producing
   `CompletedColorTargetWrite`, scheduled in `stage_color_commands` alongside
   Fill and Texrect so it composes into the same accumulated buffer and is
   digested by the same single `fill_completed_writes` call.

The declared-vs-drawn hazard the doc names (fill_completed_writes slices
without checking the raster touched it) is closed by construction if and only
if the decoder's row derivation and the executor's raster derive their covered
X range from ONE function. That is the single most load-bearing constraint in
this lane, so the span math lives in one module used by both.

## STATUS AT A GLANCE (read this before quoting progress)

| rung | state | evidence |
| --- | --- | --- |
| flat, opaque, untextured (0x08) | **DONE** | guest bytes verified end to end, no GPU |
| shade plane interpolation (0x0c) | **DONE** | hand-derived gradients pinned |
| texture s/t/w + perspective divide (0x0a/0x0e) | **DONE** | both scale factors pinned; 14 mutants, 13 killed + 1 proven equivalent |
| depth (0x09 and friends) | **NOT DONE** | not started, and deliberately out of the texture rung's scope |

**The texture rung landed.** `raw_triangle_is_executable` now refuses only
the DEPTH bit, so WM2000's opcode 0x0e -- the only opcode it issues -- is
admitted, and `CombinerInputs::tex_val0` is a real sampled texel rather than
zero. See "The texture rung, landed" below for what it does and does not
prove.

## RESULT: a flat raw triangle's bytes now reach guest RDRAM

The prior lane's chosen design (ii) is implemented, at the narrowest rung of
the widening ladder: flat (opcode 0x08 -- no shade plane, no texture plane,
no depth plane on the wire), non-Fill cycle, RGBA16/32 target, drawn through
the latched combiner and blender.

### Seams

- `raw_dpc/triangle_span.rs` -- span geometry, written fresh in-crate. ONE
  function (`covered_rows`) is called by both the decoder and the raster, so
  a declared row can never be a row the raster skipped.
- `raw_dpc/mod.rs` `plan_raw_triangle` -- one exact `ColorFramebuffer` write
  access per covered scanline. NOT via `plan_render_target_rows`, which takes
  a single rectangle: a triangle's covered X range differs per scanline.
- `raw_dpc/production_adapter.rs` -- pushes the decoder's own access slice
  and carries the span, replacing `texrect_accesses: None`.
- `targets/raw_triangle.rs` -- the CPU rasterizer, reusing the texrect
  executor's `combine_one_texel` and `blend_and_write_pixel` rather than a
  second copy of the combiner/blender/dither arithmetic.
- `production.rs` -- `ColorCommandKind::RawTriangle` in the same schedule,
  sorted on the decoder's own `command_index`, composing into the same
  accumulated buffer, digested by the same single `fill_completed_writes`.

### The lft polarity, resolved

Wire bit 23 set means **LEFT-major**. `RawTriangle::right_major()`'s name is
inverted; `triangle_span::left_major` is the single place the polarity is
decided, and it reads the accessor as left-major.

Evidence: `fn64-render-reference`'s
`real_stream_left_major_rect_split_triangle_rasterizes_interior`
(`raster/tests/group2.rs:1336`) carries byte-exact coefficients from WM2000's
live title-scene XBUS stream with bit 23 SET, `xh == 770048` (11.75px) and
`dxhdy == 0` -- a vertical edge -- while `xm == 701940` with
`dxmdy == 272435` marches right at +4.157px/line. The H edge is
unambiguously the left one. RT64 has no ground truth for this bit (it never
decodes raw edge coefficients).

The accessor was NOT renamed: renaming a `pub` accessor is a wider change,
and it has other (test-only) callers. The correction lives at the one call
site that matters.

### Two hazards closed by construction, one by a named refusal

1. Declared-but-undrawn ROWS: closed by the shared `covered_rows` call, AND
   enforced by `TexrectExecutionError::TriangleRowCountDisagreesWithJournal`
   -- because the decoder bounds its walk by installed RDRAM (SetColorImage
   carries no height) while the executor bounds it by the real extent, so
   the two lists genuinely CAN differ.
2. Declared-but-undrawn PIXELS: a declared `[x0, x1)` run is the union over
   a scanline's four subpixel sample rows, so a pixel inside it may have
   zero coverage. Closed by writing nothing for those pixels, leaving the
   resident's own current byte -- so the range's digest always describes
   real current content.

### Mutation results (five mutants, one survivor, fixed)

| mutant | result |
| --- | --- |
| M1 drop the flat-opaque admission gate | KILLED (1 failure) |
| M2 invert wire bit 23's polarity | KILLED (8+ failures) |
| M3 collapse the per-row declaration | KILLED (4 failures) |
| M4 drop the row-count-vs-journal guard | KILLED (3 failures) |
| M5 remove the `coverage == 0` skip | **SURVIVED** -> fixed |

M5 is an arm the lane KEPT and it survived for the brief's own named reason:
the existing test read a partially-covered pixel (coverage 4), never a
zero-coverage one, so it did not reach the arm. The killing case needs a
SLOPED edge. `a_declared_pixel_with_no_subpixel_coverage_is_not_painted`
builds one, asserts the precondition (the run is [2,8), pixel (2,1) has
coverage 0), and carries a positive control so it cannot pass by the raster
doing nothing.

### The routing gap the journal caught

A triangle-only packet declared its three per-row writes and then took the
no-colour-command branch, so `stage_color_commands` never ran: "backend
effect count is 0; exact journal requires 3". `stage_and_report`'s routing
condition counted fills and writing texrects but not writing raw triangles.
The exact-journal check refusing the packet rather than committing a partial
one is the design working as intended.

## MEASURED ON THE ROM: WM2000 emits ZERO flat triangles

Instrumented `plan_raw_triangle` on a scratch worktree (`probe/tri-census`,
never merged) to print every raw triangle's opcode flags, then ran the
all-Rust lane on the real ROM at `WM2000_MAX_STEPS=200000`.

```
826056  shaded=true textured=true depth=false
     0  anything else
```

**Every one of 826,056 raw triangles WM2000 issues in the attract loop is
opcode 0x0e -- shaded AND textured, no depth plane. Not one is flat.**

So the flat-triangle rung landed here is correct, proven end to end into
guest RDRAM, and draws **nothing in WM2000**. It removes the structural
blocker (a RawTriangle now declares journal writes and composes into the
guest-visible buffer through the same seam a texrect uses) and it validates
the whole path -- decoder -> adapter -> collector -> schedule -> executor ->
digest -> guest commit -- but WM2000's own geometry needs two more rungs
before a pixel changes on screen:

1. **Shade plane interpolation.** The eight shade coefficient words are
   already decoded and retained (`RawTriangle::shade()`); what is missing is
   the per-pixel `raw_attribute_plane` evaluation and feeding the result into
   `CombinerInputs::shade`. The reference's version is ~15 lines
   (`raster/draw.rs`'s `plane` closure and its `shade` arm).
2. **Texture s/t/w plane interpolation with perspective divide**, then the
   TMEM fetch. The fetch itself already exists and is already generic over
   the byte source (`sample_point`, used by `execute_texture_rectangle`); the
   missing piece is deriving s/t from the plane and dividing by w.

Both are genuinely per-pixel arithmetic over coefficients this crate already
decodes, and both go through the SAME `combine_one_texel` call this lane
already wires. The span geometry, the journal declaration, the row-by-row
guard, the composition and the guest commit do not change at all -- only
what fills `CombinerInputs` for a covered pixel.

`raw_triangle_is_flat_opaque` and `execute_raw_triangle` are the two places
that widen, in that order (executor first).

## Rung two landed: shade plane interpolation

`triangle_span::shade_planes` + `attribute_plane` + `attribute_sample`, wired
into the raster loop's `CombinerInputs::shade_color`, and
`raw_triangle_is_flat_opaque` widened to admit opcode 0x0c. The executor was
widened FIRST and the decoder predicate second, per the rule.

Hand-derived and pinned: `dcdx = 32<<16` gives red 4/36/68/100 across x=2..6
(four distinct values, which no constant-colour implementation produces);
`dcde = 8<<16` gives 1/9/17 down rows 0..2.

### A defect found while wiring it

The RDP's eight coverage subsamples are a **checkerboard**, not a 2x4 grid:
the X columns are (1,5) on Y rows 1 and 5 and (3,7) on rows 3 and 7. This
module's first draft used (1,5) on every row. Now derived from
`crate::COVERAGE_SAMPLES` so the two cannot drift, and pinned by a test whose
distinguishing case is a left edge at x=0.75px, where the checkerboard covers
2 of 8 subsamples and the frozen grid covers ZERO -- the pixel is painted or
not, not merely weighted differently.

### Mutation, round two

| mutant | result |
| --- | --- |
| M7 drop the along-edge (de) plane term | KILLED |
| M8 drop the across-span (dx) plane term | KILLED |
| M9 measure X from x=0 rather than the major edge | KILLED |
| M10 freeze the X sample columns at (1,5) | **SURVIVED** -> fixed |

M10 survived for exactly M5's reason: both gradient tests sample at Y row 1,
where the two readings agree, so nothing reached the difference. **Two of ten
mutants survived, both because a test read the arm at a point where the
correct and incorrect answers coincide.** That is the failure mode to look
for first in this area.

## What actually remains before WM2000's geometry appears

The admitted subset is now opcode 0x08 and 0x0c. WM2000 issues **only**
0x0e -- shaded AND textured. So one rung remains for the ROM:

**Texture s/t/w plane interpolation with perspective divide.** Concretely:
- `triangle_span::shade_planes`' twin for the texture block, which has the
  identical (integer, fraction) 16-bytes-apart wire layout -- the decode is
  a near-copy and `RawTriangle::texture()` already retains the words.
- Per pixel: evaluate the s, t and w planes at the same `attribute_sample`
  point the shade planes already use, then `s/w`, `t/w` when
  `other_mode.texture_perspective()` is set.
- Feed the resulting S10.5 coordinates to `sample_point`, which already
  exists, is already generic over `TmemByteSource`, and is already the
  texrect executor's one sampler. Its result goes into
  `combine_one_texel`'s `texel` argument, which this lane already passes
  (currently `[0; 4]`).
- Then widen `raw_triangle_is_flat_opaque` to admit bit 1, and thread the
  tile binding the way `execute_scheduled_texrect` already does.

Everything else is done and does not change: span geometry, the per-row
journal declaration, the declared-vs-rasterized range guard, the schedule,
the composition into the accumulated buffer, the single end-of-packet digest,
and the guest commit. Only `CombinerInputs::tex_val0` is still zero.

Depth (bit 0) remains out and is a separate, larger piece: it needs a depth
image, a depth journal declaration, and the RDP's own Z encoding.

### The texture rung's two non-obvious constants, recorded before they are needed

`fn64-render-reference`'s `draw_raw_rdp_triangle_impl` (`raster/draw.rs:898`)
carries two scale factors that are EMPIRICAL, derived against WM2000's own
title screen, and that a fresh implementation will get wrong by default:

1. **Perspective path.** Hardware `tcdiv` is not a bare S/W ratio: it feeds
   the high bits of the s15.16 attribute planes to a 2^15-normalized
   reciprocal of W, so the output is `(S/W) * 2^10` texels in S10.5 units.
   The reference records that without the `* 1024.0` "the whole title-screen
   quad collapsed onto texel (0,0) -- every pixel sampled the image's corner
   and the presented frame was a uniform field."
2. **Non-perspective path (`G_TP_NONE`).** The divide is skipped and the
   plane's own value converts s15.16 -> S10.5 by dividing by `2^21`
   (`2^16 * 2^5`).

And one robustness rule, also earned from real content: **w <= 0 must not
fault.** A perspective triangle crossing the near plane legitimately presents
non-positive W at edge pixels; real RDP hardware derives 1/w from the
operand's top bits with no sign trap, so the pixel samples garbage texels and
the chip keeps going. The reference divides by `w.unsigned_abs().max(1)`.
It records that a loud assert here was correct until real content (WM2000 gfx
task ~#27) hit it.

Cite these; do not re-derive them.

## Measured at the EXECUTION seam: zero WM2000 triangles reach the rasterizer

The earlier census counted triangles at DECODE. This measures the question
that actually matters -- how many reach `execute_scheduled_raw_triangle` and
produce guest bytes -- by instrumenting both seams on a scratch worktree
(`/private/tmp/fn64-tri-exec`, never merged) and running the real ROM.

```
TRIDECL (raw triangles reaching the admission check)  1,314,648
TRIEXEC (raw triangles reaching the CPU executor)             0
```

The probe run itself: exit 0, 1087 VI swaps, zero panics, `last render
error: None`, terminated on its own 200,000-step cap. And exactly ONE flag
combination appears across all 1,314,648 -- `s=true t=true d=false`, i.e.
opcode 0x0e -- with `executable=false` for every one.

Every one carries `textured=true`, which `raw_triangle_is_executable`
refuses. So the answer to "is a raw triangle visible in a captured guest
framebuffer from the real ROM?" is **no**, and the reason is not a bug in
this lane -- it is the missing texture rung.

This is a stronger statement than the decode-side census: it is measured at
the seam that writes guest memory, not at the one that decides whether to
declare.

## Follow-up card (NOT this card's work): nine duplicated wire-word encoders

Noted while writing fixtures, recorded here so it is not lost, and
deliberately NOT implemented -- it is a different card from this one, and the
goal is pixels on screen rather than better tooling.

This tree contains **nine hand-rolled RDP wire-word encoders for the same
command set**, three of which predate this lane:

```
production.rs               triangle_base_edge_words, flat_triangle_in_target_words
raw_dpc/production_adapter  triangle_base_edge_words
raw_dpc/mod.rs              triangle_base_word0, flat_triangle_words
raw_dpc/triangle_span/tests wire, coefficient_block
targets/raw_triangle/tests  triangle, shaded_triangle
```

Each re-derives the same bit layouts and each is a place to get a shift
wrong. Collapsing them into one builder is a pure refactor needing no new
capability, and it is verifiable because every existing test must still pass
unchanged. Any such builder must emit WIRE WORDS through the real decoder --
never construct `RawTriangle` or `ResourceAccess` directly -- or it stops
testing the thing that breaks.

One measurement worth keeping from the same observation: the census that
established WM2000's real triangle mix cost a single instrumented ROM run and
was done LAST in this session rather than first. Measuring what the ROM emits
before scoping which rung to build would have reordered the whole session.

## The three arithmetic `expect`s are provably unreachable

Checked rather than assumed, because a panic on hostile wire input from a
real ROM stream would be a crash, not a refusal. Bounds come from the wire
field widths alone:

- `yh`/`yl` are 16-bit, so `sample_y_eighth - high_origin_eighth` is at most
  ~131,072 eighths.
- `fixed_mul_ratio(i32::MIN, 131072, 8)` = 3.52e13 -- four orders of
  magnitude inside i64.
- The worst `attribute_plane` X term is `|i32::MIN| * (4096px + that)` / 2^16
  = **1.15e18**, against i64's 9.22e18. An 8x margin.

So none of the three can fire for any 32-byte (or 96-byte) triangle the
decoder accepts. They are assertions of a proven invariant, not latent
panics.

**If the texture rung widens the coordinate range** -- and a perspective
divide by a near-zero W is exactly the shape that would -- this proof must be
redone. The reference's own `w <= 0` tolerance rule (divide by
`unsigned_abs().max(1)`) exists for that reason.

## Frame evidence: no 3D geometry in any captured frame

A full 400,000-step run with `WM2000_FB_DUMP_DIR` set (2149 VI swaps, zero
panics) dumped **2,147 guest framebuffer PNGs**, of which **133 are
distinct**. So the capture is live and varied, not a stuck buffer -- which
is what makes the negative result meaningful.

Sampled across the run (frames 3, 403, 803, 1203, 1603, 2003, 2100, and the
three most frequent hashes covering 1,745 of the 2,147): every one shows
flat 2D content -- bands, blocks, and thin rules -- and none shows rendered
3D geometry.

That agrees exactly with the execution-seam count: 1,314,648 raw triangles
reach the admission check and 0 reach the rasterizer, so there is no path by
which a triangle could have appeared. The frames confirm the measurement
rather than merely illustrating it.

## ROM verification tally

Six runs, all on this lane's worktree, all reading the LAST `vi_swaps` line
and confirming termination (the harness prints a checkpoint every 50,000
steps, and a mid-run 825/1087/1355 has fooled a sibling lane before):

| run | cap | swaps | panics | render error | terminated |
| --- | --- | --- | --- | --- | --- |
| C | 400k | 2149 | 0 | None | step budget |
| D | 400k | 2149 | 0 | None | step budget |
| E | 400k | 2149 | 0 | None | step budget |
| HEAD | 400k | 2149 | 0 | None | step budget |
| frame-dump | 400k | 2149 | 0 | None | step budget |
| FINAL (true HEAD) | 400k | 2149 | 0 | None | step budget |
| exec probe | 200k | 1087 | 0 | None | step budget |

2149 is the baseline. No run regressed it, and none introduced a panic. The
probe's 1087 is correct for half the step budget.

## The texture rung, landed

`CombinerInputs::tex_val0` is a real sampled texel. The admitted subset is
now every DEPTH-FREE opcode -- 0x08, 0x0a, 0x0c and 0x0e -- which includes
the 0x0e that is 100% of what WM2000 emits.

### The two scale factors, used as cited

`triangle_span::texture_coordinates_s10_5` carries both verbatim from
`fn64-render-reference`'s `draw.rs:898`, and neither was re-derived:
perspective is `(S / |W|) * 1024`, `G_TP_NONE` is `plane / 2^21`. The
`w <= 0` rule divides by `unsigned_abs().max(1)`.

### One thing the reference did not have to decide

`TextureCoordinateS10_5` is an `i16` in this crate, so the float result must
be NARROWED, and the `w <= 0` tolerance is exactly what makes an
out-of-range coordinate reachable. It SATURATES: a coordinate that ran off
the right edge clamps to the tile's last texel, where a wrapping `as i16`
would fold it back to the FIRST -- a tear rather than a stretched edge.
`an_overflowing_texture_coordinate_saturates_to_the_last_texel_not_the_first`
pins it, and a wrapping mutant survived every other test in the file.

### TMEM prefix selection: the texrect machinery, reused

The brief flagged this as the hard part, and it needed no parallel
implementation. `stage_color_commands`' `RawTriangle` arm now matches the
same `TexrectTmemSource` the `Texrect` arm matches, calls the same
`prefix_before` over the same `prefixes` slice with the triangle's own
`command_index`, and hands `execute_raw_triangle` the resolved image. Same
`verify_tmem_identity` check on the way in.

Two things a triangle needed that the texrect path did not supply:

1. **The tile index.** `execute_scheduled_raw_triangle` reads
   `RawTriangle::tile()` -- wire word 0 bits 18:16. `PlanCollector`'s
   `bound_tile_index` originally froze a raw triangle's tile to 0 for the
   GPU uniform path, with a comment claiming "it carries no tile field of
   its own to read". That comment was wrong: the field exists, the CPU
   executor reads it, and the GPU path silently bound tile 0's descriptor
   for any triangle naming another tile. **Corrected in a follow-up:** the
   `RawTriangle` arm now reads `(raw_words[0] >> 16) & 0x7` from the
   command's own retained wire words, the same one-field read the
   `TextureRectangle` arm beside it already performed on word 1 bits 26:24,
   so both paths resolve the same tile for the same draw.
   `plan_collector_binds_the_tile_a_raw_triangle_s_own_wire_word_names`
   pins it adapterlessly, with
   `plan_collector_binds_tile_zero_when_a_raw_triangle_s_wire_word_names_it`
   holding the tile-0 arm.
2. **The opcode.** `execute_scheduled_raw_triangle` decoded with a frozen
   `0x08`, which sizes the optional coefficient blocks. Harmless until a
   textured triangle was admitted, then a hard length refusal. It now reads
   the command's own first wire byte.

### Mutation: 14 run, 13 killed, 1 proven equivalent

Four survived the first pass and each needed its own distinguishing case:
freezing `first_row_parity` (every fixture had `low_t = 0`), freezing the
tile index at 0 (every fixture used tile 0), wrapping instead of saturating
(no fixture left `i16` range), and dropping the opcode/binding guard
(unreachable from any end-to-end fixture). **That is four of fourteen, all
for the same reason: a fixture reading the arm where correct and incorrect
answers coincide.** It remains the first thing to look for here.

M4 (dropping the `max(1)` floor) is EQUIVALENT and was proven so rather
than assumed: with a saturating narrow, `W = 0` yields +inf/-inf/NaN for
positive/negative/zero S, which clamp to `i16::MAX`/`i16::MIN`/0 -- the same
three answers `max(1)` gives.

### What the ROM found that no unit test did

A latent defect PREDATING this rung, and the reason the first ROM run
aborted at 280 VI swaps: `plan_raw_triangle` bounded its row walk by
installed RDRAM and a 4096-row cap, not by the target's height. On WM2000's
480x237 target a taller triangle declares ranges past the target's end, and
`verify_accesses_inside` refuses the whole PACKET. It applies to flat and
shaded triangles identically and was unreachable only because the decoder
refused every triangle the ROM emits.

The fix threads `RenderConfig`'s height into `RdpState` at `create_inner`,
beside `configured_target_extent` and from the same field. The harness
reproduces it in 0.047 s; the ROM took about nine minutes to reach it.

**The lesson worth keeping: the fast harness found nothing here, and could
not have.** No synthetic fixture had a triangle taller than its target,
because nobody thought to write one. The ROM run is not a formality after
the harness work -- it is the only thing that exercises geometry the ROM
actually emits.

### ROM evidence: 2149 VI swaps, zero panics, exit 0

With the texture rung admitted and the height bound fixed, WM2000 runs the
full 400,000-step cap and terminates on its own:

```
steps= 50000  vi_swaps= 280   steps=250000  vi_swaps=1355
steps=100000  vi_swaps= 555   steps=300000  vi_swaps=1626
steps=150000  vi_swaps= 825   steps=350000  vi_swaps=1887
steps=200000  vi_swaps=1087   steps=400000  vi_swaps=2149
```

Final: `VI swaps observed: 2149`, `gfx tasks submitted: 5967`,
`last render error: None`, zero panics, exit 0. **Exactly the 2149-swap
baseline, not merely near it** -- so admitting a million textured triangles
into the CPU rasterizer costs no frames on this measure.

Every checkpoint is listed because a mid-run checkpoint has been mistaken
for a final result more than once in this project; the 2149 above is the
LAST line, and the run's own summary block agrees with it.

### Depth-free scanlines run in parallel

The guest-visible CPU rasterizer keeps command order sequential, but a single
depth-free triangle now divides its color target into exclusive whole rows
and runs those rows on a persistent work-stealing pool. The scalar pixel body
is still the one implementation; each job receives one local row plus its
guest-row base, and all jobs finish before the next triangle can observe the
target. Depth-bearing draws and combiner census runs stay scalar because they
retain cross-row mutable state. Triangles below 256 declared-range pixels also
stay scalar; bounded threshold measurements include the cutoff and prevent
thread-pool dispatch from consuming the win. `FN64_PARALLEL_RASTER=0` is the
exact control lane, absent means enabled, and any other value traps.

The release `texture_plane_raster_microbench` measured four interleaved A/B
pairs on 2026-08-23. Scalar mean/min-of-four was 516.635/505.721 ns per covered
pixel; parallel was 88.245/82.354 ns, a 5.85x mean speedup (6.14x by min-of-N).
This is a headless per-pixel substrate result, not a windowed frame figure.
The current execution sandbox exposes no Metal adapter, so the WM2000
rs+wgpu pump-census confirmation remains pending on a GUI-capable host.

### Frame evidence: textured geometry now appears

A `WM2000_FB_DUMP_DIR` run dumps guest framebuffer PNGs (only when
non-uniform). Measured against the 2D-only baseline
`docs/frames/wm2000-swap240-true-geometry-480x237.png`:

| frame | dims | distinct colours |
| --- | --- | --- |
| baseline, 2D only (swap 240) | 480x237 | **12** |
| post-fix (swap 371) | 320x240 | **961** |
| post-fix (swap 579) | 320x240 | **1017** |

`docs/frames/wm2000-swap579-textured-triangles-320x240.png` is kept as the
post-fix reference. **Yes, 3D geometry now appears**: the baseline is a
handful of flat black/white/blue rectangles, and the post-fix frames carry
a thousand distinct colours across recognizable textured surfaces. A
hundredfold rise in colour count is not something a 2D blitter produces.

Two things this comparison must NOT be read as saying:

1. **It is not a pixel diff.** The dumped frames are 320x240 and the
   baseline is 480x237 -- a different scene phase, not a stride bug. The
   comparison tool reads both dimensions from each file's own IHDR and
   REFUSES to diff mismatched sizes, precisely because a prior lane
   hardcoded 320x240 against a 480x237 frame and manufactured a "striping"
   defect out of the mismatch. Do not re-derive that mistake in reverse.
2. **The visible horizontal striping is not this rung's.** It is the
   known VI interlace artifact (`lane/vi-interlace-stripes` exists for it)
   and it appears on every dumped frame including untextured ones. This
   rung's claim is that TEXELS reach guest RDRAM, which the colour counts
   and the surfaces establish; the scanline interleave is a separate defect
   one layer downstream, in scanout.

## A second, latent copy of the frozen-tile bug (not fixed, recorded)

Found while fixing `PlanCollector`'s frozen raw-triangle tile index (the
commit reading `RawTriangle::tile()` instead of hardcoding 0). A second,
separate collector -- `TriangleDrawStateCollector` in
`raw_dpc/triangle_draw_data.rs` -- carries the identical pattern: it still
tracks only tile 0 for a raw triangle, and its own doc comment still claims
the triangle "carries no tile index of its own to read," which is the same
false claim `PlanCollector`'s comment made.

Left alone because it is not on the GPU execution path today --
`production.rs` drives `PlanCollector`, not this collector, so nothing
currently reads a wrong tile through it. But the defect is real and latent:
if anything ever routes the GPU path through `TriangleDrawStateCollector`
instead, it will silently bind tile 0 for every raw triangle again, the same
failure `PlanCollector`'s fix just closed.

Not a card today. Recorded so it is not rediscovered as if it were new.

---

# Depth (bit 0): scoping card

Scoped on worktree `/private/tmp/fn64-depth-scope` (branch
`card/rt64-depth-scope`) from `b484defa`. **Scoping only -- no behaviour
change proposed or landed by this card.** The conclusion is that the "three
nouns" the texture rung deferred ("a depth image, a depth journal
declaration, and the RDP's own Z encoding") understate the work by one
whole hazard, and overstate it by one whole component.

## 1. Is depth needed for WM2000 gameplay? Measured: not for anything reached so far. Unknown for a match.

Three INDEPENDENT censuses in this repo agree, and none is a re-count of
another:

| census | population | depth-bit set |
| --- | --- | --- |
| decode-seam (`RT64-TRIANGLE-WRITEBACK.md`, "MEASURED ON THE ROM") | 826,056 | **0** |
| execution-seam (same doc, "Measured at the EXECUTION seam") | 1,314,648 | **0** |
| planner-arm counters (`RT64-WM2000-INPUT-GRAMMAR.md:383`) | 1,600,000 | **0** |

Exactly one flag combination occurs across all three: `s=true t=true
d=false`, opcode 0x0e. `RT64-WM2000-INPUT-GRAMMAR.md:391` states the
consequence directly: "The depth hypothesis is refuted outright... Depth is
not what is blocking this screen, and implementing depth would not unblock
it."

**What this does and does not establish.** It establishes that depth is not
a blocker for any screen the emulator currently reaches, and that building
it would unblock nothing today. It does NOT establish that a match needs no
depth, because **no match has ever been reached** -- the ROM plateaus before
gameplay (`RT64-WM2000-INPUT-GRAMMAR.md`, the swap-1901 abort and the
button-probe matrix). The measured population is attract loop and menus.

So the honest answer to "does a match need depth" is **unknown, and not
determinable from this repository today**. It is not merely unmeasured; it
is unmeasurable until the input/abort work reaches a match. Any claim in
either direction would be prediction, not measurement.

One piece of genuine counter-evidence against assuming depth is required:
the attract loop already draws **real textured 3D geometry with the depth
bit clear on every single triangle** (the 12 -> 1017 distinct-colour frame
result above). So this engine demonstrably issues 3D content without Z, and
"3D therefore Z" is not a safe inference for it. That is evidence, not
proof: attract-loop 3D may be depth-sorted content that gameplay's
mutually-occluding wrestlers would not be.

## 2. The Z encoding is ALREADY SOLVED IN THIS REPO, twice, in two different domains

The brief expected this to be scoped from RT64's C++. It should not be:
this workspace already contains a complete, tested, manual-cited integer
implementation, and the RT64 one is the wrong domain for a CPU rasterizer.

### `fn64-render-reference/src/depth.rs` -- the INTEGER encoding (the one a CPU rasterizer needs)

Cited in-file to "Nintendo 64 Programming Manual, Chapter 16, Z Image
Format" (`depth.rs:3`). Exact layout, quoted:

- `decode_z` (`depth.rs:39`): 3-bit exponent `(encoded_z >> 11) & 7`,
  11-bit mantissa `encoded_z & 0x07ff`, into an unsigned 18-bit 15.3 value
  via two frozen tables (`depth.rs:9-12`):
  `Z_SHIFT = [6,5,4,3,2,1,0,0]`,
  `Z_ADD = [0x00000,0x20000,0x30000,0x38000,0x3c000,0x3e000,0x3f000,0x3f800]`.
- `encode_z` (`depth.rs:47`): saturates at `0x3ffff`, selects the exponent
  by an explicit eight-way range match, then
  `((z - Z_ADD[e]) >> Z_SHIFT[e])`.
- `pack` (`depth.rs:125`) is the load-bearing one:
  `visible = (encoded_z << 2) | (encoded_delta >> 2)`, `hidden = encoded_delta & 3`.
- `encode_delta_z` (`depth.rs:68`) is `floor(log2)` saturated to 15, cited
  to "Programming Manual Chapter 15, Equation 10".

### The hazard the "three nouns" missed: TWO OF THE SIXTEEN BITS DO NOT LIVE IN RDRAM

`pack` splits DeltaZ across a `visible: u16` and a `hidden: u8`.
`EncodedDepth` (`depth.rs:17-20`) exists precisely to carry both. The
hidden pair is RDRAM's two extra bits per halfword, which **ordinary CPU
halfword accesses cannot observe** (`depth.rs:5-7`).

The reference stores them in a host-side sidecar keyed by physical
halfword -- `RdramHiddenBits` (`backend/hidden_bits.rs:24-26`), a dense
`Vec<u32>` over `DEFAULT_RDRAM_SIZE / 2`, entirely outside guest memory.

This is a STRUCTURAL mismatch with the journal, not a detail. The whole
raw-triangle path's correctness argument is "declared ResourceAccess ranges
are satisfied by real CompletedWrite bytes, digested and committed into
guest RDRAM". Guest RDRAM has 16 bits per halfword and no sidecar; grepping
`fn64-abi` and `fn64-render-ir` for hidden-bit storage finds **nothing**.
So a depth rung must decide, explicitly and on evidence, one of:

  (a) model only the 16 visible bits and accept that the low two DeltaZ
      bits read back as whatever the visible word's bits 1:0 hold -- which
      changes `relations()`' tolerance, i.e. changes which pixels pass;
  (b) add a hidden-bit sidecar to the wgpu crate mirroring the reference's,
      and decide what a journal declaration means for state that never
      reaches guest RDRAM;
  (c) restrict the first rung to cases where the DeltaZ hidden bits provably
      do not affect the outcome, and refuse the rest by name.

None of these is obvious, and (a) is the one a fresh implementation would
pick by accident. **This is the single reason this card stops at scoping.**

### `fn64-render-wgpu/src/depth_encode.rs` -- RT64's FLOAT encoding (do NOT reuse for the CPU path)

Already ported, `float_to_depth16` (`depth_encode.rs:257`), with
`DEPTH_EXPONENT_SHIFT = 13` / `DEPTH_MANTISSA_SHIFT = 2` and masks
`0xE000`/`0x1FFC` (`depth_encode.rs:211-214`), cited to `Depth.hlsli:24-41`.

It is the SAME 16-bit layout, reached from a different domain: it takes
`f32` in `[0,1]`, and its own module doc restricts its equivalence claim to
that domain. It is also **inert** -- a private module whose only referrer
in the whole workspace is another RT64 port (`rt64_framebuffer_shaders.rs:300`),
matching this project's known "ported modules are inert" pattern. A CPU
rasterizer working in the RDP's own s15.16 integer planes should use
`fn64-render-reference`'s integer path's ARITHMETIC, not this one, or it
will round-trip through float for no reason and inherit a domain
restriction it cannot satisfy.

**The two must not be casually unified.** They agree on layout and differ
on domain, exactly the shape of the "two pins are both correct by design"
trap this project has recorded before.

## 2b. Corrections and additions from a second, independent evidence sweep

Three things the first pass of this card got incomplete. All verified by
reading.

### The census evidence is far wider than the triangle censuses alone

`G_SETZIMG` (`0xfe` on the GBI lane, cmd6 `0x3e` on the raw-DPC lane) is
measured absent across **five** windows of increasing size, not one:

| window | population | `G_SETZIMG` |
| --- | --- | --- |
| `RT64-WM2000-CENSUS.md:207` | 142,606 commands | **0** |
| `RT64-WM2000-0X1CC-DIAGNOSIS.md:79` | 2,636,852 commands | **0** |
| `RT64-WM2000-SECTION-LOCAL.md:71` | 5,406,193 commands | **0** |
| `RT64-WM2000-REMAINING.md:83` | 109,041 decode entries (~500x) | **0** |
| `RT64-WM2000-GAMEPLAY-GAP.md:185` | 6,526,330 commands, **input-driven through 18 menu screens** | **0** |

The last one matters most: it is not the attract loop. Someone drove the
game through eighteen menu screens and the command set did not change --
"same 21 opcodes, same rank order... still zero Z-variants, zero two-cycle,
zero `G_SETZIMG`". `G_SETPRIMDEPTH` (`0xee`) is **also zero**
(`RT64-WM2000-CENSUS.md:216`), despite already being decoded and staged
here (`raw_dpc/mod.rs:1261`).

The instrument would have caught one: the census is `[AtomicU64; 256]`, one
counter per command byte, not an allowlist (`gbi/census.rs:26-30`), and
unrecognized bytes report as `UNNAMED_<byte>`.

Note also that `0x3e` today makes the decoder refuse the **whole packet**
(`raw_dpc/mod.rs:2553` pins it as an `UnsupportedCommand` fixture), so a
`G_SETZIMG` could not have passed silently.

### The a-priori "wrestling game must be Z-buffered" argument was already made HERE, and already retracted BY MEASUREMENT

`RT64-WM2000-GAP.md:312` ranked depth as item 4 on exactly that reasoning:
"A wrestling game is z-buffered 3D -- two wrestlers, a ring, a crowd, all
interpenetrating." `RT64-WM2000-CENSUS.md:405` then demoted it to item 8,
"Deferred. Zero occurrences and zero Z-variant triangles in this window.
**Revisit only when the window reaches gameplay**."

So the intuition this card was asked to test has already been raised,
acted on, and overturned on evidence once in this project. Raising it a
second time without new measurement would repeat that cycle.

### The genuine gap: nobody has ever decoded the othermode Z bits

This is the one place the evidence is **absent rather than negative**, and
it is worth stating loudly because it is the only way the "0% depth" story
could be misleading.

`G_RDPSETOTHERMODE` is the single most frequent opcode WM2000 issues --
23,639 occurrences, 16.6% (`RT64-WM2000-CENSUS.md:177`) -- and **its
payload bits are never decoded** by any census. Stated explicitly in at
least four docs, e.g. `RT64-LANE-DIVERGENCES.md:816`: "the census counts
opcodes and does not decode `G_RDPSETOTHERMODE` payload bits... Absence in
the census window is not absence."

So: no measurement anywhere says whether WM2000 sets `Z_CMP`/`Z_UPD`.
`RT64-WM2000-INPUT-GRAMMAR.md:383`'s `no_other_mode = 0` proves every
triangle HAD a latched othermode; nobody read its Z bits out.

**This is cheap to close and should be done before any depth
implementation.** `RT64-WM2000-CYCLE-MODES.md` already decoded othermode
payloads for cycle type on this ROM, so the instrument exists and merely
needs pointing at bits `0x0010`/`0x0020`. If WM2000 turns out to set
Z_CMP/Z_UPD while issuing only non-Z triangles, that is a genuinely
different picture from the one the triangle censuses paint, and it is
knowable today without reaching a match.

## 2c. RT64's pinned C++ (`5473732a`), read directly -- and why it is the WRONG reference here

Checked out at `/private/tmp/rt64-pin-5473732a`. Every claim below was
verified by reading the file, not inferred.

### The headline: RT64 never software-rasterizes depth

Depth testing and writing are **GPU fixed-function depth-stencil state**
(`src/render/rt64_raster_shader.cpp:315-319`):
`depthEnabled = zCmp || zUpd`, `depthFunction = zCmp ? LESS : ALWAYS`,
`depthWriteEnabled = zUpd`, and `depthTargetFormat = D32_FLOAT`. There is
no span walker, no per-pixel Z compare loop, and no CPU depth encoder
anywhere in the tree -- CPU-side has decode only
(`ColorConverter::D16::toF`, `src/hle/rt64_color_converter.cpp:61-70`),
used for exactly one thing: turning a FILLRECT fill colour into a clear
depth (`src/render/rt64_framebuffer_renderer.cpp:1528`).

**So for the inner loop this card would have to write, RT64 has nothing to
cite.** `fn64-render-reference` is the better reference on every axis: it
is integer, it is a CPU rasterizer, it is in-tree, and it is tested.

RT64 remains the right citation for two things: the 16-bit word layout, and
the othermode->depth-state mapping.

### The 16-bit word layout (`src/shaders/Depth.hlsli:7-42`), verified

```
DEPTH_EXPONENT_MASK   0xE000   DEPTH_EXPONENT_SHIFT  13
DEPTH_MANTISSA_MASK   0x1FFC   DEPTH_MANTISSA_SHIFT   2
```
bits 15:13 exponent, bits 12:2 mantissa (**11 bits**), bits 1:0 dz.

`FloatToDepth16` computes the exponent by counting leading ones
(`depthFixed << 14`, `firstbithigh(~depthShifted)`, `clamp(31-firstZero,0,7)`)
and `Depth16ToFloat` computes the bias as
`0x40000 - (0x40000 >> exponent)`. **RT64 has no lookup tables.**

**The two implementations AGREE, and that is worth stating.** RT64's
computed bias `0x40000 - (0x40000 >> e)` expands to exactly
`fn64-render-reference`'s frozen `Z_ADD` table
(`0, 0x20000, 0x30000, 0x38000, 0x3c000, 0x3e000, 0x3f000, 0x3f800`), and
its `6 - min(6, e)` shift expands to exactly that crate's
`Z_SHIFT = [6,5,4,3,2,1,0,0]` -- including the asymmetric cap where
exponent 7 shifts the same as 6. Two independently-sourced implementations
(HLSL from RT64, integer from the Programming Manual) producing the same
eight-entry table is strong corroboration for the encoding.

### RT64's dz is a dead path -- do not cite it as behaviour

`FloatToDepth16` keeps only `(dzBit >> 2) & 0x3`, the top two bits of the
power-of-two index. And **both call sites pass zero**:
`FbWriteDepthCS.hlsl:27` is `float dz = 0.0f; // TODO`, and
`RtCopyDepthToColorPS.hlsl:42` passes `0.0f`. RT64 has never written a
nonzero dz to a depth image.

This is the same 18-bit split the reference handles as visible+hidden, seen
from the other side: RT64 keeps 2 dz bits because 2 is all that fits in the
visible halfword. The other two are the RDRAM hidden bits. **RT64's answer
to the hidden-bit question is "discard them", which is option (a) above --
and it is a GPU renderer that never needed the tolerance to be right.**

### The mode bits (`src/shared/rt64_f3d_defines.h:84-101`), verified

`Z_CMP 0x10` (bit 4), `Z_UPD 0x20` (bit 5), `ZMODE_MASK 0xc00` (bits 11:10)
with `OPA 0 / INTER 0x400 / XLU 0x800 / DEC 0xc00`. Z-source select is
`G_MDSFT_ZSRCSEL 2` (bit 2), `G_ZS_PIXEL`/`G_ZS_PRIM`
(`rt64_f3d_defines.h:12-14`).

**These match `fn64-render-reference` exactly** -- `depth_compare_enabled`
is `low & 0x0010`, `depth_update_enabled` is `low & 0x0020`, `depth_mode`
is `(low >> 10) & 3` (`gbi/types.rs:445-451`, `:490`). Second independent
confirmation of the bit positions.

**`G_CLR_ZCMP` does not exist in RT64's source.** Grep finds only `Z_CMP`.
The brief named it; it is not an RT64 symbol.

RT64 also **collapses OPA/INTER/XLU into one `LESS` test** and only
distinguishes `ZMODE_DEC`, which it implements as a manual depth-read and
tolerance compare in the pixel shader
(`rt64_framebuffer_renderer.cpp:553-562`, `RasterPS.hlsl:88-110`), not as a
depth bias. `fn64-render-reference` is STRICTER here -- its `mode_passes`
gives Translucent and Decal genuinely different predicates
(`depth.rs:110-118`). Another reason to prefer the in-tree reference.

### G_SETZIMG carries an address and nothing else -- confirmed

`src/hle/rt64_rdp.h:85-89`: the depth image struct is `{ uint32_t address;
bool changed; }`, against the colour image struct directly above it
(`:76-83`) which carries `fmt`, `siz` and `width`. Depth width is inherited
from the **colour** image (`rt64_rdp.cpp:215-217` derives `depthBpr` from
`imageWidth`, which came from `fbPair.colorImage.width` at `:196`), and
16-bit size is asserted rather than decoded
(`rt64_native_target.cpp:274`).

This confirms the extent-inheritance decision named in section 3 below is
real and unavoidable, not an artifact of fn64's design.

### The Z coefficient block: order and layout confirmed

`src/hle/rt64_rdp.h:31-35` sizes the blocks (base 4 / shade 8 / texture 8 /
**depth 2** 64-bit words) and `src/gbi/rt64_gbi_rdp.cpp:410-574` confirms
the stream order base -> shade -> texture -> depth. The depth block is
`w0.w0 = Z`, `w0.w1 = dZdx`, `w1.w0 = dZde`, `w1.w1 = dZdy`
(`rt64_gbi_rdp.cpp:559-566`).

This exactly matches what `RawTriangle` already decodes
(`DepthWords = [RawWord; 2]`, `triangle.rs:97`), so **the fn64 decoder's
Z-block sizing is confirmed correct against RT64 as well as against its own
test.**

Two cautions on this RT64 path specifically: it discards `dZdy` (comment:
"only used on edge pixels for anti aliasing purposes"), it converts the
edge-walked Z into three per-vertex Z values for the GPU rather than
interpolating per pixel, and it is **incomplete** -- `rt64_gbi_rdp.cpp:306`
and `:583` both carry `// TODO do more than 1 triangle` followed by
`break;`. It is not a reference to copy.

## 3. What already exists on the fn64 side, and what genuinely does not

Existing, tested, and directly reusable:

- **Wire decode of the Z coefficient block.** `TriangleFlags::depth`
  (`raw_dpc/triangle.rs:45`), `DepthWords = [RawWord; 2]`
  (`triangle.rs:97`), retained by `RawTriangle::depth()` (`triangle.rs:322`),
  pinned by `depth_triangle_carries_exactly_two_depth_words`
  (`triangle.rs:548`). **The decoder already sizes 0x09 correctly.**
- **The compare/mode logic, already public in the wgpu crate.**
  `relations`, `mode_passes`, `depth_mode_decision`
  (`depth_mode.rs:106`, `:140`), exported from `lib.rs:434-436`, including
  the coverage-wrap override and an explicit
  `UnsupportedInterpenetratingCoverageAdjustment` variant rather than a
  silent Reject.
- **The othermode bits**, in the reference: `depth_compare_enabled` is
  `low & 0x0010`, `depth_update_enabled` is `low & 0x0020`
  (`gbi/types.rs:445-451`), `depth_mode()` is `(low >> 10) & 3`
  (`gbi/types.rs:490`), Z-source select is `primitive_depth_source`
  (`gbi/types.rs:437`).
- **Prim-depth precedent.** The reference's fill/texrect arms show the
  exact shape: `crate::depth::pack(u32::from(primitive.z & 0x7fff) << 3,
  primitive.delta_z)` (`raster/draw.rs:277`, `:545`) -- note the `<< 3`,
  which is the s15.3 promotion, and the `& 0x7fff`.

Genuinely missing, and the real work:

1. **A depth image in the wgpu crate.** There is no `SetZImage`/`0xfe`
   decoder arm at all -- grep finds only RT64-port doc comments
   (`rt64_gbi_f3d.rs:687`'s `decode_depth_image` is an inert HLE-side
   passthrough). This is the true twin of `SetColorImage`, and unlike
   colour it has NO width/format word: G_SETZIMG carries an address only,
   so extent must be inherited, which is its own decision.
2. **A second journal target.** Every declaration today is a
   `ColorFramebuffer` access into one accumulated buffer, digested by ONE
   `fill_completed_writes` call at packet end. Depth is a SECOND, disjoint
   guest region written by the SAME command. The exact-journal machinery
   counts accesses and matches them positionally, so a depth-writing
   triangle declares roughly 2N rows instead of N -- and the
   read-modify-write nature of a Z test means the depth buffer must be
   LOADED as well as stored, which no existing target does.
3. **Per-pixel Z in the rasterizer.** Cheapest part, and **cheaper than
   first scoped**: `fn64-render-reference` already does exactly this for
   raw triangles, per pixel, with the manual cited. `raster/draw.rs:988-1001`
   evaluates the Z plane, converts 16.16 -> the 15.3 working domain by
   `* 8 / Q16_ONE`, clamps to `0x3ffff`, and derives DeltaZ as
   `(|dzdx| + |dzdy|) * 8 / Q16_ONE` -- cited in-file to "Programming
   Manual, Chapter 16, Equation 4: DeltaZpix = |dZ/dx| + |dZ/dy|". The Z
   block is gated on the opcode bit at `gbi/stream.rs:332` and decoded by
   `gbi/entries.rs:611`. In fn64-render-wgpu this is the same
   `attribute_plane`/`attribute_sample` shape `triangle_span` already uses,
   with two coefficient words instead of eight.

   Note this makes the reference's `dzdy` handling STRICTER than RT64's,
   which discards `dzdy` entirely. Follow the reference.

## 4. Size: LARGER than the texture rung, and differently shaped

The texture rung, measured (`git show --stat`, six commits
`3790af9d`, `99a432b4`, `ebadd6ca`, `6e1d3cee`, `b718a59e`, `4d24c8f2`):
**~1,623 insertions**, 14 mutants, and it needed one ROM run to find a
defect no unit test could.

Depth is **larger**, and the reason is not line count:

- The texture rung added a per-pixel INPUT to an existing write. Every
  seam downstream -- span geometry, journal declaration, row guard,
  schedule, composition, digest, guest commit -- was reused **unchanged**,
  which the doc above states explicitly.
- Depth adds a per-pixel OUTPUT to a **second guest region**, plus a
  per-pixel READ of that region's prior contents. It is the first thing in
  this path that is read-modify-write, the first that declares against two
  images, and the first whose correct value does not fit in guest RDRAM at
  all (the hidden bits).

Estimate: **large** -- 2-3x the texture rung, dominated by the journal's
second target and the hidden-bit decision, not by the arithmetic.

**Two of the "three nouns" are effectively already solved**, which is the
single biggest correction this card makes to the deferred framing:

| the deferred noun | actual state |
| --- | --- |
| "the RDP's own Z encoding" | **done** -- `depth.rs`, tested, manual-cited, and independently corroborated by RT64's HLSL |
| "a depth image" | genuinely missing, but small -- address-only, extent inherited (confirmed against RT64) |
| "a depth journal declaration" | **the whole job**, and it is read-modify-write against a second image, which nothing in this path has ever done |

So the work is not three equal thirds. It is one small piece, one large
piece, and one already finished.

## 5. Stopped at scoping deliberately

No code changed. Crossing into implementation was declined for one specific,
non-negotiable reason rather than general caution:

**The first increment the brief proposed cannot be built honestly without
first deciding the hidden-bit question, and that decision is not mine to
make on inference.** The proposed increment -- prim-depth Z, always-pass
compare, "correct encoded Z bytes into a depth image" -- has to write
`pack()`'s output. `pack()` produces 18 bits. Two of them have nowhere to
go in guest RDRAM. A test asserting "correct encoded Z bytes" would
therefore be asserting the 16 visible bits and **silently discarding the
other two**, which is exactly the "substitute a placeholder value" the
evidence rules forbid, dressed as a passing test. Choosing option (a) above
by default is the failure mode, and it would be invisible until a depth
compare's tolerance came out wrong much later.

RT64 sharpens this rather than resolving it. RT64's answer to the hidden
bits is to **discard them** -- `FloatToDepth16` keeps only 2 of the 4 dz
bits, and both its call sites pass `dz = 0.0f` with a literal `// TODO`
(`FbWriteDepthCS.hlsl:27`). That is a defensible choice for a GPU renderer
that does its real depth test in `D32_FLOAT` and only serializes to the N64
format at the RDRAM boundary. It is **not** defensible for a CPU rasterizer
whose depth test IS the encoded value: here the dz tolerance directly
decides which pixels pass. So the one available precedent for option (a)
comes from a renderer that never depended on the answer, and cannot be
cited as evidence that (a) is correct for this path.

The secondary reason: with **five** independent censuses showing zero
`G_SETZIMG` and zero Z-variant triangles -- including one driven through
eighteen menu screens -- and no match reachable, a depth rung is
**speculative for this ROM today** by this repo's own measurement. The
project's own recorded lesson, from this very doc ("Measuring what the ROM
emits before scoping which rung to build would have reordered the whole
session"), argues for spending the next unit of effort on reaching a match
rather than building for one.

## 6. What to do instead, in order

1. **Decode the othermode Z bits on the real ROM.** Cheap, answers a
   question nobody has asked, and needs no match. `Z_CMP` is `low & 0x0010`
   and `Z_UPD` is `low & 0x0020` -- confirmed by two independent sources
   (`gbi/types.rs:445-451` and RT64's `rt64_f3d_defines.h:85-86`). The
   instrument already exists: `RT64-WM2000-CYCLE-MODES.md` decoded othermode
   payloads on this ROM for cycle type. If WM2000 latches Z_CMP/Z_UPD while
   issuing only non-Z triangles, the current picture is incomplete in a way
   that matters. If it never sets them, the "no depth" conclusion gets its
   fourth independent leg and depth can be closed out with confidence.

2. **Reach a match.** This is the only thing that converts "unknown" into a
   measurement, and `RT64-WM2000-GAMEPLAY-GAP.md:219-223` already names the
   approach (read the guest's menu state machine rather than guessing
   buttons). It is also the gate on re-checking every other "five flat
   deltas" claim, not just depth.

3. **Only then, if the evidence calls for it, build depth** -- and decide
   the hidden-bit question explicitly and in writing before the first line
   of the rasterizer, because it is not recoverable later by testing.

---

# The othermode Z bits, now decoded: measured

Closes the gap named above ("The genuine gap: nobody has ever decoded the
othermode Z bits"). Measurement only -- no renderer behaviour changed.

## The instrument

`crates/fn64-render-reference/src/gbi/census.rs`, module `othermode` -- an
env-gated aggregate tally hooked at the decoder's `G_RDPSETOTHERMODE` arm
(`gbi/stream.rs`, immediately after the `state.other_mode` latch). Armed by
`FN64_GBI_OTHERMODE_CENSUS`, sink `FN64_GBI_OTHERMODE_CENSUS_OUT`, gated the
same way `texrect` already is (inert under `cfg(test)`; one relaxed load and
return when off). Fixed-size atomics, no per-command rows, so it is safe to
leave armed across a multi-million-command run.

**Bit positions were not re-derived.** The probe takes the latched
`OtherMode` by value and reports through this crate's own accessors, so a
census row and a rasterizer decision cannot disagree:

| field | accessor | bits | site |
| --- | --- | --- | --- |
| Z_CMP | `depth_compare_enabled` | `low & 0x0010` | `gbi/types.rs:445` |
| Z_UPD | `depth_update_enabled` | `low & 0x0020` | `gbi/types.rs:449` |
| ZMODE | `depth_mode` | `(low >> 10) & 3` | `gbi/types.rs:490` |
| ZSRCSEL | `primitive_depth_source` | `low & (1 << 2)` | `gbi/types.rs:437` |

These agree with RT64's `src/shared/rt64_f3d_defines.h:84-101`
(`Z_CMP 0x10`, `Z_UPD 0x20`, `ZMODE_MASK 0xc00`), but the in-tree accessor is
the authority cited, per section 2c's finding that RT64 is the wrong
reference for CPU-side depth work. Four unit tests pin the positions as
literals against the accessors.

## The tallies

Both lead-ins, real ROM (`aki-recomp/games/NWXE/wm2000.z64`), via
`examples/wm2000-census`.

| | attract loop | 18-menu-screen script |
| --- | --- | --- |
| `G_RDPSETOTHERMODE` writes | **2,033,550** | **622,102** |
| Z_CMP set | **0 (0.0000%)** | **0 (0.0000%)** |
| Z_UPD set | **0 (0.0000%)** | **0 (0.0000%)** |
| neither set | 2,033,550 (100%) | 622,102 (100%) |
| ZSRCSEL = primitive | **0 (0.0000%)** | **0 (0.0000%)** |
| ZMODE OPA / INTER / XLU / DEC | **2,033,550 / 0 / 0 / 0** | **622,102 / 0 / 0 / 0** |

The menu run used the same lead-in `docs/tools/wm2000-input-probe.py`'s
`prefix_script` produces (START at swap 1100, then A every 100 swaps),
translated to this harness's `first:end:buttons` comma grammar; it armed 14
phases and ran to swap 2624, past the full lead-in.

**Coverage is total, not sampled.** `G_RDPSETOTHERMODE` (`0xef`) is the
*only* othermode-writing opcode WM2000 issues -- the opcode census over the
same run shows no `G_SETOTHERMODE_H` and no `G_SETOTHERMODE_L` at all, so
there is no path by which a Z bit could be set unobserved. The probe's write
total also matches the opcode census's `0xef` row exactly at every sample
point, confirming no occurrence is missed.

## What this does and does not establish

It establishes that across 2,655,652 othermode writes over two lead-ins,
**WM2000 never once arms the RDP depth pipeline** -- not Z_CMP, not Z_UPD,
not primitive-Z, and never a ZMODE other than OPA (which is also the
all-zeroes value, consistent with the depth fields simply never being
programmed). Combined with the five prior censuses' zero `G_SETZIMG` and
zero Z-variant triangles, the absence is now positive rather than merely
unobserved: the earlier "the census does not decode othermode payload bits,
absence is not absence" caveat no longer applies.

It still does not prove depth is unneeded for a **match**, for the unchanged
reason section 1 gives: no match has ever been reached. What changed is that
the one place the evidence was *absent rather than negative* is now negative
too.

**Recommendation: depth's priority is unchanged and stays low.** 0 of
2,655,652 is the number that justifies it. Revisit only when a window
reaches actual gameplay.

## Secondary finding, recorded to avoid a later contradiction

Two-cycle mode is **not** absent from othermode writes: 342,471 of 2,033,550
(16.84%) on the attract loop and 214,969 of 622,102 (34.56%) through the
menus. This does not contradict `RT64-PLAYABLE-PLAN-REVIEW.md:71`'s "zero
two-cycle programs" or `RT64-WM2000-GAMEPLAY-GAP.md:188`'s "zero two-cycle"
-- those are the `texrect` probe's scope (combiner programs on `G_TEXRECT`
rectangles), a strictly smaller population than all othermode writes. The
two measurements are compatible, but a reader comparing them without this
note would reasonably think otherwise.

Note also that the attract loop's two-cycle count *freezes* at 342,471 while
its one-cycle count keeps climbing -- those writes happen during boot and do
not recur in the steady-state attract loop -- whereas the menu run's keeps
rising throughout. Menu screens use two-cycle continuously; the attract
plateau does not.

## An unpinned behavior found by the frozen-value sweep (test gap, not a defect)

A proactive sweep for the frozen-value/stale-comment defect shape examined 44
candidates and cleared 42 of them: the comments were correct, and the values
they said were unavailable genuinely are. Zero new behavioral defects. Two
stale comments were corrected, no behavior changed.

One candidate is worth recording rather than filing as clean. The texrect path
passes a literal `0` for `max_level` (`gbi/stream.rs:1378`) where the triangle
path passes the latched `tex_max_level`. The value IS available and a sibling
call site does read it, so this matched the defect shape exactly.

It was cleared on an architectural argument: a texrect names its own tile on
the RDP wire (`w1 >> 24`), and the texrect path already ignores G_TEXTURE's
on-bit and tile field, which live in the same word as its level field. Ignoring
max-level is the consistent choice, not an oversight.

**But that clearing rests on reasoning, not evidence.** Mutating the literal
`0` to `tex_max_level` passes all 499 `fn64-render-reference` tests -- nothing
pins the behavior in either direction. If this needs to be defended rather than
argued, the test to add is a LOD-enabled minified texrect fixture, which would
distinguish the two readings. Recorded so the next reader inherits the argument
and its limit together, instead of re-investigating from the same grep hit.
