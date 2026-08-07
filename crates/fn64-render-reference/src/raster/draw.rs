use crate::gbi::*;
use crate::depth::EncodedDepth;
use super::*;

use super::coverage::*;
use super::combiner::*;
use super::blend::*;

impl Framebuffer {
    pub const DEFAULT_NOISE_SEED: u64 = 0x4e36_3452_4450_4e53;

    pub fn new(width: u32, height: u32) -> Self {
        Framebuffer {
            width,
            height,
            pixels: vec![0u8; (width * height * 4) as usize],
            coverage: vec![Coverage::FULL; (width * height) as usize],
            depth: vec![f32::INFINITY; (width * height) as usize],
            encoded_depth: vec![None; (width * height) as usize],
            primitive_depth: None,
            color_layout: ColorImageLayout::Rgba16,
            noise: NoiseState::default(),
        }
    }

    /// Select the deterministic reference stream used for all RDP noise
    /// inputs. This is a reproducibility policy, not the unpublished hardware
    /// seed or generator.
    pub fn set_noise_seed(&mut self, seed: u64) {
        self.noise.reseed(seed);
    }

    pub(crate) fn resized(&self, width: u32, height: u32) -> Self {
        let mut resized = Self::new(width, height);
        resized.noise = self.noise;
        resized
    }

    /// A clone for VI scanout: copies the buffers the scanout filter chain
    /// reads (`pixels`, `coverage`) and reinitializes the two depth buffers to
    /// their empty-framebuffer values instead of copying them.
    ///
    /// `vi::scanout` and everything it calls never read `depth` or
    /// `encoded_depth`, and no consumer of `presented_framebuffer()` reads
    /// them either -- every one reads `pixels`. Depth is per-pixel `f32` plus a
    /// per-pixel `Option<EncodedDepth>`, so at 320x240 the derived `Clone`
    /// copies 975 KiB of which 600 KiB (62%) is depth state the presented
    /// frame has no meaning for.
    ///
    /// The buffers are RESIZED, not left empty: `Framebuffer`'s invariant is
    /// that all four are parallel to `width * height`, and a public accessor
    /// hands this value out. A shorter vector would turn a depth read into a
    /// panic rather than a wrong answer, but both are worse than paying for
    /// the initialization, which `vec!`/`resize` does without a source read.
    pub(crate) fn cloned_for_scanout(&self) -> Self {
        let pixel_count = self.pixels.len() / 4;
        Framebuffer {
            width: self.width,
            height: self.height,
            pixels: self.pixels.clone(),
            coverage: self.coverage.clone(),
            depth: vec![f32::INFINITY; pixel_count],
            encoded_depth: vec![None; pixel_count],
            primitive_depth: self.primitive_depth,
            color_layout: self.color_layout,
            noise: self.noise,
        }
    }

    #[cfg(test)]
    pub(crate) fn noise_position(&self) -> (u64, u64) {
        (self.noise.seed, self.noise.fragment_index)
    }

    pub fn clear(&mut self, r: u8, g: u8, b: u8, a: u8) {
        for px in self.pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[r, g, b, a]);
        }
        self.coverage.fill(Coverage::FULL);
        for d in self.depth.iter_mut() {
            *d = f32::INFINITY;
        }
        self.encoded_depth.fill(None);
    }

    pub(crate) fn set_primitive_depth(&mut self, primitive_depth: Option<PrimitiveDepth>) {
        self.primitive_depth = primitive_depth;
    }

    pub(crate) fn set_color_layout(&mut self, color_layout: ColorImageLayout) {
        self.color_layout = color_layout;
    }

    pub(crate) fn color_layout(&self) -> ColorImageLayout {
        self.color_layout
    }

    pub(crate) fn coverage_count(&self, pixel: usize) -> u8 {
        self.coverage[pixel].count()
    }

    /// True if any pixel differs from a uniform `(r,g,b,a)` fill -- the
    /// honest "did this frame actually render geometry, not just a clear"
    /// check the task requires (`first_frame`'s whole point).
    pub fn has_non_uniform_content(&self, r: u8, g: u8, b: u8, a: u8) -> bool {
        self.pixels.chunks_exact(4).any(|px| px != [r, g, b, a])
    }

    /// Execute an RDP rectangle against the active public 8-bit, RGBA16, or
    /// RGBA32 color-image format. Fill cycle bypasses the pixel pipeline and
    /// includes the lower/right edge; one/two-cycle mode uses the combiner,
    /// blender, and exclusive lower/right edge.
    pub fn draw_fill_rectangle(&mut self, rect: &FillRectangle, target: ColorImage) {
        let layout = target
            .layout()
            .expect("fill target must be I8/CI8, RGBA16, or RGBA32");
        if rect.cycle_type == CycleType::Fill {
            require_safe_fill_cycle_bypass(rect.other_mode, "G_FILLRECT");
        }
        assert_ne!(
            rect.cycle_type,
            CycleType::Copy,
            "G_FILLRECT in copy cycle has no guaranteed public result; use G_TEXRECT"
        );
        self.color_layout = layout;
        if matches!(rect.cycle_type, CycleType::OneCycle | CycleType::TwoCycle) {
            self.draw_combined_fill_rectangle(rect);
            return;
        }
        debug_assert_eq!(rect.cycle_type, CycleType::Fill);
        let decode_16 = |pixel: u16| {
            let expand = |value: u16| -> u8 {
                let value = value as u8;
                (value << 3) | (value >> 2)
            };
            [
                expand((pixel >> 11) & 0x1f),
                expand((pixel >> 6) & 0x1f),
                expand((pixel >> 1) & 0x1f),
                if pixel & 1 != 0 { 255 } else { 0 },
            ]
        };
        let (colors, coverages, period) = match layout {
            ColorImageLayout::Index8 => {
                let bytes = rect.fill_color.to_be_bytes();
                (
                    bytes.map(|intensity| [intensity, intensity, intensity, 255]),
                    [Coverage::FULL; 4],
                    4,
                )
            }
            ColorImageLayout::Rgba16 => (
                [
                    decode_16((rect.fill_color >> 16) as u16),
                    decode_16(rect.fill_color as u16),
                    decode_16((rect.fill_color >> 16) as u16),
                    decode_16(rect.fill_color as u16),
                ],
                [
                    if (rect.fill_color >> 16) as u16 & 1 != 0 {
                        Coverage::FULL
                    } else {
                        Coverage::new(1)
                    },
                    if rect.fill_color as u16 & 1 != 0 {
                        Coverage::FULL
                    } else {
                        Coverage::new(1)
                    },
                    if (rect.fill_color >> 16) as u16 & 1 != 0 {
                        Coverage::FULL
                    } else {
                        Coverage::new(1)
                    },
                    if rect.fill_color as u16 & 1 != 0 {
                        Coverage::FULL
                    } else {
                        Coverage::new(1)
                    },
                ],
                2,
            ),
            ColorImageLayout::Rgba32 => {
                let [red, green, blue, alpha_coverage] = rect.fill_color.to_be_bytes();
                let coverage = Coverage::from_stored(alpha_coverage >> 5);
                let alpha = (alpha_coverage & 0x1f) << 3 | (alpha_coverage & 0x1f) >> 2;
                let color = [red, green, blue, alpha];
                ([color; 4], [coverage; 4], 1)
            }
        };
        let scissor = rect
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let clip_min_x = (scissor.ulx - 0.5).ceil() as i32;
        let clip_max_x = (scissor.lrx - 0.5).ceil() as i32;
        let clip_min_y = (scissor.uly - 0.5).ceil() as i32;
        let clip_max_y = (scissor.lry - 0.5).ceil() as i32;
        let min_x = (rect.ulx.ceil() as i32).max(clip_min_x).max(0);
        let max_x = (rect.lrx.floor() as i32)
            .min(clip_max_x - 1)
            .min(self.width as i32 - 1);
        let min_y = (rect.uly.ceil() as i32).max(clip_min_y).max(0);
        let max_y = (rect.lry.floor() as i32)
            .min(clip_max_y - 1)
            .min(self.height as i32 - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }

        for y in min_y..=max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            for x in min_x..=max_x {
                let index = (y as u32 * self.width + x as u32) as usize * 4;
                let fill_index = (x as usize) % period;
                self.pixels[index..index + 4].copy_from_slice(&colors[fill_index]);
                self.coverage[index / 4] = coverages[fill_index];
            }
        }
    }

    fn draw_combined_fill_rectangle(&mut self, rect: &FillRectangle) {
        require_supported_alpha_compare(rect.other_mode, "combined G_FILLRECT");
        let scissor = rect
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
        let min_x = pixel_min(rect.ulx).max(pixel_min(scissor.ulx)).max(0);
        let max_x = (pixel_min(rect.lrx) - 1)
            .min(pixel_min(scissor.lrx) - 1)
            .min(self.width as i32 - 1);
        let min_y = pixel_min(rect.uly).max(pixel_min(scissor.uly)).max(0);
        let max_y = (pixel_min(rect.lry) - 1)
            .min(pixel_min(scissor.lry) - 1)
            .min(self.height as i32 - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }

        let depth = DepthControl::from_other_mode(rect.other_mode);

        for y in min_y..=max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            for x in min_x..=max_x {
                let noise = self.noise.next_sample();
                let rgba = evaluate_combiner(
                    rect.combiner,
                    rect.cycle_type,
                    rect.other_mode.combine_key(),
                    CombinerPixel {
                        lod_fraction: 0.0,
                        shade: [0; 4],
                        texel0: [0; 4],
                        texel1: [0; 4],
                        noise,
                    },
                );
                let (rgba, coverage) = apply_coverage_alpha(rect.other_mode, rgba, Coverage::FULL);
                if coverage.count() == 0
                    || !alpha_compare_value(
                        rect.other_mode.alpha_compare(),
                        rgba[3],
                        rect.other_mode.blend_color_alpha,
                        noise,
                    )
                {
                    continue;
                }
                if depth.compare || depth.update {
                    let primitive = self.primitive_depth.expect(
                        "depth-enabled fill rectangle selected primitive Z without G_SETPRIMDEPTH",
                    );
                    let encoded =
                        crate::depth::pack(u32::from(primitive.z & 0x7fff) << 3, primitive.delta_z);
                    self.set_depth_controlled_blended(
                        x,
                        y,
                        DepthFragment {
                            z: (u32::from(primitive.z & 0x7fff) << 3) as f32,
                            delta_z: primitive.delta_z,
                            encoded_depth: Some(encoded),
                            coverage,
                            rgba,
                            shade_alpha: 0,
                            noise,
                        },
                        rect.blender,
                        depth,
                        rect.other_mode,
                    );
                } else {
                    self.set_blended(
                        x,
                        y,
                        ColorFragment {
                            rgba,
                            coverage,
                            shade_alpha: 0,
                            noise,
                        },
                        rect.blender,
                        rect.other_mode,
                    );
                }
            }
        }
    }

    /// Clear software depth samples under a fill directed at the depth image.
    /// The coverage calculation intentionally mirrors `draw_fill_rectangle`.
    pub fn clear_depth_rectangle(&mut self, rect: &FillRectangle) {
        require_safe_fill_cycle_bypass(rect.other_mode, "depth G_FILLRECT");
        let scissor = rect
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let clip_min_x = (scissor.ulx - 0.5).ceil() as i32;
        let clip_max_x = (scissor.lrx - 0.5).ceil() as i32;
        let clip_min_y = (scissor.uly - 0.5).ceil() as i32;
        let clip_max_y = (scissor.lry - 0.5).ceil() as i32;
        let min_x = (rect.ulx.ceil() as i32).max(clip_min_x).max(0);
        let max_x = (rect.lrx.floor() as i32)
            .min(clip_max_x - 1)
            .min(self.width as i32 - 1);
        let min_y = (rect.uly.ceil() as i32).max(clip_min_y).max(0);
        let max_y = (rect.lry.floor() as i32)
            .min(clip_max_y - 1)
            .min(self.height as i32 - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }
        let encoded = [
            EncodedDepth::from_fill_halfword((rect.fill_color >> 16) as u16),
            EncodedDepth::from_fill_halfword(rect.fill_color as u16),
        ];
        for y in min_y..=max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            for x in min_x..=max_x {
                let index = (y as u32 * self.width + x as u32) as usize;
                let sample = encoded[(x as usize) & 1];
                self.depth[index] = crate::depth::unpack(sample).0 as f32;
                self.encoded_depth[index] = Some(sample);
            }
        }
    }

    /// Execute the public GBI copy-cycle texture-rectangle path. Copy mode
    /// includes the lower/right bounds and emits four horizontal texels per
    /// clock, so raw `dsdx = 4<<10` advances one texel per output pixel.
    pub fn draw_copy_texture_rectangle(&mut self, rect: &TextureRectangle) {
        assert_eq!(rect.other_mode.cycle_type(), CycleType::Copy);
        require_supported_alpha_compare(rect.other_mode, "copy-cycle G_TEXRECT");
        let texture = rect
            .texture
            .as_ref()
            .expect("copy texture rectangle reached rasterizer without its tile texture");
        let scissor = rect
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        // Copy mode ignores the two screen-coordinate fraction bits. Its
        // lower/right pixel is included; scissor lower/right remains an
        // exclusive boundary after the caller has checked the documented
        // four-pixel copy-mode restriction.
        let min_x = (rect.ulx.floor() as i32).max(scissor.ulx as i32).max(0);
        let max_x = (rect.lrx.floor() as i32)
            .min(scissor.lrx as i32 - 1)
            .min(self.width as i32 - 1);
        let min_y = (rect.uly.floor() as i32).max(scissor.uly as i32).max(0);
        let max_y = (rect.lry.floor() as i32)
            .min(scissor.lry as i32 - 1)
            .min(self.height as i32 - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }
        let origin_x = rect.ulx.floor();
        let origin_y = rect.uly.floor();
        let ds_per_pixel = rect.dsdx as f32 / 4096.0;
        let dt_per_pixel = rect.dtdy as f32 / 1024.0;
        for y in min_y..=max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            for x in min_x..=max_x {
                let dx = x as f32 - origin_x;
                let dy = y as f32 - origin_y;
                // Public gSPTextureRectangleFlip swaps the screen axes that
                // advance S and T. Copy mode still applies its documented
                // four-texel dsdx encoding, so normalize each field exactly
                // as the non-flipped path before swapping the axes.
                let (s, t) = if rect.flip {
                    (rect.s + dy * ds_per_pixel, rect.t + dx * dt_per_pixel)
                } else {
                    (rect.s + dx * ds_per_pixel, rect.t + dy * dt_per_pixel)
                };
                let sample = texture.sample_copy(s, t);
                let texel = sample.rgba;
                let noise = self.noise.next_sample();
                if !copy_alpha_compare_value(
                    rect.other_mode.alpha_compare(),
                    texture,
                    texel[3],
                    rect.other_mode.blend_color_alpha,
                    noise,
                ) {
                    continue;
                }
                let index = (y as u32 * self.width + x as u32) as usize * 4;
                if self.color_layout == ColorImageLayout::Index8 {
                    // Programming Manual 13.11 and 15.5 define copy as a
                    // direct 8-bit memory transfer after source-format alpha
                    // comparison. In particular, IA8 must retain both packed
                    // nibbles rather than store its expanded intensity lane.
                    let byte = sample.direct_8bit.unwrap_or_else(|| {
                        panic!(
                            "copy-cycle 8-bit target reached rasterizer without a direct source byte (format={} size={})",
                            texture.format, texture.size
                        )
                    });
                    self.pixels[index..index + 4].copy_from_slice(&[byte, byte, byte, texel[3]]);
                } else {
                    self.pixels[index..index + 4].copy_from_slice(&texel);
                }
                self.coverage[index / 4] = Coverage::FULL;
            }
        }
    }

    /// Execute a one/two-cycle texture rectangle through the shared texture
    /// filter, color combiner, alpha compare, and framebuffer blender. The
    /// public command excludes its lower/right edge in these cycle modes;
    /// `G_TEXRECTFLIP` swaps the screen axes driven by S and T.
    pub fn draw_texture_rectangle(&mut self, rect: &TextureRectangle) {
        let cycle_type = rect.other_mode.cycle_type();
        assert!(matches!(
            cycle_type,
            CycleType::OneCycle | CycleType::TwoCycle
        ));
        require_supported_alpha_compare(rect.other_mode, "combined G_TEXRECT");
        let texture0 = rect
            .texture
            .as_ref()
            .expect("texture rectangle reached rasterizer without TEXEL0 tile");
        let scissor = rect
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
        let min_x = pixel_min(rect.ulx).max(pixel_min(scissor.ulx)).max(0);
        let max_x = (pixel_min(rect.lrx) - 1)
            .min(pixel_min(scissor.lrx) - 1)
            .min(self.width as i32 - 1);
        let min_y = pixel_min(rect.uly).max(pixel_min(scissor.uly)).max(0);
        let max_y = (pixel_min(rect.lry) - 1)
            .min(pixel_min(scissor.lry) - 1)
            .min(self.height as i32 - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }

        let origin_x = rect.ulx.floor();
        let origin_y = rect.uly.floor();
        let ds = rect.dsdx as f32 / 1024.0;
        let dt = rect.dtdy as f32 / 1024.0;
        // Loop-invariant sampler inputs. The derivatives come from the
        // rectangle's constant dsdx/dtdy, and whether the combiner reads
        // TEXEL1 is a property of the primitive's combiner mode -- but
        // `uses_texel1` scans up to eight combiner sources, and it was being
        // rescanned for every pixel.
        let derivatives = if rect.flip {
            TextureDerivatives {
                dtdx: dt,
                dsdy: ds,
                ..TextureDerivatives::default()
            }
        } else {
            TextureDerivatives {
                dsdx: ds,
                dtdy: dt,
                ..TextureDerivatives::default()
            }
        };
        let require_texel1 = rect.combiner.mode.uses_texel1(cycle_type);
        for y in min_y..=max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            for x in min_x..=max_x {
                let dx = x as f32 - origin_x;
                let dy = y as f32 - origin_y;
                let (s, t) = if rect.flip {
                    (rect.s + dy * ds, rect.t + dx * dt)
                } else {
                    (rect.s + dx * ds, rect.t + dy * dt)
                };
                let (texel0, texel1, lod_fraction) = texture0.sample_rdp_pair(
                    rect.texture1.as_ref(),
                    TextureSampleRequest {
                        s,
                        t,
                        derivatives,
                        other_mode: rect.other_mode,
                        convert: rect.combiner.convert,
                        min_level: rect.combiner.min_lod_level,
                        require_texel1,
                    },
                );
                // Rectangle commands carry no shade attributes. Validation
                // rejects programs selecting SHADE, so zero is an inert and
                // observable placeholder rather than an invented constant.
                let shade = [0; 4];
                let noise = self.noise.next_sample();
                let rgba = evaluate_combiner(
                    rect.combiner,
                    cycle_type,
                    rect.other_mode.combine_key(),
                    CombinerPixel {
                        lod_fraction,
                        shade,
                        texel0,
                        texel1,
                        noise,
                    },
                );
                let (rgba, coverage) = apply_coverage_alpha(rect.other_mode, rgba, Coverage::FULL);
                if coverage.count() == 0 {
                    continue;
                }
                if !alpha_compare_value(
                    rect.other_mode.alpha_compare(),
                    rgba[3],
                    rect.other_mode.blend_color_alpha,
                    noise,
                ) {
                    continue;
                }
                let depth = DepthControl::from_other_mode(rect.other_mode);
                if depth.compare || depth.update {
                    let primitive = self.primitive_depth.expect(
                        "depth-enabled texture rectangle selected primitive Z without G_SETPRIMDEPTH",
                    );
                    let encoded =
                        crate::depth::pack(u32::from(primitive.z & 0x7fff) << 3, primitive.delta_z);
                    self.set_depth_controlled_blended(
                        x,
                        y,
                        DepthFragment {
                            z: (u32::from(primitive.z & 0x7fff) << 3) as f32,
                            delta_z: primitive.delta_z,
                            encoded_depth: Some(encoded),
                            coverage,
                            rgba,
                            shade_alpha: 0,
                            noise,
                        },
                        rect.blender,
                        depth,
                        rect.other_mode,
                    );
                } else {
                    self.set_blended(
                        x,
                        y,
                        ColorFragment {
                            rgba,
                            coverage,
                            shade_alpha: 0,
                            noise,
                        },
                        rect.blender,
                        rect.other_mode,
                    );
                }
            }
        }
    }

    pub(super) fn set_blended(
        &mut self,
        x: i32,
        y: i32,
        fragment: ColorFragment,
        blender: BlenderState,
        other_mode: OtherMode,
    ) -> bool {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return false;
        }
        let pix = (y as u32 * self.width + x as u32) as usize;
        let memory = other_mode.image_read_enabled().then(|| {
            let idx = pix * 4;
            ReadFramebufferMemory {
                rgba: self.pixels[idx..idx + 4].try_into().unwrap(),
                coverage: self.coverage[pix],
            }
        });
        let result = coverage_result(fragment.coverage, self.coverage[pix], other_mode);
        self.coverage[pix] = result.destination;
        if other_mode.clear_on_coverage() && !result.wraps {
            return false;
        }
        let idx = pix * 4;
        let mut rgba = fragment.rgba;
        rgba[3] = apply_alpha_dither(
            rgba[3],
            other_mode.alpha_dither(),
            other_mode.rgb_dither(),
            x,
            y,
            fragment.noise,
        );
        let out = blend_fragment(
            rgba,
            memory,
            fragment.shade_alpha,
            blender,
            result.blend_enabled,
        );
        let out = apply_rgb_dither(out, other_mode.rgb_dither(), x, y, fragment.noise);
        self.pixels[idx..idx + 4].copy_from_slice(&out);
        true
    }

    /// Depth-tested pixel write: pass iff `z` is strictly nearer (less than)
    /// the stored depth. On pass, write the color AND the new depth. This is
    /// the standard "less-than passes, nearer wins" z-compare
    /// (`F3DEX2-CONCEPTS.md` §4.3). Returns whether the write happened (used
    /// only by tests to assert occlusion behavior).
    #[cfg(test)]
    pub(super) fn set_depth_tested(&mut self, x: i32, y: i32, z: f32, rgba: [u8; 4]) -> bool {
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
            // not a no-op. See `FN64_DUMP_PROJ` in gbi.rs.
            #[cfg(not(test))]
            zstat::note_reject();
            false
        }
    }

    pub(super) fn set_depth_controlled_blended(
        &mut self,
        x: i32,
        y: i32,
        fragment: DepthFragment,
        blender: BlenderState,
        depth: DepthControl,
        other_mode: OtherMode,
    ) -> bool {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return false;
        }
        let pix = (y as u32 * self.width + x as u32) as usize;
        let memory = other_mode.image_read_enabled().then(|| {
            let idx = pix * 4;
            ReadFramebufferMemory {
                rgba: self.pixels[idx..idx + 4].try_into().unwrap(),
                coverage: self.coverage[pix],
            }
        });
        let coverage = coverage_result(fragment.coverage, self.coverage[pix], other_mode);
        let passes_depth = if !depth.compare {
            true
        } else {
            let (memory_z, memory_encoded_delta_z) = self.encoded_depth[pix].map_or_else(
                || (self.depth[pix].clamp(0.0, 0x3ffff as f32).round() as u32, 0),
                crate::depth::unpack,
            );
            let relations = crate::depth::relations(
                fragment.z.clamp(0.0, 0x3ffff as f32).round() as u32,
                fragment.delta_z,
                memory_z,
                memory_encoded_delta_z,
            );
            match depth_coverage_decision(depth.mode, relations, coverage.wraps) {
                DepthCoverageDecision::Pass => true,
                DepthCoverageDecision::Reject => false,
                DepthCoverageDecision::UnsupportedInterpenetratingCoverageAdjustment => {
                    crate::render_unsupported_panic(
                        "render.reference.raster.interpenetration-coverage-adjustment",
                        format!(
                            "ZMODE_INTER coverage wrap requires unsupported interpenetration \
                             coverage adjustment: pixel_coverage={} memory_coverage={} \
                             depth_relations={relations:?}",
                            coverage.pixel.count(),
                            coverage.memory.count(),
                        ),
                    )
                }
            }
        };
        if passes_depth {
            self.coverage[pix] = coverage.destination;
            if other_mode.clear_on_coverage() && !coverage.wraps {
                return false;
            }
            let idx = pix * 4;
            let mut rgba = fragment.rgba;
            rgba[3] = apply_alpha_dither(
                rgba[3],
                other_mode.alpha_dither(),
                other_mode.rgb_dither(),
                x,
                y,
                fragment.noise,
            );
            let out = blend_fragment(
                rgba,
                memory,
                fragment.shade_alpha,
                blender,
                coverage.blend_enabled,
            );
            let out = apply_rgb_dither(out, other_mode.rgb_dither(), x, y, fragment.noise);
            // The fragment pipeline is combiner -> alpha compare -> depth
            // test -> blend -> write. Keep both writes after compositing so
            // a rejected fragment cannot mutate either target.
            if depth.update {
                self.depth[pix] = fragment
                    .encoded_depth
                    .map_or(fragment.z, |encoded| crate::depth::unpack(encoded).0 as f32);
                self.encoded_depth[pix] = fragment.encoded_depth;
            }
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
        self.draw_triangle_impl(tri, CullMode::None, DepthControl::DISABLED);
    }

    /// Rasterize with F3DEX2 back/front-face culling (by screen-space signed
    /// area / winding, `F3DEX2-CONCEPTS.md` §2.4/§4.2) and z-buffering
    /// (§4.3). This is the path the real OoT scene uses so far geometry is
    /// occluded correctly and inside-out back faces don't overpaint front
    /// faces.
    pub fn draw_triangle_culled(&mut self, tri: &Triangle, cull: CullMode) {
        self.draw_triangle_impl(tri, cull, DepthControl::from_other_mode(tri.other_mode));
    }

    /// Same culling as [`draw_triangle_culled`] but with NO depth test
    /// (submission/painter's order). Used only by the `FN64_NO_DEPTH` A/B
    /// instrumentation to prove that correct occlusion comes from the
    /// z-buffer, not draw order.
    pub fn draw_triangle_no_depth_culled(&mut self, tri: &Triangle, cull: CullMode) {
        self.draw_triangle_impl(tri, cull, DepthControl::DISABLED);
    }

    /// Rasterize an F3DEX2/L3DEX line with the public width, shade, texture,
    /// scissor, blender, and read-only depth contract.
    pub fn draw_line(&mut self, line: &Line) {
        self.draw_line_impl(line, DepthControl::for_line(line.other_mode));
    }

    pub fn draw_line_no_depth(&mut self, line: &Line) {
        self.draw_line_impl(line, DepthControl::DISABLED);
    }

    /// Rasterize a raw RDP triangle directly from its edge and attribute
    /// planes. SGI *RDP Command Summary* Tables 12-15 define the major edge,
    /// upper/lower minor edges, and the `d/de` plus `d/dx` coefficient groups.
    /// The public 4x4 checkerboard mask supplies eight coverage samples per
    /// pixel, retained as a typed identity mask until the fragment boundary.
    /// Full-coverage attributes retain pixel-center evaluation. Partial masks
    /// use the shared typed on-primitive sample policy; the unpublished
    /// silicon lookup and fixed-width accumulator truncation remain separate
    /// fidelity work.
    pub fn draw_raw_rdp_triangle(&mut self, triangle: &RawRdpTriangle) {
        self.draw_raw_rdp_triangle_impl(
            triangle,
            DepthControl::from_other_mode(triangle.other_mode),
        );
    }

    pub fn draw_raw_rdp_triangle_no_depth(&mut self, triangle: &RawRdpTriangle) {
        self.draw_raw_rdp_triangle_impl(triangle, DepthControl::DISABLED);
    }

    fn draw_raw_rdp_triangle_impl(&mut self, triangle: &RawRdpTriangle, depth: DepthControl) {
        require_supported_alpha_compare(triangle.other_mode, "raw RDP triangle");
        let edge = triangle.edge;
        let yh_eighth = i32::from(edge.yh) * 2;
        let yl_eighth = i32::from(edge.yl) * 2;
        let high_origin_eighth = i32::from(edge.yh & !3) * 2;
        let scissor = triangle
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let scissor_ulx_eighth = (scissor.ulx * 8.0).round() as i32;
        let scissor_uly_eighth = (scissor.uly * 8.0).round() as i32;
        let scissor_lrx_eighth = (scissor.lrx * 8.0).round() as i32;
        let scissor_lry_eighth = (scissor.lry * 8.0).round() as i32;
        let min_y = (ceil_ratio(i64::from(yh_eighth - 7), 8) as i32)
            .max(ceil_ratio(i64::from(scissor_uly_eighth - 7), 8) as i32)
            .clamp(0, self.height as i32);
        let max_y = (ceil_ratio(i64::from(yl_eighth - 1), 8) as i32)
            .min(ceil_ratio(i64::from(scissor_lry_eighth - 1), 8) as i32)
            .clamp(0, self.height as i32);
        // Whether the combiner reads TEXEL1 is fixed by the primitive's
        // combiner mode, but `uses_texel1` scans up to eight combiner sources
        // and was being rescanned once per covered pixel.
        let require_texel1 = triangle
            .combiner
            .mode
            .uses_texel1(triangle.other_mode.cycle_type());
        for y in min_y..max_y {
            if !scissor.line_enabled(y) {
                continue;
            }
            let mut min_left = i64::MAX;
            let mut max_right = i64::MIN;
            for offset_y in [1, 3, 5, 7] {
                let row_y_eighth = y * 8 + offset_y;
                if row_y_eighth < yh_eighth
                    || row_y_eighth >= yl_eighth
                    || row_y_eighth < scissor_uly_eighth
                    || row_y_eighth >= scissor_lry_eighth
                {
                    continue;
                }
                let (left_x, right_x) = raw_span_edges_at_y_eighth(edge, row_y_eighth);
                if right_x > left_x {
                    min_left = min_left.min(left_x);
                    max_right = max_right.max(right_x);
                }
            }
            if min_left == i64::MAX || max_right == i64::MIN {
                continue;
            }
            let min_x = (ceil_ratio(min_left - 7 * Q16_ONE / 8, Q16_ONE) as i32)
                .max(ceil_ratio(i64::from(scissor_ulx_eighth - 7), 8) as i32)
                .clamp(0, self.width as i32);
            let max_x = (ceil_ratio(max_right - Q16_ONE / 8, Q16_ONE) as i32)
                .min(ceil_ratio(i64::from(scissor_lrx_eighth - 1), 8) as i32)
                .clamp(0, self.width as i32);

            for x in min_x..max_x {
                let coverage_mask = raw_pixel_coverage(edge, scissor, x, y);
                let coverage = coverage_mask.coverage();
                if coverage.count() == 0 {
                    continue;
                }
                let attribute_sample = coverage_mask.attribute_sample();
                let (sample_x_eighth, sample_y_eighth) = attribute_sample.offsets_eighth();
                let sample_y_eighth = y * 8 + sample_y_eighth;
                let edge_delta_y_eighth = sample_y_eighth - high_origin_eighth;
                let major_x = i64::from(edge.xh)
                    + fixed_mul_ratio(edge.dxhdy, i64::from(edge_delta_y_eighth), 8);
                let sample_x = i64::from(x) * Q16_ONE + i64::from(sample_x_eighth) * Q16_ONE / 8;
                let edge_delta_x = sample_x - major_x;
                let plane = |base: i32, dx: i32, de: i32| {
                    raw_attribute_plane(base, dx, de, edge_delta_y_eighth, edge_delta_x)
                };
                let shade = triangle.shade.map_or(triangle.combiner.primitive, |shade| {
                    std::array::from_fn(|component| {
                        let value = plane(
                            shade.color[component],
                            shade.dcdx[component],
                            shade.dcde[component],
                        )
                        .div_euclid(Q16_ONE)
                        .clamp(0, 255);
                        value as u8
                    })
                });
                let (texel0, texel1, lod_fraction) =
                    if let Some(coefficients) = triangle.texture_coefficients {
                        let stw = std::array::from_fn::<_, 3, _>(|component| {
                            plane(
                                coefficients.stw[component],
                                coefficients.dstdx[component],
                                coefficients.dstde[component],
                            )
                        });
                        // Non-positive W tolerance (2026-07-21 WM2000 demo-scene
                        // rung): a perspective triangle crossing the near plane
                        // legitimately presents w <= 0 at edge pixels of the
                        // interpolated plane. Real RDP hardware's tcdiv derives
                        // 1/w from the operand's top bits with NO sign trap --
                        // the pixel samples garbage texels but the chip never
                        // faults. Mirror that defined-garbage tolerance: divide
                        // by the magnitude (min one ULP so it stays finite).
                        // This replaced a loud assert the moment real content
                        // (WM2000 gfx task ~#27, pixel-level near-plane
                        // crossing) hit it; the assert was right to exist until
                        // then, and the hardware-faithful behavior is "keep
                        // rasterizing", not "abort the machine".
                        //
                        // Scale (2026-07-21 WM2000 title-scene rung): hardware
                        // tcdiv is not a bare S/W ratio -- it produces an S10.5
                        // texel coordinate. The pipeline feeds tcdiv the high
                        // bits of the s15.16 attribute planes and multiplies by
                        // a 2^15-normalized reciprocal of W, so the output is
                        // (S/W) * 2^15 in S10.5 units = (S/W) * 2^10 texels
                        // (angrylion `tcdiv` persp path; RT64 divides s.w by
                        // w and scales identically). Without the 2^10 the whole
                        // title-screen quad collapsed onto texel (0,0) -- every
                        // pixel sampled the image's corner and the presented
                        // frame was a uniform field. With G_TP_NONE the divide
                        // is skipped entirely and the plane's integer part IS
                        // the S10.5 coordinate (angrylion `tcdiv_nopersp`).
                        let persp = triangle.other_mode.texture_perspective();
                        let corrected = move |values: [i64; 3]| {
                            if persp {
                                let denom = values[2].unsigned_abs().max(1) as f32;
                                (
                                    values[0] as f32 / denom * 1024.0,
                                    values[1] as f32 / denom * 1024.0,
                                )
                            } else {
                                // s15.16 plane -> S10.5 texels: 2^16 * 2^5.
                                const PLANE_TO_TEXEL: f32 = (1u32 << 21) as f32;
                                (
                                    values[0] as f32 / PLANE_TO_TEXEL,
                                    values[1] as f32 / PLANE_TO_TEXEL,
                                )
                            }
                        };
                        let (s, t) = corrected(stw);
                        let next_x = std::array::from_fn(|component| {
                            stw[component] + i64::from(coefficients.dstdx[component])
                        });
                        let next_y = std::array::from_fn(|component| {
                            stw[component] + i64::from(coefficients.dstdy[component])
                        });
                        let (sx, tx) = corrected(next_x);
                        let (sy, ty) = corrected(next_y);
                        triangle
                            .texture
                            .as_ref()
                            .expect("validated raw RDP texture disappeared before rasterization")
                            .sample_rdp_pair(
                                None,
                                TextureSampleRequest {
                                    s,
                                    t,
                                    derivatives: TextureDerivatives {
                                        dsdx: sx - s,
                                        dtdx: tx - t,
                                        dsdy: sy - s,
                                        dtdy: ty - t,
                                    },
                                    other_mode: triangle.other_mode,
                                    convert: triangle.combiner.convert,
                                    min_level: triangle.combiner.min_lod_level,
                                    require_texel1,
                                },
                            )
                    } else {
                        ([255; 4], [255; 4], 0.0)
                    };
                let (z, delta_z, encoded_depth) = triangle.z.map_or((0.0, 0, None), |z| {
                    // Nintendo 64 Programming Manual, "Z Stepper": command
                    // Z is 16.16 while the blender compares unsigned 15.3;
                    // near is zero and far is G_MAXZ. Clamp to that documented
                    // 18-bit working range after the eightfold conversion.
                    let working_z = round_ratio(
                        i128::from(plane(z.z, z.dzdx, z.dzde)) * 8,
                        i128::from(Q16_ONE),
                    )
                    .clamp(0, 0x3ffff) as u32;
                    // Nintendo 64 Programming Manual, Chapter 16, Equation 4:
                    // DeltaZpix = |dZ/dx| + |dZ/dy|. Command derivatives are
                    // 16.16 and convert to the same 15.3 working domain as Z.
                    let delta_z = round_ratio(
                        (i128::from(z.dzdx).abs() + i128::from(z.dzdy).abs()) * 8,
                        i128::from(Q16_ONE),
                    )
                    .clamp(0, i128::from(u16::MAX)) as u16;
                    let encoded = crate::depth::pack(working_z, delta_z);
                    (working_z as f32, delta_z, Some(encoded))
                });
                self.write_combined_fragment(
                    x,
                    y,
                    FragmentInputs {
                        z,
                        delta_z,
                        encoded_depth,
                        coverage,
                        shade,
                        texel0,
                        texel1,
                        lod_fraction,
                    },
                    FragmentPipeline {
                        other_mode: triangle.other_mode,
                        combiner: triangle.combiner,
                        blender: triangle.blender,
                        depth,
                    },
                );
            }
        }
    }

    fn write_combined_fragment(
        &mut self,
        x: i32,
        y: i32,
        fragment: FragmentInputs,
        pipeline: FragmentPipeline,
    ) -> bool {
        let mut fragment = fragment;
        if fragment.coverage.count() == 0 {
            return false;
        }
        if pipeline.other_mode.primitive_depth_source()
            && (pipeline.depth.compare || pipeline.depth.update)
        {
            let primitive = self
                .primitive_depth
                .expect("depth-enabled primitive selected primitive Z without G_SETPRIMDEPTH");
            let encoded =
                crate::depth::pack(u32::from(primitive.z & 0x7fff) << 3, primitive.delta_z);
            fragment.z = (u32::from(primitive.z & 0x7fff) << 3) as f32;
            fragment.delta_z = primitive.delta_z;
            fragment.encoded_depth = Some(encoded);
        }
        let noise = self.noise.next_sample();
        let rgba = evaluate_combiner(
            pipeline.combiner,
            pipeline.other_mode.cycle_type(),
            pipeline.other_mode.combine_key(),
            CombinerPixel {
                lod_fraction: fragment.lod_fraction,
                shade: fragment.shade,
                texel0: fragment.texel0,
                texel1: fragment.texel1,
                noise,
            },
        );
        let (rgba, coverage) = apply_coverage_alpha(pipeline.other_mode, rgba, fragment.coverage);
        if coverage.count() == 0 {
            return false;
        }
        if !alpha_compare_value(
            pipeline.other_mode.alpha_compare(),
            rgba[3],
            pipeline.other_mode.blend_color_alpha,
            noise,
        ) {
            return false;
        }
        if pipeline.depth.compare || pipeline.depth.update {
            self.set_depth_controlled_blended(
                x,
                y,
                DepthFragment {
                    z: fragment.z,
                    delta_z: fragment.delta_z,
                    encoded_depth: fragment.encoded_depth,
                    coverage,
                    rgba,
                    shade_alpha: fragment.shade[3],
                    noise,
                },
                pipeline.blender,
                pipeline.depth,
                pipeline.other_mode,
            )
        } else {
            self.set_blended(
                x,
                y,
                ColorFragment {
                    rgba,
                    coverage,
                    shade_alpha: fragment.shade[3],
                    noise,
                },
                pipeline.blender,
                pipeline.other_mode,
            )
        }
    }

    fn draw_triangle_impl(&mut self, tri: &Triangle, cull: CullMode, depth: DepthControl) {
        require_supported_alpha_compare(tri.other_mode, "F3DEX2 triangle");
        let [a, b, c] = tri.v;
        if tri.texture.is_some() {
            assert!(
                [a.w, b.w, c.w].iter().all(|&w| w > 1e-4),
                "textured triangle reached perspective interpolation with non-positive clip w; \
                 F3DEX2 decode must near-plane-cull it before rasterization"
            );
        }
        #[cfg(not(test))]
        let ignore_scissor = std::env::var_os("FN64_DIAG_IGNORE_SCISSOR").is_some();
        #[cfg(test)]
        let ignore_scissor = false;
        let scissor = (!ignore_scissor)
            .then_some(tri.scissor)
            .flatten()
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        // Candidate bounds are deliberately one pixel wider than the vertex
        // and scissor extrema. Coverage samples range from 1/8 through 7/8,
        // so a pixel center outside either bound can still contain selected
        // samples. The mask below performs the exact rejection.
        let clip_min_x = scissor.ulx.floor() as i32 - 1;
        let clip_max_x = scissor.lrx.ceil() as i32 + 1;
        let clip_min_y = scissor.uly.floor() as i32 - 1;
        let clip_max_y = scissor.lry.ceil() as i32 + 1;
        let min_x = (a.x.min(b.x).min(c.x).floor() as i32 - 1)
            .max(clip_min_x)
            .clamp(0, self.width as i32);
        let max_x = (a.x.max(b.x).max(c.x).ceil() as i32 + 1)
            .min(clip_max_x)
            .clamp(0, self.width as i32);
        let min_y = (a.y.min(b.y).min(c.y).floor() as i32 - 1)
            .max(clip_min_y)
            .clamp(0, self.height as i32);
        let max_y = (a.y.max(b.y).max(c.y).ceil() as i32 + 1)
            .min(clip_max_y)
            .clamp(0, self.height as i32);

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

        // Primitive-constant sampler inputs, hoisted out of the pixel loop.
        // `uses_texel1` scans up to eight combiner sources, and the diagnostic
        // env lookup was a `getenv` per covered pixel.
        let require_texel1 = tri.combiner.mode.uses_texel1(tri.other_mode.cycle_type());
        #[cfg(not(test))]
        let affine_texture = std::env::var_os("FN64_DIAG_AFFINE_TEXTURE").is_some();
        #[cfg(test)]
        let affine_texture = false;
        for y in min_y..max_y {
            for x in min_x..max_x {
                let coverage_mask = triangle_pixel_coverage([a, b, c], area, scissor, x, y);
                let coverage = coverage_mask.coverage();
                if coverage.count() == 0 {
                    continue;
                }
                let attribute_sample = coverage_mask.attribute_sample();
                let (sample_x_eighth, sample_y_eighth) = attribute_sample.offsets_eighth();
                let p = Vertex {
                    x: x as f32 + sample_x_eighth as f32 / 8.0,
                    y: y as f32 + sample_y_eighth as f32 / 8.0,
                    ..Default::default()
                };
                let w0 = edge(b, c, p) / area;
                let w1 = edge(c, a, p) / area;
                let w2 = edge(a, b, p) / area;
                // Interpolated (screen-linear) shade color.
                let shade = [
                    (w0 * a.r as f32 + w1 * b.r as f32 + w2 * c.r as f32) as u8,
                    (w0 * a.g as f32 + w1 * b.g as f32 + w2 * c.g as f32) as u8,
                    (w0 * a.b as f32 + w1 * b.b as f32 + w2 * c.b as f32) as u8,
                    (w0 * a.a as f32 + w1 * b.a as f32 + w2 * c.a as f32) as u8,
                ];
                // Interpolate S/w, T/w, and 1/w, then divide before sampling.
                // Derivatives retain the selected within-pixel offset so the
                // correction translates the plane without changing its
                // adjacent-pixel gradient.
                let (texel0, texel1, lod_fraction) = if let Some(tex) = &tri.texture {
                    let coordinates_at = |px: f32, py: f32| {
                        let point = Vertex {
                            x: px,
                            y: py,
                            ..Default::default()
                        };
                        let q0 = edge(b, c, point) / area;
                        let q1 = edge(c, a, point) / area;
                        let q2 = edge(a, b, point) / area;
                        if affine_texture {
                            (
                                q0 * a.s + q1 * b.s + q2 * c.s,
                                q0 * a.t + q1 * b.t + q2 * c.t,
                            )
                        } else {
                            let rw0 = q0 / a.w;
                            let rw1 = q1 / b.w;
                            let rw2 = q2 / c.w;
                            let reciprocal_w = rw0 + rw1 + rw2;
                            assert!(
                                reciprocal_w > 0.0,
                                "F3DEX2 texture interpolation produced non-positive reciprocal W"
                            );
                            (
                                (rw0 * a.s + rw1 * b.s + rw2 * c.s) / reciprocal_w,
                                (rw0 * a.t + rw1 * b.t + rw2 * c.t) / reciprocal_w,
                            )
                        }
                    };
                    let (s, t) = coordinates_at(p.x, p.y);
                    let (sx, tx) = coordinates_at(p.x + 1.0, p.y);
                    let (sy, ty) = coordinates_at(p.x, p.y + 1.0);
                    tex.sample_rdp_pair(
                        None,
                        TextureSampleRequest {
                            s,
                            t,
                            derivatives: TextureDerivatives {
                                dsdx: sx - s,
                                dtdx: tx - t,
                                dsdy: sy - s,
                                dtdy: ty - t,
                            },
                            other_mode: tri.other_mode,
                            convert: tri.combiner.convert,
                            min_level: tri.combiner.min_lod_level,
                            require_texel1,
                        },
                    )
                } else {
                    ([255; 4], [255; 4], 0.0)
                };
                // Screen-linear depth interpolation remains the F3DEX2
                // approximation. Raw RDP work uses its coefficient plane.
                // F3DEX vertices carry viewport-mapped screen Z. Convert
                // to the RDP blender's unsigned 15.3 working domain so
                // HLE and raw-command samples compare in identical units.
                let z = ((w0 * a.z + w1 * b.z + w2 * c.z) * 8.0).clamp(0.0, 0x3ffff as f32);
                let denominator = edge(a, b, c);
                let dzdx = ((b.z - a.z) * (c.y - a.y) - (c.z - a.z) * (b.y - a.y)) / denominator;
                let dzdy = ((b.x - a.x) * (c.z - a.z) - (c.x - a.x) * (b.z - a.z)) / denominator;
                let delta_z = ((dzdx.abs() + dzdy.abs()) * 8.0)
                    .round()
                    .clamp(0.0, u16::MAX as f32) as u16;
                self.write_combined_fragment(
                    x,
                    y,
                    FragmentInputs {
                        z,
                        delta_z,
                        encoded_depth: Some(crate::depth::pack(z.round() as u32, delta_z)),
                        coverage,
                        shade,
                        texel0,
                        texel1,
                        lod_fraction,
                    },
                    FragmentPipeline {
                        other_mode: tri.other_mode,
                        combiner: tri.combiner,
                        blender: tri.blender,
                        depth,
                    },
                );
            }
        }
    }

    fn draw_line_impl(&mut self, line: &Line, depth: DepthControl) {
        require_supported_alpha_compare(line.other_mode, "F3DEX2/L3DEX line");
        let [a, b] = line.v;
        if line.texture.is_some() {
            assert!(
                a.w > 1e-4 && b.w > 1e-4,
                "textured G_LINE3D reached perspective interpolation with non-positive clip w"
            );
        }
        let scissor = line
            .scissor
            .unwrap_or_else(|| ScissorRect::framebuffer(self.width, self.height));
        let radius = line.width * 0.5;
        let min_x = ((a.x.min(b.x) - radius).floor() as i32 - 1)
            .max(scissor.ulx.floor() as i32 - 1)
            .clamp(0, self.width as i32);
        let max_x = ((a.x.max(b.x) + radius).ceil() as i32 + 1)
            .min(scissor.lrx.ceil() as i32 + 1)
            .clamp(0, self.width as i32);
        let min_y = ((a.y.min(b.y) - radius).floor() as i32 - 1)
            .max(scissor.uly.floor() as i32 - 1)
            .clamp(0, self.height as i32);
        let max_y = ((a.y.max(b.y) + radius).ceil() as i32 + 1)
            .min(scissor.lry.ceil() as i32 + 1)
            .clamp(0, self.height as i32);
        let segment_length = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let delta_z = if segment_length > f32::EPSILON {
            (((b.z - a.z).abs() / segment_length) * 8.0)
                .round()
                .clamp(0.0, u16::MAX as f32) as u16
        } else {
            0
        };
        let lerp_channel = |start: u8, end: u8, parameter: f32| {
            (f32::from(start) + (f32::from(end) - f32::from(start)) * parameter).clamp(0.0, 255.0)
                as u8
        };
        let parameter_at = |x: f32, y: f32| {
            line_parameter_and_distance_squared(a, b, x, y)
                .0
                .clamp(0.0, 1.0)
        };
        let texture_coordinates_at = |x: f32, y: f32| {
            let parameter = parameter_at(x, y);
            let start_weight = 1.0 - parameter;
            let end_weight = parameter;
            let reciprocal_w = start_weight / a.w + end_weight / b.w;
            assert!(
                reciprocal_w > 0.0,
                "G_LINE3D texture interpolation produced non-positive reciprocal W"
            );
            (
                (start_weight * a.s / a.w + end_weight * b.s / b.w) / reciprocal_w,
                (start_weight * a.t / a.w + end_weight * b.t / b.w) / reciprocal_w,
            )
        };

        // Fixed by the primitive's combiner mode; `uses_texel1` scans up to
        // eight combiner sources, so it does not belong in the pixel loop.
        let require_texel1 = line
            .combiner
            .mode
            .uses_texel1(line.other_mode.cycle_type());
        for y in min_y..max_y {
            for x in min_x..max_x {
                let coverage_mask = line_pixel_coverage(line, scissor, x, y);
                let coverage = coverage_mask.coverage();
                if coverage.count() == 0 {
                    continue;
                }
                let attribute_sample = coverage_mask.attribute_sample();
                let (sample_x_eighth, sample_y_eighth) = attribute_sample.offsets_eighth();
                let sample_x = x as f32 + sample_x_eighth as f32 / 8.0;
                let sample_y = y as f32 + sample_y_eighth as f32 / 8.0;
                let parameter = parameter_at(sample_x, sample_y);
                let shade = if line.smooth_shading {
                    [
                        lerp_channel(a.r, b.r, parameter),
                        lerp_channel(a.g, b.g, parameter),
                        lerp_channel(a.b, b.b, parameter),
                        lerp_channel(a.a, b.a, parameter),
                    ]
                } else {
                    [a.r, a.g, a.b, a.a]
                };
                let (texel0, texel1, lod_fraction) = if let Some(texture) = &line.texture {
                    let (s, t) = texture_coordinates_at(sample_x, sample_y);
                    let (sx, tx) = texture_coordinates_at(sample_x + 1.0, sample_y);
                    let (sy, ty) = texture_coordinates_at(sample_x, sample_y + 1.0);
                    texture.sample_rdp_pair(
                        None,
                        TextureSampleRequest {
                            s,
                            t,
                            derivatives: TextureDerivatives {
                                dsdx: sx - s,
                                dtdx: tx - t,
                                dsdy: sy - s,
                                dtdy: ty - t,
                            },
                            other_mode: line.other_mode,
                            convert: line.combiner.convert,
                            min_level: line.combiner.min_lod_level,
                            require_texel1,
                        },
                    )
                } else {
                    ([255; 4], [255; 4], 0.0)
                };
                let z = ((a.z + (b.z - a.z) * parameter) * 8.0).clamp(0.0, 0x3ffff as f32);
                self.write_combined_fragment(
                    x,
                    y,
                    FragmentInputs {
                        z,
                        delta_z,
                        encoded_depth: Some(crate::depth::pack(z.round() as u32, delta_z)),
                        coverage,
                        shade,
                        texel0,
                        texel1,
                        lod_fraction,
                    },
                    FragmentPipeline {
                        other_mode: line.other_mode,
                        combiner: line.combiner,
                        blender: line.blender,
                        depth,
                    },
                );
            }
        }
    }
}
