//! Bounded static operand recovery for direct PI/EPI DMA calls.
//!
//! The public libultra contract is
//! `osPiStartDma(mb, priority, direction, devAddr, vAddr, nbytes, mq)`.
//! Under the N64 o32 ABI, `direction`/`devAddr` are in `$a2`/`$a3`, and
//! `vAddr`/`nbytes` are in the caller's stack argument area at
//! `$sp + 0x10`/`$sp + 0x14`.
//! This module recovers only constants established in the straight-line run
//! immediately before a direct `jal`, including the call's delay slot —
//! extended by exactly one ABI-carried allowance: across an intervening
//! DIRECT call, o32 callee-saved registers (`$s0..$s7`, `$gp`, `$sp`,
//! `$fp`) and the caller's tracked frame slots persist, because the o32
//! contract obliges callees to preserve them, UNLESS a frame address was
//! materialized in the bounded window (an escaped `&local` lets a callee
//! legally write the caller's frame). Caller-saved registers still never
//! cross any transfer; branches and indirect transfers remain full
//! barriers; escapes older than the bounded window are invisible, which is
//! one reason recovered geometry stays candidate-strength.
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
    pub pointer_register: u8,
    pub pointer: StaticOperand<VirtualAddress>,
}

/// Find direct calls to `callee` and recover the pointer argument in the
/// declared register. Which register — and what the pointer means — is the
/// caller's cited claim; this slicer only recovers straight-line constants.
/// The single-contract entry point delegates to the batch implementation so
/// both paths have identical operand semantics.
pub fn slice_pointer_arg_calls(
    image_words: &[u32],
    image_start: VirtualAddress,
    callee: VirtualAddress,
    rdram_len: u32,
    pointer_register: u8,
) -> Result<Vec<PointerArgCallSlice>, PiDmaSliceError> {
    slice_pointer_arg_call_contracts(
        image_words,
        image_start,
        rdram_len,
        &[(callee, pointer_register)],
    )
}

/// Recover several direct-call pointer contracts with one image scan.
/// Duplicate contracts collapse, callees and registers are matched exactly,
/// and output order is call-PC then register order regardless of input order.
pub fn slice_pointer_arg_call_contracts(
    image_words: &[u32],
    image_start: VirtualAddress,
    rdram_len: u32,
    contracts: &[(VirtualAddress, u8)],
) -> Result<Vec<PointerArgCallSlice>, PiDmaSliceError> {
    validate_input(image_words, image_start, rdram_len)?;
    let mut registers_by_callee = BTreeMap::<u32, BTreeSet<u8>>::new();
    for &(callee, pointer_register) in contracts {
        if pointer_register >= 32 {
            return Err(PiDmaSliceError::InvalidPointerRegister {
                register: pointer_register,
            });
        }
        registers_by_callee
            .entry(callee.get())
            .or_default()
            .insert(pointer_register);
    }
    if registers_by_callee.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for call_index in 0..image_words.len().saturating_sub(1) {
        let call_pc = pc_at(image_start, call_index);
        let Some(callee) = direct_jal_target(image_words[call_index], call_pc) else {
            continue;
        };
        let Some(registers) = registers_by_callee.get(&callee) else {
            continue;
        };
        let requested: Vec<u8> = registers.iter().copied().collect();
        let state = slice_state_at_call(image_words, image_start, call_index, &requested);
        for &pointer_register in registers {
            out.push(PointerArgCallSlice {
                call_pc,
                callee: VirtualAddress::new(callee),
                pointer_register,
                pointer: map_operand(register_operand(&state, pointer_register), |raw| {
                    Ok(VirtualAddress::new(raw))
                }),
            });
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PiDmaSliceError {
    EmptyImage,
    ImageAddressUnaligned { start: VirtualAddress },
    ImageAddressOverflow,
    RdramLengthOutsidePhysicalDomain { rdram_len: u32 },
    InvalidPointerRegister { register: u8 },
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

/// Registers the o32 ABI obliges a callee to preserve: `$s0..$s7`, `$gp`,
/// `$sp`, `$fp`. Everything else is caller-saved and resets at any call.
fn abi_preserved(register: u8) -> bool {
    matches!(register, 16..=23 | 28 | 29 | 30)
}

/// Does this word materialize a frame address into a general register (or
/// store `$sp` itself)? That is how `&stack_local` escapes to a callee,
/// which may then legally write the caller's frame — the condition that
/// disables caller-frame slot persistence across a call.
fn materializes_frame_address(word: u32) -> bool {
    let op = word >> 26;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    match op {
        0x09 | 0x19 => rs == 29 && rt != 29,
        0 => {
            matches!(word & 0x3f, 0x21 | 0x23 | 0x25 | 0x2d) && (rs == 29 || rt == 29) && {
                let rd = ((word >> 11) & 0x1f) as u8;
                rd != 29
            }
        }
        0x28..=0x2f | 0x38..=0x3f => rt == 29,
        _ => false,
    }
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
        if is_control_transfer(word)
            && index + 1 < call_index
            && !ordinary_branch_skips_call(word, index, call_index)
        {
            slice_start = index + 2;
        }
    }
    let budget_exhausted = slice_start == budget_start && budget_start != 0;
    let slice_start_pc = pc_at(image_start, slice_start);
    let mut state = SliceState::new(slice_start_pc, budget_exhausted);

    // ABI-carried history (a documented extension of the straight-line
    // contract): walk the window before `slice_start`. Across a DIRECT jal
    // the o32 contract preserves callee-saved registers and the caller's
    // frame, so tracked stack slots and preserved registers survive —
    // UNLESS a frame address escaped (a callee holding `&local` may write
    // the frame), the transfer is not a direct call, or a store went
    // through an unknown base. Escapes created before the bounded window
    // are invisible; the recovered operands remain candidate-strength and
    // are corroborated downstream, matching this module's contract.
    if slice_start > budget_start {
        let mut history = SliceState::new(pc_at(image_start, budget_start), budget_start != 0);
        let mut escaped = false;
        let mut index = budget_start;
        while index < call_index && index < slice_start {
            let word = words[index];
            escaped |= materializes_frame_address(word);
            if is_control_transfer(word) && index + 1 < call_index {
                let delay = words[index + 1];
                escaped |= materializes_frame_address(delay);
                execute(&mut history, pc_at(image_start, index + 1), delay);
                let soft = word >> 26 == 0x03 && !escaped;
                let after = pc_at(image_start, index + 2);
                history.memory_words.clear();
                if !soft {
                    history.stack_words.clear();
                }
                let carried_sp = history.registers[29].clone();
                for register in 1..32u8 {
                    if !(soft && abi_preserved(register)) {
                        history.set_register(
                            register,
                            Value::open(SliceBlocker::DefinitionNotRecovered {
                                register,
                                slice_start: after,
                                budget_exhausted: false,
                            }),
                        );
                    }
                }
                // `$sp` itself is preserved by every call and unchanged by
                // in-function branches; carrying it keeps stack-slot keys
                // consistent across the whole window.
                history.registers[29] = carried_sp;
                index += 2;
                continue;
            }
            execute(&mut history, pc_at(image_start, index), word);
            index += 1;
        }
        state.stack_words = history.stack_words;
        state.registers[29] = history.registers[29].clone();
        for register in [16u8, 17, 18, 19, 20, 21, 22, 23, 28, 30] {
            if !matches!(history.registers[register as usize], Value::Open(_)) {
                state.registers[register as usize] = history.registers[register as usize].clone();
            }
        }
    }

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

/// An ordinary conditional branch whose taken edge skips the call is not a
/// merge on the call path: reaching the call selects its not-taken edge, and
/// the ordinary delay slot executes on that edge. Branch-likely and REGIMM
/// forms stay hard boundaries because their delay/link semantics differ.
fn ordinary_branch_skips_call(word: u32, branch_index: usize, call_index: usize) -> bool {
    let op = word >> 26;
    if !matches!(op, 0x04..=0x07) {
        return false;
    }
    let rs = (word >> 21) & 0x1f;
    let rt = (word >> 16) & 0x1f;
    let always_taken = match op {
        0x04 => rs == rt,
        0x06 => rs == 0,
        _ => false,
    };
    if always_taken {
        return false;
    }
    let displacement = i64::from(word as i16);
    let target = branch_index as i64 + 1 + displacement;
    target > call_index.saturating_add(1) as i64
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
                let value = match (&state.registers[rs as usize], &state.registers[rt as usize]) {
                    (Value::Constant { value: lhs, .. }, Value::Constant { value: rhs, .. }) => {
                        Value::Constant {
                            value: match word & 0x3f {
                                0x21 => lhs.wrapping_add(*rhs),
                                0x23 => lhs.wrapping_sub(*rhs),
                                _ => lhs | rhs,
                            },
                            source: OperandSource::RegisterDefinition { register: rd, pc },
                        }
                    }
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
        // A word load from a TRACKED stack slot restores the stored value —
        // IDO spills call arguments and reloads them in the delay slot
        // (SM64's `dma_read` sites). `clobber_memory` clears the map, so a
        // hit can never be stale; every other load stays open.
        0x23 => {
            let restored = match state.registers[rs as usize].clone() {
                Value::StackPointer { offset } => offset
                    .checked_add(immediate)
                    .and_then(|address| state.stack_words.get(&address).cloned()),
                _ => None,
            };
            state.set_register(
                rt,
                restored.unwrap_or_else(|| {
                    Value::open(SliceBlocker::LoadedFromMemory { pc, register: rt })
                }),
            );
        }
        0x27 => state.set_register(
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

/// A game-local wrapper whose body establishes the o32 contract
/// `(destination, physical_start, physical_end_exclusive)` and issues the
/// transfer through a chunked `osPiStartDma`-shaped call.
///
/// This is a semantic body classification, not a symbol or byte signature.
/// The retained sites make the classification independently auditable against
/// the admitted image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalEndDmaWrapper {
    pub entry_va: u32,
    pub callers: Vec<u32>,
    pub nested_dma_call_pc: u32,
}

/// Which of the wrapper shape's required dataflow facts a candidate failed to
/// establish. A candidate must satisfy every one, so a rejection census over a
/// corpus names the specific fact the detector cannot recover rather than
/// reporting an undifferentiated "examined, not admitted".
///
/// Counts are per rejected candidate and one candidate may miss several facts,
/// so these sum to at least the rejection count, not exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WrapperRejectionCensus {
    /// Length was never computed as `a2 - a1`.
    pub no_end_minus_start: usize,
    /// No inner `osPiStartDma`-shaped call with the required argument shape.
    pub no_nested_dma_call: usize,
    pub destination_not_advanced: usize,
    pub physical_not_advanced: usize,
    pub remaining_not_reduced: usize,
    /// Body never loops backward, so it copies at most one chunk.
    pub no_backward_loop: usize,
    /// Bounded body ended without a return: the window is too small or this is
    /// not a self-contained function.
    pub no_return: usize,
}

impl WrapperRejectionCensus {
    fn record(&mut self, facts: &WrapperFacts) {
        if !facts.saw_end_minus_start {
            self.no_end_minus_start += 1;
        }
        if facts.nested_dma_call.is_none() {
            self.no_nested_dma_call += 1;
        }
        if !facts.destination_advanced {
            self.destination_not_advanced += 1;
        }
        if !facts.physical_advanced {
            self.physical_not_advanced += 1;
        }
        if !facts.remaining_reduced {
            self.remaining_not_reduced += 1;
        }
        if !facts.backward_loop {
            self.no_backward_loop += 1;
        }
        if !facts.saw_return {
            self.no_return += 1;
        }
    }
}

/// The dataflow facts [`classify_physical_end_wrapper`] establishes while
/// walking a candidate body. Admission requires all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct WrapperFacts {
    saw_end_minus_start: bool,
    nested_dma_call: Option<u32>,
    destination_advanced: bool,
    physical_advanced: bool,
    remaining_reduced: bool,
    backward_loop: bool,
    saw_return: bool,
}

impl WrapperFacts {
    fn admits(&self) -> Option<u32> {
        (self.saw_end_minus_start
            && self.destination_advanced
            && self.physical_advanced
            && self.remaining_reduced
            && self.backward_loop
            && self.saw_return)
            .then_some(self.nested_dma_call)
            .flatten()
    }
}

/// Bounded result of [`infer_physical_end_dma_wrappers`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PhysicalEndDmaWrapperInference {
    pub admitted: Vec<PhysicalEndDmaWrapper>,
    pub candidates_examined: usize,
    pub candidate_limit_hit: bool,
    /// Why examined candidates were not admitted.
    pub rejections: WrapperRejectionCensus,
}

const MAX_PHYSICAL_END_WRAPPER_CANDIDATES: usize = 4096;
const MAX_PHYSICAL_END_WRAPPER_WORDS: usize = 128;
const MIN_PHYSICAL_END_WRAPPER_CALLERS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WrapperValue {
    Unknown,
    Constant(u32),
    Stack(i32),
    Destination,
    PhysicalStart,
    PhysicalEnd,
    Length,
    DestinationCursor,
    PhysicalCursor,
}

#[derive(Clone)]
struct WrapperState {
    registers: [WrapperValue; 32],
    stack_words: BTreeMap<i32, WrapperValue>,
}

impl WrapperState {
    fn new() -> Self {
        let mut registers = [WrapperValue::Unknown; 32];
        registers[0] = WrapperValue::Constant(0);
        registers[4] = WrapperValue::Destination;
        registers[5] = WrapperValue::PhysicalStart;
        registers[6] = WrapperValue::PhysicalEnd;
        registers[29] = WrapperValue::Stack(0);
        Self {
            registers,
            stack_words: BTreeMap::new(),
        }
    }

    fn stack_word(&self, offset: i32) -> WrapperValue {
        let WrapperValue::Stack(sp) = self.registers[29] else {
            return WrapperValue::Unknown;
        };
        sp.checked_add(offset)
            .and_then(|address| self.stack_words.get(&address).copied())
            .unwrap_or(WrapperValue::Unknown)
    }

    fn clobber_caller_saved(&mut self) {
        for register in 1..=15 {
            self.registers[register] = WrapperValue::Unknown;
        }
        for register in 24..=27 {
            self.registers[register] = WrapperValue::Unknown;
        }
        self.registers[31] = WrapperValue::Unknown;
        self.registers[0] = WrapperValue::Constant(0);
    }
}

fn wrapper_move_or_arithmetic(lhs: WrapperValue, rhs: WrapperValue, funct: u32) -> WrapperValue {
    if matches!(funct, 0x21 | 0x25 | 0x2d) {
        if rhs == WrapperValue::Constant(0) {
            return lhs;
        }
        if lhs == WrapperValue::Constant(0) {
            return rhs;
        }
    }
    if funct == 0x23 && lhs == WrapperValue::PhysicalEnd && rhs == WrapperValue::PhysicalStart {
        return WrapperValue::Length;
    }
    if funct == 0x24
        && ((lhs == WrapperValue::Length && matches!(rhs, WrapperValue::Constant(_)))
            || (rhs == WrapperValue::Length && matches!(lhs, WrapperValue::Constant(_))))
    {
        return WrapperValue::Length;
    }
    if funct == 0x21 {
        return match (lhs, rhs) {
            (WrapperValue::Destination | WrapperValue::DestinationCursor, WrapperValue::Length)
            | (WrapperValue::Length, WrapperValue::Destination | WrapperValue::DestinationCursor) => {
                WrapperValue::DestinationCursor
            }
            (WrapperValue::PhysicalStart | WrapperValue::PhysicalCursor, WrapperValue::Length)
            | (WrapperValue::Length, WrapperValue::PhysicalStart | WrapperValue::PhysicalCursor) => {
                WrapperValue::PhysicalCursor
            }
            _ => WrapperValue::Unknown,
        };
    }
    if funct == 0x23 && lhs == WrapperValue::Length && rhs == WrapperValue::Length {
        return WrapperValue::Length;
    }
    WrapperValue::Unknown
}

fn execute_wrapper_word(state: &mut WrapperState, word: u32) -> (bool, bool, bool) {
    let op = word >> 26;
    let rs = ((word >> 21) & 0x1f) as usize;
    let rt = ((word >> 16) & 0x1f) as usize;
    let rd = ((word >> 11) & 0x1f) as usize;
    let immediate = (word as i16) as i32;
    let mut destination_advanced = false;
    let mut physical_advanced = false;
    let mut remaining_reduced = false;
    match op {
        0 => {
            let funct = word & 0x3f;
            if rd != 0 {
                let value =
                    wrapper_move_or_arithmetic(state.registers[rs], state.registers[rt], funct);
                destination_advanced = value == WrapperValue::DestinationCursor
                    && matches!(
                        state.registers[rs],
                        WrapperValue::Destination | WrapperValue::DestinationCursor
                    )
                    || value == WrapperValue::DestinationCursor
                        && matches!(
                            state.registers[rt],
                            WrapperValue::Destination | WrapperValue::DestinationCursor
                        );
                physical_advanced = value == WrapperValue::PhysicalCursor
                    && matches!(
                        state.registers[rs],
                        WrapperValue::PhysicalStart | WrapperValue::PhysicalCursor
                    )
                    || value == WrapperValue::PhysicalCursor
                        && matches!(
                            state.registers[rt],
                            WrapperValue::PhysicalStart | WrapperValue::PhysicalCursor
                        );
                remaining_reduced = funct == 0x23
                    && state.registers[rs] == WrapperValue::Length
                    && state.registers[rt] == WrapperValue::Length;
                state.registers[rd] = value;
            }
        }
        0x0f => state.registers[rt] = WrapperValue::Constant((word & 0xffff) << 16),
        0x0d => {
            state.registers[rt] = match state.registers[rs] {
                WrapperValue::Constant(value) => WrapperValue::Constant(value | (word & 0xffff)),
                value if (word & 0xffff) == 0 => value,
                _ => WrapperValue::Unknown,
            };
        }
        0x08 | 0x09 | 0x18 | 0x19 => {
            state.registers[rt] = match state.registers[rs] {
                WrapperValue::Stack(offset) => offset
                    .checked_add(immediate)
                    .map(WrapperValue::Stack)
                    .unwrap_or(WrapperValue::Unknown),
                WrapperValue::Constant(value) => {
                    WrapperValue::Constant(value.wrapping_add(immediate as u32))
                }
                WrapperValue::Length => WrapperValue::Length,
                _ => WrapperValue::Unknown,
            };
        }
        0x0c => {
            state.registers[rt] = match state.registers[rs] {
                WrapperValue::Length => WrapperValue::Length,
                WrapperValue::Constant(value) => WrapperValue::Constant(value & (word & 0xffff)),
                _ => WrapperValue::Unknown,
            };
        }
        0x23 => {
            state.registers[rt] = match state.registers[rs] {
                WrapperValue::Stack(offset) => offset
                    .checked_add(immediate)
                    .and_then(|address| state.stack_words.get(&address).copied())
                    .unwrap_or(WrapperValue::Unknown),
                _ => WrapperValue::Unknown,
            };
        }
        0x2b => {
            if let WrapperValue::Stack(offset) = state.registers[rs] {
                if let Some(address) = offset.checked_add(immediate) {
                    state.stack_words.insert(address, state.registers[rt]);
                }
            }
        }
        _ => {
            if let Some(register) = written_gpr(word) {
                state.registers[register as usize] = WrapperValue::Unknown;
            }
        }
    }
    (destination_advanced, physical_advanced, remaining_reduced)
}

fn branch_target(pc: u32, word: u32) -> Option<u32> {
    matches!(word >> 26, 0x01..=0x07 | 0x14..=0x17).then(|| {
        pc.wrapping_add(4)
            .wrapping_add(((word as i16 as i32) << 2) as u32)
    })
}

fn classify_physical_end_wrapper(
    words: &[u32],
    image_start: u32,
    entry_index: usize,
) -> WrapperFacts {
    let end = words
        .len()
        .min(entry_index.saturating_add(MAX_PHYSICAL_END_WRAPPER_WORDS));
    let mut state = WrapperState::new();
    let mut saw_end_minus_start = false;
    let mut nested_dma_call = None;
    let mut destination_advanced = false;
    let mut physical_advanced = false;
    let mut remaining_reduced = false;
    let mut backward_loop = false;
    let mut saw_return = false;
    let mut index = entry_index;
    while index < end {
        let word = words[index];
        let pc = image_start.wrapping_add((index as u32).wrapping_mul(4));
        if word == 0x03e0_0008 {
            saw_return = true;
            break;
        }
        if word >> 26 == 0x03 && index + 1 < end {
            let mut call_state = state.clone();
            execute_wrapper_word(&mut call_state, words[index + 1]);
            if call_state.registers[6] == WrapperValue::Constant(0)
                && matches!(
                    call_state.registers[7],
                    WrapperValue::PhysicalStart | WrapperValue::PhysicalCursor
                )
                && matches!(
                    call_state.stack_word(0x10),
                    WrapperValue::Destination | WrapperValue::DestinationCursor
                )
                && call_state.stack_word(0x14) == WrapperValue::Length
            {
                nested_dma_call = Some(pc);
            }
            state = call_state;
            state.clobber_caller_saved();
            index += 2;
            continue;
        }
        let rs = ((word >> 21) & 0x1f) as usize;
        let rt = ((word >> 16) & 0x1f) as usize;
        if word >> 26 == 0
            && word & 0x3f == 0x23
            && state.registers[rs] == WrapperValue::PhysicalEnd
            && state.registers[rt] == WrapperValue::PhysicalStart
        {
            saw_end_minus_start = true;
        }
        let (advanced_destination, advanced_physical, reduced_remaining) =
            execute_wrapper_word(&mut state, word);
        if nested_dma_call.is_some() {
            destination_advanced |= advanced_destination;
            physical_advanced |= advanced_physical;
            remaining_reduced |= reduced_remaining;
            if branch_target(pc, word)
                .is_some_and(|target| target <= nested_dma_call.expect("checked above"))
            {
                backward_loop = true;
            }
        }
        index += 1;
    }
    WrapperFacts {
        saw_end_minus_start,
        nested_dma_call,
        destination_advanced,
        physical_advanced,
        remaining_reduced,
        backward_loop,
        saw_return,
    }
}

/// Infer chunked physical-ROM loaders without a symbol, title-specific
/// address, or exact instruction signature.
///
/// A candidate must have at least two direct callers and its bounded body must
/// establish all of these dataflow facts: length is `a2 - a1`; an inner
/// `osPiStartDma`-shaped call passes direction zero, the physical cursor in
/// `$a3`, the destination cursor at `$sp+0x10`, and a length-derived chunk at
/// `$sp+0x14`; after that call both cursors advance by a length-derived chunk,
/// remaining length decreases, and control loops backward before returning.
pub fn infer_physical_end_dma_wrappers(
    words: &[u32],
    image_va_start: u32,
) -> PhysicalEndDmaWrapperInference {
    let mut callers = BTreeMap::<u32, Vec<u32>>::new();
    for (index, word) in words.iter().enumerate() {
        let pc = image_va_start.wrapping_add((index as u32).wrapping_mul(4));
        if let Some(target) = direct_jal_target(*word, VirtualAddress::new(pc)) {
            let offset = target.wrapping_sub(image_va_start);
            if offset & 3 == 0 && (offset as usize) / 4 < words.len() {
                callers.entry(target).or_default().push(pc);
            }
        }
    }
    let mut inference = PhysicalEndDmaWrapperInference::default();
    for (entry_va, caller_sites) in callers {
        if caller_sites.len() < MIN_PHYSICAL_END_WRAPPER_CALLERS {
            continue;
        }
        if inference.candidates_examined == MAX_PHYSICAL_END_WRAPPER_CANDIDATES {
            inference.candidate_limit_hit = true;
            break;
        }
        inference.candidates_examined += 1;
        let entry_index = (entry_va.wrapping_sub(image_va_start) / 4) as usize;
        let facts = classify_physical_end_wrapper(words, image_va_start, entry_index);
        if let Some(nested_dma_call_pc) = facts.admits() {
            inference.admitted.push(PhysicalEndDmaWrapper {
                entry_va,
                callers: caller_sites,
                nested_dma_call_pc,
            });
        } else {
            inference.rejections.record(&facts);
        }
    }
    inference
}

#[cfg(test)]
mod physical_end_wrapper_tests {
    use super::*;

    const START: u32 = 0x8000_1000;
    const WRAPPER: u32 = START + 0x20;
    const INNER_DMA: u32 = 0x8000_1800;

    fn i(op: u32, rs: u8, rt: u8, immediate: i16) -> u32 {
        (op << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | immediate as u16 as u32
    }

    fn r(rs: u8, rt: u8, rd: u8, funct: u32) -> u32 {
        ((rs as u32) << 21) | ((rt as u32) << 16) | ((rd as u32) << 11) | funct
    }

    fn jal(target: u32) -> u32 {
        (0x03 << 26) | ((target >> 2) & 0x03ff_ffff)
    }

    fn wrapper_image() -> Vec<u32> {
        vec![
            jal(WRAPPER),
            0,
            jal(WRAPPER),
            0,
            0x03e0_0008,
            0,
            0,
            0,
            i(0x09, 29, 29, -0x20),
            i(0x2b, 29, 4, 0x20),
            i(0x2b, 29, 5, 0x24),
            i(0x2b, 29, 6, 0x28),
            i(0x23, 29, 8, 0x28),
            i(0x23, 29, 9, 0x24),
            r(8, 9, 10, 0x23),
            i(0x09, 0, 11, -16),
            r(10, 11, 10, 0x24),
            i(0x2b, 29, 10, 0x1c),
            i(0x23, 29, 7, 0x24),
            i(0x23, 29, 11, 0x20),
            i(0x2b, 29, 11, 0x10),
            i(0x23, 29, 12, 0x1c),
            i(0x09, 0, 6, 0),
            jal(INNER_DMA),
            i(0x2b, 29, 12, 0x14),
            i(0x23, 29, 13, 0x20),
            i(0x23, 29, 14, 0x1c),
            r(13, 14, 13, 0x21),
            i(0x2b, 29, 13, 0x20),
            i(0x23, 29, 15, 0x24),
            r(15, 14, 15, 0x21),
            i(0x2b, 29, 15, 0x24),
            i(0x23, 29, 24, 0x1c),
            r(24, 14, 24, 0x23),
            i(0x2b, 29, 24, 0x1c),
            i(0x05, 24, 0, -18),
            0,
            0x03e0_0008,
            i(0x09, 29, 29, 0x20),
        ]
    }

    #[test]
    fn infers_end_address_chunk_wrapper_from_semantics() {
        let image = wrapper_image();
        let report = infer_physical_end_dma_wrappers(&image, START);
        assert!(!report.candidate_limit_hit);
        assert_eq!(report.candidates_examined, 1);
        assert_eq!(
            report.admitted,
            [PhysicalEndDmaWrapper {
                entry_va: WRAPPER,
                callers: vec![START, START + 8],
                nested_dma_call_pc: START + 23 * 4,
            }]
        );

        let mut db = crate::facts::FactDb::new();
        let diagnostics = crate::record_physical_end_dma_wrapper_candidates(&image, START, &mut db);
        assert_eq!(diagnostics.semantic_proof_unavailable, 1);
        assert!(db.proven_rom_mappings().is_empty());
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            crate::facts::Fact::Evidence { note, .. }
                if note.contains("CFG/path and inner-callee authority remain open")
        )));
    }

    #[test]
    fn rejects_count_semantics_and_non_looping_copy_shapes() {
        let mut count_semantics = wrapper_image();
        count_semantics[14] = r(8, 0, 10, 0x21);
        assert!(infer_physical_end_dma_wrappers(&count_semantics, START)
            .admitted
            .is_empty());

        let mut no_loop = wrapper_image();
        no_loop[35] = 0;
        assert!(infer_physical_end_dma_wrappers(&no_loop, START)
            .admitted
            .is_empty());
    }

    #[test]
    fn rejection_census_names_the_fact_the_candidate_failed() {
        // The facts are not independent: the loop, cursor, and remaining-length
        // facts are only evaluated once the inner DMA call is recognized, which
        // itself needs the length dataflow. So breaking the length computation
        // cascades, and the census reports every fact left unestablished rather
        // than a single root cause. What must hold is that the broken fact is
        // always named, and that a fact broken in isolation is named alone.
        let mut count_semantics = wrapper_image();
        count_semantics[14] = r(8, 0, 10, 0x21);
        let report = infer_physical_end_dma_wrappers(&count_semantics, START);
        assert_eq!(report.candidates_examined, 1);
        assert_eq!(report.rejections.no_end_minus_start, 1);

        // Breaking only the backward branch leaves every earlier fact intact,
        // so exactly one counter moves.
        let mut no_loop = wrapper_image();
        no_loop[35] = 0;
        let report = infer_physical_end_dma_wrappers(&no_loop, START);
        assert_eq!(
            report.rejections,
            WrapperRejectionCensus {
                no_backward_loop: 1,
                ..WrapperRejectionCensus::default()
            }
        );

        // An admitted candidate contributes nothing to the census.
        let report = infer_physical_end_dma_wrappers(&wrapper_image(), START);
        assert_eq!(report.admitted.len(), 1);
        assert_eq!(report.rejections, WrapperRejectionCensus::default());
    }

    #[test]
    fn rejects_a_single_caller_even_when_the_body_matches() {
        let mut image = wrapper_image();
        image[2] = 0;
        let report = infer_physical_end_dma_wrappers(&image, START);
        assert_eq!(report.candidates_examined, 0);
        assert!(report.admitted.is_empty());
    }
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
    fn caller_frame_slots_and_saved_registers_cross_direct_calls_only() {
        const START: u32 = 0x8000_0000;
        const CALLEE: u32 = 0x8000_2000;
        let jal_word = |target: u32| 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
        // sw of a constant arg to the frame, an intervening helper call,
        // then the sliced call reloading the spill in its delay slot —
        // the SM64 dma_read shape.
        let body = |middle: u32, escape: Option<u32>| {
            let mut words = vec![
                0x3c08_00aa, // lui   r8, 0x00aa      (spilled value)
                0xafa8_0024, // sw    r8, 0x24(sp)
            ];
            if let Some(word) = escape {
                words.push(word);
            }
            words.extend([
                middle,      // intervening transfer
                0x0000_0000, // its delay slot
                0x3c06_0123, // lui   r6, 0x0123      (device register a2)
                jal_word(CALLEE),
                0x8fa6_0024, // delay: lw r6, 0x24(sp) — reload the spill
                0x0000_0000,
            ]);
            words
        };
        let device = |words: &[u32]| {
            slice_pointer_arg_calls(
                words,
                VirtualAddress::new(START),
                VirtualAddress::new(CALLEE),
                0x80_0000,
                6,
            )
            .unwrap()[0]
                .pointer
                .proven()
                .copied()
        };

        // Direct helper call, no escape: the spill survives and the reload
        // yields the stored constant.
        let direct = body(jal_word(0x8000_3000), None);
        assert_eq!(device(&direct).map(|va| va.get()), Some(0x00aa_0000));

        // Same shape with a materialized frame address: persistence off.
        let escaped = body(jal_word(0x8000_3000), Some(0x27a4_0024)); // addiu a0, sp, 0x24
        assert_eq!(device(&escaped), None);

        // A branch instead of a call: full barrier, as ever.
        let branch = body(0x1000_0004, None); // beq r0, r0, +4
        assert_eq!(device(&branch), None);
    }

    #[test]
    fn batched_pointer_contracts_equal_independent_slices_and_deduplicate() {
        const OTHER_CALLEE: u32 = 0x8000_5000;
        let words = vec![
            i(0x0f, 0, 4, 0x8001u16 as i16),
            i(0x0d, 4, 4, 0x1110),
            i(0x0f, 0, 5, 0x8002u16 as i16),
            i(0x0d, 5, 5, 0x2220),
            jal(CALLEE),
            0,
            i(0x0f, 0, 4, 0x8003u16 as i16),
            i(0x0d, 4, 4, 0x3330),
            jal(OTHER_CALLEE),
            0,
        ];
        let start = VirtualAddress::new(START);
        let contracts = [
            (VirtualAddress::new(OTHER_CALLEE), 4),
            (VirtualAddress::new(CALLEE), 5),
            (VirtualAddress::new(CALLEE), 4),
            (VirtualAddress::new(CALLEE), 4),
        ];
        let batch =
            slice_pointer_arg_call_contracts(&words, start, 0x0080_0000, &contracts).unwrap();

        let mut independent = Vec::new();
        for &(callee, register) in &contracts[..3] {
            independent.extend(
                slice_pointer_arg_calls(&words, start, callee, 0x0080_0000, register).unwrap(),
            );
        }
        independent.sort_by_key(|slice| (slice.call_pc, slice.pointer_register));

        assert_eq!(batch, independent);
        assert_eq!(batch.len(), 3);
        assert_eq!(
            batch
                .iter()
                .map(|slice| (
                    slice.call_pc.get(),
                    slice.pointer_register,
                    slice.pointer.proven().map(|pointer| pointer.get()),
                ))
                .collect::<Vec<_>>(),
            vec![
                (START + 0x10, 4, Some(0x8001_1110)),
                (START + 0x10, 5, Some(0x8002_2220)),
                (START + 0x20, 4, Some(0x8003_3330)),
            ]
        );
    }

    #[test]
    fn pointer_contract_register_domain_is_checked_before_slicing() {
        assert_eq!(
            slice_pointer_arg_call_contracts(&[0], VirtualAddress::new(START), 0x0080_0000, &[],),
            Ok(Vec::new())
        );
        let error = slice_pointer_arg_call_contracts(
            &[0],
            VirtualAddress::new(START),
            0x0080_0000,
            &[(VirtualAddress::new(CALLEE), 32)],
        );
        assert_eq!(
            error,
            Err(PiDmaSliceError::InvalidPointerRegister { register: 32 })
        );
        assert_eq!(
            slice_pointer_arg_calls(
                &[0],
                VirtualAddress::new(START),
                VirtualAddress::new(CALLEE),
                0x0080_0000,
                32,
            ),
            error
        );
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

    fn branch_guarded_load_request(branch_op: u32, branch_target: usize) -> Vec<u32> {
        let branch_index = 2usize;
        let displacement = i16::try_from(branch_target - (branch_index + 1)).unwrap();
        vec![
            0x3c05_00b9,                      // lui   a1, 0x00b9
            0x24a5_ad30,                      // addiu a1, a1, -0x52d0
            i(branch_op, 8, 0, displacement), // branch to a point after/at call
            0x3c04_801c,                      // delay: lui a0, 0x801c
            0x3c0f_00ba,                      // lui   t7, 0x00ba
            0x25ef_da40,                      // addiu t7, t7, -0x25c0
            0x01e5_3023,                      // subu  a2, t7, a1
            jal(CALLEE),                      // call_index
            0x2484_6e80,                      // delay: addiu a0, a0, 0x6e80
            0,
            0,
            0,
        ]
    }

    #[test]
    fn load_request_slice_crosses_ordinary_branch_that_skips_the_call() {
        let words = branch_guarded_load_request(0x05, 10);
        let slices = slice_load_request_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x0080_0000,
            4,
            5,
            6,
        )
        .unwrap();
        let candidate = slices[0]
            .candidate()
            .expect("not-taken call path has exact operands");
        assert_eq!(slices[0].dram_pointer.proven().unwrap().get(), 0x801c_6e80);
        assert_eq!(candidate.device_address.get(), 0x00b8_ad30);
        assert_eq!(candidate.byte_count.get(), 0x0001_2d10);
    }

    #[test]
    fn load_request_slice_keeps_annulled_and_merging_branches_hard() {
        let mut always_taken = branch_guarded_load_request(0x04, 10);
        always_taken[2] = i(0x04, 8, 8, 7); // beq t0, t0, after_call
        for words in [
            branch_guarded_load_request(0x15, 10),
            branch_guarded_load_request(0x05, 7),
            branch_guarded_load_request(0x05, 8),
            always_taken,
        ] {
            let slices = slice_load_request_calls(
                &words,
                VirtualAddress::new(START),
                VirtualAddress::new(CALLEE),
                0x0080_0000,
                4,
                5,
                6,
            )
            .unwrap();
            assert!(slices[0].candidate().is_none());
        }
    }
}

/// A routine that accesses the PI registers directly -- the primitive every
/// cart-to-RDRAM load ultimately goes through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiPrimitive {
    pub entry_va: u32,
    /// `lui rt, 0xA460` sites inside the routine. The primitive touches the PI
    /// register block several times (address, address, length); a routine with
    /// one incidental reference is a weaker candidate and the count says so.
    pub register_sites: u32,
    /// Exact sorted PCs behind `register_sites`, retained so a receipt can be
    /// independently checked against the admitted image rather than trusting
    /// an aggregate count.
    pub register_site_pcs: Vec<u32>,
    /// Direct `jal` sites targeting this routine, anywhere in the scanned image.
    pub callers: Vec<u32>,
}

/// PI registers occupy `0xA460_xxxx`. An access materializes that upper half
/// with `lui`, which is a fixed absolute constant no engine convention can move.
const PI_REGISTER_UPPER: u32 = 0xA460;

fn is_lui_pi_register(word: u32) -> bool {
    (word >> 26) == 0x0f && (word & 0xFFFF) == PI_REGISTER_UPPER
}

fn is_jr_ra(word: u32) -> bool {
    word == 0x03e0_0008
}

/// Locate the routines that drive the PI registers, and who calls them.
///
/// This is the non-circular half of static DMA recovery. Recovering a transfer's
/// OPERANDS statically is circular on most ROMs -- the vrom/size arguments are
/// read from the very table a scan would be trying to find -- but locating the
/// ROUTINE is not circular at all. PI registers sit at a fixed absolute address,
/// an access has to materialize `0xA460` with `lui`, and the IPL3 boot image is
/// always uncompressed at an address the ROM header states. So the primitive is
/// findable with no table, no answer key, and no emulator.
///
/// Measured `lui rt,0xA460` sites in the boot image: Super Mario 64 9,
/// GoldenEye 42, Perfect Dark 12, WCW World Tour 47, Majora's Mask 51 --
/// present in every ROM, including all five that compose to the boot bank alone.
///
/// Enclosing-routine attribution walks back to the nearest `jr $ra` and takes
/// the following delay slot's successor as the entry. That is the o32 shape of
/// a function boundary and needs no symbol; a site with no preceding return in
/// the image is attributed to the image start.
///
/// # Callers are only found inside the image you pass
///
/// Measured on Majora's Mask, whose DMA wrapper is known independently to live
/// at 0x80090270 (recovered from a live capture: 888 of 889 transfers issued
/// from it): that address has ZERO `jal` callers in the boot image, out of 6,221
/// distinct jal targets there. Its callers are in `code`, a separately-loaded
/// file outside the 1 MiB boot window.
///
/// So the boot image locates the PRIMITIVE but generally not its callers. To
/// recover those, pass a composed image -- the banks a composition strategy or
/// [`crate::delta_vote::sweep_untabled_regions`] produced -- rather than the
/// boot copy alone. The API takes arbitrary `(bytes, va_start)` for exactly
/// that reason.
///
/// Everything here is a CANDIDATE. Instruction bytes do not prove a routine is
/// reached, and a `jal` in the image does not prove the call executes.
pub fn recover_pi_primitives(image_bytes: &[u8], image_va_start: u32) -> Vec<PiPrimitive> {
    let words: Vec<u32> = image_bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes(chunk.try_into().expect("four bytes")))
        .collect();
    recover_pi_primitives_words(&words, image_va_start)
}

/// Word-oriented form for callers which already decoded an admitted image.
/// This avoids rematerializing the same complete word vector in inventory
/// pipelines that also perform cache and direct-call scans.
pub fn recover_pi_primitives_words(words: &[u32], image_va_start: u32) -> Vec<PiPrimitive> {
    use std::collections::BTreeMap;

    let va_at = |index: usize| image_va_start.wrapping_add((index as u32).wrapping_mul(4));

    // Attribute every PI register access to its enclosing routine.
    let mut sites_per_entry: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut last_return = None;
    for (index, word) in words.iter().enumerate() {
        if is_jr_ra(*word) {
            last_return = Some(index);
        }
        if !is_lui_pi_register(*word) {
            continue;
        }
        // `jr $ra` plus its delay slot ends the routine; retaining the most
        // recent return is exactly the prior reverse search, but makes the
        // complete fixed-address inventory linear in image size.
        let entry_index = last_return
            .map(|return_index| (return_index + 2).min(words.len().saturating_sub(1)))
            .unwrap_or(0);
        sites_per_entry
            .entry(va_at(entry_index))
            .or_default()
            .push(va_at(index));
    }
    if sites_per_entry.is_empty() {
        return Vec::new();
    }

    // Callers: one pass over the image collecting direct calls to any candidate.
    let mut callers_per_entry: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (index, word) in words.iter().enumerate() {
        let pc = va_at(index);
        if let Some(target) = direct_jal_target(*word, VirtualAddress::new(pc)) {
            if sites_per_entry.contains_key(&target) {
                callers_per_entry.entry(target).or_default().push(pc);
            }
        }
    }

    let mut primitives: Vec<PiPrimitive> = sites_per_entry
        .into_iter()
        .map(|(entry_va, register_site_pcs)| PiPrimitive {
            entry_va,
            register_sites: u32::try_from(register_site_pcs.len())
                .expect("PI register-site count exceeds u32"),
            register_site_pcs,
            callers: callers_per_entry.remove(&entry_va).unwrap_or_default(),
        })
        .collect();
    // Most register-driving first, then most-called, then address: deterministic
    // and puts the likeliest primitive at the front.
    primitives.sort_by(|a, b| {
        b.register_sites
            .cmp(&a.register_sites)
            .then(b.callers.len().cmp(&a.callers.len()))
            .then(a.entry_va.cmp(&b.entry_va))
    });
    primitives
}

#[cfg(test)]
mod pi_primitive_tests {
    use super::*;

    #[test]
    fn word_and_byte_entry_points_match_across_return_boundaries() {
        let words: [u32; 7] = [
            0x3c08_a460,
            0,
            0x03e0_0008,
            0x3c09_a460,
            0x3c0a_a460,
            0x03e0_0008,
            0,
        ];
        let bytes = words
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect::<Vec<_>>();

        let from_words = recover_pi_primitives_words(&words, 0x8000_1000);
        assert_eq!(from_words, recover_pi_primitives(&bytes, 0x8000_1000));
        assert_eq!(
            from_words
                .iter()
                .map(|primitive| (primitive.entry_va, primitive.register_site_pcs.clone()))
                .collect::<Vec<_>>(),
            vec![
                (0x8000_1010, vec![0x8000_100c, 0x8000_1010]),
                (0x8000_1000, vec![0x8000_1000]),
            ]
        );
    }
}

