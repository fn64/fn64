//! Out-of-tree adapter for the AKI-family audio microcode.
//!
//! `build.rs` recompiles the ucode from the user's own ROM into `OUT_DIR`; no
//! game-derived bytes enter this repository. The wrapper mirrors the OoT
//! adapter's DMEM-seeding contract: seed the 64-byte OSTask into DMEM[0xFC0],
//! copy the ucode data segment into DMEM[0..0xF80], then run the recompiled
//! ucode against the live RSP machine.
#![allow(
    unused_variables,
    unused_assignments,
    unused_parens,
    unused_braces,
    unused_imports,
    clippy::all
)]

use std::cell::Cell;

use fn64_audio::rsp::recomp::runtime::RspMachine;

mod aki_audio {
    include!(concat!(env!("OUT_DIR"), "/aki_audio_ucode.rs"));
}

pub use aki_audio::aki_audio_ucode as aki_audio_ucode_inner;

thread_local! {
    static RDRAM_LEN: Cell<usize> = const { Cell::new(0) };
}

pub fn set_rdram_len(len: usize) {
    RDRAM_LEN.with(|cell| cell.set(len));
}

/// Run the recompiled AKI audio ucode against one live `OSTask`.
///
/// Signature matches `fn64_abi::AudioUcodeFn` for optional translated-policy
/// experiments. Release/parity harnesses use live-IMEM LLE authority.
///
/// # Safety
///
/// `rdram` must remain valid for the length registered with [`set_rdram_len`],
/// and `task_offset..task_offset + 64` must be in bounds.
#[no_mangle]
pub unsafe extern "C" fn aki_audio_ucode(rdram: *mut u8, task_offset: u32) -> u32 {
    let len = RDRAM_LEN.with(Cell::get);
    assert!(len != 0, "AKI audio adapter: rdram length not configured");

    let rdram = unsafe { std::slice::from_raw_parts_mut(rdram, len) };
    let task_offset = task_offset as usize;
    assert!(
        task_offset
            .checked_add(0x40)
            .is_some_and(|end| end <= rdram.len()),
        "AKI audio adapter: OSTask at {task_offset:#x} is out of bounds"
    );

    let mut machine = RspMachine::new(rdram);
    machine.dmem.as_bytes_mut()[0xFC0..0x1000]
        .copy_from_slice(&machine.rdram[task_offset..task_offset + 0x40]);

    let ucode_data = u32::from_ne_bytes(
        machine.rdram[task_offset + 0x18..task_offset + 0x1C]
            .try_into()
            .unwrap(),
    ) & 0x00FF_FFFF;
    let ucode_data = ucode_data as usize;
    assert!(
        ucode_data
            .checked_add(0xF80)
            .is_some_and(|end| end <= machine.rdram.len()),
        "AKI audio adapter: ucode data at {ucode_data:#x} is out of bounds"
    );
    machine.dmem.as_bytes_mut()[..0xF80]
        .copy_from_slice(&machine.rdram[ucode_data..ucode_data + 0xF80]);

    aki_audio_ucode_inner(&mut machine) as u32
}
