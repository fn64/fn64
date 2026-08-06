    use super::*;
    use fn64_discover::dense_aot_pack::{DenseAotGenerationV1, DenseAotShardV1};
    use fn64_discover::facts::{
        function_entry_subject, BankAddr, CandidateDetector, FunctionEntryEvidence,
    };
    use fn64_discover::overlay_recipe::{OverlayLoadRecipeV1, OVERLAY_RECIPE_SCHEMA_V1};
    use fn64_discover::trace::{
        ExecutableImageCapture, ExecutableImageLineage, NormalizedRomDigest,
        EXECUTABLE_IMAGE_SCHEMA,
    };
    use fn64_recomp_rs::boot::{
        BootCicIdentity, BootCop0Context, BootRegion, Sha256Digest, BOOT_CONTEXT_SCHEMA_V1,
    };

    const SHA: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    #[test]
    fn runtime_catalog_builder_matches_generated_identity_and_digest_rules() {
        const BOOT_ROM: u32 = 0x1000;
        const OVERLAY_ROM: u32 = 0x1100;
        const BOOT_VA: u32 = 0x8000_0400;
        const SPLIT_VA: u32 = BOOT_VA + 0x20;
        const IMAGE_END: u32 = BOOT_VA + 0x40;
        let mut raw = vec![0u8; 0x1200];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&BOOT_VA.to_be_bytes());
        for (index, byte) in raw[BOOT_ROM as usize..BOOT_ROM as usize + 0x40]
            .iter_mut()
            .enumerate()
        {
            *byte = index as u8;
        }
        for (index, byte) in raw[OVERLAY_ROM as usize..OVERLAY_ROM as usize + 0x20]
            .iter_mut()
            .enumerate()
        {
            *byte = 0x80 | index as u8;
        }
        let rom = fn64_discover::normalize(&raw).unwrap();
        let overlay_bytes = &rom.bytes[OVERLAY_ROM as usize..OVERLAY_ROM as usize + 0x20];
        let recipe = OverlayLoadRecipeV1 {
            schema: OVERLAY_RECIPE_SCHEMA_V1.to_owned(),
            descriptor_rom_offset: 0x200,
            rom_start: OVERLAY_ROM,
            rom_end: OVERLAY_ROM + 0x20,
            load_start: SPLIT_VA,
            text_start: SPLIT_VA,
            text_end: IMAGE_END,
            data_start: IMAGE_END,
            data_end: IMAGE_END,
            bss_start: IMAGE_END,
            bss_end: IMAGE_END,
            loaded_sha256: format!("{:x}", Sha256::digest(overlay_bytes)),
            text_sha256: format!("{:x}", Sha256::digest(overlay_bytes)),
        };
        let pack = build_dense_aot_pack_v1(
            &rom,
            &[
                DenseAotGenerationInput {
                    name: BOOT_BANK,
                    source_rom_start: BOOT_ROM,
                    source_rom_end: BOOT_ROM + 0x40,
                    load_start: BOOT_VA,
                    text_start: BOOT_VA,
                    text_end: IMAGE_END,
                    data_start: IMAGE_END,
                    data_end: IMAGE_END,
                    bss_start: IMAGE_END,
                    bss_end: IMAGE_END,
                },
                DenseAotGenerationInput::from(("recovered_overlay_0", &recipe)),
            ],
        )
        .unwrap();
        let topology = build_generation_topology_v1(
            &rom,
            &pack,
            BOOT_BANK,
            WM_RESIDENT_TAIL_IDENTITY_DOMAIN_V1,
            std::slice::from_ref(&recipe),
        )
        .unwrap();
        let catalog = build_backed_dense_generation_catalog_v1(&rom, &pack, &topology).unwrap();
        let evidence = catalog.evidence_snapshot();

        assert_eq!(evidence.generations.len(), 2);
        assert_eq!(evidence.backings.len(), 2);
        for geometry in &topology.generations {
            let generation = evidence
                .generations
                .iter()
                .find(|candidate| candidate.generation.get() == geometry.generation_id)
                .unwrap();
            let (source_start, identity_name) = match geometry.role {
                CatalogGenerationRoleV1::ResidentTail => (BOOT_ROM + 0x20, "resident_tail"),
                CatalogGenerationRoleV1::Overlay => (OVERLAY_ROM, "recovered_overlay_0"),
            };
            let bytes = &rom.bytes[source_start as usize
                ..source_start as usize + (geometry.image_end - geometry.image_start) as usize];
            let words = bytes
                .chunks_exact(4)
                .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
                .collect::<Vec<_>>();
            let expected_bank = fn64_discover::dense_aot_pack::dense_aot_artifact_bank_id(
                &rom.sha256,
                identity_name,
                geometry.image_start,
                &words,
            );
            assert_eq!(generation.expected_sha256, Sha256::digest(bytes).as_slice());
            assert_eq!(generation.shards.len(), 1);
            assert_eq!(generation.shards[0].bank().get(), expected_bank);
            assert_eq!(generation.shards[0].start().get(), geometry.image_start);
            assert_eq!(generation.shards[0].end().get(), geometry.image_end);
            let backing = evidence
                .backings
                .iter()
                .find(|candidate| candidate.generation == generation.generation)
                .unwrap();
            assert_eq!(backing.spans.len(), 1);
            assert_eq!(
                backing.spans[0].physical_start(),
                geometry.invalidation_start & 0x1fff_ffff
            );
        }
        assert_eq!(catalog.reserved_banks().len(), 2);

        let mut digest_drift = topology.clone();
        digest_drift.generations[0].image_sha256 = "00".repeat(32);
        let error = build_backed_dense_generation_catalog_v1(&rom, &pack, &digest_drift)
            .err()
            .expect("topology digest drift must fail closed");
        assert!(error.contains("ROM digest disagrees with topology"));
    }

    fn dense_generation(
        name: &str,
        source_rom_start: u32,
        load_start: u32,
        byte_len: u32,
        shard_count: usize,
    ) -> DenseAotGenerationV1 {
        let source_rom_end = source_rom_start + byte_len;
        let load_end = load_start + byte_len;
        let shards = (0..shard_count)
            .map(|index| {
                let offset = index as u32 * DENSE_AOT_SHARD_BYTES;
                let shard_len = DENSE_AOT_SHARD_BYTES.min(byte_len - offset);
                DenseAotShardV1 {
                    index: index as u32,
                    source_rom_start: source_rom_start + offset,
                    source_rom_end: source_rom_start + offset + shard_len,
                    va_start: load_start + offset,
                    va_end: load_start + offset + shard_len,
                    sha256: SHA.to_string(),
                    delay_lookahead: None,
                    artifact_identity: SHA.to_string(),
                }
            })
            .collect();
        DenseAotGenerationV1 {
            name: name.to_string(),
            bank_id: 1,
            source_rom_start,
            source_rom_end,
            load_start,
            load_end,
            text_start: load_start,
            text_end: load_end,
            data_start: load_end,
            data_end: load_end,
            bss_start: load_end,
            bss_end: load_end,
            loaded_sha256: SHA.to_string(),
            aligned_entry_count: byte_len / 4,
            shards,
        }
    }

    fn overlay_recipe(
        index: usize,
        source_rom_start: u32,
        load_start: u32,
        shard_count: usize,
    ) -> OverlayLoadRecipeV1 {
        let byte_len = shard_count as u32 * DENSE_AOT_SHARD_BYTES;
        OverlayLoadRecipeV1 {
            schema: OVERLAY_RECIPE_SCHEMA_V1.to_string(),
            descriptor_rom_offset: 0x200 + index as u32 * 36,
            rom_start: source_rom_start,
            rom_end: source_rom_start + byte_len,
            load_start,
            text_start: load_start,
            text_end: load_start + byte_len,
            data_start: load_start + byte_len,
            data_end: load_start + byte_len,
            bss_start: load_start + byte_len,
            bss_end: load_start + byte_len,
            loaded_sha256: SHA.to_string(),
            text_sha256: SHA.to_string(),
        }
    }

    fn static_graph_fixture(
        first_overlay_start: u32,
    ) -> (DenseAotPackV1, Vec<OverlayLoadRecipeV1>) {
        let boot_start = 0x8000_0400;
        let mut generations = vec![dense_generation(
            BOOT_BANK,
            0x1000,
            boot_start,
            16 * DENSE_AOT_SHARD_BYTES,
            16,
        )];
        let overlay_shards = [3usize, 1, 6, 8];
        let recipes = overlay_shards
            .into_iter()
            .enumerate()
            .map(|(index, shard_count)| {
                let recipe = overlay_recipe(
                    index,
                    0x20_0000 + index as u32 * 0x10_0000,
                    first_overlay_start + index as u32 * 0x20_0000,
                    shard_count,
                );
                generations.push(dense_generation(
                    &format!("recovered_overlay_{index}"),
                    recipe.rom_start,
                    recipe.load_start,
                    recipe.rom_end - recipe.rom_start,
                    shard_count,
                ));
                recipe
            })
            .collect();
        (
            DenseAotPackV1 {
                schema: fn64_discover::dense_aot_pack::DENSE_AOT_PACK_SCHEMA_V1.to_string(),
                normalized_rom_sha256: SHA.to_string(),
                generations,
            },
            recipes,
        )
    }

    fn transfer_scan_snapshots_fixture() -> (DenseAotPackV1, Vec<ProgramSnapshotV1>) {
        const BOOT_ROM: u32 = 0x1000;
        const OVERLAY_ROM: u32 = 0x2000;
        const CANDIDATE_ROM: u32 = 0x3000;
        const BOOT_VA: u32 = 0x8000_0400;
        const OVERLAY_VA: u32 = 0x8000_1000;
        const CANDIDATE_VA: u32 = 0x8000_2000;
        const NOP: u32 = 0;
        const JR_RA: u32 = 0x03e0_0008;

        let words = |values: &[u32]| {
            values
                .iter()
                .flat_map(|word| word.to_be_bytes())
                .collect::<Vec<_>>()
        };
        let boot = words(&[
            0x0c00_0000 | ((OVERLAY_VA >> 2) & 0x03ff_ffff),
            NOP,
            JR_RA,
            NOP,
        ]);
        let overlay = words(&[JR_RA, NOP]);
        let candidate = words(&[JR_RA, NOP]);
        let mut source = vec![0u8; CANDIDATE_ROM as usize + candidate.len()];
        source[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        source[8..12].copy_from_slice(&BOOT_VA.to_be_bytes());
        source[BOOT_ROM as usize..BOOT_ROM as usize + boot.len()].copy_from_slice(&boot);
        source[OVERLAY_ROM as usize..OVERLAY_ROM as usize + overlay.len()]
            .copy_from_slice(&overlay);
        source[CANDIDATE_ROM as usize..CANDIDATE_ROM as usize + candidate.len()]
            .copy_from_slice(&candidate);
        let rom = fn64_discover::normalize(&source).unwrap();

        let mut facts = FactDb::new();
        for (bank, rom_start, va_start, bytes) in [
            (BOOT_BANK, BOOT_ROM, BOOT_VA, boot.as_slice()),
            (
                "recovered_overlay_0",
                OVERLAY_ROM,
                OVERLAY_VA,
                overlay.as_slice(),
            ),
            (
                "recovered_overlay_1",
                CANDIDATE_ROM,
                CANDIDATE_VA,
                candidate.as_slice(),
            ),
        ] {
            let mapping = facts.insert(Fact::RomMapping {
                bank: bank.to_string(),
                rom_space: RomAddressSpace::Physical,
                rom_start,
                rom_end: rom_start + bytes.len() as u32,
                va_start,
                va_end: va_start + bytes.len() as u32,
            });
            facts
                .conclude(
                    format!("bank:{bank}"),
                    ProofState::Proven,
                    vec![mapping],
                    "transfer scan test mapping",
                )
                .unwrap();
        }
        let entry = BankAddr::new(BOOT_BANK, BOOT_VA);
        let claim = facts.insert(Fact::FunctionEntryClaim {
            target: entry.clone(),
            detector: CandidateDetector::HardwareEntrypoint,
            evidence: FunctionEntryEvidence::RomHeaderEntrypoint,
            proposed_state: ProofState::Proven,
        });
        facts
            .conclude(
                function_entry_subject(&entry),
                ProofState::Proven,
                vec![claim],
                "transfer scan test hardware entry",
            )
            .unwrap();

        let inputs = [
            MaterializedBankInput {
                bank: BOOT_BANK,
                va_start: BOOT_VA,
                bytes: &boot,
                seed_roots: &[BOOT_VA],
            },
            MaterializedBankInput {
                bank: "recovered_overlay_0",
                va_start: OVERLAY_VA,
                bytes: &overlay,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "recovered_overlay_1",
                va_start: CANDIDATE_VA,
                bytes: &candidate,
                seed_roots: &[CANDIDATE_VA],
            },
        ];
        let snapshots = compose_materialized_banks_validated_v2(&rom, &facts, &inputs)
            .unwrap()
            .snapshots()
            .to_vec();

        let mut generations = vec![
            dense_generation(BOOT_BANK, BOOT_ROM, BOOT_VA, boot.len() as u32, 1),
            dense_generation(
                "recovered_overlay_0",
                OVERLAY_ROM,
                OVERLAY_VA,
                overlay.len() as u32,
                1,
            ),
            dense_generation(
                "recovered_overlay_1",
                CANDIDATE_ROM,
                CANDIDATE_VA,
                candidate.len() as u32,
                1,
            ),
        ];
        for (index, generation) in generations.iter_mut().enumerate() {
            generation.bank_id = index as u64 + 1;
        }
        (
            DenseAotPackV1 {
                schema: fn64_discover::dense_aot_pack::DENSE_AOT_PACK_SCHEMA_V1.to_string(),
                normalized_rom_sha256: rom.sha256,
                generations,
            },
            snapshots,
        )
    }

    #[test]
    fn transfer_scan_uses_cross_bank_overlay_authority_from_composition() {
        let (pack, snapshots) = transfer_scan_snapshots_fixture();
        let banks = dense_transfer_banks(&pack, &snapshots).unwrap();
        assert_eq!(banks[1].bank, "recovered_overlay_0");
        assert!(banks[1]
            .closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == 0x8000_1000));
        assert!(std::ptr::eq(
            banks[1].closure,
            &snapshots[1].banks[0].authority_closure
        ));
    }

    #[test]
    fn transfer_scan_excludes_candidate_only_traversal_hints() {
        let (pack, snapshots) = transfer_scan_snapshots_fixture();
        assert!(snapshots[2].banks[0]
            .closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == 0x8000_2000));
        let banks = dense_transfer_banks(&pack, &snapshots).unwrap();
        assert_eq!(banks[2].bank, "recovered_overlay_1");
        assert!(banks[2].closure.cfg.blocks.is_empty());
        assert!(std::ptr::eq(
            banks[2].closure,
            &snapshots[2].banks[0].authority_closure
        ));
    }

    #[test]
    fn transfer_scan_rejects_snapshot_order_or_identity_drift() {
        let (pack, mut snapshots) = transfer_scan_snapshots_fixture();
        snapshots.swap(1, 2);
        let error = dense_transfer_banks(&pack, &snapshots).unwrap_err();
        assert!(error.contains("identity/geometry mismatch"));
    }

    #[test]
    fn static_graph_names_overlay_split_resident_tail_instead_of_obsolete_boot_shards() {
        let first_overlay_start = 0x8000_0400 + 14 * DENSE_AOT_SHARD_BYTES + 0x8000;
        let (pack, recipes) = static_graph_fixture(first_overlay_start);
        let packages = expected_static_shard_packages(&pack, &recipes).unwrap();
        assert_eq!(packages.len(), 35);
        assert!(packages.contains("wm2000-block-shard-14"));
        assert!(!packages.contains("wm2000-block-shard-15"));
        assert!(!packages.contains("wm2000-block-shard-16"));
        assert!(packages.contains("wm2000-block-resident-tail-shard-00"));
        assert!(packages.contains("wm2000-block-resident-tail-shard-01"));
        assert!(packages.contains("wm2000-block-overlay-0-shard-02"));
        assert!(packages.contains("wm2000-block-overlay-3-shard-07"));
    }

    #[test]
    fn static_graph_rejects_dense_overlay_geometry_that_drifted_from_recipe() {
        let first_overlay_start = 0x8000_0400 + 14 * DENSE_AOT_SHARD_BYTES + 0x8000;
        let (mut pack, recipes) = static_graph_fixture(first_overlay_start);
        pack.generations[2].source_rom_start += 4;
        let error = expected_static_shard_packages(&pack, &recipes).unwrap_err();
        assert!(error.contains("dense overlay generation 1 disagrees"));
    }

    #[test]
    fn static_graph_rejects_a_resident_split_other_than_fifteen_plus_two() {
        let first_overlay_start = 0x8000_0400 + 14 * DENSE_AOT_SHARD_BYTES;
        let (pack, recipes) = static_graph_fixture(first_overlay_start);
        let error = expected_static_shard_packages(&pack, &recipes).unwrap_err();
        assert!(error.contains("14 static-prefix / 2 resident-tail"));
    }

    fn image(image_id: &str, va_start: u32, byte_len: u32) -> ExternalExecutableImageIdentityV1 {
        ExternalExecutableImageIdentityV1 {
            image_id: image_id.to_string(),
            lineage: "CpuWritten".to_string(),
            generation: 0,
            va_start,
            byte_len,
            sha256: SHA.to_string(),
            first_executed_pc: va_start,
        }
    }

    fn boot_bound_rom_and_context() -> (fn64_discover::NormalizedRom, BootContext) {
        let mut bytes = vec![0u8; 0x1000];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        bytes[0x3e] = b'E';
        let rom = fn64_discover::normalize(&bytes).unwrap();
        let context = BootContext {
            schema: BOOT_CONTEXT_SCHEMA_V1.to_string(),
            producer: "black-box-test".to_string(),
            normalized_rom_sha256: Sha256Digest::parse(&rom.sha256).unwrap(),
            cic: BootCicIdentity {
                ipl3_sha256: Sha256Digest::parse(&sha256_hex(&rom.bytes[0x40..0x1000])).unwrap(),
            },
            region: BootRegion {
                destination_code: b'E',
                tv_standard: BootTvStandard::Ntsc,
            },
            entry_pc: rom.header.entry_point,
            gprs: [0; 32],
            hi: 0,
            lo: 0,
            cp0: BootCop0Context { registers: [0; 32] },
        };
        (rom, context)
    }

    #[test]
    fn captured_general_vector_owns_only_the_entry_covered_by_its_exact_range() {
        let vectors = modeled_exception_vectors(&[image("general", 0x8000_0180, 16)]).unwrap();
        assert_eq!(
            vectors
                .iter()
                .map(|vector| vector.destination)
                .collect::<Vec<_>>(),
            MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1
        );
        assert!(!vectors
            .iter()
            .any(|vector| vector.destination == 0x8000_0100));
        for vector in vectors {
            if vector.destination == 0x8000_0180 {
                assert!(matches!(
                    vector.disposition,
                    ExceptionVectorDispositionV1::ExactCodeOwner(_)
                ));
            } else {
                assert!(matches!(
                    vector.disposition,
                    ExceptionVectorDispositionV1::Open { .. }
                ));
            }
        }
    }

    #[test]
    fn capture_range_does_not_own_vectors_other_than_its_first_fetch() {
        let vectors = modeled_exception_vectors(&[image("broad", 0x8000_0000, 0x184)]).unwrap();
        for vector in vectors {
            if vector.destination == 0x8000_0000 {
                assert!(matches!(
                    vector.disposition,
                    ExceptionVectorDispositionV1::ExactCodeOwner(_)
                ));
            } else {
                assert!(matches!(
                    vector.disposition,
                    ExceptionVectorDispositionV1::Open { .. }
                ));
            }
        }
    }

    #[test]
    fn ambiguous_external_vector_ownership_is_rejected() {
        let error = modeled_exception_vectors(&[
            image("general-a", 0x8000_0180, 16),
            image("general-b", 0x8000_0180, 16),
        ])
        .unwrap_err();
        assert!(error.contains("covered by multiple external executable images"));
    }

    #[test]
    fn initial_status_authority_binds_canonical_context_and_normalized_rom() {
        let (rom, mut context) = boot_bound_rom_and_context();
        context.cp0.registers[12] = 0x3400_0000;
        let authority = validated_initial_cop0_status_authority(&rom, context).unwrap();
        assert!(authority.bev_is_proven_clear());
        assert!(matches!(
            authority,
            InitialCop0StatusAuthorityV1::BootContext {
                destination_code: b'E',
                entry_pc: 0x8000_0400,
                cp0_status: 0x3400_0000,
                ..
            }
        ));

        let (rom, mut mismatch) = boot_bound_rom_and_context();
        mismatch.region.destination_code = b'P';
        let error = validated_initial_cop0_status_authority(&rom, mismatch).unwrap_err();
        assert!(error.contains("destination code"));
    }

    #[test]
    fn external_status_scan_uses_captured_words_and_first_fetch_root() {
        let words = vec![0x4088_6000, 0x03e0_0008, 0];
        let bytes = words
            .iter()
            .flat_map(|word: &u32| word.to_be_bytes())
            .collect::<Vec<_>>();
        let capture = ExecutableImageCapture {
            schema: EXECUTABLE_IMAGE_SCHEMA.to_string(),
            producer: "black-box-test".to_string(),
            normalized_rom_sha256: NormalizedRomDigest::try_from(SHA.to_string()).unwrap(),
            image_id: "external-status".to_string(),
            lineage: ExecutableImageLineage::CpuProduced,
            generation: 4,
            capture_pc: 0x8000_1000,
            first_executed_pc: 0x8000_1000,
            retired_instructions: 10,
            va_start: 0x8000_1000,
            byte_len: bytes.len() as u32,
            sha256: sha256_hex(&bytes),
            words,
        };
        let (scans, closures) = external_cop0_status_scans(&[capture]).unwrap();
        assert_eq!(scans.len(), 1);
        assert_eq!(closures.len(), 1);
        assert_eq!(scans[0].proven_code_writes.len(), 1);
        assert_eq!(scans[0].proven_code_writes[0].site_pc, 0x8000_1000);
        assert_eq!(scans[0].proven_code_value_proofs.len(), 1);
        assert!(scans[0].proven_code_value_proofs[0]
            .blockers
            .contains(&fn64_discover::source_closure::Cop0StatusValueBlockerV1::ValueOpen));
    }

    #[test]
    fn cache_inventory_promotes_only_proven_code_or_data() {
        use fn64_discover::cfg::WordClass;

        assert_eq!(
            cache_site_disposition(Some(WordClass::ProvenCode)),
            CacheSiteDispositionV1::ReachableInstruction
        );
        assert_eq!(
            cache_site_disposition(Some(WordClass::ProvenData)),
            CacheSiteDispositionV1::ProvenData
        );
        for word_class in [
            None,
            Some(WordClass::Unknown),
            Some(WordClass::CandidateCode),
            Some(WordClass::CandidateData),
            Some(WordClass::Conflict),
        ] {
            assert_eq!(
                cache_site_disposition(word_class),
                CacheSiteDispositionV1::Unclassified
            );
        }
    }
