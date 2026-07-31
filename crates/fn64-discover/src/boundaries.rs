//! Answer-key-free function-boundary recovery.
//!
//! Recovers the list of function starts (+ proven byte extents where
//! available) for a ROM's boot bank AND its mechanically-recovered overlay
//! banks, from ROM bytes and discovery facts only -- no decomp answer key.
//! Shared by the `gate_recover_boundaries` emitter and the
//! `gate_recompiler_lint` correctness checker.
//!
//! Reuses the proven composition pipeline: overlay recovery + correct
//! materialization of relocated/compressed bytes + authoritative-entry-fenced
//! partition + exact-owner proof. A boundary is a machine-checked callable
//! entry (a `DirectJal` target is sound at any proof state -- a real `jal`
//! proves the target is called; other evidence needs Supported/Proven
//! corroboration), whose first word is a real non-zero instruction.

use crate::banks::{self, BankNamePattern};
use crate::delta_vote::DeltaVoteConfig;
use crate::facts::{FunctionEntryEvidence, ProofState, RomAddressSpace};
use crate::overlay_regions::SearchConfig;
use crate::owner_proof::OwnerAssessment;
use crate::snapshot::{compose_materialized_banks_v1, MaterializedBankInput, ProgramSnapshotV1};
use crate::{run_discovery_with_recovered_overlay_regions, Fact, FactDb, RecoveredOverlayInput};
use std::collections::{BTreeMap, BTreeSet};

/// One proven bank's ROM/VA mapping.
#[derive(Debug, Clone)]
pub struct BankMapping {
    pub bank: String,
    pub rom_start: u32,
    pub rom_end: u32,
    pub va_start: u32,
    pub va_end: u32,
}

/// One recovered function boundary.
#[derive(Debug, Clone)]
pub struct Boundary {
    pub bank: String,
    /// Function start VA (a machine-checked callable entry).
    pub entry: u32,
    /// Exclusive end VA, present only when the exact-owner proof succeeded.
    pub va_end: Option<u32>,
}

/// Everything the boundary recovery produced, retained so a lint can reason
/// over the CFG/partition/owners without re-running discovery.
pub struct RecoveredProgram {
    /// Proven bank mappings, boot first then overlays.
    pub banks: Vec<BankMapping>,
    /// Decoded big-endian words per bank (parallel to `banks`).
    pub bank_words: Vec<Vec<u32>>,
    /// One snapshot per bank (parallel to `banks`): CFG, partition, owners.
    pub snapshots: Vec<ProgramSnapshotV1>,
    /// The sound function-boundary list, sorted by (bank, entry).
    pub boundaries: Vec<Boundary>,
    pub facts: FactDb,
}

/// Recover boundaries for an AKI-family ROM, answer-key-free.
pub fn recover_boundaries(rom_bytes: &[u8]) -> Result<RecoveredProgram, String> {
    let search = SearchConfig::aki_family();
    let input = RecoveredOverlayInput {
        min_mapped_regions: search.min_records,
        search,
        delta_vote: DeltaVoteConfig::default(),
        table_name: "recovered_overlay_descriptors".to_string(),
        bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    let (rom, facts, _recovery) = run_discovery_with_recovered_overlay_regions(rom_bytes, &input)
        .map_err(|error| error.to_string())?;

    let boot = mapping_of(&facts, |b| b == banks::BOOT_BANK)?
        .into_iter()
        .next()
        .ok_or_else(|| "no proven resident boot bank mapping".to_string())?;
    let mut overlays = mapping_of(&facts, |b| b != banks::BOOT_BANK)?;
    overlays.sort_by(|l, r| l.bank.cmp(&r.bank));
    let all: Vec<BankMapping> = std::iter::once(boot).chain(overlays).collect();

    // Materialize bytes + traversal roots (must outlive composition).
    let mut bank_bytes: Vec<&[u8]> = Vec::with_capacity(all.len());
    let mut bank_roots: Vec<Vec<u32>> = Vec::with_capacity(all.len());
    for mapping in &all {
        let bytes = rom
            .bytes
            .get(mapping.rom_start as usize..mapping.rom_end as usize)
            .ok_or_else(|| {
                format!(
                    "{} ROM interval [0x{:x},0x{:x}) is outside the normalized image",
                    mapping.bank, mapping.rom_start, mapping.rom_end
                )
            })?;
        bank_bytes.push(bytes);
        bank_roots.push(roots_for(&facts, mapping, false));
    }
    let bank_words: Vec<Vec<u32>> = bank_bytes
        .iter()
        .map(|b| {
            b.chunks_exact(4)
                .map(|c| u32::from_be_bytes(c.try_into().unwrap()))
                .collect()
        })
        .collect();
    let inputs: Vec<MaterializedBankInput> = all
        .iter()
        .enumerate()
        .map(|(i, mapping)| MaterializedBankInput {
            bank: &mapping.bank,
            va_start: mapping.va_start,
            bytes: bank_bytes[i],
            seed_roots: &bank_roots[i],
        })
        .collect();
    let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs)
        .map_err(|error| format!("composing banks: {error}"))?;

    // Proven byte extents, keyed by (bank, entry).
    let mut extent_of: BTreeMap<(String, u32), u32> = BTreeMap::new();
    for (i, snapshot) in snapshots.iter().enumerate() {
        for bank_snap in &snapshot.banks {
            for assessment in &bank_snap.owner_proof.assessments {
                if let OwnerAssessment::Proven { owner } = assessment {
                    extent_of.insert((all[i].bank.clone(), owner.entry.pc), owner.va_end);
                }
            }
        }
    }

    // The sound boundary list: machine-checked callable entries whose first
    // word is a non-zero instruction (drops jal-shaped data in the staging
    // tail). A proven extent is attached where available.
    let mut boundaries: Vec<Boundary> = Vec::new();
    for (i, mapping) in all.iter().enumerate() {
        let words = &bank_words[i];
        for entry in roots_for(&facts, mapping, true) {
            let off = ((entry - mapping.va_start) / 4) as usize;
            match words.get(off) {
                Some(&w) if w != 0 => {}
                _ => continue,
            }
            boundaries.push(Boundary {
                bank: mapping.bank.clone(),
                entry,
                va_end: extent_of.get(&(mapping.bank.clone(), entry)).copied(),
            });
        }
    }
    boundaries.sort_by(|a, b| (&a.bank, a.entry).cmp(&(&b.bank, b.entry)));
    boundaries.dedup_by(|a, b| a.bank == b.bank && a.entry == b.entry);

    Ok(RecoveredProgram {
        banks: all,
        bank_words,
        snapshots,
        boundaries,
        facts,
    })
}

/// Proven ROM mappings (boot or overlays per `keep`), enforcing the physical,
/// extent-equal invariants the composer relies on.
fn mapping_of(facts: &FactDb, keep: impl Fn(&str) -> bool) -> Result<Vec<BankMapping>, String> {
    let mut out = Vec::new();
    for fact in facts.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_space,
            rom_start,
            rom_end,
            va_start,
            va_end,
        } = fact
        else {
            unreachable!("proven_rom_mappings returned a non-mapping fact")
        };
        if !keep(bank) {
            continue;
        }
        if *rom_space != RomAddressSpace::Physical {
            return Err(format!("bank {bank} is not physically ROM-backed"));
        }
        if rom_end.checked_sub(*rom_start) != va_end.checked_sub(*va_start) {
            return Err(format!("bank {bank} has unequal ROM and VA extents"));
        }
        out.push(BankMapping {
            bank: bank.clone(),
            rom_start: *rom_start,
            rom_end: *rom_end,
            va_start: *va_start,
            va_end: *va_end,
        });
    }
    Ok(out)
}

/// Answer-key-free callable entries for a bank.
///
/// `boundary_only`: when true, restrict to the SOUND boundary subset (a
/// `DirectJal` target at any proof state -- a real `jal` proves the target is
/// called; other evidence needs Supported/Proven). When false, admit Candidate
/// state too -- those are TRAVERSAL seeds that expose reachable code for the
/// CFG, verbatim from `gate_owners_overlays::callable_roots`.
fn roots_for(facts: &FactDb, mapping: &BankMapping, boundary_only: bool) -> Vec<u32> {
    let mut roots: BTreeSet<u32> = facts
        .proven_function_entries(&mapping.bank)
        .into_iter()
        .collect();
    for fact in facts.facts() {
        let Fact::FunctionEntryClaim {
            target,
            evidence,
            proposed_state,
            ..
        } = fact
        else {
            continue;
        };
        let is_direct_jal = matches!(evidence, FunctionEntryEvidence::DirectJal { .. });
        let state_ok = if boundary_only {
            is_direct_jal || matches!(proposed_state, ProofState::Supported | ProofState::Proven)
        } else {
            matches!(
                proposed_state,
                ProofState::Candidate | ProofState::Supported | ProofState::Proven
            )
        };
        if target.bank != mapping.bank
            || target.pc < mapping.va_start
            || target.pc >= mapping.va_end
            || !state_ok
            || !matches!(
                evidence,
                FunctionEntryEvidence::DirectJal { .. }
                    | FunctionEntryEvidence::ResolvedJalr { .. }
                    | FunctionEntryEvidence::ExhaustiveIndirectCall { .. }
                    | FunctionEntryEvidence::TableEntry { .. }
                    | FunctionEntryEvidence::HandlerTablePointer { .. }
            )
        {
            continue;
        }
        roots.insert(target.pc);
    }
    roots.into_iter().collect()
}
