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
mod tests {
    use super::*;

    mod os_pi_candidate_classifier {
        use super::*;
        use crate::cfg::{BasicBlock, BlockTerminator};
        use crate::facts::{
            executable_range_subject, function_entry_subject, BankAddr, CandidateDetector, Fact,
            FunctionEntryEvidence, ProofState,
        };

        const BASE: u32 = 0x8000_1000;
        const PI: u32 = BASE;
        const EPI: u32 = BASE + 0x100;

        fn jal(pc: u32, target: u32) -> u32 {
            assert_eq!((pc + 4) & 0xf000_0000, target & 0xf000_0000);
            0x0c00_0000 | (target >> 2 & 0x03ff_ffff)
        }

        fn pi_words(entry: u32, epi: u32) -> Vec<u32> {
            vec![
                0x27bd_ffe0,
                0xafbf_001c,
                0x0080_4021,
                0xa105_0002,
                0xa100_0003,
                0x8fa9_0030,
                0xad09_0008,
                0xad07_000c,
                0x8faa_0034,
                0xad0a_0010,
                0x8fab_0038,
                0xad0b_0004,
                0x0100_2821,
                0x3c04_8000,
                jal(entry + 14 * 4, epi),
                0x00c0_3021,
            ]
        }

        fn epi_words() -> [u32; 15] {
            [
                0x3c02_8030,
                0x8c42_1000,
                0x27bd_ffe8,
                0xafb0_0010,
                0x00a0_8021,
                0x1440_0002,
                0xafbf_0014,
                0x0800_0500,
                0x2402_ffff,
                0x14c0_0002,
                0xae04_0014,
                0x0800_0504,
                0x2402_000f,
                0x2402_0010,
                0xa602_0000,
            ]
        }

        fn fixture() -> (Vec<u32>, Cfg, FactDb) {
            let mut words = vec![0; 96];
            let pi = pi_words(PI, EPI);
            words[..pi.len()].copy_from_slice(&pi);
            words[64..79].copy_from_slice(&epi_words());
            let mut cfg = Cfg {
                bank: "resident".into(),
                word_class: BTreeMap::new(),
                blocks: vec![
                    BasicBlock {
                        start_va: PI,
                        end_va: PI + (pi.len() as u32) * 4,
                        terminator: BlockTerminator::Call {
                            target: EPI,
                            next: PI + (pi.len() as u32) * 4,
                        },
                    },
                    BasicBlock {
                        start_va: EPI,
                        end_va: EPI + 15 * 4,
                        terminator: BlockTerminator::Fallthrough { next: EPI + 15 * 4 },
                    },
                ],
                direct_calls: vec![(PI + 14 * 4, EPI)],
                tail_transfers: vec![],
                indirect_sites: vec![],
                plain_delay_entry_aliases: vec![],
                unsupported_delay_entries: vec![],
                rejected_transfer_targets: Vec::new(),
                proven_roots: vec![PI, EPI],
            };
            for pc in (PI..PI + (pi.len() as u32) * 4).step_by(4) {
                cfg.word_class.insert(pc, WordClass::ProvenCode);
            }
            for pc in (EPI..EPI + 15 * 4).step_by(4) {
                cfg.word_class.insert(pc, WordClass::ProvenCode);
            }

            let mut facts = FactDb::new();
            let end = BASE + (words.len() as u32) * 4;
            let executable = facts.insert(Fact::ExecutableRange {
                bank: "resident".into(),
                va_start: BASE,
                va_end: end,
            });
            facts
                .conclude(
                    executable_range_subject("resident", BASE, end),
                    ProofState::Proven,
                    vec![executable],
                    "test executable authority",
                )
                .unwrap();
            for pc in [PI, EPI] {
                let target = BankAddr::new("resident", pc);
                let claim = facts.insert(Fact::FunctionEntryClaim {
                    target: target.clone(),
                    detector: CandidateDetector::JalTarget,
                    evidence: FunctionEntryEvidence::DirectJal {
                        call_site: BankAddr::new("resident", pc.wrapping_sub(4)),
                    },
                    proposed_state: ProofState::Proven,
                });
                facts
                    .conclude(
                        function_entry_subject(&target),
                        ProofState::Proven,
                        vec![claim],
                        "test root authority",
                    )
                    .unwrap();
            }
            (words, cfg, facts)
        }

        #[test]
        fn classifies_only_relational_wrapper_shape() {
            let (words, cfg, facts) = fixture();
            assert_eq!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Candidate(OsPiStartDmaShapeCandidate {
                    bank: "resident".into(),
                    vram: PI,
                    os_epi_start_dma_shape_vram: EPI,
                    device_base: OsPiDeviceBasePrerequisite::UnresolvedCartHandleAndDeviceBase,
                })
            );
        }

        #[test]
        fn wrong_message_field_target_stays_open() {
            let (mut words, cfg, facts) = fixture();
            words[6] = 0xad09_000c;
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn stale_cfg_edge_cannot_classify_a_wrong_machine_call_target() {
            let (mut words, cfg, facts) = fixture();
            words[14] = jal(PI + 14 * 4, EPI + 4);
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn stale_straight_line_block_cannot_hide_an_earlier_branch() {
            let (mut words, cfg, facts) = fixture();
            words[13] = 0x1000_0001;
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn unmodeled_gpr_writer_clobbers_a_stale_direction_tag() {
            let (mut words, cfg, facts) = fixture();
            words[13] = 0x9326_0000; // lbu a2,0(t9)
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn unmodeled_width_or_aliasing_store_stays_open() {
            let (mut words, cfg, facts) = fixture();
            words[13] = 0xa720_0000; // sh zero,0(t9)
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn cfg_call_block_terminator_must_match_exactly() {
            let (words, mut cfg, facts) = fixture();
            cfg.blocks[0].terminator = BlockTerminator::Call {
                target: EPI,
                next: PI + 15 * 4,
            };
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn every_candidate_classifier_cap_is_loud_and_sampled() {
            let (words, cfg, facts) = fixture();
            let cases = [
                (
                    OsPiShapeLimits {
                        roots: 1,
                        ..DEFAULT_PI_SHAPE_LIMITS
                    },
                    OsPiCandidateLimitKind::Roots,
                    2,
                    1,
                    PI,
                ),
                (
                    OsPiShapeLimits {
                        calls: 0,
                        ..DEFAULT_PI_SHAPE_LIMITS
                    },
                    OsPiCandidateLimitKind::DirectCalls,
                    1,
                    0,
                    PI + 14 * 4,
                ),
                (
                    OsPiShapeLimits {
                        blocks: 1,
                        ..DEFAULT_PI_SHAPE_LIMITS
                    },
                    OsPiCandidateLimitKind::Blocks,
                    2,
                    1,
                    PI,
                ),
                (
                    OsPiShapeLimits {
                        work: 1,
                        ..DEFAULT_PI_SHAPE_LIMITS
                    },
                    OsPiCandidateLimitKind::Work,
                    16,
                    1,
                    PI,
                ),
            ];
            for (limits, kind, observed, cap, first_sample) in cases {
                assert!(matches!(
                    classify_os_pi_start_dma_candidate_with_limits(
                        "resident", &words, BASE, &cfg, &facts, limits
                    ),
                    OsPiStartDmaCandidateClassification::Open(
                        OsPiStartDmaCandidateOpenReason::LimitHit {
                            kind: actual_kind,
                            observed: actual_observed,
                            cap: actual_cap,
                            samples,
                        }
                    ) if actual_kind == kind
                        && actual_observed == observed
                        && actual_cap == cap
                        && samples.first() == Some(&first_sample)
                ));
            }
        }

        #[test]
        fn ambiguous_epi_target_stays_open() {
            let (mut words, mut cfg, mut facts) = fixture();
            let second_epi = BASE + 0x140;
            words[80..95].copy_from_slice(&epi_words());
            cfg.proven_roots.push(second_epi);
            cfg.blocks.push(BasicBlock {
                start_va: second_epi,
                end_va: second_epi + 15 * 4,
                terminator: BlockTerminator::Fallthrough {
                    next: second_epi + 15 * 4,
                },
            });
            for pc in (second_epi..second_epi + 15 * 4).step_by(4) {
                cfg.word_class.insert(pc, WordClass::ProvenCode);
            }
            let target = BankAddr::new("resident", second_epi);
            let claim = facts.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::JalTarget,
                evidence: FunctionEntryEvidence::DirectJal {
                    call_site: BankAddr::new("resident", second_epi - 4),
                },
                proposed_state: ProofState::Proven,
            });
            facts
                .conclude(
                    function_entry_subject(&target),
                    ProofState::Proven,
                    vec![claim],
                    "test second root authority",
                )
                .unwrap();
            assert_eq!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsEPiStartDmaShape {
                        candidates: vec![EPI, second_epi],
                    }
                )
            );
        }

        #[test]
        fn unreachable_image_lookalike_is_not_a_candidate() {
            let (words, mut cfg, facts) = fixture();
            cfg.proven_roots.retain(|root| *root != PI);
            cfg.blocks.retain(|block| block.start_va != PI);
            cfg.direct_calls.clear();
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }
    }

    #[test]
    fn si_device_busy_is_unique_and_invariant_sensitive() {
        let base = 0x8000_1000;
        let expected = base + 8;
        let mut words = vec![0, 0];
        words.extend([
            0x3c02_a480,
            0x3442_0018,
            0x8c42_0000,
            0x3042_0003,
            0x03e0_0008,
            0x0002_102b,
        ]);
        assert_eq!(
            discover_si_device_busy_host_binding(&words, base).unwrap(),
            HostBinding {
                symbol: HostBindingSymbol::OsSiDeviceBusy,
                vram: expected,
            }
        );

        for index in 2..8 {
            let mut broken = words.clone();
            broken[index] ^= 1;
            assert!(matches!(
                discover_si_device_busy_host_binding(&broken, base),
                Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                    symbol: HostBindingSymbol::OsSiDeviceBusy,
                    candidates,
                }) if candidates.is_empty()
            ));
        }
        words.extend_from_slice(&[
            0x3c02_a480,
            0x3442_0018,
            0x8c42_0000,
            0x3042_0003,
            0x03e0_0008,
            0x0002_102b,
        ]);
        assert!(matches!(
            discover_si_device_busy_host_binding(&words, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsSiDeviceBusy,
                candidates,
            }) if candidates.len() == 2
        ));
    }

    #[test]
    fn wm_host_status_effect_catalog_is_explicit_for_every_symbol() {
        let symbols = WM_BLOCK_RUNTIME_HOST_SYMBOLS;
        assert_eq!(symbols.len(), 15);
        for symbol in symbols {
            assert_eq!(
                symbol.current_status_effect(),
                HostCurrentStatusEffect::CBridgeRuntimeEnforcedPreservesBev
            );
            assert_eq!(
                symbol.spawned_status_effect(),
                if symbol == HostBindingSymbol::OsCreateThread {
                    HostSpawnedStatusEffect::InheritsCallerClearingFr
                } else {
                    HostSpawnedStatusEffect::None
                }
            );
        }
    }

    fn jal(pc: u32, target: u32) -> u32 {
        assert_eq!((pc + 4) & 0xf000_0000, target & 0xf000_0000);
        0x0c00_0000 | (target >> 2 & 0x03ff_ffff)
    }

    fn create_thread_fixture(pc: u32) -> [u32; 42] {
        let mut words = [0; 42];
        words[0] = 0x27bd_ffe8;
        words[2] = 0x0080_9821;
        words[7] = 0x2402_0001;
        words[8] = 0x00e0_4825;
        words[10] = 0xae62_0100;
        words[11] = 0xae63_0104;
        words[12] = 0xae62_0118;
        words[13] = 0xae62_0128;
        words[14] = 0xae65_0014;
        words[15] = 0xae60_0000;
        words[16] = 0xae60_0008;
        words[17] = 0xae66_011c;
        words[18] = 0xae68_0038;
        words[19] = 0xae69_003c;
        words[20] = 0xae64_012c;
        words[21] = 0xae60_0018;
        words[22] = 0xa662_0010;
        words[23] = 0xa660_0012;
        words[24] = 0x8fa2_002c;
        words[25] = 0xae62_0004;
        words[32] = 0x8fab_0028;
        words[34] = 0xae6a_00f0;
        words[35] = 0xae6b_00f4;
        words[39] = jal(pc + 39 * 4, pc + 0x1000);
        words[41] = 0x03e0_0008;
        words
    }

    #[test]
    fn create_thread_role_is_unique_absent_or_ambiguous() {
        let base = 0x8040_1000;
        let fixture = create_thread_fixture(base + 8);
        let mut words = vec![0, 0];
        words.extend(fixture);
        assert_eq!(
            discover_os_create_thread_host_binding(&words, base).unwrap(),
            HostBinding {
                symbol: HostBindingSymbol::OsCreateThread,
                vram: base + 8,
            }
        );

        let mut absent = words.clone();
        absent[2 + 14] ^= 1;
        assert!(matches!(
            discover_os_create_thread_host_binding(&absent, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsCreateThread,
                candidates,
            }) if candidates.is_empty()
        ));

        words.extend(fixture);
        assert!(matches!(
            discover_os_create_thread_host_binding(&words, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsCreateThread,
                candidates,
            }) if candidates.len() == 2
        ));
    }

    #[test]
    fn structural_roles_produce_addresses_without_address_signatures() {
        let base = 0x8000_0000;
        let create_index = 8usize;
        let epi_index = 48usize;
        let recv_index = 80usize;
        let send_index = 180usize;
        let create_thread_index = 240usize;
        let start_thread_index = 290usize;
        let set_event_index = 330usize;
        let get_thread_pri_index = 352usize;
        let set_thread_pri_index = 360usize;
        let sp_task_load_index = 420usize;
        let sp_task_start_go_index = 560usize;
        let sp_task_yield_index = 580usize;
        let sp_task_yielded_index = 600usize;
        let set_timer_index = 650usize;
        let loader_index = 120usize;
        let mut words = vec![0u32; 740];
        words[create_index..create_index + 9].copy_from_slice(&[
            0x3c02_8123,
            0x2442_4560,
            0xac82_0000,
            0xac82_0004,
            0xac80_0008,
            0xac80_000c,
            0xac86_0010,
            0x03e0_0008,
            0xac85_0014,
        ]);
        words[epi_index..epi_index + 15].copy_from_slice(&[
            0x3c02_8123,
            0x8c42_4560,
            0x27bd_ffe8,
            0xafb0_0010,
            0x00a0_8021,
            0x1440_0003,
            0xafbf_0014,
            0x0800_1234,
            0x2402_ffff,
            0x14c0_0003,
            0xae04_0014,
            0x0800_1235,
            0x2402_000f,
            0x2402_0010,
            0xa602_0000,
        ]);
        let create = base + create_index as u32 * 4;
        let epi = base + epi_index as u32 * 4;
        let recv = base + recv_index as u32 * 4;
        words[loader_index] = jal(base + loader_index as u32 * 4, create);
        words[loader_index + 20] = jal(base + (loader_index + 20) as u32 * 4, epi);
        words[loader_index + 25] = 0x27a4_0010;
        words[loader_index + 26] = 0x27a5_002c;
        words[loader_index + 27] = jal(base + (loader_index + 27) as u32 * 4, recv);
        words[loader_index + 28] = 0x2406_0001;
        let send = base + send_index as u32 * 4;
        let send_words = &mut words[send_index..send_index + 57];
        send_words[0] = 0x27bd_ffd0;
        send_words[2] = 0x0080_8021;
        send_words[4] = 0x00a0_a821;
        send_words[6] = 0x00c0_9021;
        send_words[10] = jal(send + 40, base + 0x1000);
        send_words[12] = 0x8e03_0008;
        send_words[13] = 0x8e04_0010;
        send_words[14] = 0x0064_182a;
        send_words[34] = 0x8e03_000c;
        send_words[35] = 0x8e04_0008;
        send_words[36] = 0x8e02_0010;
        send_words[37] = 0x0064_1821;
        send_words[38] = 0x0062_001a;
        send_words[48] = 0x0000_1010;
        send_words[49] = 0x8e03_0014;
        send_words[50] = 0x0002_1080;
        send_words[51] = 0x0043_1021;
        send_words[52] = 0xac55_0000;
        send_words[53] = 0x8e02_0008;
        send_words[55] = 0x2442_0001;
        send_words[56] = 0xae02_0008;
        let create_thread = base + create_thread_index as u32 * 4;
        // Use s3 and a different legal store schedule so this fixture proves
        // the role from OSThread initialization rather than one compiler's
        // saved-register choice and instruction positions.
        words[create_thread_index..create_thread_index + 42]
            .copy_from_slice(&create_thread_fixture(create_thread));
        let start_thread = base + start_thread_index as u32 * 4;
        let start_words = &mut words[start_thread_index..start_thread_index + 15];
        start_words[0] = 0x27bd_ffe0;
        start_words[2] = 0x0080_8021;
        start_words[5] = jal(start_thread + 5 * 4, base + 0x1000);
        start_words[7] = 0x9603_0010;
        start_words[9] = 0x2402_0001;
        start_words[10] = 0x1062_0008;
        start_words[11] = 0x2402_0008;
        start_words[12] = 0x1462_001e;
        start_words[13] = 0x2402_0002;
        start_words[14] = 0xa602_0010;
        let set_event = base + set_event_index as u32 * 4;
        let event_words = &mut words[set_event_index..set_event_index + 19];
        event_words[0] = 0x27bd_ffd8;
        event_words[2] = 0x0080_8021;
        event_words[4] = 0x00a0_8821;
        event_words[6] = 0x00c0_9021;
        event_words[8] = jal(set_event + 8 * 4, base + 0x1000);
        event_words[10] = 0x0010_18c0;
        event_words[12] = 0x2484_ed68;
        event_words[13] = 0x0064_1821;
        event_words[14] = 0x0040_9821;
        event_words[15] = 0x2402_000e;
        event_words[16] = 0xac71_0000;
        event_words[17] = 0x1602_0010;
        event_words[18] = 0xac72_0004;
        let get_thread_pri = base + get_thread_pri_index as u32 * 4;
        words[get_thread_pri_index..get_thread_pri_index + 6].copy_from_slice(&[
            0x1480_0003,
            0,
            0x3c04_8123,
            0x8c84_4560,
            0x03e0_0008,
            0x8c82_0004,
        ]);
        let set_thread_pri = base + set_thread_pri_index as u32 * 4;
        let pri_words = &mut words[set_thread_pri_index..set_thread_pri_index + 20];
        pri_words[0] = 0x27bd_ffe0;
        pri_words[2] = 0x0080_8021;
        pri_words[4] = 0x00a0_8821;
        pri_words[6] = jal(set_thread_pri + 6 * 4, base + 0x1000);
        pri_words[8] = 0x1600_0003;
        pri_words[9] = 0x0040_9021;
        pri_words[10] = 0x3c10_8123;
        pri_words[11] = 0x8e10_4560;
        pri_words[12] = 0x8e02_0004;
        pri_words[13] = 0x1051_001c;
        pri_words[15] = 0x3c02_8123;
        pri_words[16] = 0x8c42_4560;
        pri_words[17] = 0x1202_000b;
        pri_words[18] = 0xae11_0004;

        let sp_task_load = base + sp_task_load_index as u32 * 4;
        let load_words = &mut words[sp_task_load_index..sp_task_load_index + 131];
        load_words[0] = 0x27bd_ffe0;
        load_words[2] = 0x0080_8021;
        load_words[6] = 0x0220_2821;
        load_words[8] = jal(sp_task_load + 8 * 4, base + 0x1700);
        load_words[9] = 0x2406_0040;
        load_words[68] = 0x3042_0001;
        load_words[69] = 0x1040_0019;
        load_words[79] = 0x8e02_0004;
        load_words[80] = 0x2403_fffe;
        load_words[81] = 0x0043_1024;
        load_words[82] = 0xae02_0004;
        load_words[85] = 0x3042_0004;
        load_words[86] = 0x1040_0008;
        load_words[88] = 0x8e02_0038;
        load_words[94] = 0x0220_2021;
        load_words[95] = jal(sp_task_load + 95 * 4, base + 0x1710);
        load_words[96] = 0x2405_0040;
        load_words[97] = jal(sp_task_load + 97 * 4, base + 0x1800);
        load_words[98] = 0x2404_2b00;
        load_words[100] = 0x3c04_0400;
        load_words[101] = jal(sp_task_load + 101 * 4, base + 0x1810);
        load_words[102] = 0x3484_1000;
        load_words[106] = 0x2404_0001;
        load_words[107] = 0x3c05_0400;
        load_words[108] = 0x34a5_0fc0;
        load_words[110] = jal(sp_task_load + 110 * 4, base + 0x1820);
        load_words[111] = 0x2407_0040;
        load_words[114] = jal(sp_task_load + 114 * 4, base + 0x1830);
        load_words[119] = 0x8e26_0008;
        load_words[120] = 0x8e27_000c;
        load_words[121] = 0x3c05_0400;
        load_words[122] = jal(sp_task_load + 122 * 4, base + 0x1820);
        load_words[123] = 0x34a5_1000;
        load_words[129] = 0x03e0_0008;
        load_words[130] = 0x27bd_0020;

        let sp_task_start_go = base + sp_task_start_go_index as u32 * 4;
        words[sp_task_start_go_index..sp_task_start_go_index + 11].copy_from_slice(&[
            0x27bd_ffe8,
            0xafbf_0010,
            jal(sp_task_start_go + 2 * 4, base + 0x1830),
            0,
            0x1440_fffd,
            0,
            jal(sp_task_start_go + 6 * 4, base + 0x1800),
            0x2404_0125,
            0x8fbf_0010,
            0x03e0_0008,
            0x27bd_0018,
        ]);
        let sp_task_yield = base + sp_task_yield_index as u32 * 4;
        words[sp_task_yield_index..sp_task_yield_index + 7].copy_from_slice(&[
            0x27bd_ffe8,
            0xafbf_0010,
            jal(sp_task_yield + 2 * 4, base + 0x1800),
            0x2404_0400,
            0x8fbf_0010,
            0x03e0_0008,
            0x27bd_0018,
        ]);
        let sp_task_yielded = base + sp_task_yielded_index as u32 * 4;
        words[sp_task_yielded_index..sp_task_yielded_index + 19].copy_from_slice(&[
            0x27bd_ffe8,
            0xafb0_0010,
            0xafbf_0014,
            jal(sp_task_yielded + 3 * 4, base + 0x1840),
            0x0080_8021,
            0x0002_2202,
            0x3042_0080,
            0x1040_0006,
            0x3084_0001,
            0x8e02_0004,
            0x2403_fffd,
            0x0044_1025,
            0x0043_1024,
            0xae02_0004,
            0x0080_1021,
            0x8fbf_0014,
            0x8fb0_0010,
            0x03e0_0008,
            0x27bd_0018,
        ]);
        let set_timer = base + set_timer_index as u32 * 4;
        let timer_words = &mut words[set_timer_index..set_timer_index + 75];
        timer_words[0] = 0x27bd_ffe0;
        timer_words[1] = 0x8fa2_0030;
        timer_words[2] = 0x8fa3_0034;
        timer_words[4] = 0x0080_8021;
        timer_words[8] = 0xae00_0000;
        timer_words[9] = 0xae00_0004;
        timer_words[10] = 0xae06_0010;
        timer_words[11] = 0xae07_0014;
        timer_words[12] = 0xae02_0008;
        timer_words[13] = 0xae03_000c;
        timer_words[14] = 0x8fa4_0038;
        timer_words[15] = 0x8fa5_003c;
        timer_words[17] = 0xae04_0018;
        timer_words[19] = 0xae04_0018;
        timer_words[20] = 0xae02_0010;
        timer_words[21] = 0xae03_0014;
        timer_words[22] = 0xae04_0018;
        timer_words[23] = jal(set_timer + 23 * 4, base + 0x1900);
        timer_words[24] = 0xae05_001c;
        timer_words[30] = jal(set_timer + 30 * 4, base + 0x1910);
        timer_words[58] = jal(set_timer + 58 * 4, base + 0x1920);
        timer_words[59] = 0x0200_2021;
        timer_words[64] = jal(set_timer + 64 * 4, base + 0x1930);
        timer_words[66] = jal(set_timer + 66 * 4, base + 0x1940);
        timer_words[68] = 0x0000_1021;
        timer_words[73] = 0x03e0_0008;
        timer_words[74] = 0x27bd_0020;

        let si_device_busy = base + words.len() as u32 * 4;
        words.extend([
            0x3c02_a480,
            0x3442_0018,
            0x8c42_0000,
            0x3042_0003,
            0x03e0_0008,
            0x0002_102b,
        ]);
        let discovered = discover_wm_block_runtime_host_bindings(&words, base).unwrap();
        assert_eq!(
            discovered,
            vec![
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
                    symbol: HostBindingSymbol::OsSiDeviceBusy,
                    vram: si_device_busy,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSetThreadPri,
                    vram: set_thread_pri,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSetTimer,
                    vram: set_timer,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSpTaskLoad,
                    vram: sp_task_load,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSpTaskStartGo,
                    vram: sp_task_start_go,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSpTaskYield,
                    vram: sp_task_yield,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSpTaskYielded,
                    vram: sp_task_yielded,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsStartThread,
                    vram: start_thread,
                },
            ]
        );

        for offset in [
            0usize, 2, 7, 8, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 32,
            34, 35, 39, 41,
        ] {
            let mut broken = words.clone();
            if offset == 39 {
                broken[create_thread_index + offset] = 0;
            } else {
                broken[create_thread_index + offset] ^= 1;
            }
            assert!(
                matches!(
                    discover_overlay_loader_host_bindings(&broken, base),
                    Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                        symbol: HostBindingSymbol::OsCreateThread,
                        candidates,
                    }) if candidates.is_empty()
                ),
                "mutated osCreateThread invariant at word {offset}"
            );
        }

        let mut overwritten_thread_base = words.clone();
        overwritten_thread_base[create_thread_index + 9] = 0x3c13_0000;
        assert!(matches!(
            discover_overlay_loader_host_bindings(&overwritten_thread_base, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsCreateThread,
                candidates,
            }) if candidates.is_empty()
        ));

        let mut duplicate_create_thread = words.clone();
        duplicate_create_thread
            .extend_from_slice(&words[create_thread_index..create_thread_index + 42]);
        assert!(matches!(
            discover_overlay_loader_host_bindings(&duplicate_create_thread, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsCreateThread,
                candidates,
            }) if candidates.len() == 2
        ));
    }

    #[test]
    fn nwxe_rsp_task_roles_are_unique_and_invariant_sensitive_when_rom_is_available() {
        let Some(path) = std::env::var_os("FN64_DISCOVER_NWXE_ROM") else {
            eprintln!("skip: FN64_DISCOVER_NWXE_ROM unset");
            return;
        };
        let source = std::fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "reading FN64_DISCOVER_NWXE_ROM {}: {error}",
                path.to_string_lossy()
            )
        });
        let rom = crate::rom::normalize(&source).expect("normalizing NWXE corpus ROM");
        let base = rom.header.entry_point;
        let words = rom.bytes[0x1000..0x101000]
            .chunks_exact(4)
            .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
            .collect::<Vec<_>>();
        let expected = vec![
            HostBinding {
                symbol: HostBindingSymbol::OsSpTaskLoad,
                vram: 0x8003_1cc0,
            },
            HostBinding {
                symbol: HostBindingSymbol::OsSpTaskStartGo,
                vram: 0x8003_1ecc,
            },
            HostBinding {
                symbol: HostBindingSymbol::OsSpTaskYield,
                vram: 0x8003_1f00,
            },
            HostBinding {
                symbol: HostBindingSymbol::OsSpTaskYielded,
                vram: 0x8003_1f20,
            },
        ];
        let load_index = usize::try_from((expected[0].vram - base) / 4).unwrap();
        let load_words = &words[load_index..load_index + 131];
        let load_groups = [
            (
                "copy/yield prefix",
                is_addiu(load_words[0], 29, 29, imm(load_words[0]))
                    && is_move_addu(load_words[2], 16, 4)
                    && is_move_addu(load_words[6], 5, 17)
                    && jal_field(load_words[8]).is_some()
                    && is_addiu(load_words[9], 6, 0, 0x40)
                    && is_andi(load_words[68], 2, 2, 1)
                    && is_beq(load_words[69], 2, 0),
            ),
            (
                "task-header/cache prefix",
                is_lw_at(load_words[79], 2, 16, 4)
                    && is_addiu(load_words[80], 3, 0, -2)
                    && is_sw(load_words[82], 2, 16, 4)
                    && is_andi(load_words[85], 2, 2, 4)
                    && is_beq(load_words[86], 2, 0)
                    && is_lw_at(load_words[88], 2, 16, 0x38)
                    && is_move_addu(load_words[94], 4, 17)
                    && jal_field(load_words[95]).is_some()
                    && is_addiu(load_words[96], 5, 0, 0x40),
            ),
            (
                "status/task DMA",
                jal_field(load_words[97]).is_some()
                    && is_addiu(load_words[98], 4, 0, 0x2b00)
                    && is_lui(load_words[100], 4)
                    && load_words[100] as u16 == 0x0400
                    && jal_field(load_words[101]).is_some()
                    && is_addiu(load_words[106], 4, 0, 1)
                    && is_lui(load_words[107], 5)
                    && load_words[107] as u16 == 0x0400
                    && jal_field(load_words[110]).is_some()
                    && is_addiu(load_words[111], 7, 0, 0x40)
                    && jal_field(load_words[114]).is_some(),
            ),
            (
                "rspboot DMA/epilogue",
                is_lw_at(load_words[119], 6, 17, 8)
                    && is_lw_at(load_words[120], 7, 17, 12)
                    && is_lui(load_words[121], 5)
                    && load_words[121] as u16 == 0x0400
                    && jal_field(load_words[122]) == jal_field(load_words[110])
                    && load_words[123] as u16 == 0x1000
                    && is_jr_ra(load_words[129])
                    && is_addiu(load_words[130], 29, 29, -imm(load_words[0])),
            ),
        ];
        for (group, matched) in load_groups {
            assert!(matched, "NWXE osSpTaskLoad {group} did not match");
        }
        assert!(
            is_sp_task_load(load_words),
            "expected NWXE load body was not structurally recognized"
        );
        assert_eq!(
            discover_rsp_task_host_bindings(&words, base).unwrap(),
            expected
        );
        let expected_timer = HostBinding {
            symbol: HostBindingSymbol::OsSetTimer,
            vram: 0x8003_2600,
        };
        assert_eq!(
            discover_timer_host_bindings(&words, base).unwrap(),
            vec![expected_timer]
        );

        let start_index = usize::try_from((expected[1].vram - base) / 4).unwrap();
        for offset in [68usize, 96, 98, 108, 111, 122, 123] {
            let mut broken = words.clone();
            broken[load_index + offset] ^= 1;
            assert!(matches!(
                discover_rsp_task_host_bindings(&broken, base),
                Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                    symbol: HostBindingSymbol::OsSpTaskLoad,
                    candidates,
                }) if candidates.is_empty()
            ));
        }
        for offset in [2usize, 4, 6, 7] {
            let mut broken = words.clone();
            broken[start_index + offset] ^= 1;
            assert!(matches!(
                discover_rsp_task_host_bindings(&broken, base),
                Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                    symbol: HostBindingSymbol::OsSpTaskStartGo,
                    candidates,
                }) if candidates.is_empty()
            ));
        }

        let mut duplicate_load = words.clone();
        duplicate_load.extend_from_slice(&words[load_index..load_index + 131]);
        assert!(matches!(
            discover_rsp_task_host_bindings(&duplicate_load, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsSpTaskLoad,
                candidates,
            }) if candidates.len() == 2
        ));

        let mut duplicate_start = words.clone();
        duplicate_start.extend_from_slice(&words[start_index..start_index + 11]);
        assert!(matches!(
            discover_rsp_task_host_bindings(&duplicate_start, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsSpTaskStartGo,
                candidates,
            }) if candidates.len() == 2
        ));

        let timer_index = usize::try_from((expected_timer.vram - base) / 4).unwrap();
        for offset in [1usize, 10, 14, 24, 59, 68, 73] {
            let mut broken = words.clone();
            broken[timer_index + offset] ^= 1;
            assert!(matches!(
                discover_timer_host_bindings(&broken, base),
                Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                    symbol: HostBindingSymbol::OsSetTimer,
                    candidates,
                }) if candidates.is_empty()
            ));
        }
        let mut duplicate_timer = words.clone();
        duplicate_timer.extend_from_slice(&words[timer_index..timer_index + 75]);
        assert!(matches!(
            discover_timer_host_bindings(&duplicate_timer, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsSetTimer,
                candidates,
            }) if candidates.len() == 2
        ));
    }

    #[test]
    fn running_thread_global_is_triangulated_from_relocated_priority_routines() {
        let resident_vram = 0x8040_0000;
        let running_thread_vram = 0x8040_ff20;
        let mut resident = vec![0u32; 64];
        resident[4..10].copy_from_slice(&[
            0x1480_0002,
            0,
            0x3c04_8041,
            0x8c84_ff20,
            0x03e0_0008,
            0x8c82_0004,
        ]);
        resident[20..40].copy_from_slice(&[
            0x27bd_ffe8,
            0,
            0x0080_8021,
            0,
            0x00a0_8821,
            0,
            0x0c00_1234,
            0,
            0x1600_0002,
            0x0040_9021,
            0x3c10_8041,
            0x8e10_ff20,
            0x8e02_0004,
            0x1051_0002,
            0,
            0x3c02_8042,
            0x8c42_1000,
            0x1202_0002,
            0xae11_0004,
            0,
        ]);

        assert_eq!(
            discover_guest_thread_globals(&resident, resident_vram).unwrap(),
            GuestThreadGlobals {
                running_thread_vram
            }
        );

        resident[31] = 0x8e10_1234;
        assert!(matches!(
            discover_guest_thread_globals(&resident, resident_vram),
            Err(
                HostBindingDiscoveryError::InconsistentRunningThreadGlobals {
                    get_thread_pri: 0x8040_ff20,
                    set_thread_pri: 0x8041_1234,
                }
            )
        ));
    }
}
