//! Bounded static operand recovery for direct PI/EPI DMA calls.
//!
//! The public libultra contract is
//! `osPiStartDma(mb, priority, direction, devAddr, vAddr, nbytes, mq)`.
//! Under the N64 o32 ABI, `direction`/`devAddr` are in `$a2`/`$a3`, and
//! `vAddr`/`nbytes` are in the caller's stack argument area at
//! `$sp + 0x10`/`$sp + 0x14`.
//! This module recovers only constants established in the straight-line run
//! immediately before a direct `jal`, including the call's delay slot.
//!
//! Recovered operands are exact call-boundary semantics.  The resulting DMA
//! geometry remains a candidate: instruction bytes do not prove reachability,
//! successful asynchronous completion, or how an `OSPiHandle` maps a raw
//! device address to a normalized ROM-file offset.
//!
//! Provenance: the public libultra `osPiStartDma` manual entry supplies the
//! argument order, direction meanings, and asynchronous completion contract.
//! Instruction behavior follows the public MIPS III ISA.  The o32 argument
//! locations are the ABI used by N64 libultra callers.
//!
//! The public libultra `osEPiStartDma` manual entry supplies its distinct
//! `(OSPiHandle *, OSIoMesg *, direction)` argument order and identifies
//! `dramAddr`, `devAddr`, and `size` as message fields. Their byte offsets
//! `+0x08`, `+0x0c`, and `+0x10` are the same layout byte-verified by
//! `fn64-abi::pi`; keeping the APIs as different slice types prevents one
//! calling convention from being silently applied to the other.

use crate::loaders::{RdramAddress, VirtualAddress};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

const MAX_BACKWARD_WORDS: usize = 64;
const KSEG0_START: u32 = 0x8000_0000;
const KSEG1_START: u32 = 0xa000_0000;
const KSEG_SIZE: u32 = 0x2000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PiDeviceAddress(u32);

impl PiDeviceAddress {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiDmaDirection {
    ToRdram,
    FromRdram,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SliceBlocker {
    DefinitionNotRecovered {
        register: u8,
        slice_start: VirtualAddress,
        budget_exhausted: bool,
    },
    LoadedFromMemory {
        pc: VirtualAddress,
        register: u8,
    },
    UnsupportedRegisterWrite {
        pc: VirtualAddress,
        register: u8,
    },
    SignedOverflowCannotBeExcluded {
        pc: VirtualAddress,
        register: u8,
    },
    AddressArithmeticOverflow {
        pc: VirtualAddress,
    },
    StackPointerUnresolved,
    StackArgumentNotWritten {
        offset: u8,
    },
    MessageFieldNotWritten {
        offset: u8,
    },
    PotentialStackAlias {
        pc: VirtualAddress,
    },
    ControlTransferInDelaySlot {
        pc: VirtualAddress,
    },
    InvalidDirection {
        raw: u32,
    },
    ZeroByteCount,
    DramPointerOutsideKseg {
        raw: u32,
    },
    RdramRangeOverflow,
    RdramRangeOutOfBounds {
        end_exclusive: u64,
        rdram_len: u32,
    },
    DeviceRangeOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandSource {
    RegisterDefinition { register: u8, pc: VirtualAddress },
    StackStore { offset: i32, pc: VirtualAddress },
    MemoryStore { address: u32, pc: VirtualAddress },
    HardwareZeroRegister,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StaticOperand<T> {
    Proven { value: T, source: OperandSource },
    Open { blockers: Vec<SliceBlocker> },
}

impl<T> StaticOperand<T> {
    pub fn proven(&self) -> Option<&T> {
        match self {
            Self::Proven { value, .. } => Some(value),
            Self::Open { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiDmaCallSlice {
    pub call_pc: VirtualAddress,
    pub callee: VirtualAddress,
    pub direction: StaticOperand<PiDmaDirection>,
    pub device_address: StaticOperand<PiDeviceAddress>,
    pub dram_pointer: StaticOperand<VirtualAddress>,
    pub rdram_address: StaticOperand<RdramAddress>,
    pub byte_count: StaticOperand<NonZeroU32>,
}

/// Fully recovered call geometry.  This is deliberately named `Candidate`:
/// static call operands cannot establish completion or device-to-ROM mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticPiDmaCandidate {
    pub call_pc: VirtualAddress,
    pub direction: PiDmaDirection,
    pub device_address: PiDeviceAddress,
    pub rdram_address: RdramAddress,
    pub byte_count: NonZeroU32,
}

impl PiDmaCallSlice {
    pub fn candidate(&self) -> Option<StaticPiDmaCandidate> {
        Some(StaticPiDmaCandidate {
            call_pc: self.call_pc,
            direction: *self.direction.proven()?,
            device_address: *self.device_address.proven()?,
            rdram_address: *self.rdram_address.proven()?,
            byte_count: *self.byte_count.proven()?,
        })
    }
}

/// Recovered operands for the distinct
/// `osEPiStartDma(OSPiHandle *, OSIoMesg *, direction)` ABI. Geometry is read
/// from the message object (`dramAddr +0x08`, `devAddr +0x0c`, `size +0x10`),
/// not from nonexistent stack arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpiDmaCallSlice {
    pub call_pc: VirtualAddress,
    pub callee: VirtualAddress,
    pub handle_pointer: StaticOperand<VirtualAddress>,
    pub message_pointer: StaticOperand<VirtualAddress>,
    pub direction: StaticOperand<PiDmaDirection>,
    pub device_address: StaticOperand<PiDeviceAddress>,
    pub dram_pointer: StaticOperand<VirtualAddress>,
    pub rdram_address: StaticOperand<RdramAddress>,
    pub byte_count: StaticOperand<NonZeroU32>,
}

impl EpiDmaCallSlice {
    pub fn candidate(&self) -> Option<StaticPiDmaCandidate> {
        Some(StaticPiDmaCandidate {
            call_pc: self.call_pc,
            direction: *self.direction.proven()?,
            device_address: *self.device_address.proven()?,
            rdram_address: *self.rdram_address.proven()?,
            byte_count: *self.byte_count.proven()?,
        })
    }
}

/// Recovered operands for a game-specific load-request wrapper (e.g. a DMA
/// manager's request function) whose destination pointer, device address,
/// and byte count travel in caller-declared argument registers. Which
/// registers — and which address space the device operand names — is part
/// of the caller's cited claim, not something this slicer infers. Requests
/// are loads by contract, so direction is `ToRdram` by construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadRequestCallSlice {
    pub call_pc: VirtualAddress,
    pub callee: VirtualAddress,
    pub dram_pointer: StaticOperand<VirtualAddress>,
    pub rdram_address: StaticOperand<RdramAddress>,
    pub device_address: StaticOperand<PiDeviceAddress>,
    pub byte_count: StaticOperand<NonZeroU32>,
}

impl LoadRequestCallSlice {
    pub fn candidate(&self) -> Option<StaticPiDmaCandidate> {
        Some(StaticPiDmaCandidate {
            call_pc: self.call_pc,
            direction: PiDmaDirection::ToRdram,
            device_address: *self.device_address.proven()?,
            rdram_address: *self.rdram_address.proven()?,
            byte_count: *self.byte_count.proven()?,
        })
    }
}

/// A statically recovered code-pointer argument at a direct call to a
/// cited callee (e.g. `osCreateThread`'s entry argument): the OS will
/// transfer control to the recovered address, making a constant operand a
/// callable-entry observation of the same strength as the other static
/// call-boundary slices — reachability of the call site itself stays
/// unproven, as ever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PointerArgCallSlice {
    pub call_pc: VirtualAddress,
    pub callee: VirtualAddress,
    pub pointer: StaticOperand<VirtualAddress>,
}

/// Find direct calls to `callee` and recover the pointer argument in the
/// declared register. Which register — and what the pointer means — is the
/// caller's cited claim; this slicer only recovers straight-line constants.
pub fn slice_pointer_arg_calls(
    image_words: &[u32],
    image_start: VirtualAddress,
    callee: VirtualAddress,
    rdram_len: u32,
    pointer_register: u8,
) -> Result<Vec<PointerArgCallSlice>, PiDmaSliceError> {
    validate_input(image_words, image_start, rdram_len)?;
    let mut out = Vec::new();
    for call_index in 0..image_words.len().saturating_sub(1) {
        let call_pc = pc_at(image_start, call_index);
        if direct_jal_target(image_words[call_index], call_pc) != Some(callee.get()) {
            continue;
        }
        let state = slice_state_at_call(image_words, image_start, call_index, &[pointer_register]);
        out.push(PointerArgCallSlice {
            call_pc,
            callee,
            pointer: map_operand(register_operand(&state, pointer_register), |raw| {
                Ok(VirtualAddress::new(raw))
            }),
        });
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PiDmaSliceError {
    EmptyImage,
    ImageAddressUnaligned { start: VirtualAddress },
    ImageAddressOverflow,
    RdramLengthOutsidePhysicalDomain { rdram_len: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    Constant { value: u32, source: OperandSource },
    StackPointer { offset: i32 },
    Open(BTreeSet<SliceBlocker>),
}

impl Value {
    fn open(blocker: SliceBlocker) -> Self {
        Self::Open(BTreeSet::from([blocker]))
    }

    fn as_constant(&self) -> Result<(u32, OperandSource), Vec<SliceBlocker>> {
        match self {
            Self::Constant { value, source } => Ok((*value, *source)),
            Self::StackPointer { .. } => Err(vec![SliceBlocker::StackPointerUnresolved]),
            Self::Open(blockers) => Err(blockers.iter().cloned().collect()),
        }
    }
}

#[derive(Clone, Debug)]
struct SliceState {
    registers: [Value; 32],
    stack_words: BTreeMap<i32, Value>,
    memory_words: BTreeMap<u32, Value>,
    last_stack_clobber: Option<SliceBlocker>,
}

impl SliceState {
    fn new(slice_start: VirtualAddress, budget_exhausted: bool) -> Self {
        let mut registers = std::array::from_fn(|register| {
            Value::open(SliceBlocker::DefinitionNotRecovered {
                register: register as u8,
                slice_start,
                budget_exhausted,
            })
        });
        registers[0] = Value::Constant {
            value: 0,
            source: OperandSource::HardwareZeroRegister,
        };
        registers[29] = Value::StackPointer { offset: 0 };
        Self {
            registers,
            stack_words: BTreeMap::new(),
            memory_words: BTreeMap::new(),
            last_stack_clobber: None,
        }
    }

    fn set_register(&mut self, register: u8, value: Value) {
        if register != 0 {
            self.registers[register as usize] = value;
        }
    }

    fn clobber_memory(&mut self, blocker: SliceBlocker) {
        self.stack_words.clear();
        self.memory_words.clear();
        self.last_stack_clobber = Some(blocker);
    }
}

/// Find direct calls to `os_pi_start_dma` and recover their constant operands.
///
/// The scan is bounded to 64 words before each call and never propagates a
/// value across an earlier control transfer. A missing definition remains an
/// explicit blocker. The call delay slot is evaluated because its writes are
/// visible to the callee.
pub fn slice_os_pi_start_dma_calls(
    image_words: &[u32],
    image_start: VirtualAddress,
    os_pi_start_dma: VirtualAddress,
    rdram_len: u32,
) -> Result<Vec<PiDmaCallSlice>, PiDmaSliceError> {
    validate_input(image_words, image_start, rdram_len)?;
    let mut out = Vec::new();

    for call_index in 0..image_words.len().saturating_sub(1) {
        let call_pc = pc_at(image_start, call_index);
        if direct_jal_target(image_words[call_index], call_pc) != Some(os_pi_start_dma.get()) {
            continue;
        }
        out.push(slice_call(
            image_words,
            image_start,
            call_index,
            os_pi_start_dma,
            rdram_len,
        ));
    }
    Ok(out)
}

/// Find direct calls to `osEPiStartDma` and recover geometry from stores to
/// the caller-provided `OSIoMesg`. Both stack-local and statically addressed
/// message objects are supported; a load or ambiguous alias remains open.
pub fn slice_os_epi_start_dma_calls(
    image_words: &[u32],
    image_start: VirtualAddress,
    os_epi_start_dma: VirtualAddress,
    rdram_len: u32,
) -> Result<Vec<EpiDmaCallSlice>, PiDmaSliceError> {
    validate_input(image_words, image_start, rdram_len)?;
    let mut out = Vec::new();
    for call_index in 0..image_words.len().saturating_sub(1) {
        let call_pc = pc_at(image_start, call_index);
        if direct_jal_target(image_words[call_index], call_pc) != Some(os_epi_start_dma.get()) {
            continue;
        }
        let state = slice_state_at_call(image_words, image_start, call_index, &[5, 6]);
        let message = &state.registers[5];
        let dram_raw = message_operand(&state, message, 0x08);
        let device_raw = message_operand(&state, message, 0x0c);
        let size_raw = message_operand(&state, message, 0x10);
        let mut device_address = map_operand(device_raw, |raw| Ok(PiDeviceAddress::new(raw)));
        let byte_count = map_byte_count(size_raw.clone());
        apply_device_range_check(&mut device_address, &byte_count);
        out.push(EpiDmaCallSlice {
            call_pc,
            callee: os_epi_start_dma,
            handle_pointer: pointer_operand(&state.registers[4]),
            message_pointer: pointer_operand(message),
            direction: map_direction(register_operand(&state, 6)),
            dram_pointer: map_operand(dram_raw.clone(), |raw| Ok(VirtualAddress::new(raw))),
            rdram_address: resolve_rdram_operand(dram_raw, size_raw, rdram_len),
            device_address,
            byte_count,
        });
    }
    Ok(out)
}

/// Find direct calls to a cited load-request wrapper and recover the
/// destination/device/size argument registers. The device operand is NOT
/// range-checked against the physical cartridge domain here: request
/// wrappers may name a VROM address a DMA manager later translates, so the
/// caller owns that judgement along with the declared device space.
pub fn slice_load_request_calls(
    image_words: &[u32],
    image_start: VirtualAddress,
    callee: VirtualAddress,
    rdram_len: u32,
    dram_register: u8,
    device_register: u8,
    size_register: u8,
) -> Result<Vec<LoadRequestCallSlice>, PiDmaSliceError> {
    validate_input(image_words, image_start, rdram_len)?;
    let mut out = Vec::new();
    for call_index in 0..image_words.len().saturating_sub(1) {
        let call_pc = pc_at(image_start, call_index);
        if direct_jal_target(image_words[call_index], call_pc) != Some(callee.get()) {
            continue;
        }
        let state = slice_state_at_call(
            image_words,
            image_start,
            call_index,
            &[dram_register, device_register, size_register],
        );
        let dram_raw = register_operand(&state, dram_register);
        let size_raw = register_operand(&state, size_register);
        out.push(LoadRequestCallSlice {
            call_pc,
            callee,
            dram_pointer: map_operand(dram_raw.clone(), |raw| Ok(VirtualAddress::new(raw))),
            rdram_address: resolve_rdram_operand(dram_raw, size_raw.clone(), rdram_len),
            device_address: map_operand(register_operand(&state, device_register), |raw| {
                Ok(PiDeviceAddress::new(raw))
            }),
            byte_count: map_byte_count(size_raw),
        });
    }
    Ok(out)
}

fn validate_input(
    words: &[u32],
    start: VirtualAddress,
    rdram_len: u32,
) -> Result<(), PiDmaSliceError> {
    if words.is_empty() {
        return Err(PiDmaSliceError::EmptyImage);
    }
    if start.get() & 3 != 0 {
        return Err(PiDmaSliceError::ImageAddressUnaligned { start });
    }
    let byte_len = (words.len() as u64) * 4;
    if start.get() as u64 + byte_len > u32::MAX as u64 + 1 {
        return Err(PiDmaSliceError::ImageAddressOverflow);
    }
    if rdram_len > KSEG_SIZE {
        return Err(PiDmaSliceError::RdramLengthOutsidePhysicalDomain { rdram_len });
    }
    Ok(())
}

fn slice_call(
    words: &[u32],
    image_start: VirtualAddress,
    call_index: usize,
    callee: VirtualAddress,
    rdram_len: u32,
) -> PiDmaCallSlice {
    let state = slice_state_at_call(words, image_start, call_index, &[6, 7]);

    let direction_raw = register_operand(&state, 6);
    let device_raw = register_operand(&state, 7);
    let dram_raw = stack_operand(&state, 0x10);
    let size_raw = stack_operand(&state, 0x14);

    let direction = map_direction(direction_raw);
    let device_address = map_operand(device_raw, |raw| Ok(PiDeviceAddress::new(raw)));
    let dram_pointer = map_operand(dram_raw.clone(), |raw| Ok(VirtualAddress::new(raw)));
    let byte_count = map_byte_count(size_raw.clone());
    let rdram_address = resolve_rdram_operand(dram_raw, size_raw, rdram_len);

    let call_pc = pc_at(image_start, call_index);
    let mut slice = PiDmaCallSlice {
        call_pc,
        callee,
        direction,
        device_address,
        dram_pointer,
        rdram_address,
        byte_count,
    };
    apply_device_range_check(&mut slice.device_address, &slice.byte_count);
    slice
}

fn slice_state_at_call(
    words: &[u32],
    image_start: VirtualAddress,
    call_index: usize,
    delay_affected_registers: &[u8],
) -> SliceState {
    let budget_start = call_index.saturating_sub(MAX_BACKWARD_WORDS);
    let mut slice_start = budget_start;
    for (index, &word) in words.iter().enumerate().take(call_index).skip(budget_start) {
        if is_control_transfer(word) && index + 1 < call_index {
            slice_start = index + 2;
        }
    }
    let budget_exhausted = slice_start == budget_start && budget_start != 0;
    let slice_start_pc = pc_at(image_start, slice_start);
    let mut state = SliceState::new(slice_start_pc, budget_exhausted);

    for (index, &word) in words.iter().enumerate().take(call_index).skip(slice_start) {
        execute(&mut state, pc_at(image_start, index), word);
    }

    let delay_pc = pc_at(image_start, call_index + 1);
    let delay_is_control = is_control_transfer(words[call_index + 1]);
    if delay_is_control {
        let blocker = SliceBlocker::ControlTransferInDelaySlot { pc: delay_pc };
        for &register in delay_affected_registers {
            state.set_register(register, Value::open(blocker.clone()));
        }
        state.clobber_memory(blocker);
    } else {
        execute(&mut state, delay_pc, words[call_index + 1]);
    }

    state
}

fn map_direction(operand: StaticOperand<u32>) -> StaticOperand<PiDmaDirection> {
    map_operand(operand, |raw| match raw {
        0 => Ok(PiDmaDirection::ToRdram),
        1 => Ok(PiDmaDirection::FromRdram),
        _ => Err(SliceBlocker::InvalidDirection { raw }),
    })
}

fn map_byte_count(operand: StaticOperand<u32>) -> StaticOperand<NonZeroU32> {
    map_operand(operand, |raw| {
        NonZeroU32::new(raw).ok_or(SliceBlocker::ZeroByteCount)
    })
}

fn register_operand(state: &SliceState, register: u8) -> StaticOperand<u32> {
    match state.registers[register as usize].as_constant() {
        Ok((value, source)) => StaticOperand::Proven { value, source },
        Err(blockers) => StaticOperand::Open { blockers },
    }
}

fn stack_operand(state: &SliceState, offset: u8) -> StaticOperand<u32> {
    let Value::StackPointer { offset: sp_offset } = state.registers[29] else {
        return StaticOperand::Open {
            blockers: vec![SliceBlocker::StackPointerUnresolved],
        };
    };
    let Some(slot) = sp_offset.checked_add(offset as i32) else {
        return StaticOperand::Open {
            blockers: vec![SliceBlocker::AddressArithmeticOverflow {
                pc: VirtualAddress::new(0),
            }],
        };
    };
    let Some(value) = state.stack_words.get(&slot) else {
        let blocker = state
            .last_stack_clobber
            .clone()
            .unwrap_or(SliceBlocker::StackArgumentNotWritten { offset });
        return StaticOperand::Open {
            blockers: vec![blocker],
        };
    };
    match value.as_constant() {
        Ok((value, source)) => StaticOperand::Proven { value, source },
        Err(blockers) => StaticOperand::Open { blockers },
    }
}

fn pointer_operand(value: &Value) -> StaticOperand<VirtualAddress> {
    match value.as_constant() {
        Ok((value, source)) => StaticOperand::Proven {
            value: VirtualAddress::new(value),
            source,
        },
        Err(blockers) => StaticOperand::Open { blockers },
    }
}

fn message_operand(state: &SliceState, message: &Value, field: u8) -> StaticOperand<u32> {
    let value = match message {
        Value::StackPointer { offset } => {
            let Some(address) = offset.checked_add(i32::from(field)) else {
                return StaticOperand::Open {
                    blockers: vec![SliceBlocker::AddressArithmeticOverflow {
                        pc: VirtualAddress::new(0),
                    }],
                };
            };
            state.stack_words.get(&address)
        }
        Value::Constant { value, .. } => {
            let Some(address) = value.checked_add(u32::from(field)) else {
                return StaticOperand::Open {
                    blockers: vec![SliceBlocker::AddressArithmeticOverflow {
                        pc: VirtualAddress::new(0),
                    }],
                };
            };
            state.memory_words.get(&address)
        }
        Value::Open(blockers) => {
            return StaticOperand::Open {
                blockers: blockers.iter().cloned().collect(),
            };
        }
    };
    let Some(value) = value else {
        let blocker = state
            .last_stack_clobber
            .clone()
            .unwrap_or(SliceBlocker::MessageFieldNotWritten { offset: field });
        return StaticOperand::Open {
            blockers: vec![blocker],
        };
    };
    match value.as_constant() {
        Ok((value, source)) => StaticOperand::Proven { value, source },
        Err(blockers) => StaticOperand::Open { blockers },
    }
}

fn map_operand<T, U>(
    operand: StaticOperand<T>,
    map: impl FnOnce(T) -> Result<U, SliceBlocker>,
) -> StaticOperand<U> {
    match operand {
        StaticOperand::Proven { value, source } => match map(value) {
            Ok(value) => StaticOperand::Proven { value, source },
            Err(blocker) => StaticOperand::Open {
                blockers: vec![blocker],
            },
        },
        StaticOperand::Open { blockers } => StaticOperand::Open { blockers },
    }
}

fn resolve_rdram_operand(
    dram: StaticOperand<u32>,
    size: StaticOperand<u32>,
    rdram_len: u32,
) -> StaticOperand<RdramAddress> {
    let (raw, source) = match dram {
        StaticOperand::Proven { value, source } => (value, source),
        StaticOperand::Open { blockers } => return StaticOperand::Open { blockers },
    };
    let physical = if raw.wrapping_sub(KSEG0_START) < KSEG_SIZE {
        raw - KSEG0_START
    } else if raw.wrapping_sub(KSEG1_START) < KSEG_SIZE {
        raw - KSEG1_START
    } else {
        return StaticOperand::Open {
            blockers: vec![SliceBlocker::DramPointerOutsideKseg { raw }],
        };
    };
    let size = match size {
        StaticOperand::Proven { value, .. } => value,
        StaticOperand::Open { blockers } => return StaticOperand::Open { blockers },
    };
    let Some(end_exclusive) = (physical as u64).checked_add(size as u64) else {
        return StaticOperand::Open {
            blockers: vec![SliceBlocker::RdramRangeOverflow],
        };
    };
    if end_exclusive > u32::MAX as u64 + 1 {
        return StaticOperand::Open {
            blockers: vec![SliceBlocker::RdramRangeOverflow],
        };
    }
    if end_exclusive > rdram_len as u64 {
        return StaticOperand::Open {
            blockers: vec![SliceBlocker::RdramRangeOutOfBounds {
                end_exclusive,
                rdram_len,
            }],
        };
    }
    StaticOperand::Proven {
        value: RdramAddress::new(physical),
        source,
    }
}

fn apply_device_range_check(
    device_address: &mut StaticOperand<PiDeviceAddress>,
    byte_count: &StaticOperand<NonZeroU32>,
) {
    let (Some(device), Some(size)) = (
        device_address.proven().copied(),
        byte_count.proven().copied(),
    ) else {
        return;
    };
    if device.get() as u64 + size.get() as u64 > u32::MAX as u64 + 1 {
        *device_address = StaticOperand::Open {
            blockers: vec![SliceBlocker::DeviceRangeOverflow],
        };
    }
}

fn execute(state: &mut SliceState, pc: VirtualAddress, word: u32) {
    let op = word >> 26;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let rd = ((word >> 11) & 0x1f) as u8;
    let immediate = (word as i16) as i32;
    match op {
        0 => match word & 0x3f {
            0x21 | 0x25 | 0x2d if rt == 0 => {
                state.set_register(rd, state.registers[rs as usize].clone());
            }
            0x21 | 0x25 | 0x2d if rs == 0 => {
                state.set_register(rd, state.registers[rt as usize].clone());
            }
            // addu/subu/or over two proven constants folds exactly: these
            // never trap and wrap mod 2^32 like the hardware (IDO computes
            // e.g. a segment's byte count as `subu end, start` of two
            // link-time constants). Anything else stays unsupported.
            0x21 | 0x23 | 0x25 if rd != 0 => {
                let value = match (
                    &state.registers[rs as usize],
                    &state.registers[rt as usize],
                ) {
                    (
                        Value::Constant { value: lhs, .. },
                        Value::Constant { value: rhs, .. },
                    ) => Value::Constant {
                        value: match word & 0x3f {
                            0x21 => lhs.wrapping_add(*rhs),
                            0x23 => lhs.wrapping_sub(*rhs),
                            _ => lhs | rhs,
                        },
                        source: OperandSource::RegisterDefinition { register: rd, pc },
                    },
                    _ => Value::open(SliceBlocker::UnsupportedRegisterWrite { pc, register: rd }),
                };
                state.set_register(rd, value);
            }
            _ if rd != 0 => state.set_register(
                rd,
                Value::open(SliceBlocker::UnsupportedRegisterWrite { pc, register: rd }),
            ),
            _ => {}
        },
        0x0f => state.set_register(
            rt,
            Value::Constant {
                value: (word & 0xffff) << 16,
                source: OperandSource::RegisterDefinition { register: rt, pc },
            },
        ),
        0x09 | 0x19 => {
            let value = add_immediate(&state.registers[rs as usize], immediate, pc, rt, false);
            state.set_register(rt, value);
        }
        0x08 | 0x18 => {
            let value = add_immediate(&state.registers[rs as usize], immediate, pc, rt, true);
            state.set_register(rt, value);
        }
        0x0d => {
            let value = match &state.registers[rs as usize] {
                Value::Constant { value, .. } => Value::Constant {
                    value: *value | (word & 0xffff),
                    source: OperandSource::RegisterDefinition { register: rt, pc },
                },
                other => other.clone(),
            };
            state.set_register(rt, value);
        }
        0x23 | 0x27 => state.set_register(
            rt,
            Value::open(SliceBlocker::LoadedFromMemory { pc, register: rt }),
        ),
        0x2b => store_word(state, pc, rs, rt, immediate),
        0x28..=0x2f | 0x38..=0x3f => {
            state.clobber_memory(SliceBlocker::PotentialStackAlias { pc });
        }
        _ => {
            if let Some(register) = written_gpr(word) {
                state.set_register(
                    register,
                    Value::open(SliceBlocker::UnsupportedRegisterWrite { pc, register }),
                );
            }
        }
    }
}

fn add_immediate(
    base: &Value,
    immediate: i32,
    pc: VirtualAddress,
    register: u8,
    trapping: bool,
) -> Value {
    match base {
        Value::Constant { value, .. } => {
            if trapping {
                let signed = *value as i32;
                let Some(result) = signed.checked_add(immediate) else {
                    return Value::open(SliceBlocker::SignedOverflowCannotBeExcluded {
                        pc,
                        register,
                    });
                };
                Value::Constant {
                    value: result as u32,
                    source: OperandSource::RegisterDefinition { register, pc },
                }
            } else {
                Value::Constant {
                    value: value.wrapping_add(immediate as u32),
                    source: OperandSource::RegisterDefinition { register, pc },
                }
            }
        }
        Value::StackPointer { offset } if !trapping => match offset.checked_add(immediate) {
            Some(offset) => Value::StackPointer { offset },
            None => Value::open(SliceBlocker::AddressArithmeticOverflow { pc }),
        },
        Value::StackPointer { .. } => {
            Value::open(SliceBlocker::SignedOverflowCannotBeExcluded { pc, register })
        }
        Value::Open(blockers) => Value::Open(blockers.clone()),
    }
}

fn store_word(
    state: &mut SliceState,
    pc: VirtualAddress,
    base: u8,
    value_register: u8,
    immediate: i32,
) {
    match state.registers[base as usize].clone() {
        Value::StackPointer { offset } => {
            let Some(address) = offset.checked_add(immediate) else {
                state.clobber_memory(SliceBlocker::AddressArithmeticOverflow { pc });
                return;
            };
            let stored = match &state.registers[value_register as usize] {
                Value::Constant { value, .. } => Value::Constant {
                    value: *value,
                    source: OperandSource::StackStore {
                        offset: address,
                        pc,
                    },
                },
                other => other.clone(),
            };
            state.stack_words.insert(address, stored);
        }
        Value::Constant { value: base, .. } => {
            let address = if immediate >= 0 {
                base.checked_add(immediate as u32)
            } else {
                base.checked_sub(immediate.unsigned_abs())
            };
            let Some(address) = address else {
                state.clobber_memory(SliceBlocker::AddressArithmeticOverflow { pc });
                return;
            };
            let stored = match &state.registers[value_register as usize] {
                Value::Constant { value, .. } => Value::Constant {
                    value: *value,
                    source: OperandSource::MemoryStore { address, pc },
                },
                other => other.clone(),
            };
            state.memory_words.insert(address, stored);
        }
        Value::Open(_) => state.clobber_memory(SliceBlocker::PotentialStackAlias { pc }),
    }
}

fn written_gpr(word: u32) -> Option<u8> {
    let op = word >> 26;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let rd = ((word >> 11) & 0x1f) as u8;
    match op {
        0 => (rd != 0).then_some(rd),
        0x03 => Some(31),
        0x08..=0x0f | 0x18..=0x1b | 0x20..=0x27 | 0x30..=0x37 => (rt != 0).then_some(rt),
        0x10..=0x13 if matches!(rs, 0..=2) => (rt != 0).then_some(rt),
        _ => None,
    }
}

fn is_control_transfer(word: u32) -> bool {
    let op = word >> 26;
    match op {
        0 => matches!(word & 0x3f, 0x08 | 0x09 | 0x0c | 0x0d),
        0x01..=0x07 | 0x14..=0x17 => true,
        0x10..=0x13 => ((word >> 21) & 0x1f) == 0x08,
        _ => false,
    }
}

fn direct_jal_target(word: u32, pc: VirtualAddress) -> Option<u32> {
    (word >> 26 == 0x03)
        .then(|| ((pc.get().wrapping_add(4)) & 0xf000_0000) | ((word & 0x03ff_ffff) << 2))
}

fn pc_at(start: VirtualAddress, index: usize) -> VirtualAddress {
    VirtualAddress::new(start.get().wrapping_add((index as u32).wrapping_mul(4)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: u32 = 0x8000_1000;
    const CALLEE: u32 = 0x8000_4000;

    fn i(op: u32, rs: u8, rt: u8, immediate: i16) -> u32 {
        (op << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | immediate as u16 as u32
    }

    fn jal(target: u32) -> u32 {
        (0x03 << 26) | ((target >> 2) & 0x03ff_ffff)
    }

    fn sw(base: u8, value: u8, offset: i16) -> u32 {
        i(0x2b, base, value, offset)
    }

    fn canonical_call(device: u32, dram: u32, size: u32) -> Vec<u32> {
        vec![
            i(0x09, 29, 29, -0x30),
            i(0x09, 0, 6, 0),
            i(0x0f, 0, 7, (device >> 16) as i16),
            i(0x0d, 7, 7, device as i16),
            i(0x0f, 0, 8, (dram >> 16) as i16),
            i(0x0d, 8, 8, dram as i16),
            sw(29, 8, 0x10),
            i(0x0f, 0, 9, (size >> 16) as i16),
            i(0x0d, 9, 9, size as i16),
            sw(29, 9, 0x14),
            jal(CALLEE),
            0,
        ]
    }

    fn canonical_epi_call(device: u32, dram: u32, size: u32, global: bool) -> Vec<u32> {
        let message_setup = if global {
            vec![i(0x0f, 0, 5, 0x8001u16 as i16), i(0x0d, 5, 5, 0x2000)]
        } else {
            vec![i(0x09, 29, 5, 0x18)]
        };
        let mut words = vec![
            i(0x09, 29, 29, -0x40),
            i(0x0f, 0, 4, 0x8000u16 as i16),
            i(0x0d, 4, 4, 0x3000),
        ];
        words.extend(message_setup);
        words.extend([
            i(0x09, 0, 6, 0),
            i(0x0f, 0, 8, (dram >> 16) as i16),
            i(0x0d, 8, 8, dram as i16),
            sw(5, 8, 0x08),
            i(0x0f, 0, 9, (device >> 16) as i16),
            i(0x0d, 9, 9, device as i16),
            sw(5, 9, 0x0c),
            i(0x0f, 0, 10, (size >> 16) as i16),
            i(0x0d, 10, 10, size as i16),
            sw(5, 10, 0x10),
            jal(CALLEE),
            0,
        ]);
        words
    }

    #[test]
    fn recovers_exact_read_geometry_but_keeps_candidate_type() {
        let words = canonical_call(0x0012_3400, 0x8030_0000, 0x2400);
        let slices = slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap();
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].direction.proven(), Some(&PiDmaDirection::ToRdram));
        assert_eq!(
            slices[0].device_address.proven(),
            Some(&PiDeviceAddress::new(0x0012_3400))
        );
        assert_eq!(
            slices[0].rdram_address.proven(),
            Some(&RdramAddress::new(0x0030_0000))
        );
        assert_eq!(slices[0].byte_count.proven().map(|v| v.get()), Some(0x2400));
        assert_eq!(
            slices[0].candidate(),
            Some(StaticPiDmaCandidate {
                call_pc: VirtualAddress::new(START + 40),
                direction: PiDmaDirection::ToRdram,
                device_address: PiDeviceAddress::new(0x0012_3400),
                rdram_address: RdramAddress::new(0x0030_0000),
                byte_count: NonZeroU32::new(0x2400).unwrap(),
            })
        );
    }

    #[test]
    fn delay_slot_write_is_visible_to_callee() {
        let mut words = canonical_call(0x1000, 0x8000_2000, 0x80);
        words[9] = 0;
        words[11] = sw(29, 9, 0x14);
        let slice = &slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert_eq!(slice.byte_count.proven().map(|v| v.get()), Some(0x80));
    }

    #[test]
    fn recovers_epi_message_geometry_from_stack_local_object() {
        let words = canonical_epi_call(0x0012_3400, 0x8030_0000, 0x2400, false);
        let slices = slice_os_epi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap();
        assert_eq!(slices.len(), 1);
        let slice = &slices[0];
        assert_eq!(slice.direction.proven(), Some(&PiDmaDirection::ToRdram));
        assert_eq!(
            slice.device_address.proven(),
            Some(&PiDeviceAddress::new(0x0012_3400))
        );
        assert_eq!(
            slice.rdram_address.proven(),
            Some(&RdramAddress::new(0x0030_0000))
        );
        assert_eq!(
            slice.byte_count.proven().map(|size| size.get()),
            Some(0x2400)
        );
        assert!(matches!(
            slice.message_pointer,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::StackPointerUnresolved]
        ));
        assert!(slice.handle_pointer.proven().is_some());
        assert!(slice.candidate().is_some());
    }

    #[test]
    fn recovers_epi_message_geometry_from_global_object() {
        let words = canonical_epi_call(0x2000, 0xa000_4000, 0x80, true);
        let slice = &slice_os_epi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert_eq!(
            slice.message_pointer.proven(),
            Some(&VirtualAddress::new(0x8001_2000))
        );
        assert_eq!(
            slice.rdram_address.proven(),
            Some(&RdramAddress::new(0x4000))
        );
    }

    #[test]
    fn epi_delay_slot_message_store_is_visible() {
        let mut words = canonical_epi_call(0x2000, 0x8000_4000, 0x80, false);
        let call = words.len() - 2;
        words[call - 1] = 0;
        words[call + 1] = sw(5, 10, 0x10);
        let slice = &slice_os_epi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert_eq!(slice.byte_count.proven().map(|size| size.get()), Some(0x80));
    }

    #[test]
    fn missing_epi_message_field_and_ambiguous_store_alias_stay_open() {
        let mut missing = canonical_epi_call(0x2000, 0x8000_4000, 0x80, false);
        let size_store = missing.len() - 3;
        missing[size_store] = 0;
        let slice = &slice_os_epi_start_dma_calls(
            &missing,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.byte_count,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::MessageFieldNotWritten { offset: 0x10 }]
        ));

        let mut aliased = canonical_epi_call(0x2000, 0x8000_4000, 0x80, false);
        let call = aliased.len() - 2;
        aliased.insert(call, sw(11, 10, 0));
        let slice = &slice_os_epi_start_dma_calls(
            &aliased,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.byte_count,
            StaticOperand::Open { ref blockers }
                if matches!(blockers.as_slice(), [SliceBlocker::PotentialStackAlias { .. }])
        ));
    }

    #[test]
    fn unresolved_load_and_missing_stack_write_stay_open() {
        let mut words = canonical_call(0x1000, 0x8000_2000, 0x80);
        words[8] = i(0x23, 4, 9, 0);
        words[3] = i(0x23, 4, 7, 0);
        let slice = &slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.device_address,
            StaticOperand::Open { ref blockers }
                if matches!(blockers.as_slice(), [SliceBlocker::LoadedFromMemory { register: 7, .. }])
        ));
        assert!(matches!(
            slice.byte_count,
            StaticOperand::Open { ref blockers }
                if matches!(blockers.as_slice(), [SliceBlocker::LoadedFromMemory { register: 9, .. }])
        ));
        assert_eq!(slice.candidate(), None);
    }

    #[test]
    fn never_propagates_values_across_an_earlier_control_transfer() {
        let mut words = canonical_call(0x1000, 0x8000_2000, 0x80);
        words.insert(4, jal(0x8000_3000));
        words.insert(5, 0);
        let slice = &slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(slice.device_address.proven().is_none());
        assert!(slice.rdram_address.proven().is_some());
    }

    #[test]
    fn rejects_non_kseg_and_out_of_bounds_dram_ranges() {
        let non_kseg = canonical_call(0x1000, 0x1000, 0x80);
        let slice = &slice_os_pi_start_dma_calls(
            &non_kseg,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.rdram_address,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::DramPointerOutsideKseg { raw: 0x1000 }]
        ));

        let too_long = canonical_call(0x1000, 0x807f_fff0, 0x20);
        let slice = &slice_os_pi_start_dma_calls(
            &too_long,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.rdram_address,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::RdramRangeOutOfBounds {
                    end_exclusive: 0x80_0010,
                    rdram_len: 0x80_0000,
                }]
        ));
    }

    #[test]
    fn rejects_zero_size_invalid_direction_and_device_overflow() {
        let mut words = canonical_call(0xffff_fff0, 0x8000_2000, 0);
        words[1] = i(0x09, 0, 6, 3);
        let slice = &slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.direction,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::InvalidDirection { raw: 3 }]
        ));
        assert!(matches!(
            slice.byte_count,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::ZeroByteCount]
        ));

        let words = canonical_call(0xffff_fff0, 0x8000_2000, 0x20);
        let slice = &slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.device_address,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::DeviceRangeOverflow]
        ));
    }

    #[test]
    fn rejects_unaligned_overflowing_and_impossible_inputs() {
        assert_eq!(
            slice_os_pi_start_dma_calls(
                &[0],
                VirtualAddress::new(START + 2),
                VirtualAddress::new(CALLEE),
                0x80_0000,
            ),
            Err(PiDmaSliceError::ImageAddressUnaligned {
                start: VirtualAddress::new(START + 2),
            })
        );
        assert_eq!(
            slice_os_pi_start_dma_calls(
                &[0, 0],
                VirtualAddress::new(0xffff_fffc),
                VirtualAddress::new(CALLEE),
                0x80_0000,
            ),
            Err(PiDmaSliceError::ImageAddressOverflow)
        );
        assert!(matches!(
            slice_os_pi_start_dma_calls(
                &[0],
                VirtualAddress::new(START),
                VirtualAddress::new(CALLEE),
                KSEG_SIZE + 1,
            ),
            Err(PiDmaSliceError::RdramLengthOutsidePhysicalDomain { .. })
        ));
    }

    #[test]
    fn load_request_slice_folds_subu_of_link_constants() {
        // IDO computes a request's byte count as `subu end, start` of two
        // link-time constants (MM boot's Main_Init code load). The slicer
        // must fold it; a size taken from memory must stay open.
        const START: u32 = 0x8008_0000;
        const CALLEE: u32 = 0x8008_0c04;
        let jal = 0x0c00_0000 | ((CALLEE >> 2) & 0x03ff_ffff);
        let words = vec![
            0x3c03_00b4, // lui   r3, 0x00b4
            0x3c0f_00c8, // lui   r15, 0x00c8
            0x2466_c000, // addiu r6, r3, -0x4000   -> device 0x00b3c000
            0x25ef_a4e0, // addiu r15, r15, -0x5b20 -> 0x00c7a4e0
            0x3c05_800a, // lui   r5, 0x800a
            0x24a5_5ac0, // addiu r5, r5, 0x5ac0    -> dram 0x800a5ac0
            0x01e6_3823, // subu  r7, r15, r6       -> size 0x0013e4e0
            jal,         // jal   callee
            0x0000_0000, // nop
            0x0000_0000,
        ];
        let slices = slice_load_request_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x0080_0000,
            5,
            6,
            7,
        )
        .unwrap();
        assert_eq!(slices.len(), 1);
        let candidate = slices[0].candidate().expect("all operands constant");
        assert_eq!(candidate.device_address.get(), 0x00b3_c000);
        // MM's real map places code at exactly this size (0x13E4E0): the
        // addiu immediate 0xA4E0 sign-extends negative, which is the very
        // subtlety a naive unsigned reading of the fixture gets wrong.
        assert_eq!(candidate.byte_count.get(), 0x0013_e4e0);
        assert_eq!(slices[0].dram_pointer.proven().unwrap().get(), 0x800a_5ac0);
        assert_eq!(candidate.direction, PiDmaDirection::ToRdram);

        // Same call, size loaded from memory: candidate must not form.
        let mut open_words = words.clone();
        open_words[6] = 0x8ce7_0000; // lw r7, 0x0(r7)
        let slices = slice_load_request_calls(
            &open_words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x0080_0000,
            5,
            6,
            7,
        )
        .unwrap();
        assert!(slices[0].candidate().is_none());
    }
}
