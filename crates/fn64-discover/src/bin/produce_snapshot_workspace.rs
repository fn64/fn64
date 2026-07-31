//! Produce a bounded, diagnostic snapshot workspace from one ROM.
//!
//! Game-derived bytes are published only into a caller-owned private
//! workspace outside Git. Fixed indexed names avoid turning bank names into
//! paths, and the manifest is published last as the completion marker.

use fn64_discover::facts::{
    function_entry_subject, load_image_table_record_subject, CandidateDetector,
    MappingAddressSpace, ProofState,
};
use fn64_discover::file_table::{VromMaterializationLimits, DEFAULT_MAX_DECODED_VROM_FILE_BYTES};
use fn64_discover::grade_candidates::{
    scoped_candidate_identities_v3, ScopedCandidateIdentitiesV3,
};
use fn64_discover::owner_proof::OwnerAssessment;
use fn64_discover::snapshot::{
    compose_materialized_bank_validated_v2, compose_materialized_banks_validated_v2_with_limits,
    MultiBankCompositionLimits, ProgramSnapshotV1, PROGRAM_SNAPSHOT_SCHEMA_V5,
};
use fn64_discover::snapshot_inputs::{
    prepare_snapshot_banks_with_limits, PrepareSnapshotBanksLimits,
};
use fn64_discover::tool_adapter::Sha256Digest;
use fn64_discover::workspace_artifacts::{publish_new, validate_output_path, validate_workspace};
use fn64_discover::{
    run_discovery_auto_with_limits, AutoDiscoveryLimits, DiscoveryStrategy, Fact, FactDb,
    RomAddressSpace, StrategyOutcome,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const MIB: u64 = 1024 * 1024;
const MAX_ROM_BYTES: u64 = 64 * MIB;
const MAX_BANKS: usize = 4096;
const MAX_SNAPSHOT_ARTIFACT_BYTES: usize = 128 * MIB as usize;
const MAX_AGGREGATE_SNAPSHOT_ARTIFACT_BYTES: u64 = 1024 * MIB;
const MAX_COLD_CANDIDATE_ARTIFACT_BYTES: usize = 64 * MIB as usize;
const MANIFEST_NAME: &str = "snapshot-workspace.json";
const COLD_CANDIDATES_NAME: &str = "cold-candidates.json";
const COMPOSITION_LIMITS: MultiBankCompositionLimits = MultiBankCompositionLimits {
    max_projected_fact_rows: 4_000_000,
    max_projected_fact_bytes: 256 * MIB,
    max_aggregate_materialized_bytes: 256 * MIB,
    max_cross_bank_authority_records: 1_048_576,
};
const PREPARATION_LIMITS: PrepareSnapshotBanksLimits = PrepareSnapshotBanksLimits {
    max_banks: MAX_BANKS,
    max_aggregate_rom_bytes: 256 * MIB,
    max_decoded_vrom_file_bytes: DEFAULT_MAX_DECODED_VROM_FILE_BYTES,
};
const DISCOVERY_LIMITS: AutoDiscoveryLimits = AutoDiscoveryLimits {
    vrom_materialization: VromMaterializationLimits {
        max_decoded_file_bytes: DEFAULT_MAX_DECODED_VROM_FILE_BYTES,
    },
};

#[derive(Serialize)]
struct WorkspaceManifest<'a> {
    schema: &'static str,
    schema_version: u32,
    state: WorkspaceState,
    open_reason: Option<&'static str>,
    normalized_rom_sha256: &'a str,
    discovery: DiscoveryReceipt<'a>,
    limits: LimitsReceipt,
    snapshot_wire: SnapshotWireReceipt,
    aggregate_snapshot_artifact_bytes: u64,
    rom_recompilation_complete: bool,
    remaining_recompilation_frontier: &'static str,
    intended_use: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection: Option<SelectionReceipt<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_training: Option<ColdTrainingReceipt>,
    banks: Vec<BankReceipt>,
}

#[derive(Serialize)]
struct ColdTrainingReceipt {
    schema_version: u32,
    algorithm: &'static str,
    answer_key_present: bool,
    candidate_artifact: &'static str,
    candidate_artifact_byte_length: usize,
    candidate_artifact_sha256: Sha256Digest,
    scoped_candidate_identities_v3_sha256: String,
}

enum OutputMode {
    GhidraFull,
    GhidraSelected(String),
    ColdTraining,
}

#[derive(Serialize)]
struct SelectionReceipt<'a> {
    mode: &'static str,
    requested_bank: &'a str,
    available_proven_bank_count: usize,
    cross_bank_authority: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum WorkspaceState {
    Composed,
    Open,
}

#[derive(Serialize)]
struct DiscoveryReceipt<'a> {
    selected: DiscoveryStrategy,
    outcomes: &'a [StrategyOutcome],
}

#[derive(Serialize)]
struct LimitsReceipt {
    max_rom_bytes: u64,
    max_banks: usize,
    max_snapshot_artifact_bytes: usize,
    max_aggregate_snapshot_artifact_bytes: u64,
    max_discovery_decoded_vrom_file_bytes: usize,
    max_preparation_decoded_vrom_file_bytes: usize,
    max_projected_fact_rows: u64,
    max_projected_fact_bytes: u64,
    max_aggregate_materialized_bytes: u64,
    max_cross_bank_authority_records: u64,
}

#[derive(Serialize)]
struct SnapshotWireReceipt {
    schema_version: u32,
    authority: &'static str,
    duplicates_fact_db_per_bank: bool,
    remaining_large_rom_frontier: &'static str,
}

#[derive(Serialize)]
struct BankReceipt {
    index: usize,
    bank: String,
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
    byte_length: usize,
    backing_evidence_fact_indices: Vec<usize>,
    bank_sha256: Sha256Digest,
    bank_artifact: String,
    snapshot_artifact: String,
    snapshot_artifact_byte_length: usize,
    snapshot_artifact_sha256: Sha256Digest,
    program_snapshot_sha256: Sha256Digest,
    ghidra_seeds: GhidraSeeds,
}

#[derive(Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum GhidraSeeds {
    DiscoveryOnly {
        role: &'static str,
    },
    BaseOnly {
        base_seed: u32,
        base_seed_role: &'static str,
    },
    Paired {
        base_seed: u32,
        base_seed_role: &'static str,
        snapshot_seed: u32,
        snapshot_seed_role: &'static str,
        snapshot_seed_assessment: &'static str,
    },
}

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1)) {
        eprintln!("produce-snapshot-workspace: {error}");
        std::process::exit(1);
    }
}

fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let first = args.next().ok_or_else(usage)?;
    let (mode, rom_path) = if first == OsString::from("--bank") {
        let bank = args.next().ok_or_else(usage)?;
        let bank = bank
            .into_string()
            .map_err(|_| "selected bank must be UTF-8".to_string())?;
        validate_bank_token(&bank)?;
        let rom = args.next().map(PathBuf::from).ok_or_else(usage)?;
        (OutputMode::GhidraSelected(bank), rom)
    } else if first == OsString::from("--training") {
        let rom = args.next().map(PathBuf::from).ok_or_else(usage)?;
        (OutputMode::ColdTraining, rom)
    } else {
        (OutputMode::GhidraFull, PathBuf::from(first))
    };
    let selected_bank = match &mode {
        OutputMode::GhidraSelected(bank) => Some(bank.as_str()),
        OutputMode::GhidraFull | OutputMode::ColdTraining => None,
    };
    let workspace_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }

    let workspace = validate_workspace(&workspace_path)?;
    require_clean_reserved_namespace(&workspace)?;
    let manifest_path = workspace.join(MANIFEST_NAME);
    validate_output_path(&workspace, &manifest_path)?;
    require_new(&manifest_path)?;
    let rom_bytes = read_bounded_regular(&rom_path, MAX_ROM_BYTES)?;
    let discovery = run_discovery_auto_with_limits(&rom_bytes, DISCOVERY_LIMITS)
        .map_err(|error| format!("normalizing/discovering ROM: {error:?}"))?;
    let cold_training_requested = matches!(mode, OutputMode::ColdTraining);

    if discovery.facts.proven_rom_mappings().is_empty() {
        if selected_bank.is_some() {
            return Err("selected bank is unavailable because discovery proved no banks".into());
        }
        let cold_training = cold_training_requested
            .then(|| publish_cold_candidates(&workspace, &discovery.facts, &[]))
            .transpose()?;
        let manifest = WorkspaceManifest {
            schema: "fn64.snapshot-workspace",
            schema_version: if cold_training.is_some() { 3 } else { 1 },
            state: WorkspaceState::Open,
            open_reason: Some("no_proven_banks"),
            normalized_rom_sha256: &discovery.rom.sha256,
            discovery: DiscoveryReceipt {
                selected: discovery.selected,
                outcomes: &discovery.outcomes,
            },
            limits: limits_receipt(),
            snapshot_wire: snapshot_wire_receipt(),
            aggregate_snapshot_artifact_bytes: 0,
            rom_recompilation_complete: false,
            remaining_recompilation_frontier: "proven_bank_and_callable_owner_closure",
            intended_use: if cold_training.is_some() {
                "sealed_cold_function_training_input"
            } else {
                "candidate_ghidra_only"
            },
            selection: None,
            cold_training,
            banks: Vec::new(),
        };
        publish_manifest(&manifest_path, &manifest)?;
        println!(
            "produce-snapshot-workspace: state=open banks=0 rom_sha256={}",
            discovery.rom.sha256
        );
        return Ok(());
    }

    let prepared =
        prepare_snapshot_banks_with_limits(&discovery.rom, &discovery.facts, PREPARATION_LIMITS)
            .map_err(|error| format!("preparing proven banks: {error}"))?;
    if prepared.banks().len() > MAX_BANKS {
        return Err(format!(
            "prepared bank count {} exceeds {MAX_BANKS}",
            prepared.banks().len()
        ));
    }
    let available_proven_bank_count = prepared.banks().len();
    let inputs = prepared.materialized_inputs();
    let (published_indices, composed) = if let Some(requested) = selected_bank {
        let matches: Vec<usize> = prepared
            .banks()
            .iter()
            .enumerate()
            .filter_map(|(index, bank)| (bank.bank == requested).then_some(index))
            .collect();
        let [index] = matches.as_slice() else {
            return Err(format!(
                "selected bank {requested:?} matched {} of {available_proven_bank_count} proven banks",
                matches.len()
            ));
        };
        let input = inputs
            .into_iter()
            .nth(*index)
            .expect("selected prepared bank has a materialized input");
        let composed =
            compose_materialized_bank_validated_v2(&discovery.rom, &discovery.facts, input)
                .map_err(|error| format!("composing selected bank: {error}"))?;
        (vec![*index], composed)
    } else {
        let composed = compose_materialized_banks_validated_v2_with_limits(
            &discovery.rom,
            &discovery.facts,
            &inputs,
            COMPOSITION_LIMITS,
        )
        .map_err(|error| format!("composing proven banks: {error}"))?;
        ((0..available_proven_bank_count).collect(), composed)
    };
    let published_banks: Vec<_> = published_indices
        .iter()
        .map(|index| &prepared.banks()[*index])
        .collect();
    if composed.snapshots().len() != published_banks.len() {
        return Err("composer returned a different snapshot count than prepared banks".into());
    }
    let cold_training = cold_training_requested
        .then(|| publish_cold_candidates(&workspace, &discovery.facts, composed.snapshots()))
        .transpose()?;

    // V2 stores one exact bank-indexed fact projection in every snapshot.
    // Count the compact wire before publishing bank zero so the aggregate
    // disk commitment is bounded as well as each in-memory serialization.
    let mut snapshot_lengths = Vec::with_capacity(composed.snapshots().len());
    let mut aggregate_snapshot_artifact_bytes = 0u64;
    for snapshot in composed.snapshots() {
        let json_bytes = count_serialized_bounded(snapshot, MAX_SNAPSHOT_ARTIFACT_BYTES - 1)?;
        let artifact_bytes = u64::try_from(json_bytes)
            .ok()
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or_else(|| "snapshot artifact length overflow".to_string())?;
        aggregate_snapshot_artifact_bytes = aggregate_snapshot_artifact_bytes
            .checked_add(artifact_bytes)
            .ok_or_else(|| "aggregate snapshot artifact length overflow".to_string())?;
        if aggregate_snapshot_artifact_bytes > MAX_AGGREGATE_SNAPSHOT_ARTIFACT_BYTES {
            return Err(format!(
                "aggregate snapshot artifacts require {aggregate_snapshot_artifact_bytes} bytes, exceeding {MAX_AGGREGATE_SNAPSHOT_ARTIFACT_BYTES}"
            ));
        }
        snapshot_lengths.push(json_bytes);
    }
    let backing_evidence: Vec<Vec<usize>> = published_banks
        .iter()
        .map(|bank| {
            validate_backing_evidence(
                &discovery.facts,
                bank.rom_space,
                bank.rom_start,
                bank.rom_end,
                &bank.backing_evidence,
            )
        })
        .collect::<Result<_, _>>()?;

    let mut destinations = Vec::with_capacity(published_banks.len() * 2);
    for index in 0..published_banks.len() {
        let bank_path = workspace.join(bank_artifact_name(index));
        let snapshot_path = workspace.join(snapshot_artifact_name(index));
        validate_output_path(&workspace, &bank_path)?;
        validate_output_path(&workspace, &snapshot_path)?;
        require_new(&bank_path)?;
        require_new(&snapshot_path)?;
        destinations.push((bank_path, snapshot_path));
    }

    let mut bank_receipts = Vec::with_capacity(published_banks.len());
    for (index, ((bank, snapshot), (bank_path, snapshot_path))) in published_banks
        .iter()
        .zip(composed.snapshots())
        .zip(destinations.iter())
        .enumerate()
    {
        if snapshot.banks.len() != 1 || snapshot.banks[0].input.bank != bank.bank {
            return Err(format!("snapshot {index} does not match prepared bank"));
        }
        let mut snapshot_bytes = serialize_bounded(snapshot, MAX_SNAPSHOT_ARTIFACT_BYTES - 1)?;
        if snapshot_bytes.len() != snapshot_lengths[index] {
            return Err(format!(
                "snapshot {index} changed length between preflight and serialization"
            ));
        }
        let program_snapshot_sha256 = program_snapshot_digest(&snapshot_bytes);
        snapshot_bytes.push(b'\n');
        let snapshot_artifact_sha256 = Sha256Digest::of(&snapshot_bytes);
        let bank_sha256 = Sha256Digest::of(&bank.bytes);

        publish_new(bank_path, &bank.bytes)?;
        publish_new(snapshot_path, &snapshot_bytes)?;
        bank_receipts.push(BankReceipt {
            index,
            bank: bank.bank.clone(),
            rom_space: bank.rom_space,
            rom_start: bank.rom_start,
            rom_end: bank.rom_end,
            va_start: bank.va_start,
            va_end: bank.va_end,
            byte_length: bank.bytes.len(),
            backing_evidence_fact_indices: backing_evidence[index].clone(),
            bank_sha256,
            bank_artifact: bank_artifact_name(index),
            snapshot_artifact: snapshot_artifact_name(index),
            snapshot_artifact_byte_length: snapshot_bytes.len(),
            snapshot_artifact_sha256,
            program_snapshot_sha256,
            ghidra_seeds: ghidra_seeds(snapshot),
        });
    }

    let manifest = WorkspaceManifest {
        schema: "fn64.snapshot-workspace",
        schema_version: match &mode {
            OutputMode::GhidraFull => 1,
            OutputMode::GhidraSelected(_) => 2,
            OutputMode::ColdTraining => 3,
        },
        state: WorkspaceState::Composed,
        open_reason: None,
        normalized_rom_sha256: &discovery.rom.sha256,
        discovery: DiscoveryReceipt {
            selected: discovery.selected,
            outcomes: &discovery.outcomes,
        },
        limits: limits_receipt(),
        snapshot_wire: snapshot_wire_receipt(),
        aggregate_snapshot_artifact_bytes,
        rom_recompilation_complete: false,
        remaining_recompilation_frontier: if selected_bank.is_some() {
            "unselected_banks_and_callable_owner_closure"
        } else {
            "proven_bank_and_callable_owner_closure"
        },
        intended_use: match &mode {
            OutputMode::GhidraFull => "candidate_ghidra_only",
            OutputMode::GhidraSelected(_) => "candidate_ghidra_single_bank_only",
            OutputMode::ColdTraining => "sealed_cold_function_training_input",
        },
        selection: selected_bank.map(|requested_bank| SelectionReceipt {
            mode: "single_bank",
            requested_bank,
            available_proven_bank_count,
            cross_bank_authority: false,
        }),
        cold_training,
        banks: bank_receipts,
    };
    publish_manifest(&manifest_path, &manifest)?;
    println!(
        "produce-snapshot-workspace: state=composed banks={} rom_sha256={}",
        published_banks.len(),
        discovery.rom.sha256
    );
    Ok(())
}

fn bank_artifact_name(index: usize) -> String {
    format!("bank-{index:06}.bin")
}

fn snapshot_artifact_name(index: usize) -> String {
    format!("bank-{index:06}.snapshot.json")
}

fn publish_cold_candidates(
    workspace: &Path,
    base_facts: &FactDb,
    snapshots: &[ProgramSnapshotV1],
) -> Result<ColdTrainingReceipt, String> {
    let path = workspace.join(COLD_CANDIDATES_NAME);
    validate_output_path(workspace, &path)?;
    require_new(&path)?;
    let identities =
        cold_candidate_identities(base_facts, snapshots.iter().map(|snapshot| &snapshot.facts))?;
    let scoped_candidate_identities_v3_sha256 = identities.digest_sha256();
    let mut bytes = serialize_bounded(&identities, MAX_COLD_CANDIDATE_ARTIFACT_BYTES - 1)?;
    bytes.push(b'\n');
    let candidate_artifact_sha256 = Sha256Digest::of(&bytes);
    let candidate_artifact_byte_length = bytes.len();
    publish_new(&path, &bytes)?;
    Ok(ColdTrainingReceipt {
        schema_version: 3,
        algorithm: "fn64.cold-function-training.v3",
        answer_key_present: false,
        candidate_artifact: COLD_CANDIDATES_NAME,
        candidate_artifact_byte_length,
        candidate_artifact_sha256,
        scoped_candidate_identities_v3_sha256,
    })
}

fn cold_candidate_identities<'a>(
    base_facts: &FactDb,
    snapshot_facts: impl IntoIterator<Item = &'a FactDb>,
) -> Result<ScopedCandidateIdentitiesV3, String> {
    let mut facts = base_facts.clone();
    let mut semantic_claims = Vec::new();
    for snapshot_facts in snapshot_facts {
        for fact in snapshot_facts.facts() {
            let Fact::FunctionEntryClaim {
                target,
                detector: CandidateDetector::SemanticCallableArgument,
                proposed_state: ProofState::Proven,
                ..
            } = fact
            else {
                continue;
            };
            let index = match facts.facts().iter().position(|existing| existing == fact) {
                Some(index) => index,
                None => facts.insert(fact.clone()),
            };
            semantic_claims.push((target.clone(), index));
        }
    }
    semantic_claims.sort();
    semantic_claims.dedup();
    for (target, index) in semantic_claims {
        let subject = function_entry_subject(&target);
        let mut justifications = facts
            .conclusion(&subject)
            .map(|conclusion| conclusion.justified_by.clone())
            .unwrap_or_default();
        justifications.push(index);
        justifications.sort_unstable();
        justifications.dedup();
        facts
            .conclude(
                subject,
                ProofState::Proven,
                justifications,
                "cold_semantic_callable_composition",
            )
            .map_err(|error| format!("merging semantic callable cold facts: {error}"))?;
    }
    Ok(scoped_candidate_identities_v3(&facts, |_| true))
}

fn validate_backing_evidence(
    facts: &FactDb,
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    indices: &[usize],
) -> Result<Vec<usize>, String> {
    if rom_space == RomAddressSpace::Physical {
        if !indices.is_empty() {
            return Err("physical bank unexpectedly carries VROM backing evidence".into());
        }
        return Ok(Vec::new());
    }
    let [index] = indices else {
        return Err("VROM bank must carry exactly one backing-evidence fact index".into());
    };
    let fact = facts
        .facts()
        .get(*index)
        .ok_or_else(|| "VROM backing-evidence fact index is out of range".to_string())?;
    let Fact::LoadImageTableRecord {
        table,
        index: record_index,
        source_space: MappingAddressSpace::VirtualRom,
        source_start,
        source_end,
        destination_space: MappingAddressSpace::PhysicalRom,
        ..
    } = fact
    else {
        return Err("VROM backing evidence does not name a VROM-to-physical record".into());
    };
    if rom_start < *source_start || rom_end > *source_end {
        return Err("VROM backing record does not contain the prepared bank interval".into());
    }
    if !facts
        .conclusion(&load_image_table_record_subject(table, *record_index))
        .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
    {
        return Err("VROM backing record is not Proven".into());
    }
    Ok(vec![*index])
}

fn ghidra_seeds(snapshot: &ProgramSnapshotV1) -> GhidraSeeds {
    let bank = &snapshot.banks[0];
    let assessments = &bank.owner_proof.assessments;
    let eligible_pc =
        |pc: u32| pc.is_multiple_of(4) && pc >= bank.input.va_start && pc < bank.input.va_end;
    let base_seed = assessments
        .iter()
        .filter_map(|assessment| match assessment {
            OwnerAssessment::Proven { owner } if eligible_pc(owner.entry.pc) => {
                Some(owner.entry.pc)
            }
            _ => None,
        })
        .min();
    let Some(base_seed) = base_seed else {
        return GhidraSeeds::DiscoveryOnly {
            role: "candidate_only",
        };
    };
    match assessments
        .iter()
        .filter(|assessment| {
            assessment.entry().pc != base_seed && eligible_pc(assessment.entry().pc)
        })
        .map(|assessment| {
            let state = match assessment {
                OwnerAssessment::Proven { .. } => "proven",
                OwnerAssessment::Candidate { .. } => "candidate",
                OwnerAssessment::Ambiguous { .. } => "ambiguous",
            };
            (assessment.entry().pc, state)
        })
        .min_by_key(|(pc, _)| *pc)
    {
        Some((snapshot_seed, snapshot_seed_assessment)) => GhidraSeeds::Paired {
            base_seed,
            base_seed_role: "proven_owner",
            snapshot_seed,
            snapshot_seed_role: "assessed_owner",
            snapshot_seed_assessment,
        },
        None => GhidraSeeds::BaseOnly {
            base_seed,
            base_seed_role: "proven_owner",
        },
    }
}

fn limits_receipt() -> LimitsReceipt {
    LimitsReceipt {
        max_rom_bytes: MAX_ROM_BYTES,
        max_banks: MAX_BANKS,
        max_snapshot_artifact_bytes: MAX_SNAPSHOT_ARTIFACT_BYTES,
        max_aggregate_snapshot_artifact_bytes: MAX_AGGREGATE_SNAPSHOT_ARTIFACT_BYTES,
        max_discovery_decoded_vrom_file_bytes: DISCOVERY_LIMITS
            .vrom_materialization
            .max_decoded_file_bytes,
        max_preparation_decoded_vrom_file_bytes: PREPARATION_LIMITS.max_decoded_vrom_file_bytes,
        max_projected_fact_rows: COMPOSITION_LIMITS.max_projected_fact_rows,
        max_projected_fact_bytes: COMPOSITION_LIMITS.max_projected_fact_bytes,
        max_aggregate_materialized_bytes: COMPOSITION_LIMITS.max_aggregate_materialized_bytes,
        max_cross_bank_authority_records: COMPOSITION_LIMITS.max_cross_bank_authority_records,
    }
}

fn snapshot_wire_receipt() -> SnapshotWireReceipt {
    SnapshotWireReceipt {
        schema_version: PROGRAM_SNAPSHOT_SCHEMA_V5,
        authority: "diagnostic_only",
        duplicates_fact_db_per_bank: false,
        remaining_large_rom_frontier: "streaming_v5",
    }
}

fn publish_manifest(path: &Path, manifest: &WorkspaceManifest<'_>) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(manifest)
        .map_err(|error| format!("serializing workspace manifest: {error}"))?;
    bytes.push(b'\n');
    publish_new(path, &bytes)
}

fn program_snapshot_digest(serialized: &[u8]) -> Sha256Digest {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.program-snapshot.v2\0");
    hasher.update(serialized);
    Sha256Digest(hasher.finalize().into())
}

fn serialize_bounded<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, String> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, value)
        .map_err(|error| format!("serializing bounded snapshot: {error}"))?;
    Ok(writer.bytes)
}

fn count_serialized_bounded<T: Serialize>(value: &T, limit: usize) -> Result<usize, String> {
    let mut writer = CountingWriter { bytes: 0, limit };
    serde_json::to_writer(&mut writer, value)
        .map_err(|error| format!("counting bounded snapshot: {error}"))?;
    Ok(writer.bytes)
}

struct CountingWriter {
    bytes: usize,
    limit: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self
            .bytes
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("snapshot artifact length overflow"))?;
        if next > self.limit {
            return Err(io::Error::other(format!(
                "snapshot artifact exceeds {} bytes",
                self.limit
            )));
        }
        self.bytes = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("snapshot artifact length overflow"))?;
        if next > self.limit {
            return Err(io::Error::other(format!(
                "snapshot artifact exceeds {} bytes",
                self.limit
            )));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn read_bounded_regular(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    read_bounded_regular_after_inspect(path, limit, || {})
}

fn read_bounded_regular_after_inspect(
    path: &Path,
    limit: u64,
    after_inspect: impl FnOnce(),
) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspecting ROM {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("ROM is not a regular file: {}", path.display()));
    }
    if metadata.len() > limit {
        return Err(format!("ROM exceeds {limit} bytes"));
    }
    after_inspect();
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(path)
        .map_err(|error| format!("opening ROM {}: {error}", path.display()))?;
    let opened_metadata = file
        .metadata()
        .map_err(|error| format!("inspecting opened ROM {}: {error}", path.display()))?;
    if !opened_metadata.is_file() {
        return Err(format!(
            "opened ROM is not a regular file: {}",
            path.display()
        ));
    }
    ensure_same_opened_file(&metadata, &opened_metadata)?;
    let mut bytes =
        Vec::with_capacity(usize::try_from(opened_metadata.len().min(limit)).unwrap_or(0));
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("reading ROM {}: {error}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!("ROM exceeds {limit} bytes while reading"));
    }
    Ok(bytes)
}

fn ensure_same_opened_file(initial: &fs::Metadata, opened: &fs::Metadata) -> Result<(), String> {
    #[cfg(unix)]
    if initial.dev() != opened.dev() || initial.ino() != opened.ino() {
        return Err("ROM path identity changed while opening".into());
    }
    Ok(())
}

fn require_new(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(format!("refusing to overwrite {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("inspecting output {}: {error}", path.display())),
    }
}

fn require_clean_reserved_namespace(workspace: &Path) -> Result<(), String> {
    for entry in
        fs::read_dir(workspace).map_err(|error| format!("reading workspace namespace: {error}"))?
    {
        let entry = entry.map_err(|error| format!("reading workspace entry: {error}"))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let reserved = name
            .strip_prefix("bank-")
            .and_then(|rest| {
                rest.strip_suffix(".bin")
                    .or_else(|| rest.strip_suffix(".snapshot.json"))
            })
            .is_some_and(|index| {
                index.len() == 6 && index.bytes().all(|byte| byte.is_ascii_digit())
            });
        if reserved || name == COLD_CANDIDATES_NAME {
            return Err(format!("reserved snapshot artifact already exists: {name}"));
        }
    }
    Ok(())
}

fn usage() -> String {
    "usage: produce_snapshot_workspace [--training | --bank PATH_FREE_BANK] ROM CANONICAL_MODE_0700_WORKSPACE".into()
}

fn validate_bank_token(bank: &str) -> Result<(), String> {
    if bank.is_empty()
        || bank == "."
        || bank == ".."
        || bank.len() > 128
        || !bank
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
    {
        return Err("selected bank must be a path-free ASCII token".into());
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use fn64_discover::facts::{BankAddr, FunctionEntryEvidence, SemanticCallableContract};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn cold_receipt_merges_composed_semantic_authority_once() {
        let mut base = FactDb::new();
        let mapping = base.insert(Fact::RomMapping {
            bank: "resident".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1040,
            va_start: 0x8000_0400,
            va_end: 0x8000_0440,
        });
        base.conclude("bank:resident", ProofState::Proven, vec![mapping], "test")
            .unwrap();
        let mut composed = base.clone();
        let target = BankAddr::new("resident", 0x8000_0420);
        let claim = composed.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::SemanticCallableArgument,
            evidence: FunctionEntryEvidence::SemanticCallableArgument {
                call_site: BankAddr::new("resident", 0x8000_0408),
                callee: BankAddr::new("resident", 0x8000_0410),
                pointer_register: 6,
                contract: SemanticCallableContract::OsCreateThread,
            },
            proposed_state: ProofState::Proven,
        });
        composed
            .conclude(
                function_entry_subject(&target),
                ProofState::Proven,
                vec![claim],
                "test",
            )
            .unwrap();

        let receipt = cold_candidate_identities(&base, [&composed, &composed]).unwrap();
        let semantic = receipt
            .per_detector
            .iter()
            .find(|row| row.detector == CandidateDetector::SemanticCallableArgument)
            .unwrap();
        assert_eq!(semantic.candidates.len(), 1);
        assert_eq!(semantic.provenance.len(), 1);
        assert_eq!(receipt.combined_candidates.len(), 1);
    }

    #[test]
    fn opened_file_identity_rejects_a_path_swap() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "fn64-producer-identity-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let first = directory.join("first");
        let second = directory.join("second");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        assert!(ensure_same_opened_file(
            &fs::metadata(&first).unwrap(),
            &fs::metadata(&second).unwrap()
        )
        .unwrap_err()
        .contains("identity changed"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn no_follow_open_rejects_swap_to_symlink_after_inspection() {
        use std::os::unix::fs::symlink;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "fn64-producer-nofollow-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("input.z64");
        let original = directory.join("original.z64");
        let replacement = directory.join("replacement.z64");
        fs::write(&path, b"original").unwrap();
        fs::write(&replacement, b"replacement").unwrap();

        let error = read_bounded_regular_after_inspect(&path, 1024, || {
            fs::rename(&path, &original).unwrap();
            symlink(&replacement, &path).unwrap();
        })
        .unwrap_err();
        assert!(error.contains("opening ROM"));
        assert!(!error.contains("identity changed"));
        fs::remove_dir_all(directory).unwrap();
    }
}
