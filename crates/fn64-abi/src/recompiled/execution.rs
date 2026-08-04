use super::*;

/// Return the static link-time vram for a callback pointer relocated into a
/// currently loaded overlay, if any.
pub fn canonical_vram(vram: u32) -> Option<u32> {
    with_host(|host| host.sections.canonical_vram(vram))
}

/// Install the generated module's dispatcher for both thread 0 and every
/// OSThread subsequently created by `osCreateThread`.
pub fn set_entry_lookup(lookup: Lookup, rdram_len: usize) {
    set_entry_lookup_config(lookup, rdram_len, None);
}

/// Install a function-lane dispatcher and bind the stable identity of its
/// generated native artifact for release evidence.
///
/// The identity must come from the artifact producer (normally its SHA-256),
/// not from section geometry or native callable addresses. Reinstalling via
/// [`set_entry_lookup`] deliberately clears it.
pub fn set_entry_lookup_with_artifact_identity(
    lookup: Lookup,
    rdram_len: usize,
    identity: ProgramArtifactIdentity,
) {
    set_entry_lookup_config(lookup, rdram_len, Some(identity));
}

/// Install a function-lane dispatcher whose generated artifact exports the
/// current whole-function entry-observation schema.
///
/// `schema` must be the `FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA` constant from
/// the same generated artifact identified by `identity`. Keeping this separate
/// from [`set_entry_lookup_with_artifact_identity`] makes stale or handwritten
/// callable sets non-authoritative instead of silently producing a partial
/// destination history.
pub fn set_entry_lookup_with_execution_observation(
    lookup: Lookup,
    rdram_len: usize,
    identity: ProgramArtifactIdentity,
    schema: FunctionEntryObservationSchema,
) {
    set_entry_lookup_config(lookup, rdram_len, Some(identity));
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|installed| installed.set(Some(schema)));
    fn64_recomp_rs::set_function_entry_observer(Some(observe_function_entry));
}

fn set_entry_lookup_config(
    lookup: Lookup,
    rdram_len: usize,
    identity: Option<ProgramArtifactIdentity>,
) {
    assert!(rdram_len > 0, "recompiled RDRAM length must be nonzero");
    PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    CPU_INSTRUCTION_STORE_TRACE.with(|trace| *trace.borrow_mut() = None);
    RDP_RENDERER_WRITER_TRACE.with(|trace| *trace.borrow_mut() = None);
    EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow_mut().clear());
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| destinations.borrow_mut().clear());
    FUNCTION_LANE_ARTIFACT_IDENTITY.with(|installed| installed.set(identity));
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|installed| installed.set(None));
    fn64_recomp_rs::set_function_entry_observer(None);
    fn64_recomp_rs::set_write_observer(Some(observe_renderer_write));
    fn64_recomp_rs::set_guest_write_boundary_observer(None);
    fn64_recomp_rs::set_unsupported_observer(Some(record_recompiled_unsupported));
    with_host(|host| {
        host.recompiled_lookup = Some(lookup);
        host.recompiled_program = None;
        host.canonical_recompiled_program = None;
        host.recompiled_rdram_len = rdram_len;
    });
}

pub(super) fn set_block_program(program: LiveBlockProgram, rdram_len: usize) {
    assert!(rdram_len > 0, "recompiled RDRAM length must be nonzero");
    PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    RDP_RENDERER_WRITER_TRACE.with(|trace| *trace.borrow_mut() = None);
    EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow_mut().clear());
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| destinations.borrow_mut().clear());
    BLOCK_HOST_BOUNDARIES.with(|boundaries| boundaries.borrow_mut().clear());
    BLOCK_HOST_BOUNDARY_HISTORY_LIMIT.with(|limit| limit.set(None));
    BLOCK_HOST_BOUNDARY_HISTORY_ENABLED.with(|enabled| enabled.set(true));
    FUNCTION_LANE_ARTIFACT_IDENTITY.with(|installed| installed.set(None));
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|installed| installed.set(None));
    fn64_recomp_rs::set_function_entry_observer(None);
    fn64_recomp_rs::set_unsupported_observer(Some(record_recompiled_unsupported));
    fn64_recomp_rs::set_guest_write_boundary_observer(Some(classify_live_executable_write));
    with_host(|host| {
        host.recompiled_lookup = None;
        host.recompiled_program = Some(program);
        host.canonical_recompiled_program = None;
        host.recompiled_rdram_len = rdram_len;
    });
}

pub(super) fn set_catalog_block_program(
    install: CatalogResolverInstallV1,
    rdram_len: usize,
) -> CanonicalLiveBlockProgramV1 {
    set_catalog_program_parts(install, None, rdram_len, None)
}

pub(super) fn set_catalog_generation_program(
    install: CatalogGenerationInstallV1,
    rdram_len: usize,
) -> CanonicalLiveBlockProgramV1 {
    set_catalog_program_parts(install.resolver, Some(install.generations), rdram_len, None)
}

fn set_catalog_program_parts(
    install: CatalogResolverInstallV1,
    generations: Option<BackedPrecompiledGenerationCatalogV1>,
    rdram_len: usize,
    bootstrap: Option<&ValidatedBootstrapRdramV1>,
) -> CanonicalLiveBlockProgramV1 {
    assert!(rdram_len > 0, "recompiled RDRAM length must be nonzero");
    let ranges = executable_physical_ranges_for_parts(&install, generations.as_ref());
    let watched_ranges = ranges
        .iter()
        .map(
            |&(physical_start, physical_end)| PendingExecutableWriteEvidenceSnapshot {
                physical_start,
                physical_end,
            },
        )
        .collect::<Vec<_>>();
    let writer_program_model_sha256 =
        canonical_writer_program_model_sha256(&install, generations.as_ref(), &watched_ranges);
    if let Some(&(_, required_end)) = ranges.last() {
        assert!(
            usize::try_from(required_end).unwrap() <= rdram_len,
            "canonical executable backing ends at physical RDRAM {required_end:#010x}, beyond the installed {rdram_len:#x}-byte allocation"
        );
    }
    PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    RDP_RENDERER_WRITER_TRACE.with(|trace| *trace.borrow_mut() = None);
    EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow_mut().clear());
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| destinations.borrow_mut().clear());
    BLOCK_HOST_BOUNDARIES.with(|boundaries| boundaries.borrow_mut().clear());
    BLOCK_HOST_BOUNDARY_HISTORY_LIMIT.with(|limit| limit.set(None));
    BLOCK_HOST_BOUNDARY_HISTORY_ENABLED.with(|enabled| enabled.set(true));
    FUNCTION_LANE_ARTIFACT_IDENTITY.with(|installed| installed.set(None));
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|installed| installed.set(None));
    fn64_recomp_rs::set_function_entry_observer(None);
    fn64_recomp_rs::set_unsupported_observer(Some(record_recompiled_unsupported));
    fn64_recomp_rs::set_host_lookup(None);
    let mutation_state = (!ranges.is_empty()).then(|| {
        let state = bootstrap.map_or_else(
            || CanonicalExecutableMutationStateV1::new(&ranges),
            |validated| {
                CanonicalExecutableMutationStateV1::from_bootstrap(
                    validated.receipt.evidence(),
                    &validated.storage,
                )
            },
        );
        Rc::new(RefCell::new(state))
    });
    let generations = generations.map(|generations| Rc::new(RefCell::new(generations)));
    if mutation_state.is_some() {
        EXECUTABLE_WRITE_RANGES.with(|installed| {
            installed.borrow_mut().extend_from_slice(&ranges);
        });
        fn64_recomp_rs::set_guest_write_boundary_observer(Some(classify_live_executable_write));
        fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));
    } else {
        fn64_recomp_rs::set_guest_write_boundary_observer(None);
        fn64_recomp_rs::set_write_observer(Some(observe_renderer_write));
    }
    let live = CanonicalLiveBlockProgramV1 {
        install: Rc::new(install),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_units: Rc::new(RefCell::new(None)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_withheld_static_key: Rc::new(Cell::new(None)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_execution_aggregates: Rc::new(RefCell::new(BTreeMap::new())),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_identity_activations: Rc::new(Cell::new(0)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_identity_charged_instructions: Rc::new(Cell::new(0)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_identity_unsupported_exits: Rc::new(Cell::new(0)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_attempted_entry_activations: Rc::new(Cell::new(0)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_attempted_entry_charged_instructions: Rc::new(Cell::new(0)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_attempted_entry_unsupported_exits: Rc::new(Cell::new(0)),
        canonical_charged_instructions: Rc::new(Cell::new(0)),
        canonical_instruction_limit: Rc::new(Cell::new(None)),
        thread_publications: Rc::new(RefCell::new(BTreeMap::new())),
        generations,
        mutation_state,
        bootstrap_evidence: bootstrap.map(|validated| validated.receipt.evidence().clone()),
        writer_program_model_sha256,
        bootstrap_writer_completion: Rc::new(RefCell::new(None)),
        cpu_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        cpu_writer_trace_epoch_id: Rc::new(Cell::new(None)),
        pi_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        pi_writer_trace_epoch_id: Rc::new(Cell::new(None)),
        si_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        sp_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        sp_writer_trace_epoch_id: Rc::new(Cell::new(None)),
        host_abi_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        rsp_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        rsp_writer_trace_epoch_id: Rc::new(Cell::new(None)),
        rdp_renderer_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        rdp_renderer_writer_trace_epoch_id: Rc::new(Cell::new(None)),
    };
    with_host(|host| {
        host.recompiled_lookup = None;
        host.recompiled_program = None;
        host.canonical_recompiled_program = Some(live.clone());
        host.recompiled_rdram_len = rdram_len;
    });
    live
}

/// Install the closed, immutable AOT generation inventory used by fetch-time
/// digest selection. Every referenced shard must already be registered in the
/// live `BlockProgram`; activation changes only virtual interval ownership and
/// never invokes a builder, interpreter, or runtime translator.
pub fn install_precompiled_generation_catalog(catalog: PrecompiledGenerationCatalog) {
    let live = with_host(|host| host.recompiled_program.clone()).unwrap_or_else(|| {
        panic!("install_precompiled_generation_catalog: no live BlockProgram is installed")
    });
    catalog
        .validate_program(&live.program.borrow())
        .unwrap_or_else(|error| {
            panic!("install_precompiled_generation_catalog: catalog does not match the live BlockProgram: {error}")
        });
    let mut installed = live.precompiled_generations.borrow_mut();
    assert!(
        installed.is_none(),
        "precompiled generation catalog is already installed"
    );
    *installed = Some(catalog);
}

/// Replace one live executable region with a new immutable bank generation.
/// This is safe at host/device boundaries, where `run_block_program` has
/// already dropped its immutable dispatch borrow. The old bank and runner are
/// retired atomically by [`ExecutableRegion::install`].
pub fn install_live_block_generation(
    region: &mut ExecutableRegion,
    code: CodeBank,
    runner: GeneratedBankRunner,
) -> Result<Option<BankId>, GenerationError> {
    let live = with_host(|host| host.recompiled_program.clone()).unwrap_or_else(|| {
        panic!("install_live_block_generation: no live BlockProgram is installed")
    });
    let result = region.install(&mut live.program.borrow_mut(), code, runner);
    result
}

/// Observe one physical RDRAM span as the backing bytes for an already-live
/// virtual executable region. CPU stores and device DMA writes that intersect
/// the physical span rebuild and atomically replace its active immutable bank
/// at the next host boundary.
///
/// The builder is deliberately a pure function of the final committed byte
/// image and a monotonically increasing generation number. It may select a
/// pre-generated runner today or invoke a future translator, but may not
/// publish half of a code/runner pair.
pub fn register_live_executable_region(
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    builder: LiveGenerationBuilder,
) {
    register_live_executable_region_config(
        physical_start,
        physical_end,
        region,
        builder,
        None,
        ExecutableActivation::EagerPublication,
    );
}

/// Observe a replaceable executable image whose writer may construct it over
/// many guest instructions. Writes mark it dirty, but generation lookup is
/// deferred until an attempted fetch reports the completed live digest.
pub fn register_fetch_activated_executable_region(
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    builder: LiveGenerationBuilder,
) {
    register_live_executable_region_config(
        physical_start,
        physical_end,
        region,
        builder,
        None,
        ExecutableActivation::FetchBoundary,
    );
}

/// Register a dynamic executable region while binding the stable artifact
/// which implements its pure generation builder.
pub fn register_live_executable_region_with_artifact_identity(
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    builder: LiveGenerationBuilder,
    builder_artifact_identity: ProgramArtifactIdentity,
) {
    register_live_executable_region_config(
        physical_start,
        physical_end,
        region,
        builder,
        Some(builder_artifact_identity),
        ExecutableActivation::EagerPublication,
    );
}

/// Register a fetch-activated executable image while binding the stable
/// artifact which performs its closed-set generation selection.
pub fn register_fetch_activated_executable_region_with_artifact_identity(
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    builder: LiveGenerationBuilder,
    builder_artifact_identity: ProgramArtifactIdentity,
) {
    register_live_executable_region_config(
        physical_start,
        physical_end,
        region,
        builder,
        Some(builder_artifact_identity),
        ExecutableActivation::FetchBoundary,
    );
}

fn register_live_executable_region_config(
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    builder: LiveGenerationBuilder,
    builder_artifact_identity: Option<ProgramArtifactIdentity>,
    activation: ExecutableActivation,
) {
    assert!(
        physical_start < physical_end,
        "observed executable RDRAM region must be nonempty"
    );
    assert!(
        physical_start.is_multiple_of(4) && physical_end.is_multiple_of(4),
        "observed executable RDRAM bounds must be instruction-aligned"
    );
    let physical_len = physical_end - physical_start;
    let virtual_len = region.end().get() - region.start().get();
    assert_eq!(
        physical_len, virtual_len,
        "observed physical and virtual executable regions must have equal byte lengths"
    );
    let active = region.active_bank().unwrap_or_else(|| {
        panic!("observed executable region must have an installed active generation")
    });
    let (live, rdram_len) = with_host(|host| {
        (
            host.recompiled_program.clone().unwrap_or_else(|| {
                panic!("register_live_executable_region: no live BlockProgram is installed")
            }),
            host.recompiled_rdram_len,
        )
    });
    assert!(
        usize::try_from(physical_end).expect("physical executable end exceeds usize") <= rdram_len,
        "observed executable RDRAM end {physical_end:#010x} exceeds allocation {rdram_len:#x}"
    );
    {
        let program = live.program.borrow();
        let code = program.code().bank(active).unwrap_or_else(|| {
            panic!("observed executable region references missing active generation {active}")
        });
        assert_eq!(
            (code.vram_start(), code.vram_end()),
            (region.start(), region.end()),
            "observed executable region does not match its active bank"
        );
    }
    let mut regions = live.executable_regions.borrow_mut();
    assert!(
        regions.iter().all(|existing| {
            physical_end <= existing.physical_start || physical_start >= existing.physical_end
        }),
        "observed executable physical region overlaps an existing registration"
    );
    assert!(
        regions.iter().all(|existing| {
            region.end() <= existing.region.start() || region.start() >= existing.region.end()
        }),
        "observed executable virtual region overlaps an existing registration"
    );
    regions.push(ObservedExecutableRegion {
        physical_start,
        physical_end,
        region,
        next_generation: 1,
        builder,
        builder_artifact_identity,
        activation,
    });
    EXECUTABLE_WRITE_RANGES.with(|ranges| {
        ranges.borrow_mut().push((physical_start, physical_end));
    });
}

/// Apply DMA-originated executable writes after the device fabric has
/// committed all bytes, but before it publishes completion messages or any
/// guest coroutine can resume.
pub(crate) fn process_live_executable_writes_from_host() {
    let (catalog, live) = with_host(|host| {
        (
            host.canonical_recompiled_program.clone(),
            host.recompiled_program.clone(),
        )
    });
    if let Some(catalog) = catalog {
        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        let required_end = catalog
            .mutation_state
            .as_ref()
            .map(|state| state.borrow().required_physical_end() as usize)
            .unwrap_or(1);
        assert!(
            !rdram.is_null() && rdram_len >= required_end,
            "canonical generation host write RDRAM allocation {rdram_len:#x} does not cover watched physical end {required_end:#x}"
        );
        // SAFETY: device/host publication runs only while guest execution is
        // suspended; the registered process allocation remains live.
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        catalog.invalidate_pending_physical_writes_with(|physical| unsafe {
            storage.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
        });
        fn64_recomp_rs::discard_executable_write_boundary();
        return;
    }
    let Some(live) = live else {
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        fn64_recomp_rs::discard_executable_write_boundary();
        return;
    };
    let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    assert!(
        !rdram.is_null() && rdram_len > 0,
        "live BlockProgram has no process RDRAM allocation"
    );
    // SAFETY: block execution is suspended at this device boundary and the
    // boot contract keeps the one process RDRAM allocation live throughout.
    // Use the raw storage adapter instead of manufacturing a second `&mut`
    // slice while the dormant coroutine retains its checked `Rdram` view.
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    process_executable_writes(&live, |offset| unsafe {
        storage.read_u8(fn64_runtime::RdramAddr::from_offset(offset))
    });
    fn64_recomp_rs::discard_executable_write_boundary();
}

/// Commit the active catalog host transaction's current mutation prefix before
/// its coroutine yields control. This closes the exact interleaving
/// `HostAbi write -> coroutine suspend -> device/other-thread same-byte write
/// -> HostAbi resume`: the parent prefix advances the canonical baseline before
/// any child or different guest coroutine can run.
pub(crate) fn checkpoint_catalog_host_transaction_before_suspend() {
    let Some(thread) = crate::ACTIVE_THREAD_ID.with(Cell::get) else {
        return;
    };
    let (live, rdram, rdram_len) = with_host(|host| {
        (
            host.canonical_recompiled_program.clone(),
            host.runtime_rdram,
            host.runtime_rdram_len,
        )
    });
    let Some(live) = live else {
        return;
    };
    let Some(required_end) = live.mutation_state.as_ref().and_then(|state| {
        let state = state.borrow();
        state
            .active_host_transaction(thread)
            .map(|_| state.required_physical_end() as usize)
    }) else {
        return;
    };
    assert!(
        !rdram.is_null() && rdram_len >= required_end,
        "catalog host transaction checkpoint has no live RDRAM through watched end {required_end:#x}"
    );
    // SAFETY: this hook runs on the currently executing guest coroutine before
    // suspension. The process allocation is stable, and reads finish before
    // the yielder can transfer control to another coroutine.
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    live.flush_active_host_abi_transaction_with(thread, |physical| unsafe {
        storage.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
    });
}

/// Move-only ownership of one synchronous child writer publication.
///
/// Dropping an uncommitted canonical token poisons the mutation owner. This
/// closes the unwind interleaving `parent prefix committed -> child mutates ->
/// child unwinds -> parent/guest resumes`: no later dispatch can accept the
/// possibly-partial child image as an unattributed host suffix.
pub(crate) struct CatalogNestedWriterTransactionV1 {
    live: Option<CanonicalLiveBlockProgramV1>,
    transaction_id: Option<u64>,
    thread: Option<ThreadId>,
    operation: &'static str,
    committed: bool,
}

impl CatalogNestedWriterTransactionV1 {
    pub(super) fn is_canonical(&self) -> bool {
        self.transaction_id.is_some()
    }

    fn assert_thread_owner(&self) {
        if let Some(expected) = self.thread {
            assert_eq!(
                crate::ACTIVE_THREAD_ID.with(Cell::get),
                Some(expected),
                "{} child writer transaction changed guest-thread owner before commit",
                self.operation
            );
        }
    }

    fn commit_with(mut self, mut read_physical_byte: impl FnMut(u32) -> u8) {
        self.assert_thread_owner();
        if let Some(live) = &self.live {
            if let Some(transaction_id) = self.transaction_id {
                live.mutation_state
                    .as_ref()
                    .expect("canonical child writer token lost its mutation state")
                    .borrow()
                    .assert_active_child_transaction(transaction_id);
            }
            live.invalidate_pending_physical_writes_with(&mut read_physical_byte);
            fn64_recomp_rs::discard_executable_write_boundary();
            if let Some(transaction_id) = self.transaction_id {
                live.mutation_state
                    .as_ref()
                    .expect("canonical child writer token lost its mutation state")
                    .borrow_mut()
                    .finish_child_transaction(transaction_id);
            }
        }
        self.committed = true;
    }

    pub(crate) fn commit(self, rdram: &[u8]) {
        let view = fn64_runtime::RdramView::from_storage(rdram);
        self.commit_with(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
    }

    pub(super) fn commit_changed_bytes(self, rdram: &[u8], notify: impl Fn(u32, u32)) {
        self.assert_thread_owner();
        let live = self
            .live
            .as_ref()
            .expect("canonical child writer transaction lost its live owner");
        let state = live
            .mutation_state
            .as_ref()
            .expect("canonical child writer transaction has no mutation state");
        let view = fn64_runtime::RdramView::from_storage(rdram);
        let snapshot = state
            .borrow()
            .read_snapshot_from_view(&view);
        let changed = state.borrow().current_changed_ranges(&snapshot);
        for (physical_start, physical_end) in changed {
            notify(physical_start, physical_end - physical_start);
        }
        self.commit(rdram);
    }
}

/// Commit the selected `OSThread *` mirror through the canonical executable
/// mutation owner before the selected coroutine can execute. Compatibility
/// lanes return `false` and retain the legacy raw publication in `host.rs`.
///
/// This publication uses the HostAbi writer channel but intentionally does
/// not enter the host-call lifecycle trace: scheduler selection has no guest
/// call target/resume pair. Consequently the existing host-call-only
/// completion receipt remains open rather than miscounting this boundary.
pub(crate) fn commit_scheduler_running_thread_mirror(
    origin: SchedulerRunningThreadMirrorV1,
) -> bool {
    let live = with_host(|host| host.canonical_recompiled_program.clone());
    let Some(live) = live else {
        return false;
    };
    let Some(state) = live.mutation_state.as_ref() else {
        return false;
    };
    let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    let physical_start = origin.global.offset();
    let physical_end = physical_start
        .checked_add(4)
        .expect("scheduler running-thread mirror range overflow");
    assert!(
        !rdram.is_null() && usize::try_from(physical_end).unwrap() <= rdram_len,
        "scheduler running-thread mirror for selected thread {} exceeds registered RDRAM: [{physical_start:#010x}, {physical_end:#010x}) vs {rdram_len:#x}",
        origin.selected_thread
    );

    // SAFETY: scheduler selection runs between coroutine resumes. The process
    // allocation is stable, and all raw reads/writes finish before the selected
    // coroutine receives RunToken.
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    live.reconcile_before_dispatch_with(|physical| unsafe {
        storage.read_u8(RdramAddr::from_offset(physical))
    });
    if unsafe { storage.read_u32(origin.global) } == origin.handle {
        return true;
    }

    let transaction_id = state.borrow_mut().begin_child_transaction();
    let transaction = CatalogNestedWriterTransactionV1 {
        live: Some(live),
        transaction_id: Some(transaction_id),
        thread: None,
        operation: "scheduler running-thread mirror",
        committed: false,
    };
    unsafe { storage.write_u32(origin.global, origin.handle) };
    fn64_recomp_rs::notify_host_abi_write(physical_start, 4);
    transaction
        .commit_with(|physical| unsafe { storage.read_u8(RdramAddr::from_offset(physical)) });
    true
}

impl Drop for CatalogNestedWriterTransactionV1 {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let Some(state) = self
            .live
            .as_ref()
            .and_then(|live| live.mutation_state.as_ref())
        else {
            return;
        };
        state.borrow_mut().poison(format!(
            "{} child writer transaction unwound before commit",
            self.operation
        ));
    }
}

pub(crate) fn begin_catalog_nested_writer(
    rdram: &[u8],
    operation: &'static str,
) -> CatalogNestedWriterTransactionV1 {
    let thread = crate::ACTIVE_THREAD_ID.with(Cell::get);
    let live = with_host(|host| host.canonical_recompiled_program.clone());
    if let Some(live) = &live {
        if let Some(state) = &live.mutation_state {
            state.borrow().assert_not_poisoned();
        }
        if let Some(thread) = thread {
            let view = fn64_runtime::RdramView::from_storage(rdram);
            live.flush_active_host_abi_transaction_with(thread, |physical| {
                view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
            });
        }
    }
    let transaction_id = live.as_ref().and_then(|live| {
        live.mutation_state
            .as_ref()
            .map(|state| state.borrow_mut().begin_child_transaction())
    });
    CatalogNestedWriterTransactionV1 {
        live,
        transaction_id,
        thread,
        operation,
        committed: false,
    }
}

/// Test-only ownership token for replacing executable-write preflight inputs.
///
/// The token is thread-bound because the guarded state lives in thread-local
/// storage. Nested scopes are supported and restore the immediately preceding
/// state, including while unwinding from an expected loud trap.
#[cfg(all(test, feature = "recomp-rs"))]
#[must_use = "dropping the guard restores the prior executable-write preflight state"]
pub(crate) struct TestExecutableWritePreflightState {
    prior_ranges: Vec<(u32, u32)>,
    prior_pending: Vec<(u32, u32)>,
    prior_attributed: Vec<GuestWriteEvent>,
    _thread_bound: std::marker::PhantomData<Rc<()>>,
}

#[cfg(all(test, feature = "recomp-rs"))]
impl Drop for TestExecutableWritePreflightState {
    fn drop(&mut self) {
        EXECUTABLE_WRITE_RANGES.with(|ranges| {
            PENDING_EXECUTABLE_WRITES.with(|pending| {
                let mut ranges = ranges.borrow_mut();
                let mut pending = pending.borrow_mut();
                *ranges = std::mem::take(&mut self.prior_ranges);
                *pending = std::mem::take(&mut self.prior_pending);
            });
        });
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| {
            *pending.borrow_mut() = std::mem::take(&mut self.prior_attributed);
        });
    }
}

#[cfg(all(test, feature = "recomp-rs"))]
pub(crate) fn scoped_test_executable_write_preflight_state(
    ranges: Vec<(u32, u32)>,
    pending: Vec<(u32, u32)>,
) -> TestExecutableWritePreflightState {
    let (prior_ranges, prior_pending) = EXECUTABLE_WRITE_RANGES.with(|current_ranges| {
        PENDING_EXECUTABLE_WRITES.with(|current_pending| {
            let mut current_ranges = current_ranges.borrow_mut();
            let mut current_pending = current_pending.borrow_mut();
            (
                std::mem::replace(&mut *current_ranges, ranges),
                std::mem::replace(&mut *current_pending, pending),
            )
        })
    });
    let prior_attributed = PENDING_ATTRIBUTED_EXECUTABLE_WRITES
        .with(|current| std::mem::take(&mut *current.borrow_mut()));
    TestExecutableWritePreflightState {
        prior_ranges,
        prior_pending,
        prior_attributed,
        _thread_bound: std::marker::PhantomData,
    }
}

/// Reject a planned host publication that would require fallible executable
/// generation after the guest bytes become visible.
///
/// The verified-audio adapter has no transaction spanning RDRAM, device state,
/// and native-code installation. This read-only check lets ordinary audio-data
/// writes proceed while forcing executable writes to remain on the interpreter
/// path until such a transaction exists.
#[cfg(test)]
pub(crate) fn preflight_non_executable_host_writes(
    writes: &[(usize, usize)],
) -> Result<(), String> {
    EXECUTABLE_WRITE_RANGES.with(|ranges| {
        let ranges = ranges.borrow();
        let pending = PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone());
        for (start, len) in pending {
            let end = start.saturating_add(len);
            if let Some(&(executable_start, executable_end)) = ranges
                .iter()
                .find(|&&(executable_start, executable_end)| {
                    start < executable_end && end > executable_start
                })
            {
                return Err(format!(
                    "pending host write [{start:#010x}, {end:#010x}) overlaps live executable region [{executable_start:#010x}, {executable_end:#010x}); transactional executable publication is unavailable"
                ));
            }
        }
        for &(start, end) in writes {
            let start = u32::try_from(start)
                .map_err(|_| format!("host write start {start:#x} exceeds physical address space"))?;
            let end = u32::try_from(end)
                .map_err(|_| format!("host write end {end:#x} exceeds physical address space"))?;
            if let Some(&(executable_start, executable_end)) = ranges
                .iter()
                .find(|&&(executable_start, executable_end)| {
                    start < executable_end && end > executable_start
                })
            {
                return Err(format!(
                    "verified audio write [{start:#010x}, {end:#010x}) overlaps live executable region [{executable_start:#010x}, {executable_end:#010x}); transactional executable publication is unavailable"
                ));
            }
        }
        Ok(())
    })
}

fn pause_active_recompiled_thread() {
    crate::suspend_active_coroutine(fn64_runtime::Yield::PauseSelf);
}

pub(super) fn read_raw_mmio(vaddr: u64) -> Option<u32> {
    crate::pi::read_raw_mmio_word(vaddr)
}

pub(super) fn write_raw_mmio(vaddr: u64, value: u32) -> bool {
    crate::pi::write_raw_mmio_word(vaddr, value)
}

pub(super) fn record_recompiled_unsupported(context: &str) {
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Recompiler,
        "recompiler.cpu.unsupported-instruction",
        context,
        Some(fn64_runtime::Cycles::new(crate::sim_time())),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
}

pub(crate) fn recompiled_gap_panic(context: impl Into<String>) -> ! {
    let context = context.into();
    record_recompiled_unsupported(&context);
    panic!("{context}")
}

/// Create and start thread 0 with a typed recompiled entrypoint on fn64's existing
/// single executor. No second executor, RDRAM allocation, or host thread is
/// created.
///
/// # Safety
/// `rdram` must address `rdram_len` live bytes for every coroutine's lifetime,
/// exactly like [`crate::boot_thread0`]'s existing C ABI contract.
pub unsafe fn boot_thread0(
    rdram: *mut u8,
    rdram_len: usize,
    lookup: Lookup,
    entry: RecompFunc,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe {
        boot_thread0_config(
            rdram, rdram_len, lookup, entry, None, None, thread_id, priority,
        )
    };
}

/// Boot the function lane while binding its stable generated-artifact
/// identity for pointer-independent release evidence.
///
/// # Safety
/// Identical to [`boot_thread0`]. The identity describes the native artifact
/// containing both `lookup` and `entry`; it is not derived from either pointer.
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_with_artifact_identity(
    rdram: *mut u8,
    rdram_len: usize,
    lookup: Lookup,
    entry: RecompFunc,
    artifact_identity: ProgramArtifactIdentity,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe {
        boot_thread0_config(
            rdram,
            rdram_len,
            lookup,
            entry,
            Some(artifact_identity),
            None,
            thread_id,
            priority,
        )
    };
}

/// Boot an artifact emitted with authoritative whole-function entry
/// observation enabled.
///
/// `schema` must be the generated artifact's
/// `FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA` export.
///
/// # Safety
/// Identical to [`boot_thread0`]. The artifact identity and schema must both
/// describe the native artifact containing `lookup` and `entry`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_with_execution_observation(
    rdram: *mut u8,
    rdram_len: usize,
    lookup: Lookup,
    entry: RecompFunc,
    artifact_identity: ProgramArtifactIdentity,
    schema: FunctionEntryObservationSchema,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe {
        boot_thread0_config(
            rdram,
            rdram_len,
            lookup,
            entry,
            Some(artifact_identity),
            Some(schema),
            thread_id,
            priority,
        )
    };
}

#[allow(clippy::too_many_arguments)]
unsafe fn boot_thread0_config(
    rdram: *mut u8,
    rdram_len: usize,
    lookup: Lookup,
    entry: RecompFunc,
    artifact_identity: Option<ProgramArtifactIdentity>,
    observation_schema: Option<FunctionEntryObservationSchema>,
    thread_id: ThreadId,
    priority: Priority,
) {
    match (artifact_identity, observation_schema) {
        (Some(identity), Some(schema)) => {
            set_entry_lookup_with_execution_observation(lookup, rdram_len, identity, schema);
        }
        (identity, None) => set_entry_lookup_config(lookup, rdram_len, identity),
        (None, Some(_)) => {
            panic!("function entry observation requires a stable generated-artifact identity")
        }
    }
    unsafe { crate::register_process_rdram(rdram, rdram_len) };
    fn64_recomp_rs::set_host_pause(Some(pause_active_recompiled_thread));
    fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));

    let rdram_addr = rdram as usize;
    with_executor(|exec| {
        exec.create_thread(thread_id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(thread_id, rdram_ptr, yielder, || {
                let _ = first_input;
                // SAFETY: the boot host guarantees the allocation outlives all
                // executor coroutines and contains exactly `rdram_len` bytes.
                let bytes = unsafe { std::slice::from_raw_parts_mut(rdram_ptr, rdram_len) };
                let mut mem = Rdram::new(bytes);
                let mut ctx = RsContext::new();
                entry(&mut ctx, &mut mem);
            });
        });
        exec.start_thread(thread_id);
    });
}

/// Create and start thread 0 with the bank-qualified arbitrary-PC execution
/// program as the live owner. Each instruction checkpoint suspends back to
/// the existing executor, which charges guest instructions to virtual time
/// and services due devices before any coroutine resumes.
///
/// `boot_context` must be the black-box IPL3 handoff captured for the exact
/// installed normalized ROM. Its entry PC and TV standard are checked before
/// the coroutine is created.
///
/// # Safety
/// `rdram` must address `rdram_len` live bytes for every coroutine's lifetime,
/// exactly like [`boot_thread0`]'s existing shared-allocation contract.
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_block_program(
    rdram: *mut u8,
    rdram_len: usize,
    program: BlockProgram,
    entry: ExecutionKey,
    boot_context: BootContext,
    entry_lookup: ProgramEntryLookup,
    transfer_lookup: ProgramTransferLookup,
    budget: InstructionBudget,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe {
        boot_thread0_block_program_config(
            rdram,
            rdram_len,
            program,
            entry,
            boot_context,
            entry_lookup,
            transfer_lookup,
            budget,
            None,
            thread_id,
            priority,
        )
    };
}

/// Boot the block lane while binding the stable artifact which supplies its
/// entry/transfer resolver implementations.
///
/// # Safety
/// Identical to [`boot_thread0_block_program`].
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_block_program_with_artifact_identity(
    rdram: *mut u8,
    rdram_len: usize,
    program: BlockProgram,
    entry: ExecutionKey,
    boot_context: BootContext,
    entry_lookup: ProgramEntryLookup,
    transfer_lookup: ProgramTransferLookup,
    budget: InstructionBudget,
    dispatch_artifact_identity: ProgramArtifactIdentity,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe {
        boot_thread0_block_program_config(
            rdram,
            rdram_len,
            program,
            entry,
            boot_context,
            entry_lookup,
            transfer_lookup,
            budget,
            Some(dispatch_artifact_identity),
            thread_id,
            priority,
        )
    };
}

/// Boot the callback-free canonical static catalog lane.
///
/// The consumed install is the sole owner of the program, entry, instruction
/// budget, host targets, and dispatch identity. Dynamic image activation is
/// intentionally unavailable in this first authority-capable path: an image
/// change traps loudly instead of entering a legacy builder or resolver.
///
/// # Safety
/// `rdram` must address `rdram_len` live bytes for every coroutine's lifetime,
/// exactly like [`boot_thread0_block_program`].
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_catalog_program_v1(
    rdram: *mut u8,
    rdram_len: usize,
    install: CatalogResolverInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) {
    let entry = install.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let live = set_catalog_block_program(install, rdram_len);
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        )
    };
}

/// Boot the canonical catalog with explicit operational exact-unit fallback.
/// The dynamic catalog is source-bound and remains outside immutable static
/// program evidence; enabling it makes every static writer/release authority
/// constructor fail with `DynamicExecutionInstalled`.
///
/// # Safety
/// Identical to [`boot_thread0_catalog_program_v1`].
#[cfg(feature = "dynamic-mapped-runtime")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_catalog_program_with_dynamic_mapped_v1(
    rdram: *mut u8,
    rdram_len: usize,
    install: CatalogResolverInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) {
    let entry = install.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let live = set_catalog_block_program(install, rdram_len);
    live.enable_dynamic_mapped_execution();
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        )
    };
}

/// Boot the canonical catalog lane with a closed, explicitly physically
/// backed precompiled-generation inventory.
///
/// # Safety
/// Identical to [`boot_thread0_catalog_program_v1`].
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_catalog_generation_program_v1(
    rdram: *mut u8,
    rdram_len: usize,
    install: CatalogGenerationInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) {
    let entry = install.resolver.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let live = set_catalog_generation_program(install, rdram_len);
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        )
    };
}

/// Boot the closed precompiled-generation catalog with explicit operational
/// exact-unit fallback for destinations absent from usable AOT.
///
/// # Safety
/// Identical to [`boot_thread0_catalog_generation_program_v1`].
#[cfg(feature = "dynamic-mapped-runtime")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_catalog_generation_program_with_dynamic_mapped_v1(
    rdram: *mut u8,
    rdram_len: usize,
    install: CatalogGenerationInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) {
    let entry = install.resolver.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let live = set_catalog_generation_program(install, rdram_len);
    live.enable_dynamic_mapped_execution();
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        )
    };
}

/// Boot the canonical generation lane from an owned, validated bootstrap
/// allocation. Unlike the raw-pointer compatibility entry above, this API
/// retains allocation ownership inside HostState and initializes mutation
/// evidence from the bootstrap receipt before the first guest dispatch.
#[allow(clippy::too_many_arguments)]
pub fn boot_thread0_validated_catalog_generation_program_v1(
    validated: ValidatedBootstrapRdramV1,
    install: CatalogGenerationInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) -> Result<(), BootstrapImportErrorV1> {
    validate_bootstrap_binding(&validated, &install)?;
    let receipt = validated.receipt.evidence();
    let installed_rom = with_host(|host| host.installed_rom);
    if !installed_rom.is_some_and(|installed| {
        installed.byte_len == receipt.rom_byte_len && installed.sha256 == receipt.rom_sha256
    }) {
        return Err(BootstrapImportErrorV1::InstalledRomMismatch);
    }
    let entry = install.resolver.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let rdram_len = validated.storage.len();
    let live = set_catalog_program_parts(
        install.resolver,
        Some(install.generations),
        rdram_len,
        Some(&validated),
    );
    let ValidatedBootstrapRdramV1 { storage, .. } = validated;
    let (rdram, installed_len) = crate::host::install_owned_process_rdram(storage);
    assert_eq!(installed_len, rdram_len);
    // SAFETY: HostState now exclusively owns this stable allocation, no guest
    // coroutine has been created or resumed, and all reads finish before the
    // thread-0 constructor below. The validator also rejects any pending
    // writer event or open transaction in the actual live mutation owner.
    let installed_storage = unsafe { std::slice::from_raw_parts(rdram, installed_len) };
    live.mint_bootstrap_writer_completion(installed_storage)
        .unwrap_or_else(|error| panic!("minting bootstrap writer-channel authority: {error}"));
    // SAFETY: the just-installed owned allocation covers physical RDRAM and
    // cannot be moved while HostState retains it. This canonical typed-Rust
    // lane intercepts MMIO; the legacy generated-C sparse mirror is neither
    // read nor synchronized here.
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        );
    }
    Ok(())
}

/// Boot a validated, owned generation catalog with operational exact-unit
/// fallback for destinations absent from usable AOT.
///
/// This preserves the bootstrap, ROM, and executable-mutation provenance used
/// by the canonical generation lane, but deliberately does not mint static
/// writer-channel or release authority: dynamic execution is part of the
/// installed program from its first dispatch.
#[cfg(feature = "dynamic-mapped-runtime")]
#[allow(clippy::too_many_arguments)]
pub fn boot_thread0_validated_catalog_generation_program_with_dynamic_mapped_v1(
    validated: ValidatedBootstrapRdramV1,
    install: CatalogGenerationInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) -> Result<(), BootstrapImportErrorV1> {
    validate_bootstrap_binding(&validated, &install)?;
    let receipt = validated.receipt.evidence();
    let installed_rom = with_host(|host| host.installed_rom);
    if !installed_rom.is_some_and(|installed| {
        installed.byte_len == receipt.rom_byte_len && installed.sha256 == receipt.rom_sha256
    }) {
        return Err(BootstrapImportErrorV1::InstalledRomMismatch);
    }
    let entry = install.resolver.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let rdram_len = validated.storage.len();
    let live = set_catalog_program_parts(
        install.resolver,
        Some(install.generations),
        rdram_len,
        Some(&validated),
    );
    live.enable_dynamic_mapped_execution();
    let ValidatedBootstrapRdramV1 { storage, .. } = validated;
    let (rdram, installed_len) = crate::host::install_owned_process_rdram(storage);
    assert_eq!(installed_len, rdram_len);
    // SAFETY: HostState exclusively owns this stable allocation for the
    // lifetime of every executor coroutine created below.
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        );
    }
    Ok(())
}

/// Boot a validated, owned generation catalog while forcing its exact static
/// canonical entry once through the operational dynamic mapped executor.
///
/// The selected key must equal the install entry and resolve to itself in the
/// installed static catalog. The redirect remains armed across a zero-work
/// budget rejection and clears only after that attempted key charges work.
/// The immutable program and its identity are retained unchanged; withholding
/// is applied only at the unified dispatch seam and cannot mint static writer
/// or release authority.
#[cfg(feature = "dynamic-mapped-runtime")]
#[allow(clippy::too_many_arguments)]
pub fn boot_thread0_validated_catalog_generation_program_with_exact_static_key_withheld_v1(
    validated: ValidatedBootstrapRdramV1,
    install: CatalogGenerationInstallV1,
    withheld_static_key: ExecutionKey,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) -> Result<(), BootstrapImportErrorV1> {
    validate_bootstrap_binding(&validated, &install)?;
    let receipt = validated.receipt.evidence();
    let installed_rom = with_host(|host| host.installed_rom);
    if !installed_rom.is_some_and(|installed| {
        installed.byte_len == receipt.rom_byte_len && installed.sha256 == receipt.rom_sha256
    }) {
        return Err(BootstrapImportErrorV1::InstalledRomMismatch);
    }
    let entry = install.resolver.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let rdram_len = validated.storage.len();
    let live = set_catalog_program_parts(
        install.resolver,
        Some(install.generations),
        rdram_len,
        Some(&validated),
    );
    live.enable_dynamic_mapped_execution_with_exact_static_key_withheld(withheld_static_key);
    let ValidatedBootstrapRdramV1 { storage, .. } = validated;
    let (rdram, installed_len) = crate::host::install_owned_process_rdram(storage);
    assert_eq!(installed_len, rdram_len);
    // SAFETY: HostState exclusively owns this stable allocation for the
    // lifetime of every executor coroutine created below.
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
unsafe fn boot_thread0_catalog_live_v1(
    rdram: *mut u8,
    rdram_len: usize,
    live: CanonicalLiveBlockProgramV1,
    entry: ExecutionKey,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe { crate::register_process_rdram(rdram, rdram_len) };
    fn64_recomp_rs::set_host_pause(Some(pause_active_recompiled_thread));
    fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));

    let rdram_addr = rdram as usize;
    let boot_return_pc = boot_context.gprs[31] as u32;
    // The coroutine implementation transfers the closure onto its native
    // stack with a fixed 1 KiB object limit. Keep the growing canonical owner
    // behind one pointer so adding evidence fields cannot silently make boot
    // construction exceed that architectural transfer bound.
    let live = Rc::new(live);
    with_executor(|exec| {
        exec.restore_cp0_clock(
            boot_context.cp0.registers[9] as u32,
            boot_context.cp0.registers[11] as u32,
            boot_context.cp0.registers[13] & CpuInterruptLine::TIMER.cause_bit() as u64 != 0,
        );
        exec.create_thread(thread_id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(thread_id, rdram_ptr, yielder, || {
                let _ = first_input;
                // SAFETY: the boot host guarantees this one allocation
                // outlives every executor coroutine.
                let bytes = unsafe { std::slice::from_raw_parts_mut(rdram_ptr, rdram_len) };
                let mut mem = Rdram::new(bytes);
                let mut ctx = RsContext::new();
                ctx.restore_boot_context(&boot_context)
                    .unwrap_or_else(|error| panic!("restoring catalog BootContext: {error}"));
                validate_restored_catalog_boot_context(entry, &boot_context, &ctx);
                ctx.set_thread_return_pc(Some(boot_return_pc));
                run_catalog_block_program(live.as_ref(), entry, &mut ctx, &mut mem);
            });
        });
        exec.start_thread(thread_id);
    });
}

fn validate_block_boot_context(entry: GuestPc, boot_context: &BootContext) {
    boot_context
        .validate_for_entry(entry.get())
        .unwrap_or_else(|error| panic!("block-lane BootContext rejected: {error}"));
    let expected_tv_type = match boot_context.region.tv_standard {
        BootTvStandard::Ntsc => fn64_runtime::TvType::Ntsc,
        BootTvStandard::Pal => fn64_runtime::TvType::Pal,
        BootTvStandard::Mpal => fn64_runtime::TvType::Mpal,
    };
    with_host(|host| {
        let installed = host
            .installed_rom
            .unwrap_or_else(|| panic!("block-lane BootContext requires an installed ROM"));
        assert_eq!(
            installed.sha256,
            boot_context.normalized_rom_sha256.bytes(),
            "block-lane BootContext normalized ROM identity does not match the installed ROM"
        );
        assert_eq!(
            host.device_fabric.tv_type(),
            Some(expected_tv_type),
            "block-lane BootContext TV standard does not match the configured device fabric"
        );
    });
}

pub(super) fn validate_restored_catalog_boot_context(
    entry: ExecutionKey,
    boot_context: &BootContext,
    ctx: &RsContext,
) {
    assert_eq!(
        entry.pc.get(),
        boot_context.entry_pc,
        "catalog dispatch entry differs from the validated BootContext entry"
    );
    let mismatches = ctx
        .boot_context_state_mismatches(boot_context)
        .expect("validating restored catalog BootContext");
    assert!(
        mismatches.is_empty(),
        "catalog context differs from BootContext before first unified dispatch: {mismatches:?}"
    );
}

#[allow(clippy::too_many_arguments)]
unsafe fn boot_thread0_block_program_config(
    rdram: *mut u8,
    rdram_len: usize,
    program: BlockProgram,
    entry: ExecutionKey,
    boot_context: BootContext,
    entry_lookup: ProgramEntryLookup,
    transfer_lookup: ProgramTransferLookup,
    budget: InstructionBudget,
    dispatch_artifact_identity: Option<ProgramArtifactIdentity>,
    thread_id: ThreadId,
    priority: Priority,
) {
    validate_block_boot_context(entry.pc, &boot_context);

    let live = LiveBlockProgram {
        program: Rc::new(RefCell::new(program)),
        entry_lookup,
        transfer_lookup,
        budget,
        dispatch_artifact_identity,
        executable_regions: Rc::new(RefCell::new(Vec::new())),
        precompiled_generations: Rc::new(RefCell::new(None)),
    };
    set_block_program(live.clone(), rdram_len);
    unsafe { crate::register_process_rdram(rdram, rdram_len) };
    fn64_recomp_rs::set_host_pause(Some(pause_active_recompiled_thread));
    fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
    fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));

    let rdram_addr = rdram as usize;
    let boot_return_pc = boot_context.gprs[31] as u32;
    with_executor(|exec| {
        exec.restore_cp0_clock(
            boot_context.cp0.registers[9] as u32,
            boot_context.cp0.registers[11] as u32,
            boot_context.cp0.registers[13] & CpuInterruptLine::TIMER.cause_bit() as u64 != 0,
        );
        exec.create_thread(thread_id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(thread_id, rdram_ptr, yielder, || {
                let _ = first_input;
                // SAFETY: the boot host guarantees this one allocation
                // outlives every executor coroutine.
                let bytes = unsafe { std::slice::from_raw_parts_mut(rdram_ptr, rdram_len) };
                let mut mem = Rdram::new(bytes);
                let mut ctx = RsContext::new();
                ctx.restore_boot_context(&boot_context)
                    .unwrap_or_else(|error| panic!("restoring block-lane BootContext: {error}"));
                // IPL3 enters the ROM header with `jalr`, so a normal return
                // targets the captured `$ra` in SP DMEM rather than the
                // synthetic sentinel used for later OSThreads. That return
                // terminates the bootstrap coroutine; IPL3 is outside the
                // game AOT pack and must not be admitted as guest game code.
                ctx.set_thread_return_pc(Some(boot_return_pc));
                run_block_program(&live, entry, &mut ctx, &mut mem);
            });
        });
        exec.start_thread(thread_id);
    });
}

pub(super) fn park_host_scheduled_exception(
    canonical_live: Option<&CanonicalLiveBlockProgramV1>,
    fault: CpuFault,
    ctx: &mut RsContext,
) -> bool {
    let CpuFaultKind::Exception { exception, .. } = fault.kind else {
        return false;
    };
    let host_scheduled = crate::ACTIVE_THREAD_ID
        .with(|active| active.get())
        .is_some_and(|thread| with_host(|host| host.thread_handle_vrams.contains_key(&thread)));
    if std::env::var_os("FN64_PROFILE_EXCEPTIONS").is_some() {
        let active = crate::ACTIVE_THREAD_ID.with(|active| active.get());
        let handles = with_host(|host| host.thread_handle_vrams.clone());
        eprintln!(
            "[fn64-exception-profile] exception={exception:?} active={active:?} thread_handles={handles:?} host_scheduled={host_scheduled}"
        );
    }
    if !host_scheduled {
        return false;
    }

    fault.enter_exception(ctx).unwrap_or_else(|| {
        unreachable!("typed Exception fault must enter architectural exception state")
    });
    // Public libultra event numbering: BREAK is 10 and FAULT is 12. Missing
    // registration is valid for these synchronous events; the current thread
    // still stops, while a registered debugger/fault manager receives its
    // configured message through the executor's single queue path.
    let event = if exception == CpuException::Breakpoint {
        10
    } else {
        12
    };
    with_executor(|executor| {
        executor.inject_optional_os_event(event);
    });
    if let Some(live) = canonical_live {
        live.publish_parked_fault(fault, ctx);
    }
    let resumed = crate::suspend_active_coroutine(fn64_runtime::Yield::StopSelf);
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Recompiler,
        "recompiler.cpu.fault-context-resume",
        format!(
            "faulted guest thread was explicitly restarted after {exception:?} with {resumed:?}; resuming a saved fault context is not implemented"
        ),
        Some(fn64_runtime::Cycles::new(crate::sim_time())),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!(
        "faulted guest thread was explicitly restarted after {exception:?} with {resumed:?}; \
         resuming a saved fault context is not implemented"
    );
}
