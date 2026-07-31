//! Exact-owner admission gate for mechanically recovered NWXE overlay banks.
//!
//! Discovery, CFG closure, executable-range proof, partitioning, and owner
//! proof consume ROM bytes and discovery facts only. The NWXE dump is opened
//! after all four overlay snapshots are complete and is used solely to grade
//! admitted extents.

use fn64_discover::banks::{self, BankNamePattern};
use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::facts::{FunctionEntryEvidence, ProofState, RomAddressSpace};
use fn64_discover::overlay_regions::SearchConfig;
use fn64_discover::owner_proof::OwnerAssessment;
use fn64_discover::snapshot::{
    compose_materialized_banks_v1, MaterializedBankInput, OwnerBlockerKind, OwnerBlockerSummary,
    ProgramSnapshotV1,
};
use fn64_discover::{
    required_env_path, run_discovery_with_recovered_overlay_regions, Fact, FactDb,
    RecoveredOverlayInput,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

const EXPECTED_OVERLAY_BANKS: usize = 4;

#[derive(Debug, Clone)]
struct OverlayMapping {
    bank: String,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

#[derive(Debug)]
struct CompletedOverlay {
    mapping: OverlayMapping,
    seed_roots: usize,
    proven_roots: usize,
    snapshot: ProgramSnapshotV1,
}

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct KeyExtent {
    name: String,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

#[derive(Debug, Default)]
struct ExtentGrade {
    admitted: usize,
    exact: usize,
    interior_direct_calls: usize,
    wrong: Vec<String>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_owners_overlays: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rom_path = required_env_path("FN64_DISCOVER_NWXE_ROM", "the NWXE .z64")?;
    let dump_path =
        required_env_path("FN64_DISCOVER_NWXE_DUMP", "the NWXE grading-only dump.toml")?;
    let rom_bytes =
        std::fs::read(&rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;

    let search = SearchConfig::aki_family();
    let input = RecoveredOverlayInput {
        min_mapped_regions: search.min_records,
        search,
        delta_vote: DeltaVoteConfig::default(),
        table_name: "recovered_overlay_descriptors".to_string(),
        bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    let (rom, facts, recovery) = run_discovery_with_recovered_overlay_regions(&rom_bytes, &input)
        .map_err(|error| error.to_string())?;
    let admitted_tables = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .count();
    if admitted_tables != 1 {
        return Err(format!(
            "expected exactly one admitted recovered table, got {admitted_tables}"
        ));
    }

    let mappings = overlay_mappings(&facts)?;
    if mappings.len() != EXPECTED_OVERLAY_BANKS {
        return Err(format!(
            "expected {EXPECTED_OVERLAY_BANKS} proven physical overlay banks, got {}",
            mappings.len()
        ));
    }

    // The resident boot bank is composed alongside the overlays purely as a
    // cross-bank authority SOURCE: a direct `jal` in proven boot code whose
    // target lands inside an overlay's proven VA range authorizes that overlay
    // entry (the same authority rule a same-bank direct call already confers).
    // Sibling overlays are sources for one another symmetrically. Boot's own
    // owner proof is discarded; it never enters overlay grading.
    let boot = boot_mapping(&facts)?;

    // Materialize every bank's bytes and roots first; `MaterializedBankInput`
    // borrows both, so they must outlive the composition call.
    let mut bank_bytes: Vec<&[u8]> = Vec::with_capacity(mappings.len() + 1);
    let mut bank_roots: Vec<Vec<u32>> = Vec::with_capacity(mappings.len() + 1);
    let mut proven_root_counts: Vec<usize> = Vec::with_capacity(mappings.len() + 1);

    // Index 0 is boot; indices 1.. are the overlays, in `mappings` order.
    for mapping in std::iter::once(&boot).chain(mappings.iter()) {
        let bytes = rom
            .bytes
            .get(mapping.rom_start as usize..mapping.rom_end as usize)
            .ok_or_else(|| {
                format!(
                    "{} ROM interval [0x{:x},0x{:x}) is outside the normalized image",
                    mapping.bank, mapping.rom_start, mapping.rom_end
                )
            })?;
        bank_bytes.push(bytes);
        bank_roots.push(callable_roots(&facts, mapping));
        proven_root_counts.push(facts.proven_function_entries(&mapping.bank).len());
    }

    let inputs: Vec<MaterializedBankInput> = std::iter::once(&boot)
        .chain(mappings.iter())
        .enumerate()
        .map(|(index, mapping)| MaterializedBankInput {
            bank: &mapping.bank,
            va_start: mapping.va_start,
            bytes: bank_bytes[index],
            seed_roots: &bank_roots[index],
        })
        .collect();

    let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs)
        .map_err(|error| format!("composing banks: {error}"))?;

    // Drop the boot snapshot (index 0); grade only the overlays.
    let mut completed = Vec::with_capacity(mappings.len());
    for (index, mapping) in mappings.iter().enumerate() {
        completed.push(CompletedOverlay {
            mapping: mapping.clone(),
            seed_roots: bank_roots[index + 1].len(),
            proven_roots: proven_root_counts[index + 1],
            snapshot: snapshots[index + 1].clone(),
        });
    }

    // Held-out boundary: every discovery and proof pass above is complete.
    // Nothing parsed from the dump below can become a root, fact, range, or
    // owner; it can only reject an admitted extent that disagrees with the key.
    let dump_text = std::fs::read_to_string(&dump_path)
        .map_err(|error| format!("reading {dump_path}: {error}"))?;
    let key = parse_key_extents(&dump_text)?;

    println!("gate_owners_overlays: NWXE recovered-overlay exact owners");
    println!("ROM SHA-256 {}", rom.sha256);
    println!(
        "recovery: raw_tables={} admitted_tables={} proven_overlay_banks={}",
        recovery.candidate_tables.len(),
        admitted_tables,
        completed.len()
    );

    let mut total_exact = 0usize;
    let mut total_wrong = 0usize;
    let mut total_histogram = BTreeMap::<OwnerBlockerKind, (u64, u64, u64)>::new();
    for overlay in &completed {
        let bank_snapshot = overlay
            .snapshot
            .banks
            .first()
            .ok_or_else(|| format!("{} snapshot contains no bank", overlay.mapping.bank))?;
        let exact = bank_snapshot
            .owner_proof
            .assessments
            .iter()
            .filter(|assessment| matches!(assessment, OwnerAssessment::Proven { .. }))
            .count();
        let executable_bytes: u64 = overlay
            .snapshot
            .facts
            .proven_executable_ranges(&overlay.mapping.bank)
            .into_iter()
            .map(|(start, end)| u64::from(end - start))
            .sum();
        let bank_key: Vec<_> = key
            .iter()
            .filter(|extent| extent_in_mapping(extent, &overlay.mapping))
            .cloned()
            .collect();
        if bank_key.is_empty() {
            return Err(format!(
                "grading key contains no functions for {} [ROM 0x{:x}..0x{:x}, VA 0x{:08x}..0x{:08x})",
                overlay.mapping.bank,
                overlay.mapping.rom_start,
                overlay.mapping.rom_end,
                overlay.mapping.va_start,
                overlay.mapping.va_end
            ));
        }
        let grade = grade_extents(overlay, &bank_key);
        total_exact += exact;
        total_wrong += grade.wrong.len();
        merge_histogram(&mut total_histogram, &bank_snapshot.blocker_histogram);

        println!(
            "{}: ROM=[0x{:08x},0x{:08x}) VA=[0x{:08x},0x{:08x}) roots={} (proven={}) reached_blocks={} proven_executable_bytes={} exact_owners={} key_functions={}",
            overlay.mapping.bank,
            overlay.mapping.rom_start,
            overlay.mapping.rom_end,
            overlay.mapping.va_start,
            overlay.mapping.va_end,
            overlay.seed_roots,
            overlay.proven_roots,
            bank_snapshot.block_proof.proven_blocks,
            executable_bytes,
            exact,
            bank_key.len()
        );
        println!(
            "  owner_blockers={}",
            serde_json::to_string(&bank_snapshot.blocker_histogram)
                .map_err(|error| format!("serializing blocker histogram: {error}"))?
        );
        println!(
            "  extent_grade: admitted={} exact={} interior_direct_calls={} wrong={}",
            grade.admitted,
            grade.exact,
            grade.interior_direct_calls,
            grade.wrong.len()
        );
        if !grade.wrong.is_empty() {
            println!("  wrong_extents={:?}", grade.wrong);
        }
    }

    let total_histogram: Vec<OwnerBlockerSummary> = total_histogram
        .into_iter()
        .map(
            |(kind, (affected_assessments, occurrences, sole_blocker_assessments))| {
                OwnerBlockerSummary {
                    kind,
                    affected_assessments,
                    occurrences,
                    sole_blocker_assessments,
                }
            },
        )
        .collect();
    println!(
        "total: exact_owners={} wrong_extents={} owner_blockers={}",
        total_exact,
        total_wrong,
        serde_json::to_string(&total_histogram)
            .map_err(|error| format!("serializing total blocker histogram: {error}"))?
    );
    if total_wrong != 0 {
        return Err(format!(
            "hard extent gate requires wrong_extents=0, got {total_wrong}"
        ));
    }
    Ok(())
}

/// The proven resident boot bank mapping. It is composed only as a cross-bank
/// authority source, never graded.
fn boot_mapping(facts: &FactDb) -> Result<OverlayMapping, String> {
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
        if bank != banks::BOOT_BANK {
            continue;
        }
        if *rom_space != RomAddressSpace::Physical {
            return Err("resident boot bank is not physically ROM-backed".to_string());
        }
        if rom_end.checked_sub(*rom_start) != va_end.checked_sub(*va_start) {
            return Err("resident boot bank has unequal ROM and VA extents".to_string());
        }
        return Ok(OverlayMapping {
            bank: bank.clone(),
            rom_start: *rom_start,
            rom_end: *rom_end,
            va_start: *va_start,
            va_end: *va_end,
        });
    }
    Err("no proven resident boot bank mapping".to_string())
}

fn overlay_mappings(facts: &FactDb) -> Result<Vec<OverlayMapping>, String> {
    let mut mappings = Vec::new();
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
        if bank == banks::BOOT_BANK {
            continue;
        }
        if *rom_space != RomAddressSpace::Physical {
            return Err(format!("overlay {bank} is not physically ROM-backed"));
        }
        if rom_end.checked_sub(*rom_start) != va_end.checked_sub(*va_start) {
            return Err(format!("overlay {bank} has unequal ROM and VA extents"));
        }
        mappings.push(OverlayMapping {
            bank: bank.clone(),
            rom_start: *rom_start,
            rom_end: *rom_end,
            va_start: *va_start,
            va_end: *va_end,
        });
    }
    mappings.sort_by(|left, right| left.bank.cmp(&right.bank));
    Ok(mappings)
}

/// Traversal roots come only from ROM-derived discovery claims. Authoritative
/// roots are included by the composer itself; direct and exhaustive indirect
/// call claims add overlay-local callable traversal starts without changing
/// their proof state. Candidate prologues are deliberately not bulk-seeded.
fn callable_roots(facts: &FactDb, mapping: &OverlayMapping) -> Vec<u32> {
    let mut roots: BTreeSet<u32> = facts
        .proven_function_entries(&mapping.bank)
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
        if target.bank != mapping.bank
            || target.pc < mapping.va_start
            || target.pc >= mapping.va_end
            || !matches!(
                proposed_state,
                ProofState::Candidate | ProofState::Supported | ProofState::Proven
            )
            || !matches!(
                evidence,
                FunctionEntryEvidence::DirectJal { .. }
                    | FunctionEntryEvidence::ResolvedJalr { .. }
                    | FunctionEntryEvidence::ExhaustiveIndirectCall { .. }
                    | FunctionEntryEvidence::TableEntry { .. }
                    | FunctionEntryEvidence::HandlerTablePointer { .. }
            )
        {
            continue;
        }
        roots.insert(target.pc);
    }
    roots.into_iter().collect()
}

fn parse_key_extents(text: &str) -> Result<Vec<KeyExtent>, String> {
    let doc: SymbolsDoc = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut extents = Vec::new();
    for section in doc.section {
        for function in section.functions {
            let offset = function.vram.checked_sub(section.vram).ok_or_else(|| {
                format!(
                    "function {:?} precedes section {:?} in VRAM",
                    function.name, section.name
                )
            })?;
            let relative_end = offset
                .checked_add(function.size)
                .ok_or_else(|| format!("function {:?} extent overflows", function.name))?;
            if function.size == 0 || relative_end > section.size {
                return Err(format!(
                    "function {:?} lies outside section {:?}",
                    function.name, section.name
                ));
            }
            extents.push(KeyExtent {
                name: function.name,
                rom_start: section
                    .rom
                    .checked_add(offset)
                    .ok_or_else(|| "function ROM start overflows".to_string())?,
                rom_end: section
                    .rom
                    .checked_add(relative_end)
                    .ok_or_else(|| "function ROM end overflows".to_string())?,
                va_start: function.vram,
                va_end: function
                    .vram
                    .checked_add(function.size)
                    .ok_or_else(|| "function VA end overflows".to_string())?,
            });
        }
    }
    extents.sort();
    Ok(extents)
}

fn extent_in_mapping(extent: &KeyExtent, mapping: &OverlayMapping) -> bool {
    extent.rom_start >= mapping.rom_start
        && extent.rom_end <= mapping.rom_end
        && extent.va_start >= mapping.va_start
        && extent.va_end <= mapping.va_end
        && extent.rom_start - mapping.rom_start == extent.va_start - mapping.va_start
        && extent.rom_end - mapping.rom_start == extent.va_end - mapping.va_start
}

fn grade_extents(overlay: &CompletedOverlay, key: &[KeyExtent]) -> ExtentGrade {
    let bank_snapshot = &overlay.snapshot.banks[0];
    let mut direct_targets: BTreeSet<u32> = bank_snapshot
        .closure
        .cfg
        .direct_calls
        .iter()
        .map(|&(_, target)| target)
        .collect();
    direct_targets.extend(overlay.snapshot.facts.facts().iter().filter_map(|fact| {
        let Fact::DirectCall { source, target } = fact else {
            return None;
        };
        (source.bank != target.bank && target.bank == overlay.mapping.bank).then_some(target.pc)
    }));
    let mut grade = ExtentGrade::default();
    for assessment in &bank_snapshot.owner_proof.assessments {
        let OwnerAssessment::Proven { owner } = assessment else {
            continue;
        };
        grade.admitted += 1;
        if key.iter().any(|extent| {
            extent.rom_start == owner.rom_start
                && extent.rom_end == owner.rom_end
                && extent.va_start == owner.entry.pc
                && extent.va_end == owner.va_end
        }) {
            grade.exact += 1;
            continue;
        }
        if key.iter().any(|extent| {
            is_direct_call_subspan(
                owner.entry.pc,
                owner.va_end,
                owner.rom_start,
                owner.rom_end,
                extent,
                &direct_targets,
            )
        }) {
            grade.interior_direct_calls += 1;
            continue;
        }
        grade.wrong.push(format!(
            "{}:0x{:08x}..0x{:08x} / ROM 0x{:08x}..0x{:08x}",
            owner.entry.bank, owner.entry.pc, owner.va_end, owner.rom_start, owner.rom_end
        ));
    }
    grade
}

fn is_direct_call_subspan(
    owner_va_start: u32,
    owner_va_end: u32,
    owner_rom_start: u32,
    owner_rom_end: u32,
    extent: &KeyExtent,
    direct_targets: &BTreeSet<u32>,
) -> bool {
    owner_rom_start >= extent.rom_start
        && owner_rom_end <= extent.rom_end
        && owner_va_start >= extent.va_start
        && owner_va_end <= extent.va_end
        && (owner_rom_start > extent.rom_start || owner_rom_end < extent.rom_end)
        && (direct_targets.contains(&owner_va_start) || direct_targets.contains(&owner_va_end))
}

fn merge_histogram(
    total: &mut BTreeMap<OwnerBlockerKind, (u64, u64, u64)>,
    histogram: &[OwnerBlockerSummary],
) {
    for summary in histogram {
        let count = total.entry(summary.kind).or_default();
        count.0 += summary.affected_assessments;
        count.1 += summary.occurrences;
        count.2 += summary.sole_blocker_assessments;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_wrong_split_without_a_callable_boundary_is_rejected() {
        let key = KeyExtent {
            name: "synthetic".into(),
            rom_start: 0x1000,
            rom_end: 0x1100,
            va_start: 0x8000_0000,
            va_end: 0x8000_0100,
        };
        assert!(!is_direct_call_subspan(
            0x8000_0020,
            0x8000_0040,
            0x1020,
            0x1040,
            &key,
            &BTreeSet::new(),
        ));
    }
}
