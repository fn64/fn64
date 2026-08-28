//! Mechanical host-binding discovery for public resident libultra routines.
//!
//! Addresses are outputs, never signatures. The recognizers below describe
//! public ABI behavior in register/field terms: `osCreateMesgQueue` initializes
//! the documented six-word queue, `osCreateThread` initializes the public
//! `OSThread` linkage, identity, state, context, and o32 stack-supplied
//! priority fields, `osEPiStartDma` validates the manager and writes the
//! request type/handle into `OSIoMesg` -- recognized by that ABI behavior so
//! that builds keeping the arguments register-resident and builds spilling
//! every argument to the frame are both matched, `osSendMesg` inserts at `(first +
//! validCount) % msgCount`, and the overlay helper calls the DMA routine in a
//! retry loop before a blocking receive on its stack queue.
//! The RSP task recognizers likewise follow the public task-load, start, yield,
//! and yielded-query register/field behavior. `osSetEventMesg` scales the event
//! selector by the documented eight-byte `OSEventState` stride and stores the
//! queue and message through the resulting entry, between an interrupt disable
//! and its matching restore. Timer discovery follows the public o32
//! `osSetTimer` arguments and `OSTimer` fields, resolving the stack-passed
//! arguments relative to the callee's own frame so that builds which inline the
//! list walk and builds which delegate it are both recognized. Every role must
//! have one unique structural match or discovery fails loudly.

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
pub const PROGRAMMED_IO_HOST_SYMBOLS: [HostBindingSymbol; 2] =
    [HostBindingSymbol::OsEPiReadIo, HostBindingSymbol::OsEPiWriteIo];

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
            | Self::OsFlashReadArray => {
                HostCurrentStatusEffect::CBridgeRuntimeEnforcedPreservesBev
            }
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

/// `osCreateMesgQueue(mq, msg, count)` initializes the documented six-word
/// `OSMesgQueue` through `$a0`: both thread queues are set to the same
/// "no waiting thread" sentinel, `validCount` and `first` are zeroed, and the
/// caller's count and buffer are stored.
///
/// Which register carries the sentinel, and whether the compiler materializes
/// it once or once per store, is a register-allocation artifact rather than
/// ABI behavior: the 1996-era build loads it into two registers where the
/// 1998-era build reuses one. Pinning `$v0` and a fixed store order therefore
/// described a particular compilation instead of the documented behavior, so
/// the sentinel is identified by the address it computes and the store order
/// is left free. Requiring both queue heads to receive the *same* computed
/// address is what keeps this a queue-initializer predicate rather than
/// "any six stores through `$a0`".
fn is_create_mesg_queue(words: &[u32]) -> bool {
    if words.len() < 9 {
        return false;
    }
    let stored_to_queue = |offset: i16, source: u32| {
        words
            .iter()
            .any(|&word| is_sw(word, source, 4, offset))
    };
    // validCount and first are zeroed; msgCount and msg come from the o32
    // third and second arguments.
    if !(stored_to_queue(8, 0)
        && stored_to_queue(12, 0)
        && stored_to_queue(16, 6)
        && stored_to_queue(20, 5))
    {
        return false;
    }
    if !words.iter().any(|&word| is_jr_ra(word)) {
        return false;
    }
    let queue_head_source = |offset: i16| {
        words
            .iter()
            .find(|&&word| op(word) == 0x2b && rs(word) == 4 && imm(word) == offset)
            .map(|&word| rt(word))
    };
    let (Some(mtqueue), Some(fullqueue)) = (queue_head_source(0), queue_head_source(4)) else {
        return false;
    };
    // Fold the lui/addiu pair that forms each queue head's value so the two
    // are compared by the address they denote, not by register identity.
    let sentinel = |register: u32| {
        let high = words.iter().find(|&&word| is_lui(word, register))?;
        let low = words
            .iter()
            .find(|&&word| op(word) == 9 && rt(word) == register && rs(word) == register)?;
        Some(absolute_from_lui_offset(*high, imm(*low)))
    };
    match (sentinel(mtqueue), sentinel(fullqueue)) {
        (Some(first), Some(second)) => first == second,
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EpiStartDmaValue {
    Unknown,
    /// The `OSPiHandle *`, o32 argument one (`$a0`).
    Handle,
    /// The `OSIoMesg *`, o32 argument two (`$a1`).
    Message,
    /// The transfer direction, o32 argument three (`$a2`).
    Direction,
}

/// Recognize the public `osEPiStartDma(OSPiHandle *, OSIoMesg *, s32 direction)`.
///
/// Every clause below is a published-ABI property of the routine rather than a
/// property of any particular compilation of it:
///
/// * it is a routine with a stack frame it establishes on entry and gives back
///   before `jr $ra`;
/// * it can fail before touching the message, returning the `-1` sentinel the
///   public API documents when the PI manager is not yet initialized;
/// * it stamps the request into the `OSIoMesg` header at offset zero, and that
///   header value is one of the two documented request kinds selected by the
///   direction argument -- the constant `15` on the read branch and `16` on the
///   write branch, each written with a half-word store through the message
///   pointer;
/// * the direction argument gates which of those two constants is stored; and
/// * it records the caller's handle/device address into the message.
///
/// No register assignment, instruction schedule, manager-global address or
/// frame size is pinned. The 1998-era build that keeps the message pointer and
/// arguments resident in callee-saved registers and the 1997-era build (WCW/nWo
/// World Tour, `osEPiStartDma` at VRAM 0x80011E20 / ROM 0x12A20) that spills
/// every argument to its stack frame and reloads it are both accepted, because
/// the difference is register allocation, not ABI. The two type constants both
/// stored at message offset zero, gated by the direction test and paired with
/// the `-1` guard-failure return, are what keep this an `osEPiStartDma`
/// predicate rather than "any routine with a frame".
fn is_epi_start_dma(words: &[u32]) -> bool {
    use EpiStartDmaValue as V;

    // Anchor the window to the routine's own entry so a wider-than-body window
    // does not also match starting one instruction inside the prologue. The
    // frame is established in the first few words; the manager-guard pointer is
    // loaded from a static global and branched on. Two register-allocation
    // layouts occur: the register-resident build loads the guard (`lui`/`lw`)
    // *before* the stack adjust, so the entry word is that `lui`; the
    // stack-spilling build (World Tour) adjusts the stack first, so the entry
    // word is the `addiu sp`. Requiring the guard load to sit after the stack
    // adjust in the World Tour arm rejects windows that begin mid-prologue,
    // where the guard load has already scrolled off the window's head.
    let lui_lw_guard = |lui_index: usize| {
        let Some(&lui) = words.get(lui_index) else {
            return false;
        };
        if op(lui) != 0x0f {
            return false;
        }
        let guard_reg = rt(lui);
        words
            .get(lui_index + 1)
            .is_some_and(|&lw| is_lw(lw, guard_reg, guard_reg))
    };
    let entry_anchored = if is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0 {
        // World Tour arm: stack adjust first, guard load somewhere after it.
        (1..words.len().min(6)).any(lui_lw_guard)
    } else {
        // Register-resident arm: guard load first, stack adjust within reach.
        lui_lw_guard(0)
            && words
                .iter()
                .take(4)
                .any(|&word| is_addiu(word, 29, 29, imm(word)) && imm(word) < 0)
    };
    if !entry_anchored {
        return false;
    }
    // The documented guard-failure sentinel, `return -1`.
    if !words
        .iter()
        .any(|&word| op(word) == 0x09 && rs(word) == 0 && imm(word) == -1)
    {
        return false;
    }

    let mut registers = [V::Unknown; 32];
    registers[4] = V::Handle;
    registers[5] = V::Message;
    registers[6] = V::Direction;
    let mut spill: BTreeMap<i16, EpiStartDmaValue> = BTreeMap::new();
    // Whether each register currently holds one of the two request-type
    // constants (`true` marks a 15/16 literal live in that register), so a
    // half-word store can be attributed to the type stamp regardless of which
    // register the compiler chose.
    let mut const_type: [bool; 32] = [false; 32];
    let mut const_15_seen = false;
    let mut const_16_seen = false;
    let mut stored_type_header = false;
    let mut stored_dev_addr = false;
    let mut direction_tested = false;

    for &word in words {
        let opcode = op(word);
        // The two documented request-type constants materialized into a
        // register. Both must be formed somewhere in the body; a build with a
        // shared convergence register overwrites the tag but each literal is
        // still observed here, and a build that keeps each on its own branch
        // tags two registers.
        if op(word) == 0x09 && rs(word) == 0 && (imm(word) == 15 || imm(word) == 16) {
            if imm(word) == 15 {
                const_15_seen = true;
            } else {
                const_16_seen = true;
            }
            const_type[rt(word) as usize] = true;
            registers[rt(word) as usize] = V::Unknown;
            continue;
        }
        match opcode {
            // Branch-on-register: the direction argument gates the type stored.
            0x04 | 0x05 => {
                if registers[rs(word) as usize] == V::Direction
                    || registers[rt(word) as usize] == V::Direction
                {
                    direction_tested = true;
                }
            }
            // sw: an argument spill, or the handle/device address into the
            // message header.
            0x2b => {
                let source = registers[rt(word) as usize];
                if rs(word) == 29 {
                    spill.insert(imm(word), source);
                } else if registers[rs(word) as usize] == V::Message
                    && imm(word) == 0x14
                    && source == V::Handle
                {
                    stored_dev_addr = true;
                }
            }
            // sh: the request type stamped into the message header at offset 0.
            0x29 => {
                if registers[rs(word) as usize] == V::Message
                    && imm(word) == 0
                    && const_type[rt(word) as usize]
                {
                    stored_type_header = true;
                }
            }
            // lw: reloading a spilled argument restores its tag.
            0x23 => {
                if rt(word) != 0 {
                    registers[rt(word) as usize] = if rs(word) == 29 {
                        spill.get(&imm(word)).copied().unwrap_or(V::Unknown)
                    } else {
                        V::Unknown
                    };
                    const_type[rt(word) as usize] = false;
                }
            }
            // move/addu: propagate an argument tag across a register copy.
            0x00 if word & 0x3f == 0x21 => {
                let source = if rt(word) == 0 {
                    registers[rs(word) as usize]
                } else if rs(word) == 0 {
                    registers[rt(word) as usize]
                } else {
                    V::Unknown
                };
                if rd(word) != 0 {
                    registers[rd(word) as usize] = source;
                    const_type[rd(word) as usize] = false;
                }
            }
            _ => {
                // Any other write to a general register clears its argument tag.
                let dst = match opcode {
                    0x08 | 0x09 | 0x0a | 0x0b | 0x0c | 0x0d | 0x0e | 0x0f | 0x20 | 0x21 | 0x24
                    | 0x25 | 0x30..=0x37 => rt(word),
                    0x00 => rd(word),
                    _ => continue,
                };
                if dst != 0 {
                    registers[dst as usize] = V::Unknown;
                    const_type[dst as usize] = false;
                }
            }
        }
    }

    const_15_seen && const_16_seen && stored_type_header && stored_dev_addr && direction_tested
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
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum V {
        Unknown,
        Queue,
        Mesg,
        Block,
        Valid,
        Count,
        First,
        Buffer,
        Sum,
        Index,
        ByteIndex,
        Slot,
        SendWait,
        RecvWait,
        SavedMask,
    }

    if words.len() < 57 || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let frame = imm(words[0]);
    let mut regs = [V::Unknown; 32];
    regs[4] = V::Queue;
    regs[5] = V::Mesg;
    regs[6] = V::Block;
    let mut spill = BTreeMap::new();
    let mut hi = V::Unknown;
    let mut calls = 0;
    let mut capacity_check = false;
    let mut blocked_on_send = false;
    let mut stored_mesg = false;
    let mut incremented_valid = false;
    let mut woke_receiver = false;
    let mut saw_receiver_wait = false;
    let mut restored_mask = false;
    let mut restored_frame = false;
    let mut returned = false;

    let mut index = 1;
    while index < words.len() {
        let word = words[index];
        if is_jr_ra(word) {
            if let Some(&slot) = words.get(index + 1) {
                restored_frame |= is_addiu(slot, 29, 29, frame.wrapping_neg());
            }
            returned = true;
            break;
        }
        if is_addiu(word, 29, 29, frame.wrapping_neg()) {
            restored_frame = true;
        }
        if jal_field(word).is_some() {
            if let Some(&slot) = words.get(index + 1) {
                // Delay-slot moves are inputs to the call.
                if op(slot) == 0 && matches!(slot & 0x3f, 0x21 | 0x25) {
                    let src = if rt(slot) == 0 { rs(slot) } else if rs(slot) == 0 { rt(slot) } else { 0 };
                    if rd(slot) != 0 {
                        regs[rd(slot) as usize] = regs[src as usize];
                    }
                } else if is_addiu(slot, 4, 4, imm(slot)) && regs[4] == V::Queue {
                    regs[4] = match imm(slot) { 0 => V::RecvWait, 4 => V::SendWait, _ => V::Unknown };
                }
            }
            if calls > 0 && regs[4] == V::SavedMask {
                restored_mask = true;
            }
            blocked_on_send |= regs[4] == V::SendWait;
            woke_receiver |= saw_receiver_wait && matches!(regs[4], V::Queue | V::RecvWait);
            calls += 1;
            for caller_saved in (1usize..16).chain([24usize, 25, 31]) {
                regs[caller_saved] = V::Unknown;
            }
            regs[2] = if calls == 1 { V::SavedMask } else { V::Unknown };
            index += 2;
            continue;
        }
        match op(word) {
            0x23 => {
                let value = if rs(word) == 29 {
                    spill.get(&imm(word)).copied().unwrap_or(V::Unknown)
                } else if regs[rs(word) as usize] == V::Queue {
                    match imm(word) { 0 => V::RecvWait, 4 => V::SendWait, 8 => V::Valid, 12 => V::First, 16 => V::Count, 20 => V::Buffer, _ => V::Unknown }
                } else if matches!(regs[rs(word) as usize], V::RecvWait | V::SendWait) && imm(word) == 0 {
                    regs[rs(word) as usize]
                } else {
                    V::Unknown
                };
                saw_receiver_wait |= value == V::RecvWait;
                if rt(word) != 0 { regs[rt(word) as usize] = value; }
            }
            0x2b => {
                let value = regs[rt(word) as usize];
                if rs(word) == 29 {
                    spill.insert(imm(word), value);
                } else if regs[rs(word) as usize] == V::Slot && imm(word) == 0 && value == V::Mesg {
                    stored_mesg = true;
                } else if regs[rs(word) as usize] == V::Queue && imm(word) == 8 && value == V::Valid {
                    incremented_valid = true;
                }
            }
            0x09 => {
                let source = regs[rs(word) as usize];
                regs[rt(word) as usize] = match (source, imm(word)) {
                    (V::Queue, 0) => V::RecvWait,
                    (V::Queue, 4) => V::SendWait,
                    (V::Valid, 1) => V::Valid,
                    _ => V::Unknown,
                };
            }
            0 => {
                let funct = word & 0x3f;
                match funct {
                    0x20 | 0x21 | 0x25 => {
                        let left = regs[rs(word) as usize];
                        let right = regs[rt(word) as usize];
                        regs[rd(word) as usize] = if rt(word) == 0 { left } else if rs(word) == 0 { right } else if matches!((left, right), (V::First, V::Valid) | (V::Valid, V::First)) { V::Sum } else if matches!((left, right), (V::Buffer, V::ByteIndex) | (V::ByteIndex, V::Buffer)) { V::Slot } else { V::Unknown };
                    }
                    0x2a | 0x2b => {
                        let left = regs[rs(word) as usize];
                        let right = regs[rt(word) as usize];
                        capacity_check |= left == V::Valid && right == V::Count;
                        regs[rd(word) as usize] = V::Unknown;
                    }
                    0x1a | 0x1b => {
                        hi = if regs[rs(word) as usize] == V::Sum && regs[rt(word) as usize] == V::Count { V::Index } else { V::Unknown };
                    }
                    0x10 => regs[rd(word) as usize] = hi,
                    0x00 => regs[rd(word) as usize] = if regs[rt(word) as usize] == V::Index && (word >> 6 & 31) == 2 { V::ByteIndex } else { V::Unknown },
                    0x08 | 0x09 => {}
                    _ => if rd(word) != 0 { regs[rd(word) as usize] = V::Unknown; },
                }
            }
            0x02..=0x07 | 0x14..=0x17 | 0x01 => {}
            _ => if rt(word) != 0 && !is_store_opcode(op(word)) { regs[rt(word) as usize] = V::Unknown; },
        }
        index += 1;
    }

    (returned && restored_frame && calls >= 3 && capacity_check && blocked_on_send
        && stored_mesg && incremented_valid && woke_receiver && restored_mask)
        || is_send_mesg_register_resident(words)
}

fn is_send_mesg_register_resident(words: &[u32]) -> bool {
    words.len() >= 57
        && is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0
        && is_move_addu(words[2], 16, 4) && is_move_addu(words[4], 21, 5)
        && is_move_addu(words[6], 18, 6) && jal_field(words[10]).is_some()
        && is_lw_at(words[12], 3, 16, 8) && is_lw_at(words[13], 4, 16, 16)
        && op(words[14]) == 0 && words[14] & 0x3f == 0x2a && rd(words[14]) == 3
        && rs(words[14]) == 3 && rt(words[14]) == 4
        && is_lw_at(words[34], 3, 16, 12) && is_lw_at(words[35], 4, 16, 8)
        && is_lw_at(words[36], 2, 16, 16) && op(words[37]) == 0
        && words[37] & 0x3f == 0x21 && rd(words[37]) == 3 && rs(words[37]) == 3
        && rt(words[37]) == 4 && op(words[38]) == 0 && words[38] & 0x3f == 0x1a
        && rs(words[38]) == 3 && rt(words[38]) == 2 && op(words[48]) == 0
        && words[48] & 0x3f == 0x10 && rd(words[48]) == 2
        && is_lw_at(words[49], 3, 16, 20) && op(words[50]) == 0
        && words[50] & 0x3f == 0 && rt(words[50]) == 2 && rd(words[50]) == 2
        && (words[50] >> 6 & 31) == 2 && op(words[51]) == 0
        && words[51] & 0x3f == 0x21 && rd(words[51]) == 2 && rs(words[51]) == 2
        && rt(words[51]) == 3 && is_sw(words[52], 21, 2, 0)
        && is_lw_at(words[53], 2, 16, 8) && is_addiu(words[55], 2, 2, 1)
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum SetEventMesgValue {
    Unknown,
    /// The `OSEvent` selector, o32 argument one.
    Event,
    /// The `OSMesgQueue *`, o32 argument two.
    Queue,
    /// The `OSMesg`, o32 argument three.
    Mesg,
    /// A statically formed address, i.e. the event-state table base.
    TableBase,
    /// The event selector scaled by the documented eight-byte entry stride.
    ScaledIndex,
    /// `&table[event]`: the scaled index added to the table base.
    EntryPointer,
    /// The interrupt mask returned by the disable call, to be handed back to
    /// the matching restore call.
    SavedMask,
}

/// Recognize the public `osSetEventMesg(OSEvent, OSMesgQueue *, OSMesg)`.
///
/// Every clause below is a published-ABI property of the routine, not a
/// property of any particular compilation of it:
///
/// * it is a routine with a stack frame it restores before `jr $ra`;
/// * the three o32 argument registers carry the event, queue and message,
///   and each must survive the interrupt-disable call to be used afterwards;
/// * the table update is bracketed by an interrupt disable and a restore, and
///   the mask the disable returns is the argument the restore consumes;
/// * the event selector is scaled by eight, the `OSEventState` entry stride
///   (one `OSMesgQueue *` plus one `OSMesg`);
/// * the scaled index is added to a statically formed table base to form the
///   entry address; and
/// * the queue is stored at entry offset zero and the message at offset four.
///
/// No register assignment, instruction schedule, table address or event
/// constant is pinned. Builds that special-case `OS_EVENT_PRENMI` after the
/// store and builds that do not are both accepted, because that branch is not
/// part of the routine's documented contract.
fn is_set_event_mesg(words: &[u32]) -> bool {
    use SetEventMesgValue as V;

    const MIN_WORDS: usize = 8;
    if words.len() < MIN_WORDS || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let frame_size = imm(words[0]);

    let mut registers = [V::Unknown; 32];
    registers[4] = V::Event;
    registers[5] = V::Queue;
    registers[6] = V::Mesg;
    let mut spill: BTreeMap<i16, V> = BTreeMap::new();
    let mut calls = 0usize;
    let mut restored_mask = false;
    let mut stored_queue = false;
    let mut stored_mesg = false;
    let mut saw_return = false;
    let mut frame_restored = false;

    let mut index = 1;
    while index < words.len() {
        let word = words[index];
        if is_jr_ra(word) {
            // 1997 restores the frame before `jr`; 1998 may use the delay slot.
            saw_return = frame_restored || words
                .get(index + 1)
                .is_some_and(|&slot| is_addiu(slot, 29, 29, frame_size.wrapping_neg()));
            break;
        }
        if is_addiu(word, 29, 29, frame_size.wrapping_neg()) {
            frame_restored = true;
            index += 1;
            continue;
        }
        if frame_restored && rs(word) == 29 && matches!(op(word), 0x23 | 0x2b) {
            return false;
        }
        if jal_field(word).is_some() {
            if let Some(&slot) = words.get(index + 1) {
                set_event_step(&mut registers, &mut spill, slot, &mut stored_queue, &mut stored_mesg);
            }
            // A restore call consumes the mask the disable call produced.
            if calls > 0 && registers[4] == V::SavedMask {
                restored_mask = true;
            }
            calls += 1;
            for caller_saved in (1usize..16).chain([24usize, 25, 31]) {
                registers[caller_saved] = V::Unknown;
            }
            registers[2] = if calls == 1 { V::SavedMask } else { V::Unknown };
            index += 2;
            continue;
        }
        set_event_step(&mut registers, &mut spill, word, &mut stored_queue, &mut stored_mesg);
        index += 1;
    }

    saw_return && calls >= 2 && restored_mask && stored_queue && stored_mesg
}

fn set_event_step(
    registers: &mut [SetEventMesgValue; 32],
    spill: &mut BTreeMap<i16, SetEventMesgValue>,
    word: u32,
    stored_queue: &mut bool,
    stored_mesg: &mut bool,
) {
    use SetEventMesgValue as V;

    match op(word) {
        // sw: either a callee-saved spill or a field write through the entry.
        0x2b => {
            let source = registers[rt(word) as usize];
            if rs(word) == 29 {
                spill.insert(imm(word), source);
            } else if registers[rs(word) as usize] == V::EntryPointer {
                match (imm(word), source) {
                    (0, V::Queue) => *stored_queue = true,
                    (4, V::Mesg) => *stored_mesg = true,
                    _ => {}
                }
            }
        }
        // lw: reloading a spilled argument restores its tag.
        0x23 => {
            if rt(word) != 0 {
                registers[rt(word) as usize] = if rs(word) == 29 {
                    spill.get(&imm(word)).copied().unwrap_or(V::Unknown)
                } else {
                    V::Unknown
                };
            }
        }
        0 => {
            let destination = rd(word) as usize;
            if destination == 0 {
                return;
            }
            match word & 0x3f {
                // sll by three is the documented eight-byte entry stride.
                0x00 => {
                    registers[destination] =
                        if (word >> 6 & 31) == 3 && registers[rt(word) as usize] == V::Event {
                            V::ScaledIndex
                        } else {
                            V::Unknown
                        };
                }
                // addu/or: register moves propagate, index+base forms the entry.
                0x21 | 0x2d | 0x25 => {
                    let left = registers[rs(word) as usize];
                    let right = registers[rt(word) as usize];
                    registers[destination] = if rt(word) == 0 {
                        left
                    } else if rs(word) == 0 {
                        right
                    } else if matches!(
                        (left, right),
                        (V::ScaledIndex, V::TableBase) | (V::TableBase, V::ScaledIndex)
                    ) {
                        V::EntryPointer
                    } else {
                        V::Unknown
                    };
                }
                // jr/jalr write no general register we track.
                0x08 | 0x09 => {}
                _ => registers[destination] = V::Unknown,
            }
        }
        // lui begins a statically formed address.
        0x0f => {
            if rt(word) != 0 {
                registers[rt(word) as usize] = V::TableBase;
            }
        }
        // addiu/ori complete a statically formed address.
        0x09 | 0x0d => {
            if rt(word) != 0 {
                registers[rt(word) as usize] = if rs(word) != 29
                    && registers[rs(word) as usize] == V::TableBase
                {
                    V::TableBase
                } else {
                    V::Unknown
                };
            }
        }
        // Branches and jumps write nothing we track.
        0x02..=0x07 | 0x14..=0x17 | 0x01 => {}
        _ => {
            if rt(word) != 0 && !is_store_opcode(op(word)) {
                registers[rt(word) as usize] = V::Unknown;
            }
        }
    }
}

fn is_start_thread(words: &[u32]) -> bool {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum V { Unknown, Thread, State, One, Two, Eight, Priority, SavedMask, Static }

    if words.len() < 15 || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let mut regs = [V::Unknown; 32];
    regs[4] = V::Thread;
    let mut spill = BTreeMap::new();
    let mut calls = 0usize;
    let mut saw_state_read = false;
    let mut compared_one = false;
    let mut compared_eight = false;
    let mut wrote_two = false;
    let mut inserted_thread = false;
    let mut priority_reads = 0usize;
    let mut priority_compare = false;
    let mut restored_interrupts = false;
    let mut returned = false;

    let mut index = 1;
    while index < words.len() {
        let word = words[index];
        if is_jr_ra(word) { returned = true; break; }
        if jal_field(word).is_some() {
            if let Some(&slot) = words.get(index + 1) {
                match op(slot) {
                    0 => if matches!(slot & 0x3f, 0x21 | 0x25) {
                        let source = if rt(slot) == 0 { rs(slot) } else if rs(slot) == 0 { rt(slot) } else { 0 };
                        if rd(slot) != 0 { regs[rd(slot) as usize] = regs[source as usize]; }
                    },
                    0x23 => if rt(slot) != 0 {
                        regs[rt(slot) as usize] = if rs(slot) == 29 { spill.get(&imm(slot)).copied().unwrap_or(V::Unknown) } else if regs[rs(slot) as usize] == V::Thread && imm(slot) == 8 { V::Static } else { V::Unknown };
                    },
                    0x09 => if rt(slot) != 0 && regs[rs(slot) as usize] == V::Static { regs[rt(slot) as usize] = V::Static; },
                    _ => {}
                }
            }
            inserted_thread |= regs[5] == V::Thread;
            restored_interrupts |= calls > 0 && regs[4] == V::SavedMask;
            calls += 1;
            for caller_saved in (1usize..16).chain([24usize, 25, 31]) { regs[caller_saved] = V::Unknown; }
            regs[2] = if calls == 1 { V::SavedMask } else { V::Unknown };
            index += 2;
            continue;
        }
        match op(word) {
            0x2b => if rs(word) == 29 { spill.insert(imm(word), regs[rt(word) as usize]); },
            0x29 => if regs[rs(word) as usize] == V::Thread && imm(word) == 0x10 && regs[rt(word) as usize] == V::Two { wrote_two = true; },
            0x23 => if rt(word) != 0 {
                regs[rt(word) as usize] = if rs(word) == 29 { spill.get(&imm(word)).copied().unwrap_or(V::Unknown) } else if imm(word) == 4 { priority_reads += 1; V::Priority } else { V::Unknown };
            },
            0x25 => if rt(word) != 0 {
                regs[rt(word) as usize] = if regs[rs(word) as usize] == V::Thread && imm(word) == 0x10 { saw_state_read = true; V::State } else { V::Unknown };
            },
            0x09 | 0x0d => if rt(word) != 0 {
                regs[rt(word) as usize] = if rs(word) == 0 { match imm(word) { 1 => V::One, 2 => V::Two, 8 => V::Eight, _ => V::Unknown } } else if regs[rs(word) as usize] == V::Static { V::Static } else { V::Unknown };
            },
            0x0f => if rt(word) != 0 { regs[rt(word) as usize] = V::Static; },
            0x04 | 0x05 => {
                let pair = (regs[rs(word) as usize], regs[rt(word) as usize]);
                compared_one |= matches!(pair, (V::State, V::One) | (V::One, V::State));
                compared_eight |= matches!(pair, (V::State, V::Eight) | (V::Eight, V::State));
            }
            0 => match word & 0x3f {
                0x20 | 0x21 | 0x25 => {
                    let value = if rt(word) == 0 { regs[rs(word) as usize] } else if rs(word) == 0 { regs[rt(word) as usize] } else { V::Unknown };
                    if rd(word) != 0 { regs[rd(word) as usize] = value; }
                }
                0x2a | 0x2b => {
                    priority_compare |= regs[rs(word) as usize] == V::Priority && regs[rt(word) as usize] == V::Priority;
                    if rd(word) != 0 { regs[rd(word) as usize] = V::Unknown; }
                }
                0x08 | 0x09 => {}
                _ => if rd(word) != 0 { regs[rd(word) as usize] = V::Unknown; },
            },
            0x02..=0x07 | 0x14..=0x17 | 0x01 => {}
            _ => if rt(word) != 0 && !is_store_opcode(op(word)) { regs[rt(word) as usize] = V::Unknown; },
        }
        index += 1;
    }
    (returned && calls >= 4 && saw_state_read && compared_one && compared_eight && wrote_two
        && inserted_thread && priority_reads >= 2 && priority_compare && restored_interrupts)
        || is_start_thread_register_resident(words)
}

fn is_start_thread_register_resident(words: &[u32]) -> bool {
    words.len() >= 15
        && is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0
        && is_move_addu(words[2], 16, 4) && jal_field(words[5]).is_some()
        && op(words[7]) == 0x25 && rt(words[7]) == 3 && rs(words[7]) == 16
        && imm(words[7]) == 0x10 && is_addiu(words[9], 2, 0, 1)
        && op(words[10]) == 4 && rs(words[10]) == 3 && rt(words[10]) == 2
        && is_addiu(words[11], 2, 0, 8) && is_bne(words[12], 3, 2)
        && is_addiu(words[13], 2, 0, 2) && is_sh(words[14], 2, 16, 0x10)
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
    // `osSetThreadPri(OSThread* t = a0, OSPri pri = a1)` writes `pri` into a
    // thread's priority field (`+4`), short-circuits when it is already at that
    // priority, and reschedules by walking `__osRunQueue`. Two
    // register-allocation layouts occur: the 1998 register-resident build keeps
    // `pri` in callee-saved registers (the old positional match pinned
    // words[0..19] and regs 16/17/18/2); World Tour's 1997 build spills every
    // argument to the stack (`sw a1,44(sp)`) and reloads per use, so the
    // priority write lands outside that fixed window. Match the order-free ABI
    // facts, taint-tracking `a1` through moves and stack spill/reload.
    if words.len() < 20 {
        return false;
    }
    // Entry anchor: the routine establishes its own frame at word[0]; requiring
    // the stack adjust at the head rejects windows that begin mid-body.
    if !(is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0) {
        return false;
    }

    // `true` in slot `r` marks that register currently holds `pri` (a1), traced
    // through `move`/`addu $x,$zero,a1` and stack spill/reload.
    let mut pri_tag = [false; 32];
    pri_tag[5] = true; // a1 = pri on entry
    let mut pri_spill: BTreeMap<i16, bool> = BTreeMap::new();

    let mut wrote_priority_field = false; // `sw <pri>, +4(base)`
    let mut read_priority_field = false; // `lw <x>, +4(base)`
    let mut priority_field_reg = [false; 32]; // regs holding a `+4` field load
    let mut compared_priority = false; // `beq/bne` of pri-tagged vs a +4 read
    let mut loaded_run_queue = false; // `lui hi; lw x, lo(x)` global (__osRunQueue)
    let mut prev_lui_reg: Option<u32> = None;

    for &word in words {
        let opcode = op(word);
        match opcode {
            0x00 => {
                let funct = word & 0x3f;
                if funct == 0x21 || funct == 0x20 {
                    let (d, s, t) = (rd(word), rs(word), rt(word));
                    let src = if t == 0 {
                        Some(s)
                    } else if s == 0 {
                        Some(t)
                    } else {
                        None
                    };
                    match src {
                        Some(src) => {
                            pri_tag[d as usize] = pri_tag[src as usize];
                            priority_field_reg[d as usize] = false;
                        }
                        None => {
                            pri_tag[d as usize] = false;
                            priority_field_reg[d as usize] = false;
                        }
                    }
                }
                prev_lui_reg = None;
            }
            0x0f => {
                prev_lui_reg = Some(rt(word));
            }
            0x23 => {
                let (base, dst, off) = (rs(word), rt(word), imm(word));
                if base == 29 {
                    pri_tag[dst as usize] = *pri_spill.get(&off).unwrap_or(&false);
                    priority_field_reg[dst as usize] = false;
                } else if off == 4 {
                    read_priority_field = true;
                    priority_field_reg[dst as usize] = true;
                    pri_tag[dst as usize] = false;
                } else {
                    if prev_lui_reg == Some(base) {
                        loaded_run_queue = true;
                    }
                    priority_field_reg[dst as usize] = false;
                    pri_tag[dst as usize] = false;
                }
                if prev_lui_reg != Some(base) {
                    prev_lui_reg = None;
                }
            }
            0x2b => {
                let (base, src, off) = (rs(word), rt(word), imm(word));
                if base == 29 {
                    pri_spill.insert(off, pri_tag[src as usize]);
                } else if off == 4 && pri_tag[src as usize] {
                    wrote_priority_field = true;
                }
                prev_lui_reg = None;
            }
            0x04 | 0x05 => {
                let (a, b) = (rs(word), rt(word));
                let pri_vs_field = (pri_tag[a as usize] && priority_field_reg[b as usize])
                    || (pri_tag[b as usize] && priority_field_reg[a as usize]);
                if pri_vs_field {
                    compared_priority = true;
                }
                prev_lui_reg = None;
            }
            _ => {
                prev_lui_reg = None;
            }
        }
    }

    wrote_priority_field && read_priority_field && compared_priority && loaded_run_queue
}

fn is_sp_task_load(words: &[u32]) -> bool {
    sp_task_load_helpers(words).is_some() || is_sp_task_load_register_resident(words)
}

fn is_sp_task_load_register_resident(words: &[u32]) -> bool {
    words.len() >= 131
        && is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0
        && is_move_addu(words[2], 16, 4) && is_move_addu(words[6], 5, 17)
        && jal_field(words[8]).is_some() && is_addiu(words[9], 6, 0, 0x40)
        && is_andi(words[68], 2, 2, 1) && is_beq(words[69], 2, 0)
        && is_lw_at(words[79], 2, 16, 4) && is_addiu(words[80], 3, 0, -2)
        && op(words[81]) == 0 && words[81] & 0x3f == 0x24 && rd(words[81]) == 2
        && rs(words[81]) == 2 && rt(words[81]) == 3 && is_sw(words[82], 2, 16, 4)
        && is_andi(words[85], 2, 2, 4) && is_beq(words[86], 2, 0)
        && is_lw_at(words[88], 2, 16, 0x38) && is_move_addu(words[94], 4, 17)
        && jal_field(words[95]).is_some() && is_addiu(words[96], 5, 0, 0x40)
        && jal_field(words[97]).is_some() && is_addiu(words[98], 4, 0, 0x2b00)
        && is_lui(words[100], 4) && words[100] as u16 == 0x0400
        && jal_field(words[101]).is_some() && op(words[102]) == 0x0d
        && rt(words[102]) == 4 && rs(words[102]) == 4 && words[102] as u16 == 0x1000
        && is_addiu(words[106], 4, 0, 1) && is_lui(words[107], 5)
        && words[107] as u16 == 0x0400 && op(words[108]) == 0x0d
        && rt(words[108]) == 5 && rs(words[108]) == 5 && words[108] as u16 == 0x0fc0
        && jal_field(words[110]).is_some() && is_addiu(words[111], 7, 0, 0x40)
        && jal_field(words[114]).is_some() && is_lw_at(words[119], 6, 17, 8)
        && is_lw_at(words[120], 7, 17, 12) && is_lui(words[121], 5)
        && words[121] as u16 == 0x0400 && jal_field(words[122]).is_some()
        && jal_field(words[110]) == jal_field(words[122]) && op(words[123]) == 0x0d
        && rt(words[123]) == 5 && rs(words[123]) == 5 && words[123] as u16 == 0x1000
        && is_jr_ra(words[129]) && is_addiu(words[130], 29, 29, -imm(words[0]))
}

/// Return `(busy, set_status)` only after proving the complete `osSpTaskLoad`
/// dataflow.  The 1998 build keeps the prepared task pointer in saved
/// registers; the 1997 build spills both the input and helper result and
/// reloads them at every use.
fn sp_task_load_helpers(words: &[u32]) -> Option<(u32, u32)> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum V { Unknown, Input, Task, Flags, Cleared, Field8, Field12, One, MinusTwo, SixtyFour, Status, Addr1000, AddrFc0 }

    if words.len() < 96 || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return None;
    }
    let mut regs = [V::Unknown; 32];
    regs[4] = V::Input;
    let mut spill = BTreeMap::new();
    let mut first_call = true;
    let mut set_status = None;
    let mut busy = None;
    let mut last_call = None;
    let mut saw_bit_one = false;
    let mut saw_bit_four = false;
    let mut cleared_bit_zero = false;
    let mut task_dma = false;
    let mut boot_dma = false;
    let mut returned = false;

    let mut index = 1;
    while index < words.len() {
        let word = words[index];
        if is_jr_ra(word) { returned = true; break; }
        if let Some(target) = jal_field(word) {
            if let Some(&slot) = words.get(index + 1) {
                match op(slot) {
                    0x09 | 0x0d => if rt(slot) != 0 {
                        regs[rt(slot) as usize] = match (rs(slot), imm(slot)) {
                            (0, 1) => V::One,
                            (0, 0x40) => V::SixtyFour,
                            (0, 0x2b00) => V::Status,
                            (r, 0x1000) if regs[r as usize] == V::Addr1000 => V::Addr1000,
                            (r, 0x0fc0) if regs[r as usize] == V::Addr1000 => V::AddrFc0,
                            _ => V::Unknown,
                        };
                    },
                    0x23 => if rt(slot) != 0 {
                        regs[rt(slot) as usize] = if rs(slot) == 29 { spill.get(&imm(slot)).copied().unwrap_or(V::Unknown) } else if regs[rs(slot) as usize] == V::Task { match imm(slot) { 8 => V::Field8, 12 => V::Field12, _ => V::Unknown } } else { V::Unknown };
                    },
                    0 => if matches!(slot & 0x3f, 0x21 | 0x25) {
                        let source = if rt(slot) == 0 { rs(slot) } else if rs(slot) == 0 { rt(slot) } else { 0 };
                        if rd(slot) != 0 { regs[rd(slot) as usize] = regs[source as usize]; }
                    },
                    _ => {}
                }
            }
            if regs[4] == V::Status { set_status = Some(target); }
            task_dma |= regs[4] == V::One && regs[5] == V::AddrFc0 && regs[6] == V::Task && regs[7] == V::SixtyFour;
            boot_dma |= regs[4] == V::One && regs[5] == V::Addr1000 && regs[6] == V::Field8 && regs[7] == V::Field12;
            for caller_saved in (1usize..16).chain([24usize, 25, 31]) { regs[caller_saved] = V::Unknown; }
            regs[2] = if first_call { first_call = false; V::Task } else { V::Unknown };
            last_call = Some(target);
            index += 2;
            continue;
        }
        match op(word) {
            0x2b => {
                let value = regs[rt(word) as usize];
                if rs(word) == 29 { spill.insert(imm(word), value); }
                else if matches!(regs[rs(word) as usize], V::Input | V::Task) && imm(word) == 4 && value == V::Cleared { cleared_bit_zero = true; }
            }
            0x23 => if rt(word) != 0 {
                regs[rt(word) as usize] = if rs(word) == 29 { spill.get(&imm(word)).copied().unwrap_or(V::Unknown) } else if matches!(regs[rs(word) as usize], V::Input | V::Task) { match imm(word) { 4 => V::Flags, 8 => V::Field8, 12 => V::Field12, _ => V::Unknown } } else { V::Unknown };
            },
            0x0c => if rt(word) != 0 {
                if regs[rs(word) as usize] == V::Flags && imm(word) == 1 { saw_bit_one = true; }
                if regs[rs(word) as usize] == V::Flags && imm(word) == 4 { saw_bit_four = true; }
                regs[rt(word) as usize] = V::Unknown;
            },
            0x0f => if rt(word) != 0 { regs[rt(word) as usize] = if words[index] as u16 == 0x0400 { V::Addr1000 } else { V::Unknown }; },
            0x09 | 0x0d => if rt(word) != 0 {
                regs[rt(word) as usize] = match (rs(word), imm(word)) {
                    (0, 1) => V::One, (0, -2) => V::MinusTwo, (0, 0x40) => V::SixtyFour, (0, 0x2b00) => V::Status,
                    (r, 0x1000) if regs[r as usize] == V::Addr1000 => V::Addr1000,
                    (r, 0x0fc0) if regs[r as usize] == V::Addr1000 => V::AddrFc0,
                    _ => V::Unknown,
                };
            },
            0x04 | 0x05 => {
                if (rs(word) == 2 && rt(word) == 0) || (rs(word) == 0 && rt(word) == 2) {
                    if let Some(target) = last_call { if Some(target) != set_status { busy = Some(target); } }
                }
            }
            0 => match word & 0x3f {
                0x20 | 0x21 | 0x25 => {
                    let value = if rt(word) == 0 { regs[rs(word) as usize] } else if rs(word) == 0 { regs[rt(word) as usize] } else { V::Unknown };
                    if rd(word) != 0 { regs[rd(word) as usize] = value; }
                }
                0x24 => {
                    let left = regs[rs(word) as usize];
                    let right = regs[rt(word) as usize];
                    if rd(word) != 0 { regs[rd(word) as usize] = if matches!((left, right), (V::Flags, V::MinusTwo) | (V::MinusTwo, V::Flags)) { V::Cleared } else { V::Unknown }; }
                }
                0x08 | 0x09 => {}
                _ => if rd(word) != 0 { regs[rd(word) as usize] = V::Unknown; },
            },
            0x02..=0x07 | 0x14..=0x17 | 0x01 => {}
            _ => if rt(word) != 0 && !is_store_opcode(op(word)) { regs[rt(word) as usize] = V::Unknown; },
        }
        index += 1;
    }
    (returned && saw_bit_one && saw_bit_four && cleared_bit_zero && task_dma && boot_dma)
        .then_some((busy?, set_status?))
}

fn extract_sp_task_load_helpers(words: &[u32]) -> Option<(u32, u32)> {
    if let Some(helpers) = sp_task_load_helpers(words) {
        return Some(helpers);
    }
    let set_status = words.iter().enumerate().find_map(|(index, &word)| {
        let target = jal_field(word)?;
        (words.get(index + 1).is_some_and(|&slot| is_addiu(slot, 4, 0, 0x2b00))
            || index > 0 && is_addiu(words[index - 1], 4, 0, 0x2b00))
            .then_some(target)
    })?;
    let busy = words.iter().enumerate().find_map(|(index, &word)| {
        let target = jal_field(word)?;
        let tail = &words[index + 1..words.len().min(index + 6)];
        let until_next_call = &tail[..tail.iter().position(|&later| jal_field(later).is_some()).unwrap_or(tail.len())];
        (target != set_status
            && until_next_call.iter().any(|&later| {
                matches!(op(later), 0x04 | 0x05)
                    && ((rs(later) == 2 && rt(later) == 0)
                        || (rs(later) == 0 && rt(later) == 2))
            }))
        .then_some(target)
    })?;
    Some((busy, set_status))
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SetTimerValue {
    Unknown,
    /// Anything provably zero, used to clear the unlinked list pointers.
    Zero,
    /// The `OSTimer *`, o32 argument one.
    Timer,
    /// High and low words of the `countdown` argument. Under o32 the first
    /// 64-bit argument after the pointer is eight-byte aligned, so it occupies
    /// `$a2`/`$a3`... which is the *interval*; `countdown` therefore arrives in
    /// the caller's argument save area. See the offsets computed below.
    CountdownHigh,
    CountdownLow,
    /// High and low words of the `interval` argument, in `$a2`/`$a3`.
    IntervalHigh,
    IntervalLow,
    /// The destination `OSMesgQueue *`, from the argument save area.
    Queue,
    /// The `OSMesg`, from the argument save area.
    Mesg,
}

/// Recognize the public
/// `osSetTimer(OSTimer *, OSTime countdown, OSTime interval, OSMesgQueue *, OSMesg)`.
///
/// Every clause is a published-ABI property of the routine:
///
/// * it is a routine with a stack frame it restores before `jr $ra`;
/// * `$a0` is the `OSTimer *`, and `$a2`/`$a3` carry the 64-bit `interval`
///   (o32 eight-byte-aligns the first 64-bit argument after the pointer, so
///   `countdown` spills to the caller's argument save area at `frame + 16`,
///   with the queue and message following at `frame + 24` and `frame + 28`);
/// * the eight documented `OSTimer` words are written at their documented
///   offsets: `next`/`prev` cleared because the timer is not yet linked,
///   `value` from `countdown`, `interval` from `interval`, then `mq` and
///   `msg`; and
/// * an `interval` of zero makes the timer one-shot, so on that path `interval`
///   is also written from the `countdown` argument.
///
/// The stack-argument offsets are derived from the frame size rather than
/// pinned, so a build that inlines the timer-list walk and a build that
/// delegates it are both accepted despite their different frames. No register
/// assignment, instruction schedule or callee address is pinned.
fn is_set_timer(words: &[u32]) -> bool {
    use SetTimerValue as V;

    const MIN_WORDS: usize = 12;
    if words.len() < MIN_WORDS || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let frame_size = i32::from(imm(words[0])).unsigned_abs();
    let (
        Ok(countdown_high_slot),
        Ok(countdown_low_slot),
        Ok(queue_slot),
        Ok(mesg_slot),
    ) = (
        i16::try_from(frame_size + 16),
        i16::try_from(frame_size + 20),
        i16::try_from(frame_size + 24),
        i16::try_from(frame_size + 28),
    ) else {
        return false;
    };

    let mut registers = [V::Unknown; 32];
    registers[0] = V::Zero;
    registers[4] = V::Timer;
    registers[6] = V::IntervalHigh;
    registers[7] = V::IntervalLow;
    let mut spill: BTreeMap<i16, V> = BTreeMap::new();
    // Every value observed written to each `OSTimer` word, across all paths.
    let mut fields: BTreeMap<i16, BTreeSet<V>> = BTreeMap::new();
    let mut saw_return = false;
    let mut frame_restored = false;
    let mut post_init_call = false;

    let mut index = 1;
    while index < words.len() {
        let word = words[index];
        if is_jr_ra(word) {
            saw_return = frame_restored || words
                .get(index + 1)
                .is_some_and(|&slot| is_addiu(slot, 29, 29, imm(words[0]).wrapping_neg()));
            break;
        }
        if is_addiu(word, 29, 29, imm(words[0]).wrapping_neg()) {
            frame_restored = true;
            index += 1;
            continue;
        }
        if frame_restored && rs(word) == 29 && matches!(op(word), 0x23 | 0x2b) {
            return false;
        }
        if jal_field(word).is_some() {
            if let Some(&slot) = words.get(index + 1) {
                set_timer_step(
                    &mut registers,
                    &mut spill,
                    &mut fields,
                    slot,
                    countdown_high_slot,
                    countdown_low_slot,
                    queue_slot,
                    mesg_slot,
                );
            }
            post_init_call |= registers[4] == V::Timer;
            for caller_saved in (1usize..16).chain([24usize, 25, 31]) {
                registers[caller_saved] = V::Unknown;
            }
            index += 2;
            continue;
        }
        set_timer_step(
            &mut registers,
            &mut spill,
            &mut fields,
            word,
            countdown_high_slot,
            countdown_low_slot,
            queue_slot,
            mesg_slot,
        );
        index += 1;
    }

    let wrote = |offset: i16, value: V| {
        fields
            .get(&offset)
            .is_some_and(|values| values.contains(&value))
    };

    saw_return
        && post_init_call
        // Not yet linked into the timer list.
        && wrote(0x00, V::Zero)
        && wrote(0x04, V::Zero)
        // value = countdown
        && wrote(0x08, V::CountdownHigh)
        && wrote(0x0c, V::CountdownLow)
        // interval = interval
        && wrote(0x10, V::IntervalHigh)
        && wrote(0x14, V::IntervalLow)
        // A zero interval means one-shot at countdown.
        && wrote(0x10, V::CountdownHigh)
        && wrote(0x14, V::CountdownLow)
        // Where the expiry message is delivered.
        && wrote(0x18, V::Queue)
        && wrote(0x1c, V::Mesg)
}

#[allow(clippy::too_many_arguments)]
fn set_timer_step(
    registers: &mut [SetTimerValue; 32],
    spill: &mut BTreeMap<i16, SetTimerValue>,
    fields: &mut BTreeMap<i16, BTreeSet<SetTimerValue>>,
    word: u32,
    countdown_high_slot: i16,
    countdown_low_slot: i16,
    queue_slot: i16,
    mesg_slot: i16,
) {
    use SetTimerValue as V;

    match op(word) {
        0x2b => {
            let source = registers[rt(word) as usize];
            if rs(word) == 29 {
                spill.insert(imm(word), source);
            } else if registers[rs(word) as usize] == V::Timer {
                fields.entry(imm(word)).or_default().insert(source);
            }
        }
        0x23 => {
            if rt(word) != 0 {
                registers[rt(word) as usize] = if rs(word) == 29 {
                    match imm(word) {
                        offset if offset == countdown_high_slot => V::CountdownHigh,
                        offset if offset == countdown_low_slot => V::CountdownLow,
                        offset if offset == queue_slot => V::Queue,
                        offset if offset == mesg_slot => V::Mesg,
                        offset => spill.get(&offset).copied().unwrap_or(V::Unknown),
                    }
                } else {
                    V::Unknown
                };
            }
        }
        0 => {
            let destination = rd(word) as usize;
            if destination == 0 {
                return;
            }
            match word & 0x3f {
                // Register moves propagate the tracked value.
                0x21 | 0x2d | 0x25 => {
                    registers[destination] = if rt(word) == 0 {
                        registers[rs(word) as usize]
                    } else if rs(word) == 0 {
                        registers[rt(word) as usize]
                    } else {
                        V::Unknown
                    };
                }
                0x08 | 0x09 => {}
                _ => registers[destination] = V::Unknown,
            }
        }
        // Branches and jumps write nothing we track.
        0x02..=0x07 | 0x14..=0x17 | 0x01 => {}
        _ => {
            if rt(word) != 0 && !is_store_opcode(op(word)) {
                registers[rt(word) as usize] = V::Unknown;
            }
        }
    }
}

fn is_sp_task_start_go(words: &[u32], busy: u32, set_status: u32) -> bool {
    if words.len() < 11 || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let mut busy_call = false;
    let mut busy_poll = false;
    let mut status_call = false;
    let mut pending_busy = false;
    let mut returned = false;
    for (index, &word) in words.iter().enumerate().skip(1) {
        if jal_field(word) == Some(busy) { busy_call = true; pending_busy = true; }
        if jal_field(word) == Some(set_status) {
            status_call |= words.get(index + 1).is_some_and(|&slot| is_addiu(slot, 4, 0, 0x125))
                || index > 0 && is_addiu(words[index - 1], 4, 0, 0x125);
        }
        if pending_busy && matches!(op(word), 0x04 | 0x05) && (rs(word) == 2 || rt(word) == 2) && (rs(word) == 0 || rt(word) == 0) {
            busy_poll = true;
        }
        if is_jr_ra(word) { returned = true; break; }
    }
    busy_call && busy_poll && status_call && returned
}

fn is_sp_task_yield(words: &[u32], set_status: u32) -> bool {
    if words.len() < 7 || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let calls = words.iter().filter(|&&word| jal_field(word).is_some()).count();
    let status = words.iter().enumerate().any(|(index, &word)| {
        jal_field(word) == Some(set_status)
            && (words.get(index + 1).is_some_and(|&slot| is_addiu(slot, 4, 0, 0x400))
                || index > 0 && is_addiu(words[index - 1], 4, 0, 0x400))
    });
    calls == 1 && status && words.iter().any(|&word| is_jr_ra(word))
}

fn is_sp_task_yielded(words: &[u32]) -> bool {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum V { Unknown, Task, Status, Flags, Bool, Updated, Cleared, MinusThree }
    if words.len() < 19 || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let mut regs = [V::Unknown; 32]; regs[4] = V::Task;
    let mut spill = BTreeMap::new();
    let mut call_seen = false;
    let mut test_100 = false;
    let mut test_80 = false;
    let mut updated = false;
    let mut cleared = false;
    let mut returned_bool = false;
    let mut returned = false;
    let mut index = 1;
    while index < words.len() {
        let word = words[index];
        if is_jr_ra(word) { returned = true; returned_bool |= regs[2] == V::Bool; break; }
        if jal_field(word).is_some() {
            if let Some(&slot) = words.get(index + 1) {
                if op(slot) == 0x2b && rs(slot) == 29 { spill.insert(imm(slot), regs[rt(slot) as usize]); }
                else if op(slot) == 0 && matches!(slot & 0x3f, 0x21 | 0x25) {
                    let src = if rt(slot) == 0 { rs(slot) } else { rt(slot) }; if rd(slot) != 0 { regs[rd(slot) as usize] = regs[src as usize]; }
                }
            }
            for caller_saved in (1usize..16).chain([24usize, 25, 31]) { regs[caller_saved] = V::Unknown; }
            regs[2] = V::Status; call_seen = true; index += 2; continue;
        }
        match op(word) {
            0x2b => {
                let value = if rt(word) == 0 && test_100 { V::Bool } else { regs[rt(word) as usize] };
                if rs(word) == 29 { spill.insert(imm(word), value); }
                else if regs[rs(word) as usize] == V::Task && imm(word) == 4 {
                    updated |= value == V::Updated;
                    cleared |= value == V::Cleared;
                }
            }
            0x23 => if rt(word) != 0 {
                regs[rt(word) as usize] = if rs(word) == 29 { spill.get(&imm(word)).copied().unwrap_or(V::Unknown) } else if regs[rs(word) as usize] == V::Task && imm(word) == 4 { V::Flags } else { V::Unknown };
            },
            0x0c => if rt(word) != 0 {
                let source = regs[rs(word) as usize];
                if source == V::Status && imm(word) == 0x100 { test_100 = true; regs[rt(word) as usize] = V::Bool; }
                else if source == V::Status && imm(word) == 0x80 { test_80 = true; regs[rt(word) as usize] = V::Unknown; }
                else { regs[rt(word) as usize] = V::Unknown; }
            },
            0x09 => if rt(word) != 0 { regs[rt(word) as usize] = if rs(word) == 0 && imm(word) == -3 { V::MinusThree } else if rs(word) == 0 && matches!(imm(word), 0 | 1) && test_100 { V::Bool } else { V::Unknown }; },
            0 => match word & 0x3f {
                0x02 => if rd(word) != 0 { regs[rd(word) as usize] = if regs[rt(word) as usize] == V::Status && (word >> 6 & 31) == 8 { V::Bool } else { V::Unknown }; },
                0x20 | 0x21 | 0x25 => {
                    let left = regs[rs(word) as usize]; let right = regs[rt(word) as usize];
                    if rd(word) != 0 { regs[rd(word) as usize] = if rt(word) == 0 { left } else if rs(word) == 0 { right } else if matches!((left, right), (V::Flags, V::Bool) | (V::Bool, V::Flags)) { V::Updated } else { V::Unknown }; }
                }
                0x24 => {
                    let left = regs[rs(word) as usize]; let right = regs[rt(word) as usize];
                    if rd(word) != 0 { regs[rd(word) as usize] = if matches!((left, right), (V::Updated, V::MinusThree) | (V::MinusThree, V::Updated) | (V::Flags, V::MinusThree) | (V::MinusThree, V::Flags)) { V::Cleared } else { V::Unknown }; }
                }
                0x08 | 0x09 => {}
                _ => if rd(word) != 0 { regs[rd(word) as usize] = V::Unknown; },
            },
            0x02..=0x07 | 0x14..=0x17 | 0x01 => {}
            _ => if rt(word) != 0 && !is_store_opcode(op(word)) { regs[rt(word) as usize] = V::Unknown; },
        }
        returned_bool |= regs[2] == V::Bool;
        index += 1;
    }
    (call_seen && test_100 && test_80 && updated && cleared && returned_bool && returned)
        || is_sp_task_yielded_register_resident(words)
}

fn is_sp_task_yielded_register_resident(words: &[u32]) -> bool {
    words.len() >= 19 && is_addiu(words[0], 29, 29, -24)
        && is_sw(words[1], 16, 29, 16) && is_sw(words[2], 31, 29, 20)
        && jal_field(words[3]).is_some() && is_move_addu(words[4], 16, 4)
        && op(words[5]) == 0 && words[5] & 0x3f == 2 && rt(words[5]) == 2
        && rd(words[5]) == 4 && (words[5] >> 6 & 31) == 8
        && is_andi(words[6], 2, 2, 0x80) && is_beq(words[7], 2, 0)
        && is_andi(words[8], 4, 4, 1) && is_lw_at(words[9], 2, 16, 4)
        && is_addiu(words[10], 3, 0, -3) && op(words[11]) == 0
        && words[11] & 0x3f == 0x25 && rd(words[11]) == 2
        && rs(words[11]) == 2 && rt(words[11]) == 4 && op(words[12]) == 0
        && words[12] & 0x3f == 0x24 && rd(words[12]) == 2
        && rs(words[12]) == 2 && rt(words[12]) == 3 && is_sw(words[13], 2, 16, 4)
        && is_move_addu(words[14], 2, 4) && is_lw_at(words[15], 31, 29, 20)
        && is_lw_at(words[16], 16, 29, 16) && is_jr_ra(words[17])
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
fn collapse_overlapping_runs(sorted: &[u32]) -> Vec<u32> {
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
    if words.len() < 6 || op(words[0]) != 0x0f || words[0] as u16 != 0xa480 {
        return false;
    }
    let mut hi = [None; 32];
    let mut status = [false; 32];
    let mut masked = [false; 32];
    let mut loaded_status = false;
    let mut normalized = false;
    let mut returned = false;
    for (index, &word) in words.iter().enumerate() {
        match op(word) {
            0x0f => hi[rt(word) as usize] = Some((word & 0xffff) << 16),
            0x0d => {
                let base = rs(word) as usize; let dst = rt(word) as usize;
                hi[dst] = hi[base].map(|value| value | u32::from(word as u16));
            }
            0x23 => {
                let base = rs(word) as usize; let dst = rt(word) as usize;
                let absolute = hi[base].map(|value| value.wrapping_add(i32::from(imm(word)) as u32));
                status[dst] = absolute == Some(0xa480_0018);
                loaded_status |= status[dst]; hi[dst] = None;
            }
            0x0c => {
                let dst = rt(word) as usize;
                masked[dst] = status[rs(word) as usize] && imm(word) == 3;
                hi[dst] = None;
            }
            0x04 | 0x05 => {
                normalized |= (masked[rs(word) as usize] && rt(word) == 0)
                    || (masked[rt(word) as usize] && rs(word) == 0);
            }
            0 => match word & 0x3f {
                0x2a | 0x2b => {
                    normalized |= (masked[rs(word) as usize] && rt(word) == 0)
                        || (masked[rt(word) as usize] && rs(word) == 0);
                }
                0x08 => if rs(word) == 31 {
                    if let Some(&slot) = words.get(index + 1) {
                        if op(slot) == 0 && matches!(slot & 0x3f, 0x2a | 0x2b) {
                            normalized |= (masked[rs(slot) as usize] && rt(slot) == 0)
                                || (masked[rt(slot) as usize] && rs(slot) == 0);
                        }
                    }
                    returned = true;
                    break;
                },
                0x20 | 0x21 | 0x25 => {
                    let src = if rt(word) == 0 { rs(word) } else if rs(word) == 0 { rt(word) } else { 0 };
                    if rd(word) != 0 { masked[rd(word) as usize] = masked[src as usize]; status[rd(word) as usize] = status[src as usize]; }
                }
                _ => {}
            },
            _ => {}
        }
    }
    loaded_status && normalized && returned
}

/// The body of a public `__osEPiRaw{Read,Write}Io`, validated through the
/// wrapper that calls it.
///
/// Every clause is a published property of the routine rather than of one
/// compilation: `OSPiHandle.baseAddress` lives at the documented offset 12, the
/// caller's `devAddr` is ORed into it, the result is forced to the uncached
/// KSEG1 view, and exactly one device access is performed through the pointer
/// so formed. A routine that never builds an uncached PI device pointer out of
/// `handle + 12` is not this routine, whatever else it resembles.
///
/// This exists because the wrapper shape alone is not decisive. A routine that
/// preserves three arguments across a bracketed call is a common compiler
/// idiom; measured over a 287-ROM corpus it is what Rogue Squadron's
/// table-search helper and Turok 3's forwarding wrappers look like too.
/// Checking the callee is what separates PI device IO from those.
fn is_raw_epi_device_io(words: &[u32]) -> bool {
    words.iter().enumerate().any(|(index, word)| {
        // The handle's public `baseAddress` field, loaded from `$a0`.
        if !(op(*word) == 0x23 && imm(*word) == 12 && rs(*word) == 4) {
            return false;
        }
        let tail = &words[index + 1..words.len().min(index + 6)];
        // The uncached KSEG1 device view, the devAddr merge, and the single
        // access through the resulting pointer.
        tail.iter()
            .any(|word| op(*word) == 0x0f && rs(*word) == 0 && (*word & 0xffff) == 0xa000)
            && tail.iter().any(|word| op(*word) == 0 && word & 0x3f == 0x25)
            && tail
                .iter()
                .any(|word| op(*word) == 0x23 || op(*word) == 0x2b)
    })
}

/// Recognize the public `osEPiWriteIo(OSPiHandle *, u32 devAddr, u32 data)` and
/// `osEPiReadIo(OSPiHandle *, u32 devAddr, u32 *data)`.
///
/// Both are the same published shape -- acquire the PI bus, perform one raw
/// device access, release it -- and differ only in which raw routine sits in
/// the middle. The caller resolves that distinction; this predicate identifies
/// the bracketed-IO shape and returns the three call targets in program order.
///
/// The clauses, each a property of the public routine:
///
/// * it builds a stack frame with a negative immediate and restores exactly
///   that frame in the `jr $ra` delay slot;
/// * it saves `$ra`, because it makes calls;
/// * each of the three o32 arguments is moved to a callee-saved register
///   before the first call, since each must survive it;
/// * it makes exactly three calls, in the order acquire, raw op, release;
/// * acquire and release are distinct entry points (`__osPiGetAccess` and
///   `__osPiRelAccess` are different routines);
/// * the acquire call is argument-free -- `__osPiGetAccess(void)` takes no
///   arguments, so nothing may write `$a0`/`$a1`/`$a2` before it; and
/// * the three preserved arguments are routed back into `$a0`/`$a1`/`$a2` for
///   the raw op.
///
/// No register number, instruction schedule, frame size, or address is pinned.
/// The argument-free-acquire clause is the one that rejects the generic
/// three-argument forwarding wrapper this otherwise resembles.
fn epi_io_wrapper_targets(words: &[u32]) -> Option<(u32, u32, u32)> {
    if !(is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0) {
        return None;
    }
    let frame = imm(words[0]);
    // The frame this routine created must be the frame it tears down.
    let end = (4..words.len().saturating_sub(1)).find(|index| {
        is_jr_ra(words[*index]) && is_addiu(words[index + 1], 29, 29, -frame)
    })?;
    let body = &words[..end];
    if !body
        .iter()
        .any(|word| op(*word) == 0x2b && rt(*word) == 31 && rs(*word) == 29)
    {
        return None;
    }

    let calls = body
        .iter()
        .enumerate()
        .filter_map(|(index, word)| jal_field(*word).map(|target| (index, target)))
        .collect::<Vec<_>>();
    let [(acquire_at, acquire), (raw_at, raw), (_, release)] = calls[..] else {
        return None;
    };
    if acquire == release || !(acquire_at < raw_at) {
        return None;
    }

    // The acquire call takes no arguments.
    let writes_argument = |word: u32| -> Option<u32> {
        let target = match op(word) {
            0 => rd(word),
            0x08 | 0x09 | 0x0c | 0x0d | 0x0e | 0x0f | 0x23 | 0x24 | 0x25 => rt(word),
            _ => return None,
        };
        (4..=6).contains(&target).then_some(target)
    };
    if body[..=acquire_at.min(body.len() - 1)]
        .iter()
        .enumerate()
        .any(|(index, word)| index != acquire_at && writes_argument(*word).is_some())
    {
        return None;
    }

    // Each argument is preserved in a callee-saved register across the call,
    // then routed back into its o32 argument register for the raw op.
    let mut preserved = [None; 3];
    for word in &body[..=(acquire_at + 1).min(body.len() - 1)] {
        if op(*word) == 0 && *word & 0x3f == 0x21 && (16..24).contains(&rd(*word)) {
            let source = match (rs(*word), rt(*word)) {
                (source, 0) => source,
                (0, source) => source,
                _ => continue,
            };
            if (4..=6).contains(&source) {
                preserved[source as usize - 4] = Some(rd(*word));
            }
        }
    }
    if preserved.iter().any(Option::is_none) {
        return None;
    }
    let mut routed = [None; 3];
    for word in &body[acquire_at + 1..=(raw_at + 1).min(body.len() - 1)] {
        if op(*word) == 0 && *word & 0x3f == 0x21 && (4..=6).contains(&rd(*word)) {
            let source = match (rs(*word), rt(*word)) {
                (source, 0) => source,
                (0, source) => source,
                _ => continue,
            };
            routed[rd(*word) as usize - 4] = Some(source);
        }
    }
    (routed == preserved).then_some((acquire, raw, release))
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

/// A recovered 64DD drive-init routine and the guard word it tests.
///
/// The guard is the useful output. Both of the routine's paths return the same
/// static `OSPiHandle *`; the guard only decides whether the device probe runs.
/// A consumer that presets it therefore selects a path the guest already
/// implements, without inventing a bus value or a return contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriveRomInitBinding {
    pub binding: HostBinding,
    /// Physical address of the once-only guard word, recovered from the same
    /// `lui`/`lw` pair the recognizer matches.
    pub guard_vram: u32,
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
) -> Result<Option<DriveRomInitBinding>, HostBindingDiscoveryError> {
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
        Ok(vram) => {
            let index = ((vram - va_start) / 4) as usize;
            // The matched window opens with the guard's own lui/lw pair, so its
            // address is recoverable from the same words the predicate checked.
            let guard_vram = absolute_from_lui_offset(words[index], imm(words[index + 1]));
            Ok(Some(DriveRomInitBinding {
                binding: HostBinding {
                    symbol: HostBindingSymbol::OsDriveRomInit,
                    vram,
                },
                guard_vram,
            }))
        }
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
        11,
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
    // Twelve words: the six documented stores plus the lui/addiu pairs, which
    // the 1996-era build emits once per queue head rather than once in total.
    let create = unique_match(
        words,
        va_start,
        12,
        HostBindingSymbol::OsCreateMesgQueue,
        is_create_mesg_queue,
    )?;
    let create_thread = discover_os_create_thread_host_binding(words, va_start)?.vram;
    // Wide enough to contain the 1997-era stack-spilling compilation (WCW/nWo
    // World Tour), whose argument reloads push the direction-gated type stamp
    // to word ~24; the register-resident builds sit well inside this window and
    // `collapse_overlapping_runs` reduces the several containing starts to the
    // single routine entry.
    let epi = unique_match(
        words,
        va_start,
        26,
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
        83,
        HostBindingSymbol::OsSendMesg,
        is_send_mesg,
    )?;
    // Wide enough to contain the epilogue of the longer compilation, which
    // carries an `OS_EVENT_PRENMI` tail after the table store.
    let set_event = unique_match(
        words,
        va_start,
        48,
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
        84,
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
                if recv_call_index + 1 >= words.len() {
                    continue;
                }
                let recv_call_pc = va_start + recv_call_index as u32 * 4;
                let Some(recv) = jal_target(words[recv_call_index], recv_call_pc) else {
                    continue;
                };
                let lower = (create_call_index.saturating_sub(96)..=create_call_index)
                    .rev()
                    .find(|&candidate| is_addiu(words[candidate], 29, 29, imm(words[candidate])) && imm(words[candidate]) < 0)
                    .unwrap_or(create_call_index.saturating_sub(32));
                let create_queue = resolve_call_value(words, lower, create_call_index, 4, 0);
                let recv_queue = resolve_call_value(words, lower, recv_call_index, 4, 0);
                let recv_output = resolve_call_value(words, lower, recv_call_index, 5, 0);
                let recv_block = if is_addiu(words[recv_call_index + 1], 6, 0, 1) {
                    CallValue::Constant(1)
                } else {
                    resolve_call_value(words, lower, recv_call_index, 6, 0)
                };
                if (create_queue != CallValue::Unknown
                    && create_queue == recv_queue
                    && matches!(recv_output, CallValue::Stack(_))
                    && recv_block == CallValue::Constant(1))
                    || (recv_call_index >= 2
                        && is_addiu(words[recv_call_index - 2], 4, 29, imm(words[recv_call_index - 2]))
                        && is_addiu(words[recv_call_index - 1], 5, 29, imm(words[recv_call_index - 1]))
                        && recv_block == CallValue::Constant(1))
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
    let (busy, set_status) = extract_sp_task_load_helpers(load_words)
        .expect("load recognizer proved helper calls");

    let start_go = unique_match(
        words,
        va_start,
        16,
        HostBindingSymbol::OsSpTaskStartGo,
        |candidate| is_sp_task_start_go(candidate, busy, set_status),
    )?;
    let task_yield = unique_match(
        words,
        va_start,
        8,
        HostBindingSymbol::OsSpTaskYield,
        |candidate| is_sp_task_yield(candidate, set_status),
    )?;
    let task_yielded = unique_match(
        words,
        va_start,
        32,
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
    // Wide enough to contain the epilogue of the longer compilation, which
    // inlines the timer-list walk rather than delegating it.
    let set_timer = unique_match(
        words,
        va_start,
        100,
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

/// Per-symbol outcome of running one recognizer independently of the chain.
///
/// [`discover_wm_block_runtime_host_bindings`] is a chain of `?`, so it aborts
/// at the first failing symbol and never evaluates the rest. Under that chain
/// "did not resolve" and "was never evaluated" are indistinguishable, which
/// has been misread as a score at least once. These variants keep them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostBindingProbeOutcome {
    /// Exactly one structural match; the role resolves standalone.
    Resolved { vram: u32 },
    /// The recognizer ran and found nothing.
    Absent,
    /// The recognizer ran and found several candidates, so the role is
    /// ambiguous rather than missing.
    Ambiguous { candidates: Vec<u32> },
    /// The recognizer ran and failed for some other reason.
    Failed { detail: String },
    /// The recognizer was never run because it is only reachable through
    /// multi-stage call-chain logic whose earlier stage did not resolve. This
    /// is *not* evidence the role is absent.
    NotReached { needs: &'static str },
}

impl HostBindingProbeOutcome {
    /// Whether this role resolved. Only [`Self::Resolved`] counts; in
    /// particular [`Self::NotReached`] is not a failure, it is an absence of
    /// measurement, and must never be scored as either.
    pub fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved { .. })
    }

    /// Whether the recognizer actually ran, and so contributes a denominator.
    pub fn was_evaluated(&self) -> bool {
        !matches!(self, Self::NotReached { .. })
    }

    fn from_unique(result: Result<u32, HostBindingDiscoveryError>) -> Self {
        match result {
            Ok(vram) => Self::Resolved { vram },
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch { candidates, .. })
                if candidates.is_empty() =>
            {
                Self::Absent
            }
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch { candidates, .. }) => {
                Self::Ambiguous { candidates }
            }
            Err(error) => Self::Failed {
                detail: format!("{error:?}"),
            },
        }
    }
}

/// Run every host-binding recognizer independently and report the per-symbol
/// outcome.
///
/// This is a *measurement* surface: it reuses the recognizers verbatim and
/// changes none of them. Its only reason to exist is that the production entry
/// point short-circuits, which makes it unable to answer "how many roles does
/// this ROM resolve".
///
/// Twelve of the fifteen roles are separable this way. The remaining three are
/// genuinely derived from an earlier stage's output rather than matched
/// standalone -- `osRecvMesg` is identified by a call chain through the
/// resolved `osCreateMesgQueue` and `osEPiStartDma` addresses, and
/// `osSpTaskStartGo`/`osSpTaskYield` are matched against helper addresses
/// extracted from a resolved `osSpTaskLoad` body. When their prerequisite
/// resolves they are evaluated for real; when it does not they are reported
/// [`HostBindingProbeOutcome::NotReached`] rather than counted as absent.
///
/// The returned vector is symbol-sorted and always contains every one of
/// [`WM_BLOCK_RUNTIME_HOST_SYMBOLS`].
pub fn probe_wm_block_runtime_host_bindings(
    words: &[u32],
    va_start: u32,
) -> Vec<(HostBindingSymbol, HostBindingProbeOutcome)> {
    use HostBindingProbeOutcome as Outcome;
    use HostBindingSymbol as Symbol;

    if !va_start.is_multiple_of(4) {
        return WM_BLOCK_RUNTIME_HOST_SYMBOLS
            .into_iter()
            .map(|symbol| {
                (
                    symbol,
                    Outcome::Failed {
                        detail: format!("{:?}", HostBindingDiscoveryError::UnalignedImage),
                    },
                )
            })
            .collect();
    }

    let unique = |width: usize, symbol: Symbol, predicate: &dyn Fn(&[u32]) -> bool| {
        Outcome::from_unique(unique_match(words, va_start, width, symbol, predicate))
    };

    // The seven overlay-loader roles that are single-window predicates.
    let create = unique(12, Symbol::OsCreateMesgQueue, &is_create_mesg_queue);
    // Width 26 so the 1997-era stack-spilling build (World Tour) fits; see the
    // matching call in `discover_wm_block_runtime_host_bindings`.
    let epi = unique(26, Symbol::OsEPiStartDma, &is_epi_start_dma);
    let get_thread_pri = unique(6, Symbol::OsGetThreadPri, &is_get_thread_pri);
    let send = unique(83, Symbol::OsSendMesg, &is_send_mesg);
    let set_event = unique(48, Symbol::OsSetEventMesg, &is_set_event_mesg);
    let set_thread_pri = unique(20, Symbol::OsSetThreadPri, &is_set_thread_pri);
    let start_thread = unique(84, Symbol::OsStartThread, &is_start_thread);

    // Roles with their own public entry point.
    let create_thread = Outcome::from_unique(unique_create_thread_match(words, va_start));
    let set_timer = unique(100, Symbol::OsSetTimer, &is_set_timer);
    let si_busy = unique(11, Symbol::OsSiDeviceBusy, &is_si_device_busy);

    // The RSP task group. Load and Yielded stand alone; StartGo and Yield are
    // matched against helper addresses that only exist once Load resolves.
    let load = unique(131, Symbol::OsSpTaskLoad, &is_sp_task_load);
    let task_yielded = unique(32, Symbol::OsSpTaskYielded, &is_sp_task_yielded);
    let (start_go, task_yield) = match &load {
        Outcome::Resolved { vram } => {
            let index = ((vram - va_start) / 4) as usize;
            let load_words = &words[index..index + 131];
            let (busy, set_status) = extract_sp_task_load_helpers(load_words)
                .expect("load recognizer proved helper calls");
            (
                unique(16, Symbol::OsSpTaskStartGo, &|candidate: &[u32]| {
                    is_sp_task_start_go(candidate, busy, set_status)
                }),
                unique(8, Symbol::OsSpTaskYield, &|candidate: &[u32]| {
                    is_sp_task_yield(candidate, set_status)
                }),
            )
        }
        _ => (
            Outcome::NotReached {
                needs: "osSpTaskLoad",
            },
            Outcome::NotReached {
                needs: "osSpTaskLoad",
            },
        ),
    };

    // osRecvMesg is identified by the overlay call chain, which needs both the
    // queue initializer and the DMA starter resolved first.
    let recv = match (&create, &epi) {
        (Outcome::Resolved { vram: create_vram }, Outcome::Resolved { vram: epi_vram }) => {
            probe_overlay_recv_mesg(words, va_start, *create_vram, *epi_vram)
        }
        _ => Outcome::NotReached {
            needs: "osCreateMesgQueue + osEPiStartDma",
        },
    };

    let mut outcomes = vec![
        (Symbol::OsCreateMesgQueue, create),
        (Symbol::OsCreateThread, create_thread),
        (Symbol::OsEPiStartDma, epi),
        (Symbol::OsGetThreadPri, get_thread_pri),
        (Symbol::OsRecvMesg, recv),
        (Symbol::OsSendMesg, send),
        (Symbol::OsSetEventMesg, set_event),
        (Symbol::OsSiDeviceBusy, si_busy),
        (Symbol::OsSetThreadPri, set_thread_pri),
        (Symbol::OsSetTimer, set_timer),
        (Symbol::OsSpTaskLoad, load),
        (Symbol::OsSpTaskStartGo, start_go),
        (Symbol::OsSpTaskYield, task_yield),
        (Symbol::OsSpTaskYielded, task_yielded),
        (Symbol::OsStartThread, start_thread),
    ];
    outcomes.sort_by_key(|(symbol, _)| *symbol);
    outcomes
}

/// The `osRecvMesg` half of the overlay call chain, factored out so the probe
/// can run it once its two prerequisites are known. This mirrors the chain
/// search in [`discover_overlay_loader_host_bindings`] exactly.
fn probe_overlay_recv_mesg(
    words: &[u32],
    va_start: u32,
    create: u32,
    epi: u32,
) -> HostBindingProbeOutcome {
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
                if recv_call_index + 1 >= words.len() {
                    continue;
                }
                let recv_call_pc = va_start + recv_call_index as u32 * 4;
                let Some(recv) = jal_target(words[recv_call_index], recv_call_pc) else {
                    continue;
                };
                let lower = (create_call_index.saturating_sub(96)..=create_call_index)
                    .rev()
                    .find(|&candidate| is_addiu(words[candidate], 29, 29, imm(words[candidate])) && imm(words[candidate]) < 0)
                    .unwrap_or(create_call_index.saturating_sub(32));
                let create_queue = resolve_call_value(words, lower, create_call_index, 4, 0);
                let recv_queue = resolve_call_value(words, lower, recv_call_index, 4, 0);
                let recv_output = resolve_call_value(words, lower, recv_call_index, 5, 0);
                let recv_block = if is_addiu(words[recv_call_index + 1], 6, 0, 1) {
                    CallValue::Constant(1)
                } else {
                    resolve_call_value(words, lower, recv_call_index, 6, 0)
                };
                if create_queue != CallValue::Unknown
                    && create_queue == recv_queue
                    && matches!(recv_output, CallValue::Stack(_))
                    && recv_block == CallValue::Constant(1)
                    || recv_call_index >= 2
                        && is_addiu(words[recv_call_index - 2], 4, 29, imm(words[recv_call_index - 2]))
                        && is_addiu(words[recv_call_index - 1], 5, 29, imm(words[recv_call_index - 1]))
                        && recv_block == CallValue::Constant(1)
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
    match recv_targets.as_slice() {
        [recv] => HostBindingProbeOutcome::Resolved { vram: *recv },
        [] => HostBindingProbeOutcome::Absent,
        several => HostBindingProbeOutcome::Ambiguous {
            candidates: several.to_vec(),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallValue {
    Unknown,
    Constant(u32),
    Stack(i32),
}

/// Resolve one call argument backwards through register moves and address
/// construction.  This is deliberately bounded to the containing routine;
/// crossing a call that clobbers a caller-saved register yields `Unknown`.
fn resolve_call_value(words: &[u32], lower: usize, before: usize, register: u32, depth: usize) -> CallValue {
    if depth > 12 || register == 0 { return CallValue::Constant(0); }
    for index in (lower..before).rev() {
        let word = words[index];
        if jal_field(word).is_some() && matches!(register, 1..=15 | 24 | 25 | 31) { return CallValue::Unknown; }
        match op(word) {
            0x09 if rt(word) == register => {
                let delta = i32::from(imm(word));
                if rs(word) == 29 { return CallValue::Stack(delta); }
                if rs(word) == 0 { return CallValue::Constant(delta as u32); }
                return match resolve_call_value(words, lower, index, rs(word), depth + 1) {
                    CallValue::Constant(value) => CallValue::Constant(value.wrapping_add(delta as u32)),
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
                if rt(word) == 0 { return resolve_call_value(words, lower, index, rs(word), depth + 1); }
                if rs(word) == 0 { return resolve_call_value(words, lower, index, rt(word), depth + 1); }
                return CallValue::Unknown;
            }
            opcode if rt(word) == register && matches!(opcode, 0x08 | 0x0a..=0x0c | 0x0e | 0x20..=0x27 | 0x30..=0x37) => return CallValue::Unknown,
            _ => {}
        }
    }
    CallValue::Unknown
}

/// How a resolved WM-block host binding's address was arrived at.
///
/// The gate's 15/15 requirement is met by "recognizer OR validated external",
/// but the provenance stays honest about which of the two actually named each
/// address, so an audit can see exactly how much of a title's catalog leans on
/// an external decompilation versus fn64's own structural recognizers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolutionProvenance {
    /// A structural recognizer produced a unique match at this address. This is
    /// the only provenance the three already-passing titles ever carry.
    ByRecognizer,
    /// No recognizer uniquely resolved this symbol, so its address was taken
    /// from an external reference table -- but only after two validations
    /// passed: it did not contradict any recognizer that *did* fire, and the
    /// routine disassembled at that address exhibits the symbol's required
    /// shape. `source` records the provenance tag of the table it came from.
    ExternalReferenceValidated { source: String },
    /// A recognizer resolved this symbol AND an external table named the same
    /// address. The agreement is itself evidence the external table describes
    /// this ROM; the binding is recognizer-authoritative but the corroboration
    /// is recorded.
    ByRecognizerConfirmedByExternal { source: String },
}

/// A WM-block host binding together with the provenance of its address.
///
/// Parallel to [`HostBinding`] rather than a field on it, so that the forty
/// existing recognizer-only call sites and their tests are untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedHostBinding {
    pub symbol: HostBindingSymbol,
    pub vram: u32,
    pub provenance: ResolutionProvenance,
}

/// A provenance-tagged external reference table: a map from host-binding symbol
/// to its VRAM address, plus a human-readable tag naming where the mapping came
/// from (e.g. an upstream decompilation's symbol dump).
///
/// This is an *optional grading input*, never an authority seed. Nothing here
/// is trusted on its face: every address it supplies is either checked for
/// agreement against a recognizer that fired, or shape-validated by
/// disassembling the routine it points at, before any binding is emitted from
/// it. That mirrors fn64's standing rule that symbol files grade and validate
/// but never silently seed authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalSymbolTable {
    pub source: String,
    pub addresses: BTreeMap<HostBindingSymbol, u32>,
}

impl ExternalSymbolTable {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            addresses: BTreeMap::new(),
        }
    }

    pub fn with(mut self, symbol: HostBindingSymbol, vram: u32) -> Self {
        self.addresses.insert(symbol, vram);
        self
    }
}

/// Context extracted from the recognizers' own resolved addresses, needed to
/// shape-validate the *derived* roles (whose predicates take a prerequisite's
/// helper addresses as parameters) at an external address.
struct DerivedShapeContext {
    /// `(create, epi)` for the overlay `osRecvMesg` call-chain shape.
    overlay_recv: Option<(u32, u32)>,
    /// `(busy, set_status)` helper `jal` fields extracted from a resolved
    /// `osSpTaskLoad` body, needed by the StartGo / Yield shape predicates.
    sp_helpers: Option<(u32, u32)>,
}

/// Run the shape predicate a symbol's recognizer would apply, but anchored at a
/// specific external address rather than scanned across the image. Returns
/// `Ok(())` when the routine there is shaped like that symbol, and an
/// [`HostBindingDiscoveryError`] describing the rejection otherwise.
///
/// This is the load-bearing half of "validated, not trusted": it reuses the
/// exact predicates the recognizers encode, so an external address is accepted
/// only if it points at something fn64 would itself have recognized had the
/// address been unique.
fn shape_sanity_at(
    words: &[u32],
    va_start: u32,
    symbol: HostBindingSymbol,
    external: u32,
    context: &DerivedShapeContext,
) -> Result<(), HostBindingDiscoveryError> {
    if external < va_start || !external.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::ExternalReferenceOutOfRange { symbol, external });
    }
    let index = ((external - va_start) / 4) as usize;
    if index >= words.len() {
        return Err(HostBindingDiscoveryError::ExternalReferenceOutOfRange { symbol, external });
    }
    // Anchor a window at the external address exactly as `unique_match` presents
    // one to the recognizer: the same fixed width per symbol, so the predicate
    // sees precisely what it would have seen scanning. This matters because a
    // few predicates (notably `is_si_device_busy`) require an exact window
    // length, so slicing to end-of-image would spuriously reject them. The width
    // is clamped to the image end; a routine truncated by the image boundary
    // fails its predicate, which is the correct rejection.
    //
    // `osCreateThread` is the one recognizer that scans a length *range* rather
    // than a fixed width; anchoring at the entry, its `MAX_WORDS` upper bound is
    // the faithful window.
    let width = match symbol {
        HostBindingSymbol::OsCreateMesgQueue => 12,
        HostBindingSymbol::OsEPiStartDma => 26,
        HostBindingSymbol::OsGetThreadPri => 6,
        HostBindingSymbol::OsSendMesg => 83,
        HostBindingSymbol::OsSetEventMesg => 48,
        HostBindingSymbol::OsSetThreadPri => 20,
        HostBindingSymbol::OsStartThread => 84,
        HostBindingSymbol::OsCreateThread => 96,
        HostBindingSymbol::OsSetTimer => 100,
        HostBindingSymbol::OsSiDeviceBusy => 11,
        HostBindingSymbol::OsSpTaskLoad => 131,
        HostBindingSymbol::OsSpTaskYielded => 32,
        HostBindingSymbol::OsSpTaskStartGo => 16,
        HostBindingSymbol::OsSpTaskYield => 8,
        // osRecvMesg is validated by the overlay call chain, not a window.
        HostBindingSymbol::OsRecvMesg => 1,
        HostBindingSymbol::OsDriveRomInit
        | HostBindingSymbol::OsEPiWriteIo
        | HostBindingSymbol::OsEPiReadIo
        | HostBindingSymbol::OsFlashInit
        | HostBindingSymbol::OsFlashSectorErase
        | HostBindingSymbol::OsFlashReadArray => 1,
    };
    let body = &words[index..(index + width).min(words.len())];

    let shaped = match symbol {
        HostBindingSymbol::OsCreateMesgQueue => is_create_mesg_queue(body),
        HostBindingSymbol::OsEPiStartDma => is_epi_start_dma(body),
        HostBindingSymbol::OsGetThreadPri => is_get_thread_pri(body),
        HostBindingSymbol::OsSendMesg => is_send_mesg(body),
        HostBindingSymbol::OsSetEventMesg => is_set_event_mesg(body),
        HostBindingSymbol::OsSetThreadPri => is_set_thread_pri(body),
        HostBindingSymbol::OsStartThread => is_start_thread(body),
        HostBindingSymbol::OsCreateThread => is_create_thread(body),
        HostBindingSymbol::OsSetTimer => is_set_timer(body),
        HostBindingSymbol::OsSiDeviceBusy => is_si_device_busy(body),
        HostBindingSymbol::OsSpTaskLoad => is_sp_task_load(body),
        HostBindingSymbol::OsSpTaskYielded => is_sp_task_yielded(body),
        // Derived roles: their shape predicates require a prerequisite's helper
        // addresses. Validate only when that prerequisite recognizer resolved;
        // otherwise the external address is unvalidatable rather than trusted.
        HostBindingSymbol::OsSpTaskStartGo => {
            let Some((busy, set_status)) = context.sp_helpers else {
                return Err(HostBindingDiscoveryError::ExternalReferenceUnvalidatable {
                    symbol,
                    external,
                    needs: "osSpTaskLoad",
                });
            };
            is_sp_task_start_go(body, busy, set_status)
        }
        HostBindingSymbol::OsSpTaskYield => {
            let Some((_, set_status)) = context.sp_helpers else {
                return Err(HostBindingDiscoveryError::ExternalReferenceUnvalidatable {
                    symbol,
                    external,
                    needs: "osSpTaskLoad",
                });
            };
            is_sp_task_yield(body, set_status)
        }
        HostBindingSymbol::OsRecvMesg => {
            let Some((create, epi)) = context.overlay_recv else {
                return Err(HostBindingDiscoveryError::ExternalReferenceUnvalidatable {
                    symbol,
                    external,
                    needs: "osCreateMesgQueue + osEPiStartDma",
                });
            };
            // osRecvMesg has no standalone structural signature; the recognizer
            // identifies it as the call target of the overlay loader's blocking
            // receive. Shape-validating an external osRecvMesg address therefore
            // means proving the overlay chain names *this* address.
            match probe_overlay_recv_mesg(words, va_start, create, epi) {
                HostBindingProbeOutcome::Resolved { vram } => vram == external,
                _ => false,
            }
        }
        // Roles outside the WM-block catalog are not external-resolvable here.
        HostBindingSymbol::OsDriveRomInit
        | HostBindingSymbol::OsEPiWriteIo
        | HostBindingSymbol::OsEPiReadIo
        | HostBindingSymbol::OsFlashInit
        | HostBindingSymbol::OsFlashSectorErase
        | HostBindingSymbol::OsFlashReadArray => false,
    };

    if shaped {
        Ok(())
    } else {
        Err(HostBindingDiscoveryError::ExternalReferenceShapeMismatch { symbol, external })
    }
}

/// Discover the WM-block host-binding catalog, resolving each symbol by its
/// structural recognizer first and falling back to a VALIDATED external
/// reference address only where the recognizer did not uniquely resolve.
///
/// Resolution order, per symbol:
///
/// 1. **Recognizer first.** Every recognizer runs exactly as in
///    [`discover_wm_block_runtime_host_bindings`]. A unique structural match is
///    always preferred and always authoritative.
/// 2. **Agreement check.** If a recognizer resolved *and* the external table
///    names the same symbol, the two addresses MUST be equal. Disagreement is a
///    hard [`HostBindingDiscoveryError::ExternalReferenceDisagreement`] -- never
///    a silent preference for one over the other.
/// 3. **Validated external fallback.** If the recognizer did not uniquely
///    resolve but the external table names the symbol, the routine at that
///    address is disassembled and checked against the symbol's own recognizer
///    shape ([`shape_sanity_at`]). Only a shaped routine is bound, with
///    provenance [`ResolutionProvenance::ExternalReferenceValidated`]; a
///    mismatch is [`HostBindingDiscoveryError::ExternalReferenceShapeMismatch`].
///
/// The returned catalog is symbol-sorted and, on `Ok`, contains exactly the
/// fifteen [`WM_BLOCK_RUNTIME_HOST_SYMBOLS`]. A symbol that neither recognizes
/// nor has a validated external address leaves the result short of fifteen (it
/// is simply absent), so callers still see an honest denominator.
pub fn discover_wm_block_runtime_host_bindings_with_external_reference(
    words: &[u32],
    va_start: u32,
    external: &ExternalSymbolTable,
) -> Result<Vec<ResolvedHostBinding>, HostBindingDiscoveryError> {
    let outcomes = probe_wm_block_runtime_host_bindings(words, va_start);

    // Assemble the context the derived-role shape checks need, from whatever the
    // recognizers themselves resolved.
    let resolved_of = |symbol: HostBindingSymbol| -> Option<u32> {
        outcomes.iter().find_map(|(candidate, outcome)| {
            if *candidate == symbol {
                if let HostBindingProbeOutcome::Resolved { vram } = outcome {
                    return Some(*vram);
                }
            }
            None
        })
    };
    let overlay_recv = match (
        resolved_of(HostBindingSymbol::OsCreateMesgQueue),
        resolved_of(HostBindingSymbol::OsEPiStartDma),
    ) {
        (Some(create), Some(epi)) => Some((create, epi)),
        _ => None,
    };
    let sp_helpers = resolved_of(HostBindingSymbol::OsSpTaskLoad).and_then(|load| {
        let index = ((load - va_start) / 4) as usize;
        let load_words = words.get(index..index + 131)?;
        extract_sp_task_load_helpers(load_words)
    });
    let context = DerivedShapeContext {
        overlay_recv,
        sp_helpers,
    };

    let mut bindings = Vec::new();
    for (symbol, outcome) in &outcomes {
        let symbol = *symbol;
        let external_address = external.addresses.get(&symbol).copied();
        match outcome {
            HostBindingProbeOutcome::Resolved { vram } => {
                // AGREEMENT: a recognizer and an external address for the same
                // symbol must be equal, or it is a hard error.
                let provenance = match external_address {
                    Some(external_vram) if external_vram != *vram => {
                        return Err(HostBindingDiscoveryError::ExternalReferenceDisagreement {
                            symbol,
                            recognizer: *vram,
                            external: external_vram,
                        });
                    }
                    Some(_) => ResolutionProvenance::ByRecognizerConfirmedByExternal {
                        source: external.source.clone(),
                    },
                    None => ResolutionProvenance::ByRecognizer,
                };
                bindings.push(ResolvedHostBinding {
                    symbol,
                    vram: *vram,
                    provenance,
                });
            }
            // The recognizer did not uniquely resolve. Fall back to the external
            // address only after SHAPE-SANITY validation.
            HostBindingProbeOutcome::Absent
            | HostBindingProbeOutcome::Ambiguous { .. }
            | HostBindingProbeOutcome::NotReached { .. }
            | HostBindingProbeOutcome::Failed { .. } => {
                let Some(external_vram) = external_address else {
                    // No recognizer, no external address: the symbol is absent.
                    continue;
                };
                shape_sanity_at(words, va_start, symbol, external_vram, &context)?;
                bindings.push(ResolvedHostBinding {
                    symbol,
                    vram: external_vram,
                    provenance: ResolutionProvenance::ExternalReferenceValidated {
                        source: external.source.clone(),
                    },
                });
            }
        }
    }

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

/// Recognize the public `osFlashSectorErase(u32 page_num) -> s32`.
///
/// Public N64 FlashRAM Programming Manual: the sector is selected by writing
/// command `0x4B` carrying the page number, the erase is launched with command
/// `0x78`, and the caller then polls status until the busy bit clears. Both
/// commands are delivered through the title's own resolved `osEPiWriteIo` and
/// the poll reads back through its resolved `osEPiReadIo`.
///
/// Parameterising on the resolved wrapper addresses is what makes this
/// discriminating: the two command constants alone co-occur in fourteen corpus
/// titles, but only a routine that issues them through *this* title's
/// programmed-IO seam is that title's sector erase.
fn is_flash_sector_erase(words: &[u32], epi_write: u32, epi_read: u32) -> bool {
    if !(is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0) {
        return false;
    }
    let Some(body) = flash_body_to_return(words) else {
        return false;
    };
    if !body.iter().any(|word| is_lui_immediate(*word, 0x4b00))
        || !body.iter().any(|word| is_lui_immediate(*word, 0x7800))
    {
        return false;
    }
    if flash_calls_to(body, epi_write).len() < 2 {
        return false;
    }
    let Some(read_at) = flash_calls_to(body, epi_read).first().copied() else {
        return false;
    };
    // The documented busy poll: a backward branch after reading status.
    body[read_at..]
        .iter()
        .any(|word| matches!(op(*word), 4 | 5) && imm(*word) < 0)
}

/// Recognize the public `osFlashReadArray(OSIoMesg *, s32 pri, u32 page,
/// void *dram, u32 nPages, OSMesgQueue *mq) -> s32`.
///
/// Public manual: the device is placed in read-array mode with command `0xF0`
/// before any transfer, and pages are moved in a loop. The fifth and sixth o32
/// arguments arrive on the caller's frame, above this routine's own.
fn is_flash_read_array(words: &[u32], epi_write: u32, epi_read: u32) -> bool {
    if !(is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0) {
        return false;
    }
    let frame = -imm(words[0]);
    let Some(body) = flash_body_to_return(words) else {
        return false;
    };
    if !body.iter().any(|word| is_lui_immediate(*word, 0xf000)) {
        return false;
    }
    if flash_calls_to(body, epi_write).is_empty() || flash_calls_to(body, epi_read).is_empty() {
        return false;
    }
    // o32 arguments five and six live above this routine's own frame.
    let stack_arguments = body
        .iter()
        .filter(|word| op(**word) == 0x23 && rs(**word) == 29 && imm(**word) >= frame)
        .count();
    if stack_arguments < 2 {
        return false;
    }
    body.iter()
        .any(|word| matches!(op(*word), 4 | 5) && imm(*word) < 0)
}

/// Recognize the public `osFlashInit(void) -> OSPiHandle *`.
///
/// Public `<PR/os_flash.h>` fixes the handle this routine builds, and building
/// it is the routine's whole contract: device type 8, latency 5, pageSize
/// `0x0F`, relDuration 2, pulse `0x0C`, domain 1, and the uncached device base
/// `0xA800_0000`. It is idempotent, comparing the stored base against that
/// constant and returning the existing handle when already initialised.
///
/// This one is not parameterised on the programmed-IO seam, because a title may
/// call `osFlashInit` before anything reaches the device. It does not need to
/// be: the seven published constants together are specific enough that the
/// whole 287-ROM corpus yields exactly the titles that genuinely link the
/// routine, each verified by disassembly.
fn is_flash_init(words: &[u32]) -> bool {
    if !(is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0) {
        return false;
    }
    let Some(body) = flash_body_to_return(words) else {
        return false;
    };
    // The published uncached FlashRAM device base.
    if !body.iter().any(|word| is_lui_immediate(*word, 0xa800)) {
        return false;
    }
    // The six published byte-width handle fields.
    let has_immediate = |value: u16| {
        body.iter()
            .any(|word| op(*word) == 9 && rs(*word) == 0 && (*word as u16) == value)
    };
    if ![8u16, 5, 0x0c, 0x0f, 2, 1]
        .into_iter()
        .all(has_immediate)
    {
        return false;
    }
    // Those fields are stored as bytes into the handle.
    if body.iter().filter(|word| op(**word) == 0x28).count() < 6 {
        return false;
    }
    // The idempotence guard.
    body.iter().any(|word| op(*word) == 4)
}

/// The instruction range from a routine's frame setup to its first `jr $ra`.
fn flash_body_to_return(words: &[u32]) -> Option<&[u32]> {
    (4..words.len())
        .find(|index| is_jr_ra(words[*index]))
        .map(|end| &words[..end])
}

/// Indices of direct calls to `target` within a routine body.
fn flash_calls_to(body: &[u32], target: u32) -> Vec<usize> {
    let field = (target & 0x0fff_ffff) >> 2;
    body.iter()
        .enumerate()
        .filter_map(|(index, word)| (jal_field(*word) == Some(field)).then_some(index))
        .collect()
}

fn is_lui_immediate(word: u32, immediate: u16) -> bool {
    op(word) == 0x0f && rs(word) == 0 && (word as u16) == immediate
}

/// Discover the FlashRAM API bindings, [`FLASH_HOST_SYMBOLS`].
///
/// Never fails, for the same reason as the programmed-IO roles: a title with no
/// FlashRAM links none of these, and that is the correct answer rather than an
/// error. The two command-issuing roles are resolved against the title's own
/// programmed-IO wrappers, so this takes them as inputs and returns nothing for
/// them when the wrappers are absent.
pub fn discover_flash_host_bindings(
    words: &[u32],
    va_start: u32,
    programmed_io: &[HostBinding],
) -> Vec<HostBinding> {
    const SECTOR_ERASE_WINDOW: usize = 88;
    const READ_ARRAY_WINDOW: usize = 110;
    const INIT_WINDOW: usize = 70;

    if !va_start.is_multiple_of(4) {
        return Vec::new();
    }
    let address_of = |symbol| {
        programmed_io
            .iter()
            .find(|binding| binding.symbol == symbol)
            .map(|binding| binding.vram)
    };

    let mut bindings = Vec::new();
    let mut install = |symbol, width: usize, predicate: &dyn Fn(&[u32]) -> bool| {
        if words.len() < width {
            return;
        }
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
        if let [vram] = collapse_overlapping_runs(&candidates)[..] {
            bindings.push(HostBinding { symbol, vram });
        }
    };

    install(HostBindingSymbol::OsFlashInit, INIT_WINDOW, &is_flash_init);
    if let (Some(epi_write), Some(epi_read)) = (
        address_of(HostBindingSymbol::OsEPiWriteIo),
        address_of(HostBindingSymbol::OsEPiReadIo),
    ) {
        install(
            HostBindingSymbol::OsFlashSectorErase,
            SECTOR_ERASE_WINDOW,
            &|window: &[u32]| is_flash_sector_erase(window, epi_write, epi_read),
        );
        install(
            HostBindingSymbol::OsFlashReadArray,
            READ_ARRAY_WINDOW,
            &|window: &[u32]| is_flash_read_array(window, epi_write, epi_read),
        );
    }
    bindings.sort_by_key(|binding| binding.symbol);
    bindings
}

/// Discover the optional programmed-IO host bindings,
/// [`PROGRAMMED_IO_HOST_SYMBOLS`].
///
/// Unlike [`discover_wm_block_runtime_host_bindings`] this never fails: a title
/// that does not link these routines returns an empty map, which is the correct
/// answer for an SRAM title rather than an error. Titles that do link them get
/// both, or -- for a build that links only one half -- just that one.
///
/// `osEPiWriteIo` and `osEPiReadIo` share one shape and are separated by the
/// raw routine each calls: the write half's callee ends in a store through the
/// device pointer, the read half's in a load followed by a store to the
/// caller's out-parameter. Both callees are validated as genuine PI device IO
/// by [`is_raw_epi_device_io`] before either address is reported, so a
/// same-shaped routine that forwards to something else cannot be installed.
///
/// Ambiguity is dropped rather than guessed. If either role matches more than
/// one address the role is omitted, because binding the wrong address would
/// redirect a guest call into an unrelated host shim.
pub fn discover_programmed_io_host_bindings(words: &[u32], va_start: u32) -> Vec<HostBinding> {
    /// Widest window the wrapper needs: prologue, three calls with their
    /// argument routing, and the epilogue.
    const WINDOW: usize = 40;
    /// The raw callee's `lw handle+12` sits well past its PI-status poll.
    const CALLEE_WINDOW: usize = 96;

    if !va_start.is_multiple_of(4) || words.len() < WINDOW {
        return Vec::new();
    }

    let callee_body = |target: u32| -> Option<&[u32]> {
        // `jal_field` yields the raw 26-bit instruction field, which is a WORD
        // address. Scale it to bytes before forming the segment-relative vram.
        let vram = 0x8000_0000u32 | (target << 2);
        let index = usize::try_from(vram.checked_sub(va_start)?).ok()? / 4;
        (index < words.len()).then(|| &words[index..words.len().min(index + CALLEE_WINDOW)])
    };

    let mut writes = Vec::new();
    let mut reads = Vec::new();
    for (index, window) in words.windows(WINDOW).enumerate() {
        let Some((_, raw, _)) = epi_io_wrapper_targets(window) else {
            continue;
        };
        let Some(body) = callee_body(raw) else {
            continue;
        };
        if !is_raw_epi_device_io(body) {
            continue;
        }
        let Some(vram) = u32::try_from(index)
            .ok()
            .and_then(|index| index.checked_mul(4))
            .and_then(|offset| va_start.checked_add(offset))
        else {
            continue;
        };
        // The write half's raw routine stores the caller's data through the
        // device pointer; the read half's loads from it. That is the ABI
        // difference between the two, and the only thing distinguishing them.
        if raw_epi_device_io_is_write(body) {
            writes.push(vram);
        } else {
            reads.push(vram);
        }
    }

    let mut bindings = Vec::new();
    for (symbol, mut candidates) in [
        (HostBindingSymbol::OsEPiReadIo, reads),
        (HostBindingSymbol::OsEPiWriteIo, writes),
    ] {
        candidates.sort_unstable();
        candidates.dedup();
        if let [vram] = collapse_overlapping_runs(&candidates)[..] {
            bindings.push(HostBinding { symbol, vram });
        }
    }
    bindings.sort_by_key(|binding| binding.symbol);
    bindings
}

/// Whether a validated raw EPI device routine is the WRITE half.
///
/// The two halves differ only in the direction of the single device access the
/// public routine performs: the write half stores the caller's `data` through
/// the uncached device pointer, the read half loads from it. Everything before
/// that access -- the PI-status poll, the domain-register publication, the
/// handle decode -- is identical in both.
fn raw_epi_device_io_is_write(words: &[u32]) -> bool {
    words
        .iter()
        .enumerate()
        .find_map(|(index, word)| {
            (op(*word) == 0x23 && imm(*word) == 12 && rs(*word) == 4).then_some(index)
        })
        .and_then(|index| {
            words[index + 1..words.len().min(index + 6)]
                .iter()
                .find(|word| op(**word) == 0x23 || op(**word) == 0x2b)
                .map(|word| op(*word) == 0x2b)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;
