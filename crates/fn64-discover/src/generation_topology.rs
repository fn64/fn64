//! ROM-bound catalog geometry for offline diagnostics.
//!
//! This derives topology only. It never claims that a generation was active.

use crate::dense_aot_pack::{
    build_dense_aot_pack_v1, DenseAotGenerationInput, DenseAotGenerationV1, DenseAotPackV1,
    DENSE_AOT_PACK_SCHEMA_V1,
};
use crate::overlay_recipe::{OverlayLoadRecipeV1, OVERLAY_RECIPE_SCHEMA_V1};
use crate::NormalizedRom;
use fn64_recomp_rs::{
    decode, BackedGenerationCatalogEvidenceV1, BackedPrecompiledGenerationCatalogV1, Instruction,
    BACKED_GENERATION_CATALOG_EVIDENCE_SCHEMA_V1,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const GENERATION_TOPOLOGY_SCHEMA_V1: &str = "fn64.generation-topology.v1";
pub const CATALOG_BOUND_EXACT_TRANSFER_SCHEMA_V1: &str = "fn64.catalog-bound-exact-transfer.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExactTransferKindV1 {
    Call,
    Jump,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExactTransferRequestV1 {
    pub source_bank: String,
    pub source_pc: u32,
    pub kind: ExactTransferKindV1,
    pub target_pc: u32,
}

/// Move-only authority for one exact cross-generation transfer. Its fields are
/// private so diagnostic geometry or a serialized runtime observation cannot
/// be promoted into composition authority.
#[derive(Debug)]
pub struct CatalogBoundExactTransferV1 {
    schema: &'static str,
    normalized_rom_sha256: String,
    dense_pack_sha256: [u8; 32],
    topology_sha256: String,
    catalog_definition_sha256: [u8; 32],
    source_generation: u64,
    source_bank: String,
    source_pc: u32,
    kind: ExactTransferKindV1,
    target_pc: u32,
    target_generation: u64,
    target_bank: String,
}

impl CatalogBoundExactTransferV1 {
    pub(crate) fn exact_edge(&self) -> (&str, u32, ExactTransferKindV1, u32) {
        (&self.source_bank, self.source_pc, self.kind, self.target_pc)
    }

    pub(crate) fn selected_target(&self) -> (&str, u64) {
        (&self.target_bank, self.target_generation)
    }

    pub(crate) fn normalized_rom_sha256(&self) -> &str {
        &self.normalized_rom_sha256
    }

    pub(crate) fn matches_composition_identity(
        &self,
        normalized_rom_sha256: &str,
        dense_pack_sha256: [u8; 32],
        topology: &GenerationTopologyV1,
        catalog_definition_sha256: [u8; 32],
    ) -> bool {
        let source_matches = topology.generations.iter().any(|generation| {
            generation.generation_id == self.source_generation
                && generation.materialized_bank == self.source_bank
                && generation.image_start <= self.source_pc
                && self.source_pc < generation.image_end
        });
        let target_matches = topology.generations.iter().any(|generation| {
            generation.generation_id == self.target_generation
                && generation.materialized_bank == self.target_bank
                && generation.image_start <= self.target_pc
                && self.target_pc < generation.image_end
        });
        self.schema == CATALOG_BOUND_EXACT_TRANSFER_SCHEMA_V1
            && self.normalized_rom_sha256 == normalized_rom_sha256
            && self.dense_pack_sha256 == dense_pack_sha256
            && self.topology_sha256 == topology.topology_sha256
            && self.catalog_definition_sha256 == catalog_definition_sha256
            && source_matches
            && target_matches
    }

    pub fn target_bank(&self) -> &str {
        &self.target_bank
    }

    pub fn target_generation(&self) -> u64 {
        self.target_generation
    }
}

#[derive(Debug)]
pub enum CatalogBoundExactTransferResolutionV1 {
    Authorized(CatalogBoundExactTransferV1),
    ActivationMiss {
        request: ExactTransferRequestV1,
        excluded_generations: Vec<u64>,
    },
    Ambiguous {
        request: ExactTransferRequestV1,
        compatible_generations: Vec<u64>,
    },
}

/// Validator-owned binding between immutable discovery inputs and one real
/// runtime generation catalog.
///
/// The context can only be constructed from
/// [`BackedPrecompiledGenerationCatalogV1`], so a caller-authored evidence
/// snapshot or digest cannot enter the generic transfer sweep. Runtime active
/// segments remain observational and are excluded from the bound definition.
pub struct ValidatedCatalogBoundExactTransferContextV1<'a> {
    rom: &'a NormalizedRom,
    dense_pack: &'a DenseAotPackV1,
    topology: &'a GenerationTopologyV1,
    catalog: BackedGenerationCatalogEvidenceV1,
    catalog_definition_sha256: [u8; 32],
}

impl ValidatedCatalogBoundExactTransferContextV1<'_> {
    pub fn verify(
        &self,
        request: ExactTransferRequestV1,
    ) -> Result<CatalogBoundExactTransferResolutionV1, CatalogBoundExactTransferErrorV1> {
        verify_catalog_bound_exact_transfer_prevalidated_v1(
            self.rom,
            self.dense_pack,
            self.topology,
            &self.catalog,
            self.catalog_definition_sha256,
            request,
        )
    }

    pub(crate) fn catalog_definition_sha256(&self) -> [u8; 32] {
        self.catalog_definition_sha256
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogBoundExactTransferErrorV1 {
    RomIdentityMismatch,
    TopologyIdentityMismatch,
    DensePackMismatch,
    CatalogSchema,
    CatalogDefinitionDigestMismatch,
    CatalogTopologyMismatch,
    SourceNotExactlyOneGeneration { count: usize },
    SourceBackingIncomplete,
    SourceSiteOutsideImage,
    MissingDelayWord,
    TransferEncodingMismatch,
    ControlOrDelayMayWriteMemory { pc: u32 },
}

impl std::fmt::Display for CatalogBoundExactTransferErrorV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid catalog-bound exact transfer: {self:?}")
    }
}

impl std::error::Error for CatalogBoundExactTransferErrorV1 {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImmutableExecutablePrefixV1 {
    pub bank: String,
    pub va_start: u32,
    pub va_end: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogGenerationRoleV1 {
    ResidentTail,
    Overlay,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CatalogGenerationGeometryV1 {
    pub role: CatalogGenerationRoleV1,
    pub name: String,
    /// Snapshot/materialization bank whose bytes this generation selects.
    pub materialized_bank: String,
    pub generation_id: u64,
    pub image_start: u32,
    pub image_end: u32,
    pub invalidation_start: u32,
    pub invalidation_end: u32,
    pub image_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GenerationTopologyV1 {
    pub schema: String,
    pub normalized_rom_sha256: String,
    /// Digest of this diagnostic topology definition. This is not the runtime
    /// catalog's canonical-definition digest until a later binding validates
    /// the two artifacts against each other.
    pub topology_sha256: String,
    pub immutable_prefix: ImmutableExecutablePrefixV1,
    pub generations: Vec<CatalogGenerationGeometryV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CatalogActiveSegmentV1 {
    pub start: u32,
    pub end: u32,
    pub generation_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CatalogGeometryStateV1 {
    pub active_segments: Vec<CatalogActiveSegmentV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GenerationGeometryStateSpaceV1 {
    pub topology_sha256: String,
    pub states: Vec<CatalogGeometryStateV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerationGeometryStateSpaceLimits {
    pub max_states: usize,
}

impl Default for GenerationGeometryStateSpaceLimits {
    fn default() -> Self {
        Self { max_states: 65_536 }
    }
}

/// Diagnostic geometry closure. It describes states permitted by interval
/// splitting alone, not states proven reachable by guest execution or writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GenerationGeometryAnalysisV1 {
    topology: GenerationTopologyV1,
    state_space: GenerationGeometryStateSpaceV1,
}

impl GenerationGeometryAnalysisV1 {
    pub fn topology(&self) -> &GenerationTopologyV1 {
        &self.topology
    }

    pub fn state_space(&self) -> &GenerationGeometryStateSpaceV1 {
        &self.state_space
    }

    /// A diagnostic negative filter only. `true` means interval geometry does
    /// not rule coexistence out; it is not activation or reachability proof.
    pub fn pcs_may_coexist_by_geometry_v1(
        &self,
        source_bank: &str,
        source_pc: u32,
        target_bank: &str,
        target_pc: u32,
    ) -> bool {
        self.state_space.states.iter().any(|state| {
            state_owns(&self.topology, state, source_bank, source_pc)
                && state_owns(&self.topology, state, target_bank, target_pc)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenerationTopologyError {
    DensePackSchema,
    RomIdentityMismatch,
    NoResidentGeneration,
    NoOverlayGenerations,
    ResidentBankMismatch,
    DensePackMismatch,
    OverlayCountMismatch { dense: usize, recipes: usize },
    DuplicateName { name: String },
    DuplicateGenerationId { generation_id: u64 },
    InvalidDigest { field: &'static str },
    InvalidResidentSplit,
    ResidentTailOutsideRom,
    OverlayRecipeMismatch { index: usize },
    StateLimitExceeded { states: usize, limit: usize },
}

impl std::fmt::Display for GenerationTopologyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid generation topology: {self:?}")
    }
}

impl std::error::Error for GenerationTopologyError {}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Exact resident-tail identity rule used by the selected build. The caller
/// supplies the domain so game-specific historical domains remain explicit.
pub fn resident_tail_generation_id_v1(
    identity_domain: &[u8],
    rom_sha256: &str,
    image_start: u32,
    image_end: u32,
    invalidation_start: u32,
    invalidation_end: u32,
    image_sha256: [u8; 32],
) -> u64 {
    let mut digest = Sha256::new();
    digest.update(identity_domain);
    digest.update((rom_sha256.len() as u64).to_be_bytes());
    digest.update(rom_sha256.as_bytes());
    for value in [image_start, image_end, invalidation_start, invalidation_end] {
        digest.update(value.to_be_bytes());
    }
    digest.update(image_sha256);
    u64::from_be_bytes(digest.finalize()[..8].try_into().unwrap())
}

fn overlay_matches(generation: &DenseAotGenerationV1, recipe: &OverlayLoadRecipeV1) -> bool {
    recipe.schema == OVERLAY_RECIPE_SCHEMA_V1
        && generation.source_rom_start == recipe.rom_start
        && generation.source_rom_end == recipe.rom_end
        && generation.load_start == recipe.load_start
        && generation.load_end == recipe.data_end
        && generation.text_start == recipe.text_start
        && generation.text_end == recipe.text_end
        && generation.data_start == recipe.data_start
        && generation.data_end == recipe.data_end
        && generation.bss_start == recipe.bss_start
        && generation.bss_end == recipe.bss_end
        && generation.loaded_sha256 == recipe.loaded_sha256
}

fn topology_sha256(
    rom_sha256: &str,
    prefix: &ImmutableExecutablePrefixV1,
    generations: &[CatalogGenerationGeometryV1],
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"fn64:generation-topology-catalog:v1:");
    digest.update((rom_sha256.len() as u64).to_be_bytes());
    digest.update(rom_sha256.as_bytes());
    digest.update((prefix.bank.len() as u64).to_be_bytes());
    digest.update(prefix.bank.as_bytes());
    digest.update(prefix.va_start.to_be_bytes());
    digest.update(prefix.va_end.to_be_bytes());
    for generation in generations {
        digest.update([match generation.role {
            CatalogGenerationRoleV1::ResidentTail => 0,
            CatalogGenerationRoleV1::Overlay => 1,
        }]);
        digest.update((generation.name.len() as u64).to_be_bytes());
        digest.update(generation.name.as_bytes());
        digest.update((generation.materialized_bank.len() as u64).to_be_bytes());
        digest.update(generation.materialized_bank.as_bytes());
        digest.update(generation.generation_id.to_be_bytes());
        for value in [
            generation.image_start,
            generation.image_end,
            generation.invalidation_start,
            generation.invalidation_end,
        ] {
            digest.update(value.to_be_bytes());
        }
        digest.update(generation.image_sha256.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

/// Validate and derive the immutable prefix, resident tail, and overlay
/// generation catalog from two independently produced ROM-bound inputs.
pub fn build_generation_topology_v1(
    rom: &NormalizedRom,
    dense_pack: &DenseAotPackV1,
    resident_bank: &str,
    resident_tail_identity_domain: &[u8],
    recipes: &[OverlayLoadRecipeV1],
) -> Result<GenerationTopologyV1, GenerationTopologyError> {
    if dense_pack.schema != DENSE_AOT_PACK_SCHEMA_V1 {
        return Err(GenerationTopologyError::DensePackSchema);
    }
    if dense_pack.normalized_rom_sha256 != rom.sha256 {
        return Err(GenerationTopologyError::RomIdentityMismatch);
    }
    if !valid_sha256(&rom.sha256) {
        return Err(GenerationTopologyError::InvalidDigest { field: "rom" });
    }
    let (resident, overlays) = dense_pack
        .generations
        .split_first()
        .ok_or(GenerationTopologyError::NoResidentGeneration)?;
    if overlays.is_empty() || recipes.is_empty() {
        return Err(GenerationTopologyError::NoOverlayGenerations);
    }
    if overlays.len() != recipes.len() {
        return Err(GenerationTopologyError::OverlayCountMismatch {
            dense: overlays.len(),
            recipes: recipes.len(),
        });
    }
    if resident.name != resident_bank {
        return Err(GenerationTopologyError::ResidentBankMismatch);
    }
    if !valid_sha256(&resident.loaded_sha256) {
        return Err(GenerationTopologyError::InvalidDigest { field: "resident" });
    }
    let split = recipes
        .iter()
        .map(|recipe| recipe.load_start)
        .min()
        .unwrap();
    let invalidation_end = recipes.iter().map(|recipe| recipe.bss_end).max().unwrap();
    if !split.is_multiple_of(4)
        || split <= resident.load_start
        || split >= resident.load_end
        || invalidation_end < resident.load_end
    {
        return Err(GenerationTopologyError::InvalidResidentSplit);
    }
    let tail_rom_start = resident
        .source_rom_start
        .checked_add(split - resident.load_start)
        .ok_or(GenerationTopologyError::ResidentTailOutsideRom)?;
    let tail_bytes = rom
        .bytes
        .get(tail_rom_start as usize..resident.source_rom_end as usize)
        .ok_or(GenerationTopologyError::ResidentTailOutsideRom)?;
    if tail_bytes.len() != (resident.load_end - split) as usize {
        return Err(GenerationTopologyError::ResidentTailOutsideRom);
    }
    let tail_digest: [u8; 32] = Sha256::digest(tail_bytes).into();
    let tail_sha256 = tail_digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut generations = vec![CatalogGenerationGeometryV1 {
        role: CatalogGenerationRoleV1::ResidentTail,
        name: format!("{resident_bank}:resident_tail"),
        materialized_bank: resident_bank.to_owned(),
        generation_id: resident_tail_generation_id_v1(
            resident_tail_identity_domain,
            &rom.sha256,
            split,
            resident.load_end,
            split,
            invalidation_end,
            tail_digest,
        ),
        image_start: split,
        image_end: resident.load_end,
        invalidation_start: split,
        invalidation_end,
        image_sha256: tail_sha256,
    }];
    for (index, (generation, recipe)) in overlays.iter().zip(recipes).enumerate() {
        if !overlay_matches(generation, recipe) {
            return Err(GenerationTopologyError::OverlayRecipeMismatch { index });
        }
        if !valid_sha256(&generation.loaded_sha256) {
            return Err(GenerationTopologyError::InvalidDigest { field: "overlay" });
        }
        generations.push(CatalogGenerationGeometryV1 {
            role: CatalogGenerationRoleV1::Overlay,
            name: generation.name.clone(),
            materialized_bank: generation.name.clone(),
            generation_id: generation.bank_id,
            image_start: generation.load_start,
            image_end: generation.load_end,
            invalidation_start: generation.load_start,
            invalidation_end: generation.bss_end,
            image_sha256: generation.loaded_sha256.clone(),
        });
    }
    let mut expected_inputs = vec![DenseAotGenerationInput {
        name: resident_bank,
        source_rom_start: resident.source_rom_start,
        source_rom_end: resident.source_rom_end,
        load_start: resident.load_start,
        text_start: resident.text_start,
        text_end: resident.text_end,
        data_start: resident.data_start,
        data_end: resident.data_end,
        bss_start: resident.bss_start,
        bss_end: resident.bss_end,
    }];
    expected_inputs.extend(overlays.iter().zip(recipes).map(|(generation, recipe)| {
        DenseAotGenerationInput::from((generation.name.as_str(), recipe))
    }));
    let expected_pack = build_dense_aot_pack_v1(rom, &expected_inputs)
        .map_err(|_| GenerationTopologyError::DensePackMismatch)?;
    if &expected_pack != dense_pack {
        return Err(GenerationTopologyError::DensePackMismatch);
    }
    generations.sort_by_key(|generation| {
        (
            generation.role,
            generation.generation_id,
            generation.name.clone(),
        )
    });
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for generation in &generations {
        if !names.insert(generation.name.clone()) {
            return Err(GenerationTopologyError::DuplicateName {
                name: generation.name.clone(),
            });
        }
        if !ids.insert(generation.generation_id) {
            return Err(GenerationTopologyError::DuplicateGenerationId {
                generation_id: generation.generation_id,
            });
        }
    }
    let immutable_prefix = ImmutableExecutablePrefixV1 {
        bank: resident_bank.to_owned(),
        va_start: resident.load_start,
        va_end: split,
    };
    Ok(GenerationTopologyV1 {
        schema: GENERATION_TOPOLOGY_SCHEMA_V1.to_owned(),
        normalized_rom_sha256: rom.sha256.clone(),
        topology_sha256: topology_sha256(&rom.sha256, &immutable_prefix, &generations),
        immutable_prefix,
        generations,
    })
}

fn activate(
    state: &CatalogGeometryStateV1,
    generation: &CatalogGenerationGeometryV1,
) -> CatalogGeometryStateV1 {
    let mut segments = Vec::with_capacity(state.active_segments.len() + 2);
    for active in &state.active_segments {
        if active.end <= generation.invalidation_start
            || active.start >= generation.invalidation_end
        {
            segments.push(*active);
            continue;
        }
        if active.start < generation.invalidation_start {
            segments.push(CatalogActiveSegmentV1 {
                start: active.start,
                end: generation.invalidation_start,
                generation_id: active.generation_id,
            });
        }
        if active.end > generation.invalidation_end {
            segments.push(CatalogActiveSegmentV1 {
                start: generation.invalidation_end,
                end: active.end,
                generation_id: active.generation_id,
            });
        }
    }
    segments.push(CatalogActiveSegmentV1 {
        start: generation.image_start,
        end: generation.image_end,
        generation_id: generation.generation_id,
    });
    segments.sort_unstable();
    CatalogGeometryStateV1 {
        active_segments: segments,
    }
}

/// Enumerate geometry-possible interval states from a hypothetical resident
/// tail. This mirrors split/invalidate geometry but does not prove the runtime
/// starts in, or can transition to, any state.
fn enumerate_geometry_states_v1(
    topology: &GenerationTopologyV1,
    limits: GenerationGeometryStateSpaceLimits,
) -> Result<GenerationGeometryStateSpaceV1, GenerationTopologyError> {
    let resident = topology
        .generations
        .iter()
        .find(|generation| generation.role == CatalogGenerationRoleV1::ResidentTail)
        .ok_or(GenerationTopologyError::NoResidentGeneration)?;
    let initial = CatalogGeometryStateV1 {
        active_segments: vec![CatalogActiveSegmentV1 {
            start: resident.image_start,
            end: resident.image_end,
            generation_id: resident.generation_id,
        }],
    };
    if limits.max_states == 0 {
        return Err(GenerationTopologyError::StateLimitExceeded {
            states: 1,
            limit: 0,
        });
    }
    let mut seen = BTreeSet::from([initial.clone()]);
    let mut pending = std::collections::VecDeque::from([initial]);
    while let Some(state) = pending.pop_front() {
        for generation in &topology.generations {
            let next = activate(&state, generation);
            if seen.insert(next.clone()) {
                if seen.len() > limits.max_states {
                    return Err(GenerationTopologyError::StateLimitExceeded {
                        states: seen.len(),
                        limit: limits.max_states,
                    });
                }
                pending.push_back(next);
            }
        }
    }
    Ok(GenerationGeometryStateSpaceV1 {
        topology_sha256: topology.topology_sha256.clone(),
        states: seen.into_iter().collect(),
    })
}

fn state_owns(
    topology: &GenerationTopologyV1,
    state: &CatalogGeometryStateV1,
    bank: &str,
    pc: u32,
) -> bool {
    if topology.immutable_prefix.bank == bank
        && topology.immutable_prefix.va_start <= pc
        && pc < topology.immutable_prefix.va_end
    {
        return true;
    }
    state.active_segments.iter().any(|segment| {
        segment.start <= pc
            && pc < segment.end
            && topology.generations.iter().any(|generation| {
                generation.generation_id == segment.generation_id
                    && generation.materialized_bank == bank
                    && generation.image_start <= pc
                    && pc < generation.image_end
            })
    })
}

/// Rebuild the ROM-bound topology and its bounded geometry-only state space.
pub fn build_generation_geometry_analysis_v1(
    rom: &NormalizedRom,
    dense_pack: &DenseAotPackV1,
    resident_bank: &str,
    resident_tail_identity_domain: &[u8],
    recipes: &[OverlayLoadRecipeV1],
    limits: GenerationGeometryStateSpaceLimits,
) -> Result<GenerationGeometryAnalysisV1, GenerationTopologyError> {
    let topology = build_generation_topology_v1(
        rom,
        dense_pack,
        resident_bank,
        resident_tail_identity_domain,
        recipes,
    )?;
    let state_space = enumerate_geometry_states_v1(&topology, limits)?;
    Ok(GenerationGeometryAnalysisV1 {
        topology,
        state_space,
    })
}

pub fn dense_aot_pack_sha256_v1(pack: &DenseAotPackV1) -> [u8; 32] {
    let bytes = serde_json::to_vec(pack).expect("dense AOT pack serialization is infallible");
    let mut digest = Sha256::new();
    digest.update(b"fn64:dense-aot-pack-canonical:v1:");
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    digest.finalize().into()
}

fn catalog_definition_sha256_v1(evidence: &BackedGenerationCatalogEvidenceV1) -> [u8; 32] {
    fn update_len(digest: &mut Sha256, len: usize) {
        digest.update((len as u64).to_be_bytes());
    }
    let mut digest = Sha256::new();
    let schema = BACKED_GENERATION_CATALOG_EVIDENCE_SCHEMA_V1.as_bytes();
    update_len(&mut digest, schema.len());
    digest.update(schema);
    update_len(&mut digest, evidence.generations.len());
    for generation in &evidence.generations {
        digest.update(generation.generation.get().to_be_bytes());
        digest.update(generation.image_start.get().to_be_bytes());
        digest.update(generation.image_end.get().to_be_bytes());
        digest.update(generation.invalidation_start.get().to_be_bytes());
        digest.update(generation.invalidation_end.get().to_be_bytes());
        digest.update(generation.expected_sha256);
        update_len(&mut digest, generation.shards.len());
        for shard in &generation.shards {
            digest.update(shard.bank().get().to_be_bytes());
            digest.update(shard.start().get().to_be_bytes());
            digest.update(shard.end().get().to_be_bytes());
        }
    }
    update_len(&mut digest, evidence.backings.len());
    for backing in &evidence.backings {
        digest.update(backing.generation.get().to_be_bytes());
        update_len(&mut digest, backing.spans.len());
        for span in &backing.spans {
            digest.update(span.virtual_start().get().to_be_bytes());
            digest.update(span.physical_start().to_be_bytes());
            digest.update(span.byte_len().to_be_bytes());
        }
    }
    digest.finalize().into()
}

fn parse_sha256(value: &str) -> Option<[u8; 32]> {
    if !valid_sha256(value) {
        return None;
    }
    let mut result = [0; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(result)
}

fn generation_bytes<'a>(
    rom: &'a NormalizedRom,
    dense_pack: &DenseAotPackV1,
    topology: &GenerationTopologyV1,
    generation: &CatalogGenerationGeometryV1,
) -> Option<&'a [u8]> {
    let dense = if generation.role == CatalogGenerationRoleV1::ResidentTail {
        dense_pack.generations.first()?
    } else {
        dense_pack
            .generations
            .iter()
            .find(|dense| dense.name == generation.materialized_bank)?
    };
    let offset = generation.image_start.checked_sub(dense.load_start)?;
    let rom_start = dense.source_rom_start.checked_add(offset)?;
    let byte_len = generation.image_end.checked_sub(generation.image_start)?;
    let rom_end = rom_start.checked_add(byte_len)?;
    let bytes = rom.bytes.get(rom_start as usize..rom_end as usize)?;
    (Sha256::digest(bytes).as_slice() == parse_sha256(&generation.image_sha256)?.as_slice()
        && topology.normalized_rom_sha256 == rom.sha256)
        .then_some(bytes)
}

fn backing_physical_at(
    evidence: &BackedGenerationCatalogEvidenceV1,
    generation_id: u64,
    va: u32,
) -> Option<u32> {
    evidence
        .backings
        .iter()
        .find(|backing| backing.generation.get() == generation_id)?
        .spans
        .iter()
        .find_map(|span| {
            let start = span.virtual_start().get();
            let end = start.checked_add(span.byte_len())?;
            (start <= va && va < end).then(|| span.physical_start() + (va - start))
        })
}

fn backing_covers_invalidation(
    evidence: &BackedGenerationCatalogEvidenceV1,
    generation: &CatalogGenerationGeometryV1,
) -> bool {
    let Some(backing) = evidence
        .backings
        .iter()
        .find(|backing| backing.generation.get() == generation.generation_id)
    else {
        return false;
    };
    let mut cursor = generation.invalidation_start;
    for span in &backing.spans {
        if span.virtual_start().get() != cursor {
            return false;
        }
        let Some(end) = cursor.checked_add(span.byte_len()) else {
            return false;
        };
        cursor = end;
    }
    cursor == generation.invalidation_end
}

fn topology_matches_dense_and_catalog(
    rom: &NormalizedRom,
    dense_pack: &DenseAotPackV1,
    topology: &GenerationTopologyV1,
    catalog: &BackedGenerationCatalogEvidenceV1,
) -> bool {
    if topology.schema != GENERATION_TOPOLOGY_SCHEMA_V1
        || topology.normalized_rom_sha256 != rom.sha256
        || dense_pack.schema != DENSE_AOT_PACK_SCHEMA_V1
        || dense_pack.normalized_rom_sha256 != rom.sha256
        || topology.topology_sha256
            != topology_sha256(
                &topology.normalized_rom_sha256,
                &topology.immutable_prefix,
                &topology.generations,
            )
        || catalog.schema != BACKED_GENERATION_CATALOG_EVIDENCE_SCHEMA_V1
        || topology.generations.len() != catalog.generations.len()
        || topology.generations.len() != catalog.backings.len()
    {
        return false;
    }
    topology.generations.iter().all(|generation| {
        let Some(runtime) = catalog
            .generations
            .iter()
            .find(|runtime| runtime.generation.get() == generation.generation_id)
        else {
            return false;
        };
        runtime.image_start.get() == generation.image_start
            && runtime.image_end.get() == generation.image_end
            && runtime.invalidation_start.get() == generation.invalidation_start
            && runtime.invalidation_end.get() == generation.invalidation_end
            && Some(runtime.expected_sha256) == parse_sha256(&generation.image_sha256)
            && generation_bytes(rom, dense_pack, topology, generation).is_some()
            && backing_covers_invalidation(catalog, generation)
    })
}

fn instruction_may_write_memory(instruction: Instruction) -> bool {
    matches!(
        instruction,
        Instruction::Sb { .. }
            | Instruction::Sh { .. }
            | Instruction::Sw { .. }
            | Instruction::Swl { .. }
            | Instruction::Swr { .. }
            | Instruction::Sd { .. }
            | Instruction::Sdl { .. }
            | Instruction::Sdr { .. }
            | Instruction::Sc { .. }
            | Instruction::Scd { .. }
            | Instruction::Swc1 { .. }
            | Instruction::Sdc1 { .. }
            | Instruction::Unknown { .. }
    )
}

fn direct_transfer_matches(word: u32, pc: u32, request: &ExactTransferRequestV1) -> bool {
    let opcode = word >> 26;
    let kind_matches = match request.kind {
        ExactTransferKindV1::Call => opcode == 3,
        ExactTransferKindV1::Jump => opcode == 2,
    };
    let target = (pc.wrapping_add(4) & 0xf000_0000) | ((word & 0x03ff_ffff) << 2);
    kind_matches && target == request.target_pc
}

fn generations_conflict_on_physical_byte(
    rom: &NormalizedRom,
    dense_pack: &DenseAotPackV1,
    topology: &GenerationTopologyV1,
    catalog: &BackedGenerationCatalogEvidenceV1,
    source: &CatalogGenerationGeometryV1,
    target: &CatalogGenerationGeometryV1,
) -> bool {
    if source.generation_id == target.generation_id {
        return false;
    }
    let Some(source_bytes) = generation_bytes(rom, dense_pack, topology, source) else {
        return false;
    };
    let Some(target_bytes) = generation_bytes(rom, dense_pack, topology, target) else {
        return false;
    };
    let source_physical = source_bytes
        .iter()
        .enumerate()
        .filter_map(|(offset, byte)| {
            let va = source
                .image_start
                .checked_add(u32::try_from(offset).ok()?)?;
            Some((
                backing_physical_at(catalog, source.generation_id, va)?,
                *byte,
            ))
        })
        .collect::<BTreeMap<_, _>>();
    target_bytes.iter().enumerate().any(|(offset, byte)| {
        let Some(va) = target
            .image_start
            .checked_add(u32::try_from(offset).ok().unwrap_or(u32::MAX))
        else {
            return false;
        };
        let Some(physical) = backing_physical_at(catalog, target.generation_id, va) else {
            return false;
        };
        source_physical
            .get(&physical)
            .is_some_and(|source_byte| source_byte != byte)
    })
}

/// Verify one direct transfer against the immutable ROM, dense pack, topology,
/// and backed runtime catalog definition. Runtime activation observations and
/// geometry-state enumeration are intentionally absent from this constructor.
pub fn validate_catalog_bound_exact_transfer_context_v1<'a>(
    rom: &'a NormalizedRom,
    dense_pack: &'a DenseAotPackV1,
    topology: &'a GenerationTopologyV1,
    catalog: &BackedPrecompiledGenerationCatalogV1,
) -> Result<ValidatedCatalogBoundExactTransferContextV1<'a>, CatalogBoundExactTransferErrorV1> {
    let evidence = catalog.evidence_snapshot();
    let definition = catalog.canonical_definition_sha256();
    validate_catalog_bound_exact_transfer_inputs_v1(
        rom, dense_pack, topology, &evidence, definition,
    )?;
    Ok(ValidatedCatalogBoundExactTransferContextV1 {
        rom,
        dense_pack,
        topology,
        catalog: evidence,
        catalog_definition_sha256: definition,
    })
}

// Evidence-only fixtures may exercise the verifier's classification rules, but
// production capability minting must enter through
// `validate_catalog_bound_exact_transfer_context_v1` and therefore a real
// `BackedPrecompiledGenerationCatalogV1`. Keeping this wrapper test-only makes
// a caller-authored evidence snapshot structurally unable to mint authority.
#[cfg(test)]
fn verify_catalog_bound_exact_transfer_v1(
    rom: &NormalizedRom,
    dense_pack: &DenseAotPackV1,
    topology: &GenerationTopologyV1,
    catalog: &BackedGenerationCatalogEvidenceV1,
    catalog_definition_sha256: [u8; 32],
    request: ExactTransferRequestV1,
) -> Result<CatalogBoundExactTransferResolutionV1, CatalogBoundExactTransferErrorV1> {
    validate_catalog_bound_exact_transfer_inputs_v1(
        rom,
        dense_pack,
        topology,
        catalog,
        catalog_definition_sha256,
    )?;
    verify_catalog_bound_exact_transfer_prevalidated_v1(
        rom,
        dense_pack,
        topology,
        catalog,
        catalog_definition_sha256,
        request,
    )
}

fn validate_catalog_bound_exact_transfer_inputs_v1(
    rom: &NormalizedRom,
    dense_pack: &DenseAotPackV1,
    topology: &GenerationTopologyV1,
    catalog: &BackedGenerationCatalogEvidenceV1,
    catalog_definition_sha256: [u8; 32],
) -> Result<(), CatalogBoundExactTransferErrorV1> {
    if dense_pack.normalized_rom_sha256 != rom.sha256
        || topology.normalized_rom_sha256 != rom.sha256
    {
        return Err(CatalogBoundExactTransferErrorV1::RomIdentityMismatch);
    }
    if topology.topology_sha256
        != topology_sha256(
            &topology.normalized_rom_sha256,
            &topology.immutable_prefix,
            &topology.generations,
        )
    {
        return Err(CatalogBoundExactTransferErrorV1::TopologyIdentityMismatch);
    }
    if dense_pack.schema != DENSE_AOT_PACK_SCHEMA_V1 {
        return Err(CatalogBoundExactTransferErrorV1::DensePackMismatch);
    }
    if catalog.schema != BACKED_GENERATION_CATALOG_EVIDENCE_SCHEMA_V1 {
        return Err(CatalogBoundExactTransferErrorV1::CatalogSchema);
    }
    if catalog_definition_sha256_v1(catalog) != catalog_definition_sha256 {
        return Err(CatalogBoundExactTransferErrorV1::CatalogDefinitionDigestMismatch);
    }
    if !topology_matches_dense_and_catalog(rom, dense_pack, topology, catalog) {
        return Err(CatalogBoundExactTransferErrorV1::CatalogTopologyMismatch);
    }
    Ok(())
}

fn verify_catalog_bound_exact_transfer_prevalidated_v1(
    rom: &NormalizedRom,
    dense_pack: &DenseAotPackV1,
    topology: &GenerationTopologyV1,
    catalog: &BackedGenerationCatalogEvidenceV1,
    catalog_definition_sha256: [u8; 32],
    request: ExactTransferRequestV1,
) -> Result<CatalogBoundExactTransferResolutionV1, CatalogBoundExactTransferErrorV1> {
    let source_generations = topology
        .generations
        .iter()
        .filter(|generation| {
            generation.materialized_bank == request.source_bank
                && generation.image_start <= request.source_pc
                && request.source_pc < generation.image_end
        })
        .collect::<Vec<_>>();
    let [source] = source_generations.as_slice() else {
        return Err(
            CatalogBoundExactTransferErrorV1::SourceNotExactlyOneGeneration {
                count: source_generations.len(),
            },
        );
    };
    if !backing_covers_invalidation(catalog, source) {
        return Err(CatalogBoundExactTransferErrorV1::SourceBackingIncomplete);
    }
    let source_bytes = generation_bytes(rom, dense_pack, topology, source)
        .ok_or(CatalogBoundExactTransferErrorV1::SourceSiteOutsideImage)?;
    let offset = usize::try_from(request.source_pc - source.image_start)
        .map_err(|_| CatalogBoundExactTransferErrorV1::SourceSiteOutsideImage)?;
    let control = source_bytes
        .get(offset..offset + 4)
        .ok_or(CatalogBoundExactTransferErrorV1::SourceSiteOutsideImage)?;
    let delay = source_bytes
        .get(offset + 4..offset + 8)
        .ok_or(CatalogBoundExactTransferErrorV1::MissingDelayWord)?;
    let control = u32::from_be_bytes(control.try_into().unwrap());
    let delay = u32::from_be_bytes(delay.try_into().unwrap());
    if !direct_transfer_matches(control, request.source_pc, &request) {
        return Err(CatalogBoundExactTransferErrorV1::TransferEncodingMismatch);
    }
    for (pc, word) in [(request.source_pc, control), (request.source_pc + 4, delay)] {
        if instruction_may_write_memory(decode(word)) {
            return Err(CatalogBoundExactTransferErrorV1::ControlOrDelayMayWriteMemory { pc });
        }
    }

    let targets = topology
        .generations
        .iter()
        .filter(|generation| {
            generation.image_start <= request.target_pc && request.target_pc < generation.image_end
        })
        .collect::<Vec<_>>();
    let mut excluded_generations = Vec::new();
    let mut compatible = Vec::new();
    for target in targets {
        if generations_conflict_on_physical_byte(rom, dense_pack, topology, catalog, source, target)
        {
            excluded_generations.push(target.generation_id);
        } else {
            compatible.push(target);
        }
    }
    excluded_generations.sort_unstable();
    compatible.sort_by_key(|generation| generation.generation_id);
    match compatible.as_slice() {
        [target] => Ok(CatalogBoundExactTransferResolutionV1::Authorized(
            CatalogBoundExactTransferV1 {
                schema: CATALOG_BOUND_EXACT_TRANSFER_SCHEMA_V1,
                normalized_rom_sha256: rom.sha256.clone(),
                dense_pack_sha256: dense_aot_pack_sha256_v1(dense_pack),
                topology_sha256: topology.topology_sha256.clone(),
                catalog_definition_sha256,
                source_generation: source.generation_id,
                source_bank: request.source_bank,
                source_pc: request.source_pc,
                kind: request.kind,
                target_pc: request.target_pc,
                target_generation: target.generation_id,
                target_bank: target.materialized_bank.clone(),
            },
        )),
        [] => Ok(CatalogBoundExactTransferResolutionV1::ActivationMiss {
            request,
            excluded_generations,
        }),
        many => Ok(CatalogBoundExactTransferResolutionV1::Ambiguous {
            request,
            compatible_generations: many
                .iter()
                .map(|generation| generation.generation_id)
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dense_aot_pack::{build_dense_aot_pack_v1, DenseAotGenerationInput};
    use crate::facts::{
        function_entry_subject, BankAddr, CandidateDetector, Fact, FactDb, FunctionEntryEvidence,
        ProloguePattern, ProofState, RomAddressSpace,
    };
    use crate::snapshot::{compose_materialized_banks_catalog_bound_v1, MaterializedBankInput};
    use fn64_recomp_rs::{
        BackedExecutableSpanV1, BankId, GuestPc, PrecompiledGenerationBackingEvidenceV1,
        PrecompiledGenerationEvidenceV1, PrecompiledShard,
    };

    const BOOT: u32 = 0x8000_0400;
    const OVERLAY: u32 = 0x8000_1400;
    const RESIDENT_ID_DOMAIN: &[u8] = b"fn64:wm2000-resident-tail-generation:v1:";

    fn fixture() -> (NormalizedRom, DenseAotPackV1, Vec<OverlayLoadRecipeV1>) {
        let mut raw = vec![0u8; 0x5000];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&BOOT.to_be_bytes());
        for (index, byte) in raw[0x1000..0x3040].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(17);
        }
        let rom = crate::normalize(&raw).unwrap();
        let inputs = [
            DenseAotGenerationInput {
                name: "boot",
                source_rom_start: 0x1000,
                source_rom_end: 0x3000,
                load_start: BOOT,
                text_start: BOOT,
                text_end: BOOT + 0x2000,
                data_start: BOOT + 0x2000,
                data_end: BOOT + 0x2000,
                bss_start: BOOT + 0x2000,
                bss_end: BOOT + 0x2000,
            },
            DenseAotGenerationInput {
                name: "overlay_a",
                source_rom_start: 0x3000,
                source_rom_end: 0x3020,
                load_start: OVERLAY,
                text_start: OVERLAY,
                text_end: OVERLAY + 0x10,
                data_start: OVERLAY + 0x10,
                data_end: OVERLAY + 0x20,
                bss_start: OVERLAY + 0x20,
                bss_end: BOOT + 0x2040,
            },
            DenseAotGenerationInput {
                name: "overlay_b",
                source_rom_start: 0x3020,
                source_rom_end: 0x3040,
                load_start: OVERLAY + 0x40,
                text_start: OVERLAY + 0x40,
                text_end: OVERLAY + 0x50,
                data_start: OVERLAY + 0x50,
                data_end: OVERLAY + 0x60,
                bss_start: OVERLAY + 0x60,
                bss_end: BOOT + 0x2080,
            },
        ];
        let pack = build_dense_aot_pack_v1(&rom, &inputs).unwrap();
        let recipes = pack
            .generations
            .iter()
            .skip(1)
            .enumerate()
            .map(|(index, generation)| OverlayLoadRecipeV1 {
                schema: OVERLAY_RECIPE_SCHEMA_V1.to_owned(),
                descriptor_rom_offset: 0x200 + index as u32 * 0x24,
                rom_start: generation.source_rom_start,
                rom_end: generation.source_rom_end,
                load_start: generation.load_start,
                text_start: generation.text_start,
                text_end: generation.text_end,
                data_start: generation.data_start,
                data_end: generation.data_end,
                bss_start: generation.bss_start,
                bss_end: generation.bss_end,
                loaded_sha256: generation.loaded_sha256.clone(),
            })
            .collect();
        (rom, pack, recipes)
    }

    fn transfer_fixture_with_conflicts(
        delay_word: u32,
        alter_overlay_a: bool,
        alter_overlay_b: bool,
        kind: ExactTransferKindV1,
    ) -> (
        NormalizedRom,
        DenseAotPackV1,
        GenerationTopologyV1,
        BackedGenerationCatalogEvidenceV1,
        [u8; 32],
        ExactTransferRequestV1,
    ) {
        const RESIDENT_END: u32 = BOOT + 0x1400;
        const OVERLAY_LEN: u32 = 0x800;
        const SOURCE: u32 = OVERLAY + 4;
        const TARGET: u32 = OVERLAY + OVERLAY_LEN - 8;
        let mut raw = vec![0u8; 0x5000];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&BOOT.to_be_bytes());
        for (index, byte) in raw[0x1000..0x2400].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(17).wrapping_add(3);
        }
        let resident_source_pc = 0x1000usize + usize::try_from(SOURCE - BOOT).unwrap();
        let transfer_opcode = match kind {
            ExactTransferKindV1::Call => 0x0c00_0000,
            ExactTransferKindV1::Jump => 0x0800_0000,
        };
        raw[resident_source_pc..resident_source_pc + 4]
            .copy_from_slice(&(transfer_opcode | ((TARGET >> 2) & 0x03ff_ffff)).to_be_bytes());
        raw[resident_source_pc + 4..resident_source_pc + 8]
            .copy_from_slice(&delay_word.to_be_bytes());
        let resident_overlap = raw[0x2000..0x2400].to_vec();
        raw[0x3000..0x3400].copy_from_slice(&resident_overlap);
        raw[0x3800..0x3c00].copy_from_slice(&resident_overlap);
        if alter_overlay_a {
            raw[0x3000] ^= 1;
        }
        if alter_overlay_b {
            raw[0x3800] ^= 1;
        }
        for rom_start in [0x3000usize, 0x3800] {
            let target_offset = usize::try_from(TARGET - OVERLAY).unwrap();
            raw[rom_start + target_offset..rom_start + target_offset + 4]
                .copy_from_slice(&0x03e0_0008u32.to_be_bytes());
            raw[rom_start + target_offset + 4..rom_start + target_offset + 8]
                .copy_from_slice(&0u32.to_be_bytes());
        }
        let rom = crate::normalize(&raw).unwrap();
        let inputs = [
            DenseAotGenerationInput {
                name: "boot",
                source_rom_start: 0x1000,
                source_rom_end: 0x2400,
                load_start: BOOT,
                text_start: BOOT,
                text_end: RESIDENT_END,
                data_start: RESIDENT_END,
                data_end: RESIDENT_END,
                bss_start: RESIDENT_END,
                bss_end: RESIDENT_END,
            },
            DenseAotGenerationInput {
                name: "overlay_a",
                source_rom_start: 0x3000,
                source_rom_end: 0x3800,
                load_start: OVERLAY,
                text_start: OVERLAY,
                text_end: OVERLAY + OVERLAY_LEN,
                data_start: OVERLAY + OVERLAY_LEN,
                data_end: OVERLAY + OVERLAY_LEN,
                bss_start: OVERLAY + OVERLAY_LEN,
                bss_end: OVERLAY + OVERLAY_LEN,
            },
            DenseAotGenerationInput {
                name: "overlay_b",
                source_rom_start: 0x3800,
                source_rom_end: 0x4000,
                load_start: OVERLAY,
                text_start: OVERLAY,
                text_end: OVERLAY + OVERLAY_LEN,
                data_start: OVERLAY + OVERLAY_LEN,
                data_end: OVERLAY + OVERLAY_LEN,
                bss_start: OVERLAY + OVERLAY_LEN,
                bss_end: OVERLAY + OVERLAY_LEN,
            },
        ];
        let pack = build_dense_aot_pack_v1(&rom, &inputs).unwrap();
        let recipes = pack
            .generations
            .iter()
            .skip(1)
            .enumerate()
            .map(|(index, generation)| OverlayLoadRecipeV1 {
                schema: OVERLAY_RECIPE_SCHEMA_V1.to_owned(),
                descriptor_rom_offset: 0x200 + index as u32 * 0x24,
                rom_start: generation.source_rom_start,
                rom_end: generation.source_rom_end,
                load_start: generation.load_start,
                text_start: generation.text_start,
                text_end: generation.text_end,
                data_start: generation.data_start,
                data_end: generation.data_end,
                bss_start: generation.bss_start,
                bss_end: generation.bss_end,
                loaded_sha256: generation.loaded_sha256.clone(),
            })
            .collect::<Vec<_>>();
        let topology =
            build_generation_topology_v1(&rom, &pack, "boot", RESIDENT_ID_DOMAIN, &recipes)
                .unwrap();
        let mut generations = topology
            .generations
            .iter()
            .enumerate()
            .map(|(index, generation)| PrecompiledGenerationEvidenceV1 {
                generation: fn64_recomp_rs::GenerationId::new(generation.generation_id),
                image_start: GuestPc::new(generation.image_start),
                image_end: GuestPc::new(generation.image_end),
                invalidation_start: GuestPc::new(generation.invalidation_start),
                invalidation_end: GuestPc::new(generation.invalidation_end),
                expected_sha256: parse_sha256(&generation.image_sha256).unwrap(),
                shards: vec![PrecompiledShard::new(
                    BankId::new(index as u64 + 1),
                    GuestPc::new(generation.image_start),
                    GuestPc::new(generation.image_end),
                )
                .unwrap()],
            })
            .collect::<Vec<_>>();
        generations.sort_by_key(|generation| generation.generation.get());
        let mut backings = topology
            .generations
            .iter()
            .map(|generation| PrecompiledGenerationBackingEvidenceV1 {
                generation: fn64_recomp_rs::GenerationId::new(generation.generation_id),
                spans: vec![BackedExecutableSpanV1::new(
                    GuestPc::new(generation.invalidation_start),
                    generation.invalidation_start & 0x007f_ffff,
                    generation.invalidation_end - generation.invalidation_start,
                )
                .unwrap()],
            })
            .collect::<Vec<_>>();
        backings.sort_by_key(|backing| backing.generation.get());
        let catalog = BackedGenerationCatalogEvidenceV1 {
            schema: BACKED_GENERATION_CATALOG_EVIDENCE_SCHEMA_V1.to_owned(),
            generations,
            backings,
            active_segments: Vec::new(),
        };
        let digest = catalog_definition_sha256_v1(&catalog);
        (
            rom,
            pack,
            topology,
            catalog,
            digest,
            ExactTransferRequestV1 {
                source_bank: "boot".to_owned(),
                source_pc: SOURCE,
                kind,
                target_pc: TARGET,
            },
        )
    }

    fn transfer_fixture(
        delay_word: u32,
    ) -> (
        NormalizedRom,
        DenseAotPackV1,
        GenerationTopologyV1,
        BackedGenerationCatalogEvidenceV1,
        [u8; 32],
        ExactTransferRequestV1,
    ) {
        transfer_fixture_with_conflicts(delay_word, false, true, ExactTransferKindV1::Call)
    }

    fn prove_transfer_bank(
        facts: &mut FactDb,
        bank: &str,
        rom_start: u32,
        va_start: u32,
        byte_len: u32,
        entry: Option<u32>,
    ) {
        let mapping = facts.insert(Fact::RomMapping {
            bank: bank.to_owned(),
            rom_space: RomAddressSpace::Physical,
            rom_start,
            rom_end: rom_start + byte_len,
            va_start,
            va_end: va_start + byte_len,
        });
        facts
            .conclude(
                format!("bank:{bank}"),
                ProofState::Proven,
                vec![mapping],
                "catalog_transfer_test_mapping",
            )
            .unwrap();
        if let Some(entry) = entry {
            let target = BankAddr::new(bank, entry);
            let claim = facts.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::ProloguePattern,
                evidence: FunctionEntryEvidence::Prologue {
                    stack_adjust: target.clone(),
                    frame_size: 16,
                    pattern: ProloguePattern::LeafWithMatchedRestore,
                    corroborating_site: BankAddr::new(bank, entry + 4),
                },
                proposed_state: ProofState::Proven,
            });
            facts
                .conclude(
                    function_entry_subject(&target),
                    ProofState::Proven,
                    vec![claim],
                    "catalog_transfer_test_entry",
                )
                .unwrap();
        }
    }

    #[test]
    fn derives_exact_topology_deterministically() {
        let (rom, pack, recipes) = fixture();
        let first = build_generation_topology_v1(&rom, &pack, "boot", RESIDENT_ID_DOMAIN, &recipes)
            .unwrap();
        assert_eq!(
            first,
            build_generation_topology_v1(&rom, &pack, "boot", RESIDENT_ID_DOMAIN, &recipes)
                .unwrap()
        );
        assert_eq!(
            (
                first.immutable_prefix.va_start,
                first.immutable_prefix.va_end
            ),
            (BOOT, OVERLAY)
        );
        assert_eq!(first.generations.len(), 3);
        let resident = first
            .generations
            .iter()
            .find(|generation| generation.role == CatalogGenerationRoleV1::ResidentTail)
            .unwrap();
        assert_eq!(
            (resident.image_start, resident.image_end),
            (OVERLAY, BOOT + 0x2000)
        );
        assert_eq!(resident.invalidation_end, BOOT + 0x2080);
        assert!(valid_sha256(&first.topology_sha256));
    }

    #[test]
    fn resident_identity_matches_selected_build_byte_rule() {
        let image_sha256 = [0x5au8; 32];
        let actual = resident_tail_generation_id_v1(
            RESIDENT_ID_DOMAIN,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            0x800e_1b90,
            0x8010_0400,
            0x800e_1b90,
            0x8017_1a60,
            image_sha256,
        );
        let mut expected = Sha256::new();
        expected.update(RESIDENT_ID_DOMAIN);
        let rom = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        expected.update((rom.len() as u64).to_be_bytes());
        expected.update(rom.as_bytes());
        for value in [0x800e_1b90u32, 0x8010_0400, 0x800e_1b90, 0x8017_1a60] {
            expected.update(value.to_be_bytes());
        }
        expected.update(image_sha256);
        assert_eq!(
            actual,
            u64::from_be_bytes(expected.finalize()[..8].try_into().unwrap())
        );
    }

    #[test]
    fn rejects_recipe_dense_disagreement() {
        let (rom, pack, mut recipes) = fixture();
        recipes[1].loaded_sha256.replace_range(..1, "f");
        assert_eq!(
            build_generation_topology_v1(&rom, &pack, "boot", RESIDENT_ID_DOMAIN, &recipes,),
            Err(GenerationTopologyError::OverlayRecipeMismatch { index: 1 })
        );
    }

    #[test]
    fn rejects_dense_identity_not_rebuilt_from_rom() {
        let (rom, mut pack, recipes) = fixture();
        pack.generations[1].bank_id ^= 1;
        assert_eq!(
            build_generation_topology_v1(&rom, &pack, "boot", RESIDENT_ID_DOMAIN, &recipes,),
            Err(GenerationTopologyError::DensePackMismatch)
        );
    }

    #[test]
    fn invalidation_union_must_cover_resident_tail() {
        let (rom, pack, mut recipes) = fixture();
        for recipe in &mut recipes {
            recipe.bss_end = BOOT + 0x1800;
        }
        assert_eq!(
            build_generation_topology_v1(&rom, &pack, "boot", RESIDENT_ID_DOMAIN, &recipes,),
            Err(GenerationTopologyError::InvalidResidentSplit)
        );
    }

    #[test]
    fn state_space_preserves_only_noninvalidated_generation_segments() {
        let (rom, pack, recipes) = fixture();
        let analysis = build_generation_geometry_analysis_v1(
            &rom,
            &pack,
            "boot",
            RESIDENT_ID_DOMAIN,
            &recipes,
            GenerationGeometryStateSpaceLimits::default(),
        )
        .unwrap();
        assert!(analysis.pcs_may_coexist_by_geometry_v1("boot", BOOT + 4, "overlay_a", OVERLAY,));
        assert!(!analysis.pcs_may_coexist_by_geometry_v1(
            "boot",
            OVERLAY + 4,
            "overlay_a",
            OVERLAY,
        ));
        assert!(analysis.pcs_may_coexist_by_geometry_v1(
            "overlay_a",
            OVERLAY + 4,
            "overlay_b",
            OVERLAY + 0x44,
        ));
    }

    #[test]
    fn state_enumeration_is_bounded() {
        let (rom, pack, recipes) = fixture();
        assert!(matches!(
            build_generation_geometry_analysis_v1(
                &rom,
                &pack,
                "boot",
                RESIDENT_ID_DOMAIN,
                &recipes,
                GenerationGeometryStateSpaceLimits { max_states: 1 },
            ),
            Err(GenerationTopologyError::StateLimitExceeded { limit: 1, .. })
        ));
    }

    #[test]
    fn exact_physical_conflict_selects_one_catalog_generation() {
        let (rom, pack, topology, catalog, digest, request) = transfer_fixture(0);
        let resolution = verify_catalog_bound_exact_transfer_v1(
            &rom, &pack, &topology, &catalog, digest, request,
        )
        .unwrap();
        let CatalogBoundExactTransferResolutionV1::Authorized(capability) = resolution else {
            panic!("one nonconflicting generation must be selected");
        };
        assert_eq!(capability.target_bank(), "overlay_a");
    }

    #[test]
    fn exact_physical_conflicts_yield_typed_activation_miss() {
        let (rom, pack, topology, catalog, digest, request) =
            transfer_fixture_with_conflicts(0, true, true, ExactTransferKindV1::Call);
        assert!(matches!(
            verify_catalog_bound_exact_transfer_v1(
                &rom, &pack, &topology, &catalog, digest, request,
            )
            .unwrap(),
            CatalogBoundExactTransferResolutionV1::ActivationMiss {
                excluded_generations,
                ..
            } if excluded_generations.len() == 2
        ));
    }

    #[test]
    fn absent_physical_conflicts_remain_typed_ambiguous() {
        let (rom, pack, topology, catalog, digest, request) =
            transfer_fixture_with_conflicts(0, false, false, ExactTransferKindV1::Call);
        assert!(matches!(
            verify_catalog_bound_exact_transfer_v1(
                &rom, &pack, &topology, &catalog, digest, request,
            )
            .unwrap(),
            CatalogBoundExactTransferResolutionV1::Ambiguous {
                compatible_generations,
                ..
            } if compatible_generations.len() == 2
        ));
    }

    #[test]
    fn exact_transfer_store_delay_cannot_mint_authority() {
        let (rom, pack, topology, catalog, digest, request) = transfer_fixture(0xa400_0000);
        let delay_pc = request.source_pc + 4;
        assert!(matches!(
            verify_catalog_bound_exact_transfer_v1(
                &rom, &pack, &topology, &catalog, digest, request,
            ),
            Err(CatalogBoundExactTransferErrorV1::ControlOrDelayMayWriteMemory {
                pc
            })
            if pc == delay_pc
        ));
    }

    #[test]
    fn catalog_definition_digest_is_bound_and_observations_are_not_authority() {
        let (rom, pack, topology, mut catalog, digest, request) = transfer_fixture(0);
        catalog
            .active_segments
            .push(fn64_recomp_rs::ActiveGenerationSegment {
                start: GuestPc::new(OVERLAY),
                end: GuestPc::new(OVERLAY + 4),
                generation: catalog.generations[0].generation,
            });
        assert!(matches!(
            verify_catalog_bound_exact_transfer_v1(
                &rom,
                &pack,
                &topology,
                &catalog,
                digest,
                request.clone(),
            ),
            Ok(CatalogBoundExactTransferResolutionV1::Authorized(_))
        ));
        assert!(matches!(
            verify_catalog_bound_exact_transfer_v1(
                &rom,
                &pack,
                &topology,
                &catalog,
                {
                    let mut wrong = digest;
                    wrong[0] ^= 1;
                    wrong
                },
                request,
            ),
            Err(CatalogBoundExactTransferErrorV1::CatalogDefinitionDigestMismatch)
        ));
    }

    #[test]
    fn snapshot_composer_consumes_only_selected_call_capability() {
        let (rom, pack, topology, catalog, digest, request) = transfer_fixture(0);
        let source_pc = request.source_pc;
        let target_pc = request.target_pc;
        let resolution = verify_catalog_bound_exact_transfer_v1(
            &rom, &pack, &topology, &catalog, digest, request,
        )
        .unwrap();
        let CatalogBoundExactTransferResolutionV1::Authorized(mut capability) = resolution else {
            panic!("fixture must select overlay_a");
        };
        let boot = &rom.bytes[0x1000..0x2400];
        let overlay_a = &rom.bytes[0x3000..0x3800];
        let overlay_b = &rom.bytes[0x3800..0x4000];
        let mut facts = FactDb::new();
        prove_transfer_bank(
            &mut facts,
            "boot",
            0x1000,
            BOOT,
            boot.len() as u32,
            Some(source_pc),
        );
        prove_transfer_bank(
            &mut facts,
            "overlay_a",
            0x3000,
            OVERLAY,
            overlay_a.len() as u32,
            None,
        );
        prove_transfer_bank(
            &mut facts,
            "overlay_b",
            0x3800,
            OVERLAY,
            overlay_b.len() as u32,
            None,
        );
        let inputs = [
            MaterializedBankInput {
                bank: "boot",
                va_start: BOOT,
                bytes: boot,
                seed_roots: &[source_pc],
            },
            MaterializedBankInput {
                bank: "overlay_a",
                va_start: OVERLAY,
                bytes: overlay_a,
                seed_roots: &[target_pc],
            },
            MaterializedBankInput {
                bank: "overlay_b",
                va_start: OVERLAY,
                bytes: overlay_b,
                seed_roots: &[target_pc],
            },
        ];
        let without =
            crate::snapshot::compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(without[1..].iter().all(|snapshot| !snapshot.banks[0]
            .authority_closure
            .cfg
            .proven_roots
            .contains(&target_pc)));

        let with = compose_materialized_banks_catalog_bound_v1(
            &rom,
            &facts,
            &inputs,
            &pack,
            &topology,
            digest,
            std::slice::from_ref(&capability),
        )
        .unwrap()
        .into_diagnostic_snapshots();
        assert!(with[1].banks[0]
            .authority_closure
            .cfg
            .proven_roots
            .contains(&target_pc));
        assert!(!with[2].banks[0]
            .authority_closure
            .cfg
            .proven_roots
            .contains(&target_pc));
        assert!(with[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "boot"
                    && source.pc == source_pc
                    && target.bank == "overlay_a"
                    && target.pc == target_pc
        )));

        let recipes = pack
            .generations
            .iter()
            .skip(1)
            .enumerate()
            .map(|(index, generation)| OverlayLoadRecipeV1 {
                schema: OVERLAY_RECIPE_SCHEMA_V1.to_owned(),
                descriptor_rom_offset: 0x200 + index as u32 * 0x24,
                rom_start: generation.source_rom_start,
                rom_end: generation.source_rom_end,
                load_start: generation.load_start,
                text_start: generation.text_start,
                text_end: generation.text_end,
                data_start: generation.data_start,
                data_end: generation.data_end,
                bss_start: generation.bss_start,
                bss_end: generation.bss_end,
                loaded_sha256: generation.loaded_sha256.clone(),
            })
            .collect::<Vec<_>>();
        let other_topology = build_generation_topology_v1(
            &rom,
            &pack,
            "boot",
            b"fn64:test-other-resident-domain:v1:",
            &recipes,
        )
        .unwrap();
        assert_eq!(
            compose_materialized_banks_catalog_bound_v1(
                &rom,
                &facts,
                &inputs,
                &pack,
                &other_topology,
                digest,
                std::slice::from_ref(&capability),
            )
            .unwrap_err(),
            crate::snapshot::SnapshotError::CatalogCapabilityIdentityMismatch { index: 0 }
        );
        let mut other_catalog_digest = digest;
        other_catalog_digest[0] ^= 1;
        assert_eq!(
            compose_materialized_banks_catalog_bound_v1(
                &rom,
                &facts,
                &inputs,
                &pack,
                &topology,
                other_catalog_digest,
                std::slice::from_ref(&capability),
            )
            .unwrap_err(),
            crate::snapshot::SnapshotError::CatalogCapabilityIdentityMismatch { index: 0 }
        );
        capability.target_generation ^= 1;
        assert_eq!(
            compose_materialized_banks_catalog_bound_v1(
                &rom,
                &facts,
                &inputs,
                &pack,
                &topology,
                digest,
                &[capability],
            )
            .unwrap_err(),
            crate::snapshot::SnapshotError::CatalogCapabilityIdentityMismatch { index: 0 }
        );
    }

    #[test]
    fn selected_direct_jump_grants_reachability_without_callable_authority() {
        let (rom, pack, topology, catalog, digest, request) =
            transfer_fixture_with_conflicts(0, false, true, ExactTransferKindV1::Jump);
        let source_pc = request.source_pc;
        let target_pc = request.target_pc;
        let CatalogBoundExactTransferResolutionV1::Authorized(capability) =
            verify_catalog_bound_exact_transfer_v1(
                &rom, &pack, &topology, &catalog, digest, request,
            )
            .unwrap()
        else {
            panic!("jump fixture must select overlay_a");
        };
        let boot = &rom.bytes[0x1000..0x2400];
        let overlay_a = &rom.bytes[0x3000..0x3800];
        let overlay_b = &rom.bytes[0x3800..0x4000];
        let mut facts = FactDb::new();
        prove_transfer_bank(
            &mut facts,
            "boot",
            0x1000,
            BOOT,
            boot.len() as u32,
            Some(source_pc),
        );
        prove_transfer_bank(
            &mut facts,
            "overlay_a",
            0x3000,
            OVERLAY,
            overlay_a.len() as u32,
            None,
        );
        prove_transfer_bank(
            &mut facts,
            "overlay_b",
            0x3800,
            OVERLAY,
            overlay_b.len() as u32,
            None,
        );
        let inputs = [
            MaterializedBankInput {
                bank: "boot",
                va_start: BOOT,
                bytes: boot,
                seed_roots: &[source_pc],
            },
            MaterializedBankInput {
                bank: "overlay_a",
                va_start: OVERLAY,
                bytes: overlay_a,
                seed_roots: &[target_pc],
            },
            MaterializedBankInput {
                bank: "overlay_b",
                va_start: OVERLAY,
                bytes: overlay_b,
                seed_roots: &[target_pc],
            },
        ];
        let snapshots = compose_materialized_banks_catalog_bound_v1(
            &rom,
            &facts,
            &inputs,
            &pack,
            &topology,
            digest,
            &[capability],
        )
        .unwrap()
        .into_diagnostic_snapshots();
        assert!(snapshots[1].banks[0]
            .authority_closure
            .cfg
            .proven_roots
            .contains(&target_pc));
        assert!(!snapshots[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "boot" && target.bank == "overlay_a"
        )));
        assert!(!snapshots[1].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                crate::owner_proof::OwnerAssessment::Proven { owner }
                    if owner.entry.pc == target_pc
            )));
    }

    /// Read-only selected-ROM characterization. No ROM-derived bytes or
    /// generated game output enter the repository; the caller supplies their
    /// private image through the normal discovery environment variable.
    #[test]
    #[ignore = "requires private FN64_DISCOVER_NWXE_ROM"]
    fn wm_known_catalog_transfer_outcomes() {
        use crate::banks::{BankNamePattern, BOOT_BANK};
        use crate::delta_vote::DeltaVoteConfig;
        use crate::overlay_regions::SearchConfig;
        use crate::{run_discovery_with_recovered_overlay_regions, RecoveredOverlayInput};

        let path = std::env::var("FN64_DISCOVER_NWXE_ROM")
            .expect("FN64_DISCOVER_NWXE_ROM names the caller-owned NWXE ROM");
        let raw = std::fs::read(path).unwrap();
        let search = SearchConfig::aki_family();
        let input = RecoveredOverlayInput {
            min_mapped_regions: search.min_records,
            search,
            delta_vote: DeltaVoteConfig::default(),
            table_name: "recovered_overlay_descriptors".to_owned(),
            bank_name: BankNamePattern::new("recovered_overlay_", 0, ""),
        };
        let (rom, _facts, recovery) =
            run_discovery_with_recovered_overlay_regions(&raw, &input).unwrap();
        let recipes =
            crate::overlay_recipe::admitted_overlay_load_recipes_v1(&rom.bytes, &recovery).unwrap();
        let names = (0..recipes.len())
            .map(|index| format!("recovered_overlay_{index}"))
            .collect::<Vec<_>>();
        let mut inputs = vec![DenseAotGenerationInput {
            name: BOOT_BANK,
            source_rom_start: 0x1000,
            source_rom_end: 0x101000,
            load_start: 0x8000_0400,
            text_start: 0x8000_0400,
            text_end: 0x8010_0400,
            data_start: 0x8010_0400,
            data_end: 0x8010_0400,
            bss_start: 0x8010_0400,
            bss_end: 0x8010_0400,
        }];
        inputs.extend(
            names
                .iter()
                .zip(&recipes)
                .map(|(name, recipe)| DenseAotGenerationInput::from((name.as_str(), recipe))),
        );
        let dense = build_dense_aot_pack_v1(&rom, &inputs).unwrap();
        let topology = build_generation_topology_v1(
            &rom,
            &dense,
            BOOT_BANK,
            b"fn64:wm2000-resident-tail-generation:v1:",
            &recipes,
        )
        .unwrap();
        let mut generations = topology
            .generations
            .iter()
            .map(|generation| {
                let bytes = generation_bytes(&rom, &dense, &topology, generation).unwrap();
                let generation_name = if generation.role == CatalogGenerationRoleV1::ResidentTail {
                    "resident_tail"
                } else {
                    generation.materialized_bank.as_str()
                };
                let shards = bytes
                    .chunks(crate::dense_aot_pack::DENSE_AOT_SHARD_BYTES as usize)
                    .enumerate()
                    .map(|(index, bytes)| {
                        let start = generation.image_start
                            + u32::try_from(
                                index * crate::dense_aot_pack::DENSE_AOT_SHARD_BYTES as usize,
                            )
                            .unwrap();
                        let words = bytes
                            .chunks_exact(4)
                            .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
                            .collect::<Vec<_>>();
                        PrecompiledShard::new(
                            BankId::new(crate::dense_aot_pack::dense_aot_artifact_bank_id(
                                &rom.sha256,
                                generation_name,
                                start,
                                &words,
                            )),
                            GuestPc::new(start),
                            GuestPc::new(start + bytes.len() as u32),
                        )
                        .unwrap()
                    })
                    .collect();
                PrecompiledGenerationEvidenceV1 {
                    generation: fn64_recomp_rs::GenerationId::new(generation.generation_id),
                    image_start: GuestPc::new(generation.image_start),
                    image_end: GuestPc::new(generation.image_end),
                    invalidation_start: GuestPc::new(generation.invalidation_start),
                    invalidation_end: GuestPc::new(generation.invalidation_end),
                    expected_sha256: parse_sha256(&generation.image_sha256).unwrap(),
                    shards,
                }
            })
            .collect::<Vec<_>>();
        generations.sort_by_key(|generation| generation.generation.get());
        let mut backings = topology
            .generations
            .iter()
            .map(|generation| PrecompiledGenerationBackingEvidenceV1 {
                generation: fn64_recomp_rs::GenerationId::new(generation.generation_id),
                spans: vec![BackedExecutableSpanV1::new(
                    GuestPc::new(generation.invalidation_start),
                    generation.invalidation_start & 0x1fff_ffff,
                    generation.invalidation_end - generation.invalidation_start,
                )
                .unwrap()],
            })
            .collect::<Vec<_>>();
        backings.sort_by_key(|backing| backing.generation.get());
        let catalog = BackedGenerationCatalogEvidenceV1 {
            schema: BACKED_GENERATION_CATALOG_EVIDENCE_SCHEMA_V1.to_owned(),
            generations,
            backings,
            active_segments: Vec::new(),
        };
        let digest = catalog_definition_sha256_v1(&catalog);
        let verify = |source_pc, kind, target_pc| {
            verify_catalog_bound_exact_transfer_v1(
                &rom,
                &dense,
                &topology,
                &catalog,
                digest,
                ExactTransferRequestV1 {
                    source_bank: BOOT_BANK.to_owned(),
                    source_pc,
                    kind,
                    target_pc,
                },
            )
        };
        let first = verify(0x800e_1bcc, ExactTransferKindV1::Call, 0x8013_b744).unwrap();
        assert!(matches!(
            first,
            CatalogBoundExactTransferResolutionV1::Authorized(ref capability)
                if capability.target_bank() == "recovered_overlay_2"
        ));
        assert!(matches!(
            verify(0x800f_1de4, ExactTransferKindV1::Jump, 0x8010_211c,).unwrap(),
            CatalogBoundExactTransferResolutionV1::ActivationMiss { .. }
        ));
        assert!(matches!(
            verify(0x800e_1bb4, ExactTransferKindV1::Call, 0x8013_c3c0,),
            Err(CatalogBoundExactTransferErrorV1::ControlOrDelayMayWriteMemory { pc: 0x800e_1bb8 })
        ));
    }
}
