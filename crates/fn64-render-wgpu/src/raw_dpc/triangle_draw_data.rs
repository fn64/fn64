//! Sealed-plan triangle-draw composition: adapts one admitted `RawTriangle`
//! command and the `SetOtherMode`/`SetCombine` state current at its own
//! stream position into the real `TrianglePipelineRenderer`'s input shapes
//! (`RasterVertex`/`OtherMode`/`CombineParams`), reading everything through
//! [`fn64_render::ExactRawDpcPlanVisitor`]'s nonextracting borrow -- never
//! `decoded.commands()` directly (see `production_triangle_draw` card §4).
//! This module composes a sealed plan's own data into the pipeline's input
//! shapes; it does not reach `WgpuBackend`/`RenderBackend` (`production.rs`)
//! itself.
//!
//! Two seams:
//! - [`neutral_vertex_to_raster_vertex`]: a plain field-by-field adapter,
//!   `NeutralTriangleVertex -> RasterVertex`, no arithmetic -- same shape as
//!   `triangle_composition.rs`'s existing position-only adapter, but wider
//!   output (position/uv/color instead of position alone).
//! - [`TriangleDrawStateCollector`]/[`RetrievedTriangleDraw`]: an
//!   [`fn64_render::ExactRawDpcPlanVisitor`] implementation (same shape as
//!   `production.rs`'s own `PlanCollector` -- `Default`, driven through
//!   [`fn64_render::BoundSubmittedRawDpc::execution_view`], the sealed API's
//!   only nonextracting route to a plan's contents) that snapshots the
//!   `SetOtherMode`/`SetCombine` state current *at each triangle's own
//!   stream position* onto that triangle, mirroring
//!   `production_adapter.rs`'s own stream-position-sensitive
//!   `current_other_mode` tracking at decode time -- not a single
//!   whole-plan-final value, which would retroactively apply a later
//!   state change to an earlier draw. [`retrieve_triangle_draws`] is a
//!   convenience for callers that already hold `&ExactValidatedRawDpcPlan`
//!   directly.
//!
//! Absence handling (card §7): a triangle walked before this plan's own
//! first `SetOtherMode`/`SetCombine` is not a silent default. This module's
//! own choice, stated here rather than only at the fix site: **hard
//! error**, naming which triangle (its plan-order index), not a silent
//! `OtherMode::from_wire(0, 0)` fallback -- the fixed fixture path
//! (`targets/triangle_pipeline.rs`'s `fixed_fixture_other_mode`) already
//! covers the "no real state" case explicitly; this module's whole purpose
//! is wiring *real* decoded state, so silently substituting a default here
//! would defeat that purpose without saying so.

use fn64_render::{
    ExactRawDpcPlanVisitor, ExactValidatedRawDpcPlan, NeutralTriangleVertex,
    RawDpcSemanticCommandRef, RdpStateCommand, RdpTriangleCommand,
};

use crate::state::OtherMode;
use crate::{CombineParams, RasterVertex};

/// Field-by-field adapter from the sealed plan's decoded triangle-vertex
/// shape to the real pipeline's `VertexInput` layout
/// (`triangle_pipeline_vertex.wgsl`'s `{position, uv, color}`). No
/// transformation of any kind -- `position` is the raw RDP screen-pixel
/// `x`/`y`/`z`/`w` the vertex shader itself converts to NDC (see
/// `triangle_pipeline_vertex.wgsl`'s own module doc); this function performs
/// none of that conversion.
pub const fn neutral_vertex_to_raster_vertex(vertex: NeutralTriangleVertex) -> RasterVertex {
    RasterVertex {
        position: [vertex.x, vertex.y, vertex.z, vertex.w],
        uv: vertex.texcoord,
        color: vertex.color,
    }
}

/// Loud, named absence: a triangle at plan-order index `triangle_index` was
/// visited before this plan's own first [`RdpStateCommand::SetOtherMode`]
/// or [`RdpStateCommand::SetCombine`] -- see this module's own doc for why
/// that is a hard error here, not a silent default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingTriangleDrawState {
    NoOtherMode { triangle_index: usize },
    NoCombine { triangle_index: usize },
}

impl core::fmt::Display for MissingTriangleDrawState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (missing, triangle_index) = match self {
            Self::NoOtherMode { triangle_index } => ("SetOtherMode", *triangle_index),
            Self::NoCombine { triangle_index } => ("SetCombine", *triangle_index),
        };
        write!(
            formatter,
            "triangle #{triangle_index} (plan order) was visited before this plan's own first \
             {missing} command; a triangle draw cannot retrieve real state that was never \
             admitted at its own stream position"
        )
    }
}

impl std::error::Error for MissingTriangleDrawState {}

/// One admitted triangle's vertices plus the `OtherMode`/`CombineParams`
/// current at ITS OWN stream position in the sealed plan -- never a
/// whole-plan-final value, which would let a later `SetOtherMode`/
/// `SetCombine` retroactively change an earlier triangle's draw state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RetrievedTriangleDraw {
    pub vertices: [NeutralTriangleVertex; 3],
    pub other_mode: OtherMode,
    pub combine_params: CombineParams,
}

/// [`ExactRawDpcPlanVisitor`] implementation collecting one
/// [`RetrievedTriangleDraw`] per admitted triangle, in plan order --
/// `Default` and driven through
/// [`fn64_render::BoundSubmittedRawDpc::execution_view`] (same shape as
/// `production.rs`'s own `PlanCollector`) for callers that only have a
/// `BoundSubmittedRawDpc`, not a bare `&ExactValidatedRawDpcPlan`.
#[derive(Default)]
pub struct TriangleDrawStateCollector {
    draws: Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
    current_other_mode: Option<OtherMode>,
    current_combine: Option<CombineParams>,
}

impl ExactRawDpcPlanVisitor for TriangleDrawStateCollector {
    fn command(&mut self, command: RawDpcSemanticCommandRef<'_>) {
        match command {
            RawDpcSemanticCommandRef::Triangle(RdpTriangleCommand { vertices, .. }) => {
                let triangle_index = self.draws.len();
                let snapshot = (|| {
                    let other_mode = self
                        .current_other_mode
                        .ok_or(MissingTriangleDrawState::NoOtherMode { triangle_index })?;
                    let combine_params = self
                        .current_combine
                        .ok_or(MissingTriangleDrawState::NoCombine { triangle_index })?;
                    Ok(RetrievedTriangleDraw {
                        vertices: *vertices,
                        other_mode,
                        combine_params,
                    })
                })();
                self.draws.push(snapshot);
            }
            RawDpcSemanticCommandRef::State(state) => match state {
                RdpStateCommand::SetOtherMode { other_mode, .. } => {
                    self.current_other_mode =
                        Some(OtherMode::from_wire(other_mode.high, other_mode.low));
                }
                RdpStateCommand::SetCombine { combine, .. } => {
                    self.current_combine =
                        Some(CombineParams::from_wire(combine.low, combine.high));
                }
                _ => {}
            },
            RawDpcSemanticCommandRef::TmemLoad(_) => {}
            _ => {}
        }
    }

    fn access(&mut self, _access: fn64_render_ir::ResourceAccess) {}
}

impl TriangleDrawStateCollector {
    /// Consumes this collector into the final per-triangle result list, in
    /// plan order. The first triangle that was visited before this plan's
    /// own first `SetOtherMode`/`SetCombine` (if any) fails the whole call
    /// with the loud, indexed error naming that triangle -- never silently
    /// dropped or defaulted (see module doc).
    pub fn finish(self) -> Result<Vec<RetrievedTriangleDraw>, MissingTriangleDrawState> {
        self.draws.into_iter().collect()
    }
}

/// Walks `plan` once via [`ExactValidatedRawDpcPlan::visit`] (never reading
/// `decoded.commands()` directly -- card §4/§7) and returns every admitted
/// triangle's draw state, each snapshotted at its own stream position. For
/// callers that already hold `&ExactValidatedRawDpcPlan` directly; callers
/// with only a `BoundSubmittedRawDpc` drive [`TriangleDrawStateCollector`]
/// through `execution_view` instead.
pub fn retrieve_triangle_draws(
    plan: &ExactValidatedRawDpcPlan,
) -> Result<Vec<RetrievedTriangleDraw>, MissingTriangleDrawState> {
    let mut collector = TriangleDrawStateCollector::default();
    plan.visit(&mut collector);
    collector.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertex(seed: f32) -> NeutralTriangleVertex {
        NeutralTriangleVertex {
            x: seed,
            y: seed + 1.0,
            z: seed + 2.0,
            w: seed + 3.0,
            color: [seed + 4.0, seed + 5.0, seed + 6.0, seed + 7.0],
            texcoord: [seed + 8.0, seed + 9.0],
        }
    }

    #[test]
    fn vertex_adapter_round_trips_every_field_unchanged() {
        let source = vertex(1.0);
        let raster = neutral_vertex_to_raster_vertex(source);
        assert_eq!(raster.position, [source.x, source.y, source.z, source.w]);
        assert_eq!(raster.uv, source.texcoord);
        assert_eq!(raster.color, source.color);
    }

    fn location(index: u32) -> fn64_render::RawDpcCommandLocation {
        fn64_render::RawDpcCommandLocation {
            command_index: index,
            stream_index: 0,
            chunk_index: 0,
            source_address: fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000)
                .unwrap()
                .address(0)
                .unwrap(),
            source_byte_offset: 0,
            source_byte_len: 8,
            wire_opcode: 0,
        }
    }

    fn other_mode_command(index: u32, high: u32, low: u32) -> RdpStateCommand {
        RdpStateCommand::SetOtherMode {
            location: location(index),
            raw_words: Box::new([0, 0]),
            other_mode: fn64_render::NeutralOtherMode { high, low },
            before: None,
            after: fn64_render::RdpStateIdentity::of_other_mode(fn64_render::NeutralOtherMode {
                high,
                low,
            }),
        }
    }

    fn combine_command(index: u32, low: u32, high: u32) -> RdpStateCommand {
        RdpStateCommand::SetCombine {
            location: location(index),
            raw_words: Box::new([0, 0]),
            combine: fn64_render::NeutralCombineParams { low, high },
            before: None,
            after: fn64_render::RdpStateIdentity::of_combine(fn64_render::NeutralCombineParams {
                low,
                high,
            }),
        }
    }

    fn triangle_command(vertices: [NeutralTriangleVertex; 3]) -> RdpTriangleCommand {
        RdpTriangleCommand {
            location: fn64_render::RawDpcCommandLocation {
                source_byte_len: 32,
                wire_opcode: 0x08,
                ..location(0)
            },
            raw_words: Box::new([0; 8]),
            vertices,
        }
    }

    #[test]
    fn a_triangle_snapshots_the_state_current_at_its_own_stream_position_not_a_later_one() {
        // Interleaved: SetOtherMode(A)/SetCombine(A) -> triangle A ->
        // SetOtherMode(B)/SetCombine(B) -> triangle B. Triangle A must keep
        // state A even though the collector later observes state B.
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode_a = other_mode_command(0, 0, 0);
        let combine_a = combine_command(1, 1, 2);
        let triangle_a = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);
        let other_mode_b = other_mode_command(3, 0x00c0_0000, 0);
        let combine_b = combine_command(4, 3, 4);
        let triangle_b = triangle_command([vertex(10.0), vertex(11.0), vertex(12.0)]);

        collector.command(RawDpcSemanticCommandRef::State(&other_mode_a));
        collector.command(RawDpcSemanticCommandRef::State(&combine_a));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_a));
        collector.command(RawDpcSemanticCommandRef::State(&other_mode_b));
        collector.command(RawDpcSemanticCommandRef::State(&combine_b));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_b));

        let draws = collector.finish().expect("both triangles have state");
        assert_eq!(draws.len(), 2);
        assert_eq!(draws[0].vertices, triangle_a.vertices);
        assert_eq!(draws[0].other_mode, OtherMode::from_wire(0, 0));
        assert_eq!(draws[0].combine_params, CombineParams::from_wire(1, 2));
        assert_eq!(draws[1].vertices, triangle_b.vertices);
        assert_eq!(draws[1].other_mode, OtherMode::from_wire(0x00c0_0000, 0));
        assert_eq!(draws[1].combine_params, CombineParams::from_wire(3, 4));
        assert_ne!(
            draws[0].other_mode, draws[1].other_mode,
            "the two OtherMode values must actually differ, or this test cannot distinguish \
             correct per-triangle snapshotting from an incorrect shared final value"
        );
    }

    #[test]
    fn a_triangle_before_any_state_is_a_loud_named_error_naming_that_triangles_index() {
        let mut collector = TriangleDrawStateCollector::default();
        let triangle = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
        let error = collector.finish().unwrap_err();
        assert_eq!(
            error,
            MissingTriangleDrawState::NoOtherMode { triangle_index: 0 }
        );
    }

    #[test]
    fn a_triangle_with_other_mode_but_no_combine_is_a_loud_named_error() {
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode = other_mode_command(0, 0, 0);
        let triangle = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);
        collector.command(RawDpcSemanticCommandRef::State(&other_mode));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
        let error = collector.finish().unwrap_err();
        assert_eq!(
            error,
            MissingTriangleDrawState::NoCombine { triangle_index: 0 }
        );
    }

    #[test]
    fn missing_state_display_names_the_command_and_the_triangle_index() {
        assert_eq!(
            MissingTriangleDrawState::NoOtherMode { triangle_index: 2 }.to_string(),
            "triangle #2 (plan order) was visited before this plan's own first SetOtherMode \
             command; a triangle draw cannot retrieve real state that was never admitted at \
             its own stream position"
        );
        assert_eq!(
            MissingTriangleDrawState::NoCombine { triangle_index: 0 }.to_string(),
            "triangle #0 (plan order) was visited before this plan's own first SetCombine \
             command; a triangle draw cannot retrieve real state that was never admitted at \
             its own stream position"
        );
    }
}
