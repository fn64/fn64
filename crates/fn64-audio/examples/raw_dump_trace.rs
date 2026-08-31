//! Trace which ucode step corrupts a given output sample in a raw live
//! audio-task dump (see raw_dump_replay for the dump format).
//!
//! usage: raw_dump_trace <dumpdir> <index> <logical_rdram_addr_hex>
//!
//! Phase A: full replay, list DMA-journal writes covering the watch address.
//! Phase B: single-step replay watching the DMEM staging byte for that DMA,
//!          printing PC/instruction/VU context when the watched i16 changes.

use fn64_audio::hle_rspboot::{execute_audio_rspboot_to_entry, AudioRspbootInput};
use fn64_audio::rsp::runtime::{RspDmaDirection, RspMachine};
use fn64_audio::rsp::{run_imem, RspExitReason};
use fn64_runtime::rsp::RspMemory;
use fn64_runtime::{
    OsTaskHeader, RdramAddr, RspMemAddr, RspMemoryBank, SP_STATUS_BROKE, SP_STATUS_HALT,
};

fn logical(storage: &[u8], addr: usize) -> u8 {
    storage[addr ^ 3]
}

fn logical_range(storage: &[u8], addr: usize, len: usize) -> Vec<u8> {
    (addr..addr + len).map(|a| logical(storage, a)).collect()
}

fn logical_imem_words(imem: &[u8; 0x1000]) -> [u32; 0x400] {
    let mut words = [0u32; 0x400];
    for (i, word) in words.iter_mut().enumerate() {
        *word = u32::from_be_bytes(imem[i * 4..i * 4 + 4].try_into().unwrap());
    }
    words
}

fn build_input(dir: &str, index: u64) -> AudioRspbootInput {
    let meta = std::fs::read(format!("{dir}/task_{index:05}.meta")).expect("read meta");
    assert_eq!(meta.len(), 64);
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
    let task_addr = task_addr.expect("OSTask header not found in RDRAM");

    let mut rsp_memory = RspMemory::new();
    let boot = logical_range(
        &rdram,
        (header.ucode_boot & 0x007f_ffff) as usize,
        header.ucode_boot_size as usize,
    );
    rsp_memory
        .write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &boot)
        .unwrap();
    rsp_memory
        .write_bytes(RspMemAddr::from_register(0xfc0), &meta)
        .unwrap();
    let machine_state = {
        let mut scratch = [0u8; 8];
        let mut machine = RspMachine::new(&mut scratch);
        machine.set_sp_status_raw(SP_STATUS_HALT | SP_STATUS_BROKE);
        machine.snapshot_state()
    };
    AudioRspbootInput::new(
        RdramAddr::from_offset(task_addr),
        header,
        rdram,
        rsp_memory.snapshot(),
        0,
        machine_state,
    )
    .expect("construct rspboot input")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("usage: raw_dump_trace <dumpdir> <index> <addr_hex>");
    let index: u64 = args.next().expect("index").parse().unwrap();
    let watch_addr = usize::from_str_radix(
        args.next().expect("watch addr").trim_start_matches("0x"),
        16,
    )
    .unwrap();
    // Optional: dump the full VU register file whenever pc hits this value
    // (step range limited by the next two optional args).
    let probe_pcs: Vec<u32> = args
        .next()
        .map(|s| {
            s.split(',')
                .map(|p| u32::from_str_radix(p.trim_start_matches("0x"), 16).unwrap())
                .collect()
        })
        .unwrap_or_default();
    let probe_from: u64 = args.next().map(|s| s.parse().unwrap()).unwrap_or(0);
    let probe_to: u64 = args.next().map(|s| s.parse().unwrap()).unwrap_or(u64::MAX);

    let input = build_input(&dir, index);
    let entry = execute_audio_rspboot_to_entry(input)
        .expect("execute rspboot")
        .into_entry();

    // ---- Phase A: full run, journal the DMAs over the watch address ----
    let mut rdram = entry.rdram().storage().to_vec();
    let mut persistent = RspMemory::from_snapshot(entry.rsp_memory().clone());
    let mut imem = *persistent.bank(RspMemoryBank::Imem);
    let mut machine = RspMachine::new(&mut rdram);
    machine.set_dma_rdram_ranges(entry.admitted_dma_ranges().to_vec());
    machine.load_dmem_logical(persistent.bank(RspMemoryBank::Dmem));
    machine.restore_state(entry.machine_state().clone());
    let mut pc = entry.entry_pc_low12();
    loop {
        let words = logical_imem_words(&imem);
        let result = run_imem(&words, pc, &mut machine, 4096);
        pc = result.pc & 0x0fff;
        match result.reason {
            RspExitReason::Broke => break,
            RspExitReason::SwapOverlay => machine.complete_imem_dma(&mut imem),
            RspExitReason::StepLimit => {}
            reason => panic!("unexpected exit {reason:?} at pc {pc:#x}"),
        }
    }
    let journal = machine.take_dma_journal();
    drop(machine);
    let mut watch_dmem: Option<(u32, u32)> = None; // (sp_mem addr of watch byte, dma dram start)
    for entry_row in &journal {
        let len = (entry_row.raw_length_descriptor & 0xFFF) as usize + 1;
        let dram = entry_row.effective_dram_address as usize;
        if matches!(entry_row.direction, RspDmaDirection::Write)
            && watch_addr >= dram
            && watch_addr < dram + len
        {
            let sp = entry_row.sp_mem_address + (watch_addr - dram) as u32;
            println!(
                "DMA write covers watch: dram {dram:#x}+{len:#x} from sp_mem {:#x} -> watch byte staged at DMEM {sp:#x}",
                entry_row.sp_mem_address
            );
            watch_dmem = Some((sp, dram as u32));
        }
    }
    let Some((watch_sp, _)) = watch_dmem else {
        println!("no DMA write covered the watch address; nothing to trace");
        return;
    };
    let watch_sp = (watch_sp & 0xFFF) as usize;

    // ---- Phase B: single-step run watching the DMEM staging i16 ----
    let mut rdram = entry.rdram().storage().to_vec();
    let mut persistent = RspMemory::from_snapshot(entry.rsp_memory().clone());
    let mut imem = *persistent.bank(RspMemoryBank::Imem);
    let mut machine = RspMachine::new(&mut rdram);
    machine.set_dma_rdram_ranges(entry.admitted_dma_ranges().to_vec());
    machine.load_dmem_logical(persistent.bank(RspMemoryBank::Dmem));
    machine.restore_state(entry.machine_state().clone());
    let mut pc = entry.entry_pc_low12();
    let mut step: u64 = 0;
    let read_watch = |machine: &RspMachine| -> i16 {
        let dmem = machine.dmem_logical();
        i16::from_be_bytes([dmem[watch_sp & !1], dmem[(watch_sp & !1) + 1]])
    };
    let mut last = read_watch(&machine);
    let mut history: Vec<(u64, u32, u32, i16)> = Vec::new(); // (step, pc, instr, new value)
    let mut dma_step: Option<u64> = None;
    loop {
        let words = logical_imem_words(&imem);
        let instr = words[(pc as usize & 0xFFF) >> 2];
        let before_pc = pc;
        if probe_pcs.contains(&pc) && step >= probe_from && step <= probe_to {
            println!("VU probe at pc {pc:#05x} step {step}:");
            for v in 0..32 {
                println!("  v{v:<2} = {:?}", machine.ctx.rsp.regs.r[v]);
            }
            let acc: Vec<i64> = (0..8).map(|l| machine.ctx.rsp.acc.signed(l)).collect();
            println!("  acc = {acc:x?}");
            println!(
                "  vco = {:#06x} vcc = {:#06x} vce = {:#04x}",
                machine.ctx.rsp.flags.vco, machine.ctx.rsp.flags.vcc, machine.ctx.rsp.flags.vce
            );
        }
        let result = run_imem(&words, pc, &mut machine, 1);
        step += result.steps;
        pc = result.pc & 0x0fff;
        let now = read_watch(&machine);
        if now != last {
            history.push((step, before_pc, instr, now));
            last = now;
        }
        // Log any vector store (SWC2) whose effective DMEM address hits the
        // staging range, even when it stores an unchanged value. A store in a
        // branch delay slot executes within the branch's step, so check the
        // following word too.
        let delay_word = words[((before_pc as usize & 0xFFF) >> 2).wrapping_add(1) & 0x3FF];
        for instr in [instr, delay_word] {
        if instr >> 26 == 0b111010 {
            let base = ((instr >> 21) & 0x1F) as usize;
            let funct = (instr >> 11) & 0x1F;
            let offset7 = (instr & 0x7F) as i32 - if instr & 0x40 != 0 { 0x80 } else { 0 };
            let shift = match funct {
                0 => 0,  // SBV
                1 => 1,  // SSV
                2 => 2,  // SLV
                3 => 3,  // SDV
                4 => 4,  // SQV
                _ => 4,
            };
            let ea = (machine.ctx.r[base] as i32 + (offset7 << shift)) as u32 as usize & 0xFFF;
            let span = 1usize << shift;
            if ea < watch_sp + 0x50 && ea + span > watch_sp.saturating_sub(0x10) {
                let vt = ((instr >> 16) & 0x1F) as usize;
                println!(
                    "  store step {step:>7} pc {before_pc:#05x} instr {instr:#010x} funct={funct} ea={ea:#x} v{vt}={:?}",
                    machine.ctx.rsp.regs.r[vt]
                );
            }
        }
        }
        for row in machine.take_dma_journal() {
            let len = (row.raw_length_descriptor & 0xFFF) as usize + 1;
            let dram = row.effective_dram_address as usize;
            if (0xd5e00..0xd6600).contains(&dram) {
                println!(
                    "  dma step {step:>7} pc {before_pc:#05x} {:?} dram {dram:#x}+{len:#x} sp {:#x}",
                    row.direction, row.sp_mem_address
                );
            }
            if dma_step.is_none()
                && matches!(row.direction, RspDmaDirection::Write)
                && watch_addr >= dram
                && watch_addr < dram + len
            {
                dma_step = Some(step);
                println!(
                    "covering DMA-out at step {step} (pc {before_pc:#05x}): dram {dram:#x}+{len:#x} from sp_mem {:#x}",
                    row.sp_mem_address
                );
            }
        }
        match result.reason {
            RspExitReason::Broke => break,
            RspExitReason::SwapOverlay => machine.complete_imem_dma(&mut imem),
            RspExitReason::StepLimit => {}
            reason => panic!("unexpected exit {reason:?} at pc {pc:#x}"),
        }
    }
    let cutoff = dma_step.unwrap_or(u64::MAX);
    println!(
        "DMEM {watch_sp:#x} (staging for RDRAM {watch_addr:#x}): {} changes; last before DMA (step {cutoff}):",
        history.len()
    );
    for (step, pc, instr, value) in history.iter().filter(|(s, ..)| *s <= cutoff).rev().take(6) {
        println!("  step {step:>7} pc {pc:#05x} instr {instr:#010x} -> {value}");
    }

    // Chunk-boundary sensitivity: the single-step (budget=1) phase-B machine
    // just finished the whole task. Its final RDRAM must equal the live
    // capture (produced with large chunked budgets); any difference means
    // run_imem mishandles state across step-budget boundaries.
    drop(machine);
    let post = std::fs::read(format!("{dir}/task_{index:05}.post.rdram")).expect("read post");
    let diffs = rdram
        .iter()
        .zip(post.iter())
        .filter(|(a, b)| a != b)
        .count();
    if diffs == 0 {
        println!("BUDGET=1 replay matches live post-RDRAM byte-for-byte");
    } else {
        let first = rdram.iter().zip(post.iter()).position(|(a, b)| a != b).unwrap();
        println!("BUDGET=1 replay DIFFERS from live post: {diffs} bytes, first at {first:#x}");
    }
}
