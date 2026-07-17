//! Replay one captured OoT audio task against the generated aspMain adapter.
//!
//! Inputs are produced by `FN64_DUMP_AUDIO_TASK=/tmp/task.rdram` and its
//! sidecar. The generated ucode remains out-of-tree via this crate's normal
//! build script; this binary only provides a deterministic local replay loop.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let mut args = env::args_os().skip(1);
    let Some(rdram_path) = args.next().map(PathBuf::from) else {
        panic!("usage: replay_task /path/to/task.rdram [/path/to/task.meta]");
    };
    let meta_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| rdram_path.with_extension("meta"));

    let mut rdram = fs::read(&rdram_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", rdram_path.display()));
    let before = rdram.clone();
    let task_offset = read_task_offset(&meta_path);

    oot_audio_ucode::set_rdram_len(rdram.len());
    let reason = unsafe { oot_audio_ucode::oot_audio_ucode(rdram.as_mut_ptr(), task_offset) };
    println!("exit_reason={reason:#x}");

    print_task_header(&before, task_offset as usize);
    print_save_commands(&before, task_offset as usize);
    print_changed_ranges(&before, &rdram);
    if let Some(path) = env::var_os("RSP_TRACE_WRITE_RDRAM") {
        fs::write(&path, &rdram).unwrap_or_else(|error| {
            panic!("failed to write {}: {error}", PathBuf::from(path).display())
        });
    }
}

fn read_task_offset(path: &Path) -> u32 {
    let meta = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    for line in meta.lines() {
        if let Some(value) = line.strip_prefix("task_offset=") {
            return value.parse().unwrap_or_else(|error| {
                panic!("invalid task_offset in {}: {error}", path.display())
            });
        }
    }
    panic!("{} does not contain task_offset=", path.display());
}

fn print_task_header(rdram: &[u8], task: usize) {
    let field = |offset| read_native_u32(rdram, task + offset) & 0x00FF_FFFF;
    println!(
        "task=0x{task:06x} ucode=0x{:06x} ucode_data=0x{:06x} data_ptr=0x{:06x} data_size={}",
        field(0x10),
        field(0x18),
        field(0x30),
        read_native_u32(rdram, task + 0x34)
    );
}

fn print_save_commands(rdram: &[u8], task: usize) {
    let data_ptr = (read_native_u32(rdram, task + 0x30) & 0x00FF_FFFF) as usize;
    let data_size = read_native_u32(rdram, task + 0x34) as usize;
    let mut saves = 0usize;
    for offset in (0..data_size).step_by(8) {
        let w0 = read_guest_u32(rdram, data_ptr + offset);
        let w1 = read_guest_u32(rdram, data_ptr + offset + 4);
        if w0 >> 24 == 0x15 {
            saves += 1;
            println!(
                "save[{saves:02}] cmd={} w0=0x{w0:08x} dst=0x{:06x}",
                offset / 8,
                w1 & 0x00FF_FFFF
            );
        }
    }
}

fn print_changed_ranges(before: &[u8], after: &[u8]) {
    let mut ranges = Vec::new();
    let mut start = None;
    for (index, (&a, &b)) in before.iter().zip(after).enumerate() {
        match (start, a == b) {
            (None, false) => start = Some(index),
            (Some(s), true) => {
                ranges.push((s, index));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        ranges.push((s, before.len().min(after.len())));
    }

    println!("changed_ranges={}", ranges.len());
    for (index, (start, end)) in ranges.iter().copied().take(64).enumerate() {
        println!(
            "changed[{index:02}]=0x{start:06x}..0x{end:06x} bytes={}",
            end - start
        );
    }
    if ranges.len() > 64 {
        println!("changed_truncated={}", ranges.len() - 64);
    }
}

fn read_native_u32(rdram: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(rdram[offset..offset + 4].try_into().unwrap())
}

fn read_guest_u32(rdram: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        rdram[offset ^ 3],
        rdram[(offset + 1) ^ 3],
        rdram[(offset + 2) ^ 3],
        rdram[(offset + 3) ^ 3],
    ])
}
