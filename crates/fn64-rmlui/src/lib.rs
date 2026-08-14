//! Rust binding for RmlUi, scoped to fn64's own settings-menu needs.
//!
//! All RmlUi/plume C++ state stays behind the opaque handles declared in
//! `ffi/fn64_rmlui_shim.h`, mirroring `fn64-render-rt64`'s own convention of
//! quarantining C++ interop to one crate rather than letting it leak into
//! the rest of the workspace.
//!
//! The `rmlui` feature gates the native build; without it this crate
//! compiles to nothing callable, matching `fn64-render-rt64`'s own
//! default-off `rt64` feature so CI/no-GPU hosts stay pure Rust.
