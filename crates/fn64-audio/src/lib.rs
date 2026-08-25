//! `fn64-audio`: the audio backend seam, per `docs/DECOUPLING.md`'s crate
//! plan ("**fn64-audio** — the `AudioBackend` trait (consume AI samples →
//! host stream) + a cpal backend + the RSPRecomp'd-ucode path behind it.
//! Symmetric with fn64-render.").
//!
//! ## What this crate is
//!
//! Mirrors `fn64-render`'s split exactly:
//! - `AudioBackend` is the ONE trait boundary the runtime hands AI (Audio
//!   Interface) DMA sample buffers to. No backend-specific type escapes it.
//! - This crate defines the trait plus one real backend (`CpalBackend`,
//!   feeding a live cpal output stream from a ring buffer). No game-derived
//!   code lives here, matching the workspace's "no game content ships in
//!   this repo" rule.
//!
//! ## Two very different halves, on purpose
//!
//! N64 audio is produced by an **RSP microcode program** (the "audio
//! ucode") that the CPU hands a task to; the ucode reads voice/envelope/
//! ADPCM data out of rdram and writes finished PCM samples into an AI DMA
//! buffer, which the VI/AI hardware then streams to the DAC. That is two
//! separable jobs:
//!
//! 1. **Ucode DECODE/EXECUTION** — [`rsp`] is fn64's clean-room,
//!    manual-complete scalar/vector RSP interpreter and is the accuracy
//!    authority for live audio microcode. [`hle`] is the family-neutral
//!    command-list substrate for a separately admitted optimized path.
//!    Family-specific HLE execution remains loud until its RDRAM/DMEM output
//!    matches that LLE authority; no convenient opcode table is guessed from
//!    a task header.
//! 2. **Sample DELIVERY** — once *some* source (a real ucode interpreter
//!    later, a test fixture today) has produced a buffer of finished PCM
//!    samples, getting those samples to the host's actual sound card. This
//!    is ordinary buffer/ring-buffer plumbing with no game-derived logic in
//!    it at all — **this half is real**, implemented against `cpal`
//!    (portable Rust audio I/O, same tier of dependency as this crate's
//!    sibling `fn64-render-rt64` takes on RT64 for its half of the render
//!    seam).
//!
//! `AudioBackend` models half 2. The older [`UcodeExecutor`] callback remains
//! only as a compatibility-shaped loud boundary; it is not the live LLE task
//! path and its immutable-RDRAM/returned-PCM shape is not suitable for HLE.
#![forbid(unsafe_code)]

pub mod characterize;
pub mod compact_abi;
pub mod compact_dsp_abi;
pub mod compact_memory_abi;
pub mod hle;
// The former ucode-phase commit candidate compared a reduced visible-state
// projection and accepted an independently supplied LLE result. Keep its
// crate-private characterization available to `whole_task`, but expose no
// activation surface until a paired lane seals complete RSP architectural
// state from one snapshot.
#[allow(dead_code)]
pub(crate) mod hle_commit;
pub use hle_commit::{AudioTaskStepTotals, PrepareUcodeCommitError};
pub mod hle_effects;
pub mod hle_executor;
pub mod hle_lle;
pub mod hle_memory;
pub mod hle_outcome;
pub mod hle_rspboot;
pub mod hle_snapshot;
pub mod hle_transaction;
pub mod rsp;
pub mod standard_abi;
mod units;
pub mod whole_task;

pub use units::{
    ChannelCount, GuestDmaByteCount, GuestPcm16, GuestSampleRateHz, GuestSampleSlotCount,
    HostFrameCount, HostSampleRateHz, HostSampleSlotCount,
};

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};

/// Errors an `AudioBackend` call can surface. Every variant is loud/named
/// (mirroring `fn64_render::RenderError`'s "no silent black frame" rule,
/// applied here as "no silent dropped/garbled audio") — there is no
/// `AudioError::Other(String)` catch-all.
#[derive(Debug)]
pub enum AudioError {
    /// `queue_samples`/`frames_remaining`/`set_frequency` was called before
    /// `create`, or after the backend's stream died.
    NotReady(&'static str),
    /// The backend's own internal failure (device open failed, stream
    /// build/play failed, etc). Adapters map their own detailed error into
    /// this with a short static tag, mirroring
    /// `RenderError::Backend { backend, reason }`.
    Backend {
        backend: &'static str,
        reason: String,
    },
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NotReady(reason) => write!(f, "audio backend not ready: {reason}"),
            AudioError::Backend { backend, reason } => {
                write!(f, "{backend} backend error: {reason}")
            }
        }
    }
}

impl std::error::Error for AudioError {}

/// Backend configuration for `AudioBackend::create`. Deliberately minimal —
/// mirrors `fn64_render::RenderConfig`'s "only what every backend needs to
/// agree on" stance. `channels` is 2 (stereo) for every real N64 title's AI
/// output per the public libultra manual's documented `osAiSetFrequency`/
/// AI DMA contract; kept explicit here rather than hardcoded so a future
/// mono test fixture backend isn't forced to lie about it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AudioConfig {
    pub sample_rate_hz: GuestSampleRateHz,
    pub channels: ChannelCount,
}

impl AudioConfig {
    pub fn new(sample_rate_hz: u32, channels: u16) -> Self {
        AudioConfig {
            sample_rate_hz: GuestSampleRateHz::new(sample_rate_hz),
            channels: ChannelCount::new(channels),
        }
    }
}

/// Cumulative host-stream delivery counters. `underrun_sample_slots` counts
/// the exact interleaved output slots filled with silence because the producer
/// ring was empty; it is the mechanical choppiness signal that a point-in-time
/// ring depth cannot provide.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioStreamHealth {
    pub callbacks: u64,
    pub requested_sample_slots: HostSampleSlotCount,
    /// Compatibility total: empty-ring plus producer-lock-miss silence.
    pub underrun_sample_slots: HostSampleSlotCount,
    pub empty_ring_underrun_sample_slots: HostSampleSlotCount,
    pub lock_miss_underrun_sample_slots: HostSampleSlotCount,
    pub late_callbacks: u64,
    pub max_callback_gap_us: u64,
}

/// A host audio backend: consumes finished PCM sample buffers (the output
/// of an AI DMA / audio-ucode run, already decoded to interleaved i16 by
/// whatever produced them) and delivers them to a real output stream.
///
/// Mirrors `fn64_render::RenderBackend`'s shape one-for-one:
/// `create`/lifecycle, a per-buffer submit method, and a self-report of
/// backend capability. Every method takes exactly the data it needs and
/// returns a plain `Result` — no callback into the runtime.
pub trait AudioBackend {
    /// Initialize the backend (device/stream) for `cfg.sample_rate_hz`
    /// Hz, `cfg.channels` channels. Must be called before `queue_samples`.
    fn create(&mut self, cfg: &AudioConfig) -> Result<(), AudioError>;

    /// Enqueue one buffer of already-decoded interleaved PCM samples (i16,
    /// matching the N64 AI DMA's own sample format) for playback. Mirrors
    /// `RenderBackend::process_task` taking a raw byte slice: this crate
    /// does not interpret *how* `pcm` was produced (live RSP task output,
    /// silence, or a test tone). Live execution is owned by fn64's task
    /// dispatcher and RSP interpreter, not this delivery trait. `GuestPcm16`
    /// proves the buffer contains complete frames and carries its channel
    /// count across the trait-object seam.
    fn queue_samples(&mut self, pcm: GuestPcm16<'_>) -> Result<(), AudioError>;

    /// How many queued sample FRAMES (one frame = one sample per channel,
    /// matching libultra's own "sample" vs "frame" usage in
    /// `osAiSetNextBuffer`'s documented semantics) have not yet been
    /// consumed by the host stream. This is host-delivery telemetry and a
    /// capacity signal; it is not an emulated AI register. Guest-visible
    /// DMA progress comes from
    /// [`current_dma_bytes_remaining`](Self::current_dma_bytes_remaining).
    fn frames_remaining(&self) -> Result<HostFrameCount, AudioError>;

    /// Change the output sample rate at runtime, matching real hardware's
    /// `osAiSetFrequency` being callable mid-game (title music vs SFX often
    /// run the AI at different rates). Infallible by design, mirroring
    /// `RenderBackend::resize`: a backend that cannot honor a rate change
    /// should surface that at the next `queue_samples` call via
    /// `AudioError`, not here.
    fn set_frequency(&mut self, sample_rate_hz: GuestSampleRateHz);

    /// The host stream's actual rate, when the backend knows it. This is
    /// telemetry for proving guest/device conversion, not an AI-register
    /// input. Default `None` keeps existing/fake backends contract-compatible.
    fn stream_rate_hz(&self) -> Option<HostSampleRateHz> {
        None
    }

    /// Bytes remaining in the N64 AI DMA currently at the head of the
    /// playback queue. This is deliberately distinct from
    /// [`frames_remaining`](Self::frames_remaining): host-side prebuffering
    /// is not visible in the hardware `AI_LEN` register.
    fn current_dma_bytes_remaining(&self) -> Option<GuestDmaByteCount> {
        None
    }

    /// Cumulative realtime delivery health when the backend owns a callback
    /// stream. Fake/headless backends return `None` by default.
    fn stream_health(&self) -> Option<AudioStreamHealth> {
        None
    }
}

/// Legacy callback shape retained as a loud compatibility boundary.
///
/// Fn64's live audio tasks already execute through the clean-room RSP LLE
/// path. This older interface cannot represent that behavior: real tasks
/// mutate RDRAM and persistent RSP state, and the CPU names PCM later at the
/// AI DMA boundary. New execution code must use the typed task/outcome path
/// rather than implementing this immutable-input/returned-PCM callback.
pub trait UcodeExecutor {
    /// Interpret and run one audio-ucode task's RSP program against
    /// `rdram`, starting at `ucode_addr` (rdram-relative, matching
    /// `fn64_render::OsTask::ucode`'s own convention). A real
    /// This method is unsupported because returning PCM cannot represent the
    /// complete task effect.
    fn execute_task(&mut self, rdram: &[u8], ucode_addr: u32) -> Result<Vec<i16>, AudioError>;
}

/// The loud, named implementation for the unsupported legacy callback.
///
/// It never fabricates silence or impersonates the live task path.
#[derive(Default)]
pub struct LoudStubUcodeExecutor;

impl UcodeExecutor for LoudStubUcodeExecutor {
    fn execute_task(&mut self, _rdram: &[u8], ucode_addr: u32) -> Result<Vec<i16>, AudioError> {
        let reason = format!(
            "legacy audio ucode callback cannot represent mutable RDRAM/RSP task effects; \
             use fn64's live-image LLE task path instead of fabricating output for ucode \
             at rdram offset {ucode_addr:#010x}"
        );
        fn64_runtime::record_unsupported_event(
            fn64_runtime::UnsupportedSubsystem::Audio,
            "audio.ucode-executor.unimplemented",
            reason.clone(),
            None,
            fn64_runtime::UnsupportedDisposition::ReturnedError,
        );
        Err(AudioError::Backend {
            backend: "ucode-executor",
            reason,
        })
    }
}

/// Shared ring buffer between the producer side (`CpalBackend::queue_samples`,
/// called from the emulation thread) and cpal's realtime callback. The
/// callback only attempts the lock: producer contention is rendered as a
/// counted underrun instead of ever blocking the device thread. The producer
/// needs exclusive access to preserve the established drop-oldest overflow
/// policy and its per-DMA byte accounting; a strict SPSC producer cannot
/// safely evict the consumer's oldest slots.
type SampleRing = Arc<Mutex<OutputRing>>;

#[derive(Debug)]
struct DmaSpan {
    output_samples_total: usize,
    output_samples_remaining: usize,
    guest_bytes_total: GuestDmaByteCount,
}

#[derive(Debug)]
struct OutputRing {
    samples: VecDeque<i16>,
    dmas: VecDeque<DmaSpan>,
    sample_cap: usize,
}

impl OutputRing {
    fn with_capacity(sample_cap: usize) -> Self {
        OutputRing {
            samples: VecDeque::with_capacity(sample_cap),
            // Every retained DMA owns at least one retained sample, so this
            // worst-case metadata bound cannot be exceeded while samples are
            // capped. Preallocating it keeps producer work inside the shared
            // critical section allocation-free as well.
            dmas: VecDeque::with_capacity(sample_cap),
            sample_cap,
        }
    }

    /// Append one DMA without ever growing the realtime sample storage.
    ///
    /// Logically this is the old `extend` followed by `cap_samples`: oldest
    /// queued samples (including an oversized DMA's prefix) are discarded,
    /// and DMA progress advances by the same count. Doing the eviction first
    /// keeps `samples.len() <= sample_cap` throughout, so `extend` cannot
    /// allocate after stream creation.
    fn push_dma(&mut self, output: &[i16], guest_bytes: GuestDmaByteCount) -> usize {
        if output.is_empty() {
            return 0;
        }
        let dropped = self
            .samples
            .len()
            .saturating_add(output.len())
            .saturating_sub(self.sample_cap);
        let old_dropped = dropped.min(self.samples.len());
        if old_dropped > 0 {
            self.samples.drain(..old_dropped);
            self.consume_spans(old_dropped);
        }
        let input_dropped = dropped - old_dropped;
        if input_dropped < output.len() {
            self.dmas.push_back(DmaSpan {
                output_samples_total: output.len(),
                output_samples_remaining: output.len() - input_dropped,
                guest_bytes_total: guest_bytes,
            });
        }
        self.samples.extend(&output[input_dropped..]);
        dropped
    }

    fn consume_spans(&mut self, mut samples: usize) {
        while samples > 0 {
            let Some(front) = self.dmas.front_mut() else {
                break;
            };
            let consumed = samples.min(front.output_samples_remaining);
            front.output_samples_remaining -= consumed;
            samples -= consumed;
            if front.output_samples_remaining == 0 {
                self.dmas.pop_front();
            }
        }
    }

    #[cfg(test)]
    fn drain_into(&mut self, output: &mut [i16]) -> usize {
        let delivered = output.len().min(self.samples.len());
        for slot in &mut output[..delivered] {
            *slot = self
                .samples
                .pop_front()
                .expect("delivered length was bounded");
        }
        output[delivered..].fill(0);
        self.consume_spans(delivered);
        output.len() - delivered
    }

    fn drain_into_f32(&mut self, output: &mut [f32]) -> usize {
        let delivered = output.len().min(self.samples.len());
        {
            let (front, back) = self.samples.as_slices();
            for (slot, sample) in output[..delivered].iter_mut().zip(front.iter().chain(back)) {
                *slot = f32::from(*sample) / 32768.0;
            }
        }
        self.samples.drain(..delivered);
        output[delivered..].fill(0.0);
        self.consume_spans(delivered);
        output.len() - delivered
    }

    fn current_dma_bytes_remaining(&self) -> GuestDmaByteCount {
        let Some(front) = self.dmas.front() else {
            return GuestDmaByteCount::ZERO;
        };
        let remaining = u64::from(front.guest_bytes_total.get())
            * front.output_samples_remaining as u64
            / front.output_samples_total as u64;
        GuestDmaByteCount::new(u32::try_from(remaining).unwrap_or(u32::MAX) & !3)
    }

    fn has_ai_double_buffer(&self) -> bool {
        self.dmas.len() >= 2
    }
}

fn drain_ring_into_f32(mut ring: MutexGuard<'_, OutputRing>, output: &mut [f32]) -> usize {
    ring.drain_into_f32(output)
}

/// Never wait on the emulation thread from cpal's realtime callback. A busy
/// producer is indistinguishable from an empty ring for this one pull: both
/// must become silence and both are included in `underrun_sample_slots`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RingUnderrun {
    empty_ring_sample_slots: usize,
    lock_miss_sample_slots: usize,
}

impl RingUnderrun {
    fn total(self) -> usize {
        self.empty_ring_sample_slots + self.lock_miss_sample_slots
    }
}

fn try_drain_ring_into_f32(ring: &SampleRing, output: &mut [f32]) -> RingUnderrun {
    match ring.try_lock() {
        Ok(guard) => RingUnderrun {
            empty_ring_sample_slots: drain_ring_into_f32(guard, output),
            lock_miss_sample_slots: 0,
        },
        Err(TryLockError::Poisoned(error)) => RingUnderrun {
            empty_ring_sample_slots: drain_ring_into_f32(error.into_inner(), output),
            lock_miss_sample_slots: 0,
        },
        Err(TryLockError::WouldBlock) => {
            output.fill(0.0);
            RingUnderrun {
                empty_ring_sample_slots: 0,
                lock_miss_sample_slots: output.len(),
            }
        }
    }
}

/// A real `AudioBackend` backed by a live `cpal` output stream. Consumes
/// interleaved i16 PCM (see `AudioBackend::queue_samples`) into a shared
/// ring buffer; cpal's own audio callback (running on its own realtime
/// thread once `create` succeeds) drains that ring buffer into the actual
/// host output stream, underrunning with silence rather than blocking if
/// the ring runs dry.
pub struct CpalBackend {
    ring: SampleRing,
    channels: ChannelCount,
    stream: Option<Stream>,
    stream_started: bool,
    /// The rate the host stream actually runs at, negotiated in [`create`]:
    /// the requested guest rate when the device accepts it, else the
    /// device's default output rate (macOS commonly rejects the N64's
    /// 32 kHz). Playing guest-rate samples on a faster stream without
    /// conversion starves the ring ~proportionally and the callback's
    /// zero-fill turns that into loud static -- hence [`BandlimitedResampler`].
    /// The host callback itself writes `f32`, CoreAudio's native path on macOS;
    /// the emulated AI/ring stays i16 until that final device boundary.
    stream_rate_hz: Option<HostSampleRateHz>,
    /// The guest-side production rate the queued samples arrive at. Seeded
    /// from the `create` config, updated live by `set_frequency`
    /// (`osAiSetFrequency`).
    guest_rate_hz: Option<GuestSampleRateHz>,
    resampler: BandlimitedResampler,
    /// Reused producer-side conversion storage. Its capacity is reserved from
    /// the exact rate ratio before resampling, avoiding a guaranteed growth
    /// allocation for common 28.8/32 kHz -> 48 kHz conversion.
    resample_output: Vec<i16>,
    /// One-shot flag so ring-overflow drops are reported loudly exactly once
    /// per stream, not once per queue call.
    warned_overflow: bool,
    callback_count: Arc<AtomicU64>,
    requested_sample_slots: Arc<AtomicU64>,
    underrun_sample_slots: Arc<AtomicU64>,
    empty_ring_underrun_sample_slots: Arc<AtomicU64>,
    lock_miss_underrun_sample_slots: Arc<AtomicU64>,
    late_callbacks: Arc<AtomicU64>,
    max_callback_gap_us: Arc<AtomicU64>,
    output_dump: Option<PcmStreamDump>,
    output_dump_checked: bool,
}

impl Default for CpalBackend {
    fn default() -> Self {
        CpalBackend::new()
    }
}

impl CpalBackend {
    pub fn new() -> Self {
        CpalBackend {
            ring: Arc::new(Mutex::new(OutputRing::with_capacity(0))),
            channels: ChannelCount::STEREO,
            stream: None,
            stream_started: false,
            stream_rate_hz: None,
            guest_rate_hz: None,
            resampler: BandlimitedResampler::new(),
            resample_output: Vec::new(),
            warned_overflow: false,
            callback_count: Arc::new(AtomicU64::new(0)),
            requested_sample_slots: Arc::new(AtomicU64::new(0)),
            underrun_sample_slots: Arc::new(AtomicU64::new(0)),
            empty_ring_underrun_sample_slots: Arc::new(AtomicU64::new(0)),
            lock_miss_underrun_sample_slots: Arc::new(AtomicU64::new(0)),
            late_callbacks: Arc::new(AtomicU64::new(0)),
            max_callback_gap_us: Arc::new(AtomicU64::new(0)),
            output_dump: None,
            output_dump_checked: false,
        }
    }

    /// The negotiated host stream rate, once `create` has succeeded.
    pub fn stream_rate_hz(&self) -> Option<HostSampleRateHz> {
        self.stream.as_ref()?;
        self.stream_rate_hz
    }
}

const OUTPUT_STREAM_DUMP_SECONDS: u64 = 12;

struct PcmStreamDump {
    file: std::fs::File,
    path: std::path::PathBuf,
    sample_rate_hz: HostSampleRateHz,
    channels: ChannelCount,
    samples_written: u64,
    buffers_written: u64,
}

impl PcmStreamDump {
    fn maybe_create_from_env(
        sample_rate_hz: HostSampleRateHz,
        channels: ChannelCount,
    ) -> Option<Self> {
        let path = std::env::var_os("FN64_DUMP_AUDIO_OUTPUT_STREAM_PCM")?;
        let path = std::path::PathBuf::from(path);
        match std::fs::File::create(&path) {
            Ok(file) => {
                eprintln!(
                    "fn64-audio: capturing up to {OUTPUT_STREAM_DUMP_SECONDS}s of post-resample {channels}-channel PCM at {sample_rate_hz} Hz to {path:?}"
                );
                Some(PcmStreamDump {
                    file,
                    path,
                    sample_rate_hz,
                    channels,
                    samples_written: 0,
                    buffers_written: 0,
                })
            }
            Err(error) => {
                eprintln!("fn64-audio: failed to create post-resample PCM dump: {error}");
                None
            }
        }
    }

    fn write_samples(&mut self, samples: &[i16]) {
        use std::io::Write as _;

        let max_samples = u64::from(self.sample_rate_hz.get())
            .saturating_mul(u64::from(self.channels.get()))
            .saturating_mul(OUTPUT_STREAM_DUMP_SECONDS);
        let remaining = max_samples.saturating_sub(self.samples_written);
        let take = usize::try_from(remaining.min(samples.len() as u64)).unwrap_or(samples.len());
        if take == 0 {
            return;
        }
        let bytes: Vec<u8> = samples[..take]
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        if let Err(error) = self.file.write_all(&bytes) {
            eprintln!("fn64-audio: failed to append post-resample PCM dump: {error}");
            return;
        }
        self.samples_written += take as u64;
        self.buffers_written += 1;
        let meta = format!(
            "format=s16le\nchannels={}\nsample_rate_hz={}\nsample_slots={}\nframes={}\nbuffers={}\nseconds={:.6}\n",
            self.channels,
            self.sample_rate_hz,
            self.samples_written,
            self.samples_written / u64::from(self.channels.get()),
            self.buffers_written,
            self.samples_written as f64
                / f64::from(self.channels.get())
                / f64::from(self.sample_rate_hz.get()),
        );
        if let Err(error) = std::fs::write(self.path.with_extension("meta"), meta) {
            eprintln!("fn64-audio: failed to update post-resample PCM metadata: {error}");
        }
        if self.samples_written == max_samples {
            eprintln!(
                "fn64-audio: completed {OUTPUT_STREAM_DUMP_SECONDS}s post-resample PCM capture at {:?}",
                self.path
            );
        }
    }
}

/// Stateful band-limited resampler over interleaved `i16` frames. It keeps
/// enough past and future input frames for a windowed-sinc interpolation
/// kernel, so chunked `queue_samples` input is sample-identical to one large
/// call while avoiding the high-frequency images produced by linear
/// interpolation.
struct BandlimitedResampler {
    frames: Vec<i16>,
    channels: Option<ChannelCount>,
    in_hz: Option<GuestSampleRateHz>,
    out_hz: Option<HostSampleRateHz>,
    /// Fractional read position in `frames` coordinates.
    phase: f64,
}

impl BandlimitedResampler {
    const RADIUS: isize = 16;

    fn new() -> Self {
        BandlimitedResampler {
            frames: Vec::new(),
            channels: None,
            in_hz: None,
            out_hz: None,
            phase: 0.0,
        }
    }

    /// Convert `input` (interleaved, `channels` per frame, produced at
    /// `in_hz`) to `out_hz`, appending to `out`. Equal rates pass through
    /// unchanged (byte-identical to the pre-resampler behavior).
    fn process(
        &mut self,
        input: GuestPcm16<'_>,
        in_hz: GuestSampleRateHz,
        out_hz: HostSampleRateHz,
        out: &mut Vec<i16>,
    ) {
        let channels = input.channels();
        let channel_slots = channels.as_usize();
        if in_hz.get() == out_hz.get() {
            self.reset();
            out.extend_from_slice(input.samples());
            return;
        }

        if self.channels != Some(channels)
            || self.in_hz != Some(in_hz)
            || self.out_hz != Some(out_hz)
        {
            self.reset();
            self.channels = Some(channels);
            self.in_hz = Some(in_hz);
            self.out_hz = Some(out_hz);
        }

        let in_frames = input.samples().len() / channel_slots;
        self.frames
            .extend_from_slice(&input.samples()[..in_frames * channel_slots]);
        let total_frames = self.frames.len() / channel_slots;
        if total_frames == 0 {
            return;
        }

        let step = f64::from(in_hz.get()) / f64::from(out_hz.get());
        while self.phase + (Self::RADIUS as f64) < total_frames as f64 - 1.0e-9 {
            for ch in 0..channel_slots {
                out.push(self.sample_at(self.phase, ch));
            }
            self.phase += step;
        }

        let keep_from = (self.phase.floor() as isize - Self::RADIUS).max(0) as usize;
        if keep_from > 0 {
            self.frames.drain(..keep_from * channel_slots);
            self.phase -= keep_from as f64;
        }
    }

    fn reset(&mut self) {
        self.frames.clear();
        self.channels = None;
        self.in_hz = None;
        self.out_hz = None;
        self.phase = 0.0;
    }

    fn output_samples_hint(
        input: GuestPcm16<'_>,
        in_hz: GuestSampleRateHz,
        out_hz: HostSampleRateHz,
    ) -> usize {
        if in_hz.get() == out_hz.get() {
            return input.samples().len();
        }
        let channels = input.channels().as_usize();
        let input_frames = input.samples().len() / channels;
        let output_frames = (input_frames as u128)
            .saturating_mul(u128::from(out_hz.get()))
            .div_ceil(u128::from(in_hz.get()))
            .saturating_add(1);
        usize::try_from(output_frames.saturating_mul(channels as u128)).unwrap_or(usize::MAX)
    }

    fn sample_at(&self, pos: f64, channel: usize) -> i16 {
        let channels = self
            .channels
            .expect("resampler samples require a configured channel count")
            .as_usize();
        let frame_count = self.frames.len() / channels;
        let center = pos.floor() as isize;
        let mut sum = 0.0;
        let mut weight_sum = 0.0;
        for index in center - Self::RADIUS + 1..=center + Self::RADIUS {
            let distance = pos - index as f64;
            let Some(weight) = Self::windowed_sinc(distance) else {
                continue;
            };
            let clamped = index.clamp(0, frame_count.saturating_sub(1) as isize) as usize;
            let sample = self.frames[clamped * channels + channel] as f64;
            sum += sample * weight;
            weight_sum += weight;
        }
        if weight_sum != 0.0 {
            sum /= weight_sum;
        }
        sum.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16
    }

    fn windowed_sinc(distance: f64) -> Option<f64> {
        let radius = Self::RADIUS as f64;
        let abs = distance.abs();
        if abs > radius {
            return None;
        }
        let sinc = if abs < f64::EPSILON {
            1.0
        } else {
            let x = std::f64::consts::PI * distance;
            x.sin() / x
        };
        let window = 0.42
            + 0.5 * (std::f64::consts::PI * abs / radius).cos()
            + 0.08 * (2.0 * std::f64::consts::PI * abs / radius).cos();
        Some(sinc * window)
    }
}

#[cfg(test)]
fn tone(frames: usize, hz: f64, sample_rate: f64, amplitude: f64) -> Vec<i16> {
    let mut out = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let s = (amplitude * (2.0 * std::f64::consts::PI * hz * n as f64 / sample_rate).sin())
            .round() as i16;
        out.push(s);
        out.push(s);
    }
    out
}

#[cfg(test)]
fn goertzel_power_mono(samples: &[i16], channels: usize, sample_rate: f64, target_hz: f64) -> f64 {
    let frames = samples.len() / channels;
    let omega = 2.0 * std::f64::consts::PI * target_hz / sample_rate;
    let coeff = 2.0 * omega.cos();
    let mut s_prev = 0.0;
    let mut s_prev2 = 0.0;
    for frame in samples.chunks_exact(channels) {
        let s = frame[0] as f64 + coeff * s_prev - s_prev2;
        s_prev2 = s_prev;
        s_prev = s;
    }
    let power = s_prev2 * s_prev2 + s_prev * s_prev - coeff * s_prev * s_prev2;
    power / frames.max(1) as f64
}

#[cfg(test)]
fn linear_resample_for_quality_test(
    input: &[i16],
    channels: usize,
    in_hz: u32,
    out_hz: u32,
) -> Vec<i16> {
    let in_frames = input.len() / channels;
    let mut out = Vec::new();
    let step = in_hz as f64 / out_hz as f64;
    let mut pos = 0.0;
    while pos < in_frames.saturating_sub(1) as f64 {
        let i = pos as usize;
        let frac = pos - i as f64;
        for ch in 0..channels {
            let a = input[i * channels + ch] as f64;
            let b = input[(i + 1) * channels + ch] as f64;
            out.push((a + (b - a) * frac).round() as i16);
        }
        pos += step;
    }
    out
}

impl AudioBackend for CpalBackend {
    fn create(&mut self, cfg: &AudioConfig) -> Result<(), AudioError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(AudioError::Backend {
            backend: "cpal",
            reason: "no default output device".to_string(),
        })?;

        let build = |rate_hz: HostSampleRateHz| {
            let stream_config = StreamConfig {
                channels: cfg.channels.get(),
                sample_rate: rate_hz.get(),
                buffer_size: cpal::BufferSize::Default,
            };
            // Allocate the complete 250 ms latency bound before cpal can run
            // its callback. `OutputRing::push_dma` evicts before appending, so
            // neither sample nor DMA-span storage grows on either thread.
            let ring_capacity = (rate_hz.get() as usize / 4).max(1) * cfg.channels.as_usize();
            let ring = Arc::new(Mutex::new(OutputRing::with_capacity(ring_capacity)));
            let callback_ring = Arc::clone(&ring);
            let callback_count = Arc::clone(&self.callback_count);
            let requested_sample_slots = Arc::clone(&self.requested_sample_slots);
            let underrun_sample_slots = Arc::clone(&self.underrun_sample_slots);
            let empty_ring_underrun_sample_slots =
                Arc::clone(&self.empty_ring_underrun_sample_slots);
            let lock_miss_underrun_sample_slots = Arc::clone(&self.lock_miss_underrun_sample_slots);
            let late_callbacks = Arc::clone(&self.late_callbacks);
            let max_callback_gap_us = Arc::clone(&self.max_callback_gap_us);
            let mut last_pull = None;
            device
                .build_output_stream(
                    stream_config,
                    move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                        let now = std::time::Instant::now();
                        if let Some(last) = last_pull.replace(now) {
                            let gap = now.saturating_duration_since(last);
                            let expected = std::time::Duration::from_secs_f64(
                                data.len() as f64
                                    / f64::from(stream_config.channels)
                                    / f64::from(stream_config.sample_rate),
                            );
                            if gap > expected.mul_f64(1.5) {
                                late_callbacks.fetch_add(1, Ordering::Relaxed);
                            }
                            max_callback_gap_us.fetch_max(
                                u64::try_from(gap.as_micros()).unwrap_or(u64::MAX),
                                Ordering::Relaxed,
                            );
                        }
                        callback_count.fetch_add(1, Ordering::Relaxed);
                        requested_sample_slots.fetch_add(data.len() as u64, Ordering::Relaxed);
                        let underrun = try_drain_ring_into_f32(&callback_ring, data);
                        underrun_sample_slots.fetch_add(underrun.total() as u64, Ordering::Relaxed);
                        empty_ring_underrun_sample_slots
                            .fetch_add(underrun.empty_ring_sample_slots as u64, Ordering::Relaxed);
                        lock_miss_underrun_sample_slots
                            .fetch_add(underrun.lock_miss_sample_slots as u64, Ordering::Relaxed);
                    },
                    move |err| {
                        // cpal stream error callback: no runtime state to
                        // report it into (matches `RenderError`'s stance that
                        // a backend failure is surfaced for a caller to poll,
                        // not force-propagated); logged so it isn't silent.
                        eprintln!("fn64-audio: cpal stream error: {err}");
                    },
                    None,
                )
                .map(|stream| (stream, ring))
        };

        // Prefer a stream at the guest's own rate (no conversion). Devices
        // are allowed to refuse it (macOS CoreAudio commonly rejects the
        // N64's 32 kHz); then run at the device's default rate and let
        // `queue_samples` resample. Never play guest-rate samples on a
        // different-rate stream: the rate mismatch chronically starves or
        // floods the ring and the callback's zero-fill renders that as
        // loud static.
        let requested_host_rate = HostSampleRateHz::new(cfg.sample_rate_hz.get());
        let (stream, ring, stream_rate_hz) = match build(requested_host_rate) {
            Ok((stream, ring)) => (stream, ring, requested_host_rate),
            Err(requested_err) => {
                let default_cfg =
                    device
                        .default_output_config()
                        .map_err(|e| AudioError::Backend {
                            backend: "cpal",
                            reason: format!(
                                "build_output_stream failed at {} Hz ({requested_err}) and \
                                 default_output_config failed: {e}",
                                cfg.sample_rate_hz
                            ),
                        })?;
                let fallback_hz = default_cfg.sample_rate();
                eprintln!(
                    "fn64-audio: device rejected {} Hz ({requested_err}); opening at \
                     device-default {fallback_hz} Hz with band-limited resampling",
                    cfg.sample_rate_hz
                );
                let fallback_rate = HostSampleRateHz::new(fallback_hz);
                let (stream, ring) = build(fallback_rate).map_err(|e| AudioError::Backend {
                    backend: "cpal",
                    reason: format!(
                        "build_output_stream failed at requested {} Hz and at \
                         device-default {fallback_hz} Hz: {e}",
                        cfg.sample_rate_hz
                    ),
                })?;
                (stream, ring, fallback_rate)
            }
        };

        self.ring = ring;
        self.channels = cfg.channels;
        self.stream_rate_hz = Some(stream_rate_hz);
        self.guest_rate_hz = Some(cfg.sample_rate_hz);
        self.resampler = BandlimitedResampler::new();
        self.stream_started = false;
        self.output_dump = None;
        self.output_dump_checked = false;
        self.stream = Some(stream);
        Ok(())
    }

    fn queue_samples(&mut self, pcm: GuestPcm16<'_>) -> Result<(), AudioError> {
        if self.stream.is_none() {
            return Err(AudioError::NotReady("create() not called"));
        }
        if pcm.channels() != self.channels {
            return Err(AudioError::Backend {
                backend: "cpal",
                reason: format!(
                    "queued PCM has {} channels but stream was created with {}",
                    pcm.channels(),
                    self.channels
                ),
            });
        }
        let guest_rate_hz = self
            .guest_rate_hz
            .expect("created stream must retain its guest sample rate");
        let stream_rate_hz = self
            .stream_rate_hz
            .expect("created stream must retain its host sample rate");
        let reserve = BandlimitedResampler::output_samples_hint(pcm, guest_rate_hz, stream_rate_hz);
        self.resample_output.clear();
        if self.resample_output.capacity() < reserve {
            self.resample_output.reserve(reserve);
        }
        self.resampler.process(
            pcm,
            guest_rate_hz,
            stream_rate_hz,
            &mut self.resample_output,
        );
        if !self.output_dump_checked {
            self.output_dump_checked = true;
            self.output_dump = PcmStreamDump::maybe_create_from_env(stream_rate_hz, self.channels);
        }
        if let Some(dump) = self.output_dump.as_mut() {
            dump.write_samples(&self.resample_output);
        }
        let (should_start, dropped, ring_cap) = {
            let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
            let dropped = ring.push_dma(&self.resample_output, pcm.dma_bytes());
            (
                !self.stream_started && ring.has_ai_double_buffer(),
                dropped,
                ring.sample_cap,
            )
        };
        if dropped > 0 && !self.warned_overflow {
            self.warned_overflow = true;
            eprintln!(
                "fn64-audio: output ring exceeded {ring_cap} samples; dropped {dropped} oldest \
                 (producer outrunning the drain -- reported once)"
            );
        }
        if should_start {
            self.stream
                .as_ref()
                .expect("stream checked above")
                .play()
                .map_err(|e| AudioError::Backend {
                    backend: "cpal",
                    reason: format!("stream.play failed after AI double-buffer prefill: {e}"),
                })?;
            self.stream_started = true;
        }
        Ok(())
    }

    fn frames_remaining(&self) -> Result<HostFrameCount, AudioError> {
        if self.stream.is_none() {
            return Err(AudioError::NotReady("create() not called"));
        }
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        Ok(HostFrameCount::new(
            u64::try_from(ring.samples.len() / self.channels.as_usize())
                .expect("host ring frame count must fit u64"),
        ))
    }

    fn set_frequency(&mut self, sample_rate_hz: GuestSampleRateHz) {
        // Real hardware allows `osAiSetFrequency` mid-game. The live cpal
        // stream keeps its negotiated rate; only the producer-side resample
        // ratio changes, so this is cheap and infallible (matching
        // `RenderBackend::resize`'s contract). A zero rate is a caller bug
        // upstream (`osAiSetFrequency` returns -1 before reaching us) and
        // cannot cross this typed boundary.
        self.guest_rate_hz = Some(sample_rate_hz);
    }

    fn stream_rate_hz(&self) -> Option<HostSampleRateHz> {
        CpalBackend::stream_rate_hz(self)
    }

    fn current_dma_bytes_remaining(&self) -> Option<GuestDmaByteCount> {
        self.stream.as_ref()?;
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        Some(ring.current_dma_bytes_remaining())
    }

    fn stream_health(&self) -> Option<AudioStreamHealth> {
        self.stream.as_ref()?;
        Some(AudioStreamHealth {
            callbacks: self.callback_count.load(Ordering::Relaxed),
            requested_sample_slots: HostSampleSlotCount::new(
                self.requested_sample_slots.load(Ordering::Relaxed),
            ),
            underrun_sample_slots: HostSampleSlotCount::new(
                self.underrun_sample_slots.load(Ordering::Relaxed),
            ),
            empty_ring_underrun_sample_slots: HostSampleSlotCount::new(
                self.empty_ring_underrun_sample_slots
                    .load(Ordering::Relaxed),
            ),
            lock_miss_underrun_sample_slots: HostSampleSlotCount::new(
                self.lock_miss_underrun_sample_slots.load(Ordering::Relaxed),
            ),
            late_callbacks: self.late_callbacks.load(Ordering::Relaxed),
            max_callback_gap_us: self.max_callback_gap_us.load(Ordering::Relaxed),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cpal-less fake backend -- proves the `AudioBackend` trait object
    /// contract (create-before-use, sample accounting) without needing a
    /// real output device, which CI/sandboxed test runs may not have.
    /// Mirrors `fn64-render`'s in-crate `FakeBackend`.
    #[derive(Default)]
    struct FakeBackend {
        ready: bool,
        channels: Option<ChannelCount>,
        queued_frames: HostFrameCount,
    }

    impl AudioBackend for FakeBackend {
        fn create(&mut self, cfg: &AudioConfig) -> Result<(), AudioError> {
            self.ready = true;
            self.channels = Some(cfg.channels);
            Ok(())
        }

        fn queue_samples(&mut self, pcm: GuestPcm16<'_>) -> Result<(), AudioError> {
            if !self.ready {
                return Err(AudioError::NotReady("create() not called"));
            }
            let channels = self.channels.expect("ready fake must retain channels");
            assert_eq!(pcm.channels(), channels);
            let added = u64::try_from(pcm.samples().len() / channels.as_usize()).unwrap();
            self.queued_frames = HostFrameCount::new(self.queued_frames.get() + added);
            Ok(())
        }

        fn frames_remaining(&self) -> Result<HostFrameCount, AudioError> {
            if !self.ready {
                return Err(AudioError::NotReady("create() not called"));
            }
            Ok(self.queued_frames)
        }

        fn set_frequency(&mut self, _sample_rate_hz: GuestSampleRateHz) {}
    }

    fn stereo(samples: &[i16]) -> GuestPcm16<'_> {
        GuestPcm16::new(samples, ChannelCount::STEREO)
    }

    fn guest_rate(rate: u32) -> GuestSampleRateHz {
        GuestSampleRateHz::new(rate)
    }

    fn host_rate(rate: u32) -> HostSampleRateHz {
        HostSampleRateHz::new(rate)
    }

    fn dma_bytes(bytes: u32) -> GuestDmaByteCount {
        GuestDmaByteCount::new(bytes)
    }

    #[test]
    fn is_dyn_safe_and_usable_through_a_trait_object() {
        let mut backend: Box<dyn AudioBackend> = Box::<FakeBackend>::default();
        backend.create(&AudioConfig::new(32000, 2)).unwrap();
        let samples = [0i16; 8]; // 4 stereo frames
        backend.queue_samples(stereo(&samples)).unwrap();
        assert_eq!(backend.frames_remaining().unwrap(), HostFrameCount::new(4));
    }

    #[test]
    fn queue_samples_before_create_is_not_ready() {
        let mut backend = FakeBackend::default();
        let err = backend.queue_samples(stereo(&[0i16; 4])).unwrap_err();
        assert!(matches!(err, AudioError::NotReady(_)));
    }

    #[test]
    fn frames_remaining_before_create_is_not_ready() {
        let backend = FakeBackend::default();
        let err = backend.frames_remaining().unwrap_err();
        assert!(matches!(err, AudioError::NotReady(_)));
    }

    #[test]
    fn frames_remaining_accounts_for_channel_count() {
        let mut backend = FakeBackend::default();
        backend.create(&AudioConfig::new(48000, 2)).unwrap();
        backend.queue_samples(stereo(&[1, 2, 3, 4, 5, 6])).unwrap(); // 3 stereo frames
        assert_eq!(backend.frames_remaining().unwrap(), HostFrameCount::new(3));
    }

    #[test]
    fn audio_error_display_is_informative() {
        let e = AudioError::Backend {
            backend: "cpal",
            reason: "device busy".to_string(),
        };
        let s = format!("{e}");
        assert!(s.contains("cpal"));
        assert!(s.contains("device busy"));
    }

    #[test]
    fn unsupported_loud_stub_ucode_executor_traps_by_name_not_silently() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let mut executor = LoudStubUcodeExecutor;
        let rdram = vec![0u8; 16];
        let err = executor.execute_task(&rdram, 0x8001_2340).unwrap_err();
        match err {
            AudioError::Backend { backend, reason } => {
                assert_eq!(backend, "ucode-executor");
                assert!(reason.contains("cannot represent mutable RDRAM/RSP task effects"));
                assert!(reason.contains("80012340"));
            }
            other => panic!("expected AudioError::Backend, got {other:?}"),
        }
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].operation,
            concat!("audio.ucode-executor.", "unimplemented")
        );
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::ReturnedError
        );
    }

    #[test]
    fn cpal_backend_reports_not_ready_before_create() {
        let mut backend = CpalBackend::new();
        let err = backend.queue_samples(stereo(&[0i16; 2])).unwrap_err();
        assert!(matches!(err, AudioError::NotReady(_)));
        let err = backend.frames_remaining().unwrap_err();
        assert!(matches!(err, AudioError::NotReady(_)));
    }

    #[test]
    fn cpal_backend_is_dyn_compatible() {
        // Compile-time proof only (mirrors fn64-render's dyn-safety test
        // shape) -- does NOT call `create`, since a real cpal device may
        // not exist in a headless CI/sandbox environment.
        let backend: Box<dyn AudioBackend> = Box::new(CpalBackend::new());
        drop(backend);
    }

    // --- BandlimitedResampler (the rate-conversion core; device-less, pure)

    #[test]
    fn resampler_equal_rates_pass_through_unchanged() {
        let mut rs = BandlimitedResampler::new();
        let input: Vec<i16> = (0..64).collect();
        let mut out = Vec::new();
        rs.process(
            stereo(&input),
            guest_rate(32000),
            host_rate(32000),
            &mut out,
        );
        assert_eq!(out, input);
    }

    #[test]
    fn resampler_32k_to_48k_produces_three_frames_per_two() {
        // 32000 -> 48000 is exactly 2:3. The windowed-sinc kernel withholds a
        // fixed tail for future taps, so a long input should still converge on
        // the correct ratio.
        let mut rs = BandlimitedResampler::new();
        let input: Vec<i16> = (0..4000).collect(); // 2000 stereo frames
        let mut out = Vec::new();
        rs.process(
            stereo(&input),
            guest_rate(32000),
            host_rate(48000),
            &mut out,
        );
        let frames_out = out.len() / 2;
        let expected = 2000 * 3 / 2;
        assert!(
            frames_out.abs_diff(expected) <= 32,
            "expected ~{expected}, got {frames_out}"
        );
    }

    #[test]
    fn resampler_output_hint_covers_upsampling_without_growth() {
        let mut resampler = BandlimitedResampler::new();
        let mut output = Vec::new();
        for sample_count in [2, 30, 106, 2096, 14, 512] {
            let input: Vec<i16> = (0..sample_count).collect();
            let hint = BandlimitedResampler::output_samples_hint(
                stereo(&input),
                guest_rate(28_800),
                host_rate(48_000),
            );
            output.clear();
            if output.capacity() < hint {
                output.reserve(hint);
            }
            let allocated = output.capacity();

            resampler.process(
                stereo(&input),
                guest_rate(28_800),
                host_rate(48_000),
                &mut output,
            );

            assert!(output.len() <= hint, "{sample_count} input samples");
            assert_eq!(output.capacity(), allocated, "{sample_count} input samples");
        }
    }

    #[test]
    fn resampler_preserves_stereo_channel_identity() {
        // L = constant 1000, R = constant -2000: linear interpolation of
        // constants is the constant, so ANY cross-channel mixup (the L/R
        // interleave bug class) shows up as a wrong value immediately.
        let mut rs = BandlimitedResampler::new();
        let mut input = Vec::new();
        for _ in 0..100 {
            input.push(1000i16);
            input.push(-2000i16);
        }
        let mut out = Vec::new();
        rs.process(
            stereo(&input),
            guest_rate(32000),
            host_rate(48000),
            &mut out,
        );
        assert!(!out.is_empty());
        for frame in out.chunks_exact(2) {
            assert_eq!(frame, [1000, -2000]);
        }
    }

    #[test]
    fn resampler_chunked_input_equals_single_call() {
        // Continuity across queue_samples boundaries: splitting the same
        // input into arbitrary chunks must produce the identical output
        // stream (a per-chunk phase/carry reset = an audible seam pop).
        let input: Vec<i16> = (0..600).map(|i| ((i * 37) % 5000) as i16).collect();

        let mut whole = Vec::new();
        BandlimitedResampler::new().process(
            stereo(&input),
            guest_rate(32000),
            host_rate(48000),
            &mut whole,
        );

        let mut chunked = Vec::new();
        let mut rs = BandlimitedResampler::new();
        for chunk in [&input[..106], &input[106..340], &input[340..]] {
            rs.process(
                stereo(chunk),
                guest_rate(32000),
                host_rate(48000),
                &mut chunked,
            );
        }
        assert_eq!(whole, chunked);
    }

    #[test]
    fn preallocated_ring_drops_oldest_keeps_newest_without_growing() {
        let mut ring = OutputRing::with_capacity(40);
        let allocated = ring.samples.capacity();
        let allocated_dmas = ring.dmas.capacity();
        let input: Vec<i16> = (0..100).collect();
        assert_eq!(ring.push_dma(&input, dma_bytes(200)), 60);
        assert_eq!(ring.samples.len(), 40);
        assert_eq!(
            *ring.samples.front().unwrap(),
            60,
            "oldest dropped, newest kept"
        );
        assert_eq!(*ring.samples.back().unwrap(), 99);
        assert_eq!(ring.samples.capacity(), allocated);
        assert_eq!(ring.dmas.capacity(), allocated_dmas);
        assert_eq!(ring.current_dma_bytes_remaining(), dma_bytes(80));
    }

    #[test]
    fn realtime_callback_uses_silence_instead_of_waiting_for_busy_producer() {
        let mut output_ring = OutputRing::with_capacity(8);
        output_ring.push_dma(&[10, 20, 30], dma_bytes(6));
        let ring = Arc::new(Mutex::new(output_ring));
        let producer_guard = ring.lock().unwrap();
        let mut output = [99.0; 5];

        assert_eq!(
            try_drain_ring_into_f32(&ring, &mut output),
            RingUnderrun {
                empty_ring_sample_slots: 0,
                lock_miss_sample_slots: 5,
            }
        );
        assert_eq!(output, [0.0; 5]);
        assert_eq!(
            producer_guard.samples.len(),
            3,
            "contention consumes nothing"
        );
    }

    #[test]
    fn realtime_callback_distinguishes_empty_ring_from_producer_lock_miss() {
        let mut output_ring = OutputRing::with_capacity(8);
        output_ring.push_dma(&[i16::MAX], dma_bytes(2));
        let ring = Arc::new(Mutex::new(output_ring));
        let mut output = [99.0; 3];

        let underrun = try_drain_ring_into_f32(&ring, &mut output);
        assert_eq!(
            underrun,
            RingUnderrun {
                empty_ring_sample_slots: 2,
                lock_miss_sample_slots: 0,
            }
        );
        assert_eq!(underrun.total(), 2);
        assert_eq!(
            underrun.total(),
            underrun.empty_ring_sample_slots + underrun.lock_miss_sample_slots,
            "the compatibility total must close over the two causal buckets"
        );
        assert_eq!(output, [i16::MAX as f32 / 32768.0, 0.0, 0.0]);
    }

    #[test]
    fn ai_length_tracks_only_the_head_dma_not_host_prebuffer() {
        let mut ring = OutputRing::with_capacity(32);
        ring.push_dma(&[1; 8], dma_bytes(16));
        ring.push_dma(&[2; 8], dma_bytes(16));
        assert!(ring.has_ai_double_buffer());
        assert_eq!(ring.current_dma_bytes_remaining(), dma_bytes(16));

        let mut output = [0; 6];
        assert_eq!(ring.drain_into(&mut output), 0);
        assert_eq!(ring.current_dma_bytes_remaining(), dma_bytes(4));

        let mut output = [0; 2];
        assert_eq!(ring.drain_into(&mut output), 0);
        assert_eq!(
            ring.current_dma_bytes_remaining(),
            dma_bytes(16),
            "the second queued DMA becomes current only after the first drains"
        );
    }

    #[test]
    fn overflow_advances_head_dma_before_retaining_newest_dma() {
        let mut ring = OutputRing::with_capacity(10);
        ring.push_dma(&[1; 8], dma_bytes(16));
        assert_eq!(ring.push_dma(&[2; 8], dma_bytes(16)), 6);
        assert_eq!(ring.current_dma_bytes_remaining(), dma_bytes(4));
        assert_eq!(
            ring.samples.iter().copied().collect::<Vec<_>>(),
            [1, 1, 2, 2, 2, 2, 2, 2, 2, 2]
        );

        let mut head_tail = [0; 2];
        assert_eq!(ring.drain_into(&mut head_tail), 0);
        assert_eq!(head_tail, [1, 1]);
        assert_eq!(ring.current_dma_bytes_remaining(), dma_bytes(16));
    }

    #[test]
    fn f32_drain_converts_i16_samples_at_the_host_boundary() {
        let mut ring = OutputRing::with_capacity(8);
        ring.push_dma(&[i16::MIN, -16384, 0, 16384], dma_bytes(8));
        let mut output = [99.0; 6];

        assert_eq!(ring.drain_into_f32(&mut output), 2);
        assert_eq!(output, [-1.0, -0.5, 0.0, 0.5, 0.0, 0.0]);
        assert_eq!(ring.current_dma_bytes_remaining(), GuestDmaByteCount::ZERO);
    }

    #[test]
    fn resampler_downsamples_toward_slower_stream() {
        // 32000 -> 22050 (a slower device): fewer frames out than in.
        let mut rs = BandlimitedResampler::new();
        let input: Vec<i16> = (0..400).collect(); // 200 stereo frames
        let mut out = Vec::new();
        rs.process(
            stereo(&input),
            guest_rate(32000),
            host_rate(22050),
            &mut out,
        );
        let frames_out = out.len() / 2;
        let expected = (200.0 * 22050.0 / 32000.0) as usize; // ~137
        assert!(
            frames_out.abs_diff(expected) <= 16,
            "expected ~{expected} output frames, got {frames_out}"
        );
    }

    #[test]
    fn resampler_suppresses_linear_image_band() {
        // A 12 kHz tone is valid at the guest's ~32 kHz rate. Linear
        // interpolation leaves a strong upsampling image near 20 kHz on a
        // 48 kHz device, heard as a faint buzz on bright material.
        let input = tone(4096, 12_000.0, 32_000.0, 12_000.0);
        let linear = linear_resample_for_quality_test(&input, 2, 32_000, 48_000);
        let mut bandlimited = Vec::new();
        BandlimitedResampler::new().process(
            stereo(&input),
            guest_rate(32_000),
            host_rate(48_000),
            &mut bandlimited,
        );

        let linear_tone = goertzel_power_mono(&linear, 2, 48_000.0, 12_000.0);
        let linear_image = goertzel_power_mono(&linear, 2, 48_000.0, 20_000.0);
        let sinc_tone = goertzel_power_mono(&bandlimited, 2, 48_000.0, 12_000.0);
        let sinc_image = goertzel_power_mono(&bandlimited, 2, 48_000.0, 20_000.0);

        assert!(
            sinc_tone > linear_tone * 0.85,
            "tone should not be materially dulled"
        );
        assert!(
            sinc_image < linear_image * 0.2,
            "bandlimited image {sinc_image} should be much lower than linear image {linear_image}"
        );
    }
}
