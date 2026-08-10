use super::*;

    #[test]
    fn canonical_publication_static_break_replaces_exact_with_parked_fault() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let mut bytes = vec![0u8; 0x1000];
        let thread_id = 0xb4eb;

        // SAFETY: `bytes` remains live while the deliberately stopped thread
        // retains its dormant coroutine.
        unsafe {
            boot_thread0_catalog_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                canonical_brk_install(),
                test_boot_context(BRK_ENTRY),
                thread_id,
                10,
            );
        }
        with_host(|host| {
            host.thread_handle_vrams.insert(thread_id, 0x8000_0200);
        });

        assert_canonical_break_parks_with_post_exception_publication(thread_id);
    }


    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_publication_dynamic_break_replaces_exact_with_parked_fault() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let mut bytes = vec![0u8; 0x1000];
        let thread_id = 0xb4ec;

        // SAFETY: `bytes` remains live while the deliberately stopped thread
        // retains its dormant coroutine.
        unsafe {
            boot_thread0_catalog_program_with_dynamic_mapped_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                canonical_brk_install(),
                test_boot_context(BRK_ENTRY),
                thread_id,
                10,
            );
        }
        with_host(|host| {
            host.thread_handle_vrams.insert(thread_id, 0x8000_0200);
        });

        assert_canonical_break_parks_with_post_exception_publication(thread_id);
    }


    #[test]
    fn block_program_vectors_mid_function_break_instead_of_panicking() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let mut bytes = vec![0u8; 0x1000];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(BRK_BANK, BRK_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new(BRK_BANK, brk_runner),
            )
            .unwrap();
        let thread_id = 0xB4EA;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(BRK_BANK, BRK_ENTRY),
                test_boot_context(BRK_ENTRY),
                brk_lookup,
                brk_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        // Runs to completion — reaching the handler and returning — rather than
        // hitting recompiled_gap_panic on the BREAK fault. The entry block, the
        // vectored handler, and the thread-return retire across steps; drive the
        // executor until the thread is dead (bounded so a regression can't spin).
        let mut steps = 0;
        while !crate::is_thread_dead(thread_id) {
            assert!(
                crate::run_one_step(),
                "executor stalled before thread return"
            );
            steps += 1;
            assert!(
                steps < 8,
                "BREAK vectoring did not converge to thread return"
            );
        }

        let mem = Rdram::new(&mut bytes);
        // EPC captured the faulting PC, and Cause.ExcCode == 9 (Breakpoint).
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000) as u32, BRK_ENTRY.get());
        assert_eq!((mem.load_w(0xFFFF_FFFF_8000_0004) as u32 >> 2) & 0x1F, 9);
    }


    #[test]
    fn checkpoint_due_pi_enters_ip2_handler_before_the_next_guest_block() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        crate::load_rom_with_fixed_pi_latency(rom, 5);
        let mut bytes = vec![0u8; 0x1000];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(IRQ_BANK, IRQ_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new(IRQ_BANK, irq_runner),
            )
            .unwrap();
        let thread_id = 0x1A2;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(IRQ_BANK, IRQ_ENTRY),
                test_boot_context(IRQ_ENTRY),
                irq_lookup,
                irq_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400) as u32, 0x1234_5678);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0);
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 7);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000) as u32, IRQ_RESUME.get());
            assert_eq!((mem.load_w(0xFFFF_FFFF_8000_0004) as u32 >> 2) & 0x1F, 0);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_8000_0004) as u32 & CpuInterruptLine::RCP.cause_bit(),
                0
            );
            assert_ne!(mem.load_w(0xFFFF_FFFF_8000_0008) as u32 & (1 << 1), 0);
        }

        assert!(crate::run_one_step());
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_8000_000C) as u32 & CpuInterruptLine::RCP.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0010) as u32 & (1 << 1), 0);
        }
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }


    #[test]
    fn checkpoint_count_compare_match_enters_ip7_and_compare_write_acks_it() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let mut bytes = vec![0u8; 0x100];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(TIMER_BANK, IRQ_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new(TIMER_BANK, timer_runner),
            )
            .unwrap();
        let thread_id = 0x1A7;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(TIMER_BANK, IRQ_ENTRY),
                test_boot_context(IRQ_ENTRY),
                timer_lookup,
                timer_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 4);
        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 6);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0020) as u32, IRQ_RESUME.get());
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_8000_0024) as u32 & CpuInterruptLine::TIMER.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0028) as u32, 2);
        }

        assert!(crate::run_one_step());
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_8000_002C) as u32 & CpuInterruptLine::TIMER.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0030) as u32, 3);
        }
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }


    #[test]
    fn status_adapters_are_per_context_state() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        ctx.set_r(4, 0x3400_0001);
        os_set_sr(&mut ctx, &mut mem);
        ctx.set_r(2, 0);
        os_get_sr(&mut ctx, &mut mem);
        assert_eq!(ctx.r_u32(2), 0x3400_0001);
    }


    #[test]
    fn typed_fpcsr_setter_and_new_thread_use_the_generated_cop1_authority() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut first = new_osthread_context(None);
        let mut second = new_osthread_context(None);

        assert_eq!(first.read_fcr(31), INITIAL_FPCSR);
        assert_eq!(second.read_fcr(31), INITIAL_FPCSR);

        first.set_r(4, 3);
        os_set_fpc_csr(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), INITIAL_FPCSR);
        assert_eq!(first.read_fcr(31), 3);
        assert_eq!(second.read_fcr(31), INITIAL_FPCSR);

        second.set_r(4, 2);
        os_set_fpc_csr(&mut second, &mut mem);
        assert_eq!(second.r_u32(2), INITIAL_FPCSR);
        assert_eq!(second.read_fcr(31), 2);
        assert_eq!(first.read_fcr(31), 3);

        let pending: u32 = (1 << 16) | (1 << 11);
        first.set_r(4, u64::from(pending));
        let loud = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            os_set_fpc_csr(&mut first, &mut mem);
        }));
        assert!(
            loud.is_err(),
            "enabled Cause written by host call must stay loud"
        );
        assert_eq!(first.r_u32(2), 3);
        assert_eq!(first.read_fcr(31), pending);
        assert_eq!(second.read_fcr(31), 2);
    }


    /// Public osCreateThread gives each OSThread its own saved FPCSR. This
    /// drives real executor coroutine suspension and alternates A/B/A/B/A/B;
    /// the context-local values must survive switches through another thread.
    #[test]
    fn alternating_osthread_coroutines_preserve_independent_fpcsr() {
        const THREAD_A: ThreadId = 0xF5A0;
        const THREAD_B: ThreadId = 0xF5B0;

        let observed_a = Rc::new(RefCell::new(Vec::new()));
        let observed_b = Rc::new(RefCell::new(Vec::new()));
        let observed_a_body = Rc::clone(&observed_a);
        let observed_b_body = Rc::clone(&observed_b);

        with_executor(|exec| {
            exec.create_thread(THREAD_A, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = new_osthread_context(None);
                ctx.write_fcr(31, 3);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
                ctx.write_fcr(31, 1);
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
            });
            exec.create_thread(THREAD_B, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = new_osthread_context(None);
                ctx.write_fcr(31, 2);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
                ctx.write_fcr(31, 0);
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
            });
            exec.start_thread(THREAD_A);
            exec.start_thread(THREAD_B);
        });

        for _ in 0..6 {
            assert!(crate::run_one_step());
        }

        assert_eq!(&*observed_a.borrow(), &[3, 3, 1]);
        assert_eq!(&*observed_b.borrow(), &[2, 2, 0]);
        with_executor(|exec| {
            assert!(exec.is_thread_dead(THREAD_A));
            assert!(exec.is_thread_dead(THREAD_B));
        });
    }


    /// Thread 0 is the reset context, not an osCreateThread context. The
    /// public osInitialize contract performs the observable 0 -> FS|EV
    /// transition at the real typed boot entry.
    #[test]
    fn thread0_boot_path_transitions_fpcsr_only_at_os_initialize() {
        const THREAD0: ThreadId = 0xF500;
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom_with_fixed_pi_latency(Vec::new(), 1);
        BOOT_FPCSR_OBSERVATIONS.with(|observed| observed.borrow_mut().clear());
        let mut bytes = [0u8; 8];

        unsafe {
            boot_thread0(
                bytes.as_mut_ptr(),
                bytes.len(),
                evidence_lookup,
                observe_thread0_fpcsr_boot,
                THREAD0,
                10,
            );
        }
        crate::run_to_idle();

        BOOT_FPCSR_OBSERVATIONS.with(|observed| {
            assert_eq!(&*observed.borrow(), &[0, INITIAL_FPCSR]);
        });
        assert!(crate::is_thread_dead(THREAD0));
    }


    #[test]
    fn typed_os_initialize_replaces_the_current_context_fpcsr() {
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom_with_fixed_pi_latency(Vec::new(), 1);
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        ctx.write_fcr(31, 3);

        os_initialize(&mut ctx, &mut mem);

        assert_eq!(ctx.read_fcr(31), INITIAL_FPCSR);
    }


    #[test]
    fn created_osthread_enters_fr0_without_discarding_other_status_fields() {
        let inherited = 0xA5A5_5A5A | STATUS_FR;

        let ctx = new_osthread_context(Some(inherited));

        assert_eq!(ctx.cop0_status, inherited & !STATUS_FR);
        assert_eq!(ctx.read_fcr(31), INITIAL_FPCSR);
    }


    #[test]
    fn alternating_osthread_coroutines_preserve_all_physical_fgr_bits() {
        const THREAD_A: ThreadId = 0xF5C0;
        const THREAD_B: ThreadId = 0xF5D0;
        let state_a = patterned_fgr_state(0x1111_2222_3333_4444);
        let state_b = patterned_fgr_state(0xAAAA_BBBB_CCCC_DDDD);
        let observed_a = Rc::new(RefCell::new(Vec::new()));
        let observed_b = Rc::new(RefCell::new(Vec::new()));
        let observed_a_body = Rc::clone(&observed_a);
        let observed_b_body = Rc::clone(&observed_b);

        with_executor(|exec| {
            exec.create_thread(THREAD_A, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = RsContext::new();
                ctx.cop0_status &= !STATUS_FR;
                ctx.replace_physical_fgr_state(state_a);
                observed_a_body.borrow_mut().push(ctx.physical_fgr_state());
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.physical_fgr_state());
            });
            exec.create_thread(THREAD_B, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = RsContext::new();
                ctx.cop0_status |= STATUS_FR;
                ctx.replace_physical_fgr_state(state_b);
                observed_b_body.borrow_mut().push(ctx.physical_fgr_state());
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.physical_fgr_state());
            });
            exec.start_thread(THREAD_A);
            exec.start_thread(THREAD_B);
        });

        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::run_one_step());

        assert_eq!(&*observed_a.borrow(), &[state_a, state_a]);
        assert_eq!(&*observed_b.borrow(), &[state_b, state_b]);
        with_executor(|exec| {
            assert!(exec.is_thread_dead(THREAD_A));
            assert!(exec.is_thread_dead(THREAD_B));
        });
    }


    #[test]
    fn typed_interrupt_masks_return_each_contexts_own_previous_value() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut first = RsContext::new();
        let mut second = RsContext::new();

        first.set_r(4, 0x0010_0401);
        os_set_int_mask(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), 0);
        second.set_r(4, 0x0008_0401);
        os_set_int_mask(&mut second, &mut mem);
        assert_eq!(second.r_u32(2), 0);
        first.set_r(4, 0x0004_0401);
        os_set_int_mask(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), 0x0010_0401);
    }


    #[test]
    fn typed_raw_word_accesses_and_sp_shims_share_one_device_fabric_state() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);

        mem.store_w(0xFFFF_FFFF_A408_0000, 0x0A8);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A408_0000) as u32, 0x0A8);

        let mut set = CContext::zeroed();
        set.r4 = 1 << 10;
        unsafe { crate::__osSpSetStatus_recomp(std::ptr::null_mut(), &mut set) };
        assert_eq!(mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & (1 << 7), 1 << 7);

        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }


    #[test]
    fn typed_raw_sp_dma_replaces_persistent_imem_on_guest_time() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let mut bytes = vec![0u8; 0x1000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut bytes);
            for (index, byte) in [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80]
                .into_iter()
                .enumerate()
            {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(0x100 + index as u32),
                    byte,
                );
            }
        }
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A404_0000, 0x1000);
            mem.store_w(0xFFFF_FFFF_A404_0004, 0x100);
            mem.store_w(0xFFFF_FFFF_A404_0008, 7);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & fn64_runtime::SP_STATUS_DMA_BUSY,
                0
            );
        }

        crate::advance_virtual_time(8);
        {
            let mem = Rdram::new(&mut bytes);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & fn64_runtime::SP_STATUS_DMA_BUSY,
                0
            );
        }
        crate::advance_virtual_time(9);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & 4, 0);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A400_1000) as u32, 0x1020_3040);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A400_1004) as u32, 0x5060_7080);
        }
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().sp_imem_generation),
            1
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }


    #[test]
    fn typed_raw_pi_registers_drive_the_live_timed_device_fabric() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        crate::load_rom_with_fixed_pi_latency(rom, 5);
        let mut bytes = vec![0u8; 0x1000];
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A460_0000, 0x400);
            mem.store_w(0xFFFF_FFFF_A460_0004, 0x1000_0020);
            mem.store_w(0xFFFF_FFFF_A460_000C, 3);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_A460_0010) as u32,
                fn64_runtime::PI_STATUS_DMA_BUSY
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400), 0);
        }

        crate::advance_virtual_time(4);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400), 0);
        }
        crate::advance_virtual_time(5);

        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400) as u32, 0x1234_5678);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A460_0010), 0);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Pi.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }


    #[test]
    fn typed_raw_rcp_acknowledgements_clear_the_shared_mi_sources() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let sources = [
            fn64_runtime::InterruptSource::Sp,
            fn64_runtime::InterruptSource::Si,
            fn64_runtime::InterruptSource::Ai,
            fn64_runtime::InterruptSource::Vi,
            fn64_runtime::InterruptSource::Dp,
        ];
        with_host(|host| {
            let fabric = &mut host.device_fabric;
            for source in sources {
                fabric.raise_interrupt(source);
            }
        });

        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A404_0010, 1 << 3);
        mem.store_w(0xFFFF_FFFF_A480_0018, 0);
        mem.store_w(0xFFFF_FFFF_A450_000C, 0);
        mem.store_w(0xFFFF_FFFF_A440_0010, 0);
        mem.store_w(0xFFFF_FFFF_A430_0000, 1 << 11);

        let pending = with_host(|host| host.device_fabric.snapshot().mi_pending);
        assert_eq!(pending & 0x3F, 0);
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }


    #[test]
    fn typed_raw_vi_registers_drive_half_line_timing_and_shared_mi() {
        crate::test_support::install_complete_render_backend(
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        );
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A440_0018, 525);
        mem.store_w(0xFFFF_FFFF_A440_000C, 100);
        crate::vi::arm_vi_retrace(1_000);

        crate::advance_virtual_time(190);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 98);
        crate::advance_virtual_time(191);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 100);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Vi.bit(),
            0
        );

        mem.store_w(0xFFFF_FFFF_A440_0010, 0xFFFF_FFFF);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 100);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Vi.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }


    #[test]
    fn typed_raw_ai_registers_schedule_the_live_guest_cycle_fifo() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A450_0008, 1);
        mem.store_w(0xFFFF_FFFF_A450_0010, 151);
        mem.store_w(0xFFFF_FFFF_A450_0000, 0x1000);
        mem.store_w(0xFFFF_FFFF_A450_0004, 0x80);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A450_000C) as u32 & fn64_runtime::AI_STATUS_BUSY,
            0
        );
        let deadline = with_host(|host| host.device_fabric.next_deadline().unwrap().get());
        crate::advance_virtual_time(deadline);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_A450_000C) as u32,
            fn64_runtime::AI_STATUS_ENABLED
        );
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().mi_pending)
                & fn64_runtime::InterruptSource::Ai.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }


    #[test]
    fn typed_raw_si_registers_run_separate_timed_pif_write_and_read_dmas() {
        let mut bytes = vec![0u8; 0x200];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut bytes);
            for (offset, byte) in [(0, 1), (1, 3), (2, 0xFF), (3, 0), (6, 0xFE)] {
                view.write_u8(fn64_runtime::RdramAddr::from_offset(offset), byte);
            }
        }
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A480_0000, 0);
            mem.store_w(0xFFFF_FFFF_A480_0010, 0);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A480_0018) & 1, 1);
        }
        crate::advance_virtual_time(1);
        {
            let mut mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A480_0018) as u32, 1 << 12);
            mem.store_w(0xFFFF_FFFF_A480_0018, 0);
            mem.store_w(0xFFFF_FFFF_A480_0000, 0);
            mem.store_w(0xFFFF_FFFF_A480_0004, 0);
        }
        crate::advance_virtual_time(2);
        let view = fn64_runtime::RdramView::from_storage(&bytes);
        assert_eq!(
            (3..6)
                .map(|offset| view.read_u8(fn64_runtime::RdramAddr::from_offset(offset)))
                .collect::<Vec<_>>(),
            vec![0x05, 0, 0]
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }
