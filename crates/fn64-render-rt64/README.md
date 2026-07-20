# fn64-render-rt64

This crate contains both fn64 graphics backends:

- `ReferenceBackend` is the deterministic, pure-Rust software rasterizer used
  by the default build and headless CI.
- `Rt64Backend` is an opt-in C ABI wrapper around RT64's MIT C++ render/HLE
  library. It is enabled with the crate's `rt64` Cargo feature.

Both HLE backends use content-addressed F3DEX2 admission. `with_f3dex2()` only
selects the reference decoder; it does not trust the task-entry IMEM image.
Task-entry and changed `G_LOAD_UCODE` images may enter HLE only when their exact
4 KiB SHA-256 was configured with `with_f3dex2_ucode_sha256` (or derived from a
synthetic fixture through `with_f3dex2_ucode_text`). Otherwise the backend
returns `FrameStatus::NeedsLle` without mutations and the runtime replays the
complete ucode phase from post-rspboot state through its general interpreter.
The reference preflight runs on cloned RDRAM/RSP state for admitted images that
self-load; the catalog stores identities only, never ucode or game bytes.

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

One/two-cycle reference rendering accepts RGB and alpha dither only when both
selectors are disabled. Nintendo 64 Programming Manual section 15.5 proves
that the memory interface adds three low dither bits before reducing RGB and
that alpha uses a related five-bit path, but it does not publish the ordered
tables or long-period noise generator. Active selectors therefore trap by
name instead of being silently ignored. The proven disabled path truncates
RGBA16 RGB and RGBA32 memory alpha with `>> 3`; copy and fill cycles retain
their documented blender-bypass behavior.

The RT64 adapter implements that fallback's raw-RDP half as well. Bounded DRAM
or staged XBUS ranges cross the C ABI through RT64's MIT public embedding entry
`Application::processDisplayLists(memory, start, end, false)`, wait for the
exact render-to-RAM workload, and retain the VI output selected by the rejected
task. The raw renderer call carries that VI output explicitly, so CPU-only DPC
streams do not depend on backend call history. Unknown microcode therefore
does not require RT64 to recognize its GBI.

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

## FFI boundary

`ffi/fn64_rt64_shim.cpp` exposes an opaque context through five C functions:
create, process an `OSTask`, present, resize, and destroy. Create supplies
context-owned DPC and VI storage to `RT64::Application::Core`. Each task
synchronizes that context's DMEM/IMEM with fn64's persistent device-fabric
banks, temporarily points RT64 at fn64's stable 8 MiB RDRAM allocation, loads
the task's graphics microcode, and submits its raw display list. Changed RSP
banks are copied back before the Rust borrow returns. RT64's
render-to-RAM mode writes the native framebuffer into the same allocation;
the shim waits for the submitted workload before returning the Rust borrow.
The macOS surface is only an initialization dependency of RT64/plume's current
`Application::setup` path; framebuffer delivery remains render-to-RAM and the
existing fn64 VI capture, not swapchain readback.

Presentation passes fn64's vblank-latched VI state across the same C boundary.
Black disables the shim's VI pixel type for that scanout; RepeatLine supplies
zero vertical scale; Fade supplies zero scale with its public 10-bit factor as
the VI vertical subpixel offset. RT64 therefore executes those operations with
its normal VI path instead of rejecting them or rewriting the RDP framebuffer.

The wrapper CMake build also applies an exact-source-checked Metal ownership fix
to plume: several convenience-factory results (the command buffer, persistent
encoders, a formatted-buffer texture descriptor, and stored shader names) had
no retained ownership despite later manual releases. Without the balancing
retains, shutdown joined the workload thread while its implicit autorelease pool
released already-deallocated Metal objects, crashing in `objc_release` after an
otherwise successful render.

`src/ffi.rs` is the only raw Rust FFI surface. It wraps the opaque pointer in a
safe, uniquely owned `Context`, documents each unsafe call, and maps every
recoverable C++ failure to a Rust `Result`.

The `oot-boot` harness selects the implementation with
`FN64_RENDER=reference` (default) or `FN64_RENDER=rt64`. Requesting RT64 also
enables the Cargo feature in its `oot` helper script. If creation fails, the
harness logs the exact reason and continues with `ReferenceBackend`.
