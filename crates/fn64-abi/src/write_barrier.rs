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
    assert!(
        size.is_power_of_two(),
        "host page size is not a power of two"
    );
    SIZE.store(size, Ordering::Relaxed);
    size
}

/// Whether the heap allocation lane is forced, for the A/B.
fn heap_forced() -> bool {
    static FORCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| env_flag("FN64_HEAP_RDRAM"))
}

/// Read a boolean environment flag, where only an affirmative value is on.
///
/// Absent, empty, and `0` all mean off. Anything else affirmative -- `1`,
/// `true`, `yes`, `on`, any case, surrounding whitespace ignored -- means on,
/// and any other value means off.
///
/// One helper rather than three hand-rolled predicates because the hand-rolled
/// version got it wrong in a way that silently invalidated an A/B: see
/// [`requested`].
fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| {
        matches!(
            value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Whether the barrier is requested by the environment.
///
/// `FN64_MPROTECT_BARRIER=1` arms it. Off by default, so both lanes exist in
/// one binary and the A/B is a single environment variable rather than a
/// rebuild -- which is what makes "byte-identical output" a claim about the
/// same program rather than two programs.
///
/// AN EMPTY VALUE IS OFF, and that is not a detail. `FN64_MPROTECT_BARRIER=`
/// is how a shell writes "the off lane" in an inline `env` assignment, and the
/// first version of this used `var_os(..).is_some_and(|v| v != "0")`, under
/// which an empty-but-set variable read as ON. Both lanes of an A/B run with
/// `FN64_MPROTECT_BARRIER=` and `=1` were therefore the SAME lane, which
/// produced a fabricated 4.9x: the real comparison was against a binary from
/// before an unrelated renderer optimisation, not against the scan.
///
/// Treating only `1`/`true`/`yes`/`on` as on means an empty value, `0`, and an
/// absent variable all agree, so no spelling of "off" can silently mean "on".
pub fn requested() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| env_flag("FN64_MPROTECT_BARRIER"))
}

/// Count of `mprotect` calls and nanoseconds spent inside them, by call site.
///
/// # Why this exists before any optimisation did
///
/// The post-barrier profile attributed 50.9% of remaining self time to
/// `__mprotect` and inferred ~182,892 calls at ~1.7 us from the boundary count.
/// Both halves of that are inference: a sampling profiler attributes samples,
/// not calls, and a boundary count is not a syscall count -- the handler issues
/// one per FAULT, and `arm`/`take_dirty` each issue one per boundary they
/// actually run on, which is not every boundary.
///
/// A previous measurement on this same barrier was fabricated by exactly this
/// shape of unverified inference, so the count is counted. `FN64_MPROTECT_
/// BARRIER_SYSCALLS=1`; inert otherwise, and the gate is read once into a
/// `OnceLock` so the disabled lane is a predictable branch on a cached bool.
///
/// The clock is read only when enabled. `Instant::now` is a `mach_absolute_
/// time` on Darwin, cheap relative to the syscall it brackets but not free,
/// which is why the equivalence run is made with this off.
mod syscalls {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// One (count, nanoseconds) pair per call site.
    pub struct Site {
        pub calls: AtomicU64,
        pub nanos: AtomicU64,
    }

    impl Site {
        const fn new() -> Self {
            Self {
                calls: AtomicU64::new(0),
                nanos: AtomicU64::new(0),
            }
        }
    }

    /// `arm` -- protect the whole span at a boundary.
    pub static ARM: Site = Site::new();
    /// `take_dirty` -- unprotect the whole span at a boundary. Since the
    /// selective re-protect this is only the overflow and teardown fallback.
    pub static DISARM: Site = Site::new();
    /// `take_dirty` -- re-protect one faulted page, keeping the window open.
    pub static REPROTECT: Site = Site::new();
    /// The fault handler -- unprotect one page so the store can retire.
    pub static FAULT: Site = Site::new();

    pub fn enabled() -> bool {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENABLED.get_or_init(|| super::env_flag("FN64_MPROTECT_BARRIER_SYSCALLS"))
    }

    /// Record one call. Called from the signal handler for `FAULT`, so it does
    /// nothing but add to two atomics -- no allocation, no lock, no formatting.
    pub fn note(site: &Site, nanos: u64) {
        site.calls.fetch_add(1, Ordering::Relaxed);
        site.nanos.fetch_add(nanos, Ordering::Relaxed);
    }

    /// Print at exit, for the same reason the served/fell-back stats do: the
    /// harness `main` is hashed into the program identity and must not change.
    pub fn arm_report() {
        if !enabled() {
            return;
        }
        extern "C" fn at_exit() {
            let line = |name: &str, site: &Site| {
                let calls = site.calls.load(Ordering::Relaxed);
                let nanos = site.nanos.load(Ordering::Relaxed);
                let mean = if calls == 0 {
                    0.0
                } else {
                    nanos as f64 / calls as f64
                };
                format!(
                    "{name}={calls} ({:.1}ms, {mean:.0}ns each)",
                    nanos as f64 / 1e6
                )
            };
            let sites = [
                ("arm", &ARM),
                ("disarm", &DISARM),
                ("reprotect", &REPROTECT),
                ("fault", &FAULT),
            ];
            let total_calls: u64 = sites
                .iter()
                .map(|(_, site)| site.calls.load(Ordering::Relaxed))
                .sum();
            let total_nanos: u64 = sites
                .iter()
                .map(|(_, site)| site.nanos.load(Ordering::Relaxed))
                .sum();
            let breakdown = sites
                .iter()
                .map(|(name, site)| line(name, site))
                .collect::<Vec<_>>()
                .join(" ");
            println!(
                "[mprotect-syscalls] total={total_calls} ({:.1}ms) {breakdown}",
                total_nanos as f64 / 1e6,
            );
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
        }
        static ARMED: std::sync::Once = std::sync::Once::new();
        ARMED.call_once(|| {
            extern "C" {
                fn atexit(f: extern "C" fn()) -> i32;
            }
            // SAFETY: a plain `extern "C" fn()` with no arguments.
            unsafe { atexit(at_exit) };
        });
    }
}

/// `mprotect`, timed and counted when the syscall census is on.
///
/// One wrapper rather than three timing blocks, so no call site can be counted
/// at one place and not another -- which is how a census undercounts and then
/// "proves" the syscall is not the cost.
///
/// # Safety
///
/// As `mprotect`: `addr` must be page-aligned and `[addr, addr + len)` inside a
/// mapping this process owns.
unsafe fn timed_mprotect(site: &syscalls::Site, addr: *mut u8, len: usize, prot: i32) -> i32 {
    if !syscalls::enabled() {
        return mprotect(addr, len, prot);
    }
    let started = std::time::Instant::now();
    let result = mprotect(addr, len, prot);
    syscalls::note(site, started.elapsed().as_nanos() as u64);
    result
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
    let ok = unsafe {
        timed_mprotect(
            &syscalls::FAULT,
            page_base as *mut u8,
            page,
            PROT_READ | PROT_WRITE,
        )
    } == 0;
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
    /// The barrier overflowed, was refused, or cannot answer. Scan.
    ///
    /// This is the only variant that invalidates a previously captured set:
    /// it means the barrier's picture is incomplete, so nothing derived from
    /// it can be trusted.
    Unknown,
    /// The barrier was already disarmed, so there is nothing new to report.
    ///
    /// NOT the same as `Unknown`. A set captured by the disarm that already
    /// happened remains valid -- the region has not been executing guest code
    /// since -- so the caller must keep it rather than fall back to a scan.
    /// Conflating the two made the second read within one boundary discard the
    /// first read's pages, which is how a real mutation got through.
    AlreadyDisarmed,
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
    ///
    /// Idempotent, and that idempotence now carries the common case: every
    /// [`Self::take_dirty`] that answered leaves the region armed and fully
    /// protected, so this has nothing to do and issues no syscall. What remains
    /// for it is the FIRST arm of a window that was genuinely torn down -- the
    /// initial arm after sealing, and the arm after an overflow, a
    /// `force_disarm` or a poison-free fallback. See [`Self::take_dirty`] for
    /// why staying protected across a boundary is sound.
    pub fn arm(&self) {
        if !self.usable() {
            return;
        }
        syscalls::arm_report();
        if STATE.armed.load(Ordering::SeqCst) && STATE.dirty_len.load(Ordering::Relaxed) == 0 {
            // Already armed with an empty dirty set -- the state this call
            // would establish. The region is protected, so nothing has been
            // written unobserved since; re-issuing `mprotect(PROT_READ)` over
            // an already-`PROT_READ` span would be a 1.2us no-op.
            //
            // The `dirty_len == 0` half is load-bearing, not belt-and-braces.
            // Armed with a NONEMPTY set means faults were recorded that no
            // boundary has consumed, and arming must clear them: those pages
            // were absorbed into the baseline by the caller that just proved
            // the region clean, so carrying them into the next window would
            // re-report absorbed writes as changes.
            return;
        }
        STATE.dirty_len.store(0, Ordering::Relaxed);
        // SAFETY: our own page-aligned mapping.
        let ok = unsafe {
            timed_mprotect(
                &syscalls::ARM,
                self.start as *mut u8,
                self.end - self.start,
                PROT_READ,
            )
        } == 0;
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
    ///
    /// # The boundary never unprotects the span, and that is the optimisation
    ///
    /// A boundary used to unprotect the whole 1.44 MiB span here and re-protect
    /// it in the next [`Self::arm`]. Neither call is needed, because the span's
    /// protection state at the end of a boundary is the state it should have at
    /// the start of the next one:
    ///
    /// - 75% of boundaries take zero faults, so the span is already entirely
    ///   `PROT_READ` and there is nothing at all to do.
    /// - On the rest, the pages the handler unprotected are the ONLY writable
    ///   pages of the span, and re-protecting just those (0.2532 per boundary
    ///   on average) restores the armed state without touching the other 87.
    ///
    /// Either way `armed` stays true and the recording window continues rather
    /// than being torn down and rebuilt.
    ///
    /// This is sound because it makes the barrier STRICTLY MORE protected than
    /// the disarming version, never less. The invariant `arm` establishes --
    /// every byte of the region is write-protected and the recorded dirty set
    /// describes every write since the baseline was proven -- holds continuously
    /// rather than being torn down and rebuilt. A host write landing in the
    /// window that used to be unprotected now faults and is RECORDED, where
    /// before it was silently permitted and the boundary relied on the caller
    /// having invalidated. Recording strictly more is the safe direction: the
    /// dirty set is already a superset of the changed-byte set by construction.
    ///
    /// ## Why this does not reintroduce the stale-window bug
    ///
    /// The bug documented at [`guard::dirty_spans`] is a caller reading a set
    /// that describes an OLDER window than its own question. The defence is that
    /// the set is consuming: a boundary that reads it leaves `None` behind, so a
    /// path that fails to `arm` gets `None` and scans, rather than getting a set
    /// that predates it and treating every page outside it as proven-unchanged.
    ///
    /// That defence is untouched here, because this changes only whether two
    /// syscalls are issued, never what the dirty set says or when it is
    /// consumed. `guard::dirty_spans` still calls `disarm_and_capture` and still
    /// `take`s `PENDING` in the same operation. Concretely, the two properties
    /// the defence needs both survive:
    ///
    /// - The set still describes the window ENDING NOW. Staying armed does not
    ///   carry pages forward, because the set is CLEARED as the window is
    ///   closed -- either it was already empty, or the selective re-protect
    ///   clears it after re-protecting exactly the pages it names. The window
    ///   that continues therefore starts from empty, so there is no older
    ///   window whose pages could leak into a later answer.
    /// - A missed `arm` still costs a scan and never soundness. If the boundary
    ///   returns without arming, `PENDING` is `None` and the next boundary
    ///   scans -- and the region is *still protected*, so the pages that fault
    ///   meanwhile are recorded rather than lost. The failure mode is strictly
    ///   better than before, not worse.
    ///
    /// The asymmetry the design rests on is preserved exactly: no caller is
    /// trusted to disarm, because `dirty_spans` still reads and closes in one
    /// operation. What changes is that "closing" a window with nothing in it no
    /// longer requires making the region writable in order to make it read-only
    /// again a moment later.
    pub fn take_dirty(&self) -> Dirty {
        if !self.usable() {
            return Dirty::Unknown;
        }
        if !STATE.armed.load(Ordering::SeqCst) {
            // Already disarmed. Distinct from `Unknown`: there is nothing NEW
            // to report, but the set captured by the disarm that already
            // happened is still valid and must not be discarded. Collapsing
            // this into `Unknown` is what made the second read of a boundary
            // wipe the first read's pages.
            return Dirty::AlreadyDisarmed;
        }
        if STATE.dirty_len.load(Ordering::Relaxed) == 0 {
            // Nothing faulted. The region is already exactly as `arm` would
            // leave it, so leave it -- armed, protected, and recording. Read
            // AFTER the `armed` check and BEFORE clearing anything, so the
            // ordinary path below is reached with the count unchanged.
            //
            // Returning an empty page list rather than `AlreadyDisarmed` is
            // what makes this an optimisation rather than a behaviour change:
            // the caller learns "no page changed", which is what the disarming
            // version told it too.
            return Dirty::Pages(Vec::new());
        }
        let used = STATE.dirty_len.load(Ordering::Relaxed);
        if used >= MAX_DIRTY_PAGES {
            // Overflowed: the barrier does not know which pages are writable,
            // so it cannot re-protect selectively. Fall back to the whole-span
            // restore and hand the caller the scan.
            STATE.armed.store(false, Ordering::SeqCst);
            // SAFETY: our own page-aligned mapping.
            let ok = unsafe {
                timed_mprotect(
                    &syscalls::DISARM,
                    self.start as *mut u8,
                    self.end - self.start,
                    PROT_READ | PROT_WRITE,
                )
            } == 0;
            if !ok {
                STATE.poisoned.store(true, Ordering::Relaxed);
            }
            return Dirty::Unknown;
        }
        let page = page_size();
        let mut pages: Vec<u64> = (0..used)
            .map(|slot| STATE.dirty[slot].load(Ordering::Relaxed))
            .collect();
        pages.sort_unstable();
        pages.dedup();

        // Re-protect ONLY the pages that faulted, and stay armed.
        //
        // # Why the whole-span pair is not needed here either
        //
        // The faulted pages are, by construction, the ONLY pages of the span
        // that are writable: `arm` protected all of them and the handler
        // unprotects exactly one page per fault. So the whole-span unprotect
        // this used to do, followed by the whole-span re-protect in the next
        // `arm`, differed from "re-protect the faulted pages" only in the
        // window between them -- and that window is precisely where the clean
        // boundary already stopped unprotecting, for the reasons given above.
        //
        // The same soundness argument applies unchanged, and applies MORE
        // strongly: staying protected can only cause a write to be recorded
        // that would previously have been permitted silently. The dirty set is
        // a superset of the changed-byte set by construction, and this can only
        // enlarge the superset, never puncture it.
        //
        // The count is 0.2532 dirty pages per served boundary against a
        // 1.44 MiB span, so this replaces a ~1.3us whole-span call with a
        // ~0.37us single-page one on the ~25% of boundaries that are dirty --
        // and replaces the next `arm`'s whole-span call with nothing.
        //
        // Clearing the dirty set here rather than in `arm` is what lets the
        // window continue: the caller is about to absorb these pages into the
        // baseline, and the window that continues starts from clean.
        //
        // If any re-protect fails the barrier's picture of which pages are
        // writable is no longer complete, which is exactly what `poison` means.
        // Poisoning restores write permission over the whole span, so the
        // failure is a fallback to the scan rather than a stuck page.
        for &index in &pages {
            let page_base = self.start + index as usize * page;
            // SAFETY: a page inside our own mapping, from an index the handler
            // derived from this same span.
            let ok = unsafe {
                timed_mprotect(&syscalls::REPROTECT, page_base as *mut u8, page, PROT_READ)
            } == 0;
            if !ok {
                poison();
                return Dirty::Unknown;
            }
        }
        STATE.dirty_len.store(0, Ordering::Relaxed);
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

    /// Unprotect unconditionally and stop recording, discarding the dirty set.
    ///
    /// Distinct from [`Self::take_dirty`], which since the clean-boundary skip
    /// may leave the region protected: this one always makes the region
    /// ordinary writable memory. That is what a path which is about to write
    /// the region from the host -- or to drop the mapping -- actually needs,
    /// and it is not something a "did anything fault" answer can provide.
    ///
    /// Discarding the dirty set is safe in the only direction that matters: the
    /// caller is asserting it will scan, and a discarded set means the next
    /// boundary finds `None` and does exactly that.
    pub fn force_disarm(&self) {
        if !self.usable() {
            return;
        }
        STATE.armed.store(false, Ordering::SeqCst);
        STATE.dirty_len.store(0, Ordering::Relaxed);
        // SAFETY: our own page-aligned mapping.
        let ok = unsafe {
            timed_mprotect(
                &syscalls::DISARM,
                self.start as *mut u8,
                self.end - self.start,
                PROT_READ | PROT_WRITE,
            )
        } == 0;
        if !ok {
            STATE.poisoned.store(true, Ordering::Relaxed);
        }
    }
}

/// Force the barrier off for the rest of the process.
///
/// The escape hatch for a path that finds itself unable to reason about the
/// protection state. Latching rather than toggling: a barrier that has been
/// wrong once has no claim on the next boundary either.
///
/// Restores write permission over the recorded span on the way out. A poisoned
/// barrier answers nothing and every boundary scans, but the region must still
/// be WRITABLE for the process to keep running -- and since the clean-boundary
/// skip leaves it protected across boundaries, "poisoned" and "unprotected" no
/// longer coincide by accident. Clearing the flags without clearing the
/// protection would leave every subsequent guest store faulting into a handler
/// that has just declared it will not handle anything.
///
/// Best-effort by construction: if the `mprotect` fails there is nothing left
/// to escalate to, and the flags are already latched.
pub fn poison() {
    let start = STATE.protected_start.load(Ordering::Relaxed);
    let end = STATE.protected_end.load(Ordering::Relaxed);
    STATE.poisoned.store(true, Ordering::Relaxed);
    STATE.armed.store(false, Ordering::SeqCst);
    if start != 0 && end > start {
        // SAFETY: the span the barrier recorded, inside our own mapping.
        unsafe { mprotect(start as *mut u8, end - start, PROT_READ | PROT_WRITE) };
    }
}

/// The process's single barrier, and the guard's view of it.
///
/// RDRAM is a process singleton and so is the watched region, so the barrier is
/// too. This module is the seam the mutation guard talks to; it owns the
/// arm/disarm lifecycle so no call site has to reason about protection state.
///
/// # The lifecycle invariant
///
/// The barrier may be armed only while the watched region is known to equal the
/// baseline, because "no page faulted since arming" is only useful if it means
/// "no byte differs from the baseline". The guard establishes exactly that fact
/// at every boundary it passes, so arming immediately after a passed boundary
/// is sound and arming anywhere else is not.
///
/// Everything that could write RDRAM or the baseline outside that window
/// disarms first. Disarming is unconditionally safe -- it can only cost a scan.
/// Arming is the operation with a precondition, and it has exactly one caller.
pub mod guard {
    use super::{page_size, requested, Barrier, Dirty};
    use std::cell::RefCell;

    thread_local! {
        /// The armed barrier, if any. Thread-local because the executor and
        /// everything that touches RDRAM is; the SIGNAL state behind it is
        /// process-global, which is correct because a signal handler cannot
        /// read a `thread_local!`.
        static BARRIER: RefCell<Option<Barrier>> = const { RefCell::new(None) };
        /// The dirty set the last disarm produced, waiting to be consumed by
        /// the boundary that disarmed.
        static PENDING: RefCell<Option<Vec<(u32, u32)>>> = const { RefCell::new(None) };
    }

    /// Bind the barrier to the installed RDRAM and the watched region.
    ///
    /// Idempotent and cheap to call; does nothing unless the barrier is
    /// requested and the allocation is page-aligned.
    pub fn bind(base: *mut u8, allocation_len: usize, watched: &[(u32, u32)]) {
        if !requested() {
            return;
        }
        let (Some(first), Some(last)) = (watched.first(), watched.last()) else {
            return;
        };
        BARRIER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return;
            }
            // Protect the whole span from the first watched byte to the last.
            // Watched ranges are ascending and, on the routes that matter, a
            // single contiguous bank; covering any gap between them only adds
            // spurious faults, which cost a re-arm and never a missed write.
            *slot = Some(Barrier::new(base, allocation_len, first.0, last.1));
        });
    }

    /// Whether the barrier can answer for this boundary.
    pub fn active() -> bool {
        BARRIER.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|barrier| barrier.usable())
        })
    }

    /// Stop recording and keep the dirty set for [`take_dirty_spans`].
    ///
    /// Called at the top of every boundary and before every host write. Safe
    /// to call when not armed.
    pub fn disarm_and_capture() {
        BARRIER.with(|slot| {
            let slot = slot.borrow();
            let Some(barrier) = slot.as_ref() else { return };
            match barrier.take_dirty() {
                Dirty::Pages(pages) => PENDING.with(|pending| {
                    let mut pending = pending.borrow_mut();
                    // Union with anything not yet consumed: two disarms before
                    // a boundary reads the set must not lose the first one's
                    // pages. Losing a page is the one failure that could make
                    // the barrier miss a mutation, so this merges rather than
                    // replaces.
                    match pending.as_mut() {
                        Some(existing) => existing.extend_from_slice(&pages),
                        None => *pending = Some(pages),
                    }
                }),
                // Nothing new; keep whatever an earlier disarm captured.
                Dirty::AlreadyDisarmed => {}
                // Unknown poisons the pending set: the boundary must scan.
                Dirty::Unknown => PENDING.with(|pending| *pending.borrow_mut() = None),
            }
        });
    }

    /// Take the barrier fully down: unprotect the region and stop recording.
    ///
    /// For teardown and for any host path that needs the region to be ORDINARY
    /// WRITABLE MEMORY rather than merely "observed". [`disarm_and_capture`]
    /// cannot serve that need since the clean-boundary skip, because a boundary
    /// with no faults deliberately leaves the region protected.
    ///
    /// The distinction matters exactly once and it matters absolutely: process
    /// exit writes RDRAM from several paths and then DROPS the mapping. A page
    /// left `PROT_READ` would fault somewhere in that sequence with the handler
    /// already naming memory the allocator is about to reclaim.
    pub fn force_disarm() {
        BARRIER.with(|slot| {
            let slot = slot.borrow();
            if let Some(barrier) = slot.as_ref() {
                barrier.force_disarm();
            }
        });
    }

    /// Throw away any recorded dirty set, forcing the next boundary to scan.
    ///
    /// The correct response to anything that makes the barrier's picture
    /// incomplete -- a host path that wrote RDRAM while disarmed, a baseline
    /// change, a reseal.
    pub fn invalidate() {
        PENDING.with(|pending| *pending.borrow_mut() = None);
    }

    /// The dirty spans as of NOW, or `None` meaning "scan everything".
    ///
    /// # This closes the recording window itself, and that is not optional
    ///
    /// The faults live in the handler's array until a `take_dirty` moves them
    /// here. An earlier version left that to the boundary's entry point and
    /// let this be a pure read of what had already been captured -- and that
    /// silently broke the guard.
    ///
    /// `reconcile_matched_before_dispatch` (`live_program.rs:2159`, `:2194`)
    /// and `flush_host_abi_transaction` (`execution.rs:714`) reach the
    /// comparison WITHOUT passing through
    /// `invalidate_pending_physical_writes_inner`, so nothing had captured for
    /// them. They read an empty leftover set and concluded "no page was
    /// written, therefore nothing changed" while the handler was holding the
    /// pages that said otherwise. On WM2000's deep route that surfaced as
    ///
    /// ```text
    /// unjournaled executable mutation changed physical RDRAM
    /// [0x00086090, 0x00086094) before canonical static dispatch
    /// ```
    ///
    /// -- a real mutation the full scan catches and the barrier missed. It is
    /// exactly the failure the barrier must never have.
    ///
    /// Capturing here instead makes the invariant structural rather than a
    /// convention every call site has to remember: THE DIRTY SET IS READ AND
    /// CLOSED IN ONE OPERATION, so a caller cannot obtain a set that predates
    /// its own question. New comparison sites inherit it for free.
    ///
    /// Idempotent within a boundary. The second and third asks -- the commit
    /// after the reconcile, and the `debug_assert` that re-derives the list to
    /// check the reuse -- take an already-disarmed barrier, whose `take_dirty`
    /// returns `Unknown` without clearing, so `PENDING` still holds the union
    /// captured by the first ask. All of them see the same answer, which is
    /// correct because no guest code runs between them.
    pub fn dirty_spans() -> Option<Vec<(u32, u32)>> {
        if !active() {
            return None;
        }
        disarm_and_capture();
        // CONSUMING. The set describes one window: the interval between the
        // `arm` that opened it and the disarm just performed. Once a boundary
        // reads it, the only thing that may produce another is a fresh `arm`.
        //
        // This is what makes a missed arm cost a scan instead of corrupting
        // the guard. If a boundary path returns without arming, the next
        // boundary finds `None` here and scans -- rather than finding a set
        // that describes an OLDER window and treating every page outside it as
        // proven-unchanged, which is exactly how a real mutation got through
        // on WM2000's deep route.
        //
        // Within one boundary the repeated asks still agree, because
        // `arm_after_proven_clean` has not run between them: the second ask
        // gets `None` and scans, which is correct but slower. The reconcile
        // arms immediately on its match path, so the common case reads once.
        PENDING.with(|pending| pending.borrow_mut().take())
    }

    /// Re-protect the region.
    ///
    /// PRECONDITION, and the only one in this module: the watched region must
    /// currently equal the baseline. The caller is the boundary that just
    /// proved it. Arming when that is false would let the next boundary
    /// conclude "no faults, therefore unchanged" about a region that was
    /// already different.
    ///
    /// # Why forgetting to call this is safe, and forgetting to disarm is not
    ///
    /// This asymmetry is deliberate and it is what makes the integration
    /// tractable. There are many boundary paths and no reliable way to
    /// enumerate them all by inspection; a design in which missing one is
    /// unsound would be a design that cannot be verified.
    ///
    /// Missing an ARM leaves the barrier down. `dirty_spans` then finds the
    /// region unprotected, reports `None`, and the boundary runs the full
    /// scan. Cost: one scan. Correctness: unchanged.
    ///
    /// Missing a DISARM would be unsound -- the boundary would read a stale
    /// set. Which is why no caller is trusted to disarm: `dirty_spans` does it
    /// itself, in the same operation as the read, so the two cannot separate.
    pub fn arm_after_proven_clean() {
        BARRIER.with(|slot| {
            let slot = slot.borrow();
            if let Some(barrier) = slot.as_ref() {
                barrier.arm();
            }
        });
        // The freshly armed region is clean by the precondition, so no page is
        // dirty yet. Any set left over from before is stale and must go: it
        // describes writes that the baseline has since absorbed, and carrying
        // it forward would re-report them as changes at the next boundary.
        PENDING.with(|pending| *pending.borrow_mut() = Some(Vec::new()));
    }

    /// Clip a dirty span list to one watched range, in that range's own terms.
    ///
    /// Returns the byte ranges of `[range_start, range_end)` that the barrier
    /// says may have changed, clipped and ascending. An empty result means the
    /// barrier proved this whole watched range untouched.
    pub fn clip(spans: &[(u32, u32)], range_start: u32, range_end: u32) -> Vec<(u32, u32)> {
        let mut clipped: Vec<(u32, u32)> = Vec::new();
        for &(lo, hi) in spans {
            let lo = lo.max(range_start);
            let hi = hi.min(range_end);
            if lo >= hi {
                continue;
            }
            match clipped.last_mut() {
                Some((_, end)) if *end >= lo => *end = (*end).max(hi),
                _ => clipped.push((lo, hi)),
            }
        }
        clipped
    }

    /// Sort and merge a dirty span list so `clip` sees ascending, disjoint
    /// input.
    ///
    /// The union in `disarm_and_capture` can leave the list unsorted and
    /// overlapping, and `clip`'s coalescing assumes ascending order.
    pub fn normalize(mut spans: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
        spans.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(spans.len());
        for (lo, hi) in spans {
            match merged.last_mut() {
                Some((_, end)) if *end >= lo => *end = (*end).max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        merged
    }

    /// Page granule, for tests and diagnostics.
    pub fn granule() -> usize {
        page_size()
    }

    /// How often the barrier actually served a boundary, versus falling back.
    ///
    /// A speedup alone cannot distinguish "the barrier is doing the work" from
    /// "the barrier is cheap and something else got faster", and a fallback is
    /// silent by design. This counts both outcomes so the claim can be checked
    /// rather than inferred. `FN64_MPROTECT_BARRIER_STATS=1`; inert otherwise.
    pub mod stats {
        use std::sync::atomic::{AtomicU64, Ordering};

        static SERVED: AtomicU64 = AtomicU64::new(0);
        static FELL_BACK: AtomicU64 = AtomicU64::new(0);
        static DIRTY_PAGES: AtomicU64 = AtomicU64::new(0);
        static CLEAN_BOUNDARIES: AtomicU64 = AtomicU64::new(0);

        pub fn enabled() -> bool {
            static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *ENABLED.get_or_init(|| super::super::env_flag("FN64_MPROTECT_BARRIER_STATS"))
        }

        /// Running totals for the counters `frame_census` samples per VI
        /// field: `(served, fell_back, dirty_pages, clean_boundaries)`.
        ///
        /// Four relaxed loads. When `FN64_MPROTECT_BARRIER_STATS` is off these
        /// all read zero, which is the correct answer -- nothing was counted --
        /// and the bimodal census reports the bucket difference as "no data"
        /// rather than as "no difference".
        pub fn running_totals() -> (u64, u64, u64, u64) {
            (
                SERVED.load(Ordering::Relaxed),
                FELL_BACK.load(Ordering::Relaxed),
                DIRTY_PAGES.load(Ordering::Relaxed),
                CLEAN_BOUNDARIES.load(Ordering::Relaxed),
            )
        }

        /// Record the outcome of one boundary's ask.
        pub fn note(spans: Option<&Vec<(u32, u32)>>) {
            if !enabled() {
                return;
            }
            arm_report();
            match spans {
                Some(spans) => {
                    SERVED.fetch_add(1, Ordering::Relaxed);
                    if spans.is_empty() {
                        CLEAN_BOUNDARIES.fetch_add(1, Ordering::Relaxed);
                    }
                    let granule = super::page_size() as u64;
                    let pages: u64 = spans
                        .iter()
                        .map(|&(lo, hi)| (u64::from(hi) - u64::from(lo)).div_ceil(granule))
                        .sum();
                    DIRTY_PAGES.fetch_add(pages, Ordering::Relaxed);
                }
                None => {
                    FELL_BACK.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        /// Print at exit rather than from a harness `main`.
        ///
        /// `examples/wm2000-block-boot/src/main.rs` is hashed verbatim into
        /// `DISPATCH_SOURCE_SHA256` (`build.rs:794`), so any edit to it -- even
        /// a comment -- changes the canonical program identity and invalidates
        /// the A/B. Reporting from here keeps the measured program identical to
        /// the unmeasured one.
        fn arm_report() {
            extern "C" fn at_exit() {
                let served = SERVED.load(Ordering::Relaxed);
                let fell_back = FELL_BACK.load(Ordering::Relaxed);
                let total = served + fell_back;
                let pages = DIRTY_PAGES.load(Ordering::Relaxed);
                let clean = CLEAN_BOUNDARIES.load(Ordering::Relaxed);
                let share = |n: u64| {
                    if total == 0 {
                        0.0
                    } else {
                        100.0 * n as f64 / total as f64
                    }
                };
                let mean = if served == 0 {
                    0.0
                } else {
                    pages as f64 / served as f64
                };
                println!(
                    "[mprotect-barrier] boundaries={total} served={served} ({:.2}%) \
                     fell_back={fell_back} ({:.2}%) clean={clean} ({:.2}%) \
                     mean_dirty_pages_per_served={mean:.4}",
                    share(served),
                    share(fell_back),
                    share(clean),
                );
                use std::io::Write as _;
                let _ = std::io::stdout().flush();
            }
            static ARMED: std::sync::Once = std::sync::Once::new();
            ARMED.call_once(|| {
                extern "C" {
                    fn atexit(f: extern "C" fn()) -> i32;
                }
                unsafe { atexit(at_exit) };
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every spelling of "off" must be off, and only affirmatives on.
    ///
    /// This pins the bug that fabricated a 4.9x result. The gate was
    /// `var_os(..).is_some_and(|v| v != "0")`, so `FN64_MPROTECT_BARRIER=` --
    /// set but empty, which is exactly how a shell writes the off lane in an
    /// inline `env` assignment -- read as ON. Both lanes of the A/B were the
    /// barrier lane, and the "scan lane" number it was compared against came
    /// from a binary predating an unrelated renderer optimisation.
    ///
    /// The env var itself cannot be exercised here (the gates are `OnceLock`
    /// and the test binary is shared), so this tests the predicate directly.
    #[test]
    fn only_affirmative_env_values_enable_a_flag() {
        fn decide(value: &str) -> bool {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        }
        // The one that mattered: empty is OFF.
        assert!(!decide(""), "an empty value must be off, not on");
        assert!(!decide("0"));
        assert!(!decide("false"));
        assert!(!decide("no"));
        assert!(!decide("off"));
        assert!(!decide("  "));
        for on in ["1", "true", "TRUE", "Yes", "on", " 1 "] {
            assert!(decide(on), "{on:?} must enable the flag");
        }
    }

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

    /// A clean boundary must stay protected, and the next window must still
    /// report a write made after it.
    ///
    /// This is the property the clean-boundary skip rests on. The failure it
    /// pins is the one that would make the skip unsound: if the elided
    /// `arm`/`disarm` pair left the region WRITABLE, a write in the following
    /// window would not fault and the boundary after it would report clean over
    /// changed bytes -- exactly the missed mutation the barrier exists to
    /// prevent.
    #[test]
    fn a_clean_boundary_stays_armed_and_the_next_write_is_still_reported() {
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

        // A boundary with no writes: clean, and deliberately still protected.
        let Dirty::Pages(ranges) = barrier.take_dirty() else {
            panic!("a clean boundary must report pages, not Unknown");
        };
        assert!(
            ranges.is_empty(),
            "nothing was written, so nothing is dirty"
        );

        // `arm` over an already-armed clean region is a no-op, not a reset.
        barrier.arm();

        // The write lands in the continued window and must be observed.
        rdram[page * 3 + 7] = 0xc3;
        let Dirty::Pages(ranges) = barrier.take_dirty() else {
            panic!("barrier returned Unknown for a one-page write");
        };
        assert_eq!(
            ranges,
            vec![((page * 3) as u32, (page * 4) as u32)],
            "a write after a clean boundary must still be reported"
        );
        assert_eq!(rdram[page * 3 + 7], 0xc3);
    }

    /// A dirty boundary re-protects the faulted pages and keeps recording.
    ///
    /// The property the selective re-protect rests on. If a re-protected page
    /// were left writable, the SECOND write to it would not fault and the
    /// boundary after that would report the page clean over changed bytes.
    /// Writing the same page in two consecutive windows is what pins it.
    #[test]
    fn a_dirty_boundary_reprotects_and_the_same_page_faults_again() {
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

        rdram[page * 4 + 1] = 0x11;
        let Dirty::Pages(first) = barrier.take_dirty() else {
            panic!("barrier returned Unknown for a one-page write");
        };
        assert_eq!(first, vec![((page * 4) as u32, (page * 5) as u32)]);

        // The window continues; `arm` is a no-op. Writing the SAME page again
        // must fault again, which it can only do if the re-protect happened.
        barrier.arm();
        rdram[page * 4 + 2] = 0x22;
        let Dirty::Pages(second) = barrier.take_dirty() else {
            panic!("barrier returned Unknown for the second write");
        };
        assert_eq!(
            second,
            vec![((page * 4) as u32, (page * 5) as u32)],
            "a page re-protected at the previous boundary must fault again"
        );

        // And a boundary with no write is clean, so the set really was cleared
        // rather than carried forward.
        barrier.arm();
        let Dirty::Pages(third) = barrier.take_dirty() else {
            panic!("barrier returned Unknown for a clean boundary");
        };
        assert!(
            third.is_empty(),
            "the previous window's pages must not carry forward: {third:?}"
        );
        assert_eq!(rdram[page * 4 + 1], 0x11);
        assert_eq!(rdram[page * 4 + 2], 0x22);
    }

    /// `force_disarm` must leave the region genuinely writable.
    ///
    /// Teardown depends on this: `prepare_process_exit` writes RDRAM from
    /// several paths and then drops the mapping. Since a clean `take_dirty` now
    /// leaves the region protected, "the barrier is down" and "the region is
    /// writable" are no longer the same statement, and this is the method that
    /// promises the second one.
    #[test]
    fn force_disarm_leaves_the_region_writable_after_a_clean_boundary() {
        if !requested() {
            return;
        }
        let page = page_size();
        let mut rdram = PageAlignedRdram::new(page * 4).expect("mmap");
        let base = rdram.as_ptr();
        let barrier = Barrier::new(base, rdram.len(), 0, (page * 4) as u32);
        if !barrier.usable() {
            return;
        }
        barrier.arm();
        // Clean boundary: stays protected.
        assert!(matches!(barrier.take_dirty(), Dirty::Pages(ranges) if ranges.is_empty()));
        barrier.force_disarm();
        // No fault may be taken here; the handler is disarmed, so a still-
        // protected page would deliver SIGSEGV to the default handler.
        rdram[page * 2] = 0x77;
        assert_eq!(rdram[page * 2], 0x77);
        assert!(
            matches!(barrier.take_dirty(), Dirty::AlreadyDisarmed),
            "force_disarm must leave the barrier disarmed"
        );
    }
}
