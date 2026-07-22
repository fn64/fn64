//! Backend-neutral physical VI source footprint.
//!
//! This module stops at the rows selected by the public coordinate generator.
//! Reference-only restoration and coverage-filter halos belong to the
//! software renderer; native backends own their internal bus/filter fetches.

use crate::{RenderError, ViPixelType, ViPresentation, ViScaleAxis};

/// Physical RDRAM extent selected by one live VI register image.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViSourceFootprint {
    pub origin: u32,
    pub stride_pixels: u32,
    pub rows: u64,
    pub bytes_per_pixel: u8,
}

impl ViSourceFootprint {
    /// Prove that every selected source byte lies inside physical RDRAM.
    pub fn validate_rdram_len(self, rdram_len: usize) -> Result<(), RenderError> {
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
}

/// Derive the public source rows selected at one presentation boundary.
///
/// The lower interpolation sample is included when resampling is enabled.
/// Fade and repeat-line replace the vertical generator and therefore select
/// exactly two or one source rows, respectively.
/// Backend-specific filter halos are deliberately excluded.
pub fn programmed_vi_source_footprint(
    vi: ViPresentation,
) -> Result<Option<ViSourceFootprint>, RenderError> {
    let filters = vi.scanout.filters();
    if filters.pixel_type == ViPixelType::Reserved {
        return Err(RenderError::Backend {
            backend: "vi-source-footprint",
            reason: "VI STATUS selects reserved pixel type 1".to_owned(),
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
    let bytes_per_pixel = match filters.pixel_type {
        ViPixelType::Rgba16 => 2,
        ViPixelType::Rgba32 => 4,
        ViPixelType::Blank | ViPixelType::Reserved | ViPixelType::Unspecified => unreachable!(),
    };
    let rows = if vi.fade.is_some() {
        2
    } else if vi.repeat_line {
        1
    } else {
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
        let last_center = last_u10 >> ViScaleAxis::FRACTION_BITS;
        let sample_extra = u64::from(filters.antialias_mode.resampling_enabled());
        last_center
            .checked_add(sample_extra)
            .and_then(|value| value.checked_add(1))
            .expect("VI source row count overflow")
    };
    Ok(Some(ViSourceFootprint {
        origin: registers.origin(),
        stride_pixels: registers.width(),
        rows,
        bytes_per_pixel,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ViScanoutRegisters, ViScanoutState};

    fn presentation(status: u32, origin: u32, width: u32, y_scale: u32) -> ViPresentation {
        let mut words = [0; ViScanoutRegisters::WORD_COUNT];
        words[0] = status;
        words[1] = origin;
        words[2] = width;
        words[9] = 8;
        words[10] = 2;
        words[12] = u32::from(ViScaleAxis::ONE);
        words[13] = y_scale;
        ViPresentation {
            scanout: ViScanoutState::Registers(ViScanoutRegisters::from_words(words)),
            ..ViPresentation::default()
        }
    }

    #[test]
    fn programmed_rows_include_only_the_public_resampling_sample() {
        let vi = presentation(2, 0x100, 8, u32::from(ViScaleAxis::ONE));
        let footprint = programmed_vi_source_footprint(vi).unwrap().unwrap();
        assert_eq!(footprint.rows, 2);
        assert_eq!(footprint.bytes_per_pixel, 2);
        footprint.validate_rdram_len(0x120).unwrap();
    }

    #[test]
    fn blank_and_inactive_images_do_not_claim_physical_bytes() {
        let blank = presentation(0, u32::MAX, 8, u32::from(ViScaleAxis::ONE));
        assert_eq!(programmed_vi_source_footprint(blank).unwrap(), None);

        let mut inactive = presentation(2, u32::MAX, 8, u32::from(ViScaleAxis::ONE));
        inactive.blanked = true;
        assert_eq!(programmed_vi_source_footprint(inactive).unwrap(), None);
    }

    #[test]
    fn out_of_bounds_footprint_is_named() {
        let vi = presentation(3, 0x100, 8, u32::from(ViScaleAxis::ONE));
        let footprint = programmed_vi_source_footprint(vi).unwrap().unwrap();
        assert!(matches!(
            footprint.validate_rdram_len(0x13f),
            Err(RenderError::InvalidViSourceBounds { .. })
        ));
    }

    #[test]
    fn fade_and_repeat_line_replace_the_vertical_coordinate_generator() {
        let mut fade = presentation(2, 0x100, 8, u32::from(ViScaleAxis::ONE));
        fade.fade = Some(512);
        fade.repeat_line = true;
        let footprint = programmed_vi_source_footprint(fade).unwrap().unwrap();
        assert_eq!(footprint.rows, 2);
        footprint.validate_rdram_len(0x120).unwrap();
        assert!(matches!(
            footprint.validate_rdram_len(0x11f),
            Err(RenderError::InvalidViSourceBounds { .. })
        ));

        let mut repeat = presentation(2, 0x100, 8, u32::from(ViScaleAxis::ONE));
        repeat.repeat_line = true;
        let footprint = programmed_vi_source_footprint(repeat).unwrap().unwrap();
        assert_eq!(footprint.rows, 1);
        footprint.validate_rdram_len(0x110).unwrap();
        assert!(matches!(
            footprint.validate_rdram_len(0x10f),
            Err(RenderError::InvalidViSourceBounds { .. })
        ));
    }
}
