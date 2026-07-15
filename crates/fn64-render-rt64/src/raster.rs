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

use crate::gbi::{Triangle, Vertex};

pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    /// RGBA8888, row-major, top-left origin.
    pub pixels: Vec<u8>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Framebuffer {
            width,
            height,
            pixels: vec![0u8; (width * height * 4) as usize],
        }
    }

    pub fn clear(&mut self, r: u8, g: u8, b: u8, a: u8) {
        for px in self.pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[r, g, b, a]);
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

    /// Rasterize one flat/interpolated-color triangle using a standard
    /// edge-function (barycentric) scan -- textbook technique (Pineda
    /// 1988-style edge functions), not derived from any N64-specific
    /// source. Vertex `(x, y)` are already screen-space pixel coordinates
    /// per `gbi.rs`'s decode step.
    pub fn draw_triangle(&mut self, tri: &Triangle) {
        let [a, b, c] = tri.v;
        let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i32;
        let max_x = a.x.max(b.x).max(c.x).ceil().min(self.width as f32) as i32;
        let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as i32;
        let max_y = a.y.max(b.y).max(c.y).ceil().min(self.height as f32) as i32;

        let area = edge(a, b, c);
        if area == 0.0 {
            return; // degenerate triangle: zero screen-space area.
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
                    let r = (w0 * a.r as f32 + w1 * b.r as f32 + w2 * c.r as f32) as u8;
                    let g = (w0 * a.g as f32 + w1 * b.g as f32 + w2 * c.g as f32) as u8;
                    let bl = (w0 * a.b as f32 + w1 * b.b as f32 + w2 * c.b as f32) as u8;
                    let al = (w0 * a.a as f32 + w1 * b.a as f32 + w2 * c.a as f32) as u8;
                    self.set(x, y, [r, g, bl, al]);
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
        Vertex { x, y, r, g, b, a }
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
    fn degenerate_triangle_paints_nothing() {
        let mut fb = Framebuffer::new(8, 8);
        fb.clear(1, 2, 3, 4);
        let tri = Triangle {
            v: [
                v(1.0, 1.0, 9, 9, 9, 9),
                v(1.0, 1.0, 9, 9, 9, 9),
                v(1.0, 1.0, 9, 9, 9, 9),
            ],
        };
        fb.draw_triangle(&tri);
        assert!(!fb.has_non_uniform_content(1, 2, 3, 4));
    }
}
