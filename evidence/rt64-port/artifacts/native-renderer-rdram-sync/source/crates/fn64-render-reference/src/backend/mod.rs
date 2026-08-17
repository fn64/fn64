// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use crate::raster::Framebuffer;
use crate::gbi;
use fn64_render::{
    F3dex2UcodeCatalog, MicrocodePairCatalog, OsTask, RenderBackend, S2dexUcodeCatalog, ViPresentation,
};


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

#[derive(Clone)]
pub struct ReferenceBackend {
    /// TV standard accepted by the last successful `create`. Clearing this
    /// before recreation prevents failed attempts from retaining stale
    /// release authority.
    active_tv_type: Option<fn64_runtime::TvType>,
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
    rdram_hidden_bits: RdramHiddenBits,
    clear_color: [u8; 4],
    noise_seed: u64,
    decode_mode: DecodeMode,
    /// Exact geometry-microcode text images allowed at task entry and after a
    /// `G_LOAD_UCODE`, together with their public command-wire families.
    /// Selecting the decode mode does not admit content.
    f3dex2_ucodes: F3dex2UcodeCatalog,
    /// Exact S2DEX/S2DEX2-compatible task-entry images and their public wire
    /// families. No F3DEX2 digest or opcode-family guess is inherited.
    s2dex_ucodes: S2dexUcodeCatalog,
    /// Exact complete text/data pairs admitted independently for runtime
    /// consumption evidence. Text-only HLE catalogs cannot populate this.
    microcode_pairs: MicrocodePairCatalog,
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
    /// Suppress non-rollbackable environment-driven diagnostic files while a
    /// complete raw-DPC batch is executing against a speculative clone.
    #[cfg(not(test))]
    suppress_task_diagnostics: bool,
    /// Backend-owned checkpoint for the one HLE task currently between
    /// committed operation boundaries.
    continuation: Option<ReferenceTaskContinuation>,
    next_continuation_token: u64,
    /// Reused scratch buffer for `process_rdp_commands`'s terminator-append
    /// copy. Holds no state between calls -- every call clears and refills
    /// it before reading it -- kept only so each of the ~18,838 raw-RDP
    /// tasks/route reuses one allocation's capacity instead of `to_vec()`ing
    /// a fresh 8 MiB RDRAM copy from scratch.
    raw_rdp_scratch: Vec<u8>,
    /// Exact visible RDRAM byte ranges written by a synthetic IR execution.
    /// `None` on every production path. The adapter enables it only on its
    /// disposable backend clone so command staging and equal-value renderer
    /// writes remain distinguishable without any process-global observer.
    ir_rdram_write_trace: Option<Vec<(usize, usize)>>,
}

#[derive(Clone)]
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

#[derive(Clone)]
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
    /// Report the first task omitted by the bound, then remain quiet.
    limit_reported: bool,
}

impl Default for ReferenceBackend {
    fn default() -> Self {
        Self::new()
    }
}


mod hidden_bits;
mod vi_source;
mod imp;
mod ir_adapter;
mod render_backend;
mod validate;
mod framebuffer_io;

pub use ir_adapter::ReferenceIrRawDpcAdapter;

use hidden_bits::*;

#[cfg(test)]
mod tests;
