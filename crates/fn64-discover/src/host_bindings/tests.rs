    use super::*;

    mod os_pi_candidate_classifier {
        use super::*;
        use crate::cfg::{BasicBlock, BlockTerminator};
        use crate::facts::{
            executable_range_subject, function_entry_subject, BankAddr, CandidateDetector, Fact,
            FunctionEntryEvidence, ProofState,
        };

        const BASE: u32 = 0x8000_1000;
        const PI: u32 = BASE;
        const EPI: u32 = BASE + 0x100;

        fn jal(pc: u32, target: u32) -> u32 {
            assert_eq!((pc + 4) & 0xf000_0000, target & 0xf000_0000);
            0x0c00_0000 | (target >> 2 & 0x03ff_ffff)
        }

        fn pi_words(entry: u32, epi: u32) -> Vec<u32> {
            vec![
                0x27bd_ffe0,
                0xafbf_001c,
                0x0080_4021,
                0xa105_0002,
                0xa100_0003,
                0x8fa9_0030,
                0xad09_0008,
                0xad07_000c,
                0x8faa_0034,
                0xad0a_0010,
                0x8fab_0038,
                0xad0b_0004,
                0x0100_2821,
                0x3c04_8000,
                jal(entry + 14 * 4, epi),
                0x00c0_3021,
            ]
        }

        fn epi_words() -> [u32; 15] {
            [
                0x3c02_8030,
                0x8c42_1000,
                0x27bd_ffe8,
                0xafb0_0010,
                0x00a0_8021,
                0x1440_0002,
                0xafbf_0014,
                0x0800_0500,
                0x2402_ffff,
                0x14c0_0002,
                0xae04_0014,
                0x0800_0504,
                0x2402_000f,
                0x2402_0010,
                0xa602_0000,
            ]
        }

        fn fixture() -> (Vec<u32>, Cfg, FactDb) {
            let mut words = vec![0; 96];
            let pi = pi_words(PI, EPI);
            words[..pi.len()].copy_from_slice(&pi);
            words[64..79].copy_from_slice(&epi_words());
            let mut cfg = Cfg {
                bank: "resident".into(),
                word_class: BTreeMap::new(),
                blocks: vec![
                    BasicBlock {
                        start_va: PI,
                        end_va: PI + (pi.len() as u32) * 4,
                        terminator: BlockTerminator::Call {
                            target: EPI,
                            next: PI + (pi.len() as u32) * 4,
                        },
                    },
                    BasicBlock {
                        start_va: EPI,
                        end_va: EPI + 15 * 4,
                        terminator: BlockTerminator::Fallthrough { next: EPI + 15 * 4 },
                    },
                ],
                direct_calls: vec![(PI + 14 * 4, EPI)],
                tail_transfers: vec![],
                indirect_sites: vec![],
                plain_delay_entry_aliases: vec![],
                unsupported_delay_entries: vec![],
                rejected_transfer_targets: Vec::new(),
                proven_roots: vec![PI, EPI],
            };
            for pc in (PI..PI + (pi.len() as u32) * 4).step_by(4) {
                cfg.word_class.insert(pc, WordClass::ProvenCode);
            }
            for pc in (EPI..EPI + 15 * 4).step_by(4) {
                cfg.word_class.insert(pc, WordClass::ProvenCode);
            }

            let mut facts = FactDb::new();
            let end = BASE + (words.len() as u32) * 4;
            let executable = facts.insert(Fact::ExecutableRange {
                bank: "resident".into(),
                va_start: BASE,
                va_end: end,
            });
            facts
                .conclude(
                    executable_range_subject("resident", BASE, end),
                    ProofState::Proven,
                    vec![executable],
                    "test executable authority",
                )
                .unwrap();
            for pc in [PI, EPI] {
                let target = BankAddr::new("resident", pc);
                let claim = facts.insert(Fact::FunctionEntryClaim {
                    target: target.clone(),
                    detector: CandidateDetector::JalTarget,
                    evidence: FunctionEntryEvidence::DirectJal {
                        call_site: BankAddr::new("resident", pc.wrapping_sub(4)),
                    },
                    proposed_state: ProofState::Proven,
                });
                facts
                    .conclude(
                        function_entry_subject(&target),
                        ProofState::Proven,
                        vec![claim],
                        "test root authority",
                    )
                    .unwrap();
            }
            (words, cfg, facts)
        }

        #[test]
        fn classifies_only_relational_wrapper_shape() {
            let (words, cfg, facts) = fixture();
            assert_eq!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Candidate(OsPiStartDmaShapeCandidate {
                    bank: "resident".into(),
                    vram: PI,
                    os_epi_start_dma_shape_vram: EPI,
                    device_base: OsPiDeviceBasePrerequisite::UnresolvedCartHandleAndDeviceBase,
                })
            );
        }

        #[test]
        fn wrong_message_field_target_stays_open() {
            let (mut words, cfg, facts) = fixture();
            words[6] = 0xad09_000c;
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn stale_cfg_edge_cannot_classify_a_wrong_machine_call_target() {
            let (mut words, cfg, facts) = fixture();
            words[14] = jal(PI + 14 * 4, EPI + 4);
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn stale_straight_line_block_cannot_hide_an_earlier_branch() {
            let (mut words, cfg, facts) = fixture();
            words[13] = 0x1000_0001;
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn unmodeled_gpr_writer_clobbers_a_stale_direction_tag() {
            let (mut words, cfg, facts) = fixture();
            words[13] = 0x9326_0000; // lbu a2,0(t9)
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn unmodeled_width_or_aliasing_store_stays_open() {
            let (mut words, cfg, facts) = fixture();
            words[13] = 0xa720_0000; // sh zero,0(t9)
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn cfg_call_block_terminator_must_match_exactly() {
            let (words, mut cfg, facts) = fixture();
            cfg.blocks[0].terminator = BlockTerminator::Call {
                target: EPI,
                next: PI + 15 * 4,
            };
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }

        #[test]
        fn every_candidate_classifier_cap_is_loud_and_sampled() {
            let (words, cfg, facts) = fixture();
            let cases = [
                (
                    OsPiShapeLimits {
                        roots: 1,
                        ..DEFAULT_PI_SHAPE_LIMITS
                    },
                    OsPiCandidateLimitKind::Roots,
                    2,
                    1,
                    PI,
                ),
                (
                    OsPiShapeLimits {
                        calls: 0,
                        ..DEFAULT_PI_SHAPE_LIMITS
                    },
                    OsPiCandidateLimitKind::DirectCalls,
                    1,
                    0,
                    PI + 14 * 4,
                ),
                (
                    OsPiShapeLimits {
                        blocks: 1,
                        ..DEFAULT_PI_SHAPE_LIMITS
                    },
                    OsPiCandidateLimitKind::Blocks,
                    2,
                    1,
                    PI,
                ),
                (
                    OsPiShapeLimits {
                        work: 1,
                        ..DEFAULT_PI_SHAPE_LIMITS
                    },
                    OsPiCandidateLimitKind::Work,
                    16,
                    1,
                    PI,
                ),
            ];
            for (limits, kind, observed, cap, first_sample) in cases {
                assert!(matches!(
                    classify_os_pi_start_dma_candidate_with_limits(
                        "resident", &words, BASE, &cfg, &facts, limits
                    ),
                    OsPiStartDmaCandidateClassification::Open(
                        OsPiStartDmaCandidateOpenReason::LimitHit {
                            kind: actual_kind,
                            observed: actual_observed,
                            cap: actual_cap,
                            samples,
                        }
                    ) if actual_kind == kind
                        && actual_observed == observed
                        && actual_cap == cap
                        && samples.first() == Some(&first_sample)
                ));
            }
        }

        #[test]
        fn ambiguous_epi_target_stays_open() {
            let (mut words, mut cfg, mut facts) = fixture();
            let second_epi = BASE + 0x140;
            words[80..95].copy_from_slice(&epi_words());
            cfg.proven_roots.push(second_epi);
            cfg.blocks.push(BasicBlock {
                start_va: second_epi,
                end_va: second_epi + 15 * 4,
                terminator: BlockTerminator::Fallthrough {
                    next: second_epi + 15 * 4,
                },
            });
            for pc in (second_epi..second_epi + 15 * 4).step_by(4) {
                cfg.word_class.insert(pc, WordClass::ProvenCode);
            }
            let target = BankAddr::new("resident", second_epi);
            let claim = facts.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::JalTarget,
                evidence: FunctionEntryEvidence::DirectJal {
                    call_site: BankAddr::new("resident", second_epi - 4),
                },
                proposed_state: ProofState::Proven,
            });
            facts
                .conclude(
                    function_entry_subject(&target),
                    ProofState::Proven,
                    vec![claim],
                    "test second root authority",
                )
                .unwrap();
            assert_eq!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsEPiStartDmaShape {
                        candidates: vec![EPI, second_epi],
                    }
                )
            );
        }

        #[test]
        fn unreachable_image_lookalike_is_not_a_candidate() {
            let (words, mut cfg, facts) = fixture();
            cfg.proven_roots.retain(|root| *root != PI);
            cfg.blocks.retain(|block| block.start_va != PI);
            cfg.direct_calls.clear();
            assert!(matches!(
                classify_os_pi_start_dma_candidate("resident", &words, BASE, &cfg, &facts),
                OsPiStartDmaCandidateClassification::Open(
                    OsPiStartDmaCandidateOpenReason::NonUniqueOsPiStartDmaShape { candidates }
                ) if candidates.is_empty()
            ));
        }
    }

    #[test]
    fn si_device_busy_is_unique_and_invariant_sensitive() {
        let base = 0x8000_1000;
        let expected = base + 8;
        let mut words = vec![0, 0];
        words.extend([
            0x3c02_a480,
            0x3442_0018,
            0x8c42_0000,
            0x3042_0003,
            0x03e0_0008,
            0x0002_102b,
        ]);
        assert_eq!(
            discover_si_device_busy_host_binding(&words, base).unwrap(),
            HostBinding {
                symbol: HostBindingSymbol::OsSiDeviceBusy,
                vram: expected,
            }
        );

        for index in 2..8 {
            let mut broken = words.clone();
            broken[index] ^= 1;
            assert!(matches!(
                discover_si_device_busy_host_binding(&broken, base),
                Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                    symbol: HostBindingSymbol::OsSiDeviceBusy,
                    candidates,
                }) if candidates.is_empty()
            ));
        }
        words.extend_from_slice(&[
            0x3c02_a480,
            0x3442_0018,
            0x8c42_0000,
            0x3042_0003,
            0x03e0_0008,
            0x0002_102b,
        ]);
        assert!(matches!(
            discover_si_device_busy_host_binding(&words, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsSiDeviceBusy,
                candidates,
            }) if candidates.len() == 2
        ));
    }

    #[test]
    fn wm_host_status_effect_catalog_is_explicit_for_every_symbol() {
        let symbols = WM_BLOCK_RUNTIME_HOST_SYMBOLS;
        assert_eq!(symbols.len(), 15);
        for symbol in symbols {
            assert_eq!(
                symbol.current_status_effect(),
                HostCurrentStatusEffect::CBridgeRuntimeEnforcedPreservesBev
            );
            assert_eq!(
                symbol.spawned_status_effect(),
                if symbol == HostBindingSymbol::OsCreateThread {
                    HostSpawnedStatusEffect::InheritsCallerClearingFr
                } else {
                    HostSpawnedStatusEffect::None
                }
            );
        }
    }

    fn jal(pc: u32, target: u32) -> u32 {
        assert_eq!((pc + 4) & 0xf000_0000, target & 0xf000_0000);
        0x0c00_0000 | (target >> 2 & 0x03ff_ffff)
    }

    fn create_thread_fixture(pc: u32) -> [u32; 42] {
        let mut words = [0; 42];
        words[0] = 0x27bd_ffe8;
        words[2] = 0x0080_9821;
        words[7] = 0x2402_0001;
        words[8] = 0x00e0_4825;
        words[10] = 0xae62_0100;
        words[11] = 0xae63_0104;
        words[12] = 0xae62_0118;
        words[13] = 0xae62_0128;
        words[14] = 0xae65_0014;
        words[15] = 0xae60_0000;
        words[16] = 0xae60_0008;
        words[17] = 0xae66_011c;
        words[18] = 0xae68_0038;
        words[19] = 0xae69_003c;
        words[20] = 0xae64_012c;
        words[21] = 0xae60_0018;
        words[22] = 0xa662_0010;
        words[23] = 0xa660_0012;
        words[24] = 0x8fa2_002c;
        words[25] = 0xae62_0004;
        words[32] = 0x8fab_0028;
        words[34] = 0xae6a_00f0;
        words[35] = 0xae6b_00f4;
        words[39] = jal(pc + 39 * 4, pc + 0x1000);
        words[41] = 0x03e0_0008;
        words
    }

    #[test]
    fn create_thread_role_is_unique_absent_or_ambiguous() {
        let base = 0x8040_1000;
        let fixture = create_thread_fixture(base + 8);
        let mut words = vec![0, 0];
        words.extend(fixture);
        assert_eq!(
            discover_os_create_thread_host_binding(&words, base).unwrap(),
            HostBinding {
                symbol: HostBindingSymbol::OsCreateThread,
                vram: base + 8,
            }
        );

        let mut absent = words.clone();
        absent[2 + 14] ^= 1;
        assert!(matches!(
            discover_os_create_thread_host_binding(&absent, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsCreateThread,
                candidates,
            }) if candidates.is_empty()
        ));

        words.extend(fixture);
        assert!(matches!(
            discover_os_create_thread_host_binding(&words, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsCreateThread,
                candidates,
            }) if candidates.len() == 2
        ));
    }

    #[test]
    fn structural_roles_produce_addresses_without_address_signatures() {
        let base = 0x8000_0000;
        let create_index = 8usize;
        let epi_index = 48usize;
        let recv_index = 80usize;
        let send_index = 180usize;
        let create_thread_index = 240usize;
        let start_thread_index = 290usize;
        // 23 words, so it must end clear of `get_thread_pri` at 352.
        let set_event_index = 324usize;
        let get_thread_pri_index = 352usize;
        let set_thread_pri_index = 360usize;
        let sp_task_load_index = 420usize;
        let sp_task_start_go_index = 560usize;
        let sp_task_yield_index = 580usize;
        let sp_task_yielded_index = 600usize;
        let set_timer_index = 650usize;
        let loader_index = 120usize;
        // 760, not 740: `unique_match` scans with `windows(width)`, which yields
        // nothing once fewer than `width` elements remain. At the timer's 100-word
        // width the last window starts at len-100, so a 740-word array stops
        // offering windows at 640 and the routine at 650 is never presented to the
        // predicate at all -- zero candidates because nothing was ever scanned.
        // Size this against the widest WIDTH plus its start index, not the widest
        // body: 650 + 100 = 750, and the slack keeps the next widening honest.
        let mut words = vec![0u32; 760];
        words[create_index..create_index + 9].copy_from_slice(&[
            0x3c02_8123,
            0x2442_4560,
            0xac82_0000,
            0xac82_0004,
            0xac80_0008,
            0xac80_000c,
            0xac86_0010,
            0x03e0_0008,
            0xac85_0014,
        ]);
        words[epi_index..epi_index + 15].copy_from_slice(&[
            0x3c02_8123,
            0x8c42_4560,
            0x27bd_ffe8,
            0xafb0_0010,
            0x00a0_8021,
            0x1440_0003,
            0xafbf_0014,
            0x0800_1234,
            0x2402_ffff,
            0x14c0_0003,
            0xae04_0014,
            0x0800_1235,
            0x2402_000f,
            0x2402_0010,
            0xa602_0000,
        ]);
        let create = base + create_index as u32 * 4;
        let epi = base + epi_index as u32 * 4;
        let recv = base + recv_index as u32 * 4;
        words[loader_index] = jal(base + loader_index as u32 * 4, create);
        words[loader_index + 20] = jal(base + (loader_index + 20) as u32 * 4, epi);
        words[loader_index + 25] = 0x27a4_0010;
        words[loader_index + 26] = 0x27a5_002c;
        words[loader_index + 27] = jal(base + (loader_index + 27) as u32 * 4, recv);
        words[loader_index + 28] = 0x2406_0001;
        let send = base + send_index as u32 * 4;
        let send_words = &mut words[send_index..send_index + 57];
        send_words[0] = 0x27bd_ffd0;
        send_words[2] = 0x0080_8021;
        send_words[4] = 0x00a0_a821;
        send_words[6] = 0x00c0_9021;
        send_words[10] = jal(send + 40, base + 0x1000);
        send_words[12] = 0x8e03_0008;
        send_words[13] = 0x8e04_0010;
        send_words[14] = 0x0064_182a;
        send_words[34] = 0x8e03_000c;
        send_words[35] = 0x8e04_0008;
        send_words[36] = 0x8e02_0010;
        send_words[37] = 0x0064_1821;
        send_words[38] = 0x0062_001a;
        send_words[48] = 0x0000_1010;
        send_words[49] = 0x8e03_0014;
        send_words[50] = 0x0002_1080;
        send_words[51] = 0x0043_1021;
        send_words[52] = 0xac55_0000;
        send_words[53] = 0x8e02_0008;
        send_words[55] = 0x2442_0001;
        send_words[56] = 0xae02_0008;
        let create_thread = base + create_thread_index as u32 * 4;
        // Use s3 and a different legal store schedule so this fixture proves
        // the role from OSThread initialization rather than one compiler's
        // saved-register choice and instruction positions.
        words[create_thread_index..create_thread_index + 42]
            .copy_from_slice(&create_thread_fixture(create_thread));
        let start_thread = base + start_thread_index as u32 * 4;
        let start_words = &mut words[start_thread_index..start_thread_index + 15];
        start_words[0] = 0x27bd_ffe0;
        start_words[2] = 0x0080_8021;
        start_words[5] = jal(start_thread + 5 * 4, base + 0x1000);
        start_words[7] = 0x9603_0010;
        start_words[9] = 0x2402_0001;
        start_words[10] = 0x1062_0008;
        start_words[11] = 0x2402_0008;
        start_words[12] = 0x1462_001e;
        start_words[13] = 0x2402_0002;
        start_words[14] = 0xa602_0010;
        // The compilation that delegates the `OS_EVENT_PRENMI` handling rather
        // than inlining it: the arguments are captured across the interrupt
        // disable, the selector is scaled by the eight-byte entry stride, and
        // the queue and message are stored through the resulting entry.
        let set_event = base + set_event_index as u32 * 4;
        let event_words = &mut words[set_event_index..set_event_index + 23];
        event_words[0] = 0x27bd_ffe0;
        event_words[1] = 0xafb0_0010;
        event_words[2] = 0x0080_8021;
        event_words[3] = 0xafb1_0014;
        event_words[4] = 0x00a0_8821;
        event_words[5] = 0xafb2_0018;
        event_words[6] = 0xafbf_001c;
        event_words[7] = jal(set_event + 7 * 4, base + 0x1000);
        event_words[8] = 0x00c0_9021;
        event_words[9] = 0x0010_80c0;
        event_words[10] = 0x3c03_8123;
        event_words[11] = 0x2463_df30;
        event_words[12] = 0x0203_8021;
        event_words[13] = 0x0040_2021;
        event_words[14] = 0xae11_0000;
        event_words[15] = jal(set_event + 15 * 4, base + 0x1040);
        event_words[16] = 0xae12_0004;
        event_words[17] = 0x8fbf_001c;
        event_words[18] = 0x8fb2_0018;
        event_words[19] = 0x8fb1_0014;
        event_words[20] = 0x8fb0_0010;
        event_words[21] = 0x03e0_0008;
        event_words[22] = 0x27bd_0020;
        let get_thread_pri = base + get_thread_pri_index as u32 * 4;
        words[get_thread_pri_index..get_thread_pri_index + 6].copy_from_slice(&[
            0x1480_0003,
            0,
            0x3c04_8123,
            0x8c84_4560,
            0x03e0_0008,
            0x8c82_0004,
        ]);
        let set_thread_pri = base + set_thread_pri_index as u32 * 4;
        let pri_words = &mut words[set_thread_pri_index..set_thread_pri_index + 20];
        pri_words[0] = 0x27bd_ffe0;
        pri_words[2] = 0x0080_8021;
        pri_words[4] = 0x00a0_8821;
        pri_words[6] = jal(set_thread_pri + 6 * 4, base + 0x1000);
        pri_words[8] = 0x1600_0003;
        pri_words[9] = 0x0040_9021;
        pri_words[10] = 0x3c10_8123;
        pri_words[11] = 0x8e10_4560;
        pri_words[12] = 0x8e02_0004;
        pri_words[13] = 0x1051_001c;
        pri_words[15] = 0x3c02_8123;
        pri_words[16] = 0x8c42_4560;
        pri_words[17] = 0x1202_000b;
        pri_words[18] = 0xae11_0004;

        let sp_task_load = base + sp_task_load_index as u32 * 4;
        let load_words = &mut words[sp_task_load_index..sp_task_load_index + 131];
        load_words[0] = 0x27bd_ffe0;
        load_words[2] = 0x0080_8021;
        load_words[6] = 0x0220_2821;
        load_words[8] = jal(sp_task_load + 8 * 4, base + 0x1700);
        load_words[9] = 0x2406_0040;
        load_words[68] = 0x3042_0001;
        load_words[69] = 0x1040_0019;
        load_words[79] = 0x8e02_0004;
        load_words[80] = 0x2403_fffe;
        load_words[81] = 0x0043_1024;
        load_words[82] = 0xae02_0004;
        load_words[85] = 0x3042_0004;
        load_words[86] = 0x1040_0008;
        load_words[88] = 0x8e02_0038;
        load_words[94] = 0x0220_2021;
        load_words[95] = jal(sp_task_load + 95 * 4, base + 0x1710);
        load_words[96] = 0x2405_0040;
        load_words[97] = jal(sp_task_load + 97 * 4, base + 0x1800);
        load_words[98] = 0x2404_2b00;
        load_words[100] = 0x3c04_0400;
        load_words[101] = jal(sp_task_load + 101 * 4, base + 0x1810);
        load_words[102] = 0x3484_1000;
        load_words[106] = 0x2404_0001;
        load_words[107] = 0x3c05_0400;
        load_words[108] = 0x34a5_0fc0;
        load_words[110] = jal(sp_task_load + 110 * 4, base + 0x1820);
        load_words[111] = 0x2407_0040;
        load_words[114] = jal(sp_task_load + 114 * 4, base + 0x1830);
        load_words[119] = 0x8e26_0008;
        load_words[120] = 0x8e27_000c;
        load_words[121] = 0x3c05_0400;
        load_words[122] = jal(sp_task_load + 122 * 4, base + 0x1820);
        load_words[123] = 0x34a5_1000;
        load_words[129] = 0x03e0_0008;
        load_words[130] = 0x27bd_0020;

        let sp_task_start_go = base + sp_task_start_go_index as u32 * 4;
        words[sp_task_start_go_index..sp_task_start_go_index + 11].copy_from_slice(&[
            0x27bd_ffe8,
            0xafbf_0010,
            jal(sp_task_start_go + 2 * 4, base + 0x1830),
            0,
            0x1440_fffd,
            0,
            jal(sp_task_start_go + 6 * 4, base + 0x1800),
            0x2404_0125,
            0x8fbf_0010,
            0x03e0_0008,
            0x27bd_0018,
        ]);
        let sp_task_yield = base + sp_task_yield_index as u32 * 4;
        words[sp_task_yield_index..sp_task_yield_index + 7].copy_from_slice(&[
            0x27bd_ffe8,
            0xafbf_0010,
            jal(sp_task_yield + 2 * 4, base + 0x1800),
            0x2404_0400,
            0x8fbf_0010,
            0x03e0_0008,
            0x27bd_0018,
        ]);
        let sp_task_yielded = base + sp_task_yielded_index as u32 * 4;
        words[sp_task_yielded_index..sp_task_yielded_index + 19].copy_from_slice(&[
            0x27bd_ffe8,
            0xafb0_0010,
            0xafbf_0014,
            jal(sp_task_yielded + 3 * 4, base + 0x1840),
            0x0080_8021,
            0x0002_2202,
            0x3042_0080,
            0x1040_0006,
            0x3084_0001,
            0x8e02_0004,
            0x2403_fffd,
            0x0044_1025,
            0x0043_1024,
            0xae02_0004,
            0x0080_1021,
            0x8fbf_0014,
            0x8fb0_0010,
            0x03e0_0008,
            0x27bd_0018,
        ]);
        let set_timer = base + set_timer_index as u32 * 4;
        let timer_words = &mut words[set_timer_index..set_timer_index + 75];
        timer_words[0] = 0x27bd_ffe0;
        timer_words[1] = 0x8fa2_0030;
        timer_words[2] = 0x8fa3_0034;
        timer_words[4] = 0x0080_8021;
        timer_words[8] = 0xae00_0000;
        timer_words[9] = 0xae00_0004;
        timer_words[10] = 0xae06_0010;
        timer_words[11] = 0xae07_0014;
        timer_words[12] = 0xae02_0008;
        timer_words[13] = 0xae03_000c;
        timer_words[14] = 0x8fa4_0038;
        timer_words[15] = 0x8fa5_003c;
        timer_words[17] = 0xae04_0018;
        timer_words[19] = 0xae04_0018;
        timer_words[20] = 0xae02_0010;
        timer_words[21] = 0xae03_0014;
        timer_words[22] = 0xae04_0018;
        timer_words[23] = jal(set_timer + 23 * 4, base + 0x1900);
        timer_words[24] = 0xae05_001c;
        timer_words[30] = jal(set_timer + 30 * 4, base + 0x1910);
        timer_words[58] = jal(set_timer + 58 * 4, base + 0x1920);
        timer_words[59] = 0x0200_2021;
        timer_words[64] = jal(set_timer + 64 * 4, base + 0x1930);
        timer_words[66] = jal(set_timer + 66 * 4, base + 0x1940);
        timer_words[68] = 0x0000_1021;
        timer_words[73] = 0x03e0_0008;
        timer_words[74] = 0x27bd_0020;

        let si_device_busy = base + words.len() as u32 * 4;
        words.extend([
            0x3c02_a480,
            0x3442_0018,
            0x8c42_0000,
            0x3042_0003,
            0x03e0_0008,
            0x0002_102b,
        ]);
        let discovered = discover_wm_block_runtime_host_bindings(&words, base).unwrap();
        assert_eq!(
            discovered,
            vec![
                HostBinding {
                    symbol: HostBindingSymbol::OsCreateMesgQueue,
                    vram: create,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsCreateThread,
                    vram: create_thread,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsEPiStartDma,
                    vram: epi,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsGetThreadPri,
                    vram: get_thread_pri,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsRecvMesg,
                    vram: recv,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSendMesg,
                    vram: send,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSetEventMesg,
                    vram: set_event,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSiDeviceBusy,
                    vram: si_device_busy,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSetThreadPri,
                    vram: set_thread_pri,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSetTimer,
                    vram: set_timer,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSpTaskLoad,
                    vram: sp_task_load,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSpTaskStartGo,
                    vram: sp_task_start_go,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSpTaskYield,
                    vram: sp_task_yield,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsSpTaskYielded,
                    vram: sp_task_yielded,
                },
                HostBinding {
                    symbol: HostBindingSymbol::OsStartThread,
                    vram: start_thread,
                },
            ]
        );

        for offset in [
            0usize, 2, 7, 8, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 32,
            34, 35, 39, 41,
        ] {
            let mut broken = words.clone();
            if offset == 39 {
                broken[create_thread_index + offset] = 0;
            } else {
                broken[create_thread_index + offset] ^= 1;
            }
            assert!(
                matches!(
                    discover_overlay_loader_host_bindings(&broken, base),
                    Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                        symbol: HostBindingSymbol::OsCreateThread,
                        candidates,
                    }) if candidates.is_empty()
                ),
                "mutated osCreateThread invariant at word {offset}"
            );
        }

        let mut overwritten_thread_base = words.clone();
        overwritten_thread_base[create_thread_index + 9] = 0x3c13_0000;
        assert!(matches!(
            discover_overlay_loader_host_bindings(&overwritten_thread_base, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsCreateThread,
                candidates,
            }) if candidates.is_empty()
        ));

        let mut duplicate_create_thread = words.clone();
        duplicate_create_thread
            .extend_from_slice(&words[create_thread_index..create_thread_index + 42]);
        assert!(matches!(
            discover_overlay_loader_host_bindings(&duplicate_create_thread, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsCreateThread,
                candidates,
            }) if candidates.len() == 2
        ));
    }

    #[test]
    fn nwxe_rsp_task_roles_are_unique_and_invariant_sensitive_when_rom_is_available() {
        let Some(path) = std::env::var_os("FN64_DISCOVER_NWXE_ROM") else {
            eprintln!("skip: FN64_DISCOVER_NWXE_ROM unset");
            return;
        };
        let source = std::fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "reading FN64_DISCOVER_NWXE_ROM {}: {error}",
                path.to_string_lossy()
            )
        });
        let rom = crate::rom::normalize(&source).expect("normalizing NWXE corpus ROM");
        let base = rom.header.entry_point;
        let words = rom.bytes[0x1000..0x101000]
            .chunks_exact(4)
            .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
            .collect::<Vec<_>>();
        let expected = vec![
            HostBinding {
                symbol: HostBindingSymbol::OsSpTaskLoad,
                vram: 0x8003_1cc0,
            },
            HostBinding {
                symbol: HostBindingSymbol::OsSpTaskStartGo,
                vram: 0x8003_1ecc,
            },
            HostBinding {
                symbol: HostBindingSymbol::OsSpTaskYield,
                vram: 0x8003_1f00,
            },
            HostBinding {
                symbol: HostBindingSymbol::OsSpTaskYielded,
                vram: 0x8003_1f20,
            },
        ];
        let load_index = usize::try_from((expected[0].vram - base) / 4).unwrap();
        let load_words = &words[load_index..load_index + 131];
        let load_groups = [
            (
                "copy/yield prefix",
                is_addiu(load_words[0], 29, 29, imm(load_words[0]))
                    && is_move_addu(load_words[2], 16, 4)
                    && is_move_addu(load_words[6], 5, 17)
                    && jal_field(load_words[8]).is_some()
                    && is_addiu(load_words[9], 6, 0, 0x40)
                    && is_andi(load_words[68], 2, 2, 1)
                    && is_beq(load_words[69], 2, 0),
            ),
            (
                "task-header/cache prefix",
                is_lw_at(load_words[79], 2, 16, 4)
                    && is_addiu(load_words[80], 3, 0, -2)
                    && is_sw(load_words[82], 2, 16, 4)
                    && is_andi(load_words[85], 2, 2, 4)
                    && is_beq(load_words[86], 2, 0)
                    && is_lw_at(load_words[88], 2, 16, 0x38)
                    && is_move_addu(load_words[94], 4, 17)
                    && jal_field(load_words[95]).is_some()
                    && is_addiu(load_words[96], 5, 0, 0x40),
            ),
            (
                "status/task DMA",
                jal_field(load_words[97]).is_some()
                    && is_addiu(load_words[98], 4, 0, 0x2b00)
                    && is_lui(load_words[100], 4)
                    && load_words[100] as u16 == 0x0400
                    && jal_field(load_words[101]).is_some()
                    && is_addiu(load_words[106], 4, 0, 1)
                    && is_lui(load_words[107], 5)
                    && load_words[107] as u16 == 0x0400
                    && jal_field(load_words[110]).is_some()
                    && is_addiu(load_words[111], 7, 0, 0x40)
                    && jal_field(load_words[114]).is_some(),
            ),
            (
                "rspboot DMA/epilogue",
                is_lw_at(load_words[119], 6, 17, 8)
                    && is_lw_at(load_words[120], 7, 17, 12)
                    && is_lui(load_words[121], 5)
                    && load_words[121] as u16 == 0x0400
                    && jal_field(load_words[122]) == jal_field(load_words[110])
                    && load_words[123] as u16 == 0x1000
                    && is_jr_ra(load_words[129])
                    && is_addiu(load_words[130], 29, 29, -imm(load_words[0])),
            ),
        ];
        for (group, matched) in load_groups {
            assert!(matched, "NWXE osSpTaskLoad {group} did not match");
        }
        assert!(
            is_sp_task_load(load_words),
            "expected NWXE load body was not structurally recognized"
        );
        assert_eq!(
            discover_rsp_task_host_bindings(&words, base).unwrap(),
            expected
        );
        let expected_timer = HostBinding {
            symbol: HostBindingSymbol::OsSetTimer,
            vram: 0x8003_2600,
        };
        assert_eq!(
            discover_timer_host_bindings(&words, base).unwrap(),
            vec![expected_timer]
        );

        let start_index = usize::try_from((expected[1].vram - base) / 4).unwrap();
        for offset in [68usize, 96, 98, 108, 111, 122, 123] {
            let mut broken = words.clone();
            broken[load_index + offset] ^= 1;
            assert!(matches!(
                discover_rsp_task_host_bindings(&broken, base),
                Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                    symbol: HostBindingSymbol::OsSpTaskLoad,
                    candidates,
                }) if candidates.is_empty()
            ));
        }
        for offset in [2usize, 4, 6, 7] {
            let mut broken = words.clone();
            broken[start_index + offset] ^= 1;
            assert!(matches!(
                discover_rsp_task_host_bindings(&broken, base),
                Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                    symbol: HostBindingSymbol::OsSpTaskStartGo,
                    candidates,
                }) if candidates.is_empty()
            ));
        }

        let mut duplicate_load = words.clone();
        duplicate_load.extend_from_slice(&words[load_index..load_index + 131]);
        assert!(matches!(
            discover_rsp_task_host_bindings(&duplicate_load, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsSpTaskLoad,
                candidates,
            }) if candidates.len() == 2
        ));

        let mut duplicate_start = words.clone();
        duplicate_start.extend_from_slice(&words[start_index..start_index + 11]);
        assert!(matches!(
            discover_rsp_task_host_bindings(&duplicate_start, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsSpTaskStartGo,
                candidates,
            }) if candidates.len() == 2
        ));

        // Each offset below is one clause of the published contract: the frame
        // and its restore, the stack-passed countdown/queue/message loads, the
        // eight `OSTimer` field writes, and the zero-interval reload. Breaking
        // any one of them must make the routine unrecognizable. Positions that
        // merely implement the list walk are deliberately absent -- they are
        // not part of what `osSetTimer` promises, and a build that delegates
        // the walk must still be recognized.
        let timer_index = usize::try_from((expected_timer.vram - base) / 4).unwrap();
        for offset in [
            0usize, 1, 2, 8, 9, 10, 11, 12, 13, 14, 15, 20, 21, 24, 73, 74,
        ] {
            let mut broken = words.clone();
            broken[timer_index + offset] ^= 1;
            assert!(matches!(
                discover_timer_host_bindings(&broken, base),
                Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                    symbol: HostBindingSymbol::OsSetTimer,
                    candidates,
                }) if candidates.is_empty()
            ));
        }
        let mut duplicate_timer = words.clone();
        duplicate_timer.extend_from_slice(&words[timer_index..timer_index + 100]);
        assert!(matches!(
            discover_timer_host_bindings(&duplicate_timer, base),
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
                symbol: HostBindingSymbol::OsSetTimer,
                candidates,
            }) if candidates.len() == 2
        ));
    }

    #[test]
    fn running_thread_global_is_triangulated_from_relocated_priority_routines() {
        let resident_vram = 0x8040_0000;
        let running_thread_vram = 0x8040_ff20;
        let mut resident = vec![0u32; 64];
        resident[4..10].copy_from_slice(&[
            0x1480_0002,
            0,
            0x3c04_8041,
            0x8c84_ff20,
            0x03e0_0008,
            0x8c82_0004,
        ]);
        resident[20..40].copy_from_slice(&[
            0x27bd_ffe8,
            0,
            0x0080_8021,
            0,
            0x00a0_8821,
            0,
            0x0c00_1234,
            0,
            0x1600_0002,
            0x0040_9021,
            0x3c10_8041,
            0x8e10_ff20,
            0x8e02_0004,
            0x1051_0002,
            0,
            0x3c02_8042,
            0x8c42_1000,
            0x1202_0002,
            0xae11_0004,
            0,
        ]);

        assert_eq!(
            discover_guest_thread_globals(&resident, resident_vram).unwrap(),
            GuestThreadGlobals {
                running_thread_vram
            }
        );

        resident[31] = 0x8e10_1234;
        assert!(matches!(
            discover_guest_thread_globals(&resident, resident_vram),
            Err(
                HostBindingDiscoveryError::InconsistentRunningThreadGlobals {
                    get_thread_pri: 0x8040_ff20,
                    set_thread_pri: 0x8041_1234,
                }
            )
        ));
    }

mod overlapping_run_collapse {
    use super::*;

    #[test]
    fn one_routine_matching_at_adjacent_starts_is_one_candidate() {
        // A window wider than the routine also matches when it merely contains
        // it, so the same routine matches at several adjacent start offsets.
        let run = [0x8000_1000, 0x8000_1004, 0x8000_1008, 0x8000_100c];
        assert_eq!(collapse_overlapping_runs(&run), vec![0x8000_100c]);
    }

    #[test]
    fn the_reported_address_is_the_routine_entry_not_the_padding_before_it() {
        // Callers resolve `jal` targets against this address, so a run must
        // report its latest start -- the routine's own entry -- rather than an
        // earlier start that matched only by including preceding filler.
        let run = [0x8000_2000, 0x8000_2004];
        assert_eq!(collapse_overlapping_runs(&run), vec![0x8000_2004]);
    }

    #[test]
    fn genuinely_distinct_routines_stay_separate_candidates() {
        // Two real routines are never adjacent at word granularity, so
        // collapsing cannot hide real ambiguity.
        let distinct = [0x8000_1000, 0x8000_1004, 0x8000_3000];
        assert_eq!(
            collapse_overlapping_runs(&distinct),
            vec![0x8000_1004, 0x8000_3000]
        );
    }

    #[test]
    fn an_empty_candidate_list_stays_empty() {
        assert!(collapse_overlapping_runs(&[]).is_empty());
    }
}

mod create_mesg_queue_is_register_allocation_free {
    use super::*;

    /// The documented six-word queue init, with the sentinel materialized once
    /// into `$v0` -- the 1998-era build's allocation.
    fn single_register_form() -> Vec<u32> {
        vec![
            0x3c02_8005, // lui   $v0, 0x8005
            0x2442_8860, // addiu $v0, $v0, -0x77a0
            0xac82_0000, // sw    $v0, 0($a0)     mtqueue
            0xac82_0004, // sw    $v0, 4($a0)     fullqueue
            0xac80_0008, // sw    $zero, 8($a0)   validCount
            0xac80_000c, // sw    $zero, 12($a0)  first
            0xac86_0010, // sw    $a2, 16($a0)    msgCount
            0x03e0_0008, // jr    $ra
            0xac85_0014, // sw    $a1, 20($a0)    msg
            0, 0, 0,
        ]
    }

    /// The same behavior with the sentinel materialized into two registers,
    /// which is what the 1996-era build emits. Same ABI, different allocation.
    fn two_register_form() -> Vec<u32> {
        vec![
            0x3c0e_8003, // lui   $t6, 0x8003
            0x3c0f_8003, // lui   $t7, 0x8003
            0x25ce_3af0, // addiu $t6, $t6, 0x3af0
            0x25ef_3af0, // addiu $t7, $t7, 0x3af0
            0xac8e_0000, // sw    $t6, 0($a0)
            0xac8f_0004, // sw    $t7, 4($a0)
            0xac80_0008, // sw    $zero, 8($a0)
            0xac80_000c, // sw    $zero, 12($a0)
            0xac86_0010, // sw    $a2, 16($a0)
            0x03e0_0008, // jr    $ra
            0xac85_0014, // sw    $a1, 20($a0)
            0,
        ]
    }

    #[test]
    fn both_compilations_of_the_same_routine_are_recognized() {
        assert!(is_create_mesg_queue(&single_register_form()));
        assert!(is_create_mesg_queue(&two_register_form()));
    }

    #[test]
    fn queue_heads_taking_different_sentinels_are_rejected() {
        // Requiring both heads to receive the SAME computed address is what
        // keeps this a queue-initializer predicate rather than "any six
        // stores through $a0".
        let mut words = two_register_form();
        words[1] = 0x3c0f_8004; // $t7 now denotes a different address
        assert!(!is_create_mesg_queue(&words));
    }

    #[test]
    fn a_missing_documented_field_is_rejected() {
        let mut words = single_register_form();
        words[6] = 0x0000_0000; // drop the msgCount store
        assert!(!is_create_mesg_queue(&words));
    }

    #[test]
    fn storing_the_wrong_argument_to_a_field_is_rejected() {
        let mut words = single_register_form();
        words[8] = 0xac86_0014; // msg <- $a2 instead of the o32 second argument
        assert!(!is_create_mesg_queue(&words));
    }
}
