//! Grading gate for `delta_vote`: infer each NW4E overlay's mapping delta
//! from region bytes alone and grade the admissions against the byte-verified
//! `aki_reference::NW4E_BANKS` geometry.
//!
//! Held-out discipline: inference consumes ONLY `(rom_start, rom_end)` -- the
//! region's ROM byte interval -- never `va_start`. The known `va_start` enters
//! exclusively at grading time. ROM identity is bound by checking the
//! descriptor table's ROM-interval fields against the reference geometry
//! (its `vram_dest` field is deliberately not read).
//!
//! A WRONG admission (an admitted delta disagreeing with the key) is the
//! failure that matters and exits nonzero; OPEN outcomes are honest and only
//! reported. NWXE is not graded: its overlay ROM intervals have no
//! byte-verified descriptor table in `aki_reference`, and this crate has no
//! mechanical descriptor-free recovery of them yet -- that frontier is
//! stated, not papered over.
//!
//! Usage: FN64_DISCOVER_NW4E_ROM=<path to nomercy.z64> gate_delta_vote
//! Set FN64_DELTA_VOTE_FULL_SWEEP=1 to disable the lui-window narrowing and
//! grade the exhaustive pair-supported sweep instead (cost is reported).

use fn64_discover::aki_reference::{NW4E_BANKS, NW4E_DESCRIPTOR_TABLE};
use fn64_discover::banks::read_descriptor_records;
use fn64_discover::delta_vote::{
    infer_region_delta, DeltaVoteConfig, DeltaVoteOutcome, OpenReason,
};
use fn64_discover::{normalize, required_env_path};

fn main() {
    match run() {
        Ok(0) => {}
        Ok(wrong_admissions) => {
            eprintln!("gate_delta_vote: {wrong_admissions} WRONG admission(s) -- see table above");
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("gate_delta_vote: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<u32, String> {
    let rom_path = required_env_path("FN64_DISCOVER_NW4E_ROM", "the NW4E .z64")?;
    let rom_bytes =
        std::fs::read(&rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;
    let rom = normalize(&rom_bytes).map_err(|error| error.to_string())?;
    println!("gate_delta_vote: NW4E overlay delta inference (held-out va_start grading)");
    println!("ROM SHA-256 {}", rom.sha256);

    // Bind ROM identity through the descriptor table's ROM-interval fields
    // only. vram_dest is not read: the VA side stays held out end to end.
    let records = read_descriptor_records(&rom, NW4E_DESCRIPTOR_TABLE);
    for bank in &NW4E_BANKS {
        let matched = records
            .iter()
            .flatten()
            .any(|record| record.rom_start == bank.rom_start && record.rom_end == bank.rom_end);
        if !matched {
            return Err(format!(
                "descriptor table does not carry {}'s ROM interval [0x{:06x},0x{:06x}) -- wrong ROM?",
                bank.bank, bank.rom_start, bank.rom_end
            ));
        }
    }

    let config = DeltaVoteConfig {
        full_sweep: std::env::var("FN64_DELTA_VOTE_FULL_SWEEP").is_ok_and(|value| value == "1"),
        ..DeltaVoteConfig::default()
    };
    println!(
        "config: alignment=0x{:x} min_votes={} domination_factor={} full_sweep={} lui_min_count={} lui_max_uppers={}",
        config.alignment,
        config.min_votes,
        config.domination_factor,
        config.full_sweep,
        config.lui_min_count,
        config.lui_max_uppers,
    );
    println!();
    println!("bank  rom_interval             outcome   inferred_va  key_va      grade    a_top a_run margin b_top  cand    pairs");

    let mut wrong = 0u32;
    let mut correct = 0u32;
    let mut open = 0u32;
    for bank in &NW4E_BANKS {
        let region = rom
            .bytes
            .get(bank.rom_start as usize..bank.rom_end as usize)
            .ok_or_else(|| format!("{}: ROM interval out of bounds", bank.bank))?;
        let result = infer_region_delta(region, bank.rom_start, &[], &config);
        let key_delta = bank.va_start.wrapping_sub(bank.rom_start);

        let (outcome_text, inferred_va, grade) = match result.outcome {
            DeltaVoteOutcome::Admitted { delta, va_start } => {
                if delta == key_delta {
                    correct += 1;
                    (
                        "ADMITTED".to_string(),
                        format!("0x{va_start:08x}"),
                        "CORRECT",
                    )
                } else {
                    wrong += 1;
                    ("ADMITTED".to_string(), format!("0x{va_start:08x}"), "WRONG")
                }
            }
            DeltaVoteOutcome::Open { reason } => {
                open += 1;
                let reason_text = match reason {
                    OpenReason::NoLuiSegmentEvidence => "OPEN(no-lui-segment)".to_string(),
                    OpenReason::NoDeltaCandidates => "OPEN(no-candidates)".to_string(),
                    OpenReason::InsufficientVotes {
                        top_votes,
                        required,
                    } => {
                        format!("OPEN(votes {top_votes}<{required})")
                    }
                    OpenReason::NearTie {
                        top_votes,
                        runner_up_votes,
                        ..
                    } => format!("OPEN(near-tie {top_votes}:{runner_up_votes})"),
                };
                (reason_text, "-".to_string(), "open")
            }
        };
        let (a_top, b_top) = result
            .top
            .map(|score| (score.call_prologue_votes, score.hilo_in_region_votes))
            .unwrap_or((0, 0));
        let a_run = result
            .runner_up
            .map(|score| score.call_prologue_votes)
            .unwrap_or(0);
        let margin = if a_run > 0 {
            format!("{:.1}x", a_top as f64 / a_run as f64)
        } else {
            "inf".to_string()
        };
        println!(
            "{:<5} [0x{:06x},0x{:06x})  {:<22} {:<11} 0x{:08x}  {:<8} {:<5} {:<5} {:<6} {:<6} {:<7} {}",
            bank.bank,
            bank.rom_start,
            bank.rom_end,
            outcome_text,
            inferred_va,
            bank.va_start,
            grade,
            a_top,
            a_run,
            margin,
            b_top,
            result.candidate_count,
            result.pairs_considered,
        );
        println!(
            "      scan: words={} jal_sites={} distinct_targets={} prologues={} hilo_addrs={} branches_in_region={}/{} lui_sites={} windows={} segment={}",
            result.scan.words,
            result.scan.jal_sites,
            result.scan.distinct_jal_targets,
            result.scan.prologue_sites,
            result.scan.distinct_hilo_addresses,
            result.scan.branch_targets_in_region,
            result.scan.branch_sites,
            result.scan.lui_sites,
            result.scan.retained_lui_uppers,
            result
                .segment
                .map(|segment| format!("0x{segment:08x}"))
                .unwrap_or_else(|| "-".to_string()),
        );
        if grade == "WRONG" {
            println!(
                "      !!! WRONG ADMISSION: inferred delta 0x{:08x} disagrees with key delta 0x{key_delta:08x}",
                result.top.map(|score| score.delta).unwrap_or(0),
            );
        }
    }

    println!();
    println!(
        "summary: {}/{} admitted-correct, {} open, {} WRONG",
        correct,
        NW4E_BANKS.len(),
        open,
        wrong
    );
    println!(
        "NWXE: not graded -- overlay ROM intervals require a descriptor table or a \
descriptor-free table recovery that has not been byte-verified for NWXE; \
stating the frontier rather than guessing regions."
    );
    Ok(wrong)
}
