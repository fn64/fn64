//! Replay a raw live audio-task dump (task_N.meta + task_N.pre.rdram from
//! FN64_AUDIO_TASK_DUMP_DIR) through the offline whole-task interpreter and
//! byte-compare the final RDRAM against the captured task_N.post.rdram.
//!
//! usage: raw_dump_replay <dumpdir> <index>

use fn64_audio::hle::{AudioHleCatalog, AudioHleCatalogEntry};
use fn64_audio::hle_outcome::{AudioHleFamily, CanonicalRdramRanges};
use fn64_audio::hle_rspboot::execute_audio_rspboot_to_entry;
use fn64_audio::rsp::runtime::RspMachine;
use fn64_audio::whole_task::prepare_no_dpc_submission_whole_audio_task;
use fn64_runtime::rsp::RspMemory;
use fn64_runtime::{
    OsTaskHeader, RdramAddr, RspMemAddr, RspMemoryBank, SP_STATUS_BROKE, SP_STATUS_HALT,
};

/// Logical byte `a` of RDRAM stored in native little-endian word order.
fn logical(storage: &[u8], addr: usize) -> u8 {
    storage[addr ^ 3]
}

fn logical_range(storage: &[u8], addr: usize, len: usize) -> Vec<u8> {
    (addr..addr + len).map(|a| logical(storage, a)).collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("usage: raw_dump_replay <dumpdir> <index>");
    let index: u64 = args.next().expect("missing index").parse().expect("index");

    let meta = std::fs::read(format!("{dir}/task_{index:05}.meta")).expect("read meta");
    assert_eq!(meta.len(), 64, "meta must be 16 BE u32 header words");
    let word = |i: usize| u32::from_be_bytes(meta[i * 4..i * 4 + 4].try_into().unwrap());
    let header = OsTaskHeader {
        task_type: word(0),
        flags: word(1),
        ucode_boot: word(2),
        ucode_boot_size: word(3),
        ucode: word(4),
        ucode_size: word(5),
        ucode_data: word(6),
        ucode_data_size: word(7),
        dram_stack: word(8),
        dram_stack_size: word(9),
        output_buff: word(10),
        output_buff_size: word(11),
        data_ptr: word(12),
        data_size: word(13),
        yield_data_ptr: word(14),
        yield_data_size: word(15),
    };

    let rdram = std::fs::read(format!("{dir}/task_{index:05}.pre.rdram")).expect("read pre rdram");

    // Locate the 64-byte OSTask header inside RDRAM (its logical bytes are
    // exactly the meta file), to recover task_addr.
    let mut task_addr = None;
    'scan: for base in (0..rdram.len() - 64).step_by(8) {
        for (offset, &expected) in meta.iter().enumerate() {
            if logical(&rdram, base + offset) != expected {
                continue 'scan;
            }
        }
        task_addr = Some(base as u32);
        break;
    }
    let task_addr = task_addr.expect("OSTask header bytes not found in RDRAM");
    eprintln!("task_addr = {task_addr:#x}");

    let mut rsp_memory = RspMemory::new();
    let boot = logical_range(
        &rdram,
        (header.ucode_boot & 0x007f_ffff) as usize,
        header.ucode_boot_size as usize,
    );
    rsp_memory
        .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &boot)
        .expect("boot ucode fits IMEM");
    rsp_memory
        .write_bytes(RspMemAddr::from_register(0xfc0), &meta)
        .expect("header fits DMEM tail");

    let machine_state = {
        let mut scratch = [0u8; 8];
        let mut machine = RspMachine::new(&mut scratch);
        machine.set_sp_status_raw(SP_STATUS_HALT | SP_STATUS_BROKE);
        machine.snapshot_state()
    };

    let input = fn64_audio::hle_rspboot::AudioRspbootInput::new(
        RdramAddr::from_offset(task_addr),
        header,
        rdram,
        rsp_memory.snapshot(),
        0,
        machine_state,
    )
    .expect("construct rspboot input");

    let boot_result = execute_audio_rspboot_to_entry(input.clone()).expect("execute rspboot");
    let identity = boot_result.entry().identity();
    let entries = [AudioHleCatalogEntry {
        identity,
        family: AudioHleFamily::StandardAbi,
        implementation_revision: 1,
    }];
    let admission = AudioHleCatalog::new(&entries)
        .expect("one-entry catalog")
        .admit(identity)
        .expect("admit identity");
    let prepared =
        prepare_no_dpc_submission_whole_audio_task(input, admission, CanonicalRdramRanges::default())
            .expect("replay whole audio task");
    let replayed = prepared.reference().lle_result().rdram_storage();

    eprintln!(
        "replayed: rspboot_steps={} ucode_steps={}",
        prepared.reference().steps().rspboot(),
        prepared.reference().steps().ucode(),
    );

    let post =
        std::fs::read(format!("{dir}/task_{index:05}.post.rdram")).expect("read post rdram");
    assert_eq!(replayed.len(), post.len(), "rdram length mismatch");
    let mut diffs = 0usize;
    let mut first: Option<usize> = None;
    for a in 0..post.len() {
        if replayed[a] != post[a] {
            diffs += 1;
            if first.is_none() {
                first = Some(a);
            }
        }
    }
    match first {
        None => println!("REPLAY EXACT: offline replay reproduces the live post-RDRAM byte-for-byte"),
        Some(at) => println!(
            "REPLAY DIFFERS: {diffs} bytes differ, first at storage offset {at:#x} \
             (replay {:#04x} vs live {:#04x})",
            replayed[at], post[at]
        ),
    }
}
