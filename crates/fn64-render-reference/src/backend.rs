use crate::raster::Framebuffer;
use crate::{
    depth, gbi, png_dump, raster, render_unsupported_error, s2dex, vi, GeometryWireFamily,
    S2dexWireFamily,
};
use fn64_render::{
    F3dex2UcodeCatalog, FrameStatus, MicrocodeDataImageIdentity, MicrocodePairCatalog,
    NonRdpWrite16, NonRdpWrite16Disposition, OsTask, PresentMemory, PresentRequest, RenderBackend,
    RenderConfig, RenderError, S2dexUcodeCatalog, UcodeId, ViPixelType, ViPresentation,
    ViScanoutRegisters,
};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct RdramHiddenSample {
    visible: u16,
    bits: u8,
}

/// Dense physical storage for RDRAM's two hidden bits and their visible-word
/// coherence marker. Hidden state is indexed by physical halfword, so a hash
/// table paid hashing/allocation costs for a naturally dense bounded address
/// space. `u32::MAX` is outside the 18-bit packed sample domain and represents
/// an untouched halfword.
#[derive(Clone, Debug)]
struct RdramHiddenBits {
    samples: Vec<u32>,
}

impl RdramHiddenBits {
    const EMPTY: u32 = u32::MAX;
    const HALFWORDS: usize = fn64_runtime::rdram::DEFAULT_RDRAM_SIZE / 2;

    fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }

    fn slot(address: u32) -> Option<usize> {
        if address & 1 != 0 || address >= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u32 {
            return None;
        }
        Some(address as usize >> 1)
    }

    fn decode(packed: u32) -> Option<RdramHiddenSample> {
        (packed != Self::EMPTY).then_some(RdramHiddenSample {
            visible: packed as u16,
            bits: ((packed >> 16) & 3) as u8,
        })
    }

    fn encode(sample: RdramHiddenSample) -> u32 {
        u32::from(sample.visible) | (u32::from(sample.bits & 3) << 16)
    }

    fn ensure_storage(&mut self) {
        if self.samples.is_empty() {
            self.samples.resize(Self::HALFWORDS, Self::EMPTY);
        }
    }

    fn get(&self, address: &u32) -> Option<RdramHiddenSample> {
        let slot = Self::slot(*address)?;
        self.samples.get(slot).copied().and_then(Self::decode)
    }

    fn insert(&mut self, address: u32, sample: RdramHiddenSample) {
        let slot = Self::slot(address).unwrap_or_else(|| {
            panic!("hidden-RDRAM address must be an in-range halfword: {address:#010x}")
        });
        self.ensure_storage();
        self.samples[slot] = Self::encode(sample);
    }

    fn insert_pair(&mut self, address: u32, first: RdramHiddenSample, second: RdramHiddenSample) {
        assert!(
            address.is_multiple_of(4),
            "hidden-RDRAM pair must begin at a word boundary: {address:#010x}"
        );
        let slot = Self::slot(address).unwrap_or_else(|| {
            panic!("hidden-RDRAM address must be an in-range halfword: {address:#010x}")
        });
        self.ensure_storage();
        let pair = self
            .samples
            .get_mut(slot..slot + 2)
            .expect("hidden-RDRAM word extends outside dense storage");
        pair[0] = Self::encode(first);
        pair[1] = Self::encode(second);
    }

    fn update_visible(&mut self, address: u32, visible: u16) {
        let Some(slot) = Self::slot(address) else {
            return;
        };
        let Some(mut sample) = self.samples.get(slot).copied().and_then(Self::decode) else {
            return;
        };
        sample.visible = visible;
        self.samples[slot] = Self::encode(sample);
    }

    fn contains_key(&self, address: &u32) -> bool {
        self.get(address).is_some()
    }

    fn extend(&mut self, updates: impl IntoIterator<Item = (u32, RdramHiddenSample)>) {
        for (address, sample) in updates {
            self.insert(address, sample);
        }
    }

    fn clear(&mut self) {
        self.samples.fill(Self::EMPTY);
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.samples.iter().all(|sample| *sample == Self::EMPTY)
    }
}

impl<const N: usize> From<[(u32, RdramHiddenSample); N]> for RdramHiddenBits {
    fn from(entries: [(u32, RdramHiddenSample); N]) -> Self {
        let mut hidden = Self::new();
        hidden.extend(entries);
        hidden
    }
}

fn read_rdram_hidden_bits(hidden: &mut RdramHiddenBits, address: u32, visible: u16) -> u8 {
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ViSourceGeometry {
    origin: u32,
    stride_pixels: u32,
    rows: u64,
    bytes_per_pixel: u8,
    layout: gbi::ColorImageLayout,
}

/// Add the deterministic reference filter's bottom halo to the public
/// programmed span. The public patents establish the filter topology but not
/// out-of-window bus fetches, so this remains a reference policy rather than a
/// native RT64 or silicon footprint claim.
fn reference_vi_source_geometry(
    vi: ViPresentation,
) -> Result<Option<ViSourceGeometry>, RenderError> {
    let filters = vi.scanout.filters();
    let resample = vi.scanout.registers().map(ViScanoutRegisters::resample);
    let aa_halo = if filters.antialias_mode.silhouette_aa_enabled() {
        if resample.is_some_and(|value| value.field.interlaced()) {
            2
        } else {
            1
        }
    } else {
        0
    };
    let restoration_halo = u64::from(filters.dither_filter);
    let geometry = vi_source_geometry_with_bottom_halo(vi, aa_halo.max(restoration_halo))?;
    if let Some(geometry) = geometry {
        if geometry.bytes_per_pixel == 2 && !geometry.origin.is_multiple_of(2) {
            return Err(RenderError::InvalidViSourceAlignment {
                origin: geometry.origin,
                bytes_per_pixel: geometry.bytes_per_pixel,
            });
        }
    }
    Ok(geometry)
}

fn vi_source_geometry_with_bottom_halo(
    vi: ViPresentation,
    bottom_halo: u64,
) -> Result<Option<ViSourceGeometry>, RenderError> {
    let filters = vi.scanout.filters();
    if filters.pixel_type == ViPixelType::Reserved {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: "VI STATUS selects reserved pixel type 1".to_string(),
        });
    }
    let Some(registers) = vi.scanout.registers() else {
        return Ok(None);
    };
    let Some(window) = registers.active_window() else {
        return Ok(None);
    };
    if vi.blanked || filters.pixel_type == ViPixelType::Blank {
        return Ok(None);
    }
    let (bytes_per_pixel, layout) = match filters.pixel_type {
        ViPixelType::Rgba16 => (2, gbi::ColorImageLayout::Rgba16),
        ViPixelType::Rgba32 => (4, gbi::ColorImageLayout::Rgba32),
        ViPixelType::Blank | ViPixelType::Reserved | ViPixelType::Unspecified => unreachable!(),
    };
    let origin = registers.origin();
    let output_rows = u64::from(window.output_height());
    let resample = registers.resample();
    let last_output = output_rows
        .checked_sub(1)
        .expect("active VI window has no output rows");
    let last_u10 = u64::from(resample.y.offset_u2_10())
        .checked_add(
            last_output
                .checked_mul(u64::from(resample.y.step_u2_10()))
                .expect("VI vertical coordinate product overflow"),
        )
        .expect("VI vertical coordinate sum overflow");
    let last_center = last_u10 >> fn64_render::ViScaleAxis::FRACTION_BITS;
    let sample_extra = u64::from(filters.antialias_mode.resampling_enabled());
    let mut rows = last_center
        .checked_add(sample_extra)
        .and_then(|value| value.checked_add(bottom_halo))
        .and_then(|value| value.checked_add(1))
        .expect("VI reference source row count overflow");
    if vi.fade.is_some() {
        rows = rows.max(2);
    }
    Ok(Some(ViSourceGeometry {
        origin,
        stride_pixels: registers.width(),
        rows,
        bytes_per_pixel,
        layout,
    }))
}

fn load_vi_source(
    memory: &fn64_runtime::PhysicalRdramRead<'_>,
    geometry: ViSourceGeometry,
    hidden: &RdramHiddenBits,
) -> Result<(Framebuffer, Vec<(u32, RdramHiddenSample)>), RenderError> {
    validate_vi_source_footprint(memory, geometry)?;
    let height = geometry.rows as u32;
    let mut source = Framebuffer::new(geometry.stride_pixels, height);
    source.set_color_layout(geometry.layout);
    let pixel_count = u64::from(geometry.stride_pixels)
        .checked_mul(geometry.rows)
        .expect("VI source pixel count overflow");
    let mut hidden_updates = Vec::new();
    for index in 0..pixel_count {
        let byte_offset = index
            .checked_mul(u64::from(geometry.bytes_per_pixel))
            .expect("VI source pixel offset overflow");
        let logical = u64::from(geometry.origin)
            .checked_add(byte_offset)
            .expect("VI source pixel address overflow");
        let logical = u32::try_from(logical).expect("bounded VI source address exceeds u32");
        let destination = usize::try_from(index).expect("VI source index exceeds usize") * 4;
        match geometry.layout {
            gbi::ColorImageLayout::Rgba16 => {
                let pixel = memory.read_u16(fn64_runtime::RdramAddr::from_offset(logical));
                let hidden_bits = match hidden.get(&logical) {
                    Some(sample) if sample.visible == pixel => sample.bits & 3,
                    _ => {
                        let bits = if pixel & 1 == 0 { 0 } else { 3 };
                        hidden_updates.push((
                            logical,
                            RdramHiddenSample {
                                visible: pixel,
                                bits,
                            },
                        ));
                        bits
                    }
                };
                let expand = |value: u16| {
                    let value = value as u8;
                    (value << 3) | (value >> 2)
                };
                source.pixels[destination..destination + 4].copy_from_slice(&[
                    expand((pixel >> 11) & 0x1f),
                    expand((pixel >> 6) & 0x1f),
                    expand((pixel >> 1) & 0x1f),
                    255,
                ]);
                let stored_coverage = (((pixel & 1) as u8) << 2) | hidden_bits;
                source.coverage[index as usize] = raster::Coverage::from_stored(stored_coverage);
            }
            gbi::ColorImageLayout::Rgba32 => {
                let address = fn64_runtime::RdramAddr::from_offset(logical);
                let red = memory.read_u8(address);
                let green = memory.read_u8(
                    address
                        .checked_add(1)
                        .expect("VI RGBA32 green address overflow"),
                );
                let blue = memory.read_u8(
                    address
                        .checked_add(2)
                        .expect("VI RGBA32 blue address overflow"),
                );
                let alpha_coverage = memory.read_u8(
                    address
                        .checked_add(3)
                        .expect("VI RGBA32 alpha address overflow"),
                );
                let alpha5 = alpha_coverage & 0x1f;
                source.pixels[destination..destination + 4].copy_from_slice(&[
                    red,
                    green,
                    blue,
                    (alpha5 << 3) | (alpha5 >> 2),
                ]);
                source.coverage[index as usize] =
                    raster::Coverage::from_stored(alpha_coverage >> 5);
            }
            gbi::ColorImageLayout::Index8 => unreachable!(),
        }
    }
    Ok((source, hidden_updates))
}

fn validate_vi_source_footprint(
    memory: &fn64_runtime::PhysicalRdramRead<'_>,
    geometry: ViSourceGeometry,
) -> Result<(), RenderError> {
    let row_bytes = u64::from(geometry.stride_pixels)
        .checked_mul(u64::from(geometry.bytes_per_pixel))
        .expect("VI source row byte count overflow");
    let byte_len = row_bytes
        .checked_mul(geometry.rows)
        .expect("VI source footprint overflow");
    let end = u64::from(geometry.origin)
        .checked_add(byte_len)
        .expect("VI source end overflow");
    if end > memory.len() as u64 || geometry.rows > u64::from(u32::MAX) {
        return Err(RenderError::InvalidViSourceBounds {
            origin: geometry.origin,
            stride_pixels: geometry.stride_pixels,
            rows: geometry.rows,
            bytes_per_pixel: geometry.bytes_per_pixel,
            rdram_len: memory.len(),
        });
    }
    Ok(())
}

/// Record a known non-RDP 16-bit write to one physical RDRAM halfword.
///
/// Programming Manual 15.5.6 defines this mutation even when the visible
/// value is unchanged: both hidden bits receive the visible LSB. The renderer
/// calls this from its changed-visible-word fallback. A same-value external
/// store requires the host to provide a write event because `&mut [u8]`
/// alone cannot distinguish that store from no mutation.
fn record_non_rdp_16bit_write(hidden: &mut RdramHiddenBits, address: u32, visible: u16) -> u8 {
    let bits = if visible & 1 == 0 { 0 } else { 3 };
    hidden.insert(address, RdramHiddenSample { visible, bits });
    bits
}

fn write_rdram_hidden_bits(hidden: &mut RdramHiddenBits, address: u32, visible: u16, bits: u8) {
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
    hidden: &mut RdramHiddenBits,
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
        hidden.update_visible(
            address,
            view.read_u16(fn64_runtime::RdramAddr::from_offset(address)),
        );
    }
}
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

impl ReferenceBackend {
    pub fn new() -> Self {
        ReferenceBackend {
            active_tv_type: None,
            fb: None,
            presented_fb: None,
            presentation: ViPresentation::default(),
            color_image: None,
            depth_image: None,
            primitive_depth: None,
            rdp_decode_state: gbi::RdpDecodeState::default(),
            rdram_hidden_bits: RdramHiddenBits::new(),
            clear_color: [0, 0, 0, 255],
            noise_seed: Framebuffer::DEFAULT_NOISE_SEED,
            decode_mode: DecodeMode::Simple,
            f3dex2_ucodes: F3dex2UcodeCatalog::default(),
            s2dex_ucodes: S2dexUcodeCatalog::default(),
            microcode_pairs: MicrocodePairCatalog::default(),
            last_dp_full_sync: fn64_render::DpFullSyncStatus::Unidentified,
            auto_dump: None,
            #[cfg(not(test))]
            diag_task_index: 0,
            #[cfg(not(test))]
            suppress_task_diagnostics: false,
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

    /// Admit one exact complete microcode text/data identity for runtime
    /// recognition evidence. This is separate from HLE text admission.
    pub fn with_microcode_pair_sha256(
        mut self,
        family: UcodeId,
        text_sha256: [u8; 32],
        data_bytes: u32,
        data_sha256: [u8; 32],
    ) -> Self {
        self.microcode_pairs.admit(
            family,
            text_sha256,
            MicrocodeDataImageIdentity {
                bytes: data_bytes,
                sha256: data_sha256,
            },
        );
        self
    }

    /// Byte-backed fixture convenience for [`Self::with_microcode_pair_sha256`].
    pub fn with_microcode_pair(mut self, family: UcodeId, text: &[u8], data: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "microcode pair admission requires one complete 4 KiB IMEM image"
        );
        let data_bytes = u32::try_from(data.len())
            .expect("microcode pair data length exceeds the OSTask u32 size field");
        self.microcode_pairs.admit(
            family,
            sha2::Sha256::digest(text).into(),
            MicrocodeDataImageIdentity {
                bytes: data_bytes,
                sha256: sha2::Sha256::digest(data).into(),
            },
        );
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
            limit_reported: false,
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
            if !self.suppress_task_diagnostics {
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
                            .unwrap_or_else(|| {
                                std::path::PathBuf::from("/tmp/fn64-gfx-task-dumps")
                            });
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
                            "[fn64-render-reference] dumped gfx task #{dump_index} ({tri_count} reference \
                             triangles) to {path:?}"
                        );
                    }
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
                        "[fn64-render-reference] gfx task #{idx}: decoded {} triangle(s); \
                         framebuffer is UNIFORM clear -- reported blank, not dumped.",
                        state.tri_count
                    );
                } else if dump.written >= dump.limit {
                    if !dump.limit_reported {
                        eprintln!(
                            "[fn64-render-reference] gfx task #{idx}: non-clear ({} tris) but \
                             auto-dump limit ({}) reached -- suppressing later dump notices.",
                            state.tri_count, dump.limit
                        );
                        dump.limit_reported = true;
                    }
                } else {
                    let _ = std::fs::create_dir_all(&dump.dir);
                    let path = dump
                        .dir
                        .join(format!("{}-{:04}.png", dump.prefix, dump.written));
                    match png_dump::write_png(&path, fb.width, fb.height, &fb.pixels) {
                        Ok(()) => {
                            dump.written += 1;
                            eprintln!(
                                "[fn64-render-reference] gfx task #{idx}: NON-CLEAR ({} tris) \
                                 -- dumped {}",
                                state.tri_count,
                                path.display()
                            );
                        }
                        Err(error) => eprintln!(
                            "[fn64-render-reference] gfx task #{idx}: failed to write {}: {error}",
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
        self.active_tv_type.map_or(
            fn64_render::RenderBackendEvidence::Unidentified,
            |tv_type| fn64_render::RenderBackendEvidence::Reference { tv_type },
        )
    }

    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError> {
        self.active_tv_type = None;
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
        self.active_tv_type = Some(cfg.tv_type);
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
        let mut state = match self.prepare_reference_task(rdram, rsp_memory, task, output_addr)? {
            PreparedReferenceTask::Ready(state) => state,
            PreparedReferenceTask::NeedsLle(ucode_sha256) => {
                return Ok(FrameStatus::NeedsLle { ucode_sha256 });
            }
        };
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
        while state.next_operation < state.operations.len() {
            let operation = state.operations[state.next_operation].clone();
            state.next_operation += 1;
            self.execute_reference_operation(rdram, &mut state, &operation)?;
            state.reached_dp_full_sync |= matches!(operation, gbi::RenderOp::FullSync);
        }
        let dp_full_sync = if state.reached_dp_full_sync {
            fn64_render::DpFullSyncStatus::Reached
        } else {
            fn64_render::DpFullSyncStatus::NotReached
        };
        // This trait call is atomic with respect to guest execution: unlike
        // `process_task_chunk`, it publishes no continuation at which SIG0 or
        // another guest thread can observe RDRAM. Target changes and FullSync
        // still commit inside `execute_reference_operation`; the remaining
        // dirty image needs one commit at the task boundary.
        self.finish_reference_task(rdram, state)?;
        self.last_dp_full_sync = dp_full_sync;
        Ok(FrameStatus::Complete)
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

    fn raw_dpc_batch_capability(&self) -> fn64_render::RawDpcBatchCapability {
        fn64_render::RawDpcBatchCapability::DiagnosticOnly
    }

    fn process_raw_dpc_batch(
        &mut self,
        rdram: &mut [u8],
        batch: fn64_render::PreflightedRawDpcBatch,
        output_addr: u32,
    ) -> Result<fn64_render::RawDpcBatchOutcome, RenderError> {
        let expected_full_sync = batch.aggregate_full_sync();
        let outcome = batch.outcome();
        let groups = batch.stream_groups().to_vec();
        let mut image = batch.staged_image(rdram)?;
        let mut speculative = self.clone();
        // A diagnostic file cannot be rolled back if a later stream group
        // rejects. Retain the configured sink and its counters outside the
        // speculative backend, then restore them only at the batch commit.
        let retained_auto_dump = speculative.auto_dump.take();
        #[cfg(not(test))]
        {
            speculative.suppress_task_diagnostics = true;
        }
        for group in groups {
            let mut group_image = image.clone();
            let status = speculative.process_rdp_commands(
                &mut group_image,
                group.staging_start(),
                group.staging_end(),
                output_addr,
            )?;
            if status != FrameStatus::Complete {
                return Err(RenderError::Backend {
                    backend: "reference-raw-dpc-batch",
                    reason: format!("raw-DPC stream group returned nonterminal status {status:?}"),
                });
            }
            if speculative.last_dp_full_sync() != group.full_sync() {
                return Err(RenderError::Backend {
                    backend: "reference-raw-dpc-batch",
                    reason: format!(
                        "renderer reported {:?} after group preflight proved {:?}",
                        speculative.last_dp_full_sync(),
                        group.full_sync()
                    ),
                });
            }
            image[..rdram.len()].copy_from_slice(&group_image[..rdram.len()]);
        }
        speculative.last_dp_full_sync = expected_full_sync;
        speculative.auto_dump = retained_auto_dump;
        #[cfg(not(test))]
        {
            speculative.suppress_task_diagnostics = false;
        }
        rdram.copy_from_slice(&image[..rdram.len()]);
        *self = speculative;
        Ok(outcome)
    }

    fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
        self.last_dp_full_sync
    }

    fn task_chunking(&self) -> fn64_render::RenderTaskChunking {
        fn64_render::RenderTaskChunking::Resumable
    }

    fn present(&mut self, request: PresentRequest<'_>) -> Result<(), RenderError> {
        let (vi, memory) = request.into_parts();
        let resident = self
            .fb
            .as_ref()
            .ok_or(RenderError::NotReady("create() not called"))?;
        let (presented, hidden_updates) = match memory {
            PresentMemory::BackendResidentCompatibility => (vi::scanout(resident, vi)?, Vec::new()),
            PresentMemory::Physical(memory) => {
                if vi.scanout.registers().is_none() {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "physical VI presentation requires a live register image"
                            .to_string(),
                    });
                }
                match reference_vi_source_geometry(vi)? {
                    Some(geometry) => {
                        let (source, hidden_updates) =
                            load_vi_source(&memory, geometry, &self.rdram_hidden_bits)?;
                        (vi::scanout(&source, vi)?, hidden_updates)
                    }
                    None => (vi::scanout(resident, vi)?, Vec::new()),
                }
            }
        };
        self.presented_fb = Some(presented);
        self.presentation = vi;
        self.rdram_hidden_bits.extend(hidden_updates);
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
        if self.presentation.scanout.registers().is_some() {
            // A resize has no retrace-scoped RDRAM authority. Never rebuild a
            // live register image from the unrelated resident RDP surface;
            // the next field reconstructs it from current physical bytes.
            self.presented_fb = None;
        } else if let Some(fb) = &self.fb {
            // `resize` is infallible by trait contract. If the new dimensions
            // cannot support the retained VI effect, leave no fabricated
            // scanout; the next `present` reports the named error.
            self.presented_fb = vi::scanout(fb, self.presentation).ok();
        }
    }

    fn identify_microcode(
        &self,
        imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    ) -> Option<UcodeId> {
        let geometry = self.f3dex2_ucodes.identify_text(imem);
        let sprite = self.s2dex_ucodes.identify_text(imem);
        match (geometry, sprite) {
            (Some(geometry), Some(sprite)) => {
                panic!("one microcode digest cannot identify both {geometry:?} and {sprite:?}")
            }
            (Some(ucode), None) | (None, Some(ucode)) => Some(ucode),
            (None, None) => None,
        }
    }

    fn identify_microcode_pair(
        &self,
        imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        data: MicrocodeDataImageIdentity,
    ) -> Option<UcodeId> {
        self.microcode_pairs.identify(imem, data)
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
    let direct_8bit = texture.size == gbi::ColorImage::BITS_8
        && match texture.format {
            gbi::ColorImage::I_FORMAT | gbi::ColorImage::IA_FORMAT => true,
            gbi::ColorImage::CI_FORMAT => rectangle.other_mode.texture_lut() == 0,
            _ => false,
        };
    if !rgba16 && !direct_8bit {
        return Err(render_unsupported_error(
            "reference",
            "render.rdp.copy-source-layout",
            format!(
                "{} copy source format={} size={} LUT={} is unsupported; expected RGBA16, I8, IA8, or non-dereferenced CI8",
                texture_rectangle_name(rectangle),
                texture.format,
                texture.size,
                rectangle.other_mode.texture_lut()
            ),
        ));
    }
    if let Some(target) = target {
        let matching_target = matches!(
            (rgba16, direct_8bit, target.layout()),
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
        CycleType::Fill => {
            if let Err(hazards) = rectangle.other_mode.validate_fill_cycle_bypass() {
                return Err(render_unsupported_error(
                    "reference",
                    "render.rdp.fill-cycle-hazard-state",
                    format!(
                        "G_FILLRECT in Fill cycle retains unsafe {hazards} state; the public fill contract requires G_RM_NOOP/G_RM_NOOP2, and retaining Z/framebuffer consumers is outside that safe contract (a depth read can hang the RDP)"
                    ),
                ));
            }
            return Ok(());
        }
        CycleType::Copy => {
            return Err(render_unsupported_error(
                "reference",
                "render.rdp.fill-rectangle-cycle",
                "G_FILLRECT in copy cycle has no guaranteed public result; use G_TEXRECT",
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
    hidden_bits: &mut RdramHiddenBits,
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
    hidden_bits: &mut RdramHiddenBits,
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
    hidden_bits: &mut RdramHiddenBits,
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
    let available_pixels = (rdram.len().saturating_sub(start.offset() as usize) / 2).min(px_count);
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    let pixel = |i: usize| {
        let src = i * 4;
        let r = fb.pixels[src];
        let g = fb.pixels[src + 1];
        let b = fb.pixels[src + 2];
        let stored_coverage = fb.coverage[i].stored();
        let px: u16 = (to_5(r) << 11)
            | (to_5(g) << 6)
            | (to_5(b) << 1)
            | u16::from((stored_coverage >> 2) & 1);
        (px, stored_coverage & 3)
    };
    let paired_pixels = available_pixels & !1;
    for i in (0..paired_pixels).step_by(2) {
        let byte_offset = u32::try_from(i * 2).expect("framebuffer byte offset exceeds u32");
        let dst = start
            .checked_add(byte_offset)
            .expect("bounded framebuffer pair address overflow");
        let (first, first_hidden) = pixel(i);
        let (second, second_hidden) = pixel(i + 1);
        let native_word = if cfg!(target_endian = "little") {
            (u32::from(first) << 16) | u32::from(second)
        } else {
            (u32::from(second) << 16) | u32::from(first)
        };
        view.write_u32(dst, native_word);
        hidden_bits.insert_pair(
            dst.offset(),
            RdramHiddenSample {
                visible: first,
                bits: first_hidden,
            },
            RdramHiddenSample {
                visible: second,
                bits: second_hidden,
            },
        );
    }
    if available_pixels != paired_pixels {
        let i = paired_pixels;
        let byte_offset = u32::try_from(i * 2).expect("framebuffer byte offset exceeds u32");
        let dst = start
            .checked_add(byte_offset)
            .expect("bounded framebuffer tail address overflow");
        let (visible, bits) = pixel(i);
        view.write_u16(dst, visible);
        write_rdram_hidden_bits(hidden_bits, dst.offset(), visible, bits);
    }
}

fn commit_color_image(
    rdram: &mut [u8],
    target: gbi::ColorImage,
    fb: &Framebuffer,
    hidden_bits: &mut RdramHiddenBits,
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
    hidden_bits: &mut RdramHiddenBits,
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
    hidden_bits: &mut RdramHiddenBits,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run_direct_8bit_copy(
        source_format: u8,
        width: u16,
        height: u16,
        source: &[u8],
        threshold: Option<u8>,
    ) -> Vec<u8> {
        const DL: usize = 0x100;
        const TEXTURE: u32 = 0x600;
        const TARGET: u32 = 0x800;
        let pixel_count = usize::from(width) * usize::from(height);
        assert_eq!(source.len(), pixel_count);
        let mut rdram = vec![0u8; 0x1000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, value) in source.iter().copied().enumerate() {
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
        let mut commands = Vec::new();
        let alpha_compare = u32::from(threshold.is_some());
        commands.push((0xef00_0000 | (2 << 20), alpha_compare));
        if let Some(threshold) = threshold {
            commands.push((0xf900_0000, u32::from(threshold)));
        }
        let width_field = u32::from(width - 1);
        let format_field = u32::from(source_format) << 21;
        let size_field = u32::from(gbi::ColorImage::BITS_8) << 19;
        commands.push((
            0xff00_0000 | (u32::from(gbi::ColorImage::I_FORMAT) << 21) | size_field | width_field,
            TARGET,
        ));
        commands.push((
            0xfd00_0000 | format_field | size_field | width_field,
            TEXTURE,
        ));
        let line_words = u32::from(width).div_ceil(8);
        let tile_word0 = 0xf500_0000 | format_field | size_field | (line_words << 9);
        commands.push((tile_word0, 7 << 24));
        let lrs = u32::from(width - 1) * 4;
        let lrt = u32::from(height - 1) * 4;
        commands.push((0xf400_0000, (7 << 24) | (lrs << 12) | lrt));
        commands.push((tile_word0, 0x0008_0200));
        commands.push((0xf200_0000, (lrs << 12) | lrt));
        commands.push((0xe400_0000 | (lrs << 12) | lrt, 0));
        commands.push((0, 0x1000_0400));
        commands.push((0xe900_0000, 0));
        commands.push((0xdf00_0000, 0));
        for (index, (word0, word1)) in commands.into_iter().enumerate() {
            let offset = DL + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
        }

        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend
            .create(&RenderConfig::ntsc(u32::from(width), u32::from(height)))
            .unwrap();
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
        (0..pixel_count)
            .map(|index| view.read_u8(fn64_runtime::RdramAddr::from_offset(TARGET + index as u32)))
            .collect()
    }

    fn present_resident(
        backend: &mut ReferenceBackend,
        vi: ViPresentation,
    ) -> Result<(), RenderError> {
        backend.present(PresentRequest::backend_resident(vi))
    }

    fn live_presentation(
        status: u32,
        origin: u32,
        width: u32,
        output_width: u32,
        output_height: u32,
    ) -> ViPresentation {
        let mut words = [0u32; fn64_render::ViScanoutRegisters::WORD_COUNT];
        words[0] = status;
        words[1] = origin;
        words[2] = width;
        words[9] = (100 << 16) | (100 + output_width);
        words[10] = (20 << 16) | (20 + output_height * 2);
        words[12] = u32::from(fn64_render::ViScaleAxis::ONE);
        words[13] = u32::from(fn64_render::ViScaleAxis::ONE);
        ViPresentation {
            scanout: fn64_render::ViScanoutState::Registers(
                fn64_render::ViScanoutRegisters::from_words(words),
            ),
            ..ViPresentation::default()
        }
    }

    fn present_physical(
        backend: &mut ReferenceBackend,
        rdram: &[u8],
        vi: ViPresentation,
    ) -> Result<(), RenderError> {
        backend.present(PresentRequest::live(
            vi,
            fn64_runtime::PhysicalRdramRead::from_storage(rdram),
        ))
    }

    #[test]
    fn native_programmed_span_excludes_reference_filter_halo() {
        let vi = live_presentation(0x002, 0x100, 4, 1, 1);
        let programmed = fn64_render::programmed_vi_source_footprint(vi)
            .unwrap()
            .unwrap();
        let reference = reference_vi_source_geometry(vi).unwrap().unwrap();
        assert_eq!(programmed.rows, 2);
        assert_eq!(reference.rows, 3);
        assert_eq!(programmed.origin, reference.origin);
        assert_eq!(programmed.stride_pixels, reference.stride_pixels);
    }

    #[test]
    fn reference_backend_create_then_present_succeeds_with_no_geometry() {
        let mut backend = ReferenceBackend::new();
        assert_eq!(
            backend.task_chunking(),
            fn64_render::RenderTaskChunking::Resumable
        );
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
        present_resident(&mut backend, ViPresentation::default()).unwrap();
        assert!(!backend
            .framebuffer()
            .unwrap()
            .has_non_uniform_content(0, 0, 0, 255));
    }

    #[test]
    fn reference_renderer_tv_authority_tracks_create_and_survives_resize() {
        let mut backend = ReferenceBackend::new();
        assert_eq!(
            backend.release_environment(),
            fn64_render::RenderBackendEvidence::Unidentified
        );

        backend
            .create(&RenderConfig::for_tv(8, 8, fn64_runtime::TvType::Pal))
            .unwrap();
        assert_eq!(
            backend.release_environment(),
            fn64_render::RenderBackendEvidence::Reference {
                tv_type: fn64_runtime::TvType::Pal,
            }
        );
        backend.resize(16, 12);
        assert_eq!(
            backend.release_environment().tv_type(),
            Some(fn64_runtime::TvType::Pal)
        );

        backend
            .create(&RenderConfig::for_tv(4, 4, fn64_runtime::TvType::Mpal))
            .unwrap();
        assert_eq!(
            backend.release_environment().tv_type(),
            Some(fn64_runtime::TvType::Mpal)
        );
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
            backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
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
        backend.create(&RenderConfig::ntsc(4, 4)).unwrap();
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
        backend.create(&RenderConfig::ntsc(2, 1)).unwrap();
        backend.fb.as_mut().unwrap().pixels[0..4].copy_from_slice(&[9, 8, 7, 255]);

        present_resident(&mut backend, ViPresentation::default()).unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[0..4],
            &[9, 8, 7, 255]
        );

        present_resident(
            &mut backend,
            ViPresentation {
                blanked: true,
                ..ViPresentation::default()
            },
        )
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

        present_resident(&mut backend, ViPresentation::default()).unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[0..4],
            &[9, 8, 7, 255]
        );
    }

    #[test]
    fn reference_backend_executes_public_fade_and_repeat_line_scanout() {
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::ntsc(2, 2)).unwrap();
        backend.fb.as_mut().unwrap().pixels.copy_from_slice(&[
            10, 20, 30, 255, 40, 50, 60, 255, 110, 120, 130, 255, 140, 150, 160, 255,
        ]);

        present_resident(
            &mut backend,
            ViPresentation {
                fade: Some(0x03ff),
                ..ViPresentation::default()
            },
        )
        .unwrap();
        assert_eq!(
            backend.presented_framebuffer().unwrap().pixels,
            [110, 120, 130, 255, 140, 150, 160, 255, 110, 120, 130, 255, 140, 150, 160, 255,]
        );

        present_resident(
            &mut backend,
            ViPresentation {
                repeat_line: true,
                ..ViPresentation::default()
            },
        )
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
        backend.create(&RenderConfig::ntsc(3, 3)).unwrap();
        let fb = backend.fb.as_mut().unwrap();
        for pixel in fb.pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[88, 88, 88, 255]);
        }
        fb.pixels[4 * 4..4 * 4 + 4].copy_from_slice(&[80, 80, 80, 255]);
        present_resident(
            &mut backend,
            ViPresentation {
                scanout: fn64_render::ViScanoutState::BackendOnly(rgba16),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[4 * 4..4 * 4 + 4],
            &[88, 88, 88, 255]
        );

        let fb = backend.fb.as_mut().unwrap();
        fb.pixels[0..12].copy_from_slice(&[10, 10, 10, 255, 200, 200, 200, 255, 20, 20, 20, 255]);
        fb.coverage[1] = raster::Coverage::new(4);
        present_resident(
            &mut backend,
            ViPresentation {
                scanout: fn64_render::ViScanoutState::BackendOnly(fn64_render::ViFilterControl {
                    pixel_type: ViPixelType::Rgba16,
                    divot: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[4..8],
            &[20, 20, 20, 255]
        );

        backend.fb.as_mut().unwrap().pixels[0..4].copy_from_slice(&[64, 0, 255, 255]);
        present_resident(
            &mut backend,
            ViPresentation {
                scanout: fn64_render::ViScanoutState::BackendOnly(fn64_render::ViFilterControl {
                    pixel_type: ViPixelType::Rgba32,
                    gamma: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[0..4],
            &[127, 0, 255, 255]
        );
    }

    #[test]
    fn reference_backend_gamma_dither_is_seeded_and_frame_varying() {
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::ntsc(1, 1)).unwrap();
        backend.fb.as_mut().unwrap().pixels[0..4].copy_from_slice(&[101, 101, 101, 255]);
        let presentation = |noise_seed| ViPresentation {
            scanout: fn64_render::ViScanoutState::BackendOnly(fn64_render::ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                gamma_dither: true,
                ..Default::default()
            }),
            noise_seed,
            ..Default::default()
        };
        present_resident(&mut backend, presentation(0)).unwrap();
        let first = backend.presented_framebuffer().unwrap().pixels[0..3].to_vec();
        present_resident(&mut backend, presentation(0)).unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[0..3],
            first
        );

        let variants = (0..64)
            .map(|seed| {
                present_resident(&mut backend, presentation(seed)).unwrap();
                backend.presented_framebuffer().unwrap().pixels[0]
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(variants, [100, 102].into_iter().collect());
    }

    #[test]
    fn reference_vi_reads_rgba16_from_live_origin_and_effective_padded_stride() {
        const ORIGIN: u32 = 0x120;
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in [
                0xf801u16, 0x07c1, 0xf83f, 0xffc1, 0x003f, 0xffff, 0x0001, 0x07ff,
            ]
            .into_iter()
            .enumerate()
            {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(ORIGIN + index as u32 * 2),
                    pixel,
                );
            }
        }
        assert_eq!(
            &rdram[ORIGIN as usize..ORIGIN as usize + 16],
            &[
                0xc1, 0x07, 0x01, 0xf8, 0xc1, 0xff, 0x3f, 0xf8, 0xff, 0xff, 0x3f, 0x00, 0xff, 0x07,
                0x01, 0x00
            ]
        );

        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
        backend.fb.as_mut().unwrap().clear(9, 8, 7, 255);
        let vi = live_presentation(0x302, ORIGIN, 0xf000_0004, 2, 2);
        present_physical(&mut backend, &rdram, vi).unwrap();
        assert_eq!(
            backend.presented_framebuffer().unwrap().pixels,
            [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,]
        );
        assert!(backend
            .framebuffer()
            .unwrap()
            .pixels
            .chunks_exact(4)
            .all(|pixel| pixel == [9, 8, 7, 255]));

        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u16(fn64_runtime::RdramAddr::from_offset(ORIGIN), 0x07ff);
        present_physical(&mut backend, &rdram, vi).unwrap();
        assert_eq!(
            &backend.presented_framebuffer().unwrap().pixels[..4],
            &[0, 255, 255, 255],
            "a repeated field retained stale task-time or prior-present bytes"
        );
    }

    #[test]
    fn reference_vi_reads_unaligned_rgba32_rows_from_odd_live_stride() {
        const ORIGIN: u32 = 0x181;
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let logical = [
            0x10, 0x20, 0x30, 0xe4, 0x40, 0x50, 0x60, 0x63, 0xd1, 0xd2, 0xd3, 0xff, 0x70, 0x80,
            0x90, 0xa2, 0xa0, 0xb0, 0xc0, 0xff, 0xe1, 0xe2, 0xe3, 0x00,
        ];
        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_logical_bytes(fn64_runtime::RdramAddr::from_offset(ORIGIN), &logical);

        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::ntsc(3, 2)).unwrap();
        present_physical(
            &mut backend,
            &rdram,
            live_presentation(0x303, ORIGIN, 3, 2, 2),
        )
        .unwrap();
        assert_eq!(
            backend.presented_framebuffer().unwrap().pixels,
            [
                0x10, 0x20, 0x30, 33, 0x40, 0x50, 0x60, 24, 0x70, 0x80, 0x90, 16, 0xa0, 0xb0, 0xc0,
                255,
            ]
        );
    }

    #[test]
    fn reference_vi_uses_each_field_exact_live_origin_without_extra_bias() {
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(0x220), 0xf801);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(0x222), 0x07c1);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(0x280), 0x003f);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(0x282), 0xffff);
        }

        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::ntsc(2, 1)).unwrap();
        let odd = live_presentation(0x342, 0x280, 2, 2, 1);
        let mut odd_words = odd.scanout.registers().unwrap().words();
        odd_words[4] = 1;
        let odd = ViPresentation {
            scanout: fn64_render::ViScanoutState::Registers(
                fn64_render::ViScanoutRegisters::from_words(odd_words),
            ),
            ..odd
        };
        present_physical(&mut backend, &rdram, odd).unwrap();
        assert_eq!(
            backend.presented_framebuffer().unwrap().pixels,
            [0, 0, 255, 255, 255, 255, 255, 255]
        );
        present_physical(
            &mut backend,
            &rdram,
            live_presentation(0x342, 0x220, 2, 2, 1),
        )
        .unwrap();
        assert_eq!(
            backend.presented_framebuffer().unwrap().pixels,
            [255, 0, 0, 255, 0, 255, 0, 255]
        );
    }

    #[test]
    fn reference_vi_bounds_fail_transactionally_and_exact_edge_succeeds() {
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::ntsc(2, 2)).unwrap();
        present_resident(&mut backend, ViPresentation::default()).unwrap();
        let before = backend.presented_framebuffer().unwrap().clone();

        let error = present_physical(
            &mut backend,
            &rdram,
            live_presentation(0x302, 0x7f_fff8, 4, 2, 2),
        )
        .unwrap_err();
        assert!(matches!(error, RenderError::InvalidViSourceBounds { .. }));
        assert_eq!(
            backend.presented_framebuffer().unwrap().pixels,
            before.pixels
        );

        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(0x7f_fff8),
            &[1, 2, 3, 0xff, 4, 5, 6, 0xff],
        );
        present_physical(
            &mut backend,
            &rdram,
            live_presentation(0x303, 0x7f_fff8, 2, 2, 1),
        )
        .unwrap();
        let one_row = backend.presented_framebuffer().unwrap().pixels.clone();
        let error = present_physical(
            &mut backend,
            &rdram,
            live_presentation(0x303, 0x7f_fff8, 2, 2, 2),
        )
        .unwrap_err();
        assert!(matches!(error, RenderError::InvalidViSourceBounds { .. }));
        assert_eq!(backend.presented_framebuffer().unwrap().pixels, one_row);
    }

    #[test]
    fn reference_vi_blank_and_inactive_paths_do_not_fetch_live_source() {
        let rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::ntsc(2, 2)).unwrap();

        let mut inactive_words = [0u32; fn64_render::ViScanoutRegisters::WORD_COUNT];
        inactive_words[0] = 0x302;
        inactive_words[1] = 0x00ff_ffff;
        let inactive = ViPresentation {
            scanout: fn64_render::ViScanoutState::Registers(
                fn64_render::ViScanoutRegisters::from_words(inactive_words),
            ),
            ..Default::default()
        };
        present_physical(&mut backend, &rdram, inactive).unwrap();
        assert_eq!(backend.presented_framebuffer().unwrap().width, 0);
        assert_eq!(backend.presented_framebuffer().unwrap().height, 0);

        let blanked = ViPresentation {
            blanked: true,
            ..live_presentation(0x302, 0x00ff_ffff, 2, 2, 2)
        };
        present_physical(&mut backend, &rdram, blanked).unwrap();
        assert!(backend
            .presented_framebuffer()
            .unwrap()
            .pixels
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 0, 255]));

        let status_blank = live_presentation(0x300, 0x00ff_ffff, 2, 2, 2);
        present_physical(&mut backend, &rdram, status_blank).unwrap();
        let reserved = ViPresentation {
            blanked: true,
            ..live_presentation(0x301, 0x00ff_ffff, 2, 2, 2)
        };
        let error = present_physical(&mut backend, &rdram, reserved).unwrap_err();
        assert!(error.to_string().contains("reserved pixel type"));

        let misaligned = live_presentation(0x302, 0x121, 2, 2, 1);
        assert!(matches!(
            present_physical(&mut backend, &rdram, misaligned).unwrap_err(),
            RenderError::InvalidViSourceAlignment { .. }
        ));
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
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
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
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
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
    fn reference_backend_identifies_only_exact_admitted_imem_images() {
        let geometry = [0x71; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let sprite = [0x72; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let unadmitted = [0x73; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let backend = ReferenceBackend::new()
            .with_geometry_ucode_text(GeometryWireFamily::L3dex2, &geometry)
            .with_s2dex_ucode_text_for(S2dexWireFamily::S2dex, &sprite);

        assert_eq!(backend.identify_microcode(&geometry), Some(UcodeId::L3dex2));
        assert_eq!(backend.identify_microcode(&sprite), Some(UcodeId::S2dex));
        assert_eq!(backend.identify_microcode(&unadmitted), None);
        assert_eq!(backend.supported_ucodes(), &[UcodeId::L3dex2]);
    }

    #[test]
    fn reference_pair_recognition_is_separate_from_text_hle_admission() {
        let text = [0x71; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let data = [0x10, 0x20, 0x30];
        let identity = MicrocodeDataImageIdentity {
            bytes: data.len() as u32,
            sha256: sha2::Sha256::digest(data).into(),
        };
        let text_only =
            ReferenceBackend::new().with_geometry_ucode_text(GeometryWireFamily::L3dex2, &text);
        assert_eq!(text_only.identify_microcode_pair(&text, identity), None);
        let paired = text_only.with_microcode_pair(UcodeId::L3dex2, &text, &data);
        assert_eq!(
            paired.identify_microcode_pair(&text, identity),
            Some(UcodeId::L3dex2)
        );
    }

    #[test]
    fn reference_backend_requires_exact_task_entry_admission() {
        const DL: usize = 0x100;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
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

    fn raw_submission(
        source: fn64_render::RawDpcSource,
        start: u32,
        opcode: u8,
    ) -> fn64_render::OwnedRawDpcSubmission {
        let words = vec![u32::from(opcode) << 24, 0];
        match source {
            fn64_render::RawDpcSource::Rdram => {
                fn64_render::OwnedRawDpcSubmission::from_rdram_words(start, start + 8, words)
                    .unwrap()
            }
            fn64_render::RawDpcSource::XbusDmem => {
                fn64_render::OwnedRawDpcSubmission::from_xbus_payload(
                    start,
                    start + 8,
                    words.into_iter().flat_map(u32::to_be_bytes).collect(),
                )
                .unwrap()
            }
        }
    }

    #[test]
    fn reference_raw_dpc_batch_commits_mixed_sources_in_one_boundary() {
        let submissions = vec![
            raw_submission(fn64_render::RawDpcSource::Rdram, 0x100, 0xe6),
            raw_submission(fn64_render::RawDpcSource::XbusDmem, 0x20, 0xe9),
        ];
        let identities = submissions
            .iter()
            .map(fn64_render::OwnedRawDpcSubmission::identity)
            .collect::<Vec<_>>();
        let mut rdram = vec![0x5a; 0x400];
        let before = rdram.clone();
        let batch = fn64_render::RawDpcBatch::new(submissions)
            .unwrap()
            .preflight(rdram.len())
            .unwrap();
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::ntsc(2, 2)).unwrap();

        let outcome = backend.process_raw_dpc_batch(&mut rdram, batch, 0).unwrap();

        assert_eq!(
            backend.raw_dpc_batch_capability(),
            fn64_render::RawDpcBatchCapability::DiagnosticOnly
        );
        assert_eq!(outcome.identities.as_ref(), identities);
        assert_eq!(outcome.full_sync, fn64_render::DpFullSyncStatus::Reached);
        assert_eq!(outcome.stream_groups.len(), 2);
        assert_eq!(backend.last_dp_full_sync(), outcome.full_sync);
        assert_eq!(rdram, before, "private command staging leaked into RDRAM");
    }

    #[test]
    fn reference_raw_dpc_batch_not_ready_rejects_without_mutation() {
        let mut rdram = vec![0x5a; 0x400];
        let before = rdram.clone();
        let batch = fn64_render::RawDpcBatch::new(vec![raw_submission(
            fn64_render::RawDpcSource::Rdram,
            0x100,
            0xe9,
        )])
        .unwrap()
        .preflight(rdram.len())
        .unwrap();
        let mut backend = ReferenceBackend::new();

        let error = backend
            .process_raw_dpc_batch(&mut rdram, batch, 0)
            .unwrap_err();

        assert!(matches!(error, RenderError::NotReady(_)));
        assert_eq!(rdram, before);
        assert_eq!(
            backend.last_dp_full_sync(),
            fn64_render::DpFullSyncStatus::Unidentified
        );
        assert!(backend.framebuffer().is_none());
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
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
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
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();

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
                Some(RdramHiddenSample {
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
            command(0x0800_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();

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
            command(0x0900_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(2 << 16, 0); // near plane is Z=0
            command(0, 0);
            command(0xfa00_0000, 0xff00_00ff); // opaque red primitive
            command(0x0900_0000 | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            command(4 << 16, 0); // submitted later, but farther
            command(0, 0);
            command(0xe900_0000, 0);
        }
        let end = offset;
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();

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
        let triangle = 0x0900_0000 | yl;
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
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
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
        command(0x0800_0000 | yl, (ym << 16) | yh); // no Z coefficient words
        command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
        command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
        command(1 << 16, 0);
        command(0xe900_0000, 0);
        let end = offset;

        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
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
        let triangle = 0x0800_0000 | yl;
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
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
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
            command(0x0c00_0000 | yl, (ym << 16) | yh);
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
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();

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
            left_major: false,
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
            command(0x0f00_0000 | yl, (ym << 16) | yh);
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
            command(0, 1024 << 16); // S=0, T=0, perspective unity W
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
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();

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
            command(
                0xef00_0000 | (1 << 20) | (1 << 19) | (1 << 16) | (6 << 9) | 0xf0,
                0,
            );
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
            command(0x0a00_0000 | (2 << 19) | yl, (ym << 16) | yh);
            command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
            command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
            command(1 << 16, 0);
            // S=T=0, perspective-unity W; dS/dX=dT/dY=2.5. Chapter 13.7 selects
            // tiles 1 and 2 with LOD fraction 0.25.
            command(0, 1024 << 16);
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
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
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
        backend.create(&RenderConfig::ntsc(2, 1)).unwrap();

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
        backend.create(&RenderConfig::ntsc(2, 1)).unwrap();

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
        assert!(!dump.limit_reported);
    }

    #[test]
    fn framebuffer_writer_and_runtime_view_agree_on_logical_pixel_order() {
        let mut framebuffer = Framebuffer::new(2, 1);
        framebuffer.pixels[0..4].copy_from_slice(&[255, 0, 0, 255]);
        framebuffer.pixels[4..8].copy_from_slice(&[0, 0, 255, 255]);
        let mut storage = [0u8; 4];
        let mut hidden_bits = RdramHiddenBits::new();

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
        let mut hidden_bits = RdramHiddenBits::new();

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
        let mut hidden_bits = RdramHiddenBits::new();

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
        let mut hidden_bits = RdramHiddenBits::from([(
            0,
            RdramHiddenSample {
                visible: 1,
                bits: 1,
            },
        )]);
        assert_eq!(read_rdram_hidden_bits(&mut hidden_bits, 0, 0), 0);
        assert_eq!(
            hidden_bits.get(&0),
            Some(RdramHiddenSample {
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
        backend.rdram_hidden_bits = RdramHiddenBits::from([
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
        assert_eq!(backend.rdram_hidden_bits.get(&0).unwrap().bits, 0);
        assert_eq!(backend.rdram_hidden_bits.get(&2).unwrap().bits, 3);
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
        let mut hidden_bits = RdramHiddenBits::from([
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
            hidden_bits.get(&0).unwrap(),
            RdramHiddenSample {
                visible: 0x1234,
                bits: 2
            }
        );
        assert_eq!(
            hidden_bits.get(&2).unwrap(),
            RdramHiddenSample {
                visible: 0x5679,
                bits: 1
            }
        );
        assert_eq!(hidden_bits.get(&4), Some(untouched));
        let mut imported = Framebuffer::new(2, 1);
        load_color_image(&rdram, rgba16, &mut imported, &mut hidden_bits);
        assert_eq!(imported.coverage[0].stored(), 2);
        assert_eq!(imported.coverage[1].stored(), 5);
        assert_eq!(hidden_bits.get(&4), Some(untouched));
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
        let mut hidden_bits = RdramHiddenBits::from([
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
            hidden_bits.get(&0).unwrap(),
            RdramHiddenSample {
                visible: 0x1020,
                bits: 2
            }
        );
        assert_eq!(
            hidden_bits.get(&2).unwrap(),
            RdramHiddenSample {
                visible: 0x3001,
                bits: 1
            }
        );
        assert_eq!(
            hidden_bits.get(&4).unwrap(),
            RdramHiddenSample {
                visible: 0x4051,
                bits: 3
            }
        );
        assert_eq!(
            hidden_bits.get(&6).unwrap(),
            RdramHiddenSample {
                visible: 0x6000,
                bits: 0
            }
        );
        assert_eq!(hidden_bits.get(&8), Some(untouched));
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
        assert_eq!(hidden_bits.get(&8), Some(untouched));
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
                let mut hidden_bits = RdramHiddenBits::new();
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
                RdramHiddenBits::from([(0, sentinel), (2, sentinel), (4, sentinel), (6, sentinel)]);
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
                    hidden_bits.get(&address),
                    Some(expected_hidden),
                    "{layout:?} at {address}"
                );
            }
        }
    }

    #[test]
    fn fill_cycle_rejects_every_unsafe_bypass_state_before_mutation() {
        let rectangle = |low| gbi::FillRectangle {
            ulx: 0.0,
            uly: 0.0,
            lrx: 1.0,
            lry: 0.0,
            fill_color: 0xffff_ffff,
            cycle_type: gbi::CycleType::Fill,
            scissor: None,
            other_mode: gbi::OtherMode::from_raw(3 << 20, low, 0),
            combiner: gbi::CombinerState::default(),
            blender: gbi::BlenderState::default(),
        };

        assert!(validate_fill_rectangle(&rectangle(0)).is_ok());
        for hazards in 1u32..8 {
            let low = ((hazards & 1) << 4) | ((hazards & 2) << 4) | ((hazards & 4) << 4);
            let error = validate_fill_rectangle(&rectangle(low))
                .expect_err("every nonempty Fill-cycle hazard set must fail closed");
            let message = error.to_string();
            assert!(message.contains("unsafe"));
            assert!(message.contains("G_RM_NOOP/G_RM_NOOP2"));
        }

        const START: usize = 0x100;
        const TARGET: u32 = 0x400;
        let commands: [(u32, u32); 5] = [
            (0xff10_0001, TARGET),
            // Fill cycle with IM_RD retained. This was the silent old path:
            // it wrote the target even though the public fill contract
            // requires the bypass-safe NOOP render mode.
            (0xef00_0000 | (3 << 20), 1 << 6),
            (0xf700_0000, 0xffff_ffff),
            (0xf600_0000 | (4 << 12), 0),
            (0xe900_0000, 0),
        ];
        let mut rdram = vec![0xa5; 0x800];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = START + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        }
        let before = rdram.clone();
        let mut backend = ReferenceBackend::new();
        backend.create(&RenderConfig::ntsc(2, 1)).unwrap();
        let error = backend
            .process_rdp_commands(
                &mut rdram,
                START as u32,
                (START + commands.len() * 8) as u32,
                0,
            )
            .expect_err("unsafe Fill-cycle IM_RD must reject before target writeback");
        assert!(error.to_string().contains("unsafe IM_RD state"));
        assert_eq!(rdram, before);
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
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
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
        backend.create(&RenderConfig::ntsc(1, 1)).unwrap();

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
        backend.create(&RenderConfig::ntsc(1, 1)).unwrap();

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
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
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
        backend.create(&RenderConfig::ntsc(2, 1)).unwrap();
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
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
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
        let mut hidden_bits = RdramHiddenBits::new();
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
        let mut hidden_bits = RdramHiddenBits::new();
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
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
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
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
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
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
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
        backend.create(&RenderConfig::ntsc(4, 1)).unwrap();
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
            backend.create(&RenderConfig::ntsc(8, 1)).unwrap();
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
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
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

        for source_format in [gbi::ColorImage::I_FORMAT, gbi::ColorImage::IA_FORMAT] {
            for destination in gbi::ColorImageLayout::ALL {
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
                        format: source_format,
                        size: gbi::ColorImage::BITS_8,
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
                assert_eq!(
                    validate_copy_texture_rectangle(&rectangle, Some(target(destination))).is_ok(),
                    destination == gbi::ColorImageLayout::Index8,
                    "format {source_format} -> {destination:?}"
                );
            }
        }
    }

    #[test]
    fn copy_source_gate_rejects_ci8_tlut_and_undefined_eight_bit_formats() {
        let target = gbi::ColorImage {
            format: gbi::ColorImage::I_FORMAT,
            size: gbi::ColorImage::BITS_8,
            width: 1,
            address: 0,
        };
        let mut rectangle = gbi::TextureRectangle {
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
            other_mode: gbi::OtherMode::from_raw((2 << 20) | (2 << 14), 0, 0),
            combiner: gbi::CombinerState::default(),
            blender: gbi::BlenderState::default(),
            scissor: None,
            texture: Some(gbi::Texture {
                format: gbi::ColorImage::CI_FORMAT,
                size: gbi::ColorImage::BITS_8,
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
        assert!(validate_copy_texture_rectangle(&rectangle, Some(target)).is_err());

        rectangle.other_mode = gbi::OtherMode::from_raw(2 << 20, 0, 0);
        rectangle.texture.as_mut().unwrap().format = gbi::ColorImage::RGBA_FORMAT;
        assert!(validate_copy_texture_rectangle(&rectangle, Some(target)).is_err());
        rectangle.texture.as_mut().unwrap().format = 1;
        assert!(validate_copy_texture_rectangle(&rectangle, Some(target)).is_err());
    }

    #[test]
    fn copy_ci8_indices_directly_to_eight_bit_color_image() {
        assert_eq!(
            run_direct_8bit_copy(
                gbi::ColorImage::CI_FORMAT,
                4,
                1,
                &[0, 1, 0x7f, 0xff],
                Some(1),
            ),
            [0xaa, 1, 0x7f, 0xff]
        );
    }

    #[test]
    fn copy_i8_preserves_source_bytes_and_uses_intensity_as_alpha() {
        assert_eq!(
            run_direct_8bit_copy(
                gbi::ColorImage::I_FORMAT,
                8,
                1,
                &[0, 0x7f, 0x80, 0xff, 0x20, 0x81, 0x01, 0xfe],
                Some(0x80),
            ),
            [0xaa, 0xaa, 0x80, 0xff, 0xaa, 0x81, 0xaa, 0xfe]
        );
    }

    #[test]
    fn copy_ia8_preserves_packed_bytes_and_compares_expanded_alpha_nibble() {
        assert_eq!(
            run_direct_8bit_copy(
                gbi::ColorImage::IA_FORMAT,
                8,
                1,
                &[0x10, 0x17, 0x28, 0x4f, 0xf8, 0xe9, 0xa0, 0xbf],
                Some(0x88),
            ),
            [0xaa, 0xaa, 0x28, 0x4f, 0xf8, 0xe9, 0xaa, 0xbf]
        );
    }

    #[test]
    fn copy_ia8_preserves_odd_tmem_row_layout_without_alpha_compare() {
        let source = [
            0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87, 0x98, 0xa9, 0xba, 0xcb, 0xdc, 0xed,
            0xfe, 0x0f,
        ];
        assert_eq!(
            run_direct_8bit_copy(gbi::ColorImage::IA_FORMAT, 8, 2, &source, None),
            source
        );
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
        backend.create(&RenderConfig::ntsc(2, 2)).unwrap();
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
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
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
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
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
        direct.create(&RenderConfig::ntsc(4, 2)).unwrap();
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
        backend.create(&RenderConfig::ntsc(1, 1)).unwrap();
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
        backend.create(&RenderConfig::ntsc(2, 2)).unwrap();
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
        backend.create(&RenderConfig::ntsc(2, 2)).unwrap();
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
}
