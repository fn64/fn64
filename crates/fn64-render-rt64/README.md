# fn64-render-rt64

This crate currently contains both fn64 graphics backends while the reference
backend is being extracted from the native adapter boundary:

- `ReferenceBackend` is the deterministic, pure-Rust software rasterizer used
  by the default build and headless CI.
- `Rt64Backend` is an opt-in C ABI wrapper around RT64's MIT C++ render/HLE
  library. It is enabled with the crate's `rt64` Cargo feature.

Executable native behavior gates live in `fn64-certification`. Temporary
one-line compatibility examples here include those canonical sources so old
commands remain valid without changing the evidence-bound adapter manifest.

Geometry HLE uses content-addressed admission. The reference backend registers
each exact 4 KiB text SHA-256 with an explicit Fast3D, F3DEX, F3DLX,
F3DLX.Rej, public F3DEX2 variant, L3DEX, or L3DEX2 wire family; colliding opcode bytes never
select the family. The RT64
adapter remains explicitly F3DEX2-only. `with_f3dex2()` selects the reference
decoder but does not trust the task-entry IMEM image. Task-entry and changed
`G_LOAD_UCODE` images may enter HLE only when their exact identity and family
were configured. Otherwise the backend returns `FrameStatus::NeedsLle` without
mutations and the runtime replays the complete ucode phase from post-rspboot
state through its general interpreter. The reference preflight runs on cloned
RDRAM/RSP state for admitted images that self-load; the catalog stores
identities only, never ucode or game bytes. See
[`FAST3D-F3DEX-CONCEPTS.md`](FAST3D-F3DEX-CONCEPTS.md) and
[`F3DLX-CONCEPTS.md`](F3DLX-CONCEPTS.md),
[`F3DEX2-VARIANTS.md`](F3DEX2-VARIANTS.md), plus
[`L3DEX-CONCEPTS.md`](L3DEX-CONCEPTS.md).

The reference backend also has a separate content-admitted S2DEX/S2DEX2 slice.
`with_s2dex()` selects it, while each exact 4 KiB identity is registered with
an explicit legacy S2DEX or S2DEX2 wire family; the source-compatible admission
methods mean S2DEX2. This typing is load-bearing because the public families
assign colliding command bytes. The decoder reads public object payloads,
programs their TMEM-backed render tiles, and lowers them to existing typed
texture-rectangle/triangle paths. The admitted slice includes block/tile/TLUT
object loads and compound load/draw commands, full/sub object matrices,
matrix-relative rectangles, rotating sprites, S/T flips, matched bilerp
correction mode, Copy/OneCycle background strip loading, conditional display
lists, public segment pointers, and general-status writes. Typed object modes
also cover exact WIDEN edges, in-domain NOTXCLAMP, Average filtering, and the
current header's ignored XLU/AA bits; combination-specific unpublished
perimeter arithmetic and wrapped/subpixel/filter-corrected backgrounds remain
loud named frontiers.
See [`S2DEX-CONCEPTS.md`](S2DEX-CONCEPTS.md) for the exact frontier.

The admission catalogs and public raw-RDP FullSync inspector live in
`fn64-render`, not in either backend. This crate consumes those shared
mechanisms while the decoder, rasterizer, and native adapter remain separated
in later extraction steps.

The transactional geometry preflight now freezes one shared
`TaskAdmissionPlan` before native entry. Generation zero binds task-entry
text/data identities and family; every admitted `G_LOAD_UCODE` appends another
generation in executed command order. Repeated addresses and exact duplicate
identities are retained, so same-address content replacement and `A -> B -> A`
cannot collapse into a set or final-address check. The native adapter consumes
the immutable plan at RT64's pre-cache `loadUCodeGBI` boundary. It checks the
live raw recognition bytes, forces content recognition for every generation,
and requires exact source/address order and plan exhaustion. Unknown or
incompatible generations return typed `NeedsLle` before interpreter mutation;
a missing, extra, reordered, or changed generation after native execution
starts poisons the context and fails loudly. A focused, backend-neutral walker
in `fn64-render` now produces the complete entry/self-load plan, entry-inclusive
raw recognition windows, and exact public-command FullSync count from immutable
inputs. RT64 production task submission no longer invokes the reference
decoder; a structural test rejects any reintroduction of its inspect, execute,
decode, trace, or `RenderOp` paths. This removes the geometry dependency that
previously blocked extracting `ReferenceBackend` from this crate.

Device-free tests cover ABI identity, typed precommit rejection, exact plan
exhaustion parsing, and retained same-address `A -> B -> A` raw windows. The
public synthetic Extended-GBI transport does not call production
`process_task`, so its live Metal pixels are not positive observer evidence.
A positive native task-entry/self-load series still requires an admitted
allowed/private microcode input through the existing fail-closed manifest
path; until that runs, native compilation and negative fallback are verified
but positive production observer execution is not.

The reference lane also keeps the RDP device alive across task boundaries.
Other mode, combiner/key/convert and constant-color registers, fill color,
scissor, the texture-image latch, all eight tile descriptors, TLUT, and the
physical 4 KiB TMEM image are shared by admitted HLE tasks and raw DPC
submissions. `G_TEXTURE` enable/tile/scale remains RSP-owned and resets per
task. Enabling it without any live TMEM load is a named failure, never an
implicit white texture.

The same device boundary owns the RDP color-image register. Production
F3DEX2 and raw-DPC color operations require a current or persistent
`G_SETCIMG`; the VI scanout address and `process_task`'s compatibility
`output_addr` are not substitutes. Only the fixture-only simple decoder may
use `output_addr` as an implicit RGBA16 target. A persistent color image is
re-imported from RDRAM at each production task boundary, so intervening CPU or
device writes are not overwritten from a stale host RGBA cache.

One/two-cycle reference rendering implements the screen-registered RGB
MagicSquare/Bayer and alpha Pattern/InversePattern selectors for its supported
RGBA16 and RGBA32 color-image layouts. Combiner NOISE, RGB Noise, alpha Noise,
and `G_AC_DITHER` share one typed eight-bit sample per covered fragment. The
default SplitMix64 stream is explicitly seedable and deterministic so reference
framebuffer digests remain reproducible while still varying from pixel to pixel
and frame to frame. It is a host emulation policy, not a claim to reproduce the
unpublished silicon generator described by Nintendo 64 Programming Manual
section 15.5. Disabled dither retains the known exact truncation path: RGBA16
RGB and RGBA32 memory alpha reduce with `>> 3`. Copy and fill cycles retain
their documented blender-bypass behavior; `G_FILLRECT` in one/two-cycle mode
instead follows the supported combiner, alpha-compare, depth, blender, dither,
and color-write path with an exclusive lower-right edge. Copy-cycle I8, packed
IA8, and undereferenced CI8 preserve the original TMEM byte in an 8-bit color
target while alpha compare consumes the source format's intensity, expanded
IA8 low nibble, or CI8 index; RGBA16 retains its one-bit write-enable rule.

VI scanout is isolated from that RDP dither stage in `src/vi.rs`. Every live
presentation receives a retrace-scoped physical-RDRAM capability and rereads
the exact 24-bit `VI_ORIGIN` with the effective 12-bit `VI_WIDTH`; RGBA16 and
RGBA32 decoding, padded stride, field-specific origins, checked source bounds,
and blank/inactive no-read behavior have exact reference vectors. The resident
RDP image is never a fallback for live registers. Native RT64 binds that same
allocation only for the synchronous foreign call, waits its workload and
presentation queues idle, then restores placeholder aliases before returning.
It validates the backend-neutral `fn64-render` programmed source footprint
rather than borrowing the reference renderer's policy-specific filter halo.
Compatibility geometry can still drive standalone behavior fixtures, but only
a completed live-register present can produce fixed-cycle release capture.
The reference lane also implements the public full-coverage RGBA16 3x3
restoration mechanism, partial-coverage silhouette AA, horizontal median divot,
vertical-then-horizontal resampling or replication, a deterministic integer
square-root gamma policy, and seven-bit stochastic gamma quantization.
`docs/VI-FILTERS.md` records exact vectors and the honest frontier: fixed-point
and border details, the silicon gamma ROM and random stream, and post-DAC analog
video remain unproven.

The live Metal `rt64_vi_filter_behavior` gate supplies the missing native
post-VI pixel observation without promoting RT64 to a silicon oracle. Fourteen
live-register phases retain one workload, strictly advancing presents, exact
8x6 BGRA8 geometry, and six byte-identical baseline restorations. Pinned RT64's
gamma and nonidentity X/Y scale produce stable distinct images; gamma dither,
divot, RGBA16 dither restoration, and the four AA selector values are retained
as exact pixel-inert residuals because the pinned native VI shader implements
only sampling, border, and gamma. The backend-lifecycle case runs this gate in
a separate context so later source changes cannot silently convert that known
residual into an unreviewed behavior claim. Ten fresh Metal processes retained
the standalone gate's three exact baseline, gamma, and scaled SHA-256
identities; the expanded full backend-lifecycle process then re-earned its
20-clean-run platform bar with the gate included.

Raw RDP decode accepts the unassigned second word of Load/Pipe/Tile/Full Sync,
while the atomic F3DEX2 macro path keeps its stricter reserved-payload check.
Wrapped, unclamped tile axes derive their active domain from the nonzero mask
instead of unused clamp bounds, and `G_LOADTILE` uses each edge's integer part
for the inclusive source span while retaining unequal fractional quarters as
the subtexel bounds. The shared sampler now enforces the public signed S10.5
post-perspective input range, then carries tile shifts in a distinct wide host
accumulator through signed floor/remainder, addressing, and three-nearest
weights as integers. Float-to-grid and filter-output rounding remain explicit
bounded policies pending hardware traces.

The RT64 adapter implements that fallback's raw-RDP half as well. Bounded DRAM
or staged XBUS ranges cross the C ABI through RT64's MIT public embedding entry
`Application::processDisplayLists(memory, start, end, false)`, wait for the
exact render-to-RAM workload, and retain the VI output selected by the rejected
task. The raw renderer call carries that VI output explicitly, so CPU-only DPC
streams do not depend on backend call history. Unknown microcode therefore
does not require RT64 to recognize its GBI.

Native HLE task submission returns a schema-checked typed result. The result
binds the immutable plan SHA-256, planned/observed generation counts, typed
completion or precommit `NeedsLle` disposition, rejected generation, whether
RT64 admitted the entry GBI, workload IDs before and after the call, and RT64's
initial/final microcode address pair. A complete result must exhaust the exact
plan; initial/final addresses remain diagnostic rather than admission
authority. In pinned RT64, only the public FullSync path advances the state
workload ID, so the delta provides native `Reached`/`NotReached` evidence and
preserves the exact count when a task synchronizes more than once. While the
shared transactional walker supplies the public-command count, it must agree
exactly with the native delta or the adapter drops the already-mutated native
context and fails closed.

The C++ and every Rust `unsafe` block used to call it are quarantined here, as
required by `docs/DESIGN.md` section 1. `fn64-render`, `fn64-runtime`, and the
Rust recompiler remain unaware of RT64 types and continue to forbid unsafe
code.

## Building the RT64 path

The build expects the sibling RT64 checkout at
`../no-mercy-recompiled/third_party/rt64` relative to this repository. Set
`FN64_RT64_DIR` to use another checkout:

```sh
FN64_RT64_DIR=/path/to/rt64 cargo build -p fn64-render-rt64 --features rt64
```

`build.rs` checks that the checkout carries RT64's MIT license, configures the
checked-in wrapper CMake project with `RT64_STATIC=ON`, and builds only the
`fn64_rt64_shim` target. That target pulls in RT64's static `rt64` render/HLE
target and its permissively licensed static dependencies (`re-spirv`, `nfd`,
`zstd`, and `plume`). The current RT64 tree defines no mupen plugin target; its
GPL mupen64plus subtree is neither compiled nor linked. RT64's shader compiler
helpers still run at build time because the core render library embeds its
generated shaders.

The default feature set does not invoke CMake, link a graphics library, or
require a GPU. Constructing `Rt64Backend` without the feature returns a named
`RenderError::Backend`. With the feature enabled, display/GPU initialization
failures are converted to the same error type, allowing a caller to install
`ReferenceBackend` instead. On macOS the RT64 API currently hard-requires a
swapchain even when `renderToRAM` is enabled. The shim therefore owns a hidden
SDL Metal surface, passes RT64 the validated native `NSWindow` and
`CAMetalLayer` handles, and never exposes that window to fn64. Hosts without
WindowServer or a Metal device fail creation cleanly and take the reference-
backend path.

Metal resize is an intentionally pin-sensitive adapter seam. Plume's present
worker otherwise reads an old cached Cocoa size, queues its refresh onto the
blocked main thread, and can publish the creation-size framebuffer after a
host resize. The Apple shim drains and locks that worker, synchronizes the
native SDL/Cocoa geometry on the caller's main thread, applies the pinned
`MetalSwapChain::resize` field updates, clears cached present framebuffers, and
publishes the new shared size before another present can enter. A backend/type
or native-geometry mismatch terminates loudly. The synthetic
`rt64_metal_backend_behavior` gate then requires the first and second distinct
post-resize presents to be 8x4 and recreation to return to 4x2; 20 consecutive
clean live runs close the cross-thread validation bar.

For capture-path diagnosis, `FN64_RT64_PRESENT_DIAGNOSTICS=1` prints one
read-only transition per `present`: the RT64 workload ID before/after
`updateScreen`, present ID before/after it, and Metal capture generation
before/after the present-queue wait. It also labels each workload/present ID,
idle, application-end, capture-unregister, and native-surface shutdown
boundary. The switch does not alter queueing, waiting, capture validation, or
error behavior and is not release evidence.

## Typed RT64 runtime policy

`fn64-render` exposes `RenderRuntimeSettings`, a complete typed image of the
public fields in the selected pinned MIT RT64 `UserConfiguration`. Enum values
cannot carry unknown tags, numeric wrappers reject non-finite/out-of-range
values, and its versioned canonical bytes/SHA-256 bind all 19 fields. `Default`
is fn64's hardware-faithful/off profile: original resolution, 1x multiplier,
and original 2D scale. `RenderRuntimeSettings::upstream_default()` explicitly
selects RT64's different constructor profile: integer window scale, 2x, and
scaled-only 2D. The names prevent either profile from being mistaken for the
other.

`RenderEnhancementSettings` covers all eight pinned
`EnhancementConfiguration` fields, including Console, SkipBuffering, and
PresentEarly latency modes. Its `Default` is fn64's faithful/off profile;
`upstream_default()` names RT64's SkipBuffering, border-removal, correction,
and S2DEX fast-path profile explicitly. `RenderEmulatorSettings` covers all
four pinned dither/framebuffer controls; its default and upstream profile are
identical. `RenderRuntimePolicy` canonically composes the user, enhancement,
emulator, and replacement family encodings into the SHA-256 bound to release
capture.

Before `create`, `RenderBackend::apply_runtime_settings` stages the complete
image. After creation, resolution/downsample/filtering, aspect and extended
aspect, 2D upscale, three-point filtering, refresh, MSAA, hardware resolve,
idle work, and developer mode apply live. MSAA follows RT64's heavyweight
`Application::updateMultisampling` path and rejects unsupported device sample
counts; hardware resolve follows the pinned inspector's shader-library path.
Resolution mode/manual multiplier and aspect mode/manual target pass the same
framebuffer-discard boolean as RT64's `resConfigChanged` group. Graphics API,
display buffering, and internal color format are setup-owned; changing one
returns `RestartRequired` and leaves the active image untouched. The C shim
validates every scalar before assignment and never calls RT64's clamping
validator, so invalid input is a named failure rather than a changed setting.

Enhancement and emulator settings stage before create and apply live through
RT64's `updateEnhancementConfig` and `updateEmulatorConfig` methods. Every
field crosses the strict C ABI as a typed enum or validated boolean. A failed
live update invalidates fn64's active-policy identity and prevents release
capture until recreation; evidence cannot silently retain the pre-error hash.

The force-branch field now also has
causal render evidence: ten fresh Metal processes executed one public
`gSPBranchLessZraw`-shaped F3DEX2 fixture with a deliberately false depth
condition, switched `f3dex.forceBranch` off/on/off live, and produced exact
red/green/red 161-pixel triangles with stable, distinct policy and post-VI
digests. The non-default synthetic transport does not alter production
microcode recognition; unknown production tasks returned `NeedsLle` before
and after every evidence interval. See
[`RT64-FORCE-BRANCH-EVIDENCE.md`](../../docs/RT64-FORCE-BRANCH-EVIDENCE.md).

The texture-LOD scale field has a separate ten-fresh-process causal Metal
fixture. At manual 2x resolution, the same public F3DEX2 two-mip triangle
selects 259 green lower-mip pixels with the control off, 259 red base-mip
pixels after a live enable, and exact green again after restoration. Exact
policy and post-VI hashes are bound by
[`RT64-TEXTURE-LOD-EVIDENCE.md`](../../docs/RT64-TEXTURE-LOD-EVIDENCE.md).
The non-default transport again leaves production microcode recognition
unchanged.

The S2DEX framebuffer fast-path field has a separate non-default S2DEX2
fixture. One prior managed 64×64 RGBA16 framebuffer drives the public
`G_BG_1CYC` slice path: live enable coalesces the exact downstream workload
from three ordered tile copies to one, live restoration returns to the same
three-load digest, and all phases keep identical post-VI and target-RDRAM
hashes. A fast-path-on ordinary-RDRAM arm stays on three CPU uploads. See
[`RT64-S2DEX-ENHANCEMENT-EVIDENCE.md`](../../docs/RT64-S2DEX-ENHANCEMENT-EVIDENCE.md).
The same fixture also closes the bilerp-mismatch field on Metal. Point/point
and bilerp/bilerp controls retain identical complete downstream load programs
with the fix off/on. A bilerp-object-mode/point-RDP-filter mismatch instead
changes from three loads to the exact point/point two-load program when enabled
and restores the original three-load digest when disabled. All phases retain
exact target-RDRAM and post-VI identities. Pinned RT64's missing object draw
handlers remain a separate microcode-coverage gap.

The isolated `rt64_latency_present_early_behavior` gate seeds RT64's VI
history with one explicit Console-mode present, then distinguishes the two
process-time behaviors without another explicit `present`: Console leaves the
seeded capture and its ID unchanged, while a live-applied PresentEarly policy
must publish a fresh, exact Metal capture from the raw-RDP FullSync. The shim
waits that exact present ID and capture generation before returning the RDRAM
borrow, closing the workload-complete/present-capture interleaving:

```sh
cargo run -p fn64-certification --features rt64 --example rt64_latency_present_early_behavior
```

Twenty consecutive diagnostics-off live Metal runs against clean pinned RT64
produced identical history, early-capture, RDRAM, and policy digests. Each run
kept the Console process-time ID at 1 and published PresentEarly ID 2 with the
same exact valid-pixel and opaque-border bands.

Texture replacements cross a separate strict seam because RT64 owns them in
`TextureCache`, not an application configuration update. Ordered directory or
`.rtz` inputs can stage before create, load live, reload through RT64's full
clear-and-load path, and toggle live enable state. Inspection rejects duplicate
canonical roots, symlinks, special/non-UTF-8 paths, malformed or missing
`rt64.json`, future hash versions, and ambiguous `auto` defaults. The active
policy records order, exact content and raw-database SHA-256 values, RT64/Rice
naming, preload/stream/stall, default shift, and effective configuration/hash
versions. Absolute host paths never enter the digest. Pre/post inspection and
post-activation hashes make loads transactional for evidence; release capture
re-inspects the filesystem once more so later mutation of a streamed pack also
invalidates active identity instead of claiming stale bytes.

This is intentionally the complete pinned user, enhancement, emulator, and
texture-replacement control surface, not a claim that every RT64 behavior is
certified. The complete family and mutation-class denominator, including
startup, game-cooperation, and build surfaces, lives in
[`docs/RT64-RUNTIME-CONTROLS.md`](../../docs/RT64-RUNTIME-CONTROLS.md).

The Apple/Metal behavior gate
`crates/fn64-certification/examples/rt64_texture_replacement_behavior.rs` builds
only synthetic RGBA16 TMEM and generated RGBA8 DDS inputs. It discovers the
exact hash from RT64's
live cache, proves RT64 and Rice auto-path selection in pixels, and distinguishes
multi-mip from single-mip minification. For Stream it pauses only a quiescent
pinned RT64 worker set, holds a real resolved/queued job, presents the unchanged
base fallback while that queue remains unchanged with zero loads, recreates the
exact worker count, and then requires worker completion, cache installation,
and changed final pixels. Its waits are bounded by deterministic no-progress
iteration caps and do not use elapsed-time sleeps:

```sh
cargo run -p fn64-certification --features rt64 --example rt64_texture_replacement_behavior
```

The DDS, Rice-filename, and asynchronous-streaming inventory claims are closed
by 10 consecutive clean live Metal runs against clean pinned RT64. Each run
produced the same base, replacement, mip, Rice, held-fallback, and completed
Stream digests, with one completed Stream load and the same three-worker
resolved-not-installed transition.

## FFI boundary

`ffi/fn64_rt64_shim.cpp` exposes an opaque context through C functions to
create, process an `OSTask` or bounded raw-RDP range, present, resize, and
destroy. Create supplies
context-owned DPC and VI storage to `RT64::Application::Core`. Each task
synchronizes that context's DMEM/IMEM with fn64's persistent device-fabric
banks, temporarily points RT64 at fn64's stable 8 MiB RDRAM allocation, loads
the task's graphics microcode, and submits its raw display list. Changed RSP
banks are copied back before the Rust borrow returns. RT64's
render-to-RAM mode writes the native framebuffer into the same allocation;
the shim waits for the submitted workload before returning the Rust borrow.
The macOS surface is only an initialization dependency of RT64/plume's current
`Application::setup` path; normal framebuffer delivery remains render-to-RAM
and the existing fn64 VI capture. The opt-in validation readback below does not
replace that production path.

Presentation passes fn64's vblank-latched VI state across the same C boundary.
Black disables the shim's VI pixel type for that scanout; RepeatLine supplies
zero vertical scale; Fade supplies zero scale with its public 10-bit factor as
the VI vertical subpixel offset. RT64 therefore executes those operations with
its normal VI path instead of rejecting them or rewriting the RDP framebuffer.

The shim also exposes a context-free adapter capture. It copies all 14
`OSTask` words as received by C++ and derives the complete 24-word private
register block with the same function used by live presentation. The Rust
wrapper returns that state as `Rt64AdapterCapture` with a versioned canonical
SHA-256, so `tests/rt64_adapter_capture.rs` proves task/VI scalar marshalling
without opening SDL or a graphics device. This test still requires the `rt64`
feature and pinned MIT checkout because the quarantined shim is linked as one
unit:

```sh
cargo test -p fn64-render-rt64 --features rt64 --test rt64_adapter_capture
```

`crates/fn64-render-rt64/examples/rt64_pixel_differential.rs` is the explicit
device gate. On the process main thread it submits a synthetic public raw-RDP
fill twice to fresh RT64 contexts. The first verifies empty/default create and
then loads a temporary empty replacement database live; the second stages the
same database before create. Both exercise live enable/disable and a
content-changing reload, wait for each render-to-RAM workload, present typed
VI state, and require the exact RGBA16 target bytes to equal both runs and the
Rust reference backend. The gate also enables the opt-in render-present hook,
inserts a swapchain-texture-to-readback-buffer copy after RT64's VI renderer,
and compares the two fresh contexts' tightly packed post-VI bytes,
dimensions, format, and present identifier before printing both digests:

```sh
cargo run -p fn64-render-rt64 --features rt64 --example rt64_pixel_differential
```

That command is honestly GPU/display-backed. The pinned MIT RT64 render hook's
backend framebuffer exposes the concrete Plume color attachment. The shim
accepts only one BGRA8 or RGBA8 UNORM attachment with nonzero dimensions,
allocates a backend readback buffer with the required row-pitch alignment, and
encodes a Metal blit, Vulkan image-to-buffer copy with explicit layout restore,
or D3D12 placed-footprint copy with its 256-byte pitch. It exposes tightly
packed rows and preserves the exact BGRA/RGBA format tag; release capture alone
normalizes RGBA to its canonical BGRA envelope. The matching completed RT64
present records both present and workload IDs, and present-ID completion follows
the copy's command fence before CPU mapping. Unknown backends, formats, missing
attachments, inconsistent dimensions/pitches/provenance, allocation/map
failures, and a completed present without a capture all fail loudly.

The capture is after RT64's VI shader and before inspector drawing, the PRESENT
transition, the compositor, host color management, and the physical display.
These bytes are not post-analog N64 VI output and do not establish silicon
gamma/divot/dither equivalence. Metal retains its measured 20-run control;
macOS compiles the Vulkan branch and static tests retain the D3D12 seam, but no
Linux/Vulkan or Windows/Vulkan/D3D12 actual-hardware result is claimed.

RT64's paused debugger replays already completed workloads for inspection.
Pinned RT64 otherwise bypasses both HLE and raw display-list parsing while
paused and fabricates a DP interrupt, so the fn64 adapter rejects new command
submission in that state. Presentation and debugger selection remain live;
command execution resumes only after the debugger is unpaused.

The focused native VI gate is also directly runnable:

```sh
cargo run -p fn64-certification --features rt64 --example rt64_vi_filter_behavior
```

It requires a clean pinned source identity and names the unsupported native
filter stages in its terminal evidence line. A passing process therefore means
the recorded implementation boundary is stable, not that the pixel-inert
controls have been implemented.

For fixed-cycle reports, `RenderBackend::release_environment()` records the
concrete graphics API observed from the completed capture framebuffer and
command-list types, plus an API-specific RT64 identity. `Automatic` therefore
resolves from the backend that actually produced the image; a missing or
unknown capture backend and any explicit-request mismatch fail closed. The
identity records the selected source tree as a clean/dirty Git revision or an
explicitly declared source ID, the concrete post-VI API, and `adapter_sha256`:
a canonical digest over the fn64-owned `Cargo.toml`, `build.rs`, every Rust
adapter source, the shared `fn64-render` manifest and Rust sources, the CMake
wrapper/shim sources, the compilation target, and the sorted enabled feature
set. Cargo reruns the identity step when any covered source changes. The
no-argument `Rt64Backend::release_identity()` is retained
for non-release behavior examples and remains intentionally ambiguous on
Windows; report generation never accepts that ambiguous identity.
`LiveRenderEvidence` from `fn64-boot-harness`
canonically encodes that identity with guest cycle, capture stage, dimensions,
tight row size, BGRA8 format, the nonzero completed RT64 workload ID, present
ID, the canonical composite SHA-256 of
the user, enhancement, emulator, and ordered content/database-identified
replacement policies actually active for that image, and bytes. Pending
recreate settings or failed/pending replacement loads never replace the active
hash. A private host calls
`LiveReleaseGateRenderExt::capture_and_write_render_evidence`; the encoded
envelope becomes the existing framebuffer artifact, so both the artifact root
and top-level report SHA bind every field while retained JSON still contains no
private pixels. The synthetic example prints this envelope's SHA at cycle zero
as plumbing evidence only. It is not a fixed-cycle private-ROM release report.
The workload ID is process-local queue identity, not a cross-process content
digest; it prevents a completed image from being certified as a different
submitted workload within the captured run.

The registered backend remains owned behind `dyn RenderBackend`, so the live
OoT host retrieves it through the backend-neutral `release_capture` seam rather
than downcasting or retaining an RT64 alias. RT64 records
`ViPresentation::noise_seed`, which integrated execution sets to the exact VI
retrace guest cycle. Every field is presented exactly once, including repeated
progressive register images, and RT64 retains that cycle only after `present`
succeeds. The host requires the retained cycle to equal the fixed-cycle gate
before it accepts the pixels. An arbitrary cycle between presents therefore
fails loudly instead of relabeling the prior swapchain image. When an RT64
release gate is armed, capture setup or device creation failure is fatal and
cannot select the reference fallback.

Integrated presentation now crosses the native adapter with one complete
fourteen-word VI snapshot rather than reconstructing geometry from the host
window. RT64 receives the selected field's origin, source width, timing,
H/V active window, and X/Y scale together; its origin compensation uses the
retained register pixel type and effective 12-bit source stride, with one row
for ordinary fields and RT64's two-row convention for odd serrated fields. The
context preserves
that image across the no-argument VI refresh performed by later HLE/raw tasks
and resize, so PresentEarly or buffered work cannot silently revert to the
compatibility 108..748/identity-scale image. A live zero H/V window remains
inactive instead of selecting compatibility geometry. Backend-only examples
still use that compatibility image explicitly. The no-device adapter test
binds both the initial and no-argument-reapplied 24-word images; source review
separately verifies that HLE/raw/resize paths invoke that reapplication.
Hardware-exact border, field phase, and analog output remain outside this
claim.

A clean Git source is the only automatically authoritative identity. A dirty
checkout remains visible as `git-dirty`; `FN64_RT64_SOURCE_ID` is visible as
`declared` provenance and is not release-matrix admissible. The release-matrix
gate requires a lowercase adapter SHA-256, canonical `git:<40 lowercase hex>`
source, clean provenance, a nonempty overlay, and the post-VI API assigned to
the declared host platform. The adapter identity therefore cannot describe
modified fn64 Rust/C++ capture code as the same build, or an exact-source-
patched RT64 tree as the bare upstream commit. This is source/build-shape
provenance, not binary reproducibility or attestation of the compiler and
physical process.

The wrapper CMake build applies an exact-source-checked RT64 raster-shader
worker overlay on every backend. Pinned RT64 launched each idle-priority worker
before publishing its running predicate, and the worker published `true` again
on entry. If teardown stored `false` and notified before a delayed worker
entered, that worker could overwrite the stop predicate and sleep after the
only notification while its destructor waited in `join`. Overlay revision
`fn64:raster-shader-start-stop:v1` publishes `true` before launch and makes the
destructor the only post-launch writer. CMake rejects an upstream source shape
that no longer matches the reviewed patch context.

The composite source overlay also carries `vi-region-rate:v1`. Pinned RT64's
stable-factor workload inference divides a hardcoded 60 Hz base and labels PAL
workloads as 60/factor. `RenderConfig` now requires callers either to name an
NTSC fixture explicitly with `RenderConfig::ntsc` or to carry the IPL-selected
`TvType` through `RenderConfig::for_tv`. The C ABI registers each RT64
`VIHistory` with that context's 50/60 Hz nominal field rate; the exact-source
overlay replaces only the hardcoded base lookup, retaining RT64's factor
history and the later Extended-GBI `SetRefreshRate` override. A device-free C++
probe calls the patched `VIHistory::logicalRateFromFactors()` and requires
NTSC/MPAL factor 1/2 to produce 60/30 and PAL factor 1/2 to produce 50/25.
Ten fresh live Metal processes separately carry PAL and MPAL through
`RenderConfig::for_tv`, production context creation, ordinary VI events, and
FullSync without an Extended refresh override. Each run stabilizes the exact
completed-workload sequence at PAL `[0, 0, 0, 50]` and MPAL
`[0, 0, 0, 60]`, while a zero-identity production task remains `NeedsLle`.
Rust's unique mutable `Context` ownership prevents destruction from overlapping
the display-list caller that performs logical-rate lookup. Teardown drains the
RT64 workers and removes the per-context entry before RT64 destroys its state;
the registry mutex isolates simultaneous contexts without a process-global
region. This is workload-rate behavior; it does not claim physical compositor
cadence or analog PAL output. Report schema v20 now binds normalized ROM TV
region, committed device TV state, and this renderer's retained create-time TV
configuration. Representative private PAL/MPAL exact-ten evidence remains to
be retained.

The wrapper CMake build also applies an exact-source-checked Metal ownership fix
to plume: several convenience-factory results (the command buffer, persistent
encoders, a formatted-buffer texture descriptor, and stored shader names) had
no retained ownership despite later manual releases. Without the balancing
retains, shutdown joined the workload thread while its implicit autorelease pool
released already-deallocated Metal objects, crashing in `objc_release` after an
otherwise successful render.

`src/extended_gbi.rs` is the pure-Rust, F3DEX2-specific game-cooperation wire
surface. Runtime policy may disable cooperation, probe and use v1 when
available, or require v1. The zero-initialized `GetVersion` result is resolved
only after its task completes; unknown versions and missing required support
are named errors. The typed v1 encoder exposes only pinned RT64's default
`0x64` opcode and the bounded commands exercised by the private six-case
fixture and public non-ROM closure gates. This keeps cooperation runtime
optional without making the ReferenceBackend Extended-aware or broadening
production microcode recognition.

The non-default `extended-gbi-evidence` feature adds a hand-authored, non-ROM
live gate over the shared synthetic F3DEX2 transport. It proves real RT64
GetVersion, Enable, SetRectAspect, and SetRectAlign dispatch; ordinary optional
fallback; the named required-policy failure; exact typed rectangle bounds; and
the full RGBA16 target-footprint guard. Normal `process_task` recognition is a
required negative control and remains unchanged. Ten fresh watchdog-bounded
Metal processes produced identical typed bounds and ordinary/adjusted RGBA16
SHA-256 evidence. A second ten-process gate adds exact Extended refresh and
matrix-group interpolation provenance/pixels plus typed visible/occluded
Vertex-Z behavior over guarded public F3DEX2 scenes. Together with the public
aspect matrix, these close the Extended command-set behavior without claiming
a synthetic production hash. The HFR-first overlap regression passed its
separate 20-process concurrency bar. See
[`RT64-EXTENDED-GBI-SEAM.md`](../../docs/RT64-EXTENDED-GBI-SEAM.md).

The same opt-in transport also carries a public HLE aspect matrix with a
transformed triangle, explicit viewport/non-full scissor, and an Extended
Adjust rectangle with independent Left/Right origins. Ten fresh clean Metal
processes pass exact post-VI shapes and SHA-256 values at 4:3, 16:9, 2:1, and
21:9 through live framebuffer-discard transitions. Different 3D and 2D
geometry responses reject global stretch, closing the narrower widescreen and
ultrawide inventory rows while production hash recognition remains unchanged.

The HFR fixture now uses that honest HLE-dialect admission for cooperating-game
evidence rather than relying on raw DPC or an untyped synthetic shortcut. It
negotiates Extended v1, emits typed Enable, 60 Hz source-rate, and explicit
Auto-order decomposed transform-group commands, then live-switches Original to
Manual 120 Hz. Twenty fresh watchdog-bounded Metal processes bind workload
3-to-4, present 3, generated fractions 1/2 and 2/2, stable spatial midpoint and
exact endpoint pixels, toggle-back restoration, and eight two-call pacing
bursts per process. Production `process_task` remains `NeedsLle` before and
after. This closes the HFR renderer-API behavior row; compositor/physical
scanout timing remains a separately named platform-certification residual.

`src/ffi.rs` is the only raw Rust FFI surface. It wraps the opaque pointer in a
safe, uniquely owned `Context`, documents each unsafe call, and maps every
recoverable C++ failure to a Rust `Result`.

The `oot-boot` harness selects the implementation with
`FN64_RENDER=reference` (default) or `FN64_RENDER=rt64`. Requesting RT64 also
enables the Cargo feature in its `oot` helper script. If creation fails, the
harness normally logs the exact reason and continues with `ReferenceBackend`;
an armed RT64 release gate instead fails because fallback evidence is not RT64
evidence.
