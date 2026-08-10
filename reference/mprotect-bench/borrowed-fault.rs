//! Does the write barrier still behave when a long-lived `&mut [u8]` spans the
//! faults, the way the real guest store path holds one?
//!
//! `coroutine-fault.rs` cleared signal delivery onto a `corosensei` stack, but
//! it issued its stores through `ptr::write_volatile` on a raw pointer. That is
//! NOT the shape fn64 has. The real coroutine body does
//!
//! ```ignore
//! let bytes = unsafe { std::slice::from_raw_parts_mut(rdram_ptr, rdram_len) };
//! let mut mem = Rdram::new(bytes);
//! run_block_program(&live, entry, &mut ctx, &mut mem);
//! ```
//!
//! (`crates/fn64-abi/src/recompiled/execution.rs:1680-1690`, and identically at
//! `:1158`, `:1576`, `runners.rs:1263`/`:1309`/`:1324`) -- one `&mut [u8]` over
//! the WHOLE allocation, created once per coroutine entry and live across every
//! store, every yield, every host shim, and therefore across every fault.
//!
//! So this benchmark reproduces the actual shape:
//!
//!   1. a whole-region `&mut [u8]`, live across the faults;
//!   2. stores issued as safe bounds-checked slice indexing (`mem[i] = v`),
//!      exactly what `Rdram::store_b` compiles to
//!      (`crates/fn64-recomp-rs/src/runtime/host.rs:938-948`);
//!   3. reads back through the SAME borrow after the fault, which is where a
//!      compiler that had cached the region's contents around the "impossible"
//!      write would show it;
//!   4. an aliasing raw-pointer writer running while that `&mut` is live, which
//!      is what `rsp_commit.rs`, `rsp_phase.rs`, `pi/timing.rs` and the queue
//!      mirror already do to the same bytes today;
//!   5. all of it inside a `corosensei` coroutine, across suspend/resume.
//!
//! No fn64 code is involved.
//!
//! Build with the sibling `Cargo.toml.txt` (rename to `Cargo.toml`, point the
//! `[[bin]]` path here) and run under `MIRIFLAGS` if you want the aliasing
//! model's opinion -- though note Miri cannot execute `mprotect`, so what it
//! can check is item 4 in isolation, not the fault itself.

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

static REGION: AtomicUsize = AtomicUsize::new(0);
static REGION_LEN: AtomicUsize = AtomicUsize::new(0);
static PAGE_SIZE: AtomicUsize = AtomicUsize::new(16384);
static FAULTS: AtomicUsize = AtomicUsize::new(0);

/// The same three operations the real handler performs: read atomics, record,
/// `mprotect`. Nothing that allocates or locks.
extern "C" fn handler(_signal: i32, info: *mut SigInfo, _context: *mut u8) {
    let addr = unsafe { (*info).addr } as usize;
    let start = REGION.load(Ordering::Relaxed);
    let len = REGION_LEN.load(Ordering::Relaxed);
    let page = PAGE_SIZE.load(Ordering::Relaxed);
    assert!(
        addr >= start && addr < start + len,
        "fault outside the protected region"
    );
    FAULTS.fetch_add(1, Ordering::Relaxed);
    let base = addr & !(page - 1);
    unsafe { mprotect(base as *mut u8, page, PROT_READ | PROT_WRITE) };
}

fn main() {
    let page = unsafe { getpagesize() as usize };
    PAGE_SIZE.store(page, Ordering::Relaxed);

    let act = SigAction {
        handler: handler as *const () as usize,
        mask: 0,
        flags: SA_SIGINFO | SA_ONSTACK,
    };
    for signal in [SIGSEGV, SIGBUS] {
        assert_eq!(unsafe { sigaction(signal, &act, std::ptr::null_mut()) }, 0);
    }

    let pages = 16usize;
    let len = pages * page;
    let base = unsafe {
        mmap(
            std::ptr::null_mut(),
            len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANON,
            -1,
            0,
        )
    };
    assert!(!base.is_null() && base as isize != -1);
    REGION.store(base as usize, Ordering::Relaxed);
    REGION_LEN.store(len, Ordering::Relaxed);

    let base_addr = base as usize;
    let mut coro = Coroutine::<(), u32, u32>::new(move |yielder, _| {
        // THE SHAPE UNDER TEST. One `&mut [u8]` over the whole region, created
        // once at coroutine entry, live for the entire body -- across every
        // store, every fault, and every suspend. This is the borrow
        // `execution.rs:1680` creates and `run_block_program` holds.
        let mem: &mut [u8] = unsafe { std::slice::from_raw_parts_mut(base_addr as *mut u8, len) };

        // An independent mirror of what the region SHOULD contain, updated
        // alongside every write. Ordinary heap memory, never protected. The
        // final whole-region comparison against this is the real assertion:
        // it proves the barrier lost nothing and invented nothing, rather than
        // proving a formula about what should be there.
        let mut shadow = vec![0u8; len];

        let mut rounds = 0u32;
        for round in 0..4000u32 {
            let page_index = (round as usize) % pages;
            let offset = page_index * page + (round as usize % 97) * 3;

            // Arm the whole region, as the real barrier does at a boundary.
            unsafe { mprotect(base_addr as *mut u8, len, PROT_READ) };

            // The faulting store, through the long-lived borrow, as safe
            // bounds-checked slice indexing. `Rdram::store_b` is `self.mem[p]
            // = val` and compiles to exactly this.
            mem[offset] = round as u8;
            shadow[offset] = round as u8;

            // Read back THROUGH THE SAME BORROW. If the compiler had cached
            // the region's contents across a store it believed could not
            // trap, or if the fault had lost the store, this is where it
            // shows. `read_volatile` would hide the very thing being tested,
            // so this is a plain indexed read.
            assert_eq!(
                mem[offset], round as u8,
                "store lost across fault at round {round} offset {offset:#x}"
            );

            // A neighbouring page, still protected, must be untouched -- the
            // handler unprotects one page, not the region. Compared against a
            // shadow copy rather than a formula, so the check cannot be
            // satisfied by a wrong model of what should be there.
            let other = ((page_index + 1) % pages) * page;
            assert_eq!(mem[other], shadow[other], "neighbour disturbed");

            // An ALIASING raw-pointer write to the same bytes while the `&mut`
            // is live, which is what the RSP/renderer/DMA/queue-mirror paths do
            // today (`rsp_commit.rs:87`, `rsp_phase.rs:773`, `pi/timing.rs:444`,
            // `executor/mod.rs:740`). It must fault and land just the same.
            let alias = (base_addr + offset + 1) as *mut u8;
            unsafe { std::ptr::write_volatile(alias, 0xa5) };
            shadow[offset + 1] = 0xa5;
            assert_eq!(mem[offset + 1], 0xa5, "aliased write lost at round {round}");

            // Disarm, as the boundary does.
            unsafe { mprotect(base_addr as *mut u8, len, PROT_READ | PROT_WRITE) };

            // Host-side write with the barrier down, through the same borrow.
            mem[offset] = round as u8;
            shadow[offset] = round as u8;

            rounds += 1;
            if round % 100 == 0 {
                // Suspend with the borrow live and the region unprotected.
                yielder.suspend(round);
            }
            if round % 137 == 0 {
                // Suspend with the region PROTECTED and the borrow live, so a
                // stack switch happens with the barrier armed.
                unsafe { mprotect(base_addr as *mut u8, len, PROT_READ) };
                yielder.suspend(round);
                unsafe { mprotect(base_addr as *mut u8, len, PROT_READ | PROT_WRITE) };
            }
        }

        // THE REAL ASSERTION. After 4000 protect/fault/unprotect cycles with
        // the borrow live throughout, every one of the region's bytes must
        // equal the independently maintained mirror: nothing lost, nothing
        // invented, nothing stale.
        assert_eq!(mem.len(), shadow.len());
        for index in 0..len {
            assert_eq!(
                mem[index], shadow[index],
                "region diverged from the mirror at {index:#x} after {rounds} rounds"
            );
        }
        let nonzero = shadow.iter().filter(|&&b| b != 0).count();
        assert!(nonzero > 1000, "mirror check was vacuous: only {nonzero} nonzero bytes");
        rounds
    });

    let mut resumes = 0;
    loop {
        match coro.resume(()) {
            corosensei::CoroutineResult::Yield(_) => resumes += 1,
            corosensei::CoroutineResult::Return(rounds) => {
                println!("coroutine completed: {rounds} rounds, {resumes} suspend/resume cycles");
                break;
            }
        }
    }
    println!("faults delivered: {}", FAULTS.load(Ordering::Relaxed));
    println!(
        "RESULT: a whole-region &mut [u8] held across write faults on a corosensei stack \
         delivers every store, including aliased raw-pointer writes"
    );
}
