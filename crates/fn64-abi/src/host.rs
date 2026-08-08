use super::*;
use crate::task_dispatch::{
    executor_split_enabled, note_executor_split, EXECUTOR_CALLS, EXECUTOR_NS, EXEC_DEVTIME_NS,
    EXEC_MIRROR_CALLS, EXEC_MIRROR_NS, EXEC_RESUME_NS, PHASE_TIMING,
};

// ---------------------------------------------------------------------
// Host-facing (non-`_recomp`) helpers.
// ---------------------------------------------------------------------

/// Host-side entry point for injecting an external (SI/PI/VI-style)
/// completion into the executor -- not a `_recomp` shim.
pub fn inject_external_event(event: ExternalEvent) {
    with_executor(|exec| exec.inject_event(event));
}

/// Device events committed by one host virtual-time advance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VirtualTimeAdvance {
    vi_retrace_ticks: u32,
}

impl VirtualTimeAdvance {
    /// Number of VI retraces the device fabric committed during the advance.
    pub const fn vi_retrace_ticks(self) -> u32 {
        self.vi_retrace_ticks
    }
}

/// Host-side virtual-clock driver.
///
/// Also samples the VI retrace cadence probe (ROADMAP R5 probe 3): this is the
/// only place the host advances the guest's virtual clock, so it is the one
/// seam where wall-clock and fired-tick counts can be correlated. The returned
/// delta comes from committed device notifications rather than a host-side
/// interval prediction, so callers can distinguish an actual VI edge from an
/// earlier DMA/RCP deadline.
pub fn advance_virtual_time(now: u64) -> VirtualTimeAdvance {
    crate::task_dispatch::advance_hle_render_task();
    let vi_retrace_ticks = crate::pi::advance_device_time(now);
    with_executor(|exec| exec.advance_time(now));
    VirtualTimeAdvance { vi_retrace_ticks }
}

/// Next pending device-fabric deadline or immediately runnable HLE renderer
/// continuation, if any.
/// Guest slices charge little or no virtual time in the C lane, so a DMA
/// issued mid-slice lands its deadline at `sim_time + latency` -- one cycle
/// past what the post-slice commit reaches. A pump that only advances in
/// field-sized steps therefore delivers EVERY completion a full field late,
/// which breaks real issue-then-poll-next-cycle guest code (observed:
/// NWXE's hand-rolled joybus pipeline). Idle loops should advance to this
/// deadline first when it falls before their next scheduled tick.
/// ponytail: fabric deadlines only; the timer wheel still quantizes to the
/// pump interval -- surface it here too if a title's timers prove
/// sub-field-sensitive.
pub fn next_device_deadline() -> Option<u64> {
    if crate::task_dispatch::hle_render_needs_progress() {
        return Some(sim_time());
    }
    with_host(|host| host.device_fabric.next_deadline().map(|d| d.get()))
}

/// Register the one process-wide RDRAM allocation with every runtime owner.
///
/// # Safety
/// `rdram` must address `rdram_len` live bytes until every guest coroutine and
/// device request has stopped using the process allocation.
pub unsafe fn register_process_rdram(rdram: *mut u8, rdram_len: usize) {
    assert!(!rdram.is_null(), "process RDRAM pointer must be non-null");
    assert!(rdram_len > 0, "process RDRAM length must be nonzero");
    with_host(|host| {
        if !host.runtime_rdram.is_null() {
            assert!(
                host.runtime_rdram == rdram && host.runtime_rdram_len == rdram_len,
                "process RDRAM registration cannot replace the live allocation: registered pointer={:?} length={}, requested pointer={rdram:?} length={rdram_len}",
                host.runtime_rdram,
                host.runtime_rdram_len,
            );
        }
        host.runtime_rdram = rdram;
        host.runtime_rdram_len = rdram_len;
        host.native_execution_destinations.clear();
    });
    with_executor(|exec| unsafe { exec.set_rdram_base_with_len(rdram, rdram_len) });
}

#[cfg(feature = "recomp-rs")]
pub(crate) fn install_owned_process_rdram(mut storage: Box<[u8]>) -> (*mut u8, usize) {
    assert!(
        !storage.is_empty(),
        "owned process RDRAM allocation must be nonempty"
    );
    let pointer = storage.as_mut_ptr();
    let length = storage.len();
    with_host(|host| {
        assert!(
            host.owned_runtime_rdram.is_none() && host.runtime_rdram.is_null(),
            "owned process RDRAM cannot replace an existing allocation"
        );
        host.owned_runtime_rdram = Some(storage);
    });
    // SAFETY: HostState now owns the boxed allocation for the runtime lifetime.
    unsafe { register_process_rdram(pointer, length) };
    (pointer, length)
}

/// Install the structurally discovered guest global that libultra's exception
/// handler reads to find the current `OSThread`.
pub fn set_guest_running_thread_global(vram: u32) {
    let address = RdramAddr::from_gpr(u64::from(vram));
    with_host(|host| {
        assert!(
            host.guest_running_thread_global.is_none()
                || host.guest_running_thread_global == Some(address),
            "guest running-thread global cannot change after installation"
        );
        host.guest_running_thread_global = Some(address);
    });
}

fn mirror_guest_running_thread(thread_id: ThreadId) {
    let configured = with_host(|host| {
        let global = host.guest_running_thread_global?;
        let handle = host.thread_handle_vrams.get(&thread_id).copied()?;
        Some((global, handle, host.runtime_rdram, host.runtime_rdram_len))
    });
    let Some((global, handle, rdram, rdram_len)) = configured else {
        // The synthetic bootstrap coroutine has no guest OSThread object.
        return;
    };
    assert!(
        !rdram.is_null(),
        "guest running-thread mirror has no process RDRAM"
    );
    assert!(
        global.offset() as usize + 4 <= rdram_len,
        "guest running-thread global {:#010x} exceeds registered RDRAM length {rdram_len:#x}",
        global.offset()
    );
    #[cfg(feature = "recomp-rs")]
    if crate::recompiled::commit_scheduler_running_thread_mirror(
        crate::recompiled::SchedulerRunningThreadMirrorV1::new(thread_id, global, handle),
    ) {
        return;
    }
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    unsafe { storage.write_u32(global, handle) };
}

/// Copy the complete physical RDRAM device in logical guest-byte order.
///
/// The registered allocation can include fn64's host-only sparse MMIO window;
/// release evidence must cover exactly the first eight MiB visible to the N64.
/// `None` means no process RDRAM has been registered. A partial registration is
/// an invalid runtime configuration and traps by name rather than producing a
/// partial observation.
///
/// The process runtime is single-threaded. Callers must invoke this only at a
/// host boundary where no guest coroutine or device operation is executing;
/// the boot harness's committed-VI token is the production owner of that
/// boundary.
pub fn copy_registered_physical_rdram_logical() -> Option<Vec<u8>> {
    let (rdram, allocation_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    if rdram.is_null() {
        assert_eq!(
            allocation_len, 0,
            "registered process RDRAM has a length but no storage pointer"
        );
        return None;
    }
    assert!(
        allocation_len >= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        "registered process RDRAM length {allocation_len:#x} does not cover the required {:#x}-byte physical device",
        fn64_runtime::rdram::DEFAULT_RDRAM_SIZE
    );

    // SAFETY: registration owns a process-lifetime allocation, the length
    // check above covers the complete physical device, and the host-boundary
    // contract excludes concurrent guest/device access for this call.
    Some(unsafe {
        fn64_runtime::rdram::with_physical_rdram_read(rdram, allocation_len, |physical| {
            let mut logical = vec![0; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
            for (offset, byte) in logical.iter_mut().enumerate() {
                *byte = physical.read_u8(RdramAddr::from_offset(
                    u32::try_from(offset).expect("physical RDRAM offset exceeds u32"),
                ));
            }
            logical
        })
    })
}

/// Read the registered physical RDRAM device through a call-scoped capability.
///
/// [`copy_registered_physical_rdram_logical`] answers "what does the whole
/// device contain" for one-shot release evidence; it allocates eight MiB and
/// reads it a byte at a time. A windowed host presenting the VI framebuffer
/// asks a different question sixty times a second -- "what is in these ~150 KiB
/// right now" -- and cannot pay that price per frame. This exposes the same
/// capability the internal presentation path already uses
/// (`task_dispatch::present_render_backend`) so an out-of-process presenter can
/// read exactly the span it needs without copying the device or manufacturing a
/// competing Rust slice.
///
/// `None` means no process RDRAM has been registered, which is the honest
/// answer before boot rather than an empty read.
///
/// The process runtime is single-threaded. Callers must invoke this only at a
/// host boundary where no guest coroutine or device operation is executing --
/// the same contract [`copy_registered_physical_rdram_logical`] documents. A
/// windowed presenter satisfies it by reading between executor pumps.
pub fn with_registered_physical_rdram_read<R>(
    read: impl for<'call> FnOnce(fn64_runtime::PhysicalRdramRead<'call>) -> R,
) -> Option<R> {
    let (rdram, allocation_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    if rdram.is_null() {
        assert_eq!(
            allocation_len, 0,
            "registered process RDRAM has a length but no storage pointer"
        );
        return None;
    }
    // SAFETY: registration owns a process-lifetime allocation, and the
    // host-boundary contract above excludes concurrent guest/device access for
    // this call. The higher-ranked capability prevents the reader from
    // retaining it past the call, so no competing borrow can outlive us.
    Some(unsafe { fn64_runtime::with_physical_rdram_read(rdram, allocation_len, read) })
}

/// Create and start thread 0 running `recomp_entrypoint` -- the harness's
/// boot entry point. `recomp_entrypoint`'s own body (verified directly
/// against `RecompiledFuncs/funcs_0.c`) computes its own jump target from
/// literal immediates with no dependency on incoming register state, so an
/// all-zero `RecompContext` is the correct, real starting state (matching
/// what a fresh `OSThread`'s initial register file honestly is before any
/// game code has run), not a placeholder.
///
/// # Safety
/// `rdram` must be a valid pointer to the process's one shared `rdram_len`-byte
/// buffer, live for at least as long as any coroutine spawned here might run
/// (the whole process, per `docs/DESIGN.md` section 3). `entry` must be
/// `recomp_entrypoint` (or a real `recomp_func_t`-shaped function with the
/// same contract) -- the boot harness passes the real generated symbol.
pub unsafe fn boot_thread0(
    rdram: *mut u8,
    rdram_len: usize,
    entry: RecompFunc,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe { register_process_rdram(rdram, rdram_len) };
    let rdram_addr = rdram as usize;
    with_executor(|exec| {
        exec.create_thread(thread_id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(thread_id, rdram_ptr, yielder, || {
                let _ = first_input;
                let mut ctx = RecompContext::zeroed();
                ctx.arm_fpr_alias();
                unsafe { entry(rdram_ptr, &mut ctx as *mut _) };
            });
        });
        exec.start_thread(thread_id);
    });
}

/// Run one scheduling step (see `Executor::run_one_step`'s doc comment).
/// Returns `false` when nothing was runnable -- the harness should then
/// call `advance_virtual_time` to make host-driven progress (VI retrace,
/// due timers) before trying again.
///
/// This is THE seam that re-arms the coroutine-context thread-locals (see
/// `THREAD_CONTEXTS`' doc comment) to the thread ABOUT to be resumed --
/// every caller in this crate (including this crate's own tests) must
/// dispatch a scheduling step through this function (or `run_to_idle`,
/// below), never a bare `exec.run_one_step()`/`exec.run_to_idle()` inside a
/// `with_executor` closure, or the re-arm is skipped and the bug this wave
/// fixed reappears.
pub fn run_one_step() -> bool {
    let started = PHASE_TIMING.with(Cell::get).then(std::time::Instant::now);
    // Split `executor_ns`, which has no sub-counters and is 61% of a WM2000
    // render field. Separately gated from `PHASE_TIMING` because these clocks
    // are inside the hottest loop here; see `EXECUTOR_SPLIT`'s doc comment.
    let split = executor_split_enabled();
    let resume_started = split.then(std::time::Instant::now);
    let (stepped, now) = with_executor(|exec| {
        // `peek_next_thread` is a read-only preview of exactly which thread
        // `exec.run_one_step()` is about to resume -- read it BEFORE the
        // resume so the correct thread's context is active for the entire
        // duration of that resume (including every `_recomp` shim call the
        // thread's body makes after waking, up to and including its next
        // suspend). `None` means nothing is runnable -- no resume will
        // happen, so no context needs arming.
        let stepped = match exec.peek_next_thread() {
            Some(id) => {
                // Close scheduler selection -> exception-handler observation:
                // the selected coroutine's guest OSThread pointer is visible
                // before its first instruction and cannot interleave with a
                // different resume because `run_one_step` owns RunToken.
                // Timed apart from the resume below: under `recomp-rs` this
                // is a FULL watched-region journal reconcile (see
                // `EXEC_MIRROR_NS`), not the four-byte store its name
                // suggests, and it runs on every step.
                match split.then(std::time::Instant::now) {
                    Some(at) => {
                        mirror_guest_running_thread(id);
                        note_executor_split(
                            &EXEC_MIRROR_NS,
                            Some(&EXEC_MIRROR_CALLS),
                            at.elapsed().as_nanos() as u64,
                        );
                    }
                    None => mirror_guest_running_thread(id),
                }
                with_rearmed_context(id, || exec.run_one_step())
            }
            None => exec.run_one_step(),
        };
        (stepped, exec.sim_time())
    });
    // Closes over the whole `with_executor` body -- scheduler pick, the
    // mirror boundary, and the coroutine resume. `exec_mirror_ns` is nested
    // inside this, not a peer of it; the census subtracts.
    if let Some(at) = resume_started {
        note_executor_split(&EXEC_RESUME_NS, None, at.elapsed().as_nanos() as u64);
    }
    if stepped {
        // `run_one_step` returns only after a yielding coroutine is fully
        // suspended. Commit the ABI-owned device fabric at that exact guest
        // time before any later scheduling step can resume a guest. This
        // closes the interleaving checkpoint-yield -> same-thread resume ->
        // overdue PI completion, which would otherwise execute one extra
        // translated block before bytes/MI/queue state became observable.
        match split.then(std::time::Instant::now) {
            Some(at) => {
                crate::pi::advance_device_time(now);
                note_executor_split(&EXEC_DEVTIME_NS, None, at.elapsed().as_nanos() as u64);
            }
            None => {
                crate::pi::advance_device_time(now);
            }
        }
    }
    if let Some(started) = started {
        EXECUTOR_NS.with(|total| {
            total.set(
                total
                    .get()
                    .saturating_add(started.elapsed().as_nanos() as u64),
            );
        });
        EXECUTOR_CALLS.with(|calls| calls.set(calls.get() + 1));
    }
    stepped
}

/// Priority the next [`run_one_step`] will dispatch, or `None` when the run
/// queue is empty. The window driver uses `OS_PRIORITY_IDLE` as the explicit
/// quiescence boundary documented by libultra rather than guessing from a
/// wall-time or step-count threshold.
pub fn next_runnable_priority() -> Option<Priority> {
    with_executor(|exec| exec.peek_next_priority())
}

/// Run until the run queue is idle (every thread finished or blocked). See
/// `run_one_step`'s doc comment -- this loops it rather than calling
/// `Executor::run_to_idle` directly, so every individual resume inside the
/// loop gets its own correctly-armed context (a single re-arm before the
/// whole loop would be exactly as wrong as the original bug once a second
/// thread's turn came up).
pub fn run_to_idle() {
    while run_one_step() {}
}

/// Seal the process-wide runtime for normal host-process teardown.
///
/// Recompiled threads commonly finish a bounded run suspended inside an
/// `extern "C"` blocking shim. Rust TLS destruction would otherwise make
/// `corosensei` force-unwind those native stacks across a non-unwind FFI
/// boundary, which aborts after an otherwise successful run. This terminal
/// operation detaches only started, unfinished guest coroutines, clears every
/// saved pointer into those stacks, and makes all later executor access trap.
/// The operating system reclaims the detached stack allocations when the
/// process exits.
///
/// This must be called only from the host between scheduling steps, after all
/// evidence and guest-visible persistence work is complete. It is not guest
/// `osDestroyThread` behavior and must never be used to reset or continue a
/// runtime in the same process.
pub fn prepare_process_exit() -> fn64_runtime::ProcessExitSummary {
    crate::task_dispatch::drop_backends_for_process_exit();
    let summary = EXECUTOR.with(|slot| {
        slot.with(|slot| {
            let ExecutorSlot::Active(executor) = slot else {
                panic!("fn64 prepare_process_exit called more than once");
            };
            let summary = executor.prepare_process_exit();
            *slot = ExecutorSlot::PreparedForProcessExit;
            summary
        })
    });
    with_host(|host| {
        host.runtime_rdram = std::ptr::null_mut();
        host.runtime_rdram_len = 0;
        #[cfg(feature = "recomp-rs")]
        {
            host.owned_runtime_rdram = None;
        }
    });
    THREAD_CONTEXTS.with(|contexts| contexts.borrow_mut().clear());
    ACTIVE_YIELDER.with(|active| active.set(None));
    ACTIVE_THREAD_ID.with(|active| active.set(None));
    ACTIVE_RDRAM.with(|active| active.set(std::ptr::null_mut()));
    summary
}

/// Whether thread `id` has finished (its coroutine returned or was never
/// created) -- the harness's "has boot's thread 0 died" check.
pub fn is_thread_dead(id: ThreadId) -> bool {
    with_executor(|exec| exec.is_thread_dead(id))
}

/// Gfx/audio task admission counts observed so far (`Executor::task_log`).
pub fn task_counts() -> (u64, u64) {
    with_executor(|exec| (exec.task_log().gfx_count(), exec.task_log().audio_count()))
}

/// Always-on monotonic guest-resume epoch. Release evidence uses this instead
/// of optional trace length to prove no coroutine ran after a VI boundary.
pub fn executor_resume_epoch() -> u64 {
    with_executor(|exec| exec.resume_epoch())
}

/// Copy the full recorded trace out as an owned `Vec` -- the harness's
/// entry point for emitting `docs/DESIGN.md` section 4's shared
/// `TraceEvent` stream to a file.
pub fn copy_trace() -> Vec<fn64_runtime::TraceEvent> {
    with_executor(|exec| exec.trace().to_vec())
}

/// Number of retained diagnostic executor events without cloning them.
pub fn trace_len() -> usize {
    with_executor(|exec| exec.trace().len())
}

/// Enable or disable differential event capture. This controls diagnostics
/// only; emulated scheduling and peripheral behavior are unchanged.
pub fn set_trace_enabled(enabled: bool) {
    with_executor(|exec| exec.set_trace_enabled(enabled));
}

/// Arm incremental crash-safe trace flushing -- every trace event recorded
/// from this call onward is appended+flushed to `path` immediately, not
/// just buffered in memory for `copy_trace`'s end-of-run snapshot. Call
/// this BEFORE booting thread 0, so a SIGSEGV/abort mid-boot still leaves
/// every event up to the crash on disk. See
/// `fn64_runtime::TraceLog::set_sink_file`'s doc comment for the incident
/// (WM2000 rung-3 frontier) this fixes.
pub fn set_trace_sink_file(path: &str) -> std::io::Result<()> {
    with_executor(|exec| exec.set_trace_sink_file(path))
}

/// The executor's current virtual-clock reading.
pub fn sim_time() -> u64 {
    with_executor(|exec| exec.sim_time())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use sha2::Digest;

    fn reset_evidence_owners() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = HostState::default());
    }

    #[test]
    fn scheduler_mirror_publishes_registered_guest_osthread_pointer() {
        reset_evidence_owners();
        let mut storage = vec![0u8; 0x1000];
        unsafe { register_process_rdram(storage.as_mut_ptr(), storage.len()) };
        set_guest_running_thread_global(0x8000_0100);
        with_host(|host| {
            host.thread_handle_vrams.insert(7, 0x8000_0280);
        });

        mirror_guest_running_thread(7);

        assert_eq!(
            fn64_runtime::RdramView::from_storage(&storage).read_u32(RdramAddr::from_offset(0x100)),
            0x8000_0280
        );
    }

    #[test]
    fn registered_physical_rdram_copy_is_complete_and_logical() {
        reset_evidence_owners();
        assert_eq!(copy_registered_physical_rdram_logical(), None);

        let mut storage = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE + 16];
        fn64_runtime::RdramViewMut::from_storage(&mut storage)
            .write_u32(RdramAddr::from_offset(0), 0x0123_4567);
        fn64_runtime::RdramViewMut::from_storage(&mut storage).write_u32(
            RdramAddr::from_offset(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u32 - 4),
            0x89ab_cdef,
        );
        unsafe { register_process_rdram(storage.as_mut_ptr(), storage.len()) };

        let logical = copy_registered_physical_rdram_logical().unwrap();
        assert_eq!(logical.len(), fn64_runtime::rdram::DEFAULT_RDRAM_SIZE);
        assert_eq!(&logical[..4], &[0x01, 0x23, 0x45, 0x67]);
        assert_eq!(
            &logical[fn64_runtime::rdram::DEFAULT_RDRAM_SIZE - 4..],
            &[0x89, 0xab, 0xcd, 0xef]
        );

        reset_evidence_owners();
    }

    #[test]
    #[should_panic(expected = "does not cover the required 0x800000-byte physical device")]
    fn registered_physical_rdram_copy_rejects_partial_registration() {
        reset_evidence_owners();
        let mut storage = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE - 4];
        unsafe { register_process_rdram(storage.as_mut_ptr(), storage.len()) };
        let _ = copy_registered_physical_rdram_logical();
    }

    fn populated_host_snapshot(
        reverse_hash_insertions: bool,
        rdram: &mut [u8],
    ) -> AbiHostEvidenceSnapshot {
        reset_evidence_owners();
        crate::load_rom_with_fixed_pi_latency(vec![0x12, 0x34, 0x56, 0x78], 7);
        let section = crate::register_recompiled_section(0x1000, 0x8080_0000, 0x200);
        crate::set_section_loaded(section);
        crate::set_cart_rom_handle_vram(0x8000_1000);
        crate::set_flash_handle_vram(0x8000_1100);
        crate::configure_leo_disk(crate::LeoDiskConfig {
            handle_vram: 0x8000_1200,
            latency: 3,
            page_size: 4,
            release: 2,
            pulse_width: 5,
        });
        crate::set_debug_hardware(crate::DebugHardware::Kmc);
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
            let mut install = |first: bool| {
                if first {
                    host.rsp_boot_images.insert(0x300, vec![3, 2, 1]);
                    host.thread_handles.insert(0x80, 8);
                    host.thread_guest_ids.insert(8, 0x18);
                    host.timer_handles.insert(0x180, 18);
                } else {
                    host.rsp_boot_images.insert(0x100, vec![1, 2, 3, 4]);
                    host.thread_handles.insert(0x40, 4);
                    host.thread_guest_ids.insert(4, 0x14);
                    host.timer_handles.insert(0x140, 14);
                }
            };
            install(reverse_hash_insertions);
            install(!reverse_hash_insertions);
            host.next_synthetic_thread_id = 0xF000_0042;
        });
        host_evidence_snapshot()
    }

    #[test]
    fn peripheral_evidence_accessor_combines_executor_and_manager_owners() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = HostState::default());
        let mut rdram = vec![0; 0x1000];
        unsafe { register_process_rdram(rdram.as_mut_ptr(), rdram.len()) };
        crate::si::set_controller_port_state(
            0,
            fn64_runtime::PortState::StandardControllerRumblePak,
        );
        crate::si::set_controller_state(0, 0x9000, -12, 34);
        with_executor(|executor| executor.set_rumble(0, true).unwrap());
        with_host(|host| {
            let pi_request = PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0x20),
                device: fn64_runtime::PiDeviceAddress::RomOffset(0),
                len: 0x20,
            };
            host.pending_pi_completions.push_back(PendingPiCompletion {
                request: pi_request,
                rdram: rdram.as_mut_ptr(),
                rdram_len: rdram.len(),
                ret_queue: Some(RdramAddr::from_offset(0x80)),
                ret_mesg: 7,
            });
            host.pending_si_completion = Some(PendingSiCompletion {
                request: fn64_runtime::SiDmaRequest {
                    kind: fn64_runtime::SiDmaKind::ControllerRead,
                    dram_addr: RdramAddr::from_offset(0x40),
                },
                owner: PendingSiCompletionOwner::ProcessRdram {
                    rdram: rdram.as_mut_ptr(),
                    rdram_len: rdram.len(),
                },
            });
            host.pending_vi_mode = Some(PendingViMode {
                registers: [1; 14],
                fields: [[2; 5], [3; 5]],
            });
            host.pending_vi_x_scale = Some(0.5);
            host.active_vi_y_scale = 0.75;
        });

        let snapshot = peripherals_evidence_snapshot();
        assert_eq!(
            snapshot.peripherals.pif.ports[0],
            fn64_runtime::PortState::StandardControllerRumblePak
        );
        assert_eq!(snapshot.peripherals.pif.inputs[0].button, 0x9000);
        assert!(snapshot.peripherals.pif.rumble_on[0]);
        assert_eq!(snapshot.vi.pending_mode.unwrap().registers, [1; 14]);
        assert_eq!(snapshot.vi.pending_x_scale_bits, Some(0.5f32.to_bits()));
        assert_eq!(snapshot.vi.active_y_scale_bits, 0.75f32.to_bits());
        assert_eq!(snapshot.pending_pi_completions.len(), 1);
        assert_eq!(snapshot.pending_pi_completions[0].rdram_len, 0x1000);
        assert_eq!(
            snapshot.pending_si_completion.unwrap().owner,
            PendingSiCompletionOwnerEvidenceSnapshot::ProcessRdram { rdram_len: 0x1000 }
        );
        with_host(|host| *host = HostState::default());
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
    }

    #[test]
    fn host_evidence_is_hash_insertion_order_and_pointer_independent() {
        let mut rdram_a = vec![0xAA; 0x2000];
        let mut rdram_b = vec![0x55; 0x2000];
        assert_ne!(rdram_a.as_mut_ptr(), rdram_b.as_mut_ptr());

        let forward = populated_host_snapshot(false, &mut rdram_a);
        let reverse = populated_host_snapshot(true, &mut rdram_b);
        assert_eq!(forward, reverse);
        assert_eq!(
            forward
                .rsp_boot_images
                .iter()
                .map(|image| image.rdram_offset)
                .collect::<Vec<_>>(),
            vec![0x100, 0x300]
        );
        assert_eq!(forward.rsp_boot_images[0].bytes, vec![1, 2, 3, 4]);
        assert_eq!(
            forward
                .thread_handles
                .iter()
                .map(|entry| entry.osthread_offset)
                .collect::<Vec<_>>(),
            vec![0x40, 0x80]
        );
        assert_eq!(
            forward
                .thread_guest_ids
                .iter()
                .map(|entry| entry.executor_thread_id)
                .collect::<Vec<_>>(),
            vec![4, 8]
        );
        assert_eq!(
            forward
                .timer_handles
                .iter()
                .map(|entry| entry.ostimer_offset)
                .collect::<Vec<_>>(),
            vec![0x140, 0x180]
        );
        assert_eq!(
            forward.registered_rdram,
            RegisteredRdramEvidenceSnapshot {
                present: true,
                byte_len: 0x2000,
            }
        );
    }

    #[test]
    fn host_evidence_covers_each_abi_owned_field_family() {
        reset_evidence_owners();
        let mut previous = host_evidence_snapshot();
        assert!(!previous.rom_installed);
        assert_eq!(previous.installed_rom, None);

        crate::load_rom_with_fixed_pi_latency(vec![0x10, 0x20, 0x30], 2);
        let mut current = host_evidence_snapshot();
        assert_ne!(current, previous);
        assert_eq!(
            current.installed_rom,
            Some(InstalledRomEvidenceSnapshot {
                byte_len: 3,
                sha256: sha2::Sha256::digest([0x10, 0x20, 0x30]).into(),
            })
        );
        previous = current;

        crate::load_rom_with_fixed_pi_latency(vec![0x10, 0x20, 0x31], 2);
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        assert_eq!(current.installed_rom.unwrap().byte_len, 3);
        previous = current;

        with_executor(|executor| {
            executor.set_controller_input(
                0,
                fn64_runtime::ContInput {
                    button: 0x8000,
                    stick_x: -3,
                    stick_y: 4,
                },
            )
        });
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        crate::set_flash_identity(crate::FlashIdentity {
            flash_type: 0x1020_3040,
            flash_maker: 0x5060_7080,
        });
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        crate::register_recompiled_section(0x2000, 0x8080_0000, 0x400);
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        with_host(|host| {
            host.rsp_boot_images.insert(0x80, vec![0xDE, 0xAD]);
        });
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        crate::set_cart_rom_handle_vram(0x8000_2000);
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        crate::set_flash_handle_vram(0x8000_2100);
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        crate::configure_leo_disk(crate::LeoDiskConfig {
            handle_vram: 0x8000_2200,
            latency: 1,
            page_size: 2,
            release: 3,
            pulse_width: 4,
        });
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        with_host(|host| {
            host.thread_handles.insert(0x40, 7);
        });
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        with_host(|host| {
            host.thread_guest_ids.insert(7, 0x77);
        });
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        with_host(|host| {
            host.timer_handles.insert(0x80, 9);
        });
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        with_host(|host| host.next_synthetic_thread_id = 0xF000_0010);
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        let mut rdram = vec![0; 0x400];
        unsafe { register_process_rdram(rdram.as_mut_ptr(), rdram.len()) };
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
        previous = current;

        crate::set_debug_hardware(crate::DebugHardware::Isv);
        current = host_evidence_snapshot();
        assert_ne!(current, previous);
    }

    #[test]
    fn host_evidence_excludes_diagnostic_and_operation_logs() {
        reset_evidence_owners();
        let before = host_evidence_snapshot();
        with_host(|host| {
            host.debug_packets.push(crate::DebugPacket {
                packet_type: 1,
                bytes: vec![1, 2, 3],
            });
            host.save_operations.push(fn64_runtime::SaveOperationEvent {
                at: Cycles::new(5),
                device: fn64_runtime::SaveType::SramBanked,
                operation: fn64_runtime::SaveOperationKind::Write,
                offset: 8,
                len: 4,
            });
            host.controller_operations
                .push(fn64_runtime::ControllerOperationEvent {
                    at: Cycles::new(6),
                    port: 1,
                    device: fn64_runtime::ControllerOperationDevice::TransferPak,
                    operation: fn64_runtime::ControllerOperationKind::Read,
                });
            host.native_execution_destinations
                .push(NativeExecutionDestinationEvent {
                    at: Cycles::new(7),
                    destination: NativeExecutionDestination {
                        section_index: 2,
                        function_offset: 0x40,
                        link_vram: 0x8080_0040,
                    },
                });
        });
        assert_eq!(host_evidence_snapshot(), before);
    }

    #[test]
    fn process_rdram_registration_updates_the_shared_runtime_owner() {
        reset_evidence_owners();
        let mut bytes = vec![0u8; 0x100];
        with_host(|host| {
            host.native_execution_destinations
                .push(NativeExecutionDestinationEvent {
                    at: Cycles::new(1),
                    destination: NativeExecutionDestination {
                        section_index: 0,
                        function_offset: 0,
                        link_vram: 0x8000_0000,
                    },
                });
        });

        unsafe { register_process_rdram(bytes.as_mut_ptr(), bytes.len()) };

        with_host(|host| {
            assert_eq!(host.runtime_rdram, bytes.as_mut_ptr());
            assert_eq!(host.runtime_rdram_len, bytes.len());
            assert!(host.native_execution_destinations.is_empty());
        });

        unsafe { register_process_rdram(bytes.as_mut_ptr(), bytes.len()) };
    }

    #[test]
    fn process_rdram_registration_rejects_a_replacement_allocation() {
        reset_evidence_owners();
        let mut first = vec![0u8; 0x100];
        let mut replacement = vec![0u8; 0x100];
        unsafe { register_process_rdram(first.as_mut_ptr(), first.len()) };

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            register_process_rdram(replacement.as_mut_ptr(), replacement.len());
        }));
        assert!(rejected.is_err());
        with_host(|host| {
            assert_eq!(host.runtime_rdram, first.as_mut_ptr());
            assert_eq!(host.runtime_rdram_len, first.len());
        });
    }

    /// Regression: `arm_fpr_alias` must make an odd-register `mtc1` (which
    /// generated C emits as `ctx->f_odd[(N-1)*2] = value`) land in the HIGH
    /// 32-bit word of register N's even partner (FR=0 lane), not fault
    /// through a null `f_odd`. This was the OoT-boot SIGSEGV-at-0x40 in
    /// `guLookAtHiliteF` (funcs_57.c:4519, `mtc1 $at, $f9`): `f_odd` was
    /// left null by `zeroed()`, so `f_odd[16]` dereferenced 0x40.
    ///
    /// Verified to fail against the bug: comment out the `arm_fpr_alias()`
    /// call below and the test segfaults on the null-pointer write (exactly
    /// the boot fault) instead of asserting.
    #[test]
    fn arm_fpr_alias_routes_odd_mtc1_to_even_partner_high_word() {
        // The context must live at a stable address for the whole test --
        // `arm_fpr_alias` stores a self-referential pointer. Box it so the
        // pointer stays valid across the field reads below.
        let mut ctx = Box::new(ctx_zeroed());
        ctx.arm_fpr_alias();

        // Distinguishable per-register sentinels so an off-by-one landing in
        // the wrong register (or the wrong 32-bit lane) is caught, not just a
        // "didn't crash" pass.
        // Generated C for `mtc1 <val>, $fN` (N odd): ctx->f_odd[(N-1)*2] = val
        let cases: &[(usize, u32)] = &[(9, 0xDEAD_0009), (7, 0xBEEF_0007), (17, 0xCAFE_0017)];
        for &(n, val) in cases {
            // Safety: f_odd was just armed to point into this context's own
            // fpr file; the index math mirrors the generated C exactly.
            unsafe {
                *ctx.f_odd.add((n - 1) * 2) = val;
            }
        }

        // $f9's bits alias f8's high word (byte 0x44 == &f0.u32h + 0x40).
        assert_eq!(unsafe { ctx.f8.u32_halves.1 }, 0xDEAD_0009, "f9 -> f8.u32h");
        // $f7 -> f6.u32h; $f17 -> f16.u32h.
        assert_eq!(unsafe { ctx.f6.u32_halves.1 }, 0xBEEF_0007, "f7 -> f6.u32h");
        assert_eq!(
            unsafe { ctx.f16.u32_halves.1 },
            0xCAFE_0017,
            "f17 -> f16.u32h"
        );

        // The LOW words of those even partners (their own $fN even-register
        // value) must be untouched -- proves the write hit the odd lane, not
        // the even register.
        assert_eq!(unsafe { ctx.f8.u32_halves.0 }, 0, "f8 low word untouched");
        assert_eq!(unsafe { ctx.f6.u32_halves.0 }, 0, "f6 low word untouched");
        assert_eq!(unsafe { ctx.f16.u32_halves.0 }, 0, "f16 low word untouched");
    }

    #[test]
    fn retrace_progress_raises_the_shared_mi_vi_source() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::test_support::install_complete_render_backend(0);
        crate::pi::set_mi_interrupt_mask(fn64_runtime::InterruptSource::Vi.bit());
        crate::vi::arm_vi_retrace(10);

        advance_virtual_time(10);
        let pending = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
        assert_ne!(pending & fn64_runtime::InterruptSource::Vi.bit(), 0);
        assert!(crate::pi::cpu_interrupt_pending());
    }

    #[test]
    fn vi_retrace_latches_state_and_mi_before_both_message_paths() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::test_support::install_complete_render_backend(0);
        let os_queue = RdramAddr::from_offset(0x100);
        let manager_queue = RdramAddr::from_offset(0x200);
        let framebuffer = RdramAddr::from_offset(0x30_0000);
        with_executor(|executor| {
            executor.create_mesg_queue(os_queue, 1);
            executor.create_mesg_queue(manager_queue, 1);
            executor.set_event_mesg(fn64_runtime::executor::OS_EVENT_VI, os_queue, 0x31);
            executor.vi_set_event(manager_queue, 0x32, 1);
            executor.vi_swap_buffer(framebuffer);
        });
        crate::vi::arm_vi_retrace(10);

        assert_eq!(advance_virtual_time(9).vi_retrace_ticks(), 0);
        assert!(!with_host(|host| host
            .device_fabric
            .interrupt_pending(fn64_runtime::InterruptSource::Vi)));
        with_executor(|executor| {
            assert_eq!(executor.vi().current_framebuffer, None);
            assert_eq!(
                executor.recv_mesg(99, os_queue, false),
                fn64_runtime::RecvMesgOutcome::WouldBlock
            );
            assert_eq!(
                executor.recv_mesg(99, manager_queue, false),
                fn64_runtime::RecvMesgOutcome::WouldBlock
            );
        });

        assert_eq!(advance_virtual_time(10).vi_retrace_ticks(), 1);
        assert!(with_host(|host| host
            .device_fabric
            .interrupt_pending(fn64_runtime::InterruptSource::Vi)));
        with_executor(|executor| {
            assert_eq!(executor.vi().current_framebuffer, Some(framebuffer));
            assert_eq!(
                executor.recv_mesg(99, os_queue, false),
                fn64_runtime::RecvMesgOutcome::Delivered(0x31)
            );
            assert_eq!(
                executor.recv_mesg(99, manager_queue, false),
                fn64_runtime::RecvMesgOutcome::Delivered(0x32)
            );
        });
    }

    #[test]
    fn virtual_time_advance_counts_every_overdue_vi_retrace() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::test_support::install_complete_render_backend(0);
        crate::vi::arm_vi_retrace(10);

        assert_eq!(advance_virtual_time(35).vi_retrace_ticks(), 3);
        assert_eq!(crate::next_vi_deadline(), Some(40));
    }
}
