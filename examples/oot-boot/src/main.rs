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
//! 2. Registers every section from the real, out-of-tree-compiled
//!    `recomp_overlays.inl` (via `bridge/section_bridge.c`'s FFI walk into
//!    `fn64_register_func`, below) with `fn64_abi::register_section`, then
//!    marks the always-resident sections (0/1/2 -- makerom.ent/boot/code,
//!    per OoT's OWN linker `.map`: everything before the 469 `ovl_*` actor/
//!    scene overlays, which are heap-loaded on demand via DmaMgr at
//!    runtime, NOT pre-mapped at boot -- see games/OOTU/profile.toml's
//!    `[segments]` section) loaded.
//! 3. Boots thread 0 running `recomp_entrypoint` (the real, linked
//!    generated symbol) and drives the executor: `run_one_step` while
//!    runnable, `advance_virtual_time` (which fires the armed VI retrace
//!    ticker) when idle, for a bounded number of virtual-time ticks.
//! 4. On every `osViSwapBuffer_recomp` call (observed via
//!    `fn64_abi::vi_swap_count()` polling), hashes the pointed-to
//!    framebuffer region and dumps it as a PNG if non-uniform.
//! 5. Emits the trace log to a file and prints a summary ladder.

use std::collections::HashMap;
use std::io::Write;

// ---------------------------------------------------------------------
// FFI surface into the out-of-tree-compiled bridge (bridge/section_bridge.c)
// and the real generated recomp_entrypoint symbol.
// ---------------------------------------------------------------------

extern "C" {
    /// Walks the real, compiled-in `section_table[]`/`FuncEntry[]` (from
    /// the game's own `recomp_overlays.inl`) and calls `fn64_register_func`
    /// once per function -- see `bridge/section_bridge.c`.
    fn fn64_bridge_register_all_sections();
    fn fn64_bridge_num_sections() -> usize;

    /// The real generated boot entry point (`RecompiledFuncs/funcs_0.c`).
    fn recomp_entrypoint(rdram: *mut u8, ctx: *mut fn64_abi::RecompContext);
}

/// Called from C (`section_bridge.c`'s walk) once per `(section, func)`
/// pair, in registration order. Buffers into `SECTION_BUILDER` (this
/// process's one accumulator) rather than calling
/// `fn64_abi::register_section` directly per-func, since that API takes a
/// whole section's func list at once (matching `SectionRegistry`'s
/// batch-registration contract, `fn64-runtime/src/overlay.rs`).
#[no_mangle]
extern "C" fn fn64_register_func(
    section_index: usize,
    rom_addr: u32,
    ram_addr: u32,
    size: u32,
    offset: u32,
    rom_size: u32,
    func: fn64_abi::RecompFunc,
) {
    SECTION_BUILDER.with(|cell| {
        let mut builder = cell.borrow_mut();
        let entry = builder
            .sections
            .entry(section_index)
            .or_insert_with(|| (rom_addr, ram_addr, size, Vec::new()));
        entry.3.push((offset, rom_size, func));
    });
}

#[derive(Default)]
struct SectionBuilder {
    /// section_index -> (rom_addr, ram_addr, size, funcs)
    sections: HashMap<usize, (u32, u32, u32, Vec<(u32, u32, fn64_abi::RecompFunc)>)>,
}

thread_local! {
    static SECTION_BUILDER: std::cell::RefCell<SectionBuilder> =
        std::cell::RefCell::new(SectionBuilder::default());
}

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

fn main() {
    let rom_path = env_path("ROM");
    println!("[oot-boot] loading ROM from {}", rom_path.display());
    let rom_bytes = std::fs::read(&rom_path).unwrap_or_else(|e| {
        panic!(
            "oot-boot: failed to read ROM {}: {e}",
            rom_path.display()
        )
    });
    println!("[oot-boot] ROM size: {} bytes", rom_bytes.len());
    fn64_abi::load_rom(rom_bytes);

    // Register OoT's save-backing store so domain-2 (SRAM, devAddr >=
    // 0x08000000 / PI_DOM2_ADDR2) PI DMAs have somewhere to go instead of
    // being (wrongly) read from the ROM image past its end. OoT uses banked
    // SRAM (32 KiB); Sram_InitSram DMAs the whole 0x8000-byte save in at boot
    // (funcs_34.c:10636). Ephemeral in-memory store for this boot harness --
    // a persisted FileSaveStorage is a shell concern, not this bring-up's.
    fn64_abi::set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
        fn64_runtime::SaveType::SramBanked,
    )));

    // Register every section from the real recomp_overlays.inl via the
    // bridge's C-side walk (populates SECTION_BUILDER via callbacks).
    unsafe { fn64_bridge_register_all_sections() };
    let num_sections = unsafe { fn64_bridge_num_sections() };
    println!("[oot-boot] bridge reports {num_sections} sections in recomp_overlays.inl");

    let mut section_indices: HashMap<usize, fn64_runtime::SectionIndex> = HashMap::new();
    SECTION_BUILDER.with(|cell| {
        let builder = cell.borrow();
        let mut keys: Vec<_> = builder.sections.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let (rom_addr, ram_addr, size, funcs) = &builder.sections[&key];
            let idx = unsafe { fn64_abi::register_section(*rom_addr, *ram_addr, *size, funcs) };
            section_indices.insert(key, idx);
            println!(
                "[oot-boot] registered section {key}: rom={rom_addr:#010x} ram={ram_addr:#010x} \
                 size={size:#x} funcs={}",
                funcs.len()
            );
        }
    });

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
    for section_key in [0usize, 1usize, 2usize] {
        if let Some(&idx) = section_indices.get(&section_key) {
            fn64_abi::set_section_loaded(idx);
            println!("[oot-boot] marked section {section_key} (index {idx}) loaded");
        }
    }

    // Real plumbing, stand-in body (see stand_in_audio_ucode's doc comment).
    unsafe { fn64_abi::set_audio_ucode_fn(stand_in_audio_ucode) };

    // VI retrace: arm a host-chosen approximation (fn64_runtime::vi's doc:
    // not a hardware-accurate NTSC/PAL constant). 1000 virtual-time units
    // per field is an arbitrary but documented choice for this harness.
    fn64_abi::arm_vi_retrace(1000);

    // Arm crash-safe incremental trace flushing BEFORE booting thread 0 --
    // a SIGSEGV mid-boot (as rung 3 hit) must not lose the whole session's
    // trace; every event from here on is appended+flushed to disk as it's
    // recorded, not just buffered for the end-of-run `write_trace_file`
    // call below (which still runs too, on a clean exit, and rewrites the
    // same path from the in-memory copy -- harmless, since by then the
    // incremental sink already has every event that copy will contain).
    const TRACE_PATH: &str = "/tmp/oot-boot-trace.jsonl";
    if let Err(e) = fn64_abi::set_trace_sink_file(TRACE_PATH) {
        eprintln!(
            "[oot-boot] WARNING: failed to arm incremental trace sink at {TRACE_PATH}: {e} -- \
             a crash mid-boot will lose the trace (falling back to end-of-run-only)."
        );
    } else {
        println!("[oot-boot] incremental trace sink armed at {TRACE_PATH}");
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
    const RDRAM_SIZE: usize = 8 * 1024 * 1024;
    let mut rdram = vec![0u8; RDRAM_SIZE.max(fn64_runtime::RDRAM_MMIO_WINDOW_END as usize)];
    let rdram_ptr = rdram.as_mut_ptr();

    // Register the headless reference software rasterizer as the render
    // backend BEFORE booting thread 0, so every M_GFXTASK the game submits
    // via osSpTaskYielded actually decodes+rasterizes (the ABI's
    // GFX_RENDER_NOTE path) instead of being counted-and-dropped. It decodes
    // real F3DEX2 (OoT's ucode family) and auto-dumps each non-clear
    // rasterized frame to /tmp/fn64-oot-render-*.png -- the harness's only
    // way to see the backend's output, since set_render_backend takes
    // ownership of the trait object (which is deliberately not
    // Any-downcastable, per docs/DECOUPLING.md). rdram_len MUST match the
    // real backing buffer so the backend's bounds checks and the ABI's
    // from_raw_parts slice length agree; we pass rdram.len() (which includes
    // the RDRAM_MMIO_WINDOW_END headroom above) for exactly that reason.
    //
    // NOTE (honest): the CONCURRENT display-list-pointer fix has not
    // necessarily landed, so OoT's live polyOpa/polyXlu display-list head may
    // still be a garbage pointer this early in boot -- in which case the
    // decoder reads junk and either finds no triangles or lands geometry
    // nowhere recognizable. That is expected and reported (blank/garbage),
    // not faked; the objective rasterizer proof lives in
    // fn64-render-rt64/tests/f3dex2_replay.rs, independent of this live path.
    let mut render_backend = fn64_render_rt64::ReferenceBackend::new()
        .with_f3dex2()
        .with_clear_color([0, 0, 0, 255])
        .with_auto_dump("/tmp", "fn64-oot-render", 8);
    // A common NTSC low-res target; matches capture_framebuffer's assumed
    // 320x240 (this harness does not yet decode the ROM's real OSViMode).
    {
        use fn64_render::RenderBackend as _;
        if let Err(e) = render_backend.create(&fn64_render::RenderConfig::new(320, 240)) {
            eprintln!("[oot-boot] WARNING: render backend create() failed: {e}");
        }
    }
    fn64_abi::set_render_backend(Box::new(render_backend), rdram.len());
    println!(
        "[oot-boot] registered fn64-render-rt64 ReferenceBackend (F3DEX2, 320x240, \
         auto-dump /tmp/fn64-oot-render-*.png) as the render backend"
    );

    println!("[oot-boot] booting thread 0 (recomp_entrypoint)...");
    unsafe {
        fn64_abi::boot_thread0(rdram_ptr, recomp_entrypoint, 0, 10);
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
    loop {
        if steps >= MAX_STEPS {
            println!(
                "[oot-boot] step budget ({MAX_STEPS}) exhausted at sim_time={} -- stopping \
                 (this may mean a thread is spinning without truly blocking, or boot just needs \
                 a larger budget)",
                fn64_abi::sim_time()
            );
            break;
        }
        let stepped = fn64_abi::run_one_step();
        steps += 1;
        if steps % LOG_EVERY == 0 {
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
            if let Some(fb_offset) = fn64_abi::current_vi_framebuffer() {
                capture_framebuffer(&rdram, fb_offset, swap_count, &mut fb_dumps);
            }
            last_swap_count = swap_count;
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
        }
    }

    let (gfx_count, audio_count) = fn64_abi::task_counts();
    println!("[oot-boot] === BOOT SUMMARY ===");
    println!("[oot-boot] virtual ticks run: {}", fn64_abi::sim_time());
    println!(
        "[oot-boot] thread 0 dead: {}",
        fn64_abi::is_thread_dead(0)
    );
    println!(
        "[oot-boot] VI swaps observed: {}",
        fn64_abi::vi_swap_count()
    );
    println!("[oot-boot] gfx tasks submitted: {gfx_count}");
    println!("[oot-boot] audio tasks submitted: {audio_count}");
    println!(
        "[oot-boot] non-uniform framebuffers dumped: {} ({:?})",
        fb_dumps.len(),
        fb_dumps
    );

    let trace = fn64_abi::copy_trace();
    println!("[oot-boot] trace events recorded: {}", trace.len());
    write_trace_file(&trace, TRACE_PATH);
    println!("[oot-boot] trace written to {TRACE_PATH}");
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

/// Convert N64 RGBA5551 (big-endian halfwords: RRRRRGGGGGBBBBBA) to
/// RGBA8888 and write a minimal, dependency-free PNG (a hand-rolled
/// uncompressed-DEFLATE encoder -- no `png`/`image` crate dependency for
/// this one-shot dump, keeping this example's dependency footprint at just
/// `cc` for the C bridge).
fn dump_rgba5551_as_png(
    data: &[u8],
    width: usize,
    height: usize,
    path: &str,
) -> std::io::Result<()> {
    let mut rgba = Vec::with_capacity(width * height * 4);
    for chunk in data.chunks_exact(2) {
        let px = u16::from_be_bytes([chunk[0], chunk[1]]);
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
