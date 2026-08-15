//! Content-safe black-box characterization for privately supplied audio ucode.
//!
//! The request names exact local inputs, but the report contains only public
//! parameters, ranges, counters, and SHA-256 identities. No input or guest
//! memory byte is serialized. The RSP DMA journal is diagnostic-only and is
//! excluded from architectural snapshots and commit authority.

use std::fs;
use std::path::PathBuf;

use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
use fn64_runtime::{
    OsTaskHeader, RdramAddr, RdramView, RdramViewMut, RspMemAddr, RspMemory, RspMemoryBank,
    RspMemoryError, M_AUDTASK,
};
use serde::{Deserialize, Serialize};

use crate::compact_abi::execute_compact_audio_lane;
use crate::hle::{AudioHleCatalog, AudioHleCatalogEntry};
use crate::hle_lle::{run_speculative_audio_lle, SpeculativeAudioLleError};
use crate::hle_outcome::{
    AudioHleFamily, AudioTaskTerminalReason, DeferredDpcSubmission, DpcSubmissionSource,
    Sha256Digest,
};
use crate::hle_rspboot::{execute_audio_rspboot_to_entry, AudioRspbootError, AudioRspbootInput};
use crate::hle_snapshot::AudioHleSnapshotError;
use crate::rsp::runtime::{
    RspDmaDirection, RspDmaJournalEntry, RspDpCommandSource, RspMachine, RspMachineState,
};

pub const REQUEST_SCHEMA: &str = "fn64.audio-abi-characterization-request.v2";
pub const REPORT_SCHEMA: &str = "fn64.audio-abi-characterization-report.v2";
pub const COMPACT_VERIFICATION_REPORT_SCHEMA: &str = "fn64.audio-compact-verification-report.v1";
pub const FIXTURE_REVISION: u32 = 2;
const TASK_DMEM_OFFSET: u16 = 0x0fc0;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterizationRequest {
    pub schema: String,
    pub fixture_revision: u32,
    pub microcode: PrivateMicrocodePaths,
    pub layout: CharacterizationLayout,
    pub cases: Vec<CharacterizationCase>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateMicrocodePaths {
    pub rspboot_path: PathBuf,
    pub rspboot_sha256: String,
    pub text_path: PathBuf,
    pub text_sha256: String,
    pub data_path: PathBuf,
    pub data_sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterizationLayout {
    pub task_address: u32,
    pub rspboot_address: u32,
    pub text_address: u32,
    pub data_address: u32,
    pub command_address: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterizationCase {
    pub id: String,
    pub parameters: ExperimentParameters,
    pub sentinels: Vec<SentinelRange>,
    /// Independently executed control/probe lanes. Every trial starts from
    /// the same pre-rspboot image; only its packet phases differ.
    pub trials: Vec<CharacterizationTrial>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterizationTrial {
    pub phases: Vec<CharacterizationPhase>,
}

/// Public axes attached to a case. Packet words remain explicit, so this
/// schema records questions without guessing family-specific packing.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExperimentParameters {
    Address {
        opcode: u8,
        selector: u8,
        address: u32,
        alignment: u8,
    },
    Selector {
        opcode: u8,
        selector: u8,
    },
    Count {
        opcode: u8,
        count: u32,
    },
    DmemMove {
        input_dmem: u16,
        output_dmem: u16,
        count: u16,
        overlap: DmemMoveOverlap,
    },
    Aux {
        flags: u8,
        input_dmem: u16,
        output_dmem: u16,
        aux_a: u16,
        aux_c: u16,
        aux_e: u16,
    },
    Reserved {
        opcode: u8,
        word0_reserved_mask: u32,
        word1_reserved_mask: u32,
    },
    Persistence {
        state: PersistenceState,
        task_count: u16,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DmemMoveOverlap {
    None,
    Forward,
    Backward,
    ExactAlias,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistenceState {
    Segment,
    Loop,
    Codebook,
    Buffer,
    ScalarVector,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SentinelRange {
    pub start: u32,
    pub byte_len: u32,
    pub pattern_seed: u8,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterizationPhase {
    pub packets: Vec<PublicCommandPacket>,
}

/// Exact public 64-bit task packet. Interpretation belongs to the private
/// image under observation; the harness does not infer a family from it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicCommandPacket {
    pub word0: u32,
    pub word1: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct CharacterizationReport {
    pub schema: &'static str,
    pub fixture_revision: u32,
    pub request_sha256: String,
    pub verified_inputs: VerifiedInputIdentities,
    pub cases: Vec<CaseReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct VerifiedInputIdentities {
    pub rspboot_bytes: usize,
    pub rspboot_sha256: String,
    pub text_bytes: usize,
    pub text_sha256: String,
    pub data_bytes: usize,
    pub data_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CaseReport {
    pub case_sha256: String,
    pub axis: &'static str,
    pub common_baseline_sha256: String,
    pub trials: Vec<TrialReport>,
    pub comparisons: Vec<TrialComparison>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrialReport {
    pub trial_sha256: String,
    pub phases: Vec<PhaseReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrialComparison {
    pub reference_trial: usize,
    pub candidate_trial: usize,
    pub phases: Vec<PhaseComparison>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PhaseComparison {
    pub phase: usize,
    pub equivalent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_divergence: Option<FirstDivergence>,
    pub rdram_differences: Vec<DifferenceRange>,
    pub dmem_differences: Vec<DifferenceRange>,
    pub imem_differences: Vec<DifferenceRange>,
    pub reference_entry_snapshot_sha256: String,
    pub candidate_entry_snapshot_sha256: String,
    pub reference_architecture_sha256: String,
    pub candidate_architecture_sha256: String,
    pub reference_deferred_dpc_sha256: String,
    pub candidate_deferred_dpc_sha256: String,
}

/// Stable content-free location of the first exact result divergence.
///
/// Guest bytes are never emitted. `index` selects a journal/patch/register
/// entry and `address` selects a logical RDRAM/RSP-memory byte when that
/// domain has one.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FirstDivergence {
    pub domain: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<u32>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DifferenceRange {
    pub start: u32,
    pub byte_len: u32,
    pub reference_sha256: String,
    pub candidate_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PhaseReport {
    pub phase: usize,
    pub phase_sha256: String,
    pub command_count: usize,
    pub entry_snapshot_sha256: String,
    pub captured_imem_sha256: String,
    pub captured_data_bytes: u32,
    pub captured_data_sha256: String,
    pub terminal: &'static str,
    pub rspboot_steps: u64,
    pub ucode_steps: u64,
    pub rspboot_dma: Vec<DmaObservation>,
    pub ucode_dma: Vec<DmaObservation>,
    pub rspboot_imem_replacements: Vec<ImemReplacementObservation>,
    pub deferred_dpc_submissions: Vec<DeferredDpcObservation>,
    pub rspboot_mutations: Vec<MutationObservation>,
    pub ucode_mutations: Vec<MutationObservation>,
    pub selected_digests: SelectedDigests,
}

#[derive(Clone, Debug, Serialize)]
pub struct DmaObservation {
    pub direction: &'static str,
    pub effective_dram_address: u32,
    pub sp_mem_address: u32,
    pub raw_length_descriptor: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImemReplacementObservation {
    pub generation: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeferredDpcObservation {
    pub source: &'static str,
    pub start: u32,
    pub end: u32,
    pub command_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MutationObservation {
    pub start: u32,
    pub byte_len: u32,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelectedDigests {
    pub native_rdram_sha256: String,
    pub dmem_sha256: String,
    pub imem_sha256: String,
    pub sentinels: Vec<SentinelDigest>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SentinelDigest {
    pub sentinel: usize,
    pub sha256: String,
}

#[derive(Clone)]
struct LoadedInputs {
    rspboot: Vec<u8>,
    text: Vec<u8>,
    data: Vec<u8>,
}

struct TrialExecution {
    report: TrialReport,
    phases: Vec<PhaseExecution>,
}

struct PhaseExecution {
    entry_snapshot: Sha256Digest,
    rspboot_rdram_patches: crate::hle_outcome::CanonicalRdramPatches,
    rspboot_dma_journal: Vec<RspDmaJournalEntry>,
    rspboot_imem_replacements: Vec<crate::hle_effects::AudioImemReplacement>,
    terminal: AudioTaskTerminalReason,
    rspboot_steps: u64,
    ucode_steps: u64,
    rdram_patches: crate::hle_outcome::CanonicalRdramPatches,
    rdram_storage: Vec<u8>,
    dmem: [u8; crate::hle_outcome::RSP_BANK_BYTES],
    imem: [u8; crate::hle_outcome::RSP_BANK_BYTES],
    imem_generation: u64,
    machine_state: RspMachineState,
    pc_low12: u32,
    dma_journal: Vec<RspDmaJournalEntry>,
    deferred_dpc_submissions: Vec<DeferredDpcSubmission>,
    imem_replacements: Vec<crate::hle_effects::AudioImemReplacement>,
}

pub fn characterize_request(
    request: CharacterizationRequest,
) -> Result<CharacterizationReport, String> {
    validate_request_header(&request)?;
    let loaded = load_inputs(&request)?;
    characterize_loaded(request, loaded)
}

fn load_inputs(request: &CharacterizationRequest) -> Result<LoadedInputs, String> {
    Ok(LoadedInputs {
        rspboot: read_exact_input(
            "rspboot",
            &request.microcode.rspboot_path,
            &request.microcode.rspboot_sha256,
        )?,
        text: read_exact_input(
            "text",
            &request.microcode.text_path,
            &request.microcode.text_sha256,
        )?,
        data: read_exact_input(
            "data",
            &request.microcode.data_path,
            &request.microcode.data_sha256,
        )?,
    })
}

pub fn canonical_report_json(report: &CharacterizationReport) -> Result<String, String> {
    serde_json::to_string(report).map_err(|error| format!("serialize report: {error}"))
}

#[derive(Clone, Debug, Serialize)]
pub struct CompactVerificationReport {
    pub schema: &'static str,
    pub fixture_revision: u32,
    pub request_sha256: String,
    pub verified_inputs: VerifiedInputIdentities,
    pub phases: Vec<CompactPhaseVerification>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CompactPhaseVerification {
    pub case_sha256: String,
    pub trial: usize,
    pub command_count: usize,
    pub decoded_commands: usize,
    pub dmem_equivalent: bool,
    pub first_dmem_difference: Option<u16>,
    pub dmem_differences: Vec<DifferenceRange>,
    pub rdram_patches_equivalent: bool,
}

pub fn verify_compact_request(
    request: CharacterizationRequest,
) -> Result<CompactVerificationReport, String> {
    validate_request_header(&request)?;
    if request
        .cases
        .iter()
        .flat_map(|case| &case.trials)
        .any(|trial| trial.phases.len() != 1)
    {
        return Err("compact verification currently requires independent one-phase trials".into());
    }
    let loaded = load_inputs(&request)?;
    verify_compact_loaded(request, loaded)
}

fn verify_compact_loaded(
    request: CharacterizationRequest,
    loaded: LoadedInputs,
) -> Result<CompactVerificationReport, String> {
    validate_loaded_geometry(request.layout, &loaded, &request.cases)?;
    let verified_inputs = VerifiedInputIdentities {
        rspboot_bytes: loaded.rspboot.len(),
        rspboot_sha256: hex_digest(Sha256Digest::hash(&loaded.rspboot)),
        text_bytes: loaded.text.len(),
        text_sha256: hex_digest(Sha256Digest::hash(&loaded.text)),
        data_bytes: loaded.data.len(),
        data_sha256: hex_digest(Sha256Digest::hash(&loaded.data)),
    };
    let request_sha256 = hex_digest(request_digest(&request, &loaded));
    let mut phases = Vec::new();
    for case in &request.cases {
        let _content_safe_diagnostics = crate::rsp::recomp::ContentSafeDiagnosticsGuard::enter();
        let baseline = build_baseline_rdram(request.layout, &loaded, &case.sentinels);
        for (trial_index, trial) in case.trials.iter().enumerate() {
            phases.push(verify_compact_phase(
                request.layout,
                &loaded,
                &baseline,
                &case_digest(case),
                trial_index,
                &trial.phases[0],
            )?);
        }
    }
    Ok(CompactVerificationReport {
        schema: COMPACT_VERIFICATION_REPORT_SCHEMA,
        fixture_revision: FIXTURE_REVISION,
        request_sha256,
        verified_inputs,
        phases,
    })
}

fn verify_compact_phase(
    layout: CharacterizationLayout,
    loaded: &LoadedInputs,
    baseline: &[u8],
    case_sha256: &Sha256Digest,
    trial: usize,
    phase: &CharacterizationPhase,
) -> Result<CompactPhaseVerification, String> {
    let command_bytes = encode_packets(&phase.packets);
    let mut rdram = baseline.to_vec();
    install_logical(&mut rdram, layout.command_address, &command_bytes);
    let header = task_header(layout, loaded, command_bytes.len())?;
    install_logical(&mut rdram, layout.task_address, &encode_header(header));
    let mut memory = RspMemory::new();
    memory
        .write_bytes(
            RspMemAddr::from_parts(RspMemoryBank::Imem, 0),
            &loaded.rspboot,
        )
        .map_err(|error| content_safe_rsp_memory_error("install rspboot IMEM", error))?;
    memory
        .write_bytes(
            RspMemAddr::from_parts(RspMemoryBank::Dmem, TASK_DMEM_OFFSET),
            &encode_header(header),
        )
        .map_err(|error| content_safe_rsp_memory_error("install task DMEM", error))?;
    let mut machine_backing = [0; 8];
    let machine_state = RspMachine::new(&mut machine_backing).snapshot_state();
    let input = AudioRspbootInput::new(
        RdramAddr::from_offset(layout.task_address),
        header,
        rdram,
        memory.snapshot(),
        0,
        machine_state,
    )
    .map_err(|error| content_safe_rspboot_error("rspboot input", error))?;
    let entry = execute_audio_rspboot_to_entry(input)
        .map_err(|error| content_safe_rspboot_error("rspboot execution", error))?
        .into_entry();
    let catalog_entries = [AudioHleCatalogEntry {
        identity: entry.identity(),
        family: AudioHleFamily::CompactAbi,
        implementation_revision: 1,
    }];
    let admission = AudioHleCatalog::new(&catalog_entries)
        .expect("one-entry compact verification catalog is unique")
        .admit(entry.identity())
        .expect("one-entry compact verification catalog contains the entry identity");
    let admitted = entry
        .admit_hle(admission)
        .map_err(|error| content_safe_snapshot_error(error).to_owned())?;
    let hle = execute_compact_audio_lane(admitted.fork_hle_lane())
        .map_err(|error| format!("compact HLE execution failed: {error}"))?;
    let lle = run_speculative_audio_lle(admitted.fork_lle_lane().into_lle_parts())
        .map_err(content_safe_speculative_lle_error)?;
    let lle_dmem = lle.rsp_memory().bank(RspMemoryBank::Dmem);
    let first_dmem_difference = hle
        .dmem()
        .image()
        .iter()
        .zip(lle_dmem)
        .position(|(hle, lle)| hle != lle)
        .map(|offset| u16::try_from(offset).expect("DMEM offset fits u16"));
    let dmem_differences = difference_ranges(0, hle.dmem().image(), lle_dmem);
    Ok(CompactPhaseVerification {
        case_sha256: hex_digest(*case_sha256),
        trial,
        command_count: phase.packets.len(),
        decoded_commands: hle.decoded_commands(),
        dmem_equivalent: first_dmem_difference.is_none(),
        first_dmem_difference,
        dmem_differences,
        rdram_patches_equivalent: hle.rdram_patches() == lle.rdram_patches(),
    })
}

fn validate_request_header(request: &CharacterizationRequest) -> Result<(), String> {
    if request.schema != REQUEST_SCHEMA {
        return Err(format!("request schema must be {REQUEST_SCHEMA}"));
    }
    if request.fixture_revision != FIXTURE_REVISION {
        return Err(format!("fixture_revision must be {FIXTURE_REVISION}"));
    }
    if request.cases.is_empty() {
        return Err("at least one characterization case is required".into());
    }
    for (case_index, case) in request.cases.iter().enumerate() {
        if case.id.is_empty()
            || !case
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            || case.trials.len() < 2
        {
            return Err(
                "every case requires a nonempty id and at least two same-baseline trials".into(),
            );
        }
        if case.trials.iter().any(|trial| {
            trial.phases.is_empty() || trial.phases.iter().any(|phase| phase.packets.is_empty())
        }) {
            return Err(format!(
                "characterization case {case_index} contains an empty trial or command phase"
            ));
        }
        if request.cases[..case_index]
            .iter()
            .any(|prior| prior.id == case.id)
        {
            return Err(format!(
                "duplicate characterization case id at index {case_index}"
            ));
        }
        let phase_count = case.trials[0].phases.len();
        if case
            .trials
            .iter()
            .any(|trial| trial.phases.len() != phase_count)
        {
            return Err(format!(
                "characterization case {case_index} trials must have equal phase counts"
            ));
        }
        for phase in 0..phase_count {
            let command_count = case.trials[0].phases[phase].packets.len();
            if case
                .trials
                .iter()
                .any(|trial| trial.phases[phase].packets.len() != command_count)
            {
                return Err(format!(
                    "characterization case {case_index} phase {phase} trials must have equal packet counts"
                ));
            }
        }
        if let ExperimentParameters::Persistence { task_count, .. } = case.parameters {
            if usize::from(task_count) != phase_count || task_count < 2 {
                return Err(format!(
                    "persistence case {case_index} task_count must equal every trial's at least two phases"
                ));
            }
        } else if phase_count != 1 {
            return Err(format!(
                "non-persistence case {case_index} trials must contain exactly one phase"
            ));
        }
    }
    Ok(())
}

fn read_exact_input(label: &str, path: &PathBuf, expected: &str) -> Result<Vec<u8>, String> {
    let expected = parse_digest(expected).map_err(|error| format!("{label} SHA-256: {error}"))?;
    let bytes = fs::read(path).map_err(|error| format!("read {label} input: {error}"))?;
    let actual = Sha256Digest::hash(&bytes);
    if actual != expected {
        return Err(format!(
            "{label} SHA-256 mismatch: expected {}, observed {}",
            hex_digest(expected),
            hex_digest(actual)
        ));
    }
    Ok(bytes)
}

fn characterize_loaded(
    request: CharacterizationRequest,
    loaded: LoadedInputs,
) -> Result<CharacterizationReport, String> {
    validate_loaded_geometry(request.layout, &loaded, &request.cases)?;
    let verified_inputs = VerifiedInputIdentities {
        rspboot_bytes: loaded.rspboot.len(),
        rspboot_sha256: hex_digest(Sha256Digest::hash(&loaded.rspboot)),
        text_bytes: loaded.text.len(),
        text_sha256: hex_digest(Sha256Digest::hash(&loaded.text)),
        data_bytes: loaded.data.len(),
        data_sha256: hex_digest(Sha256Digest::hash(&loaded.data)),
    };
    let request_sha256 = hex_digest(request_digest(&request, &loaded));
    let mut reports = Vec::with_capacity(request.cases.len());
    for case in request.cases {
        reports.push(run_case(request.layout, &loaded, case)?);
    }
    Ok(CharacterizationReport {
        schema: REPORT_SCHEMA,
        fixture_revision: FIXTURE_REVISION,
        request_sha256,
        verified_inputs,
        cases: reports,
    })
}

fn run_case(
    layout: CharacterizationLayout,
    loaded: &LoadedInputs,
    case: CharacterizationCase,
) -> Result<CaseReport, String> {
    let _content_safe_diagnostics = crate::rsp::recomp::ContentSafeDiagnosticsGuard::enter();
    let case_sha256 = hex_digest(case_digest(&case));
    let axis = experiment_axis(&case.parameters);
    let baseline = build_baseline_rdram(layout, loaded, &case.sentinels);
    let common_baseline_sha256 =
        hex_digest(baseline_digest(layout, loaded, &case.sentinels, &baseline));
    let executions = case
        .trials
        .iter()
        .map(|trial| run_trial(layout, loaded, &case.sentinels, &baseline, trial))
        .collect::<Result<Vec<_>, _>>()?;
    for (candidate_trial, candidate) in executions.iter().enumerate().skip(1) {
        if executions[0].phases[0].entry_snapshot != candidate.phases[0].entry_snapshot {
            return Err(format!(
                "characterization case trial {candidate_trial} did not reach the control's same post-rspboot entry snapshot"
            ));
        }
    }
    let comparisons = (1..executions.len())
        .map(|candidate_trial| TrialComparison {
            reference_trial: 0,
            candidate_trial,
            phases: executions[0]
                .phases
                .iter()
                .zip(&executions[candidate_trial].phases)
                .enumerate()
                .map(|(phase, (reference, candidate))| {
                    compare_phase_execution(phase, reference, candidate)
                })
                .collect(),
        })
        .collect();
    let trials = executions
        .into_iter()
        .map(|execution| execution.report)
        .collect();

    Ok(CaseReport {
        case_sha256,
        axis,
        common_baseline_sha256,
        trials,
        comparisons,
    })
}

fn build_baseline_rdram(
    layout: CharacterizationLayout,
    loaded: &LoadedInputs,
    sentinels: &[SentinelRange],
) -> Vec<u8> {
    let mut rdram = vec![0; DEFAULT_RDRAM_SIZE];
    install_logical(&mut rdram, layout.rspboot_address, &loaded.rspboot);
    install_logical(&mut rdram, layout.text_address, &loaded.text);
    install_logical(&mut rdram, layout.data_address, &loaded.data);
    for sentinel in sentinels {
        let bytes = (0..sentinel.byte_len)
            .map(|index| sentinel.pattern_seed.wrapping_add(index as u8))
            .collect::<Vec<_>>();
        install_logical(&mut rdram, sentinel.start, &bytes);
    }
    rdram
}

fn run_trial(
    layout: CharacterizationLayout,
    loaded: &LoadedInputs,
    sentinels: &[SentinelRange],
    baseline: &[u8],
    trial: &CharacterizationTrial,
) -> Result<TrialExecution, String> {
    let mut rdram = baseline.to_vec();
    let mut memory = RspMemory::new();
    let mut machine_backing = [0; 8];
    let machine = RspMachine::new(&mut machine_backing);
    let mut machine_state = machine.snapshot_state();
    let mut reports = Vec::with_capacity(trial.phases.len());
    let mut executions = Vec::with_capacity(trial.phases.len());

    for (phase_index, phase) in trial.phases.iter().enumerate() {
        let command_bytes = encode_packets(&phase.packets);
        install_logical(&mut rdram, layout.command_address, &command_bytes);
        let header = task_header(layout, loaded, command_bytes.len())?;
        install_logical(&mut rdram, layout.task_address, &encode_header(header));
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Imem, 0),
                &loaded.rspboot,
            )
            .map_err(|error| content_safe_rsp_memory_error("install rspboot IMEM", error))?;
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Dmem, TASK_DMEM_OFFSET),
                &encode_header(header),
            )
            .map_err(|error| content_safe_rsp_memory_error("install task DMEM", error))?;

        let input = AudioRspbootInput::new(
            RdramAddr::from_offset(layout.task_address),
            header,
            rdram,
            memory.snapshot(),
            0,
            machine_state,
        )
        .map_err(|error| content_safe_rspboot_error("rspboot input", error))?;
        let boot = execute_audio_rspboot_to_entry(input)
            .map_err(|error| content_safe_rspboot_error("rspboot execution", error))?;
        let boot_dma = dma_observations(boot.dma_journal());
        let rspboot_mutations = mutation_observations(boot.boot_rdram_patches());
        let rspboot_rdram_patches = boot.boot_rdram_patches().clone();
        let rspboot_dma_journal = boot.dma_journal().to_vec();
        let rspboot_imem_replacements = boot.imem_replacements().to_vec();
        let rspboot_imem_replacement_observations =
            imem_replacement_observations(boot.imem_replacements());
        let entry = boot.into_entry();
        let entry_snapshot = entry_snapshot_digest(&entry, layout, command_bytes.len());
        let identity = entry.identity();
        let lle = run_speculative_audio_lle(entry.fork_lle_lane().into_lle_parts())
            .map_err(content_safe_speculative_lle_error)?;

        let selected_digests = selected_digests(&lle, sentinels);
        let ucode_mutations = mutation_observations(lle.rdram_patches());
        reports.push(PhaseReport {
            phase: phase_index,
            phase_sha256: hex_digest(phase_digest(phase)),
            command_count: phase.packets.len(),
            entry_snapshot_sha256: hex_digest(entry_snapshot),
            captured_imem_sha256: hex_digest(identity.imem_sha256),
            captured_data_bytes: identity.ucode_data_bytes,
            captured_data_sha256: hex_digest(identity.ucode_data_sha256),
            terminal: terminal_name(lle.terminal()),
            rspboot_steps: lle.rspboot_steps().get(),
            ucode_steps: lle.ucode_steps().get(),
            rspboot_dma: boot_dma,
            ucode_dma: dma_observations(lle.dma_journal()),
            rspboot_imem_replacements: rspboot_imem_replacement_observations,
            deferred_dpc_submissions: deferred_dpc_observations(lle.dpc_submissions()),
            rspboot_mutations,
            ucode_mutations,
            selected_digests,
        });

        executions.push(PhaseExecution {
            entry_snapshot,
            rspboot_rdram_patches,
            rspboot_dma_journal,
            rspboot_imem_replacements,
            terminal: lle.terminal(),
            rspboot_steps: lle.rspboot_steps().get(),
            ucode_steps: lle.ucode_steps().get(),
            rdram_patches: lle.rdram_patches().clone(),
            rdram_storage: lle.rdram_storage().to_vec(),
            dmem: *lle.rsp_memory().bank(RspMemoryBank::Dmem),
            imem: *lle.rsp_memory().bank(RspMemoryBank::Imem),
            imem_generation: lle.rsp_memory().imem_generation(),
            machine_state: lle.machine_state().clone(),
            pc_low12: lle.pc_low12(),
            dma_journal: lle.dma_journal().to_vec(),
            deferred_dpc_submissions: lle.dpc_submissions().to_vec(),
            imem_replacements: lle.imem_replacements().to_vec(),
        });

        rdram = lle.rdram_storage().to_vec();
        memory = RspMemory::from_snapshot(lle.rsp_memory().clone());
        machine_state = RspMachineState::from_architectural_state(
            lle.machine_state().architectural_state().clone(),
        );
    }

    Ok(TrialExecution {
        report: TrialReport {
            trial_sha256: hex_digest(trial_digest(trial)),
            phases: reports,
        },
        phases: executions,
    })
}

fn task_header(
    layout: CharacterizationLayout,
    loaded: &LoadedInputs,
    command_len: usize,
) -> Result<OsTaskHeader, String> {
    Ok(OsTaskHeader {
        task_type: M_AUDTASK,
        ucode_boot: layout.rspboot_address,
        ucode_boot_size: checked_u32("rspboot", loaded.rspboot.len())?,
        ucode: layout.text_address,
        ucode_size: checked_u32("text", loaded.text.len())?,
        ucode_data: layout.data_address,
        ucode_data_size: checked_u32("data", loaded.data.len())?,
        data_ptr: layout.command_address,
        data_size: checked_u32("commands", command_len)?,
        ..OsTaskHeader::default()
    })
}

fn validate_loaded_geometry(
    layout: CharacterizationLayout,
    loaded: &LoadedInputs,
    cases: &[CharacterizationCase],
) -> Result<(), String> {
    if loaded.rspboot.is_empty() || loaded.rspboot.len() > 0x1000 || loaded.text.is_empty() {
        return Err("rspboot/text must be nonempty and rspboot must fit IMEM".into());
    }
    if !layout.task_address.is_multiple_of(8) || !layout.command_address.is_multiple_of(8) {
        return Err("task and command addresses must be 8-byte aligned".into());
    }
    let max_commands = cases
        .iter()
        .flat_map(|case| &case.trials)
        .flat_map(|trial| &trial.phases)
        .map(|phase| phase.packets.len())
        .max()
        .unwrap_or(0);
    let fixed_ranges = vec![
        named_range("task", layout.task_address, 64)?,
        named_range("rspboot", layout.rspboot_address, loaded.rspboot.len())?,
        named_range("text", layout.text_address, loaded.text.len())?,
        named_range("data", layout.data_address, loaded.data.len())?,
        named_range(
            "commands",
            layout.command_address,
            max_commands.saturating_mul(8),
        )?,
    ];
    for (case_index, case) in cases.iter().enumerate() {
        let mut ranges = fixed_ranges.clone();
        for sentinel in &case.sentinels {
            if sentinel.byte_len == 0 {
                return Err(format!("case {case_index} has an empty sentinel"));
            }
            ranges.push(named_range(
                "sentinel",
                sentinel.start,
                sentinel.byte_len as usize,
            )?);
        }
        ranges.sort_unstable_by_key(|range| range.1);
        for pair in ranges.windows(2) {
            if pair[1].1 < pair[0].2 {
                return Err(format!(
                    "case {case_index}: {} overlaps {} in RDRAM layout",
                    pair[0].0, pair[1].0
                ));
            }
        }
    }
    Ok(())
}

fn named_range(
    name: &'static str,
    start: u32,
    byte_len: usize,
) -> Result<(&'static str, usize, usize), String> {
    let start = start as usize;
    let end = start
        .checked_add(byte_len)
        .ok_or_else(|| format!("{name} range overflows"))?;
    if end > DEFAULT_RDRAM_SIZE {
        return Err(format!("{name} range exceeds physical RDRAM"));
    }
    Ok((name, start, end))
}

fn request_digest(request: &CharacterizationRequest, loaded: &LoadedInputs) -> Sha256Digest {
    let mut wire = Vec::new();
    push_wire_str(&mut wire, REQUEST_SCHEMA);
    wire.extend_from_slice(&request.fixture_revision.to_be_bytes());
    wire.extend_from_slice(&Sha256Digest::hash(&loaded.rspboot).bytes());
    wire.extend_from_slice(&Sha256Digest::hash(&loaded.text).bytes());
    wire.extend_from_slice(&Sha256Digest::hash(&loaded.data).bytes());
    for value in [
        request.layout.task_address,
        request.layout.rspboot_address,
        request.layout.text_address,
        request.layout.data_address,
        request.layout.command_address,
    ] {
        wire.extend_from_slice(&value.to_be_bytes());
    }
    push_wire_len(&mut wire, request.cases.len());
    for case in &request.cases {
        wire.extend_from_slice(&case_digest(case).bytes());
    }
    Sha256Digest::hash(&wire)
}

fn case_digest(case: &CharacterizationCase) -> Sha256Digest {
    let mut wire = Vec::new();
    push_wire_str(&mut wire, "fn64.audio-abi-characterization-case.v2");
    push_wire_str(&mut wire, &case.id);
    encode_experiment_parameters(&mut wire, &case.parameters);
    push_wire_len(&mut wire, case.sentinels.len());
    for sentinel in &case.sentinels {
        wire.extend_from_slice(&sentinel.start.to_be_bytes());
        wire.extend_from_slice(&sentinel.byte_len.to_be_bytes());
        wire.push(sentinel.pattern_seed);
    }
    push_wire_len(&mut wire, case.trials.len());
    for trial in &case.trials {
        wire.extend_from_slice(&trial_digest(trial).bytes());
    }
    Sha256Digest::hash(&wire)
}

fn trial_digest(trial: &CharacterizationTrial) -> Sha256Digest {
    let mut wire = Vec::new();
    push_wire_str(&mut wire, "fn64.audio-abi-characterization-trial.v2");
    push_wire_len(&mut wire, trial.phases.len());
    for phase in &trial.phases {
        wire.extend_from_slice(&phase_digest(phase).bytes());
    }
    Sha256Digest::hash(&wire)
}

fn phase_digest(phase: &CharacterizationPhase) -> Sha256Digest {
    let mut wire = Vec::new();
    push_wire_str(&mut wire, "fn64.audio-abi-characterization-phase.v2");
    push_wire_len(&mut wire, phase.packets.len());
    for packet in &phase.packets {
        wire.extend_from_slice(&packet.word0.to_be_bytes());
        wire.extend_from_slice(&packet.word1.to_be_bytes());
    }
    Sha256Digest::hash(&wire)
}

fn experiment_axis(parameters: &ExperimentParameters) -> &'static str {
    match parameters {
        ExperimentParameters::Address { .. } => "address",
        ExperimentParameters::Selector { .. } => "selector",
        ExperimentParameters::Count { .. } => "count",
        ExperimentParameters::DmemMove { .. } => "dmem_move",
        ExperimentParameters::Aux { .. } => "aux",
        ExperimentParameters::Reserved { .. } => "reserved",
        ExperimentParameters::Persistence { .. } => "persistence",
    }
}

fn encode_experiment_parameters(wire: &mut Vec<u8>, parameters: &ExperimentParameters) {
    push_wire_str(wire, experiment_axis(parameters));
    match parameters {
        ExperimentParameters::Address {
            opcode,
            selector,
            address,
            alignment,
        } => {
            wire.extend_from_slice(&[*opcode, *selector]);
            wire.extend_from_slice(&address.to_be_bytes());
            wire.push(*alignment);
        }
        ExperimentParameters::Selector { opcode, selector } => {
            wire.extend_from_slice(&[*opcode, *selector]);
        }
        ExperimentParameters::Count { opcode, count } => {
            wire.push(*opcode);
            wire.extend_from_slice(&count.to_be_bytes());
        }
        ExperimentParameters::DmemMove {
            input_dmem,
            output_dmem,
            count,
            overlap,
        } => {
            wire.extend_from_slice(&input_dmem.to_be_bytes());
            wire.extend_from_slice(&output_dmem.to_be_bytes());
            wire.extend_from_slice(&count.to_be_bytes());
            wire.push(match overlap {
                DmemMoveOverlap::None => 0,
                DmemMoveOverlap::Forward => 1,
                DmemMoveOverlap::Backward => 2,
                DmemMoveOverlap::ExactAlias => 3,
            });
        }
        ExperimentParameters::Aux {
            flags,
            input_dmem,
            output_dmem,
            aux_a,
            aux_c,
            aux_e,
        } => {
            wire.push(*flags);
            for value in [input_dmem, output_dmem, aux_a, aux_c, aux_e] {
                wire.extend_from_slice(&value.to_be_bytes());
            }
        }
        ExperimentParameters::Reserved {
            opcode,
            word0_reserved_mask,
            word1_reserved_mask,
        } => {
            wire.push(*opcode);
            wire.extend_from_slice(&word0_reserved_mask.to_be_bytes());
            wire.extend_from_slice(&word1_reserved_mask.to_be_bytes());
        }
        ExperimentParameters::Persistence { state, task_count } => {
            wire.push(match state {
                PersistenceState::Segment => 0,
                PersistenceState::Loop => 1,
                PersistenceState::Codebook => 2,
                PersistenceState::Buffer => 3,
                PersistenceState::ScalarVector => 4,
            });
            wire.extend_from_slice(&task_count.to_be_bytes());
        }
    }
}

fn push_wire_str(wire: &mut Vec<u8>, value: &str) {
    push_wire_len(wire, value.len());
    wire.extend_from_slice(value.as_bytes());
}

fn push_wire_len(wire: &mut Vec<u8>, value: usize) {
    wire.extend_from_slice(
        &u64::try_from(value)
            .expect("characterization wire length must fit u64")
            .to_be_bytes(),
    );
}

fn baseline_digest(
    layout: CharacterizationLayout,
    loaded: &LoadedInputs,
    sentinels: &[SentinelRange],
    baseline: &[u8],
) -> Sha256Digest {
    let mut wire = Vec::new();
    push_wire_str(&mut wire, "fn64.audio-abi-characterization-baseline.v2");
    for value in [
        layout.task_address,
        layout.rspboot_address,
        layout.text_address,
        layout.data_address,
        layout.command_address,
    ] {
        wire.extend_from_slice(&value.to_be_bytes());
    }
    wire.extend_from_slice(&Sha256Digest::hash(&loaded.rspboot).bytes());
    wire.extend_from_slice(&Sha256Digest::hash(&loaded.text).bytes());
    wire.extend_from_slice(&Sha256Digest::hash(&loaded.data).bytes());
    push_wire_len(&mut wire, sentinels.len());
    for sentinel in sentinels {
        wire.extend_from_slice(&sentinel.start.to_be_bytes());
        wire.extend_from_slice(&sentinel.byte_len.to_be_bytes());
        wire.push(sentinel.pattern_seed);
    }
    wire.extend_from_slice(&Sha256Digest::hash(baseline).bytes());
    Sha256Digest::hash(&wire)
}

/// Hash the complete LLE entry condition while excluding only the declared
/// packet bytes that intentionally differ between matrix trials.
///
/// If rspboot reads those bytes and changes any other RDRAM byte, RSP memory,
/// register, PC, work counter, or DMA authority, the digest still differs and
/// the first-phase same-snapshot precondition rejects the matrix.
fn entry_snapshot_digest(
    entry: &crate::hle_snapshot::AudioTaskEntrySnapshot,
    layout: CharacterizationLayout,
    command_bytes: usize,
) -> Sha256Digest {
    let mut rdram = entry.rdram().storage().to_vec();
    install_logical(&mut rdram, layout.command_address, &vec![0; command_bytes]);
    let mut wire = Vec::new();
    push_wire_str(
        &mut wire,
        "fn64.audio-abi-characterization-entry-snapshot.v2",
    );
    wire.extend_from_slice(&Sha256Digest::hash(&rdram).bytes());
    wire.extend_from_slice(&encode_header(entry.loaded_header()));
    wire.extend_from_slice(&encode_header(entry.entry_header()));
    wire.extend_from_slice(
        &Sha256Digest::hash(entry.rsp_memory().bank(RspMemoryBank::Dmem)).bytes(),
    );
    wire.extend_from_slice(
        &Sha256Digest::hash(entry.rsp_memory().bank(RspMemoryBank::Imem)).bytes(),
    );
    wire.extend_from_slice(&entry.rsp_memory().imem_generation().to_be_bytes());
    wire.extend_from_slice(&architecture_digest(entry.machine_state()).bytes());
    wire.extend_from_slice(&entry.entry_pc_low12().to_be_bytes());
    wire.extend_from_slice(&entry.rspboot_steps().get().to_be_bytes());
    push_wire_len(&mut wire, entry.admitted_dma_ranges().len());
    for range in entry.admitted_dma_ranges() {
        wire.extend_from_slice(&(range.start as u64).to_be_bytes());
        wire.extend_from_slice(&(range.end as u64).to_be_bytes());
    }
    Sha256Digest::hash(&wire)
}

fn compare_phase_execution(
    phase: usize,
    reference: &PhaseExecution,
    candidate: &PhaseExecution,
) -> PhaseComparison {
    let rdram_differences = rdram_effect_differences(reference, candidate);
    let dmem_differences = difference_ranges(0, &reference.dmem, &candidate.dmem);
    let imem_differences = difference_ranges(0, &reference.imem, &candidate.imem);
    let first_divergence = first_phase_divergence(reference, candidate);
    PhaseComparison {
        phase,
        equivalent: first_divergence.is_none(),
        first_divergence,
        rdram_differences,
        dmem_differences,
        imem_differences,
        reference_entry_snapshot_sha256: hex_digest(reference.entry_snapshot),
        candidate_entry_snapshot_sha256: hex_digest(candidate.entry_snapshot),
        reference_architecture_sha256: hex_digest(architecture_digest(&reference.machine_state)),
        candidate_architecture_sha256: hex_digest(architecture_digest(&candidate.machine_state)),
        reference_deferred_dpc_sha256: hex_digest(deferred_dpc_digest(
            &reference.deferred_dpc_submissions,
        )),
        candidate_deferred_dpc_sha256: hex_digest(deferred_dpc_digest(
            &candidate.deferred_dpc_submissions,
        )),
    }
}

fn first_phase_divergence(
    reference: &PhaseExecution,
    candidate: &PhaseExecution,
) -> Option<FirstDivergence> {
    if let Some(value) = first_patch_divergence(
        reference.rspboot_rdram_patches.as_slice(),
        candidate.rspboot_rdram_patches.as_slice(),
        "rspboot_rdram_patch_range",
        "rspboot_rdram_patch_byte",
    ) {
        return Some(value);
    }
    for index in 0..reference
        .rspboot_dma_journal
        .len()
        .max(candidate.rspboot_dma_journal.len())
    {
        if reference.rspboot_dma_journal.get(index) != candidate.rspboot_dma_journal.get(index) {
            return Some(divergence("rspboot_dma_journal", Some(index), None));
        }
    }
    for index in 0..reference
        .rspboot_imem_replacements
        .len()
        .max(candidate.rspboot_imem_replacements.len())
    {
        if reference.rspboot_imem_replacements.get(index)
            != candidate.rspboot_imem_replacements.get(index)
        {
            return Some(divergence("rspboot_imem_replacement", Some(index), None));
        }
    }
    if reference.entry_snapshot != candidate.entry_snapshot {
        return Some(divergence("entry_snapshot", None, None));
    }
    if reference.terminal != candidate.terminal {
        return Some(divergence("terminal", None, None));
    }
    if let Some(value) = first_patch_divergence(
        reference.rdram_patches.as_slice(),
        candidate.rdram_patches.as_slice(),
        "rdram_patch_range",
        "rdram_patch_byte",
    ) {
        return Some(value);
    }
    if let Some(address) = first_byte_difference(&reference.dmem, &candidate.dmem) {
        return Some(divergence("dmem_byte", None, Some(address)));
    }
    if let Some(address) = first_byte_difference(&reference.imem, &candidate.imem) {
        return Some(divergence("imem_byte", None, Some(address)));
    }
    if reference.imem_generation != candidate.imem_generation {
        return Some(divergence("imem_generation", None, None));
    }
    if reference.pc_low12 != candidate.pc_low12 {
        return Some(divergence("sp_pc", None, Some(reference.pc_low12)));
    }
    if let Some(value) = first_architecture_divergence(
        reference.machine_state.architectural_state(),
        candidate.machine_state.architectural_state(),
    ) {
        return Some(value);
    }
    for index in 0..reference
        .deferred_dpc_submissions
        .len()
        .max(candidate.deferred_dpc_submissions.len())
    {
        if reference.deferred_dpc_submissions.get(index)
            != candidate.deferred_dpc_submissions.get(index)
        {
            return Some(divergence("deferred_dpc_submission", Some(index), None));
        }
    }
    for index in 0..reference.dma_journal.len().max(candidate.dma_journal.len()) {
        if reference.dma_journal.get(index) != candidate.dma_journal.get(index) {
            return Some(divergence("dma_journal", Some(index), None));
        }
    }
    for index in 0..reference
        .imem_replacements
        .len()
        .max(candidate.imem_replacements.len())
    {
        if reference.imem_replacements.get(index) != candidate.imem_replacements.get(index) {
            return Some(divergence("imem_replacement", Some(index), None));
        }
    }
    if reference.rspboot_steps != candidate.rspboot_steps {
        return Some(divergence("rspboot_steps", None, None));
    }
    if reference.ucode_steps != candidate.ucode_steps {
        return Some(divergence("ucode_steps", None, None));
    }
    None
}

fn first_patch_divergence(
    reference_patches: &[crate::hle_outcome::RdramPatch],
    candidate_patches: &[crate::hle_outcome::RdramPatch],
    range_domain: &'static str,
    byte_domain: &'static str,
) -> Option<FirstDivergence> {
    for index in 0..reference_patches.len().max(candidate_patches.len()) {
        let reference_patch = reference_patches.get(index);
        let candidate_patch = candidate_patches.get(index);
        if reference_patch.map(|patch| patch.range()) != candidate_patch.map(|patch| patch.range())
        {
            return Some(divergence(range_domain, Some(index), None));
        }
        if let (Some(reference_patch), Some(candidate_patch)) = (reference_patch, candidate_patch) {
            if let Some(offset) = reference_patch
                .bytes()
                .iter()
                .zip(candidate_patch.bytes())
                .position(|(left, right)| left != right)
            {
                return Some(divergence(
                    byte_domain,
                    Some(index),
                    Some(reference_patch.range().start() + offset as u32),
                ));
            }
        }
    }
    None
}

fn divergence(domain: &'static str, index: Option<usize>, address: Option<u32>) -> FirstDivergence {
    FirstDivergence {
        domain,
        index,
        address,
    }
}

fn first_architecture_divergence(
    reference: &crate::rsp::runtime::RspArchitecturalState,
    candidate: &crate::rsp::runtime::RspArchitecturalState,
) -> Option<FirstDivergence> {
    for (index, (left, right)) in reference.gprs().iter().zip(candidate.gprs()).enumerate() {
        if left != right {
            return Some(divergence("gpr", Some(index), None));
        }
    }
    let scalar_fields = [
        (
            "dma_dram_address",
            reference.dma_dram_address(),
            candidate.dma_dram_address(),
        ),
        (
            "dma_mem_address",
            reference.dma_mem_address(),
            candidate.dma_mem_address(),
        ),
        (
            "jump_target",
            reference.jump_target(),
            candidate.jump_target(),
        ),
        (
            "resume_address",
            reference.resume_address(),
            candidate.resume_address(),
        ),
        ("sp_status", reference.sp_status(), candidate.sp_status()),
        (
            "dma_read_length",
            reference.dma_read_length(),
            candidate.dma_read_length(),
        ),
        (
            "dma_write_length",
            reference.dma_write_length(),
            candidate.dma_write_length(),
        ),
        ("dp_start", reference.dp_start(), candidate.dp_start()),
        ("dp_end", reference.dp_end(), candidate.dp_end()),
        ("dp_current", reference.dp_current(), candidate.dp_current()),
        ("dp_status", reference.dp_status(), candidate.dp_status()),
        ("dp_clock", reference.dp_clock(), candidate.dp_clock()),
        ("dp_busy", reference.dp_busy(), candidate.dp_busy()),
        (
            "dp_pipe_busy",
            reference.dp_pipe_busy(),
            candidate.dp_pipe_busy(),
        ),
        (
            "dp_tmem_busy",
            reference.dp_tmem_busy(),
            candidate.dp_tmem_busy(),
        ),
    ];
    for (domain, left, right) in scalar_fields {
        if left != right {
            return Some(divergence(domain, None, None));
        }
    }
    if reference.resume_delay() != candidate.resume_delay() {
        return Some(divergence("resume_delay", None, None));
    }
    if reference.sp_semaphore() != candidate.sp_semaphore() {
        return Some(divergence("sp_semaphore", None, None));
    }
    let reference_vu = reference.vu();
    let candidate_vu = candidate.vu();
    for register in 0..crate::rsp::NUM_VREGS {
        for lane in 0..crate::rsp::LANES {
            if reference_vu.regs.r[register][lane] != candidate_vu.regs.r[register][lane] {
                return Some(divergence(
                    "vector_register_lane",
                    Some(register * crate::rsp::LANES + lane),
                    None,
                ));
            }
        }
    }
    for lane in 0..crate::rsp::LANES {
        if reference_vu.acc.signed(lane) != candidate_vu.acc.signed(lane) {
            return Some(divergence("accumulator_lane", Some(lane), None));
        }
    }
    if reference_vu.flags != candidate_vu.flags {
        return Some(divergence("vector_flags", None, None));
    }
    if reference_vu.div_in != candidate_vu.div_in
        || reference_vu.div_in_loaded != candidate_vu.div_in_loaded
        || reference_vu.div_out != candidate_vu.div_out
    {
        return Some(divergence("vector_divider", None, None));
    }
    for index in 0..reference
        .dp_submissions()
        .len()
        .max(candidate.dp_submissions().len())
    {
        if reference.dp_submissions().get(index) != candidate.dp_submissions().get(index) {
            return Some(divergence("dpc_submission", Some(index), None));
        }
    }
    None
}

fn architecture_digest(state: &RspMachineState) -> Sha256Digest {
    let state = state.architectural_state();
    let mut wire = Vec::new();
    push_wire_str(&mut wire, "fn64.audio-abi-characterization-architecture.v2");
    for value in state.gprs() {
        wire.extend_from_slice(&value.to_be_bytes());
    }
    for value in [
        state.dma_dram_address(),
        state.dma_mem_address(),
        state.jump_target(),
        state.resume_address(),
        state.sp_status(),
        state.dma_read_length(),
        state.dma_write_length(),
        state.dp_start(),
        state.dp_end(),
        state.dp_current(),
        state.dp_status(),
        state.dp_clock(),
        state.dp_busy(),
        state.dp_pipe_busy(),
        state.dp_tmem_busy(),
    ] {
        wire.extend_from_slice(&value.to_be_bytes());
    }
    wire.push(u8::from(state.resume_delay()));
    wire.push(u8::from(state.sp_semaphore()));
    let vu = state.vu();
    for register in &vu.regs.r {
        for lane in register {
            wire.extend_from_slice(&lane.to_be_bytes());
        }
    }
    for lane in 0..crate::rsp::LANES {
        wire.extend_from_slice(&vu.acc.signed(lane).to_be_bytes());
    }
    wire.extend_from_slice(&vu.flags.vco.to_be_bytes());
    wire.extend_from_slice(&vu.flags.vcc.to_be_bytes());
    wire.push(vu.flags.vce);
    wire.extend_from_slice(&vu.div_in.to_be_bytes());
    wire.push(u8::from(vu.div_in_loaded));
    wire.extend_from_slice(&vu.div_out.to_be_bytes());
    push_wire_len(&mut wire, state.dp_submissions().len());
    for submission in state.dp_submissions() {
        wire.extend_from_slice(&submission.start.to_be_bytes());
        wire.extend_from_slice(&submission.end.to_be_bytes());
        match submission.source() {
            RspDpCommandSource::RdramWords(words) => {
                wire.push(0);
                push_wire_len(&mut wire, 0);
                push_wire_len(&mut wire, words.len());
                for word in words {
                    wire.extend_from_slice(&word.to_be_bytes());
                }
            }
            RspDpCommandSource::XbusBytes(bytes) => {
                wire.push(1);
                push_wire_len(&mut wire, bytes.len());
                wire.extend_from_slice(bytes);
                push_wire_len(&mut wire, bytes.len() / core::mem::size_of::<u32>());
                for word in bytes.chunks_exact(core::mem::size_of::<u32>()) {
                    wire.extend_from_slice(word);
                }
            }
        }
    }
    Sha256Digest::hash(&wire)
}

fn deferred_dpc_digest(submissions: &[DeferredDpcSubmission]) -> Sha256Digest {
    let mut wire = Vec::new();
    push_wire_str(&mut wire, "fn64.audio-abi-characterization-deferred-dpc.v2");
    push_wire_len(&mut wire, submissions.len());
    for submission in submissions {
        let identity = submission.identity();
        wire.push(match identity.source {
            DpcSubmissionSource::Rdram => 0,
            DpcSubmissionSource::Dmem => 1,
        });
        wire.extend_from_slice(&identity.start.to_be_bytes());
        wire.extend_from_slice(&identity.end.to_be_bytes());
        wire.extend_from_slice(&identity.command_sha256.bytes());
    }
    Sha256Digest::hash(&wire)
}

fn first_byte_difference(reference: &[u8], candidate: &[u8]) -> Option<u32> {
    reference
        .iter()
        .zip(candidate)
        .position(|(left, right)| left != right)
        .map(|offset| offset as u32)
}

fn difference_ranges(base: u32, reference: &[u8], candidate: &[u8]) -> Vec<DifferenceRange> {
    assert_eq!(reference.len(), candidate.len());
    let mut ranges = Vec::new();
    let mut cursor = 0;
    while cursor < reference.len() {
        if reference[cursor] == candidate[cursor] {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < reference.len() && reference[cursor] != candidate[cursor] {
            cursor += 1;
        }
        ranges.push(DifferenceRange {
            start: base + start as u32,
            byte_len: (cursor - start) as u32,
            reference_sha256: hex_digest(Sha256Digest::hash(&reference[start..cursor])),
            candidate_sha256: hex_digest(Sha256Digest::hash(&candidate[start..cursor])),
        });
    }
    ranges
}

fn rdram_effect_differences(
    reference: &PhaseExecution,
    candidate: &PhaseExecution,
) -> Vec<DifferenceRange> {
    let mut ranges = reference
        .rspboot_rdram_patches
        .as_slice()
        .iter()
        .chain(candidate.rspboot_rdram_patches.as_slice())
        .chain(reference.rdram_patches.as_slice())
        .chain(candidate.rdram_patches.as_slice())
        .map(|patch| {
            (
                patch.range().start(),
                patch.range().start() + patch.range().byte_len(),
            )
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    let reference_view = RdramView::from_storage(&reference.rdram_storage);
    let candidate_view = RdramView::from_storage(&candidate.rdram_storage);
    let mut differences = Vec::new();
    for (start, end) in merged {
        let mut reference_bytes = vec![0; (end - start) as usize];
        let mut candidate_bytes = vec![0; (end - start) as usize];
        reference_view.copy_logical_bytes(RdramAddr::from_offset(start), &mut reference_bytes);
        candidate_view.copy_logical_bytes(RdramAddr::from_offset(start), &mut candidate_bytes);
        differences.extend(difference_ranges(start, &reference_bytes, &candidate_bytes));
    }
    differences
}

fn selected_digests(
    lle: &crate::hle_lle::SpeculativeAudioLleResult,
    sentinels: &[SentinelRange],
) -> SelectedDigests {
    let view = RdramView::from_storage(lle.rdram_storage());
    SelectedDigests {
        native_rdram_sha256: hex_digest(Sha256Digest::hash(lle.rdram_storage())),
        dmem_sha256: hex_digest(Sha256Digest::hash(
            lle.rsp_memory().bank(RspMemoryBank::Dmem),
        )),
        imem_sha256: hex_digest(Sha256Digest::hash(
            lle.rsp_memory().bank(RspMemoryBank::Imem),
        )),
        sentinels: sentinels
            .iter()
            .enumerate()
            .map(|(sentinel_index, sentinel)| {
                let mut logical = vec![0; sentinel.byte_len as usize];
                view.copy_logical_bytes(RdramAddr::from_offset(sentinel.start), &mut logical);
                SentinelDigest {
                    sentinel: sentinel_index,
                    sha256: hex_digest(Sha256Digest::hash(&logical)),
                }
            })
            .collect(),
    }
}

fn dma_observations(entries: &[RspDmaJournalEntry]) -> Vec<DmaObservation> {
    entries
        .iter()
        .map(|entry| DmaObservation {
            direction: match entry.direction {
                RspDmaDirection::Read => "read",
                RspDmaDirection::Write => "write",
            },
            effective_dram_address: entry.effective_dram_address,
            sp_mem_address: entry.sp_mem_address,
            raw_length_descriptor: entry.raw_length_descriptor,
        })
        .collect()
}

fn imem_replacement_observations(
    replacements: &[crate::hle_effects::AudioImemReplacement],
) -> Vec<ImemReplacementObservation> {
    replacements
        .iter()
        .map(|replacement| ImemReplacementObservation {
            generation: replacement.generation(),
            sha256: hex_digest(replacement.identity()),
        })
        .collect()
}

fn deferred_dpc_observations(submissions: &[DeferredDpcSubmission]) -> Vec<DeferredDpcObservation> {
    submissions
        .iter()
        .map(|submission| {
            let identity = submission.identity();
            DeferredDpcObservation {
                source: match submission.source() {
                    DpcSubmissionSource::Rdram => "rdram",
                    DpcSubmissionSource::Dmem => "dmem",
                },
                start: submission.start(),
                end: submission.end(),
                command_sha256: hex_digest(identity.command_sha256),
            }
        })
        .collect()
}

fn content_safe_speculative_lle_error(error: SpeculativeAudioLleError) -> String {
    let reason = match error {
        SpeculativeAudioLleError::PhysicalRdramStorageLength { .. } => {
            "invalid physical RDRAM storage length"
        }
        SpeculativeAudioLleError::EntryPcUnaligned { .. } => "unaligned entry PC",
        SpeculativeAudioLleError::EntryPcOutsideFabricRange { .. } => {
            "entry PC outside fabric range"
        }
        SpeculativeAudioLleError::ZeroRspbootSteps => "zero rspboot work",
        SpeculativeAudioLleError::RspbootStepAccountingMismatch { .. } => {
            "rspboot work accounting mismatch"
        }
        SpeculativeAudioLleError::PreexistingDpcSubmissions { .. } => "preexisting DPC submissions",
        SpeculativeAudioLleError::NoAdmittedDmaRanges => "no admitted DMA ranges",
        SpeculativeAudioLleError::InvalidAdmittedDmaRange { .. } => "invalid admitted DMA range",
        SpeculativeAudioLleError::StepBoundExceeded { .. } => "execution step bound exceeded",
        SpeculativeAudioLleError::NonBreakExit { .. } => "non-BREAK interpreter exit",
        SpeculativeAudioLleError::RdramWriteRange { .. } => "invalid RDRAM write range",
        SpeculativeAudioLleError::RdramPatch(_) => "invalid RDRAM patch",
        SpeculativeAudioLleError::CanonicalRdramPatches(_) => "noncanonical RDRAM patch collection",
        SpeculativeAudioLleError::DeferredDpcSubmission(_) => "invalid deferred DPC submission",
        SpeculativeAudioLleError::XbusCommandWordCount { .. } => "XBUS command word count mismatch",
        SpeculativeAudioLleError::XbusCommandWordMismatch { .. } => {
            "XBUS command word content mismatch"
        }
        SpeculativeAudioLleError::RdramSubmissionHasXbusPayload { .. } => {
            "RDRAM DPC submission carried XBUS payload"
        }
    };
    format!("speculative LLE: {reason}")
}

fn content_safe_rsp_memory_error(stage: &'static str, error: RspMemoryError) -> String {
    let reason = match error {
        RspMemoryError::UnalignedWord { .. } => "unaligned RSP word access",
        RspMemoryError::CrossesBank { .. } => "RSP memory range crosses bank",
    };
    format!("{stage}: {reason}")
}

fn content_safe_rspboot_error(stage: &'static str, error: AudioRspbootError) -> String {
    let reason = match error {
        AudioRspbootError::PhysicalRdramStorageLength { .. } => {
            "invalid physical RDRAM storage length"
        }
        AudioRspbootError::NonAudioTask { .. } => "non-audio task",
        AudioRspbootError::DirectImemUnsupported => "direct IMEM task unsupported",
        AudioRspbootError::InitialPcUnaligned { .. } => "unaligned initial PC",
        AudioRspbootError::InitialPcOutsideFabricRange { .. } => "initial PC outside fabric range",
        AudioRspbootError::InitialDiagnosticSteps { .. } => "nonzero initial diagnostic work",
        AudioRspbootError::InitialPendingDpcSubmissions { .. } => "initial pending DPC submissions",
        AudioRspbootError::InitialExecutionContinuation { .. } => "initial execution continuation",
        AudioRspbootError::HeaderRange { .. } => "task header range invalid",
        AudioRspbootError::StaticAliasNotAllowed { .. } => "static alias not allowed",
        AudioRspbootError::LoadedHeaderDmemMismatch { .. } => {
            "loaded header differs from DMEM header"
        }
        AudioRspbootError::StepBoundExceeded { .. } => "execution step bound exceeded",
        AudioRspbootError::EarlyBreak { .. } => "rspboot broke before ucode entry",
        AudioRspbootError::UnexpectedExit { .. } => "unexpected rspboot interpreter exit",
        AudioRspbootError::RspbootDpcSubmissions { .. } => "rspboot submitted DPC work",
        AudioRspbootError::ZeroRspbootSteps => "zero rspboot work",
        AudioRspbootError::RdramWriteRange { .. } => "invalid RDRAM write range",
        AudioRspbootError::RdramPatch(_) => "invalid RDRAM patch",
        AudioRspbootError::CanonicalRdramPatches(_) => "noncanonical RDRAM patch collection",
        AudioRspbootError::EntrySnapshot(source) => content_safe_snapshot_error(source),
    };
    format!("{stage}: {reason}")
}

fn content_safe_snapshot_error(error: AudioHleSnapshotError) -> &'static str {
    match error {
        AudioHleSnapshotError::PhysicalRdramStorageLength { .. } => {
            "entry snapshot has invalid physical RDRAM storage length"
        }
        AudioHleSnapshotError::TaskAddressUnaligned { .. } => {
            "entry snapshot task address is unaligned"
        }
        AudioHleSnapshotError::HeaderRange { .. } => "entry snapshot header range is invalid",
        AudioHleSnapshotError::StaticAliasNotAllowed { .. } => {
            "entry snapshot static alias is not allowed"
        }
        AudioHleSnapshotError::NonAudioTask { .. } => "entry snapshot is not an audio task",
        AudioHleSnapshotError::CommandAddressUnaligned { .. } => {
            "entry snapshot command address is unaligned"
        }
        AudioHleSnapshotError::PartialCommand { .. } => "entry snapshot command length is partial",
        AudioHleSnapshotError::ByteLengthMismatch { .. } => "entry snapshot byte length mismatch",
        AudioHleSnapshotError::LogicalBytesMismatch { .. } => {
            "entry snapshot logical bytes mismatch"
        }
        AudioHleSnapshotError::MicrocodeIdentityMismatch { component } => {
            content_safe_microcode_identity_mismatch(component)
        }
        AudioHleSnapshotError::EntryPcUnaligned { .. } => "entry snapshot PC is unaligned",
        AudioHleSnapshotError::EntryPcOutsideFabricRange { .. } => {
            "entry snapshot PC is outside fabric range"
        }
        AudioHleSnapshotError::EntryPcResumeMismatch { .. } => "entry snapshot PC/resume mismatch",
        AudioHleSnapshotError::ZeroRspbootSteps => "entry snapshot has zero rspboot work",
        AudioHleSnapshotError::RspbootStepAccountingMismatch { .. } => {
            "entry snapshot rspboot work accounting mismatch"
        }
        AudioHleSnapshotError::NoAdmittedDmaRanges => "entry snapshot has no admitted DMA ranges",
        AudioHleSnapshotError::EmptyDmaRange { .. } => "entry snapshot has an empty DMA range",
        AudioHleSnapshotError::StaticDmaAliasNotAllowed { .. } => {
            "entry snapshot static DMA alias is not allowed"
        }
    }
}

fn content_safe_microcode_identity_mismatch(
    component: crate::hle_snapshot::MicrocodeIdentityMismatch,
) -> &'static str {
    match component {
        crate::hle_snapshot::MicrocodeIdentityMismatch::ImemDigest { .. } => {
            "entry snapshot IMEM identity mismatch"
        }
        crate::hle_snapshot::MicrocodeIdentityMismatch::UcodeDataLength { .. } => {
            "entry snapshot ucode-data length mismatch"
        }
        crate::hle_snapshot::MicrocodeIdentityMismatch::UcodeDataDigest { .. } => {
            "entry snapshot ucode-data identity mismatch"
        }
    }
}

fn mutation_observations(
    patches: &crate::hle_outcome::CanonicalRdramPatches,
) -> Vec<MutationObservation> {
    patches
        .as_slice()
        .iter()
        .map(|patch| MutationObservation {
            start: patch.range().start(),
            byte_len: patch.range().byte_len(),
            sha256: hex_digest(Sha256Digest::hash(patch.bytes())),
        })
        .collect()
}

fn terminal_name(reason: AudioTaskTerminalReason) -> &'static str {
    match reason {
        AudioTaskTerminalReason::Broke => "broke",
        AudioTaskTerminalReason::StepLimit => "step_limit",
        AudioTaskTerminalReason::UnsupportedInstruction => "unsupported_instruction",
        AudioTaskTerminalReason::ImemOverrun => "imem_overrun",
        AudioTaskTerminalReason::UnhandledJumpTarget => "unhandled_jump_target",
        AudioTaskTerminalReason::PendingOverlaySwap => "pending_overlay_swap",
        AudioTaskTerminalReason::UnhandledResumeTarget => "unhandled_resume_target",
    }
}

fn encode_packets(packets: &[PublicCommandPacket]) -> Vec<u8> {
    packets
        .iter()
        .flat_map(|packet| {
            packet
                .word0
                .to_be_bytes()
                .into_iter()
                .chain(packet.word1.to_be_bytes())
        })
        .collect()
}

fn encode_header(header: OsTaskHeader) -> [u8; 64] {
    let fields = [
        header.task_type,
        header.flags,
        header.ucode_boot,
        header.ucode_boot_size,
        header.ucode,
        header.ucode_size,
        header.ucode_data,
        header.ucode_data_size,
        header.dram_stack,
        header.dram_stack_size,
        header.output_buff,
        header.output_buff_size,
        header.data_ptr,
        header.data_size,
        header.yield_data_ptr,
        header.yield_data_size,
    ];
    let mut bytes = [0; 64];
    for (field, output) in fields.into_iter().zip(bytes.chunks_exact_mut(4)) {
        output.copy_from_slice(&field.to_be_bytes());
    }
    bytes
}

fn install_logical(rdram: &mut [u8], start: u32, bytes: &[u8]) {
    RdramViewMut::from_storage(rdram).write_logical_bytes(RdramAddr::from_offset(start), bytes);
}

fn checked_u32(label: &str, value: usize) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{label} byte length exceeds u32"))
}

fn parse_digest(value: &str) -> Result<Sha256Digest, String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("digest must contain exactly 64 hexadecimal characters".into());
    }
    let mut bytes = [0; 32];
    for (index, output) in bytes.iter_mut().enumerate() {
        *output = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid hexadecimal digest")?;
    }
    Ok(Sha256Digest::new(bytes))
}

fn hex_digest(digest: Sha256Digest) -> String {
    let mut output = String::with_capacity(64);
    for byte in digest.bytes() {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests;
