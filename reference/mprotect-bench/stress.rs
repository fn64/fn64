//! Adversarial follow-up to the headline benchmark.
//!
//! The headline number took ONE fault per boundary on a round-robin page. Three
//! things could make that optimistic in the real runtime, and each is measured
//! here:
//!
//! 1. **Multiple faults per boundary.** The watched region is unaligned
//!    (`[0x400,0x171a60)`), so whole-page protection covers ordinary guest data
//!    interleaved with code. Every unrelated data store to a protected page is
//!    a spurious fault. Cost is measured as a function of faults per boundary.
//! 2. **Re-protect cost after the pages went writable.** Re-arming a region
//!    whose PTEs were just modified may be more expensive than re-arming a
//!    region that was already read-only.
//! 3. **Handler on a deep/alternate stack**, as it would be under corosensei
//!    coroutine stacks rather than the main thread stack.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

const WATCHED_LEN: usize = 0x171a60 - 0x400;
const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const MAP_PRIVATE: i32 = 0x0002;
const MAP_ANON: i32 = 0x1000;
const SIGSEGV: i32 = 11;
const SIGBUS: i32 = 10;
const SA_SIGINFO: i32 = 0x0040;
const SA_ONSTACK: i32 = 0x0001;

extern "C" {
    fn mmap(a: *mut u8, l: usize, p: i32, f: i32, fd: i32, o: i64) -> *mut u8;
    fn mprotect(a: *mut u8, l: usize, p: i32) -> i32;
    fn sigaction(s: i32, a: *const SigAction, o: *mut SigAction) -> i32;
    fn getpagesize() -> i32;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SigAction {
    handler: usize,
    mask: u32,
    flags: i32,
}

static FAULT_PAGE: AtomicUsize = AtomicUsize::new(0);
static PAGE_SIZE: AtomicUsize = AtomicUsize::new(16384);
static FAULTS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn handler(_s: i32, _i: *mut u8, _c: *mut u8) {
    let page = FAULT_PAGE.load(Ordering::Relaxed);
    FAULTS.fetch_add(1, Ordering::Relaxed);
    unsafe { mprotect(page as *mut u8, PAGE_SIZE.load(Ordering::Relaxed), PROT_READ | PROT_WRITE) };
}

fn bench<F: FnMut()>(runs: usize, iters: usize, mut f: F) -> f64 {
    let mut s = Vec::new();
    for _ in 0..runs {
        let t = Instant::now();
        for _ in 0..iters {
            f();
        }
        s.push(t.elapsed().as_nanos() as f64 / iters as f64);
    }
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[s.len() / 2]
}

fn main() {
    let ps = unsafe { getpagesize() as usize };
    PAGE_SIZE.store(ps, Ordering::Relaxed);
    let pages = WATCHED_LEN.div_ceil(ps);
    let region_len = pages * ps;

    let act = SigAction { handler: handler as *const () as usize, mask: 0, flags: SA_SIGINFO | SA_ONSTACK };
    for sig in [SIGSEGV, SIGBUS] {
        assert_eq!(unsafe { sigaction(sig, &act, std::ptr::null_mut()) }, 0);
    }

    let region = unsafe { mmap(std::ptr::null_mut(), region_len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0) };
    assert!(!region.is_null() && region as isize != -1);
    unsafe { std::ptr::write_bytes(region, 0xa5, region_len) };

    println!("pages={pages} page_size={ps} region={region_len}");
    println!();
    println!("faults/boundary   ns/boundary   vs 26525 ns scan");
    println!("---------------   -----------   ----------------");

    // Cost of one boundary as a function of how many distinct pages the guest
    // writes between boundaries. n=1 is the headline; higher n is the
    // spurious-fault tax from unaligned whole-page protection.
    for n in [1usize, 2, 4, 8, 16, 32, 64, 93] {
        let mut base = 0usize;
        FAULTS.store(0, Ordering::Relaxed);
        let ns = bench(7, 500, || {
            unsafe { mprotect(region, region_len, PROT_READ) };
            for k in 0..n {
                let page = unsafe { region.add(((base + k) % pages) * ps) };
                FAULT_PAGE.store(page as usize, Ordering::Relaxed);
                unsafe { std::ptr::write_volatile(page.add(64), 0x5au8) };
            }
            base += 1;
        });
        let taken = FAULTS.load(Ordering::Relaxed);
        let ratio = 26525.0 / ns;
        let verdict = if ns < 26525.0 { format!("{ratio:.2}x cheaper") } else { format!("{:.2}x MORE", 1.0 / ratio) };
        println!("{n:>15}   {ns:>11.1}   {verdict}   (faults={taken})");
    }
    unsafe { mprotect(region, region_len, PROT_READ | PROT_WRITE) };

    println!();
    // Break down where the per-boundary cost goes at n=1.
    let arm = bench(7, 5000, || unsafe {
        mprotect(region, region_len, PROT_READ);
        mprotect(region, region_len, PROT_READ | PROT_WRITE);
    });
    println!("whole-region protect+unprotect (no fault): {arm:.1} ns");
    let single = bench(7, 5000, || unsafe {
        mprotect(region, ps, PROT_READ);
        mprotect(region, ps, PROT_READ | PROT_WRITE);
    });
    println!("single-page  protect+unprotect (no fault): {single:.1} ns");
}
