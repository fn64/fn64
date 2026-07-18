//! Trace-ingestion gate: one real, digest-bound NW4E boot trace through the
//! canonical `trace::ingest_jsonl` path, a measured classification of every
//! observed PC against the resident bank's existing static evidence, and the
//! real measured `FactDb` delta from folding those observations in.
//!
//! What this gate does NOT do: promote anything beyond the one sound rule
//! `trace::fold_executed_pcs_into_fact_db` implements -- a known-bank
//! executed-PC observation becomes bank-scoped dynamic code-existence
//! evidence (`Fact::ObservedExecutedCode`, `ProofState::Supported`), never a
//! proven owner and never CFG-reachability proof. `IndirectTransfer`,
//! `PiDma`, and `WatchedTableWrite` facts still have no FactDb adapter and
//! stay at delta zero; bounded-exhaustiveness-aware indirect-target
//! corroboration remains a frontier. Before folding, this gate also reports
//! the same corroboration breakdown as before the adapter existed: how many
//! observed PCs land inside already-proven code, inside candidate code
//! (execution evidence agreeing with a heuristic claim), on
//! previously-unclassified resident-bank words, or on a word statically
//! classified as something other than code (loud conflict, never silently
//! resolved).
//!
//! Inputs (out-of-tree, loud when absent):
//!   FN64_DISCOVER_NW4E_ROM   -- the NW4E .z64
//!   FN64_DISCOVER_NW4E_TRACE -- a trace-schema JSONL capture bound to that
//!                               ROM's normalized SHA-256 (tools/mupen-trace)
//!
//! The whole report is computed twice from scratch and must be
//! byte-identical before anything is printed.

use fn64_discover::cfg::WordClass;
use fn64_discover::resolve::{build_cfg_closed_with_facts, build_cfg_exploratory_with_candidates};
use fn64_discover::trace::{ingest_jsonl, BankContext, NormalizedRomDigest, ObservedTraceFact};
use fn64_discover::{aki_reference, banks, run_discovery, DescriptorTableInput, Fact};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::BufReader;

fn main() {
    if let Err(error) = run() {
        eprintln!("trace gate FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rom_path = fn64_discover::required_env_path("FN64_DISCOVER_NW4E_ROM", "the NW4E .z64")?;
    let trace_path = fn64_discover::required_env_path(
        "FN64_DISCOVER_NW4E_TRACE",
        "a trace-schema JSONL capture for that ROM",
    )?;
    let rom_bytes =
        std::fs::read(&rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;

    println!("=== fn64-discover trace gate ===\n");
    let first = report(&rom_bytes, &trace_path)?;
    let second = report(&rom_bytes, &trace_path)?;
    if first != second {
        return Err("trace-gate report differed across two in-process runs".into());
    }
    print!("{first}");
    println!(
        "report sha256={:x} (verified byte-identical across two in-process runs)",
        Sha256::digest(first.as_bytes())
    );
    Ok(())
}

fn report(rom_bytes: &[u8], trace_path: &str) -> Result<String, String> {
    let mut out = String::new();
    let descriptor: DescriptorTableInput = (
        aki_reference::NW4E_DESCRIPTOR_TABLE,
        aki_reference::nw4e_bank_name,
    );
    let (rom, mut db) = run_discovery(rom_bytes, Some(descriptor))
        .map_err(|error| format!("normalizing NW4E ROM: {error}"))?;
    writeln!(out, "NW4E ROM: {} bytes, sha256={}", rom.len(), rom.sha256).unwrap();

    let expected = NormalizedRomDigest::try_from(rom.sha256.clone()).map_err(str::to_string)?;
    let file = std::fs::File::open(trace_path)
        .map_err(|error| format!("opening trace {trace_path}: {error}"))?;
    let ingest = ingest_jsonl(BufReader::new(file), &expected)
        .map_err(|error| format!("ingesting trace {trace_path}: {error}"))?;

    writeln!(
        out,
        "trace: id={:?} producer={:?}",
        ingest.header.trace_id, ingest.header.producer
    )
    .unwrap();
    writeln!(
        out,
        "  completion={:?} final_sequence={} facts={} (pi_dma={} executed_pc={} indirect_transfer={} watched_table_write={}) unknown_bank_observations={}",
        ingest.completion,
        ingest.final_sequence,
        ingest.facts.len(),
        ingest.counts.pi_dma,
        ingest.counts.executed_pc,
        ingest.counts.indirect_transfer,
        ingest.counts.watched_table_write,
        ingest.observations_with_unknown_bank,
    )
    .unwrap();
    for claim in &ingest.exhaustiveness {
        writeln!(
            out,
            "  bounded exhaustiveness claim: {:?} over sequences {}..={}",
            claim.domain, claim.first_sequence, claim.last_sequence
        )
        .unwrap();
    }

    // Resident (boot) bank geometry from the proven Phase-2 mapping.
    let mapping = db
        .proven_rom_mappings()
        .into_iter()
        .find_map(|fact| match fact {
            Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                va_end,
                ..
            } if bank == banks::BOOT_BANK => Some((*rom_start, *rom_end, *va_start, *va_end)),
            _ => None,
        })
        .ok_or("boot bank mapping not proven")?;
    let (rom_start, rom_end, va_start, va_end) = mapping;
    let bank_bytes = rom
        .bytes
        .get(rom_start as usize..rom_end as usize)
        .ok_or("boot mapping falls outside the normalized ROM")?;
    writeln!(
        out,
        "boot bank: rom=[0x{rom_start:x},0x{rom_end:x}) va=[0x{va_start:x},0x{va_end:x}) entrypoint=0x{:x}",
        rom.header.entry_point
    )
    .unwrap();

    // Static evidence baseline: proven-root closure vs exploratory closure
    // (proven roots + Phase-3 candidate entries). Words reached only by the
    // exploratory pass are candidate code; the trace corroborates them
    // without promoting them.
    let seed = [rom.header.entry_point];
    let (proven_cfg, _) =
        build_cfg_closed_with_facts(&db, banks::BOOT_BANK, bank_bytes, va_start, &seed);
    let (exploratory_cfg, _) =
        build_cfg_exploratory_with_candidates(&db, banks::BOOT_BANK, bank_bytes, va_start, &seed);
    let proven_code: BTreeSet<u32> = proven_cfg
        .word_class
        .iter()
        .filter_map(|(&va, &class)| (class == WordClass::ProvenCode).then_some(va))
        .collect();
    let exploratory_code: BTreeSet<u32> = exploratory_cfg
        .word_class
        .iter()
        .filter_map(|(&va, &class)| (class == WordClass::ProvenCode).then_some(va))
        .collect();
    let candidate_code: BTreeSet<u32> =
        exploratory_code.difference(&proven_code).copied().collect();
    writeln!(
        out,
        "static baseline: proven-closure code words={} exploratory-only (candidate) code words={}",
        proven_code.len(),
        candidate_code.len()
    )
    .unwrap();

    let mut total_pc_records = 0u64;
    let mut boot_pcs: BTreeSet<u32> = BTreeSet::new();
    let mut unknown_bank_pcs: BTreeSet<u32> = BTreeSet::new();
    let mut unknown_bank_records = 0u64;
    for fact in &ingest.facts {
        let ObservedTraceFact::ExecutedPc { pc, .. } = fact else {
            continue;
        };
        total_pc_records += 1;
        match &pc.bank {
            BankContext::Known { bank, .. } if bank == banks::BOOT_BANK => {
                if pc.address < va_start || pc.address >= va_end {
                    return Err(format!(
                        "trace claims boot bank for PC 0x{:08x} outside the proven boot mapping",
                        pc.address
                    ));
                }
                boot_pcs.insert(pc.address);
            }
            BankContext::Known { bank, .. } => {
                return Err(format!(
                    "trace claims unexpected bank {bank:?} for PC 0x{:08x}",
                    pc.address
                ));
            }
            BankContext::Unknown => {
                unknown_bank_records += 1;
                unknown_bank_pcs.insert(pc.address);
            }
        }
    }

    let mut proven_hits: BTreeSet<u32> = BTreeSet::new();
    let mut candidate_hits: BTreeSet<u32> = BTreeSet::new();
    let mut unclassified_hits: BTreeSet<u32> = BTreeSet::new();
    let mut non_code_class_hits: BTreeMap<u32, WordClass> = BTreeMap::new();
    for &pc in &boot_pcs {
        if proven_code.contains(&pc) {
            proven_hits.insert(pc);
        } else if candidate_code.contains(&pc) {
            candidate_hits.insert(pc);
        } else {
            match exploratory_cfg
                .word_class
                .get(&pc)
                .or_else(|| proven_cfg.word_class.get(&pc))
            {
                None | Some(WordClass::Unknown) => {
                    unclassified_hits.insert(pc);
                }
                Some(&class) => {
                    non_code_class_hits.insert(pc, class);
                }
            }
        }
    }

    writeln!(
        out,
        "observed PCs: {} records, {} unique on the resident bank, {} records / {} unique with unknown bank",
        total_pc_records,
        boot_pcs.len(),
        unknown_bank_records,
        unknown_bank_pcs.len()
    )
    .unwrap();
    if let (Some(min), Some(max)) = (boot_pcs.first(), boot_pcs.last()) {
        writeln!(out, "  resident PC span: [0x{min:08x}, 0x{max:08x}]").unwrap();
    }
    for pc in &unknown_bank_pcs {
        writeln!(out, "  unknown-bank PC: 0x{pc:08x}").unwrap();
    }
    writeln!(
        out,
        "resident classification: proven_code={} candidate_code_corroborated={} previously_unclassified={} non_code_class={}",
        proven_hits.len(),
        candidate_hits.len(),
        unclassified_hits.len(),
        non_code_class_hits.len()
    )
    .unwrap();
    // A non-code static class under an observed PC is a real disagreement
    // between static analysis and execution -- surface every site loudly.
    for (pc, class) in &non_code_class_hits {
        writeln!(
            out,
            "  CONFLICT: executed PC 0x{pc:08x} is statically {class:?}"
        )
        .unwrap();
    }
    if let (Some(min), Some(max)) = (unclassified_hits.first(), unclassified_hits.last()) {
        writeln!(
            out,
            "  previously-unclassified span: [0x{min:08x}, 0x{max:08x}] ({} words of new code-existence evidence)",
            unclassified_hits.len()
        )
        .unwrap();
    }

    for fact in &ingest.facts {
        let ObservedTraceFact::WatchedTableWrite {
            sequence,
            watch_id,
            address,
            width,
            value,
            ..
        } = fact
        else {
            continue;
        };
        writeln!(
            out,
            "watched value: seq={sequence} {watch_id} addr=0x{address:08x} {width:?} value=0x{value:x}"
        )
        .unwrap();
    }

    // Fold every known-bank ExecutedPc observation into the FactDb as typed
    // ObservedExecutedCode evidence and report the real measured delta --
    // this is the adapter trace.rs's own doc comment called a frontier.
    // Static lookup mirrors the classification above: exploratory_cfg is the
    // superset (proven-closure + candidate coverage), falling back to
    // proven_cfg for words the exploratory pass never reached at all.
    let static_word_class = |bank: &str, va: u32| -> Option<WordClass> {
        if bank != banks::BOOT_BANK {
            return None;
        }
        exploratory_cfg
            .word_class
            .get(&va)
            .or_else(|| proven_cfg.word_class.get(&va))
            .copied()
    };
    let fold_report = fn64_discover::trace::fold_executed_pcs_into_fact_db(
        &mut db,
        &ingest.header.trace_id,
        &ingest.facts,
        static_word_class,
    );
    writeln!(
        out,
        "FactDb delta from ingestion: {} facts added ({} words newly asserting code-existence, \
         {} corroborations of an already-observed word, {} static-data-vs-observed-code conflicts, \
         {} unknown-bank observations skipped)",
        fold_report.facts_added,
        fold_report.new_code_existence.len(),
        fold_report.corroborated.len(),
        fold_report.conflicts.len(),
        fold_report.unknown_bank_skipped,
    )
    .unwrap();
    if let (Some(min), Some(max)) = (
        fold_report.new_code_existence.iter().min(),
        fold_report.new_code_existence.iter().max(),
    ) {
        writeln!(
            out,
            "  new code-existence span: [0x{:08x}, 0x{:08x}]",
            min.pc, max.pc
        )
        .unwrap();
    }
    for conflict in &fold_report.conflicts {
        writeln!(
            out,
            "  CONFLICT: trace={:?} seq={} observed-executed PC 0x{:08x} is statically ProvenData",
            conflict.trace_id, conflict.sequence, conflict.site.pc
        )
        .unwrap();
    }
    Ok(out)
}
