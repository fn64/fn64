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
    M_AUDTASK,
};
use serde::{Deserialize, Serialize};

use crate::hle_lle::run_speculative_audio_lle;
use crate::hle_outcome::{AudioTaskTerminalReason, Sha256Digest};
use crate::hle_rspboot::{execute_audio_rspboot_to_entry, AudioRspbootInput};
use crate::rsp::runtime::{RspDmaDirection, RspDmaJournalEntry, RspMachine, RspMachineState};

pub const REQUEST_SCHEMA: &str = "fn64.audio-abi-characterization-request.v1";
pub const REPORT_SCHEMA: &str = "fn64.audio-abi-characterization-report.v1";
pub const FIXTURE_REVISION: u32 = 1;
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
    pub phases: Vec<PhaseReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PhaseReport {
    pub phase: usize,
    pub phase_sha256: String,
    pub command_count: usize,
    pub captured_imem_sha256: String,
    pub captured_data_bytes: u32,
    pub captured_data_sha256: String,
    pub terminal: &'static str,
    pub rspboot_steps: u64,
    pub ucode_steps: u64,
    pub rspboot_dma: Vec<DmaObservation>,
    pub ucode_dma: Vec<DmaObservation>,
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

pub fn characterize_request(
    request: CharacterizationRequest,
) -> Result<CharacterizationReport, String> {
    validate_request_header(&request)?;
    let loaded = LoadedInputs {
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
    };
    characterize_loaded(request, loaded)
}

pub fn canonical_report_json(report: &CharacterizationReport) -> Result<String, String> {
    serde_json::to_string(report).map_err(|error| format!("serialize report: {error}"))
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
            || case.phases.is_empty()
        {
            return Err("every case requires a nonempty id and at least one phase".into());
        }
        if case.phases.iter().any(|phase| phase.packets.is_empty()) {
            return Err(format!(
                "characterization case {case_index} contains an empty command phase"
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
        if let ExperimentParameters::Persistence { task_count, .. } = case.parameters {
            if usize::from(task_count) != case.phases.len() || task_count < 2 {
                return Err(format!(
                    "persistence case {case_index} task_count must equal at least two phases"
                ));
            }
        } else if case.phases.len() != 1 {
            return Err(format!(
                "non-persistence case {case_index} must contain exactly one phase"
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
    let mut rdram = vec![0; DEFAULT_RDRAM_SIZE];
    install_logical(&mut rdram, layout.rspboot_address, &loaded.rspboot);
    install_logical(&mut rdram, layout.text_address, &loaded.text);
    install_logical(&mut rdram, layout.data_address, &loaded.data);
    for sentinel in &case.sentinels {
        let bytes = (0..sentinel.byte_len)
            .map(|index| sentinel.pattern_seed.wrapping_add(index as u8))
            .collect::<Vec<_>>();
        install_logical(&mut rdram, sentinel.start, &bytes);
    }

    let mut memory = RspMemory::new();
    let mut machine_backing = [0; 8];
    let machine = RspMachine::new(&mut machine_backing);
    let mut machine_state = machine.snapshot_state();
    let mut phase_reports = Vec::with_capacity(case.phases.len());
    let case_sha256 = hex_digest(case_digest(&case));
    let axis = experiment_axis(&case.parameters);

    for (phase_index, phase) in case.phases.iter().enumerate() {
        let command_bytes = encode_packets(&phase.packets);
        install_logical(&mut rdram, layout.command_address, &command_bytes);
        let header = task_header(layout, loaded, command_bytes.len())?;
        install_logical(&mut rdram, layout.task_address, &encode_header(header));
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Imem, 0),
                &loaded.rspboot,
            )
            .map_err(|error| format!("install rspboot IMEM: {error:?}"))?;
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Dmem, TASK_DMEM_OFFSET),
                &encode_header(header),
            )
            .map_err(|error| format!("install task DMEM: {error:?}"))?;

        let input = AudioRspbootInput::new(
            RdramAddr::from_offset(layout.task_address),
            header,
            rdram,
            memory.snapshot(),
            0,
            machine_state,
        )
        .map_err(|error| format!("rspboot input: {error:?}"))?;
        let boot = execute_audio_rspboot_to_entry(input)
            .map_err(|error| format!("rspboot execution: {error:?}"))?;
        let boot_dma = dma_observations(boot.dma_journal());
        let rspboot_mutations = mutation_observations(boot.boot_rdram_patches());
        let entry = boot.into_entry();
        let identity = entry.identity();
        let lle = run_speculative_audio_lle(entry.fork_lle_lane().into_lle_parts())
            .map_err(|error| format!("speculative LLE: {error:?}"))?;

        let selected_digests = selected_digests(&lle, &case.sentinels);
        let ucode_mutations = mutation_observations(lle.rdram_patches());
        phase_reports.push(PhaseReport {
            phase: phase_index,
            phase_sha256: hex_digest(phase_digest(phase)),
            command_count: phase.packets.len(),
            captured_imem_sha256: hex_digest(identity.imem_sha256),
            captured_data_bytes: identity.ucode_data_bytes,
            captured_data_sha256: hex_digest(identity.ucode_data_sha256),
            terminal: terminal_name(lle.terminal()),
            rspboot_steps: lle.rspboot_steps().get(),
            ucode_steps: lle.ucode_steps().get(),
            rspboot_dma: boot_dma,
            ucode_dma: dma_observations(lle.dma_journal()),
            rspboot_mutations,
            ucode_mutations,
            selected_digests,
        });

        rdram = lle.rdram_storage().to_vec();
        memory = RspMemory::from_snapshot(lle.rsp_memory().clone());
        machine_state = RspMachineState::from_architectural_state(
            lle.machine_state().architectural_state().clone(),
        );
    }

    Ok(CaseReport {
        case_sha256,
        axis,
        phases: phase_reports,
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
        .flat_map(|case| &case.phases)
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
    push_wire_str(&mut wire, "fn64.audio-abi-characterization-case.v1");
    push_wire_str(&mut wire, &case.id);
    encode_experiment_parameters(&mut wire, &case.parameters);
    push_wire_len(&mut wire, case.sentinels.len());
    for sentinel in &case.sentinels {
        wire.extend_from_slice(&sentinel.start.to_be_bytes());
        wire.extend_from_slice(&sentinel.byte_len.to_be_bytes());
        wire.push(sentinel.pattern_seed);
    }
    push_wire_len(&mut wire, case.phases.len());
    for phase in &case.phases {
        wire.extend_from_slice(&phase_digest(phase).bytes());
    }
    Sha256Digest::hash(&wire)
}

fn phase_digest(phase: &CharacterizationPhase) -> Sha256Digest {
    let mut wire = Vec::new();
    push_wire_str(&mut wire, "fn64.audio-abi-characterization-phase.v1");
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
mod tests {
    use super::*;

    const BREAK: u32 = 0x0000_000d;

    fn mtc0(rt: u32, rd: u32) -> u32 {
        (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11)
    }

    fn public_fixture() -> (CharacterizationRequest, LoadedInputs) {
        let layout = CharacterizationLayout {
            task_address: 0x40,
            rspboot_address: 0x100,
            text_address: 0x200,
            data_address: 0x300,
            command_address: 0x400,
        };
        let boot_words = [
            0x2402_0000 | layout.text_address,
            mtc0(2, 1),
            0x2403_1080,
            mtc0(3, 0),
            0x2404_0007,
            mtc0(4, 2),
            0x0800_0020,
            0x2407_7777,
        ];
        let loaded = LoadedInputs {
            rspboot: boot_words
                .iter()
                .flat_map(|word| word.to_be_bytes())
                .collect(),
            text: [0x2405_5678u32, BREAK]
                .iter()
                .flat_map(|word| word.to_be_bytes())
                .collect(),
            data: vec![1, 2, 3, 4],
        };
        let request = CharacterizationRequest {
            schema: REQUEST_SCHEMA.into(),
            fixture_revision: FIXTURE_REVISION,
            microcode: PrivateMicrocodePaths {
                rspboot_path: PathBuf::new(),
                rspboot_sha256: String::new(),
                text_path: PathBuf::new(),
                text_sha256: String::new(),
                data_path: PathBuf::new(),
                data_sha256: String::new(),
            },
            layout,
            cases: vec![CharacterizationCase {
                id: "public-smoke".into(),
                parameters: ExperimentParameters::Count {
                    opcode: 2,
                    count: 8,
                },
                sentinels: vec![SentinelRange {
                    start: 0x500,
                    byte_len: 16,
                    pattern_seed: 0x40,
                }],
                phases: vec![CharacterizationPhase {
                    packets: vec![PublicCommandPacket {
                        word0: 0xdead_beef,
                        word1: 0xcafe_babe,
                    }],
                }],
            }],
        };
        (request, loaded)
    }

    #[test]
    fn public_fixture_is_deterministic_and_content_safe() {
        let (request, loaded) = public_fixture();
        let private_needles = loaded
            .rspboot
            .iter()
            .chain(&loaded.text)
            .chain(&loaded.data)
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>();
        let first =
            canonical_report_json(&characterize_loaded(request.clone(), loaded.clone()).unwrap())
                .unwrap();
        let second = canonical_report_json(&characterize_loaded(request, loaded).unwrap()).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("{\"schema\":\"fn64.audio-abi-characterization-report.v1\""));
        assert!(!first.contains("rspboot_path"));
        assert!(!first.contains("text_path"));
        assert!(!first.contains("public-smoke"));
        assert!(!first.contains("3735928559"));
        assert!(!first.contains("3405691582"));
        assert!(!first.contains("\"id\""));
        assert!(!first.contains("\"packets\""));
        assert!(!first.contains("\"parameters\""));
        assert!(!first.contains("\"layout\""));
        for needle in private_needles {
            assert!(
                !first.contains(&format!("\"{needle}\"")),
                "report serialized a raw one-byte string"
            );
        }
    }

    #[test]
    fn dma_journal_is_ordered_and_reports_raw_descriptor() {
        let (request, loaded) = public_fixture();
        let report = characterize_loaded(request, loaded).unwrap();
        let phase = &report.cases[0].phases[0];
        assert_eq!(phase.rspboot_dma.len(), 1);
        assert_eq!(phase.rspboot_dma[0].direction, "read");
        assert_eq!(phase.rspboot_dma[0].effective_dram_address, 0x200);
        assert_eq!(phase.rspboot_dma[0].sp_mem_address, 0x1080);
        assert_eq!(phase.rspboot_dma[0].raw_length_descriptor, 7);
        assert!(phase.ucode_dma.is_empty());
    }

    #[test]
    fn persistence_requires_multiple_matching_phases() {
        let (mut request, _) = public_fixture();
        request.cases[0].parameters = ExperimentParameters::Persistence {
            state: PersistenceState::Segment,
            task_count: 2,
        };
        assert!(validate_request_header(&request).is_err());
    }

    #[test]
    fn only_persistence_cases_can_carry_state_across_phases() {
        let (mut request, _) = public_fixture();
        let second = request.cases[0].phases[0].clone();
        request.cases[0].phases.push(second);
        assert!(validate_request_header(&request).is_err());
    }

    #[test]
    fn opaque_digests_bind_hidden_case_ids_and_packet_words() {
        let (request, loaded) = public_fixture();
        let original = characterize_loaded(request.clone(), loaded.clone()).unwrap();
        let mut changed = request;
        changed.cases[0].id = "different-private-label".into();
        changed.cases[0].phases[0].packets[0].word1 ^= 1;
        let changed = characterize_loaded(changed, loaded).unwrap();
        assert_ne!(original.request_sha256, changed.request_sha256);
        assert_ne!(original.cases[0].case_sha256, changed.cases[0].case_sha256);
        assert_ne!(
            original.cases[0].phases[0].phase_sha256,
            changed.cases[0].phases[0].phase_sha256
        );
    }

    #[test]
    fn exact_digest_parser_rejects_non_sha256_shapes() {
        assert!(parse_digest("00").is_err());
        assert_eq!(
            parse_digest(&"ab".repeat(32)).unwrap(),
            Sha256Digest::new([0xab; 32])
        );
    }

    #[test]
    fn request_schema_names_every_predeclared_experiment_axis() {
        let json = r#"[
            {"kind":"address","opcode":1,"selector":2,"address":3,"alignment":4},
            {"kind":"selector","opcode":5,"selector":6},
            {"kind":"count","opcode":7,"count":8},
            {"kind":"dmem_move","input_dmem":9,"output_dmem":10,"count":11,"overlap":"forward"},
            {"kind":"aux","flags":8,"input_dmem":12,"output_dmem":13,"aux_a":14,"aux_c":15,"aux_e":16},
            {"kind":"reserved","opcode":17,"word0_reserved_mask":18,"word1_reserved_mask":19},
            {"kind":"persistence","state":"codebook","task_count":2}
        ]"#;
        let parsed: Vec<ExperimentParameters> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.len(), 7);
    }
}
