//! The lockstep differential harness: parse a real reference savestate,
//! transplant it into fn64 (same real `fn64_abi`/`fn64_runtime` machinery
//! `tests/transplant.rs` proves works), run forward, and at each
//! checkpoint PC fn64 reports reaching, ask the faki-tools oracle
//! (subprocess, `oracle_client`) what the TRUE register state was at that
//! same PC from the SAME starting snapshot. Reports the first checkpoint
//! where they disagree, per `lockstep::LockstepReport`.
//!
//! ## Honesty about what's real here (same discipline as `tests/
//! transplant.rs`)
//!
//! This session's scope is `fn64-diff` only -- no real NW4E
//! `RecompiledFuncs` corpus is built/linked here (that's a separate,
//! larger milestone; a real corpus DOES exist out-of-tree at
//! `aki-recomp/games/NW4E/RecompiledFuncs` for a future harness to build
//! against, see `examples/wm2000-boot`'s build.rs for the shape). The
//! function BODY this harness runs at the transplanted entry point is the
//! same honestly-labeled stand-in `tests/transplant.rs` uses: it seeds
//! `ctx.r29`/`ctx.r31` from the snapshot and returns immediately (a
//! same-instant "checkpoint 0" observation), rather than genuine recompiled
//! NW4E logic walking forward through real basic blocks. Given that, this
//! run's REAL, honest finding is: fn64 has (as of this session) no ported
//! NW4E instruction semantics past the transplanted entry point at all, so
//! the harness's own first checkpoint already diverges from the oracle's
//! forward-stepped ground truth (the oracle keeps executing real MIPS
//! instructions past the resume PC; fn64's stand-in does not). That IS a
//! true, correctly-localized "first divergence" for the current state of
//! this repo -- reported honestly rather than manufacturing a fake match.
//! The moment a real NW4E section registry lands, this same harness
//! (unchanged) starts reporting genuinely useful divergences deeper into
//! actual game logic.
//!
//! Usage:
//! ```text
//! cargo run -p fn64-diff --release --bin lockstep -- \
//!     --oracle /path/to/oracle/binary \
//!     --state /path/to/reference.stN \
//!     [--rom /path/to/rom.z64] \
//!     [--extra-steps N]
//! ```
use std::cell::RefCell;
use std::path::PathBuf;

use fn64_abi::RecompContext;
use fn64_diff::lockstep::{compare_checkpoint, CheckpointResult, Fn64Checkpoint, LockstepReport};
use fn64_diff::oracle_client::OracleClient;

thread_local! {
    static SEED_GPRS: RefCell<[u64; 32]> = RefCell::new([0u64; 32]);
    static REACHED_PCS: RefCell<Vec<(String, u32, [u64; 32])>> = RefCell::new(Vec::new());
}

/// Stand-in transplanted entry function -- see module doc's honesty note.
/// Records ITS OWN pc (the transplanted resume PC, closed over via a
/// thread_local since a raw `extern "C" fn` can't carry captured state) and
/// the seeded GPRs as checkpoint 0, then returns.
unsafe extern "C" fn stand_in_target(_rdram: *mut u8, ctx: *mut RecompContext) {
    let seeded = SEED_GPRS.with(|s| *s.borrow());
    let ctx_ref = &mut *ctx;
    ctx_ref.r29 = seeded[29];
    ctx_ref.r31 = seeded[31];

    let pc = CHECKPOINT_PC.with(|p| *p.borrow());
    let mut recorded = [0u64; 32];
    recorded[29] = ctx_ref.r29;
    recorded[31] = ctx_ref.r31;
    REACHED_PCS.with(|r| r.borrow_mut().push(("transplant-entry".to_string(), pc, recorded)));
}

thread_local! {
    static CHECKPOINT_PC: RefCell<u32> = RefCell::new(0);
}

struct Args {
    oracle_bin: PathBuf,
    state_path: PathBuf,
    rom_path: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut oracle_bin = None;
    let mut state_path = None;
    let mut rom_path = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--oracle" => oracle_bin = args.next().map(PathBuf::from),
            "--state" => state_path = args.next().map(PathBuf::from),
            "--rom" => rom_path = args.next().map(PathBuf::from),
            other => return Err(format!("unrecognized argument: {other}")),
        }
    }

    Ok(Args {
        oracle_bin: oracle_bin.ok_or("missing required --oracle <path to built oracle binary>")?,
        state_path: state_path.ok_or("missing required --state <path to .stN savestate>")?,
        rom_path,
    })
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!(
                "usage: lockstep --oracle <path> --state <path.stN> [--rom <path>]"
            );
            std::process::exit(2);
        }
    };

    if !args.oracle_bin.exists() {
        eprintln!(
            "error: oracle binary not found at {}",
            args.oracle_bin.display()
        );
        std::process::exit(2);
    }
    if !args.state_path.exists() {
        eprintln!("error: savestate not found at {}", args.state_path.display());
        std::process::exit(2);
    }

    let bytes = std::fs::read(&args.state_path).expect("read savestate");
    let snapshot = fn64_diff::savestate::parse(&bytes).expect("parse reference savestate");
    let resume_pc = snapshot.resume_pc();

    println!(
        "LOCKSTEP: starting from snapshot {} (raw pc=0x{:08x}, resolved resume_pc=0x{resume_pc:08x})",
        args.state_path.display(),
        snapshot.pc
    );

    // --- Step 1: transplant into fn64 (real fn64_abi/fn64_runtime machinery) ---
    let rdram = fn64_diff::to_fn64_rdram(&snapshot);
    let mut rdram_bytes = rdram;

    let entry_vram = resume_pc & 0xFFFF_F000;
    let section_size = 0x0001_0000u32;
    match fn64_diff::resolve_entry_point(resume_pc, &[(entry_vram, section_size)]) {
        fn64_diff::ResolvedEntry::EnclosingFunction { .. } => {}
        other => {
            eprintln!("warning: resolve_entry_point returned unexpected {other:?}, proceeding anyway");
        }
    }

    let section_idx = unsafe {
        fn64_abi::register_section(0, entry_vram, section_size, &[(0u32, 0u32, stand_in_target)])
    };
    fn64_abi::set_section_loaded(section_idx);

    SEED_GPRS.with(|s| *s.borrow_mut() = snapshot.gprs);
    CHECKPOINT_PC.with(|p| *p.borrow_mut() = resume_pc);

    let rdram_ptr = rdram_bytes.as_mut_ptr();
    unsafe {
        fn64_abi::boot_thread0(rdram_ptr, stand_in_target, 99, 10);
    }
    fn64_abi::run_to_idle();

    let reached = REACHED_PCS.with(|r| r.borrow().clone());
    println!("LOCKSTEP: fn64 executed {} checkpoint(s)", reached.len());

    // --- Step 2: for each fn64 checkpoint, ask the oracle for ground truth ---
    let mut client = OracleClient::new(&args.oracle_bin, &args.state_path);
    if let Some(rom) = &args.rom_path {
        client = client.with_rom(rom);
    }

    let mut report = LockstepReport::new();
    for (label, pc, gprs) in &reached {
        print!("LOCKSTEP: querying oracle for checkpoint '{label}' @ pc=0x{pc:08x} ... ");
        use std::io::Write;
        std::io::stdout().flush().ok();

        let checkpoint = Fn64Checkpoint::new(label.clone(), *pc).with_gprs_from(gprs);
        match client.registers_at(*pc) {
            Ok(reference) => {
                let result = compare_checkpoint(&checkpoint, &reference);
                println!(
                    "{}",
                    if result.is_match() { "MATCH" } else { "DIVERGED" }
                );
                report.push(checkpoint, result);
            }
            Err(e) => {
                println!("ORACLE ERROR: {e}");
                report.push(checkpoint, CheckpointResult::PcNotReached { pc: *pc });
            }
        }
    }

    println!();
    println!("{}", report.summarize());

    if report.first_divergence().is_some() {
        std::process::exit(1);
    }
}
