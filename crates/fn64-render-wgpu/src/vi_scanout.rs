//! Guest-RDRAM VI scanout for [`crate::WgpuBackend::present`].
//!
//! ## What this is, and what it deliberately is not
//!
//! One hardware VI retrace selects a rectangle of guest RDRAM through the
//! latched fourteen-word register image and emits it as a field. That is the
//! whole of what this module does: it reads the programmed source rectangle
//! out of physical RDRAM, expands it to 8-bit RGBA, and maps output pixels to
//! source samples. It performs **no rasterization** -- the pixels it presents
//! were already written to guest memory by whatever produced them (for the
//! raw-DPC lane, an admitted `FillRectangle` copied back by
//! `fn64-abi`'s `copy_committed_guest_writes`).
//!
//! Every VI filter this module does *not* implement is a **named, specific**
//! refusal naming the filter, never a generic "out of scope" and never a
//! silent pass-through. See [`ViScanoutRefusal`].
//!
//! ## Coordinate convention: fn64's, stated explicitly
//!
//! RT64 computes `xScaleFloat() = 1024.0f / xScale` and *divides* by it.
//! fn64 keeps the raw U2.10 register field and *multiplies*:
//!
//! ```text
//! position_u2_10 = offset_u2_10 + output_index * step_u2_10
//! source_index   = position_u2_10 >> 10
//! ```
//!
//! The two disagree on 1,247 pairs, all exact half-integer ties, with RT64
//! always rounding down (`docs/RT64-PORT-PARITY.md`'s VI scale row). **This
//! module follows fn64's convention**, which is the live one -- the same
//! integer arithmetic `fn64-render-reference`'s `AxisPositionU10Fraction`
//! (`crates/fn64-render-reference/src/vi.rs:353-375`) already uses. RT64's
//! reciprocal is deliberately not imported; doing so silently would change
//! the sampled column on every tie.
//!
//! ## Byte-lane authority
//!
//! Source pixels are read through [`fn64_runtime::PhysicalRdramRead`], whose
//! `read_u16`/`read_u8` apply the N64Recomp `^2`/`^3` native-word lane
//! mapping (`crates/fn64-runtime/src/rdram.rs:97-140`). That is the same
//! authority `fn64-render-reference`'s `load_vi_source` reads through, and
//! the same one `RdramViewMut::write_u16` writes through. This module does
//! not index raw bytes, precisely so it cannot invent a second convention.
//!
//! ## Provenance
//!
//! Register field extents are cross-checked against
//! [`crate::rt64_vi_registers`]'s ported `rt64_vi.h` bitfield unions; the
//! decoded semantics come from `fn64_render`'s shared `ViScanoutRegisters`.
//! The public libultra VI interface and the *N64 Programming Manual*'s Video
//! Interface chapter name the registers; US 6,166,748 Figures 35M/35N
//! establish the U2.10 scale/offset split.

use fn64_render::{
    RenderError, ViPixelType, ViPresentation, ViResampleControl, ViScaleAxis, ViScanoutRegisters,
};

/// A VI feature this scanout genuinely does not implement, named
/// individually so a rejection says *which* filter was programmed rather
/// than that presentation "is out of scope".
///
/// Every variant is a real refusal: reaching one means the guest programmed
/// a filter whose output this module cannot produce, and producing the
/// unfiltered image anyway would be a silent wrong answer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ViScanoutRefusal {
    /// VI STATUS AA mode 0 or 1: the coverage-driven silhouette filter of
    /// US 5,742,277 Figure 11. Needs per-pixel coverage, which guest RDRAM
    /// RGBA16 carries in its low bit and hidden bits -- state this backend
    /// does not track.
    SilhouetteAntialias,
    /// VI STATUS bit 16: the dither-restoration filter. Same coverage and
    /// hidden-bit dependency.
    DitherRestoration,
    /// VI STATUS bit 4: the divot median filter over three horizontal
    /// post-filter samples.
    Divot,
    /// VI STATUS bit 3: the gamma curve. The silicon gamma ROM is not
    /// publicly specified; emitting a linear image while STATUS asks for
    /// gamma would be a wrong image, not a partial one.
    Gamma,
    /// VI STATUS bit 2: gamma dither, which needs a retrace-seeded noise
    /// generator this module does not own.
    GammaDither,
    /// VI STATUS AA mode 0/1/2: bilinear resampling between adjacent source
    /// samples. Only AA mode 3 (`Replicate`, nearest-neighbor) is
    /// implemented.
    BilinearResampling,
    /// `osViFade`'s two-row interpolation.
    Fade,
    /// `osViRepeatLine`.
    RepeatLine,
    /// VI STATUS pixel type 1.
    ReservedPixelType,
}

impl ViScanoutRefusal {
    /// The exact reason text carried into `RenderError::Backend`.
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::SilhouetteAntialias => {
                "VI STATUS selects coverage silhouette antialiasing (AA mode 0 or 1); this \
                 scanout implements only AA mode 3 (replicate) because it tracks no per-pixel \
                 coverage"
            }
            Self::DitherRestoration => {
                "VI STATUS bit 16 selects the dither-restoration filter; this scanout tracks \
                 no RDRAM hidden bits and cannot restore dithered RGBA16"
            }
            Self::Divot => {
                "VI STATUS bit 4 selects the divot median filter; this scanout implements no \
                 post-filter neighborhood pass"
            }
            Self::Gamma => {
                "VI STATUS bit 3 selects the gamma curve; the silicon gamma ROM is not \
                 publicly specified and emitting a linear field instead would be a wrong image"
            }
            Self::GammaDither => {
                "VI STATUS bit 2 selects gamma dither; this scanout owns no retrace-seeded \
                 noise generator"
            }
            Self::BilinearResampling => {
                "VI STATUS selects a resampling AA mode (0, 1, or 2); this scanout implements \
                 only AA mode 3 (replicate) nearest-neighbor sampling"
            }
            Self::Fade => {
                "osViFade programmed a two-row interpolation factor; this scanout implements \
                 no fade"
            }
            Self::RepeatLine => {
                "osViRepeatLine is programmed; this scanout implements no repeat-line"
            }
            Self::ReservedPixelType => "VI STATUS selects reserved pixel type 1",
        }
    }

    pub(crate) fn into_error(self) -> RenderError {
        RenderError::Backend {
            backend: "render-wgpu-vi-scanout",
            reason: self.reason().to_string(),
        }
    }
}

/// One presented VI field: 8-bit RGBA at exactly the guest-programmed active
/// output rectangle.
///
/// `width`/`height` are `ViActiveWindow::output_width`/`output_height` --
/// the guest's own digital output rectangle, never a host window size and
/// never the source image's extent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentedField {
    /// Guest-programmed active output width in pixels.
    pub width: u32,
    /// Guest-programmed active output lines for this exact field.
    pub height: u32,
    /// Row-major, tightly packed, four bytes per pixel, R G B A.
    pub rgba8: Vec<u8>,
    /// The exact VI image that selected this field, retained so a consumer
    /// cannot pair a field with a different retrace's registers.
    pub presentation: ViPresentation,
}

impl PresentedField {
    /// The pixel at `(x, y)` as `[r, g, b, a]`, or `None` outside the field.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let start = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        Some([
            self.rgba8[start],
            self.rgba8[start + 1],
            self.rgba8[start + 2],
            self.rgba8[start + 3],
        ])
    }
}

/// Guest memory footprint one field reads, derived from the live registers.
///
/// Kept as its own value so the bounds check happens once, against the whole
/// rectangle, before a single pixel is read -- rather than relying on each
/// per-pixel read's own panic.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct SourceGeometry {
    origin: u32,
    stride_pixels: u32,
    /// Source rows the programmed vertical coordinates actually reach.
    rows: u64,
    bytes_per_pixel: u8,
    pixel_type: ViPixelType,
}

/// The five-bit-to-eight-bit channel expansion the VI performs on RGBA16.
///
/// Replicating the top three bits into the low three is the standard public
/// expansion and is exactly `fn64-render-reference`'s `expand_five_bit`
/// (`crates/fn64-render-reference/src/vi.rs:206-209`). Its inverse
/// (`>> 3`) recovers the original five bits for every input, which is what
/// lets a hand-derived RGBA16 expectation be compared either way.
const fn expand_five_bit(value: u8) -> u8 {
    (value << 3) | (value >> 2)
}

/// fn64's U2.10 source coordinate: multiply the raw register step, never
/// divide by a reciprocal. See this module's header for the RT64
/// disagreement this deliberately does not adopt.
fn source_index(output_index: u32, axis: ViScaleAxis, source_extent: u64) -> u64 {
    let position = u64::from(axis.offset_u2_10())
        .checked_add(
            u64::from(output_index)
                .checked_mul(u64::from(axis.step_u2_10()))
                .expect("VI source coordinate product overflow"),
        )
        .expect("VI source coordinate sum overflow");
    let index = position >> ViScaleAxis::FRACTION_BITS;
    // The high edge is held, matching the reference backend's `HeldLast`
    // bound (`crates/fn64-render-reference/src/vi.rs:403-409`). A source
    // extent is never zero here: `SourceGeometry` proves both axes nonzero
    // before any sampling runs.
    index.min(source_extent - 1)
}

/// Rows of source the programmed vertical coordinates actually touch, so the
/// footprint check covers exactly what will be read and no halo this module
/// does not sample.
fn source_rows(registers: ViScanoutRegisters, output_height: u32) -> u64 {
    let resample = registers.resample();
    let last_output = u64::from(output_height - 1);
    let last = u64::from(resample.y.offset_u2_10())
        .checked_add(
            last_output
                .checked_mul(u64::from(resample.y.step_u2_10()))
                .expect("VI vertical coordinate product overflow"),
        )
        .expect("VI vertical coordinate sum overflow");
    (last >> ViScaleAxis::FRACTION_BITS) + 1
}

/// Reject every filter this scanout does not implement, by name, before any
/// memory is read.
///
/// Ordering is deliberate: the reserved pixel type is checked first because
/// it is a malformed register image rather than an unimplemented filter, and
/// `ViPixelType::Reserved` has no byte width to bound-check against.
fn admitted_filters(vi: ViPresentation) -> Result<(), ViScanoutRefusal> {
    let filters = vi.scanout.filters();
    if filters.pixel_type == ViPixelType::Reserved {
        return Err(ViScanoutRefusal::ReservedPixelType);
    }
    if vi.fade.is_some() {
        return Err(ViScanoutRefusal::Fade);
    }
    if vi.repeat_line {
        return Err(ViScanoutRefusal::RepeatLine);
    }
    if filters.antialias_mode.silhouette_aa_enabled() {
        return Err(ViScanoutRefusal::SilhouetteAntialias);
    }
    if filters.dither_filter {
        return Err(ViScanoutRefusal::DitherRestoration);
    }
    if filters.divot {
        return Err(ViScanoutRefusal::Divot);
    }
    if filters.antialias_mode.resampling_enabled() {
        return Err(ViScanoutRefusal::BilinearResampling);
    }
    if filters.gamma {
        return Err(ViScanoutRefusal::Gamma);
    }
    if filters.gamma_dither {
        return Err(ViScanoutRefusal::GammaDither);
    }
    Ok(())
}

impl SourceGeometry {
    /// Derive the read footprint, or report that this field reads no source
    /// at all (`Ok(None)`: a blanked or inactive-window field).
    fn derive(
        vi: ViPresentation,
        registers: ViScanoutRegisters,
    ) -> Result<Option<(Self, u32, u32)>, RenderError> {
        let filters = vi.scanout.filters();
        let Some(window) = registers.active_window() else {
            return Ok(None);
        };
        if vi.blanked || filters.pixel_type == ViPixelType::Blank {
            return Ok(None);
        }
        let (bytes_per_pixel, pixel_type) = match filters.pixel_type {
            ViPixelType::Rgba16 => (2u8, ViPixelType::Rgba16),
            ViPixelType::Rgba32 => (4u8, ViPixelType::Rgba32),
            // `admitted_filters` rejected `Reserved`; `Blank` returned above;
            // `Unspecified` cannot come from a register image, whose
            // `ViFilterControl::from_status` always names one of the four.
            ViPixelType::Blank | ViPixelType::Reserved | ViPixelType::Unspecified => {
                return Err(RenderError::Backend {
                    backend: "render-wgpu-vi-scanout",
                    reason: format!(
                        "VI STATUS decoded to {:?}, which no live register image produces",
                        filters.pixel_type
                    ),
                })
            }
        };
        let output_width = window.output_width();
        let output_height = window.output_height();
        if output_width == 0 || output_height == 0 {
            // `ViActiveWindow::from_registers` proves both intervals
            // nonempty, so a zero output extent cannot be reached from a
            // constructed window. Reject by name rather than dividing by it.
            return Err(RenderError::Backend {
                backend: "render-wgpu-vi-scanout",
                reason: format!(
                    "VI active window has a zero output extent {output_width}x{output_height}"
                ),
            });
        }
        let stride_pixels = registers.width();
        if stride_pixels == 0 {
            return Err(RenderError::Backend {
                backend: "render-wgpu-vi-scanout",
                reason: "VI WIDTH programs a zero source stride for an active scanout image"
                    .to_string(),
            });
        }
        let origin = registers.origin();
        if bytes_per_pixel == 2 && !origin.is_multiple_of(2) {
            return Err(RenderError::InvalidViSourceAlignment {
                origin,
                bytes_per_pixel,
            });
        }
        if bytes_per_pixel == 4 && !origin.is_multiple_of(4) {
            return Err(RenderError::InvalidViSourceAlignment {
                origin,
                bytes_per_pixel,
            });
        }
        Ok(Some((
            Self {
                origin,
                stride_pixels,
                rows: source_rows(registers, output_height),
                bytes_per_pixel,
                pixel_type,
            },
            output_width,
            output_height,
        )))
    }

    /// Prove the whole rectangle is inside physical RDRAM before reading it.
    fn validate(self, rdram_len: usize) -> Result<(), RenderError> {
        let row_bytes = u64::from(self.stride_pixels)
            .checked_mul(u64::from(self.bytes_per_pixel))
            .expect("VI source row byte count overflow");
        let byte_len = row_bytes
            .checked_mul(self.rows)
            .expect("VI source footprint overflow");
        let end = u64::from(self.origin)
            .checked_add(byte_len)
            .expect("VI source end overflow");
        if end > rdram_len as u64 || self.rows > u64::from(u32::MAX) {
            return Err(RenderError::InvalidViSourceBounds {
                origin: self.origin,
                stride_pixels: self.stride_pixels,
                rows: self.rows,
                bytes_per_pixel: self.bytes_per_pixel,
                rdram_len,
            });
        }
        Ok(())
    }

    /// Read one source pixel through the lane-mapped physical capability and
    /// expand it to 8-bit RGBA.
    fn sample(
        self,
        memory: &fn64_runtime::PhysicalRdramRead<'_>,
        source_x: u64,
        source_y: u64,
    ) -> [u8; 4] {
        let index = source_y * u64::from(self.stride_pixels) + source_x;
        let byte_offset = index * u64::from(self.bytes_per_pixel);
        let logical = u64::from(self.origin)
            .checked_add(byte_offset)
            .expect("VI source pixel address overflow");
        let logical = u32::try_from(logical).expect("validated VI source address exceeds u32");
        let address = fn64_runtime::RdramAddr::from_offset(logical);
        match self.pixel_type {
            ViPixelType::Rgba16 => {
                let pixel = memory.read_u16(address);
                [
                    expand_five_bit(((pixel >> 11) & 0x1f) as u8),
                    expand_five_bit(((pixel >> 6) & 0x1f) as u8),
                    expand_five_bit(((pixel >> 1) & 0x1f) as u8),
                    // RGBA16's low bit is the coverage/alpha bit. This
                    // scanout emits an opaque field, matching the reference
                    // backend's `load_vi_source`, which also writes 255 and
                    // routes the low bit into coverage instead.
                    255,
                ]
            }
            ViPixelType::Rgba32 => {
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
                [red, green, blue, (alpha5 << 3) | (alpha5 >> 2)]
            }
            ViPixelType::Blank | ViPixelType::Reserved | ViPixelType::Unspecified => {
                unreachable!("SourceGeometry::derive admits only Rgba16 and Rgba32")
            }
        }
    }
}

/// Scan one VI field out of guest physical RDRAM.
///
/// `vi` must carry a live register image; a caller holding only
/// `ViScanoutState::BackendOnly` has no origin, stride, or window to read
/// through and is rejected here rather than given a synthesized geometry.
pub(crate) fn scan_out_guest_rdram(
    vi: ViPresentation,
    memory: &fn64_runtime::PhysicalRdramRead<'_>,
) -> Result<PresentedField, RenderError> {
    let Some(registers) = vi.scanout.registers() else {
        return Err(RenderError::Backend {
            backend: "render-wgpu-vi-scanout",
            reason: "physical VI presentation requires a live fourteen-word register image; \
                     ViScanoutState::BackendOnly carries no origin, stride, or active window"
                .to_string(),
        });
    };
    admitted_filters(vi).map_err(ViScanoutRefusal::into_error)?;

    let Some((geometry, output_width, output_height)) = SourceGeometry::derive(vi, registers)?
    else {
        // A blanked or not-yet-programmed field is black at the programmed
        // output rectangle, and empty when there is no rectangle at all.
        // This is the VI's own behavior, not a fallback substituting for a
        // read this module could not perform.
        let (width, height) = match registers.active_window() {
            Some(window) => (window.output_width(), window.output_height()),
            None => (0, 0),
        };
        let mut rgba8 = vec![0u8; (width as usize) * (height as usize) * 4];
        for pixel in rgba8.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        return Ok(PresentedField {
            width,
            height,
            rgba8,
            presentation: vi,
        });
    };

    geometry.validate(memory.len())?;

    let ViResampleControl { x, y, .. } = registers.resample();
    let source_width = u64::from(geometry.stride_pixels);
    let source_height = geometry.rows;
    let mut rgba8 = Vec::with_capacity((output_width as usize) * (output_height as usize) * 4);
    for output_y in 0..output_height {
        let source_y = source_index(output_y, y, source_height);
        for output_x in 0..output_width {
            let source_x = source_index(output_x, x, source_width);
            rgba8.extend_from_slice(&geometry.sample(memory, source_x, source_y));
        }
    }

    Ok(PresentedField {
        width: output_width,
        height: output_height,
        rgba8,
        presentation: vi,
    })
}

#[cfg(test)]
mod tests;
