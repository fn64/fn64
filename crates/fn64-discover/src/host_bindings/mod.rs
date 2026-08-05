//! Mechanical host-binding discovery for public resident libultra routines.
//!
//! Addresses are outputs, never signatures. The recognizers below describe
//! public ABI behavior in register/field terms: `osCreateMesgQueue` initializes
//! the documented six-word queue, `osCreateThread` initializes the public
//! `OSThread` linkage, identity, state, context, and o32 stack-supplied
//! priority fields, `osEPiStartDma` validates the manager and writes the
//! request type/handle into `OSIoMesg`, `osSendMesg` inserts at `(first +
//! validCount) % msgCount`, and the overlay helper calls the DMA routine in a
//! retry loop before a blocking receive on its stack queue.
//! The RSP task recognizers likewise follow the public task-load, start, yield,
//! and yielded-query register/field behavior. Timer discovery follows the
//! public o32 `osSetTimer` arguments, `OSTimer` fields, and list insertion
//! behavior. Every role must have one unique structural match or discovery
//! fails loudly.

use crate::cfg::{classify_control, BlockTerminator, Cfg, ControlOp, WordClass};
use crate::facts::FactDb;
use crate::resolve::written_gpr;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostBindingSymbol {
    OsCreateMesgQueue,
    OsCreateThread,
    OsDriveRomInit,
    OsEPiStartDma,
    OsGetThreadPri,
    OsRecvMesg,
    OsSendMesg,
    OsSetEventMesg,
    OsSiDeviceBusy,
    OsSetThreadPri,
    OsSetTimer,
    OsSpTaskLoad,
    OsSpTaskStartGo,
    OsSpTaskYield,
    OsSpTaskYielded,
    OsStartThread,
}

/// Exact installed host target denominator shared by the WM production build
/// and its executable-source receipt validator.
pub const WM_BLOCK_RUNTIME_HOST_SYMBOLS: [HostBindingSymbol; 15] = [
    HostBindingSymbol::OsCreateMesgQueue,
    HostBindingSymbol::OsCreateThread,
    HostBindingSymbol::OsEPiStartDma,
    HostBindingSymbol::OsGetThreadPri,
    HostBindingSymbol::OsRecvMesg,
    HostBindingSymbol::OsSendMesg,
    HostBindingSymbol::OsSetEventMesg,
    HostBindingSymbol::OsSiDeviceBusy,
    HostBindingSymbol::OsSetThreadPri,
    HostBindingSymbol::OsSetTimer,
    HostBindingSymbol::OsSpTaskLoad,
    HostBindingSymbol::OsSpTaskStartGo,
    HostBindingSymbol::OsSpTaskYield,
    HostBindingSymbol::OsSpTaskYielded,
    HostBindingSymbol::OsStartThread,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostCurrentStatusEffect {
    /// The adapter crosses the legacy C context boundary, but `call_c`
    /// compares Status.BEV before and after every invocation and traps before
    /// copy-back on any transition.
    CBridgeRuntimeEnforcedPreservesBev,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostSpawnedStatusEffect {
    None,
    /// `osCreateThread` publishes the caller's Status to the child context and
    /// clears only FR. Therefore it preserves, rather than clears, caller BEV.
    InheritsCallerClearingFr,
}

impl HostBindingSymbol {
    pub fn current_status_effect(self) -> HostCurrentStatusEffect {
        match self {
            Self::OsCreateMesgQueue
            | Self::OsCreateThread
            | Self::OsDriveRomInit
            | Self::OsEPiStartDma
            | Self::OsGetThreadPri
            | Self::OsRecvMesg
            | Self::OsSendMesg
            | Self::OsSetEventMesg
            | Self::OsSiDeviceBusy
            | Self::OsSetThreadPri
            | Self::OsSetTimer
            | Self::OsSpTaskLoad
            | Self::OsSpTaskStartGo
            | Self::OsSpTaskYield
            | Self::OsSpTaskYielded
            | Self::OsStartThread => HostCurrentStatusEffect::CBridgeRuntimeEnforcedPreservesBev,
        }
    }

    pub fn spawned_status_effect(self) -> HostSpawnedStatusEffect {
        match self {
            Self::OsCreateThread => HostSpawnedStatusEffect::InheritsCallerClearingFr,
            Self::OsCreateMesgQueue
            | Self::OsDriveRomInit
            | Self::OsEPiStartDma
            | Self::OsGetThreadPri
            | Self::OsRecvMesg
            | Self::OsSendMesg
            | Self::OsSetEventMesg
            | Self::OsSiDeviceBusy
            | Self::OsSetThreadPri
            | Self::OsSetTimer
            | Self::OsSpTaskLoad
            | Self::OsSpTaskStartGo
            | Self::OsSpTaskYield
            | Self::OsSpTaskYielded
            | Self::OsStartThread => HostSpawnedStatusEffect::None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostBinding {
    pub symbol: HostBindingSymbol,
    pub vram: u32,
}

/// Candidate-strength shape classification for the public `osPiStartDma`
/// wrapper.
///
/// This is deliberately separate from [`HostBinding`]: authenticating the
/// wrapper's static ABI behavior does not install a runtime binding or prove
/// that any particular DMA completes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsPiStartDmaShapeCandidate {
    pub bank: String,
    pub vram: u32,
    pub os_epi_start_dma_shape_vram: u32,
    /// Static wrapper shape does not identify the cart handle's device base,
    /// so `devAddr` is not yet an authoritative physical-ROM coordinate.
    pub device_base: OsPiDeviceBasePrerequisite,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OsPiDeviceBasePrerequisite {
    UnresolvedCartHandleAndDeviceBase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsPiCandidateLimitKind {
    Roots,
    DirectCalls,
    Blocks,
    Work,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OsPiStartDmaCandidateOpenReason {
    BankMismatch,
    UnalignedImage,
    AddressOverflow,
    NonUniqueOsEPiStartDmaShape {
        candidates: Vec<u32>,
    },
    NonUniqueOsPiStartDmaShape {
        candidates: Vec<u32>,
    },
    LimitHit {
        kind: OsPiCandidateLimitKind,
        observed: usize,
        cap: usize,
        samples: Vec<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OsPiStartDmaCandidateClassification {
    Candidate(OsPiStartDmaShapeCandidate),
    Open(OsPiStartDmaCandidateOpenReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestThreadGlobals {
    pub running_thread_vram: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostBindingDiscoveryError {
    UnalignedImage,
    AddressOverflow,
    NonUniqueSemanticMatch {
        symbol: HostBindingSymbol,
        candidates: Vec<u32>,
    },
    NonUniqueOverlayCallChain {
        candidates: Vec<(u32, u32, u32)>,
    },
    InconsistentRunningThreadGlobals {
        get_thread_pri: u32,
        set_thread_pri: u32,
    },
    ConflictingBindingAddress {
        vram: u32,
        first: HostBindingSymbol,
        second: HostBindingSymbol,
    },
}

fn op(word: u32) -> u32 {
    word >> 26
}

fn rs(word: u32) -> u32 {
    word >> 21 & 31
}

fn rt(word: u32) -> u32 {
    word >> 16 & 31
}

fn rd(word: u32) -> u32 {
    word >> 11 & 31
}

fn imm(word: u32) -> i16 {
    word as u16 as i16
}

fn is_lui(word: u32, target: u32) -> bool {
    op(word) == 0x0f && rs(word) == 0 && rt(word) == target
}

fn is_addiu(word: u32, target: u32, source: u32, immediate: i16) -> bool {
    op(word) == 0x09 && rt(word) == target && rs(word) == source && imm(word) == immediate
}

fn is_lw(word: u32, target: u32, base: u32) -> bool {
    op(word) == 0x23 && rt(word) == target && rs(word) == base
}

fn is_lw_at(word: u32, target: u32, base: u32, offset: i16) -> bool {
    is_lw(word, target, base) && imm(word) == offset
}

fn is_sw(word: u32, source: u32, base: u32, offset: i16) -> bool {
    op(word) == 0x2b && rt(word) == source && rs(word) == base && imm(word) == offset
}

fn absolute_from_lui_offset(lui: u32, offset: i16) -> u32 {
    ((lui as u16 as u32) << 16).wrapping_add_signed(i32::from(offset))
}

/// Discover the guest libultra scheduler global containing the currently
/// running `OSThread` pointer.
///
/// The public thread-priority API says a null thread argument means the current
/// thread. The independently unique `osGetThreadPri` and `osSetThreadPri`
/// implementations both realize that rule by loading one global before
/// accessing the public `OSThread.priority` field. Requiring both consumers to
/// derive the same address avoids treating a data address or one incidental
/// load as a signature.
pub fn discover_guest_thread_globals(
    resident_words: &[u32],
    resident_vram: u32,
) -> Result<GuestThreadGlobals, HostBindingDiscoveryError> {
    if !resident_vram.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    let get = unique_match(
        resident_words,
        resident_vram,
        6,
        HostBindingSymbol::OsGetThreadPri,
        is_get_thread_pri,
    )?;
    let set = unique_match(
        resident_words,
        resident_vram,
        20,
        HostBindingSymbol::OsSetThreadPri,
        is_set_thread_pri,
    )?;
    let get_index = ((get - resident_vram) / 4) as usize;
    let set_index = ((set - resident_vram) / 4) as usize;
    let get_thread_pri = absolute_from_lui_offset(
        resident_words[get_index + 2],
        imm(resident_words[get_index + 3]),
    );
    let set_thread_pri = absolute_from_lui_offset(
        resident_words[set_index + 10],
        imm(resident_words[set_index + 11]),
    );
    if get_thread_pri != set_thread_pri {
        return Err(
            HostBindingDiscoveryError::InconsistentRunningThreadGlobals {
                get_thread_pri,
                set_thread_pri,
            },
        );
    }
    Ok(GuestThreadGlobals {
        running_thread_vram: get_thread_pri,
    })
}

fn is_sh(word: u32, source: u32, base: u32, offset: i16) -> bool {
    op(word) == 0x29 && rt(word) == source && rs(word) == base && imm(word) == offset
}

fn is_move_addu(word: u32, target: u32, source: u32) -> bool {
    op(word) == 0
        && word & 0x3f == 0x21
        && rd(word) == target
        && ((rs(word) == source && rt(word) == 0) || (rt(word) == source && rs(word) == 0))
}

fn is_zero_addu(word: u32, target: u32) -> bool {
    op(word) == 0 && word & 0x3f == 0x21 && rd(word) == target && rs(word) == 0 && rt(word) == 0
}

fn is_jr_ra(word: u32) -> bool {
    op(word) == 0 && word & 0x3f == 8 && rs(word) == 31
}

fn is_bne(word: u32, left: u32, right: u32) -> bool {
    op(word) == 5 && rs(word) == left && rt(word) == right
}

fn is_beq(word: u32, left: u32, right: u32) -> bool {
    op(word) == 4 && rs(word) == left && rt(word) == right
}

fn is_andi(word: u32, target: u32, source: u32, immediate: u16) -> bool {
    op(word) == 0x0c && rt(word) == target && rs(word) == source && word as u16 == immediate
}

fn jal_target(word: u32, pc: u32) -> Option<u32> {
    (op(word) == 3).then_some(((pc.wrapping_add(4)) & 0xf000_0000) | ((word & 0x03ff_ffff) << 2))
}

fn jal_field(word: u32) -> Option<u32> {
    (op(word) == 3).then_some(word & 0x03ff_ffff)
}

fn is_create_mesg_queue(words: &[u32]) -> bool {
    words.len() >= 9
        && is_lui(words[0], 2)
        && is_addiu(words[1], 2, 2, imm(words[1]))
        && is_sw(words[2], 2, 4, 0)
        && is_sw(words[3], 2, 4, 4)
        && is_sw(words[4], 0, 4, 8)
        && is_sw(words[5], 0, 4, 12)
        && is_sw(words[6], 6, 4, 16)
        && is_jr_ra(words[7])
        && is_sw(words[8], 5, 4, 20)
}

fn is_epi_start_dma(words: &[u32]) -> bool {
    words.len() >= 15
        && is_lui(words[0], 2)
        && is_lw(words[1], 2, 2)
        && is_addiu(words[2], 29, 29, imm(words[2]))
        && imm(words[2]) < 0
        && is_sw(words[3], 16, 29, imm(words[3]))
        && is_move_addu(words[4], 16, 5)
        && is_bne(words[5], 2, 0)
        && is_sw(words[6], 31, 29, imm(words[6]))
        && op(words[7]) == 2
        && is_addiu(words[8], 2, 0, -1)
        && is_bne(words[9], 6, 0)
        && is_sw(words[10], 4, 16, 20)
        && op(words[11]) == 2
        && is_addiu(words[12], 2, 0, 15)
        && is_addiu(words[13], 2, 0, 16)
        && is_sh(words[14], 2, 16, 0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PiWrapperValue {
    Unknown,
    Zero,
    Message,
    Priority,
    Direction,
    DeviceAddress,
    DramAddress,
    ByteCount,
    ReturnQueue,
    Stack(i32),
}

struct PiMessageWrites {
    priority: PiWrapperValue,
    status: PiWrapperValue,
    queue: PiWrapperValue,
    dram: PiWrapperValue,
    device: PiWrapperValue,
    size: PiWrapperValue,
}

impl Default for PiMessageWrites {
    fn default() -> Self {
        Self {
            priority: PiWrapperValue::Unknown,
            status: PiWrapperValue::Unknown,
            queue: PiWrapperValue::Unknown,
            dram: PiWrapperValue::Unknown,
            device: PiWrapperValue::Unknown,
            size: PiWrapperValue::Unknown,
        }
    }
}

fn proven_code_interval(cfg: &Cfg, executable_ranges: &[(u32, u32)], start: u32, end: u32) -> bool {
    start < end
        && executable_ranges
            .iter()
            .copied()
            .any(|(range_start, range_end)| start >= range_start && end <= range_end)
        && (start..end)
            .step_by(4)
            .all(|pc| cfg.word_class.get(&pc) == Some(&WordClass::ProvenCode))
}

fn authoritative_root(
    cfg: &Cfg,
    cfg_roots: &BTreeSet<u32>,
    proven_entries: &BTreeSet<u32>,
    root: u32,
) -> bool {
    cfg_roots.contains(&root)
        && proven_entries.contains(&root)
        && cfg.word_class.get(&root) == Some(&WordClass::ProvenCode)
}

fn image_words<'a>(words: &'a [u32], va_start: u32, start: u32, count: usize) -> Option<&'a [u32]> {
    let byte_offset = start.checked_sub(va_start)?;
    if !byte_offset.is_multiple_of(4) {
        return None;
    }
    let first = usize::try_from(byte_offset / 4).ok()?;
    words.get(first..first.checked_add(count)?)
}

fn is_store_opcode(opcode: u32) -> bool {
    matches!(opcode, 0x28..=0x2e | 0x38..=0x3f)
}

fn is_pi_wrapper_shape_candidate(words: &[u32]) -> bool {
    if words.len() < 2 {
        return false;
    }

    let mut registers = [PiWrapperValue::Unknown; 32];
    registers[0] = PiWrapperValue::Zero;
    registers[4] = PiWrapperValue::Message;
    registers[5] = PiWrapperValue::Priority;
    registers[6] = PiWrapperValue::Direction;
    registers[7] = PiWrapperValue::DeviceAddress;
    registers[29] = PiWrapperValue::Stack(0);
    let mut stack = BTreeMap::<i32, PiWrapperValue>::new();
    let mut message = PiMessageWrites::default();

    for (index, &word) in words.iter().enumerate() {
        let opcode = op(word);
        if opcode == 3 {
            if index + 2 != words.len() || !matches!(classify_control(word), ControlOp::Jal { .. })
            {
                return false;
            }
            continue;
        }
        if !matches!(classify_control(word), ControlOp::Plain) {
            return false;
        }

        match opcode {
            0 => {
                let destination = rd(word) as usize;
                let function = word & 0x3f;
                registers[destination] = if matches!(function, 0x21 | 0x25) {
                    match (registers[rs(word) as usize], registers[rt(word) as usize]) {
                        (value, PiWrapperValue::Zero) | (PiWrapperValue::Zero, value) => value,
                        _ => PiWrapperValue::Unknown,
                    }
                } else {
                    PiWrapperValue::Unknown
                };
            }
            0x09 => {
                let source = registers[rs(word) as usize];
                registers[rt(word) as usize] = match source {
                    PiWrapperValue::Stack(offset) => match offset.checked_add(i32::from(imm(word)))
                    {
                        Some(offset) => PiWrapperValue::Stack(offset),
                        None => return false,
                    },
                    value if imm(word) == 0 => value,
                    _ => PiWrapperValue::Unknown,
                };
            }
            0x0f => registers[rt(word) as usize] = PiWrapperValue::Unknown,
            0x23 => {
                registers[rt(word) as usize] = match registers[rs(word) as usize] {
                    PiWrapperValue::Stack(base) => {
                        let Some(offset) = base.checked_add(i32::from(imm(word))) else {
                            return false;
                        };
                        stack.get(&offset).copied().unwrap_or(match offset {
                            0x10 => PiWrapperValue::DramAddress,
                            0x14 => PiWrapperValue::ByteCount,
                            0x18 => PiWrapperValue::ReturnQueue,
                            _ => PiWrapperValue::Unknown,
                        })
                    }
                    _ => PiWrapperValue::Unknown,
                };
            }
            0x2b => {
                let value = registers[rt(word) as usize];
                match registers[rs(word) as usize] {
                    PiWrapperValue::Stack(base) => {
                        let Some(offset) = base.checked_add(i32::from(imm(word))) else {
                            return false;
                        };
                        stack.insert(offset, value);
                    }
                    PiWrapperValue::Message => match (imm(word), value) {
                        (4, value) => message.queue = value,
                        (8, value) => message.dram = value,
                        (12, value) => message.device = value,
                        (16, value) => message.size = value,
                        _ => return false,
                    },
                    _ => return false,
                }
            }
            0x28 => match (
                registers[rs(word) as usize],
                imm(word),
                registers[rt(word) as usize],
            ) {
                (PiWrapperValue::Message, 2, value) => message.priority = value,
                (PiWrapperValue::Message, 3, value) => message.status = value,
                _ => return false,
            },
            _ if is_store_opcode(opcode) => return false,
            _ => {
                if let Some(register) = written_gpr(word) {
                    registers[register as usize] = PiWrapperValue::Unknown;
                }
            }
        }
        registers[0] = PiWrapperValue::Zero;
    }

    message.priority == PiWrapperValue::Priority
        && message.status == PiWrapperValue::Zero
        && message.queue == PiWrapperValue::ReturnQueue
        && message.dram == PiWrapperValue::DramAddress
        && message.device == PiWrapperValue::DeviceAddress
        && message.size == PiWrapperValue::ByteCount
        && registers[5] == PiWrapperValue::Message
        && registers[6] == PiWrapperValue::Direction
}

const DEFAULT_PI_SHAPE_ROOT_CAP: usize = 16_384;
const DEFAULT_PI_SHAPE_CALL_CAP: usize = 65_536;
const DEFAULT_PI_SHAPE_BLOCK_CAP: usize = 65_536;
const DEFAULT_PI_SHAPE_WORK_CAP: usize = 1_000_000;
const PI_SHAPE_LIMIT_SAMPLE_CAP: usize = 8;

#[derive(Clone, Copy)]
struct OsPiShapeLimits {
    roots: usize,
    calls: usize,
    blocks: usize,
    work: usize,
}

const DEFAULT_PI_SHAPE_LIMITS: OsPiShapeLimits = OsPiShapeLimits {
    roots: DEFAULT_PI_SHAPE_ROOT_CAP,
    calls: DEFAULT_PI_SHAPE_CALL_CAP,
    blocks: DEFAULT_PI_SHAPE_BLOCK_CAP,
    work: DEFAULT_PI_SHAPE_WORK_CAP,
};

fn limit_hit(
    kind: OsPiCandidateLimitKind,
    observed: usize,
    cap: usize,
    samples: impl IntoIterator<Item = u32>,
) -> OsPiStartDmaCandidateClassification {
    OsPiStartDmaCandidateClassification::Open(OsPiStartDmaCandidateOpenReason::LimitHit {
        kind,
        observed,
        cap,
        samples: samples
            .into_iter()
            .take(PI_SHAPE_LIMIT_SAMPLE_CAP)
            .collect(),
    })
}

fn charge_shape_work(
    work: &mut usize,
    amount: usize,
    cap: usize,
    sample: u32,
    samples: &mut Vec<u32>,
) -> Result<(), OsPiStartDmaCandidateClassification> {
    if samples.len() < PI_SHAPE_LIMIT_SAMPLE_CAP {
        samples.push(sample);
    }
    let observed = work.checked_add(amount).unwrap_or(usize::MAX);
    if observed > cap {
        return Err(limit_hit(
            OsPiCandidateLimitKind::Work,
            observed,
            cap,
            samples.iter().copied(),
        ));
    }
    *work = observed;
    Ok(())
}

/// Classify a candidate `osPiStartDma` wrapper shape using authoritative
/// bank-local reachability inputs without promoting the symbol itself.
///
/// Raw image occurrences are never enumerated. A unique proven-root
/// `osEPiStartDma` *shape* must exist, then the PI entry block must have an
/// exact CFG/byte direct call to it and relationally populate the public
/// `OSIoMesg` fields from the seven-argument o32 ABI. This remains candidate
/// evidence: EPI path proof and cart-handle/device-base authority are open.
pub fn classify_os_pi_start_dma_candidate(
    bank: &str,
    words: &[u32],
    va_start: u32,
    cfg: &Cfg,
    facts: &FactDb,
) -> OsPiStartDmaCandidateClassification {
    classify_os_pi_start_dma_candidate_with_limits(
        bank,
        words,
        va_start,
        cfg,
        facts,
        DEFAULT_PI_SHAPE_LIMITS,
    )
}

fn classify_os_pi_start_dma_candidate_with_limits(
    bank: &str,
    words: &[u32],
    va_start: u32,
    cfg: &Cfg,
    facts: &FactDb,
    limits: OsPiShapeLimits,
) -> OsPiStartDmaCandidateClassification {
    if bank != cfg.bank {
        return OsPiStartDmaCandidateClassification::Open(
            OsPiStartDmaCandidateOpenReason::BankMismatch,
        );
    }
    if !va_start.is_multiple_of(4) {
        return OsPiStartDmaCandidateClassification::Open(
            OsPiStartDmaCandidateOpenReason::UnalignedImage,
        );
    }
    let Some(image_len) = u32::try_from(words.len())
        .ok()
        .and_then(|len| len.checked_mul(4))
    else {
        return OsPiStartDmaCandidateClassification::Open(
            OsPiStartDmaCandidateOpenReason::AddressOverflow,
        );
    };
    if va_start.checked_add(image_len).is_none() {
        return OsPiStartDmaCandidateClassification::Open(
            OsPiStartDmaCandidateOpenReason::AddressOverflow,
        );
    }
    if cfg.proven_roots.len() > limits.roots {
        return limit_hit(
            OsPiCandidateLimitKind::Roots,
            cfg.proven_roots.len(),
            limits.roots,
            cfg.proven_roots.iter().copied(),
        );
    }
    if cfg.direct_calls.len() > limits.calls {
        return limit_hit(
            OsPiCandidateLimitKind::DirectCalls,
            cfg.direct_calls.len(),
            limits.calls,
            cfg.direct_calls.iter().map(|(source, _)| *source),
        );
    }
    if cfg.blocks.len() > limits.blocks {
        return limit_hit(
            OsPiCandidateLimitKind::Blocks,
            cfg.blocks.len(),
            limits.blocks,
            cfg.blocks.iter().map(|block| block.start_va),
        );
    }

    let mut work = 0usize;
    let mut work_samples = Vec::new();
    let proven_entries: BTreeSet<_> = facts.proven_function_entries(bank).into_iter().collect();
    let cfg_roots: BTreeSet<_> = cfg.proven_roots.iter().copied().collect();
    let executable_ranges = facts.proven_executable_ranges(bank);
    let mut epi_candidates = Vec::new();
    for &root in &cfg.proven_roots {
        if let Err(limit) = charge_shape_work(&mut work, 16, limits.work, root, &mut work_samples) {
            return limit;
        }
        let Some(end) = root.checked_add(15 * 4) else {
            continue;
        };
        if proven_entries.contains(&root)
            && authoritative_root(cfg, &cfg_roots, &proven_entries, root)
            && proven_code_interval(cfg, &executable_ranges, root, end)
            && image_words(words, va_start, root, 15).is_some_and(is_epi_start_dma)
        {
            epi_candidates.push(root);
        }
    }
    epi_candidates.sort_unstable();
    epi_candidates.dedup();
    let [epi] = epi_candidates.as_slice() else {
        return OsPiStartDmaCandidateClassification::Open(
            OsPiStartDmaCandidateOpenReason::NonUniqueOsEPiStartDmaShape {
                candidates: epi_candidates,
            },
        );
    };

    let mut exact_call_blocks = BTreeMap::<(u32, u32), Vec<u32>>::new();
    for block in &cfg.blocks {
        if let Err(limit) =
            charge_shape_work(&mut work, 1, limits.work, block.start_va, &mut work_samples)
        {
            return limit;
        }
        let BlockTerminator::Call { target, next } = &block.terminator else {
            continue;
        };
        let Some(call_pc) = block.end_va.checked_sub(8) else {
            continue;
        };
        let Some(expected_next) = call_pc.checked_add(8) else {
            continue;
        };
        if *next != expected_next || block.end_va != expected_next {
            continue;
        }
        exact_call_blocks
            .entry((call_pc, *target))
            .or_default()
            .push(block.start_va);
    }

    let mut pi_candidates = Vec::new();
    for &(call_pc, target) in &cfg.direct_calls {
        if let Err(limit) = charge_shape_work(&mut work, 1, limits.work, call_pc, &mut work_samples)
        {
            return limit;
        }
        if target != *epi || cfg.word_class.get(&call_pc) != Some(&WordClass::ProvenCode) {
            continue;
        }
        let Some(roots) = exact_call_blocks.get(&(call_pc, target)) else {
            continue;
        };
        for &root in roots {
            let Some(byte_count) = call_pc
                .checked_sub(root)
                .and_then(|delta| delta.checked_add(8))
            else {
                continue;
            };
            if !byte_count.is_multiple_of(4) {
                continue;
            }
            let Some(word_count) = usize::try_from(byte_count / 4).ok() else {
                continue;
            };
            let Some(end) = root.checked_add(byte_count) else {
                continue;
            };
            if word_count > 64
                || !authoritative_root(cfg, &cfg_roots, &proven_entries, root)
                || !proven_code_interval(cfg, &executable_ranges, root, end)
            {
                continue;
            }
            if let Err(limit) =
                charge_shape_work(&mut work, word_count, limits.work, root, &mut work_samples)
            {
                return limit;
            }
            if image_words(words, va_start, call_pc, 1)
                .and_then(|call| jal_target(call[0], call_pc))
                != Some(*epi)
            {
                continue;
            }
            if image_words(words, va_start, root, word_count)
                .is_some_and(is_pi_wrapper_shape_candidate)
            {
                pi_candidates.push(root);
            }
        }
    }
    pi_candidates.sort_unstable();
    pi_candidates.dedup();
    let [vram] = pi_candidates.as_slice() else {
        return OsPiStartDmaCandidateClassification::Open(
            OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape {
                candidates: pi_candidates,
            },
        );
    };
    OsPiStartDmaCandidateClassification::Candidate(OsPiStartDmaShapeCandidate {
        bank: bank.to_owned(),
        vram: *vram,
        os_epi_start_dma_shape_vram: *epi,
        device_base: OsPiDeviceBasePrerequisite::UnresolvedCartHandleAndDeviceBase,
    })
}

fn is_send_mesg(words: &[u32]) -> bool {
    words.len() >= 57
        && is_addiu(words[0], 29, 29, imm(words[0]))
        && imm(words[0]) < 0
        && is_move_addu(words[2], 16, 4)
        && is_move_addu(words[4], 21, 5)
        && is_move_addu(words[6], 18, 6)
        && jal_target(words[10], 0).is_some()
        && is_lw_at(words[12], 3, 16, 8)
        && is_lw_at(words[13], 4, 16, 16)
        && op(words[14]) == 0
        && words[14] & 0x3f == 0x2a
        && rd(words[14]) == 3
        && rs(words[14]) == 3
        && rt(words[14]) == 4
        && is_lw_at(words[34], 3, 16, 12)
        && is_lw_at(words[35], 4, 16, 8)
        && is_lw_at(words[36], 2, 16, 16)
        && op(words[37]) == 0
        && words[37] & 0x3f == 0x21
        && rd(words[37]) == 3
        && rs(words[37]) == 3
        && rt(words[37]) == 4
        && op(words[38]) == 0
        && words[38] & 0x3f == 0x1a
        && rs(words[38]) == 3
        && rt(words[38]) == 2
        && op(words[48]) == 0
        && words[48] & 0x3f == 0x10
        && rd(words[48]) == 2
        && is_lw_at(words[49], 3, 16, 20)
        && op(words[50]) == 0
        && words[50] & 0x3f == 0
        && rt(words[50]) == 2
        && rd(words[50]) == 2
        && (words[50] >> 6 & 31) == 2
        && op(words[51]) == 0
        && words[51] & 0x3f == 0x21
        && rd(words[51]) == 2
        && rs(words[51]) == 2
        && rt(words[51]) == 3
        && is_sw(words[52], 21, 2, 0)
        && is_lw_at(words[53], 2, 16, 8)
        && is_addiu(words[55], 2, 2, 1)
        && is_sw(words[56], 2, 16, 8)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreateThreadValue {
    Unknown,
    Zero,
    NonZeroConstant,
    Thread,
    Id,
    Entry,
    Argument,
    StackArgument,
    Priority,
}

fn is_create_thread(words: &[u32]) -> bool {
    const MIN_WORDS: usize = 42;
    if words.len() < MIN_WORDS || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let frame_size = i32::from(imm(words[0])).unsigned_abs();
    let (Ok(stack_arg_offset), Ok(priority_offset)) = (
        i16::try_from(frame_size + 16),
        i16::try_from(frame_size + 20),
    ) else {
        return false;
    };
    let mut registers = [CreateThreadValue::Unknown; 32];
    registers[0] = CreateThreadValue::Zero;
    registers[4] = CreateThreadValue::Thread;
    registers[5] = CreateThreadValue::Id;
    registers[6] = CreateThreadValue::Entry;
    registers[7] = CreateThreadValue::Argument;
    let mut stack = BTreeMap::new();
    let mut fields = BTreeMap::new();
    let mut saw_late_call = false;
    let mut saw_return = false;
    let mut saw_stack_argument_load = false;
    let mut clobber_after_instruction = false;

    for (index, &word) in words.iter().enumerate() {
        if is_jr_ra(word) {
            saw_return = true;
            break;
        }
        if jal_field(word).is_some() {
            if clobber_after_instruction {
                return false;
            }
            saw_late_call |= index >= 24;
            clobber_after_instruction = true;
            continue;
        }
        match op(word) {
            0x2b | 0x29 => {
                let source = registers[rt(word) as usize];
                if rs(word) == 29 {
                    if op(word) == 0x2b {
                        stack.insert(imm(word), source);
                    }
                } else if registers[rs(word) as usize] == CreateThreadValue::Thread {
                    fields.insert((op(word), imm(word)), source);
                }
            }
            0x23 => {
                let value = if rs(word) == 29 {
                    match imm(word) {
                        offset if offset == stack_arg_offset => {
                            saw_stack_argument_load = true;
                            CreateThreadValue::StackArgument
                        }
                        offset if offset == priority_offset => CreateThreadValue::Priority,
                        offset => stack
                            .get(&offset)
                            .copied()
                            .unwrap_or(CreateThreadValue::Unknown),
                    }
                } else {
                    CreateThreadValue::Unknown
                };
                if rt(word) != 0 {
                    registers[rt(word) as usize] = value;
                }
            }
            0 => {
                let destination = rd(word) as usize;
                if destination != 0 {
                    registers[destination] =
                        if matches!(word & 0x3f, 0x21 | 0x25) && (rs(word) == 0 || rt(word) == 0) {
                            let source = if rs(word) == 0 { rt(word) } else { rs(word) };
                            registers[source as usize]
                        } else {
                            CreateThreadValue::Unknown
                        };
                }
            }
            0x09 | 0x0d => {
                if rt(word) != 0 {
                    registers[rt(word) as usize] = if rs(word) == 0 && imm(word) != 0 {
                        CreateThreadValue::NonZeroConstant
                    } else {
                        CreateThreadValue::Unknown
                    };
                }
            }
            0x0f => {
                if rt(word) != 0 {
                    registers[rt(word) as usize] = CreateThreadValue::Unknown;
                }
            }
            opcode
                if matches!(
                    opcode,
                    0x08 | 0x0a..=0x0c | 0x0e | 0x20..=0x27 | 0x30..=0x37
                ) =>
            {
                if rt(word) != 0 {
                    registers[rt(word) as usize] = CreateThreadValue::Unknown;
                }
            }
            0x1c => {
                if rd(word) != 0 {
                    registers[rd(word) as usize] = CreateThreadValue::Unknown;
                }
            }
            _ => {}
        }
        if clobber_after_instruction {
            for register in 2..=15 {
                registers[register] = CreateThreadValue::Unknown;
            }
            registers[24] = CreateThreadValue::Unknown;
            registers[25] = CreateThreadValue::Unknown;
            registers[31] = CreateThreadValue::Unknown;
            clobber_after_instruction = false;
        }
    }

    let sw = |offset| fields.get(&(0x2b, offset)).copied();
    let sh = |offset| fields.get(&(0x29, offset)).copied();
    saw_return
        && saw_late_call
        && saw_stack_argument_load
        && sw(0) == Some(CreateThreadValue::Zero)
        && sw(4) == Some(CreateThreadValue::Priority)
        && sw(8) == Some(CreateThreadValue::Zero)
        && sh(0x10) == Some(CreateThreadValue::NonZeroConstant)
        && sh(0x12) == Some(CreateThreadValue::Zero)
        && sw(0x14) == Some(CreateThreadValue::Id)
        && sw(0x18) == Some(CreateThreadValue::Zero)
        && sw(0x38).is_some()
        && sw(0x3c) == Some(CreateThreadValue::Argument)
        && sw(0xf0).is_some()
        // The saved SP is the fifth argument minus the ABI call frame. The
        // exact subtraction schedule varies; requiring both context halves
        // plus the independently tagged fifth-argument load above avoids
        // baking one arithmetic sequence into this recognizer.
        && sw(0xf4).is_some()
        && sw(0x100).is_some()
        && sw(0x104).is_some()
        && sw(0x118).is_some()
        && sw(0x11c) == Some(CreateThreadValue::Entry)
        && sw(0x128).is_some()
        && sw(0x12c).is_some()
}

fn is_set_event_mesg(words: &[u32]) -> bool {
    words.len() >= 19
        && is_addiu(words[0], 29, 29, imm(words[0]))
        && imm(words[0]) < 0
        && is_move_addu(words[2], 16, 4)
        && is_move_addu(words[4], 17, 5)
        && is_move_addu(words[6], 18, 6)
        && jal_target(words[8], 0).is_some()
        && op(words[10]) == 0
        && words[10] & 0x3f == 0
        && rt(words[10]) == 16
        && rd(words[10]) == 3
        && (words[10] >> 6 & 31) == 3
        && is_addiu(words[12], 4, 4, imm(words[12]))
        && op(words[13]) == 0
        && words[13] & 0x3f == 0x21
        && rd(words[13]) == 3
        && rs(words[13]) == 3
        && rt(words[13]) == 4
        && is_move_addu(words[14], 19, 2)
        && is_addiu(words[15], 2, 0, 14)
        && is_sw(words[16], 17, 3, 0)
        && is_bne(words[17], 16, 2)
        && is_sw(words[18], 18, 3, 4)
}

fn is_start_thread(words: &[u32]) -> bool {
    words.len() >= 15
        && is_addiu(words[0], 29, 29, imm(words[0]))
        && imm(words[0]) < 0
        && is_move_addu(words[2], 16, 4)
        && jal_target(words[5], 0).is_some()
        && op(words[7]) == 0x25
        && rt(words[7]) == 3
        && rs(words[7]) == 16
        && imm(words[7]) == 0x10
        && is_addiu(words[9], 2, 0, 1)
        && op(words[10]) == 4
        && rs(words[10]) == 3
        && rt(words[10]) == 2
        && is_addiu(words[11], 2, 0, 8)
        && is_bne(words[12], 3, 2)
        && is_addiu(words[13], 2, 0, 2)
        && is_sh(words[14], 2, 16, 0x10)
}

fn is_get_thread_pri(words: &[u32]) -> bool {
    words.len() >= 6
        && op(words[0]) == 5
        && rs(words[0]) == 4
        && rt(words[0]) == 0
        && words[1] == 0
        && is_lui(words[2], 4)
        && is_lw(words[3], 4, 4)
        && is_jr_ra(words[4])
        && is_lw_at(words[5], 2, 4, 4)
}

fn is_set_thread_pri(words: &[u32]) -> bool {
    words.len() >= 20
        && is_addiu(words[0], 29, 29, imm(words[0]))
        && imm(words[0]) < 0
        && is_move_addu(words[2], 16, 4)
        && is_move_addu(words[4], 17, 5)
        && jal_target(words[6], 0).is_some()
        && op(words[8]) == 5
        && rs(words[8]) == 16
        && rt(words[8]) == 0
        && is_move_addu(words[9], 18, 2)
        && is_lui(words[10], 16)
        && is_lw(words[11], 16, 16)
        && is_lw_at(words[12], 2, 16, 4)
        && op(words[13]) == 4
        && rs(words[13]) == 2
        && rt(words[13]) == 17
        && is_lui(words[15], 2)
        && is_lw(words[16], 2, 2)
        && op(words[17]) == 4
        && rs(words[17]) == 16
        && rt(words[17]) == 2
        && is_sw(words[18], 17, 16, 4)
}

fn is_sp_task_load(words: &[u32]) -> bool {
    words.len() >= 131
        && is_addiu(words[0], 29, 29, imm(words[0]))
        && imm(words[0]) < 0
        && is_move_addu(words[2], 16, 4)
        && is_move_addu(words[6], 5, 17)
        && jal_field(words[8]).is_some()
        && is_addiu(words[9], 6, 0, 0x40)
        // The public OSTask flags word is copied with bit zero cleared for a
        // fresh load, while a yielded task selects its saved data fields.
        && is_andi(words[68], 2, 2, 1)
        && is_beq(words[69], 2, 0)
        && is_lw_at(words[79], 2, 16, 4)
        && is_addiu(words[80], 3, 0, -2)
        && op(words[81]) == 0
        && words[81] & 0x3f == 0x24
        && rd(words[81]) == 2
        && rs(words[81]) == 2
        && rt(words[81]) == 3
        && is_sw(words[82], 2, 16, 4)
        && is_andi(words[85], 2, 2, 4)
        && is_beq(words[86], 2, 0)
        && is_lw_at(words[88], 2, 16, 0x38)
        && is_move_addu(words[94], 4, 17)
        && jal_field(words[95]).is_some()
        && is_addiu(words[96], 5, 0, 0x40)
        // Clear the prior SIG0/SIG1 handshake, wait for SP DMA room, copy the
        // complete 64-byte task to DMEM 0xfc0, wait for completion, then copy
        // the task's rspboot image to IMEM zero.
        && jal_field(words[97]).is_some()
        && is_addiu(words[98], 4, 0, 0x2b00)
        && is_lui(words[100], 4)
        && (words[100] as u16) == 0x0400
        && jal_field(words[101]).is_some()
        && op(words[102]) == 0x0d
        && rt(words[102]) == 4
        && rs(words[102]) == 4
        && words[102] as u16 == 0x1000
        && is_addiu(words[106], 4, 0, 1)
        && is_lui(words[107], 5)
        && (words[107] as u16) == 0x0400
        && op(words[108]) == 0x0d
        && rt(words[108]) == 5
        && rs(words[108]) == 5
        && words[108] as u16 == 0x0fc0
        && jal_field(words[110]).is_some()
        && is_addiu(words[111], 7, 0, 0x40)
        && jal_field(words[114]).is_some()
        && is_lw_at(words[119], 6, 17, 8)
        && is_lw_at(words[120], 7, 17, 12)
        && is_lui(words[121], 5)
        && (words[121] as u16) == 0x0400
        && jal_field(words[122]).is_some()
        && jal_field(words[110]) == jal_field(words[122])
        && op(words[123]) == 0x0d
        && rt(words[123]) == 5
        && rs(words[123]) == 5
        && words[123] as u16 == 0x1000
        && is_jr_ra(words[129])
        && is_addiu(words[130], 29, 29, -imm(words[0]))
}

fn is_set_timer(words: &[u32]) -> bool {
    words.len() >= 75
        && is_addiu(words[0], 29, 29, -32)
        // o32 stack arguments become sp+0x30..0x3c after this frame: the
        // interval high/low words, destination queue, and message.
        && is_lw_at(words[1], 2, 29, 0x30)
        && is_lw_at(words[2], 3, 29, 0x34)
        && is_move_addu(words[4], 16, 4)
        // Public OSTimer linkage/deadline/interval/route fields.
        && is_sw(words[8], 0, 16, 0)
        && is_sw(words[9], 0, 16, 4)
        && is_sw(words[10], 6, 16, 0x10)
        && is_sw(words[11], 7, 16, 0x14)
        && is_sw(words[12], 2, 16, 8)
        && is_sw(words[13], 3, 16, 0x0c)
        && is_lw_at(words[14], 4, 29, 0x38)
        && is_lw_at(words[15], 5, 29, 0x3c)
        && is_sw(words[17], 4, 16, 0x18)
        && is_sw(words[19], 4, 16, 0x18)
        && is_sw(words[20], 2, 16, 0x10)
        && is_sw(words[21], 3, 16, 0x14)
        && is_sw(words[22], 4, 16, 0x18)
        && jal_field(words[23]).is_some()
        && is_sw(words[24], 5, 16, 0x1c)
        && jal_field(words[30]).is_some()
        && jal_field(words[58]).is_some()
        && is_move_addu(words[59], 4, 16)
        && jal_field(words[64]).is_some()
        && jal_field(words[66]).is_some()
        && is_zero_addu(words[68], 2)
        && is_jr_ra(words[73])
        && is_addiu(words[74], 29, 29, 32)
}

fn is_sp_task_start_go(words: &[u32], busy: u32, set_status: u32) -> bool {
    words.len() >= 11
        && is_addiu(words[0], 29, 29, -24)
        && is_sw(words[1], 31, 29, 16)
        && jal_field(words[2]) == Some(busy)
        && words[3] == 0
        && is_bne(words[4], 2, 0)
        && imm(words[4]) == -3
        && words[5] == 0
        && jal_field(words[6]) == Some(set_status)
        && is_addiu(words[7], 4, 0, 0x125)
        && is_lw_at(words[8], 31, 29, 16)
        && is_jr_ra(words[9])
        && is_addiu(words[10], 29, 29, 24)
}

fn is_sp_task_yield(words: &[u32], set_status: u32) -> bool {
    words.len() >= 7
        && is_addiu(words[0], 29, 29, -24)
        && is_sw(words[1], 31, 29, 16)
        && jal_field(words[2]) == Some(set_status)
        && is_addiu(words[3], 4, 0, 0x400)
        && is_lw_at(words[4], 31, 29, 16)
        && is_jr_ra(words[5])
        && is_addiu(words[6], 29, 29, 24)
}

fn is_sp_task_yielded(words: &[u32]) -> bool {
    words.len() >= 19
        && is_addiu(words[0], 29, 29, -24)
        && is_sw(words[1], 16, 29, 16)
        && is_sw(words[2], 31, 29, 20)
        && jal_target(words[3], 0).is_some()
        && is_move_addu(words[4], 16, 4)
        && op(words[5]) == 0
        && words[5] & 0x3f == 2
        && rt(words[5]) == 2
        && rd(words[5]) == 4
        && (words[5] >> 6 & 31) == 8
        && is_andi(words[6], 2, 2, 0x80)
        && is_beq(words[7], 2, 0)
        && is_andi(words[8], 4, 4, 1)
        && is_lw_at(words[9], 2, 16, 4)
        && is_addiu(words[10], 3, 0, -3)
        && op(words[11]) == 0
        && words[11] & 0x3f == 0x25
        && rd(words[11]) == 2
        && rs(words[11]) == 2
        && rt(words[11]) == 4
        && op(words[12]) == 0
        && words[12] & 0x3f == 0x24
        && rd(words[12]) == 2
        && rs(words[12]) == 2
        && rt(words[12]) == 3
        && is_sw(words[13], 2, 16, 4)
        && is_move_addu(words[14], 2, 4)
        && is_lw_at(words[15], 31, 29, 20)
        && is_lw_at(words[16], 16, 29, 16)
        && is_jr_ra(words[17])
        && is_addiu(words[18], 29, 29, 24)
}

fn unique_match(
    words: &[u32],
    va_start: u32,
    width: usize,
    symbol: HostBindingSymbol,
    predicate: impl Fn(&[u32]) -> bool,
) -> Result<u32, HostBindingDiscoveryError> {
    let mut candidates = words
        .windows(width)
        .enumerate()
        .filter_map(|(index, window)| {
            predicate(window)
                .then(|| va_start.checked_add(u32::try_from(index).ok()?.checked_mul(4)?))?
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.as_slice() {
        [address] => Ok(*address),
        _ => Err(HostBindingDiscoveryError::NonUniqueSemanticMatch { symbol, candidates }),
    }
}

fn unique_create_thread_match(
    words: &[u32],
    va_start: u32,
) -> Result<u32, HostBindingDiscoveryError> {
    const MIN_WORDS: usize = 42;
    const MAX_WORDS: usize = 96;
    let mut candidates = (0..=words.len().saturating_sub(MIN_WORDS))
        .filter_map(|index| {
            let end = words.len().min(index + MAX_WORDS);
            is_create_thread(&words[index..end])
                .then(|| va_start.checked_add(u32::try_from(index).ok()?.checked_mul(4)?))?
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.as_slice() {
        [address] => Ok(*address),
        _ => Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
            symbol: HostBindingSymbol::OsCreateThread,
            candidates,
        }),
    }
}

fn is_si_device_busy(words: &[u32]) -> bool {
    words.len() == 6
        // Public N64 hardware documentation places SI_STATUS_REG at
        // 0xa480_0018; bits 0 and 1 are the DMA/read busy indicators.
        && is_lui(words[0], 2)
        && words[0] as u16 == 0xa480
        && op(words[1]) == 0x0d
        && rt(words[1]) == 2
        && rs(words[1]) == 2
        && words[1] as u16 == 0x0018
        && is_lw_at(words[2], 2, 2, 0)
        && is_andi(words[3], 2, 2, 3)
        && is_jr_ra(words[4])
        && op(words[5]) == 0
        && words[5] & 0x3f == 0x2b
        && rd(words[5]) == 2
        && rs(words[5]) == 0
        && rt(words[5]) == 2
}

/// Structural shape of libultra's 64DD drive initialisation.
///
/// The routine is recognised by what it *does*, not by any address: it loads a
/// once-only guard word through a `lui`/`lw` pair, branches away when that word
/// is already non-zero, and on the first-call path installs the 64DD base
/// `0xA600_0000` into the handle it will later probe.
///
/// The `lui $r, 0xA600` is the distinguishing behaviour. `0x0600_0000..=
/// 0x07ff_ffff` is `PI_DOM1_ADDR1`, the disk-drive window, and a cartridge-only
/// title has no device there -- which is why the probe that follows faults with
/// `abi.pi.absent-domain1-device`. No other libultra role installs that base.
///
/// Window is 20 words. Measured on WM2000: the guard `lui`/`lw` sits at
/// +0x00/+0x04 and the `lui $r, 0xA600` at +0x34, i.e. 13 words later, with
/// register setup and the handle-pointer construction in between.
fn is_drive_rom_init(words: &[u32]) -> bool {
    // lui/lw pair loading the once-only guard.
    if !(is_lui(words[0], 14) && is_lw(words[1], 14, 14)) {
        return false;
    }
    // A branch that skips initialisation when the guard is already set.
    let guard_branch = words[2..6]
        .iter()
        .any(|word| op(*word) == 0x05 && (rs(*word) == 14 || rt(*word) == 14));
    if !guard_branch {
        return false;
    }
    // The first-call path installs the 64DD base into the handle.
    words[2..20]
        .iter()
        .any(|word| op(*word) == 0x0f && rs(*word) == 0 && (*word & 0xffff) == 0xa600)
}

/// Discover libultra's 64DD drive initialisation from its public guard-word and
/// base-installation behavior.
///
/// This role is optional: a title that never touches the disk drive contains no
/// such routine, and `Ok(None)` distinguishes that from a ROM where the shape is
/// ambiguous, which stays a loud failure like every other role here.
pub fn discover_drive_rom_init_host_binding(
    words: &[u32],
    va_start: u32,
) -> Result<Option<HostBinding>, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    match unique_match(
        words,
        va_start,
        20,
        HostBindingSymbol::OsDriveRomInit,
        is_drive_rom_init,
    ) {
        Ok(vram) => Ok(Some(HostBinding {
            symbol: HostBindingSymbol::OsDriveRomInit,
            vram,
        })),
        // `unique_match` reports both "no match" and "several matches" as
        // NonUniqueSemanticMatch; an empty candidate list is the absent case.
        // A title with no disk-drive routine is normal, several is ambiguous
        // and stays a loud failure like every other role here.
        Err(HostBindingDiscoveryError::NonUniqueSemanticMatch { candidates, .. })
            if candidates.is_empty() =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Discover `__osSiDeviceBusy` from its public SI status-register behavior.
/// The role must have exactly one structural match in the resident image.
pub fn discover_si_device_busy_host_binding(
    words: &[u32],
    va_start: u32,
) -> Result<HostBinding, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    let vram = unique_match(
        words,
        va_start,
        6,
        HostBindingSymbol::OsSiDeviceBusy,
        is_si_device_busy,
    )?;
    Ok(HostBinding {
        symbol: HostBindingSymbol::OsSiDeviceBusy,
        vram,
    })
}

/// Discover `osCreateThread` from its public `OSThread` initialization
/// behavior. The role must have exactly one structural match in the resident
/// image.
pub fn discover_os_create_thread_host_binding(
    words: &[u32],
    va_start: u32,
) -> Result<HostBinding, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    va_start
        .checked_add(
            u32::try_from(words.len())
                .map_err(|_| HostBindingDiscoveryError::AddressOverflow)?
                .checked_mul(4)
                .ok_or(HostBindingDiscoveryError::AddressOverflow)?,
        )
        .ok_or(HostBindingDiscoveryError::AddressOverflow)?;
    let vram = unique_create_thread_match(words, va_start)?;
    Ok(HostBinding {
        symbol: HostBindingSymbol::OsCreateThread,
        vram,
    })
}

/// Discover the host calls required by the admitted libultra overlay-loading
/// sequence in one resident image. The returned inventory is symbol-sorted.
pub fn discover_overlay_loader_host_bindings(
    words: &[u32],
    va_start: u32,
) -> Result<Vec<HostBinding>, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    va_start
        .checked_add(
            u32::try_from(words.len())
                .map_err(|_| HostBindingDiscoveryError::AddressOverflow)?
                .checked_mul(4)
                .ok_or(HostBindingDiscoveryError::AddressOverflow)?,
        )
        .ok_or(HostBindingDiscoveryError::AddressOverflow)?;
    let create = unique_match(
        words,
        va_start,
        9,
        HostBindingSymbol::OsCreateMesgQueue,
        is_create_mesg_queue,
    )?;
    let create_thread = discover_os_create_thread_host_binding(words, va_start)?.vram;
    let epi = unique_match(
        words,
        va_start,
        15,
        HostBindingSymbol::OsEPiStartDma,
        is_epi_start_dma,
    )?;
    let get_thread_pri = unique_match(
        words,
        va_start,
        6,
        HostBindingSymbol::OsGetThreadPri,
        is_get_thread_pri,
    )?;
    let send = unique_match(
        words,
        va_start,
        57,
        HostBindingSymbol::OsSendMesg,
        is_send_mesg,
    )?;
    let set_event = unique_match(
        words,
        va_start,
        19,
        HostBindingSymbol::OsSetEventMesg,
        is_set_event_mesg,
    )?;
    let set_thread_pri = unique_match(
        words,
        va_start,
        20,
        HostBindingSymbol::OsSetThreadPri,
        is_set_thread_pri,
    )?;
    let start_thread = unique_match(
        words,
        va_start,
        15,
        HostBindingSymbol::OsStartThread,
        is_start_thread,
    )?;
    let mut chains = Vec::new();
    for (create_call_index, &word) in words.iter().enumerate() {
        let create_call_pc = va_start + create_call_index as u32 * 4;
        if jal_target(word, create_call_pc) != Some(create) {
            continue;
        }
        let search_end = (create_call_index + 128).min(words.len());
        for epi_call_index in create_call_index + 1..search_end {
            let epi_call_pc = va_start + epi_call_index as u32 * 4;
            if jal_target(words[epi_call_index], epi_call_pc) != Some(epi) {
                continue;
            }
            let recv_end = (epi_call_index + 12).min(words.len());
            for recv_call_index in epi_call_index + 1..recv_end {
                if recv_call_index < 2 || recv_call_index + 1 >= words.len() {
                    continue;
                }
                let recv_call_pc = va_start + recv_call_index as u32 * 4;
                let Some(recv) = jal_target(words[recv_call_index], recv_call_pc) else {
                    continue;
                };
                if is_addiu(
                    words[recv_call_index - 2],
                    4,
                    29,
                    imm(words[recv_call_index - 2]),
                ) && is_addiu(
                    words[recv_call_index - 1],
                    5,
                    29,
                    imm(words[recv_call_index - 1]),
                ) && is_addiu(words[recv_call_index + 1], 6, 0, 1)
                {
                    chains.push((create_call_pc, epi_call_pc, recv));
                }
            }
        }
    }
    chains.sort_unstable();
    chains.dedup();
    let mut recv_targets = chains.iter().map(|(_, _, recv)| *recv).collect::<Vec<_>>();
    recv_targets.sort_unstable();
    recv_targets.dedup();
    let recv = match recv_targets.as_slice() {
        [recv] => *recv,
        _ => {
            return Err(HostBindingDiscoveryError::NonUniqueOverlayCallChain { candidates: chains })
        }
    };
    Ok(vec![
        HostBinding {
            symbol: HostBindingSymbol::OsCreateMesgQueue,
            vram: create,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsCreateThread,
            vram: create_thread,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsEPiStartDma,
            vram: epi,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsGetThreadPri,
            vram: get_thread_pri,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsRecvMesg,
            vram: recv,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSendMesg,
            vram: send,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSetEventMesg,
            vram: set_event,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSetThreadPri,
            vram: set_thread_pri,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsStartThread,
            vram: start_thread,
        },
    ])
}

/// Discover the public RSP task admission/start and yield/query routines in a
/// resident libultra image. Cross-function helper identities are part of the
/// proof: Load and StartGo must share the busy/status helpers, and Yield must
/// use that same status helper. The returned inventory is symbol-sorted.
pub fn discover_rsp_task_host_bindings(
    words: &[u32],
    va_start: u32,
) -> Result<Vec<HostBinding>, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    va_start
        .checked_add(
            u32::try_from(words.len())
                .map_err(|_| HostBindingDiscoveryError::AddressOverflow)?
                .checked_mul(4)
                .ok_or(HostBindingDiscoveryError::AddressOverflow)?,
        )
        .ok_or(HostBindingDiscoveryError::AddressOverflow)?;

    let load = unique_match(
        words,
        va_start,
        131,
        HostBindingSymbol::OsSpTaskLoad,
        is_sp_task_load,
    )?;
    let load_index = usize::try_from((load - va_start) / 4).expect("load index fits usize");
    let load_words = &words[load_index..load_index + 131];
    let set_status = jal_field(load_words[97]).expect("load recognizer proved status call");
    let busy = jal_field(load_words[114]).expect("load recognizer proved busy call");

    let start_go = unique_match(
        words,
        va_start,
        11,
        HostBindingSymbol::OsSpTaskStartGo,
        |candidate| is_sp_task_start_go(candidate, busy, set_status),
    )?;
    let task_yield = unique_match(
        words,
        va_start,
        7,
        HostBindingSymbol::OsSpTaskYield,
        |candidate| is_sp_task_yield(candidate, set_status),
    )?;
    let task_yielded = unique_match(
        words,
        va_start,
        19,
        HostBindingSymbol::OsSpTaskYielded,
        is_sp_task_yielded,
    )?;

    Ok(vec![
        HostBinding {
            symbol: HostBindingSymbol::OsSpTaskLoad,
            vram: load,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSpTaskStartGo,
            vram: start_go,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSpTaskYield,
            vram: task_yield,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSpTaskYielded,
            vram: task_yielded,
        },
    ])
}

/// Discover the public timer-wheel admission routine in a resident libultra
/// image. The recognizer is based on the documented OSTimer field/argument
/// layout and list insertion behavior; the address is only an output.
pub fn discover_timer_host_bindings(
    words: &[u32],
    va_start: u32,
) -> Result<Vec<HostBinding>, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    va_start
        .checked_add(
            u32::try_from(words.len())
                .map_err(|_| HostBindingDiscoveryError::AddressOverflow)?
                .checked_mul(4)
                .ok_or(HostBindingDiscoveryError::AddressOverflow)?,
        )
        .ok_or(HostBindingDiscoveryError::AddressOverflow)?;
    let set_timer = unique_match(
        words,
        va_start,
        75,
        HostBindingSymbol::OsSetTimer,
        is_set_timer,
    )?;
    Ok(vec![HostBinding {
        symbol: HostBindingSymbol::OsSetTimer,
        vram: set_timer,
    }])
}

/// Discover the exact host-adapter catalog installed by the WM block runtime.
/// Keeping this assembly in the library prevents the production build and the
/// source-frontier producer from silently drifting to different target sets.
pub fn discover_wm_block_runtime_host_bindings(
    words: &[u32],
    va_start: u32,
) -> Result<Vec<HostBinding>, HostBindingDiscoveryError> {
    let mut bindings = discover_overlay_loader_host_bindings(words, va_start)?;
    bindings.extend(discover_rsp_task_host_bindings(words, va_start)?);
    bindings.extend(discover_timer_host_bindings(words, va_start)?);
    bindings.push(discover_si_device_busy_host_binding(words, va_start)?);
    bindings.sort_by_key(|binding| binding.symbol);
    for (index, binding) in bindings.iter().enumerate() {
        if let Some(conflict) = bindings[..index]
            .iter()
            .find(|known| known.vram == binding.vram)
        {
            return Err(HostBindingDiscoveryError::ConflictingBindingAddress {
                vram: binding.vram,
                first: conflict.symbol,
                second: binding.symbol,
            });
        }
    }
    Ok(bindings)
}

#[cfg(test)]
mod tests;
