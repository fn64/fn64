    use super::*;
    use crate::test_support::*;

    fn install_cart_handle(rdram: &mut [u8], offset: u32) -> u64 {
        let handle_vram = 0x8000_0000 | offset;
        set_cart_rom_handle_vram(handle_vram);
        let mut ctx = ctx_zeroed();
        unsafe { osCartRomInit_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2 as u32, handle_vram);
        ctx.r2
    }

    fn install_sram_handle(
        rdram: &mut [u8],
        offset: u32,
        timing: fn64_runtime::PiDomainTiming,
    ) -> u64 {
        let handle_vram = 0x8000_0000 | offset;
        unsafe {
            write_epi_handle(
                rdram.as_mut_ptr(),
                handle_vram,
                DEVICE_TYPE_SRAM,
                fn64_runtime::PiDomain::Domain2,
                timing,
                0xa800_0000,
            )
        };
        handle_vram as i32 as u64
    }

    #[test]
    fn loading_a_rom_clears_prior_rsp_rdp_observations() {
        load_rom(vec![0]);
        crate::record_rsp_rdp_observations(vec![crate::RspRdpObservationKind::DramDpcCommitted {
            start: 0,
            end: 8,
            command_sha256: [0x5a; 32],
        }]);
        assert_eq!(crate::copy_rsp_rdp_observations().len(), 1);

        load_rom(vec![1]);

        assert!(crate::copy_rsp_rdp_observations().is_empty());
        assert_eq!(crate::rsp_rdp_observation_count(), 0);
    }

    #[test]
    fn interactive_rsp_rdp_retention_is_constant_space_and_counted() {
        load_rom(vec![0]);
        crate::set_rsp_rdp_observation_retention(
            crate::RspRdpObservationRetention::InteractiveConstantSpace,
        );
        for start in 0..10_000u32 {
            crate::record_rsp_rdp_observations(vec![
                crate::RspRdpObservationKind::DramDpcCommitted {
                    start,
                    end: start + 8,
                    command_sha256: [0x5a; 32],
                },
            ]);
        }

        assert_eq!(crate::rsp_rdp_observation_count(), 10_000);
        crate::with_host(|host| {
            assert!(host.rsp_rdp_observations.is_empty());
            assert_eq!(host.rsp_rdp_observations.capacity(), 0);
        });
        assert_eq!(
            crate::rsp_rdp_observation_retention(),
            crate::RspRdpObservationRetention::InteractiveConstantSpace
        );
        let unavailable = std::panic::catch_unwind(crate::copy_rsp_rdp_observations);
        assert!(unavailable.is_err());

        // A new ROM starts a new evidence lifetime. No payload has been
        // discarded in that lifetime, so complete retention can be selected
        // again without pretending the prior history was reconstructed.
        load_rom(vec![1]);
        crate::set_rsp_rdp_observation_retention(
            crate::RspRdpObservationRetention::CompleteEvidence,
        );
        assert!(crate::copy_rsp_rdp_observations().is_empty());
    }

    #[test]
    fn complete_rsp_rdp_retention_cannot_resume_after_discarding_payloads() {
        load_rom(vec![0]);
        crate::set_rsp_rdp_observation_retention(
            crate::RspRdpObservationRetention::InteractiveConstantSpace,
        );
        crate::record_rsp_rdp_observations(vec![
            crate::RspRdpObservationKind::DramDpcCommitted {
                start: 0,
                end: 8,
                command_sha256: [0x5a; 32],
            },
        ]);

        let restore = std::panic::catch_unwind(|| {
            crate::set_rsp_rdp_observation_retention(
                crate::RspRdpObservationRetention::CompleteEvidence,
            );
        });
        assert!(restore.is_err());

        load_rom(vec![1]);
        crate::set_rsp_rdp_observation_retention(
            crate::RspRdpObservationRetention::CompleteEvidence,
        );
    }

    #[test]
    fn synchronous_pi_boundaries_preserve_same_cycle_cross_owner_save_order() {
        load_rom(vec![0; 0x100]);
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::Eeprom4k,
        )));

        with_pi_dma("same-cycle save ordering", |dma| {
            dma.eeprom_read_block(Cycles::ZERO, 0).unwrap();
        });
        crate::record_save_operation(
            fn64_runtime::SaveType::ControllerPak,
            fn64_runtime::SaveOperationKind::Read,
            0x20,
            fn64_runtime::pfs::PFS_BLOCK_SIZE,
        );
        with_pi_dma("same-cycle save ordering", |dma| {
            dma.eeprom_read_block(Cycles::ZERO, 1).unwrap();
        });

        assert_eq!(
            crate::copy_save_operations()
                .iter()
                .map(|event| (event.device, event.offset))
                .collect::<Vec<_>>(),
            vec![
                (fn64_runtime::SaveType::Eeprom4k, 0),
                (fn64_runtime::SaveType::ControllerPak, 0x20),
                (
                    fn64_runtime::SaveType::Eeprom4k,
                    fn64_runtime::save::EEPROM_BLOCK_SIZE as u32,
                ),
            ]
        );
    }

    #[test]
    fn sram_evidence_uses_pi_commit_cycle_not_outer_advance_target() {
        load_rom(vec![0; 0x100]);
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::SramBanked,
        )));
        let mut rdram = vec![0u8; 64];
        let started_at = crate::sim_time();
        start_timed_pi_dma(
            rdram.as_mut_ptr(),
            rdram.len(),
            PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0),
                device: fn64_runtime::PiDeviceAddress::SramOffset(0),
                len: 4,
            },
            None,
            0,
            "SRAM evidence timing test",
        )
        .unwrap();
        crate::advance_virtual_time(started_at + 9);

        assert_eq!(
            crate::copy_save_operations(),
            vec![fn64_runtime::SaveOperationEvent {
                at: Cycles::new(started_at + 1),
                device: fn64_runtime::SaveType::SramBanked,
                operation: fn64_runtime::SaveOperationKind::Read,
                offset: 0,
                len: 4,
            }]
        );
    }

    #[test]
    fn typed_cartridge_save_configuration_is_exact_release_evidence() {
        load_rom(vec![0; 0x100]);
        set_cartridge_save(
            CartridgeSaveType::SramBanked,
            Box::new(fn64_runtime::InMemorySaveStorage::new(
                CartridgeSaveType::SramBanked.byte_len(),
            )),
        );
        assert_eq!(
            host_evidence_snapshot().cartridge_save,
            CartridgeSaveEvidenceSnapshot::Configured(CartridgeSaveType::SramBanked)
        );

        load_rom(vec![1; 0x100]);
        assert_eq!(
            host_evidence_snapshot().cartridge_save,
            CartridgeSaveEvidenceSnapshot::Unidentified
        );
        configure_no_cartridge_save();
        assert_eq!(
            host_evidence_snapshot().cartridge_save,
            CartridgeSaveEvidenceSnapshot::NoCartridgeSave
        );
    }

    #[test]
    fn legacy_or_wrong_sized_save_configuration_cannot_claim_a_type() {
        load_rom(vec![0; 0x100]);
        let wrong_size = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            set_cartridge_save(
                CartridgeSaveType::Eeprom4k,
                Box::new(fn64_runtime::InMemorySaveStorage::new(511)),
            );
        }));
        assert!(wrong_size.is_err());
        assert_eq!(
            host_evidence_snapshot().cartridge_save,
            CartridgeSaveEvidenceSnapshot::Unidentified
        );

        set_save(Box::new(fn64_runtime::InMemorySaveStorage::new(512)));
        assert_eq!(
            host_evidence_snapshot().cartridge_save,
            CartridgeSaveEvidenceSnapshot::Unidentified
        );
        let relabel = std::panic::catch_unwind(configure_no_cartridge_save);
        assert!(relabel.is_err());
    }

    #[test]
    fn raw_pif_ram_window_round_trips_through_the_device_fabric() {
        // KSEG1 and KSEG0 views hit the same 64-byte PIF RAM; the boot
        // handshake's status word lives in the final word (0x1FC007FC).
        assert_eq!(read_raw_mmio_word(0xFFFF_FFFF_BFC0_07C0), Some(0));
        assert!(write_raw_mmio_word(0xFFFF_FFFF_BFC0_07C8, 0xDEAD_BEEF));
        assert_eq!(read_raw_mmio_word(0xFFFF_FFFF_BFC0_07C8), Some(0xDEAD_BEEF));
        assert_eq!(read_raw_mmio_word(0xFFFF_FFFF_9FC0_07C8), Some(0xDEAD_BEEF));
        // The final-word store runs the PIF command interpreter; a zero
        // command byte must leave a readable (non-faulting) window behind.
        assert!(write_raw_mmio_word(0xFFFF_FFFF_BFC0_07FC, 0));
        assert_eq!(read_raw_mmio_word(0xFFFF_FFFF_BFC0_07FC), Some(0));
        assert!(write_raw_mmio_word(0x0000_0000_9FC0_07CC, 0x1234_5678));
        assert_eq!(read_raw_mmio_word(0x0000_0000_BFC0_07CC), Some(0x1234_5678));
        // One byte past the window is NOT PIF RAM.
        assert_eq!(pif_ram_window_offset(0xFFFF_FFFF_BFC0_0800), None);
        assert_eq!(pif_ram_window_offset(0x0000_0000_1FC0_07C0), None);
        assert_eq!(pif_ram_window_offset(0xFFFF_FFFF_DFC0_07C0), None);
        assert_eq!(pif_ram_window_offset(0x0000_0001_BFC0_07C0), None);
        assert_eq!(read_raw_mmio_word(0x0000_0000_1FC0_07C0), None);
        assert!(!write_raw_mmio_word(0xFFFF_FFFF_DFC0_07C0, 1));
        assert_eq!(live_device_mmio_addr(0x0000_0001_A440_0000, false), None);
    }

    #[test]
    fn raw_cartridge_window_reads_installed_rom_through_both_direct_segments() {
        load_rom(vec![0x10, 0x20, 0x30, 0x40, 0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(read_raw_mmio_word(0xFFFF_FFFF_B000_0000), Some(0x1020_3040));
        assert_eq!(read_raw_mmio_word(0x0000_0000_9000_0004), Some(0xAABB_CCDD));
        assert_eq!(cartridge_rom_window_offset(0xFFFF_FFFF_AFFF_FFFC), None);
        assert_eq!(cartridge_rom_window_offset(0xFFFF_FFFF_C000_0000), None);
        assert_eq!(cartridge_rom_window_offset(0x0000_0001_B000_0000), None);
    }

    fn complete_pi_dma() {
        let deadline = with_host(|host| {
            host.device_fabric
                .next_deadline()
                .expect("test expected one pending PI deadline")
                .get()
        });
        advance_virtual_time(deadline);
    }

    #[test]
    fn live_rdram_dma_bounds_logical_bytes_by_complete_native_words() {
        let mut storage = [0u8; 4];
        let mut committed = notify_committed_dma_write;
        let mut dma = unsafe {
            fn64_runtime::ProcessDmaMemory::from_raw_parts(
                storage.as_mut_ptr(),
                storage.len(),
                &mut committed,
            )
        };

        fn64_runtime::DmaMemory::dma_write_bytes(
            &mut dma,
            fn64_runtime::DmaWriterChannel::Pi,
            3,
            &[0xA5],
        );
        assert_eq!(
            fn64_runtime::RdramView::from_storage(&storage).read_u8(RdramAddr::from_offset(3)),
            0xA5
        );

        let outside = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fn64_runtime::DmaMemory::dma_write_bytes(
                &mut dma,
                fn64_runtime::DmaWriterChannel::Pi,
                4,
                &[0x5A],
            );
        }));
        assert!(outside.is_err(), "one-past-end PI DMA byte must trap");
    }

    #[test]
    fn managed_pi_dma_commits_state_then_posts_completion_before_resume() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        load_rom_with_fixed_pi_latency(rom, 5);

        let mut rdram = vec![0u8; 0x1000];
        let cart_handle = install_cart_handle(&mut rdram, 0x800);
        let queue = RdramAddr::from_offset(0x300);
        with_executor(|exec| exec.create_mesg_queue(queue, 1));
        let mb = 0x100usize;
        rdram[mb + 0x4..mb + 0x8].copy_from_slice(&0x8000_0300u32.to_ne_bytes());
        rdram[mb + 0x8..mb + 0xC].copy_from_slice(&0x8000_0400u32.to_ne_bytes());
        rdram[mb + 0xC..mb + 0x10].copy_from_slice(&0x20u32.to_ne_bytes());
        rdram[mb + 0x10..mb + 0x14].copy_from_slice(&4u32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = 0x8000_0100;
        ctx.r6 = 0;
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        assert_eq!(&rdram[0x400..0x404], &[0, 0, 0, 0]);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().pi_status),
            fn64_runtime::PI_STATUS_DMA_BUSY
        );
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, queue, false)),
            fn64_runtime::RecvMesgOutcome::WouldBlock
        );

        advance_virtual_time(4);
        assert_eq!(&rdram[0x400..0x404], &[0, 0, 0, 0]);
        advance_virtual_time(5);

        assert_eq!(
            u32::from_ne_bytes(rdram[0x400..0x404].try_into().unwrap()),
            0xDEAD_BEEF
        );
        let snapshot = with_host(|host| host.device_fabric.snapshot());
        assert_eq!(snapshot.pi_status, 0);
        assert_ne!(
            snapshot.mi_pending & fn64_runtime::InterruptSource::Pi.bit(),
            0
        );
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, queue, false)),
            fn64_runtime::RecvMesgOutcome::Delivered(0)
        );
        let kinds = with_host(|host| {
            host.device_fabric
                .trace()
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>()
        });
        assert!(matches!(
            kinds[0],
            fn64_runtime::DeviceTraceKind::PiDmaStarted(_)
        ));
        assert!(matches!(
            kinds[1],
            fn64_runtime::DeviceTraceKind::PiBytesCommitted(_)
        ));
        assert_eq!(kinds[2], fn64_runtime::DeviceTraceKind::PiBusyCleared);
        assert_eq!(
            kinds[3],
            fn64_runtime::DeviceTraceKind::MiInterruptRaised(fn64_runtime::InterruptSource::Pi)
        );
        assert!(matches!(
            kinds[4],
            fn64_runtime::DeviceTraceKind::NotificationReady(_)
        ));
        assert_eq!(
            copy_device_trace()
                .into_iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            kinds,
            "public release-evidence accessor must copy the fabric-owned trace verbatim"
        );
    }

    /// Regression for the real OoT interleaving: the object-loading thread
    /// submitted DmaMgr's second chunk while another guest thread's managed
    /// PI request still owned the hardware channel. Exposing `PiBusy` made
    /// DmaMgr return after its first 0x2000-byte chunk and left the display
    /// list tail zero. Both calls must succeed immediately, while bytes and
    /// completion posts remain strictly FIFO at their separate deadlines.
    #[test]
    fn managed_pi_dma_serializes_concurrent_callers_fifo() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        rom[0x40..0x44].copy_from_slice(&[0x55, 0x66, 0x77, 0x88]);
        load_rom_with_fixed_pi_latency(rom, 5);

        let mut rdram = vec![0u8; 0x1000];
        let cart_handle = install_cart_handle(&mut rdram, 0x800);
        let first_queue = RdramAddr::from_offset(0x300);
        let second_queue = RdramAddr::from_offset(0x340);
        with_executor(|exec| {
            exec.create_mesg_queue(first_queue, 1);
            exec.create_mesg_queue(second_queue, 1);
        });

        let write_mb = |rdram: &mut [u8], mb: usize, queue: u32, dram: u32, dev: u32| {
            rdram[mb + 0x4..mb + 0x8].copy_from_slice(&queue.to_ne_bytes());
            rdram[mb + 0x8..mb + 0xC].copy_from_slice(&dram.to_ne_bytes());
            rdram[mb + 0xC..mb + 0x10].copy_from_slice(&dev.to_ne_bytes());
            rdram[mb + 0x10..mb + 0x14].copy_from_slice(&4u32.to_ne_bytes());
        };
        write_mb(&mut rdram, 0x100, 0x8000_0300, 0x8000_0400, 0x20);
        write_mb(&mut rdram, 0x140, 0x8000_0340, 0x8000_0440, 0x40);

        let mut first = ctx_zeroed();
        first.r4 = cart_handle;
        first.r5 = 0x8000_0100;
        first.r6 = 0;
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut first) };
        assert_eq!(first.r2, 0);

        let mut second = ctx_zeroed();
        second.r4 = cart_handle;
        second.r5 = 0x8000_0140;
        second.r6 = 0;
        second.r2 = 0xBAD0_BAD0;
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut second) };
        assert_eq!(second.r2, 0, "queued managed PI work is accepted");
        assert_eq!(with_host(|host| host.pending_pi_completions.len()), 2);

        advance_virtual_time(5);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x400..0x404].try_into().unwrap()),
            0x1122_3344
        );
        assert_eq!(&rdram[0x440..0x444], &[0, 0, 0, 0]);
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, first_queue, false)),
            fn64_runtime::RecvMesgOutcome::Delivered(0)
        );
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, second_queue, false)),
            fn64_runtime::RecvMesgOutcome::WouldBlock
        );
        assert_eq!(with_host(|host| host.pending_pi_completions.len()), 1);

        advance_virtual_time(10);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x440..0x444].try_into().unwrap()),
            0x5566_7788
        );
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, second_queue, false)),
            fn64_runtime::RecvMesgOutcome::Delivered(0)
        );
        assert!(with_host(|host| host.pending_pi_completions.is_empty()));
    }

    #[test]
    fn mi_authority_exists_before_cartridge_rom_is_installed() {
        let source = fn64_runtime::InterruptSource::Sp;
        set_mi_interrupt_mask(source.bit());
        raise_device_interrupt(source);
        assert!(cpu_interrupt_pending());
        clear_device_interrupt(source);
        assert!(!cpu_interrupt_pending());
        assert!(!with_host(|host| host.rom_installed));
    }

    #[test]
    fn mi_shim_raw_and_generated_c_proxy_share_fixed_cycle_state_and_trace() {
        const SELECTED_MASK: u32 = (1 << 0) | (1 << 2) | (1 << 4);
        const SELECTED_MASK_COMMAND: u32 = (1 << 1) | (1 << 5) | (1 << 9);

        let capture = |write_mask: &dyn Fn()| {
            load_rom(vec![0; 0x100]);
            write_mask();
            for source in [
                fn64_runtime::InterruptSource::Sp,
                fn64_runtime::InterruptSource::Ai,
                fn64_runtime::InterruptSource::Pi,
            ] {
                raise_device_interrupt(source);
            }
            assert!(cpu_interrupt_pending());
            assert_eq!(
                read_raw_mmio_word(0xFFFF_FFFF_A430_000C),
                Some(SELECTED_MASK)
            );
            with_host(|host| {
                (
                    host.device_fabric.evidence_snapshot(),
                    host.device_fabric.trace().to_vec(),
                )
            })
        };

        let shim = capture(&|| {
            let mut ctx = ctx_zeroed();
            ctx.r4 = u64::from(SELECTED_MASK << 16);
            unsafe { crate::system::osSetIntMask_recomp(std::ptr::null_mut(), &mut ctx) };
        });
        let raw = capture(&|| {
            assert!(write_raw_mmio_word(
                0xFFFF_FFFF_A430_000C,
                SELECTED_MASK_COMMAND
            ));
        });
        let generated_c_proxy = capture(&|| {
            crate::fn64_c_mmio_write_w(0xFFFF_FFFF_A430_000C, SELECTED_MASK_COMMAND);
        });

        assert_eq!(shim, raw);
        assert_eq!(raw, generated_c_proxy);
        assert_eq!(shim.0.guest.now, Cycles::ZERO);
    }

    #[test]
    fn os_virtual_to_physical_masks_kseg0() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_1234;
        unsafe { osVirtualToPhysical_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0x0000_1234);
    }

    #[test]
    fn os_virtual_to_physical_masks_kseg1() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0xA000_5678;
        unsafe { osVirtualToPhysical_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0x0000_5678);
    }

    #[test]
    fn leo_disk_init_returns_a_distinct_public_domain2_handle() {
        let mut rdram = vec![0u8; 0x2000];
        configure_leo_disk(LeoDiskConfig {
            handle_vram: 0x8000_1000,
            latency: 0x12,
            page_size: 0x0D,
            release: 0x02,
            pulse_width: 0x34,
        });
        let mut ctx = ctx_zeroed();
        unsafe { osLeoDiskInit_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0xFFFF_FFFF_8000_1000);

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let base = RdramAddr::from_offset(0x1000);
        assert_eq!(view.read_u32(base), 0);
        assert_eq!(view.read_u8(base.checked_add(4).unwrap()), 2);
        assert_eq!(view.read_u8(base.checked_add(5).unwrap()), 0x12);
        assert_eq!(view.read_u8(base.checked_add(6).unwrap()), 0x0D);
        assert_eq!(view.read_u8(base.checked_add(7).unwrap()), 0x02);
        assert_eq!(view.read_u8(base.checked_add(8).unwrap()), 0x34);
        assert_eq!(view.read_u8(base.checked_add(9).unwrap()), 1);
        assert_eq!(view.read_u32(base.checked_add(12).unwrap()), 0xa500_0000);
        assert_eq!(view.read_u32(base.checked_add(16).unwrap()), 0);
    }

    #[test]
    fn os_pi_start_dma_marshals_stack_arguments_into_the_shared_epi_path() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        load_rom(rom);
        let mut rdram = vec![0u8; 0x400];
        let stack = 0x40usize;
        rdram[stack + 0x10..stack + 0x14].copy_from_slice(&0x8000_0200u32.to_ne_bytes());
        rdram[stack + 0x14..stack + 0x18].copy_from_slice(&4u32.to_ne_bytes());
        rdram[stack + 0x18..stack + 0x1C].copy_from_slice(&0u32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0100;
        ctx.r5 = 1;
        ctx.r6 = 0;
        ctx.r7 = 0x20;
        ctx.r29 = 0x8000_0040;
        unsafe { osPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
        complete_pi_dma();

        assert_eq!(ctx.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x200..0x204].try_into().unwrap()),
            0x1234_5678
        );
        assert_eq!(
            u32::from_ne_bytes(rdram[0x108..0x10C].try_into().unwrap()),
            0x8000_0200
        );
        assert_eq!(
            u32::from_ne_bytes(rdram[0x10C..0x110].try_into().unwrap()),
            0x20
        );
        assert_eq!(
            u32::from_ne_bytes(rdram[0x110..0x114].try_into().unwrap()),
            4
        );
    }

    #[test]
    fn os_pi_read_io_remaps_both_arguments_without_losing_the_data_pointer() {
        let mut rom = vec![0u8; 0x80];
        rom[0x20..0x24].copy_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
        load_rom(rom);
        let mut rdram = vec![0u8; 0x100];
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x20;
        ctx.r5 = 0x8000_0040;
        unsafe { osPiReadIo_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x40..0x44].try_into().unwrap()),
            0xCAFE_BABE
        );

        let mut status = ctx_zeroed();
        status.r2 = 0xDEAD_BEEF;
        unsafe { osPiGetStatus_recomp(std::ptr::null_mut(), &mut status) };
        assert_eq!(status.r2, 0);
    }

    /// Regression for OoT rs boot's `AudioLoad_Dma` alignment trap.
    /// `AudioLoad_Init` stores `osCartRomInit()`'s `$v0` into
    /// `gAudioCtx.cartHandle`; ROM PC 0x800B824C later executes the ordinary
    /// aligned `sw $t1, 0x14($a0)` through that pointer. The old shim left
    /// `$v0` untouched. Seed the exact stale value observed at the failing
    /// boot so that implementation returns `0x80125636` and fails this test,
    /// while the fixed shim returns the configured aligned guest handle.
    #[test]
    fn os_cart_rom_init_replaces_stale_unaligned_v0_with_guest_handle() {
        load_rom(vec![0u8; 0x100]);
        set_cart_rom_handle_vram(0x8000_9EA0);
        let mut rdram = vec![0u8; 0x9f00];

        let mut ctx = ctx_zeroed();
        ctx.r2 = 0xFFFF_FFFF_8012_5636;
        unsafe { osCartRomInit_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(ctx.r2, 0xFFFF_FFFF_8000_9EA0);
        assert_eq!(ctx.r2 & 3, 0, "returned OSPiHandle must be word-aligned");
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let handle = RdramAddr::from_offset(0x9ea0);
        assert_eq!(
            view.read_u8(handle.checked_add(4).unwrap()),
            DEVICE_TYPE_CART
        );
        assert_eq!(view.read_u8(handle.checked_add(9).unwrap()), PI_DOMAIN1);
        assert_eq!(view.read_u32(handle.checked_add(12).unwrap()), 0xb000_0000);
    }

    /// Regression test for the real double-KSEG0-translation bug
    /// `examples/wm2000-boot`'s boot run surfaced (a genuine
    /// EXC_BAD_ACCESS deep in `osEPiStartDma_recomp`'s field reads, once
    /// boot finally reached its first real PI DMA on thread 6): `mb_addr`
    /// is placed at a REALISTIC nonzero vram address (not offset 0, which
    /// would hide the bug -- 0 minus 0 is still 0), and the OSIoMesg
    /// fields are placed at their real rdram offsets relative to that vram
    /// address, not relative to 0.
    /// Builds an OSIoMesg exactly as OOTU `DmaMgr_DmaRomToRam` does
    /// (`funcs_0.c` asm 0x800008F0-0x80000900): 0x08-byte `OSIoMesgHdr`
    /// (retQueue at +0x4), then `dramAddr` +0x8, `devAddr` +0xC, `size`
    /// +0x10. The prior version of this test placed fields +0x4 too high to
    /// match the buggy 0xC-header shim, so it passed green against the bug --
    /// the exact "weak green check" trap. A NON-UNIFORM multi-word ROM
    /// payload and a NON-ZERO multi-word `size` make a wrong-offset read
    /// (which would pick up size=0, or the wrong devAddr) fail loudly.
    #[test]
    fn os_epi_start_dma_reads_real_fields_at_a_nonzero_mb_address() {
        // Use a fresh ROM per test (with_pi_dma's HOST state is thread-local
        // per test since each #[test] gets its own OS thread by default).
        // Non-uniform big-endian cart words at devAddr 0x40 so a flat
        // (non-swizzled) DMA, a wrong devAddr, or a truncated len all fail.
        let mut rom = vec![0u8; 0x1000];
        let dev_addr: u32 = 0x40;
        rom[0x40..0x44].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        rom[0x44..0x48].copy_from_slice(&[0x00, 0x00, 0x10, 0x60]); // 0x1060 -- DmaMgr's sentinel
        rom[0x48..0x4C].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        load_rom(rom);

        let mut rdram = vec![0u8; 0x10000];
        let cart_handle = install_cart_handle(&mut rdram, 0x1000);
        let mb_vram: u64 = 0x8000_2000; // a REAL, nonzero vram address
        let mb_offset = 0x2000usize;

        // OSIoMesg fields at mb_offset + {retQueue +0x4, dramAddr +0x8,
        // devAddr +0xC, size +0x10} -- native byte order, DmaMgr's real
        // layout (0x08-byte OSIoMesgHdr).
        let dram_target_vram: u32 = 0x8000_5000;
        let size: u32 = 0xC; // 3 words -- non-zero, multi-word
        rdram[mb_offset + 0x4..mb_offset + 0x8].copy_from_slice(&0u32.to_ne_bytes()); // no retQueue
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&dram_target_vram.to_ne_bytes());
        rdram[mb_offset + 0xC..mb_offset + 0x10].copy_from_slice(&dev_addr.to_ne_bytes());
        rdram[mb_offset + 0x10..mb_offset + 0x14].copy_from_slice(&size.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = mb_vram;
        ctx.r6 = 0; // OS_READ / ToRdram
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        complete_pi_dma();

        // dramAddr (0x8000_5000) -> rdram offset 0x5000. Each big-endian
        // cart word must arrive so the guest's MEM_W reads it intact; rdram
        // is native-word storage, so physical bytes are byte-reversed. A
        // wrong offset would read size=0 (delivering nothing) or the wrong
        // devAddr; a flat copy would byte-reverse the words.
        let w0 = u32::from_ne_bytes(rdram[0x5000..0x5004].try_into().unwrap());
        let w1 = u32::from_ne_bytes(rdram[0x5004..0x5008].try_into().unwrap());
        let w2 = u32::from_ne_bytes(rdram[0x5008..0x500C].try_into().unwrap());
        assert_eq!(w0, 0x1234_5678, "first ROM word must be delivered intact");
        assert_eq!(
            w1, 0x0000_1060,
            "second word (DmaMgr's 0x1060 sentinel) proves the full size was read, not 0/one word"
        );
        assert_eq!(w2, 0xDEAD_BEEF, "third word confirms the exact len (0xC)");
        // And nothing spilled past the declared length.
        let after = u32::from_ne_bytes(rdram[0x500C..0x5010].try_into().unwrap());
        assert_eq!(after, 0, "DMA must not write past size (0xC bytes)");
    }

    /// Regression test for the OoT-boot hang (2026-07-14): `osEPiReadIo`
    /// delivered the cartridge word into rdram FLAT, but the guest reads
    /// individual bytes back through `MEM_BU`'s `^3` byte-lane XOR (rdram is
    /// native-endian-word storage). `Locale_Init` DMAs the ROM header, `lbu`s
    /// the region byte, accepts only 'E'/'J', else `LogUtils_HungupThread`s.
    /// A flat copy delivered the wrong byte -> neither-E-nor-J -> deliberate
    /// hang. This models that exact read with a distinguishable word so a
    /// regression to flat semantics fails here, not 8 frames into a boot.
    #[test]
    fn os_epi_read_io_word_reads_back_through_mem_bu_unswapped() {
        // ROM word at devAddr 0x3C = `5A 4C 4A 00` (OoT's real `Z L J \0`);
        // guest wants MEM_BU(dram+2) == 0x4A ('J').
        let mut rom = vec![0u8; 0x100];
        rom[0x3C..0x40].copy_from_slice(&[0x5A, 0x4C, 0x4A, 0x00]);
        load_rom(rom);

        let mut rdram = vec![0u8; 0x1000];
        let cart_handle = install_cart_handle(&mut rdram, 0x100);
        let dram_vram: u64 = 0x8000_0024;
        let dram_off = 0x24usize;

        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = 0x3C; // devAddr
        ctx.r6 = dram_vram; // dramAddr
        unsafe { osEPiReadIo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // MEM_BU(dram_off ^ 3) is the guest's byte read; +2 must be 'J'.
        assert_eq!(rdram[dram_off ^ 3], 0x5A); // 'Z'
        assert_eq!(rdram[(dram_off + 2) ^ 3], 0x4A); // 'J' -- the region byte
                                                     // And MEM_W reads the cart word intact (native-endian word storage).
        let w = u32::from_ne_bytes(rdram[dram_off..dram_off + 4].try_into().unwrap());
        assert_eq!(w, 0x5A4C_4A00);
    }

    /// Regression test for the SRAM-DMA-treated-as-ROM crash (2026-07-15):
    /// OoT's `Sram_InitSram -> SsSram_ReadWrite -> SsSram_Dma` issues a PI DMA
    /// with `devAddr = 0x08000000` (PI_DOM2_ADDR2, the SRAM cartridge base --
    /// rcp.h:714), which the old `osEPiStartDma_recomp` blindly read from the
    /// ROM image -> `InMemoryRom::read_into` past the 55MB ROM -> loud trap.
    /// The fix routes domain-2 devAddrs to the registered `SaveStorage`.
    ///
    /// Drives the REAL raw-pointer shim path (not `PiDma::start_dma`) for both
    /// directions: build an OSIoMesg exactly as `SsSram_Dma` does (dramAddr
    /// +0x8, devAddr +0xC, size +0x10, per pi.h:52-58), OS_WRITE the pattern to
    /// SRAM, then OS_READ it back into a different rdram region and assert the
    /// guest's own `MEM_BU`/`MEM_W` accessors read every byte in the SAME
    /// order. A flat (non-swizzled) copy in either direction fails here.
    #[test]
    fn os_epi_start_dma_round_trips_sram_save_domain() {
        // A ROM whose bytes at offset 0 are DISTINCT from the SRAM pattern, so
        // a regression that reads the ROM instead of the save is caught.
        let mut rom = vec![0u8; 0x1000];
        rom[0..4].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        load_rom(rom);
        // OoT uses 32 KiB banked SRAM.
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::SramBanked,
        )));

        let mut rdram = vec![0u8; 0x10000];
        let sram_handle = install_sram_handle(
            &mut rdram,
            0x1000,
            fn64_runtime::PiDomainTiming {
                latency: 0x12,
                pulse_width: 0x34,
                page_size: 0x0d,
                release: 2,
            },
        );
        let mb_offset = 0x2000usize;
        let mb_vram: u64 = 0x8000_2000;
        // EPI callers provide the offset from the handle's base; the shared
        // resolver must form 0x0800_0010 before entering the PI fabric.
        let sram_dev_addr: u32 = 0x10;
        let size: u32 = 8;

        // Guest lays 8 distinct bytes at rdram 0x5000 via MEM_BU (byte-lane
        // `^3`), the way it would build a save record before writing it out.
        let src = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let src_off = 0x5000usize;
        for (k, &b) in src.iter().enumerate() {
            rdram[(src_off + k) ^ 3] = b;
        }
        // OSIoMesg for the WRITE (OS_WRITE=1 -> FromRdram).
        rdram[mb_offset + 0x4..mb_offset + 0x8].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&0x8000_5000u32.to_ne_bytes());
        rdram[mb_offset + 0xC..mb_offset + 0x10].copy_from_slice(&sram_dev_addr.to_ne_bytes());
        rdram[mb_offset + 0x10..mb_offset + 0x14].copy_from_slice(&size.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = sram_handle;
        ctx.r5 = mb_vram;
        ctx.r6 = 1; // OS_WRITE
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(
            with_host(|host| {
                host.device_fabric
                    .pi_domain_timing(fn64_runtime::PiDomain::Domain2)
            }),
            fn64_runtime::PiDomainTiming {
                latency: 0x12,
                pulse_width: 0x34,
                page_size: 0x0d,
                release: 2,
            }
        );
        complete_pi_dma();

        // OSIoMesg for the READ back into a DIFFERENT region (0x6000).
        let dst_off = 0x6000usize;
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&0x8000_6000u32.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = sram_handle;
        ctx.r5 = mb_vram;
        ctx.r6 = 0; // OS_READ
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        complete_pi_dma();

        // Guest reads readBuff[k] via MEM_BU((dst)+k) = rdram[(dst+k)^3];
        // every byte must match the original -- swizzle cancels round-trip.
        for (k, &b) in src.iter().enumerate() {
            assert_eq!(
                rdram[(dst_off + k) ^ 3],
                b,
                "SRAM round-trip byte {k}: save DMA must route to the save store, \
                 word-swizzled, not the ROM"
            );
        }
        // The ROM byte at offset 0 (0xAA) must NOT appear -- proves the read
        // hit the save store, not the ROM image.
        assert_ne!(rdram[dst_off ^ 3], 0xAA);
        let save_operations = crate::copy_save_operations();
        assert_eq!(save_operations.len(), 2);
        assert_eq!(
            save_operations
                .iter()
                .map(|event| (event.device, event.operation, event.offset, event.len))
                .collect::<Vec<_>>(),
            vec![
                (
                    fn64_runtime::SaveType::SramBanked,
                    fn64_runtime::SaveOperationKind::Write,
                    0x10,
                    8,
                ),
                (
                    fn64_runtime::SaveType::SramBanked,
                    fn64_runtime::SaveOperationKind::Read,
                    0x10,
                    8,
                ),
            ]
        );
    }

    /// Regression test for the real infinite-loop bug `examples/wm2000-boot`
    /// surfaced (2026-07-14): `osEPiStartDma_recomp` never wrote `ctx.r2`
    /// ($v0), so NWXE's chunked-DMA caller (`func_80000660`, asm
    /// 0x800006E4-0x800006FC: `bne $v0, $zero, L_800006E4`) read whatever
    /// stale value `r2` already held and looped forever instead of falling
    /// through to `osRecvMesg`. Seed `ctx.r2` with a realistic STALE
    /// NON-ZERO value beforehand (mirroring the real caller's register
    /// state at the call site) so a regression that stops writing `ctx.r2`
    /// would fail this test even though a zero-initialized `ctx` would
    /// have hidden the bug.
    #[test]
    fn os_epi_start_dma_writes_zero_return_value_even_with_stale_nonzero_r2() {
        load_rom(vec![0xCDu8; 0x1000]);

        let mut rdram = vec![0u8; 0x10000];
        let cart_handle = install_cart_handle(&mut rdram, 0x1000);
        let mb_offset = 0x2000usize;
        // DmaMgr's real OSIoMesg layout: retQueue +0x4, dramAddr +0x8,
        // devAddr +0xC, size +0x10 (0x08-byte OSIoMesgHdr).
        rdram[mb_offset + 0x4..mb_offset + 0x8].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&0x8000_5000u32.to_ne_bytes());
        rdram[mb_offset + 0xC..mb_offset + 0x10].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb_offset + 0x10..mb_offset + 0x14].copy_from_slice(&4u32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = 0x8000_2000;
        ctx.r6 = 0; // OS_READ / ToRdram
        ctx.r2 = 0x1234; // stale non-zero, as a real caller's register would hold
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(
            ctx.r2, 0,
            "osEPiStartDma_recomp must overwrite $v0 with 0 (success) on every \
             accepted-start path, or NWXE's chunked-DMA retry loop spins forever"
        );
    }

    #[test]
    fn os_epi_raw_start_dma_reads_rom_with_fifth_argument_from_stack() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x28].copy_from_slice(&[0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87]);
        load_rom(rom);
        let mut rdram = vec![0u8; 0x100];
        let cart_handle = install_cart_handle(&mut rdram, 0x20);
        rdram[0x40 + 0x10..0x40 + 0x14].copy_from_slice(&8u32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = 0;
        ctx.r6 = 0x20;
        ctx.r7 = 0x8000_0080;
        ctx.r29 = 0x8000_0040;
        unsafe { osEPiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
        complete_pi_dma();
        assert_eq!(ctx.r2, 0);
        for (index, expected) in [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87]
            .into_iter()
            .enumerate()
        {
            assert_eq!(rdram[(0x80 + index) ^ 3], expected);
        }
    }

    #[test]
    fn pi_writes_to_read_only_rom_return_minus_one() {
        load_rom(vec![0u8; 0x100]);
        let mut rdram = vec![0u8; 0x100];
        let cart_handle = install_cart_handle(&mut rdram, 0x20);
        rdram[0x40 + 0x10..0x40 + 0x14].copy_from_slice(&8u32.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = 1;
        ctx.r6 = 0x20;
        ctx.r7 = 0x8000_0080;
        ctx.r29 = 0x8000_0040;
        unsafe { osEPiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, u64::MAX);
    }

    #[test]
    fn managed_raw_and_programmed_epi_calls_share_handle_address_and_timing_authority() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        load_rom(rom);
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::SramBanked,
        )));
        let mut rdram = vec![0u8; 0x800];
        let cart_timing = fn64_runtime::PiDomainTiming {
            latency: 0x21,
            pulse_width: 0x32,
            page_size: 0x0b,
            release: 1,
        };
        let save_timing = fn64_runtime::PiDomainTiming {
            latency: 0x43,
            pulse_width: 0x54,
            page_size: 0x0c,
            release: 2,
        };
        unsafe {
            write_epi_handle(
                rdram.as_mut_ptr(),
                0x8000_0100,
                DEVICE_TYPE_CART,
                fn64_runtime::PiDomain::Domain1,
                cart_timing,
                0xb000_0000,
            )
        };
        let cart_handle = 0xFFFF_FFFF_8000_0100;
        let save_handle = install_sram_handle(&mut rdram, 0x140, save_timing);

        // Raw EPI DMA consumes the same handle decode as managed EPI. The
        // request stores fn64's internal ROM offset while raw MMIO exposes
        // the handle's domain timing immediately.
        rdram[0x40 + 0x10..0x40 + 0x14].copy_from_slice(&4u32.to_ne_bytes());
        let mut raw = ctx_zeroed();
        raw.r4 = cart_handle;
        raw.r5 = 0;
        raw.r6 = 0x20;
        raw.r7 = 0x8000_0200;
        raw.r29 = 0x8000_0040;
        unsafe { osEPiRawStartDma_recomp(rdram.as_mut_ptr(), &mut raw) };
        assert_eq!(raw.r2, 0);
        assert_eq!(
            with_host(|host| host.device_fabric.pending_pi_request().unwrap().device),
            fn64_runtime::PiDeviceAddress::RomOffset(0x20)
        );
        assert_eq!(
            read_raw_mmio_word(0xA460_0014),
            Some(cart_timing.latency as u32)
        );
        assert_eq!(
            read_raw_mmio_word(0xA460_0018),
            Some(cart_timing.pulse_width as u32)
        );
        complete_pi_dma();

        // The public OR rule also admits an already-absolute KSEG1 device
        // address. Segment removal happens only after the OR, at the PI
        // boundary, so this reaches the same ROM byte as offset 0x20 above.
        let mut absolute = ctx_zeroed();
        absolute.r4 = cart_handle;
        absolute.r5 = 0xb000_0020;
        absolute.r6 = 0x8000_0204;
        unsafe { osEPiReadIo_recomp(rdram.as_mut_ptr(), &mut absolute) };
        assert_eq!(absolute.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x204..0x208].try_into().unwrap()),
            0x1234_5678
        );

        // Managed EPI forms baseAddress | devAddr for SRAM and publishes the
        // second handle's settings through those same raw registers.
        let mb = 0x300usize;
        rdram[mb + 0x4..mb + 0x8].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb + 0x8..mb + 0xC].copy_from_slice(&0x8000_0240u32.to_ne_bytes());
        rdram[mb + 0xC..mb + 0x10].copy_from_slice(&0x10u32.to_ne_bytes());
        rdram[mb + 0x10..mb + 0x14].copy_from_slice(&4u32.to_ne_bytes());
        let mut managed = ctx_zeroed();
        managed.r4 = save_handle;
        managed.r5 = 0x8000_0300;
        managed.r6 = 0;
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut managed) };
        assert_eq!(managed.r2, 0);
        assert_eq!(
            with_host(|host| host.device_fabric.pending_pi_request().unwrap().device),
            fn64_runtime::PiDeviceAddress::SramOffset(0x10)
        );
        assert_eq!(
            read_raw_mmio_word(0xA460_0024),
            Some(save_timing.latency as u32)
        );
        assert_eq!(
            read_raw_mmio_word(0xA460_0028),
            Some(save_timing.pulse_width as u32)
        );
        complete_pi_dma();

        // Programmed I/O uses the same resolver in both directions rather
        // than retaining a third handle/address implementation.
        let mut write = ctx_zeroed();
        write.r4 = save_handle;
        write.r5 = 0x20;
        write.r6 = 0xCAFE_BABE;
        unsafe { osEPiWriteIo_recomp(rdram.as_mut_ptr(), &mut write) };
        assert_eq!(write.r2, 0);
        let mut read = ctx_zeroed();
        read.r4 = save_handle;
        read.r5 = 0x20;
        read.r6 = 0x8000_0280;
        unsafe { osEPiReadIo_recomp(rdram.as_mut_ptr(), &mut read) };
        assert_eq!(read.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x280..0x284].try_into().unwrap()),
            0xCAFE_BABE
        );
        assert_eq!(
            crate::copy_save_operations()
                .iter()
                .map(|event| (event.operation, event.offset, event.len))
                .collect::<Vec<_>>(),
            vec![
                (fn64_runtime::SaveOperationKind::Read, 0x10, 4),
                (fn64_runtime::SaveOperationKind::Write, 0x20, 4),
                (fn64_runtime::SaveOperationKind::Read, 0x20, 4),
            ]
        );
    }

    #[test]
    fn epi_handle_for_unbacked_public_device_space_is_a_loud_typed_trap() {
        let mut rdram = vec![0u8; 0x200];
        unsafe {
            write_epi_handle(
                rdram.as_mut_ptr(),
                0x8000_0100,
                DEVICE_TYPE_64DD,
                fn64_runtime::PiDomain::Domain2,
                fn64_runtime::PiDomainTiming::default(),
                0xa500_0000,
            )
        };
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            resolve_epi_device_address(rdram.as_mut_ptr(), 0x8000_0100, 0, "typed EPI test")
        }));
        assert!(result.is_err());
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].subsystem, fn64_runtime::UnsupportedSubsystem::Abi);
        assert_eq!(events[0].operation, "abi.pi.epi-handle");
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        assert!(events[0].context.contains("0x05000000"));
        fn64_runtime::complete_unsupported_observation(Cycles::ZERO, &"0".repeat(64));

        assert_subprocess_aborts("pi::tests::__unbacked_epi_handle_abort_subprocess_entry");
    }

    #[test]
    fn epi_handle_outside_physical_rdram_is_a_loud_typed_trap() {
        let mut rdram = vec![0u8; 0x100];
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            resolve_epi_device_address(
                rdram.as_mut_ptr(),
                0xffff_ffff_807f_fff0,
                0,
                "out-of-range EPI test",
            )
        }));
        assert!(result.is_err());
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].operation, "abi.pi.epi-handle");
        assert!(events[0].context.contains("outside physical RDRAM"));
        fn64_runtime::complete_unsupported_observation(Cycles::ZERO, &"0".repeat(64));
    }

    #[test]
    fn epi_handle_rejects_a_raw_physical_base_instead_of_guessing_its_address_form() {
        let mut rdram = vec![0u8; 0x200];
        unsafe {
            write_epi_handle(
                rdram.as_mut_ptr(),
                0x8000_0100,
                DEVICE_TYPE_SRAM,
                fn64_runtime::PiDomain::Domain2,
                fn64_runtime::PiDomainTiming::default(),
                0xa800_0000,
            )
        };
        rdram[0x10c..0x110].copy_from_slice(&0x0800_0000u32.to_ne_bytes());

        fn64_runtime::arm_unsupported_events(None).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            resolve_epi_device_address(rdram.as_mut_ptr(), 0x8000_0100, 0, "physical-base test")
        }));
        assert!(result.is_err());
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].operation, "abi.pi.epi-handle");
        assert!(events[0].context.contains("uncached KSEG1"));
        fn64_runtime::complete_unsupported_observation(Cycles::ZERO, &"0".repeat(64));
    }

    #[test]
    #[ignore]
    fn __unbacked_epi_handle_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            load_rom(vec![0; 0x100]);
            let mut rdram = vec![0u8; 0x400];
            unsafe {
                write_epi_handle(
                    rdram.as_mut_ptr(),
                    0x8000_0100,
                    2,
                    fn64_runtime::PiDomain::Domain2,
                    fn64_runtime::PiDomainTiming::default(),
                    0xa500_0000,
                )
            };
            fn64_runtime::arm_unsupported_events(None).unwrap();
            let mut ctx = ctx_zeroed();
            ctx.r4 = 0x8000_0100;
            ctx.r5 = 0;
            ctx.r6 = 0x8000_0200;
            unsafe { osEPiReadIo_recomp(rdram.as_mut_ptr(), &mut ctx) };
        }
    }

    #[test]
    fn os_epi_start_dma_without_a_loaded_rom_is_a_loud_named_trap() {
        assert_subprocess_aborts("pi::tests::__os_epi_start_dma_no_rom_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __os_epi_start_dma_no_rom_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            // mb points at an all-zero rdram region -> ret_queue==0 (no
            // completion post attempted), dev_addr==0, len==0 -- the load-
            // bearing assertion here is that with_pi_dma panics because no
            // ROM was ever installed in this fresh subprocess, not that the
            // (deliberately trivial) transfer parameters are realistic.
            //
            // `mb` must be a real KSEG0 vram address with a buffer behind it:
            // a bare `r5 = 0` is NOT "rdram offset 0". `RdramAddr::from_gpr(0)`
            // computes `0 - 0xFFFFFFFF_80000000` = 0x80000000, so this shim's
            // `mb`-relative read of `retQueue` (+0x4) dereferenced ~2 GiB past
            // a 64-byte Vec and killed the child with SIGBUS *before* reaching
            // the `no ROM installed` panic -- the test still "passed" on
            // `!status.success()` while proving nothing about the trap.
            const MB_VRAM: u64 = 0xFFFF_FFFF_8000_0000;
            let mut ctx = ctx_zeroed();
            let mut rdram = rdram_for_vram(MB_VRAM + 0x40);
            unsafe {
                write_epi_handle(
                    rdram.as_mut_ptr(),
                    0x8000_0020,
                    DEVICE_TYPE_CART,
                    fn64_runtime::PiDomain::Domain1,
                    fn64_runtime::PiDomainTiming::default(),
                    0xb000_0000,
                )
            };
            ctx.r4 = 0x8000_0020;
            ctx.r5 = MB_VRAM;
            ctx.r6 = 0; // direction = ToRdram
            unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        }
    }
