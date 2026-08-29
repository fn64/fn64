use super::*;

pub(super) fn run_block_program(
    live: &LiveBlockProgram,
    mut entry: ExecutionKey,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) {
    loop {
        // Exact timer interleaving: checkpoint suspension -> executor advances
        // Count and latches Compare/IP7 -> coroutine resume -> this sample ->
        // exception entry before the resumed guest block. Sampling after
        // dispatch would allow that block to run once with an overdue timer.
        let (count, count_phase, compare, timer_pending) = with_executor(|executor| {
            (
                executor.cp0_count(),
                executor.cp0_count_phase(),
                executor.cp0_compare(),
                executor.cp0_timer_pending(),
            )
        });
        ctx.synchronize_cop0_timing(count, count_phase, compare);
        CpuInterruptLine::TIMER.set_level(ctx, timer_pending);
        CpuInterruptLine::RCP.set_level(ctx, crate::pi::cpu_interrupt_pending());
        if let Some(vector) = enter_pending_interrupt(ctx, entry.pc) {
            entry = live.resolve_transfer(entry.bank, vector).unwrap_or_else(|fault| {
                panic!(
                    "live BlockProgram interrupt vector {vector} from {entry} does not resolve: {fault:?}"
                )
            });
        }
        let mut resolver = LiveTransferResolver { live: live.clone() };
        let dispatched = {
            let program = live.program.borrow();
            program
                .dispatch_exposing_exceptions(entry, live.budget, ctx, mem, &mut resolver)
                .unwrap_or_else(|error| {
                    recompiled_gap_panic(format!(
                        "live BlockProgram dispatch failed at {entry}: {error}"
                    ))
                })
        };
        process_executable_writes(live, |offset| {
            mem.load_b(0xFFFF_FFFF_8000_0000u64 + u64::from(offset)) as u8
        });
        let image_changed_entry = match dispatched.exit {
            BlockExit::ImageChanged { at, miss } => Some(
                activate_fetch_generation(live, at, miss, |offset| {
                    mem.load_b(0xFFFF_FFFF_8000_0000u64 + u64::from(offset)) as u8
                })
                .unwrap_or_else(|error| recompiled_gap_panic(error)),
            ),
            _ => None,
        };
        let unresolved_generation_entry = match dispatched.exit {
            BlockExit::Fault(CpuFault {
                at,
                kind:
                    fn64_cpu_runtime::CpuFaultKind::UnknownBank
                    | fn64_cpu_runtime::CpuFaultKind::UnmappedPc { .. }
                    | fn64_cpu_runtime::CpuFaultKind::UnmappedPhysicalInstruction { .. },
            }) => live
                .precompiled_generations
                .borrow_mut()
                .as_mut()
                .and_then(|catalog| {
                    match catalog.activate_for_fetch_with(at.pc, |vaddr| {
                        mem.load_bu(0xffff_ffff_0000_0000u64 | u64::from(vaddr))
                    }) {
                        Ok(resolution) => Some(resolution.entry),
                        Err(GenerationLookupError::UnmappedPc { .. }) => None,
                        Err(error) => recompiled_gap_panic(format!(
                            "closed AOT pack could not activate attempted fetch at {}: {error}",
                            at.pc
                        )),
                    }
                }),
            _ => None,
        };
        let executable_write_fault_entry = match dispatched.exit {
            BlockExit::ExecutableWriteFault(fault) => {
                assert!(
                    dispatched.instructions > 0,
                    "live BlockProgram returned {:?} without guest progress",
                    dispatched.exit
                );
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "live BlockProgram executable-write boundary retained a non-architectural fault: {fault:?}"
                    ))
                });
                Some(
                    live.resolve_transfer(fault.at.bank, vector)
                        .unwrap_or_else(|mapping_fault| {
                            recompiled_gap_panic(format!(
                                "live BlockProgram executable-write exception vector {vector} does not resolve after generation replacement: {mapping_fault:?}"
                            ))
                        }),
                )
            }
            _ => None,
        };
        let (count_write, compare_write) = ctx.take_cop0_timing_writes();
        if count_write.is_some() || compare_write.is_some() {
            // Commit a handler's same-value Compare write before suspending:
            // otherwise checkpoint time could advance while the executor's
            // IP7 latch remained set, causing an acknowledged interrupt to
            // re-enter immediately after ERET.
            with_executor(|executor| {
                if let Some(count) = count_write {
                    executor.set_cp0_count(count);
                }
                if let Some(compare) = compare_write {
                    executor.write_cp0_compare(compare);
                }
            });
        }
        if dispatched.instructions > 0 {
            crate::suspend_active_coroutine(fn64_runtime::Yield::InstructionCheckpoint {
                instructions: dispatched.instructions,
            });
        }
        match dispatched.exit {
            BlockExit::Checkpoint(next) | BlockExit::Yield(next) => {
                assert!(
                    dispatched.instructions > 0,
                    "live BlockProgram returned {:?} without guest progress",
                    dispatched.exit
                );
                entry = live
                    .resolve_transfer(next.bank, next.pc)
                    .unwrap_or_else(|fault| {
                        recompiled_gap_panic(format!(
                            "live BlockProgram checkpoint {next} no longer resolves: {fault:?}"
                        ))
                    });
            }
            BlockExit::ExecutableWrite {
                source_bank,
                resume,
            } => {
                assert!(
                    dispatched.instructions > 0,
                    "live BlockProgram returned {:?} without guest progress",
                    dispatched.exit
                );
                entry = live
                    .resolve_transfer(source_bank, resume.pc)
                    .unwrap_or_else(|fault| {
                        recompiled_gap_panic(format!(
                            "live BlockProgram executable-write resume {resume} no longer resolves after generation replacement: {fault:?}"
                        ))
                    });
            }
            BlockExit::ExecutableWriteResolveCall {
                source_bank,
                target_pc,
                resume,
            } => {
                assert!(
                    dispatched.instructions > 0,
                    "live BlockProgram returned {:?} without guest progress",
                    dispatched.exit
                );
                let mut resolver = LiveTransferResolver { live: live.clone() };
                match resolver.resolve_call(source_bank, target_pc, resume) {
                    Ok(CallResolution::Guest(next)) => entry = next,
                    Ok(CallResolution::Host) => {
                        let host = fn64_cpu_runtime::resolve_host_function(target_pc.get())
                            .unwrap_or_else(|| {
                                recompiled_gap_panic(format!(
                                    "live BlockProgram requested unknown host call {:#010x}",
                                    target_pc.get()
                                ))
                            });
                        invoke_observed_block_host(target_pc, resume, host, ctx, mem);
                        entry = live
                            .resolve_transfer(source_bank, resume.pc)
                            .unwrap_or_else(|fault| {
                                recompiled_gap_panic(format!(
                                    "live BlockProgram executable-write host resume {resume} no longer resolves after generation replacement: {fault:?}"
                                ))
                            });
                    }
                    Err(fault) => recompiled_gap_panic(format!(
                        "live BlockProgram executable-write call target {target_pc} does not resolve after generation replacement: {fault:?}"
                    )),
                }
            }
            BlockExit::ExecutableWriteFault(_) => {
                entry = executable_write_fault_entry.unwrap_or_else(|| {
                    unreachable!(
                        "executable-write fault continuation was not prepared before suspension"
                    )
                });
            }
            BlockExit::ImageChanged { .. } => {
                entry = image_changed_entry.unwrap_or_else(|| {
                    unreachable!("image-change continuation was not prepared before suspension")
                });
            }
            BlockExit::HostCall { vram, resume } => {
                let host = fn64_cpu_runtime::resolve_host_function(vram.get()).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "live BlockProgram requested unknown host call {:#010x}",
                        vram.get()
                    ))
                });
                invoke_observed_block_host(vram, resume, host, ctx, mem);
                entry = live
                    .resolve_transfer(resume.bank, resume.pc)
                    .unwrap_or_else(|fault| {
                        recompiled_gap_panic(format!(
                            "live BlockProgram host resume {resume} no longer resolves: {fault:?}"
                        ))
                    });
            }
            BlockExit::ThreadReturn => return,
            BlockExit::Fault(_) if unresolved_generation_entry.is_some() => {
                entry = unresolved_generation_entry
                    .expect("attempted-fetch generation was checked above");
            }
            BlockExit::Fault(fault) => {
                assert!(
                    dispatched.instructions > 0,
                    "live BlockProgram returned {:?} without guest progress",
                    dispatched.exit
                );
                if park_host_scheduled_exception(None, fault, ctx) {
                    unreachable!("parking a faulted host-scheduled thread does not return")
                }
                // Architectural exceptions (mid-function BREAK/SYSCALL and the
                // conditional traps, which the block emitter renders as
                // `BlockExit::Fault { kind: Exception }`) are vectored through
                // the installed handler exactly like the executable-write
                // boundary above: `enter_exception` commits EPC/EXL/Cause.BD
                // and returns the BEV-selected vector, then the handler bank is
                // resolved as an ordinary transfer. Only a genuinely
                // non-architectural fault (a real lane gap) stays loud.
                let fault_bank = fault.at.bank;
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    let destinations = live.program.borrow().copy_execution_destinations();
                    let recent_start = destinations.len().saturating_sub(16);
                    // Read the Copy CP0 fields before taking `ctx`'s mutable
                    // borrow for `indirect_transfer_observations()` below --
                    // that borrow (needed to make the ring buffer's tail
                    // contiguous) would otherwise conflict with reading them
                    // inside the same `format!`.
                    let cop0_status = ctx.cop0_status;
                    let cop0_cause = ctx.cop0_cause;
                    let cop0_epc = ctx.cop0_epc;
                    let cop0_badvaddr = ctx.cop0_badvaddr;
                    let indirect = ctx.indirect_transfer_observations();
                    let indirect_start = indirect.len().saturating_sub(8);
                    recompiled_gap_panic(format!(
                        "live BlockProgram stopped on non-architectural guest fault: {fault:?}; current CP0 status={cop0_status:#010x} cause={cop0_cause:#010x} epc={cop0_epc:#010x} badvaddr={cop0_badvaddr:#018x}; recent entered destinations={:?}; recent indirect transfers={:?}",
                        &destinations[recent_start..],
                        &indirect[indirect_start..],
                    ))
                });
                entry = live.resolve_transfer(fault_bank, vector).unwrap_or_else(|mapping_fault| {
                    recompiled_gap_panic(format!(
                        "live BlockProgram exception vector {vector} does not resolve: {mapping_fault:?}"
                    ))
                });
            }
            BlockExit::Transfer(_)
            | BlockExit::ResolveTransfer { .. }
            | BlockExit::ResolveCall { .. } => {
                unreachable!("BlockProgram::dispatch returned an internal transfer boundary")
            }
        }
    }
}

fn resolve_catalog_transfer_with_activation(
    live: &CanonicalLiveBlockProgramV1,
    source_bank: BankId,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<ExecutionKey, String> {
    match live.resolve_transfer(source_bank, target_pc) {
        Ok(entry) => Ok(entry),
        Err(CpuFault {
            kind: CpuFaultKind::NoActiveGeneration,
            ..
        }) => {
            // TEMPORARY (generation-activation census, 2026-08-07). The
            // observer is a thread-local, so it must be installed on the
            // executor thread that actually activates. `install` is
            // `Once`-guarded and returns immediately when the census env var
            // is absent, which is every non-diagnostic run.
            super::snapshots::activation_census::install();
            live.activate_for_fetch(target_pc, mem)
                .map_err(|error| format!("generation activation at {target_pc} failed: {error}"))?;
            live.resolve_transfer(source_bank, target_pc)
                .map_err(|fault| format!("activated target {target_pc} did not resolve: {fault}"))
        }
        Err(fault) => Err(format!("target {target_pc} does not resolve: {fault}")),
    }
}

fn resolve_catalog_entry_with_activation(
    live: &CanonicalLiveBlockProgramV1,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<ExecutionKey, String> {
    match live.resolve_entry(target_pc) {
        Ok(entry) => Ok(entry),
        Err(CpuFault {
            kind: CpuFaultKind::NoActiveGeneration,
            ..
        }) => {
            live.activate_for_fetch(target_pc, mem)
                .map_err(|error| format!("generation activation at {target_pc} failed: {error}"))?;
            live.resolve_entry(target_pc)
                .map_err(|fault| format!("activated entry {target_pc} did not resolve: {fault}"))
        }
        Err(fault) => Err(format!("entry {target_pc} does not resolve: {fault}")),
    }
}

fn resolve_catalog_call_with_activation(
    live: &CanonicalLiveBlockProgramV1,
    source_bank: BankId,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<CatalogCallResolutionV1, String> {
    match live.resolve_call(source_bank, target_pc) {
        Ok(resolution) => Ok(resolution),
        Err(CpuFault {
            kind: CpuFaultKind::NoActiveGeneration,
            ..
        }) => {
            live.activate_for_fetch(target_pc, mem).map_err(|error| {
                format!("call generation activation at {target_pc} failed: {error}")
            })?;
            live.resolve_call(source_bank, target_pc).map_err(|fault| {
                format!("activated call target {target_pc} did not resolve: {fault}")
            })
        }
        Err(fault) => Err(format!("call target {target_pc} does not resolve: {fault}")),
    }
}

#[cfg(feature = "dynamic-mapped-runtime")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UnifiedCatalogTargetV1 {
    Static(ExecutionKey),
    Dynamic {
        source_bank: BankId,
        target_pc: GuestPc,
    },
}

#[cfg(feature = "dynamic-mapped-runtime")]
impl UnifiedCatalogTargetV1 {
    const fn key(self) -> ExecutionKey {
        match self {
            Self::Static(entry) => entry,
            Self::Dynamic {
                source_bank,
                target_pc,
            } => ExecutionKey::new(source_bank, target_pc),
        }
    }
}

#[cfg(feature = "dynamic-mapped-runtime")]
fn dynamic_fallback_eligible(fault: CpuFault) -> bool {
    matches!(
        fault.kind,
        CpuFaultKind::UnknownBank
            | CpuFaultKind::UnmappedPc { .. }
            | CpuFaultKind::UnmappedPhysicalInstruction { .. }
            | CpuFaultKind::StaleInstructionIdentity { .. }
            | CpuFaultKind::MissingAotEntry
    )
}

#[cfg(feature = "dynamic-mapped-runtime")]
pub(super) fn resolve_unified_catalog_target(
    live: &CanonicalLiveBlockProgramV1,
    source_bank: BankId,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<UnifiedCatalogTargetV1, String> {
    match live.resolve_transfer(source_bank, target_pc) {
        Ok(entry) => Ok(UnifiedCatalogTargetV1::Static(entry)),
        Err(CpuFault {
            kind: CpuFaultKind::NoActiveGeneration,
            ..
        }) => match live.activate_for_fetch(target_pc, mem) {
            Ok(entry) => Ok(UnifiedCatalogTargetV1::Static(entry)),
            Err(GenerationLookupError::AotMiss(_) | GenerationLookupError::UnmappedPc { .. }) => {
                Ok(UnifiedCatalogTargetV1::Dynamic {
                    source_bank,
                    target_pc,
                })
            }
            Err(error @ GenerationLookupError::AmbiguousLiveImage { .. }) => Err(format!(
                "generation activation at {target_pc} is ambiguous: {error}"
            )),
            Err(error) => Err(format!(
                "generation activation at {target_pc} did not produce an executable owner: {error}"
            )),
        },
        Err(fault) if dynamic_fallback_eligible(fault) => Ok(UnifiedCatalogTargetV1::Dynamic {
            source_bank,
            target_pc,
        }),
        Err(fault) => Err(format!("target {target_pc} does not resolve: {fault}")),
    }
}

#[cfg(feature = "dynamic-mapped-runtime")]
pub(super) fn resolve_unified_catalog_entry(
    live: &CanonicalLiveBlockProgramV1,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<UnifiedCatalogTargetV1, String> {
    match live.resolve_entry(target_pc) {
        Ok(entry) => Ok(UnifiedCatalogTargetV1::Static(entry)),
        Err(
            fault @ CpuFault {
                kind: CpuFaultKind::NoActiveGeneration,
                ..
            },
        ) => match live.activate_for_fetch(target_pc, mem) {
            Ok(entry) => Ok(UnifiedCatalogTargetV1::Static(entry)),
            Err(GenerationLookupError::AotMiss(_) | GenerationLookupError::UnmappedPc { .. }) => {
                Ok(UnifiedCatalogTargetV1::Dynamic {
                    source_bank: fault.at.bank,
                    target_pc,
                })
            }
            Err(error @ GenerationLookupError::AmbiguousLiveImage { .. }) => Err(format!(
                "entry generation activation at {target_pc} is ambiguous: {error}"
            )),
            Err(error) => Err(format!(
                "entry generation activation at {target_pc} did not produce an executable owner: {error}"
            )),
        },
        Err(fault) if dynamic_fallback_eligible(fault) => Ok(UnifiedCatalogTargetV1::Dynamic {
            source_bank: fault.at.bank,
            target_pc,
        }),
        Err(fault) => Err(format!("entry {target_pc} does not resolve: {fault}")),
    }
}

#[cfg(feature = "dynamic-mapped-runtime")]
enum UnifiedCatalogCallV1 {
    Host,
    Guest(UnifiedCatalogTargetV1),
}

#[cfg(feature = "dynamic-mapped-runtime")]
fn resolve_unified_catalog_call(
    live: &CanonicalLiveBlockProgramV1,
    source_bank: BankId,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<UnifiedCatalogCallV1, String> {
    if let Some(host) = live.install.resolve_host(target_pc.get()) {
        let _ = host;
        return Ok(UnifiedCatalogCallV1::Host);
    }
    resolve_unified_catalog_target(live, source_bank, target_pc, mem)
        .map(UnifiedCatalogCallV1::Guest)
}

#[cfg(feature = "dynamic-mapped-runtime")]
fn checked_add_unified_work(
    instructions: &mut u32,
    blocks: &mut u32,
    added_instructions: u32,
    added_blocks: u32,
) -> Result<(), String> {
    *instructions = instructions
        .checked_add(added_instructions)
        .ok_or_else(|| "unified catalog instruction count overflow".to_string())?;
    *blocks = blocks
        .checked_add(added_blocks)
        .ok_or_else(|| "unified catalog block count overflow".to_string())?;
    Ok(())
}

#[cfg(feature = "dynamic-mapped-runtime")]
pub(super) fn dispatch_unified_catalog_slice(
    live: &CanonicalLiveBlockProgramV1,
    mut target: UnifiedCatalogTargetV1,
    budget: InstructionBudget,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) -> Result<fn64_cpu_runtime::DispatchRun, String> {
    let mut instructions = 0u32;
    let mut blocks = 0u32;
    let census = dispatch_census::enabled();
    macro_rules! finish_slice {
        ($run:expr) => {{
            let run = $run;
            if census {
                dispatch_census::record_slice(&run);
            }
            return Ok(run);
        }};
    }

    loop {
        if let UnifiedCatalogTargetV1::Static(entry) = target {
            if live.dynamic_withheld_static_key.get() == Some(entry) {
                target = UnifiedCatalogTargetV1::Dynamic {
                    source_bank: entry.bank,
                    target_pc: entry.pc,
                };
            }
        }
        let remaining = budget
            .get()
            .checked_sub(instructions)
            .ok_or_else(|| "unified catalog consumed more than its slice budget".to_string())?;
        if remaining < InstructionBudget::MIN {
            finish_slice!(fn64_cpu_runtime::DispatchRun {
                exit: BlockExit::Checkpoint(target.key()),
                instructions,
                blocks,
            });
        }
        let turn_budget = InstructionBudget::new(remaining)
            .expect("unified catalog remaining budget was checked");
        let was_dynamic = matches!(target, UnifiedCatalogTargetV1::Dynamic { .. });
        let run = match target {
            UnifiedCatalogTargetV1::Static(entry) => {
                let dispatched = live
                    .dispatch_exposing_exceptions_at_budget(entry, turn_budget, ctx, mem)
                    .map_err(|error| {
                        format!("static catalog dispatch failed at {entry}: {error}")
                    })?;
                checked_add_unified_work(
                    &mut instructions,
                    &mut blocks,
                    dispatched.instructions,
                    dispatched.blocks,
                )?;
                fn64_cpu_runtime::BlockRun::new(dispatched.exit, dispatched.instructions)
            }
            UnifiedCatalogTargetV1::Dynamic {
                source_bank,
                target_pc,
            } => {
                let attempted = ExecutionKey::new(source_bank, target_pc);
                let result = {
                    // The exact-unit catalog mutates only its identity map.
                    // Its RefMut may span this one non-suspending interpreter
                    // turn because guest-write/MMIO observers cannot re-enter
                    // catalog dispatch; it is dropped before reconciliation,
                    // host calls, or coroutine suspension.
                    let mut dynamic = live.dynamic_units.borrow_mut();
                    let catalog = dynamic.as_mut().expect(
                        "unified dynamic target exists without an installed dynamic catalog",
                    );
                    catalog.activate_and_run(attempted, turn_budget, ctx, mem, |bank| {
                        live.reserves_bank(bank)
                    })
                };
                match result {
                    Ok(dynamic) => {
                        if dynamic.run.instructions > remaining {
                            return Err(format!(
                                "dynamic mapped unit at {attempted} executed {} instructions with budget {remaining}",
                                dynamic.run.instructions
                            ));
                        }
                        if dynamic.run.instructions == 0
                            && matches!(
                                dynamic.run.exit,
                                BlockExit::Checkpoint(at) if at == dynamic.entry
                            )
                            && !turn_budget
                                .can_fit(0, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS)
                        {
                            if instructions > 0 {
                                finish_slice!(fn64_cpu_runtime::DispatchRun {
                                    exit: BlockExit::Checkpoint(attempted),
                                    instructions,
                                    blocks,
                                });
                            }
                            return Err(
                                fn64_cpu_runtime::DispatchError::IndivisibleUnitExceedsBudget {
                                    at: dynamic.entry,
                                    budget: turn_budget,
                                    required: InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS,
                                }
                                .to_string(),
                            );
                        }
                        live.record_dynamic_execution(attempted, &dynamic);
                        if dynamic.run.instructions > 0
                            && live.dynamic_withheld_static_key.get() == Some(attempted)
                        {
                            live.dynamic_withheld_static_key.set(None);
                        }
                        checked_add_unified_work(
                            &mut instructions,
                            &mut blocks,
                            dynamic.run.instructions,
                            1,
                        )?;
                        dynamic.run
                    }
                    Err(fn64_cpu_runtime::DynamicMappedErrorV1::Fetch {
                        fault,
                        attempted_instructions,
                    }) => {
                        if attempted_instructions > remaining {
                            return Err(format!(
                                "dynamic fetch at {attempted} charged {attempted_instructions} instructions with budget {remaining}"
                            ));
                        }
                        checked_add_unified_work(
                            &mut instructions,
                            &mut blocks,
                            attempted_instructions,
                            0,
                        )?;
                        finish_slice!(fn64_cpu_runtime::DispatchRun {
                            exit: BlockExit::Fault(fault),
                            instructions,
                            blocks,
                        });
                    }
                    Err(error) => {
                        return Err(format!(
                            "dynamic mapped activation at {attempted} failed: {error}"
                        ));
                    }
                }
            }
        };

        live.invalidate_pending_physical_writes(mem);
        live.reconcile_before_dispatch(mem);

        if run.instructions == 0
            && matches!(
                run.exit,
                BlockExit::Transfer(_)
                    | BlockExit::ResolveTransfer { .. }
                    | BlockExit::ResolveCall { .. }
                    | BlockExit::ExecutableWrite { .. }
                    | BlockExit::ExecutableWriteResolveCall { .. }
                    | BlockExit::ExecutableWriteFault(_)
            )
        {
            return Err(format!(
                "unified catalog continuing exit made no progress at {}: {:?}",
                target.key(),
                run.exit
            ));
        }

        match run.exit {
            BlockExit::Transfer(next) => {
                target = resolve_unified_catalog_target(live, next.bank, next.pc, mem)?;
            }
            BlockExit::ResolveTransfer {
                source_bank,
                target_pc,
            } => {
                target = resolve_unified_catalog_target(live, source_bank, target_pc, mem)?;
            }
            BlockExit::ResolveCall {
                source_bank,
                target_pc,
                resume,
            } => match resolve_unified_catalog_call(live, source_bank, target_pc, mem)? {
                UnifiedCatalogCallV1::Host => {
                    finish_slice!(fn64_cpu_runtime::DispatchRun {
                        exit: BlockExit::HostCall {
                            vram: target_pc,
                            resume,
                        },
                        instructions,
                        blocks,
                    });
                }
                UnifiedCatalogCallV1::Guest(next) => target = next,
            },
            exit @ (BlockExit::ExecutableWrite { .. }
            | BlockExit::ExecutableWriteResolveCall { .. }) => {
                // Interleaving closed: the writer mutates executable bytes,
                // publishes and suspends, another runnable guest thread may
                // run, then the writer resumes and resolves the new image.
                // Resolving here would collapse both sides of that scheduler
                // boundary into one unified slice.
                finish_slice!(fn64_cpu_runtime::DispatchRun {
                    exit,
                    instructions,
                    blocks,
                });
            }
            BlockExit::ImageChanged { at, .. }
            | BlockExit::Fault(CpuFault {
                at,
                kind: CpuFaultKind::NoActiveGeneration,
            }) => {
                target = resolve_unified_catalog_target(live, at.bank, at.pc, mem)?;
            }
            BlockExit::Fault(fault) if !was_dynamic && dynamic_fallback_eligible(fault) => {
                target = UnifiedCatalogTargetV1::Dynamic {
                    source_bank: fault.at.bank,
                    target_pc: fault.at.pc,
                };
            }
            exit => {
                finish_slice!(fn64_cpu_runtime::DispatchRun {
                    exit,
                    instructions,
                    blocks,
                });
            }
        }
    }
}

/// Diagnostic-only census of catalog dispatch granularity. Gated on
/// `FN64_DISPATCH_CENSUS`; it observes nothing the runtime consumes and takes
/// no part in any certified evidence value.
pub(super) mod dispatch_census {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    pub(super) struct Census {
        /// Slices, keyed by the terminating exit's discriminant name.
        pub exits: BTreeMap<&'static str, u64>,
        /// Histogram of instructions retired per slice.
        pub slice_instructions: BTreeMap<u32, u64>,
        /// Histogram of inner turns (blocks) per slice.
        pub slice_blocks: BTreeMap<u32, u64>,
        /// Slice-terminating `(exit name, guest pc)` sites.
        pub sites: BTreeMap<(&'static str, u32), u64>,
        pub slices: u64,
        pub total_instructions: u64,
        pub total_blocks: u64,
    }

    thread_local! {
        pub(super) static CENSUS: RefCell<Census> = RefCell::new(Census::default());
    }

    /// Read the environment once. This is consulted at every dispatch, so an
    /// uncached `env::var_os` would allocate and scan the environment millions
    /// of times on a long route even with the census disabled.
    pub(super) fn enabled() -> bool {
        use std::sync::OnceLock;
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| std::env::var_os("FN64_DISPATCH_CENSUS").is_some())
    }

    pub(super) fn exit_name(exit: &fn64_cpu_runtime::BlockExit) -> &'static str {
        use fn64_cpu_runtime::BlockExit as E;
        match exit {
            E::Transfer(_) => "Transfer",
            E::ResolveTransfer { .. } => "ResolveTransfer",
            E::ResolveCall { .. } => "ResolveCall",
            E::HostCall { .. } => "HostCall",
            E::ExecutableWrite { .. } => "ExecutableWrite",
            E::ExecutableWriteResolveCall { .. } => "ExecutableWriteResolveCall",
            E::ExecutableWriteFault(_) => "ExecutableWriteFault",
            E::ImageChanged { .. } => "ImageChanged",
            E::Checkpoint(_) => "Checkpoint",
            E::Yield(_) => "Yield",
            E::ThreadReturn => "ThreadReturn",
            E::Fault(_) => "Fault",
        }
    }

    /// The guest PC a slice's terminating exit names, so a hot exit can be
    /// attributed to the code that produced it.
    fn exit_pc(exit: &fn64_cpu_runtime::BlockExit) -> u32 {
        use fn64_cpu_runtime::BlockExit as E;
        match exit {
            E::Transfer(key) | E::Checkpoint(key) | E::Yield(key) => key.pc.get(),
            E::ResolveTransfer { target_pc, .. }
            | E::ResolveCall { target_pc, .. }
            | E::ExecutableWriteResolveCall { target_pc, .. } => target_pc.get(),
            E::HostCall { vram, .. } => vram.get(),
            E::ExecutableWrite { resume, .. } => resume.pc.get(),
            E::ExecutableWriteFault(fault) | E::Fault(fault) => fault.at.pc.get(),
            E::ImageChanged { at, .. } => at.pc.get(),
            E::ThreadReturn => 0,
        }
    }

    /// Record one completed slice (one scheduler round trip).
    pub(super) fn record_slice(run: &fn64_cpu_runtime::DispatchRun) {
        CENSUS.with(|census| {
            let mut census = census.borrow_mut();
            census.slices += 1;
            census.total_instructions += u64::from(run.instructions);
            census.total_blocks += u64::from(run.blocks);
            *census.exits.entry(exit_name(&run.exit)).or_default() += 1;
            *census.slice_instructions.entry(run.instructions).or_default() += 1;
            *census.slice_blocks.entry(run.blocks).or_default() += 1;
            *census
                .sites
                .entry((exit_name(&run.exit), exit_pc(&run.exit)))
                .or_default() += 1;
        });
    }

    fn head(map: &BTreeMap<u32, u64>, limit: usize) -> String {
        map.iter()
            .take(limit)
            .map(|(key, count)| format!("{key}:{count}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(super) fn report() {
        CENSUS.with(|census| {
            let census = census.borrow();
            if census.slices == 0 {
                return;
            }
            eprintln!(
                "[dispatch-census] slices={} instructions={} blocks={} \
                 instructions_per_slice={:.3} blocks_per_slice={:.3} instructions_per_block={:.3}",
                census.slices,
                census.total_instructions,
                census.total_blocks,
                census.total_instructions as f64 / census.slices as f64,
                census.total_blocks as f64 / census.slices as f64,
                census.total_instructions as f64 / census.total_blocks.max(1) as f64,
            );
            eprintln!("[dispatch-census] slice_exit={:?}", census.exits);
            eprintln!(
                "[dispatch-census] slice_instruction_histogram {}",
                head(&census.slice_instructions, 24)
            );
            eprintln!(
                "[dispatch-census] slice_block_histogram {}",
                head(&census.slice_blocks, 24)
            );
            let mut sites: Vec<_> = census.sites.iter().collect();
            sites.sort_by(|left, right| right.1.cmp(left.1));
            for ((exit, pc), count) in sites.into_iter().take(12) {
                eprintln!("[dispatch-census] site {exit} pc={pc:#010x} count={count}");
            }
        });
    }
}

/// Print the `FN64_DISPATCH_CENSUS` report, if the census was enabled. This is
/// a diagnostic; it reads nothing the runtime writes and returns no value any
/// certified evidence depends on.
pub fn report_dispatch_census() {
    if !dispatch_census::enabled() {
        return;
    }
    dispatch_census::report();
}

#[cfg(feature = "dynamic-mapped-runtime")]
fn run_catalog_block_program_dynamic(
    live: &CanonicalLiveBlockProgramV1,
    mut target: UnifiedCatalogTargetV1,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) {
    loop {
        // Keep this phase walk aligned with `run_catalog_block_program` below.
        // Dynamic mapped execution is a production catalog route, so
        // instrumenting only the static route makes an armed split
        // indistinguishable from an unreachable one.
        let mut phase = crate::task_dispatch::ResumePhaseClock::start();
        live.reconcile_before_dispatch(mem);
        phase.lap(
            &crate::task_dispatch::RESUME_RECONCILE_NS,
            Some(&crate::task_dispatch::RESUME_RECONCILE_CALLS),
        );
        let current = target.key();
        let (count, count_phase, compare, timer_pending) = with_executor(|executor| {
            (
                executor.cp0_count(),
                executor.cp0_count_phase(),
                executor.cp0_compare(),
                executor.cp0_timer_pending(),
            )
        });
        ctx.synchronize_cop0_timing(count, count_phase, compare);
        CpuInterruptLine::TIMER.set_level(ctx, timer_pending);
        CpuInterruptLine::RCP.set_level(ctx, crate::pi::cpu_interrupt_pending());
        if let Some(vector) = enter_pending_interrupt(ctx, current.pc) {
            target = resolve_unified_catalog_target(live, current.bank, vector, mem)
                .unwrap_or_else(|error| {
                    recompiled_gap_panic(format!(
                        "unified catalog interrupt vector {vector} from {current} does not resolve: {error}"
                    ))
                });
        }
        phase.lap(&crate::task_dispatch::RESUME_COP0_NS, None);

        let dispatched =
            dispatch_unified_catalog_slice(live, target, live.next_dispatch_budget(), ctx, mem)
                .unwrap_or_else(|error| recompiled_gap_panic(error));
        phase.lap(
            &crate::task_dispatch::RESUME_DISPATCH_NS,
            Some(&crate::task_dispatch::RESUME_DISPATCH_CALLS),
        );
        if dispatch_census::enabled() {
            dispatch_census::record_slice(&dispatched);
            phase = crate::task_dispatch::ResumePhaseClock::start();
        }
        live.invalidate_pending_physical_writes(mem);
        phase.lap(&crate::task_dispatch::RESUME_INVALIDATE_NS, None);

        let (count_write, compare_write) = ctx.take_cop0_timing_writes();
        if count_write.is_some() || compare_write.is_some() {
            with_executor(|executor| {
                if let Some(count) = count_write {
                    executor.set_cp0_count(count);
                }
                if let Some(compare) = compare_write {
                    executor.write_cp0_compare(compare);
                }
            });
        }
        if dispatched.instructions > 0 {
            live.charge_canonical_instructions(dispatched.instructions);
            live.publish_checkpoint(dispatched.instructions, dispatched.exit, None, ctx);
            phase.lap(&crate::task_dispatch::RESUME_EXIT_NS, None);
            crate::suspend_active_coroutine(fn64_runtime::Yield::InstructionCheckpoint {
                instructions: dispatched.instructions,
            });
            phase.lap(&crate::task_dispatch::RESUME_SUSPEND_NS, None);
        } else {
            phase.lap(&crate::task_dispatch::RESUME_EXIT_NS, None);
        }

        match dispatched.exit {
            BlockExit::Checkpoint(next) | BlockExit::Yield(next) => {
                assert!(
                    dispatched.instructions > 0,
                    "unified catalog returned {:?} without guest progress",
                    dispatched.exit
                );
                target = resolve_unified_catalog_target(live, next.bank, next.pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::HostCall { vram, resume } => {
                let host = live.install.resolve_host(vram.get()).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "unified catalog produced host target {:#010x} absent from its owned inventory",
                        vram.get()
                    ))
                });
                phase.lap(&crate::task_dispatch::RESUME_RESOLVE_NS, None);
                invoke_catalog_block_host(live, vram, resume, host, ctx, mem);
                phase.lap(
                    &crate::task_dispatch::RESUME_HOSTCALL_NS,
                    Some(&crate::task_dispatch::RESUME_HOSTCALL_CALLS),
                );
                target = resolve_unified_catalog_target(live, resume.bank, resume.pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ThreadReturn => {
                live.publish_returned(ctx);
                return;
            }
            BlockExit::Fault(fault) => {
                if park_host_scheduled_exception(Some(live), fault, ctx) {
                    unreachable!("parking a faulted host-scheduled thread does not return")
                }
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "unified catalog stopped on non-architectural guest fault after {} instructions: {fault:?}",
                        dispatched.instructions
                    ))
                });
                assert!(
                    dispatched.instructions > 0,
                    "unified catalog architectural fault made no guest progress: {fault:?}"
                );
                target = resolve_unified_catalog_target(live, fault.at.bank, vector, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ExecutableWriteFault(fault) => {
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "unified executable-write boundary retained a non-architectural fault: {fault:?}"
                    ))
                });
                target = resolve_unified_catalog_target(live, fault.at.bank, vector, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ExecutableWrite {
                source_bank,
                resume,
            } => {
                target = resolve_unified_catalog_target(live, source_bank, resume.pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ExecutableWriteResolveCall {
                source_bank,
                target_pc,
                resume,
            } => match resolve_unified_catalog_call(live, source_bank, target_pc, mem)
                .unwrap_or_else(|error| recompiled_gap_panic(error))
            {
                UnifiedCatalogCallV1::Host => {
                    let host = live
                        .install
                        .resolve_host(target_pc.get())
                        .unwrap_or_else(|| {
                            recompiled_gap_panic(format!(
                                "unified executable-write call lost host target {target_pc}"
                            ))
                        });
                    phase.lap(&crate::task_dispatch::RESUME_RESOLVE_NS, None);
                    invoke_catalog_block_host(live, target_pc, resume, host, ctx, mem);
                    phase.lap(
                        &crate::task_dispatch::RESUME_HOSTCALL_NS,
                        Some(&crate::task_dispatch::RESUME_HOSTCALL_CALLS),
                    );
                    target = resolve_unified_catalog_target(live, source_bank, resume.pc, mem)
                        .unwrap_or_else(|error| recompiled_gap_panic(error));
                }
                UnifiedCatalogCallV1::Guest(next) => target = next,
            },
            BlockExit::ImageChanged { at, .. } => {
                target = resolve_unified_catalog_target(live, at.bank, at.pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::Transfer(_)
            | BlockExit::ResolveTransfer { .. }
            | BlockExit::ResolveCall { .. } => {
                unreachable!("unified catalog slice returned an internal transfer boundary")
            }
        }
        phase.lap(&crate::task_dispatch::RESUME_RESOLVE_NS, None);
    }
}

pub(super) fn run_catalog_block_program(
    live: &CanonicalLiveBlockProgramV1,
    mut entry: ExecutionKey,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) {
    if live.dynamic_execution_installed() {
        #[cfg(feature = "dynamic-mapped-runtime")]
        {
            run_catalog_block_program_dynamic(
                live,
                UnifiedCatalogTargetV1::Static(entry),
                ctx,
                mem,
            );
            return;
        }
        #[cfg(not(feature = "dynamic-mapped-runtime"))]
        unreachable!("dynamic execution cannot be installed without its feature");
    }
    live.reconcile_before_dispatch(mem);
    entry = resolve_catalog_transfer_with_activation(live, entry.bank, entry.pc, mem)
        .unwrap_or_else(|error| recompiled_gap_panic(error));
    loop {
        // Split `resume NET` -- 83.2% of a WM2000 render field with no
        // sub-counters. One walking clock per loop iteration; one iteration is
        // one scheduling step, because the body ends in
        // `suspend_active_coroutine` below. Disarmed to nothing without
        // `FN64_RESUME_SPLIT`. See `ResumePhaseClock`.
        let mut phase = crate::task_dispatch::ResumePhaseClock::start();
        live.reconcile_before_dispatch(mem);
        phase.lap(
            &crate::task_dispatch::RESUME_RECONCILE_NS,
            Some(&crate::task_dispatch::RESUME_RECONCILE_CALLS),
        );
        let (count, count_phase, compare, timer_pending) = with_executor(|executor| {
            (
                executor.cp0_count(),
                executor.cp0_count_phase(),
                executor.cp0_compare(),
                executor.cp0_timer_pending(),
            )
        });
        ctx.synchronize_cop0_timing(count, count_phase, compare);
        CpuInterruptLine::TIMER.set_level(ctx, timer_pending);
        CpuInterruptLine::RCP.set_level(ctx, crate::pi::cpu_interrupt_pending());
        if let Some(vector) = enter_pending_interrupt(ctx, entry.pc) {
            entry = resolve_catalog_transfer_with_activation(live, entry.bank, vector, mem)
                .unwrap_or_else(|fault| {
                    recompiled_gap_panic(format!(
                        "canonical catalog interrupt vector {vector} from {entry} does not resolve: {fault:?}"
                    ))
                });
        }

        // Everything since the reconcile lap: the COP0 borrow,
        // `synchronize_cop0_timing`, both interrupt lines, and the pending
        // interrupt's vector resolve when one fires.
        phase.lap(&crate::task_dispatch::RESUME_COP0_NS, None);

        let dispatched = live
            .dispatch_exposing_exceptions_at_budget(entry, live.next_dispatch_budget(), ctx, mem)
            .unwrap_or_else(|error| {
                recompiled_gap_panic(format!(
                    "canonical catalog dispatch failed at {entry}: {error}"
                ))
            });
        // THE GUEST. Inclusive of every host shim the translated code called
        // synchronously -- graphics and audio reach `rsp_commit` from guest SP
        // register writes, so `gfx_ns` and `audio_lle_ns` are nested in this
        // figure and must be subtracted to isolate recompiled MIPS plus the
        // memory runtime. `vi_present_ns` is NOT nested here (harness arm).
        phase.lap(
            &crate::task_dispatch::RESUME_DISPATCH_NS,
            Some(&crate::task_dispatch::RESUME_DISPATCH_CALLS),
        );
        if dispatch_census::enabled() {
            dispatch_census::record_slice(&dispatched);
            // Re-open the clock so a census run does not charge its own
            // recording to the invalidate phase. Off in every benchmark run,
            // but a diagnostic that silently inflates a neighbouring bucket is
            // exactly the kind of instrument defect this split exists to avoid.
            phase = crate::task_dispatch::ResumePhaseClock::start();
        }
        live.invalidate_pending_physical_writes(mem);
        phase.lap(&crate::task_dispatch::RESUME_INVALIDATE_NS, None);

        let image_changed_entry = match dispatched.exit {
            BlockExit::ImageChanged { at, .. } => Some(
                live.activate_for_fetch(at.pc, mem)
                    .map_err(|error| {
                        format!("image-change activation at {} failed: {error}", at.pc)
                    })
                    .unwrap_or_else(|error| recompiled_gap_panic(error)),
            ),
            _ => None,
        };
        let inactive_fault_entry = match dispatched.exit {
            BlockExit::Fault(CpuFault {
                at,
                kind: CpuFaultKind::NoActiveGeneration,
            }) => Some(
                live.activate_for_fetch(at.pc, mem)
                    .map_err(|error| format!("fault activation at {} failed: {error}", at.pc))
                    .unwrap_or_else(|error| recompiled_gap_panic(error)),
            ),
            _ => None,
        };
        let prepared_continuation = match (image_changed_entry, inactive_fault_entry) {
            (Some(entry), None) => Some(CanonicalPreparedContinuationV1::ImageChanged { entry }),
            (None, Some(entry)) => {
                Some(CanonicalPreparedContinuationV1::InactiveGeneration { entry })
            }
            (None, None) => None,
            (Some(_), Some(_)) => {
                unreachable!("one catalog exit prepared two native continuations")
            }
        };

        let (count_write, compare_write) = ctx.take_cop0_timing_writes();
        if count_write.is_some() || compare_write.is_some() {
            with_executor(|executor| {
                if let Some(count) = count_write {
                    executor.set_cp0_count(count);
                }
                if let Some(compare) = compare_write {
                    executor.write_cp0_compare(compare);
                }
            });
        }
        if dispatched.instructions > 0 {
            live.charge_canonical_instructions(dispatched.instructions);
            live.publish_checkpoint(
                dispatched.instructions,
                dispatched.exit,
                prepared_continuation,
                ctx,
            );
            // Exit classification, activation, COP0 write-back and checkpoint
            // publication -- everything between the guest returning and the
            // coroutine actually suspending.
            phase.lap(&crate::task_dispatch::RESUME_EXIT_NS, None);
            crate::suspend_active_coroutine(fn64_runtime::Yield::InstructionCheckpoint {
                instructions: dispatched.instructions,
            });
            // The ON-STACK cost of suspending: the journal flush and the switch
            // itself, with the time this stack spent PARKED subtracted by
            // `ResumePhaseClock::lap`. Without that subtraction this row read
            // 54.1 ms/field inside a 40.6 ms field, because it was charging
            // every other thread's work to this one.
            phase.lap(&crate::task_dispatch::RESUME_SUSPEND_NS, None);
        } else {
            // No guest progress: no suspend happens, so the exit work still
            // belongs to the exit phase and the clock runs on unbroken.
            phase.lap(&crate::task_dispatch::RESUME_EXIT_NS, None);
        }

        match dispatched.exit {
            BlockExit::Checkpoint(next) | BlockExit::Yield(next) => {
                assert!(
                    dispatched.instructions > 0,
                    "canonical catalog returned {:?} without guest progress",
                    dispatched.exit
                );
                entry = resolve_catalog_transfer_with_activation(live, next.bank, next.pc, mem)
                    .unwrap_or_else(|fault| {
                        recompiled_gap_panic(format!(
                            "canonical catalog continuation {next} does not resolve: {fault:?}"
                        ))
                    });
            }
            BlockExit::HostCall { vram, resume } => {
                let host = live.install.resolve_host(vram.get()).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "canonical catalog dispatch produced host target {:#010x} absent from its owned inventory",
                        vram.get()
                    ))
                });
                // THE HOST-CALL BUCKET, and it is where graphics actually
                // lives. `osSpTaskStartGo_recomp` is a guest OS-call shim
                // reached as a `BlockExit::HostCall`, and it runs
                // `dispatch_lle_task` synchronously -- which is where `gfx_ns`
                // is armed. Folding this into the next-entry resolution below
                // produced a bucket that was ~95% graphics under a label that
                // said "resolve", and made `gfx_ns` (21.530) exceed the
                // `dispatch` bucket (7.713) that was supposed to contain it.
                //
                // Timed separately so "how much of the field is graphics" and
                // "how much is the dispatch machinery" are different rows. The
                // shim also suspends, but no bracketing is needed:
                // `ResumePhaseClock::lap` subtracts parked time centrally for
                // every suspend path (see its doc comment).
                // Close the resolve phase at the exact instant the host call
                // begins, so the `resolve_host` lookup above is charged to
                // resolution and not to the host call.
                phase.lap(&crate::task_dispatch::RESUME_RESOLVE_NS, None);
                invoke_catalog_block_host(live, vram, resume, host, ctx, mem);
                phase.lap(
                    &crate::task_dispatch::RESUME_HOSTCALL_NS,
                    Some(&crate::task_dispatch::RESUME_HOSTCALL_CALLS),
                );
                entry = resolve_catalog_transfer_with_activation(live, resume.bank, resume.pc, mem)
                    .unwrap_or_else(|fault| {
                        recompiled_gap_panic(format!(
                            "canonical catalog host resume {resume} does not resolve: {fault:?}"
                        ))
                    });
            }
            BlockExit::ThreadReturn => {
                live.publish_returned(ctx);
                return;
            }
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::NoActiveGeneration,
                ..
            }) => {
                entry = inactive_fault_entry.expect("inactive fault activation was prepared");
            }
            BlockExit::Fault(fault) => {
                assert!(
                    dispatched.instructions > 0,
                    "canonical catalog returned {:?} without guest progress",
                    dispatched.exit
                );
                if park_host_scheduled_exception(Some(live), fault, ctx) {
                    unreachable!("parking a faulted host-scheduled thread does not return")
                }
                let fault_bank = fault.at.bank;
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "canonical catalog stopped on non-architectural guest fault: {fault:?}"
                    ))
                });
                entry = resolve_catalog_transfer_with_activation(live, fault_bank, vector, mem)
                    .unwrap_or_else(|mapping_fault| {
                        recompiled_gap_panic(format!(
                            "canonical catalog exception vector {vector} does not resolve: {mapping_fault:?}"
                        ))
                    });
            }
            BlockExit::ExecutableWrite {
                source_bank,
                resume,
            } if live.generations.is_some() => {
                entry = resolve_catalog_transfer_with_activation(live, source_bank, resume.pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ExecutableWriteResolveCall {
                source_bank,
                target_pc,
                resume,
            } if live.generations.is_some() => {
                match resolve_catalog_call_with_activation(live, source_bank, target_pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error))
                {
                    CatalogCallResolutionV1::Guest(next) => entry = next,
                    CatalogCallResolutionV1::Host(host) => {
                        // Second host-call site; same bucket as the arm above.
                        phase.lap(&crate::task_dispatch::RESUME_RESOLVE_NS, None);
                        invoke_catalog_block_host(live, target_pc, resume, host, ctx, mem);
                        phase.lap(
                            &crate::task_dispatch::RESUME_HOSTCALL_NS,
                            Some(&crate::task_dispatch::RESUME_HOSTCALL_CALLS),
                        );
                        entry = resolve_catalog_transfer_with_activation(
                            live,
                            source_bank,
                            resume.pc,
                            mem,
                        )
                        .unwrap_or_else(|error| recompiled_gap_panic(error));
                    }
                }
            }
            BlockExit::ExecutableWriteFault(fault) if live.generations.is_some() => {
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "canonical generation executable-write boundary retained a non-architectural fault: {fault:?}"
                    ))
                });
                entry = resolve_catalog_transfer_with_activation(live, fault.at.bank, vector, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ImageChanged { .. } if live.generations.is_some() => {
                entry = image_changed_entry.expect("image-change activation was prepared");
            }
            BlockExit::ExecutableWrite { .. }
            | BlockExit::ExecutableWriteResolveCall { .. }
            | BlockExit::ExecutableWriteFault(_)
            | BlockExit::ImageChanged { .. } => {
                recompiled_gap_panic(format!(
                    "canonical static catalog encountered an executable-image mutation boundary: {:?}",
                    dispatched.exit
                ));
            }
            BlockExit::Transfer(_)
            | BlockExit::ResolveTransfer { .. }
            | BlockExit::ResolveCall { .. } => {
                unreachable!("catalog dispatch returned an internal transfer boundary")
            }
        }
        // Next-entry resolution and any host call the exit dispatched. The
        // `ThreadReturn` arm returns above without lapping, so one final
        // partial interval per thread exit goes unattributed -- bounded by the
        // thread count, not the step count, and therefore far below the
        // residual this split reports.
        phase.lap(&crate::task_dispatch::RESUME_RESOLVE_NS, None);
    }
}

/// Dispatch a newly-created OSThread through the installed typed module.
/// Returns `false` only for the legacy C configuration.
///
/// # Safety
/// `rdram` carries the same process-lifetime allocation contract as
/// `osCreateThread_recomp` and `recompiled::boot_thread0`.
pub(crate) unsafe fn run_registered_entry(
    rdram: *mut u8,
    entry_vram: u32,
    arg: u64,
    sp: u64,
    initial_status: Option<u32>,
) -> bool {
    let (catalog, program, registered) = with_host(|host| {
        (
            host.canonical_recompiled_program.clone(),
            host.recompiled_program.clone(),
            host.recompiled_lookup
                .map(|lookup| (lookup, host.recompiled_rdram_len)),
        )
    });
    if let Some(catalog) = catalog {
        let rdram_len = with_host(|host| host.recompiled_rdram_len);
        assert!(
            rdram_len > 0,
            "canonical recompiled program has no RDRAM length"
        );
        // SAFETY: inherited from the caller's shared-allocation contract.
        let bytes = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
        let mut mem = Rdram::new(bytes);
        let entry_pc = GuestPc::new(entry_vram);
        #[cfg(feature = "dynamic-mapped-runtime")]
        if catalog.dynamic_execution_installed() {
            let target =
                resolve_unified_catalog_entry(&catalog, entry_pc, &mem).unwrap_or_else(|error| {
                    panic!(
                    "spawned canonical OSThread entry {entry_vram:#010x} is not executable: {error}"
                )
                });
            let mut ctx = new_osthread_context(initial_status);
            ctx.set_r(4, arg);
            ctx.set_r(29, sp);
            ctx.set_r32(31, THREAD_RETURN_SENTINEL as i32);
            ctx.set_thread_return_pc(Some(THREAD_RETURN_SENTINEL));
            run_catalog_block_program_dynamic(&catalog, target, &mut ctx, &mut mem);
            return true;
        }
        let entry = resolve_catalog_entry_with_activation(&catalog, entry_pc, &mem).unwrap_or_else(
            |error| {
                panic!(
                    "spawned canonical OSThread entry {entry_vram:#010x} is not executable: {error}"
                )
            },
        );
        let mut ctx = new_osthread_context(initial_status);
        ctx.set_r(4, arg);
        ctx.set_r(29, sp);
        ctx.set_r32(31, THREAD_RETURN_SENTINEL as i32);
        ctx.set_thread_return_pc(Some(THREAD_RETURN_SENTINEL));
        run_catalog_block_program(&catalog, entry, &mut ctx, &mut mem);
        return true;
    }
    if let Some(program) = program {
        let entry = program
            .resolve_entry(GuestPc::new(entry_vram))
            .unwrap_or_else(|fault| {
                panic!("spawned OSThread entry {entry_vram:#010x} is not executable: {fault:?}")
            });
        let rdram_len = with_host(|host| host.recompiled_rdram_len);
        assert!(
            rdram_len > 0,
            "recompiled block program has no RDRAM length"
        );
        // SAFETY: inherited from the caller's shared-allocation contract.
        let bytes = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
        let mut mem = Rdram::new(bytes);
        let mut ctx = new_osthread_context(initial_status);
        ctx.set_r(4, arg);
        ctx.set_r(29, sp);
        ctx.set_r32(31, THREAD_RETURN_SENTINEL as i32);
        ctx.set_thread_return_pc(Some(THREAD_RETURN_SENTINEL));
        run_block_program(&program, entry, &mut ctx, &mut mem);
        return true;
    }
    let Some((lookup, rdram_len)) = registered else {
        return false;
    };
    assert!(rdram_len > 0, "recompiled entry lookup has no RDRAM length");
    // SAFETY: inherited from the caller's shared-allocation contract.
    let bytes = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
    let mut mem = Rdram::new(bytes);
    let mut ctx = new_osthread_context(initial_status);
    ctx.set_r(4, arg);
    ctx.set_r(29, sp);
    lookup(entry_vram)(&mut ctx, &mut mem);
    true
}

fn c_fpr_image_from_physical(state: PhysicalFgrState, fr: bool) -> [u64; 32] {
    let physical = state.into_words();
    if fr {
        return physical;
    }

    // Valid generated FR=0 operations consume each even slot as one active
    // paired FPR. Direct odd double/64-bit operations are invalid and remain
    // loud, so the unreachable odd slots can carry both corresponding latent
    // upper words, making this a reversible 2048-bit permutation.
    let mut packed = [0u64; 32];
    for pair in 0..16 {
        let even = pair * 2;
        let odd = even + 1;
        packed[even] = u64::from(physical[even] as u32) | (u64::from(physical[odd] as u32) << 32);
        packed[odd] = (physical[even] >> 32) | (physical[odd] & 0xFFFF_FFFF_0000_0000);
    }
    packed
}

fn physical_from_c_fpr_image(packed: [u64; 32], fr: bool) -> PhysicalFgrState {
    if fr {
        return PhysicalFgrState::from_words(packed);
    }

    let mut physical = [0u64; 32];
    for pair in 0..16 {
        let even = pair * 2;
        let odd = even + 1;
        physical[even] = u64::from(packed[even] as u32) | ((packed[odd] as u32 as u64) << 32);
        physical[odd] = (packed[even] >> 32) | (packed[odd] & 0xFFFF_FFFF_0000_0000);
    }
    PhysicalFgrState::from_words(physical)
}

pub(super) fn c_from_recompiled(ctx: &RsContext) -> CContext {
    let r = ctx.gprs();
    let mut c = CContext::zeroed();
    c.r0 = r[0];
    c.r1 = r[1];
    c.r2 = r[2];
    c.r3 = r[3];
    c.r4 = r[4];
    c.r5 = r[5];
    c.r6 = r[6];
    c.r7 = r[7];
    c.r8 = r[8];
    c.r9 = r[9];
    c.r10 = r[10];
    c.r11 = r[11];
    c.r12 = r[12];
    c.r13 = r[13];
    c.r14 = r[14];
    c.r15 = r[15];
    c.r16 = r[16];
    c.r17 = r[17];
    c.r18 = r[18];
    c.r19 = r[19];
    c.r20 = r[20];
    c.r21 = r[21];
    c.r22 = r[22];
    c.r23 = r[23];
    c.r24 = r[24];
    c.r25 = r[25];
    c.r26 = r[26];
    c.r27 = r[27];
    c.r28 = r[28];
    c.r29 = r[29];
    c.r30 = r[30];
    c.r31 = r[31];
    c.hi = ctx.hi;
    c.lo = ctx.lo;
    c.status_reg = ctx.cop0_status;
    c.mips3_float_mode = u8::from(ctx.cop0_status & STATUS_FR != 0);
    c.set_fpr_u64_bits(c_fpr_image_from_physical(
        ctx.physical_fgr_state(),
        c.mips3_float_mode == 1,
    ));
    c.assert_float_mode_matches_status();
    c
}

pub(super) fn copy_c_back(c: &CContext, ctx: &mut RsContext) {
    c.assert_float_mode_matches_status();
    ctx.set_gprs([
        c.r0, c.r1, c.r2, c.r3, c.r4, c.r5, c.r6, c.r7, c.r8, c.r9, c.r10, c.r11, c.r12, c.r13,
        c.r14, c.r15, c.r16, c.r17, c.r18, c.r19, c.r20, c.r21, c.r22, c.r23, c.r24, c.r25, c.r26,
        c.r27, c.r28, c.r29, c.r30, c.r31,
    ]);
    ctx.hi = c.hi;
    ctx.lo = c.lo;
    ctx.replace_physical_fgr_state(physical_from_c_fpr_image(
        c.fpr_u64_bits(),
        c.mips3_float_mode == 1,
    ));
    ctx.cop0_status = c.status_reg;
}

#[cfg(test)]
fn is_test_c_shim(shim: CShim) -> bool {
    [
        tests::no_op_fpr_shim as CShim,
        tests::write_f5_word_shim as CShim,
        tests::change_fr_shim as CShim,
        tests::change_bev_shim as CShim,
    ]
    .into_iter()
    .any(|allowed| std::ptr::fn_addr_eq(allowed, shim))
}

fn is_admitted_fr_stable_c_shim(shim: CShim) -> bool {
    is_generated_adapter_c_shim(shim)
        || [
            crate::__osInitialize_common_recomp as CShim,
            crate::osInitialize_recomp as CShim,
            crate::__osInitialize_msp_recomp as CShim,
            crate::__osInitialize_kmc_recomp as CShim,
            crate::__osInitialize_isv_recomp as CShim,
        ]
        .into_iter()
        .any(|allowed| std::ptr::fn_addr_eq(allowed, shim))
        || cfg!(test) && {
            #[cfg(test)]
            {
                is_test_c_shim(shim)
            }
            #[cfg(not(test))]
            {
                false
            }
        }
}

fn shim_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("FN64_RECOMP_RS_SHIM_TRACE").is_some())
}

pub(super) fn call_c(ctx: &mut RsContext, mem: &mut Rdram<'_>, name: &'static str, shim: CShim) {
    // An exit snapshot cannot observe a shim which changes FR, accesses the
    // other FPR view, then restores FR. Admit only the closed host-shim set
    // whose implementations preserve FR for the entire call.
    assert!(
        is_admitted_fr_stable_c_shim(shim),
        "C shim {name} is not in the FR-stable adapter registry"
    );
    if shim_trace_enabled() {
        eprintln!("[fn64-cpu-runtime-shim] {name}");
    }
    let mut c = c_from_recompiled(ctx);
    // `f_odd` aliases this stack-local context, so arm it only after the C
    // image has reached its stable address.
    c.arm_fpr_alias();
    let entry_fr = c.mips3_float_mode;
    let entry_bev = c.status_reg & STATUS_BEV;
    let rdram = mem.as_mut_slice().as_mut_ptr();
    // SAFETY: `rdram` comes from the live checked Rdram view and `c` is the
    // exact `#[repr(C)]` context the existing ABI shim requires. The shim may
    // suspend/resume this same coroutine, but neither pointer changes while
    // the adapter's stack frame remains live.
    unsafe { shim(rdram, &mut c) };
    c.assert_float_mode_matches_status();
    assert_eq!(
        c.mips3_float_mode, entry_fr,
        "C shim {name} changed Status.FR across the adapter; its packed FPR image and f_odd alias still describe the entry view"
    );
    assert_eq!(
        c.status_reg & STATUS_BEV,
        entry_bev,
        "C shim {name} changed Status.BEV across the adapter; bootstrap-vector reachability requires a typed Status-replacement boundary"
    );
    copy_c_back(&c, ctx);
}

/// Construct the architectural context installed by public `osCreateThread`.
/// The libultra `osCreateThread` manual's DESCRIPTION section specifies that
/// every new thread starts with denormal-result flushing and Invalid exceptions
/// enabled. Keeping this in the context makes coroutine suspension itself the
/// FCSR save/restore boundary.
pub(super) fn new_osthread_context(initial_status: Option<u32>) -> RsContext {
    let mut ctx = RsContext::new();
    ctx.initialize_invalid_tlb_entries();
    if let Some(status) = initial_status {
        // A libultra-created OSThread starts in the FR=0 paired-register view;
        // it does not inherit the reset thread's FR=1 view. The generated NWXE
        // osCreateThread body makes that constraint concrete by initializing
        // its saved SR to 0x0000_ff03. The host scheduler eagerly retains the
        // caller's other modeled Status fields, but must close this view
        // transition before the new coroutine can execute paired doubles.
        ctx.cop0_status = status & !STATUS_FR;
    }
    ctx.write_fcr(31, INITIAL_FPCSR);
    ctx
}

fn initialize_typed_fpcsr(
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
    name: &'static str,
    shim: CShim,
) {
    call_c(ctx, mem, name, shim);
    ctx.write_fcr(31, INITIAL_FPCSR);
}

pub fn os_initialize_common(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(
        ctx,
        mem,
        "__osInitialize_common_recomp",
        crate::__osInitialize_common_recomp,
    );
}

pub fn os_initialize(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(ctx, mem, "osInitialize_recomp", crate::osInitialize_recomp);
}

pub fn os_initialize_msp(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(
        ctx,
        mem,
        "__osInitialize_msp_recomp",
        crate::__osInitialize_msp_recomp,
    );
}

pub fn os_initialize_kmc(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(
        ctx,
        mem,
        "__osInitialize_kmc_recomp",
        crate::__osInitialize_kmc_recomp,
    );
}

pub fn os_initialize_isv(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(
        ctx,
        mem,
        "__osInitialize_isv_recomp",
        crate::__osInitialize_isv_recomp,
    );
}

/// Typed `__osSetFpcCsr`: use the same per-OSThread FCSR authority as emitted
/// CFC1/CTC1. A write which requests an exception stays loud because a host
/// call cannot return the arbitrary-PC lane's typed guest transfer.
pub fn os_set_fpc_csr(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    let previous = ctx.read_fcr(31);
    ctx.write_fcr(31, ctx.r_u32(4));
    ctx.set_r32(2, previous as i32);
    if ctx.fcsr_exception_pending() {
        fn64_cpu_runtime::trap_unsupported(
            "__osSetFpcCsr wrote an enabled FCSR cause through a host-call boundary",
        );
    }
}

macro_rules! c_adapters {
    ($(($recompiled:ident, $shim:ident)),+ $(,)?) => {
        fn is_generated_adapter_c_shim(shim: CShim) -> bool {
            std::ptr::fn_addr_eq(shim, crate::osCreateThread_recomp as CShim)
                $(|| std::ptr::fn_addr_eq(shim, crate::$shim as CShim))+
        }

        $(
            pub fn $recompiled(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
                call_c(ctx, mem, stringify!($shim), crate::$shim);
            }
        )+
    };
}

thread_local! {
    static PENDING_OSTHREAD_STATUS: std::cell::Cell<Option<u32>> = const {
        std::cell::Cell::new(None)
    };
}

pub(crate) fn take_pending_osthread_status() -> Option<u32> {
    PENDING_OSTHREAD_STATUS.with(std::cell::Cell::take)
}

pub fn os_create_thread(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    PENDING_OSTHREAD_STATUS.with(|pending| {
        assert!(
            pending.replace(Some(ctx.cop0_status)).is_none(),
            "os_create_thread: nested typed OSThread status publication"
        );
    });
    call_c(
        ctx,
        mem,
        "osCreateThread_recomp",
        crate::osCreateThread_recomp,
    );
    assert!(
        take_pending_osthread_status().is_none(),
        "os_create_thread: C shim did not consume the typed OSThread status"
    );
}

c_adapters!(
    (is_prout_sync_printf, is_proutSyncPrintf_recomp),
    (check_hardware_msp, __checkHardware_msp_recomp),
    (check_hardware_kmc, __checkHardware_kmc_recomp),
    (check_hardware_isv, __checkHardware_isv_recomp),
    (os_rdb_send, __osRdbSend_recomp),
    (os_start_thread, osStartThread_recomp),
    (os_set_thread_pri, osSetThreadPri_recomp),
    (os_get_thread_pri, osGetThreadPri_recomp),
    (os_create_mesg_queue, osCreateMesgQueue_recomp),
    (os_send_mesg, osSendMesg_recomp),
    (os_recv_mesg, osRecvMesg_recomp),
    (os_set_event_mesg, osSetEventMesg_recomp),
    (os_set_timer, osSetTimer_recomp),
    (os_cart_rom_init, osCartRomInit_recomp),
    (os_pi_read_io, osPiReadIo_recomp),
    (os_pi_start_dma, osPiStartDma_recomp),
    (os_pi_get_status, osPiGetStatus_recomp),
    (os_epi_start_dma, osEPiStartDma_recomp),
    (os_epi_raw_start_dma, osEPiRawStartDma_recomp),
    (os_eeprom_probe, osEepromProbe_recomp),
    (os_eeprom_read, osEepromRead_recomp),
    (os_eeprom_write, osEepromWrite_recomp),
    (os_eeprom_long_read, osEepromLongRead_recomp),
    (os_eeprom_long_write, osEepromLongWrite_recomp),
    (os_pfs_is_plug, osPfsIsPlug_recomp),
    (os_pfs_init_pak, osPfsInitPak_recomp),
    (os_pfs_free_blocks, osPfsFreeBlocks_recomp),
    (os_pfs_allocate_file, osPfsAllocateFile_recomp),
    (os_pfs_delete_file, osPfsDeleteFile_recomp),
    (os_pfs_file_state, osPfsFileState_recomp),
    (os_pfs_find_file, osPfsFindFile_recomp),
    (os_pfs_read_write_file, osPfsReadWriteFile_recomp),
    (os_flash_init, osFlashInit_recomp),
    (os_flash_read_status, osFlashReadStatus_recomp),
    (os_flash_read_id, osFlashReadId_recomp),
    (os_flash_clear_status, osFlashClearStatus_recomp),
    (os_flash_all_erase, osFlashAllErase_recomp),
    (os_flash_all_erase_through, osFlashAllEraseThrough_recomp),
    (os_flash_sector_erase, osFlashSectorErase_recomp),
    (
        os_flash_sector_erase_through,
        osFlashSectorEraseThrough_recomp
    ),
    (os_flash_check_erase_end, osFlashCheckEraseEnd_recomp),
    (os_flash_write_buffer, osFlashWriteBuffer_recomp),
    (os_flash_write_array, osFlashWriteArray_recomp),
    (os_flash_read_array, osFlashReadArray_recomp),
    (os_flash_change, osFlashChange_recomp),
    (os_virtual_to_physical, osVirtualToPhysical_recomp),
    (os_create_pi_manager, osCreatePiManager_recomp),
    (os_si_device_busy, __osSiDeviceBusy_recomp),
    (os_si_raw_start_dma, __osSiRawStartDma_recomp),
    (os_ai_set_frequency, osAiSetFrequency_recomp),
    (os_ai_get_length, osAiGetLength_recomp),
    (os_ai_set_next_buffer, osAiSetNextBuffer_recomp),
    (os_get_mem_size, osGetMemSize_recomp),
    (os_inval_dcache, osInvalDCache_recomp),
    (os_inval_icache, osInvalICache_recomp),
    (os_writeback_dcache, osWritebackDCache_recomp),
    (os_disable_int, __osDisableInt_recomp),
    (os_restore_int, __osRestoreInt_recomp),
    (os_get_thread_id, osGetThreadId_recomp),
    (os_get_time, osGetTime_recomp),
    (os_set_count, osSetCount_recomp),
    (os_sp_task_yielded, osSpTaskYielded_recomp),
    (os_create_vi_manager, osCreateViManager_recomp),
    (os_vi_set_event, osViSetEvent_recomp),
    (os_vi_set_mode, osViSetMode_recomp),
    (os_vi_set_special_features, osViSetSpecialFeatures_recomp),
    (os_vi_set_x_scale, osViSetXScale_recomp),
    (os_vi_set_y_scale, osViSetYScale_recomp),
    (os_vi_swap_buffer, osViSwapBuffer_recomp),
    (os_vi_black, osViBlack_recomp),
    (os_vi_fade, osViFade_recomp),
    (os_vi_repeat_line, osViRepeatLine_recomp),
    (ll_div, __ll_div_recomp),
    (ll_mul, __ll_mul_recomp),
    (ull_div, __ull_div_recomp),
    (ull_rem, __ull_rem_recomp),
    (ull_to_d, __ull_to_d_recomp),
    (ull_to_f, __ull_to_f_recomp),
    (os_pi_get_access, __osPiGetAccess_recomp),
    (os_pi_rel_access, __osPiRelAccess_recomp),
    (os_sp_set_pc, __osSpSetPc_recomp),
    (os_sp_set_status, __osSpSetStatus_recomp),
    (os_cont_get_query, osContGetQuery_recomp),
    (os_cont_get_read_data, osContGetReadData_recomp),
    (os_cont_init, osContInit_recomp),
    (os_cont_set_ch, osContSetCh_recomp),
    (os_cont_start_query, osContStartQuery_recomp),
    (os_cont_start_read_data, osContStartReadData_recomp),
    (os_motor_init, osMotorInit_recomp),
    (os_motor_access, __osMotorAccess_recomp),
    (os_motor_start, osMotorStart_recomp),
    (os_motor_stop, osMotorStop_recomp),
    (os_voice_set_word, osVoiceSetWord_recomp),
    (os_voice_check_word, osVoiceCheckWord_recomp),
    (os_voice_stop_read_data, osVoiceStopReadData_recomp),
    (os_voice_init, osVoiceInit_recomp),
    (os_voice_mask_dictionary, osVoiceMaskDictionary_recomp),
    (os_voice_start_read_data, osVoiceStartReadData_recomp),
    (os_voice_control_gain, osVoiceControlGain_recomp),
    (os_voice_get_read_data, osVoiceGetReadData_recomp),
    (os_voice_clear_dictionary, osVoiceClearDictionary_recomp),
    (os_destroy_thread, osDestroyThread_recomp),
    (os_stop_thread, osStopThread_recomp),
    (os_dp_set_status, osDpSetStatus_recomp),
    (os_dp_set_next_buffer, osDpSetNextBuffer_recomp),
    (os_epi_read_io, osEPiReadIo_recomp),
    (os_epi_write_io, osEPiWriteIo_recomp),
    (os_get_count, osGetCount_recomp),
    (os_jam_mesg, osJamMesg_recomp),
    (os_sp_task_load, osSpTaskLoad_recomp),
    (os_sp_task_start_go, osSpTaskStartGo_recomp),
    (os_sp_task_yield, osSpTaskYield_recomp),
    (os_stop_timer, osStopTimer_recomp),
    (
        os_vi_get_current_framebuffer,
        osViGetCurrentFramebuffer_recomp
    ),
    (os_vi_get_next_framebuffer, osViGetNextFramebuffer_recomp),
    (os_writeback_dcache_all, osWritebackDCacheAll_recomp),
    (os_sp_get_status, __osSpGetStatus_recomp),
    (os_dp_get_status, osDpGetStatus_recomp),
);

pub(super) fn abi_host_shim_callable(shim: AbiHostShimV1) -> RecompFunc {
    match shim {
        AbiHostShimV1::OsCreateMesgQueue => os_create_mesg_queue,
        AbiHostShimV1::OsCreateThread => os_create_thread,
        AbiHostShimV1::OsEPiStartDma => os_epi_start_dma,
        AbiHostShimV1::OsGetThreadPri => os_get_thread_pri,
        AbiHostShimV1::OsRecvMesg => os_recv_mesg,
        AbiHostShimV1::OsSendMesg => os_send_mesg,
        AbiHostShimV1::OsSetEventMesg => os_set_event_mesg,
        AbiHostShimV1::OsSiDeviceBusy => os_si_device_busy,
        AbiHostShimV1::OsSetThreadPri => os_set_thread_pri,
        AbiHostShimV1::OsSetTimer => os_set_timer,
        AbiHostShimV1::OsSpTaskLoad => os_sp_task_load,
        AbiHostShimV1::OsSpTaskStartGo => os_sp_task_start_go,
        AbiHostShimV1::OsSpTaskYield => os_sp_task_yield,
        AbiHostShimV1::OsSpTaskYielded => os_sp_task_yielded,
        AbiHostShimV1::OsStartThread => os_start_thread,
        AbiHostShimV1::OsEPiWriteIo => os_epi_write_io,
        AbiHostShimV1::OsEPiReadIo => os_epi_read_io,
        AbiHostShimV1::OsFlashInit => os_flash_init,
        AbiHostShimV1::OsFlashSectorErase => os_flash_sector_erase,
        AbiHostShimV1::OsFlashReadArray => os_flash_read_array,
    }
}

pub(super) fn abi_host_shim_writer_effects(shim: AbiHostShimV1) -> Vec<WriterChannel> {
    // These are conservative synchronous/nested effects of invoking the shim,
    // not claims about all later guest execution. Every adapter may mutate
    // guest memory through its HostAbi parent transaction. Queue send/receive
    // and thread start can suspend while another guest thread and every device
    // child advance; task start can execute RSP and RDP children synchronously.
    let all_live_channels = || {
        vec![
            WriterChannel::CpuInstructionStore,
            WriterChannel::PiDma,
            WriterChannel::SiDma,
            WriterChannel::SpDma,
            WriterChannel::RspExecutionOrHleWriteback,
            WriterChannel::RdpRenderer,
            WriterChannel::HostAbi,
        ]
    };
    match shim {
        // Programmed single-word IO commits through the same PI save-device
        // path as a domain-2 DMA, so it carries the same channels as
        // `osEPiStartDma` rather than the HostAbi-only default.
        AbiHostShimV1::OsEPiStartDma
        | AbiHostShimV1::OsEPiWriteIo
        | AbiHostShimV1::OsEPiReadIo
        // The FlashRAM API commits to the same PI-backed save store, and
        // `osFlashReadArray` additionally writes the caller's guest buffer.
        | AbiHostShimV1::OsFlashInit
        | AbiHostShimV1::OsFlashSectorErase
        | AbiHostShimV1::OsFlashReadArray => {
            vec![WriterChannel::PiDma, WriterChannel::HostAbi]
        }
        AbiHostShimV1::OsRecvMesg | AbiHostShimV1::OsSendMesg | AbiHostShimV1::OsStartThread => {
            all_live_channels()
        }
        AbiHostShimV1::OsSpTaskStartGo => vec![
            WriterChannel::RspExecutionOrHleWriteback,
            WriterChannel::RdpRenderer,
            WriterChannel::HostAbi,
        ],
        AbiHostShimV1::OsSpTaskYield => vec![
            WriterChannel::RspExecutionOrHleWriteback,
            WriterChannel::HostAbi,
        ],
        AbiHostShimV1::OsCreateMesgQueue
        | AbiHostShimV1::OsCreateThread
        | AbiHostShimV1::OsGetThreadPri
        | AbiHostShimV1::OsSetEventMesg
        | AbiHostShimV1::OsSiDeviceBusy
        | AbiHostShimV1::OsSetThreadPri
        | AbiHostShimV1::OsSetTimer
        | AbiHostShimV1::OsSpTaskLoad
        | AbiHostShimV1::OsSpTaskYielded => vec![WriterChannel::HostAbi],
    }
}

/// `__osGetSR`: read this OSThread's typed COP0 Status register.
pub fn os_get_sr(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.set_r(2, ctx.cop0_status as u64);
}

/// `__osSetSR`: replace this OSThread's typed COP0 Status register.
pub fn os_set_sr(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.cop0_status = ctx.r_u32(4);
}

/// `__osGetCause`: the executor does not synthesize CPU exception frames, so
/// this reads the explicit typed Cause state (normally zero).
pub fn os_get_cause(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.set_r(2, ctx.cop0_cause as u64);
}

/// `osSetIntMask`: update this typed OSThread's packed mask and the shared MI
/// gate. Unlike the legacy C adapter, the prior value is owned by `ctx`, so
/// coroutine switches cannot make one thread return another thread's mask.
pub fn os_set_int_mask(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    const CPU_INTERRUPT_FIELDS: u32 = 1 | (0xFF << 8);
    let new_mask = ctx.r_u32(4);
    let previous = ctx.replace_os_interrupt_mask(new_mask);
    ctx.cop0_status = (ctx.cop0_status & !CPU_INTERRUPT_FIELDS) | (new_mask & CPU_INTERRUPT_FIELDS);
    crate::pi::set_mi_interrupt_mask((new_mask >> 16) & 0x3F);
    ctx.set_r(2, previous as u64);
}

/// `osGetIntMask`: return this typed OSThread's combined CPU/RCP mask.
pub fn os_get_int_mask(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.set_r(2, ctx.os_interrupt_mask() as u64);
}
