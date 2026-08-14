//! LOCAL MEASUREMENT ONLY — NOT FOR COMMIT.
//!
//! Modern-path NW4E probe: run_discovery_auto -> prepare_snapshot_banks ->
//! validated v2 multi-bank composition (with limits), then dump every
//! owner-proof assessment per composed bank (state + blocker kinds +
//! unresolved-indirect site pcs) for offline classification against the
//! answer key.
//!
//! Env: FN64_DISCOVER_ROM

use fn64_discover::owner_proof::{OwnerAssessment, OwnerBlocker};
use fn64_discover::snapshot::{
    compose_materialized_banks_validated_v2_with_limits, MultiBankCompositionLimits,
    OwnerBlockerKind,
};
use fn64_discover::snapshot_inputs::{
    prepare_snapshot_banks_with_limits, PrepareSnapshotBanksLimits,
};
use fn64_discover::{required_env_path, Fact};

fn main() {
    let rom_path = required_env_path("FN64_DISCOVER_ROM", "the game's .z64").unwrap();
    let rom_bytes = std::fs::read(&rom_path).unwrap();
    let discovery = fn64_discover::run_discovery_auto(&rom_bytes).expect("auto discovery");
    println!("STRATEGY\t{:?}", discovery.selected);
    for fact in discovery.facts.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_start,
            rom_end,
            va_start,
            va_end,
            ..
        } = fact
        else {
            continue;
        };
        println!(
            "MAPPING\t{bank}\trom=0x{rom_start:08x}..0x{rom_end:08x}\tva=0x{va_start:08x}..0x{va_end:08x}"
        );
    }
    let prepared = prepare_snapshot_banks_with_limits(
        &discovery.rom,
        &discovery.facts,
        PrepareSnapshotBanksLimits::default(),
    )
    .expect("preparing snapshot banks");
    for bank in prepared.banks() {
        println!(
            "PREPARED\t{}\tva=0x{:08x}..0x{:08x}\tseeds={}",
            bank.bank,
            bank.va_start,
            bank.va_end,
            bank.traversal_seeds.len()
        );
    }
    let inputs = prepared.materialized_inputs();
    let composed = compose_materialized_banks_validated_v2_with_limits(
        &discovery.rom,
        &discovery.facts,
        &inputs,
        MultiBankCompositionLimits::default(),
    )
    .expect("composing snapshot banks");
    for snapshot in composed.snapshots() {
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
                        "ASSESS\t{}\tproven\t0x{:08x}\t0x{:08x}\t-\t-",
                        bank, owner.entry.pc, owner.va_end
                    );
                }
                OwnerAssessment::Candidate { frontier }
                | OwnerAssessment::Ambiguous { frontier } => {
                    let state = if matches!(assessment, OwnerAssessment::Candidate { .. }) {
                        "candidate"
                    } else {
                        "ambiguous"
                    };
                    let mut kinds: Vec<String> = Vec::new();
                    let mut sites: Vec<String> = Vec::new();
                    for blocker in &frontier.blockers {
                        let label = OwnerBlockerKind::from(blocker)
                            .diagnostic_label()
                            .to_string();
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
