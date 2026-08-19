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
