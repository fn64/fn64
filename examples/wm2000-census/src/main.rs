//! WM2000 (NWXE) headless GBI/RDP opcode-census harness on fn64. See
//! `build.rs`'s module doc for the `RECOMPILED_DIR`/`RECOMP_H_DIR`/`ROM`
//! env-var contract this binary requires -- this crate itself contains zero
//! game content, per `fn64/README.md`.
//!
//! ## What this is for
//!
//! `docs/RT64-WM2000-CENSUS.md` is produced by running this binary with
//! `FN64_GBI_CENSUS=1`. It boots the real recompiled game against the
//! in-repo software `ReferenceBackend`, which decodes every graphics task
//! the guest submits, and `fn64_render_reference::gbi::census` counts each
//! dispatched command byte on the way past. The census is re-runnable after
//! each `fn64-render-wgpu` slice lands, so the ADMITTED fraction in that doc
//! is a burndown number rather than a one-time measurement.
//!
//! Derived from the `wm2000-boot` harness this repo previously carried
//! out-of-tree; the boot sequence below is unchanged from it, because a
//! census of a different boot would be a census of a different game.
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
//!    framebuffer region and dumps it as a PNG if non-uniform.
//! 5. Emits the trace log to a file, prints a summary ladder, and writes the
//!    opcode census (`FN64_GBI_CENSUS_OUT`, else stderr).
//!
//! ## Knobs
//!
//! - `WM2000_MAX_STEPS` -- scheduler-step budget (default 20,000,000).
//! - `FN64_GBI_CENSUS=1` -- arm the census.
//! - `FN64_GBI_CENSUS_OUT=<path>` -- write the census TSV there.
//! - `FN64_GBI_TEXRECT_CENSUS=1` -- arm the per-`G_TEXRECT` cycle-mode probe.
//! - `FN64_GBI_TEXRECT_CENSUS_OUT=<path>` -- write that probe's TSV there.
//! - `FN64_GBI_PACKET_DUMP=1` -- arm the raw RDP command-word dump.
//! - `FN64_GBI_PACKET_DUMP_ENTRIES=<list>` -- comma-separated zero-based
//!   decode entries to dump (default: entry 0 alone).
//! - `FN64_GBI_PACKET_DUMP_OUT=<path>` -- write the dump's TSV there.

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
/// SUPERSEDED (2026-07-21) by the clean-room RSP LLE replay: `fn64_abi::
/// set_audio_task_lle(true)` (below) routes every `M_AUDTASK` through the
/// same `fn64_audio::rsp` interpreter path non-catalog gfx tasks already
/// take, executing the REAL in-ROM audio ucode instruction by instruction
/// against guest RDRAM -- no GPL header involved, no stand-in needed. Kept
/// (unregistered) so the licensing finding above stays attached to working
/// code demonstrating the registered-function seam's shape.
#[allow(dead_code)]
unsafe extern "C" fn stand_in_audio_ucode(_rdram: *mut u8, ucode_addr: u32) -> u32 {
    eprintln!(
        "[wm2000-census] STAND-IN audio ucode invoked for ucode_addr={ucode_addr:#010x} -- NOT the \
         real wm2000_audio_ucode (see main.rs's doc comment: the real one requires the GPL-3.0 \
         librecomp/rsp.hpp header, disallowed by AGENTS.md's clean-room protocol). Plumbing is \
         real; ucode execution is not."
    );
    0
}

fn env_path(name: &str) -> std::path::PathBuf {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("wm2000-census: required environment variable {name} not set"))
        .into()
}

fn main() {
    let rom_path = env_path("ROM");
    println!("[wm2000-census] loading ROM from {}", rom_path.display());
    let rom_bytes = std::fs::read(&rom_path).unwrap_or_else(|e| {
        panic!(
            "wm2000-census: failed to read ROM {}: {e}",
            rom_path.display()
        )
    });
    println!("[wm2000-census] ROM size: {} bytes", rom_bytes.len());
    // Keep the IPL3 boot-copy source image before `load_rom` takes ownership
    // of the ROM bytes -- see the IPL3 copy block below for why this exists.
    let ipl3_image: Vec<u8> = {
        const IPL3_CART_OFFSET: usize = 0x1000;
        const IPL3_COPY_LEN: usize = 0x10_0000; // 1 MiB, per IPL3 behavior
        let end = rom_bytes.len().min(IPL3_CART_OFFSET + IPL3_COPY_LEN);
        rom_bytes[IPL3_CART_OFFSET.min(rom_bytes.len())..end].to_vec()
    };
    fn64_abi::load_rom(rom_bytes);

    let registration = fn64_boot_harness::register_linked_sections();
    println!(
        "[wm2000-census] bridge reports {} sections in recomp_overlays.inl",
        registration.reported_count()
    );
    for section in registration.sections() {
        println!(
            "[wm2000-census] registered section {}: rom={:#010x} ram={:#010x} \
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
    // NOT loaded at boot. They do NOT need marking here: the guest's own
    // `func_80000744` (overlays.json's `load_fn`) DMAs each bank in through
    // `osEPiStartDma`, and `fn64_abi::note_dma_overlay_load` -- already wired
    // into the PI completion path -- marks the matching section loaded at the
    // base the guest actually chose. MEASURED on this ROM: banks 1/2/3 are
    // marked from their own DMAs (rom 0x4C160 -> section 2, 0x73390 ->
    // section 3, 0x809D0 -> section 4), each an exact `rom_addr` match. This
    // is why sections 3 and 4 can share link base 0x8011C900 without either
    // being guessed at: residency follows the guest's DMA, never a
    // convention.
    //
    // What the harness DOES have to supply is the registration of
    // N64Recomp's SECTION-LOCAL bodies (`static_<section>_<vram>`), which
    // appear in no `FuncEntry` table yet carry the entry observer; that is
    // handled by `register_linked_sections` (see its
    // `section_local_count`).
    for section_key in [0usize, 1usize] {
        if let Some(idx) = registration.registry_index(section_key) {
            fn64_abi::set_section_loaded(idx);
            println!("[wm2000-census] marked section {section_key} (index {idx}) loaded");
        }
    }
    println!(
        "[wm2000-census] registered {} section-local bodies (absent from every FuncEntry table)",
        registration.section_local_count()
    );

    // Real audio: replay every M_AUDTASK's genuine in-ROM ucode through the
    // clean-room RSP LLE interpreter (the same fallback path gfx tasks take)
    // so the CPU-side AKI sound driver observes the real mixer/sequence
    // state the RSP writes -- see stand_in_audio_ucode's doc comment for
    // the licensing history this supersedes.
    fn64_abi::set_audio_task_lle_accuracy();

    // REAL renderer seam: the in-repo software ReferenceBackend (the same
    // pure-Rust CI oracle fn64-shell uses), in F3DEX2 decode mode. NWXE's
    // gfx ucodes are "RSP Gfx ucode F3DEX xbus 2.08" and "F3DLX.Rej xbus
    // 2.08" (ID strings verified in the user's ROM) -- F3DEX2-generation but
    // not in the (empty-by-default) exact-digest HLE catalog, so every gfx
    // task takes the honest fallback path: `process_task` returns
    // `NeedsLle`, `osSpTaskStartGo_recomp` replays the WHOLE ucode phase
    // through the LLE RSP interpreter (`dispatch_lle_task`), and the real
    // RDP command stream the ucode emits (XBUS submissions included) is
    // rasterized by `process_rdp_commands`' raw-RDP lane, whose
    // `commit_color_image` writes actual pixels back into the guest RDRAM
    // framebuffer this harness hashes at every osViSwapBuffer. Auto-dump is
    // belt-and-braces evidence: the backend also writes its own rasterized
    // surface as PNGs, independent of the swap-hash capture below.
    {
        use fn64_render::RenderBackend as _;
        let mut backend = fn64_render_reference::ReferenceBackend::new()
            .with_f3dex2()
            .with_clear_color([0, 0, 0, 255])
            .with_auto_dump(
                &format!(
                    "{}/wm2000-gfx-dumps",
                    std::env::var("WM2000_OUT_DIR").unwrap_or_else(|_| "/tmp".to_string())
                ),
                "wm2000",
                8,
            );
        backend
            .create(&fn64_render::RenderConfig::ntsc(320, 240))
            .expect("reference backend create");
        fn64_abi::set_render_backend(Box::new(backend), fn64_boot_harness::rdram_len());
    }

    // NWXE saves to 256 Kbit (32 KiB) cartridge SRAM on PI domain 2: its own
    // SRAM-handle constructor (`func_80000A88`, funcs_0.c asm
    // 0x80000A9C-0x80000ABC) builds an OSPiHandle with baseAddress
    // 0xA8000000 / type 3 (SRAM) / domain PI_DOMAIN2 and the very first
    // frame-loop iteration (`func_800F4B60`) writes the 0x20-byte
    // 0x19990901 save signature to SRAM offset 0, retrying
    // `osEPiStartDma != 0` forever on failure -- without a registered save
    // device that retry loop IS the observed boot hang. Ephemeral in-memory
    // storage: this harness is boot telemetry, not a real player session,
    // so nothing is persisted (a fresh all-0xFF chip each run, exactly the
    // first-boot path the game must handle anyway).
    fn64_abi::set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
        fn64_runtime::SaveType::SramBanked,
    )));

    // NWXE's osCartRomInit (func_80022540) returns its OSPiHandle BSS at
    // D_800839A0 (`addiu $v0, $s0, %lo(D_800839A0)`, disasm/asm/1050.s
    // vram 0x80022578); the host shim hands guest code that same address.
    fn64_abi::set_cart_rom_handle_vram(0x8008_39A0);

    // NOTE (2026-07-21 park diagnosis): the long-standing vi_swaps=0 park
    // (thread 6 woken on its queue at rdram 0x838F0 but never scheduled
    // again; 16M trailing `ThreadSwitch {to: 3}` trace events) was NOT a
    // missing event post or a wake-delivery bug -- it was priority
    // starvation by a degenerate AKI audio-pump thread (guest OSId 3,
    // entry func_80026F18, guest priority 80) whose sound-driver ops table
    // (D_800481FC) read null, collapsing its body into a never-blocking
    // raw AI_STATUS/AI_LEN poll loop. THAT, in turn, was downstream of the
    // real root cause: this harness never performed the IPL3 boot copy
    // (below), so every resident `.data` initializer read as zero and boot
    // took a degraded path that created the pump early and unbound. With
    // the IPL3 copy in place, boot follows the real path (verified: the
    // run is byte-identical with or without an experimental pump-priority
    // clamp) and the park is gone.

    // Typed IPL video standard is the shared VI/AI clock authority. The first
    // field uses nominal NTSC timing; the latched OSViMode H/V registers then
    // refine it from the public VI clock.
    fn64_abi::configure_tv_type(fn64_boot_harness::TvType::Ntsc);

    // Arm crash-safe incremental trace flushing BEFORE booting thread 0 --
    // a SIGSEGV mid-boot (as rung 3 hit) must not lose the whole session's
    // trace; every event from here on is appended+flushed to disk as it's
    // recorded, not just buffered for the end-of-run `write_trace_file`
    // call below (which still runs too, on a clean exit, and rewrites the
    // same path from the in-memory copy -- harmless, since by then the
    // incremental sink already has every event that copy will contain).
    // Diagnostic outputs live under `WM2000_OUT_DIR` when set. Default is
    // unchanged so existing recipes keep working; the diagnostic lane sets it
    // because `/private/tmp` is wiped by a host restart.
    let out_dir = std::env::var("WM2000_OUT_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let trace_path = format!("{out_dir}/wm2000-boot-trace.jsonl");
    let trace_path = trace_path.as_str();
    if let Err(e) = fn64_abi::set_trace_sink_file(trace_path) {
        eprintln!(
            "[wm2000-census] WARNING: failed to arm incremental trace sink at {trace_path}: {e} -- \
             a crash mid-boot will lose the trace (falling back to end-of-run-only)."
        );
    } else {
        println!("[wm2000-census] incremental trace sink armed at {trace_path}");
    }

    // rdram: this process's one shared buffer (docs/DESIGN.md section 3).
    // Sized to also cover the `0xA4xxxxxx` hardware-register window
    // (`fn64_runtime::RDRAM_MMIO_WINDOW_END`), not just plain RDRAM content
    // -- this is the fix for the exact crash this harness hit: a raw guest
    // `lw` at `AI_STATUS` (`0xA450000C`) previously read 4x past an 8 MB
    // buffer's end (`docs/BOOT-NOTES-WM2000.md`'s LLDB-confirmed
    // `EXC_BAD_ACCESS`). See `fn64_runtime::mmio`'s module doc for the full
    // story.
    let mut rdram = fn64_boot_harness::new_rdram(fn64_boot_harness::TvType::Ntsc);
    let rdram_ptr = rdram.as_mut_ptr();

    // IPL3 boot copy: on real hardware, the CIC-6102-family boot code copies
    // 1 MiB of cartridge ROM starting at cart offset 0x1000 into RDRAM at
    // 0x80000400 BEFORE jumping to the game's entry point (the ROM header's
    // entry, 0x80000400 here -- section 0's registered ram_addr). NWXE's
    // `recomp_entrypoint` (funcs_0.c, vram 0x80000400) does NOT copy any
    // data itself -- it only clears BSS (0x8004B4C0..0x800B1390) and jumps
    // -- because the IPL already materialized .text+.data. Without this
    // copy every resident `.data` initializer reads as zero, which is
    // exactly the previously-documented `D_800481FC == 0` mystery
    // (docs/BOOT-NOTES-WM2000.md's exhaustive no-writer search) AND the
    // zeroed segment-descriptor table at 0x80047E80 that made the game's
    // own overlay loader (`func_80000744`) no-op its bank DMAs before
    // `func_80000870`'s hardcoded `jal 0x800E1B90` trapped. Words are
    // written through `RdramViewMut::write_u32` so the big-endian cart
    // bytes land in fn64's native-word RDRAM storage order.
    {
        const IPL3_DEST_OFFSET: u32 = 0x400;
        let copy_len = ipl3_image.len() & !3;
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for word_index in 0..copy_len / 4 {
            let src = word_index * 4;
            let word = u32::from_be_bytes(ipl3_image[src..src + 4].try_into().unwrap());
            view.write_u32(
                fn64_runtime::RdramAddr::from_offset(
                    IPL3_DEST_OFFSET + u32::try_from(src).expect("IPL copy fits u32"),
                ),
                word,
            );
        }
        println!(
            "[wm2000-census] IPL3 boot copy: {copy_len:#x} bytes from cart 0x1000 to rdram \
             {IPL3_DEST_OFFSET:#x}"
        );
    }

    // Prime the MMIO backing bytes before the guest ever runs, so even a
    // raw load before any host-side register mutation observes the real
    // idle-hardware defaults (e.g. AI_STATUS not-busy/not-full,
    // SP_STATUS halted+broke) rather than zeroed memory.
    unsafe { fn64_abi::sync_mmio_into_rdram(rdram_ptr) };

    println!("[wm2000-census] booting thread 0 (recomp_entrypoint)...");
    unsafe {
        fn64_abi::boot_thread0(
            rdram_ptr,
            fn64_boot_harness::rdram_len(),
            fn64_boot_harness::c_recomp_entrypoint(),
            0,
            10,
        );
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
    // 20M: with the C-lane raw-MMIO time charge, the audio manager's
    // AI_STATUS poll consumes ~56k steps per 19ms audio buffer, so 2M steps
    // only covered ~0.8s of virtual boot.
    //
    // Step-economics re-measurement (2026-07-21, post RDRAM-swizzle/RSP-replay
    // fix): the AI pump is NOT a step sink anymore. Trace evidence
    // (/tmp/wm2000-boot-trace.jsonl, 300k-step run): audio tasks are submitted
    // at exactly one per VI field (1,563,558-cycle gaps; occasionally one per
    // two fields) -- the retrace-driven cadence real hardware has, NOT a
    // starved-feedback loop from the stand-in ucode completing instantly. The
    // whole per-field round (retrace fan-out -> AI pump's ~6 raw AI_STATUS/
    // AI_LEN polls at 32 cycles each -> audio manager task submit) costs
    // ~10-16 scheduler steps, so the default 20M-step budget covers ~1M+
    // fields (hours of virtual boot). The budget is no longer what limits gfx
    // frames; the game itself stops submitting tasks after its first 3 gfx
    // frames (sim ~16.07B) -- see the boot-stall notes in the README.
    const MAX_STEPS: u64 = 20_000_000;
    const LOG_EVERY: u64 = 500_000;
    /// Census flush cadence. Tighter than the progress cadence because the
    /// census is the deliverable and the run is expected to abort.
    const CENSUS_EVERY: u64 = 50_000;
    // Once the RSP-task throughput fix landed, 20M steps pass in a couple of
    // wall minutes; deeper ladder probes need a bigger (still bounded) budget
    // without editing the source. Same "must be a positive integer" stance as
    // the FN64_* knobs in fn64-abi.
    let max_steps = match std::env::var("WM2000_MAX_STEPS") {
        Ok(raw) => raw
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("WM2000_MAX_STEPS must be a positive integer, got {raw:?}")),
        Err(_) => MAX_STEPS,
    };
    // How many consecutive "nothing was runnable, and advancing the
    // virtual clock didn't wake anything either" ticks before concluding
    // boot has reached a genuinely idle steady state (not just a thread
    // temporarily blocked waiting for a soon-to-fire timer/retrace).
    const IDLE_TICKS_BEFORE_STOP: u32 = 200;
    let input_script = InputScript::from_env();
    let mut last_buttons = 0u16;
    let mut last_swap_count = 0u64;
    let mut fb_dumps = Vec::new();
    let mut thread0_death_logged = false;
    let mut consecutive_idle_ticks = 0u32;

    let mut tick = 0u64;
    let mut steps = 0u64;
    loop {
        if steps >= max_steps {
            println!(
                "[wm2000-census] step budget ({max_steps}) exhausted at sim_time={} -- stopping \
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
        let stepped = fn64_abi::run_one_step();
        steps += 1;
        // Emit the census incrementally, on the same cadence as the progress
        // line. A real WM2000 boot reaches a hard guest-side abort deep into
        // the attract loop (measured: an unmodelled direct-device read at
        // 0x1CC, ~383 VI swaps in), and that abort is a `panic_cannot_unwind`
        // through a nounwind shim -- no unwinding, no destructors, no
        // end-of-run report. Writing as we go is what makes the numbers
        // survive the crash that actually happens.
        if steps.is_multiple_of(CENSUS_EVERY) {
            fn64_render_reference::gbi::census::report_quiet();
            // Same crash-survival argument, applied to the per-rectangle
            // cycle-mode probe: it has no end-of-run report either, because
            // the run does not reach one.
            fn64_render_reference::gbi::census::texrect::report();
            // And to the packet dump, for the same reason. Its selection is
            // bounded by decode entry, so unlike the two censuses above it
            // stops growing once the selected entries are past -- but it
            // still has to be written before the abort to be written at all.
            fn64_render_reference::gbi::census::packet::report();
        }
        if steps.is_multiple_of(LOG_EVERY) {
            println!(
                "[wm2000-census] progress: steps={steps} sim_time={} vi_swaps={} gfx_tasks={} \
                 audio_tasks={}",
                fn64_abi::sim_time(),
                fn64_abi::vi_swap_count(),
                fn64_abi::task_counts().0,
                fn64_abi::task_counts().1
            );
            // Progress must survive a later hard kill/abort: stdout is
            // block-buffered when piped, and a guest entering a non-yielding
            // spin (a real, observed end state) means no clean exit flushes.
            let _ = std::io::stdout().flush();
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
                "[wm2000-census] thread 0 (recomp_entrypoint) returned at step {steps} -- expected \
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
            // Swap-indexed scripted input. Off unless `WM2000_INPUT_SCRIPT` is
            // set, so an un-driven run still sees the honest neutral pad
            // `fn64_runtime::ContInput::default()` documents. VI-swap ordinal
            // (not step count) is the clock, because that is the boundary the
            // guest's own per-field controller poll is phase-locked to.
            if let Some(script) = input_script.as_ref() {
                let buttons = script.buttons_at(swap_count);
                if buttons != last_buttons {
                    fn64_abi::set_controller_state(0, buttons, 0, 0);
                    println!(
                        "[wm2000-census] input: swap #{swap_count} port0 buttons={buttons:#06x}"
                    );
                    let _ = std::io::stdout().flush();
                    last_buttons = buttons;
                }
            }
            last_swap_count = swap_count;
        }

        if !stepped {
            // Nothing was runnable -- host-driven progress (VI retrace,
            // due timers) is the only way forward.
            // Base the next target on where the clock ACTUALLY is, not on a
            // private counter. Guest device work advances the fabric on its
            // own, so a counter seeded at zero and stepped by one field falls
            // behind it, and `advance_virtual_time` then asserts "device time
            // moved backwards" (crates/fn64-abi/src/pi/timing.rs:288-291) --
            // measured here at step 3 of a real WM2000 boot, current=26016446
            // against a requested 25015870. Taking the max keeps every target
            // monotonically ahead of the fabric while still stepping exactly
            // one field past whichever clock is further along.
            let field = fn64_abi::vi_field_interval()
                .expect("typed television standard must keep VI armed");
            tick = tick.max(fn64_abi::sim_time()) + field;
            fn64_abi::advance_virtual_time(tick);
            consecutive_idle_ticks += 1;
            if consecutive_idle_ticks >= IDLE_TICKS_BEFORE_STOP {
                println!(
                    "[wm2000-census] reached a steady idle state ({IDLE_TICKS_BEFORE_STOP} \
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
    println!("[wm2000-census] === BOOT SUMMARY ===");
    println!(
        "[wm2000-census] virtual ticks run: {}",
        fn64_abi::sim_time()
    );
    println!(
        "[wm2000-census] thread 0 dead: {}",
        fn64_abi::is_thread_dead(0)
    );
    println!(
        "[wm2000-census] VI swaps observed: {}",
        fn64_abi::vi_swap_count()
    );
    println!("[wm2000-census] gfx tasks submitted: {gfx_count}");
    println!("[wm2000-census] audio tasks submitted: {audio_count}");
    println!(
        "[wm2000-census] non-uniform framebuffers dumped: {} ({:?})",
        fb_dumps.len(),
        fb_dumps
    );

    let trace = fn64_abi::copy_trace();
    println!("[wm2000-census] trace events recorded: {}", trace.len());
    write_trace_file(&trace, trace_path);
    println!("[wm2000-census] trace written to {trace_path}");

    // Clean shutdown: parked guest coroutines must NOT be force-unwound by
    // the TLS destructor (they're suspended inside nounwind extern "C"
    // shims -- that unwind is an instant abort). Abandon them explicitly;
    // see `fn64_abi::shutdown_abandon_threads`'s doc comment.
    let exit_summary = fn64_abi::prepare_process_exit();
    println!("[wm2000-census] clean shutdown: {exit_summary:?}");

    // The whole point of this harness: emit the per-opcode histogram the
    // decoder accumulated. A no-op unless FN64_GBI_CENSUS is set, so the
    // binary still works as a plain boot harness when it is not.
    fn64_render_reference::gbi::census::report();
    fn64_render_reference::gbi::census::texrect::report();
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
            "[wm2000-census] swap #{swap_index}: framebuffer offset {fb_offset:#x} + assumed size \
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
            "[wm2000-census] swap #{swap_index}: framebuffer at {fb_offset:#010x} is UNIFORM \
             (all bytes == {first_byte:#04x}) -- reported as blank, not dumped (per task: \"a \
             blank/uniform fb is reported as blank\")."
        );
        return;
    }

    println!(
        "[wm2000-census] swap #{swap_index}: framebuffer at {fb_offset:#010x} is NON-UNIFORM -- \
         dumping PNG."
    );
    let path = format!(
        "{}/fn64-fb-{swap_index}.png",
        std::env::var("WM2000_OUT_DIR").unwrap_or_else(|_| "/tmp".to_string())
    );
    match dump_rgba5551_as_png(rdram, start, FB_WIDTH, FB_HEIGHT, &path) {
        Ok(()) => {
            println!("[wm2000-census] *** NON-UNIFORM FRAMEBUFFER DUMPED: {path} ***");
            dumps.push(path);
        }
        Err(e) => eprintln!("[wm2000-census] failed to write {path}: {e}"),
    }

    // Additive, env-gated companion capture at the VI's LIVE geometry, never
    // an assumed one. The 320x240 dump above is retained unchanged so both
    // readings of the SAME bytes at the SAME swap can be compared directly;
    // that is the whole point of this probe, since reading a 480-stride
    // buffer at a 320 stride shears every row and can manufacture a
    // convincing "horizontal duplication" artifact out of coherent memory.
    if std::env::var("WM2000_TRUE_GEOMETRY_DUMP").is_err() {
        return;
    }
    let (Some(vi_w), Some(vi_h)) = (fn64_abi::vi_width(), fn64_abi::vi_output_height()) else {
        eprintln!(
            "[wm2000-census] swap #{swap_index}: VI geometry not programmed yet -- refusing to \
             guess a true-geometry stride"
        );
        return;
    };
    let (true_w, true_h) = (vi_w as usize, vi_h as usize);
    let true_bytes = true_w * true_h * 2;
    if start + true_bytes > rdram.len() {
        eprintln!(
            "[wm2000-census] swap #{swap_index}: true geometry {true_w}x{true_h} exceeds rdram \
             bounds -- skipping"
        );
        return;
    }
    let true_path = format!(
        "{}/fn64-fb-true-{true_w}x{true_h}-{swap_index}.png",
        std::env::var("WM2000_OUT_DIR").unwrap_or_else(|_| "/tmp".to_string())
    );
    println!(
        "[wm2000-census] swap #{swap_index}: live VI geometry is {true_w}x{true_h} (fb \
         {fb_offset:#010x})"
    );
    match dump_rgba5551_as_png(rdram, start, true_w, true_h, &true_path) {
        Ok(()) => println!("[wm2000-census] *** TRUE-GEOMETRY FRAMEBUFFER DUMPED: {true_path} ***"),
        Err(e) => eprintln!("[wm2000-census] failed to write {true_path}: {e}"),
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
            eprintln!("[wm2000-census] failed to create trace file {path}: {e}");
            return;
        }
    };
    for event in trace {
        let line = format!("{event:?}\n");
        let _ = file.write_all(line.as_bytes());
    }
}

/// Swap-indexed scripted controller input for the diagnostic lane.
///
/// `WM2000_INPUT_SCRIPT` holds `first_swap:end_swap:buttons_hex` phases,
/// comma-separated (e.g. `1200:1210:0x1000,1260:1270:0x1000` for two Start
/// taps). Outside every phase the pad is neutral, which is what makes a
/// state change attributable to the press rather than to the harness having
/// held a button since boot. Ranges are half-open and evaluated in order.
///
/// This is deliberately NOT
/// `fn64_boot_harness::parse_controller_input_schedule`: that schema indexes
/// by successful controller-READ ordinal, which is the right clock for a
/// determinism replay but is not observable from this harness (no read-ordinal
/// counter is exported). Swap ordinal is observable here and is the clock the
/// framebuffer captures are already indexed by, so a press and the frame it
/// changed share one axis.
struct InputScript {
    phases: Vec<(u64, u64, u16)>,
}

impl InputScript {
    fn from_env() -> Option<Self> {
        let raw = std::env::var("WM2000_INPUT_SCRIPT").ok()?;
        let mut phases = Vec::new();
        for field in raw.split(',').map(str::trim).filter(|f| !f.is_empty()) {
            let parts = field.split(':').collect::<Vec<_>>();
            let [first, end, buttons] = parts.as_slice() else {
                panic!("WM2000_INPUT_SCRIPT phase {field:?} is not first:end:buttons_hex");
            };
            let first = first
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("WM2000_INPUT_SCRIPT: bad first_swap in {field:?}"));
            let end = end
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("WM2000_INPUT_SCRIPT: bad end_swap in {field:?}"));
            assert!(
                first < end,
                "WM2000_INPUT_SCRIPT phase {field:?} is empty (first_swap >= end_swap)"
            );
            let buttons = u16::from_str_radix(buttons.trim_start_matches("0x"), 16)
                .unwrap_or_else(|_| panic!("WM2000_INPUT_SCRIPT: bad buttons_hex in {field:?}"));
            phases.push((first, end, buttons));
        }
        println!(
            "[wm2000-census] input script armed: {} phase(s)",
            phases.len()
        );
        Some(Self { phases })
    }

    fn buttons_at(&self, swap: u64) -> u16 {
        self.phases
            .iter()
            .find(|&&(first, end, _)| swap >= first && swap < end)
            .map_or(0, |&(_, _, buttons)| buttons)
    }
}
