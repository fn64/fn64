//! Grading-only precision/recall cross-check for Phase 3 candidates.
//!
//! The answer key is parsed only by this module and is never reachable from
//! any detector. Candidate identity is translated through discovery's own
//! `RomMapping` facts into `(ROM offset, runtime VA)` before comparison, so
//! overlapping overlay VAs cannot earn false matches by address alone.

use crate::facts::{function_entry_subject, CandidateDetector, Fact, FactDb, ProofState};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysicalEntry {
    pub rom: u32,
    pub vram: u32,
}

#[derive(Debug, Clone)]
pub struct CandidateAnswerKey {
    pub entries: BTreeSet<PhysicalEntry>,
    pub function_count: usize,
    pub section_count: usize,
    multiplicity: BTreeMap<PhysicalEntry, usize>,
}

#[derive(Debug, Deserialize)]
struct SymbolsDoc {
    #[serde(default)]
    section: Vec<SectionDoc>,
}

#[derive(Debug, Deserialize)]
struct SectionDoc {
    name: String,
    rom: u32,
    vram: u32,
    size: u32,
    #[serde(default)]
    functions: Vec<FunctionDoc>,
}

#[derive(Debug, Deserialize)]
struct FunctionDoc {
    name: String,
    vram: u32,
    size: u32,
}

/// Parse the zeldaret-derived `dump.toml` used by the existing OoT recompile
/// loader. Malformed extents fail loudly; silently skipping a key row would
/// inflate recall.
pub fn parse_symbol_dump(text: &str) -> Result<CandidateAnswerKey, String> {
    let doc: SymbolsDoc = toml::from_str(text).map_err(|error| error.to_string())?;
    let section_count = doc.section.len();
    let mut entries = BTreeSet::new();
    let mut multiplicity = BTreeMap::new();
    let mut function_count = 0usize;

    for section in doc.section {
        for function in section.functions {
            function_count += 1;
            let offset = function.vram.checked_sub(section.vram).ok_or_else(|| {
                format!(
                    "function {:?} at 0x{:08x} precedes section {:?} at 0x{:08x}",
                    function.name, function.vram, section.name, section.vram
                )
            })?;
            let function_end = offset
                .checked_add(function.size)
                .ok_or_else(|| format!("function {:?} extent overflows u32", function.name))?;
            if function_end > section.size {
                return Err(format!(
                    "function {:?} extent [0x{:x},0x{:x}) lies outside section {:?} size 0x{:x}",
                    function.name, offset, function_end, section.name, section.size
                ));
            }
            let rom = section
                .rom
                .checked_add(offset)
                .ok_or_else(|| format!("function {:?} ROM address overflows u32", function.name))?;
            let entry = PhysicalEntry {
                rom,
                vram: function.vram,
            };
            entries.insert(entry);
            *multiplicity.entry(entry).or_insert(0) += 1;
        }
    }

    Ok(CandidateAnswerKey {
        entries,
        function_count,
        section_count,
        multiplicity,
    })
}

#[derive(Debug, Clone)]
pub struct PrecisionRecall {
    pub candidates: usize,
    pub true_positives: usize,
    pub false_positives: usize,
    /// Answer-key function rows recalled. This may exceed
    /// `true_positives` when aliases share one physical entry.
    pub recalled_functions: usize,
    pub false_negatives: usize,
}

impl PrecisionRecall {
    pub fn precision(&self) -> f64 {
        if self.candidates == 0 {
            0.0
        } else {
            self.true_positives as f64 / self.candidates as f64
        }
    }

    pub fn recall(&self) -> f64 {
        let total = self.recalled_functions + self.false_negatives;
        if total == 0 {
            0.0
        } else {
            self.recalled_functions as f64 / total as f64
        }
    }
}

#[derive(Debug, Clone)]
pub struct DetectorGrade {
    pub detector: CandidateDetector,
    pub metrics: PrecisionRecall,
    /// Positive claims that could not be translated through exactly one
    /// discovered bank mapping. They count neither as matches nor candidates;
    /// the count is explicit so a broken mapping cannot hide in the grade.
    pub ungradable: usize,
}

#[derive(Debug, Clone)]
pub struct CandidateGradeReport {
    pub answer_key_total: usize,
    pub per_detector: Vec<DetectorGrade>,
    pub combined: PrecisionRecall,
    pub combined_ungradable: usize,
}

#[derive(Debug, Clone, Copy)]
struct Mapping {
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

/// Grade every positive provider independently and the merged non-conflict
/// candidate set as a union. Answer-key rows never influence the candidates.
pub fn grade_candidates(db: &FactDb, key: &CandidateAnswerKey) -> CandidateGradeReport {
    let mappings = mappings_by_bank(db);
    let mut per_detector_candidates: BTreeMap<CandidateDetector, BTreeSet<PhysicalEntry>> =
        BTreeMap::from([
            (CandidateDetector::JalTarget, BTreeSet::new()),
            (CandidateDetector::ProloguePattern, BTreeSet::new()),
            (CandidateDetector::TableDerived, BTreeSet::new()),
        ]);
    let mut per_detector_ungradable: BTreeMap<CandidateDetector, BTreeSet<(String, u32)>> =
        BTreeMap::new();
    let mut combined_targets: BTreeSet<(String, u32)> = BTreeSet::new();

    for fact in db.facts() {
        let Fact::FunctionEntryClaim {
            target,
            detector,
            proposed_state,
            ..
        } = fact
        else {
            continue;
        };
        if !is_positive(*proposed_state) {
            continue;
        }
        match translate(target.bank.as_str(), target.pc, &mappings) {
            Some(entry) => {
                per_detector_candidates
                    .entry(*detector)
                    .or_default()
                    .insert(entry);
            }
            None => {
                per_detector_ungradable
                    .entry(*detector)
                    .or_default()
                    .insert((target.bank.clone(), target.pc));
            }
        }
        if db
            .conclusion(&function_entry_subject(target))
            .is_some_and(|conclusion| is_positive(conclusion.state))
        {
            combined_targets.insert((target.bank.clone(), target.pc));
        }
    }

    let per_detector = per_detector_candidates
        .into_iter()
        .map(|(detector, candidates)| DetectorGrade {
            detector,
            metrics: metrics(&candidates, key),
            ungradable: per_detector_ungradable
                .get(&detector)
                .map_or(0, BTreeSet::len),
        })
        .collect();

    let mut combined_candidates = BTreeSet::new();
    let mut combined_ungradable = 0;
    for (bank, pc) in combined_targets {
        match translate(&bank, pc, &mappings) {
            Some(entry) => {
                combined_candidates.insert(entry);
            }
            None => combined_ungradable += 1,
        }
    }

    CandidateGradeReport {
        answer_key_total: key.function_count,
        per_detector,
        combined: metrics(&combined_candidates, key),
        combined_ungradable,
    }
}

fn is_positive(state: ProofState) -> bool {
    matches!(
        state,
        ProofState::Candidate | ProofState::Supported | ProofState::Proven
    )
}

fn mappings_by_bank(db: &FactDb) -> BTreeMap<String, Vec<Mapping>> {
    let mut out: BTreeMap<String, Vec<Mapping>> = BTreeMap::new();
    for fact in db.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_start,
            rom_end,
            va_start,
            va_end,
            ..
        } = fact
        else {
            unreachable!("proven_rom_mappings returned non-mapping")
        };
        out.entry(bank.clone()).or_default().push(Mapping {
            rom_start: *rom_start,
            rom_end: *rom_end,
            va_start: *va_start,
            va_end: *va_end,
        });
    }
    out
}

fn translate(
    bank: &str,
    pc: u32,
    mappings: &BTreeMap<String, Vec<Mapping>>,
) -> Option<PhysicalEntry> {
    let matches: BTreeSet<PhysicalEntry> = mappings
        .get(bank)?
        .iter()
        .filter(|mapping| pc >= mapping.va_start && pc < mapping.va_end)
        .filter_map(|mapping| {
            let offset = pc.checked_sub(mapping.va_start)?;
            let rom = mapping.rom_start.checked_add(offset)?;
            (rom < mapping.rom_end).then_some(PhysicalEntry { rom, vram: pc })
        })
        .collect();
    (matches.len() == 1).then(|| *matches.iter().next().unwrap())
}

fn metrics(candidates: &BTreeSet<PhysicalEntry>, key: &CandidateAnswerKey) -> PrecisionRecall {
    let true_positives = candidates.intersection(&key.entries).count();
    let recalled_functions = candidates
        .iter()
        .filter_map(|entry| key.multiplicity.get(entry))
        .sum();
    PrecisionRecall {
        candidates: candidates.len(),
        true_positives,
        false_positives: candidates.len() - true_positives,
        recalled_functions,
        false_negatives: key.function_count - recalled_functions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{BankAddr, CandidateDetector, FunctionEntryEvidence, ProloguePattern};

    const SAMPLE: &str = r#"
[[section]]
name = "boot"
rom = 0x1000
vram = 0x80000400
size = 0x20
functions = [
  { name = "a", vram = 0x80000400, size = 0x10 },
  { name = "b", vram = 0x80000410, size = 0x10 },
]
"#;

    #[test]
    fn parses_physical_function_identities() {
        let key = parse_symbol_dump(SAMPLE).unwrap();
        assert_eq!(key.section_count, 1);
        assert_eq!(key.function_count, 2);
        assert_eq!(key.entries.len(), 2);
        assert!(key.entries.contains(&PhysicalEntry {
            rom: 0x1010,
            vram: 0x8000_0410,
        }));
    }

    #[test]
    fn grades_each_detector_and_combined_without_va_aliasing() {
        let key = parse_symbol_dump(SAMPLE).unwrap();
        let mut db = FactDb::new();
        let mapping = db.insert(Fact::RomMapping {
            bank: "boot".into(),
            rom_space: crate::facts::RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1020,
            va_start: 0x8000_0400,
            va_end: 0x8000_0420,
        });
        db.conclude("bank:boot", ProofState::Proven, vec![mapping], "test")
            .unwrap();

        for (pc, detector, evidence) in [
            (
                0x8000_0400,
                CandidateDetector::JalTarget,
                FunctionEntryEvidence::DirectJal {
                    call_site: BankAddr::new("boot", 0x8000_0410),
                },
            ),
            (
                0x8000_0408,
                CandidateDetector::ProloguePattern,
                FunctionEntryEvidence::Prologue {
                    stack_adjust: BankAddr::new("boot", 0x8000_0408),
                    frame_size: 0x20,
                    pattern: ProloguePattern::SavesReturnAddress,
                    corroborating_site: BankAddr::new("boot", 0x8000_040c),
                },
            ),
        ] {
            let target = BankAddr::new("boot", pc);
            let fact = db.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector,
                evidence,
                proposed_state: ProofState::Candidate,
            });
            db.conclude(
                function_entry_subject(&target),
                ProofState::Candidate,
                vec![fact],
                "test",
            )
            .unwrap();
        }

        let report = grade_candidates(&db, &key);
        let jal = report
            .per_detector
            .iter()
            .find(|grade| grade.detector == CandidateDetector::JalTarget)
            .unwrap();
        assert_eq!(jal.metrics.true_positives, 1);
        assert_eq!(jal.metrics.false_positives, 0);
        assert_eq!(report.combined.true_positives, 1);
        assert_eq!(report.combined.false_positives, 1);
        assert_eq!(report.combined.false_negatives, 1);
    }
}
