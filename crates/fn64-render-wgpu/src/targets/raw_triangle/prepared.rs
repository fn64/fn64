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

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PreparedRawTriangleCheckpointPatch {
    pub(crate) access: fn64_render_ir::ResourceAccess,
    pub(crate) bytes: Vec<u8>,
    pub(crate) coverage: Vec<u8>,
}

pub(crate) struct PreparedRawTriangleRowBinOutput {
    pub(crate) checkpoints: Vec<Vec<PreparedRawTriangleCheckpointPatch>>,
    pub(crate) bytes: Vec<u8>,
    pub(crate) coverage: ColorCoverageState,
    pub(crate) band_jobs: usize,
}

pub(crate) struct PreparedRawTriangleRowBinAttempt {
    pub(crate) checkpoints: Vec<Vec<PreparedRawTriangleCheckpointPatch>>,
    pub(crate) bytes: Vec<u8>,
    pub(crate) coverage: ColorCoverageState,
    pub(crate) band_jobs: usize,
    pub(crate) error: Option<(usize, TexrectExecutionError)>,
}

fn row_bin_error_key(error: &(usize, TexrectExecutionError)) -> (usize, u32, u32) {
    match &error.1 {
        TexrectExecutionError::Sample { column, row, .. } => (error.0, *row, *column),
        _ => (error.0, u32::MAX, u32::MAX),
    }
}

/// Executes one exact, already-prepared raw-triangle stream through disjoint
/// target row bands. Draw and checkpoint ownership are binned once before
/// workers start; workers retain stream order within their band and no worker
/// can observe another band's mutations.
///
/// The only cross-worker reduction is the first scalar-visible raster error.
/// Its `(draw, row, column)` key is the serial command/pixel order, so worker
/// completion order cannot change which refusal is returned. All output is
/// owned by this call and is exposed only after every band succeeds.
pub(crate) fn execute_prepared_raw_triangle_row_bins<S: TmemByteSource + Sync + ?Sized>(
    key: ColorTargetKey,
    prepared: &[PreparedRawTriangleRaster<'_, S>],
    checkpoint_draw_limits: &[usize],
    checkpoint_accesses: &[Vec<fn64_render_ir::ResourceAccess>],
    bytes: Vec<u8>,
    coverage: ColorCoverageState,
    workers: usize,
) -> Result<PreparedRawTriangleRowBinOutput, (usize, TexrectExecutionError)> {
    let attempt = execute_prepared_raw_triangle_row_bin_prefix(
        key,
        prepared,
        checkpoint_draw_limits,
        checkpoint_accesses,
        bytes,
        coverage,
        workers,
    );
    if let Some(error) = attempt.error {
        return Err(error);
    }
    assert_eq!(attempt.checkpoints.len(), checkpoint_draw_limits.len());
    Ok(PreparedRawTriangleRowBinOutput {
        checkpoints: attempt.checkpoints,
        bytes: attempt.bytes,
        coverage: attempt.coverage,
        band_jobs: attempt.band_jobs,
    })
}

/// Executes the prepared prefix while retaining all checkpoints that precede
/// the first scalar-visible raster error. This lets an enclosing ordered
/// transaction validate an earlier member before selecting a later member's
/// raster refusal, without publishing any partial mutation.
pub(crate) fn execute_prepared_raw_triangle_row_bin_prefix<S: TmemByteSource + Sync + ?Sized>(
    key: ColorTargetKey,
    prepared: &[PreparedRawTriangleRaster<'_, S>],
    checkpoint_draw_limits: &[usize],
    checkpoint_accesses: &[Vec<fn64_render_ir::ResourceAccess>],
    mut bytes: Vec<u8>,
    mut coverage: ColorCoverageState,
    workers: usize,
) -> PreparedRawTriangleRowBinAttempt {
    #[derive(Clone, Copy)]
    struct OwnedAccess {
        member: usize,
        access: fn64_render_ir::ResourceAccess,
    }
    struct Seed<'a> {
        rows: Range<u32>,
        bytes: &'a mut [u8],
        coverage: &'a mut [u8],
        draws: Vec<usize>,
        accesses: Vec<OwnedAccess>,
    }

    assert!(matches!(workers, 2 | 4 | 6 | 8));
    assert_eq!(checkpoint_draw_limits.len(), checkpoint_accesses.len());
    assert_eq!(checkpoint_draw_limits.last().copied(), Some(prepared.len()));
    let extent = key.extent();
    let width = extent.width();
    let height = extent.height();
    let bytes_per_pixel = key.format().bytes_per_pixel() as usize;
    let row_bytes = width as usize * bytes_per_pixel;
    assert_eq!(bytes.len(), row_bytes * height as usize);
    assert_eq!(coverage.cells_mut().len(), extent.pixels() as usize);

    let bands = (0..workers)
        .map(|band| {
            height * band as u32 / workers as u32..height * (band as u32 + 1) / workers as u32
        })
        .collect::<Vec<_>>();
    let mut owner = vec![usize::MAX; height as usize];
    for (band, rows) in bands.iter().enumerate() {
        for row in rows.clone() {
            assert_eq!(
                core::mem::replace(&mut owner[row as usize], band),
                usize::MAX
            );
        }
    }

    let mut draw_bins = (0..workers).map(|_| Vec::new()).collect::<Vec<_>>();
    for (draw, raster) in prepared.iter().enumerate() {
        let mut last = None;
        for row in raster.covered_rows() {
            let band = owner[row.y as usize];
            if last != Some(band) {
                draw_bins[band].push(draw);
                last = Some(band);
            }
        }
    }

    let base = key.address().get();
    let mut access_bins = (0..workers).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut checkpoint_owners = Vec::with_capacity(checkpoint_accesses.len());
    for (member, accesses) in checkpoint_accesses.iter().enumerate() {
        let mut owners = Vec::with_capacity(accesses.len());
        for access in accesses.iter().copied() {
            let fn64_render_ir::ResourceRegion::Rdram { range, .. } = access.region() else {
                panic!("prepared row-bin checkpoint must name RDRAM")
            };
            let start = range.start().get() - base;
            let end = range.end() - base;
            let first_row = start / row_bytes as u32;
            assert!(end > start);
            assert_eq!(first_row, (end - 1) / row_bytes as u32);
            let band = owner[first_row as usize];
            owners.push(band);
            access_bins[band].push(OwnedAccess { member, access });
        }
        checkpoint_owners.push(owners);
    }

    let mut byte_tail = bytes.as_mut_slice();
    let mut coverage_tail = coverage.cells_mut();
    let mut seeds = Vec::with_capacity(workers);
    for (band, rows) in bands.into_iter().enumerate() {
        let byte_len = (rows.end - rows.start) as usize * row_bytes;
        let coverage_len = (rows.end - rows.start) as usize * width as usize;
        let all_bytes = core::mem::take(&mut byte_tail);
        let (band_bytes, rest) = all_bytes.split_at_mut(byte_len);
        byte_tail = rest;
        let all_coverage = core::mem::take(&mut coverage_tail);
        let (band_coverage, rest) = all_coverage.split_at_mut(coverage_len);
        coverage_tail = rest;
        seeds.push(Seed {
            rows,
            bytes: band_bytes,
            coverage: band_coverage,
            draws: core::mem::take(&mut draw_bins[band]),
            accesses: core::mem::take(&mut access_bins[band]),
        });
    }

    let band_jobs = seeds.iter().filter(|seed| !seed.draws.is_empty()).count();
    struct BandAttempt {
        patches: Vec<PreparedRawTriangleCheckpointPatch>,
        error: Option<(usize, TexrectExecutionError)>,
    }

    let results = seeds
        .into_par_iter()
        .map(|seed| {
            let band_base = base + seed.rows.start * width * key.format().bytes_per_pixel();
            let mut view =
                ExactColorRowBandMut::from_exact_parts(key, seed.rows, seed.bytes, seed.coverage);
            let mut patches = Vec::with_capacity(seed.accesses.len());
            let mut draw_cursor = 0;
            let mut access_cursor = 0;
            let mut first_error = None;
            'members: for (member, &limit) in checkpoint_draw_limits.iter().enumerate() {
                while draw_cursor < seed.draws.len() && seed.draws[draw_cursor] < limit {
                    let draw = seed.draws[draw_cursor];
                    if let Err(error) = prepared[draw].raster_band(&mut view, None, false) {
                        first_error = Some((draw, error));
                        break 'members;
                    }
                    draw_cursor += 1;
                }
                while access_cursor < seed.accesses.len()
                    && seed.accesses[access_cursor].member == member
                {
                    let access = seed.accesses[access_cursor].access;
                    let fn64_render_ir::ResourceRegion::Rdram { range, .. } = access.region()
                    else {
                        unreachable!()
                    };
                    let start = (range.start().get() - band_base) as usize;
                    let end = start + range.len() as usize;
                    let (visible, hidden, _) = view.parts_mut();
                    patches.push(PreparedRawTriangleCheckpointPatch {
                        access,
                        bytes: visible[start..end].to_vec(),
                        coverage: hidden[start / bytes_per_pixel..end / bytes_per_pixel].to_vec(),
                    });
                    access_cursor += 1;
                }
            }
            BandAttempt {
                patches,
                error: first_error,
            }
        })
        .collect::<Vec<_>>();

    let mut first_error = None;
    let mut completed = Vec::with_capacity(workers);
    for result in results {
        if let Some(error) = result.error {
            match &first_error {
                Some(first) if row_bin_error_key(&error) >= row_bin_error_key(first) => {}
                _ => first_error = Some(error),
            }
        }
        completed.push(result.patches.into_iter());
    }
    let completed_members = first_error
        .as_ref()
        .map_or(checkpoint_draw_limits.len(), |error| {
            checkpoint_draw_limits.partition_point(|limit| *limit <= error.0)
        });
    let checkpoints = checkpoint_owners
        .into_iter()
        .take(completed_members)
        .map(|owners| {
            owners
                .into_iter()
                .map(|band| {
                    completed[band]
                        .next()
                        .expect("owned checkpoint patch exists before first raster error")
                })
                .collect()
        })
        .collect();
    if first_error.is_none() {
        assert!(completed.iter_mut().all(|patches| patches.next().is_none()));
    }
    PreparedRawTriangleRowBinAttempt {
        checkpoints,
        bytes,
        coverage,
        band_jobs,
        error: first_error,
    }
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
