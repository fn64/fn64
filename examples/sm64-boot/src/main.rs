//! Super Mario 64 (SM64U, NTSC US) headless boot harness on fn64 -- the first
//! render/execution of SM64 on the pure-Rust stack. See `build.rs`'s module
//! doc for the `RECOMPILED_DIR`/`RECOMP_H_DIR`/`ROM` env-var contract this
//! binary requires -- this crate itself contains zero game content, per
//! `fn64/README.md`. Structurally derived from `examples/wm2000-boot/src/
//! main.rs` and `examples/oot-boot/src/main.rs`; only the always-resident
//! section set (SM64's `main` segment), the entrypoint (VA 0x80246000, carried
//! inside the generated `recomp_entrypoint`), and the save type (EEPROM 4K)
//! differ. Deliberately reusing the SAME harness code proves fn64 generalizes
//! past the AKI titles and OoT.
//!
//! ## SM64's memory layout (from the generated recomp_overlays.inl)
//!
//! SM64's 181 recompiled sections split across three decomp segments:
//!
//!   - `main`    (sections 0..=153): RAM 0x80246000..0x8032D560, ROM 0x1000..
//!     -- the always-resident code segment. The N64 IPL/boot loads this
//!     contiguously at 0x80246000; section 0 holds `recomp_entrypoint`. Marked
//!     resident + seeded from ROM at boot, exactly as hardware has it before
//!     the first instruction runs.
//!   - `engine`  (sections 154..=161): RAM 0x80378800.., ROM 0xF5580.. -- NOT
//!     resident at boot; SM64's own `load_engine_code_segment()` PI-DMAs it in
//!     early during `thread3_main` (src/game/memory.c). The recomp runtime
//!     resolves those functions once the guest's own DMA lands them.
//!   - `goddard` (sections 162..=180): RAM 0x8016F000.., ROM 0x21F4C0.. -- the
//!     Mario-head intro segment, loaded on demand. Not resident at boot.
//!
//! This mirrors OoT/WM2000's "only the always-resident sections are marked
//! loaded at boot; on-demand segments arrive through the guest's own DMA."
//!
//! ## What this does
//!
//! 1. Loads the sm64-decomp build-output ROM (`ROM` env var) into
//!    `fn64_abi::load_rom`.
//! 2. Registers every section from the real, out-of-tree-compiled
//!    `recomp_overlays.inl` through `fn64-boot-harness`'s shared FFI bridge,
//!    then marks the `main` segment (sections 0..=153) loaded + seeds it.
//! 3. Boots thread 0 running the linked generated `recomp_entrypoint` (whose
//!    body starts at VA 0x80246000) and drives the executor.
//! 4. On every `osViSwapBuffer_recomp` (observed via `fn64_abi::vi_swap_count()`
//!    polling), inspects the guest framebuffer and dumps a PNG if non-uniform.
//! 5. Prints a summary ladder: steps, VI swaps, gfx/audio tasks, first trap.

use std::io::Write;

fn env_path(name: &str) -> std::path::PathBuf {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("sm64-boot: required environment variable {name} not set"))
        .into()
}

/// The index (in `recomp_overlays.inl`'s section_table) one past the last
/// always-resident `main`-segment section. SM64's `main` segment is sections
/// 0..=153 (RAM 0x80246000..0x8032D560); see this file's module doc.
const SM64_RESIDENT_SECTIONS: usize = 154;

/// End of the `main` segment's loaded image in RDRAM: `_mainSegmentNoloadStart`
/// from `build/us/sm64.us.map`. The N64 IPL loads the ENTIRE `.main` segment
/// (ROM 0x1000..0xf5580) contiguously to RAM 0x80246000..0x8033a580 -- code,
/// rodata AND initialized `.data` -- before the first instruction. Everything
/// above this (`.main.noload`, i.e. BSS) is zero-initialized, not ROM-backed.
///
/// The recomp section_table only tabulates *code* (functions); it does not
/// describe the segment's rodata/data. Seeding only the tabulated code sections
/// leaves initialized globals living in the gaps between/after code sections
/// (e.g. `gAudioHeapSize` @ 0x80334ffc, `gAudioInitPoolSize` @ 0x80335000) as
/// zero, which makes `soundAlloc(&gAudioInitPool, ...)` overflow-and-return-NULL
/// inside `audio_init` -> a null 16-bit store trap. Seeding the full contiguous
/// image (what hardware does) fixes that at its true source.
const SM64_MAIN_SEGMENT_NOLOAD_START: u32 = 0x8033_a580;

// The build.rs-generated registrar (from RECOMPILED_DIR, into OUT_DIR): walks
// SM64's `static_<sec>_<vram>` functions -- called directly by recompiled code
// but absent from recomp_overlays.inl's section_table -- and hands each to the
// callback below with its owning section's geometry. See build.rs.
extern "C" {
    fn fn64_sm64_register_statics(cb: StaticRegisterCb);
}

type StaticRegisterCb = unsafe extern "C" fn(
    section_index: u32,
    rom_addr: u32,
    ram_addr: u32,
    size: u32,
    offset: u32,
    func: fn64_abi::RecompFunc,
);

/// Count of corpus-static functions registered this run (diagnostic).
static STATICS_REGISTERED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Register one corpus-static function as its own single-function section at
/// its true guest VA (ram_addr + offset), so its native pointer lands in
/// fn64's dispatch registry (satisfying `fn64_c_recompiled_function_enter`)
/// and a `LOOKUP_FUNC` by VA resolves it too. Registering it at its exact VA
/// (rather than folding it into its parent section) keeps the destination's
/// link_vram correct and avoids mutating the already-registered parent.
unsafe extern "C" fn register_one_static(
    section_index: u32,
    rom_addr: u32,
    ram_addr: u32,
    _size: u32,
    offset: u32,
    func: fn64_abi::RecompFunc,
) {
    let _ = section_index;
    let va = ram_addr + offset;
    // A nominal 4-byte span at the function's own VA. The dispatch registry
    // keys on the native pointer; the span only needs to contain `va`.
    let idx = unsafe { fn64_abi::register_section(rom_addr + offset, va, 4, &[(0, 4, func)]) };
    fn64_abi::set_section_loaded(idx);
    STATICS_REGISTERED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn main() {
    let rom_path = env_path("ROM");
    println!("[sm64-boot] loading ROM from {}", rom_path.display());
    let rom_bytes = std::fs::read(&rom_path)
        .unwrap_or_else(|e| panic!("sm64-boot: failed to read ROM {}: {e}", rom_path.display()));
    println!("[sm64-boot] ROM size: {} bytes", rom_bytes.len());

    let tv_type = fn64_boot_harness::TvType::Ntsc;
    let mut rdram = fn64_boot_harness::new_rdram(tv_type);
    fn64_boot_harness::seed_ipl3_image(&mut rdram, &rom_bytes);
    fn64_abi::load_rom(rom_bytes.clone());

    let registration = fn64_boot_harness::register_linked_sections();
    println!(
        "[sm64-boot] bridge reports {} sections in recomp_overlays.inl",
        registration.reported_count()
    );

    // Mark SM64's always-resident `main` segment (sections 0..=153) loaded.
    // The `engine` (154..=161) and `goddard` (162..=180) segments are NOT
    // resident at boot -- the guest DMAs them in itself later (see module doc).
    let mut resident_marked = 0usize;
    for section_key in 0..SM64_RESIDENT_SECTIONS {
        if let Some(idx) = registration.registry_index(section_key) {
            fn64_abi::set_section_loaded(idx);
            resident_marked += 1;
        }
    }
    println!(
        "[sm64-boot] marked {resident_marked} always-resident `main`-segment sections \
         (0..{SM64_RESIDENT_SECTIONS}) loaded"
    );
    let resident_sections: Vec<_> = registration
        .sections()
        .iter()
        .take(SM64_RESIDENT_SECTIONS)
        .map(|section| (section.rom_addr, section.ram_addr, section.size))
        .collect();
    if let (Some(first), Some(last)) = (resident_sections.first(), resident_sections.last()) {
        println!(
            "[sm64-boot] resident range: ram {:#010x}..{:#010x} (rom {:#08x}..)",
            first.1,
            last.1 + last.2,
            first.0
        );
    }
    // Seed the ENTIRE `main` segment contiguously (ROM..RAM), exactly as the N64
    // IPL does -- not just the tabulated code sections. This carries the
    // segment's initialized `.rodata`/`.data` (which the recomp section_table
    // does not describe) into RDRAM, so link-time-constant data globals like
    // `gAudioHeapSize`/`gAudioInitPoolSize` read their real ROM values instead
    // of zero. See `SM64_MAIN_SEGMENT_NOLOAD_START`. The per-section seed below
    // would be redundant with this (contiguous is a superset), but is kept as a
    // defensive exactness check on the code sections' geometry.
    let (main_rom_start, main_ram_start) = resident_sections
        .first()
        .map(|first| (first.0, first.1))
        .expect("SM64 main segment has at least one resident section");
    let main_image_size = SM64_MAIN_SEGMENT_NOLOAD_START
        .checked_sub(main_ram_start)
        .expect("main-segment noload start must be above its load address");
    println!(
        "[sm64-boot] seeding full main segment: ram {main_ram_start:#010x}..{:#010x} \
         (rom {main_rom_start:#08x}.., {main_image_size:#x} bytes) -- code+rodata+data",
        main_ram_start + main_image_size
    );
    fn64_boot_harness::seed_resident_sections(
        &mut rdram,
        &rom_bytes,
        &[(main_rom_start, main_ram_start, main_image_size)],
    );
    fn64_boot_harness::seed_resident_sections(&mut rdram, &rom_bytes, &resident_sections);

    // Register SM64's corpus-static functions (called directly but omitted
    // from the section_table) -- without this, the first `static_*` the guest
    // calls trips fn64's "entered native callable not registered" trap. See
    // build.rs and register_one_static.
    unsafe { fn64_sm64_register_statics(register_one_static) };
    println!(
        "[sm64-boot] registered {} corpus-static functions (called-but-untabled)",
        STATICS_REGISTERED.load(std::sync::atomic::Ordering::Relaxed)
    );

    // Execute live task IMEM through fn64's clean-room RSP interpreter.
    fn64_abi::set_audio_task_lle_accuracy();

    // Cart OSPiHandle*: SM64 US only calls osCartRomInit() from the audio DMA
    // path (src/audio/load_sh.c) -- unlikely to be reached in early boot, but
    // if it is, the shim must return a guest-visible OSPiHandle address. Point
    // it at a valid, page-aligned guest KSEG0 BSS address inside SM64's own
    // BSS (past the resident code, before the heap) so a read-back doesn't
    // fall off the RDRAM image. (0x80340000 sits in SM64's BSS window, above
    // the `main` code end 0x8032D560.)
    fn64_abi::set_cart_rom_handle_vram(0x8034_0000);

    // Save-backing store: SM64 uses a 4-kbit (512-byte) EEPROM
    // (src/game/save_file.c: EEPROM_SIZE, osEepromLongRead/Write).
    fn64_abi::set_cartridge_save(
        fn64_abi::CartridgeSaveType::Eeprom4k,
        Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::Eeprom4k,
        )),
    );

    // Perf/output knobs.
    let trace_disabled = std::env::var_os("SM64_NO_TRACE").is_some();
    let trace_path: String = std::env::var("SM64_TRACE_PATH")
        .unwrap_or_else(|_| "/tmp/sm64-boot-trace.jsonl".to_string());
    let dumps_disabled = trace_disabled || std::env::var_os("SM64_NO_DUMP").is_some();

    // Reference software rasterizer, registered at the graphics seam so
    // submitted M_GFXTASKs actually rasterize. `FN64_RENDER=reference|rt64`
    // selects the backend; reference is the default for bring-up.
    use fn64_render::RenderBackend as _;
    let create_reference = || -> Box<dyn fn64_render::RenderBackend> {
        let mut backend = fn64_render_reference::ReferenceBackend::new()
            .with_f3dex2()
            .with_clear_color([0, 0, 0, 255]);
        if !dumps_disabled {
            backend = backend.with_auto_dump("/tmp", "fn64-sm64-render", 240);
        }
        backend
            .create(&fn64_render::RenderConfig::for_tv(320, 240, tv_type))
            .expect("ReferenceBackend create must be infallible for 320x240");
        Box::new(backend)
    };
    let requested_renderer =
        fn64_boot_harness::parse_release_env_value("FN64_RENDER", std::env::var_os("FN64_RENDER"))
            .unwrap_or_else(|error| panic!("sm64-boot: {error}"))
            .unwrap_or_else(|| "reference".to_string())
            .to_ascii_lowercase();
    let (render_backend, active_renderer): (Box<dyn fn64_render::RenderBackend>, &'static str) =
        match requested_renderer.as_str() {
            "reference" => (create_reference(), "reference"),
            "rt64" => {
                let mut backend = fn64_render_rt64::Rt64Backend::new();
                backend
                    .create(&fn64_render::RenderConfig::for_tv(320, 240, tv_type))
                    .unwrap_or_else(|error| {
                        panic!(
                            "sm64-boot: FN64_RENDER=rt64 requires a working native RT64 adapter; \
                             create failed: {error}"
                        )
                    });
                (Box::new(backend), "rt64")
            }
            value => panic!("sm64-boot: FN64_RENDER must be reference or rt64, got {value:?}"),
        };
    fn64_abi::set_render_backend_with_policy(
        render_backend,
        rdram.len(),
        fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy,
    );
    println!("[sm64-boot] registered {active_renderer} renderer (320x240)");

    // Typed IPL video standard: the shared VI/AI clock authority.
    fn64_abi::configure_tv_type(tv_type);

    if trace_disabled {
        println!("[sm64-boot] trace sink DISABLED (SM64_NO_TRACE)");
    } else if let Err(e) = fn64_abi::set_trace_sink_file(&trace_path) {
        eprintln!(
            "[sm64-boot] WARNING: failed to arm incremental trace sink at {trace_path}: {e} -- \
             a crash mid-boot will lose the trace."
        );
    } else {
        println!("[sm64-boot] incremental trace sink armed at {trace_path}");
    }

    // rdram: this process's one shared buffer, sized to also cover the
    // 0xA4xxxxxx hardware-register window (see fn64_runtime::mmio).
    let rdram_ptr = rdram.as_mut_ptr();

    // Prime MMIO backing bytes before the guest runs so a raw load before any
    // host-side register mutation sees real idle-hardware defaults.
    unsafe { fn64_abi::sync_mmio_into_rdram(rdram_ptr) };

    println!("[sm64-boot] booting thread 0 (recomp_entrypoint @ VA 0x80246000)...");
    unsafe {
        fn64_abi::boot_thread0(
            rdram_ptr,
            rdram.len(),
            fn64_boot_harness::c_recomp_entrypoint(),
            0,
            10,
        );
    }

    const LOG_EVERY: u64 = 50_000;
    let max_steps = std::env::var("SM64_MAX_STEPS").map_or(20_000_000, |raw| {
        let parsed = raw
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("SM64_MAX_STEPS must be a positive integer, got {raw:?}"));
        assert!(
            parsed > 0,
            "SM64_MAX_STEPS must be a positive integer, got {raw:?}"
        );
        parsed
    });
    // How many consecutive idle ticks before concluding steady state.
    const IDLE_TICKS_BEFORE_STOP: u32 = 200;

    // SM64_STOP_AT_SWAP=<n>: stop as soon as the VI swap counter reaches <n>.
    let stop_at_swap = std::env::var("SM64_STOP_AT_SWAP").ok().map(|raw| {
        raw.parse::<u64>()
            .unwrap_or_else(|_| panic!("SM64_STOP_AT_SWAP must be a positive integer, got {raw:?}"))
    });

    // SM64_PRESS_START=<swap>[+<hold>]: hold START on port 0 from swap <swap>
    // for <hold> swaps (default 10), then release. Threads the real input seam.
    struct InputWindow {
        from: u64,
        to: u64,
        buttons: u16,
    }
    let mut input_windows: Vec<InputWindow> = Vec::new();
    if let Ok(raw) = std::env::var("SM64_PRESS_START") {
        let (swap, hold) = match raw.split_once('+') {
            Some((s, h)) => (
                s.parse::<u64>()
                    .unwrap_or_else(|_| panic!("SM64_PRESS_START swap must be an integer")),
                h.parse::<u64>()
                    .unwrap_or_else(|_| panic!("SM64_PRESS_START hold must be an integer")),
            ),
            None => (
                raw.parse::<u64>()
                    .unwrap_or_else(|_| panic!("SM64_PRESS_START must be <swap>[+<hold>]")),
                10,
            ),
        };
        input_windows.push(InputWindow {
            from: swap,
            to: swap + hold,
            buttons: 0x1000, // BTN_START
        });
        println!(
            "[sm64-input] scripted: hold START swaps {swap}..{}",
            swap + hold
        );
    }
    let mut last_applied_buttons = 0u16;

    let mut last_swap_count = 0u64;
    let mut fb_dumps = Vec::new();
    let mut thread0_death_logged = false;
    let mut consecutive_idle_ticks = 0u32;

    let mut steps = 0u64;
    let mut drain = fn64_boot_harness::GuestDrain::default();
    loop {
        if steps >= max_steps {
            println!(
                "[sm64-boot] step budget ({max_steps}) exhausted at sim_time={} -- stopping (a \
                 thread may be spinning without truly blocking, or boot needs a larger budget)",
                fn64_abi::sim_time()
            );
            break;
        }

        // Re-sync MMIO model into rdram before every step: a resumed coroutine
        // may issue a raw guest MMIO load at any point.
        unsafe { fn64_abi::sync_mmio_into_rdram(rdram_ptr) };
        let next_priority = fn64_abi::next_runnable_priority();
        let advanced_field =
            drain.before_step(next_priority) == fn64_boot_harness::DrainDecision::AdvanceField;
        if advanced_field {
            let _ = drain.advance_to_next_device_event();
        } else {
            let stepped = fn64_abi::run_one_step();
            assert!(
                stepped,
                "guest drain authorized a scheduling step without runnable work"
            );
            drain.record_step(next_priority.expect("guest drain lost runnable priority"));
            steps += 1;
        }

        if steps.is_multiple_of(LOG_EVERY) {
            let (gfx, audio) = fn64_abi::task_counts();
            println!(
                "[sm64-boot] progress: steps={steps} sim_time={} vi_swaps={} gfx_tasks={gfx} \
                 audio_tasks={audio}",
                fn64_abi::sim_time(),
                fn64_abi::vi_swap_count(),
            );
        }

        // Thread 0 (recomp_entrypoint) returning is EXPECTED, not the end of
        // boot: its own initial call chain unwinds once other threads are
        // spawned (thread 1 idle, thread 3 main, thread 5 game loop). Only log
        // once; the run queue / idle-tick counter is the real end signal.
        if !thread0_death_logged && fn64_abi::is_thread_dead(0) {
            println!(
                "[sm64-boot] thread 0 (recomp_entrypoint) returned at step {steps} -- expected \
                 (initial call chain unwound); other threads keep running"
            );
            thread0_death_logged = true;
        }

        let swap_count = fn64_abi::vi_swap_count();
        if swap_count > last_swap_count {
            // Scripted input: compose the pad state for THIS swap and feed it
            // through the real seam only on change.
            if !input_windows.is_empty() {
                let mut buttons = 0u16;
                for w in &input_windows {
                    if swap_count >= w.from && swap_count < w.to {
                        buttons |= w.buttons;
                    }
                }
                if buttons != last_applied_buttons {
                    fn64_abi::set_controller_state(0, buttons, 0, 0);
                    println!("[sm64-input] swap #{swap_count}: pad0 -> buttons={buttons:#06x}");
                    let _ = std::io::stdout().flush();
                    last_applied_buttons = buttons;
                }
            }
            if !dumps_disabled && active_renderer == "reference" {
                if let Some(fb_offset) = fn64_abi::current_vi_framebuffer() {
                    capture_framebuffer(&rdram, fb_offset, swap_count, &mut fb_dumps);
                }
            }
            last_swap_count = swap_count;
        }

        if let Some(stop) = stop_at_swap.filter(|stop| swap_count >= *stop) {
            println!(
                "[sm64-boot] SM64_STOP_AT_SWAP={stop} satisfied (swap #{swap_count}, step {steps}, \
                 sim_time={}) -- stopping",
                fn64_abi::sim_time()
            );
            break;
        }

        if advanced_field {
            if fn64_abi::next_runnable_priority().is_none() {
                consecutive_idle_ticks += 1;
            } else {
                consecutive_idle_ticks = 0;
            }
            if consecutive_idle_ticks >= IDLE_TICKS_BEFORE_STOP {
                println!(
                    "[sm64-boot] reached a steady idle state ({IDLE_TICKS_BEFORE_STOP} consecutive \
                     ticks with nothing runnable) at sim_time={} steps={steps} -- stopping",
                    fn64_abi::sim_time()
                );
                break;
            }
        } else {
            consecutive_idle_ticks = 0;
        }
    }

    let (gfx_count, audio_count) = fn64_abi::task_counts();
    println!("[sm64-boot] === BOOT SUMMARY ===");
    println!("[sm64-boot] steps run: {steps}");
    println!("[sm64-boot] virtual ticks run: {}", fn64_abi::sim_time());
    println!("[sm64-boot] thread 0 dead: {}", fn64_abi::is_thread_dead(0));
    println!(
        "[sm64-boot] VI swaps observed: {}",
        fn64_abi::vi_swap_count()
    );
    println!("[sm64-boot] gfx tasks submitted: {gfx_count}");
    println!("[sm64-boot] audio tasks submitted: {audio_count}");
    println!("[sm64-boot] renderer: {active_renderer}");
    let last_render_error = fn64_abi::last_render_error();
    println!("[sm64-boot] last render error: {last_render_error:?}");
    println!(
        "[sm64-boot] frame images dumped: {} ({:?})",
        fb_dumps.len(),
        fb_dumps
    );

    if !trace_disabled {
        let trace = fn64_abi::copy_trace();
        println!("[sm64-boot] trace events recorded: {}", trace.len());
        write_trace_file(&trace, &trace_path);
        println!("[sm64-boot] trace written to {trace_path}");
    }

    let exit = fn64_abi::prepare_process_exit();
    println!(
        "[sm64-boot] process exit prepared: threads={} detached_coroutines={}",
        exit.threads, exit.detached_coroutines
    );
}

/// Hash the guest framebuffer region pointed at by the latest VI swap and dump
/// it as a PNG if it is non-uniform (has actual rendered content).
fn capture_framebuffer(rdram: &[u8], fb_offset: u32, swap_count: u64, fb_dumps: &mut Vec<String>) {
    const W: usize = 320;
    const H: usize = 240;
    const BYTES: usize = W * H * 2; // 16-bit RGBA5551
    let start = fb_offset as usize;
    let Some(fb) = rdram.get(start..start + BYTES) else {
        return;
    };
    // Skip uniform (blank) frames -- only dump frames with real content.
    let first = &fb[0..2];
    if fb.chunks_exact(2).all(|px| px == first) {
        return;
    }
    let digest = fnv1a64_hex(fb);
    let path = format!("/tmp/sm64-boot-fb-{swap_count:04}.png");
    if let Err(e) = write_rgba5551_png(&path, fb, W, H) {
        eprintln!("[sm64-boot] WARNING: failed to write {path}: {e}");
        return;
    }
    println!("[sm64-boot] swap #{swap_count}: dumped framebuffer {path} (sha256={digest})");
    fb_dumps.push(path);
}

/// Small dependency-free content fingerprint for framebuffer log lines (not a
/// cryptographic identity -- just enough to tell distinct frames apart).
fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Minimal RGBA5551 (N64 VI framebuffer) -> RGBA8 PNG writer, no deps.
fn write_rgba5551_png(path: &str, fb: &[u8], w: usize, h: usize) -> std::io::Result<()> {
    let mut rgba = Vec::with_capacity(w * h * 4);
    for px in fb.chunks_exact(2) {
        let v = u16::from_be_bytes([px[0], px[1]]);
        let r5 = ((v >> 11) & 0x1f) as u8;
        let g5 = ((v >> 6) & 0x1f) as u8;
        let b5 = ((v >> 1) & 0x1f) as u8;
        rgba.push((r5 << 3) | (r5 >> 2));
        rgba.push((g5 << 3) | (g5 >> 2));
        rgba.push((b5 << 3) | (b5 >> 2));
        rgba.push(255);
    }
    write_png_rgba8(path, &rgba, w, h)
}

/// Uncompressed-deflate PNG writer (stored blocks), no external deps.
fn write_png_rgba8(path: &str, rgba: &[u8], w: usize, h: usize) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])?;

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
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

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, no filter, no interlace
    chunk(&mut file, b"IHDR", &ihdr)?;

    // Raw scanlines with a leading filter byte (0 = none) per row.
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }

    // zlib stream: 2-byte header + stored deflate blocks + adler32.
    let mut z = vec![0x78, 0x01];
    let mut idx = 0usize;
    while idx < raw.len() {
        let block = (raw.len() - idx).min(0xFFFF);
        let last = if idx + block >= raw.len() { 1u8 } else { 0u8 };
        z.push(last);
        z.extend_from_slice(&(block as u16).to_le_bytes());
        z.extend_from_slice(&(!(block as u16)).to_le_bytes());
        z.extend_from_slice(&raw[idx..idx + block]);
        idx += block;
    }
    // adler32 of raw.
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    chunk(&mut file, b"IDAT", &z)?;
    chunk(&mut file, b"IEND", &[])?;
    Ok(())
}

fn write_trace_file(trace: &[fn64_runtime::TraceEvent], path: &str) {
    let mut file = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[sm64-boot] failed to create trace file {path}: {e}");
            return;
        }
    };
    for event in trace {
        let _ = writeln!(file, "{event:?}");
    }
}
