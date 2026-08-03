//! Corpus check for mechanical strategy selection.
//!
//! `run_discovery_auto` has to pick a composition strategy without being told
//! what game it is looking at. The valuable property is not that it picks a
//! strategy -- it is that the strategies DISCRIMINATE: on each ROM the right
//! one corroborates and the wrong one recovers nothing. Selection by
//! "whichever proved the most mappings" would still be luck if both fired.
//!
//! ROM-gated by the same env vars the discover gates use. An unset var is a
//! loud skip, never a silent pass.

use fn64_discover::{run_discovery_auto, DiscoveryStrategy, StrategyOutcome};

/// A ROM with no table of either family still yields geometry, inferred from
/// `jal` statistics -- and it is admitted at Supported, never Proven.
#[test]
fn a_rom_with_no_table_falls_back_to_inferred_geometry() {
    let Some(bytes) = rom_bytes("FN64_DISCOVER_GE_ROM") else {
        eprintln!("skip: FN64_DISCOVER_GE_ROM unset");
        return;
    };
    let auto = run_discovery_auto(&bytes).expect("GoldenEye normalizes");

    // Neither table family finds anything here; that is the precondition.
    for strategy in [
        DiscoveryStrategy::RecoveredVrom,
        DiscoveryStrategy::RecoveredOverlays,
    ] {
        assert_eq!(outcome(&auto.outcomes, strategy).admitted_tables, 0);
    }
    assert_eq!(auto.selected, DiscoveryStrategy::UntabledDeltaVote);

    let inferred = outcome(&auto.outcomes, DiscoveryStrategy::UntabledDeltaVote);
    assert!(inferred.supported_mappings > 0, "no region was inferred");
    // The whole point of the evidence class: inferred geometry is never proven.
    assert_eq!(
        inferred.proven_mappings, 1,
        "only the boot bank may be proven; inferred regions must stay Supported"
    );
}

fn rom_bytes(var: &str) -> Option<Vec<u8>> {
    let path = std::env::var_os(var)?;
    Some(std::fs::read(&path).unwrap_or_else(|error| {
        panic!("{var} is set but unreadable ({path:?}): {error}");
    }))
}

fn outcome(outcomes: &[StrategyOutcome], strategy: DiscoveryStrategy) -> &StrategyOutcome {
    outcomes
        .iter()
        .find(|outcome| outcome.strategy == strategy)
        .unwrap_or_else(|| panic!("no outcome recorded for {strategy:?}"))
}

#[test]
fn oot_selects_the_vrom_strategy_and_the_overlay_strategy_finds_nothing() {
    let Some(bytes) = rom_bytes("FN64_DISCOVER_OOT_ROM") else {
        eprintln!("skip: FN64_DISCOVER_OOT_ROM unset");
        return;
    };
    let auto = run_discovery_auto(&bytes).expect("OoT normalizes");

    assert_eq!(auto.selected, DiscoveryStrategy::RecoveredVrom);
    let vrom = outcome(&auto.outcomes, DiscoveryStrategy::RecoveredVrom);
    // gate_closure composes 923 physical banks for OoT with its hardcoded
    // per-game strategy. The auto path must reach the same geometry with no
    // per-game input at all; a floor rather than an equality so a genuine
    // discovery improvement is not a test failure, but a collapse is.
    assert!(
        vrom.proven_mappings >= 900,
        "OoT recovered only {} proven mappings (gate_closure composes 923)",
        vrom.proven_mappings
    );
    assert!(vrom.admitted_tables > 0);

    // The discrimination property: the AKI-shaped strategy must NOT fire here.
    let overlays = outcome(&auto.outcomes, DiscoveryStrategy::RecoveredOverlays);
    assert_eq!(
        overlays.admitted_tables, 0,
        "the physical-ROM overlay strategy admitted {} table(s) on a VROM-shaped ROM",
        overlays.admitted_tables
    );
}

#[test]
fn nwxe_selects_the_overlay_strategy_and_the_vrom_strategy_finds_nothing() {
    let Some(bytes) = rom_bytes("FN64_DISCOVER_NWXE_ROM") else {
        eprintln!("skip: FN64_DISCOVER_NWXE_ROM unset");
        return;
    };
    let auto = run_discovery_auto(&bytes).expect("NWXE normalizes");

    assert_eq!(auto.selected, DiscoveryStrategy::RecoveredOverlays);
    let overlays = outcome(&auto.outcomes, DiscoveryStrategy::RecoveredOverlays);
    assert!(overlays.admitted_tables > 0);
    assert!(
        overlays.proven_mappings > 1,
        "only the boot bank was proven"
    );

    let vrom = outcome(&auto.outcomes, DiscoveryStrategy::RecoveredVrom);
    assert_eq!(
        vrom.admitted_tables, 0,
        "the VROM strategy admitted {} table(s) on a physical-ROM-shaped ROM",
        vrom.admitted_tables
    );
}

#[test]
fn nw4e_recovers_its_descriptor_table_without_the_hardcoded_location() {
    let Some(bytes) = rom_bytes("FN64_DISCOVER_NW4E_ROM") else {
        eprintln!("skip: FN64_DISCOVER_NW4E_ROM unset");
        return;
    };
    let auto = run_discovery_auto(&bytes).expect("NW4E normalizes");

    // gate_closure still hands NW4E `aki_reference::NW4E_DESCRIPTOR_TABLE`, a
    // per-game constant. The mechanical overlay search finds that table on its
    // own, so the constant is no longer load-bearing for composition.
    assert_eq!(auto.selected, DiscoveryStrategy::RecoveredOverlays);
    let overlays = outcome(&auto.outcomes, DiscoveryStrategy::RecoveredOverlays);
    assert!(
        overlays.admitted_tables > 0,
        "NW4E's descriptor table was not recovered mechanically"
    );
}
