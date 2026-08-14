//! LOCAL MEASUREMENT ONLY — NOT FOR COMMIT.
//!
//! NW4E miss-taxonomy probe: run key-free discovery with the mechanical
//! AKI recovered-overlay lane (same config gate_overlay_regions grades),
//! compose boot + every proven overlay bank, and dump the per-entry
//! owner-proof assessment state so key functions that the boot grade never
//! sees can be classified (unreached / entry-lacks-authority /
//! blocked-indirect / boundary-open).
//!
//! Env: FN64_DISCOVER_ROM

use fn64_discover::banks::{self, BankNamePattern};
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::facts::{FunctionEntryEvidence, ProofState, RomAddressSpace};
use fn64_discover::overlay_regions::SearchConfig;
use fn64_discover::owner_proof::{OwnerAssessment, OwnerBlocker};
use fn64_discover::snapshot::{compose_materialized_banks_v1, MaterializedBankInput};
use fn64_discover::{
    required_env_path, run_discovery_with_recovered_overlay_regions, Fact, FactDb,
    RecoveredOverlayInput,
};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
struct Mapping {
    bank: String,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

fn mappings(facts: &FactDb) -> Vec<Mapping> {
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
            continue;
        };
        assert_eq!(*rom_space, RomAddressSpace::Physical, "{bank} not physical");
        out.push(Mapping {
            bank: bank.clone(),
            rom_start: *rom_start,
            rom_end: *rom_end,
            va_start: *va_start,
            va_end: *va_end,
        });
    }
    out.sort_by(|a, b| a.bank.cmp(&b.bank));
    out
}

/// Same rule as gate_owners_overlays::callable_roots.
fn callable_roots(facts: &FactDb, mapping: &Mapping) -> Vec<u32> {
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
        if target.bank != mapping.bank
            || target.pc < mapping.va_start
            || target.pc >= mapping.va_end
            || !matches!(
                proposed_state,
                ProofState::Candidate | ProofState::Supported | ProofState::Proven
            )
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

fn main() {
    let rom_path = required_env_path("FN64_DISCOVER_ROM", "the game's .z64").unwrap();
    let rom_bytes = std::fs::read(&rom_path).unwrap();

    let search = SearchConfig::aki_family();
    let input = RecoveredOverlayInput {
        // NW4E's table has 5 records; gate_overlay_regions uses majority => 3.
        min_mapped_regions: 3,
        search,
        delta_vote: DeltaVoteConfig::default(),
        table_name: "recovered_overlay_descriptors".to_string(),
        bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    let (rom, facts, recovery) =
        run_discovery_with_recovered_overlay_regions(&rom_bytes, &input).unwrap();
    let admitted = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .count();
    println!(
        "RECOVERY\traw_tables={}\tadmitted_tables={}",
        recovery.candidate_tables.len(),
        admitted
    );

    let all = mappings(&facts);
    for mapping in &all {
        println!(
            "MAPPING\t{}\trom=0x{:08x}..0x{:08x}\tva=0x{:08x}..0x{:08x}",
            mapping.bank, mapping.rom_start, mapping.rom_end, mapping.va_start, mapping.va_end
        );
    }

    // Compose boot + overlays exactly as gate_owners_overlays does.
    let mut bank_bytes: Vec<&[u8]> = Vec::new();
    let mut bank_roots: Vec<Vec<u32>> = Vec::new();
    for mapping in &all {
        bank_bytes.push(&rom.bytes[mapping.rom_start as usize..mapping.rom_end as usize]);
        bank_roots.push(callable_roots(&facts, mapping));
    }
    let inputs: Vec<MaterializedBankInput> = all
        .iter()
        .enumerate()
        .map(|(index, mapping)| MaterializedBankInput {
            bank: &mapping.bank,
            va_start: mapping.va_start,
            bytes: bank_bytes[index],
            seed_roots: &bank_roots[index],
        })
        .collect();
    for (index, mapping) in all.iter().enumerate() {
        println!(
            "ROOTS\t{}\tseed_roots={}\tproven_entries={}",
            mapping.bank,
            bank_roots[index].len(),
            facts.proven_function_entries(&mapping.bank).len()
        );
    }
    let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs)
        .unwrap_or_else(|error| panic!("composition failed: {error}"));

    for snapshot in &snapshots {
        let bank_snapshot = &snapshot.banks[0];
        let bank = &bank_snapshot.input.bank;
        println!(
            "HIST\t{}\t{}",
            bank,
            serde_json::to_string(&bank_snapshot.blocker_histogram).unwrap()
        );
        println!(
            "CLOSURE\t{}\tblocks={}\tdirect_calls={}\tindirect_sites={}\tauthority_roots={}\tbroad_roots={}",
            bank,
            bank_snapshot.closure.cfg.blocks.len(),
            bank_snapshot.closure.cfg.direct_calls.len(),
            bank_snapshot.closure.cfg.indirect_sites.len(),
            bank_snapshot.authority_closure.cfg.proven_roots.len(),
            bank_snapshot.closure.cfg.proven_roots.len(),
        );
        for assessment in &bank_snapshot.owner_proof.assessments {
            match assessment {
                OwnerAssessment::Proven { owner } => {
                    println!(
                        "ASSESS\t{}\tproven\t0x{:08x}\t0x{:08x}\t-",
                        bank, owner.entry.pc, owner.va_end
                    );
                }
                OwnerAssessment::Candidate { frontier } | OwnerAssessment::Ambiguous { frontier } => {
                    let state = if matches!(assessment, OwnerAssessment::Candidate { .. }) {
                        "candidate"
                    } else {
                        "ambiguous"
                    };
                    let mut kinds: Vec<String> = Vec::new();
                    let mut sites: Vec<String> = Vec::new();
                    for blocker in &frontier.blockers {
                        let kind = fn64_discover::snapshot::OwnerBlockerKind::from(blocker);
                        let label = kind.diagnostic_label().to_string();
                        if !kinds.contains(&label) {
                            kinds.push(label);
                        }
                        if let OwnerBlocker::UnresolvedIndirect { site, scope } = blocker {
                            sites.push(format!("0x{site:08x}:{scope:?}"));
                        }
                    }
                    println!(
                        "ASSESS\t{}\t{}\t0x{:08x}\t{}\t{}\t{}",
                        bank,
                        state,
                        frontier.entry.pc,
                        frontier
                            .proposed_va_end
                            .map(|end| format!("0x{end:08x}"))
                            .unwrap_or_else(|| "-".to_string()),
                        kinds.join(";"),
                        sites.join(",")
                    );
                }
            }
        }
    }
}
