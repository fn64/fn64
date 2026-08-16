//! Cheap composition precursor: `RawTriangle::decode` -> `decode_triangle_vertices`
//! -> `raster_vs`'s vertex transform, called in sequence on one decoded
//! triangle.
//!
//! This is deliberately NOT rasterization, NOT a pixel-output path, and NOT
//! production draw-path wiring. Its entire purpose is to prove -- ahead of a
//! separate, larger GPU-pipeline investment -- that the types these three
//! already-landed pure Rust oracles produce and consume actually compose:
//! `RawTriangle` (this module's parent, `triangle.rs`) decodes one wire
//! triangle command; `decode_triangle_vertices` (`triangle_vertices.rs`)
//! converts it to three `TriangleVertex` values; `raster_vs`
//! (`crate::raster_vs`) is RT64's `RasterVS.hlsl` screen-space transform,
//! taking a `RasterVsPosition`. `triangle_to_raster_vs_position` below is the
//! only new logic this module adds: a field-by-field adapter from
//! `TriangleVertex` to `RasterVsPosition`, with no transformation of its own.
//!
//! ## `is_rect` finding
//!
//! `raster_vs`'s `RasterVsParams::is_rect` (`raster_vs.rs`, wire bit 0 of the
//! render-flags word, RT64 `renderFlagRect`,
//! `shared/rt64_render_flags.h:52-54`) gates whether `RasterVS.hlsl` skips
//! the RDP-screen-to-NDC conversion (`RasterVS.hlsl:18`,
//! `if (!renderFlagRect(rp.flags))`).
//!
//! Direct read of the pinned RT64 source (commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`, `docs/RT64-PORT-AUTHORITY.md`)
//! proves a triangle-sourced draw is **always** `is_rect == false`:
//! `rt64_state.cpp:1000` sets `flags.rect = (proj.type ==
//! Projection::Type::Rectangle)`, and `Projection::Type` is set at exactly
//! two, mutually exclusive call sites: `rt64_rdp.cpp:1110`
//! (`fbPair.changeProjection(0, Projection::Type::Triangle)`, inside the
//! triangle draw path that receives `decodeTriangles`' vertex output) and
//! `rt64_rdp.cpp:1211` (`fbPair.changeProjection(0,
//! Projection::Type::Rectangle)`, inside `RDP::drawRect`, the fill/texture
//! rectangle path). No code path sets `Projection::Type::Rectangle` from a
//! triangle draw, and no code path sets `Projection::Type::Triangle` from a
//! rect draw. `crate::raw_dpc::mod.rs`'s own `decode_stream` mirrors this
//! split at the wire level: opcodes `0x08..=0x0f` decode to
//! `RawDpcCommandKind::RawTriangle`, while fill/texture rectangles decode to
//! their own, disjoint `RawDpcCommandKind` variants. This module therefore
//! always passes `is_rect: false` when constructing `RasterVsParams` for a
//! `RawTriangle`-sourced vertex, and documents that as a hard invariant of
//! the composition rather than an assumed default.
//!
//! ## Explicit nonclaims
//!
//! No rasterization, no pixel output, no production-dispatch admission, no
//! draw-path wiring beyond this isolated composition, no RT64 parity or
//! performance claim, no claim about triangle edge-walk/fill semantics.

use super::triangle::RawTriangle;
use super::triangle_vertices::{decode_triangle_vertices, TriangleVertex, TriangleVertices};
use crate::raster_vs::{raster_vs, RasterVsParams, RasterVsPosition};
use crate::state::{OtherMode, PrimDepth};

/// Adapts one decoded `TriangleVertex` to `raster_vs`'s `RasterVsPosition`
/// input. Both types already carry an `x`/`y`/`z`/`w` position in the same
/// units (`decode_triangle_vertices`'s module doc: RDP screen-pixel `x`/`y`,
/// RDP depth `z`, perspective `w`); this is a field rename only, no
/// arithmetic.
const fn triangle_vertex_to_raster_vs_position(vertex: TriangleVertex) -> RasterVsPosition {
    RasterVsPosition {
        x: vertex.x(),
        y: vertex.y(),
        z: vertex.z(),
        w: vertex.w(),
    }
}

/// Composes `RawTriangle::decode` -> `decode_triangle_vertices` ->
/// `raster_vs` for one already-decoded triangle, running the vertex
/// transform on all three of its vertices.
///
/// `is_rect` is not a caller-supplied parameter: per the module doc's
/// `is_rect` finding, a `RawTriangle`-sourced draw is always `is_rect ==
/// false`, so this function fixes that field rather than exposing a
/// parameter that could only ever be called with one value.
pub fn compose_triangle_through_raster_vs(
    triangle: &RawTriangle,
    texture_perspective: bool,
    other_mode: OtherMode,
    prim_depth: PrimDepth,
    resolution: crate::raster_vs::Resolution,
    screen: crate::raster_vs::ScreenTransform,
) -> [RasterVsPosition; 3] {
    let vertices: TriangleVertices = decode_triangle_vertices(triangle, texture_perspective);
    let params = RasterVsParams {
        is_rect: false,
        other_mode,
        prim_depth,
    };
    core::array::from_fn(|index| {
        let vertex = vertices.vertex(index);
        let position = triangle_vertex_to_raster_vs_position(vertex);
        raster_vs(position, resolution, screen, params)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster_vs::{Resolution, ScreenTransform};

    fn word_bytes(w0: u32, w1: u32) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes[0..4].copy_from_slice(&w0.to_be_bytes());
        bytes[4..8].copy_from_slice(&w1.to_be_bytes());
        bytes
    }

    fn base_word0_bytes(
        tile: u32,
        level: u32,
        right_major: bool,
        yl: u16,
        ym: u16,
        yh: u16,
    ) -> [u8; 8] {
        let w0 =
            (tile & 0x7) << 16 | (level & 0x7) << 19 | u32::from(right_major) << 23 | u32::from(yl);
        let w1 = u32::from(ym) << 16 | u32::from(yh);
        word_bytes(w0, w1)
    }

    fn edge_bytes(x: i32, dxdy: i32) -> [u8; 8] {
        word_bytes(x as u32, dxdy as u32)
    }

    fn identity_screen() -> (Resolution, ScreenTransform) {
        (
            Resolution {
                width: 320.0,
                height: 240.0,
            },
            ScreenTransform {
                scale: [1.0, 1.0],
                offset: [0.0, 0.0],
            },
        )
    }

    /// The full chain -- decode, vertex conversion, raster transform -- must
    /// run to completion without panicking on one concrete non-shaded,
    /// non-textured, non-depth triangle (opcode `0x08`), the simplest
    /// admitted triangle command.
    #[test]
    fn base_triangle_composes_through_raster_vs_without_panicking() {
        let bytes: Vec<u8> = [
            base_word0_bytes(0, 0, false, 0x0080, 0x0100, 0x0000),
            edge_bytes(0x0010_0000, 0),
            edge_bytes(0x0020_0000, 0x0000_8000),
            edge_bytes(0x0005_0000, 0),
        ]
        .into_iter()
        .flatten()
        .collect();
        let triangle = RawTriangle::decode(0x08, &bytes).expect("valid 0x08 triangle command");

        let other_mode = OtherMode::from_wire(0, 0);
        let prim_depth = PrimDepth::from_wire(0);
        let (resolution, screen) = identity_screen();

        let transformed = compose_triangle_through_raster_vs(
            &triangle, false, other_mode, prim_depth, resolution, screen,
        );

        for position in transformed {
            assert!(position.x.is_finite(), "x must be finite: {position:?}");
            assert!(position.y.is_finite(), "y must be finite: {position:?}");
            assert!(position.z.is_finite(), "z must be finite: {position:?}");
            assert!(position.w.is_finite(), "w must be finite: {position:?}");
        }
    }

    /// Fully-populated triangle (shade + texture + depth, opcode `0x0f`)
    /// composes end to end too -- proves the chain does not depend on any
    /// optional block being absent.
    #[test]
    fn fully_populated_triangle_composes_through_raster_vs() {
        let filler = |seed: u32| -> Vec<u8> {
            (0..8)
                .flat_map(|index| {
                    word_bytes(seed.wrapping_add(index), seed.wrapping_add(index * 2))
                })
                .collect()
        };
        let mut bytes: Vec<u8> = [
            base_word0_bytes(2, 1, false, 0x0100, 0x0200, 0x0000),
            edge_bytes(0x0010_0000, 0),
            edge_bytes(0x0020_0000, 0x0000_4000),
            edge_bytes(0x0008_0000, 0),
        ]
        .into_iter()
        .flatten()
        .collect();
        bytes.extend(filler(0x0001_0000));
        bytes.extend(filler(0x0002_0000));
        bytes.extend(word_bytes(0x0001_0000, 0x0000_0100));
        bytes.extend(word_bytes(0x0000_1000, 0x0000_0000));

        let triangle = RawTriangle::decode(0x0f, &bytes).expect("valid 0x0f triangle command");

        let other_mode = OtherMode::from_wire(0, 0);
        let prim_depth = PrimDepth::from_wire(0);
        let (resolution, screen) = identity_screen();

        let transformed = compose_triangle_through_raster_vs(
            &triangle, true, other_mode, prim_depth, resolution, screen,
        );

        for position in transformed {
            assert!(position.x.is_finite(), "x must be finite: {position:?}");
            assert!(position.y.is_finite(), "y must be finite: {position:?}");
            assert!(position.z.is_finite(), "z must be finite: {position:?}");
            assert!(position.w.is_finite(), "w must be finite: {position:?}");
        }
    }

    /// `compose_triangle_through_raster_vs` always constructs `is_rect:
    /// false` -- proven indirectly here by confirming the NDC conversion
    /// branch actually ran: with a non-trivial resolution and a nonzero
    /// input x/y, the output must differ from the raw decoded vertex
    /// position (an `is_rect: true` params value would skip that
    /// conversion, per `raster_vs.rs`'s documented `is_rect` branch, and the
    /// screen-space output would instead equal the untransformed decoded
    /// position within scale/offset only).
    #[test]
    fn composition_always_takes_the_non_rect_ndc_conversion_path() {
        let bytes: Vec<u8> = [
            base_word0_bytes(0, 0, false, 0x0080, 0x0100, 0x0000),
            edge_bytes(0x0010_0000, 0),
            edge_bytes(0x0020_0000, 0),
            edge_bytes(0x0005_0000, 0),
        ]
        .into_iter()
        .flatten()
        .collect();
        let triangle = RawTriangle::decode(0x08, &bytes).expect("valid 0x08 triangle command");

        let other_mode = OtherMode::from_wire(0, 0);
        let prim_depth = PrimDepth::from_wire(0);
        let resolution = Resolution {
            width: 320.0,
            height: 240.0,
        };
        let screen = ScreenTransform {
            scale: [1.0, 1.0],
            offset: [0.0, 0.0],
        };

        let decoded = decode_triangle_vertices(&triangle, false);
        let raw_vertex_0 = decoded.vertex(0);

        let transformed = compose_triangle_through_raster_vs(
            &triangle, false, other_mode, prim_depth, resolution, screen,
        );

        // Under is_rect=false with a nonzero resolution, raster_vs subtracts
        // and divides by resolution/2 before writing x/y -- a screen-pixel
        // x of 32.0 (xh=0x0020_0000 -> 32.0) against a 320-wide framebuffer
        // must not survive unchanged.
        assert_ne!(
            transformed[0].x,
            raw_vertex_0.x(),
            "NDC conversion must have run: transformed x must differ from raw decoded x"
        );
    }
}
