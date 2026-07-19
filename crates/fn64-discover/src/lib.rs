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
//! - [`content_consumer`]: a candidate-only data-flow discriminator (the
//!   Ramblr concept, angr/Ramblr BSD-2, concept-only per the clean-room
//!   protocol) for words `cfg`'s reachability test leaves open -- classifies
//!   by consumer: loaded-then-dereferenced is a pointer, a proven
//!   branch/call edge target is code, and anything else stays ambiguous.
//!   Never reads or mutates [`facts::FactDb`] or a [`cfg::Cfg`]; strictly
//!   corroborating evidence that cannot override a proven conclusion.
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
//! - [`corpus_homology`]: N-ROM identity graph built on the pairwise engines --
//!   every ROM pair is matched by [`callgraph_match`], and the cross-ROM edges
//!   are closed transitively under a per-ROM-uniqueness + body-corroboration
//!   guard. A conflicting transitive edge collapses its component to ambiguous
//!   rather than guessing; a connected component is one function identity
//!   spanning N ROMs (libultra/SDK spans many). Candidates only.
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
pub mod asm_emit;
pub mod banks;
pub mod block_pack;
pub mod block_proof;
pub mod callgraph_match;
pub mod cfg;
pub mod cfg_homology;
pub mod closure;
pub mod content_consumer;
pub mod corpus_homology;
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
pub mod reloc_grade;
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

/// Configuration and deterministic naming for the two-stage file-table/VROM
/// overlay recovery path. Every table location and record geometry consumed
/// downstream is copied from [`overlay_regions::VromOverlayRecovery`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredVromOverlayInput {
    pub search: overlay_regions::SearchConfig,
    pub delta_vote: delta_vote::DeltaVoteConfig,
    pub file_table_search: file_table::FileTableSearchConfig,
    pub vrom_min_records: u32,
    pub min_mapped_regions: u32,
    pub file_table_name: String,
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

/// Run discovery with VROM-located overlay tables recovered mechanically
/// through a mechanically recovered physical file table.
///
/// The recovery stage admits descriptor tables through its delta-vote rule.
/// This adapter then expresses only records whose own delta-derived VA agrees
/// exactly with the independently parsed descriptor destination, plus the
/// uniquely admitted file table that makes their VROM bytes materializable,
/// as generalized load-image shapes already consumed by Phase 2. No table
/// location, stride, field offset, record count, or destination address is a
/// caller-supplied game fact.
pub fn run_discovery_with_recovered_vrom_overlay_regions(
    rom_bytes: &[u8],
    input: &RecoveredVromOverlayInput,
) -> Result<(NormalizedRom, FactDb, overlay_regions::VromOverlayRecovery), RomRejectReason> {
    use banks::{
        DestinationEnd, DestinationRangeFields, DestinationSpace, LoadImageTableInput,
        LoadImageTableShape, SourceRangeFields, TableLocation,
    };

    let rom = rom::normalize(rom_bytes)?;
    let recovery = overlay_regions::recover_vrom_overlay_regions(
        &rom.bytes,
        &input.search,
        &input.delta_vote,
        &input.file_table_search,
        input.vrom_min_records,
        input.min_mapped_regions,
    );
    let mut recovered_inputs = Vec::new();

    if let Some(table) = &recovery.file_table.admitted_table {
        recovered_inputs.push(LoadImageTableInput {
            name: input.file_table_name.clone(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Physical,
                    offset: table.table_rom_offset,
                },
                record_count: table.records.len() as u32,
                record_stride: table.record_stride,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: table.field_vrom_start,
                    field_end: table.field_vrom_end,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::PhysicalRom,
                    field_start: table.field_rom_start,
                    end: DestinationEnd::FieldOrSourceLength(table.field_rom_end),
                },
            },
            bank_name: None,
        });
    }

    let mut recovered_bank_index = 0u32;
    for (table_index, admission) in recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .enumerate()
    {
        let table = &admission.table;
        assert_eq!(
            table.records.len(),
            admission.region_deltas.len(),
            "VROM overlay recovery must report one delta outcome per record"
        );
        for (record_index, (record, delta_outcome)) in table
            .records
            .iter()
            .zip(&admission.region_deltas)
            .enumerate()
        {
            let Some((delta, va_start)) = *delta_outcome else {
                continue;
            };
            if record.rom_start.wrapping_add(delta) != va_start || va_start != record.vram_dest {
                continue;
            }
            let record_offset = (record_index as u32)
                .checked_mul(table.record_stride)
                .and_then(|offset| table.table_vrom_offset.checked_add(offset))
                .expect("recovered VROM table record location overflowed u32");
            recovered_inputs.push(LoadImageTableInput {
                name: format!("{}_{}_{}", input.table_name, table_index, record_index),
                shape: LoadImageTableShape {
                    location: TableLocation {
                        space: RomAddressSpace::Virtual,
                        offset: record_offset,
                    },
                    record_count: 1,
                    record_stride: table.record_stride,
                    source: SourceRangeFields {
                        space: RomAddressSpace::Virtual,
                        field_start: table.field_rom_start,
                        field_end: table.field_rom_end,
                    },
                    destination: DestinationRangeFields {
                        space: DestinationSpace::Vram,
                        field_start: table.field_vram_dest,
                        end: DestinationEnd::SourceLength,
                    },
                },
                bank_name: Some(banks::BankNamePattern {
                    prefix: input.bank_name.prefix.clone(),
                    suffix: input.bank_name.suffix.clone(),
                    index_base: input
                        .bank_name
                        .index_base
                        .checked_add(recovered_bank_index)
                        .expect("recovered overlay bank index overflowed u32"),
                }),
            });
            recovered_bank_index += 1;
        }
    }

    let mut db = FactDb::new();
    banks::discover_boot_bank(&rom, &mut db);
    banks::scan_load_image_tables(&rom, &recovered_inputs, &mut db);
    harvest::harvest_discovered_candidates(&rom, &mut db)
        .expect("Phase 2 produced a malformed recovered VROM overlay mapping");
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

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn plant_delta_admissible_region(bytes: &mut [u8], physical_start: usize, va_start: u32) {
        let jal = |target: u32| 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
        for (offset, target_offset) in [(0, 0x40), (8, 0x90), (16, 0x100)] {
            put_u32(
                bytes,
                physical_start + offset,
                jal(va_start + target_offset),
            );
        }
        for offset in [0x40, 0x90, 0x100] {
            put_u32(bytes, physical_start + offset, 0x27bd_ffe0);
        }
        for offset in [0x20, 0x24, 0x28, 0x2c] {
            put_u32(
                bytes,
                physical_start + offset,
                0x3c04_0000 | (va_start >> 16),
            );
        }
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

    #[test]
    fn recovered_vrom_path_composes_file_table_and_strict_overlay_banks() {
        let mut bytes = vec![0u8; 0xe000];
        put_u32(&mut bytes, 0, 0x8037_1240);
        put_u32(&mut bytes, 8, 0x8000_0400);

        // Three-record physical file table: the identity image, one file
        // carrying the descriptor table, and one carrying both overlays.
        for (index, fields) in [
            [0x0000, 0x3000, 0x0000, 0x0000],
            [0x3000, 0x6000, 0x8000, 0x0000],
            [0x6000, 0x9000, 0xb000, 0x0000],
        ]
        .into_iter()
        .enumerate()
        {
            for (field, value) in fields.into_iter().enumerate() {
                put_u32(&mut bytes, 0x2000 + index * 0x10 + field * 4, value);
            }
        }

        let descriptors = [(0x6000, 0x6800, 0x8002_0000), (0x7000, 0x7800, 0x8003_0000)];
        for (index, (vrom_start, vrom_end, vram)) in descriptors.into_iter().enumerate() {
            let base = 0x8000 + index * 0x1c;
            put_u32(&mut bytes, base, vrom_start);
            put_u32(&mut bytes, base + 4, vrom_end);
            put_u32(&mut bytes, base + 8, vram);
            let physical = 0xb000 + (vrom_start - 0x6000) as usize;
            plant_delta_admissible_region(&mut bytes, physical, vram);
        }

        let input = RecoveredVromOverlayInput {
            search: overlay_regions::SearchConfig::vrom_family(),
            delta_vote: delta_vote::DeltaVoteConfig::default(),
            file_table_search: file_table::FileTableSearchConfig::n64_family(),
            vrom_min_records: 2,
            min_mapped_regions: 2,
            file_table_name: "recovered_files".into(),
            table_name: "recovered_overlays".into(),
            bank_name: banks::BankNamePattern::new("recovered_", 0, ""),
        };
        let (_, db, recovery) =
            run_discovery_with_recovered_vrom_overlay_regions(&bytes, &input).unwrap();

        assert_eq!(
            recovery
                .file_table
                .admitted_table
                .as_ref()
                .map(|table| table.table_rom_offset),
            Some(0x2000)
        );
        assert_eq!(recovery.admitted_intervals().len(), 2);
        let overlay_mappings: Vec<_> = db
            .proven_rom_mappings()
            .into_iter()
            .filter(
                |fact| matches!(fact, Fact::RomMapping { bank, .. } if bank != banks::BOOT_BANK),
            )
            .collect();
        assert_eq!(overlay_mappings.len(), 2);
        assert!(overlay_mappings.iter().all(|fact| matches!(
            fact,
            Fact::RomMapping {
                rom_space: RomAddressSpace::Virtual,
                ..
            }
        )));
    }
}
