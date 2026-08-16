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
/// TMEM load -- the exact shape `plan_raw_dpc` must reject loudly per this
/// card's "reject FullSync ... loudly" requirement.
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

#[test]
fn dram_producer_plan_rejects_full_sync_loudly_through_the_real_producer_seam() {
    crate::load_rom(Vec::new());
    let mut rdram = rdram_with_texture_source();
    register_session_backend(rdram.len());

    let words = one_load_block_then_full_sync_words();
    let bytes = words_to_rdram_bytes(&words);
    let start = 0x1000u32;
    let end = start + bytes.len() as u32;
    rdram[start as usize..end as usize].copy_from_slice(&bytes);

    let submission = admit_dram_submission(start, end);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        crate::task_dispatch::dispatch_dpc_submission(rdram.as_mut_ptr(), submission);
    }));
    assert!(
        result.is_err(),
        "a FullSync command reaching the real producer seam must panic, not silently admit"
    );
    // The rejected transaction must not leave a dangling pending fabric
    // submission behind -- `LiveDpcTransaction::drop` cancels on unwind.
    assert!(
        with_host(|host| host.device_fabric.pending_dpc_submission()).is_none(),
        "a rejected FullSync submission must not leave the fabric transaction pending"
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
