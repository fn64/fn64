//! `fn64-render`: the graphics backend seam, per `docs/DECOUPLING.md`
//! ("Renderer seam -- `fn64-render` (RT64 today, ours later)").
//!
//! ## What this crate is
//!
//! `RenderBackend` is the ONE boundary the runtime uses to hand off N64 gfx
//! work: a captured RSP task header (`OsTask`, the public libultra manual's
//! `OSTask_t` field shape -- same fields `fn64_runtime::rsp::OsTaskHeader`
//! already models, mirrored here so the seam does not expose executor
//! internals, per `docs/DECOUPLING.md`'s "the backend never reaches back into
//! runtime state") plus the raw rdram byte buffer the
//! display list and its vertex/texture data live in. Lifecycle
//! (`create`/`resize`/`present`) and a `supported_ucodes` self-report round
//! out the backend trait; the crate also owns the shared admission and
//! completion mechanisms below.
//!
//! **No backend implementation lives here.** This crate owns the mechanisms
//! every backend must share: exact content-addressed microcode admission, an
//! immutable ordered task/self-load admission plan, and public raw-RDP
//! command-width inspection for typed FullSync completion.
//! `fn64-render-rt64` is the first adapter (RT64 FFI, quarantined per
//! `docs/DESIGN.md` section 1's C++ boundary rule) and temporarily also houses
//! the headless reference software rasterizer used as its deterministic CI
//! oracle.
//!
//! ## Why `OsTask` is redefined here instead of reusing `fn64_runtime::rsp::OsTaskHeader`
//!
//! The runtime submits into this trait; the trait never receives or calls an
//! executor. It uses stable runtime-owned device value types and logical RDRAM
//! views, but deliberately redeclares the task handoff so backend APIs cannot
//! acquire scheduler state. A `From<OsTaskHeader> for OsTask` conversion is
//! executor-seam glue rather than backend policy.
#![forbid(unsafe_code)]

mod geometry_task_inspection;
mod microcode;
mod microcode_identity;
mod raw_dpc_batch;
mod rdp_completion;
mod render_ir;
mod settings;
pub mod vi_public_filters;
mod vi_source;

use std::{
    fmt,
    num::{NonZeroU32, NonZeroU64},
};

pub use fn64_render_ir as ir;
pub use geometry_task_inspection::{
    inspect_geometry_task, GeometryTaskInspection, GeometryTaskInspectionPolicy,
    TaskAdmissionRawWindow, TaskAdmissionRawWindowSize,
};
pub use microcode::{
    F3dex2UcodeCatalog, GeometryUcodeCatalog, GeometryUcodeProfile, GeometryWireFamily,
    MicrocodePairCatalog, S2dexUcodeCatalog, S2dexWireFamily, TaskAdmissionGeneration,
    TaskAdmissionPlan, TaskAdmissionSource, TaskAdmissionUcode, UcodeDigest,
};
pub use microcode_identity::{
    capture_task_admission_raw_window, identify_f3dzex2, F3dzex2Variant, F3DZEX2_IDENTITY_SOURCE,
    F3DZEX2_RAW_WINDOW_SIZE,
};
pub use raw_dpc_batch::{
    OwnedRawDpcSubmission, PreflightedRawDpcBatch, RawDpcBatch, RawDpcBatchCapability,
    RawDpcBatchOutcome, RawDpcBatchPreflightError, RawDpcSource, RawDpcStreamGroup,
    RawDpcSubmissionError, RawDpcSubmissionIdentity,
};
pub use rdp_completion::{inspect_raw_rdp_full_sync, raw_rdp_command_width};
pub use render_ir::{
    decode_raw_dpc_capture, ir_effect_content_digest, preflight_raw_dpc_capture,
    CommittedSemanticWorkloadRecord, IrGuestMemoryPreimage, IrGuestMemorySnapshot,
    IrRawDpcBackendCompletion, IrRawDpcPacketPreflight, StagedIrRdramWrite,
};
pub use settings::{
    AspectTarget, DownsampleMultiplier, RefreshRateTarget, RenderAntialiasing, RenderAspectRatio,
    RenderDisplayBuffering, RenderEmulatorSettings, RenderEnhancementSettings, RenderFiltering,
    RenderGraphicsApi, RenderHardwareResolve, RenderInternalColorFormat, RenderPolicyApply,
    RenderPresentationMode, RenderRefreshRate, RenderReplacementAutoPath,
    RenderReplacementOperation, RenderReplacementPackIdentity, RenderReplacementSettings,
    RenderReplacementShift, RenderResolution, RenderRestartField, RenderRuntimePolicy,
    RenderRuntimeSettings, RenderSettingsApply, RenderSettingsError, RenderUpscale2d,
    ResolutionMultiplier,
};
pub use vi_source::{programmed_vi_source_footprint, ViSourceFootprint};

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
    /// Original 16-entry Fast3D polygon microcode using the base GBI wire
    /// layout.
    Fast3d,
    /// Extended 32-entry Fast3DEX polygon microcode using the F3DEX_GBI wire
    /// layout.
    F3dex,
    /// F3DLX polygon microcode. Its wire layout is F3DEX-compatible, while
    /// its pixel-precision and clipping policy require a distinct identity.
    F3dlx,
    /// Legacy F3DLX.Rej polygon microcode with a 64-entry cache and reject-box
    /// processing in place of clipping.
    F3dlxRej,
    /// Fast3DEX2 family (the common late-era SDK gfx ucode; both No Mercy
    /// and Ocarina of Time's era used an F3DEX2-family microcode per public
    /// SDK documentation).
    F3dex2,
    /// Public F3DEX2.NoN variant: the F3DEX_GBI_2 wire with near-plane
    /// clipping disabled.
    F3dex2NoN,
    /// Public F3DEX2.Rej variant: subpixel transforms, 64 vertices, and
    /// reject-box processing instead of clipping.
    F3dex2Rej,
    /// Public F3DLX2.Rej variant: 64 vertices and reject-box processing,
    /// without subpixel vertex calculations.
    F3dlx2Rej,
    /// Known game-era F3DZEX2 identity. Pinned MIT RT64 supplies software
    /// parity identity and BranchW behavior, but public Nintendo materials do
    /// not specify the family-specific envelope; HLE remains unavailable.
    F3dzex2,
    /// Original S2DEX family using the F3DEX_GBI command layout.
    S2dex,
    /// S2DEX2 family using the F3DEX_GBI_2 command layout.
    S2dex2,
    /// Original L3DEX line family using the F3DEX_GBI command layout.
    L3dex,
    /// L3DEX2 line family using the F3DEX_GBI_2 command layout.
    L3dex2,
    /// Catch-all for a named-but-not-yet-modeled ucode family, so a backend
    /// can advertise partial/experimental support without this enum
    /// growing a variant per guess. `0` is never a real value produced by
    /// this crate's own code; it exists for a future adapter to construct.
    Other(u32),
}

/// Exact identity of the original microcode-data image paired with one live
/// IMEM image. The ABI owns the logical RDRAM read and supplies this typed
/// value; a renderer must explicitly admit the complete text/data pair before
/// returning a family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MicrocodeDataImageIdentity {
    pub bytes: u32,
    pub sha256: [u8; 32],
}

/// Backend configuration for `RenderBackend::create`. Deliberately minimal
/// (window/output size and IPL-selected TV standard) -- a real windowing
/// surface handle is backend-specific (RT64 wants a native window handle; a
/// headless backend wants none), so this trait models only what every backend
/// needs to agree on:
/// the target framebuffer dimensions and television standard. Backend-specific
/// extras (a raw window handle, a device preference) are the adapter crate's
/// own config type, passed alongside this one at the adapter's own construction,
/// not through this shared trait -- keeping `RenderConfig` itself
/// backend-agnostic.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RenderConfig {
    pub width: u32,
    pub height: u32,
    pub tv_type: fn64_runtime::TvType,
}

/// VI-manager state that affects scanout at a presentation boundary rather
/// than RDP rendering. Keeping this separate from [`RenderConfig`] matters:
/// `osViBlack` changes at V-blank and does not recreate the output surface or
/// destroy the RDP's most recently rendered image.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ViPixelType {
    /// No hardware STATUS value was supplied (for backend-only callers).
    #[default]
    Unspecified,
    Blank,
    Reserved,
    Rgba16,
    Rgba32,
}

/// Public VI STATUS anti-alias/resample selector (bits 8..=9).
///
/// `Unspecified` preserves the backend-only presentation seam for callers
/// that do not supply live VI registers. Every register-derived value is one
/// of the four hardware modes below.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ViAaMode {
    #[default]
    Unspecified,
    AaResampleAlways,
    AaResampleWhenNeeded,
    ResampleOnly,
    Replicate,
}

impl ViAaMode {
    pub const fn from_status(status: u32) -> Self {
        match (status >> 8) & 3 {
            0 => Self::AaResampleAlways,
            1 => Self::AaResampleWhenNeeded,
            2 => Self::ResampleOnly,
            3 => Self::Replicate,
            _ => unreachable!(),
        }
    }

    pub const fn status_bits(self) -> Option<u32> {
        match self {
            Self::Unspecified => None,
            Self::AaResampleAlways => Some(0),
            Self::AaResampleWhenNeeded => Some(1 << 8),
            Self::ResampleOnly => Some(2 << 8),
            Self::Replicate => Some(3 << 8),
        }
    }

    pub const fn silhouette_aa_enabled(self) -> bool {
        matches!(self, Self::AaResampleAlways | Self::AaResampleWhenNeeded)
    }

    pub const fn resampling_enabled(self) -> bool {
        !matches!(self, Self::Replicate)
    }
}

/// Scanout filters selected by the latched VI STATUS register.
///
/// These are presentation properties, not RDP render state. Keeping the
/// decoded register value in this shared type means every backend observes
/// the exact feature set that became live at the same V-blank as a buffer
/// swap or VI-manager transition.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ViFilterControl {
    pub pixel_type: ViPixelType,
    pub antialias_mode: ViAaMode,
    pub gamma: bool,
    pub gamma_dither: bool,
    pub divot: bool,
    pub dither_filter: bool,
}

impl ViFilterControl {
    pub fn from_status(status: u32) -> Self {
        let pixel_type = match status & 3 {
            0 => ViPixelType::Blank,
            1 => ViPixelType::Reserved,
            2 => ViPixelType::Rgba16,
            3 => ViPixelType::Rgba32,
            _ => unreachable!("VI pixel type is two bits"),
        };
        Self {
            pixel_type,
            antialias_mode: ViAaMode::from_status(status),
            gamma: status & (1 << 3) != 0,
            gamma_dither: status & (1 << 2) != 0,
            divot: status & (1 << 4) != 0,
            dither_filter: status & (1 << 16) != 0,
        }
    }
}

/// One public VI X/Y scale-register axis.
///
/// US 6,166,748 Figures 35M/35N split each register into a twelve-bit scale
/// field and a twelve-bit subpixel-offset field. Public VI programming uses
/// ten fractional bits. Keeping the encoded fields typed prevents scanout
/// code from confusing host pixels with register subpixels.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViScaleAxis {
    step_u2_10: u16,
    offset_u2_10: u16,
}

impl ViScaleAxis {
    pub const FRACTION_BITS: u32 = 10;
    pub const ONE: u16 = 1 << Self::FRACTION_BITS;
    const FIELD_MASK: u32 = 0x0fff;

    pub fn from_register(register: u32) -> Self {
        Self {
            step_u2_10: (register & Self::FIELD_MASK) as u16,
            offset_u2_10: ((register >> 16) & Self::FIELD_MASK) as u16,
        }
    }

    pub const fn step_u2_10(self) -> u16 {
        self.step_u2_10
    }

    pub const fn offset_u2_10(self) -> u16 {
        self.offset_u2_10
    }
}

/// Field provenance attached to one latched VI scanout image.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViScanoutField {
    Progressive,
    InterlacedEven,
    InterlacedOdd,
}

impl ViScanoutField {
    pub const fn interlaced(self) -> bool {
        !matches!(self, Self::Progressive)
    }
}

/// Register-derived active digital output rectangle for one VI field.
///
/// The public VI register interface encodes horizontal start/end in pixels
/// and vertical start/end in half-lines. Keeping the programmed coordinates
/// as well as their derived extent prevents a backend from substituting its
/// host window size for the guest's latched scanout geometry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViActiveWindow {
    horizontal_start: u16,
    horizontal_end: u16,
    vertical_start_half_line: u16,
    vertical_end_half_line: u16,
}

impl ViActiveWindow {
    const FIELD_MASK: u32 = 0x03ff;

    /// Decode a programmed active window, or report that either the H or V
    /// interval has not been programmed yet. Register initialization is not
    /// atomic: software may enable VI after filling V_START while H_START is
    /// still zero, which remains an inactive image until both intervals exist.
    /// Nonzero malformed intervals still trap through [`Self::from_registers`].
    pub fn try_from_registers(horizontal: u32, vertical: u32) -> Option<Self> {
        let used = Self::FIELD_MASK | (Self::FIELD_MASK << 16);
        if horizontal & used == 0 || vertical & used == 0 {
            None
        } else {
            Some(Self::from_registers(horizontal, vertical))
        }
    }

    pub fn from_registers(horizontal: u32, vertical: u32) -> Self {
        let horizontal_start = ((horizontal >> 16) & Self::FIELD_MASK) as u16;
        let horizontal_end = (horizontal & Self::FIELD_MASK) as u16;
        let vertical_start_half_line = ((vertical >> 16) & Self::FIELD_MASK) as u16;
        let vertical_end_half_line = (vertical & Self::FIELD_MASK) as u16;
        assert!(
            horizontal_end > horizontal_start,
            "VI H_START has an empty or reversed active window {horizontal_start}..{horizontal_end}"
        );
        assert!(
            vertical_end_half_line > vertical_start_half_line,
            "VI V_START has an empty or reversed active window {vertical_start_half_line}..{vertical_end_half_line}"
        );
        let vertical_half_lines = vertical_end_half_line - vertical_start_half_line;
        assert_eq!(
            vertical_half_lines & 1,
            0,
            "VI V_START active extent {vertical_half_lines} is not a whole output line"
        );
        Self {
            horizontal_start,
            horizontal_end,
            vertical_start_half_line,
            vertical_end_half_line,
        }
    }

    pub const fn horizontal_register(self) -> u32 {
        ((self.horizontal_start as u32) << 16) | self.horizontal_end as u32
    }

    pub const fn vertical_register(self) -> u32 {
        ((self.vertical_start_half_line as u32) << 16) | self.vertical_end_half_line as u32
    }

    pub const fn output_width(self) -> u32 {
        (self.horizontal_end - self.horizontal_start) as u32
    }

    pub const fn output_height(self) -> u32 {
        ((self.vertical_end_half_line - self.vertical_start_half_line) / 2) as u32
    }
}

/// Register-derived digital resampling state latched for one presentation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViResampleControl {
    pub x: ViScaleAxis,
    pub y: ViScaleAxis,
    pub field: ViScanoutField,
}

/// One complete latched fourteen-word public VI register image.
///
/// The words stay together so origin/stride/timing/window/scale/field cannot
/// drift across the renderer seam. Construction validates the geometry that
/// every digital backend must consume; reserved pixel formats remain a typed
/// presentation error rather than being rewritten here.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViScanoutRegisters {
    words: [u32; Self::WORD_COUNT],
}

impl ViScanoutRegisters {
    pub const WORD_COUNT: usize = 14;

    pub fn from_words(words: [u32; Self::WORD_COUNT]) -> Self {
        let active_window = ViActiveWindow::try_from_registers(words[9], words[10]);
        if active_window.is_some() {
            assert_ne!(
                words[2] & 0x0fff,
                0,
                "VI WIDTH has zero effective source stride for an active scanout image"
            );
        }
        let _ = ViResampleControl::from_registers(words[12], words[13], words[0], words[4] & 1);
        Self { words }
    }

    pub const fn words(self) -> [u32; Self::WORD_COUNT] {
        self.words
    }

    pub const fn status(self) -> u32 {
        self.words[0]
    }

    pub const fn origin(self) -> u32 {
        self.words[1] & 0x00ff_ffff
    }

    pub const fn width(self) -> u32 {
        self.words[2] & 0x0fff
    }

    pub fn active_window(self) -> Option<ViActiveWindow> {
        // Construction proved these fields; repeat the cheap typed decode so
        // no second representation can drift from the retained words.
        ViActiveWindow::try_from_registers(self.words[9], self.words[10])
    }

    pub const fn x_scale_register(self) -> u32 {
        self.words[12]
    }

    pub const fn y_scale_register(self) -> u32 {
        self.words[13]
    }

    pub fn resample(self) -> ViResampleControl {
        ViResampleControl::from_registers(
            self.words[12],
            self.words[13],
            self.words[0],
            self.words[4] & 1,
        )
    }

    pub fn filters(self) -> ViFilterControl {
        ViFilterControl::from_status(self.words[0])
    }
}

/// Authority for one presentation's VI state.
///
/// Integrated execution always uses `Registers`. `BackendOnly` is the
/// explicit compatibility path for unit embedders that do not own a live VI
/// register file; it cannot accidentally carry a partial geometry image.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViScanoutState {
    BackendOnly(ViFilterControl),
    Registers(ViScanoutRegisters),
}

impl Default for ViScanoutState {
    fn default() -> Self {
        Self::BackendOnly(ViFilterControl::default())
    }
}

impl ViScanoutState {
    pub fn filters(self) -> ViFilterControl {
        match self {
            Self::BackendOnly(filters) => filters,
            Self::Registers(registers) => registers.filters(),
        }
    }

    pub const fn registers(self) -> Option<ViScanoutRegisters> {
        match self {
            Self::BackendOnly(_) => None,
            Self::Registers(registers) => Some(registers),
        }
    }
}

impl ViResampleControl {
    pub fn from_registers(x_scale: u32, y_scale: u32, status: u32, field: u32) -> Self {
        let field = if status & (1 << 6) == 0 {
            assert_eq!(field, 0, "progressive VI scanout cannot carry an odd field");
            ViScanoutField::Progressive
        } else {
            match field {
                0 => ViScanoutField::InterlacedEven,
                1 => ViScanoutField::InterlacedOdd,
                _ => panic!("interlaced VI field {field} exceeds one bit"),
            }
        };
        Self {
            x: ViScaleAxis::from_register(x_scale),
            y: ViScaleAxis::from_register(y_scale),
            field,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ViPresentation {
    /// Present a black image while VI retrace timing continues normally.
    pub blanked: bool,
    /// Interpolate the first two source rows by this public 10-bit factor and
    /// repeat the resulting row over scanout. `None` disables VI fade.
    pub fade: Option<u16>,
    /// Repeat the first source row over the complete scanout image.
    pub repeat_line: bool,
    /// Complete live register authority or an explicit backend-only filter
    /// compatibility state.
    pub scanout: ViScanoutState,
    /// Deterministic entropy key for scanout noise. Integrated execution uses
    /// the exact guest cycle of the VI retrace, so repeated runs agree while
    /// successive fields do not freeze gamma dither to the screen.
    pub noise_seed: u64,
}

impl ViPresentation {
    /// Guest-programmed active digital output lines for this exact field.
    ///
    /// This is deliberately distinct from the physical source-row footprint:
    /// `Y_SCALE`, interlace, fade, and repeat-line select source samples, while
    /// `V_START` names the output rectangle. Backends that extend a source
    /// image for filtering can retain their complete storage while hosts use
    /// this typed extent to keep those extension rows out of interactive UI.
    pub fn active_output_height(self) -> Option<NonZeroU32> {
        let height = self.scanout.registers()?.active_window()?.output_height();
        Some(NonZeroU32::new(height).expect("ViActiveWindow proves a nonzero output height"))
    }
}

/// Memory authority for one exact VI presentation boundary.
///
/// Integrated execution always supplies `Physical`. The compatibility form
/// exists only for standalone backends whose presentation has no live VI
/// register image; construction enforces that distinction so arbitrary live
/// `VI_ORIGIN` bytes can never silently fall back to a resident host image.
pub enum PresentMemory<'call> {
    Physical(fn64_runtime::PhysicalRdramRead<'call>),
    BackendResidentCompatibility,
}

/// Complete input to one renderer presentation.
///
/// Keeping memory and the fourteen-word VI image in one move-only request
/// binds source bytes to the retrace that selected them. The physical-memory
/// lifetime prevents a safe backend from retaining process RDRAM beyond this
/// call.
pub struct PresentRequest<'call> {
    vi: ViPresentation,
    memory: PresentMemory<'call>,
}

impl<'call> PresentRequest<'call> {
    /// Integrated presentation with one complete live VI register image.
    pub fn live(vi: ViPresentation, memory: fn64_runtime::PhysicalRdramRead<'call>) -> Self {
        assert!(
            matches!(vi.scanout, ViScanoutState::Registers(_)),
            "live physical presentation requires complete VI registers"
        );
        Self {
            vi,
            memory: PresentMemory::Physical(memory),
        }
    }

    /// Standalone physical-memory presentation using synthesized backend
    /// geometry. This can drive behavior tests but is not live-register release
    /// evidence.
    pub fn physical_compatibility(
        vi: ViPresentation,
        memory: fn64_runtime::PhysicalRdramRead<'call>,
    ) -> Self {
        assert!(
            matches!(vi.scanout, ViScanoutState::BackendOnly(_)),
            "physical compatibility presentation cannot carry live VI registers"
        );
        Self {
            vi,
            memory: PresentMemory::Physical(memory),
        }
    }

    pub fn backend_resident(vi: ViPresentation) -> Self {
        assert!(
            matches!(vi.scanout, ViScanoutState::BackendOnly(_)),
            "backend-resident presentation cannot carry live VI registers"
        );
        Self {
            vi,
            memory: PresentMemory::BackendResidentCompatibility,
        }
    }

    pub fn into_parts(self) -> (ViPresentation, PresentMemory<'call>) {
        (self.vi, self.memory)
    }
}

/// Position and byte layout of an image captured for release evidence.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ReleaseCaptureFormat {
    /// RT64's post-VI swapchain color attachment: blue, green, red, alpha.
    PostViBgra8Unorm,
}

impl ReleaseCaptureFormat {
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::PostViBgra8Unorm => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseCapturePixelsError {
    ZeroDimension,
    ZeroVisibleHeight,
    VisibleHeightExceedsStorage { visible: u32, storage: u32 },
    TightRowBytesOverflow,
    RowBytesTooSmall { minimum: u32, actual: u32 },
    ByteLengthOverflow,
    ByteLengthMismatch { expected: usize, actual: usize },
}

impl std::fmt::Display for ReleaseCapturePixelsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroDimension => formatter.write_str("release capture dimensions must be nonzero"),
            Self::ZeroVisibleHeight => {
                formatter.write_str("release capture visible height must be nonzero")
            }
            Self::VisibleHeightExceedsStorage { visible, storage } => write!(
                formatter,
                "release capture visible height {visible} exceeds storage height {storage}"
            ),
            Self::TightRowBytesOverflow => {
                formatter.write_str("release capture tight row byte count overflows u32")
            }
            Self::RowBytesTooSmall { minimum, actual } => write!(
                formatter,
                "release capture row pitch {actual} is smaller than the {minimum}-byte pixel row"
            ),
            Self::ByteLengthOverflow => {
                formatter.write_str("release capture byte length overflows usize")
            }
            Self::ByteLengthMismatch { expected, actual } => write!(
                formatter,
                "release capture storage has {actual} bytes; layout requires exactly {expected}"
            ),
        }
    }
}

impl std::error::Error for ReleaseCapturePixelsError {}

/// Named, unvalidated input for a release-capture layout.
///
/// The fields intentionally distinguish renderer storage height from the
/// guest-visible height. Named construction also prevents adjacent `u32`
/// dimensions and row pitch from being silently transposed at backend seams.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseCaptureLayoutSpec {
    pub format: ReleaseCaptureFormat,
    pub width: u32,
    pub storage_height: u32,
    pub visible_height: u32,
    pub row_bytes: u32,
}

/// Validated format and storage geometry for one release capture.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseCaptureLayout {
    format: ReleaseCaptureFormat,
    width: u32,
    storage_height: u32,
    visible_height: u32,
    row_bytes: u32,
    byte_len: usize,
}

impl ReleaseCaptureLayout {
    pub fn try_new(spec: ReleaseCaptureLayoutSpec) -> Result<Self, ReleaseCapturePixelsError> {
        if spec.width == 0 || spec.storage_height == 0 {
            return Err(ReleaseCapturePixelsError::ZeroDimension);
        }
        if spec.visible_height == 0 {
            return Err(ReleaseCapturePixelsError::ZeroVisibleHeight);
        }
        if spec.visible_height > spec.storage_height {
            return Err(ReleaseCapturePixelsError::VisibleHeightExceedsStorage {
                visible: spec.visible_height,
                storage: spec.storage_height,
            });
        }
        let minimum = spec
            .width
            .checked_mul(spec.format.bytes_per_pixel())
            .ok_or(ReleaseCapturePixelsError::TightRowBytesOverflow)?;
        if spec.row_bytes < minimum {
            return Err(ReleaseCapturePixelsError::RowBytesTooSmall {
                minimum,
                actual: spec.row_bytes,
            });
        }
        let byte_len = usize::try_from(spec.row_bytes)
            .ok()
            .and_then(|row| {
                usize::try_from(spec.storage_height)
                    .ok()
                    .and_then(|height| row.checked_mul(height))
            })
            .ok_or(ReleaseCapturePixelsError::ByteLengthOverflow)?;
        Ok(Self {
            format: spec.format,
            width: spec.width,
            storage_height: spec.storage_height,
            visible_height: spec.visible_height,
            row_bytes: spec.row_bytes,
            byte_len,
        })
    }

    pub const fn format(self) -> ReleaseCaptureFormat {
        self.format
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn storage_height(self) -> u32 {
        self.storage_height
    }

    pub const fn visible_height(self) -> u32 {
        self.visible_height
    }

    pub const fn row_bytes(self) -> u32 {
        self.row_bytes
    }

    pub const fn byte_len(self) -> usize {
        self.byte_len
    }
}

/// Read-only fields of one validated owned release image.
///
/// This view is public so capture field reads remain concise. Only
/// [`ReleaseCapturePixels`] can install it as an owned image, and that wrapper
/// deliberately provides no mutable dereference: callers can edit pixel values
/// but cannot change the vector length independently of its layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseCapturePixelsView {
    pub format: ReleaseCaptureFormat,
    pub width: u32,
    pub height: u32,
    /// Guest-programmed active output lines at the captured present.
    /// Storage remains `height` rows so renderer evidence is never cropped.
    pub visible_height: u32,
    pub row_bytes: u32,
    pub bytes: Vec<u8>,
}

/// Owned pixel storage whose format, dimensions, row pitch, and exact byte
/// length are one validated value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseCapturePixels(ReleaseCapturePixelsView);

impl ReleaseCapturePixels {
    pub fn try_new(
        layout: ReleaseCaptureLayout,
        bytes: Vec<u8>,
    ) -> Result<Self, ReleaseCapturePixelsError> {
        let expected = layout.byte_len();
        if bytes.len() != expected {
            return Err(ReleaseCapturePixelsError::ByteLengthMismatch {
                expected,
                actual: bytes.len(),
            });
        }
        Ok(Self(ReleaseCapturePixelsView {
            format: layout.format(),
            width: layout.width(),
            height: layout.storage_height(),
            visible_height: layout.visible_height(),
            row_bytes: layout.row_bytes(),
            bytes,
        }))
    }

    /// Validate a caller-reused allocation before transferring its ownership.
    /// Validation failure leaves `reuse` unchanged.
    pub fn try_from_reused(
        layout: ReleaseCaptureLayout,
        reuse: &mut Vec<u8>,
    ) -> Result<Self, ReleaseCapturePixelsError> {
        let expected = layout.byte_len();
        if reuse.len() != expected {
            return Err(ReleaseCapturePixelsError::ByteLengthMismatch {
                expected,
                actual: reuse.len(),
            });
        }
        Self::try_new(layout, std::mem::take(reuse))
    }

    /// Install another validated layout and resize owned storage with zeroed
    /// new bytes.
    pub fn resize(&mut self, layout: ReleaseCaptureLayout) {
        self.0.bytes.resize(layout.byte_len(), 0);
        self.0.format = layout.format();
        self.0.width = layout.width();
        self.0.height = layout.storage_height();
        self.0.visible_height = layout.visible_height();
        self.0.row_bytes = layout.row_bytes();
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0.bytes
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.0.bytes
    }

    /// Complete rows inside the guest-programmed active output extent.
    /// Renderer-owned extension rows remain available through [`Self::as_bytes`].
    pub fn visible_bytes(&self) -> &[u8] {
        let visible_len = usize::try_from(self.0.row_bytes)
            .expect("validated row pitch fits the host")
            .checked_mul(
                usize::try_from(self.0.visible_height)
                    .expect("validated visible height fits the host"),
            )
            .expect("validated visible byte length fits the host");
        &self.0.bytes[..visible_len]
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0.bytes
    }
}

impl std::ops::Deref for ReleaseCapturePixels {
    type Target = ReleaseCapturePixelsView;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod release_capture_pixels_tests {
    use super::{
        ReleaseCaptureFormat, ReleaseCaptureLayout, ReleaseCaptureLayoutSpec, ReleaseCapturePixels,
        ReleaseCapturePixelsError,
    };

    fn layout(
        width: u32,
        storage_height: u32,
        visible_height: u32,
        row_bytes: u32,
    ) -> Result<ReleaseCaptureLayout, ReleaseCapturePixelsError> {
        ReleaseCaptureLayout::try_new(ReleaseCaptureLayoutSpec {
            format: ReleaseCaptureFormat::PostViBgra8Unorm,
            width,
            storage_height,
            visible_height,
            row_bytes,
        })
    }

    #[test]
    fn construction_binds_layout_to_exact_storage_length() {
        let format = ReleaseCaptureFormat::PostViBgra8Unorm;
        let pixels =
            ReleaseCapturePixels::try_new(layout(2, 2, 2, 12).unwrap(), vec![0; 24]).unwrap();
        assert_eq!(pixels.format, format);
        assert_eq!(
            (
                pixels.width,
                pixels.height,
                pixels.visible_height,
                pixels.row_bytes,
            ),
            (2, 2, 2, 12)
        );
        assert_eq!(pixels.as_bytes().len(), 24);

        assert_eq!(
            ReleaseCapturePixels::try_new(layout(2, 2, 2, 8).unwrap(), vec![0; 15]).unwrap_err(),
            ReleaseCapturePixelsError::ByteLengthMismatch {
                expected: 16,
                actual: 15,
            }
        );
        assert_eq!(
            layout(2, 2, 2, 7).unwrap_err(),
            ReleaseCapturePixelsError::RowBytesTooSmall {
                minimum: 8,
                actual: 7,
            }
        );
    }

    #[test]
    fn reused_storage_moves_only_after_validation() {
        let mut reuse = Vec::with_capacity(64);
        reuse.resize(16, 0x5a);
        let allocation = reuse.as_ptr();
        let pixels =
            ReleaseCapturePixels::try_from_reused(layout(2, 2, 2, 8).unwrap(), &mut reuse).unwrap();
        assert!(reuse.is_empty());
        assert_eq!(pixels.as_bytes().as_ptr(), allocation);

        let mut reuse = pixels.into_bytes();
        let allocation = reuse.as_ptr();
        let error = ReleaseCapturePixels::try_from_reused(layout(2, 3, 3, 8).unwrap(), &mut reuse)
            .unwrap_err();
        assert_eq!(
            error,
            ReleaseCapturePixelsError::ByteLengthMismatch {
                expected: 24,
                actual: 16,
            }
        );
        assert_eq!(reuse.as_ptr(), allocation);
        assert_eq!(reuse.len(), 16);
    }

    #[test]
    fn resize_is_transactional_for_invalid_layouts() {
        let mut pixels =
            ReleaseCapturePixels::try_new(layout(2, 2, 2, 8).unwrap(), vec![1; 16]).unwrap();
        let before = pixels.clone();
        assert_eq!(
            layout(3, 2, 2, 11).unwrap_err(),
            ReleaseCapturePixelsError::RowBytesTooSmall {
                minimum: 12,
                actual: 11,
            }
        );
        assert_eq!(pixels, before);

        pixels.resize(layout(3, 2, 2, 16).unwrap());
        assert_eq!(
            (
                pixels.width,
                pixels.height,
                pixels.visible_height,
                pixels.row_bytes,
            ),
            (3, 2, 2, 16)
        );
        assert_eq!(pixels.as_bytes().len(), 32);
    }

    #[test]
    fn visible_height_retains_renderer_extension_rows_outside_visible_prefix() {
        let row_bytes = 8;
        let mut bytes = vec![0x11; row_bytes * 240];
        bytes[row_bytes * 237..].fill(0xee);
        let pixels =
            ReleaseCapturePixels::try_new(layout(2, 240, 237, row_bytes as u32).unwrap(), bytes)
                .unwrap();

        assert_eq!(pixels.visible_bytes().len(), row_bytes * 237);
        assert!(pixels.visible_bytes().iter().all(|byte| *byte == 0x11));
        assert_eq!(pixels.as_bytes().len(), row_bytes * 240);
        assert!(pixels.as_bytes()[row_bytes * 237..]
            .iter()
            .all(|byte| *byte == 0xee));
        assert_eq!(
            layout(2, 240, 0, 8).unwrap_err(),
            ReleaseCapturePixelsError::ZeroVisibleHeight
        );
        assert_eq!(
            layout(2, 240, 241, 8).unwrap_err(),
            ReleaseCapturePixelsError::VisibleHeightExceedsStorage {
                visible: 241,
                storage: 240,
            }
        );
    }

    #[test]
    fn named_layout_spec_distinguishes_storage_visible_and_pitch() {
        let layout = ReleaseCaptureLayout::try_new(ReleaseCaptureLayoutSpec {
            format: ReleaseCaptureFormat::PostViBgra8Unorm,
            width: 320,
            storage_height: 240,
            visible_height: 237,
            row_bytes: 1280,
        })
        .unwrap();
        assert_eq!(layout.storage_height(), 240);
        assert_eq!(layout.visible_height(), 237);
        assert_eq!(layout.row_bytes(), 1280);
        assert_eq!(layout.byte_len(), 307_200);
    }
}

/// Concrete graphics API that produced an RT64 release image.
///
/// This is intentionally distinct from [`RenderGraphicsApi`], which models a
/// requested runtime policy and therefore includes `Automatic`. Release
/// evidence must name the API that actually became active; an unresolved
/// automatic selection is not an identity.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ActiveRenderGraphicsApi {
    D3d12,
    Vulkan,
    Metal,
}

/// Backend-owned identity used by fixed-cycle release evidence.
///
/// This is a self-report from the registered trait object, not a label supplied
/// by its host. The default is deliberately unidentified so compatibility and
/// test backends remain runnable while release capture fails closed unless the
/// concrete backend implements this evidence seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderBackendEvidence {
    Unidentified,
    Reference {
        /// IPL-selected television standard retained by the concrete backend
        /// from its last successful creation.
        tv_type: fn64_runtime::TvType,
    },
    Rt64 {
        /// IPL-selected television standard retained by the concrete backend
        /// from its last successful creation.
        tv_type: fn64_runtime::TvType,
        backend_identity: String,
        source_authoritative: bool,
        /// Concrete API backing the live RT64 device. This can never be an
        /// unresolved automatic request.
        graphics_api: ActiveRenderGraphicsApi,
        /// Canonical identity of the complete active RT64 runtime policy.
        settings_sha256: [u8; 32],
        /// True only when the active policy enables at least one identified
        /// replacement pack. Configured or staged packs do not qualify.
        replacement_packs_active: bool,
    },
}

impl RenderBackendEvidence {
    /// Television standard owned by an identified, successfully created
    /// renderer. Compatibility backends cannot fabricate this authority.
    pub const fn tv_type(&self) -> Option<fn64_runtime::TvType> {
        match self {
            Self::Unidentified => None,
            Self::Reference { tv_type } | Self::Rt64 { tv_type, .. } => Some(*tv_type),
        }
    }
}

/// One renderer-owned image whose presentation is tied to an exact guest
/// cycle. The backend supplies this only after the corresponding present has
/// completed; callers must still compare `guest_cycle` with their gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderReleaseCapture {
    pub guest_cycle: u64,
    pub backend_identity: String,
    pub source_authoritative: bool,
    /// Canonical identity of the complete user/enhancement/emulator/replacement
    /// policy active for this image. Pending settings or packs that require
    /// recreation, fail inspection, or fail activation must never be
    /// substituted here.
    pub settings_sha256: [u8; 32],
    pub pixels: ReleaseCapturePixels,
    /// Completed RT64 workload selected by this presentation.
    pub workload_id: NonZeroU64,
    pub present_id: u64,
}

/// Positive, finite effective raster scale selected by a renderer.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RenderResolutionScale {
    x: f32,
    y: f32,
}

impl RenderResolutionScale {
    pub fn try_new(x: f32, y: f32) -> Option<Self> {
        (x.is_finite() && y.is_finite() && x > 0.0 && y > 0.0).then_some(Self { x, y })
    }

    pub fn x(self) -> f32 {
        self.x
    }

    pub fn y(self) -> f32 {
        self.y
    }
}

/// Effective managed framebuffer geometry selected by the renderer's most
/// recent completed presentation.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RenderTargetDiagnostic {
    pub present_id: NonZeroU64,
    pub target_address: u32,
    /// Scale selected for the workload before target-specific normalization.
    pub workload_resolution_scale: RenderResolutionScale,
    /// Scale retained by the concrete target sampled by VI.
    pub resolution_scale: RenderResolutionScale,
    pub raster_width: NonZeroU32,
    pub raster_height: NonZeroU32,
    pub downsample_multiplier: NonZeroU32,
}

impl std::ops::Deref for RenderReleaseCapture {
    type Target = ReleaseCapturePixelsView;

    fn deref(&self) -> &Self::Target {
        &self.pixels.0
    }
}

impl RenderConfig {
    /// Construct a configuration for an explicitly NTSC program or fixture.
    /// Production boot paths should carry their IPL-selected [`TvType`](fn64_runtime::TvType)
    /// through [`Self::for_tv`] instead of assuming this standard.
    pub const fn ntsc(width: u32, height: u32) -> Self {
        Self::for_tv(width, height, fn64_runtime::TvType::Ntsc)
    }

    /// Construct a configuration bound to the IPL-selected television
    /// standard. Native RT64 uses its nominal 50/60 Hz rate when converting
    /// VI presentation factors into a logical workload rate.
    pub const fn for_tv(width: u32, height: u32, tv_type: fn64_runtime::TvType) -> Self {
        RenderConfig {
            width,
            height,
            tv_type,
        }
    }
}

#[cfg(test)]
mod render_config_tests {
    use super::RenderConfig;
    use fn64_runtime::TvType;

    #[test]
    fn explicit_tv_config_preserves_all_nominal_region_rates() {
        assert_eq!(
            RenderConfig::for_tv(320, 240, TvType::Ntsc).tv_type,
            TvType::Ntsc
        );
        assert_eq!(
            RenderConfig::for_tv(320, 240, TvType::Pal).tv_type,
            TvType::Pal
        );
        assert_eq!(
            RenderConfig::for_tv(320, 240, TvType::Mpal).tv_type,
            TvType::Mpal
        );
        assert_eq!(TvType::Ntsc.nominal_field_hz(), 60);
        assert_eq!(TvType::Pal.nominal_field_hz(), 50);
        assert_eq!(TvType::Mpal.nominal_field_hz(), 60);
    }

    #[test]
    fn named_ntsc_constructor_cannot_hide_its_standard() {
        assert_eq!(RenderConfig::ntsc(320, 240).tv_type, TvType::Ntsc);
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
    /// HLE preflight encountered a content-addressed microcode generation it
    /// cannot execute. The backend has committed no task effects; the runtime
    /// must run the whole ucode phase from its untouched post-rspboot state
    /// through the general RSP interpreter.
    NeedsLle {
        /// Exact complete live IMEM identity rejected by HLE. Keeping it in
        /// the typed outcome makes catalog discovery observable without
        /// weakening admission or scraping diagnostic text.
        ucode_sha256: [u8; 32],
    },
}

/// Whether one successful renderer submission reached an RDP FullSync.
///
/// The public RDP contract makes FullSync the source of the DP interrupt; task
/// type or the mere existence of a DPC range is not equivalent evidence.
/// Compatibility backends default to `Unidentified`, which remains runnable
/// for direct rendering but must not drive a fabricated device completion.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum DpFullSyncStatus {
    #[default]
    Unidentified,
    NotReached,
    Reached,
}

/// Raw-DPC progress guarantee selected by a renderer.
///
/// `Atomic` is the compatibility contract and carries no intermediate device-
/// timing authority. `Acknowledged` only promises transactional host commit
/// boundaries; a separate measured runtime policy must schedule them.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum RawDpcProgression {
    #[default]
    Atomic,
    Acknowledged,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RenderRawDpcContinuation(NonZeroU64);

impl RenderRawDpcContinuation {
    pub const fn new(value: u64) -> Self {
        Self(NonZeroU64::new(value).expect("raw DPC continuation token must be nonzero"))
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RawDpcStep {
    Start,
    Resume(RenderRawDpcContinuation),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RawDpcChunkStatus {
    /// More schedule-owned command range must remain after this commit.
    Continue(RenderRawDpcContinuation),
    /// This commit consumed the schedule's final range.
    Complete,
}

/// One runtime-issued renderer boundary. The range is an execution quantum,
/// not a claim about RDP clocks, DMA fetch timing, or silicon command width.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RawDpcQuantum {
    pub request: fn64_runtime::DpcBackendQuantumRequest,
    pub output_addr: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RawDpcChunkAck {
    pub transaction: fn64_runtime::DpcTransactionId,
    pub quantum: fn64_runtime::DpcQuantumId,
    pub committed_through: fn64_runtime::DpcCursor,
    pub status: RawDpcChunkStatus,
    pub full_sync: DpFullSyncStatus,
}

/// Opaque ownership token for a backend-retained HLE task continuation.
///
/// The backend, not the ABI scheduler, owns the renderer-local continuation
/// state. The scheduler may move this token between `Running` and `Suspended`
/// states, but may resume it at most once.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RenderTaskContinuation(u64);

impl RenderTaskContinuation {
    pub const fn new(value: u64) -> Self {
        assert!(value != 0, "render continuation token must be nonzero");
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Whether a task call begins new work or consumes one retained continuation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RenderTaskStep {
    Start,
    Resume(RenderTaskContinuation),
}

/// Result of one committed renderer chunk.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RenderTaskChunkStatus {
    Complete,
    Continue(RenderTaskContinuation),
    Yielded,
    NeedsLle { ucode_sha256: [u8; 32] },
}

/// The task-progress guarantee a backend makes at registration time.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum RenderTaskChunking {
    /// `process_task_chunk(Start)` is one indivisible compatibility call.
    #[default]
    Atomic,
    /// `Continue(token)` marks a committed boundary at which SIG0 may be
    /// observed before the token is consumed exactly once.
    Resumable,
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
    /// An ordered HLE preflight cannot bind one task-entry or self-loaded IMEM
    /// generation to the exact native input image admitted for that family.
    /// This is an internal typed handoff signal: a transactional backend maps
    /// it to [`FrameStatus::NeedsLle`] without committing speculative
    /// mutations.
    RequiresLle { ucode_sha256: [u8; 32] },
    /// `task.output_buff`/`output_buff_size` describe a region outside the
    /// `rdram` slice `process_task` was given -- a malformed or
    /// adversarial task header, reported rather than causing a panic or an
    /// out-of-bounds read.
    InvalidTaskBounds {
        offset: u32,
        len: u32,
        rdram_len: usize,
    },
    /// A live VI source image's checked physical footprint exceeds RDRAM.
    InvalidViSourceBounds {
        origin: u32,
        stride_pixels: u32,
        rows: u64,
        bytes_per_pixel: u8,
        rdram_len: usize,
    },
    /// The programmed VI origin cannot address complete pixels in the
    /// selected memory format.
    InvalidViSourceAlignment { origin: u32, bytes_per_pixel: u8 },
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
            RenderError::RequiresLle { ucode_sha256 } => {
                write!(f, "microcode SHA-256 ")?;
                for byte in ucode_sha256 {
                    write!(f, "{byte:02x}")?;
                }
                write!(f, " requires the general RSP LLE path")
            }
            RenderError::InvalidTaskBounds {
                offset,
                len,
                rdram_len,
            } => write!(
                f,
                "task output buffer [{offset:#010x}, +{len}) exceeds rdram length {rdram_len}"
            ),
            RenderError::InvalidViSourceBounds {
                origin,
                stride_pixels,
                rows,
                bytes_per_pixel,
                rdram_len,
            } => write!(
                f,
                "VI source origin {origin:#010x}, stride {stride_pixels} pixels, {rows} rows, and {bytes_per_pixel} bytes/pixel exceeds {rdram_len}-byte physical RDRAM"
            ),
            RenderError::InvalidViSourceAlignment {
                origin,
                bytes_per_pixel,
            } => write!(
                f,
                "VI source origin {origin:#010x} is not aligned for {bytes_per_pixel}-byte pixels"
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

    /// Observe one completed CPU/non-RDP halfword store to physical RDRAM.
    /// The public RDP memory-interface rule assigns a hidden-bit mutation to
    /// every such store, including a same-value store that byte comparison
    /// cannot discover later. Backends must state whether they applied that
    /// mutation to a Rust-owned sidecar; there is deliberately no silent
    /// default implementation.
    fn observe_non_rdp_write16(&mut self, write: NonRdpWrite16) -> NonRdpWrite16Disposition;

    /// Stage settings before `create`, or apply live-safe fields after it.
    /// Backends must return a named error for unsupported settings rather than
    /// retain the request while rendering with a different configuration.
    fn apply_runtime_settings(
        &mut self,
        _settings: &RenderRuntimeSettings,
    ) -> Result<RenderSettingsApply, RenderError> {
        Err(RenderError::Backend {
            backend: "render-runtime-settings",
            reason: "registered backend does not implement typed runtime settings".to_string(),
        })
    }

    /// Stage or live-apply the complete pinned RT64 enhancement policy.
    fn apply_enhancement_settings(
        &mut self,
        _settings: &RenderEnhancementSettings,
    ) -> Result<RenderPolicyApply, RenderError> {
        Err(RenderError::Backend {
            backend: "render-enhancement-settings",
            reason: "registered backend does not implement typed enhancement settings".to_string(),
        })
    }

    /// Stage or live-apply the complete pinned RT64 emulator/device policy.
    fn apply_emulator_settings(
        &mut self,
        _settings: &RenderEmulatorSettings,
    ) -> Result<RenderPolicyApply, RenderError> {
        Err(RenderError::Backend {
            backend: "render-emulator-settings",
            reason: "registered backend does not implement typed emulator settings".to_string(),
        })
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

    /// Execute a CPU/RSP-produced raw RDP command range selected through the
    /// DPC start/end registers. `output_addr` is the physical VI framebuffer
    /// selected at this submission boundary, under the same contract as
    /// `process_task`; it must not be inferred from backend call history.
    /// Backends that do not implement raw command execution return a named
    /// error; the default must never pretend the range rendered successfully.
    ///
    /// `wait_for_completion`: when `false`, a backend MAY return before the
    /// submitted work is complete, as long as it becomes complete no later
    /// than this backend's next call with `wait_for_completion = true` (or
    /// any other call that reads GPU-completed state, e.g. present).
    /// Callers must always pass `true` for the last submission before
    /// anything downstream needs the finished frame. A backend that has no
    /// concept of asynchronous completion may ignore the flag and always
    /// wait -- that is always correct, just not always fast.
    fn process_rdp_commands(
        &mut self,
        _rdram: &mut [u8],
        start: u32,
        end: u32,
        _output_addr: u32,
        _wait_for_completion: bool,
    ) -> Result<FrameStatus, RenderError> {
        let reason =
            format!("raw RDP command execution [{start:#010x}, {end:#010x}) is unsupported");
        fn64_runtime::record_unsupported_event(
            fn64_runtime::UnsupportedSubsystem::Render,
            "render.raw-rdp.default-backend",
            &reason,
            None,
            fn64_runtime::UnsupportedDisposition::ReturnedError,
        );
        Err(RenderError::Backend {
            backend: "render",
            reason,
        })
    }

    /// Whether this backend can commit separately scheduled raw-DPC chunks.
    /// Existing backends remain atomic and retain their historical call path.
    fn raw_dpc_progression(&self) -> RawDpcProgression {
        RawDpcProgression::Atomic
    }

    /// Execute one externally scheduled raw-DPC quantum.
    ///
    /// The default is a loud rejection. An acknowledged implementation must
    /// leave its private continuation unchanged on `Err`; memory is supplied
    /// as an ABI-owned shadow. Once backend entry occurs, either an `Err` or a
    /// malformed `Ok` poisons that orchestration transaction and is never
    /// retried. The ABI publishes a successful memory image only after
    /// validating transaction, quantum, cursor, identified FullSync evidence,
    /// and `Continue`/`Complete` against the remaining schedule.
    fn process_rdp_command_chunk(
        &mut self,
        _rdram: &mut [u8],
        quantum: RawDpcQuantum,
        _step: RawDpcStep,
    ) -> Result<RawDpcChunkAck, RenderError> {
        Err(RenderError::Backend {
            backend: "raw-dpc-chunk",
            reason: format!(
                "registered atomic backend cannot acknowledge DPC transaction {} quantum {}",
                quantum.request.transaction.get(),
                quantum.request.quantum.get()
            ),
        })
    }

    /// Availability of the explicitly non-certifying staged-RDRAM diagnostic.
    /// `DiagnosticOnly` is never authority to publish guest or device state.
    fn raw_dpc_batch_capability(&self) -> RawDpcBatchCapability {
        RawDpcBatchCapability::Unsupported
    }

    /// Consume a completely preflighted batch for render-only diagnostics.
    ///
    /// The default is a loud capability failure. Implementations must never
    /// loop over `process_rdp_commands` unless it owns a complete backend-state
    /// snapshot: an error after an earlier stream group would otherwise expose
    /// a partial diagnostic result. This seam does not represent `CMD_END`
    /// timing, interrupt ordering, or intermediate memory visibility.
    fn process_raw_dpc_batch(
        &mut self,
        _rdram: &mut [u8],
        _batch: PreflightedRawDpcBatch,
        _output_addr: u32,
    ) -> Result<RawDpcBatchOutcome, RenderError> {
        Err(RenderError::Backend {
            backend: "raw-dpc-batch",
            reason: "registered backend does not implement diagnostic raw-DPC batches".to_string(),
        })
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

/// One post-commit, non-RDP 16-bit write at a canonical physical RDRAM
/// halfword. Construction rejects sparse KSEG/MMIO offsets and unaligned
/// addresses so a backend never has to reinterpret guest virtual addresses.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NonRdpWrite16 {
    logical_offset: fn64_runtime::RdramAddr,
    value: u16,
}

impl NonRdpWrite16 {
    pub fn new(logical_offset: u32, value: u16) -> Self {
        assert!(
            logical_offset < fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u32,
            "non-RDP halfword write offset {logical_offset:#x} is outside physical RDRAM"
        );
        assert!(
            logical_offset.is_multiple_of(2),
            "non-RDP halfword write offset {logical_offset:#x} is unaligned"
        );
        Self {
            logical_offset: fn64_runtime::RdramAddr::from_offset(logical_offset),
            value,
        }
    }

    pub const fn logical_offset(self) -> fn64_runtime::RdramAddr {
        self.logical_offset
    }

    pub const fn value(self) -> u16 {
        self.value
    }
}

/// Explicit ownership result for [`RenderBackend::observe_non_rdp_write16`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NonRdpWrite16Disposition {
    AppliedHiddenSidecar,
    NoRustHiddenSidecar,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_resolution_scale_admits_only_positive_finite_axes() {
        let scale = RenderResolutionScale::try_new(2.0, 1.5).unwrap();
        assert_eq!((scale.x(), scale.y()), (2.0, 1.5));
        for invalid in [0.0, -1.0, f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            assert_eq!(RenderResolutionScale::try_new(invalid, 1.0), None);
            assert_eq!(RenderResolutionScale::try_new(1.0, invalid), None);
        }
    }

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

        fn observe_non_rdp_write16(&mut self, _write: NonRdpWrite16) -> NonRdpWrite16Disposition {
            NonRdpWrite16Disposition::NoRustHiddenSidecar
        }

        fn process_task(
            &mut self,
            rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
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

        fn present(&mut self, _request: PresentRequest<'_>) -> Result<(), RenderError> {
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
        backend.create(&RenderConfig::ntsc(320, 240)).unwrap();
        let mut rdram = vec![0u8; 4096];
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let task = OsTask {
            task_type: M_GFXTASK,
            output_buff: 0,
            output_buff_size: 100,
            ..Default::default()
        };
        assert_eq!(
            backend
                .process_task(&mut rdram, &mut rsp_memory, &task, 0)
                .unwrap(),
            FrameStatus::Complete
        );
        assert_eq!(
            backend.identify_microcode(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]),
            None
        );
        backend
            .present(PresentRequest::backend_resident(ViPresentation::default()))
            .unwrap();
    }

    #[test]
    fn atomic_chunk_adapter_completes_start_and_rejects_resume() {
        let mut backend = fake(vec![UcodeId::F3dex2]);
        backend.create(&RenderConfig::ntsc(1, 1)).unwrap();
        let mut rdram = vec![0u8; 16];
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        assert_eq!(backend.task_chunking(), RenderTaskChunking::Atomic);
        assert_eq!(
            backend
                .process_task_chunk(
                    &mut rdram,
                    &mut rsp_memory,
                    &OsTask::default(),
                    0,
                    RenderTaskStep::Start,
                )
                .unwrap(),
            RenderTaskChunkStatus::Complete
        );
        let error = backend
            .process_task_chunk(
                &mut rdram,
                &mut rsp_memory,
                &OsTask::default(),
                0,
                RenderTaskStep::Resume(RenderTaskContinuation::new(1)),
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("cannot resume continuation token 1"));
    }

    #[test]
    fn atomic_raw_dpc_backend_rejects_acknowledged_chunks_by_name() {
        let mut backend = fake(vec![]);
        let transaction =
            fn64_runtime::DpcTransactionId::from_submission(fn64_runtime::DpcSubmission {
                token: 9,
                source: fn64_runtime::DpcSubmissionSource::Rdram,
                start: 0x100,
                end: 0x108,
            });
        let request = fn64_runtime::DpcBackendQuantumRequest {
            transaction,
            quantum: fn64_runtime::DpcQuantumId::new(1),
            start: fn64_runtime::DpcCursor::new(fn64_runtime::DpcSubmissionSource::Rdram, 0x100)
                .unwrap(),
            end: fn64_runtime::DpcCursor::new(fn64_runtime::DpcSubmissionSource::Rdram, 0x108)
                .unwrap(),
        };
        assert_eq!(backend.raw_dpc_progression(), RawDpcProgression::Atomic);
        let error = backend
            .process_rdp_command_chunk(
                &mut [],
                RawDpcQuantum {
                    request,
                    output_addr: 0,
                },
                RawDpcStep::Start,
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("atomic backend cannot acknowledge DPC transaction 9 quantum 1"));
    }

    #[test]
    fn process_task_before_create_is_not_ready() {
        let mut backend = fake(vec![UcodeId::F3dex2]);
        let mut rdram = vec![0u8; 16];
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let err = backend
            .process_task(&mut rdram, &mut rsp_memory, &OsTask::default(), 0)
            .unwrap_err();
        assert!(matches!(err, RenderError::NotReady(_)));
    }

    #[test]
    fn unlisted_ucode_traps_by_name_not_silently() {
        let mut backend = fake(vec![]); // declares NO supported ucodes
        backend.create(&RenderConfig::ntsc(64, 64)).unwrap();
        let mut rdram = vec![0u8; 16];
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let task = OsTask {
            ucode: 0x8000_1234,
            ..Default::default()
        };
        let err = backend
            .process_task(&mut rdram, &mut rsp_memory, &task, 0)
            .unwrap_err();
        match err {
            RenderError::UnsupportedUcode { ucode_addr } => assert_eq!(ucode_addr, 0x8000_1234),
            other => panic!("expected UnsupportedUcode, got {other:?}"),
        }
    }

    #[test]
    fn out_of_bounds_output_buffer_is_a_named_error_not_a_panic() {
        let mut backend = fake(vec![UcodeId::F3dex2]);
        backend.create(&RenderConfig::ntsc(64, 64)).unwrap();
        let mut rdram = vec![0u8; 16];
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let task = OsTask {
            output_buff: 10,
            output_buff_size: 100,
            ..Default::default()
        };
        let err = backend
            .process_task(&mut rdram, &mut rsp_memory, &task, 0)
            .unwrap_err();
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
    fn default_raw_rdp_unsupported_error_records_typed_evidence() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let mut backend = FakeBackend {
            ready: true,
            ucodes: vec![],
            frames_presented: 0,
        };
        let error = backend
            .process_rdp_commands(&mut [], 0x100, 0x108, 0, true)
            .unwrap_err();
        assert!(error.to_string().contains("is unsupported"));
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert!(events[0].operation.starts_with("render.raw-rdp."));
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::ReturnedError
        );
    }

    #[test]
    fn default_raw_dpc_batch_capability_rejects_without_memory_mutation() {
        let mut backend = FakeBackend {
            ready: true,
            ucodes: vec![],
            frames_presented: 0,
        };
        let mut rdram = vec![0x5a; 0x200];
        let before = rdram.clone();
        let submission =
            OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0xe900_0000, 0]).unwrap();
        let batch = RawDpcBatch::new(vec![submission])
            .unwrap()
            .preflight(rdram.len())
            .unwrap();

        let error = backend
            .process_raw_dpc_batch(&mut rdram, batch, 0)
            .unwrap_err();

        assert_eq!(
            backend.raw_dpc_batch_capability(),
            RawDpcBatchCapability::Unsupported
        );
        assert!(error
            .to_string()
            .contains("does not implement diagnostic raw-DPC batches"));
        assert_eq!(rdram, before);
    }

    #[test]
    fn ucode_id_other_is_distinct_from_named_variants() {
        assert_ne!(UcodeId::Other(0), UcodeId::F3dex2);
        assert_ne!(UcodeId::Fast3d, UcodeId::F3dex);
        assert_ne!(UcodeId::F3dex, UcodeId::F3dlx);
        assert_ne!(UcodeId::F3dlx, UcodeId::F3dlxRej);
        assert_ne!(UcodeId::F3dex, UcodeId::F3dex2);
        assert_ne!(UcodeId::F3dex2, UcodeId::F3dex2NoN);
        assert_ne!(UcodeId::F3dex2NoN, UcodeId::F3dex2Rej);
        assert_ne!(UcodeId::F3dex2Rej, UcodeId::F3dlx2Rej);
        assert_ne!(UcodeId::F3dlx2Rej, UcodeId::F3dzex2);
        assert_ne!(UcodeId::S2dex, UcodeId::S2dex2);
        assert_ne!(UcodeId::L3dex, UcodeId::L3dex2);
        assert_eq!(UcodeId::Other(7), UcodeId::Other(7));
    }

    #[test]
    fn vi_filter_control_decodes_latched_status_bits() {
        assert_eq!(
            ViFilterControl::from_status(3 | (2 << 8) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 16),),
            ViFilterControl {
                pixel_type: ViPixelType::Rgba32,
                antialias_mode: ViAaMode::ResampleOnly,
                gamma: true,
                gamma_dither: true,
                divot: true,
                dither_filter: true,
            }
        );
        assert_eq!(
            ViFilterControl::from_status(2).pixel_type,
            ViPixelType::Rgba16
        );

        for (bits, expected) in [
            (0, ViAaMode::AaResampleAlways),
            (1, ViAaMode::AaResampleWhenNeeded),
            (2, ViAaMode::ResampleOnly),
            (3, ViAaMode::Replicate),
        ] {
            let status = bits << 8;
            let decoded = ViFilterControl::from_status(status).antialias_mode;
            assert_eq!(decoded, expected);
            assert_eq!(decoded.status_bits(), Some(status));
            assert_eq!(decoded.silhouette_aa_enabled(), bits < 2);
            assert_eq!(decoded.resampling_enabled(), bits < 3);
        }
        assert_eq!(ViAaMode::Unspecified.status_bits(), None);
        assert!(!ViAaMode::Unspecified.silhouette_aa_enabled());
        assert!(ViAaMode::Unspecified.resampling_enabled());
    }

    #[test]
    fn vi_resample_control_decodes_every_public_scale_and_offset_code() {
        for code in 0..=0x0fff {
            let step = ViScaleAxis::from_register(code);
            assert_eq!(step.step_u2_10(), code as u16);
            assert_eq!(step.offset_u2_10(), 0);

            let offset = ViScaleAxis::from_register(code << 16);
            assert_eq!(offset.step_u2_10(), 0);
            assert_eq!(offset.offset_u2_10(), code as u16);
        }

        let progressive = ViResampleControl::from_registers(0x0123_0456, 0x0789_0abc, 0, 0);
        assert_eq!(progressive.x.step_u2_10(), 0x456);
        assert_eq!(progressive.x.offset_u2_10(), 0x123);
        assert_eq!(progressive.y.step_u2_10(), 0xabc);
        assert_eq!(progressive.y.offset_u2_10(), 0x789);
        assert_eq!(progressive.field, ViScanoutField::Progressive);

        assert_eq!(
            ViResampleControl::from_registers(0, 0, 1 << 6, 0).field,
            ViScanoutField::InterlacedEven
        );
        assert_eq!(
            ViResampleControl::from_registers(0, 0, 1 << 6, 1).field,
            ViScanoutField::InterlacedOdd
        );
    }

    #[test]
    fn vi_active_window_decodes_public_pixel_and_half_line_extents() {
        let window = ViActiveWindow::from_registers(0x006c_02ec, 0x0025_01ff);
        assert_eq!(window.horizontal_register(), 0x006c_02ec);
        assert_eq!(window.vertical_register(), 0x0025_01ff);
        assert_eq!(window.output_width(), 640);
        assert_eq!(window.output_height(), 237);

        let masked = ViActiveWindow::from_registers(0xfc6c_f2ec, 0xfc25_f1ff);
        assert_eq!(masked, window);
        assert_eq!(ViActiveWindow::try_from_registers(0, 0), None);
        assert_eq!(ViActiveWindow::try_from_registers(0, 0x0025_01ff), None);
        assert_eq!(ViActiveWindow::try_from_registers(0x006c_02ec, 0), None);
        assert_eq!(
            ViActiveWindow::try_from_registers(0x006c_02ec, 0x0025_01ff),
            Some(window)
        );
    }

    #[test]
    fn active_output_height_is_owned_by_window_not_source_resampling() {
        let mut words = [0u32; ViScanoutRegisters::WORD_COUNT];
        words[0] = 2;
        words[2] = 480;
        words[9] = 0x006c_02ec;
        words[10] = 0x0025_01ff;
        words[12] = u32::from(ViScaleAxis::ONE);
        words[13] = 0x0123_0200;
        let progressive = ViPresentation {
            scanout: ViScanoutState::Registers(ViScanoutRegisters::from_words(words)),
            ..ViPresentation::default()
        };
        assert_eq!(progressive.active_output_height().unwrap().get(), 237);

        words[0] |= 1 << 6;
        words[4] = 1;
        words[13] = 0x03ff_07ff;
        let interlaced_resampled = ViPresentation {
            scanout: ViScanoutState::Registers(ViScanoutRegisters::from_words(words)),
            ..ViPresentation::default()
        };
        assert_eq!(interlaced_resampled.active_output_height(), progressive.active_output_height());
        assert_eq!(ViPresentation::default().active_output_height(), None);
    }

    #[test]
    #[should_panic(expected = "VI H_START has an empty or reversed active window")]
    fn vi_active_window_rejects_reversed_horizontal_extent() {
        let _ = ViActiveWindow::from_registers(0x0200_0100, 0x0025_01ff);
    }

    #[test]
    #[should_panic(expected = "is not a whole output line")]
    fn vi_active_window_rejects_half_line_output_extent() {
        let _ = ViActiveWindow::from_registers(0x006c_02ec, 0x0025_0200);
    }

    #[test]
    fn vi_scanout_registers_retain_one_complete_atomic_image() {
        let mut words = [0u32; ViScanoutRegisters::WORD_COUNT];
        words[0] = 3 | (1 << 6) | (2 << 8) | (1 << 16);
        words[1] = 0x0012_3456;
        words[2] = 320;
        words[3] = 2;
        words[4] = 1;
        words[5] = 0x03e5_2239;
        words[6] = 525;
        words[7] = 3093;
        words[8] = 0x0c15_0c15;
        words[9] = 0x006c_02ec;
        words[10] = 0x0025_01ff;
        words[11] = 0x000e_0204;
        words[12] = 0x0123_0200;
        words[13] = 0x0234_0400;

        let registers = ViScanoutRegisters::from_words(words);
        assert_eq!(registers.words(), words);
        assert_eq!(registers.status(), words[0]);
        assert_eq!(registers.origin(), words[1]);
        assert_eq!(registers.width(), 320);
        assert_eq!(registers.active_window().unwrap().output_width(), 640);
        assert_eq!(registers.active_window().unwrap().output_height(), 237);
        assert_eq!(registers.x_scale_register(), words[12]);
        assert_eq!(registers.y_scale_register(), words[13]);
        assert_eq!(registers.resample().field, ViScanoutField::InterlacedOdd);
        assert_eq!(registers.resample().x.step_u2_10(), 0x200);
        assert_eq!(registers.filters().pixel_type, ViPixelType::Rgba32);
        assert_eq!(registers.filters().antialias_mode, ViAaMode::ResampleOnly);
        assert!(registers.filters().dither_filter);
        assert_eq!(
            ViScanoutState::Registers(registers).registers(),
            Some(registers)
        );
    }

    #[test]
    #[should_panic(expected = "VI WIDTH has zero effective source stride")]
    fn vi_scanout_registers_reject_zero_source_stride() {
        let mut words = [0u32; ViScanoutRegisters::WORD_COUNT];
        words[9] = 0x006c_02ec;
        words[10] = 0x0025_01ff;
        let _ = ViScanoutRegisters::from_words(words);
    }

    #[test]
    #[should_panic(expected = "VI WIDTH has zero effective source stride")]
    fn vi_scanout_registers_reject_width_with_only_unused_bits() {
        let mut words = [0u32; ViScanoutRegisters::WORD_COUNT];
        words[2] = 0x1000;
        words[9] = 0x006c_02ec;
        words[10] = 0x0025_01ff;
        let _ = ViScanoutRegisters::from_words(words);
    }

    #[test]
    fn vi_scanout_registers_retain_an_inactive_live_image() {
        let mut words = [0u32; ViScanoutRegisters::WORD_COUNT];
        words[0] = 3;
        words[1] = 0x0010_0000;
        words[2] = 320;
        let registers = ViScanoutRegisters::from_words(words);
        assert_eq!(registers.words(), words);
        assert_eq!(registers.active_window(), None);
    }

    #[test]
    fn vi_scanout_registers_retain_a_partially_programmed_inactive_image() {
        let mut words = [0u32; ViScanoutRegisters::WORD_COUNT];
        words[0] = 0x311e;
        words[1] = 0x280;
        words[2] = 320;
        words[9] = 0;
        words[10] = 0x0025_01ff;
        words[12] = 0x200;
        words[13] = 0x400;
        let registers = ViScanoutRegisters::from_words(words);
        assert_eq!(registers.words(), words);
        assert_eq!(registers.active_window(), None);
    }

    #[test]
    fn vi_scale_axis_ignores_preserved_not_used_register_bits() {
        let axis = ViScaleAxis::from_register(0xf123_e456);
        assert_eq!(axis.step_u2_10(), 0x456);
        assert_eq!(axis.offset_u2_10(), 0x123);
    }
}
