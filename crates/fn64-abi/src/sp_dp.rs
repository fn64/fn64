use super::*;
use crate::ai::MMIO;

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
/// XBUS/freeze/flush, per public N64 hardware documentation). The command is
/// applied to the same `DpRegs` state returned by `osDpGetStatus` and raw
/// MMIO synchronization.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osDpSetStatus_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    MMIO.with(|cell| cell.borrow_mut().dp.apply_status_command(ctx.r4 as u32));
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
    if MMIO.with(|cell| cell.borrow_mut().dp.set_next_buffer(start, end)) {
        unsafe { crate::task_dispatch::dispatch_raw_rdp(rdram, start, end) };
        ctx.r2 = 0;
    } else {
        ctx.r2 = u64::MAX;
    }
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
    unsafe { &mut *ctx }.r2 = MMIO.with(|cell| cell.borrow().dp.status as u64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ctx_zeroed, install_complete_render_backend};
    use fn64_render::{RenderBackend, RenderConfig};

    #[test]
    fn dp_buffer_uses_o32_aligned_u64_and_reaches_idle_end_pointer() {
        MMIO.with(|cell| *cell.borrow_mut() = fn64_runtime::MmioSpace::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        install_complete_render_backend(rdram.len());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0xFFFF_FFFF_8000_1000;
        ctx.r5 = 0xDEAD_BEEF;
        ctx.r6 = 0;
        ctx.r7 = 0x80;
        unsafe { osDpSetNextBuffer_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        MMIO.with(|cell| {
            let mmio = cell.borrow();
            assert_eq!(mmio.dp.start, 0x1000);
            assert_eq!(mmio.dp.end, 0x1080);
            assert_eq!(mmio.dp.current, 0x1080);
            assert_eq!(mmio.dp.status, 0);
        });
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
        let mut backend = fn64_render_rt64::ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(4, 2)).unwrap();
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
        MMIO.with(|cell| *cell.borrow_mut() = fn64_runtime::MmioSpace::new());
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
        let mut backend = fn64_render_rt64::ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(4, 2)).unwrap();
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
        MMIO.with(|cell| *cell.borrow_mut() = fn64_runtime::MmioSpace::new());
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
}
