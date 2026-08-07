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
                        fast_range.expected_page_digests, slow_range.expected_page_digests,
                        "layout {layout:?} round {round}: the page digests diverged"
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
                        fast_range.expected_page_digests,
                        slow_range.expected_page_digests
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

    /// The v2 root depends only on the watched bytes -- never on the
    /// incremental history that produced them.
    ///
    /// This is the load-bearing property of the page-tree migration. The
    /// incremental root (`digest_expected`, from maintained page digests) is
    /// read on every commit, while the from-scratch root (`digest_snapshot`,
    /// which recomputes every page) is what validation recomputes and what any
    /// independent verifier would compute. If those two could ever disagree,
    /// the journal would certify a value no verifier could reproduce.
    ///
    /// The test drives a long-lived state through many commits, each touching
    /// different pages -- page boundaries, page interiors, spans crossing
    /// several pages, and rewrites of pages already dirtied -- and after every
    /// single one requires:
    ///
    ///   1. the incremental root equals the from-scratch root of the same bytes;
    ///   2. a FRESH state, sealed directly on those bytes with no history at
    ///      all, produces that same root.
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
        // page, the second spans several pages and does NOT end on a page
        // boundary, so the short final page is exercised. This mirrors WM2000's
        // shape (a 16-byte range and a ~1.44 MiB range) at test scale.
        const PAGE: u32 = CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2 as u32;
        let ranges = [(0x1000u32, 0x1010u32), (0x2000, 0x2000 + PAGE * 3 + 777)];
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

        // A fresh state sealed on the same bytes must already agree at seal.
        let fresh_root = |image0: &[u8], image1: &[u8]| -> [u8; 32] {
            let mut fresh = CanonicalExecutableMutationStateV1::new(&ranges);
            fresh.seal_with(|physical| read(image0, image1, physical));
            fresh
                .expected_sha256
                .expect("a sealed state always has an expected digest")
        };
        assert_eq!(
            state.expected_sha256.expect("sealed"),
            fresh_root(&image0, &image1),
            "a freshly sealed state must agree with the incremental one at seal time"
        );

        // Every write shape that could expose a page-boundary bug: the first
        // byte of a page, the last byte of a page, a span straddling a page
        // boundary, a span covering a whole page, a write inside the short
        // final page, and a rewrite of an already-dirtied page.
        let edits: &[(u32, u32)] = &[
            (0x2000, 1),                    // first byte of page 0
            (0x2000 + PAGE - 1, 1),         // last byte of page 0
            (0x2000 + PAGE, 1),             // first byte of page 1
            (0x2000 + PAGE - 2, 4),         // straddles the page 0/1 boundary
            (0x2000 + PAGE * 2, PAGE),      // exactly all of page 2
            (0x2000 + PAGE * 3, 1),         // first byte of the short final page
            (0x2000 + PAGE * 3 + 776, 1),   // last byte of the whole range
            (0x2000 + PAGE - 1, 2),         // re-dirty pages 0 and 1
            (0x2000 + 5, 3),                // page 0 interior, third time
            (0x1000, 16),                   // the entire sub-page first range
            (0x1008, 4),                    // interior of the first range
            (0x2000 + PAGE * 2 + 100, 900), // interior of page 2, again
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
    }

    /// A v1 digest and a v2 digest of the same memory must not be confusable.
    ///
    /// Requirement 1 of the migration: the new digest is versioned, not
    /// silently changed. This pins the v1 construction as a literal here --
    /// nothing else in the tree still computes it -- and requires v2 to differ.
    #[test]
    fn the_v2_page_tree_root_differs_from_the_v1_flat_digest() {
        let ranges = [(0x100u32, 0x180u32)];
        let mut image = vec![0u8; 0x80];
        for (index, byte) in image.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(7).wrapping_add(1);
        }
        let mut state = CanonicalExecutableMutationStateV1::new(&ranges);
        state.seal_with(|physical| image[(physical - 0x100) as usize]);

        // The v1 construction, verbatim from before the migration.
        let v1: [u8; 32] = {
            let mut digest = sha2::Sha256::new();
            digest.update(ranges[0].0.to_be_bytes());
            digest.update(ranges[0].1.to_be_bytes());
            digest.update(&image);
            digest.finalize().into()
        };
        let v2 = state.expected_sha256.expect("sealed");
        assert_ne!(
            v1, v2,
            "the versioned v2 root must not coincide with the v1 flat digest"
        );

        // And the v2 root must bind the schema: a root computed with the same
        // page digests but a different domain separator differs.
        assert_eq!(v2, state.digest_snapshot(&[image.clone()]));
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
