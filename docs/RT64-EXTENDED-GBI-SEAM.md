# RT64 Extended-GBI fixture and evidence seam

Status: the typed runtime-optional handshake/command encoder, private-input
admission, bounded read-only evidence seam, and non-ROM synthetic cooperation
gates are implemented. The gates reach RT64's real F3DEX2 hook, Extended
dispatch, workload, interpolation, depth-test, and render paths without
changing production microcode recognition. The `extended-gbi`, HFR, aspect,
and ultrawide public behavior rows are closed by the public HLE-dialect
admissions and exact output matrices below. A user-owned production-recognized
microcode/full-ROM run remains a separate release gate; it is not the
denominator for the Extended command-set behavior itself.

This document is a clean-room design derived only from fn64's public adapter
surface and pinned MIT RT64 commit
`f0728a2520d5aa735886240de3fee75cc805f6d6`. It does not use a GPL runtime or
game content.

## What the existing seam proves

`Rt64Backend::process_task` is the correct public entry point: it hands the
task's display-list, microcode-text, and microcode-data addresses to RT64.
There are two independent admission checks before an Extended command can
run:

1. fn64 requires the 4 KiB IMEM image to belong to the configured microcode
   catalog ([`lib.rs`](../crates/fn64-render-rt64/src/lib.rs#L4691)).
2. Pinned RT64 hashes the microcode text and data separately and requires a
   known intersecting GBI instance
   ([`rt64_gbi.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/gbi/rt64_gbi.cpp#L396)).
   The fn64 shim reports a failed match rather than parsing the display list
   under an assumed dialect
   ([`fn64_rt64_shim.cpp`](../crates/fn64-render-rt64/ffi/fn64_rt64_shim.cpp#L1748)).

The recognized microcode text and data bytes are therefore external/private
fixture input. They are ROM-derived and must not enter git. A runnable fixture
must accept explicit paths, record the SHA-256 and byte length of each input,
copy them to declared non-overlapping RDRAM addresses, copy the exact admitted
4 KiB IMEM image into `RspMemory::Imem`, and fail loudly when RT64 rejects the
pair. It must not invent a digest, silently substitute another microcode, or
fall back to raw-RDP submission, because those paths would not test
`process_task`.

`docs/PRIVATE-INPUT-ADMISSION.md` now defines the local-only manifest and
validator for that pair. It requires the complete 4 KiB text image, separate
data bytes, exact local lengths/SHA-256 values, user-owned provenance, and the
six-case denominator below. Its content-free readiness report deliberately
does not claim RT64 recognition; the runtime rejection remains authoritative.

Once a recognized F3DEX2 task is admitted, the wire path itself is present.
F3DEX2 maps opcode `0xE0` to RT64's hook
([`rt64_gbi_f3dex2.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/gbi/rt64_gbi_f3dex2.cpp#L179)),
the Enable hook installs the requested extended opcode
([`rt64_gbi_extended.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/gbi/rt64_gbi_extended.cpp#L334)),
and the interpreter dispatches later commands with that opcode through the
Extended map
([`rt64_interpreter.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_interpreter.cpp#L156)).

The public fn64 observations now certify the command-set behavior while
keeping production admission separately bounded:

| Requirement | Existing observation | Result |
|---|---|---|
| Hook reachability | `gEXGetVersion` writes `G_EX_VERSION` to RDRAM | Live synthetic proof exists; GetVersion alone still does not enable Extended GBI or prove production recognition. |
| Enable plus command dispatch | `Rt64ExtendedGbiEvidence` records the enabled opcode, hook-enable count, and all `0x34` command dispatch counts for one armed completed task. | Live GetVersion, Enable, SetRectAspect, SetRectAlign, SetRefreshRate, MatrixGroup, VertexZTest, and EndVertexZTest dispatches are proven through RT64's real F3DEX2/Extended interpreter. |
| Widescreen/2D treatment | `Rt64PresentedPixels`, `Rt64PresentSelection`, and typed rectangle origin/offset/bounds/aspect evidence expose both semantic input and exact output. | Ten fresh Metal processes cover 4:3, 16:9, 2:1, and 21:9 with transformed 3D, explicit viewport/scissor, Extended 2D origins/aspect, and exact post-VI output through the public HLE-dialect admission. Production hash recognition remains unchanged. |
| Interpolation | Typed transform-group selectors, refresh override, generated-presentation workload/present/rational-fraction provenance, and a bounded ordered post-VI image history are retained. | Extended SetRefreshRate 60 and two stable decomposed Auto-order matrix groups under Manual 120 Hz produce exact 1/2 midpoint and 2/2 endpoint provenance and pixels. Physical scanout timing remains a platform residual, not an Extended-command behavior gap. |
| Vertex-Z | Ordered begin/end evidence records command and resolved vertex indices plus exact contiguous affected face ranges. | Far and near primitive-depth controls retain the ordinary triangle without markers, then prove the enabled visible and fully occluded outputs with one exact three-index affected range. |

Opaque digest differences are used only after the typed fields, source
workloads, rational fractions, geometry, and depth controls are named.

## Synthetic cooperation gate

The non-default `extended-gbi-evidence` Cargo feature depends on the shared
`synthetic-f3dex2-evidence` transport, not on HFR pacing. Its fixture injects
only RT64's built-in F3DEX2 dialect for a hand-authored, non-ROM display list;
it does not add a text/data hash pair to RT64's production GBI database.
`RenderBackend::process_task` is exercised first as a negative control and
must still return `FrameStatus::NeedsLle` for the same unrecognized state.

`rt64_extended_gbi_synthetic_behavior` then exercises six distinct runtime
cooperation outcomes with separately initialized RDRAM cases in one fresh
backend process. Reuse is intentional here: it proves the pinned
workload-boundary Extended-map reset between the enabled and fallback cases,
while the closure fixture below uses fresh backends per independent control:

1. Disabled policy emits no probe and the ordinary display list renders.
2. An actual GetVersion hook writes version `1` to the initialized RDRAM word.
3. Enable plus SetRectAspect dispatch records opcode `0x64`, one hook enable,
   and one typed aspect command.
4. `IfAvailable` resolves an untouched zero response to the byte-identical
   ordinary fallback.
5. `Required` resolves that same missing response to its named error.
6. Enable plus Adjust/SetRectAlign records the public origins and offsets and
   changes both exact rectangle bounds and the RGBA16 target digest.

Ten fresh watchdog-bounded Metal processes against clean pinned RT64 produced
the same ordinary SHA-256
`914dd6b3edcee857f98061e528cfa102b69344b6104867d0f6414c7ab3f5de25`
at bounds x=8..55, y=8..39 and the same adjusted SHA-256
`dbb8bb25a23e67b759bcfd8276aadd036a8e5ea304736a316531645fe0df0553`
at bounds x=12..63, y=8..39. Every process also passed the production
recognition negative control, exact GetVersion/dispatch evidence, optional
fallback, required failure, and target-footprint rejection.

The bridge rejects a color target unless the full configured
`width * height * 2` RGBA16 footprint fits RDRAM, and the fixture requires that
rejection to leave RDRAM unchanged. Extended and HFR evidence arming also join
both RT64 queues and atomically reject overlapping presentation histories
under the capture mutex. The exact closed interleaving is HFR arming after
Extended's former unlocked precheck but before Extended published its flags:
both histories could become active, after which the present hook's
Extended-first branch starved HFR.

The inverse HFR-first overlap assertion and its full pacing/admission suite
passed 20 fresh watchdog-bounded Metal processes. That is the concurrency bar
for the arming fix; the ten-process Extended run above is the separate
deterministic behavior bar.

This gate proves the command mechanics and adapter invariants. The
aspect-specific extension below proves widescreen/ultrawide, and the closure
fixture proves Extended interpolation and vertex-Z through the same narrowly
typed HLE-dialect admission. None of these admits a production text/data hash;
that negative control is what lets command behavior close without weakening
the separate microcode catalog.

### Interpolation and vertex-Z closure

`rt64_extended_gbi_enhancement_behavior` negotiates a required live v1 session
and then uses fresh Metal backends for the behavior controls. Every cooperating
workload emits Enable, SetRefreshRate 60, and two exact decomposed Auto-order
MatrixGroup commands for the projection and model stacks. Under live Manual
120 Hz, three translated workloads produce an exact 161-pixel source,
one-half spatial midpoint, and current endpoint at x sums 4186, 4830, and 5152.
The retained images are bound to distinct source/current workload IDs, one
present ID, exact fractions 1/2 and 2/2, and SHA-256 values `af5e25c1...` and
`b7116e22...`.

The depth cases write either a far or near plane through ordinary primitive-Z
updates, erase only the color contribution, then bracket one triangle with
VertexZTest/EndVertexZTest. The far control remains byte-identical to ordinary
visible output (`b7116e22...`); the near control becomes fully occluded
(`5e9d5b68...`). Typed evidence binds the command vertex and resolved source
index to an exact contiguous three-index affected range. Disabled controls
have no marker evidence and retain the triangle at both depths.

All scene, display-list, color, and depth regions have leading/trailing guards.
Normal `process_task` returns `NeedsLle` before and after every synthetic lane.
Ten fresh 60-second-watchdog-bounded Metal processes produced identical exact
typed and pixel evidence on 2026-07-20.

### Aspect and ultrawide closure

`rt64_hle_aspect_behavior` combines one transformed asymmetric F3DEX2 triangle,
an explicit viewport, a non-full scissor, and one Extended Adjust rectangle
whose left and right edges declare independent Left/Right origins and six-pixel
offsets. It renders twice per setting in one live sequence at 4:3, 16:9, 2:1,
and 21:9. Every non-initial change must return the typed live-applied outcome
with framebuffer discard, and both presentations must have identical semantic
geometry and exact post-VI bytes before the case is accepted.

The evidence arm binds the Enable, SetRectAspect, and SetRectAlign dispatches,
the rectangle's quarter-pixel bounds, workload ID, present ID, selected source,
and post-VI image together. The transformed triangle and aligned rectangle have
different horizontal responses across the four settings, so a global stretch
cannot satisfy the exact shapes. The stable post-VI SHA-256 values are
`6c953ade...` (4:3), `6aaf9487...` (16:9), `3ca118f7...` (2:1), and
`c5aa2d5a...` (21:9).

Ten fresh clean Metal processes against pinned RT64 produced byte-identical
logs (SHA-256 `4f4cae24...`) and passed every exact image and typed evidence
assertion. Normal `process_task` returned `NeedsLle` before and after the
matrix. This closes the public widescreen and ultrawide behavior rows without
claiming that the synthetic test admission is a production microcode hash or
that production-recognized microcode or full-ROM release coverage is complete.

## Runtime-optional cooperation

Extended GBI does not have to be compiled unconditionally into every display
list. The public protocol provides a runtime handshake, and fn64 exposes it as
typed Rust in
[`extended_gbi.rs`](../crates/fn64-render-rt64/src/extended_gbi.rs):

1. `Policy::Disabled` emits no probe or Extended command.
2. `Policy::IfAvailable` emits `GetVersion`; a return word that remains at its
   required zero initializer selects the ordinary display-list path.
3. `Policy::Required` emits the same probe but rejects a missing response by
   name rather than rendering with absent cooperation.
4. A completed response of exactly `1` yields `Version1`. Any other nonzero
   version is unknown and is rejected instead of being parsed as v1.

The probe and its result cross a graphics-task boundary. `GetVersion` writes
RDRAM while RT64 processes the task, so guest CPU code cannot place a probe
and conditionally emit `Enable` later in that same already-built display list.
It initializes a word to zero, submits and joins the probe task, reads the
result, and only then chooses commands for a later task. The local-only
six-case example follows that ordering: the hook-control result constructs the
typed v1 session used by later cases. Every later cooperating display list
still emits `Enable`, because RT64 resets the Extended map at the workload
boundary.

The typed F3DEX2 encoder validates the public 28-bit word-aligned return
address, exposes only the header's default `0x64` Extended opcode, rejects a
zero refresh rate, and only constructs named v1 operations. It deliberately
does not accept arbitrary nonzero opcodes: the public header provides no safe
custom-opcode range, and colliding with an ordinary F3DEX2 command would be
ambiguous. Exact public-header vectors cover Enable/Disable, rectangle
aspect/alignment, refresh, matrix groups, and vertex-Z. The exact-vector tests
prove deterministic policy and encoding. The live gates above separately prove
RT64 dispatch and the advertised widescreen/interpolated/depth behavior.

## Exact command vectors

The fixture uses the typed encoder and its exact-vector tests before
submission. This keeps the C header macros as the oracle without compiling
user game content.
The pinned header defines the F3DEX2 hook opcode as `0xE0`, magic as `0x525464`,
default Extended opcode as `0x64`, and version as `1`
([`rt64_extended_gbi.h`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/include/rt64_extended_gbi.h#L8)).

| Operation | Word 0 | Word 1 / continuation |
|---|---:|---:|
| GetVersion | `0xE0525464` | low 28 bits are the segmented return address |
| Enable opcode `0x64` | `0xE0525464` | `0x10000064` |
| SetRectAlign | `0x64000006` | packed 12-bit left/right origins; the next command contains four signed 16-bit offsets |
| SetRefreshRate 120 | `0x64000009` | `0x00000078` |
| VertexZTest vertex 3 | `0x6400000A` | `0x00000003` |
| EndVertexZTest | `0x6400000B` | `0x00000000` |
| MatrixGroup | `0x6400000C` | group ID; the next command packs the selectors in word 0 and has zero word 1 |
| SetRectAspect Adjust | `0x64000033` | `0x00000002` |

The matrix selector layout and component values come directly from the public
header
([`rt64_extended_gbi.h`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/include/rt64_extended_gbi.h#L96),
[`gEXMatrixGroup`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/include/rt64_extended_gbi.h#L317)).
Rectangle alignment, viewport alignment, and scissor alignment must likewise
be emitted from the pinned public macros rather than inferred from pixels.

## Separate production-recognition/full-ROM contract

The local-only production example creates a fresh RT64 backend for every case
so hook state, workloads, and presentation IDs cannot leak across controls.
Each case uses the same recognized text/data pair, initial RDRAM image, VI
state, output target, and ordinary F3DEX2 setup commands. Every display list
includes its own Enable command because RT64 disables Extended GBI at the
workload reset boundary
([`rt64_state.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_state.cpp#L1795)).

This matrix remains required when certifying a user-owned production-recognized
microcode or full-ROM release, even though the public command-set behavior row
is already closed:

1. **Hook control:** GetVersion followed by ordinary EndDL. Assert the return
   word becomes exactly `1`; no Extended-command dispatch is claimed.
2. **Disabled negative control:** submit the chosen `0x64` command without
   Enable. Assert zero Extended dispatches and no command-specific evidence.
   Do not use the resulting image alone, because an unknown opcode is logged
   and skipped by pinned RT64.
3. **Activation:** Enable, SetRectAspect, SetRectAlign, an ordinary asymmetric
   FillRect, FullSync, then EndDL. Assert the enable opcode, dispatch counts,
   typed rectangle evidence, and exact control-versus-enabled pixel geometry.
   This proves activation, not the other inventory requirements.
4. **Widescreen:** render asymmetric 3D and 2D anchors at native, 16:9, and an
   ultrawide ratio. Pair exact pixel bounds/digests with viewport, scissor,
   rectangle-origin, and aspect evidence so a global stretch cannot pass.
5. **Interpolation:** submit two workload states with one stable matrix-group
   ID, deliberately asymmetric transforms, explicit component selectors, and
   a 120 Hz Extended refresh override. Assert the recorded group metadata,
   endpoint presentations, generated-presentation count, interpolation
   fraction/order, and at least one exact intermediate transformed anchor.
6. **Vertex-Z:** load a declared vertex, bracket known geometry with
   VertexZTest/EndVertexZTest, and use visible and occluded depth controls.
   Assert begin/end markers, resolved source vertex, bounded affected triangle
   counts, and exact enabled/control pixel results.

All RDRAM regions need leading/trailing guards. Every case records the pinned
RT64 source identity, adapter identity, task addresses, microcode input hashes,
active runtime-policy digest, workload/present IDs, typed Extended evidence,
and presented-pixel SHA-256. Ten consecutive clean runs with identical
semantic evidence are required before any deterministic behavior is reported
as closed. A future synchronization change has the separate 20-run race bar
and must name the interleaving it closes at the fix site.

## Implemented read-only evidence seam

The adapter now exposes a bounded, statically sized snapshot captured from the
armed completed workload. The F3DEX2 observer is synchronous and pass-through;
RAII restores RT64 pointers before `process_task` returns. Reading waits for
the existing queue-idle boundary and rejects stale slots, ambiguous global
state, overflow, invalid tags, nested/mismatched vertex markers, and skipped or
incomplete interpolation groups. Its public Rust image follows this shape:

```rust
pub struct Rt64ExtendedGbiEvidence {
    pub workload_id: u64,
    pub present_id: u64,
    pub enabled_opcode: u8,
    pub hook_enable_count: u32,
    pub command_counts: [u32; 0x34],
    pub refresh_rate: Option<u16>,
    pub rects: [Rt64ExtendedRectEvidence; MAX_EXTENDED_RECTS],
    pub rect_count: u32,
    pub groups: [Rt64TransformGroupEvidence; MAX_TRANSFORM_GROUPS],
    pub group_count: u32,
    pub vertex_z: [Rt64VertexZEvidence; MAX_VERTEX_Z_MARKERS],
    pub vertex_z_count: u32,
    pub generated_present_count: u32,
}
```

`Rt64ExtendedRectEvidence` needs the draw-call UID, left/right origins,
offsets, bounds, and rectangle aspect. `Rt64TransformGroupEvidence` needs the
group ID, projection/model classification, push/decompose/editable flags,
position/rotation/scale/skew/perspective/vertex/texcoord/tile/look-at selectors,
ordering and aspect mode. `Rt64VertexZEvidence` needs begin/end kind, command
vertex index, resolved source index, and affected face-index range. Generated
presentation evidence additionally needs its source workload pair and exact
interpolation fraction; a count without provenance is not behavioral proof.

These are not speculative internals: pinned RT64 stores the matrix selectors
in transform groups
([`rt64_rsp.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_rsp.cpp#L1193)),
stores the refresh override in the workload
([`rt64_state.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_state.cpp#L1587)),
and emits typed vertex-Z markers around degenerate draw calls
([`rt64_rsp.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_rsp.cpp#L1175)).
The adapter preserves those names in its bounded typed evidence image instead
of reducing the workload to counts or opaque digests.

The 1,984-byte semantic C ABI image is bounded with compile-time layout
assertions and strict Rust decoding. An additional fixed-eight-entry Metal
history retains a distinct readback buffer for every generated draw and
exposes each image through a 72-byte indexed metadata record plus two-phase
tightly packed BGRA retrieval. The existing present-queue idle boundary joins
the full generated burst before provenance is finalized; counts, ordinals,
workload/present IDs, rational fractions, format, geometry, and byte lengths
are checked before Rust exposes a pixel.

Static and isolated FFI tests cover semantic decoding, exact history bytes and
fractions, overflow, invalid tags/geometry/provenance, and null-context loud
failure. This is implementation evidence only. The history closes a
later-generated-blit-overwrites-an-earlier-readback-buffer interleaving by
retaining one buffer per hook draw, but it is a synchronization change and has
met the required 20-process bar through the HFR cooperation gate, which uses
the same fixed-slot capture ownership and validates both retained images in
every process. The Extended-specific deterministic behavior has its separate
ten-process bar above.
