use super::*;

/// `__osSpSetPc(u32 pc)` -- `a0`=`ctx->r4` (verified: `funcs_57.c:1011`,
/// `ctx->r4 = 0 | 0;` immediately before the call -- a direct RSP-PC
/// register poke, part of the SP task-load sequence `osSpTaskLoad_recomp`
/// below also models). The value is latched in the shared raw-MMIO register
/// model so KSEG1 and libultra accesses observe the same PC.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSpSetPc_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let pc = unsafe { &*ctx }.r4 as u32;
    assert_eq!(
        pc & !0x0FFC,
        0,
        "__osSpSetPc_recomp: PC {pc:#x} is outside IMEM"
    );
    crate::pi::set_live_sp_pc(pc);
}

/// `__osSpSetStatus(u32 status)` -- `a0`=`ctx->r4` (verified:
/// `funcs_55.c:30`, `ctx->r4 = ADD32(0, 0x4082);` immediately before the
/// call). SP status writes are paired clear/set commands, applied by the
/// shared register model rather than stored as if they were status bits.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSpSetStatus_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let command = unsafe { &*ctx }.r4 as u32;
    crate::pi::write_live_sp_status(command);
}

/// `osDpSetStatus(u32 status)` -- `a0`=`ctx->r4` (verified: `funcs_55.c:22`,
/// `ctx->r4 = ADD32(0, 0x28);` immediately before the call). Real hardware
/// effect: writes the RDP `DP_STATUS` command register (clear/set flags for
/// XBUS/freeze/flush, plus the four counter-clear commands for the
/// clock/cmd/pipe/tmem performance counters, per public N64 hardware
/// documentation). The command is applied to the same `DpRegs` state returned
/// by `osDpGetStatus` and raw MMIO synchronization.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osDpSetStatus_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    assert!(
        crate::pi::write_live_device_mmio(0xFFFF_FFFF_A410_000C, ctx.r4 as u32),
        "osDpSetStatus_recomp: DPC_STATUS is not mapped"
    );
}

/// `osDpSetNextBuffer(void *bufPtr, u64 size) -> s32`. Under o32 the aligned
/// 64-bit second argument occupies `$a2:$a3` (`r6:r7`). The HLE render path is
/// synchronous, so a successful range is consumed before the guest resumes.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osDpSetNextBuffer_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let virtual_start = ctx.r4 as u32;
    let size = ((ctx.r6 as u32 as u64) << 32) | ctx.r7 as u32 as u64;
    assert!(
        virtual_start.is_multiple_of(8),
        "osDpSetNextBuffer_recomp: buffer {virtual_start:#010x} is not 8-byte aligned"
    );
    assert!(
        size.is_multiple_of(8),
        "osDpSetNextBuffer_recomp: size {size} is not a whole RDP command word"
    );
    let start = virtual_start & 0x1FFF_FFFF;
    let Ok(size) = u32::try_from(size) else {
        ctx.r2 = u64::MAX;
        return;
    };
    let Some(end) = start.checked_add(size) else {
        ctx.r2 = u64::MAX;
        return;
    };
    let submission = match with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            start,
            end,
        )
    }) {
        Ok(submission) => submission,
        Err(DeviceFault::DpBusy) => {
            ctx.r2 = u64::MAX;
            return;
        }
        Err(error) => panic!("osDpSetNextBuffer_recomp: {error}"),
    };
    if let Some(submission) = submission {
        unsafe { crate::task_dispatch::dispatch_dpc_submission(rdram, submission, Vec::new()) };
    }
    ctx.r2 = 0;
}

/// `__osSpGetStatus(void) -> u32` -- raw RCP `SP_STATUS_REG` read from the
/// same register model used by KSEG1 loads and the set-status shim.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSpGetStatus_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    unsafe { &mut *ctx }.r2 = crate::pi::live_sp_status() as u64;
}

/// `osDpGetStatus(void) -> u32` -- raw RCP `DP_STATUS_REG` read from the
/// shared DP register model.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osDpGetStatus_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    unsafe { &mut *ctx }.r2 = crate::pi::read_raw_mmio_word(0xFFFF_FFFF_A410_000C)
        .expect("osDpGetStatus_recomp: DPC_STATUS is not mapped")
        as u64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ctx_zeroed;
    use fn64_render::{FrameStatus, RenderBackend, RenderConfig, RenderError};
    use std::cell::Cell;
    use std::rc::Rc;

    struct CountingRawBackend(Rc<Cell<u32>>);

    impl RenderBackend for CountingRawBackend {
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
            Ok(FrameStatus::Complete)
        }

        fn process_rdp_commands(
            &mut self,
            _rdram: &mut [u8],
            _start: u32,
            _end: u32,
            _output_addr: u32,
            _wait_for_completion: bool,
        ) -> Result<FrameStatus, RenderError> {
            self.0.set(self.0.get() + 1);
            Ok(FrameStatus::Complete)
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

        fn supported_ucodes(&self) -> &[fn64_render::UcodeId] {
            &[]
        }
    }

    #[test]
    fn dp_buffer_uses_o32_aligned_u64_and_reaches_idle_end_pointer() {
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let calls = Rc::new(Cell::new(0));
        crate::set_render_backend(Box::new(CountingRawBackend(Rc::clone(&calls))), rdram.len());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0xFFFF_FFFF_8000_1000;
        ctx.r5 = 0xDEAD_BEEF;
        ctx.r6 = 0;
        ctx.r7 = 0x80;
        unsafe { osDpSetNextBuffer_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        assert_eq!(calls.get(), 1);
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert_eq!(snapshot.dpc_start, 0x1000);
            assert_eq!(snapshot.dpc_end, 0x1080);
            assert_eq!(snapshot.dpc_current, 0x1080);
            assert_eq!(snapshot.dpc_status, 0);
            assert_eq!(snapshot.pending_dpc, None);
        });
    }

    #[test]
    fn managed_dpc_buffer_is_invisible_until_freeze_clears() {
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let calls = Rc::new(Cell::new(0));
        crate::set_render_backend(Box::new(CountingRawBackend(Rc::clone(&calls))), rdram.len());
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });

        let mut freeze = ctx_zeroed();
        freeze.r4 = 0x08;
        unsafe { osDpSetStatus_recomp(rdram.as_mut_ptr(), &mut freeze) };

        let mut submit = ctx_zeroed();
        submit.r4 = 0xFFFF_FFFF_8000_1000;
        submit.r6 = 0;
        submit.r7 = 0x80;
        unsafe { osDpSetNextBuffer_recomp(rdram.as_mut_ptr(), &mut submit) };
        assert_eq!(submit.r2, 0, "a frozen END is accepted and retained");
        assert_eq!(calls.get(), 0, "FREEZE must prevent backend visibility");
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert_eq!(snapshot.dpc_current, 0x1000);
            assert_eq!(snapshot.dpc_end, 0x1080);
            assert_eq!(snapshot.pending_dpc, None);
            assert_ne!(snapshot.dpc_status & fn64_runtime::DPC_STATUS_FREEZE, 0);
        });

        let mut unfreeze = ctx_zeroed();
        unfreeze.r4 = 0x04;
        unsafe { osDpSetStatus_recomp(rdram.as_mut_ptr(), &mut unfreeze) };
        assert_eq!(
            calls.get(),
            1,
            "clearing FREEZE releases the retained range once"
        );
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert_eq!(snapshot.dpc_current, 0x1080);
            assert_eq!(snapshot.pending_dpc, None);
            assert_eq!(snapshot.dpc_status & fn64_runtime::DPC_STATUS_FREEZE, 0);
        });
    }

    #[test]
    fn raw_end_and_libultra_buffer_share_current_without_replay() {
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let calls = Rc::new(Cell::new(0));
        crate::set_render_backend(Box::new(CountingRawBackend(Rc::clone(&calls))), rdram.len());
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });

        assert!(crate::pi::write_raw_mmio_word(0xA410_0000, 0x100));
        assert!(crate::pi::write_raw_mmio_word(0xA410_0004, 0x108));
        assert_eq!(calls.get(), 1);
        assert_eq!(crate::pi::read_raw_mmio_word(0xA410_0008), Some(0x108));

        assert!(crate::pi::write_raw_mmio_word(0xA410_0004, 0x108));
        assert_eq!(calls.get(), 1, "an unchanged END must not replay the range");

        let mut managed = ctx_zeroed();
        managed.r4 = 0xFFFF_FFFF_8000_0108;
        managed.r6 = 0;
        managed.r7 = 8;
        unsafe { osDpSetNextBuffer_recomp(rdram.as_mut_ptr(), &mut managed) };
        assert_eq!(managed.r2, 0);
        assert_eq!(calls.get(), 2);
        let snapshot = with_host(|host| host.device_fabric.snapshot());
        assert_eq!(snapshot.dpc_start, 0x108);
        assert_eq!(snapshot.dpc_end, 0x110);
        assert_eq!(snapshot.dpc_current, 0x110);
        assert_eq!(snapshot.pending_dpc, None);
    }

    #[test]
    fn dp_buffer_executes_bounded_raw_rdp_commands_without_an_enddl_sentinel() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let commands: [(u32, u32); 4] = [
            (0xef00_0000 | (3 << 20), 0),           // fill cycle
            (0xff10_0003, TARGET),                  // RGBA16 width 4
            (0xf700_0000, 0xf801_f801),             // red fill register
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0), // inclusive 4x2 fill
        ];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = START + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        }
        let mut backend = fn64_render_reference::ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
        crate::set_render_backend(Box::new(backend), rdram.len());
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0xFFFF_FFFF_8000_0100;
        ctx.r6 = 0;
        ctx.r7 = (commands.len() * 8) as u64;
        unsafe { osDpSetNextBuffer_recomp(rdram.as_mut_ptr(), &mut ctx) };

        assert_eq!(ctx.r2, 0);
        assert_eq!(crate::last_render_error(), None);
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert!(!snapshot.sp_busy);
            assert!(
                !snapshot.dp_busy,
                "a raw DPC range without FullSync must not fabricate DP completion"
            );
        });
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for index in 0..8 {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + index * 2)),
                0xf801,
                "raw RDP pixel {index}"
            );
        }

        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for index in 0..8 {
                view.write_u16(fn64_runtime::RdramAddr::from_offset(TARGET + index * 2), 0);
            }
        }
        assert!(crate::pi::write_raw_mmio_word(
            0xFFFF_FFFF_A410_0000,
            START as u32
        ));
        assert!(crate::pi::write_raw_mmio_word(
            0xFFFF_FFFF_A410_0004,
            START as u32 + (commands.len() * 8) as u32
        ));
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for index in 0..8 {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + index * 2)),
                0xf801,
                "raw DPC MMIO pixel {index}"
            );
        }
    }

    #[test]
    fn raw_dpc_full_sync_schedules_dp_without_sp() {
        const START: usize = 0x100;
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        rdram[START..START + 4].copy_from_slice(&0xe900_0000u32.to_ne_bytes());
        let mut backend = fn64_render_reference::ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
        crate::load_rom_with_fixed_pi_latency(Vec::new(), 1);
        crate::set_render_backend(Box::new(backend), rdram.len());
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0xFFFF_FFFF_8000_0100;
        ctx.r6 = 0;
        ctx.r7 = 8;
        unsafe { osDpSetNextBuffer_recomp(rdram.as_mut_ptr(), &mut ctx) };

        assert_eq!(ctx.r2, 0);
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert!(!snapshot.sp_busy);
            assert!(snapshot.dp_busy);
            assert_eq!(snapshot.dpc_current, (START + 8) as u32);
            assert_eq!(snapshot.pending_dpc, None);
        });
        crate::advance_virtual_time(1);
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert!(!snapshot.sp_busy);
            assert!(!snapshot.dp_busy);
            assert_ne!(
                snapshot.mi_pending & fn64_runtime::InterruptSource::Dp.bit(),
                0
            );
            assert_eq!(
                snapshot.mi_pending & fn64_runtime::InterruptSource::Sp.bit(),
                0
            );
        });
    }

    #[test]
    fn dp_set_and_get_status_apply_public_command_pairs() {
        let mut set = ctx_zeroed();
        set.r4 = 0x02 | 0x08 | 0x20;
        unsafe { osDpSetStatus_recomp(std::ptr::null_mut(), &mut set) };
        let mut get = ctx_zeroed();
        unsafe { osDpGetStatus_recomp(std::ptr::null_mut(), &mut get) };
        assert_eq!(get.r2, 0x07);
    }

    #[test]
    fn sp_pc_and_status_shims_share_the_raw_register_model() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let mut pc = ctx_zeroed();
        pc.r4 = 0x0A8;
        unsafe { __osSpSetPc_recomp(std::ptr::null_mut(), &mut pc) };

        let mut set = ctx_zeroed();
        set.r4 = (1 << 0) | (1 << 2) | (1 << 10);
        unsafe { __osSpSetStatus_recomp(std::ptr::null_mut(), &mut set) };

        let mut get = ctx_zeroed();
        unsafe { __osSpGetStatus_recomp(std::ptr::null_mut(), &mut get) };
        assert_eq!(get.r2, 1 << 7);
        assert_eq!(
            crate::pi::read_raw_mmio_word(0xFFFF_FFFF_A408_0000),
            Some(0x0A8)
        );
    }

    #[test]
    fn sp_status_interrupt_commands_share_the_live_mi_source() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::pi::set_mi_interrupt_mask(fn64_runtime::InterruptSource::Sp.bit());
        let mut set = ctx_zeroed();
        set.r4 = 1 << 4;
        unsafe { __osSpSetStatus_recomp(std::ptr::null_mut(), &mut set) };
        assert!(crate::pi::cpu_interrupt_pending());

        set.r4 = 1 << 3;
        unsafe { __osSpSetStatus_recomp(std::ptr::null_mut(), &mut set) };
        assert!(!crate::pi::cpu_interrupt_pending());
    }

    // Seed the four DPC counters to 1,2,3,4 in the live device via the RSP
    // execution-state commit path (they are read-only over MMIO).
    fn seed_live_dpc_counters() {
        with_host(|host| {
            let mut state = host.device_fabric.rsp_execution_state();
            state.dpc_start = 0;
            state.dpc_end = 0;
            state.dpc_current = 0;
            state.dpc_status = 0;
            state.dpc_clock = 1;
            state.dpc_busy = 2;
            state.dpc_pipe_busy = 3;
            state.dpc_tmem_busy = 4;
            host.device_fabric
                .commit_complete_rsp_execution_state(state)
                .unwrap();
        });
    }

    fn live_dpc_counters() -> (u32, u32, u32, u32) {
        (
            crate::pi::read_raw_mmio_word(0xFFFF_FFFF_A410_0010).unwrap(),
            crate::pi::read_raw_mmio_word(0xFFFF_FFFF_A410_0014).unwrap(),
            crate::pi::read_raw_mmio_word(0xFFFF_FFFF_A410_0018).unwrap(),
            crate::pi::read_raw_mmio_word(0xFFFF_FFFF_A410_001C).unwrap(),
        )
    }

    #[test]
    fn dp_counter_clear_commands_converge_between_shim_and_raw_mmio() {
        // (command bit, expected (clock, busy, pipe, tmem) after clearing from 1,2,3,4)
        let cases = [
            (0x0200u32, (0, 2, 3, 4)), // CLEAR_CLOCK
            (0x0100u32, (1, 0, 3, 4)), // CLEAR_CMD -> busy
            (0x0080u32, (1, 2, 0, 4)), // CLEAR_PIPE
            (0x0040u32, (1, 2, 3, 0)), // CLEAR_TMEM
        ];
        for (command, expected) in cases {
            // Shim path (osDpSetStatus_recomp with a0 = command).
            crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
            seed_live_dpc_counters();
            let mut set = ctx_zeroed();
            set.r4 = command as u64;
            unsafe { osDpSetStatus_recomp(std::ptr::null_mut(), &mut set) };
            let via_shim = live_dpc_counters();
            assert_eq!(via_shim, expected, "shim path, command {command:#06x}");

            // Raw MMIO path (write DPC_STATUS directly).
            crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
            seed_live_dpc_counters();
            assert!(crate::pi::write_raw_mmio_word(
                0xFFFF_FFFF_A410_000C,
                command
            ));
            let via_raw = live_dpc_counters();
            assert_eq!(
                via_raw, via_shim,
                "shim and raw MMIO disagree for command {command:#06x}"
            );
        }
    }
}
