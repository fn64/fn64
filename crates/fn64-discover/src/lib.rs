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
//! - [`source_closure`]: canonical, ROM-bound executable-source inventory
//!   receipts. Construction sorts and validates evidence but never promotes
//!   bounded writer or indirect frontiers into an exhaustiveness claim.
//! - [`external_aot`]: deterministic cross-image admission for reproducibly
//!   captured executable generations. Ranges and truncated content identities
//!   must be collision-free against each other and every immutable AOT bank.
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
//! - [`cold_sweep`]: path-free cold discovery measurement from ROM bytes only,
//!   including automatic snapshot composition, closure tiers, and the complete
//!   byte ledger. A composition frontier remains typed `open`; it never becomes
//!   a misleading zero-unsupported result.
//! - [`stage1_effects`]: conservative syntactic COP0/cache/trap and constant-
//!   address memory-effect inventory over authority-reached code. It is an
//!   explicit negative classifier, not a general purity theorem.
//! - [`boot_tlb_alias`]: fail-closed boot TLB diagnostics that reuse the
//!   execution runtime's translator and intersect only proven physical bytes;
//!   diagnostics do not mint mappings or mutate discovery authority.
//! - [`program_transfer_index`]: deterministic forward/reverse intra-bank CFG
//!   and exact cross-bank call edges plus exact-owner caller/callee projections
//!   over validated composed snapshots; open and candidate evidence cannot
//!   enter the index.
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
//! - [`overlay_recipe`]: complete nine-field ROM/load/text/data/BSS
//!   materialization recipes, admitted only when one recovered table's range
//!   equations all agree.
//! - [`generation_topology`]: ROM-bound diagnostic immutable-prefix,
//!   resident-tail, and overlay generation geometry plus bounded enumeration
//!   of geometry-possible bank-qualified segments. It is a negative filter,
//!   not runtime state reachability, catalog identity, or activation authority.
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
//! - [`spimdisasm_reference`]: strict cached per-bank normalization of
//!   adapter-owned block, direct-reference, HI/LO-pair, and data candidates.
//! - [`snapshot`]: the byte-verified one-bank composition boundary that runs
//!   closure, fact integration, partitioning, owner proof, and coverage into
//!   one deterministic artifact. Traversal seeds never imply entry proof.
//! - [`snapshot_inputs`]: proven-bank enumeration, ROM/VROM materialization,
//!   `.bss`-prefix exclusion, and callable-derived traversal seeds shared by
//!   in-process snapshot producers. Seeds remain distinct from authority.
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
//! - [`timing_trace`]: producer-neutral cycle-stamped device-event trace
//!   interchange for the differential timing oracle, plus the fn64 fabric
//!   capture tap. A C reference producer emits the same JSONL.
//! - [`timing_diff`]: the differential timing comparator -- diffs fn64's
//!   device-event stream against a reference emulator's under a two-tier
//!   tolerance (zero-tolerance event ORDERING; a per-device cycle-count BAND),
//!   reporting the first divergence. Never runs an emulator; consumes two
//!   ingested [`timing_trace`] streams. The acceptance gate for every timing
//!   refinement item.
//!
//! Dynamic indirect observations/callback-field semantics (Phase 6/7) and
//! assembly verification (Phase 8) are not yet implemented.

pub mod aki_reference;
pub mod answer_keys;
pub mod asm_emit;
pub mod banks;
pub mod block_pack;
pub mod block_proof;
pub mod boot_tlb_alias;
pub mod boundaries;
pub mod callback_flow;
pub mod callgraph_match;
pub mod candidate_cfg_probe;
pub mod candidate_corroboration;
pub mod candidate_relation_report;
pub mod catalog_transfer_fixed_point;
pub mod cfg;
pub mod cfg_homology;
pub mod closure;
pub mod closure_audit;
pub mod cold_sweep;
pub mod content_consumer;
pub mod corpus_homology;
pub mod coverage;
pub mod delta_vote;
pub mod dense_aot_pack;
pub mod evidence;
pub mod external_aot;
pub mod facts;
pub mod file_table;
pub mod generation_topology;
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
pub mod host_bindings;
pub mod ledger;
pub mod load_table_use;
pub mod loaders;
pub mod missed_function_attribution;
pub mod oot_reference;
pub mod overlay_recipe;
pub mod overlay_regions;
pub mod overlay_reloc;
pub mod owner_proof;
pub mod partition;
pub mod pi_dma;
pub mod probe;
pub mod program_transfer_index;
pub mod regions;
pub mod reloc_grade;
pub mod resolve;
pub mod rom;
pub mod runtime_generation_catalog;
pub mod sig_scan;
pub mod snapshot;
pub mod snapshot_inputs;
pub mod snapshot_workspace;
pub mod source_closure;
pub mod spimdisasm_adapter;
pub mod spimdisasm_reference;
pub mod stage1_effects;
pub mod timing_diff;
pub mod timing_trace;
pub mod tool_adapter;
pub mod tool_claims;
pub mod trace;
pub mod transfer_scan;
pub mod workspace_artifacts;
pub mod writer_denominator;
pub mod xref;

#[cfg(test)]
extern crate self as fn64_discover;
#[cfg(test)]
#[path = "../tests/candidate_corroboration.rs"]
mod candidate_corroboration_receipt_tests;

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
/// still yields a valid, if smaller, fact DB. The boot bank is proven only for
/// a complete, exactly recognized standard IPL3 and complete 1 MiB source;
/// otherwise its conclusion remains explicitly `Open`.
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
    let mut db = discover_with_load_image_tables(&rom, descriptor_table, load_image_tables);
    harvest::harvest_discovered_candidates(&rom, &mut db)
        .expect("Phase 2 produced a malformed load-image mapping");
    Ok((rom, db))
}

fn discover_with_load_image_tables(
    rom: &NormalizedRom,
    descriptor_table: Option<DescriptorTableInput>,
    load_image_tables: &[banks::LoadImageTableInput],
) -> FactDb {
    let mut db = FactDb::new();
    let _boot = banks::discover_boot_bank(rom, &mut db);
    if let Some((shape, bank_name)) = descriptor_table {
        banks::scan_descriptor_table(rom, shape, bank_name, &mut db);
    }
    banks::scan_load_image_tables(rom, load_image_tables, &mut db);
    db
}

/// [`run_discovery_with_load_image_tables`] plus a static request-DMA scan
/// over the proven boot image (cited claims; see
/// [`banks::StaticRequestDmaInput`]). The scan runs after table proving so
/// Virtual-space device operands can corroborate against proven VROM file
/// mappings, and before harvest so recovered banks feed Phase 3.
pub fn run_discovery_with_tables_and_request_dma(
    rom_bytes: &[u8],
    descriptor_table: Option<DescriptorTableInput>,
    load_image_tables: &[banks::LoadImageTableInput],
    request_dma: &[banks::StaticRequestDmaInput],
) -> Result<(NormalizedRom, FactDb, banks::StaticRequestDmaReport), RomRejectReason> {
    let rom = rom::normalize(rom_bytes)?;
    let mut db = discover_with_load_image_tables(&rom, descriptor_table, load_image_tables);
    let report = banks::scan_static_request_dma(&rom, request_dma, &mut db);
    harvest::harvest_discovered_candidates(&rom, &mut db)
        .expect("Phase 2 produced a malformed load-image mapping");
    Ok((rom, db, report))
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
    let (mut db, recovery) = discover_with_recovered_overlay_regions(&rom, input);
    harvest::harvest_discovered_candidates(&rom, &mut db)
        .expect("Phase 2 produced a malformed recovered-overlay mapping");
    Ok((rom, db, recovery))
}

fn discover_with_recovered_overlay_regions(
    rom: &NormalizedRom,
    input: &RecoveredOverlayInput,
) -> (FactDb, overlay_regions::OverlayRecovery) {
    let recovery = overlay_regions::recover_overlay_regions(
        &rom.bytes,
        &input.search,
        &input.delta_vote,
        input.min_mapped_regions,
    );
    let mut db = FactDb::new();
    let _boot = banks::discover_boot_bank(rom, &mut db);
    banks::scan_recovered_overlay_regions(
        rom,
        &recovery,
        &input.table_name,
        &input.bank_name,
        &mut db,
    );
    (db, recovery)
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
/// One corroborated call site is enough. The evidence is not how often a
/// routine is called -- a game may load its resident image exactly once from
/// boot -- but that the constant `(vrom, size)` pair recovered at the site
/// lands EXACTLY on a file-table record already proven from this ROM. Both
/// fields must match, so a coincidental hit against the recovered record set
/// is not a realistic failure mode; a single contradicting site still
/// rejects the candidate outright.
const REQUEST_DMA_MIN_SITES: usize = 1;

pub fn run_discovery_with_recovered_vrom_overlay_regions(
    rom_bytes: &[u8],
    input: &RecoveredVromOverlayInput,
) -> Result<(NormalizedRom, FactDb, overlay_regions::VromOverlayRecovery), RomRejectReason> {
    run_discovery_with_recovered_vrom_overlay_regions_with_limits(
        rom_bytes,
        input,
        file_table::VromMaterializationLimits::default(),
    )
}

/// [`run_discovery_with_recovered_vrom_overlay_regions`] with an explicit
/// complete-file VROM decode cap.
pub fn run_discovery_with_recovered_vrom_overlay_regions_with_limits(
    rom_bytes: &[u8],
    input: &RecoveredVromOverlayInput,
    materialization_limits: file_table::VromMaterializationLimits,
) -> Result<(NormalizedRom, FactDb, overlay_regions::VromOverlayRecovery), RomRejectReason> {
    run_discovery_with_recovered_vrom_and_request_dma_with_limits(
        rom_bytes,
        input,
        &[],
        materialization_limits,
    )
    .map(|(rom, db, recovery, _report)| (rom, db, recovery))
}

/// [`run_discovery_with_recovered_vrom_overlay_regions`] plus a static
/// request-DMA scan over the proven boot image. Mechanical overlay geometry
/// recovers files a descriptor table names; the request-DMA scan recovers the
/// resident images a game loads by an explicit DMA call instead, which no
/// table describes. Neither supplies per-ROM table geometry.
pub fn run_discovery_with_recovered_vrom_and_request_dma(
    rom_bytes: &[u8],
    input: &RecoveredVromOverlayInput,
    request_dma: &[banks::StaticRequestDmaInput],
) -> Result<
    (
        NormalizedRom,
        FactDb,
        overlay_regions::VromOverlayRecovery,
        banks::StaticRequestDmaReport,
    ),
    RomRejectReason,
> {
    run_discovery_with_recovered_vrom_and_request_dma_with_limits(
        rom_bytes,
        input,
        request_dma,
        file_table::VromMaterializationLimits::default(),
    )
}

/// [`run_discovery_with_recovered_vrom_and_request_dma`] with an explicit
/// complete-file VROM decode cap.
pub fn run_discovery_with_recovered_vrom_and_request_dma_with_limits(
    rom_bytes: &[u8],
    input: &RecoveredVromOverlayInput,
    request_dma: &[banks::StaticRequestDmaInput],
    materialization_limits: file_table::VromMaterializationLimits,
) -> Result<
    (
        NormalizedRom,
        FactDb,
        overlay_regions::VromOverlayRecovery,
        banks::StaticRequestDmaReport,
    ),
    RomRejectReason,
> {
    let rom = rom::normalize(rom_bytes)?;
    let (mut db, recovery, request_dma_report) = discover_with_recovered_vrom_and_request_dma(
        &rom,
        input,
        request_dma,
        materialization_limits,
    );
    harvest::harvest_discovered_candidates_bounded(
        &rom,
        &mut db,
        materialization_limits.max_decoded_file_bytes,
    )
    .expect("Phase 2 produced a malformed recovered VROM overlay mapping");
    Ok((rom, db, recovery, request_dma_report))
}

fn discover_with_recovered_vrom_and_request_dma(
    rom: &NormalizedRom,
    input: &RecoveredVromOverlayInput,
    request_dma: &[banks::StaticRequestDmaInput],
    materialization_limits: file_table::VromMaterializationLimits,
) -> (
    FactDb,
    overlay_regions::VromOverlayRecovery,
    banks::StaticRequestDmaReport,
) {
    use banks::{
        DestinationEnd, DestinationRangeFields, DestinationSpace, LoadImageTableInput,
        LoadImageTableShape, SourceRangeFields, TableLocation,
    };

    let recovery = overlay_regions::recover_vrom_overlay_regions_with_limits(
        &rom.bytes,
        &input.search,
        &input.delta_vote,
        &input.file_table_search,
        input.vrom_min_records,
        input.min_mapped_regions,
        materialization_limits,
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
    // Distinct descriptor-family runs can overlap without being aliases as
    // whole tables. Their shared records still describe one load image and
    // therefore one bank identity. Keep conflicting destinations distinct so
    // the ordinary mapping-conflict path can surface them.
    let mut recovered_geometries = std::collections::BTreeSet::new();
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
            let Some(va_end) = va_start.checked_add(record.byte_len()) else {
                continue;
            };
            if !recovered_geometries.insert((record.rom_start, record.rom_end, va_start, va_end)) {
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
    let _boot = banks::discover_boot_bank(rom, &mut db);
    banks::scan_load_image_tables_bounded(
        rom,
        &recovered_inputs,
        &mut db,
        materialization_limits.max_decoded_file_bytes,
    );
    let mut effective: Vec<banks::StaticRequestDmaInput> = request_dma.to_vec();
    let mut wrapper_diagnostics = PhysicalWrapperCandidateDiagnostics::default();
    if let Some((rom_start, rom_end, va_start)) =
        db.proven_rom_mappings().iter().find_map(|fact| match fact {
            Fact::RomMapping {
                bank,
                rom_space: RomAddressSpace::Physical,
                rom_start,
                rom_end,
                va_start,
                ..
            } if bank == banks::BOOT_BANK => Some((*rom_start, *rom_end, *va_start)),
            _ => None,
        })
    {
        if let Some(bytes) = rom.bytes.get(rom_start as usize..rom_end as usize) {
            let words: Vec<u32> = bytes
                .chunks_exact(4)
                .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
                .collect();
            wrapper_diagnostics =
                record_physical_end_dma_wrapper_candidates(&words, va_start, &mut db);
        }
    }
    // Mechanically recover the game's DMA-request routine rather than citing
    // its address: a candidate is admitted only when its call-site (vrom,
    // size) operands land exactly on file-table records already proven above.
    // Caller-supplied claims, if any, are unioned in and take no precedence.
    let callee_recovery = banks::recover_request_dma_callees(rom, &db, REQUEST_DMA_MIN_SITES);
    for (index, callee) in callee_recovery.admitted.iter().enumerate() {
        if effective.iter().any(|c| c.callee_va == callee.callee_va) {
            continue;
        }
        effective.push(banks::StaticRequestDmaInput {
            name: format!("recovered_request_dma_{index}"),
            callee_va: callee.callee_va,
            dram_arg_register: 4,
            device_arg_register: 5,
            size_arg_register: 6,
            size_is_end_address: false,
            device_space: RomAddressSpace::Virtual,
            bank_name: banks::BankNamePattern::new("request_dma_", 0, ""),
        });
    }
    let mut request_dma_report = banks::scan_static_request_dma_fixed_point_bounded(
        rom,
        &effective,
        &mut db,
        materialization_limits.max_decoded_file_bytes,
    );
    request_dma_report.physical_wrapper_candidates_examined =
        wrapper_diagnostics.candidates_examined;
    request_dma_report.wrapper_semantic_proof_unavailable =
        wrapper_diagnostics.semantic_proof_unavailable;
    request_dma_report.physical_wrapper_candidate_limit_hit = wrapper_diagnostics.limit_hit;
    if wrapper_diagnostics.semantic_proof_unavailable != 0 {
        request_dma_report.push_open_bounded(format!(
            "wrapper_semantic_proof_unavailable: {} physical end-address DMA wrapper shape candidate(s) remain candidate-only",
            wrapper_diagnostics.semantic_proof_unavailable
        ));
    }
    if wrapper_diagnostics.limit_hit {
        request_dma_report.push_open_bounded(
            "physical end-address DMA-wrapper inference reached its candidate bound; result is incomplete"
                .to_string(),
        );
    }
    (db, recovery, request_dma_report)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PhysicalWrapperCandidateDiagnostics {
    candidates_examined: usize,
    semantic_proof_unavailable: usize,
    limit_hit: bool,
}

/// Retain wrapper-shape evidence without creating a loader input or mapping.
/// CFG/path and inner-callee authority are prerequisites for that later step.
fn record_physical_end_dma_wrapper_candidates(
    words: &[u32],
    va_start: u32,
    db: &mut FactDb,
) -> PhysicalWrapperCandidateDiagnostics {
    let inference = pi_dma::infer_physical_end_dma_wrappers(words, va_start);
    let diagnostics = PhysicalWrapperCandidateDiagnostics {
        candidates_examined: inference.candidates_examined,
        semantic_proof_unavailable: inference.admitted.len(),
        limit_hit: inference.candidate_limit_hit,
    };
    for wrapper in inference.admitted {
        db.insert(Fact::Evidence {
            subject: facts::BankAddr::new(banks::BOOT_BANK, wrapper.entry_va),
            note: format!(
                "candidate-only physical end-address DMA wrapper shape at 0x{:x}: {} \
                 direct call-shaped words; inner DMA-shaped call at 0x{:x}; linear scan \
                 observed a2-a1 length, cursor advances, length reduction, and a backward \
                 branch; CFG/path and inner-callee authority remain open",
                wrapper.entry_va,
                wrapper.callers.len(),
                wrapper.nested_dma_call_pc,
            ),
        });
    }
    diagnostics
}

/// Which mechanical composition strategy corroborated a ROM's overlay
/// geometry.
///
/// Recovery-strategy declaration order is the deterministic tie-break order.
/// The two boot-only variants describe the baseline outcome and do not compete
/// with one another.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryStrategy {
    /// No boot mapping was proven because the IPL3 or its complete DMA source
    /// lacked an admitted identity/extent. Other strategies are still
    /// attempted and may supersede this baseline outcome.
    BootBankOpen,
    /// Nothing beyond the IPL3 boot copy corroborated. This is a real result,
    /// not a failure: it says the ROM carries no overlay geometry this build
    /// knows how to recover.
    BootBankOnly,
    /// Mechanically recovered VROM overlay geometry behind a recovered file
    /// table -- a dmadata-shaped table addressing virtual ROM (OoT-class).
    RecoveredVrom,
    /// Mechanically recovered overlay descriptor table addressing physical
    /// ROM (AKI-class).
    RecoveredOverlays,
    /// Load addresses inferred from `jal` statistics with NO table of any kind
    /// ([`delta_vote::prove_region`] over the ledger's unclaimed runs). Weakest
    /// evidence class here and
    /// the only one that concludes `Supported` rather than `Proven`; selected
    /// only when nothing with an independent table corroborated.
    UntabledDeltaVote,
}

impl DiscoveryStrategy {
    pub fn label(self) -> &'static str {
        match self {
            Self::BootBankOpen => "boot_bank_open",
            Self::BootBankOnly => "boot_bank_only",
            Self::RecoveredVrom => "recovered_vrom",
            Self::RecoveredOverlays => "recovered_overlays",
            Self::UntabledDeltaVote => "untabled_delta_vote",
        }
    }
}

/// What one strategy found on this ROM, recorded whether or not it was
/// selected. Every strategy attempted reports an outcome: a strategy that
/// recovered nothing is stated, never omitted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyOutcome {
    pub strategy: DiscoveryStrategy,
    pub candidate_tables: usize,
    pub admitted_tables: usize,
    pub admitted_intervals: usize,
    /// Distinct VROM files withheld by a configured complete-file decode cap.
    /// Nonzero marks a resource frontier, not proven absence.
    pub decoded_file_limit_hits: usize,
    /// Proven `RomMapping` facts the strategy's database ended with. The boot
    /// bank is included only when its IPL3-bound proof completed.
    pub proven_mappings: usize,
    /// Mappings concluded at `Supported` rather than `Proven`. Only the
    /// untabled delta-vote strategy produces these; kept in a separate field so
    /// an inferred mapping can never be counted as a corroborated one.
    pub supported_mappings: usize,
    /// Bounded request-DMA frontier rows retained by this strategy.
    pub request_dma_open_rows: usize,
    /// True when request-DMA or wrapper analysis left any typed/resource
    /// frontier open. Consumers must not interpret zero new mappings as
    /// proven absence when this is set.
    pub request_dma_incomplete: bool,
    /// Loader inputs beyond the deterministic 64-input prefix were withheld.
    pub request_dma_input_limit_hit: bool,
    /// Wrapper-shape callees examined by the bounded candidate classifier.
    pub physical_wrapper_candidates_examined: usize,
    /// Wrapper shapes retained as candidates because semantic proof is not
    /// yet available; these never feed `Proven` mappings.
    pub wrapper_semantic_proof_unavailable: usize,
    /// The wrapper candidate classifier exhausted its work bound.
    pub physical_wrapper_candidate_limit_hit: bool,
}

/// The result of trying every mechanical composition strategy against one ROM.
#[derive(Debug, Clone)]
pub struct AutoDiscovery {
    pub rom: NormalizedRom,
    pub facts: FactDb,
    pub selected: DiscoveryStrategy,
    /// Every strategy attempted, in evaluation order.
    pub outcomes: Vec<StrategyOutcome>,
}

/// Transient resource limits applied by automatic discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoDiscoveryLimits {
    /// Complete-file VROM decode cap for file-table and overlay recovery.
    pub vrom_materialization: file_table::VromMaterializationLimits,
}

impl Default for AutoDiscoveryLimits {
    fn default() -> Self {
        Self {
            vrom_materialization: file_table::VromMaterializationLimits::default(),
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AutoDiscoveryWork {
    normalizations: usize,
    harvests: usize,
}

#[cfg(test)]
thread_local! {
    static AUTO_DISCOVERY_WORK: std::cell::Cell<AutoDiscoveryWork> =
        const { std::cell::Cell::new(AutoDiscoveryWork { normalizations: 0, harvests: 0 }) };
}

fn normalize_for_auto(rom_bytes: &[u8]) -> Result<NormalizedRom, RomRejectReason> {
    #[cfg(test)]
    AUTO_DISCOVERY_WORK.with(|work| {
        let mut counts = work.get();
        counts.normalizations += 1;
        work.set(counts);
    });
    rom::normalize(rom_bytes)
}

fn harvest_for_auto(
    rom: &NormalizedRom,
    db: &mut FactDb,
    malformed_mapping: &str,
    max_decoded_vrom_file_bytes: usize,
) {
    #[cfg(test)]
    AUTO_DISCOVERY_WORK.with(|work| {
        let mut counts = work.get();
        counts.harvests += 1;
        work.set(counts);
    });
    harvest::harvest_discovered_candidates_bounded(rom, db, max_decoded_vrom_file_bytes)
        .expect(malformed_mapping);
}

/// Run discovery without being told what kind of ROM this is.
///
/// The per-strategy recovery passes are already mechanical -- no table
/// location, stride, record count, or destination is a caller-supplied game
/// fact -- but until now the CHOICE of which to run was hardcoded per game in
/// the gates, so the generic entry point ran none of them and stopped after
/// the IPL3-bound boot attempt. This tries each and keeps whichever
/// corroborates, which is the difference between "recompiles ROMs we
/// hand-wired" and "works out what a ROM is".
///
/// Selection rule: a strategy is admitted only if it proves strictly more ROM
/// mappings than the baseline attempt, and the strategy proving the most wins.
/// Ties break by [`DiscoveryStrategy`] declaration order, so the choice is
/// deterministic. Nothing is merged across strategies -- the winner's database
/// is returned whole -- so no strategy can contribute a mapping that its own
/// admission rules did not justify.
///
/// A ROM that corroborates nothing returns [`DiscoveryStrategy::BootBankOnly`]
/// when its boot mapping was proven, or [`DiscoveryStrategy::BootBankOpen`]
/// when that prerequisite remained open. Every attempt's outcome is recorded.
pub fn run_discovery_auto(rom_bytes: &[u8]) -> Result<AutoDiscovery, RomRejectReason> {
    run_discovery_auto_with_limits(rom_bytes, AutoDiscoveryLimits::default())
}

/// [`run_discovery_auto`] with explicit transient VROM materialization limits.
/// Oversized files leave the VROM strategy open and cannot contribute facts.
pub fn run_discovery_auto_with_limits(
    rom_bytes: &[u8],
    limits: AutoDiscoveryLimits,
) -> Result<AutoDiscovery, RomRejectReason> {
    // The floor every strategy must beat. Each strategy below repeats the same
    // IPL3-bound boot attempt, so this is a like-for-like comparison whether
    // that attempt proves a mapping or records an Open frontier.
    let rom = normalize_for_auto(rom_bytes)?;
    let baseline_db = discover_with_load_image_tables(&rom, None, &[]);
    let baseline_mappings = baseline_db.proven_rom_mappings().len();
    let baseline_strategy = if baseline_db
        .conclusion("bank:boot")
        .is_some_and(|conclusion| conclusion.state == facts::ProofState::Proven)
    {
        DiscoveryStrategy::BootBankOnly
    } else {
        DiscoveryStrategy::BootBankOpen
    };
    let mut outcomes = vec![StrategyOutcome {
        strategy: baseline_strategy,
        candidate_tables: 0,
        admitted_tables: 0,
        admitted_intervals: 0,
        decoded_file_limit_hits: 0,
        proven_mappings: baseline_mappings,
        supported_mappings: 0,
        request_dma_open_rows: 0,
        request_dma_incomplete: false,
        request_dma_input_limit_hit: false,
        physical_wrapper_candidates_examined: 0,
        wrapper_semantic_proof_unavailable: 0,
        physical_wrapper_candidate_limit_hit: false,
    }];
    let mut best: Option<(DiscoveryStrategy, FactDb, usize)> = None;

    let vrom_input = RecoveredVromOverlayInput {
        search: overlay_regions::SearchConfig::vrom_family(),
        delta_vote: delta_vote::DeltaVoteConfig::default(),
        file_table_search: file_table::FileTableSearchConfig::n64_family(),
        vrom_min_records: 2,
        min_mapped_regions: 2,
        file_table_name: "recovered_file_table".to_string(),
        table_name: "recovered_vrom_overlay_descriptors".to_string(),
        bank_name: banks::BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    let (vrom_db, vrom_recovery, request_dma_report) = discover_with_recovered_vrom_and_request_dma(
        &rom,
        &vrom_input,
        &[],
        limits.vrom_materialization,
    );
    let vrom_mappings = vrom_db.proven_rom_mappings().len();
    outcomes.push(StrategyOutcome {
        strategy: DiscoveryStrategy::RecoveredVrom,
        candidate_tables: vrom_recovery.candidate_tables.len(),
        admitted_tables: vrom_recovery
            .admissions
            .iter()
            .filter(|admission| admission.admitted)
            .count(),
        admitted_intervals: vrom_recovery.admitted_intervals().len(),
        decoded_file_limit_hits: vrom_recovery.decoded_file_limit_hits.len(),
        proven_mappings: vrom_mappings,
        supported_mappings: 0,
        request_dma_open_rows: request_dma_report.open.len(),
        request_dma_incomplete: !request_dma_report.open.is_empty(),
        request_dma_input_limit_hit: request_dma_report.input_limit_hit,
        physical_wrapper_candidates_examined: request_dma_report
            .physical_wrapper_candidates_examined,
        wrapper_semantic_proof_unavailable: request_dma_report.wrapper_semantic_proof_unavailable,
        physical_wrapper_candidate_limit_hit: request_dma_report
            .physical_wrapper_candidate_limit_hit,
    });
    if vrom_mappings > baseline_mappings {
        best = Some((DiscoveryStrategy::RecoveredVrom, vrom_db, vrom_mappings));
    }

    let overlay_search = overlay_regions::SearchConfig::aki_family();
    let overlay_input = RecoveredOverlayInput {
        min_mapped_regions: overlay_search.min_records,
        search: overlay_search,
        delta_vote: delta_vote::DeltaVoteConfig::default(),
        table_name: "recovered_overlay_descriptors".to_string(),
        bank_name: banks::BankNamePattern::new("recovered_overlay_", 0, ""),
    };
    let (overlay_db, overlay_recovery) =
        discover_with_recovered_overlay_regions(&rom, &overlay_input);
    let overlay_mappings = overlay_db.proven_rom_mappings().len();
    outcomes.push(StrategyOutcome {
        strategy: DiscoveryStrategy::RecoveredOverlays,
        candidate_tables: overlay_recovery.candidate_tables.len(),
        admitted_tables: overlay_recovery
            .admissions
            .iter()
            .filter(|admission| admission.admitted)
            .count(),
        admitted_intervals: overlay_recovery.admitted_intervals().len(),
        decoded_file_limit_hits: 0,
        proven_mappings: overlay_mappings,
        supported_mappings: 0,
        request_dma_open_rows: 0,
        request_dma_incomplete: false,
        request_dma_input_limit_hit: false,
        physical_wrapper_candidates_examined: 0,
        wrapper_semantic_proof_unavailable: 0,
        physical_wrapper_candidate_limit_hit: false,
    });
    if overlay_mappings > baseline_mappings
        && best
            .as_ref()
            .is_none_or(|(_, _, best_mappings)| overlay_mappings > *best_mappings)
    {
        best = Some((
            DiscoveryStrategy::RecoveredOverlays,
            overlay_db,
            overlay_mappings,
        ));
    }

    // Last resort: infer load addresses from the instruction encoding itself.
    //
    // Every strategy above recognises a STRUCTURE and is bound to the engine
    // families whose structure it knows; measured, six of twelve corpus ROMs
    // find zero candidate tables of either family. A `jal` carries an ABSOLUTE
    // target, so a blob of code states in its own bytes where it expects to run,
    // which is a property of MIPS encoding rather than of any engine.
    //
    // Concluded `Supported`, never `Proven`: delta_vote's own outcome type calls
    // an admitted delta a candidate mapping, and instruction bytes do not prove
    // a region is reachable, resident, or ever loaded. Ordering it last is the
    // guarantee that an inferred mapping never displaces a corroborated one.
    if best.is_none() {
        let mut untabled_db = baseline_db;
        harvest_for_auto(
            &rom,
            &mut untabled_db,
            "Phase 2 produced a malformed boot-bank mapping",
            limits.vrom_materialization.max_decoded_file_bytes,
        );
        // Extents come from the ledger's UNCLAIMED runs, not fixed windows.
        // That choice is the whole mechanism: WCW World Tour's 352 KiB image
        // scores 328 votes to 25 over its natural extent, and 2-3 votes per
        // fixed 32 KiB window with every window returning Open. Fragmenting a
        // region destroys the evidence the vote needs.
        //
        // The ledger is built from the baseline (boot-copy) facts, so "unclaimed"
        // means "not the boot copy" -- exactly the territory a table strategy
        // would have covered had one applied.
        let baseline_ledger = ledger::build_ledger(&rom.bytes, &untabled_db);
        let vote = delta_vote::DeltaVoteConfig::default();

        // Candidates come from TWO sources, because neither alone is sufficient
        // and the proof makes combining them safe.
        //
        // 1. The ledger's `code_like` runs. These are natural extents, which is
        //    what makes the proof work: WCW's 352 KiB image scores 328 votes to
        //    25 over its extent and 2-3 per fixed window. The heuristic's false
        //    positives cost nothing here -- the proof rejects all eight known
        //    OoT ones.
        // 2. The fixed-window sweep. An unclaimed run is "everything not yet
        //    claimed", so a small image inside megabytes of assets has no
        //    `code_like` extent of its own and would be lost. Measured: proving
        //    whole unclaimed runs alone regressed Perfect Dark and WCW Revenge
        //    from three regions each to zero.
        //
        // Union, de-duplicated by ROM start; a region proved from its natural
        // extent supersedes a windowed one covering the same start.
        let mut proved: std::collections::BTreeMap<u32, delta_vote::UntabledRegion> =
            std::collections::BTreeMap::new();
        for region in delta_vote::sweep_untabled_regions(
            &rom.bytes,
            &delta_vote::UntabledSweepConfig::default(),
        ) {
            proved.insert(region.rom_start, region);
        }
        // Code runs, with small internal gaps bridged.
        //
        // A code image is not uniformly code: jump tables and constant pools sit
        // inside it and carry no function returns, so the classifier punches
        // holes through a single image. Those holes are fatal here because the
        // proof needs a large extent to accumulate votes -- measured on
        // n64-systemtest, requiring returns split two contiguous runs (416K and
        // 160K, both proven whole) into eight fragments of which only two could
        // still be proven, losing 212 KB that had previously been recovered.
        //
        // The bridge is SCALE-RELATIVE, not a fixed size: a gap is interior to an
        // image only if it is smaller than the code on both sides of it. A
        // genuine boundary between two images is not dwarfed by its neighbours.
        // Merged hulls are offered ALONGSIDE the fragments, never instead of
        // them, so bridging can only add candidates; a hull that is not really
        // one image simply fails to prove.
        let code_runs: Vec<(u32, u32)> = baseline_ledger
            .spans
            .iter()
            .filter(|span| span.class == ledger::SpanClass::CodeLike)
            .map(|span| (span.rom_start, span.rom_end))
            .collect();
        let mut hulls: Vec<(u32, u32)> = Vec::new();
        for &(start, end) in &code_runs {
            // Bridge when the gap is small relative to the code ALREADY
            // accumulated, not to the next fragment. A jump table sitting
            // between 176 KiB of code and an 8 KiB tail is interior to the
            // image; comparing against the smaller neighbour would refuse it on
            // a tie, which is exactly what left n64-systemtest's 416 KiB run
            // split at its first data island.
            //
            // Self-limiting in practice: a genuine boundary between two images
            // is large, not dwarfed by its left-hand neighbour. And an
            // over-merged hull costs only compute, because hulls are offered
            // alongside the fragments and a hull that is not one image simply
            // fails to prove.
            let bridged = hulls.last().is_some_and(|&(hull_start, hull_end)| {
                let gap = start.saturating_sub(hull_end);
                gap > 0 && gap < hull_end - hull_start
            });
            if bridged {
                hulls.last_mut().expect("checked above").1 = end;
            } else {
                hulls.push((start, end));
            }
        }
        for (start, end) in code_runs.into_iter().chain(hulls) {
            if let Some(region) = delta_vote::prove_region(&rom.bytes, start, end, &vote) {
                proved.insert(region.rom_start, region);
            }
        }
        // A whole-extent proof SUPERSEDES any windowed region inside it. Keeping
        // both would emit two mappings claiming the same ROM bytes at different
        // addresses -- a contradiction, and one `surface_mapping_conflicts`
        // would rightly flag. Observed: WCW's windowed region at
        // 0xab0000-0xac0000 lies inside the extent-proved 0xa69000-0xac1000.
        let candidates: Vec<delta_vote::UntabledRegion> = proved.into_values().collect();
        // The raw sweep is intentionally ROM-wide, so it can rediscover a
        // delta-consistent slice inside the already-proven IPL3 copy. Such a
        // slice is not a new bank: admitting it would duplicate the boot image
        // under a supported name and make downstream coverage look larger than
        // the physical load evidence warrants.
        let baseline_physical_mappings: Vec<(u32, u32)> = untabled_db
            .proven_rom_mappings()
            .into_iter()
            .filter_map(|fact| match fact {
                Fact::RomMapping {
                    rom_space: RomAddressSpace::Physical,
                    rom_start,
                    rom_end,
                    ..
                } => Some((*rom_start, *rom_end)),
                _ => None,
            })
            .collect();
        let regions: Vec<delta_vote::UntabledRegion> = candidates
            .iter()
            .filter(|region| {
                !baseline_physical_mappings
                    .iter()
                    .any(|&(mapped_start, mapped_end)| {
                        region.rom_start < mapped_end && mapped_start < region.rom_end
                    })
            })
            .filter(|region| {
                !candidates.iter().any(|other| {
                    other.rom_start <= region.rom_start
                        && region.rom_end <= other.rom_end
                        && (other.rom_start, other.rom_end) != (region.rom_start, region.rom_end)
                })
            })
            .cloned()
            .collect();
        for (index, region) in regions.iter().enumerate() {
            let bank = format!("untabled_region_{index}");
            let mapping = untabled_db.insert(Fact::RomMapping {
                bank: bank.clone(),
                rom_space: RomAddressSpace::Physical,
                rom_start: region.rom_start,
                rom_end: region.rom_end,
                va_start: region.va_start,
                va_end: region
                    .va_start
                    .wrapping_add(region.rom_end - region.rom_start),
            });
            let evidence = untabled_db.insert(Fact::Evidence {
                subject: facts::BankAddr::new(&bank, region.va_start),
                note: format!(
                    "load address inferred from jal statistics with no table: ROM \
                     0x{:x}..0x{:x} -> VA 0x{:x} (delta 0x{:x}), agreed by {} independent \
                     window(s). Supported, not Proven -- instruction bytes do not prove the \
                     region is reachable, resident, or ever loaded.",
                    region.rom_start, region.rom_end, region.va_start, region.delta, region.windows,
                ),
            });
            untabled_db
                .conclude(
                    format!("bank:{bank}"),
                    facts::ProofState::Supported,
                    vec![mapping, evidence],
                    "untabled_delta_vote",
                )
                .expect("untabled region bank names are freshly generated");
        }
        outcomes.push(StrategyOutcome {
            strategy: DiscoveryStrategy::UntabledDeltaVote,
            candidate_tables: 0,
            admitted_tables: 0,
            admitted_intervals: regions.len(),
            decoded_file_limit_hits: 0,
            proven_mappings: untabled_db.proven_rom_mappings().len(),
            supported_mappings: regions.len(),
            request_dma_open_rows: 0,
            request_dma_incomplete: false,
            request_dma_input_limit_hit: false,
            physical_wrapper_candidates_examined: 0,
            wrapper_semantic_proof_unavailable: 0,
            physical_wrapper_candidate_limit_hit: false,
        });
        let selected = if regions.is_empty() {
            baseline_strategy
        } else {
            DiscoveryStrategy::UntabledDeltaVote
        };
        return Ok(AutoDiscovery {
            rom,
            facts: untabled_db,
            selected,
            outcomes,
        });
    }

    let (selected, mut facts, _) = best.expect("a corroborated table strategy was recorded");
    harvest_for_auto(
        &rom,
        &mut facts,
        "Phase 2 produced a malformed recovered mapping",
        limits.vrom_materialization.max_decoded_file_bytes,
    );
    Ok(AutoDiscovery {
        rom,
        facts,
        selected,
        outcomes,
    })
}

/// Run discovery from a serializable external evidence manifest. The
/// manifest is checked against the normalized ROM SHA-256 before any claim is
/// consumed. It may describe mappings and executable intervals, but never
/// function answers; those remain outputs of the discovery pipeline.
pub fn run_discovery_with_manifest(
    rom_bytes: &[u8],
    manifest: &evidence::EvidenceManifest,
) -> Result<(NormalizedRom, FactDb), DiscoveryError> {
    let (rom, db, _report) = run_discovery_with_manifest_and_request_dma(rom_bytes, manifest, &[])?;
    Ok((rom, db))
}

/// [`run_discovery_with_manifest`] plus a static request-DMA scan between
/// mapping and executable evidence, so executable ranges may bind to banks
/// this scan proves (which have no serializable in-ROM table to carry them
/// through the manifest).
pub fn run_discovery_with_manifest_and_request_dma(
    rom_bytes: &[u8],
    manifest: &evidence::EvidenceManifest,
    request_dma: &[banks::StaticRequestDmaInput],
) -> Result<(NormalizedRom, FactDb, banks::StaticRequestDmaReport), DiscoveryError> {
    let rom = rom::normalize(rom_bytes).map_err(DiscoveryError::Rom)?;
    manifest
        .validate_identity(&rom)
        .map_err(DiscoveryError::Evidence)?;
    let mut db = FactDb::new();
    let _boot = banks::discover_boot_bank(&rom, &mut db);
    evidence::apply_mapping_evidence(&rom, manifest, &mut db).map_err(DiscoveryError::Evidence)?;
    let report = banks::scan_static_request_dma(&rom, request_dma, &mut db);
    evidence::apply_executable_evidence(manifest, &mut db).map_err(DiscoveryError::Evidence)?;
    harvest::harvest_discovered_candidates(&rom, &mut db).map_err(DiscoveryError::Harvest)?;
    Ok((rom, db, report))
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
    fn auto_discovery_reports_every_strategy_even_when_none_corroborate() {
        // A ROM with an unknown synthetic IPL3 and no overlay geometry. The
        // point is not that it recovers nothing -- it is that recovering
        // nothing is REPORTED. A quiet boot-bank-only artifact is otherwise
        // indistinguishable from a successful composition.
        let auto = run_discovery_auto(&make_test_rom()).expect("synthetic ROM normalizes");

        assert_eq!(auto.selected, DiscoveryStrategy::BootBankOpen);
        assert_eq!(
            auto.outcomes
                .iter()
                .map(|outcome| outcome.strategy)
                .collect::<Vec<_>>(),
            vec![
                DiscoveryStrategy::BootBankOpen,
                DiscoveryStrategy::RecoveredVrom,
                DiscoveryStrategy::RecoveredOverlays,
                DiscoveryStrategy::UntabledDeltaVote,
            ],
            "every strategy attempted must report an outcome, in evaluation order"
        );
        for outcome in &auto.outcomes {
            assert_eq!(
                outcome.admitted_tables, 0,
                "{:?} admitted a table on a ROM with no overlay geometry",
                outcome.strategy
            );
        }
        assert_eq!(auto.facts.proven_rom_mappings().len(), 0);
        assert_eq!(
            auto.facts.conclusion("bank:boot").unwrap().state,
            ProofState::Open
        );
    }

    #[test]
    fn auto_discovery_normalizes_and_harvests_once() {
        AUTO_DISCOVERY_WORK.with(|work| work.set(AutoDiscoveryWork::default()));

        let bytes = make_test_rom();
        let auto = run_discovery_auto(&bytes).expect("synthetic ROM normalizes");
        let work = AUTO_DISCOVERY_WORK.with(std::cell::Cell::get);

        assert_eq!(
            work,
            AutoDiscoveryWork {
                normalizations: 1,
                harvests: 1,
            }
        );
        let (_, direct) = run_discovery(&bytes, None).expect("synthetic ROM normalizes");
        assert_eq!(
            serde_json::to_vec(&auto.facts).unwrap(),
            serde_json::to_vec(&direct).unwrap(),
            "single-pass auto discovery must retain the direct boot strategy byte-for-byte"
        );
    }

    #[test]
    fn auto_discovery_selection_is_deterministic() {
        // Selection must not depend on map iteration order or anything else
        // that varies run to run; a gate that pins a strategy is worthless if
        // the strategy can change underneath it.
        let rom = make_test_rom();
        let first = run_discovery_auto(&rom).expect("synthetic ROM normalizes");
        let first_facts = serde_json::to_vec(&first.facts).unwrap();
        for _ in 0..3 {
            let again = run_discovery_auto(&rom).expect("synthetic ROM normalizes");
            assert_eq!(first.selected, again.selected);
            assert_eq!(first.outcomes, again.outcomes);
            assert_eq!(first_facts, serde_json::to_vec(&again.facts).unwrap());
        }
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
    fn run_discovery_without_recognized_ipl3_keeps_boot_bank_open() {
        let bytes = make_test_rom();
        let (_rom, db) = run_discovery(&bytes, None).unwrap();
        assert_eq!(db.conclusion("bank:boot").unwrap().state, ProofState::Open);
        assert!(db.proven_function_entries("boot").is_empty());
        assert!(!db
            .facts()
            .iter()
            .any(|fact| matches!(fact, Fact::RomMapping { bank, .. } if bank == "boot")));
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
        // carrying the descriptor table, and one carrying all overlays.
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

        // The 0x1c run contains all three records while the configured 0x38
        // stride also finds records zero and two. Exact record geometries
        // shared by those distinct admitted tables must still mint one bank.
        let descriptors = [
            (0x6000, 0x6800, 0x8002_0000),
            (0x7000, 0x7800, 0x8003_0000),
            (0x8000, 0x8800, 0x8004_0000),
        ];
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
            file_table_name: "recovered_file_table".into(),
            table_name: "recovered_vrom_overlay_descriptors".into(),
            bank_name: banks::BankNamePattern::new("recovered_overlay_", 0, ""),
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
        assert_eq!(recovery.admitted_intervals().len(), 3);
        assert!(
            recovery
                .admissions
                .iter()
                .filter(|admission| admission.admitted)
                .flat_map(|admission| &admission.table.records)
                .count()
                > recovery.admitted_intervals().len(),
            "fixture must contain exact records repeated across admitted tables"
        );
        let overlay_mappings: Vec<_> = db
            .proven_rom_mappings()
            .into_iter()
            .filter(
                |fact| matches!(fact, Fact::RomMapping { bank, .. } if bank != banks::BOOT_BANK),
            )
            .collect();
        assert_eq!(overlay_mappings.len(), 3);
        assert!(overlay_mappings.iter().enumerate().all(|(index, fact)| {
            matches!(fact, Fact::RomMapping { bank, .. } if bank == &format!("recovered_overlay_{index}"))
        }));
        assert!(overlay_mappings.iter().all(|fact| matches!(
            fact,
            Fact::RomMapping {
                rom_space: RomAddressSpace::Virtual,
                ..
            }
        )));

        AUTO_DISCOVERY_WORK.with(|work| work.set(AutoDiscoveryWork::default()));
        let auto = run_discovery_auto(&bytes).expect("synthetic VROM ROM normalizes");
        assert_eq!(auto.selected, DiscoveryStrategy::RecoveredVrom);
        assert_eq!(
            AUTO_DISCOVERY_WORK.with(std::cell::Cell::get),
            AutoDiscoveryWork {
                normalizations: 1,
                harvests: 1,
            }
        );
        assert_eq!(
            serde_json::to_vec(&auto.facts).unwrap(),
            serde_json::to_vec(&db).unwrap(),
            "single-pass auto discovery must retain the selected VROM strategy byte-for-byte"
        );
    }

    #[test]
    fn auto_discovery_reports_oversized_vrom_file_without_harvesting_it() {
        const HUGE_FILE_BYTES: u32 = 0x0800_0000;
        let mut bytes = vec![0u8; 0xf000];
        put_u32(&mut bytes, 0, 0x8037_1240);
        put_u32(&mut bytes, 8, 0x8000_0400);
        for (index, fields) in [
            [0x0000, 0x3000, 0x0000, 0x0000],
            [0x3000, 0x6000, 0x8000, 0x0000],
            [0x6000, 0x9000, 0xb000, 0x0000],
            [0x9000, 0x9000 + HUGE_FILE_BYTES, 0xe000, 0xe010],
        ]
        .into_iter()
        .enumerate()
        {
            for (field, value) in fields.into_iter().enumerate() {
                put_u32(&mut bytes, 0x2000 + index * 0x10 + field * 4, value);
            }
        }
        for (index, (vrom_start, vrom_end, vram)) in
            [(0x6000, 0x6800, 0x8002_0000), (0x7000, 0x7800, 0x8003_0000)]
                .into_iter()
                .enumerate()
        {
            let base = 0x8000 + index * 0x1c;
            put_u32(&mut bytes, base, vrom_start);
            put_u32(&mut bytes, base + 4, vrom_end);
            put_u32(&mut bytes, base + 8, vram);
            let physical = 0xb000 + (vrom_start - 0x6000) as usize;
            plant_delta_admissible_region(&mut bytes, physical, vram);
        }
        bytes[0xe000..0xe004].copy_from_slice(b"Yaz0");
        put_u32(&mut bytes, 0xe004, HUGE_FILE_BYTES);

        let auto = run_discovery_auto_with_limits(
            &bytes,
            AutoDiscoveryLimits {
                vrom_materialization: file_table::VromMaterializationLimits {
                    max_decoded_file_bytes: 0x4000,
                },
            },
        )
        .expect("bounded synthetic VROM ROM normalizes");
        let vrom = auto
            .outcomes
            .iter()
            .find(|outcome| outcome.strategy == DiscoveryStrategy::RecoveredVrom)
            .expect("VROM strategy reports an outcome");
        assert_eq!(vrom.decoded_file_limit_hits, 1);
        assert_eq!(auto.selected, DiscoveryStrategy::RecoveredVrom);
        assert!(!auto.facts.proven_rom_mappings().iter().any(
            |fact| matches!(fact, Fact::RomMapping { bank, .. } if bank.starts_with("request_dma_"))
        ));
    }
}
