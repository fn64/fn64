//! Grading-only cross-ROM homology gate. Function extents from the source ROM
//! seed the search; only target executable intervals enter the matcher. Target
//! function extents are loaded after matching and used solely to grade whether
//! proposed addresses are exact entries.

use fn64_discover::evidence::EvidenceManifest;
use fn64_discover::facts::{Fact, FactDb, ProofState};
use fn64_discover::homology::{CodeRegion, HomologyConfig, HomologyResult, KnownFunction};
use fn64_discover::{normalize, run_discovery_with_manifest, NormalizedRom};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PhysicalEntry {
    rom: u32,
    vram: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_homology: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let nw4e_rom = required_path("FN64_DISCOVER_NW4E_ROM")?;
    let nw4e_dump = required_path("FN64_DISCOVER_NW4E_DUMP")?;
    let nwxe_rom = required_path("FN64_DISCOVER_NWXE_ROM")?;
    let nwxe_dump = required_path("FN64_DISCOVER_NWXE_DUMP")?;
    let nw4e_evidence = required_path("FN64_DISCOVER_NW4E_EVIDENCE")?;
    let nwxe_evidence = required_path("FN64_DISCOVER_NWXE_EVIDENCE")?;
    grade_direction(
        "NWXE -> NW4E",
        &nwxe_rom,
        &nwxe_dump,
        &nw4e_rom,
        &nw4e_dump,
        &nw4e_evidence,
    )?;
    grade_direction(
        "NW4E -> NWXE",
        &nw4e_rom,
        &nw4e_dump,
        &nwxe_rom,
        &nwxe_dump,
        &nwxe_evidence,
    )
}

fn required_path(variable: &str) -> Result<String, String> {
    std::env::var(variable).map_err(|_| format!("{variable} must name the required gate input"))
}

fn grade_direction(
    label: &str,
    source_rom_path: &str,
    source_dump_path: &str,
    target_rom_path: &str,
    target_dump_path: &str,
    target_evidence_path: &str,
) -> Result<(), String> {
    for path in [
        source_rom_path,
        source_dump_path,
        target_rom_path,
        target_dump_path,
        target_evidence_path,
    ] {
        if !Path::new(path).exists() {
            return Err(format!("required input {path} does not exist"));
        }
    }

    let source_bytes = read(source_rom_path)?;
    let source_rom = normalize(&source_bytes).map_err(|error| error.to_string())?;
    let source_doc = parse_dump(source_dump_path)?;
    let config = HomologyConfig::default();
    let (sources, skipped_sources) = known_functions(&source_rom, &source_doc, config)?;

    let target_bytes = read(target_rom_path)?;
    let evidence_text = std::fs::read_to_string(target_evidence_path)
        .map_err(|error| format!("reading {target_evidence_path}: {error}"))?;
    let evidence =
        EvidenceManifest::from_toml(&evidence_text).map_err(|error| error.to_string())?;
    let (target_rom, target_db) =
        run_discovery_with_manifest(&target_bytes, &evidence).map_err(|error| error.to_string())?;
    let target_regions = executable_regions(&target_rom, &target_db)?;

    let results =
        fn64_discover::homology::find_homology_candidates(&sources, &target_regions, config)
            .map_err(|error| format!("homology index rejected corpus: {error:?}"))?;

    let target_doc = parse_dump(target_dump_path)?;
    let target_entries = answer_entries(&target_doc)?;
    let mut proposed_entries = BTreeSet::new();
    let mut exact_entries = BTreeSet::new();
    let mut candidates = 0usize;
    let mut ambiguous = 0usize;
    let mut unmatched = 0usize;
    for result in results {
        match result {
            HomologyResult::Candidate(candidate) => {
                candidates += 1;
                let entry =
                    translate_target(&target_db, &candidate.target.bank, candidate.target.va)
                        .ok_or_else(|| {
                            format!(
                                "candidate {}:{:#010x} has no unique physical mapping",
                                candidate.target.bank, candidate.target.va
                            )
                        })?;
                proposed_entries.insert(entry);
                if target_entries.contains(&entry) {
                    exact_entries.insert(entry);
                }
            }
            HomologyResult::Ambiguous { .. } => ambiguous += 1,
            HomologyResult::Unmatched { .. } => unmatched += 1,
        }
    }

    let false_entries = proposed_entries.len().saturating_sub(exact_entries.len());
    let precision = if proposed_entries.is_empty() {
        0.0
    } else {
        exact_entries.len() as f64 / proposed_entries.len() as f64
    };
    let lower_bound_recall = exact_entries.len() as f64 / target_entries.len() as f64;
    println!("{label}:");
    println!(
        "  source eligible={} skipped_short_or_long={} target_regions={} target_functions={}",
        sources.len(),
        skipped_sources,
        target_regions.len(),
        target_entries.len()
    );
    println!(
        "  results candidates={} ambiguous={} unmatched={} unique_targets={} exact_entries={} false_entries={}",
        candidates,
        ambiguous,
        unmatched,
        proposed_entries.len(),
        exact_entries.len(),
        false_entries
    );
    println!(
        "  boundary precision={:.6}% target-entry recall lower bound={:.6}%\n",
        precision * 100.0,
        lower_bound_recall * 100.0
    );
    Ok(())
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("reading {path}: {error}"))
}

fn parse_dump(path: &str) -> Result<SymbolsDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("reading {path}: {error}"))?;
    toml::from_str(&text).map_err(|error| format!("parsing {path}: {error}"))
}

fn known_functions(
    rom: &NormalizedRom,
    doc: &SymbolsDoc,
    config: HomologyConfig,
) -> Result<(Vec<KnownFunction>, usize), String> {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for section in &doc.section {
        validate_section(rom, section)?;
        for function in &section.functions {
            if !function.size.is_multiple_of(4) {
                return Err(format!(
                    "function {:?} has non-word size {:#x}",
                    function.name, function.size
                ));
            }
            let words = function.size as usize / 4;
            if words < config.min_function_words || words > config.max_function_words {
                skipped += 1;
                continue;
            }
            let section_offset = function.vram.checked_sub(section.vram).ok_or_else(|| {
                format!(
                    "function {:?} precedes section {:?}",
                    function.name, section.name
                )
            })?;
            let function_end = section_offset
                .checked_add(function.size)
                .ok_or_else(|| format!("function {:?} extent overflows", function.name))?;
            if function_end > section.size {
                return Err(format!(
                    "function {:?} exceeds section {:?}",
                    function.name, section.name
                ));
            }
            let rom_start = section
                .rom
                .checked_add(section_offset)
                .ok_or_else(|| format!("function {:?} ROM offset overflows", function.name))?;
            let bytes = rom
                .bytes
                .get(rom_start as usize..(rom_start + function.size) as usize)
                .ok_or_else(|| format!("function {:?} exceeds ROM", function.name))?;
            out.push(KnownFunction {
                identity: format!("{}:{}:{rom_start:08x}", section.name, function.name),
                bank: section.name.clone(),
                va_start: function.vram,
                words: words_from_be(bytes),
            });
        }
    }
    Ok((out, skipped))
}

fn validate_section(rom: &NormalizedRom, section: &SectionDoc) -> Result<(), String> {
    if !section.size.is_multiple_of(4) || !section.rom.is_multiple_of(4) {
        return Err(format!("section {:?} is not word-aligned", section.name));
    }
    let end = section
        .rom
        .checked_add(section.size)
        .ok_or_else(|| format!("section {:?} extent overflows", section.name))?;
    if end as usize > rom.len() {
        return Err(format!("section {:?} exceeds ROM", section.name));
    }
    Ok(())
}

fn executable_regions(rom: &NormalizedRom, db: &FactDb) -> Result<Vec<CodeRegion>, String> {
    let mut regions = Vec::new();
    for fact in db.facts() {
        let Fact::ExecutableRange {
            bank,
            va_start,
            va_end,
        } = fact
        else {
            continue;
        };
        let subject = fn64_discover::facts::executable_range_subject(bank, *va_start, *va_end);
        if db
            .conclusion(&subject)
            .is_none_or(|conclusion| conclusion.state != ProofState::Proven)
        {
            continue;
        }
        let rom_start = translate_target(db, bank, *va_start)
            .ok_or_else(|| format!("executable range {subject} has no unique mapping"))?
            .rom;
        let len = va_end - va_start;
        let bytes = rom
            .bytes
            .get(rom_start as usize..(rom_start + len) as usize)
            .ok_or_else(|| format!("executable range {subject} exceeds ROM"))?;
        regions.push(CodeRegion {
            bank: bank.clone(),
            va_start: *va_start,
            words: words_from_be(bytes),
        });
    }
    Ok(regions)
}

fn answer_entries(doc: &SymbolsDoc) -> Result<BTreeSet<PhysicalEntry>, String> {
    let mut entries = BTreeSet::new();
    for section in &doc.section {
        for function in &section.functions {
            let offset = function.vram.checked_sub(section.vram).ok_or_else(|| {
                format!(
                    "function {:?} precedes section {:?}",
                    function.name, section.name
                )
            })?;
            entries.insert(PhysicalEntry {
                rom: section
                    .rom
                    .checked_add(offset)
                    .ok_or_else(|| format!("function {:?} ROM overflows", function.name))?,
                vram: function.vram,
            });
        }
    }
    Ok(entries)
}

fn translate_target(db: &FactDb, bank: &str, va: u32) -> Option<PhysicalEntry> {
    let matches: BTreeSet<_> = db
        .proven_rom_mappings()
        .into_iter()
        .filter_map(|fact| match fact {
            Fact::RomMapping {
                bank: fact_bank,
                rom_start,
                rom_end,
                va_start,
                va_end,
                ..
            } if fact_bank == bank && va >= *va_start && va < *va_end => {
                let rom = rom_start.checked_add(va - va_start)?;
                (rom < *rom_end).then_some(PhysicalEntry { rom, vram: va })
            }
            _ => None,
        })
        .collect();
    (matches.len() == 1).then(|| *matches.iter().next().expect("one mapping"))
}

fn words_from_be(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte chunk")))
        .collect()
}
