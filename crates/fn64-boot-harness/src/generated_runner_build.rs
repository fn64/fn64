//! Verifier-owned build authority for the repository's generated WM runner.
//!
//! The source attestation emitted by `fn64-recomp-rs` is intentionally not
//! authority: safe Rust cannot recover a function body's source from a
//! function pointer. This module closes that outer relation by owning one
//! frozen Cargo build, selecting the exact compiler artifact, launching only
//! that artifact's fixed identity mode, and retaining the result in a
//! move-only, non-serializable capability.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V2: &str =
    "fn64.generated-runner-build-identity.v2";
pub const VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V2: &str =
    "fn64.verified-generated-runner-build.v2";
pub const GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V3: &str =
    "fn64.generated-runner-build-identity.v3";
pub const VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V3: &str =
    "fn64.verified-generated-runner-build.v3";
pub const VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V4: &str =
    "fn64.verified-generated-runner-build.v4";
pub const VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V5: &str =
    "fn64.verified-generated-runner-build.v5";
pub const GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-bootstrap-runtime-report.v1";
pub const GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-bootstrap-runtime-report=";
pub const GENERATED_RUNNER_BOOTSTRAP_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-bootstrap-audit-v1";
pub const GENERATED_RUNNER_BOOTSTRAP_RUNTIME_NONCE_ENV_V1: &str =
    "FN64_GENERATED_RUNNER_BOOTSTRAP_NONCE";
pub const VERIFIED_GENERATED_RUNNER_BOOTSTRAP_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-bootstrap-series.v1";
pub const GENERATED_RUNNER_CPU_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-cpu-runtime-report.v1";
pub const GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-cpu-runtime-report=";
pub const GENERATED_RUNNER_CPU_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-cpu-audit-v1";
pub const GENERATED_RUNNER_CPU_RUNTIME_NONCE_ENV_V1: &str = "FN64_GENERATED_RUNNER_CPU_NONCE";
pub const VERIFIED_GENERATED_RUNNER_CPU_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-cpu-series.v1";
pub const GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-host-abi-runtime-report.v1";
pub const GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-host-abi-runtime-report=";
pub const GENERATED_RUNNER_HOST_ABI_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-host-abi-audit-v1";
pub const GENERATED_RUNNER_HOST_ABI_RUNTIME_NONCE_ENV_V1: &str =
    "FN64_GENERATED_RUNNER_HOST_ABI_NONCE";
pub const VERIFIED_GENERATED_RUNNER_HOST_ABI_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-host-abi-series.v1";
pub const GENERATED_RUNNER_PI_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-pi-runtime-report.v1";
pub const GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-pi-runtime-report=";
pub const GENERATED_RUNNER_PI_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-pi-audit-v1";
pub const GENERATED_RUNNER_PI_RUNTIME_NONCE_ENV_V1: &str = "FN64_GENERATED_RUNNER_PI_NONCE";
pub const VERIFIED_GENERATED_RUNNER_PI_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-pi-series.v1";
pub const GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-rdp-renderer-runtime-report.v1";
pub const GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-rdp-renderer-runtime-report=";
pub const GENERATED_RUNNER_RDP_RENDERER_RUNTIME_ARGUMENT_V1: &str =
    "--fn64-run-rdp-renderer-audit-v1";
pub const GENERATED_RUNNER_RDP_RENDERER_RUNTIME_NONCE_ENV_V1: &str =
    "FN64_GENERATED_RUNNER_RDP_RENDERER_NONCE";
pub const VERIFIED_GENERATED_RUNNER_RDP_RENDERER_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-rdp-renderer-series.v1";
pub const GENERATED_RUNNER_RSP_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-rsp-runtime-report.v1";
pub const GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-rsp-runtime-report=";
pub const GENERATED_RUNNER_RSP_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-rsp-audit-v1";
pub const GENERATED_RUNNER_RSP_RUNTIME_NONCE_ENV_V1: &str = "FN64_GENERATED_RUNNER_RSP_NONCE";
pub const VERIFIED_GENERATED_RUNNER_RSP_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-rsp-series.v1";
pub const VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-writer-audit-bundle.v1";
pub const GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1: &str = "fn64-generated-runner-build-identity=";
pub const GENERATED_RUNNER_BUILD_IDENTITY_ARGUMENT_V1: &str =
    "--fn64-emit-generated-runner-build-identity-v1";
pub const GENERATED_RUNNER_SI_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-si-runtime-report.v1";
pub const GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-si-runtime-report=";
pub const GENERATED_RUNNER_SI_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-si-audit-v1";
pub const GENERATED_RUNNER_SI_RUNTIME_NONCE_ENV_V1: &str = "FN64_GENERATED_RUNNER_SI_NONCE";
pub const VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-si-series.v1";
pub const GENERATED_RUNNER_SP_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-sp-runtime-report.v1";
pub const GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-sp-runtime-report=";
pub const GENERATED_RUNNER_SP_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-sp-audit-v1";
pub const GENERATED_RUNNER_SP_RUNTIME_NONCE_ENV_V1: &str = "FN64_GENERATED_RUNNER_SP_NONCE";
pub const VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-sp-series.v1";

const PACKAGE: &str = "wm2000-block-boot";
const PRODUCER_PACKAGE: &str = "fn64-wm-prepared-shard-producer";
const SELECTED_BUILD_CARGO_JOBS_V5: u16 = 2;
const PREPARED_ROOT_ENV: &str = "FN64_WM_PREPARED_SHARD_ROOT";
const PREPARED_MANIFEST_NAME: &str = "manifest.v2";
const PREPARED_UPDATE_MARKER_NAME: &str = ".update.v2";
const PREPARED_SOURCE_MODE_INACTIVE_V1: &str = "legacy_with_prepared_candidate";
const PREPARED_SOURCE_MODE_CONSUMED_V1: &str = "prepared_consumed";
const PREPARED_PACKAGES: [&str; 35] = [
    "wm2000-block-overlay-0-shard-00",
    "wm2000-block-overlay-0-shard-01",
    "wm2000-block-overlay-0-shard-02",
    "wm2000-block-overlay-1-shard-00",
    "wm2000-block-overlay-2-shard-00",
    "wm2000-block-overlay-2-shard-01",
    "wm2000-block-overlay-2-shard-02",
    "wm2000-block-overlay-2-shard-03",
    "wm2000-block-overlay-2-shard-04",
    "wm2000-block-overlay-2-shard-05",
    "wm2000-block-overlay-3-shard-00",
    "wm2000-block-overlay-3-shard-01",
    "wm2000-block-overlay-3-shard-02",
    "wm2000-block-overlay-3-shard-03",
    "wm2000-block-overlay-3-shard-04",
    "wm2000-block-overlay-3-shard-05",
    "wm2000-block-overlay-3-shard-06",
    "wm2000-block-overlay-3-shard-07",
    "wm2000-block-resident-tail-shard-00",
    "wm2000-block-resident-tail-shard-01",
    "wm2000-block-shard-00",
    "wm2000-block-shard-01",
    "wm2000-block-shard-02",
    "wm2000-block-shard-03",
    "wm2000-block-shard-04",
    "wm2000-block-shard-05",
    "wm2000-block-shard-06",
    "wm2000-block-shard-07",
    "wm2000-block-shard-08",
    "wm2000-block-shard-09",
    "wm2000-block-shard-10",
    "wm2000-block-shard-11",
    "wm2000-block-shard-12",
    "wm2000-block-shard-13",
    "wm2000-block-shard-14",
];
const SHARD_MANIFEST_DIRS: [&str; 35] = [
    "overlay0-shard00",
    "overlay0-shard01",
    "overlay0-shard02",
    "overlay1-shard00",
    "overlay2-shard00",
    "overlay2-shard01",
    "overlay2-shard02",
    "overlay2-shard03",
    "overlay2-shard04",
    "overlay2-shard05",
    "overlay3-shard00",
    "overlay3-shard01",
    "overlay3-shard02",
    "overlay3-shard03",
    "overlay3-shard04",
    "overlay3-shard05",
    "overlay3-shard06",
    "overlay3-shard07",
    "shard15",
    "shard16",
    "shard00",
    "shard01",
    "shard02",
    "shard03",
    "shard04",
    "shard05",
    "shard06",
    "shard07",
    "shard08",
    "shard09",
    "shard10",
    "shard11",
    "shard12",
    "shard13",
    "shard14",
];
const IDENTITY_WATCHDOG: Duration = Duration::from_secs(60);
const WRITER_RUNTIME_WATCHDOG: Duration = Duration::from_secs(10 * 60);
// The selected WM Bootstrap path emitted 8,214,477 bytes of ordinary runtime
// diagnostics before its report. Keep transport finite with one source-bound
// ceiling while extracting only the single authority-bearing envelope.
const WRITER_RUNTIME_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const WRITER_RUNTIME_REPORT_LIMIT: usize = 1024 * 1024;
const WRITER_RUNTIME_DIAGNOSTIC_TAIL_LIMIT: usize = 4096;
const SI_RUNTIME_SERIES_RUNS: usize = 10;
const SP_RUNTIME_SERIES_RUNS: usize = 10;
const BOOTSTRAP_RUNTIME_SERIES_RUNS: usize = 10;
const CPU_RUNTIME_SERIES_RUNS: usize = 10;
const HOST_ABI_RUNTIME_SERIES_RUNS: usize = 10;
const PI_RUNTIME_SERIES_RUNS: usize = 10;
const RDP_RENDERER_RUNTIME_SERIES_RUNS: usize = 10;
const RSP_RUNTIME_SERIES_RUNS: usize = 10;
pub const WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1: u8 = 1 << 0;
pub const WRITER_AUDIT_SI_COMPLETED_V1: u8 = 1 << 1;
pub const WRITER_AUDIT_SP_COMPLETED_V1: u8 = 1 << 2;
pub const WRITER_AUDIT_CPU_COMPLETED_V1: u8 = 1 << 3;
pub const WRITER_AUDIT_PI_COMPLETED_V1: u8 = 1 << 4;
pub const WRITER_AUDIT_HOST_ABI_COMPLETED_V1: u8 = 1 << 5;
pub const WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1: u8 = 1 << 6;
pub const WRITER_AUDIT_RSP_COMPLETED_V1: u8 = 1 << 7;
const WRITER_AUDIT_COMPLETED_MASK_V1: u8 = WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1
    | WRITER_AUDIT_SI_COMPLETED_V1
    | WRITER_AUDIT_SP_COMPLETED_V1
    | WRITER_AUDIT_CPU_COMPLETED_V1
    | WRITER_AUDIT_PI_COMPLETED_V1
    | WRITER_AUDIT_HOST_ABI_COMPLETED_V1
    | WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
    | WRITER_AUDIT_RSP_COMPLETED_V1;
const BUILD_MAX_RSS_MIB: u32 = 4096;
const BUILD_MIN_FREE_PERCENT: u8 = 40;
const MIN_BUILD_TIMEOUT_SECONDS: u64 = 40 * 60;
const MAX_BUILD_TIMEOUT_SECONDS: u64 = 2 * 60 * 60;
const MEMORY_GUARD_SOURCE: &[u8] = include_bytes!("../../../scripts/memory-guard.zsh");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GeneratedRunnerAdapterRoleV1 {
    DirectGenerated,
    EntryContextGate,
    DenseInstrumentationGate,
    OverlayGenerationGate,
    ExternalDigestGate,
}

impl GeneratedRunnerAdapterRoleV1 {
    const fn tag(self) -> u8 {
        match self {
            Self::DirectGenerated => 0,
            Self::EntryContextGate => 1,
            Self::DenseInstrumentationGate => 2,
            Self::OverlayGenerationGate => 3,
            Self::ExternalDigestGate => 4,
        }
    }
}

/// Child-observed identity of one callable linked into the selected root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedRunnerLinkedIdentityV1 {
    pub bank: u64,
    pub generated_runner_source_sha256: String,
    pub code_words_sha256: String,
    pub vram_start: u32,
    pub vram_end: u32,
    pub composite_subrunner_count: u32,
    pub adapter_role: GeneratedRunnerAdapterRoleV1,
}

/// Fixed identity envelope emitted by the selected WM executable.
///
/// This wire is not authority on its own. It becomes evidence only inside
/// [`VerifiedGeneratedRunnerBuildV1`], after the verifier has built, selected,
/// hashed, and directly launched the executable which emitted it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedRunnerBuildIdentityV1 {
    pub schema: String,
    pub package: String,
    pub manifest_sha256: String,
    pub lock_sha256: String,
    pub source_attestation_schema: String,
    pub cargo_source_fields_validated: bool,
    pub program_identity_sha256: String,
    pub root_adapter_source_sha256: String,
    pub shard_cargo_source_tree_sha256: String,
    pub emitter_source_sha256: String,
    pub runtime_source_sha256: String,
    pub prepared_source_mode: String,
    pub normalized_rom_sha256: String,
    pub prepared_manifest_sha256: String,
    pub prepared_tree_sha256: String,
    pub prepared_generator_source_sha256: String,
    pub prepared_discovery_source_sha256: String,
    pub prepared_emitter_source_sha256: String,
    pub prepared_runtime_source_sha256: String,
    pub prepared_materializer_source_sha256: String,
    pub producer_manifest_sha256: String,
    pub producer_lock_sha256: String,
    pub producer_cargo_graph_sha256: String,
    pub producer_cargo_source_sha256: String,
    pub producer_binary_sha256: String,
    pub binding_sha256: String,
    pub build_receipt_schema: u32,
    pub aot_runtime: bool,
    pub production_aot: bool,
    pub dev_interpreter: bool,
    pub runners: Vec<GeneratedRunnerLinkedIdentityV1>,
}

/// One named reproducible captured-image group consumed by the WM build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wm2000ExecutableImageGroupV1 {
    pub environment_name: String,
    pub captures: Vec<PathBuf>,
}

/// The only caller-selected inputs to the fixed repository build recipe.
/// Source paths, package, profile, features, Cargo flags, and output target are
/// implementation-owned below.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wm2000GeneratedRunnerBuildInputsV1 {
    pub rom: PathBuf,
    /// Exact black-box header-handoff capture required by the selected
    /// executable's normal boot path. The verifier retains and rehashes this
    /// path privately; generated-runner build evidence exposes only the
    /// aggregate private-input digest.
    pub boot_context: PathBuf,
    pub executable_image_groups: Vec<Wm2000ExecutableImageGroupV1>,
    /// Wall-time ceiling for the process-group guard. The measured full graph
    /// takes roughly forty minutes, so accepted values are 40--120 minutes.
    pub max_build_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedSourceClaimsV3 {
    generator_source_sha256: String,
    discovery_source_sha256: String,
    emitter_source_sha256: String,
    runtime_source_sha256: String,
    materializer_source_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProducerBuildMeasurementV3 {
    manifest_sha256: String,
    lock_sha256: String,
    cargo_graph_sha256: String,
    cargo_source_sha256: String,
    binary_sha256: String,
    binary: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedTreeMeasurementV3 {
    root: PathBuf,
    normalized_rom_sha256: String,
    manifest_sha256: String,
    tree_sha256: String,
    descriptor_sha256: String,
    claims: PreparedSourceClaimsV3,
}

#[derive(Clone, Debug)]
struct BuildEnvironmentV3 {
    path: std::ffi::OsString,
    home: PathBuf,
    cargo_home: PathBuf,
    temp: PathBuf,
    rustc: PathBuf,
    identity_sha256: String,
    rustc_sha256: String,
    cargo_config_sha256: String,
}

/// Integrity projection retained inside the opaque capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerBuildEvidenceV1 {
    pub schema: &'static str,
    pub builder_cargo_sha256: String,
    pub cargo_graph_sha256: String,
    pub cargo_source_sha256: String,
    pub build_environment_sha256: String,
    pub builder_rustc_sha256: String,
    pub cargo_config_sha256: String,
    pub memory_guard_sha256: String,
    pub selected_build_cargo_jobs: u16,
    pub build_max_rss_mib: u32,
    pub build_min_free_percent: u8,
    pub max_build_seconds: u64,
    pub selected_binary_sha256: String,
    pub private_build_inputs_sha256: String,
    pub prepared_tree_descriptor_sha256: String,
    pub prepared_tree_sha256: String,
    pub prepared_source_mode: String,
    pub producer_manifest_sha256: String,
    pub producer_lock_sha256: String,
    pub producer_cargo_graph_sha256: String,
    pub producer_cargo_source_sha256: String,
    pub producer_binary_sha256: String,
    pub identity: GeneratedRunnerBuildIdentityV1,
    pub authority_sha256: String,
}

/// Parent-process authority for one exact built generated runner.
///
/// This type is neither `Clone` nor serializable. Its selected executable path
/// is private, and no API returns a `Command` or path which would let a caller
/// relabel another launch as verifier-owned. A later runtime-series owner must
/// be implemented in this module, consume this capability, and launch the SI
/// child with the retained staged ROM, BootContext, and capture-group paths.
pub struct VerifiedGeneratedRunnerBuildV1 {
    evidence: GeneratedRunnerBuildEvidenceV1,
    selected_binary: PathBuf,
    private_inputs: Wm2000GeneratedRunnerBuildInputsV1,
    prepared: PreparedTreeMeasurementV3,
    producer: ProducerBuildMeasurementV3,
    _scratch: ScratchDirectory,
}

impl fmt::Debug for VerifiedGeneratedRunnerBuildV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGeneratedRunnerBuildV1")
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl VerifiedGeneratedRunnerBuildV1 {
    pub fn evidence(&self) -> &GeneratedRunnerBuildEvidenceV1 {
        &self.evidence
    }

    pub fn revalidate_selected_binary(&self) -> Result<(), GeneratedRunnerBuildError> {
        let observed = sha256_file(&self.selected_binary, "selected generated runner")?;
        if observed != self.evidence.selected_binary_sha256 {
            return Err(error(format!(
                "selected generated runner changed: expected={}, observed={observed}",
                self.evidence.selected_binary_sha256
            )));
        }
        if private_inputs_sha256(&self.private_inputs)? != self.evidence.private_build_inputs_sha256
        {
            return Err(error(
                "private generated-runner inputs changed after the verified build",
            ));
        }
        if measure_prepared_tree_v3(
            &self.prepared.root,
            &self.prepared.normalized_rom_sha256,
            &self.prepared.claims,
        )? != self.prepared
            || sha256_file(&self.producer.binary, "retained prepared producer")?
                != self.producer.binary_sha256
        {
            return Err(error(
                "retained prepared candidate or producer changed after verified build",
            ));
        }
        self.evidence.verify_integrity()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapWriterWatchedRangeV1 {
    pub physical_start: u32,
    pub physical_end: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapWriterChannelV1 {
    BootstrapOrImport,
}

impl BootstrapWriterChannelV1 {
    const fn tag(self) -> u8 {
        match self {
            Self::BootstrapOrImport => fn64_recomp_rs::WriterChannel::BootstrapOrImport as u8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapAttributedWriteV1 {
    pub channel: BootstrapWriterChannelV1,
    pub physical_start: u32,
    pub physical_end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapMutationBatchV1 {
    pub sequence: u64,
    pub declared_writes: Vec<BootstrapAttributedWriteV1>,
    pub changed_ranges: Vec<BootstrapWriterWatchedRangeV1>,
    pub before_sha256: String,
    pub after_sha256: String,
    pub invalidated_generations: Vec<u64>,
    pub journal_root_sha256: String,
}

/// Pointer-free projection of the ABI-owned bootstrap writer receipt.
/// Deserialization cannot reconstruct the move-only receipt held in the child.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapWriterRuntimePrerequisiteV1 {
    pub schema: String,
    pub program_model_sha256: String,
    pub bootstrap_receipt_sha256: String,
    pub rom_sha256: String,
    pub resolver_install_sha256: String,
    pub generation_catalog_sha256: String,
    pub watched_ranges: Vec<BootstrapWriterWatchedRangeV1>,
    pub bootstrap_watched_sha256: String,
    pub initial_generations: Vec<u64>,
    pub journal_entry: BootstrapMutationBatchV1,
    pub final_watched_sha256: String,
    pub receipt_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedRunnerBootstrapRuntimeReportV1 {
    pub schema: String,
    pub nonce: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub prerequisite: BootstrapWriterRuntimePrerequisiteV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1 {
    pub schema: &'static str,
    pub run_count: u8,
    pub build_authority_sha256: String,
    pub selected_binary_sha256: String,
    pub private_build_inputs_sha256: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub program_model_sha256: String,
    pub bootstrap_receipt_sha256: String,
    pub rom_sha256: String,
    pub resolver_install_sha256: String,
    pub generation_catalog_sha256: String,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub runtime_receipt_sha256: String,
    pub semantic_report_sha256: String,
    pub nonce_set_sha256: String,
    pub authority_sha256: String,
}

pub struct VerifiedGeneratedRunnerBootstrapRuntimeSeriesV1 {
    evidence: GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1,
    _build: VerifiedGeneratedRunnerBuildV1,
}

impl fmt::Debug for VerifiedGeneratedRunnerBootstrapRuntimeSeriesV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGeneratedRunnerBootstrapRuntimeSeriesV1")
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl VerifiedGeneratedRunnerBootstrapRuntimeSeriesV1 {
    pub fn evidence(&self) -> &GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        validate_bootstrap_runtime_series_evidence(&self.evidence).is_ok()
    }
}

/// Canonical half-open executable backing retained by a CPU-store report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuWriterWatchedRangeV1 {
    pub physical_start: u32,
    pub physical_end: u32,
}

/// Pointer-free projection of the ABI-local CPU instruction-store receipt.
/// Deserialization cannot recreate either its fresh epoch or move-only receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuWriterRuntimePrerequisiteV1 {
    pub schema: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub build_receipt_schema: u32,
    pub aot_runtime: bool,
    pub production_aot: bool,
    pub dev_interpreter: bool,
    pub trace_epoch_id: u64,
    pub watched_ranges: Vec<CpuWriterWatchedRangeV1>,
    pub journal_entry_count: u64,
    pub cpu_journal_declaration_count: u64,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub cpu_store_count: u64,
    pub cpu_store_trace_sha256: String,
    pub receipt_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedRunnerCpuRuntimeReportV1 {
    pub schema: String,
    pub nonce: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub prerequisite: CpuWriterRuntimePrerequisiteV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerCpuRuntimeSeriesEvidenceV1 {
    pub schema: &'static str,
    pub run_count: u8,
    pub build_authority_sha256: String,
    pub selected_binary_sha256: String,
    pub private_build_inputs_sha256: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub cpu_store_trace_sha256: String,
    pub runtime_receipt_sha256: String,
    pub semantic_report_sha256: String,
    pub nonce_set_sha256: String,
    pub authority_sha256: String,
}

/// Move-only parent authority for ten directly owned, semantically identical
/// CPU instruction-store audit launches of one exact generated runner.
pub struct VerifiedGeneratedRunnerCpuRuntimeSeriesV1 {
    evidence: GeneratedRunnerCpuRuntimeSeriesEvidenceV1,
    _build: VerifiedGeneratedRunnerBuildV1,
}

impl fmt::Debug for VerifiedGeneratedRunnerCpuRuntimeSeriesV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGeneratedRunnerCpuRuntimeSeriesV1")
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl VerifiedGeneratedRunnerCpuRuntimeSeriesV1 {
    pub fn evidence(&self) -> &GeneratedRunnerCpuRuntimeSeriesEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        validate_cpu_runtime_series_evidence(&self.evidence).is_ok()
    }
}

/// Canonical half-open executable backing retained by a Host ABI report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostAbiWriterWatchedRangeV1 {
    pub physical_start: u32,
    pub physical_end: u32,
}

/// Pointer-free projection of the ABI-local canonical Host ABI receipt.
/// Compatibility caller-supplied raw-pointer catalogs cannot mint that receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostAbiWriterRuntimePrerequisiteV1 {
    pub schema: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub build_receipt_schema: u32,
    pub aot_runtime: bool,
    pub production_aot: bool,
    pub dev_interpreter: bool,
    pub trace_epoch_id: u64,
    pub initial_journal_entry_count: u64,
    pub final_journal_entry_count: u64,
    pub watched_ranges: Vec<HostAbiWriterWatchedRangeV1>,
    pub host_abi_journal_entry_count: u64,
    pub host_abi_journal_declaration_count: u64,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub transactions_started: u64,
    pub transactions_finished: u64,
    pub ordering_boundaries: u64,
    pub lifecycle_sha256: String,
    pub receipt_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedRunnerHostAbiRuntimeReportV1 {
    pub schema: String,
    pub nonce: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub prerequisite: HostAbiWriterRuntimePrerequisiteV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1 {
    pub schema: &'static str,
    pub run_count: u8,
    pub build_authority_sha256: String,
    pub selected_binary_sha256: String,
    pub private_build_inputs_sha256: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub lifecycle_sha256: String,
    pub runtime_receipt_sha256: String,
    pub semantic_report_sha256: String,
    pub nonce_set_sha256: String,
    pub authority_sha256: String,
}

/// Move-only parent authority for ten directly owned, semantically identical
/// canonical Host ABI audit launches of one exact generated runner.
pub struct VerifiedGeneratedRunnerHostAbiRuntimeSeriesV1 {
    evidence: GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1,
    _build: VerifiedGeneratedRunnerBuildV1,
}

impl fmt::Debug for VerifiedGeneratedRunnerHostAbiRuntimeSeriesV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGeneratedRunnerHostAbiRuntimeSeriesV1")
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl VerifiedGeneratedRunnerHostAbiRuntimeSeriesV1 {
    pub fn evidence(&self) -> &GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        validate_host_abi_runtime_series_evidence(&self.evidence).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PiWriterWatchedRangeV1 {
    pub physical_start: u32,
    pub physical_end: u32,
}

/// Pointer-free projection of the ABI-local PI-DMA runtime prerequisite.
/// Deserialization cannot recreate either its fresh epoch or move-only receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PiWriterRuntimePrerequisiteV1 {
    pub schema: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub build_receipt_schema: u32,
    pub aot_runtime: bool,
    pub production_aot: bool,
    pub dev_interpreter: bool,
    pub trace_epoch_id: u64,
    pub watched_ranges: Vec<PiWriterWatchedRangeV1>,
    pub journal_entry_count: u64,
    pub pi_journal_declaration_count: u64,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub pi_started: u64,
    pub pi_committed: u64,
    pub pi_busy_cleared: u64,
    pub pi_interrupt_raised: u64,
    pub pi_interrupt_cleared: u64,
    pub pi_notifications: u64,
    pub pi_to_rdram_committed: u64,
    pub pi_transition_sha256: String,
    pub receipt_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedRunnerPiRuntimeReportV1 {
    pub schema: String,
    pub nonce: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub prerequisite: PiWriterRuntimePrerequisiteV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerPiRuntimeSeriesEvidenceV1 {
    pub schema: &'static str,
    pub run_count: u8,
    pub build_authority_sha256: String,
    pub selected_binary_sha256: String,
    pub private_build_inputs_sha256: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub pi_transition_sha256: String,
    pub runtime_receipt_sha256: String,
    pub semantic_report_sha256: String,
    pub nonce_set_sha256: String,
    pub authority_sha256: String,
}

/// Move-only parent authority for ten directly owned, semantically identical
/// PI-DMA audit launches of one exact generated runner.
pub struct VerifiedGeneratedRunnerPiRuntimeSeriesV1 {
    evidence: GeneratedRunnerPiRuntimeSeriesEvidenceV1,
    _build: VerifiedGeneratedRunnerBuildV1,
}

impl fmt::Debug for VerifiedGeneratedRunnerPiRuntimeSeriesV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGeneratedRunnerPiRuntimeSeriesV1")
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl VerifiedGeneratedRunnerPiRuntimeSeriesV1 {
    pub fn evidence(&self) -> &GeneratedRunnerPiRuntimeSeriesEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        validate_pi_runtime_series_evidence(&self.evidence).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RdpRendererWriterWatchedRangeV1 {
    pub physical_start: u32,
    pub physical_end: u32,
}

/// Pointer-free projection of the ABI-local RDP renderer prerequisite.
/// Deserialization cannot recreate its fresh epoch or move-only receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RdpRendererWriterRuntimePrerequisiteV1 {
    pub schema: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub build_receipt_schema: u32,
    pub aot_runtime: bool,
    pub production_aot: bool,
    pub dev_interpreter: bool,
    pub trace_epoch_id: u64,
    pub initial_journal_entry_count: u64,
    pub final_journal_entry_count: u64,
    pub watched_ranges: Vec<RdpRendererWriterWatchedRangeV1>,
    pub rdp_renderer_journal_entry_count: u64,
    pub rdp_renderer_journal_declaration_count: u64,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub renderer_publication_count: u64,
    pub publication_trace_sha256: String,
    pub receipt_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedRunnerRdpRendererRuntimeReportV1 {
    pub schema: String,
    pub nonce: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub prerequisite: RdpRendererWriterRuntimePrerequisiteV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1 {
    pub schema: &'static str,
    pub run_count: u8,
    pub build_authority_sha256: String,
    pub selected_binary_sha256: String,
    pub private_build_inputs_sha256: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub publication_trace_sha256: String,
    pub runtime_receipt_sha256: String,
    pub semantic_report_sha256: String,
    pub nonce_set_sha256: String,
    pub authority_sha256: String,
}

/// Move-only parent authority for ten directly owned, semantically identical
/// RDP renderer audit launches of one exact generated runner.
pub struct VerifiedGeneratedRunnerRdpRendererRuntimeSeriesV1 {
    evidence: GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1,
    _build: VerifiedGeneratedRunnerBuildV1,
}

impl fmt::Debug for VerifiedGeneratedRunnerRdpRendererRuntimeSeriesV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGeneratedRunnerRdpRendererRuntimeSeriesV1")
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl VerifiedGeneratedRunnerRdpRendererRuntimeSeriesV1 {
    pub fn evidence(&self) -> &GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        validate_rdp_renderer_runtime_series_evidence(&self.evidence).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RspWriterWatchedRangeV1 {
    pub physical_start: u32,
    pub physical_end: u32,
}

/// Pointer-free projection of the ABI-local RSP execution/HLE prerequisite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RspWriterRuntimePrerequisiteV1 {
    pub schema: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub build_receipt_schema: u32,
    pub aot_runtime: bool,
    pub production_aot: bool,
    pub dev_interpreter: bool,
    pub trace_epoch_id: u64,
    pub watched_ranges: Vec<RspWriterWatchedRangeV1>,
    pub journal_entry_count: u64,
    pub rsp_journal_declaration_count: u64,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub interpreter_writeback_count: u64,
    pub translated_audio_hle_publication_count: u64,
    pub writeback_range_count: u64,
    pub writeback_trace_sha256: String,
    pub receipt_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedRunnerRspRuntimeReportV1 {
    pub schema: String,
    pub nonce: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub prerequisite: RspWriterRuntimePrerequisiteV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerRspRuntimeSeriesEvidenceV1 {
    pub schema: &'static str,
    pub run_count: u8,
    pub build_authority_sha256: String,
    pub selected_binary_sha256: String,
    pub private_build_inputs_sha256: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub writeback_trace_sha256: String,
    pub runtime_receipt_sha256: String,
    pub semantic_report_sha256: String,
    pub nonce_set_sha256: String,
    pub authority_sha256: String,
}

/// Move-only parent authority for ten directly owned, semantically identical
/// RSP execution/HLE audit launches of one exact generated runner.
pub struct VerifiedGeneratedRunnerRspRuntimeSeriesV1 {
    evidence: GeneratedRunnerRspRuntimeSeriesEvidenceV1,
    _build: VerifiedGeneratedRunnerBuildV1,
}

impl fmt::Debug for VerifiedGeneratedRunnerRspRuntimeSeriesV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGeneratedRunnerRspRuntimeSeriesV1")
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl VerifiedGeneratedRunnerRspRuntimeSeriesV1 {
    pub fn evidence(&self) -> &GeneratedRunnerRspRuntimeSeriesEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        validate_rsp_runtime_series_evidence(&self.evidence).is_ok()
    }
}

/// Canonical half-open executable backing retained by an SI runtime report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiWriterWatchedRangeV1 {
    pub physical_start: u32,
    pub physical_end: u32,
}

/// Pointer-free projection of the ABI-local SI runtime-state prerequisite.
///
/// This wire is evidence only. Deserializing it cannot recreate the ABI's
/// move-only receipt or the future verifier-owned runtime-series authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiWriterRuntimePrerequisiteV1 {
    pub schema: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub build_receipt_schema: u32,
    pub aot_runtime: bool,
    pub production_aot: bool,
    pub dev_interpreter: bool,
    pub watched_ranges: Vec<SiWriterWatchedRangeV1>,
    pub journal_entry_count: u64,
    pub si_journal_declaration_count: u64,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub si_started: u64,
    pub si_committed: u64,
    pub si_pif_to_dram_committed: u64,
    pub si_transition_sha256: String,
    pub receipt_sha256: String,
}

/// One nonce-bound report emitted by a future fixed SI child mode.
///
/// The selected-build verifier must parse this report from a child it launched
/// itself. This public data shape alone carries no authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedRunnerSiRuntimeReportV1 {
    pub schema: String,
    pub nonce: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub prerequisite: SiWriterRuntimePrerequisiteV1,
}

/// Pointer-free evidence retained by the parent-owned exact-ten SI series.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerSiRuntimeSeriesEvidenceV1 {
    pub schema: &'static str,
    pub run_count: u8,
    pub build_authority_sha256: String,
    pub selected_binary_sha256: String,
    pub private_build_inputs_sha256: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub si_transition_sha256: String,
    pub runtime_receipt_sha256: String,
    pub semantic_report_sha256: String,
    pub nonce_set_sha256: String,
    pub authority_sha256: String,
}

/// Move-only parent authority for ten directly owned, semantically identical
/// SI audit launches of one exact verified generated runner.
///
/// This is not writer-denominator completion authority. It retains the build
/// capability, including its private staged inputs, and has no constructor,
/// clone, or serialization implementation outside this module.
pub struct VerifiedGeneratedRunnerSiRuntimeSeriesV1 {
    evidence: GeneratedRunnerSiRuntimeSeriesEvidenceV1,
    _build: VerifiedGeneratedRunnerBuildV1,
}

impl fmt::Debug for VerifiedGeneratedRunnerSiRuntimeSeriesV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGeneratedRunnerSiRuntimeSeriesV1")
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl VerifiedGeneratedRunnerSiRuntimeSeriesV1 {
    pub fn evidence(&self) -> &GeneratedRunnerSiRuntimeSeriesEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        validate_si_runtime_series_evidence(&self.evidence).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpWriterWatchedRangeV1 {
    pub physical_start: u32,
    pub physical_end: u32,
}

/// Pointer-free projection of the ABI-local SP runtime-state prerequisite.
/// Deserialization cannot recreate either its ABI receipt or parent series.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpWriterRuntimePrerequisiteV1 {
    pub schema: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub build_receipt_schema: u32,
    pub aot_runtime: bool,
    pub production_aot: bool,
    pub dev_interpreter: bool,
    pub trace_epoch_id: u64,
    pub watched_ranges: Vec<SpWriterWatchedRangeV1>,
    pub journal_entry_count: u64,
    pub sp_journal_declaration_count: u64,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub sp_started: u64,
    pub sp_queued: u64,
    pub sp_committed: u64,
    pub sp_busy_cleared: u64,
    pub sp_rsp_to_rdram_committed: u64,
    pub sp_transition_sha256: String,
    pub receipt_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedRunnerSpRuntimeReportV1 {
    pub schema: String,
    pub nonce: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub prerequisite: SpWriterRuntimePrerequisiteV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerSpRuntimeSeriesEvidenceV1 {
    pub schema: &'static str,
    pub run_count: u8,
    pub build_authority_sha256: String,
    pub selected_binary_sha256: String,
    pub private_build_inputs_sha256: String,
    pub build_identity_sha256: String,
    pub program_identity_sha256: String,
    pub program_model_sha256: String,
    pub resolver_install_sha256: String,
    pub abi_host_catalog_receipt_sha256: String,
    pub journal_root_sha256: String,
    pub final_watched_sha256: String,
    pub sp_transition_sha256: String,
    pub runtime_receipt_sha256: String,
    pub semantic_report_sha256: String,
    pub nonce_set_sha256: String,
    pub authority_sha256: String,
}

/// Move-only parent authority for ten directly owned, semantically identical
/// SP audit launches of one exact verified generated runner. The ABI-local
/// receipt alone grants no writer-denominator credit; this outer exact-ten
/// series is the sole accepted runtime-series input to `complete_sp`.
pub struct VerifiedGeneratedRunnerSpRuntimeSeriesV1 {
    evidence: GeneratedRunnerSpRuntimeSeriesEvidenceV1,
    _build: VerifiedGeneratedRunnerBuildV1,
}

impl fmt::Debug for VerifiedGeneratedRunnerSpRuntimeSeriesV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGeneratedRunnerSpRuntimeSeriesV1")
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl VerifiedGeneratedRunnerSpRuntimeSeriesV1 {
    pub fn evidence(&self) -> &GeneratedRunnerSpRuntimeSeriesEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        validate_sp_runtime_series_evidence(&self.evidence).is_ok()
    }
}

/// One-build owner for independently verified writer-channel series.
///
/// The selected binary and private inputs remain inside the retained build.
/// Each channel may run once; a failed series stores no evidence and does not
/// erase evidence already established for another channel.
pub struct GeneratedRunnerWriterAuditSessionV1 {
    build: VerifiedGeneratedRunnerBuildV1,
    bootstrap: Option<GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1>,
    cpu: Option<GeneratedRunnerCpuRuntimeSeriesEvidenceV1>,
    host_abi: Option<GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1>,
    pi: Option<GeneratedRunnerPiRuntimeSeriesEvidenceV1>,
    rdp_renderer: Option<GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1>,
    rsp: Option<GeneratedRunnerRspRuntimeSeriesEvidenceV1>,
    si: Option<GeneratedRunnerSiRuntimeSeriesEvidenceV1>,
    sp: Option<GeneratedRunnerSpRuntimeSeriesEvidenceV1>,
}

impl fmt::Debug for GeneratedRunnerWriterAuditSessionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GeneratedRunnerWriterAuditSessionV1")
            .field("bootstrap_complete", &self.bootstrap.is_some())
            .field("cpu_complete", &self.cpu.is_some())
            .field("host_abi_complete", &self.host_abi.is_some())
            .field("pi_complete", &self.pi.is_some())
            .field("rdp_renderer_complete", &self.rdp_renderer.is_some())
            .field("rsp_complete", &self.rsp.is_some())
            .field("si_complete", &self.si.is_some())
            .field("sp_complete", &self.sp.is_some())
            .finish_non_exhaustive()
    }
}

impl GeneratedRunnerWriterAuditSessionV1 {
    pub fn new(build: VerifiedGeneratedRunnerBuildV1) -> Self {
        Self {
            build,
            bootstrap: None,
            cpu: None,
            host_abi: None,
            pi: None,
            rdp_renderer: None,
            rsp: None,
            si: None,
            sp: None,
        }
    }

    pub fn run_bootstrap_runtime_series_v1(&mut self) -> Result<(), GeneratedRunnerBuildError> {
        if self.bootstrap.is_some() {
            return Err(error(
                "bootstrap runtime series already completed in this session",
            ));
        }
        let evidence = run_bootstrap_runtime_series_evidence_v1(&self.build)?;
        self.bootstrap = Some(evidence);
        Ok(())
    }

    pub fn run_si_runtime_series_v1(&mut self) -> Result<(), GeneratedRunnerBuildError> {
        if self.si.is_some() {
            return Err(error("SI runtime series already completed in this session"));
        }
        let evidence = run_si_runtime_series_evidence_v1(&self.build)?;
        self.si = Some(evidence);
        Ok(())
    }

    pub fn run_cpu_runtime_series_v1(&mut self) -> Result<(), GeneratedRunnerBuildError> {
        if self.cpu.is_some() {
            return Err(error(
                "CPU runtime series already completed in this session",
            ));
        }
        let evidence = run_cpu_runtime_series_evidence_v1(&self.build)?;
        self.cpu = Some(evidence);
        Ok(())
    }

    pub fn run_pi_runtime_series_v1(&mut self) -> Result<(), GeneratedRunnerBuildError> {
        if self.pi.is_some() {
            return Err(error("PI runtime series already completed in this session"));
        }
        let evidence = run_pi_runtime_series_evidence_v1(&self.build)?;
        self.pi = Some(evidence);
        Ok(())
    }

    pub fn run_host_abi_runtime_series_v1(&mut self) -> Result<(), GeneratedRunnerBuildError> {
        if self.host_abi.is_some() {
            return Err(error(
                "Host ABI runtime series already completed in this session",
            ));
        }
        let evidence = run_host_abi_runtime_series_evidence_v1(&self.build)?;
        self.host_abi = Some(evidence);
        Ok(())
    }

    pub fn run_rdp_renderer_runtime_series_v1(&mut self) -> Result<(), GeneratedRunnerBuildError> {
        if self.rdp_renderer.is_some() {
            return Err(error(
                "RDP renderer runtime series already completed in this session",
            ));
        }
        let evidence = run_rdp_renderer_runtime_series_evidence_v1(&self.build)?;
        self.rdp_renderer = Some(evidence);
        Ok(())
    }

    pub fn run_rsp_runtime_series_v1(&mut self) -> Result<(), GeneratedRunnerBuildError> {
        if self.rsp.is_some() {
            return Err(error(
                "RSP runtime series already completed in this session",
            ));
        }
        let evidence = run_rsp_runtime_series_evidence_v1(&self.build)?;
        self.rsp = Some(evidence);
        Ok(())
    }

    pub fn run_sp_runtime_series_v1(&mut self) -> Result<(), GeneratedRunnerBuildError> {
        if self.sp.is_some() {
            return Err(error("SP runtime series already completed in this session"));
        }
        let evidence = run_sp_runtime_series_evidence_v1(&self.build)?;
        self.sp = Some(evidence);
        Ok(())
    }

    pub fn seal(
        self,
    ) -> Result<VerifiedGeneratedRunnerWriterAuditBundleV1, GeneratedRunnerBuildError> {
        self.build.revalidate_selected_binary()?;
        let mut completed_channels = 0;
        if self.bootstrap.is_some() {
            completed_channels |= WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1;
        }
        if self.cpu.is_some() {
            completed_channels |= WRITER_AUDIT_CPU_COMPLETED_V1;
        }
        if self.pi.is_some() {
            completed_channels |= WRITER_AUDIT_PI_COMPLETED_V1;
        }
        if self.host_abi.is_some() {
            completed_channels |= WRITER_AUDIT_HOST_ABI_COMPLETED_V1;
        }
        if self.rdp_renderer.is_some() {
            completed_channels |= WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1;
        }
        if self.rsp.is_some() {
            completed_channels |= WRITER_AUDIT_RSP_COMPLETED_V1;
        }
        if self.si.is_some() {
            completed_channels |= WRITER_AUDIT_SI_COMPLETED_V1;
        }
        if self.sp.is_some() {
            completed_channels |= WRITER_AUDIT_SP_COMPLETED_V1;
        }
        if completed_channels == 0 {
            return Err(error(
                "writer audit session cannot seal without a completed series",
            ));
        }
        let mut evidence = GeneratedRunnerWriterAuditBundleEvidenceV1 {
            schema: VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1,
            completed_channels,
            build_authority_sha256: self.build.evidence.authority_sha256.clone(),
            selected_binary_sha256: self.build.evidence.selected_binary_sha256.clone(),
            private_build_inputs_sha256: self.build.evidence.private_build_inputs_sha256.clone(),
            bootstrap: self.bootstrap,
            cpu: self.cpu,
            host_abi: self.host_abi,
            pi: self.pi,
            rdp_renderer: self.rdp_renderer,
            rsp: self.rsp,
            si: self.si,
            sp: self.sp,
            authority_sha256: String::new(),
        };
        evidence.authority_sha256 = writer_audit_bundle_authority_sha256(&evidence)?;
        validate_writer_audit_bundle_evidence(&evidence)?;
        Ok(VerifiedGeneratedRunnerWriterAuditBundleV1 {
            evidence,
            _build: self.build,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerWriterAuditBundleEvidenceV1 {
    pub schema: &'static str,
    pub completed_channels: u8,
    pub build_authority_sha256: String,
    pub selected_binary_sha256: String,
    pub private_build_inputs_sha256: String,
    pub bootstrap: Option<GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1>,
    pub cpu: Option<GeneratedRunnerCpuRuntimeSeriesEvidenceV1>,
    pub host_abi: Option<GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1>,
    pub pi: Option<GeneratedRunnerPiRuntimeSeriesEvidenceV1>,
    pub rdp_renderer: Option<GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1>,
    pub rsp: Option<GeneratedRunnerRspRuntimeSeriesEvidenceV1>,
    pub si: Option<GeneratedRunnerSiRuntimeSeriesEvidenceV1>,
    pub sp: Option<GeneratedRunnerSpRuntimeSeriesEvidenceV1>,
    pub authority_sha256: String,
}

pub struct VerifiedGeneratedRunnerWriterAuditBundleV1 {
    evidence: GeneratedRunnerWriterAuditBundleEvidenceV1,
    _build: VerifiedGeneratedRunnerBuildV1,
}

impl fmt::Debug for VerifiedGeneratedRunnerWriterAuditBundleV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGeneratedRunnerWriterAuditBundleV1")
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl VerifiedGeneratedRunnerWriterAuditBundleV1 {
    pub fn evidence(&self) -> &GeneratedRunnerWriterAuditBundleEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        validate_writer_audit_bundle_evidence(&self.evidence).is_ok()
    }
}

#[derive(Debug)]
pub struct GeneratedRunnerBuildError(String);

impl fmt::Display for GeneratedRunnerBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for GeneratedRunnerBuildError {}

impl BuildEnvironmentV3 {
    fn new(cargo: &Path, scratch: &Path) -> Result<Self, GeneratedRunnerBuildError> {
        let toolchain = cargo
            .parent()
            .ok_or_else(|| error("verified Cargo has no parent directory"))?;
        let rustc = toolchain.join(if cfg!(windows) { "rustc.exe" } else { "rustc" });
        let rustc_sha256 = sha256_file(&rustc, "verified Cargo sibling rustc")?;
        let home = scratch.join("build-home");
        let temp = scratch.join("build-temp");
        fs::create_dir(&home).map_err(|source| error(format!("create build HOME: {source}")))?;
        fs::create_dir(&temp).map_err(|source| error(format!("create build TMPDIR: {source}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&home, fs::Permissions::from_mode(0o700))
                .map_err(|source| error(format!("restrict build HOME: {source}")))?;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o700))
                .map_err(|source| error(format!("restrict build TMPDIR: {source}")))?;
        }
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
            .ok_or_else(|| error("verified frozen build requires an explicit Cargo cache home"))?
            .canonicalize()
            .map_err(|source| error(format!("resolve Cargo cache home: {source}")))?;
        let path = std::env::join_paths([
            toolchain.to_path_buf(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
            PathBuf::from("/usr/sbin"),
            PathBuf::from("/sbin"),
        ])
        .map_err(|source| error(format!("construct verified build PATH: {source}")))?;
        let cargo_config_sha256 = cargo_config_sha256_v3(&cargo_home, scratch)?;
        let mut digest = Sha256::new();
        digest.update(b"fn64.generated-runner-build-environment.v1\0");
        for (name, value) in [
            ("PATH", path.as_encoded_bytes()),
            ("HOME", home.as_os_str().as_encoded_bytes()),
            ("CARGO_HOME", cargo_home.as_os_str().as_encoded_bytes()),
            ("TMPDIR", temp.as_os_str().as_encoded_bytes()),
            ("RUSTC", rustc.as_os_str().as_encoded_bytes()),
            ("RUSTFLAGS", b""),
        ] {
            push_bytes(&mut digest, name.as_bytes());
            push_bytes(&mut digest, value);
        }
        digest.update(decode_sha256(&rustc_sha256)?);
        digest.update(decode_sha256(&cargo_config_sha256)?);
        Ok(Self {
            path,
            home,
            cargo_home,
            temp,
            rustc,
            identity_sha256: hex(&digest.finalize()),
            rustc_sha256,
            cargo_config_sha256,
        })
    }

    fn apply(&self, command: &mut Command) {
        command
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", &self.home)
            .env("CARGO_HOME", &self.cargo_home)
            .env("TMPDIR", &self.temp)
            .env("RUSTC", &self.rustc)
            .env("RUSTFLAGS", "")
            .env("CARGO_ENCODED_RUSTFLAGS", "");
    }

    fn revalidate(&self) -> Result<(), GeneratedRunnerBuildError> {
        if sha256_file(&self.rustc, "verified Cargo sibling rustc revalidation")?
            != self.rustc_sha256
            || cargo_config_sha256_v3(
                &self.cargo_home,
                self.home.parent().expect("build HOME has scratch parent"),
            )? != self.cargo_config_sha256
        {
            return Err(error("verified build toolchain environment changed"));
        }
        Ok(())
    }
}

fn cargo_config_sha256_v3(
    cargo_home: &Path,
    cargo_current_dir: &Path,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.generated-runner-cargo-config.v1\0");
    let mut candidates = ["config", "config.toml"]
        .map(|name| cargo_home.join(name))
        .to_vec();
    for ancestor in cargo_current_dir.ancestors() {
        for name in ["config", "config.toml"] {
            candidates.push(ancestor.join(".cargo").join(name));
        }
    }
    for path in candidates {
        push_bytes(&mut digest, path.as_os_str().as_encoded_bytes());
        if path.exists() {
            digest.update([1]);
            let source = crate::private_fs::read_regular_stable(&path, "Cargo home config")
                .map_err(error)?;
            push_bytes(&mut digest, &source.contents);
        } else {
            digest.update([0]);
        }
    }
    Ok(hex(&digest.finalize()))
}

/// Build and select the exact repository-owned WM generated runner.
///
/// Cargo itself is the build authority already pinned by
/// `platform_certification`: this reuses that executable identity, invokes a
/// frozen standalone build in a fresh target directory, accepts exactly one
/// matching compiler artifact, and launches only its fixed identity mode.
pub fn build_wm2000_generated_runner_v1(
    inputs: Wm2000GeneratedRunnerBuildInputsV1,
) -> Result<VerifiedGeneratedRunnerBuildV1, GeneratedRunnerBuildError> {
    validate_inputs(&inputs)?;
    let workspace = repository_workspace()?;
    let package_root = workspace.join("examples/wm2000-block-boot");
    let manifest = package_root.join("Cargo.toml");
    let lock = package_root.join("Cargo.lock");
    let manifest_sha256 = sha256_file(&manifest, "WM generated-runner manifest")?;
    let lock_sha256 = sha256_file(&lock, "WM generated-runner lockfile")?;
    let prepared_source_mode = wm_prepared_source_mode_v3(&package_root)?;
    let expected_root_adapter_source_sha256 = wm_root_adapter_source_sha256(&package_root)?;
    let expected_shard_source_sha256 =
        wm_shard_cargo_source_sha256(&package_root, prepared_source_mode)?;
    let expected_emitter_source_sha256 = wm_emitter_source_sha256(&workspace)?;
    let expected_runtime_source_sha256 =
        hex(&fn64_recomp_rs::generated_runner_runtime_source_receipt_v1().source_sha256());
    let prepared_claims = prepared_source_claims_v3(&workspace)?;
    let memory_guard = workspace.join("scripts/memory-guard.zsh");
    let memory_guard_sha256 = validate_memory_guard(&memory_guard)?;
    let cargo = crate::platform_certification::verified_build_cargo()
        .map_err(|source| error(format!("verify Cargo build owner: {source}")))?;
    let builder_cargo_sha256 = env!("FN64_BUILD_CARGO_SHA256").to_owned();
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce)
        .map_err(|source| error(format!("obtain generated-runner build nonce: {source}")))?;
    let scratch = ScratchDirectory::create(&nonce)?;
    let build_environment = BuildEnvironmentV3::new(&cargo, scratch.path())?;
    let staged_inputs = stage_private_inputs(&inputs, scratch.path())?;

    let private_build_inputs_sha256 = private_inputs_sha256(&staged_inputs)?;
    let expected_normalized_rom_sha256 = normalized_rom_sha256(&staged_inputs.rom)?;
    let producer = build_prepared_producer_v3(
        &memory_guard,
        &cargo,
        &build_environment,
        &workspace,
        scratch.path(),
        staged_inputs.max_build_seconds,
    )?;
    build_environment.revalidate()?;
    if prepared_source_claims_v3(&workspace)? != prepared_claims {
        return Err(error(
            "prepared source claims changed during producer build",
        ));
    }
    let prepared = invoke_prepared_producer_v3(
        &memory_guard,
        &producer,
        &build_environment,
        &staged_inputs.rom,
        &prepared_claims,
        &expected_normalized_rom_sha256,
        scratch.path(),
        staged_inputs.max_build_seconds,
    )?;
    build_environment.revalidate()?;
    if prepared_source_claims_v3(&workspace)? != prepared_claims {
        return Err(error("prepared source claims changed during publication"));
    }
    let metadata = run_cargo_metadata(&cargo, &build_environment, &manifest, scratch.path())?;
    let cargo_graph_sha256 = hex(&Sha256::digest(&metadata));
    let cargo_source_sha256 = cargo_metadata_source_sha256(&metadata)?;
    if measure_prepared_tree_v3(
        &prepared.root,
        &expected_normalized_rom_sha256,
        &prepared_claims,
    )? != prepared
    {
        return Err(error("prepared tree changed before the owned Cargo build"));
    }
    let selected = build_selected_binary(
        &memory_guard,
        &cargo,
        &manifest,
        &staged_inputs,
        &prepared,
        &producer,
        prepared_source_mode,
        &build_environment,
        scratch.path(),
    )?;
    build_environment.revalidate()?;
    if prepared_source_claims_v3(&workspace)? != prepared_claims
        || wm_prepared_source_mode_v3(&package_root)? != prepared_source_mode
    {
        return Err(error(
            "prepared source authority changed during Cargo build",
        ));
    }
    crate::platform_certification::verified_build_cargo()
        .map_err(|source| error(format!("reverify Cargo build owner: {source}")))?;
    if measure_prepared_tree_v3(
        &prepared.root,
        &expected_normalized_rom_sha256,
        &prepared_claims,
    )? != prepared
    {
        return Err(error("prepared tree changed during the owned Cargo build"));
    }
    if private_inputs_sha256(&staged_inputs)? != private_build_inputs_sha256 {
        return Err(error(
            "private generated-runner build inputs changed during Cargo build",
        ));
    }
    let selected_binary_sha256 = sha256_file(&selected, "built generated runner")?;
    let staged = stage_selected_binary(&selected, scratch.path(), &selected_binary_sha256)?;
    let identity = launch_identity_child(&staged, scratch.path())?;
    build_environment.revalidate()?;
    validate_identity(&identity, &manifest_sha256, &lock_sha256)?;
    if identity.root_adapter_source_sha256 != expected_root_adapter_source_sha256
        || identity.shard_cargo_source_tree_sha256 != expected_shard_source_sha256
        || identity.emitter_source_sha256 != expected_emitter_source_sha256
        || identity.runtime_source_sha256 != expected_runtime_source_sha256
    {
        return Err(error(
            "generated-runner child source attestation does not match verifier-measured source domains",
        ));
    }
    validate_prepared_identity_v3(&identity, &prepared, &producer, prepared_source_mode)?;
    if prepared_source_claims_v3(&workspace)? != prepared_claims
        || wm_prepared_source_mode_v3(&package_root)? != prepared_source_mode
    {
        return Err(error(
            "prepared source authority changed during identity child",
        ));
    }
    revalidate_prepared_producer_v3(
        &producer,
        &cargo,
        &build_environment,
        &workspace,
        scratch.path(),
    )?;
    if measure_prepared_tree_v3(
        &prepared.root,
        &expected_normalized_rom_sha256,
        &prepared_claims,
    )? != prepared
    {
        return Err(error(
            "prepared tree changed during Cargo or identity child",
        ));
    }
    if sha256_file(&staged, "staged generated runner after identity launch")?
        != selected_binary_sha256
    {
        return Err(error(
            "selected generated runner changed during identity launch",
        ));
    }
    if private_inputs_sha256(&staged_inputs)? != private_build_inputs_sha256 {
        return Err(error(
            "private generated-runner build inputs changed during identity launch",
        ));
    }
    if sha256_file(&manifest, "WM generated-runner manifest after build")? != manifest_sha256
        || sha256_file(&lock, "WM generated-runner lockfile after build")? != lock_sha256
    {
        return Err(error(
            "generated-runner manifest or lockfile changed during the owned build",
        ));
    }
    let metadata_after = run_cargo_metadata(&cargo, &build_environment, &manifest, scratch.path())?;
    if hex(&Sha256::digest(&metadata_after)) != cargo_graph_sha256
        || cargo_metadata_source_sha256(&metadata_after)? != cargo_source_sha256
    {
        return Err(error(
            "generated-runner Cargo graph or package sources changed during the owned build",
        ));
    }
    crate::platform_certification::verified_build_cargo().map_err(|source| {
        error(format!(
            "reverify Cargo owner after identity launch: {source}"
        ))
    })?;
    if validate_memory_guard(&memory_guard)? != memory_guard_sha256 {
        return Err(error(
            "generated-runner process-group memory guard changed during the owned build",
        ));
    }
    build_environment.revalidate()?;
    fs::remove_dir_all(scratch.path().join("build-target")).map_err(|source| {
        error(format!(
            "remove completed generated-runner Cargo target from verifier scratch: {source}"
        ))
    })?;
    fs::remove_dir_all(scratch.path().join("producer-target")).map_err(|source| {
        error(format!(
            "remove completed prepared-producer Cargo target from verifier scratch: {source}"
        ))
    })?;

    let mut evidence = GeneratedRunnerBuildEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V5,
        builder_cargo_sha256,
        cargo_graph_sha256,
        cargo_source_sha256,
        build_environment_sha256: build_environment.identity_sha256,
        builder_rustc_sha256: build_environment.rustc_sha256,
        cargo_config_sha256: build_environment.cargo_config_sha256,
        memory_guard_sha256,
        selected_build_cargo_jobs: SELECTED_BUILD_CARGO_JOBS_V5,
        build_max_rss_mib: BUILD_MAX_RSS_MIB,
        build_min_free_percent: BUILD_MIN_FREE_PERCENT,
        max_build_seconds: staged_inputs.max_build_seconds,
        selected_binary_sha256,
        private_build_inputs_sha256,
        prepared_tree_descriptor_sha256: prepared.descriptor_sha256.clone(),
        prepared_tree_sha256: prepared.tree_sha256.clone(),
        prepared_source_mode: prepared_source_mode.to_owned(),
        producer_manifest_sha256: producer.manifest_sha256.clone(),
        producer_lock_sha256: producer.lock_sha256.clone(),
        producer_cargo_graph_sha256: producer.cargo_graph_sha256.clone(),
        producer_cargo_source_sha256: producer.cargo_source_sha256.clone(),
        producer_binary_sha256: producer.binary_sha256.clone(),
        identity,
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = evidence.recompute_authority_sha256();
    evidence.verify_integrity()?;
    Ok(VerifiedGeneratedRunnerBuildV1 {
        evidence,
        selected_binary: staged,
        private_inputs: staged_inputs,
        prepared,
        producer,
        _scratch: scratch,
    })
}

impl GeneratedRunnerBuildEvidenceV1 {
    fn verify_integrity(&self) -> Result<(), GeneratedRunnerBuildError> {
        if self.schema != VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V5 {
            return Err(error("unsupported verified generated-runner build schema"));
        }
        for (field, digest) in [
            ("builder_cargo_sha256", &self.builder_cargo_sha256),
            ("cargo_graph_sha256", &self.cargo_graph_sha256),
            ("cargo_source_sha256", &self.cargo_source_sha256),
            ("build_environment_sha256", &self.build_environment_sha256),
            ("builder_rustc_sha256", &self.builder_rustc_sha256),
            ("cargo_config_sha256", &self.cargo_config_sha256),
            ("memory_guard_sha256", &self.memory_guard_sha256),
            ("selected_binary_sha256", &self.selected_binary_sha256),
            (
                "private_build_inputs_sha256",
                &self.private_build_inputs_sha256,
            ),
            (
                "prepared_tree_descriptor_sha256",
                &self.prepared_tree_descriptor_sha256,
            ),
            ("prepared_tree_sha256", &self.prepared_tree_sha256),
            ("producer_manifest_sha256", &self.producer_manifest_sha256),
            ("producer_lock_sha256", &self.producer_lock_sha256),
            (
                "producer_cargo_graph_sha256",
                &self.producer_cargo_graph_sha256,
            ),
            (
                "producer_cargo_source_sha256",
                &self.producer_cargo_source_sha256,
            ),
            ("producer_binary_sha256", &self.producer_binary_sha256),
            ("authority_sha256", &self.authority_sha256),
        ] {
            require_sha256(digest, field)?;
        }
        if self.selected_build_cargo_jobs != SELECTED_BUILD_CARGO_JOBS_V5 {
            return Err(error(format!(
                "generated-runner build evidence requires exactly {SELECTED_BUILD_CARGO_JOBS_V5} selected-build Cargo jobs"
            )));
        }
        if self.build_max_rss_mib != BUILD_MAX_RSS_MIB
            || self.build_min_free_percent != BUILD_MIN_FREE_PERCENT
            || !(MIN_BUILD_TIMEOUT_SECONDS..=MAX_BUILD_TIMEOUT_SECONDS)
                .contains(&self.max_build_seconds)
        {
            return Err(error(
                "generated-runner build evidence has a noncanonical safety envelope",
            ));
        }
        if !matches!(
            self.prepared_source_mode.as_str(),
            PREPARED_SOURCE_MODE_INACTIVE_V1 | PREPARED_SOURCE_MODE_CONSUMED_V1
        ) || self.prepared_source_mode != self.identity.prepared_source_mode
        {
            return Err(error(
                "generated-runner build has an invalid prepared source mode",
            ));
        }
        validate_identity(
            &self.identity,
            &self.identity.manifest_sha256,
            &self.identity.lock_sha256,
        )?;
        if self.prepared_tree_sha256 != self.identity.prepared_tree_sha256
            || self.producer_manifest_sha256 != self.identity.producer_manifest_sha256
            || self.producer_lock_sha256 != self.identity.producer_lock_sha256
            || self.producer_cargo_graph_sha256 != self.identity.producer_cargo_graph_sha256
            || self.producer_cargo_source_sha256 != self.identity.producer_cargo_source_sha256
            || self.producer_binary_sha256 != self.identity.producer_binary_sha256
        {
            return Err(error(
                "generated-runner build evidence differs from its child prepared authority",
            ));
        }
        let recomputed = self.recompute_authority_sha256();
        if recomputed != self.authority_sha256 {
            return Err(error(format!(
                "generated-runner build authority digest mismatch: stored={}, recomputed={recomputed}",
                self.authority_sha256
            )));
        }
        Ok(())
    }

    fn recompute_authority_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"fn64.verified-generated-runner-build.v5\0");
        for bytes in [
            self.schema.as_bytes(),
            self.builder_cargo_sha256.as_bytes(),
            self.cargo_graph_sha256.as_bytes(),
            self.cargo_source_sha256.as_bytes(),
            self.build_environment_sha256.as_bytes(),
            self.builder_rustc_sha256.as_bytes(),
            self.cargo_config_sha256.as_bytes(),
            self.memory_guard_sha256.as_bytes(),
            self.selected_binary_sha256.as_bytes(),
            self.private_build_inputs_sha256.as_bytes(),
            self.prepared_tree_descriptor_sha256.as_bytes(),
            self.prepared_tree_sha256.as_bytes(),
            self.producer_manifest_sha256.as_bytes(),
            self.producer_lock_sha256.as_bytes(),
            self.producer_cargo_graph_sha256.as_bytes(),
            self.producer_cargo_source_sha256.as_bytes(),
            self.producer_binary_sha256.as_bytes(),
            self.prepared_source_mode.as_bytes(),
        ] {
            push_bytes(&mut digest, bytes);
        }
        digest.update(self.selected_build_cargo_jobs.to_be_bytes());
        digest.update(self.build_max_rss_mib.to_be_bytes());
        digest.update([self.build_min_free_percent]);
        digest.update(self.max_build_seconds.to_be_bytes());
        let identity = serde_json::to_vec(&self.identity)
            .expect("generated-runner build identity serialization is infallible");
        push_bytes(&mut digest, &identity);
        hex(&digest.finalize())
    }
}

fn validate_identity(
    identity: &GeneratedRunnerBuildIdentityV1,
    expected_manifest_sha256: &str,
    expected_lock_sha256: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    if identity.schema != GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V3
        || identity.package != PACKAGE
        || identity.source_attestation_schema
            != fn64_recomp_rs::GENERATED_RUNNER_SOURCE_ATTESTATION_SCHEMA_V2
    {
        return Err(error(
            "generated-runner child reported an unsupported identity envelope",
        ));
    }
    if identity.manifest_sha256 != expected_manifest_sha256
        || identity.lock_sha256 != expected_lock_sha256
    {
        return Err(error(
            "generated-runner child manifest/lock identity does not match the verifier-owned build",
        ));
    }
    if !identity.cargo_source_fields_validated {
        return Err(error(
            "generated-runner child did not validate its Cargo source fields",
        ));
    }
    if !matches!(
        identity.prepared_source_mode.as_str(),
        PREPARED_SOURCE_MODE_INACTIVE_V1 | PREPARED_SOURCE_MODE_CONSUMED_V1
    ) {
        return Err(error(
            "generated-runner child has an invalid prepared source mode",
        ));
    }
    for (field, digest) in [
        ("manifest_sha256", &identity.manifest_sha256),
        ("lock_sha256", &identity.lock_sha256),
        ("program_identity_sha256", &identity.program_identity_sha256),
        (
            "root_adapter_source_sha256",
            &identity.root_adapter_source_sha256,
        ),
        (
            "shard_cargo_source_tree_sha256",
            &identity.shard_cargo_source_tree_sha256,
        ),
        ("emitter_source_sha256", &identity.emitter_source_sha256),
        ("runtime_source_sha256", &identity.runtime_source_sha256),
        ("normalized_rom_sha256", &identity.normalized_rom_sha256),
        (
            "prepared_manifest_sha256",
            &identity.prepared_manifest_sha256,
        ),
        ("prepared_tree_sha256", &identity.prepared_tree_sha256),
        (
            "prepared_generator_source_sha256",
            &identity.prepared_generator_source_sha256,
        ),
        (
            "prepared_discovery_source_sha256",
            &identity.prepared_discovery_source_sha256,
        ),
        (
            "prepared_emitter_source_sha256",
            &identity.prepared_emitter_source_sha256,
        ),
        (
            "prepared_runtime_source_sha256",
            &identity.prepared_runtime_source_sha256,
        ),
        (
            "prepared_materializer_source_sha256",
            &identity.prepared_materializer_source_sha256,
        ),
        (
            "producer_manifest_sha256",
            &identity.producer_manifest_sha256,
        ),
        ("producer_lock_sha256", &identity.producer_lock_sha256),
        (
            "producer_cargo_graph_sha256",
            &identity.producer_cargo_graph_sha256,
        ),
        (
            "producer_cargo_source_sha256",
            &identity.producer_cargo_source_sha256,
        ),
        ("producer_binary_sha256", &identity.producer_binary_sha256),
        ("binding_sha256", &identity.binding_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if identity.build_receipt_schema != 1
        || !identity.aot_runtime
        || !identity.production_aot
        || identity.dev_interpreter
    {
        return Err(error(
            "selected generated runner is not the production-AOT feature artifact",
        ));
    }
    if identity.runners.is_empty() {
        return Err(error("generated-runner child reported no linked runners"));
    }
    let mut prior = None;
    for runner in &identity.runners {
        if prior.is_some_and(|bank| bank >= runner.bank) {
            return Err(error(
                "generated-runner identities are not in strictly increasing bank order",
            ));
        }
        prior = Some(runner.bank);
        require_sha256(
            &runner.generated_runner_source_sha256,
            "runners[].generated_runner_source_sha256",
        )?;
        require_sha256(&runner.code_words_sha256, "runners[].code_words_sha256")?;
        if runner.vram_start & 3 != 0
            || runner.vram_end & 3 != 0
            || runner.vram_start >= runner.vram_end
            || runner.composite_subrunner_count == 0
        {
            return Err(error(
                "generated-runner child reported invalid code geometry",
            ));
        }
    }
    let recomputed = recompute_binding_sha256(identity)?;
    if recomputed != identity.binding_sha256 {
        return Err(error(format!(
            "generated-runner binding digest mismatch: child={}, recomputed={recomputed}",
            identity.binding_sha256
        )));
    }
    Ok(())
}

fn validate_prepared_identity_v3(
    identity: &GeneratedRunnerBuildIdentityV1,
    prepared: &PreparedTreeMeasurementV3,
    producer: &ProducerBuildMeasurementV3,
    prepared_source_mode: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    if identity.prepared_source_mode != prepared_source_mode {
        return Err(error(
            "generated-runner child source mode differs from the exact shard manifests",
        ));
    }
    let pairs = [
        (
            &identity.normalized_rom_sha256,
            &prepared.normalized_rom_sha256,
        ),
        (
            &identity.prepared_manifest_sha256,
            &prepared.manifest_sha256,
        ),
        (&identity.prepared_tree_sha256, &prepared.tree_sha256),
        (
            &identity.prepared_generator_source_sha256,
            &prepared.claims.generator_source_sha256,
        ),
        (
            &identity.prepared_discovery_source_sha256,
            &prepared.claims.discovery_source_sha256,
        ),
        (
            &identity.prepared_emitter_source_sha256,
            &prepared.claims.emitter_source_sha256,
        ),
        (
            &identity.prepared_runtime_source_sha256,
            &prepared.claims.runtime_source_sha256,
        ),
        (
            &identity.prepared_materializer_source_sha256,
            &prepared.claims.materializer_source_sha256,
        ),
        (
            &identity.producer_manifest_sha256,
            &producer.manifest_sha256,
        ),
        (&identity.producer_lock_sha256, &producer.lock_sha256),
        (
            &identity.producer_cargo_graph_sha256,
            &producer.cargo_graph_sha256,
        ),
        (
            &identity.producer_cargo_source_sha256,
            &producer.cargo_source_sha256,
        ),
        (&identity.producer_binary_sha256, &producer.binary_sha256),
    ];
    if pairs
        .iter()
        .any(|(observed, expected)| observed != expected)
    {
        return Err(error(
            "generated-runner child prepared identity differs from verifier measurements",
        ));
    }
    Ok(())
}

fn recompute_binding_sha256(
    identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(fn64_recomp_rs::GENERATED_RUNNER_SOURCE_BINDING_DOMAIN_V2);
    for value in [
        &identity.program_identity_sha256,
        &identity.root_adapter_source_sha256,
        &identity.shard_cargo_source_tree_sha256,
        &identity.emitter_source_sha256,
        &identity.runtime_source_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    for runner in &identity.runners {
        digest.update(runner.bank.to_be_bytes());
        digest.update(decode_sha256(&runner.generated_runner_source_sha256)?);
        digest.update(decode_sha256(&runner.code_words_sha256)?);
        digest.update(runner.vram_start.to_be_bytes());
        digest.update(runner.vram_end.to_be_bytes());
        digest.update(runner.composite_subrunner_count.to_be_bytes());
        digest.update([runner.adapter_role.tag()]);
    }
    digest.update(identity.build_receipt_schema.to_be_bytes());
    digest.update([
        u8::from(identity.aot_runtime),
        u8::from(identity.production_aot),
        u8::from(identity.dev_interpreter),
    ]);
    Ok(hex(&digest.finalize()))
}

fn run_cargo_metadata(
    cargo: &Path,
    environment: &BuildEnvironmentV3,
    manifest: &Path,
    scratch: &Path,
) -> Result<Vec<u8>, GeneratedRunnerBuildError> {
    let mut command = Command::new(cargo);
    environment.apply(&mut command);
    let output = command
        .arg("metadata")
        .arg("--frozen")
        .arg("--format-version=1")
        .arg("--manifest-path")
        .arg(manifest)
        .current_dir(scratch)
        .output()
        .map_err(|source| error(format!("run frozen Cargo metadata: {source}")))?;
    fs::write(scratch.join("cargo-metadata.stderr.log"), &output.stderr)
        .map_err(|source| error(format!("write Cargo metadata log: {source}")))?;
    if !output.status.success() {
        return Err(error(format!(
            "frozen Cargo metadata failed {}; stderr: {}",
            output.status,
            bounded_diagnostic(&output.stderr),
        )));
    }
    Ok(output.stdout)
}

fn bounded_diagnostic(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    if text.is_empty() {
        "<empty>".to_owned()
    } else {
        let mut tail = text.chars().rev().take(4096).collect::<Vec<_>>();
        let truncated = tail.len() < text.chars().count();
        tail.reverse();
        let diagnostic: String = tail.into_iter().collect();
        if truncated {
            format!("<earlier output truncated>\n{diagnostic}")
        } else {
            diagnostic
        }
    }
}

fn bounded_diagnostic_file(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => bounded_diagnostic(&bytes),
        Err(source) => format!("<cannot read diagnostic: {source}>"),
    }
}

fn cargo_build_progress(bytes: &[u8]) -> String {
    let Ok(source) = std::str::from_utf8(bytes) else {
        return "compiler_artifacts=unreadable".to_owned();
    };
    let expected = PREPARED_PACKAGES
        .iter()
        .map(|package| package.replace('-', "_"))
        .collect::<BTreeSet<_>>();
    let mut completed_shards = BTreeSet::new();
    let mut compiler_artifacts = 0usize;
    let mut root_binary = false;
    for line in source.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        compiler_artifacts += 1;
        let Some(name) = message["target"]["name"].as_str() else {
            continue;
        };
        let kinds = message["target"]["kind"].as_array();
        if expected.contains(name)
            && kinds.is_some_and(|kinds| kinds.iter().any(|kind| kind == "lib"))
        {
            completed_shards.insert(name.to_owned());
        }
        if name == PACKAGE && kinds.is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin")) {
            root_binary = true;
        }
    }
    format!(
        "compiler_artifacts={compiler_artifacts} completed_shards={}/{} root_binary={}",
        completed_shards.len(),
        PREPARED_PACKAGES.len(),
        u8::from(root_binary),
    )
}

fn cargo_metadata_source_sha256(metadata: &[u8]) -> Result<String, GeneratedRunnerBuildError> {
    let document: serde_json::Value = serde_json::from_slice(metadata)
        .map_err(|source| error(format!("parse Cargo metadata: {source}")))?;
    let packages = document["packages"]
        .as_array()
        .ok_or_else(|| error("Cargo metadata has no packages array"))?;
    let mut roots = packages
        .iter()
        .map(|package| {
            let id = package["id"]
                .as_str()
                .ok_or_else(|| error("Cargo metadata package has no id"))?;
            let manifest = package["manifest_path"]
                .as_str()
                .ok_or_else(|| error("Cargo metadata package has no manifest_path"))?;
            let root = PathBuf::from(manifest)
                .parent()
                .ok_or_else(|| error("Cargo package manifest has no parent"))?
                .to_path_buf();
            Ok((id.to_owned(), root))
        })
        .collect::<Result<Vec<_>, GeneratedRunnerBuildError>>()?;
    roots.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    digest.update(b"fn64.generated-runner-cargo-source-graph.v1\0");
    for (id, root) in roots {
        push_bytes(&mut digest, id.as_bytes());
        let mut files = Vec::new();
        collect_package_files(&root, &root, &mut files)?;
        files.sort();
        for file in files {
            let relative = file
                .strip_prefix(&root)
                .expect("collected package file remains under root");
            push_bytes(
                &mut digest,
                relative.to_string_lossy().replace('\\', "/").as_bytes(),
            );
            let source = crate::private_fs::read_regular_stable(&file, "Cargo package source")
                .map_err(error)?;
            push_bytes(&mut digest, &source.contents);
        }
    }
    Ok(hex(&digest.finalize()))
}

fn wm_root_adapter_source_sha256(package_root: &Path) -> Result<String, GeneratedRunnerBuildError> {
    source_tree_sha256(
        package_root,
        b"fn64:wm2000-root-adapter-source:v1:",
        &["Cargo.toml", "Cargo.lock", "build.rs", "src/main.rs"],
    )
}

fn wm_shard_root(package_root: &Path) -> Result<PathBuf, GeneratedRunnerBuildError> {
    Ok(package_root
        .parent()
        .ok_or_else(|| error("WM root package has no examples parent"))?
        .join("wm2000-block-shards"))
}

fn wm_shard_cargo_source_sha256(
    package_root: &Path,
    prepared_source_mode: &str,
) -> Result<String, GeneratedRunnerBuildError> {
    let shard_root = wm_shard_root(package_root)?;
    let mut files = vec![(
        "../wm2000-block-shards/lib.rs".to_owned(),
        shard_root.join("lib.rs"),
    )];
    match prepared_source_mode {
        PREPARED_SOURCE_MODE_INACTIVE_V1 => {
            files.push((
                "../wm2000-block-shards/build.rs".to_owned(),
                shard_root.join("build.rs"),
            ));
        }
        PREPARED_SOURCE_MODE_CONSUMED_V1 => {
            files.push((
                "../wm2000-block-shards/prepared_build.rs".to_owned(),
                shard_root.join("prepared_build.rs"),
            ));
            files.push((
                "../wm2000-block-shards/materializer.rs".to_owned(),
                shard_root.join("materializer.rs"),
            ));
        }
        _ => return Err(error("unsupported prepared source mode")),
    }
    let manifests = exact_shard_manifests(&shard_root)?;
    for manifest in manifests {
        let relative = manifest.strip_prefix(&shard_root).map_err(|_| {
            error(format!(
                "WM shard manifest escaped shard source graph: {}",
                manifest.display()
            ))
        })?;
        files.push((
            format!(
                "../wm2000-block-shards/{}",
                relative.to_string_lossy().replace('\\', "/")
            ),
            manifest,
        ));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    digest.update(b"fn64:wm2000-shard-cargo-source-tree:v1:");
    for (label, path) in files {
        push_bytes(&mut digest, label.as_bytes());
        let source = crate::private_fs::read_regular_stable(&path, "WM shard Cargo source")
            .map_err(error)?;
        push_bytes(&mut digest, &source.contents);
    }
    Ok(hex(&digest.finalize()))
}

fn wm_prepared_source_mode_v3(
    package_root: &Path,
) -> Result<&'static str, GeneratedRunnerBuildError> {
    let shard_root = wm_shard_root(package_root)?;
    let manifests = exact_shard_manifests(&shard_root)?;
    let mut legacy = 0usize;
    let mut prepared = 0usize;
    for manifest in manifests {
        let source = crate::private_fs::read_regular_stable(&manifest, "WM shard manifest")
            .map_err(error)?;
        let text = std::str::from_utf8(&source.contents)
            .map_err(|source| error(format!("WM shard manifest is not UTF-8: {source}")))?;
        legacy += usize::from(
            text.lines()
                .filter(|line| line.trim() == "build = \"../build.rs\"")
                .count()
                == 1,
        );
        prepared += usize::from(
            text.lines()
                .filter(|line| line.trim() == "build = \"../prepared_build.rs\"")
                .count()
                == 1,
        );
    }
    match (legacy, prepared) {
        (35, 0) => Ok(PREPARED_SOURCE_MODE_INACTIVE_V1),
        (0, 35) => Ok(PREPARED_SOURCE_MODE_CONSUMED_V1),
        _ => Err(error(
            "WM shard manifests mix or omit legacy/prepared source modes",
        )),
    }
}

fn exact_shard_manifests(shard_root: &Path) -> Result<Vec<PathBuf>, GeneratedRunnerBuildError> {
    let expected = SHARD_MANIFEST_DIRS
        .iter()
        .map(|directory| shard_root.join(directory).join("Cargo.toml"))
        .collect::<Vec<_>>();
    if expected.iter().any(|path| !path.is_file()) {
        return Err(error(
            "WM shard manifest inventory is missing an expected package",
        ));
    }
    let mut observed = fs::read_dir(shard_root)
        .map_err(|source| error(format!("enumerate WM shard manifests: {source}")))?
        .map(|entry| {
            entry
                .map(|entry| entry.path().join("Cargo.toml"))
                .map_err(|source| error(format!("enumerate WM shard manifest: {source}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    observed.retain(|path| path.is_file());
    observed.sort();
    let mut expected_sorted = expected.clone();
    expected_sorted.sort();
    if observed != expected_sorted {
        return Err(error("WM shard manifest inventory has an extra package"));
    }
    for (path, package) in expected.iter().zip(PREPARED_PACKAGES) {
        let source =
            crate::private_fs::read_regular_stable(path, "WM shard manifest").map_err(error)?;
        let expected_name = format!("name = \"{package}\"");
        if std::str::from_utf8(&source.contents)
            .map_err(|source| error(format!("WM shard manifest is not UTF-8: {source}")))?
            .lines()
            .filter(|line| line.trim() == expected_name)
            .count()
            != 1
        {
            return Err(error(
                "WM shard manifest path/package mapping is noncanonical",
            ));
        }
    }
    Ok(expected)
}

fn source_tree_sha256(
    root: &Path,
    domain: &[u8],
    labels: &[&str],
) -> Result<String, GeneratedRunnerBuildError> {
    let mut labels = labels.to_vec();
    labels.sort_unstable();
    let mut digest = Sha256::new();
    digest.update(domain);
    for label in labels {
        let path = root.join(label);
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            error(format!(
                "inspect generated-runner source {}: {source}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(error(format!(
                "generated-runner source must be a regular non-symlink file: {}",
                path.display()
            )));
        }
        let bytes = crate::private_fs::read_regular_stable(&path, "generated-runner source")
            .map_err(error)?
            .contents;
        push_bytes(&mut digest, label.as_bytes());
        push_bytes(&mut digest, &bytes);
    }
    Ok(hex(&digest.finalize()))
}

fn collect_package_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), GeneratedRunnerBuildError> {
    for entry in fs::read_dir(directory).map_err(|source| {
        error(format!(
            "enumerate Cargo source {}: {source}",
            directory.display()
        ))
    })? {
        let path = entry
            .map_err(|source| {
                error(format!(
                    "enumerate Cargo source {}: {source}",
                    directory.display()
                ))
            })?
            .path();
        let relative = path
            .strip_prefix(root)
            .expect("package entry remains under root");
        if relative
            .components()
            .any(|component| matches!(component.as_os_str().to_str(), Some("target" | ".git")))
        {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            error(format!("inspect Cargo source {}: {source}", path.display()))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(error(format!(
                "Cargo package source contains symlink {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            collect_package_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn prepared_source_claims_v3(
    workspace: &Path,
) -> Result<PreparedSourceClaimsV3, GeneratedRunnerBuildError> {
    let shard_root = workspace.join("examples/wm2000-block-shards");
    Ok(PreparedSourceClaimsV3 {
        generator_source_sha256: source_tree_sha256(
            &shard_root,
            b"fn64.wm-prepared-generator-source.v1\0",
            &[
                "build.rs",
                "materializer.rs",
                "prepared_build.rs",
                "prepared_tree.rs",
                "producer.rs",
            ],
        )?,
        discovery_source_sha256: package_source_tree_sha256(
            &workspace.join("crates/fn64-discover"),
            b"fn64.wm-prepared-discovery-source.v1\0",
        )?,
        emitter_source_sha256: wm_emitter_source_sha256(workspace)?,
        runtime_source_sha256: hex(
            &fn64_recomp_rs::generated_runner_runtime_source_receipt_v1().source_sha256(),
        ),
        materializer_source_sha256: sha256_file(
            &shard_root.join("materializer.rs"),
            "WM prepared materializer source",
        )?,
    })
}

fn wm_emitter_source_sha256(workspace: &Path) -> Result<String, GeneratedRunnerBuildError> {
    let root = workspace.join("crates/fn64-recomp-rs-codegen");
    let mut digest = Sha256::new();
    digest.update(b"fn64:generated-runner-emitter-source:v2:");
    // This order is part of GeneratedRunnerEmitterSourceReceiptV2's wire.
    // The generic source-tree helper sorts labels and therefore cannot measure
    // this receipt independently without changing its digest.
    for label in ["Cargo.toml", "src/lib.rs", "src/emit.rs"] {
        push_bytes(&mut digest, label.as_bytes());
        let source = crate::private_fs::read_regular_stable(
            &root.join(label),
            "generated-runner emitter source",
        )
        .map_err(error)?;
        push_bytes(&mut digest, &source.contents);
    }
    Ok(hex(&digest.finalize()))
}

fn package_source_tree_sha256(
    root: &Path,
    domain: &[u8],
) -> Result<String, GeneratedRunnerBuildError> {
    let mut files = Vec::new();
    collect_package_files(root, root, &mut files)?;
    files.sort();
    let mut digest = Sha256::new();
    digest.update(domain);
    for file in files {
        let relative = file
            .strip_prefix(root)
            .expect("collected package source remains below root");
        push_bytes(
            &mut digest,
            relative.to_string_lossy().replace('\\', "/").as_bytes(),
        );
        let source =
            crate::private_fs::read_regular_stable(&file, "package source").map_err(error)?;
        push_bytes(&mut digest, &source.contents);
    }
    Ok(hex(&digest.finalize()))
}

fn producer_cargo_source_sha256_v3(
    metadata_source_sha256: &str,
    workspace: &Path,
) -> Result<String, GeneratedRunnerBuildError> {
    let external_sources = source_tree_sha256(
        &workspace.join("examples/wm2000-block-shards"),
        b"fn64.wm-prepared-producer-external-sources.v1\0",
        &["build.rs", "prepared_tree.rs", "producer.rs"],
    )?;
    let mut digest = Sha256::new();
    digest.update(b"fn64.wm-prepared-producer-cargo-source-graph.v1\0");
    digest.update(decode_sha256(metadata_source_sha256)?);
    digest.update(decode_sha256(&external_sources)?);
    Ok(hex(&digest.finalize()))
}

fn normalized_rom_sha256(path: &Path) -> Result<String, GeneratedRunnerBuildError> {
    let source = crate::private_fs::read_regular_stable(path, "staged WM ROM").map_err(error)?;
    let bytes = normalize_n64_rom_bytes(&source.contents)?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn normalize_n64_rom_bytes(source: &[u8]) -> Result<Vec<u8>, GeneratedRunnerBuildError> {
    if source.len() < 0x40 || source.len() % 4 != 0 {
        return Err(error("staged WM ROM is too small or not word aligned"));
    }
    let magic = u32::from_be_bytes(source[..4].try_into().expect("ROM header is four bytes"));
    match magic {
        0x8037_1240 => Ok(source.to_vec()),
        0x4012_3780 => Ok(source
            .chunks_exact(4)
            .flat_map(|word| [word[3], word[2], word[1], word[0]])
            .collect()),
        0x3780_4012 => Ok(source
            .chunks_exact(2)
            .flat_map(|pair| [pair[1], pair[0]])
            .collect()),
        _ => Err(error("staged WM ROM has an unknown byte-order magic")),
    }
}

fn measure_prepared_tree_v3(
    root: &Path,
    expected_rom: &str,
    expected_claims: &PreparedSourceClaimsV3,
) -> Result<PreparedTreeMeasurementV3, GeneratedRunnerBuildError> {
    validate_input_path(root, "prepared shard root")?;
    require_private_entry(root, true, "prepared shard root")?;
    let expected_root = BTreeSet::from_iter(
        std::iter::once(PREPARED_MANIFEST_NAME.to_owned()).chain(
            PREPARED_PACKAGES
                .iter()
                .map(|package| (*package).to_owned()),
        ),
    );
    let root_entries = exact_directory_entries(root, "prepared shard root")?;
    if root_entries != expected_root || root_entries.contains(PREPARED_UPDATE_MARKER_NAME) {
        return Err(error(
            "prepared shard root does not contain exactly manifest.v2 and 35 package directories",
        ));
    }

    let manifest_path = root.join(PREPARED_MANIFEST_NAME);
    require_private_entry(&manifest_path, false, "prepared root manifest")?;
    let manifest = crate::private_fs::read_regular_stable(&manifest_path, "prepared root manifest")
        .map_err(error)?;
    let manifest_text = std::str::from_utf8(&manifest.contents)
        .map_err(|source| error(format!("prepared root manifest is not UTF-8: {source}")))?;
    if !manifest_text.ends_with('\n') || manifest_text.contains("\r") {
        return Err(error("prepared root manifest is not canonical LF text"));
    }
    let lines = manifest_text.lines().collect::<Vec<_>>();
    if lines.len() != 7 + PREPARED_PACKAGES.len()
        || lines[0] != "schema fn64.wm-prepared-shard-tree.v2"
        || lines[6] != "artifact_count 35"
    {
        return Err(error("prepared root manifest has a noncanonical shape"));
    }
    let normalized_rom_sha256 = parse_manifest_digest(lines[1], "normalized_rom_sha256")?;
    let claims = PreparedSourceClaimsV3 {
        generator_source_sha256: parse_manifest_digest(lines[2], "generator_source_sha256")?,
        discovery_source_sha256: parse_manifest_digest(lines[3], "discovery_source_sha256")?,
        emitter_source_sha256: parse_manifest_digest(lines[4], "emitter_source_sha256")?,
        runtime_source_sha256: parse_manifest_digest(lines[5], "runtime_source_sha256")?,
        materializer_source_sha256: expected_claims.materializer_source_sha256.clone(),
    };
    if normalized_rom_sha256 != expected_rom || &claims != expected_claims {
        return Err(error(
            "prepared root ROM or source claims differ from verifier measurements",
        ));
    }

    let mut tree = Sha256::new();
    tree.update(b"fn64.wm-prepared-shard-complete-tree.v1\0");
    let mut descriptors = Sha256::new();
    descriptors.update(b"fn64.wm-prepared-shard-descriptors.v1\0");
    hash_directory_descriptor(&mut descriptors, root, ".")?;
    hash_stable_measurement(
        &mut tree,
        &mut descriptors,
        PREPARED_MANIFEST_NAME,
        &manifest.measurement,
    )?;
    for (index, package) in PREPARED_PACKAGES.iter().enumerate() {
        let package_root = root.join(package);
        require_private_entry(&package_root, true, "prepared package directory")?;
        hash_directory_descriptor(&mut descriptors, &package_root, package)?;
        if exact_directory_entries(&package_root, "prepared package directory")?
            != BTreeSet::from([
                "identity.v1".to_owned(),
                "metadata.rs".to_owned(),
                "runner.rs".to_owned(),
            ])
        {
            return Err(error("prepared package has noncanonical topology"));
        }
        let mut measured = Vec::new();
        for name in ["identity.v1", "runner.rs", "metadata.rs"] {
            let path = package_root.join(name);
            require_private_entry(&path, false, "prepared package artifact")?;
            let measurement =
                crate::private_fs::read_regular_stable(&path, "prepared package artifact")
                    .map_err(error)?;
            let label = format!("{package}/{name}");
            hash_stable_measurement(
                &mut tree,
                &mut descriptors,
                &label,
                &measurement.measurement,
            )?;
            measured.push((name, measurement));
        }
        let identity = &measured[0].1;
        let runner = &measured[1].1;
        let metadata = &measured[2].1;
        validate_prepared_sidecar(
            &identity.contents,
            package,
            &runner.measurement.sha256,
            &metadata.measurement.sha256,
        )?;
        let expected_line = format!(
            "artifact {package} {} {} {}",
            identity.measurement.sha256, runner.measurement.sha256, metadata.measurement.sha256,
        );
        if lines[7 + index] != expected_line {
            return Err(error(
                "prepared root manifest artifact line differs from measured package",
            ));
        }
    }
    Ok(PreparedTreeMeasurementV3 {
        root: root.to_path_buf(),
        normalized_rom_sha256,
        manifest_sha256: manifest.measurement.sha256,
        tree_sha256: hex(&tree.finalize()),
        descriptor_sha256: hex(&descriptors.finalize()),
        claims,
    })
}

fn parse_manifest_digest(line: &str, field: &str) -> Result<String, GeneratedRunnerBuildError> {
    let value = line
        .strip_prefix(field)
        .and_then(|rest| rest.strip_prefix(' '))
        .ok_or_else(|| error(format!("prepared manifest is missing canonical {field}")))?;
    require_sha256(value, field)?;
    if value == "0".repeat(64) {
        return Err(error(format!("prepared manifest {field} is zero")));
    }
    Ok(value.to_owned())
}

fn validate_prepared_sidecar(
    bytes: &[u8],
    package: &str,
    runner_sha256: &str,
    metadata_sha256: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    let expected = format!(
        "schema fn64.wm-prepared-shard-artifact.v1\npackage {package}\nrunner_sha256 {runner_sha256}\nmetadata_sha256 {metadata_sha256}\n"
    );
    if bytes != expected.as_bytes() {
        return Err(error("prepared package identity sidecar is noncanonical"));
    }
    Ok(())
}

fn exact_directory_entries(
    path: &Path,
    label: &str,
) -> Result<BTreeSet<String>, GeneratedRunnerBuildError> {
    fs::read_dir(path)
        .map_err(|source| error(format!("enumerate {label}: {source}")))?
        .map(|entry| {
            entry
                .map_err(|source| error(format!("enumerate {label}: {source}")))?
                .file_name()
                .into_string()
                .map_err(|_| error(format!("{label} contains a non-UTF-8 name")))
        })
        .collect()
}

fn require_private_entry(
    path: &Path,
    directory: bool,
    label: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| error(format!("inspect {label}: {source}")))?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(error(format!("{label} has the wrong filesystem type")));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let expected = if directory { 0o700 } else { 0o600 };
        if metadata.permissions().mode() & 0o777 != expected {
            return Err(error(format!("{label} must have mode {expected:o}")));
        }
    }
    Ok(())
}

fn hash_stable_measurement(
    tree: &mut Sha256,
    descriptors: &mut Sha256,
    label: &str,
    measurement: &crate::private_fs::StableFileMeasurement,
) -> Result<(), GeneratedRunnerBuildError> {
    push_bytes(tree, label.as_bytes());
    tree.update(measurement.bytes.to_be_bytes());
    tree.update(decode_sha256(&measurement.sha256)?);
    push_bytes(descriptors, label.as_bytes());
    descriptors.update(measurement.bytes.to_be_bytes());
    descriptors.update(measurement.unix_mode.unwrap_or(0).to_be_bytes());
    match &measurement.object_id {
        #[cfg(unix)]
        crate::private_fs::StableObjectId::Unix { device, inode } => {
            descriptors.update([1]);
            descriptors.update(device.to_be_bytes());
            descriptors.update(inode.to_be_bytes());
        }
        #[cfg(windows)]
        crate::private_fs::StableObjectId::Windows {
            volume_serial_number,
            file_id,
        } => {
            descriptors.update([2]);
            descriptors.update(volume_serial_number.to_be_bytes());
            descriptors.update(file_id);
        }
    }
    Ok(())
}

fn hash_directory_descriptor(
    descriptors: &mut Sha256,
    path: &Path,
    label: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    let before =
        crate::private_fs::check_directory_nofollow(path, "prepared directory").map_err(error)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| error(format!("inspect prepared directory: {source}")))?;
    let after =
        crate::private_fs::check_directory_nofollow(path, "prepared directory").map_err(error)?;
    if before.object_id != after.object_id {
        return Err(error(
            "prepared directory changed during descriptor measurement",
        ));
    }
    push_bytes(descriptors, label.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        descriptors.update(metadata.permissions().mode().to_be_bytes());
    }
    #[cfg(windows)]
    descriptors.update(0u32.to_be_bytes());
    match before.object_id {
        #[cfg(unix)]
        crate::private_fs::StableObjectId::Unix { device, inode } => {
            descriptors.update([3]);
            descriptors.update(device.to_be_bytes());
            descriptors.update(inode.to_be_bytes());
        }
        #[cfg(windows)]
        crate::private_fs::StableObjectId::Windows {
            volume_serial_number,
            file_id,
        } => {
            descriptors.update([4]);
            descriptors.update(volume_serial_number.to_be_bytes());
            descriptors.update(file_id);
        }
    }
    Ok(())
}

fn build_prepared_producer_v3(
    memory_guard: &Path,
    cargo: &Path,
    environment: &BuildEnvironmentV3,
    workspace: &Path,
    scratch: &Path,
    max_build_seconds: u64,
) -> Result<ProducerBuildMeasurementV3, GeneratedRunnerBuildError> {
    let package_root = workspace.join("examples/wm2000-prepared-shard-producer");
    let manifest = package_root.join("Cargo.toml");
    let lock = package_root.join("Cargo.lock");
    let manifest_sha256 = sha256_file(&manifest, "prepared producer manifest")?;
    let lock_sha256 = sha256_file(&lock, "prepared producer lockfile")?;
    let metadata = run_cargo_metadata(cargo, environment, &manifest, scratch)?;
    let cargo_graph_sha256 = hex(&Sha256::digest(&metadata));
    let cargo_source_sha256 =
        producer_cargo_source_sha256_v3(&cargo_metadata_source_sha256(&metadata)?, workspace)?;
    let stdout_path = scratch.join("producer-build.stdout.jsonl");
    let stderr_path = scratch.join("producer-build.stderr.log");
    let mut command = Command::new(memory_guard);
    environment.apply(&mut command);
    command
        .arg(cargo)
        .arg("build")
        .arg("-j1")
        .arg("--frozen")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--target-dir")
        .arg(scratch.join("producer-target"))
        .arg("--package")
        .arg(PRODUCER_PACKAGE)
        .arg("--bin")
        .arg(PRODUCER_PACKAGE)
        .arg("--message-format=json-render-diagnostics")
        .current_dir(scratch)
        .env("CARGO_BUILD_JOBS", "1")
        .env("FN64_GUARD_MAX_RSS_MIB", BUILD_MAX_RSS_MIB.to_string())
        .env(
            "FN64_GUARD_MIN_FREE_PERCENT",
            BUILD_MIN_FREE_PERCENT.to_string(),
        )
        .env("FN64_GUARD_MAX_SECONDS", max_build_seconds.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(create_new(&stdout_path)?))
        .stderr(Stdio::from(create_new(&stderr_path)?));
    let status = command
        .status()
        .map_err(|source| error(format!("run frozen prepared producer build: {source}")))?;
    if !status.success() {
        return Err(error(format!(
            "prepared producer build exited {status}; stderr: {}",
            bounded_diagnostic_file(&stderr_path),
        )));
    }
    let selected = select_named_compiler_artifact(
        &fs::read(&stdout_path)
            .map_err(|source| error(format!("read producer artifact stream: {source}")))?,
        PRODUCER_PACKAGE,
    )?;
    let binary_sha256 = sha256_file(&selected, "selected prepared producer")?;
    let binary = stage_executable(
        &selected,
        &scratch.join("selected-prepared-producer"),
        &binary_sha256,
        "prepared producer",
    )?;
    let metadata_after = run_cargo_metadata(cargo, environment, &manifest, scratch)?;
    if sha256_file(&manifest, "prepared producer manifest after build")? != manifest_sha256
        || sha256_file(&lock, "prepared producer lockfile after build")? != lock_sha256
        || hex(&Sha256::digest(&metadata_after)) != cargo_graph_sha256
        || producer_cargo_source_sha256_v3(
            &cargo_metadata_source_sha256(&metadata_after)?,
            workspace,
        )? != cargo_source_sha256
    {
        return Err(error(
            "prepared producer manifest, lock, or frozen source graph changed during build",
        ));
    }
    Ok(ProducerBuildMeasurementV3 {
        manifest_sha256,
        lock_sha256,
        cargo_graph_sha256,
        cargo_source_sha256,
        binary_sha256,
        binary,
    })
}

fn invoke_prepared_producer_v3(
    memory_guard: &Path,
    producer: &ProducerBuildMeasurementV3,
    environment: &BuildEnvironmentV3,
    rom: &Path,
    claims: &PreparedSourceClaimsV3,
    expected_rom: &str,
    scratch: &Path,
    max_build_seconds: u64,
) -> Result<PreparedTreeMeasurementV3, GeneratedRunnerBuildError> {
    let root = scratch.join("prepared-shards");
    let stdout_path = scratch.join("producer.stdout.log");
    let stderr_path = scratch.join("producer.stderr.log");
    let mut command = Command::new(memory_guard);
    environment.apply(&mut command);
    command
        .arg(&producer.binary)
        .arg("--rom")
        .arg(rom)
        .arg("--output")
        .arg(&root)
        .arg("--generator-source-sha256")
        .arg(&claims.generator_source_sha256)
        .arg("--discovery-source-sha256")
        .arg(&claims.discovery_source_sha256)
        .arg("--emitter-source-sha256")
        .arg(&claims.emitter_source_sha256)
        .arg("--runtime-source-sha256")
        .arg(&claims.runtime_source_sha256)
        .env("FN64_GUARD_MAX_RSS_MIB", BUILD_MAX_RSS_MIB.to_string())
        .env(
            "FN64_GUARD_MIN_FREE_PERCENT",
            BUILD_MIN_FREE_PERCENT.to_string(),
        )
        .env("FN64_GUARD_MAX_SECONDS", max_build_seconds.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(create_new(&stdout_path)?))
        .stderr(Stdio::from(create_new(&stderr_path)?));
    let status = command
        .status()
        .map_err(|source| error(format!("run prepared producer: {source}")))?;
    if !status.success() {
        return Err(error(format!(
            "prepared producer exited {status}; stderr: {}",
            bounded_diagnostic_file(&stderr_path),
        )));
    }
    let measurement = measure_prepared_tree_v3(&root, expected_rom, claims)?;
    let expected_stdout = format!(
        "schema=fn64.wm-prepared-shard-tree.v2 normalized_rom_sha256={} prepared_manifest_sha256={}\n",
        measurement.normalized_rom_sha256, measurement.manifest_sha256
    );
    if fs::read(&stdout_path).map_err(|source| error(format!("read producer stdout: {source}")))?
        != expected_stdout.as_bytes()
    {
        return Err(error("prepared producer stdout is not canonical"));
    }
    Ok(measurement)
}

fn revalidate_prepared_producer_v3(
    expected: &ProducerBuildMeasurementV3,
    cargo: &Path,
    environment: &BuildEnvironmentV3,
    workspace: &Path,
    scratch: &Path,
) -> Result<(), GeneratedRunnerBuildError> {
    let package_root = workspace.join("examples/wm2000-prepared-shard-producer");
    let manifest = package_root.join("Cargo.toml");
    let lock = package_root.join("Cargo.lock");
    let metadata = run_cargo_metadata(cargo, environment, &manifest, scratch)?;
    let metadata_source = cargo_metadata_source_sha256(&metadata)?;
    if sha256_file(&manifest, "prepared producer manifest revalidation")?
        != expected.manifest_sha256
        || sha256_file(&lock, "prepared producer lockfile revalidation")? != expected.lock_sha256
        || hex(&Sha256::digest(&metadata)) != expected.cargo_graph_sha256
        || producer_cargo_source_sha256_v3(&metadata_source, workspace)?
            != expected.cargo_source_sha256
        || sha256_file(&expected.binary, "staged prepared producer revalidation")?
            != expected.binary_sha256
    {
        return Err(error(
            "prepared producer authority changed after publication",
        ));
    }
    Ok(())
}

fn build_selected_binary(
    memory_guard: &Path,
    cargo: &Path,
    manifest: &Path,
    inputs: &Wm2000GeneratedRunnerBuildInputsV1,
    prepared: &PreparedTreeMeasurementV3,
    producer: &ProducerBuildMeasurementV3,
    prepared_source_mode: &str,
    environment: &BuildEnvironmentV3,
    scratch: &Path,
) -> Result<PathBuf, GeneratedRunnerBuildError> {
    let stdout_path = scratch.join("cargo-build.stdout.jsonl");
    let stderr_path = scratch.join("cargo-build.stderr.log");
    let mut command = guarded_build_command(
        memory_guard,
        cargo,
        manifest,
        inputs,
        prepared,
        producer,
        prepared_source_mode,
        environment,
        scratch,
    )?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(create_new(&stdout_path)?))
        .stderr(Stdio::from(create_new(&stderr_path)?));
    let mut child = command
        .spawn()
        .map_err(|source| error(format!("spawn frozen generated-runner build: {source}")))?;
    // The exact repository guard establishes a new session/process group
    // before Cargo begins. Its memory and wall-time failures terminate that
    // whole group, including rustc descendants orphaned by Cargo.
    let status = child
        .wait()
        .map_err(|source| error(format!("wait for guarded generated-runner build: {source}")))?;
    if !status.success() {
        let progress = fs::read(&stdout_path)
            .map(|bytes| cargo_build_progress(&bytes))
            .unwrap_or_else(|source| format!("compiler_artifacts=unreadable({source})"));
        return Err(error(format!(
            "generated-runner Cargo build exited {status}; {progress}; stderr: {}",
            bounded_diagnostic_file(&stderr_path),
        )));
    }
    select_compiler_artifact(
        &fs::read(&stdout_path)
            .map_err(|source| error(format!("read Cargo compiler-artifact stream: {source}")))?,
    )
}

fn guarded_build_command(
    memory_guard: &Path,
    cargo: &Path,
    manifest: &Path,
    inputs: &Wm2000GeneratedRunnerBuildInputsV1,
    prepared: &PreparedTreeMeasurementV3,
    producer: &ProducerBuildMeasurementV3,
    prepared_source_mode: &str,
    environment: &BuildEnvironmentV3,
    scratch: &Path,
) -> Result<Command, GeneratedRunnerBuildError> {
    validate_memory_guard(memory_guard)?;
    let mut command = Command::new(memory_guard);
    environment.apply(&mut command);
    command
        .arg(cargo)
        .arg("build")
        .arg(format!("-j{SELECTED_BUILD_CARGO_JOBS_V5}"))
        .arg("--frozen")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--target-dir")
        .arg(scratch.join("build-target"))
        .arg("--package")
        .arg(PACKAGE)
        .arg("--bin")
        .arg(PACKAGE)
        .arg("--message-format=json-render-diagnostics")
        .current_dir(scratch)
        .env("ROM", &inputs.rom)
        .env("FN64_BOOT_CONTEXT", &inputs.boot_context)
        .env(PREPARED_ROOT_ENV, &prepared.root)
        .env("FN64_WM_PREPARED_SOURCE_MODE", prepared_source_mode)
        .env(
            "FN64_WM_PREPARED_TREE_DESCRIPTOR_SHA256",
            &prepared.descriptor_sha256,
        )
        .env(
            "FN64_WM_PREPARED_MATERIALIZER_SOURCE_SHA256",
            &prepared.claims.materializer_source_sha256,
        )
        .env(
            "FN64_WM_PREPARED_PRODUCER_MANIFEST_SHA256",
            &producer.manifest_sha256,
        )
        .env(
            "FN64_WM_PREPARED_PRODUCER_LOCK_SHA256",
            &producer.lock_sha256,
        )
        .env(
            "FN64_WM_PREPARED_PRODUCER_CARGO_GRAPH_SHA256",
            &producer.cargo_graph_sha256,
        )
        .env(
            "FN64_WM_PREPARED_PRODUCER_CARGO_SOURCE_SHA256",
            &producer.cargo_source_sha256,
        )
        .env(
            "FN64_WM_PREPARED_PRODUCER_BINARY_SHA256",
            &producer.binary_sha256,
        )
        .env(
            "FN64_EXECUTABLE_IMAGE_GROUPS",
            inputs
                .executable_image_groups
                .iter()
                .map(|group| group.environment_name.as_str())
                .collect::<Vec<_>>()
                .join(","),
        )
        .env("CARGO_BUILD_JOBS", SELECTED_BUILD_CARGO_JOBS_V5.to_string())
        .env("FN64_GUARD_MAX_RSS_MIB", BUILD_MAX_RSS_MIB.to_string())
        .env(
            "FN64_GUARD_MIN_FREE_PERCENT",
            BUILD_MIN_FREE_PERCENT.to_string(),
        )
        .env(
            "FN64_GUARD_MAX_SECONDS",
            inputs.max_build_seconds.to_string(),
        );
    for group in &inputs.executable_image_groups {
        let joined = std::env::join_paths(&group.captures).map_err(|source| {
            error(format!(
                "join capture group {}: {source}",
                group.environment_name
            ))
        })?;
        command.env(&group.environment_name, joined);
    }
    Ok(command)
}

fn validate_memory_guard(path: &Path) -> Result<String, GeneratedRunnerBuildError> {
    let bytes = fs::read(path)
        .map_err(|source| error(format!("read generated-runner memory guard: {source}")))?;
    validate_memory_guard_source(&bytes)?;
    if bytes != MEMORY_GUARD_SOURCE {
        return Err(error(
            "repository memory guard differs from the implementation compiled into the verifier",
        ));
    }
    Ok(hex(&Sha256::digest(bytes)))
}

fn validate_memory_guard_source(bytes: &[u8]) -> Result<(), GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("memory guard source is not UTF-8: {source}")))?;
    for required in [
        "setsid",
        "collect_group",
        "terminate_group",
        "signal_group KILL",
        "FN64_GUARD_MAX_RSS_MIB",
        "FN64_GUARD_MIN_FREE_PERCENT",
        "FN64_GUARD_MAX_SECONDS",
    ] {
        if !source.contains(required) {
            return Err(error(format!(
                "memory guard source is missing required process-group policy {required}"
            )));
        }
    }
    Ok(())
}

fn select_compiler_artifact(bytes: &[u8]) -> Result<PathBuf, GeneratedRunnerBuildError> {
    select_named_compiler_artifact(bytes, PACKAGE)
}

fn select_named_compiler_artifact(
    bytes: &[u8],
    package: &str,
) -> Result<PathBuf, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes).map_err(|source| {
        error(format!(
            "Cargo compiler-artifact stream is not UTF-8: {source}"
        ))
    })?;
    let mut selected = None;
    for line in source.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let is_selected = message["reason"] == "compiler-artifact"
            && message["target"]["name"] == package
            && message["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"));
        if !is_selected {
            continue;
        }
        let executable = message["executable"]
            .as_str()
            .ok_or_else(|| error("selected Cargo artifact has no executable"))?;
        if selected.replace(PathBuf::from(executable)).is_some() {
            return Err(error(
                "Cargo emitted multiple selected generated-runner executables",
            ));
        }
    }
    let selected =
        selected.ok_or_else(|| error("Cargo emitted no selected generated-runner executable"))?;
    let canonical = selected.canonicalize().map_err(|source| {
        error(format!(
            "resolve selected generated runner {}: {source}",
            selected.display()
        ))
    })?;
    if !canonical.is_file() {
        return Err(error(
            "selected generated-runner executable is not a regular file",
        ));
    }
    Ok(canonical)
}

fn launch_identity_child(
    child: &Path,
    scratch: &Path,
) -> Result<GeneratedRunnerBuildIdentityV1, GeneratedRunnerBuildError> {
    let stdout_path = scratch.join("identity.stdout.log");
    let stderr_path = scratch.join("identity.stderr.log");
    let mut command = Command::new(child);
    command
        .arg(GENERATED_RUNNER_BUILD_IDENTITY_ARGUMENT_V1)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::from(create_new(&stdout_path)?))
        .stderr(Stdio::from(create_new(&stderr_path)?));
    let mut process = command
        .spawn()
        .map_err(|source| error(format!("launch generated-runner identity child: {source}")))?;
    wait_with_watchdog(
        &mut process,
        IDENTITY_WATCHDOG,
        "generated-runner identity child",
    )?;
    let status = process
        .try_wait()
        .map_err(|source| error(format!("read identity child status: {source}")))?
        .expect("watchdog returned only after child exit");
    if !status.success() {
        return Err(error(format!(
            "generated-runner identity child exited {status}; stderr: {}",
            bounded_diagnostic_file(&stderr_path),
        )));
    }
    parse_identity_output(
        &fs::read(&stdout_path)
            .map_err(|source| error(format!("read identity child output: {source}")))?,
    )
}

fn parse_identity_output(
    bytes: &[u8],
) -> Result<GeneratedRunnerBuildIdentityV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("identity child output is not UTF-8: {source}")))?;
    let mut identity = None;
    for line in source.lines() {
        let Some(json) = line.strip_prefix(GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1) else {
            continue;
        };
        let parsed = serde_json::from_str(json)
            .map_err(|source| error(format!("parse generated-runner child identity: {source}")))?;
        if identity.replace(parsed).is_some() {
            return Err(error(
                "generated-runner child emitted multiple identity envelopes",
            ));
        }
    }
    identity.ok_or_else(|| error("generated-runner child emitted no identity envelope"))
}

/// Parse and semantically validate exactly one Bootstrap child report.
///
/// This does not launch a child and does not mint authority. The future series
/// owner must supply a fresh OS-random nonce for each directly owned launch;
/// replaying the same bytes under another challenge fails here.
pub fn parse_generated_runner_bootstrap_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerBootstrapRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes).map_err(|source| {
        error(format!(
            "bootstrap runtime child output is not UTF-8: {source}"
        ))
    })?;
    let line = source.strip_suffix('\n').ok_or_else(|| {
        error("generated-runner bootstrap runtime report is not one LF-terminated line")
    })?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner bootstrap runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no bootstrap runtime report"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner bootstrap runtime report: {source}"
        ))
    })?;
    validate_generated_runner_bootstrap_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

pub fn run_wm2000_generated_runner_bootstrap_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerBootstrapRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_bootstrap_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerBootstrapRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error(
            "bootstrap runtime series authority failed self-validation",
        ));
    }
    Ok(series)
}

fn run_bootstrap_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(BOOTSTRAP_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..BOOTSTRAP_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain bootstrap audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated a bootstrap audit nonce"));
        }
        let launched = launch_bootstrap_runtime_child(build, nonce, run_index);
        let post_launch_integrity = build.revalidate_selected_binary();
        post_launch_integrity?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_bootstrap_runtime_series(&build.evidence, &observed)?;
    validate_bootstrap_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

fn bootstrap_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Bootstrap,
    )?;
    Ok(command)
}

fn launch_bootstrap_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerBootstrapRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        bootstrap_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Bootstrap,
    )?;
    parse_generated_runner_bootstrap_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

fn semantic_bootstrap_report_sha256(
    report: &GeneratedRunnerBootstrapRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize bootstrap runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn validate_bootstrap_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerBootstrapRuntimeReportV1)],
) -> Result<GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != BOOTSTRAP_RUNTIME_SERIES_RUNS {
        return Err(error("bootstrap runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-bootstrap-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("bootstrap runtime series repeats a nonce"));
        }
        validate_generated_runner_bootstrap_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = semantic_bootstrap_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|baseline| baseline != &semantic)
        {
            return Err(error(
                "bootstrap runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_BOOTSTRAP_SERIES_SCHEMA_V1,
        run_count: BOOTSTRAP_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256: report.build_identity_sha256.clone(),
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        bootstrap_receipt_sha256: prerequisite.bootstrap_receipt_sha256.clone(),
        rom_sha256: prerequisite.rom_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        generation_catalog_sha256: prerequisite.generation_catalog_sha256.clone(),
        journal_root_sha256: prerequisite.journal_entry.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = bootstrap_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

fn bootstrap_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-bootstrap-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.bootstrap_receipt_sha256,
        &evidence.rom_sha256,
        &evidence.resolver_install_sha256,
        &evidence.generation_catalog_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

fn validate_bootstrap_runtime_series_evidence(
    evidence: &GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_BOOTSTRAP_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != BOOTSTRAP_RUNTIME_SERIES_RUNS
    {
        return Err(error("bootstrap runtime series has a noncanonical shape"));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("build_identity_sha256", &evidence.build_identity_sha256),
        ("program_identity_sha256", &evidence.program_identity_sha256),
        ("program_model_sha256", &evidence.program_model_sha256),
        (
            "bootstrap_receipt_sha256",
            &evidence.bootstrap_receipt_sha256,
        ),
        ("rom_sha256", &evidence.rom_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "generation_catalog_sha256",
            &evidence.generation_catalog_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if bootstrap_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("bootstrap runtime series authority digest mismatch"));
    }
    Ok(())
}

fn validate_generated_runner_bootstrap_runtime_report_v1(
    report: &GeneratedRunnerBootstrapRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_SCHEMA_V1 {
        return Err(error(
            "unsupported generated-runner bootstrap runtime report schema",
        ));
    }
    require_sha256(&report.nonce, "bootstrap runtime report nonce")?;
    if report.nonce != hex(&expected_nonce) {
        return Err(error(
            "generated-runner bootstrap runtime report nonce mismatch",
        ));
    }
    let expected_build_identity_sha256 = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build_identity_sha256
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner bootstrap report does not bind the selected build identity",
        ));
    }
    validate_bootstrap_runtime_prerequisite(&report.prerequisite, build_identity)
}

fn validate_bootstrap_ranges(
    ranges: &[BootstrapWriterWatchedRangeV1],
    field: &str,
    allow_empty: bool,
) -> Result<(), GeneratedRunnerBuildError> {
    if !allow_empty && ranges.is_empty() {
        return Err(error(format!("{field} is empty")));
    }
    let mut previous_end = None;
    for range in ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(format!("{field} is not canonical physical backing")));
        }
        previous_end = Some(range.physical_end);
    }
    Ok(())
}

fn range_is_watched(
    range: &BootstrapWriterWatchedRangeV1,
    watched: &[BootstrapWriterWatchedRangeV1],
) -> bool {
    watched.iter().any(|owner| {
        owner.physical_start <= range.physical_start && range.physical_end <= owner.physical_end
    })
}

fn validate_bootstrap_runtime_prerequisite(
    prerequisite: &BootstrapWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::BOOTSTRAP_WRITER_CHANNEL_COMPLETION_SCHEMA_V1 {
        return Err(error("unsupported ABI bootstrap writer receipt schema"));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "bootstrap_receipt_sha256",
            &prerequisite.bootstrap_receipt_sha256,
        ),
        ("rom_sha256", &prerequisite.rom_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "generation_catalog_sha256",
            &prerequisite.generation_catalog_sha256,
        ),
        (
            "bootstrap_watched_sha256",
            &prerequisite.bootstrap_watched_sha256,
        ),
        ("before_sha256", &prerequisite.journal_entry.before_sha256),
        ("after_sha256", &prerequisite.journal_entry.after_sha256),
        (
            "journal_root_sha256",
            &prerequisite.journal_entry.journal_root_sha256,
        ),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.rom_sha256 != build_identity.normalized_rom_sha256 {
        return Err(error(
            "bootstrap writer receipt does not bind the selected normalized ROM",
        ));
    }
    validate_bootstrap_ranges(
        &prerequisite.watched_ranges,
        "bootstrap watched ranges",
        false,
    )?;
    let declared = prerequisite
        .journal_entry
        .declared_writes
        .iter()
        .map(|write| BootstrapWriterWatchedRangeV1 {
            physical_start: write.physical_start,
            physical_end: write.physical_end,
        })
        .collect::<Vec<_>>();
    if declared.iter().any(|range| {
        range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
    }) {
        return Err(error(
            "bootstrap declared writes contain an invalid physical range",
        ));
    }
    validate_bootstrap_ranges(
        &prerequisite.journal_entry.changed_ranges,
        "bootstrap changed ranges",
        true,
    )?;
    let mut declared_union = declared.clone();
    declared_union.sort_by_key(|range| (range.physical_start, range.physical_end));
    let mut merged_declared: Vec<BootstrapWriterWatchedRangeV1> = Vec::new();
    for range in declared_union {
        if let Some(previous) = merged_declared.last_mut() {
            if range.physical_start <= previous.physical_end {
                previous.physical_end = previous.physical_end.max(range.physical_end);
                continue;
            }
        }
        merged_declared.push(range);
    }
    if declared
        .iter()
        .chain(&prerequisite.journal_entry.changed_ranges)
        .any(|range| !range_is_watched(range, &prerequisite.watched_ranges))
        || prerequisite
            .journal_entry
            .changed_ranges
            .iter()
            .any(|changed| !range_is_watched(changed, &merged_declared))
        || prerequisite.journal_entry.sequence != 0
        || !prerequisite
            .journal_entry
            .invalidated_generations
            .is_empty()
        || prerequisite.journal_entry.after_sha256 != prerequisite.final_watched_sha256
        || prerequisite.bootstrap_watched_sha256 != prerequisite.final_watched_sha256
        || prerequisite
            .initial_generations
            .iter()
            .any(|generation| *generation == 0)
        || prerequisite
            .initial_generations
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err(error(
            "bootstrap writer receipt has a noncanonical sequence-zero journal",
        ));
    }
    let canonical_journal_root = recompute_bootstrap_canonical_journal_root(
        &prerequisite.watched_ranges,
        &prerequisite.journal_entry,
    )?;
    if prerequisite.journal_entry.journal_root_sha256 != canonical_journal_root {
        return Err(error(format!(
            "bootstrap canonical journal root mismatch: stored={}, recomputed={canonical_journal_root}",
            prerequisite.journal_entry.journal_root_sha256
        )));
    }
    let recomputed = recompute_bootstrap_runtime_prerequisite_receipt(prerequisite)?;
    if prerequisite.receipt_sha256 != recomputed {
        return Err(error(format!(
            "bootstrap runtime prerequisite receipt mismatch: stored={}, recomputed={recomputed}",
            prerequisite.receipt_sha256
        )));
    }
    Ok(())
}

fn recompute_bootstrap_canonical_journal_root(
    watched_ranges: &[BootstrapWriterWatchedRangeV1],
    entry: &BootstrapMutationBatchV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut initial = Sha256::new();
    initial
        .update(fn64_abi::recompiled::CANONICAL_EXECUTABLE_MUTATION_JOURNAL_SCHEMA_V1.as_bytes());
    initial.update(decode_sha256(&entry.before_sha256)?);
    for range in watched_ranges {
        initial.update(range.physical_start.to_be_bytes());
        initial.update(range.physical_end.to_be_bytes());
    }

    let mut root = Sha256::new();
    root.update(initial.finalize());
    root.update(entry.sequence.to_be_bytes());
    root.update(decode_sha256(&entry.before_sha256)?);
    root.update(decode_sha256(&entry.after_sha256)?);
    for declaration in &entry.declared_writes {
        root.update([declaration.channel.tag()]);
        root.update(declaration.physical_start.to_be_bytes());
        root.update(declaration.physical_end.to_be_bytes());
    }
    for range in &entry.changed_ranges {
        root.update(range.physical_start.to_be_bytes());
        root.update(range.physical_end.to_be_bytes());
    }
    for generation in &entry.invalidated_generations {
        root.update(generation.to_be_bytes());
    }
    Ok(hex(&root.finalize()))
}

fn recompute_bootstrap_runtime_prerequisite_receipt(
    prerequisite: &BootstrapWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:bootstrap-writer-channel-completion-receipt:v1");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.bootstrap_receipt_sha256,
        &prerequisite.rom_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.generation_catalog_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(decode_sha256(&prerequisite.bootstrap_watched_sha256)?);
    hasher.update((prerequisite.initial_generations.len() as u64).to_be_bytes());
    for generation in &prerequisite.initial_generations {
        hasher.update(generation.to_be_bytes());
    }
    let entry = &prerequisite.journal_entry;
    hasher.update(entry.sequence.to_be_bytes());
    hasher.update((entry.declared_writes.len() as u64).to_be_bytes());
    for declaration in &entry.declared_writes {
        hasher.update([declaration.channel.tag()]);
        hasher.update(declaration.physical_start.to_be_bytes());
        hasher.update(declaration.physical_end.to_be_bytes());
    }
    hasher.update((entry.changed_ranges.len() as u64).to_be_bytes());
    for range in &entry.changed_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(decode_sha256(&entry.before_sha256)?);
    hasher.update(decode_sha256(&entry.after_sha256)?);
    hasher.update((entry.invalidated_generations.len() as u64).to_be_bytes());
    for generation in &entry.invalidated_generations {
        hasher.update(generation.to_be_bytes());
    }
    hasher.update(decode_sha256(&entry.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_cpu_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerCpuRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("CPU runtime child output is not UTF-8: {source}")))?;
    let line = source.strip_suffix('\n').ok_or_else(|| {
        error("generated-runner CPU runtime report is not one LF-terminated line")
    })?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner CPU runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no CPU runtime report envelope"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner CPU runtime report: {source}"
        ))
    })?;
    validate_generated_runner_cpu_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

pub fn run_wm2000_generated_runner_cpu_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerCpuRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_cpu_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerCpuRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error("CPU runtime series authority failed self-validation"));
    }
    Ok(series)
}

fn run_cpu_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerCpuRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(CPU_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..CPU_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain CPU audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated a CPU audit nonce"));
        }
        let launched = launch_cpu_runtime_child(build, nonce, run_index);
        build.revalidate_selected_binary()?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_cpu_runtime_series(&build.evidence, &observed)?;
    validate_cpu_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

fn cpu_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Cpu,
    )?;
    Ok(command)
}

fn launch_cpu_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerCpuRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        cpu_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Cpu,
    )?;
    parse_generated_runner_cpu_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

fn cpu_semantic_report_sha256(
    report: &GeneratedRunnerCpuRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize CPU runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn validate_cpu_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerCpuRuntimeReportV1)],
) -> Result<GeneratedRunnerCpuRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != CPU_RUNTIME_SERIES_RUNS {
        return Err(error("CPU runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-cpu-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("CPU runtime series repeats a nonce"));
        }
        validate_generated_runner_cpu_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = cpu_semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|value| value != &semantic)
        {
            return Err(error(
                "CPU runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerCpuRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_CPU_SERIES_SCHEMA_V1,
        run_count: CPU_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256: report.build_identity_sha256.clone(),
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        cpu_store_trace_sha256: prerequisite.cpu_store_trace_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = cpu_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

fn cpu_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerCpuRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-cpu-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.resolver_install_sha256,
        &evidence.abi_host_catalog_receipt_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.cpu_store_trace_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

fn validate_cpu_runtime_series_evidence(
    evidence: &GeneratedRunnerCpuRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_CPU_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != CPU_RUNTIME_SERIES_RUNS
    {
        return Err(error("CPU runtime series has a noncanonical shape"));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("build_identity_sha256", &evidence.build_identity_sha256),
        ("program_identity_sha256", &evidence.program_identity_sha256),
        ("program_model_sha256", &evidence.program_model_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "abi_host_catalog_receipt_sha256",
            &evidence.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        ("cpu_store_trace_sha256", &evidence.cpu_store_trace_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if cpu_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("CPU runtime series authority digest mismatch"));
    }
    Ok(())
}

fn validate_generated_runner_cpu_runtime_report_v1(
    report: &GeneratedRunnerCpuRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_CPU_RUNTIME_REPORT_SCHEMA_V1
        || report.nonce != hex(&expected_nonce)
    {
        return Err(error(
            "generated-runner CPU runtime report schema or nonce mismatch",
        ));
    }
    require_sha256(&report.nonce, "CPU runtime report nonce")?;
    let expected_build = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner CPU report does not bind the selected build identity",
        ));
    }
    validate_cpu_runtime_prerequisite(&report.prerequisite, build_identity)
}

fn validate_cpu_runtime_prerequisite(
    prerequisite: &CpuWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::CPU_WRITER_RUNTIME_STATE_SCHEMA_V1
        || prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "CPU runtime prerequisite does not bind the selected production-AOT build",
        ));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "abi_host_catalog_receipt_sha256",
            &prerequisite.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &prerequisite.journal_root_sha256),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        (
            "cpu_store_trace_sha256",
            &prerequisite.cpu_store_trace_sha256,
        ),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || prerequisite.journal_entry_count == 0
        || prerequisite.cpu_store_count == 0
    {
        return Err(error(
            "CPU runtime prerequisite lacks a fresh store epoch or journal state",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "CPU runtime prerequisite watched ranges are not canonical",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    let recomputed = recompute_cpu_runtime_prerequisite_receipt(prerequisite)?;
    if prerequisite.receipt_sha256 != recomputed {
        return Err(error("CPU runtime prerequisite receipt digest mismatch"));
    }
    Ok(())
}

fn recompute_cpu_runtime_prerequisite_receipt(
    prerequisite: &CpuWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:cpu-instruction-store-runtime-state-receipt:v1");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.abi_host_catalog_receipt_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update(prerequisite.build_receipt_schema.to_be_bytes());
    hasher.update([
        prerequisite.aot_runtime as u8,
        prerequisite.production_aot as u8,
        prerequisite.dev_interpreter as u8,
    ]);
    hasher.update(prerequisite.trace_epoch_id.to_be_bytes());
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(prerequisite.journal_entry_count.to_be_bytes());
    hasher.update(prerequisite.cpu_journal_declaration_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.cpu_store_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.cpu_store_trace_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_host_abi_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerHostAbiRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes).map_err(|source| {
        error(format!(
            "Host ABI runtime child output is not UTF-8: {source}"
        ))
    })?;
    let line = source.strip_suffix('\n').ok_or_else(|| {
        error("generated-runner Host ABI runtime report is not one LF-terminated line")
    })?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner Host ABI runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| {
            error("generated-runner child emitted no Host ABI runtime report envelope")
        })?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner Host ABI runtime report: {source}"
        ))
    })?;
    validate_generated_runner_host_abi_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

pub fn run_wm2000_generated_runner_host_abi_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerHostAbiRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_host_abi_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerHostAbiRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error(
            "Host ABI runtime series authority failed self-validation",
        ));
    }
    Ok(series)
}

fn run_host_abi_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(HOST_ABI_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..HOST_ABI_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain Host ABI audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated a Host ABI audit nonce"));
        }
        let launched = launch_host_abi_runtime_child(build, nonce, run_index);
        build.revalidate_selected_binary()?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_host_abi_runtime_series(&build.evidence, &observed)?;
    validate_host_abi_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

fn host_abi_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::HostAbi,
    )?;
    Ok(command)
}

fn launch_host_abi_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerHostAbiRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        host_abi_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::HostAbi,
    )?;
    parse_generated_runner_host_abi_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

fn host_abi_semantic_report_sha256(
    report: &GeneratedRunnerHostAbiRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize Host ABI runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn validate_host_abi_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerHostAbiRuntimeReportV1)],
) -> Result<GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != HOST_ABI_RUNTIME_SERIES_RUNS {
        return Err(error("Host ABI runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-host-abi-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("Host ABI runtime series repeats a nonce"));
        }
        validate_generated_runner_host_abi_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = host_abi_semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|value| value != &semantic)
        {
            return Err(error(
                "Host ABI runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_HOST_ABI_SERIES_SCHEMA_V1,
        run_count: HOST_ABI_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256: report.build_identity_sha256.clone(),
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        lifecycle_sha256: prerequisite.lifecycle_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = host_abi_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

fn host_abi_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-host-abi-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.resolver_install_sha256,
        &evidence.abi_host_catalog_receipt_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.lifecycle_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

fn validate_host_abi_runtime_series_evidence(
    evidence: &GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_HOST_ABI_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != HOST_ABI_RUNTIME_SERIES_RUNS
    {
        return Err(error("Host ABI runtime series has a noncanonical shape"));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("build_identity_sha256", &evidence.build_identity_sha256),
        ("program_identity_sha256", &evidence.program_identity_sha256),
        ("program_model_sha256", &evidence.program_model_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "abi_host_catalog_receipt_sha256",
            &evidence.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        ("lifecycle_sha256", &evidence.lifecycle_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if host_abi_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("Host ABI runtime series authority digest mismatch"));
    }
    Ok(())
}

fn validate_generated_runner_host_abi_runtime_report_v1(
    report: &GeneratedRunnerHostAbiRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_SCHEMA_V1
        || report.nonce != hex(&expected_nonce)
    {
        return Err(error(
            "generated-runner Host ABI runtime report schema or nonce mismatch",
        ));
    }
    require_sha256(&report.nonce, "Host ABI runtime report nonce")?;
    let expected_build = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner Host ABI report does not bind the selected build identity",
        ));
    }
    validate_host_abi_runtime_prerequisite(&report.prerequisite, build_identity)
}

fn validate_host_abi_runtime_prerequisite(
    prerequisite: &HostAbiWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::HOST_ABI_WRITER_RUNTIME_STATE_SCHEMA_V1
        || prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "Host ABI runtime prerequisite does not bind the selected production-AOT build",
        ));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "abi_host_catalog_receipt_sha256",
            &prerequisite.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &prerequisite.journal_root_sha256),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        ("lifecycle_sha256", &prerequisite.lifecycle_sha256),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || prerequisite.final_journal_entry_count <= prerequisite.initial_journal_entry_count
        || prerequisite.host_abi_journal_entry_count == 0
        || prerequisite.host_abi_journal_declaration_count == 0
        || prerequisite.transactions_started == 0
        || prerequisite.transactions_started != prerequisite.transactions_finished
        || prerequisite.ordering_boundaries == 0
    {
        return Err(error(
            "Host ABI runtime prerequisite lacks a fresh balanced write lifecycle",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "Host ABI runtime prerequisite watched ranges are not canonical",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    let recomputed = recompute_host_abi_runtime_prerequisite_receipt(prerequisite)?;
    if prerequisite.receipt_sha256 != recomputed {
        return Err(error(
            "Host ABI runtime prerequisite receipt digest mismatch",
        ));
    }
    Ok(())
}

fn recompute_host_abi_runtime_prerequisite_receipt(
    prerequisite: &HostAbiWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:host-abi-writer-runtime-state-receipt:v1");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.abi_host_catalog_receipt_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update(prerequisite.build_receipt_schema.to_be_bytes());
    hasher.update([
        prerequisite.aot_runtime as u8,
        prerequisite.production_aot as u8,
        prerequisite.dev_interpreter as u8,
    ]);
    hasher.update(prerequisite.trace_epoch_id.to_be_bytes());
    hasher.update(prerequisite.initial_journal_entry_count.to_be_bytes());
    hasher.update(prerequisite.final_journal_entry_count.to_be_bytes());
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(prerequisite.host_abi_journal_entry_count.to_be_bytes());
    hasher.update(
        prerequisite
            .host_abi_journal_declaration_count
            .to_be_bytes(),
    );
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.transactions_started.to_be_bytes());
    hasher.update(prerequisite.transactions_finished.to_be_bytes());
    hasher.update(prerequisite.ordering_boundaries.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.lifecycle_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_pi_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerPiRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("PI runtime child output is not UTF-8: {source}")))?;
    let line = source
        .strip_suffix('\n')
        .ok_or_else(|| error("generated-runner PI runtime report is not one LF-terminated line"))?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner PI runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no PI runtime report envelope"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner PI runtime report: {source}"
        ))
    })?;
    validate_generated_runner_pi_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

pub fn run_wm2000_generated_runner_pi_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerPiRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_pi_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerPiRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error("PI runtime series authority failed self-validation"));
    }
    Ok(series)
}

fn run_pi_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerPiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(PI_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..PI_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain PI audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated a PI audit nonce"));
        }
        let launched = launch_pi_runtime_child(build, nonce, run_index);
        build.revalidate_selected_binary()?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_pi_runtime_series(&build.evidence, &observed)?;
    validate_pi_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

fn pi_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Pi,
    )?;
    Ok(command)
}

fn launch_pi_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerPiRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        pi_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Pi,
    )?;
    parse_generated_runner_pi_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

fn pi_semantic_report_sha256(
    report: &GeneratedRunnerPiRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize PI runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn validate_pi_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerPiRuntimeReportV1)],
) -> Result<GeneratedRunnerPiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != PI_RUNTIME_SERIES_RUNS {
        return Err(error("PI runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-pi-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("PI runtime series repeats a nonce"));
        }
        validate_generated_runner_pi_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = pi_semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|value| value != &semantic)
        {
            return Err(error(
                "PI runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerPiRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_PI_SERIES_SCHEMA_V1,
        run_count: PI_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256: report.build_identity_sha256.clone(),
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        pi_transition_sha256: prerequisite.pi_transition_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = pi_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

fn pi_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerPiRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-pi-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.resolver_install_sha256,
        &evidence.abi_host_catalog_receipt_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.pi_transition_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

fn validate_pi_runtime_series_evidence(
    evidence: &GeneratedRunnerPiRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_PI_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != PI_RUNTIME_SERIES_RUNS
    {
        return Err(error("PI runtime series has a noncanonical shape"));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("build_identity_sha256", &evidence.build_identity_sha256),
        ("program_identity_sha256", &evidence.program_identity_sha256),
        ("program_model_sha256", &evidence.program_model_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "abi_host_catalog_receipt_sha256",
            &evidence.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        ("pi_transition_sha256", &evidence.pi_transition_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if pi_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("PI runtime series authority digest mismatch"));
    }
    Ok(())
}

fn validate_generated_runner_pi_runtime_report_v1(
    report: &GeneratedRunnerPiRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_PI_RUNTIME_REPORT_SCHEMA_V1
        || report.nonce != hex(&expected_nonce)
    {
        return Err(error(
            "generated-runner PI runtime report schema or nonce mismatch",
        ));
    }
    require_sha256(&report.nonce, "PI runtime report nonce")?;
    let expected_build = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner PI report does not bind the selected build identity",
        ));
    }
    validate_pi_runtime_prerequisite(&report.prerequisite, build_identity)
}

fn validate_pi_runtime_prerequisite(
    prerequisite: &PiWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::PI_WRITER_RUNTIME_STATE_SCHEMA_V1
        || prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "PI runtime prerequisite does not bind the selected production-AOT build",
        ));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "abi_host_catalog_receipt_sha256",
            &prerequisite.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &prerequisite.journal_root_sha256),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        ("pi_transition_sha256", &prerequisite.pi_transition_sha256),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || prerequisite.journal_entry_count == 0
        || prerequisite.pi_started == 0
        || prerequisite.pi_committed == 0
        || prerequisite.pi_busy_cleared == 0
        || prerequisite.pi_notifications == 0
        || prerequisite.pi_to_rdram_committed == 0
    {
        return Err(error(
            "PI runtime prerequisite lacks a fresh completed read-DMA lifecycle",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "PI runtime prerequisite watched ranges are not canonical",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    if prerequisite.receipt_sha256 != recompute_pi_runtime_prerequisite_receipt(prerequisite)? {
        return Err(error("PI runtime prerequisite receipt digest mismatch"));
    }
    Ok(())
}

fn recompute_pi_runtime_prerequisite_receipt(
    prerequisite: &PiWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:pi-writer-runtime-state-receipt:v1");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.abi_host_catalog_receipt_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update(prerequisite.build_receipt_schema.to_be_bytes());
    hasher.update([
        prerequisite.aot_runtime as u8,
        prerequisite.production_aot as u8,
        prerequisite.dev_interpreter as u8,
    ]);
    hasher.update(prerequisite.trace_epoch_id.to_be_bytes());
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(prerequisite.journal_entry_count.to_be_bytes());
    hasher.update(prerequisite.pi_journal_declaration_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    for count in [
        prerequisite.pi_started,
        prerequisite.pi_committed,
        prerequisite.pi_busy_cleared,
        prerequisite.pi_interrupt_raised,
        prerequisite.pi_interrupt_cleared,
        prerequisite.pi_notifications,
        prerequisite.pi_to_rdram_committed,
    ] {
        hasher.update(count.to_be_bytes());
    }
    hasher.update(decode_sha256(&prerequisite.pi_transition_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_rdp_renderer_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerRdpRendererRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes).map_err(|source| {
        error(format!(
            "RDP renderer runtime child output is not UTF-8: {source}"
        ))
    })?;
    let line = source.strip_suffix('\n').ok_or_else(|| {
        error("generated-runner RDP renderer runtime report is not one LF-terminated line")
    })?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner RDP renderer runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| {
            error("generated-runner child emitted no RDP renderer runtime report envelope")
        })?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner RDP renderer runtime report: {source}"
        ))
    })?;
    validate_generated_runner_rdp_renderer_runtime_report_v1(
        &report,
        expected_nonce,
        build_identity,
    )?;
    Ok(report)
}

pub fn run_wm2000_generated_runner_rdp_renderer_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerRdpRendererRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_rdp_renderer_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerRdpRendererRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error(
            "RDP renderer runtime series authority failed self-validation",
        ));
    }
    Ok(series)
}

fn run_rdp_renderer_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(RDP_RENDERER_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..RDP_RENDERER_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain RDP renderer audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error(
                "OS random source repeated an RDP renderer audit nonce",
            ));
        }
        let launched = launch_rdp_renderer_runtime_child(build, nonce, run_index);
        build.revalidate_selected_binary()?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_rdp_renderer_runtime_series(&build.evidence, &observed)?;
    validate_rdp_renderer_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

fn rdp_renderer_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::RdpRenderer,
    )?;
    Ok(command)
}

fn launch_rdp_renderer_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerRdpRendererRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        rdp_renderer_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::RdpRenderer,
    )?;
    parse_generated_runner_rdp_renderer_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

fn rdp_renderer_semantic_report_sha256(
    report: &GeneratedRunnerRdpRendererRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic).map_err(|source| {
        error(format!(
            "serialize RDP renderer runtime semantics: {source}"
        ))
    })?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn validate_rdp_renderer_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerRdpRendererRuntimeReportV1)],
) -> Result<GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != RDP_RENDERER_RUNTIME_SERIES_RUNS {
        return Err(error("RDP renderer runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-rdp-renderer-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("RDP renderer runtime series repeats a nonce"));
        }
        validate_generated_runner_rdp_renderer_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = rdp_renderer_semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|value| value != &semantic)
        {
            return Err(error(
                "RDP renderer runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_RDP_RENDERER_SERIES_SCHEMA_V1,
        run_count: RDP_RENDERER_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256: report.build_identity_sha256.clone(),
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        publication_trace_sha256: prerequisite.publication_trace_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = rdp_renderer_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

fn rdp_renderer_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-rdp-renderer-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.resolver_install_sha256,
        &evidence.abi_host_catalog_receipt_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.publication_trace_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

fn validate_rdp_renderer_runtime_series_evidence(
    evidence: &GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_RDP_RENDERER_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != RDP_RENDERER_RUNTIME_SERIES_RUNS
    {
        return Err(error(
            "RDP renderer runtime series has a noncanonical shape",
        ));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("build_identity_sha256", &evidence.build_identity_sha256),
        ("program_identity_sha256", &evidence.program_identity_sha256),
        ("program_model_sha256", &evidence.program_model_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "abi_host_catalog_receipt_sha256",
            &evidence.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        (
            "publication_trace_sha256",
            &evidence.publication_trace_sha256,
        ),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if rdp_renderer_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error(
            "RDP renderer runtime series authority digest mismatch",
        ));
    }
    Ok(())
}

fn validate_generated_runner_rdp_renderer_runtime_report_v1(
    report: &GeneratedRunnerRdpRendererRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_SCHEMA_V1
        || report.nonce != hex(&expected_nonce)
    {
        return Err(error(
            "generated-runner RDP renderer runtime report schema or nonce mismatch",
        ));
    }
    require_sha256(&report.nonce, "RDP renderer runtime report nonce")?;
    let expected_build = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner RDP renderer report does not bind the selected build identity",
        ));
    }
    validate_rdp_renderer_runtime_prerequisite(&report.prerequisite, build_identity)
}

fn validate_rdp_renderer_runtime_prerequisite(
    prerequisite: &RdpRendererWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::RDP_RENDERER_WRITER_RUNTIME_STATE_SCHEMA_V1
        || prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "RDP renderer runtime prerequisite does not bind the selected production-AOT build",
        ));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "abi_host_catalog_receipt_sha256",
            &prerequisite.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &prerequisite.journal_root_sha256),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        (
            "publication_trace_sha256",
            &prerequisite.publication_trace_sha256,
        ),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || prerequisite.final_journal_entry_count <= prerequisite.initial_journal_entry_count
        || prerequisite.rdp_renderer_journal_entry_count == 0
        || prerequisite.rdp_renderer_journal_declaration_count == 0
        || prerequisite.renderer_publication_count == 0
        || prerequisite.rdp_renderer_journal_entry_count
            > prerequisite.final_journal_entry_count - prerequisite.initial_journal_entry_count
        || prerequisite.rdp_renderer_journal_declaration_count
            < prerequisite.rdp_renderer_journal_entry_count
        || prerequisite.rdp_renderer_journal_entry_count > prerequisite.renderer_publication_count
    {
        return Err(error(
            "RDP renderer runtime prerequisite lacks a fresh executable-byte publication",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "RDP renderer runtime prerequisite watched ranges are not canonical",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    if prerequisite.receipt_sha256
        != recompute_rdp_renderer_runtime_prerequisite_receipt(prerequisite)?
    {
        return Err(error(
            "RDP renderer runtime prerequisite receipt digest mismatch",
        ));
    }
    Ok(())
}

fn recompute_rdp_renderer_runtime_prerequisite_receipt(
    prerequisite: &RdpRendererWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:rdp-renderer-writer-runtime-state-receipt:v1");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.abi_host_catalog_receipt_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update(prerequisite.build_receipt_schema.to_be_bytes());
    hasher.update([
        prerequisite.aot_runtime as u8,
        prerequisite.production_aot as u8,
        prerequisite.dev_interpreter as u8,
    ]);
    hasher.update(prerequisite.trace_epoch_id.to_be_bytes());
    hasher.update(prerequisite.initial_journal_entry_count.to_be_bytes());
    hasher.update(prerequisite.final_journal_entry_count.to_be_bytes());
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(prerequisite.rdp_renderer_journal_entry_count.to_be_bytes());
    hasher.update(
        prerequisite
            .rdp_renderer_journal_declaration_count
            .to_be_bytes(),
    );
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.renderer_publication_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.publication_trace_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_rsp_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerRspRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("RSP runtime child output is not UTF-8: {source}")))?;
    let line = source.strip_suffix('\n').ok_or_else(|| {
        error("generated-runner RSP runtime report is not one LF-terminated line")
    })?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner RSP runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no RSP runtime report envelope"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner RSP runtime report: {source}"
        ))
    })?;
    validate_generated_runner_rsp_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

pub fn run_wm2000_generated_runner_rsp_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerRspRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_rsp_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerRspRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error("RSP runtime series authority failed self-validation"));
    }
    Ok(series)
}

fn run_rsp_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerRspRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(RSP_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..RSP_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain RSP audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated an RSP audit nonce"));
        }
        let launched = launch_rsp_runtime_child(build, nonce, run_index);
        build.revalidate_selected_binary()?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_rsp_runtime_series(&build.evidence, &observed)?;
    validate_rsp_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

fn rsp_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Rsp,
    )?;
    Ok(command)
}

fn launch_rsp_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerRspRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        rsp_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Rsp,
    )?;
    parse_generated_runner_rsp_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

fn rsp_semantic_report_sha256(
    report: &GeneratedRunnerRspRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize RSP runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn validate_rsp_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerRspRuntimeReportV1)],
) -> Result<GeneratedRunnerRspRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != RSP_RUNTIME_SERIES_RUNS {
        return Err(error("RSP runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-rsp-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("RSP runtime series repeats a nonce"));
        }
        validate_generated_runner_rsp_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = rsp_semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|value| value != &semantic)
        {
            return Err(error(
                "RSP runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerRspRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_RSP_SERIES_SCHEMA_V1,
        run_count: RSP_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256: report.build_identity_sha256.clone(),
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        writeback_trace_sha256: prerequisite.writeback_trace_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = rsp_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

fn rsp_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerRspRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-rsp-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.resolver_install_sha256,
        &evidence.abi_host_catalog_receipt_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.writeback_trace_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

fn validate_rsp_runtime_series_evidence(
    evidence: &GeneratedRunnerRspRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_RSP_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != RSP_RUNTIME_SERIES_RUNS
    {
        return Err(error("RSP runtime series has a noncanonical shape"));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("build_identity_sha256", &evidence.build_identity_sha256),
        ("program_identity_sha256", &evidence.program_identity_sha256),
        ("program_model_sha256", &evidence.program_model_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "abi_host_catalog_receipt_sha256",
            &evidence.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        ("writeback_trace_sha256", &evidence.writeback_trace_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if rsp_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("RSP runtime series authority digest mismatch"));
    }
    Ok(())
}

fn validate_generated_runner_rsp_runtime_report_v1(
    report: &GeneratedRunnerRspRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_RSP_RUNTIME_REPORT_SCHEMA_V1
        || report.nonce != hex(&expected_nonce)
    {
        return Err(error(
            "generated-runner RSP runtime report schema or nonce mismatch",
        ));
    }
    require_sha256(&report.nonce, "RSP runtime report nonce")?;
    let expected_build = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner RSP report does not bind the selected build identity",
        ));
    }
    validate_rsp_runtime_prerequisite(&report.prerequisite, build_identity)
}

fn validate_rsp_runtime_prerequisite(
    prerequisite: &RspWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::RSP_WRITER_RUNTIME_STATE_SCHEMA_V1
        || prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "RSP runtime prerequisite does not bind the selected production-AOT build",
        ));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "abi_host_catalog_receipt_sha256",
            &prerequisite.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &prerequisite.journal_root_sha256),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        (
            "writeback_trace_sha256",
            &prerequisite.writeback_trace_sha256,
        ),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    let publication_count = prerequisite
        .interpreter_writeback_count
        .checked_add(prerequisite.translated_audio_hle_publication_count)
        .ok_or_else(|| error("RSP runtime prerequisite publication count overflow"))?;
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || publication_count == 0
        || prerequisite.writeback_range_count != prerequisite.interpreter_writeback_count
    {
        return Err(error(
            "RSP runtime prerequisite lacks a fresh typed writeback publication",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "RSP runtime prerequisite watched ranges are not canonical",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    if prerequisite.receipt_sha256 != recompute_rsp_runtime_prerequisite_receipt(prerequisite)? {
        return Err(error("RSP runtime prerequisite receipt digest mismatch"));
    }
    Ok(())
}

fn recompute_rsp_runtime_prerequisite_receipt(
    prerequisite: &RspWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:rsp-execution-writeback-runtime-state-receipt:v1");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.abi_host_catalog_receipt_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update(prerequisite.build_receipt_schema.to_be_bytes());
    hasher.update([
        prerequisite.aot_runtime as u8,
        prerequisite.production_aot as u8,
        prerequisite.dev_interpreter as u8,
    ]);
    hasher.update(prerequisite.trace_epoch_id.to_be_bytes());
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(prerequisite.journal_entry_count.to_be_bytes());
    hasher.update(prerequisite.rsp_journal_declaration_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.interpreter_writeback_count.to_be_bytes());
    hasher.update(
        prerequisite
            .translated_audio_hle_publication_count
            .to_be_bytes(),
    );
    hasher.update(prerequisite.writeback_range_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.writeback_trace_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_si_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerSiRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("SI runtime child output is not UTF-8: {source}")))?;
    let line = source
        .strip_suffix('\n')
        .ok_or_else(|| error("generated-runner SI runtime report is not one LF-terminated line"))?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner SI runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no SI runtime report envelope"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner SI runtime report: {source}"
        ))
    })?;
    validate_generated_runner_si_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

/// Consume one verified build in a verifier-owned exact-ten SI audit series.
///
/// Every child receives a distinct OS-random nonce and only the retained
/// staged private inputs. The selected binary and all private inputs are
/// revalidated before and after every launch. Success returns a move-only
/// series capability; it does not complete the writer-channel denominator.
pub fn run_wm2000_generated_runner_si_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerSiRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_si_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerSiRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error("SI runtime series authority failed self-validation"));
    }
    Ok(series)
}

fn run_si_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerSiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(SI_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..SI_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain SI audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated an SI audit nonce"));
        }
        let launched = launch_si_runtime_child(build, nonce, run_index);
        let post_launch_integrity = build.revalidate_selected_binary();
        post_launch_integrity?;
        let report = launched?;
        observed.push((nonce, report));
    }
    let evidence = validate_si_runtime_series(&build.evidence, &observed)?;
    validate_si_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

fn si_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Si,
    )?;
    Ok(command)
}

#[derive(Clone, Copy)]
enum WriterRuntimeAuditProtocol {
    Bootstrap,
    Cpu,
    HostAbi,
    Pi,
    RdpRenderer,
    Rsp,
    Si,
    Sp,
}

impl WriterRuntimeAuditProtocol {
    const fn argument(self) -> &'static str {
        match self {
            Self::Bootstrap => GENERATED_RUNNER_BOOTSTRAP_RUNTIME_ARGUMENT_V1,
            Self::Cpu => GENERATED_RUNNER_CPU_RUNTIME_ARGUMENT_V1,
            Self::HostAbi => GENERATED_RUNNER_HOST_ABI_RUNTIME_ARGUMENT_V1,
            Self::Pi => GENERATED_RUNNER_PI_RUNTIME_ARGUMENT_V1,
            Self::RdpRenderer => GENERATED_RUNNER_RDP_RENDERER_RUNTIME_ARGUMENT_V1,
            Self::Rsp => GENERATED_RUNNER_RSP_RUNTIME_ARGUMENT_V1,
            Self::Si => GENERATED_RUNNER_SI_RUNTIME_ARGUMENT_V1,
            Self::Sp => GENERATED_RUNNER_SP_RUNTIME_ARGUMENT_V1,
        }
    }

    const fn nonce_environment(self) -> &'static str {
        match self {
            Self::Bootstrap => GENERATED_RUNNER_BOOTSTRAP_RUNTIME_NONCE_ENV_V1,
            Self::Cpu => GENERATED_RUNNER_CPU_RUNTIME_NONCE_ENV_V1,
            Self::HostAbi => GENERATED_RUNNER_HOST_ABI_RUNTIME_NONCE_ENV_V1,
            Self::Pi => GENERATED_RUNNER_PI_RUNTIME_NONCE_ENV_V1,
            Self::RdpRenderer => GENERATED_RUNNER_RDP_RENDERER_RUNTIME_NONCE_ENV_V1,
            Self::Rsp => GENERATED_RUNNER_RSP_RUNTIME_NONCE_ENV_V1,
            Self::Si => GENERATED_RUNNER_SI_RUNTIME_NONCE_ENV_V1,
            Self::Sp => GENERATED_RUNNER_SP_RUNTIME_NONCE_ENV_V1,
        }
    }

    const fn report_prefix(self) -> &'static str {
        match self {
            Self::Bootstrap => GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1,
            Self::Cpu => GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1,
            Self::HostAbi => GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1,
            Self::Pi => GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1,
            Self::RdpRenderer => GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1,
            Self::Rsp => GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1,
            Self::Si => GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1,
            Self::Sp => GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Bootstrap => "bootstrap",
            Self::Cpu => "CPU",
            Self::HostAbi => "Host ABI",
            Self::Pi => "PI",
            Self::RdpRenderer => "RDP renderer",
            Self::Rsp => "RSP",
            Self::Si => "SI",
            Self::Sp => "SP",
        }
    }
}

fn configure_writer_runtime_command(
    command: &mut Command,
    inputs: &Wm2000GeneratedRunnerBuildInputsV1,
    nonce: [u8; 32],
    protocol: WriterRuntimeAuditProtocol,
) -> Result<(), GeneratedRunnerBuildError> {
    command
        .arg(protocol.argument())
        .env_clear()
        .env("ROM", &inputs.rom)
        .env("FN64_BOOT_CONTEXT", &inputs.boot_context)
        .env(protocol.nonce_environment(), hex(&nonce))
        .env(
            "FN64_EXECUTABLE_IMAGE_GROUPS",
            inputs
                .executable_image_groups
                .iter()
                .map(|group| group.environment_name.as_str())
                .collect::<Vec<_>>()
                .join(","),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for group in &inputs.executable_image_groups {
        command.env(
            &group.environment_name,
            std::env::join_paths(&group.captures).map_err(|source| {
                error(format!(
                    "join retained staged capture group for {} audit: {source}",
                    protocol.label()
                ))
            })?,
        );
    }
    Ok(())
}

fn launch_si_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerSiRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        si_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Si,
    )?;
    parse_generated_runner_si_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

fn launch_writer_runtime_child_output(
    mut command: Command,
    run_index: usize,
    protocol: WriterRuntimeAuditProtocol,
) -> Result<Vec<u8>, GeneratedRunnerBuildError> {
    let label = protocol.label();
    let mut process = command.spawn().map_err(|source| {
        error(format!(
            "launch generated-runner {label} audit child: {source}"
        ))
    })?;
    let stdout = process
        .stdout
        .take()
        .expect("writer audit command configured piped stdout");
    let stderr = process
        .stderr
        .take()
        .expect("writer audit command configured piped stderr");
    let stdout_reader = thread::spawn(move || read_bounded_output(stdout));
    let stderr_reader = thread::spawn(move || read_bounded_output(stderr));
    let wait = wait_with_watchdog(
        &mut process,
        WRITER_RUNTIME_WATCHDOG,
        "generated-runner writer audit child",
    );
    let status = process
        .try_wait()
        .map_err(|source| error(format!("read {label} audit child status: {source}")))?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| error(format!("{label} audit stdout reader panicked")))?
        .map_err(error)?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| error(format!("{label} audit stderr reader panicked")))?
        .map_err(error)?;
    if let Err(wait_error) = wait {
        return Err(error(format!(
            "{label} audit child run {run_index} failed its watchdog: {wait_error}; stdout_bytes={} stdout_sha256={} stdout_tail={}; stderr_bytes={} stderr_sha256={} stderr_tail={}",
            stdout.total_bytes,
            stdout.sha256_hex(),
            stdout.diagnostic_tail(),
            stderr.total_bytes,
            stderr.sha256_hex(),
            stderr.diagnostic_tail(),
        )));
    }
    let status = status.expect("watchdog returned only after child exit");
    if !writer_runtime_outputs_within_limit(stdout.total_bytes, stderr.total_bytes) {
        return Err(error(format!(
            "{label} audit child run {run_index} exceeded the {}-byte output limit: stdout_bytes={} stdout_sha256={} stdout_tail={}; stderr_bytes={} stderr_sha256={} stderr_tail={}",
            WRITER_RUNTIME_OUTPUT_LIMIT,
            stdout.total_bytes,
            stdout.sha256_hex(),
            stdout.diagnostic_tail(),
            stderr.total_bytes,
            stderr.sha256_hex(),
            stderr.diagnostic_tail(),
        )));
    }
    if !status.success() {
        return Err(error(format!(
            "{label} audit child run {run_index} exited {status}: stdout_bytes={} stdout_sha256={} stderr_bytes={} stderr_sha256={}; stderr: {}",
            stdout.total_bytes,
            stdout.sha256_hex(),
            stderr.total_bytes,
            stderr.sha256_hex(),
            bounded_diagnostic(&stderr.bytes),
        )));
    }
    if stderr.total_bytes != 0 {
        return Err(error(format!(
            "{label} audit child run {run_index} emitted stderr: bytes={} sha256={}",
            stderr.total_bytes,
            stderr.sha256_hex(),
        )));
    }
    extract_writer_runtime_report_envelope(&stdout.bytes, protocol)
}

fn writer_runtime_outputs_within_limit(stdout_bytes: u64, stderr_bytes: u64) -> bool {
    stdout_bytes <= WRITER_RUNTIME_OUTPUT_LIMIT as u64
        && stderr_bytes <= WRITER_RUNTIME_OUTPUT_LIMIT as u64
}

struct BoundedOutput {
    bytes: Vec<u8>,
    tail: Vec<u8>,
    total_bytes: u64,
    sha256: [u8; 32],
}

impl BoundedOutput {
    fn sha256_hex(&self) -> String {
        hex(&self.sha256)
    }

    fn diagnostic_tail(&self) -> String {
        if self.total_bytes <= self.bytes.len() as u64 {
            bounded_diagnostic(&self.bytes)
        } else {
            bounded_diagnostic(&self.tail)
        }
    }
}

fn read_bounded_output(mut input: impl Read) -> Result<BoundedOutput, String> {
    let mut bytes = Vec::new();
    let mut tail = Vec::new();
    let mut total_bytes = 0u64;
    let mut sha256 = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|source| format!("read bounded child output: {source}"))?;
        if count == 0 {
            break;
        }
        total_bytes = total_bytes
            .checked_add(count as u64)
            .ok_or_else(|| "child output byte count overflow".to_owned())?;
        sha256.update(&buffer[..count]);
        tail.extend_from_slice(&buffer[..count]);
        if tail.len() > WRITER_RUNTIME_DIAGNOSTIC_TAIL_LIMIT {
            let excess = tail.len() - WRITER_RUNTIME_DIAGNOSTIC_TAIL_LIMIT;
            tail.drain(..excess);
        }
        if bytes.len() < WRITER_RUNTIME_OUTPUT_LIMIT {
            let retain = count.min(WRITER_RUNTIME_OUTPUT_LIMIT - bytes.len());
            bytes.extend_from_slice(&buffer[..retain]);
        }
    }
    Ok(BoundedOutput {
        bytes,
        tail,
        total_bytes,
        sha256: sha256.finalize().into(),
    })
}

fn extract_writer_runtime_report_envelope(
    stdout: &[u8],
    protocol: WriterRuntimeAuditProtocol,
) -> Result<Vec<u8>, GeneratedRunnerBuildError> {
    const PREFIXES: [&str; 8] = [
        GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1,
    ];

    let expected = protocol.report_prefix().as_bytes();
    let mut report = None;
    for line in stdout.split_inclusive(|byte| *byte == b'\n') {
        let Some(prefix) = PREFIXES
            .iter()
            .find(|prefix| line.starts_with(prefix.as_bytes()))
        else {
            continue;
        };
        if prefix.as_bytes() != expected {
            return Err(error(format!(
                "{} audit child emitted a report for another protocol",
                protocol.label()
            )));
        }
        if report.replace(line).is_some() {
            return Err(error(format!(
                "{} audit child emitted multiple runtime reports",
                protocol.label()
            )));
        }
    }
    let report = report.ok_or_else(|| {
        error(format!(
            "{} audit child emitted no runtime report",
            protocol.label()
        ))
    })?;
    if report.len() > WRITER_RUNTIME_REPORT_LIMIT {
        return Err(error(format!(
            "{} audit child report exceeds the {}-byte envelope limit",
            protocol.label(),
            WRITER_RUNTIME_REPORT_LIMIT
        )));
    }
    Ok(report.to_vec())
}

fn semantic_report_sha256(
    report: &GeneratedRunnerSiRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize SI runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn validate_si_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerSiRuntimeReportV1)],
) -> Result<GeneratedRunnerSiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != SI_RUNTIME_SERIES_RUNS {
        return Err(error("SI runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-si-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("SI runtime series repeats a nonce"));
        }
        validate_generated_runner_si_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|baseline| baseline != &semantic)
        {
            return Err(error(
                "SI runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let build_identity_sha256 = hex(&Sha256::digest(
        serde_json::to_vec(&build.identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    let mut evidence = GeneratedRunnerSiRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1,
        run_count: SI_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256,
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        si_transition_sha256: prerequisite.si_transition_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = si_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

fn si_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerSiRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-si-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.resolver_install_sha256,
        &evidence.abi_host_catalog_receipt_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.si_transition_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

fn validate_si_runtime_series_evidence(
    evidence: &GeneratedRunnerSiRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != SI_RUNTIME_SERIES_RUNS
    {
        return Err(error("SI runtime series has a noncanonical shape"));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("build_identity_sha256", &evidence.build_identity_sha256),
        ("program_identity_sha256", &evidence.program_identity_sha256),
        ("program_model_sha256", &evidence.program_model_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "abi_host_catalog_receipt_sha256",
            &evidence.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        ("si_transition_sha256", &evidence.si_transition_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if si_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("SI runtime series authority digest mismatch"));
    }
    Ok(())
}

fn validate_generated_runner_si_runtime_report_v1(
    report: &GeneratedRunnerSiRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_SI_RUNTIME_REPORT_SCHEMA_V1 {
        return Err(error(
            "unsupported generated-runner SI runtime report schema",
        ));
    }
    require_sha256(&report.nonce, "SI runtime report nonce")?;
    if report.nonce != hex(&expected_nonce) {
        return Err(error("generated-runner SI runtime report nonce mismatch"));
    }
    let identity_bytes = serde_json::to_vec(build_identity)
        .expect("generated-runner build identity serialization is infallible");
    let expected_build_identity_sha256 = hex(&Sha256::digest(identity_bytes));
    if report.build_identity_sha256 != expected_build_identity_sha256
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner SI runtime report does not bind the selected build identity",
        ));
    }
    require_sha256(
        &report.build_identity_sha256,
        "SI runtime report build_identity_sha256",
    )?;
    require_sha256(
        &report.program_identity_sha256,
        "SI runtime report program_identity_sha256",
    )?;
    validate_si_runtime_prerequisite(&report.prerequisite, build_identity)
}

fn validate_si_runtime_prerequisite(
    prerequisite: &SiWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::SI_WRITER_RUNTIME_STATE_SCHEMA_V1 {
        return Err(error(
            "unsupported ABI SI runtime-state prerequisite schema",
        ));
    }
    if prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "SI runtime prerequisite does not bind the selected production-AOT build receipt",
        ));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "abi_host_catalog_receipt_sha256",
            &prerequisite.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &prerequisite.journal_root_sha256),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        ("si_transition_sha256", &prerequisite.si_transition_sha256),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.watched_ranges.is_empty() || prerequisite.journal_entry_count == 0 {
        return Err(error(
            "SI runtime prerequisite lacks validated executable-journal state",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "SI runtime prerequisite watched ranges are not canonical executable backing",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    if prerequisite.si_started == 0
        || prerequisite.si_started != prerequisite.si_committed
        || prerequisite.si_pif_to_dram_committed == 0
        || prerequisite.si_pif_to_dram_committed > prerequisite.si_committed
    {
        return Err(error(
            "SI runtime prerequisite contains inconsistent transition counts",
        ));
    }
    let recomputed = recompute_si_runtime_prerequisite_receipt(prerequisite)?;
    if prerequisite.receipt_sha256 != recomputed {
        return Err(error(format!(
            "SI runtime prerequisite receipt mismatch: stored={}, recomputed={recomputed}",
            prerequisite.receipt_sha256
        )));
    }
    Ok(())
}

fn recompute_si_runtime_prerequisite_receipt(
    prerequisite: &SiWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:si-writer-runtime-state-receipt:v1");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.abi_host_catalog_receipt_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update(prerequisite.build_receipt_schema.to_be_bytes());
    hasher.update([
        prerequisite.aot_runtime as u8,
        prerequisite.production_aot as u8,
        prerequisite.dev_interpreter as u8,
    ]);
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(prerequisite.journal_entry_count.to_be_bytes());
    hasher.update(prerequisite.si_journal_declaration_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.si_started.to_be_bytes());
    hasher.update(prerequisite.si_committed.to_be_bytes());
    hasher.update(prerequisite.si_pif_to_dram_committed.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.si_transition_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_sp_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerSpRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("SP runtime child output is not UTF-8: {source}")))?;
    let line = source
        .strip_suffix('\n')
        .ok_or_else(|| error("generated-runner SP runtime report is not one LF-terminated line"))?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner SP runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no SP runtime report envelope"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner SP runtime report: {source}"
        ))
    })?;
    validate_generated_runner_sp_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

/// Consume one verified build in a verifier-owned exact-ten SP audit series.
/// Every child receives one fresh OS-random nonce and only retained staged
/// inputs. Pre/post launch revalidation closes replacement of the executable
/// or any private input while the bounded child is running.
pub fn run_wm2000_generated_runner_sp_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerSpRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_sp_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerSpRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error("SP runtime series authority failed self-validation"));
    }
    Ok(series)
}

fn run_sp_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerSpRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(SP_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..SP_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain SP audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated an SP audit nonce"));
        }
        let launched = launch_sp_runtime_child(build, nonce, run_index);
        let post_launch_integrity = build.revalidate_selected_binary();
        post_launch_integrity?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_sp_runtime_series(&build.evidence, &observed)?;
    validate_sp_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

fn sp_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Sp,
    )?;
    Ok(command)
}

fn launch_sp_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerSpRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        sp_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Sp,
    )?;
    parse_generated_runner_sp_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

fn semantic_sp_report_sha256(
    report: &GeneratedRunnerSpRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize SP runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn validate_sp_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerSpRuntimeReportV1)],
) -> Result<GeneratedRunnerSpRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != SP_RUNTIME_SERIES_RUNS {
        return Err(error("SP runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-sp-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("SP runtime series repeats a nonce"));
        }
        validate_generated_runner_sp_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = semantic_sp_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|baseline| baseline != &semantic)
        {
            return Err(error(
                "SP runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let build_identity_sha256 = hex(&Sha256::digest(
        serde_json::to_vec(&build.identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    let mut evidence = GeneratedRunnerSpRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1,
        run_count: SP_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256,
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        sp_transition_sha256: prerequisite.sp_transition_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = sp_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

fn sp_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerSpRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-sp-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.resolver_install_sha256,
        &evidence.abi_host_catalog_receipt_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.sp_transition_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

fn validate_sp_runtime_series_evidence(
    evidence: &GeneratedRunnerSpRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != SP_RUNTIME_SERIES_RUNS
    {
        return Err(error("SP runtime series has a noncanonical shape"));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("build_identity_sha256", &evidence.build_identity_sha256),
        ("program_identity_sha256", &evidence.program_identity_sha256),
        ("program_model_sha256", &evidence.program_model_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "abi_host_catalog_receipt_sha256",
            &evidence.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        ("sp_transition_sha256", &evidence.sp_transition_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if sp_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("SP runtime series authority digest mismatch"));
    }
    Ok(())
}

fn validate_generated_runner_sp_runtime_report_v1(
    report: &GeneratedRunnerSpRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_SP_RUNTIME_REPORT_SCHEMA_V1 {
        return Err(error(
            "unsupported generated-runner SP runtime report schema",
        ));
    }
    require_sha256(&report.nonce, "SP runtime report nonce")?;
    if report.nonce != hex(&expected_nonce) {
        return Err(error("generated-runner SP runtime report nonce mismatch"));
    }
    let expected_build_identity_sha256 = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build_identity_sha256
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner SP runtime report does not bind the selected build identity",
        ));
    }
    require_sha256(
        &report.build_identity_sha256,
        "SP runtime report build_identity_sha256",
    )?;
    require_sha256(
        &report.program_identity_sha256,
        "SP runtime report program_identity_sha256",
    )?;
    validate_sp_runtime_prerequisite(&report.prerequisite, build_identity)
}

fn validate_sp_runtime_prerequisite(
    prerequisite: &SpWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::SP_WRITER_RUNTIME_STATE_SCHEMA_V1 {
        return Err(error(
            "unsupported ABI SP runtime-state prerequisite schema",
        ));
    }
    if prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "SP runtime prerequisite does not bind the selected production-AOT build receipt",
        ));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "abi_host_catalog_receipt_sha256",
            &prerequisite.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &prerequisite.journal_root_sha256),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        ("sp_transition_sha256", &prerequisite.sp_transition_sha256),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || prerequisite.journal_entry_count == 0
    {
        return Err(error(
            "SP runtime prerequisite lacks a fresh epoch or validated journal state",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "SP runtime prerequisite watched ranges are not canonical executable backing",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    if prerequisite.sp_started == 0
        || prerequisite.sp_started != prerequisite.sp_committed
        || prerequisite.sp_busy_cleared == 0
        || prerequisite.sp_busy_cleared > prerequisite.sp_committed
        || prerequisite.sp_queued > prerequisite.sp_started
        || prerequisite.sp_rsp_to_rdram_committed == 0
        || prerequisite.sp_rsp_to_rdram_committed > prerequisite.sp_committed
    {
        return Err(error(
            "SP runtime prerequisite contains inconsistent transition counts",
        ));
    }
    let recomputed = recompute_sp_runtime_prerequisite_receipt(prerequisite)?;
    if prerequisite.receipt_sha256 != recomputed {
        return Err(error(format!(
            "SP runtime prerequisite receipt mismatch: stored={}, recomputed={recomputed}",
            prerequisite.receipt_sha256
        )));
    }
    Ok(())
}

fn recompute_sp_runtime_prerequisite_receipt(
    prerequisite: &SpWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:sp-writer-runtime-state-receipt:v1");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.abi_host_catalog_receipt_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update(prerequisite.build_receipt_schema.to_be_bytes());
    hasher.update([
        prerequisite.aot_runtime as u8,
        prerequisite.production_aot as u8,
        prerequisite.dev_interpreter as u8,
    ]);
    hasher.update(prerequisite.trace_epoch_id.to_be_bytes());
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(prerequisite.journal_entry_count.to_be_bytes());
    hasher.update(prerequisite.sp_journal_declaration_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.sp_started.to_be_bytes());
    hasher.update(prerequisite.sp_queued.to_be_bytes());
    hasher.update(prerequisite.sp_committed.to_be_bytes());
    hasher.update(prerequisite.sp_busy_cleared.to_be_bytes());
    hasher.update(prerequisite.sp_rsp_to_rdram_committed.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.sp_transition_sha256)?);
    Ok(hex(&hasher.finalize()))
}

fn writer_audit_bundle_authority_sha256(
    evidence: &GeneratedRunnerWriterAuditBundleEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-writer-audit-bundle.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.completed_channels]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    for (tag, authority) in [
        (
            WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1,
            evidence
                .bootstrap
                .as_ref()
                .map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_CPU_COMPLETED_V1,
            evidence.cpu.as_ref().map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
            evidence
                .host_abi
                .as_ref()
                .map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_PI_COMPLETED_V1,
            evidence.pi.as_ref().map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
            evidence
                .rdp_renderer
                .as_ref()
                .map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_RSP_COMPLETED_V1,
            evidence.rsp.as_ref().map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_SI_COMPLETED_V1,
            evidence.si.as_ref().map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_SP_COMPLETED_V1,
            evidence.sp.as_ref().map(|series| &series.authority_sha256),
        ),
    ] {
        digest.update([tag]);
        match authority {
            Some(authority) => {
                digest.update([1]);
                digest.update(decode_sha256(authority)?);
            }
            None => digest.update([0]),
        }
    }
    Ok(hex(&digest.finalize()))
}

fn validate_writer_audit_bundle_evidence(
    evidence: &GeneratedRunnerWriterAuditBundleEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1
        || evidence.completed_channels == 0
        || evidence.completed_channels & !WRITER_AUDIT_COMPLETED_MASK_V1 != 0
    {
        return Err(error("writer audit bundle has a noncanonical shape"));
    }
    let expected_bits = u8::from(evidence.bootstrap.is_some())
        * WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1
        | u8::from(evidence.cpu.is_some()) * WRITER_AUDIT_CPU_COMPLETED_V1
        | u8::from(evidence.host_abi.is_some()) * WRITER_AUDIT_HOST_ABI_COMPLETED_V1
        | u8::from(evidence.pi.is_some()) * WRITER_AUDIT_PI_COMPLETED_V1
        | u8::from(evidence.rdp_renderer.is_some()) * WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
        | u8::from(evidence.rsp.is_some()) * WRITER_AUDIT_RSP_COMPLETED_V1
        | u8::from(evidence.si.is_some()) * WRITER_AUDIT_SI_COMPLETED_V1
        | u8::from(evidence.sp.is_some()) * WRITER_AUDIT_SP_COMPLETED_V1;
    if evidence.completed_channels != expected_bits {
        return Err(error(
            "writer audit bundle bitmap does not match its series evidence",
        ));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if let Some(series) = &evidence.bootstrap {
        validate_bootstrap_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.cpu {
        validate_cpu_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.host_abi {
        validate_host_abi_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.pi {
        validate_pi_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.rdp_renderer {
        validate_rdp_renderer_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.rsp {
        validate_rsp_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.si {
        validate_si_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.sp {
        validate_sp_runtime_series_evidence(series)?;
    }
    let mut common = None;
    for series in [
        evidence.bootstrap.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.cpu.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.host_abi.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.pi.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.rdp_renderer.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.rsp.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.si.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.sp.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
    ]
    .into_iter()
    .flatten()
    {
        if series.0 != &evidence.build_authority_sha256
            || series.1 != &evidence.selected_binary_sha256
            || series.2 != &evidence.private_build_inputs_sha256
        {
            return Err(error(
                "writer audit bundle contains evidence from another verified build",
            ));
        }
        let identity = (series.3, series.4, series.5);
        if common.is_some_and(|expected| expected != identity) {
            return Err(error(
                "writer audit bundle contains cross-channel identity or program-model mismatch",
            ));
        }
        common.get_or_insert(identity);
    }
    if writer_audit_bundle_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("writer audit bundle authority digest mismatch"));
    }
    Ok(())
}

fn validate_inputs(
    inputs: &Wm2000GeneratedRunnerBuildInputsV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_input_path(&inputs.rom, "ROM")?;
    validate_input_path(&inputs.boot_context, "BootContext")?;
    if inputs.executable_image_groups.is_empty() {
        return Err(error(
            "generated-runner build requires at least one executable-image group",
        ));
    }
    if !(MIN_BUILD_TIMEOUT_SECONDS..=MAX_BUILD_TIMEOUT_SECONDS).contains(&inputs.max_build_seconds)
    {
        return Err(error(format!(
            "generated-runner max_build_seconds must be {MIN_BUILD_TIMEOUT_SECONDS}..={MAX_BUILD_TIMEOUT_SECONDS}"
        )));
    }
    let mut names = BTreeSet::new();
    for group in &inputs.executable_image_groups {
        let valid_name = group.environment_name.starts_with("FN64_EXECUTABLE_IMAGE_")
            && group
                .environment_name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
        if !valid_name || !names.insert(&group.environment_name) {
            return Err(error(
                "generated-runner capture group has an invalid or duplicate environment name",
            ));
        }
        if group.captures.len() < 3 {
            return Err(error(
                "generated-runner capture group requires at least three captures",
            ));
        }
        for capture in &group.captures {
            validate_input_path(capture, "executable-image capture")?;
        }
    }
    Ok(())
}

fn validate_input_path(path: &Path, label: &str) -> Result<(), GeneratedRunnerBuildError> {
    crate::private_fs::validate_absolute_no_parent(path, label).map_err(error)
}

fn stage_private_inputs(
    inputs: &Wm2000GeneratedRunnerBuildInputsV1,
    scratch: &Path,
) -> Result<Wm2000GeneratedRunnerBuildInputsV1, GeneratedRunnerBuildError> {
    let directory = scratch.join("private-inputs");
    fs::create_dir(&directory).map_err(|source| {
        error(format!(
            "create generated-runner private-input staging directory: {source}"
        ))
    })?;
    let rom = stage_private_input_file(&inputs.rom, &directory.join("rom"), "ROM")?;
    let boot_context = stage_private_input_file(
        &inputs.boot_context,
        &directory.join("boot-context"),
        "BootContext",
    )?;
    let executable_image_groups = inputs
        .executable_image_groups
        .iter()
        .enumerate()
        .map(|(group_index, group)| {
            let captures = group
                .captures
                .iter()
                .enumerate()
                .map(|(capture_index, capture)| {
                    stage_private_input_file(
                        capture,
                        &directory.join(format!("group-{group_index}-capture-{capture_index}")),
                        "executable-image capture",
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Wm2000ExecutableImageGroupV1 {
                environment_name: group.environment_name.clone(),
                captures,
            })
        })
        .collect::<Result<Vec<_>, GeneratedRunnerBuildError>>()?;
    Ok(Wm2000GeneratedRunnerBuildInputsV1 {
        rom,
        boot_context,
        executable_image_groups,
        max_build_seconds: inputs.max_build_seconds,
    })
}

fn stage_private_input_file(
    source: &Path,
    destination: &Path,
    label: &str,
) -> Result<PathBuf, GeneratedRunnerBuildError> {
    let mut output = create_new(destination)?;
    let source_measurement =
        crate::private_fs::measure_regular_stable_with(source, label, |event| match event {
            crate::private_fs::StableFileStream::Length(_) => Ok(()),
            crate::private_fs::StableFileStream::Chunk(bytes) => output
                .write_all(bytes)
                .map_err(|source| format!("stage {label} bytes: {source}")),
        })
        .map_err(error)?;
    output
        .flush()
        .map_err(|source| error(format!("flush staged {label}: {source}")))?;
    output
        .sync_all()
        .map_err(|source| error(format!("sync staged {label}: {source}")))?;
    drop(output);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o400))
            .map_err(|source| error(format!("make staged {label} read-only: {source}")))?;
    }
    let staged = crate::private_fs::measure_regular_stable(destination, &format!("staged {label}"))
        .map_err(error)?;
    if staged.bytes != source_measurement.bytes || staged.sha256 != source_measurement.sha256 {
        return Err(error(format!(
            "staged {label} does not match the descriptor-stable source measurement"
        )));
    }
    Ok(destination.to_path_buf())
}

fn private_inputs_sha256(
    inputs: &Wm2000GeneratedRunnerBuildInputsV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.wm2000-generated-runner-private-inputs.v2\0");
    hash_input_file(&mut digest, b"ROM", &inputs.rom)?;
    hash_input_file(&mut digest, b"BootContext", &inputs.boot_context)?;
    for group in &inputs.executable_image_groups {
        push_bytes(&mut digest, group.environment_name.as_bytes());
        digest.update((group.captures.len() as u64).to_be_bytes());
        for capture in &group.captures {
            hash_input_file(&mut digest, b"capture", capture)?;
        }
    }
    Ok(hex(&digest.finalize()))
}

fn hash_input_file(
    digest: &mut Sha256,
    label: &[u8],
    path: &Path,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_input_path(path, "private build input")?;
    push_bytes(digest, label);
    push_bytes(digest, path.as_os_str().as_encoded_bytes());
    crate::private_fs::measure_regular_stable_with(path, "private build input", |event| {
        match event {
            crate::private_fs::StableFileStream::Length(bytes) => {
                digest.update(bytes.to_be_bytes());
            }
            crate::private_fs::StableFileStream::Chunk(bytes) => digest.update(bytes),
        }
        Ok(())
    })
    .map_err(error)?;
    Ok(())
}

fn stage_selected_binary(
    source: &Path,
    scratch: &Path,
    expected: &str,
) -> Result<PathBuf, GeneratedRunnerBuildError> {
    stage_executable(
        source,
        &scratch.join("selected-generated-runner"),
        expected,
        "generated runner",
    )
}

fn stage_executable(
    source: &Path,
    destination: &Path,
    expected: &str,
    label: &str,
) -> Result<PathBuf, GeneratedRunnerBuildError> {
    let mut source_file = File::open(source)
        .map_err(|source_error| error(format!("open built {label}: {source_error}")))?;
    let mut destination_file = create_new(destination)?;
    std::io::copy(&mut source_file, &mut destination_file)
        .map_err(|source_error| error(format!("stage {label}: {source_error}")))?;
    destination_file
        .sync_all()
        .map_err(|source_error| error(format!("sync staged {label}: {source_error}")))?;
    drop(destination_file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o500)).map_err(
            |source_error| error(format!("make staged {label} executable: {source_error}")),
        )?;
    }
    if sha256_file(destination, &format!("staged {label}"))? != expected {
        return Err(error(format!(
            "staged {label} does not match selected Cargo artifact"
        )));
    }
    Ok(destination.to_path_buf())
}

fn repository_workspace() -> Result<PathBuf, GeneratedRunnerBuildError> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|source| error(format!("resolve fn64 workspace: {source}")))
}

fn wait_with_watchdog(
    child: &mut std::process::Child,
    timeout: Duration,
    label: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|source| error(format!("poll {label}: {source}")))?
            .is_some()
        {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error(format!(
                "{label} exceeded {} seconds",
                timeout.as_secs()
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn create_new(path: &Path) -> Result<File, GeneratedRunnerBuildError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| error(format!("create {}: {source}", path.display())))
}

fn sha256_file(path: &Path, label: &str) -> Result<String, GeneratedRunnerBuildError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| error(format!("inspect {label} {}: {source}", path.display())))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(format!("{label} is not a regular non-symlink file")));
    }
    let mut file = File::open(path)
        .map_err(|source| error(format!("open {label} {}: {source}", path.display())))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|source| error(format!("read {label} {}: {source}", path.display())))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex(&digest.finalize()))
}

fn require_sha256(value: &str, field: &str) -> Result<(), GeneratedRunnerBuildError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(error(format!("{field} is not canonical lowercase SHA-256")));
    }
    Ok(())
}

fn decode_sha256(value: &str) -> Result<[u8; 32], GeneratedRunnerBuildError> {
    require_sha256(value, "digest")?;
    let mut output = [0u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|source| error(format!("decode SHA-256: {source}")))?;
    }
    Ok(output)
}

fn push_bytes(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0xf) as usize] as char);
    }
    output
}

fn error(message: impl Into<String>) -> GeneratedRunnerBuildError {
    GeneratedRunnerBuildError(message.into())
}

#[derive(Debug)]
struct ScratchDirectory(PathBuf);

impl ScratchDirectory {
    fn create(nonce: &[u8; 32]) -> Result<Self, GeneratedRunnerBuildError> {
        let path = std::env::temp_dir().join(format!("fn64-generated-runner-{}", hex(nonce)));
        fs::create_dir(&path).map_err(|source| {
            error(format!(
                "create generated-runner scratch {}: {source}",
                path.display()
            ))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|source| {
                error(format!(
                    "restrict generated-runner scratch {}: {source}",
                    path.display()
                ))
            })?;
        }
        let canonical = path.canonicalize().map_err(|source| {
            error(format!(
                "resolve generated-runner scratch {}: {source}",
                path.display()
            ))
        })?;
        Ok(Self(canonical))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_claims() -> PreparedSourceClaimsV3 {
        PreparedSourceClaimsV3 {
            generator_source_sha256: "a1".repeat(32),
            discovery_source_sha256: "a2".repeat(32),
            emitter_source_sha256: "a3".repeat(32),
            runtime_source_sha256: "a4".repeat(32),
            materializer_source_sha256: "a5".repeat(32),
        }
    }

    fn synthetic_prepared_tree(
        changed_package: Option<&str>,
    ) -> (ScratchDirectory, PathBuf, PreparedSourceClaimsV3, String) {
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).unwrap();
        let scratch = ScratchDirectory::create(&nonce).unwrap();
        let root = scratch.path().join("prepared-test");
        fs::create_dir(&root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let claims = synthetic_claims();
        let rom = "b1".repeat(32);
        let mut manifest = format!(
            concat!(
                "schema fn64.wm-prepared-shard-tree.v2\n",
                "normalized_rom_sha256 {}\n",
                "generator_source_sha256 {}\n",
                "discovery_source_sha256 {}\n",
                "emitter_source_sha256 {}\n",
                "runtime_source_sha256 {}\n",
                "artifact_count 35\n"
            ),
            rom,
            claims.generator_source_sha256,
            claims.discovery_source_sha256,
            claims.emitter_source_sha256,
            claims.runtime_source_sha256,
        );
        for package in PREPARED_PACKAGES {
            let package_root = root.join(package);
            fs::create_dir(&package_root).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&package_root, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let mut runner = format!("// runner {package}\n").into_bytes();
            if changed_package == Some(package) {
                runner.extend_from_slice(b"// changed\n");
            }
            let metadata = format!("// metadata {package}\n").into_bytes();
            let runner_sha = hex(&Sha256::digest(&runner));
            let metadata_sha = hex(&Sha256::digest(&metadata));
            let identity = format!(
                "schema fn64.wm-prepared-shard-artifact.v1\npackage {package}\nrunner_sha256 {runner_sha}\nmetadata_sha256 {metadata_sha}\n"
            )
            .into_bytes();
            for (name, bytes) in [
                ("identity.v1", identity.as_slice()),
                ("runner.rs", runner.as_slice()),
                ("metadata.rs", metadata.as_slice()),
            ] {
                let path = package_root.join(name);
                fs::write(&path, bytes).unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
                }
            }
            manifest.push_str(&format!(
                "artifact {package} {} {runner_sha} {metadata_sha}\n",
                hex(&Sha256::digest(&identity)),
            ));
        }
        let manifest_path = root.join(PREPARED_MANIFEST_NAME);
        fs::write(&manifest_path, manifest).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(manifest_path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        (scratch, root, claims, rom)
    }

    fn identity() -> GeneratedRunnerBuildIdentityV1 {
        let mut identity = GeneratedRunnerBuildIdentityV1 {
            schema: GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V3.to_owned(),
            package: PACKAGE.to_owned(),
            manifest_sha256: "11".repeat(32),
            lock_sha256: "22".repeat(32),
            source_attestation_schema:
                fn64_recomp_rs::GENERATED_RUNNER_SOURCE_ATTESTATION_SCHEMA_V2.to_owned(),
            cargo_source_fields_validated: true,
            program_identity_sha256: "33".repeat(32),
            root_adapter_source_sha256: "44".repeat(32),
            shard_cargo_source_tree_sha256: "55".repeat(32),
            emitter_source_sha256: "66".repeat(32),
            runtime_source_sha256: "67".repeat(32),
            prepared_source_mode: PREPARED_SOURCE_MODE_INACTIVE_V1.to_owned(),
            normalized_rom_sha256: "68".repeat(32),
            prepared_manifest_sha256: "69".repeat(32),
            prepared_tree_sha256: "6a".repeat(32),
            prepared_generator_source_sha256: "6b".repeat(32),
            prepared_discovery_source_sha256: "6c".repeat(32),
            prepared_emitter_source_sha256: "6d".repeat(32),
            prepared_runtime_source_sha256: "6e".repeat(32),
            prepared_materializer_source_sha256: "6f".repeat(32),
            producer_manifest_sha256: "70".repeat(32),
            producer_lock_sha256: "71".repeat(32),
            producer_cargo_graph_sha256: "72".repeat(32),
            producer_cargo_source_sha256: "73".repeat(32),
            producer_binary_sha256: "74".repeat(32),
            binding_sha256: String::new(),
            build_receipt_schema: 1,
            aot_runtime: true,
            production_aot: true,
            dev_interpreter: false,
            runners: vec![GeneratedRunnerLinkedIdentityV1 {
                bank: 7,
                generated_runner_source_sha256: "77".repeat(32),
                code_words_sha256: "88".repeat(32),
                vram_start: 0x8000_0400,
                vram_end: 0x8000_0800,
                composite_subrunner_count: 1,
                adapter_role: GeneratedRunnerAdapterRoleV1::DirectGenerated,
            }],
        };
        identity.binding_sha256 = recompute_binding_sha256(&identity).unwrap();
        identity
    }

    fn bootstrap_prerequisite(
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> BootstrapWriterRuntimePrerequisiteV1 {
        let mut prerequisite = BootstrapWriterRuntimePrerequisiteV1 {
            schema: fn64_abi::recompiled::BOOTSTRAP_WRITER_CHANNEL_COMPLETION_SCHEMA_V1.to_owned(),
            program_model_sha256: "a1".repeat(32),
            bootstrap_receipt_sha256: "c2".repeat(32),
            rom_sha256: identity.normalized_rom_sha256.clone(),
            resolver_install_sha256: "c3".repeat(32),
            generation_catalog_sha256: "c4".repeat(32),
            watched_ranges: vec![BootstrapWriterWatchedRangeV1 {
                physical_start: 0x400,
                physical_end: 0x800,
            }],
            bootstrap_watched_sha256: "c5".repeat(32),
            initial_generations: vec![1, 2],
            journal_entry: BootstrapMutationBatchV1 {
                sequence: 0,
                declared_writes: vec![BootstrapAttributedWriteV1 {
                    channel: BootstrapWriterChannelV1::BootstrapOrImport,
                    physical_start: 0x400,
                    physical_end: 0x800,
                }],
                changed_ranges: vec![BootstrapWriterWatchedRangeV1 {
                    physical_start: 0x400,
                    physical_end: 0x800,
                }],
                before_sha256: "c6".repeat(32),
                after_sha256: "c5".repeat(32),
                invalidated_generations: Vec::new(),
                journal_root_sha256: "c7".repeat(32),
            },
            final_watched_sha256: "c5".repeat(32),
            receipt_sha256: String::new(),
        };
        prerequisite.journal_entry.journal_root_sha256 =
            recompute_bootstrap_canonical_journal_root(
                &prerequisite.watched_ranges,
                &prerequisite.journal_entry,
            )
            .unwrap();
        prerequisite.receipt_sha256 =
            recompute_bootstrap_runtime_prerequisite_receipt(&prerequisite).unwrap();
        prerequisite
    }

    fn bootstrap_report(
        nonce: [u8; 32],
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> GeneratedRunnerBootstrapRuntimeReportV1 {
        GeneratedRunnerBootstrapRuntimeReportV1 {
            schema: GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
            nonce: hex(&nonce),
            build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
            program_identity_sha256: identity.program_identity_sha256.clone(),
            prerequisite: bootstrap_prerequisite(identity),
        }
    }

    fn bootstrap_report_output(report: &GeneratedRunnerBootstrapRuntimeReportV1) -> Vec<u8> {
        format!(
            "{}{report}\n",
            GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1,
            report = serde_json::to_string(report).unwrap()
        )
        .into_bytes()
    }

    fn cpu_prerequisite(
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> CpuWriterRuntimePrerequisiteV1 {
        let mut prerequisite = CpuWriterRuntimePrerequisiteV1 {
            schema: fn64_abi::recompiled::CPU_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
            program_model_sha256: "a1".repeat(32),
            resolver_install_sha256: "d2".repeat(32),
            abi_host_catalog_receipt_sha256: "d3".repeat(32),
            build_receipt_schema: identity.build_receipt_schema,
            aot_runtime: identity.aot_runtime,
            production_aot: identity.production_aot,
            dev_interpreter: identity.dev_interpreter,
            trace_epoch_id: 1,
            watched_ranges: vec![CpuWriterWatchedRangeV1 {
                physical_start: 0x400,
                physical_end: 0x800,
            }],
            journal_entry_count: 1,
            cpu_journal_declaration_count: 0,
            journal_root_sha256: "d4".repeat(32),
            final_watched_sha256: "d5".repeat(32),
            cpu_store_count: 3,
            cpu_store_trace_sha256: "d6".repeat(32),
            receipt_sha256: String::new(),
        };
        prerequisite.receipt_sha256 =
            recompute_cpu_runtime_prerequisite_receipt(&prerequisite).unwrap();
        prerequisite
    }

    fn cpu_report(
        nonce: [u8; 32],
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> GeneratedRunnerCpuRuntimeReportV1 {
        GeneratedRunnerCpuRuntimeReportV1 {
            schema: GENERATED_RUNNER_CPU_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
            nonce: hex(&nonce),
            build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
            program_identity_sha256: identity.program_identity_sha256.clone(),
            prerequisite: cpu_prerequisite(identity),
        }
    }

    fn cpu_report_output(report: &GeneratedRunnerCpuRuntimeReportV1) -> Vec<u8> {
        format!(
            "{}{report}\n",
            GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1,
            report = serde_json::to_string(report).unwrap()
        )
        .into_bytes()
    }

    fn host_abi_prerequisite(
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> HostAbiWriterRuntimePrerequisiteV1 {
        let mut prerequisite = HostAbiWriterRuntimePrerequisiteV1 {
            schema: fn64_abi::recompiled::HOST_ABI_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
            program_model_sha256: "a1".repeat(32),
            resolver_install_sha256: "c2".repeat(32),
            abi_host_catalog_receipt_sha256: "c3".repeat(32),
            build_receipt_schema: identity.build_receipt_schema,
            aot_runtime: identity.aot_runtime,
            production_aot: identity.production_aot,
            dev_interpreter: identity.dev_interpreter,
            trace_epoch_id: 1,
            initial_journal_entry_count: 1,
            final_journal_entry_count: 2,
            watched_ranges: vec![HostAbiWriterWatchedRangeV1 {
                physical_start: 0x400,
                physical_end: 0x800,
            }],
            host_abi_journal_entry_count: 1,
            host_abi_journal_declaration_count: 1,
            journal_root_sha256: "c4".repeat(32),
            final_watched_sha256: "c5".repeat(32),
            transactions_started: 1,
            transactions_finished: 1,
            ordering_boundaries: 1,
            lifecycle_sha256: "c6".repeat(32),
            receipt_sha256: String::new(),
        };
        prerequisite.receipt_sha256 =
            recompute_host_abi_runtime_prerequisite_receipt(&prerequisite).unwrap();
        prerequisite
    }

    fn host_abi_report(
        nonce: [u8; 32],
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> GeneratedRunnerHostAbiRuntimeReportV1 {
        GeneratedRunnerHostAbiRuntimeReportV1 {
            schema: GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
            nonce: hex(&nonce),
            build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
            program_identity_sha256: identity.program_identity_sha256.clone(),
            prerequisite: host_abi_prerequisite(identity),
        }
    }

    fn host_abi_report_output(report: &GeneratedRunnerHostAbiRuntimeReportV1) -> Vec<u8> {
        format!(
            "{}{report}\n",
            GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1,
            report = serde_json::to_string(report).unwrap()
        )
        .into_bytes()
    }

    fn pi_prerequisite(identity: &GeneratedRunnerBuildIdentityV1) -> PiWriterRuntimePrerequisiteV1 {
        let mut prerequisite = PiWriterRuntimePrerequisiteV1 {
            schema: fn64_abi::recompiled::PI_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
            program_model_sha256: "a1".repeat(32),
            resolver_install_sha256: "e2".repeat(32),
            abi_host_catalog_receipt_sha256: "e3".repeat(32),
            build_receipt_schema: identity.build_receipt_schema,
            aot_runtime: identity.aot_runtime,
            production_aot: identity.production_aot,
            dev_interpreter: identity.dev_interpreter,
            trace_epoch_id: 1,
            watched_ranges: vec![PiWriterWatchedRangeV1 {
                physical_start: 0x400,
                physical_end: 0x800,
            }],
            journal_entry_count: 1,
            pi_journal_declaration_count: 0,
            journal_root_sha256: "e4".repeat(32),
            final_watched_sha256: "e5".repeat(32),
            pi_started: 1,
            pi_committed: 1,
            pi_busy_cleared: 1,
            pi_interrupt_raised: 1,
            pi_interrupt_cleared: 1,
            pi_notifications: 1,
            pi_to_rdram_committed: 1,
            pi_transition_sha256: "e6".repeat(32),
            receipt_sha256: String::new(),
        };
        prerequisite.receipt_sha256 =
            recompute_pi_runtime_prerequisite_receipt(&prerequisite).unwrap();
        prerequisite
    }

    fn pi_report(
        nonce: [u8; 32],
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> GeneratedRunnerPiRuntimeReportV1 {
        GeneratedRunnerPiRuntimeReportV1 {
            schema: GENERATED_RUNNER_PI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
            nonce: hex(&nonce),
            build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
            program_identity_sha256: identity.program_identity_sha256.clone(),
            prerequisite: pi_prerequisite(identity),
        }
    }

    fn pi_report_output(report: &GeneratedRunnerPiRuntimeReportV1) -> Vec<u8> {
        format!(
            "{}{report}\n",
            GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1,
            report = serde_json::to_string(report).unwrap()
        )
        .into_bytes()
    }

    fn rdp_renderer_prerequisite(
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> RdpRendererWriterRuntimePrerequisiteV1 {
        let mut prerequisite = RdpRendererWriterRuntimePrerequisiteV1 {
            schema: fn64_abi::recompiled::RDP_RENDERER_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
            program_model_sha256: "a1".repeat(32),
            resolver_install_sha256: "f2".repeat(32),
            abi_host_catalog_receipt_sha256: "f3".repeat(32),
            build_receipt_schema: identity.build_receipt_schema,
            aot_runtime: identity.aot_runtime,
            production_aot: identity.production_aot,
            dev_interpreter: identity.dev_interpreter,
            trace_epoch_id: 1,
            initial_journal_entry_count: 1,
            final_journal_entry_count: 2,
            watched_ranges: vec![RdpRendererWriterWatchedRangeV1 {
                physical_start: 0x400,
                physical_end: 0x800,
            }],
            rdp_renderer_journal_entry_count: 1,
            rdp_renderer_journal_declaration_count: 1,
            journal_root_sha256: "f4".repeat(32),
            final_watched_sha256: "f5".repeat(32),
            renderer_publication_count: 1,
            publication_trace_sha256: "f6".repeat(32),
            receipt_sha256: String::new(),
        };
        prerequisite.receipt_sha256 =
            recompute_rdp_renderer_runtime_prerequisite_receipt(&prerequisite).unwrap();
        prerequisite
    }

    fn rdp_renderer_report(
        nonce: [u8; 32],
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> GeneratedRunnerRdpRendererRuntimeReportV1 {
        GeneratedRunnerRdpRendererRuntimeReportV1 {
            schema: GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
            nonce: hex(&nonce),
            build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
            program_identity_sha256: identity.program_identity_sha256.clone(),
            prerequisite: rdp_renderer_prerequisite(identity),
        }
    }

    fn rdp_renderer_report_output(report: &GeneratedRunnerRdpRendererRuntimeReportV1) -> Vec<u8> {
        format!(
            "{}{report}\n",
            GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1,
            report = serde_json::to_string(report).unwrap()
        )
        .into_bytes()
    }

    fn rsp_prerequisite(
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> RspWriterRuntimePrerequisiteV1 {
        let mut prerequisite = RspWriterRuntimePrerequisiteV1 {
            schema: fn64_abi::recompiled::RSP_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
            program_model_sha256: "a1".repeat(32),
            resolver_install_sha256: "d2".repeat(32),
            abi_host_catalog_receipt_sha256: "d3".repeat(32),
            build_receipt_schema: identity.build_receipt_schema,
            aot_runtime: identity.aot_runtime,
            production_aot: identity.production_aot,
            dev_interpreter: identity.dev_interpreter,
            trace_epoch_id: 1,
            watched_ranges: vec![RspWriterWatchedRangeV1 {
                physical_start: 0x400,
                physical_end: 0x800,
            }],
            journal_entry_count: 1,
            rsp_journal_declaration_count: 1,
            journal_root_sha256: "d4".repeat(32),
            final_watched_sha256: "d5".repeat(32),
            interpreter_writeback_count: 1,
            translated_audio_hle_publication_count: 0,
            writeback_range_count: 1,
            writeback_trace_sha256: "d6".repeat(32),
            receipt_sha256: String::new(),
        };
        prerequisite.receipt_sha256 =
            recompute_rsp_runtime_prerequisite_receipt(&prerequisite).unwrap();
        prerequisite
    }

    fn rsp_report(
        nonce: [u8; 32],
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> GeneratedRunnerRspRuntimeReportV1 {
        GeneratedRunnerRspRuntimeReportV1 {
            schema: GENERATED_RUNNER_RSP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
            nonce: hex(&nonce),
            build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
            program_identity_sha256: identity.program_identity_sha256.clone(),
            prerequisite: rsp_prerequisite(identity),
        }
    }

    fn rsp_report_output(report: &GeneratedRunnerRspRuntimeReportV1) -> Vec<u8> {
        format!(
            "{}{report}\n",
            GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1,
            report = serde_json::to_string(report).unwrap()
        )
        .into_bytes()
    }

    fn si_prerequisite(identity: &GeneratedRunnerBuildIdentityV1) -> SiWriterRuntimePrerequisiteV1 {
        let mut prerequisite = SiWriterRuntimePrerequisiteV1 {
            schema: fn64_abi::recompiled::SI_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
            program_model_sha256: "a1".repeat(32),
            resolver_install_sha256: "a2".repeat(32),
            abi_host_catalog_receipt_sha256: "a3".repeat(32),
            build_receipt_schema: identity.build_receipt_schema,
            aot_runtime: identity.aot_runtime,
            production_aot: identity.production_aot,
            dev_interpreter: identity.dev_interpreter,
            watched_ranges: vec![SiWriterWatchedRangeV1 {
                physical_start: 0x400,
                physical_end: 0x800,
            }],
            journal_entry_count: 2,
            si_journal_declaration_count: 0,
            journal_root_sha256: "a4".repeat(32),
            final_watched_sha256: "a5".repeat(32),
            si_started: 1,
            si_committed: 1,
            si_pif_to_dram_committed: 1,
            si_transition_sha256: "a6".repeat(32),
            receipt_sha256: String::new(),
        };
        prerequisite.receipt_sha256 =
            recompute_si_runtime_prerequisite_receipt(&prerequisite).unwrap();
        prerequisite
    }

    fn si_report(
        nonce: [u8; 32],
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> GeneratedRunnerSiRuntimeReportV1 {
        let identity_bytes = serde_json::to_vec(identity).unwrap();
        GeneratedRunnerSiRuntimeReportV1 {
            schema: GENERATED_RUNNER_SI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
            nonce: hex(&nonce),
            build_identity_sha256: hex(&Sha256::digest(identity_bytes)),
            program_identity_sha256: identity.program_identity_sha256.clone(),
            prerequisite: si_prerequisite(identity),
        }
    }

    fn si_report_output(report: &GeneratedRunnerSiRuntimeReportV1) -> Vec<u8> {
        format!(
            "{}{report}\n",
            GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1,
            report = serde_json::to_string(report).unwrap()
        )
        .into_bytes()
    }

    fn sp_prerequisite(identity: &GeneratedRunnerBuildIdentityV1) -> SpWriterRuntimePrerequisiteV1 {
        let mut prerequisite = SpWriterRuntimePrerequisiteV1 {
            schema: fn64_abi::recompiled::SP_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
            program_model_sha256: "a1".repeat(32),
            resolver_install_sha256: "b2".repeat(32),
            abi_host_catalog_receipt_sha256: "b3".repeat(32),
            build_receipt_schema: identity.build_receipt_schema,
            aot_runtime: identity.aot_runtime,
            production_aot: identity.production_aot,
            dev_interpreter: identity.dev_interpreter,
            trace_epoch_id: 1,
            watched_ranges: vec![SpWriterWatchedRangeV1 {
                physical_start: 0x400,
                physical_end: 0x800,
            }],
            journal_entry_count: 1,
            sp_journal_declaration_count: 0,
            journal_root_sha256: "b4".repeat(32),
            final_watched_sha256: "b5".repeat(32),
            sp_started: 2,
            sp_queued: 0,
            sp_committed: 2,
            sp_busy_cleared: 2,
            sp_rsp_to_rdram_committed: 1,
            sp_transition_sha256: "b6".repeat(32),
            receipt_sha256: String::new(),
        };
        prerequisite.receipt_sha256 =
            recompute_sp_runtime_prerequisite_receipt(&prerequisite).unwrap();
        prerequisite
    }

    fn sp_report(
        nonce: [u8; 32],
        identity: &GeneratedRunnerBuildIdentityV1,
    ) -> GeneratedRunnerSpRuntimeReportV1 {
        GeneratedRunnerSpRuntimeReportV1 {
            schema: GENERATED_RUNNER_SP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
            nonce: hex(&nonce),
            build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
            program_identity_sha256: identity.program_identity_sha256.clone(),
            prerequisite: sp_prerequisite(identity),
        }
    }

    fn sp_report_output(report: &GeneratedRunnerSpRuntimeReportV1) -> Vec<u8> {
        format!(
            "{}{report}\n",
            GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1,
            report = serde_json::to_string(report).unwrap()
        )
        .into_bytes()
    }

    fn build_evidence() -> GeneratedRunnerBuildEvidenceV1 {
        let mut evidence = GeneratedRunnerBuildEvidenceV1 {
            schema: VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V5,
            builder_cargo_sha256: "91".repeat(32),
            cargo_graph_sha256: "92".repeat(32),
            cargo_source_sha256: "93".repeat(32),
            build_environment_sha256: "98".repeat(32),
            builder_rustc_sha256: "99".repeat(32),
            cargo_config_sha256: "9a".repeat(32),
            memory_guard_sha256: "97".repeat(32),
            selected_build_cargo_jobs: SELECTED_BUILD_CARGO_JOBS_V5,
            build_max_rss_mib: BUILD_MAX_RSS_MIB,
            build_min_free_percent: BUILD_MIN_FREE_PERCENT,
            max_build_seconds: 60 * 60,
            selected_binary_sha256: "94".repeat(32),
            private_build_inputs_sha256: "95".repeat(32),
            prepared_tree_descriptor_sha256: "96".repeat(32),
            prepared_tree_sha256: "6a".repeat(32),
            prepared_source_mode: PREPARED_SOURCE_MODE_INACTIVE_V1.to_owned(),
            producer_manifest_sha256: "70".repeat(32),
            producer_lock_sha256: "71".repeat(32),
            producer_cargo_graph_sha256: "72".repeat(32),
            producer_cargo_source_sha256: "73".repeat(32),
            producer_binary_sha256: "74".repeat(32),
            identity: identity(),
            authority_sha256: String::new(),
        };
        evidence.authority_sha256 = evidence.recompute_authority_sha256();
        evidence
    }

    fn writer_audit_bundle_evidence() -> GeneratedRunnerWriterAuditBundleEvidenceV1 {
        let build = build_evidence();
        let bootstrap_observed = (0u8..10)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, bootstrap_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let si_observed = (10u8..20)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, si_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let cpu_observed = (30u8..40)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, cpu_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let sp_observed = (20u8..30)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, sp_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let pi_observed = (40u8..50)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, pi_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let host_abi_observed = (50u8..60)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, host_abi_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let rdp_renderer_observed = (60u8..70)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, rdp_renderer_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let rsp_observed = (70u8..80)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, rsp_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let mut evidence = GeneratedRunnerWriterAuditBundleEvidenceV1 {
            schema: VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1,
            completed_channels: WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1
                | WRITER_AUDIT_CPU_COMPLETED_V1
                | WRITER_AUDIT_HOST_ABI_COMPLETED_V1
                | WRITER_AUDIT_PI_COMPLETED_V1
                | WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
                | WRITER_AUDIT_RSP_COMPLETED_V1
                | WRITER_AUDIT_SI_COMPLETED_V1
                | WRITER_AUDIT_SP_COMPLETED_V1,
            build_authority_sha256: build.authority_sha256.clone(),
            selected_binary_sha256: build.selected_binary_sha256.clone(),
            private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
            bootstrap: Some(
                validate_bootstrap_runtime_series(&build, &bootstrap_observed).unwrap(),
            ),
            cpu: Some(validate_cpu_runtime_series(&build, &cpu_observed).unwrap()),
            host_abi: Some(validate_host_abi_runtime_series(&build, &host_abi_observed).unwrap()),
            pi: Some(validate_pi_runtime_series(&build, &pi_observed).unwrap()),
            rdp_renderer: Some(
                validate_rdp_renderer_runtime_series(&build, &rdp_renderer_observed).unwrap(),
            ),
            rsp: Some(validate_rsp_runtime_series(&build, &rsp_observed).unwrap()),
            si: Some(validate_si_runtime_series(&build, &si_observed).unwrap()),
            sp: Some(validate_sp_runtime_series(&build, &sp_observed).unwrap()),
            authority_sha256: String::new(),
        };
        evidence.authority_sha256 = writer_audit_bundle_authority_sha256(&evidence).unwrap();
        evidence
    }

    #[test]
    fn identity_validator_recomputes_runner_binding_and_production_features() {
        let valid = identity();
        validate_identity(&valid, &valid.manifest_sha256, &valid.lock_sha256).unwrap();

        let mut wrong_role = valid.clone();
        wrong_role.runners[0].adapter_role = GeneratedRunnerAdapterRoleV1::EntryContextGate;
        assert!(validate_identity(
            &wrong_role,
            &wrong_role.manifest_sha256,
            &wrong_role.lock_sha256
        )
        .is_err());

        let mut interpreter = valid.clone();
        interpreter.production_aot = false;
        interpreter.dev_interpreter = true;
        assert!(validate_identity(
            &interpreter,
            &interpreter.manifest_sha256,
            &interpreter.lock_sha256
        )
        .is_err());
    }

    #[test]
    fn prepared_tree_measurement_binds_content_separately_from_descriptors() {
        let (_first_scratch, first_root, claims, rom) = synthetic_prepared_tree(None);
        let first = measure_prepared_tree_v3(&first_root, &rom, &claims).unwrap();
        let (_second_scratch, second_root, _, _) = synthetic_prepared_tree(None);
        let second = measure_prepared_tree_v3(&second_root, &rom, &claims).unwrap();
        assert_eq!(first.tree_sha256, second.tree_sha256);
        assert_ne!(first.descriptor_sha256, second.descriptor_sha256);

        let (_changed_scratch, changed_root, _, _) =
            synthetic_prepared_tree(Some(PREPARED_PACKAGES[24]));
        let changed = measure_prepared_tree_v3(&changed_root, &rom, &claims).unwrap();
        assert_ne!(first.tree_sha256, changed.tree_sha256);
    }

    #[test]
    fn prepared_tree_measurement_rejects_extra_marker_and_digest_drift() {
        let (_scratch, root, claims, rom) = synthetic_prepared_tree(None);
        let extra = root.join("extra");
        fs::write(&extra, b"extra").unwrap();
        assert!(measure_prepared_tree_v3(&root, &rom, &claims).is_err());
        fs::remove_file(extra).unwrap();

        let marker = root.join(PREPARED_UPDATE_MARKER_NAME);
        fs::write(&marker, b"update").unwrap();
        assert!(measure_prepared_tree_v3(&root, &rom, &claims).is_err());
        fs::remove_file(marker).unwrap();

        fs::write(root.join(PREPARED_PACKAGES[0]).join("runner.rs"), b"drift").unwrap();
        assert!(measure_prepared_tree_v3(&root, &rom, &claims).is_err());
    }

    #[test]
    fn wm_shard_source_graph_uses_hardened_sibling_paths() {
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = crate_root
            .parent()
            .and_then(Path::parent)
            .expect("boot-harness crate is under the workspace crates directory");
        let package_root = repo_root.join("examples/wm2000-block-boot");
        let shard_root = wm_shard_root(&package_root).expect("derive shard sibling");
        assert!(
            !shard_root
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir)),
            "hardened source readers reject lexical parent traversal"
        );

        let mode = wm_prepared_source_mode_v3(&package_root).expect("classify shard source mode");
        let digest = wm_shard_cargo_source_sha256(&package_root, mode)
            .expect("hash the exact shard source graph through hardened reads");
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn bounded_diagnostic_retains_the_command_failure_tail() {
        let diagnostic =
            bounded_diagnostic(format!("{}\nactual error", "progress\n".repeat(600)).as_bytes());
        assert!(diagnostic.starts_with("<earlier output truncated>\n"));
        assert!(diagnostic.ends_with("actual error"));
    }

    #[cfg(unix)]
    #[test]
    fn nonzero_writer_child_error_retains_bounded_stderr_tail() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "printf 'private diagnostic tail' >&2; exit 17"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let error =
            launch_writer_runtime_child_output(command, 3, WriterRuntimeAuditProtocol::Bootstrap)
                .unwrap_err()
                .to_string();
        assert!(error.contains("bootstrap audit child run 3 exited"));
        assert!(error.contains("stderr_bytes=23"));
        assert!(error.ends_with("stderr: private diagnostic tail"));
    }

    #[test]
    fn writer_runtime_transport_extracts_one_strict_report_amid_diagnostics() {
        let identity = identity();
        let nonce = [0x21; 32];
        let report = bootstrap_report(nonce, &identity);
        let report_wire = bootstrap_report_output(&report);
        let mut stdout = b"ordinary runtime diagnostic\n".to_vec();
        stdout.extend_from_slice(&report_wire);
        stdout.extend_from_slice(b"later ordinary diagnostic\n");

        let envelope =
            extract_writer_runtime_report_envelope(&stdout, WriterRuntimeAuditProtocol::Bootstrap)
                .unwrap();
        assert_eq!(envelope, report_wire);
        assert_eq!(
            parse_generated_runner_bootstrap_runtime_report_v1(&envelope, nonce, &identity)
                .unwrap(),
            report
        );
    }

    #[test]
    fn writer_runtime_transport_rejects_zero_multiple_malformed_and_over_limit_reports() {
        let protocol = WriterRuntimeAuditProtocol::Bootstrap;
        assert!(extract_writer_runtime_report_envelope(b"diagnostic only\n", protocol).is_err());

        let minimal = format!("{}{{}}\n", protocol.report_prefix());
        let duplicate = format!("{minimal}{minimal}");
        assert!(extract_writer_runtime_report_envelope(duplicate.as_bytes(), protocol).is_err());

        let malformed = extract_writer_runtime_report_envelope(minimal.as_bytes(), protocol)
            .expect("transport accepts one prefixed envelope for semantic validation");
        assert!(parse_generated_runner_bootstrap_runtime_report_v1(
            &malformed,
            [0x21; 32],
            &identity(),
        )
        .is_err());

        assert!(writer_runtime_outputs_within_limit(
            WRITER_RUNTIME_OUTPUT_LIMIT as u64,
            0,
        ));
        assert!(!writer_runtime_outputs_within_limit(
            WRITER_RUNTIME_OUTPUT_LIMIT as u64 + 1,
            0,
        ));

        let mut oversized = protocol.report_prefix().as_bytes().to_vec();
        oversized.resize(WRITER_RUNTIME_REPORT_LIMIT + 1, b'x');
        oversized.push(b'\n');
        assert!(extract_writer_runtime_report_envelope(&oversized, protocol).is_err());
    }

    #[test]
    fn independent_emitter_source_measurement_matches_the_linked_receipt() {
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = crate_root
            .parent()
            .and_then(Path::parent)
            .expect("boot-harness crate is under the workspace crates directory");
        let measured = wm_emitter_source_sha256(repo_root).expect("measure emitter source");
        let linked = hex(
            &fn64_recomp_rs_codegen::generated_runner_emitter_source_receipt_v2().source_sha256(),
        );
        assert_eq!(measured, linked);
    }

    #[test]
    fn identity_output_requires_exactly_one_prefixed_envelope() {
        let wire = serde_json::to_string(&identity()).unwrap();
        let output = format!("diagnostic\n{GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1}{wire}\n");
        assert_eq!(
            parse_identity_output(output.as_bytes()).unwrap(),
            identity()
        );
        assert!(parse_identity_output(b"diagnostic only\n").is_err());
        let repeated = format!(
            "{GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1}{wire}\n{GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1}{wire}\n"
        );
        assert!(parse_identity_output(repeated.as_bytes()).is_err());
    }

    #[test]
    fn identity_wire_denies_unknown_fields_and_requires_bank_order() {
        let mut unknown = serde_json::to_value(identity()).unwrap();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("caller_claim".to_owned(), serde_json::Value::Bool(true));
        assert!(serde_json::from_value::<GeneratedRunnerBuildIdentityV1>(unknown).is_err());

        let mut unsorted = identity();
        let mut earlier = unsorted.runners[0].clone();
        earlier.bank -= 1;
        unsorted.runners.push(earlier);
        unsorted.binding_sha256 = recompute_binding_sha256(&unsorted).unwrap();
        assert!(
            validate_identity(&unsorted, &unsorted.manifest_sha256, &unsorted.lock_sha256,)
                .is_err()
        );
    }

    #[test]
    fn bootstrap_runtime_report_is_one_nonce_bound_deny_unknown_sequence_zero_envelope() {
        let identity = identity();
        let nonce = [0x21; 32];
        let report = bootstrap_report(nonce, &identity);
        let output = bootstrap_report_output(&report);
        assert_eq!(
            parse_generated_runner_bootstrap_runtime_report_v1(&output, nonce, &identity).unwrap(),
            report
        );
        assert!(
            parse_generated_runner_bootstrap_runtime_report_v1(&output, [0x22; 32], &identity)
                .is_err()
        );
        let mut duplicate = output.clone();
        duplicate.extend_from_slice(&output);
        assert!(
            parse_generated_runner_bootstrap_runtime_report_v1(&duplicate, nonce, &identity)
                .is_err()
        );
        assert!(parse_generated_runner_bootstrap_runtime_report_v1(
            &output[..output.len() - 1],
            nonce,
            &identity
        )
        .is_err());

        let mut unknown = serde_json::to_value(&report).unwrap();
        unknown["prerequisite"]
            .as_object_mut()
            .unwrap()
            .insert("caller_claim".to_owned(), serde_json::Value::Bool(true));
        let unknown = format!(
            "{}{}\n",
            GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1,
            serde_json::to_string(&unknown).unwrap()
        );
        assert!(parse_generated_runner_bootstrap_runtime_report_v1(
            unknown.as_bytes(),
            nonce,
            &identity
        )
        .is_err());

        let mut later_entry = report.clone();
        later_entry.prerequisite.journal_entry.sequence = 1;
        later_entry.prerequisite.receipt_sha256 =
            recompute_bootstrap_runtime_prerequisite_receipt(&later_entry.prerequisite).unwrap();
        assert!(parse_generated_runner_bootstrap_runtime_report_v1(
            &bootstrap_report_output(&later_entry),
            nonce,
            &identity
        )
        .is_err());

        let mut wrong_rom = report.clone();
        wrong_rom.prerequisite.rom_sha256 = "ff".repeat(32);
        wrong_rom.prerequisite.receipt_sha256 =
            recompute_bootstrap_runtime_prerequisite_receipt(&wrong_rom.prerequisite).unwrap();
        assert!(parse_generated_runner_bootstrap_runtime_report_v1(
            &bootstrap_report_output(&wrong_rom),
            nonce,
            &identity
        )
        .is_err());

        let mut zero_generation = report.clone();
        zero_generation.prerequisite.initial_generations[0] = 0;
        zero_generation.prerequisite.receipt_sha256 =
            recompute_bootstrap_runtime_prerequisite_receipt(&zero_generation.prerequisite)
                .unwrap();
        assert!(parse_generated_runner_bootstrap_runtime_report_v1(
            &bootstrap_report_output(&zero_generation),
            nonce,
            &identity
        )
        .is_err());

        let mut forged_journal_root = report.clone();
        forged_journal_root
            .prerequisite
            .journal_entry
            .journal_root_sha256 = "fd".repeat(32);
        forged_journal_root.prerequisite.receipt_sha256 =
            recompute_bootstrap_runtime_prerequisite_receipt(&forged_journal_root.prerequisite)
                .unwrap();
        assert!(parse_generated_runner_bootstrap_runtime_report_v1(
            &bootstrap_report_output(&forged_journal_root),
            nonce,
            &identity
        )
        .is_err());

        let mut bad_receipt = report;
        bad_receipt.prerequisite.receipt_sha256 = "fe".repeat(32);
        assert!(parse_generated_runner_bootstrap_runtime_report_v1(
            &bootstrap_report_output(&bad_receipt),
            nonce,
            &identity
        )
        .is_err());
    }

    #[test]
    fn cpu_runtime_report_requires_one_nonce_bound_deny_unknown_envelope() {
        let identity = identity();
        let nonce = [0x31; 32];
        let report = cpu_report(nonce, &identity);
        let output = cpu_report_output(&report);
        assert_eq!(
            parse_generated_runner_cpu_runtime_report_v1(&output, nonce, &identity).unwrap(),
            report
        );
        assert!(
            parse_generated_runner_cpu_runtime_report_v1(&output, [0x32; 32], &identity).is_err()
        );
        let mut duplicate = output.clone();
        duplicate.extend_from_slice(&output);
        assert!(
            parse_generated_runner_cpu_runtime_report_v1(&duplicate, nonce, &identity).is_err()
        );

        let mut value = serde_json::to_value(&report).unwrap();
        value["unexpected"] = serde_json::json!(true);
        let unknown = format!(
            "{}{}\n",
            GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1,
            serde_json::to_string(&value).unwrap()
        );
        assert!(
            parse_generated_runner_cpu_runtime_report_v1(unknown.as_bytes(), nonce, &identity)
                .is_err()
        );
    }

    #[test]
    fn cpu_runtime_report_recomputes_receipt_and_requires_fresh_store_evidence() {
        let identity = identity();
        let nonce = [0x33; 32];
        let mut report = cpu_report(nonce, &identity);
        report.prerequisite.cpu_store_count = 0;
        report.prerequisite.receipt_sha256 =
            recompute_cpu_runtime_prerequisite_receipt(&report.prerequisite).unwrap();
        assert!(parse_generated_runner_cpu_runtime_report_v1(
            &cpu_report_output(&report),
            nonce,
            &identity
        )
        .is_err());

        let mut report = cpu_report(nonce, &identity);
        report.prerequisite.cpu_store_trace_sha256 = "fe".repeat(32);
        assert!(parse_generated_runner_cpu_runtime_report_v1(
            &cpu_report_output(&report),
            nonce,
            &identity
        )
        .is_err());
    }

    #[test]
    fn cpu_runtime_series_requires_ten_distinct_semantically_identical_reports() {
        let build = build_evidence();
        let observed = (0u8..10)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, cpu_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let evidence = validate_cpu_runtime_series(&build, &observed).unwrap();
        validate_cpu_runtime_series_evidence(&evidence).unwrap();

        let mut repeated = observed.clone();
        repeated[9].0 = repeated[0].0;
        repeated[9].1 = cpu_report(repeated[0].0, &build.identity);
        assert!(validate_cpu_runtime_series(&build, &repeated).is_err());

        let mut changed = observed;
        changed[9].1.prerequisite.cpu_store_count += 1;
        changed[9].1.prerequisite.receipt_sha256 =
            recompute_cpu_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
        assert!(validate_cpu_runtime_series(&build, &changed).is_err());

        let mut tampered = evidence;
        tampered.cpu_store_trace_sha256 = "ff".repeat(32);
        assert!(validate_cpu_runtime_series_evidence(&tampered).is_err());
    }

    #[test]
    fn host_abi_runtime_report_is_strict_nonce_bound_and_recomputes_receipt() {
        let identity = identity();
        let nonce = [0x43; 32];
        let report = host_abi_report(nonce, &identity);
        let output = host_abi_report_output(&report);
        assert_eq!(
            parse_generated_runner_host_abi_runtime_report_v1(&output, nonce, &identity).unwrap(),
            report
        );
        assert!(
            parse_generated_runner_host_abi_runtime_report_v1(&output, [0x44; 32], &identity)
                .is_err()
        );
        let mut duplicate = output.clone();
        duplicate.extend_from_slice(&output);
        assert!(
            parse_generated_runner_host_abi_runtime_report_v1(&duplicate, nonce, &identity)
                .is_err()
        );

        let mut unknown = serde_json::to_value(&report).unwrap();
        unknown["prerequisite"]
            .as_object_mut()
            .unwrap()
            .insert("raw_pointer_catalog".to_owned(), serde_json::json!(true));
        let unknown = format!(
            "{}{}\n",
            GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1,
            serde_json::to_string(&unknown).unwrap()
        );
        assert!(parse_generated_runner_host_abi_runtime_report_v1(
            unknown.as_bytes(),
            nonce,
            &identity
        )
        .is_err());

        let mut no_write = host_abi_report(nonce, &identity);
        no_write.prerequisite.host_abi_journal_declaration_count = 0;
        no_write.prerequisite.receipt_sha256 =
            recompute_host_abi_runtime_prerequisite_receipt(&no_write.prerequisite).unwrap();
        assert!(parse_generated_runner_host_abi_runtime_report_v1(
            &host_abi_report_output(&no_write),
            nonce,
            &identity
        )
        .is_err());

        let mut tampered = host_abi_report(nonce, &identity);
        tampered.prerequisite.lifecycle_sha256 = "fe".repeat(32);
        assert!(parse_generated_runner_host_abi_runtime_report_v1(
            &host_abi_report_output(&tampered),
            nonce,
            &identity
        )
        .is_err());
    }

    #[test]
    fn host_abi_runtime_series_requires_exact_ten_identical_canonical_reports() {
        let build = build_evidence();
        let observed = (0u8..10)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, host_abi_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let evidence = validate_host_abi_runtime_series(&build, &observed).unwrap();
        validate_host_abi_runtime_series_evidence(&evidence).unwrap();

        let mut repeated = observed.clone();
        repeated[9].0 = repeated[0].0;
        repeated[9].1 = host_abi_report(repeated[0].0, &build.identity);
        assert!(validate_host_abi_runtime_series(&build, &repeated).is_err());

        let mut changed = observed;
        changed[9].1.prerequisite.transactions_started += 1;
        changed[9].1.prerequisite.transactions_finished += 1;
        changed[9].1.prerequisite.receipt_sha256 =
            recompute_host_abi_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
        assert!(validate_host_abi_runtime_series(&build, &changed).is_err());

        let mut tampered = evidence;
        tampered.lifecycle_sha256 = "ff".repeat(32);
        assert!(validate_host_abi_runtime_series_evidence(&tampered).is_err());
    }

    #[test]
    fn rdp_renderer_report_requires_one_nonce_bound_deny_unknown_envelope() {
        let identity = identity();
        let nonce = [0x61; 32];
        let report = rdp_renderer_report(nonce, &identity);
        let output = rdp_renderer_report_output(&report);
        assert_eq!(
            parse_generated_runner_rdp_renderer_runtime_report_v1(&output, nonce, &identity)
                .unwrap(),
            report
        );
        assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
            &output, [0x62; 32], &identity
        )
        .is_err());
        let mut duplicate = output.clone();
        duplicate.extend_from_slice(&output);
        assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
            &duplicate, nonce, &identity
        )
        .is_err());

        let mut unknown = serde_json::to_value(&report).unwrap();
        unknown["prerequisite"]
            .as_object_mut()
            .unwrap()
            .insert("needs_lle_count".to_owned(), serde_json::json!(1));
        let unknown = format!(
            "{}{}\n",
            GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1,
            serde_json::to_string(&unknown).unwrap()
        );
        assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
            unknown.as_bytes(),
            nonce,
            &identity
        )
        .is_err());
    }

    #[test]
    fn rdp_renderer_report_requires_actual_executable_publication_and_recomputed_receipt() {
        let identity = identity();
        let nonce = [0x63; 32];

        let mut needs_lle_only = rdp_renderer_report(nonce, &identity);
        needs_lle_only.prerequisite.final_journal_entry_count =
            needs_lle_only.prerequisite.initial_journal_entry_count;
        needs_lle_only.prerequisite.rdp_renderer_journal_entry_count = 0;
        needs_lle_only
            .prerequisite
            .rdp_renderer_journal_declaration_count = 0;
        needs_lle_only.prerequisite.renderer_publication_count = 0;
        needs_lle_only.prerequisite.receipt_sha256 =
            recompute_rdp_renderer_runtime_prerequisite_receipt(&needs_lle_only.prerequisite)
                .unwrap();
        assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
            &rdp_renderer_report_output(&needs_lle_only),
            nonce,
            &identity
        )
        .is_err());

        let mut framebuffer_only = rdp_renderer_report(nonce, &identity);
        framebuffer_only.prerequisite.final_journal_entry_count =
            framebuffer_only.prerequisite.initial_journal_entry_count;
        framebuffer_only
            .prerequisite
            .rdp_renderer_journal_entry_count = 0;
        framebuffer_only
            .prerequisite
            .rdp_renderer_journal_declaration_count = 0;
        framebuffer_only.prerequisite.receipt_sha256 =
            recompute_rdp_renderer_runtime_prerequisite_receipt(&framebuffer_only.prerequisite)
                .unwrap();
        assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
            &rdp_renderer_report_output(&framebuffer_only),
            nonce,
            &identity
        )
        .is_err());

        let mut tampered = rdp_renderer_report(nonce, &identity);
        tampered.prerequisite.publication_trace_sha256 = "ee".repeat(32);
        assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
            &rdp_renderer_report_output(&tampered),
            nonce,
            &identity
        )
        .is_err());
    }

    #[test]
    fn rdp_renderer_runtime_series_requires_exact_ten_identical_reports() {
        let build = build_evidence();
        let observed = (0u8..10)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, rdp_renderer_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let evidence = validate_rdp_renderer_runtime_series(&build, &observed).unwrap();
        validate_rdp_renderer_runtime_series_evidence(&evidence).unwrap();

        let mut repeated = observed.clone();
        repeated[9].0 = repeated[0].0;
        repeated[9].1 = rdp_renderer_report(repeated[0].0, &build.identity);
        assert!(validate_rdp_renderer_runtime_series(&build, &repeated).is_err());

        let mut changed = observed;
        changed[9].1.prerequisite.renderer_publication_count += 1;
        changed[9].1.prerequisite.receipt_sha256 =
            recompute_rdp_renderer_runtime_prerequisite_receipt(&changed[9].1.prerequisite)
                .unwrap();
        assert!(validate_rdp_renderer_runtime_series(&build, &changed).is_err());

        let mut tampered = evidence;
        tampered.publication_trace_sha256 = "ff".repeat(32);
        assert!(validate_rdp_renderer_runtime_series_evidence(&tampered).is_err());
    }

    #[test]
    fn rsp_runtime_report_is_nonce_bound_deny_unknown_and_recomputes_receipt() {
        let identity = identity();
        let nonce = [0x67; 32];
        let report = rsp_report(nonce, &identity);
        let output = rsp_report_output(&report);
        assert_eq!(
            parse_generated_runner_rsp_runtime_report_v1(&output, nonce, &identity).unwrap(),
            report
        );
        assert!(
            parse_generated_runner_rsp_runtime_report_v1(&output, [0x68; 32], &identity).is_err()
        );
        let mut duplicate = output.clone();
        duplicate.extend_from_slice(&output);
        assert!(
            parse_generated_runner_rsp_runtime_report_v1(&duplicate, nonce, &identity).is_err()
        );

        let mut unknown = serde_json::to_value(&report).unwrap();
        unknown["prerequisite"]
            .as_object_mut()
            .unwrap()
            .insert("self_asserted_complete".to_owned(), serde_json::json!(true));
        let unknown = format!(
            "{}{}\n",
            GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1,
            serde_json::to_string(&unknown).unwrap()
        );
        assert!(
            parse_generated_runner_rsp_runtime_report_v1(unknown.as_bytes(), nonce, &identity)
                .is_err()
        );

        let mut no_publication = rsp_report(nonce, &identity);
        no_publication.prerequisite.interpreter_writeback_count = 0;
        no_publication.prerequisite.writeback_range_count = 0;
        no_publication.prerequisite.receipt_sha256 =
            recompute_rsp_runtime_prerequisite_receipt(&no_publication.prerequisite).unwrap();
        assert!(parse_generated_runner_rsp_runtime_report_v1(
            &rsp_report_output(&no_publication),
            nonce,
            &identity
        )
        .is_err());

        let mut tampered = rsp_report(nonce, &identity);
        tampered.prerequisite.writeback_trace_sha256 = "ee".repeat(32);
        assert!(parse_generated_runner_rsp_runtime_report_v1(
            &rsp_report_output(&tampered),
            nonce,
            &identity
        )
        .is_err());
    }

    #[test]
    fn rsp_runtime_series_requires_exact_ten_distinct_identical_reports() {
        let build = build_evidence();
        let observed = (0u8..10)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, rsp_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let evidence = validate_rsp_runtime_series(&build, &observed).unwrap();
        validate_rsp_runtime_series_evidence(&evidence).unwrap();

        let mut repeated = observed.clone();
        repeated[9].0 = repeated[0].0;
        repeated[9].1 = rsp_report(repeated[0].0, &build.identity);
        assert!(validate_rsp_runtime_series(&build, &repeated).is_err());

        let mut changed = observed;
        changed[9].1.prerequisite.interpreter_writeback_count += 1;
        changed[9].1.prerequisite.receipt_sha256 =
            recompute_rsp_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
        assert!(validate_rsp_runtime_series(&build, &changed).is_err());

        let mut tampered = evidence;
        tampered.writeback_trace_sha256 = "ff".repeat(32);
        assert!(validate_rsp_runtime_series_evidence(&tampered).is_err());
    }

    #[test]
    fn pi_runtime_report_requires_one_nonce_bound_deny_unknown_envelope() {
        let identity = identity();
        let nonce = [0x51; 32];
        let report = pi_report(nonce, &identity);
        let output = pi_report_output(&report);
        assert_eq!(
            parse_generated_runner_pi_runtime_report_v1(&output, nonce, &identity).unwrap(),
            report
        );
        assert!(
            parse_generated_runner_pi_runtime_report_v1(&output, [0x52; 32], &identity).is_err()
        );
        let mut duplicate = output.clone();
        duplicate.extend_from_slice(&output);
        assert!(parse_generated_runner_pi_runtime_report_v1(&duplicate, nonce, &identity).is_err());

        let mut unknown = serde_json::to_value(&report).unwrap();
        unknown["prerequisite"]
            .as_object_mut()
            .unwrap()
            .insert("self_asserted_complete".to_owned(), serde_json::json!(true));
        let unknown = format!(
            "{}{}\n",
            GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1,
            serde_json::to_string(&unknown).unwrap()
        );
        assert!(
            parse_generated_runner_pi_runtime_report_v1(unknown.as_bytes(), nonce, &identity)
                .is_err()
        );
    }

    #[test]
    fn pi_runtime_report_recomputes_receipt_and_requires_completed_read_dma() {
        let identity = identity();
        let nonce = [0x53; 32];

        let mut stale_epoch = pi_report(nonce, &identity);
        stale_epoch.prerequisite.trace_epoch_id = 0;
        stale_epoch.prerequisite.receipt_sha256 =
            recompute_pi_runtime_prerequisite_receipt(&stale_epoch.prerequisite).unwrap();
        assert!(parse_generated_runner_pi_runtime_report_v1(
            &pi_report_output(&stale_epoch),
            nonce,
            &identity
        )
        .is_err());

        let mut no_read_dma = pi_report(nonce, &identity);
        no_read_dma.prerequisite.pi_to_rdram_committed = 0;
        no_read_dma.prerequisite.receipt_sha256 =
            recompute_pi_runtime_prerequisite_receipt(&no_read_dma.prerequisite).unwrap();
        assert!(parse_generated_runner_pi_runtime_report_v1(
            &pi_report_output(&no_read_dma),
            nonce,
            &identity
        )
        .is_err());

        let mut tampered = pi_report(nonce, &identity);
        tampered.prerequisite.pi_transition_sha256 = "fe".repeat(32);
        assert!(parse_generated_runner_pi_runtime_report_v1(
            &pi_report_output(&tampered),
            nonce,
            &identity
        )
        .is_err());
    }

    #[test]
    fn pi_runtime_series_requires_ten_distinct_semantically_identical_reports() {
        let build = build_evidence();
        let observed = (0u8..10)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, pi_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let evidence = validate_pi_runtime_series(&build, &observed).unwrap();
        validate_pi_runtime_series_evidence(&evidence).unwrap();

        let mut repeated = observed.clone();
        repeated[9].0 = repeated[0].0;
        repeated[9].1 = pi_report(repeated[0].0, &build.identity);
        assert!(validate_pi_runtime_series(&build, &repeated).is_err());

        let mut changed = observed;
        changed[9].1.prerequisite.pi_notifications += 1;
        changed[9].1.prerequisite.receipt_sha256 =
            recompute_pi_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
        assert!(validate_pi_runtime_series(&build, &changed).is_err());

        let mut tampered = evidence;
        tampered.pi_transition_sha256 = "ff".repeat(32);
        assert!(validate_pi_runtime_series_evidence(&tampered).is_err());
    }

    #[test]
    fn si_runtime_report_requires_one_deny_unknown_envelope() {
        let identity = identity();
        let nonce = [0x31; 32];
        let report = si_report(nonce, &identity);
        let output = si_report_output(&report);
        assert_eq!(
            parse_generated_runner_si_runtime_report_v1(&output, nonce, &identity).unwrap(),
            report
        );
        assert!(parse_generated_runner_si_runtime_report_v1(
            b"diagnostic only\n",
            nonce,
            &identity
        )
        .is_err());
        let mut duplicate = output.clone();
        duplicate.extend_from_slice(&output);
        assert!(parse_generated_runner_si_runtime_report_v1(&duplicate, nonce, &identity).is_err());
        assert!(parse_generated_runner_si_runtime_report_v1(
            &output[..output.len() - 1],
            nonce,
            &identity
        )
        .is_err());
        let mut prefixed_noise = b"unexpected\n".to_vec();
        prefixed_noise.extend_from_slice(&output);
        assert!(
            parse_generated_runner_si_runtime_report_v1(&prefixed_noise, nonce, &identity).is_err()
        );
        let mut blank = output.clone();
        blank.push(b'\n');
        assert!(parse_generated_runner_si_runtime_report_v1(&blank, nonce, &identity).is_err());

        let mut unknown = serde_json::to_value(&report).unwrap();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("caller_claim".to_owned(), serde_json::Value::Bool(true));
        let unknown = format!(
            "{}{}\n",
            GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1,
            serde_json::to_string(&unknown).unwrap()
        );
        assert!(
            parse_generated_runner_si_runtime_report_v1(unknown.as_bytes(), nonce, &identity)
                .is_err()
        );
        let mut nested_unknown = serde_json::to_value(&report).unwrap();
        nested_unknown["prerequisite"]
            .as_object_mut()
            .unwrap()
            .insert(
                "self_asserted_complete".to_owned(),
                serde_json::Value::Bool(true),
            );
        let nested_unknown = format!(
            "{}{}\n",
            GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1,
            serde_json::to_string(&nested_unknown).unwrap()
        );
        assert!(parse_generated_runner_si_runtime_report_v1(
            nested_unknown.as_bytes(),
            nonce,
            &identity
        )
        .is_err());
    }

    #[test]
    fn si_runtime_report_binds_nonce_and_rejects_replay_under_another_challenge() {
        let identity = identity();
        let nonce = [0x41; 32];
        let output = si_report_output(&si_report(nonce, &identity));
        parse_generated_runner_si_runtime_report_v1(&output, nonce, &identity).unwrap();
        assert!(
            parse_generated_runner_si_runtime_report_v1(&output, [0x42; 32], &identity).is_err()
        );
    }

    #[test]
    fn sp_runtime_report_requires_one_deny_unknown_nonce_bound_envelope() {
        let identity = identity();
        let nonce = [0x61; 32];
        let report = sp_report(nonce, &identity);
        let output = sp_report_output(&report);
        assert_eq!(
            parse_generated_runner_sp_runtime_report_v1(&output, nonce, &identity).unwrap(),
            report
        );
        assert!(
            parse_generated_runner_sp_runtime_report_v1(&output, [0x62; 32], &identity).is_err()
        );
        let mut duplicate = output.clone();
        duplicate.extend_from_slice(&output);
        assert!(parse_generated_runner_sp_runtime_report_v1(&duplicate, nonce, &identity).is_err());
        assert!(parse_generated_runner_sp_runtime_report_v1(
            &output[..output.len() - 1],
            nonce,
            &identity
        )
        .is_err());
        let mut unknown = serde_json::to_value(&report).unwrap();
        unknown["prerequisite"].as_object_mut().unwrap().insert(
            "self_asserted_complete".to_owned(),
            serde_json::Value::Bool(true),
        );
        let unknown = format!(
            "{}{}\n",
            GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1,
            serde_json::to_string(&unknown).unwrap()
        );
        assert!(
            parse_generated_runner_sp_runtime_report_v1(unknown.as_bytes(), nonce, &identity)
                .is_err()
        );

        let mut stale_epoch = report.clone();
        stale_epoch.prerequisite.trace_epoch_id = 0;
        stale_epoch.prerequisite.receipt_sha256 =
            recompute_sp_runtime_prerequisite_receipt(&stale_epoch.prerequisite).unwrap();
        assert!(parse_generated_runner_sp_runtime_report_v1(
            &sp_report_output(&stale_epoch),
            nonce,
            &identity,
        )
        .is_err());

        let mut no_writeback = report.clone();
        no_writeback.prerequisite.sp_rsp_to_rdram_committed = 0;
        no_writeback.prerequisite.receipt_sha256 =
            recompute_sp_runtime_prerequisite_receipt(&no_writeback.prerequisite).unwrap();
        assert!(parse_generated_runner_sp_runtime_report_v1(
            &sp_report_output(&no_writeback),
            nonce,
            &identity,
        )
        .is_err());

        let mut bad_receipt = report;
        bad_receipt.prerequisite.receipt_sha256 = "ff".repeat(32);
        assert!(parse_generated_runner_sp_runtime_report_v1(
            &sp_report_output(&bad_receipt),
            nonce,
            &identity,
        )
        .is_err());
    }

    #[test]
    fn si_runtime_report_rejects_nonproduction_identity_and_inconsistent_prerequisite() {
        let identity = identity();
        let nonce = [0x51; 32];
        let mut nonproduction = identity.clone();
        nonproduction.production_aot = false;
        nonproduction.dev_interpreter = true;
        let output = si_report_output(&si_report(nonce, &nonproduction));
        assert!(
            parse_generated_runner_si_runtime_report_v1(&output, nonce, &nonproduction).is_err()
        );

        let mut inconsistent = si_report(nonce, &identity);
        inconsistent.prerequisite.si_committed = 0;
        let output = si_report_output(&inconsistent);
        assert!(parse_generated_runner_si_runtime_report_v1(&output, nonce, &identity).is_err());

        let mut wrong_model_receipt = si_report(nonce, &identity);
        wrong_model_receipt.prerequisite.program_model_sha256 = "ff".repeat(32);
        let output = si_report_output(&wrong_model_receipt);
        assert!(parse_generated_runner_si_runtime_report_v1(&output, nonce, &identity).is_err());
    }

    #[test]
    fn authority_integrity_binds_selected_binary_graph_sources_and_child_identity() {
        assert_eq!(BUILD_MAX_RSS_MIB, 4096);
        assert_eq!(BUILD_MIN_FREE_PERCENT, 40);
        assert_eq!(SELECTED_BUILD_CARGO_JOBS_V5, 2);
        let evidence = build_evidence();
        evidence.verify_integrity().unwrap();

        let mut wrong_jobs = evidence.clone();
        wrong_jobs.selected_build_cargo_jobs = 1;
        wrong_jobs.authority_sha256 = wrong_jobs.recompute_authority_sha256();
        assert!(wrong_jobs
            .verify_integrity()
            .unwrap_err()
            .to_string()
            .contains("requires exactly 2 selected-build Cargo jobs"));

        let mut downgraded_schema = evidence.clone();
        downgraded_schema.schema = VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V4;
        downgraded_schema.authority_sha256 = downgraded_schema.recompute_authority_sha256();
        assert!(downgraded_schema
            .verify_integrity()
            .unwrap_err()
            .to_string()
            .contains("unsupported verified generated-runner build schema"));

        for mutate in [
            |value: &mut GeneratedRunnerBuildEvidenceV1| {
                value.selected_binary_sha256 = "f1".repeat(32)
            },
            |value: &mut GeneratedRunnerBuildEvidenceV1| {
                value.private_build_inputs_sha256 = "f2".repeat(32)
            },
            |value: &mut GeneratedRunnerBuildEvidenceV1| {
                value.prepared_tree_descriptor_sha256 = "f3".repeat(32)
            },
            |value: &mut GeneratedRunnerBuildEvidenceV1| {
                value.prepared_tree_sha256 = "f4".repeat(32)
            },
            |value: &mut GeneratedRunnerBuildEvidenceV1| {
                value.producer_binary_sha256 = "f5".repeat(32)
            },
            |value: &mut GeneratedRunnerBuildEvidenceV1| {
                value.build_environment_sha256 = "f6".repeat(32)
            },
            |value: &mut GeneratedRunnerBuildEvidenceV1| value.build_max_rss_mib = 2048,
            |value: &mut GeneratedRunnerBuildEvidenceV1| value.build_min_free_percent = 39,
            |value: &mut GeneratedRunnerBuildEvidenceV1| {
                value.prepared_source_mode = PREPARED_SOURCE_MODE_CONSUMED_V1.to_owned()
            },
            |value: &mut GeneratedRunnerBuildEvidenceV1| {
                value.identity.prepared_materializer_source_sha256 = "f7".repeat(32)
            },
        ] {
            let mut changed = evidence.clone();
            mutate(&mut changed);
            assert!(changed.verify_integrity().is_err());
        }
    }

    #[test]
    fn si_runtime_series_requires_ten_distinct_nonce_bound_identical_reports() {
        let build = build_evidence();
        let observed = (0u8..10)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, si_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let evidence = validate_si_runtime_series(&build, &observed).unwrap();
        validate_si_runtime_series_evidence(&evidence).unwrap();

        let mut repeated = observed.clone();
        repeated[9] = repeated[0].clone();
        assert!(validate_si_runtime_series(&build, &repeated).is_err());

        let mut changed = observed.clone();
        changed[9].1.prerequisite.si_started = 2;
        changed[9].1.prerequisite.si_committed = 2;
        changed[9].1.prerequisite.receipt_sha256 =
            recompute_si_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
        assert!(validate_si_runtime_series(&build, &changed).is_err());

        for mutate in [
            |value: &mut GeneratedRunnerSiRuntimeSeriesEvidenceV1| value.run_count = 9,
            |value: &mut GeneratedRunnerSiRuntimeSeriesEvidenceV1| {
                value.selected_binary_sha256 = "ff".repeat(32)
            },
            |value: &mut GeneratedRunnerSiRuntimeSeriesEvidenceV1| {
                value.private_build_inputs_sha256 = "fe".repeat(32)
            },
            |value: &mut GeneratedRunnerSiRuntimeSeriesEvidenceV1| {
                value.program_model_sha256 = "fd".repeat(32)
            },
            |value: &mut GeneratedRunnerSiRuntimeSeriesEvidenceV1| {
                value.si_transition_sha256 = "fc".repeat(32)
            },
        ] {
            let mut changed = evidence.clone();
            mutate(&mut changed);
            assert!(validate_si_runtime_series_evidence(&changed).is_err());
        }
    }

    #[test]
    fn bootstrap_runtime_series_requires_ten_distinct_nonce_bound_identical_reports() {
        let build = build_evidence();
        let observed = (0u8..10)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, bootstrap_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let evidence = validate_bootstrap_runtime_series(&build, &observed).unwrap();
        validate_bootstrap_runtime_series_evidence(&evidence).unwrap();

        let mut repeated = observed.clone();
        repeated[9] = repeated[0].clone();
        assert!(validate_bootstrap_runtime_series(&build, &repeated).is_err());

        let mut changed = observed.clone();
        changed[9].1.prerequisite.journal_entry.before_sha256 = "cc".repeat(32);
        changed[9].1.prerequisite.journal_entry.journal_root_sha256 =
            recompute_bootstrap_canonical_journal_root(
                &changed[9].1.prerequisite.watched_ranges,
                &changed[9].1.prerequisite.journal_entry,
            )
            .unwrap();
        changed[9].1.prerequisite.receipt_sha256 =
            recompute_bootstrap_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
        assert!(validate_bootstrap_runtime_series(&build, &changed).is_err());

        for mutate in [
            |value: &mut GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1| value.run_count = 9,
            |value: &mut GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1| {
                value.selected_binary_sha256 = "ff".repeat(32)
            },
            |value: &mut GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1| {
                value.private_build_inputs_sha256 = "fe".repeat(32)
            },
            |value: &mut GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1| {
                value.program_model_sha256 = "fd".repeat(32)
            },
            |value: &mut GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1| {
                value.runtime_receipt_sha256 = "fc".repeat(32)
            },
        ] {
            let mut changed = evidence.clone();
            mutate(&mut changed);
            assert!(validate_bootstrap_runtime_series_evidence(&changed).is_err());
        }
    }

    #[test]
    fn sp_runtime_series_requires_ten_distinct_nonce_bound_identical_reports() {
        let build = build_evidence();
        let observed = (0u8..10)
            .map(|index| {
                let nonce = [index; 32];
                (nonce, sp_report(nonce, &build.identity))
            })
            .collect::<Vec<_>>();
        let evidence = validate_sp_runtime_series(&build, &observed).unwrap();
        validate_sp_runtime_series_evidence(&evidence).unwrap();

        let mut repeated = observed.clone();
        repeated[9] = repeated[0].clone();
        assert!(validate_sp_runtime_series(&build, &repeated).is_err());

        let mut changed = observed.clone();
        changed[9].1.prerequisite.sp_started = 3;
        changed[9].1.prerequisite.sp_committed = 3;
        changed[9].1.prerequisite.receipt_sha256 =
            recompute_sp_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
        assert!(validate_sp_runtime_series(&build, &changed).is_err());

        for mutate in [
            |value: &mut GeneratedRunnerSpRuntimeSeriesEvidenceV1| value.run_count = 9,
            |value: &mut GeneratedRunnerSpRuntimeSeriesEvidenceV1| {
                value.selected_binary_sha256 = "ff".repeat(32)
            },
            |value: &mut GeneratedRunnerSpRuntimeSeriesEvidenceV1| {
                value.private_build_inputs_sha256 = "fe".repeat(32)
            },
            |value: &mut GeneratedRunnerSpRuntimeSeriesEvidenceV1| {
                value.program_model_sha256 = "fd".repeat(32)
            },
            |value: &mut GeneratedRunnerSpRuntimeSeriesEvidenceV1| {
                value.sp_transition_sha256 = "fc".repeat(32)
            },
        ] {
            let mut changed = evidence.clone();
            mutate(&mut changed);
            assert!(validate_sp_runtime_series_evidence(&changed).is_err());
        }
    }

    #[test]
    fn writer_audit_bundle_binds_bitmap_build_channels_and_nested_authorities() {
        let evidence = writer_audit_bundle_evidence();
        validate_writer_audit_bundle_evidence(&evidence).unwrap();

        let mut partial = evidence.clone();
        partial.completed_channels = WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1;
        partial.cpu = None;
        partial.host_abi = None;
        partial.pi = None;
        partial.rdp_renderer = None;
        partial.rsp = None;
        partial.si = None;
        partial.sp = None;
        partial.authority_sha256 = writer_audit_bundle_authority_sha256(&partial).unwrap();
        validate_writer_audit_bundle_evidence(&partial).unwrap();

        let mut bitmap_mismatch = evidence.clone();
        bitmap_mismatch.completed_channels &= !WRITER_AUDIT_PI_COMPLETED_V1;
        bitmap_mismatch.authority_sha256 =
            writer_audit_bundle_authority_sha256(&bitmap_mismatch).unwrap();
        assert!(validate_writer_audit_bundle_evidence(&bitmap_mismatch).is_err());

        let mut nested_tamper = evidence.clone();
        nested_tamper
            .bootstrap
            .as_mut()
            .unwrap()
            .runtime_receipt_sha256 = "ee".repeat(32);
        assert!(validate_writer_audit_bundle_evidence(&nested_tamper).is_err());

        let mut nested_pi_tamper = evidence.clone();
        nested_pi_tamper.pi.as_mut().unwrap().runtime_receipt_sha256 = "ef".repeat(32);
        assert!(validate_writer_audit_bundle_evidence(&nested_pi_tamper).is_err());

        let mut nested_host_abi_tamper = evidence.clone();
        nested_host_abi_tamper
            .host_abi
            .as_mut()
            .unwrap()
            .runtime_receipt_sha256 = "e0".repeat(32);
        assert!(validate_writer_audit_bundle_evidence(&nested_host_abi_tamper).is_err());

        let mut nested_rdp_renderer_tamper = evidence.clone();
        nested_rdp_renderer_tamper
            .rdp_renderer
            .as_mut()
            .unwrap()
            .runtime_receipt_sha256 = "e1".repeat(32);
        assert!(validate_writer_audit_bundle_evidence(&nested_rdp_renderer_tamper).is_err());

        let mut nested_rsp_tamper = evidence.clone();
        nested_rsp_tamper
            .rsp
            .as_mut()
            .unwrap()
            .runtime_receipt_sha256 = "e2".repeat(32);
        assert!(validate_writer_audit_bundle_evidence(&nested_rsp_tamper).is_err());

        let mut cross_build = evidence.clone();
        let si = cross_build.si.as_mut().unwrap();
        si.build_authority_sha256 = "ed".repeat(32);
        si.authority_sha256 = si_runtime_series_authority_sha256(si).unwrap();
        assert!(validate_writer_audit_bundle_evidence(&cross_build).is_err());

        let mut cross_program_model = evidence.clone();
        let sp = cross_program_model.sp.as_mut().unwrap();
        sp.program_model_sha256 = "ec".repeat(32);
        sp.authority_sha256 = sp_runtime_series_authority_sha256(sp).unwrap();
        assert!(validate_writer_audit_bundle_evidence(&cross_program_model).is_err());

        let mut authority_tamper = evidence;
        authority_tamper.authority_sha256 = "eb".repeat(32);
        assert!(validate_writer_audit_bundle_evidence(&authority_tamper).is_err());
    }

    #[test]
    fn compiler_artifact_selector_rejects_absent_and_duplicate_roots() {
        assert!(select_compiler_artifact(b"{}").is_err());
        let line = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": PACKAGE, "kind": ["bin"] },
            "executable": "/does/not/matter"
        })
        .to_string();
        let duplicate = format!("{line}\n{line}\n");
        assert!(select_compiler_artifact(duplicate.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("multiple"));
    }

    #[test]
    fn cargo_progress_counts_completed_shard_libraries_without_content() {
        let shard = PREPARED_PACKAGES[0].replace('-', "_");
        let build_script = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": "build_script_build", "kind": ["custom-build"] },
        });
        let shard_library = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": shard, "kind": ["lib"] },
        });
        let root_binary = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": PACKAGE, "kind": ["bin"] },
        });
        let stream = format!("{build_script}\n{shard_library}\n{root_binary}\n");
        assert_eq!(
            cargo_build_progress(stream.as_bytes()),
            format!(
                "compiler_artifacts=3 completed_shards=1/{} root_binary=1",
                PREPARED_PACKAGES.len()
            )
        );
        assert_eq!(
            cargo_build_progress(b"not-json\n"),
            format!(
                "compiler_artifacts=0 completed_shards=0/{} root_binary=0",
                PREPARED_PACKAGES.len()
            )
        );
    }

    #[test]
    fn selected_build_command_binds_two_jobs_and_the_exact_process_group_guard() {
        let workspace = repository_workspace().unwrap();
        let guard = workspace.join("scripts/memory-guard.zsh");
        let manifest = workspace.join("examples/wm2000-block-boot/Cargo.toml");
        let staged_boot_context =
            PathBuf::from("/private/tmp/fn64-command-policy/private-inputs/boot-context.json");
        let inputs = Wm2000GeneratedRunnerBuildInputsV1 {
            rom: PathBuf::from("/private/tmp/fn64-command-policy.rom"),
            boot_context: staged_boot_context.clone(),
            executable_image_groups: vec![Wm2000ExecutableImageGroupV1 {
                environment_name: "FN64_EXECUTABLE_IMAGE_TEST".to_owned(),
                captures: vec![
                    PathBuf::from("/private/tmp/capture-a"),
                    PathBuf::from("/private/tmp/capture-b"),
                    PathBuf::from("/private/tmp/capture-c"),
                ],
            }],
            max_build_seconds: 60 * 60,
        };
        let prepared = PreparedTreeMeasurementV3 {
            root: PathBuf::from("/private/tmp/fn64-command-policy/prepared"),
            normalized_rom_sha256: "11".repeat(32),
            manifest_sha256: "12".repeat(32),
            tree_sha256: "13".repeat(32),
            descriptor_sha256: "14".repeat(32),
            claims: synthetic_claims(),
        };
        let producer = ProducerBuildMeasurementV3 {
            manifest_sha256: "21".repeat(32),
            lock_sha256: "22".repeat(32),
            cargo_graph_sha256: "23".repeat(32),
            cargo_source_sha256: "24".repeat(32),
            binary_sha256: "25".repeat(32),
            binary: PathBuf::from("/private/tmp/fn64-command-policy/producer"),
        };
        let environment = BuildEnvironmentV3 {
            path: "/usr/bin:/bin".into(),
            home: PathBuf::from("/private/tmp/fn64-command-policy/home"),
            cargo_home: PathBuf::from("/private/tmp/fn64-command-policy/cargo-home"),
            temp: PathBuf::from("/private/tmp/fn64-command-policy/temp"),
            rustc: PathBuf::from("/absolute/verifier-owned/rustc"),
            identity_sha256: "31".repeat(32),
            rustc_sha256: "32".repeat(32),
            cargo_config_sha256: "33".repeat(32),
        };
        let command = guarded_build_command(
            &guard,
            Path::new("/absolute/verifier-owned/cargo"),
            &manifest,
            &inputs,
            &prepared,
            &producer,
            PREPARED_SOURCE_MODE_INACTIVE_V1,
            &environment,
            Path::new("/private/tmp/fn64-command-policy"),
        )
        .unwrap();
        assert_eq!(command.get_program(), guard.as_os_str());
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            &args[..4],
            &["/absolute/verifier-owned/cargo", "build", "-j2", "--frozen"]
        );
        let environments = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(environments["CARGO_BUILD_JOBS"].as_deref(), Some("2"));
        assert_eq!(
            environments["FN64_BOOT_CONTEXT"].as_deref(),
            staged_boot_context.to_str()
        );
        assert_eq!(
            environments["ROM"].as_deref(),
            Some("/private/tmp/fn64-command-policy.rom")
        );
        assert_eq!(
            environments["FN64_EXECUTABLE_IMAGE_TEST"].as_deref(),
            std::env::join_paths(&inputs.executable_image_groups[0].captures)
                .unwrap()
                .to_str()
        );
        assert_eq!(
            environments["FN64_GUARD_MAX_RSS_MIB"].as_deref(),
            Some("4096")
        );
        assert_eq!(
            environments["FN64_GUARD_MIN_FREE_PERCENT"].as_deref(),
            Some("40")
        );
        assert_eq!(
            environments["FN64_GUARD_MAX_SECONDS"].as_deref(),
            Some("3600")
        );
    }

    #[test]
    fn writer_runtime_commands_have_only_exact_retained_private_inputs() {
        let inputs = Wm2000GeneratedRunnerBuildInputsV1 {
            rom: PathBuf::from("/private/tmp/staged/rom"),
            boot_context: PathBuf::from("/private/tmp/staged/boot-context"),
            executable_image_groups: vec![Wm2000ExecutableImageGroupV1 {
                environment_name: "FN64_EXECUTABLE_IMAGE_TEST".to_owned(),
                captures: vec![
                    PathBuf::from("/private/tmp/staged/capture-a"),
                    PathBuf::from("/private/tmp/staged/capture-b"),
                    PathBuf::from("/private/tmp/staged/capture-c"),
                ],
            }],
            max_build_seconds: 60 * 60,
        };
        let nonce = [0x5a; 32];
        let nonce_hex = hex(&nonce);
        for protocol in [
            WriterRuntimeAuditProtocol::Bootstrap,
            WriterRuntimeAuditProtocol::Cpu,
            WriterRuntimeAuditProtocol::HostAbi,
            WriterRuntimeAuditProtocol::Pi,
            WriterRuntimeAuditProtocol::RdpRenderer,
            WriterRuntimeAuditProtocol::Rsp,
            WriterRuntimeAuditProtocol::Si,
            WriterRuntimeAuditProtocol::Sp,
        ] {
            let mut command = Command::new("/private/tmp/staged/selected-runner");
            configure_writer_runtime_command(&mut command, &inputs, nonce, protocol).unwrap();
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                [std::ffi::OsStr::new(protocol.argument())]
            );
            let environments = command
                .get_envs()
                .map(|(name, value)| {
                    (
                        name.to_string_lossy().into_owned(),
                        value.map(|value| value.to_string_lossy().into_owned()),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(environments.len(), 5);
            assert_eq!(environments["ROM"].as_deref(), inputs.rom.to_str());
            assert_eq!(
                environments["FN64_BOOT_CONTEXT"].as_deref(),
                inputs.boot_context.to_str()
            );
            assert_eq!(
                environments[protocol.nonce_environment()].as_deref(),
                Some(nonce_hex.as_str())
            );
            for nonce_environment in [
                GENERATED_RUNNER_BOOTSTRAP_RUNTIME_NONCE_ENV_V1,
                GENERATED_RUNNER_CPU_RUNTIME_NONCE_ENV_V1,
                GENERATED_RUNNER_HOST_ABI_RUNTIME_NONCE_ENV_V1,
                GENERATED_RUNNER_PI_RUNTIME_NONCE_ENV_V1,
                GENERATED_RUNNER_RDP_RENDERER_RUNTIME_NONCE_ENV_V1,
                GENERATED_RUNNER_RSP_RUNTIME_NONCE_ENV_V1,
                GENERATED_RUNNER_SI_RUNTIME_NONCE_ENV_V1,
                GENERATED_RUNNER_SP_RUNTIME_NONCE_ENV_V1,
            ] {
                assert_eq!(
                    environments.contains_key(nonce_environment),
                    nonce_environment == protocol.nonce_environment()
                );
            }
            assert_eq!(
                environments["FN64_EXECUTABLE_IMAGE_GROUPS"].as_deref(),
                Some("FN64_EXECUTABLE_IMAGE_TEST")
            );
            assert_eq!(
                environments["FN64_EXECUTABLE_IMAGE_TEST"].as_deref(),
                std::env::join_paths(&inputs.executable_image_groups[0].captures)
                    .unwrap()
                    .to_str()
            );
        }
    }

    #[test]
    fn private_input_binding_retains_exact_boot_context_path_and_bytes() {
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).unwrap();
        let scratch = ScratchDirectory::create(&nonce).unwrap();
        let rom = scratch.path().join("game.rom");
        let boot_context = scratch.path().join("boot-context.json");
        let alternate_boot_context = scratch.path().join("alternate-boot-context.json");
        fs::write(&rom, b"synthetic-rom").unwrap();
        fs::write(&boot_context, b"synthetic-boot-context").unwrap();
        fs::write(&alternate_boot_context, b"synthetic-boot-context").unwrap();
        let captures = (0..3)
            .map(|index| {
                let path = scratch.path().join(format!("capture-{index}"));
                fs::write(&path, [u8::try_from(index).unwrap()]).unwrap();
                path
            })
            .collect::<Vec<_>>();
        let mut inputs = Wm2000GeneratedRunnerBuildInputsV1 {
            rom,
            boot_context,
            executable_image_groups: vec![Wm2000ExecutableImageGroupV1 {
                environment_name: "FN64_EXECUTABLE_IMAGE_TEST".to_owned(),
                captures,
            }],
            max_build_seconds: 60 * 60,
        };
        validate_inputs(&inputs).unwrap();
        let original = private_inputs_sha256(&inputs).unwrap();
        inputs.boot_context = alternate_boot_context;
        assert_ne!(private_inputs_sha256(&inputs).unwrap(), original);
        let staged = stage_private_inputs(&inputs, scratch.path()).unwrap();
        let staged_digest = private_inputs_sha256(&staged).unwrap();
        assert!(staged
            .rom
            .starts_with(scratch.path().join("private-inputs")));
        assert!(staged
            .boot_context
            .starts_with(scratch.path().join("private-inputs")));
        fs::write(&inputs.boot_context, b"changed-boot-context").unwrap();
        assert_eq!(private_inputs_sha256(&staged).unwrap(), staged_digest);
    }

    #[test]
    fn memory_guard_policy_requires_process_group_launch_and_termination() {
        validate_memory_guard_source(MEMORY_GUARD_SOURCE).unwrap();
        let source = std::str::from_utf8(MEMORY_GUARD_SOURCE).unwrap();
        for required in ["setsid", "terminate_group"] {
            let missing = source.replace(required, &"_".repeat(required.len()));
            assert!(validate_memory_guard_source(missing.as_bytes()).is_err());
        }
    }
}
