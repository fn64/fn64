//! EEPROM and FlashRAM ABI adapters over the runtime's single save backing
//! store.
//!
//! Signatures, 8-byte block addressing, type codes, and return values come
//! from the public libultra EEPROM Manager manual. The queue argument is part
//! of libultra's synchronous SI protocol but carries no completion visible to
//! the caller. Writes latch immediately, program the backing store at a
//! deterministic guest-cycle deadline, and share that busy state with raw
//! Joybus commands. Flash command sequencing and identity follow the public
//! N64 FlashRAM Programming Manual; [`FlashEvidenceSnapshot`] is the exhaustive
//! owner-local view of the ABI fields that can change a later Flash command.

use super::*;
use crate::pi::with_pi_dma;

const EEPROM_TYPE_4K: i32 = 1;
const EEPROM_TYPE_16K: i32 = 2;
const CONT_NO_RESPONSE_ERROR: i32 = 8;
const EEPROM_BLOCK_SIZE: usize = fn64_runtime::save::EEPROM_BLOCK_SIZE;
const FLASH_PAGE_SIZE: usize = fn64_runtime::save::FLASH_PAGE_SIZE;
const FLASH_SECTOR_SIZE: usize = fn64_runtime::save::FLASH_SECTOR_SIZE;
const FLASH_PAGE_COUNT: usize = fn64_runtime::SaveType::FlashRam.byte_len() / FLASH_PAGE_SIZE;

const FLASH_TYPE_1MBIT: u32 = 0x1111_8001;
const FLASH_MAKER_MACRONIX_C: u32 = 0x00C2_001E;
const FLASH_STATUS_READY: u8 = 0x80;
const FLASH_STATUS_ERASE_ERROR_BIT: u8 = 0x20;
const FLASH_STATUS_WRITE_ERROR_BIT: u8 = 0x10;
const FLASH_STATUS_ERASE_BUSY: i32 = 2;
const FLASH_STATUS_ERASE_OK: i32 = 0;
const FLASH_STATUS_ERASE_ERROR: i32 = -1;

const FLASH_DEVICE_TYPE: u8 = 8;
const FLASH_PI_LATENCY: u8 = 5;
const FLASH_PI_PAGE_SIZE: u8 = 0x0F;
const FLASH_PI_RELEASE: u8 = 2;
const FLASH_PI_PULSE: u8 = 0x0C;
const FLASH_PI_DOMAIN: u8 = 1;
const FLASH_KSEG1_BASE: u32 = 0xA800_0000;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FlashIdentity {
    pub flash_type: u32,
    pub flash_maker: u32,
}

/// Owned, read-only evidence for every future-affecting field in the ABI's
/// FlashRAM command sequencer. Guest pointers and the append-only save
/// operation log are deliberately absent: neither changes a later Flash
/// command's result.
///
/// This is an ABI-owner snapshot only. Release schemas may embed it later,
/// but acquiring it neither consumes a staged page nor acknowledges an erase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlashEvidenceSnapshot {
    pub write_buffer: Option<[u8; FLASH_PAGE_SIZE]>,
    pub erase_complete: bool,
    pub status: u8,
    pub identity: FlashIdentity,
}

pub(crate) struct FlashState {
    write_buffer: Option<[u8; FLASH_PAGE_SIZE]>,
    erase_complete: bool,
    status: u8,
    identity: FlashIdentity,
}

impl Default for FlashState {
    fn default() -> Self {
        Self {
            write_buffer: None,
            erase_complete: false,
            status: FLASH_STATUS_READY,
            identity: FlashIdentity {
                flash_type: FLASH_TYPE_1MBIT,
                flash_maker: FLASH_MAKER_MACRONIX_C,
            },
        }
    }
}

/// Host-visible outcome for the most recent through-erase operation.
///
/// The Macronix MX29L-family Status Register table defines DQ7 as ready/busy
/// and DQ5 as erase failure; the public `<PR/os_flash.h>` maps the three
/// `osFlashCheckEraseEnd` results to 2, 0, and -1. Fn64's in-memory device
/// completes erases before the initiating shim returns, but a host backed by a
/// fallible or timed physical device can publish the other documented outcomes
/// through this typed seam.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FlashEraseStatus {
    Busy,
    Succeeded,
    Failed,
}

/// Publish the status of a host-managed through-erase operation.
pub fn set_flash_erase_status(status: FlashEraseStatus) {
    with_host(|host| match status {
        FlashEraseStatus::Busy => {
            host.flash.erase_complete = false;
            host.flash.status = 0;
        }
        FlashEraseStatus::Succeeded => {
            host.flash.erase_complete = true;
            host.flash.status = FLASH_STATUS_READY;
        }
        FlashEraseStatus::Failed => {
            host.flash.erase_complete = true;
            host.flash.status = FLASH_STATUS_READY | FLASH_STATUS_ERASE_ERROR_BIT;
        }
    });
}

impl FlashState {
    pub(crate) fn evidence_snapshot(&self) -> FlashEvidenceSnapshot {
        FlashEvidenceSnapshot {
            write_buffer: self.write_buffer,
            erase_complete: self.erase_complete,
            status: self.status,
            identity: self.identity,
        }
    }
}

/// Snapshot the complete ABI-owned FlashRAM command state without mutating
/// it. This intentionally does not aggregate the snapshot into a release
/// artifact; callers receive the typed owner-local evidence directly.
pub fn flash_evidence_snapshot() -> FlashEvidenceSnapshot {
    with_host(|host| host.flash.evidence_snapshot())
}

fn set_result(ctx: &mut RecompContext, result: i32) {
    ctx.r2 = result as i64 as u64;
}

fn eeprom_type(kind: fn64_runtime::EepromKind) -> i32 {
    match kind {
        fn64_runtime::EepromKind::Eeprom4k => EEPROM_TYPE_4K,
        fn64_runtime::EepromKind::Eeprom16k => EEPROM_TYPE_16K,
    }
}

fn checked_eeprom_range(device_len: usize, block: u8, nbytes: usize) -> Option<usize> {
    if !nbytes.is_multiple_of(EEPROM_BLOCK_SIZE) {
        return None;
    }
    let offset = usize::from(block).checked_mul(EEPROM_BLOCK_SIZE)?;
    let end = offset.checked_add(nbytes)?;
    (end <= device_len).then_some(offset)
}

unsafe fn copy_from_guest(storage: fn64_runtime::RdramPtr, addr: RdramAddr, len: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(len);
    for index in 0..len {
        let offset = u32::try_from(index).expect("EEPROM transfer length exceeds u32");
        bytes.push(unsafe {
            storage.read_u8(
                addr.checked_add(offset)
                    .expect("EEPROM guest source address overflow"),
            )
        });
    }
    bytes
}

unsafe fn copy_to_guest(storage: fn64_runtime::RdramPtr, addr: RdramAddr, bytes: &[u8]) {
    for (index, &byte) in bytes.iter().enumerate() {
        let offset = u32::try_from(index).expect("EEPROM transfer length exceeds u32");
        unsafe {
            storage.write_u8(
                addr.checked_add(offset)
                    .expect("EEPROM guest destination address overflow"),
                byte,
            );
        }
    }
}

/// `osEepromProbe(OSMesgQueue *mq) -> s32`. The public type codes are 1 for
/// 4-Kbit EEPROM and 2 for 16-Kbit EEPROM; any other installed save protocol
/// reports no EEPROM rather than masquerading as one.
///
/// # Safety
/// Same raw guest-context contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osEepromProbe_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let now = Cycles::new(crate::sim_time());
    let kind = with_pi_dma("osEepromProbe_recomp", |dma| {
        dma.eeprom_status(now)
            .map_or(0, |status| eeprom_type(status.kind))
    });
    set_result(unsafe { &mut *ctx }, kind);
}

fn eeprom_device_len(shim: &str) -> Result<usize, i32> {
    with_pi_dma(shim, |dma| {
        dma.save_len()
            .filter(|len| fn64_runtime::EepromKind::from_byte_len(*len).is_some())
            .ok_or(CONT_NO_RESPONSE_ERROR)
    })
}

/// Libultra polls a prior programming operation before issuing the next
/// high-level EEPROM command. Advancing the one guest clock to the typed
/// deadline reproduces that synchronous wait without host wall time.
fn wait_for_eeprom_ready(shim: &str) -> Result<(), i32> {
    loop {
        let now = Cycles::new(crate::sim_time());
        let state = with_pi_dma(shim, |dma| {
            let present = dma.eeprom_status(now).is_some();
            (present, dma.eeprom_busy_until(now))
        });
        if !state.0 {
            return Err(CONT_NO_RESPONSE_ERROR);
        }
        let Some(ready_at) = state.1 else {
            return Ok(());
        };
        crate::advance_virtual_time(ready_at.get());
    }
}

fn eeprom_read(rdram: *mut u8, ctx: &mut RecompContext, nbytes: usize, shim: &str) {
    let block = ctx.r5 as u8;
    let destination = RdramAddr::from_gpr(ctx.r6);
    let result = (|| {
        let device_len = eeprom_device_len(shim)?;
        checked_eeprom_range(device_len, block, nbytes).ok_or(-1)?;
        wait_for_eeprom_ready(shim)?;
        let now = Cycles::new(crate::sim_time());
        with_pi_dma(shim, |dma| {
            let mut bytes = Vec::with_capacity(nbytes);
            for block_index in 0..nbytes / EEPROM_BLOCK_SIZE {
                let physical_block = u8::try_from(usize::from(block) + block_index)
                    .expect("validated EEPROM read block exceeds u8");
                match dma.eeprom_read_block(now, physical_block) {
                    Ok(data) => bytes.extend_from_slice(&data),
                    Err(fn64_runtime::EepromError::NoDevice) => {
                        return Err(CONT_NO_RESPONSE_ERROR);
                    }
                    Err(fn64_runtime::EepromError::Busy { ready_at }) => panic!(
                        "{shim}: EEPROM remained busy through its guest-cycle deadline {}",
                        ready_at.get()
                    ),
                }
            }
            Ok(bytes)
        })
    })();

    match result {
        Ok(bytes) => {
            let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
            unsafe { copy_to_guest(storage, destination, &bytes) };
            set_result(ctx, 0);
        }
        Err(code) => set_result(ctx, code),
    }
}

fn eeprom_write(
    rdram: *mut u8,
    ctx: &mut RecompContext,
    nbytes: usize,
    wait_after_each_block: bool,
    shim: &str,
) {
    let block = ctx.r5 as u8;
    let source = RdramAddr::from_gpr(ctx.r6);
    let result = (|| {
        let device_len = eeprom_device_len(shim)?;
        checked_eeprom_range(device_len, block, nbytes).ok_or(-1)?;
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        let bytes = unsafe { copy_from_guest(storage, source, nbytes) };
        for (block_index, chunk) in bytes.chunks_exact(EEPROM_BLOCK_SIZE).enumerate() {
            wait_for_eeprom_ready(shim)?;
            let physical_block = u8::try_from(usize::from(block) + block_index)
                .expect("validated EEPROM write block exceeds u8");
            let data: [u8; EEPROM_BLOCK_SIZE] = chunk
                .try_into()
                .expect("EEPROM chunks are exactly one physical block");
            let now = Cycles::new(crate::sim_time());
            let deadline = with_pi_dma(shim, |dma| {
                dma.start_eeprom_write(now, physical_block, data)
            });
            match deadline {
                Ok(deadline) if wait_after_each_block => {
                    crate::advance_virtual_time(deadline.get());
                    // LongWrite's documented per-block wait is also the
                    // authoritative programming boundary. Force the lazy
                    // backing-store transition at that exact guest deadline
                    // before release evidence can call the write committed.
                    with_pi_dma(shim, |dma| dma.advance_eeprom_to(deadline));
                }
                Ok(_) => {}
                Err(fn64_runtime::EepromError::NoDevice) => {
                    return Err(CONT_NO_RESPONSE_ERROR);
                }
                Err(fn64_runtime::EepromError::Busy { ready_at }) => panic!(
                    "{shim}: EEPROM rejected a write after waiting through guest cycle {}",
                    ready_at.get()
                ),
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => set_result(ctx, 0),
        Err(code) => set_result(ctx, code),
    }
}

/// `osEepromRead(OSMesgQueue *mq, u8 address, u8 *buffer) -> s32`.
///
/// # Safety
/// Same raw guest-memory contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osEepromRead_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    eeprom_read(
        rdram,
        unsafe { &mut *ctx },
        EEPROM_BLOCK_SIZE,
        "osEepromRead_recomp",
    );
}

/// `osEepromWrite(OSMesgQueue *mq, u8 address, u8 *buffer) -> s32`.
///
/// # Safety
/// Same raw guest-memory contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osEepromWrite_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    eeprom_write(
        rdram,
        unsafe { &mut *ctx },
        EEPROM_BLOCK_SIZE,
        false,
        "osEepromWrite_recomp",
    );
}

/// `osEepromLongRead(OSMesgQueue *mq, u8 address, u8 *buffer, int nbytes)`.
/// `nbytes` must be a multiple of the public 8-byte EEPROM block size.
///
/// # Safety
/// Same raw guest-memory contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osEepromLongRead_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    eeprom_read(
        rdram,
        ctx,
        ctx.r7 as u32 as usize,
        "osEepromLongRead_recomp",
    );
}

/// `osEepromLongWrite(OSMesgQueue *mq, u8 address, u8 *buffer, int nbytes)`.
///
/// # Safety
/// Same raw guest-memory contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osEepromLongWrite_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    eeprom_write(
        rdram,
        ctx,
        ctx.r7 as u32 as usize,
        true,
        "osEepromLongWrite_recomp",
    );
}

/// Register the guest-owned BSS address used for FlashRAM's `OSPiHandle`.
/// Keeping this separate from the cartridge-ROM handle prevents callers from
/// observing a fabricated handle alias across distinct PI domains.
pub fn set_flash_handle_vram(vram: u32) {
    assert!(
        (0x8000_0000..0xC000_0000).contains(&vram) && vram.is_multiple_of(4),
        "Flash OSPiHandle must be an aligned KSEG0/KSEG1 guest address, got {vram:#010x}"
    );
    with_host(|host| host.flash_handle_vram = Some(vram));
}

/// Override the physical Flash ID returned to the guest. The default is the
/// public programming manual's currently-used Macronix C-version 1-Mbit part.
pub fn set_flash_identity(identity: FlashIdentity) {
    with_host(|host| host.flash.identity = identity);
}

fn require_flash_len(shim: &str, dma: &PiDma<InMemoryRom>) -> usize {
    let len = dma
        .save_len()
        .unwrap_or_else(|| panic!("{shim}: no FlashRAM save backing store is installed"));
    assert_eq!(
        len,
        fn64_runtime::SaveType::FlashRam.byte_len(),
        "{shim}: installed save is {len} bytes, not a 128 KiB FlashRAM"
    );
    len
}

fn post_flash_completion(queue: u32) {
    if queue != 0 {
        with_executor(|exec| {
            exec.inject_event(ExternalEvent::DirectPost {
                queue_addr: RdramAddr::from_gpr(queue as u64),
                msg: 0,
            });
        });
    }
}

/// `osFlashInit(void) -> OSPiHandle*`. Public `<PR/os_flash.h>` fixes the
/// handle's device type, PI timing, and physical start address; the public
/// `OSPiHandle` layout represents that start in uncached KSEG1 form. Its storage
/// address is nevertheless a libultra BSS symbol in each linked ROM, so the
/// host must register that address rather than fn64 guessing a writable hole
/// in guest memory.
///
/// # Safety
/// Same guest-context contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashInit_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    with_pi_dma("osFlashInit_recomp", |dma| {
        require_flash_len("osFlashInit_recomp", dma);
    });
    let handle = with_host(|host| {
        host.flash_handle_vram.unwrap_or_else(|| {
            panic!(
                "osFlashInit_recomp: no guest Flash OSPiHandle address registered -- call \
                 fn64_abi::set_flash_handle_vram before initialization"
            )
        })
    });
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let base = RdramAddr::from_gpr(handle as u64);
    unsafe {
        // Public OSPiHandle prefix through baseAddress. Transfer state begins
        // after byte 16 and is not initialized by the Flash acquisition API.
        storage.write_u32(base, 0);
        storage.write_u8(base.checked_add(4).unwrap(), FLASH_DEVICE_TYPE);
        storage.write_u8(base.checked_add(5).unwrap(), FLASH_PI_LATENCY);
        storage.write_u8(base.checked_add(6).unwrap(), FLASH_PI_PAGE_SIZE);
        storage.write_u8(base.checked_add(7).unwrap(), FLASH_PI_RELEASE);
        storage.write_u8(base.checked_add(8).unwrap(), FLASH_PI_PULSE);
        storage.write_u8(base.checked_add(9).unwrap(), FLASH_PI_DOMAIN);
        storage.write_u16(base.checked_add(10).unwrap(), 0);
        storage.write_u32(base.checked_add(12).unwrap(), FLASH_KSEG1_BASE);
    }
    unsafe { &mut *ctx }.r2 = handle as i32 as u64;
}

/// `osFlashReadStatus(u8 *flash_status)`. The Macronix MX29L-family Status
/// Register table reports DQ7 set when ready and clear while busy; DQ5 and DQ4
/// retain erase and write failures until `osFlashClearStatus`. Fn64's in-memory
/// mutations complete synchronously, so guest code normally observes ready
/// immediately.
///
/// # Safety
/// Same raw guest-memory contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashReadStatus_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    with_pi_dma("osFlashReadStatus_recomp", |dma| {
        require_flash_len("osFlashReadStatus_recomp", dma);
    });
    let destination = RdramAddr::from_gpr(unsafe { &*ctx }.r4);
    let status = with_host(|host| host.flash.status);
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    unsafe { storage.write_u8(destination, status) };
}

/// `osFlashReadId(u32 *flash_type, u32 *flash_maker)`. The default values are
/// the public programming manual's 1-Mbit type and Macronix C-version IDs;
/// hosts emulating another documented chip can install a different identity.
///
/// # Safety
/// Same raw guest-memory contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashReadId_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    with_pi_dma("osFlashReadId_recomp", |dma| {
        require_flash_len("osFlashReadId_recomp", dma);
    });
    let ctx = unsafe { &*ctx };
    let type_destination = RdramAddr::from_gpr(ctx.r4);
    let maker_destination = RdramAddr::from_gpr(ctx.r5);
    let identity = with_host(|host| host.flash.identity);
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    unsafe {
        storage.write_u32(type_destination, identity.flash_type);
        storage.write_u32(maker_destination, identity.flash_maker);
    }
}

/// `osFlashClearStatus(void)`. Clearing the retained DQ5/DQ4 failure bits
/// leaves DQ7 set because the synchronous in-memory device is ready.
///
/// # Safety
/// Same guest-context contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashClearStatus_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    with_host(|host| {
        host.flash.erase_complete = false;
        host.flash.status = FLASH_STATUS_READY;
    });
}

fn flash_all_erase(shim: &str) {
    let len = with_pi_dma(shim, |dma| {
        let len = require_flash_len(shim, dma);
        dma.save_erase(0, len);
        len
    });
    crate::record_save_operation(
        fn64_runtime::SaveType::FlashRam,
        fn64_runtime::SaveOperationKind::Erase,
        0,
        len,
    );
    with_host(|host| {
        host.flash.erase_complete = false;
        host.flash.status = FLASH_STATUS_READY;
    });
}

fn flash_sector_erase(shim: &str, page: u32) -> i32 {
    let page = page as usize;
    if page >= FLASH_PAGE_COUNT {
        return -1;
    }
    let sector = page / (FLASH_SECTOR_SIZE / FLASH_PAGE_SIZE);
    with_pi_dma(shim, |dma| {
        require_flash_len(shim, dma);
        dma.save_erase(sector * FLASH_SECTOR_SIZE, FLASH_SECTOR_SIZE);
    });
    crate::record_save_operation(
        fn64_runtime::SaveType::FlashRam,
        fn64_runtime::SaveOperationKind::Erase,
        sector * FLASH_SECTOR_SIZE,
        FLASH_SECTOR_SIZE,
    );
    with_host(|host| {
        host.flash.erase_complete = false;
        host.flash.status = FLASH_STATUS_READY;
    });
    0
}

/// `osFlashAllErase(void) -> s32`.
///
/// # Safety
/// Same guest-context contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashAllErase_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    flash_all_erase("osFlashAllErase_recomp");
    set_result(unsafe { &mut *ctx }, 0);
}

/// `osFlashAllEraseThrough(void)`. The public API defers its status check;
/// fn64 commits deterministic storage synchronously and records a completed
/// operation for `osFlashCheckEraseEnd`.
///
/// # Safety
/// Same guest-context contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashAllEraseThrough_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    flash_all_erase("osFlashAllEraseThrough_recomp");
    set_flash_erase_status(FlashEraseStatus::Succeeded);
}

/// `osFlashSectorErase(u32 page_num) -> s32`; page numbers are converted to
/// their containing 128-page erase sector per the public manual.
///
/// # Safety
/// Same guest-context contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashSectorErase_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let result = flash_sector_erase("osFlashSectorErase_recomp", ctx.r4 as u32);
    set_result(ctx, result);
}

/// `osFlashSectorEraseThrough(u32 page_num)`.
///
/// # Safety
/// Same guest-context contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashSectorEraseThrough_recomp(
    _rdram: *mut u8,
    ctx: *mut RecompContext,
) {
    let result = flash_sector_erase(
        "osFlashSectorEraseThrough_recomp",
        unsafe { &*ctx }.r4 as u32,
    );
    assert_eq!(
        result, 0,
        "osFlashSectorEraseThrough_recomp: page out of range"
    );
    set_flash_erase_status(FlashEraseStatus::Succeeded);
}

/// `osFlashCheckEraseEnd(void) -> s32`. Public `<PR/os_flash.h>` defines 2,
/// -1, and 0 for erase-busy, erase-error, and erase-ok; the Programming Manual
/// section 28.2.9 assigns those three outcomes to this function. The in-memory
/// backing store transitions directly to success; [`set_flash_erase_status`]
/// preserves the other documented observations for timed or fallible hosts
/// without introducing an erase-duration policy here.
///
/// # Safety
/// Same guest-context contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashCheckEraseEnd_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let (complete, status) = with_host(|host| (host.flash.erase_complete, host.flash.status));
    let result = if !complete || status & FLASH_STATUS_READY == 0 {
        FLASH_STATUS_ERASE_BUSY
    } else if status & FLASH_STATUS_ERASE_ERROR_BIT != 0 {
        FLASH_STATUS_ERASE_ERROR
    } else {
        FLASH_STATUS_ERASE_OK
    };
    set_result(unsafe { &mut *ctx }, result);
}

/// `osFlashWriteBuffer(OSIoMesg *mb, s32 priority, void *dramAddr,
/// OSMesgQueue *mq) -> s32`. Exactly one 128-byte page is staged.
///
/// # Safety
/// Same raw guest-memory contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashWriteBuffer_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    with_pi_dma("osFlashWriteBuffer_recomp", |dma| {
        require_flash_len("osFlashWriteBuffer_recomp", dma);
    });
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let source = RdramAddr::from_gpr(ctx.r6);
    let bytes = unsafe { copy_from_guest(storage, source, FLASH_PAGE_SIZE) };
    let page: [u8; FLASH_PAGE_SIZE] = bytes
        .try_into()
        .expect("fixed Flash write-buffer length must fit one page");
    with_host(|host| host.flash.write_buffer = Some(page));
    post_flash_completion(ctx.r7 as u32);
    set_result(ctx, 0);
}

/// `osFlashWriteArray(u32 page_num) -> s32` commits the previously staged
/// write buffer to one Flash page.
///
/// # Safety
/// Same guest-context contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashWriteArray_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let page = ctx.r4 as u32 as usize;
    if page >= FLASH_PAGE_COUNT {
        set_result(ctx, -1);
        return;
    }
    let buffer = with_host(|host| {
        host.flash
            .write_buffer
            .take()
            .expect("osFlashWriteArray_recomp: no page staged by osFlashWriteBuffer")
    });
    with_pi_dma("osFlashWriteArray_recomp", |dma| {
        require_flash_len("osFlashWriteArray_recomp", dma);
        dma.save_write_from(page * FLASH_PAGE_SIZE, &buffer);
    });
    crate::record_save_operation(
        fn64_runtime::SaveType::FlashRam,
        fn64_runtime::SaveOperationKind::Write,
        page * FLASH_PAGE_SIZE,
        FLASH_PAGE_SIZE,
    );
    with_host(|host| {
        host.flash.status &= !FLASH_STATUS_WRITE_ERROR_BIT;
        host.flash.status |= FLASH_STATUS_READY;
    });
    set_result(ctx, 0);
}

/// `osFlashReadArray(OSIoMesg *mb, s32 priority, u32 page_num,
/// void *dramAddr, u32 n_pages, OSMesgQueue *mq) -> s32`. The fifth and
/// sixth o32 arguments are read from `sp+0x10` and `sp+0x14`.
///
/// # Safety
/// Same raw guest-memory contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashReadArray_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let first_page = ctx.r6 as u32 as usize;
    let destination = RdramAddr::from_gpr(ctx.r7);
    let page_count = unsafe { read_stack_word(rdram, ctx.r29, 0x10) } as usize;
    let queue = unsafe { read_stack_word(rdram, ctx.r29, 0x14) };
    let Some(end_page) = first_page.checked_add(page_count) else {
        set_result(ctx, -1);
        return;
    };
    if end_page > FLASH_PAGE_COUNT {
        set_result(ctx, -1);
        return;
    }
    let len = page_count
        .checked_mul(FLASH_PAGE_SIZE)
        .expect("Flash read length overflow");
    let bytes = with_pi_dma("osFlashReadArray_recomp", |dma| {
        require_flash_len("osFlashReadArray_recomp", dma);
        let mut bytes = vec![0; len];
        dma.save_read_into(first_page * FLASH_PAGE_SIZE, &mut bytes);
        bytes
    });
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    unsafe { copy_to_guest(storage, destination, &bytes) };
    if len > 0 {
        crate::record_save_operation(
            fn64_runtime::SaveType::FlashRam,
            fn64_runtime::SaveOperationKind::Read,
            first_page * FLASH_PAGE_SIZE,
            len,
        );
    }
    post_flash_completion(queue);
    set_result(ctx, 0);
}

/// `osFlashChange(u32 flash_num)`. One backing store represents one physical
/// chip; chip zero is selectable and additional chips trap until the host API
/// can install independent stores for them.
///
/// # Safety
/// Same guest-context contract as the other ABI shims.
#[no_mangle]
pub unsafe extern "C" fn osFlashChange_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let flash_num = unsafe { &*ctx }.r4 as u32;
    assert!(
        flash_num <= 3,
        "osFlashChange_recomp: flash chip selector {flash_num} exceeds the documented 0..=3 range"
    );
    assert_eq!(
        flash_num, 0,
        "osFlashChange_recomp: flash chip {flash_num} requested, but only chip 0 is installed"
    );
    with_pi_dma("osFlashChange_recomp", |dma| {
        require_flash_len("osFlashChange_recomp", dma);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi::{load_rom, set_save};
    use crate::test_support::ctx_zeroed;

    fn install_eeprom(kind: fn64_runtime::SaveType) {
        load_rom(vec![0; 0x100]);
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            kind,
        )));
    }

    #[test]
    fn probe_reports_public_eeprom_type_codes() {
        install_eeprom(fn64_runtime::SaveType::Eeprom16k);
        let mut ctx = ctx_zeroed();
        unsafe { osEepromProbe_recomp(std::ptr::null_mut(), &mut ctx) };
        assert_eq!(ctx.r2, EEPROM_TYPE_16K as u64);
    }

    #[test]
    fn single_write_returns_while_busy_and_read_waits_for_exact_commit() {
        install_eeprom(fn64_runtime::SaveType::Eeprom4k);
        let mut rdram = vec![0u8; 0x100];
        let payload = [0x81, 0x72, 0x63, 0x54, 0x45, 0x36, 0x27, 0x18];
        for (index, byte) in payload.iter().copied().enumerate() {
            rdram[(0x20 + index) ^ 3] = byte;
        }

        let mut write = ctx_zeroed();
        write.r5 = 2;
        write.r6 = 0x8000_0020;
        unsafe { osEepromWrite_recomp(rdram.as_mut_ptr(), &mut write) };
        assert_eq!(write.r2, 0);
        assert_eq!(crate::sim_time(), 0);
        assert!(
            crate::copy_save_operations().is_empty(),
            "an accepted EEPROM write is not committed save evidence"
        );
        let (busy, stored_before) = with_pi_dma("timed EEPROM test", |dma| {
            let busy = dma.eeprom_status(Cycles::ZERO).unwrap().busy;
            let mut stored = [0; EEPROM_BLOCK_SIZE];
            dma.save_read_into(2 * EEPROM_BLOCK_SIZE, &mut stored);
            (busy, stored)
        });
        assert!(busy);
        assert_eq!(stored_before, [0xFF; EEPROM_BLOCK_SIZE]);

        let mut read = ctx_zeroed();
        read.r5 = 2;
        read.r6 = 0x8000_0040;
        unsafe { osEepromRead_recomp(rdram.as_mut_ptr(), &mut read) };
        assert_eq!(read.r2, 0);
        assert_eq!(crate::sim_time(), fn64_runtime::EEPROM_WRITE_CYCLES.get());
        for (index, expected) in payload.iter().copied().enumerate() {
            assert_eq!(rdram[(0x40 + index) ^ 3], expected);
        }
        assert_eq!(
            crate::copy_save_operations(),
            vec![
                fn64_runtime::SaveOperationEvent {
                    at: fn64_runtime::EEPROM_WRITE_CYCLES,
                    device: fn64_runtime::SaveType::Eeprom4k,
                    operation: fn64_runtime::SaveOperationKind::Write,
                    offset: 2 * EEPROM_BLOCK_SIZE as u32,
                    len: EEPROM_BLOCK_SIZE as u32,
                },
                fn64_runtime::SaveOperationEvent {
                    at: fn64_runtime::EEPROM_WRITE_CYCLES,
                    device: fn64_runtime::SaveType::Eeprom4k,
                    operation: fn64_runtime::SaveOperationKind::Read,
                    offset: 2 * EEPROM_BLOCK_SIZE as u32,
                    len: EEPROM_BLOCK_SIZE as u32,
                },
            ]
        );
    }

    #[test]
    fn long_write_and_read_round_trip_logical_guest_bytes() {
        install_eeprom(fn64_runtime::SaveType::Eeprom4k);
        let mut rdram = vec![0u8; 0x200];
        let payload = [
            0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87, 0x98, 0xA9, 0xBA, 0xCB, 0xDC, 0xED,
            0xFE, 0x0F,
        ];
        for (index, byte) in payload.iter().copied().enumerate() {
            rdram[(0x40 + index) ^ 3] = byte;
        }

        let mut write_ctx = ctx_zeroed();
        write_ctx.r5 = 3;
        write_ctx.r6 = 0x8000_0040;
        write_ctx.r7 = payload.len() as u64;
        unsafe { osEepromLongWrite_recomp(rdram.as_mut_ptr(), &mut write_ctx) };
        assert_eq!(write_ctx.r2, 0);
        assert_eq!(
            crate::sim_time(),
            fn64_runtime::EEPROM_WRITE_CYCLES.get() * 2
        );
        let operations = crate::copy_save_operations();
        assert_eq!(operations.len(), 2);
        assert!(operations
            .iter()
            .all(|event| event.operation == fn64_runtime::SaveOperationKind::Write));
        assert_eq!(operations[0].offset, 3 * EEPROM_BLOCK_SIZE as u32);
        assert_eq!(operations[1].offset, 4 * EEPROM_BLOCK_SIZE as u32);

        let mut read_ctx = ctx_zeroed();
        read_ctx.r5 = 3;
        read_ctx.r6 = 0x8000_0080;
        read_ctx.r7 = payload.len() as u64;
        unsafe { osEepromLongRead_recomp(rdram.as_mut_ptr(), &mut read_ctx) };
        assert_eq!(read_ctx.r2, 0);
        for (index, expected) in payload.iter().copied().enumerate() {
            assert_eq!(rdram[(0x80 + index) ^ 3], expected);
        }
    }

    #[test]
    fn invalid_block_returns_minus_one_without_touching_guest_memory() {
        install_eeprom(fn64_runtime::SaveType::Eeprom4k);
        let mut rdram = vec![0xA5u8; 0x100];
        let mut ctx = ctx_zeroed();
        ctx.r5 = 64;
        ctx.r6 = 0x8000_0040;
        unsafe { osEepromRead_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, u64::MAX);
        assert!(rdram.iter().all(|&byte| byte == 0xA5));
    }

    fn install_flash() {
        load_rom(vec![0; 0x100]);
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::FlashRam,
        )));
        set_flash_handle_vram(0x8000_1200);
        with_host(|host| host.flash = FlashState::default());
    }

    #[test]
    fn flash_evidence_snapshot_covers_each_field_independently() {
        install_flash();
        let baseline = flash_evidence_snapshot();
        assert_eq!(
            baseline,
            FlashEvidenceSnapshot {
                write_buffer: None,
                erase_complete: false,
                status: FLASH_STATUS_READY,
                identity: FlashIdentity {
                    flash_type: FLASH_TYPE_1MBIT,
                    flash_maker: FLASH_MAKER_MACRONIX_C,
                },
            }
        );

        let staged = [0x5A; FLASH_PAGE_SIZE];
        with_host(|host| host.flash.write_buffer = Some(staged));
        assert_eq!(
            flash_evidence_snapshot(),
            FlashEvidenceSnapshot {
                write_buffer: Some(staged),
                ..baseline.clone()
            }
        );

        with_host(|host| {
            host.flash = FlashState::default();
            host.flash.erase_complete = true;
        });
        assert_eq!(
            flash_evidence_snapshot(),
            FlashEvidenceSnapshot {
                erase_complete: true,
                ..baseline.clone()
            }
        );

        with_host(|host| {
            host.flash = FlashState::default();
            host.flash.status = 0x81;
        });
        assert_eq!(
            flash_evidence_snapshot(),
            FlashEvidenceSnapshot {
                status: 0x81,
                ..baseline.clone()
            }
        );

        let identity = FlashIdentity {
            flash_type: 0x0123_4567,
            flash_maker: 0x89AB_CDEF,
        };
        with_host(|host| {
            host.flash = FlashState::default();
            host.flash.identity = identity;
        });
        assert_eq!(
            flash_evidence_snapshot(),
            FlashEvidenceSnapshot {
                identity,
                ..baseline
            }
        );
    }

    #[test]
    fn flash_evidence_snapshot_does_not_consume_or_alias_staged_page() {
        install_flash();
        let staged = std::array::from_fn(|index| index as u8);
        with_host(|host| host.flash.write_buffer = Some(staged));

        let mut observed = flash_evidence_snapshot();
        observed.write_buffer.as_mut().unwrap()[0] = 0xFF;
        assert_eq!(flash_evidence_snapshot().write_buffer, Some(staged));

        let mut commit = ctx_zeroed();
        commit.r4 = 9;
        unsafe { osFlashWriteArray_recomp(std::ptr::null_mut(), &mut commit) };
        assert_eq!(commit.r2, 0);
        assert_eq!(flash_evidence_snapshot().write_buffer, None);

        let stored = with_pi_dma("Flash evidence staged-page test", |dma| {
            let mut stored = [0; FLASH_PAGE_SIZE];
            dma.save_read_into(9 * FLASH_PAGE_SIZE, &mut stored);
            stored
        });
        assert_eq!(stored, staged);
    }

    #[test]
    fn flash_erase_latch_predicts_the_next_completion_check() {
        install_flash();
        let mut check = ctx_zeroed();
        unsafe { osFlashCheckEraseEnd_recomp(std::ptr::null_mut(), &mut check) };
        assert_eq!(check.r2, FLASH_STATUS_ERASE_BUSY as u64);

        set_flash_erase_status(FlashEraseStatus::Succeeded);
        assert!(flash_evidence_snapshot().erase_complete);
        unsafe { osFlashCheckEraseEnd_recomp(std::ptr::null_mut(), &mut check) };
        assert_eq!(check.r2, 0);

        unsafe { osFlashClearStatus_recomp(std::ptr::null_mut(), std::ptr::null_mut()) };
        assert!(!flash_evidence_snapshot().erase_complete);
        assert_eq!(flash_evidence_snapshot().status, FLASH_STATUS_READY);
    }

    #[test]
    fn flash_erase_status_reports_busy_success_and_failure() {
        install_flash();
        let mut check = ctx_zeroed();

        set_flash_erase_status(FlashEraseStatus::Busy);
        unsafe { osFlashCheckEraseEnd_recomp(std::ptr::null_mut(), &mut check) };
        assert_eq!(check.r2, FLASH_STATUS_ERASE_BUSY as u64);

        set_flash_erase_status(FlashEraseStatus::Succeeded);
        unsafe { osFlashCheckEraseEnd_recomp(std::ptr::null_mut(), &mut check) };
        assert_eq!(check.r2, FLASH_STATUS_ERASE_OK as u64);

        set_flash_erase_status(FlashEraseStatus::Failed);
        unsafe { osFlashCheckEraseEnd_recomp(std::ptr::null_mut(), &mut check) };
        assert_eq!(check.r2, u64::MAX);
        assert_eq!(
            flash_evidence_snapshot().status,
            FLASH_STATUS_READY | FLASH_STATUS_ERASE_ERROR_BIT
        );
    }

    #[test]
    fn flash_init_materializes_the_public_pi_handle() {
        install_flash();
        let mut rdram = vec![0xA5; 0x1300];
        let mut init = ctx_zeroed();
        unsafe { osFlashInit_recomp(rdram.as_mut_ptr(), &mut init) };

        assert_eq!(init.r2 as u32, 0x8000_1200);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let base = RdramAddr::from_offset(0x1200);
        assert_eq!(view.read_u32(base), 0);
        assert_eq!(
            view.read_u8(base.checked_add(4).unwrap()),
            FLASH_DEVICE_TYPE
        );
        assert_eq!(view.read_u8(base.checked_add(5).unwrap()), FLASH_PI_LATENCY);
        assert_eq!(
            view.read_u8(base.checked_add(6).unwrap()),
            FLASH_PI_PAGE_SIZE
        );
        assert_eq!(view.read_u8(base.checked_add(7).unwrap()), FLASH_PI_RELEASE);
        assert_eq!(view.read_u8(base.checked_add(8).unwrap()), FLASH_PI_PULSE);
        assert_eq!(view.read_u8(base.checked_add(9).unwrap()), FLASH_PI_DOMAIN);
        assert_eq!(
            view.read_u32(base.checked_add(12).unwrap()),
            FLASH_KSEG1_BASE
        );
    }

    #[test]
    fn flash_status_snapshot_predicts_the_next_status_read() {
        install_flash();
        with_host(|host| host.flash.status = 0xC3);
        assert_eq!(flash_evidence_snapshot().status, 0xC3);

        let mut rdram = vec![0u8; 0x40];
        let mut status = ctx_zeroed();
        status.r4 = 0x8000_0020;
        unsafe { osFlashReadStatus_recomp(rdram.as_mut_ptr(), &mut status) };
        assert_eq!(rdram[0x20 ^ 3], 0xC3);
    }

    #[test]
    fn flash_identity_snapshot_predicts_the_next_id_read() {
        install_flash();
        let identity = FlashIdentity {
            flash_type: 0x1020_3040,
            flash_maker: 0x5060_7080,
        };
        set_flash_identity(identity);
        assert_eq!(flash_evidence_snapshot().identity, identity);

        let mut rdram = vec![0u8; 0x80];
        let mut read = ctx_zeroed();
        read.r4 = 0x8000_0040;
        read.r5 = 0x8000_0044;
        unsafe { osFlashReadId_recomp(rdram.as_mut_ptr(), &mut read) };
        assert_eq!(
            u32::from_ne_bytes(rdram[0x40..0x44].try_into().unwrap()),
            identity.flash_type
        );
        assert_eq!(
            u32::from_ne_bytes(rdram[0x44..0x48].try_into().unwrap()),
            identity.flash_maker
        );
    }

    #[test]
    fn flash_buffer_write_array_and_multi_page_read_round_trip() {
        install_flash();
        let mut rdram = vec![0u8; 0x1000];
        for index in 0..FLASH_PAGE_SIZE {
            rdram[(0x100 + index) ^ 3] = index as u8;
        }

        let mut stage = ctx_zeroed();
        stage.r6 = 0x8000_0100;
        unsafe { osFlashWriteBuffer_recomp(rdram.as_mut_ptr(), &mut stage) };
        assert_eq!(stage.r2, 0);

        let mut commit = ctx_zeroed();
        commit.r4 = 7;
        unsafe { osFlashWriteArray_recomp(rdram.as_mut_ptr(), &mut commit) };
        assert_eq!(commit.r2, 0);

        let stack = 0x40usize;
        rdram[stack + 0x10..stack + 0x14].copy_from_slice(&1u32.to_ne_bytes());
        rdram[stack + 0x14..stack + 0x18].copy_from_slice(&0u32.to_ne_bytes());
        let mut read = ctx_zeroed();
        read.r6 = 7;
        read.r7 = 0x8000_0300;
        read.r29 = 0x8000_0040;
        unsafe { osFlashReadArray_recomp(rdram.as_mut_ptr(), &mut read) };
        assert_eq!(read.r2, 0);
        for index in 0..FLASH_PAGE_SIZE {
            assert_eq!(rdram[(0x300 + index) ^ 3], index as u8);
        }
        assert_eq!(
            crate::copy_save_operations()
                .iter()
                .map(|event| event.operation)
                .collect::<Vec<_>>(),
            vec![
                fn64_runtime::SaveOperationKind::Write,
                fn64_runtime::SaveOperationKind::Read,
            ]
        );
    }

    #[test]
    fn flash_sector_erase_uses_page_to_16k_sector_mapping() {
        install_flash();
        let mut rdram = vec![0u8; 0x400];
        for byte in 0..FLASH_PAGE_SIZE {
            rdram[(0x100 + byte) ^ 3] = 0x5A;
        }
        let mut stage = ctx_zeroed();
        stage.r6 = 0x8000_0100;
        unsafe { osFlashWriteBuffer_recomp(rdram.as_mut_ptr(), &mut stage) };
        let mut commit = ctx_zeroed();
        commit.r4 = 130;
        unsafe { osFlashWriteArray_recomp(rdram.as_mut_ptr(), &mut commit) };

        let mut erase = ctx_zeroed();
        erase.r4 = 191;
        unsafe { osFlashSectorEraseThrough_recomp(rdram.as_mut_ptr(), &mut erase) };
        let mut status = ctx_zeroed();
        unsafe { osFlashCheckEraseEnd_recomp(rdram.as_mut_ptr(), &mut status) };
        assert_eq!(status.r2, 0);
        unsafe { osFlashCheckEraseEnd_recomp(rdram.as_mut_ptr(), &mut status) };
        assert_eq!(status.r2, 0, "completed erase status remains observable");

        let stack = 0x40usize;
        rdram[stack + 0x10..stack + 0x14].copy_from_slice(&1u32.to_ne_bytes());
        rdram[stack + 0x14..stack + 0x18].copy_from_slice(&0u32.to_ne_bytes());
        let mut read = ctx_zeroed();
        read.r6 = 130;
        read.r7 = 0x8000_0200;
        read.r29 = 0x8000_0040;
        unsafe { osFlashReadArray_recomp(rdram.as_mut_ptr(), &mut read) };
        for byte in 0..FLASH_PAGE_SIZE {
            assert_eq!(rdram[(0x200 + byte) ^ 3], 0xFF);
        }
    }

    #[test]
    fn flash_status_and_identity_use_public_logical_layout() {
        install_flash();
        let mut rdram = vec![0xA5u8; 0x100];

        let mut status = ctx_zeroed();
        status.r4 = 0x8000_0020;
        unsafe { osFlashReadStatus_recomp(rdram.as_mut_ptr(), &mut status) };
        assert_eq!(rdram[0x20 ^ 3], FLASH_STATUS_READY);

        let mut identity = ctx_zeroed();
        identity.r4 = 0x8000_0040;
        identity.r5 = 0x8000_0044;
        unsafe { osFlashReadId_recomp(rdram.as_mut_ptr(), &mut identity) };
        assert_eq!(
            u32::from_ne_bytes(rdram[0x40..0x44].try_into().unwrap()),
            FLASH_TYPE_1MBIT
        );
        assert_eq!(
            u32::from_ne_bytes(rdram[0x44..0x48].try_into().unwrap()),
            FLASH_MAKER_MACRONIX_C
        );
    }

    #[test]
    fn flash_change_accepts_the_installed_chip_zero() {
        install_flash();
        let mut change = ctx_zeroed();
        change.r4 = 0;
        unsafe { osFlashChange_recomp(std::ptr::null_mut(), &mut change) };
    }
}
