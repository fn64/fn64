//! Deterministic digital VI scanout for the reference backend.
//!
//! This module stops at eight-bit RGB before the video DAC. Public hardware
//! descriptions specify the mechanisms below, but not the silicon gamma ROM,
//! random generator/seed, analog encoding, or complete AA/resampling fixed-
//! point arithmetic. The integer gamma curve and SplitMix64-derived noise are
//! therefore explicit reproducibility policies, not silicon-identical claims.

use crate::{gbi, raster::Framebuffer};
use fn64_render::{
    vi_public_filters::{gamma_dither_quantize_bounded_v1, reference_noise_bit_v1},
    RenderError, ViPixelType, ViPresentation, ViResampleControl, ViScaleAxis,
};

pub(crate) fn scanout(
    source: &Framebuffer,
    presentation: ViPresentation,
) -> Result<Framebuffer, RenderError> {
    let filters = presentation.scanout.filters();
    let registers = presentation.scanout.registers();
    let active_window = registers.and_then(|registers| registers.active_window());
    let mut output = source.clone();
    if filters.pixel_type == ViPixelType::Reserved {
        return Err(RenderError::Backend {
            backend: "reference",
            reason: "VI STATUS selects reserved pixel type 1".to_string(),
        });
    }
    if registers.is_some() && active_window.is_none() {
        let mut inactive = output_with_geometry(source, 0, 0);
        inactive.clear(0, 0, 0, 255);
        return Ok(inactive);
    }
    if presentation.blanked || filters.pixel_type == ViPixelType::Blank {
        if let Some(window) = active_window {
            output = output_with_geometry(source, window.output_width(), window.output_height());
        }
        output.clear(0, 0, 0, 255);
        return Ok(output);
    }

    let row_bytes = usize::try_from(source.width)
        .expect("framebuffer width exceeds usize")
        .checked_mul(4)
        .expect("framebuffer row byte count overflow");
    let repeated_row = if let Some(factor) = presentation.fade {
        if source.height < 2 {
            return Err(RenderError::Backend {
                backend: "reference",
                reason: "osViFade requires at least two framebuffer rows".to_string(),
            });
        }
        let factor = u32::from(factor);
        let inverse = 0x03ff - factor;
        let mut row = vec![0u8; row_bytes];
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let top = x * 4;
            let bottom = row_bytes + top;
            for (channel, output_channel) in pixel.iter_mut().take(3).enumerate() {
                let mixed = u32::from(source.pixels[top + channel]) * inverse
                    + u32::from(source.pixels[bottom + channel]) * factor;
                *output_channel = u8::try_from((mixed + 0x01ff) / 0x03ff)
                    .expect("VI fade interpolation exceeds u8");
            }
            pixel[3] = 255;
        }
        Some(row)
    } else if presentation.repeat_line {
        Some(source.pixels[..row_bytes].to_vec())
    } else {
        None
    };

    if let Some(row) = repeated_row {
        for destination in output.pixels.chunks_exact_mut(row_bytes) {
            destination.copy_from_slice(&row);
        }
    }

    let silhouette_aa_enabled = filters.antialias_mode.silhouette_aa_enabled();
    if silhouette_aa_enabled || filters.dither_filter {
        let restoration_enabled = filters.dither_filter;
        if restoration_enabled
            && (filters.pixel_type != ViPixelType::Rgba16
                || source.color_layout() != gbi::ColorImageLayout::Rgba16)
        {
            return Err(RenderError::Backend {
                backend: "reference",
                reason: "VI dither restoration requires an RGBA16 scanout image".to_string(),
            });
        }
        let interlaced = registers.is_some_and(|registers| registers.resample().field.interlaced());
        filter_scanout(
            source,
            &mut output,
            interlaced,
            silhouette_aa_enabled,
            restoration_enabled,
        );
    }
    if filters.divot {
        apply_divot(source, &mut output);
    }
    match registers {
        Some(registers) if filters.antialias_mode.resampling_enabled() => {
            let window = active_window.expect("active VI register image lost its window");
            output = apply_resampling(
                &output,
                registers.resample(),
                window.output_width(),
                window.output_height(),
            );
        }
        Some(registers) => {
            let window = active_window.expect("active VI register image lost its window");
            output = apply_replication(
                &output,
                registers.resample(),
                window.output_width(),
                window.output_height(),
            );
        }
        None => {}
    }
    if filters.gamma {
        apply_gamma(&mut output);
    }
    if filters.gamma_dither {
        apply_gamma_dither(&mut output, presentation.noise_seed);
    }
    Ok(output)
}

/// US 5,742,277 Figure 11's checkerboard approximation of six equidistant
/// neighbors around a central sample. The two horizontal samples are two
/// pixels away; the upper and lower rows use the two diagonals.
const COVERAGE_AA_NEIGHBORS: [(isize, isize); 6] =
    [(-1, -1), (1, -1), (-2, 0), (2, 0), (-1, 1), (1, 1)];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct PartialCoverage(u8);

impl PartialCoverage {
    fn new(samples: u8) -> Self {
        assert!(
            (1..8).contains(&samples),
            "VI coverage AA requires a resident partial sample in 1..=7, got {samples}"
        );
        Self(samples)
    }

    /// US 5,742,277 Equation 4. The patent specifies normalized coverage but
    /// not the VI's integer rounding. Round-to-nearest over the eight public
    /// coverage samples is fn64's deterministic bounded-reference policy.
    fn blend(self, foreground: u8, background: u8) -> u8 {
        let foreground_weight = u16::from(self.0);
        let background_weight = 8 - foreground_weight;
        let weighted =
            foreground_weight * u16::from(foreground) + background_weight * u16::from(background);
        ((weighted + 4) / 8) as u8
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CoverageAaNeighborhood {
    /// At least three full-coverage samples remain after the patent's required
    /// rejection of partial neighbors, so rejecting one minimum and maximum
    /// still leaves a defined penultimate interval.
    Preferred { colors: [[u8; 3]; 6], len: usize },
    /// The public mechanism does not define a background estimate after fewer
    /// than three full samples remain. Preserve a named bounded expansion
    /// instead of fabricating extrema from partial or out-of-frame samples.
    InsufficientFullCoverage { available: usize },
}

impl CoverageAaNeighborhood {
    fn gather(source: &Framebuffer, original: &[u8], x: usize, y: usize, interlaced: bool) -> Self {
        let width = source.width as usize;
        let height = source.height as usize;
        let mut colors = [[0; 3]; 6];
        let mut len = 0;
        for (offset_x, mut offset_y) in COVERAGE_AA_NEIGHBORS {
            if interlaced {
                // US 5,742,277 Figure 11: because an interlaced field renders
                // every other row, its upper/lower checkerboard neighbors are
                // two framebuffer lines away.
                offset_y *= 2;
            }
            let Some(neighbor_x) = x.checked_add_signed(offset_x) else {
                continue;
            };
            let Some(neighbor_y) = y.checked_add_signed(offset_y) else {
                continue;
            };
            if neighbor_x >= width || neighbor_y >= height {
                continue;
            }
            let neighbor = neighbor_y * width + neighbor_x;
            if source.coverage_count(neighbor) != 8 {
                continue;
            }
            for (channel, component) in colors[len].iter_mut().enumerate() {
                *component = scanout_component(source, original, neighbor, channel);
            }
            len += 1;
        }
        if len >= 3 {
            Self::Preferred { colors, len }
        } else {
            Self::InsufficientFullCoverage { available: len }
        }
    }
}

fn expand_five_bit(value: u8) -> u8 {
    debug_assert!(value < 32);
    (value << 3) | (value >> 2)
}

fn scanout_component(source: &Framebuffer, original: &[u8], pixel: usize, channel: usize) -> u8 {
    let stored = original[pixel * 4 + channel];
    if source.color_layout() == gbi::ColorImageLayout::Rgba16 {
        expand_five_bit(stored >> 3)
    } else {
        stored
    }
}

/// US 5,742,277 steps 707-712 and claim 7: sort full-coverage neighbors,
/// reject one maximum and minimum, extend the remaining interval to include
/// the foreground, and reflect the foreground across that interval's midpoint
/// to estimate the hidden background. Inputs are already expanded to u8.
/// Saturation is an explicit host-output policy; the patent does not publish
/// the VI's fixed-point overflow behavior.
fn estimate_coverage_background(
    foreground: u8,
    colors: &[[u8; 3]; 6],
    len: usize,
    channel: usize,
) -> u8 {
    debug_assert!((3..=6).contains(&len));
    debug_assert!(channel < 3);
    let mut components = [0; 6];
    for index in 0..len {
        components[index] = colors[index][channel];
    }
    components[..len].sort_unstable();
    let penultimate_minimum = components[1];
    let penultimate_maximum = components[len - 2];
    let low = foreground.min(penultimate_minimum);
    let high = foreground.max(penultimate_maximum);
    (i16::from(low) + i16::from(high) - i16::from(foreground)).clamp(0, 255) as u8
}

/// Public VI filter selection. Enabled partial-coverage samples use
/// US 5,742,277's silhouette-AA estimator; independently, enabled RGBA16
/// full-coverage samples use US 5,699,079's restoration filter.
fn filter_scanout(
    source: &Framebuffer,
    output: &mut Framebuffer,
    interlaced: bool,
    silhouette_aa_enabled: bool,
    restoration_enabled: bool,
) {
    debug_assert!(!restoration_enabled || source.color_layout() == gbi::ColorImageLayout::Rgba16);
    let original = output.pixels.clone();
    let width = source.width as usize;
    let height = source.height as usize;
    for y in 0..height {
        for x in 0..width {
            let pixel = y * width + x;
            let out = &mut output.pixels[pixel * 4..pixel * 4 + 4];
            let coverage = source.coverage_count(pixel);
            if coverage < 8 {
                if !silhouette_aa_enabled {
                    continue;
                }
                let foreground_coverage = PartialCoverage::new(coverage);
                let neighborhood =
                    CoverageAaNeighborhood::gather(source, &original, x, y, interlaced);
                for (channel, output_component) in out.iter_mut().take(3).enumerate() {
                    let foreground = scanout_component(source, &original, pixel, channel);
                    *output_component = match &neighborhood {
                        CoverageAaNeighborhood::Preferred { colors, len } => {
                            let background =
                                estimate_coverage_background(foreground, colors, *len, channel);
                            foreground_coverage.blend(foreground, background)
                        }
                        CoverageAaNeighborhood::InsufficientFullCoverage { .. } => foreground,
                    };
                }
                continue;
            }
            if !restoration_enabled {
                continue;
            }
            for channel in 0..3 {
                let center = original[pixel * 4 + channel] >> 3;
                let mut restored = i16::from(center) << 3;
                for neighbor_y in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for neighbor_x in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                        if neighbor_x == x && neighbor_y == y {
                            continue;
                        }
                        let neighbor =
                            original[(neighbor_y * width + neighbor_x) * 4 + channel] >> 3;
                        restored += i16::from(neighbor.cmp(&center) as i8);
                    }
                }
                out[channel] = restored.clamp(0, 255) as u8;
            }
        }
    }
}

/// Exact host accumulation of a U2.10 register offset plus output-index times
/// a U2.10 register step. The integer width is deliberately unnamed: `u64` is
/// a checked host envelope, not a claim about a silicon accumulator.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct AxisPositionU10Fraction(u64);

impl AxisPositionU10Fraction {
    fn from_output(index: usize, axis: ViScaleAxis) -> Self {
        let step = u64::try_from(index)
            .expect("VI output coordinate exceeds u64")
            .checked_mul(u64::from(axis.step_u2_10()))
            .expect("VI resampling coordinate overflow");
        Self(
            u64::from(axis.offset_u2_10())
                .checked_add(step)
                .expect("VI resampling coordinate overflow"),
        )
    }

    fn integer(self) -> u64 {
        self.0 >> ViScaleAxis::FRACTION_BITS
    }

    fn fraction_u0_10(self) -> u16 {
        (self.0 & u64::from(ViScaleAxis::ONE - 1)) as u16
    }
}

/// Host border classification for one register-derived resampling position.
/// `HeldLast` names fn64's bounded high-edge clamp; the public patents do not
/// establish the silicon fetch outside the active source window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum AxisBoundary {
    Interpolating,
    HeldLast { requested_integer: u64 },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct AxisSample {
    lower: usize,
    upper: usize,
    fraction_u0_10: u16,
    boundary: AxisBoundary,
}

impl AxisSample {
    fn from_output(index: usize, axis: ViScaleAxis, source_extent: usize) -> Self {
        assert!(
            source_extent > 0,
            "VI resampling requires a nonempty source axis"
        );
        let position = AxisPositionU10Fraction::from_output(index, axis);
        let requested_integer = position.integer();
        let last = source_extent - 1;
        if requested_integer >= last as u64 {
            return Self {
                lower: last,
                upper: last,
                fraction_u0_10: 0,
                boundary: AxisBoundary::HeldLast { requested_integer },
            };
        }
        let lower = usize::try_from(requested_integer)
            .expect("in-range VI source coordinate exceeds usize");
        Self {
            lower,
            upper: lower + 1,
            fraction_u0_10: position.fraction_u0_10(),
            boundary: AxisBoundary::Interpolating,
        }
    }

    fn fraction_u0_10(self) -> u16 {
        match self.boundary {
            AxisBoundary::Interpolating => self.fraction_u0_10,
            AxisBoundary::HeldLast { .. } => {
                debug_assert_eq!(self.lower, self.upper);
                debug_assert_eq!(self.fraction_u0_10, 0);
                0
            }
        }
    }
}

/// The patents specify linear interpolation, but not the VI accumulator's
/// integer tie behavior. This is fn64's bounded U2.10 realization: retain all
/// ten register fraction bits and round the positive weighted sum to nearest.
fn interpolate_u2_10(lower: u8, upper: u8, fraction_u0_10: u16) -> u8 {
    debug_assert!(fraction_u0_10 < ViScaleAxis::ONE);
    let upper_weight = u32::from(fraction_u0_10);
    let lower_weight = u32::from(ViScaleAxis::ONE) - upper_weight;
    ((u32::from(lower) * lower_weight
        + u32::from(upper) * upper_weight
        + u32::from(ViScaleAxis::ONE / 2))
        / u32::from(ViScaleAxis::ONE)) as u8
}

/// US 6,166,748 Figures 34A/35M/35N: vertical linear interpolation between
/// successive filtered lines precedes horizontal linear interpolation between
/// neighboring pixels. H_START/V_START supply the output extent while X/Y
/// scale supply source positions. Out-of-range fetches clamp to the last source
/// sample; neither that border rule nor integer rounding is claimed as silicon
/// behavior. All four stored host channels share this interpolation so identity
/// scanout preserves alpha; that is a host-representation contract, not a VI
/// silicon-alpha claim.
fn apply_resampling(
    source: &Framebuffer,
    control: ViResampleControl,
    output_width: u32,
    output_height: u32,
) -> Framebuffer {
    let source_width = source.width as usize;
    let source_height = source.height as usize;
    let width = output_width as usize;
    let height = output_height as usize;
    assert!(
        source_width > 0 && source_height > 0 && width > 0 && height > 0,
        "VI resampling requires a nonempty framebuffer"
    );
    let mut output = output_with_geometry(source, output_width, output_height);
    let mut vertical = vec![0u8; height * source_width * 4];

    for y in 0..height {
        let sample = AxisSample::from_output(y, control.y, source_height);
        for x in 0..source_width {
            let destination = (y * source_width + x) * 4;
            let lower = (sample.lower * source_width + x) * 4;
            let upper = (sample.upper * source_width + x) * 4;
            for channel in 0..4 {
                vertical[destination + channel] = interpolate_u2_10(
                    source.pixels[lower + channel],
                    source.pixels[upper + channel],
                    sample.fraction_u0_10(),
                );
            }
        }
    }

    for y in 0..height {
        for x in 0..width {
            let sample = AxisSample::from_output(x, control.x, source_width);
            let destination = (y * width + x) * 4;
            let lower = (y * source_width + sample.lower) * 4;
            let upper = (y * source_width + sample.upper) * 4;
            for channel in 0..4 {
                output.pixels[destination + channel] = interpolate_u2_10(
                    vertical[lower + channel],
                    vertical[upper + channel],
                    sample.fraction_u0_10(),
                );
            }
        }
    }
    output
}

/// Public AA mode 3 selects replication instead of filtered resampling. It
/// still consumes the programmed X/Y coordinate generators and active output
/// extent, but chooses the lower resident sample without interpolation.
fn apply_replication(
    source: &Framebuffer,
    control: ViResampleControl,
    output_width: u32,
    output_height: u32,
) -> Framebuffer {
    let source_width = source.width as usize;
    let source_height = source.height as usize;
    let width = output_width as usize;
    let height = output_height as usize;
    assert!(
        source_width > 0 && source_height > 0 && width > 0 && height > 0,
        "VI replication requires nonempty source and output geometry"
    );
    let mut output = output_with_geometry(source, output_width, output_height);
    for y in 0..height {
        let source_y = AxisSample::from_output(y, control.y, source_height).lower;
        for x in 0..width {
            let source_x = AxisSample::from_output(x, control.x, source_width).lower;
            let source_offset = (source_y * source_width + source_x) * 4;
            let destination = (y * width + x) * 4;
            output.pixels[destination..destination + 4]
                .copy_from_slice(&source.pixels[source_offset..source_offset + 4]);
        }
    }
    output
}

fn output_with_geometry(source: &Framebuffer, width: u32, height: u32) -> Framebuffer {
    let mut output = Framebuffer::new(width, height);
    output.set_color_layout(source.color_layout());
    output
}

/// US 6,166,748, Video Interface: pixels on or next to a silhouette edge use
/// the median of the left, center, and right post-filter samples.
fn apply_divot(source: &Framebuffer, output: &mut Framebuffer) {
    if source.width < 3 {
        return;
    }
    let original = output.pixels.clone();
    let width = source.width as usize;
    for y in 0..source.height as usize {
        for x in 1..width - 1 {
            let pixel = y * width + x;
            if (pixel - 1..=pixel + 1).all(|sample| source.coverage_count(sample) == 8) {
                continue;
            }
            for channel in 0..3 {
                let mut samples = [
                    original[(pixel - 1) * 4 + channel],
                    original[pixel * 4 + channel],
                    original[(pixel + 1) * 4 + channel],
                ];
                samples.sort_unstable();
                output.pixels[pixel * 4 + channel] = samples[1];
            }
        }
    }
}

/// Deterministic integer realization of the public square-root transfer.
fn gamma_correct(channel: u8) -> u8 {
    (u32::from(channel) * 255).isqrt() as u8
}

fn apply_gamma(output: &mut Framebuffer) {
    for pixel in output.pixels.chunks_exact_mut(4) {
        for channel in &mut pixel[..3] {
            *channel = gamma_correct(*channel);
        }
    }
}

/// Stochastically round one eight-bit channel to seven bits, then expand that
/// value back into the reference framebuffer's eight-bit storage. The random
/// bit is a separate input so the documented quantization step can be tested
/// independently of fn64's noise and output-representation policies.
/// Public documentation specifies fresh random low-bit noise before final
/// seven-bit quantization, but not its generator or seed. This coordinate and
/// channel hash is an explicit deterministic emulation policy keyed by the
/// retrace's exact guest cycle.
fn apply_gamma_dither(output: &mut Framebuffer, seed: u64) {
    for (pixel_index, pixel) in output.pixels.chunks_exact_mut(4).enumerate() {
        for (channel_index, channel) in pixel[..3].iter_mut().enumerate() {
            *channel = gamma_dither_quantize_bounded_v1(
                *channel,
                reference_noise_bit_v1(seed, pixel_index as u64, channel_index as u8),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::Coverage;
    use fn64_render::{
        ViAaMode, ViFilterControl, ViScanoutField, ViScanoutRegisters, ViScanoutState,
    };

    fn grayscale(values: &[u8], width: u32) -> Framebuffer {
        assert_eq!(values.len() % width as usize, 0);
        let mut framebuffer = Framebuffer::new(width, (values.len() / width as usize) as u32);
        for (pixel, value) in framebuffer
            .pixels
            .chunks_exact_mut(4)
            .zip(values.iter().copied())
        {
            pixel.copy_from_slice(&[value, value, value, 255]);
        }
        framebuffer
    }

    #[test]
    fn rgba16_restoration_exact_neighbor_and_border_vectors() {
        let mut greater = grayscale(&[88, 88, 88, 88, 80, 88, 88, 88, 88], 3);
        let source = greater.clone();
        filter_scanout(&source, &mut greater, false, false, true);
        assert_eq!(&greater.pixels[16..20], &[88, 88, 88, 255]);

        let mut lesser = grayscale(&[72, 72, 72, 72, 80, 72, 72, 72, 72], 3);
        let source = lesser.clone();
        filter_scanout(&source, &mut lesser, false, false, true);
        assert_eq!(&lesser.pixels[16..20], &[72, 72, 72, 255]);

        let mut corner = grayscale(&[80, 88, 0, 88, 88, 0, 0, 0, 0], 3);
        let source = corner.clone();
        filter_scanout(&source, &mut corner, false, false, true);
        assert_eq!(&corner.pixels[0..4], &[83, 83, 83, 255]);
    }

    #[test]
    fn rgba16_coverage_aa_exact_checkerboard_neighborhood_vector() {
        let mut output = grayscale(
            &[
                0, 16, 0, 198, 0, // upper diagonals
                33, 0, 224, 0, 181, // horizontal distance-two neighbors
                0, 49, 0, 165, 0, // lower diagonals
            ],
            5,
        );
        output.coverage[7] = Coverage::new(4);
        let source = output.clone();
        filter_scanout(&source, &mut output, false, true, false);
        assert_eq!(&output.pixels[7 * 4..7 * 4 + 4], &[132, 132, 132, 255]);
    }

    #[test]
    fn rgba16_coverage_aa_rejects_partial_neighbors() {
        let mut output = grayscale(
            &[
                0, 16, 0, 198, 0, // upper diagonals
                33, 0, 224, 0, 181, // horizontal distance-two neighbors
                0, 49, 0, 165, 0, // lower diagonals
            ],
            5,
        );
        output.coverage[7] = Coverage::new(4);
        output.coverage[1] = Coverage::new(7);
        output.coverage[3] = Coverage::new(7);
        let source = output.clone();
        filter_scanout(&source, &mut output, false, true, false);
        assert_eq!(&output.pixels[7 * 4..7 * 4 + 4], &[140, 140, 140, 255]);
    }

    #[test]
    fn rgba16_coverage_aa_equations_are_exhaustive_over_stored_colors() {
        for foreground_five in 0..32 {
            let foreground = expand_five_bit(foreground_five);
            for penultimate_minimum_five in 0..32 {
                let penultimate_minimum = expand_five_bit(penultimate_minimum_five);
                for penultimate_maximum_five in penultimate_minimum_five..32 {
                    let penultimate_maximum = expand_five_bit(penultimate_maximum_five);
                    let colors = [
                        [0, 0, 0],
                        [penultimate_minimum; 3],
                        [penultimate_maximum; 3],
                        [255, 255, 255],
                        [0, 0, 0],
                        [0, 0, 0],
                    ];
                    let expected = (i16::from(foreground.min(penultimate_minimum))
                        + i16::from(foreground.max(penultimate_maximum))
                        - i16::from(foreground))
                    .clamp(0, 255) as u8;
                    assert_eq!(
                        estimate_coverage_background(foreground, &colors, 4, 0),
                        expected
                    );
                }
            }
        }

        for coverage in 1..8 {
            let coverage = PartialCoverage::new(coverage);
            for foreground in 0..=u8::MAX {
                for background in 0..=u8::MAX {
                    let expected = ((u16::from(coverage.0) * u16::from(foreground)
                        + (8 - u16::from(coverage.0)) * u16::from(background)
                        + 4)
                        / 8) as u8;
                    assert_eq!(coverage.blend(foreground, background), expected);
                }
            }
        }
    }

    #[test]
    fn rgba16_partial_coverage_names_insufficient_neighborhood_policy() {
        let mut output = grayscale(&[80], 1);
        output.coverage[0] = Coverage::new(4);
        let source = output.clone();
        assert_eq!(
            CoverageAaNeighborhood::gather(&source, &output.pixels, 0, 0, false),
            CoverageAaNeighborhood::InsufficientFullCoverage { available: 0 }
        );
        filter_scanout(&source, &mut output, false, true, false);
        assert_eq!(output.pixels, [82, 82, 82, 255]);
    }

    #[test]
    fn interlaced_coverage_aa_uses_two_line_checkerboard_rows() {
        let mut source = grayscale(&[0; 25], 5);
        let center = 2 * 5 + 2;
        source.coverage[center] = Coverage::new(4);
        for (pixel, value) in [
            (1, 80),
            (3, 88),
            (6, 8),
            (8, 16),
            (10, 40),
            (14, 48),
            (16, 24),
            (18, 32),
            (21, 96),
            (23, 104),
        ] {
            source.pixels[pixel * 4..pixel * 4 + 3].fill(value);
        }

        let progressive = CoverageAaNeighborhood::gather(&source, &source.pixels, 2, 2, false);
        let interlaced = CoverageAaNeighborhood::gather(&source, &source.pixels, 2, 2, true);
        assert_eq!(
            progressive,
            CoverageAaNeighborhood::Preferred {
                colors: [[8; 3], [16; 3], [41; 3], [49; 3], [24; 3], [33; 3]],
                len: 6,
            }
        );
        assert_eq!(
            interlaced,
            CoverageAaNeighborhood::Preferred {
                colors: [[82; 3], [90; 3], [41; 3], [49; 3], [99; 3], [107; 3]],
                len: 6,
            }
        );
    }

    fn coverage_fixture() -> Framebuffer {
        let mut source = grayscale(
            &[
                0, 16, 0, 198, 0, // upper diagonals
                33, 0, 224, 0, 181, // horizontal distance-two neighbors
                0, 49, 0, 165, 0, // lower diagonals
            ],
            5,
        );
        source.coverage[7] = Coverage::new(4);
        source
    }

    fn scanout_with_aa_mode(
        source: &Framebuffer,
        antialias_mode: ViAaMode,
        dither_filter: bool,
        resample: Option<ViResampleControl>,
    ) -> Framebuffer {
        scanout(
            source,
            ViPresentation {
                scanout: test_scanout_state(
                    ViFilterControl {
                        pixel_type: ViPixelType::Rgba16,
                        antialias_mode,
                        dither_filter,
                        ..Default::default()
                    },
                    resample,
                    source.width,
                    source.height,
                ),
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn vi_status_antialias_modes_select_partial_coverage_filter_exactly() {
        let source = coverage_fixture();
        for mode in [ViAaMode::AaResampleAlways, ViAaMode::AaResampleWhenNeeded] {
            let output = scanout_with_aa_mode(&source, mode, false, None);
            assert_eq!(&output.pixels[7 * 4..7 * 4 + 4], &[132, 132, 132, 255]);
        }

        for mode in [ViAaMode::ResampleOnly, ViAaMode::Replicate] {
            let output = scanout_with_aa_mode(&source, mode, true, None);
            assert_eq!(&output.pixels[7 * 4..7 * 4 + 4], &[224, 224, 224, 255]);
        }
    }

    #[test]
    fn dither_restoration_is_independent_and_full_coverage_only() {
        let source = grayscale(&[88, 88, 88, 88, 80, 88, 88, 88, 88], 3);
        for mode in [
            ViAaMode::AaResampleAlways,
            ViAaMode::AaResampleWhenNeeded,
            ViAaMode::ResampleOnly,
            ViAaMode::Replicate,
        ] {
            let restored = scanout_with_aa_mode(&source, mode, true, None);
            assert_eq!(&restored.pixels[16..20], &[88, 88, 88, 255]);

            let unchanged = scanout_with_aa_mode(&source, mode, false, None);
            assert_eq!(&unchanged.pixels[16..20], &[80, 80, 80, 255]);
        }
    }

    fn resample_control(
        x_step: u16,
        x_offset: u16,
        y_step: u16,
        y_offset: u16,
    ) -> ViResampleControl {
        ViResampleControl::from_registers(
            u32::from(x_step) | (u32::from(x_offset) << 16),
            u32::from(y_step) | (u32::from(y_offset) << 16),
            0,
            0,
        )
    }

    fn test_scanout_state(
        filters: ViFilterControl,
        resample: Option<ViResampleControl>,
        width: u32,
        height: u32,
    ) -> ViScanoutState {
        test_scanout_state_with_window(filters, resample, width, width, height * 2)
    }

    fn test_scanout_state_with_window(
        filters: ViFilterControl,
        resample: Option<ViResampleControl>,
        source_width: u32,
        h_start: u32,
        v_start: u32,
    ) -> ViScanoutState {
        let Some(resample) = resample else {
            return ViScanoutState::BackendOnly(filters);
        };
        let pixel_type = match filters.pixel_type {
            ViPixelType::Blank => 0,
            ViPixelType::Reserved => 1,
            ViPixelType::Unspecified | ViPixelType::Rgba16 => 2,
            ViPixelType::Rgba32 => 3,
        };
        let mut status = pixel_type
            | filters.antialias_mode.status_bits().unwrap_or(0)
            | (u32::from(filters.gamma_dither) << 2)
            | (u32::from(filters.gamma) << 3)
            | (u32::from(filters.divot) << 4)
            | (u32::from(filters.dither_filter) << 16);
        let field = match resample.field {
            ViScanoutField::Progressive => 0,
            ViScanoutField::InterlacedEven => {
                status |= 1 << 6;
                0
            }
            ViScanoutField::InterlacedOdd => {
                status |= 1 << 6;
                1
            }
        };
        let mut words = [0u32; ViScanoutRegisters::WORD_COUNT];
        words[0] = status;
        words[2] = source_width;
        words[4] = field;
        words[9] = h_start;
        words[10] = v_start;
        words[12] =
            u32::from(resample.x.step_u2_10()) | (u32::from(resample.x.offset_u2_10()) << 16);
        words[13] =
            u32::from(resample.y.step_u2_10()) | (u32::from(resample.y.offset_u2_10()) << 16);
        ViScanoutState::Registers(ViScanoutRegisters::from_words(words))
    }

    #[test]
    fn active_window_drives_post_vi_extent_and_source_crop() {
        let source = grayscale(&[0, 40, 80, 120, 10, 50, 90, 130], 4);
        let filters = ViFilterControl {
            pixel_type: ViPixelType::Rgba32,
            antialias_mode: ViAaMode::ResampleOnly,
            ..Default::default()
        };
        let output = scanout(
            &source,
            ViPresentation {
                scanout: test_scanout_state_with_window(
                    filters,
                    Some(resample_control(
                        ViScaleAxis::ONE,
                        ViScaleAxis::ONE,
                        ViScaleAxis::ONE,
                        0,
                    )),
                    source.width,
                    (100 << 16) | 102,
                    (20 << 16) | 24,
                ),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((output.width, output.height), (2, 2));
        let red = output
            .pixels
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>();
        assert_eq!(red, [40, 80, 50, 90]);
    }

    #[test]
    fn normal_640_dot_window_consumes_half_rate_source_coordinates() {
        let values = (0..320).map(|x| (x % 251) as u8).collect::<Vec<_>>();
        let source = grayscale(&values, 320);
        let output = scanout(
            &source,
            ViPresentation {
                scanout: test_scanout_state_with_window(
                    ViFilterControl {
                        pixel_type: ViPixelType::Rgba32,
                        antialias_mode: ViAaMode::ResampleOnly,
                        ..Default::default()
                    },
                    Some(resample_control(0x0200, 0, ViScaleAxis::ONE, 0)),
                    source.width,
                    0x006c_02ec,
                    (37 << 16) | 39,
                ),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((output.width, output.height), (640, 1));
        let red = output
            .pixels
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>();
        assert_eq!((red[0], red[1], red[2], red[200]), (0, 1, 1, 100));
        assert_eq!(red[639], values[319]);
    }

    #[test]
    fn active_window_extent_survives_blank_fade_and_repeat_line() {
        let source = grayscale(&[0, 40, 80, 120, 10, 50, 90, 130], 4);
        let scanout_state = test_scanout_state_with_window(
            ViFilterControl {
                pixel_type: ViPixelType::Rgba32,
                antialias_mode: ViAaMode::ResampleOnly,
                ..Default::default()
            },
            Some(resample_control(
                ViScaleAxis::ONE,
                ViScaleAxis::ONE,
                ViScaleAxis::ONE,
                0,
            )),
            source.width,
            (100 << 16) | 102,
            (20 << 16) | 22,
        );
        let blank = scanout(
            &source,
            ViPresentation {
                blanked: true,
                scanout: scanout_state,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((blank.width, blank.height), (2, 1));
        assert!(blank
            .pixels
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 0, 255]));

        let fade = scanout(
            &source,
            ViPresentation {
                fade: Some(0x03ff),
                scanout: scanout_state,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((fade.width, fade.height), (2, 1));
        assert_eq!(
            fade.pixels
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            [50, 90]
        );

        let repeated = scanout(
            &source,
            ViPresentation {
                repeat_line: true,
                scanout: scanout_state,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((repeated.width, repeated.height), (2, 1));
        assert_eq!(
            repeated
                .pixels
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            [40, 80]
        );
    }

    #[test]
    fn explicit_blanking_does_not_mask_reserved_vi_pixel_type() {
        let source = grayscale(&[0], 1);
        let error = match scanout(
            &source,
            ViPresentation {
                blanked: true,
                scanout: ViScanoutState::BackendOnly(ViFilterControl {
                    pixel_type: ViPixelType::Reserved,
                    ..ViFilterControl::default()
                }),
                ..ViPresentation::default()
            },
        ) {
            Ok(_) => panic!("reserved VI pixel type was hidden by explicit blanking"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "reference backend error: VI STATUS selects reserved pixel type 1"
        );
    }

    #[test]
    fn inactive_live_window_does_not_fall_back_to_host_geometry() {
        let source = grayscale(&[0, 40, 80, 120], 2);
        let mut words = [0u32; ViScanoutRegisters::WORD_COUNT];
        words[0] = 3;
        words[1] = 0x0010_0000;
        words[2] = 2;
        let output = scanout(
            &source,
            ViPresentation {
                scanout: ViScanoutState::Registers(ViScanoutRegisters::from_words(words)),
                ..ViPresentation::default()
            },
        )
        .unwrap();
        assert_eq!((output.width, output.height), (0, 0));
        assert!(output.pixels.is_empty());
    }

    #[test]
    fn resampling_identity_and_xy_half_step_offset_vectors() {
        let identity = grayscale(&[0, 40, 80, 80, 120, 160, 160, 200, 240], 3);
        let expected = identity.pixels.clone();
        let identity = apply_resampling(
            &identity,
            resample_control(ViScaleAxis::ONE, 0, ViScaleAxis::ONE, 0),
            3,
            3,
        );
        assert_eq!(identity.pixels, expected);

        let half_source = grayscale(&[0, 40, 80, 80, 120, 160, 160, 200, 240], 3);
        let half = apply_resampling(&half_source, resample_control(512, 512, 512, 512), 3, 3);
        let red = half
            .pixels
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>();
        assert_eq!(red, [60, 80, 100, 100, 120, 140, 140, 160, 180]);
    }

    #[test]
    fn vi_status_antialias_modes_select_resampling_exactly() {
        let source = grayscale(&[0, 40, 80], 3);
        let control = resample_control(ViScaleAxis::ONE, 512, ViScaleAxis::ONE, 0);
        for mode in [
            ViAaMode::AaResampleAlways,
            ViAaMode::AaResampleWhenNeeded,
            ViAaMode::ResampleOnly,
        ] {
            let output = scanout_with_aa_mode(&source, mode, false, Some(control));
            let red = output
                .pixels
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>();
            assert_eq!(red, [20, 60, 80]);
        }

        let replicated = scanout_with_aa_mode(&source, ViAaMode::Replicate, false, Some(control));
        let red = replicated
            .pixels
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>();
        assert_eq!(red, [0, 40, 80]);
    }

    #[test]
    fn resampling_preserves_host_alpha_for_identity_and_fractional_positions() {
        let mut identity_source = grayscale(&[0, 40], 2);
        identity_source.pixels[3] = 17;
        identity_source.pixels[7] = 201;
        let expected = identity_source.pixels.clone();
        let identity = apply_resampling(
            &identity_source,
            resample_control(ViScaleAxis::ONE, 0, ViScaleAxis::ONE, 0),
            2,
            1,
        );
        assert_eq!(identity.pixels, expected);

        let mut horizontal_source = grayscale(&[0, 255], 2);
        horizontal_source.pixels[3] = 0;
        horizontal_source.pixels[7] = 200;
        let horizontal = apply_resampling(
            &horizontal_source,
            resample_control(ViScaleAxis::ONE, 512, ViScaleAxis::ONE, 0),
            2,
            1,
        );
        let alpha = horizontal
            .pixels
            .chunks_exact(4)
            .map(|pixel| pixel[3])
            .collect::<Vec<_>>();
        assert_eq!(alpha, [100, 200]);

        let mut vertical_source = grayscale(&[0, 255], 1);
        vertical_source.pixels[3] = 20;
        vertical_source.pixels[7] = 220;
        let vertical = apply_resampling(
            &vertical_source,
            resample_control(ViScaleAxis::ONE, 0, ViScaleAxis::ONE, 512),
            1,
            2,
        );
        let alpha = vertical
            .pixels
            .chunks_exact(4)
            .map(|pixel| pixel[3])
            .collect::<Vec<_>>();
        assert_eq!(alpha, [120, 220]);
    }

    #[test]
    fn resampling_typed_axis_position_exhausts_register_and_border_boundaries() {
        for code in 0..=0x0fffu32 {
            let step_axis = ViScaleAxis::from_register(code);
            for index in [0usize, 1, 17, 1023] {
                let position = AxisPositionU10Fraction::from_output(index, step_axis);
                let expected = u64::try_from(index).unwrap() * u64::from(code);
                assert_eq!(position.0, expected);
                assert_eq!(position.integer(), expected >> 10);
                assert_eq!(position.fraction_u0_10(), (expected & 0x03ff) as u16);
            }

            let offset_axis = ViScaleAxis::from_register(code << 16);
            let position = AxisPositionU10Fraction::from_output(usize::MAX, offset_axis);
            assert_eq!(position.0, u64::from(code));
        }

        for offset in 0..=0x0fffu32 {
            let axis = ViScaleAxis::from_register(offset << 16);
            let sample = AxisSample::from_output(0, axis, 4);
            let integer = u64::from(offset >> 10);
            if integer < 3 {
                assert_eq!(sample.lower, integer as usize);
                assert_eq!(sample.upper, integer as usize + 1);
                assert_eq!(sample.fraction_u0_10(), (offset & 0x03ff) as u16);
                assert_eq!(sample.boundary, AxisBoundary::Interpolating);
            } else {
                assert_eq!(sample.lower, 3);
                assert_eq!(sample.upper, 3);
                assert_eq!(sample.fraction_u0_10(), 0);
                assert_eq!(
                    sample.boundary,
                    AxisBoundary::HeldLast {
                        requested_integer: integer
                    }
                );
            }
        }

        let beyond = AxisSample::from_output(2, ViScaleAxis::from_register(0x0fff), 4);
        assert_eq!(
            beyond,
            AxisSample {
                lower: 3,
                upper: 3,
                fraction_u0_10: 0,
                boundary: AxisBoundary::HeldLast {
                    requested_integer: 7,
                },
            }
        );
    }

    #[test]
    fn resampling_clamps_register_derived_fetches_to_source_extent() {
        let source = grayscale(&[0, 40, 80], 3);
        let output = apply_resampling(
            &source,
            resample_control(ViScaleAxis::ONE, 512, ViScaleAxis::ONE, 0),
            3,
            1,
        );
        let red = output
            .pixels
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>();
        assert_eq!(red, [20, 60, 80]);
    }

    #[test]
    fn resampling_integer_policy_exhausts_public_fractions_and_color_endpoints() {
        for fraction in 0..ViScaleAxis::ONE {
            for lower in 0..=u8::MAX {
                for upper in [0, lower, u8::MAX] {
                    let expected = ((u32::from(lower)
                        * (u32::from(ViScaleAxis::ONE) - u32::from(fraction))
                        + u32::from(upper) * u32::from(fraction)
                        + u32::from(ViScaleAxis::ONE / 2))
                        / u32::from(ViScaleAxis::ONE)) as u8;
                    assert_eq!(interpolate_u2_10(lower, upper, fraction), expected);
                }
            }
        }

        for lower in 0..=u8::MAX {
            for upper in 0..=u8::MAX {
                for fraction in [0, 1, 511, 512, 1023] {
                    let value = interpolate_u2_10(lower, upper, fraction);
                    assert!((lower.min(upper)..=lower.max(upper)).contains(&value));
                }
            }
        }
    }

    #[test]
    fn scanout_resamples_before_gamma_in_patent_order() {
        let source = grayscale(&[0, 255], 2);
        let output = scanout(
            &source,
            ViPresentation {
                scanout: test_scanout_state(
                    ViFilterControl {
                        pixel_type: ViPixelType::Rgba32,
                        gamma: true,
                        ..ViFilterControl::default()
                    },
                    Some(resample_control(
                        ViScaleAxis::ONE,
                        ViScaleAxis::ONE / 2,
                        ViScaleAxis::ONE,
                        0,
                    )),
                    source.width,
                    source.height,
                ),
                ..ViPresentation::default()
            },
        )
        .unwrap();
        assert_eq!(&output.pixels[0..4], &[180, 180, 180, 255]);
    }

    #[test]
    fn divot_exact_horizontal_median_vectors() {
        for partial in 0..3 {
            let mut output = grayscale(&[10, 200, 20], 3);
            output.coverage[partial] = Coverage::new(4);
            let source = output.clone();
            apply_divot(&source, &mut output);
            assert_eq!(&output.pixels[4..8], &[20, 20, 20, 255]);
        }

        let mut full = grayscale(&[10, 200, 20], 3);
        let source = full.clone();
        apply_divot(&source, &mut full);
        assert_eq!(&full.pixels[4..8], &[200, 200, 200, 255]);
    }

    #[test]
    fn gamma_square_root_exact_integer_vectors() {
        let inputs = [0, 1, 2, 3, 4, 16, 64, 128, 254, 255];
        let expected = [0, 15, 22, 27, 31, 63, 127, 180, 254, 255];
        assert_eq!(inputs.map(gamma_correct), expected);
    }

    #[test]
    fn gamma_dither_quantizer_and_host_expansion_vectors() {
        let zero = fn64_render::vi_public_filters::ViRandomBit::new(0).unwrap();
        let one = fn64_render::vi_public_filters::ViRandomBit::new(1).unwrap();
        assert_eq!(gamma_dither_quantize_bounded_v1(0, zero), 0);
        assert_eq!(gamma_dither_quantize_bounded_v1(0, one), 0);
        assert_eq!(gamma_dither_quantize_bounded_v1(100, zero), 100);
        assert_eq!(gamma_dither_quantize_bounded_v1(100, one), 100);
        assert_eq!(gamma_dither_quantize_bounded_v1(101, zero), 100);
        assert_eq!(gamma_dither_quantize_bounded_v1(101, one), 102);
        assert_eq!(gamma_dither_quantize_bounded_v1(128, zero), 129);
        assert_eq!(gamma_dither_quantize_bounded_v1(255, one), 255);
    }

    #[test]
    fn gamma_dither_host_policy_has_an_exact_seeded_vector() {
        let mut output = grayscale(
            &[0, 1, 2, 63, 64, 65, 100, 101, 127, 128, 129, 254, 255],
            13,
        );
        apply_gamma_dither(&mut output, 0x0123_4567_89ab_cdef);
        let red = output
            .pixels
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>();
        assert_eq!(
            red,
            [0, 0, 2, 64, 64, 64, 100, 100, 126, 129, 131, 255, 255]
        );
    }

    #[test]
    fn scanout_composes_restoration_divot_gamma_then_gamma_dither() {
        let mut source = grayscale(&[88, 80, 88], 3);
        source.coverage[1] = Coverage::new(4);
        let output = scanout(
            &source,
            ViPresentation {
                scanout: ViScanoutState::BackendOnly(ViFilterControl {
                    pixel_type: ViPixelType::Rgba16,
                    antialias_mode: ViAaMode::AaResampleAlways,
                    gamma: true,
                    gamma_dither: true,
                    divot: true,
                    dither_filter: true,
                }),
                noise_seed: 0x0123_4567_89ab_cdef,
                ..ViPresentation::default()
            },
        )
        .unwrap();
        assert_eq!(&output.pixels[4..8], &[149, 149, 149, 255]);
    }
}
