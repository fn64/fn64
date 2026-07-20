//! N64 Transfer Pak libultra adapters over [`fn64_runtime::TransferPak`].
//!
//! Signatures, status lifecycle, 32-byte Game Boy bus transfers, header
//! validation, connector checks, and the 0.2/0.12-second waits come from the
//! public libultra `osGbpak*` manuals. Raw register addresses and status-byte
//! behavior come from public Joybus hardware documentation. No GPL runtime
//! implementation was consulted.

use super::*;

const PFS_ERR_NOPACK: u32 = 1;
const PFS_ERR_CONTRFAIL: u32 = 4;
const PFS_ERR_INVALID: u32 = 5;
const PFS_ERR_DEVICE: u32 = 11;
const PFS_ERR_NO_GBCART: u32 = 12;
const PFS_ERR_NEW_GBCART: u32 = 13;

const PFS_GBPAK_INITIALIZED: u32 = 0x10;
const OS_READ: u32 = 0;
const OS_WRITE: u32 = 1;
const OS_GBPAK_POWER_OFF: u32 = 0;
const OS_GBPAK_POWER_ON: u32 = 1;

const OS_GBPAK_GBCART_ON: u8 = 0x01;
const OS_GBPAK_GBCART_PULL: u8 = 0x02;
const OS_GBPAK_POWER: u8 = 0x04;
const OS_GBPAK_RSTB_DETECTION: u8 = 0x08;

const INIT_WAIT_CYCLES: u64 = fn64_runtime::CPU_CLOCK_HZ / 5;
const POWER_ON_WAIT_CYCLES: u64 = fn64_runtime::CPU_CLOCK_HZ * 12 / 100;
const GB_BLOCK: usize = fn64_runtime::TRANSFER_PAK_BLOCK_SIZE;
const GB_REGISTRATION_SIZE: usize = 0x50;
const NINTENDO_LOGO_FNV1A64: u64 = 0x0e13_f858_5a99_f41f;

fn set_result(ctx: &mut RecompContext, result: Result<(), u32>) {
    ctx.r2 = result.err().unwrap_or(0) as u64;
}

fn gbpak_channel(storage: fn64_runtime::RdramPtr, raw: u64) -> Result<usize, u32> {
    let pfs = RdramAddr::from_gpr(raw);
    if unsafe { storage.read_u32(pfs) } != PFS_GBPAK_INITIALIZED {
        return Err(PFS_ERR_INVALID);
    }
    Ok(unsafe {
        storage.read_u32(
            pfs.checked_add(8)
                .expect("OSPfs Transfer Pak channel field address overflow"),
        )
    } as usize)
}

fn with_transfer_pak<R>(
    channel: usize,
    operation: impl FnOnce(&mut fn64_runtime::TransferPak) -> Result<R, u32>,
) -> Result<R, u32> {
    with_executor(|exec| {
        let now = Cycles::new(exec.sim_time());
        match exec.pif().port_state(channel) {
            fn64_runtime::PortState::StandardControllerTransferPak => {
                let pak = exec
                    .transfer_pak_mut(channel)
                    .expect("Transfer Pak identity exists without typed TransferPak state");
                pak.advance_to(now);
                operation(pak)
            }
            fn64_runtime::PortState::StandardControllerNoPak | fn64_runtime::PortState::Absent => {
                Err(PFS_ERR_NOPACK)
            }
            fn64_runtime::PortState::StandardControllerControllerPak
            | fn64_runtime::PortState::StandardControllerRumblePak
            | fn64_runtime::PortState::VoiceRecognitionUnit => Err(PFS_ERR_DEVICE),
        }
    })
}

fn status_byte(status: fn64_runtime::TransferPakStatus) -> u8 {
    (u8::from(status.cartridge_present) * OS_GBPAK_GBCART_ON)
        | (u8::from(status.cartridge_pulled) * OS_GBPAK_GBCART_PULL)
        | (u8::from(status.powered) * OS_GBPAK_POWER)
        | (u8::from(status.reset_detected) * OS_GBPAK_RSTB_DETECTION)
}

fn observe_status(
    channel: usize,
) -> Result<(fn64_runtime::TransferPakStatus, Result<(), u32>), u32> {
    with_transfer_pak(channel, |pak| {
        let status = pak.observe_status();
        let result = if !status.cartridge_present {
            Err(PFS_ERR_NO_GBCART)
        } else if status.cartridge_pulled {
            Err(PFS_ERR_NEW_GBCART)
        } else {
            Ok(())
        };
        Ok((status, result))
    })
}

fn power(channel: usize, enabled: bool) -> Result<bool, u32> {
    with_transfer_pak(channel, |pak| {
        let transitioned_on = enabled && !pak.enabled();
        pak.set_power(enabled);
        Ok(transitioned_on)
    })
}

fn advance_wait(cycles: u64) {
    crate::advance_virtual_time(crate::sim_time().saturating_add(cycles));
}

unsafe fn copy_from_guest(storage: fn64_runtime::RdramPtr, raw: u64, len: usize) -> Vec<u8> {
    let base = RdramAddr::from_gpr(raw);
    (0..len)
        .map(|index| unsafe {
            storage.read_u8(
                base.checked_add(u32::try_from(index).expect("Transfer Pak transfer exceeds u32"))
                    .expect("Transfer Pak guest source address overflow"),
            )
        })
        .collect()
}

unsafe fn copy_to_guest(storage: fn64_runtime::RdramPtr, raw: u64, bytes: &[u8]) {
    let base = RdramAddr::from_gpr(raw);
    for (index, &byte) in bytes.iter().enumerate() {
        unsafe {
            storage.write_u8(
                base.checked_add(u32::try_from(index).expect("Transfer Pak transfer exceeds u32"))
                    .expect("Transfer Pak guest destination address overflow"),
                byte,
            );
        }
    }
}

fn read_bus(pak: &mut fn64_runtime::TransferPak, address: u16, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    pak.write_block(0xb000, &[1; GB_BLOCK]);
    for (offset, output) in bytes.chunks_exact_mut(GB_BLOCK).enumerate() {
        let mut block = [0; GB_BLOCK];
        pak.read_game_boy_block(address + (offset * GB_BLOCK) as u16, &mut block);
        output.copy_from_slice(&block);
    }
    pak.write_block(0xb000, &[0; GB_BLOCK]);
    bytes
}

fn write_bus(pak: &mut fn64_runtime::TransferPak, address: u16, bytes: &[u8]) {
    pak.write_block(0xb000, &[1; GB_BLOCK]);
    for (offset, input) in bytes.chunks_exact(GB_BLOCK).enumerate() {
        let block: [u8; GB_BLOCK] = input.try_into().expect("exact Transfer Pak block");
        pak.write_game_boy_block(address + (offset * GB_BLOCK) as u16, &block);
    }
    pak.write_block(0xb000, &[0; GB_BLOCK]);
}

fn registration_data_valid(bytes: &[u8; GB_REGISTRATION_SIZE]) -> bool {
    let logo_hash = bytes[4..0x34]
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
        });
    let complement = bytes[0x34..0x4d]
        .iter()
        .fold(0u8, |sum, byte| sum.wrapping_sub(*byte).wrapping_sub(1));
    logo_hash == NINTENDO_LOGO_FNV1A64 && complement == bytes[0x4d]
}

/// `osGbpakInit(OSMesgQueue *mq, OSPfs *pfs, int channel) -> s32`.
///
/// # Safety
/// Guest pointers in `ctx` must address complete public ABI objects in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osGbpakInit_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let channel = ctx.r6 as usize;
    let result = with_transfer_pak(channel, |pak| {
        pak.set_power(false);
        Ok(())
    });
    if result.is_ok() {
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        let pfs = RdramAddr::from_gpr(ctx.r5);
        unsafe {
            storage.write_u32(pfs, PFS_GBPAK_INITIALIZED);
            storage.write_u32(
                pfs.checked_add(4)
                    .expect("OSPfs Transfer Pak queue field address overflow"),
                ctx.r4 as u32,
            );
            storage.write_u32(
                pfs.checked_add(8)
                    .expect("OSPfs Transfer Pak channel field address overflow"),
                channel as u32,
            );
        }
        advance_wait(INIT_WAIT_CYCLES);
    }
    set_result(ctx, result);
}

/// `osGbpakPower(OSPfs *pfs, s32 flag) -> s32`.
///
/// # Safety
/// `ctx.r4` must point to an initialized `OSPfs` in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osGbpakPower_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = gbpak_channel(storage, ctx.r4).and_then(|channel| match ctx.r5 as u32 {
        OS_GBPAK_POWER_OFF => power(channel, false).map(|_| ()),
        OS_GBPAK_POWER_ON => power(channel, true).map(|transitioned| {
            if transitioned {
                advance_wait(POWER_ON_WAIT_CYCLES);
            }
        }),
        _ => Err(PFS_ERR_INVALID),
    });
    set_result(ctx, result);
}

/// `osGbpakGetStatus(OSPfs *pfs, u8 *status) -> s32`.
///
/// # Safety
/// Guest pointers in `ctx` must be valid in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osGbpakGetStatus_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = gbpak_channel(storage, ctx.r4).and_then(|channel| {
        observe_status(channel).and_then(|(status, result)| {
            unsafe { storage.write_u8(RdramAddr::from_gpr(ctx.r5), status_byte(status)) };
            result
        })
    });
    set_result(ctx, result);
}

/// `osGbpakReadWrite(OSPfs*, u16 flag, u16 address, u8 *buffer, u16 size)`.
///
/// # Safety
/// The guest buffer and o32 argument area must cover `size` bytes.
#[no_mangle]
pub unsafe extern "C" fn osGbpakReadWrite_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let address = ctx.r6 as u16;
    let size = unsafe { read_stack_word(rdram, ctx.r29, 0x10) } as usize;
    let valid_range = address.is_multiple_of(GB_BLOCK as u16)
        && size.is_multiple_of(GB_BLOCK)
        && usize::from(address).saturating_add(size) <= 0xc000;
    let result = if !valid_range {
        Err(PFS_ERR_INVALID)
    } else {
        gbpak_channel(storage, ctx.r4).and_then(|channel| {
            with_transfer_pak(channel, |pak| {
                if !pak.has_cartridge() {
                    return Err(PFS_ERR_NO_GBCART);
                }
                if !pak.enabled() {
                    return Err(PFS_ERR_CONTRFAIL);
                }
                match ctx.r5 as u32 {
                    OS_READ => {
                        let bytes = read_bus(pak, address, size);
                        unsafe { copy_to_guest(storage, ctx.r7, &bytes) };
                        Ok(())
                    }
                    OS_WRITE => {
                        let bytes = unsafe { copy_from_guest(storage, ctx.r7, size) };
                        write_bus(pak, address, &bytes);
                        Ok(())
                    }
                    _ => Err(PFS_ERR_INVALID),
                }
            })
        })
    };
    set_result(ctx, result);
}

/// `osGbpakReadId(OSPfs *pfs, u8 *id, u8 *status) -> s32`.
///
/// # Safety
/// `id` must cover 80 bytes and `status` one byte in guest RDRAM.
#[no_mangle]
pub unsafe extern "C" fn osGbpakReadId_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = gbpak_channel(storage, ctx.r4).and_then(|channel| {
        observe_status(channel).and_then(|(status, status_result)| {
            unsafe { storage.write_u8(RdramAddr::from_gpr(ctx.r6), status_byte(status)) };
            if status_result == Err(PFS_ERR_NO_GBCART) {
                return Err(PFS_ERR_NO_GBCART);
            }
            let transitioned = power(channel, true)?;
            if transitioned {
                advance_wait(POWER_ON_WAIT_CYCLES);
            }
            with_transfer_pak(channel, |pak| {
                let bytes: [u8; GB_REGISTRATION_SIZE] = read_bus(pak, 0x0100, GB_REGISTRATION_SIZE)
                    .try_into()
                    .expect("registration read has fixed size");
                if !registration_data_valid(&bytes) {
                    return Err(PFS_ERR_CONTRFAIL);
                }
                unsafe { copy_to_guest(storage, ctx.r5, &bytes) };
                Ok(())
            })
        })
    });
    set_result(ctx, result);
}

fn regions_differ(pak: &mut fn64_runtime::TransferPak, left: u16, right: u16) -> bool {
    read_bus(pak, left, 4 * GB_BLOCK) != read_bus(pak, right, 4 * GB_BLOCK)
}

/// `osGbpakCheckConnector(OSPfs *pfs, u8 *status) -> s32`.
///
/// # Safety
/// Guest pointers in `ctx` must be valid in `rdram`.
#[no_mangle]
pub unsafe extern "C" fn osGbpakCheckConnector_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let result = gbpak_channel(storage, ctx.r4).and_then(|channel| {
        observe_status(channel).and_then(|(status, status_result)| {
            unsafe { storage.write_u8(RdramAddr::from_gpr(ctx.r5), status_byte(status)) };
            if status_result == Err(PFS_ERR_NO_GBCART) {
                return Err(PFS_ERR_NO_GBCART);
            }
            let transitioned = power(channel, true)?;
            if transitioned {
                advance_wait(POWER_ON_WAIT_CYCLES);
            }
            with_transfer_pak(channel, |pak| {
                let rom_lines = [
                    0x0000, 0x0080, 0x0100, 0x0200, 0x0400, 0x0800, 0x1000, 0x2000, 0x4000,
                ];
                if rom_lines
                    .windows(2)
                    .any(|pair| !regions_differ(pak, pair[0], pair[1]))
                {
                    return Err(PFS_ERR_CONTRFAIL);
                }
                if pak.cartridge_ram_len().unwrap_or(0) != 0 {
                    write_bus(pak, 0x0000, &[0x0a; GB_BLOCK]);
                    let ram_differs = regions_differ(pak, 0x2000, 0xa000);
                    write_bus(pak, 0x0000, &[0; GB_BLOCK]);
                    if !ram_differs {
                        return Err(PFS_ERR_CONTRFAIL);
                    }
                }
                Ok(())
            })
        })
    });
    set_result(ctx, result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    fn initialize(rdram: &mut [u8], pfs: u32) -> RecompContext {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0100;
        ctx.r5 = u64::from(0x8000_0000 | pfs);
        ctx.r6 = 0;
        unsafe { osGbpakInit_recomp(rdram.as_mut_ptr(), &mut ctx) };
        ctx
    }

    fn synthetic_rom(cartridge_type: u8, banks: usize, ram_size: u8) -> Vec<u8> {
        let mut rom = vec![0; banks * 0x4000];
        for (index, byte) in rom.iter_mut().enumerate() {
            *byte = ((index / 32) as u8)
                .wrapping_mul(37)
                .wrapping_add(index as u8)
                ^ ((index >> 8) as u8).rotate_left(3)
                ^ ((index >> 16) as u8).rotate_left(5);
        }
        rom[0x147] = cartridge_type;
        rom[0x149] = ram_size;
        rom
    }

    #[test]
    fn init_and_power_use_documented_virtual_waits_and_status_bits() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
        insert_transfer_pak_cartridge(0, synthetic_rom(0, 2, 0), None).unwrap();
        let mut rdram = vec![0; 0x1000];
        let mut ctx = initialize(&mut rdram, 0x200);
        assert_eq!(ctx.r2, 0);
        assert_eq!(crate::sim_time(), INIT_WAIT_CYCLES);

        ctx.r4 = 0x8000_0200;
        ctx.r5 = OS_GBPAK_POWER_ON as u64;
        unsafe { osGbpakPower_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        assert_eq!(crate::sim_time(), INIT_WAIT_CYCLES + POWER_ON_WAIT_CYCLES);

        ctx.r5 = 0x8000_0300;
        unsafe { osGbpakGetStatus_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
        assert_eq!(
            unsafe { storage.read_u8(RdramAddr::from_offset(0x300)) },
            OS_GBPAK_GBCART_ON | OS_GBPAK_POWER | OS_GBPAK_RSTB_DETECTION
        );

        unsafe { osGbpakGetStatus_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(
            unsafe { storage.read_u8(RdramAddr::from_offset(0x300)) },
            OS_GBPAK_GBCART_ON | OS_GBPAK_POWER
        );
    }

    #[test]
    fn high_level_and_raw_paths_share_mapper_and_persistent_ram() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
        let mut rom = synthetic_rom(0x03, 64, 3);
        for bank in 0..64 {
            rom[bank * 0x4000] = bank as u8;
        }
        insert_transfer_pak_cartridge(0, rom, None).unwrap();
        let mut rdram = vec![0; 0x2000];
        let mut ctx = initialize(&mut rdram, 0x200);
        ctx.r4 = 0x8000_0200;
        ctx.r5 = OS_GBPAK_POWER_ON as u64;
        unsafe { osGbpakPower_recomp(rdram.as_mut_ptr(), &mut ctx) };

        // MBC1 ROM bank select through the high-level write, then read bank
        // two through the same high-level bus.
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
        unsafe { storage.write_u8(RdramAddr::from_offset(0x500), 2) };
        for offset in 1..GB_BLOCK {
            unsafe { storage.write_u8(RdramAddr::from_offset(0x500 + offset as u32), 2) };
        }
        ctx.r5 = OS_WRITE as u64;
        ctx.r6 = 0x2000;
        ctx.r7 = 0x8000_0500;
        ctx.r29 = 0x8000_0800;
        unsafe { storage.write_u32(RdramAddr::from_offset(0x810), GB_BLOCK as u32) };
        unsafe { osGbpakReadWrite_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);

        ctx.r5 = OS_READ as u64;
        ctx.r6 = 0x4000;
        ctx.r7 = 0x8000_0600;
        unsafe { osGbpakReadWrite_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        assert_eq!(unsafe { storage.read_u8(RdramAddr::from_offset(0x600)) }, 2);

        with_executor(|executor| {
            let pak = executor.transfer_pak_mut(0).unwrap();
            pak.write_game_boy_block(0x0000, &[0x0a; GB_BLOCK]);
            pak.write_game_boy_block(0x6000, &[1; GB_BLOCK]);
            pak.write_game_boy_block(0x4000, &[2; GB_BLOCK]);
            pak.write_game_boy_block(0xa000, &[0x5a; GB_BLOCK]);
        });
        assert_eq!(
            with_executor(
                |executor| executor.transfer_pak(0).unwrap().cartridge_ram().unwrap()[2 * 0x2000]
            ),
            0x5a
        );
    }

    #[test]
    fn removal_is_sticky_once_and_invalid_transfers_are_rejected() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
        let rom = synthetic_rom(0, 2, 0);
        insert_transfer_pak_cartridge(0, rom.clone(), None).unwrap();
        let mut rdram = vec![0; 0x1000];
        let mut ctx = initialize(&mut rdram, 0x200);
        with_executor(|executor| {
            assert!(executor
                .transfer_pak_mut(0)
                .unwrap()
                .remove_cartridge()
                .is_some());
        });
        insert_transfer_pak_cartridge(0, rom, None).unwrap();
        ctx.r4 = 0x8000_0200;
        ctx.r5 = 0x8000_0300;
        unsafe { osGbpakGetStatus_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, PFS_ERR_NEW_GBCART as u64);
        unsafe { osGbpakGetStatus_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);

        ctx.r5 = OS_READ as u64;
        ctx.r6 = 1;
        ctx.r7 = 0x8000_0400;
        ctx.r29 = 0x8000_0800;
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
        unsafe { storage.write_u32(RdramAddr::from_offset(0x810), GB_BLOCK as u32) };
        unsafe { osGbpakReadWrite_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, PFS_ERR_INVALID as u64);
    }

    #[test]
    fn read_id_rejects_a_bad_registration_header_and_connector_probes_aliasing() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
        let patterned = synthetic_rom(0, 2, 0);
        insert_transfer_pak_cartridge(0, patterned, None).unwrap();
        let mut rdram = vec![0; 0x1000];
        let mut ctx = initialize(&mut rdram, 0x200);
        ctx.r4 = 0x8000_0200;
        ctx.r5 = 0x8000_0300;
        unsafe { osGbpakCheckConnector_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);

        ctx.r5 = 0x8000_0400;
        ctx.r6 = 0x8000_0500;
        unsafe { osGbpakReadId_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, PFS_ERR_CONTRFAIL as u64);

        let mut flat = vec![0xff; 2 * 0x4000];
        flat[0x147] = 0;
        flat[0x149] = 0;
        insert_transfer_pak_cartridge(0, flat, None).unwrap();
        ctx.r5 = 0x8000_0300;
        unsafe { osGbpakCheckConnector_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, PFS_ERR_CONTRFAIL as u64);
    }
}
