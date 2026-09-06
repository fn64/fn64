use super::*;

/// Which color-target-writing command one entry of the ordered accumulation
/// schedule names, paired with its index into the plan's own per-kind list.
///
/// Deliberately carries only the *index*, never the command payload: the
/// payload is read back out of `collector.plan` at execution time so this
/// schedule cannot become a second, drifting copy of the plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ColorCommandKind {
    Fill(usize),
    Texrect(usize),
    /// A flat raw triangle that declared a per-scanline write run. Indexes
    /// `plan.raw_triangle_commands`, which holds only the triangles that
    /// declared one.
    RawTriangle(usize),
}

/// **Which TMEM image this packet's texrects sample, chosen once per packet
/// by the packet's own load count -- never a fallback.**
///
/// The RDP's TMEM is durable across submissions. A texrect samples whatever
/// is in TMEM at its own stream position, which is:
///
/// - [`Self::Pending`] -- this packet's own staged loads, when the packet
///   carries at least one. **Not one image for the whole packet:** the
///   variant carries the per-load prefix snapshots alongside the sealed
///   transaction, and each texrect is served the prefix taken after the
///   last load BEFORE its own stream position. A texrect that precedes
///   every load in its packet reads no prefix at all and falls to durable
///   committed TMEM, because that is what TMEM holds at that position.
/// - [`Self::Committed`] -- the coordinator's durable [`PhysicalTmemState`],
///   when the packet carries **zero** loads. There is no proposal to
///   observe, and the durable state is not "stale": it is precisely the
///   result of every load an earlier packet already published, which is the
///   only thing the hardware's TMEM could contain.
///
/// The two are not interchangeable and the choice is not a heuristic. A
/// packet with loads must **not** read `Committed`, because the coordinator
/// has not published this packet's own loads yet and the texrect would miss
/// texels the wire stream placed before it -- the defect commit `3a1a6a73`
/// measured as `TMEM_SAMPLE_STATUS_INVALID_BYTE`. A packet without loads
/// must not read a `Pending` image, because none exists.
///
/// **This is deliberately not a fallback for a missing pending image.** The
/// selection is made from `plan.loads.is_empty()` -- a fact about the wire
/// stream -- before any staging runs, not from a `None` observed after
/// staging failed. A `None` where a pending image was expected stays a
/// named refusal.
pub(super) enum TexrectTmemSource<'a> {
    Pending {
        pending: &'a crate::tmem::PendingTmemTransaction,
        /// TMEM as of each of this packet's loads, keyed by the load's own
        /// stream command index, in stream order.
        prefixes: &'a [(u32, crate::tmem::TmemPrefixSnapshot)],
    },
    Committed(&'a PhysicalTmemState),
}

pub(super) struct ComputeRasterReplacementPlan {
    pub(super) dispatches: Vec<ComputeRasterDispatch>,
    pub(super) declared: Vec<ResourceAccess>,
    pub(super) claimed: TargetRectangle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskComputeAdmissionRefusal {
    MixedColorCommands,
    TargetFormat,
    Untextured,
    AffineTexture,
    Depth,
    CycleType([u32; 4]),
    ProgramBits([u32; 4]),
    EmptyAccesses,
    AccessMode,
    AccessPurpose,
    AccessRegion,
    AccessOutsideTarget,
    CommandOrder,
    EmptyDispatch,
    NoDispatches,
}

impl From<ComputeRasterAdmissionRefusal> for TaskComputeAdmissionRefusal {
    fn from(reason: ComputeRasterAdmissionRefusal) -> Self {
        match reason {
            ComputeRasterAdmissionRefusal::TargetFormat => Self::TargetFormat,
            ComputeRasterAdmissionRefusal::Untextured => Self::Untextured,
            ComputeRasterAdmissionRefusal::AffineTexture => Self::AffineTexture,
            ComputeRasterAdmissionRefusal::Depth => Self::Depth,
            ComputeRasterAdmissionRefusal::CycleType(words) => Self::CycleType(words),
            ComputeRasterAdmissionRefusal::ProgramBits(words) => Self::ProgramBits(words),
            ComputeRasterAdmissionRefusal::EmptyAccesses => Self::EmptyAccesses,
            ComputeRasterAdmissionRefusal::AccessMode => Self::AccessMode,
            ComputeRasterAdmissionRefusal::AccessPurpose => Self::AccessPurpose,
            ComputeRasterAdmissionRefusal::AccessRegion => Self::AccessRegion,
            ComputeRasterAdmissionRefusal::AccessOutsideTarget => Self::AccessOutsideTarget,
            ComputeRasterAdmissionRefusal::CommandOrder => Self::CommandOrder,
        }
    }
}

pub(super) enum ComputeRasterReplacementAdmission {
    Admitted(ComputeRasterReplacementPlan),
    Refused(TaskComputeAdmissionRefusal),
}

pub(super) fn compute_replacement_target_pixels(
    plan: &ComputeRasterReplacementPlan,
    key: ColorTargetKey,
    target_width: u32,
) -> Result<u32, WgpuRawDpcExecutionError> {
    plan.dispatches.iter().try_fold(0u32, |count, dispatch| {
        let accesses: Vec<_> = dispatch
            .batch
            .draws()
            .iter()
            .flat_map(ComputeRasterDrawAdmission::accesses)
            .copied()
            .collect();
        let first_triangle_index = dispatch
            .batch
            .draws()
            .first()
            .expect("a sealed compute dispatch has an admitted draw")
            .triangle_index();
        let claimed = claimed_rectangle_from_accesses(key, &accesses, first_triangle_index)?;
        let column_count = if compute_column_bounds_enabled() {
            let first = claimed.x() & !1;
            let limit = claimed
                .x()
                .checked_add(claimed.width())
                .expect("claimed rectangle was checked when constructed")
                .checked_add(1)
                .map(|limit| limit & !1)
                .unwrap_or(target_width)
                .min(target_width);
            limit - first
        } else {
            target_width
        };
        let dispatch_pixels = column_count
            .checked_mul(claimed.height())
            .expect("bounded replacement dispatch target-pixel count fits u32");
        Ok(count
            .checked_add(dispatch_pixels)
            .expect("bounded replacement chain target-pixel count fits u32"))
    })
}

pub(super) fn retain_compute_replacement_draw<S: crate::TmemByteSource + ?Sized>(
    builder: &mut Option<ComputeRasterProbeBuilder>,
    dispatches: &mut Vec<ComputeRasterDispatch>,
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: CommandIndex,
    tmem: &S,
) -> Result<Option<TaskComputeAdmissionRefusal>, WgpuRawDpcExecutionError> {
    if let Some(active) = builder.as_mut() {
        match active.push(collector, candidate, index, tmem)? {
            ComputeRasterProbePush::Admitted => return Ok(None),
            ComputeRasterProbePush::SplitDispatch => {}
            ComputeRasterProbePush::Refused(reason) => return Ok(Some(reason.into())),
        }
    }
    if let Some(previous) = builder.take() {
        let Some((dispatch, _)) = previous.finish_dispatch() else {
            return Ok(Some(TaskComputeAdmissionRefusal::EmptyDispatch));
        };
        dispatches.push(dispatch);
    }
    let mut next = ComputeRasterProbeBuilder::new(candidate, Vec::new());
    match next.push(collector, candidate, index, tmem)? {
        ComputeRasterProbePush::Admitted => {}
        ComputeRasterProbePush::SplitDispatch => {
            return Ok(Some(TaskComputeAdmissionRefusal::EmptyDispatch));
        }
        ComputeRasterProbePush::Refused(reason) => return Ok(Some(reason.into())),
    }
    *builder = Some(next);
    Ok(None)
}

pub(super) fn claimed_rectangle_from_accesses(
    key: ColorTargetKey,
    accesses: &[ResourceAccess],
    triangle_index: usize,
) -> Result<TargetRectangle, WgpuRawDpcExecutionError> {
    verify_accesses_inside(accesses, key)?;
    let base = key.address().get();
    let target_width = key.extent().width();
    let bytes_per_pixel = key.format().bytes_per_pixel();
    let mut claimed = None;
    for access in accesses {
        let fn64_render_ir::ResourceRegion::Rdram { range, .. } = access.region() else {
            return Err(WgpuRawDpcExecutionError::FillAccessRegionKind {
                access_index: access.operation().get(),
            });
        };
        let offset = range.start().get().checked_sub(base).ok_or(
            WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                access_index: access.operation().get(),
            },
        )?;
        if offset % bytes_per_pixel != 0 || range.len() == 0 || range.len() % bytes_per_pixel != 0 {
            return Err(WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                access_index: access.operation().get(),
            });
        }
        let first_pixel = offset / bytes_per_pixel;
        let x = first_pixel % target_width;
        let y = first_pixel / target_width;
        let width = range.len() / bytes_per_pixel;
        if x.checked_add(width)
            .is_none_or(|right| right > target_width)
        {
            return Err(WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                access_index: access.operation().get(),
            });
        }
        claimed = Some(union_target_rectangle(
            TargetRectangle::try_new(x, y, width, 1)?,
            claimed,
        ));
    }
    claimed.ok_or(WgpuRawDpcExecutionError::RawTriangleDeclaredNoWrite { triangle_index })
}

pub(super) fn plan_compute_raster_replacement(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    schedule: &[(u32, ColorCommandKind)],
    tmem: &TexrectTmemSource<'_>,
) -> Result<ComputeRasterReplacementAdmission, WgpuRawDpcExecutionError> {
    if schedule
        .iter()
        .any(|(_, kind)| !matches!(kind, ColorCommandKind::RawTriangle(_)))
    {
        return Ok(ComputeRasterReplacementAdmission::Refused(
            TaskComputeAdmissionRefusal::MixedColorCommands,
        ));
    }
    let mut builder = None;
    let mut dispatches = Vec::new();
    for (_, kind) in schedule {
        let ColorCommandKind::RawTriangle(index) = *kind else {
            unreachable!("the all-raw-triangle preflight rejected every other command kind")
        };
        let index = CommandIndex::new(index);
        let command_index = collector.plan.raw_triangle_commands[index].command_index;
        let refusal = match tmem {
            TexrectTmemSource::Pending { pending, prefixes } => {
                match prefix_before(prefixes, command_index) {
                    Some(prefix) => retain_compute_replacement_draw(
                        &mut builder,
                        &mut dispatches,
                        collector,
                        candidate,
                        index,
                        &pending.prefix_image(prefix)?,
                    )?,
                    None => retain_compute_replacement_draw(
                        &mut builder,
                        &mut dispatches,
                        collector,
                        candidate,
                        index,
                        collector.physical,
                    )?,
                }
            }
            TexrectTmemSource::Committed(state) => retain_compute_replacement_draw(
                &mut builder,
                &mut dispatches,
                collector,
                candidate,
                index,
                *state,
            )?,
        };
        if let Some(reason) = refusal {
            return Ok(ComputeRasterReplacementAdmission::Refused(reason));
        }
    }
    if let Some(builder) = builder {
        let Some((dispatch, _)) = builder.finish_dispatch() else {
            return Ok(ComputeRasterReplacementAdmission::Refused(
                TaskComputeAdmissionRefusal::EmptyDispatch,
            ));
        };
        dispatches.push(dispatch);
    }
    if dispatches.is_empty() {
        return Ok(ComputeRasterReplacementAdmission::Refused(
            TaskComputeAdmissionRefusal::NoDispatches,
        ));
    }
    let declared: Vec<_> = dispatches
        .iter()
        .flat_map(|dispatch| dispatch.batch.draws())
        .flat_map(ComputeRasterDrawAdmission::accesses)
        .copied()
        .collect();
    let first_triangle_index = dispatches[0]
        .batch
        .draws()
        .first()
        .expect("a sealed compute dispatch has an admitted draw")
        .triangle_index();
    let claimed =
        claimed_rectangle_from_accesses(candidate.key(), &declared, first_triangle_index)?;
    Ok(ComputeRasterReplacementAdmission::Admitted(
        ComputeRasterReplacementPlan {
            dispatches,
            declared,
            claimed,
        },
    ))
}

/// **The N-command accumulation seam.**
///
/// Executes every admitted `FillRectangle` and `TextureRectangle` this
/// packet carried against one shared full-extent buffer, in the packet's
/// own command order, and returns the single staged token the composed
/// result publishes through.
///
/// ## Why one buffer and one candidate, not N of each
///
/// `begin_candidate` derives its generation from the registry, and this
/// staging path deliberately does not publish into the registry (that is
/// `publish_raw_dpc`'s job, after the guest commit). So a second
/// `begin_candidate` call would hand back the *same* generation as the
/// first, not a successor -- N candidates would be N copies of one
/// candidate, and N `admit_completed_initialization` calls would publish N
/// initializations of a single generation. One candidate is therefore not
/// an optimization; it is the only shape that does not forge a generation.
///
/// The buffer is threaded the same way for the same reason: each command
/// takes the accumulated buffer as its own `resident_bytes` and its
/// full-extent output *becomes* the accumulator for the next. That is what
/// makes a later command's pixels win an overlap and an earlier command's
/// pixels survive outside it -- the accumulation is the composition, not a
/// blend policy layered over it.
///
/// ## Order is derived, never chosen
///
/// The schedule is built by sorting on the `command_index` the decoder's
/// own stream walk assigned (`PlanCollector::command` increments it once
/// per wire command, and both `fills` and `texrect_commands` record it).
/// That index is the packet's command order by construction. It is a
/// *recovery* of the stream's order, not a policy: `merged_fill_and_tmem_
/// writes` independently re-derives the same order from the resource
/// journal to build the effect report, and the two agreeing is the
/// cross-check. Note the asymmetry that makes this real evidence rather
/// than a tautology -- the journal is `raw_dpc::push_access`'s `planned`
/// vector and the command index is `PlanCollector`'s own counter, two
/// separate walks of the same stream.
///
/// ## Digest staleness across N commands
///
/// A `CompletedWrite` claims "this range holds content with this digest".
/// With N commands writing one buffer at overlapping ranges, a digest
/// computed when its own command staged describes a buffer state that no
/// longer exists the moment any later command touches an overlapping byte
/// -- and `rsp_commit`'s `copy_committed_guest_writes` re-derives every
/// digest from the bytes before writing any of them and aborts the whole
/// copy on a mismatch. Measured: it did, naming write #0, when only the
/// two-command case was handled.
///
/// The fix is not per-command patching but a single rule: **every write's
/// digest is computed once, against the final composed buffer, after the
/// last command has run.** Each command contributes only its *accesses*
/// (the journal's fact, never re-derived) during the loop; the digests are
/// all filled in together at the end, so no write can carry a digest from
/// an intermediate state. A per-command recomputation would be O(N^2) and,
/// worse, would still be stale for every write except the last.
pub(super) fn stage_color_commands(
    collector: &mut ExecutionCollector<'_>,
    packet: &WorkloadPacket,
    tmem: TexrectTmemSource<'_>,
) -> Result<Option<StagedFill>, WgpuRawDpcExecutionError> {
    // The ordered schedule, recovered from the decoder's own per-command
    // stream index. `sort_by_key` on that index is not a sort *policy*: the
    // index IS the stream position, so this recovers an interleaving the
    // decoder already fixed rather than imposing one. Stable, so two
    // entries that somehow shared an index would keep their relative plan
    // order rather than being silently transposed.
    let cpu_phase_attributed = collector.task_cpu_phase_census.is_some()
        && ordered_depth_free_acff_triangle_member(collector);
    let plan = &collector.plan;
    let schedule = task_cpu_phase_census::timed(
        collector.task_cpu_phase_census.as_deref_mut(),
        cpu_phase_attributed,
        task_cpu_phase_census::Phase::ScheduleDecodeRowPrepRaster,
        || {
            let mut schedule: Vec<(u32, ColorCommandKind)> =
                plan.fills
                    .iter()
                    .enumerate()
                    .map(|(index, (command_index, ..))| {
                        (*command_index, ColorCommandKind::Fill(index))
                    })
                    .chain(plan.texrect_commands.iter().enumerate().map(
                        |(index, (_, _, _, command_index, _))| {
                            (*command_index, ColorCommandKind::Texrect(index))
                        },
                    ))
                    .chain(plan.raw_triangle_commands.iter().enumerate().map(
                        |(index, scheduled)| {
                            (
                                scheduled.command_index,
                                ColorCommandKind::RawTriangle(index),
                            )
                        },
                    ))
                    .collect();
            schedule.sort_by_key(|(command_index, _)| *command_index);
            schedule
        },
    );
    if schedule.is_empty() {
        return Ok(None);
    }

    // The candidate, and the target key, derived once from this packet's
    // own staged `SetColorImage`. Every command in the schedule composes
    // into the same target by construction -- `key_of_declared_render_
    // target` cross-checks each texrect's declared accesses against this
    // key's range, and a fill naming a different image would produce a
    // different key here and be caught by the same check.
    let key = color_target_key(collector, packet)?;
    if collector.defer_compute_replacement {
        let registry = collector
            .color_targets
            .as_ref()
            .expect("color_target_key populates the registry");
        let batch = collector
            .color_execution_batch
            .as_deref()
            .expect("deferred compute execution requires a task color planner");
        let (preview, task_input) = batch.preview_candidate(registry, key)?;
        let plan = match plan_compute_raster_replacement(collector, &preview, &schedule, &tmem)? {
            ComputeRasterReplacementAdmission::Admitted(plan) => plan,
            ComputeRasterReplacementAdmission::Refused(reason) => {
                return Err(WgpuRawDpcExecutionError::TaskBatchComputeNotAdmitted {
                    ordinal: collector.ordinal,
                    reason,
                });
            }
        };
        let program_attribution = compute_program_attribution_from_ids(
            plan.dispatches
                .iter()
                .flat_map(|dispatch| dispatch.batch.draws())
                .map(|draw| draw.program().shader_id()),
        );

        // Exact admission completed without mutating the generation planner.
        // Reserve only now. No other operation can interleave between preview
        // and reservation because both values are held inside this exclusive
        // execution borrow.
        let (candidate, reserved_input) = collector
            .color_execution_batch
            .as_deref_mut()
            .expect("the previewed task color planner remains present")
            .begin_candidate(registry, key)?;
        assert_eq!(candidate, preview);
        assert_eq!(reserved_input, task_input);

        let initial_bytes = match task_input {
            TaskColorInput::DurableRegistry => Some(
                registry
                    .residents()
                    .iter()
                    .find(|resident| resident.key() == key)
                    .map(|resident| resident.device_bytes().device_bytes().to_vec())
                    .ok_or(crate::targets::TexrectExecutionError::MissingResidentBytes { key })?,
            ),
            TaskColorInput::PriorTaskCheckpoint => None,
        };
        collector.deferred_compute = Some(DeferredComputeColor {
            candidate,
            plan,
            program_attribution,
            initial_bytes,
        });
        return Ok(None);
    }

    let wants_depth = collector.plan.triangles.iter().any(|planned| {
        planned.draw.as_ref().is_ok_and(|draw| {
            draw.other_mode.depth_compare_enabled() || draw.other_mode.depth_update_enabled()
        })
    });
    let ordered_cpu_eligible = !wants_depth
        && schedule
            .iter()
            .all(|(_, command)| matches!(command, ColorCommandKind::RawTriangle(_)))
        && !collector.defer_compute_replacement
        && !collector.compute_replacement_enabled;

    let mut ordered_seed: Option<(Vec<u8>, ColorCoverageState)> = None;
    let (candidate, task_input) = task_cpu_phase_census::timed(
        collector.task_cpu_phase_census.as_deref_mut(),
        cpu_phase_attributed,
        task_cpu_phase_census::Phase::CandidateSeedCopy,
        || -> Result<_, WgpuRawDpcExecutionError> {
            Ok(match collector.ordered_cpu_color_batch.as_deref_mut() {
                Some(batch) if ordered_cpu_eligible => {
                    let registry = collector
                        .color_targets
                        .as_mut()
                        .expect("color_target_key populates the registry");
                    let (candidate, seed) = batch.begin_member(registry, key)?;
                    ordered_seed = seed;
                    (candidate, None)
                }
                Some(batch) => {
                    if batch.tail.is_some() {
                        batch.flush(
                            collector
                                .color_targets
                                .as_mut()
                                .expect("an ordered CPU accumulator implies a registry"),
                        )?;
                    }
                    let registry = collector
                        .color_targets
                        .as_ref()
                        .expect("color_target_key populates the registry");
                    (registry.begin_candidate(key)?, None)
                }
                None => match collector.color_execution_batch.as_deref_mut() {
                    Some(batch) => {
                        let registry = collector
                            .color_targets
                            .as_ref()
                            .expect("color_target_key populates the registry");
                        let (candidate, input) = batch.begin_candidate(registry, key)?;
                        (candidate, Some(input))
                    }
                    None => {
                        let registry = collector
                            .color_targets
                            .as_ref()
                            .expect("color_target_key populates the registry");
                        (registry.begin_candidate(key)?, None)
                    }
                },
            })
        },
    )?;

    // The accumulator. Seeded from the resident's real prior bytes when
    // this target already exists, and left `None` for a brand-new target --
    // exactly the distinction `execute_fill_rectangle` already draws, and
    // deliberately NOT flattened to a zero buffer here, which would
    // fabricate content for a resident whose bytes failed to thread.
    let (mut accumulated, mut ordered_coverage) = task_cpu_phase_census::timed(
        collector.task_cpu_phase_census.as_deref_mut(),
        cpu_phase_attributed,
        task_cpu_phase_census::Phase::CandidateSeedCopy,
        || {
            if ordered_cpu_eligible {
                let seed = ordered_seed.or_else(|| {
                    if task_input == Some(TaskColorInput::PriorTaskCheckpoint) {
                        None
                    } else {
                        collector
                            .color_targets
                            .as_ref()
                            .expect("color_target_key populates the registry")
                            .residents()
                            .iter()
                            .find(|resident| resident.key() == key)
                            .map(|resident| {
                                (
                                    resident.device_bytes().device_bytes().to_vec(),
                                    resident.coverage().clone(),
                                )
                            })
                    }
                });
                match seed {
                    Some((bytes, coverage)) => (Some(bytes), Some(coverage)),
                    None => (
                        None,
                        Some(ColorCoverageState::unknown(candidate.key().extent())),
                    ),
                }
            } else {
                let bytes = ordered_seed.map(|(bytes, _)| bytes).or_else(|| {
                    if task_input == Some(TaskColorInput::PriorTaskCheckpoint) {
                        None
                    } else {
                        collector
                            .color_targets
                            .as_ref()
                            .expect("color_target_key populates the registry")
                            .residents()
                            .iter()
                            .find(|resident| resident.key() == key)
                            .map(|resident| resident.device_bytes().device_bytes().to_vec())
                    }
                });
                (bytes, None)
            }
        },
    );

    // **The depth accumulator, one RDP depth-memory cell per target pixel,
    // persisting across every draw in this packet's schedule.** It is the
    // z-buffer: a later draw's fragment sees the depth an earlier draw
    // committed, which is what makes overlapping triangles at different
    // depths resolve. Allocated (seeded to `(0, 0)` -- the value a zeroed
    // guest z-image decodes to) only when some raw triangle in this packet
    // actually requests a depth compare or update; a packet with no z-wired
    // draw keeps it `None` and every draw resolves by painter's order,
    // exactly as before. The z-image binding (`SetZImage`/`SetMaskImage`) is
    // what legalises those OtherMode z bits in the admitted subset -- they
    // are only ever set in a packet that also bound a z-image -- so keying
    // the accumulator off the z bits is equivalent here to keying it off the
    // binding, without threading the address through the neutral IR.
    let mut depth_accum: Option<Vec<crate::targets::DepthCell>> =
        wants_depth.then(|| vec![(0u32, 0u8); key.extent().pixels() as usize]);

    if collector.compute_replacement_enabled {
        let admission_started = Instant::now();
        let replacement =
            match plan_compute_raster_replacement(collector, &candidate, &schedule, &tmem)? {
                ComputeRasterReplacementAdmission::Admitted(plan) => Some(plan),
                ComputeRasterReplacementAdmission::Refused(_) => None,
            }
            .map(|plan| -> Result<_, WgpuRawDpcExecutionError> {
                let target_pixels = compute_replacement_target_pixels(
                    &plan,
                    key,
                    candidate.key().extent().width(),
                )?;
                Ok((plan, target_pixels))
            })
            .transpose()?;
        if let Some((plan, target_pixels)) = replacement.filter(|(_, target_pixels)| {
            compute_raster_replacement_admitted(*target_pixels, compute_raster_min_target_pixels())
        }) {
            let admission_elapsed = admission_started.elapsed();
            let initial = accumulated
                .take()
                .ok_or(crate::targets::TexrectExecutionError::MissingResidentBytes { key })?;
            let pipeline = collector
                .compute_replacement_pipeline
                .as_deref_mut()
                .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
            let dispatches: Vec<_> = plan
                .dispatches
                .iter()
                .map(|dispatch| {
                    let accesses: Vec<_> = dispatch
                        .batch
                        .draws()
                        .iter()
                        .flat_map(ComputeRasterDrawAdmission::accesses)
                        .copied()
                        .collect();
                    let first_triangle_index = dispatch
                        .batch
                        .draws()
                        .first()
                        .expect("a sealed compute dispatch has an admitted draw")
                        .triangle_index();
                    let claimed =
                        claimed_rectangle_from_accesses(key, &accesses, first_triangle_index)?;
                    let target_width = candidate.key().extent().width();
                    let (first_column, column_count) = if compute_column_bounds_enabled() {
                        let first = claimed.x() & !1;
                        let limit = claimed
                            .x()
                            .checked_add(claimed.width())
                            .expect("claimed rectangle was checked when constructed")
                            .checked_add(1)
                            .map(|limit| limit & !1)
                            .unwrap_or(target_width)
                            .min(target_width);
                        (first, limit - first)
                    } else {
                        (0, target_width)
                    };
                    Ok(ComputeHotColorDispatch {
                        triangles: &dispatch.triangles,
                        tmem: &dispatch.tmem,
                        tile: dispatch.tile,
                        first_row: claimed.y(),
                        row_count: claimed.height(),
                        first_column,
                        column_count,
                    })
                })
                .collect::<Result<_, WgpuRawDpcExecutionError>>()?;
            let extent = plan.dispatches[0].extent;
            let started = Instant::now();
            let output = pipeline
                .compute_triangle_hot_color_chain(extent, &initial, &dispatches)
                .map_err(WgpuRawDpcExecutionError::TriangleDraw)?;
            let elapsed = started.elapsed();
            let draw_count = plan.dispatches.iter().try_fold(0u32, |count, dispatch| {
                count.checked_add(u32::try_from(dispatch.batch.draws().len()).ok()?)
            });
            let draw_count = draw_count.expect("bounded raw-DPC replacement draw count fits u32");
            let batch_count = u32::try_from(plan.dispatches.len())
                .expect("bounded raw-DPC replacement batch count fits u32");
            let effects_started = Instant::now();
            let device_bytes = crate::DeviceColorBytes::new_for_fill(
                key,
                candidate.generation(),
                key.format(),
                output,
            )?;
            let completed = CompletedColorTargetWrite::new_for_fill(
                key,
                candidate.generation(),
                key.range(),
                plan.claimed,
                device_bytes,
            );
            let guest_writes =
                fill_completed_writes(key, completed.device_bytes(), &plan.declared)?;
            let initialized = candidate.admit_completed_initialization(completed)?;
            let effects_elapsed = effects_started.elapsed();
            collector.compute_replacement_receipt = Some(ComputeRasterProbeReceipt {
                submission_count: 1,
                batch_count,
                draw_count,
                target_pixels,
                admission_elapsed,
                elapsed,
                effects_elapsed,
            });
            return Ok(Some(StagedFill {
                initialized,
                guest_writes,
                prepared_sparse_checkpoint: None,
                cpu_phase_attributed: false,
            }));
        }
    }

    // Accesses only, in schedule order. Digests are deliberately absent
    // until the loop ends -- see this function's own doc on staleness.
    let mut declared: Vec<ResourceAccess> = Vec::new();
    let mut claimed: Option<TargetRectangle> = None;
    let mut last_completed: Option<CompletedColorTargetWrite> = None;

    let move_accumulator = move_color_accumulator_enabled();
    let own_command_input = own_color_command_input_enabled();
    let mut compute_probe_builder = None;
    let schedule_started = task_cpu_phase_census::started(
        collector.task_cpu_phase_census.as_deref(),
        cpu_phase_attributed,
    );
    for (schedule_index, (_, kind)) in schedule.iter().enumerate() {
        if compute_probe_builder.is_some() && !matches!(*kind, ColorCommandKind::RawTriangle(_)) {
            let expected = accumulated
                .as_deref()
                .expect("an active compute batch has resident CPU output");
            flush_compute_probe(
                &mut compute_probe_builder,
                collector.ordinal,
                expected,
                &mut collector.compute_probes,
            );
        }
        let color_phase = match *kind {
            ColorCommandKind::Fill(_) => raw_dpc_execute_census::Phase::ColorFill,
            ColorCommandKind::Texrect(_) => raw_dpc_execute_census::Phase::ColorTexrect,
            ColorCommandKind::RawTriangle(_) => raw_dpc_execute_census::Phase::ColorTriangle,
        };
        let (completed, accesses) = raw_dpc_execute_census::timed(
            color_phase,
            || -> Result<_, WgpuRawDpcExecutionError> {
                Ok(match *kind {
                    ColorCommandKind::Fill(index) => execute_scheduled_fill(
                        collector,
                        &candidate,
                        index,
                        if own_command_input {
                            accumulated.take()
                        } else {
                            accumulated.clone()
                        },
                    )?,
                    ColorCommandKind::Texrect(index) => {
                        // **A texrect samples TMEM at its OWN stream position.**
                        //
                        // `stage_and_report` chose the family once, upstream, from
                        // a fact about the wire stream (does this packet carry
                        // loads at all). Within the pending family the position is
                        // still per command, because TMEM is durable within a
                        // packet: a texrect observes every load before it and no
                        // load after it. Selecting on the command index the
                        // decoder's own stream walk assigned -- the same index this
                        // schedule is sorted by -- keeps that a recovery of the
                        // stream's order rather than a policy.
                        let resident =
                            color_command_input(&mut accumulated, own_command_input, key)?;
                        let command_index = collector.plan.texrect_commands[index].3;
                        match tmem {
                            TexrectTmemSource::Pending { pending, prefixes } => {
                                match prefix_before(prefixes, command_index) {
                                    Some(prefix) => execute_scheduled_texrect(
                                        collector,
                                        &candidate,
                                        &pending.prefix_image(prefix)?,
                                        true,
                                        index,
                                        resident,
                                        claimed,
                                    )?,
                                    // No load precedes this texrect in its own
                                    // packet, so what TMEM holds here is exactly
                                    // what an earlier packet published: durable
                                    // committed state, read through the same one
                                    // sampler. Not a fallback for a missing image
                                    // -- the absence of a preceding load IS the
                                    // stream fact that makes committed correct.
                                    None => execute_scheduled_texrect(
                                        collector,
                                        &candidate,
                                        collector.physical,
                                        false,
                                        index,
                                        resident,
                                        claimed,
                                    )?,
                                }
                            }
                            TexrectTmemSource::Committed(state) => execute_scheduled_texrect(
                                collector, &candidate, state, false, index, resident, claimed,
                            )?,
                        }
                    }
                    ColorCommandKind::RawTriangle(index) => {
                        let index = CommandIndex::new(index);
                        // Same resident-bytes requirement as a texrect and for the
                        // same reason: a triangle writes a sub-region, so every
                        // pixel outside it must come from real prior content.
                        let resident =
                            color_command_input(&mut accumulated, own_command_input, key)?;
                        let command_coverage = ordered_coverage
                            .as_mut()
                            .map(ColorCoverageState::take_for_command);
                        // **A raw triangle samples TMEM at its OWN stream position,
                        // by the SAME rule a texrect does.**
                        //
                        // Not a parallel implementation: this is the identical
                        // `prefix_before` call over the identical `prefixes` slice,
                        // dispatched on the identical `TexrectTmemSource` the arm
                        // above matches on. WM2000's own triangle packets carry NINE
                        // TMEM loads each, so "which load did this draw see" is a
                        // live question for a triangle exactly as it is for a
                        // texrect -- and answering it with a per-packet image would
                        // draw every triangle with the ninth load's texels.
                        let command_index =
                            collector.plan.raw_triangle_commands[index].command_index;
                        match tmem {
                            TexrectTmemSource::Pending { pending, prefixes } => {
                                match prefix_before(prefixes, command_index) {
                                    Some(prefix) => {
                                        let image = pending.prefix_image(prefix)?;
                                        if collector.collect_compute_probe {
                                            if let Some(previous) = retain_compute_probe_draw(
                                                &mut compute_probe_builder,
                                                collector,
                                                &candidate,
                                                index,
                                                &image,
                                                resident.as_ref(),
                                            )? {
                                                push_finished_compute_probe(
                                                    previous,
                                                    collector.ordinal,
                                                    resident.as_ref(),
                                                    &mut collector.compute_probes,
                                                );
                                            }
                                        }
                                        execute_scheduled_raw_triangle(
                                            collector,
                                            &candidate,
                                            index,
                                            resident,
                                            &image,
                                            true,
                                            depth_accum.as_deref_mut(),
                                            command_coverage,
                                        )?
                                    }
                                    // No load precedes this triangle in its own
                                    // packet, so TMEM holds exactly what an earlier
                                    // packet published -- durable committed state,
                                    // read through the same one sampler. The absence
                                    // of a preceding load IS the stream fact that
                                    // makes committed correct; it is not a fallback.
                                    None => {
                                        if collector.collect_compute_probe {
                                            if let Some(previous) = retain_compute_probe_draw(
                                                &mut compute_probe_builder,
                                                collector,
                                                &candidate,
                                                index,
                                                collector.physical,
                                                resident.as_ref(),
                                            )? {
                                                push_finished_compute_probe(
                                                    previous,
                                                    collector.ordinal,
                                                    resident.as_ref(),
                                                    &mut collector.compute_probes,
                                                );
                                            }
                                        }
                                        execute_scheduled_raw_triangle(
                                            collector,
                                            &candidate,
                                            index,
                                            resident,
                                            collector.physical,
                                            false,
                                            depth_accum.as_deref_mut(),
                                            command_coverage,
                                        )?
                                    }
                                }
                            }
                            TexrectTmemSource::Committed(state) => {
                                if collector.collect_compute_probe {
                                    if let Some(previous) = retain_compute_probe_draw(
                                        &mut compute_probe_builder,
                                        collector,
                                        &candidate,
                                        index,
                                        state,
                                        resident.as_ref(),
                                    )? {
                                        push_finished_compute_probe(
                                            previous,
                                            collector.ordinal,
                                            resident.as_ref(),
                                            &mut collector.compute_probes,
                                        );
                                    }
                                }
                                execute_scheduled_raw_triangle(
                                    collector,
                                    &candidate,
                                    index,
                                    resident,
                                    state,
                                    false,
                                    depth_accum.as_deref_mut(),
                                    command_coverage,
                                )?
                            }
                        }
                    }
                })
            },
        )?;
        if schedule_index + 1 == schedule.len() {
            flush_compute_probe(
                &mut compute_probe_builder,
                collector.ordinal,
                completed.device_bytes().device_bytes(),
                &mut collector.compute_probes,
            );
        }
        claimed = Some(union_target_rectangle(completed.rectangle(), claimed));
        declared.extend(accesses);
        // This command's owned output becomes the next command's resident
        // bytes. Intermediate completions have no consumer: only the last
        // completion can be admitted and published. Moving their existing
        // buffer therefore preserves the single owner instead of cloning a
        // complete target and immediately dropping the original. The last
        // command needs no next accumulator at all.
        //
        // Fresh Time Profiler attribution on the WM2000 rs+wgpu lane assigns
        // 1,646/27,437 exclusive samples to the former clone in this
        // function. `FN64_MOVE_COLOR_ACCUMULATOR=0` retains that exact clone
        // path as the same-binary measurement control.
        if move_accumulator {
            if schedule_index + 1 == schedule.len() {
                last_completed = Some(completed);
            } else if ordered_coverage.is_some() {
                let (bytes, coverage) = completed.into_task_accumulator();
                accumulated = Some(bytes);
                ordered_coverage = Some(coverage);
            } else {
                accumulated = Some(
                    completed
                        .into_device_color_bytes()
                        .into_device_bytes()
                        .into_vec(),
                );
            }
        } else {
            accumulated = Some(completed.device_bytes().device_bytes().to_vec());
            if ordered_coverage.is_some() {
                ordered_coverage = Some(completed.coverage().clone());
            }
            last_completed = Some(completed);
        }
    }
    task_cpu_phase_census::record_started(
        collector.task_cpu_phase_census.as_deref_mut(),
        task_cpu_phase_census::Phase::ScheduleDecodeRowPrepRaster,
        schedule_started,
    );

    raw_dpc_execute_census::timed(
        raw_dpc_execute_census::Phase::ColorFinalize,
        || -> Result<_, WgpuRawDpcExecutionError> {
            let completed = last_completed.expect("a non-empty schedule ran at least one command");
            // **Every digest, computed once, against the final buffer.** No write
            // in this list can describe an intermediate state, because none of them
            // existed until now -- `declared` carried only accesses through the
            // loop, and this is the single call that turns them into digests.
            //
            // `fill_completed_writes` is the existing per-access digest derivation,
            // reused rather than duplicated: what changed with N commands is *when*
            // it is called (once, at the end) and over *which* buffer (the composed
            // one), not how a digest is derived from an access.
            // The claimed rectangle is the union of every command's own, which is
            // what `admit_completed_initialization` reads to decide whether a
            // brand-new target is fully initialized. Reporting one command's
            // rectangle would understate what N proved.
            let completed = completed.with_claimed_rectangle(
                claimed.expect("a non-empty schedule claimed at least one rectangle"),
            );
            let initialized = candidate.admit_completed_initialization(completed)?;
            let (guest_writes, prepared_sparse_checkpoint) = if fused_sparse_checkpoint_enabled()
                && collector
                    .ordered_cpu_color_batch
                    .as_deref()
                    .is_some_and(|batch| batch.active.is_some())
            {
                let (checkpoint, writes) =
                    initialized.sparse_checkpoint_from_accesses(&declared)?;
                (writes, Some(checkpoint))
            } else {
                (
                    fill_completed_writes(key, initialized.device_bytes(), &declared)?,
                    None,
                )
            };
            Ok(Some(StagedFill {
                initialized,
                guest_writes,
                prepared_sparse_checkpoint,
                cpu_phase_attributed,
            }))
        },
    )
}

pub(super) fn ordered_depth_free_acff_triangle_member(collector: &ExecutionCollector<'_>) -> bool {
    task_cpu_phase_shape(
        collector.ordered_cpu_color_batch.is_some(),
        collector.plan.draw.color_image.is_some_and(|image| {
            image.format() == crate::ImageFormat::Rgba && image.size() == crate::PixelSize::Bits16
        }),
        collector.plan.fills.len(),
        collector.plan.texrect_commands.len(),
        collector.plan.raw_triangle_commands.len(),
        collector.defer_compute_replacement,
        collector.compute_replacement_enabled,
    ) && collector
        .plan
        .raw_triangle_commands
        .iter()
        .all(|scheduled| {
            collector.plan.triangles[scheduled.triangle_index]
                .draw
                .as_ref()
                .is_ok_and(|draw| {
                    task_cpu_phase_hot_program(
                        draw.combine_params,
                        draw.other_mode,
                        scheduled
                            .decoded
                            .as_ref()
                            .is_ok_and(|triangle| triangle.flags().shaded()),
                        scheduled
                            .decoded
                            .as_ref()
                            .is_ok_and(|triangle| triangle.flags().textured()),
                    )
                })
        })
}

pub(super) const fn task_cpu_phase_shape(
    ordered_batch: bool,
    rgba16_target: bool,
    fill_count: usize,
    texrect_count: usize,
    raw_triangle_count: usize,
    deferred_compute: bool,
    compute_replacement: bool,
) -> bool {
    ordered_batch
        && rgba16_target
        && fill_count == 0
        && texrect_count == 0
        && raw_triangle_count > 0
        && !deferred_compute
        && !compute_replacement
}

pub(super) const fn task_cpu_phase_hot_program(
    combine: CombineParams,
    other_mode: OtherMode,
    shaded: bool,
    textured: bool,
) -> bool {
    combine.low() == 0xfc15_fea3
        && combine.high() == 0xf00f_f23f
        && other_mode.high() == 0x0018_acff
        && other_mode.low() == 0x0f0a_7008
        && !other_mode.depth_compare_enabled()
        && !other_mode.depth_update_enabled()
        && shaded
        && textured
}

pub(super) fn move_color_accumulator_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(
        || match crate::diag_env::diag_env("FN64_MOVE_COLOR_ACCUMULATOR") {
            Some(value) if value == "0" => false,
            Some(value) if value == "1" => true,
            Some(value) => {
                panic!("FN64_MOVE_COLOR_ACCUMULATOR must be exactly 0 or 1, got {value:?}")
            }
            None => true,
        },
    )
}

pub(super) fn fused_sparse_checkpoint_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    // Was `env_default_one`, deleted with the rest of the ad-hoc env layer in
    // task 2.2b. Spelled out here rather than reintroducing a helper: this is
    // the ONLY remaining default-on diagnostic in this crate, and the arms
    // below are the same ones `env_default_one` had.
    *ENABLED.get_or_init(
        || match crate::diag_env::diag_env("FN64_FUSED_SPARSE_CHECKPOINT") {
            Some(value) if value == "0" => false,
            Some(value) if value == "1" => true,
            Some(value) => {
                panic!("FN64_FUSED_SPARSE_CHECKPOINT must be exactly 0 or 1, got {value:?}")
            }
            None => true,
        },
    )
}

pub(super) fn shared_copyback_payloads_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(
        || match crate::diag_env::diag_env("FN64_RENDER_COPYBACK_PAYLOAD_SHARE") {
            Some(value) if value == "0" => false,
            Some(value) if value == "1" => true,
            Some(value) => {
                panic!("FN64_RENDER_COPYBACK_PAYLOAD_SHARE must be exactly 0 or 1, got {value:?}")
            }
            None => true,
        },
    )
}

pub(super) fn compute_column_bounds_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(
        || match crate::diag_env::diag_env("FN64_COMPUTE_RASTER_COLUMN_BOUNDS") {
            Some(value) if value == "0" => false,
            Some(value) if value == "1" => true,
            Some(value) => {
                panic!("FN64_COMPUTE_RASTER_COLUMN_BOUNDS must be exactly 0 or 1, got {value:?}")
            }
            None => true,
        },
    )
}

pub(super) fn compute_raster_min_target_pixels() -> u32 {
    static MINIMUM: OnceLock<u32> = OnceLock::new();
    *MINIMUM.get_or_init(
        || match crate::diag_env::diag_env("FN64_COMPUTE_RASTER_MIN_TARGET_PIXELS") {
            Some(value) => value.parse::<u32>().unwrap_or_else(|error| {
                panic!(
                    "FN64_COMPUTE_RASTER_MIN_TARGET_PIXELS must be a decimal u32, got {value:?}: {error}"
                )
            }),
            None => 16_384,
        },
    )
}

pub(super) const fn compute_raster_replacement_admitted(target_pixels: u32, minimum: u32) -> bool {
    target_pixels >= minimum
}

pub(super) fn own_color_command_input_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(
        || match crate::diag_env::diag_env("FN64_OWN_COLOR_COMMAND_INPUT") {
            Some(value) if value == "0" => false,
            Some(value) if value == "1" => true,
            Some(value) => {
                panic!("FN64_OWN_COLOR_COMMAND_INPUT must be exactly 0 or 1, got {value:?}")
            }
            None => true,
        },
    )
}

pub(super) fn color_command_input<'a>(
    accumulated: &'a mut Option<Vec<u8>>,
    owned: bool,
    key: crate::targets::ColorTargetKey,
) -> Result<Cow<'a, [u8]>, crate::targets::TexrectExecutionError> {
    if owned {
        accumulated
            .take()
            .map(Cow::Owned)
            .ok_or(crate::targets::TexrectExecutionError::MissingResidentBytes { key })
    } else {
        accumulated
            .as_deref()
            .map(Cow::Borrowed)
            .ok_or(crate::targets::TexrectExecutionError::MissingResidentBytes { key })
    }
}

/// The TMEM prefix a command at `command_index` observes: the one taken
/// after the LAST load whose own stream position is strictly earlier.
///
/// Strictly earlier, never equal: a load and the texrect that samples it are
/// separate wire commands with separate indices, and an equal index would
/// mean one command was both, which the decoder cannot produce. `None` means
/// no load in this packet precedes the command, so it observes durable
/// committed TMEM instead.
///
/// `prefixes` is in stream order by construction (`stage_and_report` appends
/// one per load as it walks `plan.loads`, which the decoder filled in its
/// single stream walk), so the last qualifying entry is the latest one.
pub(super) fn prefix_before(
    prefixes: &[(u32, crate::tmem::TmemPrefixSnapshot)],
    command_index: u32,
) -> Option<&crate::tmem::TmemPrefixSnapshot> {
    prefixes
        .iter()
        .rev()
        .find(|(load_command, _)| *load_command < command_index)
        .map(|(_, prefix)| prefix)
}

/// The smallest rectangle containing both, or `covered` alone when there is
/// no prior claim.
pub(super) fn union_target_rectangle(
    covered: TargetRectangle,
    prior: Option<TargetRectangle>,
) -> TargetRectangle {
    let Some(prior) = prior else {
        return covered;
    };
    let x = covered.x().min(prior.x());
    let y = covered.y().min(prior.y());
    let right = (covered.x() + covered.width()).max(prior.x() + prior.width());
    let bottom = (covered.y() + covered.height()).max(prior.y() + prior.height());
    TargetRectangle::try_new(x, y, right - x, bottom - y)
        .expect("a union of two in-bounds rectangles is in bounds")
}

/// This packet's color-target key, derived from the `SetColorImage`
/// current at the packet's stream position -- `PlanCollector`'s tracked
/// `draw.color_image`, which is seeded from `WgpuBackend`'s durable
/// `rdp_state` and updated by any `SetColorImage` this packet carries.
///
/// **Not read off the first `FillRectangle`.** That was the previous
/// derivation and it was wrong in a way no fill-bearing packet could
/// expose: the RDP's color-image register is durable across submissions,
/// so a packet may compose into a target it never re-declares. The
/// decoder's own `raw_dpc::plan_texture_rectangle` already derives a
/// texrect's declared `ColorFramebuffer` write accesses from that same
/// durable `state.color_image()`, so reading a packet-local fill here made
/// the executor and the decoder answer one question two ways. Measured on
/// WM2000: a real packet of 14 texrects, 4 loads and zero fills, every
/// texrect declaring a four-access write run, aborted the run because
/// `fills.first()` was `None`.
///
/// The fill is retained as a **cross-check**, not a source: a fill whose
/// own `color_image` disagrees with the tracked register is a decoder /
/// executor divergence and is refused by name rather than silently
/// preferring either.
///
/// Builds the registry on the first admitted color-target command ever,
/// exactly as the fill path did, and for the same reason: neither
/// `try_new` nor `create` has a memory layout to build it from.
pub(super) fn color_target_key(
    collector: &mut ExecutionCollector<'_>,
    packet: &WorkloadPacket,
) -> Result<ColorTargetKey, WgpuRawDpcExecutionError> {
    let image = collector
        .plan
        .draw
        .color_image
        .ok_or(WgpuRawDpcExecutionError::NoStagedColorImage)?;
    if let Some((command_index, fill, ..)) = collector.plan.fills.first() {
        let declared = ColorImage::from_wire(
            image_format(fill.color_image.format),
            pixel_size(fill.color_image.size),
            fill.color_image.width,
            fill.color_image.address,
        );
        if declared != image {
            return Err(
                WgpuRawDpcExecutionError::FillColorImageDisagreesWithRegister {
                    command_index: *command_index,
                },
            );
        }
    }
    let Some(extent) = collector.configured_target_extent else {
        return Err(WgpuRawDpcExecutionError::NoColorTargetHeight);
    };
    let format = ColorTargetFormat::try_from_rdp(image.format(), image.size())?;
    let key = ColorTargetKey::try_new(
        image.address(),
        ColorTargetExtent::try_new(image.width(), extent.height)?,
        format,
    )?;
    if collector.color_targets.is_none() {
        *collector.color_targets = Some(ColorTargetRegistry::try_new(
            packet.memory_layout(),
            COLOR_TARGET_REGISTRY_CAPACITY,
        )?);
    }
    Ok(key)
}

/// Converts one captured guest-RDRAM range from the storage byte order the
/// capture delivers into the flat logical order [`crate::DeviceColorBytes`]
/// is expressed in.
///
/// **Not cosmetic, and not a guess.** `fn64-runtime`'s RDRAM stores guest
/// bytes in native words under a per-width XOR byte-lane mapping --
/// `write_u8` indexes `range(addr, 1, 3)`, i.e. `offset ^ 3` on a
/// little-endian host (`fn64-runtime/src/rdram.rs:623-627`), which is
/// exactly why `copy_committed_guest_writes` copies OUT through
/// `write_logical_bytes` rather than a raw `copy_from_slice`. The ABI's
/// guest-read capture slices the live allocation directly
/// (`fn64-abi/src/task_dispatch/rsp_commit.rs`), so what arrives here is
/// storage order, and reading it as logical bytes byte-swaps every pixel.
///
/// The conformance runner records the same trap from the other direction:
/// a raw slice copy there "reported every pixel as byte-swapped against the
/// reference backend -- a runner defect that would have been read as a
/// renderer defect".
///
/// This is the inverse of that copy: `logical[i] = storage[i ^ 3]`, applied
/// within each aligned 4-byte word so a range that is not word-aligned or
/// not a whole number of words still maps every byte it does carry.
pub(super) fn logical_bytes_from_captured_rdram(captured: &[u8]) -> Vec<u8> {
    captured
        .iter()
        .enumerate()
        .map(|(index, _)| {
            // The lane swap is defined within each aligned word; `^ 3` on
            // the index inside the word, not on the whole-buffer index,
            // so the tail of a partial word is still addressed correctly.
            let word = index & !3;
            let lane = (index & 3) ^ 3;
            captured[word + lane]
        })
        .collect()
}
