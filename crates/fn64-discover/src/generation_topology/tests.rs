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

    /// Build a consistent pack+recipe pair from arbitrary overlay dense inputs,
    /// so a fixture may vary overlay geometry without desynchronizing the two
    /// sides `overlay_matches` compares.
    fn fixture_with_overlays(
        overlays: &[DenseAotGenerationInput<'static>],
    ) -> (NormalizedRom, DenseAotPackV1, Vec<OverlayLoadRecipeV1>) {
        let mut raw = vec![0u8; 0x5000];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&BOOT.to_be_bytes());
        for (index, byte) in raw[0x1000..0x3040].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(17);
        }
        let rom = crate::normalize(&raw).unwrap();
        let mut inputs = vec![DenseAotGenerationInput {
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
        }];
        inputs.extend(overlays.iter().cloned());
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

    /// The measured WCW/nWo Revenge and WCW vs nWo World Tour geometry (M1b
    /// two-overlay swap pair): both overlay images load at ONE VA inside the
    /// resident bank, and their invalidation union stops SHORT of
    /// `resident.load_end`, because that end is the fixed 1 MiB IPL3 boot copy
    /// -- a hardware constant no game's overlays are obliged to reach.
    ///
    /// This used to be rejected as `InvalidResidentSplit`. The resident tail's
    /// image is now clamped to where overlays actually stop writing, leaving
    /// the trailing resident span immutable rather than folding it into a
    /// generation whose invalidation could not cover it.
    #[test]
    fn swap_pair_union_short_of_resident_end_clamps_the_tail_image() {
        // Both overlays at OVERLAY, union bss_end at BOOT + 0x1800, which is
        // strictly between the split (BOOT + 0x1400) and the resident end
        // (BOOT + 0x2000): Revenge's shape in miniature.
        let (rom, pack, recipes) = fixture_with_overlays(&[
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
                bss_end: BOOT + 0x1780,
            },
            DenseAotGenerationInput {
                name: "overlay_b",
                source_rom_start: 0x3020,
                source_rom_end: 0x3040,
                load_start: OVERLAY,
                text_start: OVERLAY,
                text_end: OVERLAY + 0x10,
                data_start: OVERLAY + 0x10,
                data_end: OVERLAY + 0x20,
                bss_start: OVERLAY + 0x20,
                bss_end: BOOT + 0x1800,
            },
        ]);
        let topology =
            build_generation_topology_v1(&rom, &pack, "boot", RESIDENT_ID_DOMAIN, &recipes).unwrap();

        // The swap pair shares one VA, so the immutable prefix still ends at
        // the single split.
        assert_eq!(
            (
                topology.immutable_prefix.va_start,
                topology.immutable_prefix.va_end
            ),
            (BOOT, OVERLAY)
        );
        let resident = topology
            .generations
            .iter()
            .find(|generation| generation.role == CatalogGenerationRoleV1::ResidentTail)
            .unwrap();
        // Clamped to the union end, NOT to resident.load_end (BOOT + 0x2000).
        assert_eq!(
            (resident.image_start, resident.image_end),
            (OVERLAY, BOOT + 0x1800)
        );
        // The runtime invariant this clamp exists to preserve:
        // `PrecompiledGeneration::new` rejects invalidation that does not
        // contain the image.
        assert!(resident.invalidation_start <= resident.image_start);
        assert!(resident.invalidation_end >= resident.image_end);
        assert_eq!(resident.invalidation_end, BOOT + 0x1800);

        // The trailing resident span is owned by no generation: it is
        // immutable, exactly like the pre-split prefix.
        assert!(!topology.generations.iter().any(|generation| {
            generation.image_start <= BOOT + 0x1900 && BOOT + 0x1900 < generation.image_end
        }));

        // And the whole topology still builds a real runtime catalog, which is
        // what the old guard was indirectly protecting.
        crate::runtime_generation_catalog::build_backed_dense_generation_catalog_v1(
            &rom, &pack, &topology,
        )
        .unwrap();
    }

    /// Fail-closed: when overlays stop writing at or before the split there is
    /// no contended resident byte at all, so there is no resident-tail
    /// generation to compose. `EmptyResidentTail` rejects that precisely,
    /// instead of emitting a zero-length generation the runtime would refuse
    /// as `InvalidRange`.
    ///
    /// A well-formed recipe cannot reach it -- `build_dense_aot_pack_v1`
    /// already enforces `bss_end >= bss_start >= data_end > load_start`, so the
    /// union always exceeds the minimum `load_start` that becomes the split.
    /// That is precisely why the guard is asserted here rather than through a
    /// fixture: it is the defensive floor that keeps the clamp from ever
    /// silently producing an empty image if those upstream relations are later
    /// loosened. This test pins that a recipe set which WOULD trip it is
    /// unconstructible, so the guard's unreachability is a checked property
    /// rather than an assumption.
    #[test]
    fn an_empty_resident_tail_is_unconstructible_from_well_formed_recipes() {
        let (rom, _, _) = fixture();
        // bss_end below load_start is exactly the shape that would clamp the
        // tail to nothing, and the dense pack refuses to build it at all.
        assert!(build_dense_aot_pack_v1(
            &rom,
            &[DenseAotGenerationInput {
                name: "overlay_a",
                source_rom_start: 0x3000,
                source_rom_end: 0x3020,
                load_start: OVERLAY,
                text_start: OVERLAY,
                text_end: OVERLAY + 0x10,
                data_start: OVERLAY + 0x10,
                data_end: OVERLAY + 0x20,
                bss_start: OVERLAY + 0x20,
                bss_end: OVERLAY,
            }],
        )
        .is_err());
    }

    /// The clauses that survive: a split outside the resident bank is still a
    /// blanket rejection, because it describes no split of that bank at all.
    #[test]
    fn a_split_outside_the_resident_bank_is_still_rejected() {
        let (rom, pack, recipes) = fixture();
        for (load_start, label) in [
            (BOOT, "at the resident start"),
            (BOOT + 0x2000, "at the resident end"),
        ] {
            let mut moved = recipes.clone();
            for recipe in &mut moved {
                recipe.load_start = load_start;
            }
            assert_eq!(
                build_generation_topology_v1(&rom, &pack, "boot", RESIDENT_ID_DOMAIN, &moved),
                Err(GenerationTopologyError::InvalidResidentSplit),
                "a split {label} must not compose"
            );
        }
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
