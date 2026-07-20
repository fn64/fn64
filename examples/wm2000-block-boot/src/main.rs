//! Boot WM2000 (NWXE) from fn64's OWN discovered Block Pack -- no
//! aki-recomp metadata, no N64Recomp C. `build.rs` ran discovery on the
//! user's ROM and emitted the sparse arbitrary-PC runner + pack consts;
//! this harness installs them as a live `BlockProgram`, copies the boot
//! bank's ROM bytes into RDRAM, and drives the executor until the guest
//! either idles or reaches the first destination the pack does not admit
//! (a typed fault, which is the honest current frontier of the discovery
//! lane -- see docs/ROADMAP.md D4b).

use fn64_recomp_rs::{
    BankId, CodeBank, CodeSpan, CpuFault, CpuFaultKind, ExecutableRegion, ExecutionKey,
    GeneratedBankRunner, GuestPc, InstructionBudget,
};

#[allow(clippy::all, unused)]
mod gen {
    use fn64_recomp_rs::{
        BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CpuException, CpuFault, CpuFaultKind,
        ExecutionKey, GeneratedBankRunner, GuestPc, InstructionBudget, ProgramError, Rdram,
        RecompContext,
    };
    include!(concat!(env!("OUT_DIR"), "/runner.rs"));
}
mod pack {
    include!(concat!(env!("OUT_DIR"), "/pack.rs"));
}

fn bank() -> BankId {
    BankId::new(pack::BANK_ID)
}

fn lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
    let key = ExecutionKey::new(bank(), pc);
    let mapped = pack::SPANS.iter().any(|(va, words)| {
        pc.get() >= *va && pc.get() < *va + (words.len() as u32) * 4
    });
    if mapped {
        Ok(key)
    } else {
        let first = pack::SPANS.first().expect("pack has spans");
        let last = pack::SPANS.last().expect("pack has spans");
        Err(CpuFault {
            at: key,
            kind: CpuFaultKind::UnmappedPc {
                bank_start: first.0,
                bank_end: last.0 + (last.1.len() as u32) * 4,
            },
        })
    }
}

fn entry_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
    lookup(pc)
}

fn transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
    lookup(pc)
}

fn main() {
    let words: usize = pack::SPANS.iter().map(|(_, w)| w.len()).sum();
    println!(
        "[wm2000-block-boot] discovered pack: {} blocks / {words} words, bank {:#018X}, entry {:#010X}",
        pack::SPANS.len(),
        pack::BANK_ID,
        pack::ENTRYPOINT
    );

    let rom_path = std::env::var("ROM").expect("ROM env var (same contract as build.rs)");
    let rom = std::fs::read(&rom_path).expect("reading ROM");

    let mut rdram = fn64_boot_harness::new_rdram(fn64_boot_harness::TvType::Ntsc);
    let (rom_start, rom_end, va_start) = pack::ROM_COPY;
    let dest = (va_start - 0x8000_0000) as usize;
    rdram[dest..dest + (rom_end - rom_start)].copy_from_slice(&rom[rom_start..rom_end]);
    println!(
        "[wm2000-block-boot] copied boot bank rom=[{rom_start:#x},{rom_end:#x}) to va {va_start:#010X}"
    );
    let rdram_ptr = rdram.as_mut_ptr();
    unsafe { fn64_abi::sync_mmio_into_rdram(rdram_ptr) };

    let spans = pack::SPANS
        .iter()
        .map(|(va, words)| CodeSpan::new(bank(), GuestPc::new(*va), words.to_vec()))
        .collect::<Result<Vec<_>, _>>()
        .expect("pack spans are aligned and nonempty");
    let code_bank = CodeBank::from_spans(bank(), spans).expect("admitting discovered code bank");
    let first_va = pack::SPANS.first().expect("pack has spans").0;
    let last = pack::SPANS.last().expect("pack has spans");
    let mut program = fn64_recomp_rs::BlockProgram::new();
    let mut region = ExecutableRegion::new(
        GuestPc::new(first_va),
        GuestPc::new(last.0 + (last.1.len() as u32) * 4),
    );
    region
        .install(
            &mut program,
            code_bank,
            GeneratedBankRunner::new(bank(), gen::run_nwxe_boot),
        )
        .expect("installing discovered bank runner");

    println!("[wm2000-block-boot] booting thread 0 from the discovered pack...");
    unsafe {
        fn64_abi::recompiled::boot_thread0_block_program(
            rdram_ptr,
            rdram.len(),
            program,
            ExecutionKey::new(bank(), GuestPc::new(pack::ENTRYPOINT)),
            entry_lookup,
            transfer_lookup,
            InstructionBudget::new(4096).expect("nonzero budget"),
            0,
            10,
        );
    }

    // Same bounded drive shape as ../wm2000-boot: step while runnable,
    // advance virtual time when idle, stop on a steady idle state. The
    // expected outcome today is a typed fault (panic) naming the first
    // un-admitted destination -- that PC is the deliverable.
    const MAX_STEPS: u64 = 1_000_000;
    const IDLE_TICKS_BEFORE_STOP: u32 = 200;
    let mut consecutive_idle_ticks = 0u32;
    let mut tick = 0u64;
    let mut steps = 0u64;
    while steps < MAX_STEPS {
        let stepped = fn64_abi::run_one_step();
        steps += 1;
        if !stepped {
            tick += fn64_abi::vi_field_interval().expect("typed TV standard keeps VI armed");
            fn64_abi::advance_virtual_time(tick);
            consecutive_idle_ticks += 1;
            if consecutive_idle_ticks >= IDLE_TICKS_BEFORE_STOP {
                println!(
                    "[wm2000-block-boot] steady idle at sim_time={} steps={steps}",
                    fn64_abi::sim_time()
                );
                break;
            }
        } else {
            consecutive_idle_ticks = 0;
        }
    }
    println!(
        "[wm2000-block-boot] done: steps={steps} sim_time={} thread0_dead={}",
        fn64_abi::sim_time(),
        fn64_abi::is_thread_dead(0)
    );
}
