//! Pure-Rust wgpu renderer ownership for fn64.
//!
//! M3.2 adds a bounded raw-DPC decoder and transaction-local typed RDP state to
//! the M3.1 headless fill/FullSync mechanism. It is not broad raster, TMEM,
//! VI, presentation, RT64 parity, or a performance claim.
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
//!
//! Transaction-local RDP state is likewise consumed by an explicit successor
//! decode and cannot be reused for another packet:
//!
//! ```compile_fail
//! use fn64_render_ir::SubmittedTicket;
//! use fn64_render_wgpu::{decode_raw_dpc_after, DecodedRawDpc};
//! # fn decoded() -> DecodedRawDpc { unimplemented!() }
//! # fn submitted() -> SubmittedTicket { unimplemented!() }
//! let staged = decoded().into_staged_state();
//! let next = decode_raw_dpc_after(submitted(), staged);
//! let stale = decode_raw_dpc_after(submitted(), staged);
//! # drop((next, stale));
//! ```
//!
//! A submitted ticket itself is also consumed by decode, so one submission
//! cannot mint two independent staged-state results:
//!
//! ```compile_fail
//! use fn64_render_ir::SubmittedTicket;
//! use fn64_render_wgpu::{decode_raw_dpc, RdpState};
//! # fn submitted() -> SubmittedTicket { unimplemented!() }
//! let submitted = submitted();
//! let first = decode_raw_dpc(submitted, &RdpState::default());
//! let duplicate = decode_raw_dpc(submitted, &RdpState::default());
//! # drop((first, duplicate));
//! ```
#![forbid(unsafe_code)]

mod device;
mod lifecycle;
mod raw_dpc;
mod state;

pub use device::{
    HeadlessBackend, HeadlessDeviceOutcome, InFlightFill, NoAdapter, PrewarmedRenderer,
    UninitializedRenderer,
};
pub use lifecycle::{
    NativeCompletionIdentity, StagedWgpuEffect, WgpuBackendCompletion, WgpuRenderError,
    FILL_FIXTURE_BYTES, FILL_FIXTURE_HEIGHT, FILL_FIXTURE_TEST_COLOR, FILL_FIXTURE_TEST_OUTPUT,
    FILL_FIXTURE_WIDTH,
};
pub use raw_dpc::{
    decode_raw_dpc, decode_raw_dpc_after, DecodedRawDpc, DecodedRawDpcCommand, FillRectangle,
    RawDpcCommandKind, RawDpcCommandLocation, RawDpcDecodeError, RawDpcResourcePlan,
};
pub use state::{
    ColorImage, CycleType, FillColor, ImageFormat, OtherMode, PixelSize, RdpState, RdpStateDelta,
    StagedRdpState,
};
