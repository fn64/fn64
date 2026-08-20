//! Typed ownership for native color-target generations.
//!
//! The base types plan exact device-byte rows and admit a resident generation
//! only after the complete target range is structurally covered. M3.3c's
//! bounded raster child is the sole production constructor of that completed
//! write: it requires exact GPU wait/callback/readback authority. This module
//! does not establish general raster, VI, parity, or performance behavior.
//!
//! Format and image-width semantics follow the public SGI *RDP Command
//! Summary* and libultra `gDPSetColorImage` documentation. RGBA5551 component
//! placement follows the public N64 programming/hardware documentation and
//! the already frozen M3.3a device-byte vector. No RT64 shader or runtime
//! implementation is copied into this mechanism.

use core::fmt;
use core::num::NonZeroUsize;

use fn64_render_ir::{PhysicalAddress, PhysicalMemoryLayout, PhysicalRange, ValidationError};

use crate::{ColorImage, ImageFormat, PixelSize};

mod fill;
mod oracle;
mod raster;
mod raw_triangle;
mod texrect;
mod triangle_pipeline;

pub use fill::{
    decode_fill_cycle_pixel, execute_fill_rectangle, resolve_fill_pixel_rectangle,
    FillCoordinateError, FillCycleBypassHazards, FillExecutionError, FillPixelRectangle,
};
pub use oracle::{pack_device_pixels, unpack_device_pixels, DeviceColorBytes, Rgba8};
pub use raster::{
    CommittedNativeRasterFrame, InFlightNativeRasterFill, NativeRasterDeviceOutcome,
    NativeRasterError, NativeRasterRenderer, PendingNativeRasterCommit, UninitializedNativeRaster,
};
pub use raw_triangle::{execute_raw_triangle, RawTriangleTexture};
pub use texrect::{
    execute_texture_rectangle, ClippedTexrectExtent, RdpScissorRect, TexrectAxis,
    TexrectBlendRegisters, TexrectConstantRegister, TexrectDraw, TexrectExecutionError,
    TexrectShading, TexrectTileBinding,
};
pub(crate) use triangle_pipeline::{admitted_triangle_fixture, ResolvedFragmentBlendParams};
pub use triangle_pipeline::{
    fixed_fixture_other_mode, InFlightTriangleDraw, RasterVertex, TriangleDrawOutput,
    TriangleFixture, TrianglePipelineDeviceOutcome, TrianglePipelineError,
    TrianglePipelineRenderer, TriangleRasterParams, TriangleTargetExtent,
    UninitializedTrianglePipeline, TMEM_SAMPLE_STATUS_INVALID_BYTE,
    TMEM_SAMPLE_STATUS_NO_TILE_BINDING, TMEM_SAMPLE_STATUS_OK, TMEM_SAMPLE_STATUS_REVERSED_EXTENT,
    TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorTargetFormat {
    Rgba16,
    Rgba32,
}

impl ColorTargetFormat {
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba16 => 2,
            Self::Rgba32 => 4,
        }
    }

    pub fn try_from_rdp(format: ImageFormat, size: PixelSize) -> Result<Self, TargetError> {
        match (format, size) {
            (ImageFormat::Rgba, PixelSize::Bits16) => Ok(Self::Rgba16),
            (ImageFormat::Rgba, PixelSize::Bits32) => Ok(Self::Rgba32),
            _ => Err(TargetError::UnsupportedColorTargetFormat { format, size }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ColorTargetExtent {
    width: u32,
    height: u32,
}

impl ColorTargetExtent {
    pub fn try_new(width: u32, height: u32) -> Result<Self, TargetError> {
        if width == 0 || height == 0 {
            return Err(TargetError::ZeroExtent { width, height });
        }
        width
            .checked_mul(height)
            .ok_or(TargetError::ExtentOverflow { width, height })?;
        Ok(Self { width, height })
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }

    pub const fn pixels(self) -> u32 {
        self.width * self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ColorTargetKey {
    address: PhysicalAddress,
    extent: ColorTargetExtent,
    format: ColorTargetFormat,
    range: PhysicalRange,
}

impl ColorTargetKey {
    pub fn try_new(
        address: PhysicalAddress,
        extent: ColorTargetExtent,
        format: ColorTargetFormat,
    ) -> Result<Self, TargetError> {
        let byte_len = extent
            .pixels()
            .checked_mul(format.bytes_per_pixel())
            .ok_or(TargetError::TargetByteLengthOverflow { extent, format })?;
        let end =
            address
                .get()
                .checked_add(byte_len)
                .ok_or(TargetError::TargetAddressOverflow {
                    start: address.get(),
                    byte_len,
                })?;
        let range = address.layout().range(address.get(), end)?;
        Ok(Self {
            address,
            extent,
            format,
            range,
        })
    }

    #[allow(dead_code)] // Reserved for the M3.2 decoder-to-target integration seam.
    pub(crate) fn try_from_color_image(
        image: ColorImage,
        height: u32,
    ) -> Result<Self, TargetError> {
        let format = ColorTargetFormat::try_from_rdp(image.format(), image.size())?;
        Self::try_new(
            image.address(),
            ColorTargetExtent::try_new(image.width(), height)?,
            format,
        )
    }

    pub const fn address(self) -> PhysicalAddress {
        self.address
    }

    pub const fn extent(self) -> ColorTargetExtent {
        self.extent
    }

    pub const fn format(self) -> ColorTargetFormat {
        self.format
    }

    pub const fn range(self) -> PhysicalRange {
        self.range
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetRectangle {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl TargetRectangle {
    pub fn try_new(x: u32, y: u32, width: u32, height: u32) -> Result<Self, TargetError> {
        if width == 0 || height == 0 {
            return Err(TargetError::ZeroRectangle {
                x,
                y,
                width,
                height,
            });
        }
        x.checked_add(width)
            .ok_or(TargetError::RectangleCoordinateOverflow {
                axis: "x",
                origin: x,
                length: width,
            })?;
        y.checked_add(height)
            .ok_or(TargetError::RectangleCoordinateOverflow {
                axis: "y",
                origin: y,
                length: height,
            })?;
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub const fn x(self) -> u32 {
        self.x
    }

    pub const fn y(self) -> u32 {
        self.y
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }

    pub const fn is_full(self, extent: ColorTargetExtent) -> bool {
        self.x == 0 && self.y == 0 && self.width == extent.width && self.height == extent.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TargetGeneration(u64);

impl TargetGeneration {
    pub const FIRST: Self = Self(1);

    pub const fn get(self) -> u64 {
        self.0
    }

    fn successor(self, key: ColorTargetKey) -> Result<Self, TargetError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(TargetError::GenerationExhausted { key, current: self })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetRowRange {
    row: u32,
    first_pixel: u32,
    pixel_count: u32,
    bytes: PhysicalRange,
}

impl TargetRowRange {
    pub const fn row(self) -> u32 {
        self.row
    }

    pub const fn first_pixel(self) -> u32 {
        self.first_pixel
    }

    pub const fn pixel_count(self) -> u32 {
        self.pixel_count
    }

    pub const fn bytes(self) -> PhysicalRange {
        self.bytes
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ExactRowPlan {
    key: ColorTargetKey,
    generation: TargetGeneration,
    rectangle: TargetRectangle,
}

impl ExactRowPlan {
    pub const fn key(&self) -> ColorTargetKey {
        self.key
    }

    pub const fn generation(&self) -> TargetGeneration {
        self.generation
    }

    pub const fn rectangle(&self) -> TargetRectangle {
        self.rectangle
    }

    pub fn rows(&self) -> TargetRows {
        TargetRows {
            key: self.key,
            rectangle: self.rectangle,
            next_row: 0,
        }
    }
}

pub struct TargetRows {
    key: ColorTargetKey,
    rectangle: TargetRectangle,
    next_row: u32,
}

impl Iterator for TargetRows {
    type Item = TargetRowRange;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next_row == self.rectangle.height {
            return None;
        }
        let row = self.rectangle.y + self.next_row;
        self.next_row += 1;
        let bpp = self.key.format.bytes_per_pixel();
        let first_pixel = row * self.key.extent.width + self.rectangle.x;
        let start = self.key.address.get() + first_pixel * bpp;
        let end = start + self.rectangle.width * bpp;
        let bytes = self
            .key
            .address
            .layout()
            .range(start, end)
            .expect("validated target and rectangle make every planned row in-bounds");
        Some(TargetRowRange {
            row,
            first_pixel,
            pixel_count: self.rectangle.width,
            bytes,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.rectangle.height - self.next_row) as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for TargetRows {}

#[derive(Debug, PartialEq, Eq)]
pub struct CandidateColorTarget {
    key: ColorTargetKey,
    generation: TargetGeneration,
    predecessor: Option<TargetGeneration>,
}

impl CandidateColorTarget {
    pub const fn key(&self) -> ColorTargetKey {
        self.key
    }

    pub const fn generation(&self) -> TargetGeneration {
        self.generation
    }

    pub const fn predecessor(&self) -> Option<TargetGeneration> {
        self.predecessor
    }

    pub fn plan_rows(&self, rectangle: TargetRectangle) -> Result<ExactRowPlan, TargetError> {
        let extent = self.key.extent;
        let end_x = rectangle.x + rectangle.width;
        let end_y = rectangle.y + rectangle.height;
        if end_x > extent.width || end_y > extent.height {
            return Err(TargetError::RectangleOutOfBounds {
                key: self.key,
                rectangle,
            });
        }
        Ok(ExactRowPlan {
            key: self.key,
            generation: self.generation,
            rectangle,
        })
    }

    /// Validates a move-only completion capability issued by the future raster
    /// owner. Row planning alone cannot construct `CompletedColorTargetWrite`.
    pub(crate) fn admit_completed_initialization(
        self,
        completed: CompletedColorTargetWrite,
    ) -> Result<InitializedCandidateColorTarget, TargetError> {
        if completed.key != self.key
            || completed.generation != self.generation
            || completed.range != self.key.range
        {
            return Err(TargetError::InitializationPlanMismatch {
                candidate_key: self.key,
                candidate_generation: self.generation,
                plan_key: completed.key,
                plan_generation: completed.generation,
                candidate_range: self.key.range,
                plan_range: completed.range,
            });
        }
        if completed.device_bytes.key != self.key
            || completed.device_bytes.generation != self.generation
            || completed.device_bytes.format != self.key.format
        {
            return Err(TargetError::CompletedByteDomainMismatch {
                key: self.key,
                generation: self.generation,
                byte_key: completed.device_bytes.key,
                byte_generation: completed.device_bytes.generation,
                expected_format: self.key.format,
                actual_format: completed.device_bytes.format,
            });
        }
        let expected_byte_len = self.key.range.len() as usize;
        if completed.device_bytes.bytes.len() != expected_byte_len {
            return Err(TargetError::CompletedByteLengthMismatch {
                key: self.key,
                generation: self.generation,
                expected: expected_byte_len,
                actual: completed.device_bytes.bytes.len(),
            });
        }
        // **A partial rectangle is admitted, and the uncovered region is
        // recorded rather than forgotten.**
        //
        // This used to refuse any partial completion of a brand-new target
        // (`PartialNewTargetInitialization`), on the reasoning that its
        // untouched bytes would be fabricated zeros. The premise was right
        // -- `fill::execute_fill_rectangle`'s brand-new arm really does
        // allocate a zero buffer -- but the reaction was wrong, and it was
        // wrong about the hardware: an N64 colour image is just RDRAM, and
        // the bytes outside a fill are whatever was already there. Hardware
        // has no notion of an uninitialized framebuffer to refuse.
        //
        // Refusing also cost real content. A partial-rect fill is ordinary
        // (the differential's `top-left-quadrant`, `single-pixel` and
        // `last-column-last-row` cases are all completed by
        // `fn64-render-reference`, which seeds its target from guest RDRAM
        // at `backend/imp.rs:440-447`), and a scissored fill is partial by
        // construction, so the refusal would have swallowed every clipped
        // rectangle the scissor fix now produces.
        //
        // **What keeps the fabricated zeros from becoming content**, which
        // is the invariant the old guard was really protecting:
        //
        // 1. They are never copied to guest RDRAM. The guest copy-back
        //    slices this buffer strictly by the DECLARED write ranges
        //    (`production.rs`'s `committed_guest_render_target_bytes`,
        //    consumed by `fn64-abi`'s `copy_committed_guest_writes`), and
        //    the decoder declares exactly the rectangle's own rows --
        //    now including the scissor clip, so declared and painted agree.
        //    A pixel this completion did not cover is in no declared range,
        //    so no digest is taken over it and no guest byte is written
        //    from it.
        // 2. They are named here, not assumed away. `covered` carries the
        //    rectangle this generation actually proved, so a later reader
        //    that wants to know which pixels are real can ask instead of
        //    inferring it from a row count.
        //
        // The residual, stated rather than hidden: a LATER packet against
        // the same target seeds its accumulator from this resident's full
        // buffer (`production.rs`'s `stage_and_report`), so an uncovered
        // pixel's zero survives into the next generation's buffer. It still
        // reaches no guest byte unless some later command declares a write
        // over it -- and if one does, that command paints it, because
        // declaration and execution now share one geometry. Closing the
        // gap properly means seeding a brand-new target from guest memory;
        // `docs/RT64-FILL-PARTIAL-SEED.md` records why that is possible and
        // what it costs.
        Ok(InitializedCandidateColorTarget {
            candidate: self,
            proof: InitializedRegionProof {
                range: completed.range,
                rows: completed.rectangle.height,
                covered: completed.rectangle,
                generation: completed.generation,
            },
            rectangle: completed.rectangle,
            device_bytes: completed.device_bytes,
        })
    }
}

/// Opaque evidence that the raster owner completed every named device-byte
/// write. Its only production constructor is private to the M3.3c raster
/// owner and consumes exact GPU-completion authority. A row plan by itself
/// therefore cannot publish a resident.
///
/// ```compile_fail
/// use fn64_render_wgpu::{CompletedColorTargetWrite, ExactRowPlan};
/// # fn plan() -> ExactRowPlan { unimplemented!() }
/// let completed: CompletedColorTargetWrite = plan();
/// # drop(completed);
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct CompletedColorTargetWrite {
    key: ColorTargetKey,
    generation: TargetGeneration,
    range: PhysicalRange,
    rectangle: TargetRectangle,
    device_bytes: DeviceColorBytes,
}

impl CompletedColorTargetWrite {
    /// The M4.3.4 fill executor's own production constructor. `device_bytes`
    /// must already be the target's full-extent byte content (enforced by
    /// [`DeviceColorBytes::new_for_fill`]); `rectangle` records the actual
    /// sub-region the executor wrote, for [`InitializedRegionProof`].
    pub(crate) const fn new_for_fill(
        key: ColorTargetKey,
        generation: TargetGeneration,
        range: PhysicalRange,
        rectangle: TargetRectangle,
        device_bytes: DeviceColorBytes,
    ) -> Self {
        Self {
            key,
            generation,
            range,
            rectangle,
            device_bytes,
        }
    }

    pub const fn key(&self) -> ColorTargetKey {
        self.key
    }

    pub const fn generation(&self) -> TargetGeneration {
        self.generation
    }

    pub const fn range(&self) -> PhysicalRange {
        self.range
    }

    pub const fn rectangle(&self) -> TargetRectangle {
        self.rectangle
    }

    pub const fn device_bytes(&self) -> &DeviceColorBytes {
        &self.device_bytes
    }

    /// Widens this completion's claimed rectangle to `rectangle`, leaving
    /// every byte untouched.
    ///
    /// The N-command accumulation seam's own need: with several fills and
    /// texrects composing into one buffer, the *last* command's completion
    /// carries the composed bytes but claims only its own sub-rectangle,
    /// while the region the composed buffer actually proves is the union of
    /// every command's. `admit_completed_initialization` reads exactly this
    /// rectangle to decide whether a brand-new target is fully initialized,
    /// so reporting the last command's alone would understate what N proved
    /// and reject a legitimately complete composition.
    ///
    /// Deliberately takes the rectangle rather than computing a union here:
    /// the caller is the only party that saw every command, and a union
    /// recomputed from one completion would have nothing to union with.
    ///
    /// Nonclaim: this asserts nothing about the bytes, and cannot -- the
    /// caller vouches that every pixel in `rectangle` came from a proven
    /// write, exactly as it vouches for the single-command case.
    pub(crate) fn with_claimed_rectangle(self, rectangle: TargetRectangle) -> Self {
        Self { rectangle, ..self }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitializedRegionProof {
    range: PhysicalRange,
    rows: u32,
    /// The exact rectangle this generation proved, which is NOT always the
    /// whole target: a partial fill is admitted (see
    /// `admit_completed_initialization`), and the pixels outside this
    /// rectangle were never written by the command that produced this
    /// generation.
    ///
    /// Retained because `rows` cannot answer the question a reader actually
    /// has. It is a height with no origin, so it can say "two rows are
    /// real" but never *which* two -- and nothing in the crate reads it to
    /// make a decision. This field is what a reader should consult.
    covered: TargetRectangle,
    generation: TargetGeneration,
}

impl InitializedRegionProof {
    pub const fn range(self) -> PhysicalRange {
        self.range
    }

    pub const fn rows(self) -> u32 {
        self.rows
    }

    /// The rectangle this generation actually wrote. Pixels outside it were
    /// not covered by the completion that published this resident.
    pub const fn covered(self) -> TargetRectangle {
        self.covered
    }

    /// Whether this generation's completion covered the whole target.
    pub const fn is_full(self, extent: ColorTargetExtent) -> bool {
        self.covered.is_full(extent)
    }

    pub const fn generation(self) -> TargetGeneration {
        self.generation
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InitializedCandidateColorTarget {
    candidate: CandidateColorTarget,
    proof: InitializedRegionProof,
    /// The exact rectangle the admitted completion claimed.
    ///
    /// Retained alongside `proof`, which keeps only the row count:
    /// composing a second write onto this generation's bytes needs to know
    /// *where* the proven region is, not just how tall it is.
    /// `InitializedRegionProof` is a published shape this file does not
    /// widen for one caller.
    rectangle: TargetRectangle,
    device_bytes: DeviceColorBytes,
}

/// A prevalidated, move-only resident publication which exclusively borrows
/// the registry until it is either dropped or published.
///
/// The raster owner prepares this capability before transferring the guest
/// commit ticket. That makes target publication after a successful guest
/// commit structurally infallible: no competing candidate can change the
/// predecessor, alias set, or capacity while this capability exists.
pub(crate) struct ResidentPublication<'registry> {
    registry: &'registry mut ColorTargetRegistry,
    initialized: InitializedCandidateColorTarget,
}

impl<'registry> ResidentPublication<'registry> {
    pub(crate) fn publish(self) -> &'registry ResidentColorTarget {
        let key = self.initialized.candidate.key;
        let next = ResidentColorTarget {
            key,
            generation: self.initialized.candidate.generation,
            initialized: self.initialized.proof,
            device_bytes: self.initialized.device_bytes,
        };
        if let Some(index) = self
            .registry
            .residents
            .iter()
            .position(|resident| resident.key == key)
        {
            self.registry.residents[index] = next;
            return &self.registry.residents[index];
        }

        let index = self.registry.residents.len();
        self.registry.residents.push(next);
        &self.registry.residents[index]
    }
}

impl InitializedCandidateColorTarget {
    pub const fn key(&self) -> ColorTargetKey {
        self.candidate.key
    }

    pub const fn generation(&self) -> TargetGeneration {
        self.candidate.generation
    }

    pub const fn initialized_region(&self) -> InitializedRegionProof {
        self.proof
    }

    /// The rectangle the admitted completion claimed -- what this
    /// generation's bytes are proven to cover.
    pub const fn initialized_rectangle(&self) -> TargetRectangle {
        self.rectangle
    }

    pub const fn device_bytes(&self) -> &DeviceColorBytes {
        &self.device_bytes
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ResidentColorTarget {
    key: ColorTargetKey,
    generation: TargetGeneration,
    initialized: InitializedRegionProof,
    device_bytes: DeviceColorBytes,
}

impl ResidentColorTarget {
    pub const fn key(&self) -> ColorTargetKey {
        self.key
    }

    pub const fn generation(&self) -> TargetGeneration {
        self.generation
    }

    pub const fn initialized_region(&self) -> InitializedRegionProof {
        self.initialized
    }

    pub const fn device_bytes(&self) -> &DeviceColorBytes {
        &self.device_bytes
    }
}

#[derive(Debug)]
pub struct ColorTargetRegistry {
    layout: PhysicalMemoryLayout,
    capacity: NonZeroUsize,
    residents: Vec<ResidentColorTarget>,
}

impl ColorTargetRegistry {
    pub fn try_new(layout: PhysicalMemoryLayout, capacity: usize) -> Result<Self, TargetError> {
        let capacity = NonZeroUsize::new(capacity).ok_or(TargetError::ZeroRegistryCapacity)?;
        Ok(Self {
            layout,
            capacity,
            residents: Vec::with_capacity(capacity.get()),
        })
    }

    pub const fn layout(&self) -> PhysicalMemoryLayout {
        self.layout
    }

    pub fn residents(&self) -> &[ResidentColorTarget] {
        &self.residents
    }

    pub fn begin_candidate(
        &self,
        key: ColorTargetKey,
    ) -> Result<CandidateColorTarget, TargetError> {
        if key.address.layout() != self.layout {
            return Err(TargetError::MemoryLayoutMismatch {
                registry_bytes: self.layout.bytes(),
                target_bytes: key.address.layout().bytes(),
            });
        }

        if let Some(resident) = self.residents.iter().find(|resident| resident.key == key) {
            return Ok(CandidateColorTarget {
                key,
                generation: resident.generation.successor(key)?,
                predecessor: Some(resident.generation),
            });
        }

        if let Some(resident) = self
            .residents
            .iter()
            .find(|resident| ranges_overlap(resident.key.range, key.range))
        {
            return Err(TargetError::AliasedResidentTarget {
                candidate: key,
                resident: resident.key,
            });
        }
        if self.residents.len() == self.capacity.get() {
            return Err(TargetError::RegistryFull {
                capacity: self.capacity.get(),
                candidate: key,
            });
        }

        Ok(CandidateColorTarget {
            key,
            generation: TargetGeneration::FIRST,
            predecessor: None,
        })
    }

    pub fn commit_initialized(
        &mut self,
        initialized: InitializedCandidateColorTarget,
    ) -> Result<&ResidentColorTarget, TargetError> {
        Ok(self.prepare_publication(initialized)?.publish())
    }

    pub(crate) fn prepare_publication(
        &mut self,
        initialized: InitializedCandidateColorTarget,
    ) -> Result<ResidentPublication<'_>, TargetError> {
        let key = initialized.candidate.key;
        let predecessor = initialized.candidate.predecessor;
        let actual = self
            .residents
            .iter()
            .find(|resident| resident.key == key)
            .map(|resident| resident.generation);
        if actual != predecessor {
            return Err(TargetError::StaleCandidateGeneration {
                key,
                expected_predecessor: predecessor,
                actual_resident: actual,
            });
        }

        if self.residents.iter().any(|resident| resident.key == key) {
            return Ok(ResidentPublication {
                registry: self,
                initialized,
            });
        }

        if let Some(resident) = self
            .residents
            .iter()
            .find(|resident| ranges_overlap(resident.key.range, key.range))
        {
            return Err(TargetError::AliasedResidentTarget {
                candidate: key,
                resident: resident.key,
            });
        }
        if self.residents.len() == self.capacity.get() {
            return Err(TargetError::RegistryFull {
                capacity: self.capacity.get(),
                candidate: key,
            });
        }
        Ok(ResidentPublication {
            registry: self,
            initialized,
        })
    }
}

fn ranges_overlap(left: PhysicalRange, right: PhysicalRange) -> bool {
    left.start().get() < right.end() && right.start().get() < left.end()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetError {
    Address(ValidationError),
    UnsupportedColorTargetFormat {
        format: ImageFormat,
        size: PixelSize,
    },
    ZeroExtent {
        width: u32,
        height: u32,
    },
    ExtentOverflow {
        width: u32,
        height: u32,
    },
    TargetByteLengthOverflow {
        extent: ColorTargetExtent,
        format: ColorTargetFormat,
    },
    TargetAddressOverflow {
        start: u32,
        byte_len: u32,
    },
    ZeroRectangle {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    RectangleCoordinateOverflow {
        axis: &'static str,
        origin: u32,
        length: u32,
    },
    RectangleOutOfBounds {
        key: ColorTargetKey,
        rectangle: TargetRectangle,
    },
    MemoryLayoutMismatch {
        registry_bytes: u32,
        target_bytes: u32,
    },
    ZeroRegistryCapacity,
    RegistryFull {
        capacity: usize,
        candidate: ColorTargetKey,
    },
    AliasedResidentTarget {
        candidate: ColorTargetKey,
        resident: ColorTargetKey,
    },
    GenerationExhausted {
        key: ColorTargetKey,
        current: TargetGeneration,
    },
    InitializationPlanMismatch {
        candidate_key: ColorTargetKey,
        candidate_generation: TargetGeneration,
        plan_key: ColorTargetKey,
        plan_generation: TargetGeneration,
        candidate_range: PhysicalRange,
        plan_range: PhysicalRange,
    },
    CompletedByteDomainMismatch {
        key: ColorTargetKey,
        generation: TargetGeneration,
        byte_key: ColorTargetKey,
        byte_generation: TargetGeneration,
        expected_format: ColorTargetFormat,
        actual_format: ColorTargetFormat,
    },
    CompletedByteLengthMismatch {
        key: ColorTargetKey,
        generation: TargetGeneration,
        expected: usize,
        actual: usize,
    },
    PartialNewTargetInitialization {
        key: ColorTargetKey,
        rectangle: TargetRectangle,
    },
    StaleCandidateGeneration {
        key: ColorTargetKey,
        expected_predecessor: Option<TargetGeneration>,
        actual_resident: Option<TargetGeneration>,
    },
    PixelBufferLengthOverflow {
        pixels: usize,
        bytes_per_pixel: u32,
    },
    PixelCountMismatch {
        key: ColorTargetKey,
        expected: usize,
        actual: usize,
    },
    PixelByteLength {
        format: ColorTargetFormat,
        actual: usize,
        required_multiple: usize,
    },
    DeviceDomainMismatch {
        expected: ColorTargetFormat,
        actual: ColorTargetFormat,
    },
}

impl From<ValidationError> for TargetError {
    fn from(error: ValidationError) -> Self {
        Self::Address(error)
    }
}

impl fmt::Display for TargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Address(error) => write!(formatter, "color-target physical range rejected: {error}"),
            Self::UnsupportedColorTargetFormat { format, size } => write!(
                formatter,
                "unsupported native color-target format {format:?}/{size:?}; M3.3b admits only RGBA16 and RGBA32"
            ),
            Self::ZeroExtent { width, height } => {
                write!(formatter, "color-target extent is empty: {width}x{height}")
            }
            Self::ExtentOverflow { width, height } => write!(
                formatter,
                "color-target pixel count overflows u32: {width}x{height}"
            ),
            Self::TargetByteLengthOverflow { extent, format } => write!(
                formatter,
                "color-target byte length overflows u32: {}x{} {format:?}",
                extent.width, extent.height
            ),
            Self::TargetAddressOverflow { start, byte_len } => write!(
                formatter,
                "color-target range overflows u32: start={start:#010x} bytes={byte_len:#x}"
            ),
            Self::ZeroRectangle { x, y, width, height } => write!(
                formatter,
                "color-target rectangle is empty: origin=({x},{y}) extent={width}x{height}"
            ),
            Self::RectangleCoordinateOverflow { axis, origin, length } => write!(
                formatter,
                "color-target rectangle {axis} coordinate overflows: origin={origin} length={length}"
            ),
            Self::RectangleOutOfBounds { key, rectangle } => write!(
                formatter,
                "color-target rectangle {rectangle:?} is outside {:?} at {:#010x}",
                key.extent,
                key.address.get()
            ),
            Self::MemoryLayoutMismatch { registry_bytes, target_bytes } => write!(
                formatter,
                "color-target memory-layout mismatch: registry={registry_bytes:#x} target={target_bytes:#x}"
            ),
            Self::ZeroRegistryCapacity => formatter.write_str("color-target registry capacity is zero"),
            Self::RegistryFull { capacity, candidate } => write!(
                formatter,
                "color-target registry is full at {capacity} residents; candidate={candidate:?}"
            ),
            Self::AliasedResidentTarget { candidate, resident } => write!(
                formatter,
                "color-target candidate {candidate:?} aliases incompatible resident {resident:?}"
            ),
            Self::GenerationExhausted { key, current } => write!(
                formatter,
                "color-target generation exhausted for {key:?} at {}",
                current.get()
            ),
            Self::InitializationPlanMismatch { candidate_key, candidate_generation, plan_key, plan_generation, candidate_range, plan_range } => write!(
                formatter,
                "color-target completion does not belong to candidate: candidate={candidate_key:?}/{}/{candidate_range:?} completion={plan_key:?}/{}/{plan_range:?}",
                candidate_generation.get(),
                plan_generation.get()
            ),
            Self::CompletedByteDomainMismatch { key, generation, byte_key, byte_generation, expected_format, actual_format } => write!(
                formatter,
                "completed color bytes do not belong to target {key:?}/{}: bytes={byte_key:?}/{} expected_format={expected_format:?} actual_format={actual_format:?}",
                generation.get(),
                byte_generation.get()
            ),
            Self::CompletedByteLengthMismatch { key, generation, expected, actual } => write!(
                formatter,
                "completed color bytes for {key:?}/{} have length {actual}; exact target requires {expected}",
                generation.get()
            ),
            Self::PartialNewTargetInitialization { key, rectangle } => write!(
                formatter,
                "new color target {key:?} cannot become resident from partial initialization {rectangle:?}"
            ),
            Self::StaleCandidateGeneration { key, expected_predecessor, actual_resident } => write!(
                formatter,
                "stale color-target candidate for {key:?}: expected predecessor {expected_predecessor:?}, resident is {actual_resident:?}"
            ),
            Self::PixelBufferLengthOverflow { pixels, bytes_per_pixel } => write!(
                formatter,
                "device color buffer length overflows usize: pixels={pixels} bytes_per_pixel={bytes_per_pixel}"
            ),
            Self::PixelCountMismatch { key, expected, actual } => write!(
                formatter,
                "device color pixel count for {key:?} is {actual}; exact target requires {expected}"
            ),
            Self::PixelByteLength { format, actual, required_multiple } => write!(
                formatter,
                "device {format:?} byte length {actual} is not a multiple of {required_multiple}"
            ),
            Self::DeviceDomainMismatch { expected, actual } => write!(
                formatter,
                "device color-byte domain mismatch: expected {expected:?}, actual {actual:?}"
            ),
        }
    }
}

impl std::error::Error for TargetError {}

#[cfg(test)]
mod tests;
