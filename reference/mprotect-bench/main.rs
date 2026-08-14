//! Microbenchmark deciding whether an `mprotect` write barrier can beat the
//! fn64 watched-region `memcmp` scan on this machine.
//!
//! Two costs are measured, both per dispatch boundary:
//!
//! 1. `mprotect` design: one write fault on a `PROT_READ` page (delivered as a
//!    signal, handled, page re-armed with `mprotect(PROT_READ|PROT_WRITE)`),
//!    then re-protected for the next boundary. That is the honest per-boundary
//!    cost: fault + re-arm + re-protect.
//! 2. Current design: `memcmp` over the 1,513,056-byte watched region, in the
//!    all-equal case (worst, full read) and the first-bytes-differ case
//!    (typical, early exit).
//!
//! Nothing here links fn64; it only needs the sizes.

use std::arch::asm;
use std::io::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// The fn64 watched executable region: `[0x400, 0x171a60)`.
const WATCHED_LEN: usize = 0x171a60 - 0x400; // 1_513_056

const PROT_NONE: i32 = 0x0;
const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const MAP_PRIVATE: i32 = 0x0002;
const MAP_ANON: i32 = 0x1000;
const SIGSEGV: i32 = 11;
const SIGBUS: i32 = 10;
const SA_SIGINFO: i32 = 0x0040;
const SA_ONSTACK: i32 = 0x0001;

extern "C" {
    fn mmap(
        addr: *mut u8,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut u8;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    fn getpagesize() -> i32;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SigAction {
    handler: usize,
    mask: u32,
    flags: i32,
}

/// Page currently write-protected; the handler re-arms exactly this one.
static FAULT_PAGE: AtomicUsize = AtomicUsize::new(0);
static FAULT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Async-signal-safe: two atomic ops and one `mprotect`. No allocation, no
/// locks, no libc beyond the syscall wrapper.
extern "C" fn handler(_sig: i32, _info: *mut u8, _ctx: *mut u8) {
    let page = FAULT_PAGE.load(Ordering::Relaxed);
    FAULT_COUNT.fetch_add(1, Ordering::Relaxed);
    unsafe {
        mprotect(page as *mut u8, page_size(), PROT_READ | PROT_WRITE);
    }
}

fn page_size() -> usize {
    unsafe { getpagesize() as usize }
}

fn install_handlers() {
    let act = SigAction {
        handler: handler as usize,
        mask: 0,
        flags: SA_SIGINFO | SA_ONSTACK,
    };
    for sig in [SIGSEGV, SIGBUS] {
        let rc = unsafe { sigaction(sig, &act, std::ptr::null_mut()) };
        assert_eq!(rc, 0, "sigaction({sig}) failed");
    }
}

fn map(len: usize) -> *mut u8 {
    let p = unsafe {
        mmap(
            std::ptr::null_mut(),
            len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANON,
            -1,
            0,
        )
    };
    assert!(!p.is_null() && p as isize != -1, "mmap failed");
    p
}

#[inline(never)]
fn black_box<T>(v: T) -> T {
    unsafe {
        let mut v = v;
        asm!("/* {0} */", in(reg) &mut v as *mut T, options(nostack, preserves_flags));
        v
    }
}

/// Median of `runs` measurements of `f`, each averaging `iters` iterations.
fn bench<F: FnMut()>(runs: usize, iters: usize, mut f: F) -> f64 {
    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let t = Instant::now();
        for _ in 0..iters {
            f();
        }
        samples.push(t.elapsed().as_nanos() as f64 / iters as f64);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

fn main() {
    let ps = page_size();
    let pages = WATCHED_LEN.div_ceil(ps);
    println!("page size            : {ps} bytes");
    println!("watched region       : {WATCHED_LEN} bytes ({pages} pages)");
    println!();

    install_handlers();

    // ---- 1. mprotect fault + re-arm ------------------------------------
    //
    // A whole watched region, mmap'd and touched so every page is resident.
    // Each iteration: protect one page read-only, store into it (faults, the
    // handler re-arms), which is exactly the per-boundary sequence of the
    // mprotect design -- one protect, one fault, one re-arm.
    let region_len = pages * ps;
    let region = map(region_len);
    unsafe { std::ptr::write_bytes(region, 0xa5, region_len) };

    let mut which = 0usize;
    // Round-robin over pages so the measurement is not a single-page TLB
    // best case.
    let fault_ns = bench(9, 2000, || {
        let page = unsafe { region.add((which % pages) * ps) };
        which += 1;
        FAULT_PAGE.store(page as usize, Ordering::Relaxed);
        unsafe {
            // Arm: drop write permission for this boundary.
            mprotect(page, ps, PROT_READ);
            // Guest store into a watched page -> fault -> handler re-arms.
            std::ptr::write_volatile(page.add(64), 0x5au8);
        }
    });
    let faults = FAULT_COUNT.load(Ordering::Relaxed);
    println!("--- mprotect design (per boundary) ---");
    println!("protect + fault + re-arm : {fault_ns:>10.1} ns   (faults taken: {faults})");

    // The protect half alone, so the fault half is separable.
    let protect_ns = bench(9, 20000, || {
        let page = unsafe { region.add((which % pages) * ps) };
        which += 1;
        unsafe {
            mprotect(page, ps, PROT_READ);
            mprotect(page, ps, PROT_READ | PROT_WRITE);
        }
    });
    println!("two bare mprotect calls  : {protect_ns:>10.1} ns   (no fault)");

    // Protecting the WHOLE watched region at once, which is what a design
    // re-arming every page after each boundary would actually pay.
    let whole_ns = bench(9, 2000, || unsafe {
        mprotect(region, region_len, PROT_READ);
        mprotect(region, region_len, PROT_READ | PROT_WRITE);
    });
    println!("mprotect whole region x2 : {whole_ns:>10.1} ns   ({pages} pages)");

    // The design's real per-boundary shape: the whole region is protected once
    // at the boundary, the guest takes ONE fault on the page it writes (the
    // handler re-arms that page only), and the next boundary re-protects the
    // whole region. Measured end to end.
    let realistic_ns = bench(9, 2000, || {
        let page = unsafe { region.add((which % pages) * ps) };
        which += 1;
        FAULT_PAGE.store(page as usize, Ordering::Relaxed);
        unsafe {
            mprotect(region, region_len, PROT_READ);
            std::ptr::write_volatile(page.add(64), 0x5au8);
        }
    });
    println!("whole-region arm + 1 fault:{realistic_ns:>10.1} ns   <- the design");
    unsafe { mprotect(region, region_len, PROT_READ | PROT_WRITE) };
    println!();

    // ---- 2. the memcmp scan --------------------------------------------
    let a = vec![0xa5u8; WATCHED_LEN];
    let mut b = vec![0xa5u8; WATCHED_LEN];

    let equal_ns = bench(9, 200, || {
        black_box(black_box(&a[..]) == black_box(&b[..]));
    });
    println!("--- current design (per scan) ---");
    println!("memcmp all-equal (worst) : {equal_ns:>10.1} ns   ({:.1} GB/s)",
        WATCHED_LEN as f64 / equal_ns);

    // Typical: the change is early in the region, memcmp bails immediately.
    b[16] = 0x00;
    let early_ns = bench(9, 20000, || {
        black_box(black_box(&a[..]) == black_box(&b[..]));
    });
    println!("memcmp differs @16 B     : {early_ns:>10.1} ns");
    b[16] = 0xa5;

    // A difference in the middle and at the end, to bound where the real
    // early exit lands: the changed page is wherever the guest stored.
    for (label, at) in [("1/4 in", WATCHED_LEN / 4), ("mid", WATCHED_LEN / 2), ("3/4 in", WATCHED_LEN * 3 / 4)] {
        b[at] = 0x00;
        let ns = bench(9, 500, || {
            black_box(black_box(&a[..]) == black_box(&b[..]));
        });
        println!("memcmp differs {label:<9} : {ns:>10.1} ns");
        b[at] = 0xa5;
    }

    // What `changed_ranges_into` ACTUALLY costs when a byte did change: the
    // whole-body `memcmp` runs first and only then does the 256-word chunk
    // walk re-scan to name the bytes. There is no early exit -- the leading
    // compare has already read the region.
    const CHUNK: usize = 256;
    for (label, at) in [("@16 B", 16usize), ("mid", WATCHED_LEN / 2)] {
        b[at] = 0x00;
        let ns = bench(9, 200, || {
            // leading whole-body compare
            if black_box(black_box(&a[..]) != black_box(&b[..])) {
                // chunked walk to name the differing words
                let words = WATCHED_LEN / 4;
                let mut word = 0;
                while word < words {
                    let chunk = CHUNK.min(words - word);
                    if a[word * 4..(word + chunk) * 4] == b[word * 4..(word + chunk) * 4] {
                        word += chunk;
                        continue;
                    }
                    for w in word..word + chunk {
                        let at = w * 4;
                        if a[at..at + 4] == b[at..at + 4] {
                            continue;
                        }
                        black_box(w);
                    }
                    word += chunk;
                }
            }
        });
        println!("full changed_ranges {label:<5}: {ns:>10.1} ns   (memcmp + chunk walk)");
        b[at] = 0xa5;
    }
    println!();

    // ---- verdict --------------------------------------------------------
    println!("--- comparison ---");
    println!("mprotect per boundary    : {fault_ns:>10.1} ns");
    println!("scan, all-equal          : {equal_ns:>10.1} ns");
    println!("scan, early exit         : {early_ns:>10.1} ns");
    let ratio_worst = fault_ns / equal_ns;
    let ratio_early = fault_ns / early_ns;
    println!("fault / all-equal scan   : {ratio_worst:>10.3}x");
    println!("fault / early-exit scan  : {ratio_early:>10.3}x");
    std::io::stdout().flush().unwrap();
}
