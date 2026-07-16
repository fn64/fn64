//! B2 grade (docs/DISCOVER-DESIGN.md acceptance gates + task B2's "THE
//! GRADE"): diff this crate's Phase 4/5 function boundaries against OoT's
//! real linked `boot` bank layout, as ground truth pulled mechanically from
//! the decomp's own linker map (never hand-curated -- see
//! `testdata/oot_boot_functions.csv`'s header comment / the extraction
//! script referenced there). Used only to grade; never fed into discovery.
//!
//! Grading posture, matching `grade_oot`'s: the gate requires `wrong == 0`.
//! "Wrong" here means a proven owner whose claimed extent contradicts the
//! answer key's own function boundaries (crosses into or splits a
//! known-real function incorrectly) -- not merely "found fewer functions
//! than exist" (`missed`/`open`), which is an honest, allowed gap per the
//! design doc's "honest limit."

use crate::partition::Owner;
use std::collections::BTreeSet;

/// The set of VAs that some instruction reaches via a direct `jal` (a
/// proven, machine-checkable callable-entry fact). An owner rooted at such a
/// VA is a *legitimate* interior callable entry even when a coarser answer
/// key merges it into an enclosing symbol -- see [`grade_functions`]'s
/// `InteriorEntry` classification.
pub type JalTargets = BTreeSet<u32>;

/// One answer-key row: a function name and its start VA. The *end* of each
/// function is derived as the next row's start VA (the CSV is emitted in
/// ascending address order by construction -- see the extraction script),
/// which is exactly how the linker map expressed it: one contiguous region
/// per named symbol, no gaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerFunction {
    pub name: String,
    pub va_start: u32,
}

pub struct ParsedFunctionKey {
    pub functions: Vec<AnswerFunction>,
    pub skipped: Vec<String>,
}

/// Parse the `name,va_start` (hex, no `0x` prefix) CSV format emitted by
/// the one-time extraction script.
pub fn parse_function_csv(csv: &str) -> ParsedFunctionKey {
    let mut functions = Vec::new();
    let mut skipped = Vec::new();
    for (lineno, line) in csv.lines().enumerate() {
        if lineno == 0 {
            continue; // header: "name,va_start"
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, ',');
        let (Some(name), Some(va_hex)) = (parts.next(), parts.next()) else {
            skipped.push(line.to_string());
            continue;
        };
        match u32::from_str_radix(va_hex.trim(), 16) {
            Ok(va_start) => functions.push(AnswerFunction {
                name: name.trim().to_string(),
                va_start,
            }),
            Err(_) => skipped.push(line.to_string()),
        }
    }
    // Ground truth must already be sorted ascending (extraction script
    // guarantees this) -- assert here rather than silently re-sorting,
    // since a re-sort would hide a corrupted/hand-edited answer key.
    functions.sort_by_key(|f| f.va_start);
    ParsedFunctionKey { functions, skipped }
}

/// One answer-key function's exact `[start, end)` interval, end derived
/// from the next row (or `bank_end` for the last one).
fn intervals(functions: &[AnswerFunction], bank_end: u32) -> Vec<(u32, u32, &str)> {
    let mut out = Vec::with_capacity(functions.len());
    for i in 0..functions.len() {
        let start = functions[i].va_start;
        let end = functions.get(i + 1).map(|f| f.va_start).unwrap_or(bank_end);
        out.push((start, end, functions[i].name.as_str()));
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionGrade {
    /// An owner's root_va exactly matches this answer-key function's start.
    MatchedExact,
    /// An owner exists whose extent covers this answer-key function's
    /// start, but not at an exact root boundary (a coarser/merged owner) --
    /// counted separately from `matched` since it is not a precise rung.
    MatchedCoarse { owner_root: u32 },
    /// No proven owner's extent contains this function's start at all.
    Open,
    /// A proven owner's root_va falls strictly inside this answer-key
    /// function's interval, AND that root is itself a proven `jal` target --
    /// a legitimately-discovered interior callable entry that the (coarser)
    /// answer key happens to merge into one symbol. Per the design doc's
    /// "interior callable entries remain explicit," this is CORRECT
    /// mechanical output, not a mis-split, so it does NOT count as `wrong`.
    InteriorEntry { owner_root: u32 },
    /// A proven owner's root_va falls strictly inside this answer-key
    /// function's interval and is NOT a proven `jal` target -- i.e. we split
    /// a real function at an address nothing actually calls, a genuine
    /// mis-split from bad decoding, counted as `wrong`.
    WrongSplit { owner_root: u32 },
}

#[derive(Debug, Clone)]
pub struct FunctionGradeReport {
    pub total: usize,
    pub matched_exact: usize,
    pub matched_coarse: usize,
    /// Answer-key functions the discovery split at a proven `jal` target
    /// (correct finer-grained interior entries, not errors).
    pub interior_entries: usize,
    pub open: usize,
    pub wrong: usize,
    pub per_function: Vec<(String, FunctionGrade)>,
}

impl FunctionGradeReport {
    pub fn matched_fraction(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.matched_exact + self.matched_coarse) as f64 / self.total as f64
    }
}

/// Grade a bank's [`Owner`] set against the parsed OoT `boot`-bank function
/// answer key. `bank_end` is the answer key's own section end VA
/// (`0x80006230` for OoT's boot bank -- the linker map's `.text` size),
/// used only to bound the last function's interval.
pub fn grade_functions(
    owners: &[Owner],
    answer_key: &[AnswerFunction],
    bank_end: u32,
    jal_targets: &JalTargets,
) -> FunctionGradeReport {
    let owner_roots: BTreeSet<u32> = owners.iter().map(|o| o.root_va).collect();
    let answer_intervals = intervals(answer_key, bank_end);

    let mut per_function = Vec::new();
    let mut matched_exact = 0;
    let mut matched_coarse = 0;
    let mut interior_entries = 0;
    let mut open = 0;
    let mut wrong = 0;

    for (start, end, name) in &answer_intervals {
        // Checked first and independent of the function's own start match:
        // an owner rooted strictly inside (start, end) either splits a real
        // function at an address nothing calls (a genuine mis-split =
        // `wrong`) OR splits it at a proven `jal` target (a correct interior
        // callable entry the coarser answer key merged = `InteriorEntry`,
        // NOT wrong). "Matched at your own boundary" does not excuse a
        // spurious extra root, but a jal-proven interior entry is not
        // spurious -- it is machine-checkable ground truth.
        let split_owner = owners
            .iter()
            .find(|o| o.root_va > *start && o.root_va < *end);
        if let Some(o) = split_owner {
            if jal_targets.contains(&o.root_va) {
                interior_entries += 1;
                per_function.push((
                    name.to_string(),
                    FunctionGrade::InteriorEntry {
                        owner_root: o.root_va,
                    },
                ));
            } else {
                wrong += 1;
                per_function.push((
                    name.to_string(),
                    FunctionGrade::WrongSplit {
                        owner_root: o.root_va,
                    },
                ));
            }
            continue;
        }

        if owner_roots.contains(start) {
            matched_exact += 1;
            per_function.push((name.to_string(), FunctionGrade::MatchedExact));
            continue;
        }

        // Is there an owner whose extent contains `start` (a coarser
        // owner that swallowed this function without splitting)?
        let covering = owners
            .iter()
            .find(|o| o.root_va <= *start && o.extent_end > *start);
        match covering {
            Some(o) => {
                matched_coarse += 1;
                per_function.push((
                    name.to_string(),
                    FunctionGrade::MatchedCoarse {
                        owner_root: o.root_va,
                    },
                ));
            }
            None => {
                open += 1;
                per_function.push((name.to_string(), FunctionGrade::Open));
            }
        }
    }

    FunctionGradeReport {
        total: answer_intervals.len(),
        matched_exact,
        matched_coarse,
        interior_entries,
        open,
        wrong,
        per_function,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::Owner;

    fn owner(bank: &str, root: u32, end: u32) -> Owner {
        Owner {
            bank: bank.to_string(),
            root_va: root,
            block_starts: vec![root],
            extent_end: end,
        }
    }

    #[test]
    fn parses_csv_and_sorts_by_address() {
        let csv = "name,va_start\nb,80000010\na,80000000\n";
        let parsed = parse_function_csv(csv);
        assert_eq!(parsed.functions[0].name, "a");
        assert_eq!(parsed.functions[1].name, "b");
    }

    #[test]
    fn exact_root_match_counts_as_matched_exact() {
        let key = vec![
            AnswerFunction {
                name: "f1".into(),
                va_start: 0x8000_0000,
            },
            AnswerFunction {
                name: "f2".into(),
                va_start: 0x8000_0010,
            },
        ];
        let owners = vec![
            owner("boot", 0x8000_0000, 0x8000_0010),
            owner("boot", 0x8000_0010, 0x8000_0020),
        ];
        let report = grade_functions(&owners, &key, 0x8000_0020, &JalTargets::new());
        assert_eq!(report.matched_exact, 2);
        assert_eq!(report.wrong, 0);
        assert_eq!(report.open, 0);
    }

    #[test]
    fn coarse_owner_covering_a_function_start_is_matched_coarse_not_wrong() {
        // One owner spans two real functions -- under-split, not
        // over-split, so it's `matched_coarse` for the second function,
        // not `wrong`.
        let key = vec![
            AnswerFunction {
                name: "f1".into(),
                va_start: 0x8000_0000,
            },
            AnswerFunction {
                name: "f2".into(),
                va_start: 0x8000_0010,
            },
        ];
        let owners = vec![owner("boot", 0x8000_0000, 0x8000_0020)];
        let report = grade_functions(&owners, &key, 0x8000_0020, &JalTargets::new());
        assert_eq!(report.matched_exact, 1); // f1
        assert_eq!(report.matched_coarse, 1); // f2, swallowed
        assert_eq!(report.wrong, 0);
    }

    #[test]
    fn owner_root_inside_a_real_function_interval_is_wrong() {
        // Real function f1 spans [0x0, 0x10). An owner rooted at 0x8
        // (strictly inside) is a real mis-split.
        let key = vec![
            AnswerFunction {
                name: "f1".into(),
                va_start: 0x8000_0000,
            },
            AnswerFunction {
                name: "f2".into(),
                va_start: 0x8000_0010,
            },
        ];
        let owners = vec![
            owner("boot", 0x8000_0000, 0x8000_0008),
            owner("boot", 0x8000_0008, 0x8000_0010),
            owner("boot", 0x8000_0010, 0x8000_0020),
        ];
        // 0x8 is NOT a jal target -> genuine mis-split -> wrong.
        let report = grade_functions(&owners, &key, 0x8000_0020, &JalTargets::new());
        assert_eq!(report.wrong, 1); // f1 was split by the spurious 0x8 owner
        assert_eq!(report.matched_exact, 1); // only f2 exact-matches cleanly
    }

    #[test]
    fn owner_root_inside_a_function_that_is_a_jal_target_is_interior_entry_not_wrong() {
        // Same split shape as the mis-split test, but now 0x8 IS a proven
        // jal target -- a legitimate interior callable entry the coarse
        // answer key merged. Must be `InteriorEntry`, never `wrong`.
        let key = vec![
            AnswerFunction {
                name: "f1".into(),
                va_start: 0x8000_0000,
            },
            AnswerFunction {
                name: "f2".into(),
                va_start: 0x8000_0010,
            },
        ];
        let owners = vec![
            owner("boot", 0x8000_0000, 0x8000_0008),
            owner("boot", 0x8000_0008, 0x8000_0010),
            owner("boot", 0x8000_0010, 0x8000_0020),
        ];
        let mut jal = JalTargets::new();
        jal.insert(0x8000_0008);
        let report = grade_functions(&owners, &key, 0x8000_0020, &jal);
        assert_eq!(report.wrong, 0);
        assert_eq!(report.interior_entries, 1);
        assert_eq!(report.matched_exact, 1); // f2 still exact
    }

    #[test]
    fn no_covering_owner_is_open() {
        let key = vec![AnswerFunction {
            name: "f1".into(),
            va_start: 0x8000_0000,
        }];
        let owners: Vec<Owner> = vec![];
        let report = grade_functions(&owners, &key, 0x8000_0010, &JalTargets::new());
        assert_eq!(report.open, 1);
        assert_eq!(report.wrong, 0);
    }
}
