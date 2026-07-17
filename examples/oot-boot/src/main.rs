//! OoT (OOTU, NTSC 1.0) headless boot harness on fn64 -- the decomp-driven-
//! first recompilation lane (aki-recomp/games/OOTU). See `build.rs`'s
//! module doc for the `RECOMPILED_DIR`/`RECOMP_H_DIR`/`ROM` env-var
//! contract this binary requires -- this crate itself contains zero game
//! content, per `fn64/README.md`. Structurally identical to
//! `examples/wm2000-boot/src/main.rs`; only the always-resident section
//! set and the audio-ucode stand-in's doc comment differ (see below) --
//! this is the "does fn64 generalize past the AKI titles" test, so
//! deliberately reusing the SAME harness code proves that, rather than a
//! bespoke per-game rewrite.
//!
//! ## What this does
//!
//! 1. Loads the decomp's OWN BUILD-OUTPUT ROM (`ROM` env var -- NOT the
//!    retail compressed cartridge image, see build.rs's module doc) into
//!    `fn64_abi::load_rom`.
//! 2. Registers every section from either the C lane's real, out-of-tree
//!    `recomp_overlays.inl` or the rs module's emitted section geometry,
//!    then marks the always-resident sections (0/1/2 -- makerom.ent/boot/code,
//!    per OoT's OWN linker `.map`: everything before the 469 `ovl_*` actor/
//!    scene overlays, which are heap-loaded on demand via DmaMgr at runtime,
//!    NOT pre-mapped at boot -- see games/OOTU/profile.toml's `[segments]`).
//! 3. Boots thread 0 through the selected C `recomp_entrypoint` or typed-Rust
//!    `entrypoint`, then drives the same executor: `run_one_step` while
//!    runnable and `advance_virtual_time` for host VI/timer progress.
//! 4. On every `osViSwapBuffer_recomp` call (observed via
//!    `fn64_abi::vi_swap_count()` polling), hashes the pointed-to
//!    framebuffer region and dumps it as a PNG if non-uniform.
//! 5. When `OOT_TRACE=1`, emits the trace log to a file; always prints a
//!    summary ladder.

use std::io::Write;

#[cfg(fn64_recomp_rs)]
use oot_recompiled as recompiled;

/// Audio ucode stand-in. No RSPRecomp pass has been run against OoT's audio
/// microcode in this bring-up (out of scope for the decomp-driven-recompile
/// gate this harness proves) -- and even if one had been, the same
/// clean-room blocker `examples/wm2000-boot/src/main.rs` documents applies
/// identically: RSPRecomp's own codegen template unconditionally
/// `#include`s the GPL-3.0-licensed `librecomp/rsp.hpp`
/// (`N64RecompSource/RSPRecomp/src/rsp_recomp.cpp:1179`), disallowed by
/// `fn64/AGENTS.md`'s clean-room protocol regardless of which game it's
/// generated for. This stand-in exercises the REAL plumbing
/// (`fn64_abi::set_audio_ucode_fn`/`osSpTaskYielded_recomp`'s M_AUDTASK
/// dispatch) without linking the disallowed dependency or claiming a real
/// ucode was ported. It does nothing to rdram, just proves the call
/// happened.
unsafe extern "C" fn stand_in_audio_ucode(_rdram: *mut u8, ucode_addr: u32) -> u32 {
    eprintln!(
        "[oot-boot] STAND-IN audio ucode invoked for ucode_addr={ucode_addr:#010x} -- NOT a real \
         translated ucode (no RSPRecomp pass has been run for OoT in this bring-up; see main.rs's \
         doc comment for the clean-room reason a real one couldn't be linked in even if it had \
         been). Plumbing is real; ucode execution is not."
    );
    0
}

fn env_path(name: &str) -> std::path::PathBuf {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("oot-boot: required environment variable {name} not set"))
        .into()
}

/// Open a real host output stream and register it at fn64's AI DMA boundary.
/// RDRAM bounds are registered even when no device exists so live PCM stats
/// and `FN64_DUMP_AUDIO_PCM` remain available in headless runs.
fn wire_audio_output(rdram_len: usize) {
    fn64_abi::set_audio_rdram_len(rdram_len);
    if std::env::var_os("FN64_NO_AUDIO").is_some() {
        println!("[oot-boot] FN64_NO_AUDIO set -- cpal output disabled");
        return;
    }

    use fn64_audio::{AudioBackend as _, AudioConfig, CpalBackend};
    // OoT's boot-time AI rate; osAiSetFrequency forwards the true DAC rate
    // to the backend later, so this is a starting ratio, not a commitment.
    // The backend negotiates the host stream rate with the device itself
    // (falling back to the device default + linear resampling), so the old
    // multi-rate retry ladder here is gone -- playing 32 kHz samples on a
    // 48 kHz stream without conversion was the "background static" bug.
    const OOT_BOOT_AI_RATE_HZ: u32 = 32_000;
    let mut backend = CpalBackend::new();
    match backend.create(&AudioConfig::new(OOT_BOOT_AI_RATE_HZ, 2)) {
        Ok(()) => {
            let stream_rate = backend.stream_rate_hz().unwrap_or(OOT_BOOT_AI_RATE_HZ);
            fn64_abi::set_audio_backend(Box::new(backend), rdram_len);
            println!(
                "[oot-boot] audio output wired (cpal, guest {OOT_BOOT_AI_RATE_HZ} Hz -> \
                 stream {stream_rate} Hz stereo)"
            );
        }
        Err(error) => eprintln!(
            "[oot-boot] audio output unavailable ({error}); live PCM stats/dump remain enabled"
        ),
    }
}

/// One scripted-input step: at VI-swap `frame`, hold `buttons` (an
/// `OSContPad.button` bitmask, controller.h:4-17) with the analog stick at
/// `(stick_x, stick_y)`, until the next step changes it.
#[derive(Debug, Clone, Copy)]
struct ScriptStep {
    frame: u64,
    buttons: u16,
    stick_x: i8,
    stick_y: i8,
}

/// Map a controller.h `BTN_*` name to its `OSContPad.button` bit
/// (`refs/oot-decomp/include/controller.h:4-17`). Returns 0 for an unknown
/// name (logged by the caller) so a typo can't silently masquerade as a real
/// press.
fn button_bit(name: &str) -> u16 {
    match name.trim().to_ascii_uppercase().as_str() {
        "A" => 0x8000,
        "B" => 0x4000,
        "Z" => 0x2000,
        "START" => 0x1000,
        "DUP" => 0x0800,
        "DDOWN" => 0x0400,
        "DLEFT" => 0x0200,
        "DRIGHT" => 0x0100,
        "L" => 0x0020,
        "R" => 0x0010,
        "CUP" => 0x0008,
        "CDOWN" => 0x0004,
        "CLEFT" => 0x0002,
        "CRIGHT" => 0x0001,
        "" => 0,
        other => {
            eprintln!(
                "[oot-boot] WARNING: unknown button name {other:?} in OOT_INPUT_SCRIPT -- ignored"
            );
            0
        }
    }
}

/// Build the scripted-input timeline from the environment. Priority:
/// `OOT_INPUT_SCRIPT` (full grammar, see the harness loop's doc), else the
/// discovered `OOT_SCRIPT_INTERACTIVE=1` title-to-gameplay route, else
/// `OOT_SCRIPT_START=N` shorthand (press+release Start around frame N), else
/// empty (idle). The returned steps are sorted by frame.
fn build_input_script() -> Vec<ScriptStep> {
    if let Ok(spec) = std::env::var("OOT_INPUT_SCRIPT") {
        let mut steps = parse_input_script(&spec);
        steps.sort_by_key(|s| s.frame);
        return steps;
    }
    if std::env::var_os("OOT_SCRIPT_INTERACTIVE").is_some() {
        return interactive_input_script();
    }
    if let Ok(n) = std::env::var("OOT_SCRIPT_START") {
        if let Ok(frame) = n.parse::<u64>() {
            // Press Start at `frame`, release 4 frames later -- a clean tap.
            return vec![
                ScriptStep {
                    frame,
                    buttons: 0x1000,
                    stick_x: 0,
                    stick_y: 0,
                },
                ScriptStep {
                    frame: frame + 4,
                    buttons: 0,
                    stick_x: 0,
                    stick_y: 0,
                },
            ];
        }
    }
    Vec::new()
}

/// OoT NTSC 1.0's verified title -> file-select -> new-file -> controllable
/// Link route. The named-button steps were localized against the C lane at
/// each menu transition; after gameplay starts, the repeated A taps advance
/// the opening dialogue/cutscenes while the held stick proves player motion.
fn interactive_input_script() -> Vec<ScriptStep> {
    let mut steps = parse_input_script(
        "250:START,254:,280:START,284:,360:A,364:,400:A,404:,420:START,424:,\
         440:A,444:,490:A,494:,540:A,544:,620:@60/0",
    );
    for frame in (700..=4_150).step_by(25) {
        steps.push(ScriptStep {
            frame,
            buttons: button_bit("A"),
            stick_x: 60,
            stick_y: 0,
        });
        steps.push(ScriptStep {
            frame: frame + 2,
            buttons: 0,
            stick_x: 60,
            stick_y: 0,
        });
    }
    steps
}

/// Parse `OOT_INPUT_SCRIPT`: comma-separated `frame:BTN[+BTN...][@sx/sy]`.
/// An empty button field (e.g. `50:`) releases all buttons.
fn parse_input_script(spec: &str) -> Vec<ScriptStep> {
    let mut steps = Vec::new();
    for raw in spec.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let (frame_str, rest) = match raw.split_once(':') {
            Some(parts) => parts,
            None => {
                eprintln!(
                    "[oot-boot] WARNING: OOT_INPUT_SCRIPT step {raw:?} has no ':' -- skipped"
                );
                continue;
            }
        };
        let frame: u64 = match frame_str.trim().parse() {
            Ok(f) => f,
            Err(_) => {
                eprintln!(
                    "[oot-boot] WARNING: OOT_INPUT_SCRIPT step {raw:?} has a bad frame -- skipped"
                );
                continue;
            }
        };
        // Optional `@stickX/stickY` suffix.
        let (buttons_part, stick_part) = match rest.split_once('@') {
            Some((b, s)) => (b, Some(s)),
            None => (rest, None),
        };
        let buttons = buttons_part
            .split('+')
            .filter(|s| !s.trim().is_empty())
            .map(button_bit)
            .fold(0u16, |acc, b| acc | b);
        // Stick uses `/` (not `,`) as its X/Y separator, since `,` already
        // separates whole steps at the top level.
        let (stick_x, stick_y) = match stick_part {
            Some(s) => {
                let mut it = s.split('/');
                let sx = it.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0i8);
                let sy = it.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0i8);
                (sx, sy)
            }
            None => (0i8, 0i8),
        };
        steps.push(ScriptStep {
            frame,
            buttons,
            stick_x,
            stick_y,
        });
    }
    steps
}

#[cfg(fn64_recomp_rs)]
mod host_lookup;
#[cfg(fn64_recomp_rs)]
use host_lookup::recompiled_or_host_lookup;

fn main() {
    let rom_path = env_path("ROM");
    println!("[oot-boot] loading ROM from {}", rom_path.display());
    let rom_bytes = std::fs::read(&rom_path)
        .unwrap_or_else(|e| panic!("oot-boot: failed to read ROM {}: {e}", rom_path.display()));
    println!("[oot-boot] ROM size: {} bytes", rom_bytes.len());
    fn64_abi::load_rom(rom_bytes);
    // OoT NTSC 1.0's osCartRomInit materializes and returns __CartRomHandle
    // at this guest BSS address (ROM disassembly 0x80005698-0x8000569C and
    // return path 0x800057BC). AudioLoad_Dma later dereferences the public
    // OSPiHandle transferInfo at 0x800B824C, so the host shim must return the
    // real guest-visible object rather than an opaque token.
    fn64_abi::set_cart_rom_handle_vram(0x8000_9EA0);

    // Register OoT's save-backing store so domain-2 (SRAM, devAddr >=
    // 0x08000000 / PI_DOM2_ADDR2) PI DMAs have somewhere to go instead of
    // being (wrongly) read from the ROM image past its end. OoT uses banked
    // SRAM (32 KiB); Sram_InitSram DMAs the whole 0x8000-byte save in at boot
    // (funcs_34.c:10636). Ephemeral in-memory store for this boot harness --
    // a persisted FileSaveStorage is a shell concern, not this bring-up's.
    fn64_abi::set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
        fn64_runtime::SaveType::SramBanked,
    )));

    #[cfg(not(fn64_recomp_rs))]
    let registration = fn64_boot_harness::register_linked_sections();
    #[cfg(not(fn64_recomp_rs))]
    {
        println!(
            "[oot-boot] bridge reports {} sections in recomp_overlays.inl",
            registration.reported_count()
        );
        for section in registration.sections() {
            println!(
                "[oot-boot] registered section {}: rom={:#010x} ram={:#010x} \
                 size={:#x} funcs={}",
                section.source_index,
                section.rom_addr,
                section.ram_addr,
                section.size,
                section.function_count
            );
        }
    }

    // OoT's own linker .map (games/OOTU/profile.toml's [segments]) puts
    // sections 0/1/2 (makerom.ent/boot/code) as the always-resident image;
    // the remaining 469 are ovl_* actor/scene overlays, each independently
    // relocated and heap-loaded on demand via DmaMgr at runtime (see
    // games/OOTU/docs/decomp-import-notes.md's "Overlay/segment structural
    // note" -- zero vram collisions across all 469, NOT a fixed 2-5-slot
    // bank-swap region like the AKI titles' overlay design). This harness
    // does not yet drive DmaMgr's overlay-load path, so only the
    // always-resident sections are marked loaded, matching real boot-time
    // hardware state (no actor overlay has been DMA'd in yet this early).
    #[cfg(not(fn64_recomp_rs))]
    for section_key in [0usize, 1usize, 2usize] {
        if let Some(idx) = registration.registry_index(section_key) {
            fn64_abi::set_section_loaded(idx);
            println!("[oot-boot] marked section {section_key} (index {idx}) loaded");
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
            "[oot-boot] registered {} recompiled section geometries; marked 0/1/2 resident",
            section_indices.len()
        );
    }

    let _ = stand_in_audio_ucode; // kept for reference; real ucode wired below

    // VI retrace: arm a host-chosen approximation (fn64_runtime::vi's doc:
    // not a hardware-accurate NTSC/PAL constant). 1000 virtual-time units
    // per field is an arbitrary but documented choice for this harness.
    fn64_abi::arm_vi_retrace(1000);

    // Opt-in crash-safe incremental trace flushing BEFORE booting thread 0 --
    // a SIGSEGV mid-boot (as rung 3 hit) must not lose the whole session's
    // trace; every event from here on is appended+flushed to disk as it's
    // recorded, not just buffered for the end-of-run `write_trace_file`
    // call below (which still runs too, on a clean exit, and rewrites the
    // same path from the in-memory copy -- harmless, since by then the
    // incremental sink already has every event that copy will contain).
    const TRACE_PATH: &str = "/tmp/oot-boot-trace.jsonl";
    let trace_enabled = std::env::var_os("OOT_TRACE").is_some();
    if trace_enabled {
        if let Err(e) = fn64_abi::set_trace_sink_file(TRACE_PATH) {
            eprintln!(
                "[oot-boot] WARNING: failed to arm incremental trace sink at {TRACE_PATH}: {e} -- \
                 a crash mid-boot will lose the trace (falling back to end-of-run-only)."
            );
        } else {
            println!("[oot-boot] incremental trace sink armed at {TRACE_PATH}");
        }
    } else {
        fn64_abi::set_trace_enabled(false);
        println!("[oot-boot] differential trace disabled (set OOT_TRACE=1 to enable)");
    }

    // rdram: this process's one shared buffer (docs/DESIGN.md section 3).
    // `.max(RDRAM_MMIO_WINDOW_END)` (same pattern as examples/wm2000-boot's
    // harness) is REQUIRED, not just extra headroom: OoT's own boot path
    // (`CIC6105_SaveBootMagicValues`, `RecompiledFuncs/funcs_0.c`) issues a
    // raw `MEM_W` load at a KSEG1 (uncached) RDRAM address
    // (`0xA0300000 - 0x4E0C` / `-0x1E40`, verified byte-exact against
    // `refs/oot-decomp/src/boot/cic6105.c`'s own `IO_READ(0x002FB1F4)` /
    // `IO_READ(0x002FE1C0)` -- KSEG1's `0xA0300000` base is a plain
    // uncached alias of physical RDRAM offset `0x00300000`, so this is
    // real hardware semantics, not a translation bug in `RdramAddr`).
    // `recomp.h`'s real `MEM_W` macro subtracts the KSEG0 base
    // unconditionally for every address (verified against
    // `refs/N64RecompSource/include/recomp.h`), so a KSEG1 address lands
    // 0x20000000 bytes further into the buffer than the same physical
    // offset would via KSEG0 -- a flat 8 MB buffer is a real out-of-bounds
    // read/SIGSEGV here (first hit: this harness's very first
    // `recomp_entrypoint` call, before any thread/section work even
    // starts). A plain 8 MB buffer was never big enough for a game that
    // touches KSEG1-mirrored RDRAM at boot; WM2000/NW4E's own
    // `RecompiledFuncs` corpora happen not to exercise this address range,
    // which is why examples/wm2000-boot's identical `.max(...)` guard
    // never had to fire in that harness's own testing, not because the
    // sizing itself is game-specific.
    let mut rdram = fn64_boot_harness::new_rdram();
    let rdram_ptr = rdram.as_mut_ptr();

    // Select the renderer before boot. `FN64_RENDER=rt64` requests the
    // feature-gated RT64 path; an unavailable display/GPU or a binary built
    // without `--features rt64` returns a named create error and falls back
    // to the reference oracle. rdram_len MUST match the shared allocation
    // from fn64-boot-harness so both backends and the ABI agree on bounds.
    //
    // NOTE (honest): the CONCURRENT display-list-pointer fix has not
    // necessarily landed, so OoT's live polyOpa/polyXlu display-list head may
    // still be a garbage pointer this early in boot -- in which case the
    // decoder reads junk and either finds no triangles or lands geometry
    // nowhere recognizable. That is expected and reported (blank/garbage),
    // not faked; the objective rasterizer proof lives in
    // fn64-render-rt64/tests/f3dex2_replay.rs, independent of this live path.
    let render_dump_start = std::env::var("OOT_RENDER_DUMP_START")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // NOTE: 240 (one full second of NTSC frames) so the capture reaches the
    // frames where real 3D geometry appears -- the first ~8 gfx tasks are
    // OoT's boot/logo screens (large flat gradient background quads), and
    // the recognizable projected geometry (rotating title object, then the
    // file-select 3D scene) shows up later in the boot sequence. A smaller
    // limit stops at the gradient logos and misses the geometry proof.
    // A common NTSC low-res target; matches capture_framebuffer's assumed
    // 320x240 (this harness does not yet decode the ROM's real OSViMode).
    use fn64_render::RenderBackend as _;
    let create_reference = || -> Box<dyn fn64_render::RenderBackend> {
        let mut backend = fn64_render_rt64::ReferenceBackend::new()
            .with_f3dex2()
            .with_clear_color([0, 0, 0, 255])
            .with_auto_dump("/tmp", "fn64-oot-render", 240)
            .with_auto_dump_skip(render_dump_start);
        backend
            .create(&fn64_render::RenderConfig::new(320, 240))
            .expect("ReferenceBackend create must be infallible for 320x240");
        Box::new(backend)
    };
    let requested_renderer = std::env::var("FN64_RENDER")
        .unwrap_or_else(|_| "reference".to_string())
        .to_ascii_lowercase();
    let (render_backend, active_renderer): (Box<dyn fn64_render::RenderBackend>, &'static str) =
        if requested_renderer == "rt64" {
            let mut backend = fn64_render_rt64::Rt64Backend::new();
            match backend.create(&fn64_render::RenderConfig::new(320, 240)) {
                Ok(()) => (Box::new(backend), "rt64"),
                Err(error) => {
                    eprintln!(
                        "[oot-boot] WARNING: RT64 create failed ({error}); falling back to the \
                         ReferenceBackend oracle"
                    );
                    (create_reference(), "reference-fallback")
                }
            }
        } else {
            if requested_renderer != "reference" {
                eprintln!(
                    "[oot-boot] WARNING: unknown FN64_RENDER={requested_renderer:?}; using \
                     ReferenceBackend"
                );
            }
            (create_reference(), "reference")
        };
    fn64_abi::set_render_backend(render_backend, rdram.len());
    println!(
        "[oot-boot] registered {active_renderer} renderer (320x240); reference/fallback \
         auto-dumps honor OOT_RENDER_DUMP_START={render_dump_start}"
    );

    wire_audio_output(rdram.len());

    // Register the REAL recompiled OoT aspMain audio ucode (typed Rust from
    // fn64-audio's clean-room RSP recompiler, compiled in the out-of-tree
    // `oot-audio-ucode` crate). This replaces the stand-in: M_AUDTASK
    // dispatch now actually runs the translated 1004-instruction ucode
    // against rdram. The ucode's FFI wrapper rebuilds a bounds-checked
    // `&mut [u8]` from the raw pointer, so it needs the rdram length first.
    #[cfg(feature = "oot-audio")]
    {
        oot_audio_ucode::set_rdram_len(rdram.len());
        unsafe { fn64_abi::set_audio_ucode_fn(oot_audio_ucode::oot_audio_ucode) };
        println!(
            "[oot-boot] registered recompiled OoT aspMain audio ucode (1004 instrs) \
             as the real M_AUDTASK ucode function"
        );
    }
    #[cfg(not(feature = "oot-audio"))]
    println!(
        "[oot-boot] oot-audio feature disabled; use FN64_SKIP_AUDIO_UCODE=1 for this boot probe"
    );

    println!("[oot-boot] booting thread 0 (recomp_entrypoint)...");
    #[cfg(fn64_recomp_rs)]
    {
        fn64_recomp_rs::set_host_lookup(Some(recompiled_or_host_lookup));
        println!(
            "[oot-boot] FN64_RECOMP=rs: linked oot-recompiled crate + host-first recompiled adapters active"
        );
        // SAFETY: `rdram` is the process-wide allocation and remains live
        // until every executor coroutine is dropped at process shutdown.
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

    // Drive boot for a bounded number of scheduling STEPS, not an unbounded
    // inner drain loop -- a thread that keeps calling pause_self (or any
    // other always-immediately-runnable yield) in a tight loop is a REAL,
    // legitimate boot state (rung 14's own precedent: an idle-thread self-
    // loop), and an inner `while run_one_step() {}` never returns in that
    // case, hanging the harness with no diagnostic. Every step is counted
    // against ONE shared budget and logged periodically so a genuine
    // infinite idle-spin is visible (many steps, sim_time barely advancing)
    // rather than silently indistinguishable from real progress.
    const MAX_STEPS: u64 = 2_000_000;
    const TICK_STEP: u64 = 100;
    const LOG_EVERY: u64 = 50_000;
    // Feedback-loop speedup: `OOT_MAX_SWAPS=N` stops the boot as soon as N VI
    // swaps have happened, instead of grinding the full 2M-step budget.
    // Proving the render path only needs the first few frames, so a render
    // iteration goes from ~minutes to ~seconds. `OOT_MAX_STEPS=N` raises the
    // executor budget for longer cutscenes without changing the default probe.
    // Unset = run to the full budget
    // (the durable boot-depth behavior). `OOT_STOP_ON_FRAME=1` additionally
    // stops the instant a NON-BLANK framebuffer is captured -- the exact
    // moment there's an image to eye-gate.
    let max_swaps: Option<u64> = std::env::var("OOT_MAX_SWAPS")
        .ok()
        .and_then(|s| s.parse().ok());
    let max_steps = std::env::var("OOT_MAX_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(MAX_STEPS);
    let stop_on_frame: bool = std::env::var("OOT_STOP_ON_FRAME").is_ok();
    // Performance probes must not include the harness's per-swap PNG encoder
    // and filesystem writes. Default capture behavior remains unchanged;
    // this flag affects diagnostics only, never guest execution or rendering.
    let perf_no_capture = std::env::var_os("OOT_PERF_NO_CAPTURE").is_some();

    // Scripted controller input (the INPUT-SEAM deliverable). `OOT_INPUT_SCRIPT`
    // is a comma-separated list of `frame:BUTTON[+BUTTON...][@stickX/stickY]`
    // steps, keyed by VI-swap count (one swap ~= one displayed frame). At each
    // step's frame the named buttons are HELD on port 0 until the next step (or
    // released by an empty step, e.g. `50:`). Button names match
    // controller.h's BTN_* (A,B,Z,START,DUP,DDOWN,DLEFT,DRIGHT,L,R,CUP,CDOWN,
    // CLEFT,CRIGHT). Example: `OOT_INPUT_SCRIPT=40:START,44:,60:START` presses
    // Start at frame 40, releases at 44, presses again at 60. The discovered
    // `OOT_SCRIPT_INTERACTIVE=1` preset drives OoT NTSC 1.0 through file
    // creation and its opening cutscenes, holding stick X=60 until Link can
    // move. Unset (or `OOT_SCRIPT_START=N`, a shorthand that taps Start once
    // at frame N) leaves the pad idle -- an honest un-driven boot.
    let input_script = build_input_script();
    if !input_script.is_empty() {
        println!(
            "[oot-boot] scripted input armed: {} step(s) -> {:?}",
            input_script.len(),
            input_script
                .iter()
                .map(|s| (s.frame, format!("{:#06x}", s.buttons)))
                .collect::<Vec<_>>()
        );
    }
    let mut next_script_idx = 0usize;
    let mut last_applied_pad = (0u16, 0i8, 0i8);
    // How many consecutive "nothing was runnable, and advancing the
    // virtual clock didn't wake anything either" ticks before concluding
    // boot has reached a genuinely idle steady state (not just a thread
    // temporarily blocked waiting for a soon-to-fire timer/retrace).
    const IDLE_TICKS_BEFORE_STOP: u32 = 200;
    let mut last_swap_count = 0u64;
    let mut fb_dumps = Vec::new();
    let mut thread0_death_logged = false;
    let mut consecutive_idle_ticks = 0u32;

    let mut tick = 0u64;
    let mut steps = 0u64;
    let swap_timing = std::env::var_os("OOT_SWAP_TIMING").is_some();
    let state_trace = std::env::var_os("OOT_STATE_TRACE").is_some();
    let mut last_oot_state = None;
    let mut file_select_state_offset = None;
    let mut last_file_select_state = None;
    let mut play_state_offset = None;
    let mut last_player_state = None;
    let mut last_control_state = None;
    let mut last_room_state = None;
    let mut last_swap_instant: Option<std::time::Instant> = None;
    loop {
        if steps >= max_steps {
            println!(
                "[oot-boot] step budget ({max_steps}) exhausted at sim_time={} -- stopping \
                 (this may mean a thread is spinning without truly blocking, or boot just needs \
                 a larger budget)",
                fn64_abi::sim_time()
            );
            break;
        }
        let stepped = fn64_abi::run_one_step();
        steps += 1;
        if steps.is_multiple_of(LOG_EVERY) {
            println!(
                "[oot-boot] progress: steps={steps} sim_time={} vi_swaps={} gfx_tasks={} \
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
        // real signal, handled by `stepped`/the idle-tick counter below.
        if !thread0_death_logged && fn64_abi::is_thread_dead(0) {
            println!(
                "[oot-boot] thread 0 (recomp_entrypoint) returned at step {steps} -- expected \
                 (its own initial call chain unwound); other threads keep running"
            );
            thread0_death_logged = true;
        }

        // Framebuffer capture: on every new osViSwapBuffer, hash+dump if
        // non-uniform (Task requirement 3).
        let swap_count = fn64_abi::vi_swap_count();
        if swap_count > last_swap_count {
            // Per-swap wall-clock timing (perf profiling), guarded by
            // OOT_SWAP_TIMING. Prints ms since the previous swap so a window
            // (e.g. swaps 233..240) can be averaged.
            if swap_timing {
                let now = std::time::Instant::now();
                if let Some(prev) = last_swap_instant {
                    let dt = now.duration_since(prev).as_secs_f64() * 1000.0;
                    println!("[oot-boot] SWAP_TIMING swap={swap_count} dt_ms={dt:.3}");
                }
                last_swap_instant = Some(now);
            }
            // Apply any scripted-input steps whose frame we've now reached.
            // Steps are frame-sorted; the last one at-or-before `swap_count`
            // wins (a HELD button stays until the next step changes it).
            while next_script_idx < input_script.len()
                && input_script[next_script_idx].frame <= swap_count
            {
                let step = &input_script[next_script_idx];
                fn64_abi::set_controller_state(0, step.buttons, step.stick_x, step.stick_y);
                let pad = (step.buttons, step.stick_x, step.stick_y);
                if pad != last_applied_pad {
                    println!(
                        "[oot-boot] SCRIPTED INPUT @ frame {swap_count}: port0 buttons={:#06x} \
                         stick=({},{})",
                        step.buttons, step.stick_x, step.stick_y
                    );
                }
                last_applied_pad = pad;
                next_script_idx += 1;
            }

            // Opt-in menu/gameplay frontier trace. OoT NTSC 1.0's linker map
            // places `gSaveContext` at 0x8011A5D0
            // (`refs/oot-decomp/build/ntsc-1.0/oot-ntsc-1.0.map:23819`), and
            // its public decomp layout puts `Save.entranceIndex`, `fileNum`,
            // and `gameMode` at offsets 0x0000, 0x1354, and 0x135C
            // (`refs/oot-decomp/include/save.h:270-284`). The generated
            // `MEM_W` contract is a native-endian word load, so read exactly
            // that representation here. Logging only changes makes long
            // scripted probes useful without changing guest behavior.
            if state_trace {
                const SAVE_CONTEXT: usize = 0x0011_A5D0;
                let state = (
                    read_guest_u32(&rdram, SAVE_CONTEXT),
                    read_guest_u32(&rdram, SAVE_CONTEXT + 0x1354),
                    read_guest_u32(&rdram, SAVE_CONTEXT + 0x135C),
                );
                if last_oot_state != Some(state) {
                    println!(
                        "[oot-boot] OOT STATE @ swap {swap_count}: entrance={:#010x} \
                         file_num={:#010x} game_mode={} ({}) file_select_overlay={:#010x}",
                        state.0,
                        state.1,
                        state.2,
                        match state.2 {
                            0 => "normal",
                            1 => "title",
                            2 => "file-select",
                            3 => "end-credits",
                            _ => "unknown",
                        },
                        read_guest_u32(&rdram, 0x000F_1340 + 5 * 0x30),
                    );
                    last_oot_state = Some(state);
                }

                // While file-select is resident, derive its relocated
                // `FileSelect_Main` state from GameStateOverlay.loadedRamAddr
                // (table entry 5), then find the one GameState whose `main`
                // field contains static vram 0x80811760. The generated C
                // stores that literal at ROM PCs 0x8081245C-0x8081246C
                // (`RecompiledFuncs/funcs_65.c:3291-3302`); function lookup
                // canonicalizes it only when called. GameState.main is +0x04
                // (`refs/oot-decomp/include/game.h:15-26`).
                if state.2 == 2 && file_select_state_offset.is_none() {
                    const GAMESTATE_OVERLAY_TABLE: usize = 0x000F_1340;
                    const FILE_SELECT_ENTRY: usize = GAMESTATE_OVERLAY_TABLE + 5 * 0x30;
                    const FILE_SELECT_MAIN: u32 = 0x8081_1760;
                    let loaded_base = read_guest_u32(&rdram, FILE_SELECT_ENTRY);
                    if loaded_base != 0 {
                        file_select_state_offset = find_guest_word(&rdram, FILE_SELECT_MAIN)
                            .and_then(|main_field| main_field.checked_sub(4));
                        if let Some(base) = file_select_state_offset {
                            println!(
                                "[oot-boot] OOT FILE-SELECT STATE located at rdram+{base:#x} \
                                 (overlay={loaded_base:#010x}, main={FILE_SELECT_MAIN:#010x})"
                            );
                        }
                    }
                }
                if let Some(base) = file_select_state_offset {
                    // NTSC 1.0's emitted field offsets are byte-grounded in
                    // FileSelect_UpdateMainMenu and FileSelect_InitContext.
                    // For example ROM PC 0x8080C2C8 forms base+0x10000 and
                    // PC 0x8080C2AC extends that base to +0x18000, and PC
                    // 0x8080C2CC reads buttonIndex at another +0x4A2A
                    // (`RecompiledFuncs/funcs_64.c:10212-10217`). These are
                    // 0xE below the current decomp header comments because
                    // NTSC omits the PAL-only objectMagSegment field.
                    let file_state = (
                        read_guest_u16(&rdram, base + 0x1CA2A), // buttonIndex
                        read_guest_u16(&rdram, base + 0x1CA2E), // menuMode
                        read_guest_u16(&rdram, base + 0x1CA30), // configMode
                        read_guest_u16(&rdram, base + 0x1CA36), // selectMode
                        read_guest_u16(&rdram, base + 0x1CABA), // kbdButton
                        read_guest_u16(&rdram, base + 0x1CAC2), // kbdX
                        read_guest_u16(&rdram, base + 0x1CAC4), // kbdY
                        read_guest_u16(&rdram, base + 0x1CAC6), // name length
                    );
                    if last_file_select_state != Some(file_state) {
                        println!(
                            "[oot-boot] OOT FILE-SELECT @ swap {swap_count}: button={} menu={} \
                             config={} select={} kbd_button={:#06x} kbd=({},{}) name_len={}",
                            file_state.0,
                            file_state.1,
                            file_state.2,
                            file_state.3,
                            file_state.4,
                            file_state.5,
                            file_state.6,
                            file_state.7,
                        );
                        last_file_select_state = Some(file_state);
                    }
                }

                // A normal-mode GameState whose main callback is Play_Main is
                // a live PlayState. The NTSC 1.0 symbol dump places Play_Main
                // at 0x8009CAC8 (`games/OOTU/syms/dump.toml:2022`), while the
                // generated Play_Init path loads the player pointer from
                // play+0x1C44 at ROM PC 0x8009AE38
                // (`RecompiledFuncs/funcs_37.c:6001`). Actor.world.pos is
                // +0x24 (`refs/oot-decomp/include/actor.h:187-200`). These
                // byte-grounded fields let a headless run prove that analog
                // input moved Link instead of merely surviving in Play_Main.
                const PLAY_MAIN: u32 = 0x8009_CAC8;
                if let Some(base) = play_state_offset {
                    if read_guest_u32(&rdram, base + 0x04) != PLAY_MAIN
                        || read_guest_u32(&rdram, base + 0x98) != 1
                    {
                        println!(
                            "[oot-boot] OOT PLAY STATE retired at swap {swap_count}: \
                             rdram+{base:#x} no longer has live Play_Main"
                        );
                        play_state_offset = None;
                        last_player_state = None;
                        last_control_state = None;
                        last_room_state = None;
                    }
                }
                if state.2 == 0 && play_state_offset.is_none() {
                    play_state_offset = find_guest_word(&rdram, PLAY_MAIN)
                        .and_then(|main_field| main_field.checked_sub(4));
                    if let Some(base) = play_state_offset {
                        println!(
                            "[oot-boot] OOT PLAY STATE located at rdram+{base:#x} \
                             (main={PLAY_MAIN:#010x})"
                        );
                    }
                }
                if let Some(base) = play_state_offset {
                    // CutsceneContext.state/curFrame are play+0x1D6C/+0x1D74
                    // (`refs/oot-decomp/include/play_state.h:72` and
                    // `include/cutscene.h:500-515`). MessageContext.textId/
                    // msgMode are play+0x103D0/+0x103DC
                    // (`include/play_state.h:76`, `include/message.h:136-168`).
                    let control_state = (
                        read_guest_u8(&rdram, base + 0x1D6C),
                        read_guest_u16(&rdram, base + 0x1D74),
                        read_guest_u16(&rdram, base + 0x103D0),
                        read_guest_u8(&rdram, base + 0x103DC),
                    );
                    if last_control_state.map(|last: (u8, u16, u16, u8)| {
                        (last.0, last.2, last.3)
                            != (control_state.0, control_state.2, control_state.3)
                    }) != Some(false)
                        || swap_count.is_multiple_of(100)
                    {
                        println!(
                            "[oot-boot] OOT CONTROL @ swap {swap_count}: cs_state={} \
                             cs_frame={} text_id={:#06x} msg_mode={}",
                            control_state.0, control_state.1, control_state.2, control_state.3,
                        );
                    }
                    last_control_state = Some(control_state);

                    // `Play_Init` passes play+0x11CBC to `Room_Init`
                    // (generated C PC 0x8009CE90-0x8009CEA0). The generated
                    // `Room_RequestNewRoom`/`Room_ProcessRoomRequest` path
                    // establishes the load invariant directly: +0x31 is the
                    // async-request flag, completion copies DMA destination
                    // +0x34 to current-room segment +0x0C, then
                    // `Scene_ExecuteCommands` populates the room-shape pointer
                    // at +0x08 (PCs 0x80080A54-0x80080BFC). `Room_Draw` skips
                    // the room only when +0x0C is null and otherwise dispatches
                    // through the shape type at *+0x08 (PCs
                    // 0x80080C50-0x80080C84). These generated-C fields let the
                    // opt-in trace distinguish an unloaded room from a renderer
                    // failure without importing game headers or content.
                    let room = base + 0x11CBC;
                    let room_state = (
                        read_guest_u8(&rdram, room) as i8,
                        read_guest_u32(&rdram, room + 0x08),
                        read_guest_u32(&rdram, room + 0x0C),
                        read_guest_u8(&rdram, room + 0x14) as i8,
                        read_guest_u32(&rdram, room + 0x1C),
                        read_guest_u32(&rdram, room + 0x20),
                        read_guest_u8(&rdram, room + 0x30),
                        read_guest_u8(&rdram, room + 0x31),
                        read_guest_u32(&rdram, room + 0x34),
                    );
                    if last_room_state != Some(room_state) || swap_count.is_multiple_of(100) {
                        println!(
                            "[oot-boot] OOT ROOM @ swap {swap_count}: cur={} shape={:#010x} \
                             segment={:#010x} prev={} prev_shape={:#010x} \
                             prev_segment={:#010x} buffer={} load_active={} dma_dest={:#010x}",
                            room_state.0,
                            room_state.1,
                            room_state.2,
                            room_state.3,
                            room_state.4,
                            room_state.5,
                            room_state.6,
                            room_state.7,
                            room_state.8,
                        );
                        last_room_state = Some(room_state);
                    }

                    let player_vram = read_guest_u32(&rdram, base + 0x1C44);
                    let player_offset = (player_vram & 0x1FFF_FFFF) as usize;
                    if player_vram != 0 && player_offset + 0x30 <= rdram.len() {
                        let player_state = (
                            read_guest_u16(&rdram, base + 0xA4),
                            player_vram,
                            read_guest_u32(&rdram, player_offset + 0x24),
                            read_guest_u32(&rdram, player_offset + 0x28),
                            read_guest_u32(&rdram, player_offset + 0x2C),
                        );
                        let moved =
                            last_player_state.is_none_or(|last: (u16, u32, u32, u32, u32)| {
                                let dx = f32::from_bits(player_state.2) - f32::from_bits(last.2);
                                let dy = f32::from_bits(player_state.3) - f32::from_bits(last.3);
                                let dz = f32::from_bits(player_state.4) - f32::from_bits(last.4);
                                dx * dx + dy * dy + dz * dz >= 1.0
                            });
                        if moved || swap_count.is_multiple_of(100) {
                            println!(
                                "[oot-boot] OOT PLAYER @ swap {swap_count}: scene={} \
                                 actor={:#010x} pos=({:.3},{:.3},{:.3})",
                                player_state.0,
                                player_state.1,
                                f32::from_bits(player_state.2),
                                f32::from_bits(player_state.3),
                                f32::from_bits(player_state.4),
                            );
                            last_player_state = Some(player_state);
                        }
                    }
                }
            }

            let dumps_before = fb_dumps.len();
            if !perf_no_capture && swap_count >= render_dump_start {
                if let Some(fb_offset) = fn64_abi::current_vi_framebuffer() {
                    capture_framebuffer(&rdram, fb_offset, swap_count, &mut fb_dumps);
                }
            }
            last_swap_count = swap_count;
            // Early-exit hooks (feedback-loop speedup): stop the instant we
            // have what a render iteration needs, instead of the full budget.
            if stop_on_frame && fb_dumps.len() > dumps_before {
                println!(
                    "[oot-boot] OOT_STOP_ON_FRAME: captured a non-blank framebuffer at swap \
                     #{swap_count} (step {steps}) -- stopping early"
                );
                break;
            }
            if let Some(cap) = max_swaps {
                if swap_count >= cap {
                    println!(
                        "[oot-boot] OOT_MAX_SWAPS={cap}: reached swap #{swap_count} (step {steps}) \
                         -- stopping early"
                    );
                    break;
                }
            }
        }

        if !stepped {
            // Nothing was runnable -- host-driven progress (VI retrace,
            // due timers) is the only way forward.
            tick += TICK_STEP;
            fn64_abi::advance_virtual_time(tick);
            consecutive_idle_ticks += 1;
            if consecutive_idle_ticks >= IDLE_TICKS_BEFORE_STOP {
                println!(
                    "[oot-boot] reached a steady idle state ({IDLE_TICKS_BEFORE_STOP} \
                     consecutive ticks with nothing runnable) at sim_time={} steps={steps} -- \
                     stopping",
                    fn64_abi::sim_time()
                );
                break;
            }
        } else {
            consecutive_idle_ticks = 0;
            // A voluntary `pause_self` idle loop is runnable by definition,
            // so waiting for `run_one_step == false` would starve host time
            // forever. Pace the same single-threaded executor every 100
            // scheduling steps; this is host clock injection, not a second
            // game/executor thread (docs/DESIGN.md's one-runnable-token rule).
            if steps.is_multiple_of(100) {
                tick += TICK_STEP;
                fn64_abi::advance_virtual_time(tick);
            }
        }
    }

    let (gfx_count, audio_count) = fn64_abi::task_counts();
    println!("[oot-boot] === BOOT SUMMARY ===");
    println!("[oot-boot] virtual ticks run: {}", fn64_abi::sim_time());
    println!("[oot-boot] thread 0 dead: {}", fn64_abi::is_thread_dead(0));
    println!(
        "[oot-boot] VI swaps observed: {}",
        fn64_abi::vi_swap_count()
    );
    println!("[oot-boot] gfx tasks submitted: {gfx_count}");
    println!("[oot-boot] audio tasks submitted: {audio_count}");
    // R5 probe 3: the guest's VI retrace rate. NTSC wants ~60 Hz. Materially
    // above it means the ticker over-delivers, which would explain BOTH R5
    // symptoms at once (the audio thread produces per retrace -> ring pegs ->
    // static; game logic advances per retrace -> over-speed). Headless runs
    // flat out, so a high number HERE only means virtual time outruns wall
    // time unpaced -- the number that judges the shipped product is the
    // shell's, which paces its pump on wall-clock.
    match fn64_abi::retrace_cadence() {
        Some((ticks, secs, hz)) => println!(
            "[oot-boot] VI retrace cadence: {ticks} ticks in {secs:.3}s = {hz:.1} Hz \
             (NTSC target ~60; unpaced headless runs are expected to exceed it)"
        ),
        None => println!("[oot-boot] VI retrace cadence: no ticks fired (ticker never armed?)"),
    }
    println!("[oot-boot] active renderer: {active_renderer}");
    if let Some(error) = fn64_abi::last_render_error() {
        println!("[oot-boot] last render error: {error}");
    }
    {
        let (ns, calls) = fn64_abi::audio_ucode_timing();
        if calls > 0 {
            println!(
                "[oot-boot] audio ucode timing: {calls} calls, {:.2} ms total, {:.3} ms/call avg",
                ns as f64 / 1e6,
                (ns as f64 / 1e6) / calls as f64
            );
        }
    }
    let audio = fn64_abi::audio_output_stats();
    println!(
        "[oot-boot] AI audio output: {} buffers / {} samples ({} nonzero), range {:?}..={:?}; {} buffers reached AudioBackend",
        audio.ai_buffers,
        audio.samples,
        audio.nonzero_samples,
        audio.min,
        audio.max,
        audio.backend_buffers,
    );
    if let Some(error) = fn64_abi::last_audio_error() {
        println!("[oot-boot] audio backend last error: {error}");
    }
    {
        let timing = fn64_abi::phase_timing();
        if timing.executor_calls > 0 {
            let residual_ns = timing
                .executor_ns
                .saturating_sub(timing.gfx_ns)
                .saturating_sub(timing.audio_dispatch_ns);
            println!(
                "[oot-boot] phase timing: executor={:.3} ms/{} calls, gfx={:.3} ms/{} tasks, \
                 audio_dispatch={:.3} ms/{} tasks, cpu+executor_residual={:.3} ms",
                timing.executor_ns as f64 / 1e6,
                timing.executor_calls,
                timing.gfx_ns as f64 / 1e6,
                timing.gfx_calls,
                timing.audio_dispatch_ns as f64 / 1e6,
                timing.audio_dispatch_calls,
                residual_ns as f64 / 1e6,
            );
        }
    }
    println!(
        "[oot-boot] non-uniform framebuffers dumped: {} ({:?})",
        fb_dumps.len(),
        fb_dumps
    );

    if trace_enabled {
        let trace = fn64_abi::copy_trace();
        println!("[oot-boot] trace events recorded: {}", trace.len());
        write_trace_file(&trace, TRACE_PATH);
        println!("[oot-boot] trace written to {TRACE_PATH}");
    }

    // Recompiled threads may be suspended inside an existing `extern "C"`
    // ABI shim (most commonly blocking osRecvMesg) in BOTH lanes: the C lane
    // aborted with exit 134 in `osRecvMesg_recomp` during TLS teardown the
    // same way the rs lane once did. Rust TLS teardown would make corosensei
    // force-unwind that stack across the non-unwind FFI boundary and abort
    // after an otherwise complete bounded probe. All diagnostic state is
    // explicitly flushed above, so terminate the harness process without
    // running that invalid coroutine destructor — exit code 0 is then a
    // trustworthy probe-success signal for both lanes.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: `_exit` has no memory contract; unlike `exit`, it skips C
    // atexit/TLS destructors. That distinction is the purpose here.
    unsafe { libc::_exit(0) }
}

fn read_guest_u32(rdram: &[u8], offset: usize) -> u32 {
    let bytes: [u8; 4] = rdram[offset..offset + 4]
        .try_into()
        .expect("OoT state trace address must fit the shared RDRAM buffer");
    u32::from_ne_bytes(bytes)
}

fn read_guest_u16(rdram: &[u8], offset: usize) -> u16 {
    let physical = offset ^ 2;
    let bytes: [u8; 2] = rdram[physical..physical + 2]
        .try_into()
        .expect("OoT state trace address must fit the shared RDRAM buffer");
    u16::from_ne_bytes(bytes)
}

fn read_guest_u8(rdram: &[u8], offset: usize) -> u8 {
    rdram[offset ^ 3]
}

fn find_guest_word(rdram: &[u8], needle: u32) -> Option<usize> {
    rdram[..8 * 1024 * 1024]
        .chunks_exact(4)
        .enumerate()
        .find_map(|(word_index, bytes)| {
            let main_field = word_index * 4;
            let base = main_field.checked_sub(4)?;
            let value = u32::from_ne_bytes(bytes.try_into().expect("four-byte chunk"));
            (value == needle && read_guest_u32(rdram, base + 0x98) == 1).then_some(main_field)
        })
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
            "[oot-boot] swap #{swap_index}: framebuffer offset {fb_offset:#x} + assumed size \
             {FB_BYTES:#x} exceeds rdram bounds ({} bytes) -- skipping capture, not guessing a \
             smaller region",
            rdram.len()
        );
        return;
    }
    // The RGBA5551 decode below reads each pixel through the rdram word swizzle
    // `(2*i) ^ 2`, which is only correct when the framebuffer base is
    // word-aligned (the swizzle is relative to a 32-bit word boundary). N64
    // framebuffers are DMA/cache-line aligned so this always holds; assert it
    // rather than silently mis-decode if a future fb_offset breaks the rule.
    debug_assert_eq!(start % 4, 0, "framebuffer offset {start:#x} not word-aligned; swizzle decode would be wrong");
    let region = &rdram[start..end];
    let first_byte = region[0];
    let uniform = region.iter().all(|&b| b == first_byte);

    if uniform {
        println!(
            "[oot-boot] swap #{swap_index}: framebuffer at {fb_offset:#010x} is UNIFORM \
             (all bytes == {first_byte:#04x}) -- reported as blank, not dumped (per task: \"a \
             blank/uniform fb is reported as blank\")."
        );
        return;
    }

    println!(
        "[oot-boot] swap #{swap_index}: framebuffer at {fb_offset:#010x} is NON-UNIFORM -- \
         dumping PNG."
    );
    let path = format!("/tmp/fn64-fb-{swap_index}.png");
    match dump_rgba5551_as_png(region, FB_WIDTH, FB_HEIGHT, &path) {
        Ok(()) => {
            println!("[oot-boot] *** NON-UNIFORM FRAMEBUFFER DUMPED: {path} ***");
            dumps.push(path);
        }
        Err(e) => eprintln!("[oot-boot] failed to write {path}: {e}"),
    }
}

/// Convert N64 RGBA5551 (RRRRRGGGGGBBBBBA halfwords) to RGBA8888 and write a
/// minimal, dependency-free PNG (a hand-rolled uncompressed-DEFLATE encoder --
/// no `png`/`image` crate dependency for this one-shot dump, keeping this
/// example's dependency footprint at just `cc` for the C bridge).
///
/// `data` is a slice of fn64's rdram, which is native-endian-**word** storage
/// (see fn64-recomp-rs runtime.rs): a halfword at logical byte offset `o` lives
/// at the swizzled offset `o ^ 2` and is read native-endian. Pixel `i` is the
/// halfword at logical offset `2*i`, so its two bytes are at `(2*i) ^ 2`.
/// Reading sequentially with `from_be_bytes` (no swizzle) scrambles the
/// halfword pairs within each 32-bit word and shifts every color -- that was
/// the purple/dithered cast in the captured frames.
fn dump_rgba5551_as_png(
    data: &[u8],
    width: usize,
    height: usize,
    path: &str,
) -> std::io::Result<()> {
    let mut rgba = Vec::with_capacity(width * height * 4);
    for i in 0..width * height {
        let p = (i * 2) ^ 2;
        let px = u16::from_ne_bytes([data[p], data[p + 1]]);
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
            eprintln!("[oot-boot] failed to create trace file {path}: {e}");
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
    use super::*;

    #[test]
    fn interactive_script_contains_verified_menu_route_and_held_motion() {
        let steps = interactive_input_script();
        assert_eq!(steps.len(), 295);
        assert!(steps.windows(2).all(|pair| pair[0].frame <= pair[1].frame));

        let at = |frame| {
            steps
                .iter()
                .find(|step| step.frame == frame)
                .expect("verified script frame")
        };
        assert_eq!(at(250).buttons, button_bit("START"));
        assert_eq!(at(360).buttons, button_bit("A"));
        assert_eq!(at(420).buttons, button_bit("START"));
        assert_eq!(at(540).buttons, button_bit("A"));
        assert_eq!((at(620).stick_x, at(620).stick_y), (60, 0));
        assert_eq!((at(4_152).buttons, at(4_152).stick_x), (0, 60));
    }
}
