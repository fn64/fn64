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

use crate::gbi::{CullMode, Triangle, Vertex};

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
            true
        } else {
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
                    let mut al = (w0 * a.a as f32 + w1 * b.a as f32 + w2 * c.a as f32) as u8;
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
                        self.set_depth_tested(x, y, z, [r, g, bl, al]);
                    } else {
                        self.set(x, y, [r, g, bl, al]);
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
}
