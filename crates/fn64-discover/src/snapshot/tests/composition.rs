use super::*;

    #[test]
    fn transitive_cross_bank_plain_delay_slot_root_is_an_alias() {
        const Z_BASE: u32 = 0x8000_2000;
        const Z_ROM: u32 = 0x3000;
        let delay_slot = Z_BASE + 4;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y = asm(&[jal_to(delay_slot), NOP, JR_RA, NOP]);
        let z = asm(&[
            0x1000_0003, // beq $zero,$zero,Z_BASE+0x10
            0x2404_0007, // plain shared delay entry
            0x0100_0008, // jr $t0: open, denying an exact owner
            NOP,
            JR_RA,
            NOP,
        ]);
        let mut raw = vec![0u8; Z_ROM as usize + z.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y.len()].copy_from_slice(&y);
        raw[Z_ROM as usize..Z_ROM as usize + z.len()].copy_from_slice(&z);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "middle", Y_ROM, Y_BASE, y.len() as u32, &[]);
        prove_bank(&mut facts, "leaf", Z_ROM, Z_BASE, z.len() as u32, &[Z_BASE]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "middle",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "leaf",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[Z_BASE],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        let cfg = &snapshots[2].banks[0].authority_closure.cfg;
        assert!(cfg
            .plain_delay_entry_aliases
            .iter()
            .any(|alias| { alias.entry_va == delay_slot && alias.control_pc == Z_BASE }));
        assert!(!cfg.blocks.iter().any(|block| block.start_va == delay_slot));
        assert!(cfg.blocks.iter().any(|block| {
            block.start_va == Z_BASE
                && block.end_va == Z_BASE + 8
                && matches!(block.terminator, crate::cfg::BlockTerminator::Branch { .. })
        }));
        let classified = crate::closure::classified_destinations(&snapshots);
        assert!(classified.iter().any(|destination| {
            destination.va == delay_slot
                && destination.reason == crate::closure::DestinationReason::InProvenBlock
                && destination.class() == crate::closure::DestinationClass::BlockAot
        }));
    }

    #[test]
    fn traversal_hint_cannot_seed_cross_bank_semantic_authority() {
        const CREATE_INDEX: usize = 32;
        const THREAD_INDEX: usize = 96;
        let create = Y_BASE + CREATE_INDEX as u32 * 4;
        let thread = Y_BASE + THREAD_INDEX as u32 * 4;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let mut y_words = vec![NOP; 112];
        write_create_thread_call(&mut y_words, 0, create, thread);
        y_words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        y_words[THREAD_INDEX] = JR_RA;
        y_words[THREAD_INDEX + 1] = NOP;
        let y = asm(&y_words);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(&mut facts, "caller", X_ROM, X_BASE, x.len() as u32, &[]);
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(snapshots[1].banks[0]
            .closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == Y_BASE));
        assert!(snapshots[1].banks[0]
            .authority_closure
            .cfg
            .blocks
            .is_empty());
        assert!(!snapshots[1].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| {
                matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == thread)
            }));
        assert!(!snapshots[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::FunctionEntryClaim {
                detector: CandidateDetector::SemanticCallableArgument,
                ..
            }
        )));
    }

    #[test]
    fn overlapping_cross_bank_targets_do_not_seed_semantic_authority() {
        const Y2_ROM: u32 = 0x3000;
        const CREATE_INDEX: usize = 32;
        const THREAD_INDEX: usize = 96;
        let create = Y_BASE + CREATE_INDEX as u32 * 4;
        let thread = Y_BASE + THREAD_INDEX as u32 * 4;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let mut y_words = vec![NOP; 112];
        write_create_thread_call(&mut y_words, 0, create, thread);
        y_words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        y_words[THREAD_INDEX] = JR_RA;
        y_words[THREAD_INDEX + 1] = NOP;
        let y = asm(&y_words);
        let mut raw = vec![0u8; Y2_ROM as usize + y.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y.len()].copy_from_slice(&y);
        raw[Y2_ROM as usize..Y2_ROM as usize + y.len()].copy_from_slice(&y);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee_a", Y_ROM, Y_BASE, y.len() as u32, &[]);
        prove_bank(&mut facts, "callee_b", Y2_ROM, Y_BASE, y.len() as u32, &[]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "callee_a",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[Y_BASE],
            },
            MaterializedBankInput {
                bank: "callee_b",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[Y_BASE],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        for snapshot in &snapshots[1..] {
            assert!(!snapshot.banks[0]
                .authority_closure
                .cfg
                .proven_roots
                .contains(&Y_BASE));
            assert!(!snapshot.facts.facts().iter().any(|fact| matches!(
                fact,
                Fact::DirectCall { source, target }
                    if source.bank == "caller" && target.bank == snapshot.banks[0].input.bank
            )));
            assert!(!snapshot.banks[0]
                .owner_proof
                .assessments
                .iter()
                .any(|assessment| {
                    matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == thread)
                }));
        }
    }

    #[test]
    fn overlapping_target_banks_receive_no_cross_bank_authority() {
        const Y2_ROM: u32 = 0x3000;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y1 = asm(&[JR_RA, NOP]);
        let y2 = y1.clone();
        let mut raw = vec![0u8; Y2_ROM as usize + y2.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y1.len()].copy_from_slice(&y1);
        raw[Y2_ROM as usize..Y2_ROM as usize + y2.len()].copy_from_slice(&y2);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee_a", Y_ROM, Y_BASE, y1.len() as u32, &[]);
        prove_bank(&mut facts, "callee_b", Y2_ROM, Y_BASE, y2.len() as u32, &[]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "callee_a",
                va_start: Y_BASE,
                bytes: &y1,
                seed_roots: &[Y_BASE],
            },
            MaterializedBankInput {
                bank: "callee_b",
                va_start: Y_BASE,
                bytes: &y2,
                seed_roots: &[Y_BASE],
            },
        ];
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        for snapshot in &snapshots[1..] {
            assert!(!snapshot.banks[0].owner_proof.assessments.iter().any(
                |assessment| matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == Y_BASE)
            ));
            assert!(!snapshot.facts.facts().iter().any(|fact| matches!(
                fact,
                Fact::DirectCall { source, target }
                    if source.bank == "caller" && target.bank == snapshot.banks[0].input.bank
            )));
        }
    }

    #[test]
    fn late_overlapping_roots_do_not_propagate_authority() {
        const Y2_ROM: u32 = 0x3000;
        const Z_BASE: u32 = 0x8000_2000;
        const Z_ROM: u32 = 0x4000;
        const W_BASE: u32 = 0x8000_3000;
        const W1_ROM: u32 = 0x5000;
        const W2_ROM: u32 = 0x6000;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y = asm(&[
            jal_to(Z_BASE),
            NOP,
            0x3c19_0000 | (W_BASE >> 16),
            0x3739_0000 | (W_BASE & 0xffff),
            (25u32 << 21) | (31u32 << 11) | 0x09,
            NOP,
            JR_RA,
            NOP,
        ]);
        let z = asm(&[JR_RA, NOP]);
        let w = asm(&[JR_RA, NOP]);
        let mut raw = vec![0u8; W2_ROM as usize + w.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        for (rom_start, bytes) in [
            (X_ROM, x.as_slice()),
            (Y_ROM, y.as_slice()),
            (Y2_ROM, y.as_slice()),
            (Z_ROM, z.as_slice()),
            (W1_ROM, w.as_slice()),
            (W2_ROM, w.as_slice()),
        ] {
            raw[rom_start as usize..rom_start as usize + bytes.len()].copy_from_slice(bytes);
        }
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "middle_a", Y_ROM, Y_BASE, y.len() as u32, &[]);
        prove_bank(&mut facts, "middle_b", Y2_ROM, Y_BASE, y.len() as u32, &[]);
        prove_bank(&mut facts, "unique", Z_ROM, Z_BASE, z.len() as u32, &[]);
        prove_bank(&mut facts, "overlap_a", W1_ROM, W_BASE, w.len() as u32, &[]);
        prove_bank(&mut facts, "overlap_b", W2_ROM, W_BASE, w.len() as u32, &[]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "middle_a",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "middle_b",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "unique",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "overlap_a",
                va_start: W_BASE,
                bytes: &w,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "overlap_b",
                va_start: W_BASE,
                bytes: &w,
                seed_roots: &[],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        let unique = &snapshots[3];
        assert!(!unique.banks[0].owner_proof.assessments.iter().any(
            |assessment| matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == Z_BASE)
        ));
        for source_bank in ["middle_a", "middle_b"] {
            assert!(!unique.facts.facts().iter().any(|fact| matches!(
                fact,
                Fact::DirectCall { source, target }
                    if source.bank == source_bank && target.bank == "unique" && target.pc == Z_BASE
            )));
        }
        for overlap in &snapshots[4..] {
            assert!(!overlap.banks[0].owner_proof.assessments.iter().any(
                |assessment| matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == W_BASE)
            ));
            for source_bank in ["middle_a", "middle_b"] {
                assert!(!overlap.facts.facts().iter().any(|fact| matches!(
                    fact,
                    Fact::ResolvedCall { source, target }
                        if source.bank == source_bank
                            && target.bank == overlap.banks[0].input.bank
                            && target.pc == W_BASE
                )));
            }
        }

        let reversed = [
            MaterializedBankInput {
                bank: "overlap_b",
                va_start: W_BASE,
                bytes: &w,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "overlap_a",
                va_start: W_BASE,
                bytes: &w,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "unique",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "middle_b",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "middle_a",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
        ];
        let reversed_snapshots = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &reversed,
            MultiBankCompositionLimits {
                max_cross_bank_authority_records: 0,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap();
        let cross_call_facts = |snapshots: &[ProgramSnapshotV1]| {
            snapshots
                .iter()
                .flat_map(|snapshot| snapshot.facts.facts())
                .filter(|fact| matches!(fact, Fact::DirectCall { .. } | Fact::ResolvedCall { .. }))
                .map(|fact| serde_json::to_string(fact).unwrap())
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(
            cross_call_facts(&snapshots),
            cross_call_facts(&reversed_snapshots)
        );
        assert!(cross_call_facts(&snapshots).is_empty());
    }

    #[test]
    fn overlapping_cross_bank_delay_slot_target_is_not_authorized() {
        const Y2_ROM: u32 = 0x3000;
        let delay_slot = Y_BASE + 4;
        let x = asm(&[jal_to(delay_slot), NOP, JR_RA, NOP]);
        let y = asm(&[JR_RA, NOP, JR_RA, NOP]);
        let mut raw = vec![0u8; Y2_ROM as usize + y.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y.len()].copy_from_slice(&y);
        raw[Y2_ROM as usize..Y2_ROM as usize + y.len()].copy_from_slice(&y);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(
            &mut facts,
            "callee_a",
            Y_ROM,
            Y_BASE,
            y.len() as u32,
            &[Y_BASE],
        );
        prove_bank(
            &mut facts,
            "callee_b",
            Y2_ROM,
            Y_BASE,
            y.len() as u32,
            &[Y_BASE],
        );
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "callee_a",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[Y_BASE],
            },
            MaterializedBankInput {
                bank: "callee_b",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[Y_BASE],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        for snapshot in &snapshots[1..] {
            assert!(!snapshot.banks[0]
                .authority_closure
                .cfg
                .proven_roots
                .contains(&delay_slot));
            assert!(!snapshot.facts.facts().iter().any(|fact| matches!(
                fact,
                Fact::DirectCall { source, target }
                    if source.bank == "caller"
                        && target.bank == snapshot.banks[0].input.bank
                        && target.pc == delay_slot
            )));
        }
    }

    fn locally_contained_overlap_fixture(
        source: &[u8],
        sibling_va: u32,
        sibling: &[u8],
    ) -> (NormalizedRom, FactDb) {
        let mut raw = vec![0u8; Y_ROM as usize + sibling.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + source.len()].copy_from_slice(source);
        raw[Y_ROM as usize..Y_ROM as usize + sibling.len()].copy_from_slice(sibling);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "source",
            X_ROM,
            X_BASE,
            source.len() as u32,
            &[X_BASE],
        );
        prove_bank(
            &mut facts,
            "sibling",
            Y_ROM,
            sibling_va,
            sibling.len() as u32,
            &[],
        );
        (rom, facts)
    }

    fn assert_local_target_did_not_authorize_sibling(snapshots: &[ProgramSnapshotV1], target: u32) {
        assert!(snapshots[1].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                OwnerAssessment::Candidate { frontier }
                    | OwnerAssessment::Ambiguous { frontier }
                    if frontier.entry.pc == target
                        && frontier.blockers.contains(&OwnerBlocker::EntryNotAuthoritative)
            )));
        assert!(!snapshots[1].facts.facts().iter().any(|fact| match fact {
            Fact::DirectCall {
                source,
                target: edge_target,
            }
            | Fact::ResolvedCall {
                source,
                target: edge_target,
            } => {
                source.bank == "source" && edge_target.bank == "sibling"
            }
            _ => false,
        }));
    }

    #[test]
    fn locally_contained_direct_call_does_not_authorize_overlapping_sibling() {
        let target = X_BASE + 8;
        let source = asm(&[jal_to(target), NOP, JR_RA, NOP]);
        let sibling = asm(&[JR_RA, NOP]);
        let (rom, facts) = locally_contained_overlap_fixture(&source, target, &sibling);
        let inputs = [
            MaterializedBankInput {
                bank: "source",
                va_start: X_BASE,
                bytes: &source,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "sibling",
                va_start: target,
                bytes: &sibling,
                seed_roots: &[target],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert_local_target_did_not_authorize_sibling(&snapshots, target);
    }

    #[test]
    fn locally_contained_resolved_call_does_not_authorize_overlapping_sibling() {
        let target = X_BASE + 0x10;
        let source = asm(&[
            0x3c19_0000 | (target >> 16),
            0x3739_0000 | (target & 0xffff),
            (25u32 << 21) | (31u32 << 11) | 0x09,
            NOP,
            JR_RA,
            NOP,
        ]);
        let sibling = asm(&[JR_RA, NOP]);
        let (rom, facts) = locally_contained_overlap_fixture(&source, target, &sibling);
        let inputs = [
            MaterializedBankInput {
                bank: "source",
                va_start: X_BASE,
                bytes: &source,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "sibling",
                va_start: target,
                bytes: &sibling,
                seed_roots: &[target],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert_local_target_did_not_authorize_sibling(&snapshots, target);
    }

    #[test]
    fn multi_bank_limits_fail_before_unbounded_composition() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let inputs = [MaterializedBankInput {
            bank: "bank",
            va_start: BASE,
            bytes: &bytes,
            seed_roots: &[BASE],
        }];

        let error = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &inputs,
            MultiBankCompositionLimits {
                max_projected_fact_rows: 0,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            SnapshotError::ProjectedFactRowsLimitExceeded { .. }
        ));

        let error = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &inputs,
            MultiBankCompositionLimits {
                max_projected_fact_bytes: 0,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            SnapshotError::ProjectedFactBytesLimitExceeded { .. }
        ));

        let error = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &inputs,
            MultiBankCompositionLimits {
                max_aggregate_materialized_bytes: (bytes.len() - 1) as u64,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            SnapshotError::AggregateMaterializedBytesLimitExceeded { .. }
        ));
    }

    #[test]
    fn projected_limits_do_not_multiply_irrelevant_bank_facts() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        for index in 0..100u32 {
            facts.insert(Fact::BlockStart {
                bank: format!("irrelevant_{index}"),
                pc: 0x8100_0000 + index * 4,
            });
        }
        let inputs = [MaterializedBankInput {
            bank: "bank",
            va_start: BASE,
            bytes: &bytes,
            seed_roots: &[BASE],
        }];

        let snapshots = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &inputs,
            MultiBankCompositionLimits {
                max_projected_fact_rows: 3,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert!(!snapshots[0].facts.facts().iter().any(|fact| {
            matches!(fact, Fact::BlockStart { bank, .. } if bank.starts_with("irrelevant_"))
        }));
    }

    #[test]
    #[ignore = "private regression: requires the local OoT ROM and runs the full closure gate"]
    fn oot_projection_runs_full_gate_below_default_limits() {
        let rom = std::env::var("FN64_DISCOVER_OOT_ROM")
            .expect("set FN64_DISCOVER_OOT_ROM to the private OoT ROM path");
        assert!(
            std::path::Path::new(&rom).is_file(),
            "missing private OoT ROM"
        );
        let output = std::process::Command::new(env!("CARGO"))
            .args([
                "run",
                "--quiet",
                "-p",
                "fn64-discover",
                "--bin",
                "gate_closure",
            ])
            .env("FN64_DISCOVER_OOT_ROM", &rom)
            .env(REPORT_PROJECTION_STATS_ENV, "1")
            .env_remove("FN64_DISCOVER_NW4E_ROM")
            .env_remove("FN64_DISCOVER_NWXE_ROM")
            .output()
            .expect("run gate_closure");
        assert!(
            output.status.success(),
            "gate failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("OoT"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stats = stderr
            .lines()
            .find(|line| line.starts_with("fn64 projection-stats "))
            .expect("projection stats receipt");
        eprintln!("{stats}");
    }

    #[test]
    fn duplicate_input_bank_names_fail_closed() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let inputs = [
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
        ];

        assert_eq!(
            compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap_err(),
            SnapshotError::DuplicateBankName {
                bank: "bank".into(),
            }
        );
    }

    #[test]
    fn cross_bank_authority_limit_counts_unique_records() {
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);
        let error = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]),
            MultiBankCompositionLimits {
                max_cross_bank_authority_records: 0,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            SnapshotError::CrossBankAuthorityRecordsLimitExceeded {
                records: 1,
                limit: 0,
            }
        );
    }

    #[test]
    fn interval_index_preserves_overlaps_and_prunes_disjoint_banks() {
        let count = 16_384usize;
        let intervals = (0..count)
            .map(|input_index| {
                let va_start = 0x8000_0000 + (input_index as u32) * 16;
                BankInterval {
                    input_index,
                    bank: format!("bank_{input_index}"),
                    va_start,
                    va_end: va_start + 8,
                }
            })
            .collect();
        let index = BankIntervalIndex::from_intervals(intervals);
        let mut probes = 0;
        for input_index in 0..count {
            let target = 0x8000_0000 + (input_index as u32) * 16;
            let (matches, query_probes) =
                index.matching_other_banks_with_probe_count("source", target);
            assert_eq!(matches, vec![input_index]);
            probes += query_probes;
        }
        assert_eq!(
            probes, count,
            "disjoint point queries must not rescan the catalog"
        );

        let overlapping = BankIntervalIndex::from_intervals(vec![
            BankInterval {
                input_index: 2,
                bank: "callee_b".into(),
                va_start: Y_BASE,
                va_end: Y_BASE + 16,
            },
            BankInterval {
                input_index: 1,
                bank: "callee_a".into(),
                va_start: Y_BASE,
                va_end: Y_BASE + 8,
            },
            BankInterval {
                input_index: 0,
                bank: "source".into(),
                va_start: Y_BASE,
                va_end: Y_BASE + 4,
            },
        ]);
        assert_eq!(
            overlapping.matching_other_banks("source", Y_BASE),
            vec![1, 2]
        );
        assert_eq!(
            overlapping.matching_other_banks("source", Y_BASE + 8),
            vec![2]
        );
    }

    #[test]
    fn proven_cross_bank_jal_splits_an_interior_callee_entry() {
        let interior = Y_BASE + 4;
        let x = asm(&[jal_to(interior), NOP, JR_RA, NOP]);
        let y = asm(&[NOP, JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        let bank = &snapshots[1].banks[0];
        let prefix = bank
            .partition
            .owners
            .iter()
            .find(|owner| owner.root_va == Y_BASE)
            .expect("the enclosing owner keeps its prefix");
        assert_eq!(prefix.extent_end, interior);
        let split = bank
            .partition
            .owners
            .iter()
            .find(|owner| owner.root_va == interior)
            .expect("the authorized interior entry gets its own owner");
        assert_eq!(split.extent_end, Y_BASE + 12);
        assert!(bank.owner_proof.assessments.iter().any(|assessment| {
            matches!(
                assessment,
                OwnerAssessment::Proven { owner }
                    if owner.entry.pc == interior && owner.va_end == Y_BASE + 12
            )
        }));
    }

    #[test]
    fn single_bank_composition_leaves_callee_unauthorized() {
        // The same callee composed ALONE (no caller sibling) has no in-bank
        // authority, so its entry stays non-authoritative. This is the control
        // that the win above comes from the cross-bank edge, not the geometry.
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&asm(&[JR_RA, NOP]), &y);
        let mut facts = FactDb::new();
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let callee_only = [MaterializedBankInput {
            bank: "callee",
            va_start: Y_BASE,
            bytes: &y,
            seed_roots: &[Y_BASE],
        }];
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &callee_only).unwrap();
        // snapshots[0] is the callee here; the helper indexes [1], so assert
        // directly on assessment [0].
        assert!(snapshots[0].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| assessment.entry().pc == Y_BASE
                && matches!(
                    assessment,
                    OwnerAssessment::Candidate { frontier }
                        | OwnerAssessment::Ambiguous { frontier }
                        if frontier.blockers.contains(&OwnerBlocker::EntryNotAuthoritative)
                )));
    }

    #[test]
    fn cross_bank_jal_from_unproven_code_confers_no_authority() {
        // The `jal` word in X is NOT reached as proven code: X is seeded with a
        // root that never reaches the call site, so the source word stays
        // unproven. A jal-shaped word in unproven bytes proves nothing.
        //
        // X layout: [0x00] JR_RA / NOP  (the only reached, authoritative fn)
        //           [0x08] jal Y / NOP  (unreached — never proven code)
        let x = asm(&[JR_RA, NOP, jal_to(Y_BASE), NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        // Seed X only at X_BASE (the returning fn); the jal at X_BASE+8 is never
        // traversed, so its word_class is not ProvenCode.
        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(
            !callee_owner_is_proven(&snapshots),
            "an unproven-source jal must not authorize the callee"
        );
        assert!(callee_entry_not_authoritative(&snapshots));
        // No cross-bank DirectCall fact was minted from unproven source bytes.
        assert!(!snapshots[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "caller" && target.bank == "callee"
        )));
    }

    #[test]
    fn cross_bank_jal_from_candidate_traversal_confers_no_authority() {
        let candidate_caller = X_BASE + 8;
        let x = asm(&[JR_RA, NOP, jal_to(Y_BASE), NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let snapshots = compose_materialized_banks_v1(
            &rom,
            &facts,
            &two_bank_inputs(&x, &y, &[X_BASE, candidate_caller], &[Y_BASE]),
        )
        .unwrap();
        assert!(callee_entry_not_authoritative(&snapshots));
        assert!(!snapshots[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "caller"
                    && source.pc == candidate_caller
                    && target.bank == "callee"
        )));
    }

    #[test]
    fn cross_bank_jal_missing_the_callee_range_confers_no_authority() {
        // X `jal`s an address that lands in NEITHER bank's proven VA range
        // (a gap between X and Y). No bank claims it, so no authority is
        // conferred and Y's own entry is untouched.
        let stray = Y_BASE - 0x100; // between X and Y, mapped by no bank
        let x = asm(&[jal_to(stray), NOP, JR_RA, NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(
            !callee_owner_is_proven(&snapshots),
            "a jal missing the callee's VA range must not authorize its entry"
        );
        assert!(callee_entry_not_authoritative(&snapshots));
    }

    #[test]
    fn exhaustive_cross_bank_computed_call_authorizes_callee_and_serializes_its_kind() {
        // X reaches Y through a computed `jalr $ra, $t9` (t9 built with
        // lui/ori). The value-set proof is exhaustive, so the cross-bank rule
        // is exactly the same authority already accepted within one bank.
        let lui_t9 = 0x3c19_0000 | (Y_BASE >> 16); // lui $t9, hi(Y)
        let ori_t9 = 0x3739_0000 | (Y_BASE & 0xffff); // ori $t9, $t9, lo(Y)
        let jalr_ra_t9 = (25u32 << 21) | (31u32 << 11) | 0x09; // jalr $ra, $t9
        let x = asm(&[lui_t9, ori_t9, jalr_ra_t9, NOP, JR_RA, NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(
            callee_owner_is_proven(&snapshots),
            "an exhaustive cross-bank computed call should authorize the callee entry"
        );
        assert_eq!(snapshots[1].schema_version, PROGRAM_SNAPSHOT_SCHEMA_V6);
        assert!(snapshots[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::ResolvedCall { source, target }
                if source.bank == "caller"
                    && source.pc == X_BASE + 8
                    && target.bank == "callee"
                    && target.pc == Y_BASE
        )));
        let wire = serde_json::to_value(&snapshots[1]).unwrap();
        assert_eq!(wire["schema_version"], PROGRAM_SNAPSHOT_SCHEMA_V6);
        assert!(wire["facts"]["facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fact| fact.get("ResolvedCall").is_some()));
    }

    #[test]
    fn cross_bank_computed_jump_confers_no_callable_authority() {
        let lui_t9 = 0x3c19_0000 | (Y_BASE >> 16);
        let ori_t9 = 0x3739_0000 | (Y_BASE & 0xffff);
        let jr_t9 = (25u32 << 21) | 0x08;
        let x = asm(&[lui_t9, ori_t9, jr_t9, NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let snapshots = compose_materialized_banks_v1(
            &rom,
            &facts,
            &two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]),
        )
        .unwrap();
        assert!(callee_entry_not_authoritative(&snapshots));
        assert!(!snapshots[1]
            .facts
            .facts()
            .iter()
            .any(|fact| matches!(fact, Fact::ResolvedCall { .. })));
    }

    fn prepared_resolved_call(
        state: IndirectTransferState,
        evidence_sets: &[Vec<u32>],
        source_word_proven: bool,
        delay_word_proven: bool,
    ) -> (PreparedBank, crate::cfg::BasicBlock) {
        let site_pc = X_BASE;
        let block = crate::cfg::BasicBlock {
            start_va: site_pc,
            end_va: site_pc + 8,
            terminator: crate::cfg::BlockTerminator::ResolvedIndirect {
                targets: vec![Y_BASE],
                via_call: true,
            },
        };
        let mut facts = FactDb::new();
        for targets in evidence_sets {
            facts.insert(Fact::IndirectTransferAnalysis {
                site: BankAddr::new("caller", site_pc),
                via_call: true,
                state,
                kind: Some(IndirectTransferKind::Constant),
                targets: targets.clone(),
                memory_sources: Vec::new(),
            });
        }
        let closure = ClosureResult {
            cfg: crate::cfg::Cfg {
                bank: "caller".into(),
                word_class: [
                    (
                        site_pc,
                        if source_word_proven {
                            crate::cfg::WordClass::ProvenCode
                        } else {
                            crate::cfg::WordClass::Unknown
                        },
                    ),
                    (
                        site_pc + 4,
                        if delay_word_proven {
                            crate::cfg::WordClass::ProvenCode
                        } else {
                            crate::cfg::WordClass::Unknown
                        },
                    ),
                ]
                .into_iter()
                .collect(),
                blocks: vec![block.clone()],
                direct_calls: Vec::new(),
                tail_transfers: Vec::new(),
                indirect_sites: Vec::new(),
                plain_delay_entry_aliases: Vec::new(),
                unsupported_delay_entries: Vec::new(),
                rejected_transfer_targets: Vec::new(),
                proven_roots: vec![site_pc],
            },
            indirect: evidence_sets
                .iter()
                .map(|targets| crate::resolve::IndirectResolution {
                    site_pc,
                    via_call: true,
                    state: match state {
                        IndirectTransferState::Exhaustive => IndirectProofState::Exhaustive,
                        IndirectTransferState::Bounded => IndirectProofState::Bounded,
                        IndirectTransferState::Open => IndirectProofState::Open,
                    },
                    kind: Some(IndirectResolutionKind::Constant),
                    targets: targets.clone(),
                    memory_sources: Vec::new(),
                })
                .collect(),
        };
        (
            PreparedBank {
                bank: "caller".into(),
                va_start: X_BASE,
                va_end: X_BASE + 8,
                bytes: vec![0; 8],
                digest: BankInputDigestV1 {
                    bank: "caller".into(),
                    va_start: X_BASE,
                    va_end: X_BASE + 8,
                    backing: BankBackingSpanV1::RomAffine {
                        rom_space: RomAddressSpace::Physical,
                        rom_start: X_ROM,
                        rom_end: X_ROM + 8,
                    },
                    bytes_sha256: sha256_hex(&[0; 8]),
                },
                facts,
                authority_closure: closure.clone(),
                closure,
                traversal_roots: BTreeSet::from([site_pc]),
                semantic_callable_entries: BTreeSet::new(),
                authorized_callable_roots: BTreeSet::new(),
                cross_bank_reachability_roots: BTreeSet::new(),
                semantic_cross_bank_roots: BTreeSet::new(),
            },
            block,
        )
    }

    #[test]
    fn bounded_cross_bank_call_set_stays_non_authoritative() {
        let (source, block) =
            prepared_resolved_call(IndirectTransferState::Bounded, &[vec![Y_BASE]], true, true);
        assert_eq!(
            authoritative_resolved_call_site(&source, &block, &[Y_BASE]),
            None
        );
    }

    #[test]
    fn open_cross_bank_call_claim_stays_non_authoritative() {
        let (source, block) =
            prepared_resolved_call(IndirectTransferState::Open, &[vec![Y_BASE]], true, true);
        assert_eq!(
            authoritative_resolved_call_site(&source, &block, &[Y_BASE]),
            None
        );
    }

    #[test]
    fn broad_and_authority_resolved_targets_must_agree() {
        let (source, mut broad_block) = prepared_resolved_call(
            IndirectTransferState::Exhaustive,
            &[vec![Y_BASE]],
            true,
            true,
        );
        broad_block.terminator = crate::cfg::BlockTerminator::ResolvedIndirect {
            targets: vec![Y_BASE + 4],
            via_call: true,
        };
        assert_eq!(
            authoritative_resolved_call_site(&source, &broad_block, &[Y_BASE + 4]),
            None
        );
    }

    #[test]
    fn unresolved_source_or_delay_word_stays_non_authoritative() {
        for (source_proven, delay_proven) in [(false, true), (true, false)] {
            let (source, block) = prepared_resolved_call(
                IndirectTransferState::Exhaustive,
                &[vec![Y_BASE]],
                source_proven,
                delay_proven,
            );
            assert_eq!(
                authoritative_resolved_call_site(&source, &block, &[Y_BASE]),
                None
            );
        }
    }

    #[test]
    fn duplicate_disagreeing_or_mismatched_analysis_stays_non_authoritative() {
        for evidence_sets in [
            vec![vec![Y_BASE + 4]],
            vec![vec![Y_BASE], vec![Y_BASE + 4]],
            vec![vec![Y_BASE], vec![Y_BASE]],
        ] {
            let (source, block) = prepared_resolved_call(
                IndirectTransferState::Exhaustive,
                &evidence_sets,
                true,
                true,
            );
            assert_eq!(
                authoritative_resolved_call_site(&source, &block, &[Y_BASE]),
                None
            );
        }
    }

    #[test]
    fn broad_only_exhaustive_cross_bank_call_stays_non_authoritative() {
        let (mut source, block) = prepared_resolved_call(
            IndirectTransferState::Exhaustive,
            &[vec![Y_BASE]],
            true,
            true,
        );
        source.authority_closure.cfg.blocks.clear();
        source.authority_closure.cfg.word_class.clear();

        assert_eq!(
            authoritative_resolved_call_site(&source, &block, &[Y_BASE]),
            None
        );
    }

    #[test]
    fn multi_bank_solo_matches_single_bank_composition() {
        // A bank composed alone through the multi-bank entry point must be
        // byte-identical to `compose_materialized_bank_v1`: the no-sibling path
        // adds no authority and re-shapes nothing.
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let single = compose_materialized_bank_v1(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
        )
        .unwrap();
        let multi = compose_materialized_banks_v1(
            &rom,
            &facts,
            &[MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            }],
        )
        .unwrap();
        assert_eq!(
            serde_json::to_vec(&single).unwrap(),
            serde_json::to_vec(&multi[0]).unwrap()
        );
    }
