use crate::*;

pub(crate) fn write_pc_trace(
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

pub(crate) fn write_host_boundary_trace(boundaries: &[fn64_abi::recompiled::BlockHostBoundaryObservation]) {
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
pub(crate) struct DynamicTelemetryOutput {
    final_path: std::path::PathBuf,
    partial_path: std::path::PathBuf,
    file: std::fs::File,
}

#[cfg(feature = "dynamic-withheld")]
pub(crate) fn prepare_dynamic_telemetry_output() -> DynamicTelemetryOutput {
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
pub(crate) fn build_dynamic_withheld_telemetry(
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
pub(crate) fn commit_dynamic_withheld_telemetry(
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

pub(crate) fn print_runtime_progress() {
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
    // Audio submissions count TASKS. These count SIGNAL: a regression to
    // silence leaves audio_submits untouched while nonzero_samples goes to
    // zero, which is the whole reason the block lane installs a PCM backend
    // instead of registering only the RDRAM bound.
    {
        use std::sync::atomic::Ordering::Relaxed;
        println!(
            "[wm2000-block-audio] buffers={} samples={} nonzero_samples={} peak_abs={}",
            crate::AUDIO_BUFFERS.load(Relaxed),
            crate::AUDIO_SAMPLES.load(Relaxed),
            crate::AUDIO_NONZERO_SAMPLES.load(Relaxed),
            crate::AUDIO_PEAK_ABS.load(Relaxed),
        );
    }
    let timing = fn64_abi::phase_timing();
    if timing.executor_calls > 0 {
        println!(
            "[wm2000-block-profile] phase_timing executor_ms={:.3} calls={} gfx_ms={:.3} phases={} gfx_lle_ms={:.3} tasks={} gfx_lle_rsp_ms={:.3} gfx_lle_rdp_ms={:.3} audio_ms={:.3} tasks={} vi_present_ms={:.3} fields={}",
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
            timing.vi_present_ns as f64 / 1e6,
            timing.vi_present_calls,
        );
        println!(
            "[wm2000-block-profile] phase_timing audio_lle_ms={:.3} tasks={} audio_lle_rsp_ms={:.3}",
            timing.audio_lle_ns as f64 / 1e6,
            timing.audio_lle_calls,
            timing.audio_lle_rsp_ns as f64 / 1e6,
        );
        // `executor_ms` is INCLUSIVE of everything dispatched beneath
        // `run_one_step`. Print the subtraction so a reader cannot repeat the
        // rule-2 error (inclusive read as self time) that produced three
        // artifact targets; see docs/plans/perf-method.md.
        //
        // `vi_present_ns` USED TO BE SUBTRACTED HERE AND SHOULD NOT HAVE BEEN.
        // Presentation is reached only from `pi::timing`'s
        // `advance_device_time_step`, which the harness drives through
        // `advance_virtual_time` on its `AdvanceField` arm -- outside
        // `run_one_step`, and so never counted into `executor_ns` in the first
        // place. Subtracting it removed ~1.14 ms/field that was never added,
        // understating executor self time. Corroborated three ways: the call
        // graph, devtime measuring 0.251 ms/field against vi_present's ~1.14,
        // and the executor split closing to ~100% WITHOUT it.
        //
        // Rather than trusting that argument, the runtime now counts which
        // side of the seam each presentation ran on, and this line subtracts
        // `vi_present_ns` only if the counter says it belongs -- so the
        // arithmetic follows the observation instead of either assumption.
        let vi_is_nested = timing.vi_present_in_executor_calls > 0;
        let nested_ns = timing
            .gfx_ns
            .saturating_add(timing.audio_dispatch_ns)
            .saturating_add(timing.audio_lle_ns)
            .saturating_add(if vi_is_nested {
                timing.vi_present_ns
            } else {
                0
            });
        println!(
            "[wm2000-block-profile] phase_self executor_self_ms={:.3} (executor_ms minus gfx+audio+audio_lle{}={:.3})",
            timing.executor_ns.saturating_sub(nested_ns) as f64 / 1e6,
            if vi_is_nested { "+vi_present" } else { "" },
            nested_ns as f64 / 1e6,
        );
        // The evidence for the line above, printed beside it so the reader can
        // check the attribution rather than take it on faith.
        println!(
            "[wm2000-block-profile] vi_reachability in_executor={} outside_executor={} vi_present_ms={:.3} ({})",
            timing.vi_present_in_executor_calls,
            timing.vi_present_outside_executor_calls,
            timing.vi_present_ns as f64 / 1e6,
            if vi_is_nested {
                "nested in executor_ns -- subtracted above"
            } else {
                "OUTSIDE executor_ns -- correctly NOT subtracted"
            },
        );
    }
}

pub(crate) fn print_profiled_rdram_ranges() {
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
