//! The in-game input settings overlay (issue #5): an egui panel over the
//! paused-input framebuffer, toggled with F1. Press-to-bind remapping for
//! keyboard and gamepad, a deadzone slider with a live analog-stick scope,
//! auto-saved to the `InputConfig` TOML.
//!
//! ## Why no egui-winit / egui-winit-free event feed
//!
//! pixels 0.15 pins wgpu 0.19, which pins egui-wgpu (and thus egui) to the
//! 0.27 line -- whose egui-winit wants winit 0.29, not this shell's 0.30.
//! The overlay only needs mouse events (no text fields), so the translation
//! layer egui-winit would provide is ~40 lines here: cursor position,
//! clicks, scroll. Keyboard goes straight from the winit handler in
//! `main.rs` to the capture flow, never through egui.

use crate::gamepad::{apply_deadzone_f, Gamepads};
use crate::input_map::{BindTarget, InputConfig, N64Button, StickDir};
use egui::{Align2, Color32, Pos2, Rect, RichText, Rounding, Stroke, Vec2};
// pixels' re-export, so the render pass talks to the same wgpu 0.19 instance
// pixels' surface lives in (egui-wgpu resolves to that same version).
use pixels::wgpu;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::KeyCode;

// The N64's own hardware palette carries the accents; everything else stays
// quiet (frontend-design brief: spend the boldness in one place).
const SMOKE: Color32 = Color32::from_rgb(0x14, 0x14, 0x17);
const INK: Color32 = Color32::from_rgb(0xE8, 0xE6, 0xE1);
const A_BLUE: Color32 = Color32::from_rgb(0x2E, 0x64, 0xFE);
const B_GREEN: Color32 = Color32::from_rgb(0x00, 0xA6, 0x51);
const START_RED: Color32 = Color32::from_rgb(0xE6, 0x00, 0x12);
const C_YELLOW: Color32 = Color32::from_rgb(0xF5, 0xB3, 0x01);
const MUTED: Color32 = Color32::from_rgb(0x8A, 0x88, 0x83);

/// What the next input event will be bound to, once armed by clicking a
/// binding slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    Key(BindTarget),
    Pad(N64Button),
}

pub struct Overlay {
    pub open: bool,
    pub capture: Option<Capture>,
    ctx: egui::Context,
    renderer: Option<egui_wgpu::Renderer>,
    /// Pending egui events translated from winit, drained each frame.
    events: Vec<egui::Event>,
    /// Last cursor position in egui points.
    cursor: Pos2,
    started: std::time::Instant,
    /// Set by any UI change; triggers a config save after the frame.
    dirty: bool,
}

impl Overlay {
    pub fn new() -> Self {
        let ctx = egui::Context::default();
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = SMOKE;
        visuals.window_fill = SMOKE;
        visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(0x2A, 0x2A, 0x2F));
        visuals.window_rounding = Rounding::same(6.0);
        visuals.override_text_color = Some(INK);
        ctx.set_visuals(visuals);
        Overlay {
            open: false,
            capture: None,
            ctx,
            renderer: None,
            events: Vec::new(),
            cursor: Pos2::ZERO,
            started: std::time::Instant::now(),
            dirty: false,
        }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.capture = None;
    }

    /// Translate the few winit window events egui needs. Call for every
    /// window event while the overlay is open; cheap no-ops otherwise.
    pub fn on_window_event(&mut self, event: &WindowEvent, scale_factor: f32) {
        if !self.open {
            return;
        }
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Pos2::new(
                    position.x as f32 / scale_factor,
                    position.y as f32 / scale_factor,
                );
                self.events.push(egui::Event::PointerMoved(self.cursor));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => egui::PointerButton::Primary,
                    MouseButton::Right => egui::PointerButton::Secondary,
                    MouseButton::Middle => egui::PointerButton::Middle,
                    _ => return,
                };
                self.events.push(egui::Event::PointerButton {
                    pos: self.cursor,
                    button,
                    pressed: *state == ElementState::Pressed,
                    modifiers: egui::Modifiers::default(),
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let points = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Vec2::new(x * 20.0, y * 20.0),
                    MouseScrollDelta::PixelDelta(p) => {
                        Vec2::new(p.x as f32, p.y as f32) / scale_factor
                    }
                };
                self.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: points,
                    modifiers: egui::Modifiers::default(),
                });
            }
            _ => {}
        }
    }

    /// A keyboard key arrived while a keyboard capture was armed. Returns
    /// true if it was consumed as a binding.
    pub fn apply_key_capture(&mut self, config: &mut InputConfig, key: KeyCode) -> bool {
        let Some(Capture::Key(target)) = self.capture else {
            return false;
        };
        config.bind_key(target, key);
        config.save();
        self.capture = None;
        println!("[fn64-shell] input: bound {key:?} via settings overlay");
        true
    }

    /// A gamepad button arrived while a gamepad capture was armed.
    pub fn apply_pad_capture(&mut self, config: &mut InputConfig, button: gilrs::Button) {
        let Some(Capture::Pad(target)) = self.capture else {
            return;
        };
        config.bind_pad(target, button);
        config.save();
        self.capture = None;
        println!("[fn64-shell] input: bound gamepad {button:?} via settings overlay");
    }

    /// Run the UI and paint it over the just-blitted framebuffer. Call from
    /// `Shell::present` INSTEAD of `pixels.render()` when open.
    pub fn render_over(
        &mut self,
        pixels: &pixels::Pixels<'static>,
        window_size: (u32, u32),
        scale_factor: f32,
        config: &mut InputConfig,
        gamepads: &Gamepads,
    ) -> Result<(), pixels::Error> {
        let (width, height) = window_size;
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(width as f32, height as f32) / scale_factor,
            )),
            time: Some(self.started.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(scale_factor);

        let mut capture = self.capture;
        let mut dirty = false;
        let full_output = self.ctx.clone().run(raw, |ctx| {
            draw_ui(ctx, config, gamepads, &mut capture, &mut dirty);
        });
        self.capture = capture;
        if dirty {
            config.save();
        }

        let pixels_per_point = full_output.pixels_per_point;
        let primitives = self
            .ctx
            .tessellate(full_output.shapes, pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point,
        };
        let renderer = &mut self.renderer;
        let textures_delta = full_output.textures_delta;

        pixels.render_with(|encoder, target, context| {
            // The game framebuffer first (what pixels.render() would do)...
            context.scaling_renderer.render(encoder, target);

            // ...then the egui pass on top, LoadOp::Load to keep it.
            let renderer = renderer.get_or_insert_with(|| {
                egui_wgpu::Renderer::new(
                    &context.device,
                    pixels.surface_texture_format(),
                    None,
                    1,
                )
            });
            for (id, delta) in &textures_delta.set {
                renderer.update_texture(&context.device, &context.queue, *id, delta);
            }
            let user_buffers = renderer.update_buffers(
                &context.device,
                &context.queue,
                encoder,
                &primitives,
                &screen,
            );
            debug_assert!(user_buffers.is_empty(), "no egui paint callbacks in use");
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("fn64 overlay"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                renderer.render(&mut pass, &primitives, &screen);
            }
            for id in &textures_delta.free {
                renderer.free_texture(id);
            }
            Ok(())
        })
    }
}

fn accent(button: N64Button) -> Color32 {
    match button {
        N64Button::A => A_BLUE,
        N64Button::B => B_GREEN,
        N64Button::Start => START_RED,
        N64Button::CUp | N64Button::CDown | N64Button::CLeft | N64Button::CRight => C_YELLOW,
        _ => MUTED,
    }
}

/// Compact display of a keyboard binding ("KeyX" -> "X", "Digit4" -> "4").
fn key_label(key: KeyCode) -> String {
    let name = format!("{key:?}");
    name.strip_prefix("Key")
        .or_else(|| name.strip_prefix("Digit"))
        .unwrap_or(&name)
        .to_string()
}

fn draw_ui(
    ctx: &egui::Context,
    config: &mut InputConfig,
    gamepads: &Gamepads,
    capture: &mut Option<Capture>,
    dirty: &mut bool,
) {
    // Dim the game underneath so the panel reads as a modal layer.
    egui::Area::new(egui::Id::new("fn64-dim"))
        .order(egui::Order::Background)
        .fixed_pos(Pos2::ZERO)
        .show(ctx, |ui| {
            ui.painter().rect_filled(
                ctx.screen_rect(),
                Rounding::ZERO,
                Color32::from_black_alpha(140),
            );
        });

    egui::Window::new(RichText::new("CONTROLLER").strong().color(INK))
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            match gamepads.active_name() {
                Some(name) => ui.label(RichText::new(name).color(MUTED).small()),
                None => ui.label(
                    RichText::new("no gamepad detected — keyboard only")
                        .color(MUTED)
                        .small(),
                ),
            };
            ui.add_space(6.0);

            ui.horizontal_top(|ui| {
                // Left: bindings, grouped by the controller's physical
                // regions (structure = the real hardware, not a flat list).
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            bindings_grid(ui, config, capture);
                        });
                });
                ui.separator();
                // Right: the analog column — deadzone + live scope.
                ui.vertical(|ui| {
                    stick_scope(ui, config, gamepads, dirty);
                });
            });

            ui.add_space(6.0);
            let hint = match capture {
                Some(Capture::Key(_)) => "press a key to bind — Esc cancels",
                Some(Capture::Pad(_)) => "press a gamepad button to bind — Esc cancels",
                None => "F1 close · F11 fullscreen · changes saved automatically",
            };
            ui.label(RichText::new(hint).color(MUTED).small());
        });
}

fn bindings_grid(ui: &mut egui::Ui, config: &mut InputConfig, capture: &mut Option<Capture>) {
    let sections: [(&str, &[N64Button]); 4] = [
        ("FACE", &[N64Button::A, N64Button::B, N64Button::Start]),
        (
            "C-CLUSTER",
            &[
                N64Button::CUp,
                N64Button::CDown,
                N64Button::CLeft,
                N64Button::CRight,
            ],
        ),
        ("TRIGGERS", &[N64Button::Z, N64Button::L, N64Button::R]),
        (
            "D-PAD",
            &[
                N64Button::DUp,
                N64Button::DDown,
                N64Button::DLeft,
                N64Button::DRight,
            ],
        ),
    ];

    // Stick section first: it's the control players touch most.
    ui.label(RichText::new("STICK").color(MUTED).small());
    egui::Grid::new("stick-grid").num_columns(3).show(ui, |ui| {
        for dir in StickDir::ALL {
            ui.label(RichText::new(dir.label()).color(MUTED));
            let armed = *capture == Some(Capture::Key(BindTarget::Stick(dir)));
            let text = if armed {
                "…".to_string()
            } else {
                config
                    .keyboard_stick
                    .get(&dir)
                    .map(|&k| key_label(k))
                    .unwrap_or_else(|| "—".to_string())
            };
            if ui.button(RichText::new(text).monospace()).clicked() {
                *capture = Some(Capture::Key(BindTarget::Stick(dir)));
            }
            // Gamepad column: the stick is always the physical left stick.
            ui.label(RichText::new("left stick").color(MUTED).small());
            ui.end_row();
        }
    });

    for (title, buttons) in sections {
        ui.add_space(4.0);
        ui.label(RichText::new(title).color(MUTED).small());
        egui::Grid::new(title).num_columns(3).show(ui, |ui| {
            for &button in buttons {
                ui.label(RichText::new(button.label()).color(accent(button)).strong());

                let key_armed = *capture == Some(Capture::Key(BindTarget::Button(button)));
                let key_text = if key_armed {
                    "…".to_string()
                } else {
                    config
                        .keyboard
                        .get(&button)
                        .map(|&k| key_label(k))
                        .unwrap_or_else(|| "—".to_string())
                };
                if ui.button(RichText::new(key_text).monospace()).clicked() {
                    *capture = Some(Capture::Key(BindTarget::Button(button)));
                }

                let pad_armed = *capture == Some(Capture::Pad(button));
                let pad_text = if pad_armed {
                    "…".to_string()
                } else {
                    config
                        .gamepad
                        .get(&button)
                        .map(|&b| format!("{b:?}"))
                        .unwrap_or_else(|| {
                            if matches!(
                                button,
                                N64Button::CUp
                                    | N64Button::CDown
                                    | N64Button::CLeft
                                    | N64Button::CRight
                            ) {
                                "right stick".to_string()
                            } else {
                                "—".to_string()
                            }
                        })
                };
                if ui.button(RichText::new(pad_text).monospace()).clicked() {
                    *capture = Some(Capture::Pad(button));
                }
                ui.end_row();
            }
        });
    }
}

/// The signature element: a live scope showing the raw stick position, the
/// deadzone ring, and the post-deadzone position the game actually sees.
fn stick_scope(ui: &mut egui::Ui, config: &mut InputConfig, gamepads: &Gamepads, dirty: &mut bool) {
    ui.label(RichText::new("ANALOG").color(MUTED).small());
    let (response, painter) = ui.allocate_painter(Vec2::splat(132.0), egui::Sense::hover());
    let rect = response.rect;
    let center = rect.center();
    let radius = rect.width() / 2.0 - 6.0;

    painter.circle_stroke(center, radius, Stroke::new(1.0, MUTED));
    painter.circle_filled(
        center,
        radius * config.deadzone,
        Color32::from_rgb(0x22, 0x22, 0x27),
    );
    painter.circle_stroke(
        center,
        radius * config.deadzone,
        Stroke::new(1.0, Color32::from_rgb(0x3A, 0x3A, 0x41)),
    );

    // egui y grows downward; stick y grows upward.
    let to_screen =
        |x: f32, y: f32| center + Vec2::new(x, -y) * radius;
    let (rx, ry) = gamepads.raw_stick();
    let (fx, fy) = apply_deadzone_f(rx, ry, config.deadzone);
    painter.circle_filled(to_screen(rx, ry), 3.0, MUTED);
    painter.circle_filled(to_screen(fx, fy), 4.5, C_YELLOW);
    painter.line_segment(
        [to_screen(rx, ry), to_screen(fx, fy)],
        Stroke::new(1.0, Color32::from_rgb(0x3A, 0x3A, 0x41)),
    );

    ui.add_space(4.0);
    ui.label(RichText::new("deadzone").color(MUTED).small());
    let slider = ui.add(
        egui::Slider::new(&mut config.deadzone, 0.0..=0.5)
            .fixed_decimals(2)
            .trailing_fill(true),
    );
    // Save when the drag ends, not on every tick of the drag.
    if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
        *dirty = true;
    }
}
