//! Delta-voting mapping inference for a code region with an unknown VA base.
//!
//! This mechanizes the NW4E selector VA correction (see
//! [`crate::aki_reference`], "VA correction (2026-07-18)"): there, twelve
//! `jal`-to-prologue coincidences uniquely selected the resident delta
//! `0x7fff_f400` over the naive `0x8000_0000` guess. The same evidence class
//! -- absolute call targets landing on classic function prologues -- is
//! delta-DISCRIMINATING, because a `jal`'s 26-bit target is an absolute VA
//! while a prologue's position is a fixed ROM offset; only the true mapping
//! aligns many of the former onto many of the latter.
//!
//! # Discipline
//!
//! ENUMERATE hypotheses, VALIDATE by constraint, ADMIT only on uniqueness.
//! Every function here is a pure function of the region bytes and the
//! configuration: no I/O, no randomness, byte-identical results across runs.
//! A near-tie between two surviving deltas stays [`DeltaVoteOutcome::Open`];
//! nothing is promoted by score alone. The admitted delta is a *candidate
//! mapping* for downstream proof phases, never itself a proven `RomMapping`.
//!
//! # Evidence classes per delta hypothesis `d` (`va_start = rom_start + d`)
//!
//! - **(a) call->prologue votes** (discriminating): distinct absolute `jal`
//!   targets whose implied region offset `T - va_start` is the site of a
//!   classic `addiu $sp,$sp,-N` prologue (or a caller-supplied known entry
//!   offset). Counted over *distinct* targets: a popular callee called 50
//!   times must not contribute 50 coincident votes to a wrong delta through
//!   a single lucky prologue pairing.
//! - **(b) %hi/%lo in-region votes** (corroborating): distinct
//!   `lui`+`addiu`/`ori`/load/store computed absolute addresses that fall
//!   inside `[va_start, va_start + region_len)`. These form plateaus -- a
//!   delta shifted by one word keeps nearly all of them -- so they are
//!   reported for corroboration and never used to break an (a)-vote tie.
//! - **(c) internal branch targets** (delta-INVARIANT): PC-relative, so they
//!   are identical under every delta. They are reported as a
//!   region-is-plausibly-code sanity statistic and deliberately excluded
//!   from scoring.
//!
//! # Hypothesis enumeration
//!
//! The sound narrowing is the region's own `lui` upper-half histogram: each
//! `lui $r, H` says the region addresses VA space near `H << 16`, so
//! `va_start` must lie in `[H<<16 - 0x8000 - region_len, H<<16 + 0x8000)`
//! for the region to contain such an address. Candidate deltas are the
//! distinct values `T - p - rom_start` (jal target `T` x prologue offset
//! `p`) whose implied `va_start` is alignment-quantized and falls inside one
//! of those windows. [`DeltaVoteConfig::full_sweep`] disables the window
//! filter: that is the exhaustive aligned sweep restricted to deltas with at
//! least one (a)-vote, which loses nothing admissible (a zero-(a)-vote delta
//! can never reach [`DeltaVoteConfig::min_votes`]); its cost is reported as
//! `pairs_considered`/`candidate_count`.
//!
//! # Admission rule and margin justification
//!
//! The unique top delta by (a)-votes is admitted only if:
//!
//! - `top_a >= min_votes` (default 3): three *independent* distinct-callee
//!   coincidences. One coincidence arises by chance easily (any resident
//!   `jal` target minus any prologue offset manufactures a delta); the NW4E
//!   selector precedent had twelve. Two is the smallest coincidence a
//!   single arithmetic accident (one shared spacing) can also produce.
//! - `top_a >= domination_factor * runner_a` (default 2): the winner must
//!   explain at least twice the aligned-call evidence of every alternative.
//!   Two hypotheses each explaining comparable shares of the call set is
//!   exactly the ambiguous case that must stay OPEN -- uniformly spaced
//!   prologues (an arithmetic progression) alias a delta by their period and
//!   produce near-equal counts, which this factor refuses. An exact tie
//!   always fails the factor for any `runner_a > 0`.
//!
//! A rule that admits wrong deltas on the graded NW4E overlays gets rejected
//! with its numbers (the aligned-pointer-run precedent), not tuned quietly.

use crate::cfg::{classify_control, ControlOp};
use fn64_recomp_rs::{decode, Instruction};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Tuning for [`infer_region_delta`]. Every field is part of the reported
/// result's meaning; gates must print the configuration they graded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaVoteConfig {
    /// Quantum the implied `va_start` must be aligned to. 4 (instruction
    /// alignment) is the safest sound choice; larger values encode a layout
    /// assumption and must be justified by the caller.
    pub alignment: u32,
    /// Minimum (a)-votes for admission (see module doc for justification).
    pub min_votes: u32,
    /// `top_a >= domination_factor * runner_a` required for admission.
    pub domination_factor: u32,
    /// Disable the lui-window narrowing and enumerate every pair-supported
    /// delta (bounded exhaustive sweep; cost reported, results unchanged
    /// for admissible deltas -- see module doc).
    pub full_sweep: bool,
    /// A `lui` upper half must occur at least this many times to open a
    /// candidate window.
    pub lui_min_count: u32,
    /// At most this many upper halves (by count desc, value asc) open
    /// windows, bounding the candidate set.
    pub lui_max_uppers: usize,
}

impl Default for DeltaVoteConfig {
    fn default() -> Self {
        Self {
            alignment: 4,
            min_votes: 3,
            domination_factor: 2,
            full_sweep: false,
            lui_min_count: 4,
            lui_max_uppers: 16,
        }
    }
}

/// One scored delta hypothesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaScore {
    /// `va_start - rom_start` under this hypothesis.
    pub delta: u32,
    pub va_start: u32,
    /// (a): distinct `jal` targets landing on a prologue/known entry.
    pub call_prologue_votes: u32,
    /// (b): distinct %hi/%lo computed addresses inside the mapped region.
    pub hilo_in_region_votes: u32,
}

/// Why a region stayed OPEN. Every variant carries the numbers that would
/// have had to differ for admission, so "open" is a measurement, not a
/// shrug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenReason {
    /// No `lui` upper half in the region: the 26-bit `jal` segment nibble
    /// cannot be fixed mapping-independently, so no absolute target can be
    /// reconstructed.
    NoLuiSegmentEvidence,
    /// No (jal target, prologue offset) pair survived enumeration -- e.g. a
    /// branches-only region, whose only structure is delta-invariant.
    NoDeltaCandidates,
    /// The top delta's (a)-votes fall below the admission minimum.
    InsufficientVotes { top_votes: u32, required: u32 },
    /// The runner-up explains too comparable a share of the call evidence.
    NearTie {
        top_votes: u32,
        runner_up_votes: u32,
        required_factor: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeltaVoteOutcome {
    /// The unique dominating delta. Still a *candidate mapping*: downstream
    /// phases own promotion to a proven `RomMapping`.
    Admitted {
        delta: u32,
        va_start: u32,
    },
    Open {
        reason: OpenReason,
    },
}

/// Mapping-independent scan statistics, reported so a grader can see what
/// evidence the region offered (and so "open" outcomes are auditable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionScanStats {
    pub words: usize,
    /// `jal` sites with a decodable delay slot.
    pub jal_sites: usize,
    pub distinct_jal_targets: usize,
    /// Classic `addiu $sp,$sp,-N` sites (frame nonzero, multiple of 8).
    pub prologue_sites: usize,
    /// %hi/%lo materializations and effective addresses observed.
    pub hilo_sites: usize,
    pub distinct_hilo_addresses: usize,
    /// (c) sanity statistic: PC-relative branches, delta-invariant.
    pub branch_sites: usize,
    pub branch_targets_in_region: usize,
    pub lui_sites: usize,
    /// Upper halves that opened candidate windows (0 when `full_sweep`).
    pub retained_lui_uppers: usize,
}

/// The full result for one region: evidence, cost, top two hypotheses, and
/// the typed outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaVoteResult {
    pub rom_start: u32,
    pub rom_end: u32,
    /// The 256MB `j`/`jal` segment fixed from the dominant `lui` upper half
    /// (mapping-independent). `None` when the region has no `lui` at all.
    pub segment: Option<u32>,
    pub config: DeltaVoteConfig,
    pub scan: RegionScanStats,
    /// (jal target x prologue offset) pairs enumerated -- the sweep cost.
    pub pairs_considered: u64,
    /// Distinct deltas that received at least one (a)-vote and passed the
    /// window/alignment filters.
    pub candidate_count: usize,
    pub top: Option<DeltaScore>,
    pub runner_up: Option<DeltaScore>,
    pub outcome: DeltaVoteOutcome,
}

/// Classic prologue: `addiu $sp,$sp,-N` with `N != 0`, `N % 8 == 0` (the
/// same shape `harvest::stack_allocation` accepts; duplicated here because
/// that helper is private to its provider and this module must stay
/// mapping-independent and self-contained).
fn is_classic_prologue(word: u32) -> bool {
    let opcode = word >> 26;
    let rs = (word >> 21) & 0x1f;
    let rt = (word >> 16) & 0x1f;
    let immediate = (word & 0xffff) as i16;
    if !matches!(opcode, 0x09 | 0x19) || rs != 29 || rt != 29 || immediate >= 0 {
        return false;
    }
    let frame = (-(immediate as i32)) as u32;
    frame != 0 && frame.is_multiple_of(8)
}

/// Linear per-register %hi/%lo tracking state (same conservatism rules as
/// `xref::scan_global_refs`, but collecting *all* lui-rooted computed
/// addresses instead of matching one target). Only `lui`-rooted chains are
/// tracked: a small `li` constant is not an address hypothesis.
#[derive(Clone, Copy)]
struct HiLoState {
    value: u32,
}

struct RegionScan {
    /// Distinct raw 26-bit `jal` target fields (delay-slot-valid sites).
    jal_target26: BTreeSet<u32>,
    jal_sites: usize,
    /// Region-relative byte offsets of classic prologues, ascending.
    prologue_offsets: Vec<u32>,
    /// Distinct lui-rooted computed absolute addresses, ascending.
    hilo_addresses: Vec<u32>,
    hilo_sites: usize,
    branch_sites: usize,
    branch_targets_in_region: usize,
    lui_sites: usize,
    /// upper half -> occurrence count.
    lui_uppers: BTreeMap<u16, u32>,
}

/// One linear decode pass. Mapping-independent by construction: everything
/// collected is either an absolute instruction field, a region-relative
/// offset, or a delta-invariant statistic.
fn scan_region(region_bytes: &[u8]) -> RegionScan {
    let len = region_bytes.len() as u64;
    let mut scan = RegionScan {
        jal_target26: BTreeSet::new(),
        jal_sites: 0,
        prologue_offsets: Vec::new(),
        hilo_addresses: Vec::new(),
        hilo_sites: 0,
        branch_sites: 0,
        branch_targets_in_region: 0,
        lui_sites: 0,
        lui_uppers: BTreeMap::new(),
    };
    let words: Vec<u32> = region_bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes(chunk.try_into().expect("four-byte chunk")))
        .collect();

    let mut hilo_set: BTreeSet<u32> = BTreeSet::new();
    let mut regs: [Option<HiLoState>; 32] = [None; 32];
    let mut clear_after_this_word = false;

    for (index, &word) in words.iter().enumerate() {
        let off = (index as u32) * 4;
        let this_is_delay_slot = clear_after_this_word;
        clear_after_this_word = false;
        let control = classify_control(word);

        match control {
            ControlOp::Jal { target } => {
                // A `jal` only votes if its architecturally required delay
                // slot decodes: a data word that happens to look like `jal`
                // rarely precedes decodable code.
                let delay_ok = words.get(index + 1).is_some_and(|&next| {
                    !matches!(classify_control(next), ControlOp::Invalid { .. })
                });
                if delay_ok {
                    scan.jal_sites += 1;
                    scan.jal_target26.insert(target);
                }
                clear_after_this_word = true;
                // `jal` writes $ra.
                regs[31] = None;
            }
            ControlOp::J { .. } | ControlOp::Jr { .. } => {
                clear_after_this_word = true;
            }
            ControlOp::Jalr { rd, .. } => {
                clear_after_this_word = true;
                if rd != 0 {
                    regs[rd as usize] = None;
                }
            }
            ControlOp::Branch { target, link } | ControlOp::BranchLikely { target, link } => {
                scan.branch_sites += 1;
                let target_off = off.wrapping_add(4).wrapping_add(target.wrapping_shl(2));
                if (target_off as u64) < len {
                    scan.branch_targets_in_region += 1;
                }
                clear_after_this_word = true;
                if link {
                    regs[31] = None;
                }
            }
            ControlOp::Trap | ControlOp::Invalid { .. } => {
                regs = [None; 32];
            }
            ControlOp::Plain => {
                if is_classic_prologue(word) {
                    scan.prologue_offsets.push(off);
                }
                match decode(word) {
                    Instruction::Lui { rt, imm } => {
                        scan.lui_sites += 1;
                        *scan.lui_uppers.entry(imm).or_insert(0) += 1;
                        if rt != 0 {
                            regs[rt as usize] = Some(HiLoState {
                                value: (imm as u32) << 16,
                            });
                        }
                    }
                    Instruction::Addiu { rt, rs, imm } => {
                        let derived = if rs != 0 {
                            regs[rs as usize].map(|state| HiLoState {
                                value: state.value.wrapping_add(imm as i32 as u32),
                            })
                        } else {
                            None
                        };
                        if let Some(state) = derived {
                            scan.hilo_sites += 1;
                            hilo_set.insert(state.value);
                        }
                        if rt != 0 {
                            regs[rt as usize] = derived;
                        }
                    }
                    Instruction::Ori { rt, rs, imm } => {
                        let derived = if rs != 0 {
                            regs[rs as usize].map(|state| HiLoState {
                                value: state.value | imm as u32,
                            })
                        } else {
                            None
                        };
                        if let Some(state) = derived {
                            scan.hilo_sites += 1;
                            hilo_set.insert(state.value);
                        }
                        if rt != 0 {
                            regs[rt as usize] = derived;
                        }
                    }
                    Instruction::Lb { rt, base, off: imm }
                    | Instruction::Lbu { rt, base, off: imm }
                    | Instruction::Lh { rt, base, off: imm }
                    | Instruction::Lhu { rt, base, off: imm }
                    | Instruction::Lw { rt, base, off: imm }
                    | Instruction::Lwu { rt, base, off: imm } => {
                        if let Some(state) = regs[base as usize] {
                            scan.hilo_sites += 1;
                            hilo_set.insert(state.value.wrapping_add(imm as i32 as u32));
                        }
                        if rt != 0 {
                            regs[rt as usize] = None;
                        }
                    }
                    Instruction::Sb { base, off: imm, .. }
                    | Instruction::Sh { base, off: imm, .. }
                    | Instruction::Sw { base, off: imm, .. } => {
                        if let Some(state) = regs[base as usize] {
                            scan.hilo_sites += 1;
                            hilo_set.insert(state.value.wrapping_add(imm as i32 as u32));
                        }
                    }
                    _ => {
                        // Conservative wipe: any other instruction may write
                        // any GPR. Over-clearing loses only corroborating
                        // (b)-votes -- uniformly across all delta hypotheses
                        // -- and can never invent an address.
                        regs = [None; 32];
                    }
                }
            }
        }

        if this_is_delay_slot {
            regs = [None; 32];
        }
    }
    scan.hilo_addresses = hilo_set.into_iter().collect();
    scan
}

/// Candidate `va_start` windows implied by the retained `lui` upper halves
/// (see module doc), as sorted inclusive-exclusive `u64` intervals.
fn lui_windows(
    scan: &RegionScan,
    region_len: u64,
    config: &DeltaVoteConfig,
) -> (Vec<(u64, u64)>, usize) {
    let mut uppers: Vec<(u16, u32)> = scan
        .lui_uppers
        .iter()
        .map(|(&upper, &count)| (upper, count))
        .filter(|&(_, count)| count >= config.lui_min_count)
        .collect();
    uppers.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    uppers.truncate(config.lui_max_uppers);
    let retained = uppers.len();
    let mut windows: Vec<(u64, u64)> = uppers
        .iter()
        .map(|&(upper, _)| {
            let base = (upper as u64) << 16;
            let lo = base.saturating_sub(0x8000 + region_len);
            let hi = base + 0x8000;
            (lo, hi)
        })
        .collect();
    windows.sort_unstable();
    (windows, retained)
}

fn in_windows(windows: &[(u64, u64)], value: u64) -> bool {
    windows.iter().any(|&(lo, hi)| value >= lo && value < hi)
}

/// Infer the region's mapping delta. `known_entry_offsets` are optional
/// region-relative byte offsets of already-proven function entries (from a
/// prior analysis of the *same* region); they join the prologue offsets as
/// (a)-vote landing sites. Pass `&[]` when nothing is known.
///
/// Pure function: byte-identical output for byte-identical input.
pub fn infer_region_delta(
    region_bytes: &[u8],
    rom_start: u32,
    known_entry_offsets: &[u32],
    config: &DeltaVoteConfig,
) -> DeltaVoteResult {
    assert!(
        config.alignment.is_power_of_two(),
        "alignment must be a power of two"
    );
    assert!(
        config.domination_factor >= 1,
        "domination factor must be at least 1"
    );
    let region_len = region_bytes.len() as u64;
    let scan = scan_region(region_bytes);
    let (windows, retained_lui_uppers) = lui_windows(&scan, region_len, config);

    let stats = RegionScanStats {
        words: region_bytes.len() / 4,
        jal_sites: scan.jal_sites,
        distinct_jal_targets: scan.jal_target26.len(),
        prologue_sites: scan.prologue_offsets.len(),
        hilo_sites: scan.hilo_sites,
        distinct_hilo_addresses: scan.hilo_addresses.len(),
        branch_sites: scan.branch_sites,
        branch_targets_in_region: scan.branch_targets_in_region,
        lui_sites: scan.lui_sites,
        retained_lui_uppers: if config.full_sweep {
            0
        } else {
            retained_lui_uppers
        },
    };

    let mut result = DeltaVoteResult {
        rom_start,
        rom_end: rom_start.wrapping_add(region_bytes.len() as u32),
        segment: None,
        config: *config,
        scan: stats,
        pairs_considered: 0,
        candidate_count: 0,
        top: None,
        runner_up: None,
        outcome: DeltaVoteOutcome::Open {
            reason: OpenReason::NoLuiSegmentEvidence,
        },
    };

    // The 26-bit `jal` field only fixes the low 28 bits; the segment nibble
    // comes from the dominant lui upper half -- mapping-independent, since
    // the region's own address constants name the segment it lives in and
    // references. No lui at all means no reconstructible absolute target.
    let Some(segment) = scan
        .lui_uppers
        .iter()
        .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
        .map(|(&upper, _)| ((upper as u32) << 16) & 0xf000_0000)
    else {
        return result;
    };
    result.segment = Some(segment);

    // (a)-vote landing sites: classic prologues plus caller-known entries.
    let landing_offsets: BTreeSet<u32> = scan
        .prologue_offsets
        .iter()
        .copied()
        .chain(known_entry_offsets.iter().copied())
        .filter(|&offset| (offset as u64) < region_len)
        .collect();

    // Enumerate pair-supported deltas: histogram of (T - p), filtered by
    // alignment and (unless full_sweep) the lui windows.
    let alignment_mask = config.alignment - 1;
    let mut votes: BTreeMap<u32, u32> = BTreeMap::new();
    let mut pairs: u64 = 0;
    for &target26 in &scan.jal_target26 {
        let target = segment | (target26 << 2);
        for &offset in &landing_offsets {
            pairs += 1;
            let va_start = target.wrapping_sub(offset);
            if va_start & alignment_mask != 0 {
                continue;
            }
            if !config.full_sweep && !in_windows(&windows, va_start as u64) {
                continue;
            }
            *votes.entry(va_start.wrapping_sub(rom_start)).or_insert(0) += 1;
        }
    }
    result.pairs_considered = pairs;
    result.candidate_count = votes.len();

    if votes.is_empty() {
        result.outcome = DeltaVoteOutcome::Open {
            reason: OpenReason::NoDeltaCandidates,
        };
        return result;
    }

    // Rank by (a)-votes desc, delta asc. (b)-votes are corroboration and
    // deliberately never reorder candidates (see module doc: plateaus).
    let mut top: Option<(u32, u32)> = None;
    let mut runner: Option<(u32, u32)> = None;
    for (&delta, &count) in &votes {
        let beats = |incumbent: Option<(u32, u32)>| match incumbent {
            None => true,
            Some((_, incumbent_count)) => count > incumbent_count,
        };
        if beats(top) {
            runner = top;
            top = Some((delta, count));
        } else if beats(runner) {
            runner = Some((delta, count));
        }
    }

    let score = |(delta, call_votes): (u32, u32)| {
        let va_start = rom_start.wrapping_add(delta);
        let lo = scan.hilo_addresses.partition_point(|&addr| addr < va_start);
        let hi = scan
            .hilo_addresses
            .partition_point(|&addr| (addr as u64) < va_start as u64 + region_len);
        DeltaScore {
            delta,
            va_start,
            call_prologue_votes: call_votes,
            hilo_in_region_votes: (hi - lo) as u32,
        }
    };
    let top_score = score(top.expect("nonempty vote histogram has a top"));
    let runner_score = runner.map(score);
    result.top = Some(top_score);
    result.runner_up = runner_score;

    result.outcome = if top_score.call_prologue_votes < config.min_votes {
        DeltaVoteOutcome::Open {
            reason: OpenReason::InsufficientVotes {
                top_votes: top_score.call_prologue_votes,
                required: config.min_votes,
            },
        }
    } else if let Some(runner_score) = runner_score.filter(|runner_score| {
        top_score.call_prologue_votes < config.domination_factor * runner_score.call_prologue_votes
    }) {
        DeltaVoteOutcome::Open {
            reason: OpenReason::NearTie {
                top_votes: top_score.call_prologue_votes,
                runner_up_votes: runner_score.call_prologue_votes,
                required_factor: config.domination_factor,
            },
        }
    } else {
        DeltaVoteOutcome::Admitted {
            delta: top_score.delta,
            va_start: top_score.va_start,
        }
    };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOP: u32 = 0x0000_0000;
    const PROLOGUE: u32 = 0x27bd_ffe0; // addiu $sp,$sp,-0x20
    const LUI_8010: u32 = 0x3c04_8010; // lui $a0, 0x8010

    fn jal(target_va: u32) -> u32 {
        0x0c00_0000 | ((target_va >> 2) & 0x03ff_ffff)
    }

    fn assemble(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    /// Region at ROM 0x1000 whose true mapping is VA 0x8010_0000
    /// (delta 0x800f_f000), with prologues at NON-uniform offsets so cross
    /// pairs cannot alias a second delta.
    fn admissible_region() -> Vec<u8> {
        let va = 0x8010_0000u32;
        let mut words = vec![NOP; 0x140 / 4];
        words[0] = jal(va + 0x40);
        words[2] = jal(va + 0x90);
        words[4] = jal(va + 0x100);
        for slot in words.iter_mut().skip(6).take(4) {
            *slot = LUI_8010;
        }
        words[0x40 / 4] = PROLOGUE;
        words[0x90 / 4] = PROLOGUE;
        words[0x100 / 4] = PROLOGUE;
        assemble(&words)
    }

    #[test]
    fn synthetic_region_with_three_votes_admits_true_delta() {
        let bytes = admissible_region();
        let result = infer_region_delta(&bytes, 0x1000, &[], &DeltaVoteConfig::default());
        assert_eq!(result.segment, Some(0x8000_0000));
        assert_eq!(
            result.outcome,
            DeltaVoteOutcome::Admitted {
                delta: 0x800f_f000,
                va_start: 0x8010_0000
            }
        );
        let top = result.top.unwrap();
        assert_eq!(top.call_prologue_votes, 3);
        assert!(result.runner_up.unwrap().call_prologue_votes <= 1);
    }

    #[test]
    fn two_equally_voted_deltas_stay_open() {
        // Prologues repeated at a fixed 0x100 shift: every jal->prologue
        // pairing that supports va also supports va - 0x100 with the same
        // multiplicity. Score cannot pick one; the region must stay OPEN.
        let va = 0x8010_0000u32;
        let mut words = vec![NOP; 0x240 / 4];
        words[0] = jal(va + 0x40);
        words[2] = jal(va + 0x80);
        words[4] = jal(va + 0xc0);
        for slot in words.iter_mut().skip(6).take(4) {
            *slot = LUI_8010;
        }
        for offset in [0x40u32, 0x80, 0xc0, 0x140, 0x180, 0x1c0] {
            words[(offset / 4) as usize] = PROLOGUE;
        }
        let bytes = assemble(&words);
        let result = infer_region_delta(&bytes, 0x1000, &[], &DeltaVoteConfig::default());
        let top = result.top.unwrap();
        let runner = result.runner_up.unwrap();
        assert_eq!(top.call_prologue_votes, 3);
        assert_eq!(runner.call_prologue_votes, 3);
        assert_eq!(
            result.outcome,
            DeltaVoteOutcome::Open {
                reason: OpenReason::NearTie {
                    top_votes: 3,
                    runner_up_votes: 3,
                    required_factor: 2
                }
            }
        );
    }

    #[test]
    fn branches_only_region_stays_open_with_no_candidates() {
        // Only PC-relative structure (delta-invariant) plus enough lui to
        // fix a segment: no jal, no prologue -> nothing discriminates.
        let beq_fwd = 0x1000_0002u32;
        let mut words = vec![NOP; 16];
        words[0] = beq_fwd;
        for slot in words.iter_mut().skip(2).take(4) {
            *slot = LUI_8010;
        }
        let bytes = assemble(&words);
        let result = infer_region_delta(&bytes, 0x2000, &[], &DeltaVoteConfig::default());
        assert_eq!(
            result.outcome,
            DeltaVoteOutcome::Open {
                reason: OpenReason::NoDeltaCandidates
            }
        );
        assert_eq!(result.scan.branch_sites, 1);
        assert_eq!(result.scan.branch_targets_in_region, 1);
    }

    #[test]
    fn region_without_lui_stays_open_for_missing_segment() {
        let va = 0x8010_0000u32;
        let mut words = vec![NOP; 0x60 / 4];
        words[0] = jal(va + 0x40);
        words[0x40 / 4] = PROLOGUE;
        let bytes = assemble(&words);
        let result = infer_region_delta(&bytes, 0x1000, &[], &DeltaVoteConfig::default());
        assert_eq!(result.segment, None);
        assert_eq!(
            result.outcome,
            DeltaVoteOutcome::Open {
                reason: OpenReason::NoLuiSegmentEvidence
            }
        );
    }

    #[test]
    fn full_sweep_agrees_with_windowed_enumeration_on_admissible_region() {
        let bytes = admissible_region();
        let windowed = infer_region_delta(&bytes, 0x1000, &[], &DeltaVoteConfig::default());
        let swept = infer_region_delta(
            &bytes,
            0x1000,
            &[],
            &DeltaVoteConfig {
                full_sweep: true,
                ..DeltaVoteConfig::default()
            },
        );
        assert_eq!(windowed.outcome, swept.outcome);
        assert!(swept.candidate_count >= windowed.candidate_count);
    }

    #[test]
    fn known_entry_offsets_vote_like_prologues() {
        // One jal lands on a known entry that has no classic prologue; the
        // other two land on prologues. Together they reach min_votes.
        let va = 0x8010_0000u32;
        let mut words = vec![NOP; 0x140 / 4];
        words[0] = jal(va + 0x40);
        words[2] = jal(va + 0x90);
        words[4] = jal(va + 0x104);
        for slot in words.iter_mut().skip(6).take(4) {
            *slot = LUI_8010;
        }
        words[0x40 / 4] = PROLOGUE;
        words[0x90 / 4] = PROLOGUE;
        let bytes = assemble(&words);
        let without = infer_region_delta(&bytes, 0x1000, &[], &DeltaVoteConfig::default());
        assert!(matches!(
            without.outcome,
            DeltaVoteOutcome::Open {
                reason: OpenReason::InsufficientVotes { .. }
            }
        ));
        let with = infer_region_delta(&bytes, 0x1000, &[0x104], &DeltaVoteConfig::default());
        assert_eq!(
            with.outcome,
            DeltaVoteOutcome::Admitted {
                delta: 0x800f_f000,
                va_start: 0x8010_0000
            }
        );
    }

    #[test]
    fn repeated_targets_vote_once() {
        // The same callee jal'd three times contributes one distinct
        // target: multiplicity must not manufacture domination.
        let va = 0x8010_0000u32;
        let mut words = vec![NOP; 0x80 / 4];
        words[0] = jal(va + 0x40);
        words[2] = jal(va + 0x40);
        words[4] = jal(va + 0x40);
        for slot in words.iter_mut().skip(6).take(4) {
            *slot = LUI_8010;
        }
        words[0x40 / 4] = PROLOGUE;
        let bytes = assemble(&words);
        let result = infer_region_delta(&bytes, 0x1000, &[], &DeltaVoteConfig::default());
        assert_eq!(result.scan.jal_sites, 3);
        assert_eq!(result.scan.distinct_jal_targets, 1);
        assert_eq!(result.top.unwrap().call_prologue_votes, 1);
        assert!(matches!(
            result.outcome,
            DeltaVoteOutcome::Open {
                reason: OpenReason::InsufficientVotes { .. }
            }
        ));
    }

    #[test]
    fn inference_is_byte_identical_across_runs() {
        let bytes = admissible_region();
        let first = infer_region_delta(&bytes, 0x1000, &[], &DeltaVoteConfig::default());
        let second = infer_region_delta(&bytes, 0x1000, &[], &DeltaVoteConfig::default());
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
    }
}
