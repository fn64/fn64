//! TEMPORARY measurement bin — NWXE miss taxonomy. DO NOT COMMIT.
//!
//! Replicates gate_decomp_functions' boot lane exactly (same roots, same
//! grading), asserts the same grade line, then interrogates discovery state
//! to classify every answer-key function that is not matched exact:
//!  - boot CFG/partition state (reached? ambiguous? data-referenced?)
//!  - composed-snapshot owner-proof assessments (Proven/Candidate/Ambiguous +
//!    blockers) for the boot bank
//!  - mechanical overlay recovery (aki_family) + composed owner-proof over
//!    the recovered overlay banks

use fn64_discover::banks::{self, materialize_rom_range, BankNamePattern};
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::grade_oot_functions::{grade_functions, AnswerFunction, JalTargets};
use fn64_discover::overlay_regions::SearchConfig;
use fn64_discover::owner_proof::{OwnerAssessment, OwnerBlocker};
use fn64_discover::partition::partition_with_authoritative_entries;
use fn64_discover::resolve::{build_cfg_closed_with_facts_and_claims, build_cfg_value_set_closed};
use fn64_discover::snapshot::{compose_materialized_banks_v1, MaterializedBankInput};
use fn64_discover::{
    required_env_path, run_discovery_with_recovered_overlay_regions,
    run_discovery_with_tables_and_request_dma, Fact, RecoveredOverlayInput,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

const HANDLER_TABLE_MIN_RUN: usize = 4;

#[derive(Deserialize)]
struct Dump {
    #[serde(rename = "section")]
    sections: Vec<Section>,
}

#[derive(Deserialize)]
struct Section {
    name: String,
    rom: u32,
    vram: u32,
    #[serde(default)]
    functions: Vec<Function>,
}

#[derive(Deserialize)]
struct Function {
    name: String,
    vram: u32,
    size: u32,
}

fn blocker_kind(blocker: &OwnerBlocker) -> String {
    let debug = format!("{blocker:?}");
    debug
        .split(|c: char| c == ' ' || c == '{' || c == '(')
        .next()
        .unwrap_or("?")
        .to_string()
}

fn assessment_desc(assessment: &OwnerAssessment) -> (String, String) {
    match assessment {
        OwnerAssessment::Proven { .. } => ("Proven".to_string(), String::new()),
        OwnerAssessment::Candidate { frontier } => {
            let mut kinds: Vec<String> = frontier.blockers.iter().map(blocker_kind).collect();
            kinds.sort();
            kinds.dedup();
            ("Candidate".to_string(), kinds.join("+"))
        }
        OwnerAssessment::Ambiguous { frontier } => {
            let mut kinds: Vec<String> = frontier.blockers.iter().map(blocker_kind).collect();
            kinds.sort();
            kinds.dedup();
            ("Ambiguous".to_string(), kinds.join("+"))
        }
    }
}

fn main() {
    let rom_path = required_env_path("FN64_DISCOVER_ROM", "the game's .z64").unwrap();
    let dump_path = required_env_path("FN64_DISCOVER_DUMP", "the answer-key dump.toml").unwrap();
    let rom_bytes = std::fs::read(&rom_path).unwrap();
    let dump_text = std::fs::read_to_string(&dump_path).unwrap();
    let dump: Dump = toml::from_str(&dump_text).unwrap();

    let (rom, db, _request_report) =
        run_discovery_with_tables_and_request_dma(&rom_bytes, None, &[], &[])
            .expect("baseline discovery");

    let boot = db
        .facts()
        .iter()
        .find_map(|fact| match fact {
            Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                ..
            } if bank == banks::BOOT_BANK => Some((*rom_start, *rom_end, *va_start)),
            _ => None,
        })
        .expect("boot bank not proven");
    let (boot_rom_start, boot_rom_end, boot_va_start) = boot;
    let boot_va_end = boot_va_start + (boot_rom_end - boot_rom_start);

    let mut answer: Vec<AnswerFunction> = Vec::new();
    let mut code_end = boot_va_start;
    for section in &dump.sections {
        if section.rom < boot_rom_start || section.rom >= boot_rom_end {
            continue;
        }
        if section.vram != boot_va_start + (section.rom - boot_rom_start) {
            continue;
        }
        for function in &section.functions {
            code_end = code_end.max(function.vram.saturating_add(function.size));
            answer.push(AnswerFunction {
                name: function.name.clone(),
                va_start: function.vram,
            });
        }
    }
    code_end = code_end.min(boot_va_end);
    answer.sort_by_key(|function| function.va_start);

    let code_len = (code_end - boot_va_start) as usize;
    let bank_bytes = &rom.bytes[boot_rom_start as usize..boot_rom_start as usize + code_len];
    let entrypoint = boot_va_start;
    println!(
        "boot bank: rom=0x{boot_rom_start:x}..0x{boot_rom_end:x} va_start=0x{boot_va_start:x} \
         answer_code_end=0x{code_end:x} answer_functions={}",
        answer.len()
    );

    // === Replicate gate root lanes exactly ===
    let mut roots = vec![entrypoint];
    let mut excused = vec![entrypoint];
    let full_boot_bytes = &rom.bytes[boot_rom_start as usize..boot_rom_end as usize];
    let mut baseline_seed_roots = db.proven_function_entries(banks::BOOT_BANK);
    baseline_seed_roots.push(entrypoint);
    baseline_seed_roots.sort_unstable();
    baseline_seed_roots.dedup();
    let baseline_authority = build_cfg_value_set_closed(
        banks::BOOT_BANK,
        full_boot_bytes,
        boot_va_start,
        &baseline_seed_roots,
    );
    let composition_inputs = [MaterializedBankInput {
        bank: banks::BOOT_BANK,
        va_start: boot_va_start,
        bytes: full_boot_bytes,
        seed_roots: &[],
    }];
    let composed_authority = compose_materialized_banks_v1(&rom, &db, &composition_inputs)
        .expect("boot composition");
    let composed_boot = composed_authority
        .iter()
        .find(|snapshot| snapshot.banks[0].input.bank == banks::BOOT_BANK)
        .expect("boot snapshot");
    let baseline_roots: BTreeSet<u32> = baseline_authority.cfg.proven_roots.into_iter().collect();
    let mut authority_delta: Vec<u32> = composed_boot.banks[0]
        .closure
        .cfg
        .proven_roots
        .iter()
        .copied()
        .filter(|root| !baseline_roots.contains(root) && *root < code_end)
        .collect();
    authority_delta.sort_unstable();
    authority_delta.dedup();
    for root in &authority_delta {
        roots.push(*root);
        excused.push(*root);
    }
    println!("composed seeding roots added={}", authority_delta.len());

    // in-bank flat jal
    let mut in_bank_jal = 0usize;
    for (index, chunk) in bank_bytes.chunks_exact(4).enumerate() {
        let word = u32::from_be_bytes(chunk.try_into().unwrap());
        if word >> 26 != 0x03 {
            continue;
        }
        let pc = boot_va_start.wrapping_add((index as u32) * 4);
        let target = (pc & 0xf000_0000) | ((word & 0x03ff_ffff) << 2);
        if target >= boot_va_start && target < code_end && !roots.contains(&target) {
            roots.push(target);
            excused.push(target);
            in_bank_jal += 1;
        }
    }
    println!("in-bank flat jal roots added={in_bank_jal}");

    // cross-bank jal (no-op for NWXE baseline: boot is the only proven bank)
    for fact in db.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_space,
            rom_start,
            rom_end,
            va_start,
            ..
        } = fact
        else {
            continue;
        };
        if bank == banks::BOOT_BANK {
            continue;
        }
        if let Ok(image) = materialize_rom_range(&rom, &db, *rom_space, *rom_start, *rom_end) {
            for (index, chunk) in image.bytes.chunks_exact(4).enumerate() {
                let word = u32::from_be_bytes(chunk.try_into().unwrap());
                if word >> 26 != 0x03 {
                    continue;
                }
                let pc = va_start.wrapping_add((index as u32) * 4);
                let target = (pc & 0xf000_0000) | ((word & 0x03ff_ffff) << 2);
                if target >= boot_va_start && target < code_end && !roots.contains(&target) {
                    roots.push(target);
                    excused.push(target);
                }
            }
        }
    }

    // stored code-pointer lane (keep candidate list for diagnostics)
    let mut hilo_candidates: BTreeSet<u32> = BTreeSet::new();
    {
        let images: Vec<Vec<u32>> = vec![bank_bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
            .collect()];
        let boot_words = &images[0];
        let mut stored_ptr = 0usize;
        let mut candidates: Vec<u32> = Vec::new();
        for words in &images {
            for (index, word) in words.iter().enumerate() {
                if word >> 26 != 0x0f {
                    continue;
                }
                let rt = (word >> 16) & 0x1f;
                let hi = (word & 0xffff) << 16;
                for follow in words.iter().skip(index + 1).take(8) {
                    let opcode = follow >> 26;
                    let rs = (follow >> 21) & 0x1f;
                    let rd = (follow >> 16) & 0x1f;
                    if (opcode == 0x09 || opcode == 0x0d) && rs == rt {
                        let low = follow & 0xffff;
                        let target = if opcode == 0x09 {
                            hi.wrapping_add((low as i16) as i32 as u32)
                        } else {
                            hi | low
                        };
                        if target % 4 == 0 && target > boot_va_start && target < code_end {
                            candidates.push(target);
                        }
                        break;
                    }
                    if rd == rt && opcode != 0x00 {
                        break;
                    }
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        hilo_candidates.extend(candidates.iter().copied());
        for target in candidates {
            let offset = ((target - boot_va_start) / 4) as usize;
            let Some(&first) = boot_words.get(offset) else {
                continue;
            };
            if !fn64_discover::sig_scan::plausible_function_boundary(boot_words, offset) {
                continue;
            }
            let opcode = first >> 26;
            let is_ascii = first
                .to_be_bytes()
                .iter()
                .all(|&b| (0x20..=0x7e).contains(&b));
            if first == 0
                || is_ascii
                || (0x10..=0x13).contains(&opcode)
                || opcode == 0x08
                || matches!(
                    fn64_cpu_runtime::decode(first),
                    fn64_cpu_runtime::Instruction::Unknown { .. }
                )
            {
                continue;
            }
            if !roots.contains(&target) {
                roots.push(target);
                stored_ptr += 1;
            }
        }
        println!("stored code-pointer roots added={stored_ptr}");
    }

    // handler tables
    let handler_entries: Vec<u32>;
    {
        let full_bank = &rom.bytes[boot_rom_start as usize..boot_rom_end as usize];
        let bank_words: Vec<u32> = full_bank
            .chunks_exact(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
            .collect();
        let text_words: Vec<u32> = bank_bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
            .collect();
        let handlers = fn64_discover::sig_scan::detect_handler_tables(
            &bank_words,
            &text_words,
            boot_va_start,
            code_end,
            HANDLER_TABLE_MIN_RUN,
        );
        handler_entries = handlers.clone();
        let mut added = 0usize;
        for entry in handlers {
            if !roots.contains(&entry) {
                roots.push(entry);
                excused.push(entry);
                added += 1;
            }
        }
        println!("handler-table roots added={added}");
    }

    // donor signature lane
    let mut donor_match_vas: BTreeSet<u32> = BTreeSet::new();
    if let (Ok(donor_roms), Ok(donor_dumps)) = (
        std::env::var("FN64_DISCOVER_SIG_DONOR_ROM"),
        std::env::var("FN64_DISCOVER_SIG_DONOR_DUMP"),
    ) {
        let rom_paths: Vec<&str> = donor_roms.split(';').collect();
        let dump_paths: Vec<&str> = donor_dumps.split(';').collect();
        let target_words: Vec<u32> = bank_bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
            .collect();
        for (donor_rom_path, donor_dump_path) in rom_paths.iter().zip(&dump_paths) {
            let donor_rom_bytes = std::fs::read(donor_rom_path).unwrap();
            let donor_dump_text = std::fs::read_to_string(donor_dump_path).unwrap();
            let donor_dump: Dump = toml::from_str(&donor_dump_text).unwrap();
            let (donor_rom, donor_db, _) =
                run_discovery_with_tables_and_request_dma(&donor_rom_bytes, None, &[], &[])
                    .expect("donor discovery");
            let mut donor_banks: Vec<(u32, Vec<u8>)> = Vec::new();
            for fact in donor_db.proven_rom_mappings() {
                let Fact::RomMapping {
                    rom_space,
                    rom_start,
                    rom_end,
                    va_start,
                    ..
                } = fact
                else {
                    continue;
                };
                if let Ok(image) =
                    materialize_rom_range(&donor_rom, &donor_db, *rom_space, *rom_start, *rom_end)
                {
                    donor_banks.push((*va_start, image.bytes));
                }
            }
            let mut donor_functions: Vec<(String, u32, u32)> = Vec::new();
            let mut donor_words: Vec<u32> = Vec::new();
            let push_body = |name: &str,
                             size: u32,
                             bytes: &[u8],
                             words: &mut Vec<u32>,
                             functions: &mut Vec<(String, u32, u32)>| {
                let synthetic_va = (words.len() * 4) as u32;
                words.extend(
                    bytes
                        .chunks_exact(4)
                        .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap())),
                );
                functions.push((name.to_string(), synthetic_va, size));
            };
            for section in &donor_dump.sections {
                for function in &section.functions {
                    let from_bank = donor_banks.iter().find_map(|(va_start, bytes)| {
                        let offset = function.vram.checked_sub(*va_start)? as usize;
                        bytes.get(offset..offset + function.size as usize)
                    });
                    if let Some(bytes) = from_bank {
                        push_body(
                            &function.name,
                            function.size,
                            bytes,
                            &mut donor_words,
                            &mut donor_functions,
                        );
                        continue;
                    }
                    let Some(delta) = function.vram.checked_sub(section.vram) else {
                        continue;
                    };
                    let start = (section.rom + delta) as usize;
                    if let Some(bytes) = donor_rom.bytes.get(start..start + function.size as usize)
                    {
                        push_body(
                            &function.name,
                            function.size,
                            bytes,
                            &mut donor_words,
                            &mut donor_functions,
                        );
                    }
                }
            }
            let signatures = fn64_discover::sig_scan::donor_signatures(
                &donor_functions,
                &donor_words,
                0,
                fn64_discover::sig_scan::MIN_SIGNATURE_WORDS,
            );
            let matches =
                fn64_discover::sig_scan::scan_signatures(&signatures, &target_words, boot_va_start);
            let mut seeded = 0usize;
            for m in &matches {
                donor_match_vas.insert(m.va);
                if !roots.contains(&m.va) {
                    roots.push(m.va);
                    seeded += 1;
                }
            }
            println!(
                "signature lane {donor_rom_path}: matches={} roots added={seeded}",
                matches.len()
            );
        }
    }

    let (cfg, resolved) = build_cfg_closed_with_facts_and_claims(
        &db,
        banks::BOOT_BANK,
        bank_bytes,
        boot_va_start,
        &roots,
        &BTreeMap::new(),
    );
    let authoritative_entries: BTreeSet<u32> =
        cfg.direct_calls.iter().map(|(_, target)| *target).collect();
    let part = partition_with_authoritative_entries(&cfg, &authoritative_entries);
    println!(
        "CFG: {} blocks, {} direct calls, {} indirect sites, {} bounded targets resolved",
        cfg.blocks.len(),
        cfg.direct_calls.len(),
        cfg.indirect_sites.len(),
        resolved.len()
    );
    let mut jal_targets: JalTargets = cfg.direct_calls.iter().map(|(_, target)| *target).collect();
    jal_targets.extend(excused.iter().copied());
    let report = grade_functions(&part.owners, &answer, code_end, &jal_targets);
    println!(
        "function-boundary grade: total={} matched_exact={} matched_coarse={} \
         interior_entries={} open={} wrong={}",
        report.total,
        report.matched_exact,
        report.matched_coarse,
        report.interior_entries,
        report.open,
        report.wrong
    );
    assert_eq!(report.wrong, 0, "wrong must be zero in baseline");

    // === Boot-bank per-function dump ===
    // Block interval index.
    let mut block_intervals: Vec<(u32, u32)> = cfg
        .blocks
        .iter()
        .map(|block| (block.start_va, block.end_va))
        .collect();
    block_intervals.sort_unstable();
    let block_start_index: BTreeMap<u32, u32> = cfg
        .blocks
        .iter()
        .map(|block| (block.start_va, block.end_va))
        .collect();
    let ambiguous_intervals: Vec<(u32, u32, usize)> = part
        .ambiguous
        .iter()
        .filter_map(|ambiguous| {
            block_start_index
                .get(&ambiguous.block_start)
                .map(|end| (ambiguous.block_start, *end, ambiguous.claimants.len()))
        })
        .collect();
    let owner_intervals: Vec<(u32, u32)> = part
        .owners
        .iter()
        .map(|owner| (owner.root_va, owner.extent_end))
        .collect();
    let contains = |intervals: &[(u32, u32)], va: u32| -> bool {
        let index = intervals.partition_point(|(start, _)| *start <= va);
        index > 0 && va < intervals[index - 1].1
    };

    // Data-reference scan over the full 1 MiB boot copy.
    let full_words: Vec<u32> = full_boot_bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
        .collect();
    let mut word_refs: BTreeMap<u32, Vec<u32>> = BTreeMap::new(); // target -> list of word indexes
    for (index, word) in full_words.iter().enumerate() {
        if *word >= boot_va_start && *word < code_end && *word % 4 == 0 {
            word_refs.entry(*word).or_default().push(index as u32);
        }
    }

    // Composed boot owner-proof assessment index.
    let mut composed_assessments: BTreeMap<u32, (String, String)> = BTreeMap::new();
    for assessment in &composed_boot.banks[0].owner_proof.assessments {
        let (state, blockers) = assessment_desc(assessment);
        composed_assessments.insert(assessment.entry().pc, (state, blockers));
    }
    let mut composed_state_histogram: BTreeMap<String, usize> = BTreeMap::new();
    let mut composed_blocker_histogram: BTreeMap<String, usize> = BTreeMap::new();
    for assessment in &composed_boot.banks[0].owner_proof.assessments {
        let (state, _) = assessment_desc(assessment);
        *composed_state_histogram.entry(state).or_default() += 1;
        match assessment {
            OwnerAssessment::Candidate { frontier } | OwnerAssessment::Ambiguous { frontier } => {
                let mut kinds: Vec<String> = frontier.blockers.iter().map(blocker_kind).collect();
                kinds.sort();
                kinds.dedup();
                for kind in kinds {
                    *composed_blocker_histogram.entry(kind).or_default() += 1;
                }
            }
            OwnerAssessment::Proven { .. } => {}
        }
    }
    println!("composed-boot owner-proof states: {composed_state_histogram:?}");
    println!("composed-boot blocker kinds (per assessment): {composed_blocker_histogram:?}");

    let out_dir = std::env::var("TAX_OUT").expect("TAX_OUT dir");
    let mut boot_rows = String::from(
        "name\tva\tgrade\treached_block\tin_ambiguous\tin_owner\ttext_refs\tdata_refs\t\
         hilo_candidate\thandler_entry\tdonor_match\tcomposed_state\tcomposed_blockers\n",
    );
    let roots_set: BTreeSet<u32> = roots.iter().copied().collect();
    let mut open_in_roots = 0usize;
    for (function, (name, grade)) in answer.iter().zip(&report.per_function) {
        assert_eq!(&function.name, name);
        let va = function.va_start;
        let grade_name = format!("{grade:?}");
        let grade_short = grade_name
            .split(|c: char| c == ' ' || c == '{')
            .next()
            .unwrap()
            .to_string();
        let reached = contains(&block_intervals, va);
        let ambiguous = ambiguous_intervals
            .iter()
            .any(|(start, end, _)| va >= *start && va < *end);
        let in_owner = contains(&owner_intervals, va);
        let refs = word_refs.get(&va);
        let text_refs = refs
            .map(|list| {
                list.iter()
                    .filter(|index| ((**index as usize) * 4) < code_len)
                    .count()
            })
            .unwrap_or(0);
        let data_refs = refs
            .map(|list| {
                list.iter()
                    .filter(|index| ((**index as usize) * 4) >= code_len)
                    .count()
            })
            .unwrap_or(0);
        let (composed_state, composed_blockers) = composed_assessments
            .get(&va)
            .cloned()
            .unwrap_or_default();
        if grade_short == "Open" && roots_set.contains(&va) {
            open_in_roots += 1;
        }
        boot_rows.push_str(&format!(
            "{name}\t0x{va:08x}\t{grade_short}\t{reached}\t{ambiguous}\t{in_owner}\t{text_refs}\t\
             {data_refs}\t{}\t{}\t{}\t{composed_state}\t{composed_blockers}\n",
            hilo_candidates.contains(&va),
            handler_entries.contains(&va),
            donor_match_vas.contains(&va),
        ));
    }
    println!("open functions whose VA is nonetheless in the root set: {open_in_roots}");
    std::fs::write(format!("{out_dir}/boot_functions.tsv"), boot_rows).unwrap();

    // Unresolved indirect sites in the graded CFG.
    let resolved_sites: BTreeSet<u32> = resolved.iter().map(|r| r.site_pc).collect();
    let mut unresolved_sites = 0usize;
    for site in &cfg.indirect_sites {
        if !resolved_sites.contains(&site.pc) {
            unresolved_sites += 1;
        }
    }
    println!(
        "graded-CFG indirect sites: total={} resolved={} unresolved={unresolved_sites}",
        cfg.indirect_sites.len(),
        resolved_sites.len()
    );

    // === Overlay lane: mechanical recovery + composed owner proof ===
    let search = SearchConfig::aki_family();
    let min_mapped_regions = search.min_records;
    let input = RecoveredOverlayInput {
        search,
        delta_vote: DeltaVoteConfig::default(),
        min_mapped_regions,
        table_name: "recovered_overlay_descriptors".to_string(),
        bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    let (rom2, db2, recovery) = run_discovery_with_recovered_overlay_regions(&rom_bytes, &input)
        .expect("recovered-overlay discovery");
    println!(
        "overlay recovery: raw_tables={} admitted={}",
        recovery.candidate_tables.len(),
        recovery
            .admissions
            .iter()
            .filter(|admission| admission.admitted)
            .count()
    );
    let mut overlay_mappings: Vec<(String, u32, u32, u32)> = Vec::new();
    for fact in db2.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_start,
            rom_end,
            va_start,
            ..
        } = fact
        else {
            continue;
        };
        println!(
            "proven bank: {bank} rom=0x{rom_start:x}..0x{rom_end:x} va=0x{va_start:x}..0x{:x}",
            va_start + (rom_end - rom_start)
        );
        if bank != banks::BOOT_BANK {
            overlay_mappings.push((bank.clone(), *rom_start, *rom_end, *va_start));
        }
    }

    // Compose boot + overlay banks over the recovered fact db.
    let boot2 = db2
        .facts()
        .iter()
        .find_map(|fact| match fact {
            Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                ..
            } if bank == banks::BOOT_BANK => Some((*rom_start, *rom_end, *va_start)),
            _ => None,
        })
        .expect("boot bank not proven (recovered run)");
    let full_boot2 = &rom2.bytes[boot2.0 as usize..boot2.1 as usize];
    let mut storage: Vec<(String, u32, Vec<u8>)> = vec![(
        banks::BOOT_BANK.to_string(),
        boot2.2,
        full_boot2.to_vec(),
    )];
    for (bank, rom_start, rom_end, va_start) in &overlay_mappings {
        let image = &rom2.bytes[*rom_start as usize..*rom_end as usize];
        storage.push((bank.clone(), *va_start, image.to_vec()));
        let _ = rom_end;
    }
    let inputs: Vec<MaterializedBankInput> = storage
        .iter()
        .map(|(bank, va_start, bytes)| MaterializedBankInput {
            bank,
            va_start: *va_start,
            bytes,
            seed_roots: &[],
        })
        .collect();
    let mut overlay_rows = String::from(
        "name\tsection\tva\tbank\tbank_proven\tassess_state\tassess_blockers\tproven_exact\n",
    );
    match compose_materialized_banks_v1(&rom2, &db2, &inputs) {
        Ok(snapshots) => {
            // Index assessments per bank.
            let mut per_bank: BTreeMap<String, BTreeMap<u32, (String, String)>> = BTreeMap::new();
            let mut per_bank_state_histograms: BTreeMap<String, BTreeMap<String, usize>> =
                BTreeMap::new();
            let mut per_bank_blockers: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
            let mut per_bank_proven_entries: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
            for snapshot in &snapshots {
                let bank_snapshot = &snapshot.banks[0];
                let bank = bank_snapshot.input.bank.clone();
                let mut index: BTreeMap<u32, (String, String)> = BTreeMap::new();
                for assessment in &bank_snapshot.owner_proof.assessments {
                    let (state, blockers) = assessment_desc(assessment);
                    if state == "Proven" {
                        per_bank_proven_entries
                            .entry(bank.clone())
                            .or_default()
                            .insert(assessment.entry().pc);
                    }
                    *per_bank_state_histograms
                        .entry(bank.clone())
                        .or_default()
                        .entry(state.clone())
                        .or_default() += 1;
                    match assessment {
                        OwnerAssessment::Candidate { frontier }
                        | OwnerAssessment::Ambiguous { frontier } => {
                            let mut kinds: Vec<String> =
                                frontier.blockers.iter().map(blocker_kind).collect();
                            kinds.sort();
                            kinds.dedup();
                            for kind in kinds {
                                *per_bank_blockers
                                    .entry(bank.clone())
                                    .or_default()
                                    .entry(kind)
                                    .or_default() += 1;
                            }
                        }
                        OwnerAssessment::Proven { .. } => {}
                    }
                    index.insert(assessment.entry().pc, (state, blockers));
                }
                per_bank.insert(bank, index);
            }
            for (bank, histogram) in &per_bank_state_histograms {
                println!("composed bank {bank}: states={histogram:?}");
            }
            for (bank, histogram) in &per_bank_blockers {
                println!("composed bank {bank}: blocker kinds={histogram:?}");
            }

            // Map each non-boot dump section to a proven overlay bank and
            // classify its functions.
            for section in &dump.sections {
                if section.rom >= boot_rom_start
                    && section.rom < boot_rom_end
                    && section.vram == boot_va_start + (section.rom - boot_rom_start)
                {
                    continue; // boot-affine, already graded
                }
                let matching = overlay_mappings.iter().find(|(_, rom_start, rom_end, va)| {
                    section.rom >= *rom_start
                        && section.rom < *rom_end
                        && section.vram == va + (section.rom - rom_start)
                });
                for function in &section.functions {
                    let (bank_name, proven) = match matching {
                        Some((bank, _, _, _)) => (bank.clone(), true),
                        None => (String::new(), false),
                    };
                    let (state, blockers) = matching
                        .and_then(|(bank, _, _, _)| {
                            per_bank.get(bank).and_then(|index| {
                                index.get(&function.vram).cloned()
                            })
                        })
                        .unwrap_or_default();
                    let proven_exact = matching
                        .map(|(bank, _, _, _)| {
                            per_bank_proven_entries
                                .get(bank)
                                .is_some_and(|set| set.contains(&function.vram))
                        })
                        .unwrap_or(false);
                    overlay_rows.push_str(&format!(
                        "{}\t{}\t0x{:08x}\t{bank_name}\t{proven}\t{state}\t{blockers}\t{proven_exact}\n",
                        function.name, section.name, function.vram
                    ));
                }
            }
        }
        Err(error) => {
            println!("multi-bank composition FAILED: {error:?}");
            for section in &dump.sections {
                if section.rom >= boot_rom_start
                    && section.rom < boot_rom_end
                    && section.vram == boot_va_start + (section.rom - boot_rom_start)
                {
                    continue;
                }
                let matching = overlay_mappings.iter().find(|(_, rom_start, rom_end, va)| {
                    section.rom >= *rom_start
                        && section.rom < *rom_end
                        && section.vram == va + (section.rom - rom_start)
                });
                for function in &section.functions {
                    let (bank_name, proven) = match matching {
                        Some((bank, _, _, _)) => (bank.clone(), true),
                        None => (String::new(), false),
                    };
                    overlay_rows.push_str(&format!(
                        "{}\t{}\t0x{:08x}\t{bank_name}\t{proven}\tCOMPOSE_ERROR\t\t false\n",
                        function.name, section.name, function.vram
                    ));
                }
            }
        }
    }
    std::fs::write(format!("{out_dir}/overlay_functions.tsv"), overlay_rows).unwrap();
    println!("tax_nwxe: wrote {out_dir}/boot_functions.tsv and overlay_functions.tsv");
}
