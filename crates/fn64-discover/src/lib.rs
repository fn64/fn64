//! `fn64-discover`: mechanical, zero-LLM N64 ROM function-metadata
//! recovery. This is fn64's recompiler *frontend* -- see
//! `docs/DISCOVER-DESIGN.md` for the full pipeline this crate implements
//! incrementally, phase by phase.
//!
//! # Discipline (non-negotiable, see the design doc's "Honest limit")
//!
//! Never emit a guessed symbol file. Every module here either:
//!
//! - reads a **hardware-fixed** fact directly (Phase 1 header parsing,
//!   Phase 2's boot-copy geometry), or
//! - proposes a **candidate** from a named detector and requires an
//!   explicit proof rule before promoting it, recording rejection with a
//!   reason when a candidate fails self-consistency (see [`banks`]), or
//! - accepts an **explicit, cited external claim** (e.g. a descriptor
//!   table's ROM location, from prior RE) as an input the caller must
//!   supply -- this crate never scans blind for "the" overlay table and
//!   calls that scan itself proof.
//!
//! The output of a full pipeline run is always three things: proven facts,
//! rejected candidates, and an explicit open/unresolved frontier -- never
//! only the first.
//!
//! # This crate's current scope (B1: foundation)
//!
//! - [`rom`]: Phase 1, ROM normalization + identity.
//! - [`facts`]: the monotonic fact database and discrete [`facts::ProofState`]
//!   values every conclusion in this crate is expressed in.
//! - [`banks`]: Phase 2, load-image/overlay discovery -- the boot copy
//!   (hardware-fixed, needs no scanning) and descriptor-table-shaped
//!   overlay banks (given an explicit table location/shape).
//! - [`grade_oot`] / [`grade_nw4e`]: grading-only cross-checks against the
//!   OoT decomp's segment answer key and NW4E's hand-verified
//!   `overlays.json`. Neither module is reachable from the discovery
//!   pipeline itself -- they only ever consume its output.
//!
//! Candidate harvesting (Phase 3), CFG construction (Phase 4), function
//! partitioning (Phase 5), indirect-target closure (Phase 6), dynamic
//! probes (Phase 7), and assembly verification (Phase 8) are not yet
//! implemented; this crate's public surface will grow into them.

pub mod banks;
pub mod facts;
pub mod grade_nw4e;
pub mod grade_oot;
pub mod rom;

pub use facts::{BankAddr, Fact, FactDb, ProofState};
pub use rom::{normalize, NormalizedRom, RomByteOrder, RomRejectReason};

/// An explicit, cited descriptor-table location/shape plus the naming
/// function for the banks it yields (see [`banks::scan_descriptor_table`]).
/// Named to keep [`run_discovery`]'s signature legible.
pub type DescriptorTableInput = (banks::DescriptorTableShape, fn(u32) -> String);

/// Run every currently-implemented discovery phase (1: normalize, 2: boot
/// bank + optional descriptor-table scan) over `rom_bytes` and return the
/// resulting fact database. This is the crate's single deterministic
/// entry point: calling it twice on byte-identical input must produce a
/// byte-identical `FactDb` (enforced by `tests/determinism.rs`).
///
/// `descriptor_table` is optional because not every N64 title has one
/// (OoT does not; NW4E and other AKI-family titles do) -- passing `None`
/// still yields a valid, if smaller, fact DB with just the boot bank
/// proven and nothing else claimed.
pub fn run_discovery(
    rom_bytes: &[u8],
    descriptor_table: Option<DescriptorTableInput>,
) -> Result<(NormalizedRom, FactDb), RomRejectReason> {
    let rom = rom::normalize(rom_bytes)?;
    let mut db = FactDb::new();
    banks::discover_boot_bank(&rom, &mut db);
    if let Some((shape, bank_name)) = descriptor_table {
        banks::scan_descriptor_table(&rom, shape, bank_name, &mut db);
    }
    Ok((rom, db))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_rom() -> Vec<u8> {
        let mut buf = vec![0u8; 0x1000 + 0x2000];
        buf[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        buf[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        buf[0x20..0x24].copy_from_slice(b"TEST");
        buf[0x3b..0x3f].copy_from_slice(b"CTSE");
        buf
    }

    #[test]
    fn run_discovery_without_descriptor_table_still_proves_boot_bank() {
        let bytes = make_test_rom();
        let (_rom, db) = run_discovery(&bytes, None).unwrap();
        assert_eq!(
            db.conclusion("bank:boot").unwrap().state,
            ProofState::Proven
        );
    }

    #[test]
    fn run_discovery_rejects_malformed_input_before_any_analysis() {
        let err = run_discovery(&[0u8; 4], None).unwrap_err();
        assert!(matches!(err, RomRejectReason::TooSmall { .. }));
    }

    #[test]
    fn run_discovery_is_byte_identical_across_repeated_runs() {
        let bytes = make_test_rom();
        let (_rom_a, db_a) = run_discovery(&bytes, None).unwrap();
        let (_rom_b, db_b) = run_discovery(&bytes, None).unwrap();
        let json_a = serde_json::to_string(&db_a).unwrap();
        let json_b = serde_json::to_string(&db_b).unwrap();
        assert_eq!(json_a, json_b);
    }
}
