//! Whole-ROM load-address inference with no table of any kind.
//!
//! The falsification criterion needs no answer key and no donor: the IPL3 boot
//! copy always moves ROM 0x1000 to the address in the ROM header, so a delta
//! recovered purely from `jal` statistics can be checked against a field the
//! hardware itself uses. If the sweep's arithmetic were wrong, this could not
//! agree by accident on four unrelated ROMs.
//!
//! ROM-gated by env vars; an unset var is a loud skip, never a silent pass.

use fn64_discover::delta_vote::{sweep_untabled_regions, UntabledSweepConfig};

const BOOT_COPY_ROM_START: u32 = 0x1000;

fn rom(var: &str) -> Option<Vec<u8>> {
    let path = std::env::var_os(var)?;
    Some(std::fs::read(&path).unwrap_or_else(|error| panic!("{var} unreadable: {error}")))
}

fn header_entry(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes[8..12].try_into().expect("ROM header"))
}

/// Every ROM here composes to the boot bank ALONE under every table-shape
/// search fn64 has -- GoldenEye, WCW World Tour and Perfect Dark find zero
/// candidate tables of either family. The sweep still recovers where their code
/// loads, because a `jal` states its target absolutely and that is a property of
/// the instruction encoding rather than of any engine convention.
#[test]
fn the_sweep_recovers_the_boot_delta_without_any_table() {
    let corpus = [
        ("OoT", "FN64_DISCOVER_OOT_ROM"),
        ("GoldenEye", "FN64_DISCOVER_GE_ROM"),
        ("WCW World Tour", "FN64_DISCOVER_WCWWT_ROM"),
        ("Perfect Dark", "FN64_DISCOVER_PD_ROM"),
    ];
    let mut checked = 0;
    for (name, var) in corpus {
        let Some(bytes) = rom(var) else {
            eprintln!("skip {name}: {var} unset");
            continue;
        };
        let entry = header_entry(&bytes);
        let regions = sweep_untabled_regions(&bytes, &UntabledSweepConfig::default());
        assert!(
            !regions.is_empty(),
            "{name}: the sweep admitted no region at all"
        );
        let predicted: Vec<u32> = regions
            .iter()
            .map(|region| region.delta.wrapping_add(BOOT_COPY_ROM_START))
            .collect();
        assert!(
            predicted.contains(&entry),
            "{name}: no region's delta predicts the header entry 0x{entry:08x}; \
             predictions were {predicted:08x?}"
        );
        checked += 1;
    }
    eprintln!("verified boot-delta recovery on {checked} ROM(s)");
}

/// Super Mario 64 is the honest counter-example and is asserted so it cannot
/// quietly change. Its boot window does not dominate, so the sweep does NOT
/// recover its boot delta -- but it does recover several other regions, which is
/// the right shape for an engine that DMAs individual segments to unrelated
/// addresses rather than relying on one resident image.
#[test]
fn super_mario_64_yields_segments_rather_than_a_boot_delta() {
    let Some(bytes) = rom("FN64_DISCOVER_SM64_ROM") else {
        eprintln!("skip: FN64_DISCOVER_SM64_ROM unset");
        return;
    };
    let regions = sweep_untabled_regions(&bytes, &UntabledSweepConfig::default());
    assert!(
        regions.len() >= 2,
        "expected several independently-loaded segments, got {}",
        regions.len()
    );
    let deltas: std::collections::BTreeSet<u32> = regions.iter().map(|r| r.delta).collect();
    assert!(
        deltas.len() >= 2,
        "segments must not all share one delta, or they are one image"
    );
}
