//! T4 characterization/hostile tests: the three production raw-DPC ingress
//! producers (sp_dp DRAM, MMIO DRAM/XBUS, RSP-driven XBUS-shaped DMEM
//! capture through the same `dispatch_dpc_submission` seam) routed through
//! the registered `RawDpcAbiSession` plan/execute/publish conveyor.
//!
//! Every test here registers a real `fn64_render_wgpu::WgpuBackend` paired
//! with its `RawDpcAbiSession` (dev-dependency only, per Cargo.toml's own
//! comment -- fn64-abi stays backend-agnostic in production) and drives the
//! real `crate::task_dispatch::dispatch_dpc_submission` producer entry
//! point end to end against a real 8 MiB RDRAM allocation and a real
//! `DeviceFabric` admission, exactly the shape a genuine sp_dp/MMIO call
//! site reaches.

use super::*;

const SET_TEXTURE_IMAGE: u8 = 0x3d;
const SET_TILE: u8 = 0x35;
const LOAD_SYNC: u8 = 0x26;
const LOAD_BLOCK: u8 = 0x33;
const FULL_SYNC: u8 = 0x29;

fn word(opcode: u8, payload: u32) -> u32 {
    u32::from(opcode) << 24 | payload
}

fn set_texture_image(format: u32, size: u32, width: u32, address: u32) -> [u32; 2] {
    [
        word(SET_TEXTURE_IMAGE, format << 21 | size << 19 | (width - 1)),
        address,
    ]
}

fn set_tile(tile: u32, line: u32, tmem: u32) -> [u32; 2] {
    [word(SET_TILE, 2 << 19 | line << 9 | tmem), tile << 24]
}

fn load_sync() -> [u32; 2] {
    [word(LOAD_SYNC, 0), 0]
}

const TEXTURE_SOURCE_ADDR: u32 = 0x2000;

/// One admitted, TMEM-only raw-DPC command stream: SetTextureImage, SetTile,
/// LoadSync, LoadBlock -- the same admitted TMEM/state subset
/// `fn64_render_wgpu::production`'s own tests use, and the exact v11
/// TMEM-only scope this card admits.
fn one_load_block_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(set_texture_image(0, 2, 8, TEXTURE_SOURCE_ADDR));
    words.extend(set_tile(7, 2, 0));
    words.extend(load_sync());
    words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
    words
}

/// A command stream that appends a `FullSync` after an otherwise-admitted
/// TMEM load -- the shape the FullSync two-phase contract admits as a
/// decoded *site*.
fn one_load_block_then_full_sync_words() -> Vec<u32> {
    let mut words = one_load_block_words();
    words.extend([word(FULL_SYNC, 0), 0]);
    words
}

fn words_to_rdram_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_ne_bytes()).collect()
}

fn words_to_be_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_be_bytes()).collect()
}

/// A real 8 MiB RDRAM allocation, registered as the renderer's own
/// `RDRAM_LEN` (via `set_render_backend`), with `bytes` written at
/// `TEXTURE_SOURCE_ADDR` so an admitted `LoadBlock`'s source read has real,
/// non-garbage content to compare against in hostile hash-continuity tests.
fn rdram_with_texture_source() -> Vec<u8> {
    let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
    let source: Vec<u8> = (0..64u16).flat_map(u16::to_be_bytes).collect();
    let start = TEXTURE_SOURCE_ADDR as usize;
    rdram[start..start + source.len()].copy_from_slice(&source);
    rdram
}

/// Register a fresh `WgpuBackend` + paired `RawDpcAbiSession`, exactly the
/// pairing `set_raw_dpc_session`'s own doc comment describes a shell/harness
/// performing. Returns the RDRAM allocation the caller must keep alive and
/// pass a pointer into for the duration of the test.
fn register_session_backend(rdram_len: usize) {
    let (backend, session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    set_render_backend(Box::new(backend), rdram_len);
    set_raw_dpc_session(session);
}

/// Common per-test teardown: undo `crate::load_rom`/`set_render_backend`'s
/// registrations so later tests in this binary do not observe a stale
/// session or backend. Mirrors `drop_backends_for_process_exit`'s own
/// per-slot teardown, scoped to just what this module registers.
fn teardown() {
    clear_raw_dpc_session();
    RENDER_BACKEND.with(|cell| {
        cell.borrow_mut().take();
    });
    RDRAM_LEN.with(|cell| cell.set(0));
}

fn admit_dram_submission(start: u32, end: u32) -> fn64_runtime::DpcSubmission {
    with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            start,
            end,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish")
}

fn admit_dmem_submission(start: u32, end: u32) -> fn64_runtime::DpcSubmission {
    with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Dmem,
            start,
            end,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish")
}

// ---------------------------------------------------------------------
// Producer 1: sp_dp DRAM (and producer 2's DRAM half -- both reach
// `dispatch_dpc_submission`'s `Rdram` arm identically).
// ---------------------------------------------------------------------

#[test]
fn dram_producer_routes_through_the_session_when_registered() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend(rdram.len());

    let words = one_load_block_words();
    let bytes = words_to_rdram_bytes(&words);
    let start = 0x1000u32;
    let end = start + bytes.len() as u32;
    rdram[start as usize..end as usize].copy_from_slice(&bytes);

    let submission = admit_dram_submission(start, end);
    unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }

    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "a successful session-routed submission must leave no pending fabric transaction"
    );
    teardown();
}

#[test]
fn dram_producer_falls_back_to_legacy_path_when_no_session_registered() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    // Register only a plain reference backend (no session): the legacy
    // atomic `process_rdp_commands` path must still run unchanged. A
    // `WgpuBackend` registered ALONE (no session) must also still take this
    // branch -- `try_dispatch_raw_dpc_via_session` gates on the session,
    // not on the concrete backend type.
    let (backend, _unused_session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    set_render_backend(Box::new(backend), rdram.len());

    let words = one_load_block_words();
    let bytes = words_to_rdram_bytes(&words);
    let start = 0x1000u32;
    let end = start + bytes.len() as u32;
    rdram[start as usize..end as usize].copy_from_slice(&bytes);

    let submission = admit_dram_submission(start, end);
    // `WgpuBackend::process_rdp_commands` is unimplemented on the trait's
    // default (`fn64-render-wgpu` never overrides it), so the legacy path
    // panics with a named `RenderError` -- proving this call really took
    // the legacy branch, not the session branch (which would have
    // succeeded, per the sibling test above).
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }));
    assert!(
        result.is_err(),
        "no session registered must take the legacy path, which this backend cannot serve"
    );
    teardown();
}

/// End-to-end proof of the FullSync two-phase contract through the real
/// producer seam: a FullSync no longer panics, and the commit half schedules
/// the DP completion the guest is owed.
///
/// Before this contract existed the same stream panicked here (the plan
/// seam blanket-rejected `FullSync`), so a passing assertion of admission is
/// itself the regression evidence.
#[test]
fn dram_producer_admits_a_full_sync_site_and_schedules_dp_completion() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend(rdram.len());

    let words = one_load_block_then_full_sync_words();
    let bytes = words_to_rdram_bytes(&words);
    let start = 0x1000u32;
    let end = start + bytes.len() as u32;
    rdram[start as usize..end as usize].copy_from_slice(&bytes);

    // Reserve half's precondition: the DP slot starts free, and the DP
    // interrupt line starts down.
    assert!(!with_host(|host| host.device_fabric.snapshot().dp_busy));
    assert!(!with_host(|host| host
        .device_fabric
        .interrupt_pending(fn64_runtime::InterruptSource::Dp)));

    let submission = admit_dram_submission(start, end);
    unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }

    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "an admitted FullSync submission must publish, leaving no pending fabric transaction"
    );

    // Commit half ran: the DP completion is SCHEDULED (slot occupied).
    assert!(
        with_host(|host| host.device_fabric.snapshot().dp_busy),
        "admitting a FullSync site must schedule the DP completion, not swallow it"
    );

    // THE NONCLAIM, asserted at the seam. Dispatch is over -- the capture,
    // plan, execution, commit and publication have all already happened --
    // and the DP interrupt line is still DOWN. There was therefore no
    // `Asserted` state available for any boundary built during dispatch to
    // have observed, which is exactly why the producer supplies `Clear`.
    assert!(
        !with_host(|host| host
            .device_fabric
            .interrupt_pending(fn64_runtime::InterruptSource::Dp)),
        "the DP interrupt must NOT be asserted at the end of dispatch -- a boundary claiming \
         interrupt_after == Asserted here would be fabricating an edge that has not occurred"
    );

    teardown();
}

/// The two-phase contract's reserve half is a real gate, not decoration: a DP
/// completion already pending makes an incoming FullSync submission fail
/// before the backend runs, and leaves the prior pending completion intact.
#[test]
fn dram_producer_full_sync_is_rejected_when_the_dp_slot_is_already_occupied() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend(rdram.len());

    let words = one_load_block_then_full_sync_words();
    let bytes = words_to_rdram_bytes(&words);
    let start = 0x1000u32;
    let end = start + bytes.len() as u32;
    rdram[start as usize..end as usize].copy_from_slice(&bytes);

    // Occupy the sole DP completion slot first.
    crate::pi::start_live_dp_full_sync().expect("the slot starts free");
    assert!(with_host(|host| host.device_fabric.snapshot().dp_busy));

    let submission = admit_dram_submission(start, end);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }));
    assert!(
        result.is_err(),
        "a FullSync arriving while a DP completion is pending must be rejected by the reserve \
         half, not admitted"
    );
    // The rejected transaction must not leave a dangling pending fabric
    // submission behind -- `LiveDpcTransaction::drop` cancels on unwind.
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "a rejected FullSync submission must not leave the fabric transaction pending"
    );
    // The prior pending completion is untouched: the reserve half is
    // nonmutating even when it rejects.
    assert!(
        with_host(|host| host.device_fabric.snapshot().dp_busy),
        "rejecting must not consume or replace the pending DP completion"
    );
    teardown();
}

// ---------------------------------------------------------------------
// Producer 2: MMIO DRAM/XBUS -- the XBUS/DMEM half. Reaches
// `dispatch_dpc_submission`'s `Dmem` arm, same seam MMIO-triggered
// DPC_END writes with `DPC_STATUS_XBUS_DMEM_DMA` set use in production.
// ---------------------------------------------------------------------

#[test]
fn xbus_producer_routes_through_the_session_when_registered() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend(rdram.len());

    let words = one_load_block_words();
    let bytes = words_to_be_bytes(&words);
    let start = 0u32;
    let end = bytes.len() as u32;
    with_host(|host| {
        host.device_fabric
            .rsp_memory_mut()
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0),
                bytes.as_slice(),
            )
            .expect("DMEM command write must succeed")
    });

    let submission = admit_dmem_submission(start, end);
    unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }

    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "a successful session-routed XBUS submission must leave no pending fabric transaction"
    );
    teardown();
}

#[test]
fn xbus_producer_preserves_exact_source_bytes_into_the_submission_identity() {
    // Hostile: the legacy `dispatch_captured_raw_rdp` path stages a
    // synthetic RDRAM suffix; this session path must not. Proves the T4
    // path's `OwnedRawDpcSubmission::from_xbus_payload` sees the exact same
    // big-endian DMEM bytes this test wrote, by constructing the same
    // submission independently and comparing `identity()` (a SHA-256 over
    // exactly those bytes).
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend(rdram.len());

    let words = one_load_block_words();
    let bytes = words_to_be_bytes(&words);
    let start = 0u32;
    let end = bytes.len() as u32;
    with_host(|host| {
        host.device_fabric
            .rsp_memory_mut()
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0),
                bytes.as_slice(),
            )
            .expect("DMEM command write must succeed")
    });
    let expected = fn64_render::OwnedRawDpcSubmission::from_xbus_payload(start, end, bytes.clone())
        .unwrap()
        .identity();

    let submission = admit_dmem_submission(start, end);
    unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }

    let reconstructed = fn64_render::OwnedRawDpcSubmission::from_xbus_payload(start, end, bytes)
        .unwrap()
        .identity();
    assert_eq!(
        expected, reconstructed,
        "capturing the exact same DMEM bytes twice must produce the exact same submission identity"
    );
    teardown();
}

// ---------------------------------------------------------------------
// Drop/cancel/ordinal/fabric/physical joint publication -- v11's own
// admission that a rejected or abandoned submission must never partially
// publish, driven here through the real ABI producer seam rather than
// `production.rs`'s unit-level fixtures.
// ---------------------------------------------------------------------

#[test]
fn dropping_the_transaction_before_completion_cancels_and_leaves_no_pending_submission() {
    // Hostile: a submission the fabric admitted, then never driven to
    // completion (this test builds the transaction and drops it directly
    // rather than calling the producer seam), must cancel -- exactly
    // `LiveDpcTransaction::drop`'s existing contract, now proven to still
    // hold with a T4 session registered alongside it (registering a session
    // must not change what an abandoned transaction does).
    crate::load_rom(Vec::new());
    let rdram = rdram_with_texture_source();
    register_session_backend(rdram.len());

    let submission = admit_dram_submission(0x1000, 0x1008);
    let transaction = LiveDpcTransaction::new(submission);
    drop(transaction);

    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "dropping an incomplete transaction must cancel, leaving no pending submission"
    );
    teardown();
}

#[test]
fn ordinal_and_fabric_state_advance_together_only_on_successful_publication() {
    // Joint-publication characterization at the real producer seam, mirroring
    // `production.rs`'s `publish_raw_dpc_jointly_commits_physical_slot_fabric_and_published_outcome`
    // but through `dispatch_dpc_submission` rather than calling
    // plan/execute/publish directly. `fn64-abi` never downcasts a
    // registered `Box<dyn RenderBackend>` (DECOUPLING.md's backend-agnostic
    // rule), so this observes the fabric-side half of joint publication --
    // CURRENT/status -- directly, and the coordinator-side half indirectly:
    // a second, independent submission through the same registered session
    // must ALSO complete cleanly (proving the coordinator's double-buffered
    // slot flip from the first call left it in a consistent, reusable
    // state, not a stuck/poisoned one).
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend(rdram.len());

    let words = one_load_block_words();
    let bytes = words_to_rdram_bytes(&words);
    let first_start = 0x1000u32;
    let first_end = first_start + bytes.len() as u32;
    rdram[first_start as usize..first_end as usize].copy_from_slice(&bytes);

    let before_device = with_host(|host| host.device_fabric.snapshot());
    let first_submission = admit_dram_submission(first_start, first_end);
    unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), first_submission);
    }
    let after_first_device = with_host(|host| host.device_fabric.snapshot());
    assert_ne!(
        before_device, after_first_device,
        "the first published submission must advance the fabric's CURRENT/status state"
    );

    // Independent second submission (fresh SetTile/LoadSync/LoadBlock, a
    // disjoint TMEM destination and RDRAM range) through the SAME
    // registered session -- proves the coordinator's inactive/active slot
    // flip from the first publish left the backend in a reusable state.
    let second_start = 0x2000u32;
    let second_end = second_start + bytes.len() as u32;
    rdram[second_start as usize..second_end as usize].copy_from_slice(&bytes);
    let second_submission = admit_dram_submission(second_start, second_end);
    unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), second_submission);
    }
    let after_second_device = with_host(|host| host.device_fabric.snapshot());
    assert_ne!(
        after_first_device, after_second_device,
        "a second published submission through the same session must also advance fabric state"
    );
    teardown();
}

// ---------------------------------------------------------------------
// Producer 3, for real: the RSP-driven pending-loop inside
// `dispatch_lle_task` (rsp_commit.rs's `while let Some(first) =
// pending.next()` loop), not the `dispatch_dpc_submission(Dmem)` surrogate
// the xbus_producer_* tests above exercise. This drives a real, tiny RSP
// interpreter program through COP0 MTC0 writes to DPC_START/DPC_STATUS/
// DPC_END -- the actual mechanism `RspMachine::write_dp_status`/
// `take_dp_submissions` use to select and stage an XBUS-sourced DPC
// submission -- then BREAKs, exactly the call boundary this loop reaches
// in production when RSP microcode (not the CPU via sp_dp/MMIO) drives raw
// DPC output.
// ---------------------------------------------------------------------

/// `mtc0 rt, rd` (COP0 move-to, opcode 0x10, rs=0x04): the same encoding
/// `dispatch_b.rs`'s `graphics_lle_accuracy_policy_forwards_raw_dpc_without_hle_dispatch`
/// uses to drive DPC_START/DPC_END. COP0 register 8 is DPC_START, 9 is
/// DPC_END, 11 is DPC_STATUS (`fn64-audio`'s
/// `crates/fn64-audio/src/rsp/recomp/runtime/mod.rs` `write_cop0`'s match
/// arms: 8/9/11).
fn mtc0(rt: u32, rd: u32) -> u32 {
    (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11)
}

/// `addiu $rt, $0, imm` -- loads a small unsigned immediate into `rt`.
fn addiu_zero(rt: u32, imm: u32) -> u32 {
    (0x09 << 26) | (rt << 16) | (imm & 0xffff)
}

#[test]
fn rsp_driven_xbus_pending_loop_routes_through_the_session_when_registered() {
    // Real RSP DPC command bytes staged directly into DMEM at DPC_START
    // (XBUS reads command words from DMEM, not RDRAM) -- the same
    // TMEM-only admitted fixture every other producer test in this module
    // uses.
    const DPC_START: u32 = 0x100;
    let words = one_load_block_words();
    let command_bytes = words_to_be_bytes(&words);
    let dpc_end = DPC_START + command_bytes.len() as u32;

    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    let task_addr = RdramAddr::from_offset(0);

    with_host(|host| {
        host.runtime_rdram = rdram.as_mut_ptr();
        host.runtime_rdram_len = rdram.len();
        let memory = host.device_fabric.rsp_memory_mut();
        // Stage the real command bytes into DMEM at DPC_START -- this is
        // what an actual RSP graphics/audio microcode DMA into DMEM would
        // have already done before triggering CMD_END.
        memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(
                    fn64_runtime::RspMemoryBank::Dmem,
                    u16::try_from(DPC_START).unwrap(),
                ),
                &command_bytes,
            )
            .expect("DMEM command write must succeed");
        // Tiny RSP program: load DPC_START into $2, MTC0 into COP0 r8;
        // load a XBUS-select DP_STATUS command (bit 1 = set XBUS) into $3,
        // MTC0 into COP0 r11; load DPC_END into $4, MTC0 into COP0 r9
        // (this write is what stages the submission -- see
        // `RspMachine::write_cop0`'s `9 =>` arm); BREAK.
        let program = [
            addiu_zero(2, DPC_START),
            mtc0(2, 8),
            addiu_zero(3, 0b10),
            mtc0(3, 11),
            addiu_zero(4, dpc_end),
            mtc0(4, 9),
            0x0000_000d, // BREAK
        ];
        let bytes: Vec<u8> = program.into_iter().flat_map(u32::to_be_bytes).collect();
        memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &bytes,
            )
            .expect("IMEM program write must succeed");
    });
    install_running_task_lineage(task_addr, RspTaskAdmissionGeneration::first());
    register_session_backend(rdram.len());

    let result =
        unsafe { dispatch_lle_task(rdram.as_mut_ptr(), Some(task_addr), false, None, None, None) };

    assert_eq!(
        result.dp_full_sync,
        fn64_render::DpFullSyncStatus::NotReached,
        "v11 TMEM-only scope admits no FullSync command"
    );
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "the real RSP-driven pending-loop must publish through the session, leaving no pending \
         fabric submission -- proves this call reached try_dispatch_raw_dpc_via_session, not a \
         surrogate"
    );
    teardown();
}

#[test]
fn rsp_driven_xbus_pending_loop_falls_back_to_legacy_path_when_no_session_registered() {
    // Same real RSP-driven XBUS submission as the sibling test above, but
    // with no session registered: must take the legacy
    // `dispatch_captured_raw_rdp` path unchanged (proven the same way as
    // `dram_producer_falls_back_to_legacy_path_when_no_session_registered`
    // -- `WgpuBackend::process_rdp_commands` is unimplemented, so a legacy
    // dispatch through it panics).
    const DPC_START: u32 = 0x100;
    let words = one_load_block_words();
    let command_bytes = words_to_be_bytes(&words);
    let dpc_end = DPC_START + command_bytes.len() as u32;

    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    let task_addr = RdramAddr::from_offset(0);

    with_host(|host| {
        host.runtime_rdram = rdram.as_mut_ptr();
        host.runtime_rdram_len = rdram.len();
        let memory = host.device_fabric.rsp_memory_mut();
        memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(
                    fn64_runtime::RspMemoryBank::Dmem,
                    u16::try_from(DPC_START).unwrap(),
                ),
                &command_bytes,
            )
            .expect("DMEM command write must succeed");
        let program = [
            addiu_zero(2, DPC_START),
            mtc0(2, 8),
            addiu_zero(3, 0b10),
            mtc0(3, 11),
            addiu_zero(4, dpc_end),
            mtc0(4, 9),
            0x0000_000d,
        ];
        let bytes: Vec<u8> = program.into_iter().flat_map(u32::to_be_bytes).collect();
        memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &bytes,
            )
            .expect("IMEM program write must succeed");
    });
    install_running_task_lineage(task_addr, RspTaskAdmissionGeneration::first());

    let (backend, _unused_session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    set_render_backend(Box::new(backend), rdram.len());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        dispatch_lle_task(rdram.as_mut_ptr(), Some(task_addr), false, None, None, None)
    }));
    assert!(
        result.is_err(),
        "no session registered must take the legacy path through the real RSP pending-loop too"
    );
    teardown();
}

// ---------------------------------------------------------------------
// Mismatched backend/session registration -- `set_render_backend` and
// `set_raw_dpc_session` are two independent public calls (necessarily, per
// `set_raw_dpc_session`'s own doc comment: fn64-abi cannot itself construct
// a paired `(WgpuBackend, RawDpcAbiSession)` without naming a concrete
// backend type). Nothing in fn64-abi's own registration API stops a caller
// from registering a backend from one `WgpuBackend::try_new()` call
// alongside a session from a DIFFERENT, unrelated call. This must trap
// loudly before any mutation, not silently plan/execute against the wrong
// pairing -- proving `RawDpcBackendAuthority::begin_plan`'s own paired-queue
// assertion (`fn64-render/src/render_ir.rs`) is really reached through the
// full ABI producer seam, not just in fn64-render's own unit tests.
// ---------------------------------------------------------------------

#[test]
fn mismatched_backend_and_session_registration_traps_before_any_mutation() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();

    // Register backend A, but session B (from an unrelated, independently
    // constructed `WgpuBackend::try_new()` pair) -- exactly the caller bug
    // `set_raw_dpc_session`'s doc comment warns fn64-abi cannot detect on
    // its own.
    let (backend_a, _session_a) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    let (_backend_b, session_b) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    set_render_backend(Box::new(backend_a), rdram.len());
    set_raw_dpc_session(session_b);

    let words = one_load_block_words();
    let bytes = words_to_rdram_bytes(&words);
    let start = 0x1000u32;
    let end = start + bytes.len() as u32;
    rdram[start as usize..end as usize].copy_from_slice(&bytes);

    let before_device = with_host(|host| host.device_fabric.snapshot());
    let submission = admit_dram_submission(start, end);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }));
    assert!(
        result.is_err(),
        "a mismatched backend/session pairing must trap loudly, never silently plan/execute"
    );

    let after_device = with_host(|host| host.device_fabric.snapshot());
    assert_eq!(
        before_device, after_device,
        "the trap must fire before any fabric mutation -- begin_plan's assert runs before any \
         plan field is written, and the panic unwinds through LiveDpcTransaction::drop, which \
         cancels rather than commits"
    );
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "the cancelled submission must leave no pending fabric transaction behind"
    );
    teardown();
}

// ---------------------------------------------------------------------
// FillRectangle production admission: the guest-write commit branch.
//
// Every test below drives the REAL producer seam
// (`dispatch_dpc_submission` -> `try_dispatch_raw_dpc_via_session`) with
// the real `RefCell` borrow choreography, so the nonempty-writes branch is
// exercised across the crate boundary rather than simulated.
// ---------------------------------------------------------------------

const SET_OTHER_MODE: u8 = 0x2f;
const SET_COLOR_IMAGE: u8 = 0x3f;
const SET_FILL_COLOR: u8 = 0x37;
const FILL_RECTANGLE: u8 = 0x36;

/// Where every fill fixture's `SetColorImage` points. 64-byte aligned (the
/// decoder requires it) and clear of both the command stream at 0x1000 and
/// `TEXTURE_SOURCE_ADDR`.
const FILL_TARGET_ADDR: u32 = 0x4000;
const FILL_TARGET_WIDTH: u32 = 16;
const FILL_TARGET_HEIGHT: u32 = 8;
/// RGBA16: two bytes per pixel.
const FILL_TARGET_BYTES: usize = (FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT * 2) as usize;

/// `SetOtherMode` staging Fill cycle with no Z-compare/Z-update/image-read
/// bit -- the only shape `execute_fill_rectangle` admits.
fn fill_cycle_other_mode() -> [u32; 2] {
    [word(SET_OTHER_MODE, 3 << 20), 0]
}

/// `SetColorImage` staging an RGBA16 image (`format` 0, `size` 2) whose
/// wire width field is width-1.
fn set_color_image_rgba16() -> [u32; 2] {
    [
        word(SET_COLOR_IMAGE, 2 << 19 | (FILL_TARGET_WIDTH - 1)),
        FILL_TARGET_ADDR,
    ]
}

fn set_fill_color(value: u32) -> [u32; 2] {
    [word(SET_FILL_COLOR, 0), value]
}

/// One `FillRectangle` at whole-pixel coordinates (wire fields are 10.2
/// fixed point, so each is shifted left by two).
fn fill_rectangle(x0: u32, y0: u32, x1: u32, y1: u32) -> [u32; 2] {
    [
        word(FILL_RECTANGLE, ((x1 << 2) << 12) | (y1 << 2)),
        ((x0 << 2) << 12) | (y0 << 2),
    ]
}

/// A whole-target fill: the only rectangle a *fresh* color target admits.
/// A partial rectangle on a target with no predecessor is rejected
/// (`PartialNewTargetInitialization`) because its untouched rows would be
/// fabricated zeros.
fn whole_target_fill_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(fill_cycle_other_mode());
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(0x0842_1085));
    words.extend(fill_rectangle(
        0,
        0,
        FILL_TARGET_WIDTH - 1,
        FILL_TARGET_HEIGHT - 1,
    ));
    words
}

/// A partial-width, three-row fill: `x0 = 4` is nonzero, so the decoder
/// declares three disjoint, width-strided write accesses rather than one
/// collapsed range.
fn partial_width_fill_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(fill_cycle_other_mode());
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(0x213c_4d59));
    words.extend(fill_rectangle(4, 2, 14, 4));
    words
}

/// Like `register_session_backend`, but also drives `RenderBackend::create`
/// so the backend records a host-configured color-image height.
///
/// The RDP's `SetColorImage` carries no height field, so an admitted
/// `FillRectangle` is rejected outright without this. `create` is allowed to
/// fail on an adapterless host: `WgpuBackend::create_inner` records the
/// configured extent *before* it requests a device, precisely so a CPU-side
/// fill does not require a GPU.
fn register_session_backend_for_fills(rdram_len: usize) {
    let (mut backend, session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    let _ = backend.create(&fn64_render::RenderConfig {
        width: FILL_TARGET_WIDTH,
        height: FILL_TARGET_HEIGHT,
        tv_type: fn64_runtime::TvType::default(),
    });
    set_render_backend(Box::new(backend), rdram_len);
    set_raw_dpc_session(session);
}

/// Writes `words` into `rdram` at 0x1000 and dispatches them through the
/// real producer seam.
fn dispatch_words(rdram: &mut [u8], words: &[u32]) {
    let bytes = words_to_rdram_bytes(words);
    let start = 0x1000u32;
    let end = start + bytes.len() as u32;
    rdram[start as usize..end as usize].copy_from_slice(&bytes);
    let submission = admit_dram_submission(start, end);
    unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }
}

/// **T-16:** a partial-width fill routes all the way through the real
/// session seam, taking the nonempty guest-write commit branch across the
/// crate boundary with the real `RefCell` borrow sequence.
///
/// Before this task, `try_dispatch_raw_dpc_via_session` called
/// `commit_zero_guest_writes` unconditionally, so a submission declaring
/// three `RenderTarget` writes would have panicked with
/// `EffectCountMismatch`. Completing without a panic is therefore itself the
/// evidence that the branch exists and was taken.
#[test]
fn dram_producer_routes_a_partial_width_fill_through_the_session() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend_for_fills(rdram.len());

    // A fresh target admits only a whole-target rectangle; that fill also
    // exercises the single-write branch of the same commit path.
    dispatch_words(&mut rdram, &whole_target_fill_words());
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "the whole-target fill must complete, leaving no pending fabric transaction"
    );

    dispatch_words(&mut rdram, &partial_width_fill_words());
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "a session-routed partial-width fill must complete, leaving no pending transaction -- \
         reaching here at all proves the nonempty guest-write commit branch was taken, since \
         the zero-write branch would have panicked with EffectCountMismatch"
    );
    teardown();
}

/// **T-17 -- the nonclaim, made executable.** Nothing in the FillRectangle
/// admission chain writes guest RDRAM.
///
/// `execute_fill_rectangle` produces an owned `Vec<u8>`;
/// `ResidentPublication::publish` writes into a backend-local `Vec`; a
/// `CompletedWrite` is a range plus a content digest, not bytes in motion.
/// This test snapshots the guest bytes covering the fill's own declared
/// target range and asserts they are byte-identical after a successful
/// dispatch.
///
/// A future slice that adds the RDRAM copyback MUST break this test
/// deliberately. Silently changing it would turn a documented nonclaim into
/// an undocumented behavior change.
#[test]
fn guest_rdram_is_not_modified_by_an_admitted_fill() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend_for_fills(rdram.len());

    // Poison the target range with a recognizable pattern, so "unchanged"
    // is a real observation rather than "still zero".
    let target = FILL_TARGET_ADDR as usize..FILL_TARGET_ADDR as usize + FILL_TARGET_BYTES;
    for (offset, byte) in rdram[target.clone()].iter_mut().enumerate() {
        *byte = (offset as u8).wrapping_mul(7).wrapping_add(0x5a);
    }
    let before = rdram[target.clone()].to_vec();

    dispatch_words(&mut rdram, &whole_target_fill_words());
    assert_eq!(
        rdram[target.clone()],
        before[..],
        "a whole-target fill must not modify one guest RDRAM byte -- this slice publishes into \
         a backend-local buffer and has no RDRAM copyback"
    );

    dispatch_words(&mut rdram, &partial_width_fill_words());
    assert_eq!(
        rdram[target.clone()],
        before[..],
        "a partial-width fill must not modify one guest RDRAM byte either"
    );

    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "both fills completed -- the bytes are unchanged because nothing writes them, not \
         because the dispatch silently failed"
    );
    teardown();
}

/// **T-18:** the pre-existing TMEM-only path is undisturbed by Phase 4. A
/// TMEM-only capture stages no guest render-target write, so it still takes
/// the zero-write commit branch and still completes.
#[test]
fn tmem_only_captures_still_take_the_zero_write_branch() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend_for_fills(rdram.len());

    dispatch_words(&mut rdram, &one_load_block_words());
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "a TMEM-only submission must still complete through the zero-write branch"
    );
    teardown();
}

/// The split-arm regression proof, updated for the FullSync two-phase
/// contract. Task A split `FillRectangle` and `FullSync` out of one shared
/// rejection arm; this task admits the `FullSync` side as a site. The
/// invariant that survives both changes is that the two arms remain
/// INDEPENDENT: a fill followed by a FullSync must exercise both admissions,
/// with the fill's guest-visible write and the FullSync's DP completion each
/// happening exactly once and neither aliasing the other.
#[test]
fn a_fill_followed_by_a_full_sync_admits_both_independently() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend_for_fills(rdram.len());

    let mut words = whole_target_fill_words();
    words.extend([word(FULL_SYNC, 0), 0]);
    let bytes = words_to_rdram_bytes(&words);
    let start = 0x1000u32;
    let end = start + bytes.len() as u32;
    rdram[start as usize..end as usize].copy_from_slice(&bytes);

    assert!(!with_host(|host| host.device_fabric.snapshot().dp_busy));

    let submission = admit_dram_submission(start, end);
    unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }

    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "a fill-plus-FullSync submission must publish, leaving no pending fabric transaction"
    );
    // The FullSync side took its own admission: the DP completion is
    // scheduled. If the two arms had re-merged, admitting the fill would
    // have consumed the FullSync and left this slot free.
    assert!(
        with_host(|host| host.device_fabric.snapshot().dp_busy),
        "the FullSync arm must schedule the DP completion independently of the fill arm"
    );
    // Same nonclaim as the DRAM-only sibling: dispatch is over and the DP
    // interrupt line is still down.
    assert!(
        !with_host(|host| host
            .device_fabric
            .interrupt_pending(fn64_runtime::InterruptSource::Dp)),
        "admitting a FullSync site must not assert the DP interrupt during dispatch"
    );
    teardown();
}

// ---------------------------------------------------------------------
// What the production seam actually PRODUCES, in pixels.
//
// Every sibling test above asserts that a dispatch *completed* -- no
// pending fabric transaction, the right commit branch taken, guest RDRAM
// untouched. None of them asserts the resident color-target content the
// fill published, because `set_render_backend` takes a
// `Box<dyn RenderBackend>` and `fn64-abi` never downcasts it
// (`lifecycle.rs`'s `apply_render_runtime_settings` doc: "The backend
// remains owned here; callers do not downcast it", and DECOUPLING.md's
// backend-agnostic rule). So "the dispatch succeeded" and "the dispatch
// produced the right bytes" were, until this test, two different claims
// with only the first one proven.
//
// The observation seam is a delegating backend holding a shared handle to
// the real `WgpuBackend`, registered in its place. It adds no behavior:
// every method forwards. That keeps the production plan/execute/publish
// conveyor and the paired `RawDpcAbiSession` authority exactly as they
// are -- the session is still the one `WgpuBackend::try_new` split off,
// so `RawDpcBackendAuthority::begin_plan`'s paired-queue assertion still
// gates this path -- while leaving the test a second reference through
// which to read `color_targets()` after dispatch returns.
// ---------------------------------------------------------------------

/// A `RenderBackend` that is exactly its inner `WgpuBackend`, reached
/// through a shared handle so a test can still observe the backend after
/// `set_render_backend` has taken ownership of the box.
///
/// Delegation is total and unconditional: there is no interception, no
/// recording, and no fallback arm. Anything this type could add would make
/// the thing under test something other than `WgpuBackend`.
struct ObservingBackend {
    inner: std::rc::Rc<std::cell::RefCell<fn64_render_wgpu::WgpuBackend>>,
}

impl fn64_render::RenderBackend for ObservingBackend {
    fn create(&mut self, cfg: &fn64_render::RenderConfig) -> Result<(), fn64_render::RenderError> {
        self.inner.borrow_mut().create(cfg)
    }

    fn observe_non_rdp_write16(
        &mut self,
        write: fn64_render::NonRdpWrite16,
    ) -> fn64_render::NonRdpWrite16Disposition {
        self.inner.borrow_mut().observe_non_rdp_write16(write)
    }

    fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &fn64_render::OsTask,
        output_addr: u32,
    ) -> Result<fn64_render::FrameStatus, fn64_render::RenderError> {
        self.inner
            .borrow_mut()
            .process_task(rdram, rsp_memory, task, output_addr)
    }

    fn present(
        &mut self,
        request: fn64_render::PresentRequest<'_>,
    ) -> Result<(), fn64_render::RenderError> {
        self.inner.borrow_mut().present(request)
    }

    fn resize(&mut self, w: u32, h: u32) {
        self.inner.borrow_mut().resize(w, h);
    }

    fn supported_ucodes(&self) -> &[fn64_render::UcodeId] {
        // The inner backend's own answer is the empty slice
        // (`production.rs`'s `supported_ucodes`), which is `'static` and so
        // can be returned through this borrow-free signature. A backend
        // returning a non-'static slice could not be wrapped this way; that
        // limitation is this seam's, not the production path's, and no
        // raw-DPC dispatch consults this method.
        &[]
    }

    fn raw_dpc_ir_capability(&self) -> fn64_render::RawDpcIrCapability {
        self.inner.borrow().raw_dpc_ir_capability()
    }

    fn plan_raw_dpc(
        &mut self,
        request: fn64_render::RawDpcPlanRequest,
    ) -> Result<fn64_render::PlannedRawDpcSubmission, fn64_render::RenderError> {
        self.inner.borrow_mut().plan_raw_dpc(request)
    }

    fn execute_raw_dpc(
        &mut self,
        bound: fn64_render::BoundSubmittedRawDpc,
    ) -> Result<fn64_render::BackendPreparedRawDpc, fn64_render::RenderError> {
        self.inner.borrow_mut().execute_raw_dpc(bound)
    }

    fn staged_guest_render_target_writes(
        &mut self,
        submission: fn64_render::ir::SubmissionIdentity,
    ) -> Vec<fn64_render::ir::CompletedWrite> {
        self.inner
            .borrow_mut()
            .staged_guest_render_target_writes(submission)
    }

    fn publish_raw_dpc(
        &mut self,
        publication: fn64_render::ReadyRawDpcCommitCapsule<'_>,
    ) -> fn64_render::CommittedRawDpcOutcome {
        self.inner.borrow_mut().publish_raw_dpc(publication)
    }
}

/// Register an `ObservingBackend` over a real `WgpuBackend`, configured for
/// fills exactly as `register_session_backend_for_fills` configures its
/// own. Returns the shared handle the test reads published content through.
fn register_observed_session_backend_for_fills(
    rdram_len: usize,
) -> std::rc::Rc<std::cell::RefCell<fn64_render_wgpu::WgpuBackend>> {
    let (mut backend, session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    // Allowed to fail on an adapterless host, exactly as
    // `register_session_backend_for_fills` documents: `create_inner`
    // records the configured extent BEFORE it requests a device, and the
    // fill path this test measures is entirely CPU-side.
    let _ = backend.create(&fn64_render::RenderConfig {
        width: FILL_TARGET_WIDTH,
        height: FILL_TARGET_HEIGHT,
        tv_type: fn64_runtime::TvType::default(),
    });
    let inner = std::rc::Rc::new(std::cell::RefCell::new(backend));
    set_render_backend(
        Box::new(ObservingBackend {
            inner: std::rc::Rc::clone(&inner),
        }),
        rdram_len,
    );
    set_raw_dpc_session(session);
    inner
}

/// The RGBA16 halfword an RDP fill-cycle writes at target column `x`, hand-
/// derived from the fill-color word rather than read back from the port.
///
/// Two independent derivations, reconciled by the caller (§3.2 of
/// `docs/RT64-PORT-CARD-BRIEF.md`: never assert a derived value one way
/// only). This is derivation 1 -- the direct RDP semantics:
///
/// A `SetFillColor` word holds TWO packed RGBA5551 halfwords when the color
/// image is 16-bit. The RDP emits the HIGH halfword at even target columns
/// and the LOW halfword at odd ones (`decode_fill_cycle_pixel`'s "period 2"
/// for RGBA16). The halfword is then stored big-endian.
///
/// The port takes a detour this derivation does not: it expands each 5-bit
/// channel to 8 bits (`expand_five`: `v << 3 | v >> 2`), then repacks by
/// truncating back (`>> 3`). That round trip is the IDENTITY on 5 bits --
/// `((v << 3 | v >> 2) >> 3) == v` for all v in 0..32, because the low two
/// bits `v >> 2` occupy positions below the `>> 3` truncation. The 1-bit
/// alpha does the same through 0/255 and `>> 7`. So the emitted halfword is
/// the source halfword unchanged, and this function need not model the
/// expansion at all. `expanded_round_trip_is_the_identity_on_five_bits`
/// below proves that exhaustively rather than by assertion.
fn expected_fill_halfword(fill_color: u32, x: u32) -> u16 {
    if x % 2 == 0 {
        (fill_color >> 16) as u16
    } else {
        fill_color as u16
    }
}

/// Derivation 2, independent of `expected_fill_halfword`: model the port's
/// own expand-then-truncate path literally, channel by channel, and let the
/// test reconcile the two. If either derivation is wrong they disagree, and
/// the disagreement is the finding -- §3.2's discipline, and the same shape
/// that caught `08c10916`'s one-nibble mask error by construction.
fn expected_fill_halfword_via_expansion(fill_color: u32, x: u32) -> u16 {
    let halfword = if x % 2 == 0 {
        (fill_color >> 16) as u16
    } else {
        fill_color as u16
    };
    let expand_five = |value: u8| -> u8 { (value << 3) | (value >> 2) };
    let red = expand_five(((halfword >> 11) & 0x1f) as u8);
    let green = expand_five(((halfword >> 6) & 0x1f) as u8);
    let blue = expand_five(((halfword >> 1) & 0x1f) as u8);
    let alpha: u8 = if halfword & 1 != 0 { 255 } else { 0 };
    (u16::from(red >> 3) << 11)
        | (u16::from(green >> 3) << 6)
        | (u16::from(blue >> 3) << 1)
        | u16::from(alpha >> 7)
}

/// The exhaustive proof `expected_fill_halfword`'s doc leans on: the port's
/// 5-bit expand-then-truncate round trip is the identity, so the simple
/// derivation and the expansion-modelling one cannot diverge for any input.
///
/// Proven over all 2^16 halfwords at both column parities -- not spot-
/// checked. §3.4's lesson: an arbitrary witness would have supported
/// collapsing these two helpers into one, which is exactly what must not
/// happen.
#[test]
fn expanded_round_trip_is_the_identity_on_five_bits() {
    for raw in 0..=u16::MAX {
        let fill_color = (u32::from(raw) << 16) | u32::from(raw);
        for x in [0u32, 1] {
            assert_eq!(
                expected_fill_halfword(fill_color, x),
                expected_fill_halfword_via_expansion(fill_color, x),
                "the two independent derivations must agree for halfword {raw:#06x} at x={x}"
            );
        }
    }
}

/// **The measurement this module was missing.** A whole-target
/// `FillRectangle` driven through the real `dispatch_dpc_submission`
/// producer seam, with a real `WgpuBackend` registered as the live
/// `RenderBackend`, publishes a resident color target whose every byte
/// matches a hand-derived RDP fill-cycle expectation.
///
/// The expectation is built from `SET_FILL_COLOR`'s own word and the RGBA16
/// even/odd column rule, not captured from a run. `FILL_COLOR` is chosen so
/// its two packed halfwords DIFFER (`0x0842` vs `0x1085`): a port that
/// ignored column parity, or that used the wrong half, would produce a
/// uniform image and fail here. A single-halfword fill color could not
/// distinguish those cases.
#[test]
fn a_dispatched_fill_publishes_the_hand_derived_rgba16_target_content() {
    const FILL_COLOR: u32 = 0x0842_1085;

    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    let backend = register_observed_session_backend_for_fills(rdram.len());

    // No target is resident before the dispatch: whatever this test reads
    // afterwards was produced by the dispatch, not left over from setup.
    assert!(
        backend.borrow().color_targets().is_none(),
        "no color target may exist before the first admitted fill"
    );

    dispatch_words(&mut rdram, &whole_target_fill_words());
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "the fill must publish, leaving no pending fabric transaction"
    );

    let expected: Vec<u8> = (0..FILL_TARGET_HEIGHT)
        .flat_map(|_| {
            (0..FILL_TARGET_WIDTH)
                .flat_map(|x| expected_fill_halfword(FILL_COLOR, x).to_be_bytes())
                .collect::<Vec<u8>>()
        })
        .collect();
    assert_eq!(
        expected.len(),
        FILL_TARGET_BYTES,
        "the hand-derived image must cover the whole 16x8 RGBA16 target"
    );

    let handle = backend.borrow();
    let registry = handle
        .color_targets()
        .expect("an admitted whole-target fill must have built the color-target registry");
    let residents = registry.residents();
    assert_eq!(
        residents.len(),
        1,
        "exactly one color target -- the fill's own SetColorImage address -- may be resident"
    );
    let resident = &residents[0];
    assert_eq!(
        resident.key().address().get(),
        FILL_TARGET_ADDR,
        "the resident must be keyed to the SetColorImage address the command stream named"
    );
    assert_eq!(
        resident.generation(),
        fn64_render_wgpu::TargetGeneration::FIRST,
        "a first publication must be generation FIRST, not an advanced one"
    );
    assert_eq!(
        resident.device_bytes().device_bytes(),
        expected.as_slice(),
        "every published byte must equal the hand-derived RDP fill-cycle image"
    );

    // The two halfwords really are distinct in the output, so the assertion
    // above discriminated column parity rather than passing on a uniform
    // image that any parity rule would produce.
    assert_eq!(&expected[0..2], &0x0842u16.to_be_bytes(), "even column");
    assert_eq!(&expected[2..4], &0x1085u16.to_be_bytes(), "odd column");

    drop(handle);
    teardown();
}

/// The second half of the same measurement: a partial-width fill over an
/// already-resident target advances the generation and rewrites EXACTLY the
/// rectangle it claimed, leaving every byte outside it at the previous
/// generation's content.
///
/// The rectangle is `x` in 4..=14, `y` in 2..=4 -- deliberately not row- or
/// column-aligned to the target, so a port that collapsed three strided row
/// accesses into one contiguous range would overwrite the untouched
/// columns and fail here.
#[test]
fn a_dispatched_partial_fill_rewrites_exactly_its_own_rectangle() {
    const FIRST_FILL_COLOR: u32 = 0x0842_1085;
    const SECOND_FILL_COLOR: u32 = 0x213c_4d59;
    // Mirrors `partial_width_fill_words`'s own `fill_rectangle(4, 2, 14, 4)`.
    const X0: u32 = 4;
    const X1: u32 = 14;
    const Y0: u32 = 2;
    const Y1: u32 = 4;

    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    let backend = register_observed_session_backend_for_fills(rdram.len());

    dispatch_words(&mut rdram, &whole_target_fill_words());
    dispatch_words(&mut rdram, &partial_width_fill_words());
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "both fills must publish, leaving no pending fabric transaction"
    );

    // Hand-derived: start from the whole-target image, then overwrite only
    // the claimed rectangle's pixels with the second fill's color, using
    // the same TARGET-relative (not rectangle-relative) column parity the
    // RDP applies -- `decode_fill_cycle_pixel`'s `x` is the target column.
    let mut expected: Vec<u8> = (0..FILL_TARGET_HEIGHT)
        .flat_map(|_| {
            (0..FILL_TARGET_WIDTH)
                .flat_map(|x| expected_fill_halfword(FIRST_FILL_COLOR, x).to_be_bytes())
                .collect::<Vec<u8>>()
        })
        .collect();
    for y in Y0..=Y1 {
        for x in X0..=X1 {
            let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
            expected[offset..offset + 2]
                .copy_from_slice(&expected_fill_halfword(SECOND_FILL_COLOR, x).to_be_bytes());
        }
    }

    let handle = backend.borrow();
    let residents = handle
        .color_targets()
        .expect("the registry must exist after two admitted fills")
        .residents();
    assert_eq!(residents.len(), 1, "both fills target the same address");
    let resident = &residents[0];
    assert_eq!(
        resident.generation().get(),
        fn64_render_wgpu::TargetGeneration::FIRST.get() + 1,
        "a second publication to the same key must advance exactly one generation"
    );
    assert_eq!(
        resident.device_bytes().device_bytes(),
        expected.as_slice(),
        "the partial fill must rewrite exactly its rectangle and preserve every other byte"
    );

    // Row 1 is entirely outside the rectangle and must still carry the
    // first fill's image -- the discriminator against a collapsed range.
    let row1 = (FILL_TARGET_WIDTH * 2) as usize;
    assert_eq!(
        &resident.device_bytes().device_bytes()[row1..row1 + 4],
        &expected[row1..row1 + 4],
        "row 1 is outside the rectangle and must be untouched"
    );
    // Column 0 of row 2 is inside the rectangle's rows but left of x0, so a
    // width-collapsed write would have clobbered it.
    let row2 = (FILL_TARGET_WIDTH * 2 * 2) as usize;
    assert_eq!(
        &resident.device_bytes().device_bytes()[row2..row2 + 2],
        &expected_fill_halfword(FIRST_FILL_COLOR, 0).to_be_bytes(),
        "column 0 of row 2 is left of the rectangle and must keep the first fill's pixel"
    );

    drop(handle);
    teardown();
}

/// A fill whose left edge is at an ODD target column, which the two tests
/// above cannot reach.
///
/// This exists because a mutation exposed a hole in their reach, not because
/// it looked thorough. Replacing `execute_fill_rectangle`'s
/// `target_x0 = row.first_pixel() % extent.width()` with a literal `0` --
/// i.e. decoding the fill color at RECTANGLE-relative rather than
/// TARGET-relative columns, the exact confusion `decode_fill_cycle_pixel`'s
/// doc warns about ("target-relative, not rectangle-relative") -- survived
/// both of them. It survived because their rectangles start at x0 = 0 and
/// x0 = 4, and RGBA16 fill-cycle decoding has period 2: 0 and 4 have the
/// same parity, so the correct and mutated column indices select the same
/// halfword at every pixel. The mutant was equivalent for those fixtures
/// only, never in general.
///
/// x0 = 5 has odd parity, so target-relative and rectangle-relative
/// decoding disagree at every pixel in the rectangle, and the mutant dies.
fn odd_origin_fill_words() -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(fill_cycle_other_mode());
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(0x213c_4d59));
    words.extend(fill_rectangle(5, 1, 11, 3));
    words
}

#[test]
fn a_dispatched_odd_origin_fill_decodes_at_target_relative_columns() {
    const FIRST_FILL_COLOR: u32 = 0x0842_1085;
    const SECOND_FILL_COLOR: u32 = 0x213c_4d59;
    // Mirrors `odd_origin_fill_words`'s own `fill_rectangle(5, 1, 11, 3)`.
    const X0: u32 = 5;
    const X1: u32 = 11;
    const Y0: u32 = 1;
    const Y1: u32 = 3;

    assert_eq!(X0 % 2, 1, "the whole point of this fixture is an odd x0");

    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    let backend = register_observed_session_backend_for_fills(rdram.len());

    dispatch_words(&mut rdram, &whole_target_fill_words());
    dispatch_words(&mut rdram, &odd_origin_fill_words());
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "both fills must publish, leaving no pending fabric transaction"
    );

    let mut expected: Vec<u8> = (0..FILL_TARGET_HEIGHT)
        .flat_map(|_| {
            (0..FILL_TARGET_WIDTH)
                .flat_map(|x| expected_fill_halfword(FIRST_FILL_COLOR, x).to_be_bytes())
                .collect::<Vec<u8>>()
        })
        .collect();
    for y in Y0..=Y1 {
        for x in X0..=X1 {
            let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
            expected[offset..offset + 2]
                .copy_from_slice(&expected_fill_halfword(SECOND_FILL_COLOR, x).to_be_bytes());
        }
    }

    let handle = backend.borrow();
    let residents = handle
        .color_targets()
        .expect("the registry must exist after two admitted fills")
        .residents();
    let published = residents[0].device_bytes().device_bytes();
    assert_eq!(
        published,
        expected.as_slice(),
        "an odd-origin fill must decode its fill color at TARGET-relative columns"
    );

    // The discriminator, stated positively: the rectangle's FIRST pixel sits
    // at an odd target column, so it must carry the LOW halfword of the fill
    // color. Rectangle-relative decoding would have called it column 0 and
    // written the HIGH halfword there.
    let first = ((Y0 * FILL_TARGET_WIDTH + X0) * 2) as usize;
    assert_eq!(
        &published[first..first + 2],
        &(SECOND_FILL_COLOR as u16).to_be_bytes(),
        "the rectangle's first pixel is at an odd target column and takes the low halfword"
    );
    assert_ne!(
        &published[first..first + 2],
        &((SECOND_FILL_COLOR >> 16) as u16).to_be_bytes(),
        "the two halfwords must differ here, or this assertion could not discriminate"
    );

    drop(handle);
    teardown();
}
