//! Persistent RSP memory plus task-header capture/counting.
//!
//! ## Provenance
//!
//! The two 4 KiB RSP memories and their CPU-visible addresses come from the
//! public SGI *Nintendo 64 RSP Programmer's Guide*, chapter 4, tables 4-6 and
//! 4-7: DMEM is physical `0x0400_0000..0x0400_0fff`, IMEM is
//! `0x0400_1000..0x0400_1fff`, and bit 12 of the SP DMA memory-address
//! register selects the bank. The same guide's "DMA" section requires
//! 64-bit alignment and describes I/DMEM as the contiguous side of each
//! transfer. Transfers that would leave the selected 4 KiB bank are rejected
//! loudly here: the allowed documentation does not specify wrap behavior, so
//! inventing one would hide an unsupported hardware edge case.
//!
//! `OSTask_t`'s field shape (`type`/`flags`/`ucode`/`ucode_data`/
//! `dram_stack`/`output_buff`/`data_ptr` etc) is the public libultra manual's
//! documented `OSTask` structure (RSP task-submission ABI, `os_task.h`);
//! `type` values `M_GFXTASK = 1`/`M_AUDTASK = 2` are the public libultra
//! manual's documented task-type constants. No GPL runtime RSP-dispatch
//! implementation was read -- `docs/DESIGN.md` section 1 explicitly flags
//! the gfx/audio task HANDOFF signature (which C function receives it) as
//! an open question for `fn64-rt64`; this module only models the ONE piece
//! of that boundary this milestone's evidence supports: recording what the
//! task header said, and (per the task's explicit scope) really invoking
//! the translated audio ucode function for `M_AUDTASK`, while acknowledging
//! (not executing) `M_GFXTASK`.
//!
//! ## Why this lives in `fn64-runtime`, not `fn64-rt64`
//!
//! `fn64-rt64` is reserved for actual RT64 (C++) interop (`docs/DESIGN.md`
//! section 1, reason 1: "the ONLY crate... permitted to contain C++"). Task
//! *counting*/*header capture* has no C++ dependency at all -- it's a pure
//! bookkeeping structure the executor's trace already wants a
//! `TaskKind`/`ucode` shape for (`trace.rs`'s `TaskSubmit`). Keeping it here
//! means `fn64-runtime` stays the single source of truth for "what tasks
//! were submitted," queryable by tests with no RT64/audio-ucode dependency
//! at all; only the actual audio-ucode FUNCTION POINTER CALL (which requires
//! linking the out-of-tree translated C) lives in the boot harness, per
//! `README.md`'s "no game content ships in this repo" rule -- this crate
//! only defines the callback SHAPE, never a real ucode body.

use crate::trace::TaskKind;
use std::fmt;

pub const RSP_MEMORY_BANK_SIZE: usize = 0x1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RspMemoryBank {
    Dmem,
    Imem,
}

/// One address in the SP DMA engine's 13-bit I/DMEM address space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RspMemAddr(u16);

impl RspMemAddr {
    pub const fn from_register(value: u32) -> Self {
        Self((value & 0x1fff) as u16)
    }

    pub const fn from_parts(bank: RspMemoryBank, offset: u16) -> Self {
        let bank_bit = match bank {
            RspMemoryBank::Dmem => 0,
            RspMemoryBank::Imem => 0x1000,
        };
        Self(bank_bit | (offset & 0x0fff))
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    pub const fn bank(self) -> RspMemoryBank {
        if self.0 & 0x1000 == 0 {
            RspMemoryBank::Dmem
        } else {
            RspMemoryBank::Imem
        }
    }

    pub const fn offset(self) -> usize {
        (self.0 & 0x0fff) as usize
    }

    /// DMA ignores the low three address bits and assumes them to be zero.
    pub const fn dma_aligned(self) -> Self {
        Self(self.0 & !7)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RspMemoryError {
    UnalignedWord { addr: RspMemAddr },
    CrossesBank { addr: RspMemAddr, len: usize },
}

impl fmt::Display for RspMemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::UnalignedWord { addr } => {
                write!(f, "unaligned RSP word access at {:#06x}", addr.get())
            }
            Self::CrossesBank { addr, len } => write!(
                f,
                "RSP memory range {:#06x}..+{len:#x} crosses its 4 KiB {:?} bank",
                addr.get(),
                addr.bank()
            ),
        }
    }
}

impl std::error::Error for RspMemoryError {}

/// An owned, pointer-free image of all architecturally visible RSP memory.
///
/// The IMEM generation is part of the snapshot because it identifies which
/// complete instruction-memory image is live. Restoring through ordinary
/// writes would manufacture new generations, so snapshots can only become
/// live through [`RspMemory::from_snapshot`] or [`RspMemory::restore`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RspMemorySnapshot {
    dmem: [u8; RSP_MEMORY_BANK_SIZE],
    imem: [u8; RSP_MEMORY_BANK_SIZE],
    imem_generation: u64,
}

impl RspMemorySnapshot {
    pub const fn imem_generation(&self) -> u64 {
        self.imem_generation
    }

    pub fn bank(&self, bank: RspMemoryBank) -> &[u8; RSP_MEMORY_BANK_SIZE] {
        match bank {
            RspMemoryBank::Dmem => &self.dmem,
            RspMemoryBank::Imem => &self.imem,
        }
    }

    fn into_memory(self) -> RspMemory {
        RspMemory {
            dmem: self.dmem,
            imem: self.imem,
            imem_generation: self.imem_generation,
        }
    }
}

/// The one persistent DMEM/IMEM image owned by a running console.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RspMemory {
    dmem: [u8; RSP_MEMORY_BANK_SIZE],
    imem: [u8; RSP_MEMORY_BANK_SIZE],
    imem_generation: u64,
}

impl Default for RspMemory {
    fn default() -> Self {
        Self {
            dmem: [0; RSP_MEMORY_BANK_SIZE],
            imem: [0; RSP_MEMORY_BANK_SIZE],
            imem_generation: 0,
        }
    }
}

impl RspMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Captures both complete memory banks and the exact live IMEM generation.
    pub fn snapshot(&self) -> RspMemorySnapshot {
        RspMemorySnapshot {
            dmem: self.dmem,
            imem: self.imem,
            imem_generation: self.imem_generation,
        }
    }

    /// Constructs memory from a snapshot without replaying writes.
    pub fn from_snapshot(snapshot: RspMemorySnapshot) -> Self {
        snapshot.into_memory()
    }

    /// Atomically replaces both banks and their generation under this
    /// exclusive borrow, without exposing an intermediate memory image.
    pub fn restore(&mut self, snapshot: RspMemorySnapshot) {
        *self = Self::from_snapshot(snapshot);
    }

    pub const fn imem_generation(&self) -> u64 {
        self.imem_generation
    }

    pub fn bank(&self, bank: RspMemoryBank) -> &[u8; RSP_MEMORY_BANK_SIZE] {
        match bank {
            RspMemoryBank::Dmem => &self.dmem,
            RspMemoryBank::Imem => &self.imem,
        }
    }

    fn checked_range(
        addr: RspMemAddr,
        len: usize,
    ) -> Result<std::ops::Range<usize>, RspMemoryError> {
        let start = addr.offset();
        let Some(end) = start.checked_add(len) else {
            return Err(RspMemoryError::CrossesBank { addr, len });
        };
        if end > RSP_MEMORY_BANK_SIZE {
            return Err(RspMemoryError::CrossesBank { addr, len });
        }
        Ok(start..end)
    }

    pub fn read_bytes(&self, addr: RspMemAddr, len: usize) -> Result<Vec<u8>, RspMemoryError> {
        let range = Self::checked_range(addr, len)?;
        Ok(self.bank(addr.bank())[range].to_vec())
    }

    pub fn write_bytes(&mut self, addr: RspMemAddr, bytes: &[u8]) -> Result<(), RspMemoryError> {
        let range = Self::checked_range(addr, bytes.len())?;
        match addr.bank() {
            RspMemoryBank::Dmem => self.dmem[range].copy_from_slice(bytes),
            RspMemoryBank::Imem => {
                self.imem[range].copy_from_slice(bytes);
                if !bytes.is_empty() {
                    self.imem_generation = self
                        .imem_generation
                        .checked_add(1)
                        .expect("RSP IMEM generation overflow");
                }
            }
        }
        Ok(())
    }

    pub fn read_word(&self, addr: RspMemAddr) -> Result<u32, RspMemoryError> {
        if addr.offset() & 3 != 0 {
            return Err(RspMemoryError::UnalignedWord { addr });
        }
        let bytes = self.read_bytes(addr, 4)?;
        Ok(u32::from_be_bytes(
            bytes.try_into().expect("four RSP bytes"),
        ))
    }

    pub fn write_word(&mut self, addr: RspMemAddr, value: u32) -> Result<(), RspMemoryError> {
        if addr.offset() & 3 != 0 {
            return Err(RspMemoryError::UnalignedWord { addr });
        }
        self.write_bytes(addr, &value.to_be_bytes())
    }
}

/// Public libultra manual's complete 64-byte `OSTask_t` field shape (RSP
/// task-submission ABI). Keeping all sixteen words is required because task
/// admission copies this exact structure to DMEM offset `0xfc0` for rspboot.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct OsTaskHeader {
    pub task_type: u32,
    pub flags: u32,
    pub ucode_boot: u32,
    pub ucode_boot_size: u32,
    pub ucode: u32,
    pub ucode_size: u32,
    pub ucode_data: u32,
    pub ucode_data_size: u32,
    pub dram_stack: u32,
    pub dram_stack_size: u32,
    pub output_buff: u32,
    pub output_buff_size: u32,
    pub data_ptr: u32,
    pub data_size: u32,
    pub yield_data_ptr: u32,
    pub yield_data_size: u32,
}

/// Public libultra manual's documented `OSTask.t.type` constants.
pub const M_GFXTASK: u32 = 1;
pub const M_AUDTASK: u32 = 2;
/// Public `OSYieldResult` value installed in `OSTask.t.flags` after a yield.
pub const OS_TASK_YIELDED: u32 = 1;

impl OsTaskHeader {
    pub fn kind(&self) -> Option<TaskKind> {
        match self.task_type {
            M_GFXTASK => Some(TaskKind::Graphics),
            M_AUDTASK => Some(TaskKind::Audio),
            _ => None,
        }
    }
}

/// Host-side counters/log for every RSP task image admitted by `osSpTaskLoad`.
/// This stays separate from the shared `TraceLog`, whose lighter
/// `TaskSubmit{task_kind, ucode}` event records the later `osSpTaskStartGo`
/// kickoff, so a harness can inspect complete admitted headers without
/// treating a replaced load as execution.
#[derive(Default)]
pub struct TaskLog {
    submissions: Vec<OsTaskHeader>,
    gfx_count: u64,
    audio_count: u64,
}

impl TaskLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, header: OsTaskHeader) {
        match header.kind() {
            Some(TaskKind::Graphics) => self.gfx_count += 1,
            Some(TaskKind::Audio) => self.audio_count += 1,
            None => {}
        }
        self.submissions.push(header);
    }

    pub fn gfx_count(&self) -> u64 {
        self.gfx_count
    }

    pub fn audio_count(&self) -> u64 {
        self.audio_count
    }

    pub fn submissions(&self) -> &[OsTaskHeader] {
        &self.submissions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_banks_use_architectural_word_order_and_track_imem_replacement() {
        let mut memory = RspMemory::new();
        let dmem = RspMemAddr::from_parts(RspMemoryBank::Dmem, 0x20);
        let imem = RspMemAddr::from_parts(RspMemoryBank::Imem, 0x40);

        memory.write_word(dmem, 0x1234_5678).unwrap();
        assert_eq!(
            memory.read_bytes(dmem, 4).unwrap(),
            [0x12, 0x34, 0x56, 0x78]
        );
        assert_eq!(memory.read_word(dmem).unwrap(), 0x1234_5678);
        assert_eq!(memory.imem_generation(), 0);

        memory.write_bytes(imem, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        assert_eq!(memory.read_word(imem).unwrap(), 0xDEAD_BEEF);
        assert_eq!(memory.imem_generation(), 1);
        memory.write_word(imem, 0xCAFE_BABE).unwrap();
        assert_eq!(memory.imem_generation(), 2);
    }

    #[test]
    fn memory_ranges_cannot_silently_cross_between_dmem_and_imem() {
        let mut memory = RspMemory::new();
        let addr = RspMemAddr::from_parts(RspMemoryBank::Dmem, 0x0ffc);
        assert_eq!(
            memory.write_bytes(addr, &[0; 8]),
            Err(RspMemoryError::CrossesBank { addr, len: 8 })
        );
    }

    #[test]
    fn snapshot_round_trips_complete_banks_and_exact_generation() {
        let mut memory = RspMemory::new();
        let dmem = std::array::from_fn(|index| (index as u8).wrapping_mul(3));
        let mut imem = std::array::from_fn(|index| (index as u8).wrapping_mul(5));

        memory
            .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Dmem, 0), &dmem)
            .unwrap();
        memory
            .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &imem)
            .unwrap();
        imem[0x321] ^= 0xff;
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Imem, 0x321),
                &imem[0x321..0x322],
            )
            .unwrap();
        assert_eq!(memory.imem_generation(), 2);

        let snapshot = memory.snapshot();
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Dmem, 0),
                &[0xaa; RSP_MEMORY_BANK_SIZE],
            )
            .unwrap();
        memory
            .write_bytes(
                RspMemAddr::from_parts(RspMemoryBank::Imem, 0),
                &[0x55; RSP_MEMORY_BANK_SIZE],
            )
            .unwrap();

        memory.restore(snapshot);

        assert_eq!(memory.bank(RspMemoryBank::Dmem), &dmem);
        assert_eq!(memory.bank(RspMemoryBank::Imem), &imem);
        assert_eq!(memory.imem_generation(), 2);
    }

    #[test]
    fn snapshots_are_independent_owned_images() {
        let mut memory = RspMemory::new();
        let dmem_addr = RspMemAddr::from_parts(RspMemoryBank::Dmem, 0x80);
        let imem_addr = RspMemAddr::from_parts(RspMemoryBank::Imem, 0x180);

        memory.write_bytes(dmem_addr, &[0x11]).unwrap();
        memory.write_bytes(imem_addr, &[0x22]).unwrap();
        let first = memory.snapshot();

        memory.write_bytes(dmem_addr, &[0x33]).unwrap();
        memory.write_bytes(imem_addr, &[0x44]).unwrap();
        let second = memory.snapshot();

        memory.write_bytes(dmem_addr, &[0x55]).unwrap();
        memory.write_bytes(imem_addr, &[0x66]).unwrap();

        assert_eq!(first.bank(RspMemoryBank::Dmem)[0x80], 0x11);
        assert_eq!(first.bank(RspMemoryBank::Imem)[0x180], 0x22);
        assert_eq!(first.imem_generation(), 1);
        assert_eq!(second.bank(RspMemoryBank::Dmem)[0x80], 0x33);
        assert_eq!(second.bank(RspMemoryBank::Imem)[0x180], 0x44);
        assert_eq!(second.imem_generation(), 2);

        let first_memory = RspMemory::from_snapshot(first);
        let second_memory = RspMemory::from_snapshot(second);
        assert_eq!(first_memory.bank(RspMemoryBank::Dmem)[0x80], 0x11);
        assert_eq!(second_memory.bank(RspMemoryBank::Dmem)[0x80], 0x33);
        assert_eq!(first_memory.imem_generation(), 1);
        assert_eq!(second_memory.imem_generation(), 2);
    }

    #[test]
    fn counts_gfx_and_audio_separately() {
        let mut log = TaskLog::new();
        log.record(OsTaskHeader {
            task_type: M_GFXTASK,
            ..Default::default()
        });
        log.record(OsTaskHeader {
            task_type: M_AUDTASK,
            ..Default::default()
        });
        log.record(OsTaskHeader {
            task_type: M_AUDTASK,
            ..Default::default()
        });
        assert_eq!(log.gfx_count(), 1);
        assert_eq!(log.audio_count(), 2);
        assert_eq!(log.submissions().len(), 3);
    }

    #[test]
    fn unknown_task_type_is_recorded_but_not_counted() {
        let mut log = TaskLog::new();
        log.record(OsTaskHeader {
            task_type: 99,
            ..Default::default()
        });
        assert_eq!(log.gfx_count(), 0);
        assert_eq!(log.audio_count(), 0);
        assert_eq!(log.submissions().len(), 1);
    }
}
