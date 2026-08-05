//! Host-side execution surface: guest memory views, host function
//! catalogs, write-boundary notification, and the host-or-recompiled call
//! path. Split from the runtime module body purely by size.

use super::*;


/// Number of bytes of rdram the N64 exposes (8 MiB with the Expansion Pak,
/// which is what recompiled titles assume). The checked accessors bound every
/// access against this.
pub const RDRAM_LEN: usize = 8 * 1024 * 1024;

/// The base virtual address that maps to rdram offset 0 (KSEG0). Sign-extended
/// to 64 bits, this is the `0xFFFF_FFFF_8000_0000` the C macros subtract.
pub const RDRAM_VBASE: u64 = 0xFFFF_FFFF_8000_0000;

/// A checked view over rdram. All emitted memory accesses go through these
/// typed methods; the address translation and the big-endian sub-word swizzle
/// live here and nowhere else.
pub struct Rdram<'a> {
    mem: &'a mut [u8],
}

/// The common signature of every typed-Rust recompiled function.
///
/// This is the safe-Rust equivalent of N64Recomp's MIT-licensed
/// `recomp_func_t = void(uint8_t *rdram, recomp_context *ctx)`
/// (`refs/N64RecompSource/include/recomp.h:443-451`). The three explicit
/// higher-ranked lifetimes keep the context borrow, the `Rdram` view borrow,
/// and the underlying byte-slice borrow independent; no pointer conversion or
/// lifetime erasure is involved.
pub type RecompFunc =
    for<'ctx, 'view, 'rdram> fn(&'ctx mut RecompContext, &'view mut Rdram<'rdram>);

/// Invalid input to [`HostFunctionCatalogV1`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostFunctionCatalogErrorV1 {
    MisalignedTarget { target: u32 },
    DuplicateTarget { target: u32 },
}

/// Exact, enumerable host-function targets installed beside generated code.
///
/// This catalog deliberately carries no resolver-policy authority: it proves
/// only its own sorted target/function association. In particular, creating a
/// catalog neither installs nor supersedes the legacy [`HostLookup`] hook.
/// An empty catalog is valid and represents a program with no host targets.
#[derive(Clone, Debug)]
pub struct HostFunctionCatalogV1 {
    target_pcs: Vec<u32>,
    functions: Vec<RecompFunc>,
}

impl HostFunctionCatalogV1 {
    pub fn new(mut entries: Vec<(u32, RecompFunc)>) -> Result<Self, HostFunctionCatalogErrorV1> {
        if let Some(&(target, _)) = entries.iter().find(|(target, _)| !target.is_multiple_of(4)) {
            return Err(HostFunctionCatalogErrorV1::MisalignedTarget { target });
        }
        entries.sort_unstable_by_key(|(target, _)| *target);
        if let Some(pair) = entries.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(HostFunctionCatalogErrorV1::DuplicateTarget { target: pair[0].0 });
        }
        let (target_pcs, functions) = entries.into_iter().unzip();
        Ok(Self {
            target_pcs,
            functions,
        })
    }

    /// Canonical ascending target inventory, independent of input order.
    pub fn target_pcs(&self) -> &[u32] {
        &self.target_pcs
    }

    pub fn is_empty(&self) -> bool {
        self.target_pcs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.target_pcs.len()
    }

    pub fn resolve(&self, target: u32) -> Option<RecompFunc> {
        self.target_pcs
            .binary_search(&target)
            .ok()
            .map(|index| self.functions[index])
    }
}

/// Host lookup hook used for functions that must be supplied by the runtime
/// instead of executing a recompiled body (libultra shims, exception/TLB
/// handling, and other host-owned boundaries).
pub type HostLookup = fn(u32) -> Option<RecompFunc>;
/// Cooperative-yield hook for the N64Recomp `pause_self` self-loop rule.
pub type HostPause = fn();
/// Optional raw word-MMIO read. `None` means the address is ordinary memory.
pub type MmioRead = fn(u64) -> Option<u32>;
/// Optional raw word-MMIO write. `true` means the device consumed the write.
pub type MmioWrite = fn(u64, u32) -> bool;
/// The exact byte-producing mechanism responsible for one committed guest
/// RDRAM mutation.
///
/// This is a fixed architectural denominator, not an open-ended diagnostic
/// label. Every public external-write gateway below selects exactly one
/// variant; callers cannot submit an unattributed write event.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum WriterChannel {
    CpuInstructionStore,
    PiDma,
    SiDma,
    SpDma,
    RspExecutionOrHleWriteback,
    RdpRenderer,
    HostAbi,
    BootstrapOrImport,
}

/// One attributed post-commit guest write. Only aligned CPU halfword stores
/// carry a value because public RDRAM hidden-bit behavior assigns semantics
/// to that exact operation; other effects remain exact attributed ranges.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GuestWriteEvent {
    Range {
        channel: WriterChannel,
        physical_offset: u32,
        len: u32,
    },
    NonRdpWrite16 {
        channel: WriterChannel,
        logical_offset: u32,
        value: u16,
    },
}

impl GuestWriteEvent {
    pub const fn channel(self) -> WriterChannel {
        match self {
            Self::Range { channel, .. } | Self::NonRdpWrite16 { channel, .. } => channel,
        }
    }

    pub const fn range(self) -> (u32, u32) {
        match self {
            Self::Range {
                physical_offset,
                len,
                ..
            } => (physical_offset, len),
            Self::NonRdpWrite16 { logical_offset, .. } => (logical_offset, 2),
        }
    }
}

/// Post-commit physical RDRAM write observer. Executable invalidation and
/// renderer notification are multiplexed by the host callback.
pub type WriteObserver = fn(GuestWriteEvent);

/// One successful checked arbitrary-PC guest data load from physical RDRAM.
/// The length conservatively covers the bytes touched by the backing read.
/// Whole-function generated runners and host-side snapshots do not publish
/// these events.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GuestReadEvent {
    pub physical_offset: u32,
    pub len: u32,
}

pub type ReadObserver = fn(GuestReadEvent);

/// Whether one committed guest write changed bytes owned by the active
/// executable image.
///
/// The ordinary observation callback above remains notification-only. A live
/// block-program owner installs this second callback only when it can prove
/// that a write intersects one of its registered executable regions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestWriteBoundary {
    Continue,
    ExecutableChanged,
}

pub type GuestWriteBoundaryObserver = fn(GuestWriteEvent) -> GuestWriteBoundary;

/// Host callback reached immediately before translated code preserves a loud
/// panic for an instruction shape this runtime does not model.
pub type UnsupportedObserver = fn(&str);

/// Stable identity emitted at the first statement of every translated
/// whole-function body. The enclosing artifact identity remains host-owned;
/// `(vram, symbol)` distinguishes functions within that artifact without
/// depending on native addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TranslatedFunctionIdentity {
    pub vram: u32,
    pub symbol: &'static str,
}

impl TranslatedFunctionIdentity {
    pub const fn new(vram: u32, symbol: &'static str) -> Self {
        Self { vram, symbol }
    }
}

/// Opaque version marker exported by newly generated whole-function modules.
/// Passing a generated module's marker to the ABI is the explicit assertion
/// that every callable in that artifact contains the entry hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionEntryObservationSchema(u32);

/// Entry-observation schema implemented by this emitter/runtime pair.
pub const FUNCTION_ENTRY_OBSERVATION_SCHEMA: FunctionEntryObservationSchema =
    FunctionEntryObservationSchema(1);

/// Host callback reached by an emitted body before its first translated
/// instruction executes.
pub type FunctionEntryObserver = fn(TranslatedFunctionIdentity);

thread_local! {
    /// Recompiled execution is single-threaded by design (`docs/DESIGN.md` section
    /// 2), so the override belongs to the executing host thread. A
    /// thread-local `Cell` also lets tests install an isolated resolver
    /// without unsafe global mutation or cross-test serialization.
    static HOST_LOOKUP: std::cell::Cell<Option<HostLookup>> = const {
        std::cell::Cell::new(None)
    };
    static HOST_PAUSE: std::cell::Cell<Option<HostPause>> = const {
        std::cell::Cell::new(None)
    };
    static MMIO_READ: std::cell::Cell<Option<MmioRead>> = const {
        std::cell::Cell::new(None)
    };
    static MMIO_WRITE: std::cell::Cell<Option<MmioWrite>> = const {
        std::cell::Cell::new(None)
    };
    static WRITE_OBSERVER: std::cell::Cell<Option<WriteObserver>> = const {
        std::cell::Cell::new(None)
    };
    static READ_OBSERVER: std::cell::Cell<Option<ReadObserver>> = const {
        std::cell::Cell::new(None)
    };
    static GUEST_WRITE_BOUNDARY_OBSERVER:
        std::cell::Cell<Option<GuestWriteBoundaryObserver>> = const {
            std::cell::Cell::new(None)
        };
    static EXECUTABLE_WRITE_BOUNDARY: std::cell::Cell<bool> = const {
        std::cell::Cell::new(false)
    };
    static UNSUPPORTED_OBSERVER: std::cell::Cell<Option<UnsupportedObserver>> = const {
        std::cell::Cell::new(None)
    };
    static FUNCTION_ENTRY_OBSERVER: std::cell::Cell<Option<FunctionEntryObserver>> = const {
        std::cell::Cell::new(None)
    };
    static GUEST_WRITE_SESSION: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static GUEST_WRITE_EPOCH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static GUEST_WRITE_PAGE_EPOCHS: std::cell::RefCell<Vec<u32>> =
        std::cell::RefCell::new(vec![0; RDRAM_LEN / 4096]);
}

/// Install (or clear) the current thread's host-function resolver, returning
/// the previous resolver.
///
/// Generated dispatchers consult this hook before their sorted recompiled table.
/// A host can therefore bind a vram to a safe typed adapter over an fn64 shim;
/// vrams deliberately omitted from the recompiled table fail loudly if the host
/// has not installed their adapter. The function-pointer seam itself is
/// entirely safe Rust: no `transmute`, raw pointer, or ABI cast is involved.
pub fn set_host_lookup(resolver: Option<HostLookup>) -> Option<HostLookup> {
    HOST_LOOKUP.with(|slot| slot.replace(resolver))
}

/// Install the host's cooperative-yield adapter for translated self-loops.
pub fn set_host_pause(pause: Option<HostPause>) -> Option<HostPause> {
    HOST_PAUSE.with(|slot| slot.replace(pause))
}

/// Install the raw word-MMIO boundary used by emitted `lw`/`sw` operations.
/// The hooks are thread-local like host lookup because guest execution is
/// single-threaded; ordinary RDRAM accesses remain direct checked slice I/O.
pub fn set_mmio_hooks(
    read: Option<MmioRead>,
    write: Option<MmioWrite>,
) -> (Option<MmioRead>, Option<MmioWrite>) {
    let previous_read = MMIO_READ.with(|slot| slot.replace(read));
    let previous_write = MMIO_WRITE.with(|slot| slot.replace(write));
    (previous_read, previous_write)
}

pub fn set_write_observer(observer: Option<WriteObserver>) -> Option<WriteObserver> {
    WRITE_OBSERVER.with(|slot| slot.replace(observer))
}

/// Install (or clear) the current thread's checked arbitrary-PC data-load
/// observer, returning the previous observer. The copied callback may replace
/// or clear itself; recursive checked loads recursively invoke it. A callback
/// panic propagates after the backing read and leaves the callback installed.
pub fn set_read_observer(observer: Option<ReadObserver>) -> Option<ReadObserver> {
    READ_OBSERVER.with(|slot| slot.replace(observer))
}

/// Install the live executable-owner callback and clear any request belonging
/// to the previous owner.
pub fn set_guest_write_boundary_observer(
    observer: Option<GuestWriteBoundaryObserver>,
) -> Option<GuestWriteBoundaryObserver> {
    EXECUTABLE_WRITE_BOUNDARY.with(|pending| pending.set(false));
    GUEST_WRITE_SESSION.with(|session| {
        let next = session
            .get()
            .checked_add(1)
            .expect("guest-write session overflow");
        session.set(next);
    });
    GUEST_WRITE_EPOCH.with(|epoch| epoch.set(0));
    GUEST_WRITE_PAGE_EPOCHS.with(|epochs| epochs.borrow_mut().fill(0));
    GUEST_WRITE_BOUNDARY_OBSERVER.with(|slot| slot.replace(observer))
}

fn mark_guest_write_pages(offset: u32, len: u32) {
    if len == 0 {
        return;
    }
    let start = offset as usize / 4096;
    let end = (offset as usize)
        .saturating_add(len as usize)
        .saturating_sub(1)
        / 4096;
    let epoch = GUEST_WRITE_EPOCH.with(|epoch| {
        let next = epoch
            .get()
            .checked_add(1)
            .expect("guest-write epoch overflow");
        epoch.set(next);
        next
    });
    GUEST_WRITE_PAGE_EPOCHS.with(|epochs| {
        let mut epochs = epochs.borrow_mut();
        for page in start..=end.min(epochs.len().saturating_sub(1)) {
            epochs[page] = epoch;
        }
    });
}

/// Session-qualified last-write token for a physical RDRAM range. A caller
/// may cache successful byte-image verification until this token changes.
pub fn guest_write_token(offset: u32, len: u32) -> u64 {
    assert!(len > 0, "guest-write token range must be nonempty");
    let start = offset as usize / 4096;
    let end = (offset as usize)
        .checked_add(len as usize)
        .and_then(|end| end.checked_sub(1))
        .expect("guest-write token range overflow")
        / 4096;
    let page_epoch = GUEST_WRITE_PAGE_EPOCHS.with(|epochs| {
        let epochs = epochs.borrow();
        assert!(end < epochs.len(), "guest-write token range exceeds RDRAM");
        epochs[start..=end].iter().copied().max().unwrap_or(0)
    });
    let session = GUEST_WRITE_SESSION.with(std::cell::Cell::get);
    (u64::from(session) << 32) | u64::from(page_epoch)
}

/// Consume one post-store executable invalidation request.
///
/// Generated and interpreted runners call this only at architectural
/// instruction boundaries. A control-transfer owner deliberately waits until
/// its delay slot has completed, so invalidation cannot split the pair.
#[inline]
pub fn take_executable_write_boundary() -> bool {
    EXECUTABLE_WRITE_BOUNDARY.with(|pending| pending.replace(false))
}

/// Discard a boundary request after an external writer's owner has already
/// published the replacement executable generation.
///
/// Device DMA and RSP writeback use the same post-commit notification seam as
/// CPU stores, but execute while no translated runner is active. Their host
/// boundary processes the replacement directly; retaining the request would
/// make the next generation stop after its first instruction for a write that
/// has already been serviced.
pub fn discard_executable_write_boundary() {
    EXECUTABLE_WRITE_BOUNDARY.with(|pending| pending.set(false));
}

/// Install the host's unsupported-instruction evidence sink. The translated
/// lane remains independently usable: without a sink, the same named panic
/// still fires.
pub fn set_unsupported_observer(
    observer: Option<UnsupportedObserver>,
) -> Option<UnsupportedObserver> {
    UNSUPPORTED_OBSERVER.with(|slot| slot.replace(observer))
}

/// Install the current thread's translated-function entry observer.
pub fn set_function_entry_observer(
    observer: Option<FunctionEntryObserver>,
) -> Option<FunctionEntryObserver> {
    FUNCTION_ENTRY_OBSERVER.with(|slot| slot.replace(observer))
}

/// Record entry into one emitted whole-function body. Generated code places
/// this call before initializing its local dispatch PC, so direct calls,
/// lookup-resolved calls, tail calls, and root entry share one boundary.
#[inline]
pub fn notify_function_entry(identity: TranslatedFunctionIdentity) {
    FUNCTION_ENTRY_OBSERVER.with(|slot| {
        if let Some(observer) = slot.get() {
            observer(identity);
        }
    });
}

/// Record and preserve the loud endpoint for unsupported translated CPU
/// behavior. Generated bodies use this instead of open-coded panics so the
/// fixed-cycle journal cannot miss an early abort.
#[cold]
#[inline(never)]
pub fn trap_unsupported(context: impl Into<String>) -> ! {
    let context = context.into();
    UNSUPPORTED_OBSERVER.with(|slot| {
        if let Some(observer) = slot.get() {
            observer(&context);
        }
    });
    panic!("{context}")
}

/// Common post-commit implementation. It stays private so external producers
/// must choose one of the exact channel-specific gateways below.
fn notify_attributed_guest_write(channel: WriterChannel, offset: u32, len: u32) {
    if len != 0 {
        mark_guest_write_pages(offset, len);
        let event = GuestWriteEvent::Range {
            channel,
            physical_offset: offset,
            len,
        };
        WRITE_OBSERVER.with(|slot| {
            if let Some(observer) = slot.get() {
                observer(event);
            }
        });
        request_guest_write_boundary(event);
    }
}

/// Report a generated-C or other externally adapted CPU instruction store.
pub fn notify_cpu_instruction_store(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::CpuInstructionStore, offset, len);
}

pub fn notify_pi_dma_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::PiDma, offset, len);
}

pub fn notify_si_dma_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::SiDma, offset, len);
}

pub fn notify_sp_dma_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::SpDma, offset, len);
}

pub fn notify_rsp_execution_or_hle_writeback(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::RspExecutionOrHleWriteback, offset, len);
}

pub fn notify_rdp_renderer_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::RdpRenderer, offset, len);
}

pub fn notify_host_abi_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::HostAbi, offset, len);
}

pub fn notify_bootstrap_or_import_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::BootstrapOrImport, offset, len);
}

/// Notify one aligned CPU halfword store after the visible bytes commit.
fn notify_cpu_instruction_store16(logical_offset: u32, value: u16) {
    mark_guest_write_pages(logical_offset, 2);
    let event = GuestWriteEvent::NonRdpWrite16 {
        channel: WriterChannel::CpuInstructionStore,
        logical_offset,
        value,
    };
    WRITE_OBSERVER.with(|slot| {
        if let Some(observer) = slot.get() {
            observer(event);
        }
    });
    request_guest_write_boundary(event);
}

#[inline]
fn request_guest_write_boundary(event: GuestWriteEvent) {
    GUEST_WRITE_BOUNDARY_OBSERVER.with(|slot| {
        if slot
            .get()
            .is_some_and(|observer| observer(event) == GuestWriteBoundary::ExecutableChanged)
        {
            // Closes the exact interleaving `generation-A store commits -> A
            // executes a later translated instruction -> host retires A`.
            // The runner consumes this only after the current instruction, or
            // after the complete branch/delay pair when the store is its slot.
            EXECUTABLE_WRITE_BOUNDARY.with(|pending| pending.set(true));
        }
    });
}

/// Yield the active emulated thread at an unconditional branch-to-self.
pub fn pause_self() {
    HOST_PAUSE.with(|slot| {
        slot.get()
            .unwrap_or_else(|| panic!("pause_self: rs host installed no coroutine-yield adapter"))(
        )
    });
}

/// Resolve `vram` through the current thread's host-function resolver.
#[inline]
pub fn resolve_host_function(vram: u32) -> Option<RecompFunc> {
    HOST_LOOKUP.with(|slot| slot.get().and_then(|resolver| resolver(vram)))
}

/// Invoke a statically-known recompiled target unless the host resolver overrides
/// its vram. This is the direct-JAL counterpart of generated `lookup(vram)`:
/// libultra functions whose bodies contain no privileged instruction still
/// must enter the executor-backed host shim rather than bypassing it merely
/// because the Rust recompiler could translate their machine code.
#[inline]
pub fn call_host_or_recompiled(
    vram: u32,
    recompiled: RecompFunc,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) {
    resolve_host_function(vram).unwrap_or(recompiled)(ctx, mem);
}

impl<'a> Rdram<'a> {
    /// Wrap a byte buffer as rdram. The buffer should be [`RDRAM_LEN`] bytes;
    /// shorter buffers simply make more addresses fall out of bounds (a loud
    /// panic on access) rather than corrupting host memory.
    pub fn new(mem: &'a mut [u8]) -> Self {
        Rdram { mem }
    }

    /// Borrow the shared backing allocation at the runtime ABI seam. Normal
    /// emitted code has no reason to use this; fn64's rs-lane host adapters use
    /// it to call the existing, audited `*_recomp` marshalling layer without
    /// allocating or copying a second RDRAM image.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.mem
    }

    /// Borrow the backing allocation for reading.
    ///
    /// The shared counterpart to [`Self::as_mut_slice`], for callers that only
    /// need to read. The canonical mutation journal wants this: it snapshots
    /// the watched region at every dispatch boundary, and going through a
    /// per-byte closure costs a bounds check and a lane XOR per byte over a
    /// 1 MiB region.
    pub fn as_slice(&self) -> &[u8] {
        self.mem
    }

    /// Snapshot one physical RDRAM interval in guest byte order.
    ///
    /// Canonical executable-image reconciliation uses this read-only API so
    /// the ABI owner never needs another mutable slice or raw pointer merely
    /// to prove that no unjournaled writer changed a precompiled backing.
    pub fn copy_physical_bytes(&self, physical_start: u32, byte_len: u32) -> Vec<u8> {
        let physical_end = physical_start
            .checked_add(byte_len)
            .unwrap_or_else(|| panic!("physical RDRAM snapshot range overflow"));
        assert!(
            physical_end <= RDRAM_LEN as u32,
            "physical RDRAM snapshot [{physical_start:#010x}, {physical_end:#010x}) exceeds {RDRAM_LEN:#x} bytes"
        );
        (physical_start..physical_end)
            .map(|physical| self.load_physical_bu(physical))
            .collect()
    }

    /// Translate a canonical KSEG0/KSEG1 address to its generated-code backing
    /// offset. Physical RDRAM aliases share the low 29-bit device prefix;
    /// non-RDRAM direct windows retain N64Recomp's sparse `address - KSEG0`
    /// layout. Modeled RCP/PIF words are excluded because their installed hook
    /// is the sole device authority.
    #[inline]
    pub(super) fn direct_storage_offset(vaddr: u64) -> Option<usize> {
        if Self::is_rcp_mmio(vaddr) {
            return None;
        }

        let upper = vaddr >> 32;
        let low = vaddr as u32;
        let canonical_32 = upper == 0 || upper == u32::MAX as u64;
        let direct_segment = (0x8000_0000..0xc000_0000).contains(&low);
        if !canonical_32 || !direct_segment {
            return None;
        }

        let physical = low & 0x1fff_ffff;
        Some(if physical < RDRAM_LEN as u32 {
            physical as usize
        } else {
            low.wrapping_sub(0x8000_0000) as usize
        })
    }

    #[inline]
    fn backing_offset(vaddr: u64) -> usize {
        if let Some(offset) = Self::direct_storage_offset(vaddr) {
            return offset;
        }
        let physical = (vaddr as u32) & 0x1fff_ffff;
        let reason = if Self::is_rcp_mmio(vaddr) {
            "modeled word-only device access was not consumed by the installed hook"
        } else {
            "only zero- or sign-extended KSEG0/KSEG1 are modeled"
        };
        trap_unsupported(format!(
            "Rdram: unsupported mapped address {vaddr:#018x} resolves to physical {physical:#x}; {reason}"
        ))
    }

    #[inline]
    fn read_mmio_word(vaddr: u64) -> Option<u32> {
        if !Self::may_be_mmio(vaddr) {
            return None;
        }
        MMIO_READ.with(|slot| slot.get().and_then(|read| read(vaddr)))
    }

    /// Cheap rejection for addresses no MMIO window can claim.
    ///
    /// `try_load_w` and friends consult MMIO BEFORE the backed-memory fast
    /// path, because a device register must win over stale RDRAM. That
    /// ordering is correct, but it meant every ordinary guest word load walked
    /// the whole window chain -- PIF, cartridge, device, RCP interrupt -- and
    /// took the host lock, only to be rejected.
    ///
    /// Sampling the WM2000 block runner put **98.5% of total runtime** in
    /// `read_raw_mmio_word` (5877 of 5965 samples) for exactly this reason. An
    /// address histogram then showed the callers were KSEG0 RDRAM addresses
    /// like `0x800771fc`, each seen a handful of times -- not a hot register,
    /// just ordinary memory paying MMIO dispatch on every access.
    ///
    /// Every real N64 MMIO window lives in KSEG1 (`0xA0000000..0xC0000000`),
    /// which is uncached precisely because it is device space. Testing the
    /// segment first keeps the MMIO-before-memory ordering intact while making
    /// the common case two compares.
    #[inline(always)]
    fn may_be_mmio(vaddr: u64) -> bool {
        let segment = vaddr as u32;
        (0xA000_0000..0xC000_0000).contains(&segment)
    }

    #[inline]
    fn write_mmio_word(vaddr: u64, value: u32) -> bool {
        MMIO_WRITE.with(|slot| slot.get().is_some_and(|write| write(vaddr, value)))
    }

    #[inline]
    fn load_backed_word(&self, vaddr: u64) -> i32 {
        let p = Self::backing_offset(vaddr);
        i32::from_ne_bytes(self.mem[p..p + 4].try_into().unwrap())
    }

    #[inline]
    fn store_backed_word(&mut self, vaddr: u64, value: u32) {
        let p = Self::backing_offset(vaddr);
        self.mem[p..p + 4].copy_from_slice(&value.to_ne_bytes());
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            notify_cpu_instruction_store(offset, 4);
        }
    }

    /// Canonical physical RDRAM offset for cached or uncached CPU aliases.
    /// Only the direct segments are accepted: masking KUSEG would silently
    /// add unsupported TLB behavior. Device/renderer observations are named
    /// in the same physical RDRAM space as the visible backing bytes.
    #[inline]
    pub(super) fn physical_rdram_offset(vaddr: u64) -> Option<u32> {
        let upper = vaddr >> 32;
        let low = vaddr as u32;
        let canonical_32 = upper == 0 || upper == u32::MAX as u64;
        let direct_segment = (0x8000_0000..0xc000_0000).contains(&low);
        let physical = low & 0x1fff_ffff;
        (canonical_32 && direct_segment && physical < RDRAM_LEN as u32).then_some(physical)
    }

    #[inline]
    fn notify_translated_rdram_read(vaddr: u64, len: u32) {
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            READ_OBSERVER.with(|slot| {
                if let Some(observer) = slot.get() {
                    observer(GuestReadEvent {
                        physical_offset: offset,
                        len,
                    });
                }
            });
        }
    }

    /// Generated-C's proxy exposes RCP registers and PIF RAM through the
    /// RCP's 32-bit SysAD word transaction. Subword CPU reads select the
    /// addressed big-endian lane from that word; subword writes place their
    /// value in that lane and drive zero on the other lanes. Keep the typed
    /// lane on that identical boundary instead of falling through to sparse
    /// host storage.
    #[inline]
    fn is_rcp_mmio(vaddr: u64) -> bool {
        let upper = vaddr >> 32;
        let low = vaddr as u32;
        let canonical_32 = upper == 0 || upper == u32::MAX as u64;
        if canonical_32 && (0xa400_0000..0xa490_0000).contains(&low) {
            return true;
        }
        let physical = low & 0x1fff_ffff;
        canonical_32
            && (0x8000_0000..0xc000_0000).contains(&low)
            && (0x1fc0_07c0..0x1fc0_0800).contains(&physical)
    }

    #[inline]
    fn reject_unsupported_mmio_width(vaddr: u64, width: u32, is_write: bool) {
        if Self::is_rcp_mmio(vaddr) {
            let operation = if is_write { "write" } else { "read" };
            trap_unsupported(format!(
                "Rdram: raw MMIO {operation} at {vaddr:#018x} used unsupported {width}-byte access; the RCP SysAD boundary models byte, halfword, and word transactions"
            ));
        }
    }

    #[inline]
    fn read_mmio_lane(vaddr: u64, width: u32) -> Option<u32> {
        let word = Self::read_mmio_word(vaddr & !3)?;
        let shift = match width {
            1 => 24 - ((vaddr as u32 & 3) * 8),
            2 => 16 - ((vaddr as u32 & 2) * 8),
            _ => return None,
        };
        let mask = if width == 1 { 0xff } else { 0xffff };
        Some((word >> shift) & mask)
    }

    #[inline]
    fn write_mmio_lane(vaddr: u64, width: u32, value: u32) -> bool {
        let shift = match width {
            1 => 24 - ((vaddr as u32 & 3) * 8),
            2 => 16 - ((vaddr as u32 & 2) * 8),
            _ => return false,
        };
        Self::write_mmio_word(vaddr & !3, value << shift)
    }

    /// Effective virtual address of a `off(base)` operand: full-width MIPS III
    /// addition of the 64-bit base and sign-extended 16-bit offset.
    #[inline]
    pub fn eff_addr(base_val: u64, off: i16) -> u64 {
        base_val.wrapping_add(off as i64 as u64)
    }

    // --- Aligned loads ---

    /// Load a sign-extended word. Returns the `i32` the caller sign-extends
    /// into a GPR.
    ///
    /// Perf: read the 4 bytes as ONE slice range (`self.mem[p..p+4]`) rather
    /// than four `self.mem[p+i]` indexes. The range form does a SINGLE bounds
    /// check and lets the compiler emit one aligned 32-bit load; the byte-at-
    /// a-time form did 4 bounds checks + a byte-assemble in the hot loop
    /// (millions of accesses in collision init). Same value, safe indexing,
    /// still `#![forbid(unsafe_code)]`.
    #[inline]
    pub fn load_w(&self, vaddr: u64) -> i32 {
        assert_eq!(vaddr & 3, 0, "unaligned LW at {vaddr:#018x}");
        if let Some(value) = Self::read_mmio_word(vaddr) {
            return value as i32;
        }
        self.load_backed_word(vaddr)
    }

    /// Load a sign-extended halfword (byte offset XOR 2).
    #[inline]
    pub fn load_h(&self, vaddr: u64) -> i16 {
        assert_eq!(vaddr & 1, 0, "unaligned LH at {vaddr:#018x}");
        if let Some(value) = Self::read_mmio_lane(vaddr, 2) {
            return value as i16;
        }
        let p = Self::backing_offset(vaddr) ^ 2;
        i16::from_ne_bytes(self.mem[p..p + 2].try_into().unwrap())
    }

    /// Load a zero-extended halfword (byte offset XOR 2).
    #[inline]
    pub fn load_hu(&self, vaddr: u64) -> u16 {
        self.load_h(vaddr) as u16
    }

    /// Load a sign-extended byte (byte offset XOR 3).
    #[inline]
    pub fn load_b(&self, vaddr: u64) -> i8 {
        if let Some(value) = Self::read_mmio_lane(vaddr, 1) {
            return value as i8;
        }
        let p = Self::backing_offset(vaddr) ^ 3;
        self.mem[p] as i8
    }

    /// Load a zero-extended byte (byte offset XOR 3).
    #[inline]
    pub fn load_bu(&self, vaddr: u64) -> u8 {
        if let Some(value) = Self::read_mmio_lane(vaddr, 1) {
            return value as u8;
        }
        let p = Self::backing_offset(vaddr) ^ 3;
        self.mem[p]
    }

    /// Read one explicitly admitted physical RDRAM byte without reconstructing
    /// a virtual alias. Generation-digest selection uses this after validating
    /// its complete VA-to-physical backing map.
    #[inline]
    pub(crate) fn load_physical_bu(&self, physical: u32) -> u8 {
        assert!(
            physical < RDRAM_LEN as u32,
            "physical RDRAM byte {physical:#010x} exceeds the 8 MiB device"
        );
        let p = usize::try_from(physical).expect("physical RDRAM offset exceeds usize") ^ 3;
        *self.mem.get(p).unwrap_or_else(|| {
            panic!(
                "physical RDRAM byte {physical:#010x} exceeds the installed {}-byte backing",
                self.mem.len()
            )
        })
    }

    /// Read one physical byte only when it is present in the installed RDRAM
    /// allocation. Live mapped instruction admission uses this checked form so
    /// a translated fetch beyond backing becomes a typed CPU fault rather than
    /// an indexing panic.
    #[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
    #[inline]
    pub(crate) fn try_load_physical_bu(&self, physical: u32) -> Option<u8> {
        if physical >= RDRAM_LEN as u32 {
            return None;
        }
        let p = usize::try_from(physical).ok()? ^ 3;
        self.mem.get(p).copied()
    }

    // --- Aligned stores ---

    /// Store the low word of `val`.
    #[inline]
    pub fn store_w(&mut self, vaddr: u64, val: u32) {
        assert_eq!(vaddr & 3, 0, "unaligned SW at {vaddr:#018x}");
        if Self::write_mmio_word(vaddr, val) {
            return;
        }
        self.store_backed_word(vaddr, val);
    }

    /// Store the low halfword of `val` (byte offset XOR 2).
    #[inline]
    pub fn store_h(&mut self, vaddr: u64, val: u16) {
        assert_eq!(vaddr & 1, 0, "unaligned SH at {vaddr:#018x}");
        if Self::write_mmio_lane(vaddr, 2, u32::from(val)) {
            return;
        }
        let p = Self::backing_offset(vaddr) ^ 2;
        self.mem[p..p + 2].copy_from_slice(&val.to_ne_bytes());
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            notify_cpu_instruction_store16(offset, val);
        }
    }

    /// Store the low byte of `val` (byte offset XOR 3).
    #[inline]
    pub fn store_b(&mut self, vaddr: u64, val: u8) {
        if Self::write_mmio_lane(vaddr, 1, u32::from(val)) {
            return;
        }
        let p = Self::backing_offset(vaddr) ^ 3;
        self.mem[p] = val;
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            notify_cpu_instruction_store(offset, 1);
        }
    }

    // --- Unaligned word loads/stores (LWL/LWR/SWL/SWR) ---
    //
    // Semantics clean-roomed from the MIPS III ISA: the pair of instructions
    // together load/store a full word straddling an alignment boundary. We
    // mirror N64Recomp's `do_lwl`/`do_lwr`/`do_swl`/`do_swr` helper math,
    // which is itself the ISA definition.

    /// Load-word-left: merge the high bytes of the addressed word into the
    /// high end of `initial` (the current register value).
    #[inline]
    pub fn load_wl(&self, initial: u64, vaddr: u64) -> i32 {
        let word_addr = vaddr & !0x3;
        let loaded = self.load_w(word_addr) as u32;
        let misalign = (vaddr & 0x3) as u32;
        let mask = !(0xFFFF_FFFFu32 << (misalign * 8));
        let masked = (initial as u32) & mask;
        (masked | (loaded << (misalign * 8))) as i32
    }

    /// Load-word-right: merge the low bytes into the low end of `initial`.
    #[inline]
    pub fn load_wr(&self, initial: u64, vaddr: u64) -> i32 {
        let word_addr = vaddr & !0x3;
        let loaded = self.load_w(word_addr) as u32;
        let misalign = (vaddr & 0x3) as u32;
        let mask = !(0xFFFF_FFFFu32 >> (24 - misalign * 8));
        let masked = (initial as u32) & mask;
        (masked | (loaded >> (24 - misalign * 8))) as i32
    }

    /// Store-word-left.
    #[inline]
    pub fn store_wl(&mut self, vaddr: u64, val: u32) {
        let word_addr = vaddr & !0x3;
        let misalign = (vaddr & 0x3) as u32;
        if Self::is_rcp_mmio(word_addr) {
            if misalign != 0 {
                Self::reject_unsupported_mmio_width(vaddr, 4 - misalign, true);
            }
            self.store_w(word_addr, val);
            return;
        }
        let initial = self.load_w(word_addr) as u32;
        let masked = initial & !(0xFFFF_FFFFu32 >> (misalign * 8));
        let shifted = val >> (misalign * 8);
        self.store_w(word_addr, masked | shifted);
    }

    /// Store-word-right.
    #[inline]
    pub fn store_wr(&mut self, vaddr: u64, val: u32) {
        let word_addr = vaddr & !0x3;
        let misalign = (vaddr & 0x3) as u32;
        if Self::is_rcp_mmio(word_addr) {
            if misalign != 3 {
                Self::reject_unsupported_mmio_width(vaddr, misalign + 1, true);
            }
            self.store_w(word_addr, val);
            return;
        }
        let initial = self.load_w(word_addr) as u32;
        let masked = initial & !(0xFFFF_FFFFu32 << (24 - misalign * 8));
        let shifted = val << (24 - misalign * 8);
        self.store_w(word_addr, masked | shifted);
    }

    // --- 64-bit doubleword loads/stores (LD/SD/LLD/SCD) ---
    //
    // Clean-roomed from the MIPS III ISA and matching N64Recomp's
    // `load_doubleword`/`SD` macros exactly: a doubleword is the two 32-bit
    // words at `vaddr+0` (the high half) and `vaddr+4` (the low half). Each
    // half goes through the ordinary native-endian word path
    // (`load_w`/`store_w`) with no sub-word swizzle. Logically, the high guest
    // word remains at `vaddr+0` and the low guest word at `vaddr+4`.

    /// Load a 64-bit doubleword: `(hi_word << 32) | lo_word` where `hi_word` is
    /// at `vaddr+0` and `lo_word` at `vaddr+4`.
    #[inline]
    pub fn load_d(&self, vaddr: u64) -> u64 {
        Self::reject_unsupported_mmio_width(vaddr, 8, false);
        assert_eq!(vaddr & 7, 0, "unaligned LD at {vaddr:#018x}");
        let hi = self.load_w(vaddr) as u32 as u64;
        let lo = self.load_w(vaddr.wrapping_add(4)) as u32 as u64;
        (hi << 32) | lo
    }

    /// Store a 64-bit doubleword: the high word to `vaddr+0`, the low word to
    /// `vaddr+4`, followed by one post-commit eight-byte write range.
    #[inline]
    pub fn store_d(&mut self, vaddr: u64, val: u64) {
        Self::reject_unsupported_mmio_width(vaddr, 8, true);
        assert_eq!(vaddr & 7, 0, "unaligned SD at {vaddr:#018x}");
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            let high = Self::backing_offset(vaddr);
            let low = Self::backing_offset(vaddr.wrapping_add(4));
            // Match N64Recomp's low-word then high-word commit order. The
            // observer runs only after both halves are coherent.
            self.mem[low..low + 4].copy_from_slice(&(val as u32).to_ne_bytes());
            self.mem[high..high + 4].copy_from_slice(&((val >> 32) as u32).to_ne_bytes());
            notify_cpu_instruction_store(offset, 8);
        } else {
            self.store_w(vaddr.wrapping_add(4), val as u32);
            self.store_w(vaddr, (val >> 32) as u32);
        }
    }

    // --- Unaligned doubleword loads/stores (LDL/LDR/SDL/SDR) ---
    //
    // The 64-bit analogue of LWL/LWR/SWL/SWR: the pair together moves a full
    // doubleword straddling an 8-byte boundary. Math mirrors N64Recomp's
    // `do_ldl`/`do_ldr`/`do_sdl`/`do_sdr`, which is the ISA definition. The
    // aligned dword the shift operates on is at `vaddr & !7`, and the shift
    // distances use the 3-bit misalignment (0..7).

    /// Load-doubleword-left: merge the high bytes of the addressed doubleword
    /// into the high end of `initial` (the current register value).
    #[inline]
    pub fn load_dl(&self, initial: u64, vaddr: u64) -> u64 {
        let dword_addr = vaddr & !0x7;
        let loaded = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 << (misalign * 8));
        masked | (loaded << (misalign * 8))
    }

    /// Load-doubleword-right: merge the low bytes into the low end of `initial`.
    #[inline]
    pub fn load_dr(&self, initial: u64, vaddr: u64) -> u64 {
        let dword_addr = vaddr & !0x7;
        let loaded = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 >> (56 - misalign * 8));
        masked | (loaded >> (56 - misalign * 8))
    }

    /// Store-doubleword-left.
    #[inline]
    pub fn store_dl(&mut self, vaddr: u64, val: u64) {
        let dword_addr = vaddr & !0x7;
        let initial = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 >> (misalign * 8));
        let shifted = val >> (misalign * 8);
        self.store_d(dword_addr, masked | shifted);
    }

    /// Store-doubleword-right.
    #[inline]
    pub fn store_dr(&mut self, vaddr: u64, val: u64) {
        let dword_addr = vaddr & !0x7;
        let initial = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 << (56 - misalign * 8));
        let shifted = val << (56 - misalign * 8);
        self.store_d(dword_addr, masked | shifted);
    }

    // --- Checked accessors for the bank/sparse block-runner lane (U4) ---
    //
    // The historical whole-function lane calls the unchecked accessors above:
    // an access outside backed generated-code storage is a host panic there,
    // and that panicking semantics is deliberately preserved. The block-runner
    // lane instead needs
    // a typed VR4300 memory fault it can turn into `BlockExit::Fault`, so it
    // calls these `try_` variants. On success they perform the identical access
    // as their unchecked twin; on an out-of-bounds effective address they
    // return `Err(vaddr)` carrying the faulting guest virtual address, and
    // touch no memory. This models "access outside supplied backing storage";
    // it is not full VR4300 address-error/TLB semantics (see U4 in
    // `docs/UNIVERSAL-RUNTIME-PLAN.md`).

    /// True iff the `width`-byte range beginning at storage offset `p` lies
    /// wholly inside backed storage. Every checked accessor reduces to
    /// this after applying its own swizzle so the admitted set matches exactly
    /// which unchecked accesses would not panic.
    #[inline]
    fn storage_range_backed(&self, p: usize, width: usize) -> bool {
        p.checked_add(width)
            .is_some_and(|end| end <= self.mem.len())
    }

    /// Translate and bound a checked-lane access without entering the
    /// unchecked lane's loud unsupported-address trap. `try_*` callers must
    /// return the original virtual address as a typed fault for every
    /// non-direct segment, including opt-in MMIO windows with no installed port.
    #[inline]
    fn virtual_range_backed(&self, vaddr: u64, lane_xor: usize, width: usize) -> bool {
        Self::direct_storage_offset(vaddr)
            .is_some_and(|p| self.storage_range_backed(p ^ lane_xor, width))
    }

    /// Resolve the typed MMU result into the direct alias understood by
    /// the shared RDRAM/device backing layout. Physical addresses beyond the
    /// N64's 29-bit direct window remain a loud unbacked boundary after a
    /// successful TLB lookup; they are never truncated into a different page.
    fn translated_backing_address(
        ctx: &RecompContext,
        vaddr: u64,
        access: DataAccessKind,
    ) -> Result<u64, DataAccessError> {
        match ctx.translate_data_address(vaddr, access)? {
            TranslatedDataAddress::Direct(address) => Ok(address),
            TranslatedDataAddress::DirectPhysical(physical)
            | TranslatedDataAddress::Mapped(physical)
                if physical < 0x2000_0000 =>
            {
                Ok(0xffff_ffff_a000_0000 | u64::from(physical))
            }
            TranslatedDataAddress::DirectPhysical(_) | TranslatedDataAddress::Mapped(_) => {
                Err(DataAccessError::Unbacked { vaddr })
            }
        }
    }

    fn translated_load_address(ctx: &RecompContext, vaddr: u64) -> Result<u64, DataAccessError> {
        Self::translated_backing_address(ctx, vaddr, DataAccessKind::Load)
    }

    fn translated_store_address(ctx: &RecompContext, vaddr: u64) -> Result<u64, DataAccessError> {
        Self::translated_backing_address(ctx, vaddr, DataAccessKind::Store)
    }

    /// Perform the architectural translation/access-bit checks for a store
    /// without touching backing memory. SC/SCD use this before consulting the
    /// LLbit: a failed conditional store still addresses memory and can raise
    /// a TLB refill, invalid, or modified exception.
    pub fn check_store_translation(ctx: &RecompContext, vaddr: u64) -> Result<(), DataAccessError> {
        ctx.translate_data_address(vaddr, DataAccessKind::Store)
            .map(|_| ())
    }

    /// Whether the aligned-word effective address is backed (LW/LWU/LL/…).
    #[inline]
    fn word_backed(&self, vaddr: u64) -> bool {
        self.virtual_range_backed(vaddr, 0, 4)
    }

    /// Whether the aligned-doubleword effective address is backed (LD/SD/…).
    #[inline]
    fn dword_backed(&self, vaddr: u64) -> bool {
        self.virtual_range_backed(vaddr, 0, 8)
    }

    /// Checked LW/LWU (aligned word). See the module note on the block lane.
    #[inline]
    pub fn try_load_w(&self, vaddr: u64) -> Result<i32, u64> {
        assert_eq!(vaddr & 3, 0, "unaligned LW at {vaddr:#018x}");
        if let Some(value) = Self::read_mmio_word(vaddr) {
            return Ok(value as i32);
        }
        if self.word_backed(vaddr) {
            Ok(self.load_backed_word(vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_w_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<i32, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_w(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 4);
        Ok(value)
    }

    /// Checked LH (aligned, sign-extended halfword).
    #[inline]
    pub fn try_load_h(&self, vaddr: u64) -> Result<i16, u64> {
        assert_eq!(vaddr & 1, 0, "unaligned LH at {vaddr:#018x}");
        if let Some(value) = Self::read_mmio_lane(vaddr, 2) {
            return Ok(value as i16);
        }
        if self.virtual_range_backed(vaddr, 2, 2) {
            Ok(self.load_h(vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_h_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<i16, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_h(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 2);
        Ok(value)
    }

    /// Checked LHU (aligned, zero-extended halfword).
    #[inline]
    pub fn try_load_hu(&self, vaddr: u64) -> Result<u16, u64> {
        self.try_load_h(vaddr).map(|v| v as u16)
    }

    pub fn try_load_hu_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<u16, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_hu(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 2);
        Ok(value)
    }

    /// Checked LB (sign-extended byte).
    #[inline]
    pub fn try_load_b(&self, vaddr: u64) -> Result<i8, u64> {
        if let Some(value) = Self::read_mmio_lane(vaddr, 1) {
            return Ok(value as i8);
        }
        if self.virtual_range_backed(vaddr, 3, 1) {
            Ok(self.load_b(vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_b_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<i8, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_b(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 1);
        Ok(value)
    }

    /// Checked LBU (zero-extended byte).
    #[inline]
    pub fn try_load_bu(&self, vaddr: u64) -> Result<u8, u64> {
        if let Some(value) = Self::read_mmio_lane(vaddr, 1) {
            return Ok(value as u8);
        }
        if self.virtual_range_backed(vaddr, 3, 1) {
            Ok(self.load_bu(vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_bu_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<u8, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_bu(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 1);
        Ok(value)
    }

    /// Checked LWL (the aligned word it merges from must be backed).
    #[inline]
    pub fn try_load_wl(&self, initial: u64, vaddr: u64) -> Result<i32, u64> {
        if self.word_backed(vaddr & !0x3) {
            Ok(self.load_wl(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_wl_translated(
        &self,
        ctx: &RecompContext,
        initial: u64,
        vaddr: u64,
    ) -> Result<i32, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_wl(initial, translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated & !0x3, 4);
        Ok(value)
    }

    /// Checked LWR.
    #[inline]
    pub fn try_load_wr(&self, initial: u64, vaddr: u64) -> Result<i32, u64> {
        if self.word_backed(vaddr & !0x3) {
            Ok(self.load_wr(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_wr_translated(
        &self,
        ctx: &RecompContext,
        initial: u64,
        vaddr: u64,
    ) -> Result<i32, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_wr(initial, translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated & !0x3, 4);
        Ok(value)
    }

    /// Checked LD/LLD (aligned doubleword).
    #[inline]
    pub fn try_load_d(&self, vaddr: u64) -> Result<u64, u64> {
        if self.dword_backed(vaddr) {
            Ok(self.load_d(vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_d_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<u64, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_d(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 8);
        Ok(value)
    }

    /// Checked LDL.
    #[inline]
    pub fn try_load_dl(&self, initial: u64, vaddr: u64) -> Result<u64, u64> {
        if self.dword_backed(vaddr & !0x7) {
            Ok(self.load_dl(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_dl_translated(
        &self,
        ctx: &RecompContext,
        initial: u64,
        vaddr: u64,
    ) -> Result<u64, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_dl(initial, translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated & !0x7, 8);
        Ok(value)
    }

    /// Checked LDR.
    #[inline]
    pub fn try_load_dr(&self, initial: u64, vaddr: u64) -> Result<u64, u64> {
        if self.dword_backed(vaddr & !0x7) {
            Ok(self.load_dr(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_dr_translated(
        &self,
        ctx: &RecompContext,
        initial: u64,
        vaddr: u64,
    ) -> Result<u64, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_dr(initial, translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated & !0x7, 8);
        Ok(value)
    }

    /// Checked SW.
    #[inline]
    pub fn try_store_w(&mut self, vaddr: u64, val: u32) -> Result<(), u64> {
        assert_eq!(vaddr & 3, 0, "unaligned SW at {vaddr:#018x}");
        if Self::write_mmio_word(vaddr, val) {
            return Ok(());
        }
        if self.word_backed(vaddr) {
            self.store_backed_word(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_w_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u32,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_w(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SH.
    #[inline]
    pub fn try_store_h(&mut self, vaddr: u64, val: u16) -> Result<(), u64> {
        assert_eq!(vaddr & 1, 0, "unaligned SH at {vaddr:#018x}");
        if Self::write_mmio_lane(vaddr, 2, u32::from(val)) {
            return Ok(());
        }
        if self.virtual_range_backed(vaddr, 2, 2) {
            self.store_h(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_h_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u16,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_h(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SB.
    #[inline]
    pub fn try_store_b(&mut self, vaddr: u64, val: u8) -> Result<(), u64> {
        if Self::write_mmio_lane(vaddr, 1, u32::from(val)) {
            return Ok(());
        }
        if self.virtual_range_backed(vaddr, 3, 1) {
            self.store_b(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_b_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u8,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_b(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SWL (reads and writes the aligned word it straddles).
    #[inline]
    pub fn try_store_wl(&mut self, vaddr: u64, val: u32) -> Result<(), u64> {
        if self.word_backed(vaddr & !0x3) {
            self.store_wl(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_wl_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u32,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_wl(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SWR.
    #[inline]
    pub fn try_store_wr(&mut self, vaddr: u64, val: u32) -> Result<(), u64> {
        if self.word_backed(vaddr & !0x3) {
            self.store_wr(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_wr_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u32,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_wr(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SD/SCD (aligned doubleword).
    #[inline]
    pub fn try_store_d(&mut self, vaddr: u64, val: u64) -> Result<(), u64> {
        if self.dword_backed(vaddr) {
            self.store_d(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_d_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u64,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_d(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SDL.
    #[inline]
    pub fn try_store_dl(&mut self, vaddr: u64, val: u64) -> Result<(), u64> {
        if self.dword_backed(vaddr & !0x7) {
            self.store_dl(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_dl_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u64,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_dl(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SDR.
    #[inline]
    pub fn try_store_dr(&mut self, vaddr: u64, val: u64) -> Result<(), u64> {
        if self.dword_backed(vaddr & !0x7) {
            self.store_dr(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_dr_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u64,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_dr(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }
}
