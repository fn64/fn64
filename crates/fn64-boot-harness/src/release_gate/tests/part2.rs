use super::*;

#[test]
fn device_evidence_wire_binds_every_future_state_family() {
    use fn64_runtime::{
        DpcSubmission, DpcSubmissionSource, PendingAiSnapshot, PendingDpcSnapshot,
        PendingEepromWriteSnapshot, PendingPiSnapshot, PendingSiSnapshot, PendingSpDmaSnapshot,
        ScheduledDeviceEventKind, ScheduledDeviceEventSnapshot, SpDmaRequest,
    };

    let mut baseline = snapshot(42);
    let dpc_submission = DpcSubmission {
        token: 7,
        source: DpcSubmissionSource::Rdram,
        start: 0x100,
        end: 0x180,
    };
    baseline.guest.pending_dpc = Some(dpc_submission);
    baseline.pending_dpc = Some(PendingDpcSnapshot {
        submission: dpc_submission,
        rollback_start: 0x80,
        rollback_end: 0x100,
        rollback_current: 0x80,
        rollback_status: 0x400,
    });
    let baseline_sha = sha256_hex(&encode_test_device(
        baseline.clone(),
        peripherals_snapshot(),
    ));
    let mut cases = Vec::new();
    macro_rules! changed {
        ($name:literal, $body:expr) => {{
            let mut value = baseline.clone();
            $body(&mut value);
            cases.push(($name, value));
        }};
    }

    changed!(
        "guest register projection",
        |value: &mut DeviceEvidenceSnapshot| { value.guest.pi_cart_addr ^= 1 }
    );
    changed!(
        "AI DRAM address latch",
        |value: &mut DeviceEvidenceSnapshot| {
            value.guest.ai_dram_addr =
                RdramAddr::from_offset(value.guest.ai_dram_addr.offset() ^ 8)
        }
    );
    changed!("AI control latch", |value: &mut DeviceEvidenceSnapshot| {
        value.guest.ai_control ^= 1
    });
    changed!("AI DACRATE latch", |value: &mut DeviceEvidenceSnapshot| {
        value.guest.ai_dacrate ^= 1
    });
    changed!("AI BITRATE latch", |value: &mut DeviceEvidenceSnapshot| {
        value.guest.ai_bitrate ^= 1
    });
    changed!(
        "DPC START register",
        |value: &mut DeviceEvidenceSnapshot| { value.guest.dpc_start ^= 8 }
    );
    changed!("DPC END register", |value: &mut DeviceEvidenceSnapshot| {
        value.guest.dpc_end ^= 8
    });
    changed!(
        "DPC CURRENT register",
        |value: &mut DeviceEvidenceSnapshot| { value.guest.dpc_current ^= 8 }
    );
    changed!(
        "DPC STATUS register",
        |value: &mut DeviceEvidenceSnapshot| { value.guest.dpc_status ^= 1 }
    );
    changed!(
        "DPC CLOCK register",
        |value: &mut DeviceEvidenceSnapshot| { value.guest.dpc_clock ^= 1 }
    );
    changed!(
        "DPC BUFBUSY register",
        |value: &mut DeviceEvidenceSnapshot| { value.guest.dpc_busy ^= 1 }
    );
    changed!(
        "DPC PIPEBUSY register",
        |value: &mut DeviceEvidenceSnapshot| { value.guest.dpc_pipe_busy ^= 1 }
    );
    changed!("DPC TMEM register", |value: &mut DeviceEvidenceSnapshot| {
        value.guest.dpc_tmem_busy ^= 1
    });
    changed!(
        "guest pending DPC token",
        |value: &mut DeviceEvidenceSnapshot| {
            value.guest.pending_dpc.as_mut().unwrap().token ^= 1
        }
    );
    changed!(
        "guest pending DPC source",
        |value: &mut DeviceEvidenceSnapshot| {
            value.guest.pending_dpc.as_mut().unwrap().source = DpcSubmissionSource::Dmem
        }
    );
    changed!(
        "guest pending DPC start",
        |value: &mut DeviceEvidenceSnapshot| {
            value.guest.pending_dpc.as_mut().unwrap().start ^= 8
        }
    );
    changed!(
        "guest pending DPC end",
        |value: &mut DeviceEvidenceSnapshot| {
            value.guest.pending_dpc.as_mut().unwrap().end ^= 8
        }
    );
    changed!("pending DPC token", |value: &mut DeviceEvidenceSnapshot| {
        value.pending_dpc.as_mut().unwrap().submission.token ^= 1
    });
    changed!(
        "pending DPC source",
        |value: &mut DeviceEvidenceSnapshot| {
            value.pending_dpc.as_mut().unwrap().submission.source = DpcSubmissionSource::Dmem
        }
    );
    changed!("pending DPC start", |value: &mut DeviceEvidenceSnapshot| {
        value.pending_dpc.as_mut().unwrap().submission.start ^= 8
    });
    changed!("pending DPC end", |value: &mut DeviceEvidenceSnapshot| {
        value.pending_dpc.as_mut().unwrap().submission.end ^= 8
    });
    changed!(
        "pending DPC rollback START",
        |value: &mut DeviceEvidenceSnapshot| {
            value.pending_dpc.as_mut().unwrap().rollback_start ^= 8
        }
    );
    changed!(
        "pending DPC rollback END",
        |value: &mut DeviceEvidenceSnapshot| {
            value.pending_dpc.as_mut().unwrap().rollback_end ^= 8
        }
    );
    changed!(
        "pending DPC rollback CURRENT",
        |value: &mut DeviceEvidenceSnapshot| {
            value.pending_dpc.as_mut().unwrap().rollback_current ^= 8
        }
    );
    changed!(
        "pending DPC rollback STATUS",
        |value: &mut DeviceEvidenceSnapshot| {
            value.pending_dpc.as_mut().unwrap().rollback_status ^= 1
        }
    );
    changed!("pi domain timing", |value: &mut DeviceEvidenceSnapshot| {
        value.guest.pi_domain2.release ^= 1
    });
    changed!("pi timing policy", |value: &mut DeviceEvidenceSnapshot| {
        value.pi_timing_policy.push(1)
    });
    changed!("pending PI", |value: &mut DeviceEvidenceSnapshot| {
        value.pending_pi = Some(PendingPiSnapshot {
            token: 1,
            request: PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(4),
                device: PiDeviceAddress::RomOffset(8),
                len: 12,
            },
        })
    });
    changed!("current AI", |value: &mut DeviceEvidenceSnapshot| {
        value.current_ai = Some(PendingAiSnapshot {
            id: fn64_runtime::AiDmaId::new(1),
            token: 2,
            request: AiDmaRequest {
                dram_addr: RdramAddr::from_offset(16),
                len: 32,
                sample_rate_hz: 32_000,
            },
            started_at: fn64_runtime::EmulatedInstant::new(40),
            deadline: fn64_runtime::EmulatedInstant::new(80),
        })
    });
    changed!("queued AI", |value: &mut DeviceEvidenceSnapshot| {
        value.queued_ai = Some(fn64_runtime::QueuedAiSnapshot {
            id: fn64_runtime::AiDmaId::new(2),
            request: AiDmaRequest {
                dram_addr: RdramAddr::from_offset(48),
                len: 64,
                sample_rate_hz: 44_100,
            },
        })
    });
    changed!("pending SI", |value: &mut DeviceEvidenceSnapshot| {
        value.pending_si = Some(PendingSiSnapshot::Dma {
            token: 3,
            request: SiDmaRequest {
                kind: SiDmaKind::PifToDram,
                dram_addr: RdramAddr::from_offset(80),
            },
        })
    });
    changed!("SI error", |value: &mut DeviceEvidenceSnapshot| {
        value.si_dma_error = true
    });
    changed!("SI policy", |value: &mut DeviceEvidenceSnapshot| {
        value.si_latency = Cycles::new(2)
    });
    changed!("pending direct PIF control", |value: &mut DeviceEvidenceSnapshot| {
        value.pending_si = Some(PendingSiSnapshot::PifControl {
            token: 4,
            command: fn64_runtime::PifControlCommand::TerminateBoot,
        })
    });
    changed!("direct PIF control policy", |value: &mut DeviceEvidenceSnapshot| {
        value.pif_control_latency = Cycles::new(4_618)
    });
    changed!("PIF RAM", |value: &mut DeviceEvidenceSnapshot| {
        value.pif_ram[63] = 1
    });
    changed!("RSP DMEM", |value: &mut DeviceEvidenceSnapshot| {
        value.rsp_dmem[4095] = 1
    });
    changed!("RSP IMEM", |value: &mut DeviceEvidenceSnapshot| {
        value.rsp_imem[4095] = 1
    });
    changed!("SP registers", |value: &mut DeviceEvidenceSnapshot| {
        value.sp_pc = 4
    });
    changed!("SP semaphore", |value: &mut DeviceEvidenceSnapshot| {
        value.sp_semaphore = true
    });
    let sp_request = SpDmaRequest {
        direction: SpDmaDirection::RdramToRsp,
        mem_addr: RspMemAddr::from_register(0x20),
        dram_addr: RdramAddr::from_offset(0x100),
        encoded_len: 7,
    };
    changed!("active SP DMA", |value: &mut DeviceEvidenceSnapshot| {
        value.active_sp_dma = Some(PendingSpDmaSnapshot {
            token: 4,
            request: sp_request,
        })
    });
    changed!("queued SP DMA", |value: &mut DeviceEvidenceSnapshot| {
        value.queued_sp_dma = Some(sp_request)
    });
    changed!("SP DMA policy", |value: &mut DeviceEvidenceSnapshot| {
        value.sp_dma_setup_cycles = Cycles::new(9)
    });
    changed!("VI registers", |value: &mut DeviceEvidenceSnapshot| {
        value.vi_registers[13] = 1
    });
    changed!("VI epoch", |value: &mut DeviceEvidenceSnapshot| {
        value.vi_epoch = fn64_runtime::EmulatedInstant::new(1)
    });
    changed!(
        "pending RCP tokens",
        |value: &mut DeviceEvidenceSnapshot| { value.pending_dp_token = Some(5) }
    );
    changed!(
        "scheduled event order",
        |value: &mut DeviceEvidenceSnapshot| {
            value.scheduled_events.push(ScheduledDeviceEventSnapshot {
                at: fn64_runtime::EmulatedInstant::new(43),
                sequence: 5,
                token: 5,
                kind: ScheduledDeviceEventKind::Dp,
            })
        }
    );
    changed!(
        "scheduled direct PIF event kind",
        |value: &mut DeviceEvidenceSnapshot| {
            value.scheduled_events.push(ScheduledDeviceEventSnapshot {
                at: fn64_runtime::EmulatedInstant::new(43),
                sequence: 5,
                token: 5,
                kind: ScheduledDeviceEventKind::PifControl,
            })
        }
    );
    changed!(
        "next event sequence",
        |value: &mut DeviceEvidenceSnapshot| { value.next_event_sequence = 6 }
    );
    changed!("save bytes", |value: &mut DeviceEvidenceSnapshot| {
        value.save_bytes = Some(vec![0xff; 512])
    });
    changed!("pending EEPROM", |value: &mut DeviceEvidenceSnapshot| {
        value.pending_eeprom_write = Some(PendingEepromWriteSnapshot {
            offset: 8,
            data: [0x5a; 8],
            ready_at: Cycles::new(100),
        })
    });

    for (name, value) in cases {
        assert_ne!(
            sha256_hex(&encode_test_device(value, peripherals_snapshot())),
            baseline_sha,
            "device evidence omitted {name}"
        );
    }
}

#[test]
fn device_evidence_wire_binds_executor_peripheral_and_manager_state() {
    use fn64_runtime::{
        ContInput, ControllerPak, GameBoyCartridgeEvidenceSnapshot,
        GameBoyMapperEvidenceSnapshot, PfsKey, PfsNoteEvidenceSnapshot, PortState,
        RetraceScheduleEvidenceSnapshot, TransferPakEvidenceSnapshot, VoiceData,
        VoiceEvidenceSnapshot,
    };

    let device = snapshot(42);
    let baseline = peripherals_snapshot();
    let baseline_sha = sha256_hex(&encode_test_device(device.clone(), baseline.clone()));
    let mut cases = Vec::new();
    macro_rules! changed {
        ($name:literal, $body:expr) => {{
            let mut value = baseline.clone();
            $body(&mut value);
            cases.push(($name, value));
        }};
    }

    changed!(
        "controller identity",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.peripherals.pif.ports[3] = PortState::StandardControllerNoPak;
        }
    );
    changed!(
        "controller input",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.peripherals.pif.inputs[0] = ContInput {
                button: 0x8000,
                stick_x: -12,
                stick_y: 34,
            };
        }
    );
    changed!(
        "rumble state",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.peripherals.pif.rumble_on[0] = true;
        }
    );
    changed!(
        "Controller Pak raw image",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            let mut pak = ControllerPak::new().evidence_snapshot();
            pak.raw[31] = 0x5a;
            value.peripherals.controller_paks[2] = Some(pak);
        }
    );
    changed!(
        "Controller Pak bank count",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            let mut pak = ControllerPak::new().evidence_snapshot();
            pak.bank_count = 2;
            value.peripherals.controller_paks[2] = Some(pak);
        }
    );
    changed!(
        "Controller Pak active bank",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            let mut pak = ControllerPak::new().evidence_snapshot();
            pak.active_bank = 1;
            value.peripherals.controller_paks[2] = Some(pak);
        }
    );
    changed!(
        "Controller Pak semantic notes",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            let mut pak = ControllerPak::new().evidence_snapshot();
            pak.notes[0] = Some(PfsNoteEvidenceSnapshot {
                key: PfsKey {
                    company_code: 0x1234,
                    game_code: 0x5566_7788,
                    game_name: [0x41; 16],
                    ext_name: [0x42; 4],
                },
                pages: vec![5, 6],
            });
            value.peripherals.controller_paks[2] = Some(pak);
        }
    );
    changed!(
        "Transfer Pak register state",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.peripherals.transfer_paks[1] = Some(TransferPakEvidenceSnapshot {
                now: Cycles::new(42),
                enabled: true,
                transfer_bank: 2,
                access_mode: 1,
                cartridge: None,
                cartridge_pulled: true,
                reset_detected: true,
            });
        }
    );
    changed!(
        "Transfer Pak cartridge and mapper",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.peripherals.transfer_paks[1] = Some(TransferPakEvidenceSnapshot {
                now: Cycles::new(42),
                enabled: false,
                transfer_bank: 0,
                access_mode: 0,
                cartridge: Some(GameBoyCartridgeEvidenceSnapshot {
                    rom: vec![0x11; 0x150],
                    ram: vec![0x22; 32],
                    mapper: GameBoyMapperEvidenceSnapshot::Mbc3 {
                        timer_present: true,
                        ram_enabled: true,
                        rom_bank: 3,
                        select: 0x08,
                        latch_armed: true,
                        rtc: [1, 2, 3, 4, 5],
                        latched_rtc: [6, 7, 8, 9, 10],
                        subsecond_cycles: 99,
                    },
                }),
                cartridge_pulled: false,
                reset_detected: false,
            });
        }
    );
    changed!(
        "VRU dictionary and result",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.peripherals.voice_units[0] = Some(VoiceEvidenceSnapshot {
                initialized: true,
                raw_init_step: 0,
                expected_words: Some(1),
                words: vec![b"test".to_vec()],
                mask: vec![1],
                analog_gain: 1,
                digital_gain: 7,
                status: 7,
                pending_result: Some(VoiceData {
                    answer_num: 1,
                    answer: [2, 3, 4, 5, 6],
                    ..VoiceData::default()
                }),
            });
        }
    );
    changed!(
        "VRU raw initialization sequence position",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.peripherals.voice_units[0] = Some(VoiceEvidenceSnapshot {
                initialized: false,
                raw_init_step: 3,
                expected_words: None,
                words: Vec::new(),
                mask: Vec::new(),
                analog_gain: 0,
                digital_gain: 0,
                status: 0,
                pending_result: None,
            });
        }
    );
    changed!(
        "high-level VI manager",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.peripherals.vi.next_mode_ptr = Some(0x8000_1000);
            value.peripherals.vi.next_fade = PendingViFade::Factor(0x155);
        }
    );
    changed!(
        "compatibility retrace schedule",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.peripherals.retrace = Some(RetraceScheduleEvidenceSnapshot {
                interval: 100,
                next_due: 200,
            });
        }
    );
    changed!(
        "PI manager completion queue",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value
                .pending_pi_completions
                .push(fn64_abi::PendingPiCompletionEvidenceSnapshot {
                    request: PiDmaRequest {
                        direction: DmaDirection::ToRdram,
                        dram_addr: RdramAddr::from_offset(4),
                        device: PiDeviceAddress::RomOffset(8),
                        len: 12,
                    },
                    rdram_len: 8 * 1024 * 1024,
                    ret_queue: Some(RdramAddr::from_offset(16)),
                    ret_mesg: 20,
                });
        }
    );
    changed!(
        "SI manager completion metadata",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.pending_si_completion = Some(fn64_abi::PendingSiCompletionEvidenceSnapshot {
                request: SiDmaRequest {
                    kind: SiDmaKind::ControllerRead,
                    dram_addr: RdramAddr::from_offset(24),
                },
                owner: fn64_abi::PendingSiCompletionOwnerEvidenceSnapshot::ProcessRdram {
                    rdram_len: 8 * 1024 * 1024,
                },
            });
        }
    );
    changed!(
        "completed PFS is-plug transaction",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value
                .completed_pfs_is_plug
                .push(fn64_abi::PfsIsPlugTransactionEvidenceSnapshot {
                    thread: 7,
                    queue: RdramAddr::from_offset(0x20),
                    message: 0xCAFE,
                    result_addr: RdramAddr::from_offset(0x40),
                    bitpattern: 0b1010,
                });
        }
    );
    changed!(
        "pending PFS is-plug transaction",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.pending_si_completion = Some(fn64_abi::PendingSiCompletionEvidenceSnapshot {
                request: SiDmaRequest {
                    kind: SiDmaKind::ControllerQuery,
                    dram_addr: RdramAddr::from_offset(0),
                },
                owner: fn64_abi::PendingSiCompletionOwnerEvidenceSnapshot::PfsIsPlug(
                    fn64_abi::PfsIsPlugTransactionEvidenceSnapshot {
                        thread: 7,
                        queue: RdramAddr::from_offset(0x20),
                        message: 0xCAFE,
                        result_addr: RdramAddr::from_offset(0x40),
                        bitpattern: 0b0101,
                    },
                ),
            });
        }
    );
    changed!(
        "ABI VI mode and scale latches",
        |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
            value.vi.pending_mode = Some(fn64_abi::PendingViModeEvidenceSnapshot {
                registers: [1; 14],
                fields: [[2; 5], [3; 5]],
            });
            value.vi.pending_control = Some(4);
            value.vi.pending_x_scale_bits = Some(0.5f32.to_bits());
            value.vi.active_y_scale_bits = 0.75f32.to_bits();
        }
    );

    for (name, value) in cases {
        assert_ne!(
            sha256_hex(&encode_test_device(device.clone(), value)),
            baseline_sha,
            "device evidence omitted {name}"
        );
    }

    let transaction_digest = |bitpattern| {
        let mut value = baseline.clone();
        value
            .completed_pfs_is_plug
            .push(fn64_abi::PfsIsPlugTransactionEvidenceSnapshot {
                thread: 7,
                queue: RdramAddr::from_offset(0x20),
                message: 0xCAFE,
                result_addr: RdramAddr::from_offset(0x40),
                bitpattern,
            });
        sha256_hex(&encode_test_device(device.clone(), value))
    };
    assert_ne!(
        transaction_digest(0b0101),
        transaction_digest(0b1010),
        "completed PFS transactions with different future output collided"
    );
}

#[test]
fn device_state_v16_rejects_noncanonical_dpc_clock() {
    const VALUE: u32 = 0x0100_0041;
    assert_noncanonical_dpc_counter_rejected("DPC_CLOCK", VALUE, |snapshot| {
        snapshot.guest.dpc_clock = VALUE;
    });
}

#[test]
fn device_state_v16_rejects_noncanonical_dpc_bufbusy() {
    const VALUE: u32 = 0x0200_0042;
    assert_noncanonical_dpc_counter_rejected("DPC_BUFBUSY", VALUE, |snapshot| {
        snapshot.guest.dpc_busy = VALUE;
    });
}

#[test]
fn device_state_v16_rejects_noncanonical_dpc_pipebusy() {
    const VALUE: u32 = 0x0400_0043;
    assert_noncanonical_dpc_counter_rejected("DPC_PIPEBUSY", VALUE, |snapshot| {
        snapshot.guest.dpc_pipe_busy = VALUE;
    });
}

#[test]
fn device_state_v16_rejects_noncanonical_dpc_tmem() {
    const VALUE: u32 = 0x0800_0044;
    assert_noncanonical_dpc_counter_rejected("DPC_TMEM", VALUE, |snapshot| {
        snapshot.guest.dpc_tmem_busy = VALUE;
    });
}

#[test]
fn device_state_v16_accepts_maximum_canonical_dpc_counters() {
    let mut device = snapshot(42);
    device.guest.dpc_clock = 0x00ff_ffff;
    device.guest.dpc_busy = 0x00ff_ffff;
    device.guest.dpc_pipe_busy = 0x00ff_ffff;
    device.guest.dpc_tmem_busy = 0x00ff_ffff;
    assert!(try_encode_device_snapshot(
        device,
        executor_snapshot(),
        host_snapshot(),
        crate::ProgramEvidenceSnapshot::NoProgram,
    )
    .is_ok());
}

#[test]
fn device_state_v19_wire_binds_executor_and_abi_host_families() {
    use fn64_runtime::{
        EventRegistrationEvidenceSnapshot, ExecutorQueueEvidenceSnapshot,
        ExecutorRunningEvidenceSnapshot, MesgQueueEvidenceSnapshot,
        PendingResumeEvidenceSnapshot, RdramRegistrationEvidenceSnapshot,
        SectionEvidenceSnapshot, SectionLoadEvidenceSnapshot, StaticMirrorEvidenceSnapshot,
        StaticStorageEndEvidenceSnapshot, ThreadEvidenceSnapshot,
    };

    let device = snapshot(42);
    let executor = executor_snapshot();
    let host = host_snapshot();
    let encoded = encode_device_snapshot(
        device.clone(),
        executor.clone(),
        host.clone(),
        crate::ProgramEvidenceSnapshot::NoProgram,
    );
    assert!(encoded.starts_with(b"fn64.device-evidence.v19\0"));
    assert!(!encoded.starts_with(b"fn64.device-evidence.v12\0"));
    let baseline = sha256_hex(&encode_device_snapshot(
        device.clone(),
        executor.clone(),
        host.clone(),
        crate::ProgramEvidenceSnapshot::NoProgram,
    ));

    macro_rules! changed_executor {
        ($name:literal, $body:expr) => {{
            let mut value = executor.clone();
            $body(&mut value);
            assert_ne!(
                sha256_hex(&encode_device_snapshot(
                    device.clone(),
                    value,
                    host.clone(),
                    crate::ProgramEvidenceSnapshot::NoProgram,
                )),
                baseline,
                "device-state-v15 evidence omitted executor family {}",
                $name
            );
        }};
    }
    changed_executor!(
        "RDRAM registration",
        |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
            value.rdram = RdramRegistrationEvidenceSnapshot::Present { len: 0x80 };
        }
    );
    changed_executor!(
        "threads",
        |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
            value.threads.push(ThreadEvidenceSnapshot {
                id: 7,
                priority: -2,
                state: fn64_runtime::ThreadState::Dead,
                started: true,
            });
        }
    );
    changed_executor!(
        "run queue",
        |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
            value.run_queue.push(7);
        }
    );
    changed_executor!(
        "pending resume",
        |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
            value.pending_resumes.push(PendingResumeEvidenceSnapshot {
                thread: 7,
                resume: fn64_runtime::Resume::Delivered(0x1234),
            });
        }
    );
    changed_executor!(
        "message queues",
        |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
            value.queues.push(ExecutorQueueEvidenceSnapshot {
                address: RdramAddr::from_offset(0x100),
                queue: MesgQueueEvidenceSnapshot {
                    capacity: 2,
                    first: 1,
                    messages: vec![0x55],
                    blocked_receivers: vec![fn64_runtime::BlockedReceiverEvidenceSnapshot {
                        id: 7,
                        priority: -2,
                    }],
                    blocked_senders: Vec::new(),
                },
            });
        }
    );
    changed_executor!(
        "timer wheel",
        |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
            value.timers.next_id = 9;
        }
    );
    changed_executor!(
        "event table",
        |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
            value.event_table.push(EventRegistrationEvidenceSnapshot {
                event: 7,
                queue_addr: RdramAddr::from_offset(0x100),
                msg: 0x77,
            });
        }
    );
    changed_executor!(
        "running owner",
        |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
            value.running = ExecutorRunningEvidenceSnapshot::Active(7);
        }
    );
    changed_executor!(
        "monotonic master clock",
        |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
            value.sim_time = 42;
        }
    );
    changed_executor!(
        "OSTime bias",
        |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
            value.os_time_bias = 43;
        }
    );
    changed_executor!(
        "CP0 clock",
        |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
            value.cp0_count = 21;
            value.cp0_count_phase = 1;
            value.cp0_compare = 22;
            value.cp0_timer_pending = true;
        }
    );

    macro_rules! changed_host {
        ($name:literal, $body:expr) => {{
            let mut value = host.clone();
            $body(&mut value);
            assert_ne!(
                sha256_hex(&encode_device_snapshot(
                    device.clone(),
                    executor.clone(),
                    value,
                    crate::ProgramEvidenceSnapshot::NoProgram,
                )),
                baseline,
                "device-state-v15 evidence omitted ABI HostState family {}",
                $name
            );
        }};
    }
    changed_host!(
        "controller manager",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value.controller_manager = fn64_abi::ControllerManagerEvidenceSnapshot {
                initialized: true,
                channels: 1,
            };
        }
    );
    changed_host!(
        "Flash sequencer",
        // Perturb relative to the baseline rather than assigning a literal.
        // This assertion previously wrote 0x80, which silently stopped being a
        // mutation when `FlashState::default().status` became FLASH_STATUS_READY
        // (0x80) in 8c54a81 -- the coverage claim then passed vacuously and the
        // `assert_ne!` failed against an unchanged digest. Flipping a bit keeps
        // the probe honest under any future default.
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value.flash.status ^= 0x20;
        }
    );
    changed_host!(
        "section registry",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value.sections.sections.push(SectionEvidenceSnapshot {
                rom_addr: 1,
                ram_addr: 2,
                size: 4,
                funcs: Vec::new(),
            });
            value.sections.loaded_sections.push(0);
            value
                .sections
                .runtime_loads
                .push(SectionLoadEvidenceSnapshot {
                    section: 0,
                    load_vram: 3,
                });
            value.sections.static_mirror = Some(StaticMirrorEvidenceSnapshot {
                section: 0,
                next_rom: 2,
                next_static_off: 1,
            });
            value
                .sections
                .static_storage_ends
                .push(StaticStorageEndEvidenceSnapshot { section: 0, end: 4 });
        }
    );
    changed_host!(
        "rspboot images",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value
                .rsp_boot_images
                .push(fn64_abi::RspBootImageEvidenceSnapshot {
                    rdram_offset: 0x100,
                    bytes: vec![1, 2, 3],
                });
        }
    );
    changed_host!(
        "loaded RSP task token",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value.loaded_rsp_task = Some(fn64_abi::LoadedRspTaskEvidenceSnapshot {
                task_offset: 0x200,
                admission_generation: 7,
                header: fn64_runtime::OsTaskHeader {
                    task_type: fn64_runtime::M_GFXTASK,
                    flags: fn64_runtime::OS_TASK_YIELDED,
                    ucode_boot: 0x1000,
                    ucode_boot_size: 0x80,
                    ucode: 0x2000,
                    ucode_size: 0x1000,
                    ucode_data: 0x3000,
                    ucode_data_size: 0x40,
                    dram_stack: 0x4000,
                    dram_stack_size: 0x20,
                    output_buff: 0x5000,
                    output_buff_size: 0x5004,
                    data_ptr: 0x6000,
                    data_size: 0x18,
                    yield_data_ptr: 0x7000,
                    yield_data_size: 0x80,
                },
                resumed_data_identity: Some(fn64_abi::RspTaskDataIdentityEvidenceSnapshot {
                    rdram_offset: 0x3000,
                    byte_len: 0x40,
                    sha256: [0x31; 32],
                }),
            });
        }
    );
    changed_host!(
        "yielded RSP task lineage",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value
                .rsp_task_lineages
                .push(fn64_abi::RspTaskLineageEvidenceSnapshot {
                    task_offset: 0x200,
                    admission_generation: 7,
                    original_header: fn64_runtime::OsTaskHeader {
                        task_type: fn64_runtime::M_GFXTASK,
                        ucode_data: 0x3000,
                        ucode_data_size: 0x40,
                        yield_data_ptr: 0x7000,
                        yield_data_size: 0x80,
                        ..fn64_runtime::OsTaskHeader::default()
                    },
                    data_identity: Some(fn64_abi::RspTaskDataIdentityEvidenceSnapshot {
                        rdram_offset: 0x3000,
                        byte_len: 0x40,
                        sha256: [0x32; 32],
                    }),
                    phase: fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized,
                });
        }
    );
    changed_host!(
        "next RSP task admission generation",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value.next_rsp_task_admission_generation = value
                .next_rsp_task_admission_generation
                .checked_add(1)
                .expect("test admission generation overflow");
        }
    );
    changed_host!(
        "audio task execution policy",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value.audio_task_execution = fn64_abi::AudioTaskExecutionPolicy::LleAccuracy;
        }
    );
    changed_host!(
        "installed ROM identity",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value.rom_installed = true;
            value.installed_rom = Some(fn64_abi::InstalledRomEvidenceSnapshot {
                byte_len: 3,
                sha256: [0x5a; 32],
            });
        }
    );
    changed_host!(
        "cartridge save configuration",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value.cartridge_save = fn64_abi::CartridgeSaveEvidenceSnapshot::NoCartridgeSave;
        }
    );
    changed_host!(
        "PI handles and Leo configuration",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value.cart_rom_handle_vram = Some(0x8000_1000);
            value.flash_handle_vram = Some(0x8000_2000);
            value.leo_disk = Some(fn64_abi::LeoDiskConfig {
                handle_vram: 0x8000_3000,
                latency: 1,
                page_size: 2,
                release: 3,
                pulse_width: 4,
            });
        }
    );
    changed_host!(
        "thread and timer handle maps",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value
                .thread_handles
                .push(fn64_abi::ThreadHandleEvidenceSnapshot {
                    osthread_offset: 0x100,
                    executor_thread_id: 7,
                });
            value
                .thread_guest_ids
                .push(fn64_abi::ThreadGuestIdEvidenceSnapshot {
                    executor_thread_id: 7,
                    guest_os_id: 8,
                });
            value
                .timer_handles
                .push(fn64_abi::TimerHandleEvidenceSnapshot {
                    ostimer_offset: 0x200,
                    timer_id: 9,
                });
        }
    );
    changed_host!(
        "synthetic ID and RDRAM registration",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value.next_synthetic_thread_id ^= 1;
            value.registered_rdram.present = true;
            value.registered_rdram.byte_len = 0x80;
        }
    );
    changed_host!(
        "debug hardware",
        |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
            value.debug_hardware = fn64_abi::DebugHardware::Isv;
        }
    );
}

#[test]
fn device_state_v16_wire_binds_complete_rsp_interpreter_state() {
    let device = snapshot(42);
    let executor = executor_snapshot();
    let baseline_state = rsp_architectural_state(|_| {});
    let digest = |state| {
        let mut host = host_snapshot();
        host.rsp_interpreter_state =
            fn64_abi::RspInterpreterStateEvidenceSnapshot::Exact(state);
        sha256_hex(&encode_device_snapshot(
            device.clone(),
            executor.clone(),
            host,
            crate::ProgramEvidenceSnapshot::NoProgram,
        ))
    };
    let baseline = digest(baseline_state);

    macro_rules! changed {
        ($name:literal, $body:expr) => {{
            assert_ne!(
                digest(rsp_architectural_state($body)),
                baseline,
                "device-state-v15 evidence omitted RSP interpreter family {}",
                $name
            );
        }};
    }

    changed!("scalar GPRs", |machine| machine.ctx.r[7] = 1);
    changed!("DMA DRAM address", |machine| machine.ctx.dma_dram_address =
        8);
    changed!("DMA MEM address", |machine| machine.ctx.dma_mem_address = 8);
    changed!("jump target", |machine| machine.ctx.jump_target = 4);
    changed!("resume address", |machine| machine.ctx.resume_address = 4);
    changed!("resume delay", |machine| machine.ctx.resume_delay = true);
    changed!("VU registers", |machine| machine.ctx.rsp.regs.r[3][5] = -2);
    changed!("VU accumulator", |machine| machine.ctx.rsp.acc.set(6, -3));
    changed!("VU VCO", |machine| machine.ctx.rsp.flags.vco = 1);
    changed!("VU VCC", |machine| machine.ctx.rsp.flags.vcc = 1);
    changed!("VU VCE", |machine| machine.ctx.rsp.flags.vce = 1);
    changed!("VU divider input", |machine| machine.ctx.rsp.div_in = 1);
    changed!("VU divider input-valid latch", |machine| machine
        .ctx
        .rsp
        .div_in_loaded =
        true);
    changed!("VU divider output", |machine| machine.ctx.rsp.div_out = 1);
    changed!("SP status", |machine| {
        let mut state = rsp_execution_state();
        state.sp_status = 1;
        machine.overlay_device_execution_state(state);
    });
    changed!("SP semaphore", |machine| {
        let mut state = rsp_execution_state();
        state.sp_semaphore = true;
        machine.overlay_device_execution_state(state);
    });
    changed!("SP read length", |machine| {
        let mut state = rsp_execution_state();
        state.sp_dma_read_length = 7;
        machine.overlay_device_execution_state(state);
    });
    changed!("SP write length", |machine| {
        let mut state = rsp_execution_state();
        state.sp_dma_write_length = 7;
        machine.overlay_device_execution_state(state);
    });
    changed!("DPC START", |machine| {
        let mut state = rsp_execution_state();
        state.dpc_start = 8;
        machine.overlay_device_execution_state(state);
    });
    changed!("DPC END", |machine| {
        let mut state = rsp_execution_state();
        state.dpc_end = 8;
        machine.overlay_device_execution_state(state);
    });
    changed!("DPC CURRENT", |machine| {
        let mut state = rsp_execution_state();
        state.dpc_current = 8;
        machine.overlay_device_execution_state(state);
    });
    changed!("DPC STATUS", |machine| {
        let mut state = rsp_execution_state();
        state.dpc_status = 1;
        machine.overlay_device_execution_state(state);
    });
    changed!("DPC CLOCK", |machine| {
        let mut state = rsp_execution_state();
        state.dpc_clock = 1;
        machine.overlay_device_execution_state(state);
    });
    changed!("DPC BUFBUSY", |machine| {
        let mut state = rsp_execution_state();
        state.dpc_busy = 1;
        machine.overlay_device_execution_state(state);
    });
    changed!("DPC PIPEBUSY", |machine| {
        let mut state = rsp_execution_state();
        state.dpc_pipe_busy = 1;
        machine.overlay_device_execution_state(state);
    });
    changed!("DPC TMEM", |machine| {
        let mut state = rsp_execution_state();
        state.dpc_tmem_busy = 1;
        machine.overlay_device_execution_state(state);
    });
    changed!("RDRAM DPC submission words", |machine| {
        machine.rdram[8..16].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        machine.write_cp0(8, 8);
        machine.write_cp0(9, 16);
    });
    changed!("XBUS DPC submission payload and words", |machine| {
        for (offset, byte) in (1_u8..=8).enumerate() {
            machine.dmem.write_bu(0x20 + offset as u32, byte);
        }
        machine.write_cp0(11, 1 << 1);
        machine.write_cp0(8, 0x20);
        machine.write_cp0(9, 0x28);
    });

    let ordered_digest = |reverse| {
        digest(rsp_architectural_state(|machine| {
            machine.rdram[8..24]
                .copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
            let ranges = if reverse {
                [(16, 24), (8, 16)]
            } else {
                [(8, 16), (16, 24)]
            };
            for (start, end) in ranges {
                machine.write_cp0(8, start);
                machine.write_cp0(9, end);
            }
        }))
    };
    assert_ne!(
        ordered_digest(false),
        ordered_digest(true),
        "device-state-v15 evidence omitted queued DPC submission order"
    );
}

#[test]
fn device_state_v16_wire_distinguishes_rsp_interpreter_variants() {
    let state = rsp_architectural_state(|_| {});
    let encoded = |value| {
        let mut out = Vec::new();
        encode_rsp_interpreter_state(&mut out, value);
        out
    };
    let variants = [
        encoded(fn64_abi::RspInterpreterStateEvidenceSnapshot::Reset),
        encoded(fn64_abi::RspInterpreterStateEvidenceSnapshot::Exact(
            state.clone(),
        )),
        encoded(fn64_abi::RspInterpreterStateEvidenceSnapshot::HleCompatibility(state)),
        encoded(
            fn64_abi::RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable {
                owner: fn64_abi::RspInterpreterOwner::task(0x80, admission_generation(7)),
            },
        ),
        encoded(fn64_abi::RspInterpreterStateEvidenceSnapshot::InFlight {
            owner: fn64_abi::RspInterpreterOwner::task(0x80, admission_generation(7)),
        }),
        encoded(
            fn64_abi::RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable {
                owner: fn64_abi::RspInterpreterOwner::RawKick {
                    admission_generation: admission_generation(7),
                },
            },
        ),
        encoded(fn64_abi::RspInterpreterStateEvidenceSnapshot::InFlight {
            owner: fn64_abi::RspInterpreterOwner::RawKick {
                admission_generation: admission_generation(7),
            },
        }),
    ];
    assert_eq!(
        variants.iter().map(|value| value[0]).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4, 5, 6]
    );
    // Tags 3 and 4 are pinned byte-for-byte, not merely distinct: retained
    // reports from task-driven runs hash these exact bytes, so appending the
    // raw-kick tags must not perturb them.
    assert_eq!(
        variants[3],
        [
            &[3u8][..],
            &0x80u32.to_be_bytes()[..],
            &7u64.to_be_bytes()[..]
        ]
        .concat()
    );
    assert_eq!(
        variants[4],
        [
            &[4u8][..],
            &0x80u32.to_be_bytes()[..],
            &7u64.to_be_bytes()[..]
        ]
        .concat()
    );
    assert_eq!(
        variants
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        variants.len()
    );
    assert_ne!(
        encoded(
            fn64_abi::RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable {
                owner: fn64_abi::RspInterpreterOwner::task(0x80, admission_generation(7)),
            },
        ),
        encoded(
            fn64_abi::RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable {
                owner: fn64_abi::RspInterpreterOwner::task(0x84, admission_generation(7)),
            },
        )
    );
    assert_ne!(
        encoded(fn64_abi::RspInterpreterStateEvidenceSnapshot::InFlight {
            owner: fn64_abi::RspInterpreterOwner::task(0x80, admission_generation(7)),
        }),
        encoded(fn64_abi::RspInterpreterStateEvidenceSnapshot::InFlight {
            owner: fn64_abi::RspInterpreterOwner::task(0x84, admission_generation(7)),
        })
    );
    assert_ne!(
        encoded(
            fn64_abi::RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable {
                owner: fn64_abi::RspInterpreterOwner::task(0x80, admission_generation(7)),
            },
        ),
        encoded(
            fn64_abi::RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable {
                owner: fn64_abi::RspInterpreterOwner::task(0x80, admission_generation(8)),
            },
        ),
        "device-state-v15 unavailable evidence omitted task admission generation"
    );
    assert_ne!(
        encoded(fn64_abi::RspInterpreterStateEvidenceSnapshot::InFlight {
            owner: fn64_abi::RspInterpreterOwner::task(0x80, admission_generation(7)),
        }),
        encoded(fn64_abi::RspInterpreterStateEvidenceSnapshot::InFlight {
            owner: fn64_abi::RspInterpreterOwner::task(0x80, admission_generation(8)),
        }),
        "device-state-v15 in-flight evidence omitted task admission generation"
    );
}

#[test]
fn device_state_v16_wire_distinguishes_rsp_task_admission_generations() {
    let encoded = |host| {
        let mut out = Vec::new();
        encode_abi_host(&mut out, host);
        out
    };
    let baseline = host_snapshot();

    let mut loaded_first = baseline.clone();
    loaded_first.loaded_rsp_task = Some(fn64_abi::LoadedRspTaskEvidenceSnapshot {
        task_offset: 0x200,
        admission_generation: 7,
        header: fn64_runtime::OsTaskHeader::default(),
        resumed_data_identity: None,
    });
    let mut loaded_second = loaded_first.clone();
    loaded_second
        .loaded_rsp_task
        .as_mut()
        .unwrap()
        .admission_generation = 8;
    assert_ne!(
        encoded(loaded_first),
        encoded(loaded_second),
        "device-state-v15 loaded-task evidence omitted admission generation"
    );

    let mut lineage_first = baseline.clone();
    lineage_first
        .rsp_task_lineages
        .push(fn64_abi::RspTaskLineageEvidenceSnapshot {
            task_offset: 0x200,
            admission_generation: 7,
            original_header: fn64_runtime::OsTaskHeader::default(),
            data_identity: None,
            phase: fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::Running,
        });
    let mut lineage_second = lineage_first.clone();
    lineage_second.rsp_task_lineages[0].admission_generation = 8;
    assert_ne!(
        encoded(lineage_first),
        encoded(lineage_second),
        "device-state-v15 lineage evidence omitted admission generation"
    );

    let mut next_first = baseline.clone();
    next_first.next_rsp_task_admission_generation = 7;
    let mut next_second = next_first.clone();
    next_second.next_rsp_task_admission_generation = 8;
    assert_ne!(
        encoded(next_first),
        encoded(next_second),
        "device-state-v15 evidence omitted next admission generation"
    );
}

#[test]
fn device_state_v16_wire_distinguishes_rsp_task_lineage_phases() {
    let device = snapshot(42);
    let executor = executor_snapshot();
    let digest = |phase| {
        let mut host = host_snapshot();
        host.rsp_task_lineages
            .push(fn64_abi::RspTaskLineageEvidenceSnapshot {
                task_offset: 0x200,
                admission_generation: 7,
                original_header: fn64_runtime::OsTaskHeader {
                    task_type: fn64_runtime::M_GFXTASK,
                    ucode_data: 0x3000,
                    ucode_data_size: 0x40,
                    ..fn64_runtime::OsTaskHeader::default()
                },
                data_identity: Some(fn64_abi::RspTaskDataIdentityEvidenceSnapshot {
                    rdram_offset: 0x3000,
                    byte_len: 0x40,
                    sha256: [0x32; 32],
                }),
                phase,
            });
        sha256_hex(&encode_device_snapshot(
            device.clone(),
            executor.clone(),
            host,
            crate::ProgramEvidenceSnapshot::NoProgram,
        ))
    };
    let distinct: std::collections::BTreeSet<_> = [
        digest(fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::Running),
        digest(fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized),
        digest(fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::ResumeLoaded),
    ]
    .into_iter()
    .collect();
    assert_eq!(distinct.len(), 3);
}

#[test]
fn device_state_v16_wire_distinguishes_every_audio_execution_policy() {
    let digest = |policy| {
        let mut host = host_snapshot();
        host.audio_task_execution = policy;
        sha256_hex(&encode_device_snapshot(
            snapshot(42),
            executor_snapshot(),
            host,
            crate::ProgramEvidenceSnapshot::NoProgram,
        ))
    };

    let digests = [
        digest(fn64_abi::AudioTaskExecutionPolicy::Unconfigured),
        digest(fn64_abi::AudioTaskExecutionPolicy::Translated {
            artifact_sha256: [0x11; 32],
        }),
        digest(fn64_abi::AudioTaskExecutionPolicy::Translated {
            artifact_sha256: [0x22; 32],
        }),
        digest(fn64_abi::AudioTaskExecutionPolicy::LleAccuracy),
        digest(fn64_abi::AudioTaskExecutionPolicy::DiagnosticSkip),
    ];
    for left in 0..digests.len() {
        for right in (left + 1)..digests.len() {
            assert_ne!(digests[left], digests[right]);
        }
    }
}

#[test]
fn device_state_v16_wire_distinguishes_native_program_classes_and_identity() {
    let device = snapshot(42);
    let executor = executor_snapshot();
    let host = host_snapshot();
    let digest = |program| {
        sha256_hex(&encode_device_snapshot(
            device.clone(),
            executor.clone(),
            host.clone(),
            program,
        ))
    };
    let no_program = digest(crate::ProgramEvidenceSnapshot::NoProgram);
    let unidentified = digest(crate::ProgramEvidenceSnapshot::UnidentifiedNativeProgram);
    let native_a = digest(crate::ProgramEvidenceSnapshot::IdentifiedNativeArchive(
        crate::NativeProgramArtifactIdentity::new([0x41; 32]),
    ));
    let native_b = digest(crate::ProgramEvidenceSnapshot::IdentifiedNativeArchive(
        crate::NativeProgramArtifactIdentity::new([0x42; 32]),
    ));

    let distinct: std::collections::BTreeSet<_> =
        [no_program, unidentified, native_a, native_b]
            .into_iter()
            .collect();
    assert_eq!(distinct.len(), 4);
}

#[cfg(feature = "recomp-rs")]
#[test]
fn device_state_v16_wire_binds_typed_program_identity_and_dynamic_state() {
    use fn64_abi::recompiled::{
        LiveExecutableRegionEvidenceSnapshot, PendingExecutableWriteEvidenceSnapshot,
        RecompiledProgramEvidenceSnapshot,
    };
    use fn64_cpu_runtime::{
        BankId, BlockProgramEvidenceSnapshot, CodeBankEvidenceSnapshot,
        CodeSpanEvidenceSnapshot, ExecutionKey, GuestPc, InstructionWordIdentity,
        MappedAotEvidenceSnapshot, PhysicalCodeBankEvidenceSnapshot,
        PhysicalCodeSpanEvidenceSnapshot, ProgramArtifactIdentity,
        ProgramIdentityEvidenceSnapshot, ProgramIdentitySource,
    };

    let identity = |byte| ProgramArtifactIdentity::new([byte; 32]);
    let device = snapshot(42);
    let executor = executor_snapshot();
    let host = host_snapshot();
    let baseline = sha256_hex(&encode_device_snapshot(
        device.clone(),
        executor.clone(),
        host.clone(),
        crate::ProgramEvidenceSnapshot::NoProgram,
    ));
    let function = crate::ProgramEvidenceSnapshot::TypedRust(
        RecompiledProgramEvidenceSnapshot::Function {
            identity: ProgramIdentityEvidenceSnapshot {
                identity: identity(1),
                source: ProgramIdentitySource::CallerSupplied,
            },
        },
    );
    let function_sha = sha256_hex(&encode_device_snapshot(
        device.clone(),
        executor.clone(),
        host.clone(),
        function,
    ));
    assert_ne!(function_sha, baseline);

    let block =
        crate::ProgramEvidenceSnapshot::TypedRust(RecompiledProgramEvidenceSnapshot::Block {
            program: BlockProgramEvidenceSnapshot {
                identity: ProgramIdentityEvidenceSnapshot {
                    identity: identity(2),
                    source: ProgramIdentitySource::CanonicalBlockProgramSha256,
                },
                banks: vec![CodeBankEvidenceSnapshot {
                    id: BankId::new(3),
                    runner_artifact_identity: identity(4),
                    spans: vec![CodeSpanEvidenceSnapshot {
                        vram_start: GuestPc::new(0x8000_1000),
                        words: vec![0x1234_5678],
                    }],
                }],
                physical_banks: Vec::new(),
                mapped_aot: Vec::new(),
            },
            dispatch_artifact_identity: identity(5),
            instruction_budget: 100,
            executable_regions: vec![LiveExecutableRegionEvidenceSnapshot {
                physical_start: 0x1000,
                physical_end: 0x2000,
                virtual_start: GuestPc::new(0x8000_1000),
                virtual_end: GuestPc::new(0x8000_2000),
                active_bank: BankId::new(3),
                active_generation: 6,
                next_generation: 7,
                builder_artifact_identity: identity(8),
                activation:
                    fn64_abi::recompiled::ExecutableActivationEvidence::EagerPublication,
            }],
            pending_executable_writes: vec![PendingExecutableWriteEvidenceSnapshot {
                physical_start: 0x1100,
                physical_end: 0x1200,
            }],
        });
    let block_sha = sha256_hex(&encode_device_snapshot(
        device.clone(),
        executor.clone(),
        host.clone(),
        block.clone(),
    ));
    assert_ne!(block_sha, baseline);
    assert_ne!(block_sha, function_sha);

    let mut physical = block.clone();
    let crate::ProgramEvidenceSnapshot::TypedRust(RecompiledProgramEvidenceSnapshot::Block {
        program,
        ..
    }) = &mut physical
    else {
        unreachable!("fixture is a typed block program")
    };
    program
        .physical_banks
        .push(PhysicalCodeBankEvidenceSnapshot {
            id: BankId::new(9),
            spans: vec![PhysicalCodeSpanEvidenceSnapshot {
                physical_start: 0x3000,
                words: vec![0x8765_4321],
            }],
        });
    let physical_sha = sha256_hex(&encode_device_snapshot(
        device.clone(),
        executor.clone(),
        host.clone(),
        physical.clone(),
    ));
    assert_ne!(physical_sha, block_sha);

    let mut mapped = physical;
    let crate::ProgramEvidenceSnapshot::TypedRust(RecompiledProgramEvidenceSnapshot::Block {
        program,
        ..
    }) = &mut mapped
    else {
        unreachable!("fixture is a typed block program")
    };
    program.mapped_aot.push(MappedAotEvidenceSnapshot {
        entry: ExecutionKey::new(BankId::new(9), GuestPc::new(0x0040_0000)),
        instructions: vec![InstructionWordIdentity::new(BankId::new(9), 0x3000)],
        expected_words: vec![0x8765_4321],
        runner_artifact_identity: identity(10),
    });
    let mapped_sha = sha256_hex(&encode_device_snapshot(
        device.clone(),
        executor.clone(),
        host.clone(),
        mapped.clone(),
    ));
    assert_ne!(mapped_sha, physical_sha);

    let crate::ProgramEvidenceSnapshot::TypedRust(RecompiledProgramEvidenceSnapshot::Block {
        program,
        ..
    }) = &mut mapped
    else {
        unreachable!("fixture is a typed block program")
    };
    program.mapped_aot[0].expected_words[0] ^= 1;
    let changed_expected_word_sha =
        sha256_hex(&encode_device_snapshot(device, executor, host, mapped));
    assert_ne!(changed_expected_word_sha, mapped_sha);
}
