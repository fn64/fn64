    use super::*;
    use crate::{
        BlockRun, CodeBank, CodeSpan, GeneratedBankRunner, InstructionBudget, RecompContext,
    };

    thread_local! {
        static BACKED_ACTIVATIONS: std::cell::RefCell<
            Vec<BackedGenerationActivationObservationV1>,
        > = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn record_backed_activation(observation: &BackedGenerationActivationObservationV1) {
        BACKED_ACTIVATIONS.with(|observations| observations.borrow_mut().push(observation.clone()));
    }

    const VA: GuestPc = GuestPc::new(0x8000_0100);

    fn generation(id: u64, bank: u64, bytes: &[u8]) -> PrecompiledGeneration {
        PrecompiledGeneration::new(
            GenerationId::new(id),
            VA,
            GuestPc::new(VA.get() + bytes.len() as u32),
            VA,
            GuestPc::new(VA.get() + bytes.len() as u32),
            Sha256::digest(bytes).into(),
            vec![PrecompiledShard::new(
                BankId::new(bank),
                VA,
                GuestPc::new(VA.get() + bytes.len() as u32),
            )
            .unwrap()],
        )
        .unwrap()
    }

    fn write_image_at(mem: &mut Rdram<'_>, start: GuestPc, bytes: &[u8]) {
        for (index, word) in bytes.chunks_exact(4).enumerate() {
            mem.store_w(
                0xffff_ffff_0000_0000 | (u64::from(start.get()) + index as u64 * 4),
                u32::from_be_bytes(word.try_into().unwrap()),
            );
        }
    }

    fn unreachable_runner(
        _entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        unreachable!("catalog validation never enters a runner")
    }

    fn program_with_bank(bank: CodeBank) -> BlockProgram {
        let id = bank.id();
        let mut program = BlockProgram::new();
        program
            .register(bank, GeneratedBankRunner::new(id, unreachable_runner))
            .unwrap();
        program
    }

    fn write_physical_bytes(storage: &mut [u8], physical_start: u32, bytes: &[u8]) {
        for (index, byte) in bytes.iter().copied().enumerate() {
            let physical = usize::try_from(physical_start).unwrap() + index;
            storage[physical ^ 3] = byte;
        }
    }

    fn generation_at(id: u64, bank: u64, start: GuestPc, bytes: &[u8]) -> PrecompiledGeneration {
        let end = GuestPc::new(start.get() + u32::try_from(bytes.len()).unwrap());
        PrecompiledGeneration::new(
            GenerationId::new(id),
            start,
            end,
            start,
            end,
            Sha256::digest(bytes).into(),
            vec![PrecompiledShard::new(BankId::new(bank), start, end).unwrap()],
        )
        .unwrap()
    }

    fn backed_definition(
        generation: PrecompiledGeneration,
        backing: PrecompiledGenerationBackingV1,
    ) -> BackedPrecompiledGenerationCatalogV1 {
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog.register(generation).unwrap();
        BackedPrecompiledGenerationCatalogV1::new(catalog, vec![backing]).unwrap()
    }

    #[test]
    fn activation_observer_reports_only_successful_physically_backed_selection() {
        let image_a = [0x24, 0x02, 0x00, 0x01];
        let image_b = [0x24, 0x02, 0x00, 0x02];
        let digest_a: [u8; 32] = Sha256::digest(image_a).into();
        let digest_b: [u8; 32] = Sha256::digest(image_b).into();
        BACKED_ACTIVATIONS.with(|observations| observations.borrow_mut().clear());
        let original = set_backed_generation_activation_observer_v1(None);
        assert!(
            set_backed_generation_activation_observer_v1(Some(record_backed_activation)).is_none()
        );

        let mut unbacked = PrecompiledGenerationCatalog::new();
        unbacked.register(generation(1, 11, &image_a)).unwrap();
        let mut virtual_storage = vec![0u8; 0x200];
        let mut virtual_mem = Rdram::new(&mut virtual_storage);
        write_image_at(&mut virtual_mem, VA, &image_a);
        unbacked.activate_for_fetch(VA, &virtual_mem).unwrap();

        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog.register(generation(1, 11, &image_a)).unwrap();
        catalog.register(generation(2, 12, &image_b)).unwrap();
        let backing = |id| {
            PrecompiledGenerationBackingV1::new(
                GenerationId::new(id),
                vec![BackedExecutableSpanV1::new(VA, 0x100, 4).unwrap()],
            )
            .unwrap()
        };
        let mut backed =
            BackedPrecompiledGenerationCatalogV1::new(catalog, vec![backing(1), backing(2)])
                .unwrap();
        let mut storage = vec![0u8; 0x200];
        write_physical_bytes(&mut storage, 0x100, &image_a);
        backed
            .activate_for_fetch(VA, &Rdram::new(&mut storage))
            .unwrap();
        backed
            .activate_for_fetch(VA, &Rdram::new(&mut storage))
            .unwrap();
        assert!(matches!(
            backed.activate_for_fetch(GuestPc::new(VA.get() + 0x100), &Rdram::new(&mut storage)),
            Err(GenerationLookupError::UnmappedPc { .. })
        ));
        write_physical_bytes(&mut storage, 0x100, &image_b);
        backed
            .activate_for_fetch(VA, &Rdram::new(&mut storage))
            .unwrap();
        write_physical_bytes(&mut storage, 0x100, &[0xff; 4]);
        // Two generations contain VA and the corrupted bytes match neither, so
        // this is the "none of the candidates matched" case rather than "this
        // one image changed". Reporting only the first miss is what made
        // WM2000's three-candidate activation read as a single-generation
        // failure.
        assert!(matches!(
            backed.activate_for_fetch(VA, &Rdram::new(&mut storage)),
            Err(GenerationLookupError::NoGenerationMatched { candidates: 2, .. })
        ));

        let image_long = [
            image_a[0], image_a[1], image_a[2], image_a[3], 0x24, 0x03, 0x00, 0x03,
        ];
        let mut ambiguous_catalog = PrecompiledGenerationCatalog::new();
        ambiguous_catalog
            .register(generation(4, 14, &image_long))
            .unwrap();
        ambiguous_catalog
            .register(generation(5, 15, &image_a))
            .unwrap();
        let mut ambiguous = BackedPrecompiledGenerationCatalogV1::new(
            ambiguous_catalog,
            vec![
                PrecompiledGenerationBackingV1::new(
                    GenerationId::new(4),
                    vec![BackedExecutableSpanV1::new(VA, 0x100, 8).unwrap()],
                )
                .unwrap(),
                PrecompiledGenerationBackingV1::new(
                    GenerationId::new(5),
                    vec![BackedExecutableSpanV1::new(VA, 0x100, 4).unwrap()],
                )
                .unwrap(),
            ],
        )
        .unwrap();
        write_physical_bytes(&mut storage, 0x100, &image_long);
        assert!(matches!(
            ambiguous.activate_for_fetch(VA, &Rdram::new(&mut storage)),
            Err(GenerationLookupError::AmbiguousLiveImage { .. })
        ));
        assert!(ambiguous.active_segments().is_empty());

        assert!(set_backed_generation_activation_observer_v1(None).is_some());
        set_backed_generation_activation_observer_v1(original);

        BACKED_ACTIVATIONS.with(|observations| {
            assert_eq!(
                *observations.borrow(),
                vec![
                    BackedGenerationActivationObservationV1 {
                        requested_pc: VA,
                        generation: GenerationId::new(1),
                        entry: ExecutionKey::new(BankId::new(11), VA),
                        matched_image_sha256: digest_a,
                        newly_activated: true,
                        retired: Vec::new(),
                    },
                    BackedGenerationActivationObservationV1 {
                        requested_pc: VA,
                        generation: GenerationId::new(1),
                        entry: ExecutionKey::new(BankId::new(11), VA),
                        matched_image_sha256: digest_a,
                        newly_activated: false,
                        retired: vec![GenerationId::new(1)],
                    },
                    BackedGenerationActivationObservationV1 {
                        requested_pc: VA,
                        generation: GenerationId::new(2),
                        entry: ExecutionKey::new(BankId::new(12), VA),
                        matched_image_sha256: digest_b,
                        newly_activated: true,
                        retired: vec![GenerationId::new(1)],
                    },
                ]
            );
        });
    }

    #[test]
    fn initial_generation_images_accept_zero_or_one_exact_alternative() {
        let first = [0x24, 0x02, 0x00, 0x01];
        let second = [0x24, 0x03, 0x00, 0x02];
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog.register(generation(1, 11, &first)).unwrap();
        catalog.register(generation(2, 12, &second)).unwrap();
        let backed = BackedPrecompiledGenerationCatalogV1::new(
            catalog,
            [1, 2]
                .into_iter()
                .map(|id| {
                    PrecompiledGenerationBackingV1::new(
                        GenerationId::new(id),
                        vec![BackedExecutableSpanV1::new(VA, 0x100, 4).unwrap()],
                    )
                    .unwrap()
                })
                .collect(),
        )
        .unwrap();

        let mut storage = vec![0u8; 0x200];
        {
            let mem = Rdram::new(&mut storage);
            assert!(backed
                .validate_initial_physical_images(|physical| mem.load_physical_bu(physical))
                .unwrap()
                .is_empty());
        }

        write_physical_bytes(&mut storage, 0x100, &first);
        {
            let mem = Rdram::new(&mut storage);
            assert_eq!(
                backed
                    .validate_initial_physical_images(|physical| mem.load_physical_bu(physical))
                    .unwrap(),
                [GenerationId::new(1)]
            );
        }

        write_physical_bytes(&mut storage, 0x100, &[0xff, 0xee, 0xdd, 0xcc]);
        let mem = Rdram::new(&mut storage);
        assert!(matches!(
            backed.validate_initial_physical_images(|physical| mem.load_physical_bu(physical)),
            Err(InitialGenerationImageErrorV1::UnrecognizedNonzeroByte {
                physical_address: 0x100,
                actual: 0xff,
            })
        ));
    }

    #[test]
    fn backed_catalog_definition_sha256_is_canonical_and_excludes_activation_state() {
        let first_start = GuestPc::new(0x0040_1000);
        let second_start = GuestPc::new(0x0040_2000);
        let first_bytes = [0x24, 0x02, 0x00, 0x01, 0x24, 0x03, 0x00, 0x02];
        let second_bytes = [0x24, 0x04, 0x00, 0x03, 0x24, 0x05, 0x00, 0x04];
        let make = |reverse: bool| {
            let generations = [
                generation_at(1, 11, first_start, &first_bytes),
                generation_at(2, 12, second_start, &second_bytes),
            ];
            let backing = |id: u64, start: GuestPc, first_physical: u32, second_physical: u32| {
                PrecompiledGenerationBackingV1::new(
                    GenerationId::new(id),
                    vec![
                        BackedExecutableSpanV1::new(
                            GuestPc::new(start.get() + 4),
                            second_physical,
                            4,
                        )
                        .unwrap(),
                        BackedExecutableSpanV1::new(start, first_physical, 4).unwrap(),
                    ],
                )
                .unwrap()
            };
            let backings = [
                backing(1, first_start, 0x100, 0x240),
                backing(2, second_start, 0x300, 0x440),
            ];
            let mut catalog = PrecompiledGenerationCatalog::new();
            let order = if reverse { [1, 0] } else { [0, 1] };
            for index in order {
                catalog.register(generations[index].clone()).unwrap();
            }
            BackedPrecompiledGenerationCatalogV1::new(
                catalog,
                order
                    .into_iter()
                    .map(|index| backings[index].clone())
                    .collect(),
            )
            .unwrap()
        };

        let baseline = make(false);
        let expected = baseline.canonical_definition_sha256();
        assert_eq!(make(true).canonical_definition_sha256(), expected);

        let mut activated = make(false);
        let mut storage = vec![0u8; 0x500];
        write_physical_bytes(&mut storage, 0x100, &first_bytes[..4]);
        write_physical_bytes(&mut storage, 0x240, &first_bytes[4..]);
        activated
            .activate_for_fetch(first_start, &Rdram::new(&mut storage))
            .unwrap();
        assert!(!activated.active_segments().is_empty());
        assert_eq!(activated.canonical_definition_sha256(), expected);
    }

    #[test]
    fn backed_catalog_definition_sha256_binds_generation_shards_and_backing_spans() {
        let image_start = GuestPc::new(VA.get() + 4);
        let image_end = GuestPc::new(VA.get() + 12);
        let invalidation_end = GuestPc::new(VA.get() + 16);
        let make = |id: u64,
                    image_start: GuestPc,
                    image_end: GuestPc,
                    invalidation_start: GuestPc,
                    invalidation_end: GuestPc,
                    expected_sha256: [u8; 32],
                    shards: Vec<PrecompiledShard>,
                    spans: Vec<BackedExecutableSpanV1>| {
            let generation = PrecompiledGeneration::new(
                GenerationId::new(id),
                image_start,
                image_end,
                invalidation_start,
                invalidation_end,
                expected_sha256,
                shards,
            )
            .unwrap();
            let backing =
                PrecompiledGenerationBackingV1::new(GenerationId::new(id), spans).unwrap();
            backed_definition(generation, backing).canonical_definition_sha256()
        };
        let shards = |bank: u64, start: GuestPc, end: GuestPc| {
            vec![PrecompiledShard::new(BankId::new(bank), start, end).unwrap()]
        };
        let spans = |start: GuestPc, physical: u32, len: u32| {
            vec![BackedExecutableSpanV1::new(start, physical, len).unwrap()]
        };
        let baseline = make(
            1,
            image_start,
            image_end,
            VA,
            invalidation_end,
            [0x11; 32],
            shards(11, image_start, image_end),
            spans(VA, 0x100, 16),
        );

        let variants = [
            make(
                2,
                image_start,
                image_end,
                VA,
                invalidation_end,
                [0x11; 32],
                shards(11, image_start, image_end),
                spans(VA, 0x100, 16),
            ),
            make(
                1,
                VA,
                image_end,
                VA,
                invalidation_end,
                [0x11; 32],
                shards(11, VA, image_end),
                spans(VA, 0x100, 16),
            ),
            make(
                1,
                image_start,
                invalidation_end,
                VA,
                invalidation_end,
                [0x11; 32],
                shards(11, image_start, invalidation_end),
                spans(VA, 0x100, 16),
            ),
            make(
                1,
                image_start,
                image_end,
                image_start,
                invalidation_end,
                [0x11; 32],
                shards(11, image_start, image_end),
                spans(image_start, 0x104, 12),
            ),
            make(
                1,
                image_start,
                image_end,
                VA,
                image_end,
                [0x11; 32],
                shards(11, image_start, image_end),
                spans(VA, 0x100, 12),
            ),
            make(
                1,
                image_start,
                image_end,
                VA,
                invalidation_end,
                [0x22; 32],
                shards(11, image_start, image_end),
                spans(VA, 0x100, 16),
            ),
            make(
                1,
                image_start,
                image_end,
                VA,
                invalidation_end,
                [0x11; 32],
                shards(12, image_start, image_end),
                spans(VA, 0x100, 16),
            ),
            make(
                1,
                image_start,
                image_end,
                VA,
                invalidation_end,
                [0x11; 32],
                vec![
                    PrecompiledShard::new(
                        BankId::new(11),
                        image_start,
                        GuestPc::new(image_start.get() + 4),
                    )
                    .unwrap(),
                    PrecompiledShard::new(
                        BankId::new(12),
                        GuestPc::new(image_start.get() + 4),
                        image_end,
                    )
                    .unwrap(),
                ],
                spans(VA, 0x100, 16),
            ),
            make(
                1,
                image_start,
                image_end,
                VA,
                invalidation_end,
                [0x11; 32],
                shards(11, image_start, image_end),
                spans(VA, 0x200, 16),
            ),
            make(
                1,
                image_start,
                image_end,
                VA,
                invalidation_end,
                [0x11; 32],
                shards(11, image_start, image_end),
                vec![
                    BackedExecutableSpanV1::new(VA, 0x100, 8).unwrap(),
                    BackedExecutableSpanV1::new(GuestPc::new(VA.get() + 8), 0x240, 8).unwrap(),
                ],
            ),
        ];
        assert!(variants.into_iter().all(|variant| variant != baseline));
    }

    #[test]
    fn backed_catalog_hashes_segmented_physical_pages_in_virtual_order() {
        let start = GuestPc::new(0x0040_1000);
        let bytes = [0x24, 0x02, 0x00, 0x01, 0x24, 0x03, 0x00, 0x02];
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog
            .register(generation_at(1, 11, start, &bytes))
            .unwrap();
        let backing = PrecompiledGenerationBackingV1::new(
            GenerationId::new(1),
            vec![
                BackedExecutableSpanV1::new(start, 0x100, 4).unwrap(),
                BackedExecutableSpanV1::new(GuestPc::new(start.get() + 4), 0x240, 4).unwrap(),
            ],
        )
        .unwrap();
        let mut backed = BackedPrecompiledGenerationCatalogV1::new(catalog, vec![backing]).unwrap();
        let mut storage = vec![0u8; 0x300];
        write_physical_bytes(&mut storage, 0x100, &bytes[..4]);
        write_physical_bytes(&mut storage, 0x240, &bytes[4..]);
        let mem = Rdram::new(&mut storage);

        assert_eq!(
            backed.activate_for_fetch(start, &mem).unwrap().entry,
            ExecutionKey::new(BankId::new(11), start)
        );
        assert_eq!(
            backed
                .resolve_active(GuestPc::new(start.get() + 4))
                .unwrap(),
            ExecutionKey::new(BankId::new(11), GuestPc::new(start.get() + 4))
        );
    }

    #[test]
    fn generation_backing_rejects_gaps_and_out_of_rdram_spans() {
        let gap = PrecompiledGenerationBackingV1::new(
            GenerationId::new(1),
            vec![
                BackedExecutableSpanV1::new(VA, 0x100, 4).unwrap(),
                BackedExecutableSpanV1::new(GuestPc::new(VA.get() + 8), 0x200, 4).unwrap(),
            ],
        );
        assert!(matches!(
            gap,
            Err(BackedGenerationCatalogErrorV1::BackingCoverageGap { .. })
        ));
        assert!(matches!(
            BackedExecutableSpanV1::new(VA, crate::runtime::RDRAM_LEN as u32 - 4, 8,),
            Err(BackedGenerationCatalogErrorV1::InvalidBackingSpan { .. })
        ));
    }

    #[test]
    fn backed_catalog_requires_one_exact_mapping_per_generation() {
        let bytes = [0x24, 0x02, 0x00, 0x01];
        let mut missing_catalog = PrecompiledGenerationCatalog::new();
        missing_catalog.register(generation(1, 11, &bytes)).unwrap();
        assert!(matches!(
            BackedPrecompiledGenerationCatalogV1::new(missing_catalog, Vec::new()),
            Err(BackedGenerationCatalogErrorV1::MissingGenerationBacking {
                generation
            }) if generation == GenerationId::new(1)
        ));

        let mut geometry_catalog = PrecompiledGenerationCatalog::new();
        geometry_catalog
            .register(generation(1, 11, &bytes))
            .unwrap();
        let short = PrecompiledGenerationBackingV1::new(
            GenerationId::new(1),
            vec![BackedExecutableSpanV1::new(VA, 0x100, 8).unwrap()],
        )
        .unwrap();
        assert!(matches!(
            BackedPrecompiledGenerationCatalogV1::new(geometry_catalog, vec![short]),
            Err(BackedGenerationCatalogErrorV1::BackingGeometryMismatch { .. })
        ));

        let empty_catalog = PrecompiledGenerationCatalog::new();
        let unknown = PrecompiledGenerationBackingV1::new(
            GenerationId::new(9),
            vec![BackedExecutableSpanV1::new(VA, 0x100, 4).unwrap()],
        )
        .unwrap();
        assert!(matches!(
            BackedPrecompiledGenerationCatalogV1::new(empty_catalog, vec![unknown]),
            Err(BackedGenerationCatalogErrorV1::UnknownGenerationBacking {
                generation
            }) if generation == GenerationId::new(9)
        ));
    }

    #[test]
    fn generation_validation_rejects_unclaimed_static_overlap_but_allows_adjacency() {
        let bytes = [0x24, 0x02, 0x00, 0x01];
        let make_backed = || {
            let mut catalog = PrecompiledGenerationCatalog::new();
            catalog.register(generation(1, 11, &bytes)).unwrap();
            BackedPrecompiledGenerationCatalogV1::new(
                catalog,
                vec![PrecompiledGenerationBackingV1::new(
                    GenerationId::new(1),
                    vec![BackedExecutableSpanV1::new(VA, 0x100, 4).unwrap()],
                )
                .unwrap()],
            )
            .unwrap()
        };
        let reserved = CodeBank::new(BankId::new(11), VA, vec![0x2402_0001]).unwrap();
        let mut overlapping = program_with_bank(reserved.clone());
        overlapping
            .register(
                CodeBank::new(BankId::new(12), VA, vec![0]).unwrap(),
                GeneratedBankRunner::new(BankId::new(12), unreachable_runner),
            )
            .unwrap();
        assert!(matches!(
            make_backed().validate_program(&overlapping),
            Err(GenerationCatalogError::StaticBankOverlapsGenerationOwnership {
                bank,
                generation,
                ..
            }) if bank == BankId::new(12) && generation == GenerationId::new(1)
        ));

        let mut adjacent = program_with_bank(reserved);
        adjacent
            .register(
                CodeBank::new(BankId::new(12), GuestPc::new(VA.get() + 4), vec![0]).unwrap(),
                GeneratedBankRunner::new(BankId::new(12), unreachable_runner),
            )
            .unwrap();
        assert_eq!(make_backed().validate_program(&adjacent), Ok(()));
    }

    #[test]
    fn overlapping_generation_alternatives_require_identical_backing_mapping() {
        let image_a = [0x24, 0x02, 0x00, 0x01];
        let image_b = [0x24, 0x02, 0x00, 0x02];
        let make_catalog = || {
            let mut catalog = PrecompiledGenerationCatalog::new();
            catalog.register(generation(1, 11, &image_a)).unwrap();
            catalog.register(generation(2, 12, &image_b)).unwrap();
            catalog
        };
        let backing = |generation, physical| {
            PrecompiledGenerationBackingV1::new(
                GenerationId::new(generation),
                vec![BackedExecutableSpanV1::new(VA, physical, 4).unwrap()],
            )
            .unwrap()
        };

        assert!(matches!(
            BackedPrecompiledGenerationCatalogV1::new(
                make_catalog(),
                vec![backing(1, 0x100), backing(2, 0x200)],
            ),
            Err(BackedGenerationCatalogErrorV1::InconsistentOverlappingMappings {
                first,
                second,
            }) if first == GenerationId::new(1) && second == GenerationId::new(2)
        ));
        assert!(BackedPrecompiledGenerationCatalogV1::new(
            make_catalog(),
            vec![backing(2, 0x100), backing(1, 0x100)],
        )
        .is_ok());
    }

    #[test]
    fn physical_write_retires_every_split_segment_of_the_affected_generation() {
        let resident_bytes = [
            0x24, 0x02, 0x00, 0x01, 0x24, 0x03, 0x00, 0x01, 0x24, 0x04, 0x00, 0x01, 0x24, 0x05,
            0x00, 0x01,
        ];
        let overlay_bytes = [0x24, 0x03, 0x00, 0x02, 0x24, 0x04, 0x00, 0x02];
        let overlay_start = GuestPc::new(VA.get() + 4);
        let overlay_end = GuestPc::new(VA.get() + 12);
        let resident = generation(1, 11, &resident_bytes);
        let overlay = PrecompiledGeneration::new(
            GenerationId::new(2),
            overlay_start,
            overlay_end,
            overlay_start,
            overlay_end,
            Sha256::digest(overlay_bytes).into(),
            vec![PrecompiledShard::new(BankId::new(12), overlay_start, overlay_end).unwrap()],
        )
        .unwrap();
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog.register(resident).unwrap();
        catalog.register(overlay).unwrap();
        let resident_backing = PrecompiledGenerationBackingV1::new(
            GenerationId::new(1),
            vec![BackedExecutableSpanV1::new(VA, 0x100, 16).unwrap()],
        )
        .unwrap();
        let overlay_backing = PrecompiledGenerationBackingV1::new(
            GenerationId::new(2),
            vec![BackedExecutableSpanV1::new(overlay_start, 0x104, 8).unwrap()],
        )
        .unwrap();
        let mut backed = BackedPrecompiledGenerationCatalogV1::new(
            catalog,
            vec![resident_backing, overlay_backing],
        )
        .unwrap();
        let inactive_evidence = backed.evidence_snapshot();
        assert_eq!(
            inactive_evidence.schema,
            BACKED_GENERATION_CATALOG_EVIDENCE_SCHEMA_V1
        );
        assert!(inactive_evidence.active_segments.is_empty());
        assert_eq!(inactive_evidence.generations.len(), 2);
        assert_eq!(inactive_evidence.backings.len(), 2);
        assert_eq!(
            backed.physical_invalidation_ranges(),
            vec![PhysicalInvalidationRangeV1 {
                physical_start: 0x100,
                physical_end: 0x110,
            }]
        );
        let mut storage = vec![0u8; 0x200];
        write_physical_bytes(&mut storage, 0x100, &resident_bytes);
        {
            let mem = Rdram::new(&mut storage);
            backed.activate_for_fetch(VA, &mem).unwrap();
        }
        write_physical_bytes(&mut storage, 0x104, &overlay_bytes);
        {
            let mem = Rdram::new(&mut storage);
            backed.activate_for_fetch(overlay_start, &mem).unwrap();
        }
        assert_ne!(backed.evidence_snapshot(), inactive_evidence);
        assert_eq!(backed.active_segments().len(), 3);
        assert!(backed
            .invalidate_physical_write(0x110, 0x114)
            .unwrap()
            .is_empty());
        assert_eq!(
            backed.invalidate_physical_write(0x100, 0x104).unwrap(),
            vec![GenerationId::new(1)]
        );
        assert_eq!(
            backed.resolve_active(VA),
            Err(GenerationLookupError::NoActiveGeneration { pc: VA })
        );
        assert_eq!(
            backed.resolve_active(overlay_start).unwrap().bank,
            BankId::new(12)
        );
        let resident_tail = GuestPc::new(VA.get() + 12);
        assert_eq!(
            backed.resolve_active(resident_tail),
            Err(GenerationLookupError::NoActiveGeneration { pc: resident_tail })
        );
        assert!(matches!(
            backed.invalidate_physical_write(0x104, 0x104),
            Err(BackedGenerationCatalogErrorV1::InvalidPhysicalWriteRange { .. })
        ));
    }

    #[test]
    fn catalog_validation_rejects_a_missing_generated_shard_bank() {
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog
            .register(generation(1, 11, &[0x24, 0x02, 0x00, 0x01]))
            .unwrap();

        assert_eq!(
            catalog.validate_program(&BlockProgram::new()),
            Err(GenerationCatalogError::MissingShardBank {
                generation: GenerationId::new(1),
                bank: BankId::new(11),
            })
        );
    }

    #[test]
    fn catalog_validation_rejects_nonexact_shard_bank_geometry() {
        let bytes = [0x24, 0x02, 0x00, 0x01];
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog.register(generation(1, 11, &bytes)).unwrap();
        let bank = BankId::new(11);
        let program = program_with_bank(
            CodeBank::from_spans(
                bank,
                vec![
                    CodeSpan::new(bank, VA, vec![0x2402_0001]).unwrap(),
                    CodeSpan::new(bank, GuestPc::new(VA.get() + 8), vec![0]).unwrap(),
                ],
            )
            .unwrap(),
        );

        assert!(matches!(
            catalog.validate_program(&program),
            Err(GenerationCatalogError::ShardBankGeometry {
                generation,
                bank: actual_bank,
                actual_spans: 2,
                ..
            }) if generation == GenerationId::new(1) && actual_bank == bank
        ));
    }

    #[test]
    fn catalog_validation_accepts_the_exact_installed_generated_bank() {
        let bytes = [0x24, 0x02, 0x00, 0x01];
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog.register(generation(1, 11, &bytes)).unwrap();
        let bank = BankId::new(11);
        let program = program_with_bank(CodeBank::new(bank, VA, vec![0x2402_0001]).unwrap());

        assert_eq!(catalog.validate_program(&program), Ok(()));
    }

    #[test]
    fn digest_selection_reuses_a_retired_aot_generation() {
        let image_a = [0x24, 0x02, 0x00, 0x01];
        let image_b = [0x24, 0x02, 0x00, 0x02];
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog.register(generation(1, 11, &image_a)).unwrap();
        catalog.register(generation(2, 12, &image_b)).unwrap();
        let mut storage = vec![0u8; 0x200];

        let mut mem = Rdram::new(&mut storage);
        write_image_at(&mut mem, VA, &image_a);
        assert_eq!(
            catalog.resolve_active(VA),
            Err(GenerationLookupError::NoActiveGeneration { pc: VA })
        );
        let a = catalog.activate_for_fetch(VA, &mem).unwrap();
        assert_eq!(a.entry.bank, BankId::new(11));
        assert!(a.newly_activated);
        write_image_at(&mut mem, VA, &image_b);
        assert_eq!(catalog.resolve_active(VA).unwrap().bank, BankId::new(11));
        let b = catalog.activate_for_fetch(VA, &mem).unwrap();
        assert_eq!(b.entry.bank, BankId::new(12));
        assert_eq!(b.retired, vec![GenerationId::new(1)]);
        write_image_at(&mut mem, VA, &image_a);
        let a_again = catalog.activate_for_fetch(VA, &mem).unwrap();
        assert_eq!(a_again.entry.bank, BankId::new(11));
        assert_eq!(a_again.retired, vec![GenerationId::new(2)]);
        assert_eq!(catalog.active_generations(), vec![GenerationId::new(1)]);
    }

    #[test]
    fn unknown_live_digest_is_a_typed_aot_miss() {
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog
            .register(generation(1, 11, &[0x24, 0x02, 0x00, 0x01]))
            .unwrap();
        let mut storage = vec![0u8; 0x200];
        let mut mem = Rdram::new(&mut storage);
        write_image_at(&mut mem, VA, &[0x24, 0x02, 0x00, 0x03]);
        assert!(matches!(
            catalog.activate_for_fetch(VA, &mem),
            Err(GenerationLookupError::AotMiss(AotMiss { .. }))
        ));
        assert!(catalog.active_generations().is_empty());
    }

    #[test]
    fn a_generation_may_end_mid_shard() {
        // Shards tile in fixed blocks, but an image need not be a whole number
        // of them. WM2000's overlay 1 is the witness: 0x5df0 of text inside a
        // 0xddc0 invalidation extent, so one 64 KiB shard cannot fit inside
        // the extent and the image cannot be rounded up to a shard boundary.
        // The final shard is allowed to overhang; the digest covers only
        // [image_start, image_end), so the overhang carries no identity.
        let generation = PrecompiledGeneration::new(
            GenerationId::new(1),
            VA,
            GuestPc::new(VA.get() + 4),
            VA,
            GuestPc::new(VA.get() + 4),
            [0; 32],
            vec![PrecompiledShard::new(BankId::new(1), VA, GuestPc::new(VA.get() + 8)).unwrap()],
        )
        .expect("a shard overhanging image_end is admitted");
        assert_eq!(generation.byte_len(), 4);
    }

    #[test]
    fn a_shard_starting_at_image_end_is_still_rejected() {
        // Relaxing the END must not admit a shard that contributes nothing.
        // This one starts exactly where the image stops, so it is a gap in
        // disguise rather than coverage.
        let error = PrecompiledGeneration::new(
            GenerationId::new(1),
            VA,
            GuestPc::new(VA.get() + 4),
            VA,
            GuestPc::new(VA.get() + 4),
            [0; 32],
            vec![
                PrecompiledShard::new(BankId::new(1), VA, GuestPc::new(VA.get() + 4)).unwrap(),
                PrecompiledShard::new(
                    BankId::new(2),
                    GuestPc::new(VA.get() + 4),
                    GuestPc::new(VA.get() + 8),
                )
                .unwrap(),
            ],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            GenerationCatalogError::ShardCoverage { .. }
        ));
    }

    #[test]
    fn shard_union_must_exactly_tile_the_hashed_image() {
        let error = PrecompiledGeneration::new(
            GenerationId::new(1),
            VA,
            GuestPc::new(VA.get() + 8),
            VA,
            GuestPc::new(VA.get() + 8),
            [0; 32],
            vec![PrecompiledShard::new(BankId::new(1), VA, GuestPc::new(VA.get() + 4)).unwrap()],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            GenerationCatalogError::ShardCoverage { .. }
        ));
    }

    #[test]
    fn overlapping_activation_splits_and_preserves_unaffected_ownership() {
        let resident_bytes = [
            0x24, 0x02, 0x00, 0x01, 0x24, 0x03, 0x00, 0x01, 0x24, 0x04, 0x00, 0x01, 0x24, 0x05,
            0x00, 0x01,
        ];
        let overlay_bytes = [0x24, 0x03, 0x00, 0x02, 0x24, 0x04, 0x00, 0x02];
        let overlay_start = GuestPc::new(VA.get() + 4);
        let overlay_end = GuestPc::new(VA.get() + 12);
        let resident = generation(1, 11, &resident_bytes);
        let overlay = PrecompiledGeneration::new(
            GenerationId::new(2),
            overlay_start,
            overlay_end,
            overlay_start,
            overlay_end,
            Sha256::digest(overlay_bytes).into(),
            vec![PrecompiledShard::new(BankId::new(12), overlay_start, overlay_end).unwrap()],
        )
        .unwrap();
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog.register(resident).unwrap();
        catalog.register(overlay).unwrap();
        let mut storage = vec![0u8; 0x200];
        let mut mem = Rdram::new(&mut storage);
        write_image_at(&mut mem, VA, &resident_bytes);
        catalog.activate_for_fetch(VA, &mem).unwrap();

        write_image_at(&mut mem, overlay_start, &overlay_bytes);
        catalog.activate_for_fetch(overlay_start, &mem).unwrap();

        assert_eq!(
            catalog.active_segments(),
            vec![
                ActiveGenerationSegment {
                    start: VA,
                    end: overlay_start,
                    generation: GenerationId::new(1),
                },
                ActiveGenerationSegment {
                    start: overlay_start,
                    end: overlay_end,
                    generation: GenerationId::new(2),
                },
                ActiveGenerationSegment {
                    start: overlay_end,
                    end: GuestPc::new(VA.get() + 16),
                    generation: GenerationId::new(1),
                },
            ]
        );
        assert_eq!(catalog.resolve_active(VA).unwrap().bank, BankId::new(11));
        assert_eq!(
            catalog.resolve_active(overlay_start).unwrap().bank,
            BankId::new(12)
        );
        assert_eq!(
            catalog
                .resolve_active(GuestPc::new(VA.get() + 12))
                .unwrap()
                .bank,
            BankId::new(11)
        );
    }

    #[test]
    fn overlapping_small_cpu_image_precedes_larger_rom_generation_by_digest() {
        let large_start = VA;
        let small_start = GuestPc::new(VA.get() + 4);
        let large_bytes = [
            0x24, 0x02, 0x00, 0x01, 0x24, 0x03, 0x00, 0x01, 0x24, 0x04, 0x00, 0x01,
        ];
        let small_bytes = [0x24, 0x03, 0x00, 0x02];
        let large = generation(1, 11, &large_bytes);
        let small = PrecompiledGeneration::new(
            GenerationId::new(2),
            small_start,
            GuestPc::new(small_start.get() + 4),
            small_start,
            GuestPc::new(small_start.get() + 4),
            Sha256::digest(small_bytes).into(),
            vec![PrecompiledShard::new(
                BankId::new(12),
                small_start,
                GuestPc::new(small_start.get() + 4),
            )
            .unwrap()],
        )
        .unwrap();
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog.register(large).unwrap();
        catalog.register(small).unwrap();
        let mut storage = vec![0u8; 0x200];
        let mut mem = Rdram::new(&mut storage);

        write_image_at(&mut mem, large_start, &large_bytes);
        write_image_at(&mut mem, small_start, &small_bytes);
        let selected_small = catalog.activate_for_fetch(small_start, &mem).unwrap();
        assert_eq!(selected_small.generation, GenerationId::new(2));
        assert_eq!(selected_small.entry.bank, BankId::new(12));

        write_image_at(&mut mem, large_start, &large_bytes);
        let selected_large = catalog.activate_for_fetch(small_start, &mem).unwrap();
        assert_eq!(selected_large.generation, GenerationId::new(1));
        assert_eq!(selected_large.entry.bank, BankId::new(11));
        assert_eq!(selected_large.retired, vec![GenerationId::new(2)]);
    }
