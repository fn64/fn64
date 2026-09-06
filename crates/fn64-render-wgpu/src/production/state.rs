use super::*;

/// The ten durable RDP draw registers the plan walk seeds from durable state
/// and then tracks, in stream order, as the packet's own state commands
/// arrive.
///
/// Before task 4.1 these ten fields existed in three places: as
/// `PlanCollector`'s ten `current_*` fields, mirrored field-for-field by
/// `RawDpcCarryIn`, and re-listed as nine positional parameters in a
/// test-only `seeded_from_parts` constructor. The three copies had to be
/// edited together for every register added, and the positional constructor
/// silently accepted a transposed pair of same-typed arguments. One struct
/// owning both the fields and their per-command update rule
/// ([`RdpDrawState::apply`]) makes each register appear once.
///
/// One value represents one stream instant; its fields cannot drift across
/// the packet boundary independently.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RdpDrawState {
    /// `SetOtherMode` current at this stream position.
    pub(super) other_mode: Option<OtherMode>,
    /// `SetCombine` current at this stream position.
    pub(super) combine: Option<CombineParams>,
    /// `G_SETBLENDCOLOR` current at this stream position.
    pub(super) blend_color: Color4,
    /// `G_SETENVCOLOR` current at this stream position.
    pub(super) env_color: Color4,
    /// `G_SETPRIMCOLOR` current at this stream position.
    pub(super) prim_color: PrimColor,
    /// `G_SETFOGCOLOR` current at this stream position. Needed by the
    /// production blend-cycle wiring's `Fog` selector.
    pub(super) fog_color: Color4,
    /// `G_SETSCISSOR` current at this stream position. Seeding matters more
    /// here than for the color registers: a display list commonly sets the
    /// scissor once per frame and then submits several packets, so a
    /// per-packet reset would unscissor every packet after the first.
    pub(super) scissor: Option<crate::targets::RdpScissorRect>,
    /// `G_SETPRIMDEPTH` current at this stream position. Read by the CPU
    /// raster path's z-compare under `G_ZS_PRIM`.
    pub(super) prim_depth: Option<crate::state::PrimDepth>,
    /// `G_SETCIMG` current at this stream position.
    ///
    /// **The durable seed is load-bearing, not defensive.** The decoder
    /// already derives a texrect's declared `ColorFramebuffer` write
    /// accesses from `state.color_image()` -- durable state -- in
    /// `raw_dpc::plan_texture_rectangle`. Deriving the executor's
    /// [`ColorTargetKey`] from a packet-local `FillRectangle` instead made
    /// those two derivations disagree for any packet whose texrects follow
    /// an earlier packet's `SetColorImage`: measured on WM2000, a real
    /// packet of 14 texrects, 4 TMEM loads and **zero** fills, whose
    /// texrects every one declared a real four-access write run.
    pub(super) color_image: Option<ColorImage>,
    /// Every tile's `SetTile`/`SetTileSize` current at this stream position,
    /// indexed by the RDP's own 0..=7 tile index.
    ///
    /// The RDP's eight tile descriptors are durable registers, so a packet
    /// that re-declares none still has them. Seeding `[(None, None); 8]`
    /// made every texrect in such a packet a `TexrectUnboundTile` refusal:
    /// measured on WM2000, the packet that follows the load-free texrect
    /// admission carries 46 texrects and an entirely empty tile table.
    ///
    /// The whole table, not tile 0 alone, because EVERY admitted draw
    /// names its own tile in its own wire word: a texture rectangle in
    /// word 1 bits 26:24, a raw triangle in word 0 bits 18:16. Tracking
    /// only tile 0 made every non-zero-tile texrect an `UnboundTile`
    /// refusal (WM2000's do not name tile 0), and made every non-zero-tile
    /// raw triangle silently bind tile 0's descriptor in the GPU uniform.
    pub(super) tiles: [(
        Option<fn64_render::NeutralTileDescriptor>,
        Option<fn64_render::NeutralTileSize>,
    ); 8],
}

impl RdpDrawState {
    /// Every durable RDP draw register as of `state`.
    pub(super) fn capture(state: &RdpState) -> Self {
        Self {
            other_mode: state.other_mode(),
            combine: state.combine(),
            blend_color: state.blend_color(),
            env_color: state.env_color(),
            prim_color: state.prim_color(),
            fog_color: state.fog_color(),
            scissor: state.scissor(),
            prim_depth: state.prim_depth(),
            color_image: state.color_image(),
            tiles: durable_neutral_tiles(state),
        }
    }

    /// Advances this running value by one state command, in plan order.
    ///
    /// The single implementation of the seed-then-track update rule, moved
    /// here verbatim from `PlanCollector`'s own `ExactRawDpcPlanVisitor`
    /// arms -- same order, same conditions. Commands this walk reads no
    /// field of leave the value untouched; they are still counted for
    /// `command_index` continuity by the caller.
    pub(super) fn apply(&mut self, state: &RdpStateCommand) {
        match state {
            RdpStateCommand::SetOtherMode { other_mode, .. } => {
                self.other_mode = Some(OtherMode::from_wire(other_mode.high, other_mode.low));
            }
            RdpStateCommand::SetCombine { combine, .. } => {
                self.combine = Some(CombineParams::from_wire(combine.low, combine.high));
            }
            RdpStateCommand::SetBlendColor { color, .. } => {
                self.blend_color = Color4::from_wire(color.value);
            }
            RdpStateCommand::SetEnvColor { color, .. } => {
                self.env_color = Color4::from_wire(color.value);
            }
            RdpStateCommand::SetPrimColor { color, .. } => {
                self.prim_color = PrimColor::from_wire(
                    u32::from(color.lod_frac) | (u32::from(color.lod_min) << 8),
                    color.color,
                );
            }
            RdpStateCommand::SetFogColor { color, .. } => {
                self.fog_color = Color4::from_wire(color.value);
            }
            RdpStateCommand::SetPrimDepth { depth, .. } => {
                // Reconstruct the wire form (`z` in bits 16:31, `dz` in
                // bits 0:15) so `PrimDepth::from_wire` re-applies the
                // 15-bit z / 16-bit dz masks the decoder used -- the same
                // recovery `TriangleDrawStateCollector` performs.
                self.prim_depth = Some(crate::state::PrimDepth::from_wire(
                    (u32::from(depth.z) << 16) | u32::from(depth.dz),
                ));
            }
            // Latched verbatim in wire quarter-pixels. Public libultra
            // `include/ultra64/gbi.h:4794-4837` encodes each coordinate
            // as a twelve-bit value scaled by four (or accepts the
            // fractional wire value directly).
            RdpStateCommand::SetScissor { scissor, .. } => {
                self.scissor = Some(crate::targets::RdpScissorRect::from_wire_quarter_pixels(
                    scissor.mode,
                    scissor.upper_left_x,
                    scissor.upper_left_y,
                    scissor.lower_right_x,
                    scissor.lower_right_y,
                ));
            }
            RdpStateCommand::SetColorImage { image, .. } => {
                self.color_image = Some(ColorImage::from_wire(
                    image_format(image.format),
                    pixel_size(image.size),
                    image.width,
                    image.address,
                ));
            }
            RdpStateCommand::SetTile {
                tile_index,
                descriptor,
                ..
            } => {
                if let Some(slot) = self.tiles.get_mut(usize::from(*tile_index)) {
                    slot.0 = Some(*descriptor);
                }
            }
            RdpStateCommand::SetTileSize {
                tile_index, size, ..
            } => {
                if let Some(slot) = self.tiles.get_mut(usize::from(*tile_index)) {
                    slot.1 = Some(*size);
                }
            }
            _ => {}
        }
    }
}

impl Default for RdpDrawState {
    fn default() -> Self {
        Self::capture(&RdpState::default())
    }
}

/// Every durable RDP register the execution walk may read before this
/// submission issues its first state command. One value represents one stream
/// instant; its fields cannot drift across the packet boundary independently.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(super) struct RawDpcCarryIn {
    pub(super) draw: RdpDrawState,
}

impl RawDpcCarryIn {
    pub(super) fn capture(state: &RdpState) -> Self {
        Self {
            draw: RdpDrawState::capture(state),
        }
    }
}

/// Planning can cheaply rule out members whose command shape can never form
/// a compute segment. `ComputeCandidate` is deliberately not an execution
/// capability: exact program/TMEM/tile admission happens against captured
/// execution state and yields a separate move-only `ComputeEligibleTaskMember`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum PlannedTaskCpuReason {
    NoRawTriangle(PlannedNoRawTriangleReason),
    MixedFillOrTexrect,
    DefinitelyCpu(TaskComputeAdmissionRefusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum PlannedNoRawTriangleReason {
    FillOnly,
    TexrectOnly,
    FillAndTexrect,
    TmemLoadOnly,
    FillAndTmemLoad,
    TexrectAndTmemLoad,
    FillTexrectAndTmemLoad,
    SyncStateOnly,
    NoOpOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ComputeProgramAttribution {
    Program(u32),
    MixedPrograms,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlannedTaskExecution {
    Cpu(PlannedTaskCpuReason),
    ComputeCandidate,
}

pub(super) struct PlannedRawDpcTaskMember {
    pub(super) carry_in: RawDpcCarryIn,
    pub(super) execution: PlannedTaskExecution,
}

/// One pending value binds every member's carry-in state to its planning
/// disposition. It is installed only after the whole batch plans, so a
/// mid-batch failure cannot advance durable RDP state or leave parallel queue
/// prefixes for a later execution call to mis-pair.
pub(super) struct PlannedRawDpcTaskBatch {
    pub(super) members: VecDeque<PlannedRawDpcTaskMember>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PresentedFieldDelivery {
    ConcreteDiagnostic,
    Source,
    PostVi,
}

/// The pure-Rust wgpu production raw-DPC backend. Owns its coordinator
/// outright -- there is exactly one route to one, at construction, per
/// `RawDpcBackendAuthority::into_coordinator`'s own doc comment.
pub struct WgpuBackend {
    pub(super) coordinator: RawDpcCoordinator<PhysicalTmemState>,
    /// Durable logical RDP state (`SetTile`/`SetTextureImage`/`SyncLoad`
    /// fields `RdpState` tracks) carried across submissions. `decode_raw_dpc`
    /// always forks a throwaway copy to decode against
    /// (`RdpState::fork_for_decode`) and never mutates its input, so holding
    /// this durably and applying each successful real decode's
    /// `RdpStateDelta` back onto it (`RdpState::apply`) is what gives
    /// `plan_raw_dpc` continuity across submissions instead of silently
    /// re-decoding every command stream from a fresh default state, which
    /// would be wrong for any submission whose commands depend on state a
    /// prior submission set (e.g. a `SetTile` from submission N read by a
    /// `LoadBlock` in submission N+1).
    pub(super) rdp_state: RdpState,
    /// The matching successful plan's complete pre-delta register state.
    /// Captured immediately before `RdpState::apply`; execution consumes this
    /// value as a unit, so no register can be seeded from the packet's final
    /// state while a sibling is seeded from its carry-in state.
    pub(super) raw_dpc_carry_in_before_last_plan: Option<RawDpcCarryIn>,
    /// The one move-only task value retained by the explicitly batched plan
    /// seam. Ordinary planning never writes it.
    pub(super) pending_raw_dpc_task_batch: Option<PlannedRawDpcTaskBatch>,
    /// `Some` only after a successful `RenderBackend::create`; `try_new`
    /// never populates it. Always `Some` together with
    /// `triangle_target_extent`, never one without the other.
    pub(super) triangle_pipeline: Option<Box<TrianglePipelineRenderer>>,
    /// The render-target extent for triangle draws, sized from `create`'s
    /// own `RenderConfig`. Always `Some` together with `triangle_pipeline`,
    /// never one without the other; replaced atomically with it on every
    /// `create()` call.
    pub(super) triangle_target_extent: Option<TriangleTargetExtent>,
    /// The most recent successful triangle draw's GPU-observed output.
    /// Replaced only when every triangle in a draw call succeeds; a
    /// failed draw leaves the prior value untouched. Never an accumulated
    /// history, never a persistent framebuffer.
    pub(super) triangle_draw_output: Option<TriangleDrawOutput>,
    /// Every launch-time probe/diagnostic boolean this backend holds,
    /// resolved ONCE at construction from the host's [`crate::WgpuKnobs`].
    ///
    /// Before task 2.2b these were seven loose `bool` fields, each read
    /// straight from the environment inside `try_new`. Collecting them lets
    /// a caller (a test, or `fn64-shell`) state the policy as a value
    /// instead of mutating the process environment, and puts every default
    /// in one documented place.
    pub(super) probes: ProbePolicy,
    /// Active only around an explicitly bounded offline replay window. The
    /// window retains each packet's typed compute fixtures until one final
    /// submit can prove exact intermediate target checkpoints.
    pub(super) compute_raster_checkpoint_probe: Option<ComputeRasterCheckpointProbe>,
    /// Most recent successful probe execution, consumed by the offline
    /// replay's phase accounting. An ineligible packet leaves this `None`.
    pub(super) compute_raster_probe_receipt: Option<ComputeRasterProbeReceipt>,
    /// Most recent packet whose guest-visible target was produced by the
    /// replacement chain rather than the CPU rasterizer.
    pub(super) compute_raster_replace_receipt: Option<ComputeRasterProbeReceipt>,
    pub(super) task_cpu_phase_census: Option<task_cpu_phase_census::Task>,
    pub(super) last_task_batch_execution_mechanism:
        Option<fn64_render::RawDpcTaskBatchExecutionMechanism>,
    pub(super) last_published_visual_target: Option<(
        fn64_render_ir::SubmissionIdentity,
        PublishedVisualTargetMarker,
    )>,
    /// The host-configured framebuffer extent from the most recent
    /// `RenderBackend::create` call, recorded *before* the GPU device
    /// request rather than inside its success branch.
    ///
    /// This is the only color-image height source this backend has: the
    /// RDP's `SetColorImage` carries `format`/`size`/`width`/`address` and
    /// **no height** field (`crate::ColorImage`), so an admitted
    /// `FillRectangle`'s `ColorTargetKey` must take its height from here.
    /// Deliberately separate from `triangle_target_extent` (which is only
    /// populated on GPU-device success): a `FillRectangle` is executed
    /// entirely CPU-side and has no adapter dependency, so gating fill
    /// admission on a real GPU would make an adapterless host silently
    /// unable to execute a command it is fully capable of executing.
    ///
    /// Honest nonclaim: this is a *host-configured* height, not a
    /// wire-decoded one. The RDP never states a color image's height, so a
    /// stream setting an offscreen color image of a different height would
    /// derive a `ColorTargetKey` whose range is wrong. That mismatch
    /// surfaces loudly (`RectangleOutOfBounds` from
    /// `CandidateColorTarget::plan_rows`, or `AliasedResidentTarget`), never
    /// as a silently mis-sized publish.
    pub(super) configured_target_extent: Option<TriangleTargetExtent>,
    /// `None` until the first admitted `FillRectangle` reaches
    /// `execute_raw_dpc_inner`. Built there, from that capture's own
    /// `PhysicalMemoryLayout` -- neither `try_new` nor `create` has a layout
    /// to build it from (`RenderConfig` carries a pixel extent, not an RDRAM
    /// byte size), and inventing one would be a fabricated fact. A later
    /// capture whose layout differs is rejected loudly by
    /// `ColorTargetRegistry::begin_candidate`'s existing
    /// `MemoryLayoutMismatch` check, never by silently rebuilding the
    /// registry and dropping every resident generation.
    pub(super) color_targets: Option<ColorTargetRegistry>,
    /// Set by `execute_raw_dpc_inner` when a fill staged an
    /// `InitializedCandidateColorTarget`; redeemed by `publish_raw_dpc`.
    /// See [`PendingFillPublication`].
    pub(super) pending_fill_publication: Option<PendingFillPublication>,
    /// Ordered color successors produced by one task batch. Each token is
    /// redeemed only by its own later per-submission publication.
    pub(super) task_batch_pending_fill_publications: VecDeque<PendingFillPublication>,
    /// The most recent successfully presented VI field, and nothing else.
    ///
    /// `None` until the first `present` succeeds. A `present` that returns a
    /// named refusal or a typed bounds/alignment error leaves the previous
    /// field in place rather than clearing it: the retrace that failed
    /// produced no image, and discarding the last good one would fabricate a
    /// black frame the VI never scanned out. A *successful* present always
    /// replaces it, so this is never an accumulated history.
    pub(super) presented_field: Option<crate::PresentedField>,
    /// Selects one explicit stage owner. The source and post-VI receipts are
    /// different types and cannot both claim one presentation boundary.
    pub(super) presented_field_delivery: PresentedFieldDelivery,
    pub(super) presented_source_field: Option<fn64_render::PresentedSourceField>,
    pub(super) presented_post_vi_field: Option<fn64_render::PresentedPostViField>,
}

/// One completed game-derived hottest-state compute differential. The time
/// includes the prototype's uploads, dispatch, waits, and two readbacks; it
/// intentionally does not pretend those costs are shader-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComputeRasterProbeReceipt {
    pub(super) submission_count: u32,
    pub(super) batch_count: u32,
    pub(super) draw_count: u32,
    pub(super) target_pixels: u32,
    pub(super) admission_elapsed: Duration,
    pub(super) elapsed: Duration,
    pub(super) effects_elapsed: Duration,
}

impl ComputeRasterProbeReceipt {
    pub const fn submission_count(self) -> u32 {
        self.submission_count
    }

    pub const fn batch_count(self) -> u32 {
        self.batch_count
    }

    pub const fn draw_count(self) -> u32 {
        self.draw_count
    }

    pub const fn target_pixels(self) -> u32 {
        self.target_pixels
    }

    pub const fn elapsed(self) -> Duration {
        self.elapsed
    }

    pub const fn admission_elapsed(self) -> Duration {
        self.admission_elapsed
    }

    pub const fn effects_elapsed(self) -> Duration {
        self.effects_elapsed
    }
}

pub(super) struct ComputeRasterProbe {
    pub(super) ordinal: u64,
    pub(super) batch: ComputeRasterBatch,
    pub(super) extent: TriangleTargetExtent,
    pub(super) resident_bytes: Vec<u8>,
    pub(super) triangles: Box<[ComputeCoverageTriangle]>,
    pub(super) tmem: TmemGpuProjection,
    pub(super) tile: TileBindingParams,
    pub(super) expected_bytes: Vec<u8>,
}

/// Replay-only proof vehicle for the task transport: every packet must add
/// at least one complete probe, and every probe must begin with the exact
/// bytes produced by its predecessor. Those constraints make the retained
/// checkpoint limits real packet boundaries rather than a synthetic stream.
pub(super) struct ComputeRasterCheckpointProbe {
    pub(super) probes: Vec<ComputeRasterProbe>,
    pub(super) checkpoint_limits: Vec<usize>,
    pub(super) packet_count: usize,
    pub(super) restore_probe_enabled: bool,
}

impl ComputeRasterCheckpointProbe {
    pub(super) fn new(restore_probe_enabled: bool) -> Self {
        Self {
            probes: Vec::new(),
            checkpoint_limits: Vec::new(),
            packet_count: 0,
            restore_probe_enabled,
        }
    }

    pub(super) fn push_packet(
        &mut self,
        packet: Vec<ComputeRasterProbe>,
        has_target_write: bool,
    ) -> Result<(), WgpuRawDpcExecutionError> {
        if packet.is_empty() {
            if has_target_write {
                return Err(
                    WgpuRawDpcExecutionError::ComputeRasterCheckpointPacketIneligible {
                        packet: self.packet_count,
                    },
                );
            }
            self.packet_count += 1;
            return Ok(());
        }
        for (index, probe) in packet.iter().enumerate() {
            let previous = if index == 0 {
                self.probes.last()
            } else {
                Some(&packet[index - 1])
            };
            if let Some(previous) = previous {
                if probe.extent != previous.extent
                    || probe.batch.target() != previous.batch.target()
                    || probe.resident_bytes != previous.expected_bytes
                {
                    return Err(
                        WgpuRawDpcExecutionError::ComputeRasterCheckpointDiscontinuity {
                            previous_ordinal: previous.ordinal,
                            ordinal: probe.ordinal,
                        },
                    );
                }
            }
        }
        self.probes.extend(packet);
        self.checkpoint_limits.push(self.probes.len());
        self.packet_count += 1;
        Ok(())
    }
}

pub(super) struct ComputeRasterDispatch {
    pub(super) batch: ComputeRasterBatch,
    pub(super) extent: TriangleTargetExtent,
    pub(super) triangles: Box<[ComputeCoverageTriangle]>,
    pub(super) tmem: TmemGpuProjection,
    pub(super) tile: TileBindingParams,
}

pub(super) struct ComputeRasterProbeBuilder {
    pub(super) batch: ComputeRasterBatchBuilder,
    pub(super) extent: TriangleTargetExtent,
    pub(super) resident_bytes: Vec<u8>,
    pub(super) triangles: Vec<ComputeCoverageTriangle>,
    pub(super) shared_tmem_identity: Option<crate::TmemSnapshotIdentity>,
    pub(super) shared_tmem: Option<TmemGpuProjection>,
    pub(super) shared_tile: Option<TileBindingParams>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComputeRasterProbePush {
    Admitted,
    SplitDispatch,
    Refused(ComputeRasterAdmissionRefusal),
}

impl ComputeRasterProbeBuilder {
    pub(super) fn new(candidate: &CandidateColorTarget, resident_bytes: Vec<u8>) -> Self {
        let key = candidate.key();
        Self {
            batch: ComputeRasterBatchBuilder::new(key, candidate.generation()),
            extent: TriangleTargetExtent {
                width: key.extent().width(),
                height: key.extent().height(),
            },
            resident_bytes,
            triangles: Vec::new(),
            shared_tmem_identity: None,
            shared_tmem: None,
            shared_tile: None,
        }
    }

    pub(super) fn push<S: crate::TmemByteSource + ?Sized>(
        &mut self,
        collector: &ExecutionCollector<'_>,
        candidate: &CandidateColorTarget,
        index: CommandIndex,
        tmem: &S,
    ) -> Result<ComputeRasterProbePush, WgpuRawDpcExecutionError> {
        let scheduled = &collector.plan.raw_triangle_commands[index];
        let draw = collector.plan.triangles[scheduled.triangle_index]
            .draw
            .as_ref()
            .map_err(|missing| WgpuRawDpcExecutionError::MissingTriangleDrawState(*missing))?;
        let triangle = decode_scheduled_raw_triangle(collector, index)?;
        let program = match ComputeRasterProgramKey::try_admit(
            candidate.key(),
            draw.combine_params,
            draw.other_mode,
            triangle.flags().textured(),
        ) {
            Ok(program) => program,
            Err(reason) => return Ok(ComputeRasterProbePush::Refused(reason)),
        };
        let accesses = scheduled_raw_triangle_accesses(collector, candidate, index)?;
        let identity = crate::TmemByteSource::snapshot(tmem);
        let projection = crate::project_tmem(tmem);
        let tile = draw
            .tile_binding
            .with_lut_mode(draw.other_mode.texture_lut_mode());
        if self
            .shared_tmem_identity
            .is_some_and(|shared| shared != identity)
            || self.shared_tmem.is_some_and(|shared| shared != projection)
            || self.shared_tile.is_some_and(|shared| shared != tile)
        {
            return Ok(ComputeRasterProbePush::SplitDispatch);
        }
        let admission = match ComputeRasterDrawAdmission::try_new(
            candidate.key(),
            scheduled.command_index,
            scheduled.triangle_index.get(),
            program,
            identity,
            accesses,
        ) {
            Ok(admission) => admission,
            Err(reason) => return Ok(ComputeRasterProbePush::Refused(reason)),
        };
        if let Err(reason) = self.batch.push(admission) {
            return Ok(ComputeRasterProbePush::Refused(reason));
        }
        self.shared_tmem_identity = Some(identity);
        self.shared_tmem = Some(projection);
        self.shared_tile = Some(tile);
        self.triangles.push(
            ComputeCoverageTriangle::from_raw(triangle)
                .with_material(draw.env_color, draw.prim_color)
                .with_program(program.shader_id()),
        );
        Ok(ComputeRasterProbePush::Admitted)
    }

    pub(super) fn finish_dispatch(self) -> Option<(ComputeRasterDispatch, Vec<u8>)> {
        Some((
            ComputeRasterDispatch {
                batch: self.batch.finish().ok()?,
                extent: self.extent,
                triangles: self.triangles.into_boxed_slice(),
                tmem: self.shared_tmem?,
                tile: self.shared_tile?,
            },
            self.resident_bytes,
        ))
    }

    pub(super) fn finish(
        self,
        ordinal: u64,
        expected_bytes: Vec<u8>,
    ) -> Option<ComputeRasterProbe> {
        let (dispatch, resident_bytes) = self.finish_dispatch()?;
        Some(ComputeRasterProbe {
            ordinal,
            batch: dispatch.batch,
            extent: dispatch.extent,
            resident_bytes,
            triangles: dispatch.triangles,
            tmem: dispatch.tmem,
            tile: dispatch.tile,
            expected_bytes,
        })
    }
}

pub(super) fn retain_compute_probe_draw<S: crate::TmemByteSource + ?Sized>(
    builder: &mut Option<ComputeRasterProbeBuilder>,
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: CommandIndex,
    tmem: &S,
    resident_before: &[u8],
) -> Result<Option<ComputeRasterProbeBuilder>, WgpuRawDpcExecutionError> {
    if let Some(active) = builder.as_mut() {
        if active.push(collector, candidate, index, tmem)? == ComputeRasterProbePush::Admitted {
            return Ok(None);
        }
    }
    let previous = builder.take();
    let mut next = ComputeRasterProbeBuilder::new(candidate, resident_before.to_vec());
    if next.push(collector, candidate, index, tmem)? == ComputeRasterProbePush::Admitted {
        *builder = Some(next);
    }
    Ok(previous)
}

pub(super) fn flush_compute_probe(
    builder: &mut Option<ComputeRasterProbeBuilder>,
    ordinal: u64,
    expected_bytes: &[u8],
    probes: &mut Vec<ComputeRasterProbe>,
) {
    let Some(builder) = builder.take() else {
        return;
    };
    push_finished_compute_probe(builder, ordinal, expected_bytes, probes);
}

pub(super) fn push_finished_compute_probe(
    builder: ComputeRasterProbeBuilder,
    ordinal: u64,
    expected_bytes: &[u8],
    probes: &mut Vec<ComputeRasterProbe>,
) {
    if let Some(probe) = builder.finish(ordinal, expected_bytes.to_vec()) {
        probes.push(probe);
    }
}

pub(super) fn validate_compute_probe_output(
    first_probe: &ComputeRasterProbe,
    last_probe: &ComputeRasterProbe,
    actual_bytes: &[u8],
) -> Result<(), WgpuRawDpcExecutionError> {
    if actual_bytes.len() != last_probe.expected_bytes.len() {
        return Err(WgpuRawDpcExecutionError::ComputeRasterProbeLength {
            expected: last_probe.expected_bytes.len(),
            actual: actual_bytes.len(),
        });
    }
    let Some(byte) = actual_bytes
        .iter()
        .zip(&last_probe.expected_bytes)
        .position(|(actual, expected)| actual != expected)
    else {
        return Ok(());
    };
    let pixel = byte / 2;
    let pair = pixel * 2;
    let expected = u16::from_be_bytes([
        last_probe.expected_bytes[pair],
        last_probe.expected_bytes[pair + 1],
    ]);
    let actual = u16::from_be_bytes([actual_bytes[pair], actual_bytes[pair + 1]]);
    let first_draw = first_probe
        .batch
        .draws()
        .first()
        .expect("a sealed compute batch contains at least one draw");
    let last_draw = last_probe
        .batch
        .draws()
        .last()
        .expect("a sealed compute batch contains at least one draw");
    Err(WgpuRawDpcExecutionError::ComputeRasterProbeMismatch {
        ordinal: first_probe.ordinal,
        first_program: first_draw.program().words(),
        last_program: last_draw.program().words(),
        first_command_index: first_draw.command_index(),
        last_command_index: last_draw.command_index(),
        first_triangle_index: first_draw.triangle_index(),
        last_triangle_index: last_draw.triangle_index(),
        x: pixel as u32 % last_probe.extent.width,
        y: pixel as u32 / last_probe.extent.width,
        expected,
        actual,
    })
}

/// Capacity rationale: 4 is the fixed bounded ceiling for concurrently
/// resident color targets in this slice (a color image, a Z-adjacent second
/// target, and two generations of churn headroom). It is a scope bound, not
/// a measured hardware limit -- `TargetError::RegistryFull` is the loud
/// rejection if a real stream exceeds it, never eviction.
pub(super) const COLOR_TARGET_REGISTRY_CAPACITY: usize = 4;

/// One admitted color-target write, staged and validated during
/// `execute_raw_dpc` but deliberately **not yet published** into the
/// registry. Keyed by the submission it belongs to so `publish_raw_dpc` can
/// prove the capsule it is about to publish is the same submission that
/// staged this write -- never "whatever fill was staged last".
///
/// Why a deferred token rather than a held `ResidentPublication`: that type
/// exclusively borrows the registry, and the guest-commit call it would have
/// to be held across happens in `fn64-abi`, which reaches this backend only
/// through a `RefCell<Option<Box<dyn RenderBackend>>>` in a `with` block
/// that has already returned by then. A borrow cannot survive that; a token
/// can.
///
/// The staged guest writes live here alongside the
/// `InitializedCandidateColorTarget` so the token carries both halves of the
/// same staged fact and they cannot drift.
pub(super) struct PendingFillPublication {
    pub(super) submission: fn64_render_ir::SubmissionIdentity,
    pub(super) color: PendingColorPublication,
    /// Sparse publication already sealed from the same final accumulator
    /// that produced `guest_writes`. Present only while an ordered CPU member
    /// is waiting for [`OrderedCpuColorBatch::finish_member`] to retain the
    /// full accumulator as its successor input.
    pub(super) prepared_sparse_checkpoint: Option<SparseInitializedColorCheckpoint>,
    /// The exact N `CompletedWrite`s this fill contributed to the
    /// submission's `BackendEffectReport`, in journal order.
    pub(super) guest_writes: Vec<CompletedWrite>,
    pub(super) cpu_phase_attributed: bool,
    pub(super) exact_physical_coverage: bool,
}

#[derive(Clone, Copy)]
pub(super) enum PublishedVisualTargetMarker {
    Exact(ColorTargetKey),
    NoColorTarget,
    ComputeCoverageUnavailable,
}

pub(super) enum PendingColorPublication {
    Full(InitializedCandidateColorTarget),
    Sparse(SparseInitializedColorCheckpoint),
}

impl PendingColorPublication {
    pub(super) fn full(&self) -> &InitializedCandidateColorTarget {
        match self {
            Self::Full(initialized) => initialized,
            Self::Sparse(_) => panic!("a sparse CPU checkpoint cannot enter a compute segment"),
        }
    }
}

/// One move-only full target threaded across compatible adjacent CPU task
/// members. Each completed member yields a separate sparse publication
/// capability; this value retains only the image the next raster consumes.
pub(super) struct OrderedCpuColorBatch {
    pub(super) generations: ColorTargetExecutionBatch,
    pub(super) tail: Option<InitializedCandidateColorTarget>,
    pub(super) continuity: Option<OrderedCpuColorContinuity>,
    pub(super) active: Option<OrderedCpuCandidateReservation>,
}

impl OrderedCpuColorBatch {
    pub(super) fn new() -> Self {
        Self {
            generations: ColorTargetExecutionBatch::new(),
            tail: None,
            continuity: None,
            active: None,
        }
    }

    pub(super) fn flush(&mut self, registry: &mut ColorTargetRegistry) -> Result<(), TargetError> {
        assert!(
            self.active.is_none(),
            "an ordered CPU color batch cannot flush an unfinished member"
        );
        if let Some(tail) = self.tail.take() {
            let segment = self
                .continuity
                .take()
                .expect("an ordered CPU tail has continuity authority")
                .finish(tail)?;
            registry.commit_owned_task_shadow_segment(segment)?;
        }
        self.generations = ColorTargetExecutionBatch::new();
        self.continuity = None;
        Ok(())
    }

    pub(super) fn begin_member(
        &mut self,
        registry: &mut ColorTargetRegistry,
        key: ColorTargetKey,
    ) -> Result<(CandidateColorTarget, Option<(Vec<u8>, ColorCoverageState)>), TargetError> {
        assert!(
            self.active.is_none(),
            "an ordered CPU color candidate must complete before its successor begins"
        );
        if self.tail.as_ref().is_some_and(|tail| tail.key() != key) {
            self.flush(registry)?;
        }
        let (candidate, input) = self.generations.begin_candidate(registry, key)?;
        let reservation = OrderedCpuCandidateReservation::new(&candidate);
        let accumulator = match input {
            TaskColorInput::PriorTaskCheckpoint => {
                let tail = self
                    .tail
                    .take()
                    .expect("a prior task checkpoint owns the CPU accumulator");
                assert_eq!(tail.key(), key);
                assert_eq!(Some(tail.generation()), candidate.predecessor());
                Some(tail.into_task_accumulator())
            }
            TaskColorInput::DurableRegistry if candidate.predecessor().is_none() => Some((
                vec![0; key.range().len() as usize],
                ColorCoverageState::unknown(key.extent()),
            )),
            TaskColorInput::DurableRegistry => None,
        };
        self.active = Some(reservation);
        Ok((candidate, accumulator))
    }

    pub(super) fn finish_member(
        &mut self,
        mut pending: PendingFillPublication,
    ) -> Result<PendingFillPublication, TargetError> {
        let Some(reservation) = self.active.take() else {
            assert!(
                pending.prepared_sparse_checkpoint.is_none(),
                "a prepared sparse checkpoint requires an active ordered CPU reservation"
            );
            return Ok(pending);
        };
        let prepared_sparse_checkpoint = pending.prepared_sparse_checkpoint.take();
        let PendingColorPublication::Full(initialized) = pending.color else {
            panic!("an ordered CPU member must complete with a full accumulator")
        };
        let checkpoint = match prepared_sparse_checkpoint {
            Some(checkpoint) => checkpoint,
            None => initialized.sparse_checkpoint(&pending.guest_writes)?,
        };
        self.continuity = Some(match self.continuity.take() {
            Some(continuity) => continuity.append(reservation, &initialized)?,
            None => OrderedCpuColorContinuity::start(reservation, &initialized)?,
        });
        self.tail = Some(initialized);
        pending.color = PendingColorPublication::Sparse(checkpoint);
        Ok(pending)
    }
}

/// Failure constructing a fresh [`WgpuBackend`]. Both sources are the T0
/// sealed session split's own fallible constructor
/// (`fn64_render::new_raw_dpc_roles`) and this crate's physical TMEM state
/// identity authority; neither is expected to fail in ordinary operation.
#[derive(Debug)]
pub enum WgpuBackendConstructionError {
    RawDpcRoles(ValidationError),
    PhysicalTmemState(PhysicalTmemError),
}

impl core::fmt::Display for WgpuBackendConstructionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RawDpcRoles(error) => write!(formatter, "raw-DPC role split failed: {error}"),
            Self::PhysicalTmemState(error) => {
                write!(
                    formatter,
                    "physical TMEM state construction failed: {error}"
                )
            }
        }
    }
}

impl std::error::Error for WgpuBackendConstructionError {}

impl WgpuBackend {
    /// Construct a fresh backend and its paired ABI session together. The
    /// session is the caller's (ABI-side, per T0's role split): this
    /// backend keeps only the paired `RawDpcBackendAuthority`, consumed
    /// immediately into its owned coordinator, exactly as
    /// `RawDpcBackendAuthority::into_coordinator`'s doc comment describes
    /// ("a backend obtains one exactly once, at construction").
    pub fn try_new() -> Result<(Self, RawDpcAbiSession), WgpuBackendConstructionError> {
        Self::try_new_with_knobs(&crate::WgpuKnobs::default())
    }

    /// Construct a backend whose probe/diagnostic policy is the host's,
    /// rather than the documented defaults `try_new` uses.
    ///
    /// This is the seam `fn64-shell` uses: it resolves one process-wide
    /// `Knobs` (flag > `fn64.toml` > `FN64_*` compat > default) and hands the
    /// wgpu slice of it here. Before task 2.2b this crate read those seven
    /// variables itself, at construction, with no way for a caller to state
    /// the policy except by mutating the process environment -- which no test
    /// could do safely in a shared-process test binary.
    pub fn try_new_with_knobs(
        knobs: &crate::WgpuKnobs,
    ) -> Result<(Self, RawDpcAbiSession), WgpuBackendConstructionError> {
        let (session, authority) =
            fn64_render::new_raw_dpc_roles().map_err(WgpuBackendConstructionError::RawDpcRoles)?;
        let initial = PhysicalTmemState::try_new()
            .map_err(WgpuBackendConstructionError::PhysicalTmemState)?;
        Ok((
            Self {
                coordinator: authority.into_coordinator(initial),
                rdp_state: RdpState::default(),
                raw_dpc_carry_in_before_last_plan: None,
                pending_raw_dpc_task_batch: None,
                triangle_pipeline: None,
                triangle_target_extent: None,
                triangle_draw_output: None,
                probes: ProbePolicy::from_knobs(knobs),
                compute_raster_checkpoint_probe: None,
                compute_raster_probe_receipt: None,
                compute_raster_replace_receipt: None,
                task_cpu_phase_census: None,
                last_task_batch_execution_mechanism: None,
                last_published_visual_target: None,
                configured_target_extent: None,
                color_targets: None,
                pending_fill_publication: None,
                task_batch_pending_fill_publications: VecDeque::new(),
                presented_field: None,
                presented_field_delivery: PresentedFieldDelivery::ConcreteDiagnostic,
                presented_source_field: None,
                presented_post_vi_field: None,
            },
            session,
        ))
    }

    /// The most recently presented VI field, or `None` before the first
    /// successful `present`.
    ///
    /// Exposed for diagnostics and tests, mirroring `physical_tmem()` and
    /// `color_targets()`'s convention on this struct. This is the retrieval
    /// half of `RenderBackend::present`'s own contract for a headless
    /// backend ("finalize it as retrievable").
    pub fn presented_field(&self) -> Option<&crate::PresentedField> {
        self.presented_field.as_ref()
    }

    /// Select move-only shell-compatible source-field delivery. Direct
    /// backend users retain the filtered `presented_field` behavior unless
    /// they explicitly commit to consuming this source receipt.
    pub fn enable_presented_source_field_delivery(&mut self) {
        self.presented_field_delivery = PresentedFieldDelivery::Source;
        self.presented_source_field = None;
        self.presented_post_vi_field = None;
        self.presented_field = None;
    }

    /// Consume the most recent game-derived compute-raster probe timing.
    /// `None` means either probing is disabled or the last packet was not
    /// one closed hottest-state batch.
    pub fn take_compute_raster_probe_receipt(&mut self) -> Option<ComputeRasterProbeReceipt> {
        self.compute_raster_probe_receipt.take()
    }

    /// Diagnostic replay control. It changes only whether an additional
    /// CPU/compute differential is collected; production pixels and the CPU
    /// execution path are unchanged.
    pub fn set_compute_raster_probe_enabled(&mut self, enabled: bool) {
        self.probes.compute_raster_probe_enabled = enabled;
        self.compute_raster_probe_receipt = None;
    }

    /// Selects ordered on-device chaining for the diagnostic compute probe.
    /// Enabling this does not enable probing by itself.
    pub fn set_compute_raster_chain_probe_enabled(&mut self, enabled: bool) {
        self.probes.compute_raster_chain_probe_enabled = enabled;
        self.compute_raster_probe_receipt = None;
    }

    /// Begin an offline task-window checkpoint differential. Normal CPU
    /// execution, guest commits, and publication remain packet-local; only
    /// the additional compute fixtures are retained until `finish`.
    pub fn begin_compute_raster_checkpoint_probe(&mut self) {
        assert!(
            self.compute_raster_checkpoint_probe.is_none(),
            "compute-raster checkpoint probe already active"
        );
        let restore_probe_enabled = self.probes.compute_raster_probe_enabled;
        self.probes.compute_raster_probe_enabled = true;
        self.compute_raster_probe_receipt = None;
        self.compute_raster_checkpoint_probe =
            Some(ComputeRasterCheckpointProbe::new(restore_probe_enabled));
    }

    /// Submit the retained task window once and compare the GPU target at
    /// every original packet boundary with that packet's CPU-produced bytes.
    pub fn finish_compute_raster_checkpoint_probe(
        &mut self,
    ) -> Result<ComputeRasterProbeReceipt, RenderError> {
        let retained = self
            .compute_raster_checkpoint_probe
            .take()
            .expect("compute-raster checkpoint probe is not active");
        self.probes.compute_raster_probe_enabled = retained.restore_probe_enabled;
        let first = retained
            .probes
            .first()
            .expect("an active checkpoint probe accepted at least one packet");
        let dispatches: Vec<_> = retained
            .probes
            .iter()
            .map(|probe| ComputeHotColorDispatch {
                triangles: &probe.triangles,
                tmem: &probe.tmem,
                tile: probe.tile,
                first_row: 0,
                row_count: probe.extent.height,
                first_column: 0,
                column_count: probe.extent.width,
            })
            .collect();
        let pipeline = self
            .triangle_pipeline
            .as_mut()
            .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)
            .map_err(RenderError::from)?;
        let started = Instant::now();
        let outputs = pipeline
            .compute_triangle_hot_color_chain_checkpoints(
                first.extent,
                &first.resident_bytes,
                &dispatches,
                &retained.checkpoint_limits,
            )
            .map_err(WgpuRawDpcExecutionError::TriangleDraw)
            .map_err(RenderError::from)?;
        let mut outputs = ExactCheckpointImages::try_new(outputs, retained.checkpoint_limits.len())
            .map_err(RenderError::from)?;
        for &limit in &retained.checkpoint_limits {
            let actual = outputs.take_next();
            let last = &retained.probes[limit - 1];
            validate_compute_probe_output(first, last, &actual).map_err(RenderError::from)?;
        }
        outputs.finish();
        let elapsed = started.elapsed();
        let draw_count = retained.probes.iter().try_fold(0u32, |count, probe| {
            count.checked_add(u32::try_from(probe.batch.draws().len()).ok()?)
        });
        Ok(ComputeRasterProbeReceipt {
            submission_count: 1,
            batch_count: u32::try_from(retained.probes.len())
                .expect("bounded checkpoint batch count fits u32"),
            draw_count: draw_count.expect("bounded checkpoint draw count fits u32"),
            target_pixels: first.extent.width * first.extent.height,
            admission_elapsed: Duration::ZERO,
            elapsed,
            effects_elapsed: Duration::ZERO,
        })
    }

    /// Selects the transaction-integrated compute replacement for eligible
    /// packets. It remains an explicit A/B control until live certification.
    pub fn set_compute_raster_replace_enabled(&mut self, enabled: bool) {
        self.probes.compute_raster_replace_enabled = enabled;
        self.compute_raster_replace_receipt = None;
    }

    pub fn take_compute_raster_replace_receipt(&mut self) -> Option<ComputeRasterProbeReceipt> {
        self.compute_raster_replace_receipt.take()
    }

    pub fn set_task_compute_raster_enabled(&mut self, enabled: bool) {
        self.probes.task_compute_raster_enabled = enabled;
    }

    pub fn set_task_cpu_color_batch_enabled(&mut self, enabled: bool) {
        self.probes.task_cpu_color_batch_enabled = enabled;
    }

    /// The currently-published physical TMEM state. Exposed for diagnostics
    /// and tests; production callers reach committed TMEM content only
    /// through this same coordinator-owned value.
    pub fn physical_tmem(&self) -> &PhysicalTmemState {
        self.coordinator.physical()
    }

    /// The current durable logical RDP state. Exposed for diagnostics and
    /// tests; advances only through a successful `plan_raw_dpc` call.
    pub fn rdp_state(&self) -> &RdpState {
        &self.rdp_state
    }

    /// The resident color targets this backend has published, or `None` if
    /// no admitted `FillRectangle` has ever reached execution (the registry
    /// is built lazily, from the first admitted fill's own capture layout).
    /// Exposed for diagnostics and tests, mirroring `physical_tmem()`'s own
    /// convention on this struct.
    pub fn color_targets(&self) -> Option<&ColorTargetRegistry> {
        self.color_targets.as_ref()
    }

    /// Whether a staged-but-unpublished fill token is currently held.
    /// Exposed for the nonmutation tests, which must be able to prove a
    /// rejected fill left no token behind.
    pub fn has_pending_fill_publication(&self) -> bool {
        self.pending_fill_publication.is_some()
            || !self.task_batch_pending_fill_publications.is_empty()
    }

    /// `RenderBackend::create`'s body: block once, synchronously, on
    /// `UninitializedTrianglePipeline::request()`, storing the resulting
    /// renderer or reporting a richly-typed failure. `pub(crate)`, not
    /// fully `pub` (unlike `WgpuRawDpcExecutionError`, which is public
    /// because external callers reach it through
    /// `RenderBackend::execute_raw_dpc`'s conversion path): nothing
    /// outside this crate has a reason to call `create_inner` instead of
    /// the trait's `create`, so there is no reason to widen this crate's
    /// public API surface for it. It exists, distinct from `create`
    /// itself, only so this module's own `#[cfg(test)]` code can assert
    /// on the exact `WgpuCreateError` variant -- specifically
    /// distinguishing a genuine `NoAdapter` from any other failure --
    /// which `RenderBackend::create`'s own `Result<(), RenderError>`
    /// signature cannot preserve once converted (`RenderError::Backend`'s
    /// `reason` is a plain `String`).
    ///
    /// Also derives and stores `triangle_target_extent` from `cfg`
    /// (§1e: `TriangleTargetExtent { width: cfg.width, height:
    /// cfg.height }`, an identity mapping -- no RDP viewport/scissor
    /// concept exists to derive anything narrower from), so a later
    /// triangle draw knows what render-target size to use without `cfg`
    /// being threaded through every call.
    /// Whether `create_inner` recorded the host-configured framebuffer extent.
    ///
    /// Exposed for the adapterless harness, which asserts the extent survived
    /// a `NoAdapter` create rather than reaching into the field itself.
    #[cfg(any(test, feature = "conformance-runner"))]
    pub(crate) fn has_configured_target_extent(&self) -> bool {
        self.configured_target_extent.is_some()
    }

    /// Keep adapterless conformance fixtures on the authoritative CPU raster
    /// path after `create_inner` has proved that no diagnostic GPU is
    /// available. Real backend construction still returns `NoAdapter`, and
    /// host-GPU tests still require successful creation before reaching this
    /// test-only seam.
    #[cfg(any(test, feature = "conformance-runner"))]
    pub(crate) fn disable_adapterless_gpu_diagnostic(&mut self) {
        assert!(
            self.triangle_pipeline.is_none(),
            "an adapterless fallback cannot discard a live triangle pipeline"
        );
        self.probes.gpu_triangle_draw_enabled = false;
    }

    pub(crate) fn create_inner(&mut self, cfg: &RenderConfig) -> Result<(), WgpuCreateError> {
        // Recorded before the device request, unlike `triangle_target_extent`
        // below: an admitted `FillRectangle` is executed entirely CPU-side
        // and needs only this host-configured height, so a host with no GPU
        // adapter must still be able to execute one. See
        // `configured_target_extent`'s own doc for the nonclaim this carries.
        self.configured_target_extent = Some(TriangleTargetExtent {
            width: cfg.width,
            height: cfg.height,
        });
        // **The same height, given to the DECODER.** `plan_raw_triangle`
        // declares one write per covered scanline and `SetColorImage` carries
        // no height, so without this the decoder's only bound is installed
        // RDRAM -- and a triangle taller than the target declares ranges past
        // its end, which `verify_accesses_inside` then refuses for the whole
        // packet. Set from the SAME `cfg` field as `configured_target_extent`
        // above, on the same line of control flow, so the decoder's bound and
        // the executor's extent cannot drift apart.
        self.rdp_state.set_color_target_height(cfg.height);
        let outcome = pollster::block_on(
            UninitializedTrianglePipeline::new(HeadlessBackend::default()).request(),
        )
        .map_err(WgpuCreateError::Request)?;
        match outcome {
            TrianglePipelineDeviceOutcome::Ready(renderer) => {
                self.triangle_pipeline = Some(renderer);
                self.triangle_target_extent = Some(TriangleTargetExtent {
                    width: cfg.width,
                    height: cfg.height,
                });
                Ok(())
            }
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => {
                Err(WgpuCreateError::NoAdapter(no_adapter))
            }
        }
    }

    /// The most recent triangle draw's real GPU-observed color/depth
    /// output (§1e), or `None` if no triangle-bearing `execute_raw_dpc`
    /// call has succeeded yet. Exposed for diagnostics and tests, mirroring
    /// `physical_tmem()`/`rdp_state()`'s own diagnostic-accessor
    /// convention on this struct -- never an accumulated history, never a
    /// persistent framebuffer.
    pub fn last_triangle_draw(&self) -> Option<&TriangleDrawOutput> {
        self.triangle_draw_output.as_ref()
    }

    /// Test-only escape hatch into the real `TrianglePipelineRenderer` this
    /// backend already owns (post-`create_inner`), so the CPU-vs-WGSL
    /// differential (published committed-TMEM textured-draw card §4/§7) can
    /// call `submit_admitted_triangle` directly with literal
    /// `NeutralTriangleVertex` UVs -- avoiding the separate, already-
    /// characterized RT64 fixed-point texcoord wire-decode pipeline
    /// (`raw_dpc::triangle_vertices::decode_texture`), which is not this
    /// slice's own arithmetic and would only add an unrelated correctness
    /// risk to a differential whose entire purpose is validating the WGSL
    /// TMEM-sampling port itself. `production.rs`'s own `RetrievedTriangleDraw`-
    /// literal test fixtures (e.g. `fixture_vertex`) already establish this
    /// same "construct the vertex/tile-state literally, skip wire decode"
    /// convention for host-GPU pipeline tests.
    #[cfg(all(test, feature = "host-gpu-tests"))]
    pub(crate) fn triangle_pipeline_for_test(&mut self) -> &mut TrianglePipelineRenderer {
        self.triangle_pipeline
            .as_mut()
            .expect("create_inner must succeed before this test accessor is used")
    }

    /// Maps every collected triangle, in stream order, into one
    /// `TriangleFixture` each (via `targets::triangle_pipeline`'s
    /// `admitted_triangle_fixture`, the same conversion
    /// `submit_admitted_triangle` uses for its own single-fixture path),
    /// using the identity `TriangleRasterParams` derived once from the
    /// stored `triangle_target_extent` (never recomputed per triangle,
    /// never defaulted) plus each draw's own viewport-derived
    /// `screen_scale`/`screen_offset`. The whole batch then submits through
    /// exactly one `TrianglePipelineRenderer::submit_triangles(&fixtures)`
    /// call, in the same order the fixtures were collected: one shared
    /// render pass, one `LoadOp::Clear`, no reordering or coalescing of
    /// draws. This is required, not incidental -- `submit_triangles` clears
    /// its color+depth target once, before the first draw in the pass, so
    /// a multi-triangle primitive (a `TextureRectangle`/
    /// `TextureRectangleFlip` always admits as exactly two triangles) or an
    /// ordinary sequence of several `RawTriangle` draws in one
    /// `execute_raw_dpc` call must land in the same pass to all survive
    /// into `last_triangle_draw()`'s single output; submitting them one
    /// call at a time would re-clear the target between triangles and
    /// silently discard every draw but the last.
    ///
    /// A pre-submit mapping error (a missing draw's `MissingTriangleDrawState`)
    /// is surfaced before any fixture reaches the GPU. A batch submission or
    /// shader-status error is only detected after submission -- `complete()`
    /// observes real GPU output -- but `self.triangle_draw_output` is still
    /// never touched until every stage (mapping, submission, readback,
    /// shader-status check) succeeds, so a failure anywhere in the pipeline
    /// leaves the prior successful value in place, unchanged, never cleared:
    /// an old-but-real result outlives a failed attempt to replace it,
    /// matching this file's own "never a silent partial state" convention
    /// elsewhere. Zero triangles is a successful no-op: no fixtures, no
    /// submission, `last_triangle_draw()` untouched.
    pub(super) fn draw_admitted_triangles(
        &mut self,
        triangles: Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
        pending_tmem: Option<Vec<TmemGpuProjection>>,
        project_gpu_tmem: bool,
    ) -> Result<(), WgpuRawDpcExecutionError> {
        // **The TMEM byte images this draw samples: one per triangle, from
        // this packet's OWN sealed transaction, not the published slot.**
        //
        // Projected once per triangle, at each triangle's own stream
        // position, because within one packet TMEM is not one image: a
        // packet's own loads change it as they run.
        // `project_pending_tmem_per_triangle` builds the list upstream,
        // where the sealed transaction is still alive, selecting each entry
        // with the same `prefix_before` the CPU texel reader uses. This
        // loop only indexes it.
        //
        // What changed is WHICH image, and it had to. Projecting
        // `self.coordinator.physical()` -- the published, already-committed
        // slot -- reads a state that does not contain this packet's own
        // `LoadBlock`/`LoadTile`/`LoadTLUT`, because publication happens
        // strictly after execution. Measured at `87b2f5b0`: a texrect whose
        // latched combine references `TEXEL0` failed with
        // `TmemSampleFailed { status: 2 }`
        // (`TMEM_SAMPLE_STATUS_INVALID_BYTE`) -- the shader read addresses
        // the published projection reported invalid. The same fixture with
        // `set_combine(0, 0)` executed cleanly, because a combine that
        // references no texel makes the shader short-circuit before it ever
        // samples TMEM. The control passing was never evidence the GPU
        // sampled correctly; it was evidence it never sampled at all. That
        // is why the GPU path had never actually fetched a texrect's texels.
        //
        // `None` is not a silent fallback -- it is a different, narrower
        // packet shape with its own correct answer. A packet that staged no
        // TMEM transaction at all (`StagedOutcome::NoPhysicalSuccessor`: raw
        // triangles, zero loads) has nothing pending to project, and the
        // published slot is then the honest image rather than a substitute
        // for one: there is no load in this packet for it to be missing.
        // A texrect in a packet with no load reaches the `None` arm too,
        // and committed TMEM is the correct image for it for the same
        // reason `TexrectTmemSource::Committed` is on the CPU side: the
        // packet staged no proposal, so durable state is not a substitute
        // for one -- it is what the RDP's TMEM holds. Both paths therefore
        // project the SAME image for the same packet, which is what keeps
        // the GPU raster and the CPU texel reader from disagreeing about a
        // load-free texrect. Neither arm is a guess.
        //
        // One image per triangle either way, so the loop below indexes a
        // single list and never re-decides which source an entry came from.
        // The `None` arm repeats the committed projection because that IS
        // one image for every triangle in a load-free packet -- there is no
        // load in it to change TMEM mid-packet -- so materialising the
        // repeat removes the second code path that could disagree with the
        // first, at a clone the pipeline was going to make per fixture
        // regardless.
        let per_triangle = project_gpu_tmem.then(|| match pending_tmem {
            Some(projections) => projections,
            None => vec![project_committed_tmem(self.coordinator.physical()); triangles.len()],
        });
        // A list shorter than the draw would leave a triangle with no
        // image, and the only images available to substitute are the two
        // this whole change exists to withhold: another triangle's, or the
        // whole-packet post-image. Refused by name rather than padded.
        // `project_pending_tmem_per_triangle` walks
        // `plan.triangle_commands` while `execute_raw_dpc` draws
        // `plan.triangles`, two vectors pushed at one site, so a mismatch
        // is a structural break rather than a length a caller could
        // legitimately vary.
        if let Some(per_triangle) = &per_triangle {
            if per_triangle.len() != triangles.len() {
                return Err(WgpuRawDpcExecutionError::TmemProjectionCountMismatch {
                    projections: per_triangle.len(),
                    triangles: triangles.len(),
                });
            }
        }

        let mut fixtures = Vec::with_capacity(triangles.len());
        for (triangle_index, draw) in triangles.into_iter().enumerate() {
            let draw = draw.map_err(WgpuRawDpcExecutionError::MissingTriangleDrawState)?;
            // **`tlut_en` is a `SetOtherModes` bit, not a `SetTile` field.**
            // Neither `TileBindingParams` constructor can know it, so the
            // TLUT mode is stamped here from the SAME `RetrievedTriangleDraw`
            // snapshot the combiner/blend/coverage state above came from --
            // the `OtherMode` current at THIS triangle's own stream
            // position, never the walk's running final value.
            //
            // Without this the shader saw `lut_mode = Disabled` for every
            // draw and refused WM2000's IA4-under-`G_TT_RGBA16` tile with
            // `TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT`, for a format the
            // hardware ignores while the TLUT is on.
            let lut_mode = draw.other_mode.texture_lut_mode();
            let tile_binding = draw.tile_binding.with_lut_mode(lut_mode);
            let blend_mode = BlendModeState {
                other_mode: draw.other_mode,
                blend_color_register: draw.blend_color.rgba8(),
                fog_color: draw.fog_color.rgba8(),
            };
            let cycle_count = blend_mode.cycle_count();
            let cycle0 = blend_mode.cycle(0);
            let cycle1 = blend_mode.cycle(1);
            let active_cycles = match cycle_count {
                0 => [None, None],
                1 => [Some(cycle0), None],
                _ => [Some(cycle0), Some(cycle1)],
            };
            if active_cycles
                .into_iter()
                .flatten()
                .any(ResolvedBlendCycle::requires_framebuffer_alpha)
            {
                return Err(WgpuRawDpcExecutionError::BlendRequiresFramebuffer { triangle_index });
            }
            let reads_framebuffer_color = active_cycles.into_iter().flatten().any(|cycle| {
                matches!(cycle.p, BlendColorInput::Framebuffer)
                    || matches!(cycle.m, BlendColorInput::Framebuffer)
            });
            let blend_params = ResolvedFragmentBlendParams {
                cycle_count,
                cycle0,
                cycle1,
                blend_color: draw.blend_color,
                fog_color: draw.fog_color,
                reads_framebuffer_color,
            };

            // Everything above is renderer-neutral admission and always
            // runs. Extent, TMEM projection, and pipeline construction below
            // belong only to the optional diagnostic GPU draw. Requiring
            // `RenderBackend::create` for them when that draw is disabled
            // would reject the authoritative CPU raster path in adapterless
            // ABI consumers.
            if !self.probes.gpu_triangle_draw_enabled {
                continue;
            }
            let extent = self
                .triangle_target_extent
                .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
            // Per-triangle, not loop-invariant: a `TextureRectangle`-sourced
            // draw's `screen_scale`/`screen_offset` come from its own
            // `viewport` override (RT64's `convertViewportRect`,
            // `rt64_framebuffer_renderer.cpp:1656-1658`); a `RawTriangle`
            // keeps today's hardcoded identity, byte-identical to before
            // this field existed.
            let (screen_scale, screen_offset) = match draw.viewport {
                None => ([1.0, 1.0], [0.0, 0.0]),
                Some(viewport) => {
                    let left = viewport.left as f32;
                    let top = viewport.top as f32;
                    let right = viewport.right as f32;
                    let bottom = viewport.bottom as f32;
                    let width = extent.width as f32;
                    let height = extent.height as f32;
                    (
                        [(right - left) / width, (bottom - top) / height],
                        [
                            (left + (right - left) / 2.0 - width / 2.0) / (width / 2.0),
                            (height / 2.0 - (top + (bottom - top) / 2.0)) / (height / 2.0),
                        ],
                    )
                }
            };
            let raster_params = TriangleRasterParams {
                resolution: [extent.width as f32, extent.height as f32],
                screen_scale,
                screen_offset,
            };
            if let Some(per_triangle) = &per_triangle {
                fixtures.push(admitted_triangle_fixture(
                    draw.vertices,
                    draw.other_mode,
                    draw.combine_params,
                    raster_params,
                    extent,
                    per_triangle[triangle_index],
                    tile_binding,
                    draw.blend_color,
                    draw.env_color,
                    draw.prim_color,
                    blend_params,
                    draw.source == TriangleSource::TextureRectangle,
                ));
            }
        }

        if !self.probes.gpu_triangle_draw_enabled {
            return Ok(());
        }
        if fixtures.is_empty() {
            self.triangle_pipeline
                .as_ref()
                .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
            return Ok(());
        }

        // **Everything above this line is admission validation and always runs.**
        //
        // An earlier version of this gate sat at the CALL SITE and skipped
        // this whole function on the play path. That was wrong: this is not
        // a pure output-populating draw, it is a fallible validation
        // boundary. Skipping it returned `Ok(())` for packets that should
        // have been refused -- `TriangleDrawBeforeCreate`,
        // `TmemProjectionCountMismatch`, `MissingTriangleDrawState`,
        // `BlendRequiresFramebuffer` above all
        // stopped firing. Found by an independent audit, which also showed
        // `TriangleDrawBeforeCreate` is narrower: it belongs to the optional
        // GPU diagnostic draw and is checked only when that draw is enabled.
        //
        // Only the GPU submission below is skipped, and only its *output* is
        // diagnostic: `triangle_draw_output` is "never an accumulated
        // history, never a persistent framebuffer" and `present` refuses to
        // scan it out. Guest pixels come from the CPU rasterizer.
        let pipeline = self
            .triangle_pipeline
            .as_mut()
            .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
        let in_flight = pipeline
            .submit_triangles(&fixtures)
            .map_err(WgpuRawDpcExecutionError::TriangleDraw)?;
        let output = in_flight
            .complete()
            .map_err(WgpuRawDpcExecutionError::TriangleDraw)?;
        // Observable shader failure status (card audit repair): propagate
        // any fragment's non-OK `tmem_sample.wgsl` status to a named Rust
        // execution error -- never silently accepted as though the batch's
        // texture sampling succeeded everywhere.
        if let Some(&status) = output
            .tmem_sample_status
            .iter()
            .find(|&&status| status != TMEM_SAMPLE_STATUS_OK)
        {
            // **Measure the tile, do not infer it.** The status attachment
            // is per-pixel over the whole batch, so a status code alone
            // names no triangle. Attribution is exact for the format
            // refusal specifically: `tmem_sample.wgsl` raises
            // `TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT` from a pure predicate
            // over `TileBindingParams` alone, before any addressing runs,
            // so re-evaluating that same predicate over this batch's own
            // fixtures identifies exactly the fixtures that could have
            // written it. For the other statuses (which depend on the
            // fragment's UVs, not only its tile) the first bound fixture is
            // reported instead, and the triangle index is the only field
            // that is then approximate.
            let culprit = fixtures
                .iter()
                .position(|fixture| {
                    fixture.tile_binding.bound != 0
                        && !fixture.tile_binding.is_supported_direct_format()
                })
                .or_else(|| {
                    fixtures
                        .iter()
                        .position(|fixture| fixture.tile_binding.bound != 0)
                })
                .unwrap_or(0);
            let tile = fixtures[culprit].tile_binding;
            return Err(WgpuRawDpcExecutionError::TmemSampleFailed {
                status,
                triangle_index: culprit,
                tile_format: tile.format,
                tile_pixel_size: tile.pixel_size,
                tile_lut_mode: tile.lut_mode,
            });
        }
        self.triangle_draw_output = Some(output);
        Ok(())
    }
}

/// Named, loud rejection for one `create_inner` call -- kept distinct
/// from `WgpuRawDpcExecutionError`/`WgpuBackendConstructionError` because
/// this is specifically the triangle-pipeline device-request failure
/// surface. `pub(crate)`, matching `create_inner`'s own visibility (see
/// its doc comment): this exists so this crate's own tests can
/// distinguish `NoAdapter` (no exotic device failure, just no matching
/// adapter on this host) from `Request` (a genuine `TrianglePipelineError`
/// -- adapter/device request rejected, or the pipeline prewarm itself
/// reported a device error) before either is collapsed into
/// `RenderError::Backend`'s plain `String` reason at the trait boundary.
#[derive(Debug)]
pub(crate) enum WgpuCreateError {
    NoAdapter(crate::NoAdapter),
    Request(TrianglePipelineError),
}

impl core::fmt::Display for WgpuCreateError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoAdapter(no_adapter) => write!(
                formatter,
                "no GPU adapter available for the triangle-draw pipeline: {no_adapter:?}"
            ),
            Self::Request(error) => {
                write!(
                    formatter,
                    "triangle-pipeline device request failed: {error}"
                )
            }
        }
    }
}

impl std::error::Error for WgpuCreateError {}

impl From<WgpuCreateError> for RenderError {
    fn from(error: WgpuCreateError) -> Self {
        RenderError::Backend {
            backend: "render-wgpu/create",
            reason: error.to_string(),
        }
    }
}

impl RenderBackend for WgpuBackend {
    /// Requests a real GPU device and derives this backend's triangle
    /// render-target extent from `cfg`, both eagerly and synchronously
    /// (blocks on `UninitializedTrianglePipeline::request()`). A repeated
    /// call is a full reset: `triangle_pipeline`/`triangle_target_extent`
    /// are always replaced together, from a fresh device request, never
    /// partially. Thin wrapper over `Self::create_inner`, which returns
    /// the richly-typed `WgpuCreateError` this crate's own tests need to
    /// distinguish a genuine `NoAdapter` from any other failure --
    /// `RenderError::Backend`'s plain `String` reason cannot preserve
    /// that distinction once converted.
    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError> {
        self.create_inner(cfg).map_err(RenderError::from)
    }

    fn observe_non_rdp_write16(
        &mut self,
        _write: fn64_render::NonRdpWrite16,
    ) -> fn64_render::NonRdpWrite16Disposition {
        fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar
    }

    fn deferred_non_rdp_write16_disposition(
        &self,
    ) -> Option<fn64_render::NonRdpWrite16Disposition> {
        Some(fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar)
    }

    /// This backend has no HLE display-list front end: it holds no geometry
    /// microcode catalog, no segment/matrix/vertex state, and no GBI command
    /// decoder. Its whole graphics surface is the raw-DPC seam
    /// (`process_rdp_commands` / `plan_raw_dpc` / `execute_raw_dpc` /
    /// `publish_raw_dpc`), which consumes RDP command words, not `Gfx` words.
    ///
    /// So the only truthful answer to "execute this graphics task" is the
    /// trait's existing one for a microcode this backend cannot high-level
    /// emulate: [`FrameStatus::NeedsLle`], carrying the live IMEM digest.
    /// `fn64-abi`'s `osSpTaskStartGo_recomp` then runs the task's microcode
    /// on the RSP interpreter (`dispatch_lle_task`), and the RDP commands
    /// that microcode writes into DMEM arrive back here through the XBUS
    /// raw-DPC path this backend does implement. That is the same disposition
    /// `ReferenceBackend` reaches for this title -- measured on WM2000
    /// (NWXE), whose every graphics task carries a well-formed F3DEX2 display
    /// list under IMEM digest
    /// `c50d2949c23baae24e706e8e1a5abf2dd315d00aff4cfdd567a03fe81807d1be`,
    /// which is in no catalog, so `ReferenceBackend::process_task` returns
    /// `NeedsLle` for all of them. It is a *disposition*, not a silent no-op:
    /// the task is executed, by the RSP, and `fn64-abi` records a
    /// `render.hle-ucode.needs-lle` unsupported event naming the digest.
    ///
    /// The digest is read from live IMEM, the same authority
    /// `GeometryUcodeCatalog::require_text` and `ReferenceBackend` use, so
    /// the reported microcode identity cannot disagree with theirs. A task
    /// that is not `M_GFXTASK` is still a routing bug rather than a
    /// microcode this backend declines, and stays a loud named error.
    fn process_task(
        &mut self,
        _rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &fn64_render::OsTask,
        _output_addr: u32,
    ) -> Result<fn64_render::FrameStatus, RenderError> {
        if task.task_type != fn64_render::M_GFXTASK {
            return Err(RenderError::Backend {
                backend: "render-wgpu",
                reason: format!(
                    "graphics task dispatch received task type {}; only M_GFXTASK ({}) reaches \
                     this seam",
                    task.task_type,
                    fn64_render::M_GFXTASK
                ),
            });
        }
        let ucode_sha256 =
            fn64_render::UcodeDigest::from_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))
                .as_bytes();
        Ok(fn64_render::FrameStatus::NeedsLle { ucode_sha256 })
    }

    fn enable_presented_post_vi_field_delivery(&mut self) -> Result<(), RenderError> {
        self.presented_field_delivery = PresentedFieldDelivery::PostVi;
        self.presented_source_field = None;
        self.presented_post_vi_field = None;
        self.presented_field = None;
        Ok(())
    }

    /// **Shape (a): a real scanout, not a refusal.** This replaces the named
    /// "presentation is out of scope" rejection that made a registered
    /// `WgpuBackend` fatal at the first VI retrace, because
    /// `present_render_backend` -> `with_render_backend` turns any backend
    /// error into a panic by design (`fn64-abi`'s `setup.rs`).
    ///
    /// A VI retrace is not rasterization. The trait's own doc says so:
    /// "`osViSwapBuffer` only selects which rendered buffer a later field
    /// consumes." What present owes is a *scanout* of the buffer the guest's
    /// latched VI registers point at -- and for this backend that buffer is
    /// in guest RDRAM, because an admitted `FillRectangle`'s bytes are copied
    /// back there by `fn64-abi`'s `copy_committed_guest_writes`. So present
    /// reads guest memory; it does not need a resident image and does not
    /// invent one.
    ///
    /// **Both `PresentMemory` variants are handled, and they are not
    /// symmetric.**
    ///
    /// - `Physical` -- implemented. `crate::vi_scanout` reads the programmed
    ///   source rectangle through the retrace-scoped `PhysicalRdramRead`
    ///   capability, which is the same lane-mapped authority (`^2`/`^3`) the
    ///   reference backend's `load_vi_source` reads through and
    ///   `RdramViewMut::write_u16` writes through. Every VI filter it does
    ///   not implement is refused by its own name (`ViScanoutRefusal`),
    ///   never silently skipped.
    /// - `BackendResidentCompatibility` -- **refused, specifically.** This
    ///   backend holds no resident scanout image to present. Its color
    ///   targets are published *to guest RDRAM* (`ColorTargetRegistry` plus
    ///   `copy_committed_guest_writes`), and `triangle_draw_output` is a
    ///   single draw's readback, replaced per submission and never a
    ///   framebuffer the VI would sample. Presenting it would claim the last
    ///   triangle draw was the field the guest programmed, which for a
    ///   double-buffered title is simply a different buffer. The error names
    ///   that, and names the request forms that do work.
    ///
    /// A failed present leaves the previously presented field in place; see
    /// `presented_field`'s own doc for why discarding it would fabricate a
    /// frame.
    fn present(&mut self, request: fn64_render::PresentRequest<'_>) -> Result<(), RenderError> {
        let (vi, memory) = request.into_parts();
        let memory = match memory {
            fn64_render::PresentMemory::Physical(memory) => memory,
            fn64_render::PresentMemory::BackendResidentCompatibility => {
                return Err(RenderError::Backend {
                    backend: "render-wgpu",
                    reason: "PresentMemory::BackendResidentCompatibility is not supported: \
                             WgpuBackend retains no resident scanout image. Its color targets \
                             are published into guest RDRAM and its triangle_draw_output is one \
                             submission's readback, not a VI-sampled framebuffer. Use \
                             PresentRequest::live or PresentRequest::physical_compatibility, \
                             which supply the physical memory this backend scans out."
                        .to_string(),
                })
            }
        };
        match self.presented_field_delivery {
            PresentedFieldDelivery::Source => {
                self.presented_source_field =
                    crate::vi_scanout::scan_out_rgba5551_source_field(vi, &memory)?;
                self.presented_field = None;
            }
            PresentedFieldDelivery::PostVi => {
                let field = crate::vi_scanout::scan_out_guest_rdram(vi, &memory)?;
                let post_vi = fn64_render::PresentedPostViField::rgba8888(
                    field.presentation,
                    field.width,
                    field.height,
                    field.rgba8,
                )?;
                self.presented_post_vi_field = Some(post_vi);
                self.presented_field = None;
            }
            PresentedFieldDelivery::ConcreteDiagnostic => {
                let field = crate::vi_scanout::scan_out_guest_rdram(vi, &memory)?;
                self.presented_field = Some(field);
            }
        }
        Ok(())
    }

    fn take_presented_source_field(&mut self) -> fn64_render::PresentedSourceFieldAvailability {
        self.presented_source_field.take().map_or(
            fn64_render::PresentedSourceFieldAvailability::Unsupported,
            fn64_render::PresentedSourceFieldAvailability::Ready,
        )
    }

    fn take_presented_post_vi_field(&mut self) -> fn64_render::PresentedPostViFieldAvailability {
        self.presented_post_vi_field.take().map_or(
            fn64_render::PresentedPostViFieldAvailability::Unsupported,
            fn64_render::PresentedPostViFieldAvailability::Ready,
        )
    }

    /// **Shape (a): a real reconfiguration, not a refusal.** This replaces a
    /// silent no-op stub, which AGENTS.md forbids outright.
    ///
    /// A resize here is exactly and only a change of *target geometry*.
    /// That is the whole of it, because this backend owns no device
    /// resource whose size is fixed at `create` time:
    /// `TrianglePipelineRenderer` holds an instance, a device, a queue,
    /// four extent-independent `RenderPipeline`s, a bind-group layout and
    /// one fixed-size dummy buffer -- no swapchain, no persistent color or
    /// depth attachment. Its color+depth textures are created fresh per
    /// submission from the extent handed to `submit_triangles`
    /// (`targets::triangle_pipeline`'s own "fresh per-submission color+depth
    /// texture pair ... not a persistent swapchain"). So there is nothing to
    /// reallocate and no device to re-request: recording the new extent *is*
    /// the recovery, and the next submission builds its attachments at the
    /// new size. Reusing `create_inner` would be actively wrong, not merely
    /// redundant -- it would blow away a live `triangle_pipeline` and
    /// re-request an adapter to reach the identical state.
    ///
    /// Both extents are written, and the pairing invariant
    /// `triangle_pipeline.is_some() == triangle_target_extent.is_some()`
    /// (§1a/§1e) is preserved by construction: `triangle_target_extent` is
    /// updated through `Option::as_mut`, so a resize before any successful
    /// `create` leaves it `None` rather than populating an extent with no
    /// pipeline behind it. `configured_target_extent` is written
    /// unconditionally for the same reason `create_inner` records it before
    /// the device request -- it is the CPU-side fill path's only color-image
    /// height, and an adapterless host must still be able to resize the
    /// target a `FillRectangle` executes into.
    ///
    /// **Zero dimensions are recorded, not clamped and not dropped.** The
    /// trait contract makes `resize` infallible and directs a backend that
    /// cannot honor one to surface it at the next call; both consumers of
    /// these extents already do exactly that, by name --
    /// `ColorTargetExtent::try_new` returns `TargetError::ZeroExtent` and
    /// `validate_triangle_extent` returns `TrianglePipelineError::ZeroExtent`.
    /// Clamping to 1 would fabricate a target geometry the host never asked
    /// for and silently publish a resident of the wrong byte range; ignoring
    /// the call would be the silent no-op this replaces. Recording it routes
    /// a minimized window to a named rejection at the point of use.
    ///
    /// **A pending fill token survives a resize, deliberately.** A staged
    /// `PendingFillPublication` carries an `InitializedCandidateColorTarget`
    /// whose `ColorTargetKey` -- address, extent and byte range -- was
    /// sealed inside the token when the fill executed, and
    /// `ColorTargetRegistry::prepare_publication` reads only that captured
    /// key; it never consults the backend's current
    /// `configured_target_extent`. The invariant is therefore already in the
    /// type system rather than in this method: a resize *cannot* retarget an
    /// executed fill, so there is nothing for this method to guard. Dropping
    /// the token instead would discard the guest-write report of a
    /// submission that completed correctly, and the next
    /// `staged_guest_render_target_writes` would return an empty list --
    /// failing that submission loudly with `EffectCountMismatch` for a
    /// window resize it had nothing to do with. A resize is a statement
    /// about the geometry of *future* targets, never a retroactive
    /// invalidation of a fill that already ran.
    ///
    /// Guest-write nonclaim (module doc): nothing here touches guest RDRAM.
    /// These are host-side extent fields; a resize writes no memory the
    /// guest can observe.
    fn resize(&mut self, w: u32, h: u32) {
        let extent = TriangleTargetExtent {
            width: w,
            height: h,
        };
        self.configured_target_extent = Some(extent);
        // **The decoder's height bound follows the resize too.**
        //
        // `create` sets both from the same `cfg` field and its comment there
        // says the two "cannot drift apart" -- but this method updated only
        // one of them, so every resize left the decoder bounding against the
        // height `create` configured. That was latent while nothing sized
        // anything from it; a partial fill's colour-image seed does, and the
        // drift produced a 256-byte seed for a 128-byte target
        // (`the_adapterless_fill_path_still_works_after_a_resize`, which is
        // how it was found).
        self.rdp_state.set_color_target_height(h);
        if let Some(triangle_target_extent) = self.triangle_target_extent.as_mut() {
            *triangle_target_extent = extent;
        }
    }

    fn supported_ucodes(&self) -> &[fn64_render::UcodeId] {
        &[]
    }
}

impl RawDpcBackend for WgpuBackend {
    /// Returned unconditionally, never varying with whether a fill or a
    /// FullSync site has yet been admitted: a capability is a statement about
    /// what this backend *will* admit, not about what it has admitted, and a
    /// value that changes under the caller is worse than a wider constant
    /// one.
    ///
    /// Nonclaim: the `SiteOnly` half of this variant's name is load-bearing.
    /// This backend decodes a `SYNC_FULL` opcode and binds it to the
    /// capture's boundary; it does not observe a DP interrupt and does not
    /// claim the guest did. See `RawDpcIrCapability`'s own doc for why that
    /// distinction cannot be recovered from the capability value alone and
    /// lives in the supplied `FullSyncBoundary` instead.
    fn raw_dpc_ir_capability(&self) -> RawDpcIrCapability {
        RawDpcIrCapability::TransactionalTmemFillFullSyncSiteOnly
    }

    fn raw_dpc_task_batch_capability(&self) -> RawDpcTaskBatchCapability {
        RawDpcTaskBatchCapability::Transactional
    }

    fn plan_raw_dpc(
        &mut self,
        request: RawDpcPlanRequest,
    ) -> Result<PlannedRawDpcSubmission, RenderError> {
        let (planned, delta, _) = plan_raw_dpc_inner(&self.coordinator, &self.rdp_state, request)
            .map_err(|reason| RenderError::Backend {
            backend: "render-wgpu/raw-dpc-plan",
            reason,
        })?;
        // This single capture is intentionally adjacent to and before the
        // fold: splitting it by register recreates mixed-time carry-in.
        self.raw_dpc_carry_in_before_last_plan = Some(RawDpcCarryIn::capture(&self.rdp_state));
        self.rdp_state.apply(&delta);
        Ok(planned)
    }

    fn plan_raw_dpc_task_batch(
        &mut self,
        requests: Vec<RawDpcPlanRequest>,
    ) -> Result<Vec<PlannedRawDpcSubmission>, RenderError> {
        if requests.is_empty() {
            return Err(RenderError::Backend {
                backend: "render-wgpu/raw-dpc-task-batch-plan",
                reason: "a production raw-DPC task batch cannot be empty".to_string(),
            });
        }
        assert!(
            self.pending_raw_dpc_task_batch.is_none(),
            "a prior raw-DPC task batch was planned but not executed"
        );
        let mut next_state = self.rdp_state.fork_for_decode();
        let mut planned = Vec::with_capacity(requests.len());
        let mut members = VecDeque::with_capacity(requests.len());
        for request in requests {
            let carry_in = RawDpcCarryIn::capture(&next_state);
            let (member, delta, execution) =
                plan_raw_dpc_inner(&self.coordinator, &next_state, request).map_err(|reason| {
                    RenderError::Backend {
                        backend: "render-wgpu/raw-dpc-task-batch-plan",
                        reason,
                    }
                })?;
            next_state.apply(&delta);
            members.push_back(PlannedRawDpcTaskMember {
                carry_in,
                execution,
            });
            planned.push(member);
        }
        self.rdp_state = next_state;
        self.pending_raw_dpc_task_batch = Some(PlannedRawDpcTaskBatch { members });
        Ok(planned)
    }

    fn execute_raw_dpc(
        &mut self,
        bound: BoundSubmittedRawDpc,
    ) -> Result<BackendPreparedRawDpc, RenderError> {
        let replacement_pipeline = if self.probes.compute_raster_replace_enabled {
            self.triangle_pipeline.as_deref_mut()
        } else {
            None
        };
        let (prepared, triangles, pending, draw_tmem, mut compute_probes, replacement_receipt) =
            execute_raw_dpc_inner(
                &mut self.coordinator,
                bound,
                self.raw_dpc_carry_in_before_last_plan
                    .unwrap_or_else(|| RawDpcCarryIn::capture(&self.rdp_state)),
                &mut self.color_targets,
                self.configured_target_extent,
                self.probes.project_gpu_tmem,
                self.probes.compute_raster_probe_enabled,
                self.probes.compute_raster_replace_enabled,
                replacement_pipeline,
                None,
                None,
                None,
            )
            .map_err(RenderError::from)?;

        self.compute_raster_probe_receipt = None;
        self.compute_raster_replace_receipt = replacement_receipt;
        if let Some(checkpoint) = self.compute_raster_checkpoint_probe.as_mut() {
            checkpoint
                .push_packet(core::mem::take(&mut compute_probes), pending.is_some())
                .map_err(RenderError::from)?;
        }
        let mut probe_elapsed = Duration::ZERO;
        let mut probe_draws = 0u32;
        let mut probe_pixels = 0u32;
        let mut probe_batches = 0u32;
        for probe in &compute_probes {
            probe_batches += 1;
            probe_draws += u32::try_from(probe.batch.draws().len())
                .expect("bounded raw-DPC draw count fits u32");
            probe_pixels += probe.extent.width * probe.extent.height;
        }
        if !compute_probes.is_empty() {
            let pipeline = self
                .triangle_pipeline
                .as_mut()
                .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)
                .map_err(RenderError::from)?;
            let started = Instant::now();
            if self.probes.compute_raster_chain_probe_enabled {
                let first = &compute_probes[0];
                for (batch, probe) in compute_probes.iter().enumerate().skip(1) {
                    if probe.ordinal != first.ordinal
                        || probe.extent.width != first.extent.width
                        || probe.extent.height != first.extent.height
                        || probe.batch.target() != first.batch.target()
                        || probe.batch.generation() != first.batch.generation()
                    {
                        return Err(RenderError::from(
                            WgpuRawDpcExecutionError::ComputeRasterProbeChainIncompatible {
                                ordinal: first.ordinal,
                                batch,
                            },
                        ));
                    }
                }
                let dispatches: Vec<_> = compute_probes
                    .iter()
                    .map(|probe| ComputeHotColorDispatch {
                        triangles: &probe.triangles,
                        tmem: &probe.tmem,
                        tile: probe.tile,
                        first_row: 0,
                        row_count: probe.extent.height,
                        first_column: 0,
                        column_count: probe.extent.width,
                    })
                    .collect();
                let actual = pipeline
                    .compute_triangle_hot_color_chain(
                        first.extent,
                        &first.resident_bytes,
                        &dispatches,
                    )
                    .map_err(WgpuRawDpcExecutionError::TriangleDraw)
                    .map_err(RenderError::from)?;
                validate_compute_probe_output(
                    first,
                    compute_probes.last().expect("non-empty probe chain"),
                    &actual,
                )
                .map_err(RenderError::from)?;
            } else {
                let inputs: Vec<_> = compute_probes
                    .iter()
                    .map(|probe| ComputeHotColorBatch {
                        extent: probe.extent,
                        resident_bytes: &probe.resident_bytes,
                        triangles: &probe.triangles,
                        tmem: &probe.tmem,
                        tile: probe.tile,
                    })
                    .collect();
                let outputs = pipeline
                    .compute_triangle_hot_color_batches(&inputs)
                    .map_err(WgpuRawDpcExecutionError::TriangleDraw)
                    .map_err(RenderError::from)?;
                for (probe, actual) in compute_probes.iter().zip(outputs) {
                    validate_compute_probe_output(probe, probe, &actual)
                        .map_err(RenderError::from)?;
                }
            }
            probe_elapsed = started.elapsed();
        }
        if probe_batches != 0 {
            self.compute_raster_probe_receipt = Some(ComputeRasterProbeReceipt {
                submission_count: 1,
                batch_count: probe_batches,
                draw_count: probe_draws,
                target_pixels: probe_pixels,
                admission_elapsed: Duration::ZERO,
                elapsed: probe_elapsed,
                effects_elapsed: Duration::ZERO,
            });
        }

        // **The GPU triangle draw is diagnostic, and it is 65% of this
        // backend's frame time.**
        //
        // `draw_admitted_triangles` fills `self.triangle_draw_output`, which
        // this file documents as "the most recent triangle draw's real
        // GPU-observed color/depth output ... never an accumulated history,
        // never a persistent framebuffer", and which `present` refuses to
        // scan out by name: "one submission's readback, not a VI-sampled
        // framebuffer". Guest-visible pixels come from the CPU rasterizer
        // (`targets::raw_triangle`) writing RDRAM, which VI samples.
        //
        // Measured on WM2000 (rs + wgpu, bounded census, 1200 pumps),
        // timing each layer directly rather than deriving it:
        //
        //     session census `Execute`      18.18 s
        //       draw_admitted_triangles    ~13    s   <-- THIS, unpresented
        //       execute_raw_dpc_inner        5.29 s
        //         raster_triangle            3.88 s   (94-102 ns/px, normal)
        //
        // So ~65% of `execute` cannot change a presented pixel. Every reader
        // of `last_triangle_draw()` is a `#[cfg(test)]` assertion or a
        // `Debug` impl -- verified by grep, 16 call sites, zero in production
        // code -- so skipping it on the play path loses no guest-visible
        // behaviour and no test coverage.
        //
        // Kept, not deleted: the host-GPU suite drives real WGSL on a live
        // adapter through this path. `FN64_GPU_TRIANGLE_DRAW=1` forces it on
        // for a play run that wants to exercise the pipeline.
        if !triangles.is_empty() {
            // Always called: it validates. Only its GPU submission is gated,
            // inside the function -- see the note there.
            raw_dpc_execute_census::timed(raw_dpc_execute_census::Phase::DrawValidation, || {
                self.draw_admitted_triangles(triangles, draw_tmem, self.probes.project_gpu_tmem)
            })
            .map_err(RenderError::from)?;
        }

        // Stored only after every fallible step of THIS submission has
        // succeeded, and replaced rather than merged.
        //
        // Ordering: a triangle draw that fails (`TriangleDrawBeforeCreate`
        // on an adapterless host, for one) makes this call return `Err`,
        // and a submission that failed must leave no redeemable token
        // behind -- so the store happens after the draw, never before it.
        //
        // Replacement: a token still held from an earlier submission was
        // never redeemed, and carrying it forward would let a later
        // `publish_raw_dpc` publish a fill that belongs to a submission
        // that already retired. Dropping it leaves the registry at its
        // prior generation, which is the correct "nothing published"
        // outcome. On the error path above, the stale token is likewise
        // left untouched rather than replaced -- it was already
        // unredeemable by submission identity, and this submission
        // produced nothing to put in its place.
        self.pending_fill_publication = pending;

        Ok(prepared)
    }

    fn execute_raw_dpc_task_batch(
        &mut self,
        bounds: Vec<BoundSubmittedRawDpc>,
    ) -> Result<Vec<BackendPreparedRawDpc>, RenderError> {
        assert!(
            self.last_task_batch_execution_mechanism.is_none(),
            "raw-DPC task mechanism must be consumed before the next task batch"
        );
        let task_cpu_phase_started = task_cpu_phase_census::task_started();
        assert!(
            self.task_cpu_phase_census.is_none(),
            "a task CPU phase census must reach its final publication before the next task"
        );
        let planned_batch =
            self.pending_raw_dpc_task_batch
                .take()
                .ok_or_else(|| RenderError::Backend {
                    backend: "render-wgpu/raw-dpc-task-batch-execute",
                    reason: "no planned raw-DPC task batch is pending".to_string(),
                })?;
        if bounds.is_empty() || bounds.len() != planned_batch.members.len() {
            return Err(RenderError::Backend {
                backend: "render-wgpu/raw-dpc-task-batch-execute",
                reason: format!(
                    "bound member count {} does not match planned member count {}",
                    bounds.len(),
                    planned_batch.members.len()
                ),
            });
        }
        assert!(
            self.pending_fill_publication.is_none()
                && self.task_batch_pending_fill_publications.is_empty(),
            "task-batch execution requires no unpublished color completion"
        );

        let registry_clone_bytes = self
            .color_targets
            .as_ref()
            .map(|registry| {
                registry
                    .residents()
                    .iter()
                    .map(|resident| resident.device_bytes().device_bytes().len())
                    .sum()
            })
            .unwrap_or(0);
        let mut private_color_targets =
            task_compute_census::timed_registry_clone(registry_clone_bytes, || {
                self.color_targets.clone()
            });
        let mut pending_publications = VecDeque::new();
        let mut deferred_draws = Vec::with_capacity(bounds.len());
        let mut prepared = Vec::with_capacity(bounds.len());
        let mut guest_read_pool = TaskGuestReadCapturePool::default();
        let mut ordered_cpu_color_batch = OrderedCpuColorBatch::new();
        let mut task_cpu_phase_census =
            task_cpu_phase_census::Task::begin(bounds.len(), task_cpu_phase_started);
        let mut compute_members = 0usize;
        let mut cpu_members = 0usize;
        {
            let mut batch = self.coordinator.begin_execution_batch();
            let mut members = bounds
                .into_iter()
                .zip(planned_batch.members)
                .map(|(bound, member)| {
                    (
                        bound,
                        member.carry_in,
                        TaskMemberDispatch::Planned(member.execution),
                    )
                })
                .peekable();
            let mut retry_as_cpu = None;
            while let Some((bound, carry_in, dispatch)) =
                retry_as_cpu.take().or_else(|| members.next())
            {
                if dispatch == TaskMemberDispatch::Planned(PlannedTaskExecution::ComputeCandidate)
                    && self.probes.task_compute_raster_enabled
                {
                    if ordered_cpu_color_batch.tail.is_some() {
                        ordered_cpu_color_batch
                            .flush(private_color_targets.as_mut().expect(
                                "an ordered CPU color accumulator implies a private registry",
                            ))
                            .map_err(WgpuRawDpcExecutionError::Target)
                            .map_err(RenderError::from)?;
                    }
                    let segment_started = task_compute_census::segment_started();
                    let mut color_batch = ColorTargetExecutionBatch::new();
                    let mut staged_segment = Vec::new();
                    let mut next = Some((bound, carry_in));
                    loop {
                        let (bound, carry_in) = next.take().expect("compute segment member exists");
                        let physical = staged_segment
                            .iter()
                            .rev()
                            .find_map(|member: &StagedRawDpcMember| match &member.outcome {
                                StagedOutcome::DeferredMixedColorAndTmem { tmem, .. } => {
                                    Some(tmem.physical())
                                }
                                _ => None,
                            })
                            .unwrap_or_else(|| batch.physical());
                        let disposition = admit_task_compute_member(
                            &batch,
                            Some(physical),
                            bound,
                            carry_in,
                            &mut private_color_targets,
                            self.configured_target_extent,
                            self.probes.project_gpu_tmem,
                            &mut color_batch,
                            &mut guest_read_pool,
                        )
                        .map_err(RenderError::from)?;
                        let staged = match disposition {
                            TaskComputeDisposition::Compute(ComputeEligibleTaskMember(staged)) => {
                                staged
                            }
                            TaskComputeDisposition::Cpu { bound, reason } => {
                                retry_as_cpu =
                                    Some((bound, carry_in, TaskMemberDispatch::Cpu(reason)));
                                break;
                            }
                        };
                        staged_segment.push(staged);
                        next = match members.peek() {
                            Some((
                                _,
                                _,
                                TaskMemberDispatch::Planned(PlannedTaskExecution::ComputeCandidate),
                            )) => {
                                let (bound, carry_in, _) =
                                    members.next().expect("peeked compute member disappeared");
                                Some((bound, carry_in))
                            }
                            _ => None,
                        };
                        if next.is_none() {
                            break;
                        }
                    }
                    if staged_segment.is_empty() {
                        continue;
                    }
                    let pipeline = self
                        .triangle_pipeline
                        .as_deref_mut()
                        .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)
                        .map_err(RenderError::from)?;
                    let segment_members = staged_segment.len();
                    let program_attribution = task_compute_census::wants_program_attribution()
                        .then(|| compute_segment_program_attribution(&staged_segment));
                    let completed_segment =
                        complete_deferred_compute_segment(&mut batch, pipeline, staged_segment)
                            .map_err(RenderError::from)?;
                    let compute_elapsed = task_compute_census::record_segment(
                        segment_members,
                        program_attribution,
                        segment_started,
                    );
                    if let Some(task) = task_cpu_phase_census.as_mut() {
                        task.record_compute_segment(compute_elapsed);
                    }
                    compute_members += segment_members;
                    let mut completed = completed_segment.iter();
                    let first = completed
                        .next()
                        .expect("a staged compute segment completes at least one member");
                    let mut shadow_segment =
                        CompletedTaskColorSegment::new(first.pending.color.full());
                    for member in completed {
                        shadow_segment
                            .append(member.pending.color.full())
                            .map_err(WgpuRawDpcExecutionError::Target)
                            .map_err(RenderError::from)?;
                    }
                    // `shadow_segment` has already validated every generation
                    // edge. Its full tail bytes are copied into the private
                    // registry only when another task member can consume
                    // them. A terminal segment's pending publications own all
                    // checkpoint images through `ExactCheckpointImages`; the
                    // private shadow has no remaining reader.
                    if retry_as_cpu.is_some() || members.peek().is_some() {
                        let shadow_byte_count = completed_segment
                            .last()
                            .expect("a completed compute segment is nonempty")
                            .pending
                            .color
                            .full()
                            .device_bytes()
                            .device_bytes()
                            .len();
                        task_compute_census::timed_shadow_clone(shadow_byte_count, || {
                            private_color_targets
                                .as_mut()
                                .expect("a staged color completion built the private registry")
                                .commit_task_shadow_segment(shadow_segment)
                        })
                        .map_err(WgpuRawDpcExecutionError::Target)
                        .map_err(RenderError::from)?;
                    }
                    for completed in completed_segment {
                        pending_publications.push_back(completed.pending);
                        deferred_draws.push((completed.triangles, completed.draw_tmem));
                        prepared.push(completed.prepared);
                    }
                } else {
                    cpu_members += 1;
                    let reason = match dispatch {
                        TaskMemberDispatch::Planned(PlannedTaskExecution::Cpu(reason)) => {
                            TaskComputeCpuReason::Planned(reason)
                        }
                        TaskMemberDispatch::Planned(PlannedTaskExecution::ComputeCandidate) => {
                            TaskComputeCpuReason::ComputeDisabled
                        }
                        TaskMemberDispatch::Cpu(reason) => reason,
                    };
                    let cpu_started = task_compute_census::cpu_started();
                    let ordered_cpu_batch = self
                        .probes
                        .task_cpu_color_batch_enabled
                        .then_some(&mut ordered_cpu_color_batch);
                    let (member, triangles, pending, draw_tmem, _, _) = execute_raw_dpc_inner(
                        &mut batch,
                        bound,
                        carry_in,
                        &mut private_color_targets,
                        self.configured_target_extent,
                        self.probes.project_gpu_tmem,
                        false,
                        false,
                        None,
                        Some(&mut guest_read_pool),
                        ordered_cpu_batch,
                        task_cpu_phase_census.as_mut(),
                    )
                    .map_err(RenderError::from)?;
                    let cpu_phase_attributed = pending
                        .as_ref()
                        .is_some_and(|pending| pending.cpu_phase_attributed);
                    let cpu_elapsed = task_compute_census::record_cpu(reason, cpu_started);
                    if let Some(task) = task_cpu_phase_census.as_mut() {
                        task.record_member_total(cpu_phase_attributed, cpu_elapsed);
                    }
                    if let Some(pending) = pending {
                        let pending = task_cpu_phase_census::timed(
                            task_cpu_phase_census.as_mut(),
                            pending.cpu_phase_attributed,
                            task_cpu_phase_census::Phase::SparseCheckpoint,
                            || ordered_cpu_color_batch.finish_member(pending),
                        )
                        .map_err(WgpuRawDpcExecutionError::Target)
                        .map_err(RenderError::from)?;
                        if members.peek().is_some()
                            && matches!(&pending.color, PendingColorPublication::Full(_))
                        {
                            let shadow_byte_count =
                                pending.color.full().device_bytes().device_bytes().len();
                            task_compute_census::timed_shadow_clone(shadow_byte_count, || {
                                private_color_targets
                                    .as_mut()
                                    .expect("a staged color completion built the private registry")
                                    .commit_task_shadow_segment(CompletedTaskColorSegment::new(
                                        pending.color.full(),
                                    ))
                            })
                            .map_err(WgpuRawDpcExecutionError::Target)
                            .map_err(RenderError::from)?;
                        }
                        pending_publications.push_back(pending);
                    } else {
                        assert!(
                            ordered_cpu_color_batch.active.is_none(),
                            "an ordered CPU color member must produce publication authority"
                        );
                    }
                    deferred_draws.push((triangles, draw_tmem));
                    prepared.push(member);
                }
            }
            batch.finish();
        }
        debug_assert_eq!(compute_members + cpu_members, prepared.len());
        task_compute_census::record_task(prepared.len(), cpu_members);

        if self.color_targets.is_none() {
            if let Some(private) = private_color_targets.as_ref() {
                self.color_targets = Some(
                    ColorTargetRegistry::try_new(private.layout(), COLOR_TARGET_REGISTRY_CAPACITY)
                        .map_err(WgpuRawDpcExecutionError::Target)
                        .map_err(RenderError::from)?,
                );
            }
        }
        for (triangles, draw_tmem) in deferred_draws {
            if !triangles.is_empty() {
                self.draw_admitted_triangles(triangles, draw_tmem, self.probes.project_gpu_tmem)
                    .map_err(RenderError::from)?;
            }
        }
        self.task_batch_pending_fill_publications = pending_publications;
        self.task_cpu_phase_census = task_cpu_phase_census;
        self.last_task_batch_execution_mechanism = Some(
            fn64_render::RawDpcTaskBatchExecutionMechanism::try_new(cpu_members, compute_members)
                .expect("a successful raw-DPC task batch executes at least one member"),
        );
        Ok(prepared)
    }

    fn take_raw_dpc_task_batch_execution_mechanism(
        &mut self,
    ) -> Option<fn64_render::RawDpcTaskBatchExecutionMechanism> {
        self.last_task_batch_execution_mechanism.take()
    }

    fn staged_guest_render_target_writes(
        &mut self,
        submission: fn64_render_ir::SubmissionIdentity,
    ) -> Vec<CompletedWrite> {
        // A submission mismatch deliberately yields an EMPTY list rather
        // than a panic or the wrong fill's writes: the caller then takes the
        // zero-write commit branch, which fails loudly with
        // `EffectCountMismatch` against the packet's own nonempty
        // guest-write journal. A loud rejection, never a quiet wrong publish.
        self.pending_fill_publication
            .as_ref()
            .filter(|pending| pending.submission == submission)
            .or_else(|| {
                self.task_batch_pending_fill_publications
                    .iter()
                    .find(|pending| pending.submission == submission)
            })
            .map(|pending| pending.guest_writes.clone())
            .unwrap_or_default()
    }

    /// The bytes behind the same pending fill token
    /// [`Self::staged_guest_render_target_writes`] reported ranges for, in
    /// the identical order -- one `Vec<u8>` per `CompletedWrite`, sliced out
    /// of the fill's own full-extent `DeviceColorBytes` buffer at each
    /// write's declared physical range.
    ///
    /// The slicing is the same physical -> buffer-relative arithmetic
    /// `fill_completed_writes` used to build the digests in the first place,
    /// so these bytes are by construction the exact bytes those digests
    /// cover. The caller re-derives each digest anyway (see the trait
    /// method's own doc) rather than trusting that construction argument.
    ///
    /// A submission mismatch yields an EMPTY list, exactly as the sibling
    /// method does and for the same reason: a caller that committed a
    /// nonempty write list and then gets no bytes must fail loudly, never
    /// copy some other submission's pixels into guest memory.
    ///
    /// Nonclaim: this method writes nothing. It hands owned copies to a
    /// caller that owns the RDRAM allocation; whether any guest byte changes
    /// is that caller's decision, made after its own commit succeeded.
    fn committed_guest_render_target_bytes(
        &mut self,
        submission: fn64_render_ir::SubmissionIdentity,
    ) -> Vec<Arc<[u8]>> {
        let Some(pending) = self
            .pending_fill_publication
            .as_ref()
            .filter(|pending| pending.submission == submission)
            .or_else(|| {
                self.task_batch_pending_fill_publications
                    .iter()
                    .find(|pending| pending.submission == submission)
            })
        else {
            return Vec::new();
        };

        match &pending.color {
            PendingColorPublication::Sparse(checkpoint) => {
                let started = task_cpu_phase_census::started(
                    self.task_cpu_phase_census.as_ref(),
                    pending.cpu_phase_attributed,
                );
                let payloads = if shared_copyback_payloads_enabled() {
                    checkpoint.shared_payloads().collect()
                } else {
                    checkpoint.payloads().map(Arc::<[u8]>::from).collect()
                };
                task_cpu_phase_census::record_started(
                    self.task_cpu_phase_census.as_mut(),
                    task_cpu_phase_census::Phase::GuestPayloadMaterialization,
                    started,
                );
                return payloads;
            }
            PendingColorPublication::Full(_) => {}
        }
        let initialized = pending.color.full();
        let key = initialized.key();
        let base = key.address().get();
        let buffer = initialized.device_bytes().device_bytes();
        pending
            .guest_writes
            .iter()
            .map(|write| {
                let fn64_render_ir::ResourceRegion::Rdram { range, .. } = write.access().region()
                else {
                    panic!(
                        "a staged guest render-target write must name an RDRAM region; \
                         fill_completed_writes rejected every other kind when it built this list"
                    );
                };
                let start = (range.start().get() - base) as usize;
                let end = start + range.len() as usize;
                buffer
                    .get(start..end)
                    .expect(
                        "every staged write's range lies inside its own color target's \
                         full-extent buffer -- fill_completed_writes sliced these same \
                         bounds to compute the digests",
                    )
                    .into()
            })
            .collect()
    }

    fn take_raw_dpc_visual_target_snapshot(
        &mut self,
        submission: fn64_render_ir::SubmissionIdentity,
    ) -> Result<
        fn64_render::RawDpcVisualTargetSnapshotV1,
        fn64_render::RawDpcVisualTargetSnapshotRefusal,
    > {
        let Some((expected, marker)) = self.last_published_visual_target.take() else {
            return Err(fn64_render::RawDpcVisualTargetSnapshotRefusal::NoPublishedColorTarget);
        };
        if expected != submission {
            return Err(fn64_render::RawDpcVisualTargetSnapshotRefusal::SubmissionMismatch);
        }
        let key = match marker {
            PublishedVisualTargetMarker::Exact(key) => key,
            PublishedVisualTargetMarker::NoColorTarget => {
                return Err(fn64_render::RawDpcVisualTargetSnapshotRefusal::NoPublishedColorTarget)
            }
            PublishedVisualTargetMarker::ComputeCoverageUnavailable => {
                return Err(
                    fn64_render::RawDpcVisualTargetSnapshotRefusal::ComputeCoverageUnavailable,
                )
            }
        };
        let resident = self
            .color_targets
            .as_ref()
            .and_then(|registry| {
                registry
                    .residents()
                    .iter()
                    .find(|resident| resident.key() == key)
            })
            .ok_or(fn64_render::RawDpcVisualTargetSnapshotRefusal::NoPublishedColorTarget)?;
        resident.visual_snapshot(submission)
    }

    fn publish_raw_dpc(
        &mut self,
        publication: ReadyRawDpcCommitCapsule<'_>,
    ) -> CommittedRawDpcOutcome {
        // Taken unconditionally: a stale token from an earlier submission
        // must never survive into a later one. Its
        // `InitializedCandidateColorTarget` is simply dropped, leaving the
        // registry at its prior generation -- the loud-rejection policy's
        // "nothing published" outcome, reached by construction rather than
        // by a rollback step.
        let submission = publication.submission();
        let pending = if self
            .task_batch_pending_fill_publications
            .front()
            .is_some_and(|pending| pending.submission == submission)
        {
            self.task_batch_pending_fill_publications.pop_front()
        } else {
            self.pending_fill_publication.take()
        };
        // Published after `commit()`, deliberately: `ReadyPublication::commit`
        // is the documented straight-line, infallible body, and
        // `prepare_publication` has already asserted every queue/submission/
        // retirement identity fact. Advancing the registry only after that
        // means a resident generation exists only for a submission that
        // genuinely reached `Published`. The one residual window is a panic
        // between the two; on that path the process is already unwinding and
        // the token has been taken, so the registry stays at its prior
        // generation -- the correct outcome, not a leak.
        let outcome = self.coordinator.prepare_publication(publication).commit();
        let mut task_cpu_phase_census = self.task_cpu_phase_census.take();

        let visual_marker = if let Some(pending) = pending {
            assert_eq!(
                pending.submission, submission,
                "publish_raw_dpc received a capsule for a different submission than the one \
                 execute_raw_dpc staged a color-target write for"
            );
            let registry = self
                .color_targets
                .as_mut()
                .expect("a staged fill publication implies the registry was built");
            let target_key = match &pending.color {
                PendingColorPublication::Full(initialized) => initialized.key(),
                PendingColorPublication::Sparse(checkpoint) => checkpoint.key(),
            };
            let exact_physical_coverage = pending.exact_physical_coverage;
            match pending.color {
                PendingColorPublication::Full(initialized) => {
                    registry
                        .prepare_publication(initialized)
                        .unwrap_or_else(|error| {
                            panic!("color-target publication rejected after guest commit: {error}")
                        })
                        .publish();
                }
                PendingColorPublication::Sparse(checkpoint) => {
                    let started = task_cpu_phase_census::started(
                        task_cpu_phase_census.as_ref(),
                        pending.cpu_phase_attributed,
                    );
                    registry
                        .commit_sparse_checkpoint(checkpoint)
                        .unwrap_or_else(|error| {
                            panic!("sparse color-target publication rejected after guest commit: {error}")
                        });
                    task_cpu_phase_census::record_started(
                        task_cpu_phase_census.as_mut(),
                        task_cpu_phase_census::Phase::SparsePublication,
                        started,
                    );
                }
            }
            if exact_physical_coverage {
                PublishedVisualTargetMarker::Exact(target_key)
            } else {
                PublishedVisualTargetMarker::ComputeCoverageUnavailable
            }
        } else {
            PublishedVisualTargetMarker::NoColorTarget
        };
        self.last_published_visual_target = Some((submission, visual_marker));
        self.task_cpu_phase_census =
            task_cpu_phase_census.and_then(task_cpu_phase_census::Task::publication_finished);
        outcome
    }
}

impl SettingsSink for WgpuBackend {}
