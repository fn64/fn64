//! `fn64-render-rt64`: the `RenderBackend` adapter crate reserved for RT64
//! (MIT, C++) interop, per `docs/DESIGN.md` section 1 ("the ONLY crate in
//! the workspace permitted to contain C++ or call into RT64's C++ API")
//! and `docs/DECOUPLING.md`'s renderer-seam sequencing step 3 (this crate
//! IS the renamed `fn64-rt64`, per role).
//!
//! ## RT64 feature boundary
//!
//! [`Rt64Backend`] is live when this crate's opt-in `rt64` feature is
//! enabled. Its build script compiles the sibling RT64 checkout's MIT
//! `rt64` target as a static library and links the crate-local C ABI shim;
//! the default build remains pure Rust and keeps [`ReferenceBackend`] as the
//! headless CI oracle. Building without `rt64` makes `Rt64Backend::create`
//! return a named error so a shell can fall back without pretending a GPU
//! backend exists.
//!
//! **What IS real and tested:** [`ReferenceBackend`], a headless, pure-Rust
//! software rasterizer implementing the full `RenderBackend` trait against
//! real, intentionally bounded Fast3D/F3DEX/F3DLX/F3DLX.Rej/F3DEX2-family/
//! L3DEX/L3DEX2 (`gbi.rs`) and S2DEX (`s2dex.rs`) display-list subsets plus a real
//! scanline rasterizer
//! (`raster.rs`). It proves the
//! seam end-to-end: a captured `OsTask` pointing at a display list in
//! `rdram` produces actual non-clear pixels in an output framebuffer,
//! dumpable as a PNG (`png_dump.rs`). See `tests/fixture_replay.rs` for the
//! fixture-replay test and the produced frame.
//!
//! This crate's dependency on `fn64-runtime` is for shared address-space
//! types ONLY (per `docs/DECOUPLING.md`: "for the shared types the gfx
//! task handoff needs to name") -- neither backend here calls back into
//! `fn64_runtime::Executor` or any other runtime state; both only ever see
//! `&[u8]` (rdram) and an `fn64_render::OsTask` value, matching the trait's
//! contract.

#[cfg(test)]
#[path = "../adapter_source_identity.rs"]
mod adapter_source_identity;

pub mod depth;
pub mod extended_gbi;
pub mod gbi;
pub mod png_dump;
pub mod raster;
mod s2dex;
mod vi;
pub use gbi::GeometryWireFamily;
pub use s2dex::S2dexWireFamily;

/// Read a `FN64_*` debug knob, trapping if its retired `OOT_*` name is set.
///
/// These knobs are generic observability, not game-specific state; they were
/// renamed off the `OOT_` prefix so a second game cannot fork them (ROADMAP
/// H2b). An unset var means "feature off", so a bare rename would make an
/// existing `OOT_DUMP_PROJ=1` invocation silently do nothing -- the exact
/// silent shrug AGENTS.md bans. Every read of a renamed knob goes through
/// here so the old spelling stays loud instead of no-op.
#[cfg(not(test))]
pub(crate) fn debug_flag(name: &str) -> bool {
    let legacy = format!("OOT_{}", name.strip_prefix("FN64_").unwrap_or(name));
    assert!(
        std::env::var_os(&legacy).is_none(),
        "{legacy} was renamed to {name}; it is no longer read. Re-run with {name} set."
    );
    std::env::var_os(name).is_some()
}

#[cfg(feature = "rt64")]
mod ffi;

// Named (not `_`-discarded) so the intended dependency-for-shared-types-only
// relationship documented above is visible to `cargo tree`/reviewers, even
// though no runtime type is used by name in this crate yet (see module doc:
// both backends only ever see `&[u8]` + `fn64_render::OsTask`, never an
// `fn64_runtime` type directly). Kept as an explicit workspace edge, not
// removed, per `docs/DECOUPLING.md` step 3's sequencing.
#[allow(unused_imports)]
use fn64_runtime as _;

pub(crate) fn record_render_unsupported(
    operation: &'static str,
    context: &str,
    disposition: fn64_runtime::UnsupportedDisposition,
) {
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Render,
        operation,
        context,
        None,
        disposition,
    );
}

pub(crate) fn render_unsupported_error(
    backend: &'static str,
    operation: &'static str,
    context: impl Into<String>,
) -> RenderError {
    let context = context.into();
    record_render_unsupported(
        operation,
        &context,
        fn64_runtime::UnsupportedDisposition::ReturnedError,
    );
    RenderError::Backend {
        backend,
        reason: context,
    }
}

pub(crate) fn render_unsupported_panic(operation: &'static str, context: impl Into<String>) -> ! {
    let context = context.into();
    record_render_unsupported(
        operation,
        &context,
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!("{context}")
}

#[cfg(test)]
use fn64_render::ViPixelType;
use fn64_render::{
    FrameStatus, NonRdpWrite16, NonRdpWrite16Disposition, OsTask, RenderBackend, RenderConfig,
    RenderEmulatorSettings, RenderEnhancementSettings, RenderError, RenderPolicyApply,
    RenderReplacementPackIdentity, RenderReplacementSettings, RenderRuntimePolicy,
    RenderRuntimeSettings, RenderSettingsApply, UcodeId, ViPresentation,
};
use raster::Framebuffer;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct RdramHiddenSample {
    visible: u16,
    bits: u8,
}

fn read_rdram_hidden_bits(
    hidden: &mut HashMap<u32, RdramHiddenSample>,
    address: u32,
    visible: u16,
) -> u8 {
    if let Some(sample) = hidden.get(&address) {
        if sample.visible == visible {
            return sample.bits & 3;
        }
    }
    // Programming Manual 15.5.6: a non-RDP 16-bit write replicates the
    // visible LSB into both physical hidden bits. A changed visible word is
    // therefore observable evidence that another RDRAM master wrote it.
    record_non_rdp_16bit_write(hidden, address, visible)
}

/// Record a known non-RDP 16-bit write to one physical RDRAM halfword.
///
/// Programming Manual 15.5.6 defines this mutation even when the visible
/// value is unchanged: both hidden bits receive the visible LSB. The renderer
/// calls this from its changed-visible-word fallback. A same-value external
/// store requires the host to provide a write event because `&mut [u8]`
/// alone cannot distinguish that store from no mutation.
fn record_non_rdp_16bit_write(
    hidden: &mut HashMap<u32, RdramHiddenSample>,
    address: u32,
    visible: u16,
) -> u8 {
    let bits = if visible & 1 == 0 { 0 } else { 3 };
    hidden.insert(address, RdramHiddenSample { visible, bits });
    bits
}

fn write_rdram_hidden_bits(
    hidden: &mut HashMap<u32, RdramHiddenSample>,
    address: u32,
    visible: u16,
    bits: u8,
) {
    hidden.insert(
        address,
        RdramHiddenSample {
            visible,
            bits: bits & 3,
        },
    );
}

/// Refresh the CPU-visible halfword paired with already-owned physical hidden
/// bits after an RDP write through a layout that does not consume those bits.
/// I8 and RGBA32 preserve hidden storage, but failing to update this coherence
/// marker would make a later RGBA16 import misclassify the known RDP write as
/// an external non-RDP store and replace the preserved bits from the LSB.
fn refresh_rdp_visible_halfwords_preserving_hidden(
    rdram: &[u8],
    hidden: &mut HashMap<u32, RdramHiddenSample>,
    start: u32,
    byte_len: usize,
) {
    debug_assert!(start.is_multiple_of(2));
    let view = fn64_runtime::RdramView::from_storage(rdram);
    for byte_offset in (0..byte_len).step_by(2) {
        let Ok(byte_offset) = u32::try_from(byte_offset) else {
            break;
        };
        let Some(address) = start.checked_add(byte_offset) else {
            break;
        };
        if address as usize + 2 > view.len() {
            break;
        }
        if let Some(sample) = hidden.get_mut(&address) {
            sample.visible = view.read_u16(fn64_runtime::RdramAddr::from_offset(address));
        }
    }
}
use std::collections::HashMap;
#[cfg(feature = "rt64")]
use std::ffi::CString;
use std::path::{Path, PathBuf};

#[cfg(feature = "rt64")]
use sha2::Digest;

/// A headless software `RenderBackend`: decodes bounded F3DEX2/S2DEX
/// display-list subsets to ordered geometry/image/fill/sync operations and
/// rasterizes them into an off-screen RGBA8888 buffer with explicit RGBA16/32
/// RDRAM target write-back. "Reference" in the sense of "the thing every future real backend
/// (RT64 adapter, wgpu HLE) can be A/B-diffed against for seam-level
/// correctness" -- not a claim of RDP-accurate output (see module doc).
/// Which display-list encoding `process_task` decodes with.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DecodeMode {
    /// The original simple F3D-style reference-fixture encoding
    /// (`gbi::decode_display_list`): raw screen-space `ob` coords,
    /// non-segmented `w1` addresses, `n<<12|v0` vertex packing. This is what
    /// the hand-built fixtures and the `fn64-abi` executor-seam test plant,
    /// so it stays the DEFAULT to keep those working bit-for-bit.
    Simple,
    /// Real F3DEX2 (`gbi::decode_display_list_f3dex2`): segment table,
    /// modelview/projection matrix stack, viewport, nested `G_DL`. Selected
    /// for decoding actual OoT display lists.
    F3dex2,
    /// Public legacy S2DEX or F3DEX_GBI_2 S2DEX2 commands, selected by the
    /// admitted microcode digest's explicit wire-family metadata.
    S2dex,
    /// Bounded raw RDP command DMA. Triangle opcodes are variable-width
    /// edge/coefficient records, not eight-byte RSP display-list commands.
    RawRdp,
}

pub struct ReferenceBackend {
    fb: Option<Framebuffer>,
    /// Last VI scanout image. This is deliberately distinct from `fb`: VI
    /// blanking must not erase the RDP image that becomes visible again when
    /// blanking is disabled at a later V-blank.
    presented_fb: Option<Framebuffer>,
    presentation: ViPresentation,
    /// Persistent RDP color-image register. RDP state survives across OSTask
    /// boundaries; keeping the target beside the surface prevents a later
    /// task from silently falling back to the current VI buffer.
    color_image: Option<gbi::ColorImage>,
    /// Persistent RDP depth-image register, independent of color targets.
    depth_image: Option<gbi::DepthImage>,
    /// Persistent RDP primitive Z/DeltaZ registers.
    primitive_depth: Option<gbi::PrimitiveDepth>,
    /// Persistent RDP command-decode registers and physical TMEM. This is
    /// shared by admitted F3DEX2 HLE tasks and raw DPC submissions; OSTask
    /// boundaries reset RSP state, not the RDP device.
    rdp_decode_state: gbi::RdpDecodeState,
    /// The two non-CPU-visible bits owned by every physical RDRAM halfword the
    /// RDP has touched. Color images interpret them as low coverage bits;
    /// depth images interpret them as low DeltaZ bits. One address-keyed store
    /// preserves real aliasing between overlapping image ranges.
    rdram_hidden_bits: HashMap<u32, RdramHiddenSample>,
    clear_color: [u8; 4],
    noise_seed: u64,
    decode_mode: DecodeMode,
    /// Exact geometry-microcode text images allowed at task entry and after a
    /// `G_LOAD_UCODE`, together with their public command-wire families.
    /// Selecting the decode mode does not admit content.
    f3dex2_ucodes: gbi::F3dex2UcodeCatalog,
    /// Exact S2DEX/S2DEX2-compatible task-entry images and their public wire
    /// families. No F3DEX2 digest or opcode-family guess is inherited.
    s2dex_ucodes: s2dex::UcodeCatalog,
    /// FullSync result of the last successfully committed submission.
    last_dp_full_sync: fn64_render::DpFullSyncStatus,
    /// If set, `process_task` writes the rasterized framebuffer to
    /// `<dir>/<prefix>-NNNN.png` after each task, and logs whether the frame
    /// was non-clear. This is how a harness that MOVED the backend into
    /// `fn64_abi::set_render_backend` (giving up its `&mut` handle, since the
    /// `dyn RenderBackend` trait object is deliberately not `Any`-downcastable
    /// per docs/DECOUPLING.md) still gets the rasterized output back out:
    /// the backend dumps it itself. Bounded by `auto_dump_limit`.
    auto_dump: Option<AutoDump>,
    /// Counts every gfx task this backend processes, independent of
    /// `auto_dump` being configured, so `FN64_GFX_TASK_DUMP` selects the same
    /// task index whether or not PNG auto-dumping is on.
    #[cfg(not(test))]
    diag_task_index: u64,
    /// Backend-owned checkpoint for the one HLE task currently between
    /// committed operation boundaries.
    continuation: Option<ReferenceTaskContinuation>,
    next_continuation_token: u64,
}

struct ReferenceTaskContinuation {
    token: fn64_render::RenderTaskContinuation,
    task: OsTask,
    output_addr: u32,
    decode_mode: DecodeMode,
    operations: Vec<gbi::RenderOp>,
    next_operation: usize,
    active_target: Option<gbi::ColorImage>,
    target_loaded: bool,
    active_depth_image: Option<gbi::DepthImage>,
    active_primitive_depth: Option<gbi::PrimitiveDepth>,
    saw_explicit_target: bool,
    dirty: bool,
    depth_dirty: bool,
    reached_dp_full_sync: bool,
    tri_count: usize,
    persistent_target_was_selected: bool,
}

enum PreparedReferenceTask {
    Ready(ReferenceTaskContinuation),
    NeedsLle([u8; 32]),
}

struct AutoDump {
    dir: std::path::PathBuf,
    prefix: String,
    /// How many gfx tasks have been processed (the PNG index).
    task_index: u64,
    /// Do not write PNGs for tasks before this index. The task counter still
    /// advances, so a long-running harness can capture a bounded late window
    /// without flooding the output directory with boot frames.
    skip_before_task: u64,
    /// How many non-clear PNGs have actually been written.
    written: u64,
    /// Stop dumping after this many non-clear frames (avoid flooding /tmp on
    /// a long boot). `u64::MAX` = unbounded.
    limit: u64,
}

impl ReferenceBackend {
    pub fn new() -> Self {
        ReferenceBackend {
            fb: None,
            presented_fb: None,
            presentation: ViPresentation::default(),
            color_image: None,
            depth_image: None,
            primitive_depth: None,
            rdp_decode_state: gbi::RdpDecodeState::default(),
            rdram_hidden_bits: HashMap::new(),
            clear_color: [0, 0, 0, 255],
            noise_seed: Framebuffer::DEFAULT_NOISE_SEED,
            decode_mode: DecodeMode::Simple,
            f3dex2_ucodes: gbi::F3dex2UcodeCatalog::default(),
            s2dex_ucodes: s2dex::UcodeCatalog::default(),
            last_dp_full_sync: fn64_render::DpFullSyncStatus::Unidentified,
            auto_dump: None,
            #[cfg(not(test))]
            diag_task_index: 0,
            continuation: None,
            next_continuation_token: 1,
        }
    }

    /// Select real F3DEX2 command decoding (matrix stack, segment table,
    /// viewport) instead of the simple reference-fixture encoding. This does
    /// not admit any microcode image: callers must also register every exact
    /// compatible text digest, or the task is replayed through LLE.
    pub fn with_f3dex2(mut self) -> Self {
        self.decode_mode = DecodeMode::F3dex2;
        self
    }

    /// Admit one exact task-entry or self-load target as F3DEX2-compatible.
    /// The digest is SHA-256 over the complete logical 4 KiB text image. This
    /// API carries identity rather than game bytes, so a host can configure
    /// known public variants without placing ROM or ucode content in fn64.
    pub fn with_f3dex2_ucode_sha256(mut self, digest: [u8; 32]) -> Self {
        self.f3dex2_ucodes.admit_sha256(digest);
        self
    }

    /// Admit one exact logical 4 KiB F3DEX2 text image, retaining only its
    /// SHA-256 identity. Primarily useful to deterministic fixtures that
    /// construct synthetic IMEM rather than carrying a precomputed digest.
    pub fn with_f3dex2_ucode_text(mut self, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "F3DEX2 text admission requires one complete 4 KiB IMEM image"
        );
        self.f3dex2_ucodes.admit_text(text);
        self
    }

    /// Admit one exact geometry-microcode digest with an explicit public wire
    /// family. Digest identity, never a colliding opcode, selects the decoder.
    pub fn with_geometry_ucode_sha256(
        mut self,
        family: GeometryWireFamily,
        digest: [u8; 32],
    ) -> Self {
        self.decode_mode = DecodeMode::F3dex2;
        self.f3dex2_ucodes.admit_sha256_for(family, digest);
        self
    }

    /// Admit one exact logical 4 KiB geometry-microcode text image with an
    /// explicit public wire family, retaining only its SHA-256 identity.
    pub fn with_geometry_ucode_text(mut self, family: GeometryWireFamily, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "geometry microcode text admission requires one complete 4 KiB IMEM image"
        );
        self.decode_mode = DecodeMode::F3dex2;
        self.f3dex2_ucodes.admit_text_for(family, text);
        self
    }

    /// Select the content-admitted S2DEX/S2DEX2 object decoder. This does not
    /// admit a text image or guess its wire family.
    pub fn with_s2dex(mut self) -> Self {
        self.decode_mode = DecodeMode::S2dex;
        self
    }

    /// Admit one exact 4 KiB S2DEX2 task-entry text identity.
    ///
    /// This source-compatible method predates the legacy S2DEX decoder and is
    /// deliberately defined as [`S2dexWireFamily::S2dex2`].
    pub fn with_s2dex_ucode_sha256(mut self, digest: [u8; 32]) -> Self {
        self.s2dex_ucodes.admit_sha256(digest);
        self
    }

    /// Admit one exact task-entry identity with an explicit S2DEX wire family.
    pub fn with_s2dex_ucode_sha256_for(
        mut self,
        family: S2dexWireFamily,
        digest: [u8; 32],
    ) -> Self {
        self.s2dex_ucodes.admit_sha256_for(family, digest);
        self
    }

    /// Admit one exact logical 4 KiB S2DEX2 task-entry image, retaining only
    /// its SHA-256 identity. Intended for synthetic fixtures. Use
    /// [`Self::with_s2dex_ucode_text_for`] for legacy S2DEX.
    pub fn with_s2dex_ucode_text(mut self, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "S2DEX text admission requires one complete 4 KiB IMEM image"
        );
        self.s2dex_ucodes.admit_text(text);
        self
    }

    /// Admit one exact logical 4 KiB image with an explicit S2DEX wire family.
    pub fn with_s2dex_ucode_text_for(mut self, family: S2dexWireFamily, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "S2DEX text admission requires one complete 4 KiB IMEM image"
        );
        self.s2dex_ucodes.admit_text_for(family, text);
        self
    }

    /// After each `process_task`, write the rasterized framebuffer to
    /// `<dir>/<prefix>-NNNN.png` (NNNN = the non-clear-frame counter),
    /// stopping after `limit` non-clear frames. This lets a harness recover
    /// the backend's output even after `set_render_backend` has taken
    /// ownership of it. Every dump (and every all-clear skip) is logged so a
    /// blank boot is reported honestly, never faked.
    pub fn with_auto_dump(
        mut self,
        dir: impl Into<std::path::PathBuf>,
        prefix: impl Into<String>,
        limit: u64,
    ) -> Self {
        self.auto_dump = Some(AutoDump {
            dir: dir.into(),
            prefix: prefix.into(),
            task_index: 0,
            skip_before_task: 0,
            written: 0,
            limit,
        });
        self
    }

    /// Start auto-dumping at gfx task index `first_task`.
    ///
    /// Call this after [`Self::with_auto_dump`]. Tasks before the requested
    /// index are still rendered and written back to guest RDRAM; only their
    /// diagnostic PNG output is suppressed.
    pub fn with_auto_dump_skip(mut self, first_task: u64) -> Self {
        self.auto_dump
            .as_mut()
            .expect("with_auto_dump_skip requires with_auto_dump first")
            .skip_before_task = first_task;
        self
    }

    /// Override the clear color a fresh/resized framebuffer starts from.
    /// Exposed mainly so tests can pick a clear color that's unambiguously
    /// distinct from any triangle color in a fixture, making "did geometry
    /// actually render" trivial to assert.
    pub fn with_clear_color(mut self, rgba: [u8; 4]) -> Self {
        self.clear_color = rgba;
        self
    }

    /// Select the reproducible reference stream used for combiner, RGB,
    /// alpha, and alpha-compare noise. This seed controls a host emulation
    /// policy; it is not the RDP's unpublished hardware seed.
    pub fn with_noise_seed(mut self, seed: u64) -> Self {
        self.noise_seed = seed;
        self
    }

    /// The current framebuffer's raw RGBA8888 pixels, for a test/harness to
    /// inspect or dump (`png_dump::write_png`). `None` before `create`.
    pub fn framebuffer(&self) -> Option<&Framebuffer> {
        self.fb.as_ref()
    }

    /// The image produced by the most recent VI presentation boundary.
    /// Unlike [`Self::framebuffer`], this includes VI-level blanking.
    pub fn presented_framebuffer(&self) -> Option<&Framebuffer> {
        self.presented_fb.as_ref()
    }

    fn allocate_continuation_token(&mut self) -> fn64_render::RenderTaskContinuation {
        let value = self.next_continuation_token;
        self.next_continuation_token = self
            .next_continuation_token
            .checked_add(1)
            .expect("reference render continuation token space exhausted");
        fn64_render::RenderTaskContinuation::new(value)
    }

    fn prepare_reference_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
    ) -> Result<PreparedReferenceTask, RenderError> {
        if let Some(pending) = &self.continuation {
            return Err(RenderError::Backend {
                backend: "reference-task-continuation",
                reason: format!(
                    "cannot start a new task while continuation token {} is retained",
                    pending.token.get()
                ),
            });
        }
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
        let (fb_width, fb_height) = self
            .fb
            .as_ref()
            .map(|fb| (fb.width, fb.height))
            .ok_or(RenderError::NotReady("create() not called"))?;

        // The public OSTask field is an end pointer, not a byte count.
        let out_start = (task.output_buff & 0x00FF_FFFF) as usize;
        let out_end = (task.output_buff_size & 0x00FF_FFFF) as usize;
        if task.output_buff_size != 0 && out_end > rdram.len() {
            return Err(RenderError::InvalidTaskBounds {
                offset: task.output_buff,
                len: out_end.saturating_sub(out_start) as u32,
                rdram_len: rdram.len(),
            });
        }

        let persistent_target = self.color_image;
        let persistent_depth_image = self.depth_image;
        let operations = match self.decode_mode {
            DecodeMode::Simple => gbi::decode_display_list(&*rdram, task.data_ptr)?
                .into_iter()
                .map(gbi::RenderOp::Triangle)
                .collect::<Vec<_>>(),
            DecodeMode::F3dex2 => {
                let family = match self
                    .f3dex2_ucodes
                    .require_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))
                {
                    Ok(family) => family,
                    Err(RenderError::RequiresLle { ucode_sha256 }) => {
                        return Ok(PreparedReferenceTask::NeedsLle(ucode_sha256));
                    }
                    Err(error) => return Err(error),
                };
                // HLE decode remains transactional: an unadmitted self-load
                // cannot leave partial RSP, RDRAM, or RDP-decode mutations.
                let mut speculative_rdram = rdram.to_vec();
                let mut speculative_rsp = rsp_memory.clone();
                let mut speculative_rdp = self.rdp_decode_state.clone();
                let operations =
                    match gbi::execute_display_list_geometry_ops_admitted_with_rdp_state(
                        &mut speculative_rdram,
                        &mut speculative_rsp,
                        task.data_ptr,
                        &self.f3dex2_ucodes,
                        &mut speculative_rdp,
                        family,
                    ) {
                        Ok(operations) => operations,
                        Err(RenderError::RequiresLle { ucode_sha256 }) => {
                            return Ok(PreparedReferenceTask::NeedsLle(ucode_sha256));
                        }
                        Err(error) => return Err(error),
                    };
                rdram.copy_from_slice(&speculative_rdram);
                *rsp_memory = speculative_rsp;
                self.rdp_decode_state = speculative_rdp;
                operations
            }
            DecodeMode::S2dex => {
                let family = match self
                    .s2dex_ucodes
                    .require_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))
                {
                    Ok(family) => family,
                    Err(RenderError::RequiresLle { ucode_sha256 }) => {
                        return Ok(PreparedReferenceTask::NeedsLle(ucode_sha256));
                    }
                    Err(error) => return Err(error),
                };
                let mut speculative_rdp = self.rdp_decode_state.clone();
                let operations = s2dex::decode_ops_for_family(
                    &*rdram,
                    task.data_ptr,
                    &mut speculative_rdp,
                    family,
                )?;
                self.rdp_decode_state = speculative_rdp;
                operations
            }
            DecodeMode::RawRdp => gbi::decode_raw_rdp_ops_with_state(
                &*rdram,
                task.data_ptr,
                &mut self.rdp_decode_state,
            )?,
        };
        let tri_count = operations
            .iter()
            .filter(|operation| {
                matches!(
                    operation,
                    gbi::RenderOp::Triangle(_)
                        | gbi::RenderOp::Line(_)
                        | gbi::RenderOp::RawTriangle(_)
                )
            })
            .count();

        #[cfg(not(test))]
        {
            let dump_index = self.diag_task_index;
            self.diag_task_index += 1;
            if let Some(spec) = std::env::var_os("FN64_GFX_TASK_DUMP") {
                let selected = spec.to_string_lossy().split(',').any(|entry| {
                    entry.trim().parse::<u64>().unwrap_or_else(|error| {
                        panic!(
                            "FN64_GFX_TASK_DUMP entry {entry:?} is not a u64 task index: {error}"
                        )
                    }) == dump_index
                });
                if selected {
                    let directory = std::env::var_os("FN64_GFX_TASK_DUMP_DIR")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/fn64-gfx-task-dumps"));
                    std::fs::create_dir_all(&directory).unwrap_or_else(|error| {
                        panic!("failed to create FN64_GFX_TASK_DUMP_DIR {directory:?}: {error}")
                    });
                    let command_trace = gbi::trace_display_list_f3dex2(&*rdram, task.data_ptr);
                    let report = format!(
                        "task_index={dump_index}\noutput_addr={output_addr:#010x}\n\
                         reference_triangle_count={tri_count}\ntask={task:#?}\n{command_trace}",
                    );
                    let path = directory.join(format!("task-{dump_index:04}.txt"));
                    std::fs::write(&path, report).unwrap_or_else(|error| {
                        panic!("failed to write gfx task diagnostic {path:?}: {error}")
                    });
                    eprintln!(
                        "[fn64-render-rt64] dumped gfx task #{dump_index} ({tri_count} reference \
                         triangles) to {path:?}"
                    );
                }
            }
        }

        let mut active_target = persistent_target;
        if self.decode_mode == DecodeMode::Simple && active_target.is_none() && output_addr != 0 {
            active_target = Some(gbi::ColorImage {
                format: gbi::ColorImage::RGBA_FORMAT,
                size: gbi::ColorImage::BITS_16,
                width: u16::try_from(fb_width).expect("reference framebuffer width exceeds u16"),
                address: output_addr,
            });
        }
        let target_loaded = persistent_target.is_some();
        {
            let fb = self.fb.as_mut().expect("framebuffer checked above");
            if self.decode_mode != DecodeMode::Simple {
                if let Some(target) = active_target {
                    validate_reference_color_image(rdram, fb_height, target)?;
                    load_color_image(rdram, target, fb, &mut self.rdram_hidden_bits);
                }
            }
            if let Some(target) = persistent_depth_image {
                load_rdp_depth_image(rdram, target, fb, &mut self.rdram_hidden_bits)?;
            }
        }

        Ok(PreparedReferenceTask::Ready(ReferenceTaskContinuation {
            token: self.allocate_continuation_token(),
            task: *task,
            output_addr,
            decode_mode: self.decode_mode,
            operations,
            next_operation: 0,
            active_target,
            target_loaded,
            active_depth_image: persistent_depth_image,
            active_primitive_depth: self.primitive_depth,
            saw_explicit_target: false,
            dirty: false,
            depth_dirty: false,
            reached_dp_full_sync: false,
            tri_count,
            persistent_target_was_selected: persistent_target.is_some(),
        }))
    }

    fn process_reference_task_chunk(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
        step: fn64_render::RenderTaskStep,
    ) -> Result<fn64_render::RenderTaskChunkStatus, RenderError> {
        let state = match step {
            fn64_render::RenderTaskStep::Start => {
                match self.prepare_reference_task(rdram, rsp_memory, task, output_addr)? {
                    PreparedReferenceTask::Ready(state) => state,
                    PreparedReferenceTask::NeedsLle(ucode_sha256) => {
                        return Ok(fn64_render::RenderTaskChunkStatus::NeedsLle { ucode_sha256 });
                    }
                }
            }
            fn64_render::RenderTaskStep::Resume(token) => {
                let pending = self
                    .continuation
                    .as_ref()
                    .ok_or_else(|| RenderError::Backend {
                        backend: "reference-task-continuation",
                        reason: format!(
                            "continuation token {} is stale or was already consumed",
                            token.get()
                        ),
                    })?;
                if pending.token != token {
                    return Err(RenderError::Backend {
                        backend: "reference-task-continuation",
                        reason: format!(
                            "continuation token {} does not own retained token {}",
                            token.get(),
                            pending.token.get()
                        ),
                    });
                }
                if pending.task != *task || pending.output_addr != output_addr {
                    return Err(RenderError::Backend {
                        backend: "reference-task-continuation",
                        reason: format!(
                            "continuation token {} was resumed with a different task or output target",
                            token.get()
                        ),
                    });
                }
                // Interleaving closed here: chunk N has committed and token T
                // is visible to the scheduler; SIG0 may suspend T before a
                // later host boundary resumes it. Removing T before executing
                // operation N+1 means a duplicate/stale resume can never replay
                // that operation after its first successful consumption.
                let mut state = self
                    .continuation
                    .take()
                    .expect("validated reference continuation disappeared");
                state.token = self.allocate_continuation_token();
                state
            }
        };
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
        self.advance_reference_task_chunk(rdram, state)
    }

    fn advance_reference_task_chunk(
        &mut self,
        rdram: &mut [u8],
        mut state: ReferenceTaskContinuation,
    ) -> Result<fn64_render::RenderTaskChunkStatus, RenderError> {
        if state.next_operation < state.operations.len() {
            let operation = state.operations[state.next_operation].clone();
            state.next_operation += 1;
            self.execute_reference_operation(rdram, &mut state, &operation)?;
            state.reached_dp_full_sync |= matches!(operation, gbi::RenderOp::FullSync);
            self.commit_reference_boundary(rdram, &state)?;
        }

        let dp_full_sync = if state.reached_dp_full_sync {
            fn64_render::DpFullSyncStatus::Reached
        } else {
            fn64_render::DpFullSyncStatus::NotReached
        };
        if state.next_operation < state.operations.len() {
            let token = state.token;
            assert!(
                self.continuation.replace(state).is_none(),
                "reference continuation ownership became occupied during one chunk"
            );
            self.last_dp_full_sync = dp_full_sync;
            Ok(fn64_render::RenderTaskChunkStatus::Continue(token))
        } else {
            self.finish_reference_task(rdram, state)?;
            self.last_dp_full_sync = dp_full_sync;
            Ok(fn64_render::RenderTaskChunkStatus::Complete)
        }
    }

    fn execute_reference_operation(
        &mut self,
        rdram: &mut [u8],
        state: &mut ReferenceTaskContinuation,
        operation: &gbi::RenderOp,
    ) -> Result<(), RenderError> {
        let fb = self
            .fb
            .as_mut()
            .ok_or(RenderError::NotReady("create() not called"))?;
        #[cfg(not(test))]
        let no_depth = crate::debug_flag("FN64_NO_DEPTH");
        #[cfg(test)]
        let no_depth = false;

        match operation {
            gbi::RenderOp::Triangle(triangle) => {
                require_reference_color_target(
                    state.decode_mode,
                    state.active_target,
                    "F3DEX2 triangle",
                )?;
                if !no_depth
                    && (triangle.other_mode.depth_compare_enabled()
                        || triangle.other_mode.depth_update_enabled())
                    && state.active_depth_image.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "F3DEX2 triangle enables Z compare/update without a selected G_SETZIMG target"
                            .to_string(),
                    });
                }
                if !no_depth
                    && (triangle.other_mode.depth_compare_enabled()
                        || triangle.other_mode.depth_update_enabled())
                    && triangle.other_mode.primitive_depth_source()
                    && state.active_primitive_depth.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "F3DEX2 triangle selects primitive Z without prior G_SETPRIMDEPTH"
                            .to_string(),
                    });
                }
                fb.set_primitive_depth(state.active_primitive_depth);
                if state.decode_mode == DecodeMode::Simple {
                    fb.draw_triangle(triangle);
                } else if no_depth {
                    fb.draw_triangle_no_depth_culled(triangle, triangle.cull);
                } else {
                    fb.draw_triangle_culled(triangle, triangle.cull);
                }
                state.depth_dirty |= !no_depth && triangle.other_mode.depth_update_enabled();
                state.dirty = true;
            }
            gbi::RenderOp::Line(line) => {
                require_reference_color_target(state.decode_mode, state.active_target, "G_LINE3D")?;
                if !no_depth
                    && line.other_mode.depth_compare_enabled()
                    && state.active_depth_image.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "G_LINE3D enables Z compare without a selected G_SETZIMG target"
                            .to_string(),
                    });
                }
                if !no_depth
                    && line.other_mode.depth_compare_enabled()
                    && line.other_mode.primitive_depth_source()
                    && state.active_primitive_depth.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "G_LINE3D selects primitive Z without prior G_SETPRIMDEPTH"
                            .to_string(),
                    });
                }
                fb.set_primitive_depth(state.active_primitive_depth);
                if no_depth {
                    fb.draw_line_no_depth(line);
                } else {
                    fb.draw_line(line);
                }
                state.dirty = true;
            }
            gbi::RenderOp::RawTriangle(triangle) => {
                require_reference_color_target(
                    state.decode_mode,
                    state.active_target,
                    "raw RDP triangle",
                )?;
                if !no_depth
                    && (triangle.other_mode.depth_compare_enabled()
                        || triangle.other_mode.depth_update_enabled())
                    && state.active_depth_image.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "raw RDP triangle enables Z compare/update without a selected G_SETZIMG target"
                            .to_string(),
                    });
                }
                if !no_depth
                    && (triangle.other_mode.depth_compare_enabled()
                        || triangle.other_mode.depth_update_enabled())
                    && ((triangle.other_mode.primitive_depth_source()
                        && state.active_primitive_depth.is_none())
                        || (!triangle.other_mode.primitive_depth_source() && triangle.z.is_none()))
                {
                    let reason = if triangle.other_mode.primitive_depth_source() {
                        "raw RDP triangle selects primitive Z without prior G_SETPRIMDEPTH"
                    } else {
                        "raw RDP triangle enables pixel Z compare/update without carrying Z coefficients"
                    };
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: reason.to_string(),
                    });
                }
                fb.set_primitive_depth(state.active_primitive_depth);
                if no_depth {
                    fb.draw_raw_rdp_triangle_no_depth(triangle);
                } else {
                    fb.draw_raw_rdp_triangle(triangle);
                }
                state.depth_dirty |= !no_depth && triangle.other_mode.depth_update_enabled();
                state.dirty = true;
            }
            gbi::RenderOp::SetColorImage(target) => {
                validate_reference_color_image(rdram, fb.height, *target)?;
                let changes_target = state.active_target != Some(*target) || !state.target_loaded;
                if changes_target {
                    if let Some(previous) = state.active_target {
                        let transition = previous.transition_to(*target);
                        debug_assert_eq!(transition.to, target.layout().unwrap());
                    }
                    if state.depth_dirty {
                        if let Some(depth_target) = state.active_depth_image {
                            commit_rdp_depth_image(
                                rdram,
                                depth_target,
                                fb,
                                &mut self.rdram_hidden_bits,
                            )?;
                        }
                        state.depth_dirty = false;
                    }
                    if state.dirty {
                        if let Some(previous) = state.active_target {
                            commit_color_image(rdram, previous, fb, &mut self.rdram_hidden_bits);
                        }
                    }
                    load_color_image(rdram, *target, fb, &mut self.rdram_hidden_bits);
                    if let Some(depth_target) = state.active_depth_image {
                        load_rdp_depth_image(rdram, depth_target, fb, &mut self.rdram_hidden_bits)?;
                    }
                    state.dirty = false;
                }
                state.active_target = Some(*target);
                state.target_loaded = true;
                state.saw_explicit_target = true;
            }
            gbi::RenderOp::SetDepthImage(target) => {
                if state.active_depth_image != Some(*target) {
                    if state.depth_dirty {
                        if let Some(previous) = state.active_depth_image {
                            commit_rdp_depth_image(
                                rdram,
                                previous,
                                fb,
                                &mut self.rdram_hidden_bits,
                            )?;
                        }
                        state.depth_dirty = false;
                    }
                    load_rdp_depth_image(rdram, *target, fb, &mut self.rdram_hidden_bits)?;
                    state.active_depth_image = Some(*target);
                }
            }
            gbi::RenderOp::SetPrimitiveDepth(primitive_depth) => {
                state.active_primitive_depth = Some(*primitive_depth);
                fb.set_primitive_depth(state.active_primitive_depth);
            }
            gbi::RenderOp::FillRectangle(rectangle) => {
                require_reference_color_target(
                    state.decode_mode,
                    state.active_target,
                    "G_FILLRECT",
                )?;
                validate_fill_rectangle(rectangle)?;
                if (rectangle.other_mode.depth_compare_enabled()
                    || rectangle.other_mode.depth_update_enabled())
                    && state.active_primitive_depth.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason:
                            "combined G_FILLRECT selects primitive Z without prior G_SETPRIMDEPTH"
                                .into(),
                    });
                }
                if (rectangle.other_mode.depth_compare_enabled()
                    || rectangle.other_mode.depth_update_enabled())
                    && state.active_depth_image.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "combined G_FILLRECT enables depth without a G_SETZIMG target"
                            .into(),
                    });
                }
                let target = state.active_target.unwrap_or(gbi::ColorImage {
                    format: gbi::ColorImage::RGBA_FORMAT,
                    size: gbi::ColorImage::BITS_16,
                    width: u16::try_from(fb.width)
                        .expect("reference framebuffer width exceeds u16"),
                    address: 0,
                });
                fb.draw_fill_rectangle(rectangle, target);
                if rectangle.cycle_type == gbi::CycleType::Fill
                    && state.active_target.map(|target| target.address)
                        == state.active_depth_image.map(|target| target.address)
                {
                    fb.clear_depth_rectangle(rectangle);
                    state.depth_dirty = true;
                } else if rectangle.other_mode.depth_update_enabled() {
                    state.depth_dirty = true;
                }
                state.dirty = true;
            }
            gbi::RenderOp::TextureRectangle(rectangle) => {
                require_reference_color_target(
                    state.decode_mode,
                    state.active_target,
                    texture_rectangle_name(rectangle),
                )?;
                validate_texture_rectangle(rectangle, state.active_target)?;
                if (rectangle.other_mode.depth_compare_enabled()
                    || rectangle.other_mode.depth_update_enabled())
                    && state.active_primitive_depth.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: format!(
                            "{} selects primitive Z without prior G_SETPRIMDEPTH",
                            texture_rectangle_name(rectangle)
                        ),
                    });
                }
                if (rectangle.other_mode.depth_compare_enabled()
                    || rectangle.other_mode.depth_update_enabled())
                    && state.active_depth_image.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: format!(
                            "{} enables Z compare/update without a selected G_SETZIMG target",
                            texture_rectangle_name(rectangle)
                        ),
                    });
                }
                fb.set_primitive_depth(state.active_primitive_depth);
                match rectangle.other_mode.cycle_type() {
                    gbi::CycleType::Copy => fb.draw_copy_texture_rectangle(rectangle),
                    gbi::CycleType::OneCycle | gbi::CycleType::TwoCycle => {
                        fb.draw_texture_rectangle(rectangle)
                    }
                    gbi::CycleType::Fill => {
                        unreachable!("fill-cycle texture rectangle passed reference validation")
                    }
                }
                state.depth_dirty |= rectangle.other_mode.depth_update_enabled();
                state.dirty = true;
            }
            gbi::RenderOp::FullSync => {
                if state.dirty {
                    if let Some(target) = state.active_target {
                        commit_color_image(rdram, target, fb, &mut self.rdram_hidden_bits);
                    }
                    state.dirty = false;
                }
                if state.depth_dirty {
                    if let Some(target) = state.active_depth_image {
                        commit_rdp_depth_image(rdram, target, fb, &mut self.rdram_hidden_bits)?;
                    }
                    state.depth_dirty = false;
                }
            }
        }
        Ok(())
    }

    fn commit_reference_boundary(
        &mut self,
        rdram: &mut [u8],
        state: &ReferenceTaskContinuation,
    ) -> Result<(), RenderError> {
        let fb = self
            .fb
            .as_ref()
            .ok_or(RenderError::NotReady("create() not called"))?;
        if state.dirty {
            if let Some(target) = state.active_target {
                commit_color_image(rdram, target, fb, &mut self.rdram_hidden_bits);
            }
        }
        if state.depth_dirty {
            if let Some(target) = state.active_depth_image {
                commit_rdp_depth_image(rdram, target, fb, &mut self.rdram_hidden_bits)?;
            }
        }
        Ok(())
    }

    fn finish_reference_task(
        &mut self,
        rdram: &mut [u8],
        state: ReferenceTaskContinuation,
    ) -> Result<(), RenderError> {
        self.commit_reference_boundary(rdram, &state)?;
        if state.saw_explicit_target || state.persistent_target_was_selected {
            self.color_image = state.active_target;
        }
        self.depth_image = state.active_depth_image;
        self.primitive_depth = state.active_primitive_depth;

        #[cfg(not(test))]
        if matches!(state.decode_mode, DecodeMode::F3dex2 | DecodeMode::S2dex) {
            raster::zstat::summary();
        }

        if let Some(dump) = self.auto_dump.as_mut() {
            let fb = self
                .fb
                .as_ref()
                .ok_or(RenderError::NotReady("create() not called"))?;
            let idx = dump.task_index;
            dump.task_index += 1;
            if idx >= dump.skip_before_task {
                let [cr, cg, cb, ca] = self.clear_color;
                let non_clear = fb.has_non_uniform_content(cr, cg, cb, ca);
                if !non_clear {
                    eprintln!(
                        "[fn64-render-rt64] gfx task #{idx}: decoded {} triangle(s); \
                         framebuffer is UNIFORM clear -- reported blank, not dumped.",
                        state.tri_count
                    );
                } else if dump.written >= dump.limit {
                    eprintln!(
                        "[fn64-render-rt64] gfx task #{idx}: non-clear ({} tris) but \
                         auto-dump limit ({}) reached -- not writing another PNG.",
                        state.tri_count, dump.limit
                    );
                } else {
                    let _ = std::fs::create_dir_all(&dump.dir);
                    let path = dump
                        .dir
                        .join(format!("{}-{:04}.png", dump.prefix, dump.written));
                    match png_dump::write_png(&path, fb.width, fb.height, &fb.pixels) {
                        Ok(()) => {
                            dump.written += 1;
                            eprintln!(
                                "[fn64-render-rt64] gfx task #{idx}: NON-CLEAR ({} tris) \
                                 -- dumped {}",
                                state.tri_count,
                                path.display()
                            );
                        }
                        Err(error) => eprintln!(
                            "[fn64-render-rt64] gfx task #{idx}: failed to write {}: {error}",
                            path.display()
                        ),
                    }
                }
            }
        }
        Ok(())
    }
}

impl Default for ReferenceBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderBackend for ReferenceBackend {
    fn release_environment(&self) -> fn64_render::RenderBackendEvidence {
        fn64_render::RenderBackendEvidence::Reference
    }

    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError> {
        let mut fb = Framebuffer::new(cfg.width, cfg.height);
        fb.set_noise_seed(self.noise_seed);
        let [r, g, b, a] = self.clear_color;
        fb.clear(r, g, b, a);
        self.presented_fb = Some(fb.clone());
        self.presentation = ViPresentation::default();
        self.fb = Some(fb);
        self.color_image = None;
        self.depth_image = None;
        self.primitive_depth = None;
        self.rdp_decode_state = gbi::RdpDecodeState::default();
        self.rdram_hidden_bits.clear();
        self.continuation = None;
        self.next_continuation_token = 1;
        Ok(())
    }

    fn observe_non_rdp_write16(&mut self, write: NonRdpWrite16) -> NonRdpWrite16Disposition {
        let address = write.logical_offset().offset();
        if self.rdram_hidden_bits.contains_key(&address) {
            record_non_rdp_16bit_write(&mut self.rdram_hidden_bits, address, write.value());
            NonRdpWrite16Disposition::AppliedHiddenSidecar
        } else {
            NonRdpWrite16Disposition::NoRustHiddenSidecar
        }
    }

    fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        let mut status = self.process_reference_task_chunk(
            rdram,
            rsp_memory,
            task,
            output_addr,
            fn64_render::RenderTaskStep::Start,
        )?;
        loop {
            match status {
                fn64_render::RenderTaskChunkStatus::Complete => {
                    return Ok(FrameStatus::Complete);
                }
                fn64_render::RenderTaskChunkStatus::Continue(token) => {
                    status = self.process_reference_task_chunk(
                        rdram,
                        rsp_memory,
                        task,
                        output_addr,
                        fn64_render::RenderTaskStep::Resume(token),
                    )?;
                }
                fn64_render::RenderTaskChunkStatus::Yielded => {
                    return Ok(FrameStatus::Yielded);
                }
                fn64_render::RenderTaskChunkStatus::NeedsLle { ucode_sha256 } => {
                    return Ok(FrameStatus::NeedsLle { ucode_sha256 });
                }
            }
        }
    }

    fn process_task_chunk(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
        step: fn64_render::RenderTaskStep,
    ) -> Result<fn64_render::RenderTaskChunkStatus, RenderError> {
        self.process_reference_task_chunk(rdram, rsp_memory, task, output_addr, step)
    }

    fn process_rdp_commands(
        &mut self,
        rdram: &mut [u8],
        start: u32,
        end: u32,
        _output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        gbi::validate_raw_rdp_command_range(rdram, start, end)?;
        let terminated_len = (end as usize)
            .checked_add(8)
            .ok_or_else(|| RenderError::Backend {
                backend: "reference",
                reason: "raw RDP terminator address overflow".to_string(),
            })?;
        let mut image = rdram.to_vec();
        image.resize(terminated_len.max(image.len()), 0);
        image[end as usize..end as usize + 4].copy_from_slice(&0xdf00_0000u32.to_ne_bytes());
        image[end as usize + 4..end as usize + 8].copy_from_slice(&0u32.to_ne_bytes());

        let previous_mode = self.decode_mode;
        self.decode_mode = DecodeMode::RawRdp;
        let result = self.process_task(
            &mut image,
            &mut fn64_runtime::RspMemory::new(),
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: start,
                ..OsTask::default()
            },
            0,
        );
        self.decode_mode = previous_mode;
        if result.is_ok() {
            rdram.copy_from_slice(&image[..rdram.len()]);
        }
        result
    }

    fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
        self.last_dp_full_sync
    }

    fn task_chunking(&self) -> fn64_render::RenderTaskChunking {
        fn64_render::RenderTaskChunking::Resumable
    }

    fn present(&mut self, vi: ViPresentation) -> Result<(), RenderError> {
        let source = self
            .fb
            .as_ref()
            .ok_or(RenderError::NotReady("create() not called"))?;
        self.presented_fb = Some(vi::scanout(source, vi)?);
        self.presentation = vi;
        Ok(())
    }

    fn resize(&mut self, w: u32, h: u32) {
        assert!(
            self.continuation.is_none(),
            "ReferenceBackend::resize cannot replace framebuffer storage while a render continuation is retained"
        );
        let clear_color = self.clear_color;
        if let Some(fb) = &mut self.fb {
            let mut new_fb = fb.resized(w, h);
            new_fb.clear(
                clear_color[0],
                clear_color[1],
                clear_color[2],
                clear_color[3],
            );
            *fb = new_fb;
        }
        if let Some(fb) = &self.fb {
            // `resize` is infallible by trait contract. If the new dimensions
            // cannot support the retained VI effect, leave no fabricated
            // scanout; the next `present` reports the named error.
            self.presented_fb = vi::scanout(fb, self.presentation).ok();
        }
    }

    fn supported_ucodes(&self) -> &[UcodeId] {
        match self.decode_mode {
            DecodeMode::S2dex => self.s2dex_ucodes.supported_ucodes(),
            DecodeMode::F3dex2 => self.f3dex2_ucodes.supported_ucodes(),
            DecodeMode::Simple | DecodeMode::RawRdp => gbi::SUPPORTED,
        }
    }
}

fn validate_reference_color_image(
    rdram: &[u8],
    height: u32,
    target: gbi::ColorImage,
) -> Result<(), RenderError> {
    let Some(layout) = target.layout() else {
        return Err(render_unsupported_error(
            "reference",
            "render.rdp.color-image-layout",
            format!(
                "G_SETCIMG format={} size={} is unsupported; reference execution requires 8-bit intensity, RGBA16, or RGBA32",
                target.format, target.size
            ),
        ));
    };
    let bytes_per_pixel = layout.bytes_per_pixel();
    if target.width == 0 {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: "G_SETCIMG decoded a zero-width color image".to_string(),
        });
    }
    if !target.address.is_multiple_of(8) {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETCIMG {} base {:#010x} is not 64-bit aligned",
                layout.name(),
                target.address,
            ),
        });
    }
    let byte_len = usize::from(target.width)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETCIMG dimensions overflow host address space".to_string(),
        })?;
    let end = (target.address as usize)
        .checked_add(byte_len)
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETCIMG address range overflows host address space".to_string(),
        })?;
    if end > rdram.len() {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETCIMG {} target [{:#010x}, {end:#010x}) exceeds RDRAM length {}",
                layout.name(),
                target.address,
                rdram.len()
            ),
        });
    }
    Ok(())
}

fn require_reference_color_target(
    decode_mode: DecodeMode,
    target: Option<gbi::ColorImage>,
    operation: &str,
) -> Result<(), RenderError> {
    if decode_mode != DecodeMode::Simple && target.is_none() {
        return Err(render_unsupported_error(
            "reference",
            "render.rdp.color-target-state",
            format!(
                "{operation} has no persistent G_SETCIMG color target; VI/output_addr state is not an RDP color-image substitute"
            ),
        ));
    }
    Ok(())
}

fn validate_texture_rectangle(
    rectangle: &gbi::TextureRectangle,
    target: Option<gbi::ColorImage>,
) -> Result<(), RenderError> {
    match rectangle.other_mode.cycle_type() {
        gbi::CycleType::Copy => validate_copy_texture_rectangle(rectangle, target),
        gbi::CycleType::OneCycle | gbi::CycleType::TwoCycle => {
            validate_combined_texture_rectangle(rectangle)
        }
        gbi::CycleType::Fill => Err(render_unsupported_error(
            "reference",
            "render.rdp.texture-rectangle-cycle",
            format!(
                "{} in Fill cycle is invalid; fill cycle bypasses texture sampling",
                texture_rectangle_name(rectangle)
            ),
        )),
    }
}

fn texture_rectangle_name(rectangle: &gbi::TextureRectangle) -> &'static str {
    if rectangle.flip {
        "G_TEXRECTFLIP"
    } else {
        "G_TEXRECT"
    }
}

fn validate_alpha_compare(mode: gbi::AlphaCompare, primitive: &str) -> Result<(), RenderError> {
    match mode {
        gbi::AlphaCompare::None | gbi::AlphaCompare::Threshold | gbi::AlphaCompare::Dither => {
            Ok(())
        }
        gbi::AlphaCompare::Reserved => Err(render_unsupported_error(
            "reference",
            "render.rdp.alpha-compare",
            format!("{primitive} uses reserved alpha-compare mode 2"),
        )),
    }
}

fn validate_copy_texture_rectangle(
    rectangle: &gbi::TextureRectangle,
    target: Option<gbi::ColorImage>,
) -> Result<(), RenderError> {
    let reject = |reason: String| RenderError::Backend {
        backend: "reference",
        reason,
    };
    debug_assert_eq!(rectangle.other_mode.cycle_type(), gbi::CycleType::Copy);
    if rectangle.other_mode.depth_compare_enabled() || rectangle.other_mode.depth_update_enabled() {
        return Err(reject(format!(
            "{} enables depth in Copy cycle, which bypasses the blender",
            texture_rectangle_name(rectangle)
        )));
    }
    if rectangle.dsdx != 4 << 10 {
        return Err(reject(format!(
            "{} copy dsdx={} violates the public copy-mode 4<<10 step",
            texture_rectangle_name(rectangle),
            rectangle.dsdx
        )));
    }
    validate_alpha_compare(
        rectangle.other_mode.alpha_compare(),
        texture_rectangle_name(rectangle),
    )?;
    let texture = rectangle.texture.as_ref().ok_or_else(|| {
        reject(format!(
            "{} references tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
            texture_rectangle_name(rectangle),
            rectangle.tile
        ))
    })?;
    let rgba16 =
        texture.format == gbi::ColorImage::RGBA_FORMAT && texture.size == gbi::ColorImage::BITS_16;
    let direct_ci8 = texture.format == gbi::ColorImage::CI_FORMAT
        && texture.size == gbi::ColorImage::BITS_8
        && rectangle.other_mode.texture_lut() == 0;
    if !rgba16 && !direct_ci8 {
        return Err(render_unsupported_error(
            "reference",
            "render.rdp.copy-source-layout",
            format!(
                "{} copy source format={} size={} LUT={} is unsupported; expected RGBA16 or non-dereferenced CI8",
                texture_rectangle_name(rectangle),
                texture.format,
                texture.size,
                rectangle.other_mode.texture_lut()
            ),
        ));
    }
    if let Some(target) = target {
        let matching_target = matches!(
            (rgba16, direct_ci8, target.layout()),
            (true, false, Some(gbi::ColorImageLayout::Rgba16))
                | (false, true, Some(gbi::ColorImageLayout::Index8))
        );
        if !matching_target {
            return Err(reject(format!(
                "{} copy source format={} size={} does not match color target format={} size={}",
                texture_rectangle_name(rectangle),
                texture.format,
                texture.size,
                target.format,
                target.size
            )));
        }
    }
    if let Some(scissor) = rectangle.scissor {
        let multiple_of_four = |edge: f32| edge.fract() == 0.0 && (edge as i32).rem_euclid(4) == 0;
        if ![scissor.ulx, scissor.uly, scissor.lrx, scissor.lry]
            .into_iter()
            .all(multiple_of_four)
        {
            return Err(reject(format!(
                "{} copy scissor ({}, {})..({}, {}) is not aligned to the documented four-pixel boundary",
                texture_rectangle_name(rectangle),
                scissor.ulx,
                scissor.uly,
                scissor.lrx,
                scissor.lry
            )));
        }
    }
    Ok(())
}

fn validate_combined_texture_rectangle(
    rectangle: &gbi::TextureRectangle,
) -> Result<(), RenderError> {
    let reject = |reason: String| RenderError::Backend {
        backend: "reference",
        reason,
    };
    let name = texture_rectangle_name(rectangle);
    let mode = rectangle.other_mode;
    validate_alpha_compare(mode.alpha_compare(), name)?;
    if mode.texture_filter() == gbi::TextureFilter::Reserved {
        return Err(reject(format!(
            "{name} uses reserved texture-filter mode 1"
        )));
    }
    if (mode.depth_compare_enabled() || mode.depth_update_enabled())
        && !mode.primitive_depth_source()
    {
        return Err(reject(format!(
            "{name} requests depth compare/update with pixel Z, but rectangles require G_ZS_PRIM"
        )));
    }
    if !matches!(mode.texture_convert(), 0 | 5 | 6) {
        return Err(reject(format!(
            "{name} uses reserved texture-convert mode {}",
            mode.texture_convert()
        )));
    }
    if mode.texture_detail() == 3 {
        return Err(reject(format!(
            "{name} selects reserved texture-detail mode 3"
        )));
    }
    rectangle.texture.as_ref().ok_or_else(|| {
        reject(format!(
            "{name} references tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
            rectangle.tile
        ))
    })?;

    let cycle_count = match mode.cycle_type() {
        gbi::CycleType::OneCycle => 1,
        gbi::CycleType::TwoCycle => 2,
        _ => unreachable!("combined rectangle validator called for bypass cycle"),
    };
    for (cycle_index, cycle) in rectangle
        .combiner
        .mode
        .cycles
        .iter()
        .take(cycle_count)
        .enumerate()
    {
        for source in cycle.rgb {
            validate_rectangle_color_source(rectangle, cycle_index, source)?;
        }
        for source in cycle.alpha {
            validate_rectangle_alpha_source(rectangle, cycle_index, source)?;
        }
    }
    if rectangle
        .blender
        .cycles
        .iter()
        .take(usize::from(rectangle.blender.cycle_count))
        .any(|cycle| cycle.a == gbi::BlendAlphaInput::Shade)
    {
        return Err(reject(format!(
            "{name} blender selects SHADE alpha, but rectangle commands carry no shade attributes"
        )));
    }
    Ok(())
}

fn validate_fill_rectangle(rectangle: &gbi::FillRectangle) -> Result<(), RenderError> {
    use gbi::{AlphaSource, ColorSource, CycleType};
    let reject = |reason: String| RenderError::Backend {
        backend: "reference",
        reason,
    };
    match rectangle.cycle_type {
        CycleType::Fill => return Ok(()),
        CycleType::Copy => {
            return Err(render_unsupported_error(
                "reference",
                "render.rdp.fill-rectangle-cycle",
                "G_FILLRECT in copy cycle is not implemented",
            ));
        }
        CycleType::OneCycle | CycleType::TwoCycle => {}
    }
    validate_alpha_compare(rectangle.other_mode.alpha_compare(), "combined G_FILLRECT")?;
    if (rectangle.other_mode.depth_compare_enabled() || rectangle.other_mode.depth_update_enabled())
        && !rectangle.other_mode.primitive_depth_source()
    {
        return Err(reject(
            "combined G_FILLRECT requests depth compare/update with pixel Z, but rectangles require G_ZS_PRIM"
                .into(),
        ));
    }

    let cycle_count = match rectangle.cycle_type {
        CycleType::OneCycle => 1,
        CycleType::TwoCycle => 2,
        _ => unreachable!(),
    };
    for (cycle_index, cycle) in rectangle
        .combiner
        .mode
        .cycles
        .iter()
        .take(cycle_count)
        .enumerate()
    {
        for source in cycle.rgb {
            let reason = match source {
                ColorSource::Combined | ColorSource::CombinedAlpha if cycle_index == 0 => {
                    Some("selects COMBINED before a first-cycle result exists")
                }
                ColorSource::Texel0
                | ColorSource::Texel1
                | ColorSource::Texel0Alpha
                | ColorSource::Texel1Alpha
                | ColorSource::LodFraction => {
                    Some("selects texture state, but G_FILLRECT carries no texture coordinates")
                }
                ColorSource::Shade | ColorSource::ShadeAlpha => {
                    Some("selects SHADE, but G_FILLRECT carries no shade attributes")
                }
                _ => None,
            };
            if let Some(reason) = reason {
                return Err(reject(format!(
                    "combined G_FILLRECT combiner cycle {} {reason}",
                    cycle_index + 1
                )));
            }
        }
        for source in cycle.alpha {
            let reason = match source {
                AlphaSource::Combined if cycle_index == 0 => {
                    Some("selects COMBINED before a first-cycle result exists")
                }
                AlphaSource::Texel0 | AlphaSource::Texel1 | AlphaSource::LodFraction => {
                    Some("selects texture state, but G_FILLRECT carries no texture coordinates")
                }
                AlphaSource::Shade => {
                    Some("selects SHADE, but G_FILLRECT carries no shade attributes")
                }
                _ => None,
            };
            if let Some(reason) = reason {
                return Err(reject(format!(
                    "combined G_FILLRECT alpha combiner cycle {} {reason}",
                    cycle_index + 1
                )));
            }
        }
    }
    if rectangle
        .blender
        .cycles
        .iter()
        .take(usize::from(rectangle.blender.cycle_count))
        .any(|cycle| cycle.a == gbi::BlendAlphaInput::Shade)
    {
        return Err(reject(
            "combined G_FILLRECT blender selects SHADE alpha, but the command carries no shade attributes"
                .into(),
        ));
    }
    Ok(())
}

fn validate_rectangle_color_source(
    rectangle: &gbi::TextureRectangle,
    cycle_index: usize,
    source: gbi::ColorSource,
) -> Result<(), RenderError> {
    use gbi::ColorSource;
    let name = texture_rectangle_name(rectangle);
    let unsupported = |reason: &str| {
        render_unsupported_error(
            "reference",
            "render.rdp.rectangle-color-source",
            format!("{name} combiner cycle {} {reason}", cycle_index + 1),
        )
    };
    match source {
        ColorSource::Combined | ColorSource::CombinedAlpha if cycle_index == 0 => Err(unsupported(
            "selects COMBINED before a first-cycle result exists",
        )),
        ColorSource::Texel1 | ColorSource::Texel1Alpha
            if rectangle.texture1.is_none() && !rectangle.other_mode.texture_lod() =>
        {
            Err(unsupported("selects TEXEL1 without a decoded tile+1 image"))
        }
        ColorSource::Shade | ColorSource::ShadeAlpha => Err(unsupported(
            "selects SHADE, but rectangle commands carry no shade attributes",
        )),
        _ => Ok(()),
    }
}

fn validate_rectangle_alpha_source(
    rectangle: &gbi::TextureRectangle,
    cycle_index: usize,
    source: gbi::AlphaSource,
) -> Result<(), RenderError> {
    use gbi::AlphaSource;
    let name = texture_rectangle_name(rectangle);
    let unsupported = |reason: &str| {
        render_unsupported_error(
            "reference",
            "render.rdp.rectangle-alpha-source",
            format!("{name} alpha combiner cycle {} {reason}", cycle_index + 1),
        )
    };
    match source {
        AlphaSource::Combined if cycle_index == 0 => Err(unsupported(
            "selects COMBINED before a first-cycle result exists",
        )),
        AlphaSource::Texel1
            if rectangle.texture1.is_none() && !rectangle.other_mode.texture_lod() =>
        {
            Err(unsupported("selects TEXEL1 without a decoded tile+1 image"))
        }
        AlphaSource::Shade => Err(unsupported(
            "selects SHADE, but rectangle commands carry no shade attributes",
        )),
        _ => Ok(()),
    }
}

/// Load an RGBA16 color image into the software surface before ordered work
/// continues on that target. Depth is deliberately not reset: the RDP depth
/// image is independent of color-image switches and persists across tasks.
fn load_rgba5551_framebuffer(
    rdram: &[u8],
    target: gbi::ColorImage,
    fb: &mut Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) {
    if fb.width != u32::from(target.width) {
        *fb = fb.resized(u32::from(target.width), fb.height);
    }
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..(fb.width * fb.height) as usize {
        let offset = u32::try_from(index * 2).expect("color-image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("color-image logical address overflow");
        let pixel = view.read_u16(address);
        let hidden = read_rdram_hidden_bits(hidden_bits, address.offset(), pixel);
        let stored_coverage = (((pixel & 1) as u8) << 2) | hidden;
        let expand = |value: u16| -> u8 {
            let value = value as u8;
            (value << 3) | (value >> 2)
        };
        let dst = index * 4;
        fb.pixels[dst..dst + 4].copy_from_slice(&[
            expand((pixel >> 11) & 0x1f),
            expand((pixel >> 6) & 0x1f),
            expand((pixel >> 1) & 0x1f),
            255,
        ]);
        fb.coverage[index] = raster::Coverage::from_stored(stored_coverage);
    }
}

/// Import the active public RDP color-image format into the software working
/// surface. Public Programming Manual section 15.5, "Color Image Format,"
/// defines RGBA32 memory alpha as five alpha bits plus the three coverage bits
/// in the byte's most-significant bits.
fn load_color_image(
    rdram: &[u8],
    target: gbi::ColorImage,
    fb: &mut Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) {
    let layout = target
        .layout()
        .expect("validated color image changed format");
    match layout {
        gbi::ColorImageLayout::Index8 => load_intensity8_framebuffer(rdram, target, fb),
        gbi::ColorImageLayout::Rgba16 => load_rgba5551_framebuffer(rdram, target, fb, hidden_bits),
        gbi::ColorImageLayout::Rgba32 => load_rgba8888_framebuffer(rdram, target, fb),
    }
    fb.set_color_layout(layout);
}

/// Import the public 8-bit color-image layout. Programming Manual Figure
/// 15.5.4 labels each byte as one intensity component and states that hidden
/// coverage bits are ignored for this format.
fn load_intensity8_framebuffer(rdram: &[u8], target: gbi::ColorImage, fb: &mut Framebuffer) {
    if fb.width != u32::from(target.width) {
        *fb = fb.resized(u32::from(target.width), fb.height);
    }
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..(fb.width * fb.height) as usize {
        let address = start
            .checked_add(u32::try_from(index).expect("I8 color-image offset exceeds u32"))
            .expect("I8 color-image logical address overflow");
        let intensity = view.read_u8(address);
        let destination = index * 4;
        fb.pixels[destination..destination + 4]
            .copy_from_slice(&[intensity, intensity, intensity, 255]);
        fb.coverage[index] = raster::Coverage::FULL;
    }
}

fn load_rgba8888_framebuffer(rdram: &[u8], target: gbi::ColorImage, fb: &mut Framebuffer) {
    if fb.width != u32::from(target.width) {
        *fb = fb.resized(u32::from(target.width), fb.height);
    }
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..(fb.width * fb.height) as usize {
        let offset = u32::try_from(index * 4).expect("color-image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("color-image logical address overflow");
        let [red, green, blue, alpha_coverage] = view.read_u32(address).to_be_bytes();
        let alpha5 = alpha_coverage & 0x1f;
        let alpha = (alpha5 << 3) | (alpha5 >> 2);
        let dst = index * 4;
        fb.pixels[dst..dst + 4].copy_from_slice(&[red, green, blue, alpha]);
        fb.coverage[index] = raster::Coverage::from_stored(alpha_coverage >> 5);
    }
}

/// Convert `fb`'s RGBA8888 pixels to N64 RGBA5551 and write them into
/// `rdram` starting at logical byte offset `start`, row-major with a top-left
/// origin. [`fn64_runtime::RdramViewMut`] is the sole translation from those
/// logical halfwords to N64Recomp's native-word ABI storage. A pixel whose 2
/// bytes would run past `rdram` is skipped
/// (bounds-safe; the caller already validated `output_addr` is a real
/// framebuffer offset, but a wrong width/height must not panic).
///
/// Programming Manual Chapter 15.5 specifies that the memory interface adds
/// three low dither bits and then reduces RGB from eight to five bits. The
/// rasterizer applies the public ordered matrices before this common packing
/// path and rejects only the unproven noise sequence; disabled dither remains
/// exact `>> 3` truncation. RGBA16's visible LSB is the high bit of stored
/// coverage, not retained pixel alpha; the lower two bits are committed to
/// the physical hidden-bit sidecar.
fn write_rgba5551_framebuffer(
    rdram: &mut [u8],
    start: usize,
    fb: &Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) {
    let px_count = (fb.width * fb.height) as usize;
    // The framebuffer format is a fixed 2 bytes/pixel; only write pixels the
    // fb actually has AND that fit within rdram.
    let to_5 = |c: u8| -> u16 { u16::from(c >> 3) };
    let start = fn64_runtime::RdramAddr::from_offset(
        u32::try_from(start).expect("framebuffer RDRAM offset exceeds u32"),
    );
    assert!(
        start.offset().is_multiple_of(4),
        "RGBA5551 framebuffer base must be word-aligned, got {:#x}",
        start.offset()
    );
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for i in 0..px_count {
        let byte_offset = u32::try_from(i.checked_mul(2).expect("framebuffer size overflow"))
            .expect("framebuffer byte offset exceeds u32");
        let Some(dst) = start.checked_add(byte_offset) else {
            break;
        };
        if dst.offset() as usize + 2 > view.len() {
            break;
        }
        let src = i * 4;
        let r = fb.pixels[src];
        let g = fb.pixels[src + 1];
        let b = fb.pixels[src + 2];
        let stored_coverage = fb.coverage[i].stored();
        let px: u16 = (to_5(r) << 11)
            | (to_5(g) << 6)
            | (to_5(b) << 1)
            | u16::from((stored_coverage >> 2) & 1);
        view.write_u16(dst, px);
        write_rdram_hidden_bits(hidden_bits, dst.offset(), px, stored_coverage & 3);
    }
}

fn commit_color_image(
    rdram: &mut [u8],
    target: gbi::ColorImage,
    fb: &Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) {
    match target
        .layout()
        .expect("validated color image changed format")
    {
        gbi::ColorImageLayout::Index8 => {
            write_intensity8_framebuffer(rdram, target.address as usize, fb);
            refresh_rdp_visible_halfwords_preserving_hidden(
                rdram,
                hidden_bits,
                target.address,
                fb.pixels.len() / 4,
            );
        }
        gbi::ColorImageLayout::Rgba16 => {
            write_rgba5551_framebuffer(rdram, target.address as usize, fb, hidden_bits)
        }
        gbi::ColorImageLayout::Rgba32 => {
            write_rgba8888_framebuffer(rdram, target.address as usize, fb);
            refresh_rdp_visible_halfwords_preserving_hidden(
                rdram,
                hidden_bits,
                target.address,
                fb.pixels.len(),
            );
        }
    }
}

/// Commit the color pipeline's intensity component to the public one-byte
/// color-image layout. The RDP exposes no palette for this target; callers
/// program equal RGB components when the intermediate image is meaningful,
/// so the common red/intensity lane is the byte written by the memory model.
fn write_intensity8_framebuffer(rdram: &mut [u8], start: usize, fb: &Framebuffer) {
    let pixel_count = (fb.width * fb.height) as usize;
    let start = fn64_runtime::RdramAddr::from_offset(
        u32::try_from(start).expect("I8 framebuffer RDRAM offset exceeds u32"),
    );
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for index in 0..pixel_count {
        let Some(destination) = start
            .checked_add(u32::try_from(index).expect("I8 framebuffer byte offset exceeds u32"))
        else {
            break;
        };
        if destination.offset() as usize >= view.len() {
            break;
        }
        view.write_u8(destination, fb.pixels[index * 4]);
    }
}

/// Commit RGBA32 as RGB8 plus the five-bit memory alpha and three-bit coverage
/// packing defined by public Programming Manual section 15.5. Unlike RGBA16,
/// this format does not use RDRAM hidden bits.
fn write_rgba8888_framebuffer(rdram: &mut [u8], start: usize, fb: &Framebuffer) {
    let pixel_count = (fb.width * fb.height) as usize;
    let start = fn64_runtime::RdramAddr::from_offset(
        u32::try_from(start).expect("framebuffer RDRAM offset exceeds u32"),
    );
    assert!(
        start.offset().is_multiple_of(8),
        "RGBA8888 framebuffer base must be 64-bit aligned, got {:#x}",
        start.offset()
    );
    // Chapter 15.5 stores only five bits of alpha beside three bits of
    // coverage. As with disabled RGB dither, the supported no-alpha-dither
    // path truncates rather than rounding to the nearest expanded value.
    let to_5 = |channel: u8| -> u8 { channel >> 3 };
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for index in 0..pixel_count {
        let byte_offset = u32::try_from(index.checked_mul(4).expect("framebuffer size overflow"))
            .expect("framebuffer byte offset exceeds u32");
        let Some(destination) = start.checked_add(byte_offset) else {
            break;
        };
        if destination.offset() as usize + 4 > view.len() {
            break;
        }
        let source = index * 4;
        let alpha_coverage = (fb.coverage[index].stored() << 5) | to_5(fb.pixels[source + 3]);
        view.write_u32(
            destination,
            u32::from_be_bytes([
                fb.pixels[source],
                fb.pixels[source + 1],
                fb.pixels[source + 2],
                alpha_coverage,
            ]),
        );
    }
}

fn validate_rdp_depth_image(
    rdram: &[u8],
    target: gbi::DepthImage,
    fb: &Framebuffer,
) -> Result<(), RenderError> {
    if !target.address.is_multiple_of(2) {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETZIMG base {:#010x} is not halfword-aligned",
                target.address
            ),
        });
    }
    let byte_len = (fb.width as usize)
        .checked_mul(fb.height as usize)
        .and_then(|pixels| pixels.checked_mul(2))
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETZIMG dimensions overflow host address space".to_string(),
        })?;
    let end = (target.address as usize)
        .checked_add(byte_len)
        .ok_or_else(|| RenderError::Backend {
            backend: "reference",
            reason: "G_SETZIMG address range overflows host address space".to_string(),
        })?;
    if end > rdram.len() {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: format!(
                "G_SETZIMG target [{:#010x}, {end:#010x}) exceeds RDRAM length {}",
                target.address,
                rdram.len()
            ),
        });
    }
    Ok(())
}

/// Load CPU-visible compressed Z and the separately owned hidden DeltaZ bits
/// into the software compare buffer. Nintendo 64 Programming Manual Chapter
/// 16, "Z Image Format" defines this 14+4 split; ordinary RDRAM reads expose
/// only the 16-bit word, so the hidden pair is maintained by physical address.
fn load_rdp_depth_image(
    rdram: &[u8],
    target: gbi::DepthImage,
    fb: &mut Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) -> Result<(), RenderError> {
    validate_rdp_depth_image(rdram, target, fb)?;
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    for index in 0..fb.depth.len() {
        let offset = u32::try_from(index.checked_mul(2).expect("depth image size overflow"))
            .expect("depth image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("validated depth-image logical address overflow");
        let visible = view.read_u16(address);
        let encoded = depth::EncodedDepth {
            visible,
            hidden: read_rdram_hidden_bits(hidden_bits, address.offset(), visible),
        };
        fb.depth[index] = depth::unpack(encoded).0 as f32;
        fb.encoded_depth[index] = Some(encoded);
    }
    Ok(())
}

/// Commit passing Z_UPD/fill samples to both halves of RDP depth memory.
/// Samples without an encoding are left loud at their producer rather than
/// fabricated here; every current persistent producer supplies one.
fn commit_rdp_depth_image(
    rdram: &mut [u8],
    target: gbi::DepthImage,
    fb: &Framebuffer,
    hidden_bits: &mut HashMap<u32, RdramHiddenSample>,
) -> Result<(), RenderError> {
    validate_rdp_depth_image(rdram, target, fb)?;
    let start = fn64_runtime::RdramAddr::from_offset(target.address);
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for (index, encoded) in fb.encoded_depth.iter().copied().enumerate() {
        let Some(encoded) = encoded else {
            continue;
        };
        let offset = u32::try_from(index.checked_mul(2).expect("depth image size overflow"))
            .expect("depth image byte offset exceeds u32");
        let address = start
            .checked_add(offset)
            .expect("validated depth-image logical address overflow");
        view.write_u16(address, encoded.visible);
        write_rdram_hidden_bits(
            hidden_bits,
            address.offset(),
            encoded.visible,
            encoded.hidden,
        );
    }
    Ok(())
}

/// C++-observed scalar state at the RT64 adapter boundary.
///
/// The capture is produced without creating a graphics device, but it crosses
/// the same C ABI and uses the same VI-register builder as live presentation.
/// Register indices match the private `RT64::Application::Core` block named in
/// `ffi/fn64_rt64_shim.cpp`; consumers should compare the complete array so a
/// newly populated register changes the evidence digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64AdapterCapture {
    pub task_words: [u32; 14],
    pub output_addr: u32,
    pub width: u32,
    pub height: u32,
    pub registers: [u32; 24],
}

/// Byte layout returned by RT64's post-VI swapchain render-target capture.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64PresentPixelFormat {
    /// Four unorm bytes in blue, green, red, alpha order.
    Bgra8Unorm,
    /// Four unorm bytes in red, green, blue, alpha order.
    Rgba8Unorm,
}

/// How the exact RT64 source identity was established at build time.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64SourceProvenance {
    GitClean,
    GitDirty,
    Declared,
}

/// Reproducible identity for the concrete RT64 adapter linked into this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64BackendIdentity {
    pub adapter: &'static str,
    /// Canonical SHA-256 of fn64's Rust/C++ adapter sources, target, and
    /// enabled feature set for this build.
    pub adapter_source_sha256: &'static str,
    pub source_id: &'static str,
    pub source_provenance: Rt64SourceProvenance,
    /// Stable revision of fn64's exact-source build overlay.
    pub source_overlay_id: &'static str,
    pub post_vi_api: &'static str,
}

/// Mutex-consistent live evidence from RT64's texture cache and worker queues.
/// This is intended for behavioral closure fixtures, not game policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64TextureReplacementEvidence {
    pub texture_hash: u64,
    pub stream_load_count: u64,
    pub texture_count: u32,
    pub texture_known: bool,
    pub replacement_resolved: bool,
    pub replacement_installed: bool,
    pub replacement_mip_levels: u32,
    pub replacements_enabled: bool,
    pub stream_queued: u32,
    pub stream_active: u32,
    pub stream_results_pending: u32,
    pub uploads_pending: u32,
    pub resolved_paths_pending: u32,
    pub observed_resolved_not_installed: bool,
    pub stream_workers_paused: bool,
    pub stream_worker_count: u32,
}

impl Rt64BackendIdentity {
    /// Stable identity placed inside fixed-cycle live-render evidence.
    pub fn canonical_id(&self) -> String {
        let provenance = match self.source_provenance {
            Rt64SourceProvenance::GitClean => "git-clean",
            Rt64SourceProvenance::GitDirty => "git-dirty",
            Rt64SourceProvenance::Declared => "declared",
        };
        format!(
            "adapter={};adapter_sha256={};source={};provenance={provenance};overlay={};post_vi_api={}",
            self.adapter,
            self.adapter_source_sha256,
            self.source_id,
            self.source_overlay_id,
            self.post_vi_api
        )
    }

    /// Only a clean Git tree binds its source contents without trusting an
    /// externally declared identifier or omitting local modifications.
    pub const fn is_source_authoritative(&self) -> bool {
        matches!(self.source_provenance, Rt64SourceProvenance::GitClean)
    }
}

/// One completed RT64 post-VI swapchain render target.
///
/// `bytes` is tightly packed even when the graphics API's internal readback
/// buffer requires padded rows. These bytes precede the window compositor and
/// display color management; they are not a measurement of analog VI output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64PresentedPixels {
    pub width: u32,
    pub height: u32,
    pub row_bytes: u32,
    pub format: Rt64PresentPixelFormat,
    pub present_id: u64,
    /// Workload selected by the completed present carrying these pixels.
    pub workload_id: u64,
    pub bytes: Vec<u8>,
}

/// Exact managed render target sampled by the most recently completed RT64
/// VI draw. The texture identity is process-local and intended for behavioral
/// evidence, while the address and dimensions name guest-visible state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64PresentSelection {
    pub present_id: u64,
    pub source_texture_identity: u64,
    pub target_address: u32,
    pub target_width: u32,
    pub target_height: u32,
    pub target_size: u32,
}

pub const RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS: usize = 4;
pub const RT64_DEFERRED_MAX_DRAW_CALLS: usize = 16;

/// Ordered scalar evidence for one pinned-RT64 deferred Workload.
///
/// The content digest excludes queue IDs and debugger selection so it remains
/// stable when the same recorded workload is replayed. The identity digest
/// additionally binds `workload_id` and `present_id`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64DeferredWorkloadSnapshot {
    pub workload_id: u64,
    pub present_id: u64,
    pub submission_frame: u64,
    pub content_digest: u64,
    pub identity_digest: u64,
    pub framebuffer_pair_count: u32,
    pub projection_count: u32,
    pub game_call_count: u32,
    pub triangle_count: u32,
    pub vertex_count: u32,
    pub face_index_count: u32,
    pub rdp_param_count: u32,
    pub load_operation_count: u32,
    pub selected_framebuffer_index: i32,
    pub selected_draw_call_index: i32,
    pub selected_framebuffer_address: u32,
    pub paused: bool,
    pub pair_color_addresses: [u32; RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS],
    pub pair_game_call_counts: [u32; RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS],
    pub pair_projection_counts: [u32; RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS],
    pub call_uids: [u32; RT64_DEFERRED_MAX_DRAW_CALLS],
    pub call_fill_colors: [u32; RT64_DEFERRED_MAX_DRAW_CALLS],
    pub call_triangle_counts: [u32; RT64_DEFERRED_MAX_DRAW_CALLS],
}

/// Pre-submission and current images of the same deferred workload queue slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64DeferredWorkloadEvidence {
    pub pre_submission: Rt64DeferredWorkloadSnapshot,
    pub current: Rt64DeferredWorkloadSnapshot,
}

/// Exclusive completed-workload route for a framebuffer-backed texture load.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64FramebufferCopyPath {
    /// RT64 created and sampled a GPU framebuffer tile copy.
    GpuTileCopy,
    /// RT64 replayed the RDRAM load into TMEM and used the ordinary texture upload.
    CpuRdramTmemUpload,
}

/// Read-only mechanism evidence from one completed region-copy workload.
///
/// The evidence query rejects zero, mixed, or multiple copy routes rather than
/// reducing an ambiguous workload to one policy label.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64FramebufferCopyPathEvidence {
    pub workload_id: u64,
    /// Process-local identity of the prior managed source framebuffer.
    pub source_framebuffer_identity: u64,
    pub source_framebuffer_address: u32,
    pub path: Rt64FramebufferCopyPath,
    pub gpu_create_tile_copy_operation_count: u32,
    pub gpu_tile_dispatch_count: u32,
    pub cpu_rdram_tmem_upload_count: u32,
    pub raw_tmem_tile_count: u32,
    pub sync_framebuffer_pair_count: u32,
}

/// Read-only load geometry and vector counts from one completed S2DEX texture workload.
///
/// These are downstream workload artifacts, not counters in either enhancement
/// branch. The digest binds every ordered texture, tile, and load-operation
/// descriptor; exact multiplicities distinguish the ordinary texture-upload
/// route from the single managed-framebuffer tile-copy route.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64S2dexFastPathEvidence {
    pub workload_id: u64,
    pub source_framebuffer_identity: u64,
    pub load_operation_digest: u64,
    pub source_address: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub source_size: u32,
    pub gpu_create_tile_copy_operation_count: u32,
    pub gpu_tile_dispatch_count: u32,
    pub cpu_rdram_tmem_upload_count: u32,
    pub raw_tmem_tile_count: u32,
    pub sync_framebuffer_pair_count: u32,
    pub framebuffer_pair_count: u32,
    pub valid_tile_count: u32,
    pub load_operation_count: u32,
    pub distinct_source_address_count: u32,
    pub minimum_source_address: u32,
    pub maximum_source_address: u32,
    pub base_source_load_count: u32,
    pub offset_source_load_count: u32,
    pub source_is_managed_framebuffer: bool,
}

pub const RT64_EXTENDED_COMMAND_COUNT: usize = 0x34;
pub const RT64_EXTENDED_MAX_RECTS: usize = 16;
pub const RT64_EXTENDED_MAX_GROUPS: usize = 16;
pub const RT64_EXTENDED_MAX_VERTEX_Z_MARKERS: usize = 16;
pub const RT64_EXTENDED_MAX_GENERATED_PRESENTS: usize = 8;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64TransformClass {
    Model,
    Projection,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64TransformComponentSelector {
    Skip,
    Interpolate,
    Auto,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64TransformOrdering {
    Linear,
    Auto,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64ExtendedAspectMode {
    Auto,
    Stretch,
    Adjust,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64ExtendedRectEvidence {
    pub draw_call_uid: u32,
    pub left_origin: u16,
    pub right_origin: u16,
    pub left_offset: i32,
    pub top_offset: i32,
    pub right_offset: i32,
    pub bottom_offset: i32,
    pub upper_left_x: i32,
    pub upper_left_y: i32,
    pub lower_right_x: i32,
    pub lower_right_y: i32,
    pub aspect_mode: Rt64ExtendedAspectMode,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64TransformGroupEvidence {
    pub group_id: u32,
    pub class: Rt64TransformClass,
    pub push: bool,
    pub decompose: bool,
    pub editable: bool,
    pub position: Rt64TransformComponentSelector,
    pub rotation: Rt64TransformComponentSelector,
    pub scale: Rt64TransformComponentSelector,
    pub skew: Rt64TransformComponentSelector,
    pub perspective: Rt64TransformComponentSelector,
    pub vertex: Rt64TransformComponentSelector,
    pub texcoord: Rt64TransformComponentSelector,
    pub tile: Rt64TransformComponentSelector,
    pub look_at: Rt64TransformComponentSelector,
    pub ordering: Rt64TransformOrdering,
    pub aspect_mode: Rt64ExtendedAspectMode,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64VertexZMarkerKind {
    Begin,
    End,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64VertexZEvidence {
    pub marker_kind: Rt64VertexZMarkerKind,
    pub command_vertex_index: Option<u32>,
    pub resolved_source_index: u32,
    pub affected_face_index_start: u32,
    pub affected_face_index_count: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64GeneratedPresentEvidence {
    pub previous_workload_id: u64,
    pub current_workload_id: u64,
    pub present_id: u64,
    pub presentation_ordinal: u32,
    pub interpolation_numerator: u32,
    pub interpolation_denominator: u32,
    pub original_refresh_rate: u32,
    pub target_refresh_rate: u32,
}

/// One ordered post-VI image retained from an explicitly armed Extended-GBI
/// evidence interval. `generated_ordinal` is absent for a single ordinary
/// endpoint and present for every generated/interpolated image in a burst.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64ExtendedPresentedPixels {
    pub capture_generation: u64,
    pub workload_id: u64,
    pub present_id: u64,
    pub capture_ordinal: u32,
    pub generated_ordinal: Option<u32>,
    pub interpolation_numerator: u32,
    pub interpolation_denominator: u32,
    pub width: u32,
    pub height: u32,
    pub row_bytes: u32,
    pub format: Rt64PresentPixelFormat,
    pub bytes: Vec<u8>,
}

/// One ordered post-VI image from a synthetic HFR evidence burst.
#[cfg(feature = "hfr-evidence")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64HfrPresentedPixels {
    pub capture_generation: u64,
    pub workload_id: u64,
    pub present_id: u64,
    pub capture_ordinal: u32,
    pub burst_ordinal: Option<u32>,
    pub derived_weight_numerator: u32,
    pub derived_weight_denominator: u32,
    pub width: u32,
    pub height: u32,
    pub row_bytes: u32,
    pub format: Rt64PresentPixelFormat,
    pub bytes: Vec<u8>,
}

#[cfg(feature = "hfr-evidence")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64HfrPresentationKind {
    SpatialIntermediate,
    CurrentEndpoint,
}

/// Ordered 120/60 presentation identity. The weight is derived from pinned
/// RT64's exact-double-rate integral algorithm; it is not a sampled shader value.
#[cfg(feature = "hfr-evidence")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64HfrPresentationEvidence {
    pub previous_workload_id: u64,
    pub current_workload_id: u64,
    pub present_id: u64,
    pub presentation_ordinal: u32,
    pub kind: Rt64HfrPresentationKind,
    pub derived_weight_numerator: u32,
    pub derived_weight_denominator: u32,
}

/// Causal state from one runtime-selected RT64 presentation burst.
///
/// The evidence-only synthetic admission substitutes only microcode hash
/// recognition. Workload matching, interpolation, rendering, presentation,
/// and the user refresh-rate policy are the pinned RT64 mechanisms.
#[cfg(feature = "hfr-evidence")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64HfrEvidence {
    pub previous_workload_id: u64,
    pub current_workload_id: u64,
    pub present_id: u64,
    pub interpolation_framebuffer_identity: u64,
    pub interpolation_framebuffer_address: u32,
    pub original_refresh_rate: u32,
    pub target_refresh_rate: u32,
    pub presentation_count: u32,
    pub available_interpolated_target_count: u32,
    /// The pinned present queue's internal `presented` counter. Both the
    /// Original control and exact-double-rate burst report one here.
    pub presented_counter_value: u32,
    pub presentations: Vec<Rt64HfrPresentationEvidence>,
}

/// One actual swapchain-present call bracketed by a monotonic host clock.
///
/// The start timestamp is taken after RT64's precise sleep and optional
/// present wait; the return timestamp is taken immediately after `present`
/// returns. These are API-call observations, not physical display scanout
/// timestamps.
#[cfg(feature = "hfr-evidence")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64HfrPacingSample {
    pub call_start_monotonic_ns: u64,
    pub call_return_monotonic_ns: u64,
    pub present_id: u64,
    pub burst_ordinal: u32,
    pub burst_count: u32,
    pub original_refresh_rate: u32,
    pub target_refresh_rate: u32,
}

/// Bounded ordered actual-present call history from pinned RT64.
#[cfg(feature = "hfr-evidence")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64HfrPacingEvidence {
    pub samples: Vec<Rt64HfrPacingSample>,
}

/// Typed, bounded evidence from one explicitly armed recognized-HLE task.
///
/// This is an observation surface only. It does not admit a microcode image,
/// enable Extended GBI, or imply that any public feature claim is closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64ExtendedGbiEvidence {
    pub workload_id: u64,
    pub present_id: u64,
    pub enabled_opcode: Option<u8>,
    pub hook_enable_count: u32,
    pub command_counts: [u32; RT64_EXTENDED_COMMAND_COUNT],
    pub refresh_rate: Option<u16>,
    pub rects: Vec<Rt64ExtendedRectEvidence>,
    pub groups: Vec<Rt64TransformGroupEvidence>,
    pub vertex_z: Vec<Rt64VertexZEvidence>,
    pub generated_presents: Vec<Rt64GeneratedPresentEvidence>,
}

pub const RT64_UBERSHADER_MAX_RASTER_CALLS: usize = 16;

/// Exact Metal construction events and ordered raster pipeline selections for
/// one pinned-RT64 evidence interval.
///
/// Pipeline identities are process-local. Background construction is reported
/// separately from caller, workload-worker, and present-worker events.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rt64UbershaderEvidence {
    pub workload_id: u64,
    pub present_id: u64,
    pub descriptor_digest: u64,
    pub pipeline_digest: u64,
    pub graphics_pipeline_construction_events: u64,
    pub background_construction_events: u64,
    pub caller_construction_events: u32,
    pub workload_construction_events: u32,
    pub present_construction_events: u32,
    pub precreated_pipeline_count: u32,
    pub raster_call_count: u32,
    pub matched_ubershader_call_count: u32,
    pub specialized_shader_count: u32,
    pub ubershaders_only: bool,
    pub shader_hashes: [u64; RT64_UBERSHADER_MAX_RASTER_CALLS],
    pub pipeline_state_indices: [u32; RT64_UBERSHADER_MAX_RASTER_CALLS],
    pub pipeline_identities: [u64; RT64_UBERSHADER_MAX_RASTER_CALLS],
}

impl Rt64AdapterCapture {
    /// Stable SHA-256 evidence over a versioned, big-endian encoding.
    pub fn sha256(&self) -> [u8; 32] {
        use sha2::Digest;

        let mut hasher = sha2::Sha256::new();
        hasher.update(b"fn64-rt64-adapter-capture-v1\0");
        for word in self.task_words {
            hasher.update(word.to_be_bytes());
        }
        for word in [self.output_addr, self.width, self.height] {
            hasher.update(word.to_be_bytes());
        }
        for word in self.registers {
            hasher.update(word.to_be_bytes());
        }
        hasher.finalize().into()
    }
}

/// Round typed fn64 task and VI state through the production Rust/C/C++ ABI.
///
/// This capture does not initialize SDL, a graphics API, or a GPU. Enabling
/// the `rt64` feature is still required because the C++ shim and pinned MIT
/// RT64 archive are one intentionally quarantined link unit.
pub fn capture_rt64_adapter_inputs(
    task: &OsTask,
    output_addr: u32,
    cfg: RenderConfig,
    vi: ViPresentation,
) -> Result<Rt64AdapterCapture, RenderError> {
    #[cfg(feature = "rt64")]
    {
        ffi::capture_adapter_inputs(task, output_addr, cfg.width, cfg.height, vi).map_err(
            |reason| RenderError::Backend {
                backend: "rt64-adapter-capture",
                reason,
            },
        )
    }

    #[cfg(not(feature = "rt64"))]
    {
        let _ = (task, output_addr, cfg, vi);
        Err(RenderError::Backend {
            backend: "rt64-adapter-capture",
            reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                .to_string(),
        })
    }
}

/// Validate and round-trip the complete typed settings image through the
/// production Rust/C/C++ ABI without creating a graphics device.
pub fn roundtrip_rt64_runtime_settings(
    settings: &RenderRuntimeSettings,
) -> Result<RenderRuntimeSettings, RenderError> {
    #[cfg(feature = "rt64")]
    {
        ffi::roundtrip_user_config(settings).map_err(|reason| RenderError::Backend {
            backend: "rt64-settings-roundtrip",
            reason,
        })
    }

    #[cfg(not(feature = "rt64"))]
    {
        let _ = settings;
        Err(RenderError::Backend {
            backend: "rt64-settings-roundtrip",
            reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                .to_string(),
        })
    }
}

/// Device-free validation of the complete pinned enhancement configuration
/// across the production Rust/C/C++ boundary.
pub fn roundtrip_rt64_enhancement_settings(
    settings: &RenderEnhancementSettings,
) -> Result<RenderEnhancementSettings, RenderError> {
    #[cfg(feature = "rt64")]
    {
        ffi::roundtrip_enhancement_config(settings).map_err(|reason| RenderError::Backend {
            backend: "rt64-enhancement-roundtrip",
            reason,
        })
    }

    #[cfg(not(feature = "rt64"))]
    {
        let _ = settings;
        Err(RenderError::Backend {
            backend: "rt64-enhancement-roundtrip",
            reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                .to_string(),
        })
    }
}

/// Device-free validation of the complete pinned emulator configuration
/// across the production Rust/C/C++ boundary.
pub fn roundtrip_rt64_emulator_settings(
    settings: &RenderEmulatorSettings,
) -> Result<RenderEmulatorSettings, RenderError> {
    #[cfg(feature = "rt64")]
    {
        ffi::roundtrip_emulator_config(settings).map_err(|reason| RenderError::Backend {
            backend: "rt64-emulator-roundtrip",
            reason,
        })
    }

    #[cfg(not(feature = "rt64"))]
    {
        let _ = settings;
        Err(RenderError::Backend {
            backend: "rt64-emulator-roundtrip",
            reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                .to_string(),
        })
    }
}

/// RT64's MIT C++ render/HLE core behind one crate-local C ABI boundary.
/// The feature-gated implementation passes fn64's stable RDRAM allocation,
/// the task's ucode/display-list addresses, and a private register block to
/// `RT64::Application::Core`. RT64's render-to-RAM path writes the native
/// RGBA5551 framebuffer back into the same slice the existing fn64 VI path
/// presents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rt64ReplacementPackInput {
    path: PathBuf,
}

impl Rt64ReplacementPackInput {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedReplacementPack {
    input: Rt64ReplacementPackInput,
    canonical_path: PathBuf,
    identity: RenderReplacementPackIdentity,
}

#[cfg(feature = "rt64")]
fn hash_replacement_content(path: &Path) -> Result<[u8; 32], String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("replacement-pack metadata failed for {path:?}: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "replacement-pack root may not be a symlink: {path:?}"
        ));
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64.rt64-replacement-content.v1\0");
    if metadata.is_file() {
        if path.extension().and_then(|value| value.to_str()) != Some("rtz") {
            return Err(format!(
                "replacement-pack file must have lowercase .rtz extension: {path:?}"
            ));
        }
        hasher.update([1]);
        let bytes = std::fs::read(path)
            .map_err(|error| format!("replacement-pack read failed for {path:?}: {error}"))?;
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    } else if metadata.is_dir() {
        hasher.update([2]);
        let mut pending = vec![(PathBuf::new(), path.to_path_buf())];
        let mut files = Vec::new();
        while let Some((relative_dir, absolute_dir)) = pending.pop() {
            let mut entries = std::fs::read_dir(&absolute_dir)
                .map_err(|error| {
                    format!("replacement-pack directory read failed for {absolute_dir:?}: {error}")
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    format!("replacement-pack entry read failed for {absolute_dir:?}: {error}")
                })?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let name = entry.file_name();
                let name = name.to_str().ok_or_else(|| {
                    format!("replacement-pack contains a non-UTF-8 path under {absolute_dir:?}")
                })?;
                let relative = relative_dir.join(name);
                let entry_path = entry.path();
                let entry_metadata = std::fs::symlink_metadata(&entry_path).map_err(|error| {
                    format!("replacement-pack metadata failed for {entry_path:?}: {error}")
                })?;
                if entry_metadata.file_type().is_symlink() {
                    return Err(format!(
                        "replacement-pack contains a symbolic link: {entry_path:?}"
                    ));
                }
                if entry_metadata.is_dir() {
                    pending.push((relative, entry_path));
                } else if entry_metadata.is_file() {
                    let relative = relative
                        .components()
                        .map(|component| {
                            component
                                .as_os_str()
                                .to_str()
                                .expect("all path segments were checked as UTF-8")
                        })
                        .collect::<Vec<_>>()
                        .join("/");
                    files.push((relative, entry_path));
                } else {
                    return Err(format!(
                        "replacement-pack contains a non-file entry: {entry_path:?}"
                    ));
                }
            }
        }
        files.sort_by(|left, right| left.0.cmp(&right.0));
        for (relative, absolute) in files {
            let relative_bytes = relative.as_bytes();
            let bytes = std::fs::read(&absolute).map_err(|error| {
                format!("replacement-pack read failed for {absolute:?}: {error}")
            })?;
            hasher.update(
                u32::try_from(relative_bytes.len())
                    .map_err(|_| format!("replacement-pack path is too long: {relative}"))?
                    .to_be_bytes(),
            );
            hasher.update(relative_bytes);
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        }
    } else {
        return Err(format!(
            "replacement-pack path is neither one directory nor one .rtz file: {path:?}"
        ));
    }
    Ok(hasher.finalize().into())
}

#[cfg(feature = "rt64")]
fn resolve_replacement_packs(
    inputs: &[Rt64ReplacementPackInput],
) -> Result<Vec<ResolvedReplacementPack>, String> {
    let mut resolved = Vec::with_capacity(inputs.len());
    let mut seen = std::collections::HashSet::new();
    for input in inputs {
        let root_metadata = std::fs::symlink_metadata(&input.path).map_err(|error| {
            format!(
                "replacement-pack metadata failed for {:?}: {error}",
                input.path
            )
        })?;
        if root_metadata.file_type().is_symlink() {
            return Err(format!(
                "replacement-pack root may not be a symlink: {:?}",
                input.path
            ));
        }
        let canonical_path = std::fs::canonicalize(&input.path).map_err(|error| {
            format!(
                "replacement-pack path resolution failed for {:?}: {error}",
                input.path
            )
        })?;
        if !seen.insert(canonical_path.clone()) {
            return Err(format!(
                "replacement-pack input is duplicated: {canonical_path:?}"
            ));
        }
        let path_utf8 = canonical_path.to_str().ok_or_else(|| {
            format!("replacement-pack root is not valid UTF-8: {canonical_path:?}")
        })?;
        let path_c = CString::new(path_utf8)
            .map_err(|_| format!("replacement-pack root contains NUL: {canonical_path:?}"))?;
        let content_sha256 = hash_replacement_content(&canonical_path)?;
        let (mut identity, database_bytes) = ffi::inspect_replacement_pack(&path_c)?;
        identity.content_sha256 = content_sha256;
        identity.database_sha256 = sha2::Sha256::digest(database_bytes).into();
        // Catch writes that raced the database inspection itself.
        if hash_replacement_content(&canonical_path)? != content_sha256 {
            return Err(format!(
                "replacement-pack changed during inspection: {canonical_path:?}"
            ));
        }
        resolved.push(ResolvedReplacementPack {
            input: input.clone(),
            canonical_path,
            identity,
        });
    }
    Ok(resolved)
}

#[cfg(feature = "rt64")]
fn replacement_ffi_inputs(
    packs: &[ResolvedReplacementPack],
) -> Result<Vec<(CString, RenderReplacementPackIdentity)>, String> {
    packs
        .iter()
        .map(|pack| {
            let path = pack.canonical_path.to_str().ok_or_else(|| {
                format!(
                    "replacement-pack root stopped being UTF-8: {:?}",
                    pack.canonical_path
                )
            })?;
            Ok((
                CString::new(path).expect("validated path has no NUL"),
                pack.identity.clone(),
            ))
        })
        .collect()
}

pub struct Rt64Backend {
    /// RT64's GBI selection is still HLE. Apply the same exact task-entry
    /// admission as the Rust reference backend before crossing the C ABI.
    f3dex2_ucodes: gbi::F3dex2UcodeCatalog,
    /// FullSync result of the last successfully committed submission.
    last_dp_full_sync: fn64_render::DpFullSyncStatus,
    #[cfg(feature = "rt64")]
    task_index: u64,
    #[cfg(feature = "rt64")]
    context: Option<ffi::Context>,
    #[cfg(not(feature = "rt64"))]
    created: bool,
    /// Guest cycle supplied at the last successfully completed VI present.
    /// Keeping the cycle beside the backend-owned image prevents a release
    /// gate from relabeling an older swapchain capture as current evidence.
    last_present_cycle: Option<u64>,
    /// Requested settings for the next create. This may differ from active
    /// settings only when an apply returned `RestartRequired`.
    configured_settings: RenderRuntimeSettings,
    /// Settings actually installed into the live RT64 application. Release
    /// evidence hashes this image, never pending recreate settings.
    active_settings: Option<RenderRuntimeSettings>,
    configured_enhancement_settings: RenderEnhancementSettings,
    active_enhancement_settings: Option<RenderEnhancementSettings>,
    configured_emulator_settings: RenderEmulatorSettings,
    active_emulator_settings: Option<RenderEmulatorSettings>,
    configured_replacement_packs: Vec<ResolvedReplacementPack>,
    configured_replacement_enabled: bool,
    active_replacement_settings: Option<RenderReplacementSettings>,
}

impl Rt64Backend {
    pub fn new() -> Self {
        Rt64Backend {
            f3dex2_ucodes: gbi::F3dex2UcodeCatalog::default(),
            last_dp_full_sync: fn64_render::DpFullSyncStatus::Unidentified,
            #[cfg(feature = "rt64")]
            task_index: 0,
            #[cfg(feature = "rt64")]
            context: None,
            #[cfg(not(feature = "rt64"))]
            created: false,
            last_present_cycle: None,
            configured_settings: RenderRuntimeSettings::default(),
            active_settings: None,
            configured_enhancement_settings: RenderEnhancementSettings::default(),
            active_enhancement_settings: None,
            configured_emulator_settings: RenderEmulatorSettings::default(),
            active_emulator_settings: None,
            configured_replacement_packs: Vec::new(),
            configured_replacement_enabled: RenderReplacementSettings::default().enabled,
            active_replacement_settings: None,
        }
    }

    /// Identity of the RT64 source and post-VI capture API in this feature
    /// build. The build script derives Git state from the selected source tree
    /// or records an explicit `FN64_RT64_SOURCE_ID` as declared provenance.
    #[cfg(feature = "rt64")]
    pub fn release_identity() -> Rt64BackendIdentity {
        let source_provenance = match env!("FN64_RT64_SOURCE_PROVENANCE") {
            "git-clean" => Rt64SourceProvenance::GitClean,
            "git-dirty" => Rt64SourceProvenance::GitDirty,
            "declared" => Rt64SourceProvenance::Declared,
            value => panic!("unknown RT64 source provenance {value}"),
        };
        Rt64BackendIdentity {
            adapter: "fn64-render-rt64/rt64",
            adapter_source_sha256: env!("FN64_RT64_ADAPTER_SOURCE_SHA256"),
            source_id: env!("FN64_RT64_SOURCE_ID"),
            source_provenance,
            source_overlay_id: env!("FN64_RT64_SOURCE_OVERLAY_ID"),
            post_vi_api: if cfg!(target_os = "macos") {
                "metal-bgra8-unorm"
            } else if cfg!(target_os = "windows") {
                "d3d12-or-vulkan-bgra8-rgba8-unorm"
            } else {
                "vulkan-bgra8-rgba8-unorm"
            },
        }
    }

    /// Enable exact post-VI swapchain render-target capture.
    ///
    /// The pinned RT64 generic render hook does not expose its framebuffer's
    /// attachment. This opt-in path validates the concrete Plume Metal,
    /// Vulkan, or D3D12 attachment and retains a fenced readback buffer.
    pub fn enable_present_capture(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_present_capture()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-present-capture",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Wait for the sole/selected TMEM texture, optionally including its
    /// installed replacement. Completion is defined by RT64's live cache map;
    /// the C++ seam does not use a duration, sleep, or timing threshold.
    pub fn wait_texture_replacement_evidence(
        &mut self,
        texture_hash: Option<u64>,
        require_replacement: bool,
    ) -> Result<Rt64TextureReplacementEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .wait_texture_replacement_state(texture_hash, require_replacement)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-texture-replacement-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (texture_hash, require_replacement);
            Err(RenderError::Backend {
                backend: "rt64-texture-replacement-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Pause or restore RT64's texture Stream workers for a deterministic
    /// behavior fixture. Pause succeeds only when the upload and stream queues
    /// are quiescent; resume recreates the exact pinned-cache worker count.
    /// This is an evidence scheduling gate, not renderer policy.
    pub fn set_texture_stream_workers_paused_for_evidence(
        &mut self,
        paused: bool,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .set_stream_workers_paused(paused)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-texture-stream-evidence-control",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = paused;
            Err(RenderError::Backend {
                backend: "rt64-texture-stream-evidence-control",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Wait for a real RT64 Stream path to be resolved and queued while the
    /// evidence worker hold keeps its replacement absent from the texture map.
    pub fn wait_texture_stream_fallback_evidence(
        &mut self,
        texture_hash: u64,
    ) -> Result<Rt64TextureReplacementEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .wait_stream_fallback_state(texture_hash)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-texture-stream-fallback-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = texture_hash;
            Err(RenderError::Backend {
                backend: "rt64-texture-stream-fallback-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Read the most recent completed post-VI swapchain render target.
    pub fn presented_pixels(&mut self) -> Result<Rt64PresentedPixels, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .presented_pixels()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-present-capture",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the exact source texture and framebuffer identity bound by RT64's
    /// most recently completed VI draw.
    pub fn present_selection(&mut self) -> Result<Rt64PresentSelection, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .present_selection()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-present-selection",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-present-selection",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Arm the next raw-DPC workload for a worker-excluded pre-submission
    /// snapshot. This evidence control is bounded to one completed workload.
    pub fn enable_deferred_workload_capture_for_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_deferred_workload_capture()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-deferred-workload-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-deferred-workload-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the captured pre-submission workload and its current paused-replay
    /// image after both RT64 queue workers become idle.
    pub fn deferred_workload_evidence(
        &mut self,
    ) -> Result<Rt64DeferredWorkloadEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .deferred_workload_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-deferred-workload-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-deferred-workload-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the exclusive GPU-tile-copy or CPU synchronization fallback route
    /// taken by the captured completed workload.
    pub fn framebuffer_copy_path_evidence(
        &mut self,
    ) -> Result<Rt64FramebufferCopyPathEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .framebuffer_copy_path_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-framebuffer-copy-path-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-framebuffer-copy-path-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read downstream texture-route vectors for the captured S2DEX workload.
    pub fn s2dex_fast_path_evidence(&mut self) -> Result<Rt64S2dexFastPathEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .s2dex_fast_path_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-s2dex-fast-path-evidence",
                    reason,
                })
        }
        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-s2dex-fast-path-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Arm pass-through typed evidence for exactly the next recognized-HLE
    /// task. This does not admit microcode or enable Extended GBI itself.
    pub fn enable_extended_gbi_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_extended_gbi_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-extended-gbi-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-extended-gbi-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the semantic Extended-GBI, aspect, vertex-Z, and generated-frame
    /// evidence bound to the explicitly armed completed workload.
    pub fn extended_gbi_evidence(&mut self) -> Result<Rt64ExtendedGbiEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .extended_gbi_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-extended-gbi-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-extended-gbi-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read every ordered post-VI image retained while the current Extended
    /// evidence interval was armed. Semantic evidence must be read first so
    /// the workload/present/fraction provenance has reached queue idle.
    pub fn extended_presented_pixels(
        &mut self,
    ) -> Result<Vec<Rt64ExtendedPresentedPixels>, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .extended_presented_pixels()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-extended-present-capture",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-extended-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Arm exactly one bounded HFR presentation history.
    #[cfg(feature = "hfr-evidence")]
    pub fn enable_hfr_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_hfr_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-hfr-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-hfr-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a non-ROM, hand-authored F3DEX2 display list for HFR evidence.
    ///
    /// Production [`RenderBackend::process_task`] recognition is deliberately
    /// unchanged; this method substitutes only the test fixture's microcode
    /// hash admission and then runs RT64's normal HLE/workload/render path.
    #[cfg(feature = "synthetic-f3dex2-evidence")]
    pub fn process_synthetic_hfr_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
        original_refresh_rate: u16,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_hfr_f3dex2(
                    rdram,
                    display_list,
                    output_addr,
                    original_refresh_rate,
                )
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-hfr-f3dex2",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr, original_refresh_rate);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-hfr-f3dex2",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a non-ROM, hand-authored public S2DEX2 display list.
    ///
    /// This non-default evidence seam substitutes only the fixture's GBI
    /// dialect. Normal [`RenderBackend::process_task`] recognition continues
    /// to require an exact supported microcode identity.
    #[cfg(feature = "synthetic-s2dex-evidence")]
    pub fn process_synthetic_s2dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_s2dex2(rdram, display_list, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-s2dex2",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-s2dex2",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a hand-authored, non-ROM F3DEX2 display list through RT64's
    /// normal interpreter/workload/render path for Extended-GBI evidence.
    ///
    /// This non-default test seam substitutes the fixture's GBI dialect only.
    /// Production [`RenderBackend::process_task`] still requires RT64 to
    /// recognize the submitted microcode text/data pair by hash.
    #[cfg(feature = "extended-gbi-evidence")]
    pub fn process_synthetic_extended_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_extended_f3dex2(rdram, display_list, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-extended-f3dex2",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-extended-f3dex2",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Finalize and read the causal HFR workload/presentation state.
    #[cfg(feature = "hfr-evidence")]
    pub fn hfr_evidence(&mut self) -> Result<Rt64HfrEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .hfr_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-hfr-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-hfr-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the ordered post-VI images after [`Self::hfr_evidence`] finalizes
    /// the associated workload and interpolation fractions.
    #[cfg(feature = "hfr-evidence")]
    pub fn hfr_presented_pixels(&mut self) -> Result<Vec<Rt64HfrPresentedPixels>, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .hfr_presented_pixels()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-hfr-present-capture",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-hfr-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Start a bounded observation window at RT64's actual present-call seam.
    #[cfg(feature = "hfr-evidence")]
    pub fn enable_hfr_pacing_evidence(&mut self) -> Result<(), RenderError> {
        self.context
            .as_mut()
            .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
            .enable_hfr_pacing_evidence()
            .map_err(|reason| RenderError::Backend {
                backend: "rt64-hfr-pacing-evidence",
                reason,
            })
    }

    /// Join both RT64 queues and finalize actual present-call timing evidence.
    ///
    /// This observes post-sleep call cadence, not physical display scanout.
    #[cfg(feature = "hfr-evidence")]
    pub fn hfr_pacing_evidence(&mut self) -> Result<Rt64HfrPacingEvidence, RenderError> {
        self.context
            .as_mut()
            .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
            .hfr_pacing_evidence()
            .map_err(|reason| RenderError::Backend {
                backend: "rt64-hfr-pacing-evidence",
                reason,
            })
    }

    /// Set the backend-independent debugger pause and render boundary used by
    /// pinned RT64's paused replay path.
    ///
    /// This is a deterministic host evidence seam, not a claim that RT64's
    /// ImGui Inspector frontend supports Metal.
    pub fn set_debugger_inspection_for_evidence(
        &mut self,
        paused: bool,
        framebuffer_index: i32,
        draw_call_index: i32,
        framebuffer_depth: bool,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .set_debugger_inspection_for_evidence(
                    paused,
                    framebuffer_index,
                    draw_call_index,
                    framebuffer_depth,
                )
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-debugger-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (
                paused,
                framebuffer_index,
                draw_call_index,
                framebuffer_depth,
            );
            Err(RenderError::Backend {
                backend: "rt64-debugger-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Wait for all eight pinned raster ubershader pipelines, force the
    /// backend's ubershader-only selection path, and begin exact Metal PSO
    /// construction-event evidence.
    pub fn enable_ubershader_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_ubershader_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-ubershader-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-ubershader-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read construction counters and the exact precreated ubershader pipeline
    /// selected for every raster call in the most recently completed workload.
    pub fn ubershader_evidence(&mut self) -> Result<Rt64UbershaderEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .ubershader_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-ubershader-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-ubershader-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Admit one exact task-entry F3DEX2 text image for RT64 HLE. Unknown
    /// images return `NeedsLle` without crossing the C ABI.
    pub fn with_f3dex2_ucode_sha256(mut self, digest: [u8; 32]) -> Self {
        self.f3dex2_ucodes.admit_sha256(digest);
        self
    }

    /// Admit one exact logical 4 KiB task-entry image, retaining only its
    /// SHA-256 identity. This mirrors `ReferenceBackend` fixture setup.
    pub fn with_f3dex2_ucode_text(mut self, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "F3DEX2 text admission requires one complete 4 KiB IMEM image"
        );
        self.f3dex2_ucodes.admit_text(text);
        self
    }

    /// Stage a complete settings image for the next backend creation.
    pub fn with_runtime_settings(mut self, settings: RenderRuntimeSettings) -> Self {
        self.configured_settings = settings;
        self
    }

    pub fn configured_settings(&self) -> &RenderRuntimeSettings {
        &self.configured_settings
    }

    pub fn active_settings(&self) -> Option<&RenderRuntimeSettings> {
        self.active_settings.as_ref()
    }

    pub fn with_enhancement_settings(mut self, settings: RenderEnhancementSettings) -> Self {
        self.configured_enhancement_settings = settings;
        self
    }

    pub fn with_emulator_settings(mut self, settings: RenderEmulatorSettings) -> Self {
        self.configured_emulator_settings = settings;
        self
    }

    pub fn with_runtime_policy(mut self, policy: RenderRuntimePolicy) -> Self {
        assert!(
            policy.replacement.packs.is_empty(),
            "with_runtime_policy cannot reconstruct replacement-pack host paths from byte identities; call load_replacement_packs before create"
        );
        self.configured_settings = policy.user;
        self.configured_enhancement_settings = policy.enhancement;
        self.configured_emulator_settings = policy.emulator;
        self.configured_replacement_packs.clear();
        self.configured_replacement_enabled = policy.replacement.enabled;
        self
    }

    pub fn configured_enhancement_settings(&self) -> &RenderEnhancementSettings {
        &self.configured_enhancement_settings
    }

    pub fn active_enhancement_settings(&self) -> Option<&RenderEnhancementSettings> {
        self.active_enhancement_settings.as_ref()
    }

    pub fn configured_emulator_settings(&self) -> &RenderEmulatorSettings {
        &self.configured_emulator_settings
    }

    pub fn active_emulator_settings(&self) -> Option<&RenderEmulatorSettings> {
        self.active_emulator_settings.as_ref()
    }

    pub fn configured_replacement_settings(&self) -> RenderReplacementSettings {
        RenderReplacementSettings {
            enabled: self.configured_replacement_enabled,
            packs: self
                .configured_replacement_packs
                .iter()
                .map(|pack| pack.identity.clone())
                .collect(),
        }
    }

    pub fn active_replacement_settings(&self) -> Option<&RenderReplacementSettings> {
        self.active_replacement_settings.as_ref()
    }

    pub fn configured_runtime_policy(&self) -> RenderRuntimePolicy {
        RenderRuntimePolicy {
            user: self.configured_settings.clone(),
            enhancement: self.configured_enhancement_settings.clone(),
            emulator: self.configured_emulator_settings.clone(),
            replacement: self.configured_replacement_settings(),
        }
    }

    pub fn active_runtime_policy(&self) -> Option<RenderRuntimePolicy> {
        Some(RenderRuntimePolicy {
            user: self.active_settings.as_ref()?.clone(),
            enhancement: self.active_enhancement_settings.as_ref()?.clone(),
            emulator: self.active_emulator_settings.as_ref()?.clone(),
            replacement: self.active_replacement_settings.as_ref()?.clone(),
        })
    }

    /// Inspect and stage ordered replacement packs, or transactionally load
    /// them into an existing RT64 context. Only a stable pre/load/post byte
    /// identity becomes active release policy.
    pub fn load_replacement_packs(
        &mut self,
        inputs: &[Rt64ReplacementPackInput],
        enabled: bool,
    ) -> Result<RenderPolicyApply, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let resolved =
                resolve_replacement_packs(inputs).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-inspect",
                    reason,
                })?;
            self.configured_replacement_packs = resolved.clone();
            self.configured_replacement_enabled = enabled;
            let configured_policy_sha = self.configured_runtime_policy().sha256();
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: configured_policy_sha,
                });
            };
            let ffi_inputs =
                replacement_ffi_inputs(&resolved).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason,
                })?;
            if let Err(reason) = context.load_replacement_packs(&ffi_inputs, enabled) {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason,
                });
            }
            let after = resolve_replacement_packs(inputs).map_err(|reason| {
                self.active_replacement_settings = None;
                RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason,
                }
            })?;
            if after != resolved {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason: "replacement-pack bytes changed while RT64 activated them".into(),
                });
            }
            self.active_replacement_settings = Some(RenderReplacementSettings {
                enabled,
                packs: after.into_iter().map(|pack| pack.identity).collect(),
            });
            let policy_sha256 = self
                .active_runtime_policy()
                .ok_or(RenderError::NotReady(
                    "RT64 replacement load has no complete active runtime policy",
                ))?
                .sha256();
            Ok(RenderPolicyApply::LiveApplied { policy_sha256 })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (inputs, enabled);
            Err(RenderError::Backend {
                backend: "rt64-replacement-inspect",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Re-inspect and reload the currently configured ordered pack paths.
    pub fn reload_replacement_packs(&mut self) -> Result<RenderPolicyApply, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let inputs: Vec<_> = self
                .configured_replacement_packs
                .iter()
                .map(|pack| pack.input.clone())
                .collect();
            let enabled = self.configured_replacement_enabled;
            let resolved =
                resolve_replacement_packs(&inputs).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                })?;
            self.configured_replacement_packs = resolved.clone();
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: self.configured_runtime_policy().sha256(),
                });
            };
            let ffi_inputs =
                replacement_ffi_inputs(&resolved).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                })?;
            if let Err(reason) = context.reload_replacement_packs(&ffi_inputs, enabled) {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                });
            }
            let after = resolve_replacement_packs(&inputs).map_err(|reason| {
                self.active_replacement_settings = None;
                RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                }
            })?;
            if after != resolved {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason: "replacement-pack bytes changed while RT64 reloaded them".into(),
                });
            }
            self.active_replacement_settings = Some(RenderReplacementSettings {
                enabled,
                packs: after.into_iter().map(|pack| pack.identity).collect(),
            });
            Ok(RenderPolicyApply::LiveApplied {
                policy_sha256: self
                    .active_runtime_policy()
                    .ok_or(RenderError::NotReady(
                        "RT64 replacement reload has no complete active runtime policy",
                    ))?
                    .sha256(),
            })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-replacement-reload",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    pub fn set_replacements_enabled(
        &mut self,
        enabled: bool,
    ) -> Result<RenderPolicyApply, RenderError> {
        self.configured_replacement_enabled = enabled;
        #[cfg(feature = "rt64")]
        {
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: self.configured_runtime_policy().sha256(),
                });
            };
            if let Err(reason) = context.set_replacement_enabled(enabled) {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-enable",
                    reason,
                });
            }
            let active = self
                .active_replacement_settings
                .as_mut()
                .ok_or(RenderError::NotReady(
                    "RT64 replacement enable has no active pack identity",
                ))?;
            active.enabled = enabled;
            Ok(RenderPolicyApply::LiveApplied {
                policy_sha256: self
                    .active_runtime_policy()
                    .ok_or(RenderError::NotReady(
                        "RT64 replacement enable has no complete active runtime policy",
                    ))?
                    .sha256(),
            })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Ok(RenderPolicyApply::StagedForCreate {
                policy_sha256: self.configured_runtime_policy().sha256(),
            })
        }
    }
}

impl Default for Rt64Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderBackend for Rt64Backend {
    fn release_environment(&self) -> fn64_render::RenderBackendEvidence {
        #[cfg(feature = "rt64")]
        {
            let Some(policy) = self.active_runtime_policy() else {
                return fn64_render::RenderBackendEvidence::Unidentified;
            };
            let identity = Self::release_identity();
            fn64_render::RenderBackendEvidence::Rt64 {
                backend_identity: identity.canonical_id(),
                source_authoritative: identity.is_source_authoritative(),
                settings_sha256: policy.sha256(),
                replacement_packs_active: policy.replacement.enabled
                    && !policy.replacement.packs.is_empty(),
            }
        }

        #[cfg(not(feature = "rt64"))]
        {
            fn64_render::RenderBackendEvidence::Unidentified
        }
    }

    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError> {
        self.last_present_cycle = None;
        self.active_settings = None;
        self.active_enhancement_settings = None;
        self.active_emulator_settings = None;
        self.active_replacement_settings = None;
        #[cfg(feature = "rt64")]
        {
            self.task_index = 0;
            self.context = None;
            let replacement_inputs: Vec<_> = self
                .configured_replacement_packs
                .iter()
                .map(|pack| pack.input.clone())
                .collect();
            let replacements =
                resolve_replacement_packs(&replacement_inputs).map_err(|reason| {
                    RenderError::Backend {
                        backend: "rt64-replacement-create",
                        reason,
                    }
                })?;
            self.configured_replacement_packs = replacements.clone();
            let mut context = ffi::Context::create(
                cfg.width,
                cfg.height,
                &self.configured_settings,
                &self.configured_enhancement_settings,
                &self.configured_emulator_settings,
            )
            .map_err(|reason| RenderError::Backend {
                backend: "rt64",
                reason,
            })?;
            let ffi_inputs =
                replacement_ffi_inputs(&replacements).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-create",
                    reason,
                })?;
            context
                .load_replacement_packs(&ffi_inputs, self.configured_replacement_enabled)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-create",
                    reason,
                })?;
            let replacements_after =
                resolve_replacement_packs(&replacement_inputs).map_err(|reason| {
                    RenderError::Backend {
                        backend: "rt64-replacement-create",
                        reason,
                    }
                })?;
            if replacements_after != replacements {
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-create",
                    reason: "replacement-pack bytes changed while RT64 created the backend".into(),
                });
            }
            self.context = Some(context);
            self.active_settings = Some(self.configured_settings.clone());
            self.active_enhancement_settings = Some(self.configured_enhancement_settings.clone());
            self.active_emulator_settings = Some(self.configured_emulator_settings.clone());
            self.active_replacement_settings = Some(RenderReplacementSettings {
                enabled: self.configured_replacement_enabled,
                packs: replacements_after
                    .into_iter()
                    .map(|pack| pack.identity)
                    .collect(),
            });
            Ok(())
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = cfg;
            self.created = false;
            Err(RenderError::Backend {
                backend: "rt64",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    fn observe_non_rdp_write16(&mut self, _write: NonRdpWrite16) -> NonRdpWrite16Disposition {
        // Native RT64 does not expose its hidden-bit ownership through this
        // Rust adapter. Explicitly report that boundary; acknowledging this
        // event is not evidence of native RT64 hidden-bit parity.
        NonRdpWrite16Disposition::NoRustHiddenSidecar
    }

    fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
        #[cfg(feature = "rt64")]
        {
            let family = match self
                .f3dex2_ucodes
                .require_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))
            {
                Ok(family) => family,
                Err(RenderError::RequiresLle { ucode_sha256 }) => {
                    return Ok(FrameStatus::NeedsLle { ucode_sha256 });
                }
                Err(error) => return Err(error),
            };
            // FullSync is the public source of DP completion. Inspect the
            // exact admitted display-list graph transactionally before RT64
            // consumes it; cloned RDRAM/RSP/RDP state prevents this evidence
            // pass from applying task effects twice.
            let mut inspection_rdram = rdram.to_vec();
            let mut inspection_rsp = rsp_memory.clone();
            let mut inspection_rdp = gbi::RdpDecodeState::default();
            let inspection = match gbi::execute_display_list_geometry_ops_admitted_with_rdp_state(
                &mut inspection_rdram,
                &mut inspection_rsp,
                task.data_ptr,
                &self.f3dex2_ucodes,
                &mut inspection_rdp,
                family,
            ) {
                Ok(operations) => operations,
                Err(RenderError::RequiresLle { ucode_sha256 }) => {
                    return Ok(FrameStatus::NeedsLle { ucode_sha256 });
                }
                Err(error) => return Err(error),
            };
            let full_sync = if inspection
                .iter()
                .any(|operation| matches!(operation, gbi::RenderOp::FullSync))
            {
                fn64_render::DpFullSyncStatus::Reached
            } else {
                fn64_render::DpFullSyncStatus::NotReached
            };
            let task_index = self.task_index;
            self.task_index += 1;
            if let Some(spec) = std::env::var_os("FN64_GFX_TASK_DUMP") {
                let selected = spec.to_string_lossy().split(',').any(|entry| {
                    entry.trim().parse::<u64>().unwrap_or_else(|error| {
                        panic!(
                            "FN64_GFX_TASK_DUMP entry {entry:?} is not a u64 task index: {error}"
                        )
                    }) == task_index
                });
                if selected {
                    let directory = std::env::var_os("FN64_GFX_TASK_DUMP_DIR")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/fn64-gfx-task-dumps"));
                    std::fs::create_dir_all(&directory).unwrap_or_else(|error| {
                        panic!("failed to create FN64_GFX_TASK_DUMP_DIR {directory:?}: {error}")
                    });
                    let mut diagnostic_rdram = rdram.to_vec();
                    let mut diagnostic_rsp = rsp_memory.clone();
                    let mut diagnostic_rdp = gbi::RdpDecodeState::default();
                    let triangles = gbi::execute_display_list_geometry_ops_admitted_with_rdp_state(
                        &mut diagnostic_rdram,
                        &mut diagnostic_rsp,
                        task.data_ptr,
                        &self.f3dex2_ucodes,
                        &mut diagnostic_rdp,
                        family,
                    )
                    .unwrap_or_else(|error| {
                        panic!("failed to decode diagnostic gfx task {task_index}: {error}")
                    })
                    .into_iter()
                    .filter(|operation| matches!(operation, gbi::RenderOp::Triangle(_)))
                    .count();
                    let command_trace =
                        gbi::trace_display_list_f3dex2(&diagnostic_rdram, task.data_ptr);
                    let report = format!(
                        "task_index={task_index}\noutput_addr={output_addr:#010x}\n\
                         reference_triangle_count={}\ntask={task:#?}\n{command_trace}",
                        triangles,
                    );
                    let path = directory.join(format!("task-{task_index:04}.txt"));
                    std::fs::write(&path, report).unwrap_or_else(|error| {
                        panic!("failed to write gfx task diagnostic {path:?}: {error}")
                    });
                    eprintln!(
                        "[fn64-render-rt64] dumped gfx task #{task_index} ({} reference \
                         triangles) to {path:?}",
                        triangles
                    );
                }
            }
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_task(rdram, rsp_memory, task, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64",
                    reason,
                })?;
            self.last_dp_full_sync = full_sync;
            Ok(FrameStatus::Complete)
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, rsp_memory, task, output_addr);
            Err(RenderError::NotReady(
                "Rt64Backend is unavailable without the `rt64` Cargo feature",
            ))
        }
    }

    fn process_rdp_commands(
        &mut self,
        rdram: &mut [u8],
        start: u32,
        end: u32,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
        #[cfg(feature = "rt64")]
        {
            let start_usize = usize::try_from(start).expect("u32 RDP start fits usize");
            let end_usize = usize::try_from(end).expect("u32 RDP end fits usize");
            if start >= end
                || !start.is_multiple_of(8)
                || !end.is_multiple_of(8)
                || end_usize > rdram.len()
            {
                return Err(RenderError::InvalidTaskBounds {
                    offset: start,
                    len: end.saturating_sub(start),
                    rdram_len: rdram.len(),
                });
            }
            debug_assert!(start_usize < end_usize);
            let full_sync = gbi::raw_rdp_full_sync_status(rdram, start, end)?;
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_rdp_commands(rdram, start, end, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64",
                    reason,
                })?;
            self.last_dp_full_sync = full_sync;
            Ok(FrameStatus::Complete)
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, start, end, output_addr);
            Err(RenderError::NotReady(
                "Rt64Backend is unavailable without the `rt64` Cargo feature",
            ))
        }
    }

    fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
        self.last_dp_full_sync
    }

    fn task_chunking(&self) -> fn64_render::RenderTaskChunking {
        // RT64's public task entry is presently one synchronous native call;
        // the adapter cannot manufacture a resumable native stack.
        fn64_render::RenderTaskChunking::Atomic
    }

    fn present(&mut self, vi: ViPresentation) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .present(vi)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64",
                    reason,
                })?;
            self.last_present_cycle = Some(vi.noise_seed);
            Ok(())
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = vi;
            Err(RenderError::NotReady(
                "Rt64Backend is unavailable without the `rt64` Cargo feature",
            ))
        }
    }

    fn release_capture(&mut self) -> Result<fn64_render::RenderReleaseCapture, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let guest_cycle = self.last_present_cycle.ok_or(RenderError::NotReady(
                "RT64 release capture requested before a completed VI present",
            ))?;
            let replacement_inputs: Vec<_> = self
                .configured_replacement_packs
                .iter()
                .map(|pack| pack.input.clone())
                .collect();
            let replacement_enabled = self
                .active_replacement_settings
                .as_ref()
                .ok_or(RenderError::NotReady(
                    "RT64 release capture has no active replacement identity",
                ))?
                .enabled;
            let current_replacements = match resolve_replacement_packs(&replacement_inputs) {
                Ok(packs) => RenderReplacementSettings {
                    enabled: replacement_enabled,
                    packs: packs.into_iter().map(|pack| pack.identity).collect(),
                },
                Err(reason) => {
                    self.active_replacement_settings = None;
                    return Err(RenderError::Backend {
                        backend: "rt64-release-capture",
                        reason: format!(
                            "active replacement packs could not be revalidated: {reason}"
                        ),
                    });
                }
            };
            if self.active_replacement_settings.as_ref() != Some(&current_replacements) {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-release-capture",
                    reason: "active replacement-pack bytes changed after activation; reload or recreate before capture".into(),
                });
            }
            let identity = Self::release_identity();
            let settings_sha256 = self
                .active_runtime_policy()
                .ok_or(RenderError::NotReady(
                    "RT64 release capture has no complete active runtime policy",
                ))?
                .sha256();
            let mut pixels = self.presented_pixels()?;
            let workload_id = std::num::NonZeroU64::new(pixels.workload_id).ok_or_else(|| {
                RenderError::Backend {
                    backend: "rt64-release-capture",
                    reason: "completed post-VI pixels have a zero RT64 workload ID".into(),
                }
            })?;
            let format = match pixels.format {
                Rt64PresentPixelFormat::Bgra8Unorm => {
                    fn64_render::ReleaseCaptureFormat::PostViBgra8Unorm
                }
                Rt64PresentPixelFormat::Rgba8Unorm => {
                    for pixel in pixels.bytes.chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                    }
                    fn64_render::ReleaseCaptureFormat::PostViBgra8Unorm
                }
            };
            Ok(fn64_render::RenderReleaseCapture {
                guest_cycle,
                backend_identity: identity.canonical_id(),
                source_authoritative: identity.is_source_authoritative(),
                settings_sha256,
                width: pixels.width,
                height: pixels.height,
                row_bytes: pixels.row_bytes,
                format,
                workload_id,
                present_id: pixels.present_id,
                bytes: pixels.bytes,
            })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-release-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    fn apply_runtime_settings(
        &mut self,
        settings: &RenderRuntimeSettings,
    ) -> Result<RenderSettingsApply, RenderError> {
        self.configured_settings = settings.clone();

        #[cfg(feature = "rt64")]
        {
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderSettingsApply::StagedForCreate {
                    settings_sha256: settings.sha256(),
                });
            };
            let active = self.active_settings.as_ref().ok_or(RenderError::NotReady(
                "RT64 context exists without active runtime settings",
            ))?;
            let restart_fields = settings.restart_changes_from(active);
            if !restart_fields.is_empty() {
                return Ok(RenderSettingsApply::RestartRequired {
                    fields: restart_fields,
                    active_settings_sha256: active.sha256(),
                    requested_settings_sha256: settings.sha256(),
                });
            }
            let framebuffers_discarded = match context.apply_user_config(settings) {
                Ok(discarded) => discarded,
                Err(reason) => {
                    // An exception after RT64 begins its resource-update path
                    // cannot be rolled back transactionally. Forgetting the
                    // active identity forces recreation before any release
                    // capture can claim which configuration produced it.
                    self.active_settings = None;
                    return Err(RenderError::Backend {
                        backend: "rt64-settings",
                        reason,
                    });
                }
            };
            self.active_settings = Some(settings.clone());
            Ok(RenderSettingsApply::LiveApplied {
                settings_sha256: settings.sha256(),
                framebuffers_discarded,
            })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Ok(RenderSettingsApply::StagedForCreate {
                settings_sha256: settings.sha256(),
            })
        }
    }

    fn apply_enhancement_settings(
        &mut self,
        settings: &RenderEnhancementSettings,
    ) -> Result<RenderPolicyApply, RenderError> {
        self.configured_enhancement_settings = settings.clone();

        #[cfg(feature = "rt64")]
        {
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: self.configured_runtime_policy().sha256(),
                });
            };
            if let Err(reason) = context.apply_enhancement_config(settings) {
                self.active_enhancement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-enhancement-settings",
                    reason,
                });
            }
            self.active_enhancement_settings = Some(settings.clone());
            let policy_sha256 = self
                .active_runtime_policy()
                .ok_or(RenderError::NotReady(
                    "RT64 enhancement apply has no complete active runtime policy",
                ))?
                .sha256();
            Ok(RenderPolicyApply::LiveApplied { policy_sha256 })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Ok(RenderPolicyApply::StagedForCreate {
                policy_sha256: self.configured_runtime_policy().sha256(),
            })
        }
    }

    fn apply_emulator_settings(
        &mut self,
        settings: &RenderEmulatorSettings,
    ) -> Result<RenderPolicyApply, RenderError> {
        self.configured_emulator_settings = settings.clone();

        #[cfg(feature = "rt64")]
        {
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: self.configured_runtime_policy().sha256(),
                });
            };
            if let Err(reason) = context.apply_emulator_config(settings) {
                self.active_emulator_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-emulator-settings",
                    reason,
                });
            }
            self.active_emulator_settings = Some(settings.clone());
            let policy_sha256 = self
                .active_runtime_policy()
                .ok_or(RenderError::NotReady(
                    "RT64 emulator apply has no complete active runtime policy",
                ))?
                .sha256();
            Ok(RenderPolicyApply::LiveApplied { policy_sha256 })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Ok(RenderPolicyApply::StagedForCreate {
                policy_sha256: self.configured_runtime_policy().sha256(),
            })
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        #[cfg(feature = "rt64")]
        if let Some(context) = &mut self.context {
            context.resize(w, h);
        }

        #[cfg(not(feature = "rt64"))]
        let _ = (w, h);
    }

    fn supported_ucodes(&self) -> &[UcodeId] {
        #[cfg(feature = "rt64")]
        {
            self.f3dex2_ucodes.supported_ucodes()
        }

        #[cfg(not(feature = "rt64"))]
        {
            &[]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "rt64")]
    struct SyntheticPack(PathBuf);

    #[cfg(feature = "rt64")]
    impl SyntheticPack {
        fn new(name: &str, auto_path: &str, operation: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "fn64-rt64-pack-{}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
                name
            ));
            std::fs::create_dir(&path).expect("create synthetic replacement pack");
            let database = format!(
                "{{\"configuration\":{{\"configurationVersion\":3,\"autoPath\":\"{auto_path}\",\"defaultOperation\":\"{operation}\",\"defaultShift\":\"half\",\"hashVersion\":5}},\"textures\":[],\"operationFilters\":[],\"shiftFilters\":[],\"extraFiles\":[]}}"
            );
            std::fs::write(path.join("rt64.json"), database)
                .expect("write synthetic replacement database");
            Self(path)
        }

        fn input(&self) -> Rt64ReplacementPackInput {
            Rt64ReplacementPackInput::new(&self.0)
        }
    }

    #[cfg(feature = "rt64")]
    impl Drop for SyntheticPack {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove synthetic replacement pack");
        }
    }

    #[test]
    #[cfg(feature = "rt64")]
    fn replacement_pack_inspection_is_ordered_typed_and_staged_without_active_evidence() {
        let first = SyntheticPack::new("first", "rt64", "preload");
        let second = SyntheticPack::new("second", "rice", "stall");
        std::fs::write(first.0.join("extra.bin"), b"first-content")
            .expect("write synthetic pack content");

        let mut backend = Rt64Backend::new();
        let inputs = [first.input(), second.input()];
        let applied = backend.load_replacement_packs(&inputs, false).unwrap();
        let replacement = backend.configured_replacement_settings();
        assert!(!replacement.enabled);
        assert_eq!(replacement.packs.len(), 2);
        assert_eq!(
            replacement.packs[0].auto_path,
            fn64_render::RenderReplacementAutoPath::Rt64
        );
        assert_eq!(
            replacement.packs[0].default_operation,
            fn64_render::RenderReplacementOperation::Preload
        );
        assert_eq!(
            replacement.packs[1].auto_path,
            fn64_render::RenderReplacementAutoPath::Rice
        );
        assert_eq!(
            replacement.packs[1].default_operation,
            fn64_render::RenderReplacementOperation::Stall
        );
        assert_ne!(
            replacement.packs[0].content_sha256,
            replacement.packs[1].content_sha256
        );
        assert_ne!(
            replacement.packs[0].database_sha256,
            replacement.packs[1].database_sha256
        );
        assert_eq!(backend.active_replacement_settings(), None);
        assert_eq!(
            applied,
            RenderPolicyApply::StagedForCreate {
                policy_sha256: backend.configured_runtime_policy().sha256()
            }
        );

        let reversed = resolve_replacement_packs(&[second.input(), first.input()]).unwrap();
        let reversed_policy = RenderReplacementSettings {
            enabled: false,
            packs: reversed.into_iter().map(|pack| pack.identity).collect(),
        };
        assert_ne!(replacement.sha256(), reversed_policy.sha256());
    }

    #[test]
    #[cfg(feature = "rt64")]
    fn replacement_pack_inspection_rejects_ambiguous_or_silently_ignored_inputs() {
        let pack = SyntheticPack::new("duplicate", "rt64", "stream");
        let duplicate = resolve_replacement_packs(&[pack.input(), pack.input()]).unwrap_err();
        assert!(duplicate.contains("duplicated"));

        std::fs::write(
            pack.0.join("rt64.json"),
            b"{\"configuration\":{\"hashVersion\":999}}",
        )
        .expect("write unsupported synthetic database");
        let unsupported = resolve_replacement_packs(&[pack.input()]).unwrap_err();
        assert!(unsupported.contains("newer than pinned RT64"));

        std::fs::write(
            pack.0.join("rt64.json"),
            b"{\"configuration\":{\"autoPath\":\"guess\"}}",
        )
        .expect("write ambiguous synthetic database");
        let ambiguous = resolve_replacement_packs(&[pack.input()]).unwrap_err();
        assert!(ambiguous.contains("unknown autoPath"));
    }

    #[test]
    #[cfg(not(feature = "rt64"))]
    fn rt64_backend_without_feature_is_a_named_error_not_a_silent_success() {
        let mut backend = Rt64Backend::new();
        assert_eq!(
            backend.task_chunking(),
            fn64_render::RenderTaskChunking::Atomic
        );
        let err = backend.create(&RenderConfig::new(320, 240)).unwrap_err();
        match err {
            RenderError::Backend { backend, .. } => assert_eq!(backend, "rt64"),
            other => panic!("expected Backend stub error, got {other:?}"),
        }
        assert!(!backend.created);
        assert!(backend.supported_ucodes().is_empty());
    }

    #[test]
    #[cfg(not(feature = "rt64"))]
    fn rt64_settings_stage_before_create_without_claiming_an_active_image() {
        let mut backend = Rt64Backend::new();
        let settings = RenderRuntimeSettings::upstream_default();
        assert_eq!(
            backend.apply_runtime_settings(&settings).unwrap(),
            RenderSettingsApply::StagedForCreate {
                settings_sha256: settings.sha256()
            }
        );
        assert_eq!(backend.configured_settings(), &settings);
        assert_eq!(backend.active_settings(), None);

        let enhancement = RenderEnhancementSettings::upstream_default();
        let expected_policy = RenderRuntimePolicy {
            user: settings,
            enhancement: enhancement.clone(),
            emulator: RenderEmulatorSettings::default(),
            replacement: fn64_render::RenderReplacementSettings::default(),
        };
        assert_eq!(
            backend.apply_enhancement_settings(&enhancement).unwrap(),
            RenderPolicyApply::StagedForCreate {
                policy_sha256: expected_policy.sha256()
            }
        );
        let emulator = RenderEmulatorSettings {
            post_blend_noise: false,
            ..RenderEmulatorSettings::default()
        };
        let expected_policy = RenderRuntimePolicy {
            emulator: emulator.clone(),
            ..expected_policy
        };
        assert_eq!(
            backend.apply_emulator_settings(&emulator).unwrap(),
            RenderPolicyApply::StagedForCreate {
                policy_sha256: expected_policy.sha256()
            }
        );
        assert_eq!(backend.configured_runtime_policy(), expected_policy);
        assert_eq!(backend.active_runtime_policy(), None);
    }

    #[test]
    fn reference_backend_create_then_present_succeeds_with_no_geometry() {
        let mut backend = ReferenceBackend::new();
        assert_eq!(
            backend.task_chunking(),
            fn64_render::RenderTaskChunking::Resumable
        );
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        backend.present(ViPresentation::default()).unwrap();
        assert!(!backend
            .framebuffer()
            .unwrap()
            .has_non_uniform_content(0, 0, 0, 255));
    }

    #[test]
    fn reference_backend_chunks_at_committed_operations_and_consumes_tokens_once() {
        const DL: usize = 0x100;
        const TARGET: u32 = 0x400;
        let make_rdram = || {
            let mut rdram = vec![0u8; 0x1000];
            let commands: [(u32, u32); 8] = [
                (0xef00_0000 | (3 << 20), 0),
                (0xff10_0003, TARGET),
                (0xf700_0000, 0xf801_f801),
                (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
                (0xe900_0000, 0),
                (0xf700_0000, 0x003f_003f),
                (0xf600_0000 | ((2 * 4) << 12), 4 << 12),
                (0xdf00_0000, 0),
            ];
            for (index, (w0, w1)) in commands.into_iter().enumerate() {
                let offset = DL + index * 8;
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            }
            rdram
        };
        let task = OsTask {
            task_type: fn64_render::M_GFXTASK,
            data_ptr: DL as u32,
            ..OsTask::default()
        };
        let make_backend = || {
            let mut backend = ReferenceBackend::new()
                .with_f3dex2()
                .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
            backend.create(&RenderConfig::new(4, 2)).unwrap();
            backend
        };

        let mut chunked = make_backend();
        let mut chunked_rdram = make_rdram();
        let mut chunked_rsp = fn64_runtime::RspMemory::new();
        let first = match chunked
            .process_task_chunk(
                &mut chunked_rdram,
                &mut chunked_rsp,
                &task,
                0,
                fn64_render::RenderTaskStep::Start,
            )
            .unwrap()
        {
            fn64_render::RenderTaskChunkStatus::Continue(token) => token,
            status => panic!("SETCIMG boundary did not retain a continuation: {status:?}"),
        };
        assert_eq!(
            chunked.last_dp_full_sync(),
            fn64_render::DpFullSyncStatus::NotReached
        );
        let second = match chunked
            .process_task_chunk(
                &mut chunked_rdram,
                &mut chunked_rsp,
                &task,
                0,
                fn64_render::RenderTaskStep::Resume(first),
            )
            .unwrap()
        {
            fn64_render::RenderTaskChunkStatus::Continue(token) => token,
            status => panic!("first fill boundary did not retain a continuation: {status:?}"),
        };
        assert_ne!(first, second);
        let red_boundary = chunked_rdram.clone();
        let stale = chunked
            .process_task_chunk(
                &mut chunked_rdram,
                &mut chunked_rsp,
                &task,
                0,
                fn64_render::RenderTaskStep::Resume(first),
            )
            .unwrap_err();
        assert!(stale.to_string().contains("does not own retained token"));
        assert_eq!(chunked_rdram, red_boundary, "stale token replayed a fill");
        let overlapping_start = chunked
            .process_task_chunk(
                &mut chunked_rdram,
                &mut chunked_rsp,
                &task,
                0,
                fn64_render::RenderTaskStep::Start,
            )
            .unwrap_err();
        assert!(overlapping_start
            .to_string()
            .contains("cannot start a new task"));

        let third = match chunked
            .process_task_chunk(
                &mut chunked_rdram,
                &mut chunked_rsp,
                &task,
                0,
                fn64_render::RenderTaskStep::Resume(second),
            )
            .unwrap()
        {
            fn64_render::RenderTaskChunkStatus::Continue(token) => token,
            status => panic!("FullSync boundary did not retain a continuation: {status:?}"),
        };
        assert_eq!(
            chunked.last_dp_full_sync(),
            fn64_render::DpFullSyncStatus::Reached,
            "FullSync evidence must be published at its committed boundary"
        );
        assert_eq!(
            chunked
                .process_task_chunk(
                    &mut chunked_rdram,
                    &mut chunked_rsp,
                    &task,
                    0,
                    fn64_render::RenderTaskStep::Resume(third),
                )
                .unwrap(),
            fn64_render::RenderTaskChunkStatus::Complete
        );
        assert_eq!(
            chunked.last_dp_full_sync(),
            fn64_render::DpFullSyncStatus::Reached
        );
        let completed_rdram = chunked_rdram.clone();
        let consumed = chunked
            .process_task_chunk(
                &mut chunked_rdram,
                &mut chunked_rsp,
                &task,
                0,
                fn64_render::RenderTaskStep::Resume(third),
            )
            .unwrap_err();
        assert!(consumed
            .to_string()
            .contains("stale or was already consumed"));
        assert_eq!(chunked_rdram, completed_rdram);

        let mut atomic = make_backend();
        let mut atomic_rdram = make_rdram();
        atomic
            .process_task(
                &mut atomic_rdram,
                &mut fn64_runtime::RspMemory::new(),
                &task,
                0,
            )
            .unwrap();
        assert_eq!(chunked_rdram, atomic_rdram);
        assert_eq!(
            chunked.framebuffer().unwrap().pixels,
            atomic.framebuffer().unwrap().pixels
        );
    }

    #[test]
    fn reference_backend_noise_seed_is_selectable_and_survives_resize() {
        let mut backend = ReferenceBackend::new().with_noise_seed(7);
        backend.create(&RenderConfig::new(4, 4)).unwrap();
        assert_eq!(backend.fb.as_ref().unwrap().noise_position(), (7, 0));

        let vertex = |x, y| gbi::Vertex {
            x,
            y,
            r: 255,
            g: 255,
            b: 255,
            a: 255,
            w: 1.0,
            ..gbi::Vertex::default()
        };
        backend.fb.as_mut().unwrap().draw_triangle(&gbi::Triangle {
            v: [vertex(0.0, 0.0), vertex(4.0, 0.0), vertex(0.0, 4.0)],
            ..gbi::Triangle::default()
        });
        let position = backend.fb.as_ref().unwrap().noise_position();
        assert!(position.1 > 0);

        backend.resize(8, 8);
        assert_eq!(backend.fb.as_ref().unwrap().noise_position(), position);
    }

    #[test]
    fn reference_backend_blanks_scanout_without_destroying_the_rdp_image() {
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::new(2, 1)).unwrap();
        backend.fb.as_mut().unwrap().pixels[0..4].copy_from_slice(&[9, 8, 7, 255]);

        backend.present(ViPresentation::default()).unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[0..4],
            &[9, 8, 7, 255]
        );

        backend
            .present(ViPresentation {
                blanked: true,
                ..ViPresentation::default()
            })
            .unwrap();
        assert!(backend
            .presented_framebuffer()
            .unwrap()
            .pixels
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 0, 255]));
        assert_eq!(
            &backend.framebuffer().unwrap().pixels[0..4],
            &[9, 8, 7, 255]
        );

        backend.present(ViPresentation::default()).unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[0..4],
            &[9, 8, 7, 255]
        );
    }

    #[test]
    fn reference_backend_executes_public_fade_and_repeat_line_scanout() {
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::new(2, 2)).unwrap();
        backend.fb.as_mut().unwrap().pixels.copy_from_slice(&[
            10, 20, 30, 255, 40, 50, 60, 255, 110, 120, 130, 255, 140, 150, 160, 255,
        ]);

        backend
            .present(ViPresentation {
                fade: Some(0x03ff),
                ..ViPresentation::default()
            })
            .unwrap();
        assert_eq!(
            backend.presented_framebuffer().unwrap().pixels,
            [110, 120, 130, 255, 140, 150, 160, 255, 110, 120, 130, 255, 140, 150, 160, 255,]
        );

        backend
            .present(ViPresentation {
                repeat_line: true,
                ..ViPresentation::default()
            })
            .unwrap();
        assert_eq!(
            backend.presented_framebuffer().unwrap().pixels,
            [10, 20, 30, 255, 40, 50, 60, 255, 10, 20, 30, 255, 40, 50, 60, 255,]
        );
    }

    #[test]
    fn reference_backend_executes_vi_dither_divot_and_gamma_filters() {
        let rgba16 = fn64_render::ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            dither_filter: true,
            ..Default::default()
        };
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::new(3, 3)).unwrap();
        let fb = backend.fb.as_mut().unwrap();
        for pixel in fb.pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[88, 88, 88, 255]);
        }
        fb.pixels[4 * 4..4 * 4 + 4].copy_from_slice(&[80, 80, 80, 255]);
        backend
            .present(ViPresentation {
                filters: rgba16,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[4 * 4..4 * 4 + 4],
            &[88, 88, 88, 255]
        );

        let fb = backend.fb.as_mut().unwrap();
        fb.pixels[0..12].copy_from_slice(&[10, 10, 10, 255, 200, 200, 200, 255, 20, 20, 20, 255]);
        fb.coverage[1] = raster::Coverage::new(4);
        backend
            .present(ViPresentation {
                filters: fn64_render::ViFilterControl {
                    pixel_type: ViPixelType::Rgba16,
                    divot: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[4..8],
            &[20, 20, 20, 255]
        );

        backend.fb.as_mut().unwrap().pixels[0..4].copy_from_slice(&[64, 0, 255, 255]);
        backend
            .present(ViPresentation {
                filters: fn64_render::ViFilterControl {
                    pixel_type: ViPixelType::Rgba32,
                    gamma: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[0..4],
            &[127, 0, 255, 255]
        );
    }

    #[test]
    fn reference_backend_gamma_dither_is_seeded_and_frame_varying() {
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::new(1, 1)).unwrap();
        backend.fb.as_mut().unwrap().pixels[0..4].copy_from_slice(&[101, 101, 101, 255]);
        let presentation = |noise_seed| ViPresentation {
            filters: fn64_render::ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                gamma_dither: true,
                ..Default::default()
            },
            noise_seed,
            ..Default::default()
        };
        backend.present(presentation(0)).unwrap();
        let first = backend.presented_framebuffer().unwrap().pixels[0..3].to_vec();
        backend.present(presentation(0)).unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[0..3],
            first
        );

        let variants = (0..64)
            .map(|seed| {
                backend.present(presentation(seed)).unwrap();
                backend.presented_framebuffer().unwrap().pixels[0]
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(variants, [100, 102].into_iter().collect());
    }

    #[test]
    fn reference_backend_rejects_process_task_before_create() {
        let mut backend = ReferenceBackend::new();
        let mut rdram = vec![0u8; 64];
        let err = backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask::default(),
                0,
            )
            .unwrap_err();
        assert!(matches!(err, RenderError::NotReady(_)));
    }

    #[test]
    fn reference_backend_lle_preflight_is_transactional() {
        const DL: usize = 0x1000;
        const TEXT: usize = 0x2000;
        const DATA: usize = 0x3200;
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0x11; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        let mut rdram = vec![0u8; 0x4000];
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(TEXT as u32),
            &[0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        let write_word = |rdram: &mut [u8], offset: usize, word: u32| {
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        };
        write_word(&mut rdram, DL, 0xe100_0000);
        write_word(&mut rdram, DL + 4, DATA as u32);
        write_word(&mut rdram, DL + 8, 0xdd00_0007);
        write_word(&mut rdram, DL + 12, TEXT as u32);
        write_word(&mut rdram, DL + 16, 0xd500_0000);
        write_word(&mut rdram, DL + 20, 0);

        let mut rsp_memory = fn64_runtime::RspMemory::new();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &[0x11; fn64_runtime::RSP_MEMORY_BANK_SIZE],
            )
            .unwrap();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0x40),
                b"task-entry",
            )
            .unwrap();
        let rdram_before = rdram.clone();
        let rsp_before = rsp_memory.clone();
        let status = backend
            .process_task(
                &mut rdram,
                &mut rsp_memory,
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        assert_eq!(
            status,
            FrameStatus::NeedsLle {
                ucode_sha256: gbi::UcodeDigest::from_text(
                    &[0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE]
                )
                .as_bytes(),
            }
        );
        assert_eq!(rdram, rdram_before);
        assert_eq!(rsp_memory, rsp_before);
    }

    #[test]
    fn reference_backend_selects_l3dex_wire_family_from_admitted_imem_digest() {
        const DL: usize = 0x1000;
        let text = [0x4c; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let mut backend =
            ReferenceBackend::new().with_geometry_ucode_text(GeometryWireFamily::L3dex, &text);
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        assert_eq!(backend.supported_ucodes(), &[UcodeId::L3dex]);

        let mut rdram = vec![0u8; 0x2000];
        rdram[DL..DL + 4].copy_from_slice(&0xb800_0000u32.to_ne_bytes());
        rdram[DL + 4..DL + 8].copy_from_slice(&0u32.to_ne_bytes());
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &text,
            )
            .unwrap();

        assert_eq!(
            backend
                .process_task(
                    &mut rdram,
                    &mut rsp_memory,
                    &OsTask {
                        task_type: fn64_render::M_GFXTASK,
                        data_ptr: DL as u32,
                        ..OsTask::default()
                    },
                    0,
                )
                .unwrap(),
            FrameStatus::Complete
        );
    }

    #[test]
    fn reference_backend_reports_only_admitted_polygon_wire_families() {
        let fast3d = [0x31; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let f3dex = [0x32; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let f3dlx = [0x33; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let f3dlx_rej = [0x34; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let f3dex2 = [0x35; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let f3dex2_non = [0x36; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let f3dex2_rej = [0x37; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let f3dlx2_rej = [0x38; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let backend = ReferenceBackend::new()
            .with_geometry_ucode_text(GeometryWireFamily::F3dlx2Rej, &f3dlx2_rej)
            .with_geometry_ucode_text(GeometryWireFamily::F3dex2Rej, &f3dex2_rej)
            .with_geometry_ucode_text(GeometryWireFamily::F3dex2NoN, &f3dex2_non)
            .with_geometry_ucode_text(GeometryWireFamily::F3dex2, &f3dex2)
            .with_geometry_ucode_text(GeometryWireFamily::F3dlxRej, &f3dlx_rej)
            .with_geometry_ucode_text(GeometryWireFamily::F3dlx, &f3dlx)
            .with_geometry_ucode_text(GeometryWireFamily::F3dex, &f3dex)
            .with_geometry_ucode_text(GeometryWireFamily::Fast3d, &fast3d);
        assert_eq!(
            backend.supported_ucodes(),
            &[
                UcodeId::Fast3d,
                UcodeId::F3dex,
                UcodeId::F3dlx,
                UcodeId::F3dlxRej,
                UcodeId::F3dex2,
                UcodeId::F3dex2NoN,
                UcodeId::F3dex2Rej,
                UcodeId::F3dlx2Rej
            ]
        );
    }

    #[test]
    fn reference_backend_requires_exact_task_entry_admission() {
        const DL: usize = 0x100;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        let mut rdram = vec![0u8; 0x200];
        rdram[DL..DL + 4].copy_from_slice(&0xdf00_0000u32.to_ne_bytes());
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &[0x33; fn64_runtime::RSP_MEMORY_BANK_SIZE],
            )
            .unwrap();
        let rdram_before = rdram.clone();
        let rsp_before = rsp_memory.clone();

        let status = backend
            .process_task(
                &mut rdram,
                &mut rsp_memory,
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        assert_eq!(
            status,
            FrameStatus::NeedsLle {
                ucode_sha256: gbi::UcodeDigest::from_text(
                    &[0x33; fn64_runtime::RSP_MEMORY_BANK_SIZE]
                )
                .as_bytes(),
            }
        );
        assert_eq!(rdram, rdram_before);
        assert_eq!(rsp_memory, rsp_before);
    }

    #[test]
    fn raw_depth_image_fill_clears_persistent_depth_across_color_switch() {
        const START: usize = 0x100;
        const Z_IMAGE: u32 = 0x400;
        const COLOR_IMAGE: u32 = 0x600;
        let commands: [(u32, u32); 7] = [
            (0xfe00_0000, Z_IMAGE),
            (0xff10_0003, Z_IMAGE),
            (0xef00_0000 | (3 << 20), 0),
            (0xf700_0000, 0xfffc_fffc),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
            (0xff10_0003, COLOR_IMAGE),
            (0xe900_0000, 0),
        ];
        let mut rdram = vec![0u8; 0x1000];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = START + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        }
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend.fb.as_mut().unwrap().depth.fill(1.0);

        backend
            .process_rdp_commands(
                &mut rdram,
                START as u32,
                (START + commands.len() * 8) as u32,
                0,
            )
            .unwrap();

        assert_eq!(
            backend.depth_image,
            Some(gbi::DepthImage { address: Z_IMAGE })
        );
        assert!(backend
            .fb
            .as_ref()
            .unwrap()
            .depth
            .iter()
            .all(|&value| value == 0x3ffff as f32));
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for pixel in 0..8 {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2)),
                0xfffc
            );
        }
    }

    #[test]
    fn raw_depth_fill_halfwords_replicate_lsbs_into_hidden_delta_bits() {
        const START: usize = 0x100;
        const Z_IMAGE: u32 = 0x400;
        const COLOR_IMAGE: u32 = 0x600;
        let commands: [(u32, u32); 7] = [
            (0xfe00_0000, Z_IMAGE),
            (0xff10_0003, Z_IMAGE),
            (0xef00_0000 | (3 << 20), 0),
            // Both halves retain maximum encoded Z. Their low pairs are 01
            // and 10; MI fill replication supplies hidden pairs 11 and 00,
            // yielding complete stored DeltaZ exponents 7 and 8.
            (0xf700_0000, 0xfffd_fffe),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
            (0xff10_0003, COLOR_IMAGE),
            (0xe900_0000, 0),
        ];
        let mut rdram = vec![0u8; 0x1000];
        for (index, (word0, word1)) in commands.into_iter().enumerate() {
            let offset = START + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
        }
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(4, 2)).unwrap();

        backend
            .process_rdp_commands(
                &mut rdram,
                START as u32,
                (START + commands.len() * 8) as u32,
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let framebuffer = backend.fb.as_ref().unwrap();
        for pixel in 0..8u32 {
            let even = pixel.is_multiple_of(2);
            let address = Z_IMAGE + pixel * 2;
            let visible = if even { 0xfffd } else { 0xfffe };
            let hidden = if even { 3 } else { 0 };
            let delta = if even { 7 } else { 8 };
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(address)),
                visible,
                "visible fill halfword at pixel {pixel}"
            );
            assert_eq!(
                backend.rdram_hidden_bits.get(&address),
                Some(&RdramHiddenSample {
                    visible,
                    bits: hidden,
                }),
                "hidden fill pair at pixel {pixel}"
            );
            assert_eq!(
                depth::unpack(framebuffer.encoded_depth[pixel as usize].unwrap()),
                (0x3ffff, delta),
                "reloaded depth sample at pixel {pixel}"
            );
        }
    }

    #[test]
    fn raw_edge_triangle_rasterizes_into_commanded_color_image() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            command(0xff10_0007, TARGET); // RGBA16 width 8
            command(0xfa00_0000, 0xff00_00ff); // opaque red primitive
            let yh = 4;
            let ym = 4 * 4;
            let yl = 7 * 4;
            command(0x0880_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (4 * 8 + 2) * 2
            )),
            0xf801,
            "raw edge triangle must cover its interior pixel in primitive red"
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0,
            "raw edge triangle must not paint outside its edges"
        );
        let partial_pixel = 4 * 8 + 3;
        assert_eq!(
            backend.fb.as_ref().unwrap().coverage[partial_pixel as usize],
            raster::Coverage::new(6),
            "the raw edge must retain six of the public checkerboard samples"
        );
        let partial_address = TARGET + partial_pixel * 2;
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(partial_address)),
            0xf801
        );
        assert_eq!(
            backend
                .rdram_hidden_bits
                .get(&partial_address)
                .map(|sample| sample.bits),
            Some(1),
            "coverage six stores code five as visible bit 1 plus hidden bits 01"
        );
    }

    #[test]
    fn raw_z_triangles_use_near_zero_depth_regardless_of_submission_order() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        const Z_IMAGE: u32 = 0x600;
        let mut rdram = vec![0u8; 0x1000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for pixel in 0..64 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2),
                    0xfffc,
                );
            }
        }
        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            command(0xfe00_0000, Z_IMAGE);
            command(0xff10_0007, TARGET); // RGBA16 width 8
            command(0xef00_00f0, 0x30); // dither off | Z_CMP | Z_UPD
            command(0xfa00_0000, 0x0000_ffff); // opaque blue primitive
            let yh = 4;
            let ym = 4 * 4;
            let yl = 7 * 4;
            command(0x0980_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(2 << 16, 0); // near plane is Z=0
            command(0, 0);
            command(0xfa00_0000, 0xff00_00ff); // opaque red primitive
            command(0x0980_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(4 << 16, 0); // submitted later, but farther
            command(0, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (4 * 8 + 2) * 2
            )),
            0x003f,
            "near blue raw triangle must reject the later far red fragment"
        );
    }

    #[test]
    fn raw_depth_update_persists_visible_and_hidden_bits_across_image_switches() {
        const START: usize = 0x100;
        const Z_IMAGE_A: u32 = 0x1000;
        const Z_IMAGE_B: u32 = 0x1200;
        const COLOR_IMAGE: u32 = 0x1400;
        let mut rdram = vec![0u8; 0x2000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for pixel in 0..64 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(Z_IMAGE_A + pixel * 2),
                    0xfffc,
                );
            }
        }

        let mut offset = START;
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        let triangle = 0x0980_0000 | yl;
        let edge_ym_yh = (ym << 16) | yh;
        let major_slope = (5.0f32 / 3.0 * 65536.0).round() as u32;
        let minor_slope = (5.0f32 / 6.0 * 65536.0).round() as u32;

        command(0xfe00_0000, Z_IMAGE_A);
        command(0xff10_0007, COLOR_IMAGE);
        command(0xef00_00f0, 0x30); // dither off | Z_CMP | Z_UPD
        command(0xfa00_0000, 0x0000_ffff); // opaque blue primitive
        command(triangle, edge_ym_yh);
        command(1 << 16, major_slope);
        command(1 << 16, minor_slope);
        command(1 << 16, 0);
        command(8 << 16, 0); // working Z = 64
        command(0, 4 << 16); // DeltaZ = |0| + |4|, then *8 = 32
        command(0xfe00_0000, Z_IMAGE_B); // commits A, then loads B
        command(0xfe00_0000, Z_IMAGE_A); // reloads A, including hidden bits
        command(0xef00_00f0, 0x10); // dither off, compare only: must not mutate A
        command(0xfa00_0000, 0xff00_00ff); // opaque red primitive
        command(triangle, edge_ym_yh);
        command(1 << 16, major_slope);
        command(1 << 16, minor_slope);
        command(1 << 16, 0);
        command(16 << 16, 0); // farther working Z = 128, rejected
        command(0, 0);
        command(0xe900_0000, 0);
        let end = offset;

        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let pixel = 4 * 8 + 2;
        let address = Z_IMAGE_A + pixel * 2;
        let expected = depth::pack(64, 32);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(address)),
            expected.visible
        );
        assert_eq!(
            backend
                .rdram_hidden_bits
                .get(&address)
                .map(|sample| sample.bits),
            Some(expected.hidden)
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                COLOR_IMAGE + pixel * 2
            )),
            0x003f,
            "far compare-only red fragment must not replace the persisted near blue sample"
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(Z_IMAGE_B + pixel * 2)),
            0,
            "switching through a second depth image must not alias its visible samples"
        );
    }

    #[test]
    fn raw_primitive_depth_supplies_z_and_delta_without_triangle_coefficients() {
        const START: usize = 0x100;
        const Z_IMAGE: u32 = 0x1000;
        const COLOR_IMAGE: u32 = 0x1400;
        let mut rdram = vec![0u8; 0x2000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for pixel in 0..64 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2),
                    0xfffc,
                );
            }
        }
        let mut offset = START;
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        command(0xfe00_0000, Z_IMAGE);
        command(0xff10_0007, COLOR_IMAGE);
        command(0xee00_0000, (8 << 16) | 32); // primitive Z=8, DeltaZ=32
        command(0xef00_00f0, 0x34); // dither off | G_ZS_PRIM | Z_CMP | Z_UPD
        command(0xfa00_0000, 0x0000_ffff); // opaque blue primitive
        command(0x0880_0000 | yl, (ym << 16) | yh); // no Z coefficient words
        command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
        command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
        command(1 << 16, 0);
        command(0xe900_0000, 0);
        let end = offset;

        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let pixel = 4 * 8 + 2;
        let depth_address = Z_IMAGE + pixel * 2;
        let expected = depth::pack(8 << 3, 32);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(depth_address)),
            expected.visible
        );
        assert_eq!(
            backend
                .rdram_hidden_bits
                .get(&depth_address)
                .map(|sample| sample.bits),
            Some(expected.hidden)
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                COLOR_IMAGE + pixel * 2
            )),
            0x003f
        );
        assert_eq!(
            backend.primitive_depth,
            Some(gbi::PrimitiveDepth { z: 8, delta_z: 32 })
        );
    }

    #[test]
    fn raw_decal_mode_accepts_correlated_depth_and_rejects_behind_depth() {
        const START: usize = 0x100;
        const Z_IMAGE: u32 = 0x1000;
        const COLOR_IMAGE: u32 = 0x1400;
        let mut rdram = vec![0u8; 0x2000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for pixel in 0..64 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2),
                    0xfffc,
                );
            }
        }
        let mut offset = START;
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        let triangle = 0x0880_0000 | yl;
        let edge_ym_yh = (ym << 16) | yh;
        let major_slope = (5.0f32 / 3.0 * 65536.0).round() as u32;
        let minor_slope = (5.0f32 / 6.0 * 65536.0).round() as u32;

        command(0xfe00_0000, Z_IMAGE);
        command(0xff10_0007, COLOR_IMAGE);
        command(0xef00_00f0, 0x34); // dither off | G_ZS_PRIM | Z_CMP | Z_UPD | ZMODE_OPA
        command(0xee00_0000, (16 << 16) | 8); // working Z=128, DeltaZ=8
        command(0xfa00_0000, 0x0000_ffff); // blue depth seed
        command(triangle, edge_ym_yh);
        command(1 << 16, major_slope);
        command(1 << 16, minor_slope);
        command(1 << 16, 0);
        command(0xef00_00f0, 0x0c14); // dither off | G_ZS_PRIM | Z_CMP | ZMODE_DEC
        command(0xee00_0000, (17 << 16) | 4); // working Z=136: correlated boundary
        command(0xfa00_0000, 0xff00_00ff); // red decal must pass
        command(triangle, edge_ym_yh);
        command(1 << 16, major_slope);
        command(1 << 16, minor_slope);
        command(1 << 16, 0);
        command(0xee00_0000, (18 << 16) | 4); // working Z=144: clearly behind
        command(0xfa00_0000, 0x00ff_00ff); // green decal must reject
        command(triangle, edge_ym_yh);
        command(1 << 16, major_slope);
        command(1 << 16, minor_slope);
        command(1 << 16, 0);
        command(0xe900_0000, 0);
        let end = offset;

        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let pixel = 4 * 8 + 2;
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                COLOR_IMAGE + pixel * 2
            )),
            0xf801,
            "correlated red decal must pass while clearly-behind green rejects"
        );
        let seeded = depth::pack(128, 8);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2)),
            seeded.visible,
            "compare-only decals must retain the opaque seed depth"
        );
    }

    #[test]
    fn raw_shade_triangle_rasterizes_component_gradient() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let mut offset = START;
        let major_slope = (5.0f32 / 6.0 * 65536.0).round() as i32;
        let lower_slope = (5.0f32 / 3.0 * 65536.0).round() as i32;
        let drde = (32.0f32 * 5.0 / 6.0 * 65536.0).round() as u32;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            command(0xff10_0007, TARGET); // RGBA16 width 8
            let yh = 4;
            let ym = 4 * 4;
            let yl = 7 * 4;
            command(0x0c80_0000 | yl, (ym << 16) | yh);
            command(1 << 16, lower_slope as u32);
            command(1 << 16, major_slope as u32);
            command(1 << 16, 0);
            command(0, 255); // black, opaque base shade
            command(32 << 16, 0); // red increases 32 per X pixel
            command(0, 0);
            command(0, 0);
            command((drde >> 16) << 16, 0);
            command(0, 0);
            command((drde & 0xffff) << 16, 0);
            command(0, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let pixel = |x: u32, y: u32| {
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (y * 8 + x) * 2,
            ))
        };
        let raw_edge = gbi::RdpEdgeCoefficients {
            right_major: true,
            level: 0,
            tile: 0,
            yl: 7 * 4,
            ym: 4 * 4,
            yh: 4,
            xl: 1 << 16,
            dxldy: lower_slope,
            xh: 1 << 16,
            dxhdy: major_slope,
            xm: 1 << 16,
            dxmdy: 0,
        };
        for x in [2, 3] {
            let (mask, sample) = raster::test_raw_attribute_sample(
                raw_edge,
                gbi::ScissorRect::framebuffer(8, 8),
                x,
                4,
            );
            let Some((sample_index, _, _)) = sample else {
                panic!("raw shade boundary at x={x} must select a covered attribute sample")
            };
            assert_ne!(mask, 0);
            assert_ne!(mask, u8::MAX);
            assert_ne!(mask & (1 << sample_index), 0);
        }
        assert_eq!(pixel(2, 4), 0x2801);
        assert_eq!(pixel(3, 4), 0x4801);
    }

    #[test]
    fn raw_shade_texture_z_triangle_executes_maximum_width_layout() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        const TEXTURE: u32 = 0x800;
        let mut rdram = vec![0u8; 0x1000];
        let source = [0xf801u16, 0x07c1, 0x003f, 0xffff, 0, 0, 0, 0];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in source.into_iter().enumerate() {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                    pixel,
                );
            }
        }
        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            command(0xff10_0007, TARGET); // RGBA16 width 8
            command(0xfd10_0003, TEXTURE); // RGBA16 width 4
            command(0xf510_0000, 7 << 24); // load tile 7, contiguous TMEM
            command(0xf300_0000, (7 << 24) | (7 << 12) | 0x800); // 8 texels
            command(0xf510_0200, 0x0008_0200); // render tile 0, clamp S/T
            command(0xf200_0000, 0x0000_c004); // 4x2 render extent
            let yh = 4;
            let ym = 4 * 4;
            let yl = 7 * 4;
            let dsde = (5.0f32 / 6.0 * 65536.0).round() as u32;
            command(0x0f80_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(0x00ff_00ff, 0x00ff_00ff); // opaque white base shade
            command(0, 0);
            command(0, 0);
            command(0, 0);
            command(0, 0);
            command(0, 0);
            command(0, 0);
            command(0, 0);
            command(0, 1 << 16); // S=0, T=0, inverse-W=1
            command(1 << 16, 0); // dS/dX=1
            command(0, 0);
            command(0, 0);
            command((dsde >> 16) << 16, 0);
            command(0, 0);
            command((dsde & 0xffff) << 16, 0);
            command(0, 0);
            command(4 << 16, 0); // Z
            command(0, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let pixel = |x: u32, y: u32| {
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (y * 8 + x) * 2,
            ))
        };
        assert_eq!(pixel(2, 4), 0x07c1);
        assert_eq!(pixel(3, 4), 0x003f);
    }

    #[test]
    fn raw_command_stream_triangle_selects_mips_and_trilinear_fraction() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        const TEXTURES: [u32; 3] = [0x800, 0x810, 0x820];
        let mut rdram = vec![0u8; 0x1000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (address, texel) in TEXTURES.into_iter().zip([0xf801, 0x0001, 0xffff]) {
                view.write_u16(fn64_runtime::RdramAddr::from_offset(address), texel);
            }
        }

        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            let combine_w0 = 0xfc00_0000
                | (2 << 20) // cycle 0 A = TEXEL1
                | (13 << 15) // cycle 0 C = LOD_FRACTION
                | (2 << 12) // cycle 0 alpha A = TEXEL1
                | (8 << 5) // cycle 1 A = ZERO
                | 31; // cycle 1 C = ZERO
            let combine_w1 = (1 << 28) // cycle 0 B = TEXEL0
                | (8 << 24) // cycle 1 B = ZERO
                | (7 << 21) // cycle 1 alpha A = ZERO
                | (7 << 18) // cycle 1 alpha C = ZERO
                | (1 << 15) // cycle 0 D = TEXEL0
                | (1 << 12) // cycle 0 alpha B = TEXEL0
                | (1 << 9) // cycle 0 alpha D = TEXEL0
                | (7 << 3); // cycle 1 alpha B = ZERO; D = COMBINED

            command(0xff10_0007, TARGET); // RGBA16 width 8
                                          // Two-cycle, texture LOD enabled, clamp-detail mode, filter-only,
                                          // and deterministic dither disable. Raw edge `level=2` below is
                                          // the RDP primitive's maximum mip level.
            command(0xef00_0000 | (1 << 20) | (1 << 16) | (6 << 9) | 0xf0, 0);
            command(combine_w0, combine_w1);
            for (tile, address) in TEXTURES.into_iter().enumerate() {
                let tile = tile as u32;
                command(0xfd10_0000, address); // RGBA16 width 1
                command(0xf510_0200 | tile, (tile << 24) | 0x0008_0200);
                command(0xf200_0000, tile << 24); // 1x1 render tile
                command(0xf300_0000, tile << 24); // load into that tile
            }

            let yh = 4;
            let ym = 4 * 4;
            let yl = 7 * 4;
            command(0x0a80_0000 | (2 << 19) | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            // S=T=0, W=1; dS/dX=dT/dY=2.5. Chapter 13.7 selects
            // tiles 1 and 2 with LOD fraction 0.25.
            command(0, 1 << 16);
            command(2 << 16, 0);
            command(0, 0);
            command(0x8000_0000, 0);
            command(0, 0);
            command(2, 0);
            command(0, 0);
            command(0x0000_8000, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (4 * 8 + 2) * 2
            )),
            0x4211,
            "LOD 2.5 must blend one quarter from black tile 1 toward white tile 2"
        );
    }

    #[test]
    fn raw_yuv_texture_rectangle_applies_set_convert_into_rdram() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        const TEXTURE: u32 = 0x600;
        let mut rdram = vec![0u8; 0x800];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            // Public RDP YUV16 byte order: Y0, U, Y1, V. Neutral chroma
            // makes the default public conversion table preserve each Y as
            // equal R/G/B, which gives this gate unambiguous expected pixels.
            for (index, value) in [16, 128, 235, 128].into_iter().enumerate() {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32),
                    value,
                );
            }
        }

        let field = |value: i16| u32::from(value as u16) & 0x1ff;
        let [k0, k1, k2, k3, k4, k5] = [175, -43, -89, 222, 114, 42].map(field);
        let set_convert = (
            0xec00_0000 | (k0 << 13) | (k1 << 4) | ((k2 >> 5) & 0x0f),
            ((k2 & 0x1f) << 27) | (k3 << 18) | (k4 << 9) | k5,
        );
        let combine_command = |rgb: [u32; 4], alpha: [u32; 4]| {
            let w0 = 0xfc00_0000
                | ((rgb[0] & 0x0f) << 20)
                | ((rgb[2] & 0x1f) << 15)
                | ((alpha[0] & 0x07) << 12)
                | ((alpha[2] & 0x07) << 9)
                | ((rgb[0] & 0x0f) << 5)
                | (rgb[2] & 0x1f);
            let w1 = ((rgb[1] & 0x0f) << 28)
                | ((rgb[1] & 0x0f) << 24)
                | ((alpha[0] & 0x07) << 21)
                | ((alpha[2] & 0x07) << 18)
                | ((rgb[3] & 0x07) << 15)
                | ((alpha[1] & 0x07) << 12)
                | ((alpha[3] & 0x07) << 9)
                | ((rgb[3] & 0x07) << 6)
                | ((alpha[1] & 0x07) << 3)
                | (alpha[3] & 0x07);
            (w0, w1)
        };

        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            // One-cycle, point sampled, G_TC_CONV, with color/alpha dither
            // disabled so this gate isolates the conversion table.
            command(0xef00_00f0, 0);
            command(set_convert.0, set_convert.1);
            let (combine_w0, combine_w1) = combine_command([8, 8, 31, 1], [7, 7, 7, 1]);
            command(combine_w0, combine_w1); // (0-0)*0+TEXEL0
            command(0xff10_0001, TARGET); // RGBA16 width 2
            command(0xfd30_0001, TEXTURE); // YUV16 width 2
            command(0xf530_0000, 7 << 24); // YUV16 load tile 7
            command(0xf300_0000, (7 << 24) | (1 << 12) | 0x800); // YUYV pair
            command(0xf530_0200, 0x0008_0200); // YUV16 render tile 0
            command(0xf200_0000, 0x0000_4000); // 2x1 render extent
            command(0xe400_0000 | ((2 * 4) << 12) | 4, 0);
            command(0, 0x0400_0400); // S/T=0, dS/dX=dT/dY=1
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(2, 1)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0x1085
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2)),
            0xef7b
        );
    }

    #[test]
    fn raw_chroma_key_commands_drive_alpha_fixup_and_compare() {
        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        const TEXTURE: u32 = 0x600;
        let mut rdram = vec![0u8; 0x800];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in [0x07c1u16, 0xf801].into_iter().enumerate() {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                    pixel,
                );
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TARGET + index as u32 * 2),
                    0xffff,
                );
            }
        }
        let combine_command = |rgb: [u32; 4], alpha: [u32; 4]| {
            let w0 = 0xfc00_0000
                | ((rgb[0] & 0x0f) << 20)
                | ((rgb[2] & 0x1f) << 15)
                | ((alpha[0] & 0x07) << 12)
                | ((alpha[2] & 0x07) << 9)
                | ((rgb[0] & 0x0f) << 5)
                | (rgb[2] & 0x1f);
            let w1 = ((rgb[1] & 0x0f) << 28)
                | ((rgb[1] & 0x0f) << 24)
                | ((alpha[0] & 0x07) << 21)
                | ((alpha[2] & 0x07) << 18)
                | ((rgb[3] & 0x07) << 15)
                | ((alpha[1] & 0x07) << 12)
                | ((alpha[3] & 0x07) << 9)
                | ((rgb[3] & 0x07) << 6)
                | ((alpha[1] & 0x07) << 3)
                | (alpha[3] & 0x07);
            (w0, w1)
        };

        let mut offset = START;
        {
            let mut command = |w0: u32, w1: u32| {
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
                offset += 8;
            };
            // One-cycle, filter-only, chroma key enabled, alpha threshold on.
            command(0xef00_0df0, 1);
            command(0xf900_0000, 0x0000_0080); // threshold alpha = 128
            command(0xea10_0100, 0xffff_00ff); // center green, unit widths/scales
            command(0xeb00_0000, 0x0100_00ff);
            let (combine_w0, combine_w1) = combine_command([1, 6, 6, 7], [7, 7, 7, 7]);
            command(combine_w0, combine_w1); // (TEXEL0-CENTER)*SCALE
            command(0xff10_0001, TARGET); // RGBA16 width 2
            command(0xfd10_0001, TEXTURE); // RGBA16 width 2
            command(0xf510_0000, 7 << 24); // load tile 7, contiguous TMEM
            command(0xf300_0000, (7 << 24) | (1 << 12) | 0x800); // 2 texels
            command(0xf510_0200, 0x0008_0200); // render tile 0, clamp S/T
            command(0xf200_0000, 0x0000_4000); // 2x1 render extent
            command(0xe400_0000 | ((2 * 4) << 12) | 4, 0);
            command(0, 0x0400_0400);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(2, 1)).unwrap();

        backend
            .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0x0001
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2)),
            0xffff
        );
    }

    #[test]
    fn reference_backend_auto_dump_can_skip_to_a_late_task_window() {
        let backend = ReferenceBackend::new()
            .with_auto_dump("/tmp", "fn64-render-test", 3)
            .with_auto_dump_skip(4_180);
        let dump = backend.auto_dump.unwrap();
        assert_eq!(dump.task_index, 0);
        assert_eq!(dump.skip_before_task, 4_180);
        assert_eq!(dump.written, 0);
        assert_eq!(dump.limit, 3);
    }

    #[test]
    fn framebuffer_writer_and_runtime_view_agree_on_logical_pixel_order() {
        let mut framebuffer = Framebuffer::new(2, 1);
        framebuffer.pixels[0..4].copy_from_slice(&[255, 0, 0, 255]);
        framebuffer.pixels[4..8].copy_from_slice(&[0, 0, 255, 255]);
        let mut storage = [0u8; 4];
        let mut hidden_bits = HashMap::new();

        write_rgba5551_framebuffer(&mut storage, 0, &framebuffer, &mut hidden_bits);

        let view = fn64_runtime::RdramView::from_storage(&storage);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(0)),
            0xF801,
            "pixel 0 must be logical RGBA5551 red"
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(2)),
            0x003F,
            "pixel 1 must be logical RGBA5551 blue"
        );
        assert_eq!(
            storage,
            [0x3F, 0x00, 0x01, 0xF8],
            "native-word storage must contain the two logical halfwords in lane-mapped order"
        );
    }

    #[test]
    fn disabled_dither_rgba16_truncates_low_three_bits() {
        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.pixels.copy_from_slice(&[7, 8, 15, 255]);
        let mut storage = [0u8; 4];
        let mut hidden_bits = HashMap::new();

        write_rgba5551_framebuffer(&mut storage, 0, &framebuffer, &mut hidden_bits);

        let view = fn64_runtime::RdramView::from_storage(&storage);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(0)),
            0x0043,
            "7 must remain zero while 8 and 15 truncate to one; round-to-nearest would change both boundary channels"
        );
    }

    #[test]
    fn rgba16_coverage_round_trips_visible_and_hidden_storage_bits() {
        let mut framebuffer = Framebuffer::new(8, 1);
        framebuffer.pixels.fill(255);
        for (index, coverage) in framebuffer.coverage.iter_mut().enumerate() {
            *coverage = raster::Coverage::new(index as u8 + 1);
        }
        let mut storage = [0u8; 16];
        let mut hidden_bits = HashMap::new();

        write_rgba5551_framebuffer(&mut storage, 0, &framebuffer, &mut hidden_bits);
        let view = fn64_runtime::RdramView::from_storage(&storage);
        for index in 0..8u32 {
            let address = index * 2;
            let visible = view.read_u16(fn64_runtime::RdramAddr::from_offset(address));
            let stored = index as u8;
            assert_eq!((visible & 1) as u8, stored >> 2);
            assert_eq!(
                hidden_bits.get(&address).map(|sample| sample.bits),
                Some(stored & 3)
            );
        }

        let mut loaded = Framebuffer::new(8, 1);
        load_rgba5551_framebuffer(
            &storage,
            gbi::ColorImage {
                format: gbi::ColorImage::RGBA_FORMAT,
                size: gbi::ColorImage::BITS_16,
                width: 8,
                address: 0,
            },
            &mut loaded,
            &mut hidden_bits,
        );
        assert_eq!(loaded.coverage, framebuffer.coverage);
    }

    #[test]
    fn rgba32_round_trips_five_bit_alpha_and_three_bit_coverage() {
        let mut framebuffer = Framebuffer::new(2, 1);
        framebuffer
            .pixels
            .copy_from_slice(&[0x12, 0x34, 0x56, 0x29, 0xab, 0xcd, 0xef, 0xbd]);
        framebuffer.coverage[0] = raster::Coverage::new(3);
        framebuffer.coverage[1] = raster::Coverage::FULL;
        let mut storage = [0u8; 8];

        write_rgba8888_framebuffer(&mut storage, 0, &framebuffer);
        let view = fn64_runtime::RdramView::from_storage(&storage);
        assert_eq!(
            view.read_u32(fn64_runtime::RdramAddr::from_offset(0)),
            0x1234_5645
        );
        assert_eq!(
            view.read_u32(fn64_runtime::RdramAddr::from_offset(4)),
            0xabcd_eff7
        );

        let mut loaded = Framebuffer::new(2, 1);
        load_rgba8888_framebuffer(
            &storage,
            gbi::ColorImage {
                format: gbi::ColorImage::RGBA_FORMAT,
                size: gbi::ColorImage::BITS_32,
                width: 2,
                address: 0,
            },
            &mut loaded,
        );
        assert_eq!(loaded.pixels, framebuffer.pixels);
        assert_eq!(loaded.coverage, framebuffer.coverage);
    }

    #[test]
    fn rgba32_memory_alpha_truncates_low_three_bits() {
        let mut framebuffer = Framebuffer::new(2, 1);
        framebuffer
            .pixels
            .copy_from_slice(&[1, 2, 3, 7, 4, 5, 6, 8]);
        let mut storage = [0u8; 8];

        write_rgba8888_framebuffer(&mut storage, 0, &framebuffer);

        let view = fn64_runtime::RdramView::from_storage(&storage);
        assert_eq!(
            view.read_u32(fn64_runtime::RdramAddr::from_offset(0)),
            0x0102_03e0
        );
        assert_eq!(
            view.read_u32(fn64_runtime::RdramAddr::from_offset(4)),
            0x0405_06e1
        );
    }

    #[test]
    fn changed_cpu_visible_word_reconstructs_its_hidden_bits_from_the_lsb() {
        let mut hidden_bits = HashMap::from([(
            0,
            RdramHiddenSample {
                visible: 1,
                bits: 1,
            },
        )]);
        assert_eq!(read_rdram_hidden_bits(&mut hidden_bits, 0, 0), 0);
        assert_eq!(
            hidden_bits.get(&0),
            Some(&RdramHiddenSample {
                visible: 0,
                bits: 0,
            })
        );
        assert_eq!(read_rdram_hidden_bits(&mut hidden_bits, 0, 1), 3);
    }

    #[test]
    fn known_same_value_non_rdp_write_replicates_the_visible_lsb() {
        let mut backend = ReferenceBackend::new();
        let mut visible = vec![0u8; 8];
        fn64_runtime::RdramViewMut::from_storage(&mut visible)
            .write_u16(fn64_runtime::RdramAddr::from_offset(2), 0x1235);
        backend.rdram_hidden_bits = HashMap::from([
            (
                0,
                RdramHiddenSample {
                    visible: 0x1234,
                    bits: 2,
                },
            ),
            (
                2,
                RdramHiddenSample {
                    visible: 0x1235,
                    bits: 1,
                },
            ),
        ]);

        assert_eq!(
            backend.observe_non_rdp_write16(NonRdpWrite16::new(0, 0x1234)),
            NonRdpWrite16Disposition::AppliedHiddenSidecar
        );
        assert_eq!(
            backend.observe_non_rdp_write16(NonRdpWrite16::new(2, 0x1235)),
            NonRdpWrite16Disposition::AppliedHiddenSidecar
        );
        assert_eq!(backend.rdram_hidden_bits[&0].bits, 0);
        assert_eq!(backend.rdram_hidden_bits[&2].bits, 3);
        assert_eq!(
            fn64_runtime::RdramView::from_storage(&visible)
                .read_u16(fn64_runtime::RdramAddr::from_offset(2)),
            0x1235,
            "renderer-owned hidden-bit repair must not mutate coherent CPU-visible bytes"
        );
        assert_eq!(
            backend.observe_non_rdp_write16(NonRdpWrite16::new(4, 0xffff)),
            NonRdpWrite16Disposition::NoRustHiddenSidecar
        );
    }

    #[test]
    fn index8_commit_preserves_hidden_bits_across_partial_halfword_overlap() {
        let index8 = gbi::ColorImage {
            format: gbi::ColorImage::CI_FORMAT,
            size: gbi::ColorImage::BITS_8,
            width: 3,
            address: 0,
        };
        let rgba16 = gbi::ColorImage {
            format: gbi::ColorImage::RGBA_FORMAT,
            size: gbi::ColorImage::BITS_16,
            width: 2,
            address: 0,
        };
        let mut rdram = vec![0u8; 8];
        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u8(fn64_runtime::RdramAddr::from_offset(3), 0x79);
        let untouched = RdramHiddenSample {
            visible: 0xcafe,
            bits: 3,
        };
        let mut hidden_bits = HashMap::from([
            (
                0,
                RdramHiddenSample {
                    visible: 0xaaaa,
                    bits: 2,
                },
            ),
            (
                2,
                RdramHiddenSample {
                    visible: 0xbbbb,
                    bits: 1,
                },
            ),
            (4, untouched),
        ]);
        let mut source = Framebuffer::new(3, 1);
        for (pixel, intensity) in source.pixels.chunks_exact_mut(4).zip([0x12, 0x34, 0x56]) {
            pixel.copy_from_slice(&[intensity, intensity, intensity, 255]);
        }

        commit_color_image(&mut rdram, index8, &source, &mut hidden_bits);

        assert_eq!(
            hidden_bits[&0],
            RdramHiddenSample {
                visible: 0x1234,
                bits: 2
            }
        );
        assert_eq!(
            hidden_bits[&2],
            RdramHiddenSample {
                visible: 0x5679,
                bits: 1
            }
        );
        assert_eq!(hidden_bits[&4], untouched);
        let mut imported = Framebuffer::new(2, 1);
        load_color_image(&rdram, rgba16, &mut imported, &mut hidden_bits);
        assert_eq!(imported.coverage[0].stored(), 2);
        assert_eq!(imported.coverage[1].stored(), 5);
        assert_eq!(hidden_bits[&4], untouched);
    }

    #[test]
    fn rgba32_commit_preserves_each_overlapping_halfword_hidden_pair() {
        let rgba32 = gbi::ColorImage {
            format: gbi::ColorImage::RGBA_FORMAT,
            size: gbi::ColorImage::BITS_32,
            width: 2,
            address: 0,
        };
        let rgba16 = gbi::ColorImage {
            format: gbi::ColorImage::RGBA_FORMAT,
            size: gbi::ColorImage::BITS_16,
            width: 4,
            address: 0,
        };
        let untouched = RdramHiddenSample {
            visible: 0xdead,
            bits: 2,
        };
        let mut hidden_bits = HashMap::from([
            (
                0,
                RdramHiddenSample {
                    visible: 0,
                    bits: 2,
                },
            ),
            (
                2,
                RdramHiddenSample {
                    visible: 0,
                    bits: 1,
                },
            ),
            (
                4,
                RdramHiddenSample {
                    visible: 0,
                    bits: 3,
                },
            ),
            (
                6,
                RdramHiddenSample {
                    visible: 0,
                    bits: 0,
                },
            ),
            (8, untouched),
        ]);
        let mut source = Framebuffer::new(2, 1);
        source
            .pixels
            .copy_from_slice(&[0x10, 0x20, 0x30, 0x08, 0x40, 0x51, 0x60, 0x00]);
        source.coverage.fill(raster::Coverage::new(1));
        let mut rdram = vec![0u8; 12];

        commit_color_image(&mut rdram, rgba32, &source, &mut hidden_bits);

        assert_eq!(
            hidden_bits[&0],
            RdramHiddenSample {
                visible: 0x1020,
                bits: 2
            }
        );
        assert_eq!(
            hidden_bits[&2],
            RdramHiddenSample {
                visible: 0x3001,
                bits: 1
            }
        );
        assert_eq!(
            hidden_bits[&4],
            RdramHiddenSample {
                visible: 0x4051,
                bits: 3
            }
        );
        assert_eq!(
            hidden_bits[&6],
            RdramHiddenSample {
                visible: 0x6000,
                bits: 0
            }
        );
        assert_eq!(hidden_bits[&8], untouched);
        let mut imported = Framebuffer::new(4, 1);
        load_color_image(&rdram, rgba16, &mut imported, &mut hidden_bits);
        assert_eq!(
            imported
                .coverage
                .iter()
                .map(|coverage| coverage.stored())
                .collect::<Vec<_>>(),
            [2, 5, 7, 0]
        );
        assert_eq!(hidden_bits[&8], untouched);
    }

    #[test]
    fn every_public_color_image_transition_commits_then_imports_exact_layouts() {
        const SOURCE: u32 = 0x100;
        const DESTINATION: u32 = 0x200;
        let image = |layout, address| gbi::ColorImage {
            format: match layout {
                gbi::ColorImageLayout::Index8 => gbi::ColorImage::CI_FORMAT,
                gbi::ColorImageLayout::Rgba16 | gbi::ColorImageLayout::Rgba32 => {
                    gbi::ColorImage::RGBA_FORMAT
                }
            },
            size: match layout {
                gbi::ColorImageLayout::Index8 => gbi::ColorImage::BITS_8,
                gbi::ColorImageLayout::Rgba16 => gbi::ColorImage::BITS_16,
                gbi::ColorImageLayout::Rgba32 => gbi::ColorImage::BITS_32,
            },
            width: 4,
            address,
        };
        let expected_bytes = |layout| -> &'static [u8] {
            match layout {
                gbi::ColorImageLayout::Index8 => &[0x18, 0x80, 0xf8, 0x08],
                gbi::ColorImageLayout::Rgba16 => &[0x19, 0x4e, 0x85, 0x30, 0xf8, 0x1f, 0x0f, 0xc1],
                gbi::ColorImageLayout::Rgba32 => &[
                    0x18, 0x28, 0x38, 0x09, 0x80, 0xa0, 0xc0, 0x5c, 0xf8, 0x00, 0x78, 0xa4, 0x08,
                    0xf8, 0x00, 0xff,
                ],
            }
        };
        let mut original = Framebuffer::new(4, 1);
        original.pixels.copy_from_slice(&[
            0x18, 0x28, 0x38, 0x48, 0x80, 0xa0, 0xc0, 0xe0, 0xf8, 0x00, 0x78, 0x20, 0x08, 0xf8,
            0x00, 0xff,
        ]);
        for (coverage, count) in original.coverage.iter_mut().zip([1, 3, 6, 8]) {
            *coverage = raster::Coverage::new(count);
        }

        for from in gbi::ColorImageLayout::ALL {
            for to in gbi::ColorImageLayout::ALL {
                let source = image(from, SOURCE);
                let destination = image(to, DESTINATION);
                assert_eq!(source.transition_to(destination).from, from);

                let mut rdram = vec![0xcc; 0x400];
                let mut hidden_bits = HashMap::new();
                commit_color_image(&mut rdram, destination, &original, &mut hidden_bits);
                commit_color_image(&mut rdram, source, &original, &mut hidden_bits);

                let view = fn64_runtime::RdramView::from_storage(&rdram);
                let actual = (0..expected_bytes(from).len())
                    .map(|offset| {
                        view.read_u8(fn64_runtime::RdramAddr::from_offset(SOURCE + offset as u32))
                    })
                    .collect::<Vec<_>>();
                assert_eq!(actual, expected_bytes(from), "{from:?} -> {to:?}");

                let mut loaded = Framebuffer::new(4, 1);
                load_color_image(&rdram, destination, &mut loaded, &mut hidden_bits);
                match to {
                    gbi::ColorImageLayout::Index8 => {
                        assert_eq!(
                            loaded.pixels,
                            [
                                0x18, 0x18, 0x18, 255, 0x80, 0x80, 0x80, 255, 0xf8, 0xf8, 0xf8,
                                255, 0x08, 0x08, 0x08, 255,
                            ],
                            "{from:?} -> {to:?}"
                        );
                        assert!(loaded
                            .coverage
                            .iter()
                            .all(|value| *value == raster::Coverage::FULL));
                    }
                    gbi::ColorImageLayout::Rgba16 => {
                        assert_eq!(
                            loaded.pixels,
                            [
                                0x18, 0x29, 0x39, 255, 0x84, 0xa5, 0xc6, 255, 0xff, 0x00, 0x7b,
                                255, 0x08, 0xff, 0x00, 255,
                            ],
                            "{from:?} -> {to:?}"
                        );
                        assert_eq!(loaded.coverage, original.coverage);
                    }
                    gbi::ColorImageLayout::Rgba32 => {
                        assert_eq!(
                            loaded.pixels,
                            [
                                0x18, 0x28, 0x38, 0x4a, 0x80, 0xa0, 0xc0, 0xe7, 0xf8, 0x00, 0x78,
                                0x21, 0x08, 0xf8, 0x00, 0xff,
                            ],
                            "{from:?} -> {to:?}"
                        );
                        assert_eq!(loaded.coverage, original.coverage);
                    }
                }
            }
        }
    }

    #[test]
    fn every_public_fill_layout_commits_exact_bytes_and_hidden_ownership() {
        let target = |layout| gbi::ColorImage {
            format: match layout {
                gbi::ColorImageLayout::Index8 => gbi::ColorImage::CI_FORMAT,
                gbi::ColorImageLayout::Rgba16 | gbi::ColorImageLayout::Rgba32 => {
                    gbi::ColorImage::RGBA_FORMAT
                }
            },
            size: match layout {
                gbi::ColorImageLayout::Index8 => gbi::ColorImage::BITS_8,
                gbi::ColorImageLayout::Rgba16 => gbi::ColorImage::BITS_16,
                gbi::ColorImageLayout::Rgba32 => gbi::ColorImage::BITS_32,
            },
            width: 4,
            address: 0,
        };
        let rectangle = gbi::FillRectangle {
            ulx: 0.0,
            uly: 0.0,
            lrx: 3.0,
            lry: 0.0,
            fill_color: 0x1234_5678,
            cycle_type: gbi::CycleType::Fill,
            scissor: None,
            other_mode: gbi::OtherMode::default(),
            combiner: gbi::CombinerState::default(),
            blender: gbi::BlenderState::default(),
        };
        for layout in gbi::ColorImageLayout::ALL {
            let mut framebuffer = Framebuffer::new(4, 1);
            framebuffer.draw_fill_rectangle(&rectangle, target(layout));
            let mut rdram = vec![0xcc; 16];
            let sentinel = RdramHiddenSample {
                visible: 0xaaaa,
                bits: 2,
            };
            let mut hidden_bits =
                HashMap::from([(0, sentinel), (2, sentinel), (4, sentinel), (6, sentinel)]);
            commit_color_image(&mut rdram, target(layout), &framebuffer, &mut hidden_bits);

            let expected: &[u8] = match layout {
                gbi::ColorImageLayout::Index8 => &[0x12, 0x34, 0x56, 0x78],
                gbi::ColorImageLayout::Rgba16 => &[0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78],
                gbi::ColorImageLayout::Rgba32 => &[
                    0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78, 0x12,
                    0x34, 0x56, 0x78,
                ],
            };
            let view = fn64_runtime::RdramView::from_storage(&rdram);
            let actual = (0..expected.len())
                .map(|offset| view.read_u8(fn64_runtime::RdramAddr::from_offset(offset as u32)))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{layout:?}");
            for address in [0u32, 2, 4, 6] {
                let fill_halfword = if address.is_multiple_of(4) {
                    0x1234
                } else {
                    0x5678
                };
                let expected_hidden = match layout {
                    gbi::ColorImageLayout::Rgba16 => RdramHiddenSample {
                        visible: fill_halfword,
                        bits: 0,
                    },
                    gbi::ColorImageLayout::Index8 if address < 4 => RdramHiddenSample {
                        visible: fill_halfword,
                        bits: sentinel.bits,
                    },
                    gbi::ColorImageLayout::Rgba32 => RdramHiddenSample {
                        visible: fill_halfword,
                        bits: sentinel.bits,
                    },
                    gbi::ColorImageLayout::Index8 => sentinel,
                };
                assert_eq!(
                    hidden_bits[&address], expected_hidden,
                    "{layout:?} at {address}"
                );
            }
        }
    }

    #[test]
    fn ordered_fill_rectangles_write_the_explicit_color_image() {
        const DL: usize = 0x100;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let mut offset = DL;
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        // G_RDPSETOTHERMODE: G_CYC_FILL.
        write_command(&mut rdram, offset, 0xef00_0000 | (3 << 20), 0);
        offset += 8;
        // G_SETCIMG RGBA16 width 4.
        write_command(&mut rdram, offset, 0xff10_0003, TARGET);
        offset += 8;
        // Red fill across the full 4x2 target.
        write_command(&mut rdram, offset, 0xf700_0000, 0xf801_f801);
        offset += 8;
        write_command(&mut rdram, offset, 0xf600_0000 | ((3 * 4) << 12) | 4, 0);
        offset += 8;
        // Blue overwrites row 0 pixels 1..2. Keeping two fill operations in
        // one stream proves the decoder/backend no longer groups by primitive.
        write_command(&mut rdram, offset, 0xf700_0000, 0x003f_003f);
        offset += 8;
        write_command(&mut rdram, offset, 0xf600_0000 | ((2 * 4) << 12), 4 << 12);
        offset += 8;
        write_command(&mut rdram, offset, 0xe900_0000, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xdf00_0000, 0);

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let expected = [
            0xf801, 0x003f, 0x003f, 0xf801, 0xf801, 0xf801, 0xf801, 0xf801,
        ];
        for (index, expected) in expected.into_iter().enumerate() {
            let address = fn64_runtime::RdramAddr::from_offset(TARGET + index as u32 * 2);
            assert_eq!(view.read_u16(address), expected, "pixel {index}");
        }
        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2), 0xffff);
        // RDP target state survives task boundaries. A second task omits
        // G_SETCIMG and must continue drawing the prior color image rather
        // than falling back to output_addr/VI state. The task-boundary import
        // must also retain the CPU's intervening white write to pixel 1.
        let mut second = DL;
        write_command(&mut rdram, second, 0xef00_0000 | (3 << 20), 0);
        second += 8;
        write_command(&mut rdram, second, 0xf700_0000, 0x07c1_07c1);
        second += 8;
        write_command(&mut rdram, second, 0xf600_0000, 0);
        second += 8;
        write_command(&mut rdram, second, 0xe900_0000, 0);
        second += 8;
        write_command(&mut rdram, second, 0xdf00_0000, 0);
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0x07c1
        );
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2)),
            0xffff,
            "second task must re-import CPU-visible writes to untouched persistent-target pixels"
        );
    }

    #[test]
    fn reference_backend_preserves_rdp_mode_and_fill_registers_between_tasks() {
        const DL: usize = 0x100;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x800];
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(1, 1)).unwrap();

        // Task one only programs device registers; it emits no pixels.
        write_command(&mut rdram, DL, 0xef00_0000 | (3 << 20), 0);
        write_command(&mut rdram, DL + 8, 0xff10_0000, TARGET);
        write_command(&mut rdram, DL + 16, 0xf700_0000, 0xf801_f801);
        write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        // Task two deliberately omits SETOTHERMODE, SETCIMG, and SETFILLCOLOR.
        // All three registers belong to the RDP and remain selected.
        write_command(&mut rdram, DL, 0xf600_0000, 0);
        write_command(&mut rdram, DL + 8, 0xe900_0000, 0);
        write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        assert_eq!(
            fn64_runtime::RdramView::from_storage(&rdram)
                .read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0xf801
        );
    }

    #[test]
    fn raw_dpc_and_f3dex2_hle_share_one_persistent_rdp_register_file() {
        const RAW: usize = 0x100;
        const DL: usize = 0x200;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x800];
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(1, 1)).unwrap();

        // A bounded raw DPC submission programs the device without drawing.
        write_command(&mut rdram, RAW, 0xef00_0000 | (3 << 20), 0);
        write_command(&mut rdram, RAW + 8, 0xff10_0000, TARGET);
        write_command(&mut rdram, RAW + 16, 0xf700_0000, 0x07c1_07c1);
        backend
            .process_rdp_commands(&mut rdram, RAW as u32, (RAW + 24) as u32, 0)
            .unwrap();

        // The next admitted HLE task consumes those same registers.
        write_command(&mut rdram, DL, 0xf600_0000, 0);
        write_command(&mut rdram, DL + 8, 0xe900_0000, 0);
        write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        assert_eq!(
            fn64_runtime::RdramView::from_storage(&rdram)
                .read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0x07c1
        );
    }

    #[test]
    fn rgba32_fill_cycle_writes_rgb_alpha_and_coverage_packing() {
        let mut rdram = vec![0u8; 0x1000];
        let commands: [(u32, u32); 6] = [
            (0xef00_0000 | (3 << 20), 0),
            (0xff18_0003, 0x400),
            (0xf700_0000, 0x1234_56e5),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
            (0xe900_0000, 0),
            (0xdf00_0000, 0),
        ];
        for (index, (word0, word1)) in commands.into_iter().enumerate() {
            let offset = 0x100 + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
        }
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: 0x100,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for index in 0..8 {
            assert_eq!(
                view.read_u32(fn64_runtime::RdramAddr::from_offset(0x400 + index * 4)),
                0x1234_56e5,
                "RGBA32 fill pixel {index}"
            );
        }
        let framebuffer = backend.framebuffer().unwrap();
        assert_eq!(&framebuffer.pixels[..4], &[0x12, 0x34, 0x56, 0x29]);
        assert_eq!(framebuffer.coverage[0], raster::Coverage::FULL);
    }

    #[test]
    fn ordered_target_switch_commits_each_rgba_format_with_its_own_packing() {
        let mut rdram = vec![0u8; 0x1000];
        let commands: [(u32, u32); 9] = [
            (0xef00_0000 | (3 << 20), 0),
            (0xff10_0001, 0x400),
            (0xf700_0000, 0xf801_f801),
            (0xf600_0000 | (4 << 12), 0),
            (0xff18_0001, 0x500),
            (0xf700_0000, 0x1234_56e5),
            (0xf600_0000 | (4 << 12), 0),
            (0xe900_0000, 0),
            (0xdf00_0000, 0),
        ];
        for (index, (word0, word1)) in commands.into_iter().enumerate() {
            let offset = 0x100 + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
        }
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(2, 1)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: 0x100,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for address in [0x400, 0x402] {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(address)),
                0xf801
            );
        }
        for address in [0x500, 0x504] {
            assert_eq!(
                view.read_u32(fn64_runtime::RdramAddr::from_offset(address)),
                0x1234_56e5
            );
        }
    }

    #[test]
    fn intensity8_fill_uses_all_four_fill_register_bytes_and_ignores_coverage() {
        let mut rdram = vec![0u8; 0x1000];
        let commands: [(u32, u32); 6] = [
            (0xef00_0000 | (3 << 20), 0),
            // Set Color Image: arbitrary format field, public 8-bit size,
            // width four. Figure 15.5.4 defines size=8 as intensity bytes.
            (0xff00_0000 | (4 << 21) | (1 << 19) | 3, 0x400),
            (0xf700_0000, 0x1234_5678),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
            (0xe900_0000, 0),
            (0xdf00_0000, 0),
        ];
        for (index, (word0, word1)) in commands.into_iter().enumerate() {
            let offset = 0x100 + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
        }
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: 0x100,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for row in 0..2 {
            for (column, intensity) in [0x12, 0x34, 0x56, 0x78].into_iter().enumerate() {
                assert_eq!(
                    view.read_u8(fn64_runtime::RdramAddr::from_offset(
                        0x400 + row * 4 + column as u32
                    )),
                    intensity
                );
            }
        }
        let framebuffer = backend.framebuffer().unwrap();
        assert_eq!(
            &framebuffer.pixels[..16],
            &[
                0x12, 0x12, 0x12, 255, 0x34, 0x34, 0x34, 255, 0x56, 0x56, 0x56, 255, 0x78, 0x78,
                0x78, 255
            ]
        );
        assert!(framebuffer
            .coverage
            .iter()
            .all(|coverage| *coverage == raster::Coverage::FULL));
    }

    #[test]
    fn intensity8_target_import_and_commit_share_logical_rdram_bytes() {
        let mut rdram = vec![0u8; 0x500];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, intensity) in [17, 34, 51, 68].into_iter().enumerate() {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(0x400 + index as u32),
                    intensity,
                );
            }
        }
        let target = gbi::ColorImage {
            format: 2,
            size: gbi::ColorImage::BITS_8,
            width: 4,
            address: 0x400,
        };
        let mut framebuffer = Framebuffer::new(4, 1);
        let mut hidden_bits = HashMap::new();
        load_color_image(&rdram, target, &mut framebuffer, &mut hidden_bits);
        assert_eq!(
            framebuffer.pixels,
            [17, 17, 17, 255, 34, 34, 34, 255, 51, 51, 51, 255, 68, 68, 68, 255]
        );

        framebuffer.pixels[0] = 0xa5;
        framebuffer.pixels[4] = 0xb6;
        framebuffer.pixels[8] = 0xc7;
        framebuffer.pixels[12] = 0xd8;
        framebuffer.coverage.fill(raster::Coverage::new(1));
        commit_color_image(&mut rdram, target, &framebuffer, &mut hidden_bits);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            (0..4)
                .map(|index| view.read_u8(fn64_runtime::RdramAddr::from_offset(0x400 + index)))
                .collect::<Vec<_>>(),
            [0xa5, 0xb6, 0xc7, 0xd8]
        );
        assert!(
            hidden_bits.is_empty(),
            "I8 ignores RDRAM hidden coverage bits"
        );
    }

    #[test]
    fn same_color_image_bytes_reinterpret_between_index8_and_rgba16() {
        const ADDRESS: u32 = 0x400;
        let rgba16 = gbi::ColorImage {
            format: gbi::ColorImage::RGBA_FORMAT,
            size: gbi::ColorImage::BITS_16,
            width: 2,
            address: ADDRESS,
        };
        let index8 = gbi::ColorImage {
            format: gbi::ColorImage::CI_FORMAT,
            size: gbi::ColorImage::BITS_8,
            width: 4,
            address: ADDRESS,
        };
        let mut rdram = vec![0u8; 0x500];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(ADDRESS), 0xf801);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(ADDRESS + 2), 0x07c1);
        }

        let mut framebuffer = Framebuffer::new(2, 1);
        let mut hidden_bits = HashMap::new();
        load_color_image(&rdram, rgba16, &mut framebuffer, &mut hidden_bits);
        assert_eq!(&framebuffer.pixels[..8], &[255, 0, 0, 255, 0, 255, 0, 255]);

        load_color_image(&rdram, index8, &mut framebuffer, &mut hidden_bits);
        assert_eq!(
            framebuffer
                .pixels
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            [0xf8, 0x01, 0x07, 0xc1]
        );

        for (pixel, byte) in framebuffer
            .pixels
            .chunks_exact_mut(4)
            .zip([0x00, 0x3f, 0xff, 0xff])
        {
            pixel[..3].fill(byte);
        }
        commit_color_image(&mut rdram, index8, &framebuffer, &mut hidden_bits);
        load_color_image(&rdram, rgba16, &mut framebuffer, &mut hidden_bits);
        assert_eq!(
            &framebuffer.pixels[..8],
            &[0, 0, 255, 255, 255, 255, 255, 255]
        );
    }

    #[test]
    fn reference_renderer_rejects_invalid_non_rgba_16bit_targets_by_name() {
        let mut rdram = vec![0u8; 0x1000];
        rdram[0x100..0x104].copy_from_slice(&0xff70_0003u32.to_ne_bytes());
        rdram[0x104..0x108].copy_from_slice(&0x400u32.to_ne_bytes());
        rdram[0x108..0x10c].copy_from_slice(&0xdf00_0000u32.to_ne_bytes());
        rdram[0x10c..0x110].copy_from_slice(&0u32.to_ne_bytes());
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        let error = backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: 0x100,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap_err();
        assert!(error.to_string().contains("format=3 size=2"));
        assert!(error.to_string().contains("requires 8-bit intensity"));
    }

    #[test]
    fn f3dex2_color_writes_require_persistent_setcimg_not_output_addr() {
        const DL: usize = 0x100;
        const VERTICES: usize = 0x200;
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        let mut rdram = vec![0u8; 0x2000];
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        write_command(
            &mut rdram,
            DL,
            (u32::from(gbi::G_VTX) << 24) | (3 << 12) | (3 << 1),
            VERTICES as u32,
        );
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(gbi::G_TRI1) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        write_command(&mut rdram, DL + 16, u32::from(gbi::G_ENDDL) << 24, 0);

        let error = backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0x1000,
            )
            .unwrap_err();

        assert!(error.to_string().contains("no persistent G_SETCIMG"));
        assert!(error.to_string().contains("output_addr state is not"));
    }

    #[test]
    fn one_cycle_fillrect_uses_primitive_combiner_and_excludes_lower_right_edges() {
        let mut rdram = vec![0u8; 0x1000];
        let commands = [
            (0xff10_0003u32, 0x400u32),
            (0xfcff_ffff, 0xfffd_f6fb),
            (0xfa00_0000, 0xff00_00ff),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
            (0xdf00_0000, 0),
        ];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = 0x100 + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        }
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: 0x100,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for x in 0..4u32 {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(0x400 + x * 2)),
                if x < 3 { 0xf801 } else { 0 },
                "one-cycle lower/right edges are exclusive at x={x}"
            );
        }
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(0x408)),
            0,
            "one-cycle lower edge must exclude row 1"
        );
    }

    #[test]
    fn one_cycle_ordered_rgb_dither_reaches_index8_color_image_bytes() {
        const DISPLAY_LIST: usize = 0x100;
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let commands = [
            // One-cycle plus G_CD_MAGICSQ in the full other-mode register.
            (0xef00_0000u32, 0),
            // I8/CI8 is the public one-byte color-image memory layout.
            (0xff48_0003, TARGET),
            // (0 - 0) * 0 + PRIMITIVE for color and alpha.
            (0xfcff_ffff, 0xfffd_f6fb),
            (0xfa00_0000, 0x0707_07ff),
            // Magic-square RGB dither is the reset selector. One-cycle
            // lower/right bounds are exclusive, producing x=0..3 at y=0.
            (0xf600_0000 | ((4 * 4) << 12) | 4, 0),
            (0xe900_0000, 0),
            (0xdf00_0000, 0),
        ];
        for (index, (word0, word1)) in commands.into_iter().enumerate() {
            let offset = DISPLAY_LIST + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
        }

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 1)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DISPLAY_LIST as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let actual = std::array::from_fn(|index| {
            view.read_u8(fn64_runtime::RdramAddr::from_offset(TARGET + index as u32))
        });
        assert_eq!(
            actual,
            [8, 8, 8, 7],
            "magic-square row zero thresholds [0,6,1,7] must perturb the common pre-write intensity lane"
        );
    }

    #[test]
    fn raw_fillrect_g_ac_dither_is_seeded_and_differs_from_g_ac_none() {
        const DL: usize = 0x100;
        const TARGET: u32 = 0x400;
        let render = |alpha_compare: u32| {
            let mut rdram = vec![0u8; 0x1000];
            let commands = [
                // One-cycle mode with only the alpha-compare selector changed.
                (0xef00_0000u32, alpha_compare),
                (0xff10_0007, TARGET),
                // (0 - 0) * 0 + PRIMITIVE for both color and alpha.
                (0xfcff_ffff, 0xfffd_f6fb),
                (0xfa00_0000, 0xff00_0080),
                // One-cycle lower/right edges are exclusive: eight pixels.
                (0xf600_0000 | ((8 * 4) << 12) | 4, 0),
                (0xdf00_0000, 0),
            ];
            for (index, (w0, w1)) in commands.into_iter().enumerate() {
                let offset = DL + index * 8;
                rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
                rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            }

            let mut backend = ReferenceBackend::new()
                .with_noise_seed(0x1234)
                .with_f3dex2()
                .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
            backend.create(&RenderConfig::new(8, 1)).unwrap();
            backend
                .process_task(
                    &mut rdram,
                    &mut fn64_runtime::RspMemory::new(),
                    &OsTask {
                        task_type: fn64_render::M_GFXTASK,
                        data_ptr: DL as u32,
                        ..OsTask::default()
                    },
                    0,
                )
                .unwrap();

            let view = fn64_runtime::RdramView::from_storage(&rdram);
            std::array::from_fn(|index| {
                view.read_u16(fn64_runtime::RdramAddr::from_offset(
                    TARGET + index as u32 * 2,
                ))
            })
        };

        assert_eq!(render(0), [0xf801; 8]);
        assert_eq!(
            render(3),
            [0xf801, 0, 0, 0, 0xf801, 0, 0xf801, 0],
            "seed 0x1234 yields noise bytes [54, 136, 181, 166, 58, 188, 62, 189]"
        );
    }

    #[test]
    fn copy_texture_rectangle_samples_rgba16_into_color_image() {
        const DL: usize = 0x100;
        const TEXTURE: u32 = 0x600;
        const TARGET: u32 = 0x800;
        let mut rdram = vec![0u8; 0x1000];
        let source = [
            0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
        ];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in source.into_iter().enumerate() {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                    pixel,
                );
            }
        }
        let mut offset = DL;
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        // Copy cycle, explicit RGBA16 destination, and RGBA16 source image.
        write_command(&mut rdram, offset, 0xef00_0000 | (2 << 20), 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xff10_0003, TARGET);
        offset += 8;
        write_command(&mut rdram, offset, 0xfd10_0003, TEXTURE);
        offset += 8;
        // Load tile 7 is contiguous; render tile 0 supplies the row stride.
        write_command(&mut rdram, offset, 0xf510_0000, 7 << 24);
        offset += 8;
        write_command(
            &mut rdram,
            offset,
            0xf300_0000,
            (7 << 24) | (7 << 12) | 0x800,
        );
        offset += 8;
        write_command(&mut rdram, offset, 0xf510_0200, 0x0008_0200);
        offset += 8;
        write_command(&mut rdram, offset, 0xf200_0000, 0x0000_c004);
        offset += 8;
        // Inclusive copy rectangle (0,0)..(3,1), tile 0.
        write_command(&mut rdram, offset, 0xe400_0000 | ((3 * 4) << 12) | 4, 0);
        offset += 8;
        // s=t=0; dsdx=4<<10 means one texel/pixel in copy mode, dtdy=1<<10.
        write_command(&mut rdram, offset, 0, 0x1000_0400);
        offset += 8;
        write_command(&mut rdram, offset, 0xe900_0000, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xdf00_0000, 0);

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for (index, expected) in source.into_iter().enumerate() {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(
                    TARGET + index as u32 * 2
                )),
                expected,
                "copied pixel {index}"
            );
        }
    }

    #[test]
    fn copy_layout_matrix_admits_only_public_direct_pairs() {
        let target = |layout| gbi::ColorImage {
            format: match layout {
                gbi::ColorImageLayout::Index8 => gbi::ColorImage::CI_FORMAT,
                gbi::ColorImageLayout::Rgba16 | gbi::ColorImageLayout::Rgba32 => {
                    gbi::ColorImage::RGBA_FORMAT
                }
            },
            size: match layout {
                gbi::ColorImageLayout::Index8 => gbi::ColorImage::BITS_8,
                gbi::ColorImageLayout::Rgba16 => gbi::ColorImage::BITS_16,
                gbi::ColorImageLayout::Rgba32 => gbi::ColorImage::BITS_32,
            },
            width: 1,
            address: 0,
        };
        for source in gbi::ColorImageLayout::ALL {
            for destination in gbi::ColorImageLayout::ALL {
                let source_image = target(source);
                let rectangle = gbi::TextureRectangle {
                    ulx: 0.0,
                    uly: 0.0,
                    lrx: 0.0,
                    lry: 0.0,
                    tile: 0,
                    s: 0.0,
                    t: 0.0,
                    dsdx: 4 << 10,
                    dtdy: 1 << 10,
                    flip: false,
                    other_mode: gbi::OtherMode::from_raw(2 << 20, 0, 0),
                    combiner: gbi::CombinerState::default(),
                    blender: gbi::BlenderState::default(),
                    scissor: None,
                    texture: Some(gbi::Texture {
                        format: source_image.format,
                        size: source_image.size,
                        width: 1,
                        height: 1,
                        texels: std::rc::Rc::new(vec![255; 4]),
                        clamp_s: true,
                        clamp_t: true,
                        mirror_s: false,
                        mirror_t: false,
                        mask_s: 0,
                        mask_t: 0,
                        shift_s: 0,
                        shift_t: 0,
                        origin_s: 0.0,
                        origin_t: 0.0,
                        tmem: None,
                        lod: None,
                    }),
                    texture1: None,
                };
                let admitted =
                    validate_copy_texture_rectangle(&rectangle, Some(target(destination))).is_ok();
                let expected = source == destination
                    && matches!(
                        source,
                        gbi::ColorImageLayout::Index8 | gbi::ColorImageLayout::Rgba16
                    );
                assert_eq!(admitted, expected, "{source:?} -> {destination:?}");
            }
        }
    }

    #[test]
    fn copy_ci8_indices_directly_to_eight_bit_color_image() {
        const DL: usize = 0x100;
        const TEXTURE: u32 = 0x600;
        const TARGET: u32 = 0x800;
        let mut rdram = vec![0u8; 0x1000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, value) in [0u8, 1, 0x7f, 0xff].into_iter().enumerate() {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32),
                    value,
                );
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(TARGET + index as u32),
                    0xaa,
                );
            }
        }
        let mut offset = DL;
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        // Copy cycle, no TLUT dereference, threshold alpha compare at index 1.
        write_command(&mut rdram, offset, 0xef00_0000 | (2 << 20), 1);
        offset += 8;
        write_command(&mut rdram, offset, 0xf900_0000, 1);
        offset += 8;
        // Public 8-bit color image and CI8 texture image, both width four.
        write_command(&mut rdram, offset, 0xff88_0003, TARGET);
        offset += 8;
        write_command(&mut rdram, offset, 0xfd48_0003, TEXTURE);
        offset += 8;
        write_command(&mut rdram, offset, 0xf548_0000, 7 << 24);
        offset += 8;
        write_command(&mut rdram, offset, 0xf300_0000, (7 << 24) | (3 << 12));
        offset += 8;
        write_command(&mut rdram, offset, 0xf548_0200, 0x0008_0200);
        offset += 8;
        write_command(&mut rdram, offset, 0xf200_0000, 0x0000_c000);
        offset += 8;
        write_command(&mut rdram, offset, 0xe400_0000 | ((3 * 4) << 12), 0);
        offset += 8;
        write_command(&mut rdram, offset, 0, 0x1000_0400);
        offset += 8;
        write_command(&mut rdram, offset, 0xe900_0000, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xdf00_0000, 0);

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 1)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let actual = std::array::from_fn(|index| {
            view.read_u8(fn64_runtime::RdramAddr::from_offset(TARGET + index as u32))
        });
        assert_eq!(actual, [0xaa, 1, 0x7f, 0xff]);
    }

    #[test]
    fn flipped_copy_texture_rectangle_transposes_rgba16_into_color_image() {
        const DL: usize = 0x100;
        const TEXTURE: u32 = 0x600;
        const TARGET: u32 = 0x800;
        let mut rdram = vec![0u8; 0x1000];
        let source = [0xf801u16, 0x07c1, 0x003f, 0xffff];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in source.into_iter().enumerate() {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                    pixel,
                );
            }
        }
        let mut offset = DL;
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        write_command(&mut rdram, offset, 0xef00_0000 | (2 << 20), 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xff10_0001, TARGET);
        offset += 8;
        write_command(&mut rdram, offset, 0xfd10_0001, TEXTURE);
        offset += 8;
        write_command(&mut rdram, offset, 0xf510_0200, 7 << 24);
        offset += 8;
        write_command(&mut rdram, offset, 0xf400_0000, (7 << 24) | (4 << 12) | 4);
        offset += 8;
        write_command(&mut rdram, offset, 0xf510_0200, 0x0008_0200);
        offset += 8;
        write_command(&mut rdram, offset, 0xf200_0000, 0x0000_4004);
        offset += 8;
        // Inclusive 2x2 copy rectangle. FLIP makes S advance down screen Y
        // and T advance across screen X while copy-mode dsdx retains 4<<10.
        write_command(&mut rdram, offset, 0xe500_0000 | (4 << 12) | 4, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0, 0x1000_0400);
        offset += 8;
        write_command(&mut rdram, offset, 0xe900_0000, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xdf00_0000, 0);

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(2, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let expected = [source[0], source[2], source[1], source[3]];
        for (index, pixel) in expected.into_iter().enumerate() {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(
                    TARGET + index as u32 * 2
                )),
                pixel,
                "transposed copy pixel {index}"
            );
        }
    }

    #[test]
    fn one_cycle_texture_rectangle_runs_combiner_into_commanded_rdram_image() {
        const DL: usize = 0x100;
        const TEXTURE: u32 = 0x600;
        const TARGET: u32 = 0x800;
        let mut rdram = vec![0u8; 0x1000];
        let source = [
            0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
        ];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in source.into_iter().enumerate() {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                    pixel,
                );
            }
        }
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };
        let combine_command = |rgb: [u32; 4], alpha: [u32; 4]| {
            let w0 = 0xfc00_0000
                | ((rgb[0] & 0x0f) << 20)
                | ((rgb[2] & 0x1f) << 15)
                | ((alpha[0] & 0x07) << 12)
                | ((alpha[2] & 0x07) << 9)
                | ((rgb[0] & 0x0f) << 5)
                | (rgb[2] & 0x1f);
            let w1 = ((rgb[1] & 0x0f) << 28)
                | ((rgb[1] & 0x0f) << 24)
                | ((alpha[0] & 0x07) << 21)
                | ((alpha[2] & 0x07) << 18)
                | ((rgb[3] & 0x07) << 15)
                | ((alpha[1] & 0x07) << 12)
                | ((alpha[3] & 0x07) << 9)
                | ((rgb[3] & 0x07) << 6)
                | ((alpha[1] & 0x07) << 3)
                | (alpha[3] & 0x07);
            (w0, w1)
        };

        let mut offset = DL;
        // (0-0)*0+TEXEL0 for RGBA in both programmed combiner slots.
        let (combine_w0, combine_w1) = combine_command([8, 8, 31, 1], [7, 7, 7, 1]);
        write_command(&mut rdram, offset, combine_w0, combine_w1);
        offset += 8;
        write_command(&mut rdram, offset, 0xff10_0003, TARGET);
        offset += 8;
        write_command(&mut rdram, offset, 0xfd10_0003, TEXTURE);
        offset += 8;
        write_command(&mut rdram, offset, 0xf510_0000, 7 << 24);
        offset += 8;
        write_command(
            &mut rdram,
            offset,
            0xf300_0000,
            (7 << 24) | (7 << 12) | 0x800,
        );
        offset += 8;
        write_command(&mut rdram, offset, 0xf510_0200, 0x0008_0200);
        offset += 8;
        write_command(&mut rdram, offset, 0xf200_0000, 0x0000_c004);
        offset += 8;
        // One-cycle lower/right bounds are exclusive: (0,0)..(4,2).
        write_command(
            &mut rdram,
            offset,
            0xe400_0000 | ((4 * 4) << 12) | (2 * 4),
            0,
        );
        offset += 8;
        write_command(&mut rdram, offset, 0, 0x0400_0400);
        offset += 8;
        write_command(&mut rdram, offset, 0xe900_0000, 0);
        offset += 8;
        write_command(&mut rdram, offset, 0xdf00_0000, 0);

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for (index, expected) in source.into_iter().enumerate() {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(
                    TARGET + index as u32 * 2
                )),
                expected,
                "combined pixel {index}"
            );
        }
    }

    #[test]
    fn combined_texture_rectangle_rejects_unmodeled_state_by_name() {
        let texture = gbi::Texture {
            format: 0,
            size: 2,
            width: 1,
            height: 1,
            texels: std::rc::Rc::new(vec![255; 4]),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let mut rectangle = gbi::TextureRectangle {
            ulx: 0.0,
            uly: 0.0,
            lrx: 1.0,
            lry: 1.0,
            tile: 0,
            s: 0.0,
            t: 0.0,
            dsdx: 1 << 10,
            dtdy: 1 << 10,
            flip: false,
            other_mode: gbi::OtherMode::default(),
            combiner: gbi::CombinerState::default(),
            blender: gbi::BlenderState {
                cycle_count: 1,
                ..gbi::BlenderState::default()
            },
            scissor: None,
            texture: Some(texture),
            texture1: None,
        };

        let shade_error = validate_texture_rectangle(&rectangle, None).unwrap_err();
        assert!(shade_error.to_string().contains("selects SHADE"));
        assert!(shade_error
            .to_string()
            .contains("rectangle commands carry no shade attributes"));

        let passthrough = gbi::CombinerCycle {
            rgb: [
                gbi::ColorSource::Zero,
                gbi::ColorSource::Zero,
                gbi::ColorSource::Zero,
                gbi::ColorSource::Texel0,
            ],
            alpha: [
                gbi::AlphaSource::Zero,
                gbi::AlphaSource::Zero,
                gbi::AlphaSource::Zero,
                gbi::AlphaSource::Texel0,
            ],
        };
        rectangle.combiner.mode.cycles = [passthrough; 2];
        rectangle.other_mode = gbi::OtherMode::from_raw(gbi::OtherMode::default().raw_high(), 3, 0);
        validate_texture_rectangle(&rectangle, None)
            .expect("G_AC_DITHER is implemented for combined rectangles");

        rectangle.other_mode =
            gbi::OtherMode::from_raw(gbi::OtherMode::default().raw_high(), 0x10, 0);
        let depth_error = validate_texture_rectangle(&rectangle, None).unwrap_err();
        assert!(depth_error
            .to_string()
            .contains("rectangles require G_ZS_PRIM"));
    }

    #[test]
    fn copy_texture_rectangle_rejects_mismatched_memory_layouts() {
        let texture = gbi::Texture {
            format: gbi::ColorImage::CI_FORMAT,
            size: gbi::ColorImage::BITS_8,
            width: 1,
            height: 1,
            texels: std::rc::Rc::new(vec![1; 4]),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let mut rectangle = gbi::TextureRectangle {
            ulx: 0.0,
            uly: 0.0,
            lrx: 1.0,
            lry: 1.0,
            tile: 0,
            s: 0.0,
            t: 0.0,
            dsdx: 4 << 10,
            dtdy: 1 << 10,
            flip: false,
            other_mode: gbi::OtherMode::from_raw(2 << 20, 0, 0),
            combiner: gbi::CombinerState::default(),
            blender: gbi::BlenderState::default(),
            scissor: None,
            texture: Some(texture),
            texture1: None,
        };
        let rgba16_target = gbi::ColorImage {
            format: gbi::ColorImage::RGBA_FORMAT,
            size: gbi::ColorImage::BITS_16,
            width: 1,
            address: 0,
        };
        let index8_target = gbi::ColorImage {
            format: gbi::ColorImage::CI_FORMAT,
            size: gbi::ColorImage::BITS_8,
            width: 1,
            address: 0,
        };
        rectangle.other_mode = gbi::OtherMode::from_raw(2 << 20, 3, 0);
        validate_texture_rectangle(&rectangle, Some(index8_target))
            .expect("G_AC_DITHER is implemented for direct CI8 copy rectangles");
        let error = validate_texture_rectangle(&rectangle, Some(rgba16_target)).unwrap_err();
        assert!(error.to_string().contains("does not match color target"));
        assert!(error.to_string().contains("format=0 size=2"));
    }

    #[test]
    fn admitted_s2dex_object_rectangle_renders_preloaded_tmem_to_rdram() {
        const SETUP: usize = 0x100;
        const DL: usize = 0x300;
        const SPRITE: u32 = 0x400;
        const TEXTURE: u32 = 0x800;
        const TARGET: u32 = 0x1000;
        let mut rdram = vec![0u8; 0x2000];
        let source = [
            0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
        ];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in source.into_iter().enumerate() {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                    pixel,
                );
            }
            let base = fn64_runtime::RdramAddr::from_offset(SPRITE);
            let mut half = |offset, value| view.write_u16(base.checked_add(offset).unwrap(), value);
            half(0, 0); // objX, s10.2
            half(2, 1 << 10); // scaleW, u5.10
            half(4, 4 << 5); // imageW, u10.5
            half(6, 0);
            half(8, 0); // objY, s10.2
            half(10, 1 << 10); // scaleH, u5.10
            half(12, 2 << 5); // imageH, u10.5
            half(14, 0);
            half(16, 1); // one 64-bit word per four-pixel RGBA16 row
            half(18, 0); // TMEM word zero
            view.write_u8(base.checked_add(20).unwrap(), 0); // RGBA
            view.write_u8(base.checked_add(21).unwrap(), 2); // 16-bit
            view.write_u8(base.checked_add(22).unwrap(), 0); // palette
            view.write_u8(base.checked_add(23).unwrap(), 0); // no flips
        }
        let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        };

        // Establish persistent RDP state/TMEM through the existing raw-DPC
        // path. Public S2DEX keeps texture loading separate from sprite draw.
        // (0-0)*0+TEXEL0 in both programmed combiner cycles.
        let combine_texel0 = (0xfc8f_ff1f, 0x88fc_f279);
        let setup = [
            combine_texel0,
            (0xff10_0003, TARGET),
            (0xfd10_0003, TEXTURE),
            (0xf510_0000, 7 << 24),
            (0xf300_0000, (7 << 24) | (7 << 12) | 0x800),
        ];
        for (index, (w0, w1)) in setup.into_iter().enumerate() {
            write_command(&mut rdram, SETUP + index * 8, w0, w1);
        }
        write_command(&mut rdram, DL, 0x0100_0000, SPRITE);
        write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);
        let mut direct_rdram = rdram.clone();

        let mut backend = ReferenceBackend::new()
            .with_s2dex()
            .with_s2dex_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        backend
            .process_rdp_commands(
                &mut rdram,
                SETUP as u32,
                (SETUP + setup.len() * 8) as u32,
                0,
            )
            .unwrap();
        assert_eq!(backend.supported_ucodes(), &[UcodeId::S2dex2]);
        assert_eq!(
            backend
                .process_task(
                    &mut rdram,
                    &mut fn64_runtime::RspMemory::new(),
                    &OsTask {
                        task_type: fn64_render::M_GFXTASK,
                        data_ptr: DL as u32,
                        ..OsTask::default()
                    },
                    0,
                )
                .unwrap(),
            FrameStatus::Complete
        );

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for (index, expected) in source.into_iter().enumerate() {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(
                    TARGET + index as u32 * 2
                )),
                expected,
                "S2DEX object pixel {index} must come from preloaded TMEM"
            );
        }

        // Differential: execute the exact RDP tile + texture-rectangle state
        // S2DEX is documented to generate and require byte-identical output.
        const DIRECT: usize = 0x500;
        let equivalent_rdp = [
            (0xf510_0200, 0x0008_0200), // RGBA16, line=1, clamp S/T
            (0xf200_0000, 0x0000_c004), // 4x2 render-tile extent
            (0xe401_0008, 0),           // exclusive (0,0)..(4,2)
            (0, 0x0400_0400),           // s=t=0, unit S/T gradients
            (0xe900_0000, 0),
        ];
        for (index, (w0, w1)) in equivalent_rdp.into_iter().enumerate() {
            write_command(&mut direct_rdram, DIRECT + index * 8, w0, w1);
        }
        let mut direct = ReferenceBackend::new();
        direct.create(&RenderConfig::new(4, 2)).unwrap();
        direct
            .process_rdp_commands(
                &mut direct_rdram,
                SETUP as u32,
                (SETUP + setup.len() * 8) as u32,
                0,
            )
            .unwrap();
        direct
            .process_rdp_commands(
                &mut direct_rdram,
                DIRECT as u32,
                (DIRECT + equivalent_rdp.len() * 8) as u32,
                0,
            )
            .unwrap();
        let s2dex_target = &rdram[TARGET as usize..TARGET as usize + source.len() * 2];
        let direct_target = &direct_rdram[TARGET as usize..TARGET as usize + source.len() * 2];
        assert_eq!(
            s2dex_target, direct_target,
            "S2DEX lowering must match the equivalent raw RDP rectangle byte-for-byte"
        );
    }

    #[test]
    fn s2dex_backend_reports_only_admitted_wire_families() {
        let legacy = [1; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let modern = [2; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let backend = ReferenceBackend::new().with_s2dex();
        assert!(backend.supported_ucodes().is_empty());

        let backend = backend.with_s2dex_ucode_text_for(S2dexWireFamily::S2dex, &legacy);
        assert_eq!(backend.supported_ucodes(), &[UcodeId::S2dex]);

        let backend = backend.with_s2dex_ucode_text(&modern);
        assert_eq!(
            backend.supported_ucodes(),
            &[UcodeId::S2dex, UcodeId::S2dex2]
        );
    }

    #[test]
    fn admitted_legacy_s2dex_digest_selects_legacy_command_bytes() {
        const DL: usize = 0x100;
        let text = [0; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let mut rdram = vec![0u8; 0x200];
        rdram[DL..DL + 4].copy_from_slice(&0xb800_0000u32.to_ne_bytes());
        let mut backend = ReferenceBackend::new()
            .with_s2dex()
            .with_s2dex_ucode_text_for(S2dexWireFamily::S2dex, &text);
        backend.create(&RenderConfig::new(1, 1)).unwrap();
        assert_eq!(
            backend
                .process_task(
                    &mut rdram,
                    &mut fn64_runtime::RspMemory::new(),
                    &OsTask {
                        task_type: fn64_render::M_GFXTASK,
                        data_ptr: DL as u32,
                        ..OsTask::default()
                    },
                    0,
                )
                .unwrap(),
            FrameStatus::Complete
        );
    }

    #[test]
    fn s2dex_unsupported_load_command_traps_by_public_name() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        const DL: usize = 0x100;
        let mut rdram = vec![0u8; 0x200];
        rdram[DL..DL + 4].copy_from_slice(&0x0500_0017u32.to_ne_bytes());
        rdram[DL + 4..DL + 8].copy_from_slice(&0x180u32.to_ne_bytes());
        let before = rdram.clone();
        let mut backend = ReferenceBackend::new()
            .with_s2dex()
            .with_s2dex_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::new(2, 2)).unwrap();
        let error = backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap_err();
        assert!(error.to_string().contains("G_OBJ_LOADTXTR"));
        assert!(error.to_string().contains("unsupported S2DEX command"));
        assert_eq!(rdram, before, "rejected S2DEX decode must not mutate RDRAM");
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].subsystem,
            fn64_runtime::UnsupportedSubsystem::Render
        );
        assert_eq!(events[0].operation, "render.s2dex.object-texture-type");
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::ReturnedError
        );
        assert!(events[0].context.contains("G_OBJ_LOADTXTR"));
    }

    #[test]
    fn unadmitted_s2dex_image_requests_lle_without_task_mutation() {
        const DL: usize = 0x100;
        let mut rdram = vec![0u8; 0x200];
        rdram[DL..DL + 4].copy_from_slice(&0xdf00_0000u32.to_ne_bytes());
        let before = rdram.clone();
        let mut rsp = fn64_runtime::RspMemory::new();
        rsp.write_bytes(
            fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
            &[0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        )
        .unwrap();
        let rsp_before = rsp.clone();
        let expected =
            gbi::UcodeDigest::from_text(rsp.bank(fn64_runtime::RspMemoryBank::Imem)).as_bytes();
        let mut backend = ReferenceBackend::new().with_s2dex();
        backend.create(&RenderConfig::new(2, 2)).unwrap();
        assert_eq!(
            backend
                .process_task(
                    &mut rdram,
                    &mut rsp,
                    &OsTask {
                        task_type: fn64_render::M_GFXTASK,
                        data_ptr: DL as u32,
                        ..OsTask::default()
                    },
                    0,
                )
                .unwrap(),
            FrameStatus::NeedsLle {
                ucode_sha256: expected
            }
        );
        assert_eq!(rdram, before);
        assert_eq!(rsp, rsp_before);
    }

    #[test]
    fn backend_identity_binds_fn64_adapter_source_sha256() {
        let baseline = Rt64BackendIdentity {
            adapter: "fn64-render-rt64/rt64",
            adapter_source_sha256:
                "1111111111111111111111111111111111111111111111111111111111111111",
            source_id: "git:2222222222222222222222222222222222222222",
            source_provenance: Rt64SourceProvenance::GitClean,
            source_overlay_id: "fn64:test-overlay:v1",
            post_vi_api: "metal-bgra8-unorm",
        };
        let changed = Rt64BackendIdentity {
            adapter_source_sha256:
                "3333333333333333333333333333333333333333333333333333333333333333",
            ..baseline.clone()
        };
        assert_ne!(baseline.canonical_id(), changed.canonical_id());
        assert!(baseline
            .canonical_id()
            .contains("adapter_sha256=1111111111111111"));
    }
}
