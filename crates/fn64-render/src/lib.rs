//! `fn64-render`: the graphics backend seam, per `docs/DECOUPLING.md`
//! ("Renderer seam -- `fn64-render` (RT64 today, ours later)").
//!
//! ## What this crate is
//!
//! `RenderBackend` is the ONE boundary the runtime uses to hand off N64 gfx
//! work: a captured RSP task header (`OsTask`, the public libultra manual's
//! `OSTask_t` field shape -- same fields `fn64_runtime::rsp::OsTaskHeader`
//! already models, mirrored here so this crate has zero dependency on
//! `fn64-runtime` internals, per `docs/DECOUPLING.md`'s "the backend never
//! reaches back into runtime state") plus the raw rdram byte buffer the
//! display list and its vertex/texture data live in. That's it. Lifecycle
//! (`create`/`resize`/`present`) and a `supported_ucodes` self-report round
//! out the trait.
//!
//! **Zero backend lives here.** `fn64-render-rt64` is the first adapter
//! (RT64 FFI, quarantined per `docs/DESIGN.md` section 1's C++ boundary
//! rule) and also home to a headless reference software rasterizer used as
//! the deterministic CI oracle for the feature-gated RT64 path -- see that
//! crate's README for each backend's build and fallback contract.
//!
//! ## Why `OsTask` is redefined here instead of reusing `fn64_runtime::rsp::OsTaskHeader`
//!
//! `fn64-render` must not depend on `fn64-runtime` (`docs/DECOUPLING.md`:
//! "Everything RT64-specific... lives behind that [boundary]"; the runtime
//! submits INTO this trait, this trait does not submit into the runtime).
//! A `From<OsTaskHeader> for OsTask` conversion is the adapter's job (or the
//! executor-seam glue crate's), not this crate's -- keeping this crate
//! buildable and testable with no other workspace crate in its dependency
//! graph at all.
#![forbid(unsafe_code)]

use std::fmt;

/// Public libultra manual's documented `OSTask_t` field shape -- the same
/// fields as `fn64_runtime::rsp::OsTaskHeader`, redeclared here (see module
/// doc) so this crate has no dependency on `fn64-runtime`. All fields are
/// rdram-relative byte offsets/raw values already translated out of MIPS
/// vram addressing (the caller did that translation before construction;
/// this crate never does KSEG0 math).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct OsTask {
    pub task_type: u32,
    pub flags: u32,
    pub ucode_boot: u32,
    pub ucode_boot_size: u32,
    pub ucode: u32,
    pub ucode_size: u32,
    pub ucode_data: u32,
    pub ucode_data_size: u32,
    pub dram_stack: u32,
    pub dram_stack_size: u32,
    pub output_buff: u32,
    /// `output_buff`'s end (`output_buff_end` in the real struct) -- not
    /// modeled in `fn64_runtime::rsp::OsTaskHeader` today (no call site
    /// needed it yet there), but a gfx backend needs an output bound to
    /// know how large the target buffer is, so it's included here.
    pub output_buff_size: u32,
    pub data_ptr: u32,
    pub data_size: u32,
}

/// Public libultra manual's documented `OSTask.t.type` constants. Duplicated
/// from `fn64_runtime::{M_GFXTASK, M_AUDTASK}` for the same no-dependency
/// reason as `OsTask` itself.
pub const M_GFXTASK: u32 = 1;
pub const M_AUDTASK: u32 = 2;

/// Which RSP graphics microcode family a task's display list is encoded in.
/// A backend's `supported_ucodes()` is the self-report a caller checks
/// BEFORE calling `process_task` -- an unlisted ucode must trap loudly by
/// name (`RenderError::UnsupportedUcode`), never silently produce a black
/// frame, per this task's explicit requirement.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum UcodeId {
    /// Fast3DEX2 family (the common late-era SDK gfx ucode; both No Mercy
    /// and Ocarina of Time's era used an F3DEX2-family microcode per public
    /// SDK documentation) -- the only family any backend in this workspace
    /// targets so far.
    F3dex2,
    /// Catch-all for a named-but-not-yet-modeled ucode family, so a backend
    /// can advertise partial/experimental support without this enum
    /// growing a variant per guess. `0` is never a real value produced by
    /// this crate's own code; it exists for a future adapter to construct.
    Other(u32),
}

/// Backend configuration for `RenderBackend::create`. Deliberately minimal
/// (window/output size only) -- a real windowing surface handle is backend-
/// specific (RT64 wants a native window handle; a headless backend wants
/// none), so this trait models only what every backend needs to agree on:
/// the target framebuffer dimensions. Backend-specific extras (a raw window
/// handle, a device preference) are the adapter crate's own config type,
/// passed alongside this one at the adapter's own construction, not through
/// this shared trait -- keeping `RenderConfig` itself backend-agnostic.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RenderConfig {
    pub width: u32,
    pub height: u32,
}

impl RenderConfig {
    pub fn new(width: u32, height: u32) -> Self {
        RenderConfig { width, height }
    }
}

/// Outcome of `process_task`. A gfx task on real hardware can complete
/// synchronously or ask the RSP to yield/resume later (`osSpTaskYield`'s
/// documented behavior) -- this mirrors that at the backend-seam level
/// without this crate needing to model the RSP scheduler itself (that stays
/// the runtime's job; this is just what the backend reports back about ITS
/// half of one submitted task).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FrameStatus {
    /// The task ran to completion; a frame may or may not have been
    /// presented yet (that's `present`'s job) but no further RSP-side work
    /// is pending for this task.
    Complete,
    /// The backend consumed as much of the task as it could and is
    /// yielding, matching `osSpTaskYield`'s real semantics -- the caller is
    /// expected to resume this same task later, not resubmit from scratch.
    Yielded,
}

/// Everything that can go wrong at this seam. Every variant is loud/named
/// (this task's explicit requirement: "traps by name (no silent black
/// frame)") -- there is no `RenderError::Other(String)` catch-all, so a
/// caller pattern-matching this enum can rely on it being exhaustive over
/// every failure this crate's own contract defines.
#[derive(Debug)]
pub enum RenderError {
    /// `process_task` was called with a ucode not present in
    /// `supported_ucodes()`. Carries the raw ucode text address (rdram-
    /// relative) so a diagnostic can point at exactly which task's ucode
    /// blob was unrecognized, without this crate needing to fingerprint
    /// ucode *contents* (that's the backend's own job, if it wants finer
    /// detection than "not in my declared list").
    UnsupportedUcode { ucode_addr: u32 },
    /// `task.output_buff`/`output_buff_size` describe a region outside the
    /// `rdram` slice `process_task` was given -- a malformed or
    /// adversarial task header, reported rather than causing a panic or an
    /// out-of-bounds read.
    InvalidTaskBounds {
        offset: u32,
        len: u32,
        rdram_len: usize,
    },
    /// `create`/`resize`/`present` was called in an order the backend does
    /// not support (e.g. `process_task` before `create`). Carries a short,
    /// backend-supplied reason so this doesn't degenerate into a bare
    /// "backend error" string with no actionable content.
    NotReady(&'static str),
    /// The backend's own internal failure (device lost, FFI call failed,
    /// etc). Adapters map their own detailed error into this with a short
    /// static tag identifying which backend + which operation, so the
    /// variant stays informative without requiring this shared crate to
    /// know every backend's error type.
    Backend {
        backend: &'static str,
        reason: String,
    },
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::UnsupportedUcode { ucode_addr } => {
                write!(f, "unsupported ucode at rdram offset {ucode_addr:#010x}")
            }
            RenderError::InvalidTaskBounds {
                offset,
                len,
                rdram_len,
            } => write!(
                f,
                "task output buffer [{offset:#010x}, +{len}) exceeds rdram length {rdram_len}"
            ),
            RenderError::NotReady(reason) => write!(f, "backend not ready: {reason}"),
            RenderError::Backend { backend, reason } => {
                write!(f, "{backend} backend error: {reason}")
            }
        }
    }
}

impl std::error::Error for RenderError {}

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

    /// Process one RSP gfx task: walk `task`'s display list (rooted at
    /// `task.data_ptr`, per the public libultra manual's `OSTask_t.data_ptr`
    /// field being the display-list start for `M_GFXTASK`) out of `rdram`
    /// and render into the backend's current target. `rdram` is the WHOLE
    /// N64 memory image (matching `RECOMP_FUNC`'s own `uint8_t* rdram`
    /// convention, per `docs/DESIGN.md` section 2) -- the backend reads
    /// vertex/texture/matrix data out of it directly, never through any
    /// runtime API, which is the "never reaches back into runtime state"
    /// invariant made concrete.
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
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError>;

    /// Present the most recently rendered frame (swap to screen, or for a
    /// headless backend, finalize it as retrievable). Distinct from
    /// `process_task` because real hardware's `osViSwapBuffer` (the VI
    /// manager posting a rendered buffer to the display) is a separate
    /// event from RSP task completion -- multiple gfx tasks can render
    /// before one present, matching double/triple-buffering.
    fn present(&mut self) -> Result<(), RenderError>;

    /// The output target changed size (a real window resize, or a harness
    /// reconfiguring a headless target). Infallible by design: a backend
    /// that cannot honor a resize should surface that at the next
    /// `process_task`/`present` call via `RenderError`, not here -- this
    /// keeps window-resize event handling (which callers can't always
    /// gate on a `Result`) simple to wire.
    fn resize(&mut self, w: u32, h: u32);

    /// Which microcode families this backend actually implements. A task
    /// using an unlisted ucode must be rejected by `process_task` with
    /// `RenderError::UnsupportedUcode` (named, not a silent black frame) --
    /// callers are expected to consult this before dispatch too, but
    /// `process_task` is the enforced boundary, not this advisory list.
    fn supported_ucodes(&self) -> &[UcodeId];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal in-crate fake backend, used ONLY to prove the trait object
    /// is dyn-safe and that its contract (create-before-use, unsupported-
    /// ucode trapping) is expressible and testable without pulling in any
    /// real backend crate. Not exported -- `fn64-render-rt64` has its own,
    /// separately tested, real (if partial) backends.
    struct FakeBackend {
        ready: bool,
        ucodes: Vec<UcodeId>,
        frames_presented: u32,
    }

    impl RenderBackend for FakeBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            self.ready = true;
            Ok(())
        }

        fn process_task(
            &mut self,
            rdram: &mut [u8],
            task: &OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            if !self.ready {
                return Err(RenderError::NotReady("create() not called"));
            }
            if !self.ucodes.contains(&UcodeId::F3dex2) {
                return Err(RenderError::UnsupportedUcode {
                    ucode_addr: task.ucode,
                });
            }
            let end = task.output_buff as usize + task.output_buff_size as usize;
            if end > rdram.len() {
                return Err(RenderError::InvalidTaskBounds {
                    offset: task.output_buff,
                    len: task.output_buff_size,
                    rdram_len: rdram.len(),
                });
            }
            Ok(FrameStatus::Complete)
        }

        fn present(&mut self) -> Result<(), RenderError> {
            if !self.ready {
                return Err(RenderError::NotReady("create() not called"));
            }
            self.frames_presented += 1;
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &self.ucodes
        }
    }

    fn fake(ucodes: Vec<UcodeId>) -> FakeBackend {
        FakeBackend {
            ready: false,
            ucodes,
            frames_presented: 0,
        }
    }

    #[test]
    fn is_dyn_safe_and_usable_through_a_trait_object() {
        let mut backend: Box<dyn RenderBackend> = Box::new(fake(vec![UcodeId::F3dex2]));
        backend.create(&RenderConfig::new(320, 240)).unwrap();
        let mut rdram = vec![0u8; 4096];
        let task = OsTask {
            task_type: M_GFXTASK,
            output_buff: 0,
            output_buff_size: 100,
            ..Default::default()
        };
        assert_eq!(
            backend.process_task(&mut rdram, &task, 0).unwrap(),
            FrameStatus::Complete
        );
        backend.present().unwrap();
    }

    #[test]
    fn process_task_before_create_is_not_ready() {
        let mut backend = fake(vec![UcodeId::F3dex2]);
        let mut rdram = vec![0u8; 16];
        let err = backend
            .process_task(&mut rdram, &OsTask::default(), 0)
            .unwrap_err();
        assert!(matches!(err, RenderError::NotReady(_)));
    }

    #[test]
    fn unlisted_ucode_traps_by_name_not_silently() {
        let mut backend = fake(vec![]); // declares NO supported ucodes
        backend.create(&RenderConfig::new(64, 64)).unwrap();
        let mut rdram = vec![0u8; 16];
        let task = OsTask {
            ucode: 0x8000_1234,
            ..Default::default()
        };
        let err = backend.process_task(&mut rdram, &task, 0).unwrap_err();
        match err {
            RenderError::UnsupportedUcode { ucode_addr } => assert_eq!(ucode_addr, 0x8000_1234),
            other => panic!("expected UnsupportedUcode, got {other:?}"),
        }
    }

    #[test]
    fn out_of_bounds_output_buffer_is_a_named_error_not_a_panic() {
        let mut backend = fake(vec![UcodeId::F3dex2]);
        backend.create(&RenderConfig::new(64, 64)).unwrap();
        let mut rdram = vec![0u8; 16];
        let task = OsTask {
            output_buff: 10,
            output_buff_size: 100,
            ..Default::default()
        };
        let err = backend.process_task(&mut rdram, &task, 0).unwrap_err();
        assert!(matches!(err, RenderError::InvalidTaskBounds { .. }));
    }

    #[test]
    fn render_error_display_is_informative() {
        let e = RenderError::UnsupportedUcode {
            ucode_addr: 0x8001_0000,
        };
        assert!(
            format!("{e}").contains("8001_0000".replace('_', "").as_str())
                || format!("{e}").contains("80010000")
        );
    }

    #[test]
    fn ucode_id_other_is_distinct_from_named_variants() {
        assert_ne!(UcodeId::Other(0), UcodeId::F3dex2);
        assert_eq!(UcodeId::Other(7), UcodeId::Other(7));
    }
}
