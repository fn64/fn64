//! fn64-shell (`fn64`): the interactive, windowed harness -- the thing that
//! makes a recompiled N64 game PLAYABLE. It does the SAME boot + executor
//! drive as the headless `examples/oot-boot` (load ROM, register sections
//! via the C bridge, boot thread 0 on `recomp_entrypoint`, drive
//! `run_one_step`/`advance_virtual_time`), but instead of dumping the VI
//! framebuffer to PNGs it:
//!
//! 1. **Presents** the game's live VI framebuffer (rdram, RGBA5551 320x240 --
//!    see `framebuffer.rs`) to a resizable `winit` window every VI swap, via
//!    the `pixels` (wgpu) pixel-buffer presenter with correct aspect.
//! 2. **Feeds live keyboard input** to the game: each frame the current
//!    `PadState` (see `input_map.rs`) is pushed through
//!    `fn64_abi::set_controller_state(0, ..)` so the game's next
//!    `osContGetReadData` sees it.
//! 3. **Wires audio output**: registers `fn64_audio::CpalBackend` via
//!    `fn64_abi::set_audio_backend`, so `osAiSetNextBuffer`'s finished PCM
//!    reaches a live cpal output stream. The linked game/harness separately
//!    registers the recompiled ucode that produces those samples.
//!
//! ## Game intake (same contract as oot-boot)
//!
//! The recompiled game is linked at BUILD time from `RECOMPILED_DIR`/
//! `RECOMP_H_DIR`/`ROM` (see `build.rs`). When those are unset the shell
//! still builds and runs -- it just prints the intake instructions and
//! exits (no window), because there's no game to boot. That keeps
//! `cargo build --workspace` green with zero game content.
//!
//! Run it (OoT, with the perf-friendly audio-ucode skip):
//! ```text
//! RECOMPILED_DIR=.../OOTU/RecompiledFuncs \
//! ROM=.../oot-ntsc-1.0.z64 \
//! FN64_SKIP_AUDIO_UCODE=1 \
//! cargo run -p fn64-shell
//! ```

// These two modules are consumed by the `#[cfg(fn64_game_linked)]` `game`
// module (the live window loop) and by their own unit tests. In a
// content-free build (no game linked) the binary's `main` never touches them,
// so `dead_code` would fire on every item -- allow it here rather than
// littering per-item attributes; the tests + game module are the real users.
#[allow(dead_code)]
mod framebuffer;
#[allow(dead_code)]
mod input_map;

#[cfg(not(fn64_game_linked))]
fn main() {
    // Content-free build: no game symbols were linked (RECOMPILED_DIR was
    // unset at build time -- see build.rs). Report honestly instead of
    // opening an empty window with nothing to boot.
    eprintln!(
        "fn64-shell: built WITHOUT a linked game (RECOMPILED_DIR was unset at build time).\n\
         \n\
         To get a live, playable window, rebuild with the game intake env vars set (same\n\
         contract as examples/oot-boot), e.g. for OoT:\n\
         \n\
         \x20 RECOMPILED_DIR=.../OOTU/RecompiledFuncs \\\n\
         \x20 ROM=.../oot-ntsc-1.0.z64 \\\n\
         \x20 FN64_SKIP_AUDIO_UCODE=1 \\\n\
         \x20 cargo run -p fn64-shell\n\
         \n\
         (Add --features oot-audio-ucode to link the recompiled audio ucode so the wired\n\
         cpal output stream actually gets samples.)"
    );
    std::process::exit(2);
}

#[cfg(fn64_game_linked)]
fn main() {
    game::run();
}

/// OoT's host-first rs-lane lookup table, shared verbatim with the headless
/// harness (game-profile data beside the OoT harness, not duplicated here —
/// see that file's module doc).
#[cfg(fn64_recomp_rs)]
#[path = "../../../examples/oot-boot/src/host_lookup.rs"]
mod host_lookup;

/// Everything that requires the linked game symbols lives here, gated on the
/// `fn64_game_linked` cfg `build.rs` sets only when it compiled the
/// RecompiledFuncs. Keeping it in one `cfg`'d module means the content-free
/// build never references an unresolved `recomp_entrypoint`.
#[cfg(fn64_game_linked)]
mod game {
    use crate::framebuffer::{self, FB_BYTES, FB_HEIGHT, FB_WIDTH};
    use crate::input_map::PadState;
    use std::sync::Arc;

    #[cfg(fn64_recomp_rs)]
    use oot_recompiled as recompiled;

    use pixels::{Pixels, SurfaceTexture};
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::{ElementState, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::keyboard::{KeyCode, PhysicalKey};
    use winit::window::{Window, WindowId};

    fn env_path(name: &str) -> std::path::PathBuf {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("fn64-shell: required environment variable {name} not set"))
            .into()
    }

    /// Boot the game and hold everything the window loop touches every frame.
    struct Shell {
        rdram: Vec<u8>,
        pad: PadState,
        last_swap_count: u64,
        /// Scratch RGBA8888 buffer the VI framebuffer unpacks into before
        /// blitting to the pixels surface -- allocated once, reused per frame.
        rgba: Vec<u8>,
        /// `Arc<Window>` (not a bare `Window`) so the `SurfaceTexture`
        /// pixels holds can own a `'static` window handle, letting `pixels`
        /// be `Pixels<'static>` -- otherwise pixels 0.15 borrows the window
        /// and the self-referential lifetime can't live in one struct.
        window: Option<Arc<Window>>,
        pixels: Option<Pixels<'static>>,
        reported_first_frame: bool,
        last_heartbeat_swap: u64,
        /// Wall-clock deadline for the next pumped frame (~60 Hz pacing).
        next_frame_deadline: std::time::Instant,
    }

    /// How many executor steps to run per window pump before yielding back to
    /// winit, so input/close stay responsive even while the game grinds. One
    /// VI frame is well under this; the cap only bounds a pathological spin.
    const STEPS_PER_PUMP: u64 = 200_000;

    impl Shell {
        fn boot() -> Self {
            let rom_path = env_path("ROM");
            println!("[fn64-shell] loading ROM from {}", rom_path.display());
            let rom_bytes = std::fs::read(&rom_path).unwrap_or_else(|e| {
                panic!("fn64-shell: failed to read ROM {}: {e}", rom_path.display())
            });
            println!("[fn64-shell] ROM size: {} bytes", rom_bytes.len());
            fn64_abi::load_rom(rom_bytes);

            // OoT NTSC 1.0's aligned __CartRomHandle BSS address: guest code
            // dereferences osCartRomInit's returned handle, so the shim must
            // return the game-linked object, not an opaque token (same
            // registration as oot-boot main.rs -- its absence aborts boot in
            // bootproc's osCartRomInit call).
            fn64_abi::set_cart_rom_handle_vram(0x8000_9EA0);

            // Save-backing store (banked SRAM), same as oot-boot -- domain-2
            // PI DMAs need somewhere to land.
            fn64_abi::set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
                fn64_runtime::SaveType::SramBanked,
            )));

            #[cfg(not(fn64_recomp_rs))]
            {
                let registration = fn64_boot_harness::register_linked_sections();
                println!(
                    "[fn64-shell] bridge reports {} sections",
                    registration.reported_count()
                );
                // Always-resident sections 0/1/2 (makerom.ent/boot/code), same
                // as oot-boot -- the ovl_* overlays are DMA-loaded on demand.
                for section_key in [0usize, 1usize, 2usize] {
                    if let Some(idx) = registration.registry_index(section_key) {
                        fn64_abi::set_section_loaded(idx);
                    }
                }
            }
            #[cfg(fn64_recomp_rs)]
            {
                let section_indices: Vec<_> = recompiled::RECOMPILED_SECTION_GEOMETRY
                    .iter()
                    .map(|&(rom_addr, ram_addr, size)| {
                        fn64_abi::register_recompiled_section(rom_addr, ram_addr, size)
                    })
                    .collect();
                for &section_key in &[0usize, 1, 2] {
                    fn64_abi::set_section_loaded(section_indices[section_key]);
                }
                println!(
                    "[fn64-shell] registered {} recompiled section geometries; marked 0/1/2 resident",
                    section_indices.len()
                );
            }

            // VI retrace ticker (host-chosen approximation, same as oot-boot).
            fn64_abi::arm_vi_retrace(1000);

            let mut rdram = fn64_boot_harness::new_rdram();
            let rdram_ptr = rdram.as_mut_ptr();

            // Render backend selection (same contract as oot-boot):
            // FN64_RENDER=rt64 uses the RT64 static backend (the eyes-verified
            // faithful renderer; requires the `rt64` feature), which writes
            // its frame back into the rdram VI framebuffer we present. The
            // default remains the software ReferenceBackend (the CI oracle).
            use fn64_render::RenderBackend as _;
            let create_reference = || -> Box<dyn fn64_render::RenderBackend> {
                let mut backend = fn64_render_rt64::ReferenceBackend::new()
                    .with_f3dex2()
                    .with_clear_color([0, 0, 0, 255]);
                if let Err(e) = backend.create(&fn64_render::RenderConfig::new(
                    FB_WIDTH as u32,
                    FB_HEIGHT as u32,
                )) {
                    eprintln!("[fn64-shell] WARNING: render backend create() failed: {e}");
                }
                Box::new(backend)
            };
            let requested_renderer = std::env::var("FN64_RENDER")
                .unwrap_or_else(|_| "reference".to_string())
                .to_ascii_lowercase();
            let (render_backend, active_renderer): (
                Box<dyn fn64_render::RenderBackend>,
                &'static str,
            ) = if requested_renderer == "rt64" {
                let mut backend = fn64_render_rt64::Rt64Backend::new();
                match backend.create(&fn64_render::RenderConfig::new(
                    FB_WIDTH as u32,
                    FB_HEIGHT as u32,
                )) {
                    Ok(()) => (Box::new(backend), "rt64"),
                    Err(error) => {
                        eprintln!(
                            "[fn64-shell] WARNING: RT64 create failed ({error}); falling back \
                             to the ReferenceBackend oracle"
                        );
                        (create_reference(), "reference-fallback")
                    }
                }
            } else {
                if requested_renderer != "reference" {
                    eprintln!(
                        "[fn64-shell] WARNING: unknown FN64_RENDER={requested_renderer:?}; \
                         using ReferenceBackend"
                    );
                }
                (create_reference(), "reference")
            };
            fn64_abi::set_render_backend(render_backend, rdram.len());
            println!("[fn64-shell] render backend registered ({active_renderer}, 320x240)");

            // Audio OUTPUT path: a live cpal output stream. This is the
            // shell's audio deliverable -- samples produced by M_AUDTASK and
            // submitted through osAiSetNextBuffer flow here. Gated so a headless/CI env with no audio
            // device doesn't abort; a create() failure is logged, not fatal.
            // Return value (the negotiated stream rate) is already logged by
            // wire_audio; pacing no longer needs it -- osAiGetLength's live
            // readback lets the game self-pace audio production.
            let _ = wire_audio(rdram.len());

            // Audio SYNTH (ucode): optional, behind the feature. Without it,
            // M_AUDTASK dispatch runs no ucode (silent) but the output path
            // above is still live.
            wire_audio_ucode(rdram.len());

            println!("[fn64-shell] booting thread 0 (recomp_entrypoint)...");
            #[cfg(fn64_recomp_rs)]
            {
                fn64_recomp_rs::set_host_lookup(Some(
                    crate::host_lookup::recompiled_or_host_lookup,
                ));
                println!(
                    "[fn64-shell] FN64_RECOMP=rs: linked oot-recompiled crate + host-first \
                     recompiled adapters active"
                );
                // SAFETY: `rdram` is owned by the returned Shell, which lives
                // until process exit (`clean_exit` terminates via `_exit`, so
                // no coroutine outlives it).
                unsafe {
                    fn64_abi::recompiled::boot_thread0(
                        rdram_ptr,
                        rdram.len(),
                        recompiled::lookup,
                        recompiled::entrypoint,
                        0,
                        10,
                    );
                }
            }
            #[cfg(not(fn64_recomp_rs))]
            unsafe {
                fn64_abi::boot_thread0(rdram_ptr, fn64_boot_harness::c_recomp_entrypoint(), 0, 10);
            }

            Shell {
                rdram,
                pad: PadState::new(),
                last_swap_count: 0,
                rgba: vec![0u8; FB_WIDTH * FB_HEIGHT * 4],
                window: None,
                pixels: None,
                reported_first_frame: false,
                last_heartbeat_swap: 0,
                next_frame_deadline: std::time::Instant::now(),
            }
        }

        /// Drive the executor until the next VI swap (or the per-pump step
        /// budget is hit). Returns true if a new VI swap happened this pump.
        fn pump_one_frame(&mut self) -> bool {
            let start_swaps = fn64_abi::vi_swap_count();
            let mut idle_ticks = 0u32;
            let mut tick = fn64_abi::sim_time();
            let mut steps = 0u64;
            loop {
                if steps >= STEPS_PER_PUMP {
                    break;
                }
                // Feed live input before the game polls the controller this
                // step. Cheap; keeps the pad current within the frame.
                let (buttons, sx, sy) = self.pad.resolve();
                fn64_abi::set_controller_state(0, buttons, sx, sy);

                let stepped = fn64_abi::run_one_step();
                steps += 1;
                if fn64_abi::vi_swap_count() > start_swaps {
                    return true;
                }
                if !stepped {
                    tick += 100;
                    fn64_abi::advance_virtual_time(tick);
                    idle_ticks += 1;
                    if idle_ticks >= 200 {
                        // Genuinely idle -- nothing will produce a frame this
                        // pump. Yield to winit so the window stays responsive.
                        break;
                    }
                } else {
                    idle_ticks = 0;
                    // A voluntary `pause_self` idle loop is runnable by
                    // definition (the rs lane's idle thread spins instead of
                    // blocking), so gating host time on `!stepped` starves
                    // the VI retrace forever: the window freezes on the first
                    // frame and no audio task ever runs. Same host clock
                    // injection as oot-boot's drive loop: pace every 100
                    // scheduling steps.
                    if steps.is_multiple_of(100) {
                        tick += 100;
                        fn64_abi::advance_virtual_time(tick);
                    }
                }
            }
            fn64_abi::vi_swap_count() > start_swaps
        }

        /// Blit the current VI framebuffer (rdram) into the pixels surface and
        /// present. Reports blank/uniform frames honestly.
        fn present(&mut self) {
            let Some(pixels) = self.pixels.as_mut() else {
                return;
            };
            let fb_offset = match fn64_abi::current_vi_framebuffer() {
                Some(o) => o as usize,
                None => return,
            };
            // The `^ 2` halfword decode in rgba5551_to_rgba8888 assumes a
            // word-aligned framebuffer base (every real VI fb is). Loud, not
            // silent, if that ever breaks.
            if fb_offset % 4 != 0 {
                eprintln!(
                    "[fn64-shell] VI framebuffer at {fb_offset:#x} is not word-aligned -- \
                     skipping present (decode assumption violated)"
                );
                return;
            }
            let end = fb_offset + FB_BYTES;
            let region: &[u8] = if end <= self.rdram.len() {
                &self.rdram[fb_offset..end]
            } else if fb_offset < self.rdram.len() {
                &self.rdram[fb_offset..]
            } else {
                return;
            };

            let blank = framebuffer::is_uniform(region);
            framebuffer::rgba5551_to_rgba8888(region, &mut self.rgba);
            pixels.frame_mut().copy_from_slice(&self.rgba);
            if let Err(e) = pixels.render() {
                eprintln!("[fn64-shell] pixels.render() failed: {e}");
                return;
            }

            if !self.reported_first_frame {
                let swaps = fn64_abi::vi_swap_count();
                if blank {
                    println!(
                        "[fn64-shell] presenting VI framebuffer (swap #{swaps}) -- currently \
                         BLANK/uniform (game hasn't rendered visible geometry yet; the projection \
                         path may still be landing). Window + present path are live."
                    );
                } else {
                    println!(
                        "[fn64-shell] presenting VI framebuffer (swap #{swaps}) -- NON-BLANK \
                         content."
                    );
                }
                self.reported_first_frame = true;
            } else {
                // Periodic heartbeat so the log honestly shows the game is
                // advancing frames (VI swaps climbing), not stuck on swap #1.
                let swaps = fn64_abi::vi_swap_count();
                if swaps >= self.last_heartbeat_swap + 60 {
                    let state = if blank { "blank" } else { "NON-BLANK" };
                    // Audio counters in the same line: shows at a glance
                    // whether the game is producing PCM (ai_buffers/nonzero)
                    // and whether it reaches the backend (backend_buffers).
                    let audio = fn64_abi::audio_output_stats();
                    println!(
                        "[fn64-shell] present heartbeat: VI swap #{swaps} ({state}); audio: \
                         ai_buffers={} samples={} nonzero={} backend_buffers={} ring_frames={:?}",
                        audio.ai_buffers,
                        audio.samples,
                        audio.nonzero_samples,
                        audio.backend_buffers,
                        fn64_abi::audio_frames_remaining()
                    );
                    self.last_heartbeat_swap = swaps;
                }
            }
        }
    }

    impl ApplicationHandler for Shell {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let size = LogicalSize::new((FB_WIDTH * 2) as f64, (FB_HEIGHT * 2) as f64);
            let attrs = Window::default_attributes()
                .with_title("fn64 -- N64 recompilation")
                .with_inner_size(size)
                .with_min_inner_size(LogicalSize::new(FB_WIDTH as f64, FB_HEIGHT as f64));
            let window = match event_loop.create_window(attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    eprintln!("[fn64-shell] failed to create window: {e}");
                    event_loop.exit();
                    return;
                }
            };
            let win_size = window.inner_size();
            // `Arc<Window>` is `'static + HasWindowHandle`, so the resulting
            // Pixels is `Pixels<'static>` and can be stored alongside the
            // window in `Shell` without a self-referential borrow.
            let surface = SurfaceTexture::new(win_size.width, win_size.height, Arc::clone(&window));
            match Pixels::new(FB_WIDTH as u32, FB_HEIGHT as u32, surface) {
                Ok(px) => {
                    self.pixels = Some(px);
                    self.window = Some(window);
                    println!(
                        "[fn64-shell] window opened ({}x{})",
                        win_size.width, win_size.height
                    );
                }
                Err(e) => {
                    eprintln!("[fn64-shell] failed to create pixels surface: {e}");
                    event_loop.exit();
                }
            }
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            _id: WindowId,
            event: WindowEvent,
        ) {
            match event {
                WindowEvent::CloseRequested => {
                    println!("[fn64-shell] window close requested -- exiting");
                    event_loop.exit();
                }
                WindowEvent::Resized(new_size) => {
                    if let Some(px) = self.pixels.as_mut() {
                        // Keep the game's 320x240 aspect; pixels letterboxes
                        // the surface to the window automatically.
                        if let Err(e) =
                            px.resize_surface(new_size.width.max(1), new_size.height.max(1))
                        {
                            eprintln!("[fn64-shell] resize_surface failed: {e}");
                        }
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if event.repeat {
                        return;
                    }
                    if let PhysicalKey::Code(code) = event.physical_key {
                        if code == KeyCode::Escape && event.state == ElementState::Pressed {
                            println!("[fn64-shell] Esc pressed -- exiting");
                            event_loop.exit();
                            return;
                        }
                        let pressed = event.state == ElementState::Pressed;
                        if self.pad.apply(code, pressed) {
                            // Log every real change so the keyboard->controller
                            // path is observable in the run log (the state
                            // pushed here is what set_controller_state feeds the
                            // game each pump). Only on change -- not per repeat.
                            let (b, sx, sy) = self.pad.resolve();
                            println!(
                                "[fn64-shell] input: key {code:?} {} -> pad buttons={b:#06x} \
                                 stick=({sx},{sy})",
                                if pressed { "down" } else { "up" }
                            );
                        }
                    }
                }
                WindowEvent::RedrawRequested => {
                    self.present();
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            // Real-time pacing WITHOUT blocking the event thread: pump one
            // game frame when the ~16.67 ms wall deadline is due, then hand
            // the loop a WaitUntil so input/close events keep flowing while
            // we wait. Audio stays synchronized WITHOUT being the pacing
            // master here: `osAiGetLength` reports the live ring backlog, so
            // the game's own AudioMgr skips/queues buffers against real
            // drain exactly as on hardware (the ring cap + the heartbeat's
            // ring_frames are the backstop and the evidence).
            const FRAME: std::time::Duration = std::time::Duration::from_nanos(16_666_667);

            let now_t = std::time::Instant::now();
            if now_t >= self.next_frame_deadline {
                let swapped = self.pump_one_frame();
                if swapped {
                    self.last_swap_count = fn64_abi::vi_swap_count();
                }
                // Catch-up-free schedule: hold cadence while we keep up,
                // re-anchor (dropping missed frames) when we fall behind.
                self.next_frame_deadline =
                    if now_t.saturating_duration_since(self.next_frame_deadline) < FRAME {
                        self.next_frame_deadline + FRAME
                    } else {
                        now_t + FRAME
                    };
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame_deadline));
        }
    }

    pub fn run() {
        let mut shell = Shell::boot();

        // Headless input-seam self-test: `FN64_INPUT_PROBE=<KEY>` (e.g.
        // `FN64_INPUT_PROBE=Enter`) drives ONE press through the exact
        // PadState + set_controller_state path a real keypress uses, before
        // the window loop, and asserts the game-facing state is non-neutral.
        // Lets a headless runner (no physical keyboard) prove keyboard ->
        // controller wiring reaches fn64_abi, then exits.
        if let Some(key) = std::env::var_os("FN64_INPUT_PROBE") {
            input_probe(&mut shell, &key.to_string_lossy());
            clean_exit(0);
        }

        let event_loop = EventLoop::new().expect("fn64-shell: failed to build winit event loop");
        // Poll (not Wait): the game runs continuously, we're not idle-waiting
        // on OS events.
        event_loop.set_control_flow(ControlFlow::Poll);
        if let Err(e) = event_loop.run_app(&mut shell) {
            eprintln!("[fn64-shell] event loop error: {e}");
        }
        println!("[fn64-shell] exited cleanly.");
        clean_exit(0);
    }

    /// Exit the process WITHOUT running global/TLS destructors.
    ///
    /// The one global `fn64_abi` executor holds a booted game's `GameThread`s,
    /// each wrapping a guest stack woven through the linked C
    /// `RecompiledFuncs`. `GameThread::drop` panics when run against a thread
    /// that hasn't cleanly finished, and that Drop fires from the executor's
    /// thread-local destructor at teardown -> "panic in a function that
    /// cannot unwind" -> SIGABRT (observed: exit 134 after an otherwise clean
    /// run/probe). `std::process::exit` does NOT prevent this on macOS, where
    /// pthread-key TLS destructors still run during `exit()`. `_exit(2)`
    /// terminates immediately, skipping every atexit handler and TLS
    /// destructor. A shell has nothing to persist on exit -- the save store is
    /// flushed on each write, and window/audio OS handles are reclaimed by the
    /// kernel on process death -- so immediate termination is the correct
    /// clean shutdown here, not a workaround hiding real cleanup.
    fn clean_exit(code: i32) -> ! {
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        // Safety: `_exit` is always safe to call; it never returns.
        unsafe { libc::_exit(code) }
    }

    /// Drive the keyboard->controller path once, headlessly, and report what
    /// reached `fn64_abi`. Proves the seam without a physical keyboard.
    fn input_probe(shell: &mut Shell, key_name: &str) {
        let Some(code) = crate::input_map::key_from_name(key_name) else {
            eprintln!(
                "[fn64-shell] FN64_INPUT_PROBE={key_name:?}: unknown key name (try Enter, X, Z, \
                 ArrowUp, KeyW, ...)"
            );
            return;
        };
        println!("[fn64-shell] INPUT PROBE: simulating key-down {code:?}");
        let changed = shell.pad.apply(code, true);
        let (buttons, sx, sy) = shell.pad.resolve();
        // This is the exact call the live window loop makes each pump.
        fn64_abi::set_controller_state(0, buttons, sx, sy);
        println!(
            "[fn64-shell] INPUT PROBE: pad changed={changed}, pushed to fn64_abi port 0: \
             buttons={buttons:#06x} stick=({sx},{sy})"
        );
        assert!(
            changed && (buttons != 0 || sx != 0 || sy != 0),
            "input probe produced a neutral pad -- key mapping did not reach a button/stick bit"
        );
        // Step a few frames with the input held to show the game runs with it
        // applied (not a hang), then stop.
        for _ in 0..3 {
            shell.pump_one_frame();
        }
        println!(
            "[fn64-shell] INPUT PROBE OK: keyboard->controller reached fn64_abi and the game \
             stepped {} VI swaps with input held.",
            fn64_abi::vi_swap_count()
        );
    }

    /// Register the cpal output stream as the audio backend. A create()
    /// failure (no device, headless CI) is logged, not fatal -- the game
    /// still runs and presents; only sound is unavailable. Returns the
    /// negotiated host stream rate when audio is live: the window loop uses
    /// it as the frame-pacing master clock.
    fn wire_audio(rdram_len: usize) -> Option<u32> {
        if std::env::var_os("FN64_NO_AUDIO").is_some() {
            println!("[fn64-shell] FN64_NO_AUDIO set -- audio output disabled");
            return None;
        }
        use fn64_audio::{AudioBackend as _, AudioConfig, CpalBackend};
        // Request the guest's own rate; `CpalBackend::create` negotiates
        // with the device itself (falling back to the device-default rate
        // plus producer-side linear resampling), and `osAiSetFrequency`
        // keeps the ratio tracking the game afterward. The old ladder here
        // tried 48 kHz FIRST: on hosts that accept it, the stream then
        // drained 1.5x faster than the N64's 32 kHz production, the ring
        // chronically starved, and the callback's zero-fill was audible as
        // loud static (worse when backgrounded, as App Nap throttles the
        // producer further).
        const N64_BOOT_AI_RATE_HZ: u32 = 32_000;
        let mut backend = CpalBackend::new();
        match backend.create(&AudioConfig::new(N64_BOOT_AI_RATE_HZ, 2)) {
            Ok(()) => {
                let stream_rate = backend.stream_rate_hz().unwrap_or(N64_BOOT_AI_RATE_HZ);
                fn64_abi::set_audio_backend(Box::new(backend), rdram_len);
                println!(
                    "[fn64-shell] audio output wired (cpal, guest {N64_BOOT_AI_RATE_HZ} Hz -> \
                     stream {stream_rate} Hz stereo)"
                );
                Some(stream_rate)
            }
            Err(e) => {
                eprintln!(
                    "[fn64-shell] audio output unavailable ({e}) -- continuing SILENT \
                     (window/input unaffected). Set FN64_NO_AUDIO to silence this."
                );
                None
            }
        }
    }

    /// The audio SYNTH seam. The cpal OUTPUT path (`wire_audio`) is already
    /// live; this is where a real recompiled audio ucode would be registered
    /// via `fn64_abi::set_audio_ucode_fn` so M_AUDTASK dispatch produces the
    /// samples that output stream drains. No such ucode is linked today (the
    /// only one available cross-pins `fn64-audio` -- see Cargo.toml), so this
    /// only reports the wiring status. Enabling the `oot-audio-ucode` feature
    /// flips the message; the actual `set_audio_ucode_fn` call lands here when
    /// a non-cross-pinning ucode crate becomes a real dependency.
    #[cfg(all(fn64_recomp_rs, feature = "oot-audio"))]
    fn wire_audio_ucode(rdram_len: usize) {
        // The rs manifest links the recompiled OoT aspMain ucode via the
        // harness-local build adapter (same crate oot-boot's rs lane uses),
        // so M_AUDTASK dispatch really synthesizes PCM for the cpal stream.
        oot_audio_ucode::set_rdram_len(rdram_len);
        unsafe { fn64_abi::set_audio_ucode_fn(oot_audio_ucode::oot_audio_ucode) };
        println!(
            "[fn64-shell] registered recompiled OoT aspMain audio ucode as the real \
             M_AUDTASK ucode function"
        );
    }

    #[cfg(not(all(fn64_recomp_rs, feature = "oot-audio")))]
    fn wire_audio_ucode(_rdram_len: usize) {
        if cfg!(feature = "oot-audio-ucode") {
            println!(
                "[fn64-shell] audio-ucode feature ON, but no ucode crate is linked yet (the \
                 available one cross-pins fn64-audio -- see Cargo.toml). Output path is wired; \
                 no synth."
            );
        } else {
            println!(
                "[fn64-shell] no audio ucode linked -- cpal output path is wired but receives no \
                 samples (silent). The rs manifest (crates/fn64-shell/rs) links the real ucode."
            );
        }
    }
}
