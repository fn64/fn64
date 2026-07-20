//! Voice Recognition Unit ABI adapters.
//!
//! Public `os_voice.h` supplies every signature and structure layout. The
//! public Voice Recognition System manuals supply error codes, dictionary and
//! gain constraints, and lifecycle rules. Actual recognition is host input;
//! [`inject_voice_result`] is the deterministic seam.

use super::*;

const CONT_ERR_NO_CONTROLLER: u32 = 1;
const CONT_ERR_INVALID: u32 = 5;
const CONT_ERR_DEVICE: u32 = 11;
const CONT_ERR_NOT_READY: u32 = 12;
const CONT_ERR_VOICE_WORD: u32 = 14;
const VOICE_HANDLE_MODE: u32 = 1;

fn voice_error_code(error: fn64_runtime::VoiceError) -> u32 {
    match error {
        fn64_runtime::VoiceError::Invalid => CONT_ERR_INVALID,
        fn64_runtime::VoiceError::InvalidWord => CONT_ERR_VOICE_WORD,
        fn64_runtime::VoiceError::NotReady => CONT_ERR_NOT_READY,
    }
}

fn with_voice<R>(
    channel: usize,
    evidence: fn64_runtime::ControllerOperationKind,
    operation: impl FnOnce(&mut fn64_runtime::VoiceUnit) -> Result<R, fn64_runtime::VoiceError>,
) -> Result<R, u32> {
    let result = with_executor(|exec| match exec.pif().port_state(channel) {
        fn64_runtime::PortState::VoiceRecognitionUnit => operation(
            exec.voice_unit_mut(channel)
                .expect("VRU identity exists without VoiceUnit state"),
        )
        .map_err(voice_error_code),
        fn64_runtime::PortState::Absent | fn64_runtime::PortState::StandardControllerNoPak => {
            Err(CONT_ERR_NO_CONTROLLER)
        }
        fn64_runtime::PortState::StandardControllerControllerPak
        | fn64_runtime::PortState::StandardControllerRumblePak
        | fn64_runtime::PortState::StandardControllerTransferPak => Err(CONT_ERR_DEVICE),
    });
    if result.is_ok() {
        crate::record_controller_operation(
            channel,
            fn64_runtime::ControllerOperationDevice::VoiceRecognitionUnit,
            evidence,
        );
    }
    result
}

fn handle_channel(storage: fn64_runtime::RdramPtr, raw: u64) -> Result<usize, u32> {
    let handle = RdramAddr::from_gpr(raw);
    let mode = unsafe {
        storage.read_u32(
            handle
                .checked_add(8)
                .expect("OSVoiceHandle mode field overflow"),
        )
    };
    if mode != VOICE_HANDLE_MODE {
        return Err(CONT_ERR_INVALID);
    }
    Ok(unsafe {
        storage.read_u32(
            handle
                .checked_add(4)
                .expect("OSVoiceHandle channel field overflow"),
        )
    } as usize)
}

fn set_result(ctx: &mut RecompContext, result: Result<(), u32>) {
    ctx.r2 = result.err().unwrap_or(0) as u64;
}

fn update_handle_status(storage: fn64_runtime::RdramPtr, handle_raw: u64, channel: usize) {
    let status = with_executor(|exec| {
        exec.voice_unit_mut(channel)
            .expect("initialized voice handle lost its VRU")
            .status()
    });
    unsafe {
        storage.write_u8(
            RdramAddr::from_gpr(handle_raw)
                .checked_add(12)
                .expect("OSVoiceHandle status field overflow"),
            status,
        );
    }
}

fn read_voice_word(storage: fn64_runtime::RdramPtr, raw: u64) -> Result<Vec<u8>, u32> {
    let base = RdramAddr::from_gpr(raw);
    let mut bytes = Vec::new();
    for index in 0..=34u32 {
        let byte = unsafe {
            storage.read_u8(
                base.checked_add(index)
                    .expect("voice word guest address overflow"),
            )
        };
        if byte == 0 {
            return (!bytes.is_empty())
                .then_some(bytes)
                .ok_or(CONT_ERR_VOICE_WORD);
        }
        bytes.push(byte);
    }
    Err(CONT_ERR_VOICE_WORD)
}

unsafe fn copy_from_guest(storage: fn64_runtime::RdramPtr, raw: u64, len: usize) -> Vec<u8> {
    let base = RdramAddr::from_gpr(raw);
    (0..len)
        .map(|index| unsafe {
            storage.read_u8(
                base.checked_add(u32::try_from(index).expect("voice mask exceeds u32"))
                    .expect("voice mask guest address overflow"),
            )
        })
        .collect()
}

/// Host recognition seam. Calling this before a VRU is attached traps loudly.
pub fn inject_voice_result(port: usize, result: fn64_runtime::VoiceData) {
    with_executor(|exec| {
        exec.voice_unit_mut(port)
            .unwrap_or_else(|| panic!("no Voice Recognition Unit attached to port {port}"))
            .inject_result(result)
            .unwrap_or_else(|error| {
                panic!("cannot inject a Voice Recognition Unit result on port {port}: {error:?}")
            });
    });
}

/// Host microphone seam: mark the active utterance as detected/processing.
/// Calling this outside the public START/CANCEL lifecycle traps loudly.
pub fn mark_voice_detected(port: usize) {
    with_executor(|exec| {
        exec.voice_unit_mut(port)
            .unwrap_or_else(|| panic!("no Voice Recognition Unit attached to port {port}"))
            .mark_voice_detected()
            .unwrap_or_else(|error| {
                panic!("cannot mark Voice Recognition Unit input on port {port}: {error:?}")
            });
    });
}

/// Initialize a public `OSVoiceHandle` for the selected controller port.
///
/// # Safety
/// Guest pointers in `ctx` must address their complete public ABI objects in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osVoiceInit_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let channel = ctx.r6 as usize;
    let result = with_voice(
        channel,
        fn64_runtime::ControllerOperationKind::Control,
        |voice| {
            voice.initialize();
            Ok(())
        },
    );
    if result.is_ok() {
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        let handle = RdramAddr::from_gpr(ctx.r5);
        unsafe {
            storage.write_u32(handle, ctx.r4 as u32);
            storage.write_u32(
                handle
                    .checked_add(4)
                    .expect("OSVoiceHandle channel field overflow"),
                channel as u32,
            );
            storage.write_u32(
                handle
                    .checked_add(8)
                    .expect("OSVoiceHandle mode field overflow"),
                VOICE_HANDLE_MODE,
            );
            storage.write_u8(
                handle
                    .checked_add(12)
                    .expect("OSVoiceHandle status field overflow"),
                fn64_runtime::voice::VOICE_STATUS_READY,
            );
        }
    }
    set_result(ctx, result.map(|_| ()));
}

/// Validate one public voice word image.
///
/// # Safety
/// The word pointer in `ctx` must address a terminated public word buffer in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osVoiceCheckWord_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    ctx.r2 = read_voice_word(storage, ctx.r4).err().unwrap_or(0) as u64;
}

/// Clear a dictionary on a previously initialized voice handle.
///
/// # Safety
/// The handle pointer in `ctx` must address a complete `OSVoiceHandle` in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osVoiceClearDictionary_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = handle_channel(storage, ctx.r4).and_then(|channel| {
        let result = with_voice(
            channel,
            fn64_runtime::ControllerOperationKind::Control,
            |voice| voice.clear_dictionary(ctx.r5 as u8),
        );
        if result.is_ok() {
            update_handle_status(storage, ctx.r4, channel);
        }
        result
    });
    set_result(ctx, result);
}

/// Add one public word image to the active dictionary.
///
/// # Safety
/// Handle and word pointers in `ctx` must address complete objects in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osVoiceSetWord_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = handle_channel(storage, ctx.r4).and_then(|channel| {
        let word = read_voice_word(storage, ctx.r5)?;
        with_voice(
            channel,
            fn64_runtime::ControllerOperationKind::Write,
            |voice| voice.set_word(&word),
        )
    });
    set_result(ctx, result);
}

/// Install a dictionary mask from guest memory.
///
/// # Safety
/// Handle and mask ranges in `ctx` must be valid in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osVoiceMaskDictionary_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = handle_channel(storage, ctx.r4).and_then(|channel| {
        let mask = unsafe { copy_from_guest(storage, ctx.r5, ctx.r6 as u32 as usize) };
        with_voice(
            channel,
            fn64_runtime::ControllerOperationKind::Write,
            |voice| voice.set_mask(&mask),
        )
    });
    set_result(ctx, result);
}

/// Set the public analog and digital VRU gains.
///
/// # Safety
/// The handle pointer in `ctx` must address a complete `OSVoiceHandle` in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osVoiceControlGain_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = handle_channel(storage, ctx.r4).and_then(|channel| {
        with_voice(
            channel,
            fn64_runtime::ControllerOperationKind::Control,
            |voice| voice.set_gain(ctx.r5 as i32, ctx.r6 as i32),
        )
    });
    set_result(ctx, result);
}

/// Begin recognition using the active dictionary.
///
/// # Safety
/// The handle pointer in `ctx` must address a complete `OSVoiceHandle` in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osVoiceStartReadData_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = handle_channel(storage, ctx.r4).and_then(|channel| {
        let result = with_voice(
            channel,
            fn64_runtime::ControllerOperationKind::Control,
            fn64_runtime::VoiceUnit::start,
        );
        if result.is_ok() {
            update_handle_status(storage, ctx.r4, channel);
        }
        result
    });
    set_result(ctx, result);
}

/// Stop an active recognition request.
///
/// # Safety
/// The handle pointer in `ctx` must address a complete `OSVoiceHandle` in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osVoiceStopReadData_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = handle_channel(storage, ctx.r4).and_then(|channel| {
        with_voice(
            channel,
            fn64_runtime::ControllerOperationKind::Control,
            |voice| {
                voice.stop();
                Ok(())
            },
        )?;
        update_handle_status(storage, ctx.r4, channel);
        Ok(())
    });
    set_result(ctx, result);
}

/// Copy the next recognition result into a public `OSVoiceData`.
///
/// # Safety
/// Handle and output pointers in `ctx` must address complete objects in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osVoiceGetReadData_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = handle_channel(storage, ctx.r4).and_then(|channel| {
        let result = with_voice(
            channel,
            fn64_runtime::ControllerOperationKind::Read,
            fn64_runtime::VoiceUnit::take_result,
        );
        update_handle_status(storage, ctx.r4, channel);
        result.map(|data| {
            let out = RdramAddr::from_gpr(ctx.r5);
            let mut values = [0u16; 15];
            values[0] = data.warning;
            values[1] = data.answer_num;
            values[2] = data.voice_level;
            values[3] = data.voice_sn;
            values[4] = data.voice_time;
            values[5..10].copy_from_slice(&data.answer);
            values[10..15].copy_from_slice(&data.distance);
            for (index, value) in values.into_iter().enumerate() {
                unsafe {
                    storage.write_u16(
                        out.checked_add((index as u32) * 2)
                            .expect("OSVoiceData field address overflow"),
                        value,
                    );
                }
            }
            update_handle_status(storage, ctx.r4, channel);
        })
    });
    set_result(ctx, result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ctx_zeroed;

    #[test]
    fn voice_dictionary_and_host_result_flow_through_public_structs() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        crate::load_rom(vec![0; 0x100]);
        crate::si::set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);
        let mut rdram = vec![0u8; 0x200];
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
        for (index, byte) in b"voice\0".iter().copied().enumerate() {
            unsafe { storage.write_u8(RdramAddr::from_offset(0x80 + index as u32), byte) };
        }
        unsafe { storage.write_u8(RdramAddr::from_offset(0xA0), 1) };

        let mut init = ctx_zeroed();
        init.r4 = 0x8000_0020;
        init.r5 = 0x8000_0040;
        init.r6 = 0;
        unsafe { osVoiceInit_recomp(rdram.as_mut_ptr(), &mut init) };
        assert_eq!(init.r2, 0);

        let mut clear = ctx_zeroed();
        clear.r4 = 0x8000_0040;
        clear.r5 = 1;
        unsafe { osVoiceClearDictionary_recomp(rdram.as_mut_ptr(), &mut clear) };
        assert_eq!(clear.r2, 0);

        let mut word = ctx_zeroed();
        word.r4 = 0x8000_0040;
        word.r5 = 0x8000_0080;
        unsafe { osVoiceSetWord_recomp(rdram.as_mut_ptr(), &mut word) };
        assert_eq!(word.r2, 0);

        let mut mask = ctx_zeroed();
        mask.r4 = 0x8000_0040;
        mask.r5 = 0x8000_00A0;
        mask.r6 = 1;
        unsafe { osVoiceMaskDictionary_recomp(rdram.as_mut_ptr(), &mut mask) };
        assert_eq!(mask.r2, 0);

        let mut start = ctx_zeroed();
        start.r4 = 0x8000_0040;
        unsafe { osVoiceStartReadData_recomp(rdram.as_mut_ptr(), &mut start) };
        assert_eq!(start.r2, 0);
        assert_eq!(unsafe { storage.read_u8(RdramAddr::from_offset(0x4C)) }, 1);

        let mut get = ctx_zeroed();
        get.r4 = 0x8000_0040;
        get.r5 = 0x8000_0100;
        unsafe { osVoiceGetReadData_recomp(rdram.as_mut_ptr(), &mut get) };
        assert_eq!(get.r2, CONT_ERR_NOT_READY as u64);

        mark_voice_detected(0);
        unsafe { osVoiceGetReadData_recomp(rdram.as_mut_ptr(), &mut get) };
        assert_eq!(get.r2, CONT_ERR_NOT_READY as u64);
        assert_eq!(
            unsafe { storage.read_u8(RdramAddr::from_offset(0x4C)) },
            fn64_runtime::voice::VOICE_STATUS_BUSY
        );

        inject_voice_result(
            0,
            fn64_runtime::VoiceData {
                answer_num: 1,
                voice_level: 0x1234,
                answer: [0, 0x7FFF, 0x7FFF, 0x7FFF, 0x7FFF],
                distance: [9, 0, 0, 0, 0],
                ..fn64_runtime::VoiceData::default()
            },
        );
        unsafe { osVoiceGetReadData_recomp(rdram.as_mut_ptr(), &mut get) };
        assert_eq!(get.r2, 0);
        assert_eq!(
            unsafe { storage.read_u16(RdramAddr::from_offset(0x102)) },
            1
        );
        assert_eq!(
            unsafe { storage.read_u16(RdramAddr::from_offset(0x104)) },
            0x1234
        );
        assert_eq!(
            unsafe { storage.read_u16(RdramAddr::from_offset(0x114)) },
            9
        );
        assert_eq!(unsafe { storage.read_u8(RdramAddr::from_offset(0x4C)) }, 0);
        assert_eq!(
            crate::copy_controller_operations(),
            [
                fn64_runtime::ControllerOperationKind::Control,
                fn64_runtime::ControllerOperationKind::Control,
                fn64_runtime::ControllerOperationKind::Write,
                fn64_runtime::ControllerOperationKind::Write,
                fn64_runtime::ControllerOperationKind::Control,
                fn64_runtime::ControllerOperationKind::Read,
            ]
            .into_iter()
            .map(|operation| fn64_runtime::ControllerOperationEvent {
                at: fn64_runtime::Cycles::ZERO,
                port: 0,
                device: fn64_runtime::ControllerOperationDevice::VoiceRecognitionUnit,
                operation,
            })
            .collect::<Vec<_>>()
        );
    }
}
