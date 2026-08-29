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
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use fn64_runtime::device::AiSamplePeriod;

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
/// every interleaved output slot filled with host-inserted silence;
/// `contention_sample_slots` identifies the subset for which the realtime
/// callback could not inspect the producer ring because its lock was busy.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioStreamHealth {
    pub callbacks: u64,
    pub requested_sample_slots: HostSampleSlotCount,
    pub underrun_sample_slots: HostSampleSlotCount,
    pub contention_sample_slots: HostSampleSlotCount,
    pub dropped_sample_slots: HostSampleSlotCount,
    pub late_callbacks: u64,
    pub max_callback_gap_us: u64,
}

/// Host-side work active when the realtime audio callback began.
///
/// This is diagnostic ownership telemetry, not an N64 device state and never
/// feeds the executor, AI, VI, or any guest-visible clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostExecutionPhase {
    #[default]
    Waiting,
    GuestStep,
    DeviceAdvance,
    ViScanout,
    WindowPresent,
}

impl HostExecutionPhase {
    const fn encode(self) -> u8 {
        match self {
            Self::Waiting => 0,
            Self::GuestStep => 1,
            Self::DeviceAdvance => 2,
            Self::ViScanout => 3,
            Self::WindowPresent => 4,
        }
    }

    fn decode(value: u8) -> Self {
        match value {
            0 => Self::Waiting,
            1 => Self::GuestStep,
            2 => Self::DeviceAdvance,
            3 => Self::ViScanout,
            4 => Self::WindowPresent,
            other => panic!("invalid host execution phase encoding {other}"),
        }
    }
}

/// Why one host callback inserted silence into the output stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioUnderrunReason {
    RingEmpty,
    RingShort,
    ProducerContention,
}

/// One content-free realtime callback observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioUnderrunObservation {
    pub sequence: u64,
    pub callback_at: std::time::Instant,
    pub reason: AudioUnderrunReason,
    pub requested_sample_slots: HostSampleSlotCount,
    pub delivered_sample_slots: HostSampleSlotCount,
    /// `None` means producer contention prevented an honest ring inspection.
    pub ring_sample_slots_before: Option<HostSampleSlotCount>,
    pub phase: HostExecutionPhase,
}

/// Content-free host-buffer evidence used to join producer admission to
/// callback geometry without exposing PCM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioBufferObservation {
    DmaQueued {
        sequence: u64,
        dma_id: fn64_runtime::AiDmaId,
        queued_at: std::time::Instant,
        resampled_sample_slots: HostSampleSlotCount,
        ring_sample_slots_after: HostSampleSlotCount,
        host_sample_rate_hz: HostSampleRateHz,
        channels: ChannelCount,
    },
    CallbackGeometry {
        sequence: u64,
        callback_at: std::time::Instant,
        requested_sample_slots: HostSampleSlotCount,
    },
}

const BUFFER_OBSERVATION_CAPACITY: usize = 256;

#[derive(Debug)]
struct AudioBufferProbeShared {
    next_sequence: AtomicU64,
    lost: AtomicU64,
    last_callback_slots: AtomicU64,
    observations: Mutex<VecDeque<AudioBufferObservation>>,
}

/// Opt-in bounded producer/callback geometry transport. Queue-side producers
/// may take its lock; the realtime callback only tries it and accounts loss.
#[derive(Clone, Debug)]
pub struct AudioBufferProbe(Arc<AudioBufferProbeShared>);

impl Default for AudioBufferProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioBufferProbe {
    pub fn new() -> Self {
        Self(Arc::new(AudioBufferProbeShared {
            next_sequence: AtomicU64::new(1),
            lost: AtomicU64::new(0),
            last_callback_slots: AtomicU64::new(u64::MAX),
            observations: Mutex::new(VecDeque::with_capacity(BUFFER_OBSERVATION_CAPACITY)),
        }))
    }

    pub fn drain(&self, output: &mut Vec<AudioBufferObservation>) -> u64 {
        let mut observations = self
            .0
            .observations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        output.extend(observations.drain(..));
        self.0.lost.swap(0, Ordering::AcqRel)
    }

    fn next_sequence(&self) -> u64 {
        let sequence = self.0.next_sequence.fetch_add(1, Ordering::Relaxed);
        assert_ne!(sequence, u64::MAX, "audio buffer observation sequence overflow");
        sequence
    }

    fn record_dma_queued(
        &self,
        dma_id: fn64_runtime::AiDmaId,
        queued_at: std::time::Instant,
        resampled_sample_slots: usize,
        ring_sample_slots_after: usize,
        host_sample_rate_hz: HostSampleRateHz,
        channels: ChannelCount,
    ) {
        let mut observations = self
            .0
            .observations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if observations.len() == BUFFER_OBSERVATION_CAPACITY {
            self.0.lost.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // Interleaving closed: a callback and DMA producer may both be ready
        // to publish, but the one acquiring this queue second must not retain
        // an earlier sequence than the record already appended by the first.
        let sequence = self.next_sequence();
        observations.push_back(AudioBufferObservation::DmaQueued {
            sequence,
            dma_id,
            queued_at,
            resampled_sample_slots: HostSampleSlotCount::new(resampled_sample_slots as u64),
            ring_sample_slots_after: HostSampleSlotCount::new(ring_sample_slots_after as u64),
            host_sample_rate_hz,
            channels,
        });
    }

    fn record_callback_geometry(&self, callback_at: std::time::Instant, requested_slots: usize) {
        let requested_slots = requested_slots as u64;
        if self
            .0
            .last_callback_slots
            .swap(requested_slots, Ordering::Relaxed)
            == requested_slots
        {
            return;
        }
        let retain = |mut observations: MutexGuard<'_, VecDeque<AudioBufferObservation>>| {
            if observations.len() == BUFFER_OBSERVATION_CAPACITY {
                self.0.lost.fetch_add(1, Ordering::Relaxed);
            } else {
                // Sequence allocation stays under the same queue lock as the
                // append; see `record_dma_queued`'s cross-producer invariant.
                let sequence = self.next_sequence();
                observations.push_back(AudioBufferObservation::CallbackGeometry {
                    sequence,
                    callback_at,
                    requested_sample_slots: HostSampleSlotCount::new(requested_slots),
                });
            }
        };
        match self.0.observations.try_lock() {
            Ok(observations) => retain(observations),
            Err(TryLockError::Poisoned(error)) => retain(error.into_inner()),
            Err(TryLockError::WouldBlock) => {
                self.0.lost.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

const UNDERRUN_OBSERVATION_CAPACITY: usize = 64;

#[derive(Debug)]
struct HostExecutionProbeShared {
    phase: AtomicU8,
    next_sequence: AtomicU64,
    lost: AtomicU64,
    observations: Mutex<VecDeque<AudioUnderrunObservation>>,
}

/// Shared host-activity and underrun probe for the emulation and audio threads.
///
/// The callback producer never allocates or waits: it uses a bounded,
/// preallocated queue behind `try_lock`. The emulation-side consumer must drain
/// regularly; a busy or full queue increments an explicit loss count instead
/// of blocking the device callback or overwriting an unread observation.
#[derive(Clone, Debug)]
pub struct HostExecutionProbe(Arc<HostExecutionProbeShared>);

impl Default for HostExecutionProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl HostExecutionProbe {
    pub fn new() -> Self {
        Self(Arc::new(HostExecutionProbeShared {
            phase: AtomicU8::new(HostExecutionPhase::Waiting.encode()),
            next_sequence: AtomicU64::new(1),
            lost: AtomicU64::new(0),
            observations: Mutex::new(VecDeque::with_capacity(UNDERRUN_OBSERVATION_CAPACITY)),
        }))
    }

    /// Replace the current host activity and return the prior value so a
    /// higher layer can restore nested phases with a move-only guard.
    pub fn set_phase(&self, phase: HostExecutionPhase) -> HostExecutionPhase {
        HostExecutionPhase::decode(self.0.phase.swap(phase.encode(), Ordering::AcqRel))
    }

    pub fn phase(&self) -> HostExecutionPhase {
        HostExecutionPhase::decode(self.0.phase.load(Ordering::Acquire))
    }

    /// Drain every retained observation and return the number dropped since
    /// the preceding drain. Appending to `output` may allocate on this
    /// consumer thread; the realtime producer remains allocation-free.
    pub fn drain_underrun_observations(&self, output: &mut Vec<AudioUnderrunObservation>) -> u64 {
        let mut observations = self
            .0
            .observations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        output.extend(observations.drain(..));
        self.0.lost.swap(0, Ordering::AcqRel)
    }

    fn record_underrun(
        &self,
        callback_at: std::time::Instant,
        outcome: DrainOutcome,
        phase: HostExecutionPhase,
    ) {
        let Some(reason) = outcome.underrun_reason else {
            return;
        };
        let retain = |mut observations: MutexGuard<'_, VecDeque<AudioUnderrunObservation>>| {
            // Allocate the sequence while holding the publication lock. Two
            // overlapping host callbacks can sample wall time in one order
            // and reach this point in the other; the sequence names retained
            // publication order, while callback_at retains occurrence time.
            let sequence = self.0.next_sequence.fetch_add(1, Ordering::Relaxed);
            assert_ne!(
                sequence,
                u64::MAX,
                "audio underrun observation sequence overflow"
            );
            if observations.len() == UNDERRUN_OBSERVATION_CAPACITY {
                self.0.lost.fetch_add(1, Ordering::Relaxed);
            } else {
                observations.push_back(AudioUnderrunObservation {
                    sequence,
                    callback_at,
                    reason,
                    requested_sample_slots: HostSampleSlotCount::new(
                        outcome.requested_sample_slots as u64,
                    ),
                    delivered_sample_slots: HostSampleSlotCount::new(
                        outcome.delivered_sample_slots as u64,
                    ),
                    ring_sample_slots_before: outcome
                        .ring_sample_slots_before
                        .map(|value| HostSampleSlotCount::new(value as u64)),
                    phase,
                });
            }
        };
        match self.0.observations.try_lock() {
            Ok(observations) => retain(observations),
            Err(TryLockError::Poisoned(error)) => retain(error.into_inner()),
            Err(TryLockError::WouldBlock) => {
                // Exact interleaving: the emulation thread may be draining
                // while the realtime callback reports an underrun. The
                // callback never waits behind that consumer; it accounts one
                // explicit telemetry loss and returns to the audio device.
                let sequence = self.0.next_sequence.fetch_add(1, Ordering::Relaxed);
                assert_ne!(
                    sequence,
                    u64::MAX,
                    "audio underrun observation sequence overflow"
                );
                self.0.lost.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Host-stream startup boundaries for the first active hardware AI DMA.
/// These are presentation observations only; none feeds guest device time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioStreamStartLandmark {
    pub dma_id: fn64_runtime::AiDmaId,
    pub payload_queued_at: std::time::Instant,
    pub dma_started_at: fn64_runtime::EmulatedInstant,
    /// Return from the one real host `Stream::play` call. With explicit host
    /// preactivation this may precede both payload queueing and guest DMA
    /// start; it is not the guest-authorized delivery boundary.
    pub play_returned_at: std::time::Instant,
    /// Host time sampled immediately before the Release that permits the
    /// realtime callback to consume guest PCM.
    pub delivery_activated_at: std::time::Instant,
    pub first_callback_at: Option<std::time::Instant>,
}

/// One opt-in PCM landmark observed on both sides of host delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioSyncLandmark {
    pub dma_id: fn64_runtime::AiDmaId,
    pub guest_frame_offset: u64,
    pub dma_started_at: Option<fn64_runtime::Cycles>,
    pub start_dacrate: Option<u32>,
    pub predicted_playback_at: Option<std::time::Instant>,
    /// Host-audio continuity generation at the callback that contained the
    /// selected output sample. A later generation invalidates the cue.
    pub continuity_generation: Option<u64>,
    pub dropped_before_playback: bool,
    pub retimed_after_start: bool,
}

/// A correlation point between the deterministic emulated AI timeline and
/// the host audio device's predicted playback timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioPresentationAnchor {
    pub dma_id: fn64_runtime::AiDmaId,
    pub emulated_at: fn64_runtime::EmulatedInstant,
    pub predicted_playback_at: std::time::Instant,
    pub continuity_generation: u64,
}

/// Current validity generation for host-audio presentation timing, together
/// with an anchor only when both the emulated start and host playback marker
/// belong to that generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioPresentationState {
    pub continuity_generation: u64,
    pub anchor: Option<AudioPresentationAnchor>,
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

    /// Queue PCM captured for one accepted hardware FIFO entry. Backends that
    /// do not observe device timing retain the ordinary sample-only path.
    fn queue_dma(
        &mut self,
        id: fn64_runtime::AiDmaId,
        pcm: GuestPcm16<'_>,
    ) -> Result<(), AudioError> {
        let _ = id;
        self.queue_samples(pcm)
    }

    /// Observe the exact guest cycle at which an accepted FIFO entry became
    /// the active DAC transfer. Submission and start are deliberately split.
    fn notify_dma_started(&mut self, start: fn64_runtime::AiDmaStart) -> Result<(), AudioError> {
        let _ = start;
        Ok(())
    }

    fn notify_dma_retimed(&mut self, id: fn64_runtime::AiDmaId) {
        let _ = id;
    }

    fn sync_landmark(&self) -> Option<AudioSyncLandmark> {
        None
    }

    /// Current host-presentation continuity state. This never advances AI,
    /// the executor, or any other guest-visible clock. A supported backend
    /// returns `Some` even while its current generation has no complete
    /// anchor, allowing diagnostics to reject an older correlation
    /// immediately.
    fn presentation_state(&self) -> Option<AudioPresentationState> {
        None
    }

    fn stream_start_landmark(&self) -> Option<AudioStreamStartLandmark> {
        None
    }

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

    /// Change the producer rate without discarding the fractional rate
    /// selected by `video_clock / (AI_DACRATE + 1)`. Existing backends retain
    /// their whole-Hz behavior through this compatibility default.
    fn set_sample_period(&mut self, period: AiSamplePeriod) {
        self.set_frequency(GuestSampleRateHz::new(period.floor_hz()));
    }

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

    /// Replace the host activity sampled by an attached realtime diagnostic
    /// probe. Backends without that probe remain explicit through `None`.
    fn set_host_execution_phase(&self, phase: HostExecutionPhase) -> Option<HostExecutionPhase> {
        let _ = phase;
        None
    }

    fn host_execution_probe_enabled(&self) -> bool {
        false
    }

    /// Drain callback observations without making the realtime callback own
    /// an allocator or a blocking output channel. Returns observations lost
    /// because the bounded transport was full or busy with its consumer.
    fn drain_underrun_observations(&self, output: &mut Vec<AudioUnderrunObservation>) -> u64 {
        let _ = output;
        0
    }

    /// Drain opt-in content-free producer/callback buffer observations.
    /// Returns records lost to bounded capacity or callback-side contention.
    fn drain_buffer_observations(&self, output: &mut Vec<AudioBufferObservation>) -> u64 {
        let _ = output;
        0
    }

    /// Clone the attached buffer probe so terminal host teardown can stop the
    /// callback producer before performing its final drain.
    fn buffer_probe(&self) -> Option<AudioBufferProbe> {
        None
    }

    /// Clone the attached diagnostic transport so terminal host teardown can
    /// stop the callback producer before performing its final drain.
    fn host_execution_probe(&self) -> Option<HostExecutionProbe> {
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
    landmarks: [Option<OutputLandmark>; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LandmarkKind {
    Presentation,
    Sync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OutputLandmark {
    kind: LandmarkKind,
    dma_id: fn64_runtime::AiDmaId,
    sample_slots_until: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct OutputLandmarks {
    presentation: Option<(fn64_runtime::AiDmaId, usize)>,
    sync: Option<(fn64_runtime::AiDmaId, usize)>,
}

impl OutputLandmarks {
    fn record(&mut self, kind: LandmarkKind, id: fn64_runtime::AiDmaId, offset: usize) {
        match kind {
            LandmarkKind::Presentation => self.presentation = Some((id, offset)),
            LandmarkKind::Sync => self.sync = Some((id, offset)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RingPushOutcome {
    dropped_sample_slots: usize,
    dropped_presentation: bool,
    dropped_sync_landmark: Option<fn64_runtime::AiDmaId>,
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
    fn push_dma(
        &mut self,
        output: &[i16],
        guest_bytes: GuestDmaByteCount,
        landmarks: OutputLandmarks,
    ) -> RingPushOutcome {
        if output.is_empty() {
            return RingPushOutcome::default();
        }
        let dropped = self
            .samples
            .len()
            .saturating_add(output.len())
            .saturating_sub(self.sample_cap);
        let old_dropped = dropped.min(self.samples.len());
        let mut dropped_landmarks = OutputLandmarks::default();
        if old_dropped > 0 {
            self.samples.drain(..old_dropped);
            dropped_landmarks = self.consume_spans(old_dropped);
        }
        let input_dropped = dropped - old_dropped;
        if input_dropped < output.len() {
            let retain = |kind, landmark: Option<(fn64_runtime::AiDmaId, usize)>| {
                landmark.and_then(|(dma_id, offset)| {
                    (offset >= input_dropped).then_some(OutputLandmark {
                        kind,
                        dma_id,
                        sample_slots_until: offset.saturating_sub(input_dropped),
                    })
                })
            };
            if let Some((id, offset)) = landmarks.presentation {
                if offset < input_dropped {
                    dropped_landmarks.presentation = Some((id, offset));
                }
            }
            if let Some((id, offset)) = landmarks.sync {
                if offset < input_dropped {
                    dropped_landmarks.sync = Some((id, offset));
                }
            }
            self.dmas.push_back(DmaSpan {
                output_samples_total: output.len(),
                output_samples_remaining: output.len() - input_dropped,
                guest_bytes_total: guest_bytes,
                landmarks: [
                    retain(LandmarkKind::Presentation, landmarks.presentation),
                    retain(LandmarkKind::Sync, landmarks.sync),
                ],
            });
        } else {
            dropped_landmarks = landmarks;
        }
        self.samples.extend(&output[input_dropped..]);
        RingPushOutcome {
            dropped_sample_slots: dropped,
            dropped_presentation: dropped_landmarks.presentation.is_some(),
            dropped_sync_landmark: dropped_landmarks.sync.map(|entry| entry.0),
        }
    }

    fn consume_spans(
        &mut self,
        mut samples: usize,
    ) -> OutputLandmarks {
        let mut consumed_total = 0;
        let mut crossed = OutputLandmarks::default();
        while samples > 0 {
            let Some(front) = self.dmas.front_mut() else {
                break;
            };
            let consumed = samples.min(front.output_samples_remaining);
            for landmark in &mut front.landmarks {
                if let Some(value) = landmark.as_mut() {
                    if value.sample_slots_until < consumed {
                        crossed.record(
                            value.kind,
                            value.dma_id,
                            consumed_total + value.sample_slots_until,
                        );
                        *landmark = None;
                    } else {
                        value.sample_slots_until -= consumed;
                    }
                }
            }
            front.output_samples_remaining -= consumed;
            samples -= consumed;
            consumed_total += consumed;
            if front.output_samples_remaining == 0 {
                self.dmas.pop_front();
            }
        }
        crossed
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
        let _ = self.consume_spans(delivered);
        output.len() - delivered
    }

    fn drain_into_f32(
        &mut self,
        output: &mut [f32],
    ) -> (usize, OutputLandmarks) {
        let delivered = output.len().min(self.samples.len());
        {
            let (front, back) = self.samples.as_slices();
            for (slot, sample) in output[..delivered].iter_mut().zip(front.iter().chain(back)) {
                *slot = f32::from(*sample) / 32768.0;
            }
        }
        self.samples.drain(..delivered);
        output[delivered..].fill(0.0);
        let landmark = self.consume_spans(delivered);
        (output.len() - delivered, landmark)
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

    #[cfg(test)]
    fn has_ai_double_buffer(&self) -> bool {
        self.dmas.len() >= 2
    }
}

fn drain_ring_into_f32(
    mut ring: MutexGuard<'_, OutputRing>,
    output: &mut [f32],
) -> (usize, OutputLandmarks) {
    ring.drain_into_f32(output)
}

/// Never wait on the emulation thread from cpal's realtime callback. A busy
/// producer is indistinguishable from an empty ring for this one pull: both
/// must become silence and both are included in `underrun_sample_slots`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DrainOutcome {
    requested_sample_slots: usize,
    delivered_sample_slots: usize,
    underrun_sample_slots: usize,
    contention_sample_slots: usize,
    ring_sample_slots_before: Option<usize>,
    underrun_reason: Option<AudioUnderrunReason>,
    landmarks: OutputLandmarks,
}

#[derive(Debug, Default)]
struct AudioSyncProbeShared {
    dma_id: AtomicU64,
    guest_frame_offset: AtomicU64,
    start_cycle_plus_one: AtomicU64,
    start_dacrate: AtomicU64,
    predicted_playback_ns_plus_one: AtomicU64,
    continuity_generation: AtomicU64,
    dropped: AtomicU64,
    retimed: AtomicU64,
}

#[derive(Debug)]
struct AudioSyncProbeProducer {
    threshold: i16,
    quiet_ms: u64,
    quiet_frames: u64,
    selected: bool,
}

impl AudioSyncProbeProducer {
    fn from_env() -> Option<Self> {
        std::env::var_os("FN64_AV_SYNC_PROBE")?;
        let threshold = std::env::var("FN64_AV_SYNC_THRESHOLD")
            .ok()
            .map(|value| value.parse::<i16>().expect("FN64_AV_SYNC_THRESHOLD must be i16"))
            .unwrap_or(512)
            .max(1);
        let quiet_ms = std::env::var("FN64_AV_SYNC_QUIET_MS")
            .ok()
            .map(|value| value.parse::<u64>().expect("FN64_AV_SYNC_QUIET_MS must be u64"))
            .unwrap_or(750);
        Some(Self { threshold, quiet_ms, quiet_frames: 0, selected: false })
    }

    fn inspect(
        &mut self,
        pcm: GuestPcm16<'_>,
        rate: GuestSampleRateHz,
    ) -> Option<usize> {
        if self.selected {
            return None;
        }
        let required = u64::from(rate.get()).saturating_mul(self.quiet_ms).div_ceil(1_000);
        for (frame_offset, frame) in pcm.samples().chunks_exact(pcm.channels().as_usize()).enumerate() {
            let loud = frame.iter().any(|sample| sample.unsigned_abs() >= self.threshold as u16);
            if loud {
                if self.quiet_frames >= required {
                    self.selected = true;
                    return Some(frame_offset);
                }
                self.quiet_frames = 0;
            } else {
                self.quiet_frames = self.quiet_frames.saturating_add(1);
            }
        }
        None
    }
}

fn try_drain_ring_into_f32(ring: &SampleRing, output: &mut [f32]) -> DrainOutcome {
    match ring.try_lock() {
        Ok(guard) => {
            let ring_sample_slots_before = guard.samples.len();
            let (underrun_sample_slots, landmarks) = drain_ring_into_f32(guard, output);
            DrainOutcome::inspected(
                output.len(),
                underrun_sample_slots,
                ring_sample_slots_before,
                landmarks,
            )
        }
        Err(TryLockError::Poisoned(error)) => {
            let guard = error.into_inner();
            let ring_sample_slots_before = guard.samples.len();
            let (underrun_sample_slots, landmarks) = drain_ring_into_f32(guard, output);
            DrainOutcome::inspected(
                output.len(),
                underrun_sample_slots,
                ring_sample_slots_before,
                landmarks,
            )
        }
        Err(TryLockError::WouldBlock) => {
            output.fill(0.0);
            DrainOutcome {
                requested_sample_slots: output.len(),
                delivered_sample_slots: 0,
                underrun_sample_slots: output.len(),
                contention_sample_slots: output.len(),
                ring_sample_slots_before: None,
                underrun_reason: (!output.is_empty())
                    .then_some(AudioUnderrunReason::ProducerContention),
                landmarks: OutputLandmarks::default(),
            }
        }
    }
}

fn admit_host_pcm_delivery(gate: &HostPcmDeliveryGate, output: &mut [f32]) -> bool {
    if !gate.is_active() {
        output.fill(0.0);
        return false;
    }
    true
}

impl DrainOutcome {
    fn inspected(
        requested_sample_slots: usize,
        underrun_sample_slots: usize,
        ring_sample_slots_before: usize,
        landmarks: OutputLandmarks,
    ) -> Self {
        let delivered_sample_slots = requested_sample_slots
            .checked_sub(underrun_sample_slots)
            .expect("audio drain underrun exceeded callback request");
        let underrun_reason =
            (underrun_sample_slots != 0).then_some(if ring_sample_slots_before == 0 {
                AudioUnderrunReason::RingEmpty
            } else {
                AudioUnderrunReason::RingShort
            });
        Self {
            requested_sample_slots,
            delivered_sample_slots,
            underrun_sample_slots,
            contention_sample_slots: 0,
            ring_sample_slots_before: Some(ring_sample_slots_before),
            underrun_reason,
            landmarks,
        }
    }
}

const PRESENTATION_SLOT_COUNT: usize = 64;

#[derive(Debug, Default)]
struct AudioPresentationSlot {
    dma_id: AtomicU64,
    start_cycle_plus_one: AtomicU64,
    predicted_playback_ns_plus_one: AtomicU64,
    continuity_generation: AtomicU64,
}

#[derive(Debug)]
struct AudioPresentationShared {
    slots: [AudioPresentationSlot; PRESENTATION_SLOT_COUNT],
    latest_complete_dma_id: AtomicU64,
    continuity_generation: AtomicU64,
}

impl AudioPresentationShared {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| AudioPresentationSlot::default()),
            latest_complete_dma_id: AtomicU64::new(0),
            continuity_generation: AtomicU64::new(1),
        }
    }

    fn slot(&self, id: fn64_runtime::AiDmaId) -> &AudioPresentationSlot {
        &self.slots[id.get() as usize % PRESENTATION_SLOT_COUNT]
    }

    fn queue(&self, id: fn64_runtime::AiDmaId) {
        let slot = self.slot(id);
        slot.dma_id.store(0, Ordering::Release);
        slot.start_cycle_plus_one.store(0, Ordering::Relaxed);
        slot.predicted_playback_ns_plus_one
            .store(0, Ordering::Relaxed);
        slot.continuity_generation.store(0, Ordering::Relaxed);
        slot.dma_id.store(id.get(), Ordering::Release);
    }

    fn record_start(&self, start: fn64_runtime::AiDmaStart) {
        // Interleaving closed here and in `record_playback`: after `queue`
        // publishes the slot identity, the emulation-thread AI start and the
        // realtime callback marker may arrive in either order. Neither side
        // publishes `latest_complete_dma_id` until an Acquire observes the
        // other side's Release, so readers cannot receive a half-anchor.
        let slot = self.slot(start.id);
        assert_eq!(
            slot.dma_id.load(Ordering::Acquire),
            start.id.get(),
            "AI DMA start must follow presentation-marker queue admission"
        );
        let encoded = start
            .started_at
            .get()
            .checked_add(1)
            .expect("AI DMA start cycle cannot be encoded for host presentation");
        slot.start_cycle_plus_one.store(encoded, Ordering::Release);
        if slot
            .predicted_playback_ns_plus_one
            .load(Ordering::Acquire)
            != 0
        {
            self.latest_complete_dma_id
                .store(start.id.get(), Ordering::Release);
        }
    }

    fn record_playback(&self, id: fn64_runtime::AiDmaId, encoded_ns: u64) {
        let slot = self.slot(id);
        if slot.dma_id.load(Ordering::Acquire) != id.get() {
            return;
        }
        slot.continuity_generation.store(
            self.continuity_generation.load(Ordering::Acquire),
            Ordering::Relaxed,
        );
        slot.predicted_playback_ns_plus_one
            .store(encoded_ns, Ordering::Release);
        if slot.start_cycle_plus_one.load(Ordering::Acquire) != 0 {
            self.latest_complete_dma_id
                .store(id.get(), Ordering::Release);
        }
    }

    fn invalidate(&self) {
        self.continuity_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .expect("audio presentation continuity generation exhausted");
    }

    fn current_generation(&self) -> u64 {
        self.continuity_generation.load(Ordering::Acquire)
    }

    fn state(&self, epoch: std::time::Instant) -> AudioPresentationState {
        let generation = self.continuity_generation.load(Ordering::Acquire);
        let raw_id = self.latest_complete_dma_id.load(Ordering::Acquire);
        if raw_id == 0 {
            return AudioPresentationState {
                continuity_generation: generation,
                anchor: None,
            };
        }
        let id = fn64_runtime::AiDmaId::new(raw_id);
        let slot = self.slot(id);
        if slot.dma_id.load(Ordering::Acquire) != id.get() {
            return AudioPresentationState {
                continuity_generation: generation,
                anchor: None,
            };
        }
        let slot_generation = slot.continuity_generation.load(Ordering::Acquire);
        if slot_generation == 0 || slot_generation != generation {
            return AudioPresentationState {
                continuity_generation: generation,
                anchor: None,
            };
        }
        let start = slot.start_cycle_plus_one.load(Ordering::Acquire);
        let playback = slot
            .predicted_playback_ns_plus_one
            .load(Ordering::Acquire);
        if start == 0
            || playback == 0
            || generation != self.continuity_generation.load(Ordering::Acquire)
        {
            return AudioPresentationState {
                continuity_generation: self.continuity_generation.load(Ordering::Acquire),
                anchor: None,
            };
        }
        AudioPresentationState {
            continuity_generation: generation,
            anchor: Some(AudioPresentationAnchor {
                dma_id: id,
                emulated_at: fn64_runtime::EmulatedInstant::new(start - 1),
                predicted_playback_at: epoch + std::time::Duration::from_nanos(playback - 1),
                continuity_generation: generation,
            }),
        }
    }

    #[cfg(test)]
    fn latest(&self, epoch: std::time::Instant) -> Option<AudioPresentationAnchor> {
        self.state(epoch).anchor
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QueuedStreamStartPayload {
    id: fn64_runtime::AiDmaId,
    queued_at: std::time::Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostStreamStartSource {
    UntrackedPayload,
    ActiveDma {
        payload: QueuedStreamStartPayload,
        started_at: fn64_runtime::EmulatedInstant,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HostStreamStartAuthorization(HostStreamStartSource);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HostStreamStartState {
    #[default]
    Waiting,
    Authorized(HostStreamStartSource),
    Playing(HostStreamStartSource),
}

#[derive(Debug, Default)]
struct HostStreamStartGate {
    state: HostStreamStartState,
    queued_before_start: [Option<QueuedStreamStartPayload>; 2],
    last_queued: Option<fn64_runtime::AiDmaId>,
    last_started: Option<fn64_runtime::AiDmaId>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
enum HostPcmDeliveryState {
    #[default]
    Inactive = 0,
    Active = 1,
}

#[derive(Debug, Default)]
struct HostPcmDeliveryGate {
    state: AtomicU8,
}

impl HostPcmDeliveryGate {
    fn activate(&self) {
        self.state
            .compare_exchange(
                HostPcmDeliveryState::Inactive as u8,
                HostPcmDeliveryState::Active as u8,
                Ordering::Release,
                Ordering::Acquire,
            )
            .unwrap_or_else(|state| {
                panic!("host PCM delivery gate activated from invalid state {state}")
            });
    }

    fn is_active(&self) -> bool {
        match self.state.load(Ordering::Acquire) {
            state if state == HostPcmDeliveryState::Inactive as u8 => false,
            state if state == HostPcmDeliveryState::Active as u8 => true,
            state => panic!("invalid host PCM delivery state {state}"),
        }
    }
}

impl HostStreamStartGate {
    fn queue_dma(
        &mut self,
        id: fn64_runtime::AiDmaId,
        queued_at: std::time::Instant,
    ) -> Option<HostStreamStartAuthorization> {
        assert!(
            self.last_queued.is_none_or(|prior| id > prior),
            "host audio payload notifications must have unique monotonic DMA identities"
        );
        self.last_queued = Some(id);
        match self.state {
            HostStreamStartState::Waiting => {
                let slot = self
                    .queued_before_start
                    .iter_mut()
                    .find(|slot| slot.is_none())
                    .expect("host audio startup cannot queue more than the hardware AI FIFO");
                *slot = Some(QueuedStreamStartPayload { id, queued_at });
                None
            }
            HostStreamStartState::Authorized(_) => {
                panic!("host audio payload arrived while stream start was being redeemed")
            }
            HostStreamStartState::Playing(_) => None,
        }
    }

    fn queue_untracked(&mut self) -> Option<HostStreamStartAuthorization> {
        match self.state {
            HostStreamStartState::Waiting => {
                assert!(
                    self.queued_before_start.iter().all(Option::is_none),
                    "untracked host audio cannot bypass queued hardware DMA startup"
                );
                Some(self.authorize(HostStreamStartSource::UntrackedPayload))
            }
            HostStreamStartState::Authorized(_) => {
                panic!("host audio payload arrived while stream start was being redeemed")
            }
            HostStreamStartState::Playing(_) => None,
        }
    }

    fn notify_dma_started(
        &mut self,
        start: fn64_runtime::AiDmaStart,
    ) -> Option<HostStreamStartAuthorization> {
        // Interleaving closed here: one or two payloads may be queued while AI
        // CONTROL is disabled, but neither admission may start the host stream.
        // Only the unique AiDmaStarted identity that matches a queued payload
        // creates play authority; later FIFO promotion cannot redeem it again.
        assert!(
            self.last_started.is_none_or(|prior| start.id > prior),
            "host audio start notifications must have unique monotonic DMA identities"
        );
        self.last_started = Some(start.id);
        match self.state {
            HostStreamStartState::Waiting => {
                let payload = self
                    .queued_before_start
                    .iter()
                    .flatten()
                    .find(|payload| payload.id == start.id)
                    .copied()
                    .unwrap_or_else(|| {
                        panic!(
                            "active AI DMA {} has no queued host payload",
                            start.id.get()
                        )
                    });
                Some(self.authorize(HostStreamStartSource::ActiveDma {
                    payload,
                    started_at: start.started_at,
                }))
            }
            HostStreamStartState::Authorized(_) => {
                panic!("AI DMA started while host stream start was being redeemed")
            }
            HostStreamStartState::Playing(_) => None,
        }
    }

    fn authorize(&mut self, source: HostStreamStartSource) -> HostStreamStartAuthorization {
        assert_eq!(self.state, HostStreamStartState::Waiting);
        self.state = HostStreamStartState::Authorized(source);
        HostStreamStartAuthorization(source)
    }

    fn redeem(
        &mut self,
        authorization: HostStreamStartAuthorization,
        play: impl FnOnce() -> Result<(), AudioError>,
    ) -> Result<HostStreamStartSource, AudioError> {
        assert_eq!(
            self.state,
            HostStreamStartState::Authorized(authorization.0),
            "host audio stream-start authority must be redeemed exactly once"
        );
        play()?;
        self.state = HostStreamStartState::Playing(authorization.0);
        Ok(authorization.0)
    }
}

/// A real `AudioBackend` backed by a live `cpal` output stream. Consumes
/// interleaved i16 PCM (see `AudioBackend::queue_samples`) into a shared
/// ring buffer; cpal's own audio callback (running on its own realtime
/// thread once the host stream is preactivated) drains that ring buffer into
/// the actual host output stream only after the first authoritative AI DMA
/// start, underrunning with silence rather than blocking if the ring runs dry.
pub struct CpalBackend {
    ring: SampleRing,
    channels: ChannelCount,
    stream: Option<Stream>,
    host_stream_played_at: Option<std::time::Instant>,
    pcm_delivery_gate: Arc<HostPcmDeliveryGate>,
    stream_start_gate: HostStreamStartGate,
    stream_start: Option<AudioStreamStartLandmark>,
    first_callback_ns_plus_one: Arc<AtomicU64>,
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
    /// Exact guest DAC rate used by producer-side resampling. The whole-Hz
    /// field remains for compatibility telemetry and sync-probe frequency
    /// analysis, never as the exact conversion authority.
    guest_sample_period: Option<AiSamplePeriod>,
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
    contention_sample_slots: Arc<AtomicU64>,
    dropped_sample_slots: u64,
    late_callbacks: Arc<AtomicU64>,
    max_callback_gap_us: Arc<AtomicU64>,
    output_dump: Option<PcmStreamDump>,
    output_dump_checked: bool,
    sync_probe: Option<AudioSyncProbeProducer>,
    sync_probe_shared: Arc<AudioSyncProbeShared>,
    sync_probe_epoch: std::time::Instant,
    presentation_shared: Arc<AudioPresentationShared>,
    pending_queue_dma_id: Option<fn64_runtime::AiDmaId>,
    host_execution_probe: Option<HostExecutionProbe>,
    buffer_probe: Option<AudioBufferProbe>,
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
            host_stream_played_at: None,
            pcm_delivery_gate: Arc::new(HostPcmDeliveryGate::default()),
            stream_start_gate: HostStreamStartGate::default(),
            stream_start: None,
            first_callback_ns_plus_one: Arc::new(AtomicU64::new(0)),
            stream_rate_hz: None,
            guest_rate_hz: None,
            guest_sample_period: None,
            resampler: BandlimitedResampler::new(),
            resample_output: Vec::new(),
            warned_overflow: false,
            callback_count: Arc::new(AtomicU64::new(0)),
            requested_sample_slots: Arc::new(AtomicU64::new(0)),
            underrun_sample_slots: Arc::new(AtomicU64::new(0)),
            contention_sample_slots: Arc::new(AtomicU64::new(0)),
            dropped_sample_slots: 0,
            late_callbacks: Arc::new(AtomicU64::new(0)),
            max_callback_gap_us: Arc::new(AtomicU64::new(0)),
            output_dump: None,
            output_dump_checked: false,
            sync_probe: None,
            sync_probe_shared: Arc::new(AudioSyncProbeShared::default()),
            sync_probe_epoch: std::time::Instant::now(),
            presentation_shared: Arc::new(AudioPresentationShared::new()),
            pending_queue_dma_id: None,
            host_execution_probe: None,
            buffer_probe: None,
        }
    }

    /// Attach host-only callback attribution before [`AudioBackend::create`].
    /// The caller may retain another clone to set phases and drain events.
    pub fn install_host_execution_probe(&mut self, probe: HostExecutionProbe) {
        assert!(
            self.stream.is_none(),
            "host execution probe must be installed before audio stream creation"
        );
        self.host_execution_probe = Some(probe);
    }

    /// Attach content-free queue/callback calibration before stream creation.
    pub fn install_buffer_probe(&mut self, probe: AudioBufferProbe) {
        assert!(
            self.stream.is_none(),
            "audio buffer probe must be installed before audio stream creation"
        );
        self.buffer_probe = Some(probe);
    }

    /// The negotiated host stream rate, once `create` has succeeded.
    pub fn stream_rate_hz(&self) -> Option<HostSampleRateHz> {
        self.stream.as_ref()?;
        self.stream_rate_hz
    }

    /// Start the host device stream while guest PCM delivery remains gated.
    ///
    /// A shell should call this after [`AudioBackend::create`] and before it
    /// establishes the emulation wall epoch. Until the first authoritative AI
    /// DMA start, realtime callbacks emit content-neutral silence without
    /// inspecting the PCM ring or advancing delivery health/continuity.
    pub fn preactivate_host_stream(&mut self) -> Result<(), AudioError> {
        if self.host_stream_played_at.is_some() {
            return Err(AudioError::Backend {
                backend: "cpal",
                reason: "host stream was preactivated more than once".to_owned(),
            });
        }
        let stream = self
            .stream
            .as_ref()
            .ok_or(AudioError::NotReady("create() not called"))?;
        stream.play().map_err(|error| AudioError::Backend {
            backend: "cpal",
            reason: format!("stream.play failed during host preactivation: {error}"),
        })?;
        self.host_stream_played_at = Some(std::time::Instant::now());
        Ok(())
    }

    fn redeem_stream_start(
        &mut self,
        authorization: Option<HostStreamStartAuthorization>,
    ) -> Result<(), AudioError> {
        let Some(authorization) = authorization else {
            return Ok(());
        };
        let stream = self
            .stream
            .as_ref()
            .expect("stream-start authority requires a created stream");
        let play_returned_at = match self.host_stream_played_at {
            Some(played_at) => played_at,
            None => {
                stream.play().map_err(|error| AudioError::Backend {
                    backend: "cpal",
                    reason: format!("stream.play failed for first active AI DMA: {error}"),
                })?;
                let played_at = std::time::Instant::now();
                self.host_stream_played_at = Some(played_at);
                played_at
            }
        };
        let source = authorization.0;
        if let HostStreamStartSource::ActiveDma {
            payload,
            started_at,
        } = source
        {
            let delivery_activated_at = std::time::Instant::now();
            self.stream_start = Some(AudioStreamStartLandmark {
                dma_id: payload.id,
                payload_queued_at: payload.queued_at,
                dma_started_at: started_at,
                play_returned_at,
                delivery_activated_at,
                first_callback_at: None,
            });
        }
        let pcm_delivery_gate = Arc::clone(&self.pcm_delivery_gate);
        self.stream_start_gate.redeem(authorization, move || {
            // Exact interleaving closed: the realtime callback may race the
            // first active-DMA notification after its PCM was queued. If its
            // Acquire observes Inactive it emits silence without draining;
            // if it observes this Release, every guest start/landmark write
            // above is visible before it can drain that DMA. The callback can
            // therefore neither consume PCM before AI start authority nor
            // publish playback against missing guest-start metadata.
            pcm_delivery_gate.activate();
            Ok(())
        })?;
        Ok(())
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
    in_period: Option<AiSamplePeriod>,
    out_hz: Option<HostSampleRateHz>,
    /// Fractional read position in input-frame coordinates. Keeping the phase
    /// as an integer rational prevents long-running chunk boundaries from
    /// accumulating floating-point step error.
    phase_numerator: u128,
    phase_denominator: u128,
    pending_landmarks: VecDeque<(LandmarkKind, fn64_runtime::AiDmaId, f64)>,
}

impl BandlimitedResampler {
    const RADIUS: isize = 16;

    fn new() -> Self {
        BandlimitedResampler {
            frames: Vec::new(),
            channels: None,
            in_period: None,
            out_hz: None,
            phase_numerator: 0,
            phase_denominator: 1,
            pending_landmarks: VecDeque::new(),
        }
    }

    /// Convert `input` (interleaved, `channels` per frame, produced at
    /// `in_hz`) to `out_hz`, appending to `out`. Equal rates pass through
    /// unchanged (byte-identical to the pre-resampler behavior).
    #[cfg(test)]
    fn process(
        &mut self,
        input: GuestPcm16<'_>,
        in_hz: GuestSampleRateHz,
        out_hz: HostSampleRateHz,
        out: &mut Vec<i16>,
    ) {
        let _ = self.process_tagged(
            input,
            AiSamplePeriod::new(in_hz.get(), 1),
            out_hz,
            OutputLandmarks::default(),
            out,
        );
    }

    fn process_tagged(
        &mut self,
        input: GuestPcm16<'_>,
        in_period: AiSamplePeriod,
        out_hz: HostSampleRateHz,
        landmarks: OutputLandmarks,
        out: &mut Vec<i16>,
    ) -> OutputLandmarks {
        let channels = input.channels();
        let channel_slots = channels.as_usize();
        if u128::from(in_period.video_clock_hz())
            == u128::from(in_period.dacrate_plus_one()) * u128::from(out_hz.get())
        {
            self.reset();
            let mut crossed = OutputLandmarks::default();
            if let Some((id, frame)) = landmarks.presentation {
                crossed.presentation = Some((id, out.len() + frame * channel_slots));
            }
            if let Some((id, frame)) = landmarks.sync {
                crossed.sync = Some((id, out.len() + frame * channel_slots));
            }
            out.extend_from_slice(input.samples());
            return crossed;
        }

        if self.channels != Some(channels)
            || self.in_period != Some(in_period)
            || self.out_hz != Some(out_hz)
        {
            self.reset();
            self.channels = Some(channels);
            self.in_period = Some(in_period);
            self.out_hz = Some(out_hz);
            self.phase_denominator =
                u128::from(in_period.dacrate_plus_one()) * u128::from(out_hz.get());
        }

        let in_frames = input.samples().len() / channel_slots;
        let buffered_frames = self.frames.len() / channel_slots;
        if let Some((id, frame)) = landmarks.presentation {
            self.pending_landmarks.push_back((
                LandmarkKind::Presentation,
                id,
                (buffered_frames + frame) as f64,
            ));
        }
        if let Some((id, frame)) = landmarks.sync {
            self.pending_landmarks.push_back((
                LandmarkKind::Sync,
                id,
                (buffered_frames + frame) as f64,
            ));
        }
        self.frames
            .extend_from_slice(&input.samples()[..in_frames * channel_slots]);
        let total_frames = self.frames.len() / channel_slots;
        if total_frames == 0 {
            return OutputLandmarks::default();
        }

        let step_numerator = u128::from(in_period.video_clock_hz());
        let mut crossed = OutputLandmarks::default();
        while self.phase_numerator
            + u128::try_from(Self::RADIUS).expect("positive resampler radius")
                * self.phase_denominator
            < total_frames as u128 * self.phase_denominator
        {
            let phase = self.phase_numerator as f64 / self.phase_denominator as f64;
            while self
                .pending_landmarks
                .front()
                .is_some_and(|entry| phase >= entry.2)
            {
                let (kind, id, _) = self
                    .pending_landmarks
                    .pop_front()
                    .expect("front was observed");
                crossed.record(kind, id, out.len());
            }
            for ch in 0..channel_slots {
                out.push(self.sample_at(phase, ch));
            }
            self.phase_numerator += step_numerator;
        }

        let phase_floor = self.phase_numerator / self.phase_denominator;
        let keep_from =
            usize::try_from(phase_floor.saturating_sub(u128::try_from(Self::RADIUS).unwrap()))
                .unwrap_or(usize::MAX);
        if keep_from > 0 {
            self.frames.drain(..keep_from * channel_slots);
            self.phase_numerator -= keep_from as u128 * self.phase_denominator;
            for (_, _, position) in &mut self.pending_landmarks {
                *position -= keep_from as f64;
            }
        }
        crossed
    }

    fn reset(&mut self) {
        self.frames.clear();
        self.channels = None;
        self.in_period = None;
        self.out_hz = None;
        self.phase_numerator = 0;
        self.phase_denominator = 1;
        self.pending_landmarks.clear();
    }

    fn output_samples_hint(
        input: GuestPcm16<'_>,
        in_hz: GuestSampleRateHz,
        out_hz: HostSampleRateHz,
    ) -> usize {
        Self::output_samples_hint_for_period(input, AiSamplePeriod::new(in_hz.get(), 1), out_hz)
    }

    fn output_samples_hint_for_period(
        input: GuestPcm16<'_>,
        in_period: AiSamplePeriod,
        out_hz: HostSampleRateHz,
    ) -> usize {
        if u128::from(in_period.video_clock_hz())
            == u128::from(in_period.dacrate_plus_one()) * u128::from(out_hz.get())
        {
            return input.samples().len();
        }
        let channels = input.channels().as_usize();
        let input_frames = input.samples().len() / channels;
        let output_frames =
            resampled_output_frames_ceil(input_frames as u128, in_period, out_hz).saturating_add(1);
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

fn resampled_output_frames_ceil(
    input_frames: u128,
    in_period: AiSamplePeriod,
    out_hz: HostSampleRateHz,
) -> u128 {
    input_frames
        .saturating_mul(u128::from(in_period.dacrate_plus_one()))
        .saturating_mul(u128::from(out_hz.get()))
        .div_ceil(u128::from(in_period.video_clock_hz()))
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
        self.sync_probe = AudioSyncProbeProducer::from_env();
        self.sync_probe_shared = Arc::new(AudioSyncProbeShared::default());
        self.sync_probe_epoch = std::time::Instant::now();
        self.presentation_shared = Arc::new(AudioPresentationShared::new());
        self.host_stream_played_at = None;
        // A retiring stream may still own its callback Arc until replacement
        // at the end of create. Its gate must not be able to observe the new
        // stream's first-DMA activation.
        self.pcm_delivery_gate = Arc::new(HostPcmDeliveryGate::default());
        self.stream_start_gate = HostStreamStartGate::default();
        self.stream_start = None;
        // A prior stream may still own its callback Arc until replacement at
        // the end of create. A new allocation prevents that retiring callback
        // from publishing itself as the recreated stream's first callback.
        self.first_callback_ns_plus_one = Arc::new(AtomicU64::new(0));
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
            let pcm_delivery_gate = Arc::clone(&self.pcm_delivery_gate);
            let callback_count = Arc::clone(&self.callback_count);
            let requested_sample_slots = Arc::clone(&self.requested_sample_slots);
            let underrun_sample_slots = Arc::clone(&self.underrun_sample_slots);
            let contention_sample_slots = Arc::clone(&self.contention_sample_slots);
            let late_callbacks = Arc::clone(&self.late_callbacks);
            let max_callback_gap_us = Arc::clone(&self.max_callback_gap_us);
            let sync_probe_shared = Arc::clone(&self.sync_probe_shared);
            let sync_probe_epoch = self.sync_probe_epoch;
            let first_callback_ns_plus_one = Arc::clone(&self.first_callback_ns_plus_one);
            let presentation_shared = Arc::clone(&self.presentation_shared);
            let error_presentation_shared = Arc::clone(&self.presentation_shared);
            let host_execution_probe = self.host_execution_probe.clone();
            let buffer_probe = self.buffer_probe.clone();
            let mut last_pull = None;
            device
                .build_output_stream(
                    stream_config,
                    move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                        let now = std::time::Instant::now();
                        if let Some(probe) = buffer_probe.as_ref() {
                            probe.record_callback_geometry(now, data.len());
                        }
                        if !admit_host_pcm_delivery(&pcm_delivery_gate, data) {
                            return;
                        }
                        let host_phase = host_execution_probe
                            .as_ref()
                            .map_or(HostExecutionPhase::Waiting, HostExecutionProbe::phase);
                        let encoded_callback_ns =
                            u64::try_from(now.duration_since(sync_probe_epoch).as_nanos())
                                .unwrap_or(u64::MAX - 1)
                                .saturating_add(1);
                        let _ = first_callback_ns_plus_one.compare_exchange(
                            0,
                            encoded_callback_ns,
                            Ordering::Release,
                            Ordering::Relaxed,
                        );
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
                        let outcome = try_drain_ring_into_f32(&callback_ring, data);
                        if let Some(probe) = host_execution_probe.as_ref() {
                            probe.record_underrun(now, outcome, host_phase);
                        }
                        let predicted_for = |sample_slot: usize| {
                            let callback_to_playback =
                                info.timestamp().playback - info.timestamp().callback;
                            let frame_offset = sample_slot / usize::from(stream_config.channels);
                            let intra_callback = std::time::Duration::from_secs_f64(
                                frame_offset as f64 / f64::from(stream_config.sample_rate),
                            );
                            now + callback_to_playback + intra_callback
                        };
                        if let Some((dma_id, sample_slot)) = outcome.landmarks.presentation {
                            let predicted = predicted_for(sample_slot);
                            let encoded_ns = u64::try_from(
                                predicted.duration_since(sync_probe_epoch).as_nanos(),
                            )
                            .unwrap_or(u64::MAX - 1)
                            .saturating_add(1);
                            presentation_shared.record_playback(dma_id, encoded_ns);
                        }
                        if let Some((dma_id, sample_slot)) = outcome.landmarks.sync {
                            let candidate = sync_probe_shared.dma_id.load(Ordering::Acquire);
                            if candidate == dma_id.get() {
                                let predicted = predicted_for(sample_slot);
                                let ns = u64::try_from(
                                    predicted.duration_since(sync_probe_epoch).as_nanos(),
                                )
                                .unwrap_or(u64::MAX - 1)
                                .saturating_add(1);
                                // Publish the generation before the playback
                                // marker's Release. The emulation thread reads
                                // the marker with Acquire before accepting this
                                // generation, while any concurrent invalidation
                                // either precedes this sample or makes the
                                // later live-generation comparison reject it.
                                sync_probe_shared.continuity_generation.store(
                                    presentation_shared.current_generation(),
                                    Ordering::Relaxed,
                                );
                                sync_probe_shared
                                    .predicted_playback_ns_plus_one
                                    .store(ns, Ordering::Release);
                            }
                        }
                        if outcome.underrun_sample_slots != 0
                            || outcome.contention_sample_slots != 0
                        {
                            presentation_shared.invalidate();
                        }
                        underrun_sample_slots
                            .fetch_add(outcome.underrun_sample_slots as u64, Ordering::Relaxed);
                        contention_sample_slots
                            .fetch_add(outcome.contention_sample_slots as u64, Ordering::Relaxed);
                    },
                    move |err| {
                        // cpal stream error callback: no runtime state to
                        // report it into (matches `RenderError`'s stance that
                        // a backend failure is surfaced for a caller to poll,
                        // not force-propagated); logged so it isn't silent.
                        error_presentation_shared.invalidate();
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
        let (stream, ring, stream_rate_hz) =
            match build(requested_host_rate) {
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
        self.guest_sample_period = Some(AiSamplePeriod::new(cfg.sample_rate_hz.get(), 1));
        self.resampler = BandlimitedResampler::new();
        self.output_dump = None;
        self.output_dump_checked = false;
        self.stream = Some(stream);
        Ok(())
    }

    fn queue_samples(&mut self, pcm: GuestPcm16<'_>) -> Result<(), AudioError> {
        let dma_id = self.pending_queue_dma_id.take();
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
        let guest_sample_period = self
            .guest_sample_period
            .expect("created stream must retain its exact guest sample period");
        let stream_rate_hz = self
            .stream_rate_hz
            .expect("created stream must retain its host sample rate");
        let reserve = BandlimitedResampler::output_samples_hint_for_period(
            pcm,
            guest_sample_period,
            stream_rate_hz,
        );
        self.resample_output.clear();
        if self.resample_output.capacity() < reserve {
            self.resample_output.reserve(reserve);
        }
        let guest_landmark = dma_id.and_then(|id| {
            self.sync_probe
                .as_mut()
                .and_then(|probe| probe.inspect(pcm, guest_rate_hz))
                .map(|frame_offset| (id, frame_offset))
        });
        if let Some((id, frame_offset)) = guest_landmark {
            self.sync_probe_shared
                .guest_frame_offset
                .store(frame_offset as u64, Ordering::Relaxed);
            self.sync_probe_shared.dma_id.store(id.get(), Ordering::Release);
        }
        let output_landmarks = self.resampler.process_tagged(
            pcm,
            guest_sample_period,
            stream_rate_hz,
            OutputLandmarks {
                presentation: dma_id.map(|id| (id, 0)),
                sync: guest_landmark,
            },
            &mut self.resample_output,
        );
        if !self.output_dump_checked {
            self.output_dump_checked = true;
            self.output_dump = PcmStreamDump::maybe_create_from_env(stream_rate_hz, self.channels);
        }
        if let Some(dump) = self.output_dump.as_mut() {
            dump.write_samples(&self.resample_output);
        }
        let resampled_sample_slots = self.resample_output.len();
        let (start_authorization, dropped, ring_cap, queued_at, ring_sample_slots_after) = {
            let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
            let push = ring.push_dma(&self.resample_output, pcm.dma_bytes(), output_landmarks);
            let queued_at = std::time::Instant::now();
            let ring_sample_slots_after = ring.samples.len();
            let authorization = match dma_id {
                Some(id) => self
                    .stream_start_gate
                    .queue_dma(id, queued_at),
                None => self.stream_start_gate.queue_untracked(),
            };
            (
                authorization,
                push,
                ring.sample_cap,
                queued_at,
                ring_sample_slots_after,
            )
        };
        if let (Some(probe), Some(id)) = (self.buffer_probe.as_ref(), dma_id) {
            probe.record_dma_queued(
                id,
                queued_at,
                resampled_sample_slots,
                ring_sample_slots_after,
                stream_rate_hz,
                self.channels,
            );
        }
        if dropped.dropped_sync_landmark.is_some() {
            self.sync_probe_shared.dropped.store(1, Ordering::Release);
        }
        if dropped.dropped_presentation {
            self.presentation_shared.invalidate();
        }
        let dropped = dropped.dropped_sample_slots;
        if dropped > 0 && !self.warned_overflow {
            self.warned_overflow = true;
            eprintln!(
                "fn64-audio: output ring exceeded {ring_cap} samples; dropped {dropped} oldest \
                 (producer outrunning the drain -- reported once)"
            );
        }
        self.dropped_sample_slots = self
            .dropped_sample_slots
            .saturating_add(u64::try_from(dropped).unwrap_or(u64::MAX));
        self.redeem_stream_start(start_authorization)
    }

    fn queue_dma(
        &mut self,
        id: fn64_runtime::AiDmaId,
        pcm: GuestPcm16<'_>,
    ) -> Result<(), AudioError> {
        self.presentation_shared.queue(id);
        assert!(self.pending_queue_dma_id.replace(id).is_none());
        self.queue_samples(pcm)
    }

    fn notify_dma_started(&mut self, start: fn64_runtime::AiDmaStart) -> Result<(), AudioError> {
        let start_authorization = self.stream_start_gate.notify_dma_started(start);
        self.presentation_shared.record_start(start);
        if self.sync_probe_shared.dma_id.load(Ordering::Acquire) == start.id.get() {
            self.sync_probe_shared
                .start_dacrate
                .store(u64::from(start.dacrate), Ordering::Relaxed);
            self.sync_probe_shared
                .start_cycle_plus_one
                .store(start.started_at.get().saturating_add(1), Ordering::Release);
        }
        self.redeem_stream_start(start_authorization)
    }

    fn notify_dma_retimed(&mut self, id: fn64_runtime::AiDmaId) {
        self.presentation_shared.invalidate();
        if self.sync_probe_shared.dma_id.load(Ordering::Acquire) == id.get() {
            self.sync_probe_shared.retimed.store(1, Ordering::Release);
        }
    }

    fn sync_landmark(&self) -> Option<AudioSyncLandmark> {
        let raw_id = self.sync_probe_shared.dma_id.load(Ordering::Acquire);
        if raw_id == 0 {
            return None;
        }
        let start_cycle = self
            .sync_probe_shared
            .start_cycle_plus_one
            .load(Ordering::Acquire);
        let playback_ns = self
            .sync_probe_shared
            .predicted_playback_ns_plus_one
            .load(Ordering::Acquire);
        let continuity_generation = self
            .sync_probe_shared
            .continuity_generation
            .load(Ordering::Relaxed);
        Some(AudioSyncLandmark {
            dma_id: fn64_runtime::AiDmaId::new(raw_id),
            guest_frame_offset: self
                .sync_probe_shared
                .guest_frame_offset
                .load(Ordering::Relaxed),
            dma_started_at: (start_cycle != 0)
                .then(|| fn64_runtime::Cycles::new(start_cycle - 1)),
            start_dacrate: (start_cycle != 0).then(|| {
                u32::try_from(self.sync_probe_shared.start_dacrate.load(Ordering::Relaxed))
                    .unwrap_or(u32::MAX)
            }),
            predicted_playback_at: (playback_ns != 0).then(|| {
                self.sync_probe_epoch + std::time::Duration::from_nanos(playback_ns - 1)
            }),
            continuity_generation: (playback_ns != 0).then_some(continuity_generation),
            dropped_before_playback: self.sync_probe_shared.dropped.load(Ordering::Acquire) != 0,
            retimed_after_start: self.sync_probe_shared.retimed.load(Ordering::Acquire) != 0,
        })
    }

    fn presentation_state(&self) -> Option<AudioPresentationState> {
        Some(self.presentation_shared.state(self.sync_probe_epoch))
    }

    fn stream_start_landmark(&self) -> Option<AudioStreamStartLandmark> {
        let mut landmark = self.stream_start?;
        let encoded_callback_ns = self.first_callback_ns_plus_one.load(Ordering::Acquire);
        landmark.first_callback_at = (encoded_callback_ns != 0).then(|| {
            self.sync_probe_epoch
                + std::time::Duration::from_nanos(encoded_callback_ns - 1)
        });
        Some(landmark)
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
        self.guest_sample_period = Some(AiSamplePeriod::new(sample_rate_hz.get(), 1));
    }

    fn set_sample_period(&mut self, period: AiSamplePeriod) {
        self.guest_rate_hz = Some(GuestSampleRateHz::new(period.floor_hz()));
        self.guest_sample_period = Some(period);
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
            contention_sample_slots: HostSampleSlotCount::new(
                self.contention_sample_slots.load(Ordering::Relaxed),
            ),
            dropped_sample_slots: HostSampleSlotCount::new(self.dropped_sample_slots),
            late_callbacks: self.late_callbacks.load(Ordering::Relaxed),
            max_callback_gap_us: self.max_callback_gap_us.load(Ordering::Relaxed),
        })
    }

    fn set_host_execution_phase(&self, phase: HostExecutionPhase) -> Option<HostExecutionPhase> {
        self.host_execution_probe
            .as_ref()
            .map(|probe| probe.set_phase(phase))
    }

    fn host_execution_probe_enabled(&self) -> bool {
        self.host_execution_probe.is_some()
    }

    fn drain_underrun_observations(&self, output: &mut Vec<AudioUnderrunObservation>) -> u64 {
        self.host_execution_probe
            .as_ref()
            .map_or(0, |probe| probe.drain_underrun_observations(output))
    }

    fn drain_buffer_observations(&self, output: &mut Vec<AudioBufferObservation>) -> u64 {
        self.buffer_probe.as_ref().map_or(0, |probe| probe.drain(output))
    }

    fn buffer_probe(&self) -> Option<AudioBufferProbe> {
        self.buffer_probe.clone()
    }

    fn host_execution_probe(&self) -> Option<HostExecutionProbe> {
        self.host_execution_probe.clone()
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

    fn dma_start(id: fn64_runtime::AiDmaId, cycle: u64) -> fn64_runtime::AiDmaStart {
        fn64_runtime::AiDmaStart {
            id,
            request: fn64_runtime::AiDmaRequest {
                dram_addr: fn64_runtime::RdramAddr::from_offset(0x1000),
                len: 4,
                sample_rate_hz: 32_000,
            },
            started_at: fn64_runtime::EmulatedInstant::new(cycle),
            dacrate: 1_519,
        }
    }

    fn dma_bytes(bytes: u32) -> GuestDmaByteCount {
        GuestDmaByteCount::new(bytes)
    }

    #[test]
    fn presentation_anchor_is_complete_in_both_start_callback_orders() {
        let epoch = std::time::Instant::now();
        for callback_first in [false, true] {
            let shared = AudioPresentationShared::new();
            let id = fn64_runtime::AiDmaId::new(7);
            shared.queue(id);
            if callback_first {
                shared.record_playback(id, 501);
                assert_eq!(shared.latest(epoch), None);
                shared.record_start(dma_start(id, 100));
            } else {
                shared.record_start(dma_start(id, 100));
                assert_eq!(shared.latest(epoch), None);
                shared.record_playback(id, 501);
            }
            assert_eq!(
                shared.latest(epoch),
                Some(AudioPresentationAnchor {
                    dma_id: id,
                    emulated_at: fn64_runtime::EmulatedInstant::new(100),
                    predicted_playback_at: epoch + std::time::Duration::from_nanos(500),
                    continuity_generation: 1,
                })
            );
        }
    }

    #[test]
    fn host_stream_starts_on_the_first_active_dma_not_a_dormant_second_payload() {
        let now = std::time::Instant::now();
        let first = fn64_runtime::AiDmaId::new(1);
        let second = fn64_runtime::AiDmaId::new(2);
        let mut gate = HostStreamStartGate::default();
        assert_eq!(gate.queue_dma(first, now), None);
        assert_eq!(
            gate.queue_dma(second, now + std::time::Duration::from_nanos(1)),
            None,
            "two queued-but-dormant FIFO payloads are not play authority"
        );
        let authorization = gate
            .notify_dma_started(dma_start(first, 100))
            .expect("the first active DMA authorizes playback");
        assert_eq!(
            gate.redeem(authorization, || Ok(())).unwrap(),
            HostStreamStartSource::ActiveDma {
                payload: QueuedStreamStartPayload {
                    id: first,
                    queued_at: now,
                },
                started_at: fn64_runtime::EmulatedInstant::new(100),
            }
        );
        assert_eq!(gate.notify_dma_started(dma_start(second, 200)), None);
    }

    #[test]
    fn preactivated_callback_is_content_neutral_until_first_dma_authority() {
        let gate = HostPcmDeliveryGate::default();
        let mut output_ring = OutputRing::with_capacity(8);
        output_ring.push_dma(&[10, 20, 30, 40], dma_bytes(8), OutputLandmarks::default());
        let ring = Arc::new(Mutex::new(output_ring));
        let mut output = [99.0; 4];

        assert!(!admit_host_pcm_delivery(&gate, &mut output));
        assert_eq!(output, [0.0; 4]);
        assert_eq!(
            ring.lock().unwrap().samples.len(),
            4,
            "inactive callbacks must not inspect or drain queued guest PCM"
        );

        gate.activate();
        assert!(admit_host_pcm_delivery(&gate, &mut output));
        let outcome = try_drain_ring_into_f32(&ring, &mut output);
        assert_eq!(outcome.delivered_sample_slots, 4);
        assert_eq!(outcome.underrun_sample_slots, 0);
        assert_eq!(
            output,
            [
                10.0 / 32768.0,
                20.0 / 32768.0,
                30.0 / 32768.0,
                40.0 / 32768.0
            ]
        );
    }

    #[test]
    fn delivery_activation_release_publishes_first_dma_metadata() {
        for raw_id in 1..=64 {
            let gate = Arc::new(HostPcmDeliveryGate::default());
            let published_dma_id = Arc::new(AtomicU64::new(0));
            let callback_gate = Arc::clone(&gate);
            let callback_dma_id = Arc::clone(&published_dma_id);
            let callback = std::thread::spawn(move || {
                while !callback_gate.is_active() {
                    std::thread::yield_now();
                }
                callback_dma_id.load(Ordering::Relaxed)
            });

            published_dma_id.store(raw_id, Ordering::Relaxed);
            gate.activate();
            assert_eq!(callback.join().unwrap(), raw_id);
        }
    }

    #[test]
    #[should_panic(expected = "activated from invalid state")]
    fn delivery_activation_cannot_be_redeemed_twice() {
        let gate = HostPcmDeliveryGate::default();
        gate.activate();
        gate.activate();
    }

    #[test]
    fn host_stream_start_authority_is_not_consumed_when_play_fails() {
        let id = fn64_runtime::AiDmaId::new(1);
        let mut gate = HostStreamStartGate::default();
        gate.queue_dma(id, std::time::Instant::now());
        let authorization = gate.notify_dma_started(dma_start(id, 100)).unwrap();
        let error = gate
            .redeem(authorization, || {
                Err(AudioError::Backend {
                    backend: "test",
                    reason: "play failed".to_owned(),
                })
            })
            .unwrap_err();
        assert!(error.to_string().contains("play failed"));
        assert!(matches!(gate.state, HostStreamStartState::Authorized(_)));
    }

    #[test]
    #[should_panic(expected = "unique monotonic DMA identities")]
    fn duplicate_host_stream_start_notification_is_a_loud_invariant_failure() {
        let id = fn64_runtime::AiDmaId::new(1);
        let mut gate = HostStreamStartGate::default();
        gate.queue_dma(id, std::time::Instant::now());
        let authorization = gate.notify_dma_started(dma_start(id, 100)).unwrap();
        gate.redeem(authorization, || Ok(())).unwrap();
        gate.notify_dma_started(dma_start(id, 100));
    }

    #[test]
    #[should_panic(expected = "has no queued host payload")]
    fn active_dma_without_a_host_payload_is_a_loud_invariant_failure() {
        let mut gate = HostStreamStartGate::default();
        gate.notify_dma_started(dma_start(fn64_runtime::AiDmaId::new(1), 100));
    }

    #[test]
    fn recreated_host_stream_requires_fresh_payload_and_active_dma_authority() {
        let id = fn64_runtime::AiDmaId::new(1);
        let mut gate = HostStreamStartGate::default();
        gate.queue_dma(id, std::time::Instant::now());
        let authorization = gate.notify_dma_started(dma_start(id, 100)).unwrap();
        gate.redeem(authorization, || Ok(())).unwrap();

        gate = HostStreamStartGate::default();
        assert_eq!(gate.state, HostStreamStartState::Waiting);
        assert_eq!(gate.queue_dma(id, std::time::Instant::now()), None);
    }

    #[test]
    fn concurrent_start_and_callback_publish_only_complete_anchors() {
        for raw_id in 1..=64 {
            let shared = Arc::new(AudioPresentationShared::new());
            let id = fn64_runtime::AiDmaId::new(raw_id);
            shared.queue(id);
            let barrier = Arc::new(std::sync::Barrier::new(3));
            let start_shared = Arc::clone(&shared);
            let start_barrier = Arc::clone(&barrier);
            let start_thread = std::thread::spawn(move || {
                start_barrier.wait();
                start_shared.record_start(dma_start(id, 100 + raw_id));
            });
            let callback_shared = Arc::clone(&shared);
            let callback_barrier = Arc::clone(&barrier);
            let callback_thread = std::thread::spawn(move || {
                callback_barrier.wait();
                callback_shared.record_playback(id, 501 + raw_id);
            });
            barrier.wait();
            start_thread.join().unwrap();
            callback_thread.join().unwrap();

            let anchor = shared
                .latest(std::time::Instant::now())
                .expect("both publishers completed");
            assert_eq!(anchor.dma_id, id);
            assert_eq!(anchor.emulated_at.get(), 100 + raw_id);
        }
    }

    #[test]
    fn presentation_discontinuity_rejects_stale_anchor_until_a_new_dma_crosses() {
        let shared = AudioPresentationShared::new();
        let epoch = std::time::Instant::now();
        let first = fn64_runtime::AiDmaId::new(1);
        shared.queue(first);
        shared.record_start(dma_start(first, 100));
        shared.record_playback(first, 501);
        assert!(shared.latest(epoch).is_some());

        shared.invalidate();
        assert_eq!(shared.latest(epoch), None);
        assert_eq!(
            shared.state(epoch),
            AudioPresentationState {
                continuity_generation: 2,
                anchor: None,
            },
            "invalidation must remain observable while no replacement anchor is complete"
        );

        let second = fn64_runtime::AiDmaId::new(2);
        shared.queue(second);
        shared.record_start(dma_start(second, 200));
        shared.record_playback(second, 901);
        assert_eq!(
            shared.latest(epoch)
                .expect("new marker restores the mapping")
                .continuity_generation,
            2
        );
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
        let err = backend.preactivate_host_stream().unwrap_err();
        assert!(matches!(err, AudioError::NotReady(_)));
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
    fn exact_ai_period_bounds_six_hour_host_frame_rounding_below_one_frame() {
        let period = AiSamplePeriod::new(48_681_812, 1_520);
        let output_rate = host_rate(48_000);
        let input_frames = u128::from(period.floor_hz()) * 6 * 60 * 60;
        let exact = resampled_output_frames_ceil(input_frames, period, output_rate);
        let exact_numerator =
            input_frames * u128::from(period.dacrate_plus_one()) * u128::from(output_rate.get());
        let rounding_remainder = exact * u128::from(period.video_clock_hz()) - exact_numerator;
        assert!(rounding_remainder < u128::from(period.video_clock_hz()));

        let truncated = resampled_output_frames_ceil(
            input_frames,
            AiSamplePeriod::new(period.floor_hz(), 1),
            output_rate,
        );
        assert!(
            exact.abs_diff(truncated) > 10_000,
            "whole-Hz conversion must not masquerade as a bounded long-horizon error"
        );
    }

    #[test]
    fn chunked_resampler_uses_exact_ai_period_instead_of_floor_hz() {
        let period = AiSamplePeriod::new(48_681_812, 1_520);
        let output_rate = host_rate(48_000);
        let input_frames = 500_000usize;
        let input = vec![0; input_frames * ChannelCount::STEREO.as_usize()];
        let mut output = Vec::new();
        let mut resampler = BandlimitedResampler::new();
        for chunk in input.chunks(2_000) {
            resampler.process_tagged(
                stereo(chunk),
                period,
                output_rate,
                OutputLandmarks::default(),
                &mut output,
            );
        }

        let produced = (output.len() / ChannelCount::STEREO.as_usize()) as u128;
        let exact = resampled_output_frames_ceil(input_frames as u128, period, output_rate);
        let truncated = resampled_output_frames_ceil(
            input_frames as u128,
            AiSamplePeriod::new(period.floor_hz(), 1),
            output_rate,
        );
        assert!(
            produced.abs_diff(exact) <= 32,
            "sinc tail is the only bound"
        );
        assert!(produced.abs_diff(exact) < produced.abs_diff(truncated));
    }

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
        assert_eq!(
            ring.push_dma(&input, dma_bytes(200), OutputLandmarks::default())
                .dropped_sample_slots,
            60
        );
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
        output_ring.push_dma(&[10, 20, 30], dma_bytes(6), OutputLandmarks::default());
        let ring = Arc::new(Mutex::new(output_ring));
        let producer_guard = ring.lock().unwrap();
        let mut output = [99.0; 5];

        assert_eq!(
            try_drain_ring_into_f32(&ring, &mut output),
            DrainOutcome {
                requested_sample_slots: 5,
                delivered_sample_slots: 0,
                underrun_sample_slots: 5,
                contention_sample_slots: 5,
                ring_sample_slots_before: None,
                underrun_reason: Some(AudioUnderrunReason::ProducerContention),
                landmarks: OutputLandmarks::default(),
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
    fn realtime_callback_distinguishes_empty_short_and_complete_ring_pulls() {
        let empty = Arc::new(Mutex::new(OutputRing::with_capacity(8)));
        let mut empty_output = [99.0; 5];
        let empty_outcome = try_drain_ring_into_f32(&empty, &mut empty_output);
        assert_eq!(empty_outcome.requested_sample_slots, 5);
        assert_eq!(empty_outcome.delivered_sample_slots, 0);
        assert_eq!(empty_outcome.underrun_sample_slots, 5);
        assert_eq!(empty_outcome.contention_sample_slots, 0);
        assert_eq!(empty_outcome.ring_sample_slots_before, Some(0));
        assert_eq!(
            empty_outcome.underrun_reason,
            Some(AudioUnderrunReason::RingEmpty)
        );

        let mut short_ring = OutputRing::with_capacity(8);
        short_ring.push_dma(&[10, 20, 30], dma_bytes(6), OutputLandmarks::default());
        let short = Arc::new(Mutex::new(short_ring));
        let mut short_output = [99.0; 5];
        let short_outcome = try_drain_ring_into_f32(&short, &mut short_output);
        assert_eq!(short_outcome.requested_sample_slots, 5);
        assert_eq!(short_outcome.delivered_sample_slots, 3);
        assert_eq!(short_outcome.underrun_sample_slots, 2);
        assert_eq!(short_outcome.ring_sample_slots_before, Some(3));
        assert_eq!(
            short_outcome.underrun_reason,
            Some(AudioUnderrunReason::RingShort)
        );

        let mut complete_ring = OutputRing::with_capacity(8);
        complete_ring.push_dma(&[1, 2, 3, 4, 5], dma_bytes(10), OutputLandmarks::default());
        let complete = Arc::new(Mutex::new(complete_ring));
        let mut complete_output = [99.0; 5];
        let complete_outcome = try_drain_ring_into_f32(&complete, &mut complete_output);
        assert_eq!(complete_outcome.delivered_sample_slots, 5);
        assert_eq!(complete_outcome.underrun_sample_slots, 0);
        assert_eq!(complete_outcome.ring_sample_slots_before, Some(5));
        assert_eq!(complete_outcome.underrun_reason, None);
    }

    #[test]
    fn audio_buffer_probe_retains_exact_queue_and_changed_callback_geometry() {
        let probe = AudioBufferProbe::new();
        let queued_at = std::time::Instant::now();
        let callback_at = queued_at + std::time::Duration::from_nanos(1);
        let id = fn64_runtime::AiDmaId::new(7);
        probe.record_dma_queued(
            id,
            queued_at,
            1_536,
            2_048,
            HostSampleRateHz::new(48_000),
            ChannelCount::STEREO,
        );
        probe.record_callback_geometry(callback_at, 1_024);
        probe.record_callback_geometry(callback_at + std::time::Duration::from_nanos(1), 1_024);
        probe.record_callback_geometry(callback_at + std::time::Duration::from_nanos(2), 512);

        let mut observations = Vec::new();
        assert_eq!(probe.drain(&mut observations), 0);
        assert_eq!(
            observations,
            vec![
                AudioBufferObservation::DmaQueued {
                    sequence: 1,
                    dma_id: id,
                    queued_at,
                    resampled_sample_slots: HostSampleSlotCount::new(1_536),
                    ring_sample_slots_after: HostSampleSlotCount::new(2_048),
                    host_sample_rate_hz: HostSampleRateHz::new(48_000),
                    channels: ChannelCount::STEREO,
                },
                AudioBufferObservation::CallbackGeometry {
                    sequence: 2,
                    callback_at,
                    requested_sample_slots: HostSampleSlotCount::new(1_024),
                },
                AudioBufferObservation::CallbackGeometry {
                    sequence: 3,
                    callback_at: callback_at + std::time::Duration::from_nanos(2),
                    requested_sample_slots: HostSampleSlotCount::new(512),
                },
            ]
        );
    }

    #[test]
    fn audio_buffer_probe_bounds_storage_and_reports_queue_loss() {
        let probe = AudioBufferProbe::new();
        let allocated = probe.0.observations.lock().unwrap().capacity();
        let queued_at = std::time::Instant::now();
        for raw_id in 1..=BUFFER_OBSERVATION_CAPACITY as u64 + 3 {
            probe.record_dma_queued(
                fn64_runtime::AiDmaId::new(raw_id),
                queued_at,
                raw_id as usize,
                raw_id as usize,
                HostSampleRateHz::new(48_000),
                ChannelCount::STEREO,
            );
        }
        assert_eq!(probe.0.observations.lock().unwrap().capacity(), allocated);
        let mut observations = Vec::new();
        assert_eq!(probe.drain(&mut observations), 3);
        assert_eq!(observations.len(), BUFFER_OBSERVATION_CAPACITY);
        assert_eq!(probe.drain(&mut observations), 0);
    }

    #[test]
    fn audio_buffer_callback_reports_loss_instead_of_waiting_for_consumer() {
        let probe = AudioBufferProbe::new();
        let consumer = probe.0.observations.lock().unwrap();
        probe.record_callback_geometry(std::time::Instant::now(), 1_024);
        assert!(consumer.is_empty());
        drop(consumer);

        let mut observations = Vec::new();
        assert_eq!(probe.drain(&mut observations), 1);
        assert!(observations.is_empty());
    }

    #[test]
    fn concurrent_audio_buffer_publishers_sequence_in_retained_queue_order() {
        let probe = AudioBufferProbe::new();
        let producer_probe = probe.clone();
        let callback_probe = probe.clone();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let producer_barrier = Arc::clone(&barrier);
        let producer = std::thread::spawn(move || {
            producer_barrier.wait();
            for raw_id in 1..=100 {
                producer_probe.record_dma_queued(
                    fn64_runtime::AiDmaId::new(raw_id),
                    std::time::Instant::now(),
                    1_536,
                    2_048,
                    HostSampleRateHz::new(48_000),
                    ChannelCount::STEREO,
                );
            }
        });
        let callback_barrier = Arc::clone(&barrier);
        let callback = std::thread::spawn(move || {
            callback_barrier.wait();
            for index in 0..100 {
                callback_probe.record_callback_geometry(
                    std::time::Instant::now(),
                    512 + index % 2,
                );
            }
        });
        barrier.wait();
        producer.join().unwrap();
        callback.join().unwrap();

        let mut observations = Vec::new();
        let _lost = probe.drain(&mut observations);
        assert!(observations.len() >= 100);
        for (index, observation) in observations.iter().enumerate() {
            let sequence = match observation {
                AudioBufferObservation::DmaQueued { sequence, .. }
                | AudioBufferObservation::CallbackGeometry { sequence, .. } => *sequence,
            };
            assert_eq!(sequence, index as u64 + 1);
        }
    }

    #[test]
    fn host_execution_probe_retains_phase_timestamp_depth_and_reason() {
        let probe = HostExecutionProbe::new();
        assert_eq!(probe.phase(), HostExecutionPhase::Waiting);
        assert_eq!(
            probe.set_phase(HostExecutionPhase::ViScanout),
            HostExecutionPhase::Waiting
        );
        let callback_at = std::time::Instant::now();
        probe.record_underrun(
            callback_at,
            DrainOutcome::inspected(8, 3, 5, OutputLandmarks::default()),
            probe.phase(),
        );
        let mut observations = Vec::new();
        assert_eq!(probe.drain_underrun_observations(&mut observations), 0);
        assert_eq!(
            observations,
            vec![AudioUnderrunObservation {
                sequence: 1,
                callback_at,
                reason: AudioUnderrunReason::RingShort,
                requested_sample_slots: HostSampleSlotCount::new(8),
                delivered_sample_slots: HostSampleSlotCount::new(5),
                ring_sample_slots_before: Some(HostSampleSlotCount::new(5)),
                phase: HostExecutionPhase::ViScanout,
            }]
        );
        assert_eq!(probe.drain_underrun_observations(&mut observations), 0);
        assert_eq!(
            observations.len(),
            1,
            "a drained event is not published twice"
        );
    }

    #[test]
    fn host_execution_probe_bounds_storage_and_reports_every_loss() {
        let probe = HostExecutionProbe::new();
        let callback_at = std::time::Instant::now();
        let empty = DrainOutcome::inspected(4, 4, 0, OutputLandmarks::default());
        let allocated = probe.0.observations.lock().unwrap().capacity();
        for _ in 0..UNDERRUN_OBSERVATION_CAPACITY + 3 {
            probe.record_underrun(callback_at, empty, HostExecutionPhase::GuestStep);
        }
        assert_eq!(probe.0.observations.lock().unwrap().capacity(), allocated);
        let mut observations = Vec::new();
        assert_eq!(probe.drain_underrun_observations(&mut observations), 3);
        assert_eq!(observations.len(), UNDERRUN_OBSERVATION_CAPACITY);
        assert_eq!(observations.first().unwrap().sequence, 1);
        assert_eq!(
            observations.last().unwrap().sequence,
            UNDERRUN_OBSERVATION_CAPACITY as u64
        );
        assert_eq!(probe.drain_underrun_observations(&mut observations), 0);

        probe.record_underrun(callback_at, empty, HostExecutionPhase::WindowPresent);
        let mut later = Vec::new();
        assert_eq!(probe.drain_underrun_observations(&mut later), 0);
        assert_eq!(later[0].sequence, UNDERRUN_OBSERVATION_CAPACITY as u64 + 4);
    }

    #[test]
    fn host_execution_probe_never_waits_behind_a_concurrent_drain() {
        let probe = HostExecutionProbe::new();
        let consumer = probe.0.observations.lock().unwrap();
        probe.record_underrun(
            std::time::Instant::now(),
            DrainOutcome::inspected(4, 4, 0, OutputLandmarks::default()),
            HostExecutionPhase::DeviceAdvance,
        );
        drop(consumer);
        let mut observations = Vec::new();
        assert_eq!(probe.drain_underrun_observations(&mut observations), 1);
        assert!(observations.is_empty());
    }

    #[test]
    fn concurrent_underrun_publication_sequences_stay_ordered_while_occurrence_time_may_regress() {
        const PUBLISHERS: usize = 32;
        let probe = HostExecutionProbe::new();
        let release = Arc::new(std::sync::Barrier::new(PUBLISHERS + 1));
        let callback_base = std::time::Instant::now();
        let empty = DrainOutcome::inspected(4, 4, 0, OutputLandmarks::default());
        let mut publishers = Vec::with_capacity(PUBLISHERS);
        for index in 0..PUBLISHERS {
            let publisher_probe = probe.clone();
            let publisher_release = Arc::clone(&release);
            publishers.push(std::thread::spawn(move || {
                let callback_at = callback_base
                    + std::time::Duration::from_nanos((PUBLISHERS - index) as u64);
                publisher_release.wait();
                publisher_probe.record_underrun(
                    callback_at,
                    empty,
                    HostExecutionPhase::Waiting,
                );
            }));
        }
        release.wait();
        for publisher in publishers {
            publisher.join().unwrap();
        }

        let mut observations = Vec::new();
        let lost = probe.drain_underrun_observations(&mut observations);
        assert_eq!(observations.len() as u64 + lost, PUBLISHERS as u64);
        assert!(observations
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence));

        let ordered = HostExecutionProbe::new();
        ordered.record_underrun(
            callback_base + std::time::Duration::from_nanos(2),
            empty,
            HostExecutionPhase::Waiting,
        );
        ordered.record_underrun(
            callback_base + std::time::Duration::from_nanos(1),
            empty,
            HostExecutionPhase::Waiting,
        );
        let mut reversed_occurrence = Vec::new();
        assert_eq!(
            ordered.drain_underrun_observations(&mut reversed_occurrence),
            0
        );
        assert_eq!(reversed_occurrence[0].sequence, 1);
        assert_eq!(reversed_occurrence[1].sequence, 2);
        assert!(reversed_occurrence[0].callback_at > reversed_occurrence[1].callback_at);
    }

    #[test]
    fn content_free_vi_and_window_stalls_attribute_callback_underruns() {
        let probe = HostExecutionProbe::new();
        let callback_at = std::time::Instant::now();
        for phase in [
            HostExecutionPhase::ViScanout,
            HostExecutionPhase::WindowPresent,
        ] {
            let entered = Arc::new(std::sync::Barrier::new(2));
            let release = Arc::new(std::sync::Barrier::new(2));
            let callback_probe = probe.clone();
            let callback_entered = Arc::clone(&entered);
            let callback_release = Arc::clone(&release);
            let callback = std::thread::spawn(move || {
                callback_entered.wait();
                callback_probe.record_underrun(
                    callback_at,
                    DrainOutcome::inspected(8, 8, 0, OutputLandmarks::default()),
                    callback_probe.phase(),
                );
                callback_release.wait();
            });
            let prior = probe.set_phase(phase);
            entered.wait();
            release.wait();
            probe.set_phase(prior);
            callback.join().unwrap();
        }
        let mut observations = Vec::new();
        assert_eq!(probe.drain_underrun_observations(&mut observations), 0);
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].sequence, 1);
        assert_eq!(observations[0].phase, HostExecutionPhase::ViScanout);
        assert_eq!(observations[1].sequence, 2);
        assert_eq!(
            observations[1].phase,
            HostExecutionPhase::WindowPresent
        );
        assert!(observations
            .iter()
            .all(|entry| entry.reason == AudioUnderrunReason::RingEmpty));
    }

    #[test]
    fn cpal_backend_exposes_attached_probe_through_the_backend_trait() {
        let probe = HostExecutionProbe::new();
        let mut backend = CpalBackend::new();
        backend.install_host_execution_probe(probe.clone());
        assert_eq!(
            backend.set_host_execution_phase(HostExecutionPhase::WindowPresent),
            Some(HostExecutionPhase::Waiting)
        );
        assert_eq!(probe.phase(), HostExecutionPhase::WindowPresent);
        let mut observations = Vec::new();
        assert_eq!(backend.drain_underrun_observations(&mut observations), 0);
        assert!(observations.is_empty());
    }

    #[test]
    fn ai_length_tracks_only_the_head_dma_not_host_prebuffer() {
        let mut ring = OutputRing::with_capacity(32);
        ring.push_dma(&[1; 8], dma_bytes(16), OutputLandmarks::default());
        ring.push_dma(&[2; 8], dma_bytes(16), OutputLandmarks::default());
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
        ring.push_dma(&[1; 8], dma_bytes(16), OutputLandmarks::default());
        assert_eq!(
            ring.push_dma(&[2; 8], dma_bytes(16), OutputLandmarks::default())
                .dropped_sample_slots,
            6
        );
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
        ring.push_dma(
            &[i16::MIN, -16384, 0, 16384],
            dma_bytes(8),
            OutputLandmarks::default(),
        );
        let mut output = [99.0; 6];

        assert_eq!(
            ring.drain_into_f32(&mut output),
            (2, OutputLandmarks::default())
        );
        assert_eq!(output, [-1.0, -0.5, 0.0, 0.5, 0.0, 0.0]);
        assert_eq!(ring.current_dma_bytes_remaining(), GuestDmaByteCount::ZERO);
    }

    #[test]
    fn ring_reports_exact_landmark_slot_inside_callback_drain() {
        let mut ring = OutputRing::with_capacity(16);
        let id = fn64_runtime::AiDmaId::new(7);
        ring.push_dma(
            &[1; 10],
            dma_bytes(20),
            OutputLandmarks {
                presentation: None,
                sync: Some((id, 6)),
            },
        );
        let mut output = [0.0; 8];

        assert_eq!(
            ring.drain_into_f32(&mut output),
            (
                0,
                OutputLandmarks {
                    presentation: None,
                    sync: Some((id, 6)),
                },
            )
        );
    }

    #[test]
    fn sinc_landmark_waits_for_future_support_then_crosses_once() {
        let id = fn64_runtime::AiDmaId::new(9);
        let mut resampler = BandlimitedResampler::new();
        let first = vec![0; 20 * 2];
        let second = vec![0; 32 * 2];
        let mut first_output = Vec::new();
        let mut second_output = Vec::new();

        assert_eq!(
            resampler.process_tagged(
                stereo(&first),
                AiSamplePeriod::new(32_000, 1),
                host_rate(48_000),
                OutputLandmarks {
                    presentation: None,
                    sync: Some((id, 18)),
                },
                &mut first_output,
            ),
            OutputLandmarks::default(),
            "the marker cannot cross before the sinc's future support arrives"
        );
        let crossed = resampler
            .process_tagged(
                stereo(&second),
                AiSamplePeriod::new(32_000, 1),
                host_rate(48_000),
                OutputLandmarks::default(),
                &mut second_output,
            )
            .sync
            .expect("the retained marker crosses with the next input chunk");
        assert_eq!(crossed.0, id);
        assert_eq!(crossed.1 % 2, 0, "stereo landmark stays frame aligned");
    }

    #[test]
    fn sync_probe_selects_first_loud_frame_after_quiet_gate() {
        let mut probe = AudioSyncProbeProducer {
            threshold: 100,
            quiet_ms: 1,
            quiet_frames: 0,
            selected: false,
        };
        let mut samples = vec![0; 66];
        samples.extend_from_slice(&[101, 0, 0, 0]);

        assert_eq!(
            probe.inspect(stereo(&samples), guest_rate(32_000)),
            Some(33)
        );
        assert_eq!(
            probe.inspect(stereo(&[200, 200]), guest_rate(32_000)),
            None,
            "the bounded probe publishes only one landmark"
        );
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
