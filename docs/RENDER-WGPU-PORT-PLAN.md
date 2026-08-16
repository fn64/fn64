# fn64-render-wgpu: porting RT64 to Rust (done better)

The plan for `WgpuBackend` — fn64's pure-Rust, wasm-capable RDP renderer,
**ported from RT64 (MIT), not reimplemented from specs**. Sibling to
`fn64-render-rt64` (the C++ FFI wrapper), both behind the `fn64_render::
RenderBackend` trait. **Triggered when the RT64 wrapper lands + a faithful
frame is eye-verified** (the wrapper is the port's differential oracle).

Crate name: **`fn64-render-wgpu`** (parallel to `fn64-render-rt64`, exactly
as `fn64-cpu-runtime` parallels the N64Recomp adapter).

See `RT64-GAP-REGISTER.md` for the cited gap list. The wrap-then-port decision
shipped (2026-07-16): wrap RT64 first for a faithful render now, port it to
Rust later behind the same `RenderBackend` seam — the recompiler story retold
(wrap N64Recomp, prove the runtime, then replace it in typed Rust with the
wrapper as oracle). `DESIGN.md` §1 owns the license boundary that port inherits.

## Guiding principle: oracle-faithful BEHAVIOR, idiomatic Rust STRUCTURE
Same model that made `fn64-cpu-runtime` work: the *behavior* is bit-exact
(differential-gated against C++ RT64, per module); the *code* is idiomatic
typed Rust. Port the **what** (RT64's hard-won accuracy algorithms) faithfully;
modernize the **how** (types, memory model, GPU API). Do NOT re-derive the
accuracy logic from specs — RT64 already solved those edge cases; that's the
whole reason to port rather than rebuild.

## Per-layer strategy (RT64 is NOT one thing)
1. **`shared/` state (blender, color_combiner, other_mode, gpu_tile) — PORT
   faithfully, structure idiomatically.** C bitfields → typed Rust enums/
   newtypes (kills the reinterpret bug class). We ALREADY did the combiner this
   way in `ReferenceBackend`; that's the seed. "Better" here = type safety, zero
   behavior change. Bit-exact vs RT64.
2. **`hle/` + `rdp`/`rsp`/`tmem` (the RDP pixel pipeline) — PORT the algorithm,
   idiomatic Rust memory/error model.** The crown jewels (framebuffer reinterpret,
   tile decode, fill-rule edges). Read RT64's algorithm, understand *why* it's
   right, express in idiomatic Rust (borrow-checked framebuffer lifetimes,
   `Result` not error codes, fewer allocs). Algorithm = RT64's. Differential-
   gated to bit-exact. **Fix the gap-register bugs here (don't copy them).**
3. **`rhi`/GPU backend (plume, Vulkan/D3D12/Metal, shaders) — REWRITE on
   `wgpu`, don't port.** wgpu gives Vulkan/Metal/D3D12/**WebGPU-in-browser** in
   one safe-Rust API. Porting plume would redo what wgpu does. HLSL shaders →
   WGSL (port the logic). This layer is rewrite-with-reference. Heed PR #246:
   don't assume `VK_EXT_scalar_block_layout` — design buffer layouts wgpu-safe.

## Scope: CORE-faithful (port) vs ENHANCEMENT (skip) — per RT64-GAP-REGISTER §D
- **PORT:** `gbi/*` (F3DEX2/F3DZEX2 decode — OoT's ucode), `rsp` (transform/
  lighting/clip), `rdp` + `rdp_tmem` (state/tiles/TMEM), core draw path,
  `shared/` blender+CC+othermode, raster PS logic (RasterPS.hlsl/TextureSampler
  → WGSL), framebuffer manager + native read/writeback, `vi` scan-out.
- **SKIP:** ray tracing, upscaling/widescreen, frame interpolation
  (`game_frame.cpp`/`rigid_body.cpp`), extended GBI, texture replacement,
  librashader, all D3D12/Vulkan/Metal/driver-workaround machinery. ~half of
  RT64's surface — not needed for faithful N64 output.

## Close these gaps IN the port (from RT64-GAP-REGISTER §E — RT64's own bugs, fix don't copy)
Prioritized for faithful OoT (F3DZEX2, 3D):
1. **F3DEX2 loaducode state reset** (#217)
2. **2-cycle combiner / RDP register timing + shade path** (#200, #235)
3. **Tile-index mod-8 wrap + the `RDPTiles[8]` OOB read** (#116)
4. **decal / Prim-Z / near-clip-at-0 depth** (#150, #103, #203)
5. **full blender-requirement logic + framebuffer reinterpretation cases**
Plus: TEXGEN texcoord tracking + look-at matching (currently stubbed `true &&
true`), TLUT copy/bilerp (#189), flat-shading path (#183). Each: port RT64's
structure, apply the fix, differential-test — the fix should make our output
MORE correct than upstream RT64, verified against the emulator (not against
RT64's buggy output for those specific cases).

## The verification spine (what makes it safe — same as the recompiler)
The **C++ RT64 wrapper (`Rt64Backend`) is the differential oracle.** Per ported
module: render the same DL through both `Rt64Backend` and `fn64-render-wgpu`,
diff the framebuffer, require bit-identical — EXCEPT the gap-register cases,
where our (fixed) output should match the EMULATOR/hardware, not RT64's buggy
result. No module lands until it passes. `ReferenceBackend` stays the pure-Rust
CI oracle throughout. This is why the wrapper lands first.

## First module to port (when triggered)
The **color combiner** — smallest, pure logic (no GPU), and `ReferenceBackend`
already has a hand-port from `rt64_color_combiner.h` to grow from. Proves the
differential-oracle harness end-to-end on an easy target, then tile/TMEM decode
(carrying fix #116), then the blender (#200), then the raster/framebuffer path
(the wgpu boundary). Grow `fn64-render-wgpu` module by module, each gated.
