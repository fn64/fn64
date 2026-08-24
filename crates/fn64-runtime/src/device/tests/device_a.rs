use super::*;

    #[test]
    fn device_writes_name_the_exact_dma_producer() {
        let mut pi = fabric();
        let mut pi_storage = [0u8; 0x200];
        let mut pi_writers = Vec::new();
        let mut pi_committed = |channel, _, _| pi_writers.push(channel);
        let mut pi_memory = unsafe {
            ProcessDmaMemory::from_raw_parts(
                pi_storage.as_mut_ptr(),
                pi_storage.len(),
                &mut pi_committed,
            )
        };
        pi.start_pi_dma(PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x20),
            device: PiDeviceAddress::RomOffset(0x10),
            len: 4,
        })
        .unwrap();
        pi.advance_to(Cycles::new(12), &mut pi_memory).unwrap();
        drop(pi_memory);
        drop(pi_committed);
        assert_eq!(pi_writers, [DmaWriterChannel::Pi]);

        let mut si = fabric();
        si.set_si_latency(Cycles::new(1));
        si.pif_ram_cpu_write_w(0, 0x1122_3344);
        let mut si_storage = [0u8; 0x200];
        let mut si_writers = Vec::new();
        let mut si_committed = |channel, _, _| si_writers.push(channel);
        let mut si_memory = unsafe {
            ProcessDmaMemory::from_raw_parts(
                si_storage.as_mut_ptr(),
                si_storage.len(),
                &mut si_committed,
            )
        };
        si.start_si_dma(SiDmaRequest {
            kind: SiDmaKind::PifToDram,
            dram_addr: RdramAddr::from_offset(0x40),
        })
        .unwrap();
        si.advance_to_with_pif(Cycles::new(1), &mut si_memory, |_, _, _| {})
            .unwrap();
        drop(si_memory);
        drop(si_committed);
        assert_eq!(si_writers, [DmaWriterChannel::Si]);

        let mut sp = fabric();
        sp.rsp_memory_mut()
            .write_bytes(RspMemAddr::from_register(0), &[0x5a; 8])
            .unwrap();
        let mut sp_storage = [0u8; 0x200];
        let mut sp_writers = Vec::new();
        let mut sp_committed = |channel, _, _| sp_writers.push(channel);
        let mut sp_memory = unsafe {
            ProcessDmaMemory::from_raw_parts(
                sp_storage.as_mut_ptr(),
                sp_storage.len(),
                &mut sp_committed,
            )
        };
        sp.write_mmio(SP_MEM_ADDR_REG, 0).unwrap();
        sp.write_mmio(SP_DRAM_ADDR_REG, 0x80).unwrap();
        sp.write_mmio(SP_WR_LEN_REG, 7).unwrap();
        sp.advance_to(Cycles::new(9), &mut sp_memory).unwrap();
        drop(sp_memory);
        drop(sp_committed);
        assert_eq!(sp_writers, [DmaWriterChannel::Sp]);
    }


    #[test]
    fn disabled_device_trace_retention_keeps_constant_space_summary() {
        let mut fabric = fabric();
        fabric.set_trace_enabled(false);
        let request = PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0),
            device: PiDeviceAddress::RomOffset(0x10),
            len: 4,
        };

        fabric.start_pi_dma(request).unwrap();

        assert!(fabric.trace().is_empty());
        assert_eq!(fabric.trace_summary().events, 1);
        assert_eq!(fabric.trace_summary().pi_dma_started, 1);
    }


    #[test]
    fn complete_rsp_execution_state_commits_every_register_atomically() {
        let mut fabric = fabric();
        let state = complete_rsp_state();

        fabric.commit_complete_rsp_execution_state(state).unwrap();

        let expected = RspExecutionState {
            sp_status: state.sp_status & !(SP_STATUS_DMA_BUSY | SP_STATUS_DMA_FULL),
            dpc_clock: state.dpc_clock & DPC_COUNTER_MASK,
            dpc_busy: state.dpc_busy & DPC_COUNTER_MASK,
            dpc_pipe_busy: state.dpc_pipe_busy & DPC_COUNTER_MASK,
            dpc_tmem_busy: state.dpc_tmem_busy & DPC_COUNTER_MASK,
            ..state
        };
        assert_eq!(fabric.rsp_execution_state(), expected);
        let guest = fabric.snapshot();
        assert_eq!(guest.sp_status, expected.sp_status);
        assert_eq!(guest.sp_mem_addr, expected.sp_dma_mem_addr);
        assert_eq!(guest.sp_dram_addr, expected.sp_dma_dram_addr);
        assert_eq!(guest.dpc_start, expected.dpc_start);
        assert_eq!(guest.dpc_end, expected.dpc_end);
        assert_eq!(guest.dpc_current, expected.dpc_current);
        assert_eq!(guest.dpc_status, expected.dpc_status);
        assert_eq!(guest.dpc_clock, expected.dpc_clock);
        assert_eq!(guest.dpc_busy, expected.dpc_busy);
        assert_eq!(guest.dpc_pipe_busy, expected.dpc_pipe_busy);
        assert_eq!(guest.dpc_tmem_busy, expected.dpc_tmem_busy);
        let evidence = fabric.evidence_snapshot();
        assert_eq!(evidence.sp_rd_len, expected.sp_dma_read_length);
        assert_eq!(evidence.sp_wr_len, expected.sp_dma_write_length);
        assert_eq!(evidence.sp_pc, expected.pc);
        assert_eq!(evidence.sp_semaphore, expected.sp_semaphore);
        assert_eq!(evidence.guest, guest);
        assert_eq!(fabric.read_mmio(DPC_CLOCK_REG).unwrap(), expected.dpc_clock);
        assert_eq!(
            fabric.read_mmio(DPC_BUFBUSY_REG).unwrap(),
            expected.dpc_busy
        );
        assert_eq!(
            fabric.read_mmio(DPC_PIPEBUSY_REG).unwrap(),
            expected.dpc_pipe_busy
        );
        assert_eq!(
            fabric.read_mmio(DPC_TMEM_REG).unwrap(),
            expected.dpc_tmem_busy
        );
    }


    #[test]
    fn invalid_complete_rsp_pc_rejects_without_partial_mutation() {
        for pc in [2, 0x1000, 0xffff_ffff] {
            let mut fabric = fabric();
            let before = fabric.rsp_execution_state();
            let before_snapshot = fabric.snapshot();
            let mut state = complete_rsp_state();
            state.pc = pc;

            assert_eq!(
                fabric.commit_complete_rsp_execution_state(state),
                Err(DeviceFault::InvalidRspExecutionPc { pc })
            );
            assert_eq!(fabric.rsp_execution_state(), before);
            assert_eq!(fabric.snapshot(), before_snapshot);
        }
    }


    #[test]
    fn complete_rsp_state_preflight_is_non_mutating() {
        let mut fabric = fabric();
        let before = fabric.snapshot();
        let before_execution = fabric.rsp_execution_state();

        fabric
            .preflight_complete_rsp_execution_state(&complete_rsp_state())
            .unwrap();

        assert_eq!(fabric.snapshot(), before);
        assert_eq!(fabric.rsp_execution_state(), before_execution);

        let pending = fabric
            .request_dpc_submission(DpcSubmissionSource::Rdram, 0x100, 0x180)
            .unwrap()
            .expect("unfrozen DPC submission must publish");
        let pending_snapshot = fabric.snapshot();
        let pending_execution = fabric.rsp_execution_state();
        assert_eq!(
            fabric.preflight_complete_rsp_execution_state(&complete_rsp_state()),
            Err(DeviceFault::DpBusy)
        );
        assert_eq!(fabric.snapshot(), pending_snapshot);
        assert_eq!(fabric.rsp_execution_state(), pending_execution);
        assert_eq!(fabric.pending_dpc_submission(), Some(pending));
    }


    #[test]
    fn complete_rsp_state_cannot_replace_a_pending_dpc_transaction() {
        let mut fabric = fabric();
        let pending = fabric
            .request_dpc_submission(DpcSubmissionSource::Rdram, 0x100, 0x180)
            .unwrap()
            .expect("unfrozen DPC submission must publish");
        let before = fabric.rsp_execution_state();
        let before_snapshot = fabric.snapshot();

        assert_eq!(
            fabric.commit_complete_rsp_execution_state(complete_rsp_state()),
            Err(DeviceFault::DpBusy)
        );
        assert_eq!(fabric.rsp_execution_state(), before);
        assert_eq!(fabric.snapshot(), before_snapshot);
        assert_eq!(fabric.pending_dpc_submission(), Some(pending));
    }


    #[test]
    fn complete_rsp_execution_state_applies_raw_register_address_masks() {
        let mut fabric = fabric();
        let mut state = complete_rsp_state();
        state.sp_dma_dram_addr = RdramAddr::from_offset(u32::MAX);
        state.dpc_start = u32::MAX;
        state.dpc_end = u32::MAX;
        state.dpc_current = u32::MAX;

        fabric.commit_complete_rsp_execution_state(state).unwrap();

        let committed = fabric.rsp_execution_state();
        assert_eq!(committed.sp_dma_dram_addr.offset(), 0x00ff_ffff);
        assert_eq!(committed.dpc_start, DPC_ADDR_MASK);
        assert_eq!(committed.dpc_end, DPC_ADDR_MASK);
        assert_eq!(committed.dpc_current, DPC_ADDR_MASK);
    }


    #[test]
    fn raw_ai_registers_derive_one_fifo_request_from_the_authoritative_tv_clock() {
        let mut fabric = fabric();
        assert_eq!(
            fabric.write_mmio(AI_DACRATE_REG, 151),
            Err(DeviceFault::AiClockUnconfigured),
            "raw AI programming must not guess an NTSC clock before IPL configuration"
        );
        assert_eq!(fabric.ai_dacrate(), 0);

        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        let sample_rate_hz = TvType::Ntsc.vi_clock_hz() / 152;
        assert_eq!(
            fabric.write_mmio(AI_DACRATE_REG, 151).unwrap(),
            DeviceMmioWriteEffect::AiFrequencyChanged { sample_rate_hz }
        );
        assert_eq!(
            fabric.write_mmio(AI_DRAM_ADDR_REG, 0x01ff_123f).unwrap(),
            DeviceMmioWriteEffect::None
        );
        fabric.write_mmio(AI_CONTROL_REG, u32::MAX).unwrap();
        fabric.write_mmio(AI_BITRATE_REG, 0x25).unwrap();
        let request = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x00ff_1238),
            len: 0x80,
            sample_rate_hz,
        };
        assert_eq!(
            fabric.write_mmio(AI_LEN_REG, 0x87).unwrap(),
            DeviceMmioWriteEffect::AiDmaStarted(request)
        );
        assert_eq!(fabric.read_mmio(AI_DRAM_ADDR_REG).unwrap(), 0x00ff_1238);
        assert_eq!(fabric.read_mmio(AI_CONTROL_REG).unwrap(), 1);
        assert_eq!(fabric.read_mmio(AI_DACRATE_REG).unwrap(), 151);
        assert_eq!(fabric.read_mmio(AI_BITRATE_REG).unwrap(), 5);
        assert_eq!(fabric.read_mmio(AI_LEN_REG).unwrap(), 0x80);
        assert_eq!(
            fabric.read_mmio(AI_STATUS_REG).unwrap(),
            AI_STATUS_ENABLED | AI_STATUS_BUSY
        );

        fabric.raise_interrupt(InterruptSource::Ai);
        fabric.write_mmio(AI_STATUS_REG, u32::MAX).unwrap();
        assert!(!fabric.interrupt_pending(InterruptSource::Ai));
        assert_eq!(fabric.pending_dpc_submission(), None);
    }


    #[test]
    fn typed_ai_requests_reject_unrepresentable_register_values_without_mutation() {
        let cases = [
            (
                AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(0x1001),
                    len: 8,
                    sample_rate_hz: 1,
                },
                DeviceFault::InvalidAiDramAddress { address: 0x1001 },
            ),
            (
                AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(0x0100_0000),
                    len: 8,
                    sample_rate_hz: 1,
                },
                DeviceFault::InvalidAiDramAddress {
                    address: 0x0100_0000,
                },
            ),
            (
                AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(0x1000),
                    len: 1,
                    sample_rate_hz: 1,
                },
                DeviceFault::InvalidAiDmaLength { len: 1 },
            ),
            (
                AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(0x1000),
                    len: 0x0004_0000,
                    sample_rate_hz: 1,
                },
                DeviceFault::InvalidAiDmaLength { len: 0x0004_0000 },
            ),
            (
                AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(0x00ff_fff8),
                    len: 16,
                    sample_rate_hz: 1,
                },
                DeviceFault::AiDmaRangeOverflow {
                    address: 0x00ff_fff8,
                    len: 16,
                },
            ),
        ];

        for (request, expected) in cases {
            let mut fabric = fabric();
            let before = fabric.evidence_snapshot();
            assert_eq!(fabric.start_ai_dma(request), Err(expected));
            assert_eq!(fabric.evidence_snapshot(), before);
        }
    }


    #[test]
    fn typed_ai_requests_accept_exact_register_domain_boundaries() {
        for request in [
            AiDmaRequest {
                dram_addr: RdramAddr::from_offset(0x00ff_fff8),
                len: 8,
                sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
            },
            AiDmaRequest {
                dram_addr: RdramAddr::from_offset(0),
                len: AI_LEN_MASK,
                sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
            },
        ] {
            let mut fabric = fabric();
            fabric.configure_tv_type(TvType::Ntsc).unwrap();
            fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
            fabric.start_ai_dma(request).unwrap();
            assert_eq!(fabric.current_ai.unwrap().request, request);
        }
    }


    #[test]
    fn raw_ai_len_write_canonicalizes_before_typed_admission() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        fabric.write_mmio(AI_DRAM_ADDR_REG, 0x1007).unwrap();
        let before = fabric.evidence_snapshot();

        assert_eq!(
            fabric.write_mmio(AI_LEN_REG, 1),
            Err(DeviceFault::ZeroLengthAiDma)
        );
        assert_eq!(fabric.evidence_snapshot(), before);
        assert!(matches!(
            fabric.write_mmio(AI_LEN_REG, 9),
            Ok(DeviceMmioWriteEffect::AiDmaStarted(AiDmaRequest {
                dram_addr,
                len: 8,
                ..
            })) if dram_addr == RdramAddr::from_offset(0x1000)
        ));
    }


    #[test]
    fn ai_exact_rational_deadlines_match_public_region_clocks() {
        for (tv_type, dacrate, expected_rate, expected_deadline) in [
            (TvType::Ntsc, 1_520, 32_006, 93_732),
            (TvType::Pal, 1_551, 31_995, 93_765),
            (TvType::Mpal, 1_519, 31_992, 93_773),
        ] {
            let mut fabric = fabric();
            fabric.configure_tv_type(tv_type).unwrap();
            assert_eq!(
                fabric.write_mmio(AI_DACRATE_REG, dacrate).unwrap(),
                DeviceMmioWriteEffect::AiFrequencyChanged {
                    sample_rate_hz: expected_rate,
                }
            );
            fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
            fabric.write_mmio(AI_DRAM_ADDR_REG, 0x1000).unwrap();
            fabric.write_mmio(AI_LEN_REG, 0x80).unwrap();

            let deadline = fabric.current_ai.unwrap().deadline;
            assert_eq!(deadline, Cycles::new(expected_deadline), "{tv_type:?}");
            let mut rdram = Rdram::new(0);
            fabric
                .advance_to(Cycles::new(expected_deadline - 1), &mut rdram)
                .unwrap();
            assert_ne!(fabric.ai_status() & AI_STATUS_BUSY, 0, "{tv_type:?}");
            assert!(fabric.ai_length() > 0, "{tv_type:?}");
            fabric.advance_to(deadline, &mut rdram).unwrap();
            assert_eq!(fabric.ai_status() & AI_STATUS_BUSY, 0, "{tv_type:?}");
            assert_eq!(fabric.ai_length(), 0, "{tv_type:?}");
            // A completed AI DMA raises AI even with nothing queued behind
            // it. `osAiSetNextBuffer` refuses a submission only when the
            // FIFO is already full, so a guest may keep exactly ONE buffer in
            // flight and submit the next after this one completes; under a
            // FIFO-full-only gate that guest never receives a completion.
            // This assertion previously demanded NO interrupt here, freezing
            // fn64's own gate rather than any documented rule -- and rcp.h
            // says only that a WRITE to `AI_STATUS_REG` CLEARS the audio
            // interrupt (`ultra64/rcp.h:570`), never what raises it.
            assert!(
                fabric.interrupt_pending(InterruptSource::Ai),
                "{tv_type:?}: a completed AI DMA raises AI"
            );
        }
    }


    #[test]
    fn ai_exact_rational_max_length_boundary_does_not_use_truncated_rate() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        fabric.write_mmio(AI_DACRATE_REG, 1_520).unwrap();
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        fabric.write_mmio(AI_DRAM_ADDR_REG, 0).unwrap();
        fabric.write_mmio(AI_LEN_REG, AI_LEN_MASK).unwrap();

        let deadline = fabric.current_ai.unwrap().deadline;
        assert_eq!(deadline, Cycles::new(191_955_444));
        assert_ne!(deadline, Cycles::new(191_958_149));
        let mut rdram = Rdram::new(0);
        fabric
            .advance_to(Cycles::new(deadline.get() - 1), &mut rdram)
            .unwrap();
        assert_ne!(fabric.ai_status() & AI_STATUS_BUSY, 0);
        assert!(fabric.ai_length() > 0);
        fabric.advance_to(deadline, &mut rdram).unwrap();
        assert_eq!(fabric.ai_status() & AI_STATUS_BUSY, 0);
        assert_eq!(fabric.ai_length(), 0);
    }


    #[test]
    fn ai_review_contract_rejects_metadata_and_applies_busy_rate_writes() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        fabric.write_mmio(AI_DACRATE_REG, 1_520).unwrap();
        fabric.write_mmio(AI_BITRATE_REG, 15).unwrap();
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        let request = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 0x80,
            sample_rate_hz: 32_000,
        };
        let before_mismatch = fabric.evidence_snapshot();
        assert_eq!(
            fabric.start_ai_dma(request),
            Err(DeviceFault::AiSampleRateMismatch {
                request: 32_000,
                register: 32_006,
            })
        );
        assert_eq!(fabric.evidence_snapshot(), before_mismatch);

        fabric
            .start_ai_dma(AiDmaRequest {
                sample_rate_hz: 32_006,
                ..request
            })
            .unwrap();
        let before = fabric.evidence_snapshot();
        assert_eq!(
            fabric.write_mmio(AI_DACRATE_REG, 1_520).unwrap(),
            DeviceMmioWriteEffect::None,
            "an idempotent DACRATE rewrite is not a live-FIFO transition"
        );
        assert_eq!(
            fabric.write_mmio(AI_BITRATE_REG, 15).unwrap(),
            DeviceMmioWriteEffect::None,
            "an idempotent BITRATE rewrite is not a live-FIFO transition"
        );
        assert_eq!(fabric.evidence_snapshot(), before);
        fabric
            .advance_to(Cycles::new(10_000), &mut Rdram::new(0))
            .unwrap();
        let old_deadline = fabric.current_ai.unwrap().deadline;
        let old_length = fabric.ai_length();
        assert!(old_length < request.len, "the active DMA must be partly drained");
        let pal_rate_on_ntsc_clock = TvType::Ntsc.vi_clock_hz() / 1_552;
        assert_eq!(
            fabric.write_mmio(AI_DACRATE_REG, 1_551).unwrap(),
            DeviceMmioWriteEffect::AiFrequencyChanged {
                sample_rate_hz: pal_rate_on_ntsc_clock,
            }
        );
        assert_eq!(fabric.ai_length(), old_length);
        assert!(fabric.current_ai.unwrap().deadline > old_deadline);
        assert_eq!(fabric.ai_dacrate(), 1_551);
        assert_eq!(
            fabric.write_mmio(AI_BITRATE_REG, 7).unwrap(),
            DeviceMmioWriteEffect::None
        );
        assert_eq!(fabric.ai_bitrate(), 7);
    }


    #[test]
    fn ai_deadline_failures_preserve_active_and_dormant_fifo_state() {
        let mut active = fabric();
        active.configure_tv_type(TvType::Ntsc).unwrap();
        active.events.clear();
        active.write_mmio(AI_CONTROL_REG, 1).unwrap();
        active.now = Cycles::new(u64::MAX - 6);
        let request = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 8,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };
        let before_start = active.evidence_snapshot();
        assert_eq!(
            active.start_ai_dma(AiDmaRequest {
                len: 0x80,
                ..request
            }),
            Err(DeviceFault::DeadlineOverflow)
        );
        assert_eq!(active.evidence_snapshot(), before_start);

        let mut dormant = fabric();
        dormant.configure_tv_type(TvType::Ntsc).unwrap();
        dormant.events.clear();
        dormant.now = Cycles::new(u64::MAX);
        dormant.start_ai_dma(request).unwrap();
        let before_enable = dormant.evidence_snapshot();
        assert_eq!(
            dormant.write_mmio(AI_CONTROL_REG, 1),
            Err(DeviceFault::DeadlineOverflow)
        );
        assert_eq!(dormant.evidence_snapshot(), before_enable);
    }


    #[test]
    fn ai_promotion_preflights_before_event_mutation() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        let request = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 8,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };
        fabric.start_ai_dma(request).unwrap();
        fabric
            .start_ai_dma(AiDmaRequest {
                dram_addr: RdramAddr::from_offset(0x2000),
                ..request
            })
            .unwrap();
        let deadline = fabric.current_ai.unwrap().deadline;
        fabric.next_event_sequence = u64::MAX;
        let before = fabric.evidence_snapshot();
        let mut rdram = Rdram::new(0);
        assert_eq!(
            fabric.advance_to(deadline, &mut rdram),
            Err(DeviceFault::DeadlineOverflow)
        );
        assert_eq!(fabric.evidence_snapshot(), before);
    }


    /// A 32-byte triangle arriving in two END writes must PARK, not panic.
    ///
    /// This is the defect the stall machinery exists for: the DPC accepts END
    /// extensions in 8-byte increments, so hardware stalls CURRENT at the
    /// command's start rather than decoding a truncated stream.
    #[test]
    fn raw_dpc_incomplete_command_parks_then_resumes_on_the_next_end() {
        const A: u32 = 0x100;
        let mut fabric = fabric();
        fabric.write_mmio(DPC_START_REG, A).unwrap();

        let first = match fabric.write_mmio(DPC_END_REG, A + 8).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested {
                submission,
                retained_tail,
            } => {
                assert!(retained_tail.is_empty(), "a new stream carries no tail");
                submission
            }
            other => panic!("first END did not request inspection: {other:?}"),
        };

        // The ABI scanned an 8-byte prefix of opcode 0x08 (a 32-byte base
        // triangle) and parks it rather than dispatching.
        fabric
            .park_dpc_submission(first.token, A, A + 8, 32, vec![0x0800_0000, 0])
            .unwrap();

        // Parked, not in flight: CURRENT names the stalled command, the DP is
        // still architecturally busy, and no transaction is pending.
        assert_eq!(fabric.pending_dpc_submission(), None);
        assert_eq!(fabric.read_mmio(DPC_CURRENT_REG).unwrap(), A);
        assert!(fabric.snapshot().dp_busy, "a parked tail is DP-busy");
        assert!(fabric.stalled_dpc().is_some());

        // The extending END is ACCEPTED (not DpBusy) and carries the tail back.
        let second = match fabric.write_mmio(DPC_END_REG, A + 32).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested {
                submission,
                retained_tail,
            } => {
                assert_eq!(
                    retained_tail,
                    vec![0x0800_0000, 0],
                    "the continuation must receive the captured tail, not reread memory"
                );
                submission
            }
            other => panic!("extending END did not request dispatch: {other:?}"),
        };
        assert_eq!(
            (second.start, second.end),
            (A, A + 32),
            "the resumed range starts at the stalled command, not at the new bytes"
        );

        fabric.commit_dpc_submission(second.token).unwrap();
        assert_eq!(fabric.read_mmio(DPC_CURRENT_REG).unwrap(), A + 32);
        assert!(fabric.stalled_dpc().is_none(), "commit consumes the tail");
        assert!(!fabric.snapshot().dp_busy);
    }

    /// START opens a new stream, so a parked tail from the old one is dropped.
    #[test]
    fn accepted_start_discards_a_stalled_dpc_tail() {
        const A: u32 = 0x180;
        let mut fabric = fabric();
        fabric.write_mmio(DPC_START_REG, A).unwrap();
        let partial = match fabric.write_mmio(DPC_END_REG, A + 8).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested { submission, .. } => submission,
            other => panic!("partial command did not request inspection: {other:?}"),
        };
        fabric
            .park_dpc_submission(partial.token, A, A + 8, 32, vec![0x0800_0000, 0])
            .unwrap();
        assert!(fabric.stalled_dpc().is_some());

        fabric.write_mmio(DPC_START_REG, 0x300).unwrap();

        assert!(
            fabric.stalled_dpc().is_none(),
            "a new START must not leave bytes that would splice onto the next stream"
        );
    }

    /// A non-advancing END is a stream boundary, not a continuation.
    ///
    /// This is the XBUS ring-wrap shape: `rsp_commit.rs:105-115` records that
    /// concatenating across a wrap was MEASURED wrong, so it must be refused
    /// rather than silently bridged.
    #[test]
    fn a_regressing_end_does_not_continue_a_stalled_command() {
        const A: u32 = 0x200;
        let mut fabric = fabric();
        fabric.write_mmio(DPC_START_REG, A).unwrap();
        let partial = match fabric.write_mmio(DPC_END_REG, A + 8).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested { submission, .. } => submission,
            other => panic!("partial command did not request inspection: {other:?}"),
        };
        fabric
            .park_dpc_submission(partial.token, A, A + 8, 32, vec![0x0800_0000, 0])
            .unwrap();

        assert!(
            matches!(
                fabric.write_mmio(DPC_END_REG, 0x20),
                Err(DeviceFault::InvalidStalledDpcContinuation { .. })
            ),
            "an END that moves backwards must be refused, never concatenated"
        );
        assert!(
            fabric.stalled_dpc().is_some(),
            "a refused continuation leaves the tail intact"
        );
    }

    /// Parking consumes the admitted token exactly once, like commit/cancel.
    #[test]
    fn parking_consumes_the_token_and_rejects_a_stale_one() {
        const A: u32 = 0x280;
        let mut fabric = fabric();
        fabric.write_mmio(DPC_START_REG, A).unwrap();
        let partial = match fabric.write_mmio(DPC_END_REG, A + 8).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested { submission, .. } => submission,
            other => panic!("partial command did not request inspection: {other:?}"),
        };
        fabric
            .park_dpc_submission(partial.token, A, A + 8, 32, vec![0x0800_0000, 0])
            .unwrap();

        // The token is spent: a replay finds no pending transaction.
        assert_eq!(
            fabric.park_dpc_submission(partial.token, A, A + 8, 32, vec![0x0800_0000, 0]),
            Err(DeviceFault::NoPendingDpcSubmission)
        );
        assert_eq!(
            fabric.commit_dpc_submission(partial.token),
            Err(DeviceFault::NoPendingDpcSubmission)
        );
    }

    #[test]
    fn raw_dpc_end_is_transactional_and_does_not_replay_after_commit() {
        let mut fabric = fabric();
        fabric.write_mmio(DPC_START_REG, 0x103).unwrap();
        let before_end = fabric.snapshot();
        let first = match fabric.write_mmio(DPC_END_REG, 0x147).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested { submission: submission, .. } => submission,
            other => panic!("DPC END did not request renderer work: {other:?}"),
        };
        assert_eq!(first.source, DpcSubmissionSource::Rdram);
        assert_eq!((first.start, first.end), (0x100, 0x140));
        assert_eq!(fabric.read_mmio(DPC_CURRENT_REG).unwrap(), 0x100);
        assert_eq!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap()
                & (DPC_STATUS_DMA_BUSY | DPC_STATUS_CMD_BUSY | DPC_STATUS_END_VALID),
            DPC_STATUS_DMA_BUSY | DPC_STATUS_CMD_BUSY | DPC_STATUS_END_VALID
        );
        assert_eq!(
            fabric.write_mmio(DPC_END_REG, 0x140),
            Err(DeviceFault::DpBusy)
        );
        assert!(matches!(
            fabric.commit_dpc_submission(first.token + 1),
            Err(DeviceFault::StaleDpcSubmission { .. })
        ));

        fabric.cancel_dpc_submission(first.token).unwrap();
        let cancelled = fabric.snapshot();
        assert_eq!(cancelled.dpc_start, before_end.dpc_start);
        assert_eq!(cancelled.dpc_end, before_end.dpc_end);
        assert_eq!(cancelled.dpc_current, before_end.dpc_current);
        assert_eq!(cancelled.dpc_status, before_end.dpc_status);

        let retry = match fabric.write_mmio(DPC_END_REG, 0x140).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested { submission: submission, .. } => submission,
            other => panic!("cancelled DPC END was not retryable: {other:?}"),
        };
        fabric.commit_dpc_submission(retry.token).unwrap();
        assert_eq!(fabric.read_mmio(DPC_CURRENT_REG).unwrap(), 0x140);
        assert_eq!(fabric.pending_dpc_submission(), None);

        assert_eq!(
            fabric.write_mmio(DPC_END_REG, 0x140).unwrap(),
            DeviceMmioWriteEffect::None,
            "repeating the committed END pointer must not replay the range"
        );
        let extension = match fabric.write_mmio(DPC_END_REG, 0x180).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested { submission: submission, .. } => submission,
            other => panic!("DPC END extension did not request renderer work: {other:?}"),
        };
        assert_eq!((extension.start, extension.end), (0x140, 0x180));
    }

    #[test]
    fn reserved_dpc_batch_allocates_exact_tokens_without_register_mutation_and_activates_in_order() {
        let mut fabric = fabric();
        let before = fabric.snapshot();
        let mut batch = fabric
            .reserve_dpc_submission_batch_with_temporal_spans(&[
                (DpcSubmissionSource::Dmem, 0x20, 0x40, 3),
                (DpcSubmissionSource::Dmem, 0x80, 0xa0, 1),
            ])
            .unwrap();
        assert_eq!(batch.remaining(), 2);
        assert_eq!(
            batch.submissions()[1].token,
            batch.submissions()[0].token + 3,
            "the second DPC token must sort after the first member's two reserved boundaries"
        );
        let reserved = fabric.snapshot();
        assert_eq!(reserved.dpc_start, before.dpc_start);
        assert_eq!(reserved.dpc_end, before.dpc_end);
        assert_eq!(reserved.dpc_current, before.dpc_current);
        assert_eq!(reserved.dpc_status, before.dpc_status);
        assert_eq!(fabric.pending_dpc_submission(), None);

        let first = fabric
            .activate_reserved_dpc_submission(&mut batch)
            .unwrap()
            .unwrap();
        assert_eq!(first, batch.submissions()[0]);
        assert_eq!(batch.remaining(), 1);
        fabric.commit_dpc_submission(first.token).unwrap();
        assert_eq!(fabric.read_mmio(DPC_CURRENT_REG).unwrap(), 0x40);

        let second = fabric
            .activate_reserved_dpc_submission(&mut batch)
            .unwrap()
            .unwrap();
        assert_eq!(second, batch.submissions()[1]);
        fabric.commit_dpc_submission(second.token).unwrap();
        assert_eq!(batch.remaining(), 0);
        assert_eq!(fabric.read_mmio(DPC_CURRENT_REG).unwrap(), 0xa0);
    }

    #[test]
    fn empty_dpc_start_end_pair_sets_the_extension_origin() {
        let mut fabric = fabric();
        fabric.write_mmio(DPC_START_REG, 0x100).unwrap();

        assert_eq!(
            fabric.write_mmio(DPC_END_REG, 0x100).unwrap(),
            DeviceMmioWriteEffect::None
        );
        assert_eq!(fabric.read_mmio(DPC_CURRENT_REG).unwrap(), 0x100);
        assert_eq!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap() & DPC_STATUS_START_VALID,
            0
        );

        let extension = match fabric.write_mmio(DPC_END_REG, 0x108).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested { submission: submission, .. } => submission,
            other => panic!("DPC END extension did not request renderer work: {other:?}"),
        };
        assert_eq!((extension.start, extension.end), (0x100, 0x108));
    }


    #[test]
    fn dpc_freeze_defers_end_until_clear_and_preserves_flush_latch() {
        let mut fabric = fabric();
        fabric
            .write_mmio(DPC_STATUS_REG, 0x02 | 0x08 | 0x20)
            .unwrap();
        assert_eq!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap(),
            DPC_STATUS_XBUS_DMEM_DMA | DPC_STATUS_FREEZE | DPC_STATUS_FLUSH
        );
        fabric.write_mmio(DPC_START_REG, 0x20).unwrap();
        assert_eq!(
            fabric.write_mmio(DPC_END_REG, 0x40).unwrap(),
            DeviceMmioWriteEffect::None,
            "FREEZE must latch END without exposing renderer work"
        );
        assert_eq!(fabric.pending_dpc_submission(), None);
        assert_eq!(fabric.read_mmio(DPC_CURRENT_REG).unwrap(), 0x20);
        assert_eq!(fabric.read_mmio(DPC_END_REG).unwrap(), 0x40);
        assert_eq!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap() & DPC_STATUS_FLUSH,
            DPC_STATUS_FLUSH,
            "FLUSH remains an independently controlled mode latch"
        );

        let submission = match fabric.write_mmio(DPC_STATUS_REG, 0x04).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested { submission: submission, .. } => submission,
            other => panic!("clearing FREEZE did not release renderer work: {other:?}"),
        };
        assert_eq!(submission.source, DpcSubmissionSource::Dmem);
        assert_ne!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap() & DPC_STATUS_DMA_BUSY,
            0
        );
        fabric.write_mmio(DPC_STATUS_REG, 0x01 | 0x10).unwrap();
        assert_eq!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap()
                & (DPC_STATUS_XBUS_DMEM_DMA | DPC_STATUS_FREEZE | DPC_STATUS_FLUSH),
            0
        );
        assert_ne!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap()
                & (DPC_STATUS_DMA_BUSY | DPC_STATUS_END_VALID),
            0,
            "status mode commands cannot consume the renderer transaction"
        );
    }


    #[test]
    fn dpc_counter24_boundary_is_shared_by_rsp_mmio_and_snapshot() {
        let mut fabric = fabric();
        let mut state = complete_rsp_state();
        state.dpc_clock = DPC_COUNTER_MASK;
        state.dpc_busy = DPC_COUNTER_MASK;
        state.dpc_pipe_busy = DPC_COUNTER_MASK;
        state.dpc_tmem_busy = DPC_COUNTER_MASK;
        fabric.commit_complete_rsp_execution_state(state).unwrap();

        let expected = (
            DPC_COUNTER_MASK,
            DPC_COUNTER_MASK,
            DPC_COUNTER_MASK,
            DPC_COUNTER_MASK,
        );
        let rsp = fabric.rsp_execution_state();
        assert_eq!(
            (
                rsp.dpc_clock,
                rsp.dpc_busy,
                rsp.dpc_pipe_busy,
                rsp.dpc_tmem_busy,
            ),
            expected
        );
        assert_eq!(
            (
                fabric.read_mmio(DPC_CLOCK_REG).unwrap(),
                fabric.read_mmio(DPC_BUFBUSY_REG).unwrap(),
                fabric.read_mmio(DPC_PIPEBUSY_REG).unwrap(),
                fabric.read_mmio(DPC_TMEM_REG).unwrap(),
            ),
            expected
        );
        let snapshot = fabric.snapshot();
        assert_eq!(
            (
                snapshot.dpc_clock,
                snapshot.dpc_busy,
                snapshot.dpc_pipe_busy,
                snapshot.dpc_tmem_busy,
            ),
            expected
        );

        state.dpc_clock = 0x0100_0000;
        state.dpc_busy = 0x0100_0001;
        state.dpc_pipe_busy = 0x01ff_ffff;
        state.dpc_tmem_busy = u32::MAX;
        fabric.commit_complete_rsp_execution_state(state).unwrap();
        let canonical = (0, 1, DPC_COUNTER_MASK, DPC_COUNTER_MASK);
        let snapshot = fabric.snapshot();
        assert_eq!(
            (
                snapshot.dpc_clock,
                snapshot.dpc_busy,
                snapshot.dpc_pipe_busy,
                snapshot.dpc_tmem_busy,
            ),
            canonical,
            "synchronous RSP imports cannot place high bits outside the public counter domain"
        );
        assert_eq!(
            (
                fabric.read_mmio(DPC_CLOCK_REG).unwrap(),
                fabric.read_mmio(DPC_BUFBUSY_REG).unwrap(),
                fabric.read_mmio(DPC_PIPEBUSY_REG).unwrap(),
                fabric.read_mmio(DPC_TMEM_REG).unwrap(),
            ),
            canonical
        );
    }


    #[test]
    fn dpc_source_domains_reject_wrapped_or_out_of_range_command_bytes() {
        let mut fabric = fabric();
        for (source, start, end) in [
            (DpcSubmissionSource::Dmem, 0x0ff8, 0x1008),
            (DpcSubmissionSource::Dmem, 0x0800, 0x0400),
            (DpcSubmissionSource::Rdram, 0x00ff_fff8, 0x0100_0008),
            (DpcSubmissionSource::Rdram, 0x0100, 0x0104),
        ] {
            assert_eq!(
                fabric.request_dpc_submission(source, start, end),
                Err(DeviceFault::InvalidDpcRange { source, start, end })
            );
            assert_eq!(fabric.pending_dpc_submission(), None);
        }
    }


    #[test]
    fn snapshots_distinguish_future_affecting_ai_and_dpc_latches() {
        let mut baseline = fabric();
        let mut ai_changed = fabric();
        let mut dpc_changed = fabric();
        for fabric in [&mut baseline, &mut ai_changed, &mut dpc_changed] {
            fabric.configure_tv_type(TvType::Ntsc).unwrap();
        }
        ai_changed.write_mmio(AI_CONTROL_REG, 1).unwrap();
        dpc_changed.write_mmio(DPC_START_REG, 0x80).unwrap();

        assert_ne!(baseline.snapshot(), ai_changed.snapshot());
        assert_ne!(baseline.snapshot(), dpc_changed.snapshot());
        assert_ne!(baseline.evidence_snapshot(), ai_changed.evidence_snapshot());
        assert_ne!(
            baseline.evidence_snapshot(),
            dpc_changed.evidence_snapshot()
        );
    }


    #[test]
    fn raw_and_shim_pi_starts_share_one_timed_state_machine() {
        let request = PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x20),
            device: PiDeviceAddress::RomOffset(0x10),
            len: 4,
        };
        let mut shim = fabric();
        let mut raw = fabric();
        let mut shim_rdram = Rdram::new(0x100);
        let mut raw_rdram = Rdram::new(0x100);

        shim.start_pi_dma(request).unwrap();
        raw.write_mmio(PI_DRAM_ADDR_REG, 0x20).unwrap();
        raw.write_mmio(PI_CART_ADDR_REG, 0x1000_0010).unwrap();
        raw.write_mmio(PI_WR_LEN_REG, 3).unwrap();
        assert_eq!(raw.snapshot(), shim.snapshot());
        assert_eq!(raw.read_mmio(PI_STATUS_REG).unwrap(), PI_STATUS_DMA_BUSY);

        assert!(raw
            .advance_to(Cycles::new(11), &mut raw_rdram)
            .unwrap()
            .is_empty());
        assert!(shim
            .advance_to(Cycles::new(11), &mut shim_rdram)
            .unwrap()
            .is_empty());
        assert_eq!(raw_rdram.read_w(RdramAddr::from_offset(0x20)), 0);
        assert_eq!(raw.snapshot(), shim.snapshot());

        let raw_notifications = raw.advance_to(Cycles::new(12), &mut raw_rdram).unwrap();
        let shim_notifications = shim.advance_to(Cycles::new(12), &mut shim_rdram).unwrap();
        assert_eq!(raw_notifications, shim_notifications);
        assert_eq!(raw.snapshot(), shim.snapshot());
        assert_eq!(raw.trace(), shim.trace());
        assert_eq!(
            raw_rdram.read_w(RdramAddr::from_offset(0x20)) as u32,
            0xDEAD_BEEF
        );
        assert_eq!(
            raw_rdram.read_w(RdramAddr::from_offset(0x20)),
            shim_rdram.read_w(RdramAddr::from_offset(0x20))
        );
        assert_eq!(raw.read_mmio(PI_STATUS_REG).unwrap(), 0);
        assert_eq!(
            raw.read_mmio(MI_INTR_REG).unwrap(),
            InterruptSource::Pi.bit()
        );
        assert!(raw.interrupt_pending(InterruptSource::Pi));

        let kinds = raw
            .trace()
            .iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                DeviceTraceKind::PiDmaStarted(request),
                DeviceTraceKind::PiBytesCommitted(request),
                DeviceTraceKind::PiBusyCleared,
                DeviceTraceKind::MiInterruptRaised(InterruptSource::Pi),
                DeviceTraceKind::NotificationReady(raw_notifications[0]),
            ]
        );
        assert_eq!(raw.trace()[0].at, Cycles::ZERO);
        assert!(raw.trace()[1..]
            .iter()
            .all(|event| event.at == Cycles::new(12)));
        assert_eq!(
            raw.trace()
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );

        raw.set_interrupt_mask(InterruptSource::Pi, true);
        assert!(raw.cpu_interrupt_pending());
        raw.write_mmio(PI_STATUS_REG, 0b10).unwrap();
        assert!(!raw.interrupt_pending(InterruptSource::Pi));
        assert!(!raw.cpu_interrupt_pending());
    }


    #[test]
    fn raw_pi_length_registers_decode_direction_and_device_at_trigger() {
        let mut read = fabric();
        read.write_mmio(PI_DRAM_ADDR_REG, 0x20).unwrap();
        read.write_mmio(PI_CART_ADDR_REG, 0x0800_0010).unwrap();
        assert_eq!(read.read_mmio(PI_CART_ADDR_REG).unwrap(), 0x0800_0010);
        read.write_mmio(PI_RD_LEN_REG, 3).unwrap();
        assert_eq!(
            read.pending_pi_request(),
            Some(PiDmaRequest {
                direction: DmaDirection::FromRdram,
                dram_addr: RdramAddr::from_offset(0x20),
                device: PiDeviceAddress::SramOffset(0x10),
                len: 4,
            })
        );
        assert_eq!(read.read_mmio(PI_CART_ADDR_REG).unwrap(), 0x0800_0010);

        let mut write = fabric();
        write.write_mmio(PI_DRAM_ADDR_REG, 0x24).unwrap();
        write.write_mmio(PI_CART_ADDR_REG, 0x1000_0010).unwrap();
        write.write_mmio(PI_WR_LEN_REG, 3).unwrap();
        assert_eq!(
            write.pending_pi_request(),
            Some(PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0x24),
                device: PiDeviceAddress::RomOffset(0x10),
                len: 4,
            })
        );
        assert_eq!(write.read_mmio(PI_CART_ADDR_REG).unwrap(), 0x1000_0010);
    }


    #[test]
    fn raw_pi_rd_len_rejects_a_write_to_cartridge_rom_loudly() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        fabric.write_mmio(PI_DRAM_ADDR_REG, 0x20).unwrap();
        fabric.write_mmio(PI_CART_ADDR_REG, 0x1000_0010).unwrap();
        fabric.write_mmio(PI_RD_LEN_REG, 3).unwrap();

        assert_eq!(
            fabric.advance_to(Cycles::new(12), &mut rdram),
            Err(DeviceFault::PiTransfer(PiDmaError::ReadOnlyDevice {
                device: PiDeviceAddress::RomOffset(0x10),
            }))
        );
    }


    #[test]
    fn raw_pi_sram_round_trips_in_both_register_directions() {
        let mut fabric = fabric();
        fabric
            .pi_dma_mut()
            .set_save(Box::new(crate::save::InMemorySaveStorage::for_device(
                crate::save::SaveType::SramBanked,
            )));
        let mut rdram = Rdram::new(0x100);
        rdram.write_w(RdramAddr::from_offset(0x20), 0x1122_3344);

        fabric.write_mmio(PI_DRAM_ADDR_REG, 0x20).unwrap();
        fabric.write_mmio(PI_CART_ADDR_REG, 0x0800_0010).unwrap();
        fabric.write_mmio(PI_RD_LEN_REG, 3).unwrap();
        fabric.advance_to(Cycles::new(12), &mut rdram).unwrap();

        fabric.write_mmio(PI_DRAM_ADDR_REG, 0x40).unwrap();
        fabric.write_mmio(PI_CART_ADDR_REG, 0x0800_0010).unwrap();
        fabric.write_mmio(PI_WR_LEN_REG, 3).unwrap();
        fabric.advance_to(Cycles::new(24), &mut rdram).unwrap();

        assert_eq!(rdram.read_w(RdramAddr::from_offset(0x40)), 0x1122_3344);
    }


    #[test]
    fn typed_and_raw_sram_reads_share_state_trace_and_bytes() {
        let request = PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x20),
            device: PiDeviceAddress::SramOffset(0x10),
            len: 4,
        };
        let mut typed = fabric();
        let mut raw = fabric();
        for fabric in [&mut typed, &mut raw] {
            fabric
                .pi_dma_mut()
                .set_save(Box::new(crate::save::InMemorySaveStorage::for_device(
                    crate::save::SaveType::SramBanked,
                )));
            fabric
                .pi_dma_mut()
                .save_write_from(0x10, &[0x55, 0x66, 0x77, 0x88]);
        }
        let mut typed_rdram = Rdram::new(0x100);
        let mut raw_rdram = Rdram::new(0x100);

        typed.start_pi_dma(request).unwrap();
        raw.write_mmio(PI_DRAM_ADDR_REG, 0x20).unwrap();
        raw.write_mmio(PI_CART_ADDR_REG, 0x0800_0010).unwrap();
        raw.write_mmio(PI_WR_LEN_REG, 3).unwrap();
        assert_eq!(raw.snapshot(), typed.snapshot());

        let typed_notifications = typed.advance_to(Cycles::new(12), &mut typed_rdram).unwrap();
        let raw_notifications = raw.advance_to(Cycles::new(12), &mut raw_rdram).unwrap();
        assert_eq!(raw_notifications, typed_notifications);
        assert_eq!(raw.snapshot(), typed.snapshot());
        assert_eq!(raw.trace(), typed.trace());
        assert_eq!(
            raw_rdram.read_w(RdramAddr::from_offset(0x20)),
            typed_rdram.read_w(RdramAddr::from_offset(0x20))
        );
    }


    #[test]
    fn raw_pi_device_windows_decode_boundaries_and_reject_gaps() {
        for (physical, device) in [
            (0x0800_0000, PiDeviceAddress::SramOffset(0)),
            (0x0fff_ffff, PiDeviceAddress::SramOffset(0x07ff_ffff)),
            (0x1000_0000, PiDeviceAddress::RomOffset(0)),
            (0x1fbf_ffff, PiDeviceAddress::RomOffset(0x0fbf_ffff)),
        ] {
            let mut fabric = fabric();
            fabric.write_mmio(PI_CART_ADDR_REG, physical).unwrap();
            fabric.write_mmio(PI_WR_LEN_REG, 0).unwrap();
            assert_eq!(fabric.pending_pi_request().unwrap().device, device);
            assert_eq!(fabric.read_mmio(PI_CART_ADDR_REG).unwrap(), physical);
        }

        for physical in [0x07ff_ffff, 0x1fc0_0000] {
            let mut fabric = fabric();
            fabric.write_mmio(PI_CART_ADDR_REG, physical).unwrap();
            assert_eq!(
                fabric.write_mmio(PI_WR_LEN_REG, 0),
                Err(DeviceFault::InvalidPiCartAddress { physical })
            );
            assert_eq!(fabric.pending_pi_request(), None);
            assert_eq!(fabric.read_mmio(PI_CART_ADDR_REG).unwrap(), physical);
        }

        for (physical, device) in [
            (0x0fff_ffff, PiDeviceAddress::SramOffset(0x07ff_ffff)),
            (0x1fbf_ffff, PiDeviceAddress::RomOffset(0x0fbf_ffff)),
        ] {
            let mut fabric = fabric();
            fabric.write_mmio(PI_CART_ADDR_REG, physical).unwrap();
            assert_eq!(
                fabric.write_mmio(PI_WR_LEN_REG, 1),
                Err(DeviceFault::InvalidPiDeviceRange { device, len: 2 })
            );
            assert_eq!(fabric.pending_pi_request(), None);
            assert_eq!(fabric.read_mmio(PI_CART_ADDR_REG).unwrap(), physical);
        }
    }


    #[test]
    fn rom_offset_above_eight_mib_is_not_inferred_as_sram() {
        const ROM_OFFSET: u32 = 0x0080_0010;
        let mut rom = vec![0u8; ROM_OFFSET as usize + 4];
        rom[ROM_OFFSET as usize..ROM_OFFSET as usize + 4]
            .copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        let mut fabric = DeviceFabric::new(
            PiDma::new(InMemoryRom::new(rom)),
            TestTiming(Cycles::new(12)),
        );
        let mut rdram = Rdram::new(0x100);

        fabric.write_mmio(PI_DRAM_ADDR_REG, 0x20).unwrap();
        fabric
            .write_mmio(PI_CART_ADDR_REG, 0x1000_0000 + ROM_OFFSET)
            .unwrap();
        fabric.write_mmio(PI_WR_LEN_REG, 3).unwrap();
        assert_eq!(
            fabric.pending_pi_request().unwrap().device,
            PiDeviceAddress::RomOffset(ROM_OFFSET)
        );
        fabric.advance_to(Cycles::new(12), &mut rdram).unwrap();
        assert_eq!(
            rdram.read_w(RdramAddr::from_offset(0x20)) as u32,
            0x1234_5678
        );
    }


    #[test]
    fn synthetic_banjo_pi_tuple_normalizes_without_game_content() {
        let mut fabric = fabric();
        fabric.write_mmio(PI_DRAM_ADDR_REG, 0x0002_d500).unwrap();
        fabric.write_mmio(PI_CART_ADDR_REG, 0x10f1_9250).unwrap();
        fabric.write_mmio(PI_WR_LEN_REG, 0x0001_ed3f).unwrap();

        assert_eq!(
            fabric.pending_pi_request(),
            Some(PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0x0002_d500),
                device: PiDeviceAddress::RomOffset(0x00f1_9250),
                len: 0x0001_ed40,
            })
        );
        assert_eq!(
            0x0002_d500u32 + fabric.pending_pi_request().unwrap().len,
            0x0004_c240
        );
        assert_eq!(
            0x00f1_9250u32 + fabric.pending_pi_request().unwrap().len,
            0x00f3_7f90
        );
        assert_eq!(fabric.read_mmio(PI_CART_ADDR_REG).unwrap(), 0x10f1_9250);
    }


    #[test]
    fn typed_pi_offsets_must_fit_their_physical_windows() {
        for (device, len) in [
            (PiDeviceAddress::RomOffset(0x0fc0_0000), 1),
            (PiDeviceAddress::SramOffset(0x0800_0000), 1),
            (PiDeviceAddress::RomOffset(0x0fbf_ffff), 2),
            (PiDeviceAddress::SramOffset(0x07ff_ffff), 2),
            (PiDeviceAddress::RomOffset(0x0fbf_ffff), u32::MAX),
        ] {
            let mut fabric = fabric();
            assert_eq!(
                fabric.start_pi_dma(PiDmaRequest {
                    direction: DmaDirection::ToRdram,
                    dram_addr: RdramAddr::from_offset(0),
                    device,
                    len,
                }),
                Err(DeviceFault::InvalidPiDeviceRange { device, len })
            );
            assert_eq!(fabric.pending_pi_request(), None);
        }
    }


    #[test]
    fn typed_pi_start_failures_do_not_mutate_readable_latches_or_trace() {
        let request = |device, len| PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x20),
            device,
            len,
        };

        let mut zero = fabric();
        zero.write_mmio(PI_CART_ADDR_REG, 0x0800_0040).unwrap();
        let before = zero.snapshot();
        assert_eq!(
            zero.start_pi_dma(request(PiDeviceAddress::RomOffset(0x10), 0)),
            Err(DeviceFault::ZeroLengthPiDma)
        );
        assert_eq!(zero.snapshot(), before);
        assert!(zero.trace().is_empty());

        let mut busy = fabric();
        busy.start_pi_dma(request(PiDeviceAddress::RomOffset(0x10), 4))
            .unwrap();
        let before = busy.snapshot();
        let trace = busy.trace().to_vec();
        assert_eq!(
            busy.start_pi_dma(request(PiDeviceAddress::SramOffset(0x20), 4)),
            Err(DeviceFault::PiBusy)
        );
        assert_eq!(busy.snapshot(), before);
        assert_eq!(busy.trace(), trace);

        let mut deadline = fabric();
        deadline.write_mmio(PI_CART_ADDR_REG, 0x0800_0040).unwrap();
        deadline
            .advance_to(Cycles::new(u64::MAX), &mut Rdram::new(0x100))
            .unwrap();
        let before = deadline.snapshot();
        assert_eq!(
            deadline.start_pi_dma(request(PiDeviceAddress::RomOffset(0x10), 4)),
            Err(DeviceFault::DeadlineOverflow)
        );
        assert_eq!(deadline.snapshot(), before);
        assert!(deadline.trace().is_empty());
    }
