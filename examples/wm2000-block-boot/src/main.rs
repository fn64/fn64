//! Boot WM2000 (NWXE) from fn64's OWN discovered Block Pack -- no
//! aki-recomp metadata, no N64Recomp C. `build.rs` ran discovery on the
//! user's ROM, then emitted dense arbitrary-PC resident and overlay runners.
//! Black-box image evidence is retained only for captured CPU-written
//! exception-vector images. This harness seals those artifacts with an exact
//! host catalog, physically backed generations, and a validated IPL3
//! publication in runtime-owned RDRAM, then drives the executor until the guest
//! either idles, reaches an unobserved PC, or reaches a runtime-behavior fault.

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
        println!(
            "[wm2000-block-boot] controller schedule={} phases={} sha256={}",
            path.display(),
            schedule.phases().len(),
            schedule.source_sha256_hex(),
        );
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

fn write_pc_trace(
    destinations: &[fn64_recomp_rs::ExecutionDestinationObservation],
    instruction_budget: u32,
) {
    let Some(path) = std::env::var_os("FN64_BLOCK_PC_TRACE") else {
        return;
    };
    assert_eq!(
        instruction_budget,
        InstructionBudget::MIN,
        "FN64_BLOCK_PC_TRACE requires the minimum instruction budget"
    );
    let file = std::fs::File::create(&path).unwrap_or_else(|error| {
        panic!(
            "creating FN64_BLOCK_PC_TRACE output {}: {error}",
            std::path::Path::new(&path).display()
        )
    });
    let mut output = std::io::BufWriter::new(file);
    for observation in destinations {
        assert!(
            observation.instructions <= InstructionBudget::MIN,
            "minimum-budget runner retired {} instructions at {}",
            observation.instructions,
            observation.destination
        );
        for index in 0..observation.instructions {
            let pc = observation
                .destination
                .pc
                .get()
                .wrapping_add(index.wrapping_mul(4));
            writeln!(
                output,
                "{pc:08x}\t{:016x}",
                observation.destination.bank.get()
            )
            .expect("writing FN64_BLOCK_PC_TRACE output");
        }
    }
    output.flush().expect("flushing FN64_BLOCK_PC_TRACE output");
}

fn write_host_boundary_trace(boundaries: &[fn64_abi::recompiled::BlockHostBoundaryObservation]) {
    let Some(path) = std::env::var_os("FN64_BLOCK_HOST_TRACE") else {
        return;
    };
    let file = std::fs::File::create(&path).unwrap_or_else(|error| {
        panic!(
            "creating FN64_BLOCK_HOST_TRACE output {}: {error}",
            std::path::Path::new(&path).display()
        )
    });
    let mut output = std::io::BufWriter::new(file);
    for (ordinal, boundary) in boundaries.iter().enumerate() {
        writeln!(
            output,
            concat!(
                "{{\"ordinal\":{},\"cycle\":{},\"thread\":\"{:?}\",",
                "\"phase\":\"{:?}\",\"target\":{},\"resume_bank\":{},",
                "\"resume_pc\":{},\"gprs\":{:?},\"hi\":{},\"lo\":{},",
                "\"cop0_count\":{},\"cop0_compare\":{},\"cop0_status\":{},",
                "\"cop0_cause\":{},\"cop0_epc\":{}}}"
            ),
            ordinal,
            boundary.at.get(),
            boundary.thread,
            boundary.phase,
            boundary.target.get(),
            boundary.resume.bank.get(),
            boundary.resume.pc.get(),
            boundary.gprs,
            boundary.hi,
            boundary.lo,
            boundary.cop0_count,
            boundary.cop0_compare,
            boundary.cop0_status,
            boundary.cop0_cause,
            boundary.cop0_epc,
        )
        .expect("writing FN64_BLOCK_HOST_TRACE output");
    }
    output
        .flush()
        .expect("flushing FN64_BLOCK_HOST_TRACE output");
}

#[cfg(feature = "dynamic-withheld")]
struct DynamicTelemetryOutput {
    final_path: std::path::PathBuf,
    partial_path: std::path::PathBuf,
    file: std::fs::File,
}

#[cfg(feature = "dynamic-withheld")]
fn prepare_dynamic_telemetry_output() -> DynamicTelemetryOutput {
    let requested_final_path = std::path::PathBuf::from(
        std::env::var_os("FN64_DYNAMIC_TELEMETRY")
            .expect("dynamic withheld execution requires FN64_DYNAMIC_TELEMETRY output path"),
    );
    assert!(
        requested_final_path.is_absolute(),
        "FN64_DYNAMIC_TELEMETRY must be an absolute out-of-tree path"
    );
    let parent = requested_final_path
        .parent()
        .expect("FN64_DYNAMIC_TELEMETRY must have a parent directory")
        .canonicalize()
        .unwrap_or_else(|error| {
            panic!(
                "canonicalizing FN64_DYNAMIC_TELEMETRY parent {}: {error}",
                requested_final_path.parent().unwrap().display()
            )
        });
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("fn64 repository root is canonicalizable");
    assert!(
        !parent.starts_with(&repository),
        "FN64_DYNAMIC_TELEMETRY must remain outside the fn64 repository"
    );
    let file_name = requested_final_path
        .file_name()
        .expect("FN64_DYNAMIC_TELEMETRY must have a file name");
    let final_path = parent.join(file_name);
    assert!(
        !final_path.exists(),
        "FN64_DYNAMIC_TELEMETRY refuses to overwrite {}",
        final_path.display()
    );
    let file_name = file_name
        .to_str()
        .expect("FN64_DYNAMIC_TELEMETRY must have a Unicode file name");
    let partial_path = parent.join(format!(".{file_name}.fn64-partial-{}", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial_path)
        .unwrap_or_else(|error| {
            panic!(
                "creating private dynamic telemetry staging file {}: {error}",
                partial_path.display()
            )
        });
    DynamicTelemetryOutput {
        final_path,
        partial_path,
        file,
    }
}

#[cfg(feature = "dynamic-withheld")]
fn build_dynamic_withheld_telemetry(
    withheld_static_key: ExecutionKey,
    minimum_guest_instructions: u64,
    expected_guest_instructions: Option<u64>,
    boundary: &WmOperationalBoundaryV1,
) -> serde_json::Value {
    let telemetry = fn64_abi::recompiled::copy_dynamic_mapped_execution_telemetry_v1();
    let charged_guest_instructions = boundary.achieved_guest_instructions;
    assert!(
        charged_guest_instructions >= minimum_guest_instructions,
        "dynamic run stopped at {charged_guest_instructions} guest instructions before required minimum {minimum_guest_instructions}"
    );
    if let Some(expected) = expected_guest_instructions {
        assert_eq!(
            charged_guest_instructions, expected,
            "dynamic run did not stop at the baseline's exact canonical checkpoint"
        );
    }
    assert!(
        telemetry.aggregates.iter().any(|aggregate| {
            aggregate.attempted_entries.iter().any(|entry| {
                entry.attempted_entry == withheld_static_key
                    && entry.activations > 0
                    && entry.charged_instructions > 0
                    && entry.unsupported_exits == 0
            })
        }),
        "the bounded run did not dynamically charge the exact withheld static key {withheld_static_key}",
    );
    let aggregates = telemetry
        .aggregates
        .iter()
        .map(|aggregate| {
            let instructions = aggregate
                .instructions
                .iter()
                .map(|instruction| {
                    serde_json::json!({
                        "bank": format!("{:016x}", instruction.bank.get()),
                        "physical_address": format!("{:08x}", instruction.physical_address),
                    })
                })
                .collect::<Vec<_>>();
            let attempted_entries = aggregate
                .attempted_entries
                .iter()
                .map(|entry| {
                    serde_json::json!({
                        "bank": format!("{:016x}", entry.attempted_entry.bank.get()),
                        "pc": format!("{:08x}", entry.attempted_entry.pc.get()),
                        "activations": entry.activations,
                        "charged_instructions": entry.charged_instructions,
                        "unsupported_exits": entry.unsupported_exits,
                    })
                })
                .collect::<Vec<_>>();
            serde_json::json!({
                "identity_sha256": sha256_hex(aggregate.identity.bytes()),
                "admitted_bank": format!("{:016x}", aggregate.admitted_entry.bank.get()),
                "admitted_pc": format!("{:08x}", aggregate.admitted_entry.pc.get()),
                "instructions": instructions,
                "attempted_entries": attempted_entries,
                "activations": aggregate.activations,
                "charged_instructions": aggregate.charged_instructions,
                "unsupported_exits": aggregate.unsupported_exits,
                "first_mutation_sequence": aggregate.first_mutation_sequence,
                "last_mutation_sequence": aggregate.last_mutation_sequence,
                "last_exit": format!("{:?}", aggregate.last_exit),
            })
        })
        .collect::<Vec<_>>();
    let program_identity_source = match telemetry.program_identity.source {
        fn64_recomp_rs::ProgramIdentitySource::CallerSupplied => "caller_supplied",
        fn64_recomp_rs::ProgramIdentitySource::CanonicalBlockProgramSha256 => {
            "canonical_block_program_sha256"
        }
    };
    let operational_boundary = wm_operational_boundary_json_v1(boundary);
    let mutation_quiescence = operational_boundary["mutation_quiescence"].clone();
    serde_json::json!({
        "schema": "fn64.wm2000.dynamic-withheld-telemetry.v2",
        "authority": "operational_only_dynamic_installed",
        "claim": "dynamically_executed_exact_withheld_static_key",
        "selection_basis": "validated_canonical_catalog_entry",
        "guest_instruction_horizon": {
            "minimum": minimum_guest_instructions,
            "expected_exact": expected_guest_instructions,
            "achieved": charged_guest_instructions,
            "expected_match": expected_guest_instructions
                .map(|expected| expected == charged_guest_instructions),
        },
        "scheduler_boundary": {
            "steps": boundary.scheduler_steps,
            "sim_time": boundary.sim_time,
            "next_runnable_priority": format!("{:?}", fn64_abi::next_runnable_priority()),
        },
        "full_logical_rdram": {
            "byte_len": boundary.logical_rdram_len,
            "sha256": sha256_hex(boundary.logical_rdram_sha256),
        },
        "withheld": {
            "bank": format!("{:016x}", withheld_static_key.bank.get()),
            "pc": format!("{:08x}", withheld_static_key.pc.get()),
        },
        "resolver_install_sha256": sha256_hex(telemetry.resolver_install_sha256),
        "program_identity": {
            "sha256": sha256_hex(telemetry.program_identity.identity.bytes()),
            "source": program_identity_source,
        },
        "dynamic_source_sha256": sha256_hex(telemetry.dynamic_source_sha256),
        "rom_sha256": telemetry.rom_sha256.map(sha256_hex),
        "bootstrap_receipt_sha256": telemetry.bootstrap_receipt_sha256.map(sha256_hex),
        "mutation_journal_root_sha256": telemetry.mutation_journal_root_sha256.map(sha256_hex),
        "mutation_journal_entry_count": telemetry.mutation_journal_entry_count,
        "aggregate_capacity": telemetry.aggregate_capacity,
        "attempted_entries_per_aggregate_capacity": telemetry.attempted_entries_per_aggregate_capacity,
        "dropped_identity_activations": telemetry.dropped_identity_activations,
        "dropped_identity_charged_instructions": telemetry.dropped_identity_charged_instructions,
        "dropped_identity_unsupported_exits": telemetry.dropped_identity_unsupported_exits,
        "dropped_attempted_entry_activations": telemetry.dropped_attempted_entry_activations,
        "dropped_attempted_entry_charged_instructions": telemetry.dropped_attempted_entry_charged_instructions,
        "dropped_attempted_entry_unsupported_exits": telemetry.dropped_attempted_entry_unsupported_exits,
        "mutation_quiescence": mutation_quiescence,
        "operational_boundary": operational_boundary,
        "aggregates": aggregates,
    })
}

#[cfg(feature = "dynamic-withheld")]
fn commit_dynamic_withheld_telemetry(
    output: DynamicTelemetryOutput,
    mut report: serde_json::Value,
    process_exit: &fn64_runtime::ProcessExitSummary,
) {
    report["termination"] = serde_json::json!({
        "mode": "bounded_progress_only",
        "stop_cause": "minimum_guest_instruction_checkpoint_reached",
        "process_exit_threads": process_exit.threads,
        "process_exit_detached_coroutines": process_exit.detached_coroutines,
    });
    let mut writer = std::io::BufWriter::new(output.file);
    serde_json::to_writer_pretty(&mut writer, &report)
        .expect("serializing bounded dynamic telemetry is infallible");
    writeln!(writer).expect("terminating dynamic telemetry JSON");
    writer.flush().expect("flushing dynamic telemetry JSON");
    writer
        .get_ref()
        .sync_all()
        .expect("syncing dynamic telemetry staging file");
    drop(writer);
    assert!(
        !output.final_path.exists(),
        "FN64_DYNAMIC_TELEMETRY destination appeared during the run: {}",
        output.final_path.display()
    );
    std::fs::hard_link(&output.partial_path, &output.final_path).unwrap_or_else(|error| {
        panic!(
            "atomically publishing dynamic telemetry {}: {error}",
            output.final_path.display()
        )
    });
    std::fs::remove_file(&output.partial_path).unwrap_or_else(|error| {
        panic!(
            "removing published telemetry staging link {}: {error}",
            output.partial_path.display()
        )
    });
}

fn print_runtime_progress() {
    let trace_len = fn64_abi::trace_len();
    let (graphics_submits, audio_submits) = fn64_abi::task_counts();

    let device = fn64_abi::device_trace_summary();

    let rsp_rdp = fn64_abi::copy_rsp_rdp_observations();
    let mut microcode_recognitions = 0u64;
    let mut dram_dpc_commits = 0u64;
    let mut xbus_dpc_commits = 0u64;
    let mut imem_replacements = 0u64;
    for observation in &rsp_rdp {
        match &observation.kind {
            fn64_abi::RspRdpObservationKind::MicrocodeRecognition { .. } => {
                microcode_recognitions += 1
            }
            fn64_abi::RspRdpObservationKind::DramDpcCommitted { .. } => dram_dpc_commits += 1,
            fn64_abi::RspRdpObservationKind::XbusDpcCommitted { .. } => xbus_dpc_commits += 1,
            fn64_abi::RspRdpObservationKind::ImemReplacementCommitted { .. } => {
                imem_replacements += 1
            }
        }
    }
    let host = fn64_abi::host_evidence_snapshot();
    println!(
        concat!(
            "[wm2000-block-progress] trace={} device_trace={} gfx_submits={} audio_submits={} ",
            "pi_started={} si_started={} ai_started={} sp_dma_started={} sp_tasks={} ",
            "rcp_started={} rcp_completed={} vi_interrupts={} controller_ops={} save_ops={} ",
            "rsp_rdp={} ucode_recognitions={} dram_dpc={} xbus_dpc={} imem_replacements={} ",
            "loaded_rsp_task={} rsp_lineages={} audio_policy={:?} render_error={:?}"
        ),
        trace_len,
        device.events,
        graphics_submits,
        audio_submits,
        device.pi_dma_started,
        device.si_dma_started,
        device.ai_dma_started,
        device.sp_dma_started,
        device.sp_tasks_admitted,
        device.rcp_tasks_started,
        device.rcp_tasks_completed,
        device.vi_interrupts,
        fn64_abi::copy_controller_operations().len(),
        fn64_abi::copy_save_operations().len(),
        rsp_rdp.len(),
        microcode_recognitions,
        dram_dpc_commits,
        xbus_dpc_commits,
        imem_replacements,
        host.loaded_rsp_task.is_some(),
        host.rsp_task_lineages.len(),
        host.audio_task_execution,
        fn64_abi::last_render_error(),
    );
    let timing = fn64_abi::phase_timing();
    if timing.executor_calls > 0 {
        println!(
            "[wm2000-block-profile] phase_timing executor_ms={:.3} calls={} gfx_ms={:.3} phases={} gfx_lle_ms={:.3} tasks={} gfx_lle_rsp_ms={:.3} gfx_lle_rdp_ms={:.3} audio_ms={:.3} tasks={}",
            timing.executor_ns as f64 / 1e6,
            timing.executor_calls,
            timing.gfx_ns as f64 / 1e6,
            timing.gfx_calls,
            timing.gfx_lle_ns as f64 / 1e6,
            timing.gfx_lle_calls,
            timing.gfx_lle_rsp_ns as f64 / 1e6,
            timing.gfx_lle_rdp_ns as f64 / 1e6,
            timing.audio_dispatch_ns as f64 / 1e6,
            timing.audio_dispatch_calls,
        );
    }
}

fn print_profiled_rdram_ranges() {
    let Ok(specification) = std::env::var("FN64_PROFILE_RDRAM_RANGES") else {
        return;
    };
    let memory = fn64_abi::copy_registered_physical_rdram_logical()
        .expect("validated owned RDRAM remains installed while profiling");
    for range in specification
        .split(',')
        .filter(|range| !range.trim().is_empty())
    {
        let (address, byte_len) = range
            .trim()
            .split_once(':')
            .expect("FN64_PROFILE_RDRAM_RANGES entries must be HEX_VRAM:BYTE_LEN");
        let address = u32::from_str_radix(address.trim_start_matches("0x"), 16)
            .expect("FN64_PROFILE_RDRAM_RANGES address must be hexadecimal");
        let byte_len = byte_len
            .parse::<usize>()
            .expect("FN64_PROFILE_RDRAM_RANGES length must be decimal");
        assert!(
            byte_len <= 256,
            "FN64_PROFILE_RDRAM_RANGES length exceeds the 256-byte diagnostic bound"
        );
        let offset = address & 0x1fff_ffff;
        let end = usize::try_from(offset)
            .unwrap()
            .checked_add(byte_len)
            .expect("profiled RDRAM range overflow");
        assert!(
            end <= memory.len(),
            "profiled RDRAM range exceeds installed memory"
        );
        let bytes = &memory[usize::try_from(offset).unwrap()..end];
        let hex = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        println!("[wm2000-block-profile] rdram={address:#010x} len={byte_len} bytes={hex}");
    }
}

const DENSE_AOT_ARTIFACTS: &[DenseAotArtifact] = &[
    DenseAotArtifact {
        bank_id: wm2000_block_shard_00::BANK_ID,
        code_bank: wm2000_block_shard_00::code_bank,
        runner: wm2000_block_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_01::BANK_ID,
        code_bank: wm2000_block_shard_01::code_bank,
        runner: wm2000_block_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_02::BANK_ID,
        code_bank: wm2000_block_shard_02::code_bank,
        runner: wm2000_block_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_03::BANK_ID,
        code_bank: wm2000_block_shard_03::code_bank,
        runner: wm2000_block_shard_03::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_04::BANK_ID,
        code_bank: wm2000_block_shard_04::code_bank,
        runner: wm2000_block_shard_04::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_05::BANK_ID,
        code_bank: wm2000_block_shard_05::code_bank,
        runner: wm2000_block_shard_05::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_06::BANK_ID,
        code_bank: wm2000_block_shard_06::code_bank,
        runner: wm2000_block_shard_06::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_07::BANK_ID,
        code_bank: wm2000_block_shard_07::code_bank,
        runner: wm2000_block_shard_07::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_08::BANK_ID,
        code_bank: wm2000_block_shard_08::code_bank,
        runner: wm2000_block_shard_08::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_09::BANK_ID,
        code_bank: wm2000_block_shard_09::code_bank,
        runner: wm2000_block_shard_09::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_10::BANK_ID,
        code_bank: wm2000_block_shard_10::code_bank,
        runner: wm2000_block_shard_10::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_11::BANK_ID,
        code_bank: wm2000_block_shard_11::code_bank,
        runner: wm2000_block_shard_11::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_12::BANK_ID,
        code_bank: wm2000_block_shard_12::code_bank,
        runner: wm2000_block_shard_12::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_13::BANK_ID,
        code_bank: wm2000_block_shard_13::code_bank,
        runner: wm2000_block_shard_13::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_shard_14::BANK_ID,
        code_bank: wm2000_block_shard_14::code_bank,
        runner: wm2000_block_shard_14::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_resident_tail_shard_00::BANK_ID,
        code_bank: wm2000_block_resident_tail_shard_00::code_bank,
        runner: wm2000_block_resident_tail_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_resident_tail_shard_01::BANK_ID,
        code_bank: wm2000_block_resident_tail_shard_01::code_bank,
        runner: wm2000_block_resident_tail_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_0_shard_00::BANK_ID,
        code_bank: wm2000_block_overlay_0_shard_00::code_bank,
        runner: wm2000_block_overlay_0_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_0_shard_01::BANK_ID,
        code_bank: wm2000_block_overlay_0_shard_01::code_bank,
        runner: wm2000_block_overlay_0_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_0_shard_02::BANK_ID,
        code_bank: wm2000_block_overlay_0_shard_02::code_bank,
        runner: wm2000_block_overlay_0_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_1_shard_00::BANK_ID,
        code_bank: wm2000_block_overlay_1_shard_00::code_bank,
        runner: wm2000_block_overlay_1_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_00::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_00::code_bank,
        runner: wm2000_block_overlay_2_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_01::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_01::code_bank,
        runner: wm2000_block_overlay_2_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_02::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_02::code_bank,
        runner: wm2000_block_overlay_2_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_03::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_03::code_bank,
        runner: wm2000_block_overlay_2_shard_03::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_04::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_04::code_bank,
        runner: wm2000_block_overlay_2_shard_04::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_2_shard_05::BANK_ID,
        code_bank: wm2000_block_overlay_2_shard_05::code_bank,
        runner: wm2000_block_overlay_2_shard_05::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_00::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_00::code_bank,
        runner: wm2000_block_overlay_3_shard_00::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_01::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_01::code_bank,
        runner: wm2000_block_overlay_3_shard_01::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_02::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_02::code_bank,
        runner: wm2000_block_overlay_3_shard_02::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_03::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_03::code_bank,
        runner: wm2000_block_overlay_3_shard_03::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_04::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_04::code_bank,
        runner: wm2000_block_overlay_3_shard_04::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_05::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_05::code_bank,
        runner: wm2000_block_overlay_3_shard_05::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_06::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_06::code_bank,
        runner: wm2000_block_overlay_3_shard_06::run,
    },
    DenseAotArtifact {
        bank_id: wm2000_block_overlay_3_shard_07::BANK_ID,
        code_bank: wm2000_block_overlay_3_shard_07::code_bank,
        runner: wm2000_block_overlay_3_shard_07::run,
    },
];

const DENSE_AOT_IDENTITIES: &[LinkedDenseIdentity] = &[
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_01::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_02::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_03::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_03::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_04::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_04::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_05::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_05::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_06::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_06::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_07::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_07::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_08::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_08::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_09::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_09::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_10::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_10::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_11::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_11::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_12::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_12::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_13::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_13::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_shard_14::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_shard_14::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_resident_tail_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_resident_tail_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_resident_tail_shard_01::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_resident_tail_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_0_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_0_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_0_shard_01::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_0_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_0_shard_02::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_0_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_1_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_1_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_01::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_02::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_03::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_03::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_04::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_04::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_2_shard_05::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_2_shard_05::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_00::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_00::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_01::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_01::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_02::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_02::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_03::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_03::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_04::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_04::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_05::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_05::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_06::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_06::RUNNER_SOURCE_SHA256,
    },
    LinkedDenseIdentity {
        source_sha256: wm2000_block_overlay_3_shard_07::SOURCE_SHA256,
        runner_source_sha256: wm2000_block_overlay_3_shard_07::RUNNER_SOURCE_SHA256,
    },
];

thread_local! {
    static FIRST_ENTRY_BOOT_CONTEXT: std::cell::RefCell<Option<BootContext>> = const {
        std::cell::RefCell::new(None)
    };
    static AOT_BANK_COUNTS: std::cell::RefCell<[u64; 35]> = const {
        std::cell::RefCell::new([0; 35])
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
    wm2000_block_shard_00::run(entry, budget, ctx, mem)
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

fn sha256_hex(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn diagnostic_hex_u32(value: u32) -> String {
    format!("0x{value:08x}")
}

fn diagnostic_hex_u64(value: u64) -> String {
    format!("0x{value:016x}")
}

fn diagnostic_execution_key(key: ExecutionKey) -> serde_json::Value {
    serde_json::json!({
        "bank": diagnostic_hex_u64(key.bank.get()),
        "pc": diagnostic_hex_u32(key.pc.get()),
    })
}

fn diagnostic_fault(fault: fn64_recomp_rs::CpuFault) -> serde_json::Value {
    serde_json::json!({
        "at": diagnostic_execution_key(fault.at),
        "kind": format!("{:?}", fault.kind),
    })
}

fn diagnostic_pending_exit(exit: fn64_recomp_rs::BlockExit) -> serde_json::Value {
    use fn64_recomp_rs::BlockExit;

    match exit {
        BlockExit::Transfer(entry) => serde_json::json!({
            "variant": "transfer",
            "entry": diagnostic_execution_key(entry),
        }),
        BlockExit::ResolveTransfer {
            source_bank,
            target_pc,
        } => serde_json::json!({
            "variant": "resolve_transfer",
            "source_bank": diagnostic_hex_u64(source_bank.get()),
            "target_pc": diagnostic_hex_u32(target_pc.get()),
        }),
        BlockExit::ResolveCall {
            source_bank,
            target_pc,
            resume,
        } => serde_json::json!({
            "variant": "resolve_call",
            "source_bank": diagnostic_hex_u64(source_bank.get()),
            "target_pc": diagnostic_hex_u32(target_pc.get()),
            "resume": diagnostic_execution_key(resume),
        }),
        BlockExit::HostCall { vram, resume } => serde_json::json!({
            "variant": "host_call",
            "target_pc": diagnostic_hex_u32(vram.get()),
            "resume": diagnostic_execution_key(resume),
        }),
        BlockExit::ExecutableWrite {
            source_bank,
            resume,
        } => serde_json::json!({
            "variant": "executable_write",
            "source_bank": diagnostic_hex_u64(source_bank.get()),
            "resume": diagnostic_execution_key(resume),
        }),
        BlockExit::ExecutableWriteResolveCall {
            source_bank,
            target_pc,
            resume,
        } => serde_json::json!({
            "variant": "executable_write_resolve_call",
            "source_bank": diagnostic_hex_u64(source_bank.get()),
            "target_pc": diagnostic_hex_u32(target_pc.get()),
            "resume": diagnostic_execution_key(resume),
        }),
        BlockExit::ExecutableWriteFault(fault) => serde_json::json!({
            "variant": "executable_write_fault",
            "fault": diagnostic_fault(fault),
        }),
        BlockExit::ImageChanged { at, miss } => serde_json::json!({
            "variant": "image_changed",
            "at": diagnostic_execution_key(at),
            "expected_bank": diagnostic_hex_u64(miss.expected_bank.get()),
            "va_start": diagnostic_hex_u32(miss.va_start.get()),
            "byte_len": miss.byte_len,
            "expected_sha256": sha256_hex(miss.expected_sha256),
            "actual_sha256": sha256_hex(miss.actual_sha256),
        }),
        BlockExit::Checkpoint(entry) => serde_json::json!({
            "variant": "checkpoint",
            "entry": diagnostic_execution_key(entry),
        }),
        BlockExit::Yield(entry) => serde_json::json!({
            "variant": "yield",
            "entry": diagnostic_execution_key(entry),
        }),
        BlockExit::ThreadReturn => serde_json::json!({ "variant": "thread_return" }),
        BlockExit::Fault(fault) => serde_json::json!({
            "variant": "fault",
            "fault": diagnostic_fault(fault),
        }),
    }
}

fn diagnostic_prepared_continuation(
    continuation: Option<fn64_abi::recompiled::CanonicalPreparedContinuationV1>,
) -> serde_json::Value {
    use fn64_abi::recompiled::CanonicalPreparedContinuationV1;

    match continuation {
        None => serde_json::Value::Null,
        Some(CanonicalPreparedContinuationV1::ImageChanged { entry }) => serde_json::json!({
            "variant": "image_changed",
            "entry": diagnostic_execution_key(entry),
        }),
        Some(CanonicalPreparedContinuationV1::InactiveGeneration { entry }) => {
            serde_json::json!({
                "variant": "inactive_generation",
                "entry": diagnostic_execution_key(entry),
            })
        }
    }
}

fn diagnostic_optional_hex_u32(value: Option<u32>) -> serde_json::Value {
    value
        .map(diagnostic_hex_u32)
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null)
}

fn diagnostic_cpu(
    thread: fn64_runtime::ThreadId,
    cpu: &fn64_recomp_rs::RecompContextEvidenceSnapshotV1,
) -> serde_json::Value {
    let mut count_normalized = cpu.clone();
    count_normalized.cop0_count = 0;
    let normalized_publication = [
        fn64_abi::recompiled::CanonicalThreadPublicationV1::Returned {
            thread,
            cpu: count_normalized,
        },
    ];
    let normalized = fn64_boot_harness::operational_thread_publication_digests_v2(
        &normalized_publication,
    )
    .expect("one diagnostic CPU publication is canonical");
    let mut digest = Sha256::new();
    digest.update(b"fn64.wm2000.operational-cpu-count-independent.v1");
    digest.update([0]);
    digest.update(normalized.cpu_sha256);
    let count_independent_sha256: [u8; 32] = digest.finalize().into();

    serde_json::json!({
        "cop0_count": diagnostic_hex_u32(cpu.cop0_count),
        "cop0_compare": diagnostic_hex_u32(cpu.cop0_compare),
        "cop0_count_write": diagnostic_optional_hex_u32(cpu.cop0_count_write),
        "cop0_compare_write": diagnostic_optional_hex_u32(cpu.cop0_compare_write),
        "count_independent_schema": "fn64.wm2000.operational-cpu-count-independent.v1",
        "count_independent_sha256": sha256_hex(count_independent_sha256),
    })
}

fn print_wm_publication_diagnostic_v1() {
    if std::env::var_os("FN64_WM_PUBLICATION_DIAGNOSTIC").is_none() {
        return;
    }

    for publication in fn64_abi::recompiled::copy_canonical_thread_publications_v1() {
        use fn64_abi::recompiled::CanonicalThreadPublicationV1;

        let record = match publication {
            CanonicalThreadPublicationV1::Exact(checkpoint) => serde_json::json!({
                "schema": "fn64.wm2000.publication-diagnostic.v1",
                "thread": checkpoint.thread,
                "publication_variant": "exact",
                "last_charge": checkpoint.charged_instructions,
                "cumulative_charge": checkpoint.canonical_charged_instructions_at_publication,
                "pending_exit": diagnostic_pending_exit(checkpoint.pending_exit),
                "prepared_continuation": diagnostic_prepared_continuation(checkpoint.prepared_continuation),
                "cpu": diagnostic_cpu(checkpoint.thread, &checkpoint.cpu),
            }),
            CanonicalThreadPublicationV1::OpaqueHostInFlight {
                thread,
                target,
                resume,
            } => serde_json::json!({
                "schema": "fn64.wm2000.publication-diagnostic.v1",
                "thread": thread,
                "publication_variant": "opaque_host_in_flight",
                "target_pc": diagnostic_hex_u32(target.get()),
                "resume": diagnostic_execution_key(resume),
            }),
            CanonicalThreadPublicationV1::ParkedFaultOpaque {
                thread,
                post_exception_cpu,
                fault,
                canonical_charged_instructions_at_publication,
            } => serde_json::json!({
                "schema": "fn64.wm2000.publication-diagnostic.v1",
                "thread": thread,
                "publication_variant": "parked_fault_opaque",
                "cumulative_charge": canonical_charged_instructions_at_publication,
                "fault": diagnostic_fault(fault),
                "cpu": diagnostic_cpu(thread, &post_exception_cpu),
            }),
            CanonicalThreadPublicationV1::Returned { thread, cpu } => serde_json::json!({
                "schema": "fn64.wm2000.publication-diagnostic.v1",
                "thread": thread,
                "publication_variant": "returned",
                "cpu": diagnostic_cpu(thread, &cpu),
            }),
        };
        std::println!(
            "[wm2000-publication-diagnostic] {}",
            serde_json::to_string(&record)
                .expect("publication diagnostic serialization is infallible")
        );
    }

    #[cfg(feature = "dynamic-withheld")]
    {
        let telemetry = fn64_abi::recompiled::copy_dynamic_mapped_execution_telemetry_v1();
        let entries = telemetry
            .aggregates
            .iter()
            .map(|aggregate| {
                serde_json::json!({
                    "bank": diagnostic_hex_u64(aggregate.admitted_entry.bank.get()),
                    "pc": diagnostic_hex_u32(aggregate.admitted_entry.pc.get()),
                    "activations": aggregate.activations,
                    "charged_instructions": aggregate.charged_instructions,
                    "unsupported_exits": aggregate.unsupported_exits,
                })
            })
            .collect::<Vec<_>>();
        let record = serde_json::json!({
            "schema": "fn64.wm2000.dynamic-execution-diagnostic.v1",
            "aggregate_count": telemetry.aggregates.len(),
            "entries": entries,
            "dropped_identity_activations": telemetry.dropped_identity_activations,
            "dropped_identity_charged_instructions": telemetry.dropped_identity_charged_instructions,
            "dropped_identity_unsupported_exits": telemetry.dropped_identity_unsupported_exits,
            "dropped_attempted_entry_activations": telemetry.dropped_attempted_entry_activations,
            "dropped_attempted_entry_charged_instructions": telemetry.dropped_attempted_entry_charged_instructions,
            "dropped_attempted_entry_unsupported_exits": telemetry.dropped_attempted_entry_unsupported_exits,
        });
        std::println!(
            "[wm2000-dynamic-execution-diagnostic] {}",
            serde_json::to_string(&record)
                .expect("dynamic execution diagnostic serialization is infallible")
        );
    }
}

struct WmOperationalBoundaryV1 {
    achieved_guest_instructions: u64,
    scheduler_steps: u64,
    sim_time: u64,
    logical_rdram_len: usize,
    logical_rdram_sha256: [u8; 32],
    components: fn64_boot_harness::OperationalStateComponentDigestsV1,
    publications: fn64_boot_harness::OperationalThreadPublicationDigestsV2,
    executor_thread_count: usize,
    missing_publication_count: usize,
    unexpected_publication_count: usize,
    cpu_comparable: bool,
    mutation_sealed: bool,
    pending_attributed_writes: usize,
    open_host_transactions: usize,
    mutation_journal_quiescent: bool,
}

fn canonical_publication_thread(
    publication: &fn64_abi::recompiled::CanonicalThreadPublicationV1,
) -> fn64_runtime::ThreadId {
    match publication {
        fn64_abi::recompiled::CanonicalThreadPublicationV1::Exact(checkpoint) => checkpoint.thread,
        fn64_abi::recompiled::CanonicalThreadPublicationV1::OpaqueHostInFlight {
            thread, ..
        }
        | fn64_abi::recompiled::CanonicalThreadPublicationV1::ParkedFaultOpaque {
            thread, ..
        }
        | fn64_abi::recompiled::CanonicalThreadPublicationV1::Returned { thread, .. } => *thread,
    }
}

fn capture_wm_operational_boundary_v1(
    achieved_guest_instructions: u64,
    scheduler_steps: u64,
) -> WmOperationalBoundaryV1 {
    let executor = fn64_abi::executor_control_evidence_snapshot();
    let executor_thread_ids = executor
        .threads
        .iter()
        .map(|thread| thread.id)
        .collect::<std::collections::BTreeSet<_>>();
    let components = fn64_boot_harness::operational_state_component_digests_v1(
        fn64_abi::device_evidence_snapshot(),
        executor.clone(),
        fn64_abi::host_evidence_snapshot(),
    )
    .expect("bounded WM operational component snapshots must be canonical");
    let thread_publications = fn64_abi::recompiled::copy_canonical_thread_publications_v1();
    let publication_thread_ids = thread_publications
        .iter()
        .map(canonical_publication_thread)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        publication_thread_ids.len(),
        thread_publications.len(),
        "canonical thread publication copy contained duplicate ThreadIds"
    );
    let missing_publication_count = executor_thread_ids
        .difference(&publication_thread_ids)
        .count();
    let unexpected_publication_count = publication_thread_ids
        .difference(&executor_thread_ids)
        .count();
    let publications =
        fn64_boot_harness::operational_thread_publication_digests_v2(&thread_publications)
            .expect("canonical thread publications retain strict ThreadId order");
    let logical_rdram = fn64_abi::copy_registered_physical_rdram_logical()
        .expect("canonical boot retains registered physical RDRAM");
    let logical_rdram_len = logical_rdram.len();
    let logical_rdram_sha256: [u8; 32] = Sha256::digest(&logical_rdram).into();
    drop(logical_rdram);
    let mutation = fn64_abi::recompiled::canonical_executable_mutation_journal_evidence_snapshot()
        .expect("canonical generation boot retains executable-mutation evidence");
    let open_host_transactions = mutation.open_host_transactions.len();
    let mutation_journal_quiescent =
        mutation.sealed && mutation.pending_attributed_writes == 0 && open_host_transactions == 0;
    let cpu_comparable = missing_publication_count == 0
        && unexpected_publication_count == 0
        && publications.opaque_count == 0;
    WmOperationalBoundaryV1 {
        achieved_guest_instructions,
        scheduler_steps,
        sim_time: executor.sim_time,
        logical_rdram_len,
        logical_rdram_sha256,
        components,
        publications,
        executor_thread_count: executor.threads.len(),
        missing_publication_count,
        unexpected_publication_count,
        cpu_comparable,
        mutation_sealed: mutation.sealed,
        pending_attributed_writes: mutation.pending_attributed_writes,
        open_host_transactions,
        mutation_journal_quiescent,
    }
}

fn print_wm_operational_boundary_v1(boundary: &WmOperationalBoundaryV1) {
    println!(
        concat!(
            "[wm2000-operational-boundary] schema=fn64.wm2000.operational-boundary.v1 ",
            "component_schema={} publication_schema={} capture_relation=latest_per_thread_publication_paired_with_post_scheduler_owner_snapshots ",
            "device_sha256={} executor_sha256={} abi_host_sha256={} cpu_sha256={} continuation_sha256={} ",
            "executor_threads={} publications={} exact={} opaque={} opaque_host={} parked_fault={} returned={} missing={} unexpected={} cpu_comparable={} ",
            "mutation_sealed={} pending_writes={} open_host_transactions={} mutation_quiescent={}"
        ),
        fn64_boot_harness::OPERATIONAL_STATE_COMPONENT_DIGEST_SCHEMA_V1,
        fn64_boot_harness::OPERATIONAL_THREAD_PUBLICATION_DIGEST_SCHEMA_V2,
        sha256_hex(boundary.components.device_sha256),
        sha256_hex(boundary.components.executor_sha256),
        sha256_hex(boundary.components.abi_host_sha256),
        sha256_hex(boundary.publications.cpu_sha256),
        sha256_hex(boundary.publications.continuation_sha256),
        boundary.executor_thread_count,
        boundary.publications.publication_count,
        boundary.publications.exact_count,
        boundary.publications.opaque_count,
        boundary.publications.opaque_host_count,
        boundary.publications.parked_fault_count,
        boundary.publications.returned_count,
        boundary.missing_publication_count,
        boundary.unexpected_publication_count,
        boundary.cpu_comparable,
        boundary.mutation_sealed,
        boundary.pending_attributed_writes,
        boundary.open_host_transactions,
        boundary.mutation_journal_quiescent,
    );
}

fn wm_operational_boundary_json_v1(boundary: &WmOperationalBoundaryV1) -> serde_json::Value {
    serde_json::json!({
        "schema": "fn64.wm2000.operational-boundary.v1",
        "component_schema": fn64_boot_harness::OPERATIONAL_STATE_COMPONENT_DIGEST_SCHEMA_V1,
        "publication_schema": fn64_boot_harness::OPERATIONAL_THREAD_PUBLICATION_DIGEST_SCHEMA_V2,
        "capture_relation": "latest_per_thread_publication_paired_with_post_scheduler_owner_snapshots",
        "components": {
            "device_sha256": sha256_hex(boundary.components.device_sha256),
            "executor_sha256": sha256_hex(boundary.components.executor_sha256),
            "abi_host_sha256": sha256_hex(boundary.components.abi_host_sha256),
        },
        "thread_publications": {
            "cpu_sha256": sha256_hex(boundary.publications.cpu_sha256),
            "continuation_sha256": sha256_hex(boundary.publications.continuation_sha256),
            "executor_threads": boundary.executor_thread_count,
            "publication_count": boundary.publications.publication_count,
            "exact_count": boundary.publications.exact_count,
            "opaque_count": boundary.publications.opaque_count,
            "opaque_host_count": boundary.publications.opaque_host_count,
            "parked_fault_count": boundary.publications.parked_fault_count,
            "returned_count": boundary.publications.returned_count,
            "missing_count": boundary.missing_publication_count,
            "unexpected_count": boundary.unexpected_publication_count,
            "cpu_comparable": boundary.cpu_comparable,
        },
        "mutation_quiescence": {
            "mutation_sealed": boundary.mutation_sealed,
            "pending_attributed_writes": boundary.pending_attributed_writes,
            "open_host_transactions": boundary.open_host_transactions,
            "mutation_journal_quiescent": boundary.mutation_journal_quiescent,
        },
    })
}

fn dynamic_exact_entry_withheld() -> bool {
    let Some(value) = std::env::var_os("FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY") else {
        return false;
    };
    #[cfg(not(feature = "dynamic-withheld"))]
    panic!(
        "FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY requires building wm2000-block-boot with --features dynamic-withheld; requested {:?}",
        value
    );
    #[cfg(feature = "dynamic-withheld")]
    {
        assert!(
            value == "1",
            "FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY accepts only the exact token 1"
        );
        true
    }
}

fn generated_runner_build_identity_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_BUILD_IDENTITY_ARGUMENT_V1,
                )
    )
}

fn generated_runner_bootstrap_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_BOOTSTRAP_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_si_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_SI_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_cpu_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_CPU_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_pi_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_PI_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_rdp_renderer_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_RDP_RENDERER_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_rsp_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_RSP_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_host_abi_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_HOST_ABI_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_sp_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_SP_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn bootstrap_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_BOOTSTRAP_RUNTIME_NONCE_ENV_V1)
        .expect("fixed bootstrap audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed bootstrap audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn si_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_SI_RUNTIME_NONCE_ENV_V1)
        .expect("fixed SI audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed SI audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn cpu_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_CPU_RUNTIME_NONCE_ENV_V1)
        .expect("fixed CPU audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed CPU audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn pi_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_PI_RUNTIME_NONCE_ENV_V1)
        .expect("fixed PI audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed PI audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn rdp_renderer_audit_nonce() -> String {
    let nonce =
        std::env::var(fn64_boot_harness::GENERATED_RUNNER_RDP_RENDERER_RUNTIME_NONCE_ENV_V1)
            .expect("fixed RDP renderer audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed RDP renderer audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn rsp_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_RSP_RUNTIME_NONCE_ENV_V1)
        .expect("fixed RSP audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed RSP audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn host_abi_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_HOST_ABI_RUNTIME_NONCE_ENV_V1)
        .expect("fixed Host ABI audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed Host ABI audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn sp_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_SP_RUNTIME_NONCE_ENV_V1)
        .expect("fixed SP audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed SP audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn protocol_adapter_role(
    role: GeneratedAdapterRole,
) -> fn64_boot_harness::GeneratedRunnerAdapterRoleV1 {
    match role {
        GeneratedAdapterRole::DirectGenerated => {
            fn64_boot_harness::GeneratedRunnerAdapterRoleV1::DirectGenerated
        }
        GeneratedAdapterRole::EntryContextGate => {
            fn64_boot_harness::GeneratedRunnerAdapterRoleV1::EntryContextGate
        }
        GeneratedAdapterRole::DenseInstrumentationGate => {
            fn64_boot_harness::GeneratedRunnerAdapterRoleV1::DenseInstrumentationGate
        }
        GeneratedAdapterRole::OverlayGenerationGate => {
            fn64_boot_harness::GeneratedRunnerAdapterRoleV1::OverlayGenerationGate
        }
        GeneratedAdapterRole::ExternalDigestGate => {
            fn64_boot_harness::GeneratedRunnerAdapterRoleV1::ExternalDigestGate
        }
    }
}

fn generated_runner_build_identity(
    program: &CatalogBlockProgramV1,
    bindings: &[CargoGeneratedRunnerSourceBindingV1],
) -> fn64_boot_harness::GeneratedRunnerBuildIdentityV1 {
    let attestation = program
        .generated_runner_source_attestation()
        .expect("identity mode requires the exact Cargo source attestation");
    let build_receipt = attestation.build_receipt();
    let mut bindings = bindings.to_vec();
    bindings.sort_unstable_by_key(|binding| binding.bank);
    let runners = bindings
        .into_iter()
        .map(
            |binding| fn64_boot_harness::GeneratedRunnerLinkedIdentityV1 {
                bank: binding.bank.get(),
                generated_runner_source_sha256: sha256_hex(binding.generated_runner_source_sha256),
                code_words_sha256: sha256_hex(binding.code_words_sha256),
                vram_start: binding.vram_start.get(),
                vram_end: binding.vram_end.get(),
                composite_subrunner_count: binding.composite_subrunner_count,
                adapter_role: protocol_adapter_role(binding.adapter_role),
            },
        )
        .collect();
    fn64_boot_harness::GeneratedRunnerBuildIdentityV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V3.to_owned(),
        package: env!("CARGO_PKG_NAME").to_owned(),
        manifest_sha256: sha256_hex(pack::MANIFEST_SHA256),
        lock_sha256: sha256_hex(pack::LOCK_SHA256),
        source_attestation_schema: attestation.schema().to_owned(),
        cargo_source_fields_validated: attestation.cargo_source_fields_validated(),
        program_identity_sha256: sha256_hex(attestation.program_identity().bytes()),
        root_adapter_source_sha256: sha256_hex(attestation.root_adapter_source_sha256()),
        shard_cargo_source_tree_sha256: sha256_hex(attestation.shard_cargo_source_tree_sha256()),
        emitter_source_sha256: sha256_hex(attestation.emitter_source_sha256()),
        runtime_source_sha256: sha256_hex(attestation.runtime_source_sha256()),
        prepared_source_mode: pack::PREPARED_SOURCE_MODE.to_owned(),
        normalized_rom_sha256: sha256_hex(pack::NORMALIZED_ROM_SHA256),
        prepared_manifest_sha256: sha256_hex(pack::PREPARED_MANIFEST_SHA256),
        prepared_tree_sha256: sha256_hex(pack::PREPARED_TREE_SHA256),
        prepared_generator_source_sha256: sha256_hex(pack::PREPARED_GENERATOR_SOURCE_SHA256),
        prepared_discovery_source_sha256: sha256_hex(pack::PREPARED_DISCOVERY_SOURCE_SHA256),
        prepared_emitter_source_sha256: sha256_hex(pack::PREPARED_EMITTER_SOURCE_SHA256),
        prepared_runtime_source_sha256: sha256_hex(pack::PREPARED_RUNTIME_SOURCE_SHA256),
        prepared_materializer_source_sha256: sha256_hex(pack::PREPARED_MATERIALIZER_SOURCE_SHA256),
        producer_manifest_sha256: sha256_hex(pack::PREPARED_PRODUCER_MANIFEST_SHA256),
        producer_lock_sha256: sha256_hex(pack::PREPARED_PRODUCER_LOCK_SHA256),
        producer_cargo_graph_sha256: sha256_hex(pack::PREPARED_PRODUCER_CARGO_GRAPH_SHA256),
        producer_cargo_source_sha256: sha256_hex(pack::PREPARED_PRODUCER_CARGO_SOURCE_SHA256),
        producer_binary_sha256: sha256_hex(pack::PREPARED_PRODUCER_BINARY_SHA256),
        binding_sha256: sha256_hex(attestation.binding_sha256()),
        build_receipt_schema: build_receipt.schema,
        aot_runtime: build_receipt.aot_runtime,
        production_aot: build_receipt.production_aot,
        dev_interpreter: build_receipt.dev_interpreter,
        runners,
    }
}

fn emit_generated_runner_build_identity(
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
) {
    let wire = serde_json::to_string(&identity)
        .expect("generated-runner build identity serialization is infallible");
    std::println!(
        "{}{wire}",
        fn64_boot_harness::GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1
    );
}

fn bootstrap_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedBootstrapWriterChannelReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerBootstrapRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    let journal_entry = &evidence.journal_entry;
    fn64_boot_harness::GeneratedRunnerBootstrapRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::BootstrapWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            bootstrap_receipt_sha256: sha256_hex(evidence.bootstrap_receipt_sha256),
            rom_sha256: sha256_hex(evidence.rom_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            generation_catalog_sha256: sha256_hex(evidence.generation_catalog_sha256),
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::BootstrapWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            bootstrap_watched_sha256: sha256_hex(evidence.bootstrap_watched_sha256),
            initial_generations: evidence
                .initial_generations
                .iter()
                .map(|generation| generation.get())
                .collect(),
            journal_entry: fn64_boot_harness::BootstrapMutationBatchV1 {
                sequence: journal_entry.sequence,
                declared_writes: journal_entry
                    .declared_writes
                    .iter()
                    .map(|write| {
                        assert_eq!(
                            write.channel,
                            fn64_recomp_rs::WriterChannel::BootstrapOrImport,
                            "bootstrap receipt contains another writer channel"
                        );
                        fn64_boot_harness::BootstrapAttributedWriteV1 {
                            channel: fn64_boot_harness::BootstrapWriterChannelV1::BootstrapOrImport,
                            physical_start: write.physical_start,
                            physical_end: write.physical_end,
                        }
                    })
                    .collect(),
                changed_ranges: journal_entry
                    .changed_ranges
                    .iter()
                    .map(|range| fn64_boot_harness::BootstrapWriterWatchedRangeV1 {
                        physical_start: range.physical_start,
                        physical_end: range.physical_end,
                    })
                    .collect(),
                before_sha256: sha256_hex(journal_entry.before_sha256),
                after_sha256: sha256_hex(journal_entry.after_sha256),
                invalidated_generations: journal_entry
                    .invalidated_generations
                    .iter()
                    .map(|generation| generation.get())
                    .collect(),
                journal_root_sha256: sha256_hex(journal_entry.journal_root_sha256),
            },
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn si_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedSiWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerSiRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    fn64_boot_harness::GeneratedRunnerSiRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_SI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::SiWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::SiWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            journal_entry_count: evidence.journal_entry_count,
            si_journal_declaration_count: evidence.si_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            si_started: evidence.si_started,
            si_committed: evidence.si_committed,
            si_pif_to_dram_committed: evidence.si_pif_to_dram_committed,
            si_transition_sha256: sha256_hex(evidence.si_transition_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn cpu_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedCpuWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerCpuRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    fn64_boot_harness::GeneratedRunnerCpuRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_CPU_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::CpuWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::CpuWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            journal_entry_count: evidence.journal_entry_count,
            cpu_journal_declaration_count: evidence.cpu_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            cpu_store_count: evidence.cpu_store_count,
            cpu_store_trace_sha256: sha256_hex(evidence.cpu_store_trace_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn host_abi_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedHostAbiWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerHostAbiRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    fn64_boot_harness::GeneratedRunnerHostAbiRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::HostAbiWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            initial_journal_entry_count: evidence.initial_journal_entry_count,
            final_journal_entry_count: evidence.final_journal_entry_count,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::HostAbiWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            host_abi_journal_entry_count: evidence.host_abi_journal_entry_count,
            host_abi_journal_declaration_count: evidence.host_abi_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            transactions_started: evidence.transactions_started,
            transactions_finished: evidence.transactions_finished,
            ordering_boundaries: evidence.ordering_boundaries,
            lifecycle_sha256: sha256_hex(evidence.lifecycle_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn pi_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedPiWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerPiRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    fn64_boot_harness::GeneratedRunnerPiRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_PI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::PiWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::PiWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            journal_entry_count: evidence.journal_entry_count,
            pi_journal_declaration_count: evidence.pi_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            pi_started: evidence.pi_started,
            pi_committed: evidence.pi_committed,
            pi_busy_cleared: evidence.pi_busy_cleared,
            pi_interrupt_raised: evidence.pi_interrupt_raised,
            pi_interrupt_cleared: evidence.pi_interrupt_cleared,
            pi_notifications: evidence.pi_notifications,
            pi_to_rdram_committed: evidence.pi_to_rdram_committed,
            pi_transition_sha256: sha256_hex(evidence.pi_transition_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn rdp_renderer_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedRdpRendererWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerRdpRendererRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    assert!(
        evidence.renderer_publication_count != 0
            && evidence.rdp_renderer_journal_entry_count != 0
            && evidence.rdp_renderer_journal_declaration_count != 0
            && evidence.final_journal_entry_count > evidence.initial_journal_entry_count,
        "fixed RDP renderer audit requires a committed executable-byte publication"
    );
    fn64_boot_harness::GeneratedRunnerRdpRendererRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_SCHEMA_V1
            .to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::RdpRendererWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            initial_journal_entry_count: evidence.initial_journal_entry_count,
            final_journal_entry_count: evidence.final_journal_entry_count,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::RdpRendererWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            rdp_renderer_journal_entry_count: evidence.rdp_renderer_journal_entry_count,
            rdp_renderer_journal_declaration_count: evidence.rdp_renderer_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            renderer_publication_count: evidence.renderer_publication_count,
            publication_trace_sha256: sha256_hex(evidence.publication_trace_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn rsp_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedRspWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerRspRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    assert!(
        evidence.interpreter_writeback_count != 0
            || evidence.translated_audio_hle_publication_count != 0,
        "fixed RSP audit requires a committed typed writeback publication"
    );
    fn64_boot_harness::GeneratedRunnerRspRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_RSP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::RspWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::RspWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            journal_entry_count: evidence.journal_entry_count,
            rsp_journal_declaration_count: evidence.rsp_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            interpreter_writeback_count: evidence.interpreter_writeback_count,
            translated_audio_hle_publication_count: evidence.translated_audio_hle_publication_count,
            writeback_range_count: evidence.writeback_range_count,
            writeback_trace_sha256: sha256_hex(evidence.writeback_trace_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn sp_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedSpWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerSpRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    fn64_boot_harness::GeneratedRunnerSpRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_SP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::SpWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::SpWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            journal_entry_count: evidence.journal_entry_count,
            sp_journal_declaration_count: evidence.sp_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            sp_started: evidence.sp_started,
            sp_queued: evidence.sp_queued,
            sp_committed: evidence.sp_committed,
            sp_busy_cleared: evidence.sp_busy_cleared,
            sp_rsp_to_rdram_committed: evidence.sp_rsp_to_rdram_committed,
            sp_transition_sha256: sha256_hex(evidence.sp_transition_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn take_completed_si_audit_receipt(
) -> Option<fn64_abi::recompiled::ValidatedSiWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::SiWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_si_writer_runtime_state_receipt_v1() {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed SI audit mode has no canonical runtime owner"),
        Err(
            Error::PendingDeviceSi
            | Error::PendingAbiSi
            | Error::NoSiTransitions
            | Error::NoPifToDramCommit,
        ) => None,
        Err(error) => panic!("fixed SI audit invariant failed: {error}"),
    }
}

fn take_completed_cpu_audit_receipt(
    epoch: &fn64_abi::recompiled::CpuWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedCpuWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::CpuWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_cpu_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed CPU audit mode has no unconsumed canonical runtime owner"),
        Err(Error::NoCpuStores) => None,
        Err(error) => panic!("fixed CPU audit invariant failed: {error}"),
    }
}

fn take_completed_pi_audit_receipt(
    epoch: &fn64_abi::recompiled::PiWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedPiWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::PiWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_pi_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed PI audit mode has no unconsumed canonical runtime owner"),
        Err(
            Error::PendingDevicePi
            | Error::PendingAbiPi
            | Error::NoPiTransitions
            | Error::NoToRdramCommit,
        ) => None,
        Err(error) => panic!("fixed PI audit invariant failed: {error}"),
    }
}

fn take_completed_rdp_renderer_audit_receipt(
    epoch: &fn64_abi::recompiled::RdpRendererWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedRdpRendererWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::RdpRendererWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_rdp_renderer_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed RDP renderer audit mode has no unconsumed canonical owner"),
        Err(
            Error::PendingDeviceRspTask
            | Error::PendingDeviceDpcTransaction
            | Error::PendingDeviceDpCompletion
            | Error::PendingAbiRendererWork
            | Error::NoRendererPublications,
        ) => None,
        Err(error) => panic!("fixed RDP renderer audit invariant failed: {error}"),
    }
}

fn take_completed_rsp_audit_receipt(
    epoch: &fn64_abi::recompiled::RspWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedRspWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::RspWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_rsp_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed RSP audit mode has no unconsumed canonical owner"),
        Err(Error::PendingDeviceRspTask | Error::PendingAbiRspWork | Error::NoRspWritebacks) => {
            None
        }
        Err(error) => panic!("fixed RSP audit invariant failed: {error}"),
    }
}

fn take_completed_host_abi_audit_receipt(
    epoch: &fn64_abi::recompiled::HostAbiWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedHostAbiWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::HostAbiWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_host_abi_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed Host ABI audit mode has no unconsumed canonical runtime owner"),
        Err(Error::NoHostAbiTransactions | Error::NoHostAbiWriteCommit) => None,
        Err(error) => panic!("fixed Host ABI audit invariant failed: {error}"),
    }
}

fn take_completed_sp_audit_receipt(
    epoch: &fn64_abi::recompiled::SpWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedSpWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::SpWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_sp_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed SP audit mode has no unconsumed canonical runtime owner"),
        Err(
            Error::PendingDeviceSpDma
            | Error::PendingDeviceSpTask
            | Error::PendingAbiSpWork
            | Error::NoSpTransitions
            | Error::NoRspToRdramCommit,
        ) => None,
        Err(error) => panic!("fixed SP audit invariant failed: {error}"),
    }
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
        fn64_abi::set_audio_rdram_len(fn64_recomp_rs::RDRAM_LEN);
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
        assert_eq!(
            pack::ROM_COPY,
            (0x1000, 0x101000, 0x80000400),
            "NWXE block pack no longer matches the IPL3 one-MiB boot DMA contract"
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
    assert_eq!(
        DENSE_AOT_ARTIFACTS.len(),
        pack::BOOT_SHARDS.len()
            + pack::RESIDENT_TAIL_SHARDS.len()
            + pack::OVERLAY_GENERATIONS
                .iter()
                .map(|generation| generation.shards.len())
                .sum::<usize>()
    );
    assert_eq!(DENSE_AOT_IDENTITIES.len(), DENSE_AOT_ARTIFACTS.len());
    let mut generated_runner_bindings = Vec::with_capacity(DENSE_AOT_ARTIFACTS.len() + 1);
    for (artifact_index, ((artifact, identity), expected)) in DENSE_AOT_ARTIFACTS
        .iter()
        .zip(DENSE_AOT_IDENTITIES)
        .take(pack::BOOT_SHARDS.len())
        .zip(pack::BOOT_SHARDS)
        .enumerate()
    {
        assert_eq!(artifact.bank_id, expected.bank_id);
        assert_eq!(identity.source_sha256, expected.source_sha256);
        let bank = BankId::new(expected.bank_id);
        let code_bank = (artifact.code_bank)();
        assert_eq!(code_bank.id(), bank);
        assert_eq!(code_bank_sha256(&code_bank), expected.code_sha256);
        assert_eq!(code_bank.vram_start(), GuestPc::new(expected.va_start));
        assert_eq!(
            code_bank.vram_end(),
            GuestPc::new(expected.va_start + expected.byte_len)
        );
        let mut region = ExecutableRegion::new(
            GuestPc::new(expected.va_start),
            GuestPc::new(expected.va_start + expected.byte_len),
        );
        let (runner, role) = if full_aot_instrumentation {
            (
                run_dense_aot_with_context_gate as GeneratedBankFn,
                GeneratedAdapterRole::DenseInstrumentationGate,
            )
        } else if artifact_index == 0 {
            (
                run_entry_aot_with_context_gate as GeneratedBankFn,
                GeneratedAdapterRole::EntryContextGate,
            )
        } else {
            (artifact.runner, GeneratedAdapterRole::DirectGenerated)
        };
        region
            .install(
                &mut program,
                code_bank,
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    runner,
                    ProgramArtifactIdentity::generated_adapter(
                        pack::ROOT_ADAPTER_SOURCE_SHA256,
                        identity.runner_source_sha256,
                        bank,
                        role,
                    ),
                ),
            )
            .expect("installing dense boot-shard runner");
        generated_runner_bindings.push(CargoGeneratedRunnerSourceBindingV1 {
            bank,
            generated_runner_source_sha256: identity.runner_source_sha256,
            code_words_sha256: expected.code_sha256,
            vram_start: GuestPc::new(expected.va_start),
            vram_end: GuestPc::new(expected.va_start + expected.byte_len),
            composite_subrunner_count: expected.byte_len.div_ceil(2 * 1024),
            adapter_role: role,
        });
    }
    let dynamic_shards = std::iter::once(&pack::RESIDENT_TAIL_GENERATION)
        .chain(pack::OVERLAY_GENERATIONS.iter())
        .flat_map(|generation| generation.shards.iter());
    for (dynamic_index, ((artifact, identity), expected)) in DENSE_AOT_ARTIFACTS
        .iter()
        .zip(DENSE_AOT_IDENTITIES)
        .skip(pack::BOOT_SHARDS.len())
        .zip(dynamic_shards)
        .enumerate()
    {
        assert_eq!(artifact.bank_id, expected.bank_id);
        assert_eq!(identity.source_sha256, expected.source_sha256);
        let bank = BankId::new(artifact.bank_id);
        let code = (artifact.code_bank)();
        assert_eq!(code.id(), bank);
        assert_eq!(code_bank_sha256(&code), expected.code_sha256);
        assert_eq!(code.vram_start(), GuestPc::new(expected.va_start));
        assert_eq!(
            code.vram_end(),
            GuestPc::new(expected.va_start + expected.byte_len)
        );
        let (runner, role) = if full_aot_instrumentation {
            (
                run_dense_aot_with_context_gate as GeneratedBankFn,
                GeneratedAdapterRole::DenseInstrumentationGate,
            )
        } else if dynamic_index < pack::RESIDENT_TAIL_SHARDS.len() {
            (artifact.runner, GeneratedAdapterRole::DirectGenerated)
        } else {
            (
                run_overlay_aot_with_generation_gate as GeneratedBankFn,
                GeneratedAdapterRole::OverlayGenerationGate,
            )
        };
        program
            .register(
                code,
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    runner,
                    ProgramArtifactIdentity::generated_adapter(
                        pack::ROOT_ADAPTER_SOURCE_SHA256,
                        identity.runner_source_sha256,
                        bank,
                        role,
                    ),
                ),
            )
            .expect("pre-registering immutable dynamic AOT artifact");
        generated_runner_bindings.push(CargoGeneratedRunnerSourceBindingV1 {
            bank,
            generated_runner_source_sha256: identity.runner_source_sha256,
            code_words_sha256: expected.code_sha256,
            vram_start: GuestPc::new(expected.va_start),
            vram_end: GuestPc::new(expected.va_start + expected.byte_len),
            composite_subrunner_count: expected.byte_len.div_ceil(2 * 1024),
            adapter_role: role,
        });
    }
    let mut generation_catalog = PrecompiledGenerationCatalog::new();
    let mut generation_backings = Vec::new();
    let mut dense_definition_catalog = PrecompiledGenerationCatalog::new();
    let mut dense_definition_backings = Vec::new();
    for generation in
        std::iter::once(&pack::RESIDENT_TAIL_GENERATION).chain(pack::OVERLAY_GENERATIONS.iter())
    {
        let generation_id = GenerationId::new(generation.id);
        let image_start = GuestPc::new(generation.image_start);
        let image_end = GuestPc::new(generation.image_end);
        let invalidation_start = GuestPc::new(generation.invalidation_start);
        let invalidation_end = GuestPc::new(generation.invalidation_end);
        let shards = generation
            .shards
            .iter()
            .map(|shard| {
                PrecompiledShard::new(
                    BankId::new(shard.bank_id),
                    GuestPc::new(shard.va_start),
                    GuestPc::new(shard.va_start + shard.byte_len),
                )
                .expect("generated dynamic shard geometry is valid")
            })
            .collect::<Vec<_>>();
        let compiled_generation = PrecompiledGeneration::new(
            generation_id,
            image_start,
            image_end,
            invalidation_start,
            invalidation_end,
            generation.sha256,
            shards,
        )
        .expect("generated dynamic generation geometry is valid");
        dense_definition_catalog
            .register(compiled_generation.clone())
            .expect("dense generated generation catalog is unambiguous");
        generation_catalog
            .register(compiled_generation)
            .expect("generated dynamic generation catalog is unambiguous");
        assert!(
            (0x8000_0000..0xc000_0000).contains(&invalidation_start.get())
                && invalidation_end.get() <= 0xc000_0000,
            "generated dynamic generation backing must be direct-mapped KSEG"
        );
        let backing = PrecompiledGenerationBackingV1::new(
            generation_id,
            vec![BackedExecutableSpanV1::new(
                invalidation_start,
                invalidation_start.get() & 0x1fff_ffff,
                invalidation_end.get() - invalidation_start.get(),
            )
            .expect("generated dynamic physical backing is valid")],
        )
        .expect("generated dynamic generation backing is contiguous");
        dense_definition_backings.push(backing.clone());
        generation_backings.push(backing);
    }
    let dense_definition = BackedPrecompiledGenerationCatalogV1::new(
        dense_definition_catalog,
        dense_definition_backings,
    )
    .expect("dense generated generations have exact physical backings");
    assert_eq!(
        dense_definition.canonical_definition_sha256(),
        pack::DENSE_GENERATION_CATALOG_DEFINITION_SHA256,
        "runtime dense generation catalog must equal the build-time ROM-derived definition"
    );
    for image in pack::EXTERNAL_EXECUTABLE_IMAGES {
        let bank = BankId::new(image.bank_id);
        let image_start = GuestPc::new(image.va_start);
        let image_end = GuestPc::new(image.va_end);
        register_external_executable_generation(
            &mut generation_catalog,
            &mut generation_backings,
            bank,
            image_start,
            image_end,
            image.sha256,
        );
        let code = CodeBank::new(bank, GuestPc::new(image.va_start), image.words.to_vec())
            .expect("admitting captured exception-vector image");
        assert_eq!(code_bank_sha256(&code), image.sha256);
        let mut region =
            ExecutableRegion::new(GuestPc::new(image.va_start), GuestPc::new(image.va_end));
        region
            .install(
                &mut program,
                code,
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    run_nwxe_exception_image_with_digest_gate,
                    ProgramArtifactIdentity::generated_adapter(
                        pack::ROOT_ADAPTER_SOURCE_SHA256,
                        pack::EXTERNAL_RUNNER_SOURCE_SHA256,
                        bank,
                        GeneratedAdapterRole::ExternalDigestGate,
                    ),
                ),
            )
            .expect("installing captured exception-vector runner");
        generated_runner_bindings.push(CargoGeneratedRunnerSourceBindingV1 {
            bank,
            generated_runner_source_sha256: pack::EXTERNAL_RUNNER_SOURCE_SHA256,
            code_words_sha256: image.sha256,
            vram_start: GuestPc::new(image.va_start),
            vram_end: GuestPc::new(image.va_end),
            composite_subrunner_count: 1,
            adapter_role: GeneratedAdapterRole::ExternalDigestGate,
        });
    }
    let program_evidence = program.evidence_snapshot();
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
    let instruction_budget = std::env::var("FN64_BLOCK_INSTRUCTION_BUDGET")
        .map(|value| {
            value
                .parse::<u32>()
                .ok()
                .and_then(InstructionBudget::new)
                .expect("FN64_BLOCK_INSTRUCTION_BUDGET must be an integer of at least two")
        })
        .unwrap_or_else(|_| InstructionBudget::new(4096).expect("nonzero budget"));
    let catalog_program =
        CatalogBlockProgramV1::new_with_cargo_generated_runner_source_attestation_v2(
            program,
            ExecutionKey::new(entry_bank(), GuestPc::new(pack::ENTRYPOINT)),
            instruction_budget,
            CargoGeneratedProgramSourceAttestationV2 {
                root_adapter_source_sha256: pack::ROOT_ADAPTER_SOURCE_SHA256,
                shard_cargo_source_tree_sha256: pack::SHARD_CARGO_SOURCE_TREE_SHA256,
                expected_emitter_source_sha256: pack::EMITTER_SOURCE_SHA256,
                externally_measured_emitter_source_sha256:
                    fn64_recomp_rs_codegen::generated_runner_emitter_source_receipt_v2()
                        .source_sha256(),
                expected_runtime_source_sha256: pack::RUNTIME_SOURCE_SHA256,
                runtime_source_receipt: fn64_recomp_rs::generated_runner_runtime_source_receipt_v1(
                ),
                runners: &generated_runner_bindings,
            },
        )
        .expect("Cargo-source-attested block program has one admitted fixed entry");
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
    use fn64_abi::recompiled::{AbiHostShimBindingV1 as Binding, AbiHostShimV1 as Shim};
    let host_functions = fn64_abi::recompiled::issue_abi_host_function_catalog_v1(vec![
        Binding {
            target_pc: pack::OS_SI_DEVICE_BUSY,
            shim: Shim::OsSiDeviceBusy,
        },
        Binding {
            target_pc: pack::OS_CREATE_MESG_QUEUE,
            shim: Shim::OsCreateMesgQueue,
        },
        Binding {
            target_pc: pack::OS_EPI_START_DMA,
            shim: Shim::OsEPiStartDma,
        },
        Binding {
            target_pc: pack::OS_RECV_MESG,
            shim: Shim::OsRecvMesg,
        },
        Binding {
            target_pc: pack::OS_SEND_MESG,
            shim: Shim::OsSendMesg,
        },
        Binding {
            target_pc: pack::OS_CREATE_THREAD,
            shim: Shim::OsCreateThread,
        },
        Binding {
            target_pc: pack::OS_SET_EVENT_MESG,
            shim: Shim::OsSetEventMesg,
        },
        Binding {
            target_pc: pack::OS_START_THREAD,
            shim: Shim::OsStartThread,
        },
        Binding {
            target_pc: pack::OS_GET_THREAD_PRI,
            shim: Shim::OsGetThreadPri,
        },
        Binding {
            target_pc: pack::OS_SET_THREAD_PRI,
            shim: Shim::OsSetThreadPri,
        },
        Binding {
            target_pc: pack::OS_SET_TIMER,
            shim: Shim::OsSetTimer,
        },
        Binding {
            target_pc: pack::OS_SP_TASK_LOAD,
            shim: Shim::OsSpTaskLoad,
        },
        Binding {
            target_pc: pack::OS_SP_TASK_START_GO,
            shim: Shim::OsSpTaskStartGo,
        },
        Binding {
            target_pc: pack::OS_SP_TASK_YIELD,
            shim: Shim::OsSpTaskYield,
        },
        Binding {
            target_pc: pack::OS_SP_TASK_YIELDED,
            shim: Shim::OsSpTaskYielded,
        },
    ])
    .expect("ABI-issued host-function catalog is exact and unambiguous");
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
    let exit = fn64_abi::prepare_process_exit();
    println!(
        "[wm2000-block-boot] process exit prepared: threads={} detached_coroutines={}",
        exit.threads, exit.detached_coroutines
    );
}
