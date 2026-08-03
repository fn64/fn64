#![allow(clippy::module_inception)]
use super::*;

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
pub(super) struct PreparedSourceClaimsV3 {
    generator_source_sha256: String,
    discovery_source_sha256: String,
    emitter_source_sha256: String,
    runtime_source_sha256: String,
    materializer_source_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProducerBuildMeasurementV3 {
    manifest_sha256: String,
    lock_sha256: String,
    cargo_graph_sha256: String,
    cargo_source_sha256: String,
    binary_sha256: String,
    binary: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PreparedTreeMeasurementV3 {
    root: PathBuf,
    normalized_rom_sha256: String,
    manifest_sha256: String,
    tree_sha256: String,
    descriptor_sha256: String,
    claims: PreparedSourceClaimsV3,
}

#[derive(Clone, Debug)]
pub(super) struct BuildEnvironmentV3 {
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
