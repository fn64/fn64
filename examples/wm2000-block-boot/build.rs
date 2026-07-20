//! Build script: runs fn64-discover's real pipeline on the user's own WM2000
//! (NWXE) ROM (env var `ROM`, out-of-tree, never vendored), materializes the
//! admitted Block Pack, and writes two generated files into `OUT_DIR`:
//!
//! - `runner.rs` -- the sparse arbitrary-PC bank runner emitted by
//!   `fn64-recomp-rs` (same artifact `gate_b2` proves compiles), named
//!   `run_nwxe_boot`.
//! - `pack.rs` -- the pack's bank id, disjoint spans (start VA + words), and
//!   the boot bank's ROM copy window, as plain consts the runtime harness
//!   installs without re-running discovery.
//!
//! Everything here derives from the user's ROM at build time and lands only
//! under `target/` -- no game bytes are committed, matching
//! `../wm2000-boot`'s posture.

use std::env;
use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=ROM");
    let rom_path = env::var("ROM").unwrap_or_else(|_| {
        panic!(
            "wm2000-block-boot build.rs: required environment variable ROM is not set.\n\
             Point it at your own legally-obtained WM2000 (NWXE) ROM file. This crate \
             contains zero game content; the discovered pack is derived at build time \
             and never leaves target/."
        )
    });
    println!("cargo:rerun-if-changed={rom_path}");
    let rom_bytes = std::fs::read(&rom_path)
        .unwrap_or_else(|e| panic!("wm2000-block-boot build.rs: reading ROM {rom_path}: {e}"));

    let (rom, db) = fn64_discover::run_discovery(&rom_bytes, None)
        .unwrap_or_else(|e| panic!("wm2000-block-boot build.rs: ROM rejected: {e:?}"));
    let boot_mapping = db
        .proven_rom_mappings()
        .into_iter()
        .find_map(|fact| match fact {
            fn64_discover::Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                ..
            } if *bank == fn64_discover::banks::BOOT_BANK => {
                Some((*rom_start, *rom_end, *va_start))
            }
            _ => None,
        })
        .expect("wm2000-block-boot build.rs: boot bank not proven by discovery");
    let (rom_start, rom_end, va_start) = boot_mapping;
    let bank_bytes = &rom.bytes[rom_start as usize..rom_end as usize];
    let entrypoint = rom.header.entry_point;

    let input = fn64_discover::snapshot::MaterializedBankInput {
        bank: fn64_discover::banks::BOOT_BANK,
        va_start,
        bytes: bank_bytes,
        seed_roots: std::slice::from_ref(&entrypoint),
    };
    let snapshot = fn64_discover::snapshot::compose_materialized_bank_v1(&rom, &db, input)
        .unwrap_or_else(|e| panic!("wm2000-block-boot build.rs: composing snapshot: {e}"));
    let block_pack = fn64_discover::block_pack::emit_block_pack_v1(&snapshot, &rom)
        .unwrap_or_else(|e| panic!("wm2000-block-boot build.rs: emitting Block Pack: {e}"));
    let materialized = fn64_discover::block_pack::materialize_block_pack(&block_pack, &rom)
        .unwrap_or_else(|e| panic!("wm2000-block-boot build.rs: materializing Block Pack: {e}"));
    let bank = &materialized[0];

    let runner =
        fn64_discover::block_pack::emit_materialized_bank_runner(bank, "run_nwxe_boot");

    let mut pack = String::new();
    let _ = writeln!(pack, "pub const BANK_ID: u64 = {:#018X};", bank.bank_id);
    let _ = writeln!(pack, "pub const ENTRYPOINT: u32 = {entrypoint:#010X};");
    let _ = writeln!(
        pack,
        "pub const ROM_COPY: (usize, usize, u32) = ({rom_start:#X}, {rom_end:#X}, {va_start:#010X});"
    );
    let _ = writeln!(pack, "pub static SPANS: &[(u32, &[u32])] = &[");
    for block in &bank.blocks {
        let _ = write!(pack, "    ({:#010X}, &[", block.start_va);
        for word in &block.words {
            let _ = write!(pack, "{word:#010X}, ");
        }
        let _ = writeln!(pack, "]),");
    }
    let _ = writeln!(pack, "];");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    std::fs::write(out_dir.join("runner.rs"), runner).unwrap();
    std::fs::write(out_dir.join("pack.rs"), pack).unwrap();
    println!(
        "cargo:warning=wm2000-block-boot: packed {} blocks / {} words from {}",
        bank.blocks.len(),
        bank.blocks.iter().map(|b| b.words.len()).sum::<usize>(),
        rom_path
    );
}
