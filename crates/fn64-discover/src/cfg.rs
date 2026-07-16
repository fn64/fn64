//! Phase 4 (docs/DISCOVER-DESIGN.md "construct the CFG"): a delay-slot-aware
//! MIPS-III decoder and control-flow graph builder, scoped to exactly the
//! control-transfer classification the design doc requires -- this is not a
//! general disassembler or semantic decoder. Every word in a bank is
//! classified into one of the six states the doc names:
//!
//! ```text
//! proven_code | candidate_code | proven_data | candidate_data | conflict | unknown
//! ```
//!
//! # Why a from-scratch decoder
//!
//! No MIPS decoder exists anywhere in this workspace (`fn64-recomp` shells
//! out to N64Recomp rather than decoding itself). Writing one here is
//! unavoidable for CFG construction, but it is deliberately minimal: enough
//! to classify branches, jumps, calls, delay slots, and branch-likely
//! annulment, not to interpret arithmetic/load-store semantics (that's
//! `fn64-recomp`'s job, on the far side of proof).
//!
//! # Determinism and monotonicity
//!
//! [`build_cfg`] is a pure function of `(bank_bytes, va_start)`: same input,
//! same `Cfg` output, every time -- no I/O, no randomness. Classification
//! only ever strengthens: a word reached by a proven path is `ProvenCode`
//! even if an earlier speculative decode called it `CandidateCode` or
//! `Unknown` (see [`WordClass::merge`]).

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
    Branch { target: u32 },
    /// Branch-likely: the delay slot is annulled (not executed) when the
    /// branch is not taken, per MIPS-III semantics. This changes whether
    /// the delay-slot word is itself always-reached code.
    BranchLikely { target: u32 },
    /// `break`/`syscall`: exception-producing, terminates the block but is
    /// not a transfer to a decodable target.
    Trap,
}

/// Decode one big-endian MIPS-III word's control-flow shape. Returns `None`
/// for opcodes this module does not need to recognize as control transfers
/// (they are `ControlOp::Plain` from the caller's perspective, but `None`
/// here specifically flags "not decodable as *any* recognized encoding",
/// which is different from "decodable and ordinary" -- see [`decode_word`]).
fn classify_control(word: u32) -> ControlOp {
    let opcode = (word >> 26) & 0x3f;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let target26 = word & 0x03ff_ffff;
    let imm16 = (word & 0xffff) as i16;

    match opcode {
        // SPECIAL (opcode 0): jr/jalr live here.
        0x00 => {
            let funct = word & 0x3f;
            match funct {
                0x08 => ControlOp::Jr { rs }, // jr
                0x09 => {
                    // jalr rd, rs (rd defaults to $ra=31 when the encoded
                    // field is 0 in the two-operand assembler form, but the
                    // machine encoding always carries an explicit rd field).
                    let rd = ((word >> 11) & 0x1f) as u8;
                    ControlOp::Jalr { rd, rs }
                }
                0x0c => ControlOp::Trap, // syscall
                0x0d => ControlOp::Trap, // break
                _ => ControlOp::Plain,
            }
        }
        0x02 => ControlOp::J {
            target: target26, // caller resolves the pseudo-region + <<2
        },
        0x03 => ControlOp::Jal { target: target26 },
        // Ordinary conditional branches (non-likely).
        0x04..=0x07 /* beq/bne/blez/bgtz */ => {
            ControlOp::Branch { target: imm16 as i32 as u32 }
        }
        0x01 => {
            // REGIMM: bltz/bgez family + their "likely" siblings, by rt.
            match rt {
                0x00 | 0x01 => ControlOp::Branch { target: imm16 as i32 as u32 }, // bltz/bgez
                0x02 | 0x03 => ControlOp::BranchLikely { target: imm16 as i32 as u32 }, // bltzl/bgezl
                0x10 | 0x11 => ControlOp::Branch { target: imm16 as i32 as u32 }, // bltzal/bgezal
                0x12 | 0x13 => ControlOp::BranchLikely { target: imm16 as i32 as u32 }, // bltzall/bgezall
                _ => ControlOp::Plain,
            }
        }
        // Branch-likely forms.
        0x14..=0x17 /* beql/bnel/blezl/bgtzl */ => {
            ControlOp::BranchLikely { target: imm16 as i32 as u32 }
        }
        0x11 => {
            // COP1 (FPU): bc1f/bc1t/bc1fl/bc1tl live under a sub-opcode.
            let sub = (word >> 21) & 0x1f;
            if sub == 0x08 {
                let nd_tf = (word >> 16) & 0x3;
                match nd_tf {
                    0x0 | 0x1 => ControlOp::Branch { target: imm16 as i32 as u32 },
                    _ => ControlOp::BranchLikely { target: imm16 as i32 as u32 },
                }
            } else {
                ControlOp::Plain
            }
        }
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
fn region_target(pc: u32, target26: u32) -> u32 {
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasicBlock {
    pub start_va: u32,
    /// Exclusive end, i.e. one past the last word (delay slot included).
    pub end_va: u32,
    pub terminator: BlockTerminator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    Branch { target: u32, fallthrough: u32 },
    /// Branch-likely: same two successors, but the fallthrough edge does
    /// NOT execute the delay slot (annulment) -- recorded so callers don't
    /// treat the delay-slot word as unconditionally reached.
    BranchLikely { target: u32, fallthrough: u32 },
    /// `jr $ra`: an ordinary return, terminates this path with no known
    /// successor.
    Return,
    /// `jr $rs` (rs != $ra) or `jalr`: computed transfer, target(s)
    /// unresolved by this phase -- Phase 6's job. Recorded as an open
    /// indirect site here, not silently dropped.
    Indirect { via_call: bool },
    /// `break`/`syscall`: terminates the block, no successor.
    Trap,
    /// Ran off the end of the decodable/bank region without a terminator.
    RanOffEnd,
}

/// One indirect control-transfer site the CFG could not resolve -- carried
/// forward for Phase 6 (this crate does not implement Phase 6 yet; this
/// struct is the fact record it will consume).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndirectSite {
    pub pc: u32,
    pub via_call: bool,
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
                ControlOp::J { target } => {
                    let target = region_target(pc, target);
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    let delay_pc = pc.wrapping_add(4);
                    mark(&mut word_class, delay_pc, WordClass::ProvenCode);
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
                    let delay_pc = pc.wrapping_add(4);
                    mark(&mut word_class, delay_pc, WordClass::ProvenCode);
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
                    let delay_pc = pc.wrapping_add(4);
                    mark(&mut word_class, delay_pc, WordClass::ProvenCode);
                    let end = delay_pc.wrapping_add(4);
                    let is_return = rs == 31; // $ra
                    let terminator = if is_return {
                        BlockTerminator::Return
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
                ControlOp::Jalr { rd: _, rs: _ } => {
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    let delay_pc = pc.wrapping_add(4);
                    mark(&mut word_class, delay_pc, WordClass::ProvenCode);
                    let next = delay_pc.wrapping_add(4);
                    indirect_sites.push(IndirectSite { pc, via_call: true });
                    if in_range(next) {
                        worklist.push_back(next);
                    }
                    blocks.push(BasicBlock {
                        start_va: block_start,
                        end_va: next,
                        terminator: BlockTerminator::Indirect { via_call: true },
                    });
                    break;
                }
                ControlOp::Branch { target } => {
                    let target = branch_target(pc, target);
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    let delay_pc = pc.wrapping_add(4);
                    // Ordinary branch: delay slot always executes (both
                    // taken and not-taken paths run it), so it's
                    // unconditionally proven code.
                    mark(&mut word_class, delay_pc, WordClass::ProvenCode);
                    let fallthrough = delay_pc.wrapping_add(4);
                    if in_range(target) {
                        worklist.push_back(target);
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
                        },
                    });
                    break;
                }
                ControlOp::BranchLikely { target } => {
                    let target = branch_target(pc, target);
                    mark(&mut word_class, pc, WordClass::ProvenCode);
                    let delay_pc = pc.wrapping_add(4);
                    // Branch-likely annulment: the delay slot only executes
                    // on the TAKEN path. It is still proven code (reached
                    // via the taken edge) but must not be treated as
                    // unconditionally-executed fallthrough code -- callers
                    // needing that distinction should consult the
                    // `BranchLikely` terminator, not assume `ProvenCode`
                    // implies "always executes".
                    mark(&mut word_class, delay_pc, WordClass::ProvenCode);
                    let fallthrough = delay_pc.wrapping_add(4);
                    if in_range(target) {
                        worklist.push_back(target);
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
                        },
                    });
                    break;
                }
            }
        }
    }

    Cfg {
        bank: bank.to_string(),
        word_class,
        blocks,
        direct_calls,
        tail_transfers,
        indirect_sites,
        proven_roots,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    const NOP: u32 = 0x0000_0000; // sll $zero, $zero, 0

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
            } => {
                assert_eq!(*fallthrough, 0x8000_0008);
                assert_eq!(*target, 0x8000_000c); // pc+4 + (2<<2) = 0+4+8 = 0xc
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
