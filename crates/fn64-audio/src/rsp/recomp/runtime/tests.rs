use super::*;

#[test]
fn disabled_or_exhausted_dma_traces_do_not_build_the_payload() {
    let sequence = AtomicU64::new(0);
    let builds = std::cell::Cell::new(0);
    let build = || {
        builds.set(builds.get() + 1);
        7u8
    };

    assert_eq!(build_dma_trace(None, false, &sequence, build), None);
    assert_eq!(
        build_dma_trace(Some(DmaTraceConfig { limit: 0 }), false, &sequence, build),
        None
    );
    assert_eq!(
        build_dma_trace(Some(DmaTraceConfig { limit: 2 }), true, &sequence, build),
        None
    );
    assert_eq!(builds.get(), 0);

    assert_eq!(
        build_dma_trace(Some(DmaTraceConfig { limit: 2 }), false, &sequence, build),
        Some((1, 7))
    );
    assert_eq!(builds.get(), 1);
}

#[test]
fn disabled_dma_trace_does_not_parse_its_dependent_limit() {
    assert_eq!(parse_dma_trace_config(false, Some("invalid")), None);
    assert_eq!(
        parse_dma_trace_config(true, Some("13")),
        Some(DmaTraceConfig { limit: 13 })
    );
}

fn populate_distinct_architectural_state(machine: &mut RspMachine<'_>) {
    machine.ctx.r = core::array::from_fn(|index| 0x1000_0000 | index as u32);
    machine.ctx.dma_dram_address = 0x0102_0304;
    machine.ctx.dma_mem_address = 0x1112_1314;
    machine.ctx.jump_target = 0x2122_2324;
    machine.ctx.resume_address = 0x3132_3334;
    machine.ctx.resume_delay = true;
    machine.ctx.rsp.regs.r[3][4] = -0x1234;
    machine.ctx.rsp.acc.set(5, -0x1234_5678);
    machine.ctx.rsp.flags.vco = 0x4567;
    machine.ctx.rsp.flags.vcc = 0x5678;
    machine.ctx.rsp.flags.vce = 0x69;
    machine.ctx.rsp.div_in = 0x6789;
    machine.ctx.rsp.div_in_loaded = true;
    machine.ctx.rsp.div_out = 0x789a;
    machine.ctx.steps = 0x8899_aabb_ccdd_eeff;
    machine.sp_status = 0x4142_4344;
    machine.sp_semaphore = true;
    machine.dma_read_length = 0x5152_5354;
    machine.dma_write_length = 0x6162_6364;
    machine.dp_start = 0x7172_7374;
    machine.dp_end = 0x8182_8384;
    machine.dp_current = 0x9192_9394;
    machine.dp_status = 0xa1a2_a3a4;
    machine.dp_clock = 0xb1b2_b3b4;
    machine.dp_busy = 0xc1c2_c3c4;
    machine.dp_pipe_busy = 0xd1d2_d3d4;
    machine.dp_tmem_busy = 0xe1e2_e3e4;
    machine.dp_submissions = vec![
        RspDpSubmission::from_xbus_bytes(
            0x100,
            0x108,
            vec![0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef],
        ),
        RspDpSubmission::from_rdram_words(0x200, 0x208, vec![0x89ab_cdef, 0x0123_4567]),
    ];
}

#[test]
fn architectural_snapshot_round_trips_every_future_visible_field() {
    let mut source_rdram = vec![0u8; 32];
    let mut source = RspMachine::new(&mut source_rdram);
    populate_distinct_architectural_state(&mut source);

    let state = source.snapshot_architectural_state();
    assert_eq!(state.gprs(), &source.ctx.r);
    assert_eq!(state.dma_dram_address(), 0x0102_0304);
    assert_eq!(state.dma_mem_address(), 0x1112_1314);
    assert_eq!(state.jump_target(), 0x2122_2324);
    assert_eq!(state.resume_address(), 0x3132_3334);
    assert!(state.resume_delay());
    assert_eq!(state.vu(), &source.ctx.rsp);
    assert_eq!(state.sp_status(), 0x4142_4344);
    assert!(state.sp_semaphore());
    assert_eq!(state.dma_read_length(), 0x5152_5354);
    assert_eq!(state.dma_write_length(), 0x6162_6364);
    assert_eq!(state.dp_start(), 0x7172_7374);
    assert_eq!(state.dp_end(), 0x8182_8384);
    assert_eq!(state.dp_current(), 0x9192_9394);
    assert_eq!(state.dp_status(), 0xa1a2_a3a4);
    assert_eq!(state.dp_clock(), 0xb1b2_b3b4);
    assert_eq!(state.dp_busy(), 0xc1c2_c3c4);
    assert_eq!(state.dp_pipe_busy(), 0xd1d2_d3d4);
    assert_eq!(state.dp_tmem_busy(), 0xe1e2_e3e4);
    assert_eq!(state.dp_submissions(), source.dp_submissions.as_slice());

    let mut target_rdram = vec![0u8; 32];
    let mut target = RspMachine::new(&mut target_rdram);
    target.ctx.steps = 0x1122_3344_5566_7788;
    target.restore_architectural_state(state.clone());
    assert_eq!(target.snapshot_architectural_state(), state);
    assert_eq!(
        target.ctx.steps, 0x1122_3344_5566_7788,
        "architectural restore must not replace diagnostic accounting"
    );

    let wrapped = RspMachineState::from_architectural_state(state.clone());
    assert_eq!(wrapped.architectural_state(), &state);
    assert_eq!(wrapped.diagnostic_steps(), 0);
    assert_eq!(wrapped.into_architectural_state(), state);
}

#[test]
fn device_overlay_replaces_only_fabric_owned_registers() {
    let mut rdram = vec![0u8; 32];
    let mut machine = RspMachine::new(&mut rdram);
    populate_distinct_architectural_state(&mut machine);
    machine.dp_submissions.clear();
    let scalar = machine.ctx.r;
    let jump_target = machine.ctx.jump_target;
    let resume_address = machine.ctx.resume_address;
    let resume_delay = machine.ctx.resume_delay;
    let vu = machine.ctx.rsp.clone();
    let diagnostic_steps = machine.ctx.steps;
    let fabric = fn64_runtime::RspExecutionState {
        pc: 0x0abc,
        sp_status: 0x0102_0304,
        sp_semaphore: false,
        sp_dma_mem_addr: fn64_runtime::RspMemAddr::from_register(0x1234),
        sp_dma_dram_addr: fn64_runtime::RdramAddr::from_offset(0x456788),
        sp_dma_read_length: 0x1112_1314,
        sp_dma_write_length: 0x2122_2324,
        dpc_start: 0x100,
        dpc_end: 0x180,
        dpc_current: 0x140,
        dpc_status: 0x3132_3334,
        dpc_clock: 0x4142_4344,
        dpc_busy: 0x5152_5354,
        dpc_pipe_busy: 0x6162_6364,
        dpc_tmem_busy: 0x7172_7374,
    };

    machine.overlay_device_execution_state(fabric);
    let state = machine.snapshot_architectural_state();
    assert_eq!(state.gprs(), &scalar);
    assert_eq!(state.jump_target(), jump_target);
    assert_eq!(state.resume_address(), resume_address);
    assert_eq!(state.resume_delay(), resume_delay);
    assert_eq!(state.vu(), &vu);
    assert_eq!(machine.ctx.steps, diagnostic_steps);
    assert_eq!(state.sp_status(), fabric.sp_status);
    assert_eq!(state.sp_semaphore(), fabric.sp_semaphore);
    assert_eq!(state.dma_mem_address(), 0x1234);
    assert_eq!(state.dma_dram_address(), 0x456788);
    assert_eq!(state.dma_read_length(), fabric.sp_dma_read_length);
    assert_eq!(state.dma_write_length(), fabric.sp_dma_write_length);
    assert_eq!(state.dp_start(), fabric.dpc_start);
    assert_eq!(state.dp_end(), fabric.dpc_end);
    assert_eq!(state.dp_current(), fabric.dpc_current);
    assert_eq!(state.dp_status(), fabric.dpc_status);
    assert_eq!(state.dp_clock(), fabric.dpc_clock);
    assert_eq!(state.dp_busy(), fabric.dpc_busy);
    assert_eq!(state.dp_pipe_busy(), fabric.dpc_pipe_busy);
    assert_eq!(state.dp_tmem_busy(), fabric.dpc_tmem_busy);
}

#[test]
fn complete_machine_snapshot_keeps_diagnostics_separate() {
    let mut source_rdram = vec![0u8; 32];
    let mut source = RspMachine::new(&mut source_rdram);
    populate_distinct_architectural_state(&mut source);
    let complete = source.snapshot_state();
    let expected_architecture = source.snapshot_architectural_state();
    assert_eq!(complete.architectural_state(), &expected_architecture);
    assert_eq!(complete.diagnostic_steps(), source.ctx.steps);

    let mut target_rdram = vec![0u8; 32];
    let mut target = RspMachine::new(&mut target_rdram);
    target.restore_state(complete);
    assert_eq!(target.snapshot_architectural_state(), expected_architecture);
    assert_eq!(target.ctx.steps, source.ctx.steps);
}

#[test]
fn rdram_write_journal_canonicalizes_backward_and_bridging_spans() {
    let mut rdram = vec![0u8; 0x100];
    let mut machine = RspMachine::new(&mut rdram);

    machine.record_rdram_write(0x80, 8);
    machine.record_rdram_write(0x20, 8);
    assert_eq!(machine.rdram_writes, vec![(0x20, 0x28), (0x80, 0x88)]);

    machine.record_rdram_write(0x28, 0x38);
    machine.record_rdram_write(0x58, 0x28);
    assert_eq!(machine.take_rdram_writes(), vec![(0x20, 0x88)]);
}

#[test]
fn rejected_dma_does_not_enter_diagnostic_journal() {
    let mut rdram = [0; 8];
    let mut machine = RspMachine::new(&mut rdram);
    machine.set_dma_dram(8);
    machine.set_dma_mem(0);
    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        machine.dma_read(7);
    }));
    assert!(rejected.is_err());
    assert!(machine.take_dma_journal().is_empty());
}

#[test]
fn draining_diagnostic_journal_cannot_change_machine_state() {
    let mut rdram = [0; 16];
    let mut machine = RspMachine::new(&mut rdram);
    machine.set_dma_dram(0);
    machine.set_dma_mem(0);
    assert_eq!(machine.dma_read(7), None);
    let before = machine.snapshot_state();
    assert_eq!(
        machine.take_dma_journal(),
        vec![RspDmaJournalEntry {
            direction: RspDmaDirection::Read,
            effective_dram_address: 0,
            sp_mem_address: 0,
            raw_length_descriptor: 7,
        }]
    );
    assert_eq!(machine.snapshot_state(), before);
    assert!(machine.take_dma_journal().is_empty());
}

#[test]
fn restoring_non_memory_state_keeps_rdram_and_its_write_journal_paired() {
    let mut source_rdram = vec![0u8; 32];
    let mut source = RspMachine::new(&mut source_rdram);
    populate_distinct_architectural_state(&mut source);
    let architecture = source.snapshot_architectural_state();
    let complete = source.snapshot_state();

    let mut target_rdram = vec![0u8; 32];
    let mut target = RspMachine::new(&mut target_rdram);
    target.store_w(0, 0x0123_4567);
    target.store_w(4, 0x89ab_cdef);
    target.set_dma_mem(0);
    target.set_dma_dram(0);
    target.dma_write(7);
    assert_eq!(target.rdram_writes, vec![(0, 8)]);
    let written_bytes = target.rdram[0..8].to_vec();

    target.restore_architectural_state(architecture.clone());
    assert_eq!(&target.rdram[0..8], written_bytes.as_slice());
    assert_eq!(target.rdram_writes, vec![(0, 8)]);
    assert_eq!(
        target.dp_submissions, architecture.dp_submissions,
        "queued DPC submissions are architectural and must be restored"
    );

    target.set_dma_mem(0);
    target.set_dma_dram(8);
    target.dma_write(7);
    assert_eq!(target.rdram_writes, vec![(0, 16)]);
    let written_bytes = target.rdram[0..16].to_vec();

    target.restore_state(complete);
    assert_eq!(&target.rdram[0..16], written_bytes.as_slice());
    assert_eq!(target.take_rdram_writes(), vec![(0, 16)]);
}

#[test]
fn architectural_equality_distinguishes_each_owned_field() {
    let mut rdram = vec![0u8; 32];
    let mut machine = RspMachine::new(&mut rdram);
    populate_distinct_architectural_state(&mut machine);
    let baseline = machine.snapshot_architectural_state();

    macro_rules! assert_distinct {
        ($mutation:expr) => {{
            let mut candidate = baseline.clone();
            $mutation(&mut candidate);
            assert_ne!(baseline, candidate);
        }};
    }

    assert_distinct!(|state: &mut RspArchitecturalState| state.gprs[7] ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dma_dram_address ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dma_mem_address ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.jump_target ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.resume_address ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.resume_delay = false);
    assert_distinct!(|state: &mut RspArchitecturalState| state.vu.div_out ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.sp_status ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.sp_semaphore = false);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dma_read_length ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dma_write_length ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dp_start ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dp_end ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dp_current ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dp_status ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dp_clock ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dp_busy ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dp_pipe_busy ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| state.dp_tmem_busy ^= 1);
    assert_distinct!(|state: &mut RspArchitecturalState| {
        let RspDpCommandSource::XbusBytes(bytes) = &mut state.dp_submissions[0].source else {
            panic!("first fixture submission must be XBUS")
        };
        bytes[0] ^= 1;
    });
    assert_distinct!(|state: &mut RspArchitecturalState| state.dp_submissions.reverse());
}

#[test]
fn lqv_sqv_roundtrip_full_quad() {
    let mut rdram = vec![0u8; 0x1000];
    let mut m = RspMachine::new(&mut rdram);
    // Write a known 16-byte pattern into DMEM at 0x00.
    for i in 0..16u32 {
        m.dmem.write_bu(i, (i as u8) + 1);
    }
    // LQV v3, 0(r0): load the whole quad into v3.
    m.vload(VLoadOp::Lqv, 3, 0, 0, 0);
    // Lane 0 = bytes 1,2 -> 0x0102.
    assert_eq!(m.ctx.rsp.regs.r[3][0], 0x0102);
    assert_eq!(m.ctx.rsp.regs.r[3][7], 0x0F10);
    // SQV v3, 0(r0) to a fresh offset (0x20), then read back bytes.
    m.vstore(VStoreOp::Sqv, 3, 0, 0x20, 0);
    for i in 0..16u32 {
        assert_eq!(m.dmem.read_bu(0x20 + i), (i as u8) + 1);
    }
}

#[test]
fn ldv_loads_eight_bytes_at_element() {
    let mut rdram = vec![0u8; 0x1000];
    let mut m = RspMachine::new(&mut rdram);
    for i in 0..8u32 {
        m.dmem.write_bu(0x40 + i, 0xA0 + i as u8);
    }
    // LDV v5[0], 0(base=r0 with base_val 0x40)
    m.vload(VLoadOp::Ldv, 5, 0, 0x40, 0);
    assert_eq!(m.ctx.rsp.regs.r[5][0], 0xA0A1u16 as i16);
    assert_eq!(m.ctx.rsp.regs.r[5][3], 0xA6A7u16 as i16);
    // Upper lanes untouched (still zero).
    assert_eq!(m.ctx.rsp.regs.r[5][4], 0);
}

#[test]
fn mtc2_mfc2_roundtrip_lane() {
    let mut rdram = vec![0u8; 32];
    let mut m = RspMachine::new(&mut rdram);
    m.mtc2(7, 4, 0x1234); // write element 4 (lane 2) of v7
    assert_eq!(m.ctx.rsp.regs.r[7][2], 0x1234);
    assert_eq!(m.mfc2(7, 4) as u16, 0x1234);
}

#[test]
fn mtc2_and_mfc2_wrap_byte_element_15() {
    let mut rdram = vec![0u8; 16];
    let mut m = RspMachine::new(&mut rdram);
    m.mtc2(7, 15, 0x1234);
    assert_eq!(m.mfc2(7, 15) as u16, 0x1234);
    let bytes = vec_to_bytes(&m.ctx.rsp.regs.r[7]);
    assert_eq!(bytes[15], 0x12);
    assert_eq!(bytes[0], 0x34);
}

#[test]
fn cfc2_ctc2_roundtrip_vcc() {
    let mut rdram = vec![0u8; 16];
    let mut m = RspMachine::new(&mut rdram);
    m.ctc2(1, 0x00AB);
    assert_eq!(m.ctc2_read_vcc(), 0x00AB);
    assert_eq!(m.cfc2(1) as u16, 0x00AB);
}

#[test]
fn dma_read_copies_rdram_into_dmem() {
    let mut rdram = vec![0u8; 0x1000];
    for i in 0..64usize {
        rdram[0x200 + i] = i as u8;
    }
    let mut m = RspMachine::new(&mut rdram);
    m.set_dma_dram(0x200);
    m.set_dma_mem(0x080);
    let swap = m.dma_read(63); // 63+1 = 64 bytes
    assert!(swap.is_none());
    // DMA copies FLAT bytes into DMEM (no swizzle at the DMA layer —
    // the ^3/^2 swizzle is imposed only by the sub-word RSP_MEM_*
    // accessors). So compare the flat backing store byte-for-byte.
    for i in 0..64usize {
        assert_eq!(m.dmem.as_bytes()[0x080 + i], i as u8);
    }
}

#[test]
fn dma_read_preserves_guest_byte_order_between_native_word_stores() {
    let mut rdram = vec![0u8; 0x1000];
    write_rdram_word(&mut rdram, 0x200, 0xAABB_CCDD);
    write_rdram_word(&mut rdram, 0x204, 0x1122_3344);

    let mut m = RspMachine::new(&mut rdram);
    m.set_dma_dram(0x200);
    m.set_dma_mem(0x080);
    assert_eq!(m.dma_read(7), None);

    assert_eq!(m.load_w(0x080), 0xAABB_CCDD);
    assert_eq!(m.load_hu(0x080), 0xAABB);
    assert_eq!(m.load_hu(0x082), 0xCCDD);
    assert_eq!(
        [0x080, 0x081, 0x082, 0x083].map(|addr| m.load_bu(addr) as u8),
        [0xAA, 0xBB, 0xCC, 0xDD],
        "raw DMA is correct only if RSP DMEM and RDRAM expose the same logical byte order"
    );
}

#[test]
fn dma_write_preserves_guest_byte_order_for_rdram_consumers() {
    let mut rdram = vec![0u8; 0x1000];
    let mut m = RspMachine::new(&mut rdram);
    m.store_w(0x080, 0x7F01_80FF);
    m.store_w(0x084, 0x1234_FEDC);
    m.set_dma_mem(0x080);
    m.set_dma_dram(0x200);
    m.dma_write(7);

    assert_eq!(read_rdram_i16(m.rdram, 0x200), 0x7F01);
    assert_eq!(read_rdram_i16(m.rdram, 0x202) as u16, 0x80FF);
    assert_eq!(
        [0x200, 0x201, 0x202, 0x203].map(|addr| read_rdram_u8(m.rdram, addr)),
        [0x7F, 0x01, 0x80, 0xFF],
        "AI/RDRAM readers must observe the PCM bytes in guest order after RSP DMA write"
    );
}

#[test]
fn dma_read_into_imem_signals_overlay_swap() {
    let mut rdram = vec![0u8; 0x1000];
    let mut m = RspMachine::new(&mut rdram);
    m.set_dma_mem(0x1000); // IMEM bit set
    assert_eq!(m.dma_read(0), Some(RspExitReason::SwapOverlay));
}

#[test]
fn imem_overlay_completion_replaces_logical_words_and_advances_dma() {
    let mut rdram = vec![0u8; 0x1000];
    write_rdram_word(&mut rdram, 0x200, 0x3C01_1234);
    write_rdram_word(&mut rdram, 0x204, 0x3421_5678);
    let mut m = RspMachine::new(&mut rdram);
    let mut imem = [0xAA; DMEM_SIZE];
    m.set_dma_dram(0x200);
    m.set_dma_mem(0x1020);
    assert_eq!(m.dma_read(7), Some(RspExitReason::SwapOverlay));

    m.complete_imem_dma(&mut imem);

    assert_eq!(
        &imem[0x20..0x28],
        &[0x3C, 0x01, 0x12, 0x34, 0x34, 0x21, 0x56, 0x78]
    );
    assert_eq!(m.ctx.dma_mem_address, 0x1028);
    assert_eq!(m.ctx.dma_dram_address, 0x208);
}

#[test]
fn pending_imem_dma_span_tracks_aligned_rectangular_and_wrapped_destinations() {
    let mut rdram = vec![0u8; 0x1000];
    let mut m = RspMachine::new(&mut rdram);
    m.set_dma_mem(0x1ffb);
    let descriptor = 7 | (1 << 12) | (8 << 20);
    assert_eq!(m.dma_read(descriptor), Some(RspExitReason::SwapOverlay));

    let span = m.pending_imem_dma_span();
    assert!(span.contains_pc(0x1ff8));
    assert!(span.contains_pc(0x1ffc));
    assert!(span.contains_pc(0x1000));
    assert!(span.contains_pc(0x1004));
    assert!(!span.contains_pc(0x1008));
    assert!(!span.contains_pc(0x1ff4));
}

#[test]
fn pending_imem_dma_span_covering_a_bank_contains_every_pc() {
    let mut rdram = vec![0u8; 0x1000];
    let mut m = RspMachine::new(&mut rdram);
    m.set_dma_mem(0x1180);
    let descriptor = 0x0fff;
    assert_eq!(m.dma_read(descriptor), Some(RspExitReason::SwapOverlay));
    let span = m.pending_imem_dma_span();
    assert!([0x1000, 0x117c, 0x1180, 0x1ffc]
        .into_iter()
        .all(|pc| span.contains_pc(pc)));
}

#[test]
fn dma_applies_eight_byte_alignment_count_and_skip() {
    let mut rdram = vec![0u8; 0x1000];
    for i in 0..8usize {
        rdram[0x100 + i] = 0x10 + i as u8;
        rdram[0x110 + i] = 0x20 + i as u8;
    }
    let mut m = RspMachine::new(&mut rdram);
    m.set_dma_dram(0x103);
    m.set_dma_mem(0x023);
    // length=8 bytes, count=2 lines, skip=8 bytes.
    let descriptor = 7 | (1 << 12) | (8 << 20);
    assert_eq!(m.dma_read(descriptor), None);
    assert_eq!(
        &m.dmem.as_bytes()[0x20..0x28],
        &(0x10u8..0x18).collect::<Vec<_>>()
    );
    assert_eq!(
        &m.dmem.as_bytes()[0x28..0x30],
        &(0x20u8..0x28).collect::<Vec<_>>()
    );
    assert_eq!(m.ctx.dma_mem_address, 0x30);
    assert_eq!(m.ctx.dma_dram_address, 0x120);
}

#[test]
#[should_panic(
    expected = "RSP DMA read line 1 range [0x001008, 0x001010) is outside admitted RDRAM ranges [0..4108]"
)]
fn dma_read_traps_when_a_rectangular_line_exceeds_physical_rdram() {
    let mut rdram = vec![0u8; 0x2000];
    let mut m = RspMachine::new(&mut rdram);
    m.set_dma_rdram_ranges(std::iter::once(0..0x100c).collect());
    m.set_dma_dram(0x1000);
    m.set_dma_mem(0);
    m.dma_read(7 | (1 << 12));
}

#[test]
fn dma_read_accepts_an_explicit_static_overlay_alias() {
    let mut rdram = vec![0u8; 0x2000];
    rdram[0x1800..0x1808].copy_from_slice(b"OVERLAY!");
    let mut m = RspMachine::new(&mut rdram);
    m.set_dma_rdram_ranges(vec![0..0x1000, 0x1800..0x1810]);
    m.set_dma_dram(0x1800);
    m.set_dma_mem(0x80);

    assert_eq!(m.dma_read(7), None);
    assert_eq!(&m.dmem.as_bytes()[0x80..0x88], b"OVERLAY!");
}

#[test]
fn cp0_status_break_semaphore_and_dp_registers_are_observable() {
    let mut rdram = vec![0u8; 32];
    let mut m = RspMachine::new(&mut rdram);

    // SP_STATUS write commands: set HALT, set SIG0, then clear HALT.
    assert_eq!(m.write_cp0(4, (1 << 1) | (1 << 10)), None);
    assert_eq!(m.read_cp0(4) & ((1 << 0) | (1 << 7)), (1 << 0) | (1 << 7));
    m.write_cp0(4, 1 << 0);
    assert_eq!(m.read_cp0(4) & 1, 0);
    m.break_rsp();
    assert_eq!(m.read_cp0(4) & 3, 3, "BREAK sets HALT and BROKE");

    assert_eq!(m.read_cp0(7), 0, "first semaphore read returns clear");
    assert_eq!(m.read_cp0(7), 1, "read atomically sets semaphore");
    m.write_cp0(7, 0xFFFF_FFFF);
    assert_eq!(m.read_cp0(7), 0, "any semaphore write clears it");

    m.write_cp0(8, 0x00000F);
    m.write_cp0(9, 0x00001F);
    assert_eq!(m.read_cp0(8), 0x000008);
    assert_eq!(m.read_cp0(9), 0x000018);
    assert_eq!(
        m.read_cp0(10),
        0x000018,
        "RDP command DMA completes synchronously"
    );
    assert_eq!(m.read_cp0(5), 0);
    assert_eq!(m.read_cp0(6), 0);
    assert_eq!(
        m.take_dp_submissions(),
        vec![RspDpSubmission::from_rdram_words(
            0x000008,
            0x000018,
            vec![0; 4],
        )]
    );
    m.write_cp0(11, 1 << 1);
    m.write_cp0(8, 0x80);
    m.write_cp0(9, 0x100);
    assert!(m.take_dp_submissions()[0].is_xbus());
}

#[test]
fn dpc_end_advances_submit_only_unconsumed_fifo_bytes() {
    let mut rdram = vec![0u8; 0x200];
    let mut m = RspMachine::new(&mut rdram);

    m.write_cp0(8, 0x180);
    m.write_cp0(9, 0x180);
    assert!(
        m.take_dp_submissions().is_empty(),
        "START == END initializes an empty command FIFO"
    );

    m.write_cp0(9, 0x1a0);
    m.write_cp0(9, 0x1c8);
    assert_eq!(
        m.take_dp_submissions(),
        vec![
            RspDpSubmission::from_rdram_words(0x180, 0x1a0, vec![0; 8]),
            RspDpSubmission::from_rdram_words(0x1a0, 0x1c8, vec![0; 10]),
        ],
        "each END write starts at CURRENT rather than replaying from START"
    );
    assert_eq!(m.read_cp0(10), 0x1c8);
}

#[test]
fn rdram_dpc_submission_owns_the_words_visible_at_cmd_end() {
    let mut rdram = vec![0u8; 0x200];
    write_rdram_word(&mut rdram, 0x100, 0x1122_3344);
    write_rdram_word(&mut rdram, 0x104, 0x5566_7788);
    let mut machine = RspMachine::new(&mut rdram);
    machine.write_cp0(8, 0x100);
    machine.write_cp0(9, 0x108);
    write_rdram_word(machine.rdram, 0x100, 0xdead_beef);

    let submission = machine.take_dp_submissions().pop().unwrap();
    assert_eq!(
        submission.source(),
        &RspDpCommandSource::RdramWords(vec![0x1122_3344, 0x5566_7788])
    );
}

fn submit_empty_rdram_command(machine: &mut RspMachine<'_>, start: u32) -> RspRdramReadEpoch {
    machine.write_cp0(8, start);
    machine.write_cp0(9, start + 8);
    machine.dp_submissions.last().unwrap().read_epoch()
}

fn dma_row_from_dmem(machine: &mut RspMachine<'_>, dram: u32, bytes: &[u8; 8]) {
    machine.dmem.as_bytes_mut()[0x40..0x48].copy_from_slice(bytes);
    machine.set_dma_dram(dram);
    machine.set_dma_mem(0x40);
    machine.dma_write(7);
}

#[test]
fn temporal_rdram_history_reconstructs_overlapping_dma_rows_in_reverse() {
    let mut rdram = vec![0u8; 0x200];
    rdram[0x80..0x90].copy_from_slice(b"abcdefghijklmnop");
    let first_epoch;
    let second_epoch;
    let mut history;
    {
        let mut machine = RspMachine::new(&mut rdram);
        first_epoch = submit_empty_rdram_command(&mut machine, 0x100);
        dma_row_from_dmem(&mut machine, 0x80, b"ABCDEFGH");
        machine.write_cp0(8, 0x108);
        machine.write_cp0(9, 0x110);
        second_epoch = machine.dp_submissions.last().unwrap().read_epoch();
        dma_row_from_dmem(&mut machine, 0x88, b"IJKLMNOP");
        dma_row_from_dmem(&mut machine, 0x80, b"12345678");
        history = machine.take_deferred_dpc_history();
    }
    assert_eq!(first_epoch.get(), 1);
    assert_eq!(second_epoch.get(), 2);
    assert_eq!(history.before_image_count(), 3);
    assert_eq!(history.before_image_byte_len(), 24);
    assert_eq!(history.take_submissions().len(), 2);
    let mut first = [0u8; 16];
    history
        .copy_storage_at(first_epoch, &rdram, 0x80, &mut first)
        .unwrap();
    assert_eq!(&first, b"abcdefghijklmnop");
    let mut second = [0u8; 16];
    history
        .copy_storage_at(second_epoch, &rdram, 0x80, &mut second)
        .unwrap();
    assert_eq!(&second, b"ABCDEFGHijklmnop");
}

#[test]
fn temporal_rdram_history_refuses_same_length_unrelated_storage() {
    let mut rdram = vec![0u8; 0x200];
    let epoch;
    let history;
    {
        let mut machine = RspMachine::new(&mut rdram);
        epoch = submit_empty_rdram_command(&mut machine, 0x100);
        dma_row_from_dmem(&mut machine, 0x80, b"ABCDEFGH");
        history = machine.take_deferred_dpc_history();
    }
    let unrelated = rdram.clone();
    let mut out = [0u8; 8];
    assert!(matches!(
        history.copy_storage_at(epoch, &unrelated, 0x80, &mut out),
        Err(RspRdramHistoryError::StorageIdentityMismatch { .. })
    ));
}

#[test]
fn temporal_history_capacity_checks_both_exact_boundaries_without_allocating() {
    assert_eq!(
        checked_temporal_history_growth(
            MAX_TEMPORAL_BEFORE_IMAGES - 1,
            MAX_TEMPORAL_BEFORE_IMAGE_BYTES - 8,
            8,
        ),
        Ok(MAX_TEMPORAL_BEFORE_IMAGE_BYTES)
    );
    assert_eq!(
        checked_temporal_history_growth(
            MAX_TEMPORAL_BEFORE_IMAGES,
            MAX_TEMPORAL_BEFORE_IMAGE_BYTES - 8,
            8,
        ),
        Err(TemporalHistoryCapacityError::EntryLimit {
            current: MAX_TEMPORAL_BEFORE_IMAGES,
            added: 1,
            limit: MAX_TEMPORAL_BEFORE_IMAGES,
        })
    );
    assert_eq!(
        checked_temporal_history_growth(0, MAX_TEMPORAL_BEFORE_IMAGE_BYTES - 7, 8),
        Err(TemporalHistoryCapacityError::ByteLimit {
            current: MAX_TEMPORAL_BEFORE_IMAGE_BYTES - 7,
            added: 8,
            limit: MAX_TEMPORAL_BEFORE_IMAGE_BYTES,
        })
    );
}

#[test]
fn submissions_only_drain_cannot_discard_temporal_before_images() {
    let mut rdram = vec![0u8; 0x200];
    let mut machine = RspMachine::new(&mut rdram);
    submit_empty_rdram_command(&mut machine, 0x100);
    dma_row_from_dmem(&mut machine, 0x80, b"ABCDEFGH");
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        machine.take_dp_submissions();
    }))
    .expect_err("the legacy drain must not separate submissions from history");
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(message.contains("use take_deferred_dpc_history"));
    assert_eq!(machine.dp_submissions.len(), 1);
    assert_eq!(machine.rdram_before_images.len(), 1);
}

fn write_rdram_word(rdram: &mut [u8], offset: usize, value: u32) {
    rdram[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn read_rdram_i16(rdram: &[u8], offset: usize) -> i16 {
    let o = offset ^ 2;
    i16::from_ne_bytes([rdram[o], rdram[o + 1]])
}

fn read_rdram_u8(rdram: &[u8], offset: usize) -> u8 {
    rdram[offset ^ 3]
}

#[test]
fn all_fixed_width_vector_load_store_sizes_are_element_addressed() {
    let mut rdram = vec![0u8; 16];
    let mut m = RspMachine::new(&mut rdram);
    let cases = [
        (VLoadOp::Lbv, VStoreOp::Sbv, 1usize),
        (VLoadOp::Lsv, VStoreOp::Ssv, 2),
        (VLoadOp::Llv, VStoreOp::Slv, 4),
        (VLoadOp::Ldv, VStoreOp::Sdv, 8),
    ];
    for (load, store, count) in cases {
        for i in 0..count {
            m.dmem.write_bu(0x100 + i as u32, 0x80 + i as u8);
        }
        m.ctx.rsp.regs.r[3] = [0; 8];
        m.vload(load, 3, 4, 0x100, 0);
        m.vstore(store, 3, 4, 0x180, 0);
        for i in 0..count {
            assert_eq!(m.dmem.read_bu(0x180 + i as u32), 0x80 + i as u8);
        }
    }
}

#[test]
fn quad_and_rest_pair_crosses_an_unaligned_boundary() {
    let mut rdram = vec![0u8; 16];
    let mut m = RspMachine::new(&mut rdram);
    for i in 0..16u32 {
        m.dmem.write_bu(0x105 + i, i as u8);
    }
    m.vload(VLoadOp::Lqv, 4, 0, 0x105, 0);
    m.vload(VLoadOp::Lrv, 4, 0, 0x115, 0);
    assert_eq!(
        vec_to_bytes(&m.ctx.rsp.regs.r[4]),
        core::array::from_fn(|i| i as u8)
    );

    m.vstore(VStoreOp::Sqv, 4, 0, 0x305, 0);
    m.vstore(VStoreOp::Srv, 4, 0, 0x315, 0);
    for i in 0..16u32 {
        assert_eq!(m.dmem.read_bu(0x305 + i), i as u8);
    }
}

#[test]
fn quad_and_rest_stores_wrap_nonzero_byte_elements() {
    let mut rdram = vec![0u8; 16];
    let mut m = RspMachine::new(&mut rdram);
    m.ctx.rsp.regs.r[4] = bytes_to_vec(&core::array::from_fn(|i| i as u8));

    m.vstore(VStoreOp::Sqv, 4, 14, 0x305, 0);
    for i in 0..11u32 {
        assert_eq!(m.dmem.read_bu(0x305 + i), ((14 + i) & 15) as u8);
    }

    m.vstore(VStoreOp::Srv, 4, 14, 0x32F, 0);
    for i in 0..15u32 {
        assert_eq!(m.dmem.read_bu(0x320 + i), ((15 + i) & 15) as u8);
    }
}

#[test]
fn packed_half_and_fourth_vector_transfers_match_bit_positions() {
    let mut rdram = vec![0u8; 16];
    let mut m = RspMachine::new(&mut rdram);
    for i in 0..8u32 {
        m.dmem.write_bu(0x100 + i, 0x10 + i as u8);
        m.dmem.write_bu(0x200 + i * 2, 0x20 + i as u8);
    }
    m.vload(VLoadOp::Lpv, 1, 0, 0x100, 0);
    m.vstore(VStoreOp::Spv, 1, 0, 0x140, 0);
    m.vload(VLoadOp::Luv, 2, 0, 0x100, 0);
    m.vstore(VStoreOp::Suv, 2, 0, 0x150, 0);
    for i in 0..8u32 {
        assert_eq!(
            m.ctx.rsp.regs.r[1][i as usize] as u16,
            (0x10 + i as u16) << 8
        );
        assert_eq!(
            m.ctx.rsp.regs.r[2][i as usize] as u16,
            (0x10 + i as u16) << 7
        );
        assert_eq!(m.dmem.read_bu(0x140 + i), 0x10 + i as u8);
        assert_eq!(m.dmem.read_bu(0x150 + i), 0x10 + i as u8);
    }

    m.vload(VLoadOp::Lhv, 3, 0, 0x200, 0);
    m.vstore(VStoreOp::Shv, 3, 0, 0x240, 0);
    for i in 0..8u32 {
        assert_eq!(
            m.ctx.rsp.regs.r[3][i as usize] as u16,
            (0x20 + i as u16) << 7
        );
        assert_eq!(m.dmem.read_bu(0x240 + i * 2), 0x20 + i as u8);
    }

    for i in 0..4u32 {
        m.dmem.write_bu(0x280 + i * 4, 0x30 + i as u8);
    }
    m.vload(VLoadOp::Lfv, 4, 8, 0x280, 0);
    m.vstore(VStoreOp::Sfv, 4, 8, 0x2C0, 0);
    for i in 0..4u32 {
        assert_eq!(
            m.ctx.rsp.regs.r[4][4 + i as usize] as u16,
            (0x30 + i as u16) << 7
        );
        assert_eq!(m.dmem.read_bu(0x2C0 + i * 4), 0x30 + i as u8);
    }
}

#[test]
fn transpose_and_wrapped_store_cover_register_and_row_rotation() {
    let mut rdram = vec![0u8; 16];
    let mut m = RspMachine::new(&mut rdram);
    for i in 0..8u32 {
        m.dmem.write_h(0x300 + i * 2, (0x4000 + i) as i16);
    }
    m.vload(VLoadOp::Ltv, 8, 4, 0x300, 0);
    for i in 0..8usize {
        assert_eq!(m.ctx.rsp.regs.r[8 + i][(6 + i) & 7], (0x4000 + i) as i16);
    }
    m.vstore(VStoreOp::Stv, 8, 4, 0x340, 0);
    for i in 0..8u32 {
        assert_eq!(
            m.dmem.read_hu(0x340 + ((12 + i * 2) & 0xF)),
            0x4000 + i as u16
        );
    }

    m.ctx.rsp.regs.r[5] =
        core::array::from_fn(|i| u16::from_be_bytes([i as u8 * 2, i as u8 * 2 + 1]) as i16);
    let source = vec_to_bytes(&m.ctx.rsp.regs.r[5]);
    m.vstore(VStoreOp::Swv, 5, 3, 0x385, 0);
    for i in 0..16usize {
        assert_eq!(
            m.dmem.read_bu(0x380 + ((5 + i) & 0xF) as u32),
            source[(3 + i) & 0xF]
        );
    }
}

#[test]
fn boot_sets_stack_pointer() {
    let mut rdram = vec![0u8; 16];
    let m = RspMachine::new(&mut rdram);
    assert_eq!(m.reg(1), 0xFC0);
    assert_eq!(m.reg(0), 0); // r0 hardwired zero
}

impl RspMachine<'_> {
    // test-only helper
    fn ctc2_read_vcc(&self) -> u16 {
        self.ctx.rsp.flags.vcc
    }
}
