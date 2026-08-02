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

use super::*;
use super::hidden_bits::*;
use super::validate::*;
use super::framebuffer_io::*;
use super::imp::*;
use super::render_backend::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct ViSourceGeometry {
    pub(super) origin: u32,
    pub(super) stride_pixels: u32,
    pub(super) rows: u64,
    bytes_per_pixel: u8,
    layout: gbi::ColorImageLayout,
}

/// Add the deterministic reference filter's bottom halo to the public
/// programmed span. The public patents establish the filter topology but not
/// out-of-window bus fetches, so this remains a reference policy rather than a
/// native RT64 or silicon footprint claim.
pub(super) fn reference_vi_source_geometry(
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

pub(super) fn vi_source_geometry_with_bottom_halo(
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

pub(super) fn load_vi_source(
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

pub(super) fn validate_vi_source_footprint(
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
