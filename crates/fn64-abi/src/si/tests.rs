use super::*;
use crate::pi::{load_rom, set_save};
use crate::test_support::*;

fn install_eeprom(kind: fn64_runtime::SaveType) {
    with_executor(|executor| *executor = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
        kind,
    )));
}

fn write_logical_bytes(rdram: &mut [u8], offset: u32, bytes: &[u8]) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for (index, byte) in bytes.iter().copied().enumerate() {
        view.write_u8(
            RdramAddr::from_offset(offset + u32::try_from(index).unwrap()),
            byte,
        );
    }
}

fn read_logical_bytes(rdram: &[u8], offset: u32, len: usize) -> Vec<u8> {
    let view = fn64_runtime::RdramView::from_storage(rdram);
    (0..len)
        .map(|index| {
            view.read_u8(RdramAddr::from_offset(
                offset + u32::try_from(index).unwrap(),
            ))
        })
        .collect()
}

fn raw_si_round_trip(rdram: &mut [u8]) {
    let write_deadline = crate::sim_time().saturating_add(1);
    let mut ctx = ctx_zeroed();
    ctx.r4 = 1;
    ctx.r5 = 0x8000_0000;
    unsafe { __osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
    assert_eq!(ctx.r2, 0);
    crate::advance_virtual_time(write_deadline);

    ctx.r4 = 0;
    unsafe { __osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
    assert_eq!(ctx.r2, 0);
    crate::advance_virtual_time(write_deadline + 1);
}

fn reset_controller_manager() {
    with_executor(|exec| *exec = fn64_runtime::Executor::new());
    with_host(|host| *host = HostState::default());
}

#[test]
fn os_si_device_busy_tracks_the_live_timed_si_channel() {
    reset_controller_manager();
    let mut rdram = vec![0u8; 64];
    let mut busy = ctx_zeroed();

    unsafe { __osSiDeviceBusy_recomp(std::ptr::null_mut(), &mut busy) };
    assert_eq!(busy.r2, 0);

    let mut dma = ctx_zeroed();
    dma.r4 = 0;
    dma.r5 = 0x8000_0000;
    unsafe { __osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut dma) };
    assert_eq!(dma.r2, 0);

    unsafe { __osSiDeviceBusy_recomp(std::ptr::null_mut(), &mut busy) };
    assert_eq!(busy.r2, 1);

    crate::advance_virtual_time(1);
    unsafe { __osSiDeviceBusy_recomp(std::ptr::null_mut(), &mut busy) };
    assert_eq!(busy.r2, 0);
}

fn initialize_controller_manager_for_test(channels: u8) {
    with_host(|host| {
        if !host.controller_manager.initialized {
            assert!(host.controller_manager.initialize());
        }
        host.controller_manager.set_channels(channels);
    });
}

fn complete_controller_poll(kind: ControllerPollKind, channels: u8) {
    initialize_controller_manager_for_test(channels);
    let queue = RdramAddr::from_offset(8);
    with_executor(|exec| {
        if exec.queue_activity(queue).is_none() {
            exec.create_mesg_queue(queue, 1);
        }
        exec.set_event_mesg(OS_EVENT_SI, queue, 0xCAFE);
    });
    start_controller_poll(queue, kind, usize::from(channels)).unwrap();
    crate::advance_virtual_time(crate::sim_time().saturating_add(1));
    assert_eq!(
        with_executor(|exec| exec.recv_mesg(999, queue, false)),
        fn64_runtime::RecvMesgOutcome::Delivered(0xCAFE)
    );
}

fn run_controller_init(
    rdram: &mut [u8],
    queue_vram: u64,
    bitpattern_vram: u64,
    status_vram: u64,
    thread: ThreadId,
) -> u64 {
    let queue = RdramAddr::from_gpr(queue_vram);
    with_executor(|exec| {
        exec.create_mesg_queue(queue, 1);
        exec.set_event_mesg(OS_EVENT_SI, queue, 0xCAFE);
    });
    unsafe { crate::register_process_rdram(rdram.as_mut_ptr(), rdram.len()) };
    let result = std::rc::Rc::new(std::cell::Cell::new(None));
    let thread_result = result.clone();
    let rdram_addr = rdram.as_mut_ptr() as usize;
    spawn_test_thread(thread, 10, move || {
        let mut init = ctx_zeroed();
        init.r4 = queue_vram;
        init.r5 = bitpattern_vram;
        init.r6 = status_vram;
        unsafe { osContInit_recomp(rdram_addr as *mut u8, &mut init) };
        thread_result.set(Some(init.r2));
    });
    assert!(crate::run_one_step());
    if result.get().is_none() {
        if let Some(deadline) = crate::next_device_deadline() {
            crate::advance_virtual_time(deadline);
        }
        crate::run_to_idle();
    }
    result
        .get()
        .expect("osContInit test coroutine did not resume after SI completion")
}

#[test]
fn os_cont_set_ch_limits_high_level_buffers_but_not_raw_channel_addressing() {
    reset_controller_manager();
    load_rom(vec![0; 0x100]);
    set_controller_port_state(1, fn64_runtime::PortState::StandardControllerNoPak);
    set_controller_state(1, 0x9000, -12, 34);

    let mut rdram = vec![0xA5; 0x200];
    initialize_controller_manager_for_test(4);

    let mut set_ch = ctx_zeroed();
    set_ch.r4 = 1;
    unsafe { osContSetCh_recomp(rdram.as_mut_ptr(), &mut set_ch) };
    assert_eq!(set_ch.r2, 0);

    rdram[0x40..0x50].fill(0xA5);
    complete_controller_poll(ControllerPollKind::Query, 1);
    let mut query = ctx_zeroed();
    query.r4 = 0x8000_0040;
    unsafe { osContGetQuery_recomp(rdram.as_mut_ptr(), &mut query) };
    assert!(rdram[0x40..0x44].iter().any(|&byte| byte != 0xA5));
    assert_eq!(&rdram[0x44..0x50], &[0xA5; 12]);

    rdram[0x60..0x78].fill(0xA5);
    complete_controller_poll(ControllerPollKind::Read, 1);
    let mut read = ctx_zeroed();
    read.r4 = 0x8000_0060;
    unsafe { osContGetReadData_recomp(rdram.as_mut_ptr(), &mut read) };
    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert!((0..6).any(|offset| { view.read_u8(RdramAddr::from_offset(0x60 + offset)) != 0xA5 }));
    assert!((6..24).all(|offset| { view.read_u8(RdramAddr::from_offset(0x60 + offset)) == 0xA5 }));

    // A leading zero advances an explicit Joybus packet to port 1. The
    // high-level manager's one-channel prefix must not hide that physical
    // port from raw PIF authority.
    let mut packet = [0u8; 64];
    packet[0] = 0;
    packet[1] = 1;
    packet[2] = 4;
    packet[3] = 0x01;
    packet[8] = 0xFE;
    crate::pi::with_pi_dma("raw port-1 read after osContSetCh(1)", |pi_dma| {
        execute_controller_pif(Cycles::ZERO, &mut packet, pi_dma)
    });
    assert_eq!(&packet[4..8], &[0x90, 0x00, 0xF4, 0x22]);

    assert_eq!(
        crate::host_evidence_snapshot().controller_manager,
        ControllerManagerEvidenceSnapshot {
            initialized: true,
            channels: 1,
        }
    );
}

#[test]
fn controller_manager_enforces_initialization_and_one_time_init() {
    reset_controller_manager();
    let mut rdram = vec![0xA5; 0x100];

    let mut set_before_init = ctx_zeroed();
    set_before_init.r4 = 1;
    unsafe { osContSetCh_recomp(rdram.as_mut_ptr(), &mut set_before_init) };
    assert_eq!(
        with_host(|host| host.controller_manager.evidence_snapshot()),
        ControllerManagerEvidenceSnapshot {
            initialized: false,
            channels: 4,
        }
    );

    assert_eq!(
        run_controller_init(&mut rdram, 0x8000_0008, 0x8000_0010, 0x8000_0020, 350,),
        0
    );

    let mut set_after_init = ctx_zeroed();
    set_after_init.r4 = 1;
    unsafe { osContSetCh_recomp(rdram.as_mut_ptr(), &mut set_after_init) };

    rdram[0x40..0x60].fill(0x6B);
    let mut repeated_init = ctx_zeroed();
    repeated_init.r4 = 0x8000_0008;
    repeated_init.r5 = 0x8000_0040;
    repeated_init.r6 = 0x8000_0050;
    unsafe { osContInit_recomp(rdram.as_mut_ptr(), &mut repeated_init) };
    assert_eq!(&rdram[0x40..0x60], &[0x6B; 32]);
    assert_eq!(
        with_host(|host| host.controller_manager.evidence_snapshot()),
        ControllerManagerEvidenceSnapshot {
            initialized: true,
            channels: 1,
        }
    );
}

#[test]
#[should_panic(expected = "osContSetCh: channel count 5 exceeds MAXCONTROLLERS (4)")]
fn controller_manager_traps_above_maxcontrollers() {
    let mut manager = ControllerManagerState::default();
    manager.initialize();
    manager.set_channels(5);
}

/// Regression for the OoT-boot `PadSetup_Init` EXC_BAD_ACCESS: the real
/// `osContGetQuery(OSContStatus* data)` takes its ONLY argument (the
/// array pointer) in `$a0`/`ctx.r4`; the buggy prior signature read it
/// from `$a1`/`ctx.r5`, which the real call site (`funcs_55.c:2193`)
/// leaves as stale garbage, so the shim dereferenced a wild pointer.
///
/// This test wires `r4` and `r5` to two DIFFERENT, both-valid rdram
/// addresses and asserts the OSContStatus array lands at `r4`'s address
/// (and that `r5`'s address is untouched) -- so reintroducing the bug
/// (reading the pointer from `r5`) makes it fail rather than pass. It
/// also checks all four ports are filled with the exact byte-swizzled
/// values the game's own MEM_HU/MEM_BU reads recover: port 0 a present
/// standard controller (`type == 0x0005 == CONT_TYPE_NORMAL`, `errno ==
/// 0`), ports 1-3 absent (`errno == CONT_NO_RESPONSE_ERROR == 0x08`).
#[test]
fn os_cont_get_query_reads_array_pointer_from_a0_and_fills_all_ports() {
    // Fresh PIF state (default: port 0 standard, 1-3 absent).
    reset_controller_manager();
    complete_controller_poll(ControllerPollKind::Query, 4);

    let mut buf = vec![0u8; fn64_runtime::RDRAM_MMIO_WINDOW_END as usize];

    // Two distinct, both-valid vram addresses. r4 = the REAL data
    // pointer the game passes; r5 = a decoy the buggy shim would have
    // used. Kept 0x40 apart so the 0x10-byte (4 * OSContStatus) write
    // regions can't overlap.
    let data_vram: u64 = 0xFFFF_FFFF_8020_0000;
    let decoy_vram: u64 = 0xFFFF_FFFF_8020_0040;
    let data_off = RdramAddr::from_gpr(data_vram).offset() as usize;
    let decoy_off = RdramAddr::from_gpr(decoy_vram).offset() as usize;

    // Pre-poison the decoy region with a sentinel so "untouched" is a
    // real, checkable statement, not "happened to already be zero".
    for i in 0..0x10 {
        buf[decoy_off + i] = 0xAB;
    }

    let mut ctx = ctx_zeroed();
    ctx.r4 = data_vram;
    ctx.r5 = decoy_vram;
    unsafe { osContGetQuery_recomp(buf.as_mut_ptr(), &mut ctx as *mut _) };

    // Read each OSContStatus exactly as the generated game code does:
    // MEM_HU(base, 0) = *(u16*)(rdram + (base ^ 2)); MEM_BU(base, 3) =
    // *(u8*)(rdram + ((base + 3) ^ 3)) (recomp.h). Reading through the
    // same swizzle the reader uses is what makes this a faithful check
    // rather than an encoding of whatever byte order the writer chose.
    let read_type = |base: usize| -> u16 {
        let a = base ^ 2;
        u16::from_ne_bytes([buf[a], buf[a + 1]])
    };
    let read_errno = |base: usize| -> u8 { buf[(base + 3) ^ 3] };

    // Port 0: present standard controller.
    let p0 = data_off;
    assert_eq!(
        read_type(p0),
        0x0005,
        "port 0 type must read as CONT_TYPE_NORMAL (0x0005) via the game's MEM_HU"
    );
    assert_eq!(read_errno(p0), 0, "port 0 (present) has no channel error");

    // Ports 1-3: absent -> non-zero errno so PadSetup_Init skips them.
    for port in 1..4usize {
        let base = data_off + port * 4;
        assert_eq!(
            read_errno(base),
            0x08,
            "absent port {port} must report CONT_NO_RESPONSE_ERROR (0x08)"
        );
    }

    // The decoy region (r5's address) must be completely untouched --
    // proves the pointer came from r4, not r5. Under the old bug this
    // region would have been written (and r4's region left as zeros).
    for i in 0..0x10 {
        assert_eq!(
            buf[decoy_off + i],
            0xAB,
            "byte {i} at the r5/decoy address was written -- the shim read \
                 its pointer from the wrong register (the reintroduced bug)"
        );
    }
}

/// The INPUT-SEAM contract: a host harness feeds controller state via
/// `set_controller_state`, and `osContGetReadData_recomp` writes it into
/// the game's `OSContPad[MAXCONTROLLERS]` array at `$a0`/`ctx.r4`, in the
/// exact byte-swizzled layout the game's own MEM_HU/MEM_BU reads recover.
///
/// Fail-against-the-bug: it reads every field back through the SAME
/// swizzle the recompiled game uses (`button` via MEM_HU `^2`, `stick`/
/// `errno` via MEM_BU `^3`, recomp.h:104-108). A flat/unswizzled copy (the
/// prior WIP) or a wrong button bit lands the bytes at the wrong lanes and
/// this fails. It also checks the button HIGH byte carries `BTN_START`
/// (0x1000) -- the scripted-boot press -- so an endianness flip fails too.
#[test]
fn os_cont_get_read_data_writes_swizzled_input_into_pad_array() {
    // Fresh state, then feed a distinctive input on port 0: Start+A held,
    // stick pushed. (BTN_A = 0x8000, BTN_START = 0x1000 -> 0x9000.)
    reset_controller_manager();
    set_controller_state(0, 0x9000, -50, 70);
    complete_controller_poll(ControllerPollKind::Read, 4);

    let mut buf = vec![0u8; fn64_runtime::RDRAM_MMIO_WINDOW_END as usize];
    let pad_vram: u64 = 0xFFFF_FFFF_8020_0000;
    let pad_off = RdramAddr::from_gpr(pad_vram).offset() as usize;

    let mut ctx = ctx_zeroed();
    ctx.r4 = pad_vram;
    unsafe { osContGetReadData_recomp(buf.as_mut_ptr(), &mut ctx as *mut _) };

    // Read each OSContPad field EXACTLY as the recompiled game does:
    // button via MEM_HU (`^2` halfword), the s8/u8 fields via MEM_BU
    // (`^3` byte). OSContPad size = 0x06 (controller.h:132).
    let read_button = |base: usize| -> u16 {
        let a = base ^ 2;
        u16::from_ne_bytes([buf[a], buf[a + 1]])
    };
    let read_i8 = |base: usize, o: usize| -> i8 { buf[(base + o) ^ 3] as i8 };
    let read_u8 = |base: usize, o: usize| -> u8 { buf[(base + o) ^ 3] };

    // Port 0: present -> errno 0 and the exact fed input.
    let p0 = pad_off;
    assert_eq!(
        read_button(p0),
        0x9000,
        "port 0 button must read back BTN_A|BTN_START (0x9000) via the game's MEM_HU"
    );
    assert_ne!(
        read_button(p0) & 0x1000,
        0,
        "BTN_START (0x1000) must be set -- the scripted press must reach the game"
    );
    assert_eq!(read_i8(p0, 2), -50, "stick_x");
    assert_eq!(read_i8(p0, 3), 70, "stick_y");
    assert_eq!(read_u8(p0, 4), 0, "port 0 (present) errno == 0");

    // Ports 1-3: absent -> nonzero errno so the game ignores them.
    for port in 1..MAXCONTROLLERS {
        let base = pad_off + port * 6;
        assert_eq!(
            read_u8(base, 4),
            CONT_NO_RESPONSE_ERROR,
            "absent port {port} errno must be CONT_NO_RESPONSE_ERROR (0x08)"
        );
        assert_eq!(read_button(base), 0, "absent port {port} button zeroed");
    }
}

#[test]
fn controller_read_is_unavailable_before_deadline_and_latched_after_completion() {
    reset_controller_manager();
    initialize_controller_manager_for_test(1);
    set_controller_state(0, 0x9000, -50, 70);
    let queue = RdramAddr::from_offset(8);
    with_executor(|exec| {
        exec.create_mesg_queue(queue, 1);
        exec.set_event_mesg(OS_EVENT_SI, queue, 0xCAFE);
    });
    start_controller_poll(queue, ControllerPollKind::Read, 1).unwrap();

    let pending_evidence = with_host(|host| host.device_fabric.evidence_snapshot());
    assert!(matches!(
        pending_evidence
            .pending_si
            .expect("controller read is pending in DeviceState evidence"),
        fn64_runtime::PendingSiSnapshot::Dma {
            request: fn64_runtime::SiDmaRequest {
                kind: fn64_runtime::SiDmaKind::ControllerRead,
                ..
            },
            ..
        }
    ));
    let staged = pending_evidence.pif_ram;
    let premature = std::panic::catch_unwind(|| {
        completed_controller_channels(&staged, ControllerPollKind::Read)
    })
    .expect_err("a live SI transaction must not expose controller data");
    let message = premature
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| premature.downcast_ref::<&str>().copied())
        .expect("premature Controller Manager read panic has context");
    assert!(message.contains("before the SI/PIF transaction completed"));

    crate::advance_virtual_time(1);
    // Input changes after byte commit belong to the next poll, not this
    // already-completed packet.
    set_controller_state(0, 0x4000, 12, -34);
    let mut rdram = vec![0xA5; 0x20];
    let mut get = ctx_zeroed();
    get.r4 = 0x8000_0010;
    unsafe { osContGetReadData_recomp(rdram.as_mut_ptr(), &mut get) };
    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(view.read_u16(RdramAddr::from_offset(0x10)), 0x9000);
    assert_eq!(view.read_u8(RdramAddr::from_offset(0x12)) as i8, -50);
    assert_eq!(view.read_u8(RdramAddr::from_offset(0x13)) as i8, 70);
}

#[test]
fn sustained_controller_reads_publish_each_poll_once_with_fresh_input() {
    reset_controller_manager();
    initialize_controller_manager_for_test(1);
    let queue = RdramAddr::from_offset(8);
    with_executor(|exec| {
        exec.create_mesg_queue(queue, 1);
        exec.set_event_mesg(OS_EVENT_SI, queue, 0xCAFE);
    });
    let mut rdram = vec![0u8; 0x40];

    for (buttons, stick_x, stick_y) in [(0x8000, 11, -12), (0x4000, -21, 22)] {
        set_controller_state(0, buttons, stick_x, stick_y);
        start_controller_poll(queue, ControllerPollKind::Read, 1).unwrap();
        crate::advance_virtual_time(crate::sim_time().saturating_add(1));
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(999, queue, false)),
            fn64_runtime::RecvMesgOutcome::Delivered(0xCAFE)
        );

        let mut get = ctx_zeroed();
        get.r4 = 0x8000_0020;
        unsafe { osContGetReadData_recomp(rdram.as_mut_ptr(), &mut get) };
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(view.read_u16(RdramAddr::from_offset(0x20)), buttons);
        assert_eq!(view.read_u8(RdramAddr::from_offset(0x22)) as i8, stick_x);
        assert_eq!(view.read_u8(RdramAddr::from_offset(0x23)) as i8, stick_y);
    }
}

#[test]
fn controller_read_latches_start_channel_prefix() {
    reset_controller_manager();
    initialize_controller_manager_for_test(1);
    set_controller_port_state(1, fn64_runtime::PortState::StandardControllerNoPak);
    set_controller_state(1, 0x8000, 5, 6);
    let queue = RdramAddr::from_offset(8);
    with_executor(|exec| {
        exec.create_mesg_queue(queue, 1);
        exec.set_event_mesg(OS_EVENT_SI, queue, 0xCAFE);
    });
    start_controller_poll(queue, ControllerPollKind::Read, 1).unwrap();
    with_host(|host| host.controller_manager.set_channels(2));
    crate::advance_virtual_time(1);

    let mut rdram = vec![0xA5; 0x30];
    let mut get = ctx_zeroed();
    get.r4 = 0x8000_0010;
    unsafe { osContGetReadData_recomp(rdram.as_mut_ptr(), &mut get) };
    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert!((0..6).any(|offset| view.read_u8(RdramAddr::from_offset(0x10 + offset)) != 0xA5));
    assert!((6..12).all(|offset| view.read_u8(RdramAddr::from_offset(0x10 + offset)) == 0xA5));
}

#[test]
fn high_level_input_evidence_excludes_voice_ports() {
    reset_controller_manager();
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);
    set_controller_port_state(1, fn64_runtime::PortState::StandardControllerNoPak);
    complete_controller_poll(ControllerPollKind::Read, 4);

    let mut rdram = vec![0u8; 0x40];
    let mut ctx = ctx_zeroed();
    ctx.r4 = 0x8000_0000;
    unsafe { osContGetReadData_recomp(rdram.as_mut_ptr(), &mut ctx) };

    assert_eq!(
        crate::copy_controller_operations(),
        vec![fn64_runtime::ControllerOperationEvent {
            at: Cycles::new(1),
            port: 1,
            device: fn64_runtime::ControllerOperationDevice::StandardController,
            operation: fn64_runtime::ControllerOperationKind::Read,
        }]
    );
}

/// `__osSiRawStartDma_recomp` is real this wave (replacing the prior
/// loud trap) -- verifies a port-0 status-query channel (tx_size=1,
/// rx_size=3) gets `PifModel::query_response(0)`'s real bytes written
/// back, and that an absent port (1) gets `CONT_ABSENT` set.
#[test]
fn os_si_raw_start_dma_fills_real_pif_query_responses() {
    let mut rdram = vec![0u8; 64];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        // Channel 0: tx_size=1, rx_size=3, cmd=0xFF (query), 1 tx byte,
        // then 3 response bytes to be filled at offset 3..6.
        view.write_u8(RdramAddr::from_offset(0), 1);
        view.write_u8(RdramAddr::from_offset(1), 3);
        view.write_u8(RdramAddr::from_offset(2), 0xFF);
        view.write_u8(RdramAddr::from_offset(3), 0);
        // Channel 1 starts after channel 0's three response bytes.
        view.write_u8(RdramAddr::from_offset(6), 1);
        view.write_u8(RdramAddr::from_offset(7), 3);
        view.write_u8(RdramAddr::from_offset(8), 0xFF);
        view.write_u8(RdramAddr::from_offset(9), 0);
        view.write_u8(RdramAddr::from_offset(12), 0xFE);
    }

    let mut ctx = ctx_zeroed();
    ctx.r4 = 1; // OS_WRITE: DRAM -> PIF, then execute the command block.
    ctx.r5 = 0x8000_0000; // dramAddr vram -> rdram offset 0
    unsafe { __osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
    assert_eq!(ctx.r2, 0);
    crate::advance_virtual_time(1);

    ctx.r4 = 0; // OS_READ: PIF -> DRAM response copy.
    unsafe { __osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
    crate::advance_virtual_time(2);

    // Port 0: standard controller, no pak, not absent.
    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(
        (3..6)
            .map(|offset| view.read_u8(RdramAddr::from_offset(offset)))
            .collect::<Vec<_>>(),
        vec![0x05, 0x00, 0x00]
    );
    // Port 1: absent bit set.
    assert_eq!(
        view.read_u8(RdramAddr::from_offset(11)) & fn64_runtime::CONT_ABSENT,
        fn64_runtime::CONT_ABSENT
    );
}

#[test]
fn raw_pif_records_operations_but_not_accessory_probes() {
    with_executor(|executor| *executor = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerNoPak);

    let mut input = [0u8; 64];
    input[0] = 1;
    input[1] = 4;
    input[2] = 0x01;
    input[7] = 0xfe;
    let input_observations = crate::pi::with_pi_dma("raw controller input", |pi_dma| {
        execute_controller_pif(Cycles::new(11), &mut input, pi_dma)
    });
    assert_eq!(
        input_observations.controller_operations,
        vec![fn64_runtime::ControllerOperationEvent {
            at: Cycles::new(11),
            port: 0,
            device: fn64_runtime::ControllerOperationDevice::StandardController,
            operation: fn64_runtime::ControllerOperationKind::Read,
        }]
    );

    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerRumblePak);
    let encoded_probe =
        ACCESSORY_ADDR_RUMBLE_PROBE | u16::from(accessory_address_crc(ACCESSORY_ADDR_RUMBLE_PROBE));
    let mut probe = [0u8; 64];
    probe[0] = 3;
    probe[1] = 33;
    probe[2] = 0x02;
    probe[3..5].copy_from_slice(&encoded_probe.to_be_bytes());
    probe[38] = 0xfe;
    let probe_observations = crate::pi::with_pi_dma("raw Rumble Pak probe", |pi_dma| {
        execute_controller_pif(Cycles::new(12), &mut probe, pi_dma)
    });
    assert!(probe_observations.controller_operations.is_empty());

    let encoded_motor =
        ACCESSORY_ADDR_RUMBLE_MOTOR | u16::from(accessory_address_crc(ACCESSORY_ADDR_RUMBLE_MOTOR));
    let mut motor = [0u8; 64];
    motor[0] = 35;
    motor[1] = 1;
    motor[2] = 0x03;
    motor[3..5].copy_from_slice(&encoded_motor.to_be_bytes());
    motor[5..37].fill(1);
    motor[38] = 0xfe;
    let motor_observations = crate::pi::with_pi_dma("raw Rumble Pak motor", |pi_dma| {
        execute_controller_pif(Cycles::new(13), &mut motor, pi_dma)
    });
    assert_eq!(
        motor_observations.controller_operations,
        vec![fn64_runtime::ControllerOperationEvent {
            at: Cycles::new(13),
            port: 0,
            device: fn64_runtime::ControllerOperationDevice::RumblePak,
            operation: fn64_runtime::ControllerOperationKind::Control,
        }]
    );

    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
    let mut gb_rom = vec![0xff; 0x8000];
    gb_rom[0x147] = 0;
    insert_transfer_pak_cartridge(0, gb_rom, None).unwrap();
    let mut transfer = [0u8; 64];
    transfer[0] = 3;
    transfer[1] = 33;
    transfer[2] = 0x02;
    transfer[3..5].copy_from_slice(&encoded_probe.to_be_bytes());
    transfer[38] = 0xfe;
    let transfer_observations = crate::pi::with_pi_dma("raw Transfer Pak read", |pi_dma| {
        execute_controller_pif(Cycles::new(14), &mut transfer, pi_dma)
    });
    assert_eq!(
        transfer_observations.controller_operations,
        vec![fn64_runtime::ControllerOperationEvent {
            at: Cycles::new(14),
            port: 0,
            device: fn64_runtime::ControllerOperationDevice::TransferPak,
            operation: fn64_runtime::ControllerOperationKind::Read,
        }]
    );
}

#[test]
fn raw_voice_info_and_high_level_init_share_readiness_state() {
    with_executor(|exec| *exec = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);

    let query = || {
        let mut packet = [0u8; 64];
        packet[0] = 1;
        packet[1] = 3;
        packet[2] = 0x00;
        packet[6] = 0xFE;
        packet
    };

    let mut before = query();
    crate::pi::with_pi_dma("raw Voice pre-init Info", |pi_dma| {
        execute_controller_pif(Cycles::ZERO, &mut before, pi_dma)
    });
    assert_eq!(&before[3..6], &[0x00, 0x01, 0x00]);

    let mut rdram = vec![0u8; 0x100];
    let mut init = ctx_zeroed();
    init.r4 = 0x8000_0020;
    init.r5 = 0x8000_0040;
    init.r6 = 0;
    unsafe { crate::voice::osVoiceInit_recomp(rdram.as_mut_ptr(), &mut init) };
    assert_eq!(init.r2, 0);

    let mut after = query();
    crate::pi::with_pi_dma("raw Voice post-init Info", |pi_dma| {
        execute_controller_pif(Cycles::ZERO, &mut after, pi_dma)
    });
    assert_eq!(&after[3..6], &[0x00, 0x01, 0x01]);
}

#[test]
fn raw_voice_captured_initialization_sequence_reaches_shared_readiness() {
    with_executor(|exec| *exec = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);

    let run = |packet: &mut [u8; 64]| {
        crate::pi::with_pi_dma("raw Voice initialization", |pi_dma| {
            execute_controller_pif(Cycles::new(17), packet, pi_dma)
        });
    };
    for encoded_address in [0x1E0Cu16, 0x6E07, 0x080E, 0x5618, 0x030F] {
        let mut packet = [0u8; 64];
        packet[0] = 3;
        packet[1] = 1;
        packet[2] = 0x0D;
        packet[3..5].copy_from_slice(&encoded_address.to_be_bytes());
        packet[6] = 0xFE;
        run(&mut packet);
        assert_eq!(packet[5], 0);
    }
    assert_eq!(
        with_executor(|exec| exec
            .voice_unit(0)
            .unwrap()
            .evidence_snapshot()
            .raw_init_step),
        5
    );

    let mut finish = [0u8; 64];
    finish[0] = 7;
    finish[1] = 1;
    finish[2] = 0x0C;
    finish[5..9].copy_from_slice(&[0x00, 0x00, 0x01, 0x00]);
    finish[10] = 0xFE;
    run(&mut finish);
    assert_eq!(finish[9], 0x97);
    assert!(with_executor(|exec| exec
        .voice_unit(0)
        .unwrap()
        .initialized()));
}

#[test]
fn joybus_crc_matches_public_voice_capture_vectors() {
    assert_eq!(joybus_data_crc(&[0x00, 0x00, 0x01, 0x00]), 0x97);
    assert_eq!(joybus_data_crc(&[0x02, 0x00, 0x3B, 0x00]), 0xF9);
    assert_eq!(joybus_data_crc(&[0x05, 0x00, 0x00, 0x00]), 0x4E);
    assert_eq!(joybus_data_crc(&[0x00, 0x00, 0x06, 0x00]), 0x78);
    assert_eq!(joybus_data_crc(&[0x01, 0x00]), 0x97);
    assert_eq!(joybus_data_crc(&[0x05, 0x00]), 0x44);
}

#[test]
fn raw_voice_status_clear_and_start_share_high_level_state() {
    with_executor(|exec| *exec = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);

    let run = |packet: &mut [u8; 64]| {
        crate::pi::with_pi_dma("raw Voice state convergence", |pi_dma| {
            execute_controller_pif(Cycles::new(23), packet, pi_dma)
        });
    };
    let status_packet = || {
        let mut packet = [0u8; 64];
        packet[0] = 3;
        packet[1] = 3;
        packet[2] = 0x0B;
        packet[8] = 0xFE;
        packet
    };

    let mut pre_init = status_packet();
    run(&mut pre_init);
    assert_eq!(&pre_init[5..8], &[0x01, 0x00, 0x97]);

    let mut rdram = vec![0u8; 0x100];
    let mut init = ctx_zeroed();
    init.r4 = 0x8000_0020;
    init.r5 = 0x8000_0040;
    init.r6 = 0;
    unsafe { crate::voice::osVoiceInit_recomp(rdram.as_mut_ptr(), &mut init) };
    assert_eq!(init.r2, 0);

    let mut ready = status_packet();
    run(&mut ready);
    assert_eq!(&ready[5..8], &[0x00, 0x00, 0x00]);

    let mut clear = [0u8; 64];
    clear[0] = 7;
    clear[1] = 1;
    clear[2] = 0x0C;
    clear[5..9].copy_from_slice(&[0x02, 0x00, 0x01, 0x00]);
    clear[10] = 0xFE;
    run(&mut clear);
    assert_eq!(clear[9], joybus_data_crc(&[0x02, 0x00, 0x01, 0x00]));
    assert_eq!(
        with_executor(|exec| exec
            .voice_unit(0)
            .unwrap()
            .evidence_snapshot()
            .expected_words),
        Some(1)
    );

    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
    for (index, byte) in b"voice\0".iter().copied().enumerate() {
        unsafe {
            storage.write_u8(RdramAddr::from_offset(0x80 + index as u32), byte);
        }
    }
    let mut word = ctx_zeroed();
    word.r4 = 0x8000_0040;
    word.r5 = 0x8000_0080;
    unsafe { crate::voice::osVoiceSetWord_recomp(rdram.as_mut_ptr(), &mut word) };
    assert_eq!(word.r2, 0);

    let mut start = [0u8; 64];
    start[0] = 7;
    start[1] = 1;
    start[2] = 0x0C;
    start[5..9].copy_from_slice(&[0x00, 0x00, 0x06, 0x00]);
    start[10] = 0xFE;
    run(&mut start);
    assert_eq!(start[9], 0x78);

    let mut get = ctx_zeroed();
    get.r4 = 0x8000_0040;
    get.r5 = 0x8000_00C0;
    unsafe { crate::voice::osVoiceGetReadData_recomp(rdram.as_mut_ptr(), &mut get) };
    assert_ne!(get.r2, 0);
    assert_eq!(
        unsafe { storage.read_u8(RdramAddr::from_offset(0x4C)) },
        fn64_runtime::voice::VOICE_STATUS_START
    );

    let mut started = status_packet();
    run(&mut started);
    assert_eq!(&started[5..8], &[0x01, 0x00, 0x97]);

    crate::voice::mark_voice_detected(0);
    let mut busy = status_packet();
    run(&mut busy);
    assert_eq!(&busy[5..8], &[0x05, 0x00, 0x44]);

    crate::voice::inject_voice_result(0, fn64_runtime::VoiceData::default());
    let mut ended = status_packet();
    run(&mut ended);
    assert_eq!(&ended[5..8], &[0x07, 0x00, joybus_data_crc(&[0x07, 0x00])]);

    let mut stop = [0u8; 64];
    stop[0] = 7;
    stop[1] = 1;
    stop[2] = 0x0C;
    stop[5..9].copy_from_slice(&[0x05, 0x00, 0x00, 0x00]);
    stop[10] = 0xFE;
    run(&mut stop);
    assert_eq!(stop[9], 0x4E);
    let mut canceled = status_packet();
    run(&mut canceled);
    assert_eq!(
        &canceled[5..8],
        &[0x03, 0x00, joybus_data_crc(&[0x03, 0x00])]
    );
}

#[test]
fn raw_voice_result_matches_public_capture_layout_and_consumes_shared_result() {
    with_executor(|exec| *exec = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);
    with_executor(|exec| {
        let voice = exec.voice_unit_mut(0).unwrap();
        voice.initialize();
        voice.clear_dictionary(1).unwrap();
        voice.set_word(b"capture").unwrap();
        voice.start().unwrap();
        voice
            .inject_result(fn64_runtime::VoiceData {
                warning: 0,
                answer_num: 2,
                voice_level: 0x059D,
                voice_sn: 0x077C,
                voice_time: 0x04B0,
                answer: [0, 4, 0x10, 0x14, 5],
                distance: [0x0477, 0x04CC, 0x04F9, 0x0503, 0x0512],
            })
            .unwrap();
    });

    let mut packet = [0u8; 64];
    packet[0] = 3;
    packet[1] = 37;
    packet[2] = 0x09;
    packet[42] = 0xFE;
    let observations = crate::pi::with_pi_dma("raw Voice result", |pi_dma| {
        execute_controller_pif(Cycles::new(29), &mut packet, pi_dma)
    });
    assert_eq!(
        observations.controller_operations,
        vec![fn64_runtime::ControllerOperationEvent {
            at: Cycles::new(29),
            port: 0,
            device: fn64_runtime::ControllerOperationDevice::VoiceRecognitionUnit,
            operation: fn64_runtime::ControllerOperationKind::Read,
        }]
    );
    assert_eq!(
        &packet[5..42],
        &[
            0x80, 0x00, 0x0F, 0x00, 0x00, 0x00, 0x02, 0x00, 0x9D, 0x05, 0x7C, 0x07, 0xB0, 0x04,
            0x00, 0x00, 0x77, 0x04, 0x04, 0x00, 0xCC, 0x04, 0x10, 0x00, 0xF9, 0x04, 0x14, 0x00,
            0x03, 0x05, 0x05, 0x00, 0x12, 0x05, 0x40, 0x00, 0x97,
        ]
    );
    assert_eq!(
        with_executor(|exec| exec.voice_unit(0).unwrap().status()),
        fn64_runtime::voice::VOICE_STATUS_READY
    );
}

#[test]
fn unestablished_raw_voice_payload_records_a_typed_loud_trap() {
    with_executor(|exec| *exec = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);
    fn64_runtime::arm_unsupported_events(None).unwrap();

    let mut packet = [0u8; 64];
    packet[0] = 7;
    packet[1] = 1;
    packet[2] = 0x0C;
    packet[5..9].copy_from_slice(&[0x00, 0x00, 0x07, 0x00]);
    packet[10] = 0xFE;
    let trapped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::pi::with_pi_dma("unsupported raw Voice payload", |pi_dma| {
            execute_controller_pif(Cycles::new(41), &mut packet, pi_dma)
        });
    }));
    assert!(trapped.is_err());

    let events = fn64_runtime::copy_unsupported_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].subsystem, fn64_runtime::UnsupportedSubsystem::Abi);
    assert_eq!(events[0].operation, "abi.si.voice-command-0c");
    assert_eq!(events[0].guest_cycle, Some(Cycles::new(41)));
    assert_eq!(
        events[0].disposition,
        fn64_runtime::UnsupportedDisposition::LoudTrap
    );
    assert!(events[0].context.contains("00 00 07 00"));
    fn64_runtime::complete_unsupported_observation(Cycles::new(41), &"0".repeat(64));
}

#[test]
fn voice_accessory_read_and_write_record_typed_loud_traps() {
    let cases = [
        (
            0x02,
            3,
            33,
            "abi.si.voice-command-02",
            "standard accessory-read packet",
        ),
        (
            0x03,
            35,
            1,
            "abi.si.voice-command-03",
            "standard accessory-write packet",
        ),
    ];

    for (command, tx_size, rx_size, operation, detail) in cases {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);
        fn64_runtime::arm_unsupported_events(None).unwrap();

        let mut packet = [0u8; 64];
        packet[0] = tx_size;
        packet[1] = rx_size;
        packet[2] = command;
        // Address zero has the public accessory-address CRC zero. The
        // write case's remaining 32 transmit bytes are already zero.
        let next = 2 + usize::from(tx_size) + usize::from(rx_size);
        packet[next] = 0xFE;
        let cycle = Cycles::new(50 + u64::from(command));
        let trapped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::pi::with_pi_dma("unsupported Voice accessory packet", |pi_dma| {
                execute_controller_pif(cycle, &mut packet, pi_dma)
            });
        }));
        assert!(trapped.is_err());

        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].subsystem, fn64_runtime::UnsupportedSubsystem::Abi);
        assert_eq!(events[0].operation, operation);
        assert_eq!(events[0].guest_cycle, Some(cycle));
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        assert!(events[0]
            .context
            .contains(&format!("command {command:#04x}")));
        assert!(events[0].context.contains("channel 0"));
        assert!(events[0].context.contains(detail));
        fn64_runtime::complete_unsupported_observation(cycle, &"1".repeat(64));
    }
}

#[test]
fn unknown_raw_pif_command_records_packet_shape_before_loud_trap() {
    with_executor(|exec| *exec = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    fn64_runtime::arm_unsupported_events(None).unwrap();

    let mut packet = [0u8; 64];
    packet[0] = 1;
    packet[1] = 2;
    packet[2] = 0x7E;
    packet[5] = 0xFE;
    let cycle = Cycles::new(73);
    let trapped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::pi::with_pi_dma("unsupported generic PIF packet", |pi_dma| {
            execute_controller_pif(cycle, &mut packet, pi_dma)
        });
    }));
    assert!(trapped.is_err());

    let events = fn64_runtime::copy_unsupported_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].subsystem, fn64_runtime::UnsupportedSubsystem::Abi);
    assert_eq!(events[0].operation, "abi.si.pif-command-7e");
    assert_eq!(events[0].guest_cycle, Some(cycle));
    assert_eq!(
        events[0].disposition,
        fn64_runtime::UnsupportedDisposition::LoudTrap
    );
    assert_eq!(
        events[0].context,
        "SI PIF command 0x7e on channel 0 with tx=1 rx=2 is not implemented"
    );
    fn64_runtime::complete_unsupported_observation(cycle, &"2".repeat(64));
}

#[test]
fn raw_eeprom_and_high_level_shims_share_one_backing_store() {
    install_eeprom(fn64_runtime::SaveType::Eeprom4k);
    let mut rdram = vec![0u8; 0x200];
    let raw_payload = [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87];

    let mut raw_write = vec![0; 18];
    raw_write[4] = 10;
    raw_write[5] = 1;
    raw_write[6] = 0x05;
    raw_write[7] = 7;
    raw_write[8..16].copy_from_slice(&raw_payload);
    raw_write[17] = 0xFE;
    write_logical_bytes(&mut rdram, 0, &raw_write);
    raw_si_round_trip(&mut rdram);
    assert_eq!(read_logical_bytes(&rdram, 16, 1), vec![0]);

    let mut high_read = ctx_zeroed();
    high_read.r5 = 7;
    high_read.r6 = 0x8000_0080;
    unsafe { crate::save::osEepromRead_recomp(rdram.as_mut_ptr(), &mut high_read) };
    assert_eq!(high_read.r2, 0);
    assert_eq!(read_logical_bytes(&rdram, 0x80, 8), raw_payload);

    let shim_payload = [0xF8, 0xE7, 0xD6, 0xC5, 0xB4, 0xA3, 0x92, 0x81];
    write_logical_bytes(&mut rdram, 0xA0, &shim_payload);
    let mut high_write = ctx_zeroed();
    high_write.r5 = 7;
    high_write.r6 = 0x8000_00A0;
    unsafe { crate::save::osEepromWrite_recomp(rdram.as_mut_ptr(), &mut high_write) };
    assert_eq!(high_write.r2, 0);
    crate::advance_virtual_time(
        crate::sim_time().saturating_add(fn64_runtime::EEPROM_WRITE_CYCLES.get()),
    );

    let mut raw_read = vec![0; 17];
    raw_read[4] = 2;
    raw_read[5] = 8;
    raw_read[6] = 0x04;
    raw_read[7] = 7;
    raw_read[16] = 0xFE;
    write_logical_bytes(&mut rdram, 0, &raw_read);
    raw_si_round_trip(&mut rdram);
    assert_eq!(read_logical_bytes(&rdram, 8, 8), shim_payload);
    let operations = crate::copy_save_operations();
    assert_eq!(operations.len(), 4);
    assert_eq!(
        operations
            .iter()
            .map(|event| event.operation)
            .collect::<Vec<_>>(),
        vec![
            fn64_runtime::SaveOperationKind::Write,
            fn64_runtime::SaveOperationKind::Read,
            fn64_runtime::SaveOperationKind::Write,
            fn64_runtime::SaveOperationKind::Read,
        ]
    );
    assert!(operations.iter().all(|event| {
        event.device == fn64_runtime::SaveType::Eeprom4k
            && event.offset == 7 * EEPROM_BLOCK_BYTES as u32
            && event.len == EEPROM_BLOCK_BYTES as u32
    }));
}

#[test]
fn same_cycle_eeprom_maturity_pfs_and_eeprom_read_keep_wire_order() {
    install_eeprom(fn64_runtime::SaveType::Eeprom4k);
    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerControllerPak);
    let deadline = crate::pi::with_pi_dma("same-cycle raw save ordering", |pi_dma| {
        pi_dma
            .start_eeprom_write(Cycles::ZERO, 1, [0x5a; EEPROM_BLOCK_BYTES])
            .unwrap()
    });
    crate::advance_virtual_time(deadline.get() - 1);

    let mut packet = [0u8; 64];
    let pak_address = 0u16;
    let encoded = pak_address | u16::from(accessory_address_crc(pak_address));
    packet[0] = 3;
    packet[1] = 33;
    packet[2] = 0x02;
    packet[3..5].copy_from_slice(&encoded.to_be_bytes());
    packet[38..41].fill(0);
    packet[41] = 2;
    packet[42] = 8;
    packet[43] = 0x04;
    packet[44] = 1;
    packet[53] = 0xfe;

    let mut rdram = vec![0u8; 64];
    write_logical_bytes(&mut rdram, 0, &packet);
    raw_si_round_trip(&mut rdram);

    assert_eq!(
        crate::copy_save_operations(),
        vec![
            fn64_runtime::SaveOperationEvent {
                at: deadline,
                device: fn64_runtime::SaveType::Eeprom4k,
                operation: fn64_runtime::SaveOperationKind::Write,
                offset: EEPROM_BLOCK_BYTES as u32,
                len: EEPROM_BLOCK_BYTES as u32,
            },
            fn64_runtime::SaveOperationEvent {
                at: deadline,
                device: fn64_runtime::SaveType::ControllerPak,
                operation: fn64_runtime::SaveOperationKind::Read,
                offset: 0,
                len: ACCESSORY_BLOCK_BYTES as u32,
            },
            fn64_runtime::SaveOperationEvent {
                at: deadline,
                device: fn64_runtime::SaveType::Eeprom4k,
                operation: fn64_runtime::SaveOperationKind::Read,
                offset: EEPROM_BLOCK_BYTES as u32,
                len: EEPROM_BLOCK_BYTES as u32,
            },
        ]
    );
}

#[test]
fn raw_eeprom_query_distinguishes_devices_and_reports_no_response() {
    fn query(kind: fn64_runtime::SaveType) -> ([u8; 3], u8) {
        install_eeprom(kind);
        let mut packet = [0u8; 64];
        packet[4] = 1;
        packet[5] = 3;
        packet[6] = 0;
        packet[10] = 0xFE;
        crate::pi::with_pi_dma("raw EEPROM query test", |pi_dma| {
            execute_controller_pif(Cycles::ZERO, &mut packet, pi_dma)
        });
        (packet[7..10].try_into().unwrap(), packet[5])
    }

    assert_eq!(
        query(fn64_runtime::SaveType::Eeprom4k),
        ([0x00, 0x80, 0x00], 3)
    );
    assert_eq!(
        query(fn64_runtime::SaveType::Eeprom16k),
        ([0x00, 0xC0, 0x00], 3)
    );
    assert_eq!(
        query(fn64_runtime::SaveType::SramBanked),
        ([0x00, 0x00, 0x00], 3 | PIF_CHANNEL_NO_RESPONSE)
    );
}

#[test]
fn raw_eeprom_busy_status_rejects_overlap_and_clears_at_deadline() {
    install_eeprom(fn64_runtime::SaveType::Eeprom4k);
    let first = [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87];
    let second = [0xA5; EEPROM_BLOCK_BYTES];

    let write_packet = |payload: [u8; EEPROM_BLOCK_BYTES]| {
        let mut packet = [0u8; 64];
        packet[4] = 10;
        packet[5] = 1;
        packet[6] = 0x05;
        packet[7] = 0xC9;
        packet[8..16].copy_from_slice(&payload);
        packet[17] = 0xFE;
        packet
    };
    let query_packet = || {
        let mut packet = [0u8; 64];
        packet[4] = 1;
        packet[5] = 3;
        packet[6] = 0;
        packet[10] = 0xFE;
        packet
    };

    let start = Cycles::new(50);
    let deadline = start
        .checked_add(fn64_runtime::EEPROM_WRITE_CYCLES)
        .unwrap();
    let mut write = write_packet(first);
    crate::pi::with_pi_dma("raw EEPROM timed write", |pi_dma| {
        execute_controller_pif(start, &mut write, pi_dma)
    });
    assert_eq!(write[16], 0);

    let mut busy_query = query_packet();
    crate::pi::with_pi_dma("raw EEPROM busy query", |pi_dma| {
        execute_controller_pif(start, &mut busy_query, pi_dma)
    });
    assert_eq!(&busy_query[7..10], &[0x00, 0x80, 0x80]);

    let mut overlap = write_packet(second);
    crate::pi::with_pi_dma("raw EEPROM overlapping write", |pi_dma| {
        execute_controller_pif(Cycles::new(deadline.get() - 1), &mut overlap, pi_dma)
    });
    assert_eq!(overlap[16], 0x80);

    let mut ready_query = query_packet();
    crate::pi::with_pi_dma("raw EEPROM deadline query", |pi_dma| {
        execute_controller_pif(deadline, &mut ready_query, pi_dma);
        let mut stored = [0; EEPROM_BLOCK_BYTES];
        pi_dma.save_read_into(9 * EEPROM_BLOCK_BYTES, &mut stored);
        assert_eq!(stored, first);
    });
    assert_eq!(&ready_query[7..10], &[0x00, 0x80, 0x00]);
}

#[test]
fn malformed_raw_eeprom_packet_traps_with_protocol_context() {
    install_eeprom(fn64_runtime::SaveType::Eeprom4k);
    let mut packet = [0u8; 64];
    packet[4] = 1;
    packet[5] = 8;
    packet[6] = 0x04;
    packet[15] = 0xFE;
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::pi::with_pi_dma("malformed raw EEPROM test", |pi_dma| {
            execute_controller_pif(Cycles::ZERO, &mut packet, pi_dma)
        });
    }))
    .expect_err("wrong EEPROM packet shape must trap");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("panic must carry protocol context");
    assert!(message.contains("command 0x04 on channel 4"), "{message}");
    assert!(message.contains("expected tx=2 rx=8"), "{message}");
}

#[test]
fn public_accessory_crc_vectors_match_rumble_and_block_addresses() {
    assert_eq!(accessory_address_crc(0x0020), 0x15);
    assert_eq!(accessory_address_crc(ACCESSORY_ADDR_RUMBLE_PROBE), 0x01);
    assert_eq!(accessory_address_crc(ACCESSORY_ADDR_RUMBLE_MOTOR), 0x1B);
    assert_eq!(accessory_data_crc(&[0; ACCESSORY_BLOCK_BYTES]), 0x00);
    assert_eq!(accessory_data_crc(&[1; ACCESSORY_BLOCK_BYTES]), 0xEB);
    assert_eq!(accessory_data_crc(&[0x80; ACCESSORY_BLOCK_BYTES]), 0xB8);
}

#[test]
fn raw_rumble_probe_and_write_share_the_high_level_motor_latch() {
    with_executor(|executor| *executor = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerRumblePak);
    let mut rdram = vec![0u8; 0x200];

    let mut init = ctx_zeroed();
    init.r5 = 0x8000_0100;
    init.r6 = 0;
    unsafe { osMotorInit_recomp(rdram.as_mut_ptr(), &mut init) };
    assert_eq!(init.r2, 0);
    assert!(!rumble_active(0));

    let mut raw_write = vec![0; 39];
    raw_write[0] = 35;
    raw_write[1] = 1;
    raw_write[2] = 0x03;
    raw_write[3..5].copy_from_slice(&0xC01Bu16.to_be_bytes());
    raw_write[5..37].fill(1);
    raw_write[38] = 0xFE;
    write_logical_bytes(&mut rdram, 0, &raw_write);
    raw_si_round_trip(&mut rdram);
    assert_eq!(read_logical_bytes(&rdram, 37, 1), vec![0xEB]);
    assert!(rumble_active(0));

    let mut stop = ctx_zeroed();
    stop.r4 = 0x8000_0100;
    unsafe { osMotorStop_recomp(rdram.as_mut_ptr(), &mut stop) };
    assert_eq!(stop.r2, 0);
    assert!(!rumble_active(0));

    let mut raw_probe = vec![0; 39];
    raw_probe[0] = 3;
    raw_probe[1] = 33;
    raw_probe[2] = 0x02;
    raw_probe[3..5].copy_from_slice(&0x8001u16.to_be_bytes());
    raw_probe[38] = 0xFE;
    write_logical_bytes(&mut rdram, 0, &raw_probe);
    raw_si_round_trip(&mut rdram);
    assert_eq!(read_logical_bytes(&rdram, 5, 32), vec![0x80; 32]);
    assert_eq!(read_logical_bytes(&rdram, 37, 1), vec![0xB8]);
    assert!(!rumble_active(0), "probe reads must not energize the motor");
    assert_eq!(
        crate::copy_controller_operations(),
        vec![
            fn64_runtime::ControllerOperationEvent {
                at: Cycles::new(1),
                port: 0,
                device: fn64_runtime::ControllerOperationDevice::RumblePak,
                operation: fn64_runtime::ControllerOperationKind::Control,
            },
            fn64_runtime::ControllerOperationEvent {
                at: Cycles::new(2),
                port: 0,
                device: fn64_runtime::ControllerOperationDevice::RumblePak,
                operation: fn64_runtime::ControllerOperationKind::Control,
            },
        ]
    );
}

#[test]
fn raw_controller_pak_blocks_and_high_level_files_share_data_pages() {
    with_executor(|executor| *executor = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerControllerPak);
    let key = fn64_runtime::PfsKey {
        company_code: 0x1234,
        game_code: 0x4142_4344,
        game_name: [0x21; 16],
        ext_name: [0x11; 4],
    };
    with_executor(|executor| {
        assert_eq!(
            executor
                .controller_pak_mut(0)
                .expect("configured Controller Pak")
                .allocate(key, fn64_runtime::pfs::PFS_PAGE_SIZE),
            Ok(0)
        );
    });
    let mut rdram = vec![0u8; 0x200];
    let raw_payload = [0x5A; ACCESSORY_BLOCK_BYTES];
    let first_data_address =
        (fn64_runtime::pfs::PFS_MANAGEMENT_PAGES * fn64_runtime::pfs::PFS_PAGE_SIZE) as u16;
    let encoded_address = first_data_address | u16::from(accessory_address_crc(first_data_address));
    let mut raw_write = vec![0; 39];
    raw_write[0] = 35;
    raw_write[1] = 1;
    raw_write[2] = 0x03;
    raw_write[3..5].copy_from_slice(&encoded_address.to_be_bytes());
    raw_write[5..37].copy_from_slice(&raw_payload);
    raw_write[38] = 0xFE;
    write_logical_bytes(&mut rdram, 0, &raw_write);
    raw_si_round_trip(&mut rdram);
    assert_eq!(
        read_logical_bytes(&rdram, 37, 1),
        vec![accessory_data_crc(&raw_payload)]
    );
    with_executor(|executor| {
        let mut semantic = [0; ACCESSORY_BLOCK_BYTES];
        executor
            .controller_pak(0)
            .expect("configured Controller Pak")
            .read(0, 0, &mut semantic)
            .unwrap();
        assert_eq!(semantic, raw_payload);
    });

    let semantic_payload = [0xA5; ACCESSORY_BLOCK_BYTES];
    with_executor(|executor| {
        executor
            .controller_pak_mut(0)
            .expect("configured Controller Pak")
            .write(0, ACCESSORY_BLOCK_BYTES, &semantic_payload)
            .unwrap();
    });
    let second_block = first_data_address + ACCESSORY_BLOCK_BYTES as u16;
    let encoded_second = second_block | u16::from(accessory_address_crc(second_block));
    let mut raw_read = vec![0; 39];
    raw_read[0] = 3;
    raw_read[1] = 33;
    raw_read[2] = 0x02;
    raw_read[3..5].copy_from_slice(&encoded_second.to_be_bytes());
    raw_read[38] = 0xFE;
    write_logical_bytes(&mut rdram, 0, &raw_read);
    raw_si_round_trip(&mut rdram);
    assert_eq!(read_logical_bytes(&rdram, 5, 32), semantic_payload);
    assert_eq!(
        read_logical_bytes(&rdram, 37, 1),
        vec![accessory_data_crc(&semantic_payload)]
    );
    assert_eq!(
        crate::copy_save_operations(),
        vec![
            fn64_runtime::SaveOperationEvent {
                at: Cycles::new(1),
                device: fn64_runtime::SaveType::ControllerPak,
                operation: fn64_runtime::SaveOperationKind::Write,
                offset: u32::from(first_data_address),
                len: ACCESSORY_BLOCK_BYTES as u32,
            },
            fn64_runtime::SaveOperationEvent {
                at: Cycles::new(3),
                device: fn64_runtime::SaveType::ControllerPak,
                operation: fn64_runtime::SaveOperationKind::Read,
                offset: u32::from(second_block),
                len: ACCESSORY_BLOCK_BYTES as u32,
            },
        ]
    );
}

#[test]
fn raw_controller_pak_bank_select_reaches_high_level_cross_bank_data() {
    with_executor(|executor| *executor = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_pak_bank_count(0, fn64_runtime::ControllerPakBankCount::new(2).unwrap());
    let key = fn64_runtime::PfsKey {
        company_code: 0x1234,
        game_code: 0x4241_4e4b,
        game_name: [0x22; 16],
        ext_name: [0x12; 4],
    };
    let payload = [0x6c; ACCESSORY_BLOCK_BYTES];
    with_executor(|executor| {
        let pak = executor.controller_pak_mut(0).unwrap();
        let file = pak
            .allocate(key, 122 * fn64_runtime::pfs::PFS_PAGE_SIZE)
            .unwrap();
        pak.write(file, 121 * fn64_runtime::pfs::PFS_PAGE_SIZE, &payload)
            .unwrap();
    });

    let encoded_select = ACCESSORY_ADDR_RUMBLE_PROBE | u16::from(accessory_address_crc(0x8000));
    let mut select = [0u8; 64];
    select[0] = 35;
    select[1] = 1;
    select[2] = 0x03;
    select[3..5].copy_from_slice(&encoded_select.to_be_bytes());
    select[5..37].fill(1);
    select[38] = 0xfe;
    let select_operations = crate::pi::with_pi_dma("raw Controller Pak bank select", |pi_dma| {
        execute_controller_pif(Cycles::ZERO, &mut select, pi_dma)
    });
    assert!(select_operations.save_operations.is_empty());
    assert!(select_operations.controller_operations.is_empty());
    assert_eq!(select[37], accessory_data_crc(&[1; ACCESSORY_BLOCK_BYTES]));

    let address = fn64_runtime::pfs::PFS_PAGE_SIZE as u16;
    let encoded = address | u16::from(accessory_address_crc(address));
    let mut read = [0u8; 64];
    read[0] = 3;
    read[1] = 33;
    read[2] = 0x02;
    read[3..5].copy_from_slice(&encoded.to_be_bytes());
    read[38] = 0xfe;
    let read_operations = crate::pi::with_pi_dma("raw Controller Pak banked read", |pi_dma| {
        execute_controller_pif(Cycles::ZERO, &mut read, pi_dma)
    });
    assert_eq!(&read[5..37], &payload);
    assert_eq!(read[37], accessory_data_crc(&payload));
    with_executor(|executor| {
        assert_eq!(executor.controller_pak(0).unwrap().active_bank(), 1);
    });
    assert_eq!(
        read_operations.save_operations,
        vec![fn64_runtime::SaveOperationEvent {
            at: Cycles::ZERO,
            device: fn64_runtime::SaveType::ControllerPak,
            operation: fn64_runtime::SaveOperationKind::Read,
            offset: (fn64_runtime::pfs::PFS_BANK_CAPACITY + fn64_runtime::pfs::PFS_PAGE_SIZE)
                as u32,
            len: ACCESSORY_BLOCK_BYTES as u32,
        }]
    );
    assert!(read_operations.controller_operations.is_empty());
}

#[test]
fn raw_transfer_pak_reaches_banked_game_boy_rom_and_persistent_ram() {
    with_executor(|executor| *executor = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
    let mut gb_rom = vec![0xff; 64 * 0x4000];
    gb_rom[0x147] = 0x03; // MBC1 + RAM + battery
    gb_rom[0x149] = 0x03; // 32 KiB RAM
    for bank in 0..64 {
        gb_rom[bank * 0x4000] = bank as u8;
    }
    insert_transfer_pak_cartridge(0, gb_rom, None).unwrap();

    let write = |address: u16, value: u8| {
        let encoded = address | u16::from(accessory_address_crc(address));
        let mut packet = [0u8; 64];
        packet[0] = 35;
        packet[1] = 1;
        packet[2] = 0x03;
        packet[3..5].copy_from_slice(&encoded.to_be_bytes());
        packet[5..37].fill(value);
        packet[38] = 0xfe;
        crate::pi::with_pi_dma("raw Transfer Pak write", |pi_dma| {
            execute_controller_pif(Cycles::ZERO, &mut packet, pi_dma)
        });
        assert_eq!(packet[37], accessory_data_crc(&[value; 32]));
    };
    let read = |address: u16| {
        let encoded = address | u16::from(accessory_address_crc(address));
        let mut packet = [0u8; 64];
        packet[0] = 3;
        packet[1] = 33;
        packet[2] = 0x02;
        packet[3..5].copy_from_slice(&encoded.to_be_bytes());
        packet[38] = 0xfe;
        crate::pi::with_pi_dma("raw Transfer Pak read", |pi_dma| {
            execute_controller_pif(Cycles::ZERO, &mut packet, pi_dma)
        });
        let data: [u8; 32] = packet[5..37].try_into().unwrap();
        assert_eq!(packet[37], accessory_data_crc(&data));
        data
    };

    assert_eq!(read(0x8000), [0; 32]);
    write(0x8000, 0x84);
    assert_eq!(read(0x8000), [0x84; 32]);
    assert_eq!(read(0xb000), [0x84; 32]);

    // Transfer bank zero exposes GB 0x2000 at accessory 0xe000; select
    // MBC1 ROM bank two, then Transfer bank one exposes GB 0x4000.
    write(0xe000, 2);
    write(0xa000, 1);
    assert_eq!(read(0xc000)[0], 2);

    // Select MBC1 RAM banking mode and RAM bank two, then write GB
    // 0xa000 through Transfer bank two. The host-visible cartridge RAM
    // must observe the same byte, proving raw Joybus and persistence use
    // one backing store.
    write(0xa000, 0);
    write(0xc000, 0x0a);
    write(0xa000, 1);
    write(0xc000, 2);
    write(0xe000, 1);
    write(0xa000, 2);
    write(0xe000, 0x5a);
    assert_eq!(read(0xe000), [0x5a; 32]);
    with_executor(|executor| {
        assert_eq!(
            executor
                .transfer_pak(0)
                .expect("configured Transfer Pak")
                .cartridge_ram()
                .expect("MBC1 cartridge RAM")[2 * 0x2000],
            0x5a
        );
    });
}

#[test]
fn raw_and_high_level_transfer_pak_paths_share_one_mbc3_guest_clock() {
    fn high_transfer(rdram: &mut [u8], write: bool, address: u16, buffer_offset: u32) {
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0200;
        ctx.r5 = u64::from(write);
        ctx.r6 = u64::from(address);
        ctx.r7 = u64::from(0x8000_0000 | buffer_offset);
        ctx.r29 = 0x8000_0080;
        unsafe { storage.write_u32(RdramAddr::from_offset(0x90), 32) };
        unsafe { crate::gbpak::osGbpakReadWrite_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
    }

    with_executor(|executor| *executor = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
    let mut gb_rom = vec![0xff; 4 * 0x4000];
    gb_rom[0x147] = 0x10; // MBC3 + timer + RAM + battery
    gb_rom[0x149] = 0x03; // 32 KiB RAM
    insert_transfer_pak_cartridge(0, gb_rom, None).unwrap();

    let raw_write = |address: u16, value: u8, now: Cycles| {
        let encoded = address | u16::from(accessory_address_crc(address));
        let mut packet = [0u8; 64];
        packet[0] = 35;
        packet[1] = 1;
        packet[2] = 0x03;
        packet[3..5].copy_from_slice(&encoded.to_be_bytes());
        packet[5..37].fill(value);
        packet[38] = 0xfe;
        crate::pi::with_pi_dma("raw timed Transfer Pak write", |pi_dma| {
            execute_controller_pif(now, &mut packet, pi_dma)
        });
        assert_eq!(packet[37], accessory_data_crc(&[value; 32]));
    };
    let raw_read = |address: u16, now: Cycles| {
        let encoded = address | u16::from(accessory_address_crc(address));
        let mut packet = [0u8; 64];
        packet[0] = 3;
        packet[1] = 33;
        packet[2] = 0x02;
        packet[3..5].copy_from_slice(&encoded.to_be_bytes());
        packet[38] = 0xfe;
        crate::pi::with_pi_dma("raw timed Transfer Pak read", |pi_dma| {
            execute_controller_pif(now, &mut packet, pi_dma)
        });
        let data: [u8; 32] = packet[5..37].try_into().unwrap();
        assert_eq!(packet[37], accessory_data_crc(&data));
        data
    };

    // Raw Joybus powers the Pak, enables MBC3 RAM/RTC, halts the timer,
    // initializes seconds to zero, then resumes it at guest cycle zero.
    raw_write(0x8000, 0x84, Cycles::ZERO);
    raw_write(0xa000, 0, Cycles::ZERO);
    raw_write(0xc000, 0x0a, Cycles::ZERO);
    raw_write(0xa000, 1, Cycles::ZERO);
    raw_write(0xc000, 0x0c, Cycles::ZERO);
    raw_write(0xa000, 2, Cycles::ZERO);
    raw_write(0xe000, 0x40, Cycles::ZERO);
    raw_write(0xa000, 1, Cycles::ZERO);
    raw_write(0xc000, 0x08, Cycles::ZERO);
    raw_write(0xa000, 2, Cycles::ZERO);
    raw_write(0xe000, 0, Cycles::ZERO);
    raw_write(0xa000, 1, Cycles::ZERO);
    raw_write(0xc000, 0x0c, Cycles::ZERO);
    raw_write(0xa000, 2, Cycles::ZERO);
    raw_write(0xe000, 0, Cycles::ZERO);

    let mut rdram = vec![0; 0x800];
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
    unsafe {
        storage.write_u32(RdramAddr::from_offset(0x200), 0x10);
        storage.write_u32(RdramAddr::from_offset(0x208), 0);
        for offset in 0..32 {
            storage.write_u8(RdramAddr::from_offset(0x300 + offset), 0);
            storage.write_u8(RdramAddr::from_offset(0x340 + offset), 1);
            storage.write_u8(RdramAddr::from_offset(0x380 + offset), 0x08);
        }
    }

    crate::advance_virtual_time(fn64_runtime::CPU_CLOCK_HZ - 1);
    high_transfer(&mut rdram, true, 0x6000, 0x300);
    high_transfer(&mut rdram, true, 0x6000, 0x340);
    high_transfer(&mut rdram, true, 0x4000, 0x380);
    high_transfer(&mut rdram, false, 0xa000, 0x400);
    assert_eq!(unsafe { storage.read_u8(RdramAddr::from_offset(0x400)) }, 0);

    crate::advance_virtual_time(fn64_runtime::CPU_CLOCK_HZ);
    high_transfer(&mut rdram, false, 0xa000, 0x440);
    assert_eq!(
        unsafe { storage.read_u8(RdramAddr::from_offset(0x440)) },
        0,
        "high-level read must retain the prior RTC latch"
    );
    high_transfer(&mut rdram, true, 0x6000, 0x300);
    high_transfer(&mut rdram, true, 0x6000, 0x340);
    high_transfer(&mut rdram, false, 0xa000, 0x480);
    assert_eq!(unsafe { storage.read_u8(RdramAddr::from_offset(0x480)) }, 1);
    assert_eq!(
        raw_read(0xe000, Cycles::new(fn64_runtime::CPU_CLOCK_HZ)),
        [1; 32]
    );
}

#[test]
fn host_battery_forwarding_materializes_before_guest_access() {
    let mut gb_rom = vec![0xff; 4 * 0x4000];
    gb_rom[0x147] = 0x10;
    gb_rom[0x149] = 0x03;
    let mut source = fn64_runtime::TransferPak::new();
    source.insert_cartridge(gb_rom.clone(), None).unwrap();
    let metadata = source
        .checkpoint_mbc3_battery(
            Cycles::new(fn64_runtime::CPU_CLOCK_HZ / 2),
            fn64_runtime::HostUnixNanos::new(1_000_000_000),
        )
        .unwrap()
        .unwrap();

    with_executor(|executor| *executor = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
    insert_transfer_pak_cartridge_with_battery(
        0,
        gb_rom,
        None,
        Some(fn64_runtime::Mbc3BatteryRestore::new(
            metadata,
            fn64_runtime::HostUnixNanos::new(2_500_000_000),
        )),
    )
    .unwrap();
    let checkpoint =
        checkpoint_transfer_pak_battery(0, fn64_runtime::HostUnixNanos::new(3_000_000_000))
            .unwrap()
            .unwrap();
    assert_eq!(checkpoint.rtc()[0], 2);
    assert_eq!(checkpoint.subsecond_cycles(), 0);
}

#[test]
fn transfer_pak_removal_changes_status_and_data_access_traps_by_name() {
    with_executor(|executor| *executor = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
    let mut gb_rom = vec![0xff; 2 * 0x4000];
    gb_rom[0x147] = 0x00;
    gb_rom[0x149] = 0;
    insert_transfer_pak_cartridge(0, gb_rom, None).unwrap();
    with_executor(|executor| {
        let pak = executor.transfer_pak_mut(0).unwrap();
        pak.write_block(0x8000, &[0x84; 32]);
        assert!(pak.remove_cartridge().is_some());
        let mut status = [0xff; 32];
        pak.read_block(0xb000, &mut status);
        assert_eq!(status, [0xc0; 32]);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pak.read_block(0xc000, &mut status)
        }))
        .expect_err("powered Transfer Pak without a cartridge must not fabricate data");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("panic must carry Transfer Pak context");
        assert!(message.contains("no Game Boy cartridge"), "{message}");
    });
}

#[test]
fn malformed_raw_accessory_address_crc_traps_with_address_context() {
    with_executor(|executor| *executor = fn64_runtime::Executor::new());
    load_rom(vec![0; 0x100]);
    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerRumblePak);
    let mut packet = [0u8; 64];
    packet[0] = 3;
    packet[1] = 33;
    packet[2] = 0x02;
    packet[3..5].copy_from_slice(&0xC000u16.to_be_bytes());
    packet[38] = 0xFE;
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::pi::with_pi_dma("malformed raw accessory test", |pi_dma| {
            execute_controller_pif(Cycles::ZERO, &mut packet, pi_dma)
        });
    }))
    .expect_err("wrong accessory address CRC must trap");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("panic must carry protocol context");
    assert!(message.contains("command 0x02 on channel 0"), "{message}");
    assert!(message.contains("for 0xc000; expected 0x1b"), "{message}");
}

/// osContInit: (1) OSContStatus entries must be written SWIZZLED (^3) like
/// osContGetQuery, and (2) ctlBitfield is a `u8*` -- a SINGLE swizzled
/// byte, no +1 store. Fails against the bug (flat status stores + two
/// bitfield bytes at flat +0/+1).
#[test]
fn os_cont_init_swizzles_status_and_writes_single_bitfield_byte() {
    reset_controller_manager();
    // data at offset 0x40 (16 bytes = 4 OSContStatus), bitfield at 0x80.
    let mut rdram = vec![0xEEu8; 256]; // 0xEE sentinel: catch stray writes.
    assert_eq!(
        run_controller_init(&mut rdram, 0x8000_0008, 0x8000_0080, 0x8000_0040, 351,),
        0
    );

    // Port 0 is a standard controller (type 0x0005). The swizzled entry
    // [type_hi=0x00, type_lo=0x05, status=0x00, pad=0x00] lands at
    // (0x40+o)^3, so logical byte 1 (0x05) is at host 0x40+ (1^3)=0x40+2.
    let logical = |base: usize, o: usize| rdram[(base + o) ^ 3];
    assert_eq!(logical(0x40, 0), 0x00, "port0 type_hi");
    assert_eq!(logical(0x40, 1), 0x05, "port0 type_lo (CONT_TYPE_STANDARD)");
    assert_eq!(logical(0x40, 2), 0x00, "port0 status");
    assert_eq!(logical(0x40, 3), 0x00, "port0 pad");
    // Port 1 absent -> [0,0,0,CONT_NO_RESPONSE_ERROR] swizzled.
    assert_eq!(logical(0x44, 3), CONT_NO_RESPONSE_ERROR, "port1 errno");

    // ctlBitfield: a SINGLE swizzled byte = mask (0x01, only port 0). The
    // flat address 0x80 must stay the 0xEE sentinel (the buggy flat store
    // would overwrite it), and 0x81 must stay 0xEE (the buggy +1 store
    // would clobber this adjacent byte).
    assert_eq!(
        rdram[0x80 ^ 3],
        0x01,
        "bitfield: single swizzled byte, port0 set"
    );
    assert_eq!(
        rdram[0x80], 0xEE,
        "flat bitfield addr untouched (no flat store)"
    );
    assert_eq!(rdram[0x81], 0xEE, "adjacent byte untouched (no +1 store)");
}

#[test]
fn motor_init_start_and_stop_share_the_configured_accessory_state() {
    with_executor(|exec| *exec = fn64_runtime::Executor::new());
    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerRumblePak);

    let mut rdram = vec![0u8; 0x100];
    let pfs_vram = 0x8000_0040u64;
    let queue_vram = 0x8000_0080u64;
    let mut init = ctx_with(queue_vram, pfs_vram, 0);
    unsafe { osMotorInit_recomp(rdram.as_mut_ptr(), &mut init) };
    assert_eq!(init.r2, 0);
    assert_eq!(u32::from_ne_bytes(rdram[0x40..0x44].try_into().unwrap()), 8);
    assert_eq!(
        u32::from_ne_bytes(rdram[0x44..0x48].try_into().unwrap()),
        queue_vram as u32
    );
    assert_eq!(u32::from_ne_bytes(rdram[0x48..0x4c].try_into().unwrap()), 0);
    assert_eq!(
        with_executor(|exec| exec.pif().query_response(0)),
        [0x05, 0x00, fn64_runtime::CONT_CARD_ON]
    );

    let mut access = ctx_zeroed();
    access.r4 = pfs_vram;
    unsafe { osMotorStart_recomp(rdram.as_mut_ptr(), &mut access) };
    assert_eq!(access.r2, 0);
    assert!(rumble_active(0));

    unsafe { osMotorStop_recomp(rdram.as_mut_ptr(), &mut access) };
    assert_eq!(access.r2, 0);
    assert!(!rumble_active(0));
}

#[test]
fn motor_init_returns_documented_no_pak_and_wrong_device_errors() {
    with_executor(|exec| *exec = fn64_runtime::Executor::new());
    let mut rdram = vec![0u8; 0x100];
    let mut ctx = ctx_with(0x8000_0080, 0x8000_0040, 0);

    unsafe { osMotorInit_recomp(rdram.as_mut_ptr(), &mut ctx) };
    assert_eq!(ctx.r2, PFS_ERR_NOPACK as u64);

    set_controller_port_state(0, fn64_runtime::PortState::StandardControllerControllerPak);
    unsafe { osMotorInit_recomp(rdram.as_mut_ptr(), &mut ctx) };
    assert_eq!(ctx.r2, PFS_ERR_DEVICE as u64);
}

#[test]
fn controller_dma_completion_raises_the_shared_mi_si_source() {
    reset_controller_manager();
    initialize_controller_manager_for_test(4);
    crate::pi::set_mi_interrupt_mask(fn64_runtime::InterruptSource::Si.bit());
    let queue = RdramAddr::from_offset(0x40);
    with_executor(|exec| {
        exec.create_mesg_queue(queue, 1);
        exec.set_event_mesg(OS_EVENT_SI, queue, 0);
    });
    let mut ctx = ctx_zeroed();
    ctx.r4 = queue.to_kseg0() as u64;
    unsafe { osContStartQuery_recomp(std::ptr::null_mut(), &mut ctx) };

    let before = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
    assert_eq!(before & fn64_runtime::InterruptSource::Si.bit(), 0);
    assert_eq!(
        with_executor(|exec| exec.recv_mesg(99, queue, false)),
        fn64_runtime::RecvMesgOutcome::WouldBlock
    );
    crate::advance_virtual_time(1);

    let pending = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
    assert_ne!(pending & fn64_runtime::InterruptSource::Si.bit(), 0);
    assert!(crate::pi::cpu_interrupt_pending());
    assert_eq!(
        with_executor(|exec| exec.recv_mesg(99, queue, false)),
        fn64_runtime::RecvMesgOutcome::Delivered(0)
    );
}
