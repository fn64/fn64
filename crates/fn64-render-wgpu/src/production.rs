//! T3 Phase B: the concrete `WgpuBackend` production raw-DPC seam.
//!
//! T0 (`fn64-render`) froze the sealed session/coordinator typestate seam
//! and its generic physical-state coordinator; T1
//! (`crate::raw_dpc::production_adapter`) froze the private-decoder ->
//! neutral-plan push loop; T3 Phase A
//! (`crate::PendingTmemTransaction::into_physical_successor`) froze the
//! inactive-successor construction `RawDpcCoordinator::complete_execution`'s
//! `next_physical` parameter needs. This module is the remaining piece: a
//! concrete, object-safe `WgpuBackend` that owns a
//! `fn64_render::RawDpcCoordinator<PhysicalTmemState>`, plans through T1's
//! adapter, executes a sealed `BoundSubmittedRawDpc` using only its
//! authority-scoped `execution_view` (never a bare ticket), and publishes
//! through exactly `self.coordinator.prepare_publication(publication).commit()`.
//!
//! Scope: TMEM loads, admitted state/triangle commands, and fill-cycle
//! `FillRectangle` color-target writes; no FullSync. No ABI/T4 ingress, no
//! visible presentation, no raster parity, no native GPU. This backend's
//! `process_task`/`present` are honest, named rejections -- this slice
//! proves the raw-DPC production seam, not general gfx-task execution.
//!
//! **Guest-write boundary.** An admitted `FillRectangle` declares
//! guest-visible `RenderTarget` *journal* writes and commits them through
//! `RawDpcAbiSession::commit_guest_render_target_writes`. Nothing **in this
//! crate** modifies guest RDRAM, and that remains true after the copyback
//! landed: `execute_fill_rectangle` still produces an owned `Vec<u8>`,
//! `ResidentPublication::publish` still writes into a backend-local `Vec`,
//! and a `CompletedWrite` is still a range plus a content digest, never
//! bytes in motion.
//!
//! What changed is that this backend now also *hands over* those bytes, on
//! request, through [`RenderBackend::committed_guest_render_target_bytes`].
//! It does not write them. The RDRAM copy is performed by `fn64-abi`'s
//! `task_dispatch::rsp_commit::copy_committed_guest_writes`, strictly after
//! the guest commit succeeded, and it re-derives each committed
//! `ContentDigest` from these bytes before writing any of them. Code here
//! may be described as "producing the bytes a committed write names", never
//! as "publishing to guest memory".

use fn64_render::{
    BackendPreparedRawDpc, BoundSubmittedRawDpc, CommittedRawDpcOutcome, ExactRawDpcPlanVisitor,
    PlannedRawDpcSubmission, RawDpcAbiSession, RawDpcCoordinator, RawDpcExecutionBatch,
    RawDpcExecutionView, RawDpcIrCapability, RawDpcPlanRequest, RawDpcSemanticCommandRef,
    RawDpcBackend, RawDpcTaskBatchCapability, RdpStateCommand, RdpTriangleCommand,
    ReadyRawDpcCommitCapsule, RenderBackend, RenderConfig, RenderError, SettingsSink,
    TmemLoadSemantics, TmemLoadShape, TriangleSource,
};
use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendEffectReport, CapturedGuestRead, CompletedWrite,
    DecodedTicket, DeferredBackendEffectReport, DeferredGuestRead, FastContentDigest,
    PhysicalRange, ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion,
    SubmittedTicket, TicketAuthoritySet, ValidationError, WorkloadAdmission, WorkloadPacket,
};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::knobs::ProbePolicy;
use crate::raw_dpc::push_planning_decoded_raw_dpc;
use crate::targets::{
    admitted_triangle_fixture, execute_combined_fill_rectangle_owned, execute_fill_rectangle_owned,
    CandidateColorTarget, ColorCoverageState, ColorTargetExecutionBatch, CompletedColorTargetWrite,
    CompletedTaskColorSegment, ComputeCoverageTriangle, ComputeHotColorBatch,
    ComputeHotColorDispatch, ComputeRasterAdmissionRefusal, ComputeRasterBatch,
    ComputeRasterBatchBuilder, ComputeRasterDrawAdmission, ComputeRasterProgramKey,
    OrderedCpuCandidateReservation, OrderedCpuColorContinuity, ResolvedFragmentBlendParams,
    SparseInitializedColorCheckpoint, TargetRectangle, TaskColorInput,
};
use crate::tmem::{
    project_committed_tmem, DeferredPhysicalTmemSuccessor, TileBindingParams, TmemGpuProjection,
};
#[cfg(test)]
use crate::RawDpcDecodeError;
use crate::{
    AlphaCompare, BlendColorInput, BlendModeState, Color4, ColorImage, ColorTargetExtent,
    ColorTargetFormat, ColorTargetKey, ColorTargetRegistry, CombineParams, CycleType, FillColor,
    FillExecutionError, FillRectangle, HeadlessBackend, InitializedCandidateColorTarget,
    MissingTriangleDrawState, OtherMode, PhysicalTmemError, PhysicalTmemPacketTransaction,
    PhysicalTmemState, PrimColor, RdpState, ResolvedBlendCycle, RetrievedTriangleDraw, TargetError,
    TmemLoadSourceIdentity, TmemTransferWord, TriangleDrawOutput, TrianglePipelineDeviceOutcome,
    TrianglePipelineError, TrianglePipelineRenderer, TriangleRasterParams, TriangleTargetExtent,
    UninitializedTrianglePipeline, TMEM_SAMPLE_STATUS_OK,
};


/// Executes the fill at `index` of the plan's own fill list against the
/// accumulated buffer, returning its completion and its declared accesses.
fn execute_scheduled_fill(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: usize,
    accumulated: Option<Vec<u8>>,
) -> Result<(CompletedColorTargetWrite, Vec<ResourceAccess>), WgpuRawDpcExecutionError> {
    let (
        _,
        fill,
        fill_other_mode,
        fill_scissor,
        fill_combine,
        fill_env_color,
        fill_prim_color,
        fill_blend_color,
        fill_fog_color,
    ) = &collector.plan.fills[index];
    let fill_seed_access = &fill.seed_access_index;

    // This fill's OWN `OtherMode`, snapshotted at its stream position --
    // not the walk's running value, which a later `SetOtherMode` (a
    // following texture rectangle switching to Copy cycle, say) would have
    // already overwritten with a mode the fill never ran under.
    let Some(other_mode) = *fill_other_mode else {
        return Err(WgpuRawDpcExecutionError::FillExecution(
            FillExecutionError::NotFillCycle,
        ));
    };

    // **The same scissor the texrect path already honours, from the same
    // latched state and through the same whole-target fallback.**
    // Pinned RT64 clips by intersecting its current scissor and draw
    // rectangle (`src/hle/rt64_rdp.cpp:1214-1223`, commit `f0728a2`). Reusing
    // `texrect_scissor_or_full_target` rather than writing a second
    // fallback keeps one model of "no SetScissor means the whole target".
    let scissor = texrect_scissor_or_full_target(*fill_scissor, candidate.key().extent());

    // **The seed for a partial fill's untouched pixels.**
    //
    // Preference order, and each rung is a different fact rather than a
    // fallback chain looking for something that works:
    //
    // 1. `accumulated` -- an earlier command in THIS packet already
    //    composed into the buffer, so it is the freshest content and
    //    already in device byte order.
    // 2. the declared colour-image seed read -- the guest's own framebuffer
    //    bytes, which is what hardware leaves outside a fill and what
    //    `fn64-render-reference` loads (`backend/imp.rs:440-447`).
    // 3. `None` -- the fill is full extent and declared no seed, so every
    //    byte comes from the command itself.
    //
    // A declared seed that failed to thread is NOT silently downgraded to
    // `None`: that would resurrect the fabricated zeros this whole path
    // exists to remove, and the differential measured them as
    // `wgpu: 0x0000` against a `0xffff` key. `MissingSeedBytes` names it.
    let seed = match (accumulated, fill_seed_access) {
        (Some(bytes), _) => Some(bytes),
        (None, Some(access_index)) => {
            let access_position = usize::try_from(*access_index).map_err(|_| {
                WgpuRawDpcExecutionError::MissingFillSeedBytes {
                    access_index: *access_index,
                }
            })?;
            let expected = collector
                .plan
                .accesses
                .get(access_position)
                .copied()
                .ok_or(WgpuRawDpcExecutionError::MissingFillSeedBytes {
                    access_index: *access_index,
                })?;
            let captured = collector.reads.bytes(*access_index, expected).ok_or(
                WgpuRawDpcExecutionError::MissingFillSeedBytes {
                    access_index: *access_index,
                },
            )?;
            Some(logical_bytes_from_captured_rdram(captured))
        }
        (None, None) => None,
    };

    let rectangle = FillRectangle::from_wire_fields(
        fill.upper_left_x,
        fill.upper_left_y,
        fill.lower_right_x,
        fill.lower_right_y,
    );
    let completed = match other_mode.cycle_type() {
        CycleType::Fill => execute_fill_rectangle_owned(
            candidate,
            other_mode,
            FillColor::from_wire(
                fill.fill_color
                    .ok_or(FillExecutionError::MissingCombinedState {
                        register: "SetFillColor",
                    })?
                    .value,
            ),
            rectangle,
            scissor,
            seed,
        )?,
        CycleType::OneCycle | CycleType::TwoCycle => execute_combined_fill_rectangle_owned(
            candidate,
            other_mode,
            (*fill_combine).ok_or(FillExecutionError::MissingCombinedState {
                register: "SetCombine",
            })?,
            *fill_env_color,
            *fill_prim_color,
            *fill_blend_color,
            *fill_fog_color,
            rectangle,
            scissor,
            seed,
        )?,
        CycleType::Copy => {
            return Err(WgpuRawDpcExecutionError::FillExecution(
                FillExecutionError::UnsupportedCombinedCycle {
                    cycle_type: CycleType::Copy,
                },
            ))
        }
    };
    let accesses = fill_accesses(&collector.plan.accesses, fill)?.to_vec();
    Ok((completed, accesses))
}

fn decode_scheduled_raw_triangle(
    collector: &ExecutionCollector<'_>,
    index: CommandIndex,
) -> Result<crate::raw_dpc::RawTriangle, WgpuRawDpcExecutionError> {
    let scheduled = &collector.plan.raw_triangle_commands[index];
    decoded_scheduled_raw_triangle(scheduled)
}

fn decoded_scheduled_raw_triangle(
    scheduled: &ScheduledRawTriangle,
) -> Result<crate::raw_dpc::RawTriangle, WgpuRawDpcExecutionError> {
    scheduled.decoded.map_err(
        |_| WgpuRawDpcExecutionError::RawTriangleWireWordsUndecodable {
            triangle_index: scheduled.triangle_index.get(),
        },
    )
}

fn scheduled_raw_triangle_accesses(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: CommandIndex,
) -> Result<Vec<ResourceAccess>, WgpuRawDpcExecutionError> {
    let scheduled = &collector.plan.raw_triangle_commands[index];
    let start = scheduled.span.first_access_index as usize;
    let end = start
        .checked_add(scheduled.span.access_count as usize)
        .ok_or(WgpuRawDpcExecutionError::RawTriangleDeclaredNoWrite {
            triangle_index: scheduled.triangle_index.get(),
        })?;
    let accesses = collector
        .plan
        .accesses
        .get(start..end)
        .filter(|slice| !slice.is_empty())
        .ok_or(WgpuRawDpcExecutionError::RawTriangleDeclaredNoWrite {
            triangle_index: scheduled.triangle_index.get(),
        })?
        .to_vec();
    verify_accesses_inside(&accesses, candidate.key())?;
    Ok(accesses)
}

/// Executes the flat raw triangle at `index` of the plan's own
/// `raw_triangle_commands` list against the accumulated buffer, returning
/// its completion and its declared accesses.
///
/// Every geometric fact is taken from the decoder, never re-derived: the
/// exact edge coefficients carried from the command's own authoritative wire
/// words, and the declared write run from the span the decoder recorded when
/// it pushed those accesses. The one number this function computes itself is
/// the declared row COUNT, which it hands the executor so the executor can
/// prove its own raster covers exactly those rows.
#[allow(clippy::too_many_arguments)]
fn execute_scheduled_raw_triangle<S: crate::TmemByteSource + ?Sized>(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: CommandIndex,
    resident_bytes: Cow<'_, [u8]>,
    tmem: &S,
    expect_proposed: bool,
    depth_cells: Option<&mut [crate::targets::DepthCell]>,
    coverage: Option<ColorCoverageState>,
) -> Result<(CompletedColorTargetWrite, Vec<ResourceAccess>), WgpuRawDpcExecutionError> {
    let scheduled = &collector.plan.raw_triangle_commands[index];
    let triangle_index = scheduled.triangle_index;
    let draw_state = collector.plan.triangles[triangle_index]
        .draw
        .as_ref()
        .map_err(|missing| WgpuRawDpcExecutionError::MissingTriangleDrawState(*missing))?;

    // **Decoded once from this command's own authoritative wire words**, then
    // carried exactly rather than reconstructed from the projected
    // `NeutralTriangleVertex` triple. The projection is screen-space and
    // lossy; it cannot recover dxhdy/dxmdy/dxldy at all.
    let triangle = decode_scheduled_raw_triangle(collector, index)?;
    let accesses = scheduled_raw_triangle_accesses(collector, candidate, index)?;

    // **The tile binding, for a TEXTURED triangle only, resolved from the
    // triangle's OWN wire tile field.**
    //
    // `RawTriangle::tile()` is wire word 0 bits 18:16 -- a real field the
    // triangle carries. `PlanCollector`'s `bound_tile_index` reads the same
    // field from the same word for the GPU uniform path, so the two paths
    // agree by construction; reading it here from the decoded command is the
    // same shape `execute_scheduled_texrect` already uses for a texrect,
    // which reads its own tile from its own wire word rather than the
    // uniform.
    //
    // The tile TABLE is `triangle_neutral_tiles`, the snapshot taken at this
    // triangle's own stream position -- so a packet that re-tiles between
    // draws gives each draw its own binding. Absent means no `SetTile`/
    // `SetTileSize` was staged for that index at this position: refused by
    // name, never defaulted to a zeroed tile that would silently sample TMEM
    // word zero.
    let texture = if triangle.flags().textured() {
        let tile_index = usize::from(triangle.tile().get());
        let (Some(descriptor), Some(size)) =
            collector.plan.triangles[triangle_index].neutral_tiles[tile_index]
        else {
            return Err(WgpuRawDpcExecutionError::TexrectUnboundTile {
                triangle_index: triangle_index.get(),
            });
        };
        let tile = crate::targets::TexrectTileBinding::try_from_neutral(descriptor, size)
            .map_err(|_| WgpuRawDpcExecutionError::TexrectUnboundTile {
                triangle_index: triangle_index.get(),
            })?;
        // High bit 15 is `en_tlut`, bit 14 `tlut_type`; with the enable bit
        // clear fn64 treats the TLUT as off. Pinned RT64 likewise maps only
        // the exact `G_TT_RGBA16` and `G_TT_IA16` values to a TLUT and maps
        // every other value to `None`
        // (`src/hle/rt64_rdp_tmem.cpp:176-185`, commit `f0728a2`).
        let lut_mode = draw_state.other_mode.texture_lut_mode();
        // The image this call was handed must answer the identity its CALLER
        // selected: a pending post-image answers `Proposed`, durable state
        // answers `Committed`. Checked here rather than trusted, exactly as
        // `execute_scheduled_texrect` checks it and for the same reason --
        // both variants inhabit one enum, so a wrong `snapshot()` impl
        // compiles.
        verify_tmem_identity(tmem, expect_proposed, triangle_index.get())?;
        Some(crate::targets::RawTriangleTexture {
            tile,
            tmem,
            lut_mode,
        })
    } else {
        None
    };

    let shading = crate::targets::TexrectShading::new(
        draw_state.combine_params,
        draw_state.env_color,
        draw_state.prim_color,
    );
    let blend_registers =
        crate::targets::TexrectBlendRegisters::new(draw_state.blend_color, draw_state.fog_color);
    // **The z-buffer wiring, present only when a depth accumulator was
    // threaded (i.e. this packet bound a z-image).** The compare/update
    // gates and the z source are read from THIS draw's own snapshotted
    // `OtherMode`, and the primitive depth from its snapshotted
    // `SetPrimDepth` -- both the same command-time-snapshot values every
    // other register on `draw_state` uses. Absent depth cells means no
    // z-image was bound, so the draw resolves by painter's order exactly
    // as before this change.
    let depth = depth_cells.map(|cells| crate::targets::RawTriangleDepth {
        cells,
        compare: draw_state.other_mode.depth_compare_enabled(),
        update: draw_state.other_mode.depth_update_enabled(),
        mode: draw_state.other_mode.depth_mode(),
        source_is_primitive: draw_state.other_mode.primitive_depth_source(),
        prim_depth: draw_state.prim_depth,
    });
    let completed = match coverage {
        Some(coverage) => crate::targets::execute_raw_triangle_with_coverage(
            candidate,
            draw_state.other_mode,
            &triangle,
            shading,
            blend_registers,
            resident_bytes,
            &accesses,
            texture,
            depth,
            coverage,
        )?,
        None => crate::targets::execute_raw_triangle(
            candidate,
            draw_state.other_mode,
            &triangle,
            shading,
            blend_registers,
            resident_bytes,
            &accesses,
            texture,
            depth,
        )?,
    };
    Ok((completed, accesses))
}

/// Executes the texrect at `index` of the plan's own texrect list against
/// the accumulated buffer, returning its completion and declared accesses.
///
/// Every geometric fact is taken from the decoder, never re-derived: the
/// pixel extent from the `RectViewportPixels` `texture_rectangle_vertices`
/// produced, and the declared write run from the span the decoder recorded
/// when it pushed those accesses.
#[allow(clippy::too_many_arguments)]
fn execute_scheduled_texrect<S: crate::TmemByteSource + ?Sized>(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    tmem: &S,
    expect_proposed: bool,
    index: usize,
    resident_bytes: Cow<'_, [u8]>,
    already_initialized: Option<TargetRectangle>,
) -> Result<(CompletedColorTargetWrite, Vec<ResourceAccess>), WgpuRawDpcExecutionError> {
    let (span, triangle_index, tile_index, _, flipped_axes) =
        collector.plan.texrect_commands[index];
    // A texrect that declared no write must not execute: it would write
    // bytes the journal never declared, which `merged_fill_and_tmem_writes`
    // would then reject as `MergedWriteUndeclared` -- a correct but less
    // specific diagnosis than naming the real cause here.
    let span = span.ok_or(WgpuRawDpcExecutionError::TexrectDeclaredNoWrite { triangle_index })?;

    let draw_state = collector.plan.triangles[triangle_index]
        .draw
        .as_ref()
        .map_err(|missing| WgpuRawDpcExecutionError::MissingTriangleDrawState(*missing))?;
    let viewport = draw_state
        .viewport
        .ok_or(WgpuRawDpcExecutionError::TexrectMissingViewport { triangle_index })?;
    let other_mode = draw_state.other_mode;

    // The complete neutral tile pair, not the GPU `TileBindingParams`,
    // because the CPU reader's indexed path needs `palette` and that
    // uniform layout has no such field. Absent means no `SetTile`/
    // `SetTileSize` was staged at this texrect's own stream position --
    // refused by name, never defaulted to a zeroed tile that would
    // silently sample TMEM word zero.
    let (Some(descriptor), Some(size)) =
        collector.plan.triangles[triangle_index].neutral_tiles[usize::from(tile_index)]
    else {
        return Err(WgpuRawDpcExecutionError::TexrectUnboundTile { triangle_index });
    };
    let tile = crate::targets::TexrectTileBinding::try_from_neutral(descriptor, size)
        .map_err(|_| WgpuRawDpcExecutionError::TexrectUnboundTile { triangle_index })?;

    // The two opposite texcoord corners `texture_rectangle_vertices`
    // produced. Its six-vertex texcoord order is
    // `[u1v1, u2v1, u1v2, u2v2, u2v1, u1v2]` (the unflipped arm of its own
    // `triTcFloats` push order), and `production_adapter` splits those six
    // into triangles `0,1,2` and `3,4,5`. So the upper-left pair `(u1, v1)`
    // is vertex 0 = first triangle's index 0, and the lower-right pair
    // `(u2, v2)` is vertex **3** = the SECOND triangle's index **0**.
    //
    // Not index 2 of the second triangle: that is vertex 5, whose texcoord
    // is `(u1, v2)` -- the lower-LEFT corner. Measured, not reasoned:
    // reading index 2 gave an S span of 0 (`lr = [0.0, 0.75]`), every pixel
    // in a row sampled texel 0, and the committed-TMEM oracle disagreed at
    // the first pixel of row 1.
    let upper_left = draw_state.vertices[0].texcoord;
    let second = collector.plan.triangles[triangle_index + 1]
        .draw
        .as_ref()
        .map_err(|missing| WgpuRawDpcExecutionError::MissingTriangleDrawState(*missing))?;
    let lower_right = second.vertices[0].texcoord;
    let mut draw = crate::targets::TexrectDraw::try_from_viewport_and_texcoords(
        viewport,
        upper_left,
        lower_right,
    )?;
    if flipped_axes {
        draw = draw.with_flipped_axes();
    }

    // Locate the declared run by the decoder's own recorded span.
    let start = span.first_access_index as usize;
    let end = start
        .checked_add(span.access_count as usize)
        .ok_or(WgpuRawDpcExecutionError::TexrectDeclaredNoWrite { triangle_index })?;
    let accesses = collector
        .plan
        .accesses
        .get(start..end)
        .filter(|slice| !slice.is_empty())
        .ok_or(WgpuRawDpcExecutionError::TexrectDeclaredNoWrite { triangle_index })?
        .to_vec();

    // Cross-check, not an assumption: every declared access must fall
    // inside the candidate key's own range, which was derived from the
    // packet's `SetColorImage` by a path independent of the decoder's
    // `plan_render_target_rows`.
    let key = candidate.key();
    verify_accesses_inside(&accesses, key)?;

    // High bit 15 is `en_tlut`, bit 14 `tlut_type`; with the enable bit clear
    // fn64 treats the TLUT as off. Pinned RT64 likewise maps only the exact
    // `G_TT_RGBA16` and `G_TT_IA16` values to a TLUT and maps every other
    // value to `None` (`src/hle/rt64_rdp_tmem.cpp:176-185`, commit
    // `f0728a2`).
    let lut_mode = other_mode.texture_lut_mode();
    // **The committed/pending distinction, asserted where it is crossed.**
    //
    // The image this call was handed must answer the identity its *caller*
    // selected, not merely a well-formed one: a pending post-image answers
    // `TmemSnapshotIdentity::Proposed`, durable state answers `Committed`.
    // Checked here rather than trusted, because the type system cannot:
    // both variants inhabit one enum, so a wrong `snapshot()` impl
    // compiles. Measured: forging `Committed` in `PendingTmemImage`'s impl
    // passed the entire suite before this check existed.
    //
    // Both directions are checked, not just one. A committed image reaching
    // the load-bearing packet's arm would mean the texrect silently missed
    // its own packet's loads; a proposed image reaching the load-free arm
    // would mean a proposal was fabricated for a packet that staged none.
    verify_tmem_identity(tmem, expect_proposed, triangle_index)?;
    // The one-cycle shading state, taken from the SAME
    // `RetrievedTriangleDraw` snapshot the other-mode, viewport and tile
    // above came from -- i.e. the registers current at THIS texrect's own
    // stream position. That per-command sourcing is what lets a packet
    // carry several one-cycle texrects each running a different combiner
    // program against the accumulated buffer: the schedule loop calls this
    // function once per texrect, and each call reads its own snapshot
    // rather than the walk's running final value.
    //
    // `combine_params` is non-optional on that struct (a triangle with no
    // `SetCombine` fails retrieval with `MissingTriangleDrawState` before
    // reaching here); `env_color`/`prim_color` are `Option` because their
    // registers may genuinely be unset, and the executor refuses only when
    // the program actually reads the unset one -- and only in one-cycle,
    // since Copy consults no program at all.
    let shading = crate::targets::TexrectShading::new(
        draw_state.combine_params,
        draw_state.env_color,
        draw_state.prim_color,
    );
    // The blender's two color registers, taken from the SAME
    // `RetrievedTriangleDraw` snapshot the combiner's registers above came
    // from -- i.e. the values current at THIS texrect's own stream
    // position, not the walk's running final value.
    let blend_registers =
        crate::targets::TexrectBlendRegisters::new(draw_state.blend_color, draw_state.fog_color);
    // The scissor current at THIS texrect's own stream position, from the
    // same `RetrievedTriangleDraw` snapshot as the registers above.
    //
    // **The fallback when the plan issued no `SetScissor` is the whole
    // colour target, not an empty rect and not a refusal.** The RDP's clip
    // registers hold a value from reset onward; a display list that never
    // writes them draws unscissored, and refusing it here would refuse
    // every stream that relies on the boot-time rect. The target extent is
    // this consumer's own honest widest bound -- which is exactly why
    // `RetrievedTriangleDraw::scissor` leaves the fallback to the consumer
    // instead of defaulting in the collector, which does not know it.
    //
    // Quarter-pixels, because public libultra's `gDPSetScissor` encodes each
    // coordinate after multiplying it by four
    // (`include/ultra64/gbi.h:4794-4804`); `extent` is in pixels, so it is
    // scaled by four.
    let scissor = texrect_scissor_or_full_target(draw_state.scissor, candidate.key().extent());
    let completed = crate::targets::execute_texture_rectangle(
        candidate,
        other_mode,
        draw,
        tile,
        tmem,
        lut_mode,
        shading,
        blend_registers,
        scissor,
        resident_bytes,
        already_initialized,
    )?;
    Ok((completed, accesses))
}

/// The scissor one texrect is clipped against: the rect latched at its own
/// stream position, or the whole colour target when the plan has issued
/// none.
///
/// **The fallback is the target's own extent, not an empty rect and not a
/// refusal.** The RDP's clip registers hold a value from reset onward, so a
/// display list that never writes them draws unscissored; refusing such a
/// stream would refuse every one that relies on the boot-time rect. The
/// target extent is this consumer's honest widest bound -- which is exactly
/// why `RetrievedTriangleDraw::scissor` leaves the fallback to the
/// consumer rather than defaulting in the collector, which does not know
/// the extent.
///
/// Quarter-pixels, because public libultra's `gDPSetScissor` encodes each
/// coordinate after multiplying it by four
/// (`include/ultra64/gbi.h:4794-4804`), while `extent` is in pixels -- hence
/// the factor of four on each axis.
///
/// A named function rather than an inline closure so a mutation that
/// derives the height bound from the width is reachable from a unit test;
/// while it was inline, exactly that mutation left the whole suite green.
fn texrect_scissor_or_full_target(
    latched: Option<crate::targets::RdpScissorRect>,
    extent: crate::targets::ColorTargetExtent,
) -> crate::targets::RdpScissorRect {
    latched.unwrap_or_else(|| {
        let quarter = |pixels: u32| u16::try_from(pixels.saturating_mul(4)).unwrap_or(u16::MAX);
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(
            0,
            0,
            0,
            quarter(extent.width()),
            quarter(extent.height()),
        )
    })
}

/// **One TMEM projection per admitted triangle, each taken at that
/// triangle's own stream position, through the same one projection the
/// committed path uses.**
///
/// ## Why per triangle and not per packet
///
/// The GPU half had the CPU half's old defect one layer up: a single
/// projection was sealed per `draw_admitted_triangles` call and shared by
/// every triangle in it. Within one packet TMEM is not one image --
/// WM2000's measured sixth packet interleaves seven `LoadTile`s with seven
/// texrects, all loading from TMEM word zero, so a shared projection holds
/// only the seventh load's texels and the raster draws the seventh sprite
/// seven times.
///
/// The position rule is `prefix_before`, **called here rather than
/// reimplemented**, so the CPU texel reader and the GPU raster cannot
/// disagree about which load a given command observed. Both arms match the
/// CPU side exactly: a prefix when a load precedes the triangle, durable
/// committed state when none does.
///
/// A texture rectangle's two triangles carry the same
/// `plan.triangle_commands` entry, so both halves of one rectangle project
/// the same image and cannot split across a load.
///
/// ## Cost
///
/// Bounded by the triangle count, and `submit_triangles` already creates a
/// TMEM bytes buffer and validity buffer **per fixture**
/// (`triangle_pipeline.rs`'s per-fixture `create_buffer`/`write_buffer`
/// pair), so this changed *what* each fixture uploads, not how much. No
/// pipeline, bind-group, or shader change was required.
///
/// ## What it does not relax
///
/// Every entry is `project_tmem`, at `S = PendingTmemPrefixImage` for the
/// prefix arm and `S = PhysicalTmemState` for the committed one. There is
/// no second address walk, no second validity gate, and no second bitmap
/// packing, and the CPU texel reader reaches the same two sources through
/// the same `TmemByteSource`. A pending/published *disagreement* about how
/// bytes reach the shader is therefore unrepresentable rather than merely
/// tested for -- the only thing that differs between the two calls is which
/// image is handed in.
///
/// **What the published-slot projection was protecting, and how this
/// preserves it.** The `project_committed_tmem(coordinator.physical())`
/// call the pending projection replaced was not arbitrary: reading the active
/// slot guaranteed the GPU could only ever sample bytes some publication had
/// actually durably installed, so no GPU-observed pixel could be attributed
/// to a proposal that publication later rejected. That guarantee is real and
/// it is preserved here in the same way the CPU texel reader preserves it --
/// not by refusing to read pending bytes, but by refusing to let a pending
/// read *claim* to be a durable one. The check below is
/// `execute_scheduled_texrect`'s check, at the other crossing:
///
/// - **No publication.** This borrows the transaction and copies bytes out.
///   It cannot commit, cannot advance a generation, and cannot produce a
///   `PhysicalTmemState`. `into_physical_successor` still runs afterwards
///   with every base-state, generation, epoch and backend-effect check, and
///   the sealed-proposal revalidation remains available diagnostically. If
///   any check rejects, this packet's
///   `execute_raw_dpc` returns `Err` and no draw output is stored -- the
///   pixels never become observable.
/// - **No forged snapshot identity.** Verified, not trusted: both
///   `TmemSnapshotIdentity` variants inhabit one enum, so a wrong
///   `snapshot()` impl compiles. Measured at the sibling site -- forging
///   `Committed` in `PendingTmemImage`'s impl passed the entire suite before
///   `execute_scheduled_texrect`'s equivalent check existed.
/// - **No effect-report participation.** Reading is not a write. Nothing
///   projected here enters `proposed_effects`, so the sealed proposal and
///   `validate_backend_effects`' supersequence walk see exactly what they saw
///   before.
///
/// Nonclaim: the returned `TmemGpuProjection` carries bytes, not identity.
/// It is not a receipt and nothing downstream may read a publication out of
/// it; the identity assertion happens here, at the crossing, and does not
/// travel with the bytes.
fn project_pending_tmem_per_triangle(
    triangle_commands: &[u32],
    prefixes: &[(u32, crate::tmem::TmemPrefixSnapshot)],
    pending: &crate::tmem::PendingTmemTransaction,
    committed: &PhysicalTmemState,
) -> Result<Vec<TmemGpuProjection>, WgpuRawDpcExecutionError> {
    triangle_commands
        .iter()
        .map(
            |&command_index| match prefix_before(prefixes, command_index) {
                Some(prefix) => project_proposed_image(&pending.prefix_image(prefix)?),
                // No load precedes this triangle in its own packet, so durable
                // committed state is what TMEM holds at its position -- the
                // same answer, from the same fact, that `stage_color_commands`
                // gives a texrect in the same position. Not a fallback for a
                // missing image: the absence of a preceding load IS the stream
                // fact that makes committed correct.
                None => Ok(project_committed_tmem(committed)),
            },
        )
        .collect()
}

/// **The committed/pending identity crossing, both directions, at one
/// site.**
///
/// A texrect's TMEM image must answer the identity its caller
/// *selected*, not merely a well-formed one. `expect_proposed` is
/// `TexrectTmemSource`'s own choice, made per packet from
/// `plan.loads.is_empty()`; this checks the image agrees.
///
/// Checked rather than trusted because the type system cannot: `Committed`
/// and `Proposed` inhabit one enum, so a wrong `snapshot()` impl compiles.
/// Measured on the pending direction: forging `Committed` in
/// `PendingTmemImage`'s impl passed the entire suite before that half
/// existed.
///
/// Both directions, not one. A committed image reaching a load-bearing
/// packet would mean the texrect silently missed its own packet's loads --
/// the `TMEM_SAMPLE_STATUS_INVALID_BYTE` defect commit `3a1a6a73` measured.
/// A proposed image reaching a load-free packet would mean a proposal was
/// fabricated for a packet that staged none.
///
/// Split out from `execute_scheduled_texrect` so a lying source can reach
/// it: no real image can, since both real impls are correct, and a refusal
/// with no test is a claim with no evidence.
fn verify_tmem_identity<S: crate::TmemByteSource + ?Sized>(
    tmem: &S,
    expect_proposed: bool,
    triangle_index: usize,
) -> Result<(), WgpuRawDpcExecutionError> {
    let snapshot = crate::TmemByteSource::snapshot(tmem);
    match (expect_proposed, snapshot.is_committed()) {
        (true, true) => {
            Err(WgpuRawDpcExecutionError::PendingTmemImageClaimedCommitted { triangle_index })
        }
        (false, false) => {
            Err(WgpuRawDpcExecutionError::CommittedTmemImageClaimedProposed { triangle_index })
        }
        _ => Ok(()),
    }
}

/// [`project_pending_tmem_per_triangle`]'s per-entry body, generic over the
/// source, so the forgery refusal can be exercised by a test.
///
/// Split out for exactly one reason: `PendingTmemImage`'s own `snapshot()`
/// impl is correct, so no *real* pending image can drive the refusal, and a
/// refusal with no test is a claim with no evidence -- this crate's own
/// convention (see `merged_fill_and_tmem_writes`' two loud arms, tested at
/// the function for the same reason). A test source that answers
/// `Committed` is the only way to reach the arm, and it is reachable only
/// through this generic seam, never from production code: the sole
/// production caller above passes a real `PendingTmemPrefixImage`.
fn project_proposed_image<S: crate::TmemByteSource + ?Sized>(
    image: &S,
) -> Result<TmemGpuProjection, WgpuRawDpcExecutionError> {
    if image.snapshot().is_committed() {
        return Err(WgpuRawDpcExecutionError::PendingTmemProjectionClaimedCommitted);
    }
    Ok(crate::project_tmem(image))
}

/// Every declared access must fall inside the color target's own range.
///
/// The decoder computed those ranges from the same `SetColorImage` by an
/// independent path (`plan_render_target_rows`), so agreement here is real
/// evidence the two derivations match, and disagreement is a loud refusal
/// rather than a write landing outside the target the registry tracks.
fn verify_accesses_inside(
    accesses: &[ResourceAccess],
    key: ColorTargetKey,
) -> Result<(), WgpuRawDpcExecutionError> {
    let target = key.range();
    for access in accesses {
        let inside = match access.region() {
            fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                range.start().get() >= target.start().get()
                    && range.start().get() + range.len() <= target.start().get() + target.len()
            }
            _ => false,
        };
        if !inside {
            return Err(WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                access_index: access.operation().get(),
            });
        }
    }
    Ok(())
}

/// This fill's own declared access run, located by the
/// `first_access_index`/`access_count` span the decoder recorded when it
/// pushed them -- never re-derived from the rectangle's geometry, which
/// would be a second independent derivation of the same fact.
fn fill_accesses<'plan>(
    accesses: &'plan [ResourceAccess],
    fill: &fn64_render::RdpFillRectangleCommand,
) -> Result<&'plan [ResourceAccess], WgpuRawDpcExecutionError> {
    let start = fill.first_access_index as usize;
    let end = start.checked_add(fill.access_count as usize).ok_or(
        WgpuRawDpcExecutionError::FillAccessOutsideTarget {
            access_index: fill.first_access_index,
        },
    )?;
    accesses
        .get(start..end)
        .filter(|slice| !slice.is_empty())
        .ok_or(WgpuRawDpcExecutionError::FillAccessOutsideTarget {
            access_index: fill.first_access_index,
        })
}

/// Derives the exact ordered `CompletedWrite` list for one admitted
/// FillRectangle, one per `ResourceAccess` the decoder declared for it.
///
/// Each write's `byte_count` is its own access's
/// `region().declared_bytes()` and its `content` digest covers exactly those
/// bytes, sliced out of the full-extent `DeviceColorBytes` the executor
/// produced. Deliberately **not** one digest over the whole buffer: a
/// `CompletedWrite` claims "these declared_bytes at this range now hold
/// content with this digest", and hashing bytes outside the declared range
/// would make the claim unfalsifiable for the range it names while silently
/// importing untouched inter-row bytes into the hash of a range that does
/// not contain them.
///
/// The full-extent buffer is the *source*; each per-row access's own
/// declared span is the *unit*. Those are two different invariants -- the
/// registry needs a complete byte buffer for the resident generation, the
/// journal needs exactly the bytes the fill claims -- and this function is
/// where they are correctly decoupled.
fn fill_completed_writes(
    key: ColorTargetKey,
    device_bytes: &crate::DeviceColorBytes,
    accesses: &[ResourceAccess],
) -> Result<Vec<CompletedWrite>, WgpuRawDpcExecutionError> {
    let base = key.address().get();
    let buffer = device_bytes.device_bytes();
    accesses
        .iter()
        .map(|access| {
            let range = match access.region() {
                fn64_render_ir::ResourceRegion::Rdram { range, .. } => range,
                _ => {
                    return Err(WgpuRawDpcExecutionError::FillAccessRegionKind {
                        access_index: access.operation().get(),
                    })
                }
            };
            // Physical -> buffer-relative. The access range is a subrange of
            // the target's own range by construction (both derive from the
            // same `SetColorImage` address/width), but that is re-checked
            // here rather than assumed.
            let start = range.start().get().checked_sub(base).ok_or(
                WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                    access_index: access.operation().get(),
                },
            )? as usize;
            let len = range.len() as usize;
            let slice = start
                .checked_add(len)
                .and_then(|end| buffer.get(start..end))
                .ok_or(WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                    access_index: access.operation().get(),
                })?;
            CompletedWrite::try_from_bytes(*access, slice).map_err(WgpuRawDpcExecutionError::Effect)
        })
        .collect()
}

fn image_format(format: fn64_render::NeutralImageFormat) -> crate::ImageFormat {
    match format {
        fn64_render::NeutralImageFormat::Rgba => crate::ImageFormat::Rgba,
        fn64_render::NeutralImageFormat::Yuv => crate::ImageFormat::Yuv,
        fn64_render::NeutralImageFormat::ColorIndex => crate::ImageFormat::ColorIndex,
        fn64_render::NeutralImageFormat::IntensityAlpha => crate::ImageFormat::IntensityAlpha,
        fn64_render::NeutralImageFormat::Intensity => crate::ImageFormat::Intensity,
    }
}

/// `image_format`'s inverse: the neutral mirror of a crate-typed
/// [`crate::ImageFormat`].
///
/// Exists so `PlanCollector`'s tile table can be seeded from `RdpState`'s
/// durable, crate-typed `TileState` while the table itself keeps storing
/// neutral mirrors -- the shape both its consumers
/// (`TileBindingParams::from_neutral` for the GPU uniform,
/// `TexrectTileBinding::try_from_neutral` for the CPU reader) already read.
/// Converting the *seed* is a five-function addition; converting the table
/// would mean a new typed-to-uniform path for the GPU binding, which is a
/// different change.
fn neutral_image_format(format: crate::ImageFormat) -> fn64_render::NeutralImageFormat {
    match format {
        crate::ImageFormat::Rgba => fn64_render::NeutralImageFormat::Rgba,
        crate::ImageFormat::Yuv => fn64_render::NeutralImageFormat::Yuv,
        crate::ImageFormat::ColorIndex => fn64_render::NeutralImageFormat::ColorIndex,
        crate::ImageFormat::IntensityAlpha => fn64_render::NeutralImageFormat::IntensityAlpha,
        crate::ImageFormat::Intensity => fn64_render::NeutralImageFormat::Intensity,
    }
}

/// `pixel_size`'s inverse; see [`neutral_image_format`].
fn neutral_pixel_size(size: crate::PixelSize) -> fn64_render::NeutralPixelSize {
    match size {
        crate::PixelSize::Bits4 => fn64_render::NeutralPixelSize::Bits4,
        crate::PixelSize::Bits8 => fn64_render::NeutralPixelSize::Bits8,
        crate::PixelSize::Bits16 => fn64_render::NeutralPixelSize::Bits16,
        crate::PixelSize::Bits32 => fn64_render::NeutralPixelSize::Bits32,
    }
}

/// The neutral mirror of one durable [`crate::TileDescriptor`], field for
/// field. The inverse of `TexrectTileBinding::try_from_neutral`'s own
/// decode, and deliberately total: every field on the neutral mirror has
/// exactly one accessor on the typed descriptor, so there is nothing to
/// default and nothing to drop.
fn neutral_tile_descriptor(
    descriptor: crate::TileDescriptor,
) -> fn64_render::NeutralTileDescriptor {
    fn64_render::NeutralTileDescriptor {
        format: neutral_image_format(descriptor.format()),
        size: neutral_pixel_size(descriptor.size()),
        line_words: descriptor.line_words(),
        tmem_word_address: descriptor.tmem().get(),
        palette: descriptor.palette(),
        s_mode: fn64_render::NeutralTileAddressMode {
            mirror: descriptor.s_mode().mirror(),
            clamp: descriptor.s_mode().clamp(),
        },
        mask_s: descriptor.mask_s(),
        shift_s: descriptor.shift_s(),
        t_mode: fn64_render::NeutralTileAddressMode {
            mirror: descriptor.t_mode().mirror(),
            clamp: descriptor.t_mode().clamp(),
        },
        mask_t: descriptor.mask_t(),
        shift_t: descriptor.shift_t(),
    }
}

/// The neutral mirror of one durable [`crate::TileSize`], in the same raw
/// 10.2 fixed-point encoding the neutral struct documents.
fn neutral_tile_size(size: crate::TileSize) -> fn64_render::NeutralTileSize {
    fn64_render::NeutralTileSize {
        low_s: size.low_s().raw(),
        low_t: size.low_t().raw(),
        high_s: size.high_s().raw(),
        high_t: size.high_t().raw(),
    }
}

/// `RdpState`'s eight durable tile registers as the neutral pair
/// `PlanCollector` tracks, for seeding one packet's walk.
///
/// A tile with no `SetTile` or no `SetTileSize` ever issued stays `None` on
/// that half -- absence is carried through, never defaulted to a zeroed
/// descriptor that would silently sample TMEM word zero.
fn durable_neutral_tiles(
    state: &RdpState,
) -> [(
    Option<fn64_render::NeutralTileDescriptor>,
    Option<fn64_render::NeutralTileSize>,
); 8] {
    let mut tiles = [(None, None); 8];
    for (index, slot) in tiles.iter_mut().enumerate() {
        let Ok(tile_index) = crate::TileIndex::try_new(index as u8) else {
            continue;
        };
        let tile = state.tmem().tile(tile_index);
        slot.0 = tile.descriptor().map(neutral_tile_descriptor);
        slot.1 = tile.size().map(neutral_tile_size);
    }
    tiles
}

fn pixel_size(size: fn64_render::NeutralPixelSize) -> crate::PixelSize {
    match size {
        fn64_render::NeutralPixelSize::Bits4 => crate::PixelSize::Bits4,
        fn64_render::NeutralPixelSize::Bits8 => crate::PixelSize::Bits8,
        fn64_render::NeutralPixelSize::Bits16 => crate::PixelSize::Bits16,
        fn64_render::NeutralPixelSize::Bits32 => crate::PixelSize::Bits32,
    }
}

mod capture;
mod census;
mod color;
mod execute;
mod plan;
mod state;

pub use census::{task_cpu_phase_running_totals, TaskCpuPhaseRunningTotals};
pub(self) use census::{
    raw_dpc_execute_census, raw_dpc_plan_census, task_compute_census, task_cpu_phase_census,
};
pub(self) use plan::{
    plan_raw_dpc_inner, transaction_sequence, CommandIndex, PlanCollector, ScheduledRawTriangle,
};
#[cfg(test)]
pub(self) use plan::{
    classify_no_raw_triangle_flags, finalize_with_zero_reads, submit_locally,
    ScheduledRawTriangleDecodeError, TriangleIndex,
};
#[cfg(test)]
pub(self) use plan::single_source_probe_journal;

pub use state::{ComputeRasterProbeReceipt, WgpuBackend, WgpuBackendConstructionError};
#[cfg(any(test, feature = "conformance-runner"))]
pub(crate) use state::WgpuCreateError;
pub(self) use state::{
    flush_compute_probe, push_finished_compute_probe, retain_compute_probe_draw,
    ComputeProgramAttribution, ComputeRasterDispatch, ComputeRasterProbe,
    ComputeRasterProbeBuilder, ComputeRasterProbePush, OrderedCpuColorBatch,
    PendingColorPublication, PendingFillPublication, PlannedNoRawTriangleReason,
    PlannedTaskCpuReason, PlannedTaskExecution, RawDpcCarryIn, RdpDrawState,
    COLOR_TARGET_REGISTRY_CAPACITY,
};
pub(self) use capture::{
    CapturedGuestReadAuthority, CapturedGuestReadBytes, TaskGuestReadCapturePool,
};
#[cfg(test)]
pub(self) use capture::IndexedCapturedGuestRead;
#[cfg(test)]
pub(self) use state::PublishedVisualTargetMarker;

pub use execute::WgpuRawDpcExecutionError;
pub(self) use execute::{
    admit_task_compute_member, complete_deferred_compute_segment,
    compute_program_attribution_from_ids, compute_segment_program_attribution,
    execute_raw_dpc_inner, ComputeEligibleTaskMember, DeferredComputeColor,
    ExactCheckpointImages, ExecutionCollector, StagedFill, StagedOutcome, StagedRawDpcMember,
    TaskComputeCpuReason, TaskComputeDisposition, TaskMemberDispatch,
};
#[cfg(test)]
pub(self) use execute::{
    compute_program_attribution_from_members, merged_fill_and_tmem_writes, word_source_bytes,
};

pub use color::TaskComputeAdmissionRefusal;
pub(self) use color::{
    claimed_rectangle_from_accesses, compute_column_bounds_enabled,
    logical_bytes_from_captured_rdram, ordered_depth_free_acff_triangle_member, prefix_before,
    shared_copyback_payloads_enabled, stage_color_commands, ComputeRasterReplacementPlan,
    TexrectTmemSource,
};
#[cfg(test)]
pub(self) use color::{
    color_command_input, compute_raster_replacement_admitted, task_cpu_phase_hot_program,
    task_cpu_phase_shape,
};

#[cfg(test)]
mod tests;
