//! `fn64-discover`: mechanical, zero-LLM N64 ROM function-metadata
//! recovery. This is fn64's recompiler *frontend* -- see
//! `docs/DISCOVER-DESIGN.md` for the full pipeline this crate implements
//! incrementally, phase by phase.
//!
//! # Discipline (non-negotiable, see the design doc's "Honest limit")
//!
//! Never emit a guessed symbol file. Every module here either:
//!
//! - reads a **hardware-fixed** fact directly (Phase 1 header parsing,
//!   Phase 2's boot-copy geometry), or
//! - proposes a **candidate** from a named detector and requires an
//!   explicit proof rule before promoting it, recording rejection with a
//!   reason when a candidate fails self-consistency (see [`banks`]), or
//! - accepts an **explicit, cited external claim** (e.g. a descriptor
//!   table's ROM location, from prior RE) as an input the caller must
//!   supply -- this crate never scans blind for "the" overlay table and
//!   calls that scan itself proof.
//!
//! The output of a full pipeline run is always three things: proven facts,
//! rejected candidates, and an explicit open/unresolved frontier -- never
//! only the first.
//!
//! # This crate's current scope
//!
//! - [`rom`]: Phase 1, ROM normalization + identity.
//! - [`facts`]: the monotonic fact database and discrete [`facts::ProofState`]
//!   values every conclusion in this crate is expressed in.
//! - [`banks`]: Phase 2, load-image/overlay discovery -- the boot copy
//!   (hardware-fixed, needs no scanning), descriptor-table-shaped overlay
//!   banks, and configurable ROM/VROM range tables (including file-table
//!   backed compressed overlay images).
//! - [`evidence`]: schema-versioned, normalized-ROM-bound external mapping
//!   and executable-range evidence, with provenance and range validation.
//! - [`coverage`]: distinct physical-ROM, logical-load, executable, bank, and
//!   entry-state coverage measures; it never collapses them into one number.
//! - [`regions`]: deterministic multi-scale code/data/pointer/statistical
//!   views and boundary candidates. Scores are never promoted by themselves.
//! - [`loaders`]: strict MIPS entry-stub and PI ROM-load observations, with
//!   typed address domains, explicit candidate/proven strength, and loud
//!   rejection of unsupported shapes.
//! - [`load_table_use`]: descriptor-free affine overlay-table recovery from
//!   immutable semantic load uses plus independently proven loop bounds.
//!   Consecutive observations without exact enumeration remain candidates.
//! - [`pi_dma`]: bounded static `osPiStartDma` and `osEPiStartDma`
//!   call-operand slicing. Their distinct ABIs have distinct result types;
//!   exact operands and unresolved blockers are typed separately, and static
//!   geometry remains candidate evidence until handle mapping and completion
//!   exist.
//! - [`harvest`]: Phase 3, parallel deterministic candidate providers for
//!   direct/resolved calls, classic and leaf prologues, and code-pointer
//!   entries exposed by already-discovered tables. Claims merge through
//!   [`facts::FactDb`] and retain detector-specific evidence.
//! - [`cfg`]: Phase 4, the delay-slot-aware MIPS-III CFG builder -- word
//!   classification (`proven_code`/`candidate_code`/`proven_data`/
//!   `candidate_data`/`conflict`/`unknown`), basic blocks, direct calls,
//!   tail transfers, and the open indirect-site frontier.
//! - [`partition`]: Phase 5, recursive-descent owner partitioning of a
//!   [`cfg::Cfg`]'s blocks from its proven roots -- one owner per block per
//!   bank, ambiguous claims and unowned blocks reported explicitly rather
//!   than resolved by guessing.
//! - [`owner_proof`]: the conservative Phase 5 proof boundary. Exact extents
//!   are a distinct type available only after entry authority, CFG shape,
//!   ROM backing, executable coverage, incoming edges, and indirect closure
//!   all pass; every other result remains candidate or ambiguous.
//! - [`resolve`]: Phase 6 bounded value-set closure -- HI/LO and GP-relative
//!   address construction, exact register/memory propagation, dominating
//!   switch bounds, exhaustive jump tables, and fixed-point CFG feedback.
//!   Computed-jump table entries remain intra-owner code successors; only
//!   exhaustive computed calls become callable roots. Every unresolved site
//!   is retained as bounded/open evidence in the fact database.
//! - [`delta_vote`]: mapping inference for a code region with an unknown VA
//!   base -- lui-histogram-narrowed delta hypotheses scored by distinct
//!   `jal`-target-to-prologue coincidences, admitted only on a unique
//!   dominating winner (the mechanized NW4E selector VA correction).
//!   Admitted deltas are candidate mappings, never proven `RomMapping`s.
//! - [`overlay_regions`]: ROM-only recovery of candidate overlay ROM
//!   intervals by searching for a descriptor table of the NW4E FAMILY
//!   (enumerate shapes, validate each record, canonicalize phase aliases),
//!   then tightening with [`delta_vote`] admissibility as the uniqueness
//!   filter. [`run_discovery_with_recovered_overlay_regions`] promotes a
//!   load image only when exactly one table is admitted and that record's
//!   unique delta-derived VA exactly matches its independently parsed
//!   descriptor destination; a delta vote alone remains candidate evidence.
//! - [`gp_base`]: IDO small-data `$gp` base recovery by constrained voting
//!   (boot `lui`/`addiu` constructions, or a bounded access-offset histogram
//!   fallback), admitted only on a unique dominating winner, then surfaces the
//!   gp-relative data accesses `xref.rs` cannot see. Emitted xref sites are
//!   candidate evidence; the admitted base is a typed program-level fact.
//! - [`homology`]: bounded relocation-masked cross-ROM n-gram lookup with
//!   collision-safe full-body validation and explicit ambiguous/unmatched
//!   results. Homology emits candidates only.
//! - [`cfg_homology`]: address-free typed CFG fingerprints with conservative
//!   unique-to-unique matching. Structural collisions remain ambiguous.
//! - [`trace`]: digest-bound, strictly sequenced dynamic trace ingestion with
//!   explicit unknown banks and bounded instrumentation guarantees separated
//!   from observations.
//! - [`probe`]: emulator-neutral, budgeted frontier probes with deterministic
//!   expected-information-gain scheduling and bank-aware overlap rejection.
//! - [`headless`]: content-bound black-box emulator run bundles and strict
//!   probe-filtered observation normalization into the trace schema.
//! - [`tool_adapter`]: strict bank-local external-tool interchange with exact
//!   input/lineage binding and a type-level candidate-only proof ceiling.
//! - [`tool_claims`]: a canonical snapshot-bound sidecar for external-tool
//!   candidates. It is structurally unable to mutate or promote native
//!   [`facts::FactDb`] conclusions.
//! - [`spimdisasm_adapter`]: pure-Rust normalization of pinned spimdisasm
//!   function-info CSV into that strict candidate-only interchange.
//! - [`snapshot`]: the byte-verified one-bank composition boundary that runs
//!   closure, fact integration, partitioning, owner proof, and coverage into
//!   one deterministic artifact. Traversal seeds never imply entry proof.
//! - [`grade_oot`] / [`grade_nw4e`]: grading-only cross-checks against the
//!   OoT decomp's segment answer key and NW4E's hand-verified
//!   `overlays.json`. Neither module is reachable from the discovery
//!   pipeline itself -- they only ever consume its output.
//! - [`grade_oot_functions`]: grading-only cross-check of Phase 4/5
//!   function boundaries against the OoT decomp's own linked `boot` bank
//!   (linker-map-derived, not hand-curated).
//! - [`grade_nw4e_symbols`]: grading-only cross-check of Phase 4/5 function
//!   boundaries against aki-recomp's hand-fixed NW4E `symbol_addrs.txt`
//!   rungs -- the "grind-collapse" measure.
//! - [`grade_nwxe_functions`]: grading-only cross-check against the complete
//!   WM2000/NWXE resident-bank function extents mechanically extracted from
//!   aki-recomp's generated `syms/dump.toml`.
//!
//! Dynamic indirect observations/callback-field semantics (Phase 6/7) and
//! assembly verification (Phase 8) are not yet implemented.

pub mod aki_reference;
pub mod answer_keys;
pub mod banks;
pub mod block_pack;
pub mod block_proof;
pub mod cfg;
pub mod cfg_homology;
pub mod coverage;
pub mod delta_vote;
pub mod evidence;
pub mod facts;
pub mod file_table;
pub mod gp_base;
pub mod grade_candidates;
pub mod grade_nw4e;
pub mod grade_nw4e_symbols;
pub mod grade_nwxe_functions;
pub mod grade_oot;
pub mod grade_oot_functions;
pub mod harvest;
pub mod headless;
pub mod homology;
pub mod load_table_use;
pub mod loaders;
pub mod oot_reference;
pub mod overlay_regions;
pub mod owner_proof;
pub mod partition;
pub mod pi_dma;
pub mod probe;
pub mod regions;
pub mod resolve;
pub mod rom;
pub mod snapshot;
pub mod spimdisasm_adapter;
pub mod tool_adapter;
pub mod tool_claims;
pub mod trace;
pub mod xref;

pub use facts::{BankAddr, Fact, FactDb, ProofState, RomAddressSpace};
pub use rom::{normalize, NormalizedRom, RomByteOrder, RomRejectReason};

#[derive(Debug)]
pub enum DiscoveryError {
    Rom(RomRejectReason),
    Evidence(evidence::EvidenceError),
    Harvest(harvest::HarvestError),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rom(error) => error.fmt(f),
            Self::Evidence(error) => error.fmt(f),
            Self::Harvest(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// An explicit, cited descriptor-table location/shape plus the naming
/// function for the banks it yields (see [`banks::scan_descriptor_table`]).
/// Named to keep [`run_discovery`]'s signature legible.
pub type DescriptorTableInput = (banks::DescriptorTableShape, fn(u32) -> String);

/// Configuration and deterministic naming for ROM-only recovered overlay
/// mappings. No ROM identity, table offset, or answer-key value enters this
/// input: the table location and every mapped interval are recovery outputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredOverlayInput {
    pub search: overlay_regions::SearchConfig,
    pub delta_vote: delta_vote::DeltaVoteConfig,
    pub min_mapped_regions: u32,
    pub table_name: String,
    pub bank_name: banks::BankNamePattern,
}

/// Run every currently-implemented discovery phase (1: normalize, 2: boot
/// bank + optional descriptor-table scan, 3: candidate harvest) over `rom_bytes` and return the
/// resulting fact database. This is the crate's single deterministic
/// entry point: calling it twice on byte-identical input must produce a
/// byte-identical `FactDb` (enforced by `tests/determinism.rs`).
/// Resolve a required out-of-tree input path from the environment, loudly.
///
/// Gate binaries consume user-owned inputs (ROMs, reference dumps) that must
/// never default to someone's home directory (DESIGN.md section 1.0: named
/// and declared, or absent with a loud error). `what` names the expected
/// content so the error message is actionable.
pub fn required_env_path(variable: &str, what: &str) -> Result<String, String> {
    std::env::var(variable)
        .map_err(|_| format!("{variable} is required: set it to the path of {what}"))
}

///
/// `descriptor_table` is optional because not every N64 title has one
/// (OoT does not; NW4E and other AKI-family titles do) -- passing `None`
/// still yields a valid, if smaller, fact DB with just the boot bank
/// proven and nothing else claimed.
pub fn run_discovery(
    rom_bytes: &[u8],
    descriptor_table: Option<DescriptorTableInput>,
) -> Result<(NormalizedRom, FactDb), RomRejectReason> {
    run_discovery_with_load_image_tables(rom_bytes, descriptor_table, &[])
}

/// Run discovery with additional explicitly-located generalized Phase-2
/// mapping tables. Answer keys are not accepted by this API: inputs contain
/// only table geometry and deterministic bank naming.
pub fn run_discovery_with_load_image_tables(
    rom_bytes: &[u8],
    descriptor_table: Option<DescriptorTableInput>,
    load_image_tables: &[banks::LoadImageTableInput],
) -> Result<(NormalizedRom, FactDb), RomRejectReason> {
    let rom = rom::normalize(rom_bytes)?;
    let mut db = FactDb::new();
    banks::discover_boot_bank(&rom, &mut db);
    if let Some((shape, bank_name)) = descriptor_table {
        banks::scan_descriptor_table(&rom, shape, bank_name, &mut db);
    }
    banks::scan_load_image_tables(&rom, load_image_tables, &mut db);
    harvest::harvest_discovered_candidates(&rom, &mut db)
        .expect("Phase 2 produced a malformed load-image mapping");
    Ok((rom, db))
}

/// Run discovery with overlay load images recovered mechanically from the
/// normalized ROM itself. The returned recovery keeps every raw/admitted
/// table visible; only the unique-table, matching-delta/destination proof rule
/// in [`banks::scan_recovered_overlay_regions`] can feed Phase 3.
pub fn run_discovery_with_recovered_overlay_regions(
    rom_bytes: &[u8],
    input: &RecoveredOverlayInput,
) -> Result<(NormalizedRom, FactDb, overlay_regions::OverlayRecovery), RomRejectReason> {
    let rom = rom::normalize(rom_bytes)?;
    let recovery = overlay_regions::recover_overlay_regions(
        &rom.bytes,
        &input.search,
        &input.delta_vote,
        input.min_mapped_regions,
    );
    let mut db = FactDb::new();
    banks::discover_boot_bank(&rom, &mut db);
    banks::scan_recovered_overlay_regions(
        &rom,
        &recovery,
        &input.table_name,
        &input.bank_name,
        &mut db,
    );
    harvest::harvest_discovered_candidates(&rom, &mut db)
        .expect("Phase 2 produced a malformed recovered-overlay mapping");
    Ok((rom, db, recovery))
}

/// Run discovery from a serializable external evidence manifest. The
/// manifest is checked against the normalized ROM SHA-256 before any claim is
/// consumed. It may describe mappings and executable intervals, but never
/// function answers; those remain outputs of the discovery pipeline.
pub fn run_discovery_with_manifest(
    rom_bytes: &[u8],
    manifest: &evidence::EvidenceManifest,
) -> Result<(NormalizedRom, FactDb), DiscoveryError> {
    let rom = rom::normalize(rom_bytes).map_err(DiscoveryError::Rom)?;
    manifest
        .validate_identity(&rom)
        .map_err(DiscoveryError::Evidence)?;
    let mut db = FactDb::new();
    banks::discover_boot_bank(&rom, &mut db);
    evidence::apply_mapping_evidence(&rom, manifest, &mut db).map_err(DiscoveryError::Evidence)?;
    evidence::apply_executable_evidence(manifest, &mut db).map_err(DiscoveryError::Evidence)?;
    harvest::harvest_discovered_candidates(&rom, &mut db).map_err(DiscoveryError::Harvest)?;
    Ok((rom, db))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_rom() -> Vec<u8> {
        let mut buf = vec![0u8; 0x1000 + 0x2000];
        buf[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        buf[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        buf[0x20..0x24].copy_from_slice(b"TEST");
        buf[0x3b..0x3f].copy_from_slice(b"CTSE");
        buf
    }

    #[test]
    fn run_discovery_without_descriptor_table_still_proves_boot_bank() {
        let bytes = make_test_rom();
        let (_rom, db) = run_discovery(&bytes, None).unwrap();
        assert_eq!(
            db.conclusion("bank:boot").unwrap().state,
            ProofState::Proven
        );
        assert_eq!(db.proven_function_entries("boot"), vec![0x8000_0400]);
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            Fact::FunctionEntryClaim {
                detector: facts::CandidateDetector::HardwareEntrypoint,
                evidence: facts::FunctionEntryEvidence::RomHeaderEntrypoint,
                proposed_state: ProofState::Proven,
                ..
            }
        )));
    }

    #[test]
    fn run_discovery_rejects_malformed_input_before_any_analysis() {
        let err = run_discovery(&[0u8; 4], None).unwrap_err();
        assert!(matches!(err, RomRejectReason::TooSmall { .. }));
    }

    #[test]
    fn run_discovery_is_byte_identical_across_repeated_runs() {
        let bytes = make_test_rom();
        let (_rom_a, db_a) = run_discovery(&bytes, None).unwrap();
        let (_rom_b, db_b) = run_discovery(&bytes, None).unwrap();
        let json_a = serde_json::to_string(&db_a).unwrap();
        let json_b = serde_json::to_string(&db_b).unwrap();
        assert_eq!(json_a, json_b);
    }
}
