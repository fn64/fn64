//! Probe: what does the load-only mapping product recover from a ROM whose
//! descriptors carry no section extents?
//!
//! Batman of the Future and Bottom of the 9th both reach a single admitted
//! table but fail `admitted_overlay_load_recipes_v1` at `InvalidRangeRelations`
//! -- their descriptors have no text/data/bss to promote. This probe measures
//! what is still provable for those ROMs: the ROM interval and the destination.
//!
//! Usage: `cargo run --release -p fn64-discover --example probe_load_mapping
//! -- <rom.z64> [min_records]`
use fn64_discover::overlay_load_mapping::{admitted_overlay_load_mappings_v1, shared_slot_invalidation_range};
use fn64_discover::overlay_recipe::admitted_overlay_load_recipes_v1;
use fn64_discover::overlay_regions::{recover_overlay_regions, SearchConfig};
use fn64_discover::banks::BankNamePattern;
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::{run_discovery_with_recovered_overlay_regions, Fact, RecoveredOverlayInput};

fn main() {
    let rom = std::env::args().nth(1).expect("usage: <rom> [min_records]");
    let min_records: u32 = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(2);
    let bytes = std::fs::read(&rom).expect("read rom");
    let mut config = SearchConfig::aki_family();
    config.min_records = min_records;
    let recovery =
        recover_overlay_regions(&bytes, &config, &Default::default(), config.min_records);

    match admitted_overlay_load_recipes_v1(&bytes, &recovery) {
        Ok(recipes) => println!("recipes={}", recipes.len()),
        Err(error) => println!("recipes=ERR {error:?}"),
    }
    match admitted_overlay_load_mappings_v1(&bytes, &recovery) {
        Ok(mappings) => {
            println!("mappings={}", mappings.len());
            match shared_slot_invalidation_range(&mappings) {
                Some(range) => println!(
                    "  shared-slot invalidation [{:#x},{:#x}) size={:#x}",
                    range.start,
                    range.end,
                    range.end - range.start
                ),
                None => println!("  shared-slot invalidation: none (not one slot)"),
            }
            for mapping in mappings.iter().take(6) {
                println!(
                    "  rom {:#08x}..{:#08x} len={:#x} load={:#x?}",
                    mapping.rom_start,
                    mapping.rom_end,
                    mapping.loaded_byte_len(),
                    mapping.load_start,
                );
            }
        }
        Err(error) => println!("mappings=ERR {error:?}"),
    }

    let search = SearchConfig::aki_family();
    let input = RecoveredOverlayInput {
        min_mapped_regions: search.min_records,
        search,
        delta_vote: DeltaVoteConfig::default(),
        table_name: "recovered_overlay_descriptors".to_string(),
        bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    if let Ok((_, facts, _)) = run_discovery_with_recovered_overlay_regions(&bytes, &input) {
        for i in 0..8 {
            let name = format!("recovered_overlay_{i}");
            if let Some(c) = facts.conclusion(&format!("bank:{name}")) {
                println!("  {name}: {:?} ({})", c.state, c.rule);
            }
        }
        let n = facts.proven_rom_mappings().len();
        println!("proven banks={n}");
    }
}
