//! Bounded, game-independent recognition of loader facts.
//!
//! This module intentionally recognizes only structures whose instruction
//! semantics are sufficient to establish the claim.  It does not assign
//! source-level names and it does not contain title IDs or per-ROM constants.
//!
//! Provenance: instruction decoding below follows the public MIPS III ISA
//! definitions for LUI, ADDIU, ORI, SW, BNE (including its delay slot), and
//! JAL's pseudo-direct target construction.  PI DMA argument roles follow the
//! public libultra `osPiStartDma` manual entry: device address, DRAM address,
//! byte count, direction, and asynchronous completion notification.

use std::fmt;
use std::num::NonZeroU32;

const OP_SPECIAL: u32 = 0;
const OP_J: u32 = 2;
const OP_JAL: u32 = 3;
const OP_BEQ: u32 = 4;
const OP_BNE: u32 = 5;
const OP_ADDI: u32 = 8;
const OP_ADDIU: u32 = 9;
const OP_ORI: u32 = 13;
const OP_LUI: u32 = 15;
const OP_SW: u32 = 43;

/// A virtual address in the decoded MIPS address space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtualAddress(u32);

impl VirtualAddress {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// A byte offset into a normalized, big-endian ROM image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RomOffset(u32);

impl RomOffset {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// A physical byte address in RDRAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RdramAddress(u32);

impl RdramAddress {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Exact semantics established by an accepted zero-fill loop.
///
/// `start..end_exclusive` is proven from LUI plus ADDIU/ORI construction.
/// The recognizer also proves that every four-byte word in one loop stride is
/// stored from register zero, that the induction register advances by exactly
/// `stride`, and that BNE returns to the first store until it equals the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProvenZeroFillLoop {
    pub loop_pc: VirtualAddress,
    pub branch_pc: VirtualAddress,
    pub start: VirtualAddress,
    pub end_exclusive: VirtualAddress,
    pub stride: NonZeroU32,
    pub induction_register: u8,
    pub end_register: u8,
}

/// A statically decoded direct-call target after the zero-fill loop.
///
/// The address and direct-call relationship are proven if the hardware entry
/// root reaches this instruction.  `candidate_role` remains a candidate:
/// instruction bytes alone cannot establish the source-level role "main".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostClearDirectCall {
    pub call_pc: VirtualAddress,
    pub target: VirtualAddress,
    pub candidate_role: CandidateCallRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateCallRole {
    MainEntry,
}

/// A strictly recognized entry-stub structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryStubObservation {
    pub zero_fill: ProvenZeroFillLoop,
    pub post_clear_direct_call: Option<PostClearDirectCall>,
}

/// Exact semantics established by a countdown zero-fill loop.
///
/// Unlike [`ProvenZeroFillLoop`], this form does not construct an end pointer.
/// It constructs a nonzero byte count, stores every word in one stride,
/// advances the base by `stride`, subtracts that same stride from the remaining
/// count, and loops until the remaining count is zero. `end_exclusive` is a
/// checked derivation from `start + byte_count`, not a decoded pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProvenCountdownZeroFillLoop {
    pub loop_pc: VirtualAddress,
    pub branch_pc: VirtualAddress,
    pub start: VirtualAddress,
    pub end_exclusive: VirtualAddress,
    pub byte_count: NonZeroU32,
    pub stride: NonZeroU32,
    pub base_register: u8,
    pub remaining_count_register: u8,
    pub base_update_kind: LoopAddKind,
    pub count_update_kind: LoopAddKind,
}

/// The distinct MIPS arithmetic instruction used by one loop update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopAddKind {
    /// ADDI traps on signed overflow. Accepted only after range proof.
    TrappingAddi,
    /// ADDIU sign-extends the same immediate but does not trap on signed
    /// overflow.
    NonTrappingAddiu,
}

/// A target constructed in a register and reached with `jr` after clearing.
///
/// This is not a direct call: `jr` does not write a link register. The target
/// and transfer are proven instruction semantics if the hardware entry reaches
/// the site; the source-level role `MainEntry` remains only a candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostClearConstructedJump {
    pub construction_pc: VirtualAddress,
    pub jump_pc: VirtualAddress,
    pub target: VirtualAddress,
    pub target_register: u8,
    pub stack_setup: PostClearStackSetup,
    pub candidate_role: CandidateTransferRole,
}

/// Stack-pointer semantics proven in the transfer's delay slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostClearStackSetup {
    /// A four-word handoff adjusts an already established stack pointer.
    RelativeAdjustment { amount: i16 },
    /// A five-word handoff constructs the stack pointer with adjacent
    /// LUI/ADDIU instructions. The ADDIU executes in the jump delay slot.
    Constructed {
        construction_pc: VirtualAddress,
        address: VirtualAddress,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateTransferRole {
    MainEntry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CountdownEntryStubObservation {
    pub zero_fill: ProvenCountdownZeroFillLoop,
    pub post_clear_constructed_jump: Option<PostClearConstructedJump>,
}

/// Every strict entry-stub form currently understood by this module.
///
/// [`recognize_entry_stub`] remains the compatibility entry point for the
/// end-pointer form. New callers should use [`recognize_entry_stub_any`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecognizedEntryStub {
    EndPointer(EntryStubObservation),
    Countdown(CountdownEntryStubObservation),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopRejectReason {
    MissingInductionUpdate,
    MultipleInductionUpdates,
    NonPositiveOrUnalignedStride,
    LoopContainsNonStoreOperation,
    StoreDoesNotUseZero,
    StoreOffsetOutsideStride,
    StoreCoverageHasGap,
    MissingStartAddressConstruction,
    MissingEndAddressConstruction,
    EmptyOrDescendingRange,
    RangeNotDivisibleByStride,
    MissingBaseUpdate,
    MultipleBaseUpdates,
    MissingCountUpdate,
    MultipleCountUpdates,
    CountUpdateDoesNotMatchBaseStride,
    MissingByteCountConstruction,
    ZeroByteCount,
    AddressRangeOverflow,
    SignedOverflowPossible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryStubRejectReason {
    EmptyWindow,
    EntryAddressUnaligned {
        entry: VirtualAddress,
    },
    WindowAddressOverflow,
    NoBackwardBne,
    NoCountdownBne,
    MalformedLoop {
        branch_pc: VirtualAddress,
        reason: LoopRejectReason,
    },
    AmbiguousZeroFillLoops {
        count: usize,
    },
}

impl fmt::Display for EntryStubRejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyWindow => write!(f, "entry-stub instruction window is empty"),
            Self::EntryAddressUnaligned { entry } => {
                write!(f, "entry address {:#010x} is not word-aligned", entry.get())
            }
            Self::WindowAddressOverflow => {
                write!(
                    f,
                    "entry-stub instruction window exceeds the MIPS address space"
                )
            }
            Self::NoBackwardBne => write!(f, "entry-stub window has no backward BNE loop"),
            Self::NoCountdownBne => write!(
                f,
                "entry-stub window has no backward BNE against register zero"
            ),
            Self::MalformedLoop { branch_pc, reason } => write!(
                f,
                "backward BNE at {:#010x} is not a proven zero-fill loop: {reason:?}",
                branch_pc.get()
            ),
            Self::AmbiguousZeroFillLoops { count } => write!(
                f,
                "entry-stub window contains {count} independently valid zero-fill loops"
            ),
        }
    }
}

impl std::error::Error for EntryStubRejectReason {}

/// Recognize one exact zero-fill loop in a hardware-rooted entry window.
///
/// This is deliberately strict.  Unsupported compiler-equivalent loop forms
/// are rejected instead of being promoted by resemblance.  When several
/// exact loops exist, ambiguity is returned rather than selecting the first.
pub fn recognize_entry_stub(
    words: &[u32],
    entry: VirtualAddress,
) -> Result<EntryStubObservation, EntryStubRejectReason> {
    if words.is_empty() {
        return Err(EntryStubRejectReason::EmptyWindow);
    }
    if entry.get() & 3 != 0 {
        return Err(EntryStubRejectReason::EntryAddressUnaligned { entry });
    }
    if (words.len() - 1) as u64 * 4 + entry.get() as u64 > u32::MAX as u64 {
        return Err(EntryStubRejectReason::WindowAddressOverflow);
    }

    let mut backward_bne_count = 0usize;
    let mut first_rejection = None;
    let mut accepted = Vec::new();

    for branch_index in 0..words.len() {
        if opcode(words[branch_index]) != OP_BNE {
            continue;
        }
        let Some(target_index) = branch_target_index(branch_index, words[branch_index]) else {
            continue;
        };
        if target_index >= branch_index {
            continue;
        }
        backward_bne_count += 1;

        match recognize_zero_fill_at(words, entry, branch_index, target_index) {
            Ok(zero_fill) => accepted.push(EntryStubObservation {
                zero_fill,
                post_clear_direct_call: find_post_clear_call(words, entry, branch_index),
            }),
            Err(reason) if first_rejection.is_none() => {
                first_rejection = Some((branch_index, reason));
            }
            Err(_) => {}
        }
    }

    match accepted.len() {
        1 => Ok(accepted[0]),
        count if count > 1 => Err(EntryStubRejectReason::AmbiguousZeroFillLoops { count }),
        _ if backward_bne_count == 0 => Err(EntryStubRejectReason::NoBackwardBne),
        _ => {
            let (branch_index, reason) = first_rejection.expect("a backward BNE was examined");
            Err(EntryStubRejectReason::MalformedLoop {
                branch_pc: pc_at(entry, branch_index),
                reason,
            })
        }
    }
}

/// Recognize one exact countdown zero-fill loop in a hardware-rooted entry
/// window.
///
/// The accepted loop has complete zero-store coverage for its stride, one
/// positive base update followed by one equal-magnitude negative count update,
/// BNE of the remaining count against zero, and an actual NOP delay slot.
pub fn recognize_countdown_entry_stub(
    words: &[u32],
    entry: VirtualAddress,
) -> Result<CountdownEntryStubObservation, EntryStubRejectReason> {
    validate_entry_window(words, entry)?;

    let mut countdown_bne_count = 0usize;
    let mut first_rejection = None;
    let mut accepted = Vec::new();

    for branch_index in 0..words.len() {
        let branch = words[branch_index];
        if opcode(branch) != OP_BNE {
            continue;
        }
        let (remaining_register, zero_register) = if rt(branch) == 0 {
            (rs(branch), rt(branch))
        } else if rs(branch) == 0 {
            (rt(branch), rs(branch))
        } else {
            continue;
        };
        debug_assert_eq!(zero_register, 0);
        if remaining_register == 0 {
            continue;
        }
        let Some(target_index) = branch_target_index(branch_index, branch) else {
            continue;
        };
        if target_index >= branch_index {
            continue;
        }
        countdown_bne_count += 1;

        match recognize_countdown_zero_fill_at(
            words,
            entry,
            branch_index,
            target_index,
            remaining_register,
        ) {
            Ok(zero_fill) => accepted.push(CountdownEntryStubObservation {
                zero_fill,
                post_clear_constructed_jump: find_post_clear_constructed_jump(
                    words,
                    entry,
                    branch_index,
                ),
            }),
            Err(reason) if first_rejection.is_none() => {
                first_rejection = Some((branch_index, reason));
            }
            Err(_) => {}
        }
    }

    match accepted.len() {
        1 => Ok(accepted[0]),
        count if count > 1 => Err(EntryStubRejectReason::AmbiguousZeroFillLoops { count }),
        _ if countdown_bne_count == 0 => Err(EntryStubRejectReason::NoCountdownBne),
        _ => {
            let (branch_index, reason) = first_rejection.expect("a countdown BNE was examined");
            Err(EntryStubRejectReason::MalformedLoop {
                branch_pc: pc_at(entry, branch_index),
                reason,
            })
        }
    }
}

/// Recognize either strict entry-stub form without collapsing their types.
pub fn recognize_entry_stub_any(
    words: &[u32],
    entry: VirtualAddress,
) -> Result<RecognizedEntryStub, EntryStubRejectReason> {
    let end_pointer = recognize_entry_stub(words, entry);
    let countdown = recognize_countdown_entry_stub(words, entry);
    match (end_pointer, countdown) {
        (Ok(_), Ok(_)) => Err(EntryStubRejectReason::AmbiguousZeroFillLoops { count: 2 }),
        (Err(EntryStubRejectReason::AmbiguousZeroFillLoops { count }), Ok(_))
        | (Ok(_), Err(EntryStubRejectReason::AmbiguousZeroFillLoops { count })) => {
            Err(EntryStubRejectReason::AmbiguousZeroFillLoops { count: count + 1 })
        }
        (
            Err(EntryStubRejectReason::AmbiguousZeroFillLoops { count: first }),
            Err(EntryStubRejectReason::AmbiguousZeroFillLoops { count: second }),
        ) => Err(EntryStubRejectReason::AmbiguousZeroFillLoops {
            count: first + second,
        }),
        (Ok(observation), Err(_)) => Ok(RecognizedEntryStub::EndPointer(observation)),
        (Err(_), Ok(observation)) => Ok(RecognizedEntryStub::Countdown(observation)),
        (Err(end_pointer_error), Err(EntryStubRejectReason::NoCountdownBne)) => {
            Err(end_pointer_error)
        }
        (Err(_), Err(countdown_error)) => Err(countdown_error),
    }
}

fn validate_entry_window(
    words: &[u32],
    entry: VirtualAddress,
) -> Result<(), EntryStubRejectReason> {
    if words.is_empty() {
        return Err(EntryStubRejectReason::EmptyWindow);
    }
    if entry.get() & 3 != 0 {
        return Err(EntryStubRejectReason::EntryAddressUnaligned { entry });
    }
    if (words.len() - 1) as u64 * 4 + entry.get() as u64 > u32::MAX as u64 {
        return Err(EntryStubRejectReason::WindowAddressOverflow);
    }
    Ok(())
}

fn recognize_zero_fill_at(
    words: &[u32],
    entry: VirtualAddress,
    branch_index: usize,
    target_index: usize,
) -> Result<ProvenZeroFillLoop, LoopRejectReason> {
    let branch = words[branch_index];
    let branch_rs = rs(branch);
    let branch_rt = rt(branch);

    let first = recognize_zero_fill_orientation(
        words,
        entry,
        branch_index,
        target_index,
        branch_rs,
        branch_rt,
    );
    if first.is_ok() {
        return first;
    }
    let second = recognize_zero_fill_orientation(
        words,
        entry,
        branch_index,
        target_index,
        branch_rt,
        branch_rs,
    );
    second.map_err(|_| first.unwrap_err())
}

fn recognize_countdown_zero_fill_at(
    words: &[u32],
    entry: VirtualAddress,
    branch_index: usize,
    target_index: usize,
    remaining_register: u8,
) -> Result<ProvenCountdownZeroFillLoop, LoopRejectReason> {
    let mut base_register = None;
    let mut base_stride = None;
    let mut count_delta = None;
    let mut base_update_kind = None;
    let mut count_update_kind = None;
    let mut store_offsets = Vec::new();

    for (relative_index, &word) in words[target_index..branch_index].iter().enumerate() {
        let index = target_index + relative_index;
        match opcode(word) {
            OP_SW => {
                if rt(word) != 0 {
                    return Err(LoopRejectReason::StoreDoesNotUseZero);
                }
                let register = rs(word);
                if register == remaining_register || register == 0 {
                    return Err(LoopRejectReason::LoopContainsNonStoreOperation);
                }
                if let Some(base) = base_register {
                    if register != base {
                        return Err(LoopRejectReason::LoopContainsNonStoreOperation);
                    }
                } else {
                    base_register = Some(register);
                }
                store_offsets.push(sign_extended_immediate(word));
            }
            OP_ADDI | OP_ADDIU
                if rs(word) == remaining_register && rt(word) == remaining_register =>
            {
                if count_delta.is_some() {
                    return Err(LoopRejectReason::MultipleCountUpdates);
                }
                if index + 1 != branch_index {
                    return Err(LoopRejectReason::LoopContainsNonStoreOperation);
                }
                let delta = sign_extended_immediate(word);
                if delta >= 0 || delta & 3 != 0 {
                    return Err(LoopRejectReason::CountUpdateDoesNotMatchBaseStride);
                }
                count_delta = Some(delta);
                count_update_kind = Some(loop_add_kind(word));
            }
            OP_ADDI | OP_ADDIU if rs(word) == rt(word) && rs(word) != 0 => {
                if base_stride.is_some() {
                    return Err(LoopRejectReason::MultipleBaseUpdates);
                }
                if index + 2 != branch_index {
                    return Err(LoopRejectReason::LoopContainsNonStoreOperation);
                }
                if base_register.is_some_and(|base| base != rs(word)) {
                    return Err(LoopRejectReason::LoopContainsNonStoreOperation);
                }
                base_register = Some(rs(word));
                let delta = sign_extended_immediate(word);
                if delta <= 0 || delta & 3 != 0 {
                    return Err(LoopRejectReason::NonPositiveOrUnalignedStride);
                }
                base_stride = NonZeroU32::new(delta as u32);
                base_update_kind = Some(loop_add_kind(word));
            }
            _ => return Err(LoopRejectReason::LoopContainsNonStoreOperation),
        }
    }

    let base_register = base_register.ok_or(LoopRejectReason::MissingBaseUpdate)?;
    let stride = base_stride.ok_or(LoopRejectReason::MissingBaseUpdate)?;
    let count_delta = count_delta.ok_or(LoopRejectReason::MissingCountUpdate)?;
    let base_update_kind = base_update_kind.expect("a base stride records its opcode");
    let count_update_kind = count_update_kind.expect("a count delta records its opcode");
    if count_delta.unsigned_abs() != stride.get() {
        return Err(LoopRejectReason::CountUpdateDoesNotMatchBaseStride);
    }

    store_offsets.sort_unstable();
    store_offsets.dedup();
    let expected_offsets: Vec<i32> = (0..stride.get()).step_by(4).map(|v| v as i32).collect();
    if store_offsets
        .iter()
        .any(|offset| *offset < 0 || *offset as u32 >= stride.get() || *offset as u32 & 3 != 0)
    {
        return Err(LoopRejectReason::StoreOffsetOutsideStride);
    }
    if store_offsets != expected_offsets {
        return Err(LoopRejectReason::StoreCoverageHasGap);
    }

    if words.get(branch_index + 1).copied() != Some(0) {
        return Err(LoopRejectReason::LoopContainsNonStoreOperation);
    }

    let start = find_address_construction(words, base_register, target_index)
        .ok_or(LoopRejectReason::MissingStartAddressConstruction)?;
    let byte_count = find_address_construction(words, remaining_register, target_index)
        .ok_or(LoopRejectReason::MissingByteCountConstruction)?;
    let byte_count = NonZeroU32::new(byte_count).ok_or(LoopRejectReason::ZeroByteCount)?;
    if byte_count.get() % stride.get() != 0 {
        return Err(LoopRejectReason::RangeNotDivisibleByStride);
    }
    let end = start
        .checked_add(byte_count.get())
        .ok_or(LoopRejectReason::AddressRangeOverflow)?;
    // MIPS III ADDI traps when signed operands overflow. For a positive base,
    // monotonic +stride updates are safe exactly while the inclusive final
    // result remains at or below i32::MAX. A KSEG base is already negative;
    // adding a positive immediate cannot signed-overflow before the checked
    // u32 range itself wraps. The countdown must begin nonnegative because
    // every ADDI subtracts stride until the proven terminal value zero.
    if base_update_kind == LoopAddKind::TrappingAddi
        && start <= i32::MAX as u32
        && end > i32::MAX as u32
    {
        return Err(LoopRejectReason::SignedOverflowPossible);
    }
    if count_update_kind == LoopAddKind::TrappingAddi && byte_count.get() > i32::MAX as u32 {
        return Err(LoopRejectReason::SignedOverflowPossible);
    }

    Ok(ProvenCountdownZeroFillLoop {
        loop_pc: pc_at(entry, target_index),
        branch_pc: pc_at(entry, branch_index),
        start: VirtualAddress::new(start),
        end_exclusive: VirtualAddress::new(end),
        byte_count,
        stride,
        base_register,
        remaining_count_register: remaining_register,
        base_update_kind,
        count_update_kind,
    })
}

fn loop_add_kind(word: u32) -> LoopAddKind {
    match opcode(word) {
        OP_ADDI => LoopAddKind::TrappingAddi,
        OP_ADDIU => LoopAddKind::NonTrappingAddiu,
        _ => unreachable!("caller accepts only ADDI or ADDIU"),
    }
}

fn recognize_zero_fill_orientation(
    words: &[u32],
    entry: VirtualAddress,
    branch_index: usize,
    target_index: usize,
    induction_register: u8,
    end_register: u8,
) -> Result<ProvenZeroFillLoop, LoopRejectReason> {
    let mut stride = None;
    let mut store_offsets = Vec::new();

    for (relative_index, &word) in words[target_index..branch_index].iter().enumerate() {
        let index = target_index + relative_index;
        match opcode(word) {
            OP_SW => {
                if rs(word) != induction_register {
                    return Err(LoopRejectReason::LoopContainsNonStoreOperation);
                }
                if rt(word) != 0 {
                    return Err(LoopRejectReason::StoreDoesNotUseZero);
                }
                store_offsets.push(sign_extended_immediate(word));
            }
            OP_ADDIU if rs(word) == induction_register && rt(word) == induction_register => {
                if stride.is_some() {
                    return Err(LoopRejectReason::MultipleInductionUpdates);
                }
                if index + 1 != branch_index {
                    return Err(LoopRejectReason::LoopContainsNonStoreOperation);
                }
                let immediate = sign_extended_immediate(word);
                if immediate <= 0 || immediate & 3 != 0 {
                    return Err(LoopRejectReason::NonPositiveOrUnalignedStride);
                }
                stride = NonZeroU32::new(immediate as u32);
            }
            _ => return Err(LoopRejectReason::LoopContainsNonStoreOperation),
        }
    }

    let stride = stride.ok_or(LoopRejectReason::MissingInductionUpdate)?;
    store_offsets.sort_unstable();
    store_offsets.dedup();
    let expected_offsets: Vec<i32> = (0..stride.get()).step_by(4).map(|v| v as i32).collect();
    if store_offsets
        .iter()
        .any(|offset| *offset < 0 || *offset as u32 >= stride.get() || *offset as u32 & 3 != 0)
    {
        return Err(LoopRejectReason::StoreOffsetOutsideStride);
    }
    if store_offsets != expected_offsets {
        return Err(LoopRejectReason::StoreCoverageHasGap);
    }

    // The public MIPS III BNE definition executes the following delay-slot
    // instruction on both paths.  Requiring NOP keeps the induction/coverage
    // proof local; a future recognizer may model other delay-slot operations.
    if words.get(branch_index + 1).copied() != Some(0) {
        return Err(LoopRejectReason::LoopContainsNonStoreOperation);
    }

    let start = find_address_construction(words, induction_register, target_index)
        .ok_or(LoopRejectReason::MissingStartAddressConstruction)?;
    let end = find_address_construction(words, end_register, target_index)
        .ok_or(LoopRejectReason::MissingEndAddressConstruction)?;
    if start >= end {
        return Err(LoopRejectReason::EmptyOrDescendingRange);
    }
    if end.wrapping_sub(start) % stride.get() != 0 {
        return Err(LoopRejectReason::RangeNotDivisibleByStride);
    }

    Ok(ProvenZeroFillLoop {
        loop_pc: pc_at(entry, target_index),
        branch_pc: pc_at(entry, branch_index),
        start: VirtualAddress::new(start),
        end_exclusive: VirtualAddress::new(end),
        stride,
        induction_register,
        end_register,
    })
}

fn find_address_construction(words: &[u32], register: u8, before: usize) -> Option<u32> {
    let mut low = None;
    for index in (0..before).rev() {
        let word = words[index];
        if !writes_register(word, register) {
            continue;
        }
        if low.is_none()
            && matches!(opcode(word), OP_ADDIU | OP_ORI)
            && rs(word) == register
            && rt(word) == register
        {
            low = Some(word);
            continue;
        }
        let low_word = low?;
        if opcode(word) != OP_LUI || rt(word) != register {
            return None;
        }
        let high = (word & 0xffff) << 16;
        return Some(match opcode(low_word) {
            OP_ADDIU => high.wrapping_add(sign_extended_immediate(low_word) as u32),
            OP_ORI => high | (low_word & 0xffff),
            _ => unreachable!("low construction opcode checked above"),
        });
    }
    None
}

fn find_post_clear_call(
    words: &[u32],
    entry: VirtualAddress,
    branch_index: usize,
) -> Option<PostClearDirectCall> {
    // Bound the semantic guess: a distant call is not evidence that the
    // zero-fill loop belongs to an entry stub.  Eight words permits ordinary
    // argument setup without turning this into an unbounded scan.
    let start = branch_index.saturating_add(2);
    let end = words.len().min(start.saturating_add(8));
    for (index, &word) in words.iter().enumerate().take(end).skip(start) {
        match opcode(word) {
            OP_JAL => {
                let call_pc = pc_at(entry, index);
                let target =
                    ((call_pc.get().wrapping_add(4)) & 0xf000_0000) | ((word & 0x03ff_ffff) << 2);
                return Some(PostClearDirectCall {
                    call_pc,
                    target: VirtualAddress::new(target),
                    candidate_role: CandidateCallRole::MainEntry,
                });
            }
            OP_J | OP_BEQ | OP_BNE => return None,
            OP_SPECIAL if matches!(word & 0x3f, 8 | 9) => return None,
            _ => {}
        }
    }
    None
}

fn find_post_clear_constructed_jump(
    words: &[u32],
    entry: VirtualAddress,
    branch_index: usize,
) -> Option<PostClearConstructedJump> {
    let index = branch_index.checked_add(2)?;
    let [high, low]: [u32; 2] = words.get(index..index.checked_add(2)?)?.try_into().ok()?;
    let target_register = rt(high);
    if opcode(high) != OP_LUI || rs(high) != 0 || target_register == 0 {
        return None;
    }
    if opcode(low) != OP_ADDIU || rs(low) != target_register || rt(low) != target_register {
        return None;
    }
    let target_high = (high & 0xffff) << 16;
    let target = target_high.wrapping_add(sign_extended_immediate(low) as u32);
    if target & 3 != 0 {
        return None;
    }

    let third = *words.get(index + 2)?;
    let (jump_offset, stack_setup) = if is_strict_jr(third, target_register) {
        let delay = *words.get(index + 3)?;
        if opcode(delay) != OP_ADDIU || rs(delay) != 29 || rt(delay) != 29 {
            return None;
        }
        let amount = sign_extended_immediate(delay);
        if amount >= 0 || amount & 7 != 0 {
            return None;
        }
        (
            2usize,
            PostClearStackSetup::RelativeAdjustment {
                amount: amount as i16,
            },
        )
    } else {
        let stack_high = third;
        if opcode(stack_high) != OP_LUI || rs(stack_high) != 0 || rt(stack_high) != 29 {
            return None;
        }
        let jump = *words.get(index + 3)?;
        let delay = *words.get(index + 4)?;
        if !is_strict_jr(jump, target_register)
            || opcode(delay) != OP_ADDIU
            || rs(delay) != 29
            || rt(delay) != 29
        {
            return None;
        }
        // ADDIU sign-extends its immediate and does not trap on overflow. The
        // reconstructed value, rather than the immediate alone, must satisfy
        // the N64 ABI's eight-byte stack alignment.
        let address =
            ((stack_high & 0xffff) << 16).wrapping_add(sign_extended_immediate(delay) as u32);
        if address & 7 != 0 {
            return None;
        }
        (
            3usize,
            PostClearStackSetup::Constructed {
                construction_pc: pc_at(entry, index + 2),
                address: VirtualAddress::new(address),
            },
        )
    };

    Some(PostClearConstructedJump {
        construction_pc: pc_at(entry, index),
        jump_pc: pc_at(entry, index + jump_offset),
        target: VirtualAddress::new(target),
        target_register,
        stack_setup,
        candidate_role: CandidateTransferRole::MainEntry,
    })
}

fn is_strict_jr(word: u32, register: u8) -> bool {
    opcode(word) == OP_SPECIAL
        && word & 0x3f == 8
        && rs(word) == register
        && word & 0x001f_ffff == 8
}

fn branch_target_index(branch_index: usize, word: u32) -> Option<usize> {
    let target = branch_index as i64 + 1 + sign_extended_immediate(word) as i64;
    usize::try_from(target).ok()
}

fn pc_at(entry: VirtualAddress, index: usize) -> VirtualAddress {
    VirtualAddress::new(entry.get().wrapping_add((index as u32).wrapping_mul(4)))
}

fn opcode(word: u32) -> u32 {
    word >> 26
}

fn rs(word: u32) -> u8 {
    ((word >> 21) & 0x1f) as u8
}

fn rt(word: u32) -> u8 {
    ((word >> 16) & 0x1f) as u8
}

fn sign_extended_immediate(word: u32) -> i32 {
    (word as i16) as i32
}

fn writes_register(word: u32, register: u8) -> bool {
    if register == 0 {
        return false;
    }
    match opcode(word) {
        OP_SPECIAL => ((word >> 11) & 0x1f) as u8 == register,
        OP_JAL => register == 31,
        8..=15 | 24..=27 | 32..=39 | 48..=55 => rt(word) == register,
        // Coprocessor-to-GPR transfers are uncommon in entry address setup;
        // treating every coprocessor opcode as a write is conservative.
        16..=19 => rt(word) == register,
        _ => false,
    }
}

/// Whether a ROM-to-RDRAM PI DMA claim is observed or inferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiDmaEvidence {
    /// A trace recorded the asynchronous completion notification.  This
    /// proves the copy geometry occurred, but not that its bytes are code.
    ObservedCompletion { ordinal: u64 },
    /// Static value recovery found call operands.  Reachability, successful
    /// completion, and handle/device interpretation remain to be proved.
    RecoveredCallOperands { call_pc: VirtualAddress },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimStrength {
    Proven,
    Candidate,
}

/// A validated ROM-to-RDRAM copy geometry.
///
/// `rom_offset` must already have been resolved through the PI device handle;
/// raw `osPiStartDma` device addresses are not necessarily ROM-file offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PiRomLoad {
    pub rom_offset: RomOffset,
    pub rdram_address: RdramAddress,
    pub byte_count: NonZeroU32,
    pub evidence: PiDmaEvidence,
}

impl PiRomLoad {
    pub fn strength(self) -> ClaimStrength {
        match self.evidence {
            PiDmaEvidence::ObservedCompletion { .. } => ClaimStrength::Proven,
            PiDmaEvidence::RecoveredCallOperands { .. } => ClaimStrength::Candidate,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiRomLoadRejectReason {
    ZeroLength,
    RomRangeOverflow,
    RomRangeOutOfBounds { end_exclusive: u64, rom_len: u64 },
    RdramRangeOverflow,
    RdramRangeOutOfBounds { end_exclusive: u64, rdram_len: u64 },
}

impl fmt::Display for PiRomLoadRejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLength => write!(f, "PI ROM load has zero byte count"),
            Self::RomRangeOverflow => write!(f, "PI ROM load overflows the ROM address type"),
            Self::RomRangeOutOfBounds {
                end_exclusive,
                rom_len,
            } => write!(
                f,
                "PI ROM load ends at {end_exclusive:#x}, beyond ROM length {rom_len:#x}"
            ),
            Self::RdramRangeOverflow => {
                write!(f, "PI ROM load overflows the RDRAM address type")
            }
            Self::RdramRangeOutOfBounds {
                end_exclusive,
                rdram_len,
            } => write!(
                f,
                "PI ROM load ends at RDRAM {end_exclusive:#x}, beyond length {rdram_len:#x}"
            ),
        }
    }
}

impl std::error::Error for PiRomLoadRejectReason {}

/// Validate a resolved ROM-to-RDRAM PI DMA claim without assigning code/data
/// semantics to the copied bytes.
pub fn validate_pi_rom_load(
    rom_offset: RomOffset,
    rdram_address: RdramAddress,
    byte_count: u32,
    rom_len: u64,
    rdram_len: u64,
    evidence: PiDmaEvidence,
) -> Result<PiRomLoad, PiRomLoadRejectReason> {
    let byte_count = NonZeroU32::new(byte_count).ok_or(PiRomLoadRejectReason::ZeroLength)?;
    let rom_end = rom_offset.get() as u64 + byte_count.get() as u64;
    if rom_end > u32::MAX as u64 + 1 {
        return Err(PiRomLoadRejectReason::RomRangeOverflow);
    }
    if rom_end > rom_len {
        return Err(PiRomLoadRejectReason::RomRangeOutOfBounds {
            end_exclusive: rom_end,
            rom_len,
        });
    }
    let rdram_end = rdram_address.get() as u64 + byte_count.get() as u64;
    if rdram_end > u32::MAX as u64 + 1 {
        return Err(PiRomLoadRejectReason::RdramRangeOverflow);
    }
    if rdram_end > rdram_len {
        return Err(PiRomLoadRejectReason::RdramRangeOutOfBounds {
            end_exclusive: rdram_end,
            rdram_len,
        });
    }

    Ok(PiRomLoad {
        rom_offset,
        rdram_address,
        byte_count,
        evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i(op: u32, rs: u8, rt: u8, immediate: i16) -> u32 {
        (op << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | immediate as u16 as u32
    }

    fn jal(target: u32) -> u32 {
        (OP_JAL << 26) | ((target >> 2) & 0x03ff_ffff)
    }

    fn jr(register: u8) -> u32 {
        ((register as u32) << 21) | 8
    }

    fn canonical_stub(start: u32, end: u32, stride: i16) -> Vec<u32> {
        let start_hi = ((start.wrapping_add(0x8000)) >> 16) as i16;
        let end_hi = ((end.wrapping_add(0x8000)) >> 16) as i16;
        vec![
            i(OP_LUI, 0, 8, start_hi),
            i(OP_LUI, 0, 9, end_hi),
            i(OP_ADDIU, 8, 8, start as i16),
            i(OP_ADDIU, 9, 9, end as i16),
            i(OP_SW, 8, 0, 0),
            i(OP_ADDIU, 8, 8, stride),
            i(OP_BNE, 8, 9, -3),
            0,
            jal(0x8000_1000),
            0,
        ]
    }

    fn countdown_stub(start: u32, byte_count: u32, target: u32) -> Vec<u32> {
        let start_hi = ((start.wrapping_add(0x8000)) >> 16) as i16;
        let count_hi = ((byte_count.wrapping_add(0x8000)) >> 16) as i16;
        let target_hi = ((target.wrapping_add(0x8000)) >> 16) as i16;
        vec![
            i(OP_LUI, 0, 8, start_hi),
            i(OP_LUI, 0, 9, count_hi),
            i(OP_ADDIU, 8, 8, start as i16),
            i(OP_ADDIU, 9, 9, byte_count as i16),
            i(OP_SW, 8, 0, 0),
            i(OP_SW, 8, 0, 4),
            i(OP_ADDIU, 8, 8, 8),
            i(OP_ADDIU, 9, 9, -8),
            i(OP_BNE, 9, 0, -5),
            0,
            i(OP_LUI, 0, 10, target_hi),
            i(OP_ADDIU, 10, 10, target as i16),
            jr(10),
            i(OP_ADDIU, 29, 29, -0x20),
        ]
    }

    fn countdown_stub_with_constructed_stack(
        start: u32,
        byte_count: u32,
        target: u32,
        stack_pointer: u32,
    ) -> Vec<u32> {
        let mut words = countdown_stub(start, byte_count, target);
        let stack_hi = ((stack_pointer.wrapping_add(0x8000)) >> 16) as i16;
        words.insert(12, i(OP_LUI, 0, 29, stack_hi));
        words[14] = i(OP_ADDIU, 29, 29, stack_pointer as i16);
        words
    }

    #[test]
    fn proves_canonical_word_at_a_time_zero_fill() {
        let words = canonical_stub(0x8000_8000, 0x8000_8100, 4);
        let observation = recognize_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap();

        assert_eq!(observation.zero_fill.start.get(), 0x8000_8000);
        assert_eq!(observation.zero_fill.end_exclusive.get(), 0x8000_8100);
        assert_eq!(observation.zero_fill.stride.get(), 4);
        assert_eq!(observation.zero_fill.induction_register, 8);
        assert_eq!(observation.zero_fill.end_register, 9);
        assert_eq!(
            observation.post_clear_direct_call,
            Some(PostClearDirectCall {
                call_pc: VirtualAddress::new(0x8000_0420),
                target: VirtualAddress::new(0x8000_1000),
                candidate_role: CandidateCallRole::MainEntry,
            })
        );
    }

    #[test]
    fn proves_countdown_clear_and_constructed_main_entry_jump() {
        let words = countdown_stub(0x8001_8000, 0x100, 0x8000_1230);
        let observation =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap();

        assert_eq!(observation.zero_fill.start.get(), 0x8001_8000);
        assert_eq!(observation.zero_fill.end_exclusive.get(), 0x8001_8100);
        assert_eq!(observation.zero_fill.byte_count.get(), 0x100);
        assert_eq!(observation.zero_fill.stride.get(), 8);
        assert_eq!(observation.zero_fill.base_register, 8);
        assert_eq!(observation.zero_fill.remaining_count_register, 9);
        assert_eq!(
            observation.post_clear_constructed_jump,
            Some(PostClearConstructedJump {
                construction_pc: VirtualAddress::new(0x8000_0428),
                jump_pc: VirtualAddress::new(0x8000_0430),
                target: VirtualAddress::new(0x8000_1230),
                target_register: 10,
                stack_setup: PostClearStackSetup::RelativeAdjustment { amount: -0x20 },
                candidate_role: CandidateTransferRole::MainEntry,
            })
        );
        assert!(matches!(
            recognize_entry_stub_any(&words, VirtualAddress::new(0x8000_0400)),
            Ok(RecognizedEntryStub::Countdown(_))
        ));
    }

    #[test]
    fn proves_five_word_handoff_with_constructed_stack_pointer() {
        let words =
            countdown_stub_with_constructed_stack(0x8023_4000, 0x180, 0x8012_3450, 0x8041_fff0);
        let observation =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap();

        assert_eq!(
            observation.post_clear_constructed_jump,
            Some(PostClearConstructedJump {
                construction_pc: VirtualAddress::new(0x8000_0428),
                jump_pc: VirtualAddress::new(0x8000_0434),
                target: VirtualAddress::new(0x8012_3450),
                target_register: 10,
                stack_setup: PostClearStackSetup::Constructed {
                    construction_pc: VirtualAddress::new(0x8000_0430),
                    address: VirtualAddress::new(0x8041_fff0),
                },
                candidate_role: CandidateTransferRole::MainEntry,
            })
        );
    }

    #[test]
    fn five_word_handoff_rejects_misaligned_constructed_stack_pointer() {
        let words =
            countdown_stub_with_constructed_stack(0x8023_4000, 0x180, 0x8012_3450, 0x8041_fff4);
        let observation =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap();

        assert_eq!(observation.post_clear_constructed_jump, None);
    }

    #[test]
    fn five_word_handoff_requires_immediate_adjacency() {
        let mut words =
            countdown_stub_with_constructed_stack(0x8023_4000, 0x180, 0x8012_3450, 0x8041_fff0);
        words.insert(10, 0);
        let observation =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap();

        assert_eq!(observation.post_clear_constructed_jump, None);
    }

    #[test]
    fn countdown_accepts_reversed_zero_comparison_operands() {
        let mut words = countdown_stub(0x8001_8000, 0x100, 0x8000_1230);
        words[8] = i(OP_BNE, 0, 9, -5);
        let observation =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap();
        assert_eq!(observation.zero_fill.remaining_count_register, 9);
    }

    #[test]
    fn countdown_accepts_addi_after_proving_signed_ranges() {
        let mut words = countdown_stub(0x8001_8000, 0x100, 0x8000_1230);
        words[6] = i(OP_ADDI, 8, 8, 8);
        words[7] = i(OP_ADDI, 9, 9, -8);
        let observation =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap();
        assert_eq!(
            observation.zero_fill.base_update_kind,
            LoopAddKind::TrappingAddi
        );
        assert_eq!(
            observation.zero_fill.count_update_kind,
            LoopAddKind::TrappingAddi
        );
    }

    #[test]
    fn countdown_rejects_addi_base_range_that_crosses_signed_max() {
        let mut words = countdown_stub(0x7fff_fff8, 0x10, 0x8000_1230);
        words[6] = i(OP_ADDI, 8, 8, 8);
        let error =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap_err();
        assert!(matches!(
            error,
            EntryStubRejectReason::MalformedLoop {
                reason: LoopRejectReason::SignedOverflowPossible,
                ..
            }
        ));
    }

    #[test]
    fn countdown_rejects_addi_count_that_begins_negative() {
        let mut words = countdown_stub(0, 0x8000_0000, 0x8000_1230);
        words[7] = i(OP_ADDI, 9, 9, -8);
        let error =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap_err();
        assert!(matches!(
            error,
            EntryStubRejectReason::MalformedLoop {
                reason: LoopRejectReason::SignedOverflowPossible,
                ..
            }
        ));
    }

    #[test]
    fn countdown_requires_paired_base_and_count_strides() {
        let mut words = countdown_stub(0x8001_8000, 0x100, 0x8000_1230);
        words[7] = i(OP_ADDIU, 9, 9, -4);
        let error =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap_err();
        assert!(matches!(
            error,
            EntryStubRejectReason::MalformedLoop {
                reason: LoopRejectReason::CountUpdateDoesNotMatchBaseStride,
                ..
            }
        ));
    }

    #[test]
    fn countdown_requires_complete_store_coverage() {
        let mut words = countdown_stub(0x8001_8000, 0x100, 0x8000_1230);
        words.remove(5);
        words[7] = i(OP_BNE, 9, 0, -4);
        let error =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap_err();
        assert!(matches!(
            error,
            EntryStubRejectReason::MalformedLoop {
                reason: LoopRejectReason::StoreCoverageHasGap,
                ..
            }
        ));
    }

    #[test]
    fn countdown_requires_count_divisible_by_stride() {
        let words = countdown_stub(0x8001_8000, 0x104, 0x8000_1230);
        let error =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap_err();
        assert!(matches!(
            error,
            EntryStubRejectReason::MalformedLoop {
                reason: LoopRejectReason::RangeNotDivisibleByStride,
                ..
            }
        ));
    }

    #[test]
    fn countdown_requires_actual_nop_loop_delay_slot() {
        let mut words = countdown_stub(0x8001_8000, 0x100, 0x8000_1230);
        words[9] = i(OP_ADDIU, 2, 2, 1);
        let error =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap_err();
        assert!(matches!(
            error,
            EntryStubRejectReason::MalformedLoop {
                reason: LoopRejectReason::LoopContainsNonStoreOperation,
                ..
            }
        ));
    }

    #[test]
    fn constructed_jump_requires_stack_setup_but_clear_still_proves() {
        let mut words = countdown_stub(0x8001_8000, 0x100, 0x8000_1230);
        words[13] = 0;
        let observation =
            recognize_countdown_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap();
        assert_eq!(observation.post_clear_constructed_jump, None);
    }

    #[test]
    fn any_form_preserves_end_pointer_recognition() {
        let words = canonical_stub(0x8000_8000, 0x8000_8100, 4);
        assert!(matches!(
            recognize_entry_stub_any(&words, VirtualAddress::new(0x8000_0400)),
            Ok(RecognizedEntryStub::EndPointer(_))
        ));
    }

    #[test]
    fn accepts_complete_two_word_stride_and_reversed_bne_operands() {
        let mut words = canonical_stub(0x8012_0000, 0x8012_0100, 8);
        words.insert(5, i(OP_SW, 8, 0, 4));
        words[7] = i(OP_BNE, 9, 8, -4);

        let observation = recognize_entry_stub(&words, VirtualAddress::new(0x8010_0000)).unwrap();
        assert_eq!(observation.zero_fill.stride.get(), 8);
        assert_eq!(observation.zero_fill.start.get(), 0x8012_0000);
    }

    #[test]
    fn reconstructs_addiu_low_half_with_relocation_carry() {
        let words = canonical_stub(0x8001_fff0, 0x8002_0030, 4);
        let observation = recognize_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap();
        assert_eq!(observation.zero_fill.start.get(), 0x8001_fff0);
        assert_eq!(observation.zero_fill.end_exclusive.get(), 0x8002_0030);
    }

    #[test]
    fn reconstructs_ori_low_half_without_sign_extension() {
        let mut words = canonical_stub(0x8000_4000, 0x8000_4100, 4);
        words[0] = i(OP_LUI, 0, 8, 0x8000u16 as i16);
        words[1] = i(OP_LUI, 0, 9, 0x8000u16 as i16);
        words[2] = i(OP_ORI, 8, 8, 0x4000);
        words[3] = i(OP_ORI, 9, 9, 0x4100);
        let observation = recognize_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap();
        assert_eq!(observation.zero_fill.start.get(), 0x8000_4000);
        assert_eq!(observation.zero_fill.end_exclusive.get(), 0x8000_4100);
    }

    #[test]
    fn rejects_multiple_valid_clear_loops_as_ambiguous() {
        let mut words = canonical_stub(0x8000_8000, 0x8000_8100, 4);
        words.extend(canonical_stub(0x8000_9000, 0x8000_9100, 4));
        assert_eq!(
            recognize_entry_stub(&words, VirtualAddress::new(0x8000_0400)),
            Err(EntryStubRejectReason::AmbiguousZeroFillLoops { count: 2 })
        );
    }

    #[test]
    fn rejects_store_coverage_gap_instead_of_guessing_clear_semantics() {
        let words = canonical_stub(0x8000_8000, 0x8000_8100, 8);
        let error = recognize_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap_err();
        assert!(matches!(
            error,
            EntryStubRejectReason::MalformedLoop {
                reason: LoopRejectReason::StoreCoverageHasGap,
                ..
            }
        ));
    }

    #[test]
    fn rejects_range_that_cannot_reach_end_by_exact_stride() {
        let mut words = canonical_stub(0x8000_8000, 0x8000_8104, 8);
        words.insert(5, i(OP_SW, 8, 0, 4));
        words[7] = i(OP_BNE, 8, 9, -4);
        let error = recognize_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap_err();
        assert!(matches!(
            error,
            EntryStubRejectReason::MalformedLoop {
                reason: LoopRejectReason::RangeNotDivisibleByStride,
                ..
            }
        ));
    }

    #[test]
    fn rejects_induction_update_before_a_store() {
        let mut words = canonical_stub(0x8000_8000, 0x8000_8100, 8);
        words.insert(6, i(OP_SW, 8, 0, 4));
        words[7] = i(OP_BNE, 8, 9, -4);
        let error = recognize_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap_err();
        assert!(matches!(
            error,
            EntryStubRejectReason::MalformedLoop {
                reason: LoopRejectReason::LoopContainsNonStoreOperation,
                ..
            }
        ));
    }

    #[test]
    fn rejects_truncated_or_non_nop_branch_delay_slot() {
        let mut words = canonical_stub(0x8000_8000, 0x8000_8100, 4);
        words[7] = i(OP_ORI, 0, 2, 1);
        let error = recognize_entry_stub(&words, VirtualAddress::new(0x8000_0400)).unwrap_err();
        assert!(matches!(
            error,
            EntryStubRejectReason::MalformedLoop {
                reason: LoopRejectReason::LoopContainsNonStoreOperation,
                ..
            }
        ));

        let truncated = &canonical_stub(0x8000_8000, 0x8000_8100, 4)[..7];
        assert!(matches!(
            recognize_entry_stub(truncated, VirtualAddress::new(0x8000_0400)),
            Err(EntryStubRejectReason::MalformedLoop {
                reason: LoopRejectReason::LoopContainsNonStoreOperation,
                ..
            })
        ));
    }

    #[test]
    fn rejects_unaligned_hardware_entry_root() {
        let error = recognize_entry_stub(
            &canonical_stub(0x8000_8000, 0x8000_8100, 4),
            VirtualAddress::new(0x8000_0402),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            EntryStubRejectReason::EntryAddressUnaligned { .. }
        ));
    }

    #[test]
    fn reports_no_loop_for_unrelated_words() {
        assert_eq!(
            recognize_entry_stub(&[0, 0, 0], VirtualAddress::new(0x8000_0400)),
            Err(EntryStubRejectReason::NoBackwardBne)
        );
    }

    #[test]
    fn observed_completion_proves_copy_geometry_but_not_code() {
        let load = validate_pi_rom_load(
            RomOffset::new(0x1200),
            RdramAddress::new(0x2000),
            0x400,
            0x20_0000,
            0x80_0000,
            PiDmaEvidence::ObservedCompletion { ordinal: 7 },
        )
        .unwrap();
        assert_eq!(load.strength(), ClaimStrength::Proven);
        assert_eq!(load.byte_count.get(), 0x400);
    }

    #[test]
    fn recovered_operands_remain_candidate() {
        let load = validate_pi_rom_load(
            RomOffset::new(0x1200),
            RdramAddress::new(0x2000),
            0x400,
            0x20_0000,
            0x80_0000,
            PiDmaEvidence::RecoveredCallOperands {
                call_pc: VirtualAddress::new(0x8000_1000),
            },
        )
        .unwrap();
        assert_eq!(load.strength(), ClaimStrength::Candidate);
    }

    #[test]
    fn rejects_zero_length_and_each_out_of_bounds_domain() {
        assert_eq!(
            validate_pi_rom_load(
                RomOffset::new(0),
                RdramAddress::new(0),
                0,
                0x100,
                0x100,
                PiDmaEvidence::ObservedCompletion { ordinal: 0 },
            ),
            Err(PiRomLoadRejectReason::ZeroLength)
        );
        assert!(matches!(
            validate_pi_rom_load(
                RomOffset::new(0xf0),
                RdramAddress::new(0),
                0x20,
                0x100,
                0x100,
                PiDmaEvidence::ObservedCompletion { ordinal: 0 },
            ),
            Err(PiRomLoadRejectReason::RomRangeOutOfBounds { .. })
        ));
        assert!(matches!(
            validate_pi_rom_load(
                RomOffset::new(0),
                RdramAddress::new(0xf0),
                0x20,
                0x100,
                0x100,
                PiDmaEvidence::ObservedCompletion { ordinal: 0 },
            ),
            Err(PiRomLoadRejectReason::RdramRangeOutOfBounds { .. })
        ));
        assert_eq!(
            validate_pi_rom_load(
                RomOffset::new(0xffff_fff0),
                RdramAddress::new(0),
                0x20,
                u64::MAX,
                0x100,
                PiDmaEvidence::ObservedCompletion { ordinal: 0 },
            ),
            Err(PiRomLoadRejectReason::RomRangeOverflow)
        );
    }
}
