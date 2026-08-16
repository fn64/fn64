//! Pure-Rust wgpu renderer ownership for fn64.
//!
//! M3.1 deliberately exposes only a bounded headless fill/FullSync mechanism
//! fixture. It proves the move-only IR-to-exact-GPU-completion path; it is not
//! general RDP decode, VI, presentation, RT64 parity, or a performance claim.
//!
//! Submission and completion are type states. An in-flight operation has no
//! early receipt conversion:
//!
//! ```compile_fail
//! use fn64_render_wgpu::InFlightFill;
//! # fn in_flight() -> InFlightFill<'static> { unimplemented!() }
//! let in_flight = in_flight();
//! let completion = in_flight.into_completion();
//! # drop(completion);
//! ```
//!
//! Completed effects are move-only and cannot be published twice:
//!
//! ```compile_fail
//! use fn64_render_wgpu::WgpuBackendCompletion;
//! # fn completion() -> WgpuBackendCompletion { unimplemented!() }
//! let completion = completion();
//! let first = completion.into_parts();
//! let second = completion.into_parts();
//! # drop((first, second));
//! ```
#![forbid(unsafe_code)]

mod device;
mod lifecycle;

pub use device::{
    HeadlessBackend, HeadlessDeviceOutcome, InFlightFill, NoAdapter, PrewarmedRenderer,
    UninitializedRenderer,
};
pub use lifecycle::{
    NativeCompletionIdentity, StagedWgpuEffect, WgpuBackendCompletion, WgpuRenderError,
    FILL_FIXTURE_BYTES, FILL_FIXTURE_HEIGHT, FILL_FIXTURE_TEST_COLOR, FILL_FIXTURE_TEST_OUTPUT,
    FILL_FIXTURE_WIDTH,
};
