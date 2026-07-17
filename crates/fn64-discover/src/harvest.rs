//! Phase 3 deterministic candidate harvesting.
//!
//! Providers run concurrently over immutable discovered load images and emit
//! immutable [`Fact::FunctionEntryClaim`] values. The merge is deliberately
//! single-threaded, sorted, and bank-qualified: scheduling cannot affect fact
//! order, serialized output, or proof state. No provider reads an answer key.

use crate::facts::{
    function_entry_subject, table_entry_subject, BankAddr, CandidateDetector, Fact, FactDb,
    FunctionEntryEvidence, MonotonicityViolation, ProloguePattern, ProofState,
};
use crate::resolve::resolve_linear_jalr_sites;
use crate::rom::NormalizedRom;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ProviderClaim {
    target: BankAddr,
    detector: CandidateDetector,
    evidence: FunctionEntryEvidence,
    proposed_state: ProofState,
}

#[derive(Debug, Clone)]
struct LoadImage {
    bank: String,
    rom_start: u32,
    va_start: u32,
    va_end: u32,
    bytes: Vec<u8>,
}

/// One merged function-entry conclusion returned to callers after facts are
/// committed. Evidence indices point into the same [`FactDb`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarvestedEntry {
    pub target: BankAddr,
    pub state: ProofState,
    pub detectors: BTreeSet<CandidateDetector>,
    pub evidence_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarvestReport {
    pub entries: Vec<HarvestedEntry>,
    pub claim_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarvestError {
    MappingOutsideRom {
        bank: String,
        rom_start: u32,
        rom_end: u32,
        rom_len: usize,
    },
    MappingLengthMismatch {
        bank: String,
        rom_len: u32,
        va_len: u32,
    },
    MappingBytesUnavailable {
        bank: String,
        detail: String,
    },
    Monotonicity(MonotonicityViolation),
}

impl std::fmt::Display for HarvestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HarvestError::MappingOutsideRom {
                bank,
                rom_start,
                rom_end,
                rom_len,
            } => write!(
                f,
                "Phase 3 mapping for bank {bank:?} is outside normalized ROM: [0x{rom_start:x},0x{rom_end:x}) vs len 0x{rom_len:x}"
            ),
            HarvestError::MappingLengthMismatch {
                bank,
                rom_len,
                va_len,
            } => write!(
                f,
                "Phase 3 mapping for bank {bank:?} has a VRAM range shorter than its ROM image: 0x{va_len:x} vs 0x{rom_len:x}"
            ),
            HarvestError::MappingBytesUnavailable { bank, detail } => write!(
                f,
                "Phase 3 could not materialize bytes for bank {bank:?}: {detail}"
            ),
            HarvestError::Monotonicity(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for HarvestError {}

impl From<MonotonicityViolation> for HarvestError {
    fn from(value: MonotonicityViolation) -> Self {
        Self::Monotonicity(value)
    }
}

/// Run all Phase 3 providers in parallel and deterministically merge their
/// claims into the existing fact/proof-state model.
pub fn harvest_discovered_candidates(
    rom: &NormalizedRom,
    db: &mut FactDb,
) -> Result<HarvestReport, HarvestError> {
    let images = load_images(rom, db)?;
    let table_entries: Vec<(BankAddr, u32, BankAddr, ProofState)> = db
        .facts()
        .iter()
        .filter_map(|fact| {
            let Fact::TableEntry {
                table,
                index,
                target,
            } = fact
            else {
                return None;
            };
            let state = db
                .conclusion(&table_entry_subject(table, *index))
                .map_or(ProofState::Open, |conclusion| conclusion.state);
            Some((table.clone(), *index, target.clone(), state))
        })
        .collect();

    let (jal_claims, prologue_claims, table_claims) = std::thread::scope(|scope| {
        let jal = scope.spawn(|| detect_jal_targets(&images));
        let prologue = scope.spawn(|| detect_prologues(&images));
        let table = scope.spawn(|| detect_table_entries(&images, &table_entries));
        (
            jal.join().expect("jal-target candidate provider panicked"),
            prologue
                .join()
                .expect("prologue candidate provider panicked"),
            table
                .join()
                .expect("table-entry candidate provider panicked"),
        )
    });

    let mut claims = jal_claims;
    claims.extend(prologue_claims);
    claims.extend(table_claims);
    claims.sort();
    claims.dedup();
    merge_claims(db, &claims)
}

fn load_images(rom: &NormalizedRom, db: &FactDb) -> Result<Vec<LoadImage>, HarvestError> {
    let mut images = Vec::new();
    for mapping in db.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_space,
            rom_start,
            rom_end,
            va_start,
            va_end,
        } = mapping
        else {
            unreachable!("proven_rom_mappings returned a non-mapping fact")
        };
        let rom_len = rom_end.saturating_sub(*rom_start);
        let va_len = va_end.saturating_sub(*va_start);
        if va_len < rom_len {
            return Err(HarvestError::MappingLengthMismatch {
                bank: bank.clone(),
                rom_len,
                va_len,
            });
        }
        let bytes = crate::banks::materialize_rom_range(rom, db, *rom_space, *rom_start, *rom_end)
            .map_err(|detail| HarvestError::MappingBytesUnavailable {
                bank: bank.clone(),
                detail,
            })?
            .bytes;
        images.push(LoadImage {
            bank: bank.clone(),
            rom_start: *rom_start,
            va_start: *va_start,
            va_end: *va_end,
            bytes,
        });
    }
    images.sort_by(|a, b| {
        (&a.bank, a.rom_start, a.va_start).cmp(&(&b.bank, b.rom_start, b.va_start))
    });
    Ok(images)
}

fn detect_jal_targets(images: &[LoadImage]) -> Vec<ProviderClaim> {
    let mut claims = Vec::new();
    for image in images {
        for (index, bytes) in image.bytes.chunks_exact(4).enumerate() {
            let word = u32::from_be_bytes(bytes.try_into().unwrap());
            if (word >> 26) != 0x03 {
                continue;
            }
            let site_pc = image.va_start.wrapping_add((index * 4) as u32);
            let target = (site_pc.wrapping_add(4) & 0xf000_0000) | ((word & 0x03ff_ffff) << 2);
            for target_image in target_images(images, image, target) {
                claims.push(ProviderClaim {
                    target: BankAddr::new(&target_image.bank, target),
                    detector: CandidateDetector::JalTarget,
                    evidence: FunctionEntryEvidence::DirectJal {
                        call_site: BankAddr::new(&image.bank, site_pc),
                    },
                    proposed_state: ProofState::Candidate,
                });
            }
        }

        for resolved in resolve_linear_jalr_sites(&image.bytes, image.va_start) {
            for target_image in target_images(images, image, resolved.target) {
                claims.push(ProviderClaim {
                    target: BankAddr::new(&target_image.bank, resolved.target),
                    detector: CandidateDetector::JalTarget,
                    evidence: FunctionEntryEvidence::ResolvedJalr {
                        call_site: BankAddr::new(&image.bank, resolved.site_pc),
                        construction_start: BankAddr::new(&image.bank, resolved.construction_start),
                    },
                    proposed_state: ProofState::Candidate,
                });
            }
        }
    }
    claims
}

/// A call whose target lies inside its source load image stays bank-local.
/// An out-of-image target prefers the always-resident boot image before any
/// other match. Only when neither is possible do all matching images remain
/// candidates; otherwise same-VA mutually-exclusive overlays would create
/// fabricated cross-bank claims for every bank sharing the address.
fn target_images<'a>(
    images: &'a [LoadImage],
    source: &'a LoadImage,
    target: u32,
) -> Vec<&'a LoadImage> {
    if target >= source.va_start && target < source.va_end {
        return vec![source];
    }
    let resident: Vec<_> = images
        .iter()
        .filter(|candidate| {
            candidate.bank == crate::banks::BOOT_BANK
                && target >= candidate.va_start
                && target < candidate.va_end
        })
        .collect();
    if !resident.is_empty() {
        return resident;
    }
    images
        .iter()
        .filter(|candidate| target >= candidate.va_start && target < candidate.va_end)
        .collect()
}

fn detect_prologues(images: &[LoadImage]) -> Vec<ProviderClaim> {
    let mut claims = Vec::new();
    for image in images {
        let words: Vec<u32> = image
            .bytes
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()))
            .collect();
        for (index, &word) in words.iter().enumerate() {
            let Some(frame_size) = stack_allocation(word) else {
                continue;
            };
            let entry_pc = image.va_start.wrapping_add((index * 4) as u32);

            if let Some(save_index) = find_ra_save(&words, index, frame_size) {
                claims.push(ProviderClaim {
                    target: BankAddr::new(&image.bank, entry_pc),
                    detector: CandidateDetector::ProloguePattern,
                    evidence: FunctionEntryEvidence::Prologue {
                        stack_adjust: BankAddr::new(&image.bank, entry_pc),
                        frame_size,
                        pattern: ProloguePattern::SavesReturnAddress,
                        corroborating_site: BankAddr::new(
                            &image.bank,
                            image.va_start.wrapping_add((save_index * 4) as u32),
                        ),
                    },
                    proposed_state: ProofState::Candidate,
                });
                continue;
            }

            if let Some(return_index) = find_leaf_return(&words, index, frame_size) {
                claims.push(ProviderClaim {
                    target: BankAddr::new(&image.bank, entry_pc),
                    detector: CandidateDetector::ProloguePattern,
                    evidence: FunctionEntryEvidence::Prologue {
                        stack_adjust: BankAddr::new(&image.bank, entry_pc),
                        frame_size,
                        pattern: ProloguePattern::LeafWithMatchedRestore,
                        corroborating_site: BankAddr::new(
                            &image.bank,
                            image.va_start.wrapping_add((return_index * 4) as u32),
                        ),
                    },
                    proposed_state: ProofState::Candidate,
                });
            }
        }
    }
    claims
}

fn detect_table_entries(
    images: &[LoadImage],
    entries: &[(BankAddr, u32, BankAddr, ProofState)],
) -> Vec<ProviderClaim> {
    entries
        .iter()
        .map(|(table, index, target, state)| {
            let target_is_in_bank = images.iter().any(|image| {
                image.bank == target.bank
                    && target.pc >= image.va_start
                    && target.pc < image.va_end
                    && (target.pc - image.va_start).is_multiple_of(4)
            });
            let proposed_state = if target_is_in_bank {
                *state
            } else if matches!(
                state,
                ProofState::Candidate | ProofState::Supported | ProofState::Proven
            ) {
                ProofState::Rejected
            } else {
                *state
            };
            ProviderClaim {
                target: target.clone(),
                detector: CandidateDetector::TableDerived,
                evidence: FunctionEntryEvidence::TableEntry {
                    table: table.clone(),
                    index: *index,
                },
                proposed_state,
            }
        })
        .collect()
}

fn stack_allocation(word: u32) -> Option<u32> {
    let opcode = word >> 26;
    let rs = (word >> 21) & 0x1f;
    let rt = (word >> 16) & 0x1f;
    let immediate = (word & 0xffff) as i16;
    if !matches!(opcode, 0x09 | 0x19) || rs != 29 || rt != 29 || immediate >= 0 {
        return None;
    }
    let frame_size = (-(immediate as i32)) as u32;
    (frame_size != 0 && frame_size.is_multiple_of(8)).then_some(frame_size)
}

fn find_ra_save(words: &[u32], prologue_index: usize, frame_size: u32) -> Option<usize> {
    let end = words.len().min(prologue_index.saturating_add(9));
    for (index, &word) in words.iter().enumerate().take(end).skip(prologue_index + 1) {
        if is_sw_ra_in_frame(word, frame_size) {
            return Some(index);
        }
        if is_control_transfer(word) {
            break;
        }
    }
    None
}

fn find_leaf_return(words: &[u32], prologue_index: usize, frame_size: u32) -> Option<usize> {
    let end = words.len().min(prologue_index.saturating_add(97));
    let restore = |word: u32| {
        matches!(word >> 26, 0x09 | 0x19)
            && ((word >> 21) & 0x1f) == 29
            && ((word >> 16) & 0x1f) == 29
            && (word & 0xffff) as i16 as i32 == frame_size as i32
    };

    for index in prologue_index + 1..end {
        let word = words[index];
        if is_call(word) || is_sw_ra_in_frame(word, frame_size) {
            return None;
        }
        if word == 0x03e0_0008 {
            let before_restores = index
                .checked_sub(1)
                .is_some_and(|before| restore(words[before]));
            let delay_restores = words.get(index + 1).is_some_and(|&delay| restore(delay));
            return (before_restores || delay_restores).then_some(index);
        }
        if is_unconditional_transfer_or_trap(word) {
            return None;
        }
    }
    None
}

fn is_sw_ra_in_frame(word: u32, frame_size: u32) -> bool {
    let offset = (word & 0xffff) as i16 as i32;
    (word >> 26) == 0x2b
        && ((word >> 21) & 0x1f) == 29
        && ((word >> 16) & 0x1f) == 31
        && offset >= 0
        && (offset as u32) < frame_size
}

fn is_call(word: u32) -> bool {
    let opcode = word >> 26;
    opcode == 0x03
        || (opcode == 0 && (word & 0x3f) == 0x09)
        || (opcode == 0x01 && matches!((word >> 16) & 0x1f, 0x10..=0x13))
}

fn is_control_transfer(word: u32) -> bool {
    let opcode = word >> 26;
    matches!(opcode, 0x01..=0x07 | 0x14..=0x17)
        || (opcode == 0 && matches!(word & 0x3f, 0x08 | 0x09 | 0x0c | 0x0d))
        || (opcode == 0x11 && ((word >> 21) & 0x1f) == 0x08)
}

fn is_unconditional_transfer_or_trap(word: u32) -> bool {
    let opcode = word >> 26;
    matches!(opcode, 0x02 | 0x03)
        || (opcode == 0 && matches!(word & 0x3f, 0x08 | 0x09 | 0x0c | 0x0d))
}

fn merge_claims(db: &mut FactDb, claims: &[ProviderClaim]) -> Result<HarvestReport, HarvestError> {
    for claim in claims {
        db.insert(Fact::FunctionEntryClaim {
            target: claim.target.clone(),
            detector: claim.detector,
            evidence: claim.evidence.clone(),
            proposed_state: claim.proposed_state,
        });
    }

    let mut grouped: BTreeMap<BankAddr, Vec<(usize, CandidateDetector, ProofState)>> =
        BTreeMap::new();
    for (index, fact) in db.facts().iter().enumerate() {
        let Fact::FunctionEntryClaim {
            target,
            detector,
            proposed_state,
            ..
        } = fact
        else {
            continue;
        };
        grouped
            .entry(target.clone())
            .or_default()
            .push((index, *detector, *proposed_state));
    }

    let mut entries = Vec::with_capacity(grouped.len());
    for (target, evidence) in grouped {
        let state = match db.conclusion(&function_entry_subject(&target)) {
            Some(existing) => merge_existing_state(existing.state, merged_state(&evidence)),
            None => merged_state(&evidence),
        };
        let evidence_indices: Vec<usize> = evidence.iter().map(|(index, _, _)| *index).collect();
        let detectors: BTreeSet<CandidateDetector> =
            evidence.iter().map(|(_, detector, _)| *detector).collect();
        db.conclude(
            function_entry_subject(&target),
            state,
            evidence_indices.clone(),
            "phase3_deterministic_candidate_merge",
        )?;
        entries.push(HarvestedEntry {
            target,
            state,
            detectors,
            evidence_indices,
        });
    }

    Ok(HarvestReport {
        entries,
        claim_count: claims.len(),
    })
}

fn merged_state(evidence: &[(usize, CandidateDetector, ProofState)]) -> ProofState {
    let has_positive = evidence.iter().any(|(_, _, state)| {
        matches!(
            state,
            ProofState::Candidate | ProofState::Supported | ProofState::Proven
        )
    });
    let has_negative = evidence
        .iter()
        .any(|(_, _, state)| *state == ProofState::Rejected);
    if evidence
        .iter()
        .any(|(_, _, state)| *state == ProofState::Conflict)
        || (has_positive && has_negative)
    {
        return ProofState::Conflict;
    }
    if evidence
        .iter()
        .any(|(_, _, state)| *state == ProofState::Proven)
    {
        return ProofState::Proven;
    }
    if has_positive {
        let positive_detectors: BTreeSet<CandidateDetector> = evidence
            .iter()
            .filter(|(_, _, state)| {
                matches!(
                    state,
                    ProofState::Candidate | ProofState::Supported | ProofState::Proven
                )
            })
            .map(|(_, detector, _)| *detector)
            .collect();
        if positive_detectors.len() > 1
            || evidence
                .iter()
                .any(|(_, _, state)| *state == ProofState::Supported)
        {
            return ProofState::Supported;
        }
        return ProofState::Candidate;
    }
    if has_negative {
        return ProofState::Rejected;
    }
    ProofState::Open
}

fn merge_existing_state(existing: ProofState, incoming: ProofState) -> ProofState {
    let positive = |state| {
        matches!(
            state,
            ProofState::Candidate | ProofState::Supported | ProofState::Proven
        )
    };
    if existing == ProofState::Conflict || incoming == ProofState::Conflict {
        return ProofState::Conflict;
    }
    if (existing == ProofState::Rejected && positive(incoming))
        || (incoming == ProofState::Rejected && positive(existing))
    {
        return ProofState::Conflict;
    }
    if existing == ProofState::Proven || incoming == ProofState::Proven {
        return ProofState::Proven;
    }
    if existing == ProofState::Supported || incoming == ProofState::Supported {
        return ProofState::Supported;
    }
    if existing == ProofState::Candidate || incoming == ProofState::Candidate {
        return ProofState::Candidate;
    }
    if existing == ProofState::Rejected || incoming == ProofState::Rejected {
        return ProofState::Rejected;
    }
    ProofState::Open
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::Fact;
    use crate::rom::normalize;

    const NOP: u32 = 0;
    const JR_RA: u32 = 0x03e0_0008;

    fn rom_with_words(words: &[u32]) -> NormalizedRom {
        let mut bytes = vec![0u8; 0x1000 + words.len() * 4];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        bytes[0x20..0x24].copy_from_slice(b"TEST");
        bytes[0x3b..0x3f].copy_from_slice(b"CTSE");
        for (index, word) in words.iter().enumerate() {
            let offset = 0x1000 + index * 4;
            bytes[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
        }
        normalize(&bytes).unwrap()
    }

    fn mapped_db(rom: &NormalizedRom) -> FactDb {
        let mut db = FactDb::new();
        let mapping = db.insert(Fact::RomMapping {
            bank: "boot".into(),
            rom_space: crate::facts::RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: rom.len() as u32,
            va_start: 0x8000_0400,
            va_end: 0x8000_0400 + (rom.len() as u32 - 0x1000),
        });
        db.conclude("bank:boot", ProofState::Proven, vec![mapping], "test")
            .unwrap();
        db
    }

    #[test]
    fn jal_and_resolved_jalr_targets_keep_call_site_evidence() {
        let direct_target = 0x8000_0440u32;
        let jal = 0x0c00_0000 | ((direct_target >> 2) & 0x03ff_ffff);
        let lui_t9 = 0x3c19_8000u32;
        let addiu_t9 = 0x2739_0448u32;
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut words = vec![jal, NOP, lui_t9, addiu_t9, jalr_t9, NOP];
        words.resize(20, NOP);
        let rom = rom_with_words(&words);
        let mut db = mapped_db(&rom);
        let report = harvest_discovered_candidates(&rom, &mut db).unwrap();

        for target in [direct_target, 0x8000_0448] {
            assert!(report.entries.iter().any(|entry| {
                entry.target == BankAddr::new("boot", target)
                    && entry.detectors.contains(&CandidateDetector::JalTarget)
            }));
        }
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            Fact::FunctionEntryClaim {
                evidence: FunctionEntryEvidence::DirectJal { call_site },
                ..
            } if call_site.pc == 0x8000_0400
        )));
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            Fact::FunctionEntryClaim {
                evidence: FunctionEntryEvidence::ResolvedJalr { call_site, .. },
                ..
            } if call_site.pc == 0x8000_0410
        )));
    }

    #[test]
    fn classic_and_leaf_prologues_are_distinct_weaker_candidates() {
        let addiu_sp_m20 = 0x27bd_ffe0u32;
        let sw_ra_1c_sp = 0xafbf_001cu32;
        let addiu_sp_p20 = 0x27bd_0020u32;
        let words = [
            addiu_sp_m20,
            sw_ra_1c_sp,
            JR_RA,
            NOP,
            addiu_sp_m20,
            NOP,
            JR_RA,
            addiu_sp_p20,
        ];
        let rom = rom_with_words(&words);
        let mut db = mapped_db(&rom);
        let report = harvest_discovered_candidates(&rom, &mut db).unwrap();

        let prologues: Vec<_> = report
            .entries
            .iter()
            .filter(|entry| {
                entry
                    .detectors
                    .contains(&CandidateDetector::ProloguePattern)
            })
            .collect();
        assert_eq!(prologues.len(), 2);
        assert!(prologues
            .iter()
            .all(|entry| entry.state == ProofState::Candidate));
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            Fact::FunctionEntryClaim {
                evidence: FunctionEntryEvidence::Prologue {
                    pattern: ProloguePattern::LeafWithMatchedRestore,
                    ..
                },
                ..
            }
        )));
    }

    #[test]
    fn proven_table_vector_entry_becomes_an_authoritative_candidate_root() {
        let mut words = [NOP; 8];
        words[4] = 0x8000_041c;
        let rom = rom_with_words(&words);
        let mut db = mapped_db(&rom);
        db.record_table_entry(
            BankAddr::new("boot", 0x8000_0410),
            0,
            BankAddr::new("boot", words[4]),
            ProofState::Proven,
            "synthetic_proven_vector",
        )
        .unwrap();
        let report = harvest_discovered_candidates(&rom, &mut db).unwrap();
        let entry = report
            .entries
            .iter()
            .find(|entry| entry.target.pc == 0x8000_041c)
            .unwrap();
        assert_eq!(entry.state, ProofState::Proven);
        assert_eq!(db.proven_function_entries("boot"), vec![0x8000_041c]);
    }

    #[test]
    fn incompatible_provider_states_merge_to_conflict() {
        let target = BankAddr::new("boot", 0x8000_0400);
        let evidence = vec![
            (0, CandidateDetector::JalTarget, ProofState::Candidate),
            (1, CandidateDetector::TableDerived, ProofState::Rejected),
        ];
        assert_eq!(merged_state(&evidence), ProofState::Conflict);

        let positive = vec![
            (0, CandidateDetector::JalTarget, ProofState::Candidate),
            (1, CandidateDetector::ProloguePattern, ProofState::Candidate),
        ];
        assert_eq!(merged_state(&positive), ProofState::Supported);
        assert_eq!(function_entry_subject(&target), "fn:boot:0x80000400");
        assert_eq!(
            merge_existing_state(ProofState::Proven, ProofState::Candidate),
            ProofState::Proven
        );
        assert_eq!(
            merge_existing_state(ProofState::Proven, ProofState::Rejected),
            ProofState::Conflict
        );
    }

    #[test]
    fn parallel_provider_merge_is_byte_identical_across_runs() {
        let target = 0x8000_0420u32;
        let jal = 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
        let words = [jal, NOP, 0x27bd_ffe0, 0xafbf_001c, JR_RA, NOP, NOP, NOP];
        let rom = rom_with_words(&words);
        let mut outputs = BTreeSet::new();
        for _ in 0..10 {
            let mut db = mapped_db(&rom);
            harvest_discovered_candidates(&rom, &mut db).unwrap();
            outputs.insert(serde_json::to_string(&db).unwrap());
        }
        assert_eq!(outputs.len(), 1);
    }
}
