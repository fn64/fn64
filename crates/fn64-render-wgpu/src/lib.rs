//! Pure-Rust wgpu renderer ownership for fn64.
//!
//! M3.3c executes M3.3a's exact native 4x2 RGBA16 fill through a prewarmed
//! wgpu pipeline, M3.3b's typed target generations, and M3.3d's separately
//! typed bounded VI mechanism. The target and RDP state publish only after the
//! guest owner commits the exact backing-storage bytes. This remains one
//! synthetic mechanism: it is not a general raster/VI implementation, live VI
//! adapter, surface path, RT64 parity result, or performance claim.
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
//!
//! Preparation does not grant public GPU-completion authority. Backend modules
//! inside this crate must own that transition:
//!
//! ```compile_fail
//! use fn64_render_wgpu::PreparedNativeFill;
//! # fn prepared() -> PreparedNativeFill<'static> { unimplemented!() }
//! let in_flight = prepared().begin();
//! # drop(in_flight);
//! ```
//!
//! The exclusive durable-state borrow remains live until the candidate is
//! dropped or guest commit succeeds, so no competing early publication fits:
//!
//! ```compile_fail
//! use fn64_render_wgpu::{prepare_native_fill, DecodedRawDpc, NativeDurableState};
//! # fn decoded() -> DecodedRawDpc { unimplemented!() }
//! let mut durable = NativeDurableState::default();
//! let prepared = prepare_native_fill(decoded(), &mut durable).unwrap();
//! let early = durable.generation();
//! # drop((prepared, early));
//! ```
//!
//! Logical/device pixels and N64Recomp ABI backing-storage bytes are distinct
//! types, so the guest commit seam cannot accept unswizzled RGBA16 by accident:
//!
//! ```compile_fail
//! use fn64_render_wgpu::{DeviceRgba16Bytes, N64RecompRdramStorageBytes};
//! # fn device_pixels() -> DeviceRgba16Bytes { unimplemented!() }
//! # fn abi_storage() -> N64RecompRdramStorageBytes { unimplemented!() }
//! # fn assemble_native_output(_: DeviceRgba16Bytes, _: N64RecompRdramStorageBytes) {}
//! assemble_native_output(abi_storage(), device_pixels());
//! ```
//!
//! Native completion is move-only, so one GPU observation cannot publish two
//! target generations:
//!
//! ```compile_fail
//! use fn64_render_wgpu::{InFlightNativeRasterFill, NativeRasterError};
//! # fn in_flight() -> InFlightNativeRasterFill<'static, 'static> { unimplemented!() }
//! let in_flight = in_flight();
//! let first = in_flight.complete();
//! let second = in_flight.complete();
//! # let _: (Result<_, NativeRasterError>, Result<_, NativeRasterError>) = (first, second);
//! ```
#![forbid(unsafe_code)]

mod device;
mod lifecycle;
mod native_contract;
mod raw_dpc;
mod state;
mod targets;
mod vi;

pub use device::{
    HeadlessBackend, HeadlessDeviceOutcome, InFlightFill, NoAdapter, PrewarmedRenderer,
    UninitializedRenderer,
};
pub use lifecycle::{
    NativeCompletionIdentity, StagedWgpuEffect, WgpuBackendCompletion, WgpuRenderError,
    FILL_FIXTURE_BYTES, FILL_FIXTURE_HEIGHT, FILL_FIXTURE_TEST_COLOR, FILL_FIXTURE_TEST_OUTPUT,
    FILL_FIXTURE_WIDTH,
};
pub use native_contract::{
    prepare_native_fill, CommittedNativeFrame, DeviceRgba16Bytes, InFlightNativeFill,
    N64RecompRdramStorageBytes, NativeContractError, NativeDurableState, NativeFrameBinding,
    NativeGuestCommitError, NativeTargetIdentity, PendingNativeCommit, PreparedNativeFill,
    NATIVE_FILL_COMMAND_END, NATIVE_FILL_COMMAND_START, NATIVE_FILL_COMMAND_WORDS,
    NATIVE_FILL_DEVICE_RGBA16, NATIVE_FILL_FIXTURE_SCHEMA, NATIVE_FILL_HEIGHT,
    NATIVE_FILL_JOURNAL_SHA256, NATIVE_FILL_N64RECOMP_STORAGE_RGBA16,
    NATIVE_FILL_N64RECOMP_STORAGE_RGBA16_SHA256, NATIVE_FILL_NATIVE_RGBA8,
    NATIVE_FILL_NATIVE_RGBA8_SHA256, NATIVE_FILL_POST_VI_BGRA8, NATIVE_FILL_POST_VI_BGRA8_SHA256,
    NATIVE_FILL_RDRAM_BYTES, NATIVE_FILL_STREAM_SHA256, NATIVE_FILL_TARGET_END,
    NATIVE_FILL_TARGET_START, NATIVE_FILL_TRANSACTION_SEQUENCE, NATIVE_FILL_WIDTH,
    NATIVE_FILL_WORKLOAD_SHA256,
};
pub use raw_dpc::{
    decode_raw_dpc, decode_raw_dpc_after, DecodedRawDpc, DecodedRawDpcCommand, FillRectangle,
    RawDpcCommandKind, RawDpcCommandLocation, RawDpcDecodeError, RawDpcResourcePlan,
};
pub use state::{
    ColorImage, CycleType, FillColor, ImageFormat, OtherMode, PixelSize, RdpState, RdpStateDelta,
    StagedRdpState,
};
pub use targets::{
    pack_device_pixels, unpack_device_pixels, CandidateColorTarget, ColorTargetExtent,
    ColorTargetFormat, ColorTargetKey, ColorTargetRegistry, CommittedNativeRasterFrame,
    CompletedColorTargetWrite, DeviceColorBytes, ExactRowPlan, InFlightNativeRasterFill,
    InitializedCandidateColorTarget, InitializedRegionProof, NativeRasterDeviceOutcome,
    NativeRasterError, NativeRasterRenderer, PendingNativeRasterCommit, ResidentColorTarget, Rgba8,
    TargetError, TargetGeneration, TargetRectangle, TargetRowRange, TargetRows,
    UninitializedNativeRaster,
};
