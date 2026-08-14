//! Does a write fault on an `mprotect(PROT_READ)` page deliver correctly when
//! the faulting store happens on a `corosensei` coroutine stack?
//!
//! This is the obstacle that can invalidate the mprotect write-barrier design
//! outright: the fn64 guest store path runs on coroutine stacks, and a signal
//! handler that cannot be delivered there (or that corrupts the switch) is a
//! hard crash with no diagnostic. No fn64 code is involved.

use corosensei::Coroutine;
use std::sync::atomic::{AtomicUsize, Ordering};

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

fn main() {
    let ps = unsafe { getpagesize() as usize };
    PAGE_SIZE.store(ps, Ordering::Relaxed);

    // SA_ONSTACK is deliberate: it is what a real integration would use, and it
    // is also the flag most likely to interact badly with a switched stack.
    let act = SigAction { handler: handler as *const () as usize, mask: 0, flags: SA_SIGINFO | SA_ONSTACK };
    for sig in [SIGSEGV, SIGBUS] {
        assert_eq!(unsafe { sigaction(sig, &act, std::ptr::null_mut()) }, 0);
    }

    let pages = 8;
    let len = pages * ps;
    let region = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0) };
    assert!(!region.is_null() && region as isize != -1);
    unsafe { std::ptr::write_bytes(region, 0xa5, len) };

    // Fault from inside a coroutine, repeatedly, yielding across faults so the
    // stack switches between the fault and the next one.
    let mut coro = Coroutine::<(), u32, u32>::new(move |yielder, _| {
        let mut done = 0u32;
        for round in 0..2000u32 {
            let page = unsafe { region.add((round as usize % pages) * ps) };
            FAULT_PAGE.store(page as usize, Ordering::Relaxed);
            unsafe {
                mprotect(page, ps, PROT_READ);
                // The faulting store, on the coroutine stack.
                std::ptr::write_volatile(page.add(128), round as u8);
            }
            // Verify the store actually landed after the handler re-armed.
            let seen = unsafe { std::ptr::read_volatile(page.add(128)) };
            assert_eq!(seen, round as u8, "store lost across fault at round {round}");
            done += 1;
            if round % 100 == 0 {
                // Switch stacks between faults.
                yielder.suspend(round);
            }
        }
        done
    });

    let mut resumes = 0;
    loop {
        match coro.resume(()) {
            corosensei::CoroutineResult::Yield(_) => resumes += 1,
            corosensei::CoroutineResult::Return(done) => {
                println!("coroutine completed: {done} faulting stores, {resumes} suspend/resume cycles");
                break;
            }
        }
    }
    println!("faults delivered: {}", FAULTS.load(Ordering::Relaxed));
    println!("RESULT: write faults deliver correctly on corosensei coroutine stacks");
}
