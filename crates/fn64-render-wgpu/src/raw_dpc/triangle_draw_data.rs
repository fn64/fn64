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
    RawDpcSemanticCommandRef, RdpStateCommand, RdpTriangleCommand, RectViewportPixels,
    TriangleSource,
};

use crate::state::{AlphaCompare, Color4, OtherMode, PrimColor, PrimDepth};
use crate::targets::RdpScissorRect;
use crate::tmem::TileBindingParams;
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

/// One admitted triangle's vertices plus the `OtherMode`/`CombineParams`/
/// tile binding current at ITS OWN stream position in the sealed plan --
/// never a whole-plan-final value, which would let a later `SetOtherMode`/
/// `SetCombine`/`SetTile`/`SetTileSize` retroactively change an earlier
/// triangle's draw state. `tile_binding` is
/// [`TileBindingParams::unbound`] when tile 0 (the RDP's default bound
/// texture tile for a standard triangle draw -- `RdpTriangleCommand`
/// carries no tile index of its own) had no snapshotted `TileDescriptor`/
/// `TileSize` pair at this triangle's own stream position (published
/// committed-TMEM textured-draw card §2/§6: "missing `TileDescriptor`" is a
/// named condition, never a silent default).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RetrievedTriangleDraw {
    pub vertices: [NeutralTriangleVertex; 3],
    /// Which wire command admitted this triangle -- see
    /// [`fn64_render::TriangleSource`].
    pub source: TriangleSource,
    /// Pixel-space viewport override for a `TextureRectangle`-sourced
    /// triangle; `None` for `TriangleSource::RawTriangle`.
    pub viewport: Option<RectViewportPixels>,
    pub other_mode: OtherMode,
    pub combine_params: CombineParams,
    pub tile_binding: TileBindingParams,
    /// `G_SETBLENDCOLOR` current at this triangle's own stream position --
    /// unconditionally tracked (mirrors `env_color`/`prim_color`'s
    /// command-time-snapshot pattern), since both alpha-compare's
    /// `Threshold` mode and the production blend-cycle wiring
    /// (`BlendColorInput::Blend`) independently need the real register
    /// value; each consumer validates presence only when its own
    /// selector/mode requires it (alpha-compare's `Threshold` gate at
    /// retrieval time; blend-cycle's `BlendColorInput::Blend`/
    /// `BlendAlphaInput`-independent gate at draw-submission time) rather
    /// than this struct duplicating one register behind two fields.
    pub blend_color: Color4,
    /// `G_SETENVCOLOR` current at this triangle's own stream position --
    /// mirrors `blend_color`'s same command-time-snapshot pattern,
    /// unconditionally tracked.
    pub env_color: Color4,
    /// `G_SETPRIMCOLOR` current at this triangle's own stream position --
    /// mirrors `env_color` exactly.
    pub prim_color: PrimColor,
    /// `G_SETFOGCOLOR` current at this triangle's own stream position --
    /// mirrors `blend_color`/`env_color`/`prim_color` exactly. Needed by
    /// the production blend-cycle wiring whenever a resolved cycle's `P`/
    /// `M` selects [`crate::blend::BlendColorInput::Fog`] or its `A`
    /// selects [`crate::blend::BlendAlphaInput::Fog`].
    pub fog_color: Color4,
    /// `G_SETSCISSOR` current at this triangle's own stream position, as
    /// the quarter-pixel rect the wire carries -- the same
    /// command-time-snapshot pattern as `blend_color`/`env_color`/
    /// `prim_color`/`fog_color`, and for the same reason: one packet can
    /// carry several rectangles under different scissors, so the walk's
    /// running final value would clip the earlier ones with the later
    /// one's rect.
    ///
    /// `None` means this plan issued no `SetScissor` before this triangle.
    /// It is deliberately NOT defaulted to the full framebuffer here: the
    /// consumer, not this collector, owns the fallback, because only the
    /// consumer knows the target extent that fallback has to be. See
    /// `production.rs`'s texrect submission for where it is supplied.
    pub scissor: Option<RdpScissorRect>,
    /// `G_SETPRIMDEPTH` current at this triangle's own stream position --
    /// the same command-time-snapshot pattern as the color registers. The
    /// CPU raster path's z-compare reads this as the fragment depth under
    /// `G_ZS_PRIM` (`OtherMode::primitive_depth_source()`); `None` means no
    /// `SetPrimDepth` preceded this triangle, in which case a G_ZS_PRIM
    /// z-compared draw has no primitive depth to compare and the consumer
    /// falls back to painter's order (documented at the depth-test site).
    pub prim_depth: Option<PrimDepth>,
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
    /// Every tile's binding current at the walk's current stream position.
    ///
    /// An entry is `(None, None)` until this plan's own
    /// `SetTile`/`SetTileSize` pair for that index is both present;
    /// [`TileBindingParams::unbound`] is used at snapshot time when either
    /// half is still missing (card §6: a named, non-fatal condition here --
    /// unlike `OtherMode`/`CombineParams`, a missing tile binding does not
    /// fail triangle retrieval itself; the WGSL sampler's own
    /// `TMEM_SAMPLE_STATUS_NO_TILE_BINDING` surfaces it per-fragment
    /// instead, since a flat-colored/non-textured triangle legitimately has
    /// no tile bound at all).
    ///
    /// **This was tile 0 alone, on the claim that "`RdpTriangleCommand`
    /// carries no tile index of its own". That claim was false** -- the
    /// decoder reads the index at `triangle.rs:183` and the command retains
    /// the wire words it came from.
    ///
    /// **The whole 8-entry tile table**, not tile 0 alone.
    ///
    /// EVERY admitted draw names its own tile in its own wire word -- a
    /// raw triangle in word 0 bits 18:16, a texture rectangle in word 1
    /// bits 26:24 -- so tracking only tile 0 silently bound tile 0's
    /// descriptor for any draw naming another. `production.rs`'s
    /// `PlanCollector` already carries the whole table for exactly this
    /// reason; this is the same fix on the copy its doc requires to match.
    current_tiles: [(
        Option<fn64_render::NeutralTileDescriptor>,
        Option<fn64_render::NeutralTileSize>,
    ); 8],
    /// `G_SETBLENDCOLOR` current at the walk's current stream position --
    /// mirrors `current_other_mode`/`current_combine` exactly, a fourth
    /// instance of the same command-time-snapshot pattern (card §4a).
    current_blend_color: Color4,
    /// `G_SETENVCOLOR` current at the walk's current stream position --
    /// mirrors `current_blend_color`, unconditionally tracked (no
    /// `AlphaCompare` gate).
    current_env_color: Color4,
    /// `G_SETPRIMCOLOR` current at the walk's current stream position --
    /// mirrors `current_env_color` exactly.
    current_prim_color: PrimColor,
    /// `G_SETFOGCOLOR` current at the walk's current stream position --
    /// mirrors `current_env_color`/`current_prim_color` exactly.
    current_fog_color: Color4,
    /// `G_SETSCISSOR` current at the walk's current stream position --
    /// mirrors `current_fog_color`'s pattern, except that it stays `None`
    /// until this plan issues one (see [`RetrievedTriangleDraw::scissor`]
    /// for why the fallback is the consumer's and not this collector's).
    current_scissor: Option<RdpScissorRect>,
    /// `G_SETPRIMDEPTH` current at the walk's current stream position --
    /// mirrors `current_scissor`'s `None`-until-issued pattern. Read by the
    /// CPU raster path's z-compare under `G_ZS_PRIM`.
    current_prim_depth: Option<PrimDepth>,
}

/// The tile index a draw names in its OWN wire word.
///
/// **One implementation, because there are two collectors.**
/// `production.rs`'s `PlanCollector` and this file's
/// `TriangleDrawStateCollector` both walk the same plan and must resolve the
/// same tile for the same draw; when each carried its own copy of this
/// arithmetic they drifted, and the copy here stayed frozen at tile 0 long
/// after the other was fixed. A shared function cannot drift.
///
/// A raw triangle names its tile in word 0 bits 18:16 -- the field
/// `RawTriangle::decode` reads (`triangle.rs:183`). A texture rectangle
/// names its own in word 1 bits 26:24. RT64 honours the draw's tile the
/// same way: `drawTris` assigns `drawCall.textureTile = tile`
/// (`rt64_rdp.cpp:1088-1097`).
///
/// Falls back to 0 only when the words are absent, which a decoded command
/// never is -- the fallback exists so a malformed fixture binds the default
/// tile rather than panicking mid-walk.
pub fn bound_tile_index(source: TriangleSource, raw_words: &[u32]) -> usize {
    match source {
        TriangleSource::TextureRectangle => raw_words
            .get(1)
            .map(|word| ((word >> 24) & 0x7) as usize)
            .unwrap_or(0),
        TriangleSource::RawTriangle => raw_words
            .first()
            .map(|word| ((word >> 16) & 0x7) as usize)
            .unwrap_or(0),
    }
}

impl ExactRawDpcPlanVisitor for TriangleDrawStateCollector {
    fn command(&mut self, command: RawDpcSemanticCommandRef<'_>) {
        match command {
            RawDpcSemanticCommandRef::Triangle(RdpTriangleCommand {
                vertices,
                source,
                viewport,
                raw_words,
                ..
            }) => {
                let triangle_index = self.draws.len();
                // **The draw's OWN tile, read from its retained wire words.**
                //
                // This arm previously froze the index to 0. A raw triangle
                // names its tile in word 0 bits 18:16 -- the same field
                // `RawTriangle::decode` reads as `tile` -- and a texture
                // rectangle names its own in word 1 bits 26:24. RT64 does
                // the same: `drawTris` takes the draw's tile and assigns
                // `drawCall.textureTile = tile` (`rt64_rdp.cpp:1088-1097`).
                //
                // Identical to the recovery `production.rs`'s `PlanCollector`
                // already performs, so the two collectors resolve the SAME
                // tile for the same draw.
                let bound_tile_index = bound_tile_index(*source, raw_words);
                let tile_binding = match self
                    .current_tiles
                    .get(bound_tile_index)
                    .copied()
                    .unwrap_or((None, None))
                {
                    (Some(descriptor), Some(size)) => {
                        TileBindingParams::from_neutral(descriptor, size)
                    }
                    _ => TileBindingParams::unbound(),
                };
                let snapshot = (|| {
                    let other_mode = self
                        .current_other_mode
                        .ok_or(MissingTriangleDrawState::NoOtherMode { triangle_index })?;
                    let combine_params = self
                        .current_combine
                        .ok_or(MissingTriangleDrawState::NoCombine { triangle_index })?;
                    // Retrieval-time admission gate (card §4a): `Dither`
                    // never reaches `submit_admitted_triangle` -- a loud,
                    // named panic here, not a silent None/Threshold coercion
                    // (AGENTS.md "loud traps, no silent shrugs"). There is no
                    // reserved encoding to gate: pinned RT64's shader
                    // branches only for `G_AC_DITHER` and `G_AC_THRESHOLD`,
                    // so wire 2 falls through to no compare
                    // (`src/shaders/RasterPS.hlsl:203-213`, commit
                    // `f0728a2`; `docs/RT64-GUARD-AUDIT.md` A3).
                    match other_mode.alpha_compare() {
                        AlphaCompare::Dither => panic!(
                            "triangle #{triangle_index} (plan order) selected G_AC_DITHER \
                             alpha-compare, which has no fragment-callable RT64 PRNG binding in \
                             this pipeline (no frame-count uniform exists to seed it honestly; \
                             see fn64-alpha-compare-production-card.md \u{a7}2)"
                        ),
                        // `Threshold` compares fragment alpha against
                        // `G_SETBLENDCOLOR.a`, a register that always holds
                        // a value (zero until written), so there is nothing
                        // to refuse -- see the twin gate in
                        // `production.rs` and `RdpState`'s constant-color
                        // field doc for the citations.
                        AlphaCompare::Threshold | AlphaCompare::None => {}
                    };
                    Ok(RetrievedTriangleDraw {
                        vertices: *vertices,
                        source: *source,
                        viewport: *viewport,
                        other_mode,
                        combine_params,
                        tile_binding,
                        blend_color: self.current_blend_color,
                        env_color: self.current_env_color,
                        prim_color: self.current_prim_color,
                        fog_color: self.current_fog_color,
                        scissor: self.current_scissor,
                        prim_depth: self.current_prim_depth,
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
                RdpStateCommand::SetBlendColor { color, .. } => {
                    self.current_blend_color = Color4::from_wire(color.value);
                }
                RdpStateCommand::SetEnvColor { color, .. } => {
                    self.current_env_color = Color4::from_wire(color.value);
                }
                RdpStateCommand::SetPrimColor { color, .. } => {
                    self.current_prim_color = PrimColor::from_wire(
                        u32::from(color.lod_frac) | (u32::from(color.lod_min) << 8),
                        color.color,
                    );
                }
                RdpStateCommand::SetFogColor { color, .. } => {
                    self.current_fog_color = Color4::from_wire(color.value);
                }
                RdpStateCommand::SetPrimDepth { depth, .. } => {
                    // Reconstruct the wire form the neutral DTO was minted
                    // from (`z` in bits 16:31, `dz` in bits 0:15) so this
                    // collector reads the exact same masked z/dz the decoder
                    // produced -- `PrimDepth::from_wire` re-applies the 15-bit
                    // z mask and full 16-bit dz mask.
                    self.current_prim_depth = Some(PrimDepth::from_wire(
                        (u32::from(depth.z) << 16) | u32::from(depth.dz),
                    ));
                }
                // Latched verbatim in wire quarter-pixels. Public libultra
                // `include/ultra64/gbi.h:4794-4837` encodes the four
                // coordinates as twelve-bit fields scaled by four, or
                // accepts the fractional wire values directly. Retaining a
                // reversed or empty rect until clip time is fn64's own
                // reading and is not independently confirmed against an
                // allowed hardware reference.
                RdpStateCommand::SetScissor { scissor, .. } => {
                    self.current_scissor = Some(RdpScissorRect::from_wire_quarter_pixels(
                        scissor.mode,
                        scissor.upper_left_x,
                        scissor.upper_left_y,
                        scissor.lower_right_x,
                        scissor.lower_right_y,
                    ));
                }
                RdpStateCommand::SetTile {
                    tile_index,
                    descriptor,
                    ..
                } => {
                    if let Some(slot) = self.current_tiles.get_mut(usize::from(*tile_index)) {
                        slot.0 = Some(*descriptor);
                    }
                }
                RdpStateCommand::SetTileSize {
                    tile_index, size, ..
                } => {
                    if let Some(slot) = self.current_tiles.get_mut(usize::from(*tile_index)) {
                        slot.1 = Some(*size);
                    }
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
            source: TriangleSource::RawTriangle,
            viewport: None,
            texrect_accesses: None,
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

    fn blend_color_command(index: u32, value: u32) -> RdpStateCommand {
        RdpStateCommand::SetBlendColor {
            location: location(index),
            raw_words: Box::new([0]),
            color: fn64_render::NeutralColor4 { value },
            before: None,
            after: fn64_render::RdpStateIdentity::of_blend_color(fn64_render::NeutralColor4 {
                value,
            }),
        }
    }

    /// Wire encoding `(0, 1)` decodes `AlphaCompare::Threshold`
    /// (`state.rs`'s `alpha_compare()` table: bits 0:1 == 1).
    fn threshold_other_mode_command(index: u32) -> RdpStateCommand {
        other_mode_command(index, 0, 1)
    }

    #[test]
    fn a_threshold_triangle_snapshots_blend_color_at_its_own_stream_position() {
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode = threshold_other_mode_command(0);
        let combine = combine_command(1, 1, 2);
        let blend_color = blend_color_command(2, 0x1122_3344);
        let triangle = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);

        collector.command(RawDpcSemanticCommandRef::State(&other_mode));
        collector.command(RawDpcSemanticCommandRef::State(&combine));
        collector.command(RawDpcSemanticCommandRef::State(&blend_color));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

        let draws = collector
            .finish()
            .expect("threshold triangle has blend_color");
        assert_eq!(draws[0].blend_color, Color4::from_wire(0x1122_3344));
    }

    #[test]
    fn a_none_mode_triangle_never_requires_blend_color() {
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode = other_mode_command(0, 0, 0); // None mode
        let combine = combine_command(1, 1, 2);
        let triangle = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);

        collector.command(RawDpcSemanticCommandRef::State(&other_mode));
        collector.command(RawDpcSemanticCommandRef::State(&combine));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

        let draws = collector
            .finish()
            .expect("None-mode triangle needs no blend_color at all");
        assert_eq!(draws[0].blend_color, Color4::from_wire(0));
    }

    /// **Wall 6.** A `Threshold` triangle with no `SetBlendColor` anywhere
    /// in the plan is admitted, carrying the blend register's power-on
    /// zero.
    ///
    /// This replaces a test that asserted the opposite. `Threshold`
    /// compares the fragment alpha against `G_SETBLENDCOLOR.a`, and that
    /// register always holds a value -- zero until the guest writes one.
    /// `fn64-render-reference` models it as a zero-initialized `[u8; 4]`
    /// (`gbi/state.rs:227`, `:387`) and RT64's C++ zero-initializes
    /// `blendColor` at `src/hle/rt64_state.cpp:131`. The refusal invented an
    /// "unset" state the RDP cannot be in, and it aborted WM2000's plan.
    #[test]
    fn a_threshold_triangle_before_any_blend_color_reads_the_power_on_zero() {
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode = threshold_other_mode_command(0);
        let combine = combine_command(1, 1, 2);
        let triangle = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);

        collector.command(RawDpcSemanticCommandRef::State(&other_mode));
        collector.command(RawDpcSemanticCommandRef::State(&combine));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

        let draws = collector
            .finish()
            .expect("a Threshold triangle with no SetBlendColor is admissible");
        // Derived by hand: an RDP color register powers up holding four
        // zero bytes, so the packed wire word is 0x0000_0000.
        assert_eq!(draws[0].blend_color, Color4::from_wire(0));
    }

    /// The companion to the test above: an in-plan `SetBlendColor` must
    /// still reach the snapshot. Without this, the power-on assertion could
    /// hold against a field hardcoded to zero that ignores every write.
    #[test]
    fn a_threshold_triangle_after_a_blend_color_carries_that_written_value() {
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode = threshold_other_mode_command(0);
        let combine = combine_command(1, 1, 2);
        let written = blend_color_command(2, 0x1122_3344);
        let triangle = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);

        collector.command(RawDpcSemanticCommandRef::State(&other_mode));
        collector.command(RawDpcSemanticCommandRef::State(&combine));
        collector.command(RawDpcSemanticCommandRef::State(&written));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

        let draws = collector
            .finish()
            .expect("written blend color is admissible");
        assert_eq!(draws[0].blend_color, Color4::from_wire(0x1122_3344));
        assert_ne!(
            draws[0].blend_color,
            Color4::from_wire(0),
            "the written value must differ from the power-on zero, or neither test can \
             distinguish real tracking from a hardcoded constant"
        );
    }

    /// Two Threshold triangles separated by an intervening `SetBlendColor`
    /// change must collect two different snapshots, not one collapsed
    /// whole-plan-final value -- the same regression shape §4c documents for
    /// `SetCombine`/`production.rs`'s `plan_collector_snapshots_each_
    /// triangle_at_its_own_stream_position_not_the_final_value`.
    #[test]
    fn a_and_b_triangles_snapshot_distinct_blend_colors_not_a_collapsed_final_value() {
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode = threshold_other_mode_command(0);
        let combine = combine_command(1, 1, 2);
        let blend_color_x = blend_color_command(2, 0x0000_00AA);
        let triangle_a = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);
        let blend_color_y = blend_color_command(4, 0x0000_00BB);
        let triangle_b = triangle_command([vertex(10.0), vertex(11.0), vertex(12.0)]);

        collector.command(RawDpcSemanticCommandRef::State(&other_mode));
        collector.command(RawDpcSemanticCommandRef::State(&combine));
        collector.command(RawDpcSemanticCommandRef::State(&blend_color_x));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_a));
        collector.command(RawDpcSemanticCommandRef::State(&blend_color_y));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_b));

        let draws = collector.finish().expect("both triangles have blend_color");
        assert_eq!(draws[0].blend_color, Color4::from_wire(0x0000_00AA));
        assert_eq!(draws[1].blend_color, Color4::from_wire(0x0000_00BB));
        assert_ne!(
            draws[0].blend_color, draws[1].blend_color,
            "triangle A must NOT be retroactively affected by a SetBlendColor that comes after \
             it in plan order"
        );
    }

    /// **A raw triangle binds ITS OWN tile, not tile 0.**
    ///
    /// A triangle names its tile in wire word 0 bits 18:16 -- the field
    /// `RawTriangle::decode` reads at `triangle.rs:183` -- and RT64 honours
    /// it: `drawTris` takes the draw's tile and assigns
    /// `drawCall.textureTile = tile` (`rt64_rdp.cpp:1088-1097`).
    ///
    /// This collector previously froze the index to 0 on the claim that
    /// "`RdpTriangleCommand` carries no tile index of its own", which is
    /// false, and tracked only `SetTile{tile_index: 0}` so tiles 1-7 were
    /// discarded outright. The consequence was silent: a triangle naming
    /// another tile sampled tile 0's descriptor -- wrong TMEM base, format,
    /// size and palette.
    ///
    /// WM2000 cannot catch this: measured, all 1,000,001 of its raw
    /// triangles name tile 0. So the regression guard has to be a fixture
    /// that names a NON-zero tile, and it must distinguish the two tiles by
    /// a field the binding actually carries.
    #[test]
    fn a_raw_triangle_binds_the_tile_its_own_wire_word_names() {
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode = other_mode_command(0, 0, 2);
        let combine = combine_command(1, 1, 2);

        // Two tiles that differ in a field the binding carries through.
        let descriptor = |tmem_word_address: u16| fn64_render::NeutralTileDescriptor {
            format: fn64_render::NeutralImageFormat::Rgba,
            size: fn64_render::NeutralPixelSize::Bits16,
            line_words: 1,
            tmem_word_address,
            palette: 0,
            s_mode: fn64_render::NeutralTileAddressMode {
                mirror: false,
                clamp: true,
            },
            mask_s: 0,
            shift_s: 0,
            t_mode: fn64_render::NeutralTileAddressMode {
                mirror: false,
                clamp: true,
            },
            mask_t: 0,
            shift_t: 0,
        };
        let size = fn64_render::NeutralTileSize {
            low_s: 0,
            low_t: 0,
            high_s: 4,
            high_t: 4,
        };
        let tile_state = |index: u32, tile_index: u8, tmem: u16| {
            (
                RdpStateCommand::SetTile {
                    location: location(index),
                    raw_words: Box::new([0, 0]),
                    tile_index,
                    descriptor: descriptor(tmem),
                    before: None,
                    after: fn64_render::RdpStateIdentity::of_tile_descriptor(
                        tile_index,
                        descriptor(tmem),
                    ),
                },
                RdpStateCommand::SetTileSize {
                    location: location(index + 1),
                    raw_words: Box::new([0, 0]),
                    tile_index,
                    size,
                    before: None,
                    after: fn64_render::RdpStateIdentity::of_tile_size(tile_index, size),
                },
            )
        };
        // Tile 0 at TMEM word 0; tile 5 at TMEM word 256.
        let (tile0, tile0_size) = tile_state(2, 0, 0);
        let (tile5, tile5_size) = tile_state(4, 5, 256);

        // A triangle whose word 0 names tile 5 (bits 18:16).
        let mut triangle = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);
        let mut words = triangle.raw_words.to_vec();
        words[0] = 0x0800_0000 | (5 << 16);
        triangle.raw_words = words.into_boxed_slice();

        for command in [
            &other_mode,
            &combine,
            &tile0,
            &tile0_size,
            &tile5,
            &tile5_size,
        ] {
            collector.command(RawDpcSemanticCommandRef::State(command));
        }
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

        let draws = collector.finish().expect("the triangle is admitted");
        let draw = draws.into_iter().next().expect("one triangle was staged");
        assert_eq!(
            draw.tile_binding.tmem_word_address, 256,
            "a triangle naming tile 5 must bind tile 5's descriptor, not tile 0's"
        );
    }

    /// Retargeted from `a_reserved_alpha_compare_triangle_panics_loudly_at_
    /// retrieval_time`. Pinned RT64's shader branches only for
    /// `G_AC_DITHER` and `G_AC_THRESHOLD`, so wire 2 falls through and the
    /// triangle is ordinary no-compare content, not a refusal
    /// (`src/shaders/RasterPS.hlsl:203-213`, commit `f0728a2`).
    /// See `docs/RT64-GUARD-AUDIT.md` finding A3.
    #[test]
    fn an_alpha_compare_wire_two_triangle_is_retrieved_as_no_compare() {
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode = other_mode_command(0, 0, 2);
        let combine = combine_command(1, 1, 2);
        let triangle = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);

        collector.command(RawDpcSemanticCommandRef::State(&other_mode));
        collector.command(RawDpcSemanticCommandRef::State(&combine));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

        let draws = collector
            .finish()
            .expect("wire encoding 2 is admitted, not refused");
        let draw = draws.into_iter().next().expect("one triangle was staged");
        assert_eq!(draw.other_mode.alpha_compare(), AlphaCompare::None);
        // Distinguishing check: wire 3 still panics, so the retrieval above
        // cannot be produced by a collector that stopped gating entirely.
        // (`a_dither_alpha_compare_triangle_panics_loudly_naming_the_frame_
        // count_gap` below is that assertion.)
    }

    #[test]
    #[should_panic(expected = "selected G_AC_DITHER alpha-compare")]
    fn a_dither_alpha_compare_triangle_panics_loudly_naming_the_frame_count_gap() {
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode = other_mode_command(0, 0, 3); // Dither
        let combine = combine_command(1, 1, 2);
        let triangle = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);

        collector.command(RawDpcSemanticCommandRef::State(&other_mode));
        collector.command(RawDpcSemanticCommandRef::State(&combine));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
    }

    fn env_color_command(index: u32, value: u32) -> RdpStateCommand {
        RdpStateCommand::SetEnvColor {
            location: location(index),
            raw_words: Box::new([0]),
            color: fn64_render::NeutralColor4 { value },
            before: None,
            after: fn64_render::RdpStateIdentity::of_env_color(fn64_render::NeutralColor4 {
                value,
            }),
        }
    }

    fn prim_color_command(index: u32, lod_frac: u8, lod_min: u8, color: u32) -> RdpStateCommand {
        let neutral = fn64_render::NeutralPrimColor {
            lod_frac,
            lod_min,
            color,
        };
        RdpStateCommand::SetPrimColor {
            location: location(index),
            raw_words: Box::new([0, 0]),
            color: neutral,
            before: None,
            after: fn64_render::RdpStateIdentity::of_prim_color(neutral),
        }
    }

    /// Command-time capture seam (card): `SetEnvColor(A)`/`SetPrimColor(A)`
    /// -> triangle A -> `SetEnvColor(B)`/`SetPrimColor(B)` -> triangle B
    /// must collect two distinct snapshots, exactly mirroring
    /// `a_and_b_triangles_snapshot_distinct_blend_colors_not_a_collapsed_final_value`
    /// above for the new `env_color`/`prim_color` fields.
    #[test]
    fn a_and_b_triangles_snapshot_distinct_env_and_prim_colors_not_a_collapsed_final_value() {
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode = other_mode_command(0, 0, 0); // None mode, no blend_color needed
        let combine = combine_command(1, 1, 2);
        let env_a = env_color_command(2, 0x1111_1111);
        let prim_a = prim_color_command(3, 10, 5, 0x2222_2222);
        let triangle_a = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);
        let env_b = env_color_command(4, 0x3333_3333);
        let prim_b = prim_color_command(5, 20, 10, 0x4444_4444);
        let triangle_b = triangle_command([vertex(10.0), vertex(11.0), vertex(12.0)]);

        collector.command(RawDpcSemanticCommandRef::State(&other_mode));
        collector.command(RawDpcSemanticCommandRef::State(&combine));
        collector.command(RawDpcSemanticCommandRef::State(&env_a));
        collector.command(RawDpcSemanticCommandRef::State(&prim_a));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_a));
        collector.command(RawDpcSemanticCommandRef::State(&env_b));
        collector.command(RawDpcSemanticCommandRef::State(&prim_b));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_b));

        let draws = collector.finish().expect("both triangles have state");
        assert_eq!(draws.len(), 2);
        assert_eq!(draws[0].env_color, Color4::from_wire(0x1111_1111));
        assert_eq!(
            draws[0].prim_color,
            PrimColor::from_wire(10 | (5 << 8), 0x2222_2222)
        );
        assert_eq!(draws[1].env_color, Color4::from_wire(0x3333_3333));
        assert_eq!(
            draws[1].prim_color,
            PrimColor::from_wire(20 | (10 << 8), 0x4444_4444)
        );
        assert_ne!(
            draws[0].env_color, draws[1].env_color,
            "triangle A must NOT be retroactively affected by a SetEnvColor that comes after it \
             in plan order"
        );
        assert_ne!(
            draws[0].prim_color, draws[1].prim_color,
            "triangle A must NOT be retroactively affected by a SetPrimColor that comes after it \
             in plan order"
        );
    }

    /// A triangle visited before any `SetEnvColor`/`SetPrimColor` still
    /// resolves -- unlike `blend_color`, `env_color`/`prim_color` are
    /// unconditionally `Option`, never a hard-error gate (module doc).
    #[test]
    fn a_triangle_before_any_env_or_prim_color_resolves_with_none() {
        let mut collector = TriangleDrawStateCollector::default();
        let other_mode = other_mode_command(0, 0, 0);
        let combine = combine_command(1, 1, 2);
        let triangle = triangle_command([vertex(1.0), vertex(2.0), vertex(3.0)]);

        collector.command(RawDpcSemanticCommandRef::State(&other_mode));
        collector.command(RawDpcSemanticCommandRef::State(&combine));
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

        let draws = collector
            .finish()
            .expect("no env/prim color needed to resolve");
        assert_eq!(draws[0].env_color, Color4::from_wire(0));
        assert_eq!(draws[0].prim_color, PrimColor::from_wire(0, 0));
    }
}
