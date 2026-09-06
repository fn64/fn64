//! The graphics-backend seam, split into three cohesive traits.
//!
//! This module replaces the single 38-method `RenderBackend` god-trait with
//! three traits that separate concerns a backend can genuinely implement
//! independently:
//!
//! * [`RenderBackend`] -- lifecycle, HLE task execution, and presentation.
//!   Every backend implements this; it has no default-bodied escape hatch for
//!   `create`, `present`, `resize`, `process_task`, `observe_non_rdp_write16`,
//!   or `supported_ucodes`.
//! * [`RawDpcBackend`] -- the raw-DPC plan/execute/publish production seam
//!   (`raw_dpc_*`, `plan_*`, `execute_*`, `publish_raw_dpc`). Every method has
//!   a loud default, so a backend with no raw-DPC ambitions writes
//!   `impl RawDpcBackend for MyBackend {}`.
//! * [`SettingsSink`] -- the typed settings/policy staging surface. Also fully
//!   defaulted to a named `RenderError::Backend` refusal.
//!
//! [`FullBackend`] is the blanket-implemented composition of all three. It is
//! what `fn64-abi` and `fn64-shell` store behind `Box<dyn ...>`, because the
//! registered backend is called through all three surfaces from one owner.
//! Code that needs only one surface should take the narrowest bound: the
//! raw-DPC session paths take `&mut dyn RawDpcBackend`, and settings plumbing
//! takes `&mut dyn SettingsSink`.
//!
//! Splitting the trait changed no method body, no method name, and no return
//! type; the partition is purely a regrouping of the same seam.

mod raw_dpc;
mod settings;

pub use raw_dpc::RawDpcBackend;
pub use settings::SettingsSink;

use super::*;

/// A graphics backend: consumes N64 gfx tasks (F3DEX-family display lists
/// from rdram) and produces frames. Per `docs/DECOUPLING.md`: "The runtime
/// submits gfx OSTasks through the single executor event seam to a `dyn
/// RenderBackend`; the backend never reaches back into runtime state" --
/// every method here takes exactly the data it needs (a byte slice, a task
/// struct, plain dimensions) and returns a plain `Result`. No callback into
/// the runtime, no shared mutable state beyond `&mut self`.
pub trait RenderBackend {
    /// Initialize the backend (device/window/surface) for a target of
    /// `cfg.width x cfg.height`. Must be called before `process_task` or
    /// `present`; calling it twice is backend-defined (a reference backend
    /// may treat it as a full reset).
    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError>;

    /// Observe one completed CPU/non-RDP halfword store to physical RDRAM.
    /// The public RDP memory-interface rule assigns a hidden-bit mutation to
    /// every such store, including a same-value store that byte comparison
    /// cannot discover later. Backends must state whether they applied that
    /// mutation to a Rust-owned sidecar; there is deliberately no silent
    /// default implementation.
    fn observe_non_rdp_write16(&mut self, write: NonRdpWrite16) -> NonRdpWrite16Disposition;

    /// Declare that non-RDP halfword observations may be retained in guest
    /// order while this backend is executing an owned raw-DPC batch on a
    /// worker. `None` keeps the backend strictly synchronous. A returned
    /// disposition is both the immediate answer and the value every deferred
    /// replay must return; disagreement is a backend contract violation.
    fn deferred_non_rdp_write16_disposition(&self) -> Option<NonRdpWrite16Disposition> {
        None
    }

    /// Process one RSP gfx task: walk `task`'s display list (rooted at
    /// `task.data_ptr`, per the public libultra manual's `OSTask_t.data_ptr`
    /// field being the display-list start for `M_GFXTASK`) out of `rdram`
    /// and render into the backend's current target. `rdram` is the WHOLE
    /// N64 memory image (matching `RECOMP_FUNC`'s own `uint8_t* rdram`
    /// convention, per `docs/DESIGN.md` section 2) -- the backend reads
    /// vertex/texture/matrix data out of it directly, never through any
    /// runtime API, which is the "never reaches back into runtime state"
    /// invariant made concrete. `rsp_memory` is the device fabric's ONE
    /// persistent DMEM/IMEM image. Requiring it at the trait boundary makes
    /// debug GBI DMA, CPU SP-memory access, LLE overlays, and later commands
    /// in the same task share state by construction; a backend-private shadow
    /// is not a conforming implementation.
    ///
    /// `rdram` is `&mut` because on real hardware the RDP writes the
    /// rasterized color image back into DRAM (the framebuffer the VI then
    /// scans out). `output_addr` is the physical rdram byte offset of that
    /// color framebuffer -- the region the VI presents (`osViSwapBuffer`'s
    /// frame buffer), NOT the RSP task's `output_buff` field (which on OoT
    /// is the RSP's DRAM command-FIFO output region at ~0x151640, a
    /// different address than the game's color image at 0x3b5000/0x3da800).
    /// A backend that renders into its own private surface must copy the
    /// result into `rdram[output_addr..]` (in the framebuffer's native
    /// format, RGBA5551 for OoT's 16-bit mode) so the VI-presented frame is
    /// not blank. `output_addr == 0` means "no known color target" (a
    /// fixture/test path with no VI framebuffer): the backend renders into
    /// its own surface only and writes nothing back.
    fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError>;

    /// Execute one HLE task chunk or resume one backend-owned continuation.
    ///
    /// The default is the explicit compatibility adapter: a start delegates
    /// to the historical atomic `process_task`, while a resume traps by token.
    /// Backends returning `Continue` must also report `Resumable` from
    /// `task_chunking` and retain exactly one continuation for that token.
    fn process_task_chunk(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
        step: RenderTaskStep,
    ) -> Result<RenderTaskChunkStatus, RenderError> {
        match step {
            RenderTaskStep::Start => Ok(
                match self.process_task(rdram, rsp_memory, task, output_addr)? {
                    FrameStatus::Complete => RenderTaskChunkStatus::Complete,
                    FrameStatus::Yielded => RenderTaskChunkStatus::Yielded,
                    FrameStatus::NeedsLle { ucode_sha256 } => {
                        RenderTaskChunkStatus::NeedsLle { ucode_sha256 }
                    }
                },
            ),
            RenderTaskStep::Resume(token) => Err(RenderError::Backend {
                backend: "render-task-continuation",
                reason: format!(
                    "atomic backend cannot resume continuation token {}",
                    token.get()
                ),
            }),
        }
    }

    fn task_chunking(&self) -> RenderTaskChunking {
        RenderTaskChunking::Atomic
    }

    /// FullSync result of the immediately preceding successful task, raw DPC
    /// submission, or committed task chunk. For a resumable task this result
    /// is cumulative through the returned continuation. Implementations reset
    /// it to `Unidentified` before new work and publish identified state only
    /// after commit.
    fn last_dp_full_sync(&self) -> DpFullSyncStatus {
        DpFullSyncStatus::Unidentified
    }

    /// Present one VI field from the most recently rendered framebuffer
    /// (scan it to screen, or for a headless backend, finalize it as
    /// retrievable). Distinct from `process_task` because each hardware VI
    /// retrace is separate from RSP task completion; `osViSwapBuffer` only
    /// selects which rendered buffer a later field consumes. Multiple gfx
    /// tasks can render before one present, matching double/triple-buffering,
    /// and unchanged progressive fields still present with distinct cadence
    /// and retrace-seeded scanout noise.
    fn present(&mut self, request: PresentRequest<'_>) -> Result<(), RenderError>;

    /// Consume the source field produced by the immediately preceding
    /// successful [`Self::present`]. Backends without this capability return
    /// `Unsupported`; a provider must return `Ready` exactly once per present.
    fn take_presented_source_field(&mut self) -> PresentedSourceFieldAvailability {
        PresentedSourceFieldAvailability::Unsupported
    }

    /// Select move-only post-VI delivery for subsequent successful presents.
    ///
    /// This explicit selection prevents a backend from allocating and copying
    /// a host-consumer field that no caller will take. The default is a named
    /// capability error rather than a mode switch that is silently ignored.
    fn enable_presented_post_vi_field_delivery(&mut self) -> Result<(), RenderError> {
        Err(RenderError::Backend {
            backend: "presented-post-vi-field",
            reason: "registered backend does not expose post-VI field delivery".to_string(),
        })
    }

    /// Consume the post-VI field produced by the immediately preceding
    /// successful [`Self::present`]. This stage is distinct from
    /// [`Self::take_presented_source_field`]: its pixels have already passed
    /// through every VI filter the backend admitted. Backends without this
    /// capability return `Unsupported`; a provider returns `Ready` exactly
    /// once per present.
    fn take_presented_post_vi_field(&mut self) -> PresentedPostViFieldAvailability {
        PresentedPostViFieldAvailability::Unsupported
    }

    /// Return the most recent completed renderer image for fixed-cycle
    /// release evidence. Ordinary rendering does not require this opt-in
    /// capability; asking a backend that cannot prove a typed capture is a
    /// named error rather than an empty image or stale fallback.
    fn release_capture(&mut self) -> Result<RenderReleaseCapture, RenderError> {
        Err(RenderError::Backend {
            backend: "render-release-capture",
            reason: "registered backend does not expose typed release capture".to_string(),
        })
    }

    /// Fill and return the most recent completed renderer image using a
    /// caller-owned allocation when the backend supports it.
    ///
    /// On success, ownership of `reuse` moves into the returned capture and
    /// the caller can recover it from [`ReleaseCapturePixels::into_bytes`]
    /// after consuming the image. On failure, `reuse` remains caller-owned. The
    /// default preserves existing backend behavior and leaves `reuse`
    /// untouched; allocation-sensitive backends override this seam.
    fn release_capture_into(
        &mut self,
        reuse: &mut Vec<u8>,
    ) -> Result<RenderReleaseCapture, RenderError> {
        let _ = reuse;
        self.release_capture()
    }

    /// Report the concrete backend and active capabilities for fixed-cycle
    /// evidence. Hosts cannot provide this value separately, so a reference
    /// backend cannot be relabeled as RT64 (or vice versa) after registration.
    fn release_environment(&self) -> RenderBackendEvidence {
        RenderBackendEvidence::Unidentified
    }

    /// Inspect effective target geometry after both renderer workers are idle.
    /// This is an explicit diagnostic seam rather than release-capture data:
    /// implementations may need synchronization that is inappropriate on the
    /// ordinary presentation path.
    fn render_target_diagnostic(&mut self) -> Result<RenderTargetDiagnostic, RenderError> {
        Err(RenderError::NotReady(
            "render-target diagnostics are unsupported by this backend",
        ))
    }

    /// The output target changed size (a real window resize, or a harness
    /// reconfiguring a headless target). Infallible by design: a backend
    /// that cannot honor a resize should surface that at the next
    /// `process_task`/`present` call via `RenderError`, not here -- this
    /// keeps window-resize event handling (which callers can't always
    /// gate on a `Result`) simple to wire.
    fn resize(&mut self, w: u32, h: u32);

    /// Identify one complete logical IMEM image only when this backend has
    /// explicitly admitted its exact digest as a public HLE microcode family.
    /// This is evidence about content identity, not an execution selector:
    /// callers still dispatch through the runtime's HLE/LLE mechanism, and
    /// compatibility backends make no identity claim by default.
    fn identify_microcode(
        &self,
        _imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    ) -> Option<UcodeId> {
        None
    }

    /// Identify one exact text/data pair for runtime consumption evidence.
    /// Text-only HLE admission is deliberately insufficient: compatibility
    /// backends and catalogs that have not admitted this complete pair return
    /// `None` even if [`Self::identify_microcode`] recognizes the IMEM image.
    fn identify_microcode_pair(
        &self,
        _imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        _data: MicrocodeDataImageIdentity,
    ) -> Option<UcodeId> {
        None
    }

    /// Which microcode families this backend actually implements. A task
    /// using an unlisted ucode must be rejected by `process_task` with
    /// `RenderError::UnsupportedUcode` (named, not a silent black frame) --
    /// callers are expected to consult this before dispatch too, but
    /// `process_task` is the enforced boundary, not this advisory list.
    fn supported_ucodes(&self) -> &[UcodeId];
}

/// The complete backend surface: everything a registered renderer must
/// provide to the ABI. Blanket-implemented, so a backend never names it --
/// implementing the three constituent traits is what makes a type a
/// `FullBackend`.
///
/// This exists because `fn64-abi` owns exactly one registered backend and
/// drives it through all three seams (`RENDER_BACKEND`'s
/// `Box<dyn FullBackend>`), so the trait object must carry all three vtables.
/// Consumers that touch only one surface take that trait's own object type
/// instead; `dyn FullBackend` upcasts to each supertrait.
pub trait FullBackend: RenderBackend + RawDpcBackend + SettingsSink {}

impl<T: RenderBackend + RawDpcBackend + SettingsSink> FullBackend for T {}
