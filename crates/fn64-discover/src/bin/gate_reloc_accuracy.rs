//! Held-out relocation/symbolization-accuracy gate.
//!
//! Recovery is completed from ROM bytes before `FN64_DISCOVER_OOT_DUMP` is
//! opened. The dump is grading-only and cannot become a root, mapping, range,
//! fact, or proof. Its current schema has function symbols and extents but no
//! relocation table or data symbols, so the reported misclassification rate is
//! explicitly the proxy "recovered target is an exact known function symbol."

use fn64_discover::block_proof::BlockAssessment;
use fn64_discover::facts::{FunctionEntryEvidence, ProofState};
use fn64_discover::gp_base::{self, DataRange, GpAccessKind, GpBaseOutcome};
use fn64_discover::loaders::{recognize_entry_stub_any, RecognizedEntryStub, VirtualAddress};
use fn64_discover::reloc_grade::{
    grade_references, parse_relocation_key, RecoveredReference, ReferenceGrade, ReferenceKind,
    WrongReason,
};
use fn64_discover::resolve::{IndirectProofState, IndirectResolutionKind};
use fn64_discover::snapshot::{compose_materialized_banks_v1, MaterializedBankInput};
use fn64_discover::xref::{scan_global_refs, RefKind};
use fn64_discover::{
    banks, required_env_path, run_discovery_with_load_image_tables, Fact, FactDb, NormalizedRom,
    RomAddressSpace,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const ROM_VAR: &str = "FN64_DISCOVER_OOT_ROM";
const DUMP_VAR: &str = "FN64_DISCOVER_OOT_DUMP";

#[derive(Debug, Clone)]
struct PhysicalBank {
    bank: String,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_reloc_accuracy FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rom_path = required_env_path(ROM_VAR, "an OoT NTSC 1.0 .z64")?;
    let dump_path = required_env_path(DUMP_VAR, "the held-out OoT reference dump.toml")?;
    let rom_bytes =
        std::fs::read(&rom_path).map_err(|error| format!("reading {ROM_VAR}: {error}"))?;

    println!("=== fn64-discover relocation-accuracy gate ===");
    println!("recovery input: ROM bytes only; held-out dump remains unopened");

    let tables = fn64_discover::oot_reference::oot_load_image_tables();
    let (rom, facts) = run_discovery_with_load_image_tables(&rom_bytes, None, &tables)
        .map_err(|error| format!("OoT discovery: {error}"))?;
    let physical = physical_banks(&facts)?;
    if physical.is_empty() {
        return Err("discovery produced no proven physical bank".to_string());
    }

    let mut materialized = Vec::with_capacity(physical.len());
    let mut roots = Vec::with_capacity(physical.len());
    for bank in &physical {
        materialized.push(
            rom.bytes
                .get(bank.rom_start as usize..bank.rom_end as usize)
                .ok_or_else(|| format!("bank {} has ROM backing outside the image", bank.bank))?,
        );
        roots.push(callable_roots(&facts, bank));
    }
    let inputs: Vec<_> = physical
        .iter()
        .enumerate()
        .map(|(index, bank)| MaterializedBankInput {
            bank: &bank.bank,
            va_start: bank.va_start,
            bytes: materialized[index],
            seed_roots: &roots[index],
        })
        .collect();
    let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs)
        .map_err(|error| format!("composing discovered banks: {error}"))?;

    let (references, gp_frontier) = recover_references(&rom, &physical, &snapshots)?;
    if references.is_empty() {
        return Err("recovery produced zero gradable references".to_string());
    }
    let reference_json = serde_json::to_vec(&references).map_err(|error| error.to_string())?;
    let reference_digest = format!("{:x}", Sha256::digest(&reference_json));

    println!("ROM sha256={}", rom.sha256);
    println!(
        "discovery complete: physical_banks={} recovered_references={} reference_digest={reference_digest}",
        physical.len(),
        references.len()
    );
    println!("gp recovery frontier: {gp_frontier}");

    // HELD-OUT BOUNDARY: reference enumeration and its digest are final above.
    // The key can only grade that immutable vector; it cannot affect discovery,
    // composition, executable proof, xref scanning, GP recovery, or filtering.
    let dump_text = std::fs::read_to_string(&dump_path)
        .map_err(|error| format!("reading held-out {DUMP_VAR}: {error}"))?;
    let key = parse_relocation_key(&dump_text)?;
    let report = grade_references(&references, &key);

    println!("held-out key opened after recovery: yes");
    println!("key support: {}", key.support().description());
    println!(
        "key inventory: sections={} function_rows={} distinct_function_symbols={}",
        key.section_count(),
        key.function_rows(),
        key.distinct_function_symbols()
    );
    println!(
        "HEADLINE recovered={} correct={} wrong={} misclassification_rate={:.6}%",
        report.recovered,
        report.correct,
        report.wrong,
        100.0 * report.misclassification_rate()
    );
    println!("per reference kind (correct/wrong):");
    for kind in ReferenceKind::ALL {
        let (correct, wrong) = report.per_kind.get(&kind).copied().unwrap_or((0, 0));
        println!(
            "  {:<20} recovered={:>6} correct={:>6} wrong={:>6}",
            kind.label(),
            correct + wrong,
            correct,
            wrong
        );
    }
    println!("wrong reasons:");
    for reason in WrongReason::ALL {
        println!(
            "  {:<32} {}",
            reason.label(),
            report.wrong_reasons.get(&reason).copied().unwrap_or(0)
        );
    }

    let wrong_samples: Vec<_> = report
        .references
        .iter()
        .filter_map(|graded| match graded.grade {
            ReferenceGrade::Wrong(reason) => Some((&graded.reference, reason)),
            ReferenceGrade::Correct => None,
        })
        .take(16)
        .collect();
    println!("wrong samples (bounded={}):", wrong_samples.len());
    for (reference, reason) in wrong_samples {
        println!(
            "  bank={} referrer={:#010x} target={:#010x} kind={} reason={}",
            reference.bank,
            reference.referrer,
            reference.target,
            reference.kind.label(),
            reason.label()
        );
    }
    println!(
        "frontier: OoT VROM/compressed overlays are not materializable by snapshot V1, so this is resident physical-bank coverage; data references cannot be positively matched because dump.toml has no data-symbol or relocation records"
    );
    Ok(())
}

fn physical_banks(facts: &FactDb) -> Result<Vec<PhysicalBank>, String> {
    let mut banks = Vec::new();
    for fact in facts.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_space,
            rom_start,
            rom_end,
            va_start,
            va_end,
        } = fact
        else {
            unreachable!("proven_rom_mappings returned a non-mapping fact")
        };
        if *rom_space != RomAddressSpace::Physical {
            continue;
        }
        if rom_end.checked_sub(*rom_start) != va_end.checked_sub(*va_start) {
            return Err(format!(
                "physical bank {bank} has unequal ROM and VA extents"
            ));
        }
        banks.push(PhysicalBank {
            bank: bank.clone(),
            rom_start: *rom_start,
            rom_end: *rom_end,
            va_start: *va_start,
            va_end: *va_end,
        });
    }
    banks.sort_by(|left, right| left.bank.cmp(&right.bank));
    Ok(banks)
}

fn callable_roots(facts: &FactDb, bank: &PhysicalBank) -> Vec<u32> {
    let mut roots: BTreeSet<u32> = facts
        .proven_function_entries(&bank.bank)
        .into_iter()
        .collect();
    for fact in facts.facts() {
        let Fact::FunctionEntryClaim {
            target,
            evidence,
            proposed_state,
            ..
        } = fact
        else {
            continue;
        };
        if target.bank == bank.bank
            && target.pc >= bank.va_start
            && target.pc < bank.va_end
            && matches!(
                proposed_state,
                ProofState::Candidate | ProofState::Supported | ProofState::Proven
            )
            && matches!(
                evidence,
                FunctionEntryEvidence::DirectJal { .. }
                    | FunctionEntryEvidence::ResolvedJalr { .. }
                    | FunctionEntryEvidence::ExhaustiveIndirectCall { .. }
                    | FunctionEntryEvidence::TableEntry { .. }
                    | FunctionEntryEvidence::HandlerTablePointer { .. }
            )
        {
            roots.insert(target.pc);
        }
    }
    roots.into_iter().collect()
}

fn recover_references(
    rom: &NormalizedRom,
    physical: &[PhysicalBank],
    snapshots: &[fn64_discover::snapshot::ProgramSnapshotV1],
) -> Result<(Vec<RecoveredReference>, String), String> {
    let physical_by_name: BTreeMap<_, _> = physical
        .iter()
        .map(|bank| (bank.bank.as_str(), bank))
        .collect();
    let mut references = BTreeSet::new();
    let mut proven_sites: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();

    for snapshot in snapshots {
        for bank_snapshot in &snapshot.banks {
            let bank_name = &bank_snapshot.input.bank;
            let Some(physical_bank) = physical_by_name.get(bank_name.as_str()) else {
                return Err(format!("snapshot bank {bank_name} has no physical input"));
            };
            let sites = proven_sites.entry(bank_name.clone()).or_default();
            for assessment in &bank_snapshot.block_proof.assessments {
                let BlockAssessment::Proven { block } = assessment else {
                    continue;
                };
                for pc in (block.start_va..block.end_va).step_by(4) {
                    sites.insert(pc);
                }
                let fn64_discover::facts::BankBackingSpanV1::RomAffine {
                    rom_space: fn64_discover::facts::RomAddressSpace::Physical,
                    rom_start,
                    rom_end,
                } = &block.backing
                else {
                    return Err(format!(
                        "relocation-accuracy gate requires physical affine backing for proven block {bank_name}:0x{:08x}",
                        block.start_va
                    ));
                };
                let bytes = rom
                    .bytes
                    .get(*rom_start as usize..*rom_end as usize)
                    .ok_or_else(|| {
                        format!(
                            "proven block [0x{:08x},0x{:08x}) has backing outside ROM",
                            block.start_va, block.end_va
                        )
                    })?;
                for target_bank in physical {
                    let target_len = target_bank.va_end - target_bank.va_start;
                    for site in
                        scan_global_refs(bytes, block.start_va, target_bank.va_start, target_len)
                    {
                        let kind = match site.kind {
                            RefKind::Load { .. } => ReferenceKind::HiLoLoad,
                            RefKind::Store { .. } => ReferenceKind::HiLoStore,
                            RefKind::Address => ReferenceKind::HiLoAddress,
                        };
                        references
                            .insert(RecoveredReference::new(bank_name, site.pc, site.addr, kind));
                    }
                }
            }

            for &(source, target) in &bank_snapshot.closure.cfg.direct_calls {
                if sites.contains(&source) {
                    references.insert(RecoveredReference::new(
                        bank_name,
                        source,
                        target,
                        ReferenceKind::DirectCall,
                    ));
                }
            }
            for resolution in &bank_snapshot.closure.indirect {
                if !sites.contains(&resolution.site_pc)
                    || resolution.state != IndirectProofState::Exhaustive
                    || resolution.kind != Some(IndirectResolutionKind::JumpTable)
                {
                    continue;
                }
                for &target in &resolution.targets {
                    references.insert(RecoveredReference::new(
                        bank_name,
                        resolution.site_pc,
                        target,
                        ReferenceKind::JumpTableTarget,
                    ));
                }
                for &source in &resolution.memory_sources {
                    references.insert(RecoveredReference::new(
                        bank_name,
                        resolution.site_pc,
                        source,
                        ReferenceKind::JumpTableStorage,
                    ));
                }
            }

            if physical_bank.va_end <= physical_bank.va_start {
                return Err(format!("physical bank {bank_name} is empty"));
            }
        }
    }

    let gp_frontier = recover_gp_references(rom, physical, &proven_sites, &mut references)?;
    Ok((references.into_iter().collect(), gp_frontier))
}

fn recover_gp_references(
    rom: &NormalizedRom,
    physical: &[PhysicalBank],
    proven_sites: &BTreeMap<String, BTreeSet<u32>>,
    references: &mut BTreeSet<RecoveredReference>,
) -> Result<String, String> {
    let Some(boot) = physical.iter().find(|bank| bank.bank == banks::BOOT_BANK) else {
        return Ok("no physical boot bank; zero GP references emitted".to_string());
    };
    let (bss_start, bss_end) = match resident_bss(rom) {
        Ok(bounds) => bounds,
        Err(error) => {
            return Ok(format!("Open ({error}); zero GP references emitted"));
        }
    };
    if bss_start <= boot.va_start || bss_end <= bss_start {
        return Err(format!(
            "recovered BSS [0x{bss_start:08x},0x{bss_end:08x}) is invalid for boot VA 0x{:08x}",
            boot.va_start
        ));
    }
    let initialized_len = (bss_start - boot.va_start).min(boot.rom_end - boot.rom_start) as usize;
    let bytes = rom
        .bytes
        .get(boot.rom_start as usize..boot.rom_start as usize + initialized_len)
        .ok_or("resident initialized image lies outside ROM")?;
    let analysis = gp_base::analyze(
        bytes,
        boot.va_start,
        DataRange {
            start: boot.va_start,
            end: bss_end,
        },
    );
    let proven = proven_sites.get(&boot.bank);
    match &analysis.outcome {
        GpBaseOutcome::Admitted {
            base,
            explained,
            total,
            out_of_range,
            ..
        } => {
            let mut emitted = 0usize;
            for site in &analysis.sites {
                if !proven.is_some_and(|sites| sites.contains(&site.pc)) {
                    continue;
                }
                let kind = match site.kind {
                    GpAccessKind::Load { .. } => ReferenceKind::GpLoad,
                    GpAccessKind::Store { .. } => ReferenceKind::GpStore,
                    GpAccessKind::Address => ReferenceKind::GpAddress,
                };
                emitted += usize::from(references.insert(RecoveredReference::new(
                    &boot.bank, site.pc, site.addr, kind,
                )));
            }
            Ok(format!(
                "admitted base=0x{base:08x}, explained={explained}/{total}, out_of_range={out_of_range}, proven-site references emitted={emitted}"
            ))
        }
        GpBaseOutcome::Open { contenders } => Ok(format!(
            "Open ({} contenders); zero GP references emitted",
            contenders.len()
        )),
        GpBaseOutcome::NoGpAccesses => {
            Ok("no GP-relative accesses; zero GP references emitted".to_string())
        }
    }
}

fn resident_bss(rom: &NormalizedRom) -> Result<(u32, u32), String> {
    let words: Vec<u32> = rom
        .bytes
        .get(0x1000..)
        .ok_or("ROM has no hardware boot-copy source")?
        .chunks_exact(4)
        .take(1024)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte word")))
        .collect();
    for word_count in [16usize, 32, 64, 128, 256, 512, 1024] {
        let result = recognize_entry_stub_any(
            &words[..word_count.min(words.len())],
            VirtualAddress::new(rom.header.entry_point),
        );
        match result {
            Ok(RecognizedEntryStub::Countdown(observation)) => {
                return Ok((
                    observation.zero_fill.start.get(),
                    observation.zero_fill.end_exclusive.get(),
                ));
            }
            Ok(RecognizedEntryStub::EndPointer(observation)) => {
                return Ok((
                    observation.zero_fill.start.get(),
                    observation.zero_fill.end_exclusive.get(),
                ));
            }
            Err(_) => {}
        }
    }
    Err("no accepted entry stub in 1024-word budget; cannot bound GP data range".to_string())
}
