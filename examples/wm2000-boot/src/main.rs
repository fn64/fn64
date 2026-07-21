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
//! 4. On every `osViSwapBuffer_recomp` call (observed via
//!    `fn64_abi::vi_swap_count()` polling), hashes the pointed-to
//!    framebuffer region and dumps it as a PNG if non-uniform (Task
//!    requirement 3).
//! 5. Emits the trace log to a file and prints a summary ladder.

use std::io::Write;

/// Real translated audio ucode stand-in. The GENUINE
/// `wm2000_audio_ucode` (RSPRecomp-generated, `aki-recomp/games/NWXE/rsp/
/// wm2000_audio.cpp`) `#include`s `librecomp/rsp.hpp`, which lives under
/// `N64ModernRuntime`'s GPL-3.0-licensed tree (verified directly: that
/// repo's top-level `COPYING` is GPL-3.0; `librecomp/rsp.hpp` is NOT under
/// the MIT-carved-out `N64Recomp/` subdirectory) -- linking it into this
/// MIT/Apache-2.0 example would violate `fn64/AGENTS.md`'s clean-room
/// protocol ("Disallowed: reading GPL runtime implementation code... Not
/// for 'inspiration'"). This is a REAL FINDING, reported honestly rather
/// than routed around: RSPRecomp's own codegen template (verified in
/// `N64RecompSource/RSPRecomp/src/rsp_recomp.cpp:1179`) unconditionally
/// emits `#include "librecomp/rsp.hpp"` into every ucode it generates, so
/// this is not a per-game choice -- using RSPRecomp's generated C at all
/// requires the GPL runtime header AS SHIPPED.
///
/// Until fn64 has its own MIT-clean RSP interpreter (or a from-scratch fork
/// of RSPRecomp's codegen template targeting fn64-owned headers), this
/// stand-in exercises the REAL plumbing this wave built
/// (`fn64_abi::set_audio_ucode_fn`/`osSpTaskYielded_recomp`'s M_AUDTASK
/// dispatch) without linking the disallowed dependency. It is clearly
/// NOT the real ucode -- it does nothing to rdram, just proves the call
/// happened.
unsafe extern "C" fn stand_in_audio_ucode(_rdram: *mut u8, ucode_addr: u32) -> u32 {
    eprintln!(
        "[wm2000-boot] STAND-IN audio ucode invoked for ucode_addr={ucode_addr:#010x} -- NOT the \
         real wm2000_audio_ucode (see main.rs's doc comment: the real one requires the GPL-3.0 \
         librecomp/rsp.hpp header, disallowed by AGENTS.md's clean-room protocol). Plumbing is \
         real; ucode execution is not."
    );
    0
}

fn env_path(name: &str) -> std::path::PathBuf {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("wm2000-boot: required environment variable {name} not set"))
        .into()
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

    // Real plumbing, stand-in body (see stand_in_audio_ucode's doc comment).
    unsafe { fn64_abi::set_audio_ucode_fn(stand_in_audio_ucode) };

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

    // Main's VI pump now delivers real presentations; without a registered
    // backend the first retrace trips present_render_backend's loud trap.
    // The software ReferenceBackend keeps this harness headless.
    // ponytail: no RT64/env switch like oot-boot until a wm2000 frame exists.
    {
        use fn64_render::RenderBackend as _;
        let mut backend = fn64_render_reference::ReferenceBackend::new()
            .with_f3dex2()
            .with_clear_color([0, 0, 0, 255])
            .with_auto_dump("/tmp", "fn64-wm2000-render", 240);
        backend
            .create(&fn64_render::RenderConfig::for_tv(320, 240, tv_type))
            .expect("ReferenceBackend create must be infallible for 320x240");
        fn64_abi::set_render_backend(Box::new(backend), rdram.len());
    }

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
    const TRACE_PATH: &str = "/tmp/wm2000-boot-trace.jsonl";
    if let Err(e) = fn64_abi::set_trace_sink_file(TRACE_PATH) {
        eprintln!(
            "[wm2000-boot] WARNING: failed to arm incremental trace sink at {TRACE_PATH}: {e} -- \
             a crash mid-boot will lose the trace (falling back to end-of-run-only)."
        );
    } else {
        println!("[wm2000-boot] incremental trace sink armed at {TRACE_PATH}");
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
    const MAX_STEPS: u64 = 2_000_000;
    const LOG_EVERY: u64 = 50_000;
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
    let mut drain = fn64_boot_harness::GuestDrain::default();
    loop {
        if steps >= MAX_STEPS {
            println!(
                "[wm2000-boot] step budget ({MAX_STEPS}) exhausted at sim_time={} -- stopping \
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
        if advanced_field {
            let next_field = tick
                + fn64_abi::vi_field_interval()
                    .expect("typed television standard must keep VI armed");
            // Device completions land between fields: a guest slice charges
            // (almost) no virtual time in the C lane, so a DMA issued
            // mid-slice arms its deadline just past sim_time. Jumping a
            // whole field would deliver EVERY completion a field late and
            // break real issue-then-poll-next-frame guest pipelines
            // (BOOT-NOTES-WM2000.md part 7: NWXE's joybus). Service any
            // deadline due before the next field boundary first.
            let device_deadline = fn64_abi::next_device_deadline()
                .filter(|deadline| *deadline < next_field);
            match device_deadline {
                Some(deadline) => {
                    // Service the earlier hardware event, but do NOT reset
                    // the field target: a chattering device (e.g. degenerate
                    // audio refeed) must never starve the VI tick.
                    fn64_abi::advance_virtual_time(deadline.max(fn64_abi::sim_time()));
                }
                None => {
                    tick = next_field;
                    fn64_abi::advance_virtual_time(tick);
                    drain.begin_field();
                }
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
            if let Some(fb_offset) = fn64_abi::current_vi_framebuffer() {
                capture_framebuffer(&rdram, fb_offset, swap_count, &mut fb_dumps);
            }
            last_swap_count = swap_count;
        }

        if advanced_field {
            if fn64_abi::next_runnable_priority().is_none() {
                consecutive_idle_ticks += 1;
            } else {
                consecutive_idle_ticks = 0;
            }
            if consecutive_idle_ticks >= IDLE_TICKS_BEFORE_STOP {
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
    println!(
        "[wm2000-boot] non-uniform framebuffers dumped: {} ({:?})",
        fb_dumps.len(),
        fb_dumps
    );

    let trace = fn64_abi::copy_trace();
    println!("[wm2000-boot] trace events recorded: {}", trace.len());
    write_trace_file(&trace, TRACE_PATH);
    println!("[wm2000-boot] trace written to {TRACE_PATH}");
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
    let path = format!("/tmp/fn64-fb-{swap_index}.png");
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
