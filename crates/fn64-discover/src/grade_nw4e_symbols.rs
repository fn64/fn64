//! B2 grade: the "grind-collapse" measure. Diffs this crate's Phase 4/5
//! function boundaries against aki-recomp's **hand-fixed** NW4E
//! `disasm/symbol_addrs.txt` rungs -- the ~36 addresses a human had to
//! manually mis-split-correct during the symbol-driven recompile (see
//! `testdata/nw4e_hand_fixed_symbol_addrs.txt`, copied verbatim from
//! `games/NW4E/disasm/symbol_addrs.txt`). Every row in that file already
//! carries its own `size:` field (the human-verified extent), so grading
//! here does not need a synthesized "next row's start" like
//! [`crate::grade_oot_functions`] -- each row is a complete, self-contained
//! ground-truth interval `[addr, addr+size)`, optionally bank-qualified via
//! a `segment:` comment.
//!
//! This is grading-only input: it never feeds the discovery pipeline, and
//! it is graded against the mechanical output only, never against another
//! human's opinion.

use crate::partition::Owner;
use std::collections::BTreeMap;

/// One hand-fixed rung: an exact, human-verified `[va_start, va_start+size)`
/// interval, optionally naming the bank it lives in (`None` means the
/// always-resident bank, i.e. no `segment:` comment in the source file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandFixedRung {
    pub va_start: u32,
    pub size: u32,
    pub bank: Option<String>,
}

impl HandFixedRung {
    pub fn va_end(&self) -> u32 {
        self.va_start.wrapping_add(self.size)
    }
}

#[derive(Debug)]
pub struct ParsedSymbolAddrs {
    pub rungs: Vec<HandFixedRung>,
    pub skipped: Vec<String>,
}

/// Parse aki-recomp's `symbol_addrs.txt` line format:
///
/// ```text
/// func_800E284C = 0x800E284C; // type:func size:0x74 segment:R4_text
/// ```
///
/// Only `type:func` rows are kept (this file's format is shared with
/// non-function symbol rows in general splat-style symbol_addrs files,
/// though none appear in the current NW4E file -- filtering defensively
/// rather than assuming). Anything that doesn't parse is `skipped`, not
/// silently dropped, per this crate's answer-key parsing precedent
/// (`grade_oot::parse_segments_csv`, `grade_nw4e::parse_overlays_json`).
pub fn parse_symbol_addrs(text: &str) -> ParsedSymbolAddrs {
    let mut rungs = Vec::new();
    let mut skipped = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((decl, comment)) = line.split_once("//") else {
            skipped.push(line.to_string());
            continue;
        };
        // decl: "func_800E284C = 0x800E284C;"
        let Some(eq_rhs) = decl.split('=').nth(1) else {
            skipped.push(line.to_string());
            continue;
        };
        let addr_str = eq_rhs.trim().trim_end_matches(';').trim();
        let addr_str = addr_str.strip_prefix("0x").unwrap_or(addr_str);
        let Ok(va_start) = u32::from_str_radix(addr_str, 16) else {
            skipped.push(line.to_string());
            continue;
        };

        if !comment.contains("type:func") {
            continue; // not a function row; not an error, just out of scope
        }

        let Some(size) = extract_field(comment, "size:") else {
            skipped.push(line.to_string());
            continue;
        };
        let bank = extract_field_str(comment, "segment:");

        rungs.push(HandFixedRung {
            va_start,
            size,
            bank,
        });
    }

    ParsedSymbolAddrs { rungs, skipped }
}

fn extract_field(comment: &str, key: &str) -> Option<u32> {
    let idx = comment.find(key)?;
    let rest = &comment[idx + key.len()..];
    let token = rest.split_whitespace().next()?;
    let token = token.strip_prefix("0x").unwrap_or(token);
    u32::from_str_radix(token, 16).ok()
}

fn extract_field_str(comment: &str, key: &str) -> Option<String> {
    let idx = comment.find(key)?;
    let rest = &comment[idx + key.len()..];
    rest.split_whitespace().next().map(|s| s.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RungGrade {
    /// A proven owner in the matching bank has `root_va == va_start` and
    /// `extent_end == va_end` -- the exact split the hand-fix rung
    /// required, recovered mechanically.
    Recovered,
    /// An owner starts at the right address but its extent disagrees with
    /// the hand-verified size -- partial credit, not full recovery.
    PartialExtentMismatch { owner_extent_end: u32 },
    /// No proven owner starts at this rung's address in the named bank at
    /// all -- not yet recovered by this phase (expected until Phase 3's
    /// candidate harvesting supplies the missing roots this rung's
    /// boundary depends on).
    NotRecovered,
}

#[derive(Debug, Clone)]
pub struct GrindCollapseReport {
    pub total_rungs: usize,
    pub recovered: usize,
    pub partial: usize,
    pub not_recovered: usize,
    pub per_rung: Vec<(u32, RungGrade)>,
}

impl GrindCollapseReport {
    pub fn recovered_fraction(&self) -> f64 {
        if self.total_rungs == 0 {
            return 0.0;
        }
        self.recovered as f64 / self.total_rungs as f64
    }
}

/// Grade hand-fixed rungs against owners grouped by bank name (`None` key
/// for the always-resident bank, matching [`HandFixedRung::bank`]'s
/// convention). Each rung is graded only against owners in its own bank --
/// a same-address owner in the wrong bank is not a match, since identity is
/// bank-qualified throughout this crate.
pub fn grade_grind_collapse(
    owners_by_bank: &BTreeMap<Option<String>, Vec<Owner>>,
    rungs: &[HandFixedRung],
) -> GrindCollapseReport {
    let mut recovered = 0;
    let mut partial = 0;
    let mut not_recovered = 0;
    let mut per_rung = Vec::new();

    let empty: Vec<Owner> = Vec::new();
    for rung in rungs {
        let owners = owners_by_bank.get(&rung.bank).unwrap_or(&empty);
        match owners.iter().find(|o| o.root_va == rung.va_start) {
            Some(o) if o.extent_end == rung.va_end() => {
                recovered += 1;
                per_rung.push((rung.va_start, RungGrade::Recovered));
            }
            Some(o) => {
                partial += 1;
                per_rung.push((
                    rung.va_start,
                    RungGrade::PartialExtentMismatch {
                        owner_extent_end: o.extent_end,
                    },
                ));
            }
            None => {
                not_recovered += 1;
                per_rung.push((rung.va_start, RungGrade::NotRecovered));
            }
        }
    }

    GrindCollapseReport {
        total_rungs: rungs.len(),
        recovered,
        partial,
        not_recovered,
        per_rung,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::Owner;

    #[test]
    fn parses_resident_rung_with_no_segment() {
        let text = "func_80000F98 = 0x80000F98; // type:func size:0x138\n";
        let parsed = parse_symbol_addrs(text);
        assert_eq!(parsed.rungs.len(), 1);
        assert_eq!(parsed.rungs[0].va_start, 0x8000_0f98);
        assert_eq!(parsed.rungs[0].size, 0x138);
        assert_eq!(parsed.rungs[0].bank, None);
    }

    #[test]
    fn parses_overlay_rung_with_segment() {
        let text = "func_800E284C = 0x800E284C; // type:func size:0x74 segment:R4_text\n";
        let parsed = parse_symbol_addrs(text);
        assert_eq!(parsed.rungs[0].bank, Some("R4_text".to_string()));
    }

    #[test]
    fn parses_the_real_fixture_file_without_skips() {
        let text = include_str!("../testdata/nw4e_hand_fixed_symbol_addrs.txt");
        let parsed = parse_symbol_addrs(text);
        assert!(
            parsed.skipped.is_empty(),
            "unexpected unparsed rows: {:?}",
            parsed.skipped
        );
        assert_eq!(parsed.rungs.len(), 36);
    }

    fn owner(root: u32, end: u32) -> Owner {
        Owner {
            bank: "boot".to_string(),
            root_va: root,
            block_starts: vec![root],
            extent_end: end,
        }
    }

    #[test]
    fn recovered_when_owner_matches_start_and_extent_exactly() {
        let rung = HandFixedRung {
            va_start: 0x8000_0000,
            size: 0x10,
            bank: None,
        };
        let mut map = BTreeMap::new();
        map.insert(None, vec![owner(0x8000_0000, 0x8000_0010)]);
        let report = grade_grind_collapse(&map, &[rung]);
        assert_eq!(report.recovered, 1);
        assert_eq!(report.recovered_fraction(), 1.0);
    }

    #[test]
    fn partial_when_start_matches_but_extent_disagrees() {
        let rung = HandFixedRung {
            va_start: 0x8000_0000,
            size: 0x10,
            bank: None,
        };
        let mut map = BTreeMap::new();
        map.insert(None, vec![owner(0x8000_0000, 0x8000_0020)]);
        let report = grade_grind_collapse(&map, &[rung]);
        assert_eq!(report.partial, 1);
        assert_eq!(report.recovered, 0);
    }

    #[test]
    fn not_recovered_when_no_owner_in_the_right_bank_starts_there() {
        let rung = HandFixedRung {
            va_start: 0x8000_0000,
            size: 0x10,
            bank: Some("R4_text".into()),
        };
        let mut map = BTreeMap::new();
        map.insert(None, vec![owner(0x8000_0000, 0x8000_0010)]); // wrong bank key
        let report = grade_grind_collapse(&map, &[rung]);
        assert_eq!(report.not_recovered, 1);
    }
}
