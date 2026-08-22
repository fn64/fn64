//! Adapterless entry point that drives this backend's raw-DPC seam over an
//! arbitrary RDP command stream and returns the guest-visible bytes it
//! published.
//!
//! **Why this module exists.** `crates/fn64-render-conformance` is a
//! backend-neutral replay-and-verify harness with two runner binaries -- a
//! reference-backend one and an RT64 one -- and no wgpu one. Wiring a wgpu
//! runner needed a *public*, ROM-free, adapter-free way to hand this backend
//! a list of RDP command words and read back what it committed to guest
//! memory. Every existing path to that seam was `#[cfg(test)]`
//! (`crate::rdp_harness`) or required a caller that already owned a
//! `RawDpcAbiSession`, a `DeviceFabric` and a publication token
//! (`fn64-abi`). This module is the minimum public surface that closes that
//! gap, and it is gated behind the `conformance-runner` feature so the
//! default build is unchanged.
//!
//! **It is deliberately not a second renderer path.** It performs exactly
//! the same plan -> execute -> commit -> seal -> publish sequence
//! `crate::rdp_harness::publish_packet` performs, against the same public
//! `RenderBackend` methods, and reads the result from the same
//! `ColorTargetRegistry` resident. It stages no state of its own and skips
//! no stage; a guard that refuses in production refuses here, by name.
//!
//! **Adapterless.** `create_inner` records the host-configured target extent
//! without a device (`WgpuCreateError::NoAdapter` is expected and ignored,
//! exactly as `rdp_harness::configure_extent` treats it), and the bytes this
//! module returns come from `ColorTargetRegistry`'s `device_bytes`, which is
//! a CPU `Vec<u8>`. No GPU adapter is required for any path here. A command
//! stream whose execution genuinely needs a device -- a raw triangle draw,
//! which readback goes through `triangle_draw_output` -- is refused by the
//! backend's own named error rather than silently approximated, and that
//! refusal is returned to the caller as [`ConformanceRefusal::Execute`].

use crate::production::{WgpuBackend, WgpuCreateError};
use fn64_render::{OwnedRawDpcSubmission, RawDpcAbiSession, RenderBackend};
use fn64_render_ir::{
    CapturedGuestRead, CompletedWrite, DeferredGuestReadCapture, DpInterruptState, TemporalBoundary,
};

/// One replayable raw-DPC command stream and the guest layout it addresses.
///
/// Every field is stated by the caller rather than defaulted, because a
/// conformance runner's whole job is to replay one exact fixture: a driver
/// that silently supplied its own layout or transaction sequence would make
/// two backends disagree about the input rather than about the semantics.
#[derive(Clone, Debug)]
pub struct ConformanceReplay {
    /// Total physical RDRAM size the packet's layout covers, in bytes.
    pub layout_bytes: u32,
    /// Physical address the command words are read from.
    pub command_start: u32,
    /// The RDP command words, in stream order, exactly as RDRAM carries them.
    pub words: Vec<u32>,
    /// Transaction sequence stamped into the capture.
    pub transaction_sequence: u64,
    /// Source bytes for each TMEM load the plan declares, in declaration
    /// order. A count mismatch against the plan is a refusal, not a silent
    /// zip-truncation.
    pub guest_read_sources: Vec<Vec<u8>>,
    /// The fixture's own guest RDRAM, in storage byte order, used to satisfy
    /// declared reads that `guest_read_sources` does not enumerate.
    ///
    /// **Why a second source rather than more entries in the first.**
    /// `guest_read_sources` is positional and hand-written per fixture: it
    /// exists so a TMEM-load fixture can state its texels without owning a
    /// whole RDRAM image. A partial `FillRectangle` now also declares a
    /// read -- its colour-image seed -- which no fixture author chose and
    /// whose size follows the target rather than the display list. Making
    /// every existing fixture grow an entry for it would be asking authors
    /// to hand-maintain a copy of memory they already supplied.
    ///
    /// When present, a declared read is served by slicing this at the
    /// read's own range, which is exactly what `fn64-abi` does with the
    /// live allocation (`task_dispatch/rsp_commit.rs`). `guest_read_sources`
    /// still takes precedence in declaration order, so existing fixtures
    /// replay unchanged.
    pub guest_rdram: Option<Vec<u8>>,
    /// Render-target extent handed to `create`. This is the host-configured
    /// extent, not a claim about the color image's own width.
    pub target_width: u32,
    pub target_height: u32,
}

/// Which stage refused, keyed so a caller can report the stage rather than
/// pattern-match a formatted string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConformanceRefusal {
    Construction(String),
    Capture(String),
    Plan(String),
    /// The plan declared a different number of TMEM-load source reads than
    /// the replay staged.
    GuestReadCount {
        declared: usize,
        staged: usize,
    },
    Submit(String),
    Execute(String),
    Commit(String),
    Seal(String),
    /// Execution completed but published no color-target resident at
    /// `address`. For a fill-only stream that means the fill was not
    /// admitted; it is never "the render was a no-op".
    NoResident {
        address: u32,
    },
    /// A declared read named a range outside the RDRAM the replay supplied.
    GuestReadOutOfBounds {
        start: usize,
        end: usize,
    },
}

impl core::fmt::Display for ConformanceRefusal {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Construction(message) => write!(out, "backend construction refused: {message}"),
            Self::Capture(message) => write!(out, "capture construction refused: {message}"),
            Self::Plan(message) => write!(out, "plan_raw_dpc refused: {message}"),
            Self::GuestReadCount { declared, staged } => write!(
                out,
                "the plan declared {declared} TMEM-load source reads but the replay staged {staged}"
            ),
            Self::Submit(message) => write!(out, "finalize_and_submit refused: {message}"),
            Self::Execute(message) => write!(out, "execute_raw_dpc refused: {message}"),
            Self::Commit(message) => {
                write!(out, "commit_guest_render_target_writes refused: {message}")
            }
            Self::Seal(message) => write!(out, "seal_publication refused: {message}"),
            Self::NoResident { address } => {
                write!(out, "no color-target resident published at {address:#010x}")
            }
            Self::GuestReadOutOfBounds { start, end } => write!(
                out,
                "a declared guest read [{start:#x}, {end:#x}) lies outside the replay's RDRAM"
            ),
        }
    }
}

impl std::error::Error for ConformanceRefusal {}

/// What one replayed packet published.
#[derive(Clone, Debug)]
pub struct ConformanceOutcome {
    /// The guest-visible bytes of the color-target resident at the requested
    /// address, straight from `ColorTargetRegistry`'s CPU-side
    /// `device_bytes`.
    pub target_bytes: Vec<u8>,
    /// The guest render-target writes this backend staged and the ABI
    /// session then committed, in journal order.
    pub committed_writes: Vec<CompletedWrite>,
}

/// A live backend replaying a sequence of packets against one durable RDP
/// state, which is what a multi-packet fixture needs: `SetColorImage` and
/// `SetOtherMode` are durable across submissions, so replaying two packets
/// through two fresh backends would not be the same replay.
pub struct ConformanceSession {
    backend: WgpuBackend,
    session: RawDpcAbiSession,
}

impl ConformanceSession {
    /// Construct a backend paired with its own ABI session and record the
    /// host-configured target extent.
    ///
    /// `NoAdapter` is expected on an adapterless host and is not an error:
    /// `create_inner` still records the extent, which is the only thing the
    /// CPU-side executors read from it. Any other create failure is
    /// returned, because that would mean the extent was *not* recorded and
    /// every later stage would be measuring the harness.
    pub fn try_new(width: u32, height: u32) -> Result<Self, ConformanceRefusal> {
        let (mut backend, session) = WgpuBackend::try_new()
            .map_err(|error| ConformanceRefusal::Construction(error.to_string()))?;
        match backend.create_inner(&fn64_render::RenderConfig {
            width,
            height,
            tv_type: fn64_runtime::TvType::default(),
        }) {
            Ok(()) | Err(WgpuCreateError::NoAdapter(_)) => {}
            Err(other) => return Err(ConformanceRefusal::Construction(other.to_string())),
        }
        if !backend.has_configured_target_extent() {
            return Err(ConformanceRefusal::Construction(
                "create_inner did not record the host-configured target extent".to_string(),
            ));
        }
        Ok(Self { backend, session })
    }

    /// Replay one packet all the way through the raw-DPC seam and read back
    /// the resident published at `target_address`.
    pub fn replay(
        &mut self,
        replay: &ConformanceReplay,
        target_address: u32,
    ) -> Result<ConformanceOutcome, ConformanceRefusal> {
        let committed_writes = self.publish(replay)?;
        let target_bytes = self
            .backend
            .color_targets()
            .and_then(|registry| {
                registry
                    .residents()
                    .iter()
                    .find(|resident| resident.key().address().get() == target_address)
                    .map(|resident| resident.device_bytes().device_bytes().to_vec())
            })
            .ok_or(ConformanceRefusal::NoResident {
                address: target_address,
            })?;
        Ok(ConformanceOutcome {
            target_bytes,
            committed_writes,
        })
    }

    /// Replay one packet without reading any resident back. Used for a
    /// target-establishing packet whose bytes the fixture does not observe.
    pub fn replay_without_readback(
        &mut self,
        replay: &ConformanceReplay,
    ) -> Result<Vec<CompletedWrite>, ConformanceRefusal> {
        self.publish(replay)
    }

    /// plan -> execute -> commit -> seal -> publish. The identical sequence
    /// and the identical public methods `crate::rdp_harness::publish_packet`
    /// drives; no stage is skipped and no guard is widened.
    fn publish(
        &mut self,
        replay: &ConformanceReplay,
    ) -> Result<Vec<CompletedWrite>, ConformanceRefusal> {
        let capture = build_capture(replay)?;
        let request = self.session.plan_request(capture);
        let planned = self
            .backend
            .plan_raw_dpc(request)
            .map_err(|error| ConformanceRefusal::Plan(error.to_string()))?;
        let reads = planned.guest_read_plan().reads();
        // Only the reads NOT served by `guest_rdram` have to be enumerated
        // positionally. With no RDRAM supplied that is all of them, which is
        // the original contract; with RDRAM supplied a fixture may still
        // enumerate a prefix, and the remainder is sliced from memory.
        let enumerated = replay.guest_read_sources.len();
        if replay.guest_rdram.is_none() && reads.len() != enumerated {
            return Err(ConformanceRefusal::GuestReadCount {
                declared: reads.len(),
                staged: enumerated,
            });
        }
        if enumerated > reads.len() {
            return Err(ConformanceRefusal::GuestReadCount {
                declared: reads.len(),
                staged: enumerated,
            });
        }
        let mut captured = Vec::with_capacity(reads.len());
        for (ordinal, read) in reads.iter().enumerate() {
            let len = read.range().len() as usize;
            let bytes = if let Some(source) = replay.guest_read_sources.get(ordinal) {
                // A declared read may be LONGER than the texels the
                // fixture supplied (`LoadBlock` reads whole 64-bit
                // words). Zero-padded rather than refused, matching
                // `rdp_harness::publish_packet` exactly.
                let mut padded = source.clone();
                padded.resize(len, 0);
                padded
            } else {
                let rdram = replay
                    .guest_rdram
                    .as_deref()
                    .expect("the count check above proves every read is enumerated without RDRAM");
                let start = read.range().start().get() as usize;
                let end = start + len;
                if end > rdram.len() {
                    return Err(ConformanceRefusal::GuestReadOutOfBounds { start, end });
                }
                // **Logical order, not raw storage.** `CapturedGuestRead`'s
                // contract is N64-logical bytes (`fn64-render-ir`'s
                // `guest_read.rs`: "One independently owned logical-byte
                // capture"), and the TMEM load executors index the capture
                // linearly with no lane mapping of their own. Guest RDRAM
                // stores bytes under the `^3` logical-to-storage map, so a
                // bare `rdram.get(start..end)` hands the sampler each 32-bit
                // word byte-reversed.
                //
                // Measured before this fix: an eight-texel RGBA16 fixture
                // came back as the raw storage halfwords -- `0xc107` where
                // `0xf801` was staged, all eight explained by that one rule
                // -- while RT64, reading the identical buffer, returned the
                // key exactly.
                //
                // A 32-bit command word survives the raw read by accident
                // (`^3` composed with a little-endian host load cancels), but
                // texture bytes are byte-granular and arbitrarily aligned, so
                // nothing cancels for them.
                let mut bytes = vec![0; len];
                fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(
                    fn64_runtime::RdramAddr::from_offset(read.range().start().get()),
                    &mut bytes,
                );
                bytes
            };
            captured.push(
                CapturedGuestRead::try_new(*read, bytes)
                    .expect("a capture sized to its own declared read is well formed"),
            );
        }
        let capture = DeferredGuestReadCapture::new(captured);
        let bound = self
            .session
            .finalize_and_submit(planned, capture)
            .map_err(|error| ConformanceRefusal::Submit(error.to_string()))?;
        let submission = bound.submission();
        let prepared = self
            .backend
            .execute_raw_dpc(bound)
            .map_err(|error| ConformanceRefusal::Execute(error.to_string()))?;
        let staged = self.backend.staged_guest_render_target_writes(submission);
        let committed = self
            .session
            .commit_guest_render_target_writes(prepared, staged.clone())
            .map_err(|error| ConformanceRefusal::Commit(error.to_string()))?;
        let mut fabric = admitted_fabric();
        let token = fabric
            .pending_dpc_submission()
            .expect("a fabric seeded with one submission holds it")
            .token;
        let ready = fabric
            .prepare_dpc_commit(token)
            .expect("a freshly seeded fabric admits its own only pending commit");
        let capsule = self
            .session
            .seal_publication(committed, ready)
            .map_err(|error| ConformanceRefusal::Seal(error.to_string()))?;
        self.backend.publish_raw_dpc(capsule);
        Ok(staged)
    }

    /// The backend under replay, for a caller that needs to read a
    /// diagnostic surface (`rdp_state`, `physical_tmem`, `color_targets`)
    /// the outcome does not carry.
    pub fn backend(&self) -> &WgpuBackend {
        &self.backend
    }
}

fn build_capture(
    replay: &ConformanceReplay,
) -> Result<fn64_render::OwnedRawDpcCapture, ConformanceRefusal> {
    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(replay.layout_bytes)
        .map_err(|error| ConformanceRefusal::Capture(format!("{error:?}")))?;
    let byte_len = u32::try_from(replay.words.len() * 4)
        .map_err(|_| ConformanceRefusal::Capture("command stream length overflows u32".into()))?;
    let end = replay
        .command_start
        .checked_add(byte_len)
        .ok_or_else(|| ConformanceRefusal::Capture("command end overflows u32".into()))?;
    let submission =
        OwnedRawDpcSubmission::from_rdram_words(replay.command_start, end, replay.words.clone())
            .map_err(|error| ConformanceRefusal::Capture(format!("{error:?}")))?;
    // A conformance replay is a CLOSED capture: its words are fixed and no
    // later DPC_END can extend them, so a range ending inside a command is a
    // malformed fixture rather than the hardware stall the raw CPU ingress
    // parks. Refuse it by name instead of treating it as "wait for more".
    let sites = fn64_render::count_raw_rdp_full_sync_sites(&replay.words)
        .map_err(|error| ConformanceRefusal::Capture(format!("{error:?}")))?
        .complete()
        .ok_or_else(|| {
            ConformanceRefusal::Capture(
                "replay words end inside a command; a closed capture cannot be extended"
                    .to_string(),
            )
        })?;
    let cmd_end = TemporalBoundary::new(1, DpInterruptState::Clear);
    if sites == 0 {
        return Ok(fn64_render::OwnedRawDpcCapture::new(
            submission,
            layout,
            replay.transaction_sequence,
            cmd_end,
        ));
    }
    // One reserved boundary per SYNC_FULL opcode, both interrupt states
    // `Clear` -- what the real ABI producer's nonmutating
    // `preflight_dp_full_sync` reserve half supplies. The device fabric
    // raises the DP line only on a later `advance_to`, strictly after the
    // capture is built, so a reservation observes no interrupt.
    let boundaries = (0..sites as u64)
        .map(|ordinal| {
            fn64_render_ir::FullSyncBoundary::new(
                2 + ordinal * 2,
                3 + ordinal * 2,
                DpInterruptState::Clear,
                DpInterruptState::Clear,
            )
        })
        .collect();
    Ok(fn64_render::OwnedRawDpcCapture::with_full_sync_boundaries(
        submission,
        layout,
        replay.transaction_sequence,
        cmd_end,
        boundaries,
    ))
}

fn admitted_fabric(
) -> fn64_runtime::DeviceFabric<fn64_runtime::rom::InMemoryRom, fn64_runtime::FixedPiTiming> {
    let mut fabric = fn64_runtime::DeviceFabric::new(
        fn64_runtime::rom::PiDma::new(fn64_runtime::rom::InMemoryRom::new(Vec::new())),
        fn64_runtime::FixedPiTiming(fn64_runtime::Cycles::new(0)),
    );
    fabric
        .request_dpc_submission(fn64_runtime::DpcSubmissionSource::Rdram, 0x100, 0x108)
        .expect("a fresh fabric admits a well-formed submission request")
        .expect("fresh fabric is never frozen");
    fabric
}
