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
//! 1. **Ucode DECODE/EXECUTION** — interpreting the ~72-opcode RSP vector
//!    instruction set well enough to actually run a real audio ucode
//!    (ABI-2/ABI-3-family mixing, envelope, ADPCM decode, resampling...).
//!    This is real, non-trivial DSP emulation logic, and untangling it from
//!    other open-source N64 audio HLE implementations' GPL-licensed
//!    interpreters (the ones every public reference implementation this
//!    project could look at is built from) is an open licensing question —
//!    **out of scope for this crate until that's resolved**. See
//!    [`UcodeExecutor`] below: every method traps loudly by name. No
//!    silent fake audio, no quietly-vendored GPL logic.
//! 2. **Sample DELIVERY** — once *some* source (a real ucode interpreter
//!    later, a test fixture today) has produced a buffer of finished PCM
//!    samples, getting those samples to the host's actual sound card. This
//!    is ordinary buffer/ring-buffer plumbing with no game-derived logic in
//!    it at all — **this half is real**, implemented against `cpal`
//!    (portable Rust audio I/O, same tier of dependency as this crate's
//!    sibling `fn64-render-rt64` takes on RT64 for its half of the render
//!    seam).
//!
//! `AudioBackend` models half 2. `UcodeExecutor` names half 1 as blocked so
//! that boundary isn't accidentally erased or forgotten later.
#![forbid(unsafe_code)]

pub mod rsp;

use std::collections::VecDeque;
use std::fmt;
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
    /// does not interpret *how* `samples` was produced (real ucode output,
    /// silence, a test tone) — that separation is `UcodeExecutor`'s job,
    /// not this trait's.
    fn queue_samples(&mut self, samples: &[i16]) -> Result<(), AudioError>;

    /// How many queued sample FRAMES (one frame = one sample per channel,
    /// matching libultra's own "sample" vs "frame" usage in
    /// `osAiSetNextBuffer`'s documented semantics) have not yet been
    /// consumed by the host stream. A caller (the executor) uses this to
    /// pace how far ahead of real-time it lets ucode output run, the same
    /// role `FrameStatus` plays for a gfx backend's yield/complete signal.
    fn frames_remaining(&self) -> Result<u32, AudioError>;

    /// Change the output sample rate at runtime, matching real hardware's
    /// `osAiSetFrequency` being callable mid-game (title music vs SFX often
    /// run the AI at different rates). Infallible by design, mirroring
    /// `RenderBackend::resize`: a backend that cannot honor a rate change
    /// should surface that at the next `queue_samples` call via
    /// `AudioError`, not here.
    fn set_frequency(&mut self, sample_rate_hz: u32);

    /// The host stream's actual rate, when the backend knows it. Lets the
    /// AI-register model convert ring depth to guest-rate bytes so
    /// `osAiGetLength` can report a REAL drain-aware value (the feedback
    /// loop games use to pace audio production on hardware). Default `None`
    /// keeps existing/fake backends contract-compatible.
    fn stream_rate_hz(&self) -> Option<u32> {
        None
    }
}

/// Names the RSP audio-ucode EXECUTION boundary as an explicit, loud stub —
/// see this module's doc comment section 1. Not a normal trait meant to be
/// widely implemented today: it exists so the "ucode decode is blocked"
/// fact is a compiler-visible, testable API surface rather than a comment
/// that can silently rot or be worked around by accident.
///
/// A real implementation (once the GPL-derivation question above is
/// resolved, or a from-scratch clean-room interpreter is written against
/// only the public RSP ISA + AI hardware manual, no GPL source in the
/// loop) replaces `LoudStubUcodeExecutor` wholesale; this trait's shape
/// does not need to change for that to happen, matching how `AudioBackend`
/// itself is meant to stay stable underneath a real `CpalBackend`.
pub trait UcodeExecutor {
    /// Interpret and run one audio-ucode task's RSP program against
    /// `rdram`, starting at `ucode_addr` (rdram-relative, matching
    /// `fn64_render::OsTask::ucode`'s own convention). A real
    /// implementation would return the decoded PCM sample frames it
    /// produced, ready for `AudioBackend::queue_samples`.
    fn execute_task(&mut self, rdram: &[u8], ucode_addr: u32) -> Result<Vec<i16>, AudioError>;
}

/// The loud, named stub for `UcodeExecutor`. Every call traps with
/// `AudioError::Backend { backend: "ucode-executor", .. }` naming exactly
/// why: RSP audio-ucode execution (the ~72-op vector interpreter) is real,
/// non-trivial DSP emulation logic and untangling a clean-room
/// implementation from the GPL-licensed public reference interpreters is
/// an open licensing question, per this module's doc comment. This is not
/// a placeholder that quietly returns silence — it is a deliberate,
/// testable trap so no caller can mistake "no sound" for "ucode ran and
/// produced silence."
#[derive(Default)]
pub struct LoudStubUcodeExecutor;

impl UcodeExecutor for LoudStubUcodeExecutor {
    fn execute_task(&mut self, _rdram: &[u8], ucode_addr: u32) -> Result<Vec<i16>, AudioError> {
        Err(AudioError::Backend {
            backend: "ucode-executor",
            reason: format!(
                "audio ucode execution not yet clean-room implemented (GPL-derivation \
                 question pending, see fn64-audio crate doc); refused to fabricate output \
                 for ucode at rdram offset {ucode_addr:#010x}"
            ),
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
type SampleRing = Arc<Mutex<VecDeque<i16>>>;

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
    /// The rate the host stream actually runs at, negotiated in [`create`]:
    /// the requested guest rate when the device accepts it, else the
    /// device's default output rate (macOS commonly rejects the N64's
    /// 32 kHz). Playing guest-rate samples on a faster stream without
    /// conversion starves the ring ~proportionally and the callback's
    /// zero-fill turns that into loud static — hence [`LinearResampler`].
    stream_rate_hz: u32,
    /// The guest-side production rate the queued samples arrive at. Seeded
    /// from the `create` config, updated live by `set_frequency`
    /// (`osAiSetFrequency`).
    guest_rate_hz: u32,
    resampler: LinearResampler,
    /// One-shot flag so ring-overflow drops are reported loudly exactly once
    /// per stream, not once per queue call.
    warned_overflow: bool,
}

impl Default for CpalBackend {
    fn default() -> Self {
        CpalBackend::new()
    }
}

impl CpalBackend {
    pub fn new() -> Self {
        CpalBackend {
            ring: Arc::new(Mutex::new(VecDeque::new())),
            channels: 2,
            stream: None,
            stream_rate_hz: 0,
            guest_rate_hz: 0,
            resampler: LinearResampler::new(),
            warned_overflow: false,
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
fn cap_ring(ring: &mut VecDeque<i16>, cap: usize) -> usize {
    let excess = ring.len().saturating_sub(cap);
    if excess > 0 {
        ring.drain(..excess);
    }
    excess
}

/// Stateful linear resampler over interleaved `i16` frames. Carries the
/// last input frame and the fractional read position across calls, so
/// chunked `queue_samples` input splices without discontinuities (a seam
/// pop per AI buffer would itself be audible crackle).
struct LinearResampler {
    /// Last input frame seen (len = channels); empty until first input.
    carry: Vec<i16>,
    /// Fractional read position in [0, 1) measured from `carry`.
    phase: f64,
}

impl LinearResampler {
    fn new() -> Self {
        LinearResampler {
            carry: Vec::new(),
            phase: 0.0,
        }
    }

    /// Convert `input` (interleaved, `channels` per frame, produced at
    /// `in_hz`) to `out_hz`, appending to `out`. Equal rates pass through
    /// unchanged (byte-identical to the pre-resampler behavior).
    fn process(&mut self, input: &[i16], channels: usize, in_hz: u32, out_hz: u32, out: &mut Vec<i16>) {
        debug_assert!(channels > 0 && in_hz > 0 && out_hz > 0);
        let in_frames = input.len() / channels;
        if in_hz == out_hz {
            // Keep the carry coherent so a mid-stream rate change (a game
            // calling osAiSetFrequency) still splices continuously.
            if in_frames > 0 {
                self.carry.clear();
                self.carry
                    .extend_from_slice(&input[(in_frames - 1) * channels..in_frames * channels]);
            }
            out.extend_from_slice(input);
            return;
        }

        let carry = std::mem::take(&mut self.carry);
        let have_carry = !carry.is_empty();
        // Virtual input timeline: the carry frame (if any) at index 0, then
        // this call's frames. Interpolation needs two frames.
        let total = in_frames + usize::from(have_carry);
        let frame_at = |i: usize| -> &[i16] {
            if have_carry && i == 0 {
                &carry
            } else {
                let j = i - usize::from(have_carry);
                &input[j * channels..(j + 1) * channels]
            }
        };
        if total < 2 {
            if in_frames > 0 {
                self.carry = frame_at(total - 1).to_vec();
            } else {
                self.carry = carry; // nothing new; keep prior state
            }
            return;
        }

        let step = in_hz as f64 / out_hz as f64;
        let mut pos = self.phase;
        let last = (total - 1) as f64;
        while pos < last {
            let i = pos as usize;
            let frac = pos - i as f64;
            let a = frame_at(i);
            let b = frame_at(i + 1);
            for ch in 0..channels {
                let s = f64::from(a[ch]) + (f64::from(b[ch]) - f64::from(a[ch])) * frac;
                out.push(s.round() as i16);
            }
            pos += step;
        }
        // Rebase onto the final frame, which becomes the next call's carry.
        self.phase = pos - last;
        self.carry = frame_at(total - 1).to_vec();
    }
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
            let mut first_pull = true;
            device
                .build_output_stream(
                    stream_config,
                    move |data: &mut [i16], _info: &cpal::OutputCallbackInfo| {
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
                                ring.len()
                            );
                        }
                        for slot in data.iter_mut() {
                            *slot = ring.pop_front().unwrap_or(0);
                        }
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
                     device-default {fallback_hz} Hz with linear resampling",
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

        stream.play().map_err(|e| AudioError::Backend {
            backend: "cpal",
            reason: format!("stream.play failed: {e}"),
        })?;

        self.channels = cfg.channels;
        self.stream_rate_hz = stream_rate_hz;
        self.guest_rate_hz = cfg.sample_rate_hz;
        self.resampler = LinearResampler::new();
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
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.extend(converted);
        // Bound output latency: ~250 ms of stream audio. The shell's
        // audio-clocked pacing keeps the ring an order of magnitude below
        // this; the cap is the backstop for unpaced producers.
        let cap = (self.stream_rate_hz as usize / 4).max(1) * self.channels.max(1) as usize;
        let dropped = cap_ring(&mut ring, cap);
        if dropped > 0 && !self.warned_overflow {
            self.warned_overflow = true;
            eprintln!(
                "fn64-audio: output ring exceeded {cap} samples; dropped {dropped} oldest                  (producer outrunning the drain -- reported once)"
            );
        }
        Ok(())
    }

    fn frames_remaining(&self) -> Result<u32, AudioError> {
        if self.stream.is_none() {
            return Err(AudioError::NotReady("create() not called"));
        }
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let channels = self.channels.max(1) as usize;
        Ok((ring.len() / channels) as u32)
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
    fn loud_stub_ucode_executor_traps_by_name_not_silently() {
        let mut executor = LoudStubUcodeExecutor;
        let rdram = vec![0u8; 16];
        let err = executor.execute_task(&rdram, 0x8001_2340).unwrap_err();
        match err {
            AudioError::Backend { backend, reason } => {
                assert_eq!(backend, "ucode-executor");
                assert!(reason.contains("not yet clean-room implemented"));
                assert!(reason.contains("80012340"));
            }
            other => panic!("expected AudioError::Backend, got {other:?}"),
        }
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

    // --- LinearResampler (the static-fix core; device-less, pure) --------

    #[test]
    fn resampler_equal_rates_pass_through_unchanged() {
        let mut rs = LinearResampler::new();
        let input: Vec<i16> = (0..64).collect();
        let mut out = Vec::new();
        rs.process(&input, 2, 32000, 32000, &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn resampler_32k_to_48k_produces_three_frames_per_two() {
        // 32000 -> 48000 is exactly 2:3. 200 stereo input frames must yield
        // ~300 output frames (± the carried boundary frame).
        let mut rs = LinearResampler::new();
        let input: Vec<i16> = (0..400).collect(); // 200 stereo frames
        let mut out = Vec::new();
        rs.process(&input, 2, 32000, 48000, &mut out);
        let frames_out = out.len() / 2;
        assert!(
            (298..=300).contains(&frames_out),
            "expected ~300 output frames for 200 input frames at 2:3, got {frames_out}"
        );
    }

    #[test]
    fn resampler_preserves_stereo_channel_identity() {
        // L = constant 1000, R = constant -2000: linear interpolation of
        // constants is the constant, so ANY cross-channel mixup (the L/R
        // interleave bug class) shows up as a wrong value immediately.
        let mut rs = LinearResampler::new();
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
        LinearResampler::new().process(&input, 2, 32000, 48000, &mut whole);

        let mut chunked = Vec::new();
        let mut rs = LinearResampler::new();
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
    fn resampler_downsamples_toward_slower_stream() {
        // 32000 -> 22050 (a slower device): fewer frames out than in.
        let mut rs = LinearResampler::new();
        let input: Vec<i16> = (0..400).collect(); // 200 stereo frames
        let mut out = Vec::new();
        rs.process(&input, 2, 32000, 22050, &mut out);
        let frames_out = out.len() / 2;
        let expected = (200.0 * 22050.0 / 32000.0) as usize; // ~137
        assert!(
            frames_out.abs_diff(expected) <= 2,
            "expected ~{expected} output frames, got {frames_out}"
        );
    }
}
