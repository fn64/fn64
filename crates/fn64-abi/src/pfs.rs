//! Controller Pak/PFS ABI adapters over [`fn64_runtime::ControllerPak`].
//!
//! Signatures, OSPfs/OSPfsState layouts, 16-entry directory, 256-byte
//! allocation rounding, 32-byte I/O granularity, and return codes come from
//! the public libultra Controller Pak manuals and `os_pfs.h`.

use super::*;

const PFS_ERR_NOPACK: u32 = 1;
const PFS_ERR_INCONSISTENT: u32 = 3;
const PFS_ERR_INVALID: u32 = 5;
const PFS_DATA_FULL: u32 = 7;
const PFS_DIR_FULL: u32 = 8;
const PFS_ERR_EXIST: u32 = 9;
const PFS_ERR_DEVICE: u32 = 11;
const PFS_INITIALIZED: u32 = 1;
const PFS_READ: u32 = 0;
const PFS_WRITE: u32 = 1;

fn pfs_error_code(error: fn64_runtime::PfsError) -> u32 {
    match error {
        fn64_runtime::PfsError::Invalid => PFS_ERR_INVALID,
        fn64_runtime::PfsError::Inconsistent => PFS_ERR_INCONSISTENT,
        fn64_runtime::PfsError::DataFull => PFS_DATA_FULL,
        fn64_runtime::PfsError::DirectoryFull => PFS_DIR_FULL,
        fn64_runtime::PfsError::Exists => PFS_ERR_EXIST,
    }
}

fn set_result(ctx: &mut RecompContext, result: Result<(), u32>) {
    ctx.r2 = result.err().unwrap_or(0) as u64;
}

unsafe fn read_logical<const N: usize>(storage: fn64_runtime::RdramPtr, raw: u64) -> [u8; N] {
    let base = RdramAddr::from_gpr(raw);
    std::array::from_fn(|index| unsafe {
        storage.read_u8(
            base.checked_add(u32::try_from(index).expect("PFS field length exceeds u32"))
                .expect("PFS guest field address overflow"),
        )
    })
}

unsafe fn copy_from_guest(storage: fn64_runtime::RdramPtr, raw: u64, len: usize) -> Vec<u8> {
    let base = RdramAddr::from_gpr(raw);
    (0..len)
        .map(|index| unsafe {
            storage.read_u8(
                base.checked_add(u32::try_from(index).expect("PFS transfer exceeds u32"))
                    .expect("PFS guest source address overflow"),
            )
        })
        .collect()
}

unsafe fn copy_to_guest(storage: fn64_runtime::RdramPtr, raw: u64, bytes: &[u8]) {
    let base = RdramAddr::from_gpr(raw);
    for (index, &byte) in bytes.iter().enumerate() {
        unsafe {
            storage.write_u8(
                base.checked_add(u32::try_from(index).expect("PFS transfer exceeds u32"))
                    .expect("PFS guest destination address overflow"),
                byte,
            );
        }
    }
}

fn read_key(
    storage: fn64_runtime::RdramPtr,
    company_code: u64,
    game_code: u64,
    game_name: u64,
    ext_name: u64,
) -> fn64_runtime::PfsKey {
    fn64_runtime::PfsKey {
        company_code: company_code as u16,
        game_code: game_code as u32,
        game_name: unsafe { read_logical(storage, game_name) },
        ext_name: unsafe { read_logical(storage, ext_name) },
    }
}

fn pfs_channel(storage: fn64_runtime::RdramPtr, pfs_raw: u64) -> Result<usize, u32> {
    let pfs = RdramAddr::from_gpr(pfs_raw);
    let status = unsafe { storage.read_u32(pfs) };
    if status & PFS_INITIALIZED == 0 {
        return Err(PFS_ERR_INVALID);
    }
    Ok(unsafe {
        storage.read_u32(
            pfs.checked_add(8)
                .expect("OSPfs channel field address overflow"),
        )
    } as usize)
}

fn with_controller_pak<R>(
    channel: usize,
    operation: impl FnOnce(&mut fn64_runtime::ControllerPak) -> Result<R, fn64_runtime::PfsError>,
) -> Result<R, u32> {
    with_executor(|exec| match exec.pif().port_state(channel) {
        fn64_runtime::PortState::StandardControllerControllerPak => operation(
            exec.controller_pak_mut(channel)
                .expect("Controller Pak identity exists without ControllerPak storage"),
        )
        .map_err(pfs_error_code),
        fn64_runtime::PortState::StandardControllerRumblePak
        | fn64_runtime::PortState::StandardControllerTransferPak => Err(PFS_ERR_DEVICE),
        fn64_runtime::PortState::VoiceRecognitionUnit => Err(PFS_ERR_DEVICE),
        fn64_runtime::PortState::StandardControllerNoPak | fn64_runtime::PortState::Absent => {
            Err(PFS_ERR_NOPACK)
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PfsIsPlugQueueError {
    MissingSiRegistration,
    WrongSiTarget { registered: RdramAddr },
    Uninitialized,
    SharedOrActive(fn64_runtime::MesgQueueActivity),
}

fn pfs_is_plug_completion_message(mq_addr: RdramAddr) -> Result<Mesg, PfsIsPlugQueueError> {
    const OS_EVENT_SI: u32 = 5;
    with_executor(|exec| {
        let (event_queue, message) = exec
            .event_registration(OS_EVENT_SI)
            .ok_or(PfsIsPlugQueueError::MissingSiRegistration)?;
        if event_queue != mq_addr {
            return Err(PfsIsPlugQueueError::WrongSiTarget {
                registered: event_queue,
            });
        }
        let activity = exec
            .queue_activity(mq_addr)
            .ok_or(PfsIsPlugQueueError::Uninitialized)?;
        if !activity.is_exclusively_idle() {
            return Err(PfsIsPlugQueueError::SharedOrActive(activity));
        }
        Ok(message)
    })
}

fn require_pfs_is_plug_completion_message(mq_addr: RdramAddr) -> Mesg {
    pfs_is_plug_completion_message(mq_addr).unwrap_or_else(|error| match error {
        PfsIsPlugQueueError::MissingSiRegistration => {
            panic!("osPfsIsPlug_recomp: OS_EVENT_SI has no registered completion message queue")
        }
        PfsIsPlugQueueError::WrongSiTarget { registered } => panic!(
            "osPfsIsPlug_recomp: queue {:#010x} is not the OS_EVENT_SI target {:#010x}",
            mq_addr.offset(),
            registered.offset()
        ),
        PfsIsPlugQueueError::Uninitialized => panic!(
            "osPfsIsPlug_recomp: queue {:#010x} was not initialized by osCreateMesgQueue",
            mq_addr.offset()
        ),
        PfsIsPlugQueueError::SharedOrActive(activity) => panic!(
            "osPfsIsPlug_recomp: private SI completion queue {:#010x} is shared or not idle: {activity:?}",
            mq_addr.offset()
        ),
    })
}

/// `osPfsIsPlug(OSMesgQueue *mq, u8 *bitpattern) -> s32`.
///
/// The public function manual defines bits 0 through 3 as the corresponding
/// controller ports containing a Pak. `osContSetCh` further limits this
/// multi-device probe to the Controller Manager's active `0..ch` prefix.
/// The queue is the caller-created `OS_EVENT_SI` target that makes the
/// request synchronous: the request latches the typed PIF status, then the
/// live SI fabric commits bytes, clears busy, raises MI SI, and posts the
/// completion before this shim resumes and writes the result. The fabric's
/// fixed one-cycle latency remains a compatibility
/// policy because the public manual gives no exact transfer-cycle formula.
///
/// # Safety
/// `bitpattern` in `ctx.r5` must address one writable guest byte in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osPfsIsPlug_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let thread = current_thread_id("osPfsIsPlug_recomp");
    let result_addr = RdramAddr::from_gpr(ctx.r5);
    with_host(|host| {
        assert_eq!(
            host.runtime_rdram, rdram,
            "osPfsIsPlug_recomp: caller RDRAM is not the registered process allocation"
        );
        assert!(
            (result_addr.offset() as usize) < host.runtime_rdram_len,
            "osPfsIsPlug_recomp: result byte {:#010x} lies outside registered process RDRAM length {:#x}",
            result_addr.offset(),
            host.runtime_rdram_len
        );
    });
    let channels = with_host(|host| {
        assert!(
            host.controller_manager.initialized,
            "osPfsIsPlug_recomp: osContInit must initialize the Controller Manager first"
        );
        host.controller_manager.channels()
    });
    let message = require_pfs_is_plug_completion_message(mq_addr);

    // The compatibility transaction latches the same typed PIF status image
    // that the request owns. A host hot-plug after SI completion but before
    // this coroutine is scheduled again must not rewrite an already-finished
    // query's result.
    let bitpattern = with_executor(|exec| {
        (0..channels).fold(0u8, |pattern, port| {
            let status = exec.pif().query_response(port)[2];
            if status & fn64_runtime::CONT_CARD_ON != 0 {
                pattern | (1 << port)
            } else {
                pattern
            }
        })
    });
    let transaction = PfsIsPlugTransaction {
        thread,
        queue: mq_addr,
        message,
        result_addr,
        bitpattern,
    };

    match crate::pi::start_live_si_dma(
        fn64_runtime::SiDmaRequest {
            kind: fn64_runtime::SiDmaKind::ControllerQuery,
            dram_addr: RdramAddr::from_offset(0),
        },
        PendingSiCompletionOwner::PfsIsPlug(transaction),
    ) {
        Ok(()) => {}
        // The older public function page specifies 1 for an unsuccessful
        // call. A later 5.2 page says -1; retain the older ABI's positive
        // failure value until a title's linked libultra version supplies
        // stronger authority.
        Err(DeviceFault::SiBusy) => {
            ctx.r2 = 1;
            return;
        }
        Err(error) => panic!("osPfsIsPlug_recomp: SI request failed: {error}"),
    }

    let delivered = match suspend_active_coroutine(Yield::BlockOnRecv {
        mq_addr,
        may_block: true,
    }) {
        Resume::Delivered(message) => message,
        other => {
            panic!("osPfsIsPlug_recomp: synchronous SI wait resumed with unexpected {other:?}")
        }
    };

    let completed = with_host(|host| {
        host.completed_pfs_is_plug
            .remove(&thread)
            .unwrap_or_else(|| {
                panic!(
                    "osPfsIsPlug_recomp: thread {thread} resumed without its typed SI completion"
                )
            })
    });
    assert_eq!(completed, transaction, "osPfsIsPlug transaction drifted");
    assert_eq!(
        delivered, completed.message,
        "osPfsIsPlug_recomp: private queue delivered a non-transaction message"
    );

    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    unsafe { storage.write_u8(completed.result_addr, completed.bitpattern) };
    ctx.r2 = 0;
}

/// `osPfsInitPak(OSMesgQueue *mq, OSPfs *pfs, int controller_no) -> s32`.
///
/// # Safety
/// Guest pointers in `ctx` must address their complete public ABI objects in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osPfsInitPak_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let channel = ctx.r6 as usize;
    let result = with_controller_pak(channel, |pak| pak.check_and_repair());
    if result.is_ok() {
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        let pfs = RdramAddr::from_gpr(ctx.r5);
        unsafe {
            storage.write_u32(pfs, PFS_INITIALIZED);
            storage.write_u32(
                pfs.checked_add(4)
                    .expect("OSPfs queue field address overflow"),
                ctx.r4 as u32,
            );
            storage.write_u32(
                pfs.checked_add(8)
                    .expect("OSPfs channel field address overflow"),
                channel as u32,
            );
        }
    }
    set_result(ctx, result.map(|_| ()));
}

/// `osPfsFreeBlocks(OSPfs *pfs, s32 *bytes_not_used) -> s32`.
///
/// # Safety
/// Guest pointers in `ctx` must address their complete public ABI objects in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osPfsFreeBlocks_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = pfs_channel(storage, ctx.r4).and_then(|channel| {
        with_controller_pak(channel, |pak| pak.free_bytes()).map(|free| unsafe {
            storage.write_u32(RdramAddr::from_gpr(ctx.r5), free as u32);
        })
    });
    set_result(ctx, result);
}

/// `osPfsAllocateFile`, including its three stack-passed o32 arguments.
///
/// # Safety
/// Guest pointers and the o32 argument area must be valid in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osPfsAllocateFile_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let ext_name = unsafe { read_stack_word(rdram, ctx.r29, 0x10) } as u64;
    let length = unsafe { read_stack_word(rdram, ctx.r29, 0x14) } as usize;
    let file_no = unsafe { read_stack_word(rdram, ctx.r29, 0x18) } as u64;
    let key = read_key(storage, ctx.r5, ctx.r6, ctx.r7, ext_name);
    let result = pfs_channel(storage, ctx.r4).and_then(|channel| {
        with_controller_pak(channel, |pak| pak.allocate(key, length)).map(|slot| unsafe {
            storage.write_u32(RdramAddr::from_gpr(file_no), slot as u32);
        })
    });
    set_result(ctx, result);
}

/// `osPfsDeleteFile(OSPfs*, u16, u32, u8[16], u8[4]) -> s32`.
///
/// # Safety
/// Guest pointers and the o32 argument area must be valid in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osPfsDeleteFile_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let ext_name = unsafe { read_stack_word(rdram, ctx.r29, 0x10) } as u64;
    let key = read_key(storage, ctx.r5, ctx.r6, ctx.r7, ext_name);
    let result = pfs_channel(storage, ctx.r4)
        .and_then(|channel| with_controller_pak(channel, |pak| pak.delete(key)));
    set_result(ctx, result);
}

/// `osPfsFindFile`, returning the matching directory slot through `file_no`.
///
/// # Safety
/// Guest pointers and the o32 argument area must be valid in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osPfsFindFile_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let ext_name = unsafe { read_stack_word(rdram, ctx.r29, 0x10) } as u64;
    let file_no = unsafe { read_stack_word(rdram, ctx.r29, 0x14) } as u64;
    let key = read_key(storage, ctx.r5, ctx.r6, ctx.r7, ext_name);
    let result = pfs_channel(storage, ctx.r4).and_then(|channel| {
        with_controller_pak(channel, |pak| pak.find(key)).map(|slot| unsafe {
            storage.write_u32(RdramAddr::from_gpr(file_no), slot as u32);
        })
    });
    set_result(ctx, result);
}

/// `osPfsFileState(OSPfs *pfs, s32 file_no, OSPfsState *state) -> s32`.
///
/// # Safety
/// Guest pointers in `ctx` must address their complete public ABI objects in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osPfsFileState_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = pfs_channel(storage, ctx.r4).and_then(|channel| {
        with_controller_pak(channel, |pak| pak.state(ctx.r5 as usize)).map(|state| {
            let out = RdramAddr::from_gpr(ctx.r6);
            unsafe {
                storage.write_u32(out, state.file_size);
                storage.write_u32(
                    out.checked_add(4).expect("OSPfsState game_code overflow"),
                    state.key.game_code,
                );
                storage.write_u16(
                    out.checked_add(8)
                        .expect("OSPfsState company_code overflow"),
                    state.key.company_code,
                );
                copy_to_guest(storage, ctx.r6.wrapping_add(10), &state.key.ext_name);
                copy_to_guest(storage, ctx.r6.wrapping_add(14), &state.key.game_name);
            }
        })
    });
    set_result(ctx, result);
}

/// `osPfsReadWriteFile(OSPfs*, s32, u8, int, int, u8*) -> s32`.
///
/// # Safety
/// Guest pointers and the o32 argument area must cover the requested transfer in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osPfsReadWriteFile_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let nbytes = unsafe { read_stack_word(rdram, ctx.r29, 0x10) } as usize;
    let data = unsafe { read_stack_word(rdram, ctx.r29, 0x14) } as u64;
    let channel = match pfs_channel(storage, ctx.r4) {
        Ok(channel) => channel,
        Err(error) => {
            set_result(ctx, Err(error));
            return;
        }
    };
    let operation = match ctx.r6 as u32 {
        PFS_READ => Some(fn64_runtime::SaveOperationKind::Read),
        PFS_WRITE => Some(fn64_runtime::SaveOperationKind::Write),
        _ => None,
    };
    let result = match operation {
        Some(fn64_runtime::SaveOperationKind::Read) => {
            let mut bytes = vec![0; nbytes];
            with_controller_pak(channel, |pak| {
                pak.read(ctx.r5 as usize, ctx.r7 as usize, &mut bytes)
            })
            .map(|_| unsafe { copy_to_guest(storage, data, &bytes) })
        }
        Some(fn64_runtime::SaveOperationKind::Write) => {
            let bytes = unsafe { copy_from_guest(storage, data, nbytes) };
            with_controller_pak(channel, |pak| {
                pak.write(ctx.r5 as usize, ctx.r7 as usize, &bytes)
            })
        }
        Some(fn64_runtime::SaveOperationKind::Erase) => {
            unreachable!("PFS read/write ABI does not expose erase")
        }
        None => Err(PFS_ERR_INVALID),
    };
    if result.is_ok() {
        crate::record_save_operation(
            fn64_runtime::SaveType::ControllerPak,
            operation.expect("successful PFS operation has a typed direction"),
            ctx.r7 as usize,
            nbytes,
        );
    }
    set_result(ctx, result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ctx_zeroed, spawn_test_thread};

    fn logical_write(rdram: &mut [u8], offset: usize, bytes: &[u8]) {
        for (index, &byte) in bytes.iter().enumerate() {
            rdram[(offset + index) ^ 3] = byte;
        }
    }

    fn stack_word(rdram: &mut [u8], sp: usize, offset: usize, value: u32) {
        rdram[sp + offset..sp + offset + 4].copy_from_slice(&value.to_ne_bytes());
    }

    #[test]
    fn pfs_is_plug_waits_for_si_completion_and_obeys_controller_manager_prefix() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        with_host(|host| *host = HostState::default());
        crate::si::set_controller_port_state(0, fn64_runtime::PortState::StandardControllerNoPak);
        crate::si::set_controller_port_state(
            1,
            fn64_runtime::PortState::StandardControllerControllerPak,
        );

        const MQ_VRAM: u64 = 0x8000_0008;
        const RESULT_VRAM: u64 = 0x8000_0060;
        let mq_addr = RdramAddr::from_gpr(MQ_VRAM);
        with_executor(|exec| {
            exec.create_mesg_queue(mq_addr, 1);
            exec.set_event_mesg(5, mq_addr, 0xCAFE);
        });

        let mut rdram = vec![0u8; 0x80];
        unsafe { crate::register_process_rdram(rdram.as_mut_ptr(), rdram.len()) };
        with_host(|host| {
            assert!(host.controller_manager.initialize());
        });

        let mut set_ch = ctx_zeroed();
        set_ch.r4 = 2;
        unsafe { crate::si::osContSetCh_recomp(rdram.as_mut_ptr(), &mut set_ch) };

        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
        unsafe { storage.write_u8(RdramAddr::from_gpr(RESULT_VRAM), 0xA5) };
        let result = std::rc::Rc::new(std::cell::Cell::new(None));
        let thread_result = result.clone();
        let rdram_ptr = rdram.as_mut_ptr();
        spawn_test_thread(301, 10, move || {
            let mut probe = ctx_zeroed();
            probe.r4 = MQ_VRAM;
            probe.r5 = RESULT_VRAM;
            unsafe { osPfsIsPlug_recomp(rdram_ptr, &mut probe) };
            thread_result.set(Some(probe.r2));
        });

        assert!(crate::run_one_step());
        assert_eq!(
            unsafe { storage.read_u8(RdramAddr::from_gpr(RESULT_VRAM)) },
            0xA5
        );
        let kinds = crate::copy_device_trace()
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        assert!(matches!(
            kinds.as_slice(),
            [
                fn64_runtime::DeviceTraceKind::SiDmaStarted(_),
                fn64_runtime::DeviceTraceKind::SiBytesCommitted(_),
                fn64_runtime::DeviceTraceKind::SiBusyCleared,
                fn64_runtime::DeviceTraceKind::MiInterruptRaised(fn64_runtime::InterruptSource::Si),
                fn64_runtime::DeviceTraceKind::NotificationReady(_),
            ]
        ));
        assert_ne!(
            with_host(|host| host.device_fabric.snapshot().mi_pending)
                & fn64_runtime::InterruptSource::Si.bit(),
            0
        );
        crate::si::set_controller_port_state(1, fn64_runtime::PortState::StandardControllerNoPak);

        assert!(crate::run_one_step());
        assert_eq!(
            unsafe { storage.read_u8(RdramAddr::from_gpr(RESULT_VRAM)) },
            0xA5
        );
        assert_eq!(
            with_executor(|exec| exec.queue_valid_count(mq_addr)),
            Some(0)
        );

        assert!(crate::run_one_step());
        assert_eq!(result.get(), Some(0));
        assert_eq!(
            unsafe { storage.read_u8(RdramAddr::from_gpr(RESULT_VRAM)) },
            0b10
        );
    }

    #[test]
    fn pfs_is_plug_busy_returns_one_without_touching_output() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        with_host(|host| *host = HostState::default());
        with_host(|host| {
            host.controller_manager.initialize();
            host.controller_manager.set_channels(1);
        });

        const MQ_VRAM: u64 = 0x8000_0008;
        const RESULT_VRAM: u64 = 0x8000_0060;
        let mq_addr = RdramAddr::from_gpr(MQ_VRAM);
        with_executor(|exec| {
            exec.create_mesg_queue(mq_addr, 1);
            exec.set_event_mesg(5, mq_addr, 0xCAFE);
        });
        crate::pi::start_live_si_dma(
            fn64_runtime::SiDmaRequest {
                kind: fn64_runtime::SiDmaKind::ControllerRead,
                dram_addr: RdramAddr::from_offset(0),
            },
            PendingSiCompletionOwner::OsEvent,
        )
        .unwrap();

        let mut rdram = vec![0u8; 0x80];
        unsafe { crate::register_process_rdram(rdram.as_mut_ptr(), rdram.len()) };
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
        unsafe { storage.write_u8(RdramAddr::from_gpr(RESULT_VRAM), 0xA5) };
        let result = std::rc::Rc::new(std::cell::Cell::new(None));
        let thread_result = result.clone();
        let rdram_ptr = rdram.as_mut_ptr();
        spawn_test_thread(302, 10, move || {
            let mut probe = ctx_zeroed();
            probe.r4 = MQ_VRAM;
            probe.r5 = RESULT_VRAM;
            unsafe { osPfsIsPlug_recomp(rdram_ptr, &mut probe) };
            thread_result.set(Some(probe.r2));
        });

        crate::run_to_idle();
        assert_eq!(result.get(), Some(1));
        assert_eq!(
            unsafe { storage.read_u8(RdramAddr::from_gpr(RESULT_VRAM)) },
            0xA5
        );
        let device = with_host(|host| host.device_fabric.evidence_snapshot());
        assert!(device.pending_si.is_some());
        assert!(device.si_dma_error);
    }

    #[test]
    fn pfs_is_plug_rejects_an_empty_queue_with_an_existing_blocked_receiver() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        with_host(|host| *host = HostState::default());
        const MQ_VRAM: u64 = 0x8000_0008;
        let mq_addr = RdramAddr::from_gpr(MQ_VRAM);
        with_executor(|exec| {
            exec.create_mesg_queue(mq_addr, 1);
            exec.set_event_mesg(5, mq_addr, 0xCAFE);
        });

        spawn_test_thread(303, 10, move || {
            let mut recv = ctx_zeroed();
            recv.r4 = MQ_VRAM;
            recv.r5 = 0;
            recv.r6 = 1;
            unsafe { crate::mesgqueue::osRecvMesg_recomp(std::ptr::null_mut(), &mut recv) };
        });
        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert_eq!(
            with_executor(|exec| exec.queue_activity(mq_addr)),
            Some(fn64_runtime::MesgQueueActivity {
                capacity: 1,
                valid_count: 0,
                blocked_receivers: 1,
                blocked_senders: 0,
            })
        );

        assert_eq!(
            pfs_is_plug_completion_message(mq_addr),
            Err(PfsIsPlugQueueError::SharedOrActive(
                fn64_runtime::MesgQueueActivity {
                    capacity: 1,
                    valid_count: 0,
                    blocked_receivers: 1,
                    blocked_senders: 0,
                }
            ))
        );
        with_executor(|exec| {
            exec.inject_event(fn64_runtime::ExternalEvent::DirectPost {
                queue_addr: mq_addr,
                msg: 0xCAFE,
            });
        });
        crate::run_to_idle();
    }

    #[test]
    fn pfs_is_plug_transaction_is_evidence_complete_before_and_after_si_completion() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        with_host(|host| *host = HostState::default());
        let mut rdram = vec![0u8; 0x80];
        unsafe { crate::register_process_rdram(rdram.as_mut_ptr(), rdram.len()) };
        let transaction = PfsIsPlugTransaction {
            thread: 304,
            queue: RdramAddr::from_offset(8),
            message: 0xCAFE,
            result_addr: RdramAddr::from_offset(0x60),
            bitpattern: 0b1010,
        };
        with_executor(|exec| exec.create_mesg_queue(transaction.queue, 1));
        crate::pi::start_live_si_dma(
            fn64_runtime::SiDmaRequest {
                kind: fn64_runtime::SiDmaKind::ControllerQuery,
                dram_addr: RdramAddr::from_offset(0),
            },
            PendingSiCompletionOwner::PfsIsPlug(transaction),
        )
        .unwrap();

        let pending = crate::host_evidence_snapshot();
        assert_eq!(
            pending.runtime_peripherals.pending_si_completion,
            Some(PendingSiCompletionEvidenceSnapshot {
                request: fn64_runtime::SiDmaRequest {
                    kind: fn64_runtime::SiDmaKind::ControllerQuery,
                    dram_addr: RdramAddr::from_offset(0),
                },
                owner: PendingSiCompletionOwnerEvidenceSnapshot::PfsIsPlug(transaction.into()),
            })
        );
        assert!(pending.runtime_peripherals.completed_pfs_is_plug.is_empty());

        crate::advance_virtual_time(1);
        let completed = crate::host_evidence_snapshot();
        assert!(completed
            .runtime_peripherals
            .pending_si_completion
            .is_none());
        assert_eq!(
            completed.runtime_peripherals.completed_pfs_is_plug,
            vec![transaction.into()]
        );
    }

    #[test]
    fn pfs_full_guest_lifecycle_round_trips_metadata_and_data() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        with_host(|host| host.save_operations.clear());
        crate::si::set_controller_port_state(
            0,
            fn64_runtime::PortState::StandardControllerControllerPak,
        );
        let mut rdram = vec![0u8; 0x400];
        let pfs = 0x8000_0040u64;
        let game_name = [
            0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20, 0x21, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let ext_name = [0x10, 0, 0, 0];
        logical_write(&mut rdram, 0x100, &game_name);
        logical_write(&mut rdram, 0x120, &ext_name);

        let mut init = ctx_zeroed();
        init.r4 = 0x8000_0020;
        init.r5 = pfs;
        init.r6 = 0;
        unsafe { osPfsInitPak_recomp(rdram.as_mut_ptr(), &mut init) };
        assert_eq!(init.r2, 0);

        let sp = 0x180usize;
        stack_word(&mut rdram, sp, 0x10, 0x8000_0120);
        stack_word(&mut rdram, sp, 0x14, 257);
        stack_word(&mut rdram, sp, 0x18, 0x8000_0140);
        let mut allocate = ctx_zeroed();
        allocate.r4 = pfs;
        allocate.r5 = 0x1234;
        allocate.r6 = 0x4142_4344;
        allocate.r7 = 0x8000_0100;
        allocate.r29 = 0x8000_0180;
        unsafe { osPfsAllocateFile_recomp(rdram.as_mut_ptr(), &mut allocate) };
        assert_eq!(allocate.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x140..0x144].try_into().unwrap()),
            0
        );

        let mut free = ctx_zeroed();
        free.r4 = pfs;
        free.r5 = 0x8000_0300;
        unsafe { osPfsFreeBlocks_recomp(rdram.as_mut_ptr(), &mut free) };
        assert_eq!(free.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x300..0x304].try_into().unwrap()),
            (fn64_runtime::pfs::PFS_CAPACITY - 512) as u32
        );

        let payload = [0x5Au8; 32];
        logical_write(&mut rdram, 0x200, &payload);
        stack_word(&mut rdram, sp, 0x10, 32);
        stack_word(&mut rdram, sp, 0x14, 0x8000_0200);
        let mut write = ctx_zeroed();
        write.r4 = pfs;
        write.r5 = 0;
        write.r6 = PFS_WRITE as u64;
        write.r7 = 32;
        write.r29 = 0x8000_0180;
        unsafe { osPfsReadWriteFile_recomp(rdram.as_mut_ptr(), &mut write) };
        assert_eq!(write.r2, 0);

        stack_word(&mut rdram, sp, 0x14, 0x8000_0240);
        let mut read = write;
        read.r6 = PFS_READ as u64;
        unsafe { osPfsReadWriteFile_recomp(rdram.as_mut_ptr(), &mut read) };
        assert_eq!(read.r2, 0);
        for (index, expected) in payload.into_iter().enumerate() {
            assert_eq!(rdram[(0x240 + index) ^ 3], expected);
        }
        assert_eq!(
            crate::copy_save_operations()
                .iter()
                .map(|event| (event.device, event.operation))
                .collect::<Vec<_>>(),
            vec![
                (
                    fn64_runtime::SaveType::ControllerPak,
                    fn64_runtime::SaveOperationKind::Write,
                ),
                (
                    fn64_runtime::SaveType::ControllerPak,
                    fn64_runtime::SaveOperationKind::Read,
                ),
            ]
        );

        let mut state = ctx_zeroed();
        state.r4 = pfs;
        state.r5 = 0;
        state.r6 = 0x8000_0280;
        unsafe { osPfsFileState_recomp(rdram.as_mut_ptr(), &mut state) };
        assert_eq!(state.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x280..0x284].try_into().unwrap()),
            512
        );
        assert_eq!(
            u32::from_ne_bytes(rdram[0x284..0x288].try_into().unwrap()),
            0x4142_4344
        );
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
        assert_eq!(
            unsafe { storage.read_u16(RdramAddr::from_offset(0x288)) },
            0x1234
        );

        stack_word(&mut rdram, sp, 0x10, 0x8000_0120);
        let mut delete = ctx_zeroed();
        delete.r4 = pfs;
        delete.r5 = 0x1234;
        delete.r6 = 0x4142_4344;
        delete.r7 = 0x8000_0100;
        delete.r29 = 0x8000_0180;
        unsafe { osPfsDeleteFile_recomp(rdram.as_mut_ptr(), &mut delete) };
        assert_eq!(delete.r2, 0);
    }

    #[test]
    fn pfs_init_distinguishes_no_pak_and_wrong_device() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        let mut rdram = vec![0u8; 0x100];
        let mut ctx = ctx_zeroed();
        ctx.r5 = 0x8000_0040;
        unsafe { osPfsInitPak_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, PFS_ERR_NOPACK as u64);
        crate::si::set_controller_port_state(
            0,
            fn64_runtime::PortState::StandardControllerRumblePak,
        );
        unsafe { osPfsInitPak_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, PFS_ERR_DEVICE as u64);
    }

    #[test]
    fn pfs_init_reports_inconsistent_when_both_raw_fat_copies_are_damaged() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        crate::si::set_controller_port_state(
            0,
            fn64_runtime::PortState::StandardControllerControllerPak,
        );
        with_executor(|exec| {
            let pak = exec
                .controller_pak_mut(0)
                .expect("configured Controller Pak");
            let damaged = [0xff; fn64_runtime::pfs::PFS_BLOCK_SIZE];
            pak.raw_write_block(fn64_runtime::pfs::PFS_PAGE_SIZE, &damaged)
                .unwrap();
            pak.raw_write_block(2 * fn64_runtime::pfs::PFS_PAGE_SIZE, &damaged)
                .unwrap();
        });

        let mut rdram = vec![0u8; 0x100];
        let mut ctx = ctx_zeroed();
        ctx.r5 = 0x8000_0040;
        ctx.r6 = 0;
        unsafe { osPfsInitPak_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, PFS_ERR_INCONSISTENT as u64);
        assert_eq!(u32::from_ne_bytes(rdram[0x40..0x44].try_into().unwrap()), 0);
    }

    #[test]
    fn pfs_free_blocks_uses_runtime_selected_banked_geometry() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        crate::si::set_controller_pak_bank_count(
            0,
            fn64_runtime::ControllerPakBankCount::new(2).unwrap(),
        );
        let mut rdram = vec![0u8; 0x100];
        let pfs = 0x8000_0040u64;
        let mut init = ctx_zeroed();
        init.r5 = pfs;
        init.r6 = 0;
        unsafe { osPfsInitPak_recomp(rdram.as_mut_ptr(), &mut init) };
        assert_eq!(init.r2, 0);

        let mut free = ctx_zeroed();
        free.r4 = pfs;
        free.r5 = 0x8000_0080;
        unsafe { osPfsFreeBlocks_recomp(rdram.as_mut_ptr(), &mut free) };
        assert_eq!(free.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x80..0x84].try_into().unwrap()),
            (248 * fn64_runtime::pfs::PFS_PAGE_SIZE) as u32
        );
    }
}
