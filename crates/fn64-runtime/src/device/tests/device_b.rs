use super::*;

    #[test]
    fn rcp_pi_timing_uses_programmed_domain_and_transfer_geometry() {
        let model = RcpPiTiming;
        let timing = PiDomainTiming {
            latency: 0x40,
            pulse_width: 0x12,
            page_size: 7,
            release: 3,
        };
        let request = |offset, len| PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0),
            device: PiDeviceAddress::RomOffset(offset),
            len,
        };

        // Independent constants cover the special complete 128-byte buffer,
        // one complete 512-byte page, and a 512-byte transfer split evenly
        // across two pages.
        assert_eq!(
            model.completion_latency(request(0, 128), timing),
            Cycles::new(2_369)
        );
        assert_eq!(
            model.completion_latency(request(0, 512), timing),
            Cycles::new(9_719)
        );
        assert_eq!(
            model.completion_latency(request(256, 512), timing),
            Cycles::new(9_837)
        );
        assert_eq!(
            model.evidence_bytes(),
            b"fn64.pi-timing.rcp-domain.v1\0"
        );
    }

    #[test]
    fn rcp_pi_timing_rounds_odd_raw_lengths_to_a_halfword() {
        let model = RcpPiTiming;
        let timing = PiDomainTiming::default();
        let request = |len| PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0),
            device: PiDeviceAddress::RomOffset(0),
            len,
        };

        assert_eq!(
            model.completion_latency(request(1), timing),
            model.completion_latency(request(2), timing)
        );
    }

    #[test]
    fn rcp_pi_timing_keeps_typed_and_raw_dma_busy_until_the_exact_deadline() {
        let rom = (0u8..=127).collect::<Vec<_>>();
        let make_fabric = || {
            let mut fabric = DeviceFabric::new(
                PiDma::new(InMemoryRom::new(rom.clone())),
                RcpPiTiming,
            );
            fabric.set_pi_domain_timing(
                PiDomain::Domain1,
                PiDomainTiming {
                    latency: 0x40,
                    pulse_width: 0x12,
                    page_size: 7,
                    release: 3,
                },
            );
            fabric
        };
        let request = PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0),
            device: PiDeviceAddress::RomOffset(0),
            len: 128,
        };
        let mut typed = make_fabric();
        let mut raw = make_fabric();
        let mut typed_rdram = Rdram::new(128);
        let mut raw_rdram = Rdram::new(128);

        typed.start_pi_dma(request).unwrap();
        raw.write_mmio(PI_DRAM_ADDR_REG, 0).unwrap();
        raw.write_mmio(PI_CART_ADDR_REG, 0x1000_0000).unwrap();
        raw.write_mmio(PI_WR_LEN_REG, 127).unwrap();
        assert_eq!(raw.snapshot(), typed.snapshot());

        for (fabric, rdram) in [
            (&mut typed, &mut typed_rdram),
            (&mut raw, &mut raw_rdram),
        ] {
            assert!(fabric
                .advance_to(at(2_368), rdram)
                .unwrap()
                .is_empty());
            assert_ne!(
                fabric.read_mmio(PI_STATUS_REG).unwrap() & PI_STATUS_DMA_BUSY,
                0
            );
            assert_eq!(rdram.read_bytes(0, 128), vec![0; 128]);
            assert_eq!(
                fabric.advance_to(at(2_369), rdram).unwrap(),
                vec![DeviceNotification::PiDmaComplete(DmaCompletion {
                    direction: request.direction,
                    dram_addr: request.dram_addr,
                    device: request.device,
                    len: request.len,
                })]
            );
            assert_eq!(
                fabric.read_mmio(PI_STATUS_REG).unwrap() & PI_STATUS_DMA_BUSY,
                0
            );
            assert_ne!(rdram.read_bytes(0, 128), vec![0; 128]);
        }
        assert_eq!(raw.snapshot(), typed.snapshot());
        assert_eq!(raw.trace(), typed.trace());
        assert_eq!(
            raw_rdram.read_bytes(0, 128),
            typed_rdram.read_bytes(0, 128)
        );
    }

    #[test]
    fn release_evidence_distinguishes_pif_state_that_the_compact_snapshot_cannot() {
        let mut left = fabric();
        let mut right = fabric();
        left.pif_ram_cpu_write_w(0, 0x1122_3344);
        right.pif_ram_cpu_write_w(0, 0x5566_7788);

        assert_eq!(left.snapshot(), right.snapshot());
        assert_ne!(left.evidence_snapshot(), right.evidence_snapshot());

        let request = SiDmaRequest {
            kind: SiDmaKind::PifToDram,
            dram_addr: RdramAddr::from_offset(0),
        };
        left.start_si_dma(request).unwrap();
        right.start_si_dma(request).unwrap();
        let mut left_rdram = Rdram::new(64);
        let mut right_rdram = Rdram::new(64);
        left.advance_to_with_pif(at(1), &mut left_rdram, |_, _, _| {})
            .unwrap();
        right
            .advance_to_with_pif(at(1), &mut right_rdram, |_, _, _| {})
            .unwrap();
        assert_ne!(left_rdram.read_bytes(0, 64), right_rdram.read_bytes(0, 64));
    }


    #[test]
    fn release_evidence_binds_rsp_memory_and_queued_ai_identity() {
        let mut left = fabric();
        let mut right = fabric();
        left.write_mmio(MmioAddr::new(SP_DMEM_START), 0x1122_3344)
            .unwrap();
        right
            .write_mmio(MmioAddr::new(SP_DMEM_START), 0x5566_7788)
            .unwrap();
        assert_eq!(left.snapshot(), right.snapshot());
        assert_ne!(left.evidence_snapshot(), right.evidence_snapshot());

        let current = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x20),
            len: 0x100,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };
        let mut left = fabric();
        let mut right = fabric();
        left.configure_tv_type(TvType::Ntsc).unwrap();
        right.configure_tv_type(TvType::Ntsc).unwrap();
        left.write_mmio(AI_CONTROL_REG, 1).unwrap();
        right.write_mmio(AI_CONTROL_REG, 1).unwrap();
        left.start_ai_dma(current).unwrap();
        right.start_ai_dma(current).unwrap();
        left.start_ai_dma(AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x200),
            ..current
        })
        .unwrap();
        right
            .start_ai_dma(AiDmaRequest {
                dram_addr: RdramAddr::from_offset(0x200),
                len: 0x108,
                ..current
            })
            .unwrap();
        assert_eq!(left.snapshot(), right.snapshot());
        assert_ne!(left.evidence_snapshot(), right.evidence_snapshot());
    }


    #[test]
    fn release_evidence_binds_save_bytes_and_pending_eeprom_programming() {
        use crate::save::{InMemorySaveStorage, SaveType};

        let mut left = fabric();
        let mut right = fabric();
        left.pi_dma_mut()
            .set_save(Box::new(InMemorySaveStorage::for_device(
                SaveType::Eeprom4k,
            )));
        right
            .pi_dma_mut()
            .set_save(Box::new(InMemorySaveStorage::for_device(
                SaveType::Eeprom4k,
            )));
        left.pi_dma_mut().save_write_from(0, &[0x11; 8]);
        right.pi_dma_mut().save_write_from(0, &[0x22; 8]);
        assert_eq!(left.snapshot(), right.snapshot());
        assert_ne!(left.evidence_snapshot(), right.evidence_snapshot());

        left.pi_dma_mut().save_write_from(0, &[0x33; 8]);
        right.pi_dma_mut().save_write_from(0, &[0x33; 8]);
        left.pi_dma_mut()
            .start_eeprom_write(Cycles::ZERO, 1, [0x44; 8])
            .unwrap();
        right
            .pi_dma_mut()
            .start_eeprom_write(Cycles::ZERO, 1, [0x55; 8])
            .unwrap();
        assert_eq!(left.snapshot(), right.snapshot());
        assert_ne!(left.evidence_snapshot(), right.evidence_snapshot());
    }


    #[test]
    #[should_panic(expected = "PiTimingModel::evidence_bytes must identify")]
    fn release_evidence_rejects_an_unidentified_pi_timing_policy() {
        struct UnidentifiedTiming;
        impl PiTimingModel for UnidentifiedTiming {
            fn completion_latency(
                &self,
                _request: PiDmaRequest,
                _timing: PiDomainTiming,
            ) -> Cycles {
                Cycles::new(1)
            }

            fn evidence_bytes(&self) -> Vec<u8> {
                Vec::new()
            }
        }

        let mut fabric =
            DeviceFabric::new(PiDma::new(InMemoryRom::new(Vec::new())), UnidentifiedTiming);
        let _ = fabric.evidence_snapshot();
    }


    #[test]
    fn raw_mi_mask_commands_drive_the_cpu_interrupt_gate() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        let request = PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x20),
            device: PiDeviceAddress::RomOffset(0x10),
            len: 4,
        };
        fabric.start_pi_dma(request).unwrap();
        fabric.advance_to(at(12), &mut rdram).unwrap();

        assert!(!fabric.cpu_interrupt_pending());
        fabric.write_mmio(MI_INTR_MASK_REG, 1 << 9).unwrap();
        assert_eq!(
            fabric.read_mmio(MI_INTR_MASK_REG).unwrap(),
            InterruptSource::Pi.bit()
        );
        assert!(fabric.cpu_interrupt_pending());

        fabric.write_mmio(MI_INTR_MASK_REG, 1 << 8).unwrap();
        assert_eq!(fabric.read_mmio(MI_INTR_MASK_REG).unwrap(), 0);
        assert!(!fabric.cpu_interrupt_pending());
    }


    #[test]
    fn every_rcp_source_uses_the_same_level_sensitive_mi_gate() {
        let mut fabric = fabric();
        for source in [
            InterruptSource::Sp,
            InterruptSource::Si,
            InterruptSource::Ai,
            InterruptSource::Vi,
            InterruptSource::Pi,
            InterruptSource::Dp,
        ] {
            fabric.set_interrupt_mask(source, true);
            fabric.raise_interrupt(source);
            fabric.raise_interrupt(source);
            assert!(fabric.interrupt_pending(source));
            assert!(fabric.cpu_interrupt_pending());
            fabric.clear_interrupt(source);
            assert!(!fabric.interrupt_pending(source));
            fabric.set_interrupt_mask(source, false);
        }
        assert_eq!(
            fabric
                .trace()
                .iter()
                .filter(|event| matches!(event.kind, DeviceTraceKind::MiInterruptRaised(_)))
                .count(),
            6
        );
        assert_eq!(
            fabric
                .trace()
                .iter()
                .filter(|event| matches!(event.kind, DeviceTraceKind::MiInterruptCleared(_)))
                .count(),
            6
        );
    }


    #[test]
    fn ai_fifo_drains_on_guest_cycles_and_raises_one_shared_mi_source() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        let first = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 400,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };
        let second = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x2000),
            ..first
        };
        fabric.start_ai_dma(first).unwrap();
        fabric.start_ai_dma(second).unwrap();
        assert_eq!(
            fabric.ai_status(),
            AI_STATUS_ENABLED | AI_STATUS_BUSY | AI_STATUS_FULL
        );
        assert_eq!(fabric.ai_length(), 400);
        assert_eq!(fabric.start_ai_dma(first), Err(DeviceFault::AiFull));

        let mut rdram = Rdram::new(0x100);
        assert!(fabric
            .advance_to(at(192), &mut rdram)
            .unwrap()
            .is_empty());
        assert!(fabric.ai_length() > 0);
        let first_done = fabric.advance_to(at(193), &mut rdram).unwrap();
        assert_eq!(
            first_done,
            vec![
                DeviceNotification::AiDmaStarted(AiDmaStart {
                    id: AiDmaId(2),
                    request: second,
                    started_at: at(193),
                    dacrate: 0,
                }),
                DeviceNotification::AiDmaComplete(first),
            ]
        );
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED | AI_STATUS_BUSY);
        assert_eq!(fabric.ai_length(), 400);
        assert!(fabric.interrupt_pending(InterruptSource::Ai));

        fabric.clear_interrupt(InterruptSource::Ai);
        // The SECOND (final) completion raises AI too. This previously
        // asserted no notification and no interrupt, which made the two
        // completions asymmetric: the first raised because a buffer was
        // queued behind it, the last did not. Hardware does not distinguish
        // them: rcp.h documents `AI_STATUS_FIFO_FULL` as a read status bit
        // (`ultra64/rcp.h:576`) and says only that a WRITE to
        // `AI_STATUS_REG` clears the audio interrupt (`:570`). Nothing makes
        // a FIFO-full transition the raising edge, and the libultra
        // single-buffer contract requires the final completion to signal.
        let second_done = fabric.advance_to(at(386), &mut rdram).unwrap();
        assert_eq!(second_done, vec![DeviceNotification::AiDmaComplete(second)]);
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED);
        assert_eq!(fabric.ai_length(), 0);
        assert!(fabric.interrupt_pending(InterruptSource::Ai));
    }


    #[test]
    fn ai_control_gates_drain_without_rejecting_fifo_writes() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        let request = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 0x80,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };

        fabric.write_mmio(AI_DRAM_ADDR_REG, 0x1000).unwrap();
        let ai_events_before = fabric
            .evidence_snapshot()
            .scheduled_events
            .iter()
            .filter(|event| event.kind == ScheduledDeviceEventKind::Ai)
            .count();
        assert_eq!(
            fabric.write_mmio(AI_LEN_REG, 0x80).unwrap(),
            DeviceMmioWriteEffect::AiDmaAccepted(AiDmaAdmission {
                id: AiDmaId(1),
                request,
                start: None,
            })
        );
        assert_eq!(fabric.ai_status(), AI_STATUS_BUSY);
        assert_eq!(fabric.ai_length(), 0x80);
        assert_eq!(
            fabric
                .evidence_snapshot()
                .scheduled_events
                .iter()
                .filter(|event| event.kind == ScheduledDeviceEventKind::Ai)
                .count(),
            ai_events_before
        );
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED | AI_STATUS_BUSY);
        assert_eq!(
            fabric
                .evidence_snapshot()
                .scheduled_events
                .iter()
                .filter(|event| event.kind == ScheduledDeviceEventKind::Ai)
                .count(),
            ai_events_before + 1
        );
        assert_eq!(
            fabric.write_mmio(AI_CONTROL_REG, 0),
            Err(DeviceFault::AiControlWhileBusy {
                current: 1,
                requested: 0,
            })
        );
        assert_eq!(fabric.ai_control(), 1);
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED | AI_STATUS_BUSY);
    }


    #[test]
    fn ai_disabled_fifo_accepts_two_slots_and_each_completion_interrupts() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        let first = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 8,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };
        let second = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x2000),
            ..first
        };
        fabric.start_ai_dma(first).unwrap();
        fabric.start_ai_dma(second).unwrap();
        assert_eq!(fabric.ai_status(), AI_STATUS_BUSY | AI_STATUS_FULL);
        assert_eq!(fabric.ai_length(), 8);
        assert_eq!(fabric.start_ai_dma(first), Err(DeviceFault::AiFull));
        assert_eq!(
            fabric
                .evidence_snapshot()
                .scheduled_events
                .iter()
                .filter(|event| event.kind == ScheduledDeviceEventKind::Ai)
                .count(),
            0
        );

        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        let first_deadline = fabric.current_ai.unwrap().deadline;
        let mut rdram = Rdram::new(0);
        assert_eq!(
            fabric.advance_to(first_deadline, &mut rdram).unwrap(),
            vec![
                DeviceNotification::AiDmaStarted(AiDmaStart {
                    id: AiDmaId(2),
                    request: second,
                    started_at: first_deadline,
                    dacrate: 0,
                }),
                DeviceNotification::AiDmaComplete(first),
            ]
        );
        assert!(fabric.interrupt_pending(InterruptSource::Ai));
        assert_eq!(fabric.current_ai.unwrap().request, second);
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED | AI_STATUS_BUSY);

        fabric.clear_interrupt(InterruptSource::Ai);
        let second_deadline = fabric.current_ai.unwrap().deadline;
        // The final buffer's completion raises AI as well -- see the note on
        // `ai_fifo_drains_on_guest_cycles_and_raises_one_shared_mi_source`.
        // The test name's "interrupts once" described fn64's FIFO-full gate,
        // not any documented rule.
        assert_eq!(
            fabric.advance_to(second_deadline, &mut rdram).unwrap(),
            vec![DeviceNotification::AiDmaComplete(second)]
        );
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED);
        assert!(fabric.interrupt_pending(InterruptSource::Ai));
    }


    #[test]
    fn device_clock_commits_eeprom_without_requiring_another_si_command() {
        use crate::save::{InMemorySaveStorage, SaveType, EEPROM_WRITE_CYCLES};

        let mut fabric = fabric();
        fabric
            .pi_dma_mut()
            .set_save(Box::new(InMemorySaveStorage::for_device(
                SaveType::Eeprom4k,
            )));
        let data = [0x3C; crate::save::EEPROM_BLOCK_SIZE];
        let deadline = fabric
            .pi_dma_mut()
            .start_eeprom_write(Cycles::ZERO, 5, data)
            .unwrap();
        assert_eq!(deadline, EEPROM_WRITE_CYCLES);

        let mut rdram = Rdram::new(0);
        fabric
            .advance_to(at(deadline.get() - 1), &mut rdram)
            .unwrap();
        assert!(
            fabric
                .pi_dma_mut()
                .eeprom_status(Cycles::new(deadline.get() - 1))
                .unwrap()
                .busy
        );
        fabric.advance_to(at(deadline.get()), &mut rdram).unwrap();
        assert_eq!(
            fabric.pi_dma_mut().eeprom_read_block(deadline, 5).unwrap(),
            data
        );
    }


    #[test]
    fn si_write_execute_read_uses_one_timed_pif_ram_and_mi_latch() {
        let mut fabric = fabric();
        fabric.set_si_latency(Cycles::new(5));
        let mut rdram = Rdram::new(0x200);
        rdram.dma_write_bytes(0x40, &[1, 3, 0xFF, 0]);

        fabric.write_mmio(SI_DRAM_ADDR_REG, 0x40).unwrap();
        fabric.write_mmio(SI_PIF_ADDR_WR64B_REG, 0).unwrap();
        assert_eq!(fabric.si_status() & 1, 1);
        assert!(fabric
            .advance_to_with_pif(at(4), &mut rdram, |_, _, _| unreachable!())
            .unwrap()
            .is_empty());
        let write_done = fabric
            .advance_to_with_pif(at(5), &mut rdram, |_, pif, _| {
                assert_eq!(&pif[..4], &[1, 3, 0xFF, 0]);
                pif[3..6].copy_from_slice(&[0x05, 0, 0]);
            })
            .unwrap();
        assert_eq!(
            write_done,
            vec![DeviceNotification::SiDmaComplete(SiDmaRequest {
                kind: SiDmaKind::DramToPif,
                dram_addr: RdramAddr::from_offset(0x40),
            })]
        );
        assert_eq!(fabric.si_status(), 1 << 12);
        fabric.write_mmio(SI_STATUS_REG, 0).unwrap();

        fabric.write_mmio(SI_DRAM_ADDR_REG, 0x80).unwrap();
        fabric.write_mmio(SI_PIF_ADDR_RD64B_REG, 0).unwrap();
        fabric
            .advance_to_with_pif(at(10), &mut rdram, |_, _, _| unreachable!())
            .unwrap();
        assert_eq!(rdram.dma_read_bytes_flat(0x83, 3), vec![0x05, 0, 0]);
        assert!(fabric.interrupt_pending(InterruptSource::Si));
    }


    #[test]
    fn sp_rectangular_dma_is_aligned_timed_and_replaces_imem_once() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        rdram.dma_write_bytes(0x20, &[0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17]);
        rdram.dma_write_bytes(0x30, &[0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27]);

        fabric.write_mmio(SP_MEM_ADDR_REG, 0x1003).unwrap();
        fabric.write_mmio(SP_DRAM_ADDR_REG, 0x23).unwrap();
        let encoded = (8 << 20) | (1 << 12);
        fabric.write_mmio(SP_RD_LEN_REG, encoded).unwrap();
        assert_eq!(fabric.read_mmio(SP_DMA_BUSY_REG).unwrap(), 1);
        assert_eq!(
            fabric.read_mmio(SP_STATUS_REG).unwrap() & SP_STATUS_DMA_BUSY,
            SP_STATUS_DMA_BUSY
        );
        assert_eq!(fabric.snapshot().sp_imem_generation, 0);

        assert!(fabric
            .advance_to(at(9), &mut rdram)
            .unwrap()
            .is_empty());
        assert_eq!(
            fabric
                .rsp_memory()
                .read_bytes(RspMemAddr::from_register(0x1000), 16)
                .unwrap(),
            [0; 16]
        );

        assert!(fabric
            .advance_to(at(10), &mut rdram)
            .unwrap()
            .is_empty());
        assert_eq!(
            fabric
                .rsp_memory()
                .read_bytes(RspMemAddr::from_register(0x1000), 16)
                .unwrap(),
            [
                0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25,
                0x26, 0x27,
            ]
        );
        assert_eq!(fabric.snapshot().sp_imem_generation, 1);
        assert_eq!(fabric.read_mmio(SP_DMA_BUSY_REG).unwrap(), 0);
    }


    #[test]
    fn sp_dma_pending_slot_starts_before_busy_can_clear() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        rdram.dma_write_bytes(0x20, &[1; 8]);
        rdram.dma_write_bytes(0x30, &[2; 8]);

        fabric.write_mmio(SP_MEM_ADDR_REG, 0).unwrap();
        fabric.write_mmio(SP_DRAM_ADDR_REG, 0x20).unwrap();
        fabric.write_mmio(SP_RD_LEN_REG, 7).unwrap();
        fabric.write_mmio(SP_MEM_ADDR_REG, 8).unwrap();
        fabric.write_mmio(SP_DRAM_ADDR_REG, 0x30).unwrap();
        fabric.write_mmio(SP_RD_LEN_REG, 7).unwrap();
        assert_eq!(fabric.read_mmio(SP_DMA_FULL_REG).unwrap(), 1);
        assert_eq!(
            fabric.write_mmio(SP_RD_LEN_REG, 7),
            Err(DeviceFault::SpDmaFull)
        );

        fabric.advance_to(at(9), &mut rdram).unwrap();
        assert_eq!(fabric.read_mmio(SP_DMA_BUSY_REG).unwrap(), 1);
        assert_eq!(fabric.read_mmio(SP_DMA_FULL_REG).unwrap(), 0);
        assert_eq!(
            fabric
                .rsp_memory()
                .read_bytes(RspMemAddr::from_register(0), 16)
                .unwrap(),
            [1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0]
        );

        fabric.advance_to(at(18), &mut rdram).unwrap();
        assert_eq!(fabric.read_mmio(SP_DMA_BUSY_REG).unwrap(), 0);
        assert_eq!(
            fabric
                .rsp_memory()
                .read_bytes(RspMemAddr::from_register(0), 16)
                .unwrap(),
            [1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2]
        );
        assert_eq!(
            fabric
                .trace()
                .iter()
                .filter(|event| matches!(event.kind, DeviceTraceKind::SpDmaBusyCleared))
                .count(),
            1
        );
    }


    #[test]
    fn cpu_sp_memory_pc_status_semaphore_and_write_dma_share_one_state() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);

        fabric
            .write_mmio(MmioAddr::new(0xA400_0040), 0xDEAD_BEEF)
            .unwrap();
        assert_eq!(
            fabric.read_mmio(MmioAddr::new(0xA400_0040)).unwrap(),
            0xDEAD_BEEF
        );
        fabric.write_mmio(SP_PC_REG, 0x1ABC).unwrap();
        assert_eq!(fabric.read_mmio(SP_PC_REG).unwrap(), 0x0ABC);
        assert_eq!(fabric.read_mmio(SP_SEMAPHORE_REG).unwrap(), 0);
        assert_eq!(fabric.read_mmio(SP_SEMAPHORE_REG).unwrap(), 1);
        fabric.write_mmio(SP_SEMAPHORE_REG, 0).unwrap();
        assert_eq!(fabric.read_mmio(SP_SEMAPHORE_REG).unwrap(), 0);

        fabric
            .write_mmio(SP_STATUS_REG, (1 << 0) | SP_SET_YIELD)
            .unwrap();
        assert_eq!(fabric.read_mmio(SP_STATUS_REG).unwrap(), SP_STATUS_YIELD);
        fabric.write_mmio(SP_MEM_ADDR_REG, 0x40).unwrap();
        fabric.write_mmio(SP_DRAM_ADDR_REG, 0x80).unwrap();
        fabric.write_mmio(SP_WR_LEN_REG, 7).unwrap();
        fabric.advance_to(at(9), &mut rdram).unwrap();
        assert_eq!(
            rdram.dma_read_bytes_flat(0x80, 8),
            [0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0]
        );
    }


    #[test]
    fn sp_dma_crossing_a_memory_bank_is_a_named_fault() {
        let mut fabric = fabric();
        fabric.write_mmio(SP_MEM_ADDR_REG, 0x0ff8).unwrap();
        fabric.write_mmio(SP_DRAM_ADDR_REG, 0).unwrap();
        let request = SpDmaRequest {
            direction: SpDmaDirection::RdramToRsp,
            mem_addr: RspMemAddr::from_register(0x0ff8),
            dram_addr: RdramAddr::from_offset(0),
            encoded_len: 15,
        };
        assert_eq!(
            fabric.write_mmio(SP_RD_LEN_REG, 15),
            Err(DeviceFault::SpDmaMemory(RspMemoryError::CrossesBank {
                addr: request.mem_addr,
                len: request.total_bytes(),
            }))
        );
    }


    #[test]
    fn graphics_task_completes_sp_then_dp_on_distinct_guest_cycles() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        fabric
            .start_rcp_task(RcpTaskCompletionPlan::SpThenDpFullSync)
            .unwrap();
        assert!(fabric.snapshot().sp_busy);
        assert!(fabric.snapshot().dp_busy);
        assert!(fabric
            .advance_to(at(0), &mut rdram)
            .unwrap()
            .is_empty());

        let sp = fabric.advance_to(at(1), &mut rdram).unwrap();
        assert_eq!(
            sp,
            vec![DeviceNotification::RcpTaskComplete(RcpTaskCompletion::Sp)]
        );
        assert!(!fabric.snapshot().sp_busy);
        assert!(fabric.snapshot().dp_busy);
        assert!(fabric.interrupt_pending(InterruptSource::Sp));
        assert!(!fabric.interrupt_pending(InterruptSource::Dp));

        let dp = fabric.advance_to(at(2), &mut rdram).unwrap();
        assert_eq!(
            dp,
            vec![DeviceNotification::RcpTaskComplete(RcpTaskCompletion::Dp)]
        );
        assert!(!fabric.snapshot().dp_busy);
        assert!(fabric.interrupt_pending(InterruptSource::Dp));
    }


    #[test]
    fn task_without_dp_full_sync_completes_sp_only() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        fabric
            .start_rcp_task(RcpTaskCompletionPlan::SpOnly)
            .unwrap();
        assert!(fabric.snapshot().sp_busy);
        assert!(!fabric.snapshot().dp_busy);

        assert_eq!(
            fabric.advance_to(at(1), &mut rdram).unwrap(),
            vec![DeviceNotification::RcpTaskComplete(RcpTaskCompletion::Sp)]
        );
        assert!(!fabric.interrupt_pending(InterruptSource::Dp));
        assert!(fabric
            .advance_to(at(2), &mut rdram)
            .unwrap()
            .is_empty());
    }


    #[test]
    fn chunked_rcp_task_is_busy_without_a_fabricated_completion_deadline() {
        let mut fabric = fabric();
        fabric.begin_rcp_task().unwrap();
        assert!(fabric.snapshot().sp_busy);
        assert_eq!(fabric.next_deadline(), None);

        fabric
            .finish_rcp_task(RcpTaskCompletionPlan::SpOnly, Cycles::new(2))
            .unwrap();
        assert_eq!(fabric.next_deadline(), Some(at(2)));
        assert_eq!(
            fabric.finish_rcp_task(RcpTaskCompletionPlan::SpOnly, Cycles::new(1)),
            Err(DeviceFault::SpBusy),
            "one in-flight task token may acquire only one completion event"
        );
    }


    #[test]
    fn raw_dp_full_sync_completes_dp_without_starting_sp() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        let schedule = fabric.start_dp_full_sync(Cycles::new(3)).unwrap();
        assert_eq!(schedule.deadline(), at(3));
        assert!(!fabric.snapshot().sp_busy);
        assert!(fabric.snapshot().dp_busy);
        assert!(fabric
            .advance_to(at(2), &mut rdram)
            .unwrap()
            .is_empty());

        assert_eq!(
            fabric.advance_to(at(3), &mut rdram).unwrap(),
            vec![DeviceNotification::RcpTaskComplete(RcpTaskCompletion::Dp)]
        );
        assert!(!fabric.interrupt_pending(InterruptSource::Sp));
        assert!(fabric.interrupt_pending(InterruptSource::Dp));
    }


    #[test]
    fn second_raw_dp_full_sync_rejects_without_replacing_pending_completion() {
        let mut fabric = fabric();
        fabric.start_dp_full_sync(Cycles::new(3)).unwrap();
        let before = fabric.evidence_snapshot();

        assert_eq!(
            fabric.start_dp_full_sync(Cycles::new(1)),
            Err(DeviceFault::DpBusy)
        );
        assert_eq!(fabric.evidence_snapshot(), before);

        let mut rdram = Rdram::new(0x100);
        assert!(fabric
            .advance_to(at(1), &mut rdram)
            .unwrap()
            .is_empty());
        assert_eq!(
            fabric.advance_to(at(3), &mut rdram).unwrap(),
            vec![DeviceNotification::RcpTaskComplete(RcpTaskCompletion::Dp)]
        );
    }


    /// The reserve half of the FullSync two-phase contract raises nothing.
    ///
    /// This is the evidence behind `RawDpcIrCapability::
    /// TransactionalTmemFillFullSyncSiteOnly`'s nonclaim and behind the
    /// `Clear`/`Clear` boundary the ABI producer supplies: a successful
    /// `preflight_dp_full_sync` leaves the DP interrupt line down, the slot
    /// free, no event scheduled, and the evidence snapshot byte-identical.
    /// A renderer that has only reserved therefore has nothing to observe,
    /// and a boundary claiming `interrupt_after == Asserted` off the back of
    /// one would be fabricating an edge this test proves does not exist.
    #[test]
    fn preflight_dp_full_sync_reserves_without_raising_scheduling_or_consuming_the_slot() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        let before = fabric.evidence_snapshot();

        fabric.preflight_dp_full_sync(Cycles::new(3)).unwrap();

        // Nonmutating: no interrupt, no busy slot, no recorded evidence.
        assert!(!fabric.interrupt_pending(InterruptSource::Dp));
        assert!(!fabric.snapshot().dp_busy);
        assert_eq!(fabric.evidence_snapshot(), before);

        // No event was scheduled, so advancing well past any deadline the
        // commit half would have used still produces nothing.
        assert!(fabric
            .advance_to(at(10), &mut rdram)
            .unwrap()
            .is_empty());
        assert!(!fabric.interrupt_pending(InterruptSource::Dp));

        // The slot the reserve proved free is still free: the commit half
        // succeeds afterwards, and only then does the interrupt arrive.
        fabric.start_dp_full_sync(Cycles::new(3)).unwrap();
        assert!(fabric.snapshot().dp_busy);
        assert!(!fabric.interrupt_pending(InterruptSource::Dp));
        assert_eq!(
            fabric.advance_to(at(13), &mut rdram).unwrap(),
            vec![DeviceNotification::RcpTaskComplete(RcpTaskCompletion::Dp)]
        );
        assert!(fabric.interrupt_pending(InterruptSource::Dp));
    }

    /// The reserve half rejects an occupied slot, which is what lets a
    /// renderer be turned away before it observes or changes guest memory.
    #[test]
    fn preflight_dp_full_sync_rejects_an_occupied_slot_without_disturbing_it() {
        let mut fabric = fabric();
        fabric.start_dp_full_sync(Cycles::new(3)).unwrap();
        let before = fabric.evidence_snapshot();

        assert_eq!(
            fabric.preflight_dp_full_sync(Cycles::new(1)),
            Err(DeviceFault::DpBusy)
        );
        assert_eq!(fabric.evidence_snapshot(), before);
        assert!(fabric.snapshot().dp_busy);
    }


    #[test]
    fn pi_channel_serializes_requests_and_time_never_moves_backward() {
        let mut fabric = fabric();
        let request = PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x20),
            device: PiDeviceAddress::RomOffset(0x10),
            len: 4,
        };
        fabric.start_pi_dma(request).unwrap();
        assert_eq!(fabric.start_pi_dma(request), Err(DeviceFault::PiBusy));

        let mut rdram = Rdram::new(0x100);
        fabric.advance_to(at(12), &mut rdram).unwrap();
        assert_eq!(
            fabric.advance_to(at(11), &mut rdram),
            Err(DeviceFault::TimeWentBack {
                now: at(12),
                requested: at(11),
            })
        );
    }


    #[test]
    fn unknown_or_unaligned_registers_fail_loudly() {
        let mut fabric = fabric();
        assert_eq!(
            fabric.read_mmio(MmioAddr::new(0xA460_0001)),
            Err(DeviceFault::UnalignedMmio {
                addr: MmioAddr::new(0xA460_0001)
            })
        );
        assert_eq!(
            fabric.write_mmio(MmioAddr::new(0xA460_0034), 7),
            Err(DeviceFault::UnmodeledMmioWrite {
                addr: MmioAddr::new(0xA460_0034),
                value: 7,
            })
        );
    }


    #[test]
    fn pi_domain_registers_are_the_timing_models_typed_input() {
        let mut fabric = fabric();
        fabric.write_mmio(PI_DOM2_LAT_REG, 0x105).unwrap();
        fabric.write_mmio(PI_DOM2_PWD_REG, 0x20C).unwrap();
        fabric.write_mmio(PI_DOM2_PGS_REG, 0x1D).unwrap();
        fabric.write_mmio(PI_DOM2_RLS_REG, 0x6).unwrap();

        assert_eq!(
            fabric.pi_domain_timing(PiDomain::Domain2),
            PiDomainTiming {
                latency: 0x05,
                pulse_width: 0x0C,
                page_size: 0x0D,
                release: 0x02,
            }
        );
        assert_eq!(fabric.read_mmio(PI_DOM2_LAT_REG).unwrap(), 0x05);
        assert_eq!(fabric.read_mmio(PI_DOM2_PWD_REG).unwrap(), 0x0C);
        assert_eq!(fabric.read_mmio(PI_DOM2_PGS_REG).unwrap(), 0x0D);
        assert_eq!(fabric.read_mmio(PI_DOM2_RLS_REG).unwrap(), 0x02);

        assert_eq!(
            PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0),
                device: PiDeviceAddress::SramOffset(0),
                len: 2,
            }
            .domain(),
            PiDomain::Domain2
        );
    }


    #[test]
    fn pi_reset_cancels_the_owned_completion_event() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        fabric
            .start_pi_dma(PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0x20),
                device: PiDeviceAddress::RomOffset(0x10),
                len: 4,
            })
            .unwrap();
        fabric.write_mmio(PI_STATUS_REG, 0b1).unwrap();

        assert_eq!(fabric.read_mmio(PI_STATUS_REG).unwrap(), 0);
        assert!(fabric
            .advance_to(at(12), &mut rdram)
            .unwrap()
            .is_empty());
        assert_eq!(rdram.read_w(RdramAddr::from_offset(0x20)), 0);
        assert!(!fabric.interrupt_pending(InterruptSource::Pi));
    }


    #[test]
    fn vi_half_line_interrupt_latches_mi_before_notification_and_ack_preserves_line() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0);
        fabric.write_mmio(VI_V_SYNC_REG, 525).unwrap();
        fabric.write_mmio(VI_INTR_REG, 100).unwrap();
        fabric.arm_vi(Cycles::new(1_000)).unwrap();

        assert!(fabric
            .advance_to(at(190), &mut rdram)
            .unwrap()
            .is_empty());
        assert_eq!(fabric.read_mmio(VI_CURRENT_REG).unwrap(), 98);

        let notifications = fabric.advance_to(at(191), &mut rdram).unwrap();
        assert_eq!(
            notifications,
            vec![DeviceNotification::ViRetrace {
                at: at(191)
            }]
        );
        assert_eq!(fabric.read_mmio(VI_CURRENT_REG).unwrap(), 100);
        assert!(fabric.interrupt_pending(InterruptSource::Vi));

        let tail = &fabric.trace()[fabric.trace().len() - 3..];
        assert_eq!(tail[0].kind, DeviceTraceKind::ViInterrupt);
        assert_eq!(
            tail[1].kind,
            DeviceTraceKind::MiInterruptRaised(InterruptSource::Vi)
        );
        assert_eq!(
            tail[2].kind,
            DeviceTraceKind::NotificationReady(DeviceNotification::ViRetrace {
                at: at(191)
            })
        );

        fabric.write_mmio(VI_CURRENT_REG, u32::MAX).unwrap();
        assert!(!fabric.interrupt_pending(InterruptSource::Vi));
        assert_eq!(fabric.read_mmio(VI_CURRENT_REG).unwrap(), 100);
    }


    #[test]
    fn television_standard_waits_for_programmed_vi_timing_registers() {
        let mut fabric = fabric();
        assert_eq!(fabric.configure_tv_type(TvType::Pal).unwrap(), None);
        assert_eq!(fabric.tv_type(), Some(TvType::Pal));
        assert_eq!(fabric.read_mmio(VI_INTR_REG).unwrap(), 0x3ff);
        assert_eq!(fabric.next_vi_deadline(), None);

        fabric.write_mmio(VI_V_SYNC_REG, 525).unwrap();
        assert_eq!(fabric.vi_field_interval(), None);
        assert_eq!(fabric.next_vi_deadline(), None);
        fabric.write_mmio(VI_H_SYNC_REG, 3_093).unwrap();
        assert_eq!(
            fabric.vi_field_interval(),
            Some(Cycles::new(
                TvType::Pal.programmed_field_cycles(3_093, 525).unwrap()
            ))
        );
        let interval = fabric.vi_field_interval().unwrap();
        assert_eq!(
            fabric.next_vi_deadline(),
            Some(at(fabric.vi_interrupt_offset(interval).get()))
        );
        assert_eq!(fabric.snapshot().tv_type, Some(TvType::Pal));
    }


    #[test]
    fn repeated_vi_mode_writes_preserve_the_running_field_epoch() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0);
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        fabric.write_mmio(VI_V_SYNC_REG, 525).unwrap();
        fabric.write_mmio(VI_H_SYNC_REG, 3_093).unwrap();
        fabric.write_mmio(VI_INTR_REG, 2).unwrap();
        let interval = fabric.vi_field_interval().unwrap();
        let first = fabric.next_vi_deadline().unwrap();
        fabric.advance_to(first, &mut rdram).unwrap();
        fabric.write_mmio(VI_CURRENT_REG, 0).unwrap();

        fabric.write_mmio(VI_V_SYNC_REG, 525).unwrap();
        fabric.write_mmio(VI_H_SYNC_REG, 3_093).unwrap();

        assert_eq!(
            fabric.next_vi_deadline(),
            Some(at(first.get() + interval.get()))
        );
    }


    /// `vi_output_height` is the presenter's authority for how many lines
    /// the guest actually scans out. Pinned against WM2000's measured
    /// registers -- H_START `0x006c02ec`, V_START `0x002501ff` -- which
    /// decode to a 640x237 output rectangle, the same values
    /// `fn64_render::ViActiveWindow` asserts for the identical words.
    ///
    /// 237, not 240: a presenter that assumes 240 blits three rows of RDRAM
    /// the game never rendered into, which is visible as an edge band.
    #[test]
    fn vi_output_height_decodes_the_programmed_half_line_interval() {
        const VI_H_START_REG: MmioAddr = MmioAddr::new(0xA440_0024);
        const VI_V_START_REG: MmioAddr = MmioAddr::new(0xA440_0028);

        let mut fabric = fabric();
        // Register initialization is not atomic: neither interval alone is
        // an active window.
        assert_eq!(fabric.vi_output_height(), None);
        fabric.write_mmio(VI_V_START_REG, 0x0025_01ff).unwrap();
        assert_eq!(
            fabric.vi_output_height(),
            None,
            "V_START alone is not an active window"
        );

        fabric.write_mmio(VI_H_START_REG, 0x006c_02ec).unwrap();
        // (0x1ff - 0x25) / 2 = (511 - 37) / 2 = 474 / 2 = 237.
        assert_eq!(
            fabric.vi_output_height(),
            Some(237),
            "WM2000's V_START programs 237 output lines, not 240"
        );

        // Derived a second, independent way from the raw half-line fields,
        // so a transcription slip in either expression is caught.
        let (start, end) = (0x25u32, 0x1ffu32);
        assert_eq!(fabric.vi_output_height(), Some((end - start) / 2));

        // A full 240-line window decodes to 240, so the accessor is not
        // simply biased low.
        fabric.write_mmio(VI_V_START_REG, (0x25 << 16) | 0x205).unwrap();
        assert_eq!(fabric.vi_output_height(), Some(240));

        // An ODD half-line interval must truncate, not round up. Every real
        // window is an even number of half-lines (`ViActiveWindow` asserts
        // that), but the arithmetic here must still be the plain halving --
        // `(end - start + 1) / 2` agrees on every even interval and would
        // otherwise be an invisible substitution.
        fabric.write_mmio(VI_V_START_REG, (0x25 << 16) | 0x204).unwrap();
        assert_eq!(
            fabric.vi_output_height(),
            Some(239),
            "an odd half-line interval truncates; it does not round up"
        );
    }

    #[test]
    fn vi_current_and_field_follow_progressive_and_interlaced_half_line_sequences() {
        let mut progressive = fabric();
        let mut rdram = Rdram::new(0);
        progressive.write_mmio(VI_V_SYNC_REG, 525).unwrap();
        progressive.arm_vi(Cycles::new(1_000)).unwrap();

        progressive
            .advance_to(at(999), &mut rdram)
            .unwrap();
        assert_eq!(progressive.vi_field(), 0);
        assert_eq!(progressive.vi_current() & 1, 0);
        progressive
            .advance_to(at(1_000), &mut rdram)
            .unwrap();
        assert_eq!(progressive.vi_field(), 0);
        assert_eq!(progressive.vi_current(), 0);

        let mut interlaced = fabric();
        interlaced.write_mmio(VI_STATUS_REG, 1 << 6).unwrap();
        interlaced.write_mmio(VI_V_SYNC_REG, 525).unwrap();
        interlaced.arm_vi(Cycles::new(1_000)).unwrap();

        interlaced.advance_to(at(999), &mut rdram).unwrap();
        assert_eq!(interlaced.vi_field(), 0);
        assert_eq!(interlaced.vi_current() & 1, 0);
        interlaced
            .advance_to(at(1_000), &mut rdram)
            .unwrap();
        assert_eq!(interlaced.vi_field(), 1);
        assert_eq!(interlaced.vi_current(), 1);
        interlaced
            .advance_to(at(1_999), &mut rdram)
            .unwrap();
        assert_eq!(interlaced.vi_field(), 1);
        assert_eq!(interlaced.vi_current() & 1, 1);
        interlaced
            .advance_to(at(2_000), &mut rdram)
            .unwrap();
        assert_eq!(interlaced.vi_field(), 0);
        assert_eq!(interlaced.vi_current(), 0);
    }


    #[test]
    fn vi_raw_register_file_masks_documented_fields_and_reschedules_interrupt() {
        let mut fabric = fabric();
        fabric.write_mmio(VI_STATUS_REG, 0xFFFF_FFFF).unwrap();
        fabric.write_mmio(VI_ORIGIN_REG, 0xFFFF_FFFF).unwrap();
        fabric.write_mmio(VI_V_SYNC_REG, 0xFFFF_FFFF).unwrap();
        fabric.write_mmio(VI_INTR_REG, 0xFFFF_FFFF).unwrap();
        assert_eq!(fabric.read_mmio(VI_STATUS_REG).unwrap(), 0x1FFFF);
        assert_eq!(fabric.read_mmio(VI_ORIGIN_REG).unwrap(), 0x00FF_FFFF);
        assert_eq!(fabric.read_mmio(VI_V_SYNC_REG).unwrap(), 0x3FF);
        assert_eq!(fabric.read_mmio(VI_INTR_REG).unwrap(), 0x3FF);

        fabric.arm_vi(Cycles::new(1_000)).unwrap();
        let old_deadline = fabric.next_deadline().unwrap();
        fabric.write_mmio(VI_INTR_REG, 1).unwrap();
        let new_deadline = fabric.next_deadline().unwrap();
        assert!(new_deadline < old_deadline);
        assert_eq!(new_deadline, at(1));
    }

    // Seed the four DPC counters to distinct nonzero values (1,2,3,4) with the
    // renderer idle so counter-clear behavior can be observed in isolation.

    #[test]
    fn dpc_status_counter_clear_commands_are_selective() {
        let cases = [
            (DPC_STATUS_CLEAR_CLOCK_COUNTER_COMMAND, (0, 2, 3, 4)),
            (DPC_STATUS_CLEAR_CMD_COUNTER_COMMAND, (1, 0, 3, 4)),
            (DPC_STATUS_CLEAR_PIPE_COUNTER_COMMAND, (1, 2, 0, 4)),
            (DPC_STATUS_CLEAR_TMEM_COUNTER_COMMAND, (1, 2, 3, 0)),
        ];
        for (command, (clock, busy, pipe, tmem)) in cases {
            let mut fabric = fabric();
            seed_dpc_counters(&mut fabric);
            let status_before = fabric.read_mmio(DPC_STATUS_REG).unwrap();
            fabric.write_mmio(DPC_STATUS_REG, command).unwrap();
            let s = fabric.snapshot();
            assert_eq!(
                (s.dpc_clock, s.dpc_busy, s.dpc_pipe_busy, s.dpc_tmem_busy),
                (clock, busy, pipe, tmem),
                "command {command:#06x} cleared the wrong counter(s)"
            );
            assert_eq!(
                fabric.read_mmio(DPC_STATUS_REG).unwrap(),
                status_before,
                "a counter-clear command must not perturb STATUS mode bits"
            );
        }
    }


    #[test]
    fn dpc_counter_clears_during_renderer_admission_survive_cancellation() {
        let mut fabric = fabric();
        seed_dpc_counters(&mut fabric);
        let before = fabric.snapshot();

        let submission = fabric
            .request_dpc_submission(DpcSubmissionSource::Rdram, 0x100, 0x180)
            .unwrap()
            .expect("unfrozen DPC submission must publish");
        // Clear all four counters while the renderer submission is pending.
        let clear_all = DPC_STATUS_CLEAR_CLOCK_COUNTER_COMMAND
            | DPC_STATUS_CLEAR_CMD_COUNTER_COMMAND
            | DPC_STATUS_CLEAR_PIPE_COUNTER_COMMAND
            | DPC_STATUS_CLEAR_TMEM_COUNTER_COMMAND;
        fabric.write_mmio(DPC_STATUS_REG, clear_all).unwrap();
        fabric.cancel_dpc_submission(submission.token).unwrap();

        let after = fabric.snapshot();
        // Admission is reversed...
        assert_eq!(after.dpc_start, before.dpc_start);
        assert_eq!(after.dpc_end, before.dpc_end);
        assert_eq!(after.dpc_current, before.dpc_current);
        assert_eq!(after.dpc_status, before.dpc_status);
        // ...but the cleared counters are NOT resurrected by the rollback.
        assert_eq!(
            (
                after.dpc_clock,
                after.dpc_busy,
                after.dpc_pipe_busy,
                after.dpc_tmem_busy
            ),
            (0, 0, 0, 0),
            "cancellation must not resurrect counters cleared during admission"
        );
    }


    #[test]
    fn dpc_status_mode_commands_during_renderer_admission_survive_cancellation() {
        // (initial command, interleaved command, control mask, expected control after)
        let cases = [
            (
                0x01,
                0x02,
                DPC_STATUS_XBUS_DMEM_DMA,
                DPC_STATUS_XBUS_DMEM_DMA,
            ),
            (0x02, 0x01, DPC_STATUS_XBUS_DMEM_DMA, 0),
            (0x04, 0x08, DPC_STATUS_FREEZE, DPC_STATUS_FREEZE),
            (0x10, 0x20, DPC_STATUS_FLUSH, DPC_STATUS_FLUSH),
            (0x20, 0x10, DPC_STATUS_FLUSH, 0),
        ];
        for (initial, interleaved, control_mask, expected_control) in cases {
            let mut fabric = fabric();
            fabric.write_mmio(DPC_STATUS_REG, initial).unwrap();
            let before = fabric.snapshot();

            let submission = fabric
                .request_dpc_submission(DpcSubmissionSource::Dmem, 0x100, 0x180)
                .unwrap()
                .expect("unfrozen DPC submission must publish");
            fabric.write_mmio(DPC_STATUS_REG, interleaved).unwrap();
            fabric.cancel_dpc_submission(submission.token).unwrap();

            let after = fabric.snapshot();
            assert_eq!(after.dpc_start, before.dpc_start);
            assert_eq!(after.dpc_end, before.dpc_end);
            assert_eq!(after.dpc_current, before.dpc_current);
            assert_eq!(
                after.dpc_status & control_mask,
                expected_control,
                "interleaved mode command {interleaved:#04x} did not survive cancellation"
            );
            assert_eq!(
                after.dpc_status & !control_mask,
                before.dpc_status & !control_mask,
                "cancellation moved a status bit outside the interleaved command's mask"
            );
        }
    }


    #[test]
    fn raw_sp_status_clear_halt_requests_an_rsp_start_on_the_halted_edge() {
        // Hardware starts the RSP when a STATUS write clears HALT on a halted
        // unit. The device models registers, not execution, so it reports the
        // edge as an effect for a host to act on -- rather than latching the
        // bit and leaving a guest that kicked the RSP through raw MMIO waiting
        // forever on an SP interrupt nothing will raise.
        let mut fabric = fabric();
        fabric.write_mmio(SP_PC_REG, 0x0A8).unwrap();

        // Resets halted, so the first clear-halt is the starting edge.
        assert_eq!(
            fabric.write_mmio(SP_STATUS_REG, 1 << 0).unwrap(),
            DeviceMmioWriteEffect::RspStartRequested { pc: 0x0A8 }
        );

        // Already running: a repeated clear-halt must NOT re-enter a live task.
        assert_eq!(
            fabric.write_mmio(SP_STATUS_REG, 1 << 0).unwrap(),
            DeviceMmioWriteEffect::None
        );

        // Halting and clearing again is a fresh edge.
        assert_eq!(
            fabric.write_mmio(SP_STATUS_REG, 1 << 1).unwrap(),
            DeviceMmioWriteEffect::None
        );
        assert_eq!(
            fabric.write_mmio(SP_STATUS_REG, 1 << 0).unwrap(),
            DeviceMmioWriteEffect::RspStartRequested { pc: 0x0A8 }
        );
    }


    #[test]
    fn sp_status_writes_that_do_not_release_halt_request_no_start() {
        // Only the halt bit gates execution. Signal/interrupt/single-step
        // commands must not be mistaken for a kick.
        let mut fabric = fabric();
        for command in [1 << 2, 1 << 4, SP_SET_YIELD, SP_SET_YIELDED, 1 << 6] {
            assert_eq!(
                fabric.write_mmio(SP_STATUS_REG, command).unwrap(),
                DeviceMmioWriteEffect::None,
                "command {command:#x} released halt"
            );
        }
    }
