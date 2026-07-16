# Render backend plan: wrap RT64 now, build our own native plugin later

Decision (2026-07-16): fn64 gets a **working RT64 wrapper now** for a fast
faithful render, **and** plans a **from-scratch Rust native renderer plugin**
later — the same two-step pattern that worked for the recompiler (wrap the
mature external tool first to unblock the runtime, then replace it with a
clean-room typed-Rust implementation behind the same seam).

See `RT64-WRAP-EVAL.md` for the full license/effort/gain analysis. Summary:
RT64 core is MIT (GPU stack all MIT/BSD/Apache/Zlib; the one GPL item is a
headers-only unbuilt mupen target — avoidable). Wrapping hands fn64 ~95% of a
faithful OoT frame for free (TMEM/combiner/blend/textures/framebuffer effects).

## The plugin seam (already exists — this is the plan of record, not a pivot)
`fn64_render::RenderBackend` trait (`crates/fn64-render/src/lib.rs:196`):
`create` / `process_task(rdram, task)` / `present`. Selected at one call site
via `fn64_abi::set_render_backend(Box::new(...))` (oot-boot main.rs:499).

Three implementations behind the one trait:
1. **`ReferenceBackend`** (exists) — pure-Rust software rasterizer. Was the
   bootstrap seam-proof; KEEP IT as the headless CI/test oracle + wasm/no-GPU
   fallback. All the render work built up this session (combiner/blend/alpha/
   textures/scissor) lives here and stays useful as the differential oracle.
2. **`Rt64Backend`** (feature-gated wrapper implemented) — FFI wrapper over RT64's MIT C++
   render lib. `RT64::Application::Core` (rdram/DMEM/IMEM + DPC/VI regs) +
   `Interpreter::processDisplayLists(raw F3DEX2 DL)`. The fast faithful path.
   FFI quarantined to the `fn64-render-rt64` crate (the existing unsafe-audit
   boundary), MIT-only (do NOT enable the GPL-header mupen target).
3. **`NativeBackend`** (future) — an all-Rust/wasm-capable RDP renderer, the
   eventual pure-Rust replacement. Approach (decided 2026-07-16): **PORT RT64
   to Rust, don't reimplement from scratch** — RT64 is MIT, so we can port its
   proven, accurate rendering logic directly (module by module) rather than
   re-deriving RDP behavior from specs the hard way. Port it **better**: typed
   state (no C++ reinterpret bugs), fewer allocations, cleaner GPU abstraction,
   wasm/WebGPU target. Differential-tested against the RT64 wrapper the whole
   way (the C++ RT64 becomes the oracle for its own Rust port — exactly as
   N64Recomp's C output was the oracle for fn64-recomp-native). This is the
   "own native plugin" half — **deferred, not dropped**; wrap first, port later.
   NOTE: this is the SAME two-backend model as everywhere else — `NativeBackend`
   is "the pure-Rust backend when it grows RDP-accurate," not a third separate
   thing. Today's `ReferenceBackend` (software rasterizer) is its seed + the CI
   oracle; the RT64 port is how it becomes faithful.

## Sequencing
- NOW: wire `Rt64Backend` (RT64 FFI) → faithful OoT render, eyes-verified vs
  emulator. De-prioritize piecemeal RDP-opcode work in ReferenceBackend (it's
  re-deriving what RT64 already does) — but keep ReferenceBackend green as the
  oracle.
- LATER: `NativeBackend` clean-room Rust renderer, RT64-differential-gated.

This is the recompiler story retold: [[fn64-whole-rom-recomp-milestone]] wrapped
N64Recomp, proved the runtime, then fn64-recomp-native replaced it in typed
Rust. Same move for the renderer.

## Implementation status (2026-07-16)

`fn64-render-rt64` now has an opt-in `rt64` Cargo feature. Its `build.rs`
configures a wrapper CMake project with `RT64_STATIC=ON` and builds only
`fn64_rt64_shim`; that target pulls in the static RT64 render/HLE target
`rt64` and the permissive `re-spirv`, `nfd`, `zstd`, and `plume` dependencies.
The evaluated RT64 source tree defines no mupen plugin target, so no source or
library from its GPL mupen64plus subtree is compiled or linked. The default
feature set remains the pure-Rust `ReferenceBackend` build and does not invoke
CMake.

The crate-local C shim owns `RT64::Application`, private DMEM/IMEM and DPC/VI
register storage, accepts fn64's RDRAM pointer plus the public `OSTask` field
shape, calls `loadUCodeGBI` and `Application::processDisplayLists`, and waits
for RT64's render-to-RAM workload before returning. `fn64-abi` invokes the
trait's `present` method at the real `osViSwapBuffer` boundary. All raw FFI and
unsafe Rust remain confined to `fn64-render-rt64`.

`examples/oot-boot` accepts `FN64_RENDER=rt64`; failure to initialize SDL or a
supported graphics device is a named create error and causes an explicit
fallback to `ReferenceBackend`. Frame verification and any host-specific GPU
blocker are recorded with the implementation commit rather than asserted by
this plan document.
