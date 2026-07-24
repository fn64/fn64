//! Owned post-rspboot state for transactional audio HLE/LLE lanes.
//!
//! This module is an ownership and admission boundary, not an executor. It
//! models the exact state at the first instruction of an audio microcode image
//! so a catalog-neutral LLE lane and, after exact admission, HLE/differential
//! lanes can start from independent deep copies. No live pointer or host-only
//! static alias is retained.
//!
//! Provenance: the complete `OSTask` shape and audio task type come from the
//! public libultra manual; the 4 KiB memory banks, IMEM PC domain, and SP DMA
//! address geometry come from the public SGI *Nintendo 64 RSP Programmer's
//! Guide*. Microcode-family admission remains repository-owned clean-room
//! policy and enters only through [`crate::hle::AdmittedAudioMicrocode`].
//!
//! The live rspboot runner still lives in `fn64-abi`, so this module validates
//! caller-supplied boundary state but cannot prove that rspboot historically
//! produced it. Moving a pure capture runner into `fn64-audio` is a mandatory
//! release frontier. Its first differential lane must admit only physical
//! `0..8 MiB` DMA and trap if a task actually reaches a host static alias.

use core::num::NonZeroU64;
use std::ops::Range;

use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
use fn64_runtime::rsp::RspMemorySnapshot;
use fn64_runtime::{OsTaskHeader, RdramAddr, RdramView, RspMemoryBank, M_AUDTASK};

use crate::hle::AdmittedAudioMicrocode;
use crate::hle_outcome::{
    AudioHleSelection, AudioMicrocodeIdentity, RdramByteRange, RdramRangeError, Sha256Digest,
    RSP_BANK_BYTES,
};
use crate::hle_transaction::AudioHleTaskTransaction;
use crate::rsp::runtime::RspMachineState;

const OS_TASK_HEADER_BYTES: u32 = 16 * 4;
const OS_TASK_DMEM_OFFSET: usize = RSP_BANK_BYTES - OS_TASK_HEADER_BYTES as usize;
const ABI_COMMAND_BYTES: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotByteImage {
    EntryTaskHeader,
    CommandList,
    UcodeData,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderRangeField {
    TaskHeader,
    CommandList,
    UcodeData,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioHleSnapshotError {
    PhysicalRdramStorageLength {
        storage_bytes: usize,
        required_bytes: usize,
    },
    TaskAddressUnaligned {
        address: u32,
    },
    HeaderRange {
        field: HeaderRangeField,
        source: RdramRangeError,
    },
    StaticAliasNotAllowed {
        field: HeaderRangeField,
        address: u32,
        byte_len: u32,
    },
    NonAudioTask {
        loaded_task_type: u32,
        entry_task_type: u32,
    },
    CommandAddressUnaligned {
        address: u32,
    },
    PartialCommand {
        byte_len: u32,
    },
    ByteLengthMismatch {
        image: SnapshotByteImage,
        header_bytes: u32,
        supplied_bytes: usize,
    },
    LogicalBytesMismatch {
        image: SnapshotByteImage,
        first_mismatch: u32,
    },
    MicrocodeIdentityMismatch {
        component: MicrocodeIdentityMismatch,
    },
    EntryPcUnaligned {
        pc: u32,
    },
    EntryPcOutsideFabricRange {
        pc: u32,
    },
    EntryPcResumeMismatch {
        entry_pc_low12: u32,
        resume_pc_low12: u32,
    },
    ZeroRspbootSteps,
    RspbootStepAccountingMismatch {
        rspboot_steps: u64,
        diagnostic_steps: u64,
    },
    NoAdmittedDmaRanges,
    EmptyDmaRange {
        start: usize,
        end: usize,
    },
    StaticDmaAliasNotAllowed {
        start: usize,
        end: usize,
        physical_bytes: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicrocodeIdentityMismatch {
    ImemDigest {
        selected: Sha256Digest,
        captured: Sha256Digest,
    },
    UcodeDataLength {
        selected: u32,
        captured: u32,
    },
    UcodeDataDigest {
        selected: Sha256Digest,
        captured: Sha256Digest,
    },
}

/// Caller-supplied parts claiming one post-rspboot audio task boundary.
///
/// `rdram_storage` is the native-word byte storage used by generated code,
/// not a flattened big-endian image. The command and ucode-data vectors are
/// separately retained in logical guest byte order and checked against it.
/// Construction validates internal consistency; it is not historical proof
/// that a runner executed rspboot.
#[derive(Clone, Debug)]
pub struct PostRspbootAudioTaskParts {
    pub task_addr: RdramAddr,
    pub loaded_header: OsTaskHeader,
    pub entry_header: OsTaskHeader,
    pub command_bytes: Vec<u8>,
    pub ucode_data_bytes: Vec<u8>,
    pub rdram_storage: Vec<u8>,
    pub rsp_memory: RspMemorySnapshot,
    pub machine_state: RspMachineState,
    pub entry_pc_low12: u32,
    pub rspboot_steps: u64,
    pub admitted_dma_ranges: Vec<Range<usize>>,
}

/// An exact-size owned copy of fn64's native-word physical RDRAM storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeWordRdramSnapshot {
    storage: Box<[u8]>,
}

impl NativeWordRdramSnapshot {
    fn new(storage: Vec<u8>) -> Result<Self, AudioHleSnapshotError> {
        if storage.len() != DEFAULT_RDRAM_SIZE {
            return Err(AudioHleSnapshotError::PhysicalRdramStorageLength {
                storage_bytes: storage.len(),
                required_bytes: DEFAULT_RDRAM_SIZE,
            });
        }
        Ok(Self {
            storage: storage.into_boxed_slice(),
        })
    }

    pub fn storage(&self) -> &[u8] {
        &self.storage
    }

    pub fn view(&self) -> RdramView<'_> {
        RdramView::from_storage(&self.storage)
    }

    fn into_storage(self) -> Vec<u8> {
        self.storage.into_vec()
    }
}

/// Internally consistent, pointer-free claimed post-rspboot boundary state.
///
/// This type is deliberately catalog-neutral. Capturing an unknown microcode
/// is sufficient to run the authoritative LLE path; family policy enters only
/// through [`AudioTaskEntrySnapshot::admit_hle`].
#[derive(Clone, Debug)]
pub struct AudioTaskEntrySnapshot {
    task_addr: RdramAddr,
    loaded_header: OsTaskHeader,
    entry_header: OsTaskHeader,
    command_bytes: Vec<u8>,
    ucode_data_bytes: Vec<u8>,
    identity: AudioMicrocodeIdentity,
    rdram: NativeWordRdramSnapshot,
    rsp_memory: RspMemorySnapshot,
    machine_state: RspMachineState,
    entry_pc_low12: u32,
    rspboot_steps: NonZeroU64,
    admitted_dma_ranges: Vec<Range<usize>>,
}

impl AudioTaskEntrySnapshot {
    pub fn from_post_rspboot(
        parts: PostRspbootAudioTaskParts,
    ) -> Result<Self, AudioHleSnapshotError> {
        let PostRspbootAudioTaskParts {
            task_addr,
            loaded_header,
            entry_header,
            command_bytes,
            ucode_data_bytes,
            rdram_storage,
            rsp_memory,
            machine_state,
            entry_pc_low12,
            rspboot_steps,
            admitted_dma_ranges,
        } = parts;

        let rdram = NativeWordRdramSnapshot::new(rdram_storage)?;
        validate_task_address(task_addr)?;
        validate_headers(loaded_header, entry_header)?;
        validate_entry_control(entry_pc_low12, &machine_state)?;
        let rspboot_steps =
            NonZeroU64::new(rspboot_steps).ok_or(AudioHleSnapshotError::ZeroRspbootSteps)?;
        if machine_state.diagnostic_steps() != rspboot_steps.get() {
            return Err(AudioHleSnapshotError::RspbootStepAccountingMismatch {
                rspboot_steps: rspboot_steps.get(),
                diagnostic_steps: machine_state.diagnostic_steps(),
            });
        }
        let admitted_dma_ranges = validate_dma_ranges(admitted_dma_ranges)?;

        let entry_header_bytes = encode_os_task_header(entry_header);
        let dmem = rsp_memory.bank(RspMemoryBank::Dmem);
        validate_flat_bytes(
            SnapshotByteImage::EntryTaskHeader,
            &dmem[OS_TASK_DMEM_OFFSET..],
            &entry_header_bytes,
        )?;

        validate_declared_bytes(
            rdram.view(),
            HeaderRangeField::CommandList,
            SnapshotByteImage::CommandList,
            entry_header.data_ptr,
            entry_header.data_size,
            &command_bytes,
        )?;
        if !entry_header.data_ptr.is_multiple_of(ABI_COMMAND_BYTES) {
            return Err(AudioHleSnapshotError::CommandAddressUnaligned {
                address: entry_header.data_ptr,
            });
        }
        if !entry_header.data_size.is_multiple_of(ABI_COMMAND_BYTES) {
            return Err(AudioHleSnapshotError::PartialCommand {
                byte_len: entry_header.data_size,
            });
        }

        validate_declared_bytes(
            rdram.view(),
            HeaderRangeField::UcodeData,
            SnapshotByteImage::UcodeData,
            entry_header.ucode_data,
            entry_header.ucode_data_size,
            &ucode_data_bytes,
        )?;
        let identity = AudioMicrocodeIdentity::from_task_entry(
            rsp_memory.bank(RspMemoryBank::Imem),
            &ucode_data_bytes,
        )
        .expect("OSTask ucode-data length is represented by u32");

        Ok(Self {
            task_addr,
            loaded_header,
            entry_header,
            command_bytes,
            ucode_data_bytes,
            identity,
            rdram,
            rsp_memory,
            machine_state,
            entry_pc_low12,
            rspboot_steps,
            admitted_dma_ranges,
        })
    }

    /// Deep-copy every mutable execution owner for one isolated LLE lane.
    ///
    /// No catalog admission is needed: LLE executes the captured microcode,
    /// rather than assigning it family-specific host semantics.
    pub fn fork_lle_lane(&self) -> AudioLleLane {
        AudioLleLane {
            state: self.clone(),
        }
    }

    /// Apply exact catalog admission before exposing any HLE operation.
    pub fn admit_hle(
        self,
        admission: AdmittedAudioMicrocode,
    ) -> Result<AdmittedAudioHleTaskSnapshot, AudioHleSnapshotError> {
        if admission.identity() != self.identity {
            return Err(AudioHleSnapshotError::MicrocodeIdentityMismatch {
                component: identity_mismatch(admission.identity(), self.identity),
            });
        }
        Ok(AdmittedAudioHleTaskSnapshot {
            entry: self,
            admission,
        })
    }

    pub const fn task_addr(&self) -> RdramAddr {
        self.task_addr
    }

    pub const fn loaded_header(&self) -> OsTaskHeader {
        self.loaded_header
    }

    pub const fn entry_header(&self) -> OsTaskHeader {
        self.entry_header
    }

    pub fn command_bytes(&self) -> &[u8] {
        &self.command_bytes
    }

    pub fn ucode_data_bytes(&self) -> &[u8] {
        &self.ucode_data_bytes
    }

    pub const fn identity(&self) -> AudioMicrocodeIdentity {
        self.identity
    }

    pub const fn rdram(&self) -> &NativeWordRdramSnapshot {
        &self.rdram
    }

    pub const fn rsp_memory(&self) -> &RspMemorySnapshot {
        &self.rsp_memory
    }

    pub const fn machine_state(&self) -> &RspMachineState {
        &self.machine_state
    }

    /// Canonical fabric/SP_PC form. The interpreter maps it into IMEM by
    /// adding the `0x1000` bank selector when execution begins.
    pub const fn entry_pc_low12(&self) -> u32 {
        self.entry_pc_low12
    }

    pub const fn rspboot_steps(&self) -> NonZeroU64 {
        self.rspboot_steps
    }

    pub fn admitted_dma_ranges(&self) -> &[Range<usize>] {
        &self.admitted_dma_ranges
    }

    fn into_lle_parts(self) -> AudioLleLaneParts {
        AudioLleLaneParts {
            task_addr: self.task_addr,
            loaded_header: self.loaded_header,
            entry_header: self.entry_header,
            command_bytes: self.command_bytes,
            ucode_data_bytes: self.ucode_data_bytes,
            identity: self.identity,
            rdram_storage: self.rdram.into_storage(),
            rsp_memory: self.rsp_memory,
            machine_state: self.machine_state,
            entry_pc_low12: self.entry_pc_low12,
            rspboot_steps: self.rspboot_steps.get(),
            admitted_dma_ranges: self.admitted_dma_ranges,
        }
    }
}

/// Fully owned state consumed by the LLE runner.
///
/// Its fields are private so callers cannot forge a supposedly validated lane.
/// The LLE runner obtains the raw owners only by consuming this value.
#[derive(Clone, Debug)]
pub struct AudioLleLaneParts {
    task_addr: RdramAddr,
    loaded_header: OsTaskHeader,
    entry_header: OsTaskHeader,
    command_bytes: Vec<u8>,
    ucode_data_bytes: Vec<u8>,
    identity: AudioMicrocodeIdentity,
    rdram_storage: Vec<u8>,
    rsp_memory: RspMemorySnapshot,
    machine_state: RspMachineState,
    entry_pc_low12: u32,
    rspboot_steps: u64,
    admitted_dma_ranges: Vec<Range<usize>>,
}

#[derive(Debug)]
pub(crate) struct AudioLleExecutionParts {
    pub(crate) task_addr: RdramAddr,
    pub(crate) loaded_header: OsTaskHeader,
    pub(crate) entry_header: OsTaskHeader,
    pub(crate) command_bytes: Vec<u8>,
    pub(crate) ucode_data_bytes: Vec<u8>,
    pub(crate) identity: AudioMicrocodeIdentity,
    pub(crate) rdram_storage: Vec<u8>,
    pub(crate) rsp_memory: RspMemorySnapshot,
    pub(crate) machine_state: RspMachineState,
    pub(crate) entry_pc_low12: u32,
    pub(crate) rspboot_steps: u64,
    pub(crate) admitted_dma_ranges: Vec<Range<usize>>,
    _validated: ValidatedLaneSeal,
}

#[derive(Debug)]
struct ValidatedLaneSeal;

impl AudioLleLaneParts {
    pub(crate) fn rdram_storage(&self) -> &[u8] {
        &self.rdram_storage
    }

    pub(crate) const fn machine_state(&self) -> &RspMachineState {
        &self.machine_state
    }

    pub(crate) const fn entry_pc_low12(&self) -> u32 {
        self.entry_pc_low12
    }

    pub(crate) const fn rspboot_steps(&self) -> u64 {
        self.rspboot_steps
    }

    pub(crate) fn admitted_dma_ranges(&self) -> &[Range<usize>] {
        &self.admitted_dma_ranges
    }

    pub(crate) fn into_execution_parts(self) -> AudioLleExecutionParts {
        AudioLleExecutionParts {
            task_addr: self.task_addr,
            loaded_header: self.loaded_header,
            entry_header: self.entry_header,
            command_bytes: self.command_bytes,
            ucode_data_bytes: self.ucode_data_bytes,
            identity: self.identity,
            rdram_storage: self.rdram_storage,
            rsp_memory: self.rsp_memory,
            machine_state: self.machine_state,
            entry_pc_low12: self.entry_pc_low12,
            rspboot_steps: self.rspboot_steps,
            admitted_dma_ranges: self.admitted_dma_ranges,
            _validated: ValidatedLaneSeal,
        }
    }
}

/// One independently owned mutable LLE lane forked without catalog policy.
#[derive(Clone, Debug)]
pub struct AudioLleLane {
    state: AudioTaskEntrySnapshot,
}

impl AudioLleLane {
    pub const fn snapshot(&self) -> &AudioTaskEntrySnapshot {
        &self.state
    }

    pub fn into_lle_parts(self) -> AudioLleLaneParts {
        self.state.into_lle_parts()
    }
}

/// Exact-family admission wrapped around a neutral task-entry snapshot.
#[derive(Clone, Debug)]
pub struct AdmittedAudioHleTaskSnapshot {
    entry: AudioTaskEntrySnapshot,
    admission: AdmittedAudioMicrocode,
}

impl AdmittedAudioHleTaskSnapshot {
    pub const fn entry(&self) -> &AudioTaskEntrySnapshot {
        &self.entry
    }

    pub const fn admission(&self) -> AdmittedAudioMicrocode {
        self.admission
    }

    pub const fn selection(&self) -> AudioHleSelection {
        self.admission.selection()
    }

    /// Deep-copy every mutable owner for an admitted HLE lane.
    pub fn fork_hle_lane(&self) -> AudioHleLane {
        AudioHleLane {
            state: self.clone(),
        }
    }

    /// Differential LLE starts from the same neutral entry state.
    pub fn fork_lle_lane(&self) -> AudioLleLane {
        self.entry.fork_lle_lane()
    }
}

/// Compatibility name for the explicitly admitted HLE snapshot.
pub type AudioHleTaskSnapshot = AdmittedAudioHleTaskSnapshot;

/// One independently owned mutable HLE execution lane.
#[derive(Clone, Debug)]
pub struct AudioHleLane {
    state: AdmittedAudioHleTaskSnapshot,
}

impl AudioHleLane {
    pub const fn snapshot(&self) -> &AdmittedAudioHleTaskSnapshot {
        &self.state
    }

    /// Begin a side-effect-free HLE transaction over logical guest bytes.
    pub fn hle_transaction(&self) -> AudioHleTaskTransaction<'_> {
        AudioHleTaskTransaction::new(self.state.entry.rdram.view())
            .expect("validated lane always owns complete physical RDRAM")
    }

    pub const fn selection(&self) -> AudioHleSelection {
        self.state.selection()
    }
}

fn validate_task_address(task_addr: RdramAddr) -> Result<(), AudioHleSnapshotError> {
    if !task_addr.offset().is_multiple_of(8) {
        return Err(AudioHleSnapshotError::TaskAddressUnaligned {
            address: task_addr.offset(),
        });
    }
    RdramByteRange::new(task_addr.offset(), OS_TASK_HEADER_BYTES)
        .map(|_| ())
        .map_err(|source| AudioHleSnapshotError::HeaderRange {
            field: HeaderRangeField::TaskHeader,
            source,
        })
}

fn validate_headers(
    loaded: OsTaskHeader,
    entry: OsTaskHeader,
) -> Result<(), AudioHleSnapshotError> {
    if loaded.task_type != M_AUDTASK || entry.task_type != M_AUDTASK {
        return Err(AudioHleSnapshotError::NonAudioTask {
            loaded_task_type: loaded.task_type,
            entry_task_type: entry.task_type,
        });
    }
    Ok(())
}

fn identity_mismatch(
    selected: AudioMicrocodeIdentity,
    captured: AudioMicrocodeIdentity,
) -> MicrocodeIdentityMismatch {
    if selected.imem_sha256 != captured.imem_sha256 {
        MicrocodeIdentityMismatch::ImemDigest {
            selected: selected.imem_sha256,
            captured: captured.imem_sha256,
        }
    } else if selected.ucode_data_bytes != captured.ucode_data_bytes {
        MicrocodeIdentityMismatch::UcodeDataLength {
            selected: selected.ucode_data_bytes,
            captured: captured.ucode_data_bytes,
        }
    } else {
        MicrocodeIdentityMismatch::UcodeDataDigest {
            selected: selected.ucode_data_sha256,
            captured: captured.ucode_data_sha256,
        }
    }
}

fn validate_entry_control(
    entry_pc_low12: u32,
    machine_state: &RspMachineState,
) -> Result<(), AudioHleSnapshotError> {
    if !entry_pc_low12.is_multiple_of(4) {
        return Err(AudioHleSnapshotError::EntryPcUnaligned { pc: entry_pc_low12 });
    }
    if entry_pc_low12 > 0x0ffc {
        return Err(AudioHleSnapshotError::EntryPcOutsideFabricRange { pc: entry_pc_low12 });
    }

    // A pending resume is installed when rspboot exits on the IMEM DMA that
    // loaded the selected ucode. `run_imem` consumes that value before its
    // first fetch, so its low-12 target and the fabric PC must name the same
    // entry instruction. A zero resume means control was already committed to
    // the fabric PC and supplies no duplicate value to compare.
    let resume_address = machine_state.architectural_state().resume_address();
    if resume_address != 0 {
        let resume_pc_low12 = resume_address & 0x0fff;
        if resume_pc_low12 != entry_pc_low12 {
            return Err(AudioHleSnapshotError::EntryPcResumeMismatch {
                entry_pc_low12,
                resume_pc_low12,
            });
        }
    }
    Ok(())
}

fn validate_dma_ranges(
    mut ranges: Vec<Range<usize>>,
) -> Result<Vec<Range<usize>>, AudioHleSnapshotError> {
    if ranges.is_empty() {
        return Err(AudioHleSnapshotError::NoAdmittedDmaRanges);
    }
    for range in &ranges {
        if range.start >= range.end {
            return Err(AudioHleSnapshotError::EmptyDmaRange {
                start: range.start,
                end: range.end,
            });
        }
        if range.end > DEFAULT_RDRAM_SIZE {
            return Err(AudioHleSnapshotError::StaticDmaAliasNotAllowed {
                start: range.start,
                end: range.end,
                physical_bytes: DEFAULT_RDRAM_SIZE,
            });
        }
    }

    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    let mut canonical: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(previous) = canonical.last_mut() {
            if range.start <= previous.end {
                previous.end = previous.end.max(range.end);
                continue;
            }
        }
        canonical.push(range);
    }
    Ok(canonical)
}

fn validate_declared_bytes(
    rdram: RdramView<'_>,
    field: HeaderRangeField,
    image: SnapshotByteImage,
    raw_address: u32,
    declared_bytes: u32,
    bytes: &[u8],
) -> Result<(), AudioHleSnapshotError> {
    if bytes.len() != declared_bytes as usize {
        return Err(AudioHleSnapshotError::ByteLengthMismatch {
            image,
            header_bytes: declared_bytes,
            supplied_bytes: bytes.len(),
        });
    }
    if declared_bytes == 0 {
        return Ok(());
    }

    let address = raw_address & 0x00ff_ffff;
    if address >= DEFAULT_RDRAM_SIZE as u32 {
        return Err(AudioHleSnapshotError::StaticAliasNotAllowed {
            field,
            address,
            byte_len: declared_bytes,
        });
    }
    validate_rdram_bytes(rdram, field, image, address, bytes)
}

fn validate_rdram_bytes(
    rdram: RdramView<'_>,
    field: HeaderRangeField,
    image: SnapshotByteImage,
    address: u32,
    bytes: &[u8],
) -> Result<(), AudioHleSnapshotError> {
    let byte_len = u32::try_from(bytes.len()).expect("validated snapshot byte length fits u32");
    RdramByteRange::new(address, byte_len).map_err(|source| {
        if matches!(source, RdramRangeError::OutOfBounds { .. }) {
            AudioHleSnapshotError::StaticAliasNotAllowed {
                field,
                address,
                byte_len,
            }
        } else {
            AudioHleSnapshotError::HeaderRange { field, source }
        }
    })?;

    for (offset, &expected) in bytes.iter().enumerate() {
        let offset = u32::try_from(offset).expect("validated snapshot byte length fits u32");
        let actual = rdram.read_u8(RdramAddr::from_offset(address + offset));
        if actual != expected {
            return Err(AudioHleSnapshotError::LogicalBytesMismatch {
                image,
                first_mismatch: offset,
            });
        }
    }
    Ok(())
}

fn validate_flat_bytes(
    image: SnapshotByteImage,
    actual: &[u8],
    expected: &[u8],
) -> Result<(), AudioHleSnapshotError> {
    if let Some(first_mismatch) = actual
        .iter()
        .zip(expected)
        .position(|(actual, expected)| actual != expected)
    {
        return Err(AudioHleSnapshotError::LogicalBytesMismatch {
            image,
            first_mismatch: first_mismatch as u32,
        });
    }
    Ok(())
}

fn encode_os_task_header(header: OsTaskHeader) -> [u8; OS_TASK_HEADER_BYTES as usize] {
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
    let mut bytes = [0; OS_TASK_HEADER_BYTES as usize];
    for (field, output) in fields.into_iter().zip(bytes.chunks_exact_mut(4)) {
        output.copy_from_slice(&field.to_be_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hle::{AudioHleCatalog, AudioHleCatalogEntry};
    use crate::hle_outcome::AudioHleFamily;
    use crate::rsp::runtime::RspMachine;
    use fn64_runtime::{RdramViewMut, RspMemAddr, RspMemory};

    const TASK_ADDR: u32 = 0x100;
    const UCODE_DATA_ADDR: u32 = 0x300;
    const COMMAND_ADDR: u32 = 0x400;

    fn valid_parts() -> PostRspbootAudioTaskParts {
        let header = OsTaskHeader {
            task_type: M_AUDTASK,
            ucode_boot: 0x8000_0800,
            ucode_boot_size: 0x80,
            ucode: 0x8000_1000,
            ucode_size: RSP_BANK_BYTES as u32,
            ucode_data: 0x8000_0000 | UCODE_DATA_ADDR,
            ucode_data_size: 4,
            data_ptr: 0x8000_0000 | COMMAND_ADDR,
            data_size: 8,
            ..OsTaskHeader::default()
        };
        let command_bytes = vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80];
        let ucode_data_bytes = vec![0xaa, 0xbb, 0xcc, 0xdd];
        let mut rdram_storage = vec![0; DEFAULT_RDRAM_SIZE];
        {
            let mut rdram = RdramViewMut::from_storage(&mut rdram_storage);
            rdram.write_logical_bytes(
                RdramAddr::from_offset(TASK_ADDR),
                &encode_os_task_header(header),
            );
            rdram.write_logical_bytes(RdramAddr::from_offset(COMMAND_ADDR), &command_bytes);
            rdram.write_logical_bytes(RdramAddr::from_offset(UCODE_DATA_ADDR), &ucode_data_bytes);
        }

        let mut rsp_memory = RspMemory::new();
        rsp_memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Dmem, OS_TASK_DMEM_OFFSET as u16),
                &encode_os_task_header(header),
            )
            .unwrap();
        let mut imem = [0; RSP_BANK_BYTES];
        imem[0..4].copy_from_slice(&0x0d00_0000u32.to_be_bytes());
        rsp_memory
            .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &imem)
            .unwrap();
        let machine_state = {
            let mut machine = RspMachine::new(&mut rdram_storage);
            machine.ctx.steps = 1;
            machine.snapshot_state()
        };

        PostRspbootAudioTaskParts {
            task_addr: RdramAddr::from_offset(TASK_ADDR),
            loaded_header: header,
            entry_header: header,
            command_bytes,
            ucode_data_bytes,
            rdram_storage,
            rsp_memory: rsp_memory.snapshot(),
            machine_state,
            entry_pc_low12: 0,
            rspboot_steps: 1,
            admitted_dma_ranges: std::iter::once(0..DEFAULT_RDRAM_SIZE).collect(),
        }
    }

    fn admission_for(
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
    fn captures_complete_catalog_neutral_post_rspboot_snapshot() {
        let snapshot = AudioTaskEntrySnapshot::from_post_rspboot(valid_parts()).unwrap();
        assert_eq!(snapshot.task_addr().offset(), TASK_ADDR);
        assert_eq!(snapshot.command_bytes().len(), 8);
        assert_eq!(snapshot.ucode_data_bytes(), &[0xaa, 0xbb, 0xcc, 0xdd]);
        assert_eq!(snapshot.entry_pc_low12(), 0);
        assert_eq!(snapshot.rspboot_steps().get(), 1);
        assert_eq!(snapshot.admitted_dma_ranges().len(), 1);
        assert_eq!(snapshot.admitted_dma_ranges()[0], 0..DEFAULT_RDRAM_SIZE);
        assert_eq!(snapshot.rdram().storage().len(), DEFAULT_RDRAM_SIZE);
    }

    #[test]
    fn unknown_identity_still_forks_an_lle_lane_without_catalog_admission() {
        let snapshot = AudioTaskEntrySnapshot::from_post_rspboot(valid_parts()).unwrap();
        let empty = AudioHleCatalog::new(&[]).unwrap();

        assert!(empty.admit(snapshot.identity()).is_err());
        assert_eq!(
            snapshot.fork_lle_lane().snapshot().identity(),
            snapshot.identity()
        );
    }

    #[test]
    fn hle_wrapper_requires_exact_identity_admission() {
        let snapshot = AudioTaskEntrySnapshot::from_post_rspboot(valid_parts()).unwrap();
        let exact_identity = snapshot.identity();
        let mut other_parts = valid_parts();
        other_parts.ucode_data_bytes[0] ^= 0xff;
        RdramViewMut::from_storage(&mut other_parts.rdram_storage).write_logical_bytes(
            RdramAddr::from_offset(UCODE_DATA_ADDR),
            &other_parts.ucode_data_bytes,
        );
        let other_identity = AudioTaskEntrySnapshot::from_post_rspboot(other_parts)
            .unwrap()
            .identity();

        assert!(matches!(
            snapshot.clone().admit_hle(admission_for(other_identity, 9)),
            Err(AudioHleSnapshotError::MicrocodeIdentityMismatch { .. })
        ));

        let admitted = snapshot
            .admit_hle(admission_for(exact_identity, 7))
            .unwrap();
        assert_eq!(admitted.entry().identity(), exact_identity);
        assert_eq!(admitted.selection().microcode, exact_identity);
        assert_eq!(admitted.selection().implementation_revision, 7);
    }

    #[test]
    fn admitted_hle_and_differential_lle_forks_are_deeply_owned() {
        let entry = AudioTaskEntrySnapshot::from_post_rspboot(valid_parts()).unwrap();
        let identity = entry.identity();
        let admitted = entry.admit_hle(admission_for(identity, 1)).unwrap();
        let hle = admitted.fork_hle_lane();
        let lle = admitted.fork_lle_lane();

        assert_ne!(
            hle.snapshot().entry().rdram().storage().as_ptr(),
            lle.snapshot().rdram().storage().as_ptr()
        );
        assert_ne!(
            hle.snapshot()
                .entry()
                .rsp_memory()
                .bank(RspMemoryBank::Dmem)
                .as_ptr(),
            lle.snapshot()
                .rsp_memory()
                .bank(RspMemoryBank::Dmem)
                .as_ptr()
        );
    }

    #[test]
    fn forked_neutral_lle_lanes_have_independent_rdram_and_rsp_owners() {
        let snapshot = AudioTaskEntrySnapshot::from_post_rspboot(valid_parts()).unwrap();
        let first = snapshot.fork_lle_lane();
        let second = snapshot.fork_lle_lane();

        assert_ne!(
            first.snapshot().rdram().storage().as_ptr(),
            second.snapshot().rdram().storage().as_ptr()
        );
        assert_ne!(
            first
                .snapshot()
                .rsp_memory()
                .bank(RspMemoryBank::Dmem)
                .as_ptr(),
            second
                .snapshot()
                .rsp_memory()
                .bank(RspMemoryBank::Dmem)
                .as_ptr()
        );
        assert!(!core::ptr::eq(
            first.snapshot().machine_state(),
            second.snapshot().machine_state()
        ));

        let mut first_storage = first.into_lle_parts().into_execution_parts().rdram_storage;
        RdramViewMut::from_storage(&mut first_storage)
            .write_u8(RdramAddr::from_offset(COMMAND_ADDR), 0xff);
        assert_eq!(
            second
                .snapshot()
                .rdram()
                .view()
                .read_u8(RdramAddr::from_offset(COMMAND_ADDR)),
            0x10
        );
    }

    #[test]
    fn retained_loaded_header_does_not_reread_mutable_cpu_task_storage() {
        let mut capture = valid_parts();
        RdramViewMut::from_storage(&mut capture.rdram_storage)
            .write_u8(RdramAddr::from_offset(TASK_ADDR), 0xff);
        let snapshot = AudioTaskEntrySnapshot::from_post_rspboot(capture).unwrap();
        assert_eq!(snapshot.loaded_header().task_type, M_AUDTASK);
    }

    #[test]
    fn rejects_entry_header_bytes_not_present_in_post_rspboot_dmem() {
        let mut dmem_mismatch = valid_parts();
        let mut memory = RspMemory::from_snapshot(dmem_mismatch.rsp_memory);
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Dmem, OS_TASK_DMEM_OFFSET as u16),
                &[0xff],
            )
            .unwrap();
        dmem_mismatch.rsp_memory = memory.snapshot();
        assert!(matches!(
            AudioTaskEntrySnapshot::from_post_rspboot(dmem_mismatch),
            Err(AudioHleSnapshotError::LogicalBytesMismatch {
                image: SnapshotByteImage::EntryTaskHeader,
                first_mismatch: 0,
            })
        ));
    }

    #[test]
    fn retains_distinct_loaded_and_rspboot_entry_headers() {
        let mut input = valid_parts();
        input.entry_header.flags = 0x40;
        let mut memory = RspMemory::from_snapshot(input.rsp_memory);
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Dmem, OS_TASK_DMEM_OFFSET as u16),
                &encode_os_task_header(input.entry_header),
            )
            .unwrap();
        input.rsp_memory = memory.snapshot();

        let snapshot = AudioTaskEntrySnapshot::from_post_rspboot(input).unwrap();
        assert_eq!(snapshot.loaded_header().flags, 0);
        assert_eq!(snapshot.entry_header().flags, 0x40);
    }

    #[test]
    fn rejects_command_mismatch_but_captures_unknown_identity_for_lle() {
        let mut command = valid_parts();
        command.command_bytes[0] ^= 0xff;
        assert!(matches!(
            AudioTaskEntrySnapshot::from_post_rspboot(command),
            Err(AudioHleSnapshotError::LogicalBytesMismatch {
                image: SnapshotByteImage::CommandList,
                first_mismatch: 0,
            })
        ));

        let mut identity = valid_parts();
        identity.ucode_data_bytes[0] ^= 0xff;
        RdramViewMut::from_storage(&mut identity.rdram_storage).write_logical_bytes(
            RdramAddr::from_offset(UCODE_DATA_ADDR),
            &identity.ucode_data_bytes,
        );
        let captured = AudioTaskEntrySnapshot::from_post_rspboot(identity).unwrap();
        let lane = captured.fork_lle_lane();
        assert_eq!(lane.snapshot().identity(), captured.identity());
    }

    #[test]
    fn rejects_host_alias_storage_and_dma_ranges() {
        let mut oversized = valid_parts();
        oversized.rdram_storage.push(0);
        assert!(matches!(
            AudioTaskEntrySnapshot::from_post_rspboot(oversized),
            Err(AudioHleSnapshotError::PhysicalRdramStorageLength { .. })
        ));

        let mut alias = valid_parts();
        alias.admitted_dma_ranges = vec![0..DEFAULT_RDRAM_SIZE, 0x80_0000..0x80_1000];
        assert!(matches!(
            AudioTaskEntrySnapshot::from_post_rspboot(alias),
            Err(AudioHleSnapshotError::StaticDmaAliasNotAllowed { .. })
        ));
    }

    #[test]
    fn rejects_zero_boot_steps_and_noncanonical_fabric_pc() {
        let mut zero_steps = valid_parts();
        zero_steps.rspboot_steps = 0;
        assert_eq!(
            AudioTaskEntrySnapshot::from_post_rspboot(zero_steps).unwrap_err(),
            AudioHleSnapshotError::ZeroRspbootSteps
        );

        let mut invalid_pc = valid_parts();
        invalid_pc.entry_pc_low12 = 0x1000;
        assert_eq!(
            AudioTaskEntrySnapshot::from_post_rspboot(invalid_pc).unwrap_err(),
            AudioHleSnapshotError::EntryPcOutsideFabricRange { pc: 0x1000 }
        );
    }

    #[test]
    fn cross_checks_rspboot_accounting_and_pending_resume_pc() {
        let mut accounting = valid_parts();
        accounting.rspboot_steps = 2;
        assert_eq!(
            AudioTaskEntrySnapshot::from_post_rspboot(accounting).unwrap_err(),
            AudioHleSnapshotError::RspbootStepAccountingMismatch {
                rspboot_steps: 2,
                diagnostic_steps: 1,
            }
        );

        let mut control = valid_parts();
        control.machine_state = {
            let mut machine = RspMachine::new(&mut control.rdram_storage);
            machine.ctx.steps = 1;
            machine.ctx.resume_address = 0x1024;
            machine.snapshot_state()
        };
        control.entry_pc_low12 = 0x20;
        assert_eq!(
            AudioTaskEntrySnapshot::from_post_rspboot(control).unwrap_err(),
            AudioHleSnapshotError::EntryPcResumeMismatch {
                entry_pc_low12: 0x20,
                resume_pc_low12: 0x24,
            }
        );

        let mut matching = valid_parts();
        matching.machine_state = {
            let mut machine = RspMachine::new(&mut matching.rdram_storage);
            machine.ctx.steps = 1;
            machine.ctx.resume_address = 0x1024;
            machine.snapshot_state()
        };
        matching.entry_pc_low12 = 0x24;
        assert_eq!(
            AudioTaskEntrySnapshot::from_post_rspboot(matching)
                .unwrap()
                .entry_pc_low12(),
            0x24
        );
    }

    #[test]
    fn hle_lane_exposes_only_logical_transactional_writes() {
        let entry = AudioTaskEntrySnapshot::from_post_rspboot(valid_parts()).unwrap();
        let admitted = entry
            .clone()
            .admit_hle(admission_for(entry.identity(), 1))
            .unwrap();
        let lane = admitted.fork_hle_lane();
        let mut transaction = lane.hle_transaction();
        transaction
            .write_u8(RdramAddr::from_offset(COMMAND_ADDR), 0xff)
            .unwrap();

        assert_eq!(
            transaction
                .read_u8(RdramAddr::from_offset(COMMAND_ADDR))
                .unwrap(),
            0xff
        );
        assert_eq!(
            lane.snapshot()
                .entry()
                .rdram()
                .view()
                .read_u8(RdramAddr::from_offset(COMMAND_ADDR)),
            0x10
        );
    }

    #[test]
    fn canonicalizes_only_physical_dma_ranges() {
        let mut input = valid_parts();
        input.admitted_dma_ranges = vec![0x200..0x300, 0..0x100, 0x100..0x280];
        let snapshot = AudioTaskEntrySnapshot::from_post_rspboot(input).unwrap();
        assert_eq!(snapshot.admitted_dma_ranges().len(), 1);
        assert_eq!(snapshot.admitted_dma_ranges()[0], 0..0x300);
    }
}
