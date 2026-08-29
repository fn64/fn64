//! fn64-shell (`fn64`): the interactive, windowed harness -- the thing that
//! makes a recompiled N64 game PLAYABLE. It does the SAME boot + executor
//! drive as the headless `examples/oot-boot` (load ROM, register sections
//! via the C bridge, boot thread 0 on `recomp_entrypoint`, drive
//! `run_one_step`/`advance_virtual_time`), but instead of dumping the VI
//! framebuffer to PNGs it:
//!
//! 1. **Presents** the game's live VI framebuffer (see `framebuffer.rs`) to a
//!    resizable `winit` window every VI swap. The uploaded VI sample extent is
//!    composed into the original N64 4:3 display aspect by default; it does
//!    not become a square-pixel aspect ratio. A live
//!    `osViBlack` request or VI pixel type zero presents opaque black without
//!    reading the framebuffer, matching the renderer scanout path.
//! 2. **Feeds live keyboard input** to the game: each frame the current
//!    `PadState` (see `input_map.rs`) is pushed through
//!    `fn64_abi::set_controller_state(0, ..)` so the game's next
//!    `osContGetReadData` sees it.
//! 3. **Wires audio output**: registers `fn64_audio::CpalBackend` via
//!    `fn64_abi::set_audio_backend`, so `osAiSetNextBuffer`'s finished PCM
//!    reaches a live cpal output stream. The linked game/harness separately
//!    registers the recompiled ucode that produces those samples.
//!
//! ## Shell hotkeys
//!
//! Handled before the keyboard reaches the game, so they stay reachable
//! whatever `input.toml` binds:
//!
//! | Key | Effect |
//! |---|---|
//! | F1 | Settings overlay (`overlay.rs`) |
//! | F2 | Screenshot the game frame to a PNG (`screenshot.rs`) |
//! | F3 | Stack + framerate HUD (`stack.rs`); `FN64_HUD=1` starts it open |
//! | F11 | Toggle borderless fullscreen |
//! | Esc | Close the overlay, or exit when it is closed |
//!
//! ## What this build is running on
//!
//! The recompiler lane and the renderer are chosen in two places a player
//! never sees, and both default to the opposite of the intended target stack.
//! `stack.rs` prints a greppable `[fn64-stack]` block naming both -- plus
//! whether a game is linked and which -- unconditionally at startup and again
//! at exit, and F3 puts the same identity on screen with a live framerate.
//! Paste that block into any symptom report; without it the report has no
//! cell.
//!
//! ## Game intake (same contract as oot-boot)
//!
//! The recompiled game is linked at BUILD time from `RECOMPILED_DIR`/
//! `RECOMP_H_DIR`/`ROM` (see `build.rs`). When those are unset the shell
//! still builds and runs -- it just prints the intake instructions and
//! exits (no window), because there's no game to boot. That keeps
//! `cargo build --workspace` green with zero game content.
//!
//! Run it (OoT):
//! ```text
//! RECOMPILED_DIR=.../OOTU/RecompiledFuncs \
//! ROM=.../oot-ntsc-1.0.z64 \
//! cargo run -p fn64-shell
//! ```

// These two modules are consumed by the `#[cfg(fn64_game_linked)]` `game`
// module (the live window loop) and by their own unit tests. In a
// content-free build (no game linked) the binary's `main` never touches them,
// so `dead_code` would fire on every item -- allow it here rather than
// littering per-item attributes; the tests + game module are the real users.
/// Content-free UI demo: the real presentation path driven by a synthetic
/// RDRAM field, so a checkout with no game content can still open the window.
#[cfg(not(fn64_game_linked))]
mod demo;
mod app_identity;
#[allow(dead_code)]
mod device_timing_trace;
#[allow(dead_code)]
mod frame_trip;
mod framebuffer;
#[allow(dead_code)]
mod gamepad;
#[allow(dead_code)]
mod input_map;
#[allow(dead_code)]
mod overlay;
#[allow(dead_code)]
mod presentation_trace;
/// Per-pump cost attribution, gated by `FN64_PUMP_CENSUS=1`. Answers what is
/// inside a slow pump that is not inside a fast one -- the decomposition the
/// heartbeat's distribution cannot supply.
#[allow(dead_code)]
mod pump_census;
#[allow(dead_code)]
mod screenshot;
/// What this build is running on: the recompiler lane, the renderer, and
/// whether a game is linked. Printed unconditionally at startup and exit.
#[allow(dead_code)]
mod stack;
#[allow(dead_code)]
mod timing;
mod video_config;
#[allow(dead_code)]
mod zoom_fill;

#[cfg(not(fn64_game_linked))]
fn main() {
    // Content-free build: no game symbols were linked (RECOMPILED_DIR was
    // unset at build time -- see build.rs). `--demo` drives the real window,
    // framebuffer conversion, and overlay from a synthetic RDRAM field so the
    // UI stack stays verifiable in a checkout with no game content; without
    // it, report the intake contract honestly rather than opening a window
    // with nothing to boot.
    // Unconditional, and FIRST: a content-free build is itself a stack fact
    // worth naming, and the demo path opens a window with no game behind it.
    println!("{}", stack::banner(None));
    if std::env::args().any(|a| a == "--demo") {
        demo::run();
        return;
    }
    eprintln!(
        "fn64-shell: built WITHOUT a linked game (RECOMPILED_DIR was unset at build time).\n\
         \n\
         For a content-free UI demo (synthetic framebuffer, no ROM required):\n\
         \n\
         \x20 cargo run -p fn64-shell -- --demo\n\
         \n\
         To get a live, playable window, rebuild with the game intake env vars set (same\n\
         contract as examples/oot-boot), e.g. for OoT:\n\
         \n\
         \x20 RECOMPILED_DIR=.../OOTU/RecompiledFuncs \\\n\
         \x20 ROM=.../oot-ntsc-1.0.z64 \\\n\
         \x20 cargo run -p fn64-shell\n\
         \n\
         (Audio tasks execute live IMEM through fn64's clean-room LLE interpreter.)"
    );
    std::process::exit(2);
}

#[cfg(fn64_game_linked)]
fn main() {
    // Before the ROM loads and before any backend is constructed: the two
    // facts that are already fixed (lane, linked game) are printed here so
    // even a run that dies during boot leaves its cell in the log.
    println!("{}", stack::banner(None));
    game::run();
}

/// The linked title's host-first rs-lane lookup table.
///
/// The rs lane resolves host functions **by address**, so this table is
/// game-profile data and lives beside its own harness, never in this
/// game-agnostic crate. `build.rs` locates it from the `RECOMP_RS_HOST_LOOKUP`
/// environment variable and copies it into `OUT_DIR/host_lookup.rs`, which the
/// `#[path]` below names. Pointing that variable at a different harness's
/// `host_lookup.rs` links a different title.
///
/// This replaced a hardcoded `#[path]` include of
/// `../../../examples/oot-boot/src/host_lookup.rs`, which (a) pinned the shell
/// to OoT and (b) named a file that no longer exists in this repo, so
/// `FN64_RECOMP=rs` could not compile here for any title at all.
///
/// The staging step is what makes this work: `#[path]` accepts only a plain
/// literal (no `env!`, no `concat!`), and the table file opens with `//!` inner
/// doc comments that are legal only at the top of a *file* module. So build.rs
/// strips that leading block and writes the rest to a fixed `OUT_DIR` path,
/// which `include!` can name through `env!`.
/// Crate-root alias for the linked emitted crate. The shared per-title
/// `host_lookup.rs` refers to `crate::recompiled` so it does not have to
/// hardcode a harness-specific dependency name (`wm2000-boot` calls it
/// `wm2000_recompiled`; this shell calls it `game-recompiled`).
#[cfg(fn64_cpu_runtime)]
pub(crate) use game_recompiled as recompiled;

#[cfg(fn64_cpu_runtime)]
mod host_lookup {
    include!(concat!(env!("OUT_DIR"), "/host_lookup.rs"));
}

/// Everything that requires the linked game symbols lives here, gated on the
/// `fn64_game_linked` cfg `build.rs` sets only when it compiled the
/// RecompiledFuncs. Keeping it in one `cfg`'d module means the content-free
/// build never references an unresolved `recomp_entrypoint`.
#[cfg(fn64_game_linked)]
mod game {
    use crate::framebuffer::{self, FB_BYTES, FB_HEIGHT, FB_WIDTH};
    use crate::gamepad::Gamepads;
    use crate::input_map::{InputConfig, PadState};
    use crate::overlay::{Capture, Overlay};
    use crate::timing::{
        subfield_device_deadline, vi_field_wall_duration, DrainDecision, RetraceDrain,
        RetraceOutcome, TimingWindow, VideoSyncLandmark, VideoSyncProbe,
    };
    use std::sync::Arc;

    #[cfg(fn64_cpu_runtime)]
    use crate::recompiled;

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

    /// Per-ROM save file path: `<data_dir>/fn64/saves/<rom-file-stem>.sav`.
    /// `dirs::data_dir()` is the same platform-data-dir crate `InputConfig`
    /// already uses for its config file (see input_map.rs); saves use
    /// `data_dir` rather than `config_dir` because a save is user data, not
    /// configuration. Falls back to `.fn64/saves` under the current
    /// directory if the platform has no data dir (e.g. an unusual/headless
    /// host) -- this function itself never fails, it only picks where
    /// `save_storage_for_rom` will try to open the file; that call site is
    /// what actually falls further back (to an in-memory store) if even
    /// that path can't be opened.
    fn save_path_for_rom(rom_path: &std::path::Path) -> std::path::PathBuf {
        let stem = rom_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "rom".to_string());
        let saves_dir = dirs::data_dir()
            .map(|dir| dir.join("fn64").join("saves"))
            .unwrap_or_else(|| std::path::PathBuf::from(".fn64").join("saves"));
        saves_dir.join(format!("{stem}.sav"))
    }

    /// Open the real, file-backed save store for `rom_path`, falling back to
    /// the ephemeral in-memory store (same one oot-boot always uses -- see
    /// its main.rs comment) on any I/O error, so a read-only filesystem or
    /// permission issue degrades gracefully instead of aborting boot.
    fn save_storage_for_rom(rom_path: &std::path::Path) -> Box<dyn fn64_runtime::SaveStorage> {
        let save_path = save_path_for_rom(rom_path);
        let open_result = save_path
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|()| {
                fn64_runtime::FileSaveStorage::open_for_device(
                    &save_path,
                    fn64_runtime::SaveType::SramBanked,
                )
            });
        match open_result {
            Ok(storage) => {
                println!("[fn64-shell] save file: {}", save_path.display());
                Box::new(storage)
            }
            Err(e) => {
                eprintln!(
                    "[fn64-shell] WARNING: could not open save file {} ({e}) -- falling back to \
                     an IN-MEMORY save store (progress will NOT persist across exit)",
                    save_path.display()
                );
                Box::new(fn64_runtime::InMemorySaveStorage::for_device(
                    fn64_runtime::SaveType::SramBanked,
                ))
            }
        }
    }

    /// Boot the game and hold everything the window loop touches every frame.
    struct Shell {
        rdram: Vec<u8>,
        pad: PadState,
        /// User bindings + deadzone, loaded from ~/.config/fn64/input.toml
        /// and edited live by the settings overlay.
        config: InputConfig,
        /// Display settings (overscan crop, zoom-fill), loaded from
        /// ~/.config/fn64/video.toml and edited live by the Video tab.
        video: crate::video_config::VideoConfig,
        gamepads: Gamepads,
        overlay: Overlay,
        last_swap_count: u64,
        /// Scratch RGBA8888 buffer the VI framebuffer unpacks into before
        /// blitting to the pixels surface -- reused per frame, reallocated if
        /// the framebuffer width changes.
        rgba: Vec<u8>,
        /// Exact successful-submit authority, invalidation generation, and
        /// experiment accounting for pump-driven redraw suppression.
        present_cache: framebuffer::PresentCache,
        /// Experimental exact-input observation/suppression disposition.
        /// Disabled remains the default until a repeated live A/B establishes
        /// both correctness and wall-time effect.
        present_cache_mode: framebuffer::PresentCacheMode,
        /// Current pixels-surface / scratch width in pixels. Starts at
        /// FB_WIDTH and is resized to the game's real VI_WIDTH once a mode is
        /// latched, so the full framebuffer line is presented (not cropped).
        fb_width: usize,
        /// Current pixels-surface / scratch height in lines. Starts at
        /// FB_HEIGHT and is resized to the game's own VI active output height
        /// once V_START is latched, so the window shows exactly the
        /// scanned-out rectangle and never rows the game did not render.
        fb_height: usize,
        /// `Arc<Window>` (not a bare `Window`) so the `SurfaceTexture`
        /// pixels holds can own a `'static` window handle, letting `pixels`
        /// be `Pixels<'static>` -- otherwise pixels 0.15 borrows the window
        /// and the self-referential lifetime can't live in one struct.
        window: Option<Arc<Window>>,
        pixels: Option<Pixels<'static>>,
        /// Cached blit which keeps original 4:3 display geometry separate
        /// from the VI field's sampling dimensions. Zoom-fill reuses the same
        /// presenter with a full-surface viewport.
        frame_presenter: Option<crate::zoom_fill::FramePresenter>,
        /// True once `present()` has unpacked a VI framebuffer into `rgba`.
        /// Distinct from `reported_first_frame` (a logging latch): a capture
        /// needs to know the buffer holds a real frame, because a freshly
        /// allocated `rgba` is all-zero and would encode as a plausible but
        /// fabricated black PNG.
        rgba_holds_a_frame: bool,
        /// Hands out the never-reused suffix that keeps two captures in the
        /// same millisecond from overwriting each other.
        screenshotter: crate::screenshot::Screenshotter,
        reported_first_frame: bool,
        last_heartbeat_swap: u64,
        /// Frame-hash tripwire (`FN64_FRAME_TRIP`), `None` when off.
        frame_trip: Option<crate::frame_trip::FrameTrip>,
        /// Settled tripwire verdict, acted on in `about_to_wait`.
        frame_trip_verdict: Option<crate::frame_trip::Verdict>,
        /// Exit code to propagate once the event loop has returned.
        frame_trip_exit_code: Option<i32>,
        /// `FN64_FRAME_DUMP=<dir>`: write each tripwire frame as a PNG.
        frame_dump_dir: Option<std::path::PathBuf>,
        last_presented_source_generation: Option<fn64_abi::PresentedSourceFieldGeneration>,
        last_presented_post_vi_generation: Option<fn64_abi::PresentedPostViFieldGeneration>,
        /// Immutable emulated-cycle/host-wall epoch, established only after
        /// guest code installs H_SYNC/V_SYNC. Individual VI deadlines are
        /// always mapped from this epoch rather than accumulated or rebased.
        emulated_wall_clock: Option<crate::timing::EmulatedWallClock>,
        last_pump_started: Option<std::time::Instant>,
        frame_intervals: TimingWindow,
        /// Pumps in the current heartbeat window whose interval exceeded one
        /// 60 Hz field. Counted rather than derived: the percentile summary
        /// cannot report *how many* frames breached, only where the ranks fell.
        pumps_over_budget: usize,
        pump_times: TimingWindow,
        present_times: TimingWindow,
        pump_steps_total: u64,
        pump_steps_max: u64,
        pump_step_samples: u64,
        last_audio_underrun_sample_slots: u64,
        last_audio_contention_sample_slots: u64,
        last_audio_late_callbacks: u64,
        reported_audio_sync_landmark: bool,
        audio_sync_landmark: Option<fn64_audio::AudioSyncLandmark>,
        video_sync_probe: Option<VideoSyncProbe>,
        video_sync_landmark: Option<VideoSyncLandmark>,
        reported_av_sync_pair: bool,
        av_sync_frame_dump_dir: Option<std::path::PathBuf>,
        /// Per-pump phase attribution. Inert unless `FN64_PUMP_CENSUS=1`:
        /// unarmed it is one `bool` load per pump and nothing else.
        pump_census: crate::pump_census::PumpCensus,
        /// The event-loop path that requested irreversible process teardown.
        exit_path: &'static str,
        /// Optional producer-neutral device-event trace, sealed before ABI teardown.
        device_timing_trace: crate::device_timing_trace::DeviceTimingTraceSink,
        /// Optional host-only audio/video correlation trace. Host timestamps
        /// remain separate from deterministic device evidence.
        presentation_trace: crate::presentation_trace::PresentationTraceSink,
        /// Reused destination for the ABI's bounded renderer observation drain.
        render_observation_scratch: Vec<fn64_abi::RenderBatchObservation>,
        guest_task_observation_scratch: Vec<fn64_abi::GuestTaskObservation>,
        /// Prevents the pre-exit callback and `run_app` return from sealing twice.
        process_exit_prepared: bool,
        /// The backend `boot()` actually registered, carried so the census
        /// can name it on its first line. A graphics figure without its
        /// renderer beside it is not a result, and reading it back from
        /// `FN64_RENDER` would report the REQUEST rather than the fallback
        /// that may have replaced it.
        active_renderer: &'static str,
        /// The F3 HUD's own rolling timing window. Separate from the
        /// heartbeat's `TimingWindow`s on purpose: `take_stats()` DRAINS
        /// those, so sharing would make the two instruments steal each
        /// other's samples.
        hud_timing: crate::stack::HudTiming,
    }

    /// How many executor steps to run per window pump before yielding back to
    /// winit, so input/close stay responsive even while the game grinds. One
    /// VI frame is well under this; the cap only bounds a pathological spin.
    const STEPS_PER_PUMP: u64 = 200_000;

    impl Shell {
        fn boot() -> Self {
            let device_timing_trace =
                crate::device_timing_trace::DeviceTimingTraceSink::from_env()
                    .unwrap_or_else(|error| panic!("fn64-shell device timing trace: {error}"));
            let presentation_trace = crate::presentation_trace::PresentationTraceSink::from_env()
                .unwrap_or_else(|error| panic!("fn64-shell presentation trace: {error}"));
            fn64_abi::set_render_batch_observation_enabled(presentation_trace.is_enabled());
            let rom_path = env_path("ROM");
            println!("[fn64-shell] loading ROM from {}", rom_path.display());
            let rom_bytes = std::fs::read(&rom_path).unwrap_or_else(|e| {
                panic!("fn64-shell: failed to read ROM {}: {e}", rom_path.display())
            });
            println!("[fn64-shell] ROM size: {} bytes", rom_bytes.len());
            let tv_type = fn64_boot_harness::TvType::Ntsc;
            let mut rdram = fn64_boot_harness::new_rdram(tv_type);
            fn64_boot_harness::seed_ipl3_image(&mut rdram, &rom_bytes);
            fn64_abi::load_rom(rom_bytes.clone());

            // The game's aligned __CartRomHandle BSS address: guest code
            // dereferences osCartRomInit's returned handle, so the shim must
            // return the game-linked object, not an opaque token (same
            // registration as oot-boot main.rs -- its absence aborts boot in
            // bootproc's osCartRomInit call). Default is OoT NTSC 1.0's;
            // other titles override via FN64_CART_HANDLE_VRAM (hex), e.g.
            // WM2000/NWXE's D_800839A0.
            let cart_handle_vram = std::env::var("FN64_CART_HANDLE_VRAM")
                .ok()
                .map(|raw| {
                    u32::from_str_radix(raw.trim_start_matches("0x"), 16).unwrap_or_else(|_| {
                        panic!("FN64_CART_HANDLE_VRAM must be a hex vram address, got {raw:?}")
                    })
                })
                .unwrap_or(0x8000_9EA0);
            fn64_abi::set_cart_rom_handle_vram(cart_handle_vram);

            // Save-backing store (banked SRAM), same device as oot-boot --
            // domain-2 PI DMAs need somewhere to land. Unlike oot-boot (which
            // deliberately stays in-memory -- that harness is a bring-up
            // tool, not a play session), the interactive shell is where a
            // player actually leaves the game running and quits, so this
            // wires the real `FileSaveStorage`, falling back to the same
            // in-memory store oot-boot uses if the save file can't be opened
            // (e.g. a read-only filesystem) rather than aborting boot.
            fn64_abi::set_save(save_storage_for_rom(&rom_path));

            #[cfg(not(fn64_cpu_runtime))]
            {
                let registration = fn64_boot_harness::register_linked_sections();
                println!(
                    "[fn64-shell] bridge reports {} sections",
                    registration.reported_count()
                );
                // Always-resident section count: OoT keeps 0/1/2
                // (makerom.ent/boot/code) resident; NWXE only 0/1 (its
                // section 2 is an overlay bank). FN64_RESIDENT_SECTIONS
                // overrides; overlays stay DMA-loaded on demand either way.
                let resident_count: usize = std::env::var("FN64_RESIDENT_SECTIONS")
                    .ok()
                    .map(|raw| {
                        raw.parse().unwrap_or_else(|_| {
                            panic!("FN64_RESIDENT_SECTIONS must be a count, got {raw:?}")
                        })
                    })
                    .unwrap_or(3);
                for section_key in 0..resident_count {
                    if let Some(idx) = registration.registry_index(section_key) {
                        fn64_abi::set_section_loaded(idx);
                    }
                }
                let resident_sections: Vec<_> = registration
                    .sections()
                    .iter()
                    .take(resident_count)
                    .map(|section| (section.rom_addr, section.ram_addr, section.size))
                    .collect();
                fn64_boot_harness::seed_resident_sections(
                    &mut rdram,
                    &rom_bytes,
                    &resident_sections,
                );
            }
            #[cfg(fn64_cpu_runtime)]
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
                fn64_boot_harness::seed_resident_sections(
                    &mut rdram,
                    &rom_bytes,
                    &recompiled::RECOMPILED_SECTION_GEOMETRY[..3],
                );
                println!(
                    "[fn64-shell] registered {} recompiled section geometries; marked 0/1/2 resident",
                    section_indices.len()
                );
            }

            // The typed IPL standard owns both VI and AI clocks. The nominal
            // NTSC interval bootstraps the first field; OSViMode H/V timing
            // replaces it when the mode latches.
            fn64_abi::configure_tv_type(tv_type);

            let rdram_ptr = rdram.as_mut_ptr();

            // Render backend selection (same contract as oot-boot):
            // FN64_RENDER=rt64 uses the RT64 static backend (the eyes-verified
            // faithful renderer; requires the `rt64` feature), which writes
            // its frame back into the rdram VI framebuffer we present. The
            // default remains the software ReferenceBackend (the CI oracle).
            use fn64_render::RenderBackend as _;
            let create_reference = || -> Box<dyn fn64_render::RenderBackend> {
                let mut backend = fn64_render_reference::ReferenceBackend::new()
                    .with_f3dex2()
                    .with_clear_color([0, 0, 0, 255]);
                if let Err(e) = backend.create(&fn64_render::RenderConfig::for_tv(
                    FB_WIDTH as u32,
                    FB_HEIGHT as u32,
                    tv_type,
                )) {
                    eprintln!("[fn64-shell] WARNING: render backend create() failed: {e}");
                }
                Box::new(backend)
            };
            let requested_renderer = std::env::var("FN64_RENDER")
                .unwrap_or_else(|_| "reference".to_string())
                .to_ascii_lowercase();
            // `wgpu` additionally yields the ABI-side half of `WgpuBackend`'s
            // role split. `set_raw_dpc_session`'s own doc names "a shell or
            // test harness, never `fn64-abi` itself" as the caller that must
            // register it: without it `try_dispatch_raw_dpc_via_session`
            // (`rsp_commit.rs`) early-returns `None` and the raw-DPC seam this
            // backend implements is never reached. Order against
            // `set_render_backend` is immaterial -- `set_render_backend_with_policy`
            // never touches `RAW_DPC_SESSION` -- so the doc's "first or in the
            // same setup step" is a pairing-provenance rule, honored here by
            // taking both halves from one `try_new`.
            let mut raw_dpc_session: Option<fn64_render::RawDpcAbiSession> = None;
            enum RenderBackendRegistration {
                Local(Box<dyn fn64_render::RenderBackend>),
                Threaded(Box<dyn fn64_render::RenderBackend + Send>),
            }
            let (render_backend, active_renderer): (RenderBackendRegistration, &'static str) =
                if requested_renderer == "wgpu" {
                    match fn64_render_wgpu::WgpuBackend::try_new() {
                        Ok((mut backend, session)) => {
                            backend
                                .enable_presented_post_vi_field_delivery()
                                .unwrap_or_else(|error| {
                                    panic!("WgpuBackend post-VI delivery unavailable: {error}")
                                });
                            match backend.create(&fn64_render::RenderConfig::for_tv(
                                FB_WIDTH as u32,
                                FB_HEIGHT as u32,
                                tv_type,
                            )) {
                                Ok(()) => {
                                    raw_dpc_session = Some(session);
                                    (
                                        RenderBackendRegistration::Threaded(Box::new(backend)),
                                        "wgpu",
                                    )
                                }
                                Err(error) => {
                                    eprintln!(
                                    "[fn64-shell] WARNING: WgpuBackend create failed ({error}); \
                                     falling back to the ReferenceBackend oracle"
                                );
                                    (
                                        RenderBackendRegistration::Local(create_reference()),
                                        "reference-fallback",
                                    )
                                }
                            }
                        }
                        Err(error) => {
                            eprintln!(
                                "[fn64-shell] WARNING: WgpuBackend construction failed ({error}); \
                             falling back to the ReferenceBackend oracle"
                            );
                            (
                                RenderBackendRegistration::Local(create_reference()),
                                "reference-fallback",
                            )
                        }
                    }
                } else if requested_renderer == "rt64" {
                    let mut backend = fn64_render_rt64::Rt64Backend::new();
                    match backend.create(&fn64_render::RenderConfig::for_tv(
                        FB_WIDTH as u32,
                        FB_HEIGHT as u32,
                        tv_type,
                    )) {
                        Ok(()) => (RenderBackendRegistration::Local(Box::new(backend)), "rt64"),
                        Err(error) => {
                            eprintln!(
                                "[fn64-shell] WARNING: RT64 create failed ({error}); falling back \
                             to the ReferenceBackend oracle"
                            );
                            (
                                RenderBackendRegistration::Local(create_reference()),
                                "reference-fallback",
                            )
                        }
                    }
                } else {
                    if requested_renderer != "reference" {
                        eprintln!(
                            "[fn64-shell] WARNING: unknown FN64_RENDER={requested_renderer:?}; \
                         using ReferenceBackend"
                        );
                    }
                    (
                        RenderBackendRegistration::Local(create_reference()),
                        "reference",
                    )
                };
            match render_backend {
                RenderBackendRegistration::Local(backend) => {
                    fn64_abi::set_render_backend(backend, rdram.len())
                }
                RenderBackendRegistration::Threaded(backend) => {
                    fn64_abi::set_threaded_render_backend(backend, rdram.len())
                }
            }
            if let Some(session) = raw_dpc_session {
                fn64_abi::set_raw_dpc_session(session);
                println!(
                    "[fn64-shell] raw-DPC session registered (wgpu plan/execute/publish seam)"
                );
            }
            println!("[fn64-shell] render backend registered ({active_renderer}, 320x240)");
            // The banner again, now that the renderer is a RESOLVED fact
            // rather than a request -- this is the copy that answers "what am
            // I actually running", including a silent fallback.
            println!("{}", crate::stack::banner(Some(active_renderer)));

            // Audio OUTPUT path: a live cpal output stream. This is the
            // shell's audio deliverable -- samples produced by M_AUDTASK and
            // submitted through osAiSetNextBuffer flow here. Gated so a headless/CI env with no audio
            // device doesn't abort; a create() failure is logged, not fatal.
            // The negotiated stream rate is logged by wire_audio; it does not
            // drive VI pacing or guest-visible AI DMA state.
            wire_audio(rdram.len());

            configure_audio_tasks();

            println!("[fn64-shell] booting thread 0 (recomp_entrypoint)...");
            #[cfg(fn64_cpu_runtime)]
            {
                fn64_cpu_runtime::set_host_lookup(Some(
                    crate::host_lookup::recompiled_or_host_lookup,
                ));
                // Overlay banks share a VRAM window, so the emitted
                // dispatcher cannot resolve a collided vram from its table
                // alone. This is the seam it asks; the registry behind it is
                // already maintained from the guest's own PI DMA
                // (`fn64_abi::note_dma_overlay_load`).
                fn64_cpu_runtime::set_host_section_resident(Some(fn64_abi::is_section_loaded));
                println!(
                    "[fn64-shell] FN64_RECOMP=rs: linked game-recompiled crate + host-first \
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
                        // recompile_rom emits no `entrypoint` symbol (the
                        // previous reference here was stale and could not
                        // compile). Section 0 of the emitted geometry table is
                        // the entry section, so its ram_addr IS the configured
                        // entrypoint, title-neutrally. `lookup` traps by name
                        // if that vram carries no body.
                        recompiled::lookup(recompiled::RECOMPILED_SECTION_GEOMETRY[0].1),
                        0,
                        10,
                    );
                }
            }
            #[cfg(not(fn64_cpu_runtime))]
            unsafe {
                fn64_abi::boot_thread0(
                    rdram_ptr,
                    rdram.len(),
                    fn64_boot_harness::c_recomp_entrypoint(),
                    0,
                    10,
                );
            }

            let present_cache_env = std::env::var("FN64_PRESENT_CACHE").ok();
            let present_cache_mode =
                framebuffer::PresentCacheMode::from_env_value(present_cache_env.as_deref());
            println!(
                "[fn64-shell] FN64_PRESENT_CACHE mode={} samples_dependencies={} \
                 suppresses_redraws={}",
                present_cache_mode.name(),
                present_cache_mode.samples_dependencies(),
                present_cache_mode.suppresses_redraw(true),
            );

            Shell {
                rdram,
                pad: PadState::new(),
                config: InputConfig::load(),
                video: crate::video_config::VideoConfig::load(),
                gamepads: Gamepads::new(),
                overlay: {
                    let mut overlay = Overlay::new();
                    // FN64_HUD=1 brings the HUD up with the window, so a scripted
                    // or headless run does not have to synthesize an F3.
                    overlay.hud = crate::stack::hud_starts_open();
                    overlay
                },
                last_swap_count: 0,
                rgba: vec![0u8; FB_WIDTH * FB_HEIGHT * 4],
                present_cache: framebuffer::PresentCache::default(),
                present_cache_mode,
                fb_width: FB_WIDTH,
                fb_height: FB_HEIGHT,
                window: None,
                pixels: None,
                frame_presenter: None,
                rgba_holds_a_frame: false,
                screenshotter: crate::screenshot::Screenshotter::new(),
                reported_first_frame: false,
                last_heartbeat_swap: 0,
                frame_trip: crate::frame_trip::FrameTrip::from_env(),
                frame_trip_verdict: None,
                frame_trip_exit_code: None,
                frame_dump_dir: std::env::var_os("FN64_FRAME_DUMP").map(Into::into),
                last_presented_source_generation: None,
                last_presented_post_vi_generation: None,
                emulated_wall_clock: None,
                last_pump_started: None,
                frame_intervals: TimingWindow::default(),
                pumps_over_budget: 0,
                pump_times: TimingWindow::default(),
                present_times: TimingWindow::default(),
                pump_steps_total: 0,
                pump_steps_max: 0,
                pump_step_samples: 0,
                last_audio_underrun_sample_slots: 0,
                last_audio_contention_sample_slots: 0,
                last_audio_late_callbacks: 0,
                reported_audio_sync_landmark: false,
                audio_sync_landmark: None,
                video_sync_probe: VideoSyncProbe::from_env(),
                video_sync_landmark: None,
                reported_av_sync_pair: false,
                av_sync_frame_dump_dir: std::env::var_os("FN64_AV_SYNC_FRAME_DUMP")
                    .map(Into::into),
                pump_census: crate::pump_census::PumpCensus::new(),
                exit_path: "platform-loop-exiting",
                device_timing_trace,
                presentation_trace,
                render_observation_scratch: Vec::new(),
                guest_task_observation_scratch: Vec::new(),
                process_exit_prepared: false,
                active_renderer,
                hud_timing: crate::stack::HudTiming::default(),
            }
        }

        /// Advance one VI retrace and drive every runnable non-idle guest
        /// thread to quiescence. Reports whether that retrace swapped.
        fn pump_one_frame(&mut self) -> RetraceOutcome {
            let start_swaps = fn64_abi::vi_swap_count();
            let mut drain = RetraceDrain::new(start_swaps);

            // Advance the guest clock to the exact armed retrace FIRST,
            // unconditionally, because the caller only enters here when the
            // 16.67 ms wall deadline is due (`about_to_wait`'s FRAME). One
            // pump == one field == one retrace, whatever happens below.
            //
            // Doing it at the TOP is load-bearing: retrace must not depend on
            // whether the game happens to finish a frame during this pump.
            // Use the fabric's exact edge rather than `sim_time + interval`:
            // this pump may have serviced sub-field device deadlines since the
            // preceding edge, and adding a field to that later time would drift
            // the independent VI schedule.
            let tick = fn64_abi::next_vi_deadline()
                .expect("typed television standard must keep VI armed");
            fn64_abi::advance_virtual_time(tick);

            loop {
                let next_priority = fn64_abi::next_runnable_priority();
                if drain.before_step(next_priority) == DrainDecision::Quiescent {
                    // The guest can block after scheduling an event only one
                    // cycle away (raw FullSync -> DP is the live WM2000 case).
                    // Ending the pump here rounds that event up to the next VI
                    // edge and inserts an entire field of latency. Service
                    // every exact non-VI deadline first, then resume whatever
                    // it wakes; the following VI edge still belongs to the
                    // next wall-paced pump.
                    let current = fn64_abi::sim_time();
                    let next_vi = fn64_abi::next_vi_deadline()
                        .expect("typed television standard must keep VI armed");
                    if let Some(deadline) = subfield_device_deadline(
                        current,
                        fn64_abi::next_device_deadline(),
                        next_vi,
                    ) {
                        fn64_abi::advance_virtual_time(deadline);
                        continue;
                    }
                    // Exact closed loop: after retrace work blocks, OoT's
                    // priority-0 idle thread calls pause_self, which makes it
                    // runnable again. The old driver treated every such yield
                    // as progress and resumed it 200,000 times on each of the
                    // two legitimate no-swap retraces in OoT's 20 fps path.
                    // That consumed ~31 ms, halved VI delivery, and starved
                    // audio. One idle resume preserves its observable turn;
                    // seeing it again means no higher-priority guest work is
                    // runnable until the next external event/retrace.
                    break;
                }
                assert!(
                    drain.steps() < STEPS_PER_PUMP,
                    "fn64-shell: non-idle guest work exceeded {STEPS_PER_PUMP} scheduling steps in one retrace pump"
                );
                // Feed live input before the game polls the controller this
                // step. Cheap; keeps the pad current within the frame.
                let (buttons, sx, sy) = self.merged_input();
                fn64_abi::set_controller_state(0, buttons, sx, sy);

                let stepped = fn64_abi::run_one_step();
                drain.record_step(next_priority.expect("drain authorized a step without work"));
                // A VI swap is an observation, not a scheduling boundary.
                // Returning here used to leave AudioMgr's same-retrace work
                // runnable. The next pump advanced VI first, so a new retrace
                // queued behind the unfinished one; OoT deliberately coalesces
                // queued retraces, dropping one audio update per three after
                // its title path changes to one framebuffer swap per three
                // retraces. Drain this retrace to quiescence before time can
                // advance again; presentation still happens once below.
                if !stepped {
                    // Nothing is runnable and virtual time advances only at
                    // the top of the next pump, so retrying cannot make
                    // progress on this event-loop turn.
                    break;
                }
            }
            // NOTE: the clock advance happens at the TOP of this fn, not here.
            // The old rule derived virtual time from WORK -- `tick += 100` per
            // idle iteration and again every 100 scheduling steps -- so its
            // rate tracked how hard the game was working, not the clock. Idle
            // early boot landed near 60 Hz by luck; once gameplay got busy
            // `steps % 100` fired far more often and the clock accelerated:
            // 59.9 Hz at swap 60 -> 122.1 Hz by swap 480, still climbing
            // (ROADMAP R5 probe 3). The audio thread produces one buffer per
            // retrace, so that over-produced ~2:1, pegged the ring at its cap,
            // and drop-oldest skipped playback = the static; the same
            // over-delivery advanced game logic too fast = the over-speed.
            drain.finish(fn64_abi::vi_swap_count())
        }

        /// Keyboard + gamepad, merged into the game-facing pad state.
        /// Buttons OR together; the gamepad stick wins over keyboard while
        /// deflected (a real stick beats binary keys). Neutral while the
        /// settings overlay is open, so remapping never leaks into gameplay.
        fn merged_input(&self) -> (u16, i8, i8) {
            if self.overlay.open {
                return (0, 0, 0);
            }
            let (kb_buttons, kb_x, kb_y) = self.pad.resolve();
            let (gp_buttons, gp_x, gp_y) = self.gamepads.resolve(&self.config);
            let (sx, sy) = if gp_x != 0 || gp_y != 0 {
                (gp_x, gp_y)
            } else {
                (kb_x, kb_y)
            };
            (kb_buttons | gp_buttons, sx, sy)
        }

        fn invalidate_present_cache(&mut self) {
            if self.present_cache_mode.samples_dependencies() {
                self.present_cache.invalidate();
            }
        }

        fn probe_pump_present_dependency(
            &mut self,
        ) -> Option<framebuffer::PresentDependencyReceipt> {
            if !self.present_cache_mode.samples_dependencies() {
                return None;
            }
            let probe_started = std::time::Instant::now();
            let policy = framebuffer::PresentPolicy::new(
                self.video.overscan,
                self.video.zoom_fill,
            );
            self.present_cache.synchronize_policy(policy);
            let uncacheable = if self.overlay.active() {
                Some(framebuffer::UncacheablePresentReason::Overlay)
            } else if self.frame_trip.is_some() {
                Some(framebuffer::UncacheablePresentReason::FrameTrip)
            } else if self.frame_dump_dir.is_some() {
                Some(framebuffer::UncacheablePresentReason::FrameDump)
            } else {
                None
            };
            let receipt = if let Some(reason) = uncacheable {
                self.present_cache.record_uncacheable_request(
                    self.present_cache_mode,
                    policy,
                    reason,
                )
            } else if self.pixels.is_none() {
                self.present_cache.record_uncacheable_request(
                    self.present_cache_mode,
                    policy,
                    framebuffer::UncacheablePresentReason::UnavailableFramebuffer,
                )
            } else if let Some(fb_offset) = fn64_abi::scanout_vi_framebuffer()
                .or_else(fn64_abi::current_vi_framebuffer)
                .map(|offset| offset as usize)
            {
                if fb_offset >= self.rdram.len() {
                    self.present_cache.record_uncacheable_request(
                        self.present_cache_mode,
                        policy,
                        framebuffer::UncacheablePresentReason::OutsideRdram,
                    )
                } else if !fb_offset.is_multiple_of(4) {
                    self.present_cache.record_uncacheable_request(
                        self.present_cache_mode,
                        policy,
                        framebuffer::UncacheablePresentReason::UnalignedFramebuffer,
                    )
                } else {
                    let src_stride = fn64_abi::vi_width().map_or(FB_WIDTH, |w| w as usize);
                    let overscan =
                        (self.video.overscan as usize).min(src_stride.saturating_sub(1));
                    let target_width = (src_stride - overscan).clamp(1, 4096);
                    let target_height = fn64_abi::vi_output_height()
                        .map_or(FB_HEIGHT, |height| (height as usize).clamp(1, 4096));
                    self.present_cache.probe(
                        self.present_cache_mode,
                        policy,
                        &self.rdram,
                        fb_offset,
                        src_stride,
                        target_width,
                        target_height,
                        fn64_abi::vi_blanked(),
                    )
                }
            } else {
                self.present_cache.record_uncacheable_request(
                    self.present_cache_mode,
                    policy,
                    framebuffer::UncacheablePresentReason::MissingFramebuffer,
                )
            };
            Some(receipt.with_probe_ns(
                u64::try_from(probe_started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            ))
        }

        /// Blit the current VI framebuffer (rdram) into the pixels surface and
        /// present. Reports blank/uniform frames honestly.
        fn present(&mut self) {
            let present_started = std::time::Instant::now();
            let Some(pixels) = self.pixels.as_mut() else {
                return;
            };
            let post_vi_delivery = fn64_abi::take_presented_post_vi_field();
            let source_delivery = fn64_abi::take_presented_source_field();
            let reuse_owned_rgba = post_vi_delivery.is_none()
                && source_delivery.is_none()
                && self.rgba_holds_a_frame;
            if post_vi_delivery.is_none() && source_delivery.is_none() && !reuse_owned_rgba {
                return;
            }
            let mut presented_post_vi = None;
            let mut presented_source = None;
            let mut presentation_identity = None;
            if let Some(delivery) = post_vi_delivery {
                let stage = delivery.stage();
                let generation = delivery.generation();
                let retrace_at = delivery.retrace_at();
                if let Some(prior) = self.last_presented_post_vi_generation {
                    assert!(
                        generation > prior,
                        "presented post-VI generation {} did not advance past {}",
                        generation.get(),
                        prior.get(),
                    );
                }
                self.last_presented_post_vi_generation = Some(generation);
                if let fn64_abi::PresentedPostViFieldDelivery::Ready { field, .. } = delivery {
                    presentation_identity = Some((
                        stage,
                        generation.get(),
                        retrace_at,
                    ));
                    presented_post_vi = Some(field);
                }
            }
            if let Some(delivery) = source_delivery {
                let stage = delivery.stage();
                let generation = delivery.generation();
                let retrace_at = delivery.retrace_at();
                if let Some(prior) = self.last_presented_source_generation {
                    assert!(
                        generation > prior,
                        "presented source-field generation {} did not advance past {}",
                        generation.get(),
                        prior.get(),
                    );
                }
                self.last_presented_source_generation = Some(generation);
                if let fn64_abi::PresentedSourceFieldDelivery::Ready { field, .. } = delivery {
                    assert!(
                        presented_post_vi.is_none(),
                        "one retrace returned both source and post-VI host fields"
                    );
                    presentation_identity = Some((
                        stage,
                        generation.get(),
                        retrace_at,
                    ));
                    presented_source = Some(field);
                }
            }
            let blank;
            let dependency;
            if let Some(field) = presented_post_vi.as_ref() {
                let presentation = field.presentation();
                let src_width = field.width() as usize;
                let target_height = field.height() as usize;
                if src_width == 0 || target_height == 0 {
                    return;
                }
                let vi_blanked = presentation.blanked
                    || matches!(
                        presentation.scanout.filters().pixel_type,
                        fn64_render::ViPixelType::Blank
                    );
                let overscan = (self.video.overscan as usize).min(src_width.saturating_sub(1));
                let target_width = (src_width - overscan).clamp(1, 4096);
                if target_width != self.fb_width || target_height != self.fb_height {
                    if pixels
                        .resize_buffer(target_width as u32, target_height as u32)
                        .is_ok()
                    {
                        self.fb_width = target_width;
                        self.fb_height = target_height;
                        self.rgba = vec![0u8; target_width * target_height * 4];
                    }
                }
                framebuffer::copy_presented_post_vi_field(
                    field,
                    self.fb_width,
                    self.fb_height,
                    &mut self.rgba,
                );
                blank = vi_blanked || framebuffer::is_uniform(field.rgba8());
                dependency = None;
            } else if let Some(source) = presented_source.as_ref() {
                let presentation = source.presentation();
                let src_stride = source.stride_pixels() as usize;
                let target_height = source.height() as usize;
                let vi_blanked = presentation.blanked
                    || matches!(
                        presentation.scanout.filters().pixel_type,
                        fn64_render::ViPixelType::Blank
                    );
                let overscan = (self.video.overscan as usize).min(src_stride.saturating_sub(1));
                let target_width = (src_stride - overscan).clamp(1, 4096);
                if target_width != self.fb_width || target_height != self.fb_height {
                    if pixels
                        .resize_buffer(target_width as u32, target_height as u32)
                        .is_ok()
                    {
                        self.fb_width = target_width;
                        self.fb_height = target_height;
                        self.rgba = vec![0u8; target_width * target_height * 4];
                    }
                }
                framebuffer::copy_presented_source_field(
                    source,
                    self.fb_width,
                    self.fb_height,
                    &mut self.rgba,
                );
                blank = vi_blanked || framebuffer::is_uniform(source.rgba8());
                dependency = None;
            } else if reuse_owned_rgba {
                blank = framebuffer::is_uniform(&self.rgba);
                dependency = None;
            } else {
            // VI_ORIGIN, not the `osViSwapBuffer` bookkeeping pointer.
            // `scanout_vi_framebuffer`'s own doc names this distinction: the
            // two agree for a game that swaps through libultra, but only
            // VI_ORIGIN is defined for a game that programs the register
            // directly. Measured on WM2000 they differ by exactly one
            // 480-pixel row (`0x38f800` vs `0x38fbc0`, and `0x3c7c00` vs
            // `0x3c7fc0`; the two G_SETCIMG bases are 480*240*2 apart, so
            // they are the real buffer bases and VI_ORIGIN is one row into
            // each). Reading the bookkeeping pointer therefore shifts the
            // whole image up one line and pulls a row of never-rendered
            // memory in at the bottom. Fall back to the swap pointer only
            // when VI_ORIGIN has not been programmed at all.
            let fb_offset = match fn64_abi::scanout_vi_framebuffer()
                .or_else(fn64_abi::current_vi_framebuffer)
            {
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
            // The guest's own active output rectangle, from V_START. Rows
            // past it were never rendered into, so presenting a fixed 240
            // shows stale RDRAM along the bottom -- WM2000 programs 237.
            let target_height = fn64_abi::vi_output_height()
                .map_or(FB_HEIGHT, |h| (h as usize).clamp(1, 4096));
            let end = fb_offset + FB_BYTES;
            let region: &[u8] = if end <= self.rdram.len() {
                &self.rdram[fb_offset..end]
            } else if fb_offset < self.rdram.len() {
                &self.rdram[fb_offset..]
            } else {
                return;
            };

            let vi_blanked = fn64_abi::vi_blanked();
            blank = vi_blanked || framebuffer::is_uniform(region);
            // Real framebuffer line stride (VI_WIDTH); default to the presented
            // width before the first osViSetMode. Prevents non-320-wide modes
            // from presenting sheared/offset.
            let src_stride = fn64_abi::vi_width().map_or(FB_WIDTH, |w| w as usize);
            // Crop the overscan columns the player configured (Video tab /
            // FN64_OVERSCAN). This is a display POLICY, not a geometry-derived
            // width: WM2000's rightmost column IS genuinely scanned by the VI,
            // it just holds stale RDRAM the guest never fills, which a real TV
            // hides behind overscan. `overscan=0` presents the raw full
            // scanout; the default (1) drops exactly that uncovered column.
            // Guest RDRAM and the line stride are untouched -- only fewer
            // columns are read into the surface. Never crop below 1 column.
            let overscan = (self.video.overscan as usize).min(src_stride.saturating_sub(1));
            let visible_width = src_stride - overscan;
            // Size the surface + scratch to the presented width. Resize only on
            // change -- pixels' buffer resize reallocates GPU storage. wgpu
            // caps texture dimensions; clamp defensively.
            let target_width = visible_width.clamp(1, 4096);
            if target_width != self.fb_width || target_height != self.fb_height {
                if pixels
                    .resize_buffer(target_width as u32, target_height as u32)
                    .is_ok()
                {
                    self.fb_width = target_width;
                    self.fb_height = target_height;
                    self.rgba = vec![0u8; target_width * target_height * 4];
                    println!(
                        "[fn64-shell] resized present surface to {target_width}x{target_height} \
                         (stride {src_stride} minus {overscan} overscan col(s) x active output \
                         lines); window shows the scanned-out rectangle less cropped overscan."
                    );
                }
            }
            dependency = self.present_cache_mode.samples_dependencies().then(|| {
                framebuffer::PresentDependency::capture(
                    &self.rdram,
                    fb_offset,
                    src_stride,
                    self.fb_width,
                    self.fb_height,
                    vi_blanked,
                )
            });
            if vi_blanked {
                framebuffer::fill_opaque_black(&mut self.rgba);
            } else {
                framebuffer::rgba5551_to_rgba8888(
                    fn64_runtime::RdramView::from_storage(&self.rdram),
                    fn64_runtime::RdramAddr::from_offset(fb_offset as u32),
                    src_stride,
                    self.fb_width,
                    self.fb_height,
                    &mut self.rgba,
                );
            }
            }
            // `rgba` now holds a real frame, so F2 may encode it. Set here and
            // not after `render()`: the bytes are what a screenshot wants, and
            // a failed present does not make them fabricated.
            self.rgba_holds_a_frame = true;
            let mut rgba_hash = framebuffer::PresentedRgbaHash::new(&self.rgba);

            // Frame-hash tripwire. When armed it demands the exact identity
            // from the per-presentation authority; when off it adds only one
            // `Option` test and ordinary fields do not pay for its FNV pass.
            //
            // The verdict is RECORDED here and acted on in `about_to_wait`.
            // Exiting from `present` was tried and panics with "panic in a
            // function that cannot unwind": present runs inside winit's
            // extern "C" redraw callback, where `process::exit`'s teardown
            // cannot unwind. `about_to_wait` is ordinary Rust, and is where
            // the pump census already terminates bounded runs safely.
            // `FN64_FRAME_DUMP=<dir>` writes every presented frame as a PNG.
            // Diagnostic sibling of the tripwire: the tripwire says WHICH
            // frame changed, this says what it looks like. Same recording
            // point, so the two always agree about frame numbering.
            if let Some(dir) = self.frame_dump_dir.as_ref() {
                // Share Screenshotter's move-only session sequence with F2.
                // Frame dumps may be armed without a frame tripwire; deriving
                // this suffix from FrameTrip made every such capture `0000`
                // and silently overwrote repeated-content chronology.
                let file = self
                    .screenshotter
                    .next_frame_dump_file_name(rgba_hash.exact());
                if let Err(e) = crate::screenshot::capture(
                    dir,
                    &file,
                    self.fb_width,
                    self.fb_height,
                    &self.rgba,
                    self.rgba_holds_a_frame,
                ) {
                    eprintln!("[fn64-shell] frame dump failed: {e}");
                }
            }
            if let Some(trip) = self.frame_trip.as_mut() {
                if self.frame_trip_verdict.is_none() {
                    match trip.observe(rgba_hash.exact()) {
                        crate::frame_trip::Verdict::Pending => {}
                        settled => self.frame_trip_verdict = Some(settled),
                    }
                }
            }
            pixels.frame_mut().copy_from_slice(&self.rgba);
            // End the `as_mut` borrow: the overlay path re-borrows the
            // pixels/window fields immutably alongside `&mut self.config`.
            let overlay_open_before = self.overlay.open;
            let video_policy_before = framebuffer::PresentPolicy::new(
                self.video.overscan,
                self.video.zoom_fill,
            );
            let frame_presenter = self.frame_presenter.get_or_insert_with(|| {
                crate::zoom_fill::FramePresenter::new(
                    self.pixels.as_ref().expect("checked above"),
                )
            });
            let render_result = if self.overlay.active() {
                let window = self.window.as_ref().expect("window exists with pixels");
                let size = window.inner_size();
                // Built only when the HUD is actually up: with F3 off this
                // whole branch is one `bool` test, so the readout cannot
                // perturb the timings it exists to report.
                let hud = self.overlay.hud.then(|| {
                    let live = self
                        .hud_timing
                        .sample(std::time::Instant::now())
                        .map(|sample| sample.line());
                    crate::overlay::HudReadout {
                        identity: crate::stack::hud_identity(self.active_renderer),
                        live,
                        alarm: self.active_renderer == "reference-fallback",
                    }
                });
                self.overlay.render_over(
                    self.pixels.as_ref().expect("checked above"),
                    (size.width.max(1), size.height.max(1)),
                    window.scale_factor() as f32,
                    &mut self.config,
                    &mut self.video,
                    &self.gamepads,
                    hud.as_ref(),
                    frame_presenter,
                )
            } else {
                let window = self.window.as_ref().expect("window exists with pixels");
                let size = window.inner_size();
                frame_presenter.render(
                    self.pixels.as_ref().expect("checked above"),
                    (size.width.max(1), size.height.max(1)),
                    self.video.zoom_fill,
                )
            };
            if let Err(e) = render_result {
                if self.present_cache_mode.samples_dependencies() {
                    self.present_cache.record_failure();
                }
                eprintln!("[fn64-shell] pixels.render() failed: {e}");
                return;
            }
            let presented_at = std::time::Instant::now();
            fn64_abi::drain_render_batch_observations(&mut self.render_observation_scratch);
            self.presentation_trace
                .record_render_batches(self.render_observation_scratch.drain(..));
            fn64_abi::drain_guest_task_observations(&mut self.guest_task_observation_scratch);
            self.presentation_trace
                .record_guest_tasks(self.guest_task_observation_scratch.drain(..));
            self.presentation_trace
                .observe_audio(fn64_abi::audio_presentation_state(), presented_at);
            self.presentation_trace
                .observe_audio_stream_start(fn64_abi::audio_stream_start_landmark());
            if let Some((stage, presentation_generation, retrace_at)) = presentation_identity {
                if self.presentation_trace.is_enabled() {
                    self.presentation_trace.record_vi_present(
                        stage,
                        presentation_generation,
                        retrace_at,
                        fn64_abi::vi_swap_count(),
                        rgba_hash.exact(),
                        self.fb_width,
                        self.fb_height,
                        presented_at,
                    );
                }
            }
            if let (Some(probe), Some((stage, presentation_generation, retrace_at))) =
                (self.video_sync_probe.as_mut(), presentation_identity)
            {
                let landmark = probe.needs_hash().then(|| {
                    probe.observe_successful_present(
                        rgba_hash.exact(),
                        stage,
                        presentation_generation,
                        fn64_abi::vi_swap_count(),
                        retrace_at,
                        presented_at,
                    )
                });
                if let Some(Some(landmark)) = landmark {
                    eprintln!(
                        "[fn64-av-sync] video hash={:016x} occurrence={} stage={} \
                         presentation_generation={} swap={} \
                         retrace_cycle={} presented=success",
                        landmark.rgba_hash,
                        landmark.occurrence,
                        landmark.stage.serialized_name(),
                        landmark.presentation_generation,
                        landmark.swap_count,
                        landmark.retrace_at.get(),
                    );
                    self.presentation_trace.record_video_cue(landmark);
                    self.video_sync_landmark = Some(landmark);
                }
            }
            if let Some(dependency) = dependency {
                self.present_cache.record_success(dependency);
                // The overlay's Done button and Video controls mutate their
                // state during this submission. That submission still
                // contains the old composition, so install it first and then
                // invalidate the generation consumed by the next pump.
                if overlay_open_before != self.overlay.open {
                    self.present_cache.invalidate();
                }
                let video_policy_after = framebuffer::PresentPolicy::new(
                    self.video.overscan,
                    self.video.zoom_fill,
                );
                self.present_cache.synchronize_policy(video_policy_before);
                self.present_cache.synchronize_policy(video_policy_after);
            }
            let present_wall = present_started.elapsed();
            self.present_times.record(present_wall);
            self.pump_census
                .record_present(present_started, present_wall);

            if !self.reported_first_frame {
                let swaps = fn64_abi::vi_swap_count();
                if blank {
                    println!(
                        "[fn64-shell] presenting VI framebuffer (swap #{swaps}) -- currently \
                         BLANK/uniform (game hasn't rendered visible geometry yet; the projection \
                         path may still be landing). Window + present path are live."
                    );
                } else {
                    let rgba_hash = rgba_hash.exact();
                    println!(
                        "[fn64-shell] presenting VI framebuffer (swap #{swaps}) -- non-uniform, \
                         rgba_hash={rgba_hash:016x} (hash is a comparison key, not a correctness \
                         claim); vi_width={} (framebuffer line stride, presented at {FB_WIDTH}).",
                        fn64_abi::vi_width()
                            .map_or_else(|| format!("unset->{FB_WIDTH}"), |w| w.to_string())
                    );
                }
                self.reported_first_frame = true;
            } else {
                // Periodic heartbeat so the log honestly shows the game is
                // advancing frames (VI swaps climbing), not stuck on swap #1.
                let swaps = fn64_abi::vi_swap_count();
                if swaps >= self.last_heartbeat_swap + 60 {
                    let rgba_hash = rgba_hash.exact();
                    let state = if blank { "uniform" } else { "non-uniform" };
                    // Audio counters in the same line: shows at a glance
                    // whether the game is producing PCM (ai_buffers/nonzero)
                    // and whether it reaches the backend (backend_buffers).
                    let audio = fn64_abi::audio_output_stats();
                    // R5 probe 3 on the same line as ring_frames, deliberately:
                    // this pairing IS the experiment. The shell paces its pump
                    // from the live VI field interval below, so retrace_hz
                    // materially above that mode's rate means the guest's VI
                    // ticker outruns the pump -- which
                    // would explain BOTH symptoms at once (audio produces per
                    // retrace -> ring pegs at its cap -> static; game logic
                    // advances per retrace -> over-speed). At ~60 Hz with a
                    // pegged ring, probe 3 is REFUTED and the cause is
                    // downstream in the producer.
                    let cadence = match fn64_abi::retrace_cadence() {
                        Some((_, _, hz)) => format!("{hz:.1}"),
                        None => "n/a".to_string(),
                    };
                    let interval = self
                        .frame_intervals
                        .take_stats()
                        .expect("heartbeat must follow at least one pump interval");
                    let pump = self
                        .pump_times
                        .take_stats()
                        .expect("heartbeat must follow at least one pump");
                    let present = self
                        .present_times
                        .take_stats()
                        .expect("heartbeat must follow at least one present");
                    let window_hz = 1000.0 / interval.median_ms;
                    // Choppy and slow are different failures and the median
                    // cannot tell them apart. `over_budget` is the fraction
                    // of pumps whose OWN COST breached the deadline -- not the
                    // fraction of frame intervals, which was the original
                    // shape and was an artifact: the interval median sits
                    // exactly on FRAME, so it fired as a coin flip on
                    // microsecond scheduler jitter (measured 50.4% against a
                    // real pump-cost breach rate of 0.1%, and it read 50.4 vs
                    // 56.3 on two lanes whose pump costs differed 3x). p99/max
                    // are computed by `TimingStats` and were being discarded.
                    let over_budget = self.pumps_over_budget;
                    let over_budget_pct = if interval.samples > 0 {
                        100.0 * over_budget as f64 / interval.samples as f64
                    } else {
                        0.0
                    };
                    self.pumps_over_budget = 0;
                    let average_steps =
                        self.pump_steps_total as f64 / self.pump_step_samples.max(1) as f64;
                    let audio_health = fn64_abi::audio_stream_health();
                    let audio_rates = fn64_abi::audio_rates();
                    let (
                        audio_callbacks,
                        audio_underrun_sample_slots,
                        window_underrun_sample_slots,
                        audio_contention_sample_slots,
                        window_contention_sample_slots,
                        audio_dropped_sample_slots,
                        audio_late_callbacks,
                        window_late_callbacks,
                        max_callback_gap_us,
                    ) = audio_health
                        .map(|health| {
                            (
                                health.callbacks,
                                health.underrun_sample_slots.get(),
                                health
                                    .underrun_sample_slots
                                    .get()
                                    .saturating_sub(self.last_audio_underrun_sample_slots),
                                health.contention_sample_slots.get(),
                                health
                                    .contention_sample_slots
                                    .get()
                                    .saturating_sub(self.last_audio_contention_sample_slots),
                                health.dropped_sample_slots.get(),
                                health.late_callbacks,
                                health
                                    .late_callbacks
                                    .saturating_sub(self.last_audio_late_callbacks),
                                health.max_callback_gap_us,
                            )
                        })
                        .unwrap_or((0, 0, 0, 0, 0, 0, 0, 0, 0));
                    let audio_non_contention_silence_slots =
                        audio_underrun_sample_slots.saturating_sub(audio_contention_sample_slots);
                    let (ai_status_reads, ai_busy_returns) = fn64_abi::ai_status_stats();
                    let (ai_length_reads, ai_length_last) = fn64_abi::ai_length_stats();
                    let present_cache = self.present_cache.stats();
                    println!(
                        "[fn64-shell] present heartbeat: VI swap #{swaps} ({state}, \
                         rgba_hash={rgba_hash:016x}; visual correctness not inferred); \
                         retrace_hz={cadence} cumulative, window_hz={window_hz:.1}; \
                         timing_ms median/p95/p99/max: interval={:.2}/{:.2}/{:.2}/{:.2} \
                         pump={:.2}/{:.2}/{:.2}/{:.2} present={:.2}/{:.2}/{:.2}/{:.2} (n={}); \
                         over_budget={over_budget}/{} ({over_budget_pct:.1}% of pumps > 16.67ms); \
                         pump_steps avg/max={average_steps:.1}/{}; audio: \
                         ai_buffers={} samples={} nonzero={} backend_buffers={} ring_frames={:?} \
                         callbacks={audio_callbacks} underrun_sample_slots={audio_underrun_sample_slots} \
                         (+{window_underrun_sample_slots} window; non_contention={audio_non_contention_silence_slots} \
                         contention={audio_contention_sample_slots} +{window_contention_sample_slots} window) \
                         dropped_sample_slots={audio_dropped_sample_slots} \
                         late_callbacks={audio_late_callbacks} \
                         (+{window_late_callbacks} window) max_callback_gap_us={max_callback_gap_us} \
                         ai_status_reads/busy={ai_status_reads}/{ai_busy_returns} \
                         ai_length_reads/last={ai_length_reads}/{ai_length_last} \
                         guest/stream_hz={audio_rates:?}; present_cache: mode={} \
                         requests={} hits={} misses={} successful_presents={} failed_presents={} \
                         invalidations={} dependency_samples={} dependency_bytes={} \
                         logical_digest={:016x}",
                        interval.median_ms,
                        interval.p95_ms,
                        interval.p99_ms,
                        interval.max_ms,
                        pump.median_ms,
                        pump.p95_ms,
                        pump.p99_ms,
                        pump.max_ms,
                        present.median_ms,
                        present.p95_ms,
                        present.p99_ms,
                        present.max_ms,
                        interval.samples,
                        interval.samples,
                        self.pump_steps_max,
                        audio.ai_buffers,
                        audio.samples,
                        audio.nonzero_samples,
                        audio.backend_buffers,
                        fn64_abi::audio_frames_remaining(),
                        self.present_cache_mode.name(),
                        present_cache.requests,
                        present_cache.hits,
                        present_cache.misses,
                        present_cache.successful_presents,
                        present_cache.failed_presents,
                        present_cache.invalidations,
                        present_cache.dependency_samples,
                        present_cache.dependency_bytes,
                        present_cache.logical_digest,
                    );
                    self.last_heartbeat_swap = swaps;
                    self.pump_steps_total = 0;
                    self.pump_steps_max = 0;
                    self.pump_step_samples = 0;
                    self.last_audio_underrun_sample_slots = audio_underrun_sample_slots;
                    self.last_audio_contention_sample_slots = audio_contention_sample_slots;
                    self.last_audio_late_callbacks = audio_late_callbacks;
                }
            }
        }

        /// F2: write the frame currently in `rgba` to a PNG.
        ///
        /// Every outcome is reported -- the written path on success, the
        /// concrete reason on failure -- because a screenshot key that
        /// sometimes does nothing visible is the silent no-op `AGENTS.md`
        /// forbids. Nothing here can end the session: the fallible work is
        /// behind `screenshot::capture`'s `Result`, which is logged and
        /// dropped, so a full disk or a read-only directory costs the player
        /// a screenshot and not their progress.
        fn save_screenshot(&mut self) {
            let dir = crate::screenshot::resolve_dir(
                std::env::var(crate::screenshot::DIR_ENV).ok().as_deref(),
            );
            let file = crate::screenshot::file_name(
                crate::screenshot::now_unix_millis(),
                self.screenshotter.next_seq(),
            );
            match crate::screenshot::capture(
                &dir,
                &file,
                self.fb_width,
                self.fb_height,
                &self.rgba,
                self.rgba_holds_a_frame,
            ) {
                Ok(path) => {
                    // Absolute where we can get it: a player who launched from
                    // a shortcut has no idea what the working directory is.
                    let shown = std::fs::canonicalize(&path).unwrap_or(path);
                    println!(
                        "[fn64-shell] screenshot saved: {} ({}x{}, game frame only -- \
                         the settings overlay is not captured)",
                        shown.display(),
                        self.fb_width,
                        self.fb_height
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[fn64-shell] screenshot FAILED: {e} (target directory {}; override it \
                         with {}=<dir>)",
                        dir.display(),
                        crate::screenshot::DIR_ENV
                    );
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
                .with_title(crate::app_identity::WINDOW_TITLE)
                .with_window_icon(Some(crate::app_identity::window_icon()))
                .with_inner_size(size)
                .with_min_inner_size(LogicalSize::new(FB_WIDTH as f64, FB_HEIGHT as f64));
            let window = match event_loop.create_window(attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    eprintln!("[fn64-shell] failed to create window: {e}");
                    self.exit_path = "window-create-failed";
                    event_loop.exit();
                    return;
                }
            };
            crate::app_identity::install_platform_application_icon();
            let win_size = window.inner_size();
            // `Arc<Window>` is `'static + HasWindowHandle`, so the resulting
            // Pixels is `Pixels<'static>` and can be stored alongside the
            // window in `Shell` without a self-referential borrow.
            let surface = SurfaceTexture::new(win_size.width, win_size.height, Arc::clone(&window));
            match Pixels::new(FB_WIDTH as u32, FB_HEIGHT as u32, surface) {
                Ok(px) => {
                    self.overlay.prepare(&px);
                    self.pixels = Some(px);
                    self.window = Some(window);
                    println!(
                        "[fn64-shell] window opened ({}x{})",
                        win_size.width, win_size.height
                    );
                }
                Err(e) => {
                    eprintln!("[fn64-shell] failed to create pixels surface: {e}");
                    self.exit_path = "pixels-create-failed";
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
            // The overlay consumes mouse events (the game has no mouse
            // input); a no-op while closed.
            if let Some(window) = self.window.as_ref() {
                self.overlay
                    .on_window_event(&event, window.scale_factor() as f32);
            }
            match event {
                WindowEvent::CloseRequested => {
                    // Same rule as Esc below: while the census is armed the
                    // run ends at its pump budget and nowhere else. A
                    // benchmark window sitting under whatever else is on the
                    // desktop must not have its sample size decided by a
                    // stray click or a window manager.
                    if self.pump_census.armed() {
                        println!(
                            "[fn64-shell] close request IGNORED: FN64_PUMP_CENSUS is armed and \
                             this run ends at its pump budget"
                        );
                        return;
                    }
                    println!("[fn64-shell] window close requested -- exiting");
                    self.exit_path = "window-close";
                    event_loop.exit();
                }
                WindowEvent::Resized(new_size) => {
                    self.invalidate_present_cache();
                    if let Some(px) = self.pixels.as_mut() {
                        // The frame presenter derives the centered original
                        // 4:3 viewport independently of the VI sample extent.
                        if let Err(e) =
                            px.resize_surface(new_size.width.max(1), new_size.height.max(1))
                        {
                            eprintln!("[fn64-shell] resize_surface failed: {e}");
                        }
                    }
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    // Winit follows this with the platform's redraw/resize
                    // traffic, but the cached surface belongs to the old
                    // scale until a successful new submission proves it.
                    self.invalidate_present_cache();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if event.repeat {
                        return;
                    }
                    if let PhysicalKey::Code(code) = event.physical_key {
                        let pressed = event.state == ElementState::Pressed;
                        // While the settings panel is OPEN, F1/F2/F3 select
                        // the Input/Video/Audio tabs (the panel shows the
                        // affordance). They keep their global meanings only
                        // when the panel is closed, below. Esc / the Done
                        // button close the panel.
                        if self.overlay.open && pressed {
                            let tab = match code {
                                KeyCode::F1 => Some(crate::overlay::Tab::Input),
                                KeyCode::F2 => Some(crate::overlay::Tab::Video),
                                KeyCode::F3 => Some(crate::overlay::Tab::Audio),
                                _ => None,
                            };
                            if let Some(tab) = tab {
                                self.overlay.select_tab(tab);
                                return;
                            }
                        }
                        // Shell chords, never game input: F1 settings,
                        // F2 screenshot, F11 fullscreen. Checked ahead of
                        // `pad.apply` like the others, so a chord stays a
                        // chord even if a user's input.toml binds the same
                        // key to a controller button.
                        if code == KeyCode::F2 && pressed {
                            self.save_screenshot();
                            return;
                        }
                        if code == KeyCode::F3 && pressed {
                            self.overlay.toggle_hud();
                            self.invalidate_present_cache();
                            // Named in the log too: a player who hits F3 by
                            // accident should learn what it is, and a log
                            // reader gets the stack line either way.
                            println!(
                                "[fn64-shell] stack/fps HUD {} (F3)",
                                if self.overlay.hud { "on" } else { "off" }
                            );
                            return;
                        }
                        if code == KeyCode::F1 && pressed {
                            self.overlay.toggle();
                            self.invalidate_present_cache();
                            if self.overlay.open {
                                // Keys held at open would otherwise stay
                                // latched into the game across the overlay.
                                self.pad.clear();
                            }
                            return;
                        }
                        if code == KeyCode::F11 && pressed {
                            if let Some(window) = self.window.as_ref() {
                                let next = match window.fullscreen() {
                                    Some(_) => None,
                                    None => Some(winit::window::Fullscreen::Borderless(None)),
                                };
                                window.set_fullscreen(next);
                            }
                            self.invalidate_present_cache();
                            return;
                        }
                        if self.overlay.open {
                            if !pressed {
                                return;
                            }
                            if code == KeyCode::Escape {
                                // Armed capture? Cancel it. Otherwise close.
                                if self.overlay.capture.is_some() {
                                    self.overlay.capture = None;
                                } else {
                                    self.overlay.toggle();
                                    self.invalidate_present_cache();
                                }
                                return;
                            }
                            self.overlay.apply_key_capture(&mut self.config, code);
                            return;
                        }
                        if code == KeyCode::Escape && pressed {
                            // A bounded census run must end at its pump
                            // budget and nowhere else. These runs open a real
                            // window that takes focus, so a keystroke typed at
                            // a terminal lands in the game -- which truncated
                            // four runs of a measurement matrix into short
                            // logs that still parsed, still exited 0, and
                            // still printed a plausible report. Ignoring Esc
                            // while the census is armed makes the run's length
                            // a property of the experiment rather than of what
                            // was typed nearby.
                            if self.pump_census.armed() {
                                println!(
                                    "[fn64-shell] Esc IGNORED: FN64_PUMP_CENSUS is armed and \
                                     this run ends at its pump budget"
                                );
                                return;
                            }
                            println!("[fn64-shell] Esc pressed -- exiting");
                            self.exit_path = "escape-key";
                            event_loop.exit();
                            return;
                        }
                        if self.pad.apply(&self.config, code, pressed) {
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
                    // OS expose/redraw requests bypass the pump-side cache
                    // gate. Invalidate first so even an early/failed attempt
                    // makes the next pump retry; a successful submission
                    // installs fresh authority in the new generation.
                    self.invalidate_present_cache();
                    self.present();
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            // Frame-hash tripwire verdict, settled in `present`. Acted on
            // here because this is ordinary Rust: see the note at the
            // recording site for why exiting from `present` cannot work.
            if let Some(verdict) = self.frame_trip_verdict.take() {
                use crate::frame_trip::Verdict;
                let path = self
                    .frame_trip
                    .as_ref()
                    .map(|t| t.path().display().to_string())
                    .unwrap_or_default();
                let code = match verdict {
                    Verdict::Pending => unreachable!("Pending is never stored"),
                    Verdict::Recorded(n) => {
                        match self.frame_trip.as_ref().expect("verdict implies trip").write() {
                            Ok(()) => {
                                println!(
                                    "[fn64-shell] frame tripwire: recorded {n} frame hashes to {path}"
                                );
                                0
                            }
                            Err(e) => {
                                eprintln!("[fn64-shell] frame tripwire: FAILED to write {path}: {e}");
                                1
                            }
                        }
                    }
                    Verdict::Matched(n) => {
                        println!("[fn64-shell] frame tripwire: PASS -- {n} frames match {path}");
                        0
                    }
                    Verdict::Unusable(why) => {
                        // Fails the run. A gate that cannot compare must not
                        // report success: a comment-only baseline was
                        // measured reporting "PASS -- 1 frames match".
                        eprintln!(
                            "[fn64-shell] frame tripwire: UNUSABLE -- {why} ({path})"
                        );
                        1
                    }
                    Verdict::Mismatch { index, expected, actual } => {
                        eprintln!(
                            "[fn64-shell] frame tripwire: FAIL at frame {index} -- pinned \
                             {expected:016x}, got {actual:016x} (baseline {path}). A differing \
                             hash localises the frame; it does not itself say which picture \
                             is correct."
                        );
                        1
                    }
                };
                use std::io::Write as _;
                let _ = std::io::stdout().flush();
                let _ = std::io::stderr().flush();
                // `event_loop.exit()`, not `process::exit`: on macOS BOTH
                // `present` and `about_to_wait` run inside winit's extern "C"
                // callbacks, where process teardown aborts with "panic in a
                // function that cannot unwind" (observed, exit 134, after the
                // message had already printed). Winit's own exit returns
                // through the loop, and `run_app` then propagates this code.
                self.frame_trip_exit_code = Some(code);
                self.exit_path = "frame-trip-verdict";
                event_loop.exit();
                return;
            }
            // Gamepad events every tick (gilrs state only advances when its
            // queue is drained). Presses are consumed by an armed overlay
            // capture and discarded otherwise, so arming a capture never
            // binds a stale press from seconds ago.
            self.gamepads.poll();
            let pad_press = self.gamepads.take_pressed();
            if matches!(self.overlay.capture, Some(Capture::Pad(_))) {
                if let Some(button) = pad_press {
                    self.overlay.apply_pad_capture(&mut self.config, button);
                }
            }

            // Real-time pacing WITHOUT blocking the event thread: pump one
            // game field when its hardware-derived wall deadline is due, then
            // hand the loop a WaitUntil so input/close events keep flowing while
            // we wait. VI H_SYNC/V_SYNC and AI DACRATE both derive from the
            // IPL-selected television clock; using the fabric's live field
            // interval here preserves that shared hardware authority without
            // making the host audio callback a guest-visible pacing source.
            // Heartbeat DMA/ring/underrun counters expose both boundaries.
            let now_t = std::time::Instant::now();
            let Some(_) = fn64_abi::vi_field_interval() else {
                // Before the first VI mode there is no hardware-derived wall
                // deadline. Continue guest/device boot without inventing a
                // nominal field. If both owners quiesce in that state, the
                // graphical shell has no clock it can honestly pace from.
                if !fn64_abi::run_one_step() {
                    let current = fn64_abi::sim_time();
                    let next = fn64_abi::next_device_deadline().unwrap_or_else(|| {
                        panic!(
                            "guest and devices quiesced at cycle {current} before VI H_SYNC/V_SYNC were programmed"
                        )
                    });
                    assert!(
                        next >= current,
                        "pre-VI device deadline regressed from {current} to {next}"
                    );
                    fn64_abi::advance_virtual_time(next);
                }
                event_loop.set_control_flow(ControlFlow::Poll);
                return;
            };
            let current = fn64_abi::emulated_now();
            let next_vi = fn64_abi::next_vi_instant()
                .expect("programmed VI interval must own a pending edge");
            let wall_clock = *self
                .emulated_wall_clock
                .get_or_insert_with(|| crate::timing::EmulatedWallClock::new(current, now_t));
            let scheduled_deadline = wall_clock.deadline(next_vi);

            if now_t >= scheduled_deadline {
                // Held rather than recorded here: the HUD pairs each pump's
                // COST with the interval that preceded it, and the cost is
                // only known below. Keeping the pair together is what lets
                // the HUD report the two as separate quantities instead of
                // conflating them -- the exact conflation that produced the
                // 57.3%-over-budget artifact.
                let mut hud_interval = None;
                if let Some(previous) = self.last_pump_started.replace(now_t) {
                    let elapsed = now_t.duration_since(previous);
                    self.frame_intervals.record(elapsed);
                    hud_interval = Some(elapsed);
                }
                // Bracket the pump, not its internals: the phase counters
                // `pump_census` reads are running totals `run_one_step`
                // already maintains, so differencing them across the pump
                // adds no clock to the hot loop (perf-method rule 17 -- a
                // predicted instrumentation cost was once wrong by 56x, so
                // the instrument that adds no timer is the one to prefer).
                self.pump_census
                    .before_pump(now_t, scheduled_deadline);
                let outcome = self.pump_one_frame();
                let pump_wall = now_t.elapsed();
                let following_field = vi_field_wall_duration(
                    fn64_abi::vi_field_interval()
                        .expect("typed television standard must keep VI armed"),
                );
                // The dependency census belongs to this completed pump and
                // must include the bounded window's final pump. It is outside
                // `pump_wall`, so Observe/Suppress comparison does not charge
                // the diagnostic byte traversal to emulated work.
                let present_dependency = self.probe_pump_present_dependency();
                let suppress_pump_redraw = present_dependency
                    .is_some_and(|receipt| receipt.suppress_redraw);
                let following_vi = fn64_abi::next_vi_instant()
                    .expect("completed VI pump must schedule its following edge");
                let following_deadline = wall_clock.deadline(following_vi);
                // **Pump cost, not frame interval.** The interval median sits
                // exactly on FRAME, so counting interval breaches is a coin
                // flip on microsecond scheduler jitter -- measured at 50.4%
                // on a lane whose real pump-cost breach rate was 0.1%
                // (9 of 6000), and it could not tell two lanes apart whose
                // pump costs differed 3x. Pump cost is the work the shell
                // actually did, so a breach here is a real missed deadline.
                if pump_wall > following_field {
                    self.pumps_over_budget += 1;
                }
                self.pump_times.record(pump_wall);
                if let Some(interval) = hud_interval {
                    self.hud_timing.record(interval, pump_wall);
                }
                if self.pump_census.after_pump(
                    pump_wall,
                    outcome.steps,
                    outcome.swapped,
                    now_t,
                    scheduled_deadline,
                    following_deadline,
                    false,
                    present_dependency,
                ) {
                    // Bounded run: a windowed benchmark that needs a human to
                    // close the window cannot be repeated identically, and
                    // "any timing claim needs repeated runs" is the bar.
                    self.pump_census.report_once(self.active_renderer);
                    // The DPC copy census reports from an `atexit` hook that
                    // this bounded-run exit path does not reach, so it armed
                    // and printed nothing. Ask it directly.
                    fn64_abi::dpc_copy_census::report_now();
                    // Terminate from HERE rather than by unwinding to
                    // `run_app`'s return. The ordinary exit path was observed
                    // to print "exited cleanly" and then hang with the
                    // process alive and its CPU time frozen -- a benchmark
                    // driver that waits on such a process waits forever, and
                    // killing it mid-matrix is how a run gets truncated into
                    // a plausible short log. Everything this census measures
                    // is already flushed above; the guest coroutines a normal
                    // teardown exists to seal are not observed after it.
                    //
                    // `prepare_clean_exit` FIRST, though. `process::exit`
                    // still runs thread-local destructors, and dropping the
                    // `Executor` force-unwinds the guest coroutines through
                    // `extern "C"` recomp frames -- which aborts with "panic
                    // in a function that cannot unwind". Observed: the census
                    // printed its whole report and then exited 134, turning a
                    // good measurement into a failed run. `prepare_clean_exit`
                    // detaches the coroutines so that drop has nothing to
                    // unwind.
                    prepare_clean_exit(self, "pump-census-window-complete");
                    use std::io::Write as _;
                    let _ = std::io::stdout().flush();
                    let _ = std::io::stderr().flush();
                    std::process::exit(0);
                }
                self.pump_steps_total = self.pump_steps_total.saturating_add(outcome.steps);
                self.pump_steps_max = self.pump_steps_max.max(outcome.steps);
                self.pump_step_samples += 1;
                if outcome.swapped {
                    self.last_swap_count = fn64_abi::vi_swap_count();
                }
                if !self.reported_audio_sync_landmark {
                    if let Some(landmark) = fn64_abi::audio_sync_landmark() {
                        if landmark.predicted_playback_at.is_some()
                            || landmark.dropped_before_playback
                        {
                            let playback_delta_ms = landmark.predicted_playback_at.map(|at| {
                                let now = std::time::Instant::now();
                                if at >= now {
                                    at.duration_since(now).as_secs_f64() * 1_000.0
                                } else {
                                    -now.duration_since(at).as_secs_f64() * 1_000.0
                                }
                            });
                            let landmark_cycle = match (
                                landmark.dma_started_at,
                                landmark.start_dacrate,
                                landmark.retimed_after_start,
                            ) {
                                (Some(start), Some(dacrate), false) => Some(
                                    start.get() as f64
                                        + landmark.guest_frame_offset as f64
                                            * fn64_runtime::CPU_CLOCK_HZ as f64
                                            * f64::from(dacrate + 1)
                                            / f64::from(fn64_abi::vi_clock_hz()),
                                ),
                                _ => None,
                            };
                            eprintln!(
                                "[fn64-av-sync] landmark dma={} guest_frame={} \
                                 dma_start={:?} landmark_cycle={landmark_cycle:?} \
                                 playback_from_report_ms={playback_delta_ms:?} dropped={} \
                                 retimed={} (negative playback delta means the predicted DAC \
                                 instant already passed)",
                                landmark.dma_id.get(),
                                landmark.guest_frame_offset,
                                landmark.dma_started_at.map(fn64_runtime::Cycles::get),
                                landmark.dropped_before_playback,
                                landmark.retimed_after_start,
                            );
                            self.presentation_trace.record_audio_cue(
                                landmark,
                                fn64_abi::audio_presentation_state(),
                                std::time::Instant::now(),
                            );
                            if let Some(dir) = self.av_sync_frame_dump_dir.as_ref() {
                                if let Err(error) = std::fs::create_dir_all(dir) {
                                    eprintln!(
                                        "[fn64-av-sync] failed to create frame dump directory \
                                         {dir:?}: {error}"
                                    );
                                } else {
                                    let file = "audio-landmark-latest-cached-present.png";
                                    match crate::screenshot::capture(
                                        dir,
                                        file,
                                        self.fb_width,
                                        self.fb_height,
                                        &self.rgba,
                                        self.rgba_holds_a_frame,
                                    ) {
                                        Ok(path) => eprintln!(
                                            "[fn64-av-sync] captured latest cached presentation at \
                                             {path:?}"
                                        ),
                                        Err(error) => eprintln!(
                                            "[fn64-av-sync] cached-frame capture failed: {error}"
                                        ),
                                    }
                                }
                            }
                            self.reported_audio_sync_landmark = true;
                            self.audio_sync_landmark = Some(landmark);
                        }
                    }
                }
                if !self.reported_av_sync_pair {
                    if let (Some(audio), Some(video)) =
                        (self.audio_sync_landmark, self.video_sync_landmark)
                    {
                        let audio_cycle = match (
                            audio.dma_started_at,
                            audio.start_dacrate,
                            audio.retimed_after_start,
                        ) {
                            (Some(start), Some(dacrate), false) => Some(
                                start.get() as f64
                                    + audio.guest_frame_offset as f64
                                        * fn64_runtime::CPU_CLOCK_HZ as f64
                                        * f64::from(dacrate + 1)
                                        / f64::from(fn64_abi::vi_clock_hz()),
                            ),
                            _ => None,
                        };
                        let host_phase_ms = audio.predicted_playback_at.map(|audio_wall| {
                            if video.presented_at >= audio_wall {
                                video.presented_at.duration_since(audio_wall).as_secs_f64()
                                    * 1_000.0
                            } else {
                                -audio_wall.duration_since(video.presented_at).as_secs_f64()
                                    * 1_000.0
                            }
                        });
                        let guest_phase_cycles =
                            audio_cycle.map(|cycle| video.retrace_at.get() as f64 - cycle);
                        eprintln!(
                            "[fn64-av-sync] pair video_minus_audio_host_ms={host_phase_ms:?} \
                             video_minus_audio_guest_cycles={guest_phase_cycles:?} \
                             audio_dropped={} audio_retimed={} \
                             (positive means the selected video cue follows the audio cue)",
                            audio.dropped_before_playback,
                            audio.retimed_after_start,
                        );
                        self.presentation_trace.record_av_cue_pair(
                            audio,
                            video,
                            fn64_abi::audio_presentation_state(),
                        );
                        self.reported_av_sync_pair = true;
                    }
                }
                if !suppress_pump_redraw {
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                wall_clock.deadline(
                    fn64_abi::next_vi_instant()
                        .expect("programmed VI must retain its next interrupt edge"),
                ),
            ));
        }

        fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
            // On macOS `applicationWillTerminate:` reaches this irreversible
            // callback while `run_app` is still on the stack. Waiting for
            // `run_app` to return lets Apple TLS teardown drop the executor
            // first, force-unwinding guest coroutines across extern-C frames.
            self.pump_census.report_once(self.active_renderer);
            fn64_abi::dpc_copy_census::report_now();
            prepare_clean_exit(self, self.exit_path);
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
            prepare_clean_exit(&mut shell, "input-probe");
            return;
        }

        // The only place the chords are announced to a player who never opens
        // a source file. The overlay's own hint line is shared with `--demo`
        // (which has no screenshot handler), so F2 is advertised here rather
        // than there -- a hint that lies in one of two modes is worse than no
        // hint.
        println!(
            "[fn64-shell] hotkeys: F1 settings · F2 screenshot (PNG into ./{}/, override with \
             {}=<dir>) · F3 stack/fps HUD (FN64_HUD=1 starts it open) · F11 fullscreen · \
             Esc exit",
            crate::screenshot::resolve_dir(None).display(),
            crate::screenshot::DIR_ENV
        );

        let event_loop = EventLoop::new().expect("fn64-shell: failed to build winit event loop");
        // Poll (not Wait): the game runs continuously, we're not idle-waiting
        // on OS events.
        event_loop.set_control_flow(ControlFlow::Poll);
        if let Err(e) = event_loop.run_app(&mut shell) {
            eprintln!("[fn64-shell] event loop error: {e}");
        }
        // Idempotent: the bounded-run path already printed if it fired, and a
        // report printed twice reads as two runs.
        shell.pump_census.report_once(shell.active_renderer);
        // Reprinted on exit: after twenty minutes of heartbeat lines the
        // startup banner is far off the top of the scrollback, and the log a
        // user actually copies is its tail.
        println!("{}", crate::stack::banner(Some(shell.active_renderer)));
        println!("[fn64-shell] exited cleanly.");
        // Tripwire runs are gates, so their verdict must reach the shell as
        // an exit status. Taken after `prepare_clean_exit` so the teardown
        // this path exists to perform still happens on a FAIL.
        let trip_code = shell.frame_trip_exit_code;
        prepare_clean_exit(&mut shell, "run-app-return");
        if let Some(code) = trip_code {
            std::process::exit(code);
        }
    }

    /// Seal guest coroutine ownership before normal process teardown.
    ///
    /// The one global `fn64_abi` executor holds a booted game's `GameThread`s,
    /// each wrapping a guest stack woven through linked generated code and
    /// non-unwind ABI frames. The terminal ABI operation detaches only the
    /// unfinished stacks that cannot be force-unwound, then ordinary Rust/TLS
    /// teardown remains available to window, audio, and renderer owners.
    fn prepare_clean_exit(shell: &mut Shell, path: &'static str) {
        if shell.process_exit_prepared {
            return;
        }
        use std::io::Write as _;
        let stats = shell.present_cache.stats();
        println!(
            "[fn64-present-cache] phase=final path={path} mode={} requests={} hits={} misses={} \
             successful_presents={} failed_presents={} invalidations={} \
             dependency_samples={} dependency_bytes={} logical_digest={:016x}",
            shell.present_cache_mode.name(),
            stats.requests,
            stats.hits,
            stats.misses,
            stats.successful_presents,
            stats.failed_presents,
            stats.invalidations,
            stats.dependency_samples,
            stats.dependency_bytes,
            stats.logical_digest,
        );
        if shell.device_timing_trace.is_enabled() {
            if let Some(receipt) = shell
                .device_timing_trace
                .write_once(&fn64_abi::copy_device_trace())
                .unwrap_or_else(|error| panic!("fn64-shell device timing trace: {error}"))
            {
                println!(
                    "[fn64-device-timing-trace] events={} bytes={} sha256={}",
                    receipt.events, receipt.bytes, receipt.sha256
                );
            }
        }
        let process_exit_guest_tasks = fn64_abi::take_process_exit_guest_task_observations();
        let incomplete_render_batch =
            fn64_abi::take_process_exit_render_batch_incomplete_observation();
        fn64_abi::drain_render_batch_observations(&mut shell.render_observation_scratch);
        shell
            .presentation_trace
            .record_render_batches(shell.render_observation_scratch.drain(..));
        fn64_abi::drain_guest_task_observations(&mut shell.guest_task_observation_scratch);
        shell
            .presentation_trace
            .record_guest_tasks(shell.guest_task_observation_scratch.drain(..));
        shell
            .presentation_trace
            .record_guest_tasks(process_exit_guest_tasks);
        if let Some(observation) = incomplete_render_batch {
            shell
                .presentation_trace
                .record_render_batch_incomplete(observation);
        }
        if let Some(receipt) = shell
            .presentation_trace
            .seal_once()
            .unwrap_or_else(|error| panic!("fn64-shell presentation trace: {error}"))
        {
            println!(
                "[fn64-presentation-trace] records={} bytes={} sha256={}",
                receipt.records, receipt.bytes, receipt.sha256
            );
        }
        let exit = fn64_abi::prepare_process_exit();
        let left_un_detached = exit.threads - exit.detached_coroutines;
        println!(
            "[fn64-exit-diagnostic] path={path} threads={} detached={} left_un_detached={left_un_detached}",
            exit.threads, exit.detached_coroutines,
        );
        shell.process_exit_prepared = true;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
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
        let changed = shell.pad.apply(&shell.config, code, true);
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
            let _ = shell.pump_one_frame();
        }
        println!(
            "[fn64-shell] INPUT PROBE OK: keyboard->controller reached fn64_abi and the game \
             stepped {} VI swaps with input held.",
            fn64_abi::vi_swap_count()
        );
    }

    /// Register the cpal output stream as the audio backend. A create()
    /// failure (no device, headless CI) is logged, not fatal -- the game
    /// still runs and presents; only sound is unavailable. The negotiated
    /// host rate is telemetry; VI remains paced by the wall-time retrace.
    fn wire_audio(rdram_len: usize) {
        if std::env::var_os("FN64_NO_AUDIO").is_some() {
            println!("[fn64-shell] FN64_NO_AUDIO set -- audio output disabled");
            return;
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
                let stream_rate = backend
                    .stream_rate_hz()
                    .unwrap_or_else(|| fn64_audio::HostSampleRateHz::new(N64_BOOT_AI_RATE_HZ));
                fn64_abi::set_audio_backend(Box::new(backend), rdram_len);
                println!(
                    "[fn64-shell] audio output wired (cpal, guest {N64_BOOT_AI_RATE_HZ} Hz -> \
                     stream {stream_rate} Hz stereo)"
                );
            }
            Err(e) => {
                eprintln!(
                    "[fn64-shell] audio output unavailable ({e}) -- continuing SILENT \
                     (window/input unaffected). Set FN64_NO_AUDIO to silence this."
                );
            }
        }
    }

    fn configure_audio_tasks() {
        fn64_abi::set_audio_task_lle_accuracy();
        println!("[fn64-shell] registered audio task policy: live-IMEM LLE accuracy");
    }
}
