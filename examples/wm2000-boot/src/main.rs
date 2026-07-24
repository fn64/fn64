//! WM2000 (NWXE) headless boot harness on fn64. See `build.rs`'s module
//! doc for the `RECOMPILED_DIR`/`RECOMP_H_DIR`/`ROM` env-var contract this
//! binary requires -- this crate itself contains zero game content, per
//! `fn64/README.md`.
//!
//! ## What this does
//!
//! 1. Loads the user's own ROM file (`ROM` env var) into `fn64_abi::load_rom`.
//! 2. Registers every section from the real, out-of-tree-compiled
//!    `recomp_overlays.inl` through `fn64-boot-harness`'s shared FFI bridge,
//!    then marks the always-resident sections (0/1, per the generated table --
//!    `docs/DESIGN.md`/`fn64_runtime::overlay`'s doc: entry+main) loaded.
//! 3. Boots thread 0 running `recomp_entrypoint` (the real, linked
//!    generated symbol) and drives the executor: `run_one_step` while
//!    runnable, `advance_virtual_time` (which fires the armed VI retrace
//!    ticker) when idle, for a bounded number of virtual-time ticks.
//! 4. At each committed presentation after one or more
//!    `osViSwapBuffer_recomp` calls (observed via
//!    `fn64_abi::vi_swap_count()` polling), inspects the reference lane's
//!    guest framebuffer or captures RT64's actual post-VI target. RT64 output
//!    is SHA-256-bound before optional PNG output.
//! 5. Emits the trace log to a file and prints a summary ladder.

use sha2::{Digest, Sha256};
use std::io::Write;

fn env_path(name: &str) -> std::path::PathBuf {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("wm2000-boot: required environment variable {name} not set"))
        .into()
}

fn validate_release_microcode_pair() {
    let text_path = env_path(fn64_boot_harness::RELEASE_MICROCODE_TEXT_PATH_ENV);
    let data_path = env_path(fn64_boot_harness::RELEASE_MICROCODE_DATA_PATH_ENV);
    let text = std::fs::read(&text_path).unwrap_or_else(|error| {
        panic!("wm2000-boot: read runner-staged release microcode text: {error}")
    });
    let text: [u8; fn64_runtime::RSP_MEMORY_BANK_SIZE] =
        text.try_into().unwrap_or_else(|text: Vec<u8>| {
            panic!(
                "wm2000-boot: runner-staged release microcode text has {} bytes, expected {}",
                text.len(),
                fn64_runtime::RSP_MEMORY_BANK_SIZE
            )
        });
    let data = std::fs::read(&data_path).unwrap_or_else(|error| {
        panic!("wm2000-boot: read runner-staged release microcode data: {error}")
    });
    assert!(
        !data.is_empty() && u32::try_from(data.len()).is_ok(),
        "wm2000-boot: runner-staged release microcode data must contain 1..=u32::MAX bytes"
    );
    let _ = text;
}

fn linked_native_program_identity() -> fn64_boot_harness::NativeProgramArtifactIdentity {
    fn64_boot_harness::NativeProgramArtifactIdentity::from_hex(env!(
        "FN64_NATIVE_PROGRAM_ARTIFACT_SHA256"
    ))
    .expect("wm2000-boot build script emitted an invalid native program artifact identity")
}

fn require_diagnostic_voice_map_reproduction(release_mode: bool, chan_ptr: u32, steps: u64) {
    assert!(
        !release_mode,
        "wm2000-boot: intervention-free release run reached fresh sound-channel array \
         {chan_ptr:#010x} at step {steps}; the diagnostic voice-map virgin-memory reproduction \
         would be required here, so no release report may be written"
    );
}

fn main() {
    let rom_path = env_path("ROM");
    println!("[wm2000-boot] loading ROM from {}", rom_path.display());
    let rom_bytes = std::fs::read(&rom_path).unwrap_or_else(|e| {
        panic!(
            "wm2000-boot: failed to read ROM {}: {e}",
            rom_path.display()
        )
    });
    println!("[wm2000-boot] ROM size: {} bytes", rom_bytes.len());
    let release_environment = fn64_boot_harness::release_run_environment_from_process()
        .unwrap_or_else(|error| panic!("wm2000-boot: {error}"));
    let tv_type = fn64_boot_harness::TvType::Ntsc;
    let mut rdram = fn64_boot_harness::new_rdram(tv_type);
    fn64_boot_harness::seed_ipl3_image(&mut rdram, &rom_bytes);
    fn64_abi::load_rom(rom_bytes.clone());

    let registration = fn64_boot_harness::register_linked_sections();
    println!(
        "[wm2000-boot] bridge reports {} sections in recomp_overlays.inl",
        registration.reported_count()
    );
    for section in registration.sections() {
        println!(
            "[wm2000-boot] registered section {}: rom={:#010x} ram={:#010x} \
             size={:#x} funcs={}",
            section.source_index,
            section.rom_addr,
            section.ram_addr,
            section.size,
            section.function_count
        );
    }

    // Per fn64_runtime::overlay's module doc: sections 0 (entry) and 1
    // (main/resident) are always-loaded; the four overlay banks (2-5) are
    // NOT loaded at boot (they are PI-bank-switched in later, per
    // overlays.json) -- this milestone does not yet drive that swap, so
    // only the always-resident sections are marked loaded, matching real
    // boot-time hardware state (no overlay bank has been PI-mapped in yet
    // this early).
    for section_key in [0usize, 1usize] {
        if let Some(idx) = registration.registry_index(section_key) {
            fn64_abi::set_section_loaded(idx);
            println!("[wm2000-boot] marked section {section_key} (index {idx}) loaded");
        }
    }
    let resident_sections: Vec<_> = registration
        .sections()
        .iter()
        .take(2)
        .map(|section| (section.rom_addr, section.ram_addr, section.size))
        .collect();
    fn64_boot_harness::seed_resident_sections(&mut rdram, &rom_bytes, &resident_sections);

    // Execute live task IMEM through fn64's clean-room RSP interpreter.
    fn64_abi::set_audio_task_lle_accuracy();

    // NWXE's osCartRomInit (func_80022540) returns its OSPiHandle BSS at
    // D_800839A0 (`addiu $v0, $s0, %lo(D_800839A0)`, disasm/asm/1050.s
    // vram 0x80022578); the host shim hands guest code that same address.
    fn64_abi::set_cart_rom_handle_vram(0x8008_39A0);

    // Save-backing store: NWXE's boot streams its overlay data, then
    // initializes its SRAM save region with FromRdram PI writes (observed:
    // repeating `FromRdram cart=0x0 len=0x20` retry loop when no storage is
    // registered -- the game verifies the write and retries forever).
    fn64_abi::set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
        fn64_runtime::SaveType::SramBanked,
    )));

    // Perf-run knobs, resolved once up front (used by both the render-backend
    // dump gate below and the trace sink further down):
    //   WM2000_NO_TRACE=1  -- skip the per-step JSONL trace sink entirely
    //   WM2000_NO_DUMP=1   -- skip the per-swap PNG surface dumps
    //   WM2000_TRACE_PATH  -- override the trace sink path
    // Both dumps/traces are real per-frame overhead (a live profile showed the
    // PNG encode + trace formatting on the hot path); disabling them is the
    // headless throughput configuration.
    let trace_disabled = std::env::var_os("WM2000_NO_TRACE").is_some();
    let trace_path: String = std::env::var("WM2000_TRACE_PATH")
        .unwrap_or_else(|_| "/tmp/wm2000-boot-trace.jsonl".to_string());
    // Both the backend's per-swap surface dump AND the harness's per-swap
    // framebuffer PNG are gated off together on a perf run (NO_DUMP, or the
    // trace-disabling flag which also signals "I'm measuring, not capturing").
    let dumps_disabled = trace_disabled || std::env::var_os("WM2000_NO_DUMP").is_some();
    let release_mode = release_environment.is_some();
    if release_mode {
        validate_release_microcode_pair();
    }
    let mut release_gate = release_environment.map(|environment| {
        let journal_path = environment.journal_path();
        let mut gate = fn64_boot_harness::LiveReleaseGate::new(environment.guest_cycle);
        gate.arm_with_unsupported_journal(&journal_path, &environment.run_event_sha256)
            .unwrap_or_else(|error| panic!("wm2000-boot: arm live release gate: {error}"));
        println!(
            "[wm2000-boot] intervention-free live release gate armed at guest cycle {}; \
             report={}; unsupported_journal={}",
            environment.guest_cycle,
            environment.report_path.display(),
            journal_path.display()
        );
        (gate, environment.report_path, environment.rom_class)
    });
    // Native readback is normally enabled with dumps, but certification and
    // timing probes can retain exact post-VI bytes/digests without PNG I/O.
    let rt64_capture_requested =
        release_mode || !dumps_disabled || std::env::var_os("WM2000_RT64_CAPTURE").is_some();

    // Main's VI pump delivers real presentations; without a registered
    // backend the first retrace trips present_render_backend's loud trap.
    // `FN64_RENDER=reference|rt64` selects the backend at runtime. RT64 owns
    // its hidden native surface, so this harness remains non-window-driving;
    // on macOS `main` also satisfies the adapter's required main-thread
    // initialization contract without an application event loop.
    use fn64_render::RenderBackend as _;
    let create_reference = || -> Box<dyn fn64_render::RenderBackend> {
        // The backend's per-swap surface PNG dump (encode_rgba8/write_png) is
        // real per-frame cost; skip it on a throughput run (gated together
        // with the harness fb dump via `dumps_disabled`).
        let mut backend = fn64_render_reference::ReferenceBackend::new()
            .with_f3dex2()
            .with_clear_color([0, 0, 0, 255]);
        if !dumps_disabled {
            backend = backend.with_auto_dump("/tmp", "fn64-wm2000-render", 240);
        }
        backend
            .create(&fn64_render::RenderConfig::for_tv(320, 240, tv_type))
            .expect("ReferenceBackend create must be infallible for 320x240");
        Box::new(backend)
    };
    let requested_renderer =
        fn64_boot_harness::parse_release_env_value("FN64_RENDER", std::env::var_os("FN64_RENDER"))
            .unwrap_or_else(|error| panic!("wm2000-boot: {error}"))
            .unwrap_or_else(|| "reference".to_string())
            .to_ascii_lowercase();
    assert!(
        !release_mode || requested_renderer == "rt64",
        "wm2000-boot: live release evidence requires FN64_RENDER=rt64, got {requested_renderer:?}"
    );
    let graphics_policy_name = fn64_boot_harness::parse_release_env_value(
        "WM2000_GRAPHICS_POLICY",
        std::env::var_os("WM2000_GRAPHICS_POLICY"),
    )
    .unwrap_or_else(|error| panic!("wm2000-boot: {error}"))
    .unwrap_or_else(|| {
        if requested_renderer == "rt64" {
            "lle".to_string()
        } else {
            "hle".to_string()
        }
    })
    .to_ascii_lowercase();
    let graphics_policy = match graphics_policy_name.as_str() {
        "hle" | "hle-optimized" => fn64_abi::GraphicsTaskExecutionPolicy::HleOptimized,
        "lle" | "lle-accuracy" => fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy,
        value => panic!("wm2000-boot: WM2000_GRAPHICS_POLICY must be hle or lle, got {value:?}"),
    };
    assert!(
        !release_mode || graphics_policy == fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy,
        "wm2000-boot: live release evidence requires WM2000_GRAPHICS_POLICY=lle"
    );
    let (render_backend, active_renderer, capture_rt64_present): (
        Box<dyn fn64_render::RenderBackend>,
        &'static str,
        bool,
    ) = match requested_renderer.as_str() {
        "reference" => (create_reference(), "reference", false),
        "rt64" => {
            let mut backend = fn64_render_rt64::Rt64Backend::new();
            backend
                .create(&fn64_render::RenderConfig::for_tv(320, 240, tv_type))
                .unwrap_or_else(|error| {
                    panic!(
                        "wm2000-boot: FN64_RENDER=rt64 requires a working native RT64 adapter; \
                         create failed: {error}"
                    )
                });
            let capture = rt64_capture_requested;
            if capture {
                backend.enable_present_capture().unwrap_or_else(|error| {
                    panic!("wm2000-boot: RT64 post-VI capture setup failed: {error}")
                });
            }
            if release_mode {
                #[cfg(feature = "rt64")]
                let source_identity = fn64_render_rt64::Rt64Backend::release_identity();
                #[cfg(feature = "rt64")]
                assert!(
                    source_identity.is_source_authoritative(),
                    "wm2000-boot: RT64 release evidence requires a clean Git source identity, got {}",
                    source_identity.canonical_id()
                );
                #[cfg(not(feature = "rt64"))]
                unreachable!("RT64 create cannot succeed without the example's `rt64` feature");
            }
            (Box::new(backend), "rt64", capture)
        }
        value => panic!("wm2000-boot: FN64_RENDER must be reference or rt64, got {value:?}"),
    };
    fn64_abi::set_render_backend_with_policy(render_backend, rdram.len(), graphics_policy);
    println!(
        "[wm2000-boot] registered {active_renderer} renderer (320x240), graphics policy \
         {graphics_policy:?}; RT64 post-VI capture={} (WM2000_RT64_CAPTURE overrides dump gates)",
        capture_rt64_present
    );

    // Typed IPL video standard is the shared VI/AI clock authority. The first
    // field uses nominal NTSC timing; the latched OSViMode H/V registers then
    // refine it from the public VI clock.
    fn64_abi::configure_tv_type(tv_type);

    // Arm crash-safe incremental trace flushing BEFORE booting thread 0 --
    // a SIGSEGV mid-boot (as rung 3 hit) must not lose the whole session's
    // trace; every event from here on is appended+flushed to disk as it's
    // recorded, not just buffered for the end-of-run `write_trace_file`
    // call below (which still runs too, on a clean exit, and rewrites the
    // same path from the in-memory copy -- harmless, since by then the
    // incremental sink already has every event that copy will contain).
    // The incremental trace sink appends+flushes a JSONL event per executor
    // step; invaluable for boot debugging but pure overhead on a throughput
    // run. `WM2000_NO_TRACE=1` skips arming it (the end-of-run write_trace_file
    // is also gated on it below); `WM2000_TRACE_PATH` overrides the location.
    if trace_disabled {
        println!("[wm2000-boot] trace sink DISABLED (WM2000_NO_TRACE)");
    } else if let Err(e) = fn64_abi::set_trace_sink_file(&trace_path) {
        eprintln!(
            "[wm2000-boot] WARNING: failed to arm incremental trace sink at {trace_path}: {e} -- \
             a crash mid-boot will lose the trace (falling back to end-of-run-only)."
        );
    } else {
        println!("[wm2000-boot] incremental trace sink armed at {trace_path}");
    }

    // rdram: this process's one shared buffer (docs/DESIGN.md section 3).
    // Sized to also cover the `0xA4xxxxxx` hardware-register window
    // (`fn64_runtime::RDRAM_MMIO_WINDOW_END`), not just plain RDRAM content
    // -- this is the fix for the exact crash this harness hit: a raw guest
    // `lw` at `AI_STATUS` (`0xA450000C`) previously read 4x past an 8 MB
    // buffer's end (`docs/BOOT-NOTES-WM2000.md`'s LLDB-confirmed
    // `EXC_BAD_ACCESS`). See `fn64_runtime::mmio`'s module doc for the full
    // story.
    let rdram_ptr = rdram.as_mut_ptr();

    // Prime the MMIO backing bytes before the guest ever runs, so even a
    // raw load before any host-side register mutation observes the real
    // idle-hardware defaults (e.g. AI_STATUS not-busy/not-full,
    // SP_STATUS halted+broke) rather than zeroed memory.
    unsafe { fn64_abi::sync_mmio_into_rdram(rdram_ptr) };

    println!("[wm2000-boot] booting thread 0 (recomp_entrypoint)...");
    unsafe {
        fn64_abi::boot_thread0(
            rdram_ptr,
            rdram.len(),
            fn64_boot_harness::c_recomp_entrypoint(),
            0,
            10,
        );
    }

    // Drain guest work to the public idle-thread quiescence boundary before
    // advancing each virtual VI field. This keeps host time independent of a
    // recompiler lane's internal checkpoint density while still bounding a
    // genuinely non-idle spin with MAX_STEPS.
    const LOG_EVERY: u64 = 50_000;
    let max_steps = std::env::var("WM2000_MAX_STEPS").map_or(20_000_000, |raw| {
        let parsed = raw
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("WM2000_MAX_STEPS must be a positive integer, got {raw:?}"));
        assert!(
            parsed > 0,
            "WM2000_MAX_STEPS must be a positive integer, got {raw:?}"
        );
        parsed
    });
    // How many consecutive "nothing was runnable, and advancing the
    // virtual clock didn't wake anything either" ticks before concluding
    // boot has reached a genuinely idle steady state (not just a thread
    // temporarily blocked waiting for a soon-to-fire timer/retrace).
    const IDLE_TICKS_BEFORE_STOP: u32 = 200;
    let mut last_swap_count = 0u64;
    let mut last_rt64_capture_swap = 0u64;
    let mut fb_dumps = Vec::new();
    let mut thread0_death_logged = false;
    let mut consecutive_idle_ticks = 0u32;
    let mut stop_defer_logged = false;

    // WM2000_STOP_AT_SWAP=<n>: clean, bounded scripted runs -- stop (with the
    // normal summary + trace + shutdown path) as soon as the VI swap counter
    // reaches <n>. Swap-indexed like the input script, so a scripted ladder
    // run can end deterministically right after its last press window's
    // outcome has presented, instead of being killed by hand.
    let stop_at_swap = std::env::var("WM2000_STOP_AT_SWAP").ok().map(|raw| {
        let parsed = raw.parse::<u64>().unwrap_or_else(|_| {
            panic!("WM2000_STOP_AT_SWAP must be a positive integer, got {raw:?}")
        });
        assert!(
            parsed > 0,
            "WM2000_STOP_AT_SWAP must be a positive integer, got {raw:?}"
        );
        parsed
    });

    // Voice-map virgin-allocation reproduction (2026-07-21, ladder 3 rung):
    // the AKI sound driver allocates its 4x0x30C per-channel array
    // (func_800E1DF0, pointer stored at D_8011B5D8) UNCLEARED from the
    // game's own next-fit heap, and its announcer request protocol
    // (func_8011E4BC fires exactly one frame after assigning a wrestler id)
    // resolves sound code 0 through the per-channel code->fileId voice map
    // at chan+4 BEFORE the frame-paced installer (func_800F6ED8, reached via
    // func_800F5704's assignment countdown) has populated it. The guest code
    // is explicitly robust to a ZERO map entry -- func_800F5190's code-0 rule
    // (asm 0x800F5310) falls back to the built-in announce tables
    // (D_801065A0..D_80106610) when the slot reads 0 -- but NOT to arbitrary
    // garbage. On hardware the fresh array reads the virgin (arena-init
    // bzero'd, func_80000898 first call) zeros because the heap's next-fit
    // roving pointer position at scene-init time is a function of thousands
    // of timing-dependent alloc/free pairs (the chunked/streamed loaders);
    // under fn64's current virtual-time pacing (the game frame loop runs
    // ~1400 VI fields per frame against a hardware-cadence audio pump) that
    // history collapses and the array lands exactly on the freed
    // decompression temp of sound-map file 0x435, so slot 0 reads the stale
    // compressed byte pair 0xC5BE and the streamer asserts on a wild fileId
    // (the previously-parked func_80003DD4 bounds assert). Until fn64's
    // frame pacing reproduces hardware's heap history, reproduce the
    // hardware-visible OUTCOME at the harness level: whenever a NEW chan
    // array pointer appears at D_8011B5D8, zero the four 0x124-entry
    // (0x248-byte) voice maps at chan+4 -- exactly the virgin bytes the
    // guest's own zero-tolerant fallback is designed for. Real installs
    // (func_800F6ED8 runs, verified) land later and overwrite freely.
    const WM2000_CHAN_ARRAY_PTR: u32 = 0x11B5D8; // D_8011B5D8 - 0x80000000
    const WM2000_CHAN_STRIDE: u32 = 0x30C;
    const WM2000_VOICE_MAP_BYTES: u32 = 0x248;
    let mut last_chan_array_ptr = 0u32;

    // Scripted controller input (2026-07-21, ladder-3 "press START" rung):
    // deterministic, swap-indexed input injection through the REAL input seam
    // (`fn64_abi::set_controller_state` -> `PifModel::set_input` -> the PIF
    // command-1 read-data response the game's own osContGetReadData/raw SI
    // DMA consumes). Swap-count indexing (not wall-clock) keeps runs
    // reproducible: the swap counter is itself a pure function of virtual
    // time. Two env knobs, composable:
    //
    // - `WM2000_PRESS_START=<swap>[+<hold>]` -- convenience: hold START
    //   (0x1000, `OSContPad.button` bit) on port 0 from swap <swap> for
    //   <hold> swaps (default 10), then release.
    // - `WM2000_INPUT_SCRIPT=<from>..<to>:<buttons_hex>[:<sx>:<sy>];...` --
    //   general form: during swaps [from, to) hold `buttons` (hex u16, e.g.
    //   8000=A, 1000=START, 0800..0100=dpad) with optional signed decimal
    //   stick values. Overlapping entries OR their buttons (sticks: last
    //   entry wins), so chords are expressible.
    //
    // The composed state is re-evaluated at every swap-count change and fed
    // through the seam only when it CHANGES -- an idle script leaves the pad
    // untouched (honest neutral default).
    struct InputScriptEntry {
        from: u64,
        to: u64,
        buttons: u16,
        stick_x: i8,
        stick_y: i8,
    }
    let mut input_script: Vec<InputScriptEntry> = Vec::new();
    if let Ok(raw) = std::env::var("WM2000_PRESS_START") {
        let (swap, hold) = match raw.split_once('+') {
            Some((s, h)) => (
                s.parse::<u64>().unwrap_or_else(|_| {
                    panic!("WM2000_PRESS_START swap must be an integer, got {raw:?}")
                }),
                h.parse::<u64>().unwrap_or_else(|_| {
                    panic!("WM2000_PRESS_START hold must be an integer, got {raw:?}")
                }),
            ),
            None => (
                raw.parse::<u64>().unwrap_or_else(|_| {
                    panic!("WM2000_PRESS_START must be <swap>[+<hold>], got {raw:?}")
                }),
                10,
            ),
        };
        input_script.push(InputScriptEntry {
            from: swap,
            to: swap + hold,
            buttons: 0x1000, // BTN_START
            stick_x: 0,
            stick_y: 0,
        });
    }
    if let Ok(raw) = std::env::var("WM2000_INPUT_SCRIPT") {
        for entry in raw.split(';').filter(|s| !s.trim().is_empty()) {
            let mut parts = entry.trim().split(':');
            let range = parts.next().unwrap_or_default();
            let (from, to) = range
                .split_once("..")
                .and_then(|(a, b)| Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?)))
                .unwrap_or_else(|| {
                    panic!("WM2000_INPUT_SCRIPT entry {entry:?}: range must be <from>..<to>")
                });
            let buttons = parts
                .next()
                .and_then(|s| u16::from_str_radix(s, 16).ok())
                .unwrap_or_else(|| {
                    panic!("WM2000_INPUT_SCRIPT entry {entry:?}: buttons must be hex u16")
                });
            let stick_x = parts.next().map_or(0, |s| {
                s.parse::<i8>().unwrap_or_else(|_| {
                    panic!("WM2000_INPUT_SCRIPT entry {entry:?}: stick_x must be i8")
                })
            });
            let stick_y = parts.next().map_or(0, |s| {
                s.parse::<i8>().unwrap_or_else(|_| {
                    panic!("WM2000_INPUT_SCRIPT entry {entry:?}: stick_y must be i8")
                })
            });
            assert!(
                from < to,
                "WM2000_INPUT_SCRIPT entry {entry:?}: empty swap range"
            );
            input_script.push(InputScriptEntry {
                from,
                to,
                buttons,
                stick_x,
                stick_y,
            });
        }
    }
    if !input_script.is_empty() {
        for e in &input_script {
            println!(
                "[wm2000-input] scripted: swaps {}..{} buttons={:#06x} stick=({}, {})",
                e.from, e.to, e.buttons, e.stick_x, e.stick_y
            );
        }
    }
    let mut last_applied_input: (u16, i8, i8) = (0, 0, 0);

    let mut steps = 0u64;
    let mut drain = fn64_boot_harness::GuestDrain::default();
    loop {
        if steps >= max_steps {
            if let Some(stop) = stop_at_swap.filter(|_| capture_rt64_present) {
                assert!(
                    last_rt64_capture_swap >= stop,
                    "wm2000-boot: step budget {max_steps} exhausted before the requested RT64 \
                     post-VI capture through swap {stop}; captured through \
                     {last_rt64_capture_swap}"
                );
            }
            println!(
                "[wm2000-boot] step budget ({max_steps}) exhausted at sim_time={} -- stopping \
                 (this may mean a thread is spinning without truly blocking, or boot just needs \
                 a larger budget)",
                fn64_abi::sim_time()
            );
            break;
        }
        // Re-sync the MMIO model's current values into rdram's real bytes
        // before every step: a coroutine resumed by this step may issue a
        // raw guest MMIO load (see this file's earlier RDRAM sizing comment)
        // at any point, and register state like AI_STATUS's
        // one-shot busy bit can change between steps via an `osAiXxx_recomp`
        // shim call the PREVIOUS step made.
        unsafe { fn64_abi::sync_mmio_into_rdram(rdram_ptr) };
        let next_priority = fn64_abi::next_runnable_priority();
        let advanced_field =
            drain.before_step(next_priority) == fn64_boot_harness::DrainDecision::AdvanceField;
        let mut committed_vi_field = false;
        if advanced_field {
            if release_gate.is_some() {
                let before_host_advance = fn64_abi::sim_time();
                let next_vi = fn64_abi::next_vi_deadline()
                    .expect("typed television standard must keep a VI edge scheduled");
                let tick = fn64_boot_harness::select_release_vi_edge(
                    before_host_advance,
                    next_vi,
                    release_gate.as_ref().map(|(gate, _, _)| gate.guest_cycle()),
                )
                .unwrap_or_else(|error| panic!("wm2000-boot: {error}"));
                let boundary = fn64_boot_harness::commit_scheduled_vi_boundary_with_program(
                    tick,
                    fn64_boot_harness::ReleaseProgramDescriptor::NativeArchive(
                        linked_native_program_identity(),
                    ),
                )
                .unwrap_or_else(|error| panic!("wm2000-boot: commit scheduled VI edge: {error}"));
                committed_vi_field = true;
                let arrival = fn64_boot_harness::ReleaseCycleArrival::HostAdvanceCommitted;
                let release_due = release_gate.as_ref().is_some_and(|(gate, _, _)| {
                    fn64_boot_harness::PresentationReleaseBoundary::new(gate.guest_cycle())
                        .matches(arrival, tick)
                });
                if release_due {
                    let (gate, report_path, rom_class) = release_gate
                        .take()
                        .expect("live release gate disappeared before post-advance capture");
                    let report =
                        capture_release_report(gate, boundary, &report_path, rom_class, &rom_bytes);
                    println!(
                        "[wm2000-boot] RELEASE GATE CLOSED without harness intervention at \
                         post-advance cycle {tick}: report_sha={} artifact_root={} report={}",
                        report.report_sha256,
                        report.digest.root_sha256,
                        report_path.display()
                    );
                    break;
                }
                drain.begin_field();
            } else {
                // Diagnostic mode services the exact earliest device event;
                // release mode instead freezes the complete scheduled VI
                // boundary above before any guest code can resume.
                committed_vi_field = matches!(
                    drain.advance_to_next_device_event(),
                    fn64_boot_harness::DeviceAdvance::ViFields { .. }
                );
            }
        } else {
            let stepped = fn64_abi::run_one_step();
            assert!(
                stepped,
                "guest drain authorized a scheduling step without runnable work"
            );
            drain.record_step(next_priority.expect("guest drain lost runnable priority"));
            steps += 1;
        }
        if let Some((gate, _, _)) = release_gate.as_ref() {
            let observed_cycle = fn64_abi::sim_time();
            assert!(
                observed_cycle <= gate.guest_cycle(),
                "wm2000-boot: release gate cycle {} was skipped; current guest cycle is {observed_cycle}",
                gate.guest_cycle()
            );
        }
        // Virgin-allocation reproduction for the AKI voice maps -- see the
        // block comment above `last_chan_array_ptr` for the full story.
        {
            let view = fn64_runtime::RdramView::from_storage(&rdram);
            let chan_ptr =
                view.read_u32(fn64_runtime::RdramAddr::from_offset(WM2000_CHAN_ARRAY_PTR));
            if chan_ptr != last_chan_array_ptr {
                if chan_ptr >= 0x8000_0000
                    && (chan_ptr as u64 - 0x8000_0000 + u64::from(WM2000_CHAN_STRIDE) * 4)
                        < fn64_boot_harness::rdram_len() as u64
                    && chan_ptr.is_multiple_of(4)
                {
                    require_diagnostic_voice_map_reproduction(release_mode, chan_ptr, steps);
                    let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                    for chan in 0..4u32 {
                        let map_guest = chan_ptr + chan * WM2000_CHAN_STRIDE + 4;
                        for w in (0..WM2000_VOICE_MAP_BYTES).step_by(4) {
                            view.write_u32(
                                fn64_runtime::RdramAddr::from_offset(map_guest - 0x8000_0000 + w),
                                0,
                            );
                        }
                    }
                    println!(
                        "[wm2000-boot] fresh sound-channel array at {chan_ptr:#010x} \
                         (step {steps}): zeroed the 4 per-channel voice maps (chan+4, \
                         {WM2000_VOICE_MAP_BYTES:#x} bytes each) to the virgin-arena bytes the \
                         guest's zero-tolerant code-0 fallback expects -- see source comment."
                    );
                }
                last_chan_array_ptr = chan_ptr;
            }
        }
        if steps.is_multiple_of(LOG_EVERY) {
            println!(
                "[wm2000-boot] progress: steps={steps} sim_time={} vi_swaps={} gfx_tasks={} \
                 audio_tasks={}",
                fn64_abi::sim_time(),
                fn64_abi::vi_swap_count(),
                fn64_abi::task_counts().0,
                fn64_abi::task_counts().1
            );
        }
        // Thread 0 (recomp_entrypoint) returning is EXPECTED, not the end
        // of boot: recomp_entrypoint's own body does `LOOKUP_FUNC(..)(rdram,
        // ctx); return;` once its initial call chain unwinds -- real game
        // logic continues on the OTHER threads osCreateThread/osStartThread
        // spawned along the way (thread 1, thread 6, etc, per the ladder's
        // own multi-thread evidence). Only log this once, never treat it as
        // "boot ended" -- the executor's run queue (other threads) is the
        // real signal, handled by the drain/idle-tick counter below.
        if !thread0_death_logged && fn64_abi::is_thread_dead(0) {
            println!(
                "[wm2000-boot] thread 0 (recomp_entrypoint) returned at step {steps} -- expected \
                 (its own initial call chain unwound); other threads keep running"
            );
            thread0_death_logged = true;
        }

        // Framebuffer capture: on every new osViSwapBuffer, hash+dump if
        // non-uniform (Task requirement 3).
        let swap_count = fn64_abi::vi_swap_count();
        if swap_count > last_swap_count {
            // Scripted input: compose the pad state for THIS swap index and
            // feed it through the real seam only on change (see the script
            // block above the main loop).
            if !input_script.is_empty() {
                let mut buttons = 0u16;
                let mut stick = (0i8, 0i8);
                for e in &input_script {
                    if swap_count >= e.from && swap_count < e.to {
                        buttons |= e.buttons;
                        if e.stick_x != 0 || e.stick_y != 0 {
                            stick = (e.stick_x, e.stick_y);
                        }
                    }
                }
                let desired = (buttons, stick.0, stick.1);
                if desired != last_applied_input {
                    fn64_abi::set_controller_state(0, desired.0, desired.1, desired.2);
                    println!(
                        "[wm2000-input] swap #{swap_count}: pad0 -> buttons={:#06x} \
                         stick=({}, {})",
                        desired.0, desired.1, desired.2
                    );
                    let _ = std::io::stdout().flush();
                    last_applied_input = desired;
                }
            }
            if !dumps_disabled && active_renderer == "reference" {
                if let Some(fb_offset) = fn64_abi::current_vi_framebuffer() {
                    capture_framebuffer(&rdram, fb_offset, swap_count, &mut fb_dumps);
                }
            }
            last_swap_count = swap_count;
        }

        // RT64 renders into its native targets; `current_vi_framebuffer()` is
        // only the guest RDRAM address and is not the post-VI image. Capture
        // through the backend-owned fenced readback after the first committed
        // VI field following each new swap, and bind the diagnostic to both
        // native workload/present identity and an exact SHA-256.
        if capture_rt64_present && committed_vi_field && swap_count > last_rt64_capture_swap {
            match fn64_abi::capture_render_release_frame() {
                Ok(capture) => {
                    capture_rt64_frame(capture, swap_count, !dumps_disabled, &mut fb_dumps);
                    last_rt64_capture_swap = swap_count;
                }
                Err(fn64_render::RenderError::NotReady(
                    "RT64 release capture requested before a completed VI present",
                )) => {}
                Err(fn64_render::RenderError::NotReady(
                    "RT64 has no completed post-workload present capture",
                )) => {}
                Err(error) => {
                    panic!("wm2000-boot: capture RT64 post-VI frame for swap {swap_count}: {error}")
                }
            }
        }

        if let Some(stop) = stop_at_swap.filter(|stop| swap_count >= *stop) {
            if capture_rt64_present && last_rt64_capture_swap < stop {
                if !stop_defer_logged {
                    println!(
                        "[wm2000-boot] WM2000_STOP_AT_SWAP={stop} reached at swap \
                         #{swap_count}; waiting for its next committed RT64 post-VI capture"
                    );
                    stop_defer_logged = true;
                }
            } else {
                println!(
                    "[wm2000-boot] WM2000_STOP_AT_SWAP={stop} satisfied (swap #{swap_count}, \
                     RT64 captured through #{last_rt64_capture_swap}, step {steps}, sim_time={}) \
                     -- stopping",
                    fn64_abi::sim_time()
                );
                break;
            }
        }

        if advanced_field {
            if fn64_abi::next_runnable_priority().is_none() {
                consecutive_idle_ticks += 1;
            } else {
                consecutive_idle_ticks = 0;
            }
            if consecutive_idle_ticks >= IDLE_TICKS_BEFORE_STOP {
                if let Some(stop) = stop_at_swap.filter(|_| capture_rt64_present) {
                    assert!(
                        last_rt64_capture_swap >= stop,
                        "wm2000-boot: reached steady idle before the requested RT64 post-VI \
                         capture through swap {stop}; captured through \
                         {last_rt64_capture_swap}"
                    );
                }
                println!(
                    "[wm2000-boot] reached a steady idle state ({IDLE_TICKS_BEFORE_STOP} \
                     consecutive ticks with nothing runnable) at sim_time={} steps={steps} -- \
                     stopping",
                    fn64_abi::sim_time()
                );
                break;
            }
        } else {
            consecutive_idle_ticks = 0;
        }
    }

    if let Some(stop) = stop_at_swap.filter(|_| capture_rt64_present) {
        assert!(
            last_rt64_capture_swap >= stop,
            "wm2000-boot: exited without the requested RT64 post-VI capture through swap \
             {stop}; captured through {last_rt64_capture_swap}"
        );
    }
    if let Some((gate, report_path, _)) = release_gate.as_ref() {
        panic!(
            "wm2000-boot: stopped before intervention-free live release gate cycle {}; current \
             cycle={}, report was not written to {}",
            gate.guest_cycle(),
            fn64_abi::sim_time(),
            report_path.display()
        );
    }

    let (gfx_count, audio_count) = fn64_abi::task_counts();
    println!("[wm2000-boot] === BOOT SUMMARY ===");
    println!("[wm2000-boot] virtual ticks run: {}", fn64_abi::sim_time());
    println!(
        "[wm2000-boot] thread 0 dead: {}",
        fn64_abi::is_thread_dead(0)
    );
    println!(
        "[wm2000-boot] VI swaps observed: {}",
        fn64_abi::vi_swap_count()
    );
    println!("[wm2000-boot] gfx tasks submitted: {gfx_count}");
    println!("[wm2000-boot] audio tasks submitted: {audio_count}");
    println!("[wm2000-boot] renderer: {active_renderer}, graphics policy: {graphics_policy:?}");
    let last_render_error = fn64_abi::last_render_error();
    println!("[wm2000-boot] last render error: {last_render_error:?}");
    assert!(
        last_render_error.is_none(),
        "wm2000-boot: renderer finished with a recorded error: {last_render_error:?}"
    );
    println!(
        "[wm2000-boot] frame images dumped: {} ({:?})",
        fb_dumps.len(),
        fb_dumps
    );

    if !trace_disabled {
        let trace = fn64_abi::copy_trace();
        println!("[wm2000-boot] trace events recorded: {}", trace.len());
        write_trace_file(&trace, &trace_path);
        println!("[wm2000-boot] trace written to {trace_path}");
    }

    let exit = fn64_abi::prepare_process_exit();
    println!(
        "[wm2000-boot] process exit prepared: threads={} detached_coroutines={}",
        exit.threads, exit.detached_coroutines
    );
}

fn capture_release_report(
    gate: fn64_boot_harness::LiveReleaseGate,
    boundary: fn64_boot_harness::CommittedViBoundary,
    report_path: &std::path::Path,
    rom_class: fn64_boot_harness::ReleaseRomClass,
    rom_bytes: &[u8],
) -> fn64_boot_harness::ReleaseGateReport {
    use fn64_boot_harness::LiveReleaseGateRenderExt as _;

    let observed_cycle = fn64_abi::sim_time();
    let capture = fn64_abi::capture_render_release_frame().unwrap_or_else(|error| {
        panic!("wm2000-boot: capture RT64 fixed-cycle presentation: {error}")
    });
    assert_eq!(
        capture.guest_cycle, observed_cycle,
        "wm2000-boot: RT64 presentation belongs to guest cycle {}, release gate requires \
         {observed_cycle}",
        capture.guest_cycle
    );
    assert!(
        capture.source_authoritative,
        "wm2000-boot: RT64 fixed-cycle capture has non-authoritative backend identity {}",
        capture.backend_identity
    );
    let format = match capture.format {
        fn64_render::ReleaseCaptureFormat::PostViBgra8Unorm => {
            fn64_boot_harness::RenderPixelFormat::Bgra8Unorm
        }
    };
    let render = fn64_boot_harness::LiveRenderEvidence::post_vi_swapchain(
        capture.guest_cycle,
        capture.backend_identity,
        capture.settings_sha256,
        capture.width,
        capture.height,
        capture.row_bytes,
        format,
        capture.workload_id.get(),
        capture.present_id,
        capture.bytes,
    )
    .unwrap_or_else(|error| panic!("wm2000-boot: validate RT64 fixed-cycle presentation: {error}"));
    gate.capture_and_write_render_rom_evidence(
        boundary,
        "wm2000-ntsc-headless-c-rt64-lle-accuracy-intervention-free",
        fn64_boot_harness::ReleaseRomInput::new(rom_class, rom_bytes),
        &render,
        report_path,
    )
    .unwrap_or_else(|error| {
        panic!(
            "wm2000-boot: intervention-free live release gate failed at post-advance cycle \
             {observed_cycle}; report path {}: {error}",
            report_path.display()
        )
    })
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Dump RT64's exact post-VI BGRA8 readback. This is deliberately separate
/// from `capture_framebuffer`: native output is not inferred from guest RDRAM.
fn capture_rt64_frame(
    capture: fn64_render::RenderReleaseCapture,
    swap_index: u64,
    dump_png: bool,
    dumps: &mut Vec<String>,
) {
    assert_eq!(
        capture.format,
        fn64_render::ReleaseCaptureFormat::PostViBgra8Unorm,
        "wm2000-boot: RT64 returned an unsupported post-VI pixel format"
    );
    let width = usize::try_from(capture.width).expect("RT64 capture width exceeds usize");
    let height = usize::try_from(capture.height).expect("RT64 capture height exceeds usize");
    let row_bytes =
        usize::try_from(capture.row_bytes).expect("RT64 capture row stride exceeds usize");
    let visible_row_bytes = width
        .checked_mul(4)
        .expect("RT64 capture visible row size overflow");
    assert!(
        row_bytes >= visible_row_bytes,
        "wm2000-boot: RT64 capture row stride {row_bytes} is smaller than {width} BGRA8 pixels"
    );
    let expected_len = row_bytes
        .checked_mul(height)
        .expect("RT64 capture byte length overflow");
    assert_eq!(
        capture.bytes.len(),
        expected_len,
        "wm2000-boot: RT64 capture byte length does not match its declared geometry"
    );

    let frame_sha256 = hex_sha256(&capture.bytes);
    let settings_sha256 = capture
        .settings_sha256
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    println!(
        "[wm2000-rt64] swap #{swap_index}: cycle={} workload={} present={} post_vi={}x{} \
         row_bytes={} sha256={} settings_sha256={} backend={} source_authoritative={}",
        capture.guest_cycle,
        capture.workload_id,
        capture.present_id,
        capture.width,
        capture.height,
        capture.row_bytes,
        frame_sha256,
        settings_sha256,
        capture.backend_identity,
        capture.source_authoritative
    );

    if !dump_png {
        return;
    }

    let mut rgba = Vec::with_capacity(
        width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .expect("RT64 capture RGBA conversion size overflow"),
    );
    for row in capture.bytes.chunks_exact(row_bytes) {
        for bgra in row[..visible_row_bytes].chunks_exact(4) {
            rgba.extend_from_slice(&[bgra[2], bgra[1], bgra[0], bgra[3]]);
        }
    }
    let dir = std::env::var("WM2000_FB_DUMP_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let path = format!(
        "{dir}/fn64-rt64-post-vi-swap-{swap_index}-present-{}.png",
        capture.present_id
    );
    write_png(&path, capture.width, capture.height, &rgba)
        .unwrap_or_else(|error| panic!("wm2000-boot: write RT64 post-VI capture {path}: {error}"));
    dumps.push(path);
}

/// Hash the fb region (a fixed-size guess: 320x240 RGBA5551 = 153600 bytes,
/// the common NTSC low-res mode -- NOT verified against this ROM's actual
/// `osViSetMode` mode-table contents, since this milestone doesn't decode
/// `OSViMode`'s fields; see `fn64_runtime::vi`'s doc comment on why that's
/// not modeled yet). If ANY byte differs from the first (non-uniform),
/// convert RGBA5551 -> RGBA8888 and dump a PNG. A uniform/blank buffer is
/// reported as blank, never faked as containing real content.
fn capture_framebuffer(rdram: &[u8], fb_offset: u32, swap_index: u64, dumps: &mut Vec<String>) {
    const FB_WIDTH: usize = 320;
    const FB_HEIGHT: usize = 240;
    const FB_BYTES: usize = FB_WIDTH * FB_HEIGHT * 2; // RGBA5551, 2 bytes/px

    let start = fb_offset as usize;
    let end = start + FB_BYTES;
    if end > rdram.len() {
        eprintln!(
            "[wm2000-boot] swap #{swap_index}: framebuffer offset {fb_offset:#x} + assumed size \
             {FB_BYTES:#x} exceeds rdram bounds ({} bytes) -- skipping capture, not guessing a \
             smaller region",
            rdram.len()
        );
        return;
    }
    let region = &rdram[start..end];
    let first_byte = region[0];
    let uniform = region.iter().all(|&b| b == first_byte);

    if uniform {
        println!(
            "[wm2000-boot] swap #{swap_index}: framebuffer at {fb_offset:#010x} is UNIFORM \
             (all bytes == {first_byte:#04x}) -- reported as blank, not dumped (per task: \"a \
             blank/uniform fb is reported as blank\")."
        );
        return;
    }

    println!(
        "[wm2000-boot] swap #{swap_index}: framebuffer at {fb_offset:#010x} is NON-UNIFORM -- \
         dumping PNG."
    );
    // WM2000_FB_DUMP_DIR: where swap PNGs land (default /tmp) -- lets two
    // scripted runs coexist without clobbering each other's frames.
    let dir = std::env::var("WM2000_FB_DUMP_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let path = format!("{dir}/fn64-fb-{swap_index}.png");
    match dump_rgba5551_as_png(rdram, start, FB_WIDTH, FB_HEIGHT, &path) {
        Ok(()) => {
            println!("[wm2000-boot] *** NON-UNIFORM FRAMEBUFFER DUMPED: {path} ***");
            dumps.push(path);
        }
        Err(e) => eprintln!("[wm2000-boot] failed to write {path}: {e}"),
    }
}

/// Convert logical N64 RGBA5551 halfwords (RRRRRGGGGGBBBBBA) from fn64's
/// native-word RDRAM storage to RGBA8888 and write a minimal PNG (a hand-rolled
/// uncompressed-DEFLATE encoder -- no `png`/`image` crate dependency for
/// this one-shot dump, keeping this example's dependency footprint at just
/// `cc` for the C bridge).
fn dump_rgba5551_as_png(
    rdram: &[u8],
    start: usize,
    width: usize,
    height: usize,
    path: &str,
) -> std::io::Result<()> {
    let mut rgba = Vec::with_capacity(width * height * 4);
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let start = fn64_runtime::RdramAddr::from_offset(
        u32::try_from(start).expect("framebuffer RDRAM offset exceeds u32"),
    );
    assert!(
        start.offset().is_multiple_of(4),
        "framebuffer offset {:#x} is not word-aligned",
        start.offset()
    );
    for i in 0..width * height {
        let px = view.read_u16(
            start
                .checked_add(u32::try_from(i * 2).expect("framebuffer byte offset exceeds u32"))
                .expect("framebuffer logical address overflow"),
        );
        let r5 = (px >> 11) & 0x1F;
        let g5 = (px >> 6) & 0x1F;
        let b5 = (px >> 1) & 0x1F;
        let a1 = px & 0x1;
        let expand5 = |v: u16| ((v * 255 + 15) / 31) as u8;
        rgba.push(expand5(r5));
        rgba.push(expand5(g5));
        rgba.push(expand5(b5));
        rgba.push(if a1 != 0 { 255 } else { 0 });
    }
    write_png(path, width as u32, height as u32, &rgba)
}

fn write_png(path: &str, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])?;

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // color type: RGBA
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    write_chunk(&mut file, b"IHDR", &ihdr)?;

    // Raw scanlines with a filter-type-0 byte prefix per row.
    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0u8);
        raw.extend_from_slice(&rgba[row * stride..(row + 1) * stride]);
    }
    let idat = deflate_stored(&raw);
    write_chunk(&mut file, b"IDAT", &idat)?;
    write_chunk(&mut file, b"IEND", &[])?;
    Ok(())
}

fn write_chunk(file: &mut std::fs::File, kind: &[u8; 4], data: &[u8]) -> std::io::Result<()> {
    file.write_all(&(data.len() as u32).to_be_bytes())?;
    file.write_all(kind)?;
    file.write_all(data)?;
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    file.write_all(&crc32(&crc_input).to_be_bytes())?;
    Ok(())
}

/// zlib-wrapped, stored (uncompressed) DEFLATE -- valid per RFC 1950/1951,
/// just not size-optimal. Fine for a diagnostic dump.
fn deflate_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x78);
    out.push(0x01); // zlib header, no dictionary, fastest level
    let mut offset = 0;
    while offset < data.len() || (offset == 0 && data.is_empty()) {
        let remaining = data.len() - offset;
        let block_len = remaining.min(65535);
        let is_final = offset + block_len >= data.len();
        out.push(if is_final { 1 } else { 0 });
        out.extend_from_slice(&(block_len as u16).to_le_bytes());
        out.extend_from_slice(&(!(block_len as u16)).to_le_bytes());
        out.extend_from_slice(&data[offset..offset + block_len]);
        offset += block_len;
        if data.is_empty() {
            break;
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn write_trace_file(trace: &[fn64_runtime::TraceEvent], path: &str) {
    let mut file = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[wm2000-boot] failed to create trace file {path}: {e}");
            return;
        }
    };
    for event in trace {
        let line = format!("{event:?}\n");
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::require_diagnostic_voice_map_reproduction;

    #[test]
    fn diagnostic_mode_allows_the_documented_voice_map_reproduction() {
        require_diagnostic_voice_map_reproduction(false, 0x8030_0000, 17);
    }

    #[test]
    #[should_panic(expected = "no release report may be written")]
    fn release_mode_refuses_the_voice_map_reproduction() {
        require_diagnostic_voice_map_reproduction(true, 0x8030_0000, 17);
    }
}
