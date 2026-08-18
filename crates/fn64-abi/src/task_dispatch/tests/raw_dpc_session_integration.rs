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

const TEXRECT: u8 = 0x24;

/// One `TextureRectangle` at whole-pixel coordinates (wire fields are 10.2
/// fixed point, so each is shifted left by two), sampling `tile`.
///
/// `dsdx`/`dtdy` are S5.10 texel-per-pixel steps; `0x0400` is exactly one
/// texel per pixel (1024/1024). `uls`/`ult` are S10.5 texture-space origins.
fn texture_rectangle(
    tile: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    uls: u32,
    ult: u32,
    dsdx: u32,
    dtdy: u32,
) -> [u32; 4] {
    [
        word(TEXRECT, ((x1 << 2) << 12) | (y1 << 2)),
        (tile & 0x7) << 24 | ((x0 << 2) << 12) | (y0 << 2),
        (uls << 16) | ult,
        (dsdx << 16) | dtdy,
    ]
}

/// A whole-target fill followed by a `TextureRectangle` covering pixels
/// x 4..=10, y 2..=3 of the same staged color image.
///
/// This is the WM2000-title-screen *shape* -- `G_FILLRECT` plus `G_TEXRECT`
/// into one color image, zero triangles -- reduced to the smallest packet
/// that carries both.
fn fill_then_texrect_words() -> Vec<u32> {
    let mut words = whole_target_fill_words();
    words.extend(texture_rectangle(0, 4, 2, 10, 3, 0, 0, 0x0400, 0x0400));
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

/// Poison `rdram`'s whole fill-target range with a recognizable,
/// offset-dependent pattern, and return the poisoned image.
///
/// Offset-dependent, not a constant byte: a copy that wrote the right span
/// with the wrong *contents* would still be caught, and a surviving byte can
/// be attributed to its own offset. The multiplier is odd so the pattern has
/// period 256 rather than aliasing with the 2-byte pixel stride.
///
/// Written and returned in **logical guest byte order**, through
/// `RdramViewMut::write_logical_bytes` -- the same byte-lane authority
/// `copy_committed_guest_writes` now uses. Every fill expectation in this
/// file is a guest-order image, so a raw-indexed poison would compare two
/// different address spaces and report a lane mapping as a content
/// difference.
fn poison_fill_target(rdram: &mut [u8]) -> Vec<u8> {
    let poison: Vec<u8> = (0..FILL_TARGET_BYTES)
        .map(|offset| (offset as u8).wrapping_mul(7).wrapping_add(0x5a))
        .collect();
    fn64_runtime::RdramViewMut::from_storage(rdram).write_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(FILL_TARGET_ADDR),
        &poison,
    );
    poison
}

/// Read the fill target back out of `rdram` in **logical guest byte order**,
/// through `RdramView`'s `^3` lane mapping.
///
/// This is the readback counterpart to `poison_fill_target`, and it is what
/// makes these tests independent evidence rather than a restatement of the
/// copyback: they assert the guest-visible image, derived from the RDP's own
/// rules, and the storage mapping is applied by `fn64-runtime`'s single
/// authority on both sides rather than open-coded here.
fn read_fill_target_logical(rdram: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; FILL_TARGET_BYTES];
    fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(FILL_TARGET_ADDR),
        &mut out,
    );
    out
}

/// The hand-derived RGBA16 image a whole-target fill of `fill_color`
/// produces, built from the RDP's even/odd column rule rather than captured
/// from a run.
fn expected_whole_target_image(fill_color: u32) -> Vec<u8> {
    (0..FILL_TARGET_HEIGHT)
        .flat_map(|_| {
            (0..FILL_TARGET_WIDTH)
                .flat_map(|x| expected_fill_halfword(fill_color, x).to_be_bytes())
                .collect::<Vec<u8>>()
        })
        .collect()
}

/// **T-17's successor. The nonclaim is retired: an admitted fill now DOES
/// write guest RDRAM.**
///
/// T-17 (`guest_rdram_is_not_modified_by_an_admitted_fill`) asserted the
/// opposite, and it was true when written: `execute_fill_rectangle` produced
/// an owned `Vec<u8>`, `ResidentPublication::publish` wrote into a
/// backend-local `Vec`, and a `CompletedWrite` was a range plus a content
/// digest with no bytes in motion. Its own doc required that a future slice
/// adding the RDRAM copyback break it DELIBERATELY rather than silently
/// edit it, so this test replaces it under a new name asserting the new
/// behavior, and this paragraph is the record of the supersession.
///
/// What changed is only the transport, not the verification: the copy lives
/// in `rsp_commit.rs`'s `copy_committed_guest_writes`, runs strictly AFTER
/// `commit_guest_render_target_writes` returned `Ok`, and re-derives each
/// committed `ContentDigest` from the bytes before writing any of them.
/// `CompletedWrite` still carries no bytes.
///
/// The expectation is hand-derived from `SET_FILL_COLOR`'s own word and the
/// RGBA16 even/odd column rule, never captured from the code under test.
/// A fill composed with a `TextureRectangle` reaches the real
/// `dispatch_dpc_submission` seam and is refused **by name**, with guest
/// RDRAM left byte-for-byte untouched.
///
/// This is the end-to-end half of this card's measurement, and it is a
/// *refusal* test on purpose. The decoder half landed: a texrect now
/// declares its own `ColorFramebuffer` write range in the resource journal
/// (proven in `fn64-render-wgpu`'s
/// `a_texture_rectangle_declares_a_render_target_write_access`, which can
/// read the journal directly). The executor half did not: nothing yet
/// produces the texel *content* for that declared range on the CPU, so
/// `stage_and_report` still refuses fill-plus-triangle before staging
/// anything -- and a texrect is two triangles by the time it gets there.
///
/// What this pins is that the refusal is total: a packet the backend cannot
/// execute must change no guest byte at all. A texrect that published
/// plausible pixels without a proven texel fetch is the one outcome this
/// card was told is unacceptable, so the absence of any write is the
/// property worth a test.
///
/// # The rectangle, hand-derived twice
///
/// `fill_then_texrect_words`' texrect is `x0=4, y0=2, x1=10, y1=3` in whole
/// pixels -- 10.2 wire fields `ulx=16, uly=8, lrx=40, lry=12`. The staged
/// `OtherMode` is fill cycle, so RT64's `drawRect` rounds the UL corner down
/// (`&= ~3`; both are already multiples of 4, so unchanged) and
/// `FixedRect::left/top/right/bottom(ceil=true)` gives `(coord + 3) >> 2 =
/// 4, 2, 10, 3`. Half-open, so the covered pixels are **x 4..=9, y 2..=2**.
/// Derivation 2: `ceil(coord / 4)` on the same four values gives the same
/// `4, 2, 10, 3`.
///
/// The rectangle is therefore NARROWER than its wire corners read -- x1=10
/// and y1=3 do not become inclusive pixels 10 and 3. That is exactly why the
/// planner takes its extent from `texture_rectangle_vertices` (the ported
/// RT64 geometry) rather than re-deriving it from the wire fields.
#[test]
fn a_fill_composed_with_a_texture_rectangle_changes_no_guest_byte() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend_for_fills(rdram.len());

    let target = FILL_TARGET_ADDR as usize..FILL_TARGET_ADDR as usize + FILL_TARGET_BYTES;
    let poisoned = poison_fill_target(&mut rdram);

    // Positive control, in this same test: the fill ALONE does change these
    // bytes through this same seam. Without this, "the composed packet wrote
    // nothing" could equally mean the harness never wrote anything.
    dispatch_words(&mut rdram, &whole_target_fill_words());
    assert_ne!(
        rdram[target.clone()],
        poisoned[..],
        "the fill alone must change guest bytes through this seam, or the composed case below \
         proves nothing about refusal"
    );

    // Re-poison, then dispatch the composed packet. The refusal reaches the
    // guest as a PANIC at the ABI seam (`rsp_commit`'s `execute_raw_dpc`
    // unwrap), not as a returned error, so it is caught here rather than
    // observed as a return value -- catching it is also what lets the RDRAM
    // assertion below run at all.
    let poisoned = poison_fill_target(&mut rdram);
    let words = fill_then_texrect_words();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dispatch_words(&mut rdram, &words);
    }));
    let payload = outcome.expect_err(
        "a fill composed with a texture rectangle must still be refused -- the decoder now \
         declares the texrect's write, but no CPU executor produces its texel content",
    );
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.contains(
            "declares both an admitted FillRectangle and at least one admitted \
                          triangle"
        ),
        "the refusal must be the named mixed-packet rejection, got: {message}"
    );

    assert_eq!(
        rdram[target],
        poisoned[..],
        "a refused fill+texrect packet must change no guest byte -- every one must still carry \
         the poison. A partial write here would mean the fill half published while the texrect \
         half silently vanished, which is exactly the 'plausible pixels without a proven texel \
         fetch' outcome this card must not produce"
    );
    teardown();
}

#[test]
fn an_admitted_whole_target_fill_writes_its_image_into_guest_rdram() {
    const FILL_COLOR: u32 = 0x0842_1085;

    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend_for_fills(rdram.len());

    let poisoned = poison_fill_target(&mut rdram);
    let expected = expected_whole_target_image(FILL_COLOR);
    assert_ne!(
        expected, poisoned,
        "the poison must differ from the expected image, or 'the bytes changed' would be \
         unfalsifiable"
    );

    dispatch_words(&mut rdram, &whole_target_fill_words());
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "the fill must complete -- the bytes below changed because the copyback ran, not \
         because the dispatch failed halfway"
    );

    let observed = read_fill_target_logical(&rdram);
    assert_eq!(
        observed, expected,
        "every guest byte of the target must now equal the hand-derived RDP fill-cycle image"
    );

    // The two packed halfwords really are distinct in guest memory, so the
    // assertion above discriminated column parity rather than passing on a
    // uniform image any parity rule would produce.
    //
    // Read through `RdramView::read_u16`'s `^2` lane XOR -- the SAME accessor
    // family the VI's `PhysicalRdramRead::read_u16` uses. A raw index would
    // no longer name these bytes, and that is the defect this file's e2e
    // test now proves fixed.
    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(FILL_TARGET_ADDR)),
        0x0842,
        "even column, read back through the lane-mapped halfword accessor"
    );
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(FILL_TARGET_ADDR + 2)),
        0x1085,
        "odd column, read back through the lane-mapped halfword accessor"
    );
    teardown();
}

/// The over-wide-copy test. A partial-width fill declares N **disjoint**
/// per-row RDRAM ranges, strided by the color image's width, and the
/// copyback must write those N spans and nothing between them.
///
/// Measured, not assumed: `fn64-render-wgpu`'s `raw_dpc::plan_fill`
/// collapses a fill to ONE access only when `x0 == 0 && x1 + 1 == width`.
/// `fill_rectangle(4, 2, 14, 4)` satisfies neither, so it declares three
/// accesses -- one per scanline. A copy that collapsed them into a single
/// `[first_start, last_end)` span would cover 3 * 16 - 5 = 43 pixels instead
/// of 3 * 11 = 33, claiming ~30% more bytes than the fill wrote and
/// clobbering the poison at columns 0..4 and 15 of rows 2..=4.
///
/// Those surviving poison bytes are the assertion that catches it: they are
/// checked against the ORIGINAL poison, not against a whole-target
/// expectation, so nothing but an exactly-sized copy can pass.
#[test]
fn an_admitted_partial_width_fill_writes_only_its_own_disjoint_rows() {
    const FILL_COLOR: u32 = 0x213c_4d59;
    // Mirrors `partial_width_fill_words`'s own `fill_rectangle(4, 2, 14, 4)`.
    const X0: u32 = 4;
    const X1: u32 = 14;
    const Y0: u32 = 2;
    const Y1: u32 = 4;

    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend_for_fills(rdram.len());

    // A fresh target admits only a whole-target rectangle, so the partial
    // fill needs a resident predecessor. Poison AFTER it, so the bytes the
    // partial fill must leave alone are the poison and not the first fill's
    // image -- the first fill's own copyback is the sibling test's claim,
    // and reusing it here would make "untouched" ambiguous between the two.
    dispatch_words(&mut rdram, &whole_target_fill_words());
    let poisoned = poison_fill_target(&mut rdram);

    dispatch_words(&mut rdram, &partial_width_fill_words());
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "the partial fill must complete"
    );

    // Hand-derived: the poison everywhere, overwritten ONLY inside the
    // claimed rectangle, at TARGET-relative column parity
    // (`decode_fill_cycle_pixel`'s `x` is the target column, not the
    // rectangle-relative one).
    let mut expected = poisoned.clone();
    for y in Y0..=Y1 {
        for x in X0..=X1 {
            let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
            expected[offset..offset + 2]
                .copy_from_slice(&expected_fill_halfword(FILL_COLOR, x).to_be_bytes());
        }
    }

    let observed = read_fill_target_logical(&rdram);
    assert_eq!(
        observed, expected,
        "the partial fill must write exactly its three disjoint rows and leave every other \
         guest byte at its poisoned value"
    );

    // The discriminators, named individually so a failure says which shape
    // of over-copy happened rather than just 'bytes differ'.
    //
    // Row 1 is entirely above the rectangle: a row-off-by-one that started
    // at y0 - 1 would clobber it.
    let row1 = (FILL_TARGET_WIDTH * 2) as usize;
    assert_eq!(
        &observed[row1..row1 + (FILL_TARGET_WIDTH * 2) as usize],
        &poisoned[(FILL_TARGET_WIDTH * 2) as usize..(FILL_TARGET_WIDTH * 4) as usize],
        "row 1 is above the rectangle and must be entirely poison"
    );
    // Row 5 is entirely below it: a row-off-by-one that ran to y1 + 1 would
    // clobber this instead.
    let row5 = (FILL_TARGET_WIDTH * 2 * 5) as usize;
    assert_eq!(
        &observed[row5..row5 + (FILL_TARGET_WIDTH * 2) as usize],
        &poisoned[(FILL_TARGET_WIDTH * 2 * 5) as usize..(FILL_TARGET_WIDTH * 2 * 6) as usize],
        "row 5 is below the rectangle and must be entirely poison"
    );
    // Columns 0..4 of row 2 are left of x0, and column 15 is right of x1.
    // Both lie INSIDE a collapsed [first_start, last_end) span, so they are
    // exactly what a width-collapsed copy destroys.
    let row2 = (FILL_TARGET_WIDTH * 2 * 2) as usize;
    assert_eq!(
        &observed[row2..row2 + (X0 * 2) as usize],
        &poisoned
            [(FILL_TARGET_WIDTH * 2 * 2) as usize..(FILL_TARGET_WIDTH * 2 * 2 + X0 * 2) as usize],
        "columns 0..4 of row 2 are left of x0 and must still be poison -- a collapsed \
         single-range copy would have overwritten them"
    );
    let row2_last = row2 + ((FILL_TARGET_WIDTH - 1) * 2) as usize;
    assert_eq!(
        &observed[row2_last..row2_last + 2],
        &poisoned[(FILL_TARGET_WIDTH * 2 * 2 + (FILL_TARGET_WIDTH - 1) * 2) as usize
            ..(FILL_TARGET_WIDTH * 2 * 2 + FILL_TARGET_WIDTH * 2) as usize],
        "column 15 of row 2 is right of x1 and must still be poison"
    );
    // And the rectangle itself really was written, so 'everything is poison'
    // cannot pass this test.
    let inside = row2 + (X0 * 2) as usize;
    assert_eq!(
        &observed[inside..inside + 2],
        &expected_fill_halfword(FILL_COLOR, X0).to_be_bytes(),
        "the rectangle's own first pixel must carry the second fill's color"
    );
    assert_ne!(
        &observed[inside..inside + 2],
        &poisoned[(FILL_TARGET_WIDTH * 2 * 2 + X0 * 2) as usize
            ..(FILL_TARGET_WIDTH * 2 * 2 + X0 * 2 + 2) as usize],
        "that pixel must differ from the poison, or the whole test is vacuous"
    );
    teardown();
}

/// The odd-origin case, in guest RDRAM. RGBA16 fill decoding has period 2,
/// so an even-origin-only fixture lets a rectangle-relative vs.
/// target-relative column confusion survive -- exactly the mutant
/// `a_dispatched_odd_origin_fill_decodes_at_target_relative_columns`
/// documents surviving the x0 = 0 and x0 = 4 fixtures.
///
/// `odd_origin_fill_words`'s x0 = 5 has odd parity, so the two decodings
/// disagree at every pixel in the rectangle. This test makes that
/// discrimination hold at the RDRAM copyback too, not only in the
/// backend-local buffer the sibling test reads.
#[test]
fn an_admitted_odd_origin_fill_writes_target_relative_columns_into_guest_rdram() {
    const FILL_COLOR: u32 = 0x213c_4d59;
    // Mirrors `odd_origin_fill_words`'s own `fill_rectangle(5, 1, 11, 3)`.
    const X0: u32 = 5;
    const X1: u32 = 11;
    const Y0: u32 = 1;
    const Y1: u32 = 3;

    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend_for_fills(rdram.len());

    dispatch_words(&mut rdram, &whole_target_fill_words());
    let poisoned = poison_fill_target(&mut rdram);

    dispatch_words(&mut rdram, &odd_origin_fill_words());
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "the odd-origin fill must complete"
    );

    let mut expected = poisoned.clone();
    for y in Y0..=Y1 {
        for x in X0..=X1 {
            let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
            expected[offset..offset + 2]
                .copy_from_slice(&expected_fill_halfword(FILL_COLOR, x).to_be_bytes());
        }
    }

    let observed = read_fill_target_logical(&rdram);
    assert_eq!(
        observed, expected,
        "an odd-origin fill must write target-relative column parity into guest RDRAM"
    );

    // The parity discrimination, made explicit: x0 = 5 is odd, so the
    // rectangle's first pixel takes the LOW halfword. A rectangle-relative
    // decoding would have treated it as column 0 and written the HIGH one.
    let first = ((Y0 * FILL_TARGET_WIDTH + X0) * 2) as usize;
    assert_eq!(
        &observed[first..first + 2],
        &(FILL_COLOR as u16).to_be_bytes(),
        "target column 5 is odd, so the low packed halfword must land here"
    );
    assert_ne!(
        (FILL_COLOR >> 16) as u16,
        FILL_COLOR as u16,
        "the fill color's two halfwords must differ, or the parity assertion is vacuous"
    );
    teardown();
}

thread_local! {
    /// Every `GuestWriteEvent` the copyback's write barrier publishes during
    /// `an_admitted_fill_reaches_the_write_barrier_journal`.
    static OBSERVED_COPYBACK_WRITES: std::cell::RefCell<Vec<fn64_cpu_runtime::GuestWriteEvent>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn record_copyback_write(event: fn64_cpu_runtime::GuestWriteEvent) {
    OBSERVED_COPYBACK_WRITES.with(|events| events.borrow_mut().push(event));
}

/// The copyback's writes reach the write-barrier journal, attributed to the
/// `RdpRenderer` channel.
///
/// **Written to kill a measured surviving mutant.** Deleting
/// `track_rdp_renderer_mutation` from `copy_committed_guest_writes` and
/// calling `write_logical_bytes` directly left the whole workspace green
/// (8211/8211) -- the barrier was load-bearing by argument only, with no test
/// behind it, both before and after the byte-lane fix. Every other fill test
/// in this file reads final RDRAM, which the bare write satisfies identically.
///
/// So this test observes the journal instead of the bytes: it declares the
/// fill target as a watched executable range and asserts the copyback
/// publishes a `RdpRenderer` write over it. A bare `write_logical_bytes`
/// notifies nobody and this test fails with an empty event list.
///
/// The range is watched deliberately. `track_catalog_nested_mutation` only
/// diffs and notifies bytes inside watched ranges, so an unwatched fill
/// target would produce no event even with the barrier intact -- which is
/// why the ordinary fill tests could never have caught this.
#[cfg(feature = "recomp-rs")]
#[test]
fn an_admitted_fill_reaches_the_write_barrier_journal() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend_for_fills(rdram.len());

    let _preflight = crate::recompiled::scoped_test_executable_write_preflight_state(
        vec![(
            FILL_TARGET_ADDR,
            FILL_TARGET_ADDR + FILL_TARGET_BYTES as u32,
        )],
        Vec::new(),
    );
    // Poison first: the tracker notifies CHANGED bytes, so a target already
    // equal to the fill image would publish nothing and pass vacuously.
    let poisoned = poison_fill_target(&mut rdram);

    OBSERVED_COPYBACK_WRITES.with(|events| events.borrow_mut().clear());
    let previous = fn64_cpu_runtime::set_write_observer(Some(record_copyback_write));
    dispatch_words(&mut rdram, &whole_target_fill_words());
    fn64_cpu_runtime::set_write_observer(previous);

    let observed = read_fill_target_logical(&rdram);
    assert_ne!(
        observed, poisoned,
        "the fill must have changed bytes, or there would be nothing for the barrier to \
         report and this test would pass vacuously"
    );

    let events = OBSERVED_COPYBACK_WRITES.with(|events| events.borrow().clone());
    let renderer: Vec<_> = events
        .iter()
        .filter(|event| event.channel() == fn64_cpu_runtime::WriterChannel::RdpRenderer)
        .collect();
    assert!(
        !renderer.is_empty(),
        "the copyback must publish its guest-visible writes to the write-barrier journal on \
         the RdpRenderer channel -- dropping track_rdp_renderer_mutation makes this list \
         empty while every byte-comparing test in this file still passes. Observed: {events:?}"
    );
    // The reported bytes must be the fill target's, not some unrelated
    // writer's: an event list that happened to carry another channel's
    // range would otherwise satisfy the assertion above.
    for event in &renderer {
        let fn64_cpu_runtime::GuestWriteEvent::Range {
            physical_offset,
            len,
            ..
        } = event
        else {
            panic!("the copyback publishes byte ranges, got {event:?}");
        };
        assert!(
            *physical_offset >= FILL_TARGET_ADDR
                && physical_offset + len <= FILL_TARGET_ADDR + FILL_TARGET_BYTES as u32,
            "a reported renderer write [{physical_offset:#x}, +{len:#x}) must lie inside the \
             fill target [{FILL_TARGET_ADDR:#x}, +{FILL_TARGET_BYTES:#x})"
        );
    }
    teardown();
}

/// A TMEM-only submission takes the zero-write branch, so the copyback must
/// not run at all -- the nonclaim that SURVIVES the T-17 supersession.
///
/// `copy_committed_guest_writes` is called only when the committed write
/// list is nonempty. This proves that gate empirically rather than by
/// reading the `if`: a TMEM-only dispatch over poisoned target bytes must
/// leave every one of them alone.
#[test]
fn a_tmem_only_submission_writes_no_guest_target_byte() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend_for_fills(rdram.len());

    let poisoned = poison_fill_target(&mut rdram);
    dispatch_words(&mut rdram, &one_load_block_words());
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "the TMEM-only submission must complete"
    );

    assert_eq!(
        read_fill_target_logical(&rdram),
        poisoned,
        "a TMEM-only submission stages no guest render-target write, so the copyback must \
         not run and not one target byte may change"
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

    // Forwarded like every other method. Omitting it does NOT silently
    // degrade: the trait default returns an empty byte list, and
    // `copy_committed_guest_writes` then fails loudly with a
    // committed-writes/payload count mismatch -- which is how this method's
    // absence was found in the first place, rather than by review.
    fn committed_guest_render_target_bytes(
        &mut self,
        submission: fn64_render::ir::SubmissionIdentity,
    ) -> Vec<Vec<u8>> {
        self.inner
            .borrow_mut()
            .committed_guest_render_target_bytes(submission)
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

// ---------------------------------------------------------------------
// The copyback's ORDERING, made observable.
//
// "The copy happens after the commit, never before" is unfalsifiable on a
// fixture whose commit always succeeds: both orderings produce identical
// RDRAM. Moving the copy above `commit_guest_render_target_writes` survived
// every test above for exactly that reason -- a mutation finding, not a
// review one.
//
// The discriminator is a submission whose commit REJECTS. A backend that
// over-reports its staged write list (the same list duplicated) is caught by
// `GuestCommitEffectReport::try_new` against the packet's own guest-write
// journal, and `try_dispatch_raw_dpc_via_session` panics on that rejection
// rather than swallowing it. Under the correct ordering no guest byte has
// been written when that panic unwinds; under the reversed one the copy has
// already run. The poison is therefore the proof.
// ---------------------------------------------------------------------

/// Delegates everything to a real `WgpuBackend`, except that it reports each
/// staged guest write TWICE.
///
/// The duplicated list is what the commit sees, so the commit rejects. The
/// byte transport is left honest and un-duplicated: this fixture is about
/// the copy's position relative to a failing commit, not about a
/// bytes/ranges disagreement (which `M5`/the digest check already covers).
struct OverReportingBackend {
    inner: std::rc::Rc<std::cell::RefCell<fn64_render_wgpu::WgpuBackend>>,
}

impl fn64_render::RenderBackend for OverReportingBackend {
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

    /// The single lie: each staged write is reported twice.
    fn staged_guest_render_target_writes(
        &mut self,
        submission: fn64_render::ir::SubmissionIdentity,
    ) -> Vec<fn64_render::ir::CompletedWrite> {
        let honest = self
            .inner
            .borrow_mut()
            .staged_guest_render_target_writes(submission);
        honest.iter().chain(honest.iter()).copied().collect()
    }

    fn committed_guest_render_target_bytes(
        &mut self,
        submission: fn64_render::ir::SubmissionIdentity,
    ) -> Vec<Vec<u8>> {
        let honest = self
            .inner
            .borrow_mut()
            .committed_guest_render_target_bytes(submission);
        honest.iter().chain(honest.iter()).cloned().collect()
    }

    fn publish_raw_dpc(
        &mut self,
        publication: fn64_render::ReadyRawDpcCommitCapsule<'_>,
    ) -> fn64_render::CommittedRawDpcOutcome {
        self.inner.borrow_mut().publish_raw_dpc(publication)
    }
}

/// A submission whose guest commit REJECTS must leave guest RDRAM untouched.
///
/// This is the ordering proof for `copy_committed_guest_writes`: it runs
/// strictly after `commit_guest_render_target_writes` returns `Ok`, so a
/// commit that panics means no byte was copied. Reversing the two -- copying
/// first, committing second -- makes this test fail, which is the whole
/// reason it exists.
///
/// Nonclaim: this asserts ordering, not recovery. The dispatch panics either
/// way; what differs is whether guest memory was already modified when it
/// did.
#[test]
fn a_rejected_guest_commit_leaves_guest_rdram_untouched() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();

    let (mut backend, session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    let _ = backend.create(&fn64_render::RenderConfig {
        width: FILL_TARGET_WIDTH,
        height: FILL_TARGET_HEIGHT,
        tv_type: fn64_runtime::TvType::default(),
    });
    let inner = std::rc::Rc::new(std::cell::RefCell::new(backend));
    set_render_backend(
        Box::new(OverReportingBackend {
            inner: std::rc::Rc::clone(&inner),
        }),
        rdram.len(),
    );
    set_raw_dpc_session(session);

    let poisoned = poison_fill_target(&mut rdram);

    // The dispatch must panic: the duplicated write list fails against the
    // packet's own guest-write journal, and that rejection is not caught.
    let words = whole_target_fill_words();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dispatch_words(&mut rdram, &words);
    }));
    assert!(
        outcome.is_err(),
        "an over-reported guest-write list must be rejected loudly by the commit, not accepted"
    );

    assert_eq!(
        read_fill_target_logical(&rdram),
        poisoned,
        "the commit rejected, so the copyback must never have run -- every target byte must \
         still be poison. A copy placed BEFORE the commit fails here."
    );
    teardown();
}

// ---------------------------------------------------------------------
// Shell-selectability: what a `FN64_RENDER=wgpu` arm in `fn64-shell` would
// actually meet at runtime.
//
// `crates/fn64-shell/src/main.rs`'s backend selection registers whatever
// `Box<dyn RenderBackend>` it built through `set_render_backend`, and the
// shell then runs the real guest loop. That loop reaches
// `RenderBackend::present` on EVERY VI retrace, through exactly one call
// site (`crate::pi::timing`'s retrace drain -> `present_render_backend`),
// with no backend-capability check in between. The two tests below measure
// that seam directly rather than reasoning about it: the first proves the
// raw-DPC half a shell arm exists to reach really is reachable once both
// halves are registered, and the second proves the SAME registration is
// killed by the very next VI field.
//
// **This block's conclusion changed, and the change is recorded here rather
// than by editing the old sentence away.** It previously read: "Together
// they are the evidence for why no `FN64_RENDER=wgpu` arm ships: it would be
// selectable and immediately fatal." That was true while
// `WgpuBackend::present` was a named "presentation is out of scope"
// rejection. It now scans guest RDRAM out through `fn64-render-wgpu`'s
// `vi_scanout`, so the second test asserts survival rather than death (see
// `a_registered_wgpu_backend_survives_the_first_vi_present`, which records
// the supersession).
//
// What is still NOT claimed: that a shell arm is *complete*. The scanout
// implements AA mode 3 (replicate) over RGBA16/RGBA32 and refuses every
// other VI filter by name, so a title programming silhouette AA, divot,
// gamma, or bilinear resampling still reaches a loud trap --
// `an_unimplemented_vi_filter_still_panics_the_production_retrace_path`
// measures exactly that, and it is the honest boundary of the claim.
// ---------------------------------------------------------------------

/// The exact two-call registration a shell arm would perform -- backend
/// first (so the paired `RawDpcBackendAuthority` is already the registered
/// backend's, per `set_raw_dpc_session`'s own doc comment), session second
/// -- proving `try_dispatch_raw_dpc_via_session` no longer takes its
/// `if !registered { return None; }` early return.
#[test]
fn shell_shaped_registration_reaches_the_raw_dpc_session_seam() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();

    // Before ANY registration the seam is unreachable: this is the state
    // `fn64-shell` is in today, with no `FN64_RENDER=wgpu` arm.
    assert!(
        RAW_DPC_SESSION.with(|cell| cell.borrow().is_none()),
        "no session may be registered before the shell-shaped setup runs"
    );

    // The two public calls, in the documented order.
    let (backend, session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    set_render_backend(Box::new(backend), rdram.len());
    set_raw_dpc_session(session);

    assert!(
        RAW_DPC_SESSION.with(|cell| cell.borrow().is_some()),
        "set_raw_dpc_session must leave a session registered -- this is the exact predicate \
         try_dispatch_raw_dpc_via_session's early return reads"
    );

    // Drive the real producer seam. A TMEM-only LoadBlock is inside
    // `WgpuBackend`'s admitted set, so reaching the session path returns
    // normally; taking the legacy `process_rdp_commands` path instead would
    // panic (`WgpuBackend` leaves that trait method unimplemented), which is
    // what makes this assertion discriminating rather than vacuous.
    let words = one_load_block_words();
    let bytes = words_to_rdram_bytes(&words);
    let start = 0x1000u32;
    let end = start + bytes.len() as u32;
    rdram[start as usize..end as usize].copy_from_slice(&bytes);
    let submission = admit_dram_submission(start, end);
    unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }

    // Reaching here at all is the claim: the production raw-DPC conveyor
    // ran through the registered `WgpuBackend`.
    assert!(
        crate::last_render_error().is_none(),
        "the admitted TMEM submission must have routed through the session conveyor cleanly"
    );
    teardown();
}

/// **The tripwire, flipped. Its predecessor asserted the opposite, and was
/// correct when written.**
///
/// `a_registered_wgpu_backend_panics_on_the_first_vi_present` asserted that
/// `present_render_backend` panics with `WgpuBackend`'s "presentation is out
/// of scope" rejection, and its own message said "if this ever passes, the
/// present blocker is gone and a `FN64_RENDER=wgpu` shell arm becomes
/// shippable". `WgpuBackend::present` now scans guest RDRAM out through
/// `crate::vi_scanout`, so that assertion is false and this test replaces it
/// under a new name asserting the new behavior. This paragraph is the record
/// of the supersession; the old test is not silently edited.
///
/// Nothing about the guard changed: `with_render_backend` still panics on
/// any backend error (that contract is load-bearing and was deliberately not
/// weakened). What changed is that the backend no longer produces one.
///
/// This drives the **production** caller, not `present()` directly -- the
/// panic was in the caller, so the caller is what has to be exercised.
#[test]
fn a_registered_wgpu_backend_survives_the_first_vi_present() {
    crate::load_rom(Vec::new());
    let rdram = rdram_with_texture_source();
    let (backend, session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    set_render_backend(Box::new(backend), rdram.len());
    set_raw_dpc_session(session);

    // The host must own the same allocation `present_render_backend` reads
    // through `with_host`, exactly as the boot contract arranges.
    let mut rdram = rdram;
    with_host(|host| {
        host.runtime_rdram = rdram.as_mut_ptr();
        host.runtime_rdram_len = rdram.len();
    });

    let presentation = ntsc_replicate_presentation(0, 320, 320, 240);
    // No `catch_unwind`: a panic here fails the test directly, which is the
    // stronger statement. `with_render_backend` would turn any backend error
    // into exactly that panic.
    crate::task_dispatch::present_render_backend(presentation);

    assert!(
        crate::last_render_error().is_none(),
        "a successful present must clear the recorded render error"
    );
    teardown();
}

/// A complete live VI register image of the shape the retrace drain supplies
/// from the real register file: RGBA16 (`pixel type 2`), AA mode 3
/// (replicate), progressive, unit scale on both axes.
///
/// AA mode 3 is chosen deliberately and is not a convenience: it is the one
/// mode `crate::vi_scanout` implements, and every other mode is refused *by
/// name*. A fixture programming AA mode 0 would be testing the refusal, not
/// the scanout.
fn ntsc_replicate_presentation(
    origin: u32,
    width: u32,
    output_width: u32,
    output_height: u32,
) -> fn64_render::ViPresentation {
    let mut words = [0u32; fn64_render::ViScanoutRegisters::WORD_COUNT];
    // pixel type 2 (RGBA16) | AA mode 3 (replicate, no resampling).
    words[0] = 2 | (3 << 8);
    words[1] = origin;
    words[2] = width;
    // H_START: start 0, end `output_width` pixels.
    words[9] = output_width;
    // V_START: start 0, end `output_height` output lines = 2x half-lines.
    words[10] = output_height * 2;
    words[12] = u32::from(fn64_render::ViScaleAxis::ONE);
    words[13] = u32::from(fn64_render::ViScaleAxis::ONE);
    fn64_render::ViPresentation {
        blanked: false,
        fade: None,
        repeat_line: false,
        scanout: fn64_render::ViScanoutState::Registers(
            fn64_render::ViScanoutRegisters::from_words(words),
        ),
        noise_seed: 0,
    }
}

/// Transparently delegates every `RenderBackend` method to a shared
/// `WgpuBackend`, telling no lies at all.
///
/// This exists for one reason: `present_render_backend` reaches the backend
/// through `RENDER_BACKEND`'s `Box<dyn RenderBackend>`, and the trait has no
/// downcast seam, so a test that drives the **production** path cannot
/// otherwise read the presented field back out. Holding the same backend
/// through an `Rc<RefCell<_>>` gives the test a handle to the very object
/// the production path just called. Same mechanism as
/// `OverReportingBackend`, minus its single deliberate lie.
struct SharedWgpuBackend {
    inner: std::rc::Rc<std::cell::RefCell<fn64_render_wgpu::WgpuBackend>>,
}

impl fn64_render::RenderBackend for SharedWgpuBackend {
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

    fn committed_guest_render_target_bytes(
        &mut self,
        submission: fn64_render::ir::SubmissionIdentity,
    ) -> Vec<Vec<u8>> {
        self.inner
            .borrow_mut()
            .committed_guest_render_target_bytes(submission)
    }

    fn publish_raw_dpc(
        &mut self,
        publication: fn64_render::ReadyRawDpcCommitCapsule<'_>,
    ) -> fn64_render::CommittedRawDpcOutcome {
        self.inner.borrow_mut().publish_raw_dpc(publication)
    }
}

/// **The end-to-end proof.** Dispatch an admitted `FillRectangle` through the
/// real producer seam, then drive the real VI retrace presentation call, and
/// assert the presented pixels against the fill content -- hand-derived from
/// `SET_FILL_COLOR`'s own word, never captured from a run.
///
/// This is the whole claim in one test: the raw-DPC lane writes guest RDRAM,
/// and the VI lane scans that same guest RDRAM back out. Neither half is
/// mocked.
///
/// **Recorded supersession: the byte-lane defect this test was written to
/// pin is FIXED, and its two inverted assertions are inverted back.**
///
/// The predecessor form of this test (same name, at `a9fe65ae`) asserted the
/// presented image was the fill's with **both** an adjacent-column swap and
/// a per-halfword byte reversal, AND asserted that the agreeing-conventions
/// image was WRONG. Both were true when written and both are now false. Its
/// own doc required that fixing the writeback break it loudly rather than
/// silently redefine correct; this paragraph is that break, recorded, in the
/// same convention as the T-17 supersession above.
///
/// What was wrong, and what fixed it: `copy_committed_guest_writes`
/// `copy_from_slice`d the backend's logical guest-order payload into the raw
/// native-word allocation with **no byte-lane mapping**, while the VI reads
/// the same memory through `PhysicalRdramRead::read_u16`'s `^2` lane XOR.
/// The copy now goes through `RdramViewMut::write_logical_bytes`, the same
/// `fn64-runtime` authority the reference backend's own RDP writeback uses
/// (`crates/fn64-render-reference/src/backend/framebuffer_io.rs:188`'s
/// `view.write_u16`) and the one `vi_scanout.rs`'s "Byte-lane authority"
/// section names. The lane-mapped convention was the established one and the
/// raw copy was the outlier; that direction was verified before acting on
/// it, not assumed.
///
/// So the two conventions now AGREE, and this test asserts the fill's own
/// halfword at each column with no swap and no reversal. The transformation
/// the predecessor asserted is now asserted to be WRONG, so a regression to
/// the raw copy breaks this test just as loudly in the other direction.
#[test]
fn an_admitted_fill_presents_through_the_real_vi_retrace_path() {
    const FILL_COLOR: u32 = 0x0842_1085;
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();

    let (mut backend, session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    let _ = backend.create(&fn64_render::RenderConfig {
        width: FILL_TARGET_WIDTH,
        height: FILL_TARGET_HEIGHT,
        tv_type: fn64_runtime::TvType::default(),
    });
    let inner = std::rc::Rc::new(std::cell::RefCell::new(backend));
    set_render_backend(
        Box::new(SharedWgpuBackend {
            inner: std::rc::Rc::clone(&inner),
        }),
        rdram.len(),
    );
    set_raw_dpc_session(session);

    // Poison the target first, so a present that read the wrong address or
    // scanned out a fabricated image cannot accidentally match.
    let poisoned = poison_fill_target(&mut rdram);
    dispatch_words(&mut rdram, &whole_target_fill_words());

    // Half 1: the fill really reached guest RDRAM. Hand-derived, and the
    // exact assertion `an_admitted_fill_writes_guest_rdram` already makes.
    let in_memory = read_fill_target_logical(&rdram);
    assert_eq!(
        in_memory,
        expected_whole_target_image(FILL_COLOR),
        "the admitted fill must have written the whole target"
    );
    assert_ne!(
        in_memory, poisoned,
        "the fill must have displaced the poison"
    );

    // Half 2: the production retrace path presents that same memory.
    with_host(|host| {
        host.runtime_rdram = rdram.as_mut_ptr();
        host.runtime_rdram_len = rdram.len();
    });
    let presentation = ntsc_replicate_presentation(
        FILL_TARGET_ADDR,
        FILL_TARGET_WIDTH,
        FILL_TARGET_WIDTH,
        FILL_TARGET_HEIGHT,
    );
    crate::task_dispatch::present_render_backend(presentation);
    assert!(
        crate::last_render_error().is_none(),
        "presenting the filled target must not raise a backend error"
    );

    let borrowed = inner.borrow();
    let field = borrowed
        .presented_field()
        .expect("a successful present must retain its field");
    assert_eq!(
        (field.width, field.height),
        (FILL_TARGET_WIDTH, FILL_TARGET_HEIGHT),
        "the presented field must be exactly the guest-programmed active output rectangle"
    );

    // The hand-derived expectation, built from the fill's own halfwords and
    // the VI's five-bit expansion. `expected_fill_halfword` and
    // `expected_fill_halfword_via_expansion` are reconciled per source
    // column, as they are everywhere else in this file.
    let expand_five = |value: u8| -> u8 { (value << 3) | (value >> 2) };
    let expected_pixel = |halfword: u16| -> [u8; 4] {
        [
            expand_five(((halfword >> 11) & 0x1f) as u8),
            expand_five(((halfword >> 6) & 0x1f) as u8),
            expand_five(((halfword >> 1) & 0x1f) as u8),
            255,
        ]
    };

    // The derivation, end to end, now that the conventions agree. Both are
    // `fn64-runtime`'s one mapping, so they compose to the identity on a
    // guest-order payload:
    //
    //   write_logical_bytes stores column `x`'s halfword `h` big-endian in
    //     LOGICAL space -> logical[2x] = h >> 8, logical[2x+1] = h & 0xff,
    //     landing at storage[(2x) ^ 3] and storage[(2x+1) ^ 3]
    //   read_u16(2x) reads a native (little-endian) halfword at (2x) ^ 2
    //     -> storage[2x ^ 2] | (storage[(2x ^ 2) + 1] << 8)
    //
    // `2x` is even, so `(2x ^ 2) + 1 == (2x + 1) ^ 2`, and un-mapping each
    // byte through `^3` gives low = logical[2x ^ 1] = logical[2x + 1] and
    // high = logical[2x]. That is a big-endian read of the two bytes the
    // fill wrote there: exactly `h`, at column `x`. No column swap, no byte
    // reversal.
    //
    // Worked witness at column 0: the even-column fill halfword is `0x0842`,
    // which expands to `[8, 8, 8, 255]` -- the value this assertion now
    // observes, and precisely the "agreeing conventions" image the
    // predecessor form of this test asserted was WRONG.
    let presented_halfword = |x: u32| -> u16 {
        let stored = expected_fill_halfword(FILL_COLOR, x);
        assert_eq!(
            stored,
            expected_fill_halfword_via_expansion(FILL_COLOR, x),
            "the two independent fill-halfword derivations must agree at column {x}"
        );
        stored
    };

    // Pin the worked witness before the sweep, so a failure says which half
    // of the derivation broke rather than only that pixels differ.
    assert_eq!(expected_fill_halfword(FILL_COLOR, 0), 0x0842);
    assert_eq!(expected_fill_halfword(FILL_COLOR, 1), 0x1085);
    assert_eq!(expected_pixel(0x0842), [8, 8, 8, 255]);

    for y in 0..FILL_TARGET_HEIGHT {
        for x in 0..FILL_TARGET_WIDTH {
            assert_eq!(
                field.pixel(x, y).unwrap(),
                expected_pixel(presented_halfword(x)),
                "presented pixel ({x}, {y}) must be column {x}'s own fill halfword, with no \
                 column swap and no byte reversal -- copy_committed_guest_writes \
                 (write_logical_bytes, ^3) and PhysicalRdramRead::read_u16 (^2) are the same \
                 fn64-runtime authority and now agree"
            );
        }
    }

    // The predecessor's transformation is asserted to be WRONG, so a
    // regression to the raw `copy_from_slice` breaks this test loudly rather
    // than silently redefining correct -- the same guard the predecessor
    // carried, pointing the other way.
    let column_swapped_and_reversed =
        expected_pixel(expected_fill_halfword(FILL_COLOR, 0 ^ 1).swap_bytes());
    assert_eq!(column_swapped_and_reversed, [132, 165, 66, 255]);
    assert_ne!(
        field.pixel(0, 0).unwrap(),
        column_swapped_and_reversed,
        "if this ever passes, the byte-lane defect came back: the copyback stopped applying \
         the lane mapping the VI reads through"
    );

    drop(borrowed);
    teardown();
}

/// `PresentMemory::BackendResidentCompatibility` is refused with a reason
/// that names *which* variant and *why* -- never a generic "out of scope".
///
/// This is a direct `present()` call on purpose: the production retrace path
/// only ever constructs `PresentRequest::live`, so the resident variant has
/// no production caller to drive it through.
#[test]
fn a_backend_resident_present_is_refused_by_name_not_generically() {
    use fn64_render::RenderBackend as _;
    let (mut backend, _session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    let error = backend
        .present(fn64_render::PresentRequest::backend_resident(
            fn64_render::ViPresentation::default(),
        ))
        .expect_err("WgpuBackend retains no resident image to present");
    let fn64_render::RenderError::Backend {
        backend: name,
        reason,
    } = &error
    else {
        panic!("expected a named backend refusal, got {error:?}");
    };
    assert_eq!(*name, "render-wgpu");
    assert!(
        reason.contains("BackendResidentCompatibility"),
        "the refusal must name the unsupported variant, got: {reason}"
    );
    assert!(
        reason.contains("retains no resident scanout image"),
        "the refusal must say why, got: {reason}"
    );
    assert!(
        reason.contains("PresentRequest::live"),
        "the refusal must name the variant that does work, got: {reason}"
    );
    assert!(
        !reason.contains("out of scope"),
        "the generic rejection this replaced must not come back, got: {reason}"
    );
}

/// A VI filter `WgpuBackend` does not implement still panics the production
/// path -- and that is correct, not a regression.
///
/// `with_render_backend`'s panic-on-error contract is load-bearing and was
/// deliberately NOT weakened to make presentation pass. This test is the
/// proof that the guard is intact: a programmed filter the scanout cannot
/// produce is a loud trap naming the filter, exactly as AGENTS.md's
/// loud-trap rule requires, rather than a silently unfiltered field.
#[test]
fn an_unimplemented_vi_filter_still_panics_the_production_retrace_path() {
    crate::load_rom(Vec::new());
    let rdram = rdram_with_texture_source();
    let (backend, session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    set_render_backend(Box::new(backend), rdram.len());
    set_raw_dpc_session(session);
    let mut rdram = rdram;
    with_host(|host| {
        host.runtime_rdram = rdram.as_mut_ptr();
        host.runtime_rdram_len = rdram.len();
    });

    // The same fixture, with AA mode 0 (coverage silhouette AA) programmed
    // instead of mode 3.
    let mut presentation = ntsc_replicate_presentation(0, 320, 320, 240);
    let fn64_render::ViScanoutState::Registers(registers) = presentation.scanout else {
        unreachable!("the fixture builds a live register image");
    };
    let mut words = registers.words();
    words[0] &= !(3 << 8);
    presentation.scanout =
        fn64_render::ViScanoutState::Registers(fn64_render::ViScanoutRegisters::from_words(words));

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::task_dispatch::present_render_backend(presentation);
    }));
    assert!(
        outcome.is_err(),
        "an unimplemented VI filter must be a loud trap, not a silently unfiltered field"
    );
    let reason = crate::last_render_error().expect("the failed present must be recorded");
    assert!(
        reason.contains("coverage silhouette antialiasing"),
        "the trap must name the filter it could not produce, got: {reason}"
    );
    assert!(
        !reason.contains("out of scope"),
        "the refusal must be specific, got: {reason}"
    );
    teardown();
}

// ---------------------------------------------------------------------
// Composed fill + TMEM in one packet.
//
// The census (`docs/RT64-WM2000-CENSUS.md` §4a) measures
// `MixedFillAndTmemLoadPacket` refusing 218/218 WM2000 frames: every frame
// the game draws issues both a `G_FILLRECT` and a TMEM load. These tests
// are the end-to-end evidence that the composition is admitted, that BOTH
// halves land, and that the order in which they land is the command
// stream's own -- not a merge policy this backend chose.
// ---------------------------------------------------------------------

/// The TMEM half of every composed fixture below: `SetTextureImage`,
/// `SetTile`, `LoadSync`, `LoadBlock` -- byte-for-byte
/// `one_load_block_words`'s own sequence, reused rather than re-spelled so
/// a composed packet's TMEM half is provably the same load the TMEM-only
/// tests already pin.
fn tmem_half_words() -> Vec<u32> {
    one_load_block_words()
}

/// The fill half: fill-cycle `OtherMode`, `SetColorImage`, `SetFillColor`,
/// and one whole-target `FillRectangle`. Identical to
/// `whole_target_fill_words`, and reused for the same reason.
fn fill_half_words(fill_color: u32) -> Vec<u32> {
    let mut words = Vec::new();
    words.extend(fill_cycle_other_mode());
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(fill_color));
    words.extend(fill_rectangle(
        0,
        0,
        FILL_TARGET_WIDTH - 1,
        FILL_TARGET_HEIGHT - 1,
    ));
    words
}

/// TMEM load first, then the fill -- one packet.
fn tmem_then_fill_words(fill_color: u32) -> Vec<u32> {
    let mut words = tmem_half_words();
    words.extend(fill_half_words(fill_color));
    words
}

/// The fill first, then the TMEM load -- the same two halves, swapped.
fn fill_then_tmem_words(fill_color: u32) -> Vec<u32> {
    let mut words = fill_half_words(fill_color);
    words.extend(tmem_half_words());
    words
}

/// The bytes `rdram_with_texture_source` writes at `TEXTURE_SOURCE_ADDR`,
/// hand-derived from that helper's own
/// `(0..64u16).flat_map(u16::to_be_bytes)` generator.
///
/// This is the SOURCE, not a model of where a `LoadBlock` puts each byte in
/// TMEM. That mapping is `LoadBlock`'s tile addressing plus the RDP's
/// odd-line word swizzle, which is `fn64-render-wgpu`'s own pinned
/// behavior and not this card's claim -- measured, the fixture's load lands
/// source halfwords 10..12 at TMEM 0..8 and 14..18 at TMEM 24..32. This
/// card asserts only that every published TMEM byte came from THIS source
/// (membership) and that the composed packet's TMEM state equals the
/// TMEM-only packet's exactly (the differential below), never that a
/// particular byte lands at a particular address.
fn expected_tmem_source_bytes() -> Vec<u8> {
    (0..64u16).flat_map(u16::to_be_bytes).collect()
}

/// The published physical TMEM state a TMEM-ONLY packet leaves behind, for
/// the composed packet to be compared against.
///
/// This is the differential the composition must satisfy: adding a fill to
/// a packet must not change one byte of what its TMEM half loads. Built by
/// running the identical TMEM half alone, through the identical producer
/// seam, in a freshly registered backend.
fn tmem_only_published_state() -> Vec<Option<u8>> {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    let backend = register_observed_session_backend_for_fills(rdram.len());
    dispatch_words(&mut rdram, &tmem_half_words());
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "the TMEM-only reference packet must complete"
    );
    let state = published_tmem_bytes(&backend, 128);
    drop(backend);
    teardown();
    assert!(
        state.iter().any(Option::is_some),
        "the TMEM-only reference must itself load something, or the differential is vacuous"
    );
    state
}

/// Read back the physical TMEM bytes this backend has published at
/// `0..len`, as `Option<u8>` per byte so an invalid (never-loaded) lane is
/// distinguishable from a loaded zero.
fn published_tmem_bytes(
    backend: &std::rc::Rc<std::cell::RefCell<fn64_render_wgpu::WgpuBackend>>,
    len: u16,
) -> Vec<Option<u8>> {
    let handle = backend.borrow();
    let physical = handle.physical_tmem();
    (0..len)
        .map(|address| physical.valid_byte(address))
        .collect()
}

/// **The card's headline measurement.** One packet carrying BOTH a TMEM
/// load and an admitted `FillRectangle` is admitted, and both halves land:
/// the fill's image reaches guest RDRAM and the TMEM load's bytes reach
/// published physical TMEM.
///
/// Before this card, `stage_and_report` refused this exact packet shape with
/// `MixedFillAndTmemLoadPacket` before either source staged anything, so
/// completing at all is itself part of the evidence. Both expectations are
/// hand-derived -- the fill's from `SET_FILL_COLOR`'s word and the RGBA16
/// even/odd column rule (`expected_fill_halfword`), the TMEM half's from
/// `rdram_with_texture_source`'s own generator -- never captured from a run.
///
/// The two halves are asserted independently and BOTH are nonvacuous: the
/// fill's target is poisoned first (so "the bytes changed" is falsifiable),
/// and the TMEM bytes are checked against a source pattern that is not
/// all-zero and not equal to the poison.
#[test]
fn a_composed_fill_and_tmem_packet_lands_both_halves() {
    const FILL_COLOR: u32 = 0x0842_1085;

    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    let backend = register_observed_session_backend_for_fills(rdram.len());

    // Nothing is published before the dispatch, on either side -- so
    // everything read afterwards was produced by this one packet.
    assert!(
        backend.borrow().color_targets().is_none(),
        "no color target may exist before the composed packet"
    );
    let tmem_before = published_tmem_bytes(&backend, 128);
    assert!(
        tmem_before.iter().all(Option::is_none),
        "no TMEM byte may be valid before the composed packet"
    );

    let poisoned = poison_fill_target(&mut rdram);
    let expected_image = expected_whole_target_image(FILL_COLOR);
    assert_ne!(
        expected_image, poisoned,
        "the poison must differ from the expected fill image, or the fill half is unfalsifiable"
    );

    dispatch_words(&mut rdram, &tmem_then_fill_words(FILL_COLOR));
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "a composed fill+TMEM packet must complete, leaving no pending fabric transaction -- \
         before this card it was refused outright with MixedFillAndTmemLoadPacket"
    );

    // Half one: the fill reached guest RDRAM.
    //
    // **Supersession (byte-lane).** This assertion read `rdram[target]`
    // directly and was CORRECT under the pre-`43d595c2` raw copyback, which
    // `copy_from_slice`d the backend's logical guest-order payload into the
    // native-word allocation unmapped -- making physical storage
    // coincidentally equal the logical image. `43d595c2` routed the copyback
    // through `RdramViewMut::write_logical_bytes`, so logical byte `o` now
    // lives at storage `o ^ 3` and a raw index no longer names it.
    //
    // The EXPECTATION is unchanged and still hand-derived: it was always a
    // logical guest-order image, never a model of physical storage. Only the
    // readback moved, onto `fn64-runtime`'s one lane authority. Re-derived
    // independently rather than transcribed: mapping the hand-derived
    // logical image through `^3` gives physical `[133, 16, 66, 8, ...]`,
    // which reconciles with the observed bytes, and its logical inverse is
    // exactly the unchanged `expected_whole_target_image`.
    assert_eq!(
        read_fill_target_logical(&rdram),
        expected_image,
        "the fill half of a composed packet must write its hand-derived image into guest RDRAM"
    );

    // Half two: the TMEM load reached published physical TMEM, and left
    // EXACTLY what the same load leaves when it runs with no fill beside it.
    let tmem_after = published_tmem_bytes(&backend, 128);
    let valid: Vec<(usize, u8)> = tmem_after
        .iter()
        .enumerate()
        .filter_map(|(address, byte)| byte.map(|value| (address, value)))
        .collect();
    assert!(
        !valid.is_empty(),
        "the TMEM half of a composed packet must leave valid bytes in published physical \
         TMEM -- an empty set is the signature of a dropped TMEM half"
    );

    // Every published byte came from this packet's own declared source.
    // Membership, not placement: where a LoadBlock puts each byte is
    // `fn64-render-wgpu`'s pinned tile-addressing behavior, not this card's.
    let source = expected_tmem_source_bytes();
    for (address, value) in &valid {
        assert!(
            source.contains(value),
            "published TMEM byte {address} = {value} is not one of the LoadBlock source bytes"
        );
    }
    // Nonvacuity: the loaded bytes are not all equal, so a port that
    // published a constant would fail the differential below.
    assert!(
        valid.iter().any(|(_, value)| *value != valid[0].1),
        "the loaded TMEM bytes must vary, or the TMEM differential is vacuous"
    );

    drop(backend);
    teardown();

    // **The differential.** Adding a fill to the packet must not perturb
    // one byte of what its TMEM half loads. Run last, because it registers
    // its own backend and tears it down.
    assert_eq!(
        tmem_after,
        tmem_only_published_state(),
        "a composed packet's published TMEM must be byte-identical to what the same TMEM half \
         publishes alone -- composition must not perturb the TMEM half at all"
    );
}

/// **Constraint 2 at the production seam: ordering is semantics.** The same
/// two halves in the two possible stream orders both dispatch cleanly, and
/// each lands both halves.
///
/// Why two clean dispatches prove the ordering was respected rather than
/// ignored: `fn64_render_ir::validate_effects` compares the backend's
/// reported write list against `journal().write_accesses()` **position by
/// position**, and the journal's order is the decoder's own `planned`
/// vector, appended to as the command stream is walked
/// (`raw_dpc::push_access` assigns each access an `OperationId` equal to its
/// index there). So a TMEM-then-fill packet declares its TMEM destination
/// write before the fill's render-target write, and a fill-then-TMEM packet
/// declares the reverse. A merge that emitted a FIXED order -- always fill
/// first, always TMEM first, or sorted by anything but journal position --
/// would satisfy at most one of these two fixtures and would be rejected on
/// the other with `EffectAccessMismatch`, panicking inside the dispatch.
///
/// The two orders are also proven to be genuinely different streams, so
/// this is not a claim about two identical inputs.
#[test]
fn both_composed_orders_dispatch_and_land_both_halves() {
    const FILL_COLOR: u32 = 0x0842_1085;

    let tmem_first = tmem_then_fill_words(FILL_COLOR);
    let fill_first = fill_then_tmem_words(FILL_COLOR);
    assert_ne!(
        tmem_first, fill_first,
        "the two fixtures must be genuinely different command streams, or the ordering claim \
         is about one stream twice"
    );
    assert_eq!(
        tmem_first.len(),
        fill_first.len(),
        "the two orders must carry the same commands -- only their order may differ"
    );

    let expected_image = expected_whole_target_image(FILL_COLOR);
    let mut published = Vec::new();

    for (label, words) in [("TMEM-first", tmem_first), ("fill-first", fill_first)] {
        crate::load_rom(Vec::new());
        let mut rdram = rdram_with_texture_source();
        let backend = register_observed_session_backend_for_fills(rdram.len());
        let poisoned = poison_fill_target(&mut rdram);
        assert_ne!(
            expected_image, poisoned,
            "{label}: the poison must differ from the expected image"
        );

        dispatch_words(&mut rdram, &words);
        assert!(
            with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
            "{label}: the composed order must complete -- a fixed-order merge would have been \
             rejected here with EffectAccessMismatch against this stream's own journal"
        );

        // Read through the lane authority, for the reason recorded at
        // `a_composed_fill_and_tmem_packet_lands_both_halves`'s own fill
        // assertion: `43d595c2` lane-maps the copyback, so a raw index no
        // longer names these bytes. The expectation is untouched -- it was
        // always a hand-derived logical image.
        assert_eq!(
            read_fill_target_logical(&rdram),
            expected_image,
            "{label}: the fill half must still land its hand-derived image"
        );
        let tmem = published_tmem_bytes(&backend, 128);
        assert!(
            tmem.iter().any(Option::is_some),
            "{label}: the TMEM half must still land"
        );
        published.push(tmem);
        drop(backend);
        teardown();
    }

    // Both orders load the same TMEM content: the halves are the same
    // commands, so only their declared ORDER differs, never their effect.
    assert_eq!(
        published[0], published[1],
        "both composed orders must load identical TMEM content -- the halves are the same \
         commands in a different order"
    );
}

/// A composition this slice still does NOT admit fails with a named error,
/// and leaves nothing published on either side.
///
/// Fill + TMEM + a triangle is the case: `stage_and_report` still refuses it
/// with `MixedFillAndTrianglePacket`, because a triangle raster declares no
/// write access in the journal at all, so unlike the fill+TMEM pair there is
/// no declared order to compose onto. Admitting fill+TMEM must not have
/// opened a back door for a triangle to ride along with them.
///
/// Driven through the real producer seam, where the refusal surfaces as a
/// panic inside `dispatch_dpc_submission` (an `execute_raw_dpc` error is not
/// a recoverable outcome at that seam). The panic message is asserted to
/// carry the variant's own name, so a DIFFERENT failure cannot pass as this
/// one -- and guest RDRAM is checked untouched afterwards, which is the
/// "loud rejection, never a quiet partial publish" half of the claim.
#[test]
fn a_composition_this_slice_does_not_admit_fails_by_name_at_the_producer_seam() {
    const FILL_COLOR: u32 = 0x0842_1085;
    const RAW_TRIANGLE_BASE_EDGE: u8 = 0x08;
    const SET_COMBINE: u8 = 0x3c;

    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend_for_fills(rdram.len());
    let poisoned = poison_fill_target(&mut rdram);

    // Fill + TMEM (now admitted on its own) PLUS a triangle (not admitted
    // in combination with a fill).
    let mut words = tmem_then_fill_words(FILL_COLOR);
    words.extend([word(SET_OTHER_MODE, 0), 0]);
    words.extend([word(SET_COMBINE, 0), 0]);
    // A minimal non-shade, non-texture, non-Z base-edge triangle: eight
    // words, all edge coefficients, matching `fn64-render-wgpu`'s own
    // `triangle_base_edge_words` fixture shape.
    words.extend([word(RAW_TRIANGLE_BASE_EDGE, 0), 0, 0, 0, 0, 0, 0, 0]);

    let bytes = words_to_rdram_bytes(&words);
    let start = 0x1000u32;
    let end = start + bytes.len() as u32;
    rdram[start as usize..end as usize].copy_from_slice(&bytes);
    let submission = admit_dram_submission(start, end);

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }));
    assert!(
        outcome.is_err(),
        "a fill + TMEM + triangle packet must be refused loudly at the producer seam -- \
         admitting fill + TMEM must not have let a triangle ride along silently"
    );

    let reason = crate::last_render_error().unwrap_or_default();
    let panic_text = outcome
        .err()
        .map(|payload| {
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_string())
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let named = format!("{reason}{panic_text}");
    // The exact refusal, not merely "something failed": measured, this is
    // `MixedFillAndTrianglePacket`'s own Display text, reached through
    // `render-wgpu/raw-dpc-execute`. Both fragments are asserted so a
    // DIFFERENT rejection -- a decode failure, a TMEM staging error, a
    // panic from somewhere else entirely -- cannot pass as this one.
    assert!(
        named.contains("render-wgpu/raw-dpc-execute"),
        "the refusal must come from the raw-DPC executor, got: {named}"
    );
    assert!(
        named
            .contains("declares both an admitted FillRectangle and at least one admitted triangle"),
        "the refusal must be MixedFillAndTrianglePacket's own named text, got: {named}"
    );

    // The loud-rejection half: nothing was half-published into guest memory.
    //
    // Read through the lane authority for the same reason as the sibling
    // composition tests (see
    // `a_composed_fill_and_tmem_packet_lands_both_halves`). This assertion
    // compares against `poisoned`, which `poison_fill_target` also writes and
    // returns in logical order, so both sides moved together -- and the
    // claim is unweakened: a fill half that leaked through would still
    // displace the poison and still fail here.
    assert_eq!(
        read_fill_target_logical(&rdram),
        poisoned,
        "a refused composition must leave every guest target byte at its poisoned value -- \
         never the fill half applied while the triangle half was dropped"
    );
    teardown();
}

/// **Constraint 4: composition must not collapse the partial-width ranges.**
/// A composed packet whose fill half is partial-width still declares and
/// writes N **disjoint** per-row ranges, strided by the color image's width.
///
/// Measured, not assumed: `fn64-render-wgpu`'s `raw_dpc::plan_fill`
/// collapses a fill to one access only when `x0 == 0 && x1 + 1 == width`.
/// `fill_rectangle(4, 2, 14, 4)` satisfies neither, so it declares three
/// accesses -- one per scanline, 22 bytes each. A composition that merged
/// the fill's writes into the TMEM report by concatenating a single
/// `[first_start, last_end)` span would cover 3 * 16 - 5 = 43 pixels instead
/// of 3 * 11 = 33, a ~30% over-claim, and would clobber the poison at
/// columns 0..4 and 15 of rows 2..=4.
///
/// The surviving poison bytes are what catch it, and they are checked
/// against the ORIGINAL poison rather than any fill expectation, so nothing
/// but an exactly-sized set of three disjoint copies can pass. This is the
/// sibling of `an_admitted_partial_width_fill_writes_only_its_own_disjoint_rows`,
/// carried into the composed path -- mutant (e) in this card's report.
#[test]
fn a_composed_packet_with_a_partial_width_fill_keeps_its_disjoint_rows() {
    const FIRST_FILL_COLOR: u32 = 0x0842_1085;
    const FILL_COLOR: u32 = 0x213c_4d59;
    // Mirrors `partial_width_fill_words`'s own `fill_rectangle(4, 2, 14, 4)`.
    const X0: u32 = 4;
    const X1: u32 = 14;
    const Y0: u32 = 2;
    const Y1: u32 = 4;

    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    let backend = register_observed_session_backend_for_fills(rdram.len());

    // A fresh target admits only a whole-target rectangle, so the partial
    // composed fill needs a resident predecessor first.
    dispatch_words(&mut rdram, &fill_half_words(FIRST_FILL_COLOR));
    let poisoned = poison_fill_target(&mut rdram);

    // The composed packet: the TMEM half, then a PARTIAL-width fill.
    let mut words = tmem_half_words();
    words.extend(fill_cycle_other_mode());
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(FILL_COLOR));
    words.extend(fill_rectangle(X0, Y0, X1, Y1));

    dispatch_words(&mut rdram, &words);
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "a composed packet with a partial-width fill must complete"
    );

    // Hand-derived: the poison everywhere, overwritten ONLY inside the
    // claimed rectangle, at TARGET-relative column parity.
    let mut expected = poisoned.clone();
    for y in Y0..=Y1 {
        for x in X0..=X1 {
            let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
            expected[offset..offset + 2]
                .copy_from_slice(&expected_fill_halfword(FILL_COLOR, x).to_be_bytes());
        }
    }

    // Read through the lane authority, and index the returned logical
    // image rather than raw storage -- see
    // `a_composed_fill_and_tmem_packet_lands_both_halves` for the
    // supersession record. `expected` is built from `poisoned` (logical) and
    // the hand-derived `expected_fill_halfword`, so it was always a logical
    // image; only the readback moved. The disjoint-rows claim is unweakened:
    // the surviving-poison discriminators below still compare against the
    // ORIGINAL poison, so only an exactly-sized set of three disjoint copies
    // can pass.
    let observed = read_fill_target_logical(&rdram);
    assert_eq!(
        observed, expected,
        "a composed packet's partial-width fill must write exactly its three disjoint rows \
         and leave every other guest byte at its poisoned value"
    );

    // The discriminators a collapsed span would destroy, named individually.
    let row2 = (FILL_TARGET_WIDTH * 2 * 2) as usize;
    assert_eq!(
        &observed[row2..row2 + (X0 * 2) as usize],
        &poisoned
            [(FILL_TARGET_WIDTH * 2 * 2) as usize..(FILL_TARGET_WIDTH * 2 * 2 + X0 * 2) as usize],
        "columns 0..4 of row 2 lie inside a collapsed [first_start, last_end) span and must \
         still be poison"
    );
    let row2_last = row2 + ((FILL_TARGET_WIDTH - 1) * 2) as usize;
    assert_eq!(
        &observed[row2_last..row2_last + 2],
        &poisoned[(FILL_TARGET_WIDTH * 2 * 2 + (FILL_TARGET_WIDTH - 1) * 2) as usize
            ..(FILL_TARGET_WIDTH * 2 * 2 + FILL_TARGET_WIDTH * 2) as usize],
        "column 15 of row 2 is right of x1 and must still be poison"
    );
    // And the rectangle really was written, so 'everything is poison' fails.
    let inside = row2 + (X0 * 2) as usize;
    assert_eq!(
        &observed[inside..inside + 2],
        &expected_fill_halfword(FILL_COLOR, X0).to_be_bytes(),
        "the rectangle's own first pixel must carry the composed fill's color"
    );

    // And the TMEM half still landed alongside it.
    assert!(
        published_tmem_bytes(&backend, 128)
            .iter()
            .any(Option::is_some),
        "the composed packet's TMEM half must land beside its partial-width fill"
    );
    drop(backend);
    teardown();
}
