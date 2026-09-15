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

/// The K20 + K22 contract, measured end to end on a real ROM.
///
/// K20's half is a CLASSIFICATION: every destination that used to be refused
/// `outside_all_mappings` and lands inside the Supported bank's VA range must
/// now be covered by that bank. K22's half is the CONSEQUENCE that makes it
/// worth having: the gate's total `unsupported` count must actually FALL.
///
/// Both halves are asserted because K20 alone delivered only the first. With
/// K18's voted extent, composing the slice also composed ITS calls, which
/// reached past the extent, and on Waialae the total ROSE 33 -> 50 even though
/// all 33 prior refusals retired. K22's fixed point grows the extent to cover
/// those calls at the already-voted delta, and Waialae now reaches 0. See
/// "Measured outcome of K22" in `docs/plans/corpus-campaign-2026-09-15.md`.
///
/// `before` is the count this ROM measured before either ticket.
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

    // K22's fixed point runs inside discovery and composes on every round, so
    // its determinism is load-bearing: a wobbling extent would move a pinned
    // gate digest. Discovery must be a pure function of the ROM bytes.
    let second = fn64_discover::run_discovery_auto(&rom_bytes)
        .unwrap_or_else(|error| panic!("{var}: second discovery rejected the ROM: {error:?}"));
    assert_eq!(
        discovery.facts.supported_bank_images(),
        second.facts.supported_bank_images(),
        "{var}: the extent fixed point must settle identically on every run"
    );

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

    // K22: the count must FALL. This is the assertion K20 could not make.
    assert!(
        after < before,
        "{var}: unsupported must fall once the extent reaches its fixed point: \
         {before} -> {after}; {} new refusal(s) at {:#010x?}",
        new.len(),
        new
    );

    // A newly refused destination inside the composed bank would mean the
    // geometry and the composition disagree about what is mapped.
    for va in &new {
        assert!(
            *va < slice_start || *va >= slice_end,
            "{var}: {va:#010x} is inside the composed Supported bank yet newly refused"
        );
    }
}

/// K22's delay-slot refusal, on the ROM that produced it.
///
/// NASCAR 99's voted delta implies a cross-bank call landing at 0x800fcad4 in
/// the boot bank -- the DELAY SLOT of the control word at 0x800fcad0. A delay
/// slot is not a function entry, so at least one target the delta implies is
/// not one either, and the whole slice is refused with that entry named. The
/// alternative measured at K20 was worse than a refusal: composition failed
/// outright and the gate could not report a number at all.
fn assert_delay_slot_entry_refuses_the_slice(var: &str) {
    let Ok(path) = std::env::var(var) else {
        eprintln!(
            "SKIPPING K22 delay-slot refusal check: {var} is unset. \
             This test is NOT evidence of anything while it is unset."
        );
        return;
    };
    let rom_bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("reading {var}: {error}"));
    let discovery = fn64_discover::run_discovery_auto(&rom_bytes)
        .unwrap_or_else(|error| panic!("{var}: automatic discovery rejected the ROM: {error:?}"));

    assert!(
        discovery.facts.supported_bank_images().is_empty(),
        "{var}: a slice implying a delay-slot entry must be refused, not admitted"
    );

    // An Open is a MEASUREMENT, not a shrug: the entry that refused it has to
    // be recorded, or the finding is unauditable.
    let refusal = discovery.facts.facts().iter().find_map(|fact| match fact {
        Fact::Evidence { note, .. } if note.contains("DELAY SLOT") => Some(note.clone()),
        _ => None,
    });
    let refusal = refusal.unwrap_or_else(|| {
        panic!("{var}: no delay-slot refusal was recorded; an Open must name its entry")
    });
    assert!(
        refusal.contains("0x800fcad4") && refusal.contains("0x800fcad0"),
        "{var}: the refusal must name the entry and its control word: {refusal}"
    );
    eprintln!("{var}: {refusal}");
}

#[test]
fn corpus_rom_a_supported_bank_is_composed_and_classified() {
    // Waialae Country Club: 33 before either ticket, 50 after K20 alone, and
    // 0 once K22's fixed point covers what the slice's own code calls.
    assert_supported_bank_is_composed_and_classified("FN64_K18_ROM_A", 33);
}

#[test]
fn corpus_rom_b_supported_bank_is_composed_and_classified() {
    // F-Zero X: 9 before, 2 after K20 alone, 0 after K22.
    assert_supported_bank_is_composed_and_classified("FN64_K18_ROM_B", 9);
}

#[test]
fn corpus_rom_c_delay_slot_entry_refuses_the_slice() {
    assert_delay_slot_entry_refuses_the_slice("FN64_K22_ROM_C");
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

// ---------------------------------------------------------------------------
// B11 / K23: a Supported bank with zero proven blocks must not abort emission.
// ---------------------------------------------------------------------------

/// Two composed banks, synthetic and ROM-free apart from a minimal normalized
/// image: one PROVEN with an authoritative entry (so block proof admits its
/// blocks) and one SUPPORTED with none (so block proof admits nothing).
///
/// This is the exact shape the recompile gate composes for the 32 corpus ROMs
/// B11 names, reduced to the smallest thing that reproduces it.
struct TwoBankFixture {
    rom: NormalizedRom,
    composed: fn64_discover::snapshot::ValidatedComposedSnapshotsV2,
    supported_va: (u32, u32),
}

fn compose_proven_plus_supported() -> TwoBankFixture {
    use fn64_discover::facts::{
        function_entry_subject, BankAddr, CandidateDetector, FunctionEntryEvidence, ProloguePattern,
    };

    const PROVEN_BASE: u32 = 0x8000_0400;
    const PROVEN_ROM: u32 = 0x1000;
    const SUPPORTED_BASE: u32 = 0x8000_2400;
    const SUPPORTED_ROM: u32 = 0x2000;
    const NOP: u32 = 0;
    const JR_RA: u32 = 0x03e0_0008;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    fn jal(target: u32) -> u32 {
        0x0c00_0000 | (target >> 2 & 0x03ff_ffff)
    }

    // The proven bank: an `addiu sp` prologue, a CALL INTO the supported bank
    // (this is what makes the closure have a destination there at all -- it is
    // the shape B11's corpus ROMs have, where the boot bank's own code reaches
    // the relocated slice), a matched restore, a return and its delay slot.
    let proven = asm(&[
        0x27bd_ffe8,
        jal(SUPPORTED_BASE),
        NOP,
        0x27bd_0018,
        JR_RA,
        NOP,
    ]);
    // The supported bank: the same code shape, but NO authoritative entry is
    // ever concluded for it. That is what a `relocated_slice_*` /
    // `untabled_region_*` placement looks like -- the bytes are real, nothing
    // proves execution enters them, so block proof admits nothing.
    let supported = asm(&[0x27bd_ffe8, NOP, 0x27bd_0018, JR_RA, NOP, NOP]);

    let mut raw = vec![0u8; SUPPORTED_ROM as usize + supported.len()];
    raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
    raw[8..12].copy_from_slice(&PROVEN_BASE.to_be_bytes());
    raw[PROVEN_ROM as usize..PROVEN_ROM as usize + proven.len()].copy_from_slice(&proven);
    raw[SUPPORTED_ROM as usize..SUPPORTED_ROM as usize + supported.len()]
        .copy_from_slice(&supported);
    let rom = fn64_discover::rom::normalize(&raw).expect("normalizing the synthetic ROM");

    let mut facts = FactDb::new();
    let proven_mapping = facts.insert(Fact::RomMapping {
        bank: "proven_bank".into(),
        rom_space: RomAddressSpace::Physical,
        rom_start: PROVEN_ROM,
        rom_end: PROVEN_ROM + proven.len() as u32,
        va_start: PROVEN_BASE,
        va_end: PROVEN_BASE + proven.len() as u32,
    });
    facts
        .conclude(
            "bank:proven_bank",
            ProofState::Proven,
            vec![proven_mapping],
            "k23_test_proven_mapping",
        )
        .expect("concluding the proven bank");
    let entry = BankAddr::new("proven_bank", PROVEN_BASE);
    let claim = facts.insert(Fact::FunctionEntryClaim {
        target: entry.clone(),
        detector: CandidateDetector::ProloguePattern,
        evidence: FunctionEntryEvidence::Prologue {
            stack_adjust: entry.clone(),
            frame_size: 24,
            pattern: ProloguePattern::LeafWithMatchedRestore,
            corroborating_site: BankAddr::new("proven_bank", PROVEN_BASE + 8),
        },
        proposed_state: ProofState::Proven,
    });
    facts
        .conclude(
            function_entry_subject(&entry),
            ProofState::Proven,
            vec![claim],
            "k23_test_proven_entry",
        )
        .expect("concluding the proven entry");

    // The Supported half: a mapping fact whose bank conclusion is exactly
    // `Supported`, and deliberately no function-entry conclusion at all.
    let supported_mapping = facts.insert(Fact::RomMapping {
        bank: "relocated_slice_0".into(),
        rom_space: RomAddressSpace::Physical,
        rom_start: SUPPORTED_ROM,
        rom_end: SUPPORTED_ROM + supported.len() as u32,
        va_start: SUPPORTED_BASE,
        va_end: SUPPORTED_BASE + supported.len() as u32,
    });
    facts
        .conclude(
            "bank:relocated_slice_0",
            ProofState::Supported,
            vec![supported_mapping],
            "k23_test_supported_mapping",
        )
        .expect("concluding the supported bank");

    let proven_roots = [PROVEN_BASE];
    let supported_roots: [u32; 0] = [];
    let inputs = [
        MaterializedBankInput {
            bank: "proven_bank",
            va_start: PROVEN_BASE,
            bytes: &proven,
            seed_roots: &proven_roots,
        },
        MaterializedBankInput {
            bank: "relocated_slice_0",
            va_start: SUPPORTED_BASE,
            bytes: &supported,
            seed_roots: &supported_roots,
        },
    ];
    let composed = compose_materialized_banks_admitting_supported_v2_with_limits(
        &rom,
        &facts,
        &inputs,
        MultiBankCompositionLimits::default(),
    )
    .expect("composing one proven plus one supported bank");

    TwoBankFixture {
        rom,
        composed,
        supported_va: (SUPPORTED_BASE, SUPPORTED_BASE + supported.len() as u32),
    }
}

/// B11: emitting a pack for the Supported bank is what failed the whole ROM.
/// This asserts the emitter's rule is UNCHANGED (K23 does not relax it), that
/// the predicate the gate now consults separates the two banks, and that
/// packing only the emittable banks materializes instead of aborting.
#[test]
fn a_supported_bank_with_no_proven_blocks_is_not_emittable() {
    use fn64_discover::block_pack::{
        emit_validated_block_pack_v2, materialize_block_pack, snapshot_has_emittable_blocks,
        BlockPackError, BlockPackV1, BLOCK_PACK_SCHEMA_V2,
    };

    let fixture = compose_proven_plus_supported();
    let snapshots = fixture.composed.snapshots();
    assert_eq!(snapshots.len(), 2, "both banks must compose");

    let emittable: Vec<(&str, bool)> = snapshots
        .iter()
        .map(|snapshot| {
            (
                snapshot.banks[0].input.bank.as_str(),
                snapshot_has_emittable_blocks(snapshot),
            )
        })
        .collect();
    assert_eq!(
        emittable,
        vec![("proven_bank", true), ("relocated_slice_0", false)],
        "the proven bank is emittable and the Supported bank is not"
    );

    // Asking the emitter to pack the empty bank still refuses -- which is
    // exactly why the gate must ask first rather than discover it as an error.
    let error = emit_validated_block_pack_v2(&fixture.composed, 1, &fixture.rom)
        .expect_err("a bank with no proven block must still refuse emission");
    assert!(
        matches!(&error, BlockPackError::NoProvenBlocks { bank } if bank == "relocated_slice_0"),
        "expected NoProvenBlocks for the Supported bank, got {error:?}"
    );

    // The gate's post-K23 behaviour: pack only the emittable banks.
    let mut whole = BlockPackV1 {
        schema_version: BLOCK_PACK_SCHEMA_V2,
        normalized_rom_sha256: fixture.rom.sha256.clone(),
        banks: Vec::new(),
    };
    for (index, snapshot) in snapshots.iter().enumerate() {
        if !snapshot_has_emittable_blocks(snapshot) {
            continue;
        }
        let pack = emit_validated_block_pack_v2(&fixture.composed, index, &fixture.rom)
            .unwrap_or_else(|error| panic!("emitting the proven bank: {error:?}"));
        whole.banks.extend(pack.banks);
    }
    assert_eq!(whole.banks.len(), 1, "exactly the proven bank is packed");
    assert_eq!(whole.banks[0].bank, "proven_bank");
    let materialized = materialize_block_pack(&whole, &fixture.rom)
        .unwrap_or_else(|error| panic!("materializing the skipped-bank pack: {error:?}"));
    assert_eq!(materialized.len(), 1);
    assert!(!materialized[0].blocks.is_empty());
}

/// The other half of T13: skipping the pack must not cost the Supported bank
/// its geometry. Every destination in its VA range still classifies
/// `mapped_not_proven_code`, and the closure refuses nothing.
#[test]
fn a_skipped_supported_bank_still_classifies_mapped_not_proven_code() {
    let fixture = compose_proven_plus_supported();
    let snapshots = fixture.composed.snapshots();
    let (start, end) = fixture.supported_va;

    let inside: Vec<_> = fn64_discover::closure::classified_destinations(snapshots)
        .into_iter()
        .filter(|destination| destination.va >= start && destination.va < end)
        .collect();
    assert!(
        !inside.is_empty(),
        "the Supported bank's own CFG must reach the closure at all"
    );
    for destination in &inside {
        assert_eq!(
            destination.reason,
            DestinationReason::MappedNotProvenCode,
            "{:#010x} inside the skipped Supported bank must stay mapped_not_proven_code",
            destination.va
        );
    }
    assert_eq!(
        scoreboard(snapshots).unsupported,
        0,
        "a composed Supported bank leaves nothing outside all mappings"
    );
}

/// K23's corpus half: Super Mario 64 is one of the 25+ ROMs that certified on
/// the first 2026-09-15 campaign and then FAILED on the second, because K20
/// composed its `untabled_region_0` and the emitter refused the empty pack.
/// It must reach HEADLINE again, at the same `unsupported = 0`, with the
/// Supported bank counted rather than packed.
#[test]
fn corpus_sm64_reaches_headline_with_a_skipped_supported_bank() {
    let var = "FN64_K23_ROM_SM64";
    let Ok(path) = std::env::var(var) else {
        eprintln!(
            "SKIPPING K23 corpus check: {var} is unset. Set it to Super Mario 64 (USA).z64; \
             this test is NOT evidence of anything while it is unset."
        );
        return;
    };
    let binary = env!("CARGO_BIN_EXE_fn64-discover");
    let output = std::process::Command::new(binary)
        .arg("gate-rom-recompile")
        .env("FN64_DISCOVER_ROM", &path)
        .output()
        .unwrap_or_else(|error| panic!("{var}: running gate-rom-recompile: {error}"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{var}: gate-rom-recompile must certify the ROM\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    let headline = stdout
        .lines()
        .find(|line| line.starts_with("HEADLINE "))
        .unwrap_or_else(|| panic!("{var}: the gate printed no HEADLINE\n{stdout}"));
    assert!(headline.contains("unsupported=0"), "{var}: {headline}");

    let banks_line = stdout
        .lines()
        .find(|line| line.starts_with("composed_banks="))
        .unwrap_or_else(|| panic!("{var}: the gate printed no bank counts\n{stdout}"));
    let supported: u32 = banks_line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("supported_banks="))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("{var}: no supported_banks field in {banks_line}"));
    assert!(
        supported >= 1,
        "{var}: this ROM's regression is a Supported bank; got supported_banks={supported}"
    );
    assert!(
        stdout.contains(": no proven blocks, mapping only"),
        "{var}: the skipped bank must print its note\n{stdout}"
    );
    eprintln!("{var}: {banks_line} | {headline}");
}
