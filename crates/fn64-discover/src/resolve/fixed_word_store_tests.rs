    use super::*;
    use crate::cfg::build_cfg;

    const START: u32 = 0x8000_1000;
    const SOURCE: u32 = 0x8000_1100;
    const DESTINATION: u32 = 0x8000_0180;
    const WORD: u32 = 0x27bd_ffe0;
    const NOP: u32 = 0;
    const JR_RA: u32 = 0x03e0_0008;

    fn i(op: u32, rs: u8, rt: u8, immediate: i16) -> u32 {
        (op << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | immediate as u16 as u32
    }

    fn r(rs: u8, rt: u8, rd: u8, funct: u32) -> u32 {
        ((rs as u32) << 21) | ((rt as u32) << 16) | ((rd as u32) << 11) | funct
    }

    fn j(op: u32, target: u32) -> u32 {
        (op << 26) | ((target >> 2) & 0x03ff_ffff)
    }

    fn image(words: &[u32], sources: &[(u32, u32)]) -> Vec<u8> {
        let mut bytes = vec![0; 0x300];
        for (index, word) in words.iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        for &(address, word) in sources {
            let offset = (address - START) as usize;
            bytes[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
        }
        bytes
    }

    fn analyze(
        bytes: &[u8],
        watched: &[u32],
        sources: &[AdmittedWordSource],
    ) -> FixedWordStoreReport {
        let cfg = build_cfg("stores", bytes, START, &[START]);
        derive_fixed_word_stores(&cfg, bytes, START, watched, sources).unwrap()
    }

    fn canonical_copy() -> Vec<u32> {
        vec![
            i(0x0f, 0, 8, 0x8000u16 as i16),
            i(0x09, 8, 8, 0x1100),
            i(0x23, 8, 9, 0),
            i(0x0f, 0, 10, 0x8000u16 as i16),
            i(0x2b, 10, 9, 0x0180),
            JR_RA,
            NOP,
        ]
    }

    #[test]
    fn derives_conditional_unchanged_rom_word_copy() {
        let bytes = image(&canonical_copy(), &[(SOURCE, WORD)]);
        let report = analyze(
            &bytes,
            &[DESTINATION],
            &[AdmittedWordSource {
                address: SOURCE,
                value: WORD,
            }],
        );
        assert_eq!(report.open, Vec::new());
        assert_eq!(
            report.conditional,
            vec![ConditionalFixedWordStore {
                site_pc: START + 16,
                destination: DESTINATION,
                value: WORD,
                source: AdmittedWordSource {
                    address: SOURCE,
                    value: WORD,
                },
            }]
        );
    }

    #[test]
    fn arithmetic_clears_identity_copy_provenance() {
        let mut words = canonical_copy();
        words.insert(3, i(0x09, 9, 9, 1));
        let bytes = image(&words, &[(SOURCE, WORD)]);
        let report = analyze(
            &bytes,
            &[DESTINATION],
            &[AdmittedWordSource {
                address: SOURCE,
                value: WORD,
            }],
        );
        assert!(report.conditional.is_empty());
        assert_eq!(report.open.len(), 1);
        assert!(report.open[0]
            .blockers
            .contains(&FixedWordStoreBlocker::ValueNotUnchangedStaticLoad));
    }

    #[test]
    fn unknown_destination_remains_open_as_a_possible_alias() {
        let words = vec![
            i(0x0f, 0, 8, 0x8000u16 as i16),
            i(0x09, 8, 8, 0x1100),
            i(0x23, 8, 9, 0),
            i(0x2b, 4, 9, 0),
            JR_RA,
            NOP,
        ];
        let bytes = image(&words, &[(SOURCE, WORD)]);
        let report = analyze(
            &bytes,
            &[DESTINATION],
            &[AdmittedWordSource {
                address: SOURCE,
                value: WORD,
            }],
        );
        assert!(report.conditional.is_empty());
        assert!(report.open[0]
            .blockers
            .contains(&FixedWordStoreBlocker::AddressOpen));
    }

    #[test]
    fn call_clobber_between_load_and_store_remains_open() {
        let callee = START + 0x30;
        let mut words = vec![
            i(0x0f, 0, 8, 0x8000u16 as i16),
            i(0x09, 8, 8, 0x1100),
            i(0x23, 8, 9, 0),
            i(0x0f, 0, 10, 0x8000u16 as i16),
            j(0x03, callee),
            NOP,
            i(0x2b, 10, 9, 0x0180),
            JR_RA,
            NOP,
        ];
        words.resize(0x30 / 4, NOP);
        words.extend([JR_RA, NOP]);
        let bytes = image(&words, &[(SOURCE, WORD)]);
        let report = analyze(
            &bytes,
            &[DESTINATION],
            &[AdmittedWordSource {
                address: SOURCE,
                value: WORD,
            }],
        );
        assert!(report.conditional.is_empty());
        assert!(report.open[0]
            .blockers
            .contains(&FixedWordStoreBlocker::ValueOpen));
    }

    #[test]
    fn identical_join_observations_close_but_conflicting_provenance_does_not() {
        // Both branch arms preserve the same loaded source word before the
        // shared store.  The observer's BTreeSet meet deduplicates identical
        // normalized observations rather than calling convergence disagreement.
        let words = vec![
            i(0x0f, 0, 8, 0x8000u16 as i16),
            i(0x09, 8, 8, 0x1100),
            i(0x23, 8, 9, 0),
            i(0x0f, 0, 10, 0x8000u16 as i16),
            i(0x04, 4, 0, 3),
            NOP,
            j(0x02, START + 0x28),
            NOP,
            NOP,
            NOP,
            i(0x2b, 10, 9, 0x0180),
            JR_RA,
            NOP,
        ];
        let bytes = image(&words, &[(SOURCE, WORD)]);
        let admitted = [AdmittedWordSource {
            address: SOURCE,
            value: WORD,
        }];
        let report = analyze(&bytes, &[DESTINATION], &admitted);
        assert_eq!(report.conditional.len(), 1);
        assert!(report.open.is_empty());

        // Replace one arm's nop with a load of a different admitted word.  The
        // join must retain both numerical values and cannot choose one source.
        let mut changed = words;
        changed[8] = i(0x23, 8, 9, 4);
        let other_word = WORD ^ 1;
        let bytes = image(&changed, &[(SOURCE, WORD), (SOURCE + 4, other_word)]);
        let report = analyze(
            &bytes,
            &[DESTINATION],
            &[
                admitted[0],
                AdmittedWordSource {
                    address: SOURCE + 4,
                    value: other_word,
                },
            ],
        );
        assert!(report.conditional.is_empty());
        assert!(matches!(
            report.open[0].blockers.as_slice(),
            [
                FixedWordStoreBlocker::ValueSetAmbiguous { .. },
                FixedWordStoreBlocker::ValueNotUnchangedStaticLoad,
            ]
        ));
    }

    #[test]
    fn four_words_to_four_vector_bases_are_deterministic() {
        let source_words = [0x3c1a_8000, 0x275a_0000, 0x0340_0008, 0x0000_0000];
        let vector_bases = [0x8000_0000, 0x8000_0080, 0x8000_0100, 0x8000_0180];
        let mut words = vec![
            i(0x0f, 0, 16, 0x8000u16 as i16),
            i(0x09, 16, 16, 0x1100),
            i(0x0f, 0, 17, 0x8000u16 as i16),
        ];
        for (index, _) in source_words.iter().enumerate() {
            words.push(i(0x23, 16, 8, (index * 4) as i16));
            for base in vector_bases {
                words.push(i(0x2b, 17, 8, (base & 0xffff) as i16));
            }
        }
        words.extend([JR_RA, NOP]);
        let sources: Vec<_> = source_words
            .iter()
            .enumerate()
            .map(|(index, &value)| AdmittedWordSource {
                address: SOURCE + (index as u32) * 4,
                value,
            })
            .collect();
        let source_pairs: Vec<_> = sources
            .iter()
            .map(|source| (source.address, source.value))
            .collect();
        let watched: Vec<_> = vector_bases
            .iter()
            .flat_map(|base| (0..4).map(move |index| base + index * 4))
            .collect();
        let bytes = image(&words, &source_pairs);
        let first = analyze(&bytes, &watched, &sources);
        let second = analyze(&bytes, &watched, &sources);
        assert_eq!(first, second);
        assert!(first.open.is_empty());
        assert_eq!(first.conditional.len(), 16);
    }

    #[test]
    fn widened_loop_visit_cannot_leave_an_exact_result() {
        let words = vec![
            i(0x0f, 0, 8, 0x8000u16 as i16),
            i(0x09, 8, 8, 0x1100),
            i(0x23, 8, 9, 0),
            i(0x0f, 0, 10, 0x8000u16 as i16),
            i(0x09, 0, 11, 0),
            i(0x09, 11, 11, 1),
            i(0x04, 0, 0, -2),
            i(0x2b, 10, 9, 0x0180),
            JR_RA,
            NOP,
        ];
        let bytes = image(&words, &[(SOURCE, WORD)]);
        let report = analyze(
            &bytes,
            &[DESTINATION],
            &[AdmittedWordSource {
                address: SOURCE,
                value: WORD,
            }],
        );
        assert!(report.conditional.is_empty());
        assert!(report.open[0]
            .blockers
            .contains(&FixedWordStoreBlocker::RevisitWidened));
    }

    #[test]
    fn validates_word_aligned_and_unambiguous_inputs() {
        let bytes = image(&canonical_copy(), &[(SOURCE, WORD)]);
        let cfg = build_cfg("stores", &bytes, START, &[START]);
        assert_eq!(
            derive_fixed_word_stores(&cfg, &bytes, START, &[DESTINATION + 1], &[]),
            Err(FixedWordStoreInputError::UnalignedWatchedDestination {
                address: DESTINATION + 1,
            })
        );
        assert!(matches!(
            derive_fixed_word_stores(
                &cfg,
                &bytes,
                START,
                &[DESTINATION],
                &[
                    AdmittedWordSource {
                        address: SOURCE,
                        value: WORD,
                    },
                    AdmittedWordSource {
                        address: SOURCE,
                        value: WORD ^ 1,
                    },
                ],
            ),
            Err(FixedWordStoreInputError::ConflictingSourceValues { .. })
        ));
    }

    #[test]
    fn register_move_preserves_identity_but_arithmetic_does_not() {
        let mut words = canonical_copy();
        words.insert(3, r(9, 0, 9, 0x21)); // addu t1,t1,zero
        let bytes = image(&words, &[(SOURCE, WORD)]);
        let report = analyze(
            &bytes,
            &[DESTINATION],
            &[AdmittedWordSource {
                address: SOURCE,
                value: WORD,
            }],
        );
        assert_eq!(report.open, Vec::new());
        assert_eq!(report.conditional.len(), 1);
        assert_eq!(report.conditional[0].source.address, SOURCE);
        assert_eq!(report.conditional[0].value, WORD);
    }
