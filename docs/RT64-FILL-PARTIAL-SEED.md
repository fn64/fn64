# The partial-fill seed: where untouched pixels come from

Written while fixing two `fn64-render-wgpu` fill defects the wgpu differential
runner (`crates/fn64-render-conformance`, feature `wgpu-runner`) caught against
`fn64-render-reference`. Defect 1 (fills not scissor-clipped) is mechanical.
This doc is about defect 2, which is not.

## The refusal, and why its premise is real

`TargetError::PartialNewTargetInitialization` (`targets/mod.rs:434`) refuses any
fill that does not cover the whole target when that target has no predecessor
resident. Its stated reason -- restated in
`fn64-abi/src/task_dispatch/tests/raw_dpc_session_integration.rs:739-741` as
"its untouched rows would be fabricated zeros" -- is genuinely correct about the
code as written. `execute_fill_rectangle`'s `None` arm allocates
`vec![0u8; full_len]` (`targets/fill.rs`), so admitting a partial rectangle
would publish a resident whose unfilled pixels are zeros that no guest byte and
no RDP command ever produced.

Hardware has no such problem. The framebuffer is RDRAM; the bytes outside a
fill are whatever was already there. So the fix is to give those pixels their
real value, not to refuse the draw -- and not to delete the guard, which would
trade a loud refusal for silent garbage.

## What the oracle does -- CONFIRMED

`fn64-render-reference` completes every partial-fill case the differential
sweeps. It does it by seeding its target from guest RDRAM before rendering:

- `backend/imp.rs:440-447` (`prepare_reference_task`) calls `load_color_image`
  for the active target on every non-`Simple` decode mode, and
  `process_rdp_commands` sets `DecodeMode::RawRdp` (`render_backend.rs:140`),
  so the raw-RDP path takes this branch.
- `backend/framebuffer_io.rs:12-44` (`load_rgba5551_framebuffer`) decodes each
  guest halfword into the internal RGBA8 target, expanding 5-bit channels by
  `(v << 3) | (v >> 2)` and recovering coverage from the visible LSB plus the
  hidden-bit sidecar.
- `raster/draw.rs:209-220` then writes strictly inside the clipped rectangle,
  touching nothing else.
- `framebuffer_io.rs:123-180` writes the WHOLE extent back.

So the reference's untouched pixels are the pre-existing guest bytes, round
tripped through RGBA5551 -> RGBA8 -> RGBA5551. That round trip is exact:
`expand` followed by `>> 3` is the identity on 5-bit values.

The differential's own hand-derived key says the same thing independently. Its
framebuffer is seeded with `STALE = 0xffff`, and the keys for
`top-left-quadrant`, `single-pixel`, `last-column-last-row` and
`scissor-narrower-than-rect` all expect `STALE` outside the filled region
(`fn64-render-conformance-wgpu-runner.rs:603-690`). Two independent authorities
agree, and neither is the code under test.

## What wgpu can and cannot see -- CONFIRMED, verified line by line

**No existing mechanism gives this backend the guest's framebuffer bytes.**

- Every colour access wgpu declares is a write: `raw_dpc/mod.rs:2205-2213`
  (`plan_render_target_rows`, the fill path) and `:2060-2068`
  (`plan_raw_triangle`), both `AccessMode::Write` / `AccessPurpose::RenderTarget`.
- `execute_raw_dpc` takes only a `BoundSubmittedRawDpc`
  (`fn64-render/src/lib.rs:1919`; wgpu impl `production.rs:2259`) -- no RDRAM
  slice, no byte-source trait, no host callback.
- The only guest bytes reaching execution are the ones the PLAN declared,
  delivered by `captured_reads` into `ExecutionCollector.reads`
  (`production.rs:1507-1512`) and consumed only by `load_source_bytes` for TMEM
  loads (`production.rs:1969-1990`, used at `:3257`).
- The one other guest-memory path, `PhysicalRdramRead` via `vi_scanout`, exists
  only in `present()` (`production.rs:2143-2160`), never during execution.
- The seed is `None` for a new target because it comes solely from the registry
  (`production.rs:3697-3706`), which is deliberately not mutated during
  execution (`production.rs:1370-1376` documents exactly why).

**But the vocabulary already permits the read, and the plumbing is confined to
`fn64-render-wgpu`.** Each of these was re-read directly rather than taken on
report:

- `ResourceAccess::try_new` forces `TmemLoadSource` to `AccessMode::Read`
  (`fn64-render-ir/src/journal.rs:264-278`) and explicitly permits
  `RdramResource::ColorFramebuffer` for `UploadSource | TmemLoadSource`
  (`:304-312`). So the access constructs today, with no IR change.
- `DeferredGuestReadPlan::try_from_journal` selects reads purely by
  `purpose == TmemLoadSource` and ignores the resource entirely
  (`fn64-render-ir/src/guest_read.rs:66-107`).
- The ABI capture slices live RDRAM by `read.range()` with no resource
  inspection at all (`fn64-abi/src/task_dispatch/rsp_commit.rs:1161-1182`,
  rdram acquired at `:1044`).
- A live precedent already constructs `Read`/`TmemLoadSource`/`ColorFramebuffer`
  and drives it through the production capture function
  (`fn64-abi/src/guest_read_capture.rs:114-121`).

## The write-back is narrower than the buffer -- CONFIRMED

Worth stating because it is easy to assume otherwise, and it bounds the blast
radius of the zeros.

Production copies back only the DECLARED WRITE RANGES, one payload per
`CompletedWrite`, sliced out of the full-extent buffer at each write's own
physical range (`production.rs:2364-2402`, consumed by
`copy_committed_guest_writes` at `fn64-abi/src/task_dispatch/rsp_commit.rs:1402`).
It does NOT blit the whole extent.

The differential's runner is deliberately WIDER than production here: it splices
`published[..FRAMEBUFFER_BYTES]` -- the entire resident -- back into its RDRAM
copy (`fn64-render-conformance-wgpu-runner.rs:~430`). So the sweep observes
buffer pixels that production would never copy. That makes it a stricter
instrument than production for this defect, not a looser one, and it is the
reason the sweep can see a fabricated zero at all.

## Provenance

Every claim above is CONFIRMED by reading the cited line in this worktree at
`port/rt64-conveyor` HEAD 7af110b0. No ROM was run for any of it. The
`fn64-render-reference` behaviour was traced through its own source, not
executed in isolation -- though the differential does execute it, and its
agreement with the hand-derived key is the measured half of that claim.
