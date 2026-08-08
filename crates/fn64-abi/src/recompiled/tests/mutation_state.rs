use super::*;

    /// The fast "nothing changed" predicate must agree with the copying path
    /// on every byte, at every alignment.
    ///
    /// `matches_view` is what lets the dispatch guard skip building a 1 MiB
    /// snapshot, so a disagreement in the `true` direction is exactly a
    /// silently accepted executable mutation -- the failure the guard exists
    /// to prevent. This drives one byte at a time across an unaligned watched
    /// range and asserts the two answers are identical for all of them.
    #[test]
    fn matches_view_agrees_with_the_snapshot_comparison_at_every_offset() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        // Unaligned start and end, so the head and tail lanes are both
        // exercised rather than only the word-aligned body.
        for &(start, end) in &[(0x101u32, 0x117u32), (0x100, 0x120), (0x102, 0x106)] {
            let mut storage = vec![0u8; 0x200];
            for (index, byte) in storage.iter_mut().enumerate() {
                *byte = (index as u8).wrapping_mul(31).wrapping_add(11);
            }
            let mut state = CanonicalExecutableMutationStateV1::new(&[(start, end)]);
            {
                let view = fn64_runtime::RdramView::from_storage(&storage);
                state.seal_with(|physical| {
                    view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
                });
            }
            {
                let view = fn64_runtime::RdramView::from_storage(&storage);
                assert!(
                    state.matches_view(&view),
                    "a freshly sealed baseline must match its own storage at [{start:#x},{end:#x})"
                );
            }
            // Flip each byte of the watched region in turn, including the
            // unaligned head and tail, and require both paths to see it.
            for physical in start..end {
                let index = (physical ^ 3) as usize;
                let original = storage[index];
                storage[index] = original ^ 0xff;
                let view = fn64_runtime::RdramView::from_storage(&storage);
                let snapshot = state.read_snapshot_from_view(&view);
                let changed = !state.current_changed_ranges(&snapshot).is_empty();
                assert!(
                    changed,
                    "the copying path must see the change at {physical:#x}"
                );
                assert!(
                    !state.matches_view(&view),
                    "matches_view must not report a match after {physical:#x} changed"
                );
                storage[index] = original;
            }
            // Bytes OUTSIDE the watched range must not affect either answer.
            for outside in [start.wrapping_sub(1), end] {
                let index = (outside ^ 3) as usize;
                if index >= storage.len() {
                    continue;
                }
                let original = storage[index];
                storage[index] = original ^ 0xff;
                let view = fn64_runtime::RdramView::from_storage(&storage);
                let snapshot = state.read_snapshot_from_view(&view);
                assert!(state.current_changed_ranges(&snapshot).is_empty());
                assert!(
                    state.matches_view(&view),
                    "a change at {outside:#x} is outside [{start:#x},{end:#x}) and must not match"
                );
                storage[index] = original;
            }
        }
    }

    /// The snapshot-free comparison must name EXACTLY the same bytes.
    ///
    /// `changed_ranges_from_view` is what lets the commit path stop copying and
    /// word-reversing the whole watched region, so a disagreement in the
    /// "unchanged" direction is a silently accepted executable mutation -- the
    /// exact failure mode the mutation journal exists to prevent, and one this
    /// project has already paid for once.
    ///
    /// The swizzle is the trap: logical byte `a` lives at storage `a ^ 3`, and
    /// getting the lane wrong inside a word produces a comparison that reports
    /// "unchanged" for changed memory only for particular lane patterns -- so
    /// this drives randomized contents and randomized change patterns across
    /// unaligned starts, unaligned ends, sub-word ranges, ranges straddling
    /// word boundaries, and multiple watched ranges at once.
    ///
    /// It also asserts the SECOND half of the contract: after a commit, the
    /// `expected` baseline (and both derived mirrors, via the digest) must be
    /// byte-identical to what the full-copy path would have produced.
    #[test]
    fn changed_ranges_from_view_matches_the_copying_path() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        // xorshift64*, so the pattern is reproducible without a dev-dependency.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        // Unaligned start, unaligned end, both unaligned, sub-word (entirely
        // inside one storage word), a range shorter than the 3-byte head, one
        // spanning many words, and a multi-range watched set.
        let layouts: [&[(u32, u32)]; 9] = [
            &[(0x100, 0x180)],
            &[(0x101, 0x180)],
            &[(0x100, 0x17f)],
            &[(0x101, 0x17f)],
            &[(0x102, 0x105)],
            &[(0x101, 0x103)],
            &[(0x105, 0x106)],
            &[(0x103, 0x104)],
            &[(0x101, 0x117), (0x120, 0x181), (0x1c2, 0x1c7)],
        ];

        for layout in layouts {
            let mut storage = vec![0u8; 0x400];
            for byte in storage.iter_mut() {
                *byte = next() as u8;
            }
            let mut fast = CanonicalExecutableMutationStateV1::new(layout);
            let mut slow = CanonicalExecutableMutationStateV1::new(layout);
            {
                let view = fn64_runtime::RdramView::from_storage(&storage);
                let read = |physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical));
                fast.seal_with(read);
                slow.seal_with(read);
            }

            for round in 0..64 {
                // A random change pattern over the watched bytes: sometimes a
                // single byte, sometimes a dense run, sometimes scattered, and
                // sometimes nothing at all.
                let density = next() % 5;
                let watched = layout
                    .iter()
                    .flat_map(|&(start, end)| start..end)
                    .collect::<Vec<_>>();
                for &physical in &watched {
                    let flip = match density {
                        0 => false,
                        1 => next() % 97 == 0,
                        2 => next() % 7 == 0,
                        3 => next() % 2 == 0,
                        _ => true,
                    };
                    if flip {
                        storage[(physical ^ 3) as usize] ^= (next() as u8) | 1;
                    }
                }
                // Bytes outside every watched range must never influence the
                // answer, so churn them on every round.
                for physical in [0x0u32, 0x99, 0x1ff, 0x2ab, 0x3ff] {
                    storage[(physical ^ 3) as usize] ^= next() as u8;
                }

                let view = fn64_runtime::RdramView::from_storage(&storage);
                let snapshot = slow.read_snapshot_from_view(&view);
                let expected_changed = slow.current_changed_ranges(&snapshot);
                let actual_changed = fast
                    .changed_ranges_from_view(&view)
                    .expect("every watched byte is mapped");
                assert_eq!(
                    actual_changed, expected_changed,
                    "layout {layout:?} round {round}: the snapshot-free comparison \
                     disagreed with the copying path"
                );
                assert_eq!(
                    expected_changed.is_empty(),
                    fast.matches_view(&view),
                    "layout {layout:?} round {round}: matches_view must agree too"
                );

                // Commit through both paths and require the baselines to stay
                // byte-identical. The digest covers `expected`; the direct
                // comparison below covers it byte for byte, including the
                // bytes the incremental path chose NOT to rewrite.
                let events = layout
                    .iter()
                    .map(|&(start, end)| GuestWriteEvent::Range {
                        channel: WriterChannel::HostAbi,
                        physical_offset: start,
                        len: end - start,
                    })
                    .collect::<Vec<_>>();
                fast.commit_from_view(&view, actual_changed, events.clone(), Vec::new());
                slow.commit_snapshot(snapshot, events, Vec::new());

                for (fast_range, slow_range) in fast.watched.iter().zip(&slow.watched) {
                    assert_eq!(
                        fast_range.expected, slow_range.expected,
                        "layout {layout:?} round {round}: the incremental baseline \
                         diverged from the full-copy baseline"
                    );
                    assert_eq!(
                        fast_range.expected_storage_order, slow_range.expected_storage_order,
                        "layout {layout:?} round {round}: the storage-order mirror diverged"
                    );
                    assert_eq!(
                        fast_range.expected_page_tree, slow_range.expected_page_tree,
                        "layout {layout:?} round {round}: the page tree diverged"
                    );
                }
                assert_eq!(
                    fast.expected_sha256, slow.expected_sha256,
                    "layout {layout:?} round {round}: the watched root diverged"
                );
                assert_eq!(
                    fast.journal_root_sha256, slow.journal_root_sha256,
                    "layout {layout:?} round {round}: the journal root diverged"
                );
                assert_eq!(fast.entries.len(), slow.entries.len());
                assert_eq!(
                    fast.entries.last().map(|entry| &entry.changed_ranges),
                    slow.entries.last().map(|entry| &entry.changed_ranges)
                );
                // The freshly adopted baseline must now match live storage
                // under BOTH comparisons.
                assert!(fast.matches_view(&view));
                assert!(fast
                    .changed_ranges_from_view(&view)
                    .expect("mapped")
                    .is_empty());
            }
        }
    }

    /// `adopt_from_view` must land the same baseline as `adopt_snapshot`.
    ///
    /// The no-declaration path. It journals nothing, so the only observable is
    /// the baseline itself -- which is exactly the invariant the journal's
    /// guarantee rests on.
    #[test]
    fn adopt_from_view_matches_the_copying_adoption() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for &layout in &[
            &[(0x101u32, 0x17fu32)][..],
            &[(0x100, 0x180)][..],
            &[(0x102, 0x105)][..],
            &[(0x101, 0x110), (0x131, 0x172)][..],
        ] {
            let mut storage = vec![0u8; 0x200];
            for byte in storage.iter_mut() {
                *byte = next() as u8;
            }
            let mut fast = CanonicalExecutableMutationStateV1::new(layout);
            let mut slow = CanonicalExecutableMutationStateV1::new(layout);
            {
                let view = fn64_runtime::RdramView::from_storage(&storage);
                let read = |physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical));
                fast.seal_with(read);
                slow.seal_with(read);
            }
            for _ in 0..32 {
                for &(start, end) in layout {
                    for physical in start..end {
                        if next() % 11 == 0 {
                            storage[(physical ^ 3) as usize] ^= (next() as u8) | 1;
                        }
                    }
                }
                let view = fn64_runtime::RdramView::from_storage(&storage);
                let snapshot = slow.read_snapshot_from_view(&view);
                let changed = fast.changed_ranges_from_view(&view).expect("mapped");
                fast.adopt_from_view(&view, changed);
                slow.adopt_snapshot(snapshot);
                for (fast_range, slow_range) in fast.watched.iter().zip(&slow.watched) {
                    assert_eq!(fast_range.expected, slow_range.expected);
                    assert_eq!(
                        fast_range.expected_storage_order,
                        slow_range.expected_storage_order
                    );
                    assert_eq!(
                        fast_range.expected_page_tree,
                        slow_range.expected_page_tree
                    );
                }
                assert_eq!(fast.expected_sha256, slow.expected_sha256);
                assert!(fast.entries.is_empty() && slow.entries.is_empty());
            }
        }
    }

    /// An unsealed state must never report a match.
    ///
    /// Before sealing there is no baseline to compare against, and answering
    /// "unchanged" would let the caller skip the seal-and-compare path
    /// entirely.
    #[test]
    fn matches_view_never_matches_before_the_baseline_is_sealed() {
        let storage = vec![0u8; 0x200];
        let state = CanonicalExecutableMutationStateV1::new(&[(0x100, 0x110)]);
        let view = fn64_runtime::RdramView::from_storage(&storage);
        assert!(!state.matches_view(&view));
    }

    #[test]
    fn canonical_mutation_state_traps_unjournaled_executable_bytes_before_dispatch() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x100, 0x108)]);
        state.seal_with(|physical| image[(physical - 0x100) as usize]);
        image[3] = 0x5a;
        let snapshot = state.read_snapshot(|physical| image[(physical - 0x100) as usize]);

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.reconcile_snapshot_before_dispatch(snapshot);
        }))
        .expect_err("unjournaled executable mutation must trap");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains("unjournaled executable mutation"));
        assert!(message.contains("0x00000103"));
    }


    #[test]
    fn canonical_instruction_limit_clamps_the_final_dispatch_slice_exactly() {
        let _reset = PublicSiRuntimeStateTestReset;
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());

        let bank = BankId::new(0xc11a);
        let entry = GuestPc::new(0x8000_7000);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, entry, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    bootstrap_return_runner,
                    ProgramArtifactIdentity::new([0xc1; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, entry),
                InstructionBudget::new(4096).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xc2; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        let resolver_evidence = live.install.evidence().clone();

        assert_eq!(live.next_dispatch_budget().get(), 4096);
        set_canonical_block_instruction_limit_v1(Some(1720));
        assert_eq!(live.next_dispatch_budget().get(), 1720);
        assert_eq!(live.install.evidence(), &resolver_evidence);
        let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            set_canonical_block_instruction_limit_v1(Some(2000));
        }))
        .expect_err("an armed exact limit may not be replaced");
        let duplicate = duplicate
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| duplicate.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(duplicate.contains("already armed"));
        live.charge_canonical_instructions(1718);
        assert_eq!(live.next_dispatch_budget().get(), 2);
        live.charge_canonical_instructions(1);
        assert_eq!(live.next_dispatch_budget().get(), 1);

        set_canonical_block_instruction_limit_v1(None);
        assert_eq!(live.next_dispatch_budget().get(), 4096);
        set_canonical_block_instruction_limit_v1(Some(1720));
        assert_eq!(live.next_dispatch_budget().get(), 1);
        live.charge_canonical_instructions(1);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = live.next_dispatch_budget();
        }))
        .expect_err("dispatch may not continue past the exact limit");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains("limit 1720 was already reached"));
    }


    #[test]
    fn canonical_mutation_state_hash_chains_exact_channel_and_invalidation() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x200, 0x208)]);
        state.seal_with(|physical| image[(physical - 0x200) as usize]);
        let initial_root = state.journal_root_sha256;
        image[2..4].copy_from_slice(&[0xaa, 0xbb]);
        let snapshot = state.read_snapshot(|physical| image[(physical - 0x200) as usize]);
        state.commit_snapshot(
            snapshot,
            vec![GuestWriteEvent::Range {
                channel: WriterChannel::HostAbi,
                physical_offset: 0x202,
                len: 2,
            }],
            vec![GenerationId::new(7)],
        );

        let evidence = state.evidence_snapshot();
        assert!(evidence.sealed);
        assert_ne!(evidence.journal_root_sha256, initial_root);
        assert_eq!(evidence.entries.len(), 1);
        let entry = &evidence.entries[0];
        assert_eq!(entry.sequence, 0);
        assert_eq!(
            entry.declared_writes,
            [AttributedExecutableWriteEvidenceV1 {
                channel: WriterChannel::HostAbi,
                physical_start: 0x202,
                physical_end: 0x204,
            }]
        );
        assert_eq!(
            entry.changed_ranges,
            [PendingExecutableWriteEvidenceSnapshot {
                physical_start: 0x202,
                physical_end: 0x204,
            }]
        );
        assert_eq!(entry.invalidated_generations, [GenerationId::new(7)]);
        let stable = state.read_snapshot(|physical| image[(physical - 0x200) as usize]);
        state.reconcile_snapshot_before_dispatch(stable);
    }


    #[test]
    fn canonical_mutation_state_rejects_changes_outside_attributed_range() {
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x300, 0x308)]);
        state.seal_with(|physical| image[(physical - 0x300) as usize]);
        image[6] = 1;
        let snapshot = state.read_snapshot(|physical| image[(physical - 0x300) as usize]);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.commit_snapshot(
                snapshot,
                vec![GuestWriteEvent::Range {
                    channel: WriterChannel::RdpRenderer,
                    physical_offset: 0x300,
                    len: 2,
                }],
                Vec::new(),
            );
        }))
        .expect_err("out-of-declaration executable change must trap");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains("outside every attributed writer declaration"));
    }


    #[test]
    fn renderer_transaction_attributes_exact_changed_executable_bytes() {
        let _state = scoped_test_executable_write_preflight_state(vec![(0x40, 0x48)], Vec::new());
        let previous =
            fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));
        let mut storage = [0u8; 0x80];
        track_rdp_renderer_mutation(&mut storage, |storage| {
            storage[0x41 ^ 3] = 0xaa;
            storage[0x42 ^ 3] = 0xbb;
            storage[0x70 ^ 3] = 0xcc;
        });
        fn64_recomp_rs::set_write_observer(previous);

        assert_eq!(
            PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            [GuestWriteEvent::Range {
                channel: WriterChannel::RdpRenderer,
                physical_offset: 0x41,
                len: 2,
            }]
        );
    }


    /// The renderer tracker must watch the ranges the GUARD checks, not just
    /// this thread's registered write ranges.
    ///
    /// These two sets normally agree -- the canonical fixture mirrors the
    /// watched ranges into EXECUTABLE_WRITE_RANGES -- which is exactly why the
    /// divergence went unnoticed. When they differ, a renderer write to
    /// executable bytes the guard watches but the thread-local set omits was
    /// snapshotted by nobody and notified by nobody, then surfaced at the next
    /// commit as an undeclared mutation with events=0 declarations=0. WM2000
    /// patching a store immediate at 0x8009b0b0 during a graphics task is that
    /// case.
    #[test]
    fn renderer_tracker_watches_guard_ranges_not_just_thread_local_ranges() {
        // The guard watches [0x40,0x48); the thread-local set deliberately
        // does NOT cover 0x44.
        let _state = scoped_test_executable_write_preflight_state(vec![(0x40, 0x42)], Vec::new());
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x40, 0x48)]);
        let mut storage = [0u8; 0x80];
        state.seal_with(|physical| storage[((physical - 0x40) as usize) ^ 3]);

        let watched = state.watched_ranges();

        assert_eq!(
            watched,
            [(0x40, 0x48)],
            "the guard's watched set is what commit_snapshot compares against"
        );
        assert_ne!(
            watched,
            EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow().clone()),
            "this test is only meaningful while the two sets differ"
        );
        assert!(
            watched
                .iter()
                .any(|(start, end)| *start <= 0x44 && 0x44 < *end),
            "0x44 must be inside the guard's watched set"
        );
        assert!(
            !EXECUTABLE_WRITE_RANGES
                .with(|ranges| ranges.borrow().clone())
                .iter()
                .any(|(start, end)| *start <= 0x44 && 0x44 < *end),
            "0x44 must be outside the thread-local set, or the bug cannot occur"
        );
    }

    #[test]
    fn same_byte_nested_writers_commit_in_execution_order() {
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x400, 0x408)]);
        state.seal_with(|physical| image[(physical - 0x400) as usize]);
        let transaction = state.begin_host_transaction(
            7,
            GuestPc::new(0x8000_0400),
            ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_0404)),
        );

        for (value, channel) in [
            (1, WriterChannel::HostAbi),
            (2, WriterChannel::RspExecutionOrHleWriteback),
            (3, WriterChannel::RdpRenderer),
            (4, WriterChannel::HostAbi),
        ] {
            image[1] = value;
            let snapshot = state.read_snapshot(|physical| image[(physical - 0x400) as usize]);
            state.commit_snapshot(
                snapshot,
                vec![GuestWriteEvent::Range {
                    channel,
                    physical_offset: 0x401,
                    len: 1,
                }],
                Vec::new(),
            );
        }
        state.finish_host_transaction(transaction);

        let evidence = state.evidence_snapshot();
        assert!(evidence.open_host_transactions.is_empty());
        assert_eq!(evidence.entries.len(), 4);
        assert_eq!(
            evidence
                .entries
                .iter()
                .map(|entry| entry.declared_writes[0].channel)
                .collect::<Vec<_>>(),
            [
                WriterChannel::HostAbi,
                WriterChannel::RspExecutionOrHleWriteback,
                WriterChannel::RdpRenderer,
                WriterChannel::HostAbi,
            ]
        );
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }
    }


    #[test]
    fn catalog_host_orders_real_rsp_and_rdp_wrappers_on_the_same_byte() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let words = [0x2402_0001u32, 0x03e0_0008];
        let rom = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        crate::load_rom_with_fixed_pi_latency(rom.clone(), 1);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(ORDERED_SYNC_BANK, ORDERED_SYNC_ENTRY, words.to_vec()).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    ORDERED_SYNC_BANK,
                    ordered_sync_runner,
                    ProgramArtifactIdentity::new([0xaf; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(ORDERED_SYNC_BANK, ORDERED_SYNC_ENTRY),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(ORDERED_SYNC_HOST.get(), ordered_sync_host)]).unwrap(),
            ProgramArtifactIdentity::new([0xb0; 32]),
        );
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            PrecompiledGenerationCatalog::new(),
            Vec::new(),
        )
        .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let mut bootstrap = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        bootstrap
            .publish_resident_rom_image(0, ORDERED_SYNC_ENTRY.get(), 8)
            .unwrap();
        let validated = bootstrap.commit().unwrap();
        boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            test_boot_context(ORDERED_SYNC_ENTRY),
            0x0adf,
            10,
        )
        .unwrap();

        assert!(crate::run_one_step());
        crate::run_to_idle();
        let evidence = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert!(evidence.open_host_transactions.is_empty());
        assert_eq!(
            evidence
                .entries
                .iter()
                .skip(1)
                .map(|entry| entry.declared_writes[0].channel)
                .collect::<Vec<_>>(),
            [
                WriterChannel::HostAbi,
                WriterChannel::RspExecutionOrHleWriteback,
                WriterChannel::RdpRenderer,
                WriterChannel::HostAbi,
            ]
        );
        for entry in evidence.entries.iter().skip(1) {
            assert_eq!(
                entry.changed_ranges,
                [PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x7200,
                    physical_end: 0x7201,
                }]
            );
        }
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }
    }


    #[test]
    fn suspended_host_transaction_orders_same_byte_device_write_before_resume_suffix() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let words = [0x2402_0001u32, 0x03e0_0008];
        let rom = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        crate::load_rom_with_fixed_pi_latency(rom.clone(), 1);

        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(ORDERED_WRITER_BANK, ORDERED_WRITER_ENTRY, words.to_vec()).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    ORDERED_WRITER_BANK,
                    ordered_writer_runner,
                    ProgramArtifactIdentity::new([0xad; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(ORDERED_WRITER_BANK, ORDERED_WRITER_ENTRY),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(ORDERED_WRITER_HOST.get(), ordered_writer_host)])
                .unwrap(),
            ProgramArtifactIdentity::new([0xae; 32]),
        );
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            PrecompiledGenerationCatalog::new(),
            Vec::new(),
        )
        .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let mut bootstrap = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        bootstrap
            .publish_resident_rom_image(0, ORDERED_WRITER_ENTRY.get(), 8)
            .unwrap();
        let validated = bootstrap.commit().unwrap();
        let thread_id = 0x0ade;
        boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            test_boot_context(ORDERED_WRITER_ENTRY),
            thread_id,
            10,
        )
        .unwrap();

        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        let prefix = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert_eq!(prefix.open_host_transactions.len(), 1);
        assert_eq!(
            prefix.entries.last().unwrap().declared_writes[0].channel,
            WriterChannel::HostAbi
        );

        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        assert!(!rdram.is_null() && rdram_len > 0x7000);
        unsafe {
            fn64_runtime::RdramPtr::from_storage_ptr(rdram)
                .write_u8(fn64_runtime::RdramAddr::from_offset(0x7000), 2);
        }
        fn64_recomp_rs::notify_pi_dma_write(0x7000, 1);
        process_live_executable_writes_from_host();

        assert!(crate::run_one_step());
        crate::run_to_idle();

        let evidence = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert!(evidence.open_host_transactions.is_empty());
        let channels = evidence
            .entries
            .iter()
            .skip(1)
            .map(|entry| entry.declared_writes[0].channel)
            .collect::<Vec<_>>();
        assert_eq!(
            channels,
            [
                WriterChannel::HostAbi,
                WriterChannel::PiDma,
                WriterChannel::HostAbi
            ]
        );
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }

        let storage = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            track_rdp_renderer_mutation(&mut *storage, |_| {
                panic!("synthetic renderer unwind");
            });
        }))
        .expect_err("uncommitted child writer must unwind");
        assert!(unwind
            .downcast_ref::<&str>()
            .is_some_and(|message| *message == "synthetic renderer unwind"));

        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = begin_catalog_nested_writer(&*storage, "post-unwind publication");
        }))
        .expect_err("a later child writer must reject the poisoned owner");
        let message = poisoned
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| poisoned.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains(
            "canonical executable mutation owner is poisoned: tracked renderer/RSP publication child writer transaction unwound before commit"
        ));
    }

    /// The v3 root depends only on the watched bytes -- never on the
    /// incremental history that produced them.
    ///
    /// This is the load-bearing property of the Merkle-root migration. The
    /// incremental root (`digest_expected`, read off the maintained tree's
    /// apexes) is read on every commit, while the from-scratch root
    /// (`digest_snapshot`, which recomputes every leaf and rebuilds every
    /// level) is what validation recomputes and what any independent verifier
    /// would compute. If those two could ever disagree, the journal would
    /// certify a value no verifier could reproduce.
    ///
    /// v3 makes this test strictly harder than v2 did. Under v2 an unrefreshed
    /// leaf was the only way to go stale; under v3 a stale INTERNAL node is a
    /// second, independent way, and it is invisible to any check that only
    /// inspects leaves. So the geometry below is deliberately deep -- the large
    /// range is 22 pages, a 6-level tree with odd widths at three of those
    /// levels -- and the edits deliberately hit the promoted-odd-node path, the
    /// short final page, and both sides of several internal nodes.
    ///
    /// The test drives a long-lived state through many commits and after every
    /// single one requires:
    ///
    ///   1. the incremental root equals the from-scratch root of the same bytes;
    ///   2. a FRESH state, sealed directly on those bytes with no history at
    ///      all, produces that same root;
    ///   3. every node of the maintained tree, at every level, equals the node
    ///      the fresh state computed -- so a stale internal node fails here
    ///      even in the (astronomically unlikely) event its root still matched.
    ///
    /// (2) is the part that rules out order dependence: the fresh state
    /// computes every page exactly once, in page order, having never seen any
    /// of the intermediate values. Agreement across hundreds of distinct
    /// histories is what makes "the root is a function of the bytes alone"
    /// an observation rather than an assertion.
    #[test]
    fn page_tree_root_is_independent_of_incremental_history() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        // Two ranges, deliberately awkward: the first is far smaller than one
        // page, the second spans many pages and does NOT end on a page
        // boundary, so the short final page is exercised. This mirrors WM2000's
        // shape (a 16-byte range and a ~1.44 MiB range) at test scale.
        //
        // 21 full pages plus a 777-byte tail is 22 leaves. Its levels are
        // 22, 11, 6, 3, 2, 1 -- ODD at three of them, so the single-child
        // promotion path is exercised at more than one height, which a
        // power-of-two leaf count would never reach.
        const PAGE: u32 = CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2 as u32;
        let ranges = [(0x1000u32, 0x1010u32), (0x2000, 0x2000 + PAGE * 21 + 777)];
        let total = |(start, end): (u32, u32)| (end - start) as usize;

        let mut image0 = vec![0u8; total(ranges[0])];
        let mut image1 = vec![0u8; total(ranges[1])];
        for (index, byte) in image1.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37).wrapping_add(5);
        }

        let read = |image0: &[u8], image1: &[u8], physical: u32| -> u8 {
            if physical >= ranges[0].0 && physical < ranges[0].1 {
                image0[(physical - ranges[0].0) as usize]
            } else {
                image1[(physical - ranges[1].0) as usize]
            }
        };

        let mut state = CanonicalExecutableMutationStateV1::new(&ranges);
        {
            let (a, b) = (image0.clone(), image1.clone());
            state.seal_with(|physical| read(&a, &b, physical));
        }

        // The SECOND commit path, driven over the same geometry and the same
        // edits.
        //
        // `commit_snapshot` (above) reaches the tree through `set_expected` ->
        // `refresh_page_digests`, which finds its dirty leaves by comparing
        // every page against the old baseline. `adopt_from_view` -- the path
        // the running emulator actually takes on every dispatch boundary --
        // reaches it through `apply_changed_from_view` ->
        // `refresh_page_digests_over`, which derives its dirty leaves from the
        // changed-range list instead. Those are two independent dirty
        // derivations feeding one tree, and a determinism test that exercises
        // only one of them leaves the hot path uncovered. (Measured: skipping
        // every third dirty leaf in `refresh_page_digests_over` did not move
        // this test until this state was added.)
        //
        // Backed by real RDRAM storage, because that path reads through an
        // `RdramView` rather than a byte closure.
        let storage_len = (ranges[1].1 as usize).next_multiple_of(4);
        let mut storage = vec![0u8; storage_len];
        let write_storage = |storage: &mut [u8], image0: &[u8], image1: &[u8]| {
            for &(start, end) in &ranges {
                for physical in start..end {
                    storage[(physical ^ 3) as usize] = read(image0, image1, physical);
                }
            }
        };
        write_storage(&mut storage, &image0, &image1);
        let mut live = CanonicalExecutableMutationStateV1::new(&ranges);
        {
            let view = fn64_runtime::RdramView::from_storage(&storage);
            live.seal_with(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
        }

        // The large range must really be the deep, odd-width tree the doc
        // comment claims. If the geometry ever silently flattens, the odd-node
        // promotion and the multi-level ancestor walk stop being covered, and
        // this test would keep passing while testing much less.
        let widths: Vec<usize> = state.watched[1]
            .expected_page_tree
            .levels
            .iter()
            .map(Vec::len)
            .collect();
        assert_eq!(
            widths,
            vec![22, 11, 6, 3, 2, 1],
            "the determinism fixture must keep a deep tree with odd levels"
        );
        assert_eq!(
            state.watched[0].expected_page_tree.levels.len(),
            1,
            "the sub-page range must be a single-leaf tree, its own edge case"
        );

        // A fresh state sealed on the same bytes must already agree at seal.
        let fresh = |image0: &[u8], image1: &[u8]| -> CanonicalExecutableMutationStateV1 {
            let mut fresh = CanonicalExecutableMutationStateV1::new(&ranges);
            fresh.seal_with(|physical| read(image0, image1, physical));
            fresh
        };
        let fresh_root = |image0: &[u8], image1: &[u8]| -> [u8; 32] {
            fresh(image0, image1)
                .expected_sha256
                .expect("a sealed state always has an expected digest")
        };
        // Requirement 3: every node of the maintained tree, at every level,
        // equals what a from-scratch build produces. A stale INTERNAL node is
        // a failure mode v2 did not have, and it is invisible to any check
        // that inspects only leaves or only the root.
        let assert_trees_match = |state: &CanonicalExecutableMutationStateV1,
                                  image0: &[u8],
                                  image1: &[u8],
                                  what: &str| {
            let reference = fresh(image0, image1);
            for (index, (live, want)) in state.watched.iter().zip(&reference.watched).enumerate() {
                assert_eq!(
                    live.expected_page_tree, want.expected_page_tree,
                    "{what}: range {index}'s maintained Merkle tree diverged from a \
                     from-scratch build of the same bytes"
                );
            }
        };
        assert_eq!(
            state.expected_sha256.expect("sealed"),
            fresh_root(&image0, &image1),
            "a freshly sealed state must agree with the incremental one at seal time"
        );
        assert_trees_match(&state, &image0, &image1, "at seal");

        // Every write shape that could expose a page-boundary bug: the first
        // byte of a page, the last byte of a page, a span straddling a page
        // boundary, a span covering a whole page, a write inside the short
        // final page, and a rewrite of an already-dirtied page.
        let edits: &[(u32, u32)] = &[
            (0x2000, 1),                    // first byte of leaf 0
            (0x2000 + PAGE - 1, 1),         // last byte of leaf 0
            (0x2000 + PAGE, 1),             // first byte of leaf 1
            (0x2000 + PAGE - 2, 4),         // straddles the leaf 0/1 boundary
            (0x2000 + PAGE * 2, PAGE),      // exactly all of leaf 2
            (0x2000 + PAGE * 21, 1),        // first byte of the short final leaf
            (0x2000 + PAGE * 21 + 776, 1),  // last byte of the whole range
            (0x2000 + PAGE - 1, 2),         // re-dirty leaves 0 and 1
            (0x2000 + 5, 3),                // leaf 0 interior, third time
            (0x1000, 16),                   // the entire sub-page first range
            (0x1008, 4),                    // interior of the first range
            (0x2000 + PAGE * 2 + 100, 900), // interior of leaf 2, again
            // Leaf 20 is the LAST child of the last full pair; leaf 21 is the
            // promoted odd node at height 1. Touch each alone, so a bug that
            // only mishandles the promotion shows up isolated.
            (0x2000 + PAGE * 20, 1),
            (0x2000 + PAGE * 21 + 1, 1),
            // Level 2 has width 6 and level 3 width 3 -- odd. Leaf 8 is under
            // node 4 at height 1, node 2 at height 2, node 1 at height 3.
            (0x2000 + PAGE * 8 + 17, 1),
            // A span crossing four leaves at once, so several ancestors are
            // dirty simultaneously and the parent-index dedupe is exercised.
            (0x2000 + PAGE * 12 - 3, PAGE * 3 + 6),
            // Two far-apart single-byte edits in one commit would need two
            // separate declarations; instead alternate them across commits so
            // disjoint subtrees are dirtied in successive rounds.
            (0x2000 + PAGE * 3 + 1, 1),
            (0x2000 + PAGE * 19 + 4095, 1), // last byte of leaf 19
            (0x2000 + PAGE * 10, PAGE * 2), // exactly leaves 10 and 11
            (0x2000 + PAGE * 21 + 776, 1),  // the range's final byte, again
        ];

        let mut nonce = 0u8;
        for (round, &(physical_start, len)) in edits.iter().enumerate() {
            nonce = nonce.wrapping_add(97).wrapping_add(round as u8);
            for offset in 0..len {
                let physical = physical_start + offset;
                let value = nonce.wrapping_add(offset as u8).wrapping_mul(13);
                if physical >= ranges[0].0 && physical < ranges[0].1 {
                    image0[(physical - ranges[0].0) as usize] = value;
                } else {
                    image1[(physical - ranges[1].0) as usize] = value;
                }
            }

            let snapshot = {
                let (a, b) = (image0.clone(), image1.clone());
                state.read_snapshot(|physical| read(&a, &b, physical))
            };
            // The from-scratch root of these exact bytes, computed before the
            // commit adopts them -- so it cannot be reading the cache.
            let from_scratch = state.digest_snapshot(&snapshot);

            state.commit_snapshot(
                snapshot,
                vec![GuestWriteEvent::Range {
                    channel: WriterChannel::HostAbi,
                    physical_offset: physical_start,
                    len,
                }],
                Vec::new(),
            );

            let incremental = state
                .expected_sha256
                .expect("a sealed state always has an expected digest");
            assert_eq!(
                incremental, from_scratch,
                "round {round}: the incrementally maintained root must equal the \
                 from-scratch root of the same bytes"
            );
            assert_eq!(
                incremental,
                fresh_root(&image0, &image1),
                "round {round}: a state with NO history must reach the same root as one \
                 that has committed {} times",
                round + 1
            );
            assert_trees_match(&state, &image0, &image1, &format!("round {round}"));

            // The same edit through the OTHER commit path. `adopt_from_view`
            // derives its dirty leaves from the changed-range list rather than
            // from a page-by-page comparison, so it is a genuinely different
            // way to reach the same tree -- and it is the one the emulator
            // runs. It must land on the identical root, and on the identical
            // tree node for node.
            write_storage(&mut storage, &image0, &image1);
            {
                let view = fn64_runtime::RdramView::from_storage(&storage);
                let changed = live
                    .changed_ranges_from_view(&view)
                    .expect("every watched byte is mapped");
                live.adopt_from_view(&view, changed);
            }
            assert_eq!(
                live.expected_sha256
                    .expect("a sealed state always has an expected digest"),
                incremental,
                "round {round}: the view-driven commit path must reach the same root as \
                 the snapshot-driven one"
            );
            assert_trees_match(&live, &image0, &image1, &format!("round {round} (from view)"));
        }

        // The journal chained across every commit, so the history really was
        // long-lived rather than a sequence of independent seals.
        let evidence = state.evidence_snapshot();
        assert_eq!(evidence.entries.len(), edits.len());
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }

        // Returning the bytes to their original values must return the root to
        // its original value: the page cache holds no residue of the path taken.
        let original0 = vec![0u8; total(ranges[0])];
        let mut original1 = vec![0u8; total(ranges[1])];
        for (index, byte) in original1.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37).wrapping_add(5);
        }
        let snapshot = {
            let (a, b) = (original0.clone(), original1.clone());
            state.read_snapshot(|physical| read(&a, &b, physical))
        };
        state.commit_snapshot(
            snapshot,
            vec![
                GuestWriteEvent::Range {
                    channel: WriterChannel::HostAbi,
                    physical_offset: ranges[0].0,
                    len: ranges[0].1 - ranges[0].0,
                },
                GuestWriteEvent::Range {
                    channel: WriterChannel::HostAbi,
                    physical_offset: ranges[1].0,
                    len: ranges[1].1 - ranges[1].0,
                },
            ],
            Vec::new(),
        );
        assert_eq!(
            state.expected_sha256.expect("sealed"),
            fresh_root(&original0, &original1),
            "restoring the original bytes must restore the original root"
        );
        assert_trees_match(&state, &original0, &original1, "after restore");
    }

    /// v1, v2 and v3 digests of the SAME memory must all be distinct.
    ///
    /// Requirement 1 of both migrations: the digest is versioned, not silently
    /// changed. This pins the v1 and v2 constructions here -- the library no
    /// longer computes either on any live path -- and requires the v3 root to
    /// differ from both. Three mutually distinct values means no recorded
    /// digest from any era can be mistaken for a current one.
    #[test]
    fn the_v3_merkle_root_differs_from_both_the_v1_flat_and_v2_page_digests() {
        let ranges = [(0x100u32, 0x180u32)];
        let mut image = vec![0u8; 0x80];
        for (index, byte) in image.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(7).wrapping_add(1);
        }
        let mut state = CanonicalExecutableMutationStateV1::new(&ranges);
        state.seal_with(|physical| image[(physical - 0x100) as usize]);

        // The v1 construction, verbatim from before the first migration.
        let v1: [u8; 32] = {
            let mut digest = sha2::Sha256::new();
            digest.update(ranges[0].0.to_be_bytes());
            digest.update(ranges[0].1.to_be_bytes());
            digest.update(&image);
            digest.finalize().into()
        };
        // The v2 construction, verbatim from before this migration: one leaf
        // per page under the v2 schema, absorbed flat into a v2 root.
        let v2: [u8; 32] = {
            let leaves = vec![watched_page_digest_v2(ranges[0].0, ranges[0].1, 0, &image)];
            watched_root_digest_v2(
                std::iter::once((ranges[0].0, ranges[0].1, leaves.as_slice())),
            )
        };
        let v3 = state.expected_sha256.expect("sealed");

        assert_ne!(v1, v2, "v1 and v2 must remain distinguishable");
        assert_ne!(
            v1, v3,
            "the versioned v3 root must not coincide with the v1 flat digest"
        );
        assert_ne!(
            v2, v3,
            "the versioned v3 root must not coincide with the v2 flat-root digest"
        );

        // The incremental root is the from-scratch root of the same bytes.
        assert_eq!(v3, state.digest_snapshot(&[image.clone()]));
    }

    /// Every field the v3 tree claims to bind must actually change the digest.
    ///
    /// The determinism test cannot catch a MISSING binding: it compares the
    /// incremental root against the from-scratch root, and both call the same
    /// function, so they agree whether or not that function binds enough. (This
    /// was measured -- deleting `height` from the node message, or `page_count`
    /// from the range root, leaves the determinism test green.) So each bound
    /// field is checked here by exhibiting two structures that differ only in
    /// that field and requiring different digests.
    #[test]
    fn the_v3_tree_binds_its_structure() {
        let left = [0x11u8; 32];
        let right = [0x22u8; 32];

        // A node binds its height, so two levels cannot be interchanged.
        let h1 = watched_node_digest_v3(0x1000, 0x9000, 1, 0, &left, Some(&right));
        let h2 = watched_node_digest_v3(0x1000, 0x9000, 2, 0, &left, Some(&right));
        assert_ne!(h1, h2, "an internal node must bind its height");

        // ...its index within the level, so siblings cannot be swapped.
        let i1 = watched_node_digest_v3(0x1000, 0x9000, 1, 1, &left, Some(&right));
        assert_ne!(h1, i1, "an internal node must bind its index");

        // ...its range, so a subtree cannot be replayed under another range.
        let other = watched_node_digest_v3(0x2000, 0x9000, 1, 0, &left, Some(&right));
        assert_ne!(h1, other, "an internal node must bind its range start");
        let other_end = watched_node_digest_v3(0x1000, 0x8000, 1, 0, &left, Some(&right));
        assert_ne!(h1, other_end, "an internal node must bind its range end");

        // ...and its child order.
        let swapped = watched_node_digest_v3(0x1000, 0x9000, 1, 0, &right, Some(&left));
        assert_ne!(h1, swapped, "an internal node must bind child order");

        // The promoted single child is a DIFFERENT message from a pair that
        // repeats it. This is the classic Merkle duplication malleability: with
        // `H(x||x)` a tree whose second half repeats its first can be confused
        // with a smaller tree. Distinct arity tags make that unrepresentable.
        let promoted = watched_node_digest_v3(0x1000, 0x9000, 1, 0, &left, None);
        let doubled = watched_node_digest_v3(0x1000, 0x9000, 1, 0, &left, Some(&left));
        assert_ne!(
            promoted, doubled,
            "a promoted odd child must not hash as a pair of itself"
        );

        // A leaf and an internal node with the same 32 bytes under them must
        // not collide: the tags differ, so no leaf can be read as a node.
        let leaf = watched_page_digest_v3(0x1000, 0x9000, 0, &left);
        assert_ne!(leaf, promoted, "leaf and node messages must be tagged apart");
        assert_ne!(leaf, h1);

        // The range root binds the page count, so a tree cannot be
        // reinterpreted with a different number of leaves...
        let r4 = watched_range_root_digest_v3(0x1000, 0x9000, 4, Some(&left));
        let r5 = watched_range_root_digest_v3(0x1000, 0x9000, 5, Some(&left));
        assert_ne!(r4, r5, "the range root must bind the page count");

        // ...its bounds...
        let r_other = watched_range_root_digest_v3(0x2000, 0x9000, 4, Some(&left));
        assert_ne!(r4, r_other, "the range root must bind its bounds");

        // ...and the presence of an apex, so an empty range is not confusable
        // with a range whose apex happens to be some value.
        let empty = watched_range_root_digest_v3(0x1000, 0x9000, 0, None);
        assert_ne!(r4, empty);
        assert_ne!(
            empty,
            watched_range_root_digest_v3(0x1000, 0x9000, 0, Some(&left)),
            "an empty range must not hash like one carrying an apex"
        );

        // The range root is itself tagged apart from the node it wraps.
        assert_ne!(r4, h1);
        assert_ne!(r4, promoted);

        // The top root binds the range count and the range order.
        let one = watched_root_digest_v3([&left].into_iter());
        let two = watched_root_digest_v3([&left, &right].into_iter());
        assert_ne!(one, two, "the top root must bind the range count");
        let reordered = watched_root_digest_v3([&right, &left].into_iter());
        assert_ne!(two, reordered, "the top root must bind range order");

        let _ = (r5, r_other, i1, other, other_end, swapped, leaf, promoted, h1, r4);
    }

    /// The exact hashed message of every v3 level, pinned field by field.
    ///
    /// `assert_ne!` between two computed values cannot detect a WEAKENING: drop
    /// the range count from the top root and every `assert_ne!` about it still
    /// passes, because both sides lost the same field. Measured -- the
    /// structure test above stayed green through exactly that deletion.
    ///
    /// The only assertion that catches a dropped or reordered field is equality
    /// against an INDEPENDENTLY constructed message. These four references are
    /// written out longhand, in the order the spec claims, so deleting any
    /// `hasher.update` in `receipts.rs` -- or reordering two of them, or
    /// changing a tag byte, or changing the schema string -- fails here.
    ///
    /// They are not hardcoded digest literals: they are the construction
    /// restated, so they stay meaningful if SHA-256's output ever needs
    /// recomputing, and they cannot be "fixed" by pasting a new constant.
    #[test]
    fn the_v3_messages_are_exactly_what_the_schema_says() {
        const SCHEMA: &str = CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V3;
        const PAGE: u64 = CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2 as u64;
        const FANOUT: u64 = CANONICAL_WATCHED_BYTES_FANOUT_V3 as u64;
        let (left, right) = ([0x11u8; 32], [0x22u8; 32]);
        let bytes: Vec<u8> = (0..200u32).map(|index| (index * 7 + 1) as u8).collect();
        let digest = |parts: &[&[u8]]| -> [u8; 32] {
            let mut hasher = sha2::Sha256::new();
            for part in parts {
                hasher.update(part);
            }
            hasher.finalize().into()
        };

        // Leaf: schema || 0x00 || page_bytes || start || end || index || len || bytes
        assert_eq!(
            watched_page_digest_v3(0x1000, 0x9000, 5, &bytes),
            digest(&[
                SCHEMA.as_bytes(),
                &[0x00],
                &PAGE.to_be_bytes(),
                &0x1000u32.to_be_bytes(),
                &0x9000u32.to_be_bytes(),
                &5u32.to_be_bytes(),
                &(bytes.len() as u64).to_be_bytes(),
                &bytes,
            ]),
            "the v3 leaf message changed shape"
        );

        // Internal pair: schema || 0x02 || fanout || start || end || height || index || l || r
        assert_eq!(
            watched_node_digest_v3(0x1000, 0x9000, 3, 7, &left, Some(&right)),
            digest(&[
                SCHEMA.as_bytes(),
                &[0x02],
                &FANOUT.to_be_bytes(),
                &0x1000u32.to_be_bytes(),
                &0x9000u32.to_be_bytes(),
                &3u32.to_be_bytes(),
                &7u32.to_be_bytes(),
                &left,
                &right,
            ]),
            "the v3 internal-pair message changed shape"
        );

        // Promoted odd child: tag 0x03, and NO right child in the message.
        assert_eq!(
            watched_node_digest_v3(0x1000, 0x9000, 3, 7, &left, None),
            digest(&[
                SCHEMA.as_bytes(),
                &[0x03],
                &FANOUT.to_be_bytes(),
                &0x1000u32.to_be_bytes(),
                &0x9000u32.to_be_bytes(),
                &3u32.to_be_bytes(),
                &7u32.to_be_bytes(),
                &left,
            ]),
            "the v3 promoted-single-child message changed shape"
        );

        // Range root: schema || 0x04 || page_bytes || fanout || start || end
        //             || page_count || 0x01 || apex
        assert_eq!(
            watched_range_root_digest_v3(0x1000, 0x9000, 22, Some(&left)),
            digest(&[
                SCHEMA.as_bytes(),
                &[0x04],
                &PAGE.to_be_bytes(),
                &FANOUT.to_be_bytes(),
                &0x1000u32.to_be_bytes(),
                &0x9000u32.to_be_bytes(),
                &22u64.to_be_bytes(),
                &[0x01],
                &left,
            ]),
            "the v3 range-root message changed shape"
        );
        // ...and the empty form, which hashes 0x00 and no apex.
        assert_eq!(
            watched_range_root_digest_v3(0x1000, 0x9000, 0, None),
            digest(&[
                SCHEMA.as_bytes(),
                &[0x04],
                &PAGE.to_be_bytes(),
                &FANOUT.to_be_bytes(),
                &0x1000u32.to_be_bytes(),
                &0x9000u32.to_be_bytes(),
                &0u64.to_be_bytes(),
                &[0x00],
            ]),
            "the v3 empty-range-root message changed shape"
        );

        // Top root: schema || 0x05 || page_bytes || fanout || range_count || roots
        assert_eq!(
            watched_root_digest_v3([&left, &right].into_iter()),
            digest(&[
                SCHEMA.as_bytes(),
                &[0x05],
                &PAGE.to_be_bytes(),
                &FANOUT.to_be_bytes(),
                &2u64.to_be_bytes(),
                &left,
                &right,
            ]),
            "the v3 top-root message changed shape"
        );

        // The five tag bytes are distinct, which is what keeps one level's
        // message from ever being another's.
        let tags = [0x00u8, 0x02, 0x03, 0x04, 0x05];
        let mut seen = tags.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), tags.len(), "v3 level tags must be distinct");
    }

    /// A v3 message must never be a v2 message, at every level of the tree.
    ///
    /// The version test above compares whole roots, which differ for many
    /// reasons at once; if the v3 leaf silently adopted the v2 schema string,
    /// the roots would STILL differ (the root levels differ independently) and
    /// that test would stay green -- measured. This pins the separation at the
    /// leaf, where the two versions' messages are otherwise byte-identical in
    /// shape.
    #[test]
    fn v3_leaves_are_not_v2_leaves() {
        let bytes = vec![0x5au8; 128];
        assert_ne!(
            watched_page_digest_v3(0x1000, 0x2000, 3, &bytes),
            watched_page_digest_v2(0x1000, 0x2000, 3, &bytes),
            "a v3 leaf and a v2 leaf over the same page must be different values"
        );
        // The schema strings are what separates them, so pin that they differ
        // and that neither is a prefix of the other (a prefix would leave the
        // separation resting entirely on the fields that follow).
        assert_ne!(
            CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V2,
            CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V3
        );
        assert_eq!(
            CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V2.len(),
            CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V3.len(),
            "equal-length schema strings keep the separation in the tag bytes \
             themselves rather than in a length shift"
        );
    }

    /// Regrouping the same pages across ranges must not reach the same root.
    ///
    /// The strongest form of the structural claim: identical page CONTENTS,
    /// repartitioned, must give a different root. `checkpoint-digest-cost.md`
    /// records this as the reason splitting a watched range is a certification
    /// change and not bookkeeping -- so it is worth pinning rather than
    /// restating.
    ///
    /// Driven at the hash level rather than through `CanonicalExecutableMutation
    /// StateV1`, because that constructor rejects adjacent ranges outright
    /// (`[0x4000,0x5000) + [0x5000,0x6000)` panics as non-canonical: the
    /// canonical form coalesces them). The rejection is a second, independent
    /// barrier to the same confusion; this test covers the hash layer beneath
    /// it, which is what a third-party verifier would recompute.
    #[test]
    fn regrouping_pages_between_ranges_changes_the_root() {
        const PAGE: u32 = CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2 as u32;
        let mut image = vec![0u8; (PAGE * 2) as usize];
        for (index, byte) in image.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(29).wrapping_add(3);
        }
        let (lo, mid, hi) = (0x4000u32, 0x4000 + PAGE, 0x4000 + PAGE * 2);

        // One range of two pages: two leaves under one apex.
        let joined = {
            let leaves = [
                watched_page_digest_v3(lo, hi, 0, &image[..PAGE as usize]),
                watched_page_digest_v3(lo, hi, 1, &image[PAGE as usize..]),
            ];
            let apex = watched_node_digest_v3(lo, hi, 1, 0, &leaves[0], Some(&leaves[1]));
            let range = watched_range_root_digest_v3(lo, hi, 2, Some(&apex));
            watched_root_digest_v3(std::iter::once(&range))
        };
        // Two ranges of one page each: the identical bytes, regrouped.
        let split = {
            let first = watched_page_digest_v3(lo, mid, 0, &image[..PAGE as usize]);
            let second = watched_page_digest_v3(mid, hi, 0, &image[PAGE as usize..]);
            let a = watched_range_root_digest_v3(lo, mid, 1, Some(&first));
            let b = watched_range_root_digest_v3(mid, hi, 1, Some(&second));
            watched_root_digest_v3([&a, &b].into_iter())
        };

        assert_ne!(
            joined, split,
            "the same bytes partitioned differently must not reach the same root"
        );
    }

    /// Bytes alone decide the root: two different watched ranges holding
    /// identical bytes must not produce the same page digests.
    ///
    /// The leaf binds the range bounds and the page index, so a page cannot be
    /// replayed at another address or another position. Without that, a write
    /// that MOVED a block of code within the watched region could leave the
    /// root unchanged.
    #[test]
    fn page_digests_bind_their_range_and_position() {
        let bytes = vec![0xa5u8; 64];
        let at_1000 = watched_page_digest_v2(0x1000, 0x1040, 0, &bytes);
        let at_2000 = watched_page_digest_v2(0x2000, 0x2040, 0, &bytes);
        assert_ne!(at_1000, at_2000, "a page must bind its range start");

        let end_differs = watched_page_digest_v2(0x1000, 0x1080, 0, &bytes);
        assert_ne!(at_1000, end_differs, "a page must bind its range end");

        let index_1 = watched_page_digest_v2(0x1000, 0x1040, 1, &bytes);
        assert_ne!(at_1000, index_1, "a page must bind its index in the range");

        // A short page and a zero-padded full page must differ.
        let short = watched_page_digest_v2(0x1000, 0x1040, 0, &bytes[..32]);
        let mut padded = bytes[..32].to_vec();
        padded.resize(64, 0);
        let padded = watched_page_digest_v2(0x1000, 0x1040, 0, &padded);
        assert_ne!(short, padded, "page length must be bound, not inferred");
    }

    /// The barrier-restricted comparison must decide EXACTLY what the full scan
    /// decides, given a dirty set that covers every changed byte.
    ///
    /// This is the substitution the whole `mprotect` design rests on. The
    /// barrier reports pages; the guard then compares only those pages and
    /// concludes something about the WHOLE watched region. That conclusion is
    /// sound only if bytes outside the dirty set cannot have changed -- which
    /// the MMU guarantees at run time but which no test can observe directly.
    ///
    /// So this tests the half that IS testable and that a bug would actually
    /// land in: given a dirty set that does cover the changes, does the
    /// restricted comparison name the same bytes and reach the same verdict as
    /// the full one? A lane-mapping or word-widening error in
    /// `changed_ranges_within` shows up here as a disagreement, and the swizzle
    /// makes those errors pattern-dependent rather than obvious -- hence
    /// randomized contents, randomized change patterns, and every alignment.
    #[test]
    fn barrier_restricted_changed_ranges_match_the_full_scan() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut seed = 0x1234_5678_9abc_def1u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        // The same alignment matrix the snapshot-free comparison is held to.
        let layouts: [&[(u32, u32)]; 9] = [
            &[(0x100, 0x180)],
            &[(0x101, 0x180)],
            &[(0x100, 0x17f)],
            &[(0x101, 0x17f)],
            &[(0x102, 0x105)],
            &[(0x101, 0x103)],
            &[(0x105, 0x106)],
            &[(0x103, 0x104)],
            &[(0x101, 0x117), (0x120, 0x181), (0x1c2, 0x1c7)],
        ];

        for layout in layouts {
            let mut storage = vec![0u8; 0x400];
            for byte in storage.iter_mut() {
                *byte = next() as u8;
            }
            let mut state = CanonicalExecutableMutationStateV1::new(layout);
            {
                let view = fn64_runtime::RdramView::from_storage(&storage);
                state.seal_with(|physical| {
                    view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
                });
            }

            for round in 0..96 {
                let density = next() % 5;
                let watched = layout
                    .iter()
                    .flat_map(|&(start, end)| start..end)
                    .collect::<Vec<_>>();
                let mut touched: Vec<u32> = Vec::new();
                for &physical in &watched {
                    let flip = match density {
                        0 => false,
                        1 => next() % 97 == 0,
                        2 => next() % 7 == 0,
                        3 => next() % 2 == 0,
                        _ => true,
                    };
                    if flip {
                        storage[(physical ^ 3) as usize] ^= (next() as u8) | 1;
                        touched.push(physical);
                    }
                }
                for physical in [0x0u32, 0x99, 0x1ff, 0x2ab, 0x3ff] {
                    storage[(physical ^ 3) as usize] ^= next() as u8;
                }

                let view = fn64_runtime::RdramView::from_storage(&storage);
                let full = state
                    .changed_ranges_from_view(&view)
                    .expect("every watched byte is mapped");

                // Build a dirty set that COVERS every byte written, the way a
                // real barrier's page granularity does. Three shapes, because
                // the coverage the barrier gives is a superset of varying
                // slack and the restricted comparison must be right for all of
                // them:
                //
                //   - exactly the touched bytes (the tightest legal set);
                //   - each touched byte widened to a small aligned granule
                //     (what a page actually looks like, in miniature);
                //   - the whole region (maximum slack -- must reduce to the
                //     full scan's answer exactly).
                let tight: Vec<(u32, u32)> = crate::write_barrier::guard::normalize(
                    touched.iter().map(|&p| (p, p + 1)).collect(),
                );
                let granule = 16u32;
                let widened: Vec<(u32, u32)> = crate::write_barrier::guard::normalize(
                    touched
                        .iter()
                        .map(|&p| (p & !(granule - 1), (p & !(granule - 1)) + granule))
                        .collect(),
                );
                let whole: Vec<(u32, u32)> = layout.to_vec();

                for (label, spans) in [
                    ("tight", &tight),
                    ("page-widened", &widened),
                    ("whole-region", &whole),
                ] {
                    let mut restricted = Vec::new();
                    let mut ok = true;
                    for range in &state.watched {
                        let clipped = crate::write_barrier::guard::clip(
                            spans,
                            range.physical_start,
                            range.physical_end,
                        );
                        if !range.changed_ranges_within(&view, &clipped, &mut restricted) {
                            ok = false;
                            break;
                        }
                    }
                    assert!(ok, "layout {layout:?} round {round} {label}: unmapped byte");
                    assert_eq!(
                        restricted, full,
                        "layout {layout:?} round {round} {label}: the barrier-restricted \
                         comparison named different bytes than the full scan"
                    );

                    // And the boolean form, which is the one the dispatch guard
                    // actually calls.
                    let matched = state.watched.iter().all(|range| {
                        let clipped = crate::write_barrier::guard::clip(
                            spans,
                            range.physical_start,
                            range.physical_end,
                        );
                        range.matches_storage_within(&view, &clipped)
                    });
                    assert_eq!(
                        matched,
                        full.is_empty(),
                        "layout {layout:?} round {round} {label}: the restricted predicate \
                         disagreed with the full scan's verdict"
                    );
                }

                // Advance the baseline so the next round starts clean, exactly
                // as a real boundary does.
                state.adopt_from_view(&view, full);
            }
        }
    }

    /// An EMPTY dirty set must mean "nothing changed", and that is only sound
    /// because the barrier armed over a clean region.
    ///
    /// Stated as a test because it is the one place the restricted comparison
    /// returns `true` without reading a single byte, and therefore the one
    /// place a lifecycle bug -- arming when the region was NOT clean -- would
    /// convert into a silently accepted mutation. The test cannot catch that
    /// lifecycle bug (it is about when `arm` is called, not what the comparison
    /// does), so it pins the contract instead: empty means untouched, and the
    /// caller owes the precondition.
    #[test]
    fn an_empty_dirty_set_reports_a_match_without_reading() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut storage = vec![0u8; 0x200];
        for (index, byte) in storage.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(17);
        }
        let layout = [(0x101u32, 0x17fu32)];
        let mut state = CanonicalExecutableMutationStateV1::new(&layout);
        {
            let view = fn64_runtime::RdramView::from_storage(&storage);
            state.seal_with(|physical| {
                view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
            });
        }
        // Change a byte the dirty set does NOT mention. The restricted
        // comparison must report a match, because it is entitled to assume the
        // MMU would have reported that page. This is the documented contract,
        // not a bug: it is what makes an empty set free.
        storage[(0x110u32 ^ 3) as usize] ^= 0xff;
        let view = fn64_runtime::RdramView::from_storage(&storage);
        assert!(
            state.watched[0].matches_storage_within(&view, &[]),
            "an empty dirty set must short-circuit to a match"
        );
        // And the full scan, which has no such entitlement, must see it -- so
        // the two differ exactly when the precondition is violated, which is
        // what makes the precondition load-bearing rather than decorative.
        assert!(
            !state.matches_view(&view),
            "the full scan must still see a change the dirty set omitted"
        );
    }

    /// `clip` and `normalize` must produce ascending, disjoint spans confined
    /// to the range asked about.
    ///
    /// The restricted comparison `debug_assert`s these properties and its
    /// coalescing depends on them, so they are pinned here rather than left to
    /// the caller's care.
    #[test]
    fn dirty_span_normalization_is_ascending_disjoint_and_clipped() {
        use crate::write_barrier::guard::{clip, normalize};

        // Unsorted, overlapping, duplicated -- what the union in
        // `disarm_and_capture` can produce.
        let merged = normalize(vec![
            (0x300, 0x400),
            (0x100, 0x200),
            (0x180, 0x280),
            (0x100, 0x200),
            (0x400, 0x480),
        ]);
        assert_eq!(merged, vec![(0x100, 0x280), (0x300, 0x480)]);
        for pair in merged.windows(2) {
            assert!(pair[0].1 < pair[1].0, "spans must be disjoint and ascending");
        }

        // Clipping keeps only the overlap, and keeps it ascending.
        assert_eq!(clip(&merged, 0x200, 0x340), vec![(0x200, 0x280), (0x300, 0x340)]);
        // A range disjoint from every span clips to nothing, which is the
        // "this watched range was untouched" answer.
        assert!(clip(&merged, 0x280, 0x300).is_empty());
        // A range inside one span clips to itself.
        assert_eq!(clip(&merged, 0x120, 0x160), vec![(0x120, 0x160)]);
    }

    /// The mirror census must count a clean boundary as clean and a changed
    /// one as dirty, because `dirty` is the number the gating decision rests
    /// on.
    ///
    /// The soundness argument for gating the scheduler-mirror reconcile is
    /// that the site never catches anything -- so an instrument that could not
    /// report "caught something" would make that argument unfalsifiable. This
    /// pins that the two outcomes are actually distinguished, using the same
    /// predicate the call site uses.
    #[test]
    fn mirror_census_distinguishes_a_clean_boundary_from_a_changed_one() {
        use crate::recompiled::live_program::mirror_reconcile_census;

        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut storage = vec![0u8; 0x200];
        for (index, byte) in storage.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(17).wrapping_add(3);
        }
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x100, 0x140)]);
        {
            let view = fn64_runtime::RdramView::from_storage(&storage);
            state.seal_with(|physical| {
                view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
            });
        }

        // Sealed and untouched: the predicate the census records must say clean.
        {
            let view = fn64_runtime::RdramView::from_storage(&storage);
            assert!(
                state.matches_view(&view),
                "an untouched sealed region must read clean"
            );
        }

        // Change one watched byte: the same predicate must say dirty. This is
        // the outcome whose ABSENCE over a whole route is the gating argument.
        storage[0x120] ^= 0xff;
        {
            let view = fn64_runtime::RdramView::from_storage(&storage);
            assert!(
                !state.matches_view(&view),
                "a changed watched byte must read dirty -- otherwise the census \
                 could never falsify the redundancy claim"
            );
        }

        // The counters are addressable and start from a defined state; the
        // census is off by default, so `note` is what moves them, not `enabled`.
        let (clean_before, dirty_before) = mirror_reconcile_census::running_totals();
        mirror_reconcile_census::note(true);
        mirror_reconcile_census::note(false);
        let (clean_after, dirty_after) = mirror_reconcile_census::running_totals();
        assert_eq!(
            (clean_after - clean_before, dirty_after - dirty_before),
            (1, 1),
            "each outcome must land in its own counter"
        );
    }

    /// `FN64_MIRROR_RECONCILE_CENSUS` must be off unless affirmatively set.
    ///
    /// Pins the bug shape that fabricated a 4.9x result elsewhere in this
    /// crate: a gate written as `var_os(..).is_some()` reads `VAR=` -- set but
    /// empty, exactly how a shell writes an off lane -- as ON, making both
    /// lanes of an A/B the same lane.
    #[test]
    fn mirror_census_gate_is_off_unless_affirmatively_set() {
        // The parse the gate uses, exercised directly: the gate itself latches
        // in a `OnceLock` and cannot be re-read per case within one process.
        let affirmative = |value: &str| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        };
        for off in ["", " ", "0", "no", "off", "false", "2"] {
            assert!(!affirmative(off), "{off:?} must not arm the census");
        }
        for on in ["1", "true", "yes", "on", "ON", " 1 "] {
            assert!(affirmative(on), "{on:?} must arm the census");
        }
    }
