use super::*;
#[cfg(test)]
use crate::task_dispatch::AUDIO_OUTPUT_STATS;

const AI_MIN_DAC_RATE: u32 = 132;
const AI_MAX_BIT_RATE: u32 = 16;
const AI_CONTROL_REG: u64 = 0xA450_0008;
const AI_CONTROL_DMA_ON: u32 = 1;

/// `osAiSetFrequency(u32 frequency) -> s32` -- configures the audio DAC
/// sample rate and returns the TRUE playback rate, or -1 if the frequency is
/// unusable (`dacRate < AI_MIN_DAC_RATE`). The true DAC rate is stored as
/// host state AND forwarded to any registered `AudioBackend` via
/// `set_frequency`, so the backend's producer-side resample ratio tracks
/// the game (the host stream itself is opened by the shell/harness at
/// startup and keeps its negotiated device rate). The s32 return is
/// load-bearing: the only decomp caller (heap.c:966) assigns it to
/// `aiSamplingFrequency` and then DIVIDES by it (heap.c:1002), so a
/// stale/zero $v0 divides by garbage.
///
/// The public AI register contract requires both DACRATE and BITRATE:
/// `dac_rate = (osViClock/freq + 0.5)`, `dac_rate < 132` returns -1, and the
/// serial bit-clock divider is `min(dac_rate / 66, 16)`. The registers encode
/// each divider minus one. `osViClock` is selected from the public
/// NTSC/PAL/MPAL clock constants by the IPL television type. Single u32 arg
/// in $a0=r4, return $v0=r2.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osAiSetFrequency_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let freq = ctx.r4 as u32;
    AI_FREQUENCY.with(|cell| cell.set(freq));

    let vi_clock = crate::vi_clock_hz();
    let dac_rate = (vi_clock as f32 / freq as f32 + 0.5) as u32;
    ctx.r2 = if dac_rate < AI_MIN_DAC_RATE {
        -1i32 as u32 as u64
    } else {
        let true_rate = vi_clock / dac_rate;
        let bit_rate = (dac_rate / 66).min(AI_MAX_BIT_RATE);
        crate::pi::set_live_ai_rates(dac_rate - 1, bit_rate - 1)
            .unwrap_or_else(|error| panic!("osAiSetFrequency_recomp: {error}"));
        true_rate as u64
    };
}

thread_local! {
    static AI_FREQUENCY: Cell<u32> = const { Cell::new(0) };
    static AI_STATUS_READS: Cell<u64> = const { Cell::new(0) };
    static AI_STATUS_BUSY_RETURNS: Cell<u64> = const { Cell::new(0) };
    static AI_LENGTH_READS: Cell<u64> = const { Cell::new(0) };
    static AI_LENGTH_LAST: Cell<u32> = const { Cell::new(0) };
    /// Compatibility backing for RCP registers not yet owned by
    /// `DeviceFabric`. AI and DPC never consult this state: their raw and
    /// libultra paths share the fabric's typed transactions.
    pub(crate) static MMIO: RefCell<fn64_runtime::MmioSpace> = RefCell::new(fn64_runtime::MmioSpace::new());
}

/// Write every modeled MMIO register's current value into `rdram`'s real
/// bytes, so a subsequent RAW guest load (not going through any
/// `osXxx_recomp` shim) observes it. Exposed for a harness
/// (`examples/wm2000-boot`) to call right after allocating a
/// `Rdram::new_with_mmio`-sized buffer and before/between coroutine resumes
/// -- see `fn64_runtime::mmio::MmioSpace::sync_into_rdram`'s doc comment
/// for exactly when this needs to be called (after any host mutation of
/// the model, e.g. right after this file's own `osAiSetNextBuffer_recomp`).
///
/// # Safety
/// `rdram` must point to a buffer of at least
/// `fn64_runtime::RDRAM_MMIO_WINDOW_END` bytes (i.e. allocated via
/// `Rdram::new_with_mmio`, not plain `Rdram::new`/a bare `Vec::new`).
pub unsafe fn sync_mmio_into_rdram(rdram: *mut u8) {
    MMIO.with(|cell| unsafe { cell.borrow_mut().sync_into_rdram(rdram) });
    unsafe { crate::pi::sync_live_ai_dpc_mmio_into_rdram(rdram) };
}

/// `osAiGetStatus() -> u32` -- no arguments; real hardware `AI_STATUS`
/// register read (`AI_STATUS_BUSY`/`AI_STATUS_FULL` bits, public libultra
/// manual's AI Manager section). The authoritative `DeviceFabric` owns the
/// two-slot DMA FIFO and control latch, so this shim and a raw register load
/// return the same nonmutating status snapshot.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osAiGetStatus_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let status = crate::pi::live_ai_status();
    AI_STATUS_READS.with(|cell| cell.set(cell.get() + 1));
    if status & fn64_runtime::AI_STATUS_BUSY != 0 {
        AI_STATUS_BUSY_RETURNS.with(|cell| cell.set(cell.get() + 1));
    }
    ctx.r2 = status as u64;
}

pub fn ai_status_stats() -> (u64, u64) {
    (
        AI_STATUS_READS.with(Cell::get),
        AI_STATUS_BUSY_RETURNS.with(Cell::get),
    )
}

/// `osAiGetLength() -> u32` -- no arguments; real hardware `AI_LEN` register
/// read (bytes remaining in the current/last DMA, counting DOWN as the DAC
/// drains). The authoritative `DeviceFabric` derives this only from guest
/// cycles and the active DMA deadline. Host playback telemetry and prebuffer
/// depth never manufacture guest-visible device work.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osAiGetLength_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let len = crate::pi::live_ai_length();
    AI_LENGTH_READS.with(|cell| cell.set(cell.get() + 1));
    AI_LENGTH_LAST.with(|cell| cell.set(len));
    ctx.r2 = len as u64;
}

pub fn ai_length_stats() -> (u64, u32) {
    (
        AI_LENGTH_READS.with(Cell::get),
        AI_LENGTH_LAST.with(Cell::get),
    )
}

/// `osAiSetNextBuffer(void *buf, u32 size) -> s32` -- `buf`=`ctx->r4` (an
/// rdram-relative vram pointer to the finished interleaved stereo PCM),
/// `size`=`ctx->r5` (bytes). Real hardware effect: latches the DMA
/// source/length and starts the transfer; per the public libultra manual,
/// returns 0 on success or
/// a negative error code if a DMA is already in progress and the queue is
/// full. The authoritative `DeviceFabric` owns the timed two-slot FIFO, so an
/// accepted request returns 0 and a request made while both slots are occupied
/// returns -1 without mutating the latched DRAM address.
///
/// The public `rcp.h` AI-control definition makes bit 0 the DMA enable. This
/// shim performs that public enable transition through the same typed MMIO
/// path as a raw guest write before it latches DRAM/LEN; fn64 does not invent
/// an earlier hidden AI-control write during `osInitialize`. The FIFO-full
/// rejection remains side-effect free, including leaving CONTROL unchanged.
///
/// This is also the one correct host-output boundary. A live OoT `M_AUDTASK`
/// has `OSTask.output_buff == 0`/`output_buff_size == 0`; its `A_SAVEBUFF`
/// commands write PCM to buffers selected inside the command list, and the CPU
/// later names the completed buffer here. Routing `OSTask.output_buff` from the
/// RSP dispatch therefore queued zero samples forever. Delivering this exact
/// AI DMA range to `AudioBackend` follows the public AI contract and covers
/// every producer without parsing game-specific audio commands.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osAiSetNextBuffer_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let buf_addr = RdramAddr::from_gpr(ctx.r4).offset();
    let size = ctx.r5 as u32;
    if crate::pi::live_ai_status() & fn64_runtime::AI_STATUS_FULL != 0 {
        ctx.r2 = u64::MAX;
        return;
    }
    assert!(
        crate::pi::write_raw_mmio_word(AI_CONTROL_REG, AI_CONTROL_DMA_ON),
        "osAiSetNextBuffer_recomp: authoritative AI_CONTROL register is unmapped"
    );
    let accepted = match unsafe { crate::pi::submit_live_ai_dma(rdram, buf_addr, size) } {
        Ok(()) => true,
        Err(DeviceFault::AiFull) => false,
        Err(error) => panic!("osAiSetNextBuffer_recomp: {error}"),
    };
    if !accepted {
        ctx.r2 = u64::MAX;
        return;
    }
    ctx.r2 = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    fn configure_ntsc() {
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
    }

    #[test]
    fn os_ai_set_frequency_stores_value() {
        configure_ntsc();
        let mut ctx = ctx_zeroed();
        ctx.r4 = 48000;
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(AI_FREQUENCY.with(|c| c.get()), 48000);
    }

    #[test]
    fn os_ai_set_next_buffer_stays_busy_until_guest_cycle_completion() {
        configure_ntsc();

        let mut set_ctx = ctx_zeroed();
        set_ctx.r4 = 0xFFFF_FFFF_8010_0000; // buf, a plausible vram address
        set_ctx.r5 = 0x100; // size
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut set_ctx as *mut _) };
        assert_eq!(set_ctx.r2, 0, "osAiSetNextBuffer reports success");

        let mut status_ctx = ctx_zeroed();
        unsafe { osAiGetStatus_recomp(std::ptr::null_mut(), &mut status_ctx as *mut _) };
        assert_eq!(
            status_ctx.r2 as u32 & fn64_runtime::AI_STATUS_BUSY,
            fn64_runtime::AI_STATUS_BUSY,
            "first status read after a submit observes busy"
        );

        let mut second_ctx = ctx_zeroed();
        unsafe { osAiGetStatus_recomp(std::ptr::null_mut(), &mut second_ctx as *mut _) };
        assert_ne!(
            second_ctx.r2 as u32 & fn64_runtime::AI_STATUS_BUSY,
            0,
            "status reads do not fabricate DMA completion"
        );
        let deadline = with_host(|host| host.device_fabric.next_deadline().unwrap().get());
        crate::advance_virtual_time(deadline);
        let mut completed_ctx = ctx_zeroed();
        unsafe { osAiGetStatus_recomp(std::ptr::null_mut(), &mut completed_ctx as *mut _) };
        assert_eq!(completed_ctx.r2 as u32, fn64_runtime::AI_STATUS_ENABLED);
    }

    #[test]
    fn os_ai_get_length_reports_latched_length() {
        configure_ntsc();

        let mut set_ctx = ctx_zeroed();
        set_ctx.r4 = 0xFFFF_FFFF_8010_0000;
        set_ctx.r5 = 0x40;
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut set_ctx as *mut _) };

        let mut ctx = ctx_zeroed();
        unsafe { osAiGetLength_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0x40);
    }

    #[test]
    fn live_ai_fifo_drains_before_event_delivery_and_raises_mi() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        configure_ntsc();
        crate::pi::set_mi_interrupt_mask(fn64_runtime::InterruptSource::Ai.bit());
        let queue = RdramAddr::from_offset(0x300);
        with_executor(|exec| {
            exec.create_mesg_queue(queue, 2);
            exec.set_event_mesg(6, queue, 0xA1);
        });

        let mut frequency = ctx_zeroed();
        frequency.r4 = 32_000;
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut frequency) };
        assert_eq!(frequency.r2, 32_006);

        let mut first = ctx_zeroed();
        first.r4 = 0xFFFF_FFFF_8000_1000;
        first.r5 = 0x80;
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut first) };
        let mut second = ctx_zeroed();
        second.r4 = 0xFFFF_FFFF_8000_2000;
        second.r5 = 0x80;
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut second) };
        let mut full = ctx_zeroed();
        full.r4 = 0xFFFF_FFFF_8000_3000;
        full.r5 = 0x80;
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut full) };
        assert_eq!(full.r2, u64::MAX);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().ai_dram_addr.offset()),
            0x2000,
            "a rejected libultra submit must not replace the accepted next-buffer latch"
        );
        assert_eq!(
            crate::pi::live_ai_status(),
            fn64_runtime::AI_STATUS_ENABLED
                | fn64_runtime::AI_STATUS_BUSY
                | fn64_runtime::AI_STATUS_FULL
        );

        let first_deadline = with_host(|host| host.device_fabric.next_deadline().unwrap().get());
        crate::advance_virtual_time(first_deadline - 1);
        assert!(crate::pi::live_ai_length() > 0);
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, queue, false)),
            fn64_runtime::RecvMesgOutcome::WouldBlock
        );

        crate::advance_virtual_time(first_deadline);
        assert!(crate::pi::cpu_interrupt_pending());
        assert_eq!(
            crate::pi::live_ai_status(),
            fn64_runtime::AI_STATUS_ENABLED | fn64_runtime::AI_STATUS_BUSY
        );
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, queue, false)),
            fn64_runtime::RecvMesgOutcome::Delivered(0xA1)
        );
        crate::pi::clear_device_interrupt(fn64_runtime::InterruptSource::Ai);

        let second_deadline = with_host(|host| host.device_fabric.next_deadline().unwrap().get());
        crate::advance_virtual_time(second_deadline);
        assert!(crate::pi::cpu_interrupt_pending());
        assert_eq!(crate::pi::live_ai_status(), fn64_runtime::AI_STATUS_ENABLED);
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, queue, false)),
            fn64_runtime::RecvMesgOutcome::Delivered(0xA1)
        );
    }

    #[test]
    fn raw_and_libultra_ai_submissions_share_one_future_state() {
        fn reset() {
            crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
            crate::load_rom_with_fixed_pi_latency(Vec::new(), 1);
        }

        reset();
        assert!(crate::pi::write_raw_mmio_word(
            AI_CONTROL_REG,
            AI_CONTROL_DMA_ON
        ));
        assert!(crate::pi::write_raw_mmio_word(0xA450_0010, 1_520));
        assert!(crate::pi::write_raw_mmio_word(0xA450_0014, 15));
        assert!(crate::pi::write_raw_mmio_word(0xA450_0000, 0x2_000));
        assert!(crate::pi::write_raw_mmio_word(0xA450_0004, 0x80));
        let raw = crate::device_evidence_snapshot();

        reset();
        let mut frequency = ctx_with(32_000, 0, 0);
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut frequency) };
        let mut submit = ctx_with(0xFFFF_FFFF_8000_2000, 0x80, 0);
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut submit) };
        let managed = crate::device_evidence_snapshot();

        assert_eq!(frequency.r2, 32_006);
        assert_eq!(submit.r2, 0);
        assert_eq!(managed.guest.ai_bitrate, 15);
        assert_eq!(managed, raw);
    }

    #[test]
    fn control_off_raw_and_libultra_submissions_share_the_enable_transition() {
        fn reset() {
            crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
            crate::load_rom_with_fixed_pi_latency(Vec::new(), 1);
            assert!(crate::pi::write_raw_mmio_word(AI_CONTROL_REG, 0));
            assert_eq!(
                with_host(|host| host.device_fabric.snapshot().ai_control),
                0
            );
        }

        reset();
        assert!(crate::pi::write_raw_mmio_word(
            AI_CONTROL_REG,
            AI_CONTROL_DMA_ON
        ));
        assert!(crate::pi::write_raw_mmio_word(0xA450_0000, 0x2_000));
        assert!(crate::pi::write_raw_mmio_word(0xA450_0004, 0x80));
        let raw = crate::device_evidence_snapshot();

        reset();
        let mut submit = ctx_with(0xFFFF_FFFF_8000_2000, 0x80, 0);
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut submit) };
        let managed = crate::device_evidence_snapshot();

        assert_eq!(submit.r2, 0);
        assert_eq!(managed.guest.ai_control, AI_CONTROL_DMA_ON);
        assert_eq!(managed, raw);
    }

    #[test]
    fn sync_mmio_into_rdram_backs_a_raw_guest_ai_status_load() {
        configure_ntsc();
        let mut set_ctx = ctx_zeroed();
        set_ctx.r4 = 0xFFFF_FFFF_8010_0000;
        set_ctx.r5 = 0x40;
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut set_ctx as *mut _) };

        let mut buf = vec![0u8; fn64_runtime::RDRAM_MMIO_WINDOW_END as usize];
        unsafe { sync_mmio_into_rdram(buf.as_mut_ptr()) };

        // The exact real address docs/BOOT-NOTES-WM2000.md's LLDB backtrace
        // named: a raw guest lw at AI_STATUS (0xA450000C).
        let ai_status = RdramAddr::from_gpr(0xA450_000C);
        let o = ai_status.offset() as usize;
        let raw = i32::from_ne_bytes(buf[o..o + 4].try_into().unwrap());
        assert_eq!(
            raw as u32 & fn64_runtime::AI_STATUS_BUSY,
            fn64_runtime::AI_STATUS_BUSY
        );
    }

    /// Fail-against-bug: OoT's live audio OSTask has zero
    /// `output_buff`/`output_buff_size`; aspMain's `A_SAVEBUFF` commands choose
    /// the PCM destinations, and the CPU later submits the completed range to
    /// AI through `osAiSetNextBuffer`. Routing the OSTask fields queued an
    /// empty slice forever. This exact zero-field task shape must not reach the
    /// backend, while the subsequent AI DMA must preserve sample/channel order.
    #[test]
    fn os_ai_set_next_buffer_routes_live_pcm_to_the_registered_audio_backend() {
        use std::sync::{Arc, Mutex};

        /// A cpal-less fake `AudioBackend` -- proves the seam really
        /// reaches a registered `dyn AudioBackend` (not just that
        /// `AUDIO_UCODE_FN` was called), mirroring
        /// `render_backend_dyn_dispatch_reaches_the_registered_reference_backend`'s
        /// shape for the gfx seam. Not `fn64_audio::CpalBackend` itself --
        /// a real audio device isn't guaranteed in a test/CI sandbox.
        struct CountingBackend {
            ready: bool,
            samples_seen: Arc<Mutex<Vec<i16>>>,
        }
        impl AudioBackend for CountingBackend {
            fn create(
                &mut self,
                _cfg: &fn64_audio::AudioConfig,
            ) -> Result<(), fn64_audio::AudioError> {
                self.ready = true;
                Ok(())
            }
            fn queue_samples(&mut self, samples: &[i16]) -> Result<(), fn64_audio::AudioError> {
                if !self.ready {
                    return Err(fn64_audio::AudioError::NotReady("create() not called"));
                }
                self.samples_seen
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .extend_from_slice(samples);
                Ok(())
            }
            fn frames_remaining(&self) -> Result<u32, fn64_audio::AudioError> {
                Ok((self
                    .samples_seen
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .len()
                    / 2) as u32)
            }
            fn set_frequency(&mut self, _sample_rate_hz: u32) {}
        }

        const RDRAM_LEN: usize = 4096;
        configure_ntsc();
        let mut rdram = vec![0u8; RDRAM_LEN];
        const HEADER_OFF: usize = 0x40;
        const AI_BUF_OFF: usize = 0x800; // arbitrary in-bounds output buffer
        const EXPECTED: [i16; 8] = [-1000, 1000, -500, 500, -250, 250, -125, 125];
        rdram[HEADER_OFF..HEADER_OFF + 4].copy_from_slice(&fn64_runtime::M_AUDTASK.to_ne_bytes());
        // output_buff/output_buff_size remain zero, matching the captured live
        // task at rdram+0x120c90. Seed PCM in fn64's native-word backing order.
        for (index, sample) in EXPECTED.iter().enumerate() {
            let physical = (AI_BUF_OFF + index * 2) ^ 2;
            rdram[physical..physical + 2].copy_from_slice(&sample.to_ne_bytes());
        }

        let samples_seen = Arc::new(Mutex::new(Vec::new()));
        let mut backend = CountingBackend {
            ready: false,
            samples_seen: Arc::clone(&samples_seen),
        };
        backend
            .create(&fn64_audio::AudioConfig::new(32000, 2))
            .unwrap();
        set_audio_backend(Box::new(backend), RDRAM_LEN);

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert!(
            samples_seen.lock().unwrap().is_empty(),
            "zero-sized OSTask output fields are not the AI PCM boundary"
        );

        AUDIO_OUTPUT_STATS.with(|cell| cell.set(AudioOutputStats::new()));
        let mut ai_ctx = ctx_zeroed();
        ai_ctx.r4 = 0x8000_0000 + AI_BUF_OFF as u64;
        ai_ctx.r5 = (EXPECTED.len() * 2) as u64;
        unsafe { osAiSetNextBuffer_recomp(rdram.as_mut_ptr(), &mut ai_ctx as *mut _) };

        assert_eq!(
            *samples_seen.lock().unwrap(),
            EXPECTED,
            "AI delivery must preserve the guest's interleaved sample order"
        );
        assert_eq!(
            last_audio_error(),
            None,
            "the registered backend accepted the real AI range"
        );
        assert_eq!(
            audio_output_stats(),
            AudioOutputStats {
                ai_buffers: 1,
                backend_buffers: 1,
                samples: EXPECTED.len() as u64,
                nonzero_samples: EXPECTED.len() as u64,
                min: Some(-1000),
                max: Some(1000),
            }
        );
    }

    /// osAiSetFrequency must write the true DAC playback rate to $v0
    /// (aisetfreq.c: osViClock / dacRate), or -1 for an unusably-low rate.
    /// Fails against the bug (never writes r2): stale $v0 survives.
    #[test]
    fn os_ai_set_frequency_returns_true_dac_rate_in_v0() {
        configure_ntsc();
        // 32000 Hz: dacRate = round(48681812/32000) = round(1521.3) = 1521;
        // true rate = 48681812 / 1521 = 32006. Distinct from the input.
        let mut ctx = ctx_zeroed();
        ctx.r4 = 32000;
        ctx.r2 = 0xBADD_C0DE; // stale $v0 the bug would leave in place.
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 32006, "true DAC rate 48681812/1521");
        let snapshot = with_host(|host| host.device_fabric.snapshot());
        assert_eq!(snapshot.ai_dacrate, 1_520);
        assert_eq!(snapshot.ai_bitrate, 15);

        // An unusably-high frequency drives dacRate below AI_MIN_DAC_RATE
        // (132) and must return -1: freq 400000 -> dacRate = round(121.7) =
        // 122 < 132.
        let mut ctx = ctx_with(400_000, 0, 0);
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, -1i32 as u32 as u64, "dacRate < 132 -> -1");
    }

    #[test]
    fn os_ai_set_frequency_uses_the_ipl_selected_video_clock() {
        crate::configure_tv_type(fn64_runtime::TvType::Pal);
        let mut pal = ctx_with(32_000, 0, 0);
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut pal) };
        assert_eq!(pal.r2, 31_995);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().ai_dacrate),
            1_551
        );

        crate::configure_tv_type(fn64_runtime::TvType::Mpal);
        let mut mpal = ctx_with(32_000, 0, 0);
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut mpal) };
        assert_eq!(mpal.r2, 31_992);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().ai_dacrate),
            1_519
        );

        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
    }

    /// A successful osAiSetFrequency must forward the TRUE DAC rate to the
    /// registered backend's `set_frequency` (the producer-side resample
    /// ratio), and a failed one (-1) must not. Fails against the bug where
    /// the shim only stored host state and the backend ratio went stale.
    #[test]
    fn os_ai_set_frequency_forwards_true_rate_to_backend() {
        use std::sync::{Arc, Mutex};

        struct RateRecorder {
            rates: Arc<Mutex<Vec<u32>>>,
        }
        impl AudioBackend for RateRecorder {
            fn create(
                &mut self,
                _cfg: &fn64_audio::AudioConfig,
            ) -> Result<(), fn64_audio::AudioError> {
                Ok(())
            }
            fn queue_samples(&mut self, _samples: &[i16]) -> Result<(), fn64_audio::AudioError> {
                Ok(())
            }
            fn frames_remaining(&self) -> Result<u32, fn64_audio::AudioError> {
                Ok(0)
            }
            fn set_frequency(&mut self, sample_rate_hz: u32) {
                self.rates
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(sample_rate_hz);
            }
        }

        configure_ntsc();
        let rates = Arc::new(Mutex::new(Vec::new()));
        crate::set_audio_backend(
            Box::new(RateRecorder {
                rates: Arc::clone(&rates),
            }),
            4096,
        );

        let mut ctx = ctx_with(32_000, 0, 0);
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        let mut ctx = ctx_with(400_000, 0, 0); // -1 path: must NOT forward
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };

        assert_eq!(
            *rates.lock().unwrap_or_else(|error| error.into_inner()),
            vec![32006],
            "exactly one forward, carrying the true DAC rate"
        );
    }

    /// With a live backend but no active hardware DMA, AI_LEN is zero. Neither
    /// the backend's current-buffer telemetry nor its host prebuffer can
    /// manufacture guest-visible device work.
    #[test]
    fn os_ai_get_length_reports_drain_aware_guest_bytes() {
        struct RingBackend {
            dma_bytes: u32,
        }
        impl AudioBackend for RingBackend {
            fn create(
                &mut self,
                _cfg: &fn64_audio::AudioConfig,
            ) -> Result<(), fn64_audio::AudioError> {
                Ok(())
            }
            fn queue_samples(&mut self, _samples: &[i16]) -> Result<(), fn64_audio::AudioError> {
                Ok(())
            }
            fn frames_remaining(&self) -> Result<u32, fn64_audio::AudioError> {
                Ok(4800)
            }
            fn set_frequency(&mut self, _sample_rate_hz: u32) {}
            fn stream_rate_hz(&self) -> Option<u32> {
                Some(48_000)
            }
            fn current_dma_bytes_remaining(&self) -> Option<u32> {
                Some(self.dma_bytes)
            }
        }

        crate::set_audio_backend(Box::new(RingBackend { dma_bytes: 1536 }), 4096);
        let mut ctx = ctx_zeroed();
        unsafe { osAiGetLength_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0, "idle AI_LEN is owned by the device fabric");
    }
}
