//! Schema-bound geometry for the host-owned framebuffer and memory artifacts.
//!
//! Artifact hashes alone do not say which bytes were sampled. These types bind
//! the source, destination geometry, and complete physical-RDRAM extent into
//! the release report's canonical evidence wire.

use std::path::Path;
use std::{fmt, num::NonZeroU64};

use serde::{Deserialize, Serialize};

use crate::{
    release_gate::LiveObservedArtifacts, GateError, LiveReleaseGate, ReleaseGateReport,
    DEFAULT_RDRAM_SIZE,
};

pub(crate) const RENDER_EVIDENCE_SCHEMA: &[u8] = b"fn64.render-evidence.v3\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FramebufferObservationFormat {
    Rgba16,
    Bgra8Unorm,
}

impl FramebufferObservationFormat {
    pub(crate) const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba16 => 2,
            Self::Bgra8Unorm => 4,
        }
    }

    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Rgba16 => 0,
            Self::Bgra8Unorm => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FramebufferObservationSource {
    PhysicalRdram {
        address: u32,
    },
    PostViSwapchain {
        backend_identity: String,
        settings_sha256: String,
        workload_id: NonZeroU64,
        present_id: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramebufferObservationGeometry {
    pub source: FramebufferObservationSource,
    pub width: u32,
    pub height: u32,
    pub row_bytes: u32,
    pub format: FramebufferObservationFormat,
    pub payload_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryObservationGeometry {
    pub physical_address: u32,
    pub payload_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseObservationGeometry {
    pub framebuffer: FramebufferObservationGeometry,
    pub memory: MemoryObservationGeometry,
}

impl ReleaseObservationGeometry {
    pub fn reference_rdram(
        address: u32,
        width: u32,
        height: u32,
    ) -> Result<Self, ObservationEvidenceError> {
        let row_bytes = width
            .checked_mul(FramebufferObservationFormat::Rgba16.bytes_per_pixel())
            .ok_or(ObservationEvidenceError::ByteLengthOverflow)?;
        let payload_bytes = u64::from(row_bytes)
            .checked_mul(u64::from(height))
            .ok_or(ObservationEvidenceError::ByteLengthOverflow)?;
        let observations = Self {
            framebuffer: FramebufferObservationGeometry {
                source: FramebufferObservationSource::PhysicalRdram { address },
                width,
                height,
                row_bytes,
                format: FramebufferObservationFormat::Rgba16,
                payload_bytes,
            },
            memory: MemoryObservationGeometry {
                physical_address: 0,
                payload_bytes: DEFAULT_RDRAM_SIZE as u64,
            },
        };
        observations.validate()?;
        Ok(observations)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn post_vi_swapchain(
        backend_identity: impl Into<String>,
        settings_sha256: String,
        workload_id: u64,
        present_id: u64,
        width: u32,
        height: u32,
        row_bytes: u32,
        payload_bytes: u64,
    ) -> Result<Self, ObservationEvidenceError> {
        let workload_id =
            NonZeroU64::new(workload_id).ok_or(ObservationEvidenceError::ZeroWorkloadId)?;
        let observations = Self {
            framebuffer: FramebufferObservationGeometry {
                source: FramebufferObservationSource::PostViSwapchain {
                    backend_identity: backend_identity.into(),
                    settings_sha256,
                    workload_id,
                    present_id,
                },
                width,
                height,
                row_bytes,
                format: FramebufferObservationFormat::Bgra8Unorm,
                payload_bytes,
            },
            memory: MemoryObservationGeometry {
                physical_address: 0,
                payload_bytes: DEFAULT_RDRAM_SIZE as u64,
            },
        };
        observations.validate()?;
        Ok(observations)
    }

    pub(crate) fn validate(&self) -> Result<(), ObservationEvidenceError> {
        let framebuffer = &self.framebuffer;
        if framebuffer.width == 0 || framebuffer.height == 0 {
            return Err(ObservationEvidenceError::ZeroFramebufferDimensions {
                width: framebuffer.width,
                height: framebuffer.height,
            });
        }
        let expected_row_bytes = framebuffer
            .width
            .checked_mul(framebuffer.format.bytes_per_pixel())
            .ok_or(ObservationEvidenceError::ByteLengthOverflow)?;
        if framebuffer.row_bytes != expected_row_bytes {
            return Err(ObservationEvidenceError::NonCanonicalFramebufferRows {
                expected: expected_row_bytes,
                observed: framebuffer.row_bytes,
            });
        }
        let expected_payload = u64::from(framebuffer.row_bytes)
            .checked_mul(u64::from(framebuffer.height))
            .ok_or(ObservationEvidenceError::ByteLengthOverflow)?;
        if framebuffer.payload_bytes != expected_payload {
            return Err(ObservationEvidenceError::WrongFramebufferPayload {
                expected: expected_payload,
                observed: framebuffer.payload_bytes,
            });
        }
        match &framebuffer.source {
            FramebufferObservationSource::PhysicalRdram { address } => {
                if framebuffer.format != FramebufferObservationFormat::Rgba16 {
                    return Err(ObservationEvidenceError::RdramFramebufferFormat(
                        framebuffer.format,
                    ));
                }
                let end = u64::from(*address)
                    .checked_add(framebuffer.payload_bytes)
                    .ok_or(ObservationEvidenceError::ByteLengthOverflow)?;
                if end > DEFAULT_RDRAM_SIZE as u64 {
                    return Err(ObservationEvidenceError::FramebufferOutsideRdram {
                        address: *address,
                        bytes: framebuffer.payload_bytes,
                    });
                }
            }
            FramebufferObservationSource::PostViSwapchain {
                backend_identity,
                settings_sha256,
                present_id,
                ..
            } => {
                if framebuffer.format != FramebufferObservationFormat::Bgra8Unorm {
                    return Err(ObservationEvidenceError::PostViFramebufferFormat(
                        framebuffer.format,
                    ));
                }
                if backend_identity.is_empty() {
                    return Err(ObservationEvidenceError::EmptyBackendIdentity);
                }
                validate_sha256(settings_sha256)
                    .map_err(|()| ObservationEvidenceError::InvalidSettingsSha256)?;
                if *present_id == 0 {
                    return Err(ObservationEvidenceError::ZeroPresentId);
                }
            }
        }
        if self.memory.physical_address != 0
            || self.memory.payload_bytes != DEFAULT_RDRAM_SIZE as u64
        {
            return Err(ObservationEvidenceError::IncompletePhysicalRdram {
                address: self.memory.physical_address,
                expected_bytes: DEFAULT_RDRAM_SIZE as u64,
                observed_bytes: self.memory.payload_bytes,
            });
        }
        Ok(())
    }

    pub(crate) fn validate_payload_lengths(
        &self,
        framebuffer_payload_bytes: usize,
        memory_payload_bytes: usize,
    ) -> Result<(), ObservationEvidenceError> {
        self.validate()?;
        if self.framebuffer.payload_bytes != framebuffer_payload_bytes as u64 {
            return Err(ObservationEvidenceError::WrongFramebufferPayload {
                expected: self.framebuffer.payload_bytes,
                observed: framebuffer_payload_bytes as u64,
            });
        }
        if self.memory.payload_bytes != memory_payload_bytes as u64 {
            return Err(ObservationEvidenceError::WrongMemoryPayload {
                expected: self.memory.payload_bytes,
                observed: memory_payload_bytes as u64,
            });
        }
        Ok(())
    }

    pub(crate) fn expected_framebuffer_artifact_bytes(
        &self,
    ) -> Result<u64, ObservationEvidenceError> {
        match &self.framebuffer.source {
            FramebufferObservationSource::PhysicalRdram { .. } => {
                Ok(self.framebuffer.payload_bytes)
            }
            FramebufferObservationSource::PostViSwapchain {
                backend_identity, ..
            } => {
                // `LiveRenderEvidence::canonical_bytes`: schema, cycle,
                // length-prefixed backend, settings, stage, geometry, format,
                // workload and presentation IDs, then length-prefixed pixel
                // payload.
                let fixed_bytes = u64::try_from(RENDER_EVIDENCE_SCHEMA.len())
                    .map_err(|_| ObservationEvidenceError::ByteLengthOverflow)?
                    .checked_add(8 + 8 + 32 + 1 + 12 + 1 + 8 + 8 + 8)
                    .ok_or(ObservationEvidenceError::ByteLengthOverflow)?;
                let backend_bytes = u64::try_from(backend_identity.len())
                    .map_err(|_| ObservationEvidenceError::ByteLengthOverflow)?;
                fixed_bytes
                    .checked_add(backend_bytes)
                    .and_then(|bytes| bytes.checked_add(self.framebuffer.payload_bytes))
                    .ok_or(ObservationEvidenceError::ByteLengthOverflow)
            }
        }
    }
}

/// Validated live reference-renderer framebuffer bytes in logical RDRAM order.
pub struct LiveReferenceFramebufferEvidence {
    geometry: FramebufferObservationGeometry,
    bytes: Vec<u8>,
}

impl LiveReferenceFramebufferEvidence {
    pub fn rgba16(
        address: u32,
        width: u32,
        height: u32,
        bytes: Vec<u8>,
    ) -> Result<Self, ObservationEvidenceError> {
        let observations = ReleaseObservationGeometry::reference_rdram(address, width, height)?;
        if observations.framebuffer.payload_bytes != bytes.len() as u64 {
            return Err(ObservationEvidenceError::WrongFramebufferPayload {
                expected: observations.framebuffer.payload_bytes,
                observed: bytes.len() as u64,
            });
        }
        Ok(Self {
            geometry: observations.framebuffer,
            bytes,
        })
    }

    pub(crate) fn geometry(&self) -> FramebufferObservationGeometry {
        self.geometry.clone()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

pub trait LiveReleaseGateObservationExt {
    fn capture_and_write_reference_evidence(
        self,
        boundary: crate::CommittedViBoundary,
        scenario: impl Into<String>,
        input_bytes: &[u8],
        framebuffer: &LiveReferenceFramebufferEvidence,
        report_path: impl AsRef<Path>,
    ) -> Result<ReleaseGateReport, GateError>;

    /// Capture a full-ROM report whose provenance class is declared
    /// explicitly. ROM bytes cannot distinguish retail from public homebrew.
    fn capture_and_write_reference_rom_evidence(
        self,
        boundary: crate::CommittedViBoundary,
        scenario: impl Into<String>,
        rom: crate::ReleaseRomInput<'_>,
        framebuffer: &LiveReferenceFramebufferEvidence,
        report_path: impl AsRef<Path>,
    ) -> Result<ReleaseGateReport, GateError>;
}

impl LiveReleaseGateObservationExt for LiveReleaseGate {
    fn capture_and_write_reference_evidence(
        self,
        boundary: crate::CommittedViBoundary,
        scenario: impl Into<String>,
        input_bytes: &[u8],
        framebuffer: &LiveReferenceFramebufferEvidence,
        report_path: impl AsRef<Path>,
    ) -> Result<ReleaseGateReport, GateError> {
        self.capture_and_write_observed(
            boundary,
            scenario,
            input_bytes,
            None,
            LiveObservedArtifacts {
                framebuffer_artifact_bytes: framebuffer.bytes(),
                framebuffer_payload_bytes: framebuffer.bytes().len(),
                observations: ReleaseObservationGeometry {
                    framebuffer: framebuffer.geometry(),
                    memory: MemoryObservationGeometry {
                        physical_address: 0,
                        payload_bytes: DEFAULT_RDRAM_SIZE as u64,
                    },
                },
            },
            report_path,
        )
    }

    fn capture_and_write_reference_rom_evidence(
        self,
        boundary: crate::CommittedViBoundary,
        scenario: impl Into<String>,
        rom: crate::ReleaseRomInput<'_>,
        framebuffer: &LiveReferenceFramebufferEvidence,
        report_path: impl AsRef<Path>,
    ) -> Result<ReleaseGateReport, GateError> {
        self.capture_and_write_observed(
            boundary,
            scenario,
            rom.bytes(),
            Some(rom.class()),
            LiveObservedArtifacts {
                framebuffer_artifact_bytes: framebuffer.bytes(),
                framebuffer_payload_bytes: framebuffer.bytes().len(),
                observations: ReleaseObservationGeometry {
                    framebuffer: framebuffer.geometry(),
                    memory: MemoryObservationGeometry {
                        physical_address: 0,
                        payload_bytes: DEFAULT_RDRAM_SIZE as u64,
                    },
                },
            },
            report_path,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservationEvidenceError {
    ZeroFramebufferDimensions {
        width: u32,
        height: u32,
    },
    ByteLengthOverflow,
    NonCanonicalFramebufferRows {
        expected: u32,
        observed: u32,
    },
    WrongFramebufferPayload {
        expected: u64,
        observed: u64,
    },
    WrongMemoryPayload {
        expected: u64,
        observed: u64,
    },
    RdramFramebufferFormat(FramebufferObservationFormat),
    PostViFramebufferFormat(FramebufferObservationFormat),
    FramebufferOutsideRdram {
        address: u32,
        bytes: u64,
    },
    EmptyBackendIdentity,
    InvalidSettingsSha256,
    ZeroWorkloadId,
    ZeroPresentId,
    IncompletePhysicalRdram {
        address: u32,
        expected_bytes: u64,
        observed_bytes: u64,
    },
}

impl fmt::Display for ObservationEvidenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid release observation geometry: {self:?}")
    }
}

impl std::error::Error for ObservationEvidenceError {}

fn validate_sha256(value: &str) -> Result<(), ()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_reference_evidence_is_physical_and_exact() {
        let framebuffer =
            LiveReferenceFramebufferEvidence::rgba16(0x1000, 2, 1, vec![0; 4]).unwrap();
        assert!(matches!(
            framebuffer.geometry.source,
            FramebufferObservationSource::PhysicalRdram { address: 0x1000 }
        ));
        assert!(matches!(
            LiveReferenceFramebufferEvidence::rgba16(0, 2, 1, vec![0; 8]),
            Err(ObservationEvidenceError::WrongFramebufferPayload { .. })
        ));
    }

    #[test]
    fn post_vi_geometry_rejects_zero_workload_noncanonical_rows_and_sha() {
        assert!(matches!(
            ReleaseObservationGeometry::post_vi_swapchain(
                "rt64:test",
                "11".repeat(32),
                1,
                1,
                2,
                1,
                7,
                8,
            ),
            Err(ObservationEvidenceError::NonCanonicalFramebufferRows { .. })
        ));
        assert!(matches!(
            ReleaseObservationGeometry::post_vi_swapchain(
                "rt64:test",
                "not-a-sha".to_owned(),
                1,
                1,
                1,
                1,
                4,
                4,
            ),
            Err(ObservationEvidenceError::InvalidSettingsSha256)
        ));
        assert!(matches!(
            ReleaseObservationGeometry::post_vi_swapchain(
                "rt64:test",
                "11".repeat(32),
                0,
                1,
                1,
                1,
                4,
                4,
            ),
            Err(ObservationEvidenceError::ZeroWorkloadId)
        ));

        let geometry = ReleaseObservationGeometry::post_vi_swapchain(
            "rt64:test",
            "11".repeat(32),
            5,
            7,
            1,
            1,
            4,
            4,
        )
        .unwrap();
        let mut zero: serde_json::Value = serde_json::to_value(&geometry).unwrap();
        zero["framebuffer"]["source"]["workload_id"] = 0.into();
        assert!(serde_json::from_value::<ReleaseObservationGeometry>(zero).is_err());

        let mut missing: serde_json::Value = serde_json::to_value(&geometry).unwrap();
        missing["framebuffer"]["source"]
            .as_object_mut()
            .unwrap()
            .remove("workload_id");
        assert!(serde_json::from_value::<ReleaseObservationGeometry>(missing).is_err());
    }
}
