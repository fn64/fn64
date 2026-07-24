//! Pure whole-task audio reference preparation.
//!
//! This module executes rspboot once from a consumed pre-boot snapshot, forks
//! the proven entry boundary, and runs the authoritative ucode LLE lane.  The
//! returned value owns the matching admitted HLE lane and the complete
//! reference effects with no deferred DPC submission, but deliberately carries
//! no live-runtime commit authority. A future concrete HLE executor must
//! consume the sole wrapped HLE lane and compare its visible outcome before a
//! one-time commit token can exist.
//!
//! Provenance: task and DMA behavior follow the public SGI *Nintendo 64 RSP
//! Programmer's Guide* and libultra `OSTask` contract.  Whole-task composition
//! and proof ownership are repository policy described in `AUDIO-HLE.md`.

use fn64_runtime::rsp::RspMemorySnapshot;
use fn64_runtime::{OsTaskHeader, RdramAddr, RdramView};

use crate::hle::AdmittedAudioMicrocode;
use crate::hle_commit::{AudioTaskStepTotals, PrepareUcodeCommitError};
use crate::hle_effects::AudioImemReplacement;
use crate::hle_lle::{
    run_speculative_audio_lle, speculative_audio_lle_outcome, AudioLleOutcomeError,
    SpeculativeAudioLleError, SpeculativeAudioLleResult,
};
use crate::hle_outcome::{
    AudioTaskOutcome, CanonicalRdramError, CanonicalRdramPatches, CanonicalRdramRanges,
    RdramByteRange, RdramPatch, RdramPatchError,
};
use crate::hle_rspboot::{execute_audio_rspboot_to_entry, AudioRspbootError, AudioRspbootInput};
use crate::hle_snapshot::{AdmittedAudioHleTaskSnapshot, AudioHleLane, AudioHleSnapshotError};
use crate::rsp::runtime::RspMachineState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrepareWholeAudioTaskError {
    Rspboot(AudioRspbootError),
    Admission(AudioHleSnapshotError),
    Lle(SpeculativeAudioLleError),
    Outcome(AudioLleOutcomeError),
    ReferenceDpcSubmissions {
        count: usize,
    },
    StepTotals(PrepareUcodeCommitError),
    FinalPatch(RdramPatchError),
    CanonicalFinalPatches(CanonicalRdramError),
    ImemGenerationDiscontinuity {
        index: usize,
        expected: u64,
        actual: u64,
    },
}

/// The authoritative whole-task effects produced from one pre-boot owner.
///
/// This value is intentionally not `Clone`: it is paired with the sole HLE
/// lane forked from the same proven rspboot boundary.  Its private seal proves
/// that initial state, rspboot, and the LLE ucode phase produced no deferred
/// DPC submission.
///
/// ```compile_fail
/// use fn64_audio::whole_task::NoDpcSubmissionWholeAudioTaskReference;
///
/// fn duplicate(reference: NoDpcSubmissionWholeAudioTaskReference) {
///     let _second_reference = reference.clone();
/// }
/// ```
#[derive(Debug)]
pub struct NoDpcSubmissionWholeAudioTaskReference {
    task_addr: RdramAddr,
    loaded_header: OsTaskHeader,
    initial_rdram_storage: Box<[u8]>,
    initial_rsp_memory: RspMemorySnapshot,
    initial_machine_state: RspMachineState,
    initial_pc_low12: u32,
    boot_rdram_patches: CanonicalRdramPatches,
    ucode_rdram_patches: CanonicalRdramPatches,
    final_rdram_patches: CanonicalRdramPatches,
    imem_replacements: Vec<AudioImemReplacement>,
    lle: SpeculativeAudioLleResult,
    outcome: AudioTaskOutcome,
    steps: AudioTaskStepTotals,
    _no_dpc_submission: NoDpcSubmissionSeal,
}

#[derive(Debug)]
struct NoDpcSubmissionSeal;

impl NoDpcSubmissionWholeAudioTaskReference {
    pub const fn task_addr(&self) -> RdramAddr {
        self.task_addr
    }

    pub const fn loaded_header(&self) -> OsTaskHeader {
        self.loaded_header
    }

    pub fn initial_rdram_storage(&self) -> &[u8] {
        &self.initial_rdram_storage
    }

    pub const fn initial_rsp_memory(&self) -> &RspMemorySnapshot {
        &self.initial_rsp_memory
    }

    pub const fn initial_machine_state(&self) -> &RspMachineState {
        &self.initial_machine_state
    }

    pub const fn initial_pc_low12(&self) -> u32 {
        self.initial_pc_low12
    }

    pub const fn boot_rdram_patches(&self) -> &CanonicalRdramPatches {
        &self.boot_rdram_patches
    }

    pub const fn ucode_rdram_patches(&self) -> &CanonicalRdramPatches {
        &self.ucode_rdram_patches
    }

    /// Exact write intent across both phases with bytes from the final LLE
    /// image.  Same-valued writes remain represented.
    pub const fn final_rdram_patches(&self) -> &CanonicalRdramPatches {
        &self.final_rdram_patches
    }

    /// Ordered rspboot replacements followed by ordered ucode replacements.
    pub fn imem_replacements(&self) -> &[AudioImemReplacement] {
        &self.imem_replacements
    }

    pub const fn lle_result(&self) -> &SpeculativeAudioLleResult {
        &self.lle
    }

    /// Ucode-phase visible outcome used by a future concrete HLE comparator.
    pub const fn outcome(&self) -> &AudioTaskOutcome {
        &self.outcome
    }

    pub const fn steps(&self) -> AudioTaskStepTotals {
        self.steps
    }
}

/// Sole same-entry HLE lane paired with a whole-task reference.
///
/// This wrapper capability is not cloneable or publicly unwrap-able. Its
/// admitted snapshot remains inspectable and cloneable for diagnostics, and
/// can fork raw speculative lanes, but those values cannot reconstruct this
/// paired capability or promote an outcome. A later concrete family executor
/// inside this crate must consume this owner before it can produce a candidate
/// outcome.
///
/// ```compile_fail
/// use fn64_audio::whole_task::WholeAudioTaskHleLane;
///
/// fn duplicate(lane: WholeAudioTaskHleLane) {
///     let _second_lane = lane.clone();
/// }
/// ```
#[derive(Debug)]
pub struct WholeAudioTaskHleLane {
    lane: AudioHleLane,
}

impl WholeAudioTaskHleLane {
    pub const fn snapshot(&self) -> &AdmittedAudioHleTaskSnapshot {
        self.lane.snapshot()
    }

    pub(crate) fn into_inner(self) -> AudioHleLane {
        self.lane
    }
}

/// One same-entry HLE lane paired with its authoritative whole-task reference.
///
/// No method can turn this value into live mutation.  That remains impossible
/// until a concrete family executor produces an exact visible comparison.
#[derive(Debug)]
pub struct PreparedWholeAudioTaskDifferential {
    hle_lane: WholeAudioTaskHleLane,
    reference: NoDpcSubmissionWholeAudioTaskReference,
}

impl PreparedWholeAudioTaskDifferential {
    pub const fn hle_lane(&self) -> &WholeAudioTaskHleLane {
        &self.hle_lane
    }

    pub const fn reference(&self) -> &NoDpcSubmissionWholeAudioTaskReference {
        &self.reference
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        WholeAudioTaskHleLane,
        NoDpcSubmissionWholeAudioTaskReference,
    ) {
        (self.hle_lane, self.reference)
    }
}

/// Run one owned boot and authoritative LLE continuation without touching a
/// live device, scheduler, renderer, interrupt controller, or executable map.
pub fn prepare_no_dpc_submission_whole_audio_task(
    input: AudioRspbootInput,
    admission: AdmittedAudioMicrocode,
    pcm_ranges: CanonicalRdramRanges,
) -> Result<PreparedWholeAudioTaskDifferential, PrepareWholeAudioTaskError> {
    let task_addr = input.task_addr();
    let loaded_header = input.loaded_header();
    let initial_rdram_storage = input.rdram_storage().to_vec().into_boxed_slice();
    let initial_rsp_memory = input.rsp_memory().clone();
    let initial_machine_state = input.initial_machine_state().clone();
    let initial_pc_low12 = input.initial_pc_low12();

    let boot =
        execute_audio_rspboot_to_entry(input).map_err(PrepareWholeAudioTaskError::Rspboot)?;
    let admitted = boot
        .entry()
        .clone()
        .admit_hle(admission)
        .map_err(PrepareWholeAudioTaskError::Admission)?;
    let hle_lane = admitted.fork_hle_lane();
    let lle = run_speculative_audio_lle(admitted.fork_lle_lane().into_lle_parts())
        .map_err(PrepareWholeAudioTaskError::Lle)?;
    if !lle.dpc_submissions().is_empty() {
        return Err(PrepareWholeAudioTaskError::ReferenceDpcSubmissions {
            count: lle.dpc_submissions().len(),
        });
    }
    let outcome = speculative_audio_lle_outcome(&lle, admission, pcm_ranges)
        .map_err(PrepareWholeAudioTaskError::Outcome)?;
    let steps = AudioTaskStepTotals::new(lle.rspboot_steps(), lle.ucode_steps())
        .map_err(PrepareWholeAudioTaskError::StepTotals)?;
    let final_rdram_patches = compose_final_rdram_patches(
        boot.boot_rdram_write_ranges(),
        lle.rdram_write_ranges(),
        lle.rdram_storage(),
    )?;

    let mut imem_replacements = boot.imem_replacements().to_vec();
    imem_replacements.extend_from_slice(lle.imem_replacements());
    validate_imem_generations(initial_rsp_memory.imem_generation(), &imem_replacements)?;

    Ok(PreparedWholeAudioTaskDifferential {
        hle_lane: WholeAudioTaskHleLane { lane: hle_lane },
        reference: NoDpcSubmissionWholeAudioTaskReference {
            task_addr,
            loaded_header,
            initial_rdram_storage,
            initial_rsp_memory,
            initial_machine_state,
            initial_pc_low12,
            boot_rdram_patches: boot.boot_rdram_patches().clone(),
            ucode_rdram_patches: lle.rdram_patches().clone(),
            final_rdram_patches,
            imem_replacements,
            lle,
            outcome,
            steps,
            _no_dpc_submission: NoDpcSubmissionSeal,
        },
    })
}

fn validate_imem_generations(
    initial_generation: u64,
    replacements: &[AudioImemReplacement],
) -> Result<(), PrepareWholeAudioTaskError> {
    for (index, replacement) in replacements.iter().enumerate() {
        let expected = initial_generation
            .checked_add(index as u64 + 1)
            .expect("whole-task IMEM generation index overflow");
        if replacement.generation() != expected {
            return Err(PrepareWholeAudioTaskError::ImemGenerationDiscontinuity {
                index,
                expected,
                actual: replacement.generation(),
            });
        }
    }
    Ok(())
}

fn compose_final_rdram_patches(
    boot: &[RdramByteRange],
    ucode: &[RdramByteRange],
    final_storage: &[u8],
) -> Result<CanonicalRdramPatches, PrepareWholeAudioTaskError> {
    let mut ranges = boot.iter().chain(ucode).copied().collect::<Vec<_>>();
    ranges.sort_unstable_by_key(|range| range.start());
    let mut merged: Vec<RdramByteRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(previous) if range.start() <= previous.end() => {
                let end = previous.end().max(range.end());
                *previous = RdramByteRange::new(previous.start(), end - previous.start())
                    .expect("merged validated physical ranges remain valid");
            }
            _ => merged.push(range),
        }
    }

    let view = RdramView::from_storage(final_storage);
    let patches = merged
        .into_iter()
        .map(|range| {
            let bytes = (range.start()..range.end())
                .map(|offset| view.read_u8(RdramAddr::from_offset(offset)))
                .collect();
            RdramPatch::new(range.start(), bytes).map_err(PrepareWholeAudioTaskError::FinalPatch)
        })
        .collect::<Result<Vec<_>, _>>()?;
    CanonicalRdramPatches::new(patches).map_err(PrepareWholeAudioTaskError::CanonicalFinalPatches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
    use fn64_runtime::{
        RdramViewMut, RspMemAddr, RspMemory, RspMemoryBank, M_AUDTASK, SP_STATUS_BROKE,
        SP_STATUS_HALT,
    };

    use crate::hle::{AudioHleCatalog, AudioHleCatalogEntry};
    use crate::hle_executor::{execute_standard_whole_audio_task, StandardAudioHleFrontier};
    use crate::hle_outcome::AudioHleFamily;
    use crate::rsp::runtime::RspMachine;

    const HEADER: u32 = 0x40;
    const BOOT: u32 = 0x100;
    const UCODE: u32 = 0x180;
    const COMMANDS: u32 = 0x300;
    const UCODE_DATA: u32 = 0x380;
    const BREAK: u32 = 0x0000_000d;

    fn mtc0(rt: u32, rd: u32) -> u32 {
        (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11)
    }

    fn addiu(rt: u32, rs: u32, immediate: u32) -> u32 {
        (0x09 << 26) | (rs << 21) | (rt << 16) | (immediate & 0xffff)
    }

    fn fixture() -> AudioRspbootInput {
        fixture_with_ucode(0x2405_5678, |_| {})
    }

    fn fixture_with_command(w0: u32, w1: u32) -> AudioRspbootInput {
        let input = fixture();
        let mut rdram = input.rdram_storage().to_vec();
        RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
            RdramAddr::from_offset(COMMANDS),
            &[w0.to_be_bytes(), w1.to_be_bytes()].concat(),
        );
        AudioRspbootInput::new(
            input.task_addr(),
            input.loaded_header(),
            rdram,
            input.rsp_memory().clone(),
            input.initial_pc_low12(),
            input.initial_machine_state().clone(),
        )
        .unwrap()
    }

    fn fixture_with_ucode(
        first_ucode_word: u32,
        configure_machine: impl FnOnce(&mut RspMachine<'_>),
    ) -> AudioRspbootInput {
        let header = OsTaskHeader {
            task_type: M_AUDTASK,
            ucode_boot: BOOT,
            ucode_boot_size: 32,
            ucode: UCODE,
            ucode_size: 8,
            ucode_data: UCODE_DATA,
            ucode_data_size: 4,
            data_ptr: COMMANDS,
            data_size: 8,
            ..OsTaskHeader::default()
        };
        let boot = [
            0x2402_0000 | UCODE,
            mtc0(2, 1),
            0x2403_1080,
            mtc0(3, 0),
            0x2404_0007,
            mtc0(4, 2),
            0x0800_0020,
            0x2407_7777,
        ];
        let mut rdram = vec![0u8; DEFAULT_RDRAM_SIZE];
        for (offset, word) in boot.into_iter().enumerate() {
            rdram[BOOT as usize + offset * 4..BOOT as usize + offset * 4 + 4]
                .copy_from_slice(&word.to_ne_bytes());
        }
        for (offset, word) in [first_ucode_word, BREAK].into_iter().enumerate() {
            rdram[UCODE as usize + offset * 4..UCODE as usize + offset * 4 + 4]
                .copy_from_slice(&word.to_ne_bytes());
        }
        let mut view = RdramViewMut::from_storage(&mut rdram);
        view.write_logical_bytes(RdramAddr::from_offset(COMMANDS), &[0; 8]);
        view.write_logical_bytes(RdramAddr::from_offset(UCODE_DATA), &[1, 2, 3, 4]);

        let mut rsp_memory = RspMemory::new();
        let boot_bytes = boot
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect::<Vec<_>>();
        rsp_memory
            .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &boot_bytes)
            .unwrap();
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
        let header_bytes = fields
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        rsp_memory
            .write_bytes(RspMemAddr::from_register(0x0fc0), &header_bytes)
            .unwrap();

        let mut machine_storage = [0; 8];
        let mut machine = RspMachine::new(&mut machine_storage);
        machine.set_sp_status_raw(SP_STATUS_HALT | SP_STATUS_BROKE);
        configure_machine(&mut machine);
        AudioRspbootInput::new(
            RdramAddr::from_offset(HEADER),
            header,
            rdram,
            rsp_memory.snapshot(),
            0,
            machine.snapshot_state(),
        )
        .unwrap()
    }

    fn overlapping_write_and_overlay_fixture() -> AudioRspbootInput {
        const WRITE_TARGET: u32 = 0x500;
        const OVERLAY_SOURCE: u32 = 0x600;
        const UCODE_ENTRY: u32 = 0x80;

        let ucode = [
            addiu(2, 0, 0x110),
            mtc0(2, 0),
            addiu(3, 0, WRITE_TARGET),
            mtc0(3, 1),
            addiu(4, 0, 7),
            mtc0(4, 3),
            addiu(2, 0, 0x1000 | UCODE_ENTRY),
            mtc0(2, 0),
            addiu(3, 0, OVERLAY_SOURCE),
            mtc0(3, 1),
            addiu(4, 0, 7),
            mtc0(4, 2),
            addiu(2, 0, 0x1000 | (UCODE_ENTRY + 8)),
            mtc0(2, 0),
            addiu(3, 0, OVERLAY_SOURCE + 8),
            mtc0(3, 1),
            addiu(4, 0, 7),
            mtc0(4, 2),
            BREAK,
        ];
        let ucode_bytes = (ucode.len() * core::mem::size_of::<u32>()) as u32;
        let boot = [
            addiu(2, 0, 0x100),
            mtc0(2, 0),
            addiu(3, 0, WRITE_TARGET),
            mtc0(3, 1),
            addiu(4, 0, 15),
            mtc0(4, 3),
            addiu(2, 0, UCODE),
            mtc0(2, 1),
            addiu(3, 0, 0x1000 | UCODE_ENTRY),
            mtc0(3, 0),
            addiu(4, 0, ucode_bytes - 1),
            mtc0(4, 2),
            0x0800_0000 | (UCODE_ENTRY >> 2),
            addiu(7, 0, 0x7777),
        ];
        let header = OsTaskHeader {
            task_type: M_AUDTASK,
            ucode_boot: BOOT,
            ucode_boot_size: (boot.len() * core::mem::size_of::<u32>()) as u32,
            ucode: UCODE,
            ucode_size: ucode_bytes,
            ucode_data: UCODE_DATA,
            ucode_data_size: 4,
            data_ptr: COMMANDS,
            data_size: 8,
            ..OsTaskHeader::default()
        };

        let mut rdram = vec![0u8; DEFAULT_RDRAM_SIZE];
        for (offset, word) in boot.into_iter().enumerate() {
            rdram[BOOT as usize + offset * 4..BOOT as usize + offset * 4 + 4]
                .copy_from_slice(&word.to_ne_bytes());
        }
        for (offset, word) in ucode.into_iter().enumerate() {
            rdram[UCODE as usize + offset * 4..UCODE as usize + offset * 4 + 4]
                .copy_from_slice(&word.to_ne_bytes());
        }
        let mut view = RdramViewMut::from_storage(&mut rdram);
        view.write_logical_bytes(RdramAddr::from_offset(COMMANDS), &[0; 8]);
        view.write_logical_bytes(RdramAddr::from_offset(UCODE_DATA), &[1, 2, 3, 4]);

        let mut rsp_memory = RspMemory::new();
        let boot_bytes = boot
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect::<Vec<_>>();
        rsp_memory
            .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &boot_bytes)
            .unwrap();
        rsp_memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Dmem, 0x100),
                &[
                    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
                    23, 24,
                ],
            )
            .unwrap();
        let header_bytes = [
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
        ]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect::<Vec<_>>();
        rsp_memory
            .write_bytes(RspMemAddr::from_register(0x0fc0), &header_bytes)
            .unwrap();

        let mut machine_storage = [0; 8];
        let mut machine = RspMachine::new(&mut machine_storage);
        machine.set_sp_status_raw(SP_STATUS_HALT | SP_STATUS_BROKE);
        AudioRspbootInput::new(
            RdramAddr::from_offset(HEADER),
            header,
            rdram,
            rsp_memory.snapshot(),
            0,
            machine.snapshot_state(),
        )
        .unwrap()
    }

    fn admission(input: &AudioRspbootInput) -> AdmittedAudioMicrocode {
        let boot = execute_audio_rspboot_to_entry(input.clone()).unwrap();
        let entry = boot.entry();
        let entries = [AudioHleCatalogEntry {
            identity: entry.identity(),
            family: AudioHleFamily::StandardAbi,
            implementation_revision: 1,
        }];
        AudioHleCatalog::new(&entries)
            .unwrap()
            .admit(entry.identity())
            .unwrap()
    }

    #[test]
    fn prepares_one_submission_free_reference_without_mutating_the_input() {
        let input = fixture();
        let before = input.rdram_storage().to_vec();
        let prepared = prepare_no_dpc_submission_whole_audio_task(
            input.clone(),
            admission(&input),
            CanonicalRdramRanges::default(),
        )
        .unwrap();
        let reference = prepared.reference();

        assert_eq!(input.rdram_storage(), before);
        assert_eq!(reference.initial_rdram_storage(), before);
        assert_eq!(reference.task_addr(), RdramAddr::from_offset(HEADER));
        assert_eq!(reference.steps().rspboot().get(), 7);
        assert_eq!(reference.steps().ucode().get(), 2);
        assert_eq!(reference.steps().whole_task().get(), 9);
        assert_eq!(reference.imem_replacements().len(), 1);
        assert!(reference.final_rdram_patches().as_slice().is_empty());
        assert_eq!(
            reference
                .lle_result()
                .machine_state()
                .architectural_state()
                .gprs()[5],
            0x5678
        );
        assert_eq!(
            prepared.hle_lane().snapshot().entry().identity(),
            reference.outcome().selection().microcode
        );
    }

    #[test]
    fn public_preparation_orders_boot_and_multiple_ucode_replacements_and_composes_overlap() {
        let input = overlapping_write_and_overlay_fixture();
        let initial_generation = input.rsp_memory().imem_generation();
        let prepared = prepare_no_dpc_submission_whole_audio_task(
            input.clone(),
            admission(&input),
            CanonicalRdramRanges::default(),
        )
        .unwrap();
        let reference = prepared.reference();
        let replacements = reference.imem_replacements();

        assert_eq!(replacements.len(), 3);
        assert_eq!(
            replacements
                .iter()
                .map(AudioImemReplacement::generation)
                .collect::<Vec<_>>(),
            [
                initial_generation + 1,
                initial_generation + 2,
                initial_generation + 3,
            ]
        );
        assert_eq!(
            &replacements[0].image()[0x80..0x88],
            &[addiu(2, 0, 0x110).to_be_bytes(), mtc0(2, 0).to_be_bytes(),].concat(),
            "the boot replacement must precede both ucode replacements"
        );
        assert_eq!(&replacements[1].image()[0x80..0x88], &[0; 8]);
        assert_ne!(
            &replacements[1].image()[0x88..0x90],
            &[0; 8],
            "the first ucode replacement must be captured before the second"
        );
        assert_eq!(&replacements[2].image()[0x80..0x90], &[0; 16]);
        assert_eq!(
            replacements[2].image(),
            reference
                .lle_result()
                .rsp_memory()
                .bank(RspMemoryBank::Imem)
        );

        assert_eq!(
            reference.boot_rdram_patches().as_slice()[0].range(),
            RdramByteRange::new(0x500, 16).unwrap()
        );
        assert_eq!(
            reference.ucode_rdram_patches().as_slice()[0].range(),
            RdramByteRange::new(0x500, 8).unwrap()
        );
        let final_patch = &reference.final_rdram_patches().as_slice()[0];
        assert_eq!(final_patch.range(), RdramByteRange::new(0x500, 16).unwrap());
        assert_eq!(
            final_patch.bytes(),
            &[17, 18, 19, 20, 21, 22, 23, 24, 9, 10, 11, 12, 13, 14, 15, 16],
            "ucode bytes must win the real boot/ucode overlap while both phases retain write intent"
        );
    }

    #[test]
    fn composition_preserves_write_intent_and_uses_final_bytes() {
        let mut storage = vec![0u8; DEFAULT_RDRAM_SIZE];
        let mut view = RdramViewMut::from_storage(&mut storage);
        view.write_logical_bytes(RdramAddr::from_offset(0x100), &[1, 2, 3, 4, 5, 6]);
        let patches = compose_final_rdram_patches(
            &[RdramByteRange::new(0x100, 4).unwrap()],
            &[
                RdramByteRange::new(0x102, 2).unwrap(),
                RdramByteRange::new(0x104, 2).unwrap(),
            ],
            &storage,
        )
        .unwrap();

        assert_eq!(patches.as_slice().len(), 1);
        assert_eq!(patches.as_slice()[0].range().start(), 0x100);
        assert_eq!(patches.as_slice()[0].bytes(), &[1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn reference_dpc_submission_prevents_submission_free_preparation() {
        let input = fixture_with_ucode(mtc0(31, 9), |machine| {
            machine.set_reg(31, 0x108);
            assert_eq!(machine.write_cp0(8, 0x100), None);
        });
        assert!(matches!(
            prepare_no_dpc_submission_whole_audio_task(
                input.clone(),
                admission(&input),
                CanonicalRdramRanges::default(),
            ),
            Err(PrepareWholeAudioTaskError::ReferenceDpcSubmissions { count: 1 })
        ));
    }

    #[test]
    fn mismatched_admission_cannot_prepare_a_whole_task_reference() {
        let input = fixture();
        let boot = execute_audio_rspboot_to_entry(input.clone()).unwrap();
        let mut identity = boot.entry().identity();
        identity.imem_sha256 = crate::hle_outcome::Sha256Digest::new([0xa5; 32]);
        let entries = [AudioHleCatalogEntry {
            identity,
            family: AudioHleFamily::StandardAbi,
            implementation_revision: 1,
        }];
        let wrong = AudioHleCatalog::new(&entries)
            .unwrap()
            .admit(identity)
            .unwrap();

        assert!(matches!(
            prepare_no_dpc_submission_whole_audio_task(
                input,
                wrong,
                CanonicalRdramRanges::default(),
            ),
            Err(PrepareWholeAudioTaskError::Admission(
                AudioHleSnapshotError::MicrocodeIdentityMismatch { .. }
            ))
        ));
    }

    #[test]
    fn consuming_standard_executor_stops_at_dsp_without_mutating_reference_state() {
        let input = fixture();
        let prepared = prepare_no_dpc_submission_whole_audio_task(
            input.clone(),
            admission(&input),
            CanonicalRdramRanges::default(),
        )
        .unwrap();
        let attempted = execute_standard_whole_audio_task(prepared);

        assert_eq!(attempted.decoded_commands(), 1);
        assert!(matches!(
            attempted.frontier(),
            StandardAudioHleFrontier::UnsupportedDspSemantics {
                command_index: 0,
                opcode: crate::standard_abi::StandardAbiOpcode::SpNoop,
            }
        ));
        assert_eq!(
            attempted.reference().initial_rdram_storage(),
            input.rdram_storage()
        );
        assert!(attempted
            .reference()
            .final_rdram_patches()
            .as_slice()
            .is_empty());
    }

    #[test]
    fn proven_setbuffer_state_advances_to_typed_completion_frontier() {
        let input = fixture_with_command(0x0800_0100, 0x0200_0020);
        let prepared = prepare_no_dpc_submission_whole_audio_task(
            input.clone(),
            admission(&input),
            CanonicalRdramRanges::default(),
        )
        .unwrap();
        let attempted = execute_standard_whole_audio_task(prepared);

        assert_eq!(attempted.decoded_commands(), 1);
        assert_eq!(
            attempted.frontier(),
            &StandardAudioHleFrontier::UnsupportedCompletionSemantics { command_count: 1 }
        );
    }

    #[test]
    fn unknown_standard_opcode_is_a_consuming_typed_frontier() {
        let input = fixture_with_command(0x1000_0000, 0);
        let prepared = prepare_no_dpc_submission_whole_audio_task(
            input.clone(),
            admission(&input),
            CanonicalRdramRanges::default(),
        )
        .unwrap();
        let attempted = execute_standard_whole_audio_task(prepared);

        assert_eq!(attempted.decoded_commands(), 0);
        assert!(matches!(
            attempted.frontier(),
            StandardAudioHleFrontier::UnknownOpcode {
                command_index: 0,
                source: crate::standard_abi::UnknownStandardAbiOpcode { opcode: 0x10 },
            }
        ));
    }
}
