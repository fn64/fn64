#![allow(clippy::module_inception)]
use super::*;

#[cfg(feature = "recomp-rs")]
pub(super) fn encode_publication_cpu_snapshot(
    out: &mut Vec<u8>,
    snapshot: &fn64_recomp_rs::RecompContextEvidenceSnapshotV1,
    thread: fn64_runtime::ThreadId,
    include_executor_mirrors: bool,
) -> Result<(), OperationalThreadPublicationDigestErrorV1> {
    for value in snapshot.gprs {
        push_u64(out, value);
    }
    push_u64(out, snapshot.hi);
    push_u64(out, snapshot.lo);
    for value in snapshot.physical_fgrs {
        push_u64(out, value);
    }
    out.push(u8::from(snapshot.fpu_cond));
    push_u32(out, snapshot.fcsr);
    match snapshot.ll_reservation {
        Some((address, width)) => {
            out.push(1);
            push_u64(out, address);
            out.push(width);
        }
        None => out.push(0),
    }
    if include_executor_mirrors {
        push_u32(out, snapshot.cop0_count);
        push_u32(out, snapshot.cop0_compare);
        push_option_u32(out, snapshot.cop0_count_write);
        push_option_u32(out, snapshot.cop0_compare_write);
    } else if snapshot.cop0_count_write.is_some() || snapshot.cop0_compare_write.is_some() {
        return Err(OperationalThreadPublicationDigestErrorV1::PendingCop0TimingWrite { thread });
    }
    out.push(u8::from(snapshot.cop0_cond));
    push_u32(out, snapshot.cop0_status);
    let cop0_cause = if include_executor_mirrors {
        snapshot.cop0_cause
    } else {
        snapshot.cop0_cause
            & !(fn64_recomp_rs::CpuInterruptLine::RCP.cause_bit()
                | fn64_recomp_rs::CpuInterruptLine::TIMER.cause_bit())
    };
    push_u32(out, cop0_cause);
    push_u32(out, snapshot.cop0_epc);
    push_u32(out, snapshot.cop0_error_epc);
    push_u64(out, snapshot.cop0_badvaddr);
    push_u32(out, snapshot.cop0_context);
    push_u64(out, snapshot.cop0_xcontext);
    push_u32(out, snapshot.cop0_index);
    for entry in snapshot.tlb_entries {
        push_u32(out, entry.page_mask);
        push_u64(out, entry.entry_hi);
        push_u32(out, entry.entry_lo0);
        push_u32(out, entry.entry_lo1);
    }
    push_u32(out, snapshot.cop0_entry_lo0);
    push_u32(out, snapshot.cop0_entry_lo1);
    push_u32(out, snapshot.cop0_page_mask);
    push_u32(out, snapshot.cop0_wired);
    push_u64(out, snapshot.cop0_entry_hi);
    push_u32(out, snapshot.cop0_random_phase);
    push_u32(out, snapshot.cop0_watch_lo);
    push_u32(out, snapshot.cop0_watch_hi);
    push_u32(out, snapshot.os_interrupt_mask);
    push_option_u32(out, snapshot.thread_return_pc);
    Ok(())
}

#[cfg(feature = "recomp-rs")]
pub(super) fn publication_thread_v1(
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

#[cfg(feature = "recomp-rs")]
pub(super) fn encode_execution_key_v1(out: &mut Vec<u8>, key: fn64_recomp_rs::ExecutionKey) {
    push_u64(out, key.bank.get());
    push_u32(out, key.pc.get());
}

#[cfg(feature = "recomp-rs")]
pub(super) fn encode_instruction_identity_v1(
    out: &mut Vec<u8>,
    identity: fn64_recomp_rs::InstructionWordIdentity,
) {
    push_u64(out, identity.bank.get());
    push_u32(out, identity.physical_address);
}

#[cfg(feature = "recomp-rs")]
pub(super) fn cpu_exception_tag_v1(exception: fn64_recomp_rs::CpuException) -> u8 {
    use fn64_recomp_rs::CpuException;
    match exception {
        CpuException::TlbModified => 0,
        CpuException::TlbRefillLoad => 1,
        CpuException::TlbRefillStore => 2,
        CpuException::XTlbRefillLoad => 3,
        CpuException::XTlbRefillStore => 4,
        CpuException::TlbInvalidLoad => 5,
        CpuException::TlbInvalidStore => 6,
        CpuException::AddressErrorLoad => 7,
        CpuException::AddressErrorStore => 8,
        CpuException::CoprocessorUnusable => 9,
        CpuException::Syscall => 10,
        CpuException::Breakpoint => 11,
        CpuException::ReservedInstruction => 12,
        CpuException::Trap => 13,
        CpuException::IntegerOverflow => 14,
        CpuException::FloatingPoint => 15,
    }
}

#[cfg(feature = "recomp-rs")]
pub(super) fn encode_cpu_fault_v1(out: &mut Vec<u8>, fault: fn64_recomp_rs::CpuFault) {
    use fn64_recomp_rs::CpuFaultKind;
    encode_execution_key_v1(out, fault.at);
    match fault.kind {
        CpuFaultKind::UnalignedPc => out.push(0),
        CpuFaultKind::UnknownBank => out.push(1),
        CpuFaultKind::UnmappedPc {
            bank_start,
            bank_end,
        } => {
            out.push(2);
            push_u32(out, bank_start);
            push_u32(out, bank_end);
        }
        CpuFaultKind::AmbiguousPc {
            first_candidate,
            second_candidate,
            candidate_count,
        } => {
            out.push(3);
            push_u64(out, first_candidate.get());
            push_u64(out, second_candidate.get());
            push_u32(out, candidate_count);
        }
        CpuFaultKind::NoActiveGeneration => out.push(4),
        CpuFaultKind::UnmappedPhysicalInstruction { physical_address } => {
            out.push(5);
            push_u32(out, physical_address);
        }
        CpuFaultKind::StaleInstructionIdentity { expected, actual } => {
            out.push(6);
            encode_instruction_identity_v1(out, expected);
            encode_instruction_identity_v1(out, actual);
        }
        CpuFaultKind::MissingAotEntry => out.push(7),
        CpuFaultKind::MemoryFault { addr } => {
            out.push(8);
            push_u64(out, addr);
        }
        CpuFaultKind::UnsupportedInstruction { word } => {
            out.push(9);
            push_u32(out, word);
        }
        CpuFaultKind::Exception {
            exception,
            epc,
            branch_delay,
            instruction_code,
            bad_vaddr,
            coprocessor,
        } => {
            out.push(10);
            out.push(cpu_exception_tag_v1(exception));
            push_u32(out, epc.get());
            out.push(u8::from(branch_delay));
            push_u32(out, instruction_code);
            match bad_vaddr {
                Some(value) => {
                    out.push(1);
                    push_u64(out, value);
                }
                None => out.push(0),
            }
            match coprocessor {
                Some(value) => {
                    out.push(1);
                    out.push(value);
                }
                None => out.push(0),
            }
        }
    }
}

#[cfg(feature = "recomp-rs")]
pub(super) fn encode_block_exit_v1(out: &mut Vec<u8>, exit: fn64_recomp_rs::BlockExit) {
    use fn64_recomp_rs::BlockExit;
    match exit {
        BlockExit::Transfer(key) => {
            out.push(0);
            encode_execution_key_v1(out, key);
        }
        BlockExit::ResolveTransfer {
            source_bank,
            target_pc,
        } => {
            out.push(1);
            push_u64(out, source_bank.get());
            push_u32(out, target_pc.get());
        }
        BlockExit::ResolveCall {
            source_bank,
            target_pc,
            resume,
        } => {
            out.push(2);
            push_u64(out, source_bank.get());
            push_u32(out, target_pc.get());
            encode_execution_key_v1(out, resume);
        }
        BlockExit::HostCall { vram, resume } => {
            out.push(3);
            push_u32(out, vram.get());
            encode_execution_key_v1(out, resume);
        }
        BlockExit::ExecutableWrite {
            source_bank,
            resume,
        } => {
            out.push(4);
            push_u64(out, source_bank.get());
            encode_execution_key_v1(out, resume);
        }
        BlockExit::ExecutableWriteResolveCall {
            source_bank,
            target_pc,
            resume,
        } => {
            out.push(5);
            push_u64(out, source_bank.get());
            push_u32(out, target_pc.get());
            encode_execution_key_v1(out, resume);
        }
        BlockExit::ExecutableWriteFault(fault) => {
            out.push(6);
            encode_cpu_fault_v1(out, fault);
        }
        BlockExit::ImageChanged { at, miss } => {
            out.push(7);
            encode_execution_key_v1(out, at);
            push_u64(out, miss.expected_bank.get());
            push_u32(out, miss.va_start.get());
            push_u32(out, miss.byte_len);
            out.extend_from_slice(&miss.expected_sha256);
            out.extend_from_slice(&miss.actual_sha256);
        }
        BlockExit::Checkpoint(key) => {
            out.push(8);
            encode_execution_key_v1(out, key);
        }
        BlockExit::Yield(key) => {
            out.push(9);
            encode_execution_key_v1(out, key);
        }
        BlockExit::ThreadReturn => out.push(10),
        BlockExit::Fault(fault) => {
            out.push(11);
            encode_cpu_fault_v1(out, fault);
        }
    }
}

#[cfg(feature = "recomp-rs")]
pub(super) fn operational_thread_publication_sha256(schema: &str, domain: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(schema.as_bytes());
    digest.update([0]);
    digest.update(domain);
    digest.update([0]);
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    digest.finalize().into()
}

/// Hash pointer-free canonical guest-thread publications for operational A/B
/// diagnosis. Input must retain the producer's strict `ThreadId` order;
/// sorting here would hide duplicated or reordered publications. The result
/// cannot construct a release report or program/writer authority.
#[cfg(feature = "recomp-rs")]
pub fn operational_thread_publication_digests_v1(
    publications: &[fn64_abi::recompiled::CanonicalThreadPublicationV1],
) -> Result<OperationalThreadPublicationDigestsV1, OperationalThreadPublicationDigestErrorV1> {
    operational_thread_publication_digests(
        publications,
        OPERATIONAL_THREAD_PUBLICATION_DIGEST_SCHEMA_V1,
        true,
    )
}

/// Hash pointer-free canonical guest-thread publications for operational A/B
/// diagnosis without making execution partition size or dispatch-entry
/// hardware mirrors part of equality. The authoritative executor digest owns
/// Count, Count phase, Compare, and timer state; device/ABI digests own the RCP
/// line. This digest retains every context-owned CPU field and rejects pending
/// Count/Compare writes. It is meaningful only alongside equal executor,
/// device, and ABI component digests. The per-slice charge is still validated
/// because an impossible checkpoint must not become comparable merely by
/// omitting that scheduling detail.
#[cfg(feature = "recomp-rs")]
pub fn operational_thread_publication_digests_v2(
    publications: &[fn64_abi::recompiled::CanonicalThreadPublicationV1],
) -> Result<OperationalThreadPublicationDigestsV2, OperationalThreadPublicationDigestErrorV1> {
    let digest = operational_thread_publication_digests(
        publications,
        OPERATIONAL_THREAD_PUBLICATION_DIGEST_SCHEMA_V2,
        false,
    )?;
    Ok(OperationalThreadPublicationDigestsV2 {
        cpu_sha256: digest.cpu_sha256,
        continuation_sha256: digest.continuation_sha256,
        publication_count: digest.publication_count,
        exact_count: digest.exact_count,
        opaque_count: digest.opaque_count,
        opaque_host_count: digest.opaque_host_count,
        parked_fault_count: digest.parked_fault_count,
        returned_count: digest.returned_count,
    })
}

#[cfg(feature = "recomp-rs")]
pub(super) fn operational_thread_publication_digests(
    publications: &[fn64_abi::recompiled::CanonicalThreadPublicationV1],
    schema: &str,
    include_slice_charge: bool,
) -> Result<OperationalThreadPublicationDigestsV1, OperationalThreadPublicationDigestErrorV1> {
    use fn64_abi::recompiled::CanonicalThreadPublicationV1;

    let mut cpu = Vec::new();
    let mut continuation = Vec::new();
    push_u64(&mut cpu, publications.len() as u64);
    push_u64(&mut continuation, publications.len() as u64);

    let mut previous = None;
    let mut exact_count = 0_u64;
    let mut opaque_count = 0_u64;
    let mut opaque_host_count = 0_u64;
    let mut parked_fault_count = 0_u64;
    let mut returned_count = 0_u64;
    for (index, publication) in publications.iter().enumerate() {
        let thread = publication_thread_v1(publication);
        if let Some(previous) = previous {
            if thread <= previous {
                return Err(
                    OperationalThreadPublicationDigestErrorV1::NonStrictThreadOrder {
                        index,
                        previous,
                        current: thread,
                    },
                );
            }
        }
        previous = Some(thread);
        push_u32(&mut cpu, thread);
        push_u32(&mut continuation, thread);

        match publication {
            CanonicalThreadPublicationV1::Exact(checkpoint) => {
                if checkpoint.charged_instructions == 0
                    || checkpoint.canonical_charged_instructions_at_publication
                        < u64::from(checkpoint.charged_instructions)
                {
                    return Err(
                        OperationalThreadPublicationDigestErrorV1::InvalidExactCheckpointCharge {
                            thread,
                        },
                    );
                }
                let prepared_is_coherent = match checkpoint.prepared_continuation {
                    None => !matches!(
                        checkpoint.pending_exit,
                        fn64_recomp_rs::BlockExit::ImageChanged { .. }
                            | fn64_recomp_rs::BlockExit::Fault(fn64_recomp_rs::CpuFault {
                                kind: fn64_recomp_rs::CpuFaultKind::NoActiveGeneration,
                                ..
                            })
                    ),
                    Some(fn64_abi::recompiled::CanonicalPreparedContinuationV1::ImageChanged {
                        entry,
                    }) => matches!(
                        checkpoint.pending_exit,
                        fn64_recomp_rs::BlockExit::ImageChanged { at, .. } if entry.pc == at.pc
                    ),
                    Some(
                        fn64_abi::recompiled::CanonicalPreparedContinuationV1::InactiveGeneration {
                            entry,
                        },
                    ) => matches!(
                        checkpoint.pending_exit,
                        fn64_recomp_rs::BlockExit::Fault(fn64_recomp_rs::CpuFault {
                            at,
                            kind: fn64_recomp_rs::CpuFaultKind::NoActiveGeneration,
                            ..
                        }) if entry.pc == at.pc
                    ),
                };
                if !prepared_is_coherent {
                    return Err(
                        OperationalThreadPublicationDigestErrorV1::IncoherentPreparedContinuation {
                            thread,
                        },
                    );
                }
                exact_count += 1;
                cpu.push(1);
                encode_publication_cpu_snapshot(
                    &mut cpu,
                    &checkpoint.cpu,
                    thread,
                    include_slice_charge,
                )?;
                continuation.push(0);
                if include_slice_charge {
                    push_u32(&mut continuation, checkpoint.charged_instructions);
                }
                push_u64(
                    &mut continuation,
                    checkpoint.canonical_charged_instructions_at_publication,
                );
                encode_block_exit_v1(&mut continuation, checkpoint.pending_exit);
                match checkpoint.prepared_continuation {
                    None => continuation.push(0),
                    Some(fn64_abi::recompiled::CanonicalPreparedContinuationV1::ImageChanged {
                        entry,
                    }) => {
                        continuation.push(1);
                        encode_execution_key_v1(&mut continuation, entry);
                    }
                    Some(
                        fn64_abi::recompiled::CanonicalPreparedContinuationV1::InactiveGeneration {
                            entry,
                        },
                    ) => {
                        continuation.push(2);
                        encode_execution_key_v1(&mut continuation, entry);
                    }
                }
            }
            CanonicalThreadPublicationV1::OpaqueHostInFlight { target, resume, .. } => {
                opaque_count += 1;
                opaque_host_count += 1;
                cpu.push(0);
                continuation.push(1);
                push_u32(&mut continuation, target.get());
                encode_execution_key_v1(&mut continuation, *resume);
            }
            CanonicalThreadPublicationV1::ParkedFaultOpaque {
                post_exception_cpu,
                fault,
                canonical_charged_instructions_at_publication,
                ..
            } => {
                if !matches!(fault.kind, fn64_recomp_rs::CpuFaultKind::Exception { .. }) {
                    return Err(
                        OperationalThreadPublicationDigestErrorV1::ParkedFaultIsNotArchitecturalException {
                            thread,
                        },
                    );
                }
                opaque_count += 1;
                parked_fault_count += 1;
                cpu.push(2);
                encode_publication_cpu_snapshot(
                    &mut cpu,
                    post_exception_cpu,
                    thread,
                    include_slice_charge,
                )?;
                continuation.push(3);
                push_u64(
                    &mut continuation,
                    *canonical_charged_instructions_at_publication,
                );
                encode_cpu_fault_v1(&mut continuation, *fault);
            }
            CanonicalThreadPublicationV1::Returned { cpu: snapshot, .. } => {
                returned_count += 1;
                cpu.push(1);
                encode_publication_cpu_snapshot(&mut cpu, snapshot, thread, include_slice_charge)?;
                continuation.push(2);
            }
        }
    }

    Ok(OperationalThreadPublicationDigestsV1 {
        cpu_sha256: operational_thread_publication_sha256(schema, b"cpu", &cpu),
        continuation_sha256: operational_thread_publication_sha256(
            schema,
            b"continuation",
            &continuation,
        ),
        publication_count: publications.len() as u64,
        exact_count,
        opaque_count,
        opaque_host_count,
        parked_fault_count,
        returned_count,
    })
}

pub(super) fn try_encode_device_snapshot(
    snapshot: DeviceEvidenceSnapshot,
    executor: fn64_runtime::ExecutorControlEvidenceSnapshot,
    host: fn64_abi::AbiHostEvidenceSnapshot,
    program: crate::ProgramEvidenceSnapshot,
) -> Result<Vec<u8>, GateError> {
    let mut out = try_encode_device_component_v16(snapshot)?;
    out.extend_from_slice(&encode_executor_control_component(executor));
    out.extend_from_slice(&encode_abi_host_component(host));
    encode_program(&mut out, program);
    Ok(out)
}

pub(super) fn encode_timing_trace(events: &[TraceEvent]) -> Vec<u8> {
    let mut out = Vec::with_capacity(events.len() * 32);
    push_u64(&mut out, events.len() as u64);
    for event in events {
        push_u64(&mut out, event.sim_time);
        match event.kind {
            TraceKind::ThreadSwitch { from, to, reason } => {
                out.push(0);
                push_u32(&mut out, from.unwrap_or(u32::MAX));
                push_u32(&mut out, to);
                out.push(match reason {
                    SwitchReason::PauseSelf => 0,
                    SwitchReason::BlockedOnRecv => 1,
                    SwitchReason::BlockedOnSend => 2,
                    SwitchReason::Woken => 3,
                    SwitchReason::TimerFired => 4,
                    SwitchReason::Scheduled => 5,
                });
            }
            TraceKind::QueueOp { queue, op, thread } => {
                out.push(1);
                push_u32(&mut out, queue.offset());
                out.push(match op {
                    QueueOpKind::Send => 0,
                    QueueOpKind::Recv => 1,
                    QueueOpKind::Block => 2,
                    QueueOpKind::Wake => 3,
                    QueueOpKind::Drop => 4,
                });
                push_u32(&mut out, thread);
            }
            TraceKind::Dma {
                direction,
                dram,
                device,
                len,
            } => {
                out.push(2);
                out.push(match direction {
                    DmaDirection::ToRdram => 0,
                    DmaDirection::FromRdram => 1,
                });
                push_u32(&mut out, dram.offset());
                encode_pi_device_address(&mut out, device);
                push_u32(&mut out, len);
            }
            TraceKind::TaskSubmit { task_kind, ucode } => {
                out.push(3);
                out.push(match task_kind {
                    TaskKind::Graphics => 0,
                    TaskKind::Audio => 1,
                });
                push_u32(&mut out, ucode);
            }
            TraceKind::EventMesg {
                event,
                queue,
                thread,
            } => {
                out.push(4);
                push_u32(&mut out, event);
                push_u32(&mut out, queue.offset());
                push_u32(&mut out, thread);
            }
        }
    }
    out
}

pub(super) fn encode_device_dma_trace(events: &[DeviceTraceEvent]) -> Vec<u8> {
    let dma_count = events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                DeviceTraceKind::PiDmaStarted(_)
                    | DeviceTraceKind::PiBytesCommitted(_)
                    | DeviceTraceKind::AiDmaStarted(_)
                    | DeviceTraceKind::AiDmaComplete(_)
                    | DeviceTraceKind::SiDmaStarted(_)
                    | DeviceTraceKind::SiBytesCommitted(_)
                    | DeviceTraceKind::SpDmaStarted(_)
                    | DeviceTraceKind::SpDmaQueued(_)
                    | DeviceTraceKind::SpDmaBytesCommitted(_)
                    | DeviceTraceKind::SpTaskAdmitted { .. }
            )
        })
        .count();
    let mut out = Vec::with_capacity(dma_count * 32);
    push_u64(&mut out, dma_count as u64);
    for event in events {
        let tag = match event.kind {
            DeviceTraceKind::PiDmaStarted(_) => 0,
            DeviceTraceKind::PiBytesCommitted(_) => 1,
            DeviceTraceKind::AiDmaStarted(_) => 2,
            DeviceTraceKind::AiDmaComplete(_) => 3,
            DeviceTraceKind::SiDmaStarted(_) => 4,
            DeviceTraceKind::SiBytesCommitted(_) => 5,
            DeviceTraceKind::SpDmaStarted(_) => 6,
            DeviceTraceKind::SpDmaQueued(_) => 7,
            DeviceTraceKind::SpDmaBytesCommitted(_) => 8,
            DeviceTraceKind::SpTaskAdmitted { .. } => 9,
            _ => continue,
        };
        push_u64(&mut out, event.at.get());
        out.push(tag);
        match event.kind {
            DeviceTraceKind::PiDmaStarted(request) | DeviceTraceKind::PiBytesCommitted(request) => {
                out.push(match request.direction {
                    DmaDirection::ToRdram => 0,
                    DmaDirection::FromRdram => 1,
                });
                push_u32(&mut out, request.dram_addr.offset());
                encode_pi_device_address(&mut out, request.device);
                push_u32(&mut out, request.len);
            }
            DeviceTraceKind::AiDmaStarted(request) | DeviceTraceKind::AiDmaComplete(request) => {
                push_u32(&mut out, request.dram_addr.offset());
                push_u32(&mut out, request.len);
                push_u32(&mut out, request.sample_rate_hz);
            }
            DeviceTraceKind::SiDmaStarted(request) | DeviceTraceKind::SiBytesCommitted(request) => {
                out.push(match request.kind {
                    SiDmaKind::DramToPif => 0,
                    SiDmaKind::PifToDram => 1,
                    SiDmaKind::ControllerQuery => 2,
                    SiDmaKind::ControllerRead => 3,
                });
                push_u32(&mut out, request.dram_addr.offset());
            }
            DeviceTraceKind::SpDmaStarted(request)
            | DeviceTraceKind::SpDmaQueued(request)
            | DeviceTraceKind::SpDmaBytesCommitted(request) => {
                out.push(match request.direction {
                    SpDmaDirection::RdramToRsp => 0,
                    SpDmaDirection::RspToRdram => 1,
                });
                push_u32(
                    &mut out,
                    u32::try_from(request.mem_addr.offset()).expect("RSP DMA offset fits u32"),
                );
                push_u32(&mut out, request.dram_addr.offset());
                push_u32(&mut out, request.encoded_len);
            }
            DeviceTraceKind::SpTaskAdmitted { task_addr, header } => {
                push_u32(&mut out, task_addr.offset());
                for value in [
                    header.task_type,
                    header.flags,
                    header.ucode_boot,
                    header.ucode_boot_size,
                    header.ucode,
                    header.ucode_size,
                    header.ucode_data,
                    header.ucode_data_size,
                    header.dram_stack,
                    header.dram_stack_size,
                    header.output_buff,
                    header.output_buff_size,
                    header.data_ptr,
                    header.data_size,
                    header.yield_data_ptr,
                    header.yield_data_size,
                ] {
                    push_u32(&mut out, value);
                }
            }
            _ => unreachable!("device DMA tag and request encoding diverged"),
        }
    }
    out
}

pub(super) fn push_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    push_u64(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

pub(super) fn encode_execution_destination(
    out: &mut Vec<u8>,
    destination: &ReleaseExecutionDestination,
) -> Result<(), GateError> {
    match destination {
        ReleaseExecutionDestination::Native {
            section_index,
            function_offset,
            link_vram,
        } => {
            out.push(0);
            push_u32(out, *section_index);
            push_u32(out, *function_offset);
            push_u32(out, *link_vram);
        }
        ReleaseExecutionDestination::TypedFunction { vram, symbol } => {
            out.push(1);
            push_u32(out, *vram);
            push_bytes(out, symbol.as_bytes());
        }
        ReleaseExecutionDestination::TypedBlock {
            bank,
            pc,
            runner_artifact_sha256,
        } => {
            out.push(2);
            push_u64(out, *bank);
            push_u32(out, *pc);
            out.extend_from_slice(&decode_sha256(runner_artifact_sha256).ok_or(
                GateError::InvalidReportSha256(
                    "execution_destinations.ordered[].runner_artifact_sha256",
                ),
            )?);
        }
    }
    Ok(())
}

pub(super) fn encode_ordered_execution_destinations(
    ordered: &[ExecutionDestinationEventEvidence],
) -> Result<Vec<u8>, GateError> {
    let mut out = Vec::new();
    out.extend_from_slice(b"fn64.execution-destinations.ordered.v2\0");
    push_u64(&mut out, ordered.len() as u64);
    for event in ordered {
        match event.guest_cycle {
            Some(cycle) => {
                out.push(1);
                push_u64(&mut out, cycle);
            }
            None => out.push(0),
        }
        encode_execution_destination(&mut out, &event.destination)?;
    }
    Ok(out)
}

pub(super) fn encode_unique_execution_destinations(
    unique: &[ExecutionDestinationCountEvidence],
) -> Result<Vec<u8>, GateError> {
    let mut out = Vec::new();
    out.extend_from_slice(b"fn64.execution-destinations.unique.v2\0");
    push_u64(&mut out, unique.len() as u64);
    for entry in unique {
        encode_execution_destination(&mut out, &entry.destination)?;
        push_u64(&mut out, entry.observations);
    }
    Ok(out)
}

pub(crate) fn encode_execution_destination_evidence(
    evidence: &ExecutionDestinationEvidence,
) -> Result<Vec<u8>, GateError> {
    let mut out = Vec::new();
    out.extend_from_slice(b"fn64.execution-destinations.evidence.v2\0");
    match &evidence.source {
        ExecutionDestinationSource::NoProgram => out.push(0),
        ExecutionDestinationSource::NativeArchive { artifact_sha256 } => {
            out.push(1);
            out.extend_from_slice(&decode_sha256(artifact_sha256).ok_or(
                GateError::InvalidReportSha256("execution_destinations.source.artifact_sha256"),
            )?);
        }
        ExecutionDestinationSource::TypedBlockProgram {
            program_sha256,
            dispatch_artifact_sha256,
        } => {
            out.push(3);
            out.extend_from_slice(&decode_sha256(program_sha256).ok_or(
                GateError::InvalidReportSha256("execution_destinations.source.program_sha256"),
            )?);
            out.extend_from_slice(&decode_sha256(dispatch_artifact_sha256).ok_or(
                GateError::InvalidReportSha256(
                    "execution_destinations.source.dispatch_artifact_sha256",
                ),
            )?);
        }
        ExecutionDestinationSource::TypedObservedFunctionProgram { artifact_sha256 } => {
            out.push(2);
            out.extend_from_slice(&decode_sha256(artifact_sha256).ok_or(
                GateError::InvalidReportSha256("execution_destinations.source.artifact_sha256"),
            )?);
        }
    }
    push_u64(&mut out, evidence.total_observations);
    push_u64(&mut out, evidence.unique_destinations);
    out.extend_from_slice(&decode_sha256(&evidence.ordered_sha256).ok_or(
        GateError::InvalidReportSha256("execution_destinations.ordered_sha256"),
    )?);
    out.extend_from_slice(&decode_sha256(&evidence.unique_sha256).ok_or(
        GateError::InvalidReportSha256("execution_destinations.unique_sha256"),
    )?);
    push_bytes(
        &mut out,
        &encode_ordered_execution_destinations(&evidence.ordered)?,
    );
    push_bytes(
        &mut out,
        &encode_unique_execution_destinations(&evidence.unique)?,
    );
    Ok(out)
}

pub(super) fn validate_rsp_rdp_observations(
    gate_cycle: u64,
    observations: &[RspRdpObservationEventEvidence],
) -> Result<(), GateError> {
    let mut previous_cycle = None;
    let mut previous_imem_generation = None;
    let mut imem_generation_digests = BTreeMap::<u64, &str>::new();
    for event in observations {
        if event.guest_cycle > gate_cycle {
            return Err(GateError::FutureRspRdpObservation {
                gate_cycle,
                event_cycle: event.guest_cycle,
            });
        }
        if previous_cycle.is_some_and(|previous| event.guest_cycle < previous) {
            return Err(GateError::NonMonotonicRspRdpObservationCycle {
                previous: previous_cycle.expect("checked RSP/RDP observation cycle"),
                observed: event.guest_cycle,
            });
        }
        previous_cycle = Some(event.guest_cycle);
        match &event.observation {
            RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address,
                imem_generation,
                text_sha256,
                data_address,
                data_bytes,
                data_sha256,
                ..
            } => {
                validate_rsp_task_observation_address(*task_address)?;
                decode_sha256(text_sha256).ok_or(GateError::InvalidReportSha256(
                    "rsp_rdp.ordered[].observation.text_sha256",
                ))?;
                decode_sha256(data_sha256).ok_or(GateError::InvalidReportSha256(
                    "rsp_rdp.ordered[].observation.data_sha256",
                ))?;
                validate_microcode_data_observation_range(*data_address, *data_bytes)?;
                validate_imem_generation_digest(
                    &mut imem_generation_digests,
                    *imem_generation,
                    text_sha256,
                )?;
                if previous_imem_generation.is_some_and(|previous| *imem_generation < previous) {
                    return Err(GateError::NonMonotonicImemGeneration {
                        previous: previous_imem_generation.expect("checked RSP IMEM generation"),
                        observed: *imem_generation,
                    });
                }
                previous_imem_generation = Some(*imem_generation);
            }
            RspRdpObservationKindEvidence::DramDpcCommitted {
                start,
                end,
                command_sha256,
            } => {
                validate_dpc_observation_range(
                    *start,
                    *end,
                    crate::DEFAULT_RDRAM_SIZE as u32,
                    "DRAM",
                )?;
                decode_sha256(command_sha256).ok_or(GateError::InvalidReportSha256(
                    "rsp_rdp.ordered[].observation.command_sha256",
                ))?;
            }
            RspRdpObservationKindEvidence::XbusDpcCommitted {
                start,
                end,
                command_sha256,
            } => {
                validate_dpc_observation_range(*start, *end, 0x1000, "XBUS")?;
                decode_sha256(command_sha256).ok_or(GateError::InvalidReportSha256(
                    "rsp_rdp.ordered[].observation.command_sha256",
                ))?;
            }
            RspRdpObservationKindEvidence::ImemReplacementCommitted {
                task_address,
                imem_generation,
                text_sha256,
            } => {
                validate_rsp_task_observation_address(*task_address)?;
                decode_sha256(text_sha256).ok_or(GateError::InvalidReportSha256(
                    "rsp_rdp.ordered[].observation.text_sha256",
                ))?;
                validate_imem_generation_digest(
                    &mut imem_generation_digests,
                    *imem_generation,
                    text_sha256,
                )?;
                if previous_imem_generation.is_some_and(|previous| *imem_generation <= previous) {
                    return Err(GateError::NonMonotonicImemReplacementGeneration {
                        previous: previous_imem_generation.expect("checked RSP IMEM generation"),
                        observed: *imem_generation,
                    });
                }
                previous_imem_generation = Some(*imem_generation);
            }
        }
    }
    Ok(())
}

pub(super) fn validate_imem_generation_digest<'a>(
    generations: &mut BTreeMap<u64, &'a str>,
    generation: u64,
    text_sha256: &'a str,
) -> Result<(), GateError> {
    if let Some(previous) = generations.insert(generation, text_sha256) {
        if previous != text_sha256 {
            return Err(GateError::ConflictingImemGenerationDigest {
                generation,
                previous: previous.to_owned(),
                observed: text_sha256.to_owned(),
            });
        }
    }
    Ok(())
}

pub(super) fn validate_dpc_observation_range(
    start: u32,
    end: u32,
    limit: u32,
    source: &'static str,
) -> Result<(), GateError> {
    if start >= end || !start.is_multiple_of(8) || !end.is_multiple_of(8) || end > limit {
        return Err(GateError::InvalidDpcObservationRange {
            source,
            start,
            end,
            limit,
        });
    }
    Ok(())
}

pub(super) fn validate_microcode_data_observation_range(start: u32, bytes: u32) -> Result<(), GateError> {
    let limit = u32::try_from(crate::DEFAULT_RDRAM_SIZE).expect("release RDRAM size fits u32");
    let valid =
        bytes != 0 && start < limit && start.checked_add(bytes).is_some_and(|end| end <= limit);
    if !valid {
        return Err(GateError::InvalidMicrocodeDataObservationRange {
            start,
            bytes,
            limit,
        });
    }
    Ok(())
}

pub(super) fn validate_rsp_task_observation_address(address: u32) -> Result<(), GateError> {
    const OS_TASK_HEADER_BYTES: u32 = 64;
    let limit = u32::try_from(crate::DEFAULT_RDRAM_SIZE).expect("release RDRAM size fits u32");
    if address
        .checked_add(OS_TASK_HEADER_BYTES)
        .is_none_or(|end| end > limit)
    {
        return Err(GateError::InvalidRspTaskObservationAddress { address, limit });
    }
    Ok(())
}

pub(crate) fn encode_rsp_rdp_observations(
    observations: &[RspRdpObservationEventEvidence],
) -> Result<Vec<u8>, GateError> {
    let mut out = Vec::new();
    out.extend_from_slice(b"fn64.rsp-rdp-observations.v2\0");
    push_u64(&mut out, observations.len() as u64);
    for event in observations {
        push_u64(&mut out, event.guest_cycle);
        match &event.observation {
            RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address,
                imem_generation,
                text_sha256,
                data_address,
                data_bytes,
                data_sha256,
                family,
            } => {
                out.push(0);
                push_u32(&mut out, *task_address);
                push_u64(&mut out, *imem_generation);
                out.extend_from_slice(&decode_sha256(text_sha256).ok_or(
                    GateError::InvalidReportSha256("rsp_rdp.ordered[].observation.text_sha256"),
                )?);
                push_u32(&mut out, *data_address);
                push_u32(&mut out, *data_bytes);
                out.extend_from_slice(&decode_sha256(data_sha256).ok_or(
                    GateError::InvalidReportSha256("rsp_rdp.ordered[].observation.data_sha256"),
                )?);
                match family {
                    Some(family) => {
                        out.push(1);
                        family.encode(&mut out);
                    }
                    None => out.push(0),
                }
            }
            RspRdpObservationKindEvidence::DramDpcCommitted {
                start,
                end,
                command_sha256,
            } => {
                out.push(1);
                push_u32(&mut out, *start);
                push_u32(&mut out, *end);
                out.extend_from_slice(&decode_sha256(command_sha256).ok_or(
                    GateError::InvalidReportSha256("rsp_rdp.ordered[].observation.command_sha256"),
                )?);
            }
            RspRdpObservationKindEvidence::XbusDpcCommitted {
                start,
                end,
                command_sha256,
            } => {
                out.push(2);
                push_u32(&mut out, *start);
                push_u32(&mut out, *end);
                out.extend_from_slice(&decode_sha256(command_sha256).ok_or(
                    GateError::InvalidReportSha256("rsp_rdp.ordered[].observation.command_sha256"),
                )?);
            }
            RspRdpObservationKindEvidence::ImemReplacementCommitted {
                task_address,
                imem_generation,
                text_sha256,
            } => {
                out.push(3);
                push_u32(&mut out, *task_address);
                push_u64(&mut out, *imem_generation);
                out.extend_from_slice(&decode_sha256(text_sha256).ok_or(
                    GateError::InvalidReportSha256("rsp_rdp.ordered[].observation.text_sha256"),
                )?);
            }
        }
    }
    Ok(out)
}

/// Encode a complete report without `report_sha256` itself. This is an
/// evidence wire format, so it does not depend on JSON key order or serializer
/// formatting.
pub(crate) fn encode_report_evidence(report: &ReleaseGateReport) -> Result<Vec<u8>, GateError> {
    let mut out = Vec::new();
    push_bytes(&mut out, report.schema.as_bytes());
    push_bytes(&mut out, report.scenario.as_bytes());
    out.extend_from_slice(
        &decode_sha256(&report.input_sha256)
            .ok_or(GateError::InvalidReportSha256("input_sha256"))?,
    );
    report.unsupported_instrumentation.verify_current()?;
    push_bytes(
        &mut out,
        report.unsupported_instrumentation.schema.as_bytes(),
    );
    out.extend_from_slice(
        &decode_sha256(&report.unsupported_instrumentation.sha256).ok_or(
            GateError::InvalidReportSha256("unsupported_instrumentation.sha256"),
        )?,
    );
    match &report.rom {
        Some(rom) => {
            rom.verify_integrity()?;
            out.push(1);
            out.push(rom.class.tag());
            out.push(rom.source_byte_order.tag());
            push_u64(&mut out, rom.byte_len);
            out.extend_from_slice(
                &decode_sha256(&rom.canonical_sha256)
                    .ok_or(GateError::InvalidReportSha256("rom.canonical_sha256"))?,
            );
            out.push(rom.destination_code);
            out.push(rom.decoded_tv_region.tag());
            out.push(rom.configured_tv_type.tag());
        }
        None => out.push(0),
    }
    push_u64(&mut out, report.digest.guest_cycle);
    push_u64(&mut out, report.digest.artifacts.len() as u64);
    for artifact in &report.digest.artifacts {
        push_bytes(&mut out, artifact.kind.tag());
        push_u64(&mut out, artifact.bytes);
        out.extend_from_slice(
            &decode_sha256(&artifact.sha256)
                .ok_or(GateError::InvalidReportSha256("digest.artifacts[].sha256"))?,
        );
    }
    out.extend_from_slice(
        &decode_sha256(&report.digest.root_sha256)
            .ok_or(GateError::InvalidReportSha256("digest.root_sha256"))?,
    );
    let framebuffer = &report.observations.framebuffer;
    match &framebuffer.source {
        FramebufferObservationSource::PhysicalRdram { address } => {
            out.push(0);
            push_u32(&mut out, *address);
        }
        FramebufferObservationSource::PostViSwapchain {
            backend_identity,
            settings_sha256,
            workload_id,
            present_id,
        } => {
            out.push(1);
            push_bytes(&mut out, backend_identity.as_bytes());
            out.extend_from_slice(&decode_sha256(settings_sha256).ok_or(
                GateError::InvalidReportSha256("observations.framebuffer.source.settings_sha256"),
            )?);
            push_u64(&mut out, workload_id.get());
            push_u64(&mut out, *present_id);
        }
    }
    push_u32(&mut out, framebuffer.width);
    push_u32(&mut out, framebuffer.height);
    push_u32(&mut out, framebuffer.row_bytes);
    out.push(framebuffer.format.tag());
    push_u64(&mut out, framebuffer.payload_bytes);
    push_u32(&mut out, report.observations.memory.physical_address);
    push_u64(&mut out, report.observations.memory.payload_bytes);
    out.push(match report.environment.platform {
        ReleaseHostPlatform::MacosArm64 => 0,
        ReleaseHostPlatform::LinuxX86_64 => 1,
        ReleaseHostPlatform::WindowsX86_64 => 2,
    });
    match report.environment.windows_version {
        None => out.push(0),
        Some(version) => {
            out.push(1);
            out.push(match version.family {
                ReleaseWindowsFamily::Windows10 => 0,
                ReleaseWindowsFamily::Windows11 => 1,
            });
            push_u32(&mut out, version.major);
            push_u32(&mut out, version.minor);
            push_u32(&mut out, version.build);
            push_u32(&mut out, version.update_build_revision);
            out.push(match version.product_type {
                ReleaseWindowsProductType::Workstation => 0,
            });
        }
    }
    for port in report.environment.controller_ports {
        out.push(match port {
            ReleaseControllerPort::StandardControllerNoPak => 0,
            ReleaseControllerPort::StandardControllerControllerPak => 1,
            ReleaseControllerPort::StandardControllerRumblePak => 2,
            ReleaseControllerPort::StandardControllerTransferPak => 3,
            ReleaseControllerPort::VoiceRecognitionUnit => 4,
            ReleaseControllerPort::Absent => 5,
        });
    }
    out.push(match report.environment.cartridge_save {
        ReleaseCartridgeSave::NoCartridgeSave => 0,
        ReleaseCartridgeSave::Eeprom4k => 1,
        ReleaseCartridgeSave::Eeprom16k => 2,
        ReleaseCartridgeSave::Sram32Kib => 3,
        ReleaseCartridgeSave::FlashRam128Kib => 4,
    });
    match &report.environment.audio_task_execution {
        ReleaseAudioTaskExecutionPolicy::Unconfigured => out.push(0),
        ReleaseAudioTaskExecutionPolicy::Translated { artifact_sha256 } => {
            out.push(1);
            out.extend_from_slice(&decode_sha256(artifact_sha256).ok_or(
                GateError::InvalidReportSha256("environment.audio_task_execution.artifact_sha256"),
            )?);
        }
        ReleaseAudioTaskExecutionPolicy::LleAccuracy => out.push(2),
        ReleaseAudioTaskExecutionPolicy::DiagnosticSkip => out.push(3),
    }
    match &report.environment.renderer {
        ReleaseRendererEvidence::Reference {
            execution_policy, ..
        } => {
            out.push(0);
            out.push(encode_graphics_execution_policy(*execution_policy));
            out.push(report.environment.renderer.tv_type().tag());
        }
        ReleaseRendererEvidence::Rt64 {
            execution_policy,
            graphics_api,
            backend_identity,
            source_authoritative,
            settings_sha256,
            replacement_packs_active,
            ..
        } => {
            out.push(1);
            out.push(encode_graphics_execution_policy(*execution_policy));
            out.push(report.environment.renderer.tv_type().tag());
            out.push(encode_graphics_api(*graphics_api));
            push_bytes(&mut out, backend_identity.as_bytes());
            out.push(*source_authoritative as u8);
            out.extend_from_slice(&decode_sha256(settings_sha256).ok_or(
                GateError::InvalidReportSha256("environment.renderer.settings_sha256"),
            )?);
            out.push(*replacement_packs_active as u8);
        }
    }
    push_bytes(
        &mut out,
        &encode_execution_destination_evidence(&report.execution_destinations)?,
    );
    push_bytes(
        &mut out,
        &encode_rsp_rdp_observations(&report.rsp_rdp.ordered)?,
    );
    push_u64(&mut out, report.rsp_rdp.total_observations);
    out.extend_from_slice(
        &decode_sha256(&report.rsp_rdp.ordered_sha256)
            .ok_or(GateError::InvalidReportSha256("rsp_rdp.ordered_sha256"))?,
    );
    push_u64(&mut out, report.closure.len() as u64);
    for path in &report.closure {
        push_bytes(&mut out, path.name.as_bytes());
        push_u64(&mut out, path.observations);
        out.push(match path.status {
            ClosurePathStatus::Unexercised => 0,
            ClosurePathStatus::ExercisedZeroUnsupported => 1,
            ClosurePathStatus::ExercisedUnsupported => 2,
        });
        push_u64(&mut out, path.unsupported.len() as u64);
        for event in &path.unsupported {
            push_bytes(&mut out, event.subsystem.as_bytes());
            push_bytes(&mut out, event.operation.as_bytes());
            push_bytes(&mut out, event.context.as_bytes());
            match event.guest_cycle {
                Some(cycle) => {
                    out.push(1);
                    push_u64(&mut out, cycle);
                }
                None => out.push(0),
            }
            push_bytes(&mut out, event.disposition.as_bytes());
        }
    }
    Ok(out)
}

pub(super) const fn encode_graphics_execution_policy(policy: ReleaseGraphicsExecutionPolicy) -> u8 {
    match policy {
        ReleaseGraphicsExecutionPolicy::HleOptimized => 0,
        ReleaseGraphicsExecutionPolicy::LleAccuracy => 1,
    }
}

pub(super) const fn encode_graphics_api(api: ReleaseGraphicsApi) -> u8 {
    match api {
        ReleaseGraphicsApi::D3d12 => 0,
        ReleaseGraphicsApi::Vulkan => 1,
        ReleaseGraphicsApi::Metal => 2,
    }
}
