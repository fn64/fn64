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
| texture s/t/w + perspective divide (0x0a/0x0e) | **NOT DONE** | planes decode; nothing calls them |
| depth (0x09 and friends) | **NOT DONE** | not started |

**A raw triangle is NOT yet visible in a WM2000 frame.** The ROM issues only
opcode 0x0e (shaded AND textured); the decoder refuses `textured()`, so zero
of WM2000's raw triangles reach the executor. The two finished rungs are
proven correct on synthetic fixtures through the real decoder and the real
guest-commit path -- they are not proven on a real WM2000 triangle, because
the ROM emits none this backend admits.

`texture_planes` decodes the S/T/W coefficient block and has **no callers
outside its own tests**. Decoding it is not the texture rung.

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
TRIDECL (raw triangles reaching the admission check)  116,958+
TRIEXEC (raw triangles reaching the CPU executor)           0
```

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
