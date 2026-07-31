//! Phase 4 (docs/DISCOVER-DESIGN.md "construct the CFG"): a delay-slot-aware
//! control-flow graph builder over fn64's shared MIPS-III decoder.
//! Every word in a bank is
//! classified into one of the six states the doc names:
//!
//! ```text
//! proven_code | candidate_code | proven_data | candidate_data | conflict | unknown
//! ```
//!
//! # Shared ISA authority
//!
//! Discovery must not have a weaker decoder than recompilation: treating a
//! reserved word as ordinary fallthrough would let data become `ProvenCode`
//! and could unsafely satisfy exact-owner admission. [`classify_control`]
//! therefore consumes [`fn64_recomp_rs::decode`]. Unknown instructions and
//! malformed delay slots terminate the CFG as typed blockers.
//!
//! # Determinism and monotonicity
//!
//! [`build_cfg`] is a pure function of `(bank_bytes, va_start)`: same input,
//! same `Cfg` output, every time -- no I/O, no randomness. Classification
//! only ever strengthens: a word reached by a proven path is `ProvenCode`
//! even if an earlier speculative decode called it `CandidateCode` or
//! `Unknown` (see [`WordClass::merge`]).

use fn64_recomp_rs::{decode, Instruction};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// One decoded MIPS-III instruction's control-flow-relevant shape. Never a
/// full semantic decode -- only what CFG construction needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlOp {
    /// Not a control-transfer instruction (ordinary fallthrough).
    Plain,
    /// Unconditional PC-region jump (`j target`): tail transfer, not a call.
    J { target: u32 },
    /// `jal target`: direct call, return address in $ra.
    Jal { target: u32 },
    /// `jr $rs`: computed jump. `$rs == $ra` (register 31) is treated as an
    /// ordinary return; any other register is an indirect/computed jump
    /// (Phase 6 territory -- this module only records the site).
    Jr { rs: u8 },
    /// `jalr $rd, $rs`: computed call.
    Jalr { rd: u8, rs: u8 },
    /// Ordinary (non-likely) conditional branch to a PC-relative target.
    Branch { target: u32, link: bool },
    /// Branch-likely: the delay slot is annulled (not executed) when the
    /// branch is not taken, per MIPS-III semantics. This changes whether
    /// the delay-slot word is itself always-reached code.
    BranchLikely { target: u32, link: bool },
    /// `break`/`syscall`: exception-producing, terminates the block but is
    /// not a transfer to a decodable target.
    Trap,
    /// Reserved or unsupported instruction. It is never ordinary code.
    Invalid { word: u32 },
}

/// Decode one big-endian MIPS-III word's control-flow shape. Returns `None`
/// for opcodes this module does not need to recognize as control transfers
/// (they are `ControlOp::Plain` from the caller's perspective, but `None`
/// here specifically flags "not decodable as *any* recognized encoding",
/// which is different from "decodable and ordinary" -- see [`decode_word`]).
pub fn classify_control(word: u32) -> ControlOp {
    use Instruction::*;

    match decode(word) {
        J { target } => ControlOp::J { target },
        Jal { target } => ControlOp::Jal { target },
        Jr { rs } => ControlOp::Jr { rs },
        Jalr { rd, rs } => ControlOp::Jalr { rd, rs },
        Beq { off, .. }
        | Bne { off, .. }
        | Blez { off, .. }
        | Bgtz { off, .. }
        | Bltz { off, .. }
        | Bgez { off, .. }
        | Bc0f { off }
        | Bc0t { off }
        | Bc1f { off }
        | Bc1t { off } => ControlOp::Branch {
            target: off as i32 as u32,
            link: false,
        },
        Bltzal { off, .. } | Bgezal { off, .. } => ControlOp::Branch {
            target: off as i32 as u32,
            link: true,
        },
        Beql { off, .. }
        | Bnel { off, .. }
        | Blezl { off, .. }
        | Bgtzl { off, .. }
        | Bltzl { off, .. }
        | Bgezl { off, .. }
        | Bc0fl { off }
        | Bc0tl { off }
        | Bc1fl { off }
        | Bc1tl { off } => ControlOp::BranchLikely {
            target: off as i32 as u32,
            link: false,
        },
        Bltzall { off, .. } | Bgezall { off, .. } => ControlOp::BranchLikely {
            target: off as i32 as u32,
            link: true,
        },
        Syscall { .. } | Break { .. } => ControlOp::Trap,
        Unknown { word } => ControlOp::Invalid { word },
        _ => ControlOp::Plain,
    }
}

/// Resolve a branch's PC-relative target: `pc + 4 + (imm16 << 2)`, per
/// MIPS-III's delay-slot-relative branch addressing.
fn branch_target(pc: u32, imm: u32) -> u32 {
    pc.wrapping_add(4).wrapping_add(imm.wrapping_shl(2))
}

/// Resolve a `j`/`jal`'s 26-bit pseudo-region target: high 4 bits of
/// `(pc + 4)` combined with `target26 << 2`.
pub fn region_target(pc: u32, target26: u32) -> u32 {
    ((pc.wrapping_add(4)) & 0xf000_0000) | (target26 << 2)
}

/// One classified word. `merge` implements the monotonic "strongest result
/// wins" rule the design doc requires at the word level, mirroring
/// `ProofState::supersedes` in spirit but scoped to this module's own
/// six-state lattice (kept separate from `facts::ProofState` because a
/// word's code/data classification and a bank's proof state answer
/// different questions, even though both are monotonic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WordClass {
    Unknown,
    CandidateData,
    CandidateCode,
    ProvenData,
    ProvenCode,
    /// Reached by both a proven-code path and a proven-data path (or two
    /// incompatible proven claims) -- an honest disagreement, never
    /// silently resolved.
    Conflict,
}

impl WordClass {
    /// Combine two classifications for the same word. `Conflict` only
    /// arises from two incompatible *proven* claims; a `Proven` claim
    /// always beats a mere `Candidate` one without creating a conflict, per
    /// the design doc's "a heuristic cannot overwrite contradictory proven
    /// evidence" (read here as: candidates cannot promote themselves to
    /// override proven facts, but they also don't count as contradicting
    /// them).
    pub fn merge(self, other: WordClass) -> WordClass {
        use WordClass::*;
        if self == other {
            return self;
        }
        match (self, other) {
            (ProvenCode, ProvenData) | (ProvenData, ProvenCode) => Conflict,
            (Conflict, _) | (_, Conflict) => Conflict,
            (ProvenCode, _) | (_, ProvenCode) => ProvenCode,
            (ProvenData, _) | (_, ProvenData) => ProvenData,
            (CandidateCode, CandidateData) | (CandidateData, CandidateCode) => {
                // Two heuristics disagree without proof on either side --
                // stays open rather than being called a conflict (conflict
                // is reserved for proven-vs-proven).
                Unknown
            }
            (CandidateCode, _) | (_, CandidateCode) => CandidateCode,
            (CandidateData, _) | (_, CandidateData) => CandidateData,
            (Unknown, Unknown) => Unknown,
        }
    }
}

/// A basic block: a maximal straight-line run ending at a control transfer
/// (inclusive of its delay slot, when the instruction has one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasicBlock {
    pub start_va: u32,
    /// Exclusive end, i.e. one past the last word (delay slot included).
    pub end_va: u32,
    pub terminator: BlockTerminator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockTerminator {
    /// Falls through to the next instruction (word wasn't a control
    /// transfer at all -- only produced at bank end or before a
    /// discontinuous root).
    Fallthrough { next: u32 },
    /// Unconditional tail transfer (`j`): control leaves this owner but the
    /// target is a proven code address.
    Tail { target: u32 },
    /// Direct call (`jal`): falls through to `next` after the call
    /// returns, and separately proves `target` as a callable root.
    Call { target: u32, next: u32 },
    /// Conditional branch: two successors, taken and fallthrough.
    Branch {
        target: u32,
        fallthrough: u32,
        link: bool,
    },
    /// Branch-likely: same two successors, but the fallthrough edge does
    /// NOT execute the delay slot (annulment) -- recorded so callers don't
    /// treat the delay-slot word as unconditionally reached.
    BranchLikely {
        target: u32,
        fallthrough: u32,
        link: bool,
    },
    /// `jr $ra`: an ordinary return, terminates this path with no known
    /// successor.
    Return,
    /// `jr $rs` (rs != $ra) or `jalr`: computed transfer, target(s)
    /// unresolved by this phase -- Phase 6's job. Recorded as an open
    /// indirect site here, not silently dropped.
    Indirect { via_call: bool },
    /// Phase 6 proved an exhaustive finite target set for this computed
    /// transfer. Computed jumps keep their targets inside the current owner;
    /// computed calls prove callable roots while their return edge remains in
    /// the caller. Keeping this distinct from `Tail`/`Call` prevents switch
    /// cases from becoming function-entry candidates.
    ResolvedIndirect { targets: Vec<u32>, via_call: bool },
    /// `break`/`syscall`: terminates the block, no successor.
    Trap,
    /// The shared decoder rejected this word. The word is included in the
    /// block extent for diagnostics but is deliberately not `ProvenCode`.
    InvalidInstruction { pc: u32, word: u32 },
    /// A control transfer was the final word in the materialized bank, so
    /// its architecturally required delay slot could not be decoded.
    MissingDelaySlot { control_pc: u32 },
    /// Ran off the end of the decodable/bank region without a terminator.
    RanOffEnd,
    /// Straight-line descent reached a caller-supplied data-fence VA
    /// (machine-checked embedded read-only data). The block ends here
    /// with no successor; the fenced words are never decoded.
    DataFence { at: u32 },
}

/// One indirect control-transfer site the CFG could not resolve -- carried
/// forward as Phase 6's explicit open frontier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndirectSite {
    pub pc: u32,
    pub via_call: bool,
}

/// An independently reached ordinary instruction which is also the delay word
/// of another control transfer. The predecessor block keeps the architectural
/// control+delay pair; direct entry executes `entry_va` and then continues at
/// `continuation_va`. Callability is separate authority established by exact
/// call facts, not by this structural alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlainDelayEntryAlias {
    pub entry_va: u32,
    pub control_pc: u32,
    pub continuation_va: u32,
}

/// An exact entry whose word is itself control-shaped. Executing it as the
/// predecessor's delay word is architecturally unsupported. Authority
/// consumers reject this metadata instead of deleting either overlapping
/// interpretation; a broad candidate-only CFG may retain it diagnostically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedDelayEntry {
    pub entry_va: u32,
    pub control_pc: u32,
}

/// The full CFG for one bank: word classifications, basic blocks, and the
/// open indirect-site frontier. Built by [`build_cfg`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cfg {
    pub bank: String,
    /// VA -> classification, one entry per word address actually visited.
    /// Addresses never visited by any root's traversal are simply absent
    /// (equivalent to `Unknown`) rather than stored explicitly -- keeps the
    /// map's size proportional to work done, not bank size.
    pub word_class: BTreeMap<u32, WordClass>,
    pub blocks: Vec<BasicBlock>,
    pub direct_calls: Vec<(u32, u32)>,   // (source pc, target)
    pub tail_transfers: Vec<(u32, u32)>, // (source pc, target)
    pub indirect_sites: Vec<IndirectSite>,
    #[serde(default)]
    pub plain_delay_entry_aliases: Vec<PlainDelayEntryAlias>,
    #[serde(default)]
    pub unsupported_delay_entries: Vec<UnsupportedDelayEntry>,
    /// Addresses that are proven callable roots because something reached
    /// them via `jal` or supplied them as an explicit seed root, in
    /// first-seen order. A plain `j` proves its target is code, but not that
    /// it is a function boundary: it may be an intra-function label.
    pub proven_roots: Vec<u32>,
}

/// Read one big-endian MIPS word from `bank_bytes` at bank-relative byte
/// offset `off`. Returns `None` for an out-of-range offset (end of bank).
fn read_word(bank_bytes: &[u8], off: usize) -> Option<u32> {
    let b = bank_bytes.get(off..off + 4)?;
    Some(u32::from_be_bytes(b.try_into().unwrap()))
}

/// Validate the architecturally required word after a control transfer.
/// The returned end address includes an invalid delay word when present so
/// diagnostics retain its location, but callers must only mark `Ok` slots as
/// code.
fn validate_delay_slot(
    bank_bytes: &[u8],
    va_start: u32,
    control_pc: u32,
) -> Result<u32, (u32, BlockTerminator)> {
    let delay_pc = control_pc.wrapping_add(4);
    let Some(off) = delay_pc.checked_sub(va_start).map(|off| off as usize) else {
        return Err((
            control_pc.wrapping_add(4),
            BlockTerminator::MissingDelaySlot { control_pc },
        ));
    };
    let Some(word) = read_word(bank_bytes, off) else {
        return Err((
            control_pc.wrapping_add(4),
            BlockTerminator::MissingDelaySlot { control_pc },
        ));
    };
    if matches!(classify_control(word), ControlOp::Invalid { .. }) {
        return Err((
            delay_pc.wrapping_add(4),
            BlockTerminator::InvalidInstruction { pc: delay_pc, word },
        ));
    }
    Ok(delay_pc)
}

/// Build the CFG for one bank by recursive-descent traversal from `roots`
/// (VAs, e.g. the entrypoint plus any already-proven `jal` targets). This
/// function does not itself decide *which* roots are legitimate -- that is
/// Phase 5's ownership question -- it only computes reachability and
/// per-word/per-block classification from whatever roots the caller
/// supplies, so calling it twice with a growing root set (as Phase 6's
/// fixed-point loop does) is exactly the intended usage.
///
/// `bank_bytes` must be the bank's own ROM-mapped bytes (already sliced to
/// `[rom_start, rom_end)`); `va_start` is that slice's first byte's runtime
/// VA (`RomMapping::va_start`). Every produced VA falls inside
/// `[va_start, va_start + bank_bytes.len())` by construction -- an
/// out-of-range branch/jump target is recorded as a `RanOffEnd` block
/// rather than panicking, since a wrong-target guess or a tail-call to
/// another bank is exactly the kind of thing this phase must survive
/// without crashing.
pub fn build_cfg(bank: &str, bank_bytes: &[u8], va_start: u32, roots: &[u32]) -> Cfg {
    build_cfg_with_indirect(bank, bank_bytes, va_start, roots, &BTreeMap::new())
}

/// Detect embedded in-`.text` pointer tables and return the VA of each
/// table's first word, suitable as a [`build_cfg_fenced`] fence set.
///
/// The machine-checked signal is a run of `min_run` or more consecutive
/// 4-aligned words that are each a valid VA into this bank's own code
/// window `[va_start, code_end)`. A jump/handler/vtable stored inline in
/// `.text` is exactly this: a dense run of self-references. Ordinary code
/// does not contain long runs of words that are all bank-internal
/// aligned addresses (an instruction stream mixes opcodes, immediates,
/// and offsets), so this fences data tables, not code. Descent that would
/// otherwise fall through a leaf function into the following pointer table
/// -- decoding those addresses as instructions and manufacturing a
/// computed branch into a neighbor (the MM audio/camera contamination) --
/// stops at the table instead. A shorter run stays unfenced; the caller
/// still grades wrong==0 as the final judge.
pub fn detect_embedded_data(
    bank_bytes: &[u8],
    va_start: u32,
    code_end: u32,
    min_run: usize,
) -> BTreeSet<u32> {
    let words: Vec<u32> = bank_bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
        .collect();
    let is_internal_ptr = |word: u32| word.is_multiple_of(4) && word > va_start && word < code_end;
    let mut fences = BTreeSet::new();
    let mut run_start: Option<usize> = None;
    for index in 0..=words.len() {
        let in_run = index < words.len() && is_internal_ptr(words[index]);
        match (run_start, in_run) {
            (None, true) => run_start = Some(index),
            (Some(start), false) => {
                if index - start >= min_run {
                    fences.insert(va_start + (start as u32) * 4);
                }
                run_start = None;
            }
            _ => {}
        }
    }
    fences
}

/// Build a CFG while consuming Phase 6's exhaustive indirect successors.
/// The map is keyed by transfer-site PC. A missing or empty target set leaves
/// the site open; callers must never put partially-resolved sets here.
pub fn build_cfg_with_indirect(
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    roots: &[u32],
    exhaustive_indirect: &BTreeMap<u32, Vec<u32>>,
) -> Cfg {
    build_cfg_fenced(
        bank,
        bank_bytes,
        va_start,
        roots,
        exhaustive_indirect,
        &BTreeSet::new(),
    )
}

/// [`build_cfg_with_indirect`] plus a caller-supplied set of DATA FENCE
/// addresses: VAs the caller has machine-checked to begin embedded
/// read-only data (e.g. a function's inline float-constant block, located
/// by a typed relocation). Straight-line descent that reaches a fenced VA
/// ends the current block as [`BlockTerminator::DataFence`] instead of
/// decoding the data as instructions -- the fence never adds a successor.
/// The set is evidence, not a guess: this module never derives fences
/// from raw bytes. An empty set reproduces the unfenced CFG exactly.
pub fn build_cfg_fenced(
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    roots: &[u32],
    exhaustive_indirect: &BTreeMap<u32, Vec<u32>>,
    data_fence: &BTreeSet<u32>,
) -> Cfg {
    let va_end = va_start.wrapping_add(bank_bytes.len() as u32);
    let in_range = |va: u32| va >= va_start && va < va_end && (va - va_start).is_multiple_of(4);

    let mut word_class: BTreeMap<u32, WordClass> = BTreeMap::new();
    let mut blocks = Vec::new();
    let mut direct_calls = Vec::new();
    let mut tail_transfers = Vec::new();
    let mut indirect_sites = Vec::new();
    let mut proven_roots: Vec<u32> = Vec::new();
    let mut proven_roots_seen: BTreeSet<u32> = BTreeSet::new();
    let mut block_starts_visited: BTreeSet<u32> = BTreeSet::new();

    let mark = |word_class: &mut BTreeMap<u32, WordClass>, va: u32, class: WordClass| {
        let entry = word_class.entry(va).or_insert(WordClass::Unknown);
        *entry = entry.merge(class);
    };

    let mut worklist: VecDeque<u32> = VecDeque::new();
    // Every root address is also a mandatory block boundary: straight-line
    // scanning must stop and hand off to `Fallthrough` the instant it
    // reaches another root, even mid-run. Without this, a root seeded at
    // an address inside another root's already-decoding straight-line span
    // would be silently swallowed into the first root's block instead of
    // becoming its own block start -- exactly the "interior callable
    // entries remain explicit" requirement from Phase 5's hard
    // constraints, and the reason two competing roots can ever be
    // detected as ambiguous by `partition` at all.
    let root_set: BTreeSet<u32> = roots.iter().copied().filter(|&r| in_range(r)).collect();
    for &r in roots {
        if in_range(r) && proven_roots_seen.insert(r) {
            proven_roots.push(r);
            worklist.push_back(r);
        }
    }

    while let Some(block_start) = worklist.pop_front() {
        if !block_starts_visited.insert(block_start) {
            continue;
        }
        if !in_range(block_start) {
            continue;
        }

        let mut pc = block_start;
        loop {
            // A data fence stops straight-line descent before the fenced
            // word is decoded. When the block's own start is fenced it
            // becomes a zero-instruction fence block (a root that lands in
            // data); mid-run it ends the block with the code decoded so
            // far. Either way the fenced words are never read as code, so
            // an embedded float block cannot manufacture a computed branch
            // into a neighboring function.
            if data_fence.contains(&pc) {
                blocks.push(BasicBlock {
                    start_va: block_start,
                    end_va: pc,
                    terminator: BlockTerminator::DataFence { at: pc },
                });
                break;
            }
            let off = (pc - va_start) as usize;
            let Some(word) = read_word(bank_bytes, off) else {
                blocks.push(BasicBlock {
                    start_va: block_start,
                    end_va: pc,
                    terminator: BlockTerminator::RanOffEnd,
                });
                break;
            };
            let op = classify_control(word);
            // A decoded control transfer is only proof of its own word. Its
            // delay slot becomes code after the shared decoder accepts it;
            // otherwise this path stops at a typed blocker. This closes the
            // interleaving where a valid branch plus adjacent data used to
            // promote that data to `ProvenCode` without decoding it.
            macro_rules! valid_delay {
                () => {{
                    match validate_delay_slot(bank_bytes, va_start, pc) {
                        Ok(delay_pc) => {
                            mark(&mut word_class, delay_pc, WordClass::ProvenCode);
                            delay_pc
                        }
                        Err((end_va, terminator)) => {
                            blocks.push(BasicBlock {
                                start_va: block_start,
                                end_va,
                                terminator,
                            });
                            break;
                        }
                    }
                }};
            }

            match op {
                ControlOp::Plain => {
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    let next = pc.wrapping_add(4);
                    if next != block_start && root_set.contains(&next) {
                        // Another root's entry lands mid-run: end this
                        // block here as an ordinary fallthrough rather
                        // than absorbing the root's code into this owner.
                        blocks.push(BasicBlock {
                            start_va: block_start,
                            end_va: next,
                            terminator: BlockTerminator::Fallthrough { next },
                        });
                        if in_range(next) {
                            worklist.push_back(next);
                        }
                        break;
                    }
                    pc = next;
                    // Keep scanning straight-line code until a transfer,
                    // another root's boundary, or bank end; this loop IS
                    // the block, so no other early block boundary here.
                    continue;
                }
                ControlOp::Trap => {
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    blocks.push(BasicBlock {
                        start_va: block_start,
                        end_va: pc.wrapping_add(4),
                        terminator: BlockTerminator::Trap,
                    });
                    break;
                }
                ControlOp::Invalid { word } => {
                    blocks.push(BasicBlock {
                        start_va: block_start,
                        end_va: pc.wrapping_add(4),
                        terminator: BlockTerminator::InvalidInstruction { pc, word },
                    });
                    break;
                }
                ControlOp::J { target } => {
                    let target = region_target(pc, target);
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    let delay_pc = valid_delay!();
                    tail_transfers.push((pc, target));
                    // An unconditional `j` proves code reachability, not a
                    // callable function boundary. NWXE contains ordinary
                    // intra-function `j` targets that spimdisasm correctly
                    // represents as `alabel`s. Promoting every such target
                    // to an owner root over-splits those functions. Traverse
                    // the target now; partitioning will keep it with the
                    // caller unless independent evidence (`jal` or an
                    // explicit seed) proves the target is a real root.
                    if in_range(target) {
                        worklist.push_back(target);
                    }
                    blocks.push(BasicBlock {
                        start_va: block_start,
                        end_va: delay_pc.wrapping_add(4),
                        terminator: BlockTerminator::Tail { target },
                    });
                    break;
                }
                ControlOp::Jal { target } => {
                    let target = region_target(pc, target);
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    let delay_pc = valid_delay!();
                    let next = delay_pc.wrapping_add(4);
                    direct_calls.push((pc, target));
                    if in_range(target) && proven_roots_seen.insert(target) {
                        proven_roots.push(target);
                        worklist.push_back(target);
                    }
                    // Fallthrough after the call is itself a block start
                    // (the call may not return, but "may return" is the
                    // conservative default -- an unreachable fallthrough is
                    // simply never visited and stays Unknown, not wrongly
                    // marked code).
                    if in_range(next) {
                        worklist.push_back(next);
                    }
                    blocks.push(BasicBlock {
                        start_va: block_start,
                        end_va: next,
                        terminator: BlockTerminator::Call { target, next },
                    });
                    break;
                }
                ControlOp::Jr { rs } => {
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    let delay_pc = valid_delay!();
                    let end = delay_pc.wrapping_add(4);
                    let is_return = rs == 31; // $ra
                    let terminator = if is_return {
                        BlockTerminator::Return
                    } else if let Some(targets) = exhaustive_indirect
                        .get(&pc)
                        .filter(|targets| !targets.is_empty())
                    {
                        for &target in targets {
                            if in_range(target) {
                                worklist.push_back(target);
                            }
                        }
                        BlockTerminator::ResolvedIndirect {
                            targets: targets.clone(),
                            via_call: false,
                        }
                    } else {
                        indirect_sites.push(IndirectSite {
                            pc,
                            via_call: false,
                        });
                        BlockTerminator::Indirect { via_call: false }
                    };
                    blocks.push(BasicBlock {
                        start_va: block_start,
                        end_va: end,
                        terminator,
                    });
                    break;
                }
                ControlOp::Jalr { rd, rs: _ } => {
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    let delay_pc = valid_delay!();
                    let next = delay_pc.wrapping_add(4);
                    // `jalr $zero, $rs` discards the link and is therefore a
                    // computed jump, not a call. Treating it as a call would
                    // invent both a return edge and callable owner roots.
                    let via_call = rd != 0;
                    let terminator = if let Some(targets) = exhaustive_indirect
                        .get(&pc)
                        .filter(|targets| !targets.is_empty())
                    {
                        for &target in targets {
                            if in_range(target) {
                                worklist.push_back(target);
                                if via_call && proven_roots_seen.insert(target) {
                                    proven_roots.push(target);
                                }
                            }
                        }
                        BlockTerminator::ResolvedIndirect {
                            targets: targets.clone(),
                            via_call,
                        }
                    } else {
                        indirect_sites.push(IndirectSite { pc, via_call });
                        BlockTerminator::Indirect { via_call }
                    };
                    if via_call && in_range(next) {
                        worklist.push_back(next);
                    }
                    blocks.push(BasicBlock {
                        start_va: block_start,
                        end_va: next,
                        terminator,
                    });
                    break;
                }
                ControlOp::Branch { target, link } => {
                    let target = branch_target(pc, target);
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    let delay_pc = valid_delay!();
                    // Ordinary branch: delay slot always executes (both
                    // taken and not-taken paths run it), so it's
                    // unconditionally proven code.
                    let fallthrough = delay_pc.wrapping_add(4);
                    if in_range(target) {
                        worklist.push_back(target);
                        if link && proven_roots_seen.insert(target) {
                            proven_roots.push(target);
                        }
                    }
                    if link {
                        direct_calls.push((pc, target));
                    }
                    if in_range(fallthrough) {
                        worklist.push_back(fallthrough);
                    }
                    blocks.push(BasicBlock {
                        start_va: block_start,
                        end_va: fallthrough,
                        terminator: BlockTerminator::Branch {
                            target,
                            fallthrough,
                            link,
                        },
                    });
                    break;
                }
                ControlOp::BranchLikely { target, link } => {
                    let target = branch_target(pc, target);
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    let delay_pc = valid_delay!();
                    // Branch-likely annulment: the delay slot only executes
                    // on the TAKEN path. It is still proven code (reached
                    // via the taken edge) but must not be treated as
                    // unconditionally-executed fallthrough code -- callers
                    // needing that distinction should consult the
                    // `BranchLikely` terminator, not assume `ProvenCode`
                    // implies "always executes".
                    let fallthrough = delay_pc.wrapping_add(4);
                    if in_range(target) {
                        worklist.push_back(target);
                        if link && proven_roots_seen.insert(target) {
                            proven_roots.push(target);
                        }
                    }
                    if link {
                        direct_calls.push((pc, target));
                    }
                    if in_range(fallthrough) {
                        worklist.push_back(fallthrough);
                    }
                    blocks.push(BasicBlock {
                        start_va: block_start,
                        end_va: fallthrough,
                        terminator: BlockTerminator::BranchLikely {
                            target,
                            fallthrough,
                            link,
                        },
                    });
                    break;
                }
            }
        }
    }

    let (plain_delay_entry_aliases, unsupported_delay_entries) =
        extract_plain_delay_entry_aliases(&mut blocks, bank_bytes, va_start);
    canonicalize_blocks(
        &mut blocks,
        &unsupported_delay_entries
            .iter()
            .map(|entry| entry.entry_va)
            .collect(),
    );
    direct_calls.clear();
    tail_transfers.clear();
    indirect_sites.clear();
    for block in &blocks {
        let control_pc = block.end_va.checked_sub(8);
        match &block.terminator {
            BlockTerminator::Call { target, .. } => {
                direct_calls.push((control_pc.expect("call block contains delay pair"), *target));
            }
            BlockTerminator::Tail { target } => {
                tail_transfers.push((control_pc.expect("tail block contains delay pair"), *target));
            }
            BlockTerminator::Branch {
                target, link: true, ..
            }
            | BlockTerminator::BranchLikely {
                target, link: true, ..
            } => {
                direct_calls.push((
                    control_pc.expect("link-branch block contains delay pair"),
                    *target,
                ));
            }
            BlockTerminator::Indirect { via_call } => {
                indirect_sites.push(IndirectSite {
                    pc: control_pc.expect("indirect block contains delay pair"),
                    via_call: *via_call,
                });
            }
            _ => {}
        }
    }
    direct_calls.sort_unstable();
    direct_calls.dedup();
    tail_transfers.sort_unstable();
    tail_transfers.dedup();
    indirect_sites.sort_by_key(|site| (site.pc, site.via_call));
    indirect_sites.dedup_by_key(|site| (site.pc, site.via_call));

    Cfg {
        bank: bank.to_string(),
        word_class,
        blocks,
        direct_calls,
        tail_transfers,
        indirect_sites,
        plain_delay_entry_aliases,
        unsupported_delay_entries,
        proven_roots,
    }
}

fn extract_plain_delay_entry_aliases(
    blocks: &mut Vec<BasicBlock>,
    bank_bytes: &[u8],
    va_start: u32,
) -> (Vec<PlainDelayEntryAlias>, Vec<UnsupportedDelayEntry>) {
    let mut aliases = Vec::new();
    let mut unsupported = Vec::new();
    let entry_candidates = blocks
        .iter()
        .map(|block| block.start_va)
        .collect::<BTreeSet<_>>();
    for entry_va in entry_candidates {
        let Some(control_pc) = entry_va.checked_sub(4) else {
            continue;
        };
        let Some(continuation_va) = entry_va.checked_add(4) else {
            continue;
        };
        let plain = entry_va
            .checked_sub(va_start)
            .and_then(|off| read_word(bank_bytes, off as usize))
            .is_some_and(|word| matches!(classify_control(word), ControlOp::Plain));
        let predecessor_reached = blocks.iter().any(|block| {
            block.end_va == continuation_va
                && block.start_va <= control_pc
                && matches!(
                    block.terminator,
                    BlockTerminator::Tail { .. }
                        | BlockTerminator::Call { .. }
                        | BlockTerminator::Return
                        | BlockTerminator::Indirect { .. }
                        | BlockTerminator::ResolvedIndirect { .. }
                        | BlockTerminator::Branch { .. }
                        | BlockTerminator::BranchLikely { .. }
                )
        });
        let Some(alias_index) = blocks.iter().position(|block| block.start_va == entry_va) else {
            continue;
        };
        if !predecessor_reached {
            continue;
        }

        if !plain {
            unsupported.push(UnsupportedDelayEntry {
                entry_va,
                control_pc,
            });
            continue;
        }
        if blocks
            .iter()
            .enumerate()
            .any(|(index, block)| index != alias_index && block.start_va == continuation_va)
        {
            blocks.remove(alias_index);
        } else if blocks[alias_index].end_va > continuation_va {
            blocks[alias_index].start_va = continuation_va;
        } else {
            blocks.remove(alias_index);
        }
        aliases.push(PlainDelayEntryAlias {
            entry_va,
            control_pc,
            continuation_va,
        });
    }
    blocks.sort_by_key(|block| block.start_va);
    aliases.sort_by_key(|alias| alias.entry_va);
    aliases.dedup_by_key(|alias| alias.entry_va);
    unsupported.sort_by_key(|entry| entry.entry_va);
    unsupported.dedup_by_key(|entry| entry.entry_va);
    (aliases, unsupported)
}

/// Recursive descent can discover a branch target after an earlier scan has
/// already crossed that address. Every discovered block start is nonetheless
/// a mandatory leader: truncate any earlier overlapping span into an ordinary
/// fallthrough prefix. The later block retains the real terminator, producing
/// a disjoint canonical graph without re-decoding or inventing an edge.
fn canonicalize_blocks(blocks: &mut [BasicBlock], retained_overlaps: &BTreeSet<u32>) {
    blocks.sort_by_key(|block| block.start_va);
    let starts: Vec<u32> = blocks.iter().map(|block| block.start_va).collect();
    for block in blocks.iter_mut() {
        if let Some(&leader) = starts.iter().find(|&&start| {
            start > block.start_va && start < block.end_va && !retained_overlaps.contains(&start)
        }) {
            block.end_va = leader;
            block.terminator = BlockTerminator::Fallthrough { next: leader };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    const NOP: u32 = 0x0000_0000; // sll $zero, $zero, 0

    fn jal(target: u32) -> u32 {
        0x0c00_0000 | (target >> 2 & 0x03ff_ffff)
    }

    #[test]
    fn exact_call_to_plain_delay_word_preserves_predecessor_pair() {
        let base = 0x8000_0000;
        let predecessor = base + 16;
        let delay_entry = predecessor + 4;
        let bytes = asm(&[
            jal(delay_entry),
            NOP,
            0x03e0_0008,
            NOP,
            0x1000_0001,
            NOP,
            0x03e0_0008,
            NOP,
        ]);

        let cfg = build_cfg("bank", &bytes, base, &[base, predecessor]);

        assert_eq!(
            cfg.plain_delay_entry_aliases,
            vec![PlainDelayEntryAlias {
                entry_va: delay_entry,
                control_pc: predecessor,
                continuation_va: delay_entry + 4,
            }]
        );
        assert!(cfg.proven_roots.contains(&delay_entry));
        assert!(!cfg.blocks.iter().any(|block| block.start_va == delay_entry));
        assert!(cfg.blocks.iter().any(|block| {
            block.start_va == predecessor
                && block.end_va == delay_entry + 4
                && matches!(block.terminator, BlockTerminator::Branch { .. })
        }));
    }

    #[test]
    fn exhaustive_jalr_to_plain_delay_word_uses_same_alias() {
        let base = 0x8000_0000;
        let predecessor = base + 16;
        let delay_entry = predecessor + 4;
        let jalr_ra_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let bytes = asm(&[
            jalr_ra_t9,
            NOP,
            0x03e0_0008,
            NOP,
            0x1000_0001,
            NOP,
            0x03e0_0008,
            NOP,
        ]);
        let exhaustive = BTreeMap::from([(base, vec![delay_entry])]);

        let cfg = build_cfg_with_indirect("bank", &bytes, base, &[base, predecessor], &exhaustive);

        assert!(cfg.proven_roots.contains(&delay_entry));
        assert_eq!(cfg.plain_delay_entry_aliases[0].entry_va, delay_entry);
        assert!(!cfg.blocks.iter().any(|block| block.start_va == delay_entry));
    }

    #[test]
    fn exact_noncall_transfers_to_plain_delay_word_are_transfer_only_aliases() {
        let base = 0x8000_0000;
        let predecessor = base + 16;
        let delay_entry = predecessor + 4;
        let jump = 0x0800_0000 | (delay_entry >> 2 & 0x03ff_ffff);
        let jr_t9 = (25u32 << 21) | 0x08;
        for (transfer, exhaustive) in [
            (jump, BTreeMap::new()),
            (jr_t9, BTreeMap::from([(base, vec![delay_entry])])),
        ] {
            let bytes = asm(&[
                transfer,
                NOP,
                0x03e0_0008,
                NOP,
                0x1000_0001,
                NOP,
                0x03e0_0008,
                NOP,
            ]);
            let cfg =
                build_cfg_with_indirect("bank", &bytes, base, &[base, predecessor], &exhaustive);
            assert!(cfg
                .plain_delay_entry_aliases
                .iter()
                .any(|alias| { alias.entry_va == delay_entry && alias.control_pc == predecessor }));
            assert!(!cfg.proven_roots.contains(&delay_entry));
            assert!(!cfg.blocks.iter().any(|block| block.start_va == delay_entry));
            assert!(cfg
                .blocks
                .iter()
                .any(|block| { block.start_va == predecessor && block.end_va == delay_entry + 4 }));
        }
    }

    #[test]
    fn control_shaped_delay_entry_is_retained_as_an_explicit_blocker() {
        let base = 0x8000_0000;
        let bytes = asm(&[0x03e0_0008, 0x03e0_0008, NOP]);
        let cfg = build_cfg("bank", &bytes, base, &[base, base + 4]);

        assert!(cfg.plain_delay_entry_aliases.is_empty());
        assert_eq!(
            cfg.unsupported_delay_entries,
            vec![UnsupportedDelayEntry {
                entry_va: base + 4,
                control_pc: base,
            }]
        );
        assert!(cfg.blocks.iter().any(|block| {
            block.start_va == base
                && block.end_va == base + 8
                && matches!(block.terminator, BlockTerminator::Return)
        }));
        assert!(cfg.blocks.iter().any(|block| block.start_va == base + 4));
    }

    #[test]
    fn cfg_old_json_defaults_delay_entry_metadata() {
        let cfg = build_cfg(
            "bank",
            &asm(&[0x03e0_0008, NOP]),
            0x8000_0000,
            &[0x8000_0000],
        );
        let mut value = serde_json::to_value(cfg).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("plain_delay_entry_aliases");
        object.remove("unsupported_delay_entries");

        let decoded: Cfg = serde_json::from_value(value).unwrap();
        assert!(decoded.plain_delay_entry_aliases.is_empty());
        assert!(decoded.unsupported_delay_entries.is_empty());
    }

    #[test]
    fn detect_embedded_data_finds_pointer_table_not_code() {
        // Two ordinary instructions, then a 4-entry inline pointer table
        // of bank-internal aligned addresses.
        let code = [0x27bd_ffe8u32, 0x03e0_0008]; // addiu sp; jr ra
        let table = [0x8000_0100u32, 0x8000_0140, 0x8000_0180, 0x8000_01c0];
        let mut words = Vec::new();
        words.extend_from_slice(&code);
        words.extend_from_slice(&table);
        let bytes = asm(&words);
        let fences = detect_embedded_data(&bytes, 0x8000_0000, 0x8000_1000, 4);
        // Only the table (starting at index 2 = VA 0x...08) is fenced.
        assert_eq!(fences.into_iter().collect::<Vec<_>>(), vec![0x8000_0008]);
        // A run of 3 is below threshold: nothing fenced.
        let short = asm(&[0x27bd_ffe8, 0x8000_0100, 0x8000_0140, 0x8000_0180]);
        assert!(detect_embedded_data(&short, 0x8000_0000, 0x8000_1000, 4).is_empty());
    }

    #[test]
    fn data_fence_stops_descent_before_the_fenced_word() {
        // A leaf function (addiu $sp; jr $ra; nop) immediately followed by
        // an embedded float-constant block that decodes as a valid COP1
        // store with an in-window computed target. Without a fence,
        // descent past the leaf would decode the float and could branch
        // anywhere. With the block's start fenced, descent stops clean.
        let float_word = 0xe032_ada2; // sdc1-shaped; must never be decoded
        let bytes = asm(&[0x27bd_ffe8, 0x03e0_0008, NOP, float_word, float_word]);
        let fence: BTreeSet<u32> = [0x8000_000c].into_iter().collect();
        let cfg = build_cfg_fenced(
            "boot",
            &bytes,
            0x8000_0000,
            &[0x8000_0000, 0x8000_000c],
            &BTreeMap::new(),
            &fence,
        );
        // The fenced root at 0x...0c is a zero-instruction DataFence block;
        // the float word is never classified as code.
        let fenced = cfg
            .blocks
            .iter()
            .find(|b| b.start_va == 0x8000_000c)
            .expect("fenced block present");
        assert!(matches!(
            fenced.terminator,
            BlockTerminator::DataFence { at: 0x8000_000c }
        ));
        assert_eq!(fenced.start_va, fenced.end_va);
        assert!(!cfg.word_class.contains_key(&0x8000_000c));
        // The same input WITHOUT the fence does decode the float word.
        let unfenced = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000, 0x8000_000c]);
        assert_eq!(
            unfenced.word_class.get(&0x8000_000c),
            Some(&WordClass::ProvenCode)
        );
    }

    #[test]
    fn straight_line_falls_off_end_as_ran_off_end() {
        let bytes = asm(&[NOP, NOP, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert_eq!(cfg.blocks.len(), 1);
        assert!(matches!(
            cfg.blocks[0].terminator,
            BlockTerminator::RanOffEnd
        ));
        for va in [0x8000_0000u32, 0x8000_0004, 0x8000_0008] {
            assert_eq!(cfg.word_class[&va], WordClass::ProvenCode);
        }
    }

    #[test]
    fn jal_records_direct_call_and_proves_target_as_root() {
        // jal 0x80000100 ; nop (delay slot) ; [instr at return addr]
        let jal_target: u32 = 0x8000_0100;
        let jal_word = 0x0c00_0000 | ((jal_target >> 2) & 0x03ff_ffff);
        let bytes = asm(&[jal_word, NOP, NOP]);
        // Bank spans far enough to include the jal target too, as a second
        // region of straight-line code.
        let mut full = bytes;
        full.resize(0x200, 0);
        full[0x100..0x104].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("boot", &full, 0x8000_0000, &[0x8000_0000]);

        assert_eq!(cfg.direct_calls, vec![(0x8000_0000, jal_target)]);
        assert!(cfg.proven_roots.contains(&jal_target));
        assert_eq!(cfg.word_class[&0x8000_0000], WordClass::ProvenCode);
        assert_eq!(cfg.word_class[&0x8000_0004], WordClass::ProvenCode); // delay slot
        assert_eq!(cfg.word_class[&jal_target], WordClass::ProvenCode);
    }

    #[test]
    fn jr_ra_terminates_as_return() {
        // jr $ra ; nop
        let jr_ra = 0x03e0_0008u32;
        let bytes = asm(&[jr_ra, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert_eq!(cfg.blocks.len(), 1);
        assert!(matches!(cfg.blocks[0].terminator, BlockTerminator::Return));
        assert!(cfg.indirect_sites.is_empty());
    }

    #[test]
    fn jr_other_register_is_recorded_as_indirect_site() {
        // jr $t9 (register 25)
        let jr_t9 = (25u32 << 21) | 0x08;
        let bytes = asm(&[jr_t9, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert_eq!(cfg.indirect_sites.len(), 1);
        assert_eq!(cfg.indirect_sites[0].pc, 0x8000_0000);
        assert!(!cfg.indirect_sites[0].via_call);
    }

    #[test]
    fn jalr_is_recorded_as_indirect_call_site() {
        // jalr $t9 -> rd defaults to $ra(31) in the field, rs=25 ($t9)
        let jalr = (25u32 << 21) | (31u32 << 11) | 0x09;
        let bytes = asm(&[jalr, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert_eq!(cfg.indirect_sites.len(), 1);
        assert!(cfg.indirect_sites[0].via_call);
    }

    #[test]
    fn jalr_zero_is_a_computed_jump_without_fallthrough() {
        // jalr $zero, $t9; nop; invalid. The invalid word would be visited if
        // discovery invented a call-return edge after a link-discarding jalr.
        let jalr_zero = (25u32 << 21) | 0x09;
        let unknown = 0x7801_2345;
        let bytes = asm(&[jalr_zero, NOP, unknown]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert_eq!(cfg.indirect_sites.len(), 1);
        assert!(!cfg.indirect_sites[0].via_call);
        assert!(matches!(
            cfg.blocks[0].terminator,
            BlockTerminator::Indirect { via_call: false }
        ));
        assert!(!cfg.word_class.contains_key(&0x8000_0008));
    }

    #[test]
    fn regimm_link_branch_proves_its_direct_callable_target() {
        // bltzal $at, +3; nop. The conditional link branch has both ordinary
        // branch successors and a direct callable edge to 0x80000010.
        let bltzal = (1u32 << 26) | (1u32 << 21) | (0x10u32 << 16) | 3;
        let bytes = asm(&[bltzal, NOP, NOP, NOP, NOP, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert_eq!(cfg.direct_calls, vec![(0x8000_0000, 0x8000_0010)]);
        assert!(cfg.proven_roots.contains(&0x8000_0010));
        assert!(matches!(
            cfg.blocks[0].terminator,
            BlockTerminator::Branch {
                target: 0x8000_0010,
                fallthrough: 0x8000_0008,
                link: true,
            }
        ));
    }

    #[test]
    fn canonical_leader_rebuilds_transfer_denominators_from_final_blocks() {
        let jal = 0x0c00_0005u32; // jal 0x80000014
        let bytes = asm(&[NOP, NOP, jal, NOP, NOP, 0x03e0_0008, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000, 0x8000_0008]);
        assert_eq!(cfg.direct_calls, vec![(0x8000_0008, 0x8000_0014)]);
        assert!(matches!(
            cfg.blocks[0].terminator,
            BlockTerminator::Fallthrough { next: 0x8000_0008 }
        ));
        assert!(matches!(
            cfg.blocks[1].terminator,
            BlockTerminator::Call {
                target: 0x8000_0014,
                next: 0x8000_0010
            }
        ));
    }

    #[test]
    fn unknown_root_word_is_not_proven_code() {
        let unknown = 0x7801_2345;
        let cfg = build_cfg("boot", &asm(&[unknown]), 0x8000_0000, &[0x8000_0000]);
        assert!(!cfg.word_class.contains_key(&0x8000_0000));
        assert!(matches!(
            cfg.blocks[0].terminator,
            BlockTerminator::InvalidInstruction {
                pc: 0x8000_0000,
                word: 0x7801_2345
            }
        ));
    }

    #[test]
    fn unknown_delay_word_is_not_proven_code() {
        let jr_ra = 0x03e0_0008u32;
        let unknown = 0x7801_2345;
        let cfg = build_cfg("boot", &asm(&[jr_ra, unknown]), 0x8000_0000, &[0x8000_0000]);
        assert_eq!(
            cfg.word_class.get(&0x8000_0000),
            Some(&WordClass::ProvenCode)
        );
        assert!(!cfg.word_class.contains_key(&0x8000_0004));
        assert!(matches!(
            cfg.blocks[0].terminator,
            BlockTerminator::InvalidInstruction {
                pc: 0x8000_0004,
                word: 0x7801_2345
            }
        ));
    }

    #[test]
    fn final_control_word_reports_missing_delay_slot() {
        let jr_ra = 0x03e0_0008u32;
        let cfg = build_cfg("boot", &asm(&[jr_ra]), 0x8000_0000, &[0x8000_0000]);
        assert!(matches!(
            cfg.blocks[0].terminator,
            BlockTerminator::MissingDelaySlot {
                control_pc: 0x8000_0000
            }
        ));
    }

    #[test]
    fn branch_delay_slot_is_always_proven_code_and_both_edges_enqueued() {
        // beq $zero, $zero, +2 (skip one word) ; nop (delay slot, always
        // executes) ; nop (fallthrough target if not taken -- always taken
        // here since zero==zero, but this module doesn't evaluate
        // semantics, only structure) ; nop (branch target)
        let beq = 0x1000_0002u32; // opcode 0x04 (beq), rs=rt=0, imm=2
        let bytes = asm(&[beq, NOP, NOP, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let block0 = &cfg.blocks[0];
        match &block0.terminator {
            BlockTerminator::Branch {
                target,
                fallthrough,
                link,
            } => {
                assert_eq!(*fallthrough, 0x8000_0008);
                assert_eq!(*target, 0x8000_000c); // pc+4 + (2<<2) = 0+4+8 = 0xc
                assert!(!link);
            }
            other => panic!("expected Branch, got {other:?}"),
        }
        // delay slot (pc+4) is proven code regardless of branch outcome
        assert_eq!(cfg.word_class[&0x8000_0004], WordClass::ProvenCode);
    }

    #[test]
    fn branch_likely_uses_branch_likely_terminator() {
        // beql $zero, $zero, +1
        let beql = 0x5000_0001u32; // opcode 0x14 (beql)
        let bytes = asm(&[beql, NOP, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert!(matches!(
            cfg.blocks[0].terminator,
            BlockTerminator::BranchLikely { .. }
        ));
    }

    #[test]
    fn later_discovered_leader_splits_an_earlier_overlapping_scan() {
        // The taken block at 0x10 is queued before fallthrough 0x08. Without
        // canonicalization, scanning from 0x08 crosses 0x10 and overlaps the
        // already-built taken block through the return.
        let beq_to_10 = 0x1000_0003u32;
        let jr_ra = 0x03e0_0008u32;
        let bytes = asm(&[beq_to_10, NOP, NOP, NOP, NOP, jr_ra, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let starts_and_ends: Vec<_> = cfg
            .blocks
            .iter()
            .map(|block| (block.start_va, block.end_va))
            .collect();
        assert!(starts_and_ends
            .windows(2)
            .all(|pair| pair[0].1 <= pair[1].0));
        let prefix = cfg
            .blocks
            .iter()
            .find(|block| block.start_va == 0x8000_0008)
            .unwrap();
        assert_eq!(prefix.end_va, 0x8000_0010);
        assert!(matches!(
            prefix.terminator,
            BlockTerminator::Fallthrough { next: 0x8000_0010 }
        ));
    }

    #[test]
    fn indirect_target_on_a_delay_slot_does_not_sever_the_branch_delay_pair() {
        // Regression for the boot:0x800f8e90 whole-ROM-recompile blocker. A
        // computed-jump (jr) target set includes a delay-slot VA. Making that
        // VA an ordinary leader severs the branch from its delay slot, which
        // the sparse emitter correctly rejects. The target instead becomes a
        // transfer-only alias while the predecessor pair stays in one block.
        //
        // Layout: 0x08 is a branch whose delay slot is 0x0C. A jr at 0x14 has an
        // exhaustive-indirect target set that includes 0x0C.
        let beq_fwd = 0x1000_0001u32; // beq $0,$0,+1
        let jr_t9 = 0x0320_0008u32; // jr $t9 (computed jump, not $ra)
        let bytes = asm(&[
            beq_fwd, // 0x00 beq -> 0x08
            NOP,     // 0x04 (ds of 0x00)
            beq_fwd, // 0x08 beq -> 0x10  (its delay slot is 0x0C)
            NOP,     // 0x0C (ds of 0x08) -- the contested VA an indirect set names
            NOP,     // 0x10
            jr_t9,   // 0x14 computed jump
            NOP,     // 0x18 (ds of 0x14)
            NOP,     // 0x1C
        ]);
        // The jr at 0x14 resolves to {0x10, 0x0C}. 0x0C is the delay slot of
        // the branch at 0x08 and must NOT become a block start.
        let indirect = BTreeMap::from([(0x8000_0014u32, vec![0x8000_0010u32, 0x8000_000Cu32])]);
        let cfg = build_cfg_with_indirect("boot", &bytes, 0x8000_0000, &[0x8000_0000], &indirect);
        assert!(
            cfg.blocks.iter().all(|b| b.start_va != 0x8000_000C),
            "an indirect target landing on a delay slot (0x0C) was admitted as a block start, \
             severing the branch/delay pair at 0x08"
        );
        assert!(cfg
            .plain_delay_entry_aliases
            .iter()
            .any(|alias| { alias.entry_va == 0x8000_000C && alias.control_pc == 0x8000_0008 }));
        for block in &cfg.blocks {
            assert_ne!(
                block.end_va, 0x8000_000C,
                "canonicalization severed a branch/delay pair at the delay-slot VA 0x0C"
            );
        }
    }

    #[test]
    fn j_is_a_tail_transfer_not_a_call() {
        let j_target: u32 = 0x8000_0080;
        let j_word = 0x0800_0000 | ((j_target >> 2) & 0x03ff_ffff);
        let mut bytes = asm(&[j_word, NOP]);
        bytes.resize(0x100, 0);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert_eq!(cfg.tail_transfers, vec![(0x8000_0000, j_target)]);
        assert!(cfg.direct_calls.is_empty());
        assert!(!cfg.proven_roots.contains(&j_target));
        assert_eq!(cfg.word_class[&j_target], WordClass::ProvenCode);
    }

    #[test]
    fn out_of_range_target_does_not_panic_and_is_not_traversed() {
        // jal to an address far outside the bank -- must not be added as
        // a traversed root, and must not panic on the out-of-bank word
        // read.
        let far_target: u32 = 0x8fff_0000;
        let jal_word = 0x0c00_0000 | ((far_target >> 2) & 0x03ff_ffff);
        let bytes = asm(&[jal_word, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert!(!cfg.proven_roots.contains(&far_target));
        assert_eq!(cfg.direct_calls, vec![(0x8000_0000, far_target)]);
    }

    #[test]
    fn word_class_merge_is_commutative_and_proven_beats_candidate() {
        use WordClass::*;
        assert_eq!(ProvenCode.merge(CandidateData), ProvenCode);
        assert_eq!(CandidateData.merge(ProvenCode), ProvenCode);
        assert_eq!(ProvenCode.merge(ProvenData), Conflict);
        assert_eq!(ProvenData.merge(ProvenCode), Conflict);
        assert_eq!(Unknown.merge(CandidateCode), CandidateCode);
        assert_eq!(CandidateCode.merge(CandidateData), Unknown);
    }

    #[test]
    fn build_cfg_is_deterministic_across_repeated_calls() {
        let jal_target: u32 = 0x8000_0100;
        let jal_word = 0x0c00_0000 | ((jal_target >> 2) & 0x03ff_ffff);
        let mut bytes = asm(&[jal_word, NOP, NOP]);
        bytes.resize(0x200, 0);
        let cfg_a = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let cfg_b = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let json_a = serde_json::to_string(&cfg_a).unwrap();
        let json_b = serde_json::to_string(&cfg_b).unwrap();
        assert_eq!(json_a, json_b);
    }
}
