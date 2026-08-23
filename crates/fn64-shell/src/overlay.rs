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
use crate::video_config::VideoConfig;
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

/// Which settings tab the overlay is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Input,
    Video,
    Audio,
}

pub struct Overlay {
    pub open: bool,
    /// The selected settings tab, persisted across redraws (not to disk).
    tab: Tab,
    /// The always-cheap stack/framerate HUD (F3), independent of the settings
    /// panel: a player who wants to SEE which stack a build is on should not
    /// have to open a modal that neutralizes their input to read it.
    pub hud: bool,
    pub capture: Option<Capture>,
    ctx: egui::Context,
    renderer: Option<egui_wgpu::Renderer>,
    /// Pending egui events translated from winit, drained each frame.
    events: Vec<egui::Event>,
    /// Last cursor position in egui points.
    cursor: Pos2,
    started: std::time::Instant,
    /// Reset is deliberately two-step: a stray click must not replace every
    /// binding in a persistent user config.
    reset_armed: bool,
}

impl Overlay {
    pub fn new() -> Self {
        let ctx = egui::Context::default();
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = SMOKE;
        visuals.window_fill = SMOKE;
        visuals.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x2A, 0x2A, 0x2F));
        visuals.window_rounding = Rounding::same(6.0);
        visuals.override_text_color = Some(INK);
        ctx.set_visuals(visuals);
        Overlay {
            open: false,
            tab: Tab::default(),
            hud: false,
            capture: None,
            ctx,
            renderer: None,
            events: Vec::new(),
            cursor: Pos2::ZERO,
            started: std::time::Instant::now(),
            reset_armed: false,
        }
    }

    /// Create the egui GPU pipeline while the window is still being set up.
    /// `Renderer::new` is a one-time host cost; deferring it until F1 makes
    /// opening settings the frame that pays for shader/pipeline creation.
    pub fn prepare(&mut self, pixels: &pixels::Pixels<'static>) {
        self.renderer.get_or_insert_with(|| {
            egui_wgpu::Renderer::new(pixels.device(), pixels.surface_texture_format(), None, 1)
        });
    }

    /// Whether anything at all needs the egui pass this frame. `present()`
    /// takes the plain `pixels.render()` path when this is false, so a run
    /// with both surfaces closed pays exactly what it paid before.
    pub fn active(&self) -> bool {
        self.open || self.hud
    }

    pub fn toggle_hud(&mut self) {
        self.hud = !self.hud;
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.capture = None;
        self.reset_armed = false;
        // Undrained mouse events from the closing frame would replay into
        // the next open.
        self.events.clear();
    }

    /// Translate the few winit window events egui needs. Call for every
    /// window event while the overlay is open; cheap no-ops otherwise.
    pub fn on_window_event(&mut self, event: &WindowEvent, scale_factor: f32) {
        // Keyed to `open`, not `active()`: the HUD is a read-only overlay with
        // no widgets, so feeding it pointer events would only accumulate a
        // queue nothing drains.
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

    /// A keyboard key arrived while a capture was armed. Delete/Backspace
    /// clears either kind of slot; any other key binds a keyboard slot.
    /// Returns true when the event changed the active capture.
    /// Jump to a tab (F1/F2/F3 while the panel is open). A pending key capture
    /// is dropped -- switching tabs abandons an armed rebind.
    pub fn select_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.capture = None;
    }

    pub fn apply_key_capture(&mut self, config: &mut InputConfig, key: KeyCode) -> bool {
        let clear = matches!(key, KeyCode::Delete | KeyCode::Backspace);
        match self.capture {
            Some(Capture::Key(target)) if clear => config.unbind_key(target),
            Some(Capture::Pad(target)) if clear => config.unbind_pad(target),
            Some(Capture::Key(target)) => config.bind_key(target, key),
            Some(Capture::Pad(_)) | None => return false,
        }
        config.save();
        self.capture = None;
        if clear {
            println!("[fn64-shell] input: cleared binding via settings overlay");
        } else {
            println!("[fn64-shell] input: bound {key:?} via settings overlay");
        }
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
        video: &mut VideoConfig,
        gamepads: &Gamepads,
        hud: Option<&HudReadout>,
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
        let mut reset_armed = self.reset_armed;
        let mut tab = self.tab;
        let mut dirty = false;
        let mut video_dirty = false;
        let mut close_requested = false;
        let settings_open = self.open;
        let full_output = self.ctx.clone().run(raw, |ctx| {
            if let Some(readout) = hud {
                draw_hud(ctx, readout);
            }
            if settings_open {
                draw_ui(
                    ctx,
                    config,
                    video,
                    gamepads,
                    &mut tab,
                    &mut capture,
                    &mut reset_armed,
                    &mut dirty,
                    &mut video_dirty,
                    &mut close_requested,
                );
            }
        });
        self.capture = capture;
        self.reset_armed = reset_armed;
        self.tab = tab;
        if close_requested {
            self.open = false;
            self.capture = None;
            self.reset_armed = false;
        }
        if dirty {
            config.save();
        }
        if video_dirty {
            video.save();
        }

        let pixels_per_point = full_output.pixels_per_point;
        let primitives = self.ctx.tessellate(full_output.shapes, pixels_per_point);
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
                egui_wgpu::Renderer::new(&context.device, pixels.surface_texture_format(), None, 1)
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

/// Everything the on-screen HUD paints: the build's fixed stack identity plus
/// the live timing line. Assembled by the caller from `crate::stack`, so the
/// drawing code here never re-derives a fact it could get wrong.
pub struct HudReadout {
    /// `(label, value)` identity rows -- see `stack::hud_identity`.
    pub identity: [(&'static str, String); 2],
    /// The live timing line, or `None` before the first heartbeat window has
    /// enough samples. Shown as "measuring" rather than as a fabricated zero:
    /// a HUD that displays 0.0 fps for the first second teaches the reader to
    /// distrust it.
    pub live: Option<String>,
    /// True when the renderer is a silent fallback, so the panel can carry the
    /// alarm colour rather than only the text.
    pub alarm: bool,
}

/// A compact top-left panel. Deliberately not a window: no title bar, no
/// interaction, no hit-testing -- it must not become the thing that perturbs
/// the measurement it displays.
fn draw_hud(ctx: &egui::Context, readout: &HudReadout) {
    let frame = egui::Frame::none()
        .fill(Color32::from_black_alpha(190))
        .rounding(Rounding::same(4.0))
        .inner_margin(egui::Margin::symmetric(8.0, 6.0))
        .stroke(Stroke::new(
            1.0,
            if readout.alarm {
                START_RED
            } else {
                Color32::from_rgb(0x2A, 0x2A, 0x2F)
            },
        ));
    egui::Area::new(egui::Id::new("fn64-hud"))
        .order(egui::Order::Foreground)
        // Offset from the corner so it clears a window manager's rounding and
        // stays readable in borderless fullscreen.
        .fixed_pos(Pos2::new(10.0, 10.0))
        .interactable(false)
        .show(ctx, |ui| {
            frame.show(ui, |ui| {
                for (label, value) in &readout.identity {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(*label).color(MUTED).small().strong());
                        ui.label(
                            RichText::new(value)
                                .color(if readout.alarm && *label == "GPU" {
                                    START_RED
                                } else {
                                    INK
                                })
                                .small()
                                .monospace(),
                        );
                    });
                }
                ui.label(
                    RichText::new(readout.live.as_deref().unwrap_or("measuring…"))
                        .color(if readout.live.is_some() { INK } else { MUTED })
                        .small()
                        .monospace(),
                );
            });
        });
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

#[allow(clippy::too_many_arguments)]
fn draw_ui(
    ctx: &egui::Context,
    config: &mut InputConfig,
    video: &mut VideoConfig,
    gamepads: &Gamepads,
    tab: &mut Tab,
    capture: &mut Option<Capture>,
    reset_armed: &mut bool,
    dirty: &mut bool,
    video_dirty: &mut bool,
    close_requested: &mut bool,
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

    // The bindings list grows with the binding count while the viewport does
    // not, so the panel must be BOUNDED by the screen rather than sized by its
    // content: an unbounded CENTER_CENTER window overflows symmetrically and
    // clips its own title off the top edge (measured 752.5px of content in a
    // 720px viewport, panel top at y=-16.5). Reserve room for the title bar,
    // hint row, and frame padding, then let the body scroll inside what is
    // left.
    let screen = ctx.screen_rect();
    // Room for the title bar, the hint row, and the window frame's padding.
    let chrome = 142.0;
    // NO lower floor here: a `.max(160.0)` would win whenever the viewport is
    // under 256px tall and hand the ScrollArea more height than the screen
    // has, reintroducing the clipped-title bug it exists to fix. Both the game
    // path and the demo set a 320x240 minimum inner size, so that range is
    // reachable by dragging the window down. Clamp to something still usable
    // instead, and let the ScrollArea scroll.
    let body_max_height = (screen.height() - chrome).clamp(48.0, screen.height());
    egui::Window::new(RichText::new("SETTINGS").strong().color(INK))
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .max_height(screen.height() - 24.0)
        .max_width(screen.width() - 24.0)
        .show(ctx, |ui| {
            // Tab bar: Input / Video / Audio. Selection lives in `tab`, owned
            // by the Overlay so it survives redraws. The F-key affordance
            // matches the shortcuts wired in main.rs (F1/F2/F3 while open).
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(*tab == Tab::Input, RichText::new("Input  F1").strong())
                    .clicked()
                {
                    *tab = Tab::Input;
                    *capture = None;
                }
                if ui
                    .selectable_label(*tab == Tab::Video, RichText::new("Video  F2").strong())
                    .clicked()
                {
                    *tab = Tab::Video;
                    *capture = None;
                }
                if ui
                    .selectable_label(*tab == Tab::Audio, RichText::new("Audio  F3").strong())
                    .clicked()
                {
                    *tab = Tab::Audio;
                    *capture = None;
                }
            });
            ui.separator();
            ui.add_space(4.0);

            match *tab {
                Tab::Input => draw_input_tab(
                    ui,
                    config,
                    gamepads,
                    capture,
                    reset_armed,
                    dirty,
                    body_max_height,
                ),
                Tab::Video => draw_video_tab(ui, video, video_dirty),
                Tab::Audio => draw_audio_tab(ui),
            }

            ui.add_space(6.0);
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new("Done").strong()).clicked() {
                    *close_requested = true;
                }
                ui.label(
                    RichText::new("Changes save automatically")
                        .color(MUTED)
                        .small(),
                );
            });
            ui.add_space(3.0);
            let hint = match capture {
                Some(Capture::Key(_)) => "press a key to bind · Delete clears · Esc cancels",
                Some(Capture::Pad(_)) => "press a controller button · Delete clears · Esc cancels",
                None => "F1/F2/F3 switch tabs · Esc closes · F11 fullscreen",
            };
            ui.label(RichText::new(hint).color(MUTED).small());
        });
}

/// The Input tab: the existing bindings + analog-stick UI, unchanged, hosted
/// under the tab bar.
fn draw_input_tab(
    ui: &mut egui::Ui,
    config: &mut InputConfig,
    gamepads: &Gamepads,
    capture: &mut Option<Capture>,
    reset_armed: &mut bool,
    dirty: &mut bool,
    body_max_height: f32,
) {
    {
            ui.horizontal(|ui| {
                ui.label(RichText::new("PLAYER 1").color(MUTED).small().strong());
                ui.separator();
                match gamepads.active_name() {
                    Some(name) => {
                        ui.label(RichText::new("CONNECTED").color(B_GREEN).small().strong());
                        ui.label(RichText::new(name).color(INK).small())
                    }
                    None => ui.label(
                        RichText::new("Keyboard only — connect a controller at any time")
                            .color(MUTED)
                            .small(),
                    ),
                };
            });
            ui.label(
                RichText::new("Select a slot, then press the key or controller button you want.")
                    .color(MUTED)
                    .small(),
            );
            ui.add_space(6.0);

            // ONE scroll area around BOTH columns, bounded by the viewport.
            // Previously each column sized itself freely and the window grew to
            // fit their max, so the panel could exceed the screen height and
            // clip its own title bar off the top.
            egui::ScrollArea::vertical()
                .max_height(body_max_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        // Left: bindings, grouped by the controller's physical
                        // regions (structure = the real hardware, not a flat list).
                        ui.vertical(|ui| {
                            bindings_grid(ui, config, capture);
                        });
                        ui.separator();
                        // Right: the analog column — deadzone + live scope.
                        ui.vertical(|ui| {
                            stick_scope(ui, config, gamepads, dirty);
                        });
                    });
                });

            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                if *reset_armed {
                    ui.label(
                        RichText::new("Replace every binding?")
                            .color(START_RED)
                            .small(),
                    );
                    if ui
                        .button(RichText::new("Confirm reset").color(START_RED))
                        .clicked()
                    {
                        config.restore_defaults();
                        *capture = None;
                        *reset_armed = false;
                        *dirty = true;
                    }
                    if ui.button("Cancel").clicked() {
                        *reset_armed = false;
                    }
                } else if ui.button("Restore defaults").clicked() {
                    *capture = None;
                    *reset_armed = true;
                }
            });
    }
}

/// The Video tab: the overscan crop and the zoom-to-fill toggle.
fn draw_video_tab(ui: &mut egui::Ui, video: &mut VideoConfig, video_dirty: &mut bool) {
    ui.label(RichText::new("DISPLAY").color(MUTED).small().strong());
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("Overscan crop");
        if ui
            .add(egui::Slider::new(&mut video.overscan, 0..=16).suffix(" px"))
            .changed()
        {
            *video_dirty = true;
        }
    });
    ui.label(
        RichText::new(
            "Columns cropped from the right edge on present. Hides the uncovered \
             overscan column some games leave as stale pixels. 0 shows the raw scanout.",
        )
        .color(MUTED)
        .small(),
    );

    ui.add_space(8.0);
    if ui
        .checkbox(&mut video.zoom_fill, "Zoom to fill window")
        .changed()
    {
        *video_dirty = true;
    }
    ui.label(
        RichText::new(
            "Stretch the picture to fill the whole window instead of keeping the \
             native aspect ratio with a matte. Distorts the aspect ratio.",
        )
        .color(MUTED)
        .small(),
    );
}

/// The Audio tab: a placeholder frame for future audio settings.
fn draw_audio_tab(ui: &mut egui::Ui) {
    ui.label(RichText::new("AUDIO").color(MUTED).small().strong());
    ui.add_space(4.0);
    ui.label(RichText::new("No audio settings yet.").color(MUTED));
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
        ui.label(RichText::new("CONTROL").color(MUTED).small());
        ui.label(RichText::new("KEYBOARD").color(MUTED).small());
        ui.label(RichText::new("GAMEPAD").color(MUTED).small());
        ui.end_row();
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
            if ui
                .button(RichText::new(text).monospace())
                .on_hover_text("Click, then press a keyboard key")
                .clicked()
            {
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
                if ui
                    .button(RichText::new(key_text).monospace())
                    .on_hover_text("Click, then press a keyboard key")
                    .clicked()
                {
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
                if ui
                    .button(RichText::new(pad_text).monospace())
                    .on_hover_text("Click, then press a controller button")
                    .clicked()
                {
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

    painter.circle_stroke(center, radius, Stroke::new(1.0_f32, MUTED));
    painter.circle_filled(
        center,
        radius * config.deadzone,
        Color32::from_rgb(0x22, 0x22, 0x27),
    );
    painter.circle_stroke(
        center,
        radius * config.deadzone,
        Stroke::new(1.0_f32, Color32::from_rgb(0x3A, 0x3A, 0x41)),
    );

    // egui y grows downward; stick y grows upward.
    let to_screen = |x: f32, y: f32| center + Vec2::new(x, -y) * radius;
    let (rx, ry) = gamepads.raw_stick();
    let (fx, fy) = apply_deadzone_f(rx, ry, config.deadzone);
    painter.circle_filled(to_screen(rx, ry), 3.0, MUTED);
    painter.circle_filled(to_screen(fx, fy), 4.5, C_YELLOW);
    painter.line_segment(
        [to_screen(rx, ry), to_screen(fx, fy)],
        Stroke::new(1.0_f32, Color32::from_rgb(0x3A, 0x3A, 0x41)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_config() -> InputConfig {
        InputConfig {
            persist: false,
            ..InputConfig::default()
        }
    }

    #[test]
    fn armed_keyboard_capture_rebinds_once_and_disarms() {
        let mut overlay = Overlay::new();
        let mut config = scratch_config();
        overlay.capture = Some(Capture::Key(BindTarget::Button(N64Button::A)));

        assert!(overlay.apply_key_capture(&mut config, KeyCode::F12));
        assert_eq!(config.keyboard.get(&N64Button::A), Some(&KeyCode::F12));
        assert_eq!(overlay.capture, None);
        assert!(!overlay.apply_key_capture(&mut config, KeyCode::F11));
        assert_eq!(config.keyboard.get(&N64Button::A), Some(&KeyCode::F12));
    }

    #[test]
    fn delete_clears_keyboard_or_gamepad_capture_and_disarms() {
        let mut overlay = Overlay::new();
        let mut config = scratch_config();

        overlay.capture = Some(Capture::Key(BindTarget::Stick(StickDir::Up)));
        assert!(overlay.apply_key_capture(&mut config, KeyCode::Delete));
        assert!(!config.keyboard_stick.contains_key(&StickDir::Up));
        assert_eq!(overlay.capture, None);

        overlay.capture = Some(Capture::Pad(N64Button::Start));
        assert!(overlay.apply_key_capture(&mut config, KeyCode::Backspace));
        assert!(!config.gamepad.contains_key(&N64Button::Start));
        assert_eq!(overlay.capture, None);
    }

    #[test]
    fn toggle_clears_transient_capture_and_reset_state() {
        let mut overlay = Overlay::new();
        overlay.capture = Some(Capture::Pad(N64Button::Start));
        overlay.reset_armed = true;

        overlay.toggle();

        assert!(overlay.open);
        assert_eq!(overlay.capture, None);
        assert!(!overlay.reset_armed);
    }
}
