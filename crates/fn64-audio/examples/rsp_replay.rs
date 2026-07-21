//! Offline replayer for a `FN64_RSP_LLE_DEBUG_DIR` forensic capture (see
//! `fn64-abi`'s `dispatch_lle_task`). Re-executes the captured RSP task
//! instruction by instruction and reports the first divergence events a
//! runaway task exhibits:
//!
//!  - the first write that changes the OSTask header words at DMEM
//!    0xfc0..0x1000 (the wm2000 audio ucode legitimately stores state over
//!    the header tail, but a re-read of a clobbered header is fatal), and
//!  - the first control transfer into IMEM 0x000..0x080 (outside the audio
//!    ucode text, i.e. a corrupted jump-table dispatch).
//!
//! Usage: FN64_RSP_LLE_DEBUG_DIR=<dir> cargo run -p fn64-audio --release \
//!        --example rsp_replay [max_steps]

use fn64_audio::rsp::runtime::RspMachine;
use fn64_audio::rsp::{run_imem, RspExitReason, DMEM_SIZE};
use std::collections::VecDeque;

fn read_bank(dir: &std::path::Path, name: &str) -> [u8; DMEM_SIZE] {
    let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
    bytes.as_slice().try_into().expect("4096-byte bank image")
}

fn main() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("FN64_RSP_LLE_DEBUG_DIR").expect("FN64_RSP_LLE_DEBUG_DIR must be set"),
    );
    let max_steps: u64 = std::env::args()
        .nth(1)
        .map(|s| s.parse().expect("max_steps"))
        .unwrap_or(1 << 26);

    let initial_dmem = read_bank(&dir, "initial_dmem.bin");
    let mut imem = read_bank(&dir, "initial_imem.bin");
    let mut rdram = std::fs::read(dir.join("rdram_raw.bin")).expect("rdram_raw.bin");
    let state = std::fs::read_to_string(dir.join("state.txt")).expect("state.txt");
    let initial_pc = state
        .lines()
        .find_map(|line| line.strip_prefix("initial_pc "))
        .map(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).expect("initial_pc hex"))
        .expect("initial_pc line");

    let mut machine = RspMachine::new(&mut rdram);
    machine.load_dmem_logical(&initial_dmem);

    let header_watch: Vec<u8> = (0xfc0..0x1000).map(|a| machine.load_bu(a) as u8).collect();
    let mut header_reported = false;
    let mut low_imem_reported = false;

    let mut words: Vec<u32> = imem
        .chunks_exact(4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
        .collect();

    let mut ring: VecDeque<u32> = VecDeque::with_capacity(512);
    let mut pc = initial_pc;
    let mut steps = 0u64;
    let mut last_header = header_watch.clone();

    while steps < max_steps {
        let result = run_imem(&words, pc, &mut machine, 1);
        steps += result.steps;
        if ring.len() == 512 {
            ring.pop_front();
        }
        ring.push_back(result.pc);
        pc = result.pc;

        if !header_reported {
            let now: Vec<u8> = (0xfc0..0x1000).map(|a| machine.load_bu(a) as u8).collect();
            if now != last_header {
                let changed: Vec<String> = (0..0x40)
                    .filter(|&i| now[i] != last_header[i])
                    .map(|i| format!("dmem[{:#05x}] {:02x}->{:02x}", 0xfc0 + i, last_header[i], now[i]))
                    .collect();
                println!("HEADER WRITE at step {steps}, pc now {:#06x}: {}", pc, changed.join(", "));
                dump_context(&ring, &machine);
                last_header = now;
                // Keep going; report only the first few header events.
                if changed.iter().any(|c| c.contains("0xff0") || c.contains("0xff4")) {
                    println!("(data_ptr/data_size words touched)");
                    header_reported = true;
                }
            }
        }

        let masked = 0x1000 | (pc & 0xfff);
        if !low_imem_reported && (0x1000..0x1080).contains(&masked) {
            println!("LOW-IMEM ENTRY at step {steps}: pc {:#06x}", masked);
            dump_context(&ring, &machine);
            low_imem_reported = true;
        }
        if header_reported && low_imem_reported {
            println!("both events captured; stopping at step {steps}");
            return;
        }

        match result.reason {
            RspExitReason::StepLimit => {}
            RspExitReason::SwapOverlay => {
                machine.complete_imem_dma(&mut imem);
                words = imem
                    .chunks_exact(4)
                    .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
                    .collect();
                println!("(imem overlay swap at step {steps})");
            }
            RspExitReason::Broke => {
                println!("BREAK at step {steps}, pc {:#06x} -- task completed", pc);
                return;
            }
            other => {
                println!("exit {other:?} at step {steps}, pc {:#06x}", pc);
                return;
            }
        }
    }
    println!("step budget {max_steps} exhausted at pc {:#06x}", pc);
}

fn dump_context(ring: &VecDeque<u32>, machine: &RspMachine<'_>) {
    let pcs: Vec<String> = ring.iter().rev().take(48).rev().map(|p| format!("{p:#06x}")).collect();
    println!("  recent pcs: {}", pcs.join(" "));
    for reg in (0u8..32).step_by(4) {
        println!(
            "  r{:<2} {:#010x}  r{:<2} {:#010x}  r{:<2} {:#010x}  r{:<2} {:#010x}",
            reg,
            machine.reg(reg),
            reg + 1,
            machine.reg(reg + 1),
            reg + 2,
            machine.reg(reg + 2),
            reg + 3,
            machine.reg(reg + 3)
        );
    }
}
