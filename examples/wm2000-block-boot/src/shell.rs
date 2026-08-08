//! `wm2000-shell`: WM2000 (NWXE) in a window, on live input, from the SAME
//! certified dense-AOT block program the headless `wm2000-block-boot` runs.
//!
//! ## Why this lives in the block-boot package rather than `crates/fn64-shell`
//!
//! `crates/fn64-shell` is the FUNCTION-lane harness: its boot calls
//! `fn64_abi::recompiled::boot_thread0(rdram_ptr, len, lookup, entrypoint, ..)`
//! with a linked whole-ROM `*-recompiled` crate. WM2000 has no such crate. Its
//! program is the shared dense-AOT shard catalog sealed by
//! `boot_thread0_validated_catalog_generation_program_v1`, which takes a
//! `ValidatedBootstrapRdramV1` + `CatalogGenerationInstallV1` + `BootContext`
//! instead of a lookup function. The two boot seams are not variants of one
//! call; they are different contracts.
//!
//! They also differ on RDRAM OWNERSHIP, which is what really decides the file
//! layout. The function lane hands `fn64-abi` a pointer to RDRAM the harness
//! keeps (`fn64-shell` presents straight out of its own `Vec<u8>`). The block
//! lane's bootstrap transaction VALIDATES an owned allocation and MOVES it into
//! the runtime, so nothing outside `fn64-abi` holds the framebuffer bytes
//! afterwards. A windowed block-lane runner must therefore read the VI
//! framebuffer back through the runtime
//! (`fn64_abi::with_registered_physical_rdram_read`), which is a different
//! present path, not a parameterization of `fn64-shell`'s.
//!
//! Living in this package instead buys the thing that matters most: the
//! generated shard crates, `build.rs`'s ROM discovery, and the one `OUT_DIR`
//! `pack.rs` are SHARED with the batch runner. A separate crate would rebuild
//! all of it (~3 GB of artifacts) and, worse, could drift from the certified
//! program. Here, both binaries call the same
//! [`crate::block_program::construct_catalog_program`], so "the shell boots a
//! different program than the gate certifies" is not expressible.
//!
//! The window/input/audio side is NOT reimplemented: `framebuffer.rs`,
//! `gamepad.rs`, `input_map.rs`, and `timing.rs` are included verbatim from
//! `crates/fn64-shell/src/` by `#[path]`, so key bindings, deadzones, the
//! RGBA5551 decode, and the retrace drain stay defined in exactly one place.
//!
//! Run it:
//! ```text
//! source .claude/local.env
//! C=/Users/jer/Code/aki-recomp/captures/wm-general-exception-images
//! ROM=$FN64_DISCOVER_NWXE_ROM \
//! FN64_BOOT_CONTEXT=/Users/jer/Code/aki-recomp/captures/wm2000-boot-context.json \
//! FN64_EXECUTABLE_IMAGES="$C/run-1/image.json:$C/run-2/image.json:$C/run-3/image.json" \
//! cargo run --manifest-path examples/wm2000-block-boot/Cargo.toml --bin wm2000-shell
//! ```

mod block_program;
mod dense_aot;
use block_program::*;
use dense_aot::*;

use fn64_recomp_rs::{
    BackedExecutableSpanV1, BackedPrecompiledGenerationCatalogV1, BankId, BlockRun, BootContext,
    CargoGeneratedProgramSourceAttestationV2, CargoGeneratedRunnerSourceBindingV1,
    CatalogBlockProgramV1, CodeBank, ExecutableRegion, ExecutionKey, GeneratedAdapterRole,
    GeneratedBankFn, GeneratedBankRunner, GenerationId, GuestPc, InstructionBudget,
    PrecompiledGeneration, PrecompiledGenerationBackingV1, PrecompiledGenerationCatalog,
    PrecompiledShard, ProgramArtifactIdentity, Rdram, RecompContext,
};
use sha2::{Digest, Sha256};

/// The shell's UI seam, shared verbatim with `crates/fn64-shell` rather than
/// re-derived. These four modules are pure (no dependency on that binary's
/// OoT-specific `game` module), so including them by path keeps ONE definition
/// of the RGBA5551 decode, the key/pad bindings, and the retrace drain.
#[path = "../../../crates/fn64-shell/src/framebuffer.rs"]
mod framebuffer;
#[path = "../../../crates/fn64-shell/src/input_map.rs"]
mod input_map;
#[path = "../../../crates/fn64-shell/src/gamepad.rs"]
mod gamepad;
#[path = "../../../crates/fn64-shell/src/timing.rs"]
mod timing;

use framebuffer::{FB_HEIGHT, FB_WIDTH};
use gamepad::Gamepads;
use input_map::{InputConfig, PadState};
use timing::{RetraceOutcome, TimingWindow};

use pixels::{Pixels, SurfaceTexture};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

// `block_program.rs` and `dense_aot.rs` reach for these crate-root items via
// `use crate::*`. The batch runner defines them alongside its profiling
// instrumentation; this lane needs only the plain ones.

/// Threshold in milliseconds above which a pump dumps its own attribution, or
/// 0 when the gate is off.
///
/// Diagnostics only, and `OnceLock`-gated like every other `FN64_*` census, so
/// an unset variable costs one relaxed load per frame.
///
/// This exists because a p99 says a frame was slow and nothing about WHY. A
/// windowed run recorded a single **3,058 ms** frame against a p50 of 60 ms --
/// three orders of magnitude, invisible to p95 -- and no statistic the
/// heartbeat reports can distinguish the candidate causes: guest work in the
/// scheduling arm, a device-advance storm in the other arm, or a host stall
/// (audio callback, page faults, window back-pressure) that is not our work at
/// all. Each predicts a DIFFERENT counter, so the outlier is attributable by
/// counting rather than by inference:
///
/// - guest execution  -> `steps` is large; `sim_time` advances a lot
/// - device-advance storm -> `advances` approaches its 4,096 cap with few steps
/// - a host stall -> BOTH are ordinary and the wall clock is not
///
/// The third case is the one worth naming: an ordinary step count next to a
/// three-second wall time is positive evidence that the time was not spent
/// running the guest, which no amount of reading the pump loop can establish.
///
/// `FN64_SHELL_SPIKE_MS=<ms>`; absent, empty, `0`, or unparseable means off, on
/// the same reasoning as `write_barrier::env_flag` (a diagnostic whose off lane
/// is silently the on lane is worse than no diagnostic).
fn spike_threshold_ms() -> f64 {
    static THRESHOLD: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *THRESHOLD.get_or_init(|| {
        std::env::var("FN64_SHELL_SPIKE_MS")
            .ok()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|ms| *ms > 0.0)
            .unwrap_or(0.0)
    })
}

/// Cheap counters read on both sides of one pump, so a slow frame can be
/// attributed by DIFFERENCE rather than by absolute value.
///
/// Every field is an existing always-on counter: none of this turns on tracing
/// or allocates. `host_evidence_snapshot()` would answer more questions but
/// runs assertions and builds several `Vec`s, which is not something a frame
/// path can afford to call unconditionally.
#[derive(Clone, Copy)]
struct PumpCounters {
    sim_time: u64,
    gfx_submits: u64,
    audio_submits: u64,
    vi_swaps: u64,
    ai_buffers: u64,
    backend_buffers: u64,
    audio_samples: u64,
    /// Counters from cpal's realtime thread, which runs INDEPENDENTLY of the
    /// emulation thread. This is the host-vs-guest discriminator: a stall in
    /// our pump leaves the audio callback ticking normally, whereas a stall in
    /// the machine (scheduler preemption, page-fault storm, device
    /// reconfiguration) stops both, and shows up here as a callback gap of the
    /// same order as the frame.
    audio_callbacks: u64,
    audio_max_gap_us: u64,
    audio_late_callbacks: u64,
    audio_underrun_samples: u64,
}

impl PumpCounters {
    fn read() -> Self {
        let (gfx_submits, audio_submits) = fn64_abi::task_counts();
        let audio = fn64_abi::audio_output_stats();
        let health = fn64_abi::audio_stream_health().unwrap_or_default();
        Self {
            sim_time: fn64_abi::sim_time(),
            gfx_submits,
            audio_submits,
            vi_swaps: fn64_abi::vi_swap_count(),
            ai_buffers: audio.ai_buffers,
            backend_buffers: audio.backend_buffers,
            audio_samples: audio.samples,
            audio_callbacks: health.callbacks,
            audio_max_gap_us: health.max_callback_gap_us,
            audio_late_callbacks: health.late_callbacks,
            audio_underrun_samples: health.underrun_samples,
        }
    }
}

/// What one pump did, in counts. `steps` and `advances` come from the pump
/// loop's own arms, so they attribute the wall time to a specific branch.
struct PumpAttribution {
    steps: u64,
    advances: u32,
    /// Wall time inside the scheduling arm only (`run_one_step` and the
    /// controller feed), excluding device advances. The split matters: the two
    /// arms have different fixes, and a pump that is 95% device advance is not
    /// a guest-throughput problem however slow it looks.
    step_ms: f64,
    /// Wall time inside `advance_to_next_device_event` only.
    advance_ms: f64,
    before: PumpCounters,
    after: PumpCounters,
}

impl PumpAttribution {
    fn report(&self, total_ms: f64, frame: u64) {
        let before = &self.before;
        let after = &self.after;
        // Wall time this pump did not spend in either arm. A large residual is
        // itself the finding: it means the stall was not in guest stepping or
        // device advance, which is where a host-side cause would show up.
        let unattributed_ms = (total_ms - self.step_ms - self.advance_ms).max(0.0);
        let per_step_us = if self.steps == 0 {
            0.0
        } else {
            self.step_ms * 1000.0 / self.steps as f64
        };
        println!(
            "[wm2000-shell] SPIKE frame #{frame}: wall={total_ms:.1}ms \
             steps={} ({:.1}ms, {per_step_us:.2}us/step) advances={} ({:.1}ms) \
             unattributed={unattributed_ms:.1}ms; \
             d_sim_time={} d_gfx={} d_audio={} d_vi_swaps={} \
             d_ai_buffers={} d_backend_buffers={} d_audio_samples={}; \
             host: d_audio_callbacks={} audio_max_gap_us={} d_late_callbacks={} \
             d_underrun_samples={}",
            self.steps,
            self.step_ms,
            self.advances,
            self.advance_ms,
            after.sim_time.wrapping_sub(before.sim_time),
            after.gfx_submits.wrapping_sub(before.gfx_submits),
            after.audio_submits.wrapping_sub(before.audio_submits),
            after.vi_swaps.wrapping_sub(before.vi_swaps),
            after.ai_buffers.wrapping_sub(before.ai_buffers),
            after.backend_buffers.wrapping_sub(before.backend_buffers),
            after.audio_samples.wrapping_sub(before.audio_samples),
            after.audio_callbacks.wrapping_sub(before.audio_callbacks),
            // Not a delta: `max_callback_gap_us` is a running maximum, so its
            // value AFTER the spike is what a host stall would have raised.
            after.audio_max_gap_us,
            after.audio_late_callbacks.wrapping_sub(before.audio_late_callbacks),
            after
                .audio_underrun_samples
                .wrapping_sub(before.audio_underrun_samples),
        );
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
    }
}

struct DenseAotArtifact {
    bank_id: u64,
    code_bank: fn() -> CodeBank,
    runner: GeneratedBankFn,
}

#[derive(Clone, Copy)]
struct LinkedDenseIdentity {
    source_sha256: [u8; 32],
    runner_source_sha256: [u8; 32],
}

#[allow(clippy::all, unused)]
mod gen {
    use fn64_recomp_rs::{
        BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CpuException, CpuFault, CpuFaultKind,
        ExecutionKey, GeneratedBankRunner, GuestPc, InstructionBudget, ProgramError, Rdram,
        RecompContext,
    };
    include!(concat!(env!("OUT_DIR"), "/runner.rs"));
}
mod pack {
    include!(concat!(env!("OUT_DIR"), "/pack.rs"));
}

fn code_bank_sha256(code: &CodeBank) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for span in code.spans() {
        for word in span.words() {
            hasher.update(word.to_be_bytes());
        }
    }
    hasher.finalize().into()
}

fn entry_bank() -> BankId {
    BankId::new(pack::ENTRY_BANK_ID)
}

fn external_image_for_bank(bank: BankId) -> Option<&'static pack::ExternalExecutableImage> {
    let mut matches = pack::EXTERNAL_EXECUTABLE_IMAGES
        .iter()
        .filter(|image| image.bank_id == bank.get());
    let image = matches.next();
    assert!(
        matches.next().is_none(),
        "generated external executable-image bank IDs collide at {bank}"
    );
    image
}

/// Register a captured exception-vector image as its own generation, with the
/// exact physical backing its KSEG image address implies. Identical in effect
/// to the batch runner's copy; kept here because it is three lines of geometry
/// around one catalog call, and the batch runner's version is entangled with
/// its diagnostics.
fn register_external_executable_generation(
    catalog: &mut PrecompiledGenerationCatalog,
    backings: &mut Vec<PrecompiledGenerationBackingV1>,
    bank: BankId,
    image_start: GuestPc,
    image_end: GuestPc,
    expected_sha256: [u8; 32],
) {
    let generation = GenerationId::new(bank.get());
    catalog
        .register(
            PrecompiledGeneration::new(
                generation,
                image_start,
                image_end,
                image_start,
                image_end,
                expected_sha256,
                vec![PrecompiledShard::new(bank, image_start, image_end)
                    .expect("generated dynamic shard geometry is valid")],
            )
            .expect("generated dynamic generation geometry is valid"),
        )
        .expect("generated dynamic generation catalog is unambiguous");
    assert!(
        (0x8000_0000..0xc000_0000).contains(&image_start.get()) && image_end.get() <= 0xc000_0000,
        "external executable generation backing must be direct-mapped KSEG"
    );
    backings.push(
        PrecompiledGenerationBackingV1::new(
            generation,
            vec![BackedExecutableSpanV1::new(
                image_start,
                image_start.get() & 0x1fff_ffff,
                image_end.get() - image_start.get(),
            )
            .expect("external executable generation physical backing is valid")],
        )
        .expect("external executable generation backing is contiguous"),
    );
}

// Holds the black-box `BootContext` until the first generated-bank entry
// validates against it, then takes it. The batch runner performs this same
// check; keeping it in the shell too means a windowed session cannot silently
// start from a context the certified lane would have rejected.
thread_local! {
    static FIRST_ENTRY_BOOT_CONTEXT: std::cell::RefCell<Option<BootContext>> = const {
        std::cell::RefCell::new(None)
    };
}

fn run_entry_aot_with_context_gate(
    entry: ExecutionKey,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) -> BlockRun {
    FIRST_ENTRY_BOOT_CONTEXT.with(|slot| {
        if let Some(expected) = slot.borrow_mut().take() {
            assert_eq!(entry.pc.get(), expected.entry_pc);
            let mismatches = ctx
                .boot_context_state_mismatches(&expected)
                .expect("validating first-entry BootContext");
            assert!(
                mismatches.is_empty(),
                "first generated-bank entry differs from black-box BootContext: {mismatches:?}"
            );
            println!("[wm2000-shell] first-entry BootContext matches exactly");
        }
    });
    (DENSE_AOT_ARTIFACTS[0].runner)(entry, budget, ctx, mem)
}

fn run_overlay_aot_with_generation_gate(
    entry: ExecutionKey,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) -> BlockRun {
    let artifact = DENSE_AOT_ARTIFACTS
        .iter()
        .skip(pack::BOOT_SHARDS.len() + pack::RESIDENT_TAIL_SHARDS.len())
        .find(|artifact| artifact.bank_id == entry.bank.get())
        .unwrap_or_else(|| panic!("no compiled overlay AOT artifact for {}", entry.bank));
    (artifact.runner)(entry, budget, ctx, mem)
}

/// Re-verify a captured exception image against live RDRAM before executing it.
/// This is the SAME digest gate the batch runner installs: an interactive
/// session must not execute a vector image the ROM no longer backs.
fn run_nwxe_exception_image_with_digest_gate(
    entry: ExecutionKey,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) -> BlockRun {
    let image = external_image_for_bank(entry.bank)
        .unwrap_or_else(|| panic!("no external executable image for {}", entry.bank));
    fn64_boot_harness::verify_precompiled_words(
        entry.bank,
        GuestPc::new(image.va_start),
        image.words,
        image.sha256,
        mem,
    )
    .unwrap_or_else(|miss| panic!("{miss}"));
    gen::run_nwxe_exception_image(entry, budget, ctx, mem)
}

/// Boot WM2000's certified program into the runtime, then hold everything the
/// window loop touches each frame.
///
/// Note the absent field: there is no `rdram: Vec<u8>`. The validated bootstrap
/// moved the allocation into `fn64-abi`, so every framebuffer read goes back
/// through the runtime.
struct Shell {
    pad: PadState,
    config: InputConfig,
    gamepads: Gamepads,
    rgba: Vec<u8>,
    fb_width: usize,
    /// Height of the present surface. The reference path holds this at
    /// `FB_HEIGHT` (the guest's VI framebuffer is always 240 lines here), but
    /// RT64 renders at its own internal resolution and resizes both axes, so
    /// height can no longer be assumed constant.
    fb_height: usize,
    window: Option<Arc<Window>>,
    pixels: Option<Pixels<'static>>,
    reported_first_frame: bool,
    /// Frames this shell has actually blitted to the window. The heartbeat
    /// counts these rather than `vi_swap_count()`, which is zero for the whole
    /// run of a game that programs VI_ORIGIN directly instead of calling
    /// `osViSwapBuffer` -- see `present`.
    presented_frames: u64,
    last_heartbeat_frame: u64,
    next_frame_deadline: std::time::Instant,
    frame_intervals: TimingWindow,
    pump_times: TimingWindow,
    /// False until the first pump has run the guest on the captured boot clock.
    /// See `pump_one_frame` for why the first retrace must not precede it.
    entered_first_dispatch: bool,
    /// Present when `FN64_CONTROLLER_SCHEDULE` names a committed route, which
    /// then owns the pad instead of the live gamepad.
    schedule: Option<ScheduleDriver>,
    /// Which rasterizer `FN64_RENDER` selected, decided once at registration.
    ///
    /// This is not cosmetic: it picks the PRESENT PATH. The reference backend
    /// rasterizes into guest RDRAM, so the window decodes RGBA5551 out of the
    /// runtime's framebuffer. RT64 renders into its own GPU surface and writes
    /// nothing back, so that decode would read a stale (usually black) buffer
    /// forever -- the RT64 image has to come back through
    /// `fn64_abi::capture_render_release_frame`.
    active_renderer: ActiveRenderer,
}

/// The rasterizer in use, and therefore which of the two present paths runs.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum ActiveRenderer {
    /// Software rasterization into guest RDRAM; present decodes RGBA5551.
    Reference,
    /// Native GPU rasterization into RT64's own surface; present reads the
    /// post-VI BGRA8 capture back through the ABI seam.
    Rt64,
}

/// Bound on scheduling steps per pump, so a pathological spin cannot wedge the
/// window. Same value and rationale as `crates/fn64-shell`.
const STEPS_PER_PUMP: u64 = 200_000;

/// Bound for the pump that is still bringing the guest up to its first VI
/// field, before any frame has been presented.
///
/// `STEPS_PER_PUMP` bounds a STEADY-STATE frame, where a guest that has not
/// yielded within 200k scheduling steps is spinning. Boot is not steady state:
/// WM2000's certified batch route needs ~420k steps just to reach its menus,
/// all of it before the first field, so the steady-state bound turns a normal
/// boot into either a 31-minute wait or a spurious assert -- measured, the
/// headless probe printed zero frame lines in 18m47s at 100% CPU because
/// frame 0 could not finish.
///
/// This is deliberately generous rather than unbounded: a guest that never
/// reaches a first field still trips it, which is the case the bound exists
/// for.
const BOOT_STEPS_PER_PUMP: u64 = 4_000_000;

/// Bound on consecutive device-time advances in one pump. WM2000's boot chains
/// many short device deadlines between VI fields; without a cap the loop could
/// stay inside one pump long enough to stop servicing window events.
const DEVICE_ADVANCES_PER_PUMP: u32 = 4_096;

/// Replay a committed controller route in the windowed lane.
///
/// The gate lane already accepts `FN64_CONTROLLER_SCHEDULE`; the shell only
/// took live pad input, so the two lanes could not be driven through the same
/// inputs and a route that reached a menu was not reproducible in the lane
/// that draws it.
///
/// The clock is the controller READ ORDINAL, not wall time or step count --
/// a read is a shared external boundary, so the same file replays identically
/// in both lanes even though one is pumped by a window and the other by a
/// batch loop.
struct ScheduleDriver {
    schedule: fn64_boot_harness::ControllerInputSchedule,
    read_ordinals: [u64; 4],
    current: [fn64_runtime::ContInput; 4],
    cursor: usize,
}

impl ScheduleDriver {
    fn load(path: &std::path::Path) -> Self {
        let source = std::fs::read(path).unwrap_or_else(|error| {
            panic!("reading controller schedule {}: {error}", path.display())
        });
        let schedule = fn64_boot_harness::parse_controller_input_schedule(&source)
            .unwrap_or_else(|error| {
                panic!("parsing controller schedule {}: {error}", path.display())
            });
        let driven = schedule.driven_ports();
        println!(
            "[wm2000-shell] controller schedule={} phases={} driven_ports={driven:?} sha256={}",
            path.display(),
            schedule.phases().len(),
            schedule.source_sha256_hex(),
        );
        fn64_boot_harness::attach_controllers_for_driven_ports(driven);
        Self {
            schedule,
            read_ordinals: [0; 4],
            current: [fn64_runtime::ContInput::default(); 4],
            cursor: fn64_abi::copy_controller_operations().len(),
        }
    }

    /// Advance each port's read ordinal by the reads the guest actually
    /// performed since the last call.
    fn observe_reads(&mut self) {
        let operations = fn64_abi::copy_controller_operations_since(self.cursor);
        self.cursor += operations.len();
        for operation in operations {
            if operation.device == fn64_runtime::ControllerOperationDevice::StandardController
                && operation.operation == fn64_runtime::ControllerOperationKind::Read
            {
                let port = usize::from(operation.port);
                self.read_ordinals[port] = self.read_ordinals[port]
                    .checked_add(1)
                    .expect("controller read ordinal overflow");
            }
        }
    }

    fn apply(&mut self) {
        for port in 0..4 {
            let input = self.schedule.input_for_read(port, self.read_ordinals[port]);
            if input != self.current[port] {
                fn64_abi::set_controller_state(port, input.button, input.stick_x, input.stick_y);
                self.current[port] = input;
                // Report the SAME counters the headless dump lane reports at
                // this exact boundary (`main.rs`'s `input_edge` line), so the
                // two lanes can be diffed row-for-row on one route. Printing
                // only `vi_swaps` here is what made every previous window
                // investigation ambiguous: `vi_swap_count` counts guest
                // `osViSwapBuffer` calls, NOT VI retraces, so a zero there
                // cannot distinguish "no retrace" from "retraces fine, guest
                // never swapped" -- and those have opposite root causes. And
                // `scanout` next to it, because WM2000 never calls
                // `osViSwapBuffer` at all: it flips VI_ORIGIN directly, so
                // `vi_swaps` stays 0 for a run that is displaying fine.
                let (graphics_tasks, audio_tasks) = fn64_abi::task_counts();
                println!(
                    "[wm2000-shell] controller input_edge port={port} read={} buttons={:#06x} \
                     stick=({}, {}) sim_time={} scanout={:?} vi_swaps={} gfx_submits={} \
                     audio_submits={}",
                    self.read_ordinals[port],
                    input.button,
                    input.stick_x,
                    input.stick_y,
                    fn64_abi::sim_time(),
                    fn64_abi::scanout_vi_framebuffer(),
                    fn64_abi::vi_swap_count(),
                    graphics_tasks,
                    audio_tasks,
                );
            }
        }
    }
}

impl Shell {
    fn boot() -> Self {
        let rom_path = std::env::var("ROM").expect("ROM env var (same contract as build.rs)");
        println!("[wm2000-shell] loading ROM from {rom_path}");
        let rom = std::fs::read(&rom_path).expect("reading ROM");
        let boot_context_path = std::env::var("FN64_BOOT_CONTEXT")
            .expect("FN64_BOOT_CONTEXT must name a black-box header-handoff capture");
        let boot_context = fn64_boot_harness::load_boot_context(
            std::path::Path::new(&boot_context_path),
            &rom,
            fn64_boot_harness::TvType::Ntsc,
        )
        .unwrap_or_else(|error| panic!("loading NWXE BootContext: {error}"));
        FIRST_ENTRY_BOOT_CONTEXT.with(|slot| *slot.borrow_mut() = Some(boot_context.clone()));

        println!(
            "[wm2000-shell] discovered pack: {} static-prefix shards + {} resident-tail shards, \
             entry {:#010X}; captured exception images={}",
            pack::BOOT_SHARDS.len(),
            pack::RESIDENT_TAIL_SHARDS.len(),
            pack::ENTRYPOINT,
            pack::EXTERNAL_EXECUTABLE_IMAGES.len(),
        );

        fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
        fn64_abi::load_rom(rom.clone());
        fn64_abi::set_guest_running_thread_global(pack::OS_RUNNING_THREAD);
        // An interactive session runs for minutes, not a bounded probe. The
        // per-step executor/device traces are diagnostic buffers that grow
        // without bound; leaving them on would make the window stutter and then
        // exhaust memory. Opt back in with the same env vars the batch lane uses.
        if std::env::var_os("FN64_BLOCK_EXECUTOR_TRACE").is_none() {
            fn64_abi::set_trace_enabled(false);
        }
        if std::env::var_os("FN64_BLOCK_DEVICE_TRACE").is_none() {
            fn64_abi::set_device_trace_enabled(false);
        }
        fn64_abi::set_audio_task_lle_accuracy();
        wire_audio();
        // NWXE verifies SRAM with domain-2 PI writes during boot; omitting the
        // device is a harness error, so install the same typed in-memory 32 KiB
        // store the batch lane uses.
        fn64_abi::set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::SramBanked,
        )));

        let active_renderer = Self::register_render_backend();

        let mut program = fn64_recomp_rs::BlockProgram::new();
        // Destination history is a diagnostic ring the batch lane reads after a
        // bounded run. An interactive session never reads it and would only pay
        // for it every dispatch.
        program.set_execution_destination_history_enabled(false);

        // THE shared construction: identical shards, identical generation
        // catalog, identical attestation to the certified batch lane.
        let ConstructedCatalogProgram {
            catalog_program,
            generation_catalog,
            generation_backings,
            program_evidence,
            ..
        } = construct_catalog_program(
            program,
            GateRunners {
                dense_instrumentation: None,
                entry_context: run_entry_aot_with_context_gate as GeneratedBankFn,
                overlay_generation: run_overlay_aot_with_generation_gate as GeneratedBankFn,
                external_digest: run_nwxe_exception_image_with_digest_gate as GeneratedBankFn,
            },
            InstructionBudget::new(4096).expect("nonzero budget"),
        );
        let program_artifact = program_evidence
            .identity
            .identity
            .bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        println!("[wm2000-shell] canonical program artifact={program_artifact}");

        let resolver = fn64_abi::recompiled::CatalogResolverInstallV1::new_with_abi_host_catalog(
            catalog_program,
            issue_wm2000_host_function_catalog(),
            ProgramArtifactIdentity::new(pack::DISPATCH_SOURCE_SHA256),
        );
        let generations =
            BackedPrecompiledGenerationCatalogV1::new(generation_catalog, generation_backings)
                .expect("every generated dynamic generation has one exact physical backing");
        let install = fn64_abi::recompiled::CatalogGenerationInstallV1::new(resolver, generations)
            .expect("canonical resolver admits every generated dynamic shard");

        // The bootstrap transaction validates ROM, catalog, entry image, and
        // the executable-memory baseline, then MOVES its owned RDRAM into the
        // runtime. After `commit`, this process no longer holds the guest's
        // memory -- see `present`.
        let mut bootstrap = install
            .begin_bootstrap_import_v1(&rom, fn64_recomp_rs::RDRAM_LEN, fn64_runtime::TvType::Ntsc)
            .expect("creating canonical bootstrap transaction");
        bootstrap
            .publish_ipl3_cartridge_dma()
            .expect("publishing the typed IPL3 one-MiB cartridge DMA");
        let validated = bootstrap
            .commit()
            .expect("validating ROM, catalog, entry image, and executable-memory baseline");

        println!("[wm2000-shell] booting thread 0 from the discovered pack...");
        fn64_abi::recompiled::boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            boot_context,
            0,
            10,
        )
        .expect("booting canonical program from validated owned RDRAM");

        Shell {
            pad: PadState::new(),
            config: InputConfig::load(),
            gamepads: Gamepads::new(),
            rgba: vec![0u8; FB_WIDTH * FB_HEIGHT * 4],
            fb_width: FB_WIDTH,
            fb_height: FB_HEIGHT,
            window: None,
            pixels: None,
            reported_first_frame: false,
            presented_frames: 0,
            last_heartbeat_frame: 0,
            next_frame_deadline: std::time::Instant::now(),
            frame_intervals: TimingWindow::default(),
            pump_times: TimingWindow::default(),
            entered_first_dispatch: false,
            schedule: std::env::var_os("FN64_CONTROLLER_SCHEDULE")
                .map(|path| ScheduleDriver::load(std::path::Path::new(&path))),
            active_renderer,
        }
    }

    /// Select and register the rasterizer named by `FN64_RENDER`.
    ///
    /// REFERENCE IS THE DEFAULT and must stay so: it is what the owner plays,
    /// and every committed route and recorded measurement on this lane was
    /// taken against it.
    ///
    /// An unrecognized value is a hard error rather than a silent fallback.
    /// That is the whole point of this function existing. Until now `shell.rs`
    /// hardcoded `ReferenceBackend` and contained zero occurrences of
    /// `FN64_RENDER`, so `FN64_RENDER=rt64` on the window was a SILENT no-op --
    /// it did not error, did not warn, and rendered with the software backend
    /// regardless. Anyone who set it, saw the game run, and concluded "RT64
    /// works windowed" was reasonably but wrongly served, and that trap cost
    /// real confusion (`docs/plans/perf-method.md`, the caveat section). A
    /// backend selector whose off lane is silently the on lane is worse than no
    /// selector.
    fn register_render_backend() -> ActiveRenderer {
        use fn64_render::RenderBackend as _;
        let requested = fn64_boot_harness::parse_release_env_value(
            "FN64_RENDER",
            std::env::var_os("FN64_RENDER"),
        )
        .unwrap_or_else(|error| panic!("wm2000-shell: {error}"))
        .unwrap_or_else(|| "reference".to_string())
        .to_ascii_lowercase();

        let (backend, active): (Box<dyn fn64_render::RenderBackend>, ActiveRenderer) =
            match requested.as_str() {
                "reference" => {
                    let mut backend = fn64_render_reference::ReferenceBackend::new()
                        .with_f3dex2()
                        .with_clear_color([0, 0, 0, 255]);
                    backend
                        .create(&fn64_render::RenderConfig::ntsc(
                            FB_WIDTH as u32,
                            FB_HEIGHT as u32,
                        ))
                        .expect("ReferenceBackend create must be infallible for 320x240");
                    (Box::new(backend), ActiveRenderer::Reference)
                }
                "rt64" => {
                    // RT64 needs a GPU, a Metal system-default device, and a
                    // real NSWindow, plus initialization on the macOS MAIN
                    // THREAD. This shell satisfies that: it contains no
                    // `std::thread::spawn` at all, and `Shell::new` runs from
                    // `main` before `event_loop.run_app`, so registration and
                    // every later present happen on the main thread.
                    //
                    // Failure here is loud on purpose. A fallback to reference
                    // would report software-rasterizer behavior and timing
                    // under an RT64 label, which is the one outcome this
                    // selector must not be able to produce.
                    let mut backend = fn64_render_rt64::Rt64Backend::new();
                    backend
                        .create(&fn64_render::RenderConfig::ntsc(
                            FB_WIDTH as u32,
                            FB_HEIGHT as u32,
                        ))
                        .unwrap_or_else(|error| {
                            panic!(
                                "wm2000-shell: FN64_RENDER=rt64 requires a working native RT64 \
                                 adapter (build with --features rt64 and FN64_RT64_DIR set, and \
                                 run with a GPU/display available); create failed: {error}"
                            )
                        });
                    // Without this the post-VI render target is never retained
                    // and `capture_render_release_frame` has nothing to return,
                    // so the window would open and draw nothing.
                    backend.enable_present_capture().unwrap_or_else(|error| {
                        panic!(
                            "wm2000-shell: FN64_RENDER=rt64 needs post-VI present capture to get \
                             pixels back into the window; enable failed: {error}"
                        )
                    });
                    (Box::new(backend), ActiveRenderer::Rt64)
                }
                value => panic!(
                    "wm2000-shell: FN64_RENDER must be reference or rt64, got {value:?}. \
                     (Unset it for the default reference backend.)"
                ),
            };

        fn64_abi::set_render_backend_with_policy(
            backend,
            fn64_recomp_rs::RDRAM_LEN,
            fn64_abi::GraphicsTaskExecutionPolicy::HleOptimized,
        );
        let label = match active {
            ActiveRenderer::Reference => "reference",
            ActiveRenderer::Rt64 => "rt64",
        };
        println!("[wm2000-shell] registered {label} renderer ({FB_WIDTH}x{FB_HEIGHT})");
        active
    }

    /// Run the guest until it produces one VI field, then return.
    ///
    /// This does NOT copy `crates/fn64-shell`'s "advance the clock one field at
    /// the top, then drain" rule, and the difference is load-bearing twice
    /// over.
    ///
    /// First, the catalog seam validates the restored CPU state against the
    /// black-box `BootContext` at the first generated-bank entry, and CP0
    /// `Count` is part of that state. Advancing virtual time before the guest's
    /// first instruction ticks `Count` by a full field (781,250 cycles at
    /// 46.875 MHz), so the gate reports a `Cop0(9)` mismatch and refuses to
    /// boot -- correctly, because the context really would no longer be the
    /// captured one.
    ///
    /// Second, WM2000's boot is device-bound, not compute-bound: it blocks on PI
    /// DMAs and SI transactions many times per displayed frame. Advancing a
    /// fixed one field per pump grants exactly one device deadline per wall
    /// frame, so boot crawls (measured: 120 fixed-field pumps did not finish in
    /// ten minutes). Draining to the next DEVICE event instead -- the same
    /// `fn64_boot_harness::GuestDrain` the certified batch runner uses -- lets
    /// the guest service every pending transaction, and the VI field falls out
    /// of the device schedule naturally rather than being imposed on it.
    ///
    /// Returns when a retrace lands (one frame to present) or when the step /
    /// device-advance budget is exhausted, so the window never stops pumping
    /// events for longer than a frame's worth of work.
    fn pump_one_frame(&mut self) -> RetraceOutcome {
        // Scanout origin, not `osViSwapBuffer` count -- see `present`. A game
        // that programs VI_ORIGIN directly reports `vi_swap_count() == 0`
        // forever, which would pin the boot step budget on every frame and
        // report `swapped: false` on every frame that in fact flipped buffers.
        let start_origin = fn64_abi::scanout_vi_framebuffer();
        let mut drain = fn64_boot_harness::GuestDrain::default();
        let mut steps = 0u64;
        let mut device_advances = 0u32;
        self.entered_first_dispatch = true;

        // Per-arm attribution, off unless `FN64_SHELL_SPIKE_MS` is set. When
        // off, `spike` is `None` and the only added cost per pump is one
        // `OnceLock` read plus a null check per loop iteration.
        let spike_ms = spike_threshold_ms();
        let mut spike = (spike_ms > 0.0).then(|| {
            (
                std::time::Instant::now(),
                PumpCounters::read(),
                0.0f64, // accumulated step_ms
                0.0f64, // accumulated advance_ms
            )
        });

        loop {
            match drain.before_step(fn64_abi::next_runnable_priority()) {
                fn64_boot_harness::DrainDecision::Step => {
                    // Boot gets the generous bound; every later frame gets the
                    // steady-state one. A programmed scanout origin is the
                    // boundary because it marks the first field the window can
                    // actually show.
                    let budget = if fn64_abi::scanout_vi_framebuffer().is_none() {
                        BOOT_STEPS_PER_PUMP
                    } else {
                        STEPS_PER_PUMP
                    };
                    assert!(
                        steps < budget,
                        "wm2000-shell: non-idle guest work exceeded {budget} scheduling steps in one frame pump"
                    );
                    // Feed input before the game polls the controller this
                    // step, so a press is visible within the frame it happened.
                    // A committed route, when supplied, owns the pad outright:
                    // mixing it with a live gamepad would make the replay
                    // depend on whatever hardware happened to be attached.
                    match self.schedule.as_mut() {
                        Some(driver) => {
                            driver.observe_reads();
                            driver.apply();
                        }
                        None => {
                            let (buttons, sx, sy) = self.merged_input();
                            fn64_abi::set_controller_state(0, buttons, sx, sy);
                        }
                    }
                    let next_priority = fn64_abi::next_runnable_priority();
                    let arm_started = spike.as_ref().map(|_| std::time::Instant::now());
                    assert!(fn64_abi::run_one_step());
                    if let (Some(started), Some(spike)) = (arm_started, spike.as_mut()) {
                        spike.2 += started.elapsed().as_secs_f64() * 1000.0;
                    }
                    drain.record_step(next_priority.expect("drain authorized a runnable step"));
                    steps += 1;
                }
                fn64_boot_harness::DrainDecision::AdvanceField => {
                    let arm_started = spike.as_ref().map(|_| std::time::Instant::now());
                    let advanced = drain.advance_to_next_device_event();
                    if let (Some(started), Some(spike)) = (arm_started, spike.as_mut()) {
                        spike.3 += started.elapsed().as_secs_f64() * 1000.0;
                    }
                    device_advances += 1;
                    if matches!(advanced, fn64_boot_harness::DeviceAdvance::ViFields { .. }) {
                        // One field produced: hand the frame back to the window.
                        break;
                    }
                    // A quiescent guest with a long device queue must not hold
                    // the event loop indefinitely; give winit a turn and resume
                    // on the next deadline.
                    if device_advances >= DEVICE_ADVANCES_PER_PUMP {
                        break;
                    }
                }
            }
        }
        if let Some((started, before, step_ms, advance_ms)) = spike {
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            if total_ms >= spike_ms {
                PumpAttribution {
                    steps,
                    advances: device_advances,
                    step_ms,
                    advance_ms,
                    before,
                    after: PumpCounters::read(),
                }
                .report(total_ms, self.presented_frames);
            }
        }
        RetraceOutcome {
            swapped: fn64_abi::scanout_vi_framebuffer() != start_origin,
            steps,
        }
    }

    fn merged_input(&self) -> (u16, i8, i8) {
        let (kb_buttons, kb_x, kb_y) = self.pad.resolve();
        let (gp_buttons, gp_x, gp_y) = self.gamepads.resolve(&self.config);
        let (sx, sy) = if gp_x != 0 || gp_y != 0 {
            (gp_x, gp_y)
        } else {
            (kb_x, kb_y)
        };
        (kb_buttons | gp_buttons, sx, sy)
    }

    /// Blit the current VI framebuffer to the window.
    ///
    /// Unlike `crates/fn64-shell`, this cannot index a local `Vec<u8>`: the
    /// validated bootstrap moved RDRAM into the runtime. It borrows the device
    /// back for the duration of the decode through
    /// `fn64_abi::with_registered_physical_rdram_read`, which is safe here
    /// precisely because presentation happens between pumps -- no guest
    /// coroutine or device operation is mid-flight.
    ///
    /// Two present paths, chosen by `active_renderer`, because the two backends
    /// put their output in different places. See [`Self::present_rt64`].
    fn present(&mut self) {
        match self.active_renderer {
            ActiveRenderer::Reference => self.present_reference(),
            ActiveRenderer::Rt64 => self.present_rt64(),
        }
    }

    /// Present RT64's post-VI image.
    ///
    /// RT64 rasterizes into its own GPU render targets and does NOT write the
    /// finished image back to `rdram[output_addr..]`, so the RGBA5551 decode in
    /// [`Self::present_reference`] would read a stale -- in practice black --
    /// buffer on every frame. That is blocker C in
    /// `docs/plans/rt64-on-the-block-lane.md`, and it is real for the window in
    /// a way it is not for the headless lane.
    ///
    /// The pixels come back through `fn64_abi::capture_render_release_frame`,
    /// NOT through `Rt64Backend::presented_pixels`. That distinction is
    /// load-bearing and easy to get wrong: `set_render_backend_with_policy`
    /// MOVES the backend into `fn64-abi`, `with_render_backend` is
    /// `pub(crate)`, and `RenderBackend` carries no `Any`/downcast -- so the
    /// concrete `Rt64Backend` is simply not reachable from here after
    /// registration. The ABI seam is the sanctioned route and says so in as
    /// many words ("a host neither downcasts the backend nor reaches into RT64
    /// after registration", `lifecycle.rs:602-606`); `examples/wm2000-boot`
    /// already presents RT64 frames through it. It also normalizes RT64's
    /// native format to BGRA8 for us.
    ///
    /// Geometry comes from the capture, never from `vi_width()`. RT64 renders
    /// at its own internal resolution, which is generally NOT the guest's
    /// 320x240 VI framebuffer, and it returns a row stride (`row_bytes`) that
    /// may exceed the visible width. Reusing the reference path's `fb_width`
    /// and `stride` assumptions here would tear or panic.
    fn present_rt64(&mut self) {
        let capture = match fn64_abi::capture_render_release_frame() {
            Ok(capture) => capture,
            // Before the guest's first completed VI present there is simply no
            // image yet. That is an ordinary early-boot state, not an error:
            // stay silent and let the next frame try again.
            Err(fn64_render::RenderError::NotReady(_)) => return,
            Err(error) => {
                eprintln!("[wm2000-shell] RT64 present capture failed: {error}");
                return;
            }
        };
        assert_eq!(
            capture.format,
            fn64_render::ReleaseCaptureFormat::PostViBgra8Unorm,
            "wm2000-shell: RT64 returned an unsupported post-VI pixel format"
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
            "wm2000-shell: RT64 capture row stride {row_bytes} is smaller than {width} BGRA8 pixels"
        );
        assert_eq!(
            capture.bytes.len(),
            row_bytes
                .checked_mul(height)
                .expect("RT64 capture byte length overflow"),
            "wm2000-shell: RT64 capture byte length does not match its declared geometry"
        );
        if width == 0 || height == 0 {
            return;
        }

        // Resize the surface to RT64's OWN resolution the first time it is seen
        // or whenever it changes. The reference path sizes from `vi_width()`;
        // that value describes the guest framebuffer and has no authority over
        // RT64's internal target.
        if width != self.fb_width || height != self.fb_height {
            let Some(pixels) = self.pixels.as_mut() else {
                return;
            };
            if pixels
                .resize_buffer(capture.width, capture.height)
                .is_err()
            {
                return;
            }
            self.fb_width = width;
            self.fb_height = height;
            self.rgba = vec![0u8; width * height * 4];
            println!(
                "[wm2000-shell] resized present surface to {width}x{height} (RT64 internal \
                 render resolution, not the guest's VI framebuffer)"
            );
        }

        // BGRA -> RGBA, dropping any stride padding past the visible width.
        let mut uniform_probe: Option<[u8; 4]> = None;
        let mut blank = true;
        for (row, source) in capture.bytes.chunks_exact(row_bytes).enumerate() {
            let destination = &mut self.rgba[row * width * 4..(row + 1) * width * 4];
            for (out, bgra) in destination
                .chunks_exact_mut(4)
                .zip(source[..visible_row_bytes].chunks_exact(4))
            {
                // N64's alpha is coverage, not window transparency -- force it
                // opaque exactly as the reference path does.
                let rgba = [bgra[2], bgra[1], bgra[0], 255];
                match uniform_probe {
                    None => uniform_probe = Some(rgba),
                    Some(first) if first != rgba => blank = false,
                    Some(_) => {}
                }
                out.copy_from_slice(&rgba);
            }
        }

        let Some(pixels) = self.pixels.as_mut() else {
            return;
        };
        pixels.frame_mut().copy_from_slice(&self.rgba);
        if let Err(e) = pixels.render() {
            eprintln!("[wm2000-shell] pixels.render() failed: {e}");
            return;
        }
        self.presented_frames += 1;
        let source = format!(
            "rt64_post_vi={width}x{height} present={} workload={}",
            capture.present_id, capture.workload_id
        );
        let detail = format!("row_bytes={row_bytes} backend={}", capture.backend_identity);
        self.report_presented_frame(blank, &source, &detail);
    }

    fn present_reference(&mut self) {
        // Read VI_ORIGIN, not `osViSwapBuffer` bookkeeping.
        //
        // WM2000 never calls `osViSwapBuffer`; it double-buffers by writing
        // VI_ORIGIN through raw MMIO (measured: it alternates 0x0038fbc0 and
        // 0x003c7fc0 every field). Keying the present on
        // `current_vi_framebuffer()` therefore returned `None` on every frame
        // of every run, so this function bailed before touching the window --
        // which is why the window never showed a pixel while the headless
        // dump lane, which hashes RDRAM directly, produced 2,239 good frames
        // of the same run. `vi_swaps=0` was never a progress failure; it is
        // the correct count of a libultra call this game does not make.
        let Some(fb_offset) = fn64_abi::scanout_vi_framebuffer() else {
            return;
        };
        let fb_offset = fb_offset as usize;
        // The `^ 2` halfword decode assumes a word-aligned base (every real VI
        // framebuffer is). Loud, not silent, if that ever breaks.
        if fb_offset % 4 != 0 {
            eprintln!(
                "[wm2000-shell] VI framebuffer at {fb_offset:#x} is not word-aligned -- \
                 skipping present (decode assumption violated)"
            );
            return;
        }
        let src_stride = fn64_abi::vi_width().map_or(FB_WIDTH, |w| w as usize);
        let target_width = src_stride.clamp(1, 4096);
        if target_width != self.fb_width {
            if let Some(pixels) = self.pixels.as_mut() {
                if pixels
                    .resize_buffer(target_width as u32, FB_HEIGHT as u32)
                    .is_ok()
                {
                    self.fb_width = target_width;
                    self.fb_height = FB_HEIGHT;
                    self.rgba = vec![0u8; target_width * FB_HEIGHT * 4];
                    println!(
                        "[wm2000-shell] resized present surface to {target_width}x{FB_HEIGHT} \
                         (game VI_WIDTH); window shows the full framebuffer line."
                    );
                }
            }
        }

        // Decode straight out of runtime-owned RDRAM into the scratch buffer.
        //
        // Read WORDS, not halfwords. `PhysicalRdramRead::read_u16` applies the
        // `^ 2` lane swizzle and bounds-checks every pixel individually; at
        // 480x240 that is 115,200 checked reads per frame, sixty times a
        // second. An aligned `read_u32` has `lane_xor == 0` and yields the
        // pixel PAIR in one check -- the high halfword is the EVEN pixel,
        // because fn64's rdram stores native-endian words and the `^ 2`
        // swizzle is exactly what compensates for that in the halfword path.
        // Same pixels, half the reads.
        let fb_width = self.fb_width;
        let rgba = &mut self.rgba;
        let stride = src_stride.max(1);
        let decoded = fn64_abi::with_registered_physical_rdram_read(|memory| {
            let copy_width = fb_width.min(stride);
            let mut written = 0usize;
            let mut uniform_probe: Option<u16> = None;
            let mut uniform = true;
            for row in 0..FB_HEIGHT {
                let row_first_pixel = row * stride;
                let mut col = 0usize;
                while col < copy_width {
                    let pixel_index = row_first_pixel + col;
                    let byte_offset = fb_offset + pixel_index * 2;
                    // Word-align the read; an odd starting pixel takes the low
                    // halfword of the word that contains it.
                    let word_offset = byte_offset & !3;
                    if word_offset + 4 > memory.len() {
                        return (written, uniform);
                    }
                    let word = memory.read_u32(fn64_runtime::RdramAddr::from_offset(
                        u32::try_from(word_offset).expect("framebuffer offset exceeds u32"),
                    ));
                    // Even pixel in the high halfword, odd pixel in the low.
                    let pair = [(word >> 16) as u16, word as u16];
                    let first_lane = usize::from(byte_offset & 2 != 0);
                    for (lane, px) in pair.iter().enumerate().skip(first_lane) {
                        if col >= copy_width {
                            break;
                        }
                        let _ = lane;
                        let px = *px;
                        match uniform_probe {
                            None => uniform_probe = Some(px),
                            Some(first) if first != px => uniform = false,
                            Some(_) => {}
                        }
                        let o = (row * fb_width + col) * 4;
                        rgba[o] = expand5((px >> 11) & 0x1F);
                        rgba[o + 1] = expand5((px >> 6) & 0x1F);
                        rgba[o + 2] = expand5((px >> 1) & 0x1F);
                        // N64's 1-bit alpha is coverage, not window transparency.
                        rgba[o + 3] = 255;
                        written += 1;
                        col += 1;
                    }
                }
            }
            (written, uniform)
        });
        let Some((_written, blank)) = decoded else {
            // No RDRAM registered yet: honest no-op rather than a black frame
            // implying the game rendered one.
            return;
        };

        let Some(pixels) = self.pixels.as_mut() else {
            return;
        };
        pixels.frame_mut().copy_from_slice(&self.rgba);
        if let Err(e) = pixels.render() {
            eprintln!("[wm2000-shell] pixels.render() failed: {e}");
            return;
        }

        self.presented_frames += 1;
        let origin = format!("origin={fb_offset:#010x}");
        let first_frame_detail = format!("vi_width={src_stride}");
        self.report_presented_frame(blank, &origin, &first_frame_detail);
    }

    /// Emit the first-frame line and the 60-frame heartbeat.
    ///
    /// Shared by both present paths ON PURPOSE. `pump_ms` and the late-frame
    /// fraction reported here are how the two backends get compared, so they
    /// must be the same statistic computed the same way -- a heartbeat that
    /// differed per backend would make the comparison a measurement of the
    /// logging. `source` names where the pixels came from, which is the only
    /// part that legitimately differs.
    fn report_presented_frame(&mut self, blank: bool, source: &str, first_frame_detail: &str) {
        let frames = self.presented_frames;
        let swaps = fn64_abi::vi_swap_count();
        let rgba_hash = framebuffer::rgba_hash(&self.rgba);
        if !self.reported_first_frame {
            if blank {
                println!(
                    "[wm2000-shell] presenting {source} \
                     (osViSwapBuffer calls so far: {swaps}) -- currently BLANK/uniform (the game \
                     has not rendered visible geometry yet). Window + present path are live."
                );
            } else {
                println!(
                    "[wm2000-shell] presenting {source} \
                     (osViSwapBuffer calls so far: {swaps}) -- non-uniform, \
                     rgba_hash={rgba_hash:016x} (a comparison key, not a correctness claim); \
                     {first_frame_detail}"
                );
            }
            self.reported_first_frame = true;
        } else if frames >= self.last_heartbeat_frame + 60 {
            let state = if blank { "uniform" } else { "non-uniform" };
            let audio = fn64_abi::audio_output_stats();
            let interval = self.frame_intervals.take_stats();
            let pump = self.pump_times.take_stats();
            // Report the whole distribution, and say outright whether the
            // window held 60fps. The acceptance bar is a WORST-CASE bound, so
            // `max` is the statistic that can falsify it -- a median of 8 ms
            // next to a max of 45 ms is a miss, and median/p95 alone hide that.
            let fmt = |s: &Option<timing::TimingStats>| {
                s.as_ref().map_or_else(
                    || "none".to_string(),
                    |s| {
                        format!(
                            "n={} p50={:.1} p95={:.1} p99={:.1} max={:.1}{}",
                            s.samples,
                            s.median_ms,
                            s.p95_ms,
                            s.p99_ms,
                            s.max_ms,
                            if s.holds_60fps() { "" } else { " OVER-16.7ms" },
                        )
                    },
                )
            };
            println!(
                "[wm2000-shell] heartbeat: presented frame #{frames} {source} \
                 ({state}, rgba_hash={rgba_hash:016x}; visual correctness not inferred); \
                 osViSwapBuffer calls={swaps}; frame_interval_ms[{}] pump_ms[{}]; \
                 audio{}: ai_buffers={} samples={} nonzero={} backend_buffers={}",
                fmt(&interval),
                fmt(&pump),
                // A zero audio counter has two completely different meanings and
                // the log body cannot tell them apart. On 2026-08-07 a run with
                // `FN64_NO_AUDIO=1` printed `ai_buffers=0 samples=0` and that was
                // recorded in the blocker ledger as "audio is silent, MIT-clean
                // HLE microcode remains a real project" -- when the same binary
                // without the flag delivers 1,898 buffers and 1.69M nonzero
                // samples. `FN64_NO_AUDIO` short-circuits `wire_audio` below,
                // leaving `AUDIO_RDRAM_LEN` at 0, which makes
                // `apply_live_ai_write_effect` skip `deliver_ai_buffer` entirely:
                // every counter here is then pinned at zero BY REQUEST, in a run
                // where the guest is synthesizing PCM the whole time. Say so on
                // the same line as the numbers, so the disclaimer cannot be
                // separated from the zero it explains.
                if audio_disabled_by_request() {
                    " (DISABLED by FN64_NO_AUDIO -- zeros below are the flag, not the guest)"
                } else {
                    ""
                },
                audio.ai_buffers,
                audio.samples,
                audio.nonzero_samples,
                audio.backend_buffers,
            );
            self.last_heartbeat_frame = frames;
        }
    }
}

/// Expand a 5-bit channel to 8-bit with rounding -- the same `(v*255+15)/31`
/// expansion the shared `framebuffer` module and oot-boot's PNG dump use, so a
/// frame presented here is byte-identical to one captured there.
#[inline]
fn expand5(v: u16) -> u8 {
    ((v * 255 + 15) / 31) as u8
}

/// Register a live cpal output stream. A create() failure (no device, headless
/// CI) is logged, not fatal: the window and input still work, only sound is
/// unavailable.
/// Whether this run asked for silence. Read by the heartbeat as well as
/// `wire_audio`, so a zeroed audio counter always carries its own explanation.
fn audio_disabled_by_request() -> bool {
    std::env::var_os("FN64_NO_AUDIO").is_some()
}

fn wire_audio() {
    if audio_disabled_by_request() {
        println!("[wm2000-shell] FN64_NO_AUDIO set -- audio output disabled");
        return;
    }
    use fn64_audio::{AudioBackend as _, AudioConfig, CpalBackend};
    const N64_BOOT_AI_RATE_HZ: u32 = 32_000;
    let mut backend = CpalBackend::new();
    match backend.create(&AudioConfig::new(N64_BOOT_AI_RATE_HZ, 2)) {
        Ok(()) => {
            let stream_rate = backend.stream_rate_hz().unwrap_or(N64_BOOT_AI_RATE_HZ);
            fn64_abi::set_audio_backend(Box::new(backend), fn64_recomp_rs::RDRAM_LEN);
            println!(
                "[wm2000-shell] audio output wired (cpal, guest {N64_BOOT_AI_RATE_HZ} Hz -> \
                 stream {stream_rate} Hz stereo)"
            );
        }
        Err(e) => {
            eprintln!(
                "[wm2000-shell] audio output unavailable ({e}) -- continuing SILENT \
                 (window/input unaffected). Set FN64_NO_AUDIO to silence this."
            );
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
            .with_title("fn64 -- WM2000 (dense AOT block program)")
            .with_inner_size(size)
            .with_min_inner_size(LogicalSize::new(FB_WIDTH as f64, FB_HEIGHT as f64));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("[wm2000-shell] failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let win_size = window.inner_size();
        let surface = SurfaceTexture::new(win_size.width, win_size.height, Arc::clone(&window));
        match Pixels::new(self.fb_width as u32, self.fb_height as u32, surface) {
            Ok(px) => {
                self.pixels = Some(px);
                self.window = Some(window);
                println!(
                    "[wm2000-shell] window opened ({}x{})",
                    win_size.width, win_size.height
                );
            }
            Err(e) => {
                eprintln!("[wm2000-shell] failed to create pixels surface: {e}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                println!("[wm2000-shell] window close requested -- exiting");
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                if let Some(px) = self.pixels.as_mut() {
                    if let Err(e) = px.resize_surface(new_size.width.max(1), new_size.height.max(1))
                    {
                        eprintln!("[wm2000-shell] resize_surface failed: {e}");
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.repeat {
                    return;
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    let pressed = event.state == ElementState::Pressed;
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
                    if code == KeyCode::Escape && pressed {
                        println!("[wm2000-shell] Esc pressed -- exiting");
                        event_loop.exit();
                        return;
                    }
                    if self.pad.apply(&self.config, code, pressed) {
                        let (b, sx, sy) = self.pad.resolve();
                        println!(
                            "[wm2000-shell] input: key {code:?} {} -> pad buttons={b:#06x} \
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
        self.gamepads.poll();
        let _ = self.gamepads.take_pressed();

        const FRAME: std::time::Duration = std::time::Duration::from_nanos(16_666_667);
        let now_t = std::time::Instant::now();
        if now_t >= self.next_frame_deadline {
            self.frame_intervals
                .record(now_t.saturating_duration_since(
                    self.next_frame_deadline.checked_sub(FRAME).unwrap_or(now_t),
                ));
            self.pump_one_frame();
            self.pump_times.record(now_t.elapsed());
            // Hold cadence while we keep up; re-anchor (dropping missed frames)
            // when we fall behind, so a slow pump cannot spiral into catch-up.
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

fn main() {
    let mut shell = Shell::boot();

    // Headless seam self-test: drive N pumps with no window, so a machine with
    // no display can still prove boot + scheduling + framebuffer reads work.
    if let Some(frames) = std::env::var_os("FN64_SHELL_HEADLESS_FRAMES") {
        let frames: u64 = frames
            .to_string_lossy()
            .parse()
            .expect("FN64_SHELL_HEADLESS_FRAMES must be an unsigned integer");
        println!("[wm2000-shell] headless probe: pumping {frames} retraces without a window");
        for frame in 0..frames {
            let started = std::time::Instant::now();
            let outcome = shell.pump_one_frame();
            println!(
                "[wm2000-shell] probe frame {frame}: swapped={} steps={} wall_ms={:.1} \
                 vi_swaps={} sim_time={} scanout={:?} swap_fb={:?}",
                outcome.swapped,
                outcome.steps,
                started.elapsed().as_secs_f64() * 1000.0,
                fn64_abi::vi_swap_count(),
                fn64_abi::sim_time(),
                fn64_abi::scanout_vi_framebuffer(),
                fn64_abi::current_vi_framebuffer(),
            );
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
        }
        println!(
            "[wm2000-shell] headless probe done: vi_swaps={} sim_time={} scanout={:?} \
             swap_fb={:?} vi_width={:?}",
            fn64_abi::vi_swap_count(),
            fn64_abi::sim_time(),
            fn64_abi::scanout_vi_framebuffer(),
            fn64_abi::current_vi_framebuffer(),
            fn64_abi::vi_width(),
        );
        let exit = fn64_abi::prepare_process_exit();
        println!("[wm2000-shell] process exit prepared: {exit:?}");
        return;
    }

    let event_loop = EventLoop::new().expect("wm2000-shell: failed to build winit event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    if let Err(e) = event_loop.run_app(&mut shell) {
        eprintln!("[wm2000-shell] event loop error: {e}");
    }
    println!("[wm2000-shell] exited cleanly.");
    // Seal guest coroutine ownership before ordinary Rust/TLS teardown, so the
    // window, audio, and renderer owners can drop normally.
    let exit = fn64_abi::prepare_process_exit();
    println!(
        "[wm2000-shell] process exit prepared: threads={} detached_coroutines={}",
        exit.threads, exit.detached_coroutines
    );
}
