use crate::*;

pub(crate) fn sha256_hex(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn diagnostic_hex_u32(value: u32) -> String {
    format!("0x{value:08x}")
}

pub(crate) fn diagnostic_hex_u64(value: u64) -> String {
    format!("0x{value:016x}")
}

pub(crate) fn diagnostic_execution_key(key: ExecutionKey) -> serde_json::Value {
    serde_json::json!({
        "bank": diagnostic_hex_u64(key.bank.get()),
        "pc": diagnostic_hex_u32(key.pc.get()),
    })
}

pub(crate) fn diagnostic_fault(fault: fn64_recomp_rs::CpuFault) -> serde_json::Value {
    serde_json::json!({
        "at": diagnostic_execution_key(fault.at),
        "kind": format!("{:?}", fault.kind),
    })
}

pub(crate) fn diagnostic_pending_exit(exit: fn64_recomp_rs::BlockExit) -> serde_json::Value {
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

pub(crate) fn diagnostic_prepared_continuation(
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

pub(crate) fn diagnostic_optional_hex_u32(value: Option<u32>) -> serde_json::Value {
    value
        .map(diagnostic_hex_u32)
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) fn diagnostic_cpu(
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

pub(crate) fn print_wm_publication_diagnostic_v1() {
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

pub(crate) struct WmOperationalBoundaryV1 {
    achieved_guest_instructions: u64,
    pub(crate) scheduler_steps: u64,
    pub(crate) sim_time: u64,
    pub(crate) logical_rdram_len: usize,
    pub(crate) logical_rdram_sha256: [u8; 32],
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

pub(crate) fn canonical_publication_thread(
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

pub(crate) fn capture_wm_operational_boundary_v1(
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

pub(crate) fn print_wm_operational_boundary_v1(boundary: &WmOperationalBoundaryV1) {
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

pub(crate) fn wm_operational_boundary_json_v1(boundary: &WmOperationalBoundaryV1) -> serde_json::Value {
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

pub(crate) fn dynamic_exact_entry_withheld() -> bool {
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
