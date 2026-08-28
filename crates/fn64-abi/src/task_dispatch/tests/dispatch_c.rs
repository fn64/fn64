use super::*;

    #[test]
    fn rspboot_waits_only_for_a_busy_dmem_dpc_source() {
        let dmem_busy =
            fn64_runtime::DPC_STATUS_XBUS_DMEM_DMA | fn64_runtime::DPC_STATUS_DMA_BUSY;
        assert!(rspboot_waits_for_live_dmem_dpc(dmem_busy));
        assert!(rspboot_waits_for_live_dmem_dpc(
            dmem_busy | fn64_runtime::DPC_STATUS_CMD_BUSY
        ));
        assert!(!rspboot_waits_for_live_dmem_dpc(
            fn64_runtime::DPC_STATUS_XBUS_DMEM_DMA
        ));
        assert!(!rspboot_waits_for_live_dmem_dpc(
            fn64_runtime::DPC_STATUS_DMA_BUSY
        ));
    }

    #[test]
    fn unknown_task_lle_resolves_rspboot_style_imem_overlay_and_resumes() {
        const DATA: u32 = 0x281;
        const DATA_BYTES: [u8; 7] = [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc];
        crate::load_rom(Vec::new());
        let mtc0 = |rt: u32, rd: u32| (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11);
        let boot = [
            0x2402_0200u32,
            mtc0(2, 1),
            0x2403_1000,
            mtc0(3, 0),
            0x2404_001F,
            mtc0(4, 2),
            0,
            0,
        ];
        let overlay = [0u32, 0, 0, 0, 0, 0, 0x2405_5678, 0xAC05_0104];
        let mut rdram = vec![0u8; 0x1000];
        prepare_renderer_rdram(&mut rdram);
        for (index, word) in overlay.into_iter().enumerate() {
            let offset = 0x200 + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in DATA_BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(DATA + offset as u32), byte);
            }
        }
        // The 32-byte DMA resumes at 0x1018; put BREAK in the still-existing
        // word immediately after the overlay transfer.
        let boot_bytes: Vec<u8> = boot.into_iter().flat_map(u32::to_be_bytes).collect();
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
            let memory = host.device_fabric.rsp_memory_mut();
            memory
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    &boot_bytes,
                )
                .unwrap();
            memory
                .write_word(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0x20),
                    0x0000_000D,
                )
                .unwrap();
        });
        let (generation_before, initial_digest) = with_host(|host| {
            let memory = host.device_fabric.rsp_memory();
            (
                memory.imem_generation(),
                imem_sha256(memory.bank(fn64_runtime::RspMemoryBank::Imem)),
            )
        });
        set_render_backend_with_policy(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            rdram.len(),
            GraphicsTaskExecutionPolicy::LleAccuracy,
        );
        let expected_at = Cycles::new(sim_time());

        let task_addr = RdramAddr::from_offset(0x40);
        install_running_task_lineage(task_addr, RspTaskAdmissionGeneration::first());
        let task = OsTaskHeader {
            ucode_data: 0x8000_0000 | DATA,
            ucode_data_size: DATA_BYTES.len() as u32,
            ..Default::default()
        };
        let microcode_data = unsafe {
            task_microcode_data_identity(
                rdram.as_mut_ptr(),
                task_addr,
                task.ucode_data,
                task.ucode_data_size,
            )
        };
        let result = unsafe {
            dispatch_lle_task(
                rdram.as_mut_ptr(),
                Some(task_addr),
                true,
                None,
                Some(microcode_data),
                None,
            )
        };

        assert_eq!(
            result,
            LleTaskResult {
                steps: 9,
                dp_full_sync: fn64_render::DpFullSyncStatus::NotReached,
                pending_raw_dpc_task_batch: None,
            }
        );
        with_host(|host| {
            let memory = host.device_fabric.rsp_memory();
            assert_eq!(memory.imem_generation(), generation_before + 1);
            assert_eq!(
                memory
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x104,
                    ))
                    .unwrap(),
                0x0000_5678
            );
        });
        let final_digest = with_host(|host| {
            imem_sha256(
                host.device_fabric
                    .rsp_memory()
                    .bank(fn64_runtime::RspMemoryBank::Imem),
            )
        });
        assert_eq!(
            copy_rsp_rdp_observations(),
            vec![
                RspRdpObservationEvent {
                    at: expected_at,
                    kind: RspRdpObservationKind::MicrocodeRecognition {
                        task_addr: RdramAddr::from_offset(0x40),
                        imem_generation: generation_before,
                        text_sha256: initial_digest,
                        data_addr: microcode_data.addr,
                        data_size: microcode_data.size,
                        data_sha256: microcode_data.sha256,
                        family: None,
                    },
                },
                RspRdpObservationEvent {
                    at: expected_at,
                    kind: RspRdpObservationKind::ImemReplacementCommitted {
                        task_addr: RdramAddr::from_offset(0x40),
                        imem_generation: generation_before + 1,
                        text_sha256: final_digest,
                    },
                },
                RspRdpObservationEvent {
                    at: expected_at,
                    kind: RspRdpObservationKind::MicrocodeRecognition {
                        task_addr: RdramAddr::from_offset(0x40),
                        imem_generation: generation_before + 1,
                        text_sha256: final_digest,
                        data_addr: microcode_data.addr,
                        data_size: microcode_data.size,
                        data_sha256: microcode_data.sha256,
                        family: None,
                    },
                },
            ]
        );
    }


    #[test]
    fn xbus_dpc_submission_stages_logical_dmem_commands_for_renderer() {
        use fn64_render::RenderConfig;

        crate::load_rom(Vec::new());
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let commands: [(u32, u32); 4] = [
            (0xef00_0000 | (3 << 20), 0),
            (0xff10_0003, TARGET),
            (0xf700_0000, 0x07c1_07c1),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
        ];
        let mut dmem = [0u8; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = index * 8;
            dmem[offset..offset + 4].copy_from_slice(&w0.to_be_bytes());
            dmem[offset + 4..offset + 8].copy_from_slice(&w1.to_be_bytes());
        }
        let mut backend = fn64_render_reference::ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(Box::new(backend), rdram.len());
        RAW_DPC_STAGING_SCRATCH.with(|cell| *cell.borrow_mut() = Vec::new());

        unsafe {
            dispatch_raw_rdp_xbus(rdram.as_mut_ptr(), &dmem, 0, (commands.len() * 8) as u32);
        }
        let first_staging = RAW_DPC_STAGING_SCRATCH.with(|cell| {
            let image = cell.borrow();
            assert!(!image.is_empty());
            (image.as_ptr(), image.capacity())
        });
        unsafe {
            dispatch_raw_rdp_xbus(rdram.as_mut_ptr(), &dmem, 0, (commands.len() * 8) as u32);
        }
        let second_staging = RAW_DPC_STAGING_SCRATCH.with(|cell| {
            let image = cell.borrow();
            (image.as_ptr(), image.capacity())
        });

        assert_eq!(second_staging, first_staging);
        assert_eq!(last_render_error(), None);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for index in 0..8 {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + index * 2)),
                0x07c1,
                "XBUS raw RDP pixel {index}"
            );
        }
        assert_eq!(
            copy_rsp_rdp_observations()
                .into_iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![
                RspRdpObservationKind::XbusDpcCommitted {
                    start: 0,
                    end: (commands.len() * 8) as u32,
                    command_sha256: canonical_rdp_words_sha256(
                        &commands
                            .into_iter()
                            .flat_map(|(w0, w1)| [w0, w1])
                            .collect::<Vec<_>>()
                    ),
                },
                RspRdpObservationKind::XbusDpcCommitted {
                    start: 0,
                    end: (commands.len() * 8) as u32,
                    command_sha256: canonical_rdp_words_sha256(
                        &commands
                            .into_iter()
                            .flat_map(|(w0, w1)| [w0, w1])
                            .collect::<Vec<_>>()
                    ),
                },
            ]
        );
    }


    #[test]
    fn xbus_dpc_submission_executes_variable_width_raw_z_triangle() {
        use fn64_render::RenderConfig;

        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        let commands: [(u32, u32); 9] = [
            (0xff10_0007, TARGET),
            (0xfa00_0000, 0xff00_00ff),
            // lft=0: the vertical XM minor edge sits left of the rightward-
            // sloping XH major edge (right-major geometry).
            (0x0900_0000 | yl, (ym << 16) | yh),
            (1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32),
            (1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32),
            (1 << 16, 0),
            (4 << 16, 0),
            (0, 0),
            (0xe900_0000, 0),
        ];
        let mut dmem = [0u8; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = index * 8;
            dmem[offset..offset + 4].copy_from_slice(&w0.to_be_bytes());
            dmem[offset + 4..offset + 8].copy_from_slice(&w1.to_be_bytes());
        }
        let mut backend = fn64_render_reference::ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(Box::new(backend), rdram.len());

        unsafe {
            dispatch_raw_rdp_xbus(rdram.as_mut_ptr(), &dmem, 0, (commands.len() * 8) as u32);
        }

        assert_eq!(last_render_error(), None);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (4 * 8 + 2) * 2
            )),
            0xf801
        );
    }


    #[test]
    fn os_sp_task_yielded_completed_query_does_not_resubmit_gfx_task() {
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 128];
        // OSTask_t header at offset 0x10 (mirrors the real call site's
        // s1+0x10 addressing): type = M_GFXTASK at +0x0.
        let header_off = 0x10usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        let before = with_executor(|exec| exec.task_log().gfx_count());
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(
            ctx.r2, 0,
            "task reported complete (0), not OS_TASK_YIELDED (1)"
        );
        assert_eq!(with_executor(|exec| exec.task_log().gfx_count()), before);
    }


    /// Regression for the "gfx task submitted, framebuffer never swaps"
    /// deadlock: `osSpTaskStartGo_recomp` MUST post the SP-done (and, for a
    /// graphics task, the DP-done) completion event to whatever queue the
    /// game registered via `osSetEventMesg`, mirroring OoT's Scheduler
    /// (`sched.c:704-705`: `osSetEventMesg(OS_EVENT_SP, &interruptQueue,
    /// RSP_DONE_MSG=667)` / `osSetEventMesg(OS_EVENT_DP, ..., RDP_DONE_MSG=
    /// 668)`). Without these, `Sched_ThreadEntry`'s `osRecvMesg` on
    /// `interruptQueue` (`sched.c:656`) never wakes, `Sched_TaskComplete`
    /// (`sched.c:393`) never posts to `gfxCtx->queue`, and
    /// `Graph_ExecuteAndDraw`'s `osRecvMesg` (`graph.c:234`) blocks forever
    /// -> `osViSwapBuffer` is never reached (observed as `vi_swaps=0` in
    /// `examples/oot-boot`).
    ///
    /// The prior stub was an empty `{}`, so reintroducing it (delete the
    /// two `inject_event` calls) makes both `recv_mesg` asserts below fail
    /// with `WouldBlock` -- verified by hand before committing, not a
    /// green-against-the-bug check.
    #[test]
    fn os_sp_task_start_go_posts_sp_and_dp_completion_to_registered_queue() {
        // OoT's real event->message mapping (sched.c).
        const OS_EVENT_SP: u32 = 4;
        const OS_EVENT_DP: u32 = 9;
        const RSP_DONE_MSG: u32 = 667;
        const RDP_DONE_MSG: u32 = 668;
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::pi::set_mi_interrupt_mask(
            fn64_runtime::InterruptSource::Sp.bit() | fn64_runtime::InterruptSource::Dp.bit(),
        );

        // A distinct queue address so this test can't collide with the
        // shared thread-local executor's other queues (same isolation
        // rationale as the rung tests' hand-picked addresses).
        let interrupt_q = RdramAddr::from_offset(0x0009_0000);
        with_executor(|exec| {
            exec.create_mesg_queue(interrupt_q, 4);
            exec.set_event_mesg(OS_EVENT_SP, interrupt_q, RSP_DONE_MSG);
            exec.set_event_mesg(OS_EVENT_DP, interrupt_q, RDP_DONE_MSG);
        });

        // A graphics task header (M_GFXTASK at +0x0), read from ctx.r4 the
        // same way the real `Sched_RunTask` call site passes `&spTask->list`.
        let mut rdram = vec![0u8; 128];
        let header_off = 0x10usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        admit_synthetic_hle_task(&mut rdram, header_off, &mut ctx);
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            rdram.len(),
        );

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        let before = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
        assert_eq!(
            before
                & (fn64_runtime::InterruptSource::Sp.bit()
                    | fn64_runtime::InterruptSource::Dp.bit()),
            0
        );
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, interrupt_q, false)),
            RecvMesgOutcome::WouldBlock
        );

        crate::advance_virtual_time(8);
        let after_sp = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
        assert_ne!(after_sp & fn64_runtime::InterruptSource::Sp.bit(), 0);
        assert_eq!(after_sp & fn64_runtime::InterruptSource::Dp.bit(), 0);

        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::Delivered(RSP_DONE_MSG),
                "osSpTaskStartGo must post OS_EVENT_SP -> RSP_DONE_MSG"
            );
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::WouldBlock,
                "DP completion must not collapse into the SP deadline"
            );
        });

        crate::advance_virtual_time(9);
        let after_dp = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
        assert_ne!(after_dp & fn64_runtime::InterruptSource::Dp.bit(), 0);
        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::Delivered(RDP_DONE_MSG),
                "a graphics task's osSpTaskStartGo must ALSO post OS_EVENT_DP -> RDP_DONE_MSG"
            );
            // Nothing else was posted.
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::WouldBlock,
                "exactly two completion messages, no more"
            );
        });
    }


    #[test]
    fn yielded_render_backend_sets_sig1_and_completes_sp_without_dp() {
        const OS_EVENT_SP: u32 = 4;
        const OS_EVENT_DP: u32 = 9;
        const RSP_DONE_MSG: u32 = 667;
        const RDP_DONE_MSG: u32 = 668;
        const HEADER_OFF: usize = 0x20;
        const YIELD_DATA: u32 = 0x180;
        const YIELD_SIZE: u32 = 0x200;

        crate::load_rom(Vec::new());
        crate::pi::set_mi_interrupt_mask(
            fn64_runtime::InterruptSource::Sp.bit() | fn64_runtime::InterruptSource::Dp.bit(),
        );
        let interrupt_q = RdramAddr::from_offset(0x0009_2000);
        with_executor(|exec| {
            exec.create_mesg_queue(interrupt_q, 4);
            exec.set_event_mesg(OS_EVENT_SP, interrupt_q, RSP_DONE_MSG);
            exec.set_event_mesg(OS_EVENT_DP, interrupt_q, RDP_DONE_MSG);
        });

        let mut rdram = vec![0u8; 0x300];
        for (field, value) in [
            (0x00, fn64_runtime::M_GFXTASK),
            (0x38, YIELD_DATA),
            (0x3c, YIELD_SIZE),
        ] {
            rdram[HEADER_OFF + field..HEADER_OFF + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER_OFF, &mut ctx);
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::Yielded)),
            rdram.len(),
        );

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_ne!(
            crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELDED,
            0
        );

        crate::advance_virtual_time(8);
        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::Delivered(RSP_DONE_MSG)
            );
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::WouldBlock
            );
        });
        crate::advance_virtual_time(10);
        assert_eq!(
            crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap()
                & fn64_runtime::InterruptSource::Dp.bit(),
            0,
            "a yielded display list has not reached DPFullSync"
        );

        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, u64::from(fn64_runtime::OS_TASK_YIELDED));
        let word = |field: usize| {
            u32::from_ne_bytes(
                rdram[HEADER_OFF + field..HEADER_OFF + field + 4]
                    .try_into()
                    .unwrap(),
            )
        };
        assert_eq!(word(0x18), YIELD_DATA);
        assert_eq!(word(0x1c), YIELD_SIZE);
    }


    #[test]
    fn yielded_render_task_reloads_and_resumes_from_its_saved_buffer() {
        use std::sync::{Arc, Mutex};

        struct SequenceBackend {
            calls: Arc<Mutex<Vec<fn64_render::OsTask>>>,
        }

        impl RenderBackend for SequenceBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                let mut calls = self.calls.lock().unwrap();
                calls.push(*task);
                Ok(if calls.len() == 1 {
                    FrameStatus::Yielded
                } else {
                    FrameStatus::Complete
                })
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

        const HEADER_OFF: usize = 0x40;
        const INITIAL_DATA: u32 = 0x140;
        const INITIAL_SIZE: u32 = 0x40;
        const YIELD_DATA: u32 = 0x200;
        const YIELD_SIZE: u32 = 0x180;

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x400];
        for (field, value) in [
            (0x00, fn64_runtime::M_GFXTASK),
            (0x18, INITIAL_DATA),
            (0x1c, INITIAL_SIZE),
            (0x38, YIELD_DATA),
            (0x3c, YIELD_SIZE),
        ] {
            rdram[HEADER_OFF + field..HEADER_OFF + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER_OFF, &mut ctx);
        let calls = Arc::new(Mutex::new(Vec::new()));
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(
            Box::new(SequenceBackend {
                calls: Arc::clone(&calls),
            }),
            rdram.len(),
        );
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        crate::advance_virtual_time(8);
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, u64::from(fn64_runtime::OS_TASK_YIELDED));

        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(
            crate::pi::live_sp_status()
                & (fn64_runtime::SP_STATUS_YIELD | fn64_runtime::SP_STATUS_YIELDED),
            0
        );
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        crate::advance_virtual_time(17);

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].flags & fn64_runtime::OS_TASK_YIELDED, 0);
        assert_eq!(calls[0].ucode_data, INITIAL_DATA);
        assert_ne!(calls[1].flags & fn64_runtime::OS_TASK_YIELDED, 0);
        assert_eq!(calls[1].ucode_data, YIELD_DATA);
        assert_eq!(calls[1].ucode_data_size, YIELD_SIZE);
    }


    #[test]
    fn chunked_hle_observes_sig0_between_commits_and_consumes_resume_once() {
        use std::sync::{Arc, Mutex};

        struct ChunkedBackend {
            steps: Arc<Mutex<Vec<fn64_render::RenderTaskStep>>>,
        }

        impl RenderBackend for ChunkedBackend {
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
                Err(RenderError::Backend {
                    backend: "chunked-test",
                    reason: "atomic entry must not be used".into(),
                })
            }

            fn process_task_chunk(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
                step: fn64_render::RenderTaskStep,
            ) -> Result<fn64_render::RenderTaskChunkStatus, RenderError> {
                self.steps.lock().unwrap().push(step);
                Ok(match step {
                    fn64_render::RenderTaskStep::Start => {
                        fn64_render::RenderTaskChunkStatus::Continue(
                            fn64_render::RenderTaskContinuation::new(1),
                        )
                    }
                    fn64_render::RenderTaskStep::Resume(token) if token.get() == 1 => {
                        fn64_render::RenderTaskChunkStatus::Continue(
                            fn64_render::RenderTaskContinuation::new(2),
                        )
                    }
                    fn64_render::RenderTaskStep::Resume(token) if token.get() == 2 => {
                        fn64_render::RenderTaskChunkStatus::Complete
                    }
                    fn64_render::RenderTaskStep::Resume(token) => panic!(
                        "unexpected or multiply consumed continuation token {}",
                        token.get()
                    ),
                })
            }

            fn task_chunking(&self) -> fn64_render::RenderTaskChunking {
                fn64_render::RenderTaskChunking::Resumable
            }

            fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
                fn64_render::DpFullSyncStatus::NotReached
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        const HEADER_OFF: usize = 0x40;
        const YIELD_DATA: u32 = 0x200;
        const YIELD_SIZE: u32 = 0x80;
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x400];
        for (field, value) in [
            (0x00, fn64_runtime::M_GFXTASK),
            (0x38, YIELD_DATA),
            (0x3c, YIELD_SIZE),
        ] {
            rdram[HEADER_OFF + field..HEADER_OFF + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER_OFF, &mut ctx);
        let steps = Arc::new(Mutex::new(Vec::new()));
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(
            Box::new(ChunkedBackend {
                steps: Arc::clone(&steps),
            }),
            rdram.len(),
        );

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(
            steps.lock().unwrap().as_slice(),
            [fn64_render::RenderTaskStep::Start]
        );
        assert!(with_host(|host| host.device_fabric.snapshot().sp_busy));
        assert_eq!(
            crate::next_device_deadline(),
            Some(crate::sim_time()),
            "a running continuation must remain visible to the host pump"
        );

        unsafe { osSpTaskYield_recomp(rdram.as_mut_ptr(), &mut ctx) };
        crate::advance_virtual_time(8);
        assert_eq!(
            steps.lock().unwrap().len(),
            1,
            "SIG0 must win before token consumption"
        );
        assert_ne!(
            crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELDED,
            0
        );
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, u64::from(fn64_runtime::OS_TASK_YIELDED));

        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(
            steps.lock().unwrap().as_slice(),
            [
                fn64_render::RenderTaskStep::Start,
                fn64_render::RenderTaskStep::Resume(fn64_render::RenderTaskContinuation::new(1))
            ]
        );
        crate::advance_virtual_time(16);
        crate::advance_virtual_time(17);
        assert_eq!(
            steps.lock().unwrap().as_slice(),
            [
                fn64_render::RenderTaskStep::Start,
                fn64_render::RenderTaskStep::Resume(fn64_render::RenderTaskContinuation::new(1)),
                fn64_render::RenderTaskStep::Resume(fn64_render::RenderTaskContinuation::new(2))
            ],
            "each backend continuation is consumed exactly once"
        );
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert!(!snapshot.sp_busy);
            assert!(!snapshot.dp_busy);
        });
    }


    #[test]
    fn direct_imem_chunk_yield_public_resume_completes_with_resumed_generation_owner() {
        use std::sync::{Arc, Mutex};

        struct DirectChunkBackend {
            steps: Arc<Mutex<Vec<fn64_render::RenderTaskStep>>>,
        }

        impl RenderBackend for DirectChunkBackend {
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
                panic!("direct chunk fixture must use its resumable entry")
            }

            fn process_task_chunk(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
                step: fn64_render::RenderTaskStep,
            ) -> Result<fn64_render::RenderTaskChunkStatus, RenderError> {
                self.steps.lock().unwrap().push(step);
                Ok(match step {
                    fn64_render::RenderTaskStep::Start => {
                        fn64_render::RenderTaskChunkStatus::Continue(
                            fn64_render::RenderTaskContinuation::new(7),
                        )
                    }
                    fn64_render::RenderTaskStep::Resume(token) if token.get() == 7 => {
                        fn64_render::RenderTaskChunkStatus::Complete
                    }
                    fn64_render::RenderTaskStep::Resume(token) => {
                        panic!("unexpected direct continuation token {}", token.get())
                    }
                })
            }

            fn task_chunking(&self) -> fn64_render::RenderTaskChunking {
                fn64_render::RenderTaskChunking::Resumable
            }

            fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
                fn64_render::DpFullSyncStatus::NotReached
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        const HEADER: usize = 0x40;
        const IMAGE: usize = 0x100;
        const INITIAL_DATA: u32 = 0x180;
        const YIELD_DATA: u32 = 0x200;
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x280];
        for (field, value) in [
            (0x00, fn64_runtime::M_GFXTASK),
            (0x08, 0x8000_0000 | IMAGE as u32),
            (0x0c, 8),
            (0x10, 0xa000_0000 | IMAGE as u32),
            (0x14, 8),
            (0x18, 0x8000_0000 | INITIAL_DATA),
            (0x1c, 4),
            (0x38, 0xa000_0000 | YIELD_DATA),
            (0x3c, 0x40),
        ] {
            rdram[HEADER + field..HEADER + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        prepare_renderer_rdram(&mut rdram);
        let steps = Arc::new(Mutex::new(Vec::new()));
        set_render_backend_with_policy(
            Box::new(DirectChunkBackend {
                steps: Arc::clone(&steps),
            }),
            rdram.len(),
            GraphicsTaskExecutionPolicy::HleOptimized,
        );
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;

        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };
        unsafe { osSpTaskYield_recomp(rdram.as_mut_ptr(), &mut ctx) };
        crate::advance_virtual_time(8);
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, u64::from(fn64_runtime::OS_TASK_YIELDED));

        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };
        let resumed_generation = crate::host_evidence_snapshot()
            .loaded_rsp_task
            .expect("yielded reload owns a fresh admission")
            .admission_generation;
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };
        let deadline = crate::next_device_deadline().expect("resumed completion deadline");
        crate::advance_virtual_time(deadline);

        assert_eq!(
            steps.lock().unwrap().as_slice(),
            [
                fn64_render::RenderTaskStep::Start,
                fn64_render::RenderTaskStep::Resume(fn64_render::RenderTaskContinuation::new(7),),
            ]
        );
        let evidence = crate::host_evidence_snapshot();
        assert_eq!(
            evidence.rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable {
                owner: RspInterpreterOwner::task(
                    HEADER as u32,
                    RspTaskAdmissionGeneration::new(NonZeroU64::new(resumed_generation).unwrap(),),
                ),
            }
        );
        assert!(evidence.loaded_rsp_task.is_none());
        assert!(evidence.rsp_task_lineages.is_empty());
        assert!(HLE_RENDER_CONTINUATION.with(|cell| cell.borrow().is_none()));
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert!(!snapshot.sp_busy);
            assert!(!snapshot.dp_busy);
        });
    }


    /// An explicitly skipped audio task with no DPC FullSync posts only the
    /// SP-done event. Injecting a spurious RDP_DONE_MSG would desync OoT's
    /// scheduler `curRDPTask` bookkeeping.
    #[test]
    fn os_sp_task_start_go_audio_task_posts_only_sp() {
        const OS_EVENT_SP: u32 = 4;
        const OS_EVENT_DP: u32 = 9;
        const RSP_DONE_MSG: u32 = 667;
        const RDP_DONE_MSG: u32 = 668;

        crate::load_rom(Vec::new());
        set_audio_task_diagnostic_skip();
        let interrupt_q = RdramAddr::from_offset(0x0009_1000);
        with_executor(|exec| {
            exec.create_mesg_queue(interrupt_q, 4);
            exec.set_event_mesg(OS_EVENT_SP, interrupt_q, RSP_DONE_MSG);
            exec.set_event_mesg(OS_EVENT_DP, interrupt_q, RDP_DONE_MSG);
        });

        let mut rdram = vec![0u8; 128];
        let header_off = 0x10usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_AUDTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        admit_synthetic_hle_task(&mut rdram, header_off, &mut ctx);

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, interrupt_q, false)),
            RecvMesgOutcome::WouldBlock
        );
        crate::advance_virtual_time(8);

        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::Delivered(RSP_DONE_MSG),
                "an audio task's osSpTaskStartGo posts OS_EVENT_SP"
            );
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::WouldBlock,
                "a non-graphics task must NOT post OS_EVENT_DP"
            );
        });
    }


    /// Proves the executor gfx-task seam actually reaches a real `dyn
    /// RenderBackend` end-to-end: `set_render_backend` registers a real
    /// `fn64_render_reference::ReferenceBackend`, a real F3DEX2-family display
    /// list (same tiny triangle fixture shape as
    /// `fn64-render-rt64/tests/fixture_replay.rs` -- see that file's doc
    /// comment for why this is a hand-built, not ROM-captured, fixture) is
    /// planted in the SAME `rdram` buffer `osSpTaskStartGo_recomp` reads
    /// its task header from, and the call is made through the real
    /// `extern "C"` shim, not by calling the backend directly. This is the
    /// "wire the executor gfx-task seam" gate: the FULL path (recomp shim
    /// -> registered `dyn RenderBackend` -> rasterizer -> framebuffer) is
    /// exercised, not just its two halves in isolation.
    #[test]
    fn os_sp_task_start_go_routes_gfx_tasks_through_the_registered_render_backend() {
        use fn64_render::RenderConfig;
        use fn64_render_reference::{gbi, ReferenceBackend};

        const RDRAM_LEN: usize = 0x4000;
        const VTX_ADDR: usize = 0x1000;
        const DL_ADDR: usize = 0x2000;
        const HEADER_OFF: usize = 0x10;

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; RDRAM_LEN];

        // Same 3-vertex red/green/blue triangle shape as the
        // fn64-render-rt64 fixture: SDK's public 16-byte Vtx_t
        // position-color layout.
        let verts: [([i16; 2], [u8; 4]); 3] = [
            ([8, 8], [255, 0, 0, 255]),
            ([56, 8], [0, 255, 0, 255]),
            ([32, 56], [0, 0, 255, 255]),
        ];
        for (i, (xy, rgba)) in verts.iter().enumerate() {
            let off = VTX_ADDR + i * 16;
            rdram[off..off + 2].copy_from_slice(&xy[0].to_be_bytes());
            rdram[off + 2..off + 4].copy_from_slice(&xy[1].to_be_bytes());
            rdram[off + 12..off + 16].copy_from_slice(rgba);
        }

        let mut dl = Vec::new();
        let w0 = ((gbi::G_VTX as u32) << 24) | (3u32 << 12);
        dl.extend_from_slice(&w0.to_be_bytes());
        dl.extend_from_slice(&(VTX_ADDR as u32).to_be_bytes());
        let w0 = (gbi::G_TRI1 as u32) << 24;
        let w1 = (1u32 << 8) | 2u32; // v0 index is 0, so its <<16 term is omitted (identity op)
        dl.extend_from_slice(&w0.to_be_bytes());
        dl.extend_from_slice(&w1.to_be_bytes());
        // A second ordered primitive forces the production ReferenceBackend
        // through one real continuation/resume boundary at this ABI seam.
        dl.extend_from_slice(&w0.to_be_bytes());
        dl.extend_from_slice(&w1.to_be_bytes());
        let w0 = (gbi::G_ENDDL as u32) << 24;
        dl.extend_from_slice(&w0.to_be_bytes());
        dl.extend_from_slice(&0u32.to_be_bytes());
        rdram[DL_ADDR..DL_ADDR + dl.len()].copy_from_slice(&dl);

        // OSTask_t header: type=M_GFXTASK@0x0, data_ptr=DL_ADDR@0x30.
        rdram[HEADER_OFF..HEADER_OFF + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        rdram[HEADER_OFF + 0x30..HEADER_OFF + 0x34]
            .copy_from_slice(&(DL_ADDR as u32).to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER_OFF, &mut ctx);
        let mut backend = ReferenceBackend::new().with_clear_color([1, 2, 3, 255]);
        backend.create(&RenderConfig::ntsc(64, 64)).unwrap();
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(Box::new(backend), rdram.len());
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert!(
            hle_render_needs_progress(),
            "the first real ReferenceBackend operation must retain its backend-owned continuation"
        );
        advance_hle_render_task();
        assert!(
            !hle_render_needs_progress(),
            "the second real ReferenceBackend operation must consume the continuation exactly once"
        );

        assert_eq!(
            last_render_error(),
            None,
            "the real backend must not report an error for a valid fixture -- rules out \
             NotReady/UnsupportedUcode/InvalidTaskBounds, i.e. the seam-routed call really \
             reached process_task and it really succeeded"
        );

        // `dyn RenderBackend` deliberately has no `Any` bound (keeping the
        // shared trait minimal per docs/DECOUPLING.md), so the registered
        // trait object's framebuffer can't be inspected back out through
        // this seam. Independently confirm the exact same fixture bytes
        // DO produce a non-clear frame via a second, directly-owned
        // `ReferenceBackend` (the same concrete type just registered,
        // exercised the same way `fn64-render-rt64/tests/fixture_replay.rs`
        // already proves in isolation) -- combined with the error-free
        // error-free StartGo result above, this closes the loop end-to-end:
        // the seam call really executed the real decode+rasterize path on
        // this fixture, not a silent no-op.
        let mut direct = ReferenceBackend::new().with_clear_color([1, 2, 3, 255]);
        direct.create(&RenderConfig::ntsc(64, 64)).unwrap();
        let task = fn64_render::OsTask {
            task_type: fn64_render::M_GFXTASK,
            data_ptr: DL_ADDR as u32,
            ..Default::default()
        };
        direct
            .process_task(&mut rdram, &mut fn64_runtime::RspMemory::new(), &task, 0)
            .unwrap();
        assert!(
            direct
                .framebuffer()
                .unwrap()
                .has_non_uniform_content(1, 2, 3, 255),
            "the same fixture bytes must produce a non-clear frame through the reference backend"
        );
        crate::advance_virtual_time(9);
    }


    #[test]
    fn os_sp_task_yielded_query_does_not_call_audio_ucode_again() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);
        static SEEN_UCODE_ADDR: AtomicU32 = AtomicU32::new(0);

        unsafe extern "C" fn fake_ucode(_rdram: *mut u8, task_offset: u32) -> u32 {
            CALLED.store(true, Ordering::SeqCst);
            SEEN_UCODE_ADDR.store(task_offset, Ordering::SeqCst);
            0
        }
        crate::load_rom(Vec::new());
        unsafe { set_translated_audio_ucode(fake_ucode, [0x51; 32]) };
        CALLED.store(false, Ordering::SeqCst);
        SEEN_UCODE_ADDR.store(0, Ordering::SeqCst);

        let mut rdram = vec![0u8; 128];
        let header_off = 0x20usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_AUDTASK.to_ne_bytes());
        rdram[header_off + 0x10..header_off + 0x14].copy_from_slice(&0xDEADu32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert!(!CALLED.load(Ordering::SeqCst));
        assert_eq!(SEEN_UCODE_ADDR.load(Ordering::SeqCst), 0);
    }


    /// Fail-against-bug: OoT's audio driver submits its `M_AUDTASK` via the
    /// Load+StartGo path (`AudioMgr_HandleRetrace` -> scheduler ->
    /// `Sched_RunTask` -> `osSpTaskLoad`+`osSpTaskStartGo`), NEVER the yield
    /// path. Before the fix, `osSpTaskStartGo_recomp` dispatched only
    /// `M_GFXTASK`, so a real audio task kicked here never ran its ucode -- the
    /// recompiled aspMain would never execute and no samples would be produced,
    /// even once the audio thread was submitting tasks. This asserts StartGo
    /// really invokes the registered ucode for `M_AUDTASK`, symmetric with the
    /// gfx-from-StartGo fix (commit 73a191a) and the yield-path test above.
    #[test]
    fn os_sp_task_start_go_calls_the_registered_audio_ucode_fn_for_real() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);
        static SEEN_OFFSET: AtomicU32 = AtomicU32::new(0);

        unsafe extern "C" fn fake_ucode(_rdram: *mut u8, task_offset: u32) -> u32 {
            CALLED.store(true, Ordering::SeqCst);
            SEEN_OFFSET.store(task_offset, Ordering::SeqCst);
            0
        }
        crate::load_rom(Vec::new());
        unsafe { set_translated_audio_ucode(fake_ucode, [0x52; 32]) };
        CALLED.store(false, Ordering::SeqCst);
        SEEN_OFFSET.store(0, Ordering::SeqCst);
        crate::set_trace_enabled(true);

        let mut rdram = vec![0u8; 128];
        let header_off = 0x30usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_AUDTASK.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        let prior_starts = crate::copy_trace()
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    fn64_runtime::TraceKind::TaskSubmit {
                        task_kind: fn64_runtime::TaskKind::Audio,
                        ..
                    }
                )
            })
            .count();
        admit_synthetic_hle_task(&mut rdram, header_off, &mut ctx);
        assert_eq!(
            crate::copy_trace()
                .iter()
                .filter(|event| {
                    matches!(
                        event.kind,
                        fn64_runtime::TraceKind::TaskSubmit {
                            task_kind: fn64_runtime::TaskKind::Audio,
                            ..
                        }
                    )
                })
                .count(),
            prior_starts,
            "audio admission alone cannot claim task execution"
        );
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert!(
            CALLED.load(Ordering::SeqCst),
            "osSpTaskStartGo must call the real ucode fn for an M_AUDTASK (the OoT path)"
        );
        assert_eq!(
            SEEN_OFFSET.load(Ordering::SeqCst),
            header_off as u32,
            "ucode receives the OSTask rdram offset"
        );
        assert_eq!(
            crate::copy_trace()
                .iter()
                .filter(|event| {
                    matches!(
                        event.kind,
                        fn64_runtime::TraceKind::TaskSubmit {
                            task_kind: fn64_runtime::TaskKind::Audio,
                            ..
                        }
                    )
                })
                .count(),
            prior_starts + 1,
            "audio StartGo must emit exactly one execution-qualified task trace"
        );
        crate::advance_virtual_time(8);
    }


    #[test]
    fn os_sp_task_start_go_dispatches_a_direct_4k_audio_image_without_rspboot() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);

        unsafe extern "C" fn fake_ucode(_rdram: *mut u8, _task_offset: u32) -> u32 {
            CALLED.store(true, Ordering::SeqCst);
            0
        }

        const HEADER: usize = 0x40;
        const IMAGE: usize = 0x200;
        crate::load_rom(Vec::new());
        unsafe { set_translated_audio_ucode(fake_ucode, [0x53; 32]) };
        CALLED.store(false, Ordering::SeqCst);
        let mut rdram = vec![0u8; IMAGE + fn64_runtime::RSP_MEMORY_BANK_SIZE];
        for (field, value) in [
            (0x00, fn64_runtime::M_AUDTASK),
            (0x08, 0x8000_0000 | IMAGE as u32),
            (0x0c, fn64_runtime::RSP_MEMORY_BANK_SIZE as u32),
            (0x10, 0xA000_0000 | IMAGE as u32),
            (0x14, fn64_runtime::RSP_MEMORY_BANK_SIZE as u32),
        ] {
            rdram[HEADER + field..HEADER + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        // A direct ucode is allowed to terminate with BREAK. If StartGo
        // mistakes this image for rspboot, this first word takes the existing
        // loud "BREAK before DMA-loaded ucode" trap instead of calling HLE.
        rdram[IMAGE..IMAGE + 4].copy_from_slice(&0x0000_000du32.to_ne_bytes());
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        unsafe {
            osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx);
            osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx);
        }

        assert!(
            CALLED.load(Ordering::SeqCst),
            "a complete direct IMEM audio image must enter its registered HLE backend"
        );
        assert_eq!(with_executor(|exec| exec.task_log().audio_count()), 1);
        crate::advance_virtual_time(1);
    }


    #[test]
    fn os_sp_task_yield_sets_the_public_sig0_request() {
        crate::load_rom(Vec::new());
        let mut rdram = [0u8; 4];
        let mut ctx = ctx_zeroed();

        unsafe { osSpTaskYield_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_ne!(
            crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELD,
            0
        );
        assert_eq!(
            crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELDED,
            0,
            "the CPU request must not fabricate the microcode acknowledgement"
        );
    }


    #[test]
    fn os_sp_task_yielded_prepares_the_saved_task_for_restart() {
        const HEADER_OFF: usize = 0x40;
        const FLAGS: u32 = 0x20;
        const OLD_UCODE_DATA: u32 = 0x1234;
        const OLD_UCODE_DATA_SIZE: u32 = 0x80;
        const YIELD_DATA: u32 = 0x4321;
        const YIELD_DATA_SIZE: u32 = 0x900;

        crate::load_rom(Vec::new());
        crate::pi::write_live_sp_status(fn64_runtime::SP_SET_YIELDED);
        let mut rdram = vec![0u8; HEADER_OFF + 0x40];
        for (field, value) in [
            (0x04, FLAGS),
            (0x18, OLD_UCODE_DATA),
            (0x1c, OLD_UCODE_DATA_SIZE),
            (0x38, YIELD_DATA),
            (0x3c, YIELD_DATA_SIZE),
        ] {
            rdram[HEADER_OFF + field..HEADER_OFF + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
        with_host(|host| {
            host.rsp_task_lineages.insert(
                HEADER_OFF as u32,
                RspTaskLineage {
                    admission_generation: RspTaskAdmissionGeneration::first(),
                    original_header: OsTaskHeader {
                        flags: FLAGS,
                        ucode_data: OLD_UCODE_DATA,
                        ucode_data_size: OLD_UCODE_DATA_SIZE,
                        yield_data_ptr: YIELD_DATA,
                        yield_data_size: YIELD_DATA_SIZE,
                        ..Default::default()
                    },
                    data_identity: None,
                    phase: RspTaskLineagePhase::Running,
                },
            );
        });

        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        let word = |field: usize| {
            u32::from_ne_bytes(
                rdram[HEADER_OFF + field..HEADER_OFF + field + 4]
                    .try_into()
                    .unwrap(),
            )
        };
        assert_eq!(ctx.r2, u64::from(fn64_runtime::OS_TASK_YIELDED));
        assert_eq!(word(0x04), FLAGS | fn64_runtime::OS_TASK_YIELDED);
        assert_eq!(word(0x18), YIELD_DATA);
        assert_eq!(word(0x1c), YIELD_DATA_SIZE);
        assert_eq!(
            crate::host_evidence_snapshot().rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized
        );
        assert_ne!(
            crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELDED,
            0,
            "the observation call must not invent an undocumented signal clear"
        );
    }


    #[test]
    fn os_sp_task_load_clears_stale_yield_handshake_bits() {
        const HEADER_OFF: usize = 0x40;
        const RSPBOOT_OFF: u32 = 0x100;

        crate::load_rom(Vec::new());
        crate::pi::write_live_sp_status(fn64_runtime::SP_SET_YIELD | fn64_runtime::SP_SET_YIELDED);
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER_OFF + 0x08..HEADER_OFF + 0x0c].copy_from_slice(&RSPBOOT_OFF.to_ne_bytes());
        rdram[HEADER_OFF + 0x0c..HEADER_OFF + 0x10].copy_from_slice(&8u32.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;

        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(
            crate::pi::live_sp_status()
                & (fn64_runtime::SP_STATUS_YIELD | fn64_runtime::SP_STATUS_YIELDED),
            0
        );
    }


    /// osSpTaskYielded, in this crate's synchronous run-to-completion model,
    /// must report task COMPLETED (0), not OS_TASK_YIELDED (1). Returning 1
    /// makes the scheduler re-queue an already-finished task forever. Fails
    /// against the bug (`ctx.r2 = 1`).
    #[test]
    fn os_sp_task_yielded_reports_completed_not_yielded() {
        crate::load_rom(Vec::new());
        // Minimal OSTask header at rdram offset 0x40, task_type = 0 (unknown:
        // recorded but no backend/ucode fired). Buffer covers base+0x38.
        let mut rdram = vec![0u8; 256];
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0040; // KSEG0 -> offset 0x40
        ctx.r2 = 0xFFFF_FFFF; // stale $v0.
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(
            ctx.r2, 0,
            "0 = completed (did not yield); 1 = OS_TASK_YIELDED"
        );
    }


    #[test]
    fn audio_digest_capture_distinguishes_unrequested_empty_and_real_pcm() {
        set_audio_digest_capture(false);
        assert_eq!(copy_audio_digest_bytes(), None);

        let mut rdram = vec![0u8; 8];
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_u16(RdramAddr::from_offset(0), 0x1234);
        view.write_u16(RdramAddr::from_offset(2), 0xfffe);
        set_audio_rdram_len(rdram.len());
        set_audio_digest_capture(true);
        unsafe { deliver_ai_buffer(rdram.as_mut_ptr(), 0, 4, None) };
        assert_eq!(
            copy_audio_digest_bytes(),
            Some(vec![0x34, 0x12, 0xfe, 0xff])
        );
        set_audio_digest_capture(false);
    }

    #[test]
    fn ai_pcm_decode_reuses_scratch_storage_across_buffers() {
        AUDIO_SAMPLE_SCRATCH.with(|cell| *cell.borrow_mut() = Vec::new());
        let mut rdram = vec![0u8; 16];
        set_audio_rdram_len(rdram.len());

        unsafe { deliver_ai_buffer(rdram.as_mut_ptr(), 0, 16, None) };
        let first = AUDIO_SAMPLE_SCRATCH.with(|cell| {
            let samples = cell.borrow();
            (samples.as_ptr(), samples.capacity())
        });
        unsafe { deliver_ai_buffer(rdram.as_mut_ptr(), 0, 4, None) };
        let second = AUDIO_SAMPLE_SCRATCH.with(|cell| {
            let samples = cell.borrow();
            (samples.as_ptr(), samples.capacity())
        });

        assert_eq!(second, first);
    }

    #[test]
    fn malformed_or_failed_raw_dpc_backend_result_poisons_without_publication() {
        use ScheduledRawDpcReply::{BackendError, WrongCursor, WrongQuantum, WrongTransaction};

        for reply in [BackendError, WrongTransaction, WrongQuantum, WrongCursor] {
            let mut transaction = scheduled_raw_dpc_transaction();
            let start = transaction.cursor();
            let mut backend = ScheduledRawDpcBackend::new([reply]);
            let mut live = vec![0x11; 16];
            let error = transaction
                .advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0)
                .unwrap_err();
            match error {
                ScheduledRawDpcError::Backend(error) => {
                    assert!(error.to_string().contains("injected failure"));
                }
                ScheduledRawDpcError::Schedule(_) => {}
                ScheduledRawDpcError::UnidentifiedFullSync => {
                    panic!("identity-mismatch cases cannot fail FullSync validation")
                }
            }
            assert_eq!(live, vec![0x11; 16]);
            assert_eq!(transaction.cursor(), start);
            assert_eq!(transaction.continuation(), None);
            assert_eq!(
                transaction.phase(),
                fn64_runtime::DpcScheduledPhase::Poisoned
            );
            assert!(matches!(
                transaction.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0,),
                Err(ScheduledRawDpcError::Schedule(
                    fn64_runtime::DpcScheduleError::Poisoned
                ))
            ));
            assert_eq!(backend.calls, 1, "a poisoned transaction cannot retry work");
        }
    }


    #[test]
    fn raw_dpc_status_must_match_remaining_schedule_before_publication() {
        let mut early = scheduled_raw_dpc_transaction();
        let mut backend = ScheduledRawDpcBackend::new([ScheduledRawDpcReply::Complete(
            fn64_render::DpFullSyncStatus::NotReached,
        )]);
        let mut live = vec![0x22; 16];
        assert!(matches!(
            early.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0),
            Err(ScheduledRawDpcError::Schedule(
                fn64_runtime::DpcScheduleError::EarlyComplete { .. }
            ))
        ));
        assert_eq!(live, vec![0x22; 16]);
        assert_eq!(early.phase(), fn64_runtime::DpcScheduledPhase::Poisoned);

        let mut final_continue = scheduled_raw_dpc_transaction();
        let mut backend = ScheduledRawDpcBackend::new([
            ScheduledRawDpcReply::Continue(fn64_render::DpFullSyncStatus::NotReached),
            ScheduledRawDpcReply::Continue(fn64_render::DpFullSyncStatus::NotReached),
        ]);
        let mut live = vec![0x33; 16];
        final_continue
            .advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0)
            .unwrap();
        let first_image = live.clone();
        assert!(matches!(
            final_continue.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0,),
            Err(ScheduledRawDpcError::Schedule(
                fn64_runtime::DpcScheduleError::FinalContinue { .. }
            ))
        ));
        assert_eq!(
            live, first_image,
            "the malformed final shadow stays private"
        );
        assert_eq!(
            final_continue.phase(),
            fn64_runtime::DpcScheduledPhase::Poisoned
        );
        assert_eq!(final_continue.continuation(), None);
    }


    #[test]
    fn second_raw_dpc_backend_error_preserves_only_the_first_commit() {
        let mut transaction = scheduled_raw_dpc_transaction();
        let mut backend = ScheduledRawDpcBackend::new([
            ScheduledRawDpcReply::Continue(fn64_render::DpFullSyncStatus::Reached),
            ScheduledRawDpcReply::BackendError,
        ]);
        let mut live = vec![0x55; 16];

        transaction
            .advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0)
            .unwrap();
        let first_image = live.clone();
        assert_eq!(first_image[0], 0xa1);
        assert_eq!(first_image[1], 0x55);
        assert_eq!(
            transaction.continuation(),
            Some(fn64_render::RenderRawDpcContinuation::new(91))
        );
        assert_eq!(
            transaction.full_sync(),
            fn64_render::DpFullSyncStatus::Reached
        );

        assert!(matches!(
            transaction.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0),
            Err(ScheduledRawDpcError::Backend(_))
        ));
        assert_eq!(
            backend.steps,
            vec![
                fn64_render::RawDpcStep::Start,
                fn64_render::RawDpcStep::Resume(fn64_render::RenderRawDpcContinuation::new(91)),
            ]
        );
        assert_eq!(live, first_image, "the second shadow stays unpublished");
        assert_eq!(live[1], 0x55, "the second backend mutation stayed private");
        assert_eq!(transaction.continuation(), None);
        assert_eq!(
            transaction.full_sync(),
            fn64_render::DpFullSyncStatus::Reached,
            "a rejected later quantum cannot erase prior committed FullSync evidence"
        );
        assert_eq!(
            transaction.phase(),
            fn64_runtime::DpcScheduledPhase::Poisoned
        );
        assert!(matches!(
            transaction.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0),
            Err(ScheduledRawDpcError::Schedule(
                fn64_runtime::DpcScheduleError::Poisoned
            ))
        ));
        assert_eq!(backend.calls, 2, "poison prevents a second-quantum retry");
    }


    #[test]
    fn raw_dpc_full_sync_is_identified_and_sticky_across_valid_commits() {
        let mut transaction = scheduled_raw_dpc_transaction();
        let mut backend = ScheduledRawDpcBackend::new([
            ScheduledRawDpcReply::Continue(fn64_render::DpFullSyncStatus::Reached),
            ScheduledRawDpcReply::Complete(fn64_render::DpFullSyncStatus::NotReached),
        ]);
        let mut live = vec![0; 16];
        for expected_phase in [
            fn64_runtime::DpcScheduledPhase::Scheduled,
            fn64_runtime::DpcScheduledPhase::Complete,
        ] {
            assert!(matches!(
                transaction
                    .advance_one(
                        fn64_runtime::Cycles::new(10),
                        &mut backend,
                        &mut live,
                        0,
                    )
                    .unwrap(),
                ScheduledRawDpcAdvance::Committed {
                    phase,
                    full_sync: fn64_render::DpFullSyncStatus::Reached,
                    ..
                } if phase == expected_phase
            ));
        }

        let mut unidentified = scheduled_raw_dpc_transaction();
        let mut backend = ScheduledRawDpcBackend::new([ScheduledRawDpcReply::Continue(
            fn64_render::DpFullSyncStatus::Unidentified,
        )]);
        let mut live = vec![0x44; 16];
        assert!(matches!(
            unidentified.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0,),
            Err(ScheduledRawDpcError::UnidentifiedFullSync)
        ));
        assert_eq!(live, vec![0x44; 16]);
        assert_eq!(
            unidentified.phase(),
            fn64_runtime::DpcScheduledPhase::Poisoned
        );
    }


    #[test]
    fn synthetic_scheduled_dpc_keeps_renderer_continuation_in_the_abi_lane() {
        struct Backend;

        impl RenderBackend for Backend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            fn observe_non_rdp_write16(
                &mut self,
                _write: fn64_render::NonRdpWrite16,
            ) -> fn64_render::NonRdpWrite16Disposition {
                fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar
            }

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                unreachable!("synthetic raw-DPC test cannot dispatch an HLE task")
            }

            fn raw_dpc_progression(&self) -> fn64_render::RawDpcProgression {
                fn64_render::RawDpcProgression::Acknowledged
            }

            fn process_rdp_command_chunk(
                &mut self,
                rdram: &mut [u8],
                quantum: fn64_render::RawDpcQuantum,
                step: fn64_render::RawDpcStep,
            ) -> Result<fn64_render::RawDpcChunkAck, RenderError> {
                let index = usize::try_from(quantum.request.start.address() - 0x100).unwrap();
                rdram[index] = quantum.request.quantum.get() as u8;
                let status = match step {
                    fn64_render::RawDpcStep::Start => fn64_render::RawDpcChunkStatus::Continue(
                        fn64_render::RenderRawDpcContinuation::new(77),
                    ),
                    fn64_render::RawDpcStep::Resume(token) if token.get() == 77 => {
                        fn64_render::RawDpcChunkStatus::Complete
                    }
                    fn64_render::RawDpcStep::Resume(token) => {
                        panic!("ABI supplied stale raw-DPC continuation {}", token.get())
                    }
                };
                Ok(fn64_render::RawDpcChunkAck {
                    transaction: quantum.request.transaction,
                    quantum: quantum.request.quantum,
                    committed_through: quantum.request.end,
                    status,
                    full_sync: fn64_render::DpFullSyncStatus::NotReached,
                })
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        let source = fn64_runtime::DpcSubmissionSource::Rdram;
        let cursor = |address| fn64_runtime::DpcCursor::new(source, address).unwrap();
        let execution = fn64_runtime::DpcScheduledExecution::new(
            fn64_runtime::DpcSubmission {
                token: 5,
                source,
                start: 0x100,
                end: 0x110,
            },
            fn64_runtime::Cycles::new(0),
            vec![
                fn64_runtime::DpcQuantumPlan {
                    at: fn64_runtime::Cycles::new(2),
                    id: fn64_runtime::DpcQuantumId::new(1),
                    start: cursor(0x100),
                    end: cursor(0x108),
                },
                fn64_runtime::DpcQuantumPlan {
                    at: fn64_runtime::Cycles::new(3),
                    id: fn64_runtime::DpcQuantumId::new(2),
                    start: cursor(0x108),
                    end: cursor(0x110),
                },
            ],
        )
        .unwrap();
        let mut backend = Backend;
        let mut transaction = ScheduledRawDpcTransaction::new(execution);
        let mut live = vec![0u8; 16];

        for (expected_at, expected_phase) in [
            (2, fn64_runtime::DpcScheduledPhase::Scheduled),
            (3, fn64_runtime::DpcScheduledPhase::Complete),
        ] {
            assert_eq!(
                transaction
                    .advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0,)
                    .unwrap(),
                ScheduledRawDpcAdvance::Committed {
                    at: fn64_runtime::Cycles::new(expected_at),
                    phase: expected_phase,
                    full_sync: fn64_render::DpFullSyncStatus::NotReached,
                }
            );
        }
        assert_eq!(live[0], 1);
        assert_eq!(live[8], 2);
        assert_eq!(transaction.continuation(), None);
        assert_eq!(
            transaction.phase(),
            fn64_runtime::DpcScheduledPhase::Complete
        );
    }


    /// The batched `canonical_rdp_words_sha256` must produce the exact value
    /// the per-word `Digest::update` loop produced, because that value ships
    /// as `command_sha256` in release-gate evidence.
    ///
    /// The reference here is the literal pre-batching implementation, not a
    /// recomputation through the same chunking, so a chunking bug cannot
    /// cancel out on both sides.
    #[test]
    fn canonical_rdp_words_sha256_matches_per_word_updates() {
        fn per_word_reference(words: &[u32]) -> [u8; 32] {
            let mut digest = Sha256::new();
            for word in words {
                digest.update(word.to_be_bytes());
            }
            digest.finalize().into()
        }

        // Lengths straddling the 1024-word chunk boundary: empty, short,
        // exactly one chunk, one over, and several chunks plus a remainder.
        for len in [0usize, 1, 2, 7, 1023, 1024, 1025, 2048, 3000] {
            let words: Vec<u32> = (0..len)
                .map(|index| (index as u32).wrapping_mul(0x9e37_79b9) ^ 0xe900_0000)
                .collect();
            assert_eq!(
                canonical_rdp_words_sha256(&words),
                per_word_reference(&words),
                "batched RDP command digest diverged from the per-word loop at {len} words"
            );
        }
    }

    /// Byte order is load-bearing: the digest is defined over the BIG-endian
    /// image of each word. A host-order staging bug would pass the identity
    /// test above only if the reference had the same bug, so pin one literal
    /// vector against an independently written big-endian byte sequence.
    #[test]
    fn canonical_rdp_words_sha256_digests_the_big_endian_image() {
        let words = [0xe900_0000u32, 0x0000_0001, 0xdead_beef];
        let expected: [u8; 32] = Sha256::digest([
            0xe9, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xde, 0xad, 0xbe, 0xef,
        ])
        .into();
        assert_eq!(canonical_rdp_words_sha256(&words), expected);
    }

    /// `task_microcode_data_identity` batches its `Digest::update` calls, but
    /// the sequence it digests is the LOGICAL byte order produced by
    /// `RdramPtr::read_u8`, which reads storage index `addr ^ 3`. Chunking
    /// over raw storage instead would reorder every 4-byte group and change
    /// `data_sha256` in release-gate evidence.
    ///
    /// The span deliberately starts at an unaligned logical address and runs
    /// past the 4096-byte staging chunk, so both the lane mapping and the
    /// chunk boundary are exercised.
    #[test]
    fn microcode_data_identity_batches_the_swizzled_logical_order() {
        const DATA: u32 = 0x1001;
        const LEN: usize = 5000;
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x4000];
        let payload: Vec<u8> = (0..LEN).map(|index| (index % 251) as u8).collect();
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in payload.iter().copied().enumerate() {
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
                DATA,
                LEN as u32,
            )
        };

        // Independent per-byte reference through the same accessor: this is
        // the literal pre-batching loop.
        let mut reference = Sha256::new();
        {
            let memory = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
            for offset in 0..LEN as u32 {
                reference.update([unsafe {
                    memory.read_u8(RdramAddr::from_offset(DATA).checked_add(offset).unwrap())
                }]);
            }
        }
        let reference: [u8; 32] = reference.finalize().into();
        assert_eq!(identity.sha256, reference);
        // And the logical order is the payload as written, not lane order.
        let expected: [u8; 32] = Sha256::digest(&payload).into();
        assert_eq!(identity.sha256, expected);
    }

    // --- DPC submission coalescing -------------------------------------
    //
    // `coalesce_dp_submissions` is the one site that decides where a
    // hardware command stream begins and ends. Everything downstream
    // (`request_dpc_submission`, `OwnedRawDpcSubmission::from_*`) trusts
    // its `start..end` to describe exactly the words it hands over.

    fn xbus(start: u32, end: u32, fill: u8) -> fn64_audio::rsp::runtime::RspDpSubmission {
        fn64_audio::rsp::runtime::RspDpSubmission::from_xbus_bytes(
            start,
            end,
            vec![fill; (end - start) as usize],
        )
    }

    fn rdram_words(start: u32, end: u32, fill: u32) -> fn64_audio::rsp::runtime::RspDpSubmission {
        fn64_audio::rsp::runtime::RspDpSubmission::from_rdram_words(
            start,
            end,
            vec![fill; ((end - start) / 4) as usize],
        )
    }

    /// The straddle case `7ef65d54` was written for: F3DEX xbus extends its
    /// run 8 bytes per END write, and a 16-byte command spans two of them.
    /// Contiguous XBUS submissions must still merge into one stream, or the
    /// decoder traps on a truncation hardware stalls through.
    #[test]
    fn contiguous_xbus_submissions_merge_into_one_stream() {
        let runs = coalesce_dp_submissions(vec![
            xbus(0x0ba8, 0x0bb0, 0xa1),
            xbus(0x0bb0, 0x0bb8, 0xa2),
            xbus(0x0bb8, 0x0bc0, 0xa3),
        ]);
        assert_eq!(runs.len(), 1, "one contiguous XBUS run is one stream");
        assert_eq!((runs[0].start, runs[0].end), (0x0ba8, 0x0bc0));
        assert!(runs[0].xbus);
        assert_eq!(runs[0].words.len(), 6);
        assert_eq!(
            runs[0].words,
            [
                0xa1a1_a1a1,
                0xa1a1_a1a1,
                0xa2a2_a2a2,
                0xa2a2_a2a2,
                0xa3a3_a3a3,
                0xa3a3_a3a3
            ],
            "the merged stream is every submission's bytes, in submission order"
        );
    }

    /// The measured WM2000 defect, in miniature. The graphics ucode fills a
    /// DMEM command ring and wraps back to its base; the wrap is a new START,
    /// so it opens a new stream. Coalescing through it accumulated all the
    /// bytes under the first range's `start..end` and the capture refused the
    /// result as `XbusPayloadLength { expected: 752, actual: 3400 }`.
    ///
    /// Mutation control: deleting `&& submission.start == end` from the XBUS
    /// arm of `coalesce_dp_submissions` makes this one run of 3 words over
    /// `[0x0f10, 0x0bb0)`, which the run's own length assertion rejects.
    #[test]
    fn an_xbus_ring_wrap_opens_a_new_stream() {
        let runs = coalesce_dp_submissions(vec![
            xbus(0x0f08, 0x0f10, 0xb1),
            xbus(0x0f10, 0x0f18, 0xb2),
            // ring wrap: END jumped backwards to the ring base
            xbus(0x0ba8, 0x0bb0, 0xb3),
        ]);
        assert_eq!(runs.len(), 2, "the ring wrap breaks the run");
        assert_eq!((runs[0].start, runs[0].end), (0x0f08, 0x0f18));
        assert_eq!(runs[0].words.len(), 4);
        assert_eq!((runs[1].start, runs[1].end), (0x0ba8, 0x0bb0));
        assert_eq!(runs[1].words.len(), 2);
        for run in &runs {
            assert_eq!(
                (run.end - run.start) as usize,
                run.words.len() * 4,
                "every emitted run describes exactly its own bytes"
            );
        }
    }

    /// Positive control for the fixture above: the wrapped submission list is
    /// genuinely non-contiguous, so the ring-wrap test is exercising the new
    /// adjacency test and not passing for an unrelated reason.
    #[test]
    fn the_ring_wrap_fixture_is_actually_non_contiguous() {
        let submissions = vec![
            xbus(0x0f08, 0x0f10, 0xb1),
            xbus(0x0f10, 0x0f18, 0xb2),
            xbus(0x0ba8, 0x0bb0, 0xb3),
        ];
        assert!(submissions.iter().all(|submission| submission.is_xbus()));
        assert_eq!(submissions[1].end - submissions[2].start, 0x370);
        assert_ne!(
            submissions[1].end, submissions[2].start,
            "the fixture must contain a discontinuity for the adjacency test to catch"
        );
        let total: u32 = submissions
            .iter()
            .map(|submission| submission.end - submission.start)
            .sum();
        assert_eq!(total, 24, "three 8-byte submissions carry 24 bytes");
        // The span a predicate-only coalescer would report -- first `start`
        // to last `end` -- runs backwards across the wrap, so it cannot equal
        // the payload it would be attached to under any interpretation.
        assert!(
            submissions[2].end < submissions[0].start,
            "the wrapped run's reported span is reversed, not merely short"
        );
    }

    /// Whichever source, the rule is one rule. This is the RDRAM half of the
    /// same behavior, kept beside the XBUS half so a future asymmetry has to
    /// break a test that states both.
    #[test]
    fn a_non_adjacent_rdram_range_opens_a_new_stream() {
        let runs = coalesce_dp_submissions(vec![
            rdram_words(0x100, 0x108, 0x11),
            rdram_words(0x108, 0x110, 0x22),
            rdram_words(0x200, 0x208, 0x33),
        ]);
        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].start, runs[0].end), (0x100, 0x110));
        assert_eq!((runs[1].start, runs[1].end), (0x200, 0x208));
        assert!(runs.iter().all(|run| !run.xbus));
    }

    /// A source change is a stream break even at an adjacent address: the two
    /// arms retain incompatible representations (DMEM bytes vs canonical
    /// words) and `DpcSubmissionSource` routes them differently.
    #[test]
    fn a_source_change_breaks_a_run_at_an_adjacent_address() {
        let runs = coalesce_dp_submissions(vec![
            xbus(0x100, 0x108, 0xc1),
            rdram_words(0x108, 0x110, 0x44),
            xbus(0x110, 0x118, 0xc2),
        ]);
        assert_eq!(runs.len(), 3);
        assert_eq!(
            runs.iter().map(|run| run.xbus).collect::<Vec<_>>(),
            [true, false, true]
        );
    }

    /// Every emitted run satisfies the invariant the capture layer enforces,
    /// over an interleaved list that exercises both arms and both break
    /// reasons at once.
    #[test]
    fn every_coalesced_run_describes_exactly_its_own_bytes() {
        let runs = coalesce_dp_submissions(vec![
            xbus(0x0e00, 0x0e08, 0x01),
            xbus(0x0e08, 0x0e18, 0x02),
            xbus(0x0ba8, 0x0bb0, 0x03),
            rdram_words(0x2000, 0x2010, 0x55),
            rdram_words(0x2010, 0x2018, 0x66),
            rdram_words(0x3000, 0x3008, 0x77),
            xbus(0x0bb0, 0x0bb8, 0x04),
        ]);
        assert_eq!(runs.len(), 5);
        for run in &runs {
            assert_eq!(
                u64::from(run.end - run.start),
                run.words.len() as u64 * 4,
                "run [{:#010x}, {:#010x}) xbus={} carries {} words",
                run.start,
                run.end,
                run.xbus,
                run.words.len()
            );
        }
        assert_eq!(
            runs.iter()
                .map(|run| (run.start, run.end))
                .collect::<Vec<_>>(),
            [
                (0x0e00, 0x0e18),
                (0x0ba8, 0x0bb0),
                (0x2000, 0x2018),
                (0x3000, 0x3008),
                (0x0bb0, 0x0bb8),
            ]
        );
    }

    /// A single submission is a run of one, and an empty task emits nothing.
    #[test]
    fn degenerate_submission_lists_coalesce_without_special_cases() {
        assert!(coalesce_dp_submissions(Vec::new()).is_empty());
        let runs = coalesce_dp_submissions(vec![xbus(0x0ba8, 0x0bb0, 0xd1)]);
        assert_eq!(runs.len(), 1);
        assert_eq!(
            (runs[0].start, runs[0].end, runs[0].words.len()),
            (0x0ba8, 0x0bb0, 2)
        );
    }
