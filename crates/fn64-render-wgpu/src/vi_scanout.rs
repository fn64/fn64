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
    vi_public_filters::{
        gamma_dither_quantize_bounded_v1, reference_noise_bit_v1,
        restore_rgba16_component_bounded_v1, restore_rgba16_rgb5_bounded_v1,
        restore_rgba16_rgb_bounded_v1, Rgba16Rgb5,
    },
    RenderError, ViFilterControl, ViPixelType, ViPresentation, ViResampleControl, ViScaleAxis,
    ViScanoutRegisters,
};
use rayon::prelude::*;
use std::sync::OnceLock;

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
    /// US 5,742,277 Figure 11. The backend now retains the complete three-bit
    /// RGBA16 coverage count in physical-addressed hidden memory and supplies
    /// exact geometric triangle populations. The silhouette filtering
    /// algorithm itself is still absent, so selecting it remains a loud
    /// refusal rather than silently presenting an unfiltered image.
    SilhouetteAntialias,
    /// VI STATUS bit 16 **with a coverage-bearing pixel that this backend
    /// cannot classify**: dither restoration is implemented (see
    /// [`restore_dither`]), but only for RGBA16. Bit 16 over an RGBA32
    /// scanout image asks to restore a five-bit dither that an eight-bit
    /// source never carried, which the reference backend also refuses
    /// (`crates/fn64-render-reference/src/vi.rs`'s "VI dither restoration
    /// requires an RGBA16 scanout image").
    DitherRestorationNonRgba16,
    /// VI STATUS bit 4: the divot median filter over three horizontal
    /// post-filter samples.
    Divot,
    /// VI STATUS bit 3: the gamma curve. The silicon gamma ROM is not
    /// publicly specified; emitting a linear image while STATUS asks for
    /// gamma would be a wrong image, not a partial one.
    Gamma,
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
            Self::DitherRestorationNonRgba16 => {
                "VI STATUS bit 16 selects the dither-restoration filter over a non-RGBA16 \
                 scanout image; there is no five-bit dither in an RGBA32 source to restore"
            }
            Self::Divot => {
                "VI STATUS bit 4 selects the divot median filter; this scanout implements no \
                 post-filter neighborhood pass"
            }
            Self::Gamma => {
                "VI STATUS bit 3 selects the gamma curve; the silicon gamma ROM is not \
                 publicly specified and emitting a linear field instead would be a wrong image"
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
///
/// Superseded at the sampling sites by [`AxisSample::from_output`], which
/// resolves the same position into a lower/upper pair so bilinear
/// resampling can weight between them. Retained as the independent
/// derivation of the *nearest* index: `AxisSample::from_output(..).lower`
/// must equal this for every input, and
/// `replication_agrees_with_source_index_on_every_output_column` pins that
/// so the two cannot drift apart.
#[cfg(test)]
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

/// Rows of source the programmed vertical coordinates actually touch,
/// **plus exactly the halo the enabled filters read past them**, so the
/// footprint check covers what will be read and nothing more.
///
/// Three terms, each derived from a specific reader:
///
/// - `last_center + 1` -- the coordinate generator itself. The last output
///   row maps to source row `last_center`, and rows are counted from zero.
/// - `+ 1` when resampling is enabled ([`resample_bilinear`]) -- bilinear
///   interpolation reads `AxisSample::lower` **and** `upper = lower + 1`, so
///   the row below the last centre is a real read. Without this term the
///   last output row silently degrades to `HeldLast` and stops
///   interpolating, which is a wrong image rather than a refused one.
/// - `+ 1` when dither restoration is enabled ([`SourcePlane::restore_dither`])
///   -- its 3x3 neighbourhood reads one row below each restored pixel.
///
/// The two halos are `max`'d rather than summed: both name the single row
/// immediately below the last centre, and neither reads further.
///
/// This is the same decomposition `fn64-render-reference`'s
/// `vi_source_geometry_with_bottom_halo` performs
/// (`crates/fn64-render-reference/src/backend/vi_source.rs:86-104`), reached
/// here from the readers rather than transcribed: a differential against
/// that backend is what exposed the missing terms, on the last output row
/// only, which is exactly where a missing bottom halo shows.
fn source_rows(registers: ViScanoutRegisters, output_height: u32, filters: ViFilterControl) -> u64 {
    let resample = registers.resample();
    let last_output = u64::from(output_height - 1);
    let last = u64::from(resample.y.offset_u2_10())
        .checked_add(
            last_output
                .checked_mul(u64::from(resample.y.step_u2_10()))
                .expect("VI vertical coordinate product overflow"),
        )
        .expect("VI vertical coordinate sum overflow");
    let last_center = last >> ViScaleAxis::FRACTION_BITS;
    let resample_extra = u64::from(filters.antialias_mode.resampling_enabled());
    let restoration_halo = u64::from(filters.dither_filter);
    last_center
        .checked_add(resample_extra.max(restoration_halo))
        .and_then(|value| value.checked_add(1))
        .expect("VI source row count overflow")
}

/// Reject every filter this scanout does not implement, by name, before any
/// memory is read.
///
/// Ordering is deliberate: the reserved pixel type is checked first because
/// it is a malformed register image rather than an unimplemented filter, and
/// `ViPixelType::Reserved` has no byte width to bound-check against.
///
/// A **blanked** field is then admitted unconditionally, before any filter
/// test. This is not a fallback that masks a refusal -- it is the ordering
/// the filters' own domain requires. Every remaining refusal names a filter
/// that transforms *scanned-out pixels*: silhouette AA and dither
/// restoration read per-pixel coverage, divot medians three post-filter
/// samples, resampling interpolates between adjacent source samples, and
/// gamma maps sampled components. A blanked field scans out no source at
/// all -- `SourceGeometry::derive` returns `Ok(None)` for it and the field
/// is black at the programmed rectangle -- so there are no pixels for any
/// of those filters to transform, and no output this module could get
/// wrong. Refusing here would report an unimplemented filter for a field
/// whose contents that filter cannot influence.
///
/// This mirrors `fn64-render-reference`'s `scanout`
/// (`crates/fn64-render-reference/src/vi.rs:37-44`), which likewise returns
/// the cleared field on `blanked || pixel_type == Blank` *before* consulting
/// `silhouette_aa_enabled`, `dither_filter`, `divot`, or `gamma`. The two
/// backends agree on a blanked field precisely because both order it this
/// way; measured on the real WM2000 ROM, its first present latches
/// `AaResampleAlways` **with `ViPixelType::Blank`**, which the reference
/// blanks and this module previously refused.
///
/// **Measured on the real WM2000 ROM, `FN64_RENDER=wgpu`.** Fields 0-19 are
/// blanked; field 20 is the first with content and latches
/// `status=0x00013202`: `ViPixelType::Rgba16`, `ViAaMode::ResampleOnly`
/// (AA mode 2), `dither_filter = true`, and `gamma`, `gamma_dither`,
/// `divot`, `fade`, `repeat_line` all clear. Exactly two of the eight named
/// refusals were reachable, and both are now implemented: dither
/// restoration ([`restore_dither`]) and resampling ([`resample_bilinear`]).
/// Silhouette AA, divot, gamma, fade and repeat-line stay refusing by name
/// because WM2000 never selects them and this module still cannot produce
/// them. **Gamma dither is no longer among them**: its refusal claimed a
/// missing "retrace-seeded noise generator" that was already public in
/// `fn64_render::vi_public_filters`, which this module imports from, so it
/// is implemented rather than refused (see [`apply_gamma_dither`]). WM2000
/// still does not select it; the refusal was removed because it was false,
/// not because the census reached it.
fn admitted_filters(vi: ViPresentation) -> Result<(), ViScanoutRefusal> {
    let filters = vi.scanout.filters();
    if filters.pixel_type == ViPixelType::Reserved {
        return Err(ViScanoutRefusal::ReservedPixelType);
    }
    if vi.blanked || filters.pixel_type == ViPixelType::Blank {
        return Ok(());
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
    if filters.dither_filter && filters.pixel_type != ViPixelType::Rgba16 {
        return Err(ViScanoutRefusal::DitherRestorationNonRgba16);
    }
    if filters.divot {
        return Err(ViScanoutRefusal::Divot);
    }
    if filters.gamma {
        return Err(ViScanoutRefusal::Gamma);
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
                rows: source_rows(registers, output_height, filters),
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

    /// Coverage count in `1..=8` for one source pixel.
    ///
    /// The compatibility path without hidden memory reconstructs the public
    /// Programming Manual's non-RDP-write endpoints: visible bit set means 8,
    /// clear means 1. Production scanout supplies the backend's physical
    /// hidden map, which distinguishes every intermediate count 2..=7 after
    /// validating that the visible halfword still matches its coherence
    /// marker.
    ///
    /// RGBA32 carries coverage in the top three bits of its fourth byte,
    /// which the reference reads the same way.
    #[cfg(test)]
    fn coverage(
        self,
        memory: &fn64_runtime::PhysicalRdramRead<'_>,
        source_x: u64,
        source_y: u64,
    ) -> u8 {
        self.coverage_with_hidden(memory, None, source_x, source_y)
    }

    fn coverage_with_hidden(
        self,
        memory: &fn64_runtime::PhysicalRdramRead<'_>,
        hidden: Option<&crate::targets::RdramHiddenCoverage>,
        source_x: u64,
        source_y: u64,
    ) -> u8 {
        let address = self.pixel_address(source_x, source_y);
        let stored = match self.pixel_type {
            ViPixelType::Rgba16 => {
                let visible = memory.read_u16(address);
                if let Some(hidden) = hidden {
                    return hidden.rgba16_coverage(address.offset(), visible).count();
                }
                let low_bit = (visible & 1) as u8;
                (low_bit << 2) | if low_bit == 0 { 0 } else { 3 }
            }
            ViPixelType::Rgba32 => {
                memory.read_u8(
                    address
                        .checked_add(3)
                        .expect("VI RGBA32 coverage address overflow"),
                ) >> 5
            }
            ViPixelType::Blank | ViPixelType::Reserved | ViPixelType::Unspecified => {
                unreachable!("SourceGeometry::derive admits only Rgba16 and Rgba32")
            }
        };
        (stored & 7) + 1
    }

    /// The lane-mapped physical address of one source pixel.
    fn pixel_address(self, source_x: u64, source_y: u64) -> fn64_runtime::RdramAddr {
        let index = source_y * u64::from(self.stride_pixels) + source_x;
        let byte_offset = index * u64::from(self.bytes_per_pixel);
        let logical = u64::from(self.origin)
            .checked_add(byte_offset)
            .expect("VI source pixel address overflow");
        let logical = u32::try_from(logical).expect("validated VI source address exceeds u32");
        fn64_runtime::RdramAddr::from_offset(logical)
    }

    /// Read one source pixel through the lane-mapped physical capability and
    /// expand it to 8-bit RGBA.
    fn sample(
        self,
        memory: &fn64_runtime::PhysicalRdramRead<'_>,
        source_x: u64,
        source_y: u64,
    ) -> [u8; 4] {
        let address = self.pixel_address(source_x, source_y);
        match self.pixel_type {
            ViPixelType::Rgba16 => {
                let pixel = memory.read_u16(address);
                [
                    expand_five_bit(((pixel >> 11) & 0x1f) as u8),
                    expand_five_bit(((pixel >> 6) & 0x1f) as u8),
                    expand_five_bit(((pixel >> 1) & 0x1f) as u8),
                    // RGBA16's low bit is stored coverage bit 2. This
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
#[cfg(test)]
pub(crate) fn scan_out_guest_rdram(
    vi: ViPresentation,
    memory: &fn64_runtime::PhysicalRdramRead<'_>,
) -> Result<PresentedField, RenderError> {
    scan_out_guest_rdram_with_hidden(vi, memory, None)
}

/// Build the shell-compatible pre-filter RGBA5551 source selected by one live
/// VI image. This is the raw color-image source: VI resampling, post-filters,
/// and the RDP hidden-coverage sidecar have deliberately not been applied.
/// `None` is the explicit non-RGBA16 capability boundary; callers may then use
/// their existing backend-neutral fallback.
pub(crate) fn scan_out_rgba5551_source_field(
    vi: ViPresentation,
    memory: &fn64_runtime::PhysicalRdramRead<'_>,
) -> Result<Option<fn64_render::PresentedSourceField>, RenderError> {
    let Some(registers) = vi.scanout.registers() else {
        return Err(RenderError::Backend {
            backend: "render-wgpu-source-field",
            reason: "a live source field requires complete VI registers".to_string(),
        });
    };
    let Some(window) = registers.active_window() else {
        return Ok(None);
    };
    if !vi.blanked && registers.filters().pixel_type != ViPixelType::Rgba16 {
        return Ok(None);
    }
    let origin = registers.origin();
    let stride = registers.width();
    let height = window.output_height();
    if !origin.is_multiple_of(4) {
        return Err(RenderError::InvalidViSourceAlignment {
            origin,
            bytes_per_pixel: 2,
        });
    }
    let byte_len = usize::try_from(stride)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(2))
        .ok_or_else(|| RenderError::Backend {
            backend: "render-wgpu-source-field",
            reason: "RGBA5551 source-field footprint overflow".to_string(),
        })?;
    let end = usize::try_from(origin)
        .ok()
        .and_then(|origin| origin.checked_add(byte_len))
        .ok_or_else(|| RenderError::Backend {
            backend: "render-wgpu-source-field",
            reason: "RGBA5551 source-field end overflow".to_string(),
        })?;
    if end > memory.len() {
        return Err(RenderError::InvalidViSourceBounds {
            origin,
            stride_pixels: stride,
            rows: u64::from(height),
            bytes_per_pixel: 2,
            rdram_len: memory.len(),
        });
    }
    let pixels = usize::try_from(stride)
        .expect("VI stride fits usize")
        .checked_mul(usize::try_from(height).expect("VI height fits usize"))
        .expect("validated source-field pixel count");
    let mut rgba8 = Vec::with_capacity(pixels * 4);
    if vi.blanked || registers.filters().pixel_type == ViPixelType::Blank {
        rgba8.resize(pixels * 4, 0);
        for alpha in rgba8.iter_mut().skip(3).step_by(4) {
            *alpha = 255;
        }
    } else {
        for pixel in 0..pixels {
            let offset = u32::try_from(pixel.checked_mul(2).expect("pixel offset overflow"))
                .expect("validated source-field byte offset fits u32");
            let value = memory.read_u16(
                fn64_runtime::RdramAddr::from_offset(origin)
                    .checked_add(offset)
                    .expect("validated source-field address"),
            );
            rgba8.extend_from_slice(&fn64_render::presented_rgba5551_to_rgba8888(value));
        }
    }
    fn64_render::PresentedSourceField::rgba5551(vi, origin, stride, height, rgba8).map(Some)
}

pub(crate) fn scan_out_guest_rdram_with_hidden(
    vi: ViPresentation,
    memory: &fn64_runtime::PhysicalRdramRead<'_>,
    hidden: Option<&crate::targets::RdramHiddenCoverage>,
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
        let field = PresentedField {
            width,
            height,
            rgba8,
            presentation: vi,
        };
        report_field(&field, vi.scanout.filters());
        return Ok(field);
    };

    geometry.validate(memory.len())?;

    let ViResampleControl { x, y, .. } = registers.resample();
    let filters = vi.scanout.filters();
    let mut plane = SourcePlane::load_with_hidden(geometry, memory, hidden);
    if filters.dither_filter {
        // `admitted_filters` proved `pixel_type == Rgba16` before this point;
        // bit 16 over any other source is refused by
        // `DitherRestorationNonRgba16`.
        plane.restore_dither();
    }
    let mut rgba8 = if filters.antialias_mode.resampling_enabled() {
        resample_bilinear(&plane, x, y, output_width, output_height)
    } else {
        replicate(&plane, x, y, output_width, output_height)
    };
    if filters.gamma_dither {
        apply_gamma_dither(&mut rgba8, vi.noise_seed);
    }

    let field = PresentedField {
        width: output_width,
        height: output_height,
        rgba8,
        presentation: vi,
    };
    report_field(&field, filters);
    Ok(field)
}

/// Validate the complete live scanout envelope without reading source pixels.
///
/// The host-only unconsumed-scanout experiment uses this to preserve the same
/// register/filter/geometry refusal boundary while measuring only the source
/// traversal and field construction it proposes to remove from a shell that
/// independently reads guest RDRAM. It deliberately does not validate hidden
/// coverage: that authority is consulted only while actual pixels are read.
#[cfg(feature = "host-gpu-tests")]
pub(crate) fn validate_guest_rdram_scanout(
    vi: ViPresentation,
    memory: &fn64_runtime::PhysicalRdramRead<'_>,
) -> Result<(), RenderError> {
    let Some(registers) = vi.scanout.registers() else {
        return Err(RenderError::Backend {
            backend: "render-wgpu-vi-scanout",
            reason: "physical VI presentation requires a live fourteen-word register image; \
                     ViScanoutState::BackendOnly carries no origin, stride, or active window"
                .to_string(),
        });
    };
    admitted_filters(vi).map_err(ViScanoutRefusal::into_error)?;
    if let Some((geometry, _, _)) = SourceGeometry::derive(vi, registers)? {
        geometry.validate(memory.len())?;
    }
    Ok(())
}

/// VI STATUS bit 2's gamma dither: stochastically round each RGB channel of
/// the presented field to seven bits, then expand it back to this module's
/// eight-bit storage.
///
/// **Both halves come from `fn64_render::vi_public_filters`, the shared
/// crate this module already imports one filter from.**
/// [`gamma_dither_quantize_bounded_v1`] is the quantizer and
/// [`reference_noise_bit_v1`] is the bit stream, and
/// `fn64-render-reference`'s `apply_gamma_dither`
/// (`crates/fn64-render-reference/src/vi.rs:590-600`) is the same two calls
/// over the same seed, pixel index and channel index. Neither lane
/// implements a generator of its own; there is exactly one, and it is
/// public.
///
/// This module previously refused bit 2 outright, on the stated ground that
/// gamma dither "needs a retrace-seeded noise generator this module does not
/// own." The generator was already public in the crate this file's `use`
/// statement names, alongside `restore_rgba16_component_bounded_v1` -- which
/// this module does call. The refusal was a stale nonclaim, not a
/// capability gap, so it is removed rather than narrowed.
///
/// **What this is NOT.** `VI_PUBLIC_FILTER_POLICY_ID`'s own header says it:
/// public documentation specifies fresh random low-bit noise before the
/// final seven-bit quantization but publishes no silicon generator, seed, or
/// advancement. `reference_noise_bit_v1` is fn64's declared deterministic
/// policy (`fn64.vi-public-filters.bounded-v1`), and this function inherits
/// that status exactly -- an executable cross-backend contract, not a claim
/// about the RDP's random stream. The quantizer half *is* the documented
/// mechanism; only the bit source is policy.
///
/// Alpha is untouched, matching both the reference and the public
/// description: gamma dither is an RGB output-stage filter.
///
/// Ordering: last, after resampling. The reference composes it the same way
/// (`vi.rs:126-133`, gamma then gamma dither, both after `apply_resampling`),
/// and it is the only sensible order for a filter whose whole purpose is to
/// shape the final quantization to the DAC. `ViScanoutRefusal::Gamma`
/// still refuses bit 3, so the reference's gamma-then-dither pair is not
/// reachable here in its two-filter form; the relative order is recorded so
/// that a later gamma implementation inserts itself before this call rather
/// than after.
fn apply_gamma_dither(rgba8: &mut [u8], seed: u64) {
    for (pixel_index, pixel) in rgba8.chunks_exact_mut(4).enumerate() {
        for (channel_index, channel) in pixel[..3].iter_mut().enumerate() {
            *channel = gamma_dither_quantize_bounded_v1(
                *channel,
                reference_noise_bit_v1(seed, pixel_index as u64, channel_index as u8),
            );
        }
    }
}

/// Emit one line per presented field under `FN64_VI_FIELD_DIGEST`.
///
/// This exists because the shell's own window blit does **not** go through
/// this module -- `fn64-shell`'s `present()` reads guest RDRAM directly with
/// `rgba5551_to_rgba8888`, so an F2 screenshot shows that path's output and
/// is not evidence about this one. Without this line, "the filters ran on
/// the real ROM" would be an inference from the absence of a refusal rather
/// than an observation of the pixels.
///
/// The digest is FNV-1a over the presented bytes. It is a comparison key
/// for "did this field change / is it uniform", never a correctness claim.
fn report_field(field: &PresentedField, filters: ViFilterControl) {
    if std::env::var_os("FN64_VI_FIELD_DIGEST").is_none() {
        return;
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    static ORDINAL: AtomicU64 = AtomicU64::new(0);
    let ordinal = ORDINAL.fetch_add(1, Ordering::Relaxed);
    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    for byte in &field.rgba8 {
        digest ^= u64::from(*byte);
        digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let first = field.rgba8.first_chunk::<4>().copied().unwrap_or([0; 4]);
    let uniform = field
        .rgba8
        .chunks_exact(4)
        .all(|pixel| pixel == first.as_slice());
    eprintln!(
        "[fn64-vi-scanout] field {ordinal}: {}x{} pixel_type={:?} aa={:?} dither_filter={} \
         uniform={uniform} first={first:?} digest={digest:#018x}",
        field.width,
        field.height,
        filters.pixel_type,
        filters.antialias_mode,
        filters.dither_filter
    );
}

/// The source rectangle expanded to eight-bit RGBA, held whole so a
/// neighborhood filter can read pixels the output rectangle never samples.
///
/// The VI's post-filters run over the *source* plane, before the coordinate
/// generators pick output samples: US 6,166,748 Figures 34A/35M/35N order
/// the filtered line pair ahead of the interpolators, and
/// `fn64-render-reference`'s `scanout` composes them the same way
/// (`filter_scanout` mutates the source-sized buffer, then `apply_resampling`
/// consumes it). Restoring after resampling would read interpolated
/// neighbors that carry no five-bit dither, which is a different filter.
struct SourcePlane {
    width: usize,
    height: usize,
    /// Row-major RGBA8, `width * height * 4` bytes.
    rgba8: Vec<u8>,
    /// Per-pixel coverage count in `1..=8`, parallel to `rgba8`.
    ///
    /// Production loads this from physical hidden memory, after checking the
    /// current visible halfword against the stored coherence marker. Tests and
    /// compatibility callers that provide no hidden map reconstruct only the
    /// documented non-RDP-write endpoints 1 and 8 from the visible low bit.
    coverage: Vec<u8>,
}

impl SourcePlane {
    #[cfg(test)]
    fn load(geometry: SourceGeometry, memory: &fn64_runtime::PhysicalRdramRead<'_>) -> Self {
        Self::load_with_hidden(geometry, memory, None)
    }

    fn load_with_hidden(
        geometry: SourceGeometry,
        memory: &fn64_runtime::PhysicalRdramRead<'_>,
        hidden: Option<&crate::targets::RdramHiddenCoverage>,
    ) -> Self {
        let width = geometry.stride_pixels as usize;
        let height = usize::try_from(geometry.rows).expect("validated VI source rows exceed usize");
        let mut rgba8 = Vec::with_capacity(width * height * 4);
        let mut coverage = Vec::with_capacity(width * height);
        for source_y in 0..height as u64 {
            for source_x in 0..width as u64 {
                rgba8.extend_from_slice(&geometry.sample(memory, source_x, source_y));
                coverage.push(geometry.coverage_with_hidden(memory, hidden, source_x, source_y));
            }
        }
        Self {
            width,
            height,
            rgba8,
            coverage,
        }
    }

    /// US 5,699,079's dither-restoration filter, VI STATUS bit 16.
    ///
    /// Each full-coverage RGBA16 component is recovered from the signed
    /// comparisons against its available 3x3 neighbors. The arithmetic is
    /// **not reimplemented here**: it is
    /// `fn64_render::vi_public_filters::restore_rgba16_component_bounded_v1`,
    /// the identical shared entry point `fn64-render-reference`'s
    /// `filter_scanout` calls, so the two backends cannot drift.
    ///
    /// Neighbors are read from a snapshot taken before any pixel is written,
    /// matching the reference's `let original = output.pixels.clone()` --
    /// filtering in place would feed already-restored components back in as
    /// neighbors and make the result scan-order dependent.
    fn restore_dither(&mut self) {
        self.restore_dither_with_parallelism(parallel_vi_dither_enabled());
    }

    fn restore_dither_with_parallelism(&mut self, parallel: bool) {
        self.restore_dither_with_options(parallel, grouped_vi_dither_enabled());
    }

    fn restore_dither_with_options(&mut self, parallel: bool, grouped_rgb: bool) {
        let original = self.rgba8.clone();
        let row_bytes = self.width * 4;
        if parallel {
            self.rgba8
                .par_chunks_mut(row_bytes)
                .enumerate()
                .for_each(|(y, row)| {
                    restore_dither_row(
                        row,
                        y,
                        self.width,
                        self.height,
                        &original,
                        &self.coverage,
                        grouped_rgb,
                    );
                });
        } else {
            for (y, row) in self.rgba8.chunks_mut(row_bytes).enumerate() {
                restore_dither_row(
                    row,
                    y,
                    self.width,
                    self.height,
                    &original,
                    &self.coverage,
                    grouped_rgb,
                );
            }
        }
    }

    fn component(&self, x: usize, y: usize, channel: usize) -> u8 {
        self.rgba8[(y * self.width + x) * 4 + channel]
    }
}

fn parallel_vi_dither_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("FN64_PARALLEL_VI_DITHER") {
        Ok(value) if value == "0" => false,
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("FN64_PARALLEL_VI_DITHER must be exactly 0 or 1, got {value:?}"),
        Err(std::env::VarError::NotPresent) => true,
        Err(error) => panic!("FN64_PARALLEL_VI_DITHER is not valid Unicode: {error}"),
    })
}

fn grouped_vi_dither_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("FN64_GROUPED_VI_DITHER") {
        Ok(value) if value == "0" => false,
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("FN64_GROUPED_VI_DITHER must be exactly 0 or 1, got {value:?}"),
        Err(std::env::VarError::NotPresent) => true,
        Err(error) => panic!("FN64_GROUPED_VI_DITHER is not valid Unicode: {error}"),
    })
}

fn typed_vi_dither_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("FN64_TYPED_VI_DITHER") {
        Ok(value) if value == "0" => false,
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("FN64_TYPED_VI_DITHER must be exactly 0 or 1, got {value:?}"),
        Err(std::env::VarError::NotPresent) => true,
        Err(error) => panic!("FN64_TYPED_VI_DITHER is not valid Unicode: {error}"),
    })
}

fn restore_dither_row(
    row: &mut [u8],
    y: usize,
    width: usize,
    height: usize,
    original: &[u8],
    coverage: &[u8],
    grouped_rgb: bool,
) {
    if grouped_rgb && typed_vi_dither_enabled() {
        return restore_dither_row_typed(row, y, width, height, original, coverage);
    }
    for x in 0..width {
        let pixel = y * width + x;
        if coverage[pixel] != 8 {
            continue;
        }
        if grouped_rgb {
            let center_offset = pixel * 4;
            let center = [
                original[center_offset] >> 3,
                original[center_offset + 1] >> 3,
                original[center_offset + 2] >> 3,
            ];
            let mut neighbors = [[0u8; 3]; 8];
            let mut count = 0;
            for neighbor_y in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for neighbor_x in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    if neighbor_x == x && neighbor_y == y {
                        continue;
                    }
                    let offset = (neighbor_y * width + neighbor_x) * 4;
                    neighbors[count] = [
                        original[offset] >> 3,
                        original[offset + 1] >> 3,
                        original[offset + 2] >> 3,
                    ];
                    count += 1;
                }
            }
            let restored = restore_rgba16_rgb_bounded_v1(center, &neighbors[..count]);
            row[x * 4..x * 4 + 3].copy_from_slice(&restored);
        } else {
            for channel in 0..3 {
                let center = original[pixel * 4 + channel] >> 3;
                let mut neighbors = [0u8; 8];
                let mut count = 0;
                for neighbor_y in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for neighbor_x in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                        if neighbor_x == x && neighbor_y == y {
                            continue;
                        }
                        neighbors[count] =
                            original[(neighbor_y * width + neighbor_x) * 4 + channel] >> 3;
                        count += 1;
                    }
                }
                row[x * 4 + channel] =
                    restore_rgba16_component_bounded_v1(center, &neighbors[..count]);
            }
        }
    }
}

fn restore_dither_row_typed(
    row: &mut [u8],
    y: usize,
    width: usize,
    height: usize,
    original: &[u8],
    coverage: &[u8],
) {
    for x in 0..width {
        let pixel = y * width + x;
        if coverage[pixel] != 8 {
            continue;
        }
        let center_offset = pixel * 4;
        let center = Rgba16Rgb5::from_expanded_rgba8([
            original[center_offset],
            original[center_offset + 1],
            original[center_offset + 2],
        ]);
        let mut neighbors = [Rgba16Rgb5::from_expanded_rgba8([0; 3]); 8];
        let mut count = 0;
        for neighbor_y in y.saturating_sub(1)..=(y + 1).min(height - 1) {
            for neighbor_x in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                if neighbor_x == x && neighbor_y == y {
                    continue;
                }
                let offset = (neighbor_y * width + neighbor_x) * 4;
                neighbors[count] = Rgba16Rgb5::from_expanded_rgba8([
                    original[offset],
                    original[offset + 1],
                    original[offset + 2],
                ]);
                count += 1;
            }
        }
        let restored = restore_rgba16_rgb5_bounded_v1(center, &neighbors, count);
        row[x * 4..x * 4 + 3].copy_from_slice(&restored);
    }
}

/// One axis position resolved to a lower/upper source pair plus the U0.10
/// weight between them.
///
/// This is the same `AxisSample` split `fn64-render-reference` uses
/// (`crates/fn64-render-reference/src/vi.rs:390-419`), including its
/// `HeldLast` high-edge rule: at or past the last source sample both
/// endpoints collapse onto it and the fraction is forced to zero, so the
/// held edge is a repeat rather than an extrapolation. Recomputing it here
/// rather than importing keeps this module's U2.10 multiply-not-divide
/// convention (see the module header) as the single coordinate authority.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct AxisSample {
    lower: usize,
    upper: usize,
    fraction_u0_10: u16,
}

impl AxisSample {
    fn from_output(index: u32, axis: ViScaleAxis, source_extent: usize) -> Self {
        assert!(
            source_extent > 0,
            "VI resampling requires a nonempty source axis"
        );
        let position = u64::from(axis.offset_u2_10())
            .checked_add(
                u64::from(index)
                    .checked_mul(u64::from(axis.step_u2_10()))
                    .expect("VI source coordinate product overflow"),
            )
            .expect("VI source coordinate sum overflow");
        let integer = position >> ViScaleAxis::FRACTION_BITS;
        let last = source_extent - 1;
        if integer >= last as u64 {
            return Self {
                lower: last,
                upper: last,
                fraction_u0_10: 0,
            };
        }
        let lower = usize::try_from(integer).expect("in-range VI source coordinate exceeds usize");
        Self {
            lower,
            upper: lower + 1,
            fraction_u0_10: (position & u64::from(ViScaleAxis::ONE - 1)) as u16,
        }
    }
}

/// Round-to-nearest U2.10 linear interpolation.
///
/// Byte-for-byte `fn64-render-reference`'s `interpolate_u2_10`
/// (`crates/fn64-render-reference/src/vi.rs:434-443`), including the
/// `+ ONE / 2` rounding bias. The patents specify linear interpolation but
/// not the accumulator's tie behavior, so this shares fn64's bounded policy
/// rather than inventing a second one.
fn interpolate_u2_10(lower: u8, upper: u8, fraction_u0_10: u16) -> u8 {
    debug_assert!(fraction_u0_10 < ViScaleAxis::ONE);
    let upper_weight = u32::from(fraction_u0_10);
    let lower_weight = u32::from(ViScaleAxis::ONE) - upper_weight;
    ((u32::from(lower) * lower_weight
        + u32::from(upper) * upper_weight
        + u32::from(ViScaleAxis::ONE / 2))
        / u32::from(ViScaleAxis::ONE)) as u8
}

/// VI AA modes 0, 1 and 2: bilinear resampling, vertical pass then
/// horizontal.
///
/// The pass order is US 6,166,748 Figure 34A's and the reference's: vertical
/// interpolation between successive filtered lines produces a
/// `output_height x source_width` intermediate, and the horizontal pass then
/// interpolates that to the output width. Doing both in one pass over the
/// source would apply the horizontal weight to unfiltered rather than
/// vertically-blended samples.
///
/// Alpha is interpolated with the colour channels, matching the reference's
/// "all four stored host channels share this interpolation so identity
/// scanout preserves alpha". For an RGBA16 source every alpha is already
/// 255, so the pass is an identity on that channel.
fn resample_bilinear(
    plane: &SourcePlane,
    x_axis: ViScaleAxis,
    y_axis: ViScaleAxis,
    output_width: u32,
    output_height: u32,
) -> Vec<u8> {
    let width = output_width as usize;
    let height = output_height as usize;
    let mut vertical = vec![0u8; height * plane.width * 4];
    for output_y in 0..height {
        let sample = AxisSample::from_output(output_y as u32, y_axis, plane.height);
        for source_x in 0..plane.width {
            let destination = (output_y * plane.width + source_x) * 4;
            for channel in 0..4 {
                vertical[destination + channel] = interpolate_u2_10(
                    plane.component(source_x, sample.lower, channel),
                    plane.component(source_x, sample.upper, channel),
                    sample.fraction_u0_10,
                );
            }
        }
    }

    let mut rgba8 = vec![0u8; width * height * 4];
    for output_y in 0..height {
        for output_x in 0..width {
            let sample = AxisSample::from_output(output_x as u32, x_axis, plane.width);
            let destination = (output_y * width + output_x) * 4;
            let lower = (output_y * plane.width + sample.lower) * 4;
            let upper = (output_y * plane.width + sample.upper) * 4;
            for channel in 0..4 {
                rgba8[destination + channel] = interpolate_u2_10(
                    vertical[lower + channel],
                    vertical[upper + channel],
                    sample.fraction_u0_10,
                );
            }
        }
    }
    rgba8
}

/// VI AA mode 3: nearest-neighbor replication.
///
/// Preserves this module's original sampling exactly -- the coordinate
/// generators still run, but the lower resident sample is copied without
/// interpolation, which is `AxisSample::lower` and therefore the same index
/// [`source_index`] produced.
fn replicate(
    plane: &SourcePlane,
    x_axis: ViScaleAxis,
    y_axis: ViScaleAxis,
    output_width: u32,
    output_height: u32,
) -> Vec<u8> {
    let width = output_width as usize;
    let mut rgba8 = Vec::with_capacity(width * (output_height as usize) * 4);
    for output_y in 0..output_height {
        let source_y = AxisSample::from_output(output_y, y_axis, plane.height).lower;
        for output_x in 0..output_width {
            let source_x = AxisSample::from_output(output_x, x_axis, plane.width).lower;
            let offset = (source_y * plane.width + source_x) * 4;
            rgba8.extend_from_slice(&plane.rgba8[offset..offset + 4]);
        }
    }
    rgba8
}

#[cfg(test)]
mod tests;
