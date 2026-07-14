//! fn64-rt64: empty placeholder. See `README.md` in this directory for why
//! this crate exists and is intentionally not implemented yet.

// Depended on so the workspace's real dependency graph (fn64-shell ->
// fn64-rt64 -> fn64-runtime, per docs/DESIGN.md section 1) is exercised by
// `cargo build`/`cargo test` even before any real RT64 bridging code lands.
#[allow(unused_imports)]
use fn64_runtime as _;
