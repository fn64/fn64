//! Canonical live-render evidence carried by the fixed-cycle framebuffer artifact.
//!
//! The release report intentionally retains only artifact hashes. Encoding
//! renderer identity and typed capture metadata beside the pixels before they
//! enter `ArtifactKind::Framebuffer` makes the existing artifact root and
//! top-level report SHA bind those facts without serializing private pixels.

use std::{fmt, num::NonZeroU64, path::Path};

use sha2::{Digest, Sha256};

use crate::observation_evidence::RENDER_EVIDENCE_SCHEMA;
use crate::{
    release_gate::LiveObservedArtifacts, ArtifactKind, FramebufferObservationFormat,
    FramebufferObservationGeometry, FramebufferObservationSource, GateError, LiveMemoryEvidence,
    LiveReleaseGate, ReleaseGateReport, ReleaseHostPlatform, ReleaseObservationGeometry,
};

/// Position of the captured bytes in the renderer/presentation pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderCaptureStage {
    /// RT64's swapchain color attachment after its VI shader and before the
    /// platform compositor or physical display.
    PostViSwapchain,
}

impl RenderCaptureStage {
    const fn tag(self) -> u8 {
        match self {
            Self::PostViSwapchain => 1,
        }
    }
}

/// Exact byte layout of one canonical live capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderPixelFormat {
    Bgra8Unorm,
}

impl RenderPixelFormat {
    const fn tag(self) -> u8 {
        match self {
            Self::Bgra8Unorm => 1,
        }
    }

    const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Bgra8Unorm => 4,
        }
    }
}

/// Validated renderer identity, geometry, presentation identity, and pixels
/// sampled for a fixed-cycle report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRenderEvidence {
    guest_cycle: u64,
    backend_identity: String,
    settings_sha256: [u8; 32],
    stage: RenderCaptureStage,
    width: u32,
    height: u32,
    row_bytes: u32,
    format: RenderPixelFormat,
    workload_id: NonZeroU64,
    present_id: u64,
    bytes: Vec<u8>,
}

impl LiveRenderEvidence {
    #[allow(clippy::too_many_arguments)]
    pub fn post_vi_swapchain(
        guest_cycle: u64,
        backend_identity: impl Into<String>,
        settings_sha256: [u8; 32],
        width: u32,
        height: u32,
        row_bytes: u32,
        format: RenderPixelFormat,
        workload_id: u64,
        present_id: u64,
        bytes: Vec<u8>,
    ) -> Result<Self, RenderEvidenceError> {
        let backend_identity = backend_identity.into();
        if backend_identity.is_empty() {
            return Err(RenderEvidenceError::EmptyBackendIdentity);
        }
        if width == 0 || height == 0 {
            return Err(RenderEvidenceError::ZeroDimensions { width, height });
        }
        let workload_id =
            NonZeroU64::new(workload_id).ok_or(RenderEvidenceError::ZeroWorkloadId)?;
        if present_id == 0 {
            return Err(RenderEvidenceError::ZeroPresentId);
        }
        let expected_row_bytes = width
            .checked_mul(format.bytes_per_pixel())
            .ok_or(RenderEvidenceError::ByteLengthOverflow)?;
        if row_bytes != expected_row_bytes {
            return Err(RenderEvidenceError::NonCanonicalRowBytes {
                expected: expected_row_bytes,
                observed: row_bytes,
            });
        }
        let expected_len = usize::try_from(
            row_bytes
                .checked_mul(height)
                .ok_or(RenderEvidenceError::ByteLengthOverflow)?,
        )
        .map_err(|_| RenderEvidenceError::ByteLengthOverflow)?;
        if bytes.len() != expected_len {
            return Err(RenderEvidenceError::WrongByteLength {
                expected: expected_len,
                observed: bytes.len(),
            });
        }
        Ok(Self {
            guest_cycle,
            backend_identity,
            settings_sha256,
            stage: RenderCaptureStage::PostViSwapchain,
            width,
            height,
            row_bytes,
            format,
            workload_id,
            present_id,
            bytes,
        })
    }

    pub const fn guest_cycle(&self) -> u64 {
        self.guest_cycle
    }

    pub fn backend_identity(&self) -> &str {
        &self.backend_identity
    }

    pub const fn settings_sha256(&self) -> [u8; 32] {
        self.settings_sha256
    }

    pub const fn stage(&self) -> RenderCaptureStage {
        self.stage
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub const fn row_bytes(&self) -> u32 {
        self.row_bytes
    }

    pub const fn format(&self) -> RenderPixelFormat {
        self.format
    }

    pub const fn present_id(&self) -> u64 {
        self.present_id
    }

    pub const fn workload_id(&self) -> NonZeroU64 {
        self.workload_id
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Stable wire image hashed as the fixed-cycle framebuffer artifact.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            RENDER_EVIDENCE_SCHEMA.len() + self.backend_identity.len() + self.bytes.len() + 64,
        );
        out.extend_from_slice(RENDER_EVIDENCE_SCHEMA);
        push_u64(&mut out, self.guest_cycle);
        push_bytes(&mut out, self.backend_identity.as_bytes());
        out.extend_from_slice(&self.settings_sha256);
        out.push(self.stage.tag());
        push_u32(&mut out, self.width);
        push_u32(&mut out, self.height);
        push_u32(&mut out, self.row_bytes);
        out.push(self.format.tag());
        push_u64(&mut out, self.workload_id.get());
        push_u64(&mut out, self.present_id);
        push_bytes(&mut out, &self.bytes);
        out
    }

    pub fn sha256(&self) -> [u8; 32] {
        Sha256::digest(self.canonical_bytes()).into()
    }
}

/// Capture a typed render envelope through the existing fixed-cycle gate.
pub trait LiveReleaseGateRenderExt {
    fn capture_and_write_render_evidence(
        self,
        boundary: crate::CommittedViBoundary,
        scenario: impl Into<String>,
        input_bytes: &[u8],
        render: &LiveRenderEvidence,
        memory: &LiveMemoryEvidence,
        report_path: impl AsRef<Path>,
    ) -> Result<ReleaseGateReport, GateError>;
}

impl LiveReleaseGateRenderExt for LiveReleaseGate {
    fn capture_and_write_render_evidence(
        self,
        boundary: crate::CommittedViBoundary,
        scenario: impl Into<String>,
        input_bytes: &[u8],
        render: &LiveRenderEvidence,
        memory: &LiveMemoryEvidence,
        report_path: impl AsRef<Path>,
    ) -> Result<ReleaseGateReport, GateError> {
        if render.guest_cycle != self.guest_cycle() {
            return Err(GateError::WrongCycle {
                expected: self.guest_cycle(),
                observed: render.guest_cycle,
                kind: ArtifactKind::Framebuffer,
            });
        }
        let (width, height) = render.dimensions();
        let observations = ReleaseObservationGeometry {
            framebuffer: FramebufferObservationGeometry {
                source: FramebufferObservationSource::PostViSwapchain {
                    backend_identity: render.backend_identity().to_owned(),
                    settings_sha256: hex(&render.settings_sha256()),
                    workload_id: render.workload_id(),
                    present_id: render.present_id(),
                },
                width,
                height,
                row_bytes: render.row_bytes(),
                format: FramebufferObservationFormat::Bgra8Unorm,
                payload_bytes: render.bytes().len() as u64,
            },
            memory: memory.geometry(),
        };
        self.capture_and_write_observed(
            boundary,
            scenario,
            input_bytes,
            LiveObservedArtifacts {
                framebuffer_artifact_bytes: &render.canonical_bytes(),
                framebuffer_payload_bytes: render.bytes().len(),
                memory_bytes: memory.bytes(),
                observations,
            },
            report_path,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderEvidenceError {
    EmptyBackendIdentity,
    ZeroDimensions { width: u32, height: u32 },
    ZeroWorkloadId,
    ZeroPresentId,
    NonCanonicalRowBytes { expected: u32, observed: u32 },
    ByteLengthOverflow,
    WrongByteLength { expected: usize, observed: usize },
}

impl fmt::Display for RenderEvidenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyBackendIdentity => write!(f, "render backend identity must not be empty"),
            Self::ZeroDimensions { width, height } => {
                write!(
                    f,
                    "render evidence dimensions must be nonzero, got {width}x{height}"
                )
            }
            Self::ZeroPresentId => {
                write!(f, "post-VI render evidence requires a nonzero present ID")
            }
            Self::ZeroWorkloadId => {
                write!(f, "post-VI render evidence requires a nonzero workload ID")
            }
            Self::NonCanonicalRowBytes { expected, observed } => write!(
                f,
                "render evidence row size is {observed}, expected tight canonical size {expected}"
            ),
            Self::ByteLengthOverflow => write!(f, "render evidence byte length overflows"),
            Self::WrongByteLength { expected, observed } => write!(
                f,
                "render evidence has {observed} bytes, expected {expected} from its geometry"
            ),
        }
    }
}

impl std::error::Error for RenderEvidenceError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Rt64BackendIdentityError {
    Shape,
    Adapter,
    AdapterSha256,
    SourceRevision,
    Provenance,
    EmptyOverlay,
    CaptureApi,
}

pub(crate) fn validate_authoritative_rt64_backend_identity(
    identity: &str,
    platform: ReleaseHostPlatform,
) -> Result<(), Rt64BackendIdentityError> {
    let fields = identity.split(';').collect::<Vec<_>>();
    let [adapter, adapter_sha256, source, provenance, overlay, post_vi_api] = fields.as_slice()
    else {
        return Err(Rt64BackendIdentityError::Shape);
    };
    if *adapter != "adapter=fn64-render-rt64/rt64" {
        return Err(Rt64BackendIdentityError::Adapter);
    }
    if !adapter_sha256
        .strip_prefix("adapter_sha256=")
        .is_some_and(canonical_sha256)
    {
        return Err(Rt64BackendIdentityError::AdapterSha256);
    }
    if !source.strip_prefix("source=git:").is_some_and(|value| {
        value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(Rt64BackendIdentityError::SourceRevision);
    }
    if *provenance != "provenance=git-clean" {
        return Err(Rt64BackendIdentityError::Provenance);
    }
    if overlay
        .strip_prefix("overlay=")
        .is_none_or(|value| value.is_empty())
    {
        return Err(Rt64BackendIdentityError::EmptyOverlay);
    }
    let expected_api = match platform {
        ReleaseHostPlatform::MacosArm64 => "post_vi_api=metal-bgra8-unorm",
        ReleaseHostPlatform::LinuxX86_64 => "post_vi_api=vulkan-bgra8-rgba8-unorm",
        ReleaseHostPlatform::WindowsX86_64 => "post_vi_api=d3d12-or-vulkan-bgra8-rgba8-unorm",
    };
    if *post_vi_api != expected_api {
        return Err(Rt64BackendIdentityError::CaptureApi);
    }
    Ok(())
}

fn canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    push_u64(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArtifactKind, ClosureGate, FixedCycleDigestGate, ReleaseGateReport};

    fn evidence(
        backend: &str,
        width: u32,
        height: u32,
        workload_id: u64,
        present_id: u64,
        bytes: Vec<u8>,
    ) -> LiveRenderEvidence {
        evidence_with_settings(
            backend,
            [0x5a; 32],
            width,
            height,
            workload_id,
            present_id,
            bytes,
        )
    }

    fn evidence_with_settings(
        backend: &str,
        settings_sha256: [u8; 32],
        width: u32,
        height: u32,
        workload_id: u64,
        present_id: u64,
        bytes: Vec<u8>,
    ) -> LiveRenderEvidence {
        LiveRenderEvidence::post_vi_swapchain(
            42,
            backend,
            settings_sha256,
            width,
            height,
            width * 4,
            RenderPixelFormat::Bgra8Unorm,
            workload_id,
            present_id,
            bytes,
        )
        .unwrap()
    }

    fn authoritative_identity_for(
        adapter_nibble: char,
        source_nibble: char,
        platform: ReleaseHostPlatform,
    ) -> String {
        let post_vi_api = match platform {
            ReleaseHostPlatform::MacosArm64 => "metal-bgra8-unorm",
            ReleaseHostPlatform::LinuxX86_64 => "vulkan-bgra8-rgba8-unorm",
            ReleaseHostPlatform::WindowsX86_64 => "d3d12-or-vulkan-bgra8-rgba8-unorm",
        };
        format!(
            "adapter=fn64-render-rt64/rt64;adapter_sha256={};source=git:{};provenance=git-clean;overlay=fn64-test;post_vi_api={post_vi_api}",
            adapter_nibble.to_string().repeat(64),
            source_nibble.to_string().repeat(40),
        )
    }

    fn authoritative_identity(adapter_nibble: char, source_nibble: char) -> String {
        authoritative_identity_for(
            adapter_nibble,
            source_nibble,
            crate::release_host_platform().unwrap(),
        )
    }

    #[test]
    fn authoritative_identity_is_canonical_and_platform_specific() {
        for platform in [
            ReleaseHostPlatform::MacosArm64,
            ReleaseHostPlatform::LinuxX86_64,
            ReleaseHostPlatform::WindowsX86_64,
        ] {
            let identity = authoritative_identity_for('a', 'b', platform);
            validate_authoritative_rt64_backend_identity(&identity, platform).unwrap();
            for other in [
                ReleaseHostPlatform::MacosArm64,
                ReleaseHostPlatform::LinuxX86_64,
                ReleaseHostPlatform::WindowsX86_64,
            ] {
                if other != platform {
                    assert_eq!(
                        validate_authoritative_rt64_backend_identity(&identity, other),
                        Err(Rt64BackendIdentityError::CaptureApi)
                    );
                }
            }
        }
    }

    fn report(render: &LiveRenderEvidence) -> ReleaseGateReport {
        let mut gate = FixedCycleDigestGate::new(42);
        gate.capture(42, ArtifactKind::Framebuffer, &render.canonical_bytes())
            .unwrap();
        gate.capture(42, ArtifactKind::Audio, b"audio").unwrap();
        gate.capture(
            42,
            ArtifactKind::Memory,
            &vec![0; crate::DEFAULT_RDRAM_SIZE],
        )
        .unwrap();
        gate.capture(42, ArtifactKind::DeviceState, b"device")
            .unwrap();
        gate.capture(42, ArtifactKind::TimingTrace, b"trace")
            .unwrap();
        let mut closure = ClosureGate::default();
        closure.declare("render.post-vi").unwrap();
        closure.observe_supported("render.post-vi").unwrap();
        ReleaseGateReport::new(
            "rt64-live",
            b"private-input",
            gate.finish().unwrap(),
            ReleaseObservationGeometry::post_vi_swapchain(
                render.backend_identity(),
                hex(&render.settings_sha256()),
                render.workload_id().get(),
                render.present_id(),
                render.width,
                render.height,
                render.row_bytes,
                render.bytes.len() as u64,
            )
            .unwrap(),
            closure.finish(),
        )
        .unwrap()
    }

    #[test]
    fn fixed_cycle_report_binds_backend_geometry_workload_present_and_pixels() {
        let baseline_identity = authoritative_identity('a', 'b');
        let changed_identity = authoritative_identity('c', 'd');
        let baseline = evidence(&baseline_identity, 2, 1, 5, 7, vec![1; 8]);
        let baseline_report = report(&baseline);
        assert_eq!(
            baseline_report
                .observations
                .expected_framebuffer_artifact_bytes()
                .unwrap(),
            baseline.canonical_bytes().len() as u64
        );
        assert!(matches!(
            baseline_report.observations.framebuffer.source,
            FramebufferObservationSource::PostViSwapchain { .. }
        ));
        for changed in [
            evidence(&changed_identity, 2, 1, 5, 7, vec![1; 8]),
            evidence_with_settings(&baseline_identity, [0xa5; 32], 2, 1, 5, 7, vec![1; 8]),
            evidence(&baseline_identity, 1, 2, 5, 7, vec![1; 8]),
            evidence(&baseline_identity, 2, 1, 6, 7, vec![1; 8]),
            evidence(&baseline_identity, 2, 1, 5, 8, vec![1; 8]),
            evidence(&baseline_identity, 2, 1, 5, 7, vec![2; 8]),
        ] {
            let changed_report = report(&changed);
            assert_ne!(
                baseline_report.digest.root_sha256,
                changed_report.digest.root_sha256
            );
            assert_ne!(baseline_report.report_sha256, changed_report.report_sha256);
        }
        let mut mismatched_workload = baseline_report.clone();
        let FramebufferObservationSource::PostViSwapchain { workload_id, .. } =
            &mut mismatched_workload.observations.framebuffer.source
        else {
            unreachable!("fixture report uses post-VI evidence")
        };
        *workload_id = NonZeroU64::new(6).unwrap();
        assert!(matches!(
            mismatched_workload.verify_integrity(),
            Err(GateError::ReportIntegrityMismatch { .. })
        ));
        baseline_report.verify_integrity().unwrap();
    }

    #[test]
    fn evidence_rejects_zero_workload_and_ambiguous_or_inconsistent_layouts() {
        assert_eq!(
            LiveRenderEvidence::post_vi_swapchain(
                1,
                "",
                [0x5a; 32],
                1,
                1,
                4,
                RenderPixelFormat::Bgra8Unorm,
                1,
                1,
                vec![0; 4],
            ),
            Err(RenderEvidenceError::EmptyBackendIdentity)
        );
        assert_eq!(
            LiveRenderEvidence::post_vi_swapchain(
                1,
                "rt64",
                [0x5a; 32],
                2,
                1,
                7,
                RenderPixelFormat::Bgra8Unorm,
                1,
                1,
                vec![0; 8],
            ),
            Err(RenderEvidenceError::NonCanonicalRowBytes {
                expected: 8,
                observed: 7,
            })
        );
        assert_eq!(
            LiveRenderEvidence::post_vi_swapchain(
                1,
                "rt64",
                [0x5a; 32],
                1,
                1,
                4,
                RenderPixelFormat::Bgra8Unorm,
                0,
                1,
                vec![0; 4],
            ),
            Err(RenderEvidenceError::ZeroWorkloadId)
        );
        assert_eq!(
            LiveRenderEvidence::post_vi_swapchain(
                1,
                "rt64",
                [0x5a; 32],
                1,
                1,
                4,
                RenderPixelFormat::Bgra8Unorm,
                1,
                0,
                vec![0; 4],
            ),
            Err(RenderEvidenceError::ZeroPresentId)
        );
    }

    #[test]
    fn extension_rejects_render_capture_from_another_cycle_first() {
        let gate = LiveReleaseGate::new(43);
        let render = evidence("rt64", 1, 1, 1, 1, vec![0; 4]);
        let memory =
            LiveMemoryEvidence::full_physical_rdram(vec![0; crate::DEFAULT_RDRAM_SIZE]).unwrap();
        let error = gate
            .capture_and_write_render_evidence(
                crate::CommittedViBoundary::synthetic_for_test(43),
                "scenario",
                b"input",
                &render,
                &memory,
                "/tmp/unused",
            )
            .unwrap_err();
        assert!(matches!(
            error,
            GateError::WrongCycle {
                expected: 43,
                observed: 42,
                kind: ArtifactKind::Framebuffer,
            }
        ));
    }
}
