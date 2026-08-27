use super::*;
use std::ops::Range;

use crate::targets::{ColorCoverageState, ColorTargetExtent, ColorTargetKey, ExactColorRowBandMut};
use crate::tmem::PreparedPointSampler;

/// Immutable admitted state for one raw-triangle mutation. Pixel arithmetic
/// remains owned by `raster_triangle_scalar`; this type only binds validated
/// command state to caller-owned visible and hidden-coverage row bands.
pub(crate) struct PreparedRawTriangleRaster<'a, S: TmemByteSource + ?Sized> {
    key: ColorTargetKey,
    format: ColorTargetFormat,
    extent: ColorTargetExtent,
    triangle: RawTriangle,
    rows: Box<[triangle_span::CoveredRow]>,
    shading: TexrectShading,
    base_inputs: crate::CombinerInputs,
    shade: Option<[triangle_span::AttributePlane; 4]>,
    texture_planes: Option<[triangle_span::AttributePlane; 3]>,
    texture: Option<(TexrectTileBinding, &'a S, TextureLutMode)>,
    perspective: bool,
    blend_state: crate::BlendModeState,
    stages: TexrectFragmentStages,
    evaluation: TexrectCombinerEvaluation,
    fragment_program: RawTriangleFragmentProgram,
    incremental_texture_planes: bool,
    prepared_sampler: Option<PreparedPointSampler>,
}

impl<'a, S: TmemByteSource + ?Sized> PreparedRawTriangleRaster<'a, S> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_new_exact(
        candidate: &CandidateColorTarget,
        other_mode: OtherMode,
        triangle: &RawTriangle,
        shading: TexrectShading,
        blend_registers: TexrectBlendRegisters,
        declared: &[fn64_render_ir::ResourceAccess],
        texture: Option<RawTriangleTexture<'a, S>>,
        resident_len: usize,
    ) -> Result<Self, TexrectExecutionError> {
        let evaluation = admitted_cycle_evaluation(other_mode.cycle_type())?;
        if matches!(evaluation, TexrectCombinerEvaluation::BlitsTheTexel) {
            return Err(TexrectExecutionError::UnsupportedCycleType {
                cycle_type: CycleType::Copy,
            });
        }
        let cycles = evaluation
            .validated_cycles()
            .expect("Copy cycle was refused above, and Fill by admitted_cycle_evaluation");
        let textured = triangle.flags().textured();
        if textured != texture.is_some() {
            return Err(
                TexrectExecutionError::TriangleTextureBindingDisagreesWithOpcode {
                    opcode_textured: textured,
                    binding_present: texture.is_some(),
                },
            );
        }
        let base_inputs = shading
            .validate_combiner_program_for(cycles, triangle.flags().shaded(), textured)?
            .base_inputs();
        let blend_state = blend_registers.mode_state(other_mode);
        require_blendable_mode(blend_state)?;
        let stages = TexrectFragmentStages::try_new(other_mode, blend_registers.blend_color())?;

        let key = candidate.key();
        let format = key.format();
        let extent = key.extent();
        let full_len = (extent.pixels() as usize)
            .checked_mul(format.bytes_per_pixel() as usize)
            .ok_or(TargetError::PixelBufferLengthOverflow {
                pixels: extent.pixels() as usize,
                bytes_per_pixel: format.bytes_per_pixel(),
            })?;
        if resident_len != full_len {
            return Err(TargetError::CompletedByteLengthMismatch {
                key,
                generation: candidate.generation(),
                expected: full_len,
                actual: resident_len,
            }
            .into());
        }

        let rows = triangle_span::covered_rows(triangle, extent.width(), extent.height());
        if rows.len() != declared.len() {
            return Err(
                TexrectExecutionError::TriangleRowCountDisagreesWithJournal {
                    declared: declared.len(),
                    rasterized: rows.len(),
                },
            );
        }
        let base = key.address().get();
        let bpp = format.bytes_per_pixel();
        for (position, (row, access)) in rows.iter().zip(declared.iter()).enumerate() {
            let start = base + (row.y * extent.width() + row.x0) * bpp;
            let len = (row.x1 - row.x0) * bpp;
            let fn64_render_ir::ResourceRegion::Rdram { range, .. } = access.region() else {
                return Err(
                    TexrectExecutionError::TriangleRowRangeDisagreesWithJournal {
                        position,
                        declared: (0, 0),
                        rasterized: (start, len),
                    },
                );
            };
            let declared_range = (range.start().get(), range.len());
            if declared_range != (start, len) {
                return Err(
                    TexrectExecutionError::TriangleRowRangeDisagreesWithJournal {
                        position,
                        declared: declared_range,
                        rasterized: (start, len),
                    },
                );
            }
        }
        if rows.is_empty() {
            return Err(TexrectExecutionError::Target(TargetError::ZeroRectangle {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }));
        }
        let last = rows.last().expect("rows is non-empty");
        let left = rows.iter().map(|row| row.x0).min().expect("non-empty");
        let rectangle = TargetRectangle::try_new(
            left,
            rows[0].y,
            rows.iter().map(|row| row.x1).max().expect("non-empty") - left,
            last.y - rows[0].y + 1,
        )?;
        if rectangle.x() + rectangle.width() > extent.width()
            || rectangle.y() + rectangle.height() > extent.height()
        {
            return Err(TexrectExecutionError::OutsideTarget { key, rectangle });
        }

        let prepared_sampler = texture
            .as_ref()
            .map(|binding| {
                PreparedPointSampler::try_new(
                    binding.tile.descriptor(),
                    binding.tile.size(),
                    binding.lut_mode,
                )
            })
            .transpose()
            .map_err(|source| TexrectExecutionError::Sample {
                column: rows[0].x0,
                row: rows[0].y,
                source,
            })?;
        let fragment_program = select_fragment_program(
            format,
            triangle,
            shading,
            blend_state,
            evaluation,
            FragmentProgramSelection::AdmitExact,
        );
        Ok(Self {
            key,
            format,
            extent,
            triangle: *triangle,
            rows: rows.into_boxed_slice(),
            shading,
            base_inputs,
            shade: triangle.shade().map(triangle_span::shade_planes),
            texture_planes: triangle.texture().map(triangle_span::texture_planes),
            texture: texture.map(|binding| (binding.tile, binding.tmem, binding.lut_mode)),
            perspective: other_mode.texture_perspective(),
            blend_state,
            stages,
            evaluation,
            fragment_program,
            incremental_texture_planes: incremental_texture_planes_enabled(),
            prepared_sampler,
        })
    }

    pub(crate) fn raster_rows(
        &self,
        bytes: &mut [u8],
        coverage: &mut ColorCoverageState,
        row_range: Option<&Range<u32>>,
        depth: Option<RawTriangleDepth<'_>>,
        _allow_row_parallelism: bool,
    ) -> Result<(), TexrectExecutionError> {
        let rows = row_range.cloned().unwrap_or(0..self.extent.height());
        let mut band = ExactColorRowBandMut::from_full(self.key, rows, bytes, coverage);
        self.raster_band(&mut band, depth, false)
    }

    pub(crate) fn raster_band(
        &self,
        band: &mut ExactColorRowBandMut<'_>,
        depth: Option<RawTriangleDepth<'_>>,
        _allow_row_parallelism: bool,
    ) -> Result<(), TexrectExecutionError> {
        assert_eq!(
            band.key(),
            self.key,
            "prepared raw-triangle raster cannot mutate a different color target"
        );
        let start = self.rows.partition_point(|row| row.y < band.rows().start);
        let end = self.rows.partition_point(|row| row.y < band.rows().end);
        let texture = self
            .texture
            .map(|(tile, tmem, lut_mode)| RawTriangleTexture {
                tile,
                tmem,
                lut_mode,
            });
        let (bytes, coverage, base_y) = band.parts_mut();
        raster_triangle_scalar(
            bytes,
            self.format,
            self.extent.width(),
            &self.triangle,
            &self.rows[start..end],
            self.shading,
            self.base_inputs,
            self.shade,
            self.texture_planes,
            texture,
            self.perspective,
            self.blend_state,
            self.stages,
            self.evaluation,
            self.fragment_program,
            self.incremental_texture_planes,
            self.prepared_sampler,
            Some(coverage),
            depth,
            base_y,
        )
    }

    pub(crate) fn covered_rows(&self) -> &[triangle_span::CoveredRow] {
        &self.rows
    }
}
