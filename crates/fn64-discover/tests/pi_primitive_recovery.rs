//! Locating the PI DMA primitive with no table, no answer key, no emulator.
//!
//! PI registers sit at a fixed absolute address, so an access must materialize
//! `0xA460` with `lui`, and the IPL3 boot image is always uncompressed at an
//! address the ROM header states. That makes the primitive findable on ROMs
//! where every table-shape search returns zero candidates.

use fn64_discover::pi_dma::recover_pi_primitives;

const BOOT_COPY_ROM_START: usize = 0x1000;
const BOOT_COPY_LEN: usize = 0x10_0000;

fn boot_image(var: &str) -> Option<(Vec<u8>, u32)> {
    let path = std::env::var_os(var)?;
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{var} unreadable: {error}"));
    let entry = u32::from_be_bytes(bytes[8..12].try_into().expect("ROM header"));
    let end = (BOOT_COPY_ROM_START + BOOT_COPY_LEN).min(bytes.len());
    Some((bytes[BOOT_COPY_ROM_START..end].to_vec(), entry))
}

/// Every ROM here composes to the boot bank alone under every table search --
/// GoldenEye, Perfect Dark and WCW World Tour find zero candidate tables of
/// either family. The primitive is still located.
#[test]
fn the_pi_primitive_is_located_without_any_table() {
    let corpus = [
        ("Super Mario 64", "FN64_DISCOVER_SM64_ROM"),
        ("GoldenEye 007", "FN64_DISCOVER_GE_ROM"),
        ("Perfect Dark", "FN64_DISCOVER_PD_ROM"),
        ("WCW World Tour", "FN64_DISCOVER_WCWWT_ROM"),
    ];
    let mut checked = 0;
    for (name, var) in corpus {
        let Some((image, va_start)) = boot_image(var) else {
            eprintln!("skip {name}: {var} unset");
            continue;
        };
        let primitives = recover_pi_primitives(&image, va_start);
        assert!(
            !primitives.is_empty(),
            "{name}: no routine drives the PI registers, which cannot be true of a \
             ROM that loads anything"
        );
        let best = &primitives[0];
        // The primitive writes the register block several times -- source
        // address, destination address, length -- so a single incidental
        // reference is not it. Sorting puts the most register-driving first.
        assert!(
            best.register_sites >= 2,
            "{name}: strongest candidate has only {} PI access(es)",
            best.register_sites
        );
        assert_eq!(best.register_sites as usize, best.register_site_pcs.len());
        assert!(best
            .register_site_pcs
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert!(
            best.entry_va >= va_start,
            "{name}: entry 0x{:08x} precedes the image base 0x{va_start:08x}",
            best.entry_va
        );
        checked += 1;
    }
    eprintln!("located a PI primitive on {checked} ROM(s) with no table");
}

/// Pins the known limitation so it cannot be mistaken for a regression.
///
/// Majora's Mask's DMA wrapper is at 0x80090270 -- established independently by
/// a live capture, where 888 of 889 transfers issued from it. It has no `jal`
/// caller in the boot image, because its callers live in `code`, a separately
/// loaded file. Locating callers therefore needs a composed image, not the boot
/// copy alone.
#[test]
fn callers_are_not_generally_in_the_boot_image() {
    let Some((image, va_start)) = boot_image("FN64_DISCOVER_MM_ROM") else {
        eprintln!("skip: FN64_DISCOVER_MM_ROM unset");
        return;
    };
    let primitives = recover_pi_primitives(&image, va_start);
    assert!(!primitives.is_empty(), "MM drives the PI registers");
    let callers: usize = primitives.iter().map(|p| p.callers.len()).sum();
    assert_eq!(
        callers, 0,
        "boot-image caller recovery unexpectedly found {callers}; if this now works, \
         the doc comment and the composed-image guidance need updating"
    );
}
