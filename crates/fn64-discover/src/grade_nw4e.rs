//! Cross-check for NW4E's descriptor-table bank discovery (Phase 2) against
//! `games/NW4E/overlays.json`, the byte-verified answer key faki-tools
//! already produced by hand-tracing the same ROM's loader dispatcher (see
//! that file's own provenance comment). Same posture as `grade_oot`: this
//! is grading-only input, never fed into the discovery pipeline itself.
//!
//! Unlike OoT's answer key, `overlays.json`'s `banks` array gives full
//! ROM-interval + VA-interval pairs (not just a VA start), so this
//! cross-check can and does compare both ends of the mapping exactly.

use crate::facts::{Fact, FactDb, ProofState};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct RawBank {
    bank: String,
    rom_start: String,
    rom_end: String,
    vram_load: String,
    #[serde(default)]
    vram_zero_end: Option<String>,
    #[serde(default)]
    vram_load_end: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawOverlaysJson {
    banks: Vec<RawBank>,
}

/// One answer-key bank from `overlays.json`, addresses parsed from hex
/// strings to `u32`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerBank {
    pub name: String,
    pub rom_start: u32,
    pub rom_end: u32,
    pub vram_load: u32,
    /// The end of the DMA'd (non-BSS-zero-fill) region, i.e. the VA
    /// interval this bank's ROM bytes actually cover -- `vram_load_end` if
    /// present, else falls back to `vram_zero_end`.
    pub vram_load_end: u32,
}

#[derive(Debug)]
pub enum ParseError {
    Json(serde_json::Error),
    BadHex {
        field: &'static str,
        bank: String,
        value: String,
    },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Json(e) => write!(f, "overlays.json parse error: {e}"),
            ParseError::BadHex { field, bank, value } => {
                write!(f, "bank '{bank}' field '{field}' is not valid hex: {value}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

fn parse_hex(field: &'static str, bank: &str, s: &str) -> Result<u32, ParseError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    u32::from_str_radix(s, 16).map_err(|_| ParseError::BadHex {
        field,
        bank: bank.to_string(),
        value: s.to_string(),
    })
}

/// Parse the `banks` array out of NW4E's `overlays.json`.
pub fn parse_overlays_json(json: &str) -> Result<Vec<AnswerBank>, ParseError> {
    let raw: RawOverlaysJson = serde_json::from_str(json).map_err(ParseError::Json)?;
    raw.banks
        .into_iter()
        .map(|b| {
            let rom_start = parse_hex("rom_start", &b.bank, &b.rom_start)?;
            let rom_end = parse_hex("rom_end", &b.bank, &b.rom_end)?;
            let vram_load = parse_hex("vram_load", &b.bank, &b.vram_load)?;
            let vram_load_end = match b.vram_load_end.or(b.vram_zero_end) {
                Some(v) => parse_hex("vram_load_end", &b.bank, &v)?,
                None => vram_load + (rom_end - rom_start),
            };
            Ok(AnswerBank {
                name: b.bank,
                rom_start,
                rom_end,
                vram_load,
                vram_load_end,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BankGrade {
    /// A `Proven` bank in our fact DB has the exact same ROM interval AND
    /// VA start as this answer-key bank.
    ExactMatch { discovered_bank: String },
    /// We proved a bank at this ROM interval but the VA start disagrees --
    /// this is a real discrepancy worth surfacing, not a match.
    VaMismatch {
        discovered_bank: String,
        discovered_va: u32,
    },
    /// No `Proven` bank in our fact DB has this ROM interval at all.
    Missed,
}

#[derive(Debug, Clone)]
pub struct Nw4eCrossCheck {
    pub total: usize,
    pub exact_matches: usize,
    pub va_mismatches: usize,
    pub missed: usize,
    pub per_bank: Vec<(String, BankGrade)>,
}

/// Cross-check this run's `FactDb` against NW4E's answer-key banks. Exact
/// match requires identical `(rom_start, rom_end, va_start)` -- NW4E's
/// records are simple flat DMA copies (no compression), so an honest
/// descriptor-table scan should reproduce them bit-for-bit if it read the
/// same table.
pub fn cross_check_nw4e(db: &FactDb, answer_banks: &[AnswerBank]) -> Nw4eCrossCheck {
    let proven_bank_names: Vec<(u32, u32, u32, String)> = db
        .facts()
        .iter()
        .filter_map(|f| match f {
            Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                ..
            } if db.conclusion(&format!("bank:{bank}")).map(|c| c.state)
                == Some(ProofState::Proven) =>
            {
                Some((*rom_start, *rom_end, *va_start, bank.clone()))
            }
            _ => None,
        })
        .collect();

    let mut per_bank = Vec::new();
    let mut exact_matches = 0;
    let mut va_mismatches = 0;
    let mut missed = 0;

    for answer in answer_banks {
        let same_rom_interval: Vec<&(u32, u32, u32, String)> = proven_bank_names
            .iter()
            .filter(|(rs, re, _, _)| *rs == answer.rom_start && *re == answer.rom_end)
            .collect();

        if same_rom_interval.is_empty() {
            missed += 1;
            per_bank.push((answer.name.clone(), BankGrade::Missed));
            continue;
        }

        if let Some((_, _, _, name)) = same_rom_interval
            .iter()
            .find(|(_, _, va, _)| *va == answer.vram_load)
        {
            exact_matches += 1;
            per_bank.push((
                answer.name.clone(),
                BankGrade::ExactMatch {
                    discovered_bank: name.clone(),
                },
            ));
        } else {
            let (_, _, va, name) = same_rom_interval[0];
            va_mismatches += 1;
            per_bank.push((
                answer.name.clone(),
                BankGrade::VaMismatch {
                    discovered_bank: name.clone(),
                    discovered_va: *va,
                },
            ));
        }
    }

    Nw4eCrossCheck {
        total: answer_banks.len(),
        exact_matches,
        va_mismatches,
        missed,
        per_bank,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"{
        "banks": [
            { "bank": "R1", "slot": "A", "record_rom": "0x539a0", "rom_start": "0x057310", "rom_end": "0x081210", "vram_load": "0x800d9960", "vram_load_end": "0x80103860", "vram_zero_start": "0x80103860", "vram_zero_end": "0x80106760" },
            { "bank": "R2", "slot": "B", "record_rom": "0x539c4", "rom_start": "0x081210", "rom_end": "0x0ae390", "vram_load": "0x80106760", "vram_load_end": "0x801338e0", "vram_zero_start": "0x801338e0", "vram_zero_end": "0x8016cdd0" }
        ]
    }"#;

    #[test]
    fn parses_sample_overlays_json() {
        let banks = parse_overlays_json(SAMPLE_JSON).unwrap();
        assert_eq!(banks.len(), 2);
        assert_eq!(banks[0].name, "R1");
        assert_eq!(banks[0].rom_start, 0x0005_7310);
        assert_eq!(banks[0].rom_end, 0x08_1210);
        assert_eq!(banks[0].vram_load, 0x800d_9960);
    }

    fn db_with_mapping(bank: &str, rom_start: u32, rom_end: u32, va_start: u32) -> FactDb {
        let mut db = FactDb::new();
        let f = db.insert(Fact::RomMapping {
            bank: bank.to_string(),
            rom_space: crate::facts::RomAddressSpace::Physical,
            rom_start,
            rom_end,
            va_start,
            va_end: va_start + (rom_end - rom_start),
        });
        db.conclude(format!("bank:{bank}"), ProofState::Proven, vec![f], "test")
            .unwrap();
        db
    }

    #[test]
    fn exact_match_when_rom_interval_and_va_agree() {
        let banks = parse_overlays_json(SAMPLE_JSON).unwrap();
        let db = db_with_mapping("overlay_0", 0x0005_7310, 0x0008_1210, 0x800d_9960);
        let report = cross_check_nw4e(&db, &banks[..1]);
        assert_eq!(report.exact_matches, 1);
        assert_eq!(report.missed, 0);
        assert_eq!(report.va_mismatches, 0);
    }

    #[test]
    fn va_mismatch_when_rom_interval_matches_but_va_disagrees() {
        let banks = parse_overlays_json(SAMPLE_JSON).unwrap();
        let db = db_with_mapping("overlay_0", 0x0005_7310, 0x0008_1210, 0xdead_0000);
        let report = cross_check_nw4e(&db, &banks[..1]);
        assert_eq!(report.va_mismatches, 1);
        assert_eq!(report.exact_matches, 0);
    }

    #[test]
    fn missed_when_no_bank_has_that_rom_interval() {
        let banks = parse_overlays_json(SAMPLE_JSON).unwrap();
        let db = FactDb::new();
        let report = cross_check_nw4e(&db, &banks[..1]);
        assert_eq!(report.missed, 1);
    }

    #[test]
    fn rejects_bad_hex_with_named_field() {
        let bad = r#"{"banks": [{"bank": "R1", "slot": "A", "record_rom": "0x0", "rom_start": "zzzz", "rom_end": "0x1", "vram_load": "0x1"}]}"#;
        let err = parse_overlays_json(bad).unwrap_err();
        match err {
            ParseError::BadHex { field, bank, .. } => {
                assert_eq!(field, "rom_start");
                assert_eq!(bank, "R1");
            }
            _ => panic!("expected BadHex"),
        }
    }
}
