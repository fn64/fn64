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
pub mod hle;
pub mod hle_commit;
pub mod hle_lle;
pub mod hle_memory;
pub mod hle_outcome;
pub mod hle_rspboot;
pub mod hle_snapshot;
pub mod hle_transaction;
pub mod rsp;
pub mod standard_abi;

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
    pub sample_rate_hz: u32,
    pub channels: u16,
}

impl AudioConfig {
    pub fn new(sample_rate_hz: u32, channels: u16) -> Self {
        AudioConfig {
            sample_rate_hz,
            channels,
        }
    }
}

/// Cumulative host-stream delivery counters. `underrun_samples` counts the
/// exact output slots filled with silence because the producer ring was empty;
/// it is the mechanical choppiness signal that a point-in-time ring depth
/// cannot provide.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioStreamHealth {
    pub callbacks: u64,
    pub requested_samples: u64,
    pub underrun_samples: u64,
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
    /// does not interpret *how* `samples` was produced (live RSP task output,
    /// silence, or a test tone). Live execution is owned by fn64's task
    /// dispatcher and RSP interpreter, not this delivery trait.
    fn queue_samples(&mut self, samples: &[i16]) -> Result<(), AudioError>;

    /// How many queued sample FRAMES (one frame = one sample per channel,
    /// matching libultra's own "sample" vs "frame" usage in
    /// `osAiSetNextBuffer`'s documented semantics) have not yet been
    /// consumed by the host stream. This is host-delivery telemetry and a
    /// capacity signal; it is not an emulated AI register. Guest-visible
    /// DMA progress comes from
    /// [`current_dma_bytes_remaining`](Self::current_dma_bytes_remaining).
    fn frames_remaining(&self) -> Result<u32, AudioError>;

    /// Change the output sample rate at runtime, matching real hardware's
    /// `osAiSetFrequency` being callable mid-game (title music vs SFX often
    /// run the AI at different rates). Infallible by design, mirroring
    /// `RenderBackend::resize`: a backend that cannot honor a rate change
    /// should surface that at the next `queue_samples` call via
    /// `AudioError`, not here.
    fn set_frequency(&mut self, sample_rate_hz: u32);

    /// The host stream's actual rate, when the backend knows it. This is
    /// telemetry for proving guest/device conversion, not an AI-register
    /// input. Default `None` keeps existing/fake backends contract-compatible.
    fn stream_rate_hz(&self) -> Option<u32> {
        None
    }

    /// Bytes remaining in the N64 AI DMA currently at the head of the
    /// playback queue. This is deliberately distinct from
    /// [`frames_remaining`](Self::frames_remaining): host-side prebuffering
    /// is not visible in the hardware `AI_LEN` register.
    fn current_dma_bytes_remaining(&self) -> Option<u32> {
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
/// called from the emulation thread) and the cpal audio callback (called on
/// cpal's own realtime thread). A `Mutex<VecDeque<i16>>` rather than a
/// lock-free SPSC ring: this backend's job is proving the sample-delivery
/// seam is wired end-to-end, not winning a realtime-audio benchmark: see
/// this crate's module doc, "this half is real ... ordinary buffer
/// plumbing", not a claim of production-grade low-latency audio.
type SampleRing = Arc<Mutex<OutputRing>>;

#[derive(Debug)]
struct DmaSpan {
    output_samples_total: usize,
    output_samples_remaining: usize,
    guest_bytes_total: u32,
}

#[derive(Debug, Default)]
struct OutputRing {
    samples: VecDeque<i16>,
    dmas: VecDeque<DmaSpan>,
}

impl OutputRing {
    fn push_dma(&mut self, output: Vec<i16>, guest_bytes: u32) {
        if output.is_empty() {
            return;
        }
        self.dmas.push_back(DmaSpan {
            output_samples_total: output.len(),
            output_samples_remaining: output.len(),
            guest_bytes_total: guest_bytes,
        });
        self.samples.extend(output);
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
        for slot in &mut output[..delivered] {
            let sample = self
                .samples
                .pop_front()
                .expect("delivered length was bounded");
            *slot = f32::from(sample) / 32768.0;
        }
        output[delivered..].fill(0.0);
        self.consume_spans(delivered);
        output.len() - delivered
    }

    fn cap_samples(&mut self, cap: usize) -> usize {
        let dropped = self.samples.len().saturating_sub(cap);
        if dropped > 0 {
            self.samples.drain(..dropped);
            self.consume_spans(dropped);
        }
        dropped
    }

    fn current_dma_bytes_remaining(&self) -> u32 {
        let Some(front) = self.dmas.front() else {
            return 0;
        };
        let remaining = u64::from(front.guest_bytes_total) * front.output_samples_remaining as u64
            / front.output_samples_total as u64;
        u32::try_from(remaining).unwrap_or(u32::MAX) & !3
    }

    fn has_ai_double_buffer(&self) -> bool {
        self.dmas.len() >= 2
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
    channels: u16,
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
    stream_rate_hz: u32,
    /// The guest-side production rate the queued samples arrive at. Seeded
    /// from the `create` config, updated live by `set_frequency`
    /// (`osAiSetFrequency`).
    guest_rate_hz: u32,
    resampler: BandlimitedResampler,
    /// One-shot flag so ring-overflow drops are reported loudly exactly once
    /// per stream, not once per queue call.
    warned_overflow: bool,
    callback_count: Arc<AtomicU64>,
    requested_samples: Arc<AtomicU64>,
    underrun_samples: Arc<AtomicU64>,
    late_callbacks: Arc<AtomicU64>,
    max_callback_gap_us: Arc<AtomicU64>,
    output_dump: Option<PcmStreamDump>,
}

impl Default for CpalBackend {
    fn default() -> Self {
        CpalBackend::new()
    }
}

impl CpalBackend {
    pub fn new() -> Self {
        CpalBackend {
            ring: Arc::new(Mutex::new(OutputRing::default())),
            channels: 2,
            stream: None,
            stream_started: false,
            stream_rate_hz: 0,
            guest_rate_hz: 0,
            resampler: BandlimitedResampler::new(),
            warned_overflow: false,
            callback_count: Arc::new(AtomicU64::new(0)),
            requested_samples: Arc::new(AtomicU64::new(0)),
            underrun_samples: Arc::new(AtomicU64::new(0)),
            late_callbacks: Arc::new(AtomicU64::new(0)),
            max_callback_gap_us: Arc::new(AtomicU64::new(0)),
            output_dump: None,
        }
    }

    /// The negotiated host stream rate, once `create` has succeeded.
    pub fn stream_rate_hz(&self) -> Option<u32> {
        self.stream.as_ref().map(|_| self.stream_rate_hz)
    }
}

/// Drop the OLDEST samples so `ring` holds at most `cap` samples; returns
/// how many were dropped. Bounding the ring keeps output latency finite when
/// the producer outruns the drain (a paused/App-Napped callback, or a
/// headless probe pumping the game far faster than real time -- previously
/// an unbounded memory leak). Dropping the oldest skips playback ahead
/// instead of letting it lag ever further behind the game.
#[cfg(test)]
fn cap_ring(ring: &mut VecDeque<i16>, cap: usize) -> usize {
    let excess = ring.len().saturating_sub(cap);
    if excess > 0 {
        ring.drain(..excess);
    }
    excess
}

#[cfg(test)]
fn drain_ring(ring: &mut VecDeque<i16>, output: &mut [i16]) -> usize {
    let underrun = output.len().saturating_sub(ring.len());
    for slot in output {
        *slot = ring.pop_front().unwrap_or(0);
    }
    underrun
}

const OUTPUT_STREAM_DUMP_SECONDS: u64 = 12;

struct PcmStreamDump {
    file: std::fs::File,
    path: std::path::PathBuf,
    sample_rate_hz: u32,
    samples_written: u64,
    buffers_written: u64,
}

impl PcmStreamDump {
    fn maybe_create_from_env(sample_rate_hz: u32) -> Option<Self> {
        let path = std::env::var_os("FN64_DUMP_AUDIO_OUTPUT_STREAM_PCM")?;
        let path = std::path::PathBuf::from(path);
        match std::fs::File::create(&path) {
            Ok(file) => {
                eprintln!(
                    "fn64-audio: capturing up to {OUTPUT_STREAM_DUMP_SECONDS}s of post-resample stereo PCM at {sample_rate_hz} Hz to {path:?}"
                );
                Some(PcmStreamDump {
                    file,
                    path,
                    sample_rate_hz,
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

        let max_samples = u64::from(self.sample_rate_hz)
            .saturating_mul(2)
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
            "format=s16le\nchannels=2\nsample_rate_hz={}\nsamples={}\nframes={}\nbuffers={}\nseconds={:.6}\n",
            self.sample_rate_hz,
            self.samples_written,
            self.samples_written / 2,
            self.buffers_written,
            self.samples_written as f64 / 2.0 / f64::from(self.sample_rate_hz),
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
    channels: usize,
    in_hz: u32,
    out_hz: u32,
    /// Fractional read position in `frames` coordinates.
    phase: f64,
}

impl BandlimitedResampler {
    const RADIUS: isize = 16;

    fn new() -> Self {
        BandlimitedResampler {
            frames: Vec::new(),
            channels: 0,
            in_hz: 0,
            out_hz: 0,
            phase: 0.0,
        }
    }

    /// Convert `input` (interleaved, `channels` per frame, produced at
    /// `in_hz`) to `out_hz`, appending to `out`. Equal rates pass through
    /// unchanged (byte-identical to the pre-resampler behavior).
    fn process(
        &mut self,
        input: &[i16],
        channels: usize,
        in_hz: u32,
        out_hz: u32,
        out: &mut Vec<i16>,
    ) {
        debug_assert!(channels > 0 && in_hz > 0 && out_hz > 0);
        if in_hz == out_hz {
            self.reset();
            out.extend_from_slice(input);
            return;
        }

        if self.channels != channels || self.in_hz != in_hz || self.out_hz != out_hz {
            self.reset();
            self.channels = channels;
            self.in_hz = in_hz;
            self.out_hz = out_hz;
        }

        let in_frames = input.len() / channels;
        self.frames
            .extend_from_slice(&input[..in_frames * channels]);
        let total_frames = self.frames.len() / channels;
        if total_frames == 0 {
            return;
        }

        let step = in_hz as f64 / out_hz as f64;
        while self.phase + (Self::RADIUS as f64) < total_frames as f64 - 1.0e-9 {
            for ch in 0..channels {
                out.push(self.sample_at(self.phase, ch));
            }
            self.phase += step;
        }

        let keep_from = (self.phase.floor() as isize - Self::RADIUS).max(0) as usize;
        if keep_from > 0 {
            self.frames.drain(..keep_from * channels);
            self.phase -= keep_from as f64;
        }
    }

    fn reset(&mut self) {
        self.frames.clear();
        self.channels = 0;
        self.in_hz = 0;
        self.out_hz = 0;
        self.phase = 0.0;
    }

    fn sample_at(&self, pos: f64, channel: usize) -> i16 {
        let frame_count = self.frames.len() / self.channels;
        let center = pos.floor() as isize;
        let mut sum = 0.0;
        let mut weight_sum = 0.0;
        for index in center - Self::RADIUS + 1..=center + Self::RADIUS {
            let distance = pos - index as f64;
            let Some(weight) = Self::windowed_sinc(distance) else {
                continue;
            };
            let clamped = index.clamp(0, frame_count.saturating_sub(1) as isize) as usize;
            let sample = self.frames[clamped * self.channels + channel] as f64;
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

        let build = |rate_hz: u32| {
            let stream_config = StreamConfig {
                channels: cfg.channels,
                sample_rate: rate_hz,
                buffer_size: cpal::BufferSize::Default,
            };
            let ring = Arc::clone(&self.ring);
            let callback_count = Arc::clone(&self.callback_count);
            let requested_samples = Arc::clone(&self.requested_samples);
            let underrun_samples = Arc::clone(&self.underrun_samples);
            let late_callbacks = Arc::clone(&self.late_callbacks);
            let max_callback_gap_us = Arc::clone(&self.max_callback_gap_us);
            let mut first_pull = true;
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
                                    / f64::from(stream_config.channels.max(1))
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
                        let mut ring = ring.lock().unwrap_or_else(|e| e.into_inner());
                        if first_pull {
                            first_pull = false;
                            // One line per stream: proves the realtime
                            // callback actually runs (a created-and-played
                            // stream whose callback never fires is silent
                            // with no error anywhere else).
                            eprintln!(
                                "fn64-audio: output callback live (first pull: {} samples,                                  ring holds {})",
                                data.len(),
                                ring.samples.len()
                            );
                        }
                        callback_count.fetch_add(1, Ordering::Relaxed);
                        requested_samples.fetch_add(data.len() as u64, Ordering::Relaxed);
                        let underrun = ring.drain_into_f32(data);
                        underrun_samples.fetch_add(underrun as u64, Ordering::Relaxed);
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
        };

        // Prefer a stream at the guest's own rate (no conversion). Devices
        // are allowed to refuse it (macOS CoreAudio commonly rejects the
        // N64's 32 kHz); then run at the device's default rate and let
        // `queue_samples` resample. Never play guest-rate samples on a
        // different-rate stream: the rate mismatch chronically starves or
        // floods the ring and the callback's zero-fill renders that as
        // loud static.
        let (stream, stream_rate_hz) = match build(cfg.sample_rate_hz) {
            Ok(stream) => (stream, cfg.sample_rate_hz),
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
                let stream = build(fallback_hz).map_err(|e| AudioError::Backend {
                    backend: "cpal",
                    reason: format!(
                        "build_output_stream failed at requested {} Hz and at \
                         device-default {fallback_hz} Hz: {e}",
                        cfg.sample_rate_hz
                    ),
                })?;
                (stream, fallback_hz)
            }
        };

        self.channels = cfg.channels;
        self.stream_rate_hz = stream_rate_hz;
        self.guest_rate_hz = cfg.sample_rate_hz;
        self.resampler = BandlimitedResampler::new();
        self.stream_started = false;
        self.output_dump = None;
        self.stream = Some(stream);
        Ok(())
    }

    fn queue_samples(&mut self, samples: &[i16]) -> Result<(), AudioError> {
        if self.stream.is_none() {
            return Err(AudioError::NotReady("create() not called"));
        }
        let mut converted = Vec::with_capacity(samples.len());
        self.resampler.process(
            samples,
            self.channels.max(1) as usize,
            self.guest_rate_hz,
            self.stream_rate_hz,
            &mut converted,
        );
        if self.output_dump.is_none() {
            self.output_dump = PcmStreamDump::maybe_create_from_env(self.stream_rate_hz);
        }
        if let Some(dump) = self.output_dump.as_mut() {
            dump.write_samples(&converted);
        }
        let should_start = {
            let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
            ring.push_dma(
                converted,
                u32::try_from(samples.len().saturating_mul(2)).unwrap_or(u32::MAX),
            );
            // Bound output latency: ~250 ms of stream audio. Correct
            // retrace/DMA scheduling keeps the ring well below this; the cap
            // is the backstop for an unpaced producer.
            let cap = (self.stream_rate_hz as usize / 4).max(1) * self.channels.max(1) as usize;
            let dropped = ring.cap_samples(cap);
            if dropped > 0 && !self.warned_overflow {
                self.warned_overflow = true;
                eprintln!(
                    "fn64-audio: output ring exceeded {cap} samples; dropped {dropped} oldest                  (producer outrunning the drain -- reported once)"
                );
            }
            !self.stream_started && ring.has_ai_double_buffer()
        };
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

    fn frames_remaining(&self) -> Result<u32, AudioError> {
        if self.stream.is_none() {
            return Err(AudioError::NotReady("create() not called"));
        }
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let channels = self.channels.max(1) as usize;
        Ok((ring.samples.len() / channels) as u32)
    }

    fn set_frequency(&mut self, sample_rate_hz: u32) {
        // Real hardware allows `osAiSetFrequency` mid-game. The live cpal
        // stream keeps its negotiated rate; only the producer-side resample
        // ratio changes, so this is cheap and infallible (matching
        // `RenderBackend::resize`'s contract). A zero rate is a caller bug
        // upstream (`osAiSetFrequency` returns -1 before reaching us) and
        // is ignored rather than poisoning the ratio.
        if sample_rate_hz != 0 {
            self.guest_rate_hz = sample_rate_hz;
        }
    }

    fn stream_rate_hz(&self) -> Option<u32> {
        CpalBackend::stream_rate_hz(self)
    }

    fn current_dma_bytes_remaining(&self) -> Option<u32> {
        self.stream.as_ref()?;
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        Some(ring.current_dma_bytes_remaining())
    }

    fn stream_health(&self) -> Option<AudioStreamHealth> {
        self.stream.as_ref()?;
        Some(AudioStreamHealth {
            callbacks: self.callback_count.load(Ordering::Relaxed),
            requested_samples: self.requested_samples.load(Ordering::Relaxed),
            underrun_samples: self.underrun_samples.load(Ordering::Relaxed),
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
        channels: u16,
        queued_frames: u32,
    }

    impl AudioBackend for FakeBackend {
        fn create(&mut self, cfg: &AudioConfig) -> Result<(), AudioError> {
            self.ready = true;
            self.channels = cfg.channels;
            Ok(())
        }

        fn queue_samples(&mut self, samples: &[i16]) -> Result<(), AudioError> {
            if !self.ready {
                return Err(AudioError::NotReady("create() not called"));
            }
            let channels = self.channels.max(1) as u32;
            self.queued_frames += samples.len() as u32 / channels;
            Ok(())
        }

        fn frames_remaining(&self) -> Result<u32, AudioError> {
            if !self.ready {
                return Err(AudioError::NotReady("create() not called"));
            }
            Ok(self.queued_frames)
        }

        fn set_frequency(&mut self, _sample_rate_hz: u32) {}
    }

    #[test]
    fn is_dyn_safe_and_usable_through_a_trait_object() {
        let mut backend: Box<dyn AudioBackend> = Box::<FakeBackend>::default();
        backend.create(&AudioConfig::new(32000, 2)).unwrap();
        let samples = [0i16; 8]; // 4 stereo frames
        backend.queue_samples(&samples).unwrap();
        assert_eq!(backend.frames_remaining().unwrap(), 4);
    }

    #[test]
    fn queue_samples_before_create_is_not_ready() {
        let mut backend = FakeBackend::default();
        let err = backend.queue_samples(&[0i16; 4]).unwrap_err();
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
        backend.queue_samples(&[1, 2, 3, 4, 5, 6]).unwrap(); // 3 stereo frames
        assert_eq!(backend.frames_remaining().unwrap(), 3);
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
        let err = backend.queue_samples(&[0i16; 2]).unwrap_err();
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
        rs.process(&input, 2, 32000, 32000, &mut out);
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
        rs.process(&input, 2, 32000, 48000, &mut out);
        let frames_out = out.len() / 2;
        let expected = 2000 * 3 / 2;
        assert!(
            frames_out.abs_diff(expected) <= 32,
            "expected ~{expected}, got {frames_out}"
        );
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
        rs.process(&input, 2, 32000, 48000, &mut out);
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
        BandlimitedResampler::new().process(&input, 2, 32000, 48000, &mut whole);

        let mut chunked = Vec::new();
        let mut rs = BandlimitedResampler::new();
        for chunk in [&input[..106], &input[106..340], &input[340..]] {
            rs.process(chunk, 2, 32000, 48000, &mut chunked);
        }
        assert_eq!(whole, chunked);
    }

    #[test]
    fn cap_ring_drops_oldest_keeps_newest() {
        let mut ring: VecDeque<i16> = (0..100).collect();
        assert_eq!(cap_ring(&mut ring, 40), 60);
        assert_eq!(ring.len(), 40);
        assert_eq!(*ring.front().unwrap(), 60, "oldest dropped, newest kept");
        assert_eq!(*ring.back().unwrap(), 99);
        // Under cap: untouched.
        assert_eq!(cap_ring(&mut ring, 40), 0);
        assert_eq!(ring.len(), 40);
    }

    #[test]
    fn drain_ring_counts_every_silence_filled_output_slot() {
        let mut ring = VecDeque::from([10, 20, 30]);
        let mut output = [99; 5];

        assert_eq!(drain_ring(&mut ring, &mut output), 2);
        assert_eq!(output, [10, 20, 30, 0, 0]);
        assert!(ring.is_empty());
    }

    #[test]
    fn ai_length_tracks_only_the_head_dma_not_host_prebuffer() {
        let mut ring = OutputRing::default();
        ring.push_dma(vec![1; 8], 16);
        ring.push_dma(vec![2; 8], 16);
        assert!(ring.has_ai_double_buffer());
        assert_eq!(ring.current_dma_bytes_remaining(), 16);

        let mut output = [0; 6];
        assert_eq!(ring.drain_into(&mut output), 0);
        assert_eq!(ring.current_dma_bytes_remaining(), 4);

        let mut output = [0; 2];
        assert_eq!(ring.drain_into(&mut output), 0);
        assert_eq!(
            ring.current_dma_bytes_remaining(),
            16,
            "the second queued DMA becomes current only after the first drains"
        );
    }

    #[test]
    fn f32_drain_converts_i16_samples_at_the_host_boundary() {
        let mut ring = OutputRing::default();
        ring.push_dma(vec![i16::MIN, -16384, 0, 16384], 8);
        let mut output = [99.0; 6];

        assert_eq!(ring.drain_into_f32(&mut output), 2);
        assert_eq!(output, [-1.0, -0.5, 0.0, 0.5, 0.0, 0.0]);
        assert_eq!(ring.current_dma_bytes_remaining(), 0);
    }

    #[test]
    fn resampler_downsamples_toward_slower_stream() {
        // 32000 -> 22050 (a slower device): fewer frames out than in.
        let mut rs = BandlimitedResampler::new();
        let input: Vec<i16> = (0..400).collect(); // 200 stereo frames
        let mut out = Vec::new();
        rs.process(&input, 2, 32000, 22050, &mut out);
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
        BandlimitedResampler::new().process(&input, 2, 32_000, 48_000, &mut bandlimited);

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
