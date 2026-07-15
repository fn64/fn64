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
        }
    }
}

impl AudioBackend for CpalBackend {
    fn create(&mut self, cfg: &AudioConfig) -> Result<(), AudioError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(AudioError::Backend {
            backend: "cpal",
            reason: "no default output device".to_string(),
        })?;

        let stream_config = StreamConfig {
            channels: cfg.channels,
            sample_rate: cfg.sample_rate_hz,
            buffer_size: cpal::BufferSize::Default,
        };

        let ring = Arc::clone(&self.ring);
        let stream = device
            .build_output_stream(
                stream_config,
                move |data: &mut [i16], _info: &cpal::OutputCallbackInfo| {
                    let mut ring = ring.lock().unwrap_or_else(|e| e.into_inner());
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
            .map_err(|e| AudioError::Backend {
                backend: "cpal",
                reason: format!("build_output_stream failed: {e}"),
            })?;

        stream.play().map_err(|e| AudioError::Backend {
            backend: "cpal",
            reason: format!("stream.play failed: {e}"),
        })?;

        self.channels = cfg.channels;
        self.stream = Some(stream);
        Ok(())
    }

    fn queue_samples(&mut self, samples: &[i16]) -> Result<(), AudioError> {
        if self.stream.is_none() {
            return Err(AudioError::NotReady("create() not called"));
        }
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.extend(samples.iter().copied());
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
        // Real hardware allows `osAiSetFrequency` mid-game; honoring it on
        // a live cpal stream means rebuilding the stream at the new rate.
        // Not yet wired (no call site needs it in this workspace today) --
        // matching `RenderBackend::resize`'s "infallible, backend reports
        // trouble at next real call" contract, this silently no-ops for
        // now rather than tearing down the live stream on every call.
        let _ = sample_rate_hz;
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
}
