//! Standalone verification tool: parse a real reference savestate file
//! (path given on the command line -- never checked into this repo, per
//! `fn64/README.md`'s "no game content" rule) and print its resume PC,
//! GPRs, and CP0 registers in the same layout the faki-tools oracle's
//! `breakpoint` command uses, for a byte-for-byte cross-check against the
//! oracle's own output.
//!
//! Usage: `cargo run -p fn64-diff --bin dump_snapshot -- <path/to/state.stN>`
use fn64_diff::savestate;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| panic!("usage: dump_snapshot <path/to/state.stN>"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let snap = savestate::parse(&bytes).unwrap_or_else(|e| panic!("parse {path}: {e}"));

    println!("version: {:#010x}", snap.version);
    println!("rom_md5: {}", snap.rom_md5);
    println!("raw pc (savestate field): {:#010x}", snap.pc);
    println!(
        "cp0_status={:#010x} cp0_cause={:#010x} cp0_epc={:#010x} cp0_badvaddr={:#010x} cp0_count={:#010x}",
        snap.cp0[savestate::CP0_STATUS],
        snap.cp0[savestate::CP0_CAUSE],
        snap.cp0[savestate::CP0_EPC],
        snap.cp0[8],
        snap.cp0[9],
    );
    println!(
        "resume_pc() (what state transplant should use): {:#010x}",
        snap.resume_pc()
    );

    for (i, (name, value)) in savestate::GPR_NAMES
        .iter()
        .zip(snap.gprs.iter())
        .enumerate()
    {
        print!("{name}={:#010x}  ", *value as u32);
        if i % 8 == 7 {
            println!();
        }
    }

    if let Some(word) = fn64_diff::read_raw_be_word(&snap, snap.resume_pc()) {
        println!("raw big-endian word at resume_pc: {word:#010x}");
    } else {
        println!("resume_pc is outside the 0x80000000-based RDRAM window sampled here");
    }
    println!("rdram bytes captured: {:#x}", snap.rdram.len());
}
