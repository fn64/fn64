//! Deterministic pure-Rust renderer used as fn64's headless comparison oracle.
//!
//! This crate owns software GBI/RDP/VI behavior. Backend-neutral task
//! admission and presentation contracts live in `fn64-render`; native RT64
//! interop lives in `fn64-render-rt64`.

#![forbid(unsafe_code)]

use std::cell::{Cell, RefCell};

mod backend;
pub mod depth;
pub mod gbi;
pub mod png_dump;
pub mod raster;
mod s2dex;
mod vi;

pub use backend::{DecodeMode, ReferenceBackend, ReferenceIrRawDpcAdapter};
pub use fn64_render::{GeometryWireFamily, S2dexWireFamily};

thread_local! {
    /// Process-global diagnostics are intentionally outside renderer state.
    /// A speculative IR execution therefore disables them at their source;
    /// rollback after recording or writing a file would already be too late.
    static SPECULATIVE_OBSERVATIONS_SUPPRESSED: Cell<bool> = const { Cell::new(false) };
    static SPECULATIVE_UNSUPPORTED_ATTEMPTS: RefCell<Vec<SpeculativeUnsupportedAttempt>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct SpeculativeUnsupportedAttempt {
    operation: &'static str,
    context: String,
    disposition: fn64_runtime::UnsupportedDisposition,
}

impl SpeculativeUnsupportedAttempt {
    pub(crate) fn rejection_reason(&self) -> String {
        format!(
            "speculative execution attempted forbidden observation {} ({}): {}",
            self.operation,
            self.disposition.as_str(),
            self.context
        )
    }
}

pub(crate) fn speculative_observations_suppressed() -> bool {
    SPECULATIVE_OBSERVATIONS_SUPPRESSED.get()
}

pub(crate) fn without_speculative_observations<T>(
    run: impl FnOnce() -> T,
) -> (T, Vec<SpeculativeUnsupportedAttempt>) {
    struct Restore;

    impl Drop for Restore {
        fn drop(&mut self) {
            SPECULATIVE_OBSERVATIONS_SUPPRESSED.set(false);
            SPECULATIVE_UNSUPPORTED_ATTEMPTS.with(|attempts| attempts.borrow_mut().clear());
        }
    }

    SPECULATIVE_OBSERVATIONS_SUPPRESSED.with(|suppressed| {
        assert!(
            !suppressed.replace(true),
            "nested speculative render observation suppression is unsupported"
        );
    });
    let restore = Restore;
    SPECULATIVE_UNSUPPORTED_ATTEMPTS.with(|attempts| {
        assert!(
            attempts.borrow().is_empty(),
            "stale speculative unsupported-attempt journal"
        );
    });
    let result = run();
    let attempts =
        SPECULATIVE_UNSUPPORTED_ATTEMPTS.with(|attempts| std::mem::take(&mut *attempts.borrow_mut()));
    drop(restore);
    (result, attempts)
}

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
    if speculative_observations_suppressed() {
        SPECULATIVE_UNSUPPORTED_ATTEMPTS.with(|attempts| {
            attempts.borrow_mut().push(SpeculativeUnsupportedAttempt {
                operation,
                context: context.to_owned(),
                disposition,
            });
        });
        return;
    }
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
