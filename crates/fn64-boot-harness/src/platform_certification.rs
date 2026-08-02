//! Opaque authority for actual-host RT64 platform case series.
//!
//! Retained JSON is integrity evidence only. A release matrix can receive
//! target-case credit solely from [`VerifiedRt64PlatformCaseSeries`], which is
//! deliberately not serializable. [`run_rt64_platform_case_series`] is its
//! only production constructor: it owns the repository-selected child, exact
//! repeat bar, watchdog, child-observed source/API identity, semantic output,
//! and binding to one freshly revalidated private matrix series.

#[path = "../../fn64-render-rt64/adapter_source_identity.rs"]
mod rt64_adapter_source_identity;

use crate::{
    ParsedUnsupportedJournal, ReleaseGateReport, ReleaseGraphicsApi, ReleaseHostPlatform,
    ReleaseRendererEvidence, ReleaseWindowsFamily, ReleaseWindowsVersionEvidence,
    VerifiedPrivateReleaseSeries, RELEASE_MATRIX_REPORT_COUNT,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const VERIFIED_RT64_PLATFORM_CASE_AUTHORITY_SCHEMA: &str =
    "fn64.verified-rt64-platform-case-authority.v2";
pub const RT64_PLATFORM_CHILD_IDENTITY_SCHEMA: &str = "fn64.rt64-platform-child-identity.v1";
const PINNED_RT64_SOURCE_ID: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const PINNED_RT64_COMMIT: &str = "f0728a2520d5aa735886240de3fee75cc805f6d6";
const CHILD_IDENTITY_PREFIX: &str = "fn64-rt64-platform-child-identity=";
const CHILD_WATCHDOG: Duration = Duration::from_secs(60);
const GIT_WATCHDOG: Duration = Duration::from_secs(30);
const BUILD_WATCHDOG: Duration = Duration::from_secs(15 * 60);
const RUN_EVENT_DOMAIN: &[u8] = b"fn64.rt64-platform-case-run-event.v1\0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rt64PlatformChildIdentity {
    schema: String,
    rt64_source_id: String,
    source_authoritative: bool,
    adapter_source_sha256: String,
    capture_api: String,
}

/// Emit the machine-readable identity consumed by the crate-owned platform
/// runner. Behavioral examples call this only after all of their assertions
/// pass, so a successful process binds its semantic output to the exact RT64
/// and adapter source observed by the child itself.
pub fn emit_rt64_platform_child_identity(
    rt64_source_id: &str,
    source_authoritative: bool,
    adapter_source_sha256: &str,
    capture_api: &str,
) -> Result<(), PlatformCertificationError> {
    let identity = Rt64PlatformChildIdentity {
        schema: RT64_PLATFORM_CHILD_IDENTITY_SCHEMA.to_owned(),
        rt64_source_id: rt64_source_id.to_owned(),
        source_authoritative,
        adapter_source_sha256: adapter_source_sha256.to_owned(),
        capture_api: capture_api.to_owned(),
    };
    validate_child_identity(&identity, None)?;
    let wire = serde_json::to_string(&identity).map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "serialize RT64 platform child identity: {source}"
        ))
    })?;
    println!("{CHILD_IDENTITY_PREFIX}{wire}");
    Ok(())
}

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

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|target| target.id() == id)
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

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|case| case.id() == id)
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

    const fn example(self) -> &'static str {
        match self {
            Self::BackendLifecycle => "rt64_metal_backend_behavior",
            Self::ResolutionDownsample => "rt64_resolution_downsample_behavior",
            Self::UserControlsRebuild => "rt64_user_controls_rebuild_behavior",
            Self::EnhancementEmulatorControls => "rt64_enhancement_emulator_controls_behavior",
            Self::FramebufferRdramRegion => "rt64_framebuffer_rdram_region_behavior",
            Self::FramebufferEnhancement => "rt64_framebuffer_enhancement_behavior",
            Self::TextureReplacements => "rt64_texture_replacement_behavior",
            Self::LatencySkipBuffering => "rt64_latency_skip_buffering_behavior",
            Self::LatencyPresentEarly => "rt64_latency_present_early_behavior",
            Self::DeferredDebugger => "rt64_deferred_debugger_behavior",
            Self::UbershaderCriticalPath => "rt64_ubershader_pipeline_behavior",
            Self::HfrHleCooperation => "rt64_hfr_interpolation_behavior",
            Self::ExtendedGbiCooperation => "rt64_extended_gbi_enhancement_behavior",
        }
    }

    const fn features(self) -> &'static str {
        match self {
            Self::HfrHleCooperation => "hfr-evidence",
            Self::ExtendedGbiCooperation => "extended-gbi-evidence",
            _ => "rt64",
        }
    }

    const fn rt64_adapter_features(self) -> &'static [&'static str] {
        match self {
            Self::HfrHleCooperation => &["HFR_EVIDENCE", "RT64", "SYNTHETIC_F3DEX2_EVIDENCE"],
            Self::ExtendedGbiCooperation => {
                &["EXTENDED_GBI_EVIDENCE", "RT64", "SYNTHETIC_F3DEX2_EVIDENCE"]
            }
            _ => &["RT64"],
        }
    }

    const fn supports_target(self, target: Rt64PlatformTarget) -> bool {
        let _ = self;
        // The current examples and identity envelope are actual-hardware
        // Metal cases. Other target rows remain in the denominator, but need
        // API-selecting examples before the production runner can admit them.
        matches!(target, Rt64PlatformTarget::MacosMetal)
    }

    /// Reject a target/case request that cannot bind the supplied, already
    /// retained matrix evidence before Cargo builds or a native child runs.
    ///
    /// This creates no platform authority. The opaque capability still comes
    /// only from [`run_rt64_platform_case_series`] after the complete native
    /// repeat bar succeeds.
    pub fn preflight_series_binding(
        self,
        target: Rt64PlatformTarget,
        rt64_source_directory: impl AsRef<Path>,
        bound_series: &VerifiedPrivateReleaseSeries,
        evidence: &[(ReleaseGateReport, ParsedUnsupportedJournal)],
    ) -> Result<PreflightedRt64PlatformCase, PlatformCertificationError> {
        let host = crate::release_host_identity().map_err(|source| {
            PlatformCertificationError::Runner(format!(
                "observe native host identity for RT64 platform preflight: {source}"
            ))
        })?;
        if !target.matches_host(host.0, host.1) {
            return Err(PlatformCertificationError::TargetHostMismatch);
        }
        if !self.supports_target(target) {
            return Err(PlatformCertificationError::Runner(format!(
                "RT64 platform case {} is not enrolled for target {}",
                self.id(),
                target.id()
            )));
        }

        let rt64_source_directory = validate_rt64_source_tree(rt64_source_directory.as_ref())?;
        let binding = bound_matrix_series(bound_series)?;
        let matching = evidence
            .iter()
            .filter(|(report, _)| report.scenario == binding.scenario)
            .cloned()
            .collect::<Vec<_>>();
        let verified =
            crate::verify_release_evidence_series(&matching, RELEASE_MATRIX_REPORT_COUNT).map_err(
                |source| {
                    PlatformCertificationError::Runner(format!(
                        "verify bound matrix evidence for RT64 platform preflight: {source}"
                    ))
                },
            )?;
        if verified.scenario != binding.scenario
            || verified.report_sha256 != binding.report_sha256
            || verified.run_event_sha256s != binding.run_event_sha256s
        {
            return Err(PlatformCertificationError::Runner(
                "RT64 platform request does not bind the exact retained private report series"
                    .to_owned(),
            ));
        }

        let report = matching
            .first()
            .map(|(report, _)| report)
            .expect("a verified positive-count series has a first report");
        let workspace = repository_workspace()?;
        let expected_adapter_source_sha256 = expected_adapter_source_sha256(&workspace, self)?;
        validate_report_binding(
            target,
            host,
            report.environment.platform,
            report.environment.windows_version,
            &report.environment.renderer,
            &expected_adapter_source_sha256,
        )?;
        Ok(PreflightedRt64PlatformCase {
            target,
            case: self,
            host,
            workspace,
            rt64_source_directory,
            rt64_source_id: PINNED_RT64_SOURCE_ID.to_owned(),
            adapter_source_sha256: expected_adapter_source_sha256,
            bound_series: binding,
        })
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
    pub fn64_source_sha256: String,
    pub builder_cargo_sha256: String,
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
            ("fn64_source_sha256", self.fn64_source_sha256.as_str()),
            ("builder_cargo_sha256", self.builder_cargo_sha256.as_str()),
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
        wire.extend_from_slice(b"fn64.verified-rt64-platform-case-authority.v2\0");
        push_bytes(&mut wire, self.schema.as_bytes());
        push_bytes(&mut wire, self.target.id().as_bytes());
        push_bytes(&mut wire, self.case.id().as_bytes());
        push_host(&mut wire, self.platform, self.windows_version);
        wire.push(graphics_api_tag(self.graphics_api));
        push_bytes(&mut wire, self.capture_api.as_bytes());
        push_bytes(&mut wire, self.rt64_source_id.as_bytes());
        push_bytes(&mut wire, self.adapter_source_sha256.as_bytes());
        push_bytes(&mut wire, self.fn64_source_sha256.as_bytes());
        push_bytes(&mut wire, self.builder_cargo_sha256.as_bytes());
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
            fn64_source_sha256: hex(&Sha256::digest([seed, 4])),
            builder_cargo_sha256: hex(&Sha256::digest([seed, 5])),
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
struct BoundMatrixSeries {
    scenario: String,
    report_sha256: String,
    run_event_sha256s: Vec<String>,
}

/// Opaque result of validating one exact platform-case request against the
/// current host, repository sources, pinned RT64 tree, and retained matrix
/// series. Native execution consumes this ticket and rechecks every mutable
/// identity before it builds or launches the case child.
#[derive(Debug)]
pub struct PreflightedRt64PlatformCase {
    target: Rt64PlatformTarget,
    case: Rt64PlatformCase,
    host: (ReleaseHostPlatform, Option<ReleaseWindowsVersionEvidence>),
    workspace: PathBuf,
    rt64_source_directory: PathBuf,
    rt64_source_id: String,
    adapter_source_sha256: String,
    bound_series: BoundMatrixSeries,
}

impl PreflightedRt64PlatformCase {
    fn revalidate_before_native(
        &self,
        bound_series: &VerifiedPrivateReleaseSeries,
    ) -> Result<(), PlatformCertificationError> {
        let host = crate::release_host_identity().map_err(|source| {
            PlatformCertificationError::Runner(format!(
                "observe native host identity for RT64 platform execution: {source}"
            ))
        })?;
        if host != self.host || !self.target.matches_host(host.0, host.1) {
            return Err(PlatformCertificationError::TargetHostMismatch);
        }
        if !self.case.supports_target(self.target) {
            return Err(PlatformCertificationError::Runner(format!(
                "RT64 platform case {} is not enrolled for target {}",
                self.case.id(),
                self.target.id()
            )));
        }
        if repository_workspace()? != self.workspace {
            return Err(PlatformCertificationError::Runner(
                "repository workspace changed after RT64 platform preflight".to_owned(),
            ));
        }
        if self.rt64_source_id != PINNED_RT64_SOURCE_ID
            || validate_rt64_source_tree(&self.rt64_source_directory)? != self.rt64_source_directory
        {
            return Err(PlatformCertificationError::SourceMismatch);
        }
        if bound_matrix_series(bound_series)? != self.bound_series {
            return Err(PlatformCertificationError::Runner(
                "bound private release series changed after RT64 platform preflight".to_owned(),
            ));
        }
        if expected_adapter_source_sha256(&self.workspace, self.case)? != self.adapter_source_sha256
        {
            return Err(PlatformCertificationError::AdapterSourceMismatch);
        }
        Ok(())
    }
}

/// Build and execute one repository-owned RT64 behavior case on the current
/// host, then return the only capability that can grant that target/case
/// release-matrix credit.
///
/// The caller supplies only the opaque preflight ticket and its retained
/// private series, never an executable, command, source label, API label, or
/// semantic digest. This function revalidates the ticket before build and
/// child launch, selects the exact example and features, hashes the built
/// child, launches it directly for the case's full 10/20-run bar, and derives
/// all identities from child output and the freshly revalidated series.
pub fn run_rt64_platform_case_series(
    preflight: PreflightedRt64PlatformCase,
    bound_series: &VerifiedPrivateReleaseSeries,
) -> Result<VerifiedRt64PlatformCaseSeries, PlatformCertificationError> {
    preflight.revalidate_before_native(bound_series)?;
    let target = preflight.target;
    let case = preflight.case;
    let host = preflight.host;
    let workspace = preflight.workspace.clone();
    let rt64_source_directory = preflight.rt64_source_directory.clone();
    let expected_adapter_source_sha256 = preflight.adapter_source_sha256.clone();
    let before = preflight.bound_series.clone();
    let builder_cargo = verified_build_cargo()?;
    let builder_cargo_sha256 = env!("FN64_BUILD_CARGO_SHA256").to_owned();
    let fn64_source_sha256 = certification_source_sha256(&workspace, case)?;
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce).map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "obtain OS-random RT64 platform series nonce: {source}"
        ))
    })?;
    let mut scratch = create_scratch_directory(&nonce)?;
    let child = match build_case_child(
        &workspace,
        &rt64_source_directory,
        &builder_cargo,
        case,
        scratch.path(),
    ) {
        Ok(child) => child,
        Err(source) => {
            scratch.preserve();
            return Err(PlatformCertificationError::Runner(format!(
                "{source}; preserved build logs in {}",
                scratch.path().display()
            )));
        }
    };
    verified_build_cargo()?;
    let child_executable_sha256 = sha256_file(&child, "RT64 platform case executable")?;
    let staged_child = stage_case_child(&child, scratch.path(), &child_executable_sha256)?;
    if let Err(source) = preflight.revalidate_before_native(bound_series) {
        scratch.preserve();
        return Err(PlatformCertificationError::Runner(format!(
            "revalidate RT64 platform preflight before child launch: {source}; preserved build logs in {}",
            scratch.path().display()
        )));
    }
    let run_result = run_case_children(
        &workspace,
        &staged_child,
        target,
        case,
        &before,
        &child_executable_sha256,
        &expected_adapter_source_sha256,
        &nonce,
        scratch.path(),
    );
    let (identity, semantic_evidence_sha256, run_event_sha256s) = match run_result {
        Ok(result) => result,
        Err(source) => {
            scratch.preserve();
            return Err(PlatformCertificationError::Runner(format!(
                "{source}; preserved child logs in {}",
                scratch.path().display()
            )));
        }
    };

    let after = bound_matrix_series(bound_series)?;
    if before != after {
        return Err(PlatformCertificationError::Runner(
            "bound private release series changed while the RT64 platform case ran".to_owned(),
        ));
    }
    validate_rt64_source_tree(&rt64_source_directory)?;
    if certification_source_sha256(&workspace, case)? != fn64_source_sha256 {
        return Err(PlatformCertificationError::Runner(
            "fn64 platform certification sources changed while the case series ran".to_owned(),
        ));
    }
    verified_build_cargo()?;
    if sha256_file(
        &staged_child,
        "staged RT64 platform case executable after series",
    )? != child_executable_sha256
    {
        return Err(PlatformCertificationError::Runner(
            "RT64 platform case executable changed while its series ran".to_owned(),
        ));
    }

    let mut authority = VerifiedRt64PlatformCaseAuthority {
        schema: VERIFIED_RT64_PLATFORM_CASE_AUTHORITY_SCHEMA.to_owned(),
        target,
        case,
        platform: host.0,
        windows_version: host.1,
        graphics_api: target.graphics_api(),
        capture_api: target.capture_api().to_owned(),
        rt64_source_id: identity.rt64_source_id,
        adapter_source_sha256: identity.adapter_source_sha256,
        fn64_source_sha256,
        builder_cargo_sha256,
        child_executable_sha256,
        bound_report_scenario: before.scenario,
        bound_report_sha256: before.report_sha256,
        bound_matrix_run_event_sha256s: before.run_event_sha256s,
        semantic_evidence_sha256,
        run_event_sha256s,
        authority_sha256: String::new(),
    };
    authority.authority_sha256 = authority.recompute_authority_sha256();
    authority.verify_integrity()?;
    scratch.finish()?;
    Ok(VerifiedRt64PlatformCaseSeries { authority })
}

fn repository_workspace() -> Result<PathBuf, PlatformCertificationError> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|source| {
            PlatformCertificationError::Runner(format!(
                "resolve repository workspace for RT64 platform runner: {source}"
            ))
        })
}

fn expected_adapter_source_sha256(
    workspace: &Path,
    case: Rt64PlatformCase,
) -> Result<String, PlatformCertificationError> {
    let adapter_features = case
        .rt64_adapter_features()
        .iter()
        .map(|feature| (*feature).to_owned())
        .collect::<Vec<_>>();
    rt64_adapter_source_identity::adapter_source_sha256(
        &workspace.join("crates/fn64-render-rt64"),
        env!("FN64_BUILD_TARGET"),
        &adapter_features,
    )
    .map(|sha256| hex(&sha256))
    .map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "compute expected RT64 adapter source identity: {source}"
        ))
    })
}

fn validate_report_binding(
    target: Rt64PlatformTarget,
    expected_host: (ReleaseHostPlatform, Option<ReleaseWindowsVersionEvidence>),
    platform: ReleaseHostPlatform,
    windows_version: Option<ReleaseWindowsVersionEvidence>,
    renderer: &ReleaseRendererEvidence,
    expected_adapter_source_sha256: &str,
) -> Result<(), PlatformCertificationError> {
    if (platform, windows_version) != expected_host
        || !target.matches_host(platform, windows_version)
    {
        return Err(PlatformCertificationError::TargetHostMismatch);
    }
    let ReleaseRendererEvidence::Rt64 {
        graphics_api,
        backend_identity,
        source_authoritative: true,
        ..
    } = renderer
    else {
        return Err(PlatformCertificationError::Runner(
            "RT64 platform request is bound to a report without authoritative RT64 renderer evidence"
                .to_owned(),
        ));
    };
    if *graphics_api != target.graphics_api()
        || !backend_identity.ends_with(&format!("post_vi_api={}", target.capture_api()))
    {
        return Err(PlatformCertificationError::TargetApiMismatch);
    }
    if !backend_identity.contains(&format!(";source={PINNED_RT64_SOURCE_ID};")) {
        return Err(PlatformCertificationError::SourceMismatch);
    }
    if !backend_identity.contains(&format!(
        ";adapter_sha256={expected_adapter_source_sha256};"
    )) {
        return Err(PlatformCertificationError::AdapterSourceMismatch);
    }
    Ok(())
}

fn bound_matrix_series(
    series: &VerifiedPrivateReleaseSeries,
) -> Result<BoundMatrixSeries, PlatformCertificationError> {
    let verified = series.revalidate_for_release_matrix().map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "revalidate private release series for RT64 case binding: {source}"
        ))
    })?;
    Ok(BoundMatrixSeries {
        scenario: verified.contract.report_scenario,
        report_sha256: verified.receipt.semantic_report_sha256,
        run_event_sha256s: verified
            .receipt
            .runs
            .into_iter()
            .map(|run| run.run_event_sha256)
            .collect(),
    })
}

fn validate_rt64_source_tree(path: &Path) -> Result<PathBuf, PlatformCertificationError> {
    if !path.is_absolute() {
        return Err(PlatformCertificationError::Runner(
            "RT64 platform runner requires an absolute RT64 source directory".to_owned(),
        ));
    }
    let canonical = path.canonicalize().map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "resolve RT64 source directory {}: {source}",
            path.display()
        ))
    })?;
    if canonical != path {
        return Err(PlatformCertificationError::Runner(format!(
            "RT64 source directory must already be canonical: supplied={}, canonical={}",
            path.display(),
            canonical.display()
        )));
    }
    let head = command_stdout(
        Command::new("git")
            .arg("-C")
            .arg(&canonical)
            .args(["rev-parse", "HEAD"]),
        "read RT64 source HEAD",
    )?;
    if head.trim() != PINNED_RT64_COMMIT {
        return Err(PlatformCertificationError::SourceMismatch);
    }
    let dirty = command_stdout(
        Command::new("git").arg("-C").arg(&canonical).args([
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ]),
        "inspect RT64 source cleanliness",
    )?;
    if !dirty.trim().is_empty() {
        return Err(PlatformCertificationError::Runner(format!(
            "RT64 source tree is dirty: {}",
            dirty.lines().next().unwrap_or("<unknown change>")
        )));
    }
    Ok(canonical)
}

fn command_stdout(
    command: &mut Command,
    action: &str,
) -> Result<String, PlatformCertificationError> {
    let output = command_output_with_watchdog(command, GIT_WATCHDOG)
        .map_err(|source| PlatformCertificationError::Runner(format!("{action}: {source}")))?;
    if !output.status.success() {
        return Err(PlatformCertificationError::Runner(format!(
            "{action} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout).map_err(|source| {
        PlatformCertificationError::Runner(format!("{action} returned non-UTF-8 output: {source}"))
    })
}

fn command_output_with_watchdog(
    command: &mut Command,
    timeout: Duration,
) -> std::io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .expect("piped command stdout is available");
    let mut stderr = child
        .stderr
        .take()
        .expect("piped command stderr is available");
    thread::scope(|scope| {
        let stdout_reader = scope.spawn(|| {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let stderr_reader = scope.spawn(|| {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        let status = wait_with_watchdog(&mut child, timeout);
        let stdout = stdout_reader
            .join()
            .map_err(|_| std::io::Error::other("command stdout reader panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| std::io::Error::other("command stderr reader panicked"))??;
        status.map(|status| Output {
            status,
            stdout,
            stderr,
        })
    })
}

pub(crate) fn verified_build_cargo() -> Result<PathBuf, PlatformCertificationError> {
    let invocation = PathBuf::from(env!("FN64_BUILD_CARGO_PATH"));
    let expected_canonical = Path::new(env!("FN64_BUILD_CARGO_CANONICAL_PATH"));
    let canonical = invocation.canonicalize().map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "resolve verifier-owned Cargo executable {}: {source}",
            invocation.display()
        ))
    })?;
    if canonical != expected_canonical {
        return Err(PlatformCertificationError::Runner(format!(
            "verifier-owned Cargo path changed: expected={}, observed={}",
            expected_canonical.display(),
            canonical.display()
        )));
    }
    let observed = sha256_file(&canonical, "verifier-owned Cargo executable")?;
    if observed != env!("FN64_BUILD_CARGO_SHA256") {
        return Err(PlatformCertificationError::Runner(format!(
            "verifier-owned Cargo executable identity changed: expected={}, observed={observed}",
            env!("FN64_BUILD_CARGO_SHA256")
        )));
    }
    Ok(invocation)
}

fn certification_source_sha256(
    workspace: &Path,
    case: Rt64PlatformCase,
) -> Result<String, PlatformCertificationError> {
    let mut files = vec![workspace.join("Cargo.lock")];
    for relative in [
        "crates/fn64-boot-harness",
        "crates/fn64-certification",
        "crates/fn64-runtime",
    ] {
        collect_source_files(workspace, &workspace.join(relative), &mut files)?;
    }
    files.sort();
    files.dedup();
    let mut digest = Sha256::new();
    digest.update(b"fn64.rt64-platform-certification-source.v1\0");
    push_bytes_digest(&mut digest, case.id().as_bytes());
    push_bytes_digest(&mut digest, case.features().as_bytes());
    for path in files {
        let relative = path.strip_prefix(workspace).map_err(|_| {
            PlatformCertificationError::Runner(format!(
                "platform certification source escaped workspace: {}",
                path.display()
            ))
        })?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            PlatformCertificationError::Runner(format!(
                "inspect platform certification source {}: {source}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(PlatformCertificationError::Runner(format!(
                "platform certification source must be a regular non-symlink file: {}",
                path.display()
            )));
        }
        let bytes = fs::read(&path).map_err(|source| {
            PlatformCertificationError::Runner(format!(
                "read platform certification source {}: {source}",
                path.display()
            ))
        })?;
        push_bytes_digest(
            &mut digest,
            relative.to_string_lossy().replace('\\', "/").as_bytes(),
        );
        push_bytes_digest(&mut digest, &bytes);
    }
    Ok(hex(&digest.finalize()))
}

fn collect_source_files(
    workspace: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), PlatformCertificationError> {
    for entry in fs::read_dir(directory).map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "enumerate platform certification source {}: {source}",
            directory.display()
        ))
    })? {
        let path = entry
            .map_err(|source| {
                PlatformCertificationError::Runner(format!(
                    "enumerate platform certification source {}: {source}",
                    directory.display()
                ))
            })?
            .path();
        let relative = path.strip_prefix(workspace).map_err(|_| {
            PlatformCertificationError::Runner(format!(
                "platform certification source escaped workspace: {}",
                path.display()
            ))
        })?;
        if relative
            .components()
            .any(|component| component.as_os_str() == OsStr::new("target"))
        {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            PlatformCertificationError::Runner(format!(
                "inspect platform certification source {}: {source}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(PlatformCertificationError::Runner(format!(
                "platform certification source tree contains symlink {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            collect_source_files(workspace, &path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn push_bytes_digest(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

fn build_case_child(
    workspace: &Path,
    rt64_source_directory: &Path,
    cargo: &Path,
    case: Rt64PlatformCase,
    scratch: &Path,
) -> Result<PathBuf, PlatformCertificationError> {
    let mut command = Command::new(cargo);
    command
        .current_dir(workspace)
        .arg("build")
        .arg("--frozen")
        .arg("--target-dir")
        .arg(scratch.join("build-target"))
        .arg("-p")
        .arg("fn64-certification")
        .arg("--features")
        .arg(case.features())
        .arg("--example")
        .arg(case.example())
        .arg("--message-format=json-render-diagnostics")
        .env("FN64_RT64_DIR", rt64_source_directory)
        .env_remove("CARGO")
        .env_remove("RUSTC")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CC")
        .env_remove("CXX")
        .env_remove("AR");
    let stdout_path = scratch.join("cargo-build.stdout.jsonl");
    let stderr_path = scratch.join("cargo-build.stderr.log");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(create_new_file(&stdout_path).map_err(
            |source| {
                PlatformCertificationError::Runner(format!(
                    "create Cargo output {}: {source}",
                    stdout_path.display()
                ))
            },
        )?))
        .stderr(Stdio::from(create_new_file(&stderr_path).map_err(
            |source| {
                PlatformCertificationError::Runner(format!(
                    "create Cargo error log {}: {source}",
                    stderr_path.display()
                ))
            },
        )?));
    let mut process = command.spawn().map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "spawn Cargo build for RT64 case {}: {source}",
            case.id()
        ))
    })?;
    let status = wait_with_watchdog(&mut process, BUILD_WATCHDOG).map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "wait for Cargo build of RT64 case {}: {source}",
            case.id()
        ))
    })?;
    let stdout = fs::read(&stdout_path).map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "read Cargo output {}: {source}",
            stdout_path.display()
        ))
    })?;
    let stderr = fs::read(&stderr_path).map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "read Cargo error log {}: {source}",
            stderr_path.display()
        ))
    })?;
    if !status.success() {
        return Err(PlatformCertificationError::Runner(format!(
            "build RT64 case {} exited {}:\n{}",
            case.id(),
            status,
            String::from_utf8_lossy(&stderr)
        )));
    }
    let stdout = String::from_utf8(stdout).map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "cargo build output for RT64 case {} is not UTF-8: {source}",
            case.id()
        ))
    })?;
    let mut executable = None;
    for line in stdout.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] == "compiler-artifact"
            && message["target"]["name"] == case.example()
            && message["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "example"))
        {
            let raw = message["executable"].as_str().ok_or_else(|| {
                PlatformCertificationError::Runner(format!(
                    "cargo artifact for RT64 case {} has no executable",
                    case.id()
                ))
            })?;
            if executable.replace(PathBuf::from(raw)).is_some() {
                return Err(PlatformCertificationError::Runner(format!(
                    "cargo emitted multiple executable artifacts for RT64 case {}",
                    case.id()
                )));
            }
        }
    }
    let executable = executable.ok_or_else(|| {
        PlatformCertificationError::Runner(format!(
            "cargo emitted no executable artifact for RT64 case {}",
            case.id()
        ))
    })?;
    let canonical = executable.canonicalize().map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "resolve built RT64 case executable {}: {source}",
            executable.display()
        ))
    })?;
    if !canonical.is_file() {
        return Err(PlatformCertificationError::Runner(format!(
            "built RT64 case artifact is not a regular file: {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

type ChildSeriesResult = (Rt64PlatformChildIdentity, String, Vec<String>);

#[allow(clippy::too_many_arguments)]
fn run_case_children(
    workspace: &Path,
    child: &Path,
    target: Rt64PlatformTarget,
    case: Rt64PlatformCase,
    binding: &BoundMatrixSeries,
    child_executable_sha256: &str,
    expected_adapter_source_sha256: &str,
    nonce: &[u8; 32],
    scratch: &Path,
) -> Result<ChildSeriesResult, String> {
    let mut expected_identity = None;
    let mut expected_stdout = None;
    let mut run_events = Vec::with_capacity(case.repeat_bar());
    for index in 0..case.repeat_bar() {
        let ordinal = u64::try_from(index + 1).expect("platform repeat bar fits u64");
        let stdout_path = scratch.join(format!("run-{ordinal:02}.stdout.log"));
        let stderr_path = scratch.join(format!("run-{ordinal:02}.stderr.log"));
        let stdout_file = create_new_file(&stdout_path)
            .map_err(|source| format!("create {}: {source}", stdout_path.display()))?;
        let stderr_file = create_new_file(&stderr_path)
            .map_err(|source| format!("create {}: {source}", stderr_path.display()))?;
        let mut command = Command::new(child);
        command
            .current_dir(workspace)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file));
        let mut process = command
            .spawn()
            .map_err(|source| format!("spawn case child {ordinal}: {source}"))?;
        let status = wait_with_watchdog(&mut process, CHILD_WATCHDOG)
            .map_err(|source| format!("wait for case child {ordinal}: {source}"))?;
        if !status.success() {
            return Err(format!(
                "RT64 case {} child {ordinal} exited {status}",
                case.id()
            ));
        }
        let stdout = fs::read(&stdout_path)
            .map_err(|source| format!("read {}: {source}", stdout_path.display()))?;
        let identity = parse_child_identity(&stdout, target, expected_adapter_source_sha256)
            .map_err(|source| format!("RT64 case child {ordinal}: {source}"))?;
        if let Some(expected) = &expected_identity {
            if expected != &identity {
                return Err(format!(
                    "RT64 case {} child {ordinal} reported a different source identity",
                    case.id()
                ));
            }
        } else {
            expected_identity = Some(identity);
        }
        if let Some(expected) = &expected_stdout {
            if expected != &stdout {
                return Err(format!(
                    "RT64 case {} child {ordinal} produced nondeterministic semantic output",
                    case.id()
                ));
            }
        } else {
            expected_stdout = Some(stdout);
        }
        run_events.push(derive_case_run_event(
            nonce,
            target,
            case,
            child_executable_sha256,
            &binding.report_sha256,
            ordinal,
        ));
    }
    let stdout = expected_stdout.expect("positive repeat bar retains stdout");
    Ok((
        expected_identity.expect("positive repeat bar retains identity"),
        hex(&Sha256::digest(stdout)),
        run_events,
    ))
}

struct ScratchDirectory {
    path: PathBuf,
    preserve: bool,
}

impl ScratchDirectory {
    fn path(&self) -> &Path {
        &self.path
    }

    fn preserve(&mut self) {
        self.preserve = true;
    }

    fn finish(mut self) -> Result<(), PlatformCertificationError> {
        self.preserve = true;
        fs::remove_dir_all(&self.path).map_err(|source| {
            PlatformCertificationError::Runner(format!(
                "remove successful RT64 platform scratch {}: {source}",
                self.path.display()
            ))
        })
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn create_scratch_directory(
    nonce: &[u8; 32],
) -> Result<ScratchDirectory, PlatformCertificationError> {
    let path = std::env::temp_dir().join(format!("fn64-rt64-platform-{}", hex(nonce)));
    fs::create_dir(&path).map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "create RT64 platform scratch {}: {source}",
            path.display()
        ))
    })?;
    Ok(ScratchDirectory {
        path,
        preserve: false,
    })
}

fn stage_case_child(
    source: &Path,
    scratch: &Path,
    expected_sha256: &str,
) -> Result<PathBuf, PlatformCertificationError> {
    let staged = scratch.join(
        source
            .file_name()
            .unwrap_or_else(|| OsStr::new("rt64-platform-case-child")),
    );
    let mut input = File::open(source).map_err(|source_error| {
        PlatformCertificationError::Runner(format!(
            "open RT64 platform case executable {} for staging: {source_error}",
            source.display()
        ))
    })?;
    let mut output = create_new_file(&staged).map_err(|source_error| {
        PlatformCertificationError::Runner(format!(
            "create staged RT64 platform case executable {}: {source_error}",
            staged.display()
        ))
    })?;
    std::io::copy(&mut input, &mut output).map_err(|source_error| {
        PlatformCertificationError::Runner(format!(
            "copy RT64 platform case executable to {}: {source_error}",
            staged.display()
        ))
    })?;
    output.sync_all().map_err(|source_error| {
        PlatformCertificationError::Runner(format!(
            "sync staged RT64 platform case executable {}: {source_error}",
            staged.display()
        ))
    })?;
    drop(output);
    fs::set_permissions(
        &staged,
        fs::metadata(source)
            .map_err(|source_error| {
                PlatformCertificationError::Runner(format!(
                    "read permissions from RT64 platform case executable {}: {source_error}",
                    source.display()
                ))
            })?
            .permissions(),
    )
    .map_err(|source_error| {
        PlatformCertificationError::Runner(format!(
            "set staged RT64 platform case executable permissions {}: {source_error}",
            staged.display()
        ))
    })?;
    if sha256_file(&staged, "staged RT64 platform case executable")? != expected_sha256 {
        return Err(PlatformCertificationError::Runner(
            "staged RT64 platform case executable differs from Cargo's exact artifact".to_owned(),
        ));
    }
    Ok(staged)
}

fn create_new_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

fn wait_with_watchdog(
    child: &mut std::process::Child,
    timeout: Duration,
) -> std::io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(source) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(source);
            }
        }
        if Instant::now() >= deadline {
            let kill = child.kill();
            let reap = child.wait();
            if let Err(source) = kill {
                return Err(std::io::Error::new(
                    source.kind(),
                    format!(
                        "child exceeded {} second watchdog and kill failed: {source}; reap={reap:?}",
                        timeout.as_secs()
                    ),
                ));
            }
            reap?;
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("child exceeded {} second watchdog", timeout.as_secs()),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn parse_child_identity(
    stdout: &[u8],
    target: Rt64PlatformTarget,
    expected_adapter_source_sha256: &str,
) -> Result<Rt64PlatformChildIdentity, PlatformCertificationError> {
    let stdout = std::str::from_utf8(stdout).map_err(|source| {
        PlatformCertificationError::Runner(format!(
            "RT64 platform child stdout is not UTF-8: {source}"
        ))
    })?;
    let mut identities = stdout
        .lines()
        .filter_map(|line| line.strip_prefix(CHILD_IDENTITY_PREFIX))
        .map(|wire| {
            serde_json::from_str::<Rt64PlatformChildIdentity>(wire).map_err(|source| {
                PlatformCertificationError::Runner(format!(
                    "parse RT64 platform child identity: {source}"
                ))
            })
        });
    let identity = identities.next().ok_or_else(|| {
        PlatformCertificationError::Runner(
            "RT64 platform child emitted no identity envelope".to_owned(),
        )
    })??;
    if identities.next().is_some() {
        return Err(PlatformCertificationError::Runner(
            "RT64 platform child emitted multiple identity envelopes".to_owned(),
        ));
    }
    validate_child_identity(&identity, Some((target, expected_adapter_source_sha256)))?;
    Ok(identity)
}

fn validate_child_identity(
    identity: &Rt64PlatformChildIdentity,
    expected: Option<(Rt64PlatformTarget, &str)>,
) -> Result<(), PlatformCertificationError> {
    if identity.schema != RT64_PLATFORM_CHILD_IDENTITY_SCHEMA {
        return Err(PlatformCertificationError::UnsupportedSchema(
            identity.schema.clone(),
        ));
    }
    if identity.rt64_source_id != PINNED_RT64_SOURCE_ID || !identity.source_authoritative {
        return Err(PlatformCertificationError::SourceMismatch);
    }
    if !canonical_sha256(&identity.adapter_source_sha256) {
        return Err(PlatformCertificationError::InvalidSha256(
            "adapter_source_sha256",
        ));
    }
    if let Some((target, expected_adapter_source_sha256)) = expected {
        if identity.capture_api != target.capture_api() {
            return Err(PlatformCertificationError::TargetApiMismatch);
        }
        if identity.adapter_source_sha256 != expected_adapter_source_sha256 {
            return Err(PlatformCertificationError::AdapterSourceMismatch);
        }
    } else if !Rt64PlatformTarget::ALL
        .iter()
        .any(|target| target.capture_api() == identity.capture_api)
    {
        return Err(PlatformCertificationError::TargetApiMismatch);
    }
    Ok(())
}

fn derive_case_run_event(
    nonce: &[u8; 32],
    target: Rt64PlatformTarget,
    case: Rt64PlatformCase,
    child_executable_sha256: &str,
    bound_report_sha256: &str,
    ordinal: u64,
) -> String {
    let mut wire = Vec::new();
    wire.extend_from_slice(RUN_EVENT_DOMAIN);
    wire.extend_from_slice(nonce);
    push_bytes(&mut wire, target.id().as_bytes());
    push_bytes(&mut wire, case.id().as_bytes());
    push_bytes(&mut wire, child_executable_sha256.as_bytes());
    push_bytes(&mut wire, bound_report_sha256.as_bytes());
    wire.extend_from_slice(&ordinal.to_be_bytes());
    hex(&Sha256::digest(wire))
}

fn sha256_file(path: &Path, field: &str) -> Result<String, PlatformCertificationError> {
    let mut file = File::open(path).map_err(|source| {
        PlatformCertificationError::Runner(format!("open {field} {}: {source}", path.display()))
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| {
            PlatformCertificationError::Runner(format!("read {field} {}: {source}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformCertificationError {
    UnsupportedSchema(String),
    TargetHostMismatch,
    TargetApiMismatch,
    AdapterSourceMismatch,
    SourceMismatch,
    InvalidSha256(&'static str),
    EmptyBoundReportScenario,
    WrongRunCount { expected: usize, observed: usize },
    DuplicateRunEvent(String),
    AuthorityDigestMismatch { stored: String, recomputed: String },
    Runner(String),
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
            Self::AdapterSourceMismatch => {
                write!(formatter, "RT64 platform child mismatches adapter source")
            }
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
            Self::Runner(detail) => write!(formatter, "RT64 platform runner failed: {detail}"),
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
mod tests;
