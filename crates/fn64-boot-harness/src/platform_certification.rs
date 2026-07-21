//! Opaque authority for actual-host RT64 platform case series.
//!
//! Retained JSON is integrity evidence only. A release matrix can receive
//! target-case credit solely from [`VerifiedRt64PlatformCaseSeries`], which is
//! deliberately neither serializable nor publicly constructible. Phase one
//! defines and collision-tests that boundary; no production runner creates a
//! capability until it can own an exact staged child and child-observed host,
//! API, and behavior evidence.

use crate::{
    ReleaseGraphicsApi, ReleaseHostPlatform, ReleaseWindowsFamily, ReleaseWindowsVersionEvidence,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;

pub const VERIFIED_RT64_PLATFORM_CASE_AUTHORITY_SCHEMA: &str =
    "fn64.verified-rt64-platform-case-authority.v1";
const PINNED_RT64_SOURCE_ID: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rt64PlatformTarget {
    MacosMetal,
    LinuxVulkan,
    Windows10D3d12,
    Windows10Vulkan,
    Windows11D3d12,
    Windows11Vulkan,
}

impl Rt64PlatformTarget {
    pub const ALL: [Self; 6] = [
        Self::MacosMetal,
        Self::LinuxVulkan,
        Self::Windows10D3d12,
        Self::Windows10Vulkan,
        Self::Windows11D3d12,
        Self::Windows11Vulkan,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::MacosMetal => "macos-metal",
            Self::LinuxVulkan => "linux-vulkan",
            Self::Windows10D3d12 => "windows10-d3d12",
            Self::Windows10Vulkan => "windows10-vulkan",
            Self::Windows11D3d12 => "windows11-d3d12",
            Self::Windows11Vulkan => "windows11-vulkan",
        }
    }

    pub const fn os_family_id(self) -> &'static str {
        match self {
            Self::MacosMetal => "macos",
            Self::LinuxVulkan => "linux",
            Self::Windows10D3d12 | Self::Windows10Vulkan => "windows10",
            Self::Windows11D3d12 | Self::Windows11Vulkan => "windows11",
        }
    }

    pub const fn graphics_api(self) -> ReleaseGraphicsApi {
        match self {
            Self::MacosMetal => ReleaseGraphicsApi::Metal,
            Self::LinuxVulkan | Self::Windows10Vulkan | Self::Windows11Vulkan => {
                ReleaseGraphicsApi::Vulkan
            }
            Self::Windows10D3d12 | Self::Windows11D3d12 => ReleaseGraphicsApi::D3d12,
        }
    }

    pub const fn capture_api(self) -> &'static str {
        match self.graphics_api() {
            ReleaseGraphicsApi::Metal => "metal-bgra8-unorm",
            ReleaseGraphicsApi::Vulkan => "vulkan-bgra8-rgba8-unorm",
            ReleaseGraphicsApi::D3d12 => "d3d12-bgra8-rgba8-unorm",
        }
    }

    pub(crate) fn matches_host(
        self,
        platform: ReleaseHostPlatform,
        windows_version: Option<ReleaseWindowsVersionEvidence>,
    ) -> bool {
        match (self, platform, windows_version) {
            (Self::MacosMetal, ReleaseHostPlatform::MacosArm64, None)
            | (Self::LinuxVulkan, ReleaseHostPlatform::LinuxX86_64, None) => true,
            (
                Self::Windows10D3d12 | Self::Windows10Vulkan,
                ReleaseHostPlatform::WindowsX86_64,
                Some(version),
            ) => version.verify().is_ok() && version.family == ReleaseWindowsFamily::Windows10,
            (
                Self::Windows11D3d12 | Self::Windows11Vulkan,
                ReleaseHostPlatform::WindowsX86_64,
                Some(version),
            ) => version.verify().is_ok() && version.family == ReleaseWindowsFamily::Windows11,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rt64PlatformCase {
    BackendLifecycle,
    ResolutionDownsample,
    UserControlsRebuild,
    EnhancementEmulatorControls,
    FramebufferRdramRegion,
    FramebufferEnhancement,
    TextureReplacements,
    LatencySkipBuffering,
    LatencyPresentEarly,
    DeferredDebugger,
    UbershaderCriticalPath,
    HfrHleCooperation,
    ExtendedGbiCooperation,
}

impl Rt64PlatformCase {
    pub const ALL: [Self; 13] = [
        Self::BackendLifecycle,
        Self::ResolutionDownsample,
        Self::UserControlsRebuild,
        Self::EnhancementEmulatorControls,
        Self::FramebufferRdramRegion,
        Self::FramebufferEnhancement,
        Self::TextureReplacements,
        Self::LatencySkipBuffering,
        Self::LatencyPresentEarly,
        Self::DeferredDebugger,
        Self::UbershaderCriticalPath,
        Self::HfrHleCooperation,
        Self::ExtendedGbiCooperation,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::BackendLifecycle => "backend-lifecycle",
            Self::ResolutionDownsample => "resolution-downsample",
            Self::UserControlsRebuild => "user-controls-rebuild",
            Self::EnhancementEmulatorControls => "enhancement-emulator-controls",
            Self::FramebufferRdramRegion => "framebuffer-rdram-region",
            Self::FramebufferEnhancement => "framebuffer-enhancement",
            Self::TextureReplacements => "texture-replacements",
            Self::LatencySkipBuffering => "latency-skip-buffering",
            Self::LatencyPresentEarly => "latency-present-early",
            Self::DeferredDebugger => "deferred-debugger",
            Self::UbershaderCriticalPath => "ubershader-critical-path",
            Self::HfrHleCooperation => "hfr-hle-cooperation",
            Self::ExtendedGbiCooperation => "extended-gbi-cooperation",
        }
    }

    pub const fn repeat_bar(self) -> usize {
        match self {
            Self::BackendLifecycle
            | Self::UserControlsRebuild
            | Self::LatencyPresentEarly
            | Self::DeferredDebugger
            | Self::UbershaderCriticalPath
            | Self::HfrHleCooperation => 20,
            _ => 10,
        }
    }
}

/// Retained, integrity-checkable projection of one opaque verified series.
/// This type cannot be supplied to matrix construction as authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedRt64PlatformCaseAuthority {
    pub schema: String,
    pub target: Rt64PlatformTarget,
    pub case: Rt64PlatformCase,
    pub platform: ReleaseHostPlatform,
    pub windows_version: Option<ReleaseWindowsVersionEvidence>,
    pub graphics_api: ReleaseGraphicsApi,
    pub capture_api: String,
    pub rt64_source_id: String,
    pub adapter_source_sha256: String,
    pub child_executable_sha256: String,
    pub bound_report_scenario: String,
    pub bound_report_sha256: String,
    pub bound_matrix_run_event_sha256s: Vec<String>,
    pub semantic_evidence_sha256: String,
    pub run_event_sha256s: Vec<String>,
    pub authority_sha256: String,
}

impl VerifiedRt64PlatformCaseAuthority {
    pub(crate) fn verify_integrity(&self) -> Result<(), PlatformCertificationError> {
        if self.schema != VERIFIED_RT64_PLATFORM_CASE_AUTHORITY_SCHEMA {
            return Err(PlatformCertificationError::UnsupportedSchema(
                self.schema.clone(),
            ));
        }
        if !self
            .target
            .matches_host(self.platform, self.windows_version)
        {
            return Err(PlatformCertificationError::TargetHostMismatch);
        }
        if self.graphics_api != self.target.graphics_api()
            || self.capture_api != self.target.capture_api()
        {
            return Err(PlatformCertificationError::TargetApiMismatch);
        }
        if self.rt64_source_id != PINNED_RT64_SOURCE_ID {
            return Err(PlatformCertificationError::SourceMismatch);
        }
        for (field, value) in [
            ("adapter_source_sha256", self.adapter_source_sha256.as_str()),
            (
                "child_executable_sha256",
                self.child_executable_sha256.as_str(),
            ),
            ("bound_report_sha256", self.bound_report_sha256.as_str()),
            (
                "semantic_evidence_sha256",
                self.semantic_evidence_sha256.as_str(),
            ),
            ("authority_sha256", self.authority_sha256.as_str()),
        ] {
            canonical_sha256(value)
                .then_some(())
                .ok_or(PlatformCertificationError::InvalidSha256(field))?;
        }
        if self.bound_report_scenario.is_empty() {
            return Err(PlatformCertificationError::EmptyBoundReportScenario);
        }
        validate_unique_events(
            &self.bound_matrix_run_event_sha256s,
            crate::RELEASE_MATRIX_REPORT_COUNT,
            "bound_matrix_run_event_sha256s[]",
        )?;
        if self.run_event_sha256s.len() != self.case.repeat_bar() {
            return Err(PlatformCertificationError::WrongRunCount {
                expected: self.case.repeat_bar(),
                observed: self.run_event_sha256s.len(),
            });
        }
        let mut unique = BTreeSet::new();
        for value in &self.run_event_sha256s {
            if !canonical_sha256(value) {
                return Err(PlatformCertificationError::InvalidSha256(
                    "run_event_sha256s[]",
                ));
            }
            if !unique.insert(value) {
                return Err(PlatformCertificationError::DuplicateRunEvent(value.clone()));
            }
        }
        let recomputed = self.recompute_authority_sha256();
        if self.authority_sha256 != recomputed {
            return Err(PlatformCertificationError::AuthorityDigestMismatch {
                stored: self.authority_sha256.clone(),
                recomputed,
            });
        }
        Ok(())
    }

    fn recompute_authority_sha256(&self) -> String {
        let mut wire = Vec::new();
        wire.extend_from_slice(b"fn64.verified-rt64-platform-case-authority.v1\0");
        push_bytes(&mut wire, self.schema.as_bytes());
        push_bytes(&mut wire, self.target.id().as_bytes());
        push_bytes(&mut wire, self.case.id().as_bytes());
        push_host(&mut wire, self.platform, self.windows_version);
        wire.push(graphics_api_tag(self.graphics_api));
        push_bytes(&mut wire, self.capture_api.as_bytes());
        push_bytes(&mut wire, self.rt64_source_id.as_bytes());
        push_bytes(&mut wire, self.adapter_source_sha256.as_bytes());
        push_bytes(&mut wire, self.child_executable_sha256.as_bytes());
        push_bytes(&mut wire, self.bound_report_scenario.as_bytes());
        push_bytes(&mut wire, self.bound_report_sha256.as_bytes());
        wire.extend_from_slice(&(self.bound_matrix_run_event_sha256s.len() as u64).to_be_bytes());
        for event in &self.bound_matrix_run_event_sha256s {
            push_bytes(&mut wire, event.as_bytes());
        }
        push_bytes(&mut wire, self.semantic_evidence_sha256.as_bytes());
        wire.extend_from_slice(&(self.run_event_sha256s.len() as u64).to_be_bytes());
        for event in &self.run_event_sha256s {
            push_bytes(&mut wire, event.as_bytes());
        }
        hex(&Sha256::digest(wire))
    }
}

/// Opaque local process authority. It has no `Deserialize` implementation and
/// no public constructor, so a self-hashed result or caller target label cannot
/// be promoted into release-matrix credit.
#[derive(Debug)]
pub struct VerifiedRt64PlatformCaseSeries {
    authority: VerifiedRt64PlatformCaseAuthority,
}

impl VerifiedRt64PlatformCaseSeries {
    pub(crate) fn revalidate_for_release_matrix(
        &self,
    ) -> Result<VerifiedRt64PlatformCaseAuthority, PlatformCertificationError> {
        self.authority.verify_integrity()?;
        Ok(self.authority.clone())
    }

    #[cfg(test)]
    pub(crate) fn fixture_for_test(
        target: Rt64PlatformTarget,
        case: Rt64PlatformCase,
        host: (ReleaseHostPlatform, Option<ReleaseWindowsVersionEvidence>),
        report_binding: (&str, String, Vec<String>),
        seed: u8,
    ) -> Result<Self, PlatformCertificationError> {
        let (platform, windows_version) = host;
        let (bound_report_scenario, bound_report_sha256, bound_matrix_run_event_sha256s) =
            report_binding;
        let mut authority = VerifiedRt64PlatformCaseAuthority {
            schema: VERIFIED_RT64_PLATFORM_CASE_AUTHORITY_SCHEMA.to_owned(),
            target,
            case,
            platform,
            windows_version,
            graphics_api: target.graphics_api(),
            capture_api: target.capture_api().to_owned(),
            rt64_source_id: PINNED_RT64_SOURCE_ID.to_owned(),
            adapter_source_sha256: hex(&Sha256::digest([seed, 0])),
            child_executable_sha256: hex(&Sha256::digest([seed, 1])),
            bound_report_scenario: bound_report_scenario.to_owned(),
            bound_report_sha256,
            bound_matrix_run_event_sha256s,
            semantic_evidence_sha256: hex(&Sha256::digest([seed, 2])),
            run_event_sha256s: (0..case.repeat_bar())
                .map(|ordinal| hex(&Sha256::digest([seed, ordinal as u8, 3])))
                .collect(),
            authority_sha256: String::new(),
        };
        authority.authority_sha256 = authority.recompute_authority_sha256();
        authority.verify_integrity()?;
        Ok(Self { authority })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformCertificationError {
    UnsupportedSchema(String),
    TargetHostMismatch,
    TargetApiMismatch,
    SourceMismatch,
    InvalidSha256(&'static str),
    EmptyBoundReportScenario,
    WrongRunCount { expected: usize, observed: usize },
    DuplicateRunEvent(String),
    AuthorityDigestMismatch { stored: String, recomputed: String },
}

impl fmt::Display for PlatformCertificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema(schema) => {
                write!(
                    formatter,
                    "unsupported RT64 platform authority schema {schema:?}"
                )
            }
            Self::TargetHostMismatch => write!(formatter, "RT64 platform target mismatches host"),
            Self::TargetApiMismatch => write!(formatter, "RT64 platform target mismatches API"),
            Self::SourceMismatch => {
                write!(formatter, "RT64 platform source is not the pinned tree")
            }
            Self::InvalidSha256(field) => write!(formatter, "invalid SHA-256 in {field}"),
            Self::EmptyBoundReportScenario => {
                write!(
                    formatter,
                    "RT64 platform authority has no bound report scenario"
                )
            }
            Self::WrongRunCount { expected, observed } => write!(
                formatter,
                "RT64 platform series has {observed} runs; exactly {expected} are required"
            ),
            Self::DuplicateRunEvent(event) => {
                write!(formatter, "RT64 platform series repeats run event {event}")
            }
            Self::AuthorityDigestMismatch { stored, recomputed } => write!(
                formatter,
                "RT64 platform authority digest mismatch: stored={stored}, recomputed={recomputed}"
            ),
        }
    }
}

impl std::error::Error for PlatformCertificationError {}

fn push_host(
    wire: &mut Vec<u8>,
    platform: ReleaseHostPlatform,
    windows_version: Option<ReleaseWindowsVersionEvidence>,
) {
    wire.push(match platform {
        ReleaseHostPlatform::MacosArm64 => 0,
        ReleaseHostPlatform::LinuxX86_64 => 1,
        ReleaseHostPlatform::WindowsX86_64 => 2,
    });
    match windows_version {
        None => wire.push(0),
        Some(version) => {
            wire.push(1);
            wire.push(match version.family {
                ReleaseWindowsFamily::Windows10 => 0,
                ReleaseWindowsFamily::Windows11 => 1,
            });
            wire.extend_from_slice(&version.major.to_be_bytes());
            wire.extend_from_slice(&version.minor.to_be_bytes());
            wire.extend_from_slice(&version.build.to_be_bytes());
            wire.extend_from_slice(&version.update_build_revision.to_be_bytes());
            wire.push(0);
        }
    }
}

const fn graphics_api_tag(api: ReleaseGraphicsApi) -> u8 {
    match api {
        ReleaseGraphicsApi::D3d12 => 0,
        ReleaseGraphicsApi::Vulkan => 1,
        ReleaseGraphicsApi::Metal => 2,
    }
}

fn canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_unique_events(
    events: &[String],
    expected: usize,
    field: &'static str,
) -> Result<(), PlatformCertificationError> {
    if events.len() != expected {
        return Err(PlatformCertificationError::WrongRunCount {
            expected,
            observed: events.len(),
        });
    }
    let mut unique = BTreeSet::new();
    for event in events {
        if !canonical_sha256(event) {
            return Err(PlatformCertificationError::InvalidSha256(field));
        }
        if !unique.insert(event) {
            return Err(PlatformCertificationError::DuplicateRunEvent(event.clone()));
        }
    }
    Ok(())
}

fn push_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows(build: u32) -> ReleaseWindowsVersionEvidence {
        ReleaseWindowsVersionEvidence::from_native_workstation(10, 0, build, 123).unwrap()
    }

    fn bound_events(seed: u8) -> Vec<String> {
        (0..crate::RELEASE_MATRIX_REPORT_COUNT)
            .map(|ordinal| hex(&Sha256::digest([seed, ordinal as u8, 9])))
            .collect()
    }

    fn bound_report(seed: u8) -> String {
        hex(&Sha256::digest([seed, 8]))
    }

    #[test]
    fn windows_family_is_derived_from_build_and_relabel_fails() {
        let ten = windows(21_999);
        let eleven = windows(22_000);
        assert_eq!(ten.family, ReleaseWindowsFamily::Windows10);
        assert_eq!(eleven.family, ReleaseWindowsFamily::Windows11);

        let mut relabeled = ten;
        relabeled.family = ReleaseWindowsFamily::Windows11;
        assert!(relabeled.verify().is_err());
        assert!(!Rt64PlatformTarget::Windows11D3d12
            .matches_host(ReleaseHostPlatform::WindowsX86_64, Some(relabeled),));

        let server = serde_json::json!({
            "family": "windows10",
            "major": 10,
            "minor": 0,
            "build": 19045,
            "update_build_revision": 6456,
            "product_type": "server"
        });
        assert!(serde_json::from_value::<ReleaseWindowsVersionEvidence>(server).is_err());
    }

    #[test]
    fn opaque_fixture_rejects_target_host_mismatch_and_tamper() {
        assert!(VerifiedRt64PlatformCaseSeries::fixture_for_test(
            Rt64PlatformTarget::Windows11Vulkan,
            Rt64PlatformCase::ResolutionDownsample,
            (ReleaseHostPlatform::WindowsX86_64, Some(windows(22_000)),),
            ("windows11-vulkan-report", bound_report(1), bound_events(1)),
            1,
        )
        .is_ok());
        assert!(matches!(
            VerifiedRt64PlatformCaseSeries::fixture_for_test(
                Rt64PlatformTarget::Windows10Vulkan,
                Rt64PlatformCase::ResolutionDownsample,
                (ReleaseHostPlatform::WindowsX86_64, Some(windows(22_000)),),
                ("windows10-vulkan-report", bound_report(2), bound_events(2)),
                2,
            ),
            Err(PlatformCertificationError::TargetHostMismatch)
        ));

        let series = VerifiedRt64PlatformCaseSeries::fixture_for_test(
            Rt64PlatformTarget::MacosMetal,
            Rt64PlatformCase::BackendLifecycle,
            (ReleaseHostPlatform::MacosArm64, None),
            ("macos-metal-report", bound_report(3), bound_events(3)),
            3,
        )
        .unwrap();
        let mut retained = series.revalidate_for_release_matrix().unwrap();
        retained.capture_api = "caller-label".to_owned();
        assert_eq!(
            retained.verify_integrity(),
            Err(PlatformCertificationError::TargetApiMismatch)
        );
    }

    #[test]
    fn authority_requires_exact_repeat_bar_and_unique_events() {
        let series = VerifiedRt64PlatformCaseSeries::fixture_for_test(
            Rt64PlatformTarget::LinuxVulkan,
            Rt64PlatformCase::DeferredDebugger,
            (ReleaseHostPlatform::LinuxX86_64, None),
            ("linux-vulkan-report", bound_report(4), bound_events(4)),
            4,
        )
        .unwrap();
        let mut retained = series.revalidate_for_release_matrix().unwrap();
        retained.run_event_sha256s.pop();
        retained.authority_sha256 = retained.recompute_authority_sha256();
        assert!(matches!(
            retained.verify_integrity(),
            Err(PlatformCertificationError::WrongRunCount {
                expected: 20,
                observed: 19
            })
        ));
    }

    #[test]
    fn typed_target_and_case_denominator_matches_the_project_catalog() {
        let catalog: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/rt64-platform-certification.json"
        ))
        .unwrap();
        let targets = catalog["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|target| {
                (
                    target["id"].as_str().unwrap(),
                    target["os_family"].as_str().unwrap(),
                    target["graphics_api"].as_str().unwrap(),
                    target["capture_api"].as_str().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            targets,
            Rt64PlatformTarget::ALL
                .iter()
                .map(|target| {
                    (
                        target.id(),
                        target.os_family_id(),
                        match target.graphics_api() {
                            ReleaseGraphicsApi::D3d12 => "d3d12",
                            ReleaseGraphicsApi::Vulkan => "vulkan",
                            ReleaseGraphicsApi::Metal => "metal",
                        },
                        target.capture_api(),
                    )
                })
                .collect::<Vec<_>>()
        );

        let cases = catalog["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|case| {
                (
                    case["id"].as_str().unwrap(),
                    case["repeat_bar"].as_u64().unwrap() as usize,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            cases,
            Rt64PlatformCase::ALL
                .iter()
                .map(|case| (case.id(), case.repeat_bar()))
                .collect::<Vec<_>>()
        );
    }
}
