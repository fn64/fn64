use super::*;
#[cfg(test)]
use crate::task_dispatch::AUDIO_OUTPUT_STATS;
use crate::task_dispatch::{deliver_ai_buffer, AUDIO_RDRAM_LEN};

/// `osAiSetFrequency(u32 frequency) -> s32` -- configures the audio DAC
/// sample rate and returns the TRUE playback rate, or -1 if the frequency is
/// unusable (`dacRate < AI_MIN_DAC_RATE`). No audio backend exists yet, so the
/// frequency is stored as host state, while the host output stream is opened
/// by its shell/harness at startup. The s32 return is load-bearing: the
/// only decomp caller (heap.c:966) assigns it to `aiSamplingFrequency` and
/// then DIVIDES by it (heap.c:1002), so a stale/zero $v0 divides by garbage.
///
/// Byte-exact to aisetfreq.c:12-36: `dacRate = (osViClock/freq + 0.5)`;
/// `dacRate < AI_MIN_DAC_RATE(132)` -> -1; else `osViClock / (s32)dacRate`.
/// `osViClock == VI_NTSC_CLOCK == 48681812` (rcp.h:538), AI_MIN_DAC_RATE=132
/// (rcp.h:587). Single u32 arg in $a0=r4, return $v0=r2.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osAiSetFrequency_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let freq = ctx.r4 as u32;
    AI_FREQUENCY.with(|cell| cell.set(freq));

    // osViClock: the NTSC video clock (aisetfreq.c's `extern s32 osViClock`),
    // which libultra initializes to VI_NTSC_CLOCK for an NTSC console.
    const VI_NTSC_CLOCK: i32 = 48_681_812; // rcp.h:538
    const AI_MIN_DAC_RATE: u32 = 132; // rcp.h:587
    let dac_rate = (VI_NTSC_CLOCK as f32 / freq as f32 + 0.5) as u32;
    ctx.r2 = if dac_rate < AI_MIN_DAC_RATE {
        -1i32 as u32 as u64
    } else {
        (VI_NTSC_CLOCK / dac_rate as i32) as u32 as u64
    };
}

thread_local! {
    static AI_FREQUENCY: Cell<u32> = const { Cell::new(0) };
    /// AI/VI/PI/SI/SP/DP/MI hardware-register model (`fn64_runtime::mmio`).
    /// Backs both the shim-level `osAi*` family below AND (via
    /// `sync_mmio_into_rdram`, called from `boot_thread0`/before each
    /// coroutine resume) a raw guest `MEM_W` load at the same address --
    /// see `mmio.rs`'s module doc for the real crash
    /// (`docs/BOOT-NOTES-WM2000.md`) this closes.
    static MMIO: RefCell<fn64_runtime::MmioSpace> = RefCell::new(fn64_runtime::MmioSpace::new());
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
}

/// `osAiGetStatus() -> u32` -- no arguments; real hardware `AI_STATUS`
/// register read (`AI_STATUS_BUSY`/`AI_STATUS_FULL` bits, public libultra
/// manual's AI Manager section). Backed by `fn64_runtime::mmio::AiRegs`,
/// the same model a raw guest `MEM_W` at the register's real address reads
/// (see `MMIO`'s doc comment) -- this shim and a raw load return the SAME
/// value, since both go through `AiRegs::status`'s one-shot-busy logic
/// (this call also mutates the one-shot flag, exactly like a real register
/// read would consume the interrupt-pending latch).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osAiGetStatus_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let status = MMIO.with(|cell| cell.borrow_mut().ai.status());
    ctx.r2 = status as u64;
}

/// `osAiGetLength() -> u32` -- no arguments; real hardware `AI_LEN` register
/// read (bytes remaining in the current/last DMA). See `AiRegs::length`'s
/// doc comment for why this crate reports the full latched length rather
/// than a fabricated mid-drain value.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osAiGetLength_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let len = MMIO.with(|cell| cell.borrow().ai.length());
    ctx.r2 = len as u64;
}

/// `osAiSetNextBuffer(void *buf, u32 size) -> s32` -- `buf`=`ctx->r4` (an
/// rdram-relative vram pointer to the finished interleaved stereo PCM),
/// `size`=`ctx->r5` (bytes). Real hardware effect: latches the DMA
/// source/length and starts the transfer; per the public libultra manual,
/// returns 0 on success or
/// a negative error code if a DMA is already in progress and the queue is
/// full. This crate's DMA is synchronous-modeled (see `AiRegs::set_next_buffer`'s
/// doc comment: "DMA proceeds" stance, same as `rom.rs`'s PI DMA), so this
/// always succeeds (returns 0) -- no evidence yet of a call site needing the
/// error path.
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
    MMIO.with(|cell| cell.borrow_mut().ai.set_next_buffer(buf_addr, size));
    // Host audio is optional. Once a shell/harness registers a bound, every
    // submitted AI range must satisfy it loudly; an unset bound means no host
    // consumer was requested, not that a malformed configured range is okay.
    if AUDIO_RDRAM_LEN.with(Cell::get) != 0 {
        unsafe { deliver_ai_buffer(rdram, buf_addr as usize, size as usize) };
    }
    ctx.r2 = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    #[test]
    fn os_ai_set_frequency_stores_value() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 48000;
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(AI_FREQUENCY.with(|c| c.get()), 48000);
    }

    #[test]
    fn os_ai_set_next_buffer_then_get_status_reports_busy_once() {
        // Reset MMIO's AI state to a known starting point -- other tests in
        // this file share the same thread_local, and test order is not
        // guaranteed.
        MMIO.with(|cell| *cell.borrow_mut() = fn64_runtime::MmioSpace::new());

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
        assert_eq!(second_ctx.r2, 0, "busy is one-shot");
    }

    #[test]
    fn os_ai_get_length_reports_latched_length() {
        MMIO.with(|cell| *cell.borrow_mut() = fn64_runtime::MmioSpace::new());

        let mut set_ctx = ctx_zeroed();
        set_ctx.r4 = 0xFFFF_FFFF_8010_0000;
        set_ctx.r5 = 0x40;
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut set_ctx as *mut _) };

        let mut ctx = ctx_zeroed();
        unsafe { osAiGetLength_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0x40);
    }

    #[test]
    fn sync_mmio_into_rdram_backs_a_raw_guest_ai_status_load() {
        MMIO.with(|cell| *cell.borrow_mut() = fn64_runtime::MmioSpace::new());
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
        // 32000 Hz: dacRate = round(48681812/32000) = round(1521.3) = 1521;
        // true rate = 48681812 / 1521 = 32006. Distinct from the input.
        let mut ctx = ctx_zeroed();
        ctx.r4 = 32000;
        ctx.r2 = 0xBADD_C0DE; // stale $v0 the bug would leave in place.
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 32006, "true DAC rate 48681812/1521");

        // An unusably-high frequency drives dacRate below AI_MIN_DAC_RATE
        // (132) and must return -1: freq 400000 -> dacRate = round(121.7) =
        // 122 < 132.
        let mut ctx = ctx_with(400_000, 0, 0);
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, -1i32 as u32 as u64, "dacRate < 132 -> -1");
    }
}
