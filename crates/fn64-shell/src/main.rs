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
#[allow(dead_code)]
mod frame_trip;
mod framebuffer;
#[allow(dead_code)]
mod gamepad;
#[allow(dead_code)]
mod input_map;
#[allow(dead_code)]
mod overlay;
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
    use crate::timing::{DrainDecision, RetraceDrain, RetraceOutcome, TimingWindow};
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
        gamepads: Gamepads,
        overlay: Overlay,
        last_swap_count: u64,
        /// Scratch RGBA8888 buffer the VI framebuffer unpacks into before
        /// blitting to the pixels surface -- reused per frame, reallocated if
        /// the framebuffer width changes.
        rgba: Vec<u8>,
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
        /// Wall-clock deadline for the next pumped frame (~60 Hz pacing).
        next_frame_deadline: std::time::Instant,
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
        last_audio_late_callbacks: u64,
        /// Per-pump phase attribution. Inert unless `FN64_PUMP_CENSUS=1`:
        /// unarmed it is one `bool` load per pump and nothing else.
        pump_census: crate::pump_census::PumpCensus,
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
            let (render_backend, active_renderer): (
                Box<dyn fn64_render::RenderBackend>,
                &'static str,
            ) = if requested_renderer == "wgpu" {
                match fn64_render_wgpu::WgpuBackend::try_new() {
                    Ok((mut backend, session)) => {
                        match backend.create(&fn64_render::RenderConfig::for_tv(
                            FB_WIDTH as u32,
                            FB_HEIGHT as u32,
                            tv_type,
                        )) {
                            Ok(()) => {
                                raw_dpc_session = Some(session);
                                (Box::new(backend), "wgpu")
                            }
                            Err(error) => {
                                eprintln!(
                                    "[fn64-shell] WARNING: WgpuBackend create failed ({error}); \
                                     falling back to the ReferenceBackend oracle"
                                );
                                (create_reference(), "reference-fallback")
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "[fn64-shell] WARNING: WgpuBackend construction failed ({error}); \
                             falling back to the ReferenceBackend oracle"
                        );
                        (create_reference(), "reference-fallback")
                    }
                }
            } else if requested_renderer == "rt64" {
                let mut backend = fn64_render_rt64::Rt64Backend::new();
                match backend.create(&fn64_render::RenderConfig::for_tv(
                    FB_WIDTH as u32,
                    FB_HEIGHT as u32,
                    tv_type,
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

            Shell {
                rdram,
                pad: PadState::new(),
                config: InputConfig::load(),
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
                fb_width: FB_WIDTH,
                fb_height: FB_HEIGHT,
                window: None,
                pixels: None,
                rgba_holds_a_frame: false,
                screenshotter: crate::screenshot::Screenshotter::new(),
                reported_first_frame: false,
                last_heartbeat_swap: 0,
                frame_trip: crate::frame_trip::FrameTrip::from_env(),
                frame_trip_verdict: None,
                frame_trip_exit_code: None,
                next_frame_deadline: std::time::Instant::now(),
                last_pump_started: None,
                frame_intervals: TimingWindow::default(),
                pumps_over_budget: 0,
                pump_times: TimingWindow::default(),
                present_times: TimingWindow::default(),
                pump_steps_total: 0,
                pump_steps_max: 0,
                pump_step_samples: 0,
                last_audio_underrun_sample_slots: 0,
                last_audio_late_callbacks: 0,
                pump_census: crate::pump_census::PumpCensus::new(),
                active_renderer,
                hud_timing: crate::stack::HudTiming::default(),
            }
        }

        /// Advance one VI retrace and drive every runnable non-idle guest
        /// thread to quiescence. Reports whether that retrace swapped.
        fn pump_one_frame(&mut self) -> RetraceOutcome {
            let start_swaps = fn64_abi::vi_swap_count();
            let mut drain = RetraceDrain::new(start_swaps);

            // Advance the guest clock by exactly one live retrace interval FIRST,
            // unconditionally, because the caller only enters here when the
            // 16.67 ms wall deadline is due (`about_to_wait`'s FRAME). One
            // pump == one field == one retrace, whatever happens below. The
            // interval may change when a pending OSViMode latches.
            //
            // Doing it at the TOP is load-bearing: retrace must not depend on
            // whether the game happens to finish a frame during this pump.
            let interval = fn64_abi::vi_field_interval()
                .expect("typed television standard must keep VI armed");
            let tick = fn64_abi::sim_time() + interval;
            fn64_abi::advance_virtual_time(tick);

            loop {
                let next_priority = fn64_abi::next_runnable_priority();
                if drain.before_step(next_priority) == DrainDecision::Quiescent {
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

        /// Blit the current VI framebuffer (rdram) into the pixels surface and
        /// present. Reports blank/uniform frames honestly.
        fn present(&mut self) {
            let present_started = std::time::Instant::now();
            let Some(pixels) = self.pixels.as_mut() else {
                return;
            };
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

            let blank = framebuffer::is_uniform(region);
            // Real framebuffer line stride (VI_WIDTH); default to the presented
            // width before the first osViSetMode. Prevents non-320-wide modes
            // from presenting sheared/offset.
            let src_stride = fn64_abi::vi_width().map_or(FB_WIDTH, |w| w as usize);
            // Size the surface + scratch to the real framebuffer width so the
            // WHOLE line is presented (WM2000 is 480 wide), not cropped to 320.
            // Resize only on change -- pixels' buffer resize reallocates GPU
            // storage. wgpu caps texture dimensions; clamp defensively.
            let target_width = src_stride.clamp(1, 4096);
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
                         (game VI_WIDTH x VI active output lines); window shows exactly the \
                         scanned-out rectangle."
                    );
                }
            }
            framebuffer::rgba5551_to_rgba8888(
                fn64_runtime::RdramView::from_storage(&self.rdram),
                fn64_runtime::RdramAddr::from_offset(fb_offset as u32),
                src_stride,
                self.fb_width,
                self.fb_height,
                &mut self.rgba,
            );
            // `rgba` now holds a real frame, so F2 may encode it. Set here and
            // not after `render()`: the bytes are what a screenshot wants, and
            // a failed present does not make them fabricated.
            self.rgba_holds_a_frame = true;
            let rgba_hash = framebuffer::rgba_hash(&self.rgba);

            // Frame-hash tripwire. Placed on the hash that already exists so
            // the guard adds no hashing and no clock; off by default, in
            // which case this is one `Option` test per frame.
            //
            // The verdict is RECORDED here and acted on in `about_to_wait`.
            // Exiting from `present` was tried and panics with "panic in a
            // function that cannot unwind": present runs inside winit's
            // extern "C" redraw callback, where `process::exit`'s teardown
            // cannot unwind. `about_to_wait` is ordinary Rust, and is where
            // the pump census already terminates bounded runs safely.
            if let Some(trip) = self.frame_trip.as_mut() {
                if self.frame_trip_verdict.is_none() {
                    match trip.observe(rgba_hash) {
                        crate::frame_trip::Verdict::Pending => {}
                        settled => self.frame_trip_verdict = Some(settled),
                    }
                }
            }
            pixels.frame_mut().copy_from_slice(&self.rgba);
            // End the `as_mut` borrow: the overlay path re-borrows the
            // pixels/window fields immutably alongside `&mut self.config`.
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
                    &self.gamepads,
                    hud.as_ref(),
                )
            } else {
                self.pixels.as_ref().expect("checked above").render()
            };
            if let Err(e) = render_result {
                eprintln!("[fn64-shell] pixels.render() failed: {e}");
                return;
            }
            self.present_times.record(present_started.elapsed());

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
                    let state = if blank { "uniform" } else { "non-uniform" };
                    // Audio counters in the same line: shows at a glance
                    // whether the game is producing PCM (ai_buffers/nonzero)
                    // and whether it reaches the backend (backend_buffers).
                    let audio = fn64_abi::audio_output_stats();
                    // R5 probe 3 on the same line as ring_frames, deliberately:
                    // this pairing IS the experiment. The shell paces its pump
                    // at 60 Hz (FRAME below), so retrace_hz materially above 60
                    // means the guest's VI ticker outruns the pump -- which
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
                    let (
                        audio_callbacks,
                        audio_underrun_sample_slots,
                        window_underrun_sample_slots,
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
                                health.late_callbacks,
                                health
                                    .late_callbacks
                                    .saturating_sub(self.last_audio_late_callbacks),
                                health.max_callback_gap_us,
                            )
                        })
                        .unwrap_or((0, 0, 0, 0, 0, 0));
                    let (ai_status_reads, ai_busy_returns) = fn64_abi::ai_status_stats();
                    let (ai_length_reads, ai_length_last) = fn64_abi::ai_length_stats();
                    let audio_rates = fn64_abi::audio_rates();
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
                         (+{window_underrun_sample_slots} window) late_callbacks={audio_late_callbacks} \
                         (+{window_late_callbacks} window) max_callback_gap_us={max_callback_gap_us} \
                         ai_status_reads/busy={ai_status_reads}/{ai_busy_returns} \
                         ai_length_reads/last={ai_length_reads}/{ai_length_last} \
                         guest/stream_hz={audio_rates:?}",
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
                        fn64_abi::audio_frames_remaining()
                    );
                    self.last_heartbeat_swap = swaps;
                    self.pump_steps_total = 0;
                    self.pump_steps_max = 0;
                    self.pump_step_samples = 0;
                    self.last_audio_underrun_sample_slots = audio_underrun_sample_slots;
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
                        let pressed = event.state == ElementState::Pressed;
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
            // game frame when the ~16.67 ms wall deadline is due, then hand
            // the loop a WaitUntil so input/close events keep flowing while
            // we wait. Audio stays synchronized WITHOUT being the pacing
            // master here: `osAiGetLength` reports only the current emulated
            // AI DMA, while the independent host ring absorbs callback jitter.
            // Heartbeat DMA/ring/underrun counters expose both boundaries.
            const FRAME: std::time::Duration = std::time::Duration::from_nanos(16_666_667);

            let now_t = std::time::Instant::now();
            if now_t >= self.next_frame_deadline {
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
                self.pump_census.before_pump();
                let outcome = self.pump_one_frame();
                let pump_wall = now_t.elapsed();
                // **Pump cost, not frame interval.** The interval median sits
                // exactly on FRAME, so counting interval breaches is a coin
                // flip on microsecond scheduler jitter -- measured at 50.4%
                // on a lane whose real pump-cost breach rate was 0.1%
                // (9 of 6000), and it could not tell two lanes apart whose
                // pump costs differed 3x. Pump cost is the work the shell
                // actually did, so a breach here is a real missed deadline.
                if pump_wall > FRAME {
                    self.pumps_over_budget += 1;
                }
                self.pump_times.record(pump_wall);
                if let Some(interval) = hud_interval {
                    self.hud_timing.record(interval, pump_wall);
                }
                if self
                    .pump_census
                    .after_pump(pump_wall, outcome.steps, outcome.swapped)
                {
                    // Bounded run: a windowed benchmark that needs a human to
                    // close the window cannot be repeated identically, and
                    // "any timing claim needs repeated runs" is the bar.
                    self.pump_census.report_once(self.active_renderer);
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
                    prepare_clean_exit();
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
            prepare_clean_exit();
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
        prepare_clean_exit();
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
    fn prepare_clean_exit() {
        use std::io::Write as _;
        let exit = fn64_abi::prepare_process_exit();
        println!(
            "[fn64-shell] process exit prepared: threads={} detached_coroutines={}",
            exit.threads, exit.detached_coroutines
        );
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
