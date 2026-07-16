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
    BlendAlphaInput, BlendBInput, BlendColorInput, BlenderState, CullMode, Triangle, Vertex,
};

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

    fn set_blended(
        &mut self,
        x: i32,
        y: i32,
        rgba: [u8; 4],
        shade_alpha: u8,
        blender: BlenderState,
    ) {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return;
        }
        let idx = (y as u32 * self.width + x as u32) as usize * 4;
        let dst = self.pixels[idx..idx + 4].try_into().unwrap();
        let out = blend_fragment(rgba, dst, shade_alpha, blender);
        self.pixels[idx..idx + 4].copy_from_slice(&out);
    }

    /// Depth-tested pixel write: pass iff `z` is strictly nearer (less than)
    /// the stored depth. On pass, write the color AND the new depth. This is
    /// the standard "less-than passes, nearer wins" z-compare
    /// (`F3DEX2-CONCEPTS.md` §4.3). Returns whether the write happened (used
    /// only by tests to assert occlusion behavior).
    #[cfg(test)]
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

    fn set_depth_tested_blended(
        &mut self,
        x: i32,
        y: i32,
        z: f32,
        rgba: [u8; 4],
        shade_alpha: u8,
        blender: BlenderState,
    ) -> bool {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return false;
        }
        let pix = (y as u32 * self.width + x as u32) as usize;
        if z < self.depth[pix] {
            self.depth[pix] = z;
            let idx = pix * 4;
            let dst = self.pixels[idx..idx + 4].try_into().unwrap();
            let out = blend_fragment(rgba, dst, shade_alpha, blender);
            self.pixels[idx..idx + 4].copy_from_slice(&out);
            #[cfg(not(test))]
            zstat::note_pass();
            true
        } else {
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
                    let mut r = (w0 * a.r as f32 + w1 * b.r as f32 + w2 * c.r as f32) as u8;
                    let mut g = (w0 * a.g as f32 + w1 * b.g as f32 + w2 * c.g as f32) as u8;
                    let mut bl = (w0 * a.b as f32 + w1 * b.b as f32 + w2 * c.b as f32) as u8;
                    let shade_alpha = (w0 * a.a as f32 + w1 * b.a as f32 + w2 * c.a as f32) as u8;
                    let mut al = shade_alpha;
                    // If a texture is bound, sample it at the interpolated S/T
                    // and MODULATE (texel * shade / 255) per channel -- the
                    // default OoT combiner (F3DEX2-CONCEPTS.md §5.2). Screen-
                    // linear S/T interpolation (perspective-incorrect, §4.1);
                    // adequate for a first recognizable textured frame.
                    if let Some(tex) = &tri.texture {
                        let s = w0 * a.s + w1 * b.s + w2 * c.s;
                        let t = w0 * a.t + w1 * b.t + w2 * c.t;
                        let [tr, tg, tb, ta] = tex.sample(s, t);
                        r = ((tr as u32 * r as u32) / 255) as u8;
                        g = ((tg as u32 * g as u32) / 255) as u8;
                        bl = ((tb as u32 * bl as u32) / 255) as u8;
                        al = ((ta as u32 * al as u32) / 255) as u8;
                    }
                    if depth_test {
                        // Screen-linear depth interpolation (perspective-
                        // incorrect, adequate for occlusion -- §4.1/§4.3).
                        let z = w0 * a.z + w1 * b.z + w2 * c.z;
                        self.set_depth_tested_blended(
                            x,
                            y,
                            z,
                            [r, g, bl, al],
                            shade_alpha,
                            tri.blender,
                        );
                    } else {
                        self.set_blended(x, y, [r, g, bl, al], shade_alpha, tri.blender);
                    }
                }
            }
        }
    }
}

/// Evaluate the RDP blender selectors for one covered fragment. The public
/// GBI defines each cycle as `P*A + M*B` (`GBL_c1`/`GBL_c2`, gbi.h:612-627).
/// In a second cycle, `G_BL_CLR_IN` names the first cycle's blender result;
/// the framebuffer selector always names the pre-fragment destination.
/// RT64 models the same selector ordering and sequential cycle handoff in
/// `shared/rt64_blender.h:68-81,366-504`.
fn blend_fragment(src: [u8; 4], dst: [u8; 4], shade_alpha: u8, state: BlenderState) -> [u8; 4] {
    if state.cycle_count == 0 {
        return src;
    }

    let src_rgb = [src[0] as f32, src[1] as f32, src[2] as f32];
    let mut blender_rgb = src_rgb;
    let mut final_alpha = 1.0;

    for cycle_index in 0..state.cycle_count.min(2) as usize {
        let cycle = state.cycles[cycle_index];
        let is_last = cycle_index + 1 == state.cycle_count as usize;

        // Without FORCE_BL the last blender cycle is bypassed and selects P;
        // in two-cycle mode cycle 1 still runs (the standard fog-then-pass
        // arrangement). RT64's cycle count/bypass has the same structure at
        // shared/rt64_blender.h:45-65,370-383.
        if is_last && !state.force_blend {
            blender_rgb = blend_color(cycle.p, src_rgb, dst, state, blender_rgb, cycle_index);
            final_alpha = if cycle.p == BlendColorInput::Framebuffer {
                0.0
            } else {
                1.0
            };
            continue;
        }

        let a = blend_a(cycle.a, src[3], shade_alpha, state.fog_color[3]);
        let p = blend_color(cycle.p, src_rgb, dst, state, blender_rgb, cycle_index);
        let m = blend_color(cycle.m, src_rgb, dst, state, blender_rgb, cycle_index);

        // RT64 emits framebuffer terms through dual-source alpha blending
        // (`rt64_blender.h:414-424`; `rt64_raster_shader.cpp:332-339`). This
        // software target performs that final composite here instead: the
        // non-framebuffer input becomes the source color and A becomes its
        // source-alpha factor.
        if cycle.p == BlendColorInput::Framebuffer {
            blender_rgb = m;
            final_alpha = 1.0 - a;
        } else if cycle.m == BlendColorInput::Framebuffer {
            blender_rgb = p;
            final_alpha = a;
        } else {
            let b = blend_b(cycle.b, a);
            if a == 0.0 {
                blender_rgb = m;
            } else if b == 0.0 {
                blender_rgb = p;
            } else {
                let divisor = a + b;
                for channel in 0..3 {
                    blender_rgb[channel] =
                        ((p[channel] * a + m[channel] * b) / divisor).clamp(0.0, 255.0);
                }
            }
            final_alpha = 1.0;
        }
    }

    let mut out_rgb = [0u8; 3];
    for channel in 0..3 {
        out_rgb[channel] = (blender_rgb[channel] * final_alpha
            + dst[channel] as f32 * (1.0 - final_alpha))
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    let alpha = (255.0 * final_alpha + dst[3] as f32 * (1.0 - final_alpha))
        .round()
        .clamp(0.0, 255.0) as u8;
    [out_rgb[0], out_rgb[1], out_rgb[2], alpha]
}

fn blend_color(
    input: BlendColorInput,
    src_rgb: [f32; 3],
    dst: [u8; 4],
    state: BlenderState,
    blender_rgb: [f32; 3],
    cycle_index: usize,
) -> [f32; 3] {
    match input {
        BlendColorInput::Combined if cycle_index == 0 => src_rgb,
        BlendColorInput::Combined => blender_rgb,
        BlendColorInput::Framebuffer => [dst[0] as f32, dst[1] as f32, dst[2] as f32],
        BlendColorInput::Blend => [
            state.blend_color[0] as f32,
            state.blend_color[1] as f32,
            state.blend_color[2] as f32,
        ],
        BlendColorInput::Fog => [
            state.fog_color[0] as f32,
            state.fog_color[1] as f32,
            state.fog_color[2] as f32,
        ],
    }
}

fn blend_a(input: BlendAlphaInput, combined: u8, shade: u8, fog: u8) -> f32 {
    let value = match input {
        BlendAlphaInput::Combined => combined,
        BlendAlphaInput::Fog => fog,
        BlendAlphaInput::Shade => shade,
        BlendAlphaInput::Zero => 0,
    };
    value as f32 / 255.0
}

fn blend_b(input: BlendBInput, a: f32) -> f32 {
    match input {
        BlendBInput::OneMinusA => 1.0 - a,
        // Coverage is not represented by this RGBA framebuffer, so use full
        // coverage exactly as RT64 does (`rt64_blender.h:351-357`).
        BlendBInput::FramebufferAlpha => 1.0,
        BlendBInput::One => 1.0,
        BlendBInput::Zero => 0.0,
    }
}

fn edge(a: Vertex, b: Vertex, c: Vertex) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gbi::BlendCycle;

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

    fn standard_alpha_blender(cycle_count: u8) -> BlenderState {
        let cycle = BlendCycle {
            p: BlendColorInput::Combined,
            a: BlendAlphaInput::Combined,
            m: BlendColorInput::Framebuffer,
            b: BlendBInput::OneMinusA,
        };
        BlenderState {
            cycle_count,
            force_blend: true,
            cycles: [cycle; 2],
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

    /// Fails against the overwrite bug: a half-alpha red fragment used to
    /// replace the blue framebuffer with `[255,0,0,128]`. The standard OoT
    /// XLU tuple must evaluate IN*A_IN + MEM*(1-A), retaining both colors.
    #[test]
    fn translucent_triangle_composites_over_existing_framebuffer() {
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(0, 0, 255, 255);
        let tri = Triangle {
            v: [
                v(2.0, 2.0, 255, 0, 0, 128),
                v(12.0, 2.0, 255, 0, 0, 128),
                v(7.0, 12.0, 255, 0, 0, 128),
            ],
            blender: standard_alpha_blender(1),
            ..Default::default()
        };
        fb.draw_triangle(&tri);
        let idx = (6u32 * fb.width + 7u32) as usize * 4;
        // Barycentric interpolation truncates the nominal 128 alpha to 127 at
        // this sample, so the exact source-over result is 127 red / 128 blue.
        assert_eq!(&fb.pixels[idx..idx + 4], &[127, 0, 128, 255]);
    }

    /// Cycle 2 consumes cycle 1's blender result, not the original combined
    /// color. Reusing the original red source in cycle 2 would produce
    /// `[128,0,127]` rather than retaining cycle 1's green contribution.
    #[test]
    fn two_cycle_blender_feeds_cycle_one_result_into_cycle_two() {
        let state = BlenderState {
            cycle_count: 2,
            force_blend: true,
            cycles: [
                BlendCycle {
                    p: BlendColorInput::Combined,
                    a: BlendAlphaInput::Combined,
                    m: BlendColorInput::Blend,
                    b: BlendBInput::OneMinusA,
                },
                BlendCycle {
                    p: BlendColorInput::Combined,
                    a: BlendAlphaInput::Combined,
                    m: BlendColorInput::Fog,
                    b: BlendBInput::OneMinusA,
                },
            ],
            blend_color: [0, 255, 0, 255],
            fog_color: [0, 0, 255, 255],
        };
        assert_eq!(
            blend_fragment([255, 0, 0, 128], [0, 0, 0, 255], 128, state),
            [64, 64, 127, 255]
        );
    }

    /// The common two-cycle fog arrangement blends fog by SHADE alpha in c1,
    /// then uses a non-forced c2 P-input pass. This covers selector sources
    /// beyond the standard framebuffer-alpha tuple.
    #[test]
    fn fog_cycle_then_pass_uses_shade_alpha_and_prior_cycle_color() {
        let fog_then_pass = BlenderState {
            cycle_count: 2,
            force_blend: false,
            cycles: [
                BlendCycle {
                    p: BlendColorInput::Fog,
                    a: BlendAlphaInput::Shade,
                    m: BlendColorInput::Combined,
                    b: BlendBInput::OneMinusA,
                },
                BlendCycle {
                    p: BlendColorInput::Combined,
                    a: BlendAlphaInput::Zero,
                    m: BlendColorInput::Combined,
                    b: BlendBInput::One,
                },
            ],
            fog_color: [255, 0, 0, 255],
            ..Default::default()
        };
        assert_eq!(
            blend_fragment([0, 0, 255, 255], [0, 255, 0, 255], 64, fog_then_pass),
            [64, 0, 191, 255]
        );
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
