//! `osCreateMesgQueue`, `osEPiStartDma` (plus its PI-wrapper-shape and
//! candidate-classification support), and `osSendMesg` recognizers.

use super::core::{
    absolute_from_lui_offset, authoritative_root, image_words, imm, is_addiu, is_jr_ra, is_lui,
    is_lw, is_lw_at, is_move_addu, is_store_opcode, is_sw, jal_field, jal_target, op,
    proven_code_interval, rd, rs, rt, OsPiCandidateLimitKind, OsPiDeviceBasePrerequisite,
    OsPiStartDmaCandidateClassification, OsPiStartDmaCandidateOpenReason,
    OsPiStartDmaShapeCandidate,
};
use crate::cfg::{classify_control, BlockTerminator, Cfg, ControlOp, WordClass};
use crate::facts::FactDb;
use crate::resolve::written_gpr;
use std::collections::{BTreeMap, BTreeSet};

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
pub(super) fn is_create_mesg_queue(words: &[u32]) -> bool {
    if words.len() < 9 {
        return false;
    }
    let stored_to_queue =
        |offset: i16, source: u32| words.iter().any(|&word| is_sw(word, source, 4, offset));
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
pub(super) fn is_epi_start_dma(words: &[u32]) -> bool {
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
                    0x08
                    | 0x09
                    | 0x0a
                    | 0x0b
                    | 0x0c
                    | 0x0d
                    | 0x0e
                    | 0x0f
                    | 0x20
                    | 0x21
                    | 0x24
                    | 0x25
                    | 0x30..=0x37 => rt(word),
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
pub(super) struct OsPiShapeLimits {
    pub(super) roots: usize,
    pub(super) calls: usize,
    pub(super) blocks: usize,
    pub(super) work: usize,
}

pub(super) const DEFAULT_PI_SHAPE_LIMITS: OsPiShapeLimits = OsPiShapeLimits {
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

pub(super) fn classify_os_pi_start_dma_candidate_with_limits(
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

pub(super) fn is_send_mesg(words: &[u32]) -> bool {
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
                    let src = if rt(slot) == 0 {
                        rs(slot)
                    } else if rs(slot) == 0 {
                        rt(slot)
                    } else {
                        0
                    };
                    if rd(slot) != 0 {
                        regs[rd(slot) as usize] = regs[src as usize];
                    }
                } else if is_addiu(slot, 4, 4, imm(slot)) && regs[4] == V::Queue {
                    regs[4] = match imm(slot) {
                        0 => V::RecvWait,
                        4 => V::SendWait,
                        _ => V::Unknown,
                    };
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
                    match imm(word) {
                        0 => V::RecvWait,
                        4 => V::SendWait,
                        8 => V::Valid,
                        12 => V::First,
                        16 => V::Count,
                        20 => V::Buffer,
                        _ => V::Unknown,
                    }
                } else if matches!(regs[rs(word) as usize], V::RecvWait | V::SendWait)
                    && imm(word) == 0
                {
                    regs[rs(word) as usize]
                } else {
                    V::Unknown
                };
                saw_receiver_wait |= value == V::RecvWait;
                if rt(word) != 0 {
                    regs[rt(word) as usize] = value;
                }
            }
            0x2b => {
                let value = regs[rt(word) as usize];
                if rs(word) == 29 {
                    spill.insert(imm(word), value);
                } else if regs[rs(word) as usize] == V::Slot && imm(word) == 0 && value == V::Mesg {
                    stored_mesg = true;
                } else if regs[rs(word) as usize] == V::Queue && imm(word) == 8 && value == V::Valid
                {
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
                        regs[rd(word) as usize] = if rt(word) == 0 {
                            left
                        } else if rs(word) == 0 {
                            right
                        } else if matches!(
                            (left, right),
                            (V::First, V::Valid) | (V::Valid, V::First)
                        ) {
                            V::Sum
                        } else if matches!(
                            (left, right),
                            (V::Buffer, V::ByteIndex) | (V::ByteIndex, V::Buffer)
                        ) {
                            V::Slot
                        } else {
                            V::Unknown
                        };
                    }
                    0x2a | 0x2b => {
                        let left = regs[rs(word) as usize];
                        let right = regs[rt(word) as usize];
                        capacity_check |= left == V::Valid && right == V::Count;
                        regs[rd(word) as usize] = V::Unknown;
                    }
                    0x1a | 0x1b => {
                        hi = if regs[rs(word) as usize] == V::Sum
                            && regs[rt(word) as usize] == V::Count
                        {
                            V::Index
                        } else {
                            V::Unknown
                        };
                    }
                    0x10 => regs[rd(word) as usize] = hi,
                    0x00 => {
                        regs[rd(word) as usize] =
                            if regs[rt(word) as usize] == V::Index && (word >> 6 & 31) == 2 {
                                V::ByteIndex
                            } else {
                                V::Unknown
                            }
                    }
                    0x08 | 0x09 => {}
                    _ => {
                        if rd(word) != 0 {
                            regs[rd(word) as usize] = V::Unknown;
                        }
                    }
                }
            }
            0x02..=0x07 | 0x14..=0x17 | 0x01 => {}
            _ => {
                if rt(word) != 0 && !is_store_opcode(op(word)) {
                    regs[rt(word) as usize] = V::Unknown;
                }
            }
        }
        index += 1;
    }

    (returned
        && restored_frame
        && calls >= 3
        && capacity_check
        && blocked_on_send
        && stored_mesg
        && incremented_valid
        && woke_receiver
        && restored_mask)
        || is_send_mesg_register_resident(words)
}

fn is_send_mesg_register_resident(words: &[u32]) -> bool {
    words.len() >= 57
        && is_addiu(words[0], 29, 29, imm(words[0]))
        && imm(words[0]) < 0
        && is_move_addu(words[2], 16, 4)
        && is_move_addu(words[4], 21, 5)
        && is_move_addu(words[6], 18, 6)
        && jal_field(words[10]).is_some()
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
pub(super) fn is_set_event_mesg(words: &[u32]) -> bool {
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
            saw_return = frame_restored
                || words
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
                set_event_step(
                    &mut registers,
                    &mut spill,
                    slot,
                    &mut stored_queue,
                    &mut stored_mesg,
                );
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
        set_event_step(
            &mut registers,
            &mut spill,
            word,
            &mut stored_queue,
            &mut stored_mesg,
        );
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
                registers[rt(word) as usize] =
                    if rs(word) != 29 && registers[rs(word) as usize] == V::TableBase {
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
