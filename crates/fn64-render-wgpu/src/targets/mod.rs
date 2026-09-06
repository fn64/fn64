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
use std::sync::Arc;

use fn64_render_ir::{
    CompletedWrite, PhysicalAddress, PhysicalMemoryLayout, PhysicalRange, ResourceAccess,
    ResourceRegion, ValidationError,
};

use crate::{ColorImage, ImageFormat, PixelSize};

mod compute_batch;
mod fill;
mod hidden_coverage;
mod oracle;
mod raster;
mod raw_triangle;
mod texrect;
mod triangle_pipeline;

pub(crate) use compute_batch::{
    ComputeRasterAdmissionRefusal, ComputeRasterBatch, ComputeRasterBatchBuilder,
    ComputeRasterDrawAdmission, ComputeRasterProgramKey,
};
#[cfg(all(test, feature = "host-gpu-tests"))]
pub(crate) use compute_batch::{
    HOT_COMBINE_HIGH, HOT_COMBINE_LOW, HOT_OTHER_MODE_HIGH, HOT_OTHER_MODE_LOW,
};
pub use fill::{
    decode_fill_cycle_pixel, execute_combined_fill_rectangle, execute_fill_rectangle,
    resolve_fill_pixel_rectangle, FillCoordinateError, FillCycleBypassHazards, FillExecutionError,
    FillPixelRectangle,
};
pub(crate) use fill::{execute_combined_fill_rectangle_owned, execute_fill_rectangle_owned};
pub(crate) use hidden_coverage::{
    ColorCoverageState, HiddenCoveragePublication, RdramHiddenCoverage,
};
pub use oracle::{pack_device_pixels, unpack_device_pixels, DeviceColorBytes, Rgba8};
pub use raster::{
    CommittedNativeRasterFrame, InFlightNativeRasterFill, NativeRasterDeviceOutcome,
    NativeRasterError, NativeRasterRenderer, PendingNativeRasterCommit, UninitializedNativeRaster,
};
pub(crate) use raw_triangle::{
    execute_prepared_raw_triangle_row_bin_prefix, execute_raw_triangle_with_coverage,
    PreparedRawTriangleRaster,
};
pub use raw_triangle::{execute_raw_triangle, DepthCell, RawTriangleDepth, RawTriangleTexture};
pub use texrect::{
    execute_texture_rectangle, RdpScissorRect, TexrectAxis, TexrectBlendRegisters,
    TexrectConstantRegister, TexrectDraw, TexrectExecutionError, TexrectShading,
    TexrectTileBinding,
};
pub(crate) use triangle_pipeline::{
    admitted_triangle_fixture, ComputeHotColorBatch, ComputeHotColorDispatch,
    ResolvedFragmentBlendParams,
};
pub use triangle_pipeline::{
    fixed_fixture_other_mode, ComputeCoverageTriangle, ComputeRasterSample, InFlightTriangleDraw,
    RasterVertex, TriangleDrawOutput, TriangleFixture, TrianglePipelineDeviceOutcome,
    TrianglePipelineError, TrianglePipelineRenderer, TriangleRasterParams, TriangleTargetExtent,
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

/// Exclusive mutable view of a contiguous, exact row band of one color target.
pub(crate) struct ExactColorRowBandMut<'a> {
    key: ColorTargetKey,
    rows: std::ops::Range<u32>,
    bytes: &'a mut [u8],
    coverage: &'a mut [u8],
}

impl<'a> ExactColorRowBandMut<'a> {
    pub(crate) fn from_full(
        key: ColorTargetKey,
        rows: std::ops::Range<u32>,
        bytes: &'a mut [u8],
        coverage: &'a mut ColorCoverageState,
    ) -> Self {
        let height = key.extent().height();
        assert!(
            rows.start <= rows.end && rows.end <= height,
            "exact color row band {:?} lies outside target height {height}",
            rows
        );
        let start = rows.start;
        let end = rows.end;
        let width = key.extent().width() as usize;
        let row_bytes = width * key.format().bytes_per_pixel() as usize;
        assert_eq!(bytes.len(), key.range().len() as usize);
        let coverage = &mut coverage.cells_mut()[start as usize * width..end as usize * width];
        Self {
            key,
            rows: start..end,
            bytes: &mut bytes[start as usize * row_bytes..end as usize * row_bytes],
            coverage,
        }
    }

    pub(crate) fn from_exact_parts(
        key: ColorTargetKey,
        rows: std::ops::Range<u32>,
        bytes: &'a mut [u8],
        coverage: &'a mut [u8],
    ) -> Self {
        assert!(rows.start <= rows.end && rows.end <= key.extent().height());
        let expected = (rows.end - rows.start) as usize * key.extent().width() as usize;
        assert_eq!(coverage.len(), expected);
        assert!(coverage.iter().all(|count| (1..=8).contains(count)));
        assert_eq!(
            bytes.len(),
            expected * key.format().bytes_per_pixel() as usize
        );
        Self {
            key,
            rows,
            bytes,
            coverage,
        }
    }

    pub(crate) fn rows(&self) -> &std::ops::Range<u32> {
        &self.rows
    }

    pub(crate) const fn key(&self) -> ColorTargetKey {
        self.key
    }

    pub(crate) fn parts_mut(&mut self) -> (&mut [u8], &mut [u8], u32) {
        (self.bytes, self.coverage, self.rows.start)
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
            coverage: completed.coverage,
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
    coverage: ColorCoverageState,
}

impl CompletedColorTargetWrite {
    /// The M4.3.4 fill executor's own production constructor. `device_bytes`
    /// must already be the target's full-extent byte content (enforced by
    /// [`DeviceColorBytes::new_for_fill`]); `rectangle` records the actual
    /// sub-region the executor wrote, for [`InitializedRegionProof`].
    pub(crate) fn new_for_fill(
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
            coverage: ColorCoverageState::unknown(key.extent()),
        }
    }

    pub(crate) fn with_coverage(mut self, coverage: ColorCoverageState) -> Self {
        assert_eq!(coverage.len(), self.key.extent().pixels() as usize);
        self.coverage = coverage;
        self
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

    pub(crate) const fn coverage(&self) -> &ColorCoverageState {
        &self.coverage
    }

    /// Transfers the already-validated full-target bytes to the next
    /// scheduled color command. An intermediate completion has no authority
    /// consumer of its own; only the schedule's final completion is admitted
    /// for publication, so retaining this buffer's sole owner avoids a
    /// duplicate full-target copy without widening construction authority.
    pub(crate) fn into_device_color_bytes(self) -> DeviceColorBytes {
        self.device_bytes
    }

    pub(crate) fn into_task_accumulator(self) -> (Vec<u8>, ColorCoverageState) {
        (
            self.device_bytes.into_device_bytes().into_vec(),
            self.coverage,
        )
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
    coverage: ColorCoverageState,
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
    hidden: HiddenCoveragePublication,
}

impl<'registry> ResidentPublication<'registry> {
    pub(crate) fn publish(self) -> &'registry ResidentColorTarget {
        let key = self.initialized.candidate.key;
        self.hidden
            .apply(Arc::make_mut(&mut self.registry.hidden_coverage));
        let next = ResidentColorTarget {
            key,
            generation: self.initialized.candidate.generation,
            initialized: self.initialized.proof,
            device_bytes: self.initialized.device_bytes,
            coverage: self.initialized.coverage,
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

    /// Captures exactly the journal-declared byte payloads of this completed
    /// generation while leaving its one full-target image available to the
    /// ordered task accumulator. The sparse value is independently move-only:
    /// publication can advance this generation without retaining a second
    /// full framebuffer image.
    pub(crate) fn sparse_checkpoint(
        &self,
        writes: &[CompletedWrite],
    ) -> Result<SparseInitializedColorCheckpoint, TargetError> {
        let key = self.key();
        let base = key.address().get();
        let target_end = key.range().end();
        let mut patches = Vec::with_capacity(writes.len());
        for write in writes {
            let ResourceRegion::Rdram { range, .. } = write.access().region() else {
                return Err(TargetError::SparseCheckpointNonRdramWrite {
                    operation: write.access().operation().get(),
                });
            };
            if range.start().get() < base || range.end() > target_end {
                return Err(TargetError::SparseCheckpointRangeOutsideTarget { key, range });
            }
            let start = (range.start().get() - base) as usize;
            let end = start + range.len() as usize;
            let bytes: Arc<[u8]> = self
                .device_bytes
                .device_bytes()
                .get(start..end)
                .expect("range containment proves sparse checkpoint slice bounds")
                .into();
            let coverage = self
                .coverage
                .patch_for_byte_range(key, start, range.len() as usize);
            let rebound = CompletedWrite::try_from_bytes(write.access(), &bytes)
                .map_err(TargetError::Address)?;
            if rebound != *write {
                return Err(TargetError::SparseCheckpointDigestMismatch {
                    operation: write.access().operation().get(),
                });
            }
            patches.push(SparseColorPatch {
                write: *write,
                bytes,
                coverage,
            });
        }
        let hidden = HiddenCoveragePublication::try_from_fragments(
            key,
            patches.iter().map(|patch| {
                let ResourceRegion::Rdram { range, .. } = patch.write.access().region() else {
                    unreachable!("sparse checkpoint construction accepts only RDRAM writes")
                };
                (
                    (range.start().get() - base) as usize,
                    patch.bytes.as_ref(),
                    patch.coverage.as_ref(),
                )
            }),
        )?;
        Ok(SparseInitializedColorCheckpoint {
            key,
            generation: self.generation(),
            predecessor: self.candidate.predecessor,
            proof: self.proof,
            patches: patches.into_boxed_slice(),
            hidden,
        })
    }

    /// Materializes one journal's exact payloads, write commitments, visible
    /// coverage, and hidden-coverage publication from the final accumulator
    /// in one pass. The returned checkpoint and writes share the same derived
    /// facts; no caller needs to hash the full-target slices before copying
    /// and hashing those slices again for sparse publication.
    pub(crate) fn sparse_checkpoint_from_accesses(
        &self,
        accesses: &[ResourceAccess],
    ) -> Result<(SparseInitializedColorCheckpoint, Vec<CompletedWrite>), TargetError> {
        let key = self.key();
        let base = key.address().get();
        let target_end = key.range().end();
        let mut patches = Vec::with_capacity(accesses.len());
        for access in accesses.iter().copied() {
            let ResourceRegion::Rdram { range, .. } = access.region() else {
                return Err(TargetError::SparseCheckpointNonRdramWrite {
                    operation: access.operation().get(),
                });
            };
            if range.start().get() < base || range.end() > target_end {
                return Err(TargetError::SparseCheckpointRangeOutsideTarget { key, range });
            }
            let start = (range.start().get() - base) as usize;
            let end = start + range.len() as usize;
            let bytes: Arc<[u8]> = self
                .device_bytes
                .device_bytes()
                .get(start..end)
                .expect("range containment proves sparse checkpoint slice bounds")
                .into();
            let coverage = self
                .coverage
                .patch_for_byte_range(key, start, range.len() as usize);
            let write =
                CompletedWrite::try_from_bytes(access, &bytes).map_err(TargetError::Address)?;
            patches.push(SparseColorPatch {
                write,
                bytes,
                coverage,
            });
        }
        let hidden = HiddenCoveragePublication::try_from_fragments(
            key,
            patches.iter().map(|patch| {
                let ResourceRegion::Rdram { range, .. } = patch.write.access().region() else {
                    unreachable!("sparse checkpoint construction accepts only RDRAM writes")
                };
                (
                    (range.start().get() - base) as usize,
                    patch.bytes.as_ref(),
                    patch.coverage.as_ref(),
                )
            }),
        )?;
        let writes = patches.iter().map(|patch| patch.write).collect();
        Ok((
            SparseInitializedColorCheckpoint {
                key,
                generation: self.generation(),
                predecessor: self.candidate.predecessor,
                proof: self.proof,
                patches: patches.into_boxed_slice(),
                hidden,
            },
            writes,
        ))
    }

    pub(crate) fn into_task_accumulator(self) -> (Vec<u8>, ColorCoverageState) {
        (
            self.device_bytes.into_device_bytes().into_vec(),
            self.coverage,
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
struct SparseColorPatch {
    write: CompletedWrite,
    bytes: Arc<[u8]>,
    coverage: Box<[u8]>,
}

/// Journal-bound publication authority for one task-private color
/// generation. It owns only the byte runs the packet declared, never the
/// full target image used to execute the next packet.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SparseInitializedColorCheckpoint {
    key: ColorTargetKey,
    generation: TargetGeneration,
    predecessor: Option<TargetGeneration>,
    proof: InitializedRegionProof,
    patches: Box<[SparseColorPatch]>,
    hidden: HiddenCoveragePublication,
}

impl SparseInitializedColorCheckpoint {
    pub(crate) const fn key(&self) -> ColorTargetKey {
        self.key
    }

    /// Seals one row-bin member's caller-owned visible and hidden-coverage
    /// patches into journal-bound publication authority. The executor's
    /// vectors are not capabilities: exact cardinality, access identity and
    /// order, byte digests, target containment, coverage, and the claimed
    /// rectangle are all revalidated here before a checkpoint exists.
    pub(crate) fn from_row_bin_execution(
        candidate: &CandidateColorTarget,
        rectangle: TargetRectangle,
        executed: Vec<raw_triangle::PreparedRawTriangleCheckpointPatch>,
        expected: &[ResourceAccess],
    ) -> Result<(Self, Vec<CompletedWrite>), TargetError> {
        if executed.len() != expected.len() {
            return Err(TargetError::SparseCheckpointPatchCountMismatch {
                expected: expected.len(),
                actual: executed.len(),
            });
        }
        if expected.is_empty() {
            return Err(TargetError::SparseCheckpointEmpty);
        }
        let key = candidate.key();
        if rectangle.x() + rectangle.width() > key.extent().width()
            || rectangle.y() + rectangle.height() > key.extent().height()
        {
            return Err(TargetError::RectangleOutOfBounds { key, rectangle });
        }

        let base = key.address().get();
        let target_end = key.range().end();
        let bytes_per_pixel = key.format().bytes_per_pixel();
        let target_width = key.extent().width();
        let mut derived_rectangle: Option<TargetRectangle> = None;
        let mut writes = Vec::with_capacity(expected.len());
        let mut patches = Vec::with_capacity(expected.len());
        for (position, (executed, expected)) in executed
            .into_iter()
            .zip(expected.iter().copied())
            .enumerate()
        {
            if executed.access != expected {
                return Err(TargetError::SparseCheckpointAccessMismatch {
                    position,
                    expected,
                    actual: executed.access,
                });
            }
            let ResourceRegion::Rdram { range, .. } = expected.region() else {
                return Err(TargetError::SparseCheckpointNonRdramWrite {
                    operation: expected.operation().get(),
                });
            };
            if range.start().get() < base || range.end() > target_end {
                return Err(TargetError::SparseCheckpointRangeOutsideTarget { key, range });
            }
            let offset = range.start().get() - base;
            if range.len() == 0
                || offset % bytes_per_pixel != 0
                || range.len() % bytes_per_pixel != 0
            {
                return Err(TargetError::SparseCheckpointAccessNotRow { key, range });
            }
            let first_pixel = offset / bytes_per_pixel;
            let x = first_pixel % target_width;
            let y = first_pixel / target_width;
            let width = range.len() / bytes_per_pixel;
            if x + width > target_width {
                return Err(TargetError::SparseCheckpointAccessNotRow { key, range });
            }
            let row = TargetRectangle::try_new(x, y, width, 1)?;
            derived_rectangle = Some(match derived_rectangle {
                None => row,
                Some(prior) => {
                    let left = prior.x().min(row.x());
                    let top = prior.y().min(row.y());
                    let right = (prior.x() + prior.width()).max(row.x() + row.width());
                    let bottom = (prior.y() + prior.height()).max(row.y() + row.height());
                    TargetRectangle::try_new(left, top, right - left, bottom - top)?
                }
            });
            let write = CompletedWrite::try_from_bytes(expected, &executed.bytes)
                .map_err(TargetError::Address)?;
            writes.push(write);
            patches.push(SparseColorPatch {
                write,
                bytes: executed.bytes.into(),
                coverage: executed.coverage.into_boxed_slice(),
            });
        }
        let derived_rectangle = derived_rectangle.expect("nonempty expected writes derive a row");
        if rectangle != derived_rectangle {
            return Err(TargetError::SparseCheckpointClaimedRectangleMismatch {
                expected: derived_rectangle,
                actual: rectangle,
            });
        }
        let hidden = HiddenCoveragePublication::try_from_fragments(
            key,
            patches.iter().map(|patch| {
                let ResourceRegion::Rdram { range, .. } = patch.write.access().region() else {
                    unreachable!("validated row-bin checkpoints contain only RDRAM writes")
                };
                (
                    (range.start().get() - base) as usize,
                    patch.bytes.as_ref(),
                    patch.coverage.as_ref(),
                )
            }),
        )?;
        Ok((
            Self {
                key,
                generation: candidate.generation(),
                predecessor: candidate.predecessor(),
                proof: InitializedRegionProof {
                    range: key.range(),
                    rows: rectangle.height(),
                    covered: rectangle,
                    generation: candidate.generation(),
                },
                patches: patches.into_boxed_slice(),
                hidden,
            },
            writes,
        ))
    }

    pub(crate) fn payloads(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.patches.iter().map(|patch| patch.bytes.as_ref())
    }

    pub(crate) fn shared_payloads(&self) -> impl ExactSizeIterator<Item = Arc<[u8]>> + '_ {
        self.patches.iter().map(|patch| Arc::clone(&patch.bytes))
    }

    #[cfg(test)]
    fn writes(&self) -> impl ExactSizeIterator<Item = CompletedWrite> + '_ {
        self.patches.iter().map(|patch| patch.write)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidentColorTarget {
    key: ColorTargetKey,
    generation: TargetGeneration,
    initialized: InitializedRegionProof,
    device_bytes: DeviceColorBytes,
    coverage: ColorCoverageState,
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

    pub(crate) const fn coverage(&self) -> &ColorCoverageState {
        &self.coverage
    }

    pub(crate) fn visual_snapshot(
        &self,
        submission: fn64_render_ir::SubmissionIdentity,
    ) -> Result<
        fn64_render::RawDpcVisualTargetSnapshotV1,
        fn64_render::RawDpcVisualTargetSnapshotRefusal,
    > {
        let format = match self.key.format() {
            ColorTargetFormat::Rgba16 => fn64_render::RawDpcVisualTargetFormatV1::Rgba16,
            ColorTargetFormat::Rgba32 => fn64_render::RawDpcVisualTargetFormatV1::Rgba32,
        };
        fn64_render::RawDpcVisualTargetSnapshotV1::try_new(
            submission,
            self.key.address().get(),
            self.key.extent().width(),
            self.key.extent().height(),
            format,
            self.device_bytes.device_bytes().to_vec(),
            self.coverage.cells().to_vec(),
        )
    }
}

#[derive(Clone, Debug)]
pub struct ColorTargetRegistry {
    layout: PhysicalMemoryLayout,
    capacity: NonZeroUsize,
    residents: Vec<ResidentColorTarget>,
    hidden_coverage: Arc<RdramHiddenCoverage>,
}

/// Private generation planner for one ordered renderer transaction.
///
/// Reserving a candidate does not initialize bytes and does not mutate the
/// durable registry. It only establishes the predecessor chain that later
/// completed GPU checkpoints must redeem in the same order. This is the
/// color-target counterpart of `RawDpcExecutionBatch`: later task members
/// can be planned before an earlier member's device result has been read
/// back, without publishing a placeholder resident.
pub(crate) struct ColorTargetExecutionBatch {
    reserved: Vec<ReservedColorTargetGeneration>,
}

#[derive(Clone, Copy)]
struct ReservedColorTargetGeneration {
    key: ColorTargetKey,
    generation: TargetGeneration,
    predecessor: Option<TargetGeneration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskColorInput {
    DurableRegistry,
    PriorTaskCheckpoint,
}

/// Non-empty proof that a task's completed color checkpoints form one exact
/// generation chain. The proof retains only the chain head's predecessor and
/// the final checkpoint: intermediate framebuffer bytes have no remaining
/// consumer once their successor has completed.
pub(crate) struct CompletedTaskColorSegment<'a> {
    key: ColorTargetKey,
    first_predecessor: Option<TargetGeneration>,
    final_checkpoint: &'a InitializedCandidateColorTarget,
}

/// Move-only final image for a task-private CPU segment. Unlike the borrowed
/// compute-segment proof, this transfers the accumulator into the private
/// registry at a hard execution boundary without cloning it.
pub(crate) struct OwnedTaskColorSegment {
    first_predecessor: Option<TargetGeneration>,
    final_checkpoint: InitializedCandidateColorTarget,
}

enum PreparedOwnedTaskShadowSlot {
    Replace {
        index: usize,
        expected_generation: TargetGeneration,
    },
    Append {
        expected_len: usize,
    },
}

/// Infallible install authority for one already-validated task shadow.
/// Construction closes stale-generation, alias, capacity, and payload
/// ownership failures before an enclosing transaction redeems any external
/// coordinator or guest-publication authority.
pub(crate) struct PreparedOwnedTaskShadowInstall {
    slot: PreparedOwnedTaskShadowSlot,
    next: ResidentColorTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OrderedCpuCandidateReservation {
    key: ColorTargetKey,
    generation: TargetGeneration,
    predecessor: Option<TargetGeneration>,
}

impl OrderedCpuCandidateReservation {
    pub(crate) fn new(candidate: &CandidateColorTarget) -> Self {
        Self {
            key: candidate.key,
            generation: candidate.generation,
            predecessor: candidate.predecessor,
        }
    }

    pub(crate) fn validate(
        self,
        initialized: &InitializedCandidateColorTarget,
    ) -> Result<(), TargetError> {
        if initialized.candidate.key != self.key
            || initialized.candidate.generation != self.generation
            || initialized.candidate.predecessor != self.predecessor
        {
            return Err(TargetError::OrderedCpuCandidateMismatch {
                expected_key: self.key,
                actual_key: initialized.candidate.key,
                expected_generation: self.generation,
                actual_generation: initialized.candidate.generation,
                expected_predecessor: self.predecessor,
                actual_predecessor: initialized.candidate.predecessor,
            });
        }
        Ok(())
    }
}

pub(crate) struct OrderedCpuColorContinuity {
    key: ColorTargetKey,
    first_predecessor: Option<TargetGeneration>,
    tail_generation: TargetGeneration,
    tail_predecessor: Option<TargetGeneration>,
}

impl OrderedCpuColorContinuity {
    pub(crate) fn start_reserved(reservation: OrderedCpuCandidateReservation) -> Self {
        Self {
            key: reservation.key,
            first_predecessor: reservation.predecessor,
            tail_generation: reservation.generation,
            tail_predecessor: reservation.predecessor,
        }
    }

    pub(crate) fn append_reserved(
        mut self,
        reservation: OrderedCpuCandidateReservation,
    ) -> Result<Self, TargetError> {
        let expected_predecessor = Some(self.tail_generation);
        if reservation.key != self.key || reservation.predecessor != expected_predecessor {
            return Err(TargetError::DiscontinuousTaskColorSegment {
                expected_key: self.key,
                actual_key: reservation.key,
                expected_predecessor,
                actual_predecessor: reservation.predecessor,
            });
        }
        self.tail_generation = reservation.generation;
        self.tail_predecessor = reservation.predecessor;
        Ok(self)
    }

    pub(crate) fn start(
        reservation: OrderedCpuCandidateReservation,
        initialized: &InitializedCandidateColorTarget,
    ) -> Result<Self, TargetError> {
        reservation.validate(initialized)?;
        Ok(Self {
            key: initialized.candidate.key,
            first_predecessor: initialized.candidate.predecessor,
            tail_generation: initialized.candidate.generation,
            tail_predecessor: initialized.candidate.predecessor,
        })
    }

    pub(crate) fn append(
        mut self,
        reservation: OrderedCpuCandidateReservation,
        initialized: &InitializedCandidateColorTarget,
    ) -> Result<Self, TargetError> {
        reservation.validate(initialized)?;
        let expected_predecessor = Some(self.tail_generation);
        if initialized.candidate.key != self.key
            || initialized.candidate.predecessor != expected_predecessor
        {
            return Err(TargetError::DiscontinuousTaskColorSegment {
                expected_key: self.key,
                actual_key: initialized.candidate.key,
                expected_predecessor,
                actual_predecessor: initialized.candidate.predecessor,
            });
        }
        self.tail_generation = initialized.candidate.generation;
        self.tail_predecessor = initialized.candidate.predecessor;
        Ok(self)
    }

    pub(crate) fn finish(
        self,
        final_checkpoint: InitializedCandidateColorTarget,
    ) -> Result<OwnedTaskColorSegment, TargetError> {
        if final_checkpoint.candidate.key != self.key
            || final_checkpoint.candidate.generation != self.tail_generation
            || final_checkpoint.candidate.predecessor != self.tail_predecessor
        {
            return Err(TargetError::OrderedCpuCandidateMismatch {
                expected_key: self.key,
                actual_key: final_checkpoint.candidate.key,
                expected_generation: self.tail_generation,
                actual_generation: final_checkpoint.candidate.generation,
                expected_predecessor: self.tail_predecessor,
                actual_predecessor: final_checkpoint.candidate.predecessor,
            });
        }
        Ok(OwnedTaskColorSegment {
            first_predecessor: self.first_predecessor,
            final_checkpoint,
        })
    }
}

impl<'a> CompletedTaskColorSegment<'a> {
    pub(crate) fn new(first: &'a InitializedCandidateColorTarget) -> Self {
        Self {
            key: first.candidate.key,
            first_predecessor: first.candidate.predecessor,
            final_checkpoint: first,
        }
    }

    pub(crate) fn append(
        &mut self,
        next: &'a InitializedCandidateColorTarget,
    ) -> Result<(), TargetError> {
        let expected_predecessor = Some(self.final_checkpoint.candidate.generation);
        if next.candidate.key != self.key || next.candidate.predecessor != expected_predecessor {
            return Err(TargetError::DiscontinuousTaskColorSegment {
                expected_key: self.key,
                actual_key: next.candidate.key,
                expected_predecessor,
                actual_predecessor: next.candidate.predecessor,
            });
        }
        self.final_checkpoint = next;
        Ok(())
    }
}

impl ColorTargetExecutionBatch {
    pub(crate) fn new() -> Self {
        Self {
            reserved: Vec::new(),
        }
    }

    pub(crate) fn begin_candidate(
        &mut self,
        registry: &ColorTargetRegistry,
        key: ColorTargetKey,
    ) -> Result<(CandidateColorTarget, TaskColorInput), TargetError> {
        let (candidate, input) = self.preview_candidate(registry, key)?;
        self.reserved.push(ReservedColorTargetGeneration {
            key: candidate.key,
            generation: candidate.generation,
            predecessor: candidate.predecessor,
        });
        Ok((candidate, input))
    }

    /// Derives the next task-private generation without reserving it.
    ///
    /// Deferred compute admission uses this value before it decides whether
    /// the member belongs to a device segment. A rejected member can therefore
    /// take the ordered CPU path without leaving a generation reservation that
    /// only a compute completion could redeem.
    pub(crate) fn preview_candidate(
        &self,
        registry: &ColorTargetRegistry,
        key: ColorTargetKey,
    ) -> Result<(CandidateColorTarget, TaskColorInput), TargetError> {
        if let Some(previous) = self
            .reserved
            .iter()
            .rev()
            .find(|candidate| candidate.key == key)
        {
            let candidate = CandidateColorTarget {
                key,
                generation: previous.generation.successor(key)?,
                predecessor: Some(previous.generation),
            };
            return Ok((candidate, TaskColorInput::PriorTaskCheckpoint));
        }

        if let Some(previous) = self.reserved.iter().find(|candidate| {
            candidate.key != key && ranges_overlap(candidate.key.range, key.range)
        }) {
            return Err(TargetError::AliasedResidentTarget {
                candidate: key,
                resident: previous.key,
            });
        }

        let candidate = registry.begin_candidate(key)?;
        if candidate.predecessor.is_none() {
            let mut new_keys = Vec::new();
            for reserved in &self.reserved {
                if reserved.predecessor.is_none() && !new_keys.contains(&reserved.key) {
                    new_keys.push(reserved.key);
                }
            }
            if registry.residents.len() + new_keys.len() == registry.capacity.get() {
                return Err(TargetError::RegistryFull {
                    capacity: registry.capacity.get(),
                    candidate: key,
                });
            }
        }
        Ok((candidate, TaskColorInput::DurableRegistry))
    }
}

impl ColorTargetRegistry {
    pub fn try_new(layout: PhysicalMemoryLayout, capacity: usize) -> Result<Self, TargetError> {
        let capacity = NonZeroUsize::new(capacity).ok_or(TargetError::ZeroRegistryCapacity)?;
        Ok(Self {
            layout,
            capacity,
            residents: Vec::with_capacity(capacity.get()),
            hidden_coverage: Arc::new(RdramHiddenCoverage::new(layout)),
        })
    }

    pub const fn layout(&self) -> PhysicalMemoryLayout {
        self.layout
    }

    pub(crate) fn project_coverage(&self, key: ColorTargetKey, bytes: &[u8]) -> ColorCoverageState {
        self.hidden_coverage.project(key, bytes)
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

    /// Installs the final task-private view of a completed compute segment
    /// without duplicating any intermediate framebuffer. The segment proof
    /// validates every generation edge before this method validates its head
    /// against the registry. Publication authority remains move-only in each
    /// member's pending token.
    pub(crate) fn commit_task_shadow_segment(
        &mut self,
        segment: CompletedTaskColorSegment<'_>,
    ) -> Result<&ResidentColorTarget, TargetError> {
        let key = segment.key;
        let predecessor = segment.first_predecessor;
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
        let initialized = segment.final_checkpoint;
        let next = ResidentColorTarget {
            key,
            generation: initialized.candidate.generation,
            initialized: initialized.proof,
            device_bytes: initialized.device_bytes.clone(),
            coverage: initialized.coverage.clone(),
        };
        if let Some(index) = self
            .residents
            .iter()
            .position(|resident| resident.key == key)
        {
            self.residents[index] = next;
            return Ok(&self.residents[index]);
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
        self.residents.push(next);
        let index = self.residents.len() - 1;
        Ok(&self.residents[index])
    }

    /// Installs an ordered CPU segment's final accumulator by move. Every
    /// intermediate generation remains represented by its sparse publication
    /// capability; only the final private execution view belongs here.
    pub(crate) fn commit_owned_task_shadow_segment(
        &mut self,
        segment: OwnedTaskColorSegment,
    ) -> Result<&ResidentColorTarget, TargetError> {
        let prepared = self.prepare_owned_task_shadow_install(segment)?;
        Ok(self.install_prepared_owned_task_shadow(prepared))
    }

    pub(crate) fn prepare_owned_task_shadow_install(
        &self,
        segment: OwnedTaskColorSegment,
    ) -> Result<PreparedOwnedTaskShadowInstall, TargetError> {
        let initialized = segment.final_checkpoint;
        let key = initialized.candidate.key;
        let actual = self
            .residents
            .iter()
            .find(|resident| resident.key == key)
            .map(|resident| resident.generation);
        if actual != segment.first_predecessor {
            return Err(TargetError::StaleCandidateGeneration {
                key,
                expected_predecessor: segment.first_predecessor,
                actual_resident: actual,
            });
        }
        let next = ResidentColorTarget {
            key,
            generation: initialized.candidate.generation,
            initialized: initialized.proof,
            device_bytes: initialized.device_bytes,
            coverage: initialized.coverage,
        };
        if let Some(index) = self
            .residents
            .iter()
            .position(|resident| resident.key == key)
        {
            return Ok(PreparedOwnedTaskShadowInstall {
                slot: PreparedOwnedTaskShadowSlot::Replace {
                    index,
                    expected_generation: self.residents[index].generation,
                },
                next,
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
        Ok(PreparedOwnedTaskShadowInstall {
            slot: PreparedOwnedTaskShadowSlot::Append {
                expected_len: self.residents.len(),
            },
            next,
        })
    }

    pub(crate) fn install_prepared_owned_task_shadow(
        &mut self,
        prepared: PreparedOwnedTaskShadowInstall,
    ) -> &ResidentColorTarget {
        match prepared.slot {
            PreparedOwnedTaskShadowSlot::Replace {
                index,
                expected_generation,
            } => {
                assert_eq!(
                    self.residents
                        .get(index)
                        .map(|resident| resident.generation),
                    Some(expected_generation),
                    "prepared task-shadow replacement requires the preflighted live registry"
                );
                self.residents[index] = prepared.next;
                &self.residents[index]
            }
            PreparedOwnedTaskShadowSlot::Append { expected_len } => {
                assert_eq!(
                    self.residents.len(),
                    expected_len,
                    "prepared task-shadow append requires the preflighted live registry"
                );
                assert!(
                    self.residents.len() < self.capacity.get()
                        && self.residents.iter().all(|resident| {
                            !ranges_overlap(resident.key.range, prepared.next.key.range)
                        }),
                    "prepared task-shadow append preflight remains infallible"
                );
                self.residents.push(prepared.next);
                &self.residents[expected_len]
            }
        }
    }

    /// Applies one sparse packet checkpoint after that packet's guest commit.
    /// A first-generation partial checkpoint starts from explicit zeroes,
    /// matching the existing full-image executor's named uncovered-byte
    /// semantics; a successor patches the exact prior resident bytes.
    pub(crate) fn commit_sparse_checkpoint(
        &mut self,
        checkpoint: SparseInitializedColorCheckpoint,
    ) -> Result<&ResidentColorTarget, TargetError> {
        let key = checkpoint.key;
        let index = self
            .residents
            .iter()
            .position(|resident| resident.key == key);
        let actual = index.map(|index| self.residents[index].generation);
        if actual != checkpoint.predecessor {
            return Err(TargetError::StaleCandidateGeneration {
                key,
                expected_predecessor: checkpoint.predecessor,
                actual_resident: actual,
            });
        }
        if index.is_none() {
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
        }

        let mut bytes = match index {
            Some(index) => {
                core::mem::take(&mut self.residents[index].device_bytes.bytes).into_vec()
            }
            None => vec![0; key.range().len() as usize],
        };
        let mut coverage = match index {
            Some(index) => core::mem::replace(
                &mut self.residents[index].coverage,
                ColorCoverageState::unknown(key.extent()),
            ),
            None => ColorCoverageState::unknown(key.extent()),
        };
        let base = key.address().get();
        for patch in &checkpoint.patches {
            let ResourceRegion::Rdram { range, .. } = patch.write.access().region() else {
                unreachable!("sparse checkpoint construction accepts only RDRAM writes")
            };
            let start = (range.start().get() - base) as usize;
            let end = start + patch.bytes.len();
            bytes[start..end].copy_from_slice(&patch.bytes);
            coverage.copy_patch(
                start / key.format().bytes_per_pixel() as usize,
                &patch.coverage,
            );
        }
        checkpoint
            .hidden
            .apply_ref(Arc::make_mut(&mut self.hidden_coverage));
        let next = ResidentColorTarget {
            key,
            generation: checkpoint.generation,
            initialized: checkpoint.proof,
            device_bytes: DeviceColorBytes {
                key,
                generation: checkpoint.generation,
                format: key.format(),
                bytes: bytes.into_boxed_slice(),
            },
            coverage,
        };
        match index {
            Some(index) => self.residents[index] = next,
            None => {
                self.residents.push(next);
            }
        }
        Ok(match index {
            Some(index) => &self.residents[index],
            None => self
                .residents
                .last()
                .expect("just inserted sparse resident"),
        })
    }

    pub(crate) fn prepare_publication(
        &mut self,
        initialized: InitializedCandidateColorTarget,
    ) -> Result<ResidentPublication<'_>, TargetError> {
        let hidden = HiddenCoveragePublication::try_new(
            initialized.key(),
            initialized.device_bytes().device_bytes(),
            &initialized.coverage,
        )?;
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
                hidden,
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
            hidden,
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
    DiscontinuousTaskColorSegment {
        expected_key: ColorTargetKey,
        actual_key: ColorTargetKey,
        expected_predecessor: Option<TargetGeneration>,
        actual_predecessor: Option<TargetGeneration>,
    },
    OrderedCpuCandidateMismatch {
        expected_key: ColorTargetKey,
        actual_key: ColorTargetKey,
        expected_generation: TargetGeneration,
        actual_generation: TargetGeneration,
        expected_predecessor: Option<TargetGeneration>,
        actual_predecessor: Option<TargetGeneration>,
    },
    SparseCheckpointNonRdramWrite {
        operation: u32,
    },
    SparseCheckpointRangeOutsideTarget {
        key: ColorTargetKey,
        range: PhysicalRange,
    },
    SparseCheckpointDigestMismatch {
        operation: u32,
    },
    SparseCheckpointEmpty,
    SparseCheckpointPatchCountMismatch {
        expected: usize,
        actual: usize,
    },
    SparseCheckpointAccessMismatch {
        position: usize,
        expected: fn64_render_ir::ResourceAccess,
        actual: fn64_render_ir::ResourceAccess,
    },
    SparseCheckpointAccessNotRow {
        key: ColorTargetKey,
        range: PhysicalRange,
    },
    SparseCheckpointClaimedRectangleMismatch {
        expected: TargetRectangle,
        actual: TargetRectangle,
    },
    HiddenCoverageByteLengthMismatch {
        key: ColorTargetKey,
        expected: usize,
        actual: usize,
    },
    HiddenCoverageCellCountMismatch {
        key: ColorTargetKey,
        expected: usize,
        actual: usize,
    },
    HiddenCoverageCountInvalid {
        key: ColorTargetKey,
        index: usize,
        count: u8,
    },
    HiddenCoverageVisibleBitMismatch {
        key: ColorTargetKey,
        index: usize,
        count: u8,
        expected: u8,
        actual: u8,
    },
    HiddenCoverageFragmentOutsideTarget {
        key: ColorTargetKey,
        start: usize,
        len: usize,
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
            Self::DiscontinuousTaskColorSegment { expected_key, actual_key, expected_predecessor, actual_predecessor } => write!(
                formatter,
                "discontinuous task color segment: expected {expected_key:?} after {expected_predecessor:?}, got {actual_key:?} after {actual_predecessor:?}"
            ),
            Self::OrderedCpuCandidateMismatch { expected_key, actual_key, expected_generation, actual_generation, expected_predecessor, actual_predecessor } => write!(
                formatter,
                "ordered CPU color completion mismatch: expected {expected_key:?}/{} after {expected_predecessor:?}, got {actual_key:?}/{} after {actual_predecessor:?}",
                expected_generation.get(),
                actual_generation.get(),
            ),
            Self::SparseCheckpointNonRdramWrite { operation } => write!(
                formatter,
                "sparse color checkpoint operation {operation} does not name RDRAM"
            ),
            Self::SparseCheckpointRangeOutsideTarget { key, range } => write!(
                formatter,
                "sparse color checkpoint range {range:?} lies outside target {key:?}"
            ),
            Self::SparseCheckpointDigestMismatch { operation } => write!(
                formatter,
                "sparse color checkpoint payload digest differs from completed write operation {operation}"
            ),
            Self::SparseCheckpointEmpty => {
                write!(formatter, "sparse row-bin checkpoint has no journal writes")
            }
            Self::SparseCheckpointPatchCountMismatch { expected, actual } => write!(
                formatter,
                "sparse color checkpoint has {actual} patches; expected {expected} journal writes"
            ),
            Self::SparseCheckpointAccessMismatch {
                position,
                expected,
                actual,
            } => write!(
                formatter,
                "sparse color checkpoint patch {position} names {actual:?}; expected {expected:?}"
            ),
            Self::SparseCheckpointAccessNotRow { key, range } => write!(
                formatter,
                "sparse row-bin checkpoint range {range:?} is not one aligned row inside {key:?}"
            ),
            Self::SparseCheckpointClaimedRectangleMismatch { expected, actual } => write!(
                formatter,
                "sparse row-bin checkpoint claims {actual:?}; journal rows derive {expected:?}"
            ),
            Self::HiddenCoverageByteLengthMismatch {
                key,
                expected,
                actual,
            } => write!(
                formatter,
                "hidden-coverage publication for {key:?} has {actual} visible bytes; expected {expected}"
            ),
            Self::HiddenCoverageCellCountMismatch {
                key,
                expected,
                actual,
            } => write!(
                formatter,
                "hidden-coverage publication for {key:?} has {actual} cells; expected {expected}"
            ),
            Self::HiddenCoverageCountInvalid { key, index, count } => write!(
                formatter,
                "hidden-coverage publication for {key:?} pixel {index} has invalid count {count}"
            ),
            Self::HiddenCoverageVisibleBitMismatch {
                key,
                index,
                count,
                expected,
                actual,
            } => write!(
                formatter,
                "hidden-coverage publication for {key:?} pixel {index} count {count} requires visible bit {expected}, got {actual}"
            ),
            Self::HiddenCoverageFragmentOutsideTarget { key, start, len } => write!(
                formatter,
                "hidden-coverage fragment [{start}, {}) is not an aligned range inside {key:?}",
                start.saturating_add(*len),
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

#[cfg(test)]
mod sparse_checkpoint_tests {
    use super::*;
    use fn64_render_ir::{AccessMode, AccessPurpose, OperationId, RdramResource, ResourceAccess};

    fn row_bin_patch(
        access: ResourceAccess,
        bytes: &[u8],
        coverage: &[u8],
    ) -> raw_triangle::PreparedRawTriangleCheckpointPatch {
        raw_triangle::PreparedRawTriangleCheckpointPatch {
            access,
            bytes: bytes.to_vec(),
            coverage: coverage.to_vec(),
        }
    }

    fn fixture() -> (PhysicalMemoryLayout, ColorTargetKey) {
        let layout = PhysicalMemoryLayout::try_new(0x20_0000).unwrap();
        let key = ColorTargetKey::try_new(
            layout.address(0x400).unwrap(),
            ColorTargetExtent::try_new(8, 2).unwrap(),
            ColorTargetFormat::Rgba16,
        )
        .unwrap();
        (layout, key)
    }

    fn initialized_with(
        registry: &ColorTargetRegistry,
        key: ColorTargetKey,
        bytes: Vec<u8>,
        rectangle: TargetRectangle,
    ) -> InitializedCandidateColorTarget {
        let candidate = registry.begin_candidate(key).unwrap();
        initialized_candidate(candidate, bytes, rectangle)
    }

    fn initialized_candidate(
        candidate: CandidateColorTarget,
        bytes: Vec<u8>,
        rectangle: TargetRectangle,
    ) -> InitializedCandidateColorTarget {
        let key = candidate.key();
        let generation = candidate.generation();
        let device_bytes =
            DeviceColorBytes::new_for_fill(key, generation, key.format(), bytes).unwrap();
        candidate
            .admit_completed_initialization(CompletedColorTargetWrite::new_for_fill(
                key,
                generation,
                key.range(),
                rectangle,
                device_bytes,
            ))
            .unwrap()
    }

    fn write(key: ColorTargetKey, operation: u32, start: u32, bytes: &[u8]) -> CompletedWrite {
        let range = key
            .address()
            .layout()
            .range(start, start + bytes.len() as u32)
            .unwrap();
        let access = ResourceAccess::try_new(
            OperationId::new(operation),
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range,
            },
        )
        .unwrap();
        CompletedWrite::try_from_bytes(access, bytes).unwrap()
    }

    #[test]
    fn first_generation_partial_checkpoint_has_exact_cardinality_and_zero_base() {
        let (layout, key) = fixture();
        let mut registry = ColorTargetRegistry::try_new(layout, 2).unwrap();
        let mut full = vec![0u8; key.range().len() as usize];
        full[4..8].copy_from_slice(&[1, 2, 3, 4]);
        let initialized = initialized_with(
            &registry,
            key,
            full,
            TargetRectangle::try_new(2, 0, 2, 1).unwrap(),
        );
        let writes = [write(key, 7, key.address().get() + 4, &[1, 2, 3, 4])];
        let checkpoint = initialized.sparse_checkpoint(&writes).unwrap();
        assert_eq!(checkpoint.payloads().len(), writes.len());
        assert_eq!(checkpoint.payloads().next().unwrap(), [1, 2, 3, 4]);
        let first_transport = checkpoint.shared_payloads().next().unwrap();
        let second_transport = checkpoint.shared_payloads().next().unwrap();
        assert!(Arc::ptr_eq(&first_transport, &second_transport));

        let resident = registry.commit_sparse_checkpoint(checkpoint).unwrap();
        assert_eq!(resident.generation(), TargetGeneration::FIRST);
        assert_eq!(
            resident.device_bytes().device_bytes(),
            &[
                0, 0, 0, 0, 1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn sparse_checkpoint_preserves_exact_physical_coverage_counts() {
        let (layout, key) = fixture();
        let mut registry = ColorTargetRegistry::try_new(layout, 2).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        let mut bytes = Vec::with_capacity(key.range().len() as usize);
        let mut coverage = ColorCoverageState::unknown(key.extent());
        for pixel in 0..key.extent().pixels() as usize {
            let count = (pixel % 8 + 1) as u8;
            let visible_bit = (crate::Coverage::new(count).stored() >> 2) & 1;
            bytes.extend_from_slice(&(0x2468u16 | u16::from(visible_bit)).to_be_bytes());
            coverage.set_exact(pixel, crate::Coverage::new(count));
        }
        let generation = candidate.generation();
        let device =
            DeviceColorBytes::new_for_fill(key, generation, key.format(), bytes.clone()).unwrap();
        let initialized = candidate
            .admit_completed_initialization(
                CompletedColorTargetWrite::new_for_fill(
                    key,
                    generation,
                    key.range(),
                    TargetRectangle::try_new(0, 0, key.extent().width(), key.extent().height())
                        .unwrap(),
                    device,
                )
                .with_coverage(coverage.clone()),
            )
            .unwrap();
        let writes = [write(key, 9, key.address().get(), &bytes)];
        let checkpoint = initialized.sparse_checkpoint(&writes).unwrap();
        let resident = registry.commit_sparse_checkpoint(checkpoint).unwrap();
        assert_eq!(resident.coverage(), &coverage);
        assert_eq!(registry.project_coverage(key, &bytes), coverage);
    }

    #[test]
    fn row_bin_checkpoint_sealer_preserves_journal_order_payloads_and_coverage() {
        let (layout, key) = fixture();
        let registry = ColorTargetRegistry::try_new(layout, 2).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        let first = write(key, 9, key.address().get(), &[0x24, 0x68, 0x24, 0x68]);
        let second = write(key, 3, key.address().get() + 8, &[0x13, 0x56, 0x13, 0x56]);
        let (checkpoint, writes) = SparseInitializedColorCheckpoint::from_row_bin_execution(
            &candidate,
            TargetRectangle::try_new(0, 0, 6, 1).unwrap(),
            vec![
                row_bin_patch(first.access(), &[0x24, 0x68, 0x24, 0x68], &[1, 1]),
                row_bin_patch(second.access(), &[0x13, 0x56, 0x13, 0x56], &[1, 1]),
            ],
            &[first.access(), second.access()],
        )
        .unwrap();
        assert_eq!(
            writes
                .iter()
                .map(|write| write.access().operation().get())
                .collect::<Vec<_>>(),
            [9, 3]
        );
        assert_eq!(
            checkpoint.payloads().collect::<Vec<_>>(),
            [&[0x24, 0x68, 0x24, 0x68][..], &[0x13, 0x56, 0x13, 0x56][..]]
        );
    }

    #[test]
    fn row_bin_checkpoint_sealer_rejects_cardinality_order_and_target_forgery() {
        let (layout, key) = fixture();
        let registry = ColorTargetRegistry::try_new(layout, 2).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        let first = write(key, 9, key.address().get(), &[0x24, 0x68, 0x24, 0x68]);
        let second = write(key, 3, key.address().get() + 8, &[0x13, 0x56, 0x13, 0x56]);
        let full = TargetRectangle::try_new(0, 0, 8, 2).unwrap();

        assert!(matches!(
            SparseInitializedColorCheckpoint::from_row_bin_execution(
                &candidate,
                full,
                vec![row_bin_patch(
                    first.access(),
                    &[0x24, 0x68, 0x24, 0x68],
                    &[1, 1]
                )],
                &[first.access(), second.access()],
            ),
            Err(TargetError::SparseCheckpointPatchCountMismatch {
                expected: 2,
                actual: 1
            })
        ));
        assert!(matches!(
            SparseInitializedColorCheckpoint::from_row_bin_execution(
                &candidate,
                full,
                vec![
                    row_bin_patch(second.access(), &[0x13, 0x56, 0x13, 0x56], &[1, 1]),
                    row_bin_patch(first.access(), &[0x24, 0x68, 0x24, 0x68], &[1, 1]),
                ],
                &[first.access(), second.access()],
            ),
            Err(TargetError::SparseCheckpointAccessMismatch { position: 0, .. })
        ));
        assert!(matches!(
            SparseInitializedColorCheckpoint::from_row_bin_execution(
                &candidate,
                full,
                vec![row_bin_patch(
                    first.access(),
                    &[0x24, 0x68, 0x24, 0x68],
                    &[1, 1]
                )],
                &[first.access()],
            ),
            Err(TargetError::SparseCheckpointClaimedRectangleMismatch { .. })
        ));
        assert!(matches!(
            SparseInitializedColorCheckpoint::from_row_bin_execution(
                &candidate,
                full,
                Vec::new(),
                &[],
            ),
            Err(TargetError::SparseCheckpointEmpty)
        ));
        assert!(matches!(
            SparseInitializedColorCheckpoint::from_row_bin_execution(
                &candidate,
                TargetRectangle::try_new(7, 1, 2, 1).unwrap(),
                vec![row_bin_patch(
                    first.access(),
                    &[0x24, 0x68, 0x24, 0x68],
                    &[1, 1]
                )],
                &[first.access()],
            ),
            Err(TargetError::RectangleOutOfBounds { .. })
        ));
        assert!(registry.residents().is_empty());
    }

    #[test]
    fn row_bin_checkpoint_sealer_rejects_bad_payload_and_hidden_coverage() {
        let (layout, key) = fixture();
        let mut registry = ColorTargetRegistry::try_new(layout, 2).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        let declared = write(key, 9, key.address().get(), &[0x24, 0x68, 0x24, 0x68]);
        let claimed = TargetRectangle::try_new(0, 0, 2, 1).unwrap();

        assert!(SparseInitializedColorCheckpoint::from_row_bin_execution(
            &candidate,
            claimed,
            vec![row_bin_patch(declared.access(), &[0x24, 0x68], &[1])],
            &[declared.access()],
        )
        .is_err());
        assert!(matches!(
            SparseInitializedColorCheckpoint::from_row_bin_execution(
                &candidate,
                claimed,
                vec![row_bin_patch(
                    declared.access(),
                    &[0x24, 0x68, 0x24, 0x68],
                    &[1]
                )],
                &[declared.access()],
            ),
            Err(TargetError::HiddenCoverageCellCountMismatch { .. })
        ));
        assert!(matches!(
            SparseInitializedColorCheckpoint::from_row_bin_execution(
                &candidate,
                claimed,
                vec![row_bin_patch(
                    declared.access(),
                    &[0x24, 0x68, 0x24, 0x68],
                    &[9, 1]
                )],
                &[declared.access()],
            ),
            Err(TargetError::HiddenCoverageCountInvalid { index: 0, .. })
        ));
        assert!(matches!(
            SparseInitializedColorCheckpoint::from_row_bin_execution(
                &candidate,
                claimed,
                vec![row_bin_patch(
                    declared.access(),
                    &[0x24, 0x68, 0x24, 0x68],
                    &[8, 1]
                )],
                &[declared.access()],
            ),
            Err(TargetError::HiddenCoverageVisibleBitMismatch { index: 0, .. })
        ));

        let (unknown, _) = SparseInitializedColorCheckpoint::from_row_bin_execution(
            &candidate,
            claimed,
            vec![row_bin_patch(
                declared.access(),
                &[0x24, 0x68, 0x24, 0x68],
                &[0, 1],
            )],
            &[declared.access()],
        )
        .unwrap();
        let resident = registry.commit_sparse_checkpoint(unknown).unwrap();
        assert_eq!(resident.coverage().exact(0), None);
        assert_eq!(resident.coverage().exact(1), Some(crate::Coverage::new(1)));
    }

    #[test]
    fn overlapping_packet_checkpoints_preserve_intermediate_and_final_generations() {
        let (layout, key) = fixture();
        let mut registry = ColorTargetRegistry::try_new(layout, 2).unwrap();
        let first_bytes = vec![0x11; key.range().len() as usize];
        let first = initialized_with(
            &registry,
            key,
            first_bytes.clone(),
            TargetRectangle::try_new(0, 0, 8, 2).unwrap(),
        );
        let first_writes = [write(key, 0, key.address().get(), &first_bytes)];
        let first_checkpoint = first.sparse_checkpoint(&first_writes).unwrap();
        registry.commit_sparse_checkpoint(first_checkpoint).unwrap();
        assert_eq!(
            registry.residents()[0].device_bytes().device_bytes(),
            first_bytes
        );

        let mut second_bytes = first_bytes;
        second_bytes[8..16].fill(0x22);
        let second = initialized_with(
            &registry,
            key,
            second_bytes.clone(),
            TargetRectangle::try_new(4, 0, 4, 1).unwrap(),
        );
        let second_writes = [write(key, 1, key.address().get() + 8, &second_bytes[8..16])];
        let second_checkpoint = second.sparse_checkpoint(&second_writes).unwrap();
        let resident = registry
            .commit_sparse_checkpoint(second_checkpoint)
            .unwrap();
        assert_eq!(resident.generation().get(), 2);
        assert_eq!(resident.device_bytes().device_bytes(), second_bytes);
    }

    #[test]
    fn stale_sparse_checkpoint_rejection_is_failure_atomic() {
        let (layout, key) = fixture();
        let mut registry = ColorTargetRegistry::try_new(layout, 2).unwrap();
        registry
            .commit_initialized(initialized_with(
                &registry,
                key,
                vec![0x10; key.range().len() as usize],
                TargetRectangle::try_new(0, 0, 8, 2).unwrap(),
            ))
            .unwrap();
        let make_checkpoint = |registry: &ColorTargetRegistry, value: u8| {
            let bytes = vec![value; key.range().len() as usize];
            let initialized = initialized_with(
                registry,
                key,
                bytes.clone(),
                TargetRectangle::try_new(0, 0, 8, 2).unwrap(),
            );
            initialized
                .sparse_checkpoint(&[write(key, 2, key.address().get(), &bytes)])
                .unwrap()
        };
        let accepted = make_checkpoint(&registry, 0x20);
        let stale = make_checkpoint(&registry, 0x30);
        registry.commit_sparse_checkpoint(accepted).unwrap();
        let before = registry.residents()[0].clone();
        assert!(matches!(
            registry.commit_sparse_checkpoint(stale),
            Err(TargetError::StaleCandidateGeneration { .. })
        ));
        assert_eq!(registry.residents()[0], before);
    }

    #[test]
    fn owned_task_shadow_moves_the_full_accumulator_allocation() {
        let (layout, key) = fixture();
        let mut registry = ColorTargetRegistry::try_new(layout, 2).unwrap();
        let initialized = initialized_with(
            &registry,
            key,
            vec![0x55; key.range().len() as usize],
            TargetRectangle::try_new(0, 0, 8, 2).unwrap(),
        );
        let allocation = initialized.device_bytes().device_bytes().as_ptr();
        let reservation = OrderedCpuCandidateReservation::new(&initialized.candidate);
        let continuity = OrderedCpuColorContinuity::start(reservation, &initialized).unwrap();
        let before = registry.residents().to_vec();
        let prepared = registry
            .prepare_owned_task_shadow_install(continuity.finish(initialized).unwrap())
            .unwrap();
        assert_eq!(registry.residents(), before);
        let resident = registry.install_prepared_owned_task_shadow(prepared);
        assert_eq!(resident.device_bytes().device_bytes().as_ptr(), allocation);
    }

    #[test]
    fn ordered_cpu_reservation_rejects_candidate_substitution_and_generation_skip() {
        let (layout, key) = fixture();
        let registry = ColorTargetRegistry::try_new(layout, 2).unwrap();
        let mut planner = ColorTargetExecutionBatch::new();
        let (first_candidate, _) = planner.begin_candidate(&registry, key).unwrap();
        let first_reservation = OrderedCpuCandidateReservation::new(&first_candidate);
        let full = TargetRectangle::try_new(0, 0, 8, 2).unwrap();
        let first = initialized_candidate(
            first_candidate,
            vec![0x11; key.range().len() as usize],
            full,
        );

        let other_key =
            ColorTargetKey::try_new(layout.address(0x800).unwrap(), key.extent(), key.format())
                .unwrap();
        let other_candidate = registry.begin_candidate(other_key).unwrap();
        let other = initialized_candidate(
            other_candidate,
            vec![0x22; other_key.range().len() as usize],
            full,
        );
        assert!(matches!(
            first_reservation.validate(&other),
            Err(TargetError::OrderedCpuCandidateMismatch { .. })
        ));

        let continuity = OrderedCpuColorContinuity::start(first_reservation, &first).unwrap();
        let (_second_candidate, _) = planner.begin_candidate(&registry, key).unwrap();
        let (third_candidate, _) = planner.begin_candidate(&registry, key).unwrap();
        let third_reservation = OrderedCpuCandidateReservation::new(&third_candidate);
        let third = initialized_candidate(
            third_candidate,
            vec![0x33; key.range().len() as usize],
            full,
        );
        assert!(matches!(
            continuity.append(third_reservation, &third),
            Err(TargetError::DiscontinuousTaskColorSegment { .. })
        ));
    }

    #[test]
    fn sparse_checkpoint_rejects_digest_mutation_and_binds_operation_order() {
        let (layout, key) = fixture();
        let registry = ColorTargetRegistry::try_new(layout, 2).unwrap();
        let mut bytes = vec![0u8; key.range().len() as usize];
        bytes[0..4].copy_from_slice(&[1, 2, 3, 4]);
        bytes[8..12].copy_from_slice(&[5, 6, 7, 8]);
        let initialized = initialized_with(
            &registry,
            key,
            bytes,
            TargetRectangle::try_new(0, 0, 8, 2).unwrap(),
        );
        let first = write(key, 9, key.address().get(), &[1, 2, 3, 4]);
        let second = write(key, 3, key.address().get() + 8, &[5, 6, 7, 8]);
        let checkpoint = initialized.sparse_checkpoint(&[second, first]).unwrap();
        assert_eq!(
            checkpoint
                .writes()
                .map(|write| write.access().operation().get())
                .collect::<Vec<_>>(),
            [3, 9],
            "checkpoint order is the supplied journal order, never range order"
        );

        let bad_digest = write(key, 9, key.address().get(), &[9, 9, 9, 9]);
        assert!(matches!(
            initialized.sparse_checkpoint(&[bad_digest]),
            Err(TargetError::SparseCheckpointDigestMismatch { operation: 9 })
        ));
    }

    #[test]
    fn access_fused_sparse_checkpoint_matches_independent_write_validation() {
        let (layout, key) = fixture();
        let registry = ColorTargetRegistry::try_new(layout, 2).unwrap();
        let mut bytes = vec![0u8; key.range().len() as usize];
        bytes[0..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        bytes[4..12].copy_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);
        let initialized = initialized_with(
            &registry,
            key,
            bytes,
            TargetRectangle::try_new(0, 0, 6, 1).unwrap(),
        );
        let first = write(
            key,
            7,
            key.address().get(),
            initialized.device_bytes().device_bytes().get(0..8).unwrap(),
        );
        let second = write(
            key,
            8,
            key.address().get() + 4,
            initialized
                .device_bytes()
                .device_bytes()
                .get(4..12)
                .unwrap(),
        );
        let expected = initialized.sparse_checkpoint(&[second, first]).unwrap();
        let (actual, writes) = initialized
            .sparse_checkpoint_from_accesses(&[second.access(), first.access()])
            .unwrap();
        assert_eq!(writes, [second, first]);
        assert_eq!(actual, expected);
    }
}
