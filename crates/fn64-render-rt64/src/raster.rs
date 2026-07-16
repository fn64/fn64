//! A tiny flat/barycentric-shaded triangle rasterizer writing directly into
//! an RGBA8888 framebuffer. Software, single-threaded, no attempt at
//! matching real RDP fill-convention edge rules bit-for-bit -- this
//! backend's job (per `lib.rs`'s module doc) is proving the render seam
//! end-to-end with a real, inspectable, non-clear frame, not bit-exact RDP
//! emulation (`docs/DESIGN.md`/project-wide stance: math verifies data,
//! eyes/fixtures verify the visible outcome -- there is no hardware to
//! diff a software rasterizer's exact scanline decisions against here
//! anyway, since real RDP silicon isn't part of this project's evidence
//! base).

use crate::gbi::{
    AlphaSource, ColorSource, CombinerCycle, CombinerState, CullMode, Triangle, Vertex,
};

/// Evaluate both programmed RDP color-combiner cycles.
///
/// Each cycle computes `(A - B) * C + D` independently for RGB and alpha.
/// The source meanings follow RT64's MIT `shared/rt64_color_combiner.h`
/// `fromColorInput`/`fromAlphaInput` (lines 468-540), and the equation/cycle
/// ordering follows `runCycle` (lines 567-608). Running both cycles is a
/// useful superset until the separately-owned other-mode job supplies the
/// hardware cycle-type bit: when cycle 2 ignores COMBINED, cycle 1 cannot
/// affect the result; OoT's PASS2/`*2` presets consume COMBINED and therefore
/// get the required two-cycle result now.
fn evaluate_combiner(state: CombinerState, shade: [u8; 4], texel0: [u8; 4]) -> [u8; 4] {
    let to_unit = |rgba: [u8; 4]| rgba.map(|v| v as f32 / 255.0);
    let mut inputs = CombinerInputs {
        combined: [0.0; 4],
        // This first texture slice has one decoded tile. TEXEL1 aliases it
        // until the TMEM/two-tile follow-up exists; all required OoT presets
        // here use TEXEL0, while the enum preserves TEXEL1's real identity.
        texel0: to_unit(texel0),
        texel1: to_unit(texel0),
        primitive: to_unit(state.primitive),
        shade: to_unit(shade),
        environment: to_unit(state.environment),
        lod_fraction: 0.0,
        prim_lod_fraction: state.prim_lod_fraction as f32 / 255.0,
    };

    for cycle in state.mode.cycles {
        inputs.combined = evaluate_cycle(cycle, &inputs);
    }

    inputs
        .combined
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

#[derive(Copy, Clone)]
struct CombinerInputs {
    combined: [f32; 4],
    texel0: [f32; 4],
    texel1: [f32; 4],
    primitive: [f32; 4],
    shade: [f32; 4],
    environment: [f32; 4],
    lod_fraction: f32,
    prim_lod_fraction: f32,
}

fn evaluate_cycle(cycle: CombinerCycle, inputs: &CombinerInputs) -> [f32; 4] {
    let a = color_input(cycle.rgb[0], inputs);
    let b = color_input(cycle.rgb[1], inputs);
    let c = color_input(cycle.rgb[2], inputs);
    let d = color_input(cycle.rgb[3], inputs);
    let mut out = [0.0; 4];
    for channel in 0..3 {
        out[channel] = (a[channel] - b[channel]) * c[channel] + d[channel];
    }

    let aa = alpha_input(cycle.alpha[0], inputs);
    let ab = alpha_input(cycle.alpha[1], inputs);
    let ac = alpha_input(cycle.alpha[2], inputs);
    let ad = alpha_input(cycle.alpha[3], inputs);
    out[3] = (aa - ab) * ac + ad;
    out
}

fn color_input(source: ColorSource, inputs: &CombinerInputs) -> [f32; 3] {
    let rgb = |rgba: [f32; 4]| [rgba[0], rgba[1], rgba[2]];
    let splat = |v| [v; 3];
    match source {
        ColorSource::Combined => rgb(inputs.combined),
        ColorSource::Texel0 => rgb(inputs.texel0),
        ColorSource::Texel1 => rgb(inputs.texel1),
        ColorSource::Primitive => rgb(inputs.primitive),
        ColorSource::Shade => rgb(inputs.shade),
        ColorSource::Environment => rgb(inputs.environment),
        ColorSource::CombinedAlpha => splat(inputs.combined[3]),
        ColorSource::Texel0Alpha => splat(inputs.texel0[3]),
        ColorSource::Texel1Alpha => splat(inputs.texel1[3]),
        ColorSource::PrimitiveAlpha => splat(inputs.primitive[3]),
        ColorSource::ShadeAlpha => splat(inputs.shade[3]),
        ColorSource::EnvironmentAlpha => splat(inputs.environment[3]),
        ColorSource::LodFraction => splat(inputs.lod_fraction),
        ColorSource::PrimLodFraction => splat(inputs.prim_lod_fraction),
        ColorSource::One => [1.0; 3],
        ColorSource::Zero => [0.0; 3],
        // Keying/conversion/noise registers are outside this task's state
        // slice. None of the required OoT presets uses them; retaining named
        // variants prevents their wire values from being mistaken for ZERO.
        ColorSource::KeyCenter
        | ColorSource::KeyScale
        | ColorSource::Noise
        | ColorSource::K4
        | ColorSource::K5 => [0.0; 3],
    }
}

fn alpha_input(source: AlphaSource, inputs: &CombinerInputs) -> f32 {
    match source {
        AlphaSource::Combined => inputs.combined[3],
        AlphaSource::Texel0 => inputs.texel0[3],
        AlphaSource::Texel1 => inputs.texel1[3],
        AlphaSource::Primitive => inputs.primitive[3],
        AlphaSource::Shade => inputs.shade[3],
        AlphaSource::Environment => inputs.environment[3],
        AlphaSource::LodFraction => inputs.lod_fraction,
        AlphaSource::PrimLodFraction => inputs.prim_lod_fraction,
        AlphaSource::One => 1.0,
        AlphaSource::Zero => 0.0,
    }
}

/// TEMP instrumentation (env `OOT_DUMP_PROJ=1`): count z-test passes vs
/// rejections so a real overlapping-geometry frame can PROVE the z-buffer is
/// doing occlusion work (rejecting farther fragments) rather than being a
/// no-op. Gated entirely behind the env var; call `zstat::summary()` after a
/// frame to print + reset. Remove/keep behind the flag.
#[cfg(not(test))]
pub mod zstat {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static INIT: AtomicBool = AtomicBool::new(false);
    static PASS: AtomicU64 = AtomicU64::new(0);
    static REJECT: AtomicU64 = AtomicU64::new(0);
    fn on() -> bool {
        if !INIT.swap(true, Ordering::Relaxed) {
            ENABLED.store(std::env::var("OOT_DUMP_PROJ").is_ok(), Ordering::Relaxed);
        }
        ENABLED.load(Ordering::Relaxed)
    }
    pub fn note_pass() {
        if on() {
            PASS.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn note_reject() {
        if on() {
            REJECT.fetch_add(1, Ordering::Relaxed);
        }
    }
    /// Print the frame's pass/reject counts and reset for the next frame.
    pub fn summary() {
        if !on() {
            return;
        }
        let p = PASS.swap(0, Ordering::Relaxed);
        let r = REJECT.swap(0, Ordering::Relaxed);
        if p + r > 0 {
            eprintln!(
                "[OOT_DUMP_PROJ] z-test: {p} passes (fragment written) | {r} rejects \
                 (farther fragment occluded) -- rejects>0 proves the z-buffer is \
                 doing real occlusion, not a no-op"
            );
        }
    }
}

pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    /// RGBA8888, row-major, top-left origin.
    pub pixels: Vec<u8>,
    /// Per-pixel depth buffer (screen-space `z`, nearer = smaller). Parallel
    /// to `pixels` (one `f32` per pixel). Initialized to `f32::INFINITY` by
    /// `clear`, so the first fragment at any pixel always passes the
    /// less-than test. This gives correct occlusion (far geometry no longer
    /// overpaints near geometry regardless of draw order -- the overlap/
    /// ordering artifact called out in the milestone). See
    /// `F3DEX2-CONCEPTS.md` §4.3.
    pub depth: Vec<f32>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Framebuffer {
            width,
            height,
            pixels: vec![0u8; (width * height * 4) as usize],
            depth: vec![f32::INFINITY; (width * height) as usize],
        }
    }

    pub fn clear(&mut self, r: u8, g: u8, b: u8, a: u8) {
        for px in self.pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[r, g, b, a]);
        }
        for d in self.depth.iter_mut() {
            *d = f32::INFINITY;
        }
    }

    /// True if any pixel differs from a uniform `(r,g,b,a)` fill -- the
    /// honest "did this frame actually render geometry, not just a clear"
    /// check the task requires (`first_frame`'s whole point).
    pub fn has_non_uniform_content(&self, r: u8, g: u8, b: u8, a: u8) -> bool {
        self.pixels.chunks_exact(4).any(|px| px != [r, g, b, a])
    }

    fn set(&mut self, x: i32, y: i32, rgba: [u8; 4]) {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return;
        }
        let idx = (y as u32 * self.width + x as u32) as usize * 4;
        self.pixels[idx..idx + 4].copy_from_slice(&rgba);
    }

    /// Depth-tested pixel write: pass iff `z` is strictly nearer (less than)
    /// the stored depth. On pass, write the color AND the new depth. This is
    /// the standard "less-than passes, nearer wins" z-compare
    /// (`F3DEX2-CONCEPTS.md` §4.3). Returns whether the write happened (used
    /// only by tests to assert occlusion behavior).
    fn set_depth_tested(&mut self, x: i32, y: i32, z: f32, rgba: [u8; 4]) -> bool {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return false;
        }
        let pix = (y as u32 * self.width + x as u32) as usize;
        if z < self.depth[pix] {
            self.depth[pix] = z;
            self.pixels[pix * 4..pix * 4 + 4].copy_from_slice(&rgba);
            #[cfg(not(test))]
            zstat::note_pass();
            true
        } else {
            // A farther (or equal) fragment landed on an already-written
            // pixel and was correctly discarded -- the actual occlusion work
            // the z-buffer does. Counted (env-gated) to PROVE, on a real
            // overlapping frame, that depth is doing meaningful rejection and
            // not a no-op. See `OOT_DUMP_PROJ` in gbi.rs.
            #[cfg(not(test))]
            zstat::note_reject();
            false
        }
    }

    /// Rasterize one flat/interpolated-color triangle with no culling and no
    /// depth test -- the original textbook edge-function (Pineda 1988-style)
    /// scan, kept for the depth-free reference/fixture path and tests that
    /// assert pure 2D fill. `draw_triangle_culled` layers culling + z-test on
    /// top for the real F3DEX2 scene path.
    pub fn draw_triangle(&mut self, tri: &Triangle) {
        self.draw_triangle_impl(tri, CullMode::None, false);
    }

    /// Rasterize with F3DEX2 back/front-face culling (by screen-space signed
    /// area / winding, `F3DEX2-CONCEPTS.md` §2.4/§4.2) and z-buffering
    /// (§4.3). This is the path the real OoT scene uses so far geometry is
    /// occluded correctly and inside-out back faces don't overpaint front
    /// faces.
    pub fn draw_triangle_culled(&mut self, tri: &Triangle, cull: CullMode) {
        self.draw_triangle_impl(tri, cull, true);
    }

    /// Same culling as [`draw_triangle_culled`] but with NO depth test
    /// (submission/painter's order). Used only by the `OOT_NO_DEPTH` A/B
    /// instrumentation to prove that correct occlusion comes from the
    /// z-buffer, not draw order.
    pub fn draw_triangle_no_depth_culled(&mut self, tri: &Triangle, cull: CullMode) {
        self.draw_triangle_impl(tri, cull, false);
    }

    fn draw_triangle_impl(&mut self, tri: &Triangle, cull: CullMode, depth_test: bool) {
        let [a, b, c] = tri.v;
        let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i32;
        let max_x = a.x.max(b.x).max(c.x).ceil().min(self.width as f32) as i32;
        let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as i32;
        let max_y = a.y.max(b.y).max(c.y).ceil().min(self.height as f32) as i32;

        let area = edge(a, b, c);
        if area == 0.0 {
            return; // degenerate triangle: zero screen-space area.
        }

        // Back/front-face cull by the sign of the screen-space signed area.
        // N64 screen Y is top-down (see project_vertex's Y-flip), which makes
        // a front-facing (CCW-in-model) triangle come out with a NEGATIVE
        // signed area under this `edge` convention; that is the "front" sign
        // here, so `G_CULL_BACK` drops POSITIVE-area triangles. If culling
        // ever removes the wrong half, this sign is the knob (§2.4).
        let culled = match cull {
            CullMode::None => false,
            CullMode::Back => area > 0.0,
            CullMode::Front => area < 0.0,
            CullMode::Both => true,
        };
        if culled {
            return;
        }

        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = Vertex {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                    ..Default::default()
                };
                let w0 = edge(b, c, p) / area;
                let w1 = edge(c, a, p) / area;
                let w2 = edge(a, b, p) / area;
                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                    // Interpolated (screen-linear) shade color.
                    let shade = [
                        (w0 * a.r as f32 + w1 * b.r as f32 + w2 * c.r as f32) as u8,
                        (w0 * a.g as f32 + w1 * b.g as f32 + w2 * c.g as f32) as u8,
                        (w0 * a.b as f32 + w1 * b.b as f32 + w2 * c.b as f32) as u8,
                        (w0 * a.a as f32 + w1 * b.a as f32 + w2 * c.a as f32) as u8,
                    ];
                    // Screen-linear S/T interpolation (perspective-incorrect,
                    // §4.1) remains adequate for this reference backend. A
                    // missing texture supplies white, so shade/primitive/env-
                    // only formulas and the legacy default MODULATE mode have
                    // neutral TEXEL0 input rather than turning black.
                    let texel = if let Some(tex) = &tri.texture {
                        let s = w0 * a.s + w1 * b.s + w2 * c.s;
                        let t = w0 * a.t + w1 * b.t + w2 * c.t;
                        tex.sample(s, t)
                    } else {
                        [255; 4]
                    };
                    let rgba = evaluate_combiner(tri.combiner, shade, texel);
                    if depth_test {
                        // Screen-linear depth interpolation (perspective-
                        // incorrect, adequate for occlusion -- §4.1/§4.3).
                        let z = w0 * a.z + w1 * b.z + w2 * c.z;
                        self.set_depth_tested(x, y, z, rgba);
                    } else {
                        self.set(x, y, rgba);
                    }
                }
            }
        }
    }
}

fn edge(a: Vertex, b: Vertex, c: Vertex) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycle(rgb: [ColorSource; 4], alpha: [AlphaSource; 4]) -> CombinerCycle {
        CombinerCycle { rgb, alpha }
    }

    fn repeated_state(
        cycle: CombinerCycle,
        primitive: [u8; 4],
        environment: [u8; 4],
    ) -> CombinerState {
        CombinerState {
            mode: crate::gbi::CombinerMode { cycles: [cycle; 2] },
            primitive,
            environment,
            prim_lod_fraction: 0,
        }
    }

    fn v(x: f32, y: f32, r: u8, g: u8, b: u8, a: u8) -> Vertex {
        Vertex {
            x,
            y,
            r,
            g,
            b,
            a,
            ..Default::default()
        }
    }

    #[test]
    fn clear_fills_every_pixel() {
        let mut fb = Framebuffer::new(4, 4);
        fb.clear(10, 20, 30, 255);
        assert!(!fb.has_non_uniform_content(10, 20, 30, 255));
        assert_eq!(&fb.pixels[0..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn combiner_presets_select_decal_primitive_environment_and_shade_sources() {
        // Fail-against-bug: the old rasterizer always returned TEXEL0*SHADE,
        // so every assertion except MODULATE below produced the same wrong
        // color regardless of the decoded primitive/environment registers.
        let shade = [50, 100, 150, 220];
        let texel = [128, 64, 255, 180];

        let shade_only = cycle(
            [
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Shade,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Shade,
            ],
        );
        assert_eq!(
            evaluate_combiner(repeated_state(shade_only, [0; 4], [0; 4]), shade, texel),
            shade
        );

        let decal = cycle(
            [
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Texel0,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Texel0,
            ],
        );
        assert_eq!(
            evaluate_combiner(repeated_state(decal, [0; 4], [0; 4]), shade, texel),
            texel
        );

        let primitive_tint = cycle(
            [
                ColorSource::Texel0,
                ColorSource::Zero,
                ColorSource::Primitive,
                ColorSource::Zero,
            ],
            [
                AlphaSource::Texel0,
                AlphaSource::Zero,
                AlphaSource::Primitive,
                AlphaSource::Zero,
            ],
        );
        let primitive = [128, 255, 64, 128];
        assert_eq!(
            evaluate_combiner(
                repeated_state(primitive_tint, primitive, [0; 4]),
                shade,
                texel,
            ),
            [64, 64, 64, 90]
        );

        // G_CC_BLENDI: (ENVIRONMENT - SHADE) * TEXEL0 + SHADE.
        let env_blend = cycle(
            [
                ColorSource::Environment,
                ColorSource::Shade,
                ColorSource::Texel0,
                ColorSource::Shade,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Shade,
            ],
        );
        assert_eq!(
            evaluate_combiner(
                repeated_state(env_blend, [0; 4], [250, 200, 100, 255]),
                shade,
                texel,
            ),
            [150, 125, 100, 220]
        );
    }

    #[test]
    fn combiner_second_cycle_consumes_first_cycle_combined_result() {
        // Cycle 0: TEXEL0*SHADE. Cycle 1: COMBINED*PRIMITIVE. This fails if
        // only the second programmed tuple is evaluated or COMBINED is not
        // carried between cycles.
        let first = cycle(
            [
                ColorSource::Texel0,
                ColorSource::Zero,
                ColorSource::Shade,
                ColorSource::Zero,
            ],
            [
                AlphaSource::Texel0,
                AlphaSource::Zero,
                AlphaSource::Shade,
                AlphaSource::Zero,
            ],
        );
        let second = cycle(
            [
                ColorSource::Combined,
                ColorSource::Zero,
                ColorSource::Primitive,
                ColorSource::Zero,
            ],
            [
                AlphaSource::Combined,
                AlphaSource::Zero,
                AlphaSource::Primitive,
                AlphaSource::Zero,
            ],
        );
        let state = CombinerState {
            mode: crate::gbi::CombinerMode {
                cycles: [first, second],
            },
            primitive: [128; 4],
            environment: [0; 4],
            prim_lod_fraction: 0,
        };
        assert_eq!(evaluate_combiner(state, [128; 4], [200; 4]), [50; 4]);
    }

    #[test]
    fn triangle_paints_interior_pixels_and_leaves_exterior_clear() {
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        let tri = Triangle {
            v: [
                v(2.0, 2.0, 255, 0, 0, 255),
                v(12.0, 2.0, 255, 0, 0, 255),
                v(7.0, 12.0, 255, 0, 0, 255),
            ],
            ..Default::default()
        };
        fb.draw_triangle(&tri);
        assert!(fb.has_non_uniform_content(0, 0, 0, 255));
        // Centroid should be red.
        let cx = 7u32;
        let cy = 6u32;
        let idx = (cy * fb.width + cx) as usize * 4;
        assert_eq!(&fb.pixels[idx..idx + 4], &[255, 0, 0, 255]);
        // A far corner should remain untouched (still clear color).
        let idx0 = 0usize;
        assert_eq!(&fb.pixels[idx0..idx0 + 4], &[0, 0, 0, 255]);
    }

    #[test]
    fn textured_triangle_modulates_texel_by_shade() {
        use crate::gbi::Texture;
        // 1×1 white texture: modulate leaves the shade color unchanged.
        let white = Texture {
            width: 1,
            height: 1,
            texels: std::rc::Rc::new(vec![255, 255, 255, 255]),
            clamp_s: true,
            clamp_t: true,
        };
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        // Green-shaded triangle, all S/T = 0 (samples the one white texel).
        let mut tri = Triangle {
            v: [
                v(2.0, 2.0, 0, 200, 0, 255),
                v(12.0, 2.0, 0, 200, 0, 255),
                v(7.0, 12.0, 0, 200, 0, 255),
            ],
            ..Default::default()
        };
        tri.texture = Some(white);
        fb.draw_triangle(&tri);
        let idx = (6u32 * fb.width + 7u32) as usize * 4;
        // white(255) * shade(200) / 255 == 200: texture didn't tint the shade.
        assert_eq!(&fb.pixels[idx..idx + 4], &[0, 200, 0, 255]);
    }

    #[test]
    fn textured_triangle_paints_texel_color() {
        use crate::gbi::Texture;
        // 1×1 red texture under a white shade -> red pixel (modulate).
        let red = Texture {
            width: 1,
            height: 1,
            texels: std::rc::Rc::new(vec![255, 0, 0, 255]),
            clamp_s: true,
            clamp_t: true,
        };
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        let mut tri = Triangle {
            v: [
                v(2.0, 2.0, 255, 255, 255, 255),
                v(12.0, 2.0, 255, 255, 255, 255),
                v(7.0, 12.0, 255, 255, 255, 255),
            ],
            ..Default::default()
        };
        tri.texture = Some(red);
        fb.draw_triangle(&tri);
        let idx = (6u32 * fb.width + 7u32) as usize * 4;
        assert_eq!(&fb.pixels[idx..idx + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn degenerate_triangle_paints_nothing() {
        let mut fb = Framebuffer::new(8, 8);
        fb.clear(1, 2, 3, 4);
        let tri = Triangle {
            v: [
                v(1.0, 1.0, 9, 9, 9, 9),
                v(1.0, 1.0, 9, 9, 9, 9),
                v(1.0, 1.0, 9, 9, 9, 9),
            ],
            ..Default::default()
        };
        fb.draw_triangle(&tri);
        assert!(!fb.has_non_uniform_content(1, 2, 3, 4));
    }

    // --- Depth / z-buffer occlusion regression ---------------------------
    //
    // These prove the z-buffer resolves overlapping geometry by DEPTH, not by
    // submission (painter's) order, and in the correct DIRECTION (nearer =
    // smaller `z` wins the `z < depth` compare, matching the OoT viewport z
    // mapping `pz = ndc_z*sz + tz` with sz>0, verified live: sz=tz=127.75,
    // ndc_z↑ with distance -> pz↑ with distance -> nearer has smaller pz).

    /// A vertex with an explicit screen-space depth `z`.
    fn vz(x: f32, y: f32, z: f32, r: u8, g: u8, b: u8, a: u8) -> Vertex {
        Vertex {
            x,
            y,
            z,
            r,
            g,
            b,
            a,
            ..Default::default()
        }
    }

    /// Two fully-overlapping triangles at different depths: a NEAR blue one
    /// (z=1) and a FAR red one (z=9), covering the same pixels. The nearer
    /// (blue) color must survive at the overlap REGARDLESS of the order they
    /// are submitted -- proving z-test, not painter's order.
    #[test]
    fn nearer_triangle_wins_over_farther_regardless_of_submission_order() {
        // Same screen footprint for both; only z (and color) differ.
        let near = Triangle {
            v: [
                vz(2.0, 2.0, 1.0, 0, 0, 255, 255),
                vz(12.0, 2.0, 1.0, 0, 0, 255, 255),
                vz(7.0, 12.0, 1.0, 0, 0, 255, 255),
            ],
            ..Default::default()
        };
        let far = Triangle {
            v: [
                vz(2.0, 2.0, 9.0, 255, 0, 0, 255),
                vz(12.0, 2.0, 9.0, 255, 0, 0, 255),
                vz(7.0, 12.0, 9.0, 255, 0, 0, 255),
            ],
            ..Default::default()
        };
        let overlap = (6u32 * 16 + 7u32) as usize * 4; // interior pixel (7,6)

        // Order A: far first, then near. Near must overwrite far.
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        fb.draw_triangle_culled(&far, CullMode::None);
        fb.draw_triangle_culled(&near, CullMode::None);
        assert_eq!(
            &fb.pixels[overlap..overlap + 4],
            &[0, 0, 255, 255],
            "near (blue) must win at overlap when drawn AFTER far"
        );

        // Order B: near first, then far. Near must STILL win (far is z-rejected).
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        fb.draw_triangle_culled(&near, CullMode::None);
        fb.draw_triangle_culled(&far, CullMode::None);
        assert_eq!(
            &fb.pixels[overlap..overlap + 4],
            &[0, 0, 255, 255],
            "near (blue) must STILL win when drawn BEFORE far -- this is what \
             separates a real z-test from painter's order"
        );
    }

    /// The whole point of a z-buffer over painter's order: WITHOUT the depth
    /// test, submission order decides the overlap (last drawn wins), so the
    /// far triangle drawn last would incorrectly show through. This documents
    /// the difference the z-test makes and would catch a regression that
    /// silently dropped the z-test on the culled path.
    #[test]
    fn without_depth_test_painter_order_lets_farther_show_through() {
        let far = Triangle {
            v: [
                vz(2.0, 2.0, 9.0, 255, 0, 0, 255),
                vz(12.0, 2.0, 9.0, 255, 0, 0, 255),
                vz(7.0, 12.0, 9.0, 255, 0, 0, 255),
            ],
            ..Default::default()
        };
        let near = Triangle {
            v: [
                vz(2.0, 2.0, 1.0, 0, 0, 255, 255),
                vz(12.0, 2.0, 1.0, 0, 0, 255, 255),
                vz(7.0, 12.0, 1.0, 0, 0, 255, 255),
            ],
            ..Default::default()
        };
        let overlap = (6u32 * 16 + 7u32) as usize * 4;
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 0, 255);
        // No-depth path: near first, far last -> far shows through (WRONG for a
        // real scene; this is exactly the artifact the z-buffer removes).
        fb.draw_triangle_no_depth_culled(&near, CullMode::None);
        fb.draw_triangle_no_depth_culled(&far, CullMode::None);
        assert_eq!(
            &fb.pixels[overlap..overlap + 4],
            &[255, 0, 0, 255],
            "without depth test, last-drawn (far/red) wins -- the painter's-order \
             artifact the z-buffer exists to prevent"
        );
    }

    /// Directly proves the z-test DIRECTION. `set_depth_tested` returns
    /// whether it wrote. A nearer z (smaller) must pass over an existing
    /// farther z; a farther z (larger) must be rejected. If the compare were
    /// inverted (`z > depth`), the first assert would fail -- so this test
    /// fails against a sign-flipped z-test bug.
    #[test]
    fn set_depth_tested_passes_nearer_rejects_farther() {
        let mut fb = Framebuffer::new(2, 2);
        fb.clear(0, 0, 0, 255);
        // Write a mid-depth fragment.
        assert!(fb.set_depth_tested(0, 0, 5.0, [1, 1, 1, 1]));
        // A NEARER (smaller z) fragment must PASS and overwrite.
        assert!(
            fb.set_depth_tested(0, 0, 2.0, [2, 2, 2, 2]),
            "nearer z (2 < 5) must pass -- if this fails the z-test is inverted"
        );
        assert_eq!(&fb.pixels[0..4], &[2, 2, 2, 2]);
        // A FARTHER (larger z) fragment must be REJECTED (color unchanged).
        assert!(
            !fb.set_depth_tested(0, 0, 8.0, [3, 3, 3, 3]),
            "farther z (8 > 2) must be rejected"
        );
        assert_eq!(&fb.pixels[0..4], &[2, 2, 2, 2]);
    }
}
