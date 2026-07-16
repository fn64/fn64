# fn64-render-rt64

This crate contains both fn64 graphics backends:

- `ReferenceBackend` is the deterministic, pure-Rust software rasterizer used
  by the default build and headless CI.
- `Rt64Backend` is an opt-in C ABI wrapper around RT64's MIT C++ render/HLE
  library. It is enabled with the crate's `rt64` Cargo feature.

The C++ and every Rust `unsafe` block used to call it are quarantined here, as
required by `docs/DESIGN.md` section 1. `fn64-render`, `fn64-runtime`, and the
native recompiler remain unaware of RT64 types and continue to forbid unsafe
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
private DMEM, IMEM, DPC, and VI storage to `RT64::Application::Core`. Task
processing temporarily points RT64 at fn64's stable 8 MiB RDRAM allocation,
loads the task's graphics microcode, and submits its raw display list. RT64's
render-to-RAM mode writes the native framebuffer into the same allocation;
the shim waits for the submitted workload before returning the Rust borrow.
The macOS surface is only an initialization dependency of RT64/plume's current
`Application::setup` path; framebuffer delivery remains render-to-RAM and the
existing fn64 VI capture, not swapchain readback.

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
