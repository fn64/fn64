//! K20 / B8: a bank whose conclusion is only `Supported` must be COMPOSED for
//! the recompile gate, so the code it holds classifies `mapped_not_proven_code`
//! (interpreter-covered `dynamic_mips`) instead of `outside_all_mappings`.
//!
//! The synthetic half of this ticket lives in `closure.rs`'s own test module,
//! because `ProgramGeometry` is private: see
//! `a_destination_inside_a_supported_bank_is_mapped_not_proven_code` and
//! `candidate_images_contribute_no_geometry_while_supported_and_proven_do`.
//! This file carries the end-to-end half: real ROMs, real discovery, and the
//! exact composition + scoreboard the recompile gate blocks on.
//!
//! The corpus tests SKIP loudly when their env var is unset -- a silent skip is
//! how a gate stops measuring anything without anyone noticing.

use fn64_discover::closure::{scoreboard, unsupported_destination_audit_v1, DestinationReason};
use fn64_discover::facts::{BankBackingV1, FactDb, ProofState};
use fn64_discover::snapshot::{
    compose_materialized_banks_admitting_supported_v2_with_limits,
    compose_materialized_banks_validated_v2_with_limits, MaterializedBankInput,
    MultiBankCompositionLimits,
};
use fn64_discover::{Fact, NormalizedRom, RomAddressSpace};

/// One physical bank ready to compose, plus whether discovery PROVED it or
/// only SUPPORTED it.
struct Bank {
    name: String,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
    supported: bool,
}

/// The recompile gate's own bank selection: proven physical affine mappings in
/// bank-name order, then Supported ones in bank-name order.
fn gate_banks(facts: &FactDb) -> Vec<Bank> {
    let mut proven: Vec<Bank> = Vec::new();
    for fact in facts.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_space: RomAddressSpace::Physical,
            rom_start,
            rom_end,
            va_start,
            va_end,
        } = fact
        else {
            continue;
        };
        proven.push(Bank {
            name: bank.clone(),
            rom_start: *rom_start,
            rom_end: *rom_end,
            va_start: *va_start,
            va_end: *va_end,
            supported: false,
        });
    }
    proven.sort_by(|left, right| left.name.cmp(&right.name));

    let mut supported: Vec<Bank> = Vec::new();
    for image in facts.supported_bank_images() {
        let BankBackingV1::RomAffine {
            rom_space: RomAddressSpace::Physical,
            rom_start,
            rom_end,
        } = image.backing
        else {
            continue;
        };
        supported.push(Bank {
            name: image.bank.clone(),
            rom_start,
            rom_end,
            va_start: image.va_start,
            va_end: image.va_end,
            supported: true,
        });
    }
    supported.sort_by(|left, right| left.name.cmp(&right.name));

    proven.extend(supported);
    proven
}

fn clone_bank(bank: &Bank) -> Bank {
    Bank {
        name: bank.name.clone(),
        rom_start: bank.rom_start,
        rom_end: bank.rom_end,
        va_start: bank.va_start,
        va_end: bank.va_end,
        supported: bank.supported,
    }
}

/// Compose `banks` and return the closure's `unsupported` count plus the exact
/// destination addresses behind it. `admit` selects the gate's composer
/// (Proven + Supported) or the Proven-only one every other consumer keeps.
fn unsupported_audit(
    rom: &NormalizedRom,
    facts: &FactDb,
    banks: &[Bank],
    admit: bool,
) -> (u64, Vec<u32>) {
    let mut bytes: Vec<&[u8]> = Vec::with_capacity(banks.len());
    for bank in banks {
        bytes.push(
            rom.bytes
                .get(bank.rom_start as usize..bank.rom_end as usize)
                .unwrap_or_else(|| {
                    panic!(
                        "{} ROM interval [{:#x},{:#x}) outside the normalized image",
                        bank.name, bank.rom_start, bank.rom_end
                    )
                }),
        );
    }
    let roots: Vec<Vec<u32>> = banks
        .iter()
        .map(|bank| {
            let mut roots: Vec<u32> = facts
                .proven_function_entries(&bank.name)
                .into_iter()
                .filter(|pc| *pc >= bank.va_start && *pc < bank.va_end)
                .collect();
            roots.sort_unstable();
            roots.dedup();
            roots
        })
        .collect();
    let inputs: Vec<MaterializedBankInput<'_>> = banks
        .iter()
        .enumerate()
        .map(|(index, bank)| MaterializedBankInput {
            bank: &bank.name,
            va_start: bank.va_start,
            bytes: bytes[index],
            seed_roots: &roots[index],
        })
        .collect();
    let limits = MultiBankCompositionLimits::default();
    let composed = if admit {
        compose_materialized_banks_admitting_supported_v2_with_limits(rom, facts, &inputs, limits)
    } else {
        compose_materialized_banks_validated_v2_with_limits(rom, facts, &inputs, limits)
    }
    .unwrap_or_else(|error| panic!("composing {} bank(s): {error}", inputs.len()));
    let snapshots = composed.snapshots();
    let refusals: Vec<u32> = unsupported_destination_audit_v1(snapshots)
        .into_iter()
        .map(|audit| audit.destination_va)
        .collect();
    (scoreboard(snapshots).unsupported, refusals)
}

/// K20's actual contract, measured end to end on a real ROM.
///
/// What K20 guarantees is a CLASSIFICATION: every destination that used to be
/// refused `outside_all_mappings` and lands inside the Supported bank's VA
/// range must now be covered by that bank. What K20 does NOT guarantee -- and
/// what the seven K18 ROMs measured -- is that the gate's total `unsupported`
/// count falls, because composing a Supported bank also composes ITS code,
/// whose calls reach further than the bank's own extent. On Waialae and
/// NASCAR 2000 that shortfall is larger than the retirement, and the total
/// rises: see "Measured outcome of K20" in
/// `docs/plans/corpus-campaign-2026-09-15.md`. That is a K18 extent finding,
/// which this test measures rather than hides.
///
/// `before` is the count this ROM measured at HEAD.
fn assert_supported_bank_is_composed_and_classified(var: &str, before: u64) {
    let Ok(path) = std::env::var(var) else {
        eprintln!(
            "SKIPPING K20 supported-bank corpus check: {var} is unset. \
             Set it to a ROM whose untabled strategy admits a Supported mapping; \
             this test is NOT evidence of anything while it is unset."
        );
        return;
    };
    let rom_bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("reading {var}: {error}"));
    let discovery = fn64_discover::run_discovery_auto(&rom_bytes)
        .unwrap_or_else(|error| panic!("{var}: automatic discovery rejected the ROM: {error:?}"));

    let banks = gate_banks(&discovery.facts);
    let supported: Vec<&Bank> = banks.iter().filter(|bank| bank.supported).collect();
    assert_eq!(
        supported.len(),
        1,
        "{var}: expected exactly one Supported bank, got {:?}",
        supported.iter().map(|bank| &bank.name).collect::<Vec<_>>()
    );
    let (slice_start, slice_end) = (supported[0].va_start, supported[0].va_end);

    // The whole point of B8's decision: composed, but NEVER relabelled.
    let conclusion = discovery
        .facts
        .conclusion(&format!("bank:{}", supported[0].name))
        .unwrap_or_else(|| panic!("{var}: the Supported bank has no conclusion"));
    assert_eq!(
        conclusion.state,
        ProofState::Supported,
        "{var}: composing a Supported bank must not promote it"
    );

    let proven_only: Vec<Bank> = banks
        .iter()
        .filter(|bank| !bank.supported)
        .map(clone_bank)
        .collect();
    let (head, head_refusals) =
        unsupported_audit(&discovery.rom, &discovery.facts, &proven_only, false);
    assert_eq!(
        head, before,
        "{var}: the Proven-only composition must still measure the recorded \
         HEAD count; if this moved, something other than K20 changed"
    );

    let (after, after_refusals) = unsupported_audit(&discovery.rom, &discovery.facts, &banks, true);

    // THE contract: nothing the Supported bank covers may still be refused.
    let still_refused_inside: Vec<u32> = after_refusals
        .iter()
        .copied()
        .filter(|va| *va >= slice_start && *va < slice_end)
        .collect();
    assert!(
        still_refused_inside.is_empty(),
        "{var}: {} destination(s) inside the composed Supported bank \
         [{slice_start:#010x},{slice_end:#010x}) are still refused: {:#010x?}",
        still_refused_inside.len(),
        still_refused_inside
    );

    // And it must actually retire some: a bank that covers nothing is not
    // evidence that composing it did anything.
    let retired: Vec<u32> = head_refusals
        .iter()
        .copied()
        .filter(|va| !after_refusals.contains(va))
        .collect();
    assert!(
        !retired.is_empty(),
        "{var}: composing the Supported bank retired no refusal at all"
    );

    // The remainder is the K18 extent gap, measured not assumed. Every new
    // refusal must come from the Supported bank's OWN code, never from a
    // proven bank that used to compose cleanly.
    let new: Vec<u32> = after_refusals
        .iter()
        .copied()
        .filter(|va| !head_refusals.contains(va))
        .collect();
    eprintln!(
        "{var}: unsupported {before} -> {after} (supported_banks=1, \
         retired={}, new={})",
        retired.len(),
        new.len()
    );
    for va in &new {
        assert!(
            *va < slice_start || *va >= slice_end,
            "{var}: {va:#010x} is inside the composed Supported bank yet newly refused"
        );
    }
}

#[test]
fn corpus_rom_a_supported_bank_is_composed_and_classified() {
    // Waialae Country Club: 33 unsupported destinations at HEAD. All 33 are
    // retired; the total nonetheless rises to 50, because the slice's own code
    // calls past its truncated extent. See the plan's K20 section.
    assert_supported_bank_is_composed_and_classified("FN64_K18_ROM_A", 33);
}

#[test]
fn corpus_rom_b_supported_bank_is_composed_and_classified() {
    // F-Zero X: 9 unsupported destinations at HEAD, 2 after -- this one does
    // fall, because its slice extent reaches everything its code calls.
    assert_supported_bank_is_composed_and_classified("FN64_K18_ROM_B", 9);
}

/// The AKI regression ROMs select `RecoveredOverlays` and must gain no
/// Supported bank at all: their `unsupported == 0` certification has to come
/// from proof, never from a placement.
fn assert_no_supported_bank(var: &str) {
    let Ok(path) = std::env::var(var) else {
        eprintln!("SKIPPING K20 AKI regression check: {var} is unset.");
        return;
    };
    let rom_bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("reading {var}: {error}"));
    let discovery = fn64_discover::run_discovery_auto(&rom_bytes)
        .unwrap_or_else(|error| panic!("{var}: automatic discovery rejected the ROM: {error:?}"));
    assert!(
        discovery.facts.supported_bank_images().is_empty(),
        "{var}: an AKI overlay ROM must certify with zero Supported banks"
    );
}

#[test]
fn aki_nwxe_gains_no_supported_bank() {
    assert_no_supported_bank("FN64_DISCOVER_NWXE_ROM");
}

#[test]
fn aki_nw4e_gains_no_supported_bank() {
    assert_no_supported_bank("FN64_DISCOVER_NW4E_ROM");
}

/// A destination that is in NO bank -- proven or supported -- must stay
/// `outside_all_mappings`. K20 widens what counts as mapped; it must not
/// weaken what counts as unmapped.
#[test]
fn the_unsupported_class_still_exists_after_the_widening() {
    // Reason-level guard, cheap and ROM-free: the two classes stay distinct
    // members of the enumeration and keep their opposite headline roles.
    assert_ne!(
        DestinationReason::MappedNotProvenCode,
        DestinationReason::OutsideAllMappings
    );
    assert!(DestinationReason::ALL.contains(&DestinationReason::OutsideAllMappings));
    assert!(DestinationReason::ALL.contains(&DestinationReason::MappedNotProvenCode));
}
