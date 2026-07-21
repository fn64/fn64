# fn64-render-reference

`fn64-render-reference` is fn64's deterministic, pure-Rust software rendering
oracle. It owns the bounded public GBI decoders, software RDP raster model,
hidden-bit/depth ownership, and digital VI reference path used for tests and
headless execution.

This crate is intentionally separate from `fn64-render-rt64`: applications may
select the reference backend without compiling C++, while an RT64 production
build no longer carries the software renderer. `fn64-render` remains the shared
backend-neutral contract and task-admission layer.

"Reference" means deterministic comparison oracle, not silicon-exact. Exact
and bounded behaviors are tracked in `docs/BASE-RENDERER-BEHAVIOR-MATRIX.md`;
unpublished hardware precision and full-ROM coverage remain explicit blockers.
