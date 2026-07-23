//! One-time commit authority for exactly matched speculative audio effects.
//!
//! The speculative runner owns no live runtime handle. This module preserves
//! that property while making the transition from evidence to mutation
//! explicit: a lane result is first checked against its modeled outcome, then
//! that outcome must compare exactly with the reference outcome. Only the
//! resulting non-cloneable token can yield a non-cloneable commit payload.
//!
//! This is intentionally a *ucode-phase* commit. The current post-rspboot
//! snapshot starts after rspboot has already run, so it cannot carry rspboot's
//! earlier RDRAM patches or pre-entry device effects. A whole-task commit must
//! add those boot effects to a separate owned value before this seam may be
//! used by the live dispatcher; this module does not invent them.

use core::num::NonZeroU64;

use fn64_runtime::rsp::RspMemorySnapshot;
use fn64_runtime::RspMemoryBank;

use crate::hle_lle::{SpeculativeAudioLleEffects, SpeculativeAudioLleResult};
use crate::hle_outcome::{
    compare_audio_task_outcomes, AudioHleSelection, AudioMicrocodeIdentity, AudioTaskOutcome,
    AudioTaskOutcomeMismatch, AudioTaskTerminalReason, CanonicalRdramPatches, CanonicalRdramRanges,
    DeferredDpcSubmission, RspVisibleState,
};
use crate::rsp::runtime::RspMachineState;

/// Exact instruction accounting retained for the eventual timing adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioTaskStepTotals {
    rspboot: NonZeroU64,
    ucode: NonZeroU64,
    whole_task: NonZeroU64,
}

impl AudioTaskStepTotals {
    fn new(rspboot: NonZeroU64, ucode: NonZeroU64) -> Result<Self, PrepareUcodeCommitError> {
        let whole_task = rspboot
            .get()
            .checked_add(ucode.get())
            .and_then(NonZeroU64::new)
            .ok_or(PrepareUcodeCommitError::StepTotalOverflow { rspboot, ucode })?;
        Ok(Self {
            rspboot,
            ucode,
            whole_task,
        })
    }

    pub const fn rspboot(self) -> NonZeroU64 {
        self.rspboot
    }

    pub const fn ucode(self) -> NonZeroU64 {
        self.ucode
    }

    pub const fn whole_task(self) -> NonZeroU64 {
        self.whole_task
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrepareUcodeCommitError {
    StepTotalOverflow {
        rspboot: NonZeroU64,
        ucode: NonZeroU64,
    },
    MicrocodeIdentity {
        outcome: Box<AudioMicrocodeIdentity>,
        effects: Box<AudioMicrocodeIdentity>,
    },
    TerminalReason {
        outcome: AudioTaskTerminalReason,
        effects: AudioTaskTerminalReason,
    },
    RdramPatches,
    RspMemory,
    SpPc {
        outcome: u32,
        effects: u32,
    },
    SpStatus {
        outcome: u32,
        effects: u32,
    },
    SpSemaphore {
        outcome: bool,
        effects: bool,
    },
    DmaRegisters,
    DpcRegisters,
    DeferredDpcSubmissions,
    MachineRetainsDpcSubmissions {
        count: usize,
    },
    CompletionSteps {
        outcome: NonZeroU64,
        effects: NonZeroU64,
    },
    MachineDiagnosticSteps {
        machine: u64,
        effects: NonZeroU64,
    },
}

/// A self-consistent LLE result paired with its complete comparison outcome.
///
/// Construction consumes the speculative result. Callers cannot substitute a
/// second effect image after its outcome has been checked.
#[derive(Debug)]
pub struct UcodePhaseCommitCandidate {
    outcome: AudioTaskOutcome,
    effects: CommitReadyUcodeEffects,
}

impl UcodePhaseCommitCandidate {
    pub fn from_lle_result(
        outcome: AudioTaskOutcome,
        result: SpeculativeAudioLleResult,
    ) -> Result<Self, PrepareUcodeCommitError> {
        let identity = result.identity();
        let terminal = result.terminal();
        let steps = AudioTaskStepTotals::new(result.rspboot_steps(), result.ucode_steps())?;
        let effects = result.into_effects();
        validate_effects(&outcome, identity, terminal, steps, &effects)?;
        Ok(Self {
            outcome,
            effects: CommitReadyUcodeEffects::from_speculative(effects, steps),
        })
    }

    pub const fn outcome(&self) -> &AudioTaskOutcome {
        &self.outcome
    }
}

/// Non-forgeable proof, within this module, that both complete outcomes match.
///
/// This type intentionally implements neither `Clone` nor `Copy`. Consuming it
/// is the only public route to a commit payload.
///
/// ```compile_fail
/// use fn64_audio::hle_commit::VerifiedUcodePhaseCommitToken;
///
/// fn duplicate(token: VerifiedUcodePhaseCommitToken) {
///     let _second_authority = token.clone();
/// }
/// ```
#[derive(Debug)]
pub struct VerifiedUcodePhaseCommitToken {
    candidate: UcodePhaseCommitCandidate,
    _verified: ExactOutcomeSeal,
}

#[derive(Debug)]
struct ExactOutcomeSeal;

/// Compare in the stable first-divergence order and mint one commit authority.
pub fn verify_ucode_phase_commit(
    reference: &AudioTaskOutcome,
    candidate: UcodePhaseCommitCandidate,
) -> Result<VerifiedUcodePhaseCommitToken, AudioTaskOutcomeMismatch> {
    compare_audio_task_outcomes(reference, &candidate.outcome)?;
    Ok(VerifiedUcodePhaseCommitToken {
        candidate,
        _verified: ExactOutcomeSeal,
    })
}

impl VerifiedUcodePhaseCommitToken {
    /// Consume the comparison proof and expose the owned ucode-phase effects.
    pub fn into_commit(self) -> VerifiedUcodePhaseCommit {
        let UcodePhaseCommitCandidate { outcome, effects } = self.candidate;
        VerifiedUcodePhaseCommit {
            selection: outcome.selection(),
            terminal: outcome.terminal(),
            rdram_patches: effects.rdram_patches,
            pcm_ranges: outcome.pcm_ranges().clone(),
            rsp_memory: effects.rsp_memory,
            machine_state: effects.machine_state,
            pc_low12: effects.pc_low12,
            dpc_submissions: effects.dpc_submissions,
            steps: effects.steps,
        }
    }
}

/// Consuming adapter payload for the future live-runtime commit operation.
///
/// It is non-cloneable so an adapter must move the sole authority into its
/// commit call. It contains no device, renderer, JIT, scheduler, interrupt,
/// timing, or RDRAM pointer.
#[derive(Debug)]
pub struct VerifiedUcodePhaseCommit {
    selection: AudioHleSelection,
    terminal: AudioTaskTerminalReason,
    rdram_patches: CanonicalRdramPatches,
    pcm_ranges: CanonicalRdramRanges,
    rsp_memory: RspMemorySnapshot,
    machine_state: RspMachineState,
    pc_low12: u32,
    dpc_submissions: Vec<DeferredDpcSubmission>,
    steps: AudioTaskStepTotals,
}

impl VerifiedUcodePhaseCommit {
    pub const fn selection(&self) -> AudioHleSelection {
        self.selection
    }

    pub const fn terminal(&self) -> AudioTaskTerminalReason {
        self.terminal
    }

    pub const fn rdram_patches(&self) -> &CanonicalRdramPatches {
        &self.rdram_patches
    }

    pub const fn pcm_ranges(&self) -> &CanonicalRdramRanges {
        &self.pcm_ranges
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

    pub const fn steps(&self) -> AudioTaskStepTotals {
        self.steps
    }

    /// Move every field into the dependency-direction-neutral adapter parts.
    pub fn into_parts(
        self,
        task_admission_generation: NonZeroU64,
    ) -> VerifiedUcodePhaseCommitParts {
        VerifiedUcodePhaseCommitParts {
            task_admission_generation,
            selection: self.selection,
            terminal: self.terminal,
            rdram_patches: self.rdram_patches,
            pcm_ranges: self.pcm_ranges,
            rsp_memory: self.rsp_memory,
            machine_state: self.machine_state,
            pc_low12: self.pc_low12,
            dpc_submissions: self.dpc_submissions,
            steps: self.steps,
        }
    }
}

/// Opaque authority accepted by a higher-layer commit adapter.
///
/// This value remains non-cloneable and its fields are private. The sole
/// consuming accessor moves every component into one callback, so a caller
/// cannot construct commit authority from otherwise-valid effect values or
/// apply the same verified authority twice.
///
/// ```compile_fail
/// use fn64_audio::hle_commit::VerifiedUcodePhaseCommitParts;
///
/// let forged = VerifiedUcodePhaseCommitParts {
///     task_admission_generation: todo!(),
///     selection: todo!(),
///     terminal: todo!(),
///     rdram_patches: todo!(),
///     pcm_ranges: todo!(),
///     rsp_memory: todo!(),
///     machine_state: todo!(),
///     pc_low12: 0,
///     dpc_submissions: Vec::new(),
///     steps: todo!(),
/// };
/// ```
#[derive(Debug)]
pub struct VerifiedUcodePhaseCommitParts {
    task_admission_generation: NonZeroU64,
    selection: AudioHleSelection,
    terminal: AudioTaskTerminalReason,
    rdram_patches: CanonicalRdramPatches,
    pcm_ranges: CanonicalRdramRanges,
    rsp_memory: RspMemorySnapshot,
    machine_state: RspMachineState,
    pc_low12: u32,
    dpc_submissions: Vec<DeferredDpcSubmission>,
    steps: AudioTaskStepTotals,
}

impl VerifiedUcodePhaseCommitParts {
    /// Consume this authority exactly once and move all verified components
    /// into the adapter operation.
    pub fn consume_with<T>(
        self,
        apply: impl FnOnce(
            NonZeroU64,
            AudioHleSelection,
            AudioTaskTerminalReason,
            CanonicalRdramPatches,
            CanonicalRdramRanges,
            RspMemorySnapshot,
            RspMachineState,
            u32,
            Vec<DeferredDpcSubmission>,
            AudioTaskStepTotals,
        ) -> T,
    ) -> T {
        apply(
            self.task_admission_generation,
            self.selection,
            self.terminal,
            self.rdram_patches,
            self.pcm_ranges,
            self.rsp_memory,
            self.machine_state,
            self.pc_low12,
            self.dpc_submissions,
            self.steps,
        )
    }
}

#[derive(Debug)]
struct CommitReadyUcodeEffects {
    rdram_patches: CanonicalRdramPatches,
    rsp_memory: RspMemorySnapshot,
    machine_state: RspMachineState,
    pc_low12: u32,
    dpc_submissions: Vec<DeferredDpcSubmission>,
    steps: AudioTaskStepTotals,
}

impl CommitReadyUcodeEffects {
    fn from_speculative(effects: SpeculativeAudioLleEffects, steps: AudioTaskStepTotals) -> Self {
        let rdram_patches = effects.rdram_patches().clone();
        let rsp_memory = effects.rsp_memory().clone();
        let machine_state = effects.machine_state().clone();
        let pc_low12 = effects.pc_low12();
        let dpc_submissions = effects.dpc_submissions().to_vec();
        Self {
            rdram_patches,
            rsp_memory,
            machine_state,
            pc_low12,
            dpc_submissions,
            steps,
        }
    }
}

fn validate_effects(
    outcome: &AudioTaskOutcome,
    identity: AudioMicrocodeIdentity,
    terminal: AudioTaskTerminalReason,
    steps: AudioTaskStepTotals,
    effects: &SpeculativeAudioLleEffects,
) -> Result<(), PrepareUcodeCommitError> {
    if outcome.selection().microcode != identity {
        return Err(PrepareUcodeCommitError::MicrocodeIdentity {
            outcome: Box::new(outcome.selection().microcode),
            effects: Box::new(identity),
        });
    }
    if outcome.terminal() != terminal {
        return Err(PrepareUcodeCommitError::TerminalReason {
            outcome: outcome.terminal(),
            effects: terminal,
        });
    }
    if outcome.rdram_patches() != effects.rdram_patches() {
        return Err(PrepareUcodeCommitError::RdramPatches);
    }

    let rsp = outcome.rsp();
    let memory = effects.rsp_memory();
    if rsp.dmem != *memory.bank(RspMemoryBank::Dmem)
        || rsp.imem != *memory.bank(RspMemoryBank::Imem)
        || rsp.imem_generation != memory.imem_generation()
    {
        return Err(PrepareUcodeCommitError::RspMemory);
    }
    if rsp.sp_pc() != effects.pc_low12() {
        return Err(PrepareUcodeCommitError::SpPc {
            outcome: rsp.sp_pc(),
            effects: effects.pc_low12(),
        });
    }

    validate_machine_state(rsp, effects.machine_state(), steps)?;
    if rsp.dpc_submissions != effects.dpc_submissions() {
        return Err(PrepareUcodeCommitError::DeferredDpcSubmissions);
    }
    // Both comparison lanes begin at the post-rspboot boundary, so the
    // outcome's completion work is the ucode phase. The final machine counter
    // below and the commit payload retain rspboot plus ucode for device timing.
    if outcome.completion_steps() != steps.ucode() {
        return Err(PrepareUcodeCommitError::CompletionSteps {
            outcome: outcome.completion_steps(),
            effects: steps.ucode(),
        });
    }
    Ok(())
}

fn validate_machine_state(
    rsp: &RspVisibleState,
    machine: &RspMachineState,
    steps: AudioTaskStepTotals,
) -> Result<(), PrepareUcodeCommitError> {
    let architectural = machine.architectural_state();
    if rsp.sp_status != architectural.sp_status() {
        return Err(PrepareUcodeCommitError::SpStatus {
            outcome: rsp.sp_status,
            effects: architectural.sp_status(),
        });
    }
    if rsp.sp_semaphore != architectural.sp_semaphore() {
        return Err(PrepareUcodeCommitError::SpSemaphore {
            outcome: rsp.sp_semaphore,
            effects: architectural.sp_semaphore(),
        });
    }
    if rsp.dma.mem_address != architectural.dma_mem_address()
        || rsp.dma.dram_address != architectural.dma_dram_address()
        || rsp.dma.read_length != architectural.dma_read_length()
        || rsp.dma.write_length != architectural.dma_write_length()
    {
        return Err(PrepareUcodeCommitError::DmaRegisters);
    }
    if rsp.dpc.start != architectural.dp_start()
        || rsp.dpc.end != architectural.dp_end()
        || rsp.dpc.current != architectural.dp_current()
        || rsp.dpc.status != architectural.dp_status()
        || rsp.dpc.clock != architectural.dp_clock()
        || rsp.dpc.command_busy != architectural.dp_busy()
        || rsp.dpc.pipe_busy != architectural.dp_pipe_busy()
        || rsp.dpc.tmem_busy != architectural.dp_tmem_busy()
    {
        return Err(PrepareUcodeCommitError::DpcRegisters);
    }
    if !architectural.dp_submissions().is_empty() {
        return Err(PrepareUcodeCommitError::MachineRetainsDpcSubmissions {
            count: architectural.dp_submissions().len(),
        });
    }
    if machine.diagnostic_steps() != steps.whole_task().get() {
        return Err(PrepareUcodeCommitError::MachineDiagnosticSteps {
            machine: machine.diagnostic_steps(),
            effects: steps.whole_task(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hle_outcome::{
        AudioHleFamily, AudioMicrocodeIdentity, RdramByteRange, RdramPatch, RspDmaRegisterState,
        RspDpcRegisterState, Sha256Digest, RSP_BANK_BYTES,
    };
    use crate::rsp::runtime::RspMachine;
    use fn64_runtime::RspMemory;

    fn outcome(byte: u8, dpc: Vec<DeferredDpcSubmission>) -> AudioTaskOutcome {
        let patch = RdramPatch::new(0x100, vec![byte; 8]).unwrap();
        let patches = CanonicalRdramPatches::new(vec![patch]).unwrap();
        let pcm = CanonicalRdramRanges::new(vec![RdramByteRange::new(0x100, 8).unwrap()]).unwrap();
        let mut rdram = vec![0u8; 0x100];
        let machine = RspMachine::new(&mut rdram);
        let machine_state = machine.snapshot_state();
        let architectural = machine_state.architectural_state();
        let memory = RspMemory::new().snapshot();
        let rsp = RspVisibleState::new(
            *memory.bank(RspMemoryBank::Dmem),
            *memory.bank(RspMemoryBank::Imem),
            memory.imem_generation(),
            0,
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
            dpc,
        )
        .unwrap();
        AudioTaskOutcome::new(
            AudioHleSelection {
                microcode: AudioMicrocodeIdentity {
                    imem_sha256: Sha256Digest::new([1; 32]),
                    ucode_data_bytes: 0,
                    ucode_data_sha256: Sha256Digest::new([2; 32]),
                },
                family: AudioHleFamily::StandardAbi,
                implementation_revision: 1,
            },
            AudioTaskTerminalReason::Broke,
            patches,
            pcm,
            rsp,
            NonZeroU64::new(1).unwrap(),
        )
        .unwrap()
    }

    fn candidate(outcome: AudioTaskOutcome) -> UcodePhaseCommitCandidate {
        let mut rdram = vec![0u8; 0x100];
        let machine_state = RspMachine::new(&mut rdram).snapshot_state();
        UcodePhaseCommitCandidate {
            effects: CommitReadyUcodeEffects {
                rdram_patches: outcome.rdram_patches().clone(),
                rsp_memory: RspMemory::new().snapshot(),
                machine_state,
                pc_low12: outcome.rsp().sp_pc(),
                dpc_submissions: outcome.rsp().dpc_submissions.clone(),
                steps: AudioTaskStepTotals {
                    rspboot: NonZeroU64::new(1).unwrap(),
                    ucode: NonZeroU64::new(1).unwrap(),
                    whole_task: NonZeroU64::new(2).unwrap(),
                },
            },
            outcome,
        }
    }

    #[test]
    fn mismatch_yields_no_verification_token() {
        let reference = outcome(1, Vec::new());
        let candidate = candidate(outcome(2, Vec::new()));
        assert!(matches!(
            verify_ucode_phase_commit(&reference, candidate),
            Err(AudioTaskOutcomeMismatch::RdramPatchByte { .. })
        ));
    }

    #[test]
    fn exact_match_yields_one_consumable_token() {
        let reference = outcome(1, Vec::new());
        let token = verify_ucode_phase_commit(&reference, candidate(reference.clone())).unwrap();
        let commit = token.into_commit();
        assert_eq!(commit.selection(), reference.selection());
        assert_eq!(commit.rdram_patches(), reference.rdram_patches());
        assert_eq!(commit.steps().whole_task().get(), 2);
    }

    #[test]
    fn consuming_payload_retains_dpc_and_rsp_state() {
        let submission = DeferredDpcSubmission::from_dmem_payload(
            0x20,
            0x28,
            vec![0xd0, 0xd1, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7],
        )
        .unwrap();
        let reference = outcome(1, vec![submission.clone()]);
        let token = verify_ucode_phase_commit(&reference, candidate(reference.clone())).unwrap();
        let parts = token.into_commit().into_parts(NonZeroU64::new(7).unwrap());
        let (dpc_submissions, pc_low12, rsp_memory, machine_state, steps) = parts.consume_with(
            |generation,
             _,
             _,
             _,
             _,
             rsp_memory,
             machine_state,
             pc_low12,
             dpc_submissions,
             steps| {
                assert_eq!(generation.get(), 7);
                (dpc_submissions, pc_low12, rsp_memory, machine_state, steps)
            },
        );

        assert_eq!(dpc_submissions, vec![submission]);
        assert_eq!(pc_low12, reference.rsp().sp_pc());
        assert_eq!(rsp_memory.bank(RspMemoryBank::Dmem), &[0; RSP_BANK_BYTES]);
        assert_eq!(machine_state.diagnostic_steps(), 0);
        assert_eq!(steps.whole_task().get(), 2);
    }
}
