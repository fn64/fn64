//! Development-hardware and debug-output transport shims.
//!
//! Public `rdb.h` defines the six-bit packet types and three-byte RDB packet
//! payload. Public libultra initialization/debug manuals define the MSP, KMC,
//! ISV, and RDB roles. fn64 exposes a host-selected hardware profile and an
//! owned packet log instead of probing nonexistent host MMIO or discarding
//! diagnostics.

use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugHardware {
    #[default]
    None,
    Msp,
    Kmc,
    Isv,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugPacket {
    pub packet_type: u8,
    pub bytes: Vec<u8>,
}

pub fn set_debug_hardware(hardware: DebugHardware) {
    with_host(|host| host.debug_hardware = hardware);
}

pub fn take_debug_packets() -> Vec<DebugPacket> {
    with_host(|host| std::mem::take(&mut host.debug_packets))
}

fn copy_guest_bytes(rdram: *mut u8, raw: u64, len: usize) -> Vec<u8> {
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let base = RdramAddr::from_gpr(raw);
    (0..len)
        .map(|index| unsafe {
            storage.read_u8(
                base.checked_add(u32::try_from(index).expect("debug output length exceeds u32"))
                    .expect("debug output guest address overflow"),
            )
        })
        .collect()
}

fn push_packet(packet_type: u32, bytes: Vec<u8>) {
    assert!(
        packet_type <= 63,
        "RDB packet type {packet_type} exceeds the public six-bit field"
    );
    with_host(|host| {
        host.debug_packets.push(DebugPacket {
            packet_type: packet_type as u8,
            bytes,
        });
    });
}

/// `_Printf` output callback: `(void *arg, const char *str, u32 count)`.
///
/// # Safety
/// `rdram` and `ctx` must satisfy the process-lifetime recompiler ABI contract.
#[no_mangle]
pub unsafe extern "C" fn is_proutSyncPrintf_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let bytes = copy_guest_bytes(rdram, ctx.r5, ctx.r6 as u32 as usize);
    push_packet(1, bytes);
    ctx.r2 = ctx.r4;
}

/// Internal RDB packet sender: buffer, length, and public packet type.
///
/// # Safety
/// `rdram` and `ctx` must satisfy the process-lifetime recompiler ABI contract.
#[no_mangle]
pub unsafe extern "C" fn __osRdbSend_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let len = ctx.r5 as u32 as usize;
    let bytes = copy_guest_bytes(rdram, ctx.r4, len);
    push_packet(ctx.r6 as u32, bytes);
    ctx.r2 = len as u64;
}

fn check_hardware(ctx: *mut RecompContext, expected: DebugHardware) {
    let present = with_host(|host| host.debug_hardware == expected);
    unsafe { &mut *ctx }.r2 = u64::from(present);
}

/// Check for the host-selected MSP development-hardware profile.
///
/// # Safety
/// `ctx` must point to a live recompiler context.
#[no_mangle]
pub unsafe extern "C" fn __checkHardware_msp_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    check_hardware(ctx, DebugHardware::Msp);
}

/// Check for the host-selected KMC development-hardware profile.
///
/// # Safety
/// `ctx` must point to a live recompiler context.
#[no_mangle]
pub unsafe extern "C" fn __checkHardware_kmc_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    check_hardware(ctx, DebugHardware::Kmc);
}

/// Check for the host-selected ISV development-hardware profile.
///
/// # Safety
/// `ctx` must point to a live recompiler context.
#[no_mangle]
pub unsafe extern "C" fn __checkHardware_isv_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    check_hardware(ctx, DebugHardware::Isv);
}

fn initialize_hardware(hardware: DebugHardware) {
    crate::system::initialize_common();
    set_debug_hardware(hardware);
}

/// Initialize common state and select MSP development hardware.
///
/// # Safety
/// The arguments must satisfy the recompiler ABI contract.
#[no_mangle]
pub unsafe extern "C" fn __osInitialize_msp_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    initialize_hardware(DebugHardware::Msp);
}

/// Initialize common state and select KMC development hardware.
///
/// # Safety
/// The arguments must satisfy the recompiler ABI contract.
#[no_mangle]
pub unsafe extern "C" fn __osInitialize_kmc_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    initialize_hardware(DebugHardware::Kmc);
}

/// Initialize common state and select ISV development hardware.
///
/// # Safety
/// The arguments must satisfy the recompiler ABI contract.
#[no_mangle]
pub unsafe extern "C" fn __osInitialize_isv_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    initialize_hardware(DebugHardware::Isv);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ctx_zeroed;

    #[test]
    fn printf_and_rdb_preserve_exact_guest_bytes_and_packet_type() {
        let mut rdram = vec![0u8; 0x100];
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
        for (index, byte) in b"hello".iter().copied().enumerate() {
            unsafe { storage.write_u8(RdramAddr::from_offset(0x40 + index as u32), byte) };
        }
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0xCAFE;
        ctx.r5 = 0x8000_0040;
        ctx.r6 = 5;
        unsafe { is_proutSyncPrintf_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0xCAFE);
        assert_eq!(
            take_debug_packets(),
            vec![DebugPacket {
                packet_type: 1,
                bytes: b"hello".to_vec()
            }]
        );
    }
}
