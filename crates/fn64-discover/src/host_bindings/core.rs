//! Shared decode primitives, MIPS instruction-field extraction, and the
//! generic structural-match helpers every recognizer in sibling modules
//! calls into. Also the public host-binding vocabulary (`HostBindingSymbol`,
//! `HostBinding`, the discovery error type) that both this module and its
//! external callers name.

use super::thread::{is_get_thread_pri, is_set_thread_pri};
use crate::cfg::{Cfg, WordClass};
use std::collections::BTreeSet;

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
    /// `osEPiWriteIo(OSPiHandle *, u32 devAddr, u32 data)`.
    OsEPiWriteIo,
    /// `osEPiReadIo(OSPiHandle *, u32 devAddr, u32 *data)`.
    OsEPiReadIo,
    /// `osFlashInit(void) -> OSPiHandle *`.
    OsFlashInit,
    /// `osFlashSectorErase(u32 page_num) -> s32`.
    OsFlashSectorErase,
    /// `osFlashReadArray(OSIoMesg *, s32, u32, void *, u32, OSMesgQueue *)`.
    OsFlashReadArray,
}

/// The programmed-IO roles, discovered separately from
/// [`WM_BLOCK_RUNTIME_HOST_SYMBOLS`] and deliberately NOT part of it.
///
/// These are optional by construction. A title that saves to SRAM reaches its
/// save device entirely through PI DMA and never links these routines, so
/// requiring them would fail discovery for the three titles that already
/// resolve 15/15. A title that saves to FlashRAM issues its commands through
/// them, and leaving them unbound means the guest's own recompiled copy drives
/// raw hardware -- which is the No Mercy fault at pc `0x8003d518`, a `sw` into
/// the FlashRAM command window at `0xA801_0000`.
pub const PROGRAMMED_IO_HOST_SYMBOLS: [HostBindingSymbol; 2] = [
    HostBindingSymbol::OsEPiReadIo,
    HostBindingSymbol::OsEPiWriteIo,
];

/// The FlashRAM API roles, discovered only for a title that links them.
///
/// Binding these keeps the guest's own flash driver from executing at all, so
/// fn64's existing `osFlash*` modelling carries the protocol and nothing has to
/// decode the command register. That is the whole reason this seam is preferred
/// over teaching the PI layer device type 8: `PiDeviceAddress` carries only a
/// byte offset for domain 2, so a command word routed through it would be
/// written into the save image rather than interpreted.
pub const FLASH_HOST_SYMBOLS: [HostBindingSymbol; 3] = [
    HostBindingSymbol::OsFlashInit,
    HostBindingSymbol::OsFlashSectorErase,
    HostBindingSymbol::OsFlashReadArray,
];

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
    /// The generated `osCreateThread` saved SR is restored through ERET before
    /// the child runs. Its active form supplies interrupt controls and clears
    /// BEV rather than inheriting bootstrap mode/vector fields.
    GeneratedSavedSrPostEretClearsBev,
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
            | Self::OsStartThread
            | Self::OsEPiWriteIo
            | Self::OsEPiReadIo
            | Self::OsFlashInit
            | Self::OsFlashSectorErase
            | Self::OsFlashReadArray => HostCurrentStatusEffect::CBridgeRuntimeEnforcedPreservesBev,
        }
    }

    pub fn spawned_status_effect(self) -> HostSpawnedStatusEffect {
        match self {
            Self::OsCreateThread => HostSpawnedStatusEffect::GeneratedSavedSrPostEretClearsBev,
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
            | Self::OsStartThread
            | Self::OsEPiWriteIo
            | Self::OsEPiReadIo
            | Self::OsFlashInit
            | Self::OsFlashSectorErase
            | Self::OsFlashReadArray => HostSpawnedStatusEffect::None,
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
    /// An external reference table names an address for a symbol that a
    /// recognizer *also* resolved, and the two disagree. This is a hard error,
    /// never a silent preference: either the table is for a different ROM or the
    /// recognizer is wrong, and both must be investigated rather than papered
    /// over by choosing one.
    ExternalReferenceDisagreement {
        symbol: HostBindingSymbol,
        recognizer: u32,
        external: u32,
    },
    /// An external reference table names an address for a symbol that no
    /// recognizer resolved, and disassembling the routine at that address does
    /// not exhibit the shape that symbol's recognizer requires. The address is
    /// rejected rather than bound: an external table is validated, never
    /// trusted.
    ExternalReferenceShapeMismatch {
        symbol: HostBindingSymbol,
        external: u32,
    },
    /// An external reference table names an address for a *derived* symbol whose
    /// shape can only be validated once its prerequisite recognizer resolves,
    /// but that prerequisite did not resolve. The external address cannot be
    /// shape-validated, so it is not bound (rather than trusted blindly).
    ExternalReferenceUnvalidatable {
        symbol: HostBindingSymbol,
        external: u32,
        needs: &'static str,
    },
    /// An external reference table names an address that lies outside the
    /// resident image `[va_start, va_start + 4*len)`, or is misaligned, so no
    /// routine can be disassembled there for shape validation.
    ExternalReferenceOutOfRange {
        symbol: HostBindingSymbol,
        external: u32,
    },
}

pub(super) fn op(word: u32) -> u32 {
    word >> 26
}

pub(super) fn rs(word: u32) -> u32 {
    word >> 21 & 31
}

pub(super) fn rt(word: u32) -> u32 {
    word >> 16 & 31
}

pub(super) fn rd(word: u32) -> u32 {
    word >> 11 & 31
}

pub(super) fn imm(word: u32) -> i16 {
    word as u16 as i16
}

pub(super) fn is_lui(word: u32, target: u32) -> bool {
    op(word) == 0x0f && rs(word) == 0 && rt(word) == target
}

pub(super) fn is_addiu(word: u32, target: u32, source: u32, immediate: i16) -> bool {
    op(word) == 0x09 && rt(word) == target && rs(word) == source && imm(word) == immediate
}

pub(super) fn is_lw(word: u32, target: u32, base: u32) -> bool {
    op(word) == 0x23 && rt(word) == target && rs(word) == base
}

pub(super) fn is_lw_at(word: u32, target: u32, base: u32, offset: i16) -> bool {
    is_lw(word, target, base) && imm(word) == offset
}

pub(super) fn is_sw(word: u32, source: u32, base: u32, offset: i16) -> bool {
    op(word) == 0x2b && rt(word) == source && rs(word) == base && imm(word) == offset
}

pub(super) fn absolute_from_lui_offset(lui: u32, offset: i16) -> u32 {
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

pub(super) fn is_sh(word: u32, source: u32, base: u32, offset: i16) -> bool {
    op(word) == 0x29 && rt(word) == source && rs(word) == base && imm(word) == offset
}

pub(super) fn is_move_addu(word: u32, target: u32, source: u32) -> bool {
    op(word) == 0
        && word & 0x3f == 0x21
        && rd(word) == target
        && ((rs(word) == source && rt(word) == 0) || (rt(word) == source && rs(word) == 0))
}

pub(super) fn is_jr_ra(word: u32) -> bool {
    op(word) == 0 && word & 0x3f == 8 && rs(word) == 31
}

pub(super) fn is_bne(word: u32, left: u32, right: u32) -> bool {
    op(word) == 5 && rs(word) == left && rt(word) == right
}

pub(super) fn is_beq(word: u32, left: u32, right: u32) -> bool {
    op(word) == 4 && rs(word) == left && rt(word) == right
}

pub(super) fn is_andi(word: u32, target: u32, source: u32, immediate: u16) -> bool {
    op(word) == 0x0c && rt(word) == target && rs(word) == source && word as u16 == immediate
}

pub(super) fn jal_target(word: u32, pc: u32) -> Option<u32> {
    (op(word) == 3).then_some(((pc.wrapping_add(4)) & 0xf000_0000) | ((word & 0x03ff_ffff) << 2))
}

pub(super) fn jal_field(word: u32) -> Option<u32> {
    (op(word) == 3).then_some(word & 0x03ff_ffff)
}

pub(super) fn proven_code_interval(
    cfg: &Cfg,
    executable_ranges: &[(u32, u32)],
    start: u32,
    end: u32,
) -> bool {
    start < end
        && executable_ranges
            .iter()
            .copied()
            .any(|(range_start, range_end)| start >= range_start && end <= range_end)
        && (start..end)
            .step_by(4)
            .all(|pc| cfg.word_class.get(&pc) == Some(&WordClass::ProvenCode))
}

pub(super) fn authoritative_root(
    cfg: &Cfg,
    cfg_roots: &BTreeSet<u32>,
    proven_entries: &BTreeSet<u32>,
    root: u32,
) -> bool {
    cfg_roots.contains(&root)
        && proven_entries.contains(&root)
        && cfg.word_class.get(&root) == Some(&WordClass::ProvenCode)
}

pub(super) fn image_words<'a>(
    words: &'a [u32],
    va_start: u32,
    start: u32,
    count: usize,
) -> Option<&'a [u32]> {
    let byte_offset = start.checked_sub(va_start)?;
    if !byte_offset.is_multiple_of(4) {
        return None;
    }
    let first = usize::try_from(byte_offset / 4).ok()?;
    words.get(first..first.checked_add(count)?)
}

pub(super) fn is_store_opcode(opcode: u32) -> bool {
    matches!(opcode, 0x28..=0x2e | 0x38..=0x3f)
}

pub(super) fn unique_match(
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
    let candidates = collapse_overlapping_runs(&candidates);
    match candidates.as_slice() {
        [address] => Ok(*address),
        _ => Err(HostBindingDiscoveryError::NonUniqueSemanticMatch { symbol, candidates }),
    }
}

/// Collapse a run of consecutive word addresses to its last address.
///
/// An order-free predicate over a window wider than the routine also matches
/// when the window merely *contains* the routine, so one routine matches at
/// several adjacent start offsets. Those are one candidate reported several
/// times, not several routines, and counting them separately would report a
/// correct predicate as ambiguous.
///
/// The run's last address is the reported one because that is the latest start
/// whose window still satisfies the predicate, which is the routine's own
/// entry; earlier starts only match by including preceding filler. Callers
/// resolve `jal` targets against this address, so returning a run's first
/// address would name an instruction inside the caller's padding instead of
/// the function entry. Only strictly adjacent (4-byte apart) addresses are
/// collapsed: two genuinely distinct routines are never adjacent at word
/// granularity, so this cannot merge real duplicates.
pub(super) fn collapse_overlapping_runs(sorted: &[u32]) -> Vec<u32> {
    let mut collapsed: Vec<u32> = Vec::new();
    for &address in sorted {
        if collapsed.last() == Some(&address.wrapping_sub(4)) {
            // Extend the current run: the entry is its latest start.
            *collapsed.last_mut().expect("run has a first address") = address;
        } else {
            collapsed.push(address);
        }
    }
    collapsed
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallValue {
    Unknown,
    Constant(u32),
    Stack(i32),
}

/// Resolve one call argument backwards through register moves and address
/// construction.  This is deliberately bounded to the containing routine;
/// crossing a call that clobbers a caller-saved register yields `Unknown`.
pub(super) fn resolve_call_value(
    words: &[u32],
    lower: usize,
    before: usize,
    register: u32,
    depth: usize,
) -> CallValue {
    if depth > 12 || register == 0 {
        return CallValue::Constant(0);
    }
    for index in (lower..before).rev() {
        let word = words[index];
        if jal_field(word).is_some() && matches!(register, 1..=15 | 24 | 25 | 31) {
            return CallValue::Unknown;
        }
        match op(word) {
            0x09 if rt(word) == register => {
                let delta = i32::from(imm(word));
                if rs(word) == 29 {
                    return CallValue::Stack(delta);
                }
                if rs(word) == 0 {
                    return CallValue::Constant(delta as u32);
                }
                return match resolve_call_value(words, lower, index, rs(word), depth + 1) {
                    CallValue::Constant(value) => {
                        CallValue::Constant(value.wrapping_add(delta as u32))
                    }
                    CallValue::Stack(value) => CallValue::Stack(value + delta),
                    CallValue::Unknown => CallValue::Unknown,
                };
            }
            0x0d if rt(word) == register => {
                let low = u32::from(word as u16);
                return match resolve_call_value(words, lower, index, rs(word), depth + 1) {
                    CallValue::Constant(value) => CallValue::Constant(value | low),
                    _ => CallValue::Unknown,
                };
            }
            0x0f if rt(word) == register => return CallValue::Constant((word & 0xffff) << 16),
            0 if rd(word) == register && matches!(word & 0x3f, 0x20 | 0x21 | 0x25) => {
                if rt(word) == 0 {
                    return resolve_call_value(words, lower, index, rs(word), depth + 1);
                }
                if rs(word) == 0 {
                    return resolve_call_value(words, lower, index, rt(word), depth + 1);
                }
                return CallValue::Unknown;
            }
            opcode
                if rt(word) == register
                    && matches!(opcode, 0x08 | 0x0a..=0x0c | 0x0e | 0x20..=0x27 | 0x30..=0x37) =>
            {
                return CallValue::Unknown
            }
            _ => {}
        }
    }
    CallValue::Unknown
}
