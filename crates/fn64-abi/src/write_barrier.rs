//! Page-aligned process RDRAM, and the MMU write barrier built on it.
//!
//! # Why
//!
//! The canonical executable-mutation guard proves, at every dispatch boundary,
//! that no byte of the watched executable region changed without a declaration.
//! It proves it the only way a software guard can: by READING the region and
//! comparing it against a sealed baseline. On WM2000 that is a 1,513,056-byte
//! `memcmp` per boundary, measured at 26,525 ns, and it is 96% of leaf samples
//! in a live profile of the pinned route.
//!
//! `docs/plans/wm2000-playable-blocker-ledger.md` proves that no *software*
//! substitute can beat it: a digest, checksum or Merkle path must read the same
//! bytes to be trustworthy, so it is strictly worse than the `memcmp` it would
//! replace. The proof names exactly two escapes, both hardware: dirty bits, and
//! `mprotect` write-protection with a fault handler. The MMU learns of a write
//! *without reading anything*, which is why it is not subject to that floor.
//!
//! # What this is
//!
//! Two pieces, in dependency order.
//!
//! [`PageAlignedRdram`] replaces the malloc'd `Box<[u8]>` the process
//! allocation used to be. `mprotect` operates on whole pages, so protecting a
//! malloc'd buffer would protect unrelated heap objects sharing its first and
//! last pages -- their next write would fault somewhere with no handler and no
//! diagnosis. A dedicated `mmap` owns whole pages and nothing else does.
//!
//! [`Barrier`] write-protects the watched region, records the pages the guest
//! faults on, and re-arms. At a boundary the guard then compares only the
//! faulted pages against the baseline instead of the whole region.
//!
//! # The correctness argument, stated once
//!
//! The barrier must never let a mutation through that the scan would catch.
//! It does not, and the reason is that it observes at a strictly lower level
//! than the scan's own source of truth:
//!
//! - The scan's premise is "compare live RDRAM bytes against the baseline".
//!   Every such byte lives in this `mmap`.
//! - A byte of an `mprotect(PROT_READ)` page cannot change without a write
//!   fault. That is the MMU's guarantee, not this code's.
//! - So every byte the scan could find changed is in some page this barrier
//!   recorded. The dirty page set is a SUPERSET of the changed byte set.
//! - It is a strict superset in general: a store that rewrites a byte with the
//!   value it already held faults but changes nothing, and a page is 16 KiB
//!   while a store is 1-8 bytes. That is why the byte-level comparison still
//!   runs -- over the dirty pages instead of the whole region. The guard's
//!   ANSWER is unchanged; only the number of bytes read to reach it falls.
//!
//! This is the property that makes the barrier a substitute rather than a
//! weakening, and it is why it also catches the writers no declaration path
//! sees. `Rdram::as_mut_slice`, the DMA paths, the RSP and renderer slices and
//! the raw `RdramPtr` stores all bypass `set_write_observer` -- they are
//! exactly what the guard exists to catch -- and every one of them faults,
//! because the MMU does not care which Rust function issued the store. The
//! declaration path is not consulted here at all.
//!
//! # Failing to the scan
//!
//! Every case this cannot cover falls back to the full scan, which is the
//! behaviour that exists today: allocation failure, an `mprotect` refusal, a
//! fault outside the region, an overflow of the fixed-capacity dirty set, or
//! the barrier simply being off. There is no configuration in which a boundary
//! is decided by neither the barrier nor the scan.
//!
//! # Signal-handler discipline
//!
//! The handler runs on the guest store path, on a `corosensei` coroutine stack,
//! with whatever locks and `RefCell` borrows the runtime happened to be holding
//! when the store issued. It therefore does exactly three things -- read two
//! atomics, set a bit in a preallocated atomic bitmap, and call `mprotect` --
//! and touches no allocator, no `RefCell`, and no lock. `SA_ONSTACK` is set.
//!
//! Delivery onto a coroutine stack is the obstacle that could have invalidated
//! the design; `reference/mprotect-bench/coroutine-fault.rs` settles it with
//! 2,000 faults across 20 suspend/resume cycles, asserting each store landed
//! after the handler re-armed.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

// The `mprotect` and `mmap` surface, declared rather than pulled in as a
// dependency. `fn64-abi` links no libc crate today and this is four symbols.
const PROT_NONE: i32 = 0x0;
const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const MAP_PRIVATE: i32 = 0x0002;
const MAP_ANON: i32 = 0x1000;
const MAP_FAILED: *mut u8 = usize::MAX as *mut u8;

const SIGSEGV: i32 = 11;
const SIGBUS: i32 = 10;
const SA_SIGINFO: i32 = 0x0040;
const SA_ONSTACK: i32 = 0x0001;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    fn sigaction(signal: i32, action: *const SigAction, old: *mut SigAction) -> i32;
    fn getpagesize() -> i32;
}

/// Darwin's `struct sigaction`, in the layout `sigaction(2)` expects.
///
/// Declared here rather than taken from a libc crate because the barrier needs
/// exactly this one structure. The field order is `__sigaction_u` (the handler
/// union), `sa_mask`, `sa_flags` -- `/usr/include/sys/signal.h`.
#[repr(C)]
#[derive(Clone, Copy)]
struct SigAction {
    handler: usize,
    mask: u32,
    flags: i32,
}

/// A process RDRAM allocation that owns whole pages.
///
/// Derefs to `[u8]`, so it substitutes for the `Box<[u8]>` it replaces at every
/// call site that only reads or writes bytes. The distinction that matters is
/// invisible through the `Deref`: this allocation starts on a page boundary and
/// no other object shares a page with it, so `mprotect` over any part of it
/// affects nothing else in the process.
pub struct PageAlignedRdram {
    base: *mut u8,
    /// Bytes the caller asked for.
    len: usize,
    /// Bytes actually mapped -- `len` rounded up to a page.
    mapped: usize,
}

// The allocation is owned exclusively by whoever holds this value; the pointer
// is not shared and the type hands out no interior references beyond the
// ordinary `&`/`&mut` slice borrows. Sending it between threads is no less safe
// than sending a `Box<[u8]>`, and `HostState` is thread-local regardless.
unsafe impl Send for PageAlignedRdram {}

impl PageAlignedRdram {
    /// Allocate `len` zeroed bytes on whole pages.
    ///
    /// `mmap` of anonymous memory is guaranteed zero-filled, which is what the
    /// `vec![0; len]` this replaces relied on.
    ///
    /// Returns `None` if the mapping fails. The caller must then fall back to a
    /// heap allocation and run without a barrier -- the guard's scan is
    /// unconditional, so that lane is exactly today's behaviour.
    pub fn new(len: usize) -> Option<Self> {
        assert!(len > 0, "process RDRAM allocation must be nonempty");
        let page = page_size();
        let mapped = len.div_ceil(page) * page;
        // SAFETY: a fresh anonymous private mapping; no fixed address, no file.
        let base = unsafe {
            mmap(
                std::ptr::null_mut(),
                mapped,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANON,
                -1,
                0,
            )
        };
        if base.is_null() || base == MAP_FAILED {
            return None;
        }
        debug_assert_eq!(base as usize % page, 0, "mmap returned an unaligned page");
        Some(Self { base, len, mapped })
    }

    /// The base pointer, which is page-aligned.
    pub fn as_ptr(&self) -> *mut u8 {
        self.base
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl std::ops::Deref for PageAlignedRdram {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        // SAFETY: `base` maps at least `len` readable bytes for our lifetime.
        unsafe { std::slice::from_raw_parts(self.base, self.len) }
    }
}

impl std::ops::DerefMut for PageAlignedRdram {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: as `deref`, and `&mut self` proves exclusive access.
        unsafe { std::slice::from_raw_parts_mut(self.base, self.len) }
    }
}

impl Drop for PageAlignedRdram {
    fn drop(&mut self) {
        // Restore write permission before unmapping. A page left `PROT_READ`
        // would be unmapped just the same, but leaving the barrier armed over
        // memory that is about to be recycled by the allocator is how a stray
        // fault ends up somewhere unexplainable.
        // SAFETY: our own mapping, whole length.
        unsafe {
            mprotect(self.base, self.mapped, PROT_READ | PROT_WRITE);
            munmap(self.base, self.mapped);
        }
    }
}

/// The process RDRAM allocation, however it was obtained.
///
/// Two shapes, because the barrier must degrade rather than fail: a
/// page-aligned `mmap` when one is available, and the ordinary heap allocation
/// this replaced when it is not. Both deref to `[u8]`, so nothing that reads
/// RDRAM bytes can tell them apart -- only [`Self::is_page_aligned`] can, and
/// only the barrier asks.
///
/// A heap allocation is not a degraded correctness lane. The guard's scan runs
/// unconditionally today and continues to; the barrier is an optimization that
/// declines to engage, and the boundary answers exactly as it does now.
pub enum ProcessRdram {
    /// Whole pages, owned exclusively. Protectable.
    Mapped(PageAlignedRdram),
    /// Ordinary heap. Not protectable; the guard scans.
    Heap(Box<[u8]>),
}

impl ProcessRdram {
    /// Allocate `len` zeroed bytes, page-aligned when possible.
    ///
    /// Falls back to the heap on `mmap` failure, which keeps boot working on a
    /// machine or configuration where the mapping is refused.
    ///
    /// `FN64_HEAP_RDRAM=1` forces the heap lane. That exists for the A/B: it
    /// puts the pre-change allocation and the page-aligned one in the SAME
    /// binary, so "the output is byte-identical" is a statement about one
    /// program under two settings rather than about two separately compiled
    /// programs that might differ for unrelated reasons.
    pub fn new(len: usize) -> Self {
        if heap_forced() {
            return Self::Heap(vec![0u8; len].into_boxed_slice());
        }
        match PageAlignedRdram::new(len) {
            Some(mapped) => Self::Mapped(mapped),
            None => Self::Heap(vec![0u8; len].into_boxed_slice()),
        }
    }

    /// Adopt an already-populated heap allocation.
    ///
    /// The bootstrap transaction builds RDRAM before this type exists; this is
    /// how the finished bytes arrive without a second copy when they are
    /// already heap-resident.
    pub fn from_boxed(storage: Box<[u8]>) -> Self {
        Self::Heap(storage)
    }

    /// Whether this allocation can be `mprotect`ed without touching anything
    /// else in the process.
    pub fn is_page_aligned(&self) -> bool {
        matches!(self, Self::Mapped(_))
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        match self {
            Self::Mapped(mapped) => mapped.as_ptr(),
            Self::Heap(heap) => heap.as_mut_ptr(),
        }
    }
}

impl std::ops::Deref for ProcessRdram {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Self::Mapped(mapped) => mapped,
            Self::Heap(heap) => heap,
        }
    }
}

impl std::ops::DerefMut for ProcessRdram {
    fn deref_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Mapped(mapped) => mapped,
            Self::Heap(heap) => heap,
        }
    }
}

/// Host page size, read once.
pub fn page_size() -> usize {
    static SIZE: AtomicUsize = AtomicUsize::new(0);
    let cached = SIZE.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    // SAFETY: no arguments, no state.
    let size = unsafe { getpagesize() } as usize;
    assert!(size.is_power_of_two(), "host page size is not a power of two");
    SIZE.store(size, Ordering::Relaxed);
    size
}

/// Whether the heap allocation lane is forced, for the A/B.
fn heap_forced() -> bool {
    static FORCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| std::env::var_os("FN64_HEAP_RDRAM").is_some_and(|value| value != "0"))
}

/// Whether the barrier is requested by the environment.
///
/// `FN64_MPROTECT_BARRIER=1` arms it. Off by default, so both lanes exist in
/// one binary and the A/B is a single environment variable rather than a
/// rebuild -- which is what makes "byte-identical output" a claim about the
/// same program rather than two programs.
pub fn requested() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("FN64_MPROTECT_BARRIER").is_some_and(|value| value != "0")
    })
}

/// Largest number of distinct dirty pages the barrier will track per boundary.
///
/// Beyond this the barrier gives up on the boundary and reports "everything may
/// be dirty", which routes the caller to the full scan. Sized well past the
/// measured distribution -- mean 0.68 pages per boundary, 98.7% at four or
/// fewer, and the fault/scan break-even is 9 -- so overflowing is both rare and
/// harmless: it costs the scan that would have run anyway.
const MAX_DIRTY_PAGES: usize = 512;

/// The armed barrier's state, reachable from a signal handler.
///
/// Every field is an atomic, and the dirty set is a fixed-size preallocated
/// array. Nothing here allocates, locks, or borrows, because the handler runs
/// with the runtime's `RefCell`s potentially borrowed and its locks
/// potentially held.
struct BarrierState {
    /// Whether a fault should be handled rather than forwarded.
    armed: AtomicBool,
    /// Inclusive-exclusive byte bounds of the protected span, page-aligned.
    protected_start: AtomicUsize,
    protected_end: AtomicUsize,
    /// Page index (relative to `protected_start`) per dirty slot.
    dirty: [AtomicU64; MAX_DIRTY_PAGES],
    /// Number of slots used. Saturates at `MAX_DIRTY_PAGES`, and a value of
    /// `MAX_DIRTY_PAGES` means "overflowed, assume everything dirty".
    dirty_len: AtomicUsize,
    /// A fault arrived that the barrier could not account for. Latched, never
    /// cleared: it means the barrier's picture of the region is incomplete for
    /// the rest of the process, so every subsequent boundary must scan.
    poisoned: AtomicBool,
}

#[allow(clippy::declare_interior_mutable_const)]
const DIRTY_INIT: AtomicU64 = AtomicU64::new(0);

static STATE: BarrierState = BarrierState {
    armed: AtomicBool::new(false),
    protected_start: AtomicUsize::new(0),
    protected_end: AtomicUsize::new(0),
    dirty: [DIRTY_INIT; MAX_DIRTY_PAGES],
    dirty_len: AtomicUsize::new(0),
    poisoned: AtomicBool::new(false),
};

/// The previously installed handlers, so a fault we do not own is forwarded to
/// whatever would have received it -- ordinarily the Rust runtime's, which
/// prints a segfault diagnosis. Swallowing an unrelated SIGSEGV would turn a
/// crash with a message into a silent infinite loop.
static PREVIOUS_SEGV: AtomicUsize = AtomicUsize::new(0);
static PREVIOUS_BUS: AtomicUsize = AtomicUsize::new(0);

/// `siginfo_t` prefix, far enough to reach `si_addr`.
///
/// Darwin's layout is `si_signo, si_errno, si_code, si_pid, si_uid, si_status,
/// si_addr, ...` -- six `int`/`id` words then the faulting address
/// (`/usr/include/sys/signal.h`). Only `si_addr` is read.
#[repr(C)]
struct SigInfo {
    signo: i32,
    errno: i32,
    code: i32,
    pid: i32,
    uid: u32,
    status: i32,
    addr: *mut u8,
}

/// The write-fault handler.
///
/// ASYNC-SIGNAL-SAFE BY CONSTRUCTION. Reads atomics, writes atomics, calls
/// `mprotect`. No allocation, no `RefCell`, no lock, no formatting, no libc
/// beyond the one syscall. Do not add anything here that could allocate or
/// take a lock: it runs on the guest store path with the runtime's state
/// arbitrarily borrowed.
extern "C" fn fault_handler(signal: i32, info: *mut SigInfo, context: *mut u8) {
    let handled = handle_fault(info);
    if handled {
        return;
    }
    // Not ours. Restore whatever was installed before and let it run, so an
    // ordinary segfault still produces its ordinary diagnosis rather than
    // spinning here forever on a fault we will never clear.
    forward(signal, info, context);
}

/// Record and re-arm, or report that this fault is not the barrier's.
///
/// Split out so the "is it ours" decision reads as one expression and the
/// forwarding path stays out of the fast path.
fn handle_fault(info: *mut SigInfo) -> bool {
    if !STATE.armed.load(Ordering::Relaxed) {
        return false;
    }
    if info.is_null() {
        return false;
    }
    // SAFETY: the kernel hands a valid `siginfo_t` with `SA_SIGINFO`, and only
    // the `si_addr` prefix is read.
    let addr = unsafe { (*info).addr } as usize;
    let start = STATE.protected_start.load(Ordering::Relaxed);
    let end = STATE.protected_end.load(Ordering::Relaxed);
    if addr < start || addr >= end {
        return false;
    }
    let page = page_size();
    let page_base = addr & !(page - 1);
    let index = ((page_base - start) / page) as u64;

    // Record the page. A linear scan over the used slots: the measured
    // distribution is 0.68 pages per boundary and 98.7% of boundaries are at
    // four or fewer, so this loop essentially never runs past a handful.
    let used = STATE.dirty_len.load(Ordering::Relaxed);
    if used < MAX_DIRTY_PAGES {
        let mut seen = false;
        for slot in 0..used {
            if STATE.dirty[slot].load(Ordering::Relaxed) == index {
                seen = true;
                break;
            }
        }
        if !seen {
            STATE.dirty[used].store(index, Ordering::Relaxed);
            STATE.dirty_len.store(used + 1, Ordering::Relaxed);
        }
    } else {
        // Overflow. The count stays pinned at capacity, which the boundary
        // reads as "assume everything dirty" and answers with a full scan.
        STATE.dirty_len.store(MAX_DIRTY_PAGES, Ordering::Relaxed);
    }

    // Re-arm this page writable so the faulting store can retire. This is the
    // whole mechanism: the page is now unprotected and further writes to it
    // cost nothing until the next boundary re-protects.
    // SAFETY: a page inside our own mapping.
    let ok = unsafe { mprotect(page_base as *mut u8, page, PROT_READ | PROT_WRITE) } == 0;
    if !ok {
        // Cannot clear the fault. Latch and let the default handler run rather
        // than returning into an instruction that will fault again forever.
        STATE.poisoned.store(true, Ordering::Relaxed);
        return false;
    }
    true
}

/// Restore the pre-barrier handler for `signal` and re-raise into it.
///
/// Reinstalling rather than calling directly: the previous handler may be
/// `SIG_DFL`, which is not a callable address. Returning after reinstalling
/// re-executes the faulting instruction, which faults again and is then
/// delivered to the restored handler.
fn forward(signal: i32, _info: *mut SigInfo, _context: *mut u8) {
    let slot = match signal {
        SIGBUS => &PREVIOUS_BUS,
        _ => &PREVIOUS_SEGV,
    };
    let previous = slot.load(Ordering::Relaxed);
    let action = SigAction {
        handler: previous,
        mask: 0,
        flags: SA_SIGINFO | SA_ONSTACK,
    };
    // SAFETY: `sigaction` with a valid action and no old-action out-pointer.
    unsafe { sigaction(signal, &action, std::ptr::null_mut()) };
}

/// Install the fault handler exactly once for the process.
///
/// Returns whether the barrier can be used at all.
fn install_handler() -> bool {
    static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *INSTALLED.get_or_init(|| {
        let action = SigAction {
            handler: fault_handler as *const () as usize,
            mask: 0,
            // SA_ONSTACK so a fault taken on a nearly-exhausted coroutine stack
            // still has somewhere to run. `corosensei` stacks are 8 MiB and the
            // handler uses a few words, but the flag costs nothing and its
            // absence is the failure mode with no diagnosis.
            flags: SA_SIGINFO | SA_ONSTACK,
        };
        for (signal, slot) in [(SIGSEGV, &PREVIOUS_SEGV), (SIGBUS, &PREVIOUS_BUS)] {
            let mut previous = SigAction {
                handler: 0,
                mask: 0,
                flags: 0,
            };
            // SAFETY: valid action and out-pointer.
            if unsafe { sigaction(signal, &action, &mut previous) } != 0 {
                return false;
            }
            slot.store(previous.handler, Ordering::Relaxed);
        }
        true
    })
}

/// The barrier over one process RDRAM allocation.
///
/// Construction installs the handler and records the region; it does not
/// protect anything. [`Self::arm`] protects, [`Self::take_dirty`] reads back
/// and unprotects.
pub struct Barrier {
    /// Page-aligned bounds of the protected span, as host addresses.
    start: usize,
    end: usize,
    /// The same span as RDRAM storage offsets, for translating dirty pages
    /// back into physical byte ranges.
    offset_start: usize,
    /// Whether the region ever got protected. `false` disables every operation
    /// and leaves the caller on the scan.
    usable: bool,
}

/// What a boundary learned from the barrier.
pub enum Dirty {
    /// The barrier was not armed, overflowed, or cannot answer. Scan.
    Unknown,
    /// Exactly these physical byte ranges may have changed; every byte outside
    /// them is provably identical to what it was when the barrier armed.
    Pages(Vec<(u32, u32)>),
}

impl Barrier {
    /// Prepare a barrier over `[offset_start, offset_end)` of the allocation at
    /// `base`.
    ///
    /// The span is widened outward to page boundaries, because protection has
    /// page granularity. Widening is safe in the direction that matters: it can
    /// only cause EXTRA faults for writes just outside the watched region,
    /// never miss one inside it. Those extra faults are already priced into the
    /// measured 0.68 pages/boundary, which counted every observed guest write
    /// rather than only writes to watched bytes.
    ///
    /// `base` must be page-aligned and the allocation must cover
    /// `offset_end` -- both guaranteed by [`PageAlignedRdram`].
    pub fn new(base: *mut u8, allocation_len: usize, offset_start: u32, offset_end: u32) -> Self {
        let unusable = Self {
            start: 0,
            end: 0,
            offset_start: 0,
            usable: false,
        };
        if !requested() {
            return unusable;
        }
        let page = page_size();
        if base.is_null() || base as usize % page != 0 {
            // A malloc'd allocation. Protecting it would protect whatever else
            // shares its edge pages, so refuse and stay on the scan.
            return unusable;
        }
        let (start_offset, end_offset) = (offset_start as usize, offset_end as usize);
        if end_offset > allocation_len || start_offset >= end_offset {
            return unusable;
        }
        let aligned_start = start_offset & !(page - 1);
        let aligned_end = end_offset.div_ceil(page) * page;
        if aligned_end > allocation_len {
            // The watched region's last page runs past the allocation. Only
            // possible if the allocation itself is not a whole number of pages,
            // which `PageAlignedRdram` rules out -- but refusing is cheaper
            // than reasoning about it.
            return unusable;
        }
        if !install_handler() {
            return unusable;
        }
        let start = base as usize + aligned_start;
        let end = base as usize + aligned_end;
        STATE.protected_start.store(start, Ordering::Relaxed);
        STATE.protected_end.store(end, Ordering::Relaxed);
        Self {
            start,
            end,
            offset_start: aligned_start,
            usable: true,
        }
    }

    pub fn usable(&self) -> bool {
        self.usable && !STATE.poisoned.load(Ordering::Relaxed)
    }

    /// Write-protect the region and begin recording.
    ///
    /// Called after the guard has proved the region matches its baseline, so
    /// "no page faulted since arming" and "no byte differs from the baseline"
    /// are the same statement -- which is what lets the next boundary skip the
    /// scan entirely.
    pub fn arm(&self) {
        if !self.usable() {
            return;
        }
        STATE.dirty_len.store(0, Ordering::Relaxed);
        // SAFETY: our own page-aligned mapping.
        let ok = unsafe { mprotect(self.start as *mut u8, self.end - self.start, PROT_READ) } == 0;
        if !ok {
            STATE.poisoned.store(true, Ordering::Relaxed);
            return;
        }
        STATE.armed.store(true, Ordering::SeqCst);
    }

    /// Stop recording, restore write permission, and report the dirty pages.
    ///
    /// The returned ranges are physical RDRAM byte offsets, ascending and
    /// disjoint, covering every page that took a fault since [`Self::arm`].
    /// [`Dirty::Unknown`] means the caller must scan.
    pub fn take_dirty(&self) -> Dirty {
        if !self.usable() || !STATE.armed.load(Ordering::SeqCst) {
            return Dirty::Unknown;
        }
        STATE.armed.store(false, Ordering::SeqCst);
        // Unprotect first. Between here and the next `arm` the region is
        // ordinary writable memory, which is what every host-side path
        // (baseline updates, DMA, the renderer) needs it to be.
        // SAFETY: our own page-aligned mapping.
        let ok = unsafe {
            mprotect(
                self.start as *mut u8,
                self.end - self.start,
                PROT_READ | PROT_WRITE,
            )
        } == 0;
        if !ok {
            STATE.poisoned.store(true, Ordering::Relaxed);
            return Dirty::Unknown;
        }
        let used = STATE.dirty_len.load(Ordering::Relaxed);
        if used >= MAX_DIRTY_PAGES {
            return Dirty::Unknown;
        }
        let page = page_size();
        let mut pages: Vec<u64> = (0..used)
            .map(|slot| STATE.dirty[slot].load(Ordering::Relaxed))
            .collect();
        pages.sort_unstable();
        pages.dedup();
        let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(pages.len());
        for index in pages {
            let lo = self.offset_start + index as usize * page;
            let hi = lo + page;
            let (lo, hi) = (lo as u32, hi as u32);
            // Coalesce adjacent pages so the byte comparison sees one span
            // rather than a run of 16 KiB pieces.
            match ranges.last_mut() {
                Some((_, end)) if *end == lo => *end = hi,
                _ => ranges.push((lo, hi)),
            }
        }
        Dirty::Pages(ranges)
    }

    /// Unprotect without reading the dirty set, for a host path that is about
    /// to write the region itself.
    ///
    /// Distinct from [`Self::take_dirty`] in intent only -- both unprotect --
    /// but a caller that discards the dirty set is asserting it will scan, and
    /// naming that is worth a second method.
    pub fn disarm(&self) {
        let _ = self.take_dirty();
    }
}

/// Force the barrier off for the rest of the process.
///
/// The escape hatch for a path that finds itself unable to reason about the
/// protection state. Latching rather than toggling: a barrier that has been
/// wrong once has no claim on the next boundary either.
pub fn poison() {
    STATE.poisoned.store(true, Ordering::Relaxed);
    STATE.armed.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_aligned_rdram_starts_on_a_page_and_reads_back_zero() {
        let rdram = PageAlignedRdram::new(8 * 1024 * 1024).expect("mmap");
        assert_eq!(rdram.as_ptr() as usize % page_size(), 0);
        assert_eq!(rdram.len(), 8 * 1024 * 1024);
        assert!(rdram.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn page_aligned_rdram_round_trips_bytes_like_a_boxed_slice() {
        let mut rdram = PageAlignedRdram::new(64 * 1024).expect("mmap");
        for (index, byte) in rdram.iter_mut().enumerate() {
            *byte = index as u8;
        }
        assert!(rdram.iter().enumerate().all(|(i, &b)| b == i as u8));
    }

    /// The property the whole design rests on: a write to a protected page is
    /// reported, and a page not written is not.
    ///
    /// Run only when the barrier is requested, because installing a process
    /// SIGSEGV handler inside a test binary that other tests share is not
    /// something to do unconditionally.
    #[test]
    fn a_protected_page_reports_exactly_the_pages_written() {
        if !requested() {
            return;
        }
        let page = page_size();
        let mut rdram = PageAlignedRdram::new(page * 8).expect("mmap");
        let base = rdram.as_ptr();
        let barrier = Barrier::new(base, rdram.len(), 0, (page * 8) as u32);
        if !barrier.usable() {
            return;
        }
        barrier.arm();
        rdram[page * 2 + 5] = 0xa5;
        rdram[page * 5] = 0x5a;
        let Dirty::Pages(ranges) = barrier.take_dirty() else {
            panic!("barrier returned Unknown for a two-page write");
        };
        assert_eq!(
            ranges,
            vec![
                ((page * 2) as u32, (page * 3) as u32),
                ((page * 5) as u32, (page * 6) as u32),
            ]
        );
        assert_eq!(rdram[page * 2 + 5], 0xa5);
        assert_eq!(rdram[page * 5], 0x5a);
    }
}
