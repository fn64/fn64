//! B1 gate: grade OoT bank discovery against the decomp's real segment
//! layout, per the task's "GRADE: OoT bank discovery vs the decomp segment
//! layout (spec/segments) - % real segments found, wrong=0."
//!
//! The decomp is used **only** to grade, never as pipeline input -- this
//! module never touches ROM bytes, only the answer-key file the caller
//! supplies (`baseroms/<version>/segments.csv` from a checked-out
//! oot-decomp tree) and the banks our own discovery already produced.
//!
//! `segments.csv` gives segment name + VRAM start only (no ROM-side
//! interval or size -- that only exists after a real link), so grading
//! compares each named segment's VRAM start against the **interval**
//! `[va_start, va_end)` of each `Proven` bank, not against another VRAM
//! start. This matters: Phase 2 bank discovery is deliberately coarser
//! than the decomp's linked C-level segments -- OoT's IPL3 boot copy is
//! one bank (`boot`, `[0x1000, 0x101000)` ROM -> `[header.entry_point,
//! ...)` VA) that the decomp's own linker later carves into two named
//! segments, `makerom` (0x80000000, the pre-entry-point IPL3 prefix) and
//! `boot` (0x80000460, the first real C symbol). A bank containing a named
//! segment's start address is exactly what "found" should mean at this
//! phase; only claiming VA territory the ground truth flatly contradicts
//! counts as `wrong`.

use crate::facts::{Fact, FactDb, ProofState};

/// One row of the decomp's answer key: a named segment and its VRAM start
/// address (hex string in the CSV, e.g. "80000460"; empty for
/// file-only/non-VA segments like raw asset banks that are never mapped
/// live).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerSegment {
    pub name: String,
    pub vram_start: Option<u32>,
}

/// Parse `segments.csv`'s `Name,VRAM start` format. Malformed rows are
/// skipped with a note rather than aborting the whole parse -- this is
/// answer-key ingestion, not evidence collection, so "best-effort but
/// visible" is the right posture; a caller can inspect `skipped` to see
/// exactly what didn't parse.
pub struct ParsedAnswerKey {
    pub segments: Vec<AnswerSegment>,
    pub skipped: Vec<String>,
}

pub fn parse_segments_csv(csv: &str) -> ParsedAnswerKey {
    let mut segments = Vec::new();
    let mut skipped = Vec::new();
    for (lineno, line) in csv.lines().enumerate() {
        if lineno == 0 {
            continue; // header row: "Name,VRAM start"
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, ',');
        let (Some(name), Some(vram_field)) = (parts.next(), parts.next()) else {
            skipped.push(line.to_string());
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            skipped.push(line.to_string());
            continue;
        }
        let vram_field = vram_field.trim();
        let vram_start = if vram_field.is_empty() {
            None
        } else {
            match u32::from_str_radix(vram_field, 16) {
                Ok(v) => Some(v),
                Err(_) => {
                    skipped.push(line.to_string());
                    continue;
                }
            }
        };
        segments.push(AnswerSegment {
            name: name.to_string(),
            vram_start,
        });
    }
    ParsedAnswerKey { segments, skipped }
}

/// One graded outcome for a single answer-key segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentGrade {
    /// Exactly one `Proven` bank's `[va_start, va_end)` interval contains
    /// this segment's VRAM start.
    Found { bank: String },
    /// The answer key names a segment with a known VRAM start, and no
    /// `Proven` bank's interval contains it.
    Missed,
    /// The answer key has no VRAM start for this segment (e.g. a
    /// file-only asset bank never given a live mapping) -- not counted
    /// against or for discovery, since there is nothing to find.
    NotApplicable,
    /// More than one `Proven` bank's interval contains this VA (bank
    /// intervals overlap in VA space) -- recorded honestly as ambiguous
    /// rather than arbitrarily picking one.
    Ambiguous { candidates: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct GradeReport {
    pub total_applicable: usize,
    pub found: usize,
    pub missed: usize,
    pub ambiguous: usize,
    pub wrong: usize,
    pub per_segment: Vec<(String, SegmentGrade)>,
}

impl GradeReport {
    pub fn found_fraction(&self) -> f64 {
        if self.total_applicable == 0 {
            return 0.0;
        }
        self.found as f64 / self.total_applicable as f64
    }
}

/// A proven bank's runtime-VA interval, `[va_start, va_end)`.
struct ProvenInterval {
    bank: String,
    va_start: u32,
    va_end: u32,
}

impl ProvenInterval {
    fn contains(&self, va: u32) -> bool {
        va >= self.va_start && va < self.va_end
    }
}

/// Grade this run's `FactDb` (must already have run bank discovery)
/// against a parsed OoT answer key. "Wrong" is defined strictly: a
/// `Proven` bank whose entire `[va_start, va_end)` interval overlaps no
/// answer-key segment's VA at all is a false positive (`wrong`), separate
/// from `missed` (a real segment whose VA falls inside no proven bank).
/// The gate requires `wrong == 0` -- discovery is allowed to be
/// incomplete or coarser-grained than the decomp's own segmentation,
/// never incorrect about ROM/VA territory the ground truth contradicts.
pub fn grade_against_oot(db: &FactDb, answer_key: &[AnswerSegment]) -> GradeReport {
    let proven: Vec<ProvenInterval> = db
        .facts()
        .iter()
        .filter_map(|f| match f {
            Fact::RomMapping {
                bank,
                va_start,
                va_end,
                ..
            } if db.conclusion(&format!("bank:{bank}")).map(|c| c.state)
                == Some(ProofState::Proven) =>
            {
                Some(ProvenInterval {
                    bank: bank.clone(),
                    va_start: *va_start,
                    va_end: *va_end,
                })
            }
            _ => None,
        })
        .collect();

    let mut per_segment = Vec::new();
    let mut found = 0;
    let mut missed = 0;
    let mut ambiguous = 0;
    let mut total_applicable = 0;

    for seg in answer_key {
        let Some(va) = seg.vram_start else {
            per_segment.push((seg.name.clone(), SegmentGrade::NotApplicable));
            continue;
        };
        total_applicable += 1;

        let mut containing: Vec<&str> = proven
            .iter()
            .filter(|p| p.contains(va))
            .map(|p| p.bank.as_str())
            .collect();
        containing.sort_unstable();
        containing.dedup();

        match containing.as_slice() {
            [] => {
                missed += 1;
                per_segment.push((seg.name.clone(), SegmentGrade::Missed));
            }
            [only] => {
                found += 1;
                per_segment.push((
                    seg.name.clone(),
                    SegmentGrade::Found {
                        bank: only.to_string(),
                    },
                ));
            }
            many => {
                ambiguous += 1;
                per_segment.push((
                    seg.name.clone(),
                    SegmentGrade::Ambiguous {
                        candidates: many.iter().map(|s| s.to_string()).collect(),
                    },
                ));
            }
        }
    }

    // "Wrong": a proven bank interval that overlaps no answer-key VA at
    // all -- a claim the ground truth flatly contradicts, as opposed to a
    // bank that is simply coarser than (a superset containing) one or
    // more real segments, which is exactly what `Found` captures above.
    let answer_vas: Vec<u32> = answer_key.iter().filter_map(|s| s.vram_start).collect();
    let wrong = proven
        .iter()
        .filter(|p| !answer_vas.iter().any(|va| p.contains(*va)))
        .count();

    GradeReport {
        total_applicable,
        found,
        missed,
        ambiguous,
        wrong,
        per_segment,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::BankAddr;

    #[test]
    fn parses_basic_csv_with_header_and_blank_vram() {
        let csv = "Name,VRAM start\nmakerom,80000000\nboot,80000460\ndmadata,\n";
        let parsed = parse_segments_csv(csv);
        assert_eq!(parsed.segments.len(), 3);
        assert_eq!(
            parsed.segments[0],
            AnswerSegment {
                name: "makerom".into(),
                vram_start: Some(0x8000_0000)
            }
        );
        assert_eq!(
            parsed.segments[2],
            AnswerSegment {
                name: "dmadata".into(),
                vram_start: None
            }
        );
        assert!(parsed.skipped.is_empty());
    }

    #[test]
    fn skips_malformed_rows_without_aborting() {
        let csv = "Name,VRAM start\nboot,80000460\ngarbage_row_no_comma\nok,80100000\n";
        let parsed = parse_segments_csv(csv);
        assert_eq!(parsed.segments.len(), 2);
        assert_eq!(parsed.skipped, vec!["garbage_row_no_comma".to_string()]);
    }

    fn db_with_proven_bank(bank: &str, va_start: u32, va_len: u32) -> FactDb {
        let mut db = FactDb::new();
        let f = db.insert(Fact::RomMapping {
            bank: bank.to_string(),
            rom_start: 0x1000,
            rom_end: 0x1000 + va_len,
            va_start,
            va_end: va_start + va_len,
        });
        db.conclude(format!("bank:{bank}"), ProofState::Proven, vec![f], "test")
            .unwrap();
        db
    }

    #[test]
    fn found_when_va_start_matches_exactly() {
        let db = db_with_proven_bank("boot", 0x8000_0460, 0x1000);
        let key = vec![AnswerSegment {
            name: "boot".into(),
            vram_start: Some(0x8000_0460),
        }];
        let report = grade_against_oot(&db, &key);
        assert_eq!(report.found, 1);
        assert_eq!(report.missed, 0);
        assert_eq!(report.wrong, 0);
        assert_eq!(report.found_fraction(), 1.0);
    }

    #[test]
    fn found_when_bank_interval_contains_but_does_not_start_at_the_segment_va() {
        // The real OoT case: one boot-copy bank spans both "makerom"
        // (0x80000000) and "boot" (0x80000460); both should count as
        // found even though only one can be the bank's exact va_start.
        let db = db_with_proven_bank("boot", 0x8000_0400, 0x1000);
        let key = vec![
            AnswerSegment {
                name: "makerom".into(),
                vram_start: Some(0x8000_0000),
            },
            AnswerSegment {
                name: "boot".into(),
                vram_start: Some(0x8000_0460),
            },
        ];
        // makerom (0x80000000) is NOT inside [0x80000400, 0x80001400) --
        // this asserts the containment check is real interval math, not a
        // rubber stamp: only "boot" should be found here.
        let report = grade_against_oot(&db, &key);
        assert_eq!(report.found, 1);
        assert_eq!(report.missed, 1);
        assert_eq!(report.wrong, 0);
    }

    #[test]
    fn missed_when_no_proven_bank_contains_that_va() {
        let db = FactDb::new();
        let key = vec![AnswerSegment {
            name: "boot".into(),
            vram_start: Some(0x8000_0460),
        }];
        let report = grade_against_oot(&db, &key);
        assert_eq!(report.found, 0);
        assert_eq!(report.missed, 1);
        assert_eq!(report.wrong, 0);
    }

    #[test]
    fn not_applicable_segments_neither_help_nor_hurt_the_fraction() {
        let db = db_with_proven_bank("boot", 0x8000_0460, 0x1000);
        let key = vec![
            AnswerSegment {
                name: "boot".into(),
                vram_start: Some(0x8000_0460),
            },
            AnswerSegment {
                name: "dmadata".into(),
                vram_start: None,
            },
        ];
        let report = grade_against_oot(&db, &key);
        assert_eq!(report.total_applicable, 1);
        assert_eq!(report.found, 1);
        assert_eq!(report.found_fraction(), 1.0);
    }

    #[test]
    fn wrong_is_nonzero_when_a_proven_bank_overlaps_no_real_segment() {
        let db = db_with_proven_bank("phantom", 0x8099_9999, 0x100);
        let key = vec![AnswerSegment {
            name: "boot".into(),
            vram_start: Some(0x8000_0460),
        }];
        let report = grade_against_oot(&db, &key);
        assert_eq!(report.wrong, 1);
        assert_eq!(report.missed, 1);
    }

    #[test]
    fn ambiguous_when_two_proven_bank_intervals_both_contain_the_same_va() {
        let mut db = db_with_proven_bank("bank_a", 0x8000_0000, 0x2000);
        let f = db.insert(Fact::RomMapping {
            bank: "bank_b".to_string(),
            rom_start: 0x2000,
            rom_end: 0x4000,
            va_start: 0x8000_0000,
            va_end: 0x8000_2000,
        });
        db.conclude("bank:bank_b", ProofState::Proven, vec![f], "test")
            .unwrap();

        let key = vec![AnswerSegment {
            name: "icon_item_static".into(),
            vram_start: Some(0x8000_0000),
        }];
        let report = grade_against_oot(&db, &key);
        assert_eq!(report.ambiguous, 1);
        assert_eq!(report.found, 0);
    }

    #[test]
    fn evidence_facts_do_not_affect_grading_only_rom_mapping_facts_do() {
        let mut db = FactDb::new();
        db.insert(Fact::Evidence {
            subject: BankAddr::new("boot", 0x8000_0460),
            note: "noise".into(),
        });
        let key = vec![AnswerSegment {
            name: "boot".into(),
            vram_start: Some(0x8000_0460),
        }];
        let report = grade_against_oot(&db, &key);
        assert_eq!(report.found, 0);
        assert_eq!(report.missed, 1);
    }
}
