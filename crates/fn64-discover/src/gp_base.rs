//! IDO small-data `$gp` base recovery and gp-relative data xref surfacing.
//!
//! MIPS o32 code (the IDO/gcc toolchains the AKI titles were built with)
//! reaches small global data through the global pointer: a boot idiom sets
//! `$gp` (register 28) once to a fixed base, and every small-data access is
//! then `lw/sw/addiu $r, off($gp)`. fn64's HI/LO xref scan ([`crate::xref`])
//! cannot resolve those accesses, because the base lives in `$gp` rather than
//! in a `lui` local to the straight-line run. This module recovers the base
//! by constrained voting and then re-runs the same style of access scan with
//! `$gp` bound to the admitted base.
//!
//! # Discipline
//!
//! This follows the crate's ENUMERATE / VALIDATE / ADMIT-on-uniqueness rule.
//! Base hypotheses are enumerated (from boot `lui`+`addiu`/`ori` constructions
//! of `$gp`, and, as a bounded fallback, from the access-offset histogram).
//! Each is validated by how many gp-relative accesses resolve, under that
//! base, into the mapped data range. A base is admitted ONLY when it is unique
//! with a dominating vote margin; two comparably-voted bases leave the base
//! [`GpBaseOutcome::Open`]. Nothing here is promoted by raw score alone.
//!
//! # Evidence class
//!
//! Every emitted site is **candidate** evidence, exactly like
//! [`crate::xref::GlobalRefSite`]: a linear decode pairs a `$gp`-based access
//! with the voted base. It never proves the access executes, that the base is
//! live at that PC, or anything about the target bytes. The admitted base is a
//! typed conclusion about the *program's* `$gp`, not about any one site.

use fn64_cpu_runtime::{decode, Instruction};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// MIPS o32 global-pointer register number.
const GP: u8 = 28;

/// A `$gp` base hypothesis and the source that proposed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpBaseCandidate {
    /// The proposed 32-bit `$gp` value.
    pub base: u32,
    /// How this candidate was enumerated.
    pub source: GpBaseSource,
}

/// Where a [`GpBaseCandidate`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpBaseSource {
    /// A boot `lui $gp, HI ; addiu/ori $gp, $gp, LO` construction. `def_pc`
    /// is the PC of the low-half instruction that completed the base.
    BootConstruction { def_pc: u32 },
    /// The bounded access-offset histogram fallback nominated this base
    /// because it places the most accesses in range. Only consulted when no
    /// boot construction is found.
    OffsetHistogram,
}

/// The width and direction of one gp-relative access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpAccessKind {
    Load {
        width: u8,
    },
    Store {
        width: u8,
    },
    /// `addiu $r, $gp, off` — a small-data pointer materialized, not a
    /// memory access.
    Address,
}

/// One gp-relative access site as decoded from the bank, before any base is
/// applied. `off` is the signed 16-bit displacement from `$gp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpAccess {
    pub pc: u32,
    pub off: i16,
    pub kind: GpAccessKind,
}

impl GpAccess {
    /// Resolve this access's target address under a concrete `$gp` base.
    pub fn resolved(&self, base: u32) -> u32 {
        base.wrapping_add(self.off as i32 as u32)
    }
}

/// A candidate gp-relative cross-reference: an access resolved under the
/// admitted base. Same evidence class as [`crate::xref::GlobalRefSite`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpRefSite {
    /// PC of the access instruction.
    pub pc: u32,
    /// The resolved data address (`base + off`).
    pub addr: u32,
    pub kind: GpAccessKind,
}

/// Vote tally for one candidate base against the mapped data range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpBaseVote {
    pub candidate: GpBaseCandidate,
    /// Accesses whose resolved address is inside `[data_start, data_end)`.
    pub in_range: u32,
    /// Accesses whose resolved address is outside that range (a red flag).
    pub out_of_range: u32,
}

impl GpBaseVote {
    fn score(&self) -> u32 {
        self.in_range
    }
}

/// The mapped data range a base is voted against: `[start, end)`. Derived by
/// the caller from the resident boot mapping and the entry-stub zero-fill BSS
/// facts (text end .. BSS end); small data and `.sbss` live inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataRange {
    pub start: u32,
    pub end: u32,
}

impl DataRange {
    pub fn contains(&self, addr: u32) -> bool {
        addr >= self.start && addr < self.end
    }
}

/// The outcome of gp-base recovery over one bank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpBaseOutcome {
    /// A unique base won with a dominating margin.
    Admitted {
        base: u32,
        source: GpBaseSource,
        /// Accesses this base places in range vs. total gp-relative accesses.
        explained: u32,
        total: u32,
        /// Of the explained accesses' neighbors: how many of ALL accesses
        /// resolve out of range under the admitted base (a red flag if high).
        out_of_range: u32,
    },
    /// Two or more bases voted comparably; no unique winner. Reports the top
    /// contenders so the ambiguity is auditable.
    Open { contenders: Vec<GpBaseVote> },
    /// No gp-relative accesses were seen, so there is nothing to base.
    NoGpAccesses,
}

/// The full result of a gp-base analysis over one bank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpBaseAnalysis {
    pub outcome: GpBaseOutcome,
    /// Every enumerated candidate's vote, sorted by descending in-range score
    /// then ascending base (deterministic).
    pub votes: Vec<GpBaseVote>,
    /// Total gp-relative accesses decoded from the bank.
    pub total_accesses: u32,
    /// The candidate gp-relative xref sites under the admitted base. Empty
    /// unless the outcome is [`GpBaseOutcome::Admitted`].
    pub sites: Vec<GpRefSite>,
}

/// A dominating winner must be the unique maximum AND beat the runner-up by at
/// least this many in-range accesses. Real boot constructions win by a large
/// absolute margin (usually the only construction, so the runner-up is a
/// distinct wrong base explaining almost nothing). This absolute floor keeps a
/// one-access fluke, or a near-duplicate histogram base, from being promoted:
/// a near-tie stays Open by design.
const DOMINANCE_MARGIN: u32 = 2;

/// A winner must also explain at least this many accesses to be admitted at
/// all; a base that resolves a handful of accesses in a large bank is not
/// evidence of the program's `$gp`.
const MIN_EXPLAINED: u32 = 4;

/// A real IDO/gcc `$gp` base is at least word-aligned (the compiler emits it
/// as `lui`+`addiu` of an aligned link-time symbol; the small-data area it
/// points into is word-granular). An unaligned winning base is therefore not
/// a plausible `$gp` — it is the signature of the histogram fallback fitting
/// noise, where `data_start - off` for an odd offset lands an arbitrary
/// address. Admission requires alignment; an unaligned winner leaves the field
/// Open with the vote reported.
fn is_plausible_base(base: u32) -> bool {
    base & 0x3 == 0
}

/// Scan `bytes` (a bank mapped at `va_start`) for every gp-relative access
/// (`off($gp)` load/store, or `addiu $r, $gp, off`). Pure and deterministic;
/// results are ordered by ascending PC. This never tracks or clears `$gp`:
/// the whole premise is that `$gp` holds a single fixed base for the program,
/// so any `off($gp)` is a small-data access regardless of straight-line
/// context.
pub fn scan_gp_accesses(bytes: &[u8], va_start: u32) -> Vec<GpAccess> {
    let mut accesses = Vec::new();
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let word = u32::from_be_bytes(chunk.try_into().expect("four-byte chunk"));
        let pc = va_start.wrapping_add((index as u32).wrapping_mul(4));
        let (off, kind) = match decode(word) {
            Instruction::Lb { base, off, .. } | Instruction::Lbu { base, off, .. }
                if base == GP =>
            {
                (off, GpAccessKind::Load { width: 1 })
            }
            Instruction::Lh { base, off, .. } | Instruction::Lhu { base, off, .. }
                if base == GP =>
            {
                (off, GpAccessKind::Load { width: 2 })
            }
            Instruction::Lw { base, off, .. } | Instruction::Lwu { base, off, .. }
                if base == GP =>
            {
                (off, GpAccessKind::Load { width: 4 })
            }
            Instruction::Ld { base, off, .. } if base == GP => {
                (off, GpAccessKind::Load { width: 8 })
            }
            Instruction::Sb { base, off, .. } if base == GP => {
                (off, GpAccessKind::Store { width: 1 })
            }
            Instruction::Sh { base, off, .. } if base == GP => {
                (off, GpAccessKind::Store { width: 2 })
            }
            Instruction::Sw { base, off, .. } if base == GP => {
                (off, GpAccessKind::Store { width: 4 })
            }
            Instruction::Sd { base, off, .. } if base == GP => {
                (off, GpAccessKind::Store { width: 8 })
            }
            // `addiu $r, $gp, off` materializes a small-data pointer. Exclude
            // the base-construction site itself (`addiu $gp, $gp, LO`): that
            // is not a data access, it completes the base.
            Instruction::Addiu { rt, rs, imm } if rs == GP && rt != GP => {
                (imm, GpAccessKind::Address)
            }
            _ => continue,
        };
        accesses.push(GpAccess { pc, off, kind });
    }
    accesses
}

/// Enumerate `$gp` base candidates from boot `$gp` constructions in the bank.
///
/// The recognized idiom is `lui $gp, HI` followed (in the same straight-line
/// run, tracking only `$gp`) by `addiu $gp, $gp, LO` or `ori $gp, $gp, LO`.
/// A completed `lui`+low pair yields ONE candidate: the full base, with the
/// low-half PC as `def_pc`. A `lui $gp, HI` that is never completed before a
/// control transfer or a re-write of `$gp` yields the bare 64K-aligned base
/// (some bases are 64K-aligned and have no low half). The intermediate high
/// is never emitted as its own hypothesis when a low half completes it — that
/// would manufacture a spurious rival base and defeat the dominance test.
pub fn enumerate_boot_constructions(bytes: &[u8], va_start: u32) -> Vec<GpBaseCandidate> {
    let mut candidates = Vec::new();
    // A pending `lui $gp, HI`: its value and the PC that established it. Held
    // until a low half completes it (emit the full base) or it is discarded by
    // a control transfer / re-write of `$gp` (emit the bare high).
    let mut pending: Option<(u32, u32)> = None;
    let mut clear_after_this_word = false;

    let flush_bare = |pending: &mut Option<(u32, u32)>, candidates: &mut Vec<GpBaseCandidate>| {
        if let Some((high, high_pc)) = pending.take() {
            candidates.push(GpBaseCandidate {
                base: high,
                source: GpBaseSource::BootConstruction { def_pc: high_pc },
            });
        }
    };

    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let word = u32::from_be_bytes(chunk.try_into().expect("four-byte chunk"));
        let pc = va_start.wrapping_add((index as u32).wrapping_mul(4));
        let this_is_delay_slot = clear_after_this_word;
        clear_after_this_word = false;

        match decode(word) {
            Instruction::Lui { rt, imm } if rt == GP => {
                // A second lui before completion discards the first as bare.
                flush_bare(&mut pending, &mut candidates);
                pending = Some(((imm as u32) << 16, pc));
            }
            Instruction::Addiu { rt, rs, imm } if rt == GP && rs == GP => {
                if let Some((high, _)) = pending.take() {
                    candidates.push(GpBaseCandidate {
                        base: high.wrapping_add(imm as i32 as u32),
                        source: GpBaseSource::BootConstruction { def_pc: pc },
                    });
                }
            }
            Instruction::Ori { rt, rs, imm } if rt == GP && rs == GP => {
                if let Some((high, _)) = pending.take() {
                    candidates.push(GpBaseCandidate {
                        base: high | imm as u32,
                        source: GpBaseSource::BootConstruction { def_pc: pc },
                    });
                }
            }
            // Any other write to $gp discards the pending high (as bare).
            other => {
                if writes_gp(&other) {
                    flush_bare(&mut pending, &mut candidates);
                }
            }
        }

        if is_control_transfer(word) {
            clear_after_this_word = true;
        }
        if this_is_delay_slot {
            flush_bare(&mut pending, &mut candidates);
        }
    }
    // A pending high at end-of-bank is a bare candidate.
    flush_bare(&mut pending, &mut candidates);

    candidates
}

/// Fallback: nominate bases from the access-offset histogram, bounded.
///
/// For each distinct access offset `off`, the hypothesis "the base places
/// this access at the data range's start" gives `base = data_start - off`.
/// Only these O(#distinct-offsets) bases are tried, so the sweep is bounded by
/// the number of accesses, not the 4 GiB address space. Returned sorted and
/// deduplicated for determinism.
pub fn enumerate_offset_histogram(accesses: &[GpAccess], data: DataRange) -> Vec<GpBaseCandidate> {
    let mut bases: Vec<u32> = accesses
        .iter()
        .map(|access| data.start.wrapping_sub(access.off as i32 as u32))
        .collect();
    bases.sort_unstable();
    bases.dedup();
    bases
        .into_iter()
        .map(|base| GpBaseCandidate {
            base,
            source: GpBaseSource::OffsetHistogram,
        })
        .collect()
}

/// Vote one candidate base against the data range over all accesses.
fn vote(candidate: GpBaseCandidate, accesses: &[GpAccess], data: DataRange) -> GpBaseVote {
    let mut in_range = 0u32;
    let mut out_of_range = 0u32;
    for access in accesses {
        if data.contains(access.resolved(candidate.base)) {
            in_range += 1;
        } else {
            out_of_range += 1;
        }
    }
    GpBaseVote {
        candidate,
        in_range,
        out_of_range,
    }
}

/// Recover the `$gp` base for one bank and surface its gp-relative xrefs.
///
/// `data` is the mapped data range (text end .. BSS end) the caller derives
/// from proven boot-mapping and entry-stub facts. The vote counts how many
/// accesses each base resolves into that range; the unique dominating base is
/// admitted, otherwise the outcome is Open.
pub fn analyze(bytes: &[u8], va_start: u32, data: DataRange) -> GpBaseAnalysis {
    let accesses = scan_gp_accesses(bytes, va_start);
    let total_accesses = accesses.len() as u32;

    if accesses.is_empty() {
        return GpBaseAnalysis {
            outcome: GpBaseOutcome::NoGpAccesses,
            votes: Vec::new(),
            total_accesses: 0,
            sites: Vec::new(),
        };
    }

    // Enumerate: boot constructions first; only if none exist does the
    // histogram fallback run. Both feed the same vote.
    let boot = enumerate_boot_constructions(bytes, va_start);
    let candidates = if boot.is_empty() {
        enumerate_offset_histogram(&accesses, data)
    } else {
        boot
    };

    // Vote every distinct candidate base. Distinct bases only: two
    // constructions of the same value are one hypothesis. Keep the earliest
    // (lowest def_pc / stable) source for reporting.
    let mut best_by_base: BTreeMap<u32, GpBaseCandidate> = BTreeMap::new();
    for candidate in candidates {
        best_by_base.entry(candidate.base).or_insert(candidate);
    }
    let mut votes: Vec<GpBaseVote> = best_by_base
        .into_values()
        .map(|candidate| vote(candidate, &accesses, data))
        .collect();
    // Deterministic order: descending in-range, ascending base.
    votes.sort_by(|a, b| {
        b.score()
            .cmp(&a.score())
            .then(a.candidate.base.cmp(&b.candidate.base))
    });

    let outcome = decide(&votes);
    let sites = match &outcome {
        GpBaseOutcome::Admitted { base, .. } => resolve_sites(&accesses, *base),
        _ => Vec::new(),
    };

    GpBaseAnalysis {
        outcome,
        votes,
        total_accesses,
        sites,
    }
}

/// Apply the uniqueness/dominance rule to a descending-sorted vote list.
fn decide(votes: &[GpBaseVote]) -> GpBaseOutcome {
    let Some(winner) = votes.first() else {
        return GpBaseOutcome::NoGpAccesses;
    };
    if winner.in_range < MIN_EXPLAINED {
        // Nothing clears the floor: report the field as Open rather than
        // admit a base that explains almost nothing.
        return GpBaseOutcome::Open {
            contenders: top_contenders(votes),
        };
    }
    let runner_up = votes.get(1).map(|v| v.in_range).unwrap_or(0);
    let dominates = winner.in_range > runner_up
        && winner.in_range.saturating_sub(runner_up) >= DOMINANCE_MARGIN;
    if dominates && is_plausible_base(winner.candidate.base) {
        GpBaseOutcome::Admitted {
            base: winner.candidate.base,
            source: winner.candidate.source,
            explained: winner.in_range,
            total: total_from(votes),
            out_of_range: winner.out_of_range,
        }
    } else {
        GpBaseOutcome::Open {
            contenders: top_contenders(votes),
        }
    }
}

/// Total accesses is the same for every vote (each votes over all accesses);
/// read it off the winner.
fn total_from(votes: &[GpBaseVote]) -> u32 {
    votes
        .first()
        .map(|v| v.in_range + v.out_of_range)
        .unwrap_or(0)
}

/// The contenders to report on an Open outcome: every vote tied with the top
/// score, plus the immediate runner-up for context (bounded, deterministic).
fn top_contenders(votes: &[GpBaseVote]) -> Vec<GpBaseVote> {
    let Some(top) = votes.first() else {
        return Vec::new();
    };
    let mut out: Vec<GpBaseVote> = votes
        .iter()
        .filter(|v| v.in_range == top.in_range)
        .copied()
        .collect();
    // Include the next-lower score once, so a near-miss is visible.
    if let Some(next) = votes.iter().find(|v| v.in_range < top.in_range) {
        out.push(*next);
    }
    out
}

/// Under an admitted base, resolve each access into a candidate xref site.
fn resolve_sites(accesses: &[GpAccess], base: u32) -> Vec<GpRefSite> {
    accesses
        .iter()
        .map(|access| GpRefSite {
            pc: access.pc,
            addr: access.resolved(base),
            kind: access.kind,
        })
        .collect()
}

fn writes_gp(instruction: &Instruction) -> bool {
    use Instruction::*;
    match instruction {
        Add { rd, .. }
        | Addu { rd, .. }
        | Sub { rd, .. }
        | Subu { rd, .. }
        | And { rd, .. }
        | Or { rd, .. }
        | Xor { rd, .. }
        | Nor { rd, .. }
        | Slt { rd, .. }
        | Sltu { rd, .. }
        | Sll { rd, .. }
        | Srl { rd, .. }
        | Sra { rd, .. }
        | Sllv { rd, .. }
        | Srlv { rd, .. }
        | Srav { rd, .. }
        | Daddu { rd, .. }
        | Dsubu { rd, .. }
        | Mfhi { rd }
        | Mflo { rd }
        | Jalr { rd, .. } => *rd == GP,
        Addi { rt, .. }
        | Addiu { rt, .. }
        | Slti { rt, .. }
        | Sltiu { rt, .. }
        | Andi { rt, .. }
        | Ori { rt, .. }
        | Xori { rt, .. }
        | Lui { rt, .. }
        | Lw { rt, .. }
        | Lwu { rt, .. }
        | Lb { rt, .. }
        | Lbu { rt, .. }
        | Lh { rt, .. }
        | Lhu { rt, .. }
        | Ld { rt, .. }
        | Daddiu { rt, .. }
        | Mfc0 { rt, .. }
        | Mfc1 { rt, .. } => *rt == GP,
        Jal { .. } => GP == 31,
        _ => false,
    }
}

fn is_control_transfer(word: u32) -> bool {
    use crate::cfg::{classify_control, ControlOp};
    matches!(
        classify_control(word),
        ControlOp::J { .. }
            | ControlOp::Jal { .. }
            | ControlOp::Jr { .. }
            | ControlOp::Jalr { .. }
            | ControlOp::Branch { .. }
            | ControlOp::BranchLikely { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assemble(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    // Encoders for the exact words the tests need.
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

    const DATA_START: u32 = 0x8004_0000;
    const DATA_END: u32 = 0x8004_8000;
    const RANGE: DataRange = DataRange {
        start: DATA_START,
        end: DATA_END,
    };
    // A base in the middle of the small-data window, as IDO emits.
    const BASE: u32 = 0x8004_4000;

    #[test]
    fn known_boot_stub_admits_its_base_and_resolves_accesses() {
        // lui $gp,0x8004 ; addiu $gp,$gp,0x4000 ; then five small-data
        // accesses, all inside the data range under the true base.
        let words = [
            lui(GP, 0x8004),
            addiu(GP, GP, 0x4000),
            lw(2, GP, -0x10),
            sw(2, GP, 0x20),
            lw(4, GP, 0x100),
            sw(5, GP, 0x40),
            lw(6, GP, -0x20),
        ];
        let bytes = assemble(&words);
        let analysis = analyze(&bytes, 0x8000_0400, RANGE);
        match analysis.outcome {
            GpBaseOutcome::Admitted {
                base,
                explained,
                total,
                out_of_range,
                ..
            } => {
                assert_eq!(base, BASE);
                assert_eq!(explained, 5);
                assert_eq!(total, 5);
                assert_eq!(out_of_range, 0);
            }
            other => panic!("expected Admitted, got {other:?}"),
        }
        assert_eq!(analysis.sites.len(), 5);
        assert_eq!(analysis.sites[0].addr, BASE - 0x10);
        assert_eq!(analysis.sites[1].addr, BASE + 0x20);
        assert_eq!(analysis.sites[2].addr, BASE + 0x100);
    }

    #[test]
    fn a_completed_lui_addiu_pair_is_one_base_not_two() {
        // The intermediate `lui $gp,HI` high must NOT be emitted as its own
        // rival hypothesis when the following addiu completes it; otherwise
        // the bare high would defeat the dominance test on a real stub.
        let bytes = assemble(&[lui(GP, 0x8004), addiu(GP, GP, 0x4000)]);
        let candidates = enumerate_boot_constructions(&bytes, 0x8000_0400);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].base, BASE);
    }

    #[test]
    fn an_uncompleted_lui_gp_yields_the_bare_aligned_base() {
        // lui $gp,HI ; jr $ra (control transfer, no low half) -> bare base.
        let bytes = assemble(&[lui(GP, 0x8004), 0x03e0_0008, 0x0000_0000]);
        let candidates = enumerate_boot_constructions(&bytes, 0x8000_0400);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].base, 0x8004_0000);
    }

    #[test]
    fn base_construction_site_is_not_itself_an_access() {
        // The addiu $gp,$gp,LO that builds the base must not be counted as a
        // gp-relative Address access.
        let bytes = assemble(&[lui(GP, 0x8004), addiu(GP, GP, 0x4000), lw(2, GP, 0)]);
        let accesses = scan_gp_accesses(&bytes, 0x8000_0400);
        assert_eq!(accesses.len(), 1);
        assert_eq!(accesses[0].kind, GpAccessKind::Load { width: 4 });
    }

    #[test]
    fn genuine_tie_between_two_bases_is_open() {
        // Two distinct $gp constructions, each explaining exactly the same
        // number of accesses in range, with a range admitting both equally.
        // Base A=0x80044000, Base B=0x80045000. Two accesses at off 0 and off
        // 0x1000. Range [0x44000,0x47000):
        //   under A: {0x44000(in), 0x45000(in)} => 2
        //   under B: {0x45000(in), 0x46000(in)} => 2
        // Tie -> Open (uniqueness fails). Neither MIN_EXPLAINED gate nor
        // dominance can promote a base here.
        let words = [
            lui(GP, 0x8004),
            addiu(GP, GP, 0x4000),
            0x1000_0001, // beq $zero,$zero,+1  (control transfer)
            0x0000_0000, // delay nop
            lui(GP, 0x8004),
            addiu(GP, GP, 0x5000),
            lw(2, GP, 0x0),
            lw(2, GP, 0x1000),
        ];
        let bytes = assemble(&words);
        let range = DataRange {
            start: 0x8004_4000,
            end: 0x8004_7000,
        };
        let analysis = analyze(&bytes, 0x8000_0400, range);
        match analysis.outcome {
            GpBaseOutcome::Open { contenders } => {
                assert!(contenders.iter().any(|c| c.candidate.base == 0x8004_4000));
                assert!(contenders.iter().any(|c| c.candidate.base == 0x8004_5000));
                assert!(contenders.iter().filter(|c| c.in_range == 2).count() >= 2);
            }
            other => panic!("expected Open on a genuine tie, got {other:?}"),
        }
    }

    #[test]
    fn a_near_tie_within_the_margin_stays_open() {
        // Winner explains 5, runner-up explains 4: a one-access lead is under
        // DOMINANCE_MARGIN, so the field stays Open rather than promoting a
        // fluke winner.
        let words = [
            lui(GP, 0x8004),
            addiu(GP, GP, 0x4000), // base A = 0x80044000
            0x1000_0001,
            0x0000_0000,
            lui(GP, 0x8004),
            addiu(GP, GP, 0x4008), // base B = 0x80044008 (8 bytes apart)
            // Accesses chosen so A gets 5 in range, B gets 4 in range within
            // a narrow window that clips one of B's.
            lw(2, GP, 0x0),
            lw(2, GP, 0x4),
            lw(2, GP, 0x8),
            lw(2, GP, 0xc),
            lw(2, GP, 0x10),
        ];
        let bytes = assemble(&words);
        // Range [0x44000, 0x44015): under A(0x44000) the five accesses land at
        // 0x44000,0x44004,0x44008,0x4400c,0x44010 -> all 5 in. Under
        // B(0x44008) they land at 0x44008,0x4400c,0x44010,0x44014,0x44018 ->
        // 0x44018 clips, so 4 in. Lead is 1 (< margin) -> Open.
        let range = DataRange {
            start: 0x8004_4000,
            end: 0x8004_4015,
        };
        let analysis = analyze(&bytes, 0x8000_0400, range);
        assert!(
            matches!(analysis.outcome, GpBaseOutcome::Open { .. }),
            "one-access lead must stay Open, got {:?}",
            analysis.outcome
        );
    }

    #[test]
    fn out_of_range_accesses_reduce_a_bases_vote() {
        // One base construction; one access resolves outside the data range.
        // The in-range vote counts only the in-range ones; out_of_range is
        // reported as the red flag.
        let words = [
            lui(GP, 0x8004),
            addiu(GP, GP, 0x4000),
            lw(2, GP, 0x0),              // 0x44000 in
            lw(2, GP, 0x10),             // 0x44010 in
            lw(2, GP, 0x20),             // 0x44020 in
            lw(2, GP, 0x30),             // 0x44030 in
            lw(2, GP, 0x7000u16 as i16), // 0x4b000 out (>= 0x48000)
        ];
        let bytes = assemble(&words);
        let analysis = analyze(&bytes, 0x8000_0400, RANGE);
        match analysis.outcome {
            GpBaseOutcome::Admitted {
                explained,
                out_of_range,
                total,
                ..
            } => {
                assert_eq!(explained, 4);
                assert_eq!(out_of_range, 1);
                assert_eq!(total, 5);
            }
            other => panic!("expected Admitted with reduced vote, got {other:?}"),
        }
    }

    #[test]
    fn no_gp_accesses_reports_nogpaccesses() {
        // A bank that builds $gp but never uses it (and no other gp access).
        let bytes = assemble(&[lui(GP, 0x8004), addiu(GP, GP, 0x4000)]);
        let analysis = analyze(&bytes, 0x8000_0400, RANGE);
        assert_eq!(analysis.outcome, GpBaseOutcome::NoGpAccesses);
        assert!(analysis.sites.is_empty());
    }

    #[test]
    fn histogram_fallback_runs_when_no_boot_construction_and_can_admit() {
        // No $gp construction in the bank at all. The histogram fallback must
        // run (candidate source = OffsetHistogram). A dominating winner is
        // admitted only when it beats every neighbor base by the margin: here
        // the accesses all share ONE offset repeated, so exactly one base
        // catches them all and every distinct rival catches zero.
        let words = vec![
            lw(2, GP, 0x40),
            lw(3, GP, 0x40),
            lw(4, GP, 0x40),
            lw(5, GP, 0x40),
            lw(6, GP, 0x40),
        ];
        let bytes = assemble(&words);
        // Only one distinct offset -> one histogram candidate base
        // (data_start - 0x40). It catches all five; no rival exists, so it
        // wins with runner-up 0.
        let range = DataRange {
            start: 0x8004_4000,
            end: 0x8004_4100,
        };
        let analysis = analyze(&bytes, 0x8000_0400, range);
        match analysis.outcome {
            GpBaseOutcome::Admitted {
                base,
                source,
                explained,
                ..
            } => {
                assert_eq!(source, GpBaseSource::OffsetHistogram);
                assert_eq!(base, 0x8004_4000 - 0x40);
                assert_eq!(explained, 5);
            }
            other => panic!("expected histogram Admitted, got {other:?}"),
        }
    }

    #[test]
    fn an_unaligned_winning_base_is_rejected_as_implausible() {
        // A histogram winner at an unaligned address is the signature of the
        // fallback fitting noise (real $gp is word-aligned). Even a unanimous,
        // dominating unaligned base must NOT be admitted -- the field stays
        // Open. Offset 0x41 (odd) against data_start 0x80044000 yields base
        // 0x80043fbf, which is unaligned.
        let words = vec![
            lw(2, GP, 0x41),
            lw(3, GP, 0x41),
            lw(4, GP, 0x41),
            lw(5, GP, 0x41),
            lw(6, GP, 0x41),
        ];
        let bytes = assemble(&words);
        let range = DataRange {
            start: 0x8004_4000,
            end: 0x8004_4100,
        };
        let analysis = analyze(&bytes, 0x8000_0400, range);
        assert!(
            matches!(analysis.outcome, GpBaseOutcome::Open { .. }),
            "unaligned base must stay Open, got {:?}",
            analysis.outcome
        );
    }

    #[test]
    fn histogram_fallback_stays_open_on_a_diffuse_cluster() {
        // A realistic diffuse small-data cluster: neighboring histogram bases
        // differ by only one access, so no base dominates by the margin. This
        // is the honest "the histogram cannot pin the exact base" outcome, not
        // a failure -- it is exactly why boot construction is preferred.
        let mut words = Vec::new();
        for off in [0i16, 0x10, 0x20, 0x30, 0x40] {
            words.push(lw(2, GP, off));
        }
        let bytes = assemble(&words);
        let range = DataRange {
            start: 0x8004_4000,
            end: 0x8004_4041,
        };
        let analysis = analyze(&bytes, 0x8000_0400, range);
        assert!(
            matches!(analysis.outcome, GpBaseOutcome::Open { .. }),
            "diffuse histogram cluster must stay Open, got {:?}",
            analysis.outcome
        );
    }

    #[test]
    fn analysis_is_deterministic() {
        let words = [
            lui(GP, 0x8004),
            addiu(GP, GP, 0x4000),
            lw(2, GP, 0x0),
            sw(3, GP, 0x8),
            lw(4, GP, 0x100),
        ];
        let bytes = assemble(&words);
        let a = analyze(&bytes, 0x8000_0400, RANGE);
        let b = analyze(&bytes, 0x8000_0400, RANGE);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }
}
