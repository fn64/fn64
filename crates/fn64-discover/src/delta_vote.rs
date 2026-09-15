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
use fn64_cpu_runtime::{decode, Instruction};
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
    /// `jr $ra` sites. Every function returns; data does not. Measured across
    /// the corpus, this separates code from asset bytes more sharply than any
    /// other single signal -- OoT's known boot code shows 33-65 returns per
    /// 8 KiB, while eight spans a prologue-presence test wrongly called code
    /// show ZERO.
    pub return_sites: usize,
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
    /// `jr $ra` sites -- function returns. Delta-invariant, and the signal that
    /// separates code from data most sharply.
    return_sites: usize,
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
        return_sites: 0,
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
        // `jr $ra` -- a function return. Counted before the control match so it
        // is tallied regardless of how that match classifies an indirect jump.
        if word == 0x03e0_0008 {
            scan.return_sites += 1;
        }
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
        return_sites: scan.return_sites,
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

/// Prove a candidate ROM extent is a consistent MIPS image, and recover the
/// address it loads at.
///
/// This is one question that answers two: a region admits a dominating delta
/// only if its own calls land on its own function entries under that hypothesis,
/// which data essentially never does. So the same call is both a VA oracle and a
/// code filter, and the heuristic "does this look like code" test is not needed
/// as a gate.
///
/// Measured on the eight OoT spans a prologue-presence heuristic wrongly called
/// code: **0 of 8 admit** (all `InsufficientVotes`, top_votes=1), while OoT's
/// known boot file admits at `va=0x80000460` -- its true load address.
///
/// Two escalations, in cost order, because the default configuration leaves real
/// images on the table:
///
/// 1. **Default.** The `lui`-window narrowing keeps the candidate set small.
/// 2. **`full_sweep`.** The narrowing can EXCLUDE the correct delta. Measured on
///    WCW World Tour's 216 KiB image at ROM 0xa21000: default returns
///    `NearTie{12,10}`, `full_sweep` admits `va=0x8008f6c0`. Subdividing that
///    same region does not help (every half and quarter near-ties), so the fix is
///    a wider candidate set, not a smaller window.
///
/// Extent choice matters more than either: voting over a natural extent rather
/// than a fixed window is what makes this work at all. WCW's 352 KiB image admits
/// at 328 votes against 25 over its whole extent, where individual 32 KiB windows
/// scored 2-3 votes and returned `Open`.
/// The largest RDRAM a retail N64 reaches, with the Expansion Pak. A recovered
/// image has to LIVE somewhere; an address beyond this cannot be where any code
/// loads, whatever the vote said.
const RDRAM_LEN: u32 = 0x0080_0000;

/// Is this the KSEG0 or KSEG1 view of real RDRAM?
///
/// A vote fixes its 256 MiB segment from the region's dominant `lui` upper half,
/// so a region whose `lui`s are noise yields a confidently-scored delta pointing
/// somewhere impossible. Measured across the corpus before this check: 6 of 21
/// admitted regions named an address outside RDRAM entirely -- 0x8f0cc4d0,
/// 0x701f97b0, 0x7fffa5e4. A dominating vote is not the same as a possible
/// answer.
fn addressable(va_start: u32, va_end: u32) -> bool {
    if va_end <= va_start {
        return false;
    }
    for base in [0x8000_0000u32, 0xa000_0000] {
        if va_start >= base && va_end <= base.saturating_add(RDRAM_LEN) {
            return true;
        }
    }
    false
}

pub fn prove_region(
    rom_bytes: &[u8],
    rom_start: u32,
    rom_end: u32,
    config: &DeltaVoteConfig,
) -> Option<UntabledRegion> {
    let (start, end) = (
        rom_start as usize,
        rom_end.min(rom_bytes.len() as u32) as usize,
    );
    if end.saturating_sub(start) < 0x400 {
        return None;
    }
    let bytes = &rom_bytes[start..end];

    let mut escalated = *config;
    escalated.full_sweep = true;
    for (attempt, cfg) in [config, &escalated].into_iter().enumerate() {
        let result = infer_region_delta(bytes, rom_start, &[], cfg);
        if let DeltaVoteOutcome::Admitted { delta, va_start } = result.outcome {
            let va_end = va_start.wrapping_add(rom_end - rom_start);
            if !addressable(va_start, va_end) {
                // Do not fall through to the escalation: a wider candidate set
                // cannot make an unaddressable region addressable, and the
                // exhaustive sweep is the expensive path.
                return None;
            }
            return Some(UntabledRegion {
                rom_start,
                rom_end: end as u32,
                va_start,
                delta,
                // A whole-extent proof is one agreement over the entire region,
                // not N independent window votes; `windows` stays 1 so it is
                // never confused with the corroboration count the windowed
                // sweep reports.
                windows: 1,
                full_sweep_required: attempt == 1,
            });
        }
    }
    None
}

/// One ROM region whose load address was inferred with NO table of any kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UntabledRegion {
    pub rom_start: u32,
    pub rom_end: u32,
    pub va_start: u32,
    /// `va_start - rom_start`, constant across every window that merged here.
    pub delta: u32,
    /// Windows that independently agreed on this delta. A region that survived
    /// several adjacent independent votes is far stronger evidence than one
    /// window that happened to dominate.
    pub windows: u32,
    /// The default candidate-set narrowing excluded the winning delta and the
    /// exhaustive sweep was needed. Recorded because it is a real signal about
    /// the region, not just about cost.
    #[serde(default)]
    pub full_sweep_required: bool,
}

/// The ROM header plus the IPL3 blob, whose extent is fixed by hardware. Never
/// copied to RDRAM as part of a load image.
const HEADER_AND_IPL3_END: u32 = 0x1000;

/// Tuning for [`sweep_untabled_regions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UntabledSweepConfig {
    /// Bytes per voting window. Large enough that a window holds enough
    /// intra-region calls to vote, small enough that two differently-loaded
    /// images rarely share one.
    pub window_len: u32,
    /// Advance between windows. Equal to `window_len` means no overlap.
    pub stride: u32,
    pub vote: DeltaVoteConfig,
}

impl Default for UntabledSweepConfig {
    fn default() -> Self {
        Self {
            window_len: 0x8000,
            stride: 0x8000,
            vote: DeltaVoteConfig::default(),
        }
    }
}

/// Infer load addresses for a whole ROM with no file table, no descriptor
/// table, and no emulator.
///
/// The mechanism is a property of the instruction encoding rather than of any
/// engine convention: a `jal` carries an ABSOLUTE target, so a blob of code
/// states, in its own bytes, the address it expects to run at. Score candidate
/// load addresses by how much internal evidence lands consistently, and the
/// winner is where that blob loads.
///
/// This is why it reaches ROMs that every table-shape search returns zero for.
/// [`infer_region_delta`] already did all of the hard part and documents itself
/// as mapping-independent; it had only ever been pointed at regions a table
/// search had already located, which is precisely the case where a load address
/// is least needed.
///
/// Windows that agree on a delta and are adjacent in ROM are merged, and the
/// count of independently agreeing windows is retained: one dominating window
/// is a hypothesis, six adjacent ones voting identically is a region.
///
/// Every region is a CANDIDATE. Instruction bytes do not prove a mapping is
/// reachable, resident, or ever loaded -- promotion stays with the caller.
pub fn sweep_untabled_regions(
    rom_bytes: &[u8],
    config: &UntabledSweepConfig,
) -> Vec<UntabledRegion> {
    assert!(
        config.window_len >= 0x100,
        "a window must hold enough calls to vote"
    );
    assert!(config.stride > 0, "stride must advance");

    let mut merged: Vec<UntabledRegion> = Vec::new();
    // Start past the header and IPL3. Those bytes are never loaded to RDRAM, so
    // a window covering them reports where ROM offset 0 *would* load -- an
    // address the game never uses. Measured: three corpus regions began at ROM 0
    // and reported va 0x7ffff400 for exactly this reason.
    let mut offset: u32 = HEADER_AND_IPL3_END;
    while (offset as usize) + (config.window_len as usize) <= rom_bytes.len() {
        let start = offset as usize;
        let end = start + config.window_len as usize;
        let result = infer_region_delta(&rom_bytes[start..end], offset, &[], &config.vote);
        offset = offset.saturating_add(config.stride);

        let DeltaVoteOutcome::Admitted { delta, va_start } = result.outcome else {
            continue;
        };
        if !addressable(
            va_start,
            va_start.wrapping_add(result.rom_end - result.rom_start),
        ) {
            continue;
        }

        // Merge with the previous region when it is the same image: same delta,
        // and contiguous in ROM. Same delta alone is not enough -- two copies of
        // one library at the same relative offset in different files would join
        // regions that were never one image.
        match merged.last_mut() {
            Some(previous) if previous.delta == delta && previous.rom_end == result.rom_start => {
                previous.rom_end = result.rom_end;
                previous.windows += 1;
            }
            _ => merged.push(UntabledRegion {
                rom_start: result.rom_start,
                rom_end: result.rom_end,
                va_start,
                delta,
                windows: 1,
                full_sweep_required: false,
            }),
        }
    }
    merged
}

// ---------------------------------------------------------------------------
// Relocated boot-image slice vote (B7 / K18)
// ---------------------------------------------------------------------------

/// KSEG0's base. A relocated slice must live in cached RDRAM: the corpus
/// frontier class of call targets at or above `KSEG0_BASE + RDRAM_LEN` is
/// value-set imprecision, not code nobody mapped.
const KSEG0_BASE: u32 = 0x8000_0000;

/// Tuning for [`relocated_slice_vote`].
///
/// Deliberately the SAME bar as [`DeltaVoteConfig`]'s: three distinct-target
/// coincidences and a 2x margin over the runner-up. This step widens the
/// evidence SOURCE (calls from a different, already-proven region rather than
/// from inside the candidate), never the admission rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelocatedSliceConfig {
    /// Minimum distinct call targets that must land on a prologue under the
    /// winning delta.
    pub min_votes: u32,
    /// `top >= domination_factor * runner_up` required for admission. An exact
    /// tie always fails for any nonzero runner-up.
    pub domination_factor: u32,
    /// The implied `va_start` must be aligned to this quantum.
    pub alignment: u32,
    /// The admitted extent's HIGH end is rounded out to this page size, so the
    /// mapping covers the body of the function the last landing prologue opens
    /// rather than stopping at its entry word. The low end is never padded: it
    /// is a voted function entry, which is already a boundary.
    pub extent_page: u32,
}

impl Default for RelocatedSliceConfig {
    fn default() -> Self {
        Self {
            min_votes: 3,
            domination_factor: 2,
            alignment: 4,
            extent_page: 0x1000,
        }
    }
}

/// Why a relocated-slice vote refused to admit. Every variant carries the
/// counts that would have had to differ, so `Open` is a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelocatedSliceOpenReason {
    /// No call target lay outside every mapping AND inside addressable RDRAM.
    NoVoteSources,
    /// The ROM holds no classic `addiu $sp,$sp,-N` prologue at all.
    NoLandingSites,
    /// No (target, prologue) pair survived alignment.
    NoDeltaCandidates,
    InsufficientVotes { top_votes: u32, required: u32 },
    /// The runner-up explains a comparable share of the same call evidence.
    /// An exact tie lands here for any `domination_factor >= 1`.
    NearTie {
        top_votes: u32,
        runner_up_votes: u32,
        required_factor: u32,
    },
    /// The implied extent runs past the end of the ROM image.
    ExtentOutsideRom {
        rom_start: u32,
        rom_end: u32,
        rom_len: u32,
    },
    /// The implied VA range is not KSEG0/KSEG1 RDRAM.
    ExtentNotAddressable { va_start: u32, va_end: u32 },
    /// The implied VA range overlaps a mapping that already exists. Admitting
    /// it would put two banks at one address -- a contradiction, not a second
    /// residency.
    VaOverlapsExistingMapping { va_start: u32, va_end: u32 },
    /// `sources x prologues` exceeded the enumeration bound. A RESOURCE
    /// frontier, never an absence of evidence: the vote did not run.
    VoteWorkLimitExceeded {
        sources: u32,
        prologues: u32,
        limit: u64,
    },
}

/// Bound on `|sources| x |prologues|` pairs the relocated-slice histogram may
/// enumerate. See the refusal site for the measured corpus numbers.
const MAX_RELOCATED_VOTE_PAIRS: u64 = 64_000_000;

/// One admitted relocated slice: the SAME ROM bytes, executed at a second VA.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelocatedSlice {
    pub rom_start: u32,
    pub rom_end: u32,
    pub va_start: u32,
    pub va_end: u32,
    pub delta: u32,
    /// Distinct call targets that landed on a prologue under `delta`.
    pub votes: u32,
    /// Distinct call targets the best alternative delta explained.
    pub runner_up_votes: u32,
    /// Vote sources considered (call targets outside every mapping, in RDRAM).
    pub sources: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelocatedSliceOutcome {
    Admitted(RelocatedSlice),
    Open {
        reason: RelocatedSliceOpenReason,
        /// Vote sources considered, so an `Open` is auditable.
        sources: u32,
    },
}

/// ROM byte offsets of every classic `addiu $sp,$sp,-N` prologue in
/// `rom_bytes`, ascending. The relocated-slice vote's landing sites are
/// ROM-WIDE because the slice's own extent is precisely what the vote is
/// trying to find, so it cannot be scanned for first.
pub fn whole_rom_prologue_offsets(rom_bytes: &[u8]) -> Vec<u32> {
    rom_bytes
        .chunks_exact(4)
        .enumerate()
        .filter_map(|(index, chunk)| {
            let word = u32::from_be_bytes(chunk.try_into().expect("four-byte chunk"));
            is_classic_prologue(word).then_some((index as u32) * 4)
        })
        .collect()
}

/// Vote a relocated boot-image slice into existence from calls that leave
/// every known mapping.
///
/// # The mechanism
///
/// Boot code that copies part of its own image to a second address and calls
/// it there leaves two halves of one fact in the ROM: the CALL TARGETS are
/// absolute VAs recorded in already-proven code, and the FUNCTION ENTRIES they
/// name are `addiu $sp,$sp,-N` prologues sitting at fixed ROM offsets. A single
/// `delta = target - prologue_offset` reconciling many (target, prologue) pairs
/// is the slice's load address, by exactly the argument
/// [`infer_region_delta`] makes -- with the one difference that is the whole
/// point of this step: the voting calls are NOT inside the candidate region.
/// [`infer_region_delta`] votes only with a region's own internal `jal`s, so a
/// slice whose callers all live in the boot bank scores zero there. Measured on
/// the seven corpus ROMs of this class, every one of them scored zero.
///
/// # Why ROM-space overlap with the boot copy is allowed HERE
///
/// The untabled strategy drops any candidate whose ROM extent overlaps a proven
/// physical mapping, because re-admitting the boot image under a second name
/// would inflate coverage past the load evidence. That rule is about the same
/// bytes at the SAME address. This step's evidence is the opposite: proven code
/// calls these bytes at an address the boot copy does not cover, so the second
/// mapping is a second RESIDENCY, not a duplicate. The disjointness that must
/// still hold is therefore in VA space, and it is enforced below -- a candidate
/// whose VA range touches an existing mapping is refused.
///
/// # Admission
///
/// Concluded `Supported`, never `Proven`, on the same bar [`DeltaVoteConfig`]
/// uses: `top >= min_votes` distinct targets and `top >= factor * runner_up`.
/// Instruction bytes plus a call target do not prove a copy ever ran.
///
/// `targets` are the distinct call destinations outside every mapping (the
/// caller supplies them from proven code); `existing_va_ranges` are the VA
/// intervals already mapped. Pure function of its inputs: byte-identical
/// output for byte-identical input.
pub fn relocated_slice_vote(
    rom_bytes: &[u8],
    targets: &[u32],
    existing_va_ranges: &[(u32, u32)],
    config: &RelocatedSliceConfig,
) -> RelocatedSliceOutcome {
    assert!(
        config.alignment.is_power_of_two(),
        "alignment must be a power of two"
    );
    assert!(
        config.extent_page.is_power_of_two(),
        "extent page must be a power of two"
    );
    assert!(
        config.domination_factor >= 1,
        "domination factor must be at least 1"
    );

    // Only addressable KSEG0 RDRAM destinations vote. A target beyond the
    // largest RDRAM a retail N64 reaches cannot be where any code lives,
    // whatever called it -- the same rule [`addressable`] enforces for
    // whole-region proofs, and the reason the corpus's ">= 0x80800000"
    // frontier class is value-set imprecision rather than missing code.
    let sources: BTreeSet<u32> = targets
        .iter()
        .copied()
        .filter(|&target| {
            target.is_multiple_of(4)
                && (KSEG0_BASE..KSEG0_BASE.saturating_add(RDRAM_LEN)).contains(&target)
                && !existing_va_ranges
                    .iter()
                    .any(|&(start, end)| target >= start && target < end)
        })
        .collect();
    let source_count = sources.len() as u32;
    let open = |reason| RelocatedSliceOutcome::Open {
        reason,
        sources: source_count,
    };
    if sources.is_empty() {
        return open(RelocatedSliceOpenReason::NoVoteSources);
    }

    let prologues = whole_rom_prologue_offsets(rom_bytes);
    if prologues.is_empty() {
        return open(RelocatedSliceOpenReason::NoLandingSites);
    }
    // The histogram is |sources| x |prologues| entries, so both factors are
    // bounded. Measured across the seven corpus ROMs of this class: 9-57
    // sources against 1,074-3,355 prologues, four orders of magnitude inside
    // this bound. Exceeding it means the caller handed this step something
    // other than an authority-projected refusal set, and refusing loudly is
    // correct -- a resource frontier is not an answer.
    if (sources.len() as u64).saturating_mul(prologues.len() as u64) > MAX_RELOCATED_VOTE_PAIRS {
        return open(RelocatedSliceOpenReason::VoteWorkLimitExceeded {
            sources: source_count,
            prologues: prologues.len() as u32,
            limit: MAX_RELOCATED_VOTE_PAIRS,
        });
    }

    // Histogram delta -> DISTINCT targets landing on a prologue under it.
    // Distinct targets, not pairs: a slice with one popular callee must not
    // manufacture domination out of a single lucky pairing.
    let alignment_mask = config.alignment - 1;
    let mut votes: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for &target in &sources {
        for &prologue in &prologues {
            let delta = target.wrapping_sub(prologue);
            if delta & alignment_mask != 0 {
                continue;
            }
            votes.entry(delta).or_default().insert(target);
        }
    }
    if votes.is_empty() {
        return open(RelocatedSliceOpenReason::NoDeltaCandidates);
    }

    // Rank by distinct-target votes desc, delta asc -- deterministic.
    let mut top: Option<(u32, u32)> = None;
    let mut runner: Option<(u32, u32)> = None;
    for (&delta, landed) in &votes {
        let count = landed.len() as u32;
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
    let (delta, top_votes) = top.expect("nonempty vote histogram has a top");
    let runner_up_votes = runner.map_or(0, |(_, count)| count);

    if top_votes < config.min_votes {
        return open(RelocatedSliceOpenReason::InsufficientVotes {
            top_votes,
            required: config.min_votes,
        });
    }
    if runner_up_votes > 0 && top_votes < config.domination_factor.saturating_mul(runner_up_votes) {
        return open(RelocatedSliceOpenReason::NearTie {
            top_votes,
            runner_up_votes,
            required_factor: config.domination_factor,
        });
    }

    // Extent = the ROM offsets the winning delta's own voters named. The two
    // ends are NOT symmetric, and treating them as if they were is wrong in a
    // way that was measured:
    //
    // * The LOW end is the first voted entry exactly. A voted offset is a
    //   function ENTRY -- a proven boundary -- so nothing that voted lies
    //   below it, and padding down claims bytes no evidence reaches. On
    //   NASCAR 99 that padding pushed `va_start` 0xae0 bytes back INTO the
    //   boot bank's own VA range and the whole slice was refused for an
    //   overlap that the rounding itself had manufactured.
    // * The HIGH end rounds OUT to a page, because the last voted offset is
    //   an entry too: the function's BODY continues past it, and stopping at
    //   the entry word would leave its own instructions outside the mapping.
    let landed = votes.get(&delta).expect("top delta was enumerated");
    let offsets: Vec<u32> = landed
        .iter()
        .map(|&target| target.wrapping_sub(delta))
        .collect();
    let lowest = *offsets.iter().min().expect("top delta has voters");
    let highest = *offsets.iter().max().expect("top delta has voters");
    let page = config.extent_page;
    let rom_start = lowest;
    let Some(rom_end) = highest.checked_add(page).map(|end| end & !(page - 1)) else {
        return open(RelocatedSliceOpenReason::ExtentOutsideRom {
            rom_start,
            rom_end: u32::MAX,
            rom_len: rom_bytes.len() as u32,
        });
    };
    let rom_len = rom_bytes.len() as u32;
    if rom_end <= rom_start || rom_end > rom_len {
        return open(RelocatedSliceOpenReason::ExtentOutsideRom {
            rom_start,
            rom_end,
            rom_len,
        });
    }

    let va_start = rom_start.wrapping_add(delta);
    let va_end = rom_end.wrapping_add(delta);
    if !addressable(va_start, va_end) {
        return open(RelocatedSliceOpenReason::ExtentNotAddressable { va_start, va_end });
    }
    // VA-space disjointness. ROM-space overlap with the boot copy is the point
    // of this step; two banks at one ADDRESS never is.
    if existing_va_ranges
        .iter()
        .any(|&(start, end)| va_start < end && start < va_end)
    {
        return open(RelocatedSliceOpenReason::VaOverlapsExistingMapping { va_start, va_end });
    }

    RelocatedSliceOutcome::Admitted(RelocatedSlice {
        rom_start,
        rom_end,
        va_start,
        va_end,
        delta,
        votes: top_votes,
        runner_up_votes,
        sources: source_count,
    })
}
