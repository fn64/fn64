//! Pure rspboot execution up to an audio microcode's first instruction.
//!
//! This kernel owns its RDRAM and RSP-memory images and has no access to the
//! live device fabric, renderer, JIT, scheduler, interrupt controller, timing
//! model, or evidence log.  It therefore cannot leak a speculative boot into
//! host state.  The loaded CPU-side `OSTask` header remains a separate value;
//! only the entry header DMA'd into DMEM may be changed by rspboot.
//!
//! Provenance: the task boundary, SP DMA length/count/skip behavior, and IMEM
//! execution model follow the public SGI *Nintendo 64 RSP Programmer's Guide*.
//! The sixteen-word task shape is the public libultra `OSTask_t` contract.

use core::num::NonZeroU64;

use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
use fn64_runtime::{
    OsTaskHeader, RdramAddr, RdramView, RspMemAddr, RspMemory, RspMemoryBank, M_AUDTASK,
    SP_STATUS_BROKE, SP_STATUS_HALT,
};

use crate::hle_outcome::{
    CanonicalRdramError, CanonicalRdramPatches, RdramByteRange, RdramPatch, RdramPatchError,
    RdramRangeError, Sha256Digest, RSP_BANK_BYTES,
};
use crate::hle_snapshot::{
    AudioHleSnapshotError, AudioTaskEntrySnapshot, PostRspbootAudioTaskParts,
};
use crate::rsp::runtime::{ImemDmaSpan, RspMachine, RspMachineState};
use crate::rsp::{run_imem, RspExitReason};

const BOOT_CHUNK_STEPS: u64 = 1 << 12;
const MAX_BOOT_STEPS: u64 = 1 << 20;
const OS_TASK_DMEM_OFFSET: usize = RSP_BANK_BYTES - 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RspbootHeaderRange {
    Task,
    UcodeBoot,
    Ucode,
    UcodeData,
    CommandList,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioRspbootError {
    PhysicalRdramStorageLength {
        storage_bytes: usize,
        required_bytes: usize,
    },
    NonAudioTask {
        task_type: u32,
    },
    DirectImemUnsupported,
    InitialPcUnaligned {
        pc: u32,
    },
    InitialPcOutsideFabricRange {
        pc: u32,
    },
    InitialDiagnosticSteps {
        steps: u64,
    },
    InitialPendingDpcSubmissions {
        count: usize,
    },
    InitialExecutionContinuation {
        jump_target: u32,
        resume_address: u32,
        resume_delay: bool,
    },
    HeaderRange {
        field: RspbootHeaderRange,
        source: RdramRangeError,
    },
    StaticAliasNotAllowed {
        field: RspbootHeaderRange,
        address: u32,
        byte_len: u32,
    },
    LoadedHeaderDmemMismatch {
        first_mismatch: usize,
    },
    StepBoundExceeded {
        maximum_steps: u64,
        pc: u32,
    },
    EarlyBreak {
        pc: u32,
        steps: u64,
    },
    UnexpectedExit {
        reason: RspExitReason,
        pc: u32,
        steps: u64,
    },
    RspbootDpcSubmissions {
        count: usize,
    },
    ZeroRspbootSteps,
    RdramWriteRange {
        start: usize,
        end: usize,
    },
    RdramPatch(RdramPatchError),
    CanonicalRdramPatches(CanonicalRdramError),
    EntrySnapshot(AudioHleSnapshotError),
}

/// Complete owned inputs at the instant an admitted audio rspboot starts.
#[derive(Clone, Debug)]
pub struct AudioRspbootInput {
    task_addr: RdramAddr,
    loaded_header: OsTaskHeader,
    rdram_storage: Vec<u8>,
    rsp_memory: fn64_runtime::rsp::RspMemorySnapshot,
    initial_pc_low12: u32,
    initial_machine_state: RspMachineState,
}

impl AudioRspbootInput {
    pub fn new(
        task_addr: RdramAddr,
        loaded_header: OsTaskHeader,
        rdram_storage: Vec<u8>,
        rsp_memory: fn64_runtime::rsp::RspMemorySnapshot,
        initial_pc_low12: u32,
        initial_machine_state: RspMachineState,
    ) -> Result<Self, AudioRspbootError> {
        validate_input(
            task_addr,
            loaded_header,
            &rdram_storage,
            &rsp_memory,
            initial_pc_low12,
            &initial_machine_state,
        )?;
        Ok(Self {
            task_addr,
            loaded_header,
            rdram_storage,
            rsp_memory,
            initial_pc_low12,
            initial_machine_state,
        })
    }

    pub const fn task_addr(&self) -> RdramAddr {
        self.task_addr
    }

    pub const fn loaded_header(&self) -> OsTaskHeader {
        self.loaded_header
    }

    pub fn rdram_storage(&self) -> &[u8] {
        &self.rdram_storage
    }

    pub const fn rsp_memory(&self) -> &fn64_runtime::rsp::RspMemorySnapshot {
        &self.rsp_memory
    }

    pub const fn initial_pc_low12(&self) -> u32 {
        self.initial_pc_low12
    }

    pub const fn initial_machine_state(&self) -> &RspMachineState {
        &self.initial_machine_state
    }
}

/// One complete IMEM image installed by rspboot, in installation order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RspbootImemReplacement {
    generation: u64,
    identity: Sha256Digest,
    image: [u8; RSP_BANK_BYTES],
}

impl RspbootImemReplacement {
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn identity(&self) -> Sha256Digest {
        self.identity
    }

    pub const fn image(&self) -> &[u8; RSP_BANK_BYTES] {
        &self.image
    }
}

/// Owned post-rspboot entry plus the exact effects of the boot phase.
#[derive(Clone, Debug)]
pub struct AudioRspbootEntry {
    entry: AudioTaskEntrySnapshot,
    boot_rdram_write_ranges: Vec<RdramByteRange>,
    boot_rdram_patches: CanonicalRdramPatches,
    imem_replacements: Vec<RspbootImemReplacement>,
}

impl AudioRspbootEntry {
    pub const fn entry(&self) -> &AudioTaskEntrySnapshot {
        &self.entry
    }

    pub fn boot_rdram_write_ranges(&self) -> &[RdramByteRange] {
        &self.boot_rdram_write_ranges
    }

    pub const fn boot_rdram_patches(&self) -> &CanonicalRdramPatches {
        &self.boot_rdram_patches
    }

    pub fn imem_replacements(&self) -> &[RspbootImemReplacement] {
        &self.imem_replacements
    }

    pub fn into_entry(self) -> AudioTaskEntrySnapshot {
        self.entry
    }
}

/// Execute rspboot until the next instruction fetch belongs to an IMEM image
/// installed by that boot. The first loaded-ucode instruction is not executed.
pub fn execute_audio_rspboot_to_entry(
    input: AudioRspbootInput,
) -> Result<AudioRspbootEntry, AudioRspbootError> {
    let AudioRspbootInput {
        task_addr,
        loaded_header,
        mut rdram_storage,
        rsp_memory,
        initial_pc_low12,
        initial_machine_state,
    } = input;

    let initial_generation = rsp_memory.imem_generation();
    let mut persistent_memory = RspMemory::from_snapshot(rsp_memory);
    let mut imem = *persistent_memory.bank(RspMemoryBank::Imem);
    let mut machine = RspMachine::new(&mut rdram_storage);
    machine.set_dma_rdram_ranges(std::iter::once(0..DEFAULT_RDRAM_SIZE).collect());
    machine.load_dmem_logical(persistent_memory.bank(RspMemoryBank::Dmem));
    machine.restore_state(initial_machine_state);
    machine.set_sp_status_raw(machine.sp_status() & !(SP_STATUS_HALT | SP_STATUS_BROKE));

    let mut pc = initial_pc_low12;
    let mut total_steps = 0u64;
    let mut loaded_spans: Vec<ImemDmaSpan> = Vec::new();
    let mut replacements = Vec::new();

    loop {
        let execution_pc = if machine.ctx.resume_address != 0 {
            0x1000 | (machine.ctx.resume_address & 0x0fff)
        } else {
            0x1000 | (pc & 0x0fff)
        };
        if loaded_spans
            .iter()
            .copied()
            .any(|span| span.contains_pc(execution_pc))
        {
            pc = execution_pc & 0x0fff;
            break;
        }

        if total_steps >= MAX_BOOT_STEPS {
            return Err(AudioRspbootError::StepBoundExceeded {
                maximum_steps: MAX_BOOT_STEPS,
                pc: pc & 0x0fff,
            });
        }
        let words = logical_imem_words(&imem);
        let budget = if loaded_spans.is_empty() {
            BOOT_CHUNK_STEPS.min(MAX_BOOT_STEPS - total_steps)
        } else {
            1
        };
        let result = run_imem(&words, pc, &mut machine, budget);
        total_steps = total_steps
            .checked_add(result.steps)
            .expect("rspboot step counter overflow");
        pc = result.pc & 0x0fff;

        match result.reason {
            RspExitReason::SwapOverlay => {
                loaded_spans.push(machine.pending_imem_dma_span());
                machine.complete_imem_dma(&mut imem);
                persistent_memory
                    .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &imem)
                    .expect("complete IMEM image is always in range");
                replacements.push(RspbootImemReplacement {
                    generation: persistent_memory.imem_generation(),
                    identity: Sha256Digest::hash(&imem),
                    image: imem,
                });
            }
            RspExitReason::StepLimit => {}
            RspExitReason::Broke => {
                return Err(AudioRspbootError::EarlyBreak {
                    pc,
                    steps: total_steps,
                });
            }
            reason => {
                return Err(AudioRspbootError::UnexpectedExit {
                    reason,
                    pc,
                    steps: total_steps,
                });
            }
        }
    }

    let rspboot_steps = NonZeroU64::new(total_steps).ok_or(AudioRspbootError::ZeroRspbootSteps)?;
    let dmem = machine.dmem_logical();
    let entry_header = decode_os_task_header(&dmem[OS_TASK_DMEM_OFFSET..]);
    let machine_state: RspMachineState = machine.snapshot_state();
    let dpc_submission_count = machine_state.architectural_state().dp_submissions().len();
    if dpc_submission_count != 0 {
        return Err(AudioRspbootError::RspbootDpcSubmissions {
            count: dpc_submission_count,
        });
    }
    let storage_write_ranges = machine.take_rdram_writes();
    drop(machine);

    persistent_memory
        .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Dmem, 0), &dmem)
        .expect("complete DMEM image is always in range");
    assert_eq!(
        persistent_memory.imem_generation(),
        initial_generation
            .checked_add(replacements.len() as u64)
            .expect("rspboot IMEM generation overflow"),
        "rspboot replacement count diverged from owned RSP-memory generation"
    );

    let (boot_rdram_write_ranges, boot_rdram_patches) =
        collect_logical_rdram_effects(&rdram_storage, storage_write_ranges)?;
    let command_bytes = capture_logical_declared_bytes(
        &rdram_storage,
        RspbootHeaderRange::CommandList,
        entry_header.data_ptr,
        entry_header.data_size,
    )?;
    let ucode_data_bytes = capture_logical_declared_bytes(
        &rdram_storage,
        RspbootHeaderRange::UcodeData,
        entry_header.ucode_data,
        entry_header.ucode_data_size,
    )?;
    let entry = AudioTaskEntrySnapshot::from_post_rspboot(PostRspbootAudioTaskParts {
        task_addr,
        loaded_header,
        entry_header,
        command_bytes,
        ucode_data_bytes,
        rdram_storage,
        rsp_memory: persistent_memory.snapshot(),
        machine_state,
        entry_pc_low12: pc,
        rspboot_steps: rspboot_steps.get(),
        admitted_dma_ranges: std::iter::once(0..DEFAULT_RDRAM_SIZE).collect(),
    })
    .map_err(AudioRspbootError::EntrySnapshot)?;

    Ok(AudioRspbootEntry {
        entry,
        boot_rdram_write_ranges,
        boot_rdram_patches,
        imem_replacements: replacements,
    })
}

fn validate_input(
    task_addr: RdramAddr,
    header: OsTaskHeader,
    rdram: &[u8],
    rsp_memory: &fn64_runtime::rsp::RspMemorySnapshot,
    pc: u32,
    machine_state: &RspMachineState,
) -> Result<(), AudioRspbootError> {
    if rdram.len() != DEFAULT_RDRAM_SIZE {
        return Err(AudioRspbootError::PhysicalRdramStorageLength {
            storage_bytes: rdram.len(),
            required_bytes: DEFAULT_RDRAM_SIZE,
        });
    }
    if header.task_type != M_AUDTASK {
        return Err(AudioRspbootError::NonAudioTask {
            task_type: header.task_type,
        });
    }
    if direct_imem_shape(header) {
        return Err(AudioRspbootError::DirectImemUnsupported);
    }
    if !pc.is_multiple_of(4) {
        return Err(AudioRspbootError::InitialPcUnaligned { pc });
    }
    if pc > 0x0ffc {
        return Err(AudioRspbootError::InitialPcOutsideFabricRange { pc });
    }
    if machine_state.diagnostic_steps() != 0 {
        return Err(AudioRspbootError::InitialDiagnosticSteps {
            steps: machine_state.diagnostic_steps(),
        });
    }
    let architectural = machine_state.architectural_state();
    if architectural.jump_target() != 0
        || architectural.resume_address() != 0
        || architectural.resume_delay()
    {
        return Err(AudioRspbootError::InitialExecutionContinuation {
            jump_target: architectural.jump_target(),
            resume_address: architectural.resume_address(),
            resume_delay: architectural.resume_delay(),
        });
    }
    let dpc_submission_count = machine_state.architectural_state().dp_submissions().len();
    if dpc_submission_count != 0 {
        return Err(AudioRspbootError::InitialPendingDpcSubmissions {
            count: dpc_submission_count,
        });
    }

    validate_physical_range(RspbootHeaderRange::Task, task_addr.offset(), 64, false)?;
    validate_physical_range(
        RspbootHeaderRange::UcodeBoot,
        header.ucode_boot,
        header.ucode_boot_size,
        false,
    )?;
    validate_physical_range(
        RspbootHeaderRange::Ucode,
        header.ucode,
        header.ucode_size,
        false,
    )?;
    validate_physical_range(
        RspbootHeaderRange::UcodeData,
        header.ucode_data,
        header.ucode_data_size,
        true,
    )?;
    validate_physical_range(
        RspbootHeaderRange::CommandList,
        header.data_ptr,
        header.data_size,
        true,
    )?;

    let expected = encode_os_task_header(header);
    let actual = &rsp_memory.bank(RspMemoryBank::Dmem)[OS_TASK_DMEM_OFFSET..];
    if let Some(first_mismatch) = actual.iter().zip(expected).position(|(a, b)| a != &b) {
        return Err(AudioRspbootError::LoadedHeaderDmemMismatch { first_mismatch });
    }
    Ok(())
}

fn direct_imem_shape(header: OsTaskHeader) -> bool {
    let boot = header.ucode_boot & 0x1fff_ffff;
    let ucode = header.ucode & 0x1fff_ffff;
    let aligned_boot_size = header.ucode_boot_size.checked_add(7).map(|size| size & !7);
    boot == ucode
        && boot.is_multiple_of(8)
        && header.ucode_size != 0
        && header.ucode_size as usize <= RSP_BANK_BYTES
        && aligned_boot_size.is_some_and(|size| size != 0 && size >= header.ucode_size)
}

fn validate_physical_range(
    field: RspbootHeaderRange,
    raw_address: u32,
    byte_len: u32,
    empty_allowed: bool,
) -> Result<(), AudioRspbootError> {
    if byte_len == 0 && empty_allowed {
        return Ok(());
    }
    let address = raw_address & 0x00ff_ffff;
    if address >= DEFAULT_RDRAM_SIZE as u32 {
        return Err(AudioRspbootError::StaticAliasNotAllowed {
            field,
            address,
            byte_len,
        });
    }
    RdramByteRange::new(address, byte_len)
        .map(|_| ())
        .map_err(|source| match source {
            RdramRangeError::OutOfBounds { .. } => AudioRspbootError::StaticAliasNotAllowed {
                field,
                address,
                byte_len,
            },
            source => AudioRspbootError::HeaderRange { field, source },
        })
}

fn capture_logical_declared_bytes(
    storage: &[u8],
    field: RspbootHeaderRange,
    raw_address: u32,
    byte_len: u32,
) -> Result<Vec<u8>, AudioRspbootError> {
    if byte_len == 0 {
        return Ok(Vec::new());
    }
    validate_physical_range(field, raw_address, byte_len, false)?;
    let address = raw_address & 0x00ff_ffff;
    let view = RdramView::from_storage(storage);
    Ok((0..byte_len)
        .map(|offset| view.read_u8(RdramAddr::from_offset(address + offset)))
        .collect())
}

fn logical_imem_words(imem: &[u8; RSP_BANK_BYTES]) -> Vec<u32> {
    imem.chunks_exact(4)
        .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four IMEM bytes")))
        .collect()
}

fn collect_logical_rdram_effects(
    rdram_storage: &[u8],
    storage_ranges: Vec<(usize, usize)>,
) -> Result<(Vec<RdramByteRange>, CanonicalRdramPatches), AudioRspbootError> {
    let view = RdramView::from_storage(rdram_storage);
    let mut ranges = Vec::with_capacity(storage_ranges.len());
    let mut patches = Vec::with_capacity(storage_ranges.len());
    for (start, end) in storage_ranges {
        let byte_len = end
            .checked_sub(start)
            .ok_or(AudioRspbootError::RdramWriteRange { start, end })?;
        let start =
            u32::try_from(start).map_err(|_| AudioRspbootError::RdramWriteRange { start, end })?;
        let byte_len = u32::try_from(byte_len).map_err(|_| AudioRspbootError::RdramWriteRange {
            start: start as usize,
            end,
        })?;
        let range = RdramByteRange::new(start, byte_len).map_err(|_| {
            AudioRspbootError::RdramWriteRange {
                start: start as usize,
                end,
            }
        })?;
        let bytes = (range.start()..range.end())
            .map(|offset| view.read_u8(RdramAddr::from_offset(offset)))
            .collect();
        patches.push(RdramPatch::new(range.start(), bytes).map_err(AudioRspbootError::RdramPatch)?);
        ranges.push(range);
    }
    let patches =
        CanonicalRdramPatches::new(patches).map_err(AudioRspbootError::CanonicalRdramPatches)?;
    Ok((ranges, patches))
}

fn encode_os_task_header(header: OsTaskHeader) -> [u8; 64] {
    let fields = header_fields(header);
    let mut bytes = [0; 64];
    for (field, output) in fields.into_iter().zip(bytes.chunks_exact_mut(4)) {
        output.copy_from_slice(&field.to_be_bytes());
    }
    bytes
}

fn decode_os_task_header(bytes: &[u8]) -> OsTaskHeader {
    assert_eq!(
        bytes.len(),
        64,
        "OSTask entry image must be exactly 64 bytes"
    );
    let mut fields = [0u32; 16];
    for (field, input) in fields.iter_mut().zip(bytes.chunks_exact(4)) {
        *field = u32::from_be_bytes(input.try_into().expect("four OSTask bytes"));
    }
    OsTaskHeader {
        task_type: fields[0],
        flags: fields[1],
        ucode_boot: fields[2],
        ucode_boot_size: fields[3],
        ucode: fields[4],
        ucode_size: fields[5],
        ucode_data: fields[6],
        ucode_data_size: fields[7],
        dram_stack: fields[8],
        dram_stack_size: fields[9],
        output_buff: fields[10],
        output_buff_size: fields[11],
        data_ptr: fields[12],
        data_size: fields[13],
        yield_data_ptr: fields[14],
        yield_data_size: fields[15],
    }
}

fn header_fields(header: OsTaskHeader) -> [u32; 16] {
    [
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_runtime::{RdramViewMut, RspMemory};

    const HEADER: u32 = 0x40;
    const BOOT: u32 = 0x100;
    const UCODE: u32 = 0x180;
    const COMMANDS: u32 = 0x300;
    const UCODE_DATA: u32 = 0x380;
    const BREAK: u32 = 0x0000_000d;

    fn mtc0(rt: u32, rd: u32) -> u32 {
        (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11)
    }

    fn initial_machine_state(sp_status: u32) -> RspMachineState {
        let mut rdram = [0; 8];
        let mut machine = RspMachine::new(&mut rdram);
        machine.set_sp_status_raw(sp_status);
        machine.snapshot_state()
    }

    fn fixture() -> AudioRspbootInput {
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
        for (offset, word) in [0x2405_5678u32, BREAK].into_iter().enumerate() {
            rdram[UCODE as usize + offset * 4..UCODE as usize + offset * 4 + 4]
                .copy_from_slice(&word.to_ne_bytes());
        }
        let mut view = RdramViewMut::from_storage(&mut rdram);
        view.write_logical_bytes(RdramAddr::from_offset(COMMANDS), &[0; 8]);
        view.write_logical_bytes(RdramAddr::from_offset(UCODE_DATA), &[1, 2, 3, 4]);

        let mut rsp_memory = RspMemory::new();
        rsp_memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Imem, 0),
                &boot_words(&boot),
            )
            .unwrap();
        rsp_memory
            .write_bytes(
                RspMemAddr::from_register(OS_TASK_DMEM_OFFSET as u32),
                &encode_os_task_header(header),
            )
            .unwrap();
        AudioRspbootInput::new(
            RdramAddr::from_offset(HEADER),
            header,
            rdram,
            rsp_memory.snapshot(),
            0,
            initial_machine_state(SP_STATUS_HALT | SP_STATUS_BROKE),
        )
        .unwrap()
    }

    fn boot_words(words: &[u32; 8]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    #[test]
    fn stops_before_first_loaded_instruction_and_owns_every_mutable_image() {
        let input = fixture();
        let source = input.rdram_storage().to_vec();
        let result = execute_audio_rspboot_to_entry(input).unwrap();

        assert_eq!(result.entry().entry_pc_low12(), 0x80);
        assert_eq!(result.entry().machine_state().diagnostic_steps(), 7);
        assert_eq!(
            result.entry().machine_state().architectural_state().gprs()[5],
            0,
            "the loaded ADDIU must remain behind the entry boundary"
        );
        assert_eq!(
            source[UCODE as usize..UCODE as usize + 8],
            result.entry().rdram().storage()[UCODE as usize..UCODE as usize + 8]
        );
        assert!(result.boot_rdram_write_ranges().is_empty());
    }

    #[test]
    fn preserves_complete_future_visible_state_that_rspboot_does_not_write() {
        let mut input = fixture();
        let mut backing = [0; 8];
        let mut machine = RspMachine::new(&mut backing);
        machine.set_reg(31, 0x1357_9bdf);
        machine.mtc2(8, 0, 0x2468);
        machine.ctc2(0, 0xa55a);
        machine.set_sp_status_raw(SP_STATUS_HALT | SP_STATUS_BROKE | (1 << 7));
        assert_eq!(machine.read_cp0(7), 0);
        assert_eq!(machine.write_cp0(3, 7), None);
        assert_eq!(machine.write_cp0(8, 0x40), None);
        assert_eq!(machine.write_cp0(11, 1 << 3), None);
        let initial = machine.snapshot_state();
        input.initial_machine_state = initial.clone();

        let result = execute_audio_rspboot_to_entry(input).unwrap();
        let final_state = result.entry().machine_state().architectural_state();
        let initial_state = initial.architectural_state();
        assert_eq!(final_state.gprs()[31], 0x1357_9bdf);
        assert_eq!(final_state.vu(), initial_state.vu());
        assert!(final_state.sp_semaphore());
        assert_eq!(final_state.sp_status(), 1 << 7);
        assert_eq!(final_state.dma_write_length(), 7);
        assert_eq!(final_state.dp_start(), 0x40);
        assert_eq!(final_state.dp_current(), 0x40);
        assert_eq!(final_state.dp_status(), 1 << 1);
    }

    #[test]
    fn rejects_nonzero_initial_diagnostic_accounting() {
        let valid = fixture();
        let mut backing = [0; 8];
        let mut machine = RspMachine::new(&mut backing);
        machine.ctx.steps = 1;
        assert!(matches!(
            AudioRspbootInput::new(
                valid.task_addr(),
                valid.loaded_header(),
                valid.rdram_storage().to_vec(),
                valid.rsp_memory().clone(),
                0,
                machine.snapshot_state(),
            ),
            Err(AudioRspbootError::InitialDiagnosticSteps { steps: 1 })
        ));
    }

    #[test]
    fn rejects_a_stale_initial_execution_continuation() {
        let valid = fixture();
        let mut backing = [0; 8];
        let mut machine = RspMachine::new(&mut backing);
        machine.ctx.resume_address = 0x1080;
        assert!(matches!(
            AudioRspbootInput::new(
                valid.task_addr(),
                valid.loaded_header(),
                valid.rdram_storage().to_vec(),
                valid.rsp_memory().clone(),
                0,
                machine.snapshot_state(),
            ),
            Err(AudioRspbootError::InitialExecutionContinuation {
                resume_address: 0x1080,
                ..
            })
        ));
    }

    #[test]
    fn rejects_pending_initial_dpc_work() {
        let valid = fixture();
        let mut backing = [0; 8];
        let mut machine = RspMachine::new(&mut backing);
        assert_eq!(machine.write_cp0(9, 8), None);
        assert!(matches!(
            AudioRspbootInput::new(
                valid.task_addr(),
                valid.loaded_header(),
                valid.rdram_storage().to_vec(),
                valid.rsp_memory().clone(),
                0,
                machine.snapshot_state(),
            ),
            Err(AudioRspbootError::InitialPendingDpcSubmissions { count: 1 })
        ));
    }

    #[test]
    fn rspboot_dpc_work_is_a_typed_failure() {
        let mut input = fixture();
        let boot = [
            0x2402_0000,
            mtc0(2, 8),
            0x2402_0008,
            mtc0(2, 9),
            0x2402_0000 | UCODE,
            mtc0(2, 1),
            0x2403_1080,
            mtc0(3, 0),
            0x2404_0007,
            mtc0(4, 2),
            0x0800_0020,
            0x2407_7777,
        ];
        let mut memory = RspMemory::from_snapshot(input.rsp_memory);
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Imem, 0),
                &boot
                    .iter()
                    .flat_map(|word| word.to_be_bytes())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        input.rsp_memory = memory.snapshot();

        assert!(matches!(
            execute_audio_rspboot_to_entry(input),
            Err(AudioRspbootError::RspbootDpcSubmissions { count: 1 })
        ));
    }

    #[test]
    fn boot_dma_writes_are_returned_without_mutating_the_source_image() {
        const OUTPUT: u32 = 0x400;
        const DMEM_SOURCE: u16 = 0x100;
        let mut input = fixture();
        let source = input.rdram_storage.clone();
        let boot = [
            0x2402_0000 | OUTPUT,
            mtc0(2, 1),
            0x2403_0000 | u32::from(DMEM_SOURCE),
            mtc0(3, 0),
            0x2404_0007,
            mtc0(4, 3),
            0x2402_0000 | UCODE,
            mtc0(2, 1),
            0x2403_1080,
            mtc0(3, 0),
            mtc0(4, 2),
            0x0800_0020,
            0x2407_7777,
        ];
        let mut memory = RspMemory::from_snapshot(input.rsp_memory);
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Imem, 0),
                &boot
                    .iter()
                    .flat_map(|word| word.to_be_bytes())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Dmem, DMEM_SOURCE),
                &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
            )
            .unwrap();
        input.rsp_memory = memory.snapshot();

        let result = execute_audio_rspboot_to_entry(input).unwrap();
        assert_eq!(&source[OUTPUT as usize..OUTPUT as usize + 8], &[0; 8]);
        assert_eq!(
            result.boot_rdram_write_ranges(),
            &[RdramByteRange::new(OUTPUT, 8).unwrap()]
        );
        assert_eq!(
            result.boot_rdram_patches().as_slice()[0].bytes(),
            &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
        );
        let view = result.entry().rdram().view();
        assert_eq!(
            (0..8)
                .map(|offset| view.read_u8(RdramAddr::from_offset(OUTPUT + offset)))
                .collect::<Vec<_>>(),
            [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
        );
    }

    #[test]
    fn preserves_loaded_header_separately_from_the_entry_header() {
        let result = execute_audio_rspboot_to_entry(fixture()).unwrap();
        assert_eq!(
            result.entry().loaded_header(),
            result.entry().entry_header()
        );
        assert_eq!(result.entry().loaded_header().task_type, M_AUDTASK);
    }

    #[test]
    fn replacement_generation_and_image_are_exact() {
        let initial_generation = fixture().rsp_memory().imem_generation();
        let result = execute_audio_rspboot_to_entry(fixture()).unwrap();
        assert_eq!(result.imem_replacements().len(), 1);
        let replacement = &result.imem_replacements()[0];
        assert_eq!(replacement.generation(), initial_generation + 1);
        assert_eq!(
            result.entry().rsp_memory().imem_generation(),
            replacement.generation()
        );
        assert_eq!(
            replacement.identity(),
            Sha256Digest::hash(replacement.image())
        );
        assert_eq!(
            &replacement.image()[0x80..0x88],
            &[0x24, 0x05, 0x56, 0x78, 0, 0, 0, 0x0d]
        );
    }

    #[test]
    fn rejects_a_declared_static_alias_before_execution() {
        let valid = fixture();
        let mut header = valid.loaded_header();
        header.ucode = 0x00f0_0000;
        let mut memory = RspMemory::from_snapshot(valid.rsp_memory().clone());
        memory
            .write_bytes(
                RspMemAddr::from_register(OS_TASK_DMEM_OFFSET as u32),
                &encode_os_task_header(header),
            )
            .unwrap();
        assert!(matches!(
            AudioRspbootInput::new(
                valid.task_addr(),
                header,
                valid.rdram_storage().to_vec(),
                memory.snapshot(),
                0,
                initial_machine_state(0),
            ),
            Err(AudioRspbootError::StaticAliasNotAllowed {
                field: RspbootHeaderRange::Ucode,
                ..
            })
        ));
    }

    #[test]
    fn early_break_is_a_typed_failure() {
        let valid = fixture();
        let mut memory = RspMemory::from_snapshot(valid.rsp_memory().clone());
        memory
            .write_word(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), BREAK)
            .unwrap();
        let input = AudioRspbootInput::new(
            valid.task_addr(),
            valid.loaded_header(),
            valid.rdram_storage().to_vec(),
            memory.snapshot(),
            0,
            initial_machine_state(0),
        )
        .unwrap();
        assert!(matches!(
            execute_audio_rspboot_to_entry(input),
            Err(AudioRspbootError::EarlyBreak { steps: 1, .. })
        ));
    }

    #[test]
    fn direct_imem_is_typed_unsupported() {
        let valid = fixture();
        let mut header = valid.loaded_header();
        header.ucode_boot = UCODE;
        header.ucode_boot_size = 8;
        let mut memory = RspMemory::from_snapshot(valid.rsp_memory().clone());
        memory
            .write_bytes(
                RspMemAddr::from_register(OS_TASK_DMEM_OFFSET as u32),
                &encode_os_task_header(header),
            )
            .unwrap();
        assert!(matches!(
            AudioRspbootInput::new(
                valid.task_addr(),
                header,
                valid.rdram_storage().to_vec(),
                memory.snapshot(),
                0,
                initial_machine_state(0),
            ),
            Err(AudioRspbootError::DirectImemUnsupported)
        ));
    }
}
