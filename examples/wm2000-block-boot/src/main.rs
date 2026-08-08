//! Boot WM2000 (NWXE) from fn64's OWN discovered Block Pack -- no
//! aki-recomp metadata, no N64Recomp C. `build.rs` ran discovery on the
//! user's ROM, then emitted dense arbitrary-PC resident and overlay runners.
//! Black-box image evidence is retained only for captured CPU-written
//! exception-vector images. This harness seals those artifacts with an exact
//! host catalog, physically backed generations, and a validated IPL3
//! publication in runtime-owned RDRAM, then drives the executor until the guest
//! either idles, reaches an unobserved PC, or reaches a runtime-behavior fault.

mod block_program;
mod dense_aot;
mod diagnostics;
mod runner_reports;
mod telemetry;
use block_program::*;
use dense_aot::*;
use diagnostics::*;
use runner_reports::*;
use telemetry::*;

use fn64_recomp_rs::{
    BackedExecutableSpanV1, BackedPrecompiledGenerationCatalogV1, BankId, BlockRun, BootContext,
    CargoGeneratedProgramSourceAttestationV2, CargoGeneratedRunnerSourceBindingV1,
    CatalogBlockProgramV1, CodeBank, ExecutableRegion, ExecutionKey, GeneratedAdapterRole,
    GeneratedBankFn, GeneratedBankRunner, GenerationId, GuestPc, InstructionBudget,
    PrecompiledGeneration, PrecompiledGenerationBackingV1, PrecompiledGenerationCatalog,
    PrecompiledShard, ProgramArtifactIdentity, Rdram, RecompContext,
};
use sha2::{Digest, Sha256};
use std::io::Write;

static SUPPRESS_PROTOCOL_DIAGNOSTICS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

macro_rules! println {
    ($($argument:tt)*) => {
        if !SUPPRESS_PROTOCOL_DIAGNOSTICS.load(std::sync::atomic::Ordering::Relaxed) {
            std::println!($($argument)*);
        }
    };
}

macro_rules! eprintln {
    ($($argument:tt)*) => {
        if !SUPPRESS_PROTOCOL_DIAGNOSTICS.load(std::sync::atomic::Ordering::Relaxed) {
            std::eprintln!($($argument)*);
        }
    };
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

fn code_bank_sha256(code: &CodeBank) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for span in code.spans() {
        for word in span.words() {
            hasher.update(word.to_be_bytes());
        }
    }
    hasher.finalize().into()
}

fn register_external_executable_generation(
    catalog: &mut PrecompiledGenerationCatalog,
    backings: &mut Vec<PrecompiledGenerationBackingV1>,
    bank: BankId,
    image_start: GuestPc,
    image_end: GuestPc,
    expected_sha256: [u8; 32],
) {
    // Capture ordinals are scoped to an image producer, while bank identities
    // are already collision-checked across the complete executable catalog.
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

struct ControllerScheduleDriver {
    schedule: fn64_boot_harness::ControllerInputSchedule,
    read_ordinals: [u64; 4],
    current_inputs: [fn64_runtime::ContInput; 4],
    operation_cursor: usize,
}

/// Live PCM evidence, kept as counters so a long run costs no memory.
///
/// The distinction these exist to make: a task SUBMISSION count proves the
/// dispatcher ran, while `AUDIO_NONZERO_SAMPLES` proves the RSP produced
/// actual signal. Audio that regressed to silence would leave the submission
/// count untouched and drive this to zero.
static AUDIO_BUFFERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static AUDIO_SAMPLES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static AUDIO_NONZERO_SAMPLES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static AUDIO_PEAK_ABS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A headless [`fn64_audio::AudioBackend`] that retains statistics instead of
/// samples.
///
/// The block lane has no host audio device, and opening one would make the
/// gate depend on machine state. Accepting every buffer and measuring it keeps
/// the delivery path exercised -- including the rate handling below -- while
/// staying deterministic and constant-space.
#[derive(Default)]
struct PcmEvidenceBackend {
    sample_rate_hz: u32,
}

impl fn64_audio::AudioBackend for PcmEvidenceBackend {
    fn create(&mut self, cfg: &fn64_audio::AudioConfig) -> Result<(), fn64_audio::AudioError> {
        self.sample_rate_hz = cfg.sample_rate_hz;
        Ok(())
    }

    fn queue_samples(&mut self, samples: &[i16]) -> Result<(), fn64_audio::AudioError> {
        use std::sync::atomic::Ordering::Relaxed;
        AUDIO_BUFFERS.fetch_add(1, Relaxed);
        AUDIO_SAMPLES.fetch_add(samples.len() as u64, Relaxed);
        let nonzero = samples.iter().filter(|sample| **sample != 0).count();
        AUDIO_NONZERO_SAMPLES.fetch_add(nonzero as u64, Relaxed);
        // i16::MIN negates out of range, so widen before taking magnitude.
        let peak = samples
            .iter()
            .map(|sample| (*sample as i32).unsigned_abs() as u64)
            .max()
            .unwrap_or(0);
        AUDIO_PEAK_ABS.fetch_max(peak, Relaxed);
        Ok(())
    }

    /// Nothing is retained, so nothing is ever pending. A real device reports
    /// its backlog here; this backend consuming instantly is what keeps the
    /// lane from stalling on delivery it does not perform.
    fn frames_remaining(&self) -> Result<u32, fn64_audio::AudioError> {
        Ok(0)
    }

    fn set_frequency(&mut self, sample_rate_hz: u32) {
        self.sample_rate_hz = sample_rate_hz;
    }

    fn stream_rate_hz(&self) -> Option<u32> {
        (self.sample_rate_hz != 0).then_some(self.sample_rate_hz)
    }
}

static PROBE_STEP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CONTROLLER_READ_ORDINALS: [std::sync::atomic::AtomicU64; 4] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 4];

impl ControllerScheduleDriver {
    fn load(path: &std::path::Path) -> Self {
        let source = std::fs::read(path).unwrap_or_else(|error| {
            panic!("reading controller schedule {}: {error}", path.display())
        });
        let schedule =
            fn64_boot_harness::parse_controller_input_schedule(&source).unwrap_or_else(|error| {
                panic!("parsing controller schedule {}: {error}", path.display())
            });
        let driven = schedule.driven_ports();
        println!(
            "[wm2000-block-boot] controller schedule={} phases={} driven_ports={driven:?} sha256={}",
            path.display(),
            schedule.phases().len(),
            schedule.source_sha256_hex(),
        );
        fn64_boot_harness::attach_controllers_for_driven_ports(driven);
        Self {
            schedule,
            read_ordinals: [0; 4],
            current_inputs: [fn64_runtime::ContInput::default(); 4],
            operation_cursor: fn64_abi::copy_controller_operations().len(),
        }
    }

    fn apply_current_inputs(&mut self) {
        for port in 0..4 {
            let input = self.schedule.input_for_read(port, self.read_ordinals[port]);
            if input != self.current_inputs[port] {
                fn64_abi::set_controller_state(port, input.button, input.stick_x, input.stick_y);
                self.current_inputs[port] = input;
                let (graphics_tasks, audio_tasks) = fn64_abi::task_counts();
                println!(
                    "[wm2000-block-boot] controller input_edge port={port} read={} buttons={:#06x} stick=({}, {}) step={} sim_time={} gfx_submits={} audio_submits={} generations={:?}",
                    self.read_ordinals[port],
                    input.button,
                    input.stick_x,
                    input.stick_y,
                    PROBE_STEP.load(std::sync::atomic::Ordering::Relaxed),
                    fn64_abi::sim_time(),
                    graphics_tasks,
                    audio_tasks,
                    entered_overlay_generation_ids(),
                );
            }
        }
    }

    fn observe_completed_operations(&mut self) {
        let operations = fn64_abi::copy_controller_operations_since(self.operation_cursor);
        self.operation_cursor += operations.len();
        for operation in operations {
            if operation.device == fn64_runtime::ControllerOperationDevice::StandardController
                && operation.operation == fn64_runtime::ControllerOperationKind::Read
            {
                let port = usize::from(operation.port);
                self.read_ordinals[port] = self.read_ordinals[port]
                    .checked_add(1)
                    .expect("controller read ordinal overflow");
                CONTROLLER_READ_ORDINALS[port].store(
                    self.read_ordinals[port],
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
        }
        self.apply_current_inputs();
    }

    const fn read_ordinals(&self) -> [u64; 4] {
        self.read_ordinals
    }
}

thread_local! {
    static FIRST_ENTRY_BOOT_CONTEXT: std::cell::RefCell<Option<BootContext>> = const {
        std::cell::RefCell::new(None)
    };
    static AOT_BANK_COUNTS: std::cell::RefCell<[u64; DENSE_AOT_ARTIFACTS.len()]> = const {
        std::cell::RefCell::new([0; DENSE_AOT_ARTIFACTS.len()])
    };
    static AOT_PC_COUNTS: std::cell::RefCell<Vec<u64>> = const {
        std::cell::RefCell::new(Vec::new())
    };
    static AOT_PC_FIRST_GPRS: std::cell::RefCell<Vec<Option<[u64; 14]>>> = const {
        std::cell::RefCell::new(Vec::new())
    };
    static AOT_PC_FIRST_SYSTEM: std::cell::RefCell<Vec<Option<[u64; 7]>>> = const {
        std::cell::RefCell::new(Vec::new())
    };
    static AOT_PC_LAST_GPRS: std::cell::RefCell<Vec<Option<[u64; 14]>>> = const {
        std::cell::RefCell::new(Vec::new())
    };
    static AOT_PC_LAST_SYSTEM: std::cell::RefCell<Vec<Option<[u64; 7]>>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

static PROFILE_AOT_BANKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
static PROFILE_AOT_PCS: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
static PROFILE_STOP_AT_AOT_PC: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
static PROFILE_STOP_AT_AOT_PC_REACHED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static PROFILE_STOP_AT_OVERLAY_GENERATION: std::sync::OnceLock<Option<u64>> =
    std::sync::OnceLock::new();
static PROFILE_STOP_AT_OVERLAY_GENERATION_REACHED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static LAST_AOT_ENTRY_PC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static LAST_AOT_ENTRY_BANK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static AOT_ENTRY_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static ENTERED_OVERLAY_GENERATION_BITS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

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

fn entered_overlay_generation_ids() -> Vec<u64> {
    let entered_bits = ENTERED_OVERLAY_GENERATION_BITS.load(std::sync::atomic::Ordering::Relaxed);
    pack::OVERLAY_GENERATIONS
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            entered_bits
                & 1u64
                    .checked_shl(u32::try_from(*index).expect("generation index fits u32"))
                    .expect("at most 64 recovered overlay generations")
                != 0
        })
        .map(|(_, generation)| generation.id)
        .collect()
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

fn run_dense_aot_with_context_gate(
    entry: ExecutionKey,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) -> BlockRun {
    LAST_AOT_ENTRY_PC.store(entry.pc.get(), std::sync::atomic::Ordering::Relaxed);
    LAST_AOT_ENTRY_BANK.store(entry.bank.get(), std::sync::atomic::Ordering::Relaxed);
    AOT_ENTRY_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if PROFILE_STOP_AT_AOT_PC
        .get()
        .copied()
        .flatten()
        .is_some_and(|pc| pc == entry.pc.get())
    {
        PROFILE_STOP_AT_AOT_PC_REACHED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    validate_first_entry_boot_context(entry, ctx);
    let (artifact_index, artifact) = DENSE_AOT_ARTIFACTS
        .iter()
        .enumerate()
        .find(|(_, artifact)| artifact.bank_id == entry.bank.get())
        .unwrap_or_else(|| panic!("no compiled dense-AOT artifact for {}", entry.bank));
    if *PROFILE_AOT_BANKS
        .get()
        .expect("AOT bank profiling mode is initialized before guest execution")
    {
        AOT_BANK_COUNTS.with(|counts| {
            counts.borrow_mut()[artifact_index] += 1;
        });
    }
    let watched_pcs = PROFILE_AOT_PCS
        .get()
        .expect("AOT PC profiling mode is initialized before guest execution");
    if !watched_pcs.is_empty() {
        AOT_PC_COUNTS.with(|counts| {
            let mut counts = counts.borrow_mut();
            if counts.is_empty() {
                counts.resize(watched_pcs.len(), 0);
            }
            for (index, watched_pc) in watched_pcs.iter().enumerate() {
                if entry.pc.get() == *watched_pc {
                    counts[index] += 1;
                    let stack_argument =
                        |offset| u64::from(mem.load_w(Rdram::eff_addr(ctx.r(29), offset)) as u32);
                    let gprs = [
                        ctx.r(2),
                        ctx.r(3),
                        ctx.r(4),
                        ctx.r(5),
                        ctx.r(6),
                        ctx.r(7),
                        ctx.r(29),
                        ctx.r(31),
                        stack_argument(16),
                        stack_argument(20),
                        stack_argument(24),
                        stack_argument(28),
                        stack_argument(32),
                        stack_argument(36),
                    ];
                    let system = [
                        u64::from(ctx.cop0_status),
                        u64::from(ctx.cop0_cause),
                        u64::from(ctx.cop0_epc),
                        ctx.cop0_badvaddr,
                        u64::from(ctx.read_fcr(31)),
                        ctx.d_bits(0),
                        ctx.d_bits(18),
                    ];
                    AOT_PC_FIRST_GPRS.with(|first_gprs| {
                        let mut first_gprs = first_gprs.borrow_mut();
                        if first_gprs.is_empty() {
                            first_gprs.resize(watched_pcs.len(), None);
                        }
                        first_gprs[index].get_or_insert(gprs);
                    });
                    AOT_PC_FIRST_SYSTEM.with(|first_system| {
                        let mut first_system = first_system.borrow_mut();
                        if first_system.is_empty() {
                            first_system.resize(watched_pcs.len(), None);
                        }
                        first_system[index].get_or_insert(system);
                    });
                    AOT_PC_LAST_GPRS.with(|last_gprs| {
                        let mut last_gprs = last_gprs.borrow_mut();
                        if last_gprs.is_empty() {
                            last_gprs.resize(watched_pcs.len(), None);
                        }
                        last_gprs[index] = Some(gprs);
                    });
                    AOT_PC_LAST_SYSTEM.with(|last_system| {
                        let mut last_system = last_system.borrow_mut();
                        if last_system.is_empty() {
                            last_system.resize(watched_pcs.len(), None);
                        }
                        last_system[index] = Some(system);
                    });
                }
            }
        });
    }
    if artifact_index >= pack::BOOT_SHARDS.len() + pack::RESIDENT_TAIL_SHARDS.len() {
        record_overlay_generation(entry.bank);
    }
    (artifact.runner)(entry, budget, ctx, mem)
}

fn validate_first_entry_boot_context(entry: ExecutionKey, ctx: &RecompContext) {
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
            println!("[wm2000-block-boot] first-entry BootContext matches exactly");
        }
    });
}

fn record_overlay_generation(bank: BankId) {
    let mut matching_generations =
        pack::OVERLAY_GENERATIONS
            .iter()
            .enumerate()
            .filter(|(_, generation)| {
                generation
                    .shards
                    .iter()
                    .any(|shard| shard.bank_id == bank.get())
            });
    let Some((generation_index, generation)) = matching_generations.next() else {
        panic!(
            "overlay AOT bank {:#018x} belongs to no recovered generation",
            bank.get()
        );
    };
    assert!(
        matching_generations.next().is_none(),
        "overlay AOT bank {:#018x} belongs to multiple recovered generations",
        bank.get()
    );
    let generation_bit = 1u64
        .checked_shl(u32::try_from(generation_index).expect("generation index fits u32"))
        .expect("at most 64 recovered overlay generations");
    let prior = ENTERED_OVERLAY_GENERATION_BITS
        .fetch_or(generation_bit, std::sync::atomic::Ordering::Relaxed);
    if prior & generation_bit == 0 {
        let (graphics_tasks, audio_tasks) = fn64_abi::task_counts();
        println!(
            "[wm2000-block-boot] first generation={} image=[{:#010x},{:#010x}) step={} sim_time={} controller_read0={} gfx_submits={} audio_submits={}",
            generation.id,
            generation.image_start,
            generation.image_end,
            PROBE_STEP.load(std::sync::atomic::Ordering::Relaxed),
            fn64_abi::sim_time(),
            CONTROLLER_READ_ORDINALS[0].load(std::sync::atomic::Ordering::Relaxed),
            graphics_tasks,
            audio_tasks,
        );
        if PROFILE_STOP_AT_OVERLAY_GENERATION
            .get()
            .copied()
            .flatten()
            .is_some_and(|target| target == generation.id)
        {
            PROFILE_STOP_AT_OVERLAY_GENERATION_REACHED
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

fn run_entry_aot_with_context_gate(
    entry: ExecutionKey,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) -> BlockRun {
    validate_first_entry_boot_context(entry, ctx);
    // Artifact 0, not a named crate. `block_program.rs` already selects this
    // gate by `artifact_index == 0`, and bank IDs are content-derived rather
    // than name-derived, so nothing here is title-specific except the symbol.
    (DENSE_AOT_ARTIFACTS[0].runner)(entry, budget, ctx, mem)
}

fn run_overlay_aot_with_generation_gate(
    entry: ExecutionKey,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) -> BlockRun {
    record_overlay_generation(entry.bank);
    let artifact = DENSE_AOT_ARTIFACTS
        .iter()
        .skip(pack::BOOT_SHARDS.len() + pack::RESIDENT_TAIL_SHARDS.len())
        .find(|artifact| artifact.bank_id == entry.bank.get())
        .unwrap_or_else(|| panic!("no compiled overlay AOT artifact for {}", entry.bank));
    (artifact.runner)(entry, budget, ctx, mem)
}

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

fn main() {
    let generated_runner_build_identity_mode = generated_runner_build_identity_mode();
    let generated_runner_bootstrap_audit_mode = generated_runner_bootstrap_audit_mode();
    let generated_runner_cpu_audit_mode = generated_runner_cpu_audit_mode();
    let generated_runner_host_abi_audit_mode = generated_runner_host_abi_audit_mode();
    let generated_runner_pi_audit_mode = generated_runner_pi_audit_mode();
    let generated_runner_rdp_renderer_audit_mode = generated_runner_rdp_renderer_audit_mode();
    let generated_runner_rsp_audit_mode = generated_runner_rsp_audit_mode();
    let generated_runner_si_audit_mode = generated_runner_si_audit_mode();
    let generated_runner_sp_audit_mode = generated_runner_sp_audit_mode();
    let dynamic_exact_entry_withheld = dynamic_exact_entry_withheld();
    let minimum_guest_instructions = std::env::var("FN64_BLOCK_MIN_GUEST_INSTRUCTIONS")
        .map(|value| {
            value
                .parse::<std::num::NonZeroU64>()
                .expect("FN64_BLOCK_MIN_GUEST_INSTRUCTIONS must be a positive integer")
                .get()
        })
        .ok();
    let expected_guest_instructions = std::env::var("FN64_BLOCK_EXPECT_GUEST_INSTRUCTIONS")
        .map(|value| {
            value
                .parse::<std::num::NonZeroU64>()
                .expect("FN64_BLOCK_EXPECT_GUEST_INSTRUCTIONS must be a positive integer")
                .get()
        })
        .ok();
    assert!(
        usize::from(generated_runner_build_identity_mode)
            + usize::from(generated_runner_bootstrap_audit_mode)
            + usize::from(generated_runner_cpu_audit_mode)
            + usize::from(generated_runner_host_abi_audit_mode)
            + usize::from(generated_runner_pi_audit_mode)
            + usize::from(generated_runner_rdp_renderer_audit_mode)
            + usize::from(generated_runner_rsp_audit_mode)
            + usize::from(generated_runner_si_audit_mode)
            + usize::from(generated_runner_sp_audit_mode)
            <= 1,
        "generated-runner protocol modes are mutually exclusive"
    );
    let generated_runner_protocol_mode = generated_runner_build_identity_mode
        || generated_runner_bootstrap_audit_mode
        || generated_runner_cpu_audit_mode
        || generated_runner_host_abi_audit_mode
        || generated_runner_pi_audit_mode
        || generated_runner_rdp_renderer_audit_mode
        || generated_runner_rsp_audit_mode
        || generated_runner_si_audit_mode
        || generated_runner_sp_audit_mode;
    assert!(
        !dynamic_exact_entry_withheld || !generated_runner_protocol_mode,
        "dynamic withheld execution is operational-only and cannot run in a generated-runner authority protocol"
    );
    assert!(
        !dynamic_exact_entry_withheld
            || std::env::var_os("FN64_BLOCK_PROGRESS_ONLY").is_some(),
        "dynamic withheld execution currently requires the bounded FN64_BLOCK_PROGRESS_ONLY contract"
    );
    assert!(
        expected_guest_instructions.is_none() || minimum_guest_instructions.is_some(),
        "FN64_BLOCK_EXPECT_GUEST_INSTRUCTIONS requires FN64_BLOCK_MIN_GUEST_INSTRUCTIONS"
    );
    if let (Some(minimum), Some(expected)) =
        (minimum_guest_instructions, expected_guest_instructions)
    {
        assert!(
            minimum <= expected,
            "FN64_BLOCK_EXPECT_GUEST_INSTRUCTIONS {expected} cannot be below FN64_BLOCK_MIN_GUEST_INSTRUCTIONS {minimum}"
        );
    }
    assert!(
        !dynamic_exact_entry_withheld || minimum_guest_instructions.is_some(),
        "dynamic withheld execution requires FN64_BLOCK_MIN_GUEST_INSTRUCTIONS"
    );
    #[cfg(feature = "dynamic-withheld")]
    let dynamic_telemetry_output = dynamic_exact_entry_withheld.then(prepare_dynamic_telemetry_output);
    SUPPRESS_PROTOCOL_DIAGNOSTICS.store(
        generated_runner_protocol_mode,
        std::sync::atomic::Ordering::Relaxed,
    );
    PROFILE_AOT_BANKS
        .set(std::env::var_os("FN64_PROFILE_AOT_BANKS").is_some())
        .expect("AOT bank profiling mode is initialized once");
    let watched_pcs = std::env::var("FN64_PROFILE_AOT_PCS")
        .map(|value| {
            value
                .split(',')
                .filter(|value| !value.trim().is_empty())
                .map(|value| {
                    let value = value.trim().trim_start_matches("0x");
                    u32::from_str_radix(value, 16)
                        .expect("FN64_PROFILE_AOT_PCS entries must be hexadecimal u32 PCs")
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (index, pc) in watched_pcs.iter().enumerate() {
        assert!(
            !watched_pcs[..index].contains(pc),
            "FN64_PROFILE_AOT_PCS contains duplicate {pc:#010x}"
        );
    }
    PROFILE_AOT_PCS
        .set(watched_pcs)
        .expect("AOT PC profiling mode is initialized once");
    let stop_at_aot_pc = std::env::var("FN64_PROFILE_STOP_AT_PC")
        .map(|value| {
            let value = value.trim().trim_start_matches("0x");
            u32::from_str_radix(value, 16)
                .expect("FN64_PROFILE_STOP_AT_PC must be one hexadecimal u32 PC")
        })
        .ok();
    PROFILE_STOP_AT_AOT_PC
        .set(stop_at_aot_pc)
        .expect("AOT stop-PC profiling mode is initialized once");
    let stop_at_overlay_generation = std::env::var("FN64_PROFILE_STOP_AT_GENERATION")
        .map(|value| {
            let value = value.trim();
            if let Some(value) = value.strip_prefix("0x") {
                u64::from_str_radix(value, 16).expect(
                    "FN64_PROFILE_STOP_AT_GENERATION must be one decimal or 0x-prefixed u64 generation ID",
                )
            } else {
                value.parse::<u64>().expect(
                    "FN64_PROFILE_STOP_AT_GENERATION must be one decimal or 0x-prefixed u64 generation ID",
                )
            }
        })
        .ok();
    if let Some(target) = stop_at_overlay_generation {
        assert!(
            pack::OVERLAY_GENERATIONS
                .iter()
                .any(|generation| generation.id == target),
            "FN64_PROFILE_STOP_AT_GENERATION={target} is absent from the recovered generation catalog"
        );
    }
    PROFILE_STOP_AT_OVERLAY_GENERATION
        .set(stop_at_overlay_generation)
        .expect("overlay-generation stop mode is initialized once");
    let full_aot_instrumentation = !generated_runner_protocol_mode
        && (*PROFILE_AOT_BANKS.get().unwrap()
            || !PROFILE_AOT_PCS.get().unwrap().is_empty()
            || PROFILE_STOP_AT_AOT_PC.get().unwrap().is_some()
            || std::env::var_os("FN64_BLOCK_WATCHDOG").is_some());
    if !generated_runner_protocol_mode {
        let words: usize = pack::BOOT_SHARDS
            .iter()
            .chain(pack::RESIDENT_TAIL_SHARDS)
            .map(|shard| shard.byte_len as usize / 4)
            .sum();
        println!(
            "[wm2000-block-boot] discovered pack: {} static-prefix shards + {} resident-tail shards / {words} words, bank {:#018X}, entry {:#010X}; captured exception images={}",
            pack::BOOT_SHARDS.len(),
            pack::RESIDENT_TAIL_SHARDS.len(),
            pack::ENTRY_BANK_ID,
            pack::ENTRYPOINT,
            pack::EXTERNAL_EXECUTABLE_IMAGES.len(),
        );
        for image in pack::EXTERNAL_EXECUTABLE_IMAGES {
            println!(
                "[wm2000-block-boot] exception image={} generation={} bank={:#018X} range=[{:#010X},{:#010X}) words={} digest={}",
                image.image_id,
                image.generation,
                image.bank_id,
                image.va_start,
                image.va_end,
                image.words.len(),
                image.sha256_hex,
            );
        }
    }
    let boot_inputs = if generated_runner_build_identity_mode {
        None
    } else {
        let rom_path = std::env::var("ROM").expect("ROM env var (same contract as build.rs)");
        let rom = std::fs::read(&rom_path).expect("reading ROM");
        let boot_context_path = std::env::var("FN64_BOOT_CONTEXT")
            .expect("FN64_BOOT_CONTEXT must name a black-box header-handoff capture");
        let boot_context = fn64_boot_harness::load_boot_context(
            std::path::Path::new(&boot_context_path),
            &rom,
            fn64_boot_harness::TvType::Ntsc,
        )
        .unwrap_or_else(|error| panic!("loading NWXE BootContext: {error}"));
        // The catalog boot seam validates the exact entry and restored CPU
        // state immediately before unified dispatch. Ordinary AOT repeats that
        // check at its first generated-bank call. Withholding executes that
        // entry dynamically, so its first static call is a post-instruction
        // resume; the bounded telemetry gate separately requires supported,
        // positive work at the exact withheld key.
        FIRST_ENTRY_BOOT_CONTEXT.with(|slot| {
            *slot.borrow_mut() = (!dynamic_exact_entry_withheld).then(|| boot_context.clone())
        });

        fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
        fn64_abi::load_rom(rom.clone());
        fn64_abi::set_guest_running_thread_global(pack::OS_RUNNING_THREAD);
        if std::env::var_os("FN64_BLOCK_PROGRESS_ONLY").is_some()
            && std::env::var_os("FN64_BLOCK_EXECUTOR_TRACE").is_none()
        {
            fn64_abi::set_trace_enabled(false);
        }
        if std::env::var_os("FN64_BLOCK_PROGRESS_ONLY").is_some()
            && std::env::var_os("FN64_BLOCK_DEVICE_TRACE").is_none()
        {
            fn64_abi::set_device_trace_enabled(false);
        }
        fn64_abi::set_audio_task_lle_accuracy();
        // `set_audio_rdram_len` alone registers the AI-buffer bound but leaves
        // no backend installed, so the RSP synthesizes PCM and the dispatcher
        // throws it away. The gate headline's "audio submissions" then counts
        // TASKS, which says nothing about whether a single correct sample was
        // produced -- a silent regression would keep that number identical.
        //
        // This lane is headless, so it cannot open a host device. It installs
        // a backend that keeps only statistics: enough to prove the audio path
        // end to end, and bounded regardless of run length.
        fn64_abi::set_audio_backend(
            Box::new(PcmEvidenceBackend::default()),
            fn64_recomp_rs::RDRAM_LEN,
        );
        // NWXE verifies SRAM by issuing domain-2 PI writes during boot. The block
        // lane is intentionally ephemeral, so give it a typed in-memory 32 KiB
        // device; omitting the device is a harness error and remains a loud trap.
        fn64_abi::set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::SramBanked,
        )));
        use fn64_render::RenderBackend as _;
        let mut render_backend = fn64_render_reference::ReferenceBackend::new()
            .with_f3dex2()
            .with_clear_color([0, 0, 0, 255]);
        if let Some(directory) = std::env::var_os("FN64_RENDER_DUMP_DIR") {
            let first_task = std::env::var("FN64_RENDER_DUMP_FIRST_TASK")
                .map(|value| {
                    value
                        .parse::<u64>()
                        .expect("FN64_RENDER_DUMP_FIRST_TASK must be an unsigned integer")
                })
                .unwrap_or(0);
            let limit = std::env::var("FN64_RENDER_DUMP_LIMIT")
                .map(|value| {
                    value
                        .parse::<u64>()
                        .expect("FN64_RENDER_DUMP_LIMIT must be an unsigned integer")
                })
                .unwrap_or(1);
            assert!(limit != 0, "FN64_RENDER_DUMP_LIMIT must be nonzero");
            let directory = std::path::PathBuf::from(directory);
            println!(
                "[wm2000-block-boot] render dump dir={} first_task={} limit={}",
                directory.display(),
                first_task,
                limit,
            );
            render_backend = render_backend
                .with_auto_dump(directory, "fn64-wm2000-block", limit)
                .with_auto_dump_skip(first_task);
        }
        render_backend
            .create(&fn64_render::RenderConfig::ntsc(320, 240))
            .expect("ReferenceBackend create must be infallible for 320x240");
        fn64_abi::set_render_backend_with_policy(
            Box::new(render_backend),
            fn64_recomp_rs::RDRAM_LEN,
            if generated_runner_rsp_audit_mode {
                fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy
            } else {
                fn64_abi::GraphicsTaskExecutionPolicy::HleOptimized
            },
        );
        println!("[wm2000-block-boot] registered reference renderer (320x240)");
        let (rom_start, rom_end, va_start) = pack::ROM_COPY;
        // The contract this asserts is IPL3's fixed one-MiB boot DMA, which is
        // universal; the literal triple was WM2000's instance of it. Checking
        // the invariant rather than the instance lets a differently-based title
        // through -- `va_start` is CIC-dependent (6103 subtracts 0x100000,
        // 6106 subtracts 0x200000), and pinning it here rejected every ROM that
        // is not WM2000 for no stated reason.
        assert_eq!(
            (rom_start, rom_end - rom_start),
            (0x1000, 0x100000),
            "block pack no longer matches the IPL3 one-MiB boot DMA contract: \
             rom=[{rom_start:#x},{rom_end:#x}) va={va_start:#010X}"
        );
        println!(
            "[wm2000-block-boot] validating boot publication rom=[{rom_start:#x},{rom_end:#x}) to va {va_start:#010X}"
        );
        Some((rom, boot_context))
    };

    let recent_history_limit = std::env::var("FN64_PROFILE_AOT_RECENT")
        .map(|value| {
            value
                .parse::<std::num::NonZeroUsize>()
                .expect("FN64_PROFILE_AOT_RECENT must be a positive integer")
        })
        .ok();
    let mut program = fn64_recomp_rs::BlockProgram::new();
    if let Some(limit) = recent_history_limit {
        program.set_execution_destination_history_limit(Some(limit));
    } else if std::env::var_os("FN64_BLOCK_PC_TRACE").is_none() {
        program.set_execution_destination_history_enabled(false);
    }
    let instruction_budget = std::env::var("FN64_BLOCK_INSTRUCTION_BUDGET")
        .map(|value| {
            value
                .parse::<u32>()
                .ok()
                .and_then(InstructionBudget::new)
                .expect("FN64_BLOCK_INSTRUCTION_BUDGET must be an integer of at least two")
        })
        .unwrap_or_else(|_| InstructionBudget::new(4096).expect("nonzero budget"));
    let ConstructedCatalogProgram {
        catalog_program,
        generation_catalog,
        generation_backings,
        generated_runner_bindings,
        program_evidence,
    } = construct_catalog_program(
        program,
        GateRunners {
            dense_instrumentation: full_aot_instrumentation
                .then_some(run_dense_aot_with_context_gate as GeneratedBankFn),
            entry_context: run_entry_aot_with_context_gate as GeneratedBankFn,
            overlay_generation: run_overlay_aot_with_generation_gate as GeneratedBankFn,
            external_digest: run_nwxe_exception_image_with_digest_gate as GeneratedBankFn,
        },
        instruction_budget,
    );
    let build_receipt = fn64_recomp_rs::static_execution_build_receipt();
    if !generated_runner_protocol_mode {
        println!(
            "[wm2000-block-boot] static execution build schema={} aot_runtime={} production_aot={} dev_interpreter={}",
            build_receipt.schema,
            build_receipt.aot_runtime,
            build_receipt.production_aot,
            build_receipt.dev_interpreter,
        );
    }
    let program_artifact = program_evidence
        .identity
        .identity
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if !generated_runner_protocol_mode {
        println!("[wm2000-block-boot] canonical program artifact={program_artifact}");
        println!("[wm2000-block-boot] booting thread 0 from the discovered pack...");
    }
    let generated_build_identity = generated_runner_protocol_mode
        .then(|| generated_runner_build_identity(&catalog_program, &generated_runner_bindings));
    if generated_runner_build_identity_mode {
        emit_generated_runner_build_identity(
            generated_build_identity
                .as_ref()
                .expect("protocol mode constructed the generated build identity"),
        );
        return;
    }
    let (rom, boot_context) =
        boot_inputs.expect("normal boot mode initialized ROM and BootContext before admission");
    let host_functions = issue_wm2000_host_function_catalog();
    let resolver = fn64_abi::recompiled::CatalogResolverInstallV1::new_with_abi_host_catalog(
        catalog_program,
        host_functions,
        ProgramArtifactIdentity::new(pack::DISPATCH_SOURCE_SHA256),
    );
    let generations =
        BackedPrecompiledGenerationCatalogV1::new(generation_catalog, generation_backings)
            .expect("every generated dynamic generation has one exact physical backing");
    let install = fn64_abi::recompiled::CatalogGenerationInstallV1::new(resolver, generations)
        .expect("canonical resolver admits every generated dynamic shard");
    let installed_evidence = install.evidence_snapshot();
    let canonical_entry = installed_evidence.resolver.entry;
    let program_identity_source = match installed_evidence.resolver.program_identity.source {
        fn64_recomp_rs::ProgramIdentitySource::CallerSupplied => "caller_supplied",
        fn64_recomp_rs::ProgramIdentitySource::CanonicalBlockProgramSha256 => {
            "canonical_block_program_sha256"
        }
    };
    let mut bootstrap = install
        .begin_bootstrap_import_v1(&rom, fn64_recomp_rs::RDRAM_LEN, fn64_runtime::TvType::Ntsc)
        .expect("creating canonical bootstrap transaction");
    bootstrap
        .publish_ipl3_cartridge_dma()
        .expect("publishing the typed IPL3 one-MiB cartridge DMA");
    let validated = bootstrap
        .commit()
        .expect("validating ROM, catalog, entry image, and executable-memory baseline");
    println!(
        "[wm2000-program-identity] schema=fn64.wm2000.program-identity.v1 sha256={} source={} resolver_sha256={} entry_bank={:016x} entry_pc={:08x}",
        sha256_hex(installed_evidence.resolver.program_identity.identity.bytes()),
        program_identity_source,
        sha256_hex(validated.receipt().evidence().resolver_install_sha256),
        canonical_entry.bank.get(),
        canonical_entry.pc.get(),
    );
    #[cfg(feature = "dynamic-withheld")]
    if dynamic_exact_entry_withheld {
        fn64_abi::recompiled::boot_thread0_validated_catalog_generation_program_with_exact_static_key_withheld_v1(
            validated,
            install,
            canonical_entry,
            boot_context,
            0,
            10,
        )
        .expect("booting operational dynamic program from validated owned RDRAM");
    } else {
        fn64_abi::recompiled::boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            boot_context,
            0,
            10,
        )
        .expect("booting canonical program from validated owned RDRAM");
    }
    #[cfg(not(feature = "dynamic-withheld"))]
    fn64_abi::recompiled::boot_thread0_validated_catalog_generation_program_v1(
        validated,
        install,
        boot_context,
        0,
        10,
    )
    .expect("booting canonical program from validated owned RDRAM");
    if generated_runner_bootstrap_audit_mode {
        // Bootstrap authority is minted before thread 0 can run. Consume it
        // immediately so no guest, device, or later writer enters this report.
        let receipt = fn64_abi::recompiled::take_validated_bootstrap_writer_channel_receipt_v1()
            .expect("fixed bootstrap audit did not own the canonical bootstrap receipt");
        let report = bootstrap_runtime_report(
            bootstrap_audit_nonce(),
            generated_build_identity
                .as_ref()
                .expect("bootstrap protocol mode constructed the generated build identity"),
            receipt,
        );
        let _exit = fn64_abi::prepare_process_exit();
        let wire = serde_json::to_string(&report)
            .expect("bootstrap runtime report serialization is infallible");
        std::println!(
            "{}{wire}",
            fn64_boot_harness::GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1
        );
        return;
    }
    let recent_host_history_limit = std::env::var("FN64_PROFILE_HOST_RECENT")
        .map(|value| {
            value
                .parse::<std::num::NonZeroUsize>()
                .expect("FN64_PROFILE_HOST_RECENT must be a positive integer")
        })
        .ok();
    if let Some(limit) = recent_host_history_limit {
        fn64_abi::recompiled::set_block_host_boundary_history_limit(Some(limit));
    } else if std::env::var_os("FN64_BLOCK_HOST_TRACE").is_none() {
        fn64_abi::recompiled::set_block_host_boundary_history_enabled(false);
    }
    let mut controller_schedule = std::env::var_os("FN64_CONTROLLER_SCHEDULE").map(|path| {
        let mut driver = ControllerScheduleDriver::load(std::path::Path::new(&path));
        driver.apply_current_inputs();
        driver
    });
    if std::env::var_os("FN64_BLOCK_WATCHDOG").is_some() {
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            eprintln!(
                "[wm2000-block-watchdog] entries={} last_pc={:#010x}",
                AOT_ENTRY_COUNT.load(std::sync::atomic::Ordering::Relaxed),
                LAST_AOT_ENTRY_PC.load(std::sync::atomic::Ordering::Relaxed),
            );
        });
    }
    // Bounded closure probe: step while runnable, advance deterministic
    // device time while idle, then require evidence that digest selection
    // entered at least one immutable overlay artifact.
    let generated_runner_writer_audit_mode = generated_runner_cpu_audit_mode
        || generated_runner_host_abi_audit_mode
        || generated_runner_pi_audit_mode
        || generated_runner_rdp_renderer_audit_mode
        || generated_runner_rsp_audit_mode
        || generated_runner_si_audit_mode
        || generated_runner_sp_audit_mode;
    let max_steps = if generated_runner_writer_audit_mode {
        2_000_000
    } else {
        std::env::var("FN64_BLOCK_MAX_STEPS")
            .map(|value| {
                value
                    .parse::<u64>()
                    .expect("FN64_BLOCK_MAX_STEPS must be an unsigned integer")
            })
            .unwrap_or(2_000_000)
    };
    const IDLE_TICKS_BEFORE_STOP: u32 = 200;
    let continue_after_overlay = generated_runner_writer_audit_mode
        || std::env::var_os("FN64_BLOCK_CONTINUE_AFTER_OVERLAY").is_some();
    let mut reported_overlay_entry = false;
    let mut consecutive_idle_ticks = 0u32;
    let mut steps = 0u64;
    let mut drain = fn64_boot_harness::GuestDrain::default();
    let mut cpu_audit_receipt = None;
    let mut host_abi_audit_receipt = None;
    let mut pi_audit_receipt = None;
    let mut rdp_renderer_audit_receipt = None;
    let mut rsp_audit_receipt = None;
    let mut si_audit_receipt = None;
    let mut sp_audit_receipt = None;
    let sp_audit_epoch = generated_runner_sp_audit_mode.then(|| {
        // This token clears retained history immediately before canonical
        // guest/device scheduling, so setup cannot satisfy the exercised path.
        fn64_abi::recompiled::begin_sp_writer_runtime_trace_epoch_v1()
            .expect("arming the fixed guest-driven SP writer audit epoch")
            .expect("fixed SP audit mode has no canonical runtime owner")
    });
    let cpu_audit_epoch = generated_runner_cpu_audit_mode.then(|| {
        // Arm after canonical boot but immediately before scheduling. This
        // excludes bootstrap/setup stores from the guest-driven audit window.
        fn64_abi::recompiled::begin_cpu_writer_runtime_trace_epoch_v1()
            .expect("arming the fixed guest-driven CPU writer audit epoch")
            .expect("fixed CPU audit mode has no canonical runtime owner")
    });
    let host_abi_audit_epoch = generated_runner_host_abi_audit_mode.then(|| {
        // Arm immediately before guest scheduling. Setup host calls therefore
        // cannot satisfy this canonical selected-build write window.
        fn64_abi::recompiled::begin_host_abi_writer_runtime_trace_epoch_v1()
            .expect("arming the fixed canonical Host ABI writer audit epoch")
            .expect("fixed Host ABI audit mode has no canonical runtime owner")
    });
    let pi_audit_epoch = generated_runner_pi_audit_mode.then(|| {
        // Arm immediately before guest/device scheduling so bootstrap PI work
        // cannot satisfy this fresh selected-build audit window.
        fn64_abi::recompiled::begin_pi_writer_runtime_trace_epoch_v1()
            .expect("arming the fixed guest-driven PI writer audit epoch")
            .expect("fixed PI audit mode has no canonical runtime owner")
    });
    let rdp_renderer_audit_epoch = generated_runner_rdp_renderer_audit_mode.then(|| {
        // Arm after canonical boot and immediately before guest/device
        // scheduling. Setup rendering and NeedsLle preflights cannot satisfy
        // this selected-build publication window.
        fn64_abi::recompiled::begin_rdp_renderer_writer_runtime_trace_epoch_v1()
            .expect("arming the fixed guest-driven RDP renderer audit epoch")
            .expect("fixed RDP renderer audit mode has no canonical runtime owner")
    });
    let rsp_audit_epoch = generated_runner_rsp_audit_mode.then(|| {
        // Arm after canonical boot and immediately before guest/device
        // scheduling. Setup work cannot satisfy this selected-build window.
        fn64_abi::recompiled::begin_rsp_writer_runtime_trace_epoch_v1()
            .expect("arming the fixed guest-driven RSP writer audit epoch")
            .expect("fixed RSP audit mode has no canonical runtime owner")
    });
    if generated_runner_si_audit_mode {
        // The retained trace window begins immediately before the bounded SI
        // audit. This excludes any earlier setup transaction from satisfying
        // the minimum PIF-to-RDRAM path requirement.
        fn64_abi::set_device_trace_enabled(false);
        fn64_abi::set_device_trace_enabled(true);
    }
    if let Some(expected) = expected_guest_instructions {
        fn64_abi::recompiled::set_canonical_block_instruction_limit_v1(Some(expected));
    }
    while steps < max_steps {
        let mut stop_for_idle = false;
        let next_priority = fn64_abi::next_runnable_priority();
        match drain.before_step(next_priority) {
            fn64_boot_harness::DrainDecision::Step => {
                PROBE_STEP.store(steps + 1, std::sync::atomic::Ordering::Relaxed);
                assert!(fn64_abi::run_one_step());
                drain.record_step(next_priority.expect("drain authorized a runnable step"));
                steps += 1;
                consecutive_idle_ticks = 0;
                let stop_guest_instructions =
                    expected_guest_instructions.or(minimum_guest_instructions);
                if stop_guest_instructions.is_some_and(|stop| {
                    fn64_abi::recompiled::canonical_block_charged_instructions_v1()
                        .is_some_and(|charged| charged >= stop)
                }) {
                    let charged = fn64_abi::recompiled::canonical_block_charged_instructions_v1()
                        .expect("canonical program instruction counter disappeared");
                    println!(
                        "[wm2000-block-profile] guest instruction stop reached: minimum={} expected={:?} achieved={} step={} sim_time={}",
                        minimum_guest_instructions.unwrap(), expected_guest_instructions,
                        charged,
                        steps,
                        fn64_abi::sim_time(),
                    );
                    break;
                }
                if PROFILE_STOP_AT_AOT_PC_REACHED.load(std::sync::atomic::Ordering::Relaxed) {
                    println!(
                        "[wm2000-block-profile] stop PC {:#010x} reached at step {steps} sim_time={}",
                        PROFILE_STOP_AT_AOT_PC
                            .get()
                            .copied()
                            .flatten()
                            .expect("reached stop-PC flag has a configured PC"),
                        fn64_abi::sim_time(),
                    );
                    break;
                }
                if PROFILE_STOP_AT_OVERLAY_GENERATION_REACHED
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    println!(
                        "[wm2000-block-profile] stop generation {} reached at step {steps} sim_time={}",
                        PROFILE_STOP_AT_OVERLAY_GENERATION
                            .get()
                            .copied()
                            .flatten()
                            .expect("reached stop-generation flag has a configured generation"),
                        fn64_abi::sim_time(),
                    );
                    break;
                }
                if !reported_overlay_entry
                    && ENTERED_OVERLAY_GENERATION_BITS.load(std::sync::atomic::Ordering::Relaxed)
                        != 0
                {
                    println!(
                        "[wm2000-block-boot] digest-selected overlay entry reached at step {steps} sim_time={}",
                        fn64_abi::sim_time()
                    );
                    reported_overlay_entry = true;
                    if !continue_after_overlay {
                        break;
                    }
                }
            }
            fn64_boot_harness::DrainDecision::AdvanceField => {
                let advanced = drain.advance_to_next_device_event();
                if let Some(schedule) = controller_schedule.as_mut() {
                    schedule.observe_completed_operations();
                }
                if matches!(advanced, fn64_boot_harness::DeviceAdvance::ViFields { .. }) {
                    consecutive_idle_ticks += 1;
                    if consecutive_idle_ticks >= IDLE_TICKS_BEFORE_STOP {
                        println!(
                            "[wm2000-block-boot] steady idle at sim_time={} steps={steps}",
                            fn64_abi::sim_time()
                        );
                        stop_for_idle = true;
                    }
                }
            }
        }
        if generated_runner_si_audit_mode {
            if let Some(receipt) = take_completed_si_audit_receipt() {
                si_audit_receipt = Some(receipt);
                break;
            }
        }
        if let Some(epoch) = cpu_audit_epoch.as_ref() {
            if let Some(receipt) = take_completed_cpu_audit_receipt(epoch) {
                cpu_audit_receipt = Some(receipt);
                break;
            }
        }
        if let Some(epoch) = host_abi_audit_epoch.as_ref() {
            if let Some(receipt) = take_completed_host_abi_audit_receipt(epoch) {
                host_abi_audit_receipt = Some(receipt);
                break;
            }
        }
        if let Some(epoch) = pi_audit_epoch.as_ref() {
            if let Some(receipt) = take_completed_pi_audit_receipt(epoch) {
                pi_audit_receipt = Some(receipt);
                break;
            }
        }
        if let Some(epoch) = rdp_renderer_audit_epoch.as_ref() {
            if let Some(receipt) = take_completed_rdp_renderer_audit_receipt(epoch) {
                rdp_renderer_audit_receipt = Some(receipt);
                break;
            }
        }
        if let Some(epoch) = rsp_audit_epoch.as_ref() {
            if let Some(receipt) = take_completed_rsp_audit_receipt(epoch) {
                rsp_audit_receipt = Some(receipt);
                break;
            }
        }
        if let Some(epoch) = sp_audit_epoch.as_ref() {
            if let Some(receipt) = take_completed_sp_audit_receipt(epoch) {
                sp_audit_receipt = Some(receipt);
                break;
            }
        }
        if stop_for_idle {
            break;
        }
    }
    if generated_runner_si_audit_mode {
        let report = si_runtime_report(
            si_audit_nonce(),
            generated_build_identity
                .as_ref()
                .expect("SI protocol mode constructed the generated build identity"),
            si_audit_receipt
                .expect("fixed SI audit exhausted its step bound without a complete SI receipt"),
        );
        let _exit = fn64_abi::prepare_process_exit();
        let wire =
            serde_json::to_string(&report).expect("SI runtime report serialization is infallible");
        std::println!(
            "{}{wire}",
            fn64_boot_harness::GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1
        );
        return;
    }
    if generated_runner_cpu_audit_mode {
        let report = cpu_runtime_report(
            cpu_audit_nonce(),
            generated_build_identity
                .as_ref()
                .expect("CPU protocol mode constructed the generated build identity"),
            cpu_audit_receipt
                .expect("fixed CPU audit exhausted its step bound without a guest store receipt"),
        );
        let _exit = fn64_abi::prepare_process_exit();
        let wire =
            serde_json::to_string(&report).expect("CPU runtime report serialization is infallible");
        std::println!(
            "{}{wire}",
            fn64_boot_harness::GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1
        );
        return;
    }
    if generated_runner_host_abi_audit_mode {
        let report = host_abi_runtime_report(
            host_abi_audit_nonce(),
            generated_build_identity
                .as_ref()
                .expect("Host ABI protocol mode constructed the generated build identity"),
            host_abi_audit_receipt.expect(
                "fixed Host ABI audit exhausted its step bound without a canonical write receipt",
            ),
        );
        let _exit = fn64_abi::prepare_process_exit();
        let wire = serde_json::to_string(&report)
            .expect("Host ABI runtime report serialization is infallible");
        std::println!(
            "{}{wire}",
            fn64_boot_harness::GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1
        );
        return;
    }
    if generated_runner_pi_audit_mode {
        let report = pi_runtime_report(
            pi_audit_nonce(),
            generated_build_identity
                .as_ref()
                .expect("PI protocol mode constructed the generated build identity"),
            pi_audit_receipt
                .expect("fixed PI audit exhausted its step bound without a read-DMA receipt"),
        );
        let _exit = fn64_abi::prepare_process_exit();
        let wire =
            serde_json::to_string(&report).expect("PI runtime report serialization is infallible");
        std::println!(
            "{}{wire}",
            fn64_boot_harness::GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1
        );
        return;
    }
    if generated_runner_rdp_renderer_audit_mode {
        let report = rdp_renderer_runtime_report(
            rdp_renderer_audit_nonce(),
            generated_build_identity
                .as_ref()
                .expect("RDP renderer protocol mode constructed the generated build identity"),
            rdp_renderer_audit_receipt.expect(
                "fixed RDP renderer audit exhausted its step bound without an executable-byte publication receipt",
            ),
        );
        let _exit = fn64_abi::prepare_process_exit();
        let wire = serde_json::to_string(&report)
            .expect("RDP renderer runtime report serialization is infallible");
        std::println!(
            "{}{wire}",
            fn64_boot_harness::GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1
        );
        return;
    }
    if generated_runner_rsp_audit_mode {
        let report = rsp_runtime_report(
            rsp_audit_nonce(),
            generated_build_identity
                .as_ref()
                .expect("RSP protocol mode constructed the generated build identity"),
            rsp_audit_receipt.expect(
                "fixed RSP audit exhausted its step bound without a typed writeback publication receipt",
            ),
        );
        let _exit = fn64_abi::prepare_process_exit();
        let wire =
            serde_json::to_string(&report).expect("RSP runtime report serialization is infallible");
        std::println!(
            "{}{wire}",
            fn64_boot_harness::GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1
        );
        return;
    }
    if generated_runner_sp_audit_mode {
        let report = sp_runtime_report(
            sp_audit_nonce(),
            generated_build_identity
                .as_ref()
                .expect("SP protocol mode constructed the generated build identity"),
            sp_audit_receipt.expect(
                "fixed SP audit exhausted its step bound without a guest-driven SP receipt",
            ),
        );
        let _exit = fn64_abi::prepare_process_exit();
        let wire =
            serde_json::to_string(&report).expect("SP runtime report serialization is infallible");
        std::println!(
            "{}{wire}",
            fn64_boot_harness::GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1
        );
        return;
    }
    if std::env::var_os("FN64_DEVICE_ADVANCE_CENSUS").is_some() {
        fn64_abi::print_device_advance_census();
    }
    fn64_abi::recompiled::report_dispatch_census();
    println!(
        "[wm2000-block-boot] done: steps={steps} sim_time={} thread0_dead={}",
        fn64_abi::sim_time(),
        fn64_abi::is_thread_dead(0)
    );
    let operational_boundary = minimum_guest_instructions.map(|minimum| {
        let achieved = fn64_abi::recompiled::canonical_block_charged_instructions_v1()
            .expect("canonical program instruction counter disappeared");
        assert!(
            achieved >= minimum,
            "bounded run stopped at {achieved} guest instructions before required minimum {minimum}"
        );
        if let Some(expected) = expected_guest_instructions {
            assert_eq!(
                achieved, expected,
                "bounded run did not stop at the requested exact canonical checkpoint"
            );
        }
        let boundary = capture_wm_operational_boundary_v1(achieved, steps);
        println!(
            "[wm2000-block-checkpoint] minimum_guest_instructions={} expected_guest_instructions={:?} achieved_guest_instructions={} scheduler_steps={} sim_time={} logical_rdram_bytes={} logical_rdram_sha256={}",
            minimum,
            expected_guest_instructions,
            achieved,
            boundary.scheduler_steps,
            boundary.sim_time,
            boundary.logical_rdram_len,
            sha256_hex(boundary.logical_rdram_sha256),
        );
        print_wm_operational_boundary_v1(&boundary);
        print_wm_publication_diagnostic_v1();
        boundary
    });
    let controller_read_ordinals = controller_schedule
        .as_ref()
        .map(ControllerScheduleDriver::read_ordinals)
        .unwrap_or([0; 4]);
    println!(
        "[wm2000-block-boot] standard controller reads port0={} port1={} port2={} port3={}",
        controller_read_ordinals[0],
        controller_read_ordinals[1],
        controller_read_ordinals[2],
        controller_read_ordinals[3],
    );
    print_runtime_progress();
    if std::env::var_os("FN64_PROFILE_CONTROL").is_some() {
        println!(
            "[wm2000-block-profile] control={:?}",
            fn64_abi::executor_control_evidence_snapshot()
        );
    }
    let entered_overlay_generations = entered_overlay_generation_ids();
    println!(
        "[wm2000-block-boot] entered digest-selected ROM-recovered generations: {entered_overlay_generations:?}"
    );
    print_profiled_rdram_ranges();
    if *PROFILE_AOT_BANKS.get().unwrap() {
        AOT_BANK_COUNTS.with(|counts| {
            let counts = counts.borrow();
            for (index, (artifact, count)) in DENSE_AOT_ARTIFACTS.iter().zip(*counts).enumerate() {
                if count != 0 {
                    println!(
                        "[wm2000-block-profile] artifact_index={index} bank={:#018x} overlay={} entries={count}",
                        artifact.bank_id,
                        index >= pack::BOOT_SHARDS.len() + pack::RESIDENT_TAIL_SHARDS.len(),
                    );
                }
            }
        });
    }
    AOT_PC_COUNTS.with(|counts| {
        AOT_PC_FIRST_GPRS.with(|first_gprs| {
            AOT_PC_FIRST_SYSTEM.with(|first_system| {
                AOT_PC_LAST_GPRS.with(|last_gprs| {
                    AOT_PC_LAST_SYSTEM.with(|last_system| {
                        let counts = counts.borrow();
                        let first_gprs = first_gprs.borrow();
                        let first_system = first_system.borrow();
                        let last_gprs = last_gprs.borrow();
                        let last_system = last_system.borrow();
                        for (index, pc) in PROFILE_AOT_PCS.get().unwrap().iter().enumerate() {
                            let count = counts.get(index).copied().unwrap_or(0);
                            let first_gprs = first_gprs.get(index).copied().flatten();
                            let first_system = first_system.get(index).copied().flatten();
                            let last_gprs = last_gprs.get(index).copied().flatten();
                            let last_system = last_system.get(index).copied().flatten();
                            println!(
                                "[wm2000-block-profile] pc={pc:#010x} entries={count} \
                     first_v0_v1_a0_a1_a2_a3_sp_ra_stack16_20_24_28_32_36={first_gprs:x?} \
                                 first_status_cause_epc_badvaddr_fcsr_d0_d18={first_system:x?} \
                                 last_v0_v1_a0_a1_a2_a3_sp_ra_stack16_20_24_28_32_36={last_gprs:x?} \
                                 last_status_cause_epc_badvaddr_fcsr_d0_d18={last_system:x?}"
                            );
                        }
                    })
                })
            })
        });
    });
    if let Some(limit) = recent_history_limit {
        let destinations = fn64_abi::recompiled::copy_block_execution_destinations();
        assert!(destinations.len() <= limit.get());
        let mut frequencies = std::collections::BTreeMap::<(u64, u32), (u64, u64)>::new();
        for observation in &destinations {
            let counts = frequencies
                .entry((
                    observation.destination.bank.get(),
                    observation.destination.pc.get(),
                ))
                .or_default();
            counts.0 += 1;
            counts.1 += u64::from(observation.instructions);
        }
        let mut frequencies = frequencies.into_iter().collect::<Vec<_>>();
        frequencies.sort_by_key(|((bank, pc), (entries, instructions))| {
            (std::cmp::Reverse(*entries), *bank, *pc, *instructions)
        });
        for ((bank, pc), (entries, instructions)) in frequencies.into_iter().take(20) {
            println!(
                "[wm2000-block-profile] recent bank={bank:#018x} pc={pc:#010x} entries={entries} instructions={instructions}"
            );
        }
    }
    if let Some(limit) = recent_host_history_limit {
        let boundaries = fn64_abi::recompiled::copy_block_host_boundaries();
        assert!(boundaries.len() <= limit.get());
        let mut frequencies = std::collections::BTreeMap::<(u32, u32, bool, u64, u32), u64>::new();
        for boundary in &boundaries {
            *frequencies
                .entry((
                    boundary.thread,
                    boundary.target.get(),
                    matches!(
                        boundary.phase,
                        fn64_abi::recompiled::BlockHostBoundaryPhase::Exit
                    ),
                    boundary.resume.bank.get(),
                    boundary.resume.pc.get(),
                ))
                .or_default() += 1;
        }
        let mut frequencies = frequencies.into_iter().collect::<Vec<_>>();
        frequencies.sort_by_key(|((thread, target, exit, resume_bank, resume_pc), count)| {
            (
                std::cmp::Reverse(*count),
                *thread,
                *target,
                *exit,
                *resume_bank,
                *resume_pc,
            )
        });
        for ((thread, target, exit, resume_bank, resume_pc), count) in
            frequencies.into_iter().take(20)
        {
            let phase = if exit { "exit" } else { "enter" };
            println!(
                "[wm2000-block-profile] recent_host thread={thread} target={target:#010x} phase={phase} resume_bank={resume_bank:#018x} resume_pc={resume_pc:#010x} boundaries={count}"
            );
        }
        for boundary in boundaries.iter().rev().take(12).rev() {
            println!(
                "[wm2000-block-profile] recent_host_tail cycle={} thread={} target={:#010x} phase={:?} resume_bank={:#018x} resume_pc={:#010x} a0_a1_a2_a3={:x?}",
                boundary.at.get(),
                boundary.thread,
                boundary.target.get(),
                boundary.phase,
                boundary.resume.bank.get(),
                boundary.resume.pc.get(),
                &boundary.gprs[4..8],
            );
        }
    }
    let destinations = fn64_abi::recompiled::copy_block_execution_destinations();
    write_pc_trace(&destinations, instruction_budget.get());
    let host_boundaries = fn64_abi::recompiled::copy_block_host_boundaries();
    write_host_boundary_trace(&host_boundaries);
    if std::env::var_os("FN64_BLOCK_PROGRESS_ONLY").is_some() {
        #[cfg(feature = "dynamic-withheld")]
        let dynamic_report = dynamic_exact_entry_withheld.then(|| {
            build_dynamic_withheld_telemetry(
                canonical_entry,
                minimum_guest_instructions
                    .expect("dynamic withheld mode requires a guest-instruction minimum"),
                expected_guest_instructions,
                operational_boundary
                    .as_ref()
                    .expect("dynamic withheld mode captured an operational boundary"),
            )
        });
        let exit = fn64_abi::prepare_process_exit();
        if let Some(target) = PROFILE_STOP_AT_OVERLAY_GENERATION.get().copied().flatten() {
            assert!(
                PROFILE_STOP_AT_OVERLAY_GENERATION_REACHED
                    .load(std::sync::atomic::Ordering::Relaxed),
                "requested stop generation {target} was not reached before the bounded exit; process_exit={exit:?}"
            );
        }
        #[cfg(feature = "dynamic-withheld")]
        if let (Some(output), Some(report)) = (dynamic_telemetry_output, dynamic_report) {
            commit_dynamic_withheld_telemetry(output, report, &exit);
        }
        println!("[wm2000-block-boot] bounded progress-only exit: {exit:?}");
        return;
    }
    if entered_overlay_generations.is_empty() {
        let control = fn64_abi::executor_control_evidence_snapshot();
        let trace = fn64_abi::copy_trace();
        let trace_start = trace.len().saturating_sub(32);
        let destination_start = destinations.len().saturating_sub(64);
        let controller_read_ordinals = controller_schedule
            .as_ref()
            .map(ControllerScheduleDriver::read_ordinals);
        let exit = fn64_abi::prepare_process_exit();
        panic!(
            "closed AOT catalog never entered a digest-selected ROM-recovered overlay generation; entered_aot={} last_aot_bank={:#018x} last_aot_pc={:#010x}; controller_read_ordinals={controller_read_ordinals:?}; recent_destinations={:?}; control={control:?}; recent_device_trace={:?}; process_exit={exit:?}",
            AOT_ENTRY_COUNT.load(std::sync::atomic::Ordering::Relaxed),
            LAST_AOT_ENTRY_BANK.load(std::sync::atomic::Ordering::Relaxed),
            LAST_AOT_ENTRY_PC.load(std::sync::atomic::Ordering::Relaxed),
            &destinations[destination_start..],
            &trace[trace_start..],
        );
    }
    // Why the run stopped: a guest that reaches the step limit without
    // submitting graphics is blocked on something, and the thread/queue state
    // names it exactly. Printed unconditionally -- it is a few lines and it is
    // the first question asked of every bounded run.
    let control = fn64_abi::executor_control_evidence_snapshot();
    println!(
        "[wm2000-block-boot] final control: sim_time={} run_queue={:?} cp0_timer_pending={}",
        control.sim_time, control.run_queue, control.cp0_timer_pending
    );
    for thread in &control.threads {
        println!("[wm2000-block-boot]   thread {thread:?}");
    }
    for queue in &control.queues {
        println!("[wm2000-block-boot]   queue {queue:?}");
    }
    for event in &control.event_table {
        println!("[wm2000-block-boot]   event {event:?}");
    }
    let exit = fn64_abi::prepare_process_exit();
    println!(
        "[wm2000-block-boot] process exit prepared: threads={} detached_coroutines={}",
        exit.threads, exit.detached_coroutines
    );
}
