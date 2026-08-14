//! Bounded fixed-point composition for catalog-selected direct transfers.
//!
//! The sweep starts from ordinary byte-verified composition, considers only
//! direct calls and jumps reached by the authority-only CFG, and asks the
//! generation-topology validator to select multiply-owned targets. Every
//! admitted edge is a move-only capability derived from a real backed runtime
//! catalog. Non-authorized outcomes remain typed diagnostics and never become
//! roots.

use crate::cfg::{BlockTerminator, WordClass};
use crate::dense_aot_pack::DenseAotPackV1;
use crate::facts::FactDb;
use crate::generation_topology::{
    validate_catalog_bound_exact_transfer_context_v1, CatalogBoundExactTransferErrorV1,
    CatalogBoundExactTransferResolutionV1, CatalogBoundExactTransferV1, ExactTransferKindV1,
    ExactTransferRequestV1, GenerationTopologyV1,
};
use crate::owner_proof::exact_authority_direct_call;
use crate::snapshot::{
    compose_materialized_banks_catalog_bound_v1, MaterializedBankInput, ProgramSnapshotV1,
    SnapshotError, ValidatedComposedSnapshotsV2,
};
use crate::NormalizedRom;
use fn64_recomp_rs::BackedPrecompiledGenerationCatalogV1;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogTransferFixedPointLimitsV1 {
    pub max_rounds: usize,
    pub max_capabilities: usize,
}

impl Default for CatalogTransferFixedPointLimitsV1 {
    fn default() -> Self {
        Self {
            max_rounds: 64,
            max_capabilities: 65_536,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogTransferFixedPointTerminationV1 {
    NoNewAuthorizedCapabilities,
    RepeatedAuthorityState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogTransferDispositionV1 {
    Authorized {
        target_bank: String,
        target_generation: u64,
    },
    ActivationMiss {
        excluded_generations: Vec<u64>,
    },
    Ambiguous {
        compatible_generations: Vec<u64>,
    },
    Rejected {
        error: CatalogBoundExactTransferErrorV1,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogTransferFindingV1 {
    pub request: ExactTransferRequestV1,
    pub disposition: CatalogTransferDispositionV1,
}

#[derive(Debug)]
pub struct CatalogTransferFixedPointResultV1 {
    validated: ValidatedComposedSnapshotsV2,
    findings: Vec<CatalogTransferFindingV1>,
    rounds: usize,
    authorized_capabilities: usize,
    termination: CatalogTransferFixedPointTerminationV1,
}

impl CatalogTransferFixedPointResultV1 {
    pub fn validated(&self) -> &ValidatedComposedSnapshotsV2 {
        &self.validated
    }

    pub fn into_validated(self) -> ValidatedComposedSnapshotsV2 {
        self.validated
    }

    pub fn findings(&self) -> &[CatalogTransferFindingV1] {
        &self.findings
    }

    pub const fn rounds(&self) -> usize {
        self.rounds
    }

    pub const fn authorized_capabilities(&self) -> usize {
        self.authorized_capabilities
    }

    pub const fn termination(&self) -> CatalogTransferFixedPointTerminationV1 {
        self.termination
    }
}

#[derive(Debug)]
pub enum CatalogTransferFixedPointErrorV1 {
    InvalidLimits,
    Context(CatalogBoundExactTransferErrorV1),
    Composition(SnapshotError),
    RoundLimitExceeded { limit: usize },
    CapabilityLimitExceeded { capabilities: usize, limit: usize },
}

impl fmt::Display for CatalogTransferFixedPointErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "catalog-bound direct-transfer fixed point failed: {self:?}"
        )
    }
}

impl std::error::Error for CatalogTransferFixedPointErrorV1 {}

/// Compose all banks while monotonically admitting exact, catalog-selected
/// direct transfers.
///
/// `catalog` is the runtime's validated constructor-owned catalog, not a
/// serialized evidence projection. The context constructor snapshots and
/// validates its immutable definition before the first composition. Every
/// round considers only newly authority-reachable direct call/jump edges whose
/// target VA is owned by more than one topology generation.
pub fn compose_catalog_bound_direct_transfer_fixed_point_v1(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
    dense_pack: &DenseAotPackV1,
    topology: &GenerationTopologyV1,
    catalog: &BackedPrecompiledGenerationCatalogV1,
    limits: CatalogTransferFixedPointLimitsV1,
) -> Result<CatalogTransferFixedPointResultV1, CatalogTransferFixedPointErrorV1> {
    if limits.max_rounds == 0 || limits.max_capabilities == 0 {
        return Err(CatalogTransferFixedPointErrorV1::InvalidLimits);
    }
    let context =
        validate_catalog_bound_exact_transfer_context_v1(rom, dense_pack, topology, catalog)
            .map_err(CatalogTransferFixedPointErrorV1::Context)?;
    let mut capabilities = Vec::<CatalogBoundExactTransferV1>::new();
    let mut findings = BTreeMap::<ExactTransferRequestV1, CatalogTransferDispositionV1>::new();
    let mut authority_states = BTreeSet::new();

    for round in 0..limits.max_rounds {
        let validated = compose_materialized_banks_catalog_bound_v1(
            rom,
            base_facts,
            inputs,
            dense_pack,
            topology,
            context.catalog_definition_sha256(),
            &capabilities,
        )
        .map_err(CatalogTransferFixedPointErrorV1::Composition)?;
        let state = authority_state(validated.snapshots());
        if !authority_states.insert(state) {
            return Ok(finish(
                validated,
                findings,
                round + 1,
                capabilities.len(),
                CatalogTransferFixedPointTerminationV1::RepeatedAuthorityState,
            ));
        }

        let requests = multiply_owned_authority_direct_transfers(validated.snapshots(), topology);
        let mut admitted_this_round = 0usize;
        for request in requests {
            if findings.contains_key(&request) {
                continue;
            }
            match context.verify(request.clone()) {
                Ok(CatalogBoundExactTransferResolutionV1::Authorized(capability)) => {
                    let target_bank = capability.target_bank().to_owned();
                    let target_generation = capability.target_generation();
                    findings.insert(
                        request,
                        CatalogTransferDispositionV1::Authorized {
                            target_bank,
                            target_generation,
                        },
                    );
                    capabilities.push(capability);
                    admitted_this_round += 1;
                }
                Ok(CatalogBoundExactTransferResolutionV1::ActivationMiss {
                    excluded_generations,
                    ..
                }) => {
                    findings.insert(
                        request,
                        CatalogTransferDispositionV1::ActivationMiss {
                            excluded_generations,
                        },
                    );
                }
                Ok(CatalogBoundExactTransferResolutionV1::Ambiguous {
                    compatible_generations,
                    ..
                }) => {
                    findings.insert(
                        request,
                        CatalogTransferDispositionV1::Ambiguous {
                            compatible_generations,
                        },
                    );
                }
                Err(error) => {
                    findings.insert(request, CatalogTransferDispositionV1::Rejected { error });
                }
            }
        }
        if capabilities.len() > limits.max_capabilities {
            return Err(CatalogTransferFixedPointErrorV1::CapabilityLimitExceeded {
                capabilities: capabilities.len(),
                limit: limits.max_capabilities,
            });
        }
        if admitted_this_round == 0 {
            return Ok(finish(
                validated,
                findings,
                round + 1,
                capabilities.len(),
                CatalogTransferFixedPointTerminationV1::NoNewAuthorizedCapabilities,
            ));
        }
    }

    Err(CatalogTransferFixedPointErrorV1::RoundLimitExceeded {
        limit: limits.max_rounds,
    })
}

fn finish(
    validated: ValidatedComposedSnapshotsV2,
    findings: BTreeMap<ExactTransferRequestV1, CatalogTransferDispositionV1>,
    rounds: usize,
    authorized_capabilities: usize,
    termination: CatalogTransferFixedPointTerminationV1,
) -> CatalogTransferFixedPointResultV1 {
    CatalogTransferFixedPointResultV1 {
        validated,
        findings: findings
            .into_iter()
            .map(|(request, disposition)| CatalogTransferFindingV1 {
                request,
                disposition,
            })
            .collect(),
        rounds,
        authorized_capabilities,
        termination,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AuthorityBankState {
    bank: String,
    roots: Vec<u32>,
    calls: Vec<(u32, u32)>,
    jumps: Vec<(u32, u32)>,
}

fn authority_state(snapshots: &[ProgramSnapshotV1]) -> Vec<AuthorityBankState> {
    let mut state = snapshots
        .iter()
        .flat_map(|snapshot| &snapshot.banks)
        .map(|bank| {
            let mut roots = bank
                .authority_closure
                .cfg
                .proven_roots
                .iter()
                .copied()
                .collect::<Vec<_>>();
            roots.sort_unstable();
            AuthorityBankState {
                bank: bank.input.bank.clone(),
                roots,
                calls: authority_direct_calls(&bank.authority_closure.cfg),
                jumps: authority_direct_jumps(&bank.authority_closure.cfg),
            }
        })
        .collect::<Vec<_>>();
    state.sort_unstable();
    state
}

fn multiply_owned_authority_direct_transfers(
    snapshots: &[ProgramSnapshotV1],
    topology: &GenerationTopologyV1,
) -> Vec<ExactTransferRequestV1> {
    let mut requests = BTreeSet::new();
    for bank in snapshots.iter().flat_map(|snapshot| &snapshot.banks) {
        for (source_pc, target_pc, kind) in authority_direct_calls(&bank.authority_closure.cfg)
            .into_iter()
            .map(|(source, target)| (source, target, ExactTransferKindV1::Call))
            .chain(
                authority_direct_jumps(&bank.authority_closure.cfg)
                    .into_iter()
                    .map(|(source, target)| (source, target, ExactTransferKindV1::Jump)),
            )
        {
            let prepared_owner_count = snapshots
                .iter()
                .flat_map(|snapshot| &snapshot.banks)
                .filter(|candidate| {
                    candidate.input.bank != bank.input.bank
                        && candidate.input.va_start <= target_pc
                        && target_pc < candidate.input.va_end
                })
                .count();
            let topology_owner_count = topology
                .generations
                .iter()
                .filter(|generation| {
                    generation.image_start <= target_pc && target_pc < generation.image_end
                })
                .count();
            if prepared_owner_count > 1 && topology_owner_count > 1 {
                requests.insert(ExactTransferRequestV1 {
                    source_bank: bank.input.bank.clone(),
                    source_pc,
                    kind,
                    target_pc,
                });
            }
        }
    }
    requests.into_iter().collect()
}

fn authority_direct_calls(cfg: &crate::cfg::Cfg) -> Vec<(u32, u32)> {
    let mut calls = cfg
        .blocks
        .iter()
        .filter_map(|block| exact_authority_direct_call(cfg, block))
        .collect::<Vec<_>>();
    calls.sort_unstable();
    calls.dedup();
    calls
}

fn authority_direct_jumps(cfg: &crate::cfg::Cfg) -> Vec<(u32, u32)> {
    let mut jumps = cfg
        .blocks
        .iter()
        .filter_map(|block| {
            let BlockTerminator::Tail { target } = &block.terminator else {
                return None;
            };
            let source_pc = block.end_va.checked_sub(8)?;
            (cfg.word_class.get(&source_pc) == Some(&WordClass::ProvenCode)
                && cfg.word_class.get(&(source_pc + 4)) == Some(&WordClass::ProvenCode)
                && cfg.tail_transfers.contains(&(source_pc, *target)))
            .then_some((source_pc, *target))
        })
        .collect::<Vec<_>>();
    jumps.sort_unstable();
    jumps.dedup();
    jumps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::banks::BOOT_BANK;
    use crate::dense_aot_pack::{build_dense_aot_pack_v1, DenseAotGenerationInput};
    use crate::facts::{
        function_entry_subject, BankAddr, CandidateDetector, Fact, FunctionEntryEvidence,
        ProloguePattern, ProofState, RomAddressSpace,
    };
    use crate::generation_topology::{
        build_generation_topology_v1, CatalogBoundExactTransferErrorV1,
    };
    use crate::overlay_recipe::{OverlayLoadRecipeV1, OVERLAY_RECIPE_SCHEMA_V1};
    use fn64_recomp_rs::{
        BackedExecutableSpanV1, BackedPrecompiledGenerationCatalogV1, BankId, GenerationId,
        GuestPc, PrecompiledGeneration, PrecompiledGenerationBackingV1,
        PrecompiledGenerationCatalog, PrecompiledShard,
    };
    use sha2::{Digest, Sha256};

    const BOOT: u32 = 0x8000_0400;
    const OVERLAY: u32 = 0x8000_1400;
    const FIRST_TARGET: u32 = OVERLAY + 0x700;
    const SECOND_OVERLAY: u32 = OVERLAY + 0x780;
    const SECOND_TARGET: u32 = OVERLAY + 0x900;
    const SOURCE: u32 = OVERLAY + 4;
    const RESIDENT_DOMAIN: &[u8] = b"fn64:test-fixed-point-resident:v1:";

    #[derive(Clone, Copy)]
    struct FixtureOptions {
        first_a_conflicts: bool,
        first_b_conflicts: bool,
        second_c_conflicts: bool,
        second_d_conflicts: bool,
        store_delay: bool,
        first_jump: bool,
        nested_call: bool,
        first_target_preproven: bool,
        reverse_inputs: bool,
    }

    impl Default for FixtureOptions {
        fn default() -> Self {
            Self {
                first_a_conflicts: false,
                first_b_conflicts: true,
                second_c_conflicts: false,
                second_d_conflicts: true,
                store_delay: false,
                first_jump: false,
                nested_call: true,
                first_target_preproven: false,
                reverse_inputs: false,
            }
        }
    }

    struct Fixture {
        rom: NormalizedRom,
        facts: FactDb,
        pack: DenseAotPackV1,
        topology: GenerationTopologyV1,
        catalog: BackedPrecompiledGenerationCatalogV1,
        names: Vec<String>,
        ranges: Vec<(u32, u32, u32)>,
        roots: Vec<Vec<u32>>,
    }

    fn put_word(raw: &mut [u8], offset: usize, word: u32) {
        raw[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
    }

    fn jal(target: u32) -> u32 {
        0x0c00_0000 | ((target >> 2) & 0x03ff_ffff)
    }

    fn parse_sha256(value: &str) -> [u8; 32] {
        let mut result = [0; 32];
        for (index, byte) in result.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap();
        }
        result
    }

    fn prove_bank(
        facts: &mut FactDb,
        bank: &str,
        rom_start: u32,
        va_start: u32,
        byte_len: u32,
        roots: &[u32],
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
                "catalog_fixed_point_test_mapping",
            )
            .unwrap();
        for &root in roots {
            let target = BankAddr::new(bank, root);
            let claim = facts.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::ProloguePattern,
                evidence: FunctionEntryEvidence::Prologue {
                    stack_adjust: target.clone(),
                    frame_size: 16,
                    pattern: ProloguePattern::LeafWithMatchedRestore,
                    corroborating_site: BankAddr::new(bank, root + 4),
                },
                proposed_state: ProofState::Proven,
            });
            facts
                .conclude(
                    function_entry_subject(&target),
                    ProofState::Proven,
                    vec![mapping, claim],
                    "catalog_fixed_point_test_entry",
                )
                .unwrap();
        }
    }

    fn fixture(options: FixtureOptions) -> Fixture {
        const BOOT_ROM: u32 = 0x1000;
        const BOOT_LEN: u32 = 0x1400;
        const OVERLAY_LEN: u32 = 0x800;
        const A_ROM: u32 = 0x3000;
        const B_ROM: u32 = 0x3800;
        const C_ROM: u32 = 0x4000;
        const D_ROM: u32 = 0x4800;
        let mut raw = vec![0u8; 0x6000];
        put_word(&mut raw, 0, 0x8037_1240);
        put_word(&mut raw, 8, BOOT);
        for (index, byte) in raw[BOOT_ROM as usize..(BOOT_ROM + BOOT_LEN) as usize]
            .iter_mut()
            .enumerate()
        {
            *byte = (index as u8).wrapping_mul(17).wrapping_add(3);
        }
        let source_offset = (BOOT_ROM + (SOURCE - BOOT)) as usize;
        put_word(
            &mut raw,
            source_offset,
            if options.first_jump {
                0x0800_0000 | ((FIRST_TARGET >> 2) & 0x03ff_ffff)
            } else {
                jal(FIRST_TARGET)
            },
        );
        put_word(
            &mut raw,
            source_offset + 4,
            if options.store_delay { 0xac00_0000 } else { 0 },
        );

        let resident_tail =
            raw[(BOOT_ROM + (OVERLAY - BOOT)) as usize..(BOOT_ROM + BOOT_LEN) as usize].to_vec();
        raw[A_ROM as usize..(A_ROM + 0x400) as usize].copy_from_slice(&resident_tail);
        raw[B_ROM as usize..(B_ROM + 0x400) as usize].copy_from_slice(&resident_tail);
        for (index, byte) in raw[(A_ROM + 0x400) as usize..(A_ROM + OVERLAY_LEN) as usize]
            .iter_mut()
            .enumerate()
        {
            *byte = (index as u8).wrapping_mul(29).wrapping_add(11);
        }
        let a_tail = raw[(A_ROM + 0x400) as usize..(A_ROM + OVERLAY_LEN) as usize].to_vec();
        raw[(B_ROM + 0x400) as usize..(B_ROM + OVERLAY_LEN) as usize].copy_from_slice(&a_tail);
        if options.first_a_conflicts {
            raw[A_ROM as usize] ^= 1;
        }
        if options.first_b_conflicts {
            raw[B_ROM as usize] ^= 1;
        }
        let first_offset = (FIRST_TARGET - OVERLAY) as usize;
        let first_word = if options.nested_call {
            jal(SECOND_TARGET)
        } else {
            0x03e0_0008
        };
        put_word(&mut raw, A_ROM as usize + first_offset, first_word);
        put_word(&mut raw, A_ROM as usize + first_offset + 4, 0);
        put_word(&mut raw, B_ROM as usize + first_offset, first_word);
        put_word(&mut raw, B_ROM as usize + first_offset + 4, 0);
        // Runtime catalogs reject byte-identical generations. Keep each pair's
        // digest distinct outside the physical overlap used by selection.
        raw[(B_ROM + 0x500) as usize] ^= 0x40;

        let overlap_start = (SECOND_OVERLAY - OVERLAY) as usize;
        let overlap = raw[A_ROM as usize + overlap_start..(A_ROM + OVERLAY_LEN) as usize].to_vec();
        raw[C_ROM as usize..C_ROM as usize + overlap.len()].copy_from_slice(&overlap);
        raw[D_ROM as usize..D_ROM as usize + overlap.len()].copy_from_slice(&overlap);
        for rom_start in [C_ROM, D_ROM] {
            for (index, byte) in raw
                [(rom_start as usize + overlap.len())..(rom_start + OVERLAY_LEN) as usize]
                .iter_mut()
                .enumerate()
            {
                *byte = (index as u8).wrapping_mul(31).wrapping_add(7);
            }
        }
        if options.second_c_conflicts {
            raw[C_ROM as usize] ^= 1;
        }
        if options.second_d_conflicts {
            raw[D_ROM as usize] ^= 1;
        }
        raw[(D_ROM + 0x100) as usize] ^= 0x40;
        let second_offset = (SECOND_TARGET - SECOND_OVERLAY) as usize;
        for rom_start in [C_ROM, D_ROM] {
            put_word(&mut raw, rom_start as usize + second_offset, 0x03e0_0008);
            put_word(&mut raw, rom_start as usize + second_offset + 4, 0);
        }

        let rom = crate::normalize(&raw).unwrap();
        let specs = [
            (BOOT_BANK, BOOT_ROM, BOOT_LEN, BOOT),
            ("overlay_a", A_ROM, OVERLAY_LEN, OVERLAY),
            ("overlay_b", B_ROM, OVERLAY_LEN, OVERLAY),
            ("overlay_c", C_ROM, OVERLAY_LEN, SECOND_OVERLAY),
            ("overlay_d", D_ROM, OVERLAY_LEN, SECOND_OVERLAY),
        ];
        let dense_inputs = specs.map(|(name, rom_start, len, va_start)| DenseAotGenerationInput {
            name,
            source_rom_start: rom_start,
            source_rom_end: rom_start + len,
            load_start: va_start,
            text_start: va_start,
            text_end: va_start + len,
            data_start: va_start + len,
            data_end: va_start + len,
            bss_start: va_start + len,
            bss_end: va_start + len,
        });
        let pack = build_dense_aot_pack_v1(&rom, &dense_inputs).unwrap();
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
                text_sha256: generation.loaded_sha256.clone(),
            })
            .collect::<Vec<_>>();
        let topology =
            build_generation_topology_v1(&rom, &pack, BOOT_BANK, RESIDENT_DOMAIN, &recipes)
                .unwrap();
        let mut runtime_catalog = PrecompiledGenerationCatalog::new();
        let mut backings = Vec::new();
        for (index, generation) in topology.generations.iter().enumerate() {
            let id = GenerationId::new(generation.generation_id);
            runtime_catalog
                .register(
                    PrecompiledGeneration::new(
                        id,
                        GuestPc::new(generation.image_start),
                        GuestPc::new(generation.image_end),
                        GuestPc::new(generation.invalidation_start),
                        GuestPc::new(generation.invalidation_end),
                        parse_sha256(&generation.image_sha256),
                        vec![PrecompiledShard::new(
                            BankId::new(index as u64 + 1),
                            GuestPc::new(generation.image_start),
                            GuestPc::new(generation.image_end),
                        )
                        .unwrap()],
                    )
                    .unwrap(),
                )
                .unwrap();
            backings.push(
                PrecompiledGenerationBackingV1::new(
                    id,
                    vec![BackedExecutableSpanV1::new(
                        GuestPc::new(generation.invalidation_start),
                        generation.invalidation_start & 0x007f_ffff,
                        generation.invalidation_end - generation.invalidation_start,
                    )
                    .unwrap()],
                )
                .unwrap(),
            );
        }
        let catalog = BackedPrecompiledGenerationCatalogV1::new(runtime_catalog, backings).unwrap();

        let mut facts = FactDb::new();
        let mut roots = vec![vec![SOURCE], Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        if options.first_target_preproven {
            roots[1].push(FIRST_TARGET);
        }
        for ((name, rom_start, len, va_start), bank_roots) in specs.iter().zip(&roots) {
            prove_bank(&mut facts, name, *rom_start, *va_start, *len, bank_roots);
        }
        let mut names = specs
            .iter()
            .map(|spec| spec.0.to_owned())
            .collect::<Vec<_>>();
        let mut ranges = specs
            .iter()
            .map(|spec| (spec.1, spec.2, spec.3))
            .collect::<Vec<_>>();
        if options.reverse_inputs {
            names.reverse();
            ranges.reverse();
            roots.reverse();
        }
        Fixture {
            rom,
            facts,
            pack,
            topology,
            catalog,
            names,
            ranges,
            roots,
        }
    }

    fn run(
        options: FixtureOptions,
        limits: CatalogTransferFixedPointLimitsV1,
    ) -> Result<CatalogTransferFixedPointResultV1, CatalogTransferFixedPointErrorV1> {
        let fixture = fixture(options);
        let bytes = fixture
            .ranges
            .iter()
            .map(|(rom_start, len, _)| {
                &fixture.rom.bytes[*rom_start as usize..(*rom_start + *len) as usize]
            })
            .collect::<Vec<_>>();
        let inputs = fixture
            .names
            .iter()
            .zip(&fixture.ranges)
            .zip(&fixture.roots)
            .zip(&bytes)
            .map(
                |(((name, (_, _, va_start)), roots), bytes)| MaterializedBankInput {
                    bank: name,
                    va_start: *va_start,
                    bytes,
                    seed_roots: roots,
                },
            )
            .collect::<Vec<_>>();
        compose_catalog_bound_direct_transfer_fixed_point_v1(
            &fixture.rom,
            &fixture.facts,
            &inputs,
            &fixture.pack,
            &fixture.topology,
            &fixture.catalog,
            limits,
        )
    }

    #[test]
    fn two_round_catalog_selection_unlocks_a_second_authority_edge() {
        let result = run(FixtureOptions::default(), Default::default()).unwrap();
        assert_eq!(result.authorized_capabilities(), 2);
        assert_eq!(result.rounds(), 3);
        assert_eq!(
            result.termination(),
            CatalogTransferFixedPointTerminationV1::NoNewAuthorizedCapabilities
        );
        assert_eq!(result.findings().len(), 2);
        assert!(result.findings().iter().all(|finding| matches!(
            finding.disposition,
            CatalogTransferDispositionV1::Authorized { .. }
        )));
        let c = result
            .validated()
            .snapshots()
            .iter()
            .flat_map(|snapshot| &snapshot.banks)
            .find(|bank| bank.input.bank == "overlay_c")
            .unwrap();
        assert!(c
            .authority_closure
            .cfg
            .proven_roots
            .contains(&SECOND_TARGET));
    }

    #[test]
    fn findings_are_canonical_across_input_order_and_duplicate_rounds() {
        let forward = run(FixtureOptions::default(), Default::default()).unwrap();
        let reverse = run(
            FixtureOptions {
                reverse_inputs: true,
                ..FixtureOptions::default()
            },
            Default::default(),
        )
        .unwrap();
        assert_eq!(forward.findings(), reverse.findings());
        assert_eq!(forward.findings().len(), 2);
    }

    #[test]
    fn authority_reachable_direct_jump_enters_the_same_catalog_sweep() {
        let result = run(
            FixtureOptions {
                first_jump: true,
                nested_call: false,
                ..FixtureOptions::default()
            },
            Default::default(),
        )
        .unwrap();
        assert_eq!(result.authorized_capabilities(), 1);
        assert_eq!(result.findings().len(), 1);
        assert_eq!(result.findings()[0].request.kind, ExactTransferKindV1::Jump);
        assert!(matches!(
            result.findings()[0].disposition,
            CatalogTransferDispositionV1::Authorized { ref target_bank, .. }
                if target_bank == "overlay_a"
        ));
        let target = result
            .validated()
            .snapshots()
            .iter()
            .flat_map(|snapshot| &snapshot.banks)
            .find(|bank| bank.input.bank == "overlay_a")
            .unwrap();
        assert!(target
            .authority_closure
            .cfg
            .proven_roots
            .contains(&FIRST_TARGET));
        assert!(!result
            .validated()
            .snapshots()
            .iter()
            .flat_map(|snapshot| snapshot.facts.facts())
            .any(|fact| matches!(
                fact,
                Fact::DirectCall { source, target }
                    if source.bank == BOOT_BANK && target.bank == "overlay_a"
            )));
        assert!(!target
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                crate::owner_proof::OwnerAssessment::Proven { owner }
                    if owner.entry.pc == FIRST_TARGET
            )));
    }

    #[test]
    fn nonauthorized_dispositions_never_add_capabilities() {
        let ambiguous = run(
            FixtureOptions {
                first_b_conflicts: false,
                ..FixtureOptions::default()
            },
            Default::default(),
        )
        .unwrap();
        assert_eq!(ambiguous.authorized_capabilities(), 0);
        assert!(matches!(
            ambiguous.findings()[0].disposition,
            CatalogTransferDispositionV1::Ambiguous { .. }
        ));

        let miss = run(
            FixtureOptions {
                first_a_conflicts: true,
                ..FixtureOptions::default()
            },
            Default::default(),
        )
        .unwrap();
        assert_eq!(miss.authorized_capabilities(), 0);
        assert!(matches!(
            miss.findings()[0].disposition,
            CatalogTransferDispositionV1::ActivationMiss { .. }
        ));

        let rejected = run(
            FixtureOptions {
                store_delay: true,
                ..FixtureOptions::default()
            },
            Default::default(),
        )
        .unwrap();
        assert_eq!(rejected.authorized_capabilities(), 0);
        assert!(matches!(
            rejected.findings()[0].disposition,
            CatalogTransferDispositionV1::Rejected {
                error: CatalogBoundExactTransferErrorV1::ControlOrDelayMayWriteMemory { .. }
            }
        ));
    }

    #[test]
    fn constructor_bound_context_rejects_topology_tampering_before_composition() {
        let mut fixture = fixture(FixtureOptions::default());
        fixture.topology.generations[0].image_sha256 = format!("{:064x}", 1);
        let bytes = fixture
            .ranges
            .iter()
            .map(|(rom_start, len, _)| {
                &fixture.rom.bytes[*rom_start as usize..(*rom_start + *len) as usize]
            })
            .collect::<Vec<_>>();
        let inputs = fixture
            .names
            .iter()
            .zip(&fixture.ranges)
            .zip(&fixture.roots)
            .zip(&bytes)
            .map(
                |(((name, (_, _, va_start)), roots), bytes)| MaterializedBankInput {
                    bank: name,
                    va_start: *va_start,
                    bytes,
                    seed_roots: roots,
                },
            )
            .collect::<Vec<_>>();
        assert!(matches!(
            compose_catalog_bound_direct_transfer_fixed_point_v1(
                &fixture.rom,
                &fixture.facts,
                &inputs,
                &fixture.pack,
                &fixture.topology,
                &fixture.catalog,
                Default::default(),
            ),
            Err(CatalogTransferFixedPointErrorV1::Context(
                CatalogBoundExactTransferErrorV1::TopologyIdentityMismatch
            ))
        ));
    }

    #[test]
    fn constructor_bound_context_rejects_a_different_runtime_catalog() {
        let baseline = fixture(FixtureOptions::default());
        let other = fixture(FixtureOptions {
            first_a_conflicts: true,
            ..FixtureOptions::default()
        });
        assert!(matches!(
            validate_catalog_bound_exact_transfer_context_v1(
                &baseline.rom,
                &baseline.pack,
                &baseline.topology,
                &other.catalog,
            ),
            Err(CatalogBoundExactTransferErrorV1::CatalogTopologyMismatch)
        ));
    }

    #[test]
    fn fixed_point_has_explicit_round_capability_and_repeated_state_bounds() {
        assert!(matches!(
            run(
                FixtureOptions::default(),
                CatalogTransferFixedPointLimitsV1 {
                    max_rounds: 1,
                    max_capabilities: 8,
                }
            ),
            Err(CatalogTransferFixedPointErrorV1::RoundLimitExceeded { limit: 1 })
        ));
        assert!(matches!(
            run(
                FixtureOptions::default(),
                CatalogTransferFixedPointLimitsV1 {
                    max_rounds: 8,
                    max_capabilities: 1,
                }
            ),
            Err(CatalogTransferFixedPointErrorV1::CapabilityLimitExceeded {
                capabilities: 2,
                limit: 1
            })
        ));
        let repeated = run(
            FixtureOptions {
                first_target_preproven: true,
                nested_call: false,
                ..FixtureOptions::default()
            },
            Default::default(),
        )
        .unwrap();
        assert_eq!(
            repeated.termination(),
            CatalogTransferFixedPointTerminationV1::RepeatedAuthorityState
        );
    }

    #[test]
    fn real_catalog_context_digest_matches_runtime_constructor() {
        let fixture = fixture(FixtureOptions::default());
        let context = validate_catalog_bound_exact_transfer_context_v1(
            &fixture.rom,
            &fixture.pack,
            &fixture.topology,
            &fixture.catalog,
        )
        .unwrap();
        assert_eq!(
            context.catalog_definition_sha256(),
            fixture.catalog.canonical_definition_sha256()
        );
        assert_ne!(context.catalog_definition_sha256(), [0; 32]);
        assert_ne!(
            Sha256::digest(b"caller-authored evidence").as_slice(),
            &context.catalog_definition_sha256()
        );
    }
}
