//! Side-effect-free LLE execution from a validated post-rspboot audio snapshot.
//!
//! This runner owns every byte and register it mutates. It has no handle to
//! the live device fabric, renderer, JIT, scheduler, interrupt controller, or
//! timing model; those effects remain a later commit operation after an HLE
//! outcome compares equal.
//!
//! Provenance: RSP execution, IMEM overlay DMA, and BREAK behavior use this
//! crate's clean-room interpreter derived from the public SGI *Nintendo 64 RSP
//! Programmer's Guide*. Task-entry validation and lane ownership are
//! repository-owned policy in [`crate::hle_snapshot`]; family admission is
//! deliberately absent from this authoritative lane.

use core::num::NonZeroU64;

use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
use fn64_runtime::rsp::RspMemorySnapshot;
use fn64_runtime::{RdramAddr, RdramView, RspMemAddr, RspMemory, RspMemoryBank};

use crate::hle::AdmittedAudioMicrocode;
use crate::hle_effects::AudioImemReplacement;
use crate::hle_outcome::{
    AudioMicrocodeIdentity, AudioTaskOutcome, AudioTaskOutcomeError, AudioTaskTerminalReason,
    CanonicalRdramError, CanonicalRdramPatches, CanonicalRdramRanges, DeferredDpcSubmission,
    DpcSubmissionError, RdramByteRange, RdramPatch, RdramPatchError, RspDmaRegisterState,
    RspDpcRegisterState, RspVisibleState, RspVisibleStateError,
};
use crate::hle_snapshot::AudioLleLaneParts;
use crate::rsp::runtime::{
    RspDmaJournalEntry, RspDpCommandSource, RspDpSubmission, RspMachine, RspMachineState,
};
use crate::rsp::{run_imem, RspExitReason, DMEM_SIZE};

const INTERPRETER_CHUNK_STEPS: u64 = 1 << 20;
const MAX_UCODE_STEPS: u64 = 1 << 26;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpeculativeAudioLleError {
    PhysicalRdramStorageLength {
        storage_bytes: usize,
        required_bytes: usize,
    },
    EntryPcUnaligned {
        pc: u32,
    },
    EntryPcOutsideFabricRange {
        pc: u32,
    },
    ZeroRspbootSteps,
    RspbootStepAccountingMismatch {
        rspboot_steps: u64,
        diagnostic_steps: u64,
    },
    PreexistingDpcSubmissions {
        count: usize,
    },
    NoAdmittedDmaRanges,
    InvalidAdmittedDmaRange {
        start: usize,
        end: usize,
        physical_bytes: usize,
    },
    StepBoundExceeded {
        maximum_steps: u64,
        pc: u32,
    },
    NonBreakExit {
        reason: RspExitReason,
        pc: u32,
        ucode_steps: u64,
    },
    RdramWriteRange {
        start: usize,
        end: usize,
    },
    RdramPatch(RdramPatchError),
    CanonicalRdramPatches(CanonicalRdramError),
    DeferredDpcSubmission(DpcSubmissionError),
    XbusCommandWordCount {
        expected: usize,
        actual: usize,
    },
    XbusCommandWordMismatch {
        index: usize,
        payload_word: u32,
        raw_word: u32,
    },
    RdramSubmissionHasXbusPayload {
        byte_len: usize,
    },
}

/// Owned effects produced by one isolated LLE lane.
///
/// `rdram_write_ranges` is exact written coverage, including writes whose
/// final byte equals its entry value. `rdram_patches` carries those ranges in
/// canonical logical guest-byte order, while `rdram_storage` retains the
/// complete final native-word image needed by another isolated lane or a
/// later one-time commit.
#[derive(Clone, Debug)]
pub struct SpeculativeAudioLleResult {
    identity: AudioMicrocodeIdentity,
    terminal: AudioTaskTerminalReason,
    effects: SpeculativeAudioLleEffects,
    rspboot_steps: NonZeroU64,
    ucode_steps: NonZeroU64,
    dma_journal: Vec<RspDmaJournalEntry>,
}

/// Fully owned guest-visible mutations from a completed speculative lane.
///
/// This deliberately excludes HLE selection and PCM-range policy. A caller
/// may move the effects toward comparison or commit, but must still construct
/// an [`crate::hle_outcome::AudioTaskOutcome`] through that type's validated
/// API.
#[derive(Clone, Debug)]
pub struct SpeculativeAudioLleEffects {
    rdram_storage: Vec<u8>,
    rdram_write_ranges: Vec<RdramByteRange>,
    rdram_patches: CanonicalRdramPatches,
    rsp_memory: RspMemorySnapshot,
    machine_state: RspMachineState,
    pc_low12: u32,
    dpc_submissions: Vec<DeferredDpcSubmission>,
    imem_replacements: Vec<AudioImemReplacement>,
}

/// Failure to project an authoritative LLE result into the comparison model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioLleOutcomeError {
    AdmissionIdentityMismatch {
        admitted: Box<AudioMicrocodeIdentity>,
        result: Box<AudioMicrocodeIdentity>,
    },
    RspVisibleState(RspVisibleStateError),
    Outcome(AudioTaskOutcomeError),
}

impl SpeculativeAudioLleResult {
    pub const fn identity(&self) -> AudioMicrocodeIdentity {
        self.identity
    }

    pub const fn terminal(&self) -> AudioTaskTerminalReason {
        self.terminal
    }

    pub fn rdram_storage(&self) -> &[u8] {
        self.effects.rdram_storage()
    }

    pub fn rdram_write_ranges(&self) -> &[RdramByteRange] {
        self.effects.rdram_write_ranges()
    }

    pub const fn rdram_patches(&self) -> &CanonicalRdramPatches {
        self.effects.rdram_patches()
    }

    pub const fn rsp_memory(&self) -> &RspMemorySnapshot {
        self.effects.rsp_memory()
    }

    pub const fn machine_state(&self) -> &RspMachineState {
        self.effects.machine_state()
    }

    pub const fn pc_low12(&self) -> u32 {
        self.effects.pc_low12()
    }

    pub fn dpc_submissions(&self) -> &[DeferredDpcSubmission] {
        self.effects.dpc_submissions()
    }

    /// Complete ucode-phase IMEM images, in DMA installation order.
    ///
    /// This journal is evidence beside the architectural state; it is not
    /// embedded in [`RspMachineState`].
    pub fn imem_replacements(&self) -> &[AudioImemReplacement] {
        self.effects.imem_replacements()
    }

    pub const fn rspboot_steps(&self) -> NonZeroU64 {
        self.rspboot_steps
    }

    pub const fn ucode_steps(&self) -> NonZeroU64 {
        self.ucode_steps
    }

    /// Diagnostic observations only; consuming effects discards this lane's
    /// journal before anything can become commit authority.
    pub fn dma_journal(&self) -> &[RspDmaJournalEntry] {
        &self.dma_journal
    }

    pub fn into_rdram_storage(self) -> Vec<u8> {
        self.effects.rdram_storage
    }

    pub fn into_effects(self) -> SpeculativeAudioLleEffects {
        self.effects
    }
}

impl SpeculativeAudioLleEffects {
    pub fn rdram_storage(&self) -> &[u8] {
        &self.rdram_storage
    }

    pub fn rdram_write_ranges(&self) -> &[RdramByteRange] {
        &self.rdram_write_ranges
    }

    pub const fn rdram_patches(&self) -> &CanonicalRdramPatches {
        &self.rdram_patches
    }

    pub const fn rsp_memory(&self) -> &RspMemorySnapshot {
        &self.rsp_memory
    }

    pub const fn machine_state(&self) -> &RspMachineState {
        &self.machine_state
    }

    pub const fn pc_low12(&self) -> u32 {
        self.pc_low12
    }

    pub fn dpc_submissions(&self) -> &[DeferredDpcSubmission] {
        &self.dpc_submissions
    }

    /// Complete ucode-phase IMEM images, in DMA installation order.
    pub fn imem_replacements(&self) -> &[AudioImemReplacement] {
        &self.imem_replacements
    }

    pub fn into_rdram_storage(self) -> Vec<u8> {
        self.rdram_storage
    }
}

/// Build the complete comparison outcome without consuming or mutating the
/// speculative result.
///
/// PCM ownership is an explicit caller policy because the authoritative AI
/// buffer boundary is later than this RSP task. An empty policy is therefore
/// valid; a nonempty policy must still be covered by the lane's exact RDRAM
/// patches.
pub fn speculative_audio_lle_outcome(
    result: &SpeculativeAudioLleResult,
    admission: AdmittedAudioMicrocode,
    pcm_ranges: CanonicalRdramRanges,
) -> Result<AudioTaskOutcome, AudioLleOutcomeError> {
    if admission.identity() != result.identity() {
        return Err(AudioLleOutcomeError::AdmissionIdentityMismatch {
            admitted: Box::new(admission.identity()),
            result: Box::new(result.identity()),
        });
    }

    let memory = result.rsp_memory();
    let architectural = result.machine_state().architectural_state();
    let rsp = RspVisibleState::new(
        *memory.bank(RspMemoryBank::Dmem),
        *memory.bank(RspMemoryBank::Imem),
        memory.imem_generation(),
        result.pc_low12() & 0x0fff,
        architectural.sp_status(),
        architectural.sp_semaphore(),
        RspDmaRegisterState {
            mem_address: architectural.dma_mem_address(),
            dram_address: architectural.dma_dram_address(),
            read_length: architectural.dma_read_length(),
            write_length: architectural.dma_write_length(),
        },
        RspDpcRegisterState {
            start: architectural.dp_start(),
            end: architectural.dp_end(),
            current: architectural.dp_current(),
            status: architectural.dp_status(),
            clock: architectural.dp_clock(),
            command_busy: architectural.dp_busy(),
            pipe_busy: architectural.dp_pipe_busy(),
            tmem_busy: architectural.dp_tmem_busy(),
        },
        result.dpc_submissions().to_vec(),
    )
    .map_err(AudioLleOutcomeError::RspVisibleState)?;

    AudioTaskOutcome::new(
        admission.selection(),
        result.terminal(),
        result.rdram_patches().clone(),
        pcm_ranges,
        rsp,
        result.ucode_steps(),
    )
    .map_err(AudioLleOutcomeError::Outcome)
}

/// Execute the loaded audio ucode to BREAK without touching live runtime
/// state.
pub fn run_speculative_audio_lle(
    parts: AudioLleLaneParts,
) -> Result<SpeculativeAudioLleResult, SpeculativeAudioLleError> {
    run_speculative_audio_lle_with_limits(parts, MAX_UCODE_STEPS, INTERPRETER_CHUNK_STEPS)
}

fn run_speculative_audio_lle_with_limits(
    parts: AudioLleLaneParts,
    maximum_steps: u64,
    chunk_steps: u64,
) -> Result<SpeculativeAudioLleResult, SpeculativeAudioLleError> {
    assert!(
        maximum_steps > 0 && chunk_steps > 0,
        "speculative LLE limits must be nonzero"
    );
    validate_lane_parts(&parts)?;

    let execution = parts.into_execution_parts();
    let _task_addr = execution.task_addr;
    let _loaded_header = execution.loaded_header;
    let _entry_header = execution.entry_header;
    let _command_bytes = execution.command_bytes;
    let _ucode_data_bytes = execution.ucode_data_bytes;
    let identity = execution.identity;
    let mut rdram_storage = execution.rdram_storage;
    let rsp_memory = execution.rsp_memory;
    let machine_state = execution.machine_state;
    let entry_pc_low12 = execution.entry_pc_low12;
    let rspboot_steps = execution.rspboot_steps;
    let admitted_dma_ranges = execution.admitted_dma_ranges;

    let rspboot_steps =
        NonZeroU64::new(rspboot_steps).ok_or(SpeculativeAudioLleError::ZeroRspbootSteps)?;
    let mut persistent_memory = RspMemory::from_snapshot(rsp_memory);
    let mut imem = *persistent_memory.bank(RspMemoryBank::Imem);
    let mut machine = RspMachine::new(&mut rdram_storage);
    machine.set_dma_rdram_ranges(admitted_dma_ranges);
    machine.load_dmem_logical(persistent_memory.bank(RspMemoryBank::Dmem));
    machine.restore_state(machine_state);

    let mut pc = entry_pc_low12;
    let mut ucode_steps = 0u64;
    let initial_imem_generation = persistent_memory.imem_generation();
    let mut imem_replacements = Vec::new();
    loop {
        let remaining = maximum_steps
            .checked_sub(ucode_steps)
            .expect("LLE step accounting cannot exceed its checked bound");
        if remaining == 0 {
            return Err(SpeculativeAudioLleError::StepBoundExceeded {
                maximum_steps,
                pc: pc & 0x0fff,
            });
        }
        let words = logical_imem_words(&imem);
        let result = run_imem(&words, pc, &mut machine, remaining.min(chunk_steps));
        ucode_steps = ucode_steps
            .checked_add(result.steps)
            .expect("LLE ucode step counter overflow");
        pc = result.pc & 0x0fff;

        match result.reason {
            RspExitReason::Broke => break,
            RspExitReason::SwapOverlay => {
                machine.complete_imem_dma(&mut imem);
                persistent_memory
                    .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &imem)
                    .expect("complete IMEM bank replacement is always in range");
                imem_replacements.push(AudioImemReplacement::from_image(
                    persistent_memory.imem_generation(),
                    imem,
                ));
            }
            RspExitReason::StepLimit if ucode_steps < maximum_steps => {}
            RspExitReason::StepLimit => {
                return Err(SpeculativeAudioLleError::StepBoundExceeded { maximum_steps, pc });
            }
            reason => {
                return Err(SpeculativeAudioLleError::NonBreakExit {
                    reason,
                    pc,
                    ucode_steps,
                });
            }
        }
    }

    let ucode_steps =
        NonZeroU64::new(ucode_steps).expect("BREAK execution consumes at least one instruction");
    let final_dmem = machine.dmem_logical();
    let dpc_submissions = machine
        .take_dp_submissions()
        .into_iter()
        .map(canonicalize_dpc_submission)
        .collect::<Result<Vec<_>, _>>()?;
    let machine_state = machine.snapshot_state();
    let storage_write_ranges = machine.take_rdram_writes();
    let dma_journal = machine.take_dma_journal();
    drop(machine);

    persistent_memory
        .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Dmem, 0), &final_dmem)
        .expect("complete DMEM bank replacement is always in range");
    let rsp_memory = persistent_memory.snapshot();
    assert_eq!(
        rsp_memory.imem_generation(),
        initial_imem_generation
            .checked_add(imem_replacements.len() as u64)
            .expect("ucode IMEM generation overflow"),
        "ucode replacement count diverged from owned RSP-memory generation"
    );
    let (rdram_write_ranges, rdram_patches) =
        collect_logical_rdram_effects(&rdram_storage, storage_write_ranges)?;

    Ok(SpeculativeAudioLleResult {
        identity,
        terminal: AudioTaskTerminalReason::Broke,
        effects: SpeculativeAudioLleEffects {
            rdram_storage,
            rdram_write_ranges,
            rdram_patches,
            rsp_memory,
            machine_state,
            pc_low12: pc,
            dpc_submissions,
            imem_replacements,
        },
        rspboot_steps,
        ucode_steps,
        dma_journal,
    })
}

fn validate_lane_parts(parts: &AudioLleLaneParts) -> Result<(), SpeculativeAudioLleError> {
    if parts.rdram_storage().len() != DEFAULT_RDRAM_SIZE {
        return Err(SpeculativeAudioLleError::PhysicalRdramStorageLength {
            storage_bytes: parts.rdram_storage().len(),
            required_bytes: DEFAULT_RDRAM_SIZE,
        });
    }
    if !parts.entry_pc_low12().is_multiple_of(4) {
        return Err(SpeculativeAudioLleError::EntryPcUnaligned {
            pc: parts.entry_pc_low12(),
        });
    }
    if parts.entry_pc_low12() > 0x0ffc {
        return Err(SpeculativeAudioLleError::EntryPcOutsideFabricRange {
            pc: parts.entry_pc_low12(),
        });
    }
    if parts.rspboot_steps() == 0 {
        return Err(SpeculativeAudioLleError::ZeroRspbootSteps);
    }
    if parts.machine_state().diagnostic_steps() != parts.rspboot_steps() {
        return Err(SpeculativeAudioLleError::RspbootStepAccountingMismatch {
            rspboot_steps: parts.rspboot_steps(),
            diagnostic_steps: parts.machine_state().diagnostic_steps(),
        });
    }
    let dpc_submission_count = parts
        .machine_state()
        .architectural_state()
        .dp_submissions()
        .len();
    if dpc_submission_count != 0 {
        return Err(SpeculativeAudioLleError::PreexistingDpcSubmissions {
            count: dpc_submission_count,
        });
    }
    if parts.admitted_dma_ranges().is_empty() {
        return Err(SpeculativeAudioLleError::NoAdmittedDmaRanges);
    }
    for range in parts.admitted_dma_ranges() {
        if range.start >= range.end || range.end > DEFAULT_RDRAM_SIZE {
            return Err(SpeculativeAudioLleError::InvalidAdmittedDmaRange {
                start: range.start,
                end: range.end,
                physical_bytes: DEFAULT_RDRAM_SIZE,
            });
        }
    }
    Ok(())
}

fn logical_imem_words(imem: &[u8; DMEM_SIZE]) -> Vec<u32> {
    imem.chunks_exact(4)
        .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four IMEM bytes")))
        .collect()
}

fn canonicalize_dpc_submission(
    raw: RspDpSubmission,
) -> Result<DeferredDpcSubmission, SpeculativeAudioLleError> {
    let (start, end, source) = raw.into_parts();
    match source {
        RspDpCommandSource::XbusBytes(payload) => {
            DeferredDpcSubmission::from_dmem_payload(start, end, payload)
                .map_err(SpeculativeAudioLleError::DeferredDpcSubmission)
        }
        RspDpCommandSource::RdramWords(words) => {
            DeferredDpcSubmission::from_rdram_words(start, end, words)
                .map_err(SpeculativeAudioLleError::DeferredDpcSubmission)
        }
    }
}

fn collect_logical_rdram_effects(
    rdram_storage: &[u8],
    storage_ranges: Vec<(usize, usize)>,
) -> Result<(Vec<RdramByteRange>, CanonicalRdramPatches), SpeculativeAudioLleError> {
    let view = RdramView::from_storage(rdram_storage);
    let mut written_ranges = Vec::with_capacity(storage_ranges.len());
    let mut patches = Vec::with_capacity(storage_ranges.len());
    for (start, end) in storage_ranges {
        let byte_len = end
            .checked_sub(start)
            .ok_or(SpeculativeAudioLleError::RdramWriteRange { start, end })?;
        let start_u32 = u32::try_from(start)
            .map_err(|_| SpeculativeAudioLleError::RdramWriteRange { start, end })?;
        let byte_len_u32 = u32::try_from(byte_len)
            .map_err(|_| SpeculativeAudioLleError::RdramWriteRange { start, end })?;
        let range = RdramByteRange::new(start_u32, byte_len_u32)
            .map_err(|_| SpeculativeAudioLleError::RdramWriteRange { start, end })?;
        let bytes = (range.start()..range.end())
            .map(|offset| view.read_u8(RdramAddr::from_offset(offset)))
            .collect();
        patches.push(
            RdramPatch::new(range.start(), bytes).map_err(SpeculativeAudioLleError::RdramPatch)?,
        );
        written_ranges.push(range);
    }
    let patches = CanonicalRdramPatches::new(patches)
        .map_err(SpeculativeAudioLleError::CanonicalRdramPatches)?;
    Ok((written_ranges, patches))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_runtime::{OsTaskHeader, RdramViewMut, M_AUDTASK};

    use crate::hle::{AudioHleCatalog, AudioHleCatalogEntry};
    use crate::hle_commit::UcodePhaseCommitCandidate;
    use crate::hle_outcome::{AudioHleFamily, DpcSubmissionSource, RdramRangeError, Sha256Digest};
    use crate::hle_snapshot::{AudioTaskEntrySnapshot, PostRspbootAudioTaskParts};

    const BREAK: u32 = 0x0000_000d;

    fn addiu(rt: u8, rs: u8, immediate: u16) -> u32 {
        (0x09 << 26) | (u32::from(rs) << 21) | (u32::from(rt) << 16) | u32::from(immediate)
    }

    fn mtc0(rt: u8, cop0: u8) -> u32 {
        (0x10 << 26) | (0x04 << 21) | (u32::from(rt) << 16) | (u32::from(cop0) << 11)
    }

    fn lane_parts(
        words: &[u32],
        dmem: [u8; DMEM_SIZE],
        rdram_storage: Vec<u8>,
    ) -> AudioLleLaneParts {
        lane_parts_with_machine(words, dmem, rdram_storage, |_| {})
    }

    fn lane_parts_with_machine(
        words: &[u32],
        mut dmem: [u8; DMEM_SIZE],
        rdram_storage: Vec<u8>,
        configure_machine: impl FnOnce(&mut RspMachine<'_>),
    ) -> AudioLleLaneParts {
        let mut imem = [0u8; DMEM_SIZE];
        for (slot, word) in imem.chunks_exact_mut(4).zip(words.iter().copied()) {
            slot.copy_from_slice(&word.to_be_bytes());
        }

        let header = OsTaskHeader {
            task_type: M_AUDTASK,
            ..OsTaskHeader::default()
        };
        let header_words = [
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
        for (slot, word) in dmem[DMEM_SIZE - 64..].chunks_exact_mut(4).zip(header_words) {
            slot.copy_from_slice(&word.to_be_bytes());
        }

        let mut memory = RspMemory::new();
        memory
            .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Dmem, 0), &dmem)
            .unwrap();
        memory
            .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &imem)
            .unwrap();

        let mut machine_storage = vec![0; DEFAULT_RDRAM_SIZE];
        let mut machine = RspMachine::new(&mut machine_storage);
        machine.ctx.steps = 3;
        configure_machine(&mut machine);
        let machine_state = machine.snapshot_state();
        drop(machine);

        AudioTaskEntrySnapshot::from_post_rspboot(PostRspbootAudioTaskParts {
            task_addr: RdramAddr::from_offset(0),
            loaded_header: header,
            entry_header: header,
            command_bytes: Vec::new(),
            ucode_data_bytes: Vec::new(),
            rdram_storage,
            rsp_memory: memory.snapshot(),
            machine_state,
            entry_pc_low12: 0,
            rspboot_steps: 3,
            admitted_dma_ranges: std::iter::once(0..DEFAULT_RDRAM_SIZE).collect(),
        })
        .unwrap()
        .fork_lle_lane()
        .into_lle_parts()
    }

    fn admit(
        identity: AudioMicrocodeIdentity,
        implementation_revision: u32,
    ) -> AdmittedAudioMicrocode {
        let entries = [AudioHleCatalogEntry {
            identity,
            family: AudioHleFamily::StandardAbi,
            implementation_revision,
        }];
        AudioHleCatalog::new(&entries)
            .unwrap()
            .admit(identity)
            .unwrap()
    }

    #[test]
    fn break_lane_is_owned_and_does_not_mutate_its_source_bytes() {
        let source = vec![0x5a; DEFAULT_RDRAM_SIZE];
        let parts = lane_parts(&[BREAK], [0; DMEM_SIZE], source.clone());
        let result = run_speculative_audio_lle(parts).unwrap();

        assert_eq!(source, vec![0x5a; DEFAULT_RDRAM_SIZE]);
        assert_eq!(result.rdram_storage(), source);
        assert!(result.rdram_write_ranges().is_empty());
        assert_eq!(result.ucode_steps().get(), 1);
        assert_eq!(result.rspboot_steps().get(), 3);
        assert_eq!(result.terminal(), AudioTaskTerminalReason::Broke);
        assert_eq!(result.pc_low12(), 0);
        assert!(result.imem_replacements().is_empty());
        let mut imem = [0; DMEM_SIZE];
        imem[0..4].copy_from_slice(&BREAK.to_be_bytes());
        assert_eq!(
            result.identity(),
            AudioMicrocodeIdentity::from_task_entry(&imem, &[]).unwrap()
        );
        let effects = result.into_effects();
        assert_eq!(effects.rdram_storage(), source);
        assert!(effects.dpc_submissions().is_empty());
        assert!(effects.imem_replacements().is_empty());
    }

    #[test]
    fn deterministic_step_bound_is_a_typed_loud_failure() {
        let parts = lane_parts(&[0; 4], [0; DMEM_SIZE], vec![0; DEFAULT_RDRAM_SIZE]);

        assert!(matches!(
            run_speculative_audio_lle_with_limits(parts, 3, 2),
            Err(SpeculativeAudioLleError::StepBoundExceeded {
                maximum_steps: 3,
                pc: 12,
            })
        ));
    }

    #[test]
    fn non_break_interpreter_exit_is_a_typed_loud_failure() {
        let parts = lane_parts(&[0xffff_ffff], [0; DMEM_SIZE], vec![0; DEFAULT_RDRAM_SIZE]);

        assert!(matches!(
            run_speculative_audio_lle_with_limits(parts, 3, 2),
            Err(SpeculativeAudioLleError::NonBreakExit {
                reason: RspExitReason::Unsupported,
                pc: 0,
                ucode_steps: 1,
            })
        ));
    }

    #[test]
    fn dma_write_retains_same_value_write_coverage_and_logical_patch_bytes() {
        let program = [
            addiu(2, 0, 0x100),
            mtc0(2, 0),
            addiu(3, 0, 0x200),
            mtc0(3, 1),
            addiu(4, 0, 7),
            mtc0(4, 3),
            BREAK,
        ];
        let mut dmem = [0u8; DMEM_SIZE];
        dmem[0x100..0x108].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut storage = vec![0; DEFAULT_RDRAM_SIZE];
        RdramViewMut::from_storage(&mut storage)
            .write_logical_bytes(RdramAddr::from_offset(0x200), &[1, 2, 3, 4, 5, 6, 7, 8]);

        let result = run_speculative_audio_lle(lane_parts(&program, dmem, storage)).unwrap();

        assert_eq!(
            result.rdram_write_ranges(),
            &[RdramByteRange::new(0x200, 8).unwrap()]
        );
        assert_eq!(result.rdram_patches().as_slice().len(), 1);
        assert_eq!(
            result.rdram_patches().as_slice()[0].bytes(),
            &[1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn imem_overlay_increments_generation_once_and_resumes_to_break() {
        let program = [
            addiu(2, 0, 0x1000),
            mtc0(2, 0),
            addiu(3, 0, 0x300),
            mtc0(3, 1),
            addiu(4, 0, 7),
            mtc0(4, 2),
            BREAK,
        ];
        let storage = vec![0; DEFAULT_RDRAM_SIZE];
        let entry = lane_parts(&program, [0; DMEM_SIZE], storage);
        let entry_generation = entry
            .clone()
            .into_execution_parts()
            .rsp_memory
            .imem_generation();

        let result = run_speculative_audio_lle(entry).unwrap();

        assert_eq!(result.rsp_memory().imem_generation(), entry_generation + 1);
        assert_eq!(result.imem_replacements().len(), 1);
        let replacement = &result.imem_replacements()[0];
        assert_eq!(replacement.generation(), entry_generation + 1);
        assert_eq!(
            replacement.identity(),
            Sha256Digest::hash(replacement.image())
        );
        assert_eq!(
            replacement.image(),
            result.rsp_memory().bank(RspMemoryBank::Imem)
        );
        assert_eq!(result.ucode_steps().get(), 7);
        assert_eq!(
            &result.rsp_memory().bank(RspMemoryBank::Imem)[0..8],
            &[0; 8]
        );
    }

    #[test]
    fn multiple_imem_replacements_retain_dma_order_and_contiguous_generations() {
        let program = [
            addiu(2, 0, 0x1000),
            mtc0(2, 0),
            addiu(3, 0, 0x300),
            mtc0(3, 1),
            addiu(4, 0, 7),
            mtc0(4, 2),
            addiu(2, 0, 0x1008),
            mtc0(2, 0),
            addiu(3, 0, 0x308),
            mtc0(3, 1),
            addiu(4, 0, 7),
            mtc0(4, 2),
            BREAK,
        ];
        let entry = lane_parts(&program, [0; DMEM_SIZE], vec![0; DEFAULT_RDRAM_SIZE]);
        let initial_memory = entry.clone().into_execution_parts().rsp_memory;
        let initial_generation = initial_memory.imem_generation();
        let initial_image = *initial_memory.bank(RspMemoryBank::Imem);

        let result = run_speculative_audio_lle(entry).unwrap();
        let replacements = result.imem_replacements();

        assert_eq!(replacements.len(), 2);
        assert_eq!(replacements[0].generation(), initial_generation + 1);
        assert_eq!(replacements[1].generation(), initial_generation + 2);
        assert_eq!(
            replacements
                .iter()
                .map(AudioImemReplacement::generation)
                .collect::<Vec<_>>(),
            [initial_generation + 1, initial_generation + 2]
        );
        assert_eq!(&replacements[0].image()[0..8], &[0; 8]);
        assert_eq!(
            &replacements[0].image()[8..16],
            &initial_image[8..16],
            "the first DMA must be recorded before the second image is installed"
        );
        assert_eq!(&replacements[1].image()[0..16], &[0; 16]);
        assert_eq!(
            replacements[1].image(),
            result.rsp_memory().bank(RspMemoryBank::Imem)
        );
        for replacement in replacements {
            assert_eq!(
                replacement.identity(),
                Sha256Digest::hash(replacement.image())
            );
        }
        assert_eq!(
            result.rsp_memory().imem_generation(),
            initial_generation + replacements.len() as u64
        );
    }

    #[test]
    fn imem_replacement_journal_is_separate_from_architectural_machine_state() {
        let program = [
            addiu(2, 0, 0x1000),
            mtc0(2, 0),
            addiu(3, 0, 0x300),
            mtc0(3, 1),
            addiu(4, 0, 7),
            mtc0(4, 2),
            BREAK,
        ];
        let result = run_speculative_audio_lle(lane_parts(
            &program,
            [0; DMEM_SIZE],
            vec![0; DEFAULT_RDRAM_SIZE],
        ))
        .unwrap();
        let machine_state = result.machine_state().clone();
        let replacements = result.imem_replacements().to_vec();

        let effects = result.into_effects();

        assert_eq!(effects.machine_state(), &machine_state);
        assert_eq!(effects.imem_replacements(), replacements);
        assert!(
            effects
                .machine_state()
                .architectural_state()
                .dp_submissions()
                .is_empty(),
            "effect journals must not be smuggled through architectural submission state"
        );
    }

    #[test]
    fn dpc_submissions_retain_ordered_xbus_payload_and_words() {
        let program = [
            addiu(2, 0, 2),
            mtc0(2, 11),
            addiu(3, 0, 0x100),
            mtc0(3, 8),
            addiu(4, 0, 0x108),
            mtc0(4, 9),
            BREAK,
        ];
        let mut dmem = [0u8; DMEM_SIZE];
        dmem[0x100..0x108].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let result =
            run_speculative_audio_lle(lane_parts(&program, dmem, vec![0; DEFAULT_RDRAM_SIZE]))
                .unwrap();

        assert_eq!(result.dpc_submissions().len(), 1);
        assert_eq!(result.dpc_submissions()[0].start(), 0x100);
        assert_eq!(result.dpc_submissions()[0].end(), 0x108);
        assert_eq!(
            result.dpc_submissions()[0].source(),
            DpcSubmissionSource::Dmem
        );
        assert_eq!(
            result.dpc_submissions()[0].xbus_payload(),
            Some([1, 2, 3, 4, 5, 6, 7, 8].as_slice())
        );
        assert_eq!(
            result.dpc_submissions()[0].command_words(),
            [0x0102_0304, 0x0506_0708]
        );
        assert!(
            result
                .machine_state()
                .architectural_state()
                .dp_submissions()
                .is_empty(),
            "deferred DPC work must have exactly one owner after lane execution"
        );
    }

    #[test]
    fn raw_xbus_submission_has_only_one_owned_command_representation() {
        let raw = RspDpSubmission::from_xbus_bytes(0x100, 0x108, vec![1, 2, 3, 4, 5, 6, 7, 8]);

        let deferred = canonicalize_dpc_submission(raw).unwrap();
        assert_eq!(deferred.source(), DpcSubmissionSource::Dmem);
        assert_eq!(deferred.command_words(), [0x0102_0304, 0x0506_0708]);
    }

    #[test]
    fn raw_rdram_submission_retains_captured_canonical_words() {
        let raw = RspDpSubmission::from_rdram_words(0x200, 0x208, vec![0x1122_3344, 0xaabb_ccdd]);

        let deferred = canonicalize_dpc_submission(raw).unwrap();
        assert_eq!(deferred.source(), DpcSubmissionSource::Rdram);
        assert_eq!(deferred.command_words(), [0x1122_3344, 0xaabb_ccdd]);
        assert_eq!(deferred.xbus_payload(), None);
    }

    #[test]
    fn preexisting_dpc_submission_is_a_loud_snapshot_frontier() {
        let parts = lane_parts_with_machine(
            &[BREAK],
            [0; DMEM_SIZE],
            vec![0; DEFAULT_RDRAM_SIZE],
            |machine| {
                machine.write_cp0(8, 0x100);
                machine.write_cp0(9, 0x108);
            },
        );

        assert!(matches!(
            run_speculative_audio_lle(parts),
            Err(SpeculativeAudioLleError::PreexistingDpcSubmissions { count: 1 })
        ));
    }

    #[test]
    fn cloned_lanes_do_not_share_rdram_or_rsp_memory() {
        let program = [
            addiu(2, 0, 0x100),
            mtc0(2, 0),
            addiu(3, 0, 0x200),
            mtc0(3, 1),
            addiu(4, 0, 7),
            mtc0(4, 3),
            BREAK,
        ];
        let mut dmem = [0u8; DMEM_SIZE];
        dmem[0x100..0x108].copy_from_slice(&[9; 8]);
        let lane = lane_parts(&program, dmem, vec![0; DEFAULT_RDRAM_SIZE]);

        let first = run_speculative_audio_lle(lane.clone()).unwrap();
        let second = run_speculative_audio_lle(lane).unwrap();

        assert_eq!(first.rdram_storage(), second.rdram_storage());
        assert_eq!(first.rsp_memory(), second.rsp_memory());
        assert_eq!(first.machine_state(), second.machine_state());
    }

    #[test]
    fn outcome_bridge_maps_every_visible_field_and_keeps_ucode_step_accounting() {
        let program = [
            addiu(2, 0, 0x100),
            mtc0(2, 0),
            addiu(3, 0, 0x200),
            mtc0(3, 1),
            addiu(4, 0, 7),
            mtc0(4, 3),
            addiu(5, 0, 2),
            mtc0(5, 11),
            addiu(6, 0, 0x100),
            mtc0(6, 8),
            addiu(7, 0, 0x108),
            mtc0(7, 9),
            BREAK,
        ];
        let mut dmem = [0u8; DMEM_SIZE];
        dmem[0x100..0x108].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let parts =
            lane_parts_with_machine(&program, dmem, vec![0; DEFAULT_RDRAM_SIZE], |machine| {
                machine.set_sp_status_raw(0x41);
                assert_eq!(machine.read_cp0(7), 0);
            });
        let result = run_speculative_audio_lle(parts).unwrap();
        let outcome = speculative_audio_lle_outcome(
            &result,
            admit(result.identity(), 17),
            CanonicalRdramRanges::default(),
        )
        .unwrap();

        assert_eq!(outcome.selection().microcode, result.identity());
        assert_eq!(outcome.selection().family, AudioHleFamily::StandardAbi);
        assert_eq!(outcome.selection().implementation_revision, 17);
        assert_eq!(outcome.terminal(), result.terminal());
        assert_eq!(outcome.rdram_patches(), result.rdram_patches());
        assert!(outcome.pcm_ranges().as_slice().is_empty());
        assert_eq!(outcome.completion_steps(), result.ucode_steps());

        let memory = result.rsp_memory();
        let architectural = result.machine_state().architectural_state();
        let rsp = outcome.rsp();
        assert_eq!(rsp.dmem, *memory.bank(RspMemoryBank::Dmem));
        assert_eq!(rsp.imem, *memory.bank(RspMemoryBank::Imem));
        assert_eq!(rsp.imem_generation, memory.imem_generation());
        assert_eq!(rsp.sp_pc(), result.pc_low12() & 0x0fff);
        assert_eq!(rsp.sp_status, architectural.sp_status());
        assert_eq!(rsp.sp_semaphore, architectural.sp_semaphore());
        assert_eq!(rsp.dma.mem_address, architectural.dma_mem_address());
        assert_eq!(rsp.dma.dram_address, architectural.dma_dram_address());
        assert_eq!(rsp.dma.read_length, architectural.dma_read_length());
        assert_eq!(rsp.dma.write_length, architectural.dma_write_length());
        assert_eq!(rsp.dpc.start, architectural.dp_start());
        assert_eq!(rsp.dpc.end, architectural.dp_end());
        assert_eq!(rsp.dpc.current, architectural.dp_current());
        assert_eq!(rsp.dpc.status, architectural.dp_status());
        assert_eq!(rsp.dpc.clock, architectural.dp_clock());
        assert_eq!(rsp.dpc.command_busy, architectural.dp_busy());
        assert_eq!(rsp.dpc.pipe_busy, architectural.dp_pipe_busy());
        assert_eq!(rsp.dpc.tmem_busy, architectural.dp_tmem_busy());
        assert_eq!(rsp.dpc_submissions, result.dpc_submissions());
        assert_eq!(rsp.dpc_submissions.len(), 1);
    }

    #[test]
    fn outcome_bridge_rejects_admission_for_another_exact_identity() {
        let result = run_speculative_audio_lle(lane_parts(
            &[BREAK],
            [0; DMEM_SIZE],
            vec![0; DEFAULT_RDRAM_SIZE],
        ))
        .unwrap();
        let other_identity = AudioMicrocodeIdentity {
            imem_sha256: Sha256Digest::new([0xa5; 32]),
            ucode_data_bytes: 9,
            ucode_data_sha256: Sha256Digest::new([0x5a; 32]),
        };

        assert_eq!(
            speculative_audio_lle_outcome(
                &result,
                admit(other_identity, 1),
                CanonicalRdramRanges::default(),
            ),
            Err(AudioLleOutcomeError::AdmissionIdentityMismatch {
                admitted: Box::new(other_identity),
                result: Box::new(result.identity()),
            })
        );
    }

    #[test]
    fn outcome_bridge_preserves_visible_state_validation() {
        let mut result = run_speculative_audio_lle(lane_parts(
            &[BREAK],
            [0; DMEM_SIZE],
            vec![0; DEFAULT_RDRAM_SIZE],
        ))
        .unwrap();
        result.effects.pc_low12 = 2;

        assert_eq!(
            speculative_audio_lle_outcome(
                &result,
                admit(result.identity(), 1),
                CanonicalRdramRanges::default(),
            ),
            Err(AudioLleOutcomeError::RspVisibleState(
                RspVisibleStateError::InvalidPc(2)
            ))
        );
    }

    #[test]
    fn outcome_bridge_requires_every_declared_pcm_range_to_be_written() {
        let result = run_speculative_audio_lle(lane_parts(
            &[BREAK],
            [0; DMEM_SIZE],
            vec![0; DEFAULT_RDRAM_SIZE],
        ))
        .unwrap();
        let pcm_range = RdramByteRange::new(0x100, 8).unwrap();
        let pcm_ranges = CanonicalRdramRanges::new(vec![pcm_range]).unwrap();

        assert_eq!(
            speculative_audio_lle_outcome(&result, admit(result.identity(), 1), pcm_ranges),
            Err(AudioLleOutcomeError::Outcome(
                AudioTaskOutcomeError::PcmRangeNotWritten { range: pcm_range }
            ))
        );

        assert_eq!(RdramByteRange::new(0x100, 0), Err(RdramRangeError::Empty));
    }

    #[test]
    fn borrowed_outcome_remains_compatible_with_exact_consuming_commit_candidate() {
        let program = [
            addiu(2, 0, 0x100),
            mtc0(2, 0),
            addiu(3, 0, 0x200),
            mtc0(3, 1),
            addiu(4, 0, 7),
            mtc0(4, 3),
            BREAK,
        ];
        let mut dmem = [0u8; DMEM_SIZE];
        dmem[0x100..0x108].copy_from_slice(&[9; 8]);
        let result =
            run_speculative_audio_lle(lane_parts(&program, dmem, vec![0; DEFAULT_RDRAM_SIZE]))
                .unwrap();
        let pcm_range = RdramByteRange::new(0x200, 8).unwrap();
        let outcome = speculative_audio_lle_outcome(
            &result,
            admit(result.identity(), 3),
            CanonicalRdramRanges::new(vec![pcm_range]).unwrap(),
        )
        .unwrap();

        let candidate = UcodePhaseCommitCandidate::from_lle_result(outcome.clone(), result)
            .expect("bridge outcome and consuming commit candidate must agree exactly");
        assert_eq!(candidate.outcome(), &outcome);
    }
}
