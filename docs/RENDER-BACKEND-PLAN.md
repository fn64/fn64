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
2. **`Rt64Backend`** (stub → being wired) — FFI wrapper over RT64's MIT C++
   render lib. `RT64::Application::Core` (rdram/DMEM/IMEM + DPC/VI regs) +
   `Interpreter::processDisplayLists(raw F3DEX2 DL)`. The fast faithful path.
   FFI quarantined to the `fn64-render-rt64` crate (the existing unsafe-audit
   boundary), MIT-only (do NOT enable the GPL-header mupen target).
3. **`NativeBackend`** (future) — a from-scratch clean-room Rust RDP renderer,
   the eventual all-Rust/wasm-capable replacement. Built later behind the same
   trait, differential-tested against RT64 (RT64 becomes ITS oracle, exactly
   as N64Recomp's C output was the oracle for fn64-recomp-native). This is the
   "own native plugin" half of the decision — deferred, not dropped.

## Sequencing
- NOW: wire `Rt64Backend` (RT64 FFI) → faithful OoT render, eyes-verified vs
  emulator. De-prioritize piecemeal RDP-opcode work in ReferenceBackend (it's
  re-deriving what RT64 already does) — but keep ReferenceBackend green as the
  oracle.
- LATER: `NativeBackend` clean-room Rust renderer, RT64-differential-gated.

This is the recompiler story retold: [[fn64-whole-rom-recomp-milestone]] wrapped
N64Recomp, proved the runtime, then fn64-recomp-native replaced it in typed
Rust. Same move for the renderer.
