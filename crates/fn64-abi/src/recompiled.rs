//! Typed-Rust recompiler adapters over the existing fn64 host ABI.
//!
//! The generated module stays `#![forbid(unsafe_code)]`: it calls ordinary
//! safe [`fn64_recomp_rs::RecompFunc`]s. Raw-pointer reconstruction is
//! confined here, beside the C ABI seam that already owns the identical
//! process-lifetime RDRAM and coroutine contracts.

use std::{cell::RefCell, rc::Rc};

use fn64_recomp_rs::{
    enter_pending_interrupt, BankId, BlockExit, BlockProgram, BlockProgramEvidenceSnapshot,
    CallResolution, CodeBank, CpuFault, CpuInterruptLine, ExecutableRegion,
    ExecutionDestinationObservation, ExecutionKey, FunctionEntryObservationSchema,
    GeneratedBankRunner, GenerationError, GuestPc, GuestWriteBoundary, GuestWriteEvent,
    InstructionBudget, PhysicalFgrState, ProgramArtifactIdentity, ProgramIdentityEvidenceSnapshot,
    ProgramIdentitySource, Rdram, RecompContext as RsContext, RecompFunc, TransferResolver,
    TranslatedFunctionIdentity,
};
use fn64_runtime::{Priority, ThreadId};

use super::{with_active_yielder, with_executor, with_host, RecompContext as CContext};

type Lookup = fn(u32) -> RecompFunc;
type CShim = unsafe extern "C" fn(*mut u8, *mut CContext);
const STATUS_FR: u32 = 1 << 26;
const THREAD_RETURN_SENTINEL: u32 = 0xFFFF_FFFC;
const INITIAL_FPCSR: u32 = crate::system::FPCSR_FS | crate::system::FPCSR_EV;
pub type ProgramEntryLookup = fn(GuestPc) -> Result<ExecutionKey, CpuFault>;
pub type ProgramTransferLookup = fn(BankId, GuestPc) -> Result<ExecutionKey, CpuFault>;
pub type LiveGenerationBuilder = fn(&[u8], u64) -> Result<(CodeBank, GeneratedBankRunner), String>;

struct ObservedExecutableRegion {
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    next_generation: u64,
    builder: LiveGenerationBuilder,
    builder_artifact_identity: Option<ProgramArtifactIdentity>,
}

thread_local! {
    static PENDING_EXECUTABLE_WRITES: RefCell<Vec<(u32, u32)>> = const {
        RefCell::new(Vec::new())
    };
    static EXECUTABLE_WRITE_RANGES: RefCell<Vec<(u32, u32)>> = const {
        RefCell::new(Vec::new())
    };
    static FUNCTION_LANE_ARTIFACT_IDENTITY: std::cell::Cell<Option<ProgramArtifactIdentity>> =
        const { std::cell::Cell::new(None) };
    static FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA:
        std::cell::Cell<Option<FunctionEntryObservationSchema>> =
        const { std::cell::Cell::new(None) };
    static FUNCTION_EXECUTION_DESTINATIONS:
        RefCell<Vec<FunctionExecutionDestinationObservation>> = const {
        RefCell::new(Vec::new())
    };
}

/// One successfully entered emitted whole-function destination. The artifact
/// identity and `(vram, symbol)` pair are pointer-independent; `at` is the
/// guest device cycle sampled before the first translated instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionExecutionDestinationObservation {
    pub at: fn64_runtime::Cycles,
    pub artifact_identity: ProgramArtifactIdentity,
    pub function: TranslatedFunctionIdentity,
}

/// One immutable arbitrary-PC program plus the active executable-mapping
/// resolvers shared by every guest coroutine in this runtime instance.
#[derive(Clone)]
pub(super) struct LiveBlockProgram {
    program: Rc<RefCell<BlockProgram>>,
    entry_lookup: ProgramEntryLookup,
    transfer_lookup: ProgramTransferLookup,
    budget: InstructionBudget,
    dispatch_artifact_identity: Option<ProgramArtifactIdentity>,
    executable_regions: Rc<RefCell<Vec<ObservedExecutableRegion>>>,
}

/// One canonical half-open physical write range awaiting executable-image
/// invalidation at the next host boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingExecutableWriteEvidenceSnapshot {
    pub physical_start: u32,
    pub physical_end: u32,
}

/// Canonical state of one dynamically replaceable executable region.
///
/// The builder and its native address are absent. `active_generation` is the
/// generation whose bank is installed now; `next_generation` is retained
/// because it determines the identity passed to the next pure builder call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveExecutableRegionEvidenceSnapshot {
    pub physical_start: u32,
    pub physical_end: u32,
    pub virtual_start: GuestPc,
    pub virtual_end: GuestPc,
    pub active_bank: BankId,
    pub active_generation: u64,
    pub next_generation: u64,
    pub builder_artifact_identity: ProgramArtifactIdentity,
}

/// Pointer-independent executable evidence for the active typed-Rust lane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecompiledProgramEvidenceSnapshot {
    /// Whole-function native callables. Their bodies are opaque at runtime,
    /// so only a producer-supplied artifact identity is authoritative.
    Function {
        identity: ProgramIdentityEvidenceSnapshot,
    },
    /// Bank-qualified arbitrary-PC execution with its complete code image and
    /// future-affecting dynamic-generation state.
    Block {
        program: BlockProgramEvidenceSnapshot,
        dispatch_artifact_identity: ProgramArtifactIdentity,
        instruction_budget: u32,
        executable_regions: Vec<LiveExecutableRegionEvidenceSnapshot>,
        pending_executable_writes: Vec<PendingExecutableWriteEvidenceSnapshot>,
    },
}

struct LiveTransferResolver {
    live: LiveBlockProgram,
}

impl TransferResolver for LiveTransferResolver {
    fn resolve(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.live.resolve_transfer(source_bank, target_pc)
    }

    fn resolve_call(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
        _resume: ExecutionKey,
    ) -> Result<CallResolution, CpuFault> {
        if fn64_recomp_rs::resolve_host_function(target_pc.get()).is_some() {
            Ok(CallResolution::Host)
        } else {
            self.resolve(source_bank, target_pc)
                .map(CallResolution::Guest)
        }
    }
}

impl LiveBlockProgram {
    fn resolve_transfer(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        if let Some(key) = self
            .executable_regions
            .borrow()
            .iter()
            .find_map(|observed| observed.region.resolve(target_pc))
        {
            return Ok(key);
        }
        (self.transfer_lookup)(source_bank, target_pc)
    }

    fn resolve_entry(&self, target_pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        if let Some(key) = self
            .executable_regions
            .borrow()
            .iter()
            .find_map(|observed| observed.region.resolve(target_pc))
        {
            return Ok(key);
        }
        (self.entry_lookup)(target_pc)
    }
}

fn canonical_pending_executable_writes() -> Vec<PendingExecutableWriteEvidenceSnapshot> {
    let mut writes = PENDING_EXECUTABLE_WRITES.with(|pending| {
        pending
            .borrow()
            .iter()
            .map(|&(physical_start, len)| {
                assert!(len > 0, "pending executable write has zero length");
                let physical_end = physical_start
                    .checked_add(len)
                    .expect("pending executable write exceeds physical address space");
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start,
                    physical_end,
                }
            })
            .collect::<Vec<_>>()
    });
    writes.sort_unstable_by_key(|write| (write.physical_start, write.physical_end));
    let mut canonical: Vec<PendingExecutableWriteEvidenceSnapshot> = Vec::new();
    for write in writes {
        if let Some(previous) = canonical.last_mut() {
            if write.physical_start <= previous.physical_end {
                previous.physical_end = previous.physical_end.max(write.physical_end);
                continue;
            }
        }
        canonical.push(write);
    }
    canonical
}

/// Capture the installed typed-Rust program without runner, resolver,
/// builder, lookup, or native function-pointer values.
///
/// The legacy function install API remains executable for compatibility, but
/// it is intentionally not evidence-capable: callers must use
/// [`set_entry_lookup_with_artifact_identity`] (or the matching boot helper)
/// before this function will describe a function lane. This prevents section
/// geometry or process-specific pointer bits from impersonating code identity.
pub fn recompiled_program_evidence_snapshot() -> Option<RecompiledProgramEvidenceSnapshot> {
    let (function_lane, block_lane) = with_host(|host| {
        (
            host.recompiled_lookup.is_some(),
            host.recompiled_program.clone(),
        )
    });
    assert!(
        !(function_lane && block_lane.is_some()),
        "function and block recompiled lanes are installed simultaneously"
    );
    if function_lane {
        let identity = FUNCTION_LANE_ARTIFACT_IDENTITY
            .with(std::cell::Cell::get)
            .unwrap_or_else(|| {
                panic!(
                    "function-lane release evidence requires a stable host-provided artifact identity"
                )
            });
        return Some(RecompiledProgramEvidenceSnapshot::Function {
            identity: ProgramIdentityEvidenceSnapshot {
                identity,
                source: ProgramIdentitySource::CallerSupplied,
            },
        });
    }
    let live = block_lane?;
    let program = live.program.borrow().evidence_snapshot();
    let dispatch_artifact_identity = live.dispatch_artifact_identity.unwrap_or_else(|| {
        panic!(
            "block-lane release evidence requires a stable host-provided dispatch artifact identity"
        )
    });
    let mut executable_regions = live
        .executable_regions
        .borrow()
        .iter()
        .map(|observed| {
            let active_bank = observed.region.active_bank().unwrap_or_else(|| {
                panic!("observed executable region has no active bank during evidence capture")
            });
            let active_generation = observed
                .next_generation
                .checked_sub(1)
                .expect("observed executable region has no active generation");
            let builder_artifact_identity = observed.builder_artifact_identity.unwrap_or_else(|| {
                panic!(
                    "executable-region release evidence requires a stable host-provided builder artifact identity"
                )
            });
            LiveExecutableRegionEvidenceSnapshot {
                physical_start: observed.physical_start,
                physical_end: observed.physical_end,
                virtual_start: observed.region.start(),
                virtual_end: observed.region.end(),
                active_bank,
                active_generation,
                next_generation: observed.next_generation,
                builder_artifact_identity,
            }
        })
        .collect::<Vec<_>>();
    executable_regions.sort_unstable_by_key(|region| {
        (
            region.physical_start,
            region.physical_end,
            region.virtual_start,
            region.virtual_end,
        )
    });
    Some(RecompiledProgramEvidenceSnapshot::Block {
        program,
        dispatch_artifact_identity,
        instruction_budget: live.budget.get(),
        executable_regions,
        pending_executable_writes: canonical_pending_executable_writes(),
    })
}

/// Copy successfully entered arbitrary-PC destinations in exact runner-entry
/// order. An empty vector means either that no block lane is installed or that
/// its admitted program has not executed; callers select the authoritative
/// interpretation from [`recompiled_program_evidence_snapshot`].
pub fn copy_block_execution_destinations() -> Vec<ExecutionDestinationObservation> {
    let live = with_host(|host| host.recompiled_program.clone());
    live.map_or_else(Vec::new, |live| {
        live.program.borrow().copy_execution_destinations()
    })
}

/// Copy successfully entered emitted whole-function destinations in exact
/// entry order. An installed legacy function lane fails closed: only the API
/// which consumes the generated artifact's observation-schema marker can make
/// this history authoritative.
pub fn copy_function_execution_destinations() -> Vec<FunctionExecutionDestinationObservation> {
    let function_lane = with_host(|host| host.recompiled_lookup.is_some());
    if !function_lane {
        return Vec::new();
    }
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|schema| {
        schema.get().unwrap_or_else(|| {
            panic!(
                "function-lane destination evidence requires the generated artifact's entry-observation schema"
            )
        });
    });
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| destinations.borrow().clone())
}

fn observe_function_entry(function: TranslatedFunctionIdentity) {
    let artifact_identity = FUNCTION_LANE_ARTIFACT_IDENTITY
        .with(std::cell::Cell::get)
        .unwrap_or_else(|| {
            panic!("observed function entry has no stable generated-artifact identity")
        });
    let at = fn64_runtime::Cycles::new(crate::sim_time());
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| {
        destinations
            .borrow_mut()
            .push(FunctionExecutionDestinationObservation {
                at,
                artifact_identity,
                function,
            });
    });
}

fn observe_renderer_write(event: GuestWriteEvent) {
    if let GuestWriteEvent::NonRdpWrite16 {
        logical_offset,
        value,
    } = event
    {
        super::task_dispatch::observe_non_rdp_write16(logical_offset, value);
    }
}

fn record_executable_and_renderer_write(event: GuestWriteEvent) {
    let (offset, len) = event.range();
    PENDING_EXECUTABLE_WRITES.with(|writes| writes.borrow_mut().push((offset, len)));
    observe_renderer_write(event);
}

fn classify_live_executable_write(event: GuestWriteEvent) -> GuestWriteBoundary {
    let (start, len) = event.range();
    let end = start.saturating_add(len);
    if EXECUTABLE_WRITE_RANGES.with(|ranges| {
        ranges
            .borrow()
            .iter()
            .any(|&(physical_start, physical_end)| start < physical_end && end > physical_start)
    }) {
        GuestWriteBoundary::ExecutableChanged
    } else {
        GuestWriteBoundary::Continue
    }
}

fn process_executable_writes(
    live: &LiveBlockProgram,
    mut read_logical_byte: impl FnMut(u32) -> u8,
) -> Vec<BankId> {
    let writes =
        PENDING_EXECUTABLE_WRITES.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
    if writes.is_empty() {
        return Vec::new();
    }
    let mut regions = live.executable_regions.borrow_mut();
    let mut program = live.program.borrow_mut();
    let mut retired = Vec::new();
    for observed in regions.iter_mut() {
        let touched = writes.iter().any(|&(start, len)| {
            let end = start.saturating_add(len);
            start < observed.physical_end && end > observed.physical_start
        });
        if !touched {
            continue;
        }
        let bytes = (observed.physical_start..observed.physical_end)
            .map(&mut read_logical_byte)
            .collect::<Vec<_>>();
        let generation = observed.next_generation;
        let (code, runner) = (observed.builder)(&bytes, generation).unwrap_or_else(|error| {
            panic!(
                "executable rewrite [{:#010x}, {:#010x}) generation {generation} could not be translated: {error}",
                observed.physical_start, observed.physical_end
            )
        });
        if let Some(previous) = observed
            .region
            .install(&mut program, code, runner)
            .unwrap_or_else(|error| panic!("executable generation install failed: {error}"))
        {
            retired.push(previous);
        }
        observed.next_generation = observed
            .next_generation
            .checked_add(1)
            .expect("executable generation counter overflow");
    }
    retired
}

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
        host.recompiled_rdram_len = rdram_len;
    });
}

fn set_block_program(program: LiveBlockProgram, rdram_len: usize) {
    assert!(rdram_len > 0, "recompiled RDRAM length must be nonzero");
    PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow_mut().clear());
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| destinations.borrow_mut().clear());
    FUNCTION_LANE_ARTIFACT_IDENTITY.with(|installed| installed.set(None));
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|installed| installed.set(None));
    fn64_recomp_rs::set_function_entry_observer(None);
    fn64_recomp_rs::set_unsupported_observer(Some(record_recompiled_unsupported));
    fn64_recomp_rs::set_guest_write_boundary_observer(Some(classify_live_executable_write));
    with_host(|host| {
        host.recompiled_lookup = None;
        host.recompiled_program = Some(program);
        host.recompiled_rdram_len = rdram_len;
    });
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
    register_live_executable_region_config(physical_start, physical_end, region, builder, None);
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
    );
}

fn register_live_executable_region_config(
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    builder: LiveGenerationBuilder,
    builder_artifact_identity: Option<ProgramArtifactIdentity>,
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
    });
    EXECUTABLE_WRITE_RANGES.with(|ranges| {
        ranges.borrow_mut().push((physical_start, physical_end));
    });
}

/// Apply DMA-originated executable writes after the device fabric has
/// committed all bytes, but before it publishes completion messages or any
/// guest coroutine can resume.
pub(crate) fn process_live_executable_writes_from_host() {
    let live = with_host(|host| host.recompiled_program.clone());
    let Some(live) = live else {
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
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
    TestExecutableWritePreflightState {
        prior_ranges,
        prior_pending,
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
    super::suspend_active_coroutine(fn64_runtime::Yield::PauseSelf);
}

fn read_raw_mmio(vaddr: u64) -> Option<u32> {
    crate::pi::read_raw_mmio_word(vaddr)
}

fn write_raw_mmio(vaddr: u64, value: u32) -> bool {
    crate::pi::write_raw_mmio_word(vaddr, value)
}

fn record_recompiled_unsupported(context: &str) {
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Recompiler,
        "recompiler.cpu.unsupported-instruction",
        context,
        Some(fn64_runtime::Cycles::new(crate::sim_time())),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
}

fn recompiled_gap_panic(context: impl Into<String>) -> ! {
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
/// exactly like [`super::boot_thread0`]'s existing C ABI contract.
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
    unsafe { super::register_process_rdram(rdram, rdram_len) };
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
/// # Safety
/// `rdram` must address `rdram_len` live bytes for every coroutine's lifetime,
/// exactly like [`boot_thread0`]'s existing shared-allocation contract.
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_block_program(
    rdram: *mut u8,
    rdram_len: usize,
    program: BlockProgram,
    entry: ExecutionKey,
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
            entry_lookup,
            transfer_lookup,
            budget,
            Some(dispatch_artifact_identity),
            thread_id,
            priority,
        )
    };
}

#[allow(clippy::too_many_arguments)]
unsafe fn boot_thread0_block_program_config(
    rdram: *mut u8,
    rdram_len: usize,
    program: BlockProgram,
    entry: ExecutionKey,
    entry_lookup: ProgramEntryLookup,
    transfer_lookup: ProgramTransferLookup,
    budget: InstructionBudget,
    dispatch_artifact_identity: Option<ProgramArtifactIdentity>,
    thread_id: ThreadId,
    priority: Priority,
) {
    let live = LiveBlockProgram {
        program: Rc::new(RefCell::new(program)),
        entry_lookup,
        transfer_lookup,
        budget,
        dispatch_artifact_identity,
        executable_regions: Rc::new(RefCell::new(Vec::new())),
    };
    set_block_program(live.clone(), rdram_len);
    unsafe { super::register_process_rdram(rdram, rdram_len) };
    fn64_recomp_rs::set_host_pause(Some(pause_active_recompiled_thread));
    fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
    fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));

    let rdram_addr = rdram as usize;
    with_executor(|exec| {
        exec.create_thread(thread_id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(thread_id, rdram_ptr, yielder, || {
                let _ = first_input;
                // SAFETY: the boot host guarantees this one allocation
                // outlives every executor coroutine.
                let bytes = unsafe { std::slice::from_raw_parts_mut(rdram_ptr, rdram_len) };
                let mut mem = Rdram::new(bytes);
                let mut ctx = RsContext::new();
                ctx.set_r32(31, THREAD_RETURN_SENTINEL as i32);
                ctx.set_thread_return_pc(Some(THREAD_RETURN_SENTINEL));
                run_block_program(&live, entry, &mut ctx, &mut mem);
            });
        });
        exec.start_thread(thread_id);
    });
}

fn run_block_program(
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
        let (count, compare, timer_pending) = with_executor(|executor| {
            (
                executor.cp0_count(),
                executor.cp0_compare(),
                executor.cp0_timer_pending(),
            )
        });
        ctx.synchronize_cop0_timing(count, compare);
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
                .dispatch(entry, live.budget, ctx, mem, &mut resolver)
                .unwrap_or_else(|error| {
                    recompiled_gap_panic(format!(
                        "live BlockProgram dispatch failed at {entry}: {error}"
                    ))
                })
        };
        process_executable_writes(live, |offset| {
            mem.load_b(0xFFFF_FFFF_8000_0000u64 + u64::from(offset)) as u8
        });
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
            super::suspend_active_coroutine(fn64_runtime::Yield::InstructionCheckpoint {
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
                        let host = fn64_recomp_rs::resolve_host_function(target_pc.get())
                            .unwrap_or_else(|| {
                                recompiled_gap_panic(format!(
                                    "live BlockProgram requested unknown host call {:#010x}",
                                    target_pc.get()
                                ))
                            });
                        host(ctx, mem);
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
            BlockExit::HostCall { vram, resume } => {
                let host = fn64_recomp_rs::resolve_host_function(vram.get()).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "live BlockProgram requested unknown host call {:#010x}",
                        vram.get()
                    ))
                });
                host(ctx, mem);
                entry = live
                    .resolve_transfer(resume.bank, resume.pc)
                    .unwrap_or_else(|fault| {
                        recompiled_gap_panic(format!(
                            "live BlockProgram host resume {resume} no longer resolves: {fault:?}"
                        ))
                    });
            }
            BlockExit::ThreadReturn => return,
            BlockExit::Fault(fault) => recompiled_gap_panic(format!(
                "live BlockProgram stopped on unresolved guest fault: {fault:?}"
            )),
            BlockExit::Transfer(_)
            | BlockExit::ResolveTransfer { .. }
            | BlockExit::ResolveCall { .. } => {
                unreachable!("BlockProgram::dispatch returned an internal transfer boundary")
            }
        }
    }
}

/// Dispatch a newly-created OSThread through the installed typed module.
/// Returns `false` only for the legacy C configuration.
///
/// # Safety
/// `rdram` carries the same process-lifetime allocation contract as
/// `osCreateThread_recomp` and `recompiled::boot_thread0`.
pub(super) unsafe fn run_registered_entry(
    rdram: *mut u8,
    entry_vram: u32,
    arg: u64,
    sp: u64,
) -> bool {
    let (program, registered) = with_host(|host| {
        (
            host.recompiled_program.clone(),
            host.recompiled_lookup
                .map(|lookup| (lookup, host.recompiled_rdram_len)),
        )
    });
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
        let mut ctx = new_osthread_context();
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
    let mut ctx = new_osthread_context();
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

fn c_from_recompiled(ctx: &RsContext) -> CContext {
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

fn copy_c_back(c: &CContext, ctx: &mut RsContext) {
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
    ]
    .into_iter()
    .any(|allowed| std::ptr::fn_addr_eq(allowed, shim))
}

fn is_admitted_fr_stable_c_shim(shim: CShim) -> bool {
    is_generated_adapter_c_shim(shim)
        || [
            super::__osInitialize_common_recomp as CShim,
            super::osInitialize_recomp as CShim,
            super::__osInitialize_msp_recomp as CShim,
            super::__osInitialize_kmc_recomp as CShim,
            super::__osInitialize_isv_recomp as CShim,
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

fn call_c(ctx: &mut RsContext, mem: &mut Rdram<'_>, name: &'static str, shim: CShim) {
    // An exit snapshot cannot observe a shim which changes FR, accesses the
    // other FPR view, then restores FR. Admit only the closed host-shim set
    // whose implementations preserve FR for the entire call.
    assert!(
        is_admitted_fr_stable_c_shim(shim),
        "C shim {name} is not in the FR-stable adapter registry"
    );
    if std::env::var_os("FN64_RECOMP_RS_SHIM_TRACE").is_some() {
        eprintln!("[fn64-recomp-rs-shim] {name}");
    }
    let mut c = c_from_recompiled(ctx);
    // `f_odd` aliases this stack-local context, so arm it only after the C
    // image has reached its stable address.
    c.arm_fpr_alias();
    let entry_fr = c.mips3_float_mode;
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
    copy_c_back(&c, ctx);
}

/// Construct the architectural context installed by public `osCreateThread`.
/// The libultra `osCreateThread` manual's DESCRIPTION section specifies that
/// every new thread starts with denormal-result flushing and Invalid exceptions
/// enabled. Keeping this in the context makes coroutine suspension itself the
/// FCSR save/restore boundary.
fn new_osthread_context() -> RsContext {
    let mut ctx = RsContext::new();
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
        super::__osInitialize_common_recomp,
    );
}

pub fn os_initialize(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(ctx, mem, "osInitialize_recomp", super::osInitialize_recomp);
}

pub fn os_initialize_msp(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(
        ctx,
        mem,
        "__osInitialize_msp_recomp",
        super::__osInitialize_msp_recomp,
    );
}

pub fn os_initialize_kmc(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(
        ctx,
        mem,
        "__osInitialize_kmc_recomp",
        super::__osInitialize_kmc_recomp,
    );
}

pub fn os_initialize_isv(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(
        ctx,
        mem,
        "__osInitialize_isv_recomp",
        super::__osInitialize_isv_recomp,
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
        fn64_recomp_rs::trap_unsupported(
            "__osSetFpcCsr wrote an enabled FCSR cause through a host-call boundary",
        );
    }
}

macro_rules! c_adapters {
    ($(($recompiled:ident, $shim:ident)),+ $(,)?) => {
        fn is_generated_adapter_c_shim(shim: CShim) -> bool {
            false $(|| std::ptr::fn_addr_eq(shim, super::$shim as CShim))+
        }

        $(
            pub fn $recompiled(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
                call_c(ctx, mem, stringify!($shim), super::$shim);
            }
        )+
    };
}

c_adapters!(
    (is_prout_sync_printf, is_proutSyncPrintf_recomp),
    (check_hardware_msp, __checkHardware_msp_recomp),
    (check_hardware_kmc, __checkHardware_kmc_recomp),
    (check_hardware_isv, __checkHardware_isv_recomp),
    (os_rdb_send, __osRdbSend_recomp),
    (os_create_thread, osCreateThread_recomp),
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

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_recomp_rs::{
        run_bank, BlockRun, CodeBank, CodeCatalog, CpuFaultKind, GeneratedBankRunner,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    static TRANSIENT_FR_SHIM_ENTERED: AtomicBool = AtomicBool::new(false);

    const LIVE_BANK: BankId = BankId::new(0xA11CE);
    const LIVE_SECOND_BANK: BankId = BankId::new(0xA11CF);
    const LIVE_ENTRY: GuestPc = GuestPc::new(0x8000_1000);
    const LIVE_NEXT: GuestPc = GuestPc::new(0x8000_1004);
    const LIVE_HOST: GuestPc = GuestPc::new(0x8000_2000);
    const IRQ_BANK: BankId = BankId::new(0x1A2);
    const IRQ_ENTRY: GuestPc = GuestPc::new(0x8000_0100);
    const IRQ_RESUME: GuestPc = GuestPc::new(0x8000_0104);
    const IRQ_VECTOR: GuestPc = GuestPc::new(0x8000_0180);
    const TIMER_BANK: BankId = BankId::new(0x1A7);
    const REWRITE_OLD_BANK: BankId = BankId::new(0xC0DE_0000);
    const REWRITE_NEW_BANK: BankId = BankId::new(0xC0DE_0001);
    const REWRITE_ENTRY: GuestPc = GuestPc::new(0x8000_3000);
    const REWRITE_RESUME: GuestPc = GuestPc::new(REWRITE_ENTRY.get() + 0x24);
    const REWRITE_PHYSICAL: u32 = 0x80;
    const REWRITE_A_WORDS: [u32; 13] = [
        0x3c09_8000, // lui t1, 0x8000
        0x240c_0055, // addiu t4, zero, 0x55
        0xad2c_0020, // sw t4, 0x20(t1) -- non-executable store
        0x240d_0066, // addiu t5, zero, 0x66
        0xad2d_0024, // sw t5, 0x24(t1) -- proves ordinary stores do not split
        0x240a_0001, // addiu t2, zero, 1 -- prepare the post-store sentinel
        0x3c08_1122, // lui t0, 0x1122
        0x3508_3344, // ori t0, t0, 0x3344
        0xad28_0080, // sw t0, 0x80(t1) -- replaces this executable image
        0xad2a_0010, // generation-A post-store sentinel
        0x03e0_0008, // jr ra
        0,
        0,
    ];
    const REWRITE_B_WORDS: [u32; 13] = [
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0x240b_0002, // addiu t3, zero, 2
        0xad2b_0014, // sw t3, 0x14(t1)
        0x03e0_0008, // jr ra
        0,
    ];
    const DMA_OLD_BANK: BankId = BankId::new(0xD00D_0000);
    const DMA_NEW_BANK: BankId = BankId::new(0xD00D_0001);
    const DMA_ENTRY: GuestPc = GuestPc::new(0x8000_4000);
    const DMA_PHYSICAL: u32 = 0x100;

    thread_local! {
        static LIVE_ACTIVE_BANK: std::cell::Cell<BankId> = const { std::cell::Cell::new(LIVE_BANK) };
        static REWRITE_BUILDS: std::cell::RefCell<Vec<(u64, Vec<u8>)>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static REWRITE_B_ENTRIES: std::cell::RefCell<Vec<ExecutionKey>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static BOOT_FPCSR_OBSERVATIONS: std::cell::RefCell<Vec<u32>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }

    fn evidence_callable(_ctx: &mut RsContext, _mem: &mut Rdram<'_>) {}

    fn alternate_evidence_callable(_ctx: &mut RsContext, _mem: &mut Rdram<'_>) {}

    fn evidence_lookup(_vram: u32) -> RecompFunc {
        evidence_callable
    }

    fn alternate_evidence_lookup(_vram: u32) -> RecompFunc {
        alternate_evidence_callable
    }

    fn observe_thread0_fpcsr_boot(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        BOOT_FPCSR_OBSERVATIONS.with(|observed| observed.borrow_mut().push(ctx.read_fcr(31)));
        os_initialize(ctx, mem);
        BOOT_FPCSR_OBSERVATIONS.with(|observed| observed.borrow_mut().push(ctx.read_fcr(31)));
    }

    fn unused_evidence_builder(
        _bytes: &[u8],
        _generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        Err("evidence-only builder must not run".to_string())
    }

    fn alternate_unused_evidence_builder(
        _bytes: &[u8],
        _generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        Err("alternate evidence-only builder must not run".to_string())
    }

    fn install_evidence_block_lane(budget: u32, reverse_regions: bool, alternate_builders: bool) {
        let first_bank = BankId::new(0xE100);
        let second_bank = BankId::new(0xE200);
        let first_start = GuestPc::new(0x8000_5000);
        let second_start = GuestPc::new(0x8000_6000);
        let mut program = BlockProgram::new();
        let mut first_region =
            ExecutableRegion::new(first_start, GuestPc::new(first_start.get() + 4));
        let mut second_region =
            ExecutableRegion::new(second_start, GuestPc::new(second_start.get() + 4));
        let runner_artifact = ProgramArtifactIdentity::new([0xE5; 32]);
        first_region
            .install(
                &mut program,
                CodeBank::new(first_bank, first_start, vec![0x1111_2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    first_bank,
                    live_test_runner,
                    runner_artifact,
                ),
            )
            .unwrap();
        second_region
            .install(
                &mut program,
                CodeBank::new(second_bank, second_start, vec![0x3333_4444]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    second_bank,
                    live_test_runner,
                    runner_artifact,
                ),
            )
            .unwrap();
        let live = LiveBlockProgram {
            program: Rc::new(RefCell::new(program)),
            entry_lookup: live_entry_lookup,
            transfer_lookup: live_transfer_lookup,
            budget: InstructionBudget::new(budget).unwrap(),
            dispatch_artifact_identity: Some(ProgramArtifactIdentity::new([0xD1; 32])),
            executable_regions: Rc::new(RefCell::new(Vec::new())),
        };
        set_block_program(live, 0x100);
        let first_builder = if alternate_builders {
            alternate_unused_evidence_builder
        } else {
            unused_evidence_builder
        };
        let registrations = [
            (0x20, 0x24, first_region, first_builder),
            (0x40, 0x44, second_region, unused_evidence_builder),
        ];
        if reverse_regions {
            for (start, end, region, builder) in registrations.into_iter().rev() {
                register_live_executable_region_with_artifact_identity(
                    start,
                    end,
                    region,
                    builder,
                    ProgramArtifactIdentity::new([0xB1; 32]),
                );
            }
        } else {
            for (start, end, region, builder) in registrations {
                register_live_executable_region_with_artifact_identity(
                    start,
                    end,
                    region,
                    builder,
                    ProgramArtifactIdentity::new([0xB1; 32]),
                );
            }
        }
    }

    #[test]
    fn translated_cpu_unsupported_gap_records_the_typed_release_event() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        record_recompiled_unsupported("unsupported COP0 register 7");

        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].subsystem,
            fn64_runtime::UnsupportedSubsystem::Recompiler
        );
        assert_eq!(
            events[0].operation,
            concat!("recompiler.cpu.", "unsupported-instruction")
        );
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        assert!(events[0].guest_cycle.is_some());
    }

    #[test]
    fn function_lane_evidence_requires_identity_and_excludes_callable_pointers() {
        with_host(|host| *host = super::super::HostState::default());
        set_entry_lookup(evidence_lookup, 0x100);
        let missing = std::panic::catch_unwind(recompiled_program_evidence_snapshot)
            .expect_err("unidentified function lane must fail evidence capture");
        let message = missing
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| missing.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("stable host-provided artifact identity"));

        let identity = ProgramArtifactIdentity::new([0xA5; 32]);
        set_entry_lookup_with_artifact_identity(evidence_lookup, 0x100, identity);
        let first = recompiled_program_evidence_snapshot().unwrap();
        set_entry_lookup_with_artifact_identity(alternate_evidence_lookup, 0x100, identity);
        assert_eq!(first, recompiled_program_evidence_snapshot().unwrap());

        set_entry_lookup_with_artifact_identity(
            evidence_lookup,
            0x100,
            ProgramArtifactIdentity::new([0x5A; 32]),
        );
        assert_ne!(first, recompiled_program_evidence_snapshot().unwrap());
    }

    #[test]
    fn function_destination_history_binds_artifact_function_cycle_and_schema() {
        with_host(|host| *host = super::super::HostState::default());
        let identity = ProgramArtifactIdentity::new([0xC3; 32]);
        set_entry_lookup_with_execution_observation(
            evidence_lookup,
            0x100,
            identity,
            fn64_recomp_rs::FUNCTION_ENTRY_OBSERVATION_SCHEMA,
        );
        with_executor(|executor| executor.set_sim_time(37));
        fn64_recomp_rs::notify_function_entry(TranslatedFunctionIdentity::new(
            0x8000_1000,
            "entry",
        ));
        with_executor(|executor| executor.set_sim_time(41));
        fn64_recomp_rs::notify_function_entry(TranslatedFunctionIdentity::new(
            0x8000_2000,
            "callee",
        ));

        assert_eq!(
            copy_function_execution_destinations(),
            vec![
                FunctionExecutionDestinationObservation {
                    at: fn64_runtime::Cycles::new(37),
                    artifact_identity: identity,
                    function: TranslatedFunctionIdentity::new(0x8000_1000, "entry"),
                },
                FunctionExecutionDestinationObservation {
                    at: fn64_runtime::Cycles::new(41),
                    artifact_identity: identity,
                    function: TranslatedFunctionIdentity::new(0x8000_2000, "callee"),
                },
            ]
        );

        set_entry_lookup_with_artifact_identity(evidence_lookup, 0x100, identity);
        let stale = std::panic::catch_unwind(copy_function_execution_destinations)
            .expect_err("identity-only function install must not claim a complete history");
        let message = stale
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| stale.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("entry-observation schema"));
    }

    #[test]
    fn block_lane_evidence_sorts_regions_and_excludes_builder_pointers() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        PENDING_EXECUTABLE_WRITES
            .with(|pending| *pending.borrow_mut() = vec![(0x42, 2), (0x20, 2), (0x21, 3)]);
        let forward = recompiled_program_evidence_snapshot().unwrap();

        install_evidence_block_lane(8, true, true);
        PENDING_EXECUTABLE_WRITES
            .with(|pending| *pending.borrow_mut() = vec![(0x21, 3), (0x42, 2), (0x20, 2)]);
        let reverse = recompiled_program_evidence_snapshot().unwrap();
        assert_eq!(forward, reverse);

        let RecompiledProgramEvidenceSnapshot::Block {
            instruction_budget,
            executable_regions,
            pending_executable_writes,
            ..
        } = forward
        else {
            panic!("block install captured as function lane")
        };
        assert_eq!(instruction_budget, 8);
        assert_eq!(
            executable_regions
                .iter()
                .map(|region| region.physical_start)
                .collect::<Vec<_>>(),
            vec![0x20, 0x40]
        );
        assert_eq!(
            pending_executable_writes,
            vec![
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x20,
                    physical_end: 0x24,
                },
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x42,
                    physical_end: 0x44,
                },
            ]
        );
    }

    #[test]
    fn block_destination_copy_api_reads_the_live_program_history() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        assert!(copy_block_execution_destinations().is_empty());
        let live = with_host(|host| {
            host.recompiled_program
                .clone()
                .expect("evidence fixture installs a live block program")
        });
        let entry = ExecutionKey::new(BankId::new(0xE100), GuestPc::new(0x8000_5000));
        let mut bytes = [0u8; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        live.program.borrow().run(
            entry,
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(
            copy_block_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: entry,
                runner_artifact_identity: Some(ProgramArtifactIdentity::new([0xE5; 32])),
            }]
        );
    }

    #[test]
    fn block_lane_evidence_binds_budget_region_generation_and_pending_writes() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        let baseline = recompiled_program_evidence_snapshot().unwrap();

        install_evidence_block_lane(12, false, false);
        let changed_budget = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_budget);

        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        live.executable_regions.borrow_mut()[0].next_generation = 2;
        let changed_generation = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_generation);

        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        {
            let mut regions = live.executable_regions.borrow_mut();
            regions[0].physical_start += 4;
            regions[0].physical_end += 4;
        }
        let changed_region_geometry = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_region_geometry);

        install_evidence_block_lane(8, false, false);
        with_host(|host| {
            host.recompiled_program
                .as_mut()
                .unwrap()
                .dispatch_artifact_identity = Some(ProgramArtifactIdentity::new([0xD2; 32]));
        });
        let changed_dispatch_artifact = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_dispatch_artifact);

        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        live.executable_regions.borrow_mut()[0].builder_artifact_identity =
            Some(ProgramArtifactIdentity::new([0xB2; 32]));
        let changed_builder_artifact = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_builder_artifact);

        install_evidence_block_lane(8, false, false);
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x30, 4)));
        let changed_pending = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_pending);
    }

    #[test]
    #[should_panic(expected = "pending executable write has zero length")]
    fn block_lane_evidence_never_omits_malformed_pending_write() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x30, 0)));
        let _ = recompiled_program_evidence_snapshot();
    }

    #[test]
    #[should_panic(expected = "stable host-provided dispatch artifact identity")]
    fn block_lane_evidence_rejects_unidentified_dispatch_artifact() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        with_host(|host| {
            host.recompiled_program
                .as_mut()
                .unwrap()
                .dispatch_artifact_identity = None;
        });
        let _ = recompiled_program_evidence_snapshot();
    }

    #[test]
    #[should_panic(expected = "stable host-provided builder artifact identity")]
    fn block_lane_evidence_rejects_unidentified_builder_artifact() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        live.executable_regions.borrow_mut()[0].builder_artifact_identity = None;
        let _ = recompiled_program_evidence_snapshot();
    }

    #[test]
    fn verified_host_write_preflight_rejects_only_executable_overlap() {
        let _state = scoped_test_executable_write_preflight_state(vec![(0x100, 0x180)], Vec::new());

        assert_eq!(
            preflight_non_executable_host_writes(&[(0x80, 0x100)]),
            Ok(())
        );
        let overlap = preflight_non_executable_host_writes(&[(0x17f, 0x181)]).unwrap_err();
        assert!(overlap.contains("overlaps live executable region"));
        assert!(overlap.contains("transactional executable publication is unavailable"));

        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x120, 4)));
        let pending = preflight_non_executable_host_writes(&[]).unwrap_err();
        assert!(pending.contains("pending host write"));
    }

    #[test]
    fn executable_write_preflight_test_scope_restores_on_unwind() {
        let _outer =
            scoped_test_executable_write_preflight_state(vec![(0x20, 0x40)], vec![(0x24, 4)]);

        let panic = std::panic::catch_unwind(|| {
            let _inner = scoped_test_executable_write_preflight_state(
                vec![(0x100, 0x180)],
                vec![(0x120, 8)],
            );
            assert_eq!(
                EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow().clone()),
                vec![(0x100, 0x180)]
            );
            assert_eq!(
                PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
                vec![(0x120, 8)]
            );
            panic!("expected test-scope unwind");
        });

        assert!(panic.is_err());
        assert_eq!(
            EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow().clone()),
            vec![(0x20, 0x40)]
        );
        assert_eq!(
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            vec![(0x24, 4)]
        );
    }

    #[test]
    fn typed_halfword_write_multiplexes_invalidation_and_renderer_once() {
        use std::{cell::Cell, rc::Rc};

        struct CountBackend(Rc<Cell<u32>>);

        impl fn64_render::RenderBackend for CountBackend {
            fn create(
                &mut self,
                _cfg: &fn64_render::RenderConfig,
            ) -> Result<(), fn64_render::RenderError> {
                Ok(())
            }

            fn observe_non_rdp_write16(
                &mut self,
                _write: fn64_render::NonRdpWrite16,
            ) -> fn64_render::NonRdpWrite16Disposition {
                self.0.set(self.0.get() + 1);
                fn64_render::NonRdpWrite16Disposition::AppliedHiddenSidecar
            }

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<fn64_render::FrameStatus, fn64_render::RenderError> {
                Ok(fn64_render::FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), fn64_render::RenderError> {
                Ok(())
            }

            fn resize(&mut self, _width: u32, _height: u32) {}

            fn supported_ucodes(&self) -> &[fn64_render::UcodeId] {
                &[]
            }
        }

        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let renderer_calls = Rc::new(Cell::new(0));
        crate::set_render_backend(Box::new(CountBackend(renderer_calls.clone())), 0x100);
        let previous =
            fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));
        let mut bytes = [0u8; 0x100];
        Rdram::new(&mut bytes).store_h(0xffff_ffff_a000_0040, 0x1235);
        fn64_recomp_rs::set_write_observer(previous);

        assert_eq!(
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            vec![(0x40, 2)]
        );
        assert_eq!(renderer_calls.get(), 1);
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    }

    fn unmapped(bank: BankId, pc: GuestPc, start: GuestPc, end: GuestPc) -> CpuFault {
        CpuFault {
            at: ExecutionKey::new(bank, pc),
            kind: CpuFaultKind::UnmappedPc {
                bank_start: start.get(),
                bank_end: end.get(),
            },
        }
    }

    fn rewrite_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        Err(unmapped(
            REWRITE_OLD_BANK,
            pc,
            REWRITE_ENTRY,
            GuestPc::new(REWRITE_ENTRY.get() + 0x34),
        ))
    }

    fn rewrite_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        rewrite_lookup(pc)
    }

    fn rewrite_interpreter_runner(
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let words = match entry.bank {
            REWRITE_OLD_BANK => REWRITE_A_WORDS,
            REWRITE_NEW_BANK => {
                REWRITE_B_ENTRIES.with(|entries| entries.borrow_mut().push(entry));
                REWRITE_B_WORDS
            }
            bank => {
                return BlockRun::new(
                    BlockExit::Fault(unmapped(
                        bank,
                        entry.pc,
                        REWRITE_ENTRY,
                        GuestPc::new(REWRITE_ENTRY.get() + 0x34),
                    )),
                    0,
                );
            }
        };
        let mut catalog = CodeCatalog::new();
        catalog
            .register(CodeBank::new(entry.bank, REWRITE_ENTRY, words.to_vec()).unwrap())
            .unwrap();
        let run =
            run_bank(&catalog, entry.bank, entry, budget, ctx, mem).unwrap_or_else(|unsupported| {
                panic!("rewrite interpreter hit unsupported op: {unsupported:?}")
            });
        match run.exit {
            BlockExit::ResolveTransfer { target_pc, .. }
                if ctx.is_thread_return(target_pc.get()) =>
            {
                BlockRun::new(BlockExit::ThreadReturn, run.instructions)
            }
            _ => run,
        }
    }

    fn rewrite_builder(
        bytes: &[u8],
        generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().push((generation, bytes.to_vec())));
        let expected = std::iter::once(0x1122_3344)
            .chain(REWRITE_A_WORDS.into_iter().skip(1))
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        if generation != 1 || bytes != expected {
            return Err(format!(
                "unexpected CPU rewrite generation/image: {generation} {bytes:02x?}"
            ));
        }
        Ok((
            CodeBank::new(REWRITE_NEW_BANK, REWRITE_ENTRY, REWRITE_B_WORDS.to_vec())
                .map_err(|error| error.to_string())?,
            GeneratedBankRunner::new(REWRITE_NEW_BANK, rewrite_interpreter_runner),
        ))
    }

    fn dma_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        Err(unmapped(
            DMA_OLD_BANK,
            pc,
            DMA_ENTRY,
            GuestPc::new(DMA_ENTRY.get() + 8),
        ))
    }

    fn dma_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        dma_lookup(pc)
    }

    fn dma_rewrite_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match (entry.bank, entry.pc) {
            (DMA_OLD_BANK, DMA_ENTRY) => {
                mem.store_w(0xFFFF_FFFF_A460_0000, DMA_PHYSICAL);
                mem.store_w(0xFFFF_FFFF_A460_0004, 0x20);
                mem.store_w(0xFFFF_FFFF_A460_0008, 7);
                BlockRun::new(BlockExit::Checkpoint(entry), 5)
            }
            (DMA_NEW_BANK, DMA_ENTRY) => {
                mem.store_w(0xFFFF_FFFF_8000_0014, 0xD00D_0001);
                BlockRun::new(
                    BlockExit::Transfer(ExecutionKey::new(
                        DMA_NEW_BANK,
                        GuestPc::new(DMA_ENTRY.get() + 4),
                    )),
                    1,
                )
            }
            (DMA_NEW_BANK, pc) if pc == GuestPc::new(DMA_ENTRY.get() + 4) => {
                mem.store_w(0xFFFF_FFFF_8000_0018, 0xD00D_0002);
                BlockRun::new(BlockExit::ThreadReturn, 1)
            }
            (bank, pc) => BlockRun::new(
                BlockExit::Fault(unmapped(
                    bank,
                    pc,
                    DMA_ENTRY,
                    GuestPc::new(DMA_ENTRY.get() + 8),
                )),
                0,
            ),
        }
    }

    fn dma_rewrite_builder(
        bytes: &[u8],
        generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().push((generation, bytes.to_vec())));
        if generation != 1 || bytes != [0x3c, 0x08, 0x12, 0x34, 0x35, 0x08, 0x56, 0x78] {
            return Err(format!(
                "unexpected DMA rewrite generation/image: {generation} {bytes:02x?}"
            ));
        }
        Ok((
            CodeBank::new(DMA_NEW_BANK, DMA_ENTRY, vec![1, 1])
                .map_err(|error| error.to_string())?,
            GeneratedBankRunner::new(DMA_NEW_BANK, dma_rewrite_runner),
        ))
    }

    fn live_host(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        ctx.set_r32(2, 0x1234);
        mem.store_w(0xFFFF_FFFF_8000_0000, ctx.r_u32(2));
    }

    fn live_host_lookup(vram: u32) -> Option<RecompFunc> {
        (vram == LIVE_HOST.get()).then_some(live_host)
    }

    fn live_test_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            LIVE_ENTRY => BlockRun::new(
                BlockExit::ResolveCall {
                    source_bank: LIVE_BANK,
                    target_pc: LIVE_HOST,
                    resume: ExecutionKey::new(LIVE_BANK, LIVE_NEXT),
                },
                3,
            ),
            LIVE_NEXT => BlockRun::new(BlockExit::ThreadReturn, 2),
            pc => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(LIVE_BANK, pc),
                    kind: CpuFaultKind::UnmappedPc {
                        bank_start: LIVE_ENTRY.get(),
                        bank_end: LIVE_NEXT.get() + 4,
                    },
                }),
                0,
            ),
        }
    }

    fn live_entry_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        let key = ExecutionKey::new(LIVE_ACTIVE_BANK.with(std::cell::Cell::get), pc);
        if matches!(pc, LIVE_ENTRY | LIVE_NEXT) {
            Ok(key)
        } else {
            Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: LIVE_ENTRY.get(),
                    bank_end: LIVE_NEXT.get() + 4,
                },
            })
        }
    }

    fn live_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        live_entry_lookup(pc)
    }

    fn irq_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            IRQ_ENTRY => {
                ctx.set_r(4, 0x0010_0401); // OS_IM_PI
                os_set_int_mask(ctx, mem);
                mem.store_w(0xFFFF_FFFF_A460_0000, 0x400);
                mem.store_w(0xFFFF_FFFF_A460_0004, 0x20);
                mem.store_w(0xFFFF_FFFF_A460_0008, 3);
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(IRQ_BANK, IRQ_RESUME)),
                    5,
                )
            }
            IRQ_VECTOR => {
                mem.store_w(0xFFFF_FFFF_8000_0000, ctx.cop0_epc);
                mem.store_w(0xFFFF_FFFF_8000_0004, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0008, ctx.cop0_status);
                mem.store_w(0xFFFF_FFFF_A460_0010, 1 << 1); // clear PI interrupt
                let resume = GuestPc::new(ctx.exception_return_pc());
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(IRQ_BANK, resume)),
                    2,
                )
            }
            IRQ_RESUME => {
                mem.store_w(0xFFFF_FFFF_8000_000C, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0010, ctx.cop0_status);
                BlockRun::new(BlockExit::ThreadReturn, 2)
            }
            pc => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(IRQ_BANK, pc),
                    kind: CpuFaultKind::UnmappedPc {
                        bank_start: IRQ_ENTRY.get(),
                        bank_end: IRQ_VECTOR.get() + 4,
                    },
                }),
                0,
            ),
        }
    }

    fn irq_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        let key = ExecutionKey::new(IRQ_BANK, pc);
        if matches!(pc, IRQ_ENTRY | IRQ_RESUME | IRQ_VECTOR) {
            Ok(key)
        } else {
            Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: IRQ_ENTRY.get(),
                    bank_end: IRQ_VECTOR.get() + 4,
                },
            })
        }
    }

    fn irq_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        irq_lookup(pc)
    }

    fn timer_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            IRQ_ENTRY => {
                ctx.cop0_status = 1 | CpuInterruptLine::TIMER.cause_bit();
                ctx.write_cop0(9, 0);
                ctx.write_cop0(11, 2);
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(TIMER_BANK, IRQ_RESUME)),
                    4,
                )
            }
            IRQ_VECTOR => {
                mem.store_w(0xFFFF_FFFF_8000_0020, ctx.cop0_epc);
                mem.store_w(0xFFFF_FFFF_8000_0024, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0028, ctx.cop0_count);
                ctx.write_cop0(11, ctx.cop0_compare);
                let resume = GuestPc::new(ctx.exception_return_pc());
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(TIMER_BANK, resume)),
                    2,
                )
            }
            IRQ_RESUME => {
                mem.store_w(0xFFFF_FFFF_8000_002C, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0030, ctx.cop0_count);
                BlockRun::new(BlockExit::ThreadReturn, 2)
            }
            pc => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(TIMER_BANK, pc),
                    kind: CpuFaultKind::UnmappedPc {
                        bank_start: IRQ_ENTRY.get(),
                        bank_end: IRQ_VECTOR.get() + 4,
                    },
                }),
                0,
            ),
        }
    }

    fn timer_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        let key = ExecutionKey::new(TIMER_BANK, pc);
        if matches!(pc, IRQ_ENTRY | IRQ_RESUME | IRQ_VECTOR) {
            Ok(key)
        } else {
            Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: IRQ_ENTRY.get(),
                    bank_end: IRQ_VECTOR.get() + 4,
                },
            })
        }
    }

    fn timer_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        timer_lookup(pc)
    }

    #[test]
    fn c_adapter_round_trips_all_gprs_and_forces_zero() {
        let mut recompiled = RsContext::new();
        for i in 1..32 {
            recompiled.set_r(i, 0xA000_0000_0000_0000 | i as u64);
        }
        let mut c = c_from_recompiled(&recompiled);
        c.r0 = u64::MAX;
        c.r2 = 0x1234;
        copy_c_back(&c, &mut recompiled);
        assert_eq!(recompiled.r(0), 0);
        assert_eq!(recompiled.r(2), 0x1234);
        assert_eq!(recompiled.r(31), 0xA000_0000_0000_001F);
    }

    pub(super) unsafe extern "C" fn no_op_fpr_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` supplies its live stack-local C context.
        let ctx = unsafe { &mut *ctx };
        ctx.assert_float_mode_matches_status();
        let expected = if ctx.mips3_float_mode == 0 {
            // Safety: taking a union field address does not read that field.
            unsafe { &mut ctx.f0.u32_halves.1 as *mut u32 }
        } else {
            // Safety: taking a union field address does not read that field.
            unsafe { &mut ctx.f1.u32_halves.0 as *mut u32 }
        };
        assert_eq!(ctx.f_odd, expected);
    }

    pub(super) unsafe extern "C" fn write_f5_word_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` arms `f_odd` for this live context. N64Recomp's
        // generated odd-register expression for f5 is `(5 - 1) * 2`.
        unsafe { *(*ctx).f_odd.add(8) = 0xDEAD_BEEF };
    }

    pub(super) unsafe extern "C" fn change_fr_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` supplies its live stack-local C context.
        let ctx = unsafe { &mut *ctx };
        ctx.status_reg ^= STATUS_FR;
        ctx.mips3_float_mode ^= 1;
        ctx.arm_fpr_alias();
    }

    unsafe extern "C" fn transient_fr_write_shim(_rdram: *mut u8, ctx: *mut CContext) {
        TRANSIENT_FR_SHIM_ENTERED.store(true, Ordering::SeqCst);
        // Safety: the regression deliberately models a raw ABI shim which
        // changes to the other FPR view, accesses it, then restores the entry
        // mode before returning.
        let ctx = unsafe { &mut *ctx };
        let entry_status = ctx.status_reg;
        let entry_mode = ctx.mips3_float_mode;
        ctx.status_reg ^= STATUS_FR;
        ctx.mips3_float_mode ^= 1;
        ctx.arm_fpr_alias();
        // Safety: `arm_fpr_alias` made this pointer live for the transient
        // view. The generated odd-register expression for f5 is `(5-1)*2`.
        unsafe { *ctx.f_odd.add(8) = 0xA11C_E55E };
        ctx.status_reg = entry_status;
        ctx.mips3_float_mode = entry_mode;
        ctx.arm_fpr_alias();
    }

    fn patterned_fgr_state(tag: u64) -> PhysicalFgrState {
        PhysicalFgrState::from_words(std::array::from_fn(|idx| {
            let high = (tag >> 32) as u32 ^ (0x0101_0000 + idx as u32);
            let low = tag as u32 ^ (0x0000_0101 + idx as u32);
            (u64::from(high) << 32) | u64::from(low)
        }))
    }

    #[test]
    fn c_adapter_layout_is_reversible_and_mode_exact() {
        let physical = patterned_fgr_state(0xA5A5_5A5A_DEAD_BEEF);
        let words = physical.into_words();
        for fr in [false, true] {
            let mut source = RsContext::new();
            source.cop0_status = if fr { STATUS_FR } else { 0 };
            source.replace_physical_fgr_state(physical);
            let c = c_from_recompiled(&source);
            c.assert_float_mode_matches_status();
            let image = c.fpr_u64_bits();
            if fr {
                assert_eq!(image, words);
            } else {
                for pair in 0..16 {
                    let even = pair * 2;
                    let odd = even + 1;
                    assert_eq!(
                        image[even],
                        u64::from(words[even] as u32) | (u64::from(words[odd] as u32) << 32)
                    );
                    assert_eq!(
                        image[odd],
                        (words[even] >> 32) | (words[odd] & 0xFFFF_FFFF_0000_0000)
                    );
                }
            }

            let mut restored = RsContext::new();
            copy_c_back(&c, &mut restored);
            assert_eq!(restored.physical_fgr_state(), physical);
            assert_eq!(restored.cop0_status & STATUS_FR != 0, fr);
        }
    }

    #[test]
    fn c_adapter_noop_preserves_every_physical_fgr_in_both_fr_modes() {
        for fr in [false, true] {
            let expected = patterned_fgr_state(if fr {
                0xA5A5_5A5A_DEAD_BEEF
            } else {
                0x1122_3344_5566_7788
            });
            let mut ctx = RsContext::new();
            ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            ctx.replace_physical_fgr_state(expected);
            let mut bytes = [];
            let mut mem = Rdram::new(&mut bytes);

            call_c(&mut ctx, &mut mem, "no_op_fpr_shim", no_op_fpr_shim);

            assert_eq!(ctx.physical_fgr_state(), expected, "FR={fr}");
            assert_eq!(ctx.cop0_status & STATUS_FR != 0, fr);
        }
    }

    #[test]
    fn c_adapter_f_odd_write_targets_physical_fgr5_in_both_modes() {
        for fr in [false, true] {
            let initial = patterned_fgr_state(0x1234_5678_9ABC_DEF0).into_words();
            let mut ctx = RsContext::new();
            ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            let mut bytes = [];
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "write_f5_word_shim",
                write_f5_word_shim,
            );
            let mut expected = initial;
            expected[5] = (expected[5] & 0xFFFF_FFFF_0000_0000) | 0xDEAD_BEEF;
            assert_eq!(ctx.physical_fgr_state().into_words(), expected, "FR={fr}");
        }
    }

    #[test]
    fn c_adapter_rejects_an_fr_transition_before_decoding_entry_view_bytes() {
        let expected = patterned_fgr_state(0x0BAD_F00D_CAFE_BABE);
        let mut ctx = RsContext::new();
        ctx.replace_physical_fgr_state(expected);
        let mut bytes = [];
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "change_fr_shim",
                change_fr_shim,
            );
        }));
        assert!(rejected.is_err());
        assert_eq!(ctx.cop0_status & STATUS_FR, 0);
        assert_eq!(ctx.physical_fgr_state(), expected);
    }

    #[test]
    fn c_adapter_rejects_a_transient_fr_transition_before_the_shim_runs() {
        TRANSIENT_FR_SHIM_ENTERED.store(false, Ordering::SeqCst);
        let expected = patterned_fgr_state(0x1357_9BDF_2468_ACE0);
        let mut ctx = RsContext::new();
        ctx.replace_physical_fgr_state(expected);
        let mut bytes = [];
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "transient_fr_write_shim",
                transient_fr_write_shim,
            );
        }));
        let panic = rejected.expect_err("unadmitted transient-FR shim must be rejected");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("registry rejection must use a string panic payload");
        assert!(
            message.contains("is not in the FR-stable adapter registry"),
            "unexpected rejection: {message}"
        );
        assert!(!TRANSIENT_FR_SHIM_ENTERED.load(Ordering::SeqCst));
        assert_eq!(ctx.cop0_status & STATUS_FR, 0);
        assert_eq!(ctx.physical_fgr_state(), expected);
    }

    #[test]
    fn c_adapter_float_helpers_return_through_f0_in_both_fr_modes() {
        let value = 0xFEDC_BA98_7654_3210u64;
        for fr in [false, true] {
            let initial = patterned_fgr_state(0xC001_D00D_A55A_5AA5).into_words();

            let mut float_ctx = RsContext::new();
            float_ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            float_ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            float_ctx.set_r(4, value >> 32);
            float_ctx.set_r(5, value as u32 as u64);
            let mut float_bytes = [];
            ull_to_f(&mut float_ctx, &mut Rdram::new(&mut float_bytes));
            assert_eq!(float_ctx.f_bits(0), (value as f32).to_bits(), "FR={fr}");
            let mut expected_float = initial;
            expected_float[0] =
                (expected_float[0] & 0xFFFF_FFFF_0000_0000) | u64::from((value as f32).to_bits());
            assert_eq!(
                float_ctx.physical_fgr_state().into_words(),
                expected_float,
                "FR={fr} float result changed non-result state"
            );

            let mut double_ctx = RsContext::new();
            double_ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            double_ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            double_ctx.set_r(4, value >> 32);
            double_ctx.set_r(5, value as u32 as u64);
            let mut double_bytes = [];
            ull_to_d(&mut double_ctx, &mut Rdram::new(&mut double_bytes));
            let result = (value as f64).to_bits();
            assert_eq!(double_ctx.d_bits(0), result, "FR={fr}");
            let mut expected_double = initial;
            if fr {
                expected_double[0] = result;
            } else {
                expected_double[0] =
                    (expected_double[0] & 0xFFFF_FFFF_0000_0000) | u64::from(result as u32);
                expected_double[1] = (expected_double[1] & 0xFFFF_FFFF_0000_0000) | (result >> 32);
            }
            assert_eq!(
                double_ctx.physical_fgr_state().into_words(),
                expected_double,
                "FR={fr} double result changed non-result state"
            );
        }
    }

    #[test]
    fn live_block_program_owns_thread_dispatch_and_charges_instruction_time() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x100];
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(LIVE_ENTRY, GuestPc::new(LIVE_NEXT.get() + 4));
        LIVE_ACTIVE_BANK.with(|active| active.set(LIVE_BANK));
        region
            .install(
                &mut program,
                CodeBank::new(LIVE_BANK, LIVE_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new(LIVE_BANK, live_test_runner),
            )
            .unwrap();
        let thread_id = 0xB10C;
        let previous_host_lookup = fn64_recomp_rs::set_host_lookup(Some(live_host_lookup));

        // SAFETY: `bytes` remains live until the installed thread has
        // returned and the executor marks it dead below.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(LIVE_BANK, LIVE_ENTRY),
                live_entry_lookup,
                live_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 3);
        assert!(!crate::is_thread_dead(thread_id));
        LIVE_ACTIVE_BANK.with(|active| active.set(LIVE_SECOND_BANK));
        assert_eq!(
            install_live_block_generation(
                &mut region,
                CodeBank::new(LIVE_SECOND_BANK, LIVE_ENTRY, vec![1, 1]).unwrap(),
                GeneratedBankRunner::new(LIVE_SECOND_BANK, live_test_runner),
            )
            .unwrap(),
            Some(LIVE_BANK)
        );
        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        assert!(!crate::is_thread_dead(thread_id));
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0x1234);
        fn64_recomp_rs::set_host_lookup(previous_host_lookup);
    }

    #[test]
    fn interpreter_cpu_store_retires_generation_before_its_next_instruction() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().clear());
        REWRITE_B_ENTRIES.with(|entries| entries.borrow_mut().clear());
        let mut bytes = vec![0u8; 0x200];
        fn64_recomp_rs::set_write_observer(None);
        fn64_recomp_rs::set_guest_write_boundary_observer(None);
        {
            let mut mem = Rdram::new(&mut bytes);
            for (index, word) in REWRITE_A_WORDS.into_iter().enumerate() {
                mem.store_w(
                    0xFFFF_FFFF_8000_0000
                        | u64::from(REWRITE_PHYSICAL + u32::try_from(index * 4).unwrap()),
                    word,
                );
            }
        }
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(
            REWRITE_ENTRY,
            GuestPc::new(REWRITE_ENTRY.get() + u32::try_from(REWRITE_A_WORDS.len() * 4).unwrap()),
        );
        region
            .install(
                &mut program,
                CodeBank::new(REWRITE_OLD_BANK, REWRITE_ENTRY, REWRITE_A_WORDS.to_vec()).unwrap(),
                GeneratedBankRunner::new(REWRITE_OLD_BANK, rewrite_interpreter_runner),
            )
            .unwrap();
        let thread_id = 0xC0DE;

        // SAFETY: `bytes` remains live until this thread returns below.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(REWRITE_OLD_BANK, REWRITE_ENTRY),
                rewrite_lookup,
                rewrite_transfer_lookup,
                InstructionBudget::new(13).unwrap(),
                thread_id,
                10,
            );
        }
        register_live_executable_region(
            REWRITE_PHYSICAL,
            REWRITE_PHYSICAL + u32::try_from(REWRITE_A_WORDS.len() * 4).unwrap(),
            region,
            rewrite_builder,
        );

        assert!(crate::run_one_step());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0020) as u32, 0x55);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0024) as u32, 0x66);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_8000_0010) as u32,
            0,
            "generation A executed its post-store sentinel before invalidation"
        );
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0014) as u32, 0);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        assert!(live
            .program
            .borrow()
            .code()
            .bank(REWRITE_OLD_BANK)
            .is_none());
        assert!(live
            .program
            .borrow()
            .code()
            .bank(REWRITE_NEW_BANK)
            .is_some());
        assert_eq!(
            live.resolve_entry(REWRITE_ENTRY).unwrap().bank,
            REWRITE_NEW_BANK
        );
        assert_eq!(
            live.resolve_entry(REWRITE_RESUME).unwrap(),
            ExecutionKey::new(REWRITE_NEW_BANK, REWRITE_RESUME)
        );
        assert_eq!(
            REWRITE_BUILDS.with(|builds| builds.borrow().clone()),
            vec![(
                1,
                std::iter::once(0x1122_3344)
                    .chain(REWRITE_A_WORDS.into_iter().skip(1))
                    .flat_map(u32::to_be_bytes)
                    .collect::<Vec<_>>()
            )]
        );
        assert!(REWRITE_B_ENTRIES.with(|entries| entries.borrow().is_empty()));

        assert!(crate::run_one_step());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0010) as u32, 0);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0014) as u32, 2);
        assert_eq!(
            REWRITE_B_ENTRIES.with(|entries| entries.borrow().clone()),
            vec![ExecutionKey::new(REWRITE_NEW_BANK, REWRITE_RESUME)]
        );
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn pi_dma_rebuilds_executable_region_before_completion_is_observable() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().clear());
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x28].copy_from_slice(&[0x3c, 0x08, 0x12, 0x34, 0x35, 0x08, 0x56, 0x78]);
        crate::load_rom_with_fixed_pi_latency(rom, 5);
        let mut bytes = vec![0u8; 0x200];
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(DMA_ENTRY, GuestPc::new(DMA_ENTRY.get() + 8));
        region
            .install(
                &mut program,
                CodeBank::new(DMA_OLD_BANK, DMA_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new(DMA_OLD_BANK, dma_rewrite_runner),
            )
            .unwrap();
        let thread_id = 0xD00D;

        // SAFETY: `bytes` remains live until this thread returns below.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(DMA_OLD_BANK, DMA_ENTRY),
                dma_lookup,
                dma_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }
        register_live_executable_region(
            DMA_PHYSICAL,
            DMA_PHYSICAL + 8,
            region,
            dma_rewrite_builder,
        );

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        assert!(live.program.borrow().code().bank(DMA_OLD_BANK).is_none());
        assert_eq!(live.resolve_entry(DMA_ENTRY).unwrap().bank, DMA_NEW_BANK);
        assert_eq!(
            REWRITE_BUILDS.with(|builds| builds.borrow().clone()),
            vec![(1, vec![0x3c, 0x08, 0x12, 0x34, 0x35, 0x08, 0x56, 0x78])]
        );

        assert!(crate::run_one_step());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0014) as u32, 0xD00D_0001);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_8000_0018) as u32,
            0xD00D_0002,
            "the already-serviced DMA boundary split generation B's first turn"
        );
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn checkpoint_due_pi_enters_ip2_handler_before_the_next_guest_block() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        crate::load_rom_with_fixed_pi_latency(rom, 5);
        let mut bytes = vec![0u8; 0x1000];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(IRQ_BANK, IRQ_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new(IRQ_BANK, irq_runner),
            )
            .unwrap();
        let thread_id = 0x1A2;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(IRQ_BANK, IRQ_ENTRY),
                irq_lookup,
                irq_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400) as u32, 0x1234_5678);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0);
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 7);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000) as u32, IRQ_RESUME.get());
            assert_eq!((mem.load_w(0xFFFF_FFFF_8000_0004) as u32 >> 2) & 0x1F, 0);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_8000_0004) as u32 & CpuInterruptLine::RCP.cause_bit(),
                0
            );
            assert_ne!(mem.load_w(0xFFFF_FFFF_8000_0008) as u32 & (1 << 1), 0);
        }

        assert!(crate::run_one_step());
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_8000_000C) as u32 & CpuInterruptLine::RCP.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0010) as u32 & (1 << 1), 0);
        }
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn checkpoint_count_compare_match_enters_ip7_and_compare_write_acks_it() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x100];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(TIMER_BANK, IRQ_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new(TIMER_BANK, timer_runner),
            )
            .unwrap();
        let thread_id = 0x1A7;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(TIMER_BANK, IRQ_ENTRY),
                timer_lookup,
                timer_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 4);
        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 6);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0020) as u32, IRQ_RESUME.get());
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_8000_0024) as u32 & CpuInterruptLine::TIMER.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0028) as u32, 2);
        }

        assert!(crate::run_one_step());
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_8000_002C) as u32 & CpuInterruptLine::TIMER.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0030) as u32, 3);
        }
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn status_adapters_are_per_context_state() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        ctx.set_r(4, 0x3400_0001);
        os_set_sr(&mut ctx, &mut mem);
        ctx.set_r(2, 0);
        os_get_sr(&mut ctx, &mut mem);
        assert_eq!(ctx.r_u32(2), 0x3400_0001);
    }

    #[test]
    fn typed_fpcsr_setter_and_new_thread_use_the_generated_cop1_authority() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut first = new_osthread_context();
        let mut second = new_osthread_context();

        assert_eq!(first.read_fcr(31), INITIAL_FPCSR);
        assert_eq!(second.read_fcr(31), INITIAL_FPCSR);

        first.set_r(4, 3);
        os_set_fpc_csr(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), INITIAL_FPCSR);
        assert_eq!(first.read_fcr(31), 3);
        assert_eq!(second.read_fcr(31), INITIAL_FPCSR);

        second.set_r(4, 2);
        os_set_fpc_csr(&mut second, &mut mem);
        assert_eq!(second.r_u32(2), INITIAL_FPCSR);
        assert_eq!(second.read_fcr(31), 2);
        assert_eq!(first.read_fcr(31), 3);

        let pending: u32 = (1 << 16) | (1 << 11);
        first.set_r(4, u64::from(pending));
        let loud = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            os_set_fpc_csr(&mut first, &mut mem);
        }));
        assert!(
            loud.is_err(),
            "enabled Cause written by host call must stay loud"
        );
        assert_eq!(first.r_u32(2), 3);
        assert_eq!(first.read_fcr(31), pending);
        assert_eq!(second.read_fcr(31), 2);
    }

    /// Public osCreateThread gives each OSThread its own saved FPCSR. This
    /// drives real executor coroutine suspension and alternates A/B/A/B/A/B;
    /// the context-local values must survive switches through another thread.
    #[test]
    fn alternating_osthread_coroutines_preserve_independent_fpcsr() {
        const THREAD_A: ThreadId = 0xF5A0;
        const THREAD_B: ThreadId = 0xF5B0;

        let observed_a = Rc::new(RefCell::new(Vec::new()));
        let observed_b = Rc::new(RefCell::new(Vec::new()));
        let observed_a_body = Rc::clone(&observed_a);
        let observed_b_body = Rc::clone(&observed_b);

        with_executor(|exec| {
            exec.create_thread(THREAD_A, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = new_osthread_context();
                ctx.write_fcr(31, 3);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
                ctx.write_fcr(31, 1);
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
            });
            exec.create_thread(THREAD_B, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = new_osthread_context();
                ctx.write_fcr(31, 2);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
                ctx.write_fcr(31, 0);
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
            });
            exec.start_thread(THREAD_A);
            exec.start_thread(THREAD_B);
        });

        for _ in 0..6 {
            assert!(crate::run_one_step());
        }

        assert_eq!(&*observed_a.borrow(), &[3, 3, 1]);
        assert_eq!(&*observed_b.borrow(), &[2, 2, 0]);
        with_executor(|exec| {
            assert!(exec.is_thread_dead(THREAD_A));
            assert!(exec.is_thread_dead(THREAD_B));
        });
    }

    /// Thread 0 is the reset context, not an osCreateThread context. The
    /// public osInitialize contract performs the observable 0 -> FS|EV
    /// transition at the real typed boot entry.
    #[test]
    fn thread0_boot_path_transitions_fpcsr_only_at_os_initialize() {
        const THREAD0: ThreadId = 0xF500;
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom_with_fixed_pi_latency(Vec::new(), 1);
        BOOT_FPCSR_OBSERVATIONS.with(|observed| observed.borrow_mut().clear());
        let mut bytes = [0u8; 8];

        unsafe {
            boot_thread0(
                bytes.as_mut_ptr(),
                bytes.len(),
                evidence_lookup,
                observe_thread0_fpcsr_boot,
                THREAD0,
                10,
            );
        }
        crate::run_to_idle();

        BOOT_FPCSR_OBSERVATIONS.with(|observed| {
            assert_eq!(&*observed.borrow(), &[0, INITIAL_FPCSR]);
        });
        assert!(crate::is_thread_dead(THREAD0));
    }

    #[test]
    fn typed_os_initialize_replaces_the_current_context_fpcsr() {
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom_with_fixed_pi_latency(Vec::new(), 1);
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        ctx.write_fcr(31, 3);

        os_initialize(&mut ctx, &mut mem);

        assert_eq!(ctx.read_fcr(31), INITIAL_FPCSR);
    }

    #[test]
    fn alternating_osthread_coroutines_preserve_all_physical_fgr_bits() {
        const THREAD_A: ThreadId = 0xF5C0;
        const THREAD_B: ThreadId = 0xF5D0;
        let state_a = patterned_fgr_state(0x1111_2222_3333_4444);
        let state_b = patterned_fgr_state(0xAAAA_BBBB_CCCC_DDDD);
        let observed_a = Rc::new(RefCell::new(Vec::new()));
        let observed_b = Rc::new(RefCell::new(Vec::new()));
        let observed_a_body = Rc::clone(&observed_a);
        let observed_b_body = Rc::clone(&observed_b);

        with_executor(|exec| {
            exec.create_thread(THREAD_A, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = RsContext::new();
                ctx.cop0_status &= !STATUS_FR;
                ctx.replace_physical_fgr_state(state_a);
                observed_a_body.borrow_mut().push(ctx.physical_fgr_state());
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.physical_fgr_state());
            });
            exec.create_thread(THREAD_B, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = RsContext::new();
                ctx.cop0_status |= STATUS_FR;
                ctx.replace_physical_fgr_state(state_b);
                observed_b_body.borrow_mut().push(ctx.physical_fgr_state());
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.physical_fgr_state());
            });
            exec.start_thread(THREAD_A);
            exec.start_thread(THREAD_B);
        });

        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::run_one_step());

        assert_eq!(&*observed_a.borrow(), &[state_a, state_a]);
        assert_eq!(&*observed_b.borrow(), &[state_b, state_b]);
        with_executor(|exec| {
            assert!(exec.is_thread_dead(THREAD_A));
            assert!(exec.is_thread_dead(THREAD_B));
        });
    }

    #[test]
    fn typed_interrupt_masks_return_each_contexts_own_previous_value() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut first = RsContext::new();
        let mut second = RsContext::new();

        first.set_r(4, 0x0010_0401);
        os_set_int_mask(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), 0);
        second.set_r(4, 0x0008_0401);
        os_set_int_mask(&mut second, &mut mem);
        assert_eq!(second.r_u32(2), 0);
        first.set_r(4, 0x0004_0401);
        os_set_int_mask(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), 0x0010_0401);
    }

    #[test]
    fn typed_raw_word_accesses_and_sp_shims_share_one_device_fabric_state() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);

        mem.store_w(0xFFFF_FFFF_A408_0000, 0x0A8);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A408_0000) as u32, 0x0A8);

        let mut set = CContext::zeroed();
        set.r4 = 1 << 10;
        unsafe { crate::__osSpSetStatus_recomp(std::ptr::null_mut(), &mut set) };
        assert_eq!(mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & (1 << 7), 1 << 7);

        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_sp_dma_replaces_persistent_imem_on_guest_time() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let mut bytes = vec![0u8; 0x1000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut bytes);
            for (index, byte) in [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80]
                .into_iter()
                .enumerate()
            {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(0x100 + index as u32),
                    byte,
                );
            }
        }
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A404_0000, 0x1000);
            mem.store_w(0xFFFF_FFFF_A404_0004, 0x100);
            mem.store_w(0xFFFF_FFFF_A404_0008, 7);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & fn64_runtime::SP_STATUS_DMA_BUSY,
                0
            );
        }

        crate::advance_virtual_time(8);
        {
            let mem = Rdram::new(&mut bytes);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & fn64_runtime::SP_STATUS_DMA_BUSY,
                0
            );
        }
        crate::advance_virtual_time(9);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & 4, 0);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A400_1000) as u32, 0x1020_3040);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A400_1004) as u32, 0x5060_7080);
        }
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().sp_imem_generation),
            1
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_pi_registers_drive_the_live_timed_device_fabric() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        crate::load_rom_with_fixed_pi_latency(rom, 5);
        let mut bytes = vec![0u8; 0x1000];
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A460_0000, 0x400);
            mem.store_w(0xFFFF_FFFF_A460_0004, 0x20);
            mem.store_w(0xFFFF_FFFF_A460_0008, 3);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_A460_0010) as u32,
                fn64_runtime::PI_STATUS_DMA_BUSY
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400), 0);
        }

        crate::advance_virtual_time(4);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400), 0);
        }
        crate::advance_virtual_time(5);

        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400) as u32, 0x1234_5678);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A460_0010), 0);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Pi.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_rcp_acknowledgements_clear_the_shared_mi_sources() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let sources = [
            fn64_runtime::InterruptSource::Sp,
            fn64_runtime::InterruptSource::Si,
            fn64_runtime::InterruptSource::Ai,
            fn64_runtime::InterruptSource::Vi,
            fn64_runtime::InterruptSource::Dp,
        ];
        with_host(|host| {
            let fabric = &mut host.device_fabric;
            for source in sources {
                fabric.raise_interrupt(source);
            }
        });

        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A404_0010, 1 << 3);
        mem.store_w(0xFFFF_FFFF_A480_0018, 0);
        mem.store_w(0xFFFF_FFFF_A450_000C, 0);
        mem.store_w(0xFFFF_FFFF_A440_0010, 0);
        mem.store_w(0xFFFF_FFFF_A430_0000, 1 << 11);

        let pending = with_host(|host| host.device_fabric.snapshot().mi_pending);
        assert_eq!(pending & 0x3F, 0);
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_vi_registers_drive_half_line_timing_and_shared_mi() {
        crate::test_support::install_complete_render_backend(
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        );
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A440_0018, 525);
        mem.store_w(0xFFFF_FFFF_A440_000C, 100);
        crate::vi::arm_vi_retrace(1_000);

        crate::advance_virtual_time(190);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 98);
        crate::advance_virtual_time(191);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 100);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Vi.bit(),
            0
        );

        mem.store_w(0xFFFF_FFFF_A440_0010, 0xFFFF_FFFF);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 100);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Vi.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_ai_registers_schedule_the_live_guest_cycle_fifo() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A450_0008, 1);
        mem.store_w(0xFFFF_FFFF_A450_0010, 151);
        mem.store_w(0xFFFF_FFFF_A450_0000, 0x1000);
        mem.store_w(0xFFFF_FFFF_A450_0004, 0x80);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A450_000C) as u32 & fn64_runtime::AI_STATUS_BUSY,
            0
        );
        let deadline = with_host(|host| host.device_fabric.next_deadline().unwrap().get());
        crate::advance_virtual_time(deadline);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_A450_000C) as u32,
            fn64_runtime::AI_STATUS_ENABLED
        );
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().mi_pending)
                & fn64_runtime::InterruptSource::Ai.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_si_registers_run_separate_timed_pif_write_and_read_dmas() {
        let mut bytes = vec![0u8; 0x200];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut bytes);
            for (offset, byte) in [(0, 1), (1, 3), (2, 0xFF), (3, 0), (6, 0xFE)] {
                view.write_u8(fn64_runtime::RdramAddr::from_offset(offset), byte);
            }
        }
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A480_0000, 0);
            mem.store_w(0xFFFF_FFFF_A480_0010, 0);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A480_0018) & 1, 1);
        }
        crate::advance_virtual_time(1);
        {
            let mut mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A480_0018) as u32, 1 << 12);
            mem.store_w(0xFFFF_FFFF_A480_0018, 0);
            mem.store_w(0xFFFF_FFFF_A480_0000, 0);
            mem.store_w(0xFFFF_FFFF_A480_0004, 0);
        }
        crate::advance_virtual_time(2);
        let view = fn64_runtime::RdramView::from_storage(&bytes);
        assert_eq!(
            (3..6)
                .map(|offset| view.read_u8(fn64_runtime::RdramAddr::from_offset(offset)))
                .collect::<Vec<_>>(),
            vec![0x05, 0, 0]
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }
}
