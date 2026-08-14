use super::*;

    #[test]
    fn second_raw_full_sync_rejects_before_renderer_or_rdram_mutation() {
        const FIRST: usize = 0x100;
        const SECOND: usize = 0x108;
        const MUTATION: usize = 0x400;

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        rdram[FIRST..FIRST + 4].copy_from_slice(&0xe900_0000u32.to_ne_bytes());
        rdram[SECOND..SECOND + 4].copy_from_slice(&0xe900_0000u32.to_ne_bytes());
        let calls = Rc::new(Cell::new(0));
        set_render_backend(
            Box::new(MutatingRawBackend {
                calls: Rc::clone(&calls),
                outcome: RawMutationOutcome::Complete,
                mutation_offset: MUTATION,
            }),
            rdram.len(),
        );

        unsafe { dispatch_raw_rdp(rdram.as_mut_ptr(), FIRST as u32, SECOND as u32) };
        assert_eq!(calls.get(), 1);
        let before_rdram = rdram.clone();
        let before_device = with_host(|host| host.device_fabric.snapshot());
        let before_observations = copy_rsp_rdp_observations();

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_raw_rdp(rdram.as_mut_ptr(), SECOND as u32, (SECOND + 8) as u32)
        }))
        .expect_err("a second unserviced raw FullSync must remain loud");

        assert!(panic_message(rejected.as_ref()).contains("graphics task start while DP is busy"));
        assert_eq!(
            calls.get(),
            1,
            "the occupied DP slot must reject before renderer entry"
        );
        assert_eq!(rdram, before_rdram);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot()),
            before_device
        );
        assert_eq!(copy_rsp_rdp_observations(), before_observations);
    }


    /// The rename is only honest if the retired spelling is LOUD: an unset var
    /// means "feature off", so a silently-ignored `OOT_*` name would let a
    /// stale invocation look like a clean run while measuring the wrong thing.
    /// Every retired name must reach a message naming its replacement -- that
    /// string is the whole trap, and a typo'd table entry would gut it.
    #[test]
    fn every_legacy_env_var_names_its_replacement() {
        for (old, new) in RENAMED_ENV_VARS {
            assert!(old.starts_with("OOT_") && new.starts_with("FN64_"));
            let message = legacy_env_var_message(old, new);
            assert!(
                message.contains(new),
                "the trap for {old} must name {new}, or the operator cannot act on it"
            );
        }
    }


    #[test]
    fn os_sp_task_load_admits_complete_header_and_rspboot_to_persistent_rsp_memory() {
        const TASK_OFFSET: usize = 0x100;
        const BOOT_OFFSET: usize = 0x200;
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let mut rdram = vec![0u8; 0x400];
        let header = OsTaskHeader {
            task_type: fn64_runtime::M_GFXTASK,
            flags: 0x1122_3344,
            ucode_boot: 0x8000_0000 | BOOT_OFFSET as u32,
            ucode_boot_size: 13,
            ucode: 0x3456,
            ucode_size: 0x789A,
            ucode_data: 0xBCDE,
            ucode_data_size: 0x20,
            dram_stack: 0x1234,
            dram_stack_size: 0x40,
            output_buff: 0x5678,
            output_buff_size: 0x9ABC,
            data_ptr: 0xDEF0,
            data_size: 0x80,
            yield_data_ptr: 0x1357,
            yield_data_size: 0x2468,
        };
        let words = [
            header.task_type,
            header.flags,
            header.ucode_boot,
            header.ucode_boot_size,
            header.ucode,
            header.ucode_size,
            header.ucode_data,
            header.ucode_data_size,
            header.dram_stack,
            header.dram_stack_size,
            header.output_buff,
            header.output_buff_size,
            header.data_ptr,
            header.data_size,
            header.yield_data_ptr,
            header.yield_data_size,
        ];
        for (index, word) in words.into_iter().enumerate() {
            let start = TASK_OFFSET + index * 4;
            rdram[start..start + 4].copy_from_slice(&word.to_ne_bytes());
        }
        let boot = (0..16).map(|value| 0xA0 + value).collect::<Vec<u8>>();
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, byte) in boot.iter().copied().enumerate() {
                view.write_u8(RdramAddr::from_offset((BOOT_OFFSET + index) as u32), byte);
            }
        }
        let prior_count = with_executor(|exec| exec.task_log().submissions().len());
        crate::set_trace_enabled(true);
        let prior_starts = crate::copy_trace()
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    fn64_runtime::TraceKind::TaskSubmit {
                        task_kind: fn64_runtime::TaskKind::Graphics,
                        ucode,
                    } if ucode == header.ucode
                )
            })
            .count();
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + TASK_OFFSET as u64;
        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };

        with_host(|host| {
            let rsp = host.device_fabric.rsp_memory();
            assert_eq!(
                rsp.read_bytes(fn64_runtime::RspMemAddr::from_register(0x1000), 16)
                    .unwrap(),
                boot
            );
            let task = rsp
                .read_bytes(fn64_runtime::RspMemAddr::from_register(0x0FC0), 64)
                .unwrap();
            assert_eq!(&task[0..4], &header.task_type.to_be_bytes());
            assert_eq!(&task[8..12], &header.ucode_boot.to_be_bytes());
            assert_eq!(&task[60..64], &header.yield_data_size.to_be_bytes());
            assert_eq!(rsp.imem_generation(), 1);
        });
        assert_eq!(
            crate::pi::read_live_device_mmio(0xFFFF_FFFF_A408_0000),
            Some(0)
        );
        with_executor(|exec| {
            assert_eq!(exec.task_log().submissions().len(), prior_count + 1);
            assert_eq!(exec.task_log().submissions().last(), Some(&header));
        });
        assert_eq!(
            crate::copy_trace()
                .iter()
                .filter(|event| {
                    matches!(
                        event.kind,
                        fn64_runtime::TraceKind::TaskSubmit {
                            task_kind: fn64_runtime::TaskKind::Graphics,
                            ucode,
                        } if ucode == header.ucode
                    )
                })
                .count(),
            prior_starts,
            "osSpTaskLoad admission cannot emit the StartGo-qualified TaskSubmit trace"
        );
    }


    #[test]
    fn repeated_task_load_uses_the_cpu_cached_rspboot_image_after_rsp_dma_writes() {
        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        with_host(|host| host.rsp_boot_images.clear());
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        let boot_off = u32::from_ne_bytes(rdram[HEADER + 8..HEADER + 12].try_into().unwrap());
        let original = with_host(|host| {
            host.device_fabric
                .rsp_memory()
                .read_bytes(fn64_runtime::RspMemAddr::from_register(0x1000), 8)
                .unwrap()
        });

        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_logical_bytes(RdramAddr::from_offset(boot_off), &[0; 8]);
        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };

        let mut physical = [0xff; 8];
        fn64_runtime::RdramView::from_storage(&rdram)
            .copy_logical_bytes(RdramAddr::from_offset(boot_off), &mut physical);
        assert_eq!(physical, [0; 8], "the RSP's physical write stays visible");
        with_host(|host| {
            assert_eq!(
                host.device_fabric
                    .rsp_memory()
                    .read_bytes(fn64_runtime::RspMemAddr::from_register(0x1000), 8)
                    .unwrap(),
                original,
                "osSpTaskLoad must re-DMA the CPU-cached boot text"
            );
        });
    }


    #[test]
    fn hle_rspboot_commits_overlay_and_stops_before_executing_loaded_ucode() {
        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        let task_addr = RdramAddr::from_offset(HEADER as u32);
        let loaded = take_loaded_rsp_task(task_addr);
        retain_started_rsp_task_lineage(loaded, None);
        let ucode_off = u32::from_ne_bytes(rdram[HEADER + 0x10..HEADER + 0x14].try_into().unwrap());
        for (index, word) in [0x2405_5678u32, 0xac05_0100].into_iter().enumerate() {
            let offset = ucode_off as usize + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        let generation_before = with_host(|host| host.device_fabric.rsp_memory().imem_generation());

        let boot = unsafe { dispatch_hle_rspboot(rdram.as_mut_ptr(), task_addr) };

        assert_eq!(boot.steps, 7);
        assert_eq!(boot.task.task_type, fn64_runtime::M_GFXTASK);
        with_host(|host| {
            let fabric = &host.device_fabric;
            assert_eq!(fabric.sp_pc(), 0x80);
            assert_eq!(fabric.rsp_memory().imem_generation(), generation_before + 1);
            assert_eq!(
                fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x100,
                    ))
                    .unwrap(),
                0,
                "the first loaded-ucode instruction must remain behind the HLE boundary"
            );
            assert_eq!(
                fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Imem,
                        0x80,
                    ))
                    .unwrap(),
                0x2405_5678
            );
        });
    }


    #[test]
    fn hle_rspboot_traps_if_boot_breaks_before_loading_ucode() {
        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        let task_addr = RdramAddr::from_offset(HEADER as u32);
        let loaded = take_loaded_rsp_task(task_addr);
        retain_started_rsp_task_lineage(loaded, None);
        with_host(|host| {
            host.device_fabric
                .rsp_memory_mut()
                .write_word(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    0x0000_000d,
                )
                .unwrap();
        });

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_hle_rspboot(rdram.as_mut_ptr(), task_addr)
        }))
        .expect_err("rspboot BREAK before ucode must trap");
        assert!(panic_message(panic.as_ref())
            .contains("RSP HLE rspboot reached BREAK before entering DMA-loaded ucode"));
    }


    #[test]
    fn direct_imem_shape_requires_the_admitted_boot_copy_to_cover_the_ucode() {
        let direct = OsTaskHeader {
            ucode_boot: 0x8000_0200,
            ucode_boot_size: 0x1000,
            ucode: 0xA000_0200,
            ucode_size: 0x1000,
            ..Default::default()
        };
        assert_eq!(
            admitted_task_image_shape(&direct),
            AdmittedTaskImageShape::DirectImem
        );
        assert_eq!(
            admitted_task_image_shape(&OsTaskHeader {
                ucode_boot_size: 8,
                ucode_size: 16,
                ..direct
            }),
            AdmittedTaskImageShape::BootOverlay,
            "equal pointers alone must not bypass rspboot when the admitted copy is incomplete"
        );
    }


    #[test]
    fn direct_imem_rejects_prior_inflight_owner_before_backend_entry() {
        const TASK: u32 = 0x40;
        const IMAGE: u32 = 0x100;
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x200];
        prepare_renderer_rdram(&mut rdram);
        let calls = std::rc::Rc::new(Cell::new(0));
        set_render_backend(
            Box::new(CountingPanicRenderBackend(calls.clone())),
            rdram.len(),
        );
        install_running_task_lineage(
            RdramAddr::from_offset(TASK),
            RspTaskAdmissionGeneration::new(NonZeroU64::new(2).unwrap()),
        );
        with_host(|host| {
            host.rsp_interpreter_state = RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: RspInterpreterOwner::task(0x180, RspTaskAdmissionGeneration::first()),
            };
        });

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state =
                unsafe { begin_direct_hle_phase(rdram.as_mut_ptr(), RdramAddr::from_offset(TASK)) };
            let _ = unsafe {
                dispatch_gfx_task_chunk(
                    rdram.as_mut_ptr(),
                    &direct_imem_test_header(IMAGE),
                    fn64_render::RenderTaskStep::Start,
                    0,
                )
            };
        }));

        assert!(rejected.is_err());
        assert_eq!(calls.get(), 0, "backend ran before owner admission");
        assert_eq!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: RspInterpreterOwner::task(0x180, RspTaskAdmissionGeneration::first()),
            }
        );
    }


    #[test]
    fn direct_imem_backend_panic_leaves_same_task_inflight() {
        const TASK: u32 = 0x40;
        const IMAGE: u32 = 0x100;
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x200];
        prepare_renderer_rdram(&mut rdram);
        let calls = std::rc::Rc::new(Cell::new(0));
        set_render_backend(
            Box::new(CountingPanicRenderBackend(calls.clone())),
            rdram.len(),
        );
        install_running_task_lineage(
            RdramAddr::from_offset(TASK),
            RspTaskAdmissionGeneration::first(),
        );

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state =
                unsafe { begin_direct_hle_phase(rdram.as_mut_ptr(), RdramAddr::from_offset(TASK)) };
            let _ = unsafe {
                dispatch_gfx_task_chunk(
                    rdram.as_mut_ptr(),
                    &direct_imem_test_header(IMAGE),
                    fn64_render::RenderTaskStep::Start,
                    0,
                )
            };
        }));

        assert!(rejected.is_err());
        assert_eq!(calls.get(), 1);
        assert_eq!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: RspInterpreterOwner::task(TASK, RspTaskAdmissionGeneration::first()),
            }
        );
    }


    #[test]
    fn direct_imem_resume_reclaims_same_suspended_owner() {
        const TASK: u32 = 0x40;
        with_host(|host| {
            *host = HostState::default();
            host.rsp_interpreter_state =
                RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable {
                    owner: RspInterpreterOwner::task(TASK, RspTaskAdmissionGeneration::first()),
                };
        });
        install_running_task_lineage(
            RdramAddr::from_offset(TASK),
            RspTaskAdmissionGeneration::new(NonZeroU64::new(2).unwrap()),
        );

        resume_direct_hle_phase(RdramAddr::from_offset(TASK));

        assert_eq!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: RspInterpreterOwner::task(
                    TASK,
                    RspTaskAdmissionGeneration::new(NonZeroU64::new(2).unwrap(),)
                ),
            }
        );
    }


    #[test]
    fn direct_imem_graphics_task_starts_lle_at_pc_zero_without_rspboot_overlay() {
        const HEADER: usize = 0x40;
        const IMAGE: usize = 0x100;
        const DATA: u32 = 0x181;
        const MUTATED_DATA: u32 = 0x1d1;
        const INITIAL_DATA: [u8; 5] = [0x01, 0x23, 0x45, 0x67, 0x89];
        const START_DATA: [u8; 5] = [0xfe, 0xdc, 0xba, 0x98, 0x76];
        const MUTATED_DATA_BYTES: [u8; 3] = [0xaa, 0xbb, 0xcc];
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x220];
        for (field, value) in [
            (0x00, fn64_runtime::M_GFXTASK),
            (0x08, 0x8000_0000 | IMAGE as u32),
            (0x0c, 12),
            (0x10, 0xA000_0000 | IMAGE as u32),
            (0x14, 12),
            (0x18, 0xA000_0000 | DATA),
            (0x1c, INITIAL_DATA.len() as u32),
        ] {
            rdram[HEADER + field..HEADER + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        for (index, word) in [0x2408_1234u32, 0xac08_0100, 0x0000_000d]
            .into_iter()
            .enumerate()
        {
            let offset = IMAGE + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in INITIAL_DATA.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(DATA + offset as u32), byte);
            }
        }
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        prepare_renderer_rdram(&mut rdram);
        set_render_backend_with_policy(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            rdram.len(),
            GraphicsTaskExecutionPolicy::LleAccuracy,
        );
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        let admitted_header = unsafe { read_os_task_header(rdram.as_mut_ptr(), HEADER) };
        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };
        let admitted_snapshot = crate::host_evidence_snapshot();
        let admission_generation = admitted_snapshot
            .loaded_rsp_task
            .expect("loaded task evidence")
            .admission_generation;
        assert_eq!(
            admitted_snapshot.loaded_rsp_task,
            Some(LoadedRspTaskEvidenceSnapshot {
                task_offset: HEADER as u32,
                admission_generation,
                header: admitted_header,
                resumed_data_identity: None,
            })
        );
        assert_eq!(
            admitted_snapshot.next_rsp_task_admission_generation,
            admission_generation + 1
        );
        // StartGo hashes current bytes at the address/size admitted by Load.
        // Mutating the CPU header to a second source must not change that
        // admitted source, while mutation of source A's bytes remains visible.
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in START_DATA.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(DATA + offset as u32), byte);
            }
            for (offset, byte) in MUTATED_DATA_BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(MUTATED_DATA + offset as u32), byte);
            }
        }
        rdram[HEADER + 0x18..HEADER + 0x1c]
            .copy_from_slice(&(0xA000_0000 | MUTATED_DATA).to_ne_bytes());
        rdram[HEADER + 0x1c..HEADER + 0x20]
            .copy_from_slice(&(MUTATED_DATA_BYTES.len() as u32).to_ne_bytes());
        let expected_at = Cycles::new(sim_time());
        let (imem_generation, expected_digest) = with_host(|host| {
            let memory = host.device_fabric.rsp_memory();
            (
                memory.imem_generation(),
                imem_sha256(memory.bank(fn64_runtime::RspMemoryBank::Imem)),
            )
        });
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        let host_evidence = crate::host_evidence_snapshot();
        assert_eq!(host_evidence.loaded_rsp_task, None);
        assert!(
            host_evidence.rsp_task_lineages.is_empty(),
            "a synchronous normal completion must retire its Running lineage"
        );
        assert!(matches!(
            host_evidence.rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::Exact(_)
        ));

        with_host(|host| {
            let fabric = &host.device_fabric;
            assert_eq!(
                fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x100,
                    ))
                    .unwrap(),
                0x0000_1234,
                "direct-image LLE must execute the instruction at admitted IMEM PC zero"
            );
            assert!(fabric.snapshot().sp_busy);
        });
        assert_eq!(
            copy_rsp_rdp_observations(),
            vec![RspRdpObservationEvent {
                at: expected_at,
                kind: RspRdpObservationKind::MicrocodeRecognition {
                    task_addr: RdramAddr::from_offset(HEADER as u32),
                    imem_generation,
                    text_sha256: expected_digest,
                    data_addr: RdramAddr::from_offset(DATA),
                    data_size: START_DATA.len() as u32,
                    data_sha256: Sha256::digest(START_DATA).into(),
                    family: None,
                },
            }]
        );
        crate::advance_virtual_time(3);
    }


    #[test]
    fn direct_imem_hle_needs_lle_replays_untouched_pc_zero_entry_through_public_task_path() {
        const HEADER: usize = 0x40;
        const IMAGE: usize = 0x100;
        const DATA: u32 = 0x180;
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x200];
        for (field, value) in [
            (0x00, fn64_runtime::M_GFXTASK),
            (0x08, 0x8000_0000 | IMAGE as u32),
            (0x0c, 12),
            (0x10, 0xa000_0000 | IMAGE as u32),
            (0x14, 12),
            (0x18, 0x8000_0000 | DATA),
            (0x1c, 4),
        ] {
            rdram[HEADER + field..HEADER + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        for (index, word) in [0x2408_1234u32, 0xac08_0100, 0x0000_000d]
            .into_iter()
            .enumerate()
        {
            let offset = IMAGE + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        prepare_renderer_rdram(&mut rdram);
        set_render_backend_with_policy(
            Box::new(StatusRenderBackend(FrameStatus::NeedsLle {
                ucode_sha256: [0x42; 32],
            })),
            rdram.len(),
            GraphicsTaskExecutionPolicy::HleOptimized,
        );
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;

        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        let evidence = crate::host_evidence_snapshot();
        let RspInterpreterStateEvidenceSnapshot::Exact(state) = evidence.rsp_interpreter_state
        else {
            panic!("direct-IMEM NeedsLle fallback did not publish exact terminal state")
        };
        assert_eq!(state.gprs()[8], 0x1234);
        assert!(evidence.loaded_rsp_task.is_none());
        assert!(evidence.rsp_task_lineages.is_empty());
        with_host(|host| {
            assert_eq!(
                host.device_fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x100,
                    ))
                    .unwrap(),
                0x0000_1234
            );
        });
    }


    #[test]
    fn yielded_resume_reuses_typed_original_data_lineage_until_rom_reset() {
        const TASK: u32 = 0x40;
        const ORIGINAL: u32 = 0x101;
        const YIELD: u32 = 0x181;
        const ORIGINAL_BYTES: [u8; 5] = [0x11, 0x22, 0x33, 0x44, 0x55];
        const YIELD_BYTES: [u8; 5] = [0xaa, 0xbb, 0xcc, 0xdd, 0xee];

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x200];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in ORIGINAL_BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(ORIGINAL + offset as u32), byte);
            }
            for (offset, byte) in YIELD_BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(YIELD + offset as u32), byte);
            }
        }
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        let task_addr = RdramAddr::from_offset(TASK);
        let initial_header = OsTaskHeader {
            task_type: M_GFXTASK,
            ucode_data: 0x8000_0000 | ORIGINAL,
            ucode_data_size: ORIGINAL_BYTES.len() as u32,
            yield_data_ptr: 0xA000_0000 | YIELD,
            yield_data_size: YIELD_BYTES.len() as u32,
            ..Default::default()
        };
        let original = unsafe {
            task_microcode_data_identity(
                rdram.as_mut_ptr(),
                task_addr,
                initial_header.ucode_data,
                initial_header.ucode_data_size,
            )
        };
        with_host(|host| {
            host.rsp_task_lineages.insert(
                task_addr.offset(),
                RspTaskLineage {
                    admission_generation: RspTaskAdmissionGeneration::first(),
                    original_header: initial_header,
                    data_identity: Some(original),
                    phase: RspTaskLineagePhase::ResumeAuthorized,
                },
            );
            host.next_rsp_task_admission_generation =
                RspTaskAdmissionGeneration(NonZeroU64::new(2).unwrap());
        });

        let resumed_header = OsTaskHeader {
            flags: fn64_runtime::OS_TASK_YIELDED,
            ucode_data: initial_header.yield_data_ptr,
            ucode_data_size: initial_header.yield_data_size,
            ..initial_header
        };
        let resumed = loaded_rsp_task_from_header(task_addr, resumed_header);
        assert_eq!(resumed.resumed_data_identity, Some(original));
        assert_eq!(
            crate::host_evidence_snapshot().rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized
        );
        let yield_sha256: [u8; 32] = Sha256::digest(YIELD_BYTES).into();
        assert_ne!(
            resumed
                .resumed_data_identity
                .expect("yielded load retains data identity")
                .sha256,
            yield_sha256
        );

        retain_loaded_rsp_task(resumed);
        let resumed_load = crate::host_evidence_snapshot();
        assert_eq!(
            resumed_load.rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeLoaded
        );
        assert_eq!(resumed_load.rsp_task_lineages[0].admission_generation, 2);
        assert_eq!(
            resumed_load
                .loaded_rsp_task
                .expect("yielded reload token")
                .admission_generation,
            2
        );
        assert_eq!(resumed_load.next_rsp_task_admission_generation, 3);
        let replay =
            std::panic::catch_unwind(|| loaded_rsp_task_from_header(task_addr, resumed_header))
                .unwrap_err();
        let replay_message = panic_message(replay.as_ref());
        assert!(replay_message.contains("has no unused resume authorization"));

        let loaded = take_loaded_rsp_task(task_addr);
        retain_started_rsp_task_lineage(loaded, Some(original));
        assert_eq!(
            crate::host_evidence_snapshot().rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::Running
        );

        crate::load_rom(Vec::new());
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loaded_rsp_task_from_header(task_addr, resumed_header)
        }))
        .unwrap_err();
        let message = panic_message(panic.as_ref());
        assert!(message.contains("yielded RSP task 0x00000040 has no retained task lineage"));
    }


    #[test]
    fn rom_reset_invalidates_unconsumed_loaded_task_authority() {
        let task_addr = RdramAddr::from_offset(0x40);
        retain_loaded_rsp_task(PendingLoadedRspTask {
            task_addr,
            header: OsTaskHeader {
                task_type: M_GFXTASK,
                ..Default::default()
            },
            resumed_data_identity: None,
        });
        crate::load_rom(Vec::new());

        let panic = std::panic::catch_unwind(|| take_loaded_rsp_task(task_addr)).unwrap_err();
        let message = panic_message(panic.as_ref());
        assert!(message.contains("has no unconsumed osSpTaskLoad admission"));
        assert!(crate::host_evidence_snapshot().rsp_task_lineages.is_empty());
    }


    #[test]
    fn loading_one_suspended_task_preserves_other_resume_authorizations() {
        crate::load_rom(Vec::new());
        let original = |yield_data_ptr| OsTaskHeader {
            yield_data_ptr,
            yield_data_size: 0x40,
            ..Default::default()
        };
        let first_addr = RdramAddr::from_offset(0x40);
        let second_addr = RdramAddr::from_offset(0x80);
        let first = RspTaskLineage {
            admission_generation: RspTaskAdmissionGeneration::first(),
            original_header: original(0x180),
            data_identity: None,
            phase: RspTaskLineagePhase::ResumeAuthorized,
        };
        let second = RspTaskLineage {
            admission_generation: RspTaskAdmissionGeneration(NonZeroU64::new(2).unwrap()),
            original_header: original(0x1c0),
            data_identity: None,
            phase: RspTaskLineagePhase::ResumeAuthorized,
        };
        with_host(|host| {
            host.rsp_task_lineages.insert(first_addr.offset(), first);
            host.rsp_task_lineages.insert(second_addr.offset(), second);
        });

        let loaded = loaded_rsp_task_from_header(first_addr, first.yielded_header());
        retain_loaded_rsp_task(loaded);
        let snapshot = crate::host_evidence_snapshot();
        assert_eq!(snapshot.rsp_task_lineages.len(), 2);
        assert_eq!(
            snapshot.rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeLoaded
        );
        assert_eq!(
            snapshot.rsp_task_lineages[1].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized
        );

        let loaded = take_loaded_rsp_task(first_addr);
        retain_started_rsp_task_lineage(loaded, None);
        retire_running_rsp_task_lineage(first_addr, "multiple-suspended test completion");
        let snapshot = crate::host_evidence_snapshot();
        assert_eq!(snapshot.rsp_task_lineages.len(), 1);
        assert_eq!(
            snapshot.rsp_task_lineages[0].task_offset,
            second_addr.offset()
        );
        assert_eq!(
            snapshot.rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized
        );
    }


    #[test]
    fn fresh_load_reuse_cancels_same_address_resume_authorization() {
        crate::load_rom(Vec::new());
        let task_addr = RdramAddr::from_offset(0x40);
        with_host(|host| {
            host.rsp_task_lineages.insert(
                task_addr.offset(),
                RspTaskLineage {
                    admission_generation: RspTaskAdmissionGeneration::first(),
                    original_header: OsTaskHeader {
                        yield_data_ptr: 0x180,
                        yield_data_size: 0x40,
                        ..Default::default()
                    },
                    data_identity: None,
                    phase: RspTaskLineagePhase::ResumeAuthorized,
                },
            );
        });

        retain_loaded_rsp_task(PendingLoadedRspTask {
            task_addr,
            header: OsTaskHeader::default(),
            resumed_data_identity: None,
        });

        assert!(crate::host_evidence_snapshot().rsp_task_lineages.is_empty());
    }


    #[test]
    fn microcode_data_capture_rejects_out_of_bounds_task_range() {
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x100];
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        let header = OsTaskHeader {
            task_type: M_GFXTASK,
            ucode_data: 0x8000_00ff,
            ucode_data_size: 2,
            ..Default::default()
        };
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            task_microcode_data_identity(
                rdram.as_mut_ptr(),
                RdramAddr::from_offset(0x40),
                header.ucode_data,
                header.ucode_data_size,
            )
        }))
        .unwrap_err();
        let message = panic_message(panic.as_ref());
        assert!(message.contains("microcode-data range [0x000000ff, 0x00000101)"));
        assert!(message.contains("registered allocation length 0x100"));
    }


    #[test]
    fn microcode_data_capture_uses_sp_dram_addr_high_alias() {
        const DATA: u32 = 0x81;
        const BYTES: [u8; 5] = [0x10, 0x32, 0x54, 0x76, 0x98];
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x100];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(DATA + offset as u32), byte);
            }
        }
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });

        let identity = unsafe {
            task_microcode_data_identity(
                rdram.as_mut_ptr(),
                RdramAddr::from_offset(0x40),
                0xab00_0000 | DATA,
                BYTES.len() as u32,
            )
        };

        assert_eq!(identity.addr, RdramAddr::from_offset(DATA));
        let expected_sha256: [u8; 32] = Sha256::digest(BYTES).into();
        assert_eq!(identity.sha256, expected_sha256);
    }


    #[test]
    fn microcode_data_capture_rejects_sparse_host_bytes_beyond_physical_rdram() {
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE + 0x100];
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            task_microcode_data_identity(
                rdram.as_mut_ptr(),
                RdramAddr::from_offset(0x40),
                fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u32 - 1,
                2,
            )
        }))
        .unwrap_err();
        let message = panic_message(panic.as_ref());
        assert!(message.contains("microcode-data range [0x007fffff, 0x00800001)"));
        assert!(message.contains("exceeds physical RDRAM length 0x800000"));
    }


    #[test]
    fn text_only_backend_identity_cannot_set_a_microcode_pair_family() {
        struct TextOnlyBackend {
            admitted: [u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        }

        impl RenderBackend for TextOnlyBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn identify_microcode(
                &self,
                imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
            ) -> Option<UcodeId> {
                (imem == &self.admitted).then_some(UcodeId::F3dex2)
            }

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        let admitted = [0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let backend = TextOnlyBackend { admitted };
        assert_eq!(backend.identify_microcode(&admitted), Some(UcodeId::F3dex2));
        set_render_backend(Box::new(backend), fn64_runtime::rdram::DEFAULT_RDRAM_SIZE);
        assert_eq!(
            identify_microcode_pair(
                &admitted,
                TaskMicrocodeDataIdentity {
                    addr: RdramAddr::from_offset(0x100),
                    size: 3,
                    sha256: Sha256::digest([1, 2, 3]).into(),
                },
                None,
            ),
            None
        );
    }


    #[test]
    fn pinned_family_authority_fills_absent_backend_identity_and_rejects_conflict() {
        let imem = [0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let data = TaskMicrocodeDataIdentity {
            addr: RdramAddr::from_offset(0x100),
            size: 3,
            sha256: Sha256::digest([1, 2, 3]).into(),
        };
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        );
        assert_eq!(
            identify_microcode_pair(&imem, data, Some(UcodeId::F3dzex2)),
            Some(UcodeId::F3dzex2)
        );

        set_render_backend(
            Box::new(ExactIdentityBackend {
                admitted: imem,
                admitted_data: fn64_render::MicrocodeDataImageIdentity {
                    bytes: data.size,
                    sha256: data.sha256,
                },
                family: UcodeId::F3dex2,
            }),
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        );
        let panic = std::panic::catch_unwind(|| {
            identify_microcode_pair(&imem, data, Some(UcodeId::F3dzex2))
        })
        .unwrap_err();
        assert!(panic_message(panic.as_ref()).contains("backend pair catalog claimed F3dex2"));
    }


    #[test]
    fn lle_microcode_recognition_requires_the_backends_exact_text_data_pair() {
        const HEADER: usize = 0x40;
        const IMAGE: usize = 0x100;
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x200];
        for (field, value) in [
            (0x00, fn64_runtime::M_GFXTASK),
            (0x08, 0x8000_0000 | IMAGE as u32),
            (0x0c, 12),
            (0x10, 0xA000_0000 | IMAGE as u32),
            (0x14, 12),
        ] {
            rdram[HEADER + field..HEADER + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        for (index, word) in [0x2408_4321u32, 0xac08_0100, 0x0000_000d]
            .into_iter()
            .enumerate()
        {
            let offset = IMAGE + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        prepare_renderer_rdram(&mut rdram);
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };
        let (admitted, imem_generation) = with_host(|host| {
            let memory = host.device_fabric.rsp_memory();
            (
                *memory.bank(fn64_runtime::RspMemoryBank::Imem),
                memory.imem_generation(),
            )
        });
        let expected_digest = imem_sha256(&admitted);
        let expected_at = Cycles::new(sim_time());
        set_render_backend_with_policy(
            Box::new(ExactIdentityBackend {
                admitted,
                admitted_data: fn64_render::MicrocodeDataImageIdentity {
                    bytes: 0,
                    sha256: Sha256::digest([]).into(),
                },
                family: UcodeId::F3dzex2,
            }),
            rdram.len(),
            GraphicsTaskExecutionPolicy::LleAccuracy,
        );

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        assert_eq!(
            copy_rsp_rdp_observations(),
            vec![RspRdpObservationEvent {
                at: expected_at,
                kind: RspRdpObservationKind::MicrocodeRecognition {
                    task_addr: RdramAddr::from_offset(HEADER as u32),
                    imem_generation,
                    text_sha256: expected_digest,
                    data_addr: RdramAddr::from_offset(0),
                    data_size: 0,
                    data_sha256: Sha256::digest([]).into(),
                    family: Some(UcodeId::F3dzex2),
                },
            }]
        );
    }


    #[test]
    fn graphics_hle_unsupported_fallback_records_then_replays_untouched_ucode_through_lle() {
        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        let ucode_off =
            u32::from_ne_bytes(rdram[HEADER + 0x10..HEADER + 0x14].try_into().unwrap()) as usize;
        for (index, word) in [0x2405_5678u32, 0xac07_0100].into_iter().enumerate() {
            let offset = ucode_off + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        with_host(|host| {
            host.device_fabric
                .rsp_memory_mut()
                .write_word(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0x88),
                    0x0000_000d,
                )
                .unwrap();
        });
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::NeedsLle {
                ucode_sha256: [0; 32],
            })),
            rdram.len(),
        );
        fn64_runtime::arm_unsupported_events(None).unwrap();

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        let unsupported = fn64_runtime::copy_unsupported_events();
        assert_eq!(unsupported.len(), 1);
        assert!(unsupported[0].operation.starts_with("render.hle-ucode."));
        assert_eq!(
            unsupported[0].disposition,
            fn64_runtime::UnsupportedDisposition::NeedsLle
        );
        assert_eq!(unsupported[0].guest_cycle, Some(fn64_runtime::Cycles::ZERO));

        with_host(|host| {
            let fabric = &host.device_fabric;
            assert_eq!(
                fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x100,
                    ))
                    .unwrap(),
                0x0000_7777,
                "LLE fallback must retain the rspboot jump-delay scalar register state"
            );
            assert_eq!(fabric.sp_pc(), 0x88);
            assert!(
                fabric.snapshot().sp_busy,
                "the LLE BREAK schedules externally visible SP completion"
            );
        });
    }


    #[test]
    fn graphics_lle_accuracy_policy_forwards_raw_dpc_without_hle_dispatch() {
        use std::cell::RefCell;
        use std::rc::Rc;

        crate::load_rom(Vec::new());

        type DpcCall = (u32, u32, u32, u32);
        struct LleDpcBackend {
            hle_calls: Rc<Cell<u32>>,
            dpc_calls: Rc<RefCell<Vec<DpcCall>>>,
        }

        impl RenderBackend for LleDpcBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                self.hle_calls.set(self.hle_calls.get() + 1);
                Ok(FrameStatus::Complete)
            }

            fn process_rdp_commands(
                &mut self,
                rdram: &mut [u8],
                start: u32,
                end: u32,
                output_addr: u32,
                _wait_for_completion: bool,
            ) -> Result<FrameStatus, RenderError> {
                let first = fn64_runtime::RdramView::from_storage(rdram)
                    .read_u32(fn64_runtime::RdramAddr::from_offset(start));
                self.dpc_calls
                    .borrow_mut()
                    .push((start, end, output_addr, first));
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
                fn64_render::DpFullSyncStatus::Reached
            }

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        const HEADER: usize = 0x40;
        const DPC_START: u32 = 0x180;
        const DPC_END: u32 = 0x188;
        const VI_OUTPUT: u32 = 0x100;
        const MICROCODE_DATA: u32 = 0x1a1;
        const MICROCODE_DATA_BYTES: [u8; 5] = [0x13, 0x57, 0x9b, 0xdf, 0x24];
        let mtc0 = |rt: u32, rd: u32| (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11);
        let mut rdram = vec![0u8; 0x200];
        rdram[DPC_START as usize..DPC_START as usize + 4]
            .copy_from_slice(&0xe900_0000u32.to_ne_bytes());
        rdram[DPC_START as usize + 4..DPC_END as usize].copy_from_slice(&0u32.to_ne_bytes());
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        rdram[HEADER + 0x18..HEADER + 0x1c]
            .copy_from_slice(&(0xA000_0000 | MICROCODE_DATA).to_ne_bytes());
        rdram[HEADER + 0x1c..HEADER + 0x20]
            .copy_from_slice(&(MICROCODE_DATA_BYTES.len() as u32).to_ne_bytes());
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in MICROCODE_DATA_BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(MICROCODE_DATA + offset as u32), byte);
            }
        }
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        let ucode_off =
            u32::from_ne_bytes(rdram[HEADER + 0x10..HEADER + 0x14].try_into().unwrap()) as usize;
        for (index, word) in [0x2402_0000 | DPC_START, mtc0(2, 8)]
            .into_iter()
            .enumerate()
        {
            let offset = ucode_off + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        with_host(|host| {
            let memory = host.device_fabric.rsp_memory_mut();
            for (offset, word) in [
                (0x88, 0x2403_0000 | DPC_END),
                (0x8c, mtc0(3, 9)),
                (0x90, 0x0000_000d),
            ] {
                memory
                    .write_word(
                        fn64_runtime::RspMemAddr::from_parts(
                            fn64_runtime::RspMemoryBank::Imem,
                            offset,
                        ),
                        word,
                    )
                    .unwrap();
            }
        });
        let submissions = Rc::new(RefCell::new(Vec::new()));
        let hle_calls = Rc::new(Cell::new(0));
        prepare_renderer_rdram(&mut rdram);
        set_render_backend_with_policy(
            Box::new(LleDpcBackend {
                hle_calls: Rc::clone(&hle_calls),
                dpc_calls: Rc::clone(&submissions),
            }),
            rdram.len(),
            GraphicsTaskExecutionPolicy::LleAccuracy,
        );
        let mut vi_ctx = ctx_zeroed();
        vi_ctx.r4 = u64::from(0x8000_0000 | VI_OUTPUT);
        unsafe { crate::vi::osViSwapBuffer_recomp(rdram.as_mut_ptr(), &mut vi_ctx) };

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        let submissions = submissions.borrow();
        assert_eq!(
            hle_calls.get(),
            0,
            "LLE accuracy policy must not offer graphics microcode to HLE"
        );
        assert_eq!(submissions.len(), 1);
        let (start, end, output, first) = submissions[0];
        assert_eq!(end - start, DPC_END - DPC_START);
        assert_eq!(output, VI_OUTPUT);
        assert_eq!(first, 0xe900_0000);
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert!(snapshot.sp_busy);
            assert!(snapshot.dp_busy);
        });
        let observations = copy_rsp_rdp_observations();
        assert_eq!(observations.len(), 3);
        let microcode_data_sha256: [u8; 32] = Sha256::digest(MICROCODE_DATA_BYTES).into();
        let replacement_generation = match &observations[0].kind {
            RspRdpObservationKind::ImemReplacementCommitted {
                task_addr,
                imem_generation,
                ..
            } => {
                assert_eq!(*task_addr, RdramAddr::from_offset(HEADER as u32));
                *imem_generation
            }
            ref other => panic!("expected rspboot replacement first, got {other:?}"),
        };
        assert!(matches!(
            &observations[1].kind,
            RspRdpObservationKind::MicrocodeRecognition {
                task_addr,
                imem_generation,
                data_addr,
                data_size,
                data_sha256,
                family: None,
                ..
            } if *task_addr == RdramAddr::from_offset(HEADER as u32)
                && *imem_generation == replacement_generation
                && *data_addr == RdramAddr::from_offset(MICROCODE_DATA)
                && *data_size == MICROCODE_DATA_BYTES.len() as u32
                && *data_sha256 == microcode_data_sha256
        ));
        assert_eq!(
            observations[2].kind,
            RspRdpObservationKind::DramDpcCommitted {
                start: DPC_START,
                end: DPC_END,
                command_sha256: canonical_rdp_words_sha256(&[0xe900_0000, 0]),
            }
        );
    }


    #[test]
    fn graphics_hle_optimized_policy_remains_explicitly_selectable() {
        use std::rc::Rc;

        struct CountingHleBackend(Rc<Cell<u32>>);

        impl RenderBackend for CountingHleBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                self.0.set(self.0.get() + 1);
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
                fn64_render::DpFullSyncStatus::NotReached
            }

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        let ucode_off =
            u32::from_ne_bytes(rdram[HEADER + 0x10..HEADER + 0x14].try_into().unwrap()) as usize;
        for (index, word) in [0x2405_5678u32, 0xac05_0100].into_iter().enumerate() {
            let offset = ucode_off + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        let calls = Rc::new(Cell::new(0));
        prepare_renderer_rdram(&mut rdram);
        set_render_backend_with_policy(
            Box::new(CountingHleBackend(Rc::clone(&calls))),
            rdram.len(),
            GraphicsTaskExecutionPolicy::HleOptimized,
        );

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        assert_eq!(calls.get(), 1);
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert!(snapshot.sp_busy);
            assert!(
                !snapshot.dp_busy,
                "an HLE graphics task without FullSync must schedule SP only"
            );
            assert_eq!(
                host.device_fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x100,
                    ))
                    .unwrap(),
                0,
                "optimized HLE must retain the loaded ucode behind its backend boundary"
            );
        });
    }


    #[test]
    fn unknown_task_lle_executes_persistent_imem_through_break() {
        let mut rdram = vec![0u8; 0x1000];
        let task_addr = RdramAddr::from_offset(0);
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
            let program = [0x2402_1234u32, 0xAC02_0100, 0x0000_000D];
            let bytes: Vec<u8> = program.into_iter().flat_map(u32::to_be_bytes).collect();
            host.device_fabric
                .rsp_memory_mut()
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    &bytes,
                )
                .unwrap();
        });
        install_running_task_lineage(task_addr, RspTaskAdmissionGeneration::first());

        let result = unsafe {
            dispatch_lle_task(rdram.as_mut_ptr(), Some(task_addr), false, None, None, None)
        };

        assert_eq!(
            result,
            LleTaskResult {
                steps: 3,
                dp_full_sync: fn64_render::DpFullSyncStatus::NotReached,
            }
        );
        with_host(|host| {
            assert_eq!(
                host.device_fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x100,
                    ))
                    .unwrap(),
                0x0000_1234
            );
            assert_eq!(
                host.device_fabric.sp_status()
                    & (fn64_runtime::SP_STATUS_HALT | fn64_runtime::SP_STATUS_BROKE),
                fn64_runtime::SP_STATUS_HALT | fn64_runtime::SP_STATUS_BROKE
            );
        });
    }


    #[test]
    fn os_sp_task_start_go_routes_unknown_task_through_lle() {
        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x1000];
        // task_type zero is intentionally not one of the exact HLE selectors.
        rdram[HEADER..HEADER + 4].copy_from_slice(&0u32.to_ne_bytes());
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
            let program = [0x2402_3456u32, 0xAC02_0108, 0x0000_000D];
            let bytes: Vec<u8> = program.into_iter().flat_map(u32::to_be_bytes).collect();
            host.device_fabric
                .rsp_memory_mut()
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    &bytes,
                )
                .unwrap();
        });
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        retain_loaded_rsp_task(PendingLoadedRspTask {
            task_addr: RdramAddr::from_offset(HEADER as u32),
            header: OsTaskHeader::default(),
            resumed_data_identity: None,
        });

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        with_host(|host| {
            let fabric = &host.device_fabric;
            assert_eq!(
                fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x108,
                    ))
                    .unwrap(),
                0x0000_3456
            );
            assert!(
                fabric.snapshot().sp_busy,
                "LLE BREAK schedules externally visible SP completion"
            );
        });
    }


    #[test]
    fn raw_sp_status_clear_halt_runs_the_rsp_without_the_task_shim() {
        // The raw-MMIO analogue of
        // `os_sp_task_start_go_routes_unknown_task_through_lle`: same IMEM
        // program, same expected DMEM result, but no OSTask, no shim call, and
        // no admitted lineage -- only SP_PC, IMEM, and a SP_STATUS write that
        // clears HALT. A guest running its own libultra kicks the RSP exactly
        // this way, which is why an unknown ROM does not need `osSpTaskStartGo`
        // identified to drive the RSP.
        let mut rdram = vec![0u8; 0x1000];
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
            let program = [0x2402_3456u32, 0xAC02_0108, 0x0000_000D];
            let bytes: Vec<u8> = program.into_iter().flat_map(u32::to_be_bytes).collect();
            host.device_fabric
                .rsp_memory_mut()
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    &bytes,
                )
                .unwrap();
            assert!(
                host.rsp_task_lineages.is_empty(),
                "the raw kick path must not depend on any admitted task lineage"
            );
        });
        crate::pi::set_live_sp_pc(0);

        // SP_STATUS bit 0 is clear-halt. The device is halted out of reset, so
        // this is the starting edge.
        assert!(crate::pi::write_live_device_mmio(
            0xFFFF_FFFF_A404_0010,
            1 << 0
        ));

        with_host(|host| {
            let fabric = &host.device_fabric;
            assert_eq!(
                fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x108,
                    ))
                    .unwrap(),
                0x0000_3456,
                "the raw kick executed the IMEM program and its store landed"
            );
            assert!(
                fabric.snapshot().sp_busy,
                "raw kick BREAK schedules externally visible SP completion"
            );
            assert!(
                host.rsp_task_lineages.is_empty(),
                "a raw kick must never fabricate a task lineage"
            );
        });
    }


    #[test]
    fn two_consecutive_normal_tasks_retire_running_lineage_without_yield_query() {
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x1000];
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
            let program = [0x0000_000du32];
            let bytes: Vec<u8> = program.into_iter().flat_map(u32::to_be_bytes).collect();
            host.device_fabric
                .rsp_memory_mut()
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    &bytes,
                )
                .unwrap();
        });
        let mut ctx = ctx_zeroed();

        for task_offset in [0x40, 0x80] {
            crate::pi::set_live_sp_pc(0);
            let task_addr = RdramAddr::from_offset(task_offset);
            retain_loaded_rsp_task(PendingLoadedRspTask {
                task_addr,
                header: OsTaskHeader::default(),
                resumed_data_identity: None,
            });
            ctx.r4 = 0x8000_0000 + u64::from(task_offset);

            unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

            assert!(
                crate::host_evidence_snapshot().rsp_task_lineages.is_empty(),
                "normal task {task_offset:#x} must retire before another task starts"
            );
            let deadline = crate::next_device_deadline().expect("normal task completion deadline");
            crate::advance_virtual_time(deadline);
        }
    }
