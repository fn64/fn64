//! Content-free demo mode: the real window, overlay, and input stack driven by
//! a synthetic RDRAM framebuffer instead of a booted game.
//!
//! Why this exists: the shell links its game at BUILD time from
//! `RECOMPILED_DIR`/`ROM` (see `build.rs`), so a checkout without a ROM could
//! not open the window at all — it printed intake instructions and exited. That
//! made every UI change unverifiable without game content, which
//! `AGENTS.md` keeps out of git by design.
//!
//! This module supplies the one thing the game supplied: bytes in RDRAM. It
//! writes N64-native RGBA5551 into a real `RdramViewMut` and hands it to the
//! same `framebuffer::rgba5551_to_rgba8888` the game path uses, so the
//! conversion, window, and overlay code under test are the production ones. It
//! is NOT an emulator, a stub renderer, or a second UI: everything below the
//! byte source is unchanged.
//!
//! What it therefore CANNOT tell you: whether a real game renders correctly.
//! It exercises the presentation path, not the runtime. A green demo is
//! evidence the UI stack works, never evidence a ROM boots.
//!
//! ponytail: synthetic pattern generator, not a recorded frame dump. A dump
//! would be more faithful but needs game content in git; swap the pattern for
//! a loaded capture if a real frame is ever needed and licensable.

use crate::framebuffer::{self, FB_HEIGHT, FB_WIDTH};
use crate::gamepad::Gamepads;
use crate::input_map::InputConfig;
use crate::overlay::Overlay;
use std::sync::Arc;

use pixels::{Pixels, SurfaceTexture};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// One RGBA5551 pixel, N64 layout: `RRRRRGGG GGBBBBB1`.
fn rgba5551(r: u8, g: u8, b: u8) -> u16 {
    let r = (r >> 3) as u16;
    let g = (g >> 3) as u16;
    let b = (b >> 3) as u16;
    (r << 11) | (g << 6) | (b << 1) | 1
}

/// Paint one synthetic field into a guest-endian RDRAM framebuffer.
///
/// The pattern is chosen so a human can see at a glance whether the
/// presentation path is intact: colour bars prove channel order and the 5-bit
/// quantisation, the moving bar proves frames actually advance, and the corner
/// markers prove no row/column is being dropped by the stride math.
pub fn paint_field(rdram: &mut [u8], frame: u64) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    // The framebuffer lives at RDRAM offset 0 for the demo; a real game's VI
    // origin is wherever it programmed, which is exactly what the game path
    // passes through instead.
    for y in 0..FB_HEIGHT {
        for x in 0..FB_WIDTH {
            // Eight vertical colour bars across the width.
            let bar = (x * 8) / FB_WIDTH;
            let (r, g, b) = match bar {
                0 => (255, 255, 255),
                1 => (255, 255, 0),
                2 => (0, 255, 255),
                3 => (0, 255, 0),
                4 => (255, 0, 255),
                5 => (255, 0, 0),
                6 => (0, 0, 255),
                _ => (0, 0, 0),
            };
            // A vertical bar sweeping left-to-right: motion, so a frozen frame
            // is visibly distinct from a live one.
            let sweep = ((frame as usize) * 2) % FB_WIDTH;
            let on_sweep = x.abs_diff(sweep) < 3;
            // Gradient down the field so vertical stride errors show up.
            let shade = ((y * 255) / FB_HEIGHT) as u8;
            let px = if on_sweep {
                rgba5551(255, 255, 255)
            } else {
                rgba5551(
                    r.min(shade.max(32)),
                    g.min(shade.max(32)),
                    b.min(shade.max(32)),
                )
            };
            // Corner markers: 4x4 red squares, one per corner.
            let corner =
                !(4..FB_WIDTH - 4).contains(&x) && !(4..FB_HEIGHT - 4).contains(&y);
            let px = if corner { rgba5551(255, 0, 0) } else { px };

            let offset = (y * FB_WIDTH + x) * 2;
            view.write_u16(fn64_runtime::RdramAddr::from_offset(offset as u32), px);
        }
    }
}

struct Demo {
    window: Option<Arc<Window>>,
    pixels: Option<Pixels<'static>>,
    overlay: Overlay,
    /// The settings the overlay edits. Real `InputConfig`, so the demo
    /// exercises the production settings UI rather than a mock panel.
    config: InputConfig,
    /// The overlay's rebind UI reads live pad state; empty without a
    /// controller, which is a valid state rather than a special demo case.
    gamepads: Gamepads,
    rdram: Vec<u8>,
    rgba: Vec<u8>,
    frame: u64,
    /// Exit after this many frames when set, so the demo is runnable headlessly
    /// in CI as well as interactively.
    max_frames: Option<u64>,
}

impl ApplicationHandler for Demo {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Same 2x as the game path, deliberately: the demo mirrors production
        // window geometry so a layout problem seen here is a real one there.
        let size = LogicalSize::new((FB_WIDTH * 2) as f64, (FB_HEIGHT * 2) as f64);
        let attrs = Window::default_attributes()
            .with_title("fn64 -- UI demo (no game linked)")
            .with_inner_size(size)
            .with_min_inner_size(LogicalSize::new(FB_WIDTH as f64, FB_HEIGHT as f64));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("[fn64-demo] failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let win_size = window.inner_size();
        let surface = SurfaceTexture::new(win_size.width, win_size.height, Arc::clone(&window));
        match Pixels::new(FB_WIDTH as u32, FB_HEIGHT as u32, surface) {
            Ok(px) => {
                self.pixels = Some(px);
                window.request_redraw();
                self.window = Some(window);
                println!(
                    "[fn64-demo] window opened ({}x{})",
                    win_size.width, win_size.height
                );
            }
            Err(e) => {
                eprintln!("[fn64-demo] failed to create pixels surface: {e}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let Some(window) = self.window.as_ref() {
            self.overlay
                .on_window_event(&event, window.scale_factor() as f32);
        }
        match event {
            WindowEvent::CloseRequested => {
                println!("[fn64-demo] window close requested -- exiting");
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                if let Some(px) = self.pixels.as_mut() {
                    let _ = px.resize_surface(new_size.width, new_size.height);
                }
            }
            // F1 opens/closes the settings overlay, Escape closes it --
            // the same bindings as the game path (`main.rs`), so the demo
            // teaches the real shortcuts rather than demo-only ones.
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        match code {
                            KeyCode::F1 => {
                                self.overlay.toggle();
                                println!(
                                    "[fn64-demo] settings overlay {}",
                                    if self.overlay.open { "opened" } else { "closed" }
                                );
                            }
                            KeyCode::Escape if self.overlay.open => {
                                self.overlay.toggle();
                                println!("[fn64-demo] settings overlay closed");
                            }
                            _ => {}
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                paint_field(&mut self.rdram, self.frame);
                framebuffer::rgba5551_to_rgba8888(
                    fn64_runtime::RdramView::from_storage(&self.rdram),
                    fn64_runtime::RdramAddr::from_offset(0),
                    FB_WIDTH,
                    FB_WIDTH,
                    &mut self.rgba,
                );
                if let Some(px) = self.pixels.as_mut() {
                    px.frame_mut().copy_from_slice(&self.rgba);
                }
                // Same branch the game path takes: the overlay composites over
                // the presented field, so an open overlay renders through
                // egui instead of the plain pixels present.
                let render_result = if self.overlay.open {
                    let window = self.window.as_ref().expect("window exists with pixels");
                    let size = window.inner_size();
                    self.overlay.render_over(
                        self.pixels.as_ref().expect("checked above"),
                        (size.width.max(1), size.height.max(1)),
                        window.scale_factor() as f32,
                        &mut self.config,
                        &self.gamepads,
                    )
                } else {
                    self.pixels.as_ref().expect("checked above").render()
                };
                if let Err(e) = render_result {
                    eprintln!("[fn64-demo] render error: {e}");
                    event_loop.exit();
                    return;
                }
                self.frame += 1;
                // Periodic proof-of-life: a window that opens but never
                // presents is the exact failure this demo exists to catch.
                if self.frame == 1 || self.frame.is_multiple_of(60) {
                    println!("[fn64-demo] presented frame {}", self.frame);
                }
                if let Some(max) = self.max_frames {
                    if self.frame >= max {
                        println!("[fn64-demo] reached {max} frames -- exiting");
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }
    }

    /// Drive redraws from here, matching the game path (`main.rs`'s
    /// `about_to_wait`). Re-requesting from inside `RedrawRequested` alone
    /// never starts: nothing asks for the FIRST frame, so the window opens and
    /// stays blank.
    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // gilrs only advances its state when its queue is drained, so the
        // overlay's pad-rebind capture needs this even with no pad attached.
        self.gamepads.poll();
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

/// Write one converted field to a PNG without opening a window, so the demo is
/// inspectable on a headless machine and in CI. Reuses the same
/// filter-0 + stored-DEFLATE encoding as `examples/oot-boot`'s
/// `dump_rgba5551_as_png`; no image crate is pulled in for a diagnostic dump.
pub fn capture_png(path: &str, frame: u64) -> std::io::Result<()> {
    use std::io::Write;

    let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
    paint_field(&mut rdram, frame);
    let mut rgba = vec![0u8; FB_WIDTH * FB_HEIGHT * 4];
    framebuffer::rgba5551_to_rgba8888(
        fn64_runtime::RdramView::from_storage(&rdram),
        fn64_runtime::RdramAddr::from_offset(0),
        FB_WIDTH,
        FB_WIDTH,
        &mut rgba,
    );

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }
    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in data {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn chunk(file: &mut std::fs::File, kind: &[u8; 4], data: &[u8]) -> std::io::Result<()> {
        file.write_all(&(data.len() as u32).to_be_bytes())?;
        file.write_all(kind)?;
        file.write_all(data)?;
        let mut crc_input = Vec::with_capacity(4 + data.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        file.write_all(&crc32(&crc_input).to_be_bytes())
    }

    let mut file = std::fs::File::create(path)?;
    file.write_all(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])?;
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(FB_WIDTH as u32).to_be_bytes());
    ihdr.extend_from_slice(&(FB_HEIGHT as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
    chunk(&mut file, b"IHDR", &ihdr)?;

    let stride = FB_WIDTH * 4;
    let mut raw = Vec::with_capacity((stride + 1) * FB_HEIGHT);
    for row in 0..FB_HEIGHT {
        raw.push(0u8); // filter type 0
        raw.extend_from_slice(&rgba[row * stride..(row + 1) * stride]);
    }
    // zlib-wrapped stored DEFLATE: valid, just not size-optimal.
    let mut idat = vec![0x78, 0x01];
    let mut offset = 0;
    loop {
        let block = (raw.len() - offset).min(65535);
        let final_block = offset + block >= raw.len();
        idat.push(u8::from(final_block));
        idat.extend_from_slice(&(block as u16).to_le_bytes());
        idat.extend_from_slice(&(!(block as u16)).to_le_bytes());
        idat.extend_from_slice(&raw[offset..offset + block]);
        offset += block;
        if final_block {
            break;
        }
    }
    idat.extend_from_slice(&adler32(&raw).to_be_bytes());
    chunk(&mut file, b"IDAT", &idat)?;
    chunk(&mut file, b"IEND", &[])?;
    Ok(())
}

/// Run the demo window. `FN64_DEMO_FRAMES=N` exits after N frames;
/// `FN64_DEMO_PNG=path` writes one field and exits without opening a window.
pub fn run() {
    let max_frames = std::env::var("FN64_DEMO_FRAMES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());

    println!("[fn64-demo] content-free UI demo: synthetic framebuffer, no ROM, no recompilation.");

    // Headless capture: no display needed, so this works over ssh and in CI.
    if let Ok(path) = std::env::var("FN64_DEMO_PNG") {
        let frame = max_frames.unwrap_or(0);
        match capture_png(&path, frame) {
            Ok(()) => println!("[fn64-demo] wrote {path} (frame {frame}, {FB_WIDTH}x{FB_HEIGHT})"),
            Err(e) => {
                eprintln!("[fn64-demo] failed to write {path}: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    println!("[fn64-demo] F1 = settings overlay, Escape = close it, window close = quit.");
    if let Some(n) = max_frames {
        println!("[fn64-demo] will exit after {n} frames (FN64_DEMO_FRAMES)");
    }

    let event_loop = match EventLoop::new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("[fn64-demo] no display available: {e}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut demo = Demo {
        window: None,
        pixels: None,
        overlay: Overlay::new(),
        // Default rather than `InputConfig::load()`: the demo must not read or
        // rewrite the user's real keybind file as a side effect of being run.
        config: InputConfig::default(),
        gamepads: Gamepads::new(),
        rdram: vec![0; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE],
        rgba: vec![0; FB_WIDTH * FB_HEIGHT * 4],
        frame: 0,
        max_frames,
    };
    if let Err(e) = event_loop.run_app(&mut demo) {
        eprintln!("[fn64-demo] event loop error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one check that fails if the demo stops exercising the real
    /// presentation path: synthetic RDRAM must survive the production
    /// RGBA5551->RGBA8888 conversion as a non-uniform, changing image.
    #[test]
    fn synthetic_field_converts_to_a_live_changing_image() {
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let mut rgba = vec![0u8; FB_WIDTH * FB_HEIGHT * 4];

        paint_field(&mut rdram, 0);
        framebuffer::rgba5551_to_rgba8888(
            fn64_runtime::RdramView::from_storage(&rdram),
            fn64_runtime::RdramAddr::from_offset(0),
            FB_WIDTH,
            FB_WIDTH,
            &mut rgba,
        );
        assert!(
            !framebuffer::is_uniform(&rgba),
            "a colour-bar field must not convert to a flat image"
        );
        let first = framebuffer::rgba_hash(&rgba);

        // A later frame must differ, or the window would show a frozen image
        // and still look 'working'.
        paint_field(&mut rdram, 30);
        framebuffer::rgba5551_to_rgba8888(
            fn64_runtime::RdramView::from_storage(&rdram),
            fn64_runtime::RdramAddr::from_offset(0),
            FB_WIDTH,
            FB_WIDTH,
            &mut rgba,
        );
        assert_ne!(
            first,
            framebuffer::rgba_hash(&rgba),
            "the sweep must advance between frames"
        );
    }
}
