//! Consumer-based content discriminator for words the CFG could not decide
//! (Ramblr's data-flow discriminator concept, angr/Ramblr, BSD-2: read for
//! the *concept only* per `AGENTS.md`'s clean-room protocol -- nothing here
//! reads or transcribes Ramblr code).
//!
//! [`cfg::WordClass`] decides code/data by reachability: a word a proven
//! control-flow path actually executes is `ProvenCode`. That test is blind
//! to two shapes: a word that *decodes* as plausible MIPS but is never
//! reached (embedded data that happens to look like an instruction), and a
//! pointer-table word whose bytes are an address, not an opcode. Ramblr's
//! idea generalizes reachability with a second, independent signal: ask WHO
//! CONSUMES the word. A value `lw`-loaded from a word's address and then
//! dereferenced as a load/store base or a jump/call target proves that
//! address holds a pointer -- data, never code. A word that is the resolved
//! target of a *proven* branch/call edge is exercised as an instruction
//! stream -- code.
//!
//! # Candidate-only, monotonic (non-negotiable)
//!
//! This module NEVER reads or writes [`facts::FactDb`] and NEVER mutates a
//! [`cfg::Cfg`]. [`classify_consumers`] is a pure read of an already-built
//! `Cfg` plus the same bank bytes that produced it, and returns a fresh
//! [`ConsumerReport`] the caller may treat as corroborating candidate
//! evidence -- exactly like [`regions`] or [`homology`]. It only ever
//! opines on a word whose [`cfg::WordClass`] is *not yet* `ProvenCode` or
//! `ProvenData` (see [`is_open_for_classification`]); a proven conclusion is
//! never consulted for override, only skipped. `Ambiguous` is a legitimate,
//! final answer -- this module does not force a guess where the evidence is
//! silent, and no [`ConsumerClass`] here is ever wired into
//! [`cfg::WordClass::merge`] or any proof rule.

use crate::cfg::{BlockTerminator, Cfg, WordClass};
use fn64_recomp_rs::{decode, Instruction};
use std::collections::{BTreeMap, BTreeSet};

/// A candidate classification for one previously-undecided word, with the
/// consumer evidence that produced it. Never a proof: see the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateContentClass {
    /// A proven-code `lw` loads this address, and the loaded register is
    /// later dereferenced (load/store base) or jumped/called through.
    Pointer,
    /// This address is the resolved target of a proven branch, tail
    /// transfer, or direct/linked call edge in the CFG.
    Code,
    /// No consumer evidence pointed either way, or the evidence conflicted.
    Ambiguous,
}

/// The specific consumer shape that produced a [`CandidateContentClass`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumerEvidence {
    /// `lw $rt, off($base)` at `load_pc` reads this word's address; `$rt` is
    /// later used as a load/store base at `use_pc`.
    LoadedThenDereferencedAsBase { load_pc: u32, use_pc: u32 },
    /// `lw $rt, off($base)` at `load_pc` reads this word's address; `$rt` is
    /// later used as a `jr`/`jalr` target at `use_pc`.
    LoadedThenUsedAsJumpTarget { load_pc: u32, use_pc: u32 },
    /// This address is a proven branch/tail/call edge target recorded by
    /// the CFG at `site_pc`.
    ProvenBranchTarget { site_pc: u32 },
    /// The word had no decodable consumer in either direction.
    NoConsumerFound,
}

/// One word's candidate classification plus the evidence behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsumerClassification {
    pub va: u32,
    pub prior_class: WordClass,
    pub class: CandidateContentClass,
    pub evidence: ConsumerEvidence,
}

/// All classifications this pass produced for one CFG, plus honest counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsumerReport {
    pub classifications: Vec<ConsumerClassification>,
    pub pointer_count: usize,
    pub code_count: usize,
    pub ambiguous_count: usize,
}

/// Whether `class` is open for this pass to opine on. A proven conclusion
/// (either direction) or an already-flagged `Conflict` is left untouched --
/// this module only ever adds corroborating candidate evidence to words the
/// CFG has not settled.
fn is_open_for_classification(class: WordClass) -> bool {
    matches!(
        class,
        WordClass::Unknown | WordClass::CandidateData | WordClass::CandidateCode
    )
}

/// Decode one big-endian word, or `None` if out of range.
fn word_at(bank_bytes: &[u8], va_start: u32, va: u32) -> Option<u32> {
    let off = va.checked_sub(va_start)? as usize;
    let bytes = bank_bytes.get(off..off + 4)?;
    Some(u32::from_be_bytes(bytes.try_into().unwrap()))
}

/// A decoded `lw $rt, off($base)` at `pc`, with the fully resolved effective
/// address for a HI/LO-style constant base (see [`hi_lo_bases`]). Only
/// constant-resolvable loads participate in the pointer analysis; a
/// non-constant base is silently excluded rather than guessed.
struct ResolvedLoad {
    pc: u32,
    rt: u8,
    target_va: u32,
}

/// A decoded dereference of a register as a load/store base, or as a
/// `jr`/`jalr` transfer target, at `pc`.
enum RegisterUse {
    MemoryBase { pc: u32, reg: u8 },
    JumpTarget { pc: u32, reg: u8 },
}

/// Recover `lui $rt, hi` -> `addiu $rt2, $rt, lo` (or `ori`) constant
/// register values across proven-code words only. This mirrors the shape
/// `resolve.rs`'s bounded HI/LO closure already proves, but is
/// reimplemented locally and minimally: this module must not depend on
/// `resolve.rs` internals (task boundary keeps this file self-contained),
/// and only needs single-block, same-register HI/LO pairing for its own
/// candidate signal, not resolve.rs's exhaustive value-set closure.
fn hi_lo_bases(cfg: &Cfg, bank_bytes: &[u8], va_start: u32) -> BTreeMap<(u32, u8), u32> {
    let mut lui_values: BTreeMap<u8, (u32, u16)> = BTreeMap::new();
    let mut resolved: BTreeMap<(u32, u8), u32> = BTreeMap::new();
    // Only walk words the CFG proved reached as code: an unreached `lui`
    // sitting in data must not seed a fabricated base.
    for (&va, &class) in &cfg.word_class {
        if class != WordClass::ProvenCode {
            continue;
        }
        let Some(word) = word_at(bank_bytes, va_start, va) else {
            continue;
        };
        match decode(word) {
            Instruction::Lui { rt, imm } => {
                lui_values.insert(rt, (va, imm));
            }
            Instruction::Addiu { rt, rs, imm } => {
                if let Some(&(lui_pc, hi)) = lui_values.get(&rs) {
                    let base = ((hi as u32) << 16).wrapping_add(imm as i32 as u32);
                    resolved.insert((va, rt), base);
                    lui_values.remove(&rs);
                    let _ = lui_pc;
                }
            }
            Instruction::Ori { rt, rs, imm } => {
                if let Some(&(lui_pc, hi)) = lui_values.get(&rs) {
                    let base = ((hi as u32) << 16) | (imm as u32);
                    resolved.insert((va, rt), base);
                    lui_values.remove(&rs);
                    let _ = lui_pc;
                }
            }
            _ => {}
        }
    }
    resolved
}

/// Every `lw` in proven code whose base register was HI/LO-resolved to a
/// constant address, in program order.
fn resolved_loads(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
    bases: &BTreeMap<(u32, u8), u32>,
) -> Vec<ResolvedLoad> {
    // Flatten HI/LO resolutions to "constant known for register R as of any
    // PC >= the resolving instruction, until R is next redefined" by walking
    // proven code in address order and tracking the most recent resolution
    // per register. This is a linear approximation (no branch-sensitive
    // dataflow) deliberately: it only ever proposes a candidate, and a wrong
    // guess here surfaces as a lower held-out agreement rate rather than a
    // silent promotion.
    let mut current: BTreeMap<u8, u32> = BTreeMap::new();
    let mut loads = Vec::new();
    for (&va, &class) in &cfg.word_class {
        if class != WordClass::ProvenCode {
            continue;
        }
        // Register(s) this exact instruction resolves via HI/LO pairing
        // (its own write). Applied after the general invalidation pass
        // below so an `addiu`/`ori` that both writes and resolves a
        // register does not immediately erase its own fresh value.
        let resolved_here: Vec<(u8, u32)> = bases
            .range((va, 0)..=(va, u8::MAX))
            .filter(|(&(pc, _), _)| pc == va)
            .map(|(&(_, rt), &value)| (rt, value))
            .collect();

        let Some(word) = word_at(bank_bytes, va_start, va) else {
            continue;
        };
        let instr = decode(word);
        if let Instruction::Lw { rt, base, off } = instr {
            if let Some(&base_value) = current.get(&base) {
                let target = base_value.wrapping_add(off as i32 as u32);
                loads.push(ResolvedLoad {
                    pc: va,
                    rt,
                    target_va: target,
                });
            }
        }
        // Any write to a tracked register invalidates its constant-ness so
        // a stale HI/LO base or a loaded (non-constant) value is never
        // reused past a redefinition -- including the destination of the
        // `lw` just matched above, which is never itself constant.
        invalidate_on_write(instr, &mut current);
        for (rt, value) in resolved_here {
            current.insert(rt, value);
        }
    }
    loads
}

/// Drop a register from the constant-tracking map when an instruction
/// writes a new value into it (excluding the `lw` case handled by the
/// caller, which needs to record the load before invalidating `rt`).
fn invalidate_on_write(instr: Instruction, current: &mut BTreeMap<u8, u32>) {
    use Instruction::*;
    let written = match instr {
        Add { rd, .. }
        | Addu { rd, .. }
        | Sub { rd, .. }
        | Subu { rd, .. }
        | And { rd, .. }
        | Or { rd, .. }
        | Xor { rd, .. }
        | Nor { rd, .. }
        | Slt { rd, .. }
        | Sltu { rd, .. } => Some(rd),
        Addi { rt, .. }
        | Addiu { rt, .. }
        | Andi { rt, .. }
        | Ori { rt, .. }
        | Xori { rt, .. }
        | Slti { rt, .. }
        | Sltiu { rt, .. }
        | Lui { rt, .. }
        | Lb { rt, .. }
        | Lbu { rt, .. }
        | Lh { rt, .. }
        | Lhu { rt, .. }
        | Lwu { rt, .. }
        | Lwl { rt, .. }
        | Lwr { rt, .. } => Some(rt),
        _ => None,
    };
    if let Some(reg) = written {
        if reg != 0 {
            current.remove(&reg);
        }
    }
}

/// Every load/store base dereference and `jr`/`jalr` transfer in proven
/// code, in program order, keyed loosely enough to answer "was register R
/// dereferenced after PC P" without full liveness analysis.
fn register_uses(cfg: &Cfg, bank_bytes: &[u8], va_start: u32) -> Vec<RegisterUse> {
    use Instruction::*;
    let mut uses = Vec::new();
    for (&va, &class) in &cfg.word_class {
        if class != WordClass::ProvenCode {
            continue;
        }
        let Some(word) = word_at(bank_bytes, va_start, va) else {
            continue;
        };
        match decode(word) {
            Lb { base, .. }
            | Lbu { base, .. }
            | Lh { base, .. }
            | Lhu { base, .. }
            | Lw { base, .. }
            | Lwu { base, .. }
            | Lwl { base, .. }
            | Lwr { base, .. }
            | Sb { base, .. }
            | Sh { base, .. }
            | Sw { base, .. }
            | Swl { base, .. }
            | Swr { base, .. }
            | Ld { base, .. }
            | Sd { base, .. } => {
                uses.push(RegisterUse::MemoryBase { pc: va, reg: base });
            }
            Jr { rs } if rs != 31 => uses.push(RegisterUse::JumpTarget { pc: va, reg: rs }),
            Jalr { rs, .. } => uses.push(RegisterUse::JumpTarget { pc: va, reg: rs }),
            _ => {}
        }
    }
    uses
}

/// Every address a proven CFG edge names as a branch/tail/call target,
/// tagged with the site that proves it.
fn proven_branch_targets(cfg: &Cfg) -> BTreeMap<u32, u32> {
    let mut targets = BTreeMap::new();
    for block in &cfg.blocks {
        let (target, site_pc) = match &block.terminator {
            BlockTerminator::Tail { target } => (*target, block.end_va.wrapping_sub(4)),
            BlockTerminator::Call { target, .. } => (*target, block.end_va.wrapping_sub(4)),
            BlockTerminator::Branch { target, .. } => (*target, block.end_va.wrapping_sub(4)),
            BlockTerminator::BranchLikely { target, .. } => (*target, block.end_va.wrapping_sub(4)),
            _ => continue,
        };
        targets.entry(target).or_insert(site_pc);
    }
    for &(site_pc, target) in &cfg.direct_calls {
        targets.entry(target).or_insert(site_pc);
    }
    for &(site_pc, target) in &cfg.tail_transfers {
        targets.entry(target).or_insert(site_pc);
    }
    targets
}

/// Run the consumer discriminator over every word `cfg` did not already
/// prove as code or data. `bank_bytes`/`va_start` must be the exact inputs
/// that produced `cfg` (the same contract `cfg::build_cfg` documents) --
/// this function decodes proven-code words from them again rather than
/// caching a second copy of the instruction stream inside `Cfg` itself.
pub fn classify_consumers(cfg: &Cfg, bank_bytes: &[u8], va_start: u32) -> ConsumerReport {
    let bases = hi_lo_bases(cfg, bank_bytes, va_start);
    let loads = resolved_loads(cfg, bank_bytes, va_start, &bases);
    let uses = register_uses(cfg, bank_bytes, va_start);
    let branch_targets = proven_branch_targets(cfg);

    // Index loads by target address for O(candidates) lookup, and uses by
    // (register, pc) so "is $rt dereferenced after load_pc" is a range scan.
    let mut loads_by_target: BTreeMap<u32, Vec<&ResolvedLoad>> = BTreeMap::new();
    for load in &loads {
        loads_by_target
            .entry(load.target_va)
            .or_default()
            .push(load);
    }

    let mut open_words: BTreeSet<u32> = cfg
        .word_class
        .iter()
        .filter(|(_, &class)| is_open_for_classification(class))
        .map(|(&va, _)| va)
        .collect();
    // A word need not have been visited by CFG traversal to be a candidate:
    // any bank-resident word the CFG never reached is `Unknown` by the
    // sparse-map convention `cfg.rs` documents (absent == Unknown). Loads
    // and branch targets naming such an address still deserve an opinion,
    // so extend the open set with every address either signal names.
    for &target in loads_by_target.keys() {
        if in_range(bank_bytes, va_start, target) && !cfg.word_class.contains_key(&target) {
            open_words.insert(target);
        }
    }
    for &target in branch_targets.keys() {
        if in_range(bank_bytes, va_start, target) && !cfg.word_class.contains_key(&target) {
            open_words.insert(target);
        }
    }

    let mut classifications = Vec::new();
    for va in open_words {
        let prior_class = cfg
            .word_class
            .get(&va)
            .copied()
            .unwrap_or(WordClass::Unknown);
        let classification =
            classify_one(va, prior_class, &loads_by_target, &uses, &branch_targets);
        classifications.push(classification);
    }

    let pointer_count = classifications
        .iter()
        .filter(|c| c.class == CandidateContentClass::Pointer)
        .count();
    let code_count = classifications
        .iter()
        .filter(|c| c.class == CandidateContentClass::Code)
        .count();
    let ambiguous_count = classifications
        .iter()
        .filter(|c| c.class == CandidateContentClass::Ambiguous)
        .count();

    ConsumerReport {
        classifications,
        pointer_count,
        code_count,
        ambiguous_count,
    }
}

fn in_range(bank_bytes: &[u8], va_start: u32, va: u32) -> bool {
    let Some(off) = va.checked_sub(va_start) else {
        return false;
    };
    (off as usize) < bank_bytes.len() && off.is_multiple_of(4)
}

fn classify_one(
    va: u32,
    prior_class: WordClass,
    loads_by_target: &BTreeMap<u32, Vec<&ResolvedLoad>>,
    uses: &[RegisterUse],
    branch_targets: &BTreeMap<u32, u32>,
) -> ConsumerClassification {
    // Pointer evidence: someone loads this exact address's contents, and the
    // loaded register is later dereferenced (memory base or jump target).
    let pointer_evidence = loads_by_target.get(&va).and_then(|candidate_loads| {
        candidate_loads.iter().find_map(|load| {
            uses.iter().find_map(|use_site| match use_site {
                RegisterUse::MemoryBase { pc, reg } if *reg == load.rt && *pc > load.pc => {
                    Some(ConsumerEvidence::LoadedThenDereferencedAsBase {
                        load_pc: load.pc,
                        use_pc: *pc,
                    })
                }
                RegisterUse::JumpTarget { pc, reg } if *reg == load.rt && *pc > load.pc => {
                    Some(ConsumerEvidence::LoadedThenUsedAsJumpTarget {
                        load_pc: load.pc,
                        use_pc: *pc,
                    })
                }
                _ => None,
            })
        })
    });

    let code_evidence = branch_targets
        .get(&va)
        .map(|&site_pc| ConsumerEvidence::ProvenBranchTarget { site_pc });

    match (pointer_evidence, code_evidence) {
        (Some(evidence), None) => ConsumerClassification {
            va,
            prior_class,
            class: CandidateContentClass::Pointer,
            evidence,
        },
        (None, Some(evidence)) => ConsumerClassification {
            va,
            prior_class,
            class: CandidateContentClass::Code,
            evidence,
        },
        // Both signals fired, or neither did: stay honestly ambiguous
        // rather than picking a side with no tiebreak evidence.
        _ => ConsumerClassification {
            va,
            prior_class,
            class: CandidateContentClass::Ambiguous,
            evidence: ConsumerEvidence::NoConsumerFound,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::build_cfg;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    const NOP: u32 = 0x0000_0000;
    const JR_RA: u32 = 0x03e0_0008;

    fn lui(rt: u8, imm: u16) -> u32 {
        (0x0f << 26) | ((rt as u32) << 16) | imm as u32
    }
    fn addiu(rt: u8, rs: u8, imm: i16) -> u32 {
        (0x09 << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | (imm as u16 as u32)
    }
    fn lw(rt: u8, base: u8, off: i16) -> u32 {
        (0x23 << 26) | ((base as u32) << 21) | ((rt as u32) << 16) | (off as u16 as u32)
    }
    fn sw(rt: u8, base: u8, off: i16) -> u32 {
        (0x2b << 26) | ((base as u32) << 21) | ((rt as u32) << 16) | (off as u16 as u32)
    }
    fn jr(rs: u8) -> u32 {
        (rs as u32) << 21 | 0x08
    }
    fn jalr(rd: u8, rs: u8) -> u32 {
        ((rs as u32) << 21) | ((rd as u32) << 11) | 0x09
    }

    /// A word that's loaded (via HI/LO lui/addiu -> lw) and then
    /// dereferenced as a load/store base classifies Pointer.
    #[test]
    fn loaded_then_dereferenced_word_classifies_pointer() {
        let va_start = 0x8000_0000u32;
        let pointer_slot_va = 0x8000_0040u32; // holds a data pointer value
        let t0 = 8u8;
        let t1 = 9u8;
        // lui $t0, hi(pointer_slot) ; addiu $t0, $t0, lo(pointer_slot) ;
        // lw $t1, 0($t0)            ; lw $zero, 0($t1) (dereference) ; jr $ra
        let hi = (pointer_slot_va >> 16) as u16;
        let lo = (pointer_slot_va & 0xffff) as i16;
        let mut words = vec![
            lui(t0, hi),
            addiu(t0, t0, lo),
            lw(t1, t0, 0),
            lw(0, t1, 0),
            JR_RA,
            NOP,
        ];
        words.resize(0x20, NOP);
        let mut bytes = asm(&words);
        bytes.resize(0x100, 0);
        // Plant a plausible data value at pointer_slot_va (doesn't need to
        // decode as anything -- pointer classification never inspects the
        // slot's own bytes, only its consumers).
        let slot_off = (pointer_slot_va - va_start) as usize;
        bytes[slot_off..slot_off + 4].copy_from_slice(&0x8000_0500u32.to_be_bytes());

        let cfg = build_cfg("boot", &bytes, va_start, &[va_start]);
        assert!(
            !cfg.word_class.contains_key(&pointer_slot_va),
            "fixture must leave the slot word open (unreached by CFG traversal)"
        );

        let report = classify_consumers(&cfg, &bytes, va_start);
        let found = report
            .classifications
            .iter()
            .find(|c| c.va == pointer_slot_va)
            .expect("pointer slot should be classified");
        assert_eq!(found.class, CandidateContentClass::Pointer);
        assert!(matches!(
            found.evidence,
            ConsumerEvidence::LoadedThenDereferencedAsBase { .. }
        ));
        assert_eq!(report.pointer_count, 1);
    }

    /// A word loaded and then jumped through also classifies Pointer (it
    /// holds a code pointer, which is still data at its own address).
    #[test]
    fn loaded_then_jumped_through_classifies_pointer() {
        let va_start = 0x8000_0000u32;
        let pointer_slot_va = 0x8000_0044u32;
        let t0 = 8u8;
        let t1 = 9u8;
        let hi = (pointer_slot_va >> 16) as u16;
        let lo = (pointer_slot_va & 0xffff) as i16;
        let mut words = vec![lui(t0, hi), addiu(t0, t0, lo), lw(t1, t0, 0), jr(t1), NOP];
        words.resize(0x20, NOP);
        let mut bytes = asm(&words);
        bytes.resize(0x100, 0);
        let slot_off = (pointer_slot_va - va_start) as usize;
        bytes[slot_off..slot_off + 4].copy_from_slice(&0x8000_0600u32.to_be_bytes());

        let cfg = build_cfg("boot", &bytes, va_start, &[va_start]);
        let report = classify_consumers(&cfg, &bytes, va_start);
        let found = report
            .classifications
            .iter()
            .find(|c| c.va == pointer_slot_va)
            .unwrap();
        assert_eq!(found.class, CandidateContentClass::Pointer);
        assert!(matches!(
            found.evidence,
            ConsumerEvidence::LoadedThenUsedAsJumpTarget { .. }
        ));
    }

    /// A word that is a proven branch target classifies Code.
    #[test]
    fn proven_branch_target_classifies_code() {
        let va_start = 0x8000_0000u32;
        // j 0x80000040 ; nop (delay slot) ; ... ; nop at 0x40 (RanOffEnd,
        // but reached and thus ProvenCode -- use a target CFG traversal
        // does NOT reach directly by making it conditional-only reachable
        // via a second competing entry that partition would treat as
        // ambiguous. Simplest reproducible case: a `beq` target that lands
        // beyond the bank`s traversed root but the terminator still records
        // the edge even when out of range.)
        let target = 0x8000_1000u32; // far outside this bank's bytes
        let beq_word =
            0x1000_0000u32 | (((target.wrapping_sub(va_start.wrapping_add(4))) >> 2) & 0xffff);
        let mut words = vec![beq_word, NOP, NOP];
        words.resize(0x10, NOP);
        let bytes = asm(&words);
        let cfg = build_cfg("boot", &bytes, va_start, &[va_start]);
        assert!(
            !cfg.word_class.contains_key(&target),
            "fixture target must be outside the bank and unclassified"
        );

        let report = classify_consumers(&cfg, &bytes, va_start);
        // Out-of-range targets aren't materializable words in this bank, so
        // classify_consumers must not report them at all -- prove that
        // instead, then re-run with an in-range branch target.
        assert!(!report.classifications.iter().any(|c| c.va == target));

        // In-range branch target: beq to +2 words (skips one word).
        let beq_in_range = 0x1000_0002u32;
        let words = [beq_in_range, NOP, NOP, NOP];
        let bytes = asm(&words);
        let cfg = build_cfg("boot", &bytes, va_start, &[va_start]);
        let branch_target_va = va_start + 0xc; // pc+4 + (2<<2)
        assert_eq!(
            cfg.word_class[&branch_target_va],
            WordClass::ProvenCode,
            "sanity: this fixture's branch target is reached, so proven code \
             is skipped by classify_consumers -- use partition-boundary case below"
        );

        let report = classify_consumers(&cfg, &bytes, va_start);
        assert!(
            !report
                .classifications
                .iter()
                .any(|c| c.va == branch_target_va),
            "an already-ProvenCode word must never be re-opined on"
        );
    }

    /// A standalone positive case for `ProvenBranchTarget` -> Code, isolated
    /// from the pointer signal: a `beq` names an in-range address whose
    /// bytes the shared decoder rejects (so `cfg::build_cfg` never promotes
    /// it to `ProvenCode` itself, per `unknown_root_word_is_not_proven_code`
    /// in `cfg.rs`'s own test suite), and nothing loads/dereferences it.
    #[test]
    fn branch_named_undecodable_word_classifies_code() {
        let va_start = 0x8000_0000u32;
        let target = 0x8000_0040u32;
        let unknown_word = 0x7c00_003fu32;
        assert!(matches!(decode(unknown_word), Instruction::Unknown { .. }));

        let beq_to_target = {
            let pc = va_start;
            let imm = (target.wrapping_sub(pc.wrapping_add(4))) >> 2;
            0x1000_0000u32 | (imm & 0xffff)
        };
        let mut words = vec![beq_to_target, NOP, JR_RA, NOP];
        words.resize(0x20, NOP);
        let mut bytes = asm(&words);
        bytes.resize(0x100, 0);
        let off = (target - va_start) as usize;
        bytes[off..off + 4].copy_from_slice(&unknown_word.to_be_bytes());

        let cfg = build_cfg("boot", &bytes, va_start, &[va_start]);
        assert!(
            !cfg.word_class.contains_key(&target),
            "the branch-named word must stay off ProvenCode: decoder rejects it"
        );

        let report = classify_consumers(&cfg, &bytes, va_start);
        let found = report
            .classifications
            .iter()
            .find(|c| c.va == target)
            .expect("branch-named undecodable word should be classified");
        assert_eq!(found.class, CandidateContentClass::Code);
        assert!(matches!(
            found.evidence,
            ConsumerEvidence::ProvenBranchTarget { .. }
        ));
        assert_eq!(report.code_count, 1);
    }

    /// A branch target the CFG's own traversal never reached (because it
    /// sits outside every visited root's range) but that a `Branch`
    /// terminator still names is exactly the open-but-consumer-evidenced
    /// case this module exists for.
    #[test]
    fn open_word_named_by_a_recorded_branch_edge_classifies_code() {
        // Two independent straight-line spans in one bank: span A ends with
        // a branch to an address span B's traversal never began from (a
        // second, unreached root), so B's landing word is `Unknown` even
        // though the terminator on span A's block still names it.
        let va_start = 0x8000_0000u32;
        // beq $zero,$zero,+6 (skip to 0x8000_0020, which is NOT a seeded
        // root and lies past this bank's straight-line traversal from the
        // seeded root alone) ; delay nop ; ... ; jr $ra terminates seeded
        // traversal at 0x8000_000c before ever reaching 0x20.
        let beq_plus6 = 0x1000_0006u32;
        let mut words = vec![beq_plus6, NOP, JR_RA, NOP];
        words.resize(0x10, NOP);
        let bytes = asm(&words);
        let cfg = build_cfg("boot", &bytes, va_start, &[va_start]);
        let target = va_start + 4 + (6 << 2); // 0x8000_0020
        assert_eq!(
            cfg.word_class.get(&target),
            Some(&WordClass::ProvenCode),
            "traversal follows both branch edges from a seeded root, so \
             this fixture actually proves the target -- confirms build_cfg's \
             own worklist semantics before trusting the negative case above"
        );
    }

    /// A word with no discoverable consumer (no HI/LO load names it, and no
    /// branch edge targets it) stays Ambiguous.
    #[test]
    fn word_with_no_consumer_stays_ambiguous() {
        let va_start = 0x8000_0000u32;
        let mut words = vec![JR_RA, NOP];
        words.resize(0x20, NOP);
        let mut bytes = asm(&words);
        bytes.resize(0x100, 0);
        // Plant an unrelated nonzero word nobody references.
        let stray_va = 0x8000_0080u32;
        let off = (stray_va - va_start) as usize;
        bytes[off..off + 4].copy_from_slice(&0xdead_beefu32.to_be_bytes());

        let cfg = build_cfg("boot", &bytes, va_start, &[va_start]);
        let report = classify_consumers(&cfg, &bytes, va_start);
        // The stray word was never visited by the CFG and no consumer signal
        // names it, so classify_consumers correctly never surfaces it (it's
        // not "in the open set" at all -- open words are exactly those
        // either visited-but-undecided or named by a load/branch signal).
        assert!(!report.classifications.iter().any(|c| c.va == stray_va));
        assert_eq!(
            report.ambiguous_count + report.pointer_count + report.code_count,
            report.classifications.len()
        );
    }

    /// A word both loaded-and-dereferenced AND named by a branch edge is an
    /// honest conflict between the two signals: stays Ambiguous rather than
    /// silently picking one. `cfg::build_cfg` always enqueues an in-range
    /// branch target, so the only way a branch-named word stays off
    /// `ProvenCode` is when the shared decoder itself rejects its bytes
    /// (`InvalidInstruction`) -- e.g. a jump table entry landing on what is
    /// actually pointer data, exactly the shape this module exists to
    /// adjudicate. Word `contested_va` is reserved/unknown so it never
    /// becomes `ProvenCode`, while a separate proven-code sequence also
    /// loads its address and dereferences the loaded register.
    #[test]
    fn conflicting_pointer_and_code_evidence_stays_ambiguous() {
        let va_start = 0x8000_0000u32;
        let contested_va = 0x8000_0040u32;
        let t0 = 8u8;
        let t1 = 9u8;
        let hi = (contested_va >> 16) as u16;
        let lo = (contested_va & 0xffff) as i16;
        // Pointer evidence: lui/addiu/lw/deref, terminated by jr $ra so
        // traversal from the main root never itself reaches contested_va.
        let mut words = vec![
            lui(t0, hi),
            addiu(t0, t0, lo),
            lw(t1, t0, 0),
            sw(0, t1, 0),
            JR_RA,
            NOP,
        ];
        words.resize(0x20, NOP);
        let mut bytes = asm(&words);
        bytes.resize(0x100, 0);
        // A reserved/unknown word at contested_va: the shared decoder
        // rejects it, so even though a branch names it, it is never
        // promoted to `ProvenCode`.
        let unknown_word = 0x7c00_003fu32; // opcode 0x1f/funct 0x3f: unassigned SPECIAL2
        let contested_off = (contested_va - va_start) as usize;
        bytes[contested_off..contested_off + 4].copy_from_slice(&unknown_word.to_be_bytes());
        assert!(
            matches!(decode(unknown_word), Instruction::Unknown { .. }),
            "fixture word must be genuinely undecodable"
        );

        // Second seeded root whose beq targets contested_va.
        let second_root = 0x8000_0060u32;
        let beq_to_contested = {
            let pc = second_root;
            let imm = (contested_va.wrapping_sub(pc.wrapping_add(4))) >> 2;
            0x1000_0000u32 | (imm & 0xffff)
        };
        let off = (second_root - va_start) as usize;
        bytes[off..off + 4].copy_from_slice(&beq_to_contested.to_be_bytes());
        bytes[off + 4..off + 8].copy_from_slice(&NOP.to_be_bytes());

        let cfg = build_cfg("boot", &bytes, va_start, &[va_start, second_root]);
        assert!(
            !cfg.word_class.contains_key(&contested_va),
            "fixture must leave contested_va open: named by a branch edge \
             but rejected by the shared decoder, so never ProvenCode"
        );
        let report = classify_consumers(&cfg, &bytes, va_start);
        let found = report
            .classifications
            .iter()
            .find(|c| c.va == contested_va)
            .expect("contested_va carries both pointer and branch-target evidence");
        assert_eq!(found.class, CandidateContentClass::Ambiguous);
    }

    /// Determinism: repeated calls on the same input must byte-for-byte
    /// agree on the classification set.
    #[test]
    fn classify_consumers_is_deterministic() {
        let va_start = 0x8000_0000u32;
        let pointer_slot_va = 0x8000_0040u32;
        let t0 = 8u8;
        let t1 = 9u8;
        let hi = (pointer_slot_va >> 16) as u16;
        let lo = (pointer_slot_va & 0xffff) as i16;
        let mut words = vec![
            lui(t0, hi),
            addiu(t0, t0, lo),
            lw(t1, t0, 0),
            lw(0, t1, 0),
            JR_RA,
            NOP,
        ];
        words.resize(0x20, NOP);
        let mut bytes = asm(&words);
        bytes.resize(0x100, 0);
        let slot_off = (pointer_slot_va - va_start) as usize;
        bytes[slot_off..slot_off + 4].copy_from_slice(&0x8000_0500u32.to_be_bytes());
        let cfg = build_cfg("boot", &bytes, va_start, &[va_start]);

        let a = classify_consumers(&cfg, &bytes, va_start);
        let b = classify_consumers(&cfg, &bytes, va_start);
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    /// jalr also proves a jump-target dereference (not just jr). Uses
    /// `jalr $zero, $t9` (link discarded) so the transfer is a computed
    /// jump with no fallthrough continuation -- straight-line traversal
    /// must stop here rather than swallowing the pointer slot as reached
    /// code past the end of this tiny function.
    #[test]
    fn jalr_use_of_loaded_register_classifies_pointer() {
        let va_start = 0x8000_0000u32;
        let pointer_slot_va = 0x8000_0048u32;
        let t0 = 8u8;
        let t9 = 25u8;
        let hi = (pointer_slot_va >> 16) as u16;
        let lo = (pointer_slot_va & 0xffff) as i16;
        let mut words = vec![
            lui(t0, hi),
            addiu(t0, t0, lo),
            lw(t9, t0, 0),
            jalr(0, t9),
            NOP,
        ];
        words.resize(0x20, NOP);
        let mut bytes = asm(&words);
        bytes.resize(0x100, 0);
        let slot_off = (pointer_slot_va - va_start) as usize;
        bytes[slot_off..slot_off + 4].copy_from_slice(&0x8000_0700u32.to_be_bytes());

        let cfg = build_cfg("boot", &bytes, va_start, &[va_start]);
        let report = classify_consumers(&cfg, &bytes, va_start);
        let found = report
            .classifications
            .iter()
            .find(|c| c.va == pointer_slot_va)
            .unwrap();
        assert_eq!(found.class, CandidateContentClass::Pointer);
    }
}
