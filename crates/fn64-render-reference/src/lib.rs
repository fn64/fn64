//! Deterministic pure-Rust renderer used as fn64's headless comparison oracle.
//!
//! This crate owns software GBI/RDP/VI behavior. Backend-neutral task
//! admission and presentation contracts live in `fn64-render`; native RT64
//! interop lives in `fn64-render-rt64`.

#![forbid(unsafe_code)]

mod backend;
pub mod depth;
pub mod gbi;
pub mod png_dump;
pub mod raster;
mod s2dex;
mod vi;

pub use backend::{DecodeMode, ReferenceBackend};
pub use fn64_render::{GeometryWireFamily, S2dexWireFamily};

/// Read a generic `FN64_*` observability knob while keeping its retired
/// game-specific spelling loud.
#[cfg(not(test))]
pub(crate) fn debug_flag(name: &str) -> bool {
    let legacy = format!("OOT_{}", name.strip_prefix("FN64_").unwrap_or(name));
    assert!(
        std::env::var_os(&legacy).is_none(),
        "{legacy} was renamed to {name}; it is no longer read. Re-run with {name} set."
    );
    std::env::var_os(name).is_some()
}

pub(crate) fn record_render_unsupported(
    operation: &'static str,
    context: &str,
    disposition: fn64_runtime::UnsupportedDisposition,
) {
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Render,
        operation,
        context,
        None,
        disposition,
    );
}

pub(crate) fn render_unsupported_error(
    backend: &'static str,
    operation: &'static str,
    context: impl Into<String>,
) -> fn64_render::RenderError {
    let context = context.into();
    record_render_unsupported(
        operation,
        &context,
        fn64_runtime::UnsupportedDisposition::ReturnedError,
    );
    fn64_render::RenderError::Backend {
        backend,
        reason: context,
    }
}

pub(crate) fn render_unsupported_panic(operation: &'static str, context: impl Into<String>) -> ! {
    let context = context.into();
    record_render_unsupported(
        operation,
        &context,
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!("{context}")
}
