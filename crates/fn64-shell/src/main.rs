//! fn64-shell: the executable. See `docs/DESIGN.md` section 1 and section
//! 5's wave 5. Placeholder entry point -- window/input/audio-out backend
//! selection and ROM/RecompiledFuncs intake are not yet implemented; this
//! exists so the workspace has a runnable binary target and the dependency
//! graph (shell -> abi, runtime, rt64) is real and building, not aspirational.

fn main() {
    println!("fn64-shell: pre-alpha scaffold, no runnable game intake yet.");
    println!("See docs/DESIGN.md section 5 (work packages) for the wave that lands this.");
}
