//! fn64-shell: the executable. See `docs/DESIGN.md` section 1 and section
//! 5's wave 5. Placeholder entry point -- window/input/audio-out backend
//! selection and ROM/RecompiledFuncs intake are not yet implemented; this
//! exists so the workspace has a runnable binary target and the dependency
//! graph (shell -> abi, runtime, render, render-rt64) is real and building,
//! not aspirational.
//!
//! ## The render seam IS wired, real RT64 is not
//!
//! `fn64_abi::set_render_backend` is the door every recompiled game's gfx
//! tasks go through (see `fn64-abi`'s `GFX_RENDER_NOTE`/`GFX_TASK_NOTE`).
//! This entry point does not call it yet because there is no window/ROM
//! intake to drive a backend with (see above) -- registering a backend
//! with nothing to feed it would be decorative, not a real wiring. Once a
//! real ROM/RecompiledFuncs intake lands (this file's own TODO), the
//! choice here is exactly:
//! `fn64_abi::set_render_backend(Box::new(fn64_render_rt64::ReferenceBackend::new()), rdram_len)`
//! (headless, real triangles, no window) or
//! `Box::new(fn64_render_rt64::Rt64Backend::new())` (currently a named
//! stub -- see that struct's doc comment for the two concrete blockers).
//! Both already satisfy `fn64_render::RenderBackend`; swapping which one
//! this future call site constructs is the entire integration cost.

fn main() {
    println!("fn64-shell: pre-alpha scaffold, no runnable game intake yet.");
    println!("See docs/DESIGN.md section 5 (work packages) for the wave that lands this.");
    println!(
        "Render seam: fn64_abi::set_render_backend is wired to a real ReferenceBackend \
         (headless software rasterizer) or the stubbed Rt64Backend -- see main.rs module doc."
    );
}
