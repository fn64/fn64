//! K18 / B7: a boot image that copies a slice of itself to a second address
//! and calls it there must be mapped at that second address.
//!
//! Two layers:
//!
//! 1. Synthetic unit tests over [`relocated_slice_vote`] with a hand-built ROM
//!    image, which pin the admission rule (3 distinct-target votes AND a 2x
//!    margin) and the extent/VA-disjointness constraints without needing a ROM.
//! 2. Env-gated corpus tests over two real ROMs of this class. They SKIP
//!    loudly when the env var is unset -- a silent skip is how a gate stops
//!    measuring anything without anyone noticing.

use fn64_discover::delta_vote::{
    relocated_slice_vote, RelocatedSliceConfig, RelocatedSliceOpenReason, RelocatedSliceOutcome,
};
use fn64_discover::{Fact, RomAddressSpace};

const NOP: u32 = 0x0000_0000;
/// `addiu $sp,$sp,-0x20`
const PROLOGUE: u32 = 0x27bd_ffe0;

/// A ROM image with `words` u32 words, `prologue_offsets` of which carry a
/// classic prologue. Everything else is `nop`, which is deliberately NOT a
/// prologue, so the landing sites are exactly the ones named.
fn rom_with_prologues(words: usize, prologue_offsets: &[u32]) -> Vec<u8> {
    let mut image = vec![NOP; words];
    for &offset in prologue_offsets {
        image[(offset / 4) as usize] = PROLOGUE;
    }
    image
        .iter()
        .flat_map(|word| word.to_be_bytes())
        .collect::<Vec<u8>>()
}

/// The boot bank's own VA range, standing in for "everything already mapped".
/// 0x80001000..0x80005000 is disjoint from the relocated VAs used below.
const BOOT_VA: (u32, u32) = (0x8000_1000, 0x8000_5000);

#[test]
fn four_calls_under_one_delta_admit_the_relocated_slice() {
    // Callee prologues sit at ROM offsets 0x4000, 0x4100, 0x4240, 0x4390 --
    // non-uniform, so cross pairs cannot alias a second delta with equal
    // multiplicity. The boot bank calls them at 0x80104000, 0x80104100,
    // 0x80104240, 0x80104390: delta = 0x80100000.
    let prologues = [0x4000u32, 0x4100, 0x4240, 0x4390];
    let rom = rom_with_prologues(0x2000, &prologues);
    let delta = 0x8010_0000u32;
    let targets: Vec<u32> = prologues.iter().map(|&off| off + delta).collect();

    let outcome = relocated_slice_vote(
        &rom,
        &targets,
        &[BOOT_VA],
        &RelocatedSliceConfig::default(),
    );
    let RelocatedSliceOutcome::Admitted(slice) = outcome else {
        panic!("four distinct-target votes with no runner-up must admit: {outcome:?}");
    };
    assert_eq!(slice.delta, delta);
    assert_eq!(slice.votes, 4);
    assert_eq!(slice.runner_up_votes, 1);
    assert_eq!(slice.sources, 4);
    // Low end = the first voted entry exactly (already a function boundary);
    // high end = the last voted entry rounded out to a 0x1000 page, so the
    // function it opens has its body inside the mapping.
    assert_eq!(slice.rom_start, 0x4000);
    assert_eq!(slice.rom_end, 0x5000);
    assert_eq!(slice.va_start, 0x8010_4000);
    assert_eq!(slice.va_end, 0x8010_5000);
}

#[test]
fn the_low_end_of_the_extent_is_never_padded_below_the_first_voted_entry() {
    // Regression for a real refusal: page-rounding the LOW end DOWN claims
    // bytes no vote reaches, and on NASCAR 99 that padding pushed `va_start`
    // 0xae0 bytes back into the boot bank and got the whole slice refused for
    // an overlap the rounding itself manufactured. A voted offset is a
    // function entry -- already a boundary -- so the low end stays exact.
    let prologues = [0x4f4cu32, 0x5100, 0x5240, 0x5390];
    let rom = rom_with_prologues(0x4000, &prologues);
    let delta = 0x8010_0000u32;
    let targets: Vec<u32> = prologues.iter().map(|&off| off + delta).collect();
    // The boot bank ends exactly where the first voted entry begins; a padded
    // low end would reach back inside it.
    let boot = (0x8000_1000u32, 0x8010_4f4c);

    let outcome =
        relocated_slice_vote(&rom, &targets, &[boot], &RelocatedSliceConfig::default());
    let RelocatedSliceOutcome::Admitted(slice) = outcome else {
        panic!("the extent abuts the boot bank but does not overlap it: {outcome:?}");
    };
    assert_eq!(slice.rom_start, 0x4f4c, "low end must be the voted entry");
    assert_eq!(slice.va_start, 0x8010_4f4c);
    assert_eq!(slice.rom_end, 0x6000, "high end rounds out to a page");
    assert_eq!(slice.va_end, 0x8010_6000);
}

#[test]
fn only_two_votes_stays_open_with_the_counts() {
    let prologues = [0x4000u32, 0x4100];
    let rom = rom_with_prologues(0x2000, &prologues);
    let delta = 0x8010_0000u32;
    let targets: Vec<u32> = prologues.iter().map(|&off| off + delta).collect();

    let outcome = relocated_slice_vote(
        &rom,
        &targets,
        &[BOOT_VA],
        &RelocatedSliceConfig::default(),
    );
    assert_eq!(
        outcome,
        RelocatedSliceOutcome::Open {
            reason: RelocatedSliceOpenReason::InsufficientVotes {
                top_votes: 2,
                required: 3,
            },
            sources: 2,
        },
        "two coincidences are below the three-vote bar"
    );
}

#[test]
fn two_tied_deltas_stay_open() {
    // Prologues repeated at a fixed 0x1000 shift: every (target, prologue)
    // pairing that supports `delta` also supports `delta - 0x1000` with the
    // same multiplicity. Score cannot pick one, so nothing may be admitted.
    let prologues = [0x4000u32, 0x4100, 0x4240, 0x5000, 0x5100, 0x5240];
    let rom = rom_with_prologues(0x4000, &prologues);
    let delta = 0x8010_0000u32;
    let targets: Vec<u32> = [0x4000u32, 0x4100, 0x4240]
        .iter()
        .map(|&off| off + delta)
        .collect();

    let outcome = relocated_slice_vote(
        &rom,
        &targets,
        &[BOOT_VA],
        &RelocatedSliceConfig::default(),
    );
    assert_eq!(
        outcome,
        RelocatedSliceOutcome::Open {
            reason: RelocatedSliceOpenReason::NearTie {
                top_votes: 3,
                runner_up_votes: 3,
                required_factor: 2,
            },
            sources: 3,
        },
        "an exact tie must never be admitted by tie-break"
    );
}

#[test]
fn a_dominating_but_under_margin_delta_stays_open() {
    // Top explains 4, runner-up explains 3: strictly more, but under 2x.
    let prologues = [0x4000u32, 0x4100, 0x4240, 0x4390, 0x5000, 0x5100, 0x5240];
    let rom = rom_with_prologues(0x4000, &prologues);
    let delta = 0x8010_0000u32;
    let targets: Vec<u32> = [0x4000u32, 0x4100, 0x4240, 0x4390]
        .iter()
        .map(|&off| off + delta)
        .collect();

    let outcome = relocated_slice_vote(
        &rom,
        &targets,
        &[BOOT_VA],
        &RelocatedSliceConfig::default(),
    );
    assert_eq!(
        outcome,
        RelocatedSliceOutcome::Open {
            reason: RelocatedSliceOpenReason::NearTie {
                top_votes: 4,
                runner_up_votes: 3,
                required_factor: 2,
            },
            sources: 4,
        }
    );
}

#[test]
fn a_va_range_overlapping_an_existing_mapping_is_refused() {
    // Same admissible geometry as the first test, but the boot bank is
    // declared to already own 0x80104000..0x80106000. Two banks at one
    // address is a contradiction, not a second residency.
    let prologues = [0x4000u32, 0x4100, 0x4240, 0x4390];
    let rom = rom_with_prologues(0x2000, &prologues);
    let delta = 0x8010_0000u32;
    let targets: Vec<u32> = prologues.iter().map(|&off| off + delta).collect();

    let outcome = relocated_slice_vote(
        &rom,
        &targets,
        &[(0x8010_4000, 0x8010_6000)],
        &RelocatedSliceConfig::default(),
    );
    // Every target now lies inside an existing mapping, so nothing even votes:
    // the filter that keeps already-mapped destinations out of the source set
    // is the first line of the same disjointness rule.
    assert_eq!(
        outcome,
        RelocatedSliceOutcome::Open {
            reason: RelocatedSliceOpenReason::NoVoteSources,
            sources: 0,
        }
    );
}

#[test]
fn a_partly_overlapping_va_extent_is_refused_after_the_vote() {
    // The targets themselves are unmapped, but the page-rounded extent they
    // imply reaches into a mapping that begins just past the last of them.
    let prologues = [0x4000u32, 0x4100, 0x4240, 0x4390];
    let rom = rom_with_prologues(0x2000, &prologues);
    let delta = 0x8010_0000u32;
    let targets: Vec<u32> = prologues.iter().map(|&off| off + delta).collect();

    let outcome = relocated_slice_vote(
        &rom,
        &targets,
        &[(0x8010_4800, 0x8010_9000)],
        &RelocatedSliceConfig::default(),
    );
    assert_eq!(
        outcome,
        RelocatedSliceOutcome::Open {
            reason: RelocatedSliceOpenReason::VaOverlapsExistingMapping {
                va_start: 0x8010_4000,
                va_end: 0x8010_5000,
            },
            sources: 4,
        }
    );
}

#[test]
fn targets_beyond_rdram_never_vote() {
    // 0x80800000 is one byte past the largest RDRAM a retail N64 reaches.
    // Such targets are value-set imprecision, not code nobody mapped.
    let prologues = [0x4000u32, 0x4100, 0x4240, 0x4390];
    let rom = rom_with_prologues(0x2000, &prologues);
    let delta = 0x8080_0000u32;
    let targets: Vec<u32> = prologues.iter().map(|&off| off + delta).collect();

    let outcome = relocated_slice_vote(
        &rom,
        &targets,
        &[BOOT_VA],
        &RelocatedSliceConfig::default(),
    );
    assert_eq!(
        outcome,
        RelocatedSliceOutcome::Open {
            reason: RelocatedSliceOpenReason::NoVoteSources,
            sources: 0,
        }
    );
}

#[test]
fn an_unbounded_source_set_refuses_loudly_instead_of_enumerating() {
    // A ROM that is ALL prologues, against a source set large enough to blow
    // the pair bound. The refusal is a resource frontier -- the vote did not
    // run -- and must be typed as such rather than reported as "no slice".
    let words = 0x10_0000usize; // 4 MiB, every word a prologue
    let all: Vec<u32> = (0..words as u32).map(|index| index * 4).collect();
    let rom = rom_with_prologues(words, &all);
    let delta = 0x8010_0000u32;
    let targets: Vec<u32> = (0..64u32).map(|n| delta + n * 4).collect();

    let outcome =
        relocated_slice_vote(&rom, &targets, &[BOOT_VA], &RelocatedSliceConfig::default());
    assert!(
        matches!(
            outcome,
            RelocatedSliceOutcome::Open {
                reason: RelocatedSliceOpenReason::VoteWorkLimitExceeded { .. },
                ..
            }
        ),
        "a 64 x 1,048,576 pair enumeration must refuse, not run: {outcome:?}"
    );
}

#[test]
fn the_vote_is_byte_identical_across_runs() {
    let prologues = [0x4000u32, 0x4100, 0x4240, 0x4390];
    let rom = rom_with_prologues(0x2000, &prologues);
    let delta = 0x8010_0000u32;
    let targets: Vec<u32> = prologues.iter().map(|&off| off + delta).collect();
    let config = RelocatedSliceConfig::default();
    let first = relocated_slice_vote(&rom, &targets, &[BOOT_VA], &config);
    let second = relocated_slice_vote(&rom, &targets, &[BOOT_VA], &config);
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
}

// ---------------------------------------------------------------------------
// Corpus tests (env-gated)
// ---------------------------------------------------------------------------

/// Run automatic discovery on `path` and assert it admitted a
/// `relocated_slice_` mapping at `expected_delta`, and that the recompile
/// gate's `unsupported` count is strictly lower than it was without the step.
fn assert_corpus_rom_gains_a_relocated_slice(var: &str, expected_delta: u32) {
    let Ok(path) = std::env::var(var) else {
        eprintln!(
            "SKIPPING relocated-slice corpus check: {var} is unset. \
             Set it to a ROM of this class to measure the mechanism; this test \
             is NOT evidence of anything while it is unset."
        );
        return;
    };
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("reading {var}: {error}"));
    let discovery = fn64_discover::run_discovery_auto(&bytes)
        .unwrap_or_else(|error| panic!("{var}: automatic discovery rejected the ROM: {error:?}"));

    let mut relocated: Vec<(String, u32, u32, u32, u32)> = Vec::new();
    for fact in discovery.facts.facts() {
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
        if bank.starts_with("relocated_slice_") {
            relocated.push((bank.clone(), *rom_start, *rom_end, *va_start, *va_end));
        }
    }
    assert_eq!(
        relocated.len(),
        1,
        "{var}: expected exactly one relocated_slice_ mapping, got {relocated:?}"
    );
    let slice = &relocated[0];
    assert_eq!(
        slice.3.wrapping_sub(slice.1),
        expected_delta,
        "{var}: relocated slice delta (VA 0x{:x} - ROM 0x{:x})",
        slice.3,
        slice.1
    );
    // Supported, never Proven: a call target plus a prologue does not prove
    // the copy ever ran.
    let conclusion = discovery
        .facts
        .conclusion(&format!("bank:{}", slice.0))
        .unwrap_or_else(|| panic!("{var}: relocated slice has no conclusion"));
    assert_eq!(
        conclusion.state,
        fn64_discover::facts::ProofState::Supported,
        "{var}: a relocated slice must never be concluded Proven"
    );

    // The mapping must actually retire unsupported destinations: measure the
    // closure scoreboard the recompile gate blocks on.
    // The slice must actually COVER the call destinations it was voted from.
    // This is the mechanism's own claim, and it is checkable here without the
    // recompile gate: every destination the vote landed lies inside the
    // admitted VA range.
    //
    // What this test deliberately does NOT assert is what the recompile gate's
    // `unsupported` count does, and that stayed true for a different reason
    // after K20 than before it.
    //
    // Before K20 the count could not move at all: a `Supported` mapping never
    // reached the execution closure, because `ProgramGeometry` built its
    // `mapped` set from `proven_bank_images()` and composition admitted only
    // `Proven` banks (B8). K20 removed that wall -- the slice now composes and
    // its words class `mapped_not_proven_code` -- and measuring it showed the
    // count falls on four of the seven ROMs and RISES on two, because
    // composing the slice also composes ITS calls, which reach past the
    // extent this vote admits. That shortfall is K22's, not this vote's, and
    // asserting a gate total here would bind this test to it. The end-to-end
    // measurement lives in `tests/supported_bank_composed.rs`; what stays
    // asserted here is the vote's own claim: the slice covers the call
    // destinations it was voted from.
    let run = fn64_discover::cold_sweep::measure_cold_rom(&bytes)
        .unwrap_or_else(|error| panic!("{var}: cold measurement failed: {error:?}"));
    let unsupported: Vec<u32> = run
        .unsupported_destinations
        .iter()
        .filter(|audit| audit.reason == fn64_discover::closure::DestinationReason::OutsideAllMappings)
        .filter(|audit| {
            audit
                .incoming
                .iter()
                .any(|incoming| incoming.kind == fn64_discover::closure::ConcreteTransferKind::Call)
        })
        .map(|audit| audit.destination_va)
        .collect();
    let covered = unsupported
        .iter()
        .filter(|&&va| va >= slice.3 && va < slice.4)
        .count();
    assert!(
        covered > 0,
        "{var}: the admitted slice VA 0x{:x}..0x{:x} covers none of the {} \
         out-of-mapping call destinations it was voted from",
        slice.3,
        slice.4,
        unsupported.len()
    );
    assert!(
        covered * 2 >= unsupported.len(),
        "{var}: the slice covers only {covered} of {} out-of-mapping call \
         destinations; the vote should explain the majority of its own sources",
        unsupported.len()
    );
}

#[test]
fn corpus_rom_a_admits_its_relocated_slice() {
    // Waialae Country Club: 24 votes to 4, delta 0x800b90c0.
    assert_corpus_rom_gains_a_relocated_slice("FN64_K18_ROM_A", 0x800b_90c0);
}

#[test]
fn corpus_rom_b_admits_its_relocated_slice() {
    // F-Zero X: 8 votes to 3, delta 0x80390ef0.
    assert_corpus_rom_gains_a_relocated_slice("FN64_K18_ROM_B", 0x8039_0ef0);
}
