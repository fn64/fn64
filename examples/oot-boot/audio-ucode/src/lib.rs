//! Build-only adapter for OoT's generated aspMain module.
//!
//! The generated, game-derived module is copied from the sibling `aki-recomp`
//! checkout into Cargo's `OUT_DIR`; no game bytes or generated game code enter
//! this repository. Keeping this small wrapper in the active fn64 worktree
//! makes its `fn64-audio` dependency resolve to the same package instance as
//! `fn64-abi`, avoiding Cargo's path-source collision with another worktree.
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

mod oot_aspmain {
    include!(concat!(env!("OUT_DIR"), "/oot_aspmain.rs"));
}

pub use oot_aspmain::oot_aspmain_ucode;

thread_local! {
    static RDRAM_LEN: Cell<usize> = const { Cell::new(0) };
}

pub fn set_rdram_len(len: usize) {
    RDRAM_LEN.with(|cell| cell.set(len));
}

/// Run the generated OoT aspMain against one live `OSTask`.
///
/// # Safety
///
/// `rdram` must remain valid for the length registered with
/// [`set_rdram_len`], and `task_offset..task_offset + 64` must be in bounds.
#[no_mangle]
pub unsafe extern "C" fn oot_audio_ucode(rdram: *mut u8, task_offset: u32) -> u32 {
    let len = RDRAM_LEN.with(Cell::get);
    assert!(len != 0, "OoT audio adapter: rdram length not configured");

    let rdram = unsafe { std::slice::from_raw_parts_mut(rdram, len) };
    let task_offset = task_offset as usize;
    assert!(
        task_offset
            .checked_add(0x40)
            .is_some_and(|end| end <= rdram.len()),
        "OoT audio adapter: OSTask at {task_offset:#x} is out of bounds"
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
        "OoT audio adapter: ucode data at {ucode_data:#x} is out of bounds"
    );
    machine.dmem.as_bytes_mut()[..0xF80]
        .copy_from_slice(&machine.rdram[ucode_data..ucode_data + 0xF80]);

    oot_aspmain_ucode(&mut machine) as u32
}
