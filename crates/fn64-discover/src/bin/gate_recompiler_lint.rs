//! Answer-key-free recompiler-correctness lint.
//!
//! A static recompiler that consumes N64Recomp-style per-function generated C
//! silently miscompiles a handful of function shapes that only surface at
//! runtime as stack corruption or infinite parks. fn64 hit these REACTIVELY
//! while booting WM2000 (the build.rs fall-through mend). This binary detects
//! them PROACTIVELY, with NO decomp answer key, by reasoning over the
//! machine-checked boundary list from
//! `fn64_discover::boundaries::recover_boundaries`.
//!
//! Class reported:
//!   MISSPLIT -- a proven-extent owner whose body ends with NO terminator
//!     (`jr`/`j`/`eret`) and whose extent-end is the start of the very next
//!     boundary. On hardware, execution falls straight through the split into
//!     the successor: the two fenced owners are one real function that
//!     authoritative-entry fencing (a `jal` landing at an interior address)
//!     cut in half. A per-function recompile drops the epilogue at the cut
//!     and mis-splits the frame -- the exact miscompile the WM2000 build.rs
//!     mend repairs by hand. Sound and answer-key-free: both extents are
//!     machine-checked; the terminator check is decode-only.
//!
//! Why only this class: under fn64's authoritative-entry-fenced partition,
//! owners never NEST (verified: 0 containment cases on WM2000) -- an interior
//! `jal` target starts a new fenced owner ABUTTING its predecessor rather than
//! sitting inside it. So the "bogus interior root" and "truncation" classes
//! from the N64Recomp world collapse into this single abutment-without-
//! terminator signal. Jump-table-OOB is structurally impossible in fn64's own
//! recompiler (typed `BlockExit`, never `goto L_XXXX`) and needs a resolved
//! table this ROM's overlays don't expose answer-key-free, so it is not
//! reported.
//!
//! Output: JSON-lines, one finding per line. READ-ONLY.
//!
//! Env:  FN64_DISCOVER_ROM   the game's .z64  (AKI family; no answer key)

use fn64_discover::boundaries::recover_boundaries;
use fn64_discover::required_env_path;
use std::collections::{BTreeMap, BTreeSet};

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_recompiler_lint: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rom_path =
        required_env_path("FN64_DISCOVER_ROM", "the game's .z64").map_err(|e| e.to_string())?;
    let rom_bytes =
        std::fs::read(&rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;

    let program = recover_boundaries(&rom_bytes)?;
    let findings = lint(&program);

    eprintln!(
        "recompiler-lint: {} boundaries, {} findings (MISSPLIT={})",
        program.boundaries.len(),
        findings.len(),
        findings.len()
    );
    for f in &findings {
        let detail = f.detail.replace('\\', "\\\\").replace('"', "\\\"");
        println!(
            "{{\"class\":\"MISSPLIT\",\"bank\":\"{}\",\"va\":\"0x{:x}\",\"detail\":\"{}\"}}",
            f.bank, f.va, detail
        );
    }
    Ok(())
}

struct Finding {
    bank: String,
    va: u32,
    detail: String,
}

/// The MISSPLIT check, factored out so tests can drive it with synthetic
/// programs. See module docs for the class definition.
fn lint(program: &fn64_discover::boundaries::RecoveredProgram) -> Vec<Finding> {
    let words_of: BTreeMap<&str, (&Vec<u32>, u32)> = program
        .banks
        .iter()
        .enumerate()
        .map(|(i, m)| (m.bank.as_str(), (&program.bank_words[i], m.va_start)))
        .collect();

    let mut starts_by_bank: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
    for b in &program.boundaries {
        starts_by_bank
            .entry(b.bank.as_str())
            .or_default()
            .insert(b.entry);
    }

    let mut findings: Vec<Finding> = Vec::new();
    for b in &program.boundaries {
        let (Some(end), Some(&(words, va_start))) = (b.va_end, words_of.get(b.bank.as_str()))
        else {
            continue; // no proven extent, or bank words missing
        };
        let starts = &starts_by_bank[b.bank.as_str()];
        // The extent-end must be the start of the very next boundary.
        if end == b.entry || !starts.contains(&end) {
            continue;
        }
        // Last real (non-zero) word of the body, and its delay slot: a jr/j in
        // the second-to-last slot with a nop-ish tail still terminates.
        let end_off = ((end - va_start) / 4) as usize;
        let root_off = ((b.entry - va_start) / 4) as usize;
        if end_off == 0 || end_off > words.len() || root_off >= end_off {
            continue;
        }
        let mut last = end_off - 1;
        while last > root_off && words.get(last) == Some(&0) {
            last -= 1;
        }
        let ends_clean =
            is_terminator(words[last]) || (last > root_off && is_terminator(words[last - 1]));
        if ends_clean {
            continue;
        }
        findings.push(Finding {
            bank: b.bank.clone(),
            va: b.entry,
            detail: format!(
                "owner [0x{:x}, 0x{:x}) ends without a terminator and abuts the next boundary at \
                 0x{:x} -- execution falls through the split; the two fenced owners are one real \
                 function a per-function recompile mis-splits (drops the epilogue at the cut)",
                b.entry, end, end
            ),
        });
    }
    findings.sort_by(|a, b| (&a.bank, a.va).cmp(&(&b.bank, b.va)));
    findings.dedup_by(|x, y| x.bank == y.bank && x.va == y.va);
    findings
}

/// True if `word` unconditionally ends straight-line flow: `jr` (funct 0x08),
/// `eret`, or an unconditional `j` (op 0x02). `jalr` links -> not a tail.
fn is_terminator(word: u32) -> bool {
    if word == 0x4200_0018 {
        return true; // eret
    }
    let op = word >> 26;
    op == 0x02 || (op == 0x00 && (word & 0x3f) == 0x08)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_discover::boundaries::{Boundary, RecoveredProgram};
    use fn64_discover::FactDb;

    fn prog(words: Vec<u32>, boundaries: Vec<(u32, Option<u32>)>) -> RecoveredProgram {
        use fn64_discover::boundaries::BankMapping;
        let va = 0x8000_0000u32;
        RecoveredProgram {
            banks: vec![BankMapping {
                bank: "b".into(),
                rom_start: 0,
                rom_end: (words.len() * 4) as u32,
                va_start: va,
                va_end: va + (words.len() * 4) as u32,
            }],
            bank_words: vec![words],
            snapshots: vec![],
            boundaries: boundaries
                .into_iter()
                .map(|(e, end)| Boundary {
                    bank: "b".into(),
                    entry: va + e,
                    va_end: end.map(|x| va + x),
                })
                .collect(),
            facts: FactDb::default(),
        }
    }

    const JR_RA: u32 = 0x03e0_0008; // jr $ra
    const NOP: u32 = 0x0000_0000;
    const SW: u32 = 0xad47_0020; // sw $7,0x20($10) -- not a terminator

    #[test]
    fn missplit_flagged_when_body_falls_through_into_next_boundary() {
        // owner [0x0,0x8) ends in SW (no terminator); next boundary at 0x8.
        let p = prog(
            vec![SW, SW, JR_RA, NOP],
            vec![(0x0, Some(0x8)), (0x8, Some(0x10))],
        );
        let f = lint(&p);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].va, 0x8000_0000);
    }

    #[test]
    fn clean_terminator_not_flagged() {
        // owner [0x0,0x8) ends in jr $ra + delay slot -> clean.
        let p = prog(
            vec![JR_RA, NOP, SW, JR_RA],
            vec![(0x0, Some(0x8)), (0x8, Some(0x10))],
        );
        assert!(lint(&p).is_empty());
    }

    #[test]
    fn no_next_boundary_not_flagged() {
        // owner [0x0,0x8) falls through but 0x8 is NOT a boundary start.
        let p = prog(vec![SW, SW, SW, SW], vec![(0x0, Some(0x8))]);
        assert!(lint(&p).is_empty());
    }

    #[test]
    fn no_proven_extent_not_flagged() {
        let p = prog(vec![SW, SW], vec![(0x0, None), (0x8, None)]);
        assert!(lint(&p).is_empty());
    }
}
