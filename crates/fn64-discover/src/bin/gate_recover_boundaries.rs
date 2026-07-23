//! Answer-key-FREE function-boundary recovery.
//!
//! Emits the list of proven function starts + byte extents for a ROM's boot
//! bank AND its mechanically-recovered overlay banks, using ONLY ROM bytes
//! and discovery facts -- no decomp answer key. This is the foundation an
//! answer-key-free recompiler lint (or a symbol bootstrap) needs: every other
//! discovery gate funnels these same owners into `grade_functions` against a
//! key; this one just prints them.
//!
//! The pipeline is the proven one from `gate_owners_overlays`:
//!   1. `run_discovery_with_recovered_overlay_regions` recovers the overlay
//!      banks mechanically (descriptor-table family enumeration), materializes
//!      relocated/compressed overlay bytes correctly (NOT a flat boot-VA scan),
//!      and produces machine-checked `FunctionEntryClaim` facts.
//!   2. Each proven bank is materialized and seeded with `callable_roots`
//!      (proven function entries + direct/indirect-call claims).
//!   3. `compose_materialized_banks_v1` builds the CFG, fences the partition
//!      with cross-bank jal authority (so owners cannot over-extend or
//!      overlap -- the `same_bank_overlaps` invariant holds), and proves exact
//!      owners (`OwnerAssessment::Proven`).
//! Only Proven owners are emitted -- Candidate/Ambiguous frontiers are the
//! honest "not sound enough" set, held back exactly as wrong==0 requires.
//!
//! Output: JSON-lines, one `{bank, entry, va_end, bytes}` per proven owner,
//! sorted by (bank, entry). A summary goes to stderr.
//!
//! Env:  FN64_DISCOVER_ROM   the game's .z64  (AKI family; no answer key)

use fn64_discover::banks::{self, BankNamePattern};
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::facts::{FunctionEntryEvidence, ProofState, RomAddressSpace};
use fn64_discover::overlay_regions::SearchConfig;
use fn64_discover::owner_proof::OwnerAssessment;
use fn64_discover::snapshot::{compose_materialized_banks_v1, MaterializedBankInput};
use fn64_discover::{
    required_env_path, run_discovery_with_recovered_overlay_regions, Fact, FactDb,
    RecoveredOverlayInput,
};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
struct BankMapping {
    bank: String,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_recover_boundaries: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rom_path = required_env_path("FN64_DISCOVER_ROM", "the game's .z64")
        .map_err(|error| error.to_string())?;
    let rom_bytes =
        std::fs::read(&rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;

    // Mechanical, answer-key-free overlay recovery (AKI descriptor family).
    let search = SearchConfig::aki_family();
    let input = RecoveredOverlayInput {
        min_mapped_regions: search.min_records,
        search,
        delta_vote: DeltaVoteConfig::default(),
        table_name: "recovered_overlay_descriptors".to_string(),
        bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    let (rom, facts, _recovery) =
        run_discovery_with_recovered_overlay_regions(&rom_bytes, &input)
            .map_err(|error| error.to_string())?;

    let boot = boot_mapping(&facts)?;
    let overlays = overlay_mappings(&facts)?;
    let all: Vec<BankMapping> = std::iter::once(boot.clone()).chain(overlays).collect();

    // Materialize every bank's bytes + roots (they must outlive composition).
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
        bank_roots.push(callable_roots(&facts, mapping));
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

    // A function BOUNDARY (start) is any machine-checked callable root: a
    // direct/indirect jal target, a reloc R_MIPS_26 entry, a table entry, or a
    // proven function entry. These are sound function starts on their own --
    // the entrypoint/jal-target authority does not depend on the whole owner
    // proving exhaustively. So the boundary list is the union of every bank's
    // `callable_roots`, and we ATTACH a proven byte extent (`va_end`) wherever
    // the exact-owner proof succeeded (most starts get a start-only entry --
    // an owner blocked by an interior `UnresolvedIndirect` is still a real
    // function). `exact=true` marks a start with a proven extent.
    let mut extent_of: std::collections::BTreeMap<(String, u32), u32> =
        std::collections::BTreeMap::new();
    let mut per_bank: Vec<(String, usize, usize)> = Vec::new();
    for (i, snapshot) in snapshots.iter().enumerate() {
        let mut proven = 0usize;
        let mut total = 0usize;
        for bank_snap in &snapshot.banks {
            for assessment in &bank_snap.owner_proof.assessments {
                total += 1;
                if let OwnerAssessment::Proven { owner } = assessment {
                    extent_of.insert((all[i].bank.clone(), owner.entry.pc), owner.va_end);
                    proven += 1;
                }
            }
        }
        per_bank.push((all[i].bank.clone(), proven, total));
    }
    // (bank, start_va, Option<va_end>). A boundary is a machine-checked
    // callable entry (see `sound_boundaries`) whose first word is a real,
    // non-zero instruction. The instruction guard drops the handful of
    // `jal`-shaped DATA words in the staging tail past real code (round
    // addresses like 0x80040000 that a data word coincidentally targets) --
    // a genuine function never starts on a zero word.
    let mut boundaries: Vec<(String, u32, Option<u32>)> = Vec::new();
    for (i, mapping) in all.iter().enumerate() {
        let words = &bank_words[i];
        for start in sound_boundaries(&facts, mapping) {
            let off = ((start - mapping.va_start) / 4) as usize;
            match words.get(off) {
                Some(&w) if w != 0 => {}
                _ => continue, // out of bank, or a zero word -> not code
            }
            let ext = extent_of.get(&(mapping.bank.clone(), start)).copied();
            boundaries.push((mapping.bank.clone(), start, ext));
        }
    }
    boundaries.sort_by(|a, b| (a.0.clone(), a.1).cmp(&(b.0.clone(), b.1)));
    boundaries.dedup();

    eprintln!("recover-boundaries: {} banks, per-bank proven/total owners:", all.len());
    for (bank, proven, total) in &per_bank {
        eprintln!("  {bank}: {proven}/{total} proven");
    }
    if std::env::var_os("FN64_LINT_BLOCKERS").is_some() {
        for (i, snapshot) in snapshots.iter().enumerate() {
            for bank_snap in &snapshot.banks {
                let mut top: Vec<_> = bank_snap.blocker_histogram.iter().collect();
                top.sort_by_key(|s| std::cmp::Reverse(s.sole_blocker_assessments));
                let shown: Vec<String> = top
                    .iter()
                    .take(5)
                    .map(|s| format!("{:?}={}(sole {})", s.kind, s.affected_assessments, s.sole_blocker_assessments))
                    .collect();
                eprintln!("  [{}] blockers: {}", all[i].bank, shown.join(" "));
            }
        }
    }
    let with_extent = boundaries.iter().filter(|b| b.2.is_some()).count();
    eprintln!(
        "recover-boundaries: {} function boundaries ({} with a proven byte extent)",
        boundaries.len(),
        with_extent
    );
    for (bank, entry, va_end) in &boundaries {
        match va_end {
            Some(end) => println!(
                "{{\"bank\":\"{}\",\"entry\":\"0x{:x}\",\"va_end\":\"0x{:x}\",\"bytes\":{},\"exact\":true}}",
                bank, entry, end, end.saturating_sub(*entry)
            ),
            None => println!(
                "{{\"bank\":\"{}\",\"entry\":\"0x{:x}\",\"exact\":false}}",
                bank, entry
            ),
        }
    }
    Ok(())
}

fn boot_mapping(facts: &FactDb) -> Result<BankMapping, String> {
    mapping_of(facts, |bank| bank == banks::BOOT_BANK)?
        .into_iter()
        .next()
        .ok_or_else(|| "no proven resident boot bank mapping".to_string())
}

fn overlay_mappings(facts: &FactDb) -> Result<Vec<BankMapping>, String> {
    let mut mappings = mapping_of(facts, |bank| bank != banks::BOOT_BANK)?;
    mappings.sort_by(|l, r| l.bank.cmp(&r.bank));
    Ok(mappings)
}

/// Collect proven ROM mappings (boot or overlays per `keep`), enforcing the
/// physical, extent-equal invariants the composer relies on.
fn mapping_of(
    facts: &FactDb,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<BankMapping>, String> {
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

/// Answer-key-free traversal roots for a bank (proven entries + Candidate/
/// Supported/Proven direct-jal, resolved-jalr, exhaustive-indirect-call and
/// table-entry claims), verbatim from gate_owners_overlays. Candidates are
/// admitted here because they are TRAVERSAL seeds -- they expose reachable
/// code for the CFG; whether they are real function *boundaries* is a
/// separate, stricter question answered by [`sound_boundary`].
fn callable_roots(facts: &FactDb, mapping: &BankMapping) -> Vec<u32> {
    roots_for(facts, mapping, /* boundary_only = */ false)
}

/// The SOUND boundary subset: only machine-checked entries a wrong==0
/// boundary list may emit. Drops `Candidate`-state claims (a heuristic
/// Candidate can land mid-function -> a false split, empirically 7 boot
/// starts inside real functions).
fn sound_boundaries(facts: &FactDb, mapping: &BankMapping) -> BTreeSet<u32> {
    roots_for(facts, mapping, /* boundary_only = */ true)
        .into_iter()
        .collect()
}

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
        // A DirectJal claim is machine-checked at ANY proof state: a real
        // `jal <target>` in ROM proves the target is called, so it is a sound
        // boundary even at `Candidate` function-entry state (the state tracks
        // entry INFERENCE corroboration, not the jal fact). ResolvedJalr /
        // exhaustive-indirect / table entries need corroboration
        // (Supported/Proven) to be sound. Traversal seeding (boundary_only=
        // false) admits everything.
        let is_direct_jal = matches!(evidence, FunctionEntryEvidence::DirectJal { .. });
        let state_ok = if boundary_only {
            is_direct_jal
                || matches!(proposed_state, ProofState::Supported | ProofState::Proven)
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
            )
        {
            continue;
        }
        roots.insert(target.pc);
    }
    roots.into_iter().collect()
}
