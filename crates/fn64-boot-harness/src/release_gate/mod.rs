//! Deterministic end-to-end evidence for a boot host.
//!
//! This module does not decide which game paths matter. A host declares those
//! paths, records whether each ran, and captures all output channels at one
//! exact guest cycle. That keeps a missing observation distinct from a proved
//! zero and prevents a shorter boot from masquerading as release closure.




pub(crate) const REPORT_SCHEMA: &str = "fn64.release-gate.v34";

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use fn64_runtime::{
    ControllerOperationDevice, ControllerOperationEvent, DeviceEvidenceSnapshot, DeviceSnapshot,
    DeviceTraceEvent, DeviceTraceKind, DmaDirection, GameBoyMapperEvidenceSnapshot, PendingViFade,
    PiDeviceAddress, PortState, QueueOpKind, RdramAddr, SaveOperationEvent, SaveType,
    ScheduledDeviceEventKind, SiDmaKind, SpDmaDirection, SwitchReason, TaskKind, TraceEvent,
    TraceKind, UnsupportedEvent as RuntimeUnsupportedEvent,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use crate::{
    FramebufferObservationSource, ObservationEvidenceError, ReleaseAudioTaskExecutionPolicy,
    ReleaseCartridgeSave, ReleaseControllerPort, ReleaseEnvironmentEvidence, ReleaseGraphicsApi,
    ReleaseGraphicsExecutionPolicy, ReleaseHostPlatform, ReleaseObservationGeometry,
    ReleaseRendererEvidence, ReleaseWindowsFamily, ReleaseWindowsProductType,
    ReleaseWindowsVersionEvidence,
};

mod rom;
mod evidence;
mod live_gate;
mod report;
mod encode;
mod publication;

pub use rom::*;
pub use evidence::*;
pub use live_gate::*;
pub use report::*;
pub use encode::*;
// publication's pub items all sit behind the recomp-rs feature; without it
// the glob would re-export nothing public and rustc warns. The siblings and
// tests still need its pub(super) items either way.
#[cfg(feature = "recomp-rs")]
pub use publication::*;
#[cfg(not(feature = "recomp-rs"))]
pub(crate) use publication::*;

#[cfg(test)]
fn test_release_environment(
    observations: &ReleaseObservationGeometry,
) -> ReleaseEnvironmentEvidence {
    let renderer = match &observations.framebuffer.source {
        FramebufferObservationSource::PhysicalRdram { .. } => ReleaseRendererEvidence::Reference {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
            tv_type: ReleaseTvStandard::Ntsc,
        },
        FramebufferObservationSource::PostViSwapchain {
            backend_identity,
            settings_sha256,
            ..
        } => ReleaseRendererEvidence::Rt64 {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
            tv_type: ReleaseTvStandard::Ntsc,
            graphics_api: match super::release_host_platform()
                .expect("test platform is release-supported")
            {
                ReleaseHostPlatform::MacosArm64 => ReleaseGraphicsApi::Metal,
                ReleaseHostPlatform::LinuxX86_64 => ReleaseGraphicsApi::Vulkan,
                ReleaseHostPlatform::WindowsX86_64 => ReleaseGraphicsApi::D3d12,
            },
            backend_identity: backend_identity.clone(),
            source_authoritative: true,
            settings_sha256: settings_sha256.clone(),
            replacement_packs_active: false,
        },
    };
    ReleaseEnvironmentEvidence {
        platform: super::release_host_platform().expect("test platform is release-supported"),
        windows_version: super::test_release_windows_version(),
        controller_ports: [
            ReleaseControllerPort::StandardControllerNoPak,
            ReleaseControllerPort::Absent,
            ReleaseControllerPort::Absent,
            ReleaseControllerPort::Absent,
        ],
        cartridge_save: ReleaseCartridgeSave::NoCartridgeSave,
        audio_task_execution: ReleaseAudioTaskExecutionPolicy::LleAccuracy,
        renderer,
    }
}

#[cfg(test)]
fn test_rsp_rdp_evidence(
    guest_cycle: u64,
    closure: &[ClosurePath],
) -> Result<RspRdpEvidence, GateError> {
    let graphics_exercised = closure.iter().any(|path| {
        path.name == "rsp.graphics-task"
            && matches!(
                path.status,
                ClosurePathStatus::ExercisedZeroUnsupported
                    | ClosurePathStatus::ExercisedUnsupported
            )
    });
    let ordered = if graphics_exercised {
        vec![RspRdpObservationEventEvidence {
            guest_cycle,
            observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address: 0,
                imem_generation: 0,
                text_sha256: sha256_hex(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]),
                data_address: 0,
                data_bytes: 1,
                data_sha256: sha256_hex(&[0]),
                family: None,
            },
        }]
    } else {
        Vec::new()
    };
    RspRdpEvidence::from_ordered(ordered)
}


#[cfg(test)]
mod tests;
