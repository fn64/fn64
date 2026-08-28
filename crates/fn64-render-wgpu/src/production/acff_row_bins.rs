use super::*;

use crate::raw_dpc::RawTriangle;
use crate::targets::{
    execute_prepared_raw_triangle_row_bin_prefix, ColorCoverageState, OwnedTaskColorSegment,
    PreparedRawTriangleRaster, RawTriangleTexture, TexrectBlendRegisters, TexrectShading,
    TexrectTileBinding,
};
use crate::tmem::{DeferredPhysicalTmemWithPrefixes, TmemLoadStreamPosition};

/// Immutable command facts captured while one packet's authoritative plan is
/// borrowed. TMEM authority is deliberately absent: it is selected later from
/// the member's move-only sealed prefix arena at this exact stream position.
pub(super) struct DeferredAcffCommand {
    pub(super) position: TmemLoadStreamPosition,
    pub(super) triangle: RawTriangle,
    pub(super) other_mode: OtherMode,
    pub(super) shading: TexrectShading,
    pub(super) blend: TexrectBlendRegisters,
    pub(super) tile: TexrectTileBinding,
    pub(super) lut_mode: crate::TextureLutMode,
    pub(super) declared: Vec<ResourceAccess>,
}

/// One staged task member whose TMEM successor and read prefixes have been
/// atomically split. It owns no completed raster and cannot mutate the
/// coordinator, color registry, guest memory, or publication queue.
pub(super) struct DeferredAcffMember {
    pub(super) ordinal: u64,
    pub(super) candidate: CandidateColorTarget,
    pub(super) claimed: TargetRectangle,
    pub(super) effects: DeferredBackendEffectReport,
    pub(super) tmem: DeferredPhysicalTmemWithPrefixes,
    pub(super) commands: Vec<DeferredAcffCommand>,
    pub(super) checkpoint_accesses: Vec<ResourceAccess>,
}

pub(super) struct PreparedAcffMember {
    pub(super) ordinal: u64,
    pub(super) effects: BackendEffectReport,
    pub(super) physical: PhysicalTmemState,
    pub(super) checkpoint: SparseInitializedColorCheckpoint,
    pub(super) guest_writes: Vec<CompletedWrite>,
}

/// Fully validated owned raster parts, but deliberately not a redemption
/// capability. An outer task transaction must still preflight one infallible
/// shadow color-registry install before it mutates the coordinator, registry,
/// guest memory, or publication queue.
pub(super) struct PreparedAcffSegment {
    pub(super) members: Vec<PreparedAcffMember>,
    pub(super) final_color: OwnedTaskColorSegment,
    pub(super) draws: usize,
    pub(super) declared_pixels: u64,
    pub(super) band_jobs: usize,
}

fn preserve_earlier_result<T, E>(prior: Result<T, E>, later: Option<E>) -> Result<T, E> {
    match prior {
        Err(error) => Err(error),
        Ok(_) if later.is_some() => Err(later.expect("checked as present")),
        Ok(value) => Ok(value),
    }
}

fn first_terminal_failure<E>(
    raster: Option<E>,
    preparation: Option<E>,
    staging: Option<E>,
) -> Option<E> {
    raster.or(preparation).or(staging)
}

/// Executes and fully validates a non-empty compatible ACFF segment without
/// redeeming any coordinator, registry, guest-copy, or publication authority.
///
/// A staging failure from the first later member is escrowed by the caller.
/// All raster/checkpoint/effect/TMEM validation for the already-staged prefix
/// runs first, so a scalar-earlier failure wins. Only a completely validated
/// result can cross this function's return boundary.
pub(super) fn prepare_deferred_acff_segment(
    members: Vec<DeferredAcffMember>,
    initial_physical: &PhysicalTmemState,
    initial_bytes: Vec<u8>,
    initial_coverage: ColorCoverageState,
    workers: usize,
    later_staging_failure: Option<WgpuRawDpcExecutionError>,
) -> Result<PreparedAcffSegment, WgpuRawDpcExecutionError> {
    if members.is_empty() {
        return Err(
            later_staging_failure.unwrap_or(WgpuRawDpcExecutionError::AcffRowBinEmptySegment)
        );
    }
    let key = members[0].candidate.key();
    if !matches!(workers, 2 | 4 | 6 | 8) {
        return Err(WgpuRawDpcExecutionError::AcffRowBinInvalidWorkers { workers });
    }
    let expected_bytes = key.range().len() as usize;
    if initial_bytes.len() != expected_bytes
        || initial_coverage.len() != key.extent().pixels() as usize
    {
        return Err(WgpuRawDpcExecutionError::AcffRowBinInitialStateLength {
            expected_bytes,
            actual_bytes: initial_bytes.len(),
            expected_coverage: key.extent().pixels() as usize,
            actual_coverage: initial_coverage.len(),
        });
    }
    let mut checkpoint_limits = Vec::with_capacity(members.len());
    let mut checkpoint_accesses = Vec::with_capacity(members.len());
    let mut prepared = Vec::new();
    let mut declared_pixels = 0u64;
    let resident_len = initial_bytes.len();
    let mut expected_physical = initial_physical;
    let mut internal_preparation_failure = None;
    for (member_index, member) in members.iter().enumerate() {
        let member_draw_start = prepared.len();
        let preparation = (|| {
            member.tmem.validate_predecessor(expected_physical)?;
            if member.candidate.key() != key
                || (member_index > 0
                    && member.candidate.predecessor()
                        != Some(members[member_index - 1].candidate.generation()))
            {
                return Err(WgpuRawDpcExecutionError::AcffRowBinDiscontinuous {
                    member: member_index,
                    ordinal: member.ordinal,
                });
            }
            if member.commands.is_empty() {
                return Err(WgpuRawDpcExecutionError::AcffRowBinEmptyMember {
                    member: member_index,
                    ordinal: member.ordinal,
                });
            }
            if key.format() != ColorTargetFormat::Rgba16
                || member.commands.iter().any(|command| {
                    command.shading.combine().low() != 0xfc15_fea3
                        || command.shading.combine().high() != 0xf00f_f23f
                        || command.other_mode.high() != 0x0018_acff
                        || command.other_mode.low() != 0x0f0a_7008
                        || command.other_mode.depth_compare_enabled()
                        || command.other_mode.depth_update_enabled()
                        || !command.triangle.flags().shaded()
                        || !command.triangle.flags().textured()
                })
            {
                return Err(WgpuRawDpcExecutionError::AcffRowBinProgramMismatch {
                    member: member_index,
                    ordinal: member.ordinal,
                });
            }
            let flattened_accesses = member
                .commands
                .iter()
                .flat_map(|command| command.declared.iter().copied())
                .collect::<Vec<_>>();
            if flattened_accesses != member.checkpoint_accesses {
                return Err(
                    WgpuRawDpcExecutionError::AcffRowBinCheckpointAccessMismatch {
                        member: member_index,
                        ordinal: member.ordinal,
                    },
                );
            }
            for command in &member.commands {
                let prefix = member
                    .tmem
                    .prefixes()
                    .prefix_before(command.position)
                    .ok_or(WgpuRawDpcExecutionError::AcffRowBinMissingPrefix {
                        member: member_index,
                        ordinal: member.ordinal,
                        position: command.position.get(),
                    })?;
                let image = member.tmem.prefixes().image(prefix)?;
                prepared.push(PreparedRawTriangleRaster::try_new_exact(
                    &member.candidate,
                    command.other_mode,
                    &command.triangle,
                    command.shading,
                    command.blend,
                    &command.declared,
                    Some(RawTriangleTexture {
                        tile: command.tile,
                        tmem: image,
                        lut_mode: command.lut_mode,
                    }),
                    resident_len,
                )?);
            }
            Ok(())
        })();
        if let Err(error) = preparation {
            prepared.truncate(member_draw_start);
            internal_preparation_failure = Some(error);
            break;
        }
        checkpoint_limits.push(prepared.len());
        checkpoint_accesses.push(member.checkpoint_accesses.clone());
        declared_pixels = declared_pixels.saturating_add(
            member
                .checkpoint_accesses
                .iter()
                .map(|access| u64::from(access.region().declared_bytes()) / 2)
                .sum::<u64>(),
        );
        expected_physical = member.tmem.physical();
    }

    if checkpoint_limits.is_empty() {
        return Err(internal_preparation_failure
            .or(later_staging_failure)
            .unwrap_or(WgpuRawDpcExecutionError::AcffRowBinEmptySegment));
    }
    let draw_count = prepared.len();
    let execution = execute_prepared_raw_triangle_row_bin_prefix(
        key,
        &prepared,
        &checkpoint_limits,
        &checkpoint_accesses,
        initial_bytes,
        initial_coverage,
        workers,
    );
    let raster_failure = execution.error.map(|(draw, source)| {
        let member = checkpoint_limits.partition_point(|limit| *limit <= draw);
        WgpuRawDpcExecutionError::AcffRowBinRaster {
            member,
            ordinal: members[member].ordinal,
            draw,
            source,
        }
    });
    drop(prepared);

    let band_jobs = execution.band_jobs;
    let mut final_bytes = Some(execution.bytes);
    let mut final_coverage = Some(execution.coverage);
    let member_count = members.len();
    let terminal_failure = first_terminal_failure(
        raster_failure,
        internal_preparation_failure,
        later_staging_failure,
    );
    let successful_members = execution.checkpoints.len();
    let mut continuity: Option<OrderedCpuColorContinuity> = None;
    let mut prepared_members = Vec::with_capacity(successful_members);
    let mut final_initialized = None;
    for (member_index, (member, patches)) in
        members.into_iter().zip(execution.checkpoints).enumerate()
    {
        let (checkpoint, guest_writes) = SparseInitializedColorCheckpoint::from_row_bin_execution(
            &member.candidate,
            member.claimed,
            patches,
            &checkpoint_accesses[member_index],
        )?;
        let writes = merge_deferred_packet_writes(
            member.effects.expected_writes(),
            &guest_writes,
            member.tmem.proposed_effects(),
        )?;
        let effects = member
            .effects
            .complete(writes)
            .map_err(WgpuRawDpcExecutionError::Effect)?;
        let physical = member.tmem.complete(&effects)?;
        let reservation = OrderedCpuCandidateReservation::new(&member.candidate);
        continuity = Some(match continuity {
            None => OrderedCpuColorContinuity::start_reserved(reservation),
            Some(prior) => prior.append_reserved(reservation)?,
        });
        if terminal_failure.is_none() && member_index + 1 == member_count {
            let device_bytes = crate::DeviceColorBytes::new_for_fill(
                key,
                member.candidate.generation(),
                key.format(),
                final_bytes.take().expect("segment tail owns final bytes"),
            )?;
            let final_completed = CompletedColorTargetWrite::new_for_fill(
                key,
                member.candidate.generation(),
                key.range(),
                member.claimed,
                device_bytes,
            )
            .with_coverage(
                final_coverage
                    .take()
                    .expect("segment tail owns final coverage"),
            );
            final_initialized = Some(
                member
                    .candidate
                    .admit_completed_initialization(final_completed)?,
            );
        }
        prepared_members.push(PreparedAcffMember {
            ordinal: member.ordinal,
            effects,
            physical,
            checkpoint,
            guest_writes,
        });
    }
    if let Some(error) = terminal_failure {
        return Err(error);
    }
    if successful_members != member_count {
        return Err(WgpuRawDpcExecutionError::ComputeRasterCheckpointCount {
            expected: member_count,
            actual: successful_members,
        });
    }
    let final_color = continuity
        .expect("nonempty segment establishes continuity")
        .finish(final_initialized.expect("segment tail was admitted"))?;
    Ok(PreparedAcffSegment {
        members: prepared_members,
        final_color,
        draws: draw_count,
        declared_pixels,
        band_jobs,
    })
}

#[cfg(test)]
mod tests {
    use super::{first_terminal_failure, prepare_deferred_acff_segment, preserve_earlier_result};

    #[test]
    fn earlier_raster_or_validation_error_beats_later_staging_failure() {
        assert_eq!(
            preserve_earlier_result::<(), _>(Err("earlier-raster"), Some("later-stage")),
            Err("earlier-raster")
        );
        assert_eq!(
            preserve_earlier_result::<(), _>(Err("earlier-validation"), Some("later-stage")),
            Err("earlier-validation")
        );
        assert_eq!(
            preserve_earlier_result(Ok("validated"), Some("later-stage")),
            Err("later-stage")
        );
        assert_eq!(
            preserve_earlier_result(Ok("validated"), None::<&str>),
            Ok("validated")
        );
        assert_eq!(
            first_terminal_failure(
                Some("earlier-raster"),
                Some("later-preparation"),
                Some("later-stage"),
            ),
            Some("earlier-raster")
        );
        assert_eq!(
            first_terminal_failure(None, Some("earlier-preparation"), Some("later-stage")),
            Some("earlier-preparation")
        );
    }

    #[test]
    fn first_staging_failure_is_not_hidden_by_a_synthetic_empty_segment_error() {
        let error = match prepare_deferred_acff_segment(
            Vec::new(),
            &crate::tmem::PhysicalTmemState::try_new().unwrap(),
            Vec::new(),
            crate::targets::ColorCoverageState::unknown(
                crate::targets::ColorTargetExtent::try_new(1, 1).unwrap(),
            ),
            4,
            Some(super::WgpuRawDpcExecutionError::NoCompletedLoads),
        ) {
            Err(error) => error,
            Ok(_) => panic!("the first staging failure must be returned"),
        };
        assert!(matches!(
            error,
            super::WgpuRawDpcExecutionError::NoCompletedLoads
        ));
    }
}
