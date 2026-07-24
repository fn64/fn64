use super::*;
use sha2::{Digest, Sha256};

struct LiveRdramDma {
    storage: fn64_runtime::RdramPtr,
    len: usize,
}

impl LiveRdramDma {
    /// # Safety
    /// `rdram` must remain valid for `len` bytes while this adapter is used,
    /// and guest execution must be suspended for every access.
    unsafe fn new(rdram: *mut u8, len: usize) -> Self {
        assert!(
            len.is_multiple_of(4),
            "PI DMA RDRAM storage extent {len:#x} must cover complete native words"
        );
        Self {
            storage: unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) },
            len,
        }
    }

    fn checked_addr(&self, offset: usize) -> RdramAddr {
        let offset = u32::try_from(offset).expect("PI DMA RDRAM offset exceeds u32");
        assert!(
            (offset as usize) < self.len,
            "PI DMA logical byte {offset:#x} is outside RDRAM allocation {:#x}",
            self.len
        );
        RdramAddr::from_offset(offset)
    }
}

impl fn64_runtime::DmaMemory for LiveRdramDma {
    fn dma_write_bytes(&mut self, offset: usize, data: &[u8]) {
        for (index, byte) in data.iter().copied().enumerate() {
            let addr = self.checked_addr(
                offset
                    .checked_add(index)
                    .expect("PI DMA RDRAM write range overflow"),
            );
            unsafe { self.storage.write_u8(addr, byte) };
        }
        #[cfg(feature = "recomp-rs")]
        fn64_recomp_rs::notify_guest_write(
            u32::try_from(offset).expect("DMA write offset exceeds u32"),
            u32::try_from(data.len()).expect("DMA write length exceeds u32"),
        );
    }

    fn dma_read_bytes_flat(&self, offset: usize, len: usize) -> Vec<u8> {
        (0..len)
            .map(|index| {
                let addr = self.checked_addr(
                    offset
                        .checked_add(index)
                        .expect("PI DMA RDRAM read range overflow"),
                );
                unsafe { self.storage.read_u8(addr) }
            })
            .collect()
    }
}

/// Install the real ROM bytes the PI/EPI DMA shims read from. Must be
/// called once before any `osEPiStartDma_recomp`/`osCartRomInit_recomp`
/// call, per `README.md`'s "no game content ships in this repo" rule --
/// `fn64-shell` supplies the user's own loaded ROM file's bytes here.
pub fn load_rom(bytes: Vec<u8>) {
    load_rom_with_fixed_pi_latency(bytes, 1);
}

/// Install ROM bytes with an explicit deterministic PI completion latency.
/// The fixed model is a compatibility policy, not a cycle-accuracy claim;
/// hardware-derived timing can replace it behind `PiTimingModel` without
/// changing DMA ordering or either entry path.
pub fn load_rom_with_fixed_pi_latency(bytes: Vec<u8>, latency_cycles: u64) {
    assert!(
        latency_cycles > 0,
        "PI latency must be at least one guest cycle so start and completion remain observable"
    );
    let installed_rom = InstalledRomEvidenceSnapshot {
        byte_len: u64::try_from(bytes.len()).expect("installed ROM length exceeds evidence wire"),
        sha256: Sha256::digest(&bytes).into(),
    };
    with_host(|host| {
        let tv_type = host.device_fabric.tv_type();
        let mut device_fabric = DeviceFabric::new(
            PiDma::new(InMemoryRom::new(bytes)),
            FixedPiTiming(Cycles::new(latency_cycles)),
        );
        if let Some(tv_type) = tv_type {
            device_fabric
                .configure_tv_type(tv_type)
                .unwrap_or_else(|error| panic!("restoring television standard failed: {error}"));
        }
        host.device_fabric = device_fabric;
        host.rom_installed = true;
        host.installed_rom = Some(installed_rom);
        host.cartridge_save = CartridgeSaveEvidenceSnapshot::Unidentified;
        host.pending_pi_completions.clear();
        host.save_operations.clear();
        host.controller_operations.clear();
        host.rsp_rdp_observations.clear();
        host.rsp_boot_images.clear();
        host.loaded_rsp_task = None;
        host.rsp_task_lineages.clear();
        host.native_execution_destinations.clear();
    });
    crate::task_dispatch::reset_audio_task_execution_for_rom();
}

/// Register the guest BSS address of libultra's cartridge `OSPiHandle`.
///
/// `osCartRomInit` returns an `OSPiHandle*` that ordinary recompiled game code
/// may dereference before passing it back to an EPI shim. The address therefore
/// cannot be an opaque host token: it must be the aligned, guest-visible BSS
/// object from this particular ROM's link map.
pub fn set_cart_rom_handle_vram(vram: u32) {
    assert!(
        (0x8000_0000..0xC000_0000).contains(&vram),
        "cart OSPiHandle must be a KSEG0/KSEG1 guest address, got {vram:#010x}"
    );
    assert!(
        vram.is_multiple_of(4),
        "cart OSPiHandle must be word-aligned, got {vram:#010x}"
    );
    with_host(|host| host.cart_rom_handle_vram = Some(vram));
}

/// Guest storage and PI timing needed to construct the 64DD-register
/// `OSPiHandle` returned by `osLeoDiskInit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeoDiskConfig {
    pub handle_vram: u32,
    pub latency: u8,
    pub page_size: u8,
    pub release: u8,
    pub pulse_width: u8,
}

const DEVICE_TYPE_CART: u8 = 0;
const DEVICE_TYPE_64DD: u8 = 2;
const DEVICE_TYPE_SRAM: u8 = 3;
const PI_DOMAIN1: u8 = 0;
const PI_DOMAIN2: u8 = 1;
const KSEG1_BASE: u32 = 0xa000_0000;
const KSEG1_END: u32 = 0xc000_0000;
const PI_DOM1_ADDR1: std::ops::RangeInclusive<u32> = 0x0600_0000..=0x07ff_ffff;
const PI_DOM1_ADDR2: std::ops::RangeInclusive<u32> = 0x1000_0000..=0x1fbf_ffff;
const PI_DOM1_ADDR3: std::ops::RangeInclusive<u32> = 0x1fd0_0000..=0x7fff_ffff;
const PI_DOM2_ADDR1: std::ops::RangeInclusive<u32> = 0x0500_0000..=0x05ff_ffff;
const PI_DOM2_ADDR2: std::ops::RangeInclusive<u32> = 0x0800_0000..=0x0fff_ffff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EpiHandle {
    device_type: u8,
    domain: fn64_runtime::PiDomain,
    timing: fn64_runtime::PiDomainTiming,
    base_address: u32,
}

fn trap_epi_handle(shim: &str, detail: impl std::fmt::Display) -> ! {
    let message = format!("{shim}: {detail}");
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Abi,
        "abi.pi.epi-handle",
        &message,
        Some(with_host(|host| host.device_fabric.now())),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!("{message}")
}

fn epi_domain_for_address(shim: &str, address: u32) -> fn64_runtime::PiDomain {
    if PI_DOM1_ADDR1.contains(&address)
        || PI_DOM1_ADDR2.contains(&address)
        || PI_DOM1_ADDR3.contains(&address)
    {
        fn64_runtime::PiDomain::Domain1
    } else if PI_DOM2_ADDR1.contains(&address) || PI_DOM2_ADDR2.contains(&address) {
        fn64_runtime::PiDomain::Domain2
    } else {
        trap_epi_handle(
            shim,
            format_args!(
                "OSPiHandle base address {address:#010x} is outside the public PI domain map"
            ),
        )
    }
}

fn epi_physical_base(shim: &str, base_address: u32) -> u32 {
    if !(KSEG1_BASE..KSEG1_END).contains(&base_address) {
        trap_epi_handle(
            shim,
            format_args!(
                "OSPiHandle base address {base_address:#010x} is not the public uncached KSEG1 device address form"
            ),
        );
    }
    base_address & 0x1fff_ffff
}

/// Decode one public `OSPiHandle` and apply its bus parameters to the same
/// domain registers exposed through raw PI MMIO. Chapter 27 defines this as
/// the common authority for managed and raw EPI calls: a handle switch updates
/// the bus before the operation. The public SRAM example stores
/// `PHYS_TO_K1(device_start)` in `baseAddress`; the decoder applies the
/// documented `baseAddress | devAddr` operation in that public address form,
/// then removes the uncached CPU segment tag at the PI boundary.
///
/// # Safety
/// `rdram` must point at guest storage containing the complete handle named by
/// `handle_gpr`.
unsafe fn resolve_epi_device_address(
    rdram: *mut u8,
    handle_gpr: u64,
    dev_addr: u32,
    shim: &str,
) -> u32 {
    let upper = handle_gpr >> 32;
    let handle_vram = handle_gpr as u32;
    if (upper != 0 && upper != u32::MAX as u64)
        || !(0x8000_0000..0xc000_0000).contains(&handle_vram)
        || !handle_vram.is_multiple_of(4)
    {
        trap_epi_handle(
            shim,
            format_args!(
                "OSPiHandle pointer {handle_gpr:#018x} is not an aligned zero/sign-extended KSEG0/KSEG1 address"
            ),
        );
    }
    let handle_offset = handle_vram & 0x1fff_ffff;
    let public_end = handle_offset
        .checked_add(20)
        .expect("OSPiHandle public range overflow");
    if public_end > fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u32 {
        trap_epi_handle(
            shim,
            format_args!(
                "OSPiHandle pointer {handle_gpr:#018x} maps outside physical RDRAM: {handle_offset:#010x}..{public_end:#010x}"
            ),
        );
    }

    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let base = RdramAddr::from_gpr(handle_gpr);
    let handle = EpiHandle {
        device_type: unsafe { storage.read_u8(base.checked_add(4).unwrap()) },
        timing: fn64_runtime::PiDomainTiming {
            latency: unsafe { storage.read_u8(base.checked_add(5).unwrap()) },
            page_size: unsafe { storage.read_u8(base.checked_add(6).unwrap()) },
            release: unsafe { storage.read_u8(base.checked_add(7).unwrap()) },
            pulse_width: unsafe { storage.read_u8(base.checked_add(8).unwrap()) },
        },
        domain: match unsafe { storage.read_u8(base.checked_add(9).unwrap()) } {
            PI_DOMAIN1 => fn64_runtime::PiDomain::Domain1,
            PI_DOMAIN2 => fn64_runtime::PiDomain::Domain2,
            value => trap_epi_handle(shim, format_args!("OSPiHandle has invalid domain {value}")),
        },
        base_address: unsafe { storage.read_u32(base.checked_add(12).unwrap()) },
    };
    if handle.device_type > DEVICE_TYPE_SRAM {
        trap_epi_handle(
            shim,
            format_args!("OSPiHandle has invalid device type {}", handle.device_type),
        );
    }
    if handle.timing.page_size > 0x0f || handle.timing.release > 0x03 {
        trap_epi_handle(
            shim,
            format_args!(
                "OSPiHandle timing fields exceed PI register width: pageSize={:#04x}, relDuration={:#04x}",
                handle.timing.page_size, handle.timing.release
            ),
        );
    }
    let physical_base = epi_physical_base(shim, handle.base_address);
    let mapped_domain = epi_domain_for_address(shim, physical_base);
    if mapped_domain != handle.domain {
        trap_epi_handle(
            shim,
            format_args!(
                "OSPiHandle domain {:?} disagrees with base address {:#010x} ({mapped_domain:?})",
                handle.domain, handle.base_address
            ),
        );
    }
    let device_address = handle.base_address | dev_addr;
    let physical = epi_physical_base(shim, device_address);
    if epi_domain_for_address(shim, physical) != handle.domain {
        trap_epi_handle(
            shim,
            format_args!(
                "OSPiHandle base {:#010x} OR device address {dev_addr:#010x} escapes {:?}",
                handle.base_address, handle.domain
            ),
        );
    }
    with_host(|host| {
        host.device_fabric
            .set_pi_domain_timing(handle.domain, handle.timing)
    });

    if PI_DOM1_ADDR2.contains(&physical) {
        if handle.device_type != DEVICE_TYPE_CART {
            trap_epi_handle(
                shim,
                format_args!(
                    "OSPiHandle type {} cannot use the Game Pak ROM address space",
                    handle.device_type
                ),
            );
        }
        physical - 0x1000_0000
    } else if PI_DOM2_ADDR2.contains(&physical) {
        if handle.device_type != DEVICE_TYPE_SRAM {
            trap_epi_handle(
                shim,
                format_args!(
                    "OSPiHandle type {} cannot use the SRAM address space",
                    handle.device_type
                ),
            );
        }
        physical
    } else {
        trap_epi_handle(
            shim,
            format_args!(
                "OSPiHandle selects documented PI device space {physical:#010x}, but no backing device is attached for that space"
            ),
        )
    }
}

/// Write the public portion of an `OSPiHandle` from typed device state. The
/// base address is the public uncached KSEG1 form used by Chapter 27's SRAM
/// acquisition example, not a raw PI physical address.
/// Acquisition shims share this constructor so Cart ROM and 64DD handles
/// cannot drift in field layout or timing-register authority.
///
/// # Safety
/// `rdram` must contain writable storage for the complete public handle.
pub(crate) unsafe fn write_epi_handle(
    rdram: *mut u8,
    handle_vram: u32,
    device_type: u8,
    domain: fn64_runtime::PiDomain,
    timing: fn64_runtime::PiDomainTiming,
    base_address: u32,
) {
    assert!(
        (0x8000_0000..0xc000_0000).contains(&handle_vram) && handle_vram.is_multiple_of(4),
        "OSPiHandle storage must be an aligned KSEG0/KSEG1 address, got {handle_vram:#010x}"
    );
    assert!(
        device_type <= DEVICE_TYPE_SRAM,
        "OSPiHandle device type {device_type} exceeds the public range"
    );
    assert!(
        timing.page_size <= 0x0f && timing.release <= 0x03,
        "OSPiHandle timing exceeds PI register width"
    );
    assert_eq!(
        epi_domain_for_address(
            "write_epi_handle",
            epi_physical_base("write_epi_handle", base_address),
        ),
        domain,
        "OSPiHandle domain disagrees with its public base address"
    );
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let base = RdramAddr::from_gpr(handle_vram as u64);
    unsafe {
        storage.write_u32(base, 0);
        storage.write_u8(base.checked_add(4).unwrap(), device_type);
        storage.write_u8(base.checked_add(5).unwrap(), timing.latency);
        storage.write_u8(base.checked_add(6).unwrap(), timing.page_size);
        storage.write_u8(base.checked_add(7).unwrap(), timing.release);
        storage.write_u8(base.checked_add(8).unwrap(), timing.pulse_width);
        storage.write_u8(
            base.checked_add(9).unwrap(),
            match domain {
                fn64_runtime::PiDomain::Domain1 => PI_DOMAIN1,
                fn64_runtime::PiDomain::Domain2 => PI_DOMAIN2,
            },
        );
        storage.write_u16(base.checked_add(10).unwrap(), 0);
        storage.write_u32(base.checked_add(12).unwrap(), base_address);
        storage.write_u32(base.checked_add(16).unwrap(), 0);
    }
}

/// Install a 64DD-register handle configuration. This does not fabricate a
/// mounted disk; it makes the public EPI device description available to a
/// host that also supplies the general 64DD device path.
pub fn configure_leo_disk(config: LeoDiskConfig) {
    assert!(
        (0x8000_0000..0xC000_0000).contains(&config.handle_vram),
        "64DD OSPiHandle must be a KSEG0/KSEG1 guest address, got {:#010x}",
        config.handle_vram
    );
    assert!(
        config.handle_vram.is_multiple_of(4),
        "64DD OSPiHandle must be word-aligned, got {:#010x}",
        config.handle_vram
    );
    assert!(
        config.page_size <= 0xF,
        "PI page-size field exceeds four bits"
    );
    assert!(config.release <= 0x3, "PI release field exceeds two bits");
    with_host(|host| host.leo_disk = Some(config));
}

/// Register the game's save-backing store (SRAM/EEPROM/Flash) the domain-2
/// PI-DMA path routes to -- `fn64-shell`/the harness supplies an
/// `InMemorySaveStorage`/`FileSaveStorage` sized for the game's save device
/// (OoT: `SaveType::SramBanked`, 32 KiB). Must be called after `load_rom`
/// (the `PiDma` engine must exist) and before any domain-2 (SRAM) DMA. A
/// domain-2 DMA with no save registered is a loud trap, not a silent ROM
/// read past its end (see `PiDma::set_save`).
pub fn set_save(save: Box<dyn fn64_runtime::SaveStorage>) {
    with_host(|host| {
        assert!(
            host.rom_installed,
            "set_save: no ROM installed -- call fn64_abi::load_rom(bytes) before installing save storage"
        );
        host.save_operations
            .extend(host.device_fabric.pi_dma_mut().take_save_operations());
        host.device_fabric.pi_dma_mut().set_save(save);
        host.cartridge_save = CartridgeSaveEvidenceSnapshot::Unidentified;
    });
}

/// Install cartridge save storage with an exact closed hardware identity.
///
/// The type excludes Controller Pak by construction. Storage length is
/// checked before ownership moves into the PI engine, preventing an EEPROM,
/// SRAM, or Flash declaration from labeling a differently sized device.
pub fn set_cartridge_save(save_type: CartridgeSaveType, save: Box<dyn fn64_runtime::SaveStorage>) {
    assert_eq!(
        save.len(),
        save_type.byte_len(),
        "cartridge save storage length does not match {save_type:?}"
    );
    with_host(|host| {
        assert!(
            host.rom_installed,
            "set_cartridge_save: no ROM installed -- call fn64_abi::load_rom(bytes) before installing save storage"
        );
        host.save_operations
            .extend(host.device_fabric.pi_dma_mut().take_save_operations());
        host.device_fabric.pi_dma_mut().set_save(save);
        host.cartridge_save = CartridgeSaveEvidenceSnapshot::Configured(save_type);
    });
}

/// Assert that this cartridge has no mounted save hardware.
///
/// This does not remove a device. Calling it after any save installation is a
/// host configuration error and traps rather than relabeling live storage.
pub fn configure_no_cartridge_save() {
    with_host(|host| {
        assert!(
            host.rom_installed,
            "configure_no_cartridge_save: no ROM installed"
        );
        assert!(
            host.device_fabric
                .pi_dma_mut()
                .save_snapshot_bytes()
                .is_none(),
            "configure_no_cartridge_save cannot relabel installed save storage"
        );
        host.cartridge_save = CartridgeSaveEvidenceSnapshot::NoCartridgeSave;
    });
}

pub(crate) fn with_pi_dma<R>(shim: &str, f: impl FnOnce(&mut PiDma<InMemoryRom>) -> R) -> R {
    with_host(|host| {
        if !host.rom_installed {
            panic!(
                "{shim}: no ROM installed -- call fn64_abi::load_rom(bytes) before any PI/EPI \
                 DMA shim runs (see that function's doc comment; this crate never ships game \
                 content, so there is no default ROM to fall back to)"
            )
        }
        host.save_operations
            .extend(host.device_fabric.pi_dma_mut().take_save_operations());
        let result = f(host.device_fabric.pi_dma_mut());
        host.save_operations
            .extend(host.device_fabric.pi_dma_mut().take_save_operations());
        result
    })
}

fn start_timed_pi_dma(
    rdram: *mut u8,
    rdram_len: usize,
    request: PiDmaRequest,
    ret_queue: Option<RdramAddr>,
    ret_mesg: u32,
    shim: &str,
) -> Result<(), DeviceFault> {
    // Cached: this sits on the per-chunk PI DMA hot path -- an uncached
    // getenv per 0x200-byte chunk dominated wall time on multi-megabyte
    // overlay streams (lldb-confirmed, BOOT-NOTES-WM2000.md part 7).
    static TRACE_PI_DMA: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *TRACE_PI_DMA.get_or_init(|| std::env::var_os("FN64_TRACE_PI_DMA").is_some()) {
        let thread = crate::current_thread_id("FN64_TRACE_PI_DMA");
        eprintln!(
            "[fn64-abi/pi] thread={thread} {shim} {:?} cart={:#010x} dram={:#010x} len={:#x} \
             ret_queue={ret_queue:?}",
            request.direction,
            request.cart_addr,
            request.dram_addr.offset(),
            request.len
        );
    }
    let result = with_host(|host| {
        if matches!(request.direction, DmaDirection::FromRdram)
            && !fn64_runtime::rom::is_sram_dev_addr(request.cart_addr)
        {
            return Err(DeviceFault::PiTransfer(PiDmaError::ReadOnlyDevice {
                dev_addr: request.cart_addr,
            }));
        }
        if !host.rom_installed {
            panic!(
                "{shim}: no ROM installed -- call fn64_abi::load_rom(bytes) before any PI/EPI DMA"
            )
        }
        if request.len == 0 {
            return Err(DeviceFault::ZeroLengthPiDma);
        }

        let pending = PendingPiCompletion {
            request,
            rdram,
            rdram_len,
            ret_queue,
            ret_mesg,
        };
        // Interleaving closed here: thread A starts a managed PI transfer and
        // blocks on its completion queue; before that deadline, thread B
        // submits another managed transfer. The PI manager must accept B and
        // serialize it behind A. Returning raw `PiBusy` to B makes DmaMgr
        // report a completed-but-truncated multi-chunk load to its client.
        if host.pending_pi_completions.is_empty() {
            host.device_fabric.start_pi_dma(request)?;
        }
        host.pending_pi_completions.push_back(pending);
        Ok(())
    });
    if matches!(result, Err(DeviceFault::PiBusy)) {
        // Charge outside the HostState borrow so the checkpoint may advance
        // virtual time and commit the in-flight transfer before the retry.
        crate::charge_guest_device_busy_retry();
    }
    result
}

/// Execute one raw PI transfer through the single ROM/save engine. This is
/// the convergence point for unmanaged EPI now and for the DeviceFabric
/// migration of managed requests.
fn start_raw_pi_dma(
    rdram: *mut u8,
    direction: DmaDirection,
    dram_addr: RdramAddr,
    dev_addr: u32,
    len: u32,
    shim: &str,
) -> bool {
    assert!(
        dev_addr.is_multiple_of(2),
        "{shim}: PI device address {dev_addr:#010x} is not 2-byte aligned"
    );
    assert!(
        dram_addr.offset().is_multiple_of(8),
        "{shim}: RDRAM address {:#010x} is not 8-byte aligned",
        dram_addr.offset()
    );
    assert!(
        len != 0 && len.is_multiple_of(2) && len <= 0x0100_0000,
        "{shim}: PI length {len:#x} must be a nonzero multiple of 2 no larger than 16 MiB"
    );

    let logical_end = usize::try_from(
        dram_addr
            .offset()
            .checked_add(len)
            .expect("PI DMA RDRAM range overflow"),
    )
    .expect("PI DMA RDRAM extent exceeds usize");
    let rdram_len = logical_end
        .checked_add(3)
        .expect("PI DMA RDRAM storage extent overflow")
        & !3;
    start_timed_pi_dma(
        rdram,
        rdram_len,
        PiDmaRequest {
            direction,
            dram_addr,
            cart_addr: dev_addr,
            len,
        },
        None,
        0,
        shim,
    )
    .is_ok()
}

fn live_device_mmio_addr(vaddr: u64, write: bool) -> Option<MmioAddr> {
    let upper = vaddr >> 32;
    if upper != 0 && upper != u32::MAX as u64 {
        return None;
    }
    let addr = vaddr as u32;
    let is_sp = (0xA400_0000..0xA400_2000).contains(&addr)
        || matches!(
            addr,
            0xA404_0000
                | 0xA404_0004
                | 0xA404_0008
                | 0xA404_000C
                | 0xA404_0010
                | 0xA404_0014
                | 0xA404_0018
                | 0xA404_001C
                | 0xA408_0000
        );
    let is_pi_write = write && (0xA460_0000..=0xA460_0030).contains(&addr);
    let is_pi_read = !write
        && matches!(
            addr,
            0xA460_0000
                | 0xA460_0004
                | 0xA460_0010
                | 0xA460_0014
                | 0xA460_0018
                | 0xA460_001C
                | 0xA460_0020
                | 0xA460_0024
                | 0xA460_0028
                | 0xA460_002C
                | 0xA460_0030
        );
    let is_mi_read = !write && matches!(addr, 0xA430_0008 | 0xA430_000C);
    let is_mi_write = write && addr == 0xA430_000C;
    let is_dpc_read =
        !write && matches!(addr, 0xA410_0000 | 0xA410_0004 | 0xA410_0008 | 0xA410_000C);
    let is_dpc_write = write && matches!(addr, 0xA410_0000 | 0xA410_0004 | 0xA410_000C);
    let is_vi = (0xA440_0000..=0xA440_0034).contains(&addr);
    let is_ai = matches!(
        addr,
        0xA450_0000 | 0xA450_0004 | 0xA450_0008 | 0xA450_000C | 0xA450_0010 | 0xA450_0014
    );
    let is_si_read = !write && matches!(addr, 0xA480_0000 | 0xA480_0018);
    let is_si_write =
        write && matches!(addr, 0xA480_0000 | 0xA480_0004 | 0xA480_0010 | 0xA480_0018);
    (is_sp
        || is_pi_write
        || is_pi_read
        || is_mi_read
        || is_mi_write
        || is_dpc_read
        || is_dpc_write
        || is_vi
        || is_ai
        || is_si_read
        || is_si_write)
        .then(|| MmioAddr::new(addr))
}

/// Sample the level-sensitive RCP interrupt output presented to CPU IP2.
#[cfg(any(test, feature = "recomp-rs"))]
pub(crate) fn cpu_interrupt_pending() -> bool {
    with_host(|host| host.device_fabric.cpu_interrupt_pending())
}

/// Replace the six-source MI mask after `osSetIntMask` has unpacked its
/// bits 16..21. The CPU IP2 mask is separate and lives in the running
/// context's Status register; this function owns only the global RCP gate.
pub(crate) fn set_mi_interrupt_mask(mask: u32) {
    with_host(|host| {
        let fabric = &mut host.device_fabric;
        for source in [
            fn64_runtime::InterruptSource::Sp,
            fn64_runtime::InterruptSource::Si,
            fn64_runtime::InterruptSource::Ai,
            fn64_runtime::InterruptSource::Vi,
            fn64_runtime::InterruptSource::Pi,
            fn64_runtime::InterruptSource::Dp,
        ] {
            fabric.set_interrupt_mask(source, mask & source.bit() != 0);
        }
    });
}

pub(crate) fn raise_device_interrupt(source: fn64_runtime::InterruptSource) {
    with_host(|host| host.device_fabric.raise_interrupt(source));
}

pub(crate) fn clear_device_interrupt(source: fn64_runtime::InterruptSource) {
    with_host(|host| host.device_fabric.clear_interrupt(source));
}

fn apply_live_ai_write_effect(rdram: *mut u8, effect: fn64_runtime::DeviceMmioWriteEffect) {
    match effect {
        fn64_runtime::DeviceMmioWriteEffect::None => {}
        fn64_runtime::DeviceMmioWriteEffect::AiFrequencyChanged { sample_rate_hz } => {
            crate::task_dispatch::notify_audio_frequency(sample_rate_hz);
        }
        fn64_runtime::DeviceMmioWriteEffect::AiDmaStarted(request) => {
            if crate::boot_probe_enabled() {
                use std::sync::atomic::{AtomicU32, Ordering};
                static CALLS: AtomicU32 = AtomicU32::new(0);
                let n = CALLS.fetch_add(1, Ordering::Relaxed);
                if n < 6 || n.is_multiple_of(2048) {
                    eprintln!(
                        "[boot-probe] live AI DMA #{n} len={:#x} rate={}Hz",
                        request.len, request.sample_rate_hz
                    );
                }
            }
            // Host audio is optional. Once a shell/harness registers a bound,
            // both raw and libultra submissions deliver the exact accepted
            // fabric request; an absent consumer does not weaken device state.
            if crate::task_dispatch::AUDIO_RDRAM_LEN.with(Cell::get) != 0 {
                assert!(
                    !rdram.is_null(),
                    "AI DMA reached a registered audio backend without process RDRAM"
                );
                unsafe {
                    crate::task_dispatch::deliver_ai_buffer(
                        rdram,
                        request.dram_addr.offset() as usize,
                        request.len as usize,
                    )
                };
            }
        }
        fn64_runtime::DeviceMmioWriteEffect::DpcSubmissionRequested(submission) => {
            panic!(
                "AI register write unexpectedly produced DPC transaction token {}",
                submission.token
            )
        }
    }
}

pub(crate) fn set_live_ai_rates(
    encoded_dac_rate: u32,
    encoded_bit_rate: u32,
) -> Result<(), DeviceFault> {
    let dac_effect = with_host(|host| {
        host.device_fabric
            .write_mmio(MmioAddr::new(0xA450_0010), encoded_dac_rate)
    })?;
    let bit_effect = with_host(|host| {
        host.device_fabric
            .write_mmio(MmioAddr::new(0xA450_0014), encoded_bit_rate)
    })?;
    apply_live_ai_write_effect(std::ptr::null_mut(), dac_effect);
    apply_live_ai_write_effect(std::ptr::null_mut(), bit_effect);
    Ok(())
}

/// Submit one libultra AI range through the same DRAM/LEN register transaction
/// as raw generated-C MMIO. The returned effect is a notification only: the
/// fabric has already enqueued the accepted request exactly once.
///
/// # Safety
/// `rdram` must be the process allocation registered for the active guest.
pub(crate) unsafe fn submit_live_ai_dma(
    rdram: *mut u8,
    dram_addr: u32,
    len: u32,
) -> Result<(), DeviceFault> {
    // The public shim reports a full two-slot FIFO without issuing either
    // register write. Raw MMIO retains its architectural write-through
    // behavior, but a rejected libultra call must not replace AI_DRAM_ADDR.
    if with_host(|host| host.device_fabric.ai_status()) & fn64_runtime::AI_STATUS_FULL != 0 {
        return Err(DeviceFault::AiFull);
    }
    let dram_effect = with_host(|host| {
        host.device_fabric
            .write_mmio(MmioAddr::new(0xA450_0000), dram_addr)
    })?;
    apply_live_ai_write_effect(rdram, dram_effect);
    let start_effect = with_host(|host| {
        host.device_fabric
            .write_mmio(MmioAddr::new(0xA450_0004), len)
    })?;
    apply_live_ai_write_effect(rdram, start_effect);
    Ok(())
}

pub(crate) fn start_live_si_dma(
    request: fn64_runtime::SiDmaRequest,
    owner: PendingSiCompletionOwner,
) -> Result<(), DeviceFault> {
    with_host(|host| {
        host.device_fabric.start_si_dma(request)?;
        assert!(
            host.pending_si_completion.is_none(),
            "SI hardware accepted a request while prior completion ownership remained live"
        );
        host.pending_si_completion = Some(PendingSiCompletion { request, owner });
        Ok(())
    })
}

pub(crate) fn start_live_controller_si_dma(
    request: fn64_runtime::SiDmaRequest,
    owner: PendingSiCompletionOwner,
    command: [u8; 64],
) -> Result<(), DeviceFault> {
    assert!(
        matches!(
            request.kind,
            fn64_runtime::SiDmaKind::ControllerQuery | fn64_runtime::SiDmaKind::ControllerRead
        ),
        "Controller Manager attempted to stage a non-controller SI request"
    );
    with_host(|host| {
        host.device_fabric.start_si_dma(request)?;
        assert!(
            host.pending_si_completion.is_none(),
            "SI hardware accepted a request while prior completion ownership remained live"
        );
        host.device_fabric.stage_controller_pif_command(command);
        host.pending_si_completion = Some(PendingSiCompletion { request, owner });
        Ok(())
    })
}

pub(crate) fn start_live_rcp_task_with_latency(
    plan: fn64_runtime::RcpTaskCompletionPlan,
    rsp_steps: u64,
) -> Result<(), DeviceFault> {
    with_host(|host| {
        host.device_fabric
            .start_rcp_task_with_latency(plan, fn64_runtime::Cycles::new(rsp_steps.max(1)))
    })
}

pub(crate) fn begin_live_rcp_task() -> Result<(), DeviceFault> {
    with_host(|host| host.device_fabric.begin_rcp_task())
}

pub(crate) fn finish_live_rcp_task(
    plan: fn64_runtime::RcpTaskCompletionPlan,
    rsp_steps: u64,
) -> Result<(), DeviceFault> {
    with_host(|host| {
        host.device_fabric
            .finish_rcp_task(plan, fn64_runtime::Cycles::new(rsp_steps.max(1)))
    })
}

pub(crate) fn start_live_dp_full_sync() -> Result<(), DeviceFault> {
    // Public documentation defines the FullSync -> DP-interrupt relationship,
    // but not a cycle formula. Keep the existing one-cycle compatibility
    // policy explicit at this single host/device boundary.
    with_host(|host| {
        host.device_fabric
            .start_dp_full_sync(fn64_runtime::Cycles::new(1))
    })
}

pub(crate) fn set_live_sp_pc(pc: u32) {
    with_host(|host| host.device_fabric.set_sp_pc(pc));
}

pub(crate) fn write_live_sp_status(command: u32) {
    with_host(|host| host.device_fabric.write_sp_status(command));
}

pub(crate) fn live_sp_status() -> u32 {
    with_host(|host| host.device_fabric.sp_status())
}

pub(crate) fn live_vi_field() -> u32 {
    with_host(|host| host.device_fabric.vi_field())
}

/// Complete `osSpTaskLoad`'s two documented DMA-and-poll operations against
/// the process's existing RDRAM allocation.
///
/// # Safety
/// `rdram` must remain valid through the end of the task header and aligned
/// rspboot range, as required by the shim's public ABI contract.
pub(crate) unsafe fn admit_live_sp_task(
    rdram: *mut u8,
    task_addr: RdramAddr,
    header: fn64_runtime::OsTaskHeader,
    boot: &[u8],
) -> Result<(), DeviceFault> {
    let boot_size = header
        .ucode_boot_size
        .checked_add(7)
        .map(|size| size & !7)
        .unwrap_or(header.ucode_boot_size);
    let task_end = task_addr.offset() as usize + 64;
    let boot_start = ((header.ucode_boot & 0x1fff_ffff) & !7) as usize;
    let boot_end = boot_start
        .checked_add(boot_size as usize)
        .expect("osSpTaskLoad rspboot range overflow");
    let required_len = task_end.max(boot_end);
    let view = unsafe { LiveRdramDma::new(rdram, required_len) };
    with_host(|host| {
        host.device_fabric
            .admit_sp_task_with_boot_image(&view, task_addr, header, boot)
    })
}

pub(crate) fn arm_live_vi(interval: u64) -> Result<(), DeviceFault> {
    with_host(|host| host.device_fabric.arm_vi(Cycles::new(interval)))
}

pub(crate) fn queue_live_vi_mode(registers: [u32; 14], fields: [[u32; 5]; 2]) {
    with_host(|host| {
        host.pending_vi_mode = Some(PendingViMode { registers, fields });
        // The public VI manager resets prior scale/special-feature overrides
        // when a new mode is queued. Later calls may mutate this mode image.
        host.pending_vi_control = None;
        host.pending_vi_x_scale = None;
        host.pending_vi_y_scale = None;
    });
}

pub(crate) fn queue_live_vi_x_scale(scale: f32) {
    assert!(
        scale.is_finite() && (0.25..=1.0).contains(&scale),
        "osViSetXScale: scale {scale:?} is outside the public 0.25..=1.0 range"
    );
    with_host(|host| host.pending_vi_x_scale = Some(scale));
}

pub(crate) fn queue_live_vi_y_scale(scale: f32) {
    assert!(
        scale.is_finite() && (0.05..=1.0).contains(&scale),
        "osViSetYScale: scale {scale:?} is outside the public 0.05..=1.0 range"
    );
    with_host(|host| host.pending_vi_y_scale = Some(scale));
}

fn scaled_vi_register(base: u32, scale: f32) -> u32 {
    let coefficient = ((base & 0x0fff) as f32 * scale) as u32;
    (base & !0x0fff) | coefficient
}

pub(crate) fn queue_live_vi_special_features(commands: u32) {
    const KNOWN_COMMANDS: u32 = 0xff;
    assert_eq!(
        commands & !KNOWN_COMMANDS,
        0,
        "osViSetSpecialFeatures: unknown command bits {:#010x}",
        commands & !KNOWN_COMMANDS
    );

    fn apply(mut control: u32, commands: u32) -> u32 {
        for (on_command, off_command, control_bit) in [
            (0x01, 0x02, 1 << 3),
            (0x04, 0x08, 1 << 2),
            (0x10, 0x20, 1 << 4),
            (0x40, 0x80, 1 << 16),
        ] {
            if commands & on_command != 0 {
                control |= control_bit;
            }
            if commands & off_command != 0 {
                control &= !control_bit;
            }
        }
        control
    }

    with_host(|host| {
        if let Some(mode) = host.pending_vi_mode.as_mut() {
            mode.registers[0] = apply(mode.registers[0], commands);
        } else {
            let control = host.pending_vi_control.unwrap_or_else(|| {
                host.device_fabric
                    .read_mmio(MmioAddr::new(0xA440_0000))
                    .expect("VI_STATUS register is not mapped")
            });
            host.pending_vi_control = Some(apply(control, commands));
        }
    });
}

pub(crate) fn live_ai_status() -> u32 {
    with_host(|host| host.device_fabric.ai_status())
}

pub(crate) fn live_ai_length() -> u32 {
    with_host(|host| host.device_fabric.ai_length())
}

pub(crate) fn read_live_rcp_interrupt_mmio(vaddr: u64) -> Option<u32> {
    let addr = vaddr as u32;
    with_host(|host| {
        let fabric = &mut host.device_fabric;
        match addr {
            0xA480_0018 => {
                Some(u32::from(fabric.interrupt_pending(fn64_runtime::InterruptSource::Si)) << 12)
            }
            _ => None,
        }
    })
}

/// Overlay the authoritative AI/DPC register image onto legacy sparse backing.
/// Generated C and the block lane use the typed handlers directly; this keeps
/// old hosts which still mirror the sparse window from publishing stale
/// shadow values between coroutine resumes.
///
/// # Safety
/// `rdram` must cover `RDRAM_MMIO_WINDOW_END` bytes.
pub(crate) unsafe fn sync_live_ai_dpc_mmio_into_rdram(rdram: *mut u8) {
    let registers = [
        0xA410_0000u64,
        0xA410_0004,
        0xA410_0008,
        0xA410_000C,
        0xA450_0000,
        0xA450_0004,
        0xA450_0008,
        0xA450_000C,
        0xA450_0010,
        0xA450_0014,
    ];
    let values = registers.map(|vaddr| {
        read_live_device_mmio(vaddr)
            .unwrap_or_else(|| panic!("authoritative AI/DPC register {vaddr:#018X} is unmapped"))
    });
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    for (vaddr, value) in registers.into_iter().zip(values) {
        unsafe { storage.write_u32(RdramAddr::from_gpr(vaddr), value) };
    }
}

/// Apply interrupt set/acknowledgement side effects for raw RCP register
/// writes which remain backed by [`fn64_runtime::MmioSpace`]. Public `rcp.h`
/// defines SP status bits 3/4 as clear/set SP interrupt, MI mode bit 11 as
/// clear DP interrupt, and writes to VI_CURRENT, AI_STATUS, and SI_STATUS as
/// acknowledgements of their respective interrupt lines.
pub(crate) fn observe_live_interrupt_mmio(vaddr: u64, value: u32) {
    let addr = vaddr as u32;
    match addr {
        0xA404_0010 => {
            if value & (1 << 3) != 0 {
                clear_device_interrupt(fn64_runtime::InterruptSource::Sp);
            }
            if value & (1 << 4) != 0 {
                raise_device_interrupt(fn64_runtime::InterruptSource::Sp);
            }
        }
        0xA430_0000 if value & (1 << 11) != 0 => {
            clear_device_interrupt(fn64_runtime::InterruptSource::Dp);
        }
        0xA440_0010 => clear_device_interrupt(fn64_runtime::InterruptSource::Vi),
        0xA450_000C => clear_device_interrupt(fn64_runtime::InterruptSource::Ai),
        0xA480_0018 => clear_device_interrupt(fn64_runtime::InterruptSource::Si),
        _ => {}
    }
}

pub(crate) fn read_live_device_mmio(vaddr: u64) -> Option<u32> {
    let addr = live_device_mmio_addr(vaddr, false)?;
    Some(with_host(|host| {
        let fabric = &mut host.device_fabric;
        fabric
            .read_mmio(addr)
            .unwrap_or_else(|error| panic!("raw MMIO read failed: {error}"))
    }))
}

pub(crate) fn write_live_device_mmio(vaddr: u64, value: u32) -> bool {
    let Some(addr) = live_device_mmio_addr(vaddr, true) else {
        return false;
    };
    let (effect, rdram, rdram_len) = with_host(|host| {
        let is_pi_dma_start = matches!(addr.get(), 0xA460_0008 | 0xA460_000C);
        let is_si_dma_start = matches!(addr.get(), 0xA480_0004 | 0xA480_0010);
        let is_sp_dma_start = matches!(addr.get(), 0xA404_0008 | 0xA404_000C);
        let is_host_memory_transaction_start = matches!(
            addr.get(),
            0xA410_0004 | 0xA460_0008 | 0xA460_000C | 0xA480_0004 | 0xA480_0010
        );
        if is_pi_dma_start && !host.pending_pi_completions.is_empty() {
            panic!("raw MMIO PI DMA start while the PI channel is busy");
        }
        if is_pi_dma_start && !host.rom_installed {
            panic!("raw MMIO PI DMA start has no installed cartridge ROM");
        }
        let (rdram, rdram_len) = (host.runtime_rdram, host.runtime_rdram_len);
        if (is_pi_dma_start
            || is_si_dma_start
            || is_sp_dma_start
            || is_host_memory_transaction_start)
            && (rdram.is_null() || rdram_len == 0)
        {
            panic!(
                "raw MMIO device DMA start has no registered process RDRAM; boot through the typed runtime entrypoint first"
            );
        }
        let fabric = &mut host.device_fabric;
        if addr.get() == 0xA460_000C
            && !fn64_runtime::rom::is_sram_dev_addr(fabric.snapshot().pi_cart_addr)
        {
            panic!(
                "raw MMIO PI write targets read-only cartridge ROM at device address {:#010x}",
                fabric.snapshot().pi_cart_addr
            );
        }
        let write_result = fabric.write_mmio(addr, value);
        if is_si_dma_start && matches!(write_result, Err(DeviceFault::SiBusy)) {
            return (fn64_runtime::DeviceMmioWriteEffect::None, rdram, rdram_len);
        }
        let effect = write_result.unwrap_or_else(|error| panic!("raw MMIO write failed: {error}"));
        if is_pi_dma_start {
            let request = fabric
                .pending_pi_request()
                .expect("PI length write succeeded without a pending transfer");
            let end = request
                .dram_addr
                .offset()
                .checked_add(request.len)
                .expect("raw PI DMA RDRAM range overflow") as usize;
            assert!(
                end <= rdram_len,
                "raw PI DMA RDRAM range ends at {end:#x}, beyond registered allocation {rdram_len:#x}"
            );
            host.pending_pi_completions.push_back(PendingPiCompletion {
                request,
                rdram,
                rdram_len,
                ret_queue: None,
                ret_mesg: 0,
            });
        } else if is_si_dma_start {
            let request = fabric
                .pending_si_request()
                .expect("SI start register write succeeded without a pending transfer");
            host.pending_si_completion = Some(PendingSiCompletion {
                request,
                owner: PendingSiCompletionOwner::ProcessRdram { rdram, rdram_len },
            });
        } else if addr.get() == 0xA460_0010 && value & 1 != 0 {
            host.pending_pi_completions.clear();
        }
        (effect, rdram, rdram_len)
    });
    match effect {
        fn64_runtime::DeviceMmioWriteEffect::None => {}
        effect @ (fn64_runtime::DeviceMmioWriteEffect::AiFrequencyChanged { .. }
        | fn64_runtime::DeviceMmioWriteEffect::AiDmaStarted(_)) => {
            apply_live_ai_write_effect(rdram, effect);
        }
        fn64_runtime::DeviceMmioWriteEffect::DpcSubmissionRequested(submission) => {
            assert!(
                !rdram.is_null(),
                "raw DPC submission has no registered RDRAM"
            );
            if submission.source == fn64_runtime::DpcSubmissionSource::Rdram {
                assert!(
                    submission.end as usize <= rdram_len,
                    "raw DPC command range ends at {:#010x}, beyond registered RDRAM {rdram_len:#x}",
                    submission.end
                );
            }
            unsafe { crate::task_dispatch::dispatch_dpc_submission(rdram, submission) };
        }
    }
    true
}

fn require_no_mmio_write_effect(
    result: Result<fn64_runtime::DeviceMmioWriteEffect, DeviceFault>,
    context: &'static str,
) {
    match result.unwrap_or_else(|error| panic!("{context}: {error}")) {
        fn64_runtime::DeviceMmioWriteEffect::None => {}
        effect => panic!("{context}: non-host MMIO latch produced unexpected effect {effect:?}"),
    }
}

/// Shared raw word path for typed block execution and the generated-C MMIO
/// proxy. Non-device RCP registers remain in `MmioSpace`; modeled timed
/// devices are intercepted by `DeviceFabric` first.
/// Physical-window match for direct CPU access to the 64-byte PIF RAM
/// (`0x1FC007C0..0x1FC00800`), accepting both KSEG0 and KSEG1 views. Raw
/// PIF polling is real hardware behavior (AKI-era joybus code and boot
/// handshakes); without this window such a load computes an out-of-range
/// flat-RDRAM index instead.
fn pif_ram_window_offset(vaddr: u64) -> Option<usize> {
    let upper = vaddr >> 32;
    let low = vaddr as u32;
    let physical = low & 0x1FFF_FFFF;
    ((upper == 0 || upper == u32::MAX as u64)
        && (0x8000_0000..0xC000_0000).contains(&low)
        && (0x1FC0_07C0..0x1FC0_0800).contains(&physical))
    .then(|| (physical - 0x1FC0_07C0) as usize)
}

pub(crate) fn read_raw_mmio_word(vaddr: u64) -> Option<u32> {
    if crate::boot_probe_enabled() {
        let low = vaddr as u32;
        if (0xA480_0000..0xA480_0020).contains(&low) {
            let value = read_live_rcp_interrupt_mmio(vaddr);
            eprintln!("[boot-probe] raw SI read {low:#010x} -> {value:?}");
        }
    }
    if let Some(offset) = pif_ram_window_offset(vaddr) {
        return Some(with_host(|host| {
            host.device_fabric.pif_ram_cpu_read_w(offset)
        }));
    }
    if let Some(value) = read_live_device_mmio(vaddr) {
        return Some(value);
    }
    if let Some(value) = read_live_rcp_interrupt_mmio(vaddr) {
        return Some(value);
    }
    let offset = RdramAddr::from_gpr(vaddr).offset();
    fn64_runtime::is_mmio_offset(offset).then(|| {
        crate::ai::MMIO.with(|cell| {
            cell.borrow_mut()
                .read_w(offset - fn64_runtime::RDRAM_MMIO_WINDOW_START)
        })
    })
}

pub(crate) fn write_raw_mmio_word(vaddr: u64, value: u32) -> bool {
    if crate::boot_probe_enabled() {
        let low = vaddr as u32;
        if (0xA480_0000..0xA480_0020).contains(&low) || (0xA404_0000..0xA404_0020).contains(&low) {
            eprintln!("[boot-probe] raw SI/SP write {low:#010x} = {value:#010x}");
        }
    }
    if let Some(offset) = pif_ram_window_offset(vaddr) {
        with_host(|host| host.device_fabric.pif_ram_cpu_write_w(offset, value));
        return true;
    }
    if write_live_device_mmio(vaddr, value) {
        return true;
    }
    let offset = RdramAddr::from_gpr(vaddr).offset();
    if !fn64_runtime::is_mmio_offset(offset) {
        return false;
    }
    observe_live_interrupt_mmio(vaddr, value);
    crate::ai::MMIO.with(|cell| {
        cell.borrow_mut()
            .write_w(offset - fn64_runtime::RDRAM_MMIO_WINDOW_START, value);
    });
    true
}

/// Commit due device work before any executor resume is possible.
pub(crate) fn advance_device_time(now: u64) -> u32 {
    let mut vi_retrace_ticks = 0u32;
    loop {
        let step = with_host(|host| {
            let current = host.device_fabric.now().get();
            assert!(
                now >= current,
                "device time moved backwards from {current} to {now}"
            );
            host.device_fabric
                .next_deadline()
                .filter(|deadline| deadline.get() <= now)
                .map_or(now, fn64_runtime::Cycles::get)
        });
        vi_retrace_ticks = vi_retrace_ticks
            .checked_add(advance_device_time_step(step))
            .expect("VI retrace count overflow during one virtual-time advance");
        if step == now {
            break;
        }
    }
    vi_retrace_ticks
}

/// Advance through exactly one due device deadline. Keeping notification
/// handling at this boundary lets a VI mode latch reschedule the following
/// field before the fabric advances again, while executor wakeups remain
/// deferred until the committed event has been converted to owned messages.
fn advance_device_time_step(now: u64) -> u32 {
    if crate::boot_probe_enabled() {
        let next = with_host(|host| host.device_fabric.next_deadline().map(|d| d.get()));
        if next.is_some_and(|d| d <= now) {
            eprintln!("[boot-probe] advance_device_time(now={now}) due_deadline={next:?}");
        }
    }
    enum ReadyNotification {
        External(ExternalEvent),
        ViRetrace {
            scanout: fn64_render::ViScanoutState,
            noise_seed: u64,
        },
    }

    let pending_vi_framebuffer =
        with_executor(|exec| exec.vi().next_framebuffer.map(RdramAddr::offset));
    let (events, overlays) = with_host(|host| {
        let pending_pi = host.pending_pi_completions.front().copied();
        let pending_si = host.pending_si_completion;
        let sp_memory = host
            .device_fabric
            .sp_dma_busy()
            .then_some((host.runtime_rdram, host.runtime_rdram_len));
        let memory = [
            pending_pi.map(|pending| (pending.rdram, pending.rdram_len)),
            pending_si.and_then(|pending| match pending.owner {
                PendingSiCompletionOwner::ProcessRdram { rdram, rdram_len } => {
                    Some((rdram, rdram_len))
                }
                PendingSiCompletionOwner::OsEvent | PendingSiCompletionOwner::PfsIsPlug(_) => None,
            }),
            sp_memory,
        ]
        .into_iter()
        .flatten()
        .fold(
            None::<(*mut u8, usize)>,
            |combined, (ptr, len)| match combined {
                Some((prior_ptr, prior_len)) => {
                    assert_eq!(
                        prior_ptr, ptr,
                        "concurrent device requests reference different process RDRAM allocations"
                    );
                    Some((ptr, prior_len.max(len)))
                }
                None => Some((ptr, len)),
            },
        );
        let mut raw_save_operations = Vec::new();
        let mut raw_controller_operations = Vec::new();
        let fabric = &mut host.device_fabric;
        let notifications = if let Some((rdram, rdram_len)) = memory {
            assert!(
                !rdram.is_null(),
                "pending device DMA has a null RDRAM pointer"
            );
            // SAFETY: every managed start records the live caller allocation
            // and every raw start uses the process allocation registered by
            // boot_thread0. This raw-pointer adapter deliberately does not
            // manufacture a second `&mut [u8]` while typed recompiled code's
            // dormant RDRAM view exists across a coroutine suspension.
            let mut view = unsafe { LiveRdramDma::new(rdram, rdram_len) };
            fabric
                .advance_to_with_pif(
                    Cycles::new(now),
                    &mut view,
                    |device_time, pif_ram, pi_dma| {
                        raw_save_operations.extend(pi_dma.take_save_operations());
                        let observations =
                            crate::si::execute_controller_pif(device_time, pif_ram, pi_dma);
                        raw_save_operations.extend(observations.save_operations);
                        raw_controller_operations.extend(observations.controller_operations);
                        raw_save_operations.extend(pi_dma.take_save_operations());
                    },
                )
                .unwrap_or_else(|error| panic!("device advance failed: {error}"))
        } else {
            let mut empty = fn64_runtime::RdramViewMut::from_storage(&mut []);
            fabric
                .advance_to_with_pif(
                    Cycles::new(now),
                    &mut empty,
                    |device_time, pif_ram, pi_dma| {
                        raw_save_operations.extend(pi_dma.take_save_operations());
                        let observations =
                            crate::si::execute_controller_pif(device_time, pif_ram, pi_dma);
                        raw_save_operations.extend(observations.save_operations);
                        raw_controller_operations.extend(observations.controller_operations);
                        raw_save_operations.extend(pi_dma.take_save_operations());
                    },
                )
                .unwrap_or_else(|error| panic!("device clock advance failed: {error}"))
        };

        raw_save_operations.extend(fabric.pi_dma_mut().take_save_operations());
        host.save_operations.extend(raw_save_operations);
        host.controller_operations.extend(raw_controller_operations);
        let mut events = Vec::new();
        let mut overlays = Vec::new();
        for notification in notifications {
            match notification {
                DeviceNotification::PiDmaComplete(completion) => {
                    let pending = host
                        .pending_pi_completions
                        .pop_front()
                        .expect("PI hardware completed without OS-side completion metadata");
                    assert_eq!(
                        pending.request,
                        PiDmaRequest {
                            direction: completion.direction,
                            dram_addr: completion.dram_addr,
                            cart_addr: completion.dev_addr,
                            len: completion.len,
                        },
                        "PI completion does not match the sole in-flight request"
                    );

                    if matches!(completion.direction, DmaDirection::ToRdram)
                        && !fn64_runtime::rom::is_sram_dev_addr(completion.dev_addr)
                    {
                        if let Some(static_off) = host
                            .sections
                            .plan_static_mirror(completion.dev_addr, completion.len)
                        {
                            let mut bytes = vec![0u8; completion.len as usize];
                            host.device_fabric
                                .pi_dma_mut()
                                .read_rom_bytes(completion.dev_addr, &mut bytes);
                            let storage =
                                unsafe { fn64_runtime::RdramPtr::from_storage_ptr(pending.rdram) };
                            let mirror = RdramAddr::from_offset(static_off);
                            for (index, byte) in bytes.into_iter().enumerate() {
                                unsafe {
                                    storage.write_u8(
                                        mirror
                                            .checked_add(
                                                u32::try_from(index)
                                                    .expect("PI mirror length exceeds u32"),
                                            )
                                            .expect("PI mirror logical address overflow"),
                                        byte,
                                    );
                                }
                            }
                            #[cfg(feature = "recomp-rs")]
                            fn64_recomp_rs::notify_guest_write(mirror.offset(), completion.len);
                        }
                        overlays.push((
                            completion.dev_addr,
                            completion.dram_addr.offset() | 0x8000_0000,
                            completion.len,
                        ));
                    }
                    if let Some(queue_addr) = pending.ret_queue {
                        events.push(ReadyNotification::External(ExternalEvent::DirectPost {
                            queue_addr,
                            msg: pending.ret_mesg,
                        }));
                    }
                    if let Some(next) = host.pending_pi_completions.front().copied() {
                        host.device_fabric
                            .start_pi_dma(next.request)
                            .unwrap_or_else(|error| {
                                panic!(
                                    "PI manager could not start queued request {:?}: {error}",
                                    next.request
                                )
                            });
                    }
                }
                DeviceNotification::AiDmaComplete(_) => {
                    const OS_EVENT_AI: u32 = 6;
                    events.push(ReadyNotification::External(ExternalEvent::OsEvent(
                        OS_EVENT_AI,
                    )));
                }
                DeviceNotification::SiDmaComplete(request) => {
                    if crate::boot_probe_enabled() {
                        eprintln!("[boot-probe] SiDmaComplete delivered at now={now}");
                    }
                    let pending = host
                        .pending_si_completion
                        .take()
                        .expect("SI hardware completed without OS-side completion metadata");
                    assert_eq!(pending.request, request, "SI completion request drifted");
                    match pending.owner {
                        PendingSiCompletionOwner::PfsIsPlug(transaction) => {
                            let replaced = host
                                .completed_pfs_is_plug
                                .insert(transaction.thread, transaction);
                            assert!(
                                replaced.is_none(),
                                "osPfsIsPlug completion collided with an unconsumed transaction for thread {}",
                                transaction.thread
                            );
                            events.push(ReadyNotification::External(ExternalEvent::DirectPost {
                                queue_addr: transaction.queue,
                                msg: transaction.message,
                            }));
                        }
                        PendingSiCompletionOwner::ProcessRdram { .. }
                        | PendingSiCompletionOwner::OsEvent => {
                            const OS_EVENT_SI: u32 = 5;
                            events.push(ReadyNotification::External(ExternalEvent::OsEvent(
                                OS_EVENT_SI,
                            )));
                        }
                    }
                }
                DeviceNotification::ViRetrace { at } => {
                    assert_eq!(
                        host.device_fabric.now(),
                        at,
                        "VI retrace notification escaped its device deadline"
                    );
                    if let Some(mode) = host.pending_vi_mode.take() {
                        const FIELD_REGISTER_INDICES: [usize; 5] = [1, 13, 10, 11, 3];
                        for (index, value) in mode.registers.into_iter().enumerate() {
                            if index == 4 || FIELD_REGISTER_INDICES.contains(&index) {
                                continue;
                            }
                            let addr = MmioAddr::new(
                                0xA440_0000
                                    + u32::try_from(index).expect("VI register index exceeds u32")
                                        * 4,
                            );
                            require_no_mmio_write_effect(
                                host.device_fabric.write_mmio(addr, value),
                                "VI mode register latch failed",
                            );
                        }
                        host.active_vi_mode = Some(mode);
                        host.active_vi_x_scale = 1.0;
                        host.active_vi_y_scale = 1.0;
                    }
                    if let Some(control) = host.pending_vi_control.take() {
                        require_no_mmio_write_effect(
                            host.device_fabric
                                .write_mmio(MmioAddr::new(0xA440_0000), control),
                            "VI special-feature latch failed",
                        );
                    }
                    if let Some(scale) = host.pending_vi_x_scale.take() {
                        assert!(
                            host.active_vi_mode.is_some(),
                            "osViSetXScale reached retrace without a latched OSViMode"
                        );
                        host.active_vi_x_scale = scale;
                    }
                    if let Some(scale) = host.pending_vi_y_scale.take() {
                        assert!(
                            host.active_vi_mode.is_some(),
                            "osViSetYScale reached retrace without a latched OSViMode"
                        );
                        host.active_vi_y_scale = scale;
                    }
                    if let Some(mode) = host.active_vi_mode {
                        const FIELD_REGISTER_INDICES: [usize; 5] = [1, 13, 10, 11, 3];
                        let field = host.device_fabric.vi_field() as usize;
                        require_no_mmio_write_effect(
                            host.device_fabric.write_mmio(
                                MmioAddr::new(0xA440_0030),
                                scaled_vi_register(mode.registers[12], host.active_vi_x_scale),
                            ),
                            "VI X-scale latch failed",
                        );
                        for (index, mut value) in
                            FIELD_REGISTER_INDICES.into_iter().zip(mode.fields[field])
                        {
                            if index == 1 {
                                value = pending_vi_framebuffer
                                    .unwrap_or(0)
                                    .checked_add(value)
                                    .expect("VI framebuffer plus field origin overflow")
                                    & 0x00ff_ffff;
                            } else if index == 13 {
                                value = scaled_vi_register(value, host.active_vi_y_scale);
                            }
                            let addr = MmioAddr::new(
                                0xA440_0000
                                    + u32::try_from(index).expect("VI register index exceeds u32")
                                        * 4,
                            );
                            require_no_mmio_write_effect(
                                host.device_fabric.write_mmio(addr, value),
                                "VI field register latch failed",
                            );
                        }
                    } else if let Some(framebuffer) = pending_vi_framebuffer {
                        require_no_mmio_write_effect(
                            host.device_fabric
                                .write_mmio(MmioAddr::new(0xA440_0004), framebuffer),
                            "VI framebuffer-origin latch failed",
                        );
                    }
                    let mut words = [0u32; fn64_render::ViScanoutRegisters::WORD_COUNT];
                    for (index, word) in words.iter_mut().enumerate() {
                        let address = 0xA440_0000
                            + u32::try_from(index).expect("VI register index exceeds u32") * 4;
                        *word = host
                            .device_fabric
                            .read_mmio(MmioAddr::new(address))
                            .expect("complete VI register image is not mapped");
                    }
                    let scanout = fn64_render::ViScanoutState::Registers(
                        fn64_render::ViScanoutRegisters::from_words(words),
                    );
                    events.push(ReadyNotification::ViRetrace {
                        scanout,
                        noise_seed: at.get(),
                    });
                }
                DeviceNotification::RcpTaskComplete(completion) => {
                    let code = match completion {
                        fn64_runtime::RcpTaskCompletion::Sp => 4,
                        fn64_runtime::RcpTaskCompletion::Dp => 9,
                    };
                    events.push(ReadyNotification::External(ExternalEvent::OsEvent(code)));
                }
            }
        }
        (events, overlays)
    });

    #[cfg(feature = "recomp-rs")]
    crate::recompiled::process_live_executable_writes_from_host();

    for (rom_start, dest_vram, len) in overlays {
        note_dma_overlay_load(rom_start, dest_vram, len);
    }
    let mut committed_vi_ticks = 0u32;
    if !events.is_empty() {
        let (vi_ticks, presentations) = with_executor(|exec| {
            // Interleaving closed here: checkpoint suspend -> PI/SI byte
            // commit, AI drain, or VI mode/framebuffer latch -> device state
            // current -> MI source -> OS message -> later coroutine resume.
            // DeviceFabric has completed hardware state and MI before this
            // loop; `deliver_vi_retrace` then latches manager/presentation
            // state in the same executor borrow immediately before its two
            // queue writes. No coroutine can resume between those steps.
            let mut vi_ticks = 0u32;
            let mut presentations = Vec::new();
            for notification in events {
                match notification {
                    ReadyNotification::External(event) => {
                        if let ExternalEvent::OsEvent(code) = event {
                            if !exec.event_table_contains(code) {
                                continue;
                            }
                        }
                        exec.inject_event(event);
                    }
                    ReadyNotification::ViRetrace {
                        scanout,
                        noise_seed,
                    } => {
                        vi_ticks = vi_ticks.saturating_add(1);
                        exec.deliver_vi_retrace();
                        presentations.push(fn64_render::ViPresentation {
                            blanked: exec.vi().blanked,
                            fade: exec.vi().fade,
                            repeat_line: exec.vi().repeat_line,
                            scanout,
                            noise_seed,
                        });
                    }
                }
            }
            (vi_ticks, presentations)
        });
        committed_vi_ticks = vi_ticks;
        crate::vi::note_retrace_ticks(vi_ticks);
        for presentation in presentations {
            crate::task_dispatch::present_render_backend(presentation);
        }
    }
    committed_vi_ticks
}

// ---------------------------------------------------------------------
// PI/ROM seam: osCartRomInit / osEPiStartDma / osVirtualToPhysical /
// osCreatePiManager / __osSiRawStartDma / osSetIntMask / osInitialize /
// osAiSetFrequency / osSpTaskYielded.
// ---------------------------------------------------------------------

/// `osCartRomInit(void) -> OSPiHandle*` -- no arguments (verified: every
/// real call site is `osCartRomInit_recomp(rdram, ctx)` with no register
/// setup beforehand). The PI engine remains host-owned, but the returned
/// pointer is not optional or opaque: guest code dereferences the handle's
/// public `transferInfo` fields before calling the host DMA shim. Return the
/// game-owned BSS address registered by [`set_cart_rom_handle_vram`].
///
/// OoT exposed the old no-op at `AudioLoad_Dma` ROM PC `0x800B824C`: its
/// aligned `sw $t1, 0x14($a0)` consumed `gAudioCtx.cartHandle`, which
/// `AudioLoad_Init` had populated from this return value. Leaving `$v0`
/// untouched propagated a stale, unaligned address into that store. The C
/// lane's raw memory macro tolerated the host-unaligned write; typed Rust's
/// alignment trap correctly refused it.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osCartRomInit_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    with_pi_dma("osCartRomInit_recomp", |_dma| {});
    let (handle_vram, timing) = with_host(|host| {
        let handle_vram = host.cart_rom_handle_vram.unwrap_or_else(|| {
            panic!(
                "osCartRomInit_recomp: no guest OSPiHandle address registered -- call \
                 fn64_abi::set_cart_rom_handle_vram with the ROM's aligned __CartRomHandle \
                 BSS address before boot"
            )
        });
        (
            handle_vram,
            host.device_fabric
                .pi_domain_timing(fn64_runtime::PiDomain::Domain1),
        )
    });
    unsafe {
        write_epi_handle(
            rdram,
            handle_vram,
            DEVICE_TYPE_CART,
            fn64_runtime::PiDomain::Domain1,
            timing,
            0xb000_0000,
        )
    };
    let ctx = unsafe { &mut *ctx };
    ctx.r2 = (handle_vram as i32 as i64) as u64;
}

/// `osEPiStartDma(OSPiHandle *handle, OSIoMesg *mb, s32 direction)` --
/// `a0`=handle (`ctx->r4`, decoded through the public `OSPiHandle` layout),
/// `a1`=mb (`ctx->r5`, an `OSIoMesg*`), `a2`=direction (`ctx->r6`,
/// `OS_READ`=0/`OS_WRITE`=1 per the public manual).
///
/// The `OSIoMesg` field offsets are byte-verified against the OoT decomp
/// header (`oot-decomp/include/ultra64/pi.h`) AND cross-checked against
/// DmaMgr's own stack-struct build in OOTU `funcs_0.c`
/// `DmaMgr_DmaRomToRam` (asm 0x800008F0-0x80000900): `OSIoMesgHdr` is only
/// 0x08 bytes (`type` +0x0, `pri` +0x2, `status` +0x3, `retQueue` +0x4),
/// so `dramAddr` is at +0x8, `devAddr` at +0xC, `size` at +0x10. A prior
/// wave wrongly assumed a 0xC (3-word) header and read every body field
/// +0x4 too high (size fell on the unwritten +0x14, reading 0) -- the OoT
/// `DmaMgr_Init` dmadata-DMA hang. See the inline comment at the field
/// reads below for the full store-to-field mapping. The DMA completion posts
/// through `Executor::inject_event(DirectPost)` -- the same "ONE explicit
/// host-side injection point" every other completion source uses
/// (`docs/DESIGN.md` section 2).
///
/// ## Correction (2026-07-14): must set `ctx.r2` (the `$v0` return value)
///
/// A prior wave never wrote a return value at all, leaving `ctx.r2` at
/// whatever stale value the caller's own earlier computation left there.
/// Real `osEPiStartDma` returns `s32`: 0 on successful enqueue, -1 if
/// `!__osPiDevMgr.active` (byte-identical shape confirmed against WCW
/// Revenge's `func_800219B0`,
/// `aki-recomp/refs/WCWnWoRevengeRecomp/disasm/libultra.md` ~line 213).
/// `examples/wm2000-boot`'s real boot run surfaced the consequence: the
/// chunked-DMA loop in NWXE's `func_80000660`
/// (`aki-recomp/games/NWXE/RecompiledFuncs/funcs_0.c`, asm
/// 0x800006E4-0x800006FC) re-issues `osEPiStartDma` while `$v0 != 0` and
/// only falls through to a blocking `osRecvMesg` once `$v0` reads exactly
/// 0 -- with `ctx.r2` never written, that test read garbage left over from
/// an earlier instruction (observed non-zero), so the loop re-issued the
/// same DMA chunk forever: a real, tens-of-seconds unbounded recompiled loop,
/// not a missing host model. This shim now schedules the shared deterministic
/// PI fabric and returns before bytes move. A request to write the read-only
/// cartridge ROM returns -1; successful starts return 0, and the host clock
/// later commits bytes/status/MI before posting completion. Missing host
/// wiring still traps loudly rather than being misreported as contention.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osEPiStartDma_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    unsafe { epi_start_dma_impl(rdram, ctx, true) }
}

unsafe fn epi_start_dma_impl(rdram: *mut u8, ctx: *mut RecompContext, use_handle: bool) {
    let ctx = unsafe { &mut *ctx };
    let mb_addr = RdramAddr::from_gpr(ctx.r5);
    let direction = if ctx.r6 == 0 {
        DmaDirection::ToRdram
    } else {
        DmaDirection::FromRdram
    };

    // OSIoMesg layout, byte-verified against the OoT decomp header
    // `oot-decomp/include/ultra64/pi.h`: `OSIoMesgHdr` is 0x08 bytes
    // (`u16 type` +0x0, `u8 pri` +0x2, `u8 status` +0x3, `OSMesgQueue*
    // retQueue` +0x4), NOT the 3-word (0xC) header a prior wave assumed.
    // The body follows immediately: `dramAddr` +0x8, `devAddr` +0xC,
    // `size` +0x10, `piHandle` +0x14. This exactly matches DmaMgr's own
    // struct build in OOTU `funcs_0.c` DmaMgr_DmaRomToRam (mb = $sp+0x70):
    // `sb $zero,0x72` (pri/status, +0x2), `sw $s6,0x74` (retQueue, +0x4),
    // `sw $s4,0x78` (dramAddr = a1/RAM dest, +0x8, asm 0x800008FC),
    // `sw $s2,0x7C` (devAddr = a0/romStart, +0xC, asm 0x800008F8),
    // `sw $s0,0x80` (size = chunk, +0x10, asm 0x80000900). The prior +0x4-
    // shifted offsets read dramAddr as retMesg, devAddr as dramAddr, size
    // as devAddr, and size from unwritten +0x14 (=0) -- the OoT DmaMgr_Init
    // hang: the dmadata DMA delivered len=0 and MEM_W(dest+4)!=0x1060 ->
    // Fault_AddHungupAndCrash (assert 0x345). There is no `retMesg` field.
    //
    // Correction (this wave): a prior wave called `read_stack_word` (which
    // itself calls `RdramAddr::from_gpr`, subtracting the KSEG0 base) with
    // `mb_addr.offset()` -- an ALREADY-rdram-relative offset (KSEG0 already
    // subtracted once, on line computing `mb_addr` above). Subtracting
    // KSEG0 a SECOND time produced a wildly wrong address, first caught by
    // `examples/wm2000-boot`'s actual boot run (a real EXC_BAD_ACCESS deep
    // in this function once boot finally reached its first real PI DMA,
    // thread 6's `func_800222D8` -> ... -> `osEPiStartDma_recomp` call
    // chain). Fixed via `read_offset_word` (below), a sibling helper that
    // takes an ALREADY-resolved rdram offset and does no further KSEG0
    // translation -- the two helpers now have distinct names specifically
    // so this class of double-translation mistake doesn't recur silently at
    // a future call site (per `AGENTS.md`'s "mechanism over patch": fixing
    // just this one call site without a differently-named sibling helper
    // would leave the same trap for the next `RdramAddr`-holding caller).
    let ret_queue = read_offset_word(rdram, mb_addr.offset(), 0x4);
    // No `retMesg` field exists (OSIoMesgHdr ends at retQueue); DmaMgr's
    // osRecvMesg waits on retQueue with a NULL msg-out pointer, so post a 0.
    let ret_mesg = 0u32;
    // dramAddr is a raw vram POINTER the game computed the normal way (e.g.
    // `&someBuffer`), same as any other vram value -- it needs the SAME
    // KSEG0 translation `RdramAddr::from_gpr` performs, not
    // `RdramAddr::from_offset` (which assumes the value is ALREADY an
    // rdram-relative offset with no translation needed). Using
    // `from_offset` here was a real bug (this field's value is a raw vram
    // address like any other, not a pre-resolved offset) -- caught by this
    // wave's own regression test after the sibling double-translation bug
    // (see the correction note above `read_offset_word`'s introduction).
    let dram_addr = RdramAddr::from_gpr(read_offset_word(rdram, mb_addr.offset(), 0x8) as u64);
    let mut dev_addr = read_offset_word(rdram, mb_addr.offset(), 0xC);
    let len = read_offset_word(rdram, mb_addr.offset(), 0x10);
    if use_handle {
        dev_addr =
            unsafe { resolve_epi_device_address(rdram, ctx.r4, dev_addr, "osEPiStartDma_recomp") };
    }
    if crate::boot_probe_enabled() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let n = CALLS.fetch_add(1, Ordering::Relaxed);
        if n < 12 || n.is_multiple_of(65536) {
            eprintln!(
                "[boot-probe] osEPiStartDma handle={:#010x} dev={:#x}",
                ctx.r4 as u32,
                read_offset_word(rdram, mb_addr.offset(), 0xC)
            );
        }
    }

    let logical_end = usize::try_from(dram_addr.offset().checked_add(len).unwrap_or_else(|| {
        panic!(
            "osEPiStartDma_recomp: PI DMA RDRAM range overflow -- OSIoMesg at {:#010x} carries \
             dramAddr offset {:#010x} + size {len:#010x} (devAddr {dev_addr:#010x}); a garbage \
             size like this usually means the guest OSIoMesg was never initialized by the code \
             that owns it",
            mb_addr.offset(),
            dram_addr.offset(),
        )
    }))
    .expect("PI DMA RDRAM extent exceeds usize");
    let required_len = logical_end
        .checked_add(3)
        .expect("PI DMA RDRAM storage extent overflow")
        & !3;
    let ret_queue = (ret_queue != 0).then(|| RdramAddr::from_gpr(ret_queue as u64));
    let result = start_timed_pi_dma(
        rdram,
        required_len,
        PiDmaRequest {
            direction,
            dram_addr,
            cart_addr: dev_addr,
            len,
        },
        ret_queue,
        ret_mesg,
        "osEPiStartDma_recomp",
    );
    ctx.r2 = if result.is_ok() { 0 } else { u64::MAX };
}

/// `osEPiRawStartDma(OSPiHandle *handle, s32 direction, u32 devAddr,
/// void *vAddr, u32 nbytes) -> s32`. The fifth o32 argument is at `sp+0x10`.
/// The public manual requires a two-byte transfer size/device alignment and
/// eight-byte RDRAM alignment; [`start_raw_pi_dma`] enforces those constraints
/// for ROM reads and save-device reads/writes. A write directed at the
/// read-only cartridge ROM returns -1 without posting a managed completion.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osEPiRawStartDma_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let direction = match ctx.r5 as u32 {
        0 => DmaDirection::ToRdram,
        1 => DmaDirection::FromRdram,
        value => panic!("osEPiRawStartDma_recomp: invalid direction {value}"),
    };
    let dram_addr = RdramAddr::from_gpr(ctx.r7);
    let len = unsafe { read_stack_word(rdram, ctx.r29, 0x10) };
    let dev_addr = unsafe {
        resolve_epi_device_address(rdram, ctx.r4, ctx.r6 as u32, "osEPiRawStartDma_recomp")
    };
    ctx.r2 = if start_raw_pi_dma(
        rdram,
        direction,
        dram_addr,
        dev_addr,
        len,
        "osEPiRawStartDma_recomp",
    ) {
        0
    } else {
        u64::MAX
    };
}

/// `osVirtualToPhysical(void* vaddr) -> u32` -- KSEG0/1 virtual-to-physical
/// translation (M1-WORKLIST.md #15, highest call count in the whole
/// undefined set at 104x). Per the public libultra manual: for KSEG0/KSEG1
/// addresses (the only kind generated code passes -- MIPS o32 KSEG0 base
/// `0x80000000`/KSEG1 base `0xA0000000`), physical address is simply the
/// virtual address with the top 3 bits masked off (`vaddr & 0x1FFFFFFF`) --
/// documented, standard MIPS32 segment-translation arithmetic, not a
/// runtime-specific behavior. Returns the result in `ctx->r2` (`$v0`, the
/// o32 single-word return-value register).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osVirtualToPhysical_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let vaddr = ctx.r4 as u32;
    ctx.r2 = (vaddr & 0x1FFF_FFFF) as u64;
}

/// `osCreatePiManager(OSPri pri, OSMesgQueue *cmdQ, OSMesg *cmdBuf, s32
/// cmdMsgCnt)` -- spins up the PI-manager thread. Per `docs/DESIGN.md`
/// section 2's stackful-coroutine model, "the PI manager" is not a second
/// host thread in this design (there is exactly one executor thread) --
/// its role (serializing `osEPiStartDma` requests onto the single PI bus,
/// posting completions) is owned by the ABI manager queue in front of the
/// shared timed `DeviceFabric` used by `osEPiStartDma_recomp` and raw PI MMIO.
/// The fabric still has exactly one hardware transfer; accepted managed
/// requests wait FIFO behind it instead of observing `PiBusy` as an API error.
/// This shim's remaining setup effect is
/// registering `cmdQ` as a genuine `MesgQueue` (so a real ROM's own
/// `osSendMesg`/`osRecvMesg` calls against it, if any, behave correctly),
/// matching the one piece of `osCreatePiManager`'s documented contract this
/// milestone's evidence (rung 9) actually needs: a real, non-garbage
/// message queue existing at `cmdQ`'s address.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osCreatePiManager_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let cmd_q = RdramAddr::from_gpr(ctx.r5);
    let cmd_msg_cnt = ctx.r7 as usize;
    with_executor(|exec| exec.create_mesg_queue(cmd_q, cmd_msg_cnt.max(1)));
}

/// `__osPiGetAccess(void)` -- no arguments (verified: real call site
/// `funcs_0.c` asm 0x80001608, a bare `jal` with no register setup
/// immediately before it, same no-arg shape `osCartRomInit_recomp`'s doc
/// comment already established for this corpus's PI-bus bring-up
/// sequence). Real hardware effect: acquires the PI-bus mutex so this
/// thread has exclusive access for a following DMA/IO sequence. Per
/// `docs/DESIGN.md`'s single-executor-thread model there is no real
/// concurrent PI-bus contention to arbitrate (see `osSetIntMask_recomp`'s
/// doc comment for the identical reasoning already applied to the
/// interrupt-mask shim) -- a safe no-op beyond existing as a callable
/// symbol.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osPiGetAccess_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `__osPiRelAccess(void)` -- no arguments (verified: both real call sites
/// in `funcs_0.c`, asm 0x80001628 and 0x800017B8, are bare `jal`s with no
/// register setup beforehand). Releases the mutex `__osPiGetAccess_recomp`
/// acquires; same no-op reasoning.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osPiRelAccess_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `osEPiReadIo(OSPiHandle *handle, u32 devAddr, void *dramAddr) -> s32` --
/// `a0`=handle (`ctx->r4`, the common handle/address/timing authority shared
/// with raw EPI DMA), `a1`=devAddr
/// (`ctx->r5`), `a2`=dramAddr (`ctx->r6`) -- verified against the real call
/// site (`funcs_0.c:2611`: `ctx->r4=MEM_W(...)` a handle-shaped global,
/// `ctx->r5=0x3C` a devAddr, `ctx->r6=sp+0x24` a stack dramAddr). Public
/// libultra manual: a SYNCHRONOUS single 4-byte cartridge-domain read (no
/// `OSIoMesg`/queue involved, unlike `osEPiStartDma`'s async multi-byte
/// transfer) -- reads one word directly from ROM at `devAddr` into
/// `*dramAddr`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osEPiReadIo_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    unsafe { epi_read_io_impl(rdram, ctx, true) }
}

unsafe fn epi_read_io_impl(rdram: *mut u8, ctx: *mut RecompContext, use_handle: bool) {
    let ctx = unsafe { &mut *ctx };
    let dev_addr = if use_handle {
        unsafe { resolve_epi_device_address(rdram, ctx.r4, ctx.r5 as u32, "osEPiReadIo_recomp") }
    } else {
        ctx.r5 as u32
    };
    let dram_addr = RdramAddr::from_gpr(ctx.r6).offset() as usize;
    let record_sram = with_pi_dma("osEPiReadIo_recomp", |dma| {
        let mut buf = [0u8; 4];
        let record_sram = if fn64_runtime::rom::is_sram_dev_addr(dev_addr) {
            dma.sram_read_into(dev_addr - fn64_runtime::rom::SRAM_DOMAIN2_BASE, &mut buf);
            dma.save_len() == Some(fn64_runtime::SaveType::SramBanked.byte_len())
        } else {
            dma.read_rom_bytes(dev_addr, &mut buf);
            false
        };
        // Same word-swizzle as PiDma::start_dma / Rdram::write_bytes: rdram is
        // native-endian-WORD storage, so a big-endian cartridge word must be
        // stored byte-reversed or a later MEM_W/MEM_BU reads it swapped. A flat
        // copy here is exactly the bug that hung OoT's Locale_Init region check.
        let swz = [buf[3], buf[2], buf[1], buf[0]];
        unsafe {
            std::ptr::copy_nonoverlapping(swz.as_ptr(), rdram.add(dram_addr), 4);
        }
        record_sram
    });
    if record_sram {
        crate::record_save_operation(
            fn64_runtime::SaveType::SramBanked,
            fn64_runtime::SaveOperationKind::Read,
            (dev_addr - fn64_runtime::rom::SRAM_DOMAIN2_BASE) as usize,
            4,
        );
    }
    ctx.r2 = 0;
}

/// `osPiReadIo(u32 devAddr, u32 *data) -> s32`, the cartridge-handle-free
/// managed counterpart to `osEPiReadIo`. Both public APIs perform the same
/// synchronous 32-bit PI read, so this adapter remaps their argument
/// registers and reuses the one byte-order-tested implementation.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osPiReadIo_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let dev_addr = ctx.r4;
    let data = ctx.r5;
    ctx.r5 = dev_addr;
    ctx.r6 = data;
    unsafe { epi_read_io_impl(rdram, ctx, false) };
}

unsafe fn write_io_mesg_word(rdram: *mut u8, mb: RdramAddr, offset: u32, value: u32) {
    let address = mb
        .checked_add(offset)
        .expect("OSIoMesg field address overflow")
        .offset() as usize;
    unsafe {
        std::ptr::copy_nonoverlapping(value.to_ne_bytes().as_ptr(), rdram.add(address), 4);
    }
}

/// `osPiStartDma(OSIoMesg *mb, s32 priority, s32 direction, u32 devAddr,
/// void *vAddr, u32 nbytes, OSMesgQueue *mq) -> s32`. The first four o32
/// arguments occupy r4-r7; vAddr/nbytes/mq are at sp+0x10/+0x14/+0x18.
/// This managed wrapper populates the public `OSIoMesg` body and delegates to
/// the same EPI transfer/completion/overlay path used by `osEPiStartDma`.
///
/// # Safety
/// Same raw guest-memory contract as every other DMA shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osPiStartDma_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mb = RdramAddr::from_gpr(ctx.r4);
    let priority = ctx.r5 as u8;
    let direction = ctx.r6;
    let dev_addr = ctx.r7 as u32;
    let dram_addr = unsafe { read_stack_word(rdram, ctx.r29, 0x10) };
    let nbytes = unsafe { read_stack_word(rdram, ctx.r29, 0x14) };
    let queue = unsafe { read_stack_word(rdram, ctx.r29, 0x18) };

    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    unsafe {
        storage.write_u8(
            mb.checked_add(2)
                .expect("OSIoMesg priority address overflow"),
            priority,
        );
        storage.write_u8(
            mb.checked_add(3).expect("OSIoMesg status address overflow"),
            0,
        );
        write_io_mesg_word(rdram, mb, 0x4, queue);
        write_io_mesg_word(rdram, mb, 0x8, dram_addr);
        write_io_mesg_word(rdram, mb, 0xC, dev_addr);
        write_io_mesg_word(rdram, mb, 0x10, nbytes);
    }

    ctx.r5 = mb.to_kseg0() as i32 as u64;
    ctx.r6 = direction;
    unsafe { epi_start_dma_impl(rdram, ctx, false) };
}

/// `osPiGetStatus(void) -> u32`. Reads the same authoritative PI status used
/// by raw MMIO and timed managed transfers.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osPiGetStatus_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let status = with_host(|host| host.device_fabric.snapshot().pi_status);
    unsafe { &mut *ctx }.r2 = status as u64;
}

/// `osEPiWriteIo(OSPiHandle *handle, u32 devAddr, u32 data) -> s32` --
/// the synchronous single-word counterpart to `osEPiReadIo_recomp`. The
/// shared handle resolver applies `baseAddress | devAddr` and publishes the
/// handle's bus timing through the raw PI registers. SRAM writes reach the
/// same `PiDma` save store as DMA; cartridge-ROM writes return -1 because the
/// installed ROM source is read-only.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osEPiWriteIo_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let dev_addr =
        unsafe { resolve_epi_device_address(rdram, ctx.r4, ctx.r5 as u32, "osEPiWriteIo_recomp") };
    if fn64_runtime::rom::is_sram_dev_addr(dev_addr) {
        let record_sram = with_pi_dma("osEPiWriteIo_recomp", |dma| {
            dma.sram_write_from(
                dev_addr - fn64_runtime::rom::SRAM_DOMAIN2_BASE,
                &(ctx.r6 as u32).to_be_bytes(),
            );
            dma.save_len() == Some(fn64_runtime::SaveType::SramBanked.byte_len())
        });
        if record_sram {
            crate::record_save_operation(
                fn64_runtime::SaveType::SramBanked,
                fn64_runtime::SaveOperationKind::Write,
                (dev_addr - fn64_runtime::rom::SRAM_DOMAIN2_BASE) as usize,
                4,
            );
        }
        ctx.r2 = 0;
    } else {
        ctx.r2 = u64::MAX;
    }
}

/// `osLeoDiskInit(void) -> OSPiHandle *` -- constructs the public EPI handle
/// for the N64 Disk Drive register range. Public Chapter 27 documentation
/// defines device type 2 and the domain-2 physical address 0x0500_0000; the
/// public handle stores its uncached KSEG1 form 0xA500_0000, matching Chapter
/// 27's acquisition pattern. Timing parameters are supplied by the host
/// configuration rather than guessed. A retail boot that reaches this path
/// without configuring a drive traps with setup context instead of inventing
/// attached hardware.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osLeoDiskInit_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let config = with_host(|host| {
        host.leo_disk.unwrap_or_else(|| {
            panic!(
                "osLeoDiskInit_recomp: no 64DD configuration installed -- call \
                 fn64_abi::configure_leo_disk with guest handle storage and hardware-derived PI \
                 timing before entering a disk-manager path"
            )
        })
    });
    unsafe {
        write_epi_handle(
            rdram,
            config.handle_vram,
            DEVICE_TYPE_64DD,
            fn64_runtime::PiDomain::Domain2,
            fn64_runtime::PiDomainTiming {
                latency: config.latency,
                page_size: config.page_size,
                release: config.release,
                pulse_width: config.pulse_width,
            },
            0xa500_0000,
        )
    }
    unsafe { &mut *ctx }.r2 = (config.handle_vram as i32 as i64) as u64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    fn install_cart_handle(rdram: &mut [u8], offset: u32) -> u64 {
        let handle_vram = 0x8000_0000 | offset;
        set_cart_rom_handle_vram(handle_vram);
        let mut ctx = ctx_zeroed();
        unsafe { osCartRomInit_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2 as u32, handle_vram);
        ctx.r2
    }

    fn install_sram_handle(
        rdram: &mut [u8],
        offset: u32,
        timing: fn64_runtime::PiDomainTiming,
    ) -> u64 {
        let handle_vram = 0x8000_0000 | offset;
        unsafe {
            write_epi_handle(
                rdram.as_mut_ptr(),
                handle_vram,
                DEVICE_TYPE_SRAM,
                fn64_runtime::PiDomain::Domain2,
                timing,
                0xa800_0000,
            )
        };
        handle_vram as i32 as u64
    }

    #[test]
    fn loading_a_rom_clears_prior_rsp_rdp_observations() {
        load_rom(vec![0]);
        crate::record_rsp_rdp_observations(vec![crate::RspRdpObservationKind::DramDpcCommitted {
            start: 0,
            end: 8,
            command_sha256: [0x5a; 32],
        }]);
        assert_eq!(crate::copy_rsp_rdp_observations().len(), 1);

        load_rom(vec![1]);

        assert!(crate::copy_rsp_rdp_observations().is_empty());
    }

    #[test]
    fn synchronous_pi_boundaries_preserve_same_cycle_cross_owner_save_order() {
        load_rom(vec![0; 0x100]);
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::Eeprom4k,
        )));

        with_pi_dma("same-cycle save ordering", |dma| {
            dma.eeprom_read_block(Cycles::ZERO, 0).unwrap();
        });
        crate::record_save_operation(
            fn64_runtime::SaveType::ControllerPak,
            fn64_runtime::SaveOperationKind::Read,
            0x20,
            fn64_runtime::pfs::PFS_BLOCK_SIZE,
        );
        with_pi_dma("same-cycle save ordering", |dma| {
            dma.eeprom_read_block(Cycles::ZERO, 1).unwrap();
        });

        assert_eq!(
            crate::copy_save_operations()
                .iter()
                .map(|event| (event.device, event.offset))
                .collect::<Vec<_>>(),
            vec![
                (fn64_runtime::SaveType::Eeprom4k, 0),
                (fn64_runtime::SaveType::ControllerPak, 0x20),
                (
                    fn64_runtime::SaveType::Eeprom4k,
                    fn64_runtime::save::EEPROM_BLOCK_SIZE as u32,
                ),
            ]
        );
    }

    #[test]
    fn sram_evidence_uses_pi_commit_cycle_not_outer_advance_target() {
        load_rom(vec![0; 0x100]);
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::SramBanked,
        )));
        let mut rdram = vec![0u8; 64];
        let started_at = crate::sim_time();
        start_timed_pi_dma(
            rdram.as_mut_ptr(),
            rdram.len(),
            PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0),
                cart_addr: fn64_runtime::rom::SRAM_DOMAIN2_BASE,
                len: 4,
            },
            None,
            0,
            "SRAM evidence timing test",
        )
        .unwrap();
        crate::advance_virtual_time(started_at + 9);

        assert_eq!(
            crate::copy_save_operations(),
            vec![fn64_runtime::SaveOperationEvent {
                at: Cycles::new(started_at + 1),
                device: fn64_runtime::SaveType::SramBanked,
                operation: fn64_runtime::SaveOperationKind::Read,
                offset: 0,
                len: 4,
            }]
        );
    }

    #[test]
    fn typed_cartridge_save_configuration_is_exact_release_evidence() {
        load_rom(vec![0; 0x100]);
        set_cartridge_save(
            CartridgeSaveType::SramBanked,
            Box::new(fn64_runtime::InMemorySaveStorage::new(
                CartridgeSaveType::SramBanked.byte_len(),
            )),
        );
        assert_eq!(
            host_evidence_snapshot().cartridge_save,
            CartridgeSaveEvidenceSnapshot::Configured(CartridgeSaveType::SramBanked)
        );

        load_rom(vec![1; 0x100]);
        assert_eq!(
            host_evidence_snapshot().cartridge_save,
            CartridgeSaveEvidenceSnapshot::Unidentified
        );
        configure_no_cartridge_save();
        assert_eq!(
            host_evidence_snapshot().cartridge_save,
            CartridgeSaveEvidenceSnapshot::NoCartridgeSave
        );
    }

    #[test]
    fn legacy_or_wrong_sized_save_configuration_cannot_claim_a_type() {
        load_rom(vec![0; 0x100]);
        let wrong_size = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            set_cartridge_save(
                CartridgeSaveType::Eeprom4k,
                Box::new(fn64_runtime::InMemorySaveStorage::new(511)),
            );
        }));
        assert!(wrong_size.is_err());
        assert_eq!(
            host_evidence_snapshot().cartridge_save,
            CartridgeSaveEvidenceSnapshot::Unidentified
        );

        set_save(Box::new(fn64_runtime::InMemorySaveStorage::new(512)));
        assert_eq!(
            host_evidence_snapshot().cartridge_save,
            CartridgeSaveEvidenceSnapshot::Unidentified
        );
        let relabel = std::panic::catch_unwind(configure_no_cartridge_save);
        assert!(relabel.is_err());
    }

    #[test]
    fn raw_pif_ram_window_round_trips_through_the_device_fabric() {
        // KSEG1 and KSEG0 views hit the same 64-byte PIF RAM; the boot
        // handshake's status word lives in the final word (0x1FC007FC).
        assert_eq!(read_raw_mmio_word(0xFFFF_FFFF_BFC0_07C0), Some(0));
        assert!(write_raw_mmio_word(0xFFFF_FFFF_BFC0_07C8, 0xDEAD_BEEF));
        assert_eq!(read_raw_mmio_word(0xFFFF_FFFF_BFC0_07C8), Some(0xDEAD_BEEF));
        assert_eq!(read_raw_mmio_word(0xFFFF_FFFF_9FC0_07C8), Some(0xDEAD_BEEF));
        // The final-word store runs the PIF command interpreter; a zero
        // command byte must leave a readable (non-faulting) window behind.
        assert!(write_raw_mmio_word(0xFFFF_FFFF_BFC0_07FC, 0));
        assert_eq!(read_raw_mmio_word(0xFFFF_FFFF_BFC0_07FC), Some(0));
        assert!(write_raw_mmio_word(0x0000_0000_9FC0_07CC, 0x1234_5678));
        assert_eq!(read_raw_mmio_word(0x0000_0000_BFC0_07CC), Some(0x1234_5678));
        // One byte past the window is NOT PIF RAM.
        assert_eq!(pif_ram_window_offset(0xFFFF_FFFF_BFC0_0800), None);
        assert_eq!(pif_ram_window_offset(0x0000_0000_1FC0_07C0), None);
        assert_eq!(pif_ram_window_offset(0xFFFF_FFFF_DFC0_07C0), None);
        assert_eq!(pif_ram_window_offset(0x0000_0001_BFC0_07C0), None);
        assert_eq!(read_raw_mmio_word(0x0000_0000_1FC0_07C0), None);
        assert!(!write_raw_mmio_word(0xFFFF_FFFF_DFC0_07C0, 1));
        assert_eq!(live_device_mmio_addr(0x0000_0001_A440_0000, false), None);
    }

    fn complete_pi_dma() {
        let deadline = with_host(|host| {
            host.device_fabric
                .next_deadline()
                .expect("test expected one pending PI deadline")
                .get()
        });
        advance_virtual_time(deadline);
    }

    #[test]
    fn live_rdram_dma_bounds_logical_bytes_by_complete_native_words() {
        let mut storage = [0u8; 4];
        let mut dma = unsafe { LiveRdramDma::new(storage.as_mut_ptr(), storage.len()) };

        fn64_runtime::DmaMemory::dma_write_bytes(&mut dma, 3, &[0xA5]);
        assert_eq!(
            fn64_runtime::RdramView::from_storage(&storage).read_u8(RdramAddr::from_offset(3)),
            0xA5
        );

        let outside = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fn64_runtime::DmaMemory::dma_write_bytes(&mut dma, 4, &[0x5A]);
        }));
        assert!(outside.is_err(), "one-past-end PI DMA byte must trap");
    }

    #[test]
    fn managed_pi_dma_commits_state_then_posts_completion_before_resume() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        load_rom_with_fixed_pi_latency(rom, 5);

        let mut rdram = vec![0u8; 0x1000];
        let cart_handle = install_cart_handle(&mut rdram, 0x800);
        let queue = RdramAddr::from_offset(0x300);
        with_executor(|exec| exec.create_mesg_queue(queue, 1));
        let mb = 0x100usize;
        rdram[mb + 0x4..mb + 0x8].copy_from_slice(&0x8000_0300u32.to_ne_bytes());
        rdram[mb + 0x8..mb + 0xC].copy_from_slice(&0x8000_0400u32.to_ne_bytes());
        rdram[mb + 0xC..mb + 0x10].copy_from_slice(&0x20u32.to_ne_bytes());
        rdram[mb + 0x10..mb + 0x14].copy_from_slice(&4u32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = 0x8000_0100;
        ctx.r6 = 0;
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        assert_eq!(&rdram[0x400..0x404], &[0, 0, 0, 0]);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().pi_status),
            fn64_runtime::PI_STATUS_DMA_BUSY
        );
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, queue, false)),
            fn64_runtime::RecvMesgOutcome::WouldBlock
        );

        advance_virtual_time(4);
        assert_eq!(&rdram[0x400..0x404], &[0, 0, 0, 0]);
        advance_virtual_time(5);

        assert_eq!(
            u32::from_ne_bytes(rdram[0x400..0x404].try_into().unwrap()),
            0xDEAD_BEEF
        );
        let snapshot = with_host(|host| host.device_fabric.snapshot());
        assert_eq!(snapshot.pi_status, 0);
        assert_ne!(
            snapshot.mi_pending & fn64_runtime::InterruptSource::Pi.bit(),
            0
        );
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, queue, false)),
            fn64_runtime::RecvMesgOutcome::Delivered(0)
        );
        let kinds = with_host(|host| {
            host.device_fabric
                .trace()
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>()
        });
        assert!(matches!(
            kinds[0],
            fn64_runtime::DeviceTraceKind::PiDmaStarted(_)
        ));
        assert!(matches!(
            kinds[1],
            fn64_runtime::DeviceTraceKind::PiBytesCommitted(_)
        ));
        assert_eq!(kinds[2], fn64_runtime::DeviceTraceKind::PiBusyCleared);
        assert_eq!(
            kinds[3],
            fn64_runtime::DeviceTraceKind::MiInterruptRaised(fn64_runtime::InterruptSource::Pi)
        );
        assert!(matches!(
            kinds[4],
            fn64_runtime::DeviceTraceKind::NotificationReady(_)
        ));
        assert_eq!(
            copy_device_trace()
                .into_iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            kinds,
            "public release-evidence accessor must copy the fabric-owned trace verbatim"
        );
    }

    /// Regression for the real OoT interleaving: the object-loading thread
    /// submitted DmaMgr's second chunk while another guest thread's managed
    /// PI request still owned the hardware channel. Exposing `PiBusy` made
    /// DmaMgr return after its first 0x2000-byte chunk and left the display
    /// list tail zero. Both calls must succeed immediately, while bytes and
    /// completion posts remain strictly FIFO at their separate deadlines.
    #[test]
    fn managed_pi_dma_serializes_concurrent_callers_fifo() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        rom[0x40..0x44].copy_from_slice(&[0x55, 0x66, 0x77, 0x88]);
        load_rom_with_fixed_pi_latency(rom, 5);

        let mut rdram = vec![0u8; 0x1000];
        let cart_handle = install_cart_handle(&mut rdram, 0x800);
        let first_queue = RdramAddr::from_offset(0x300);
        let second_queue = RdramAddr::from_offset(0x340);
        with_executor(|exec| {
            exec.create_mesg_queue(first_queue, 1);
            exec.create_mesg_queue(second_queue, 1);
        });

        let write_mb = |rdram: &mut [u8], mb: usize, queue: u32, dram: u32, dev: u32| {
            rdram[mb + 0x4..mb + 0x8].copy_from_slice(&queue.to_ne_bytes());
            rdram[mb + 0x8..mb + 0xC].copy_from_slice(&dram.to_ne_bytes());
            rdram[mb + 0xC..mb + 0x10].copy_from_slice(&dev.to_ne_bytes());
            rdram[mb + 0x10..mb + 0x14].copy_from_slice(&4u32.to_ne_bytes());
        };
        write_mb(&mut rdram, 0x100, 0x8000_0300, 0x8000_0400, 0x20);
        write_mb(&mut rdram, 0x140, 0x8000_0340, 0x8000_0440, 0x40);

        let mut first = ctx_zeroed();
        first.r4 = cart_handle;
        first.r5 = 0x8000_0100;
        first.r6 = 0;
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut first) };
        assert_eq!(first.r2, 0);

        let mut second = ctx_zeroed();
        second.r4 = cart_handle;
        second.r5 = 0x8000_0140;
        second.r6 = 0;
        second.r2 = 0xBAD0_BAD0;
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut second) };
        assert_eq!(second.r2, 0, "queued managed PI work is accepted");
        assert_eq!(with_host(|host| host.pending_pi_completions.len()), 2);

        advance_virtual_time(5);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x400..0x404].try_into().unwrap()),
            0x1122_3344
        );
        assert_eq!(&rdram[0x440..0x444], &[0, 0, 0, 0]);
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, first_queue, false)),
            fn64_runtime::RecvMesgOutcome::Delivered(0)
        );
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, second_queue, false)),
            fn64_runtime::RecvMesgOutcome::WouldBlock
        );
        assert_eq!(with_host(|host| host.pending_pi_completions.len()), 1);

        advance_virtual_time(10);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x440..0x444].try_into().unwrap()),
            0x5566_7788
        );
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, second_queue, false)),
            fn64_runtime::RecvMesgOutcome::Delivered(0)
        );
        assert!(with_host(|host| host.pending_pi_completions.is_empty()));
    }

    #[test]
    fn mi_authority_exists_before_cartridge_rom_is_installed() {
        let source = fn64_runtime::InterruptSource::Sp;
        set_mi_interrupt_mask(source.bit());
        raise_device_interrupt(source);
        assert!(cpu_interrupt_pending());
        clear_device_interrupt(source);
        assert!(!cpu_interrupt_pending());
        assert!(!with_host(|host| host.rom_installed));
    }

    #[test]
    fn os_virtual_to_physical_masks_kseg0() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_1234;
        unsafe { osVirtualToPhysical_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0x0000_1234);
    }

    #[test]
    fn os_virtual_to_physical_masks_kseg1() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0xA000_5678;
        unsafe { osVirtualToPhysical_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0x0000_5678);
    }

    #[test]
    fn leo_disk_init_returns_a_distinct_public_domain2_handle() {
        let mut rdram = vec![0u8; 0x2000];
        configure_leo_disk(LeoDiskConfig {
            handle_vram: 0x8000_1000,
            latency: 0x12,
            page_size: 0x0D,
            release: 0x02,
            pulse_width: 0x34,
        });
        let mut ctx = ctx_zeroed();
        unsafe { osLeoDiskInit_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0xFFFF_FFFF_8000_1000);

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let base = RdramAddr::from_offset(0x1000);
        assert_eq!(view.read_u32(base), 0);
        assert_eq!(view.read_u8(base.checked_add(4).unwrap()), 2);
        assert_eq!(view.read_u8(base.checked_add(5).unwrap()), 0x12);
        assert_eq!(view.read_u8(base.checked_add(6).unwrap()), 0x0D);
        assert_eq!(view.read_u8(base.checked_add(7).unwrap()), 0x02);
        assert_eq!(view.read_u8(base.checked_add(8).unwrap()), 0x34);
        assert_eq!(view.read_u8(base.checked_add(9).unwrap()), 1);
        assert_eq!(view.read_u32(base.checked_add(12).unwrap()), 0xa500_0000);
        assert_eq!(view.read_u32(base.checked_add(16).unwrap()), 0);
    }

    #[test]
    fn os_pi_start_dma_marshals_stack_arguments_into_the_shared_epi_path() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        load_rom(rom);
        let mut rdram = vec![0u8; 0x400];
        let stack = 0x40usize;
        rdram[stack + 0x10..stack + 0x14].copy_from_slice(&0x8000_0200u32.to_ne_bytes());
        rdram[stack + 0x14..stack + 0x18].copy_from_slice(&4u32.to_ne_bytes());
        rdram[stack + 0x18..stack + 0x1C].copy_from_slice(&0u32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0100;
        ctx.r5 = 1;
        ctx.r6 = 0;
        ctx.r7 = 0x20;
        ctx.r29 = 0x8000_0040;
        unsafe { osPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
        complete_pi_dma();

        assert_eq!(ctx.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x200..0x204].try_into().unwrap()),
            0x1234_5678
        );
        assert_eq!(
            u32::from_ne_bytes(rdram[0x108..0x10C].try_into().unwrap()),
            0x8000_0200
        );
        assert_eq!(
            u32::from_ne_bytes(rdram[0x10C..0x110].try_into().unwrap()),
            0x20
        );
        assert_eq!(
            u32::from_ne_bytes(rdram[0x110..0x114].try_into().unwrap()),
            4
        );
    }

    #[test]
    fn os_pi_read_io_remaps_both_arguments_without_losing_the_data_pointer() {
        let mut rom = vec![0u8; 0x80];
        rom[0x20..0x24].copy_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
        load_rom(rom);
        let mut rdram = vec![0u8; 0x100];
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x20;
        ctx.r5 = 0x8000_0040;
        unsafe { osPiReadIo_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x40..0x44].try_into().unwrap()),
            0xCAFE_BABE
        );

        let mut status = ctx_zeroed();
        status.r2 = 0xDEAD_BEEF;
        unsafe { osPiGetStatus_recomp(std::ptr::null_mut(), &mut status) };
        assert_eq!(status.r2, 0);
    }

    /// Regression for OoT rs boot's `AudioLoad_Dma` alignment trap.
    /// `AudioLoad_Init` stores `osCartRomInit()`'s `$v0` into
    /// `gAudioCtx.cartHandle`; ROM PC 0x800B824C later executes the ordinary
    /// aligned `sw $t1, 0x14($a0)` through that pointer. The old shim left
    /// `$v0` untouched. Seed the exact stale value observed at the failing
    /// boot so that implementation returns `0x80125636` and fails this test,
    /// while the fixed shim returns the configured aligned guest handle.
    #[test]
    fn os_cart_rom_init_replaces_stale_unaligned_v0_with_guest_handle() {
        load_rom(vec![0u8; 0x100]);
        set_cart_rom_handle_vram(0x8000_9EA0);
        let mut rdram = vec![0u8; 0x9f00];

        let mut ctx = ctx_zeroed();
        ctx.r2 = 0xFFFF_FFFF_8012_5636;
        unsafe { osCartRomInit_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(ctx.r2, 0xFFFF_FFFF_8000_9EA0);
        assert_eq!(ctx.r2 & 3, 0, "returned OSPiHandle must be word-aligned");
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let handle = RdramAddr::from_offset(0x9ea0);
        assert_eq!(
            view.read_u8(handle.checked_add(4).unwrap()),
            DEVICE_TYPE_CART
        );
        assert_eq!(view.read_u8(handle.checked_add(9).unwrap()), PI_DOMAIN1);
        assert_eq!(view.read_u32(handle.checked_add(12).unwrap()), 0xb000_0000);
    }

    /// Regression test for the real double-KSEG0-translation bug
    /// `examples/wm2000-boot`'s boot run surfaced (a genuine
    /// EXC_BAD_ACCESS deep in `osEPiStartDma_recomp`'s field reads, once
    /// boot finally reached its first real PI DMA on thread 6): `mb_addr`
    /// is placed at a REALISTIC nonzero vram address (not offset 0, which
    /// would hide the bug -- 0 minus 0 is still 0), and the OSIoMesg
    /// fields are placed at their real rdram offsets relative to that vram
    /// address, not relative to 0.
    /// Builds an OSIoMesg exactly as OOTU `DmaMgr_DmaRomToRam` does
    /// (`funcs_0.c` asm 0x800008F0-0x80000900): 0x08-byte `OSIoMesgHdr`
    /// (retQueue at +0x4), then `dramAddr` +0x8, `devAddr` +0xC, `size`
    /// +0x10. The prior version of this test placed fields +0x4 too high to
    /// match the buggy 0xC-header shim, so it passed green against the bug --
    /// the exact "weak green check" trap. A NON-UNIFORM multi-word ROM
    /// payload and a NON-ZERO multi-word `size` make a wrong-offset read
    /// (which would pick up size=0, or the wrong devAddr) fail loudly.
    #[test]
    fn os_epi_start_dma_reads_real_fields_at_a_nonzero_mb_address() {
        // Use a fresh ROM per test (with_pi_dma's HOST state is thread-local
        // per test since each #[test] gets its own OS thread by default).
        // Non-uniform big-endian cart words at devAddr 0x40 so a flat
        // (non-swizzled) DMA, a wrong devAddr, or a truncated len all fail.
        let mut rom = vec![0u8; 0x1000];
        let dev_addr: u32 = 0x40;
        rom[0x40..0x44].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        rom[0x44..0x48].copy_from_slice(&[0x00, 0x00, 0x10, 0x60]); // 0x1060 -- DmaMgr's sentinel
        rom[0x48..0x4C].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        load_rom(rom);

        let mut rdram = vec![0u8; 0x10000];
        let cart_handle = install_cart_handle(&mut rdram, 0x1000);
        let mb_vram: u64 = 0x8000_2000; // a REAL, nonzero vram address
        let mb_offset = 0x2000usize;

        // OSIoMesg fields at mb_offset + {retQueue +0x4, dramAddr +0x8,
        // devAddr +0xC, size +0x10} -- native byte order, DmaMgr's real
        // layout (0x08-byte OSIoMesgHdr).
        let dram_target_vram: u32 = 0x8000_5000;
        let size: u32 = 0xC; // 3 words -- non-zero, multi-word
        rdram[mb_offset + 0x4..mb_offset + 0x8].copy_from_slice(&0u32.to_ne_bytes()); // no retQueue
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&dram_target_vram.to_ne_bytes());
        rdram[mb_offset + 0xC..mb_offset + 0x10].copy_from_slice(&dev_addr.to_ne_bytes());
        rdram[mb_offset + 0x10..mb_offset + 0x14].copy_from_slice(&size.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = mb_vram;
        ctx.r6 = 0; // OS_READ / ToRdram
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        complete_pi_dma();

        // dramAddr (0x8000_5000) -> rdram offset 0x5000. Each big-endian
        // cart word must arrive so the guest's MEM_W reads it intact; rdram
        // is native-word storage, so physical bytes are byte-reversed. A
        // wrong offset would read size=0 (delivering nothing) or the wrong
        // devAddr; a flat copy would byte-reverse the words.
        let w0 = u32::from_ne_bytes(rdram[0x5000..0x5004].try_into().unwrap());
        let w1 = u32::from_ne_bytes(rdram[0x5004..0x5008].try_into().unwrap());
        let w2 = u32::from_ne_bytes(rdram[0x5008..0x500C].try_into().unwrap());
        assert_eq!(w0, 0x1234_5678, "first ROM word must be delivered intact");
        assert_eq!(
            w1, 0x0000_1060,
            "second word (DmaMgr's 0x1060 sentinel) proves the full size was read, not 0/one word"
        );
        assert_eq!(w2, 0xDEAD_BEEF, "third word confirms the exact len (0xC)");
        // And nothing spilled past the declared length.
        let after = u32::from_ne_bytes(rdram[0x500C..0x5010].try_into().unwrap());
        assert_eq!(after, 0, "DMA must not write past size (0xC bytes)");
    }

    /// Regression test for the OoT-boot hang (2026-07-14): `osEPiReadIo`
    /// delivered the cartridge word into rdram FLAT, but the guest reads
    /// individual bytes back through `MEM_BU`'s `^3` byte-lane XOR (rdram is
    /// native-endian-word storage). `Locale_Init` DMAs the ROM header, `lbu`s
    /// the region byte, accepts only 'E'/'J', else `LogUtils_HungupThread`s.
    /// A flat copy delivered the wrong byte -> neither-E-nor-J -> deliberate
    /// hang. This models that exact read with a distinguishable word so a
    /// regression to flat semantics fails here, not 8 frames into a boot.
    #[test]
    fn os_epi_read_io_word_reads_back_through_mem_bu_unswapped() {
        // ROM word at devAddr 0x3C = `5A 4C 4A 00` (OoT's real `Z L J \0`);
        // guest wants MEM_BU(dram+2) == 0x4A ('J').
        let mut rom = vec![0u8; 0x100];
        rom[0x3C..0x40].copy_from_slice(&[0x5A, 0x4C, 0x4A, 0x00]);
        load_rom(rom);

        let mut rdram = vec![0u8; 0x1000];
        let cart_handle = install_cart_handle(&mut rdram, 0x100);
        let dram_vram: u64 = 0x8000_0024;
        let dram_off = 0x24usize;

        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = 0x3C; // devAddr
        ctx.r6 = dram_vram; // dramAddr
        unsafe { osEPiReadIo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // MEM_BU(dram_off ^ 3) is the guest's byte read; +2 must be 'J'.
        assert_eq!(rdram[dram_off ^ 3], 0x5A); // 'Z'
        assert_eq!(rdram[(dram_off + 2) ^ 3], 0x4A); // 'J' -- the region byte
                                                     // And MEM_W reads the cart word intact (native-endian word storage).
        let w = u32::from_ne_bytes(rdram[dram_off..dram_off + 4].try_into().unwrap());
        assert_eq!(w, 0x5A4C_4A00);
    }

    /// Regression test for the SRAM-DMA-treated-as-ROM crash (2026-07-15):
    /// OoT's `Sram_InitSram -> SsSram_ReadWrite -> SsSram_Dma` issues a PI DMA
    /// with `devAddr = 0x08000000` (PI_DOM2_ADDR2, the SRAM cartridge base --
    /// rcp.h:714), which the old `osEPiStartDma_recomp` blindly read from the
    /// ROM image -> `InMemoryRom::read_into` past the 55MB ROM -> loud trap.
    /// The fix routes domain-2 devAddrs to the registered `SaveStorage`.
    ///
    /// Drives the REAL raw-pointer shim path (not `PiDma::start_dma`) for both
    /// directions: build an OSIoMesg exactly as `SsSram_Dma` does (dramAddr
    /// +0x8, devAddr +0xC, size +0x10, per pi.h:52-58), OS_WRITE the pattern to
    /// SRAM, then OS_READ it back into a different rdram region and assert the
    /// guest's own `MEM_BU`/`MEM_W` accessors read every byte in the SAME
    /// order. A flat (non-swizzled) copy in either direction fails here.
    #[test]
    fn os_epi_start_dma_round_trips_sram_save_domain() {
        // A ROM whose bytes at offset 0 are DISTINCT from the SRAM pattern, so
        // a regression that reads the ROM instead of the save is caught.
        let mut rom = vec![0u8; 0x1000];
        rom[0..4].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        load_rom(rom);
        // OoT uses 32 KiB banked SRAM.
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::SramBanked,
        )));

        let mut rdram = vec![0u8; 0x10000];
        let sram_handle = install_sram_handle(
            &mut rdram,
            0x1000,
            fn64_runtime::PiDomainTiming {
                latency: 0x12,
                pulse_width: 0x34,
                page_size: 0x0d,
                release: 2,
            },
        );
        let mb_offset = 0x2000usize;
        let mb_vram: u64 = 0x8000_2000;
        // EPI callers provide the offset from the handle's base; the shared
        // resolver must form 0x0800_0010 before entering the PI fabric.
        let sram_dev_addr: u32 = 0x10;
        let size: u32 = 8;

        // Guest lays 8 distinct bytes at rdram 0x5000 via MEM_BU (byte-lane
        // `^3`), the way it would build a save record before writing it out.
        let src = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let src_off = 0x5000usize;
        for (k, &b) in src.iter().enumerate() {
            rdram[(src_off + k) ^ 3] = b;
        }
        // OSIoMesg for the WRITE (OS_WRITE=1 -> FromRdram).
        rdram[mb_offset + 0x4..mb_offset + 0x8].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&0x8000_5000u32.to_ne_bytes());
        rdram[mb_offset + 0xC..mb_offset + 0x10].copy_from_slice(&sram_dev_addr.to_ne_bytes());
        rdram[mb_offset + 0x10..mb_offset + 0x14].copy_from_slice(&size.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = sram_handle;
        ctx.r5 = mb_vram;
        ctx.r6 = 1; // OS_WRITE
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(
            with_host(|host| {
                host.device_fabric
                    .pi_domain_timing(fn64_runtime::PiDomain::Domain2)
            }),
            fn64_runtime::PiDomainTiming {
                latency: 0x12,
                pulse_width: 0x34,
                page_size: 0x0d,
                release: 2,
            }
        );
        complete_pi_dma();

        // OSIoMesg for the READ back into a DIFFERENT region (0x6000).
        let dst_off = 0x6000usize;
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&0x8000_6000u32.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = sram_handle;
        ctx.r5 = mb_vram;
        ctx.r6 = 0; // OS_READ
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        complete_pi_dma();

        // Guest reads readBuff[k] via MEM_BU((dst)+k) = rdram[(dst+k)^3];
        // every byte must match the original -- swizzle cancels round-trip.
        for (k, &b) in src.iter().enumerate() {
            assert_eq!(
                rdram[(dst_off + k) ^ 3],
                b,
                "SRAM round-trip byte {k}: save DMA must route to the save store, \
                 word-swizzled, not the ROM"
            );
        }
        // The ROM byte at offset 0 (0xAA) must NOT appear -- proves the read
        // hit the save store, not the ROM image.
        assert_ne!(rdram[dst_off ^ 3], 0xAA);
        let save_operations = crate::copy_save_operations();
        assert_eq!(save_operations.len(), 2);
        assert_eq!(
            save_operations
                .iter()
                .map(|event| (event.device, event.operation, event.offset, event.len))
                .collect::<Vec<_>>(),
            vec![
                (
                    fn64_runtime::SaveType::SramBanked,
                    fn64_runtime::SaveOperationKind::Write,
                    0x10,
                    8,
                ),
                (
                    fn64_runtime::SaveType::SramBanked,
                    fn64_runtime::SaveOperationKind::Read,
                    0x10,
                    8,
                ),
            ]
        );
    }

    /// Regression test for the real infinite-loop bug `examples/wm2000-boot`
    /// surfaced (2026-07-14): `osEPiStartDma_recomp` never wrote `ctx.r2`
    /// ($v0), so NWXE's chunked-DMA caller (`func_80000660`, asm
    /// 0x800006E4-0x800006FC: `bne $v0, $zero, L_800006E4`) read whatever
    /// stale value `r2` already held and looped forever instead of falling
    /// through to `osRecvMesg`. Seed `ctx.r2` with a realistic STALE
    /// NON-ZERO value beforehand (mirroring the real caller's register
    /// state at the call site) so a regression that stops writing `ctx.r2`
    /// would fail this test even though a zero-initialized `ctx` would
    /// have hidden the bug.
    #[test]
    fn os_epi_start_dma_writes_zero_return_value_even_with_stale_nonzero_r2() {
        load_rom(vec![0xCDu8; 0x1000]);

        let mut rdram = vec![0u8; 0x10000];
        let cart_handle = install_cart_handle(&mut rdram, 0x1000);
        let mb_offset = 0x2000usize;
        // DmaMgr's real OSIoMesg layout: retQueue +0x4, dramAddr +0x8,
        // devAddr +0xC, size +0x10 (0x08-byte OSIoMesgHdr).
        rdram[mb_offset + 0x4..mb_offset + 0x8].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&0x8000_5000u32.to_ne_bytes());
        rdram[mb_offset + 0xC..mb_offset + 0x10].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb_offset + 0x10..mb_offset + 0x14].copy_from_slice(&4u32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = 0x8000_2000;
        ctx.r6 = 0; // OS_READ / ToRdram
        ctx.r2 = 0x1234; // stale non-zero, as a real caller's register would hold
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(
            ctx.r2, 0,
            "osEPiStartDma_recomp must overwrite $v0 with 0 (success) on every \
             accepted-start path, or NWXE's chunked-DMA retry loop spins forever"
        );
    }

    #[test]
    fn os_epi_raw_start_dma_reads_rom_with_fifth_argument_from_stack() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x28].copy_from_slice(&[0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87]);
        load_rom(rom);
        let mut rdram = vec![0u8; 0x100];
        let cart_handle = install_cart_handle(&mut rdram, 0x20);
        rdram[0x40 + 0x10..0x40 + 0x14].copy_from_slice(&8u32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = 0;
        ctx.r6 = 0x20;
        ctx.r7 = 0x8000_0080;
        ctx.r29 = 0x8000_0040;
        unsafe { osEPiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
        complete_pi_dma();
        assert_eq!(ctx.r2, 0);
        for (index, expected) in [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87]
            .into_iter()
            .enumerate()
        {
            assert_eq!(rdram[(0x80 + index) ^ 3], expected);
        }
    }

    #[test]
    fn pi_writes_to_read_only_rom_return_minus_one() {
        load_rom(vec![0u8; 0x100]);
        let mut rdram = vec![0u8; 0x100];
        let cart_handle = install_cart_handle(&mut rdram, 0x20);
        rdram[0x40 + 0x10..0x40 + 0x14].copy_from_slice(&8u32.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = cart_handle;
        ctx.r5 = 1;
        ctx.r6 = 0x20;
        ctx.r7 = 0x8000_0080;
        ctx.r29 = 0x8000_0040;
        unsafe { osEPiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, u64::MAX);
    }

    #[test]
    fn managed_raw_and_programmed_epi_calls_share_handle_address_and_timing_authority() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        load_rom(rom);
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::SramBanked,
        )));
        let mut rdram = vec![0u8; 0x800];
        let cart_timing = fn64_runtime::PiDomainTiming {
            latency: 0x21,
            pulse_width: 0x32,
            page_size: 0x0b,
            release: 1,
        };
        let save_timing = fn64_runtime::PiDomainTiming {
            latency: 0x43,
            pulse_width: 0x54,
            page_size: 0x0c,
            release: 2,
        };
        unsafe {
            write_epi_handle(
                rdram.as_mut_ptr(),
                0x8000_0100,
                DEVICE_TYPE_CART,
                fn64_runtime::PiDomain::Domain1,
                cart_timing,
                0xb000_0000,
            )
        };
        let cart_handle = 0xFFFF_FFFF_8000_0100;
        let save_handle = install_sram_handle(&mut rdram, 0x140, save_timing);

        // Raw EPI DMA consumes the same handle decode as managed EPI. The
        // request stores fn64's internal ROM offset while raw MMIO exposes
        // the handle's domain timing immediately.
        rdram[0x40 + 0x10..0x40 + 0x14].copy_from_slice(&4u32.to_ne_bytes());
        let mut raw = ctx_zeroed();
        raw.r4 = cart_handle;
        raw.r5 = 0;
        raw.r6 = 0x20;
        raw.r7 = 0x8000_0200;
        raw.r29 = 0x8000_0040;
        unsafe { osEPiRawStartDma_recomp(rdram.as_mut_ptr(), &mut raw) };
        assert_eq!(raw.r2, 0);
        assert_eq!(
            with_host(|host| host.device_fabric.pending_pi_request().unwrap().cart_addr),
            0x20
        );
        assert_eq!(
            read_raw_mmio_word(0xA460_0014),
            Some(cart_timing.latency as u32)
        );
        assert_eq!(
            read_raw_mmio_word(0xA460_0018),
            Some(cart_timing.pulse_width as u32)
        );
        complete_pi_dma();

        // The public OR rule also admits an already-absolute KSEG1 device
        // address. Segment removal happens only after the OR, at the PI
        // boundary, so this reaches the same ROM byte as offset 0x20 above.
        let mut absolute = ctx_zeroed();
        absolute.r4 = cart_handle;
        absolute.r5 = 0xb000_0020;
        absolute.r6 = 0x8000_0204;
        unsafe { osEPiReadIo_recomp(rdram.as_mut_ptr(), &mut absolute) };
        assert_eq!(absolute.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x204..0x208].try_into().unwrap()),
            0x1234_5678
        );

        // Managed EPI forms baseAddress | devAddr for SRAM and publishes the
        // second handle's settings through those same raw registers.
        let mb = 0x300usize;
        rdram[mb + 0x4..mb + 0x8].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb + 0x8..mb + 0xC].copy_from_slice(&0x8000_0240u32.to_ne_bytes());
        rdram[mb + 0xC..mb + 0x10].copy_from_slice(&0x10u32.to_ne_bytes());
        rdram[mb + 0x10..mb + 0x14].copy_from_slice(&4u32.to_ne_bytes());
        let mut managed = ctx_zeroed();
        managed.r4 = save_handle;
        managed.r5 = 0x8000_0300;
        managed.r6 = 0;
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut managed) };
        assert_eq!(managed.r2, 0);
        assert_eq!(
            with_host(|host| host.device_fabric.pending_pi_request().unwrap().cart_addr),
            fn64_runtime::rom::SRAM_DOMAIN2_BASE + 0x10
        );
        assert_eq!(
            read_raw_mmio_word(0xA460_0024),
            Some(save_timing.latency as u32)
        );
        assert_eq!(
            read_raw_mmio_word(0xA460_0028),
            Some(save_timing.pulse_width as u32)
        );
        complete_pi_dma();

        // Programmed I/O uses the same resolver in both directions rather
        // than retaining a third handle/address implementation.
        let mut write = ctx_zeroed();
        write.r4 = save_handle;
        write.r5 = 0x20;
        write.r6 = 0xCAFE_BABE;
        unsafe { osEPiWriteIo_recomp(rdram.as_mut_ptr(), &mut write) };
        assert_eq!(write.r2, 0);
        let mut read = ctx_zeroed();
        read.r4 = save_handle;
        read.r5 = 0x20;
        read.r6 = 0x8000_0280;
        unsafe { osEPiReadIo_recomp(rdram.as_mut_ptr(), &mut read) };
        assert_eq!(read.r2, 0);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x280..0x284].try_into().unwrap()),
            0xCAFE_BABE
        );
        assert_eq!(
            crate::copy_save_operations()
                .iter()
                .map(|event| (event.operation, event.offset, event.len))
                .collect::<Vec<_>>(),
            vec![
                (fn64_runtime::SaveOperationKind::Read, 0x10, 4),
                (fn64_runtime::SaveOperationKind::Write, 0x20, 4),
                (fn64_runtime::SaveOperationKind::Read, 0x20, 4),
            ]
        );
    }

    #[test]
    fn epi_handle_for_unbacked_public_device_space_is_a_loud_typed_trap() {
        let mut rdram = vec![0u8; 0x200];
        unsafe {
            write_epi_handle(
                rdram.as_mut_ptr(),
                0x8000_0100,
                DEVICE_TYPE_64DD,
                fn64_runtime::PiDomain::Domain2,
                fn64_runtime::PiDomainTiming::default(),
                0xa500_0000,
            )
        };
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            resolve_epi_device_address(rdram.as_mut_ptr(), 0x8000_0100, 0, "typed EPI test")
        }));
        assert!(result.is_err());
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].subsystem, fn64_runtime::UnsupportedSubsystem::Abi);
        assert_eq!(events[0].operation, "abi.pi.epi-handle");
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        assert!(events[0].context.contains("0x05000000"));
        fn64_runtime::complete_unsupported_observation(Cycles::ZERO, &"0".repeat(64));

        assert_subprocess_aborts("pi::tests::__unbacked_epi_handle_abort_subprocess_entry");
    }

    #[test]
    fn epi_handle_outside_physical_rdram_is_a_loud_typed_trap() {
        let mut rdram = vec![0u8; 0x100];
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            resolve_epi_device_address(
                rdram.as_mut_ptr(),
                0xffff_ffff_807f_fff0,
                0,
                "out-of-range EPI test",
            )
        }));
        assert!(result.is_err());
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].operation, "abi.pi.epi-handle");
        assert!(events[0].context.contains("outside physical RDRAM"));
        fn64_runtime::complete_unsupported_observation(Cycles::ZERO, &"0".repeat(64));
    }

    #[test]
    fn epi_handle_rejects_a_raw_physical_base_instead_of_guessing_its_address_form() {
        let mut rdram = vec![0u8; 0x200];
        unsafe {
            write_epi_handle(
                rdram.as_mut_ptr(),
                0x8000_0100,
                DEVICE_TYPE_SRAM,
                fn64_runtime::PiDomain::Domain2,
                fn64_runtime::PiDomainTiming::default(),
                0xa800_0000,
            )
        };
        rdram[0x10c..0x110].copy_from_slice(&0x0800_0000u32.to_ne_bytes());

        fn64_runtime::arm_unsupported_events(None).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            resolve_epi_device_address(rdram.as_mut_ptr(), 0x8000_0100, 0, "physical-base test")
        }));
        assert!(result.is_err());
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].operation, "abi.pi.epi-handle");
        assert!(events[0].context.contains("uncached KSEG1"));
        fn64_runtime::complete_unsupported_observation(Cycles::ZERO, &"0".repeat(64));
    }

    #[test]
    #[ignore]
    fn __unbacked_epi_handle_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            load_rom(vec![0; 0x100]);
            let mut rdram = vec![0u8; 0x400];
            unsafe {
                write_epi_handle(
                    rdram.as_mut_ptr(),
                    0x8000_0100,
                    2,
                    fn64_runtime::PiDomain::Domain2,
                    fn64_runtime::PiDomainTiming::default(),
                    0xa500_0000,
                )
            };
            fn64_runtime::arm_unsupported_events(None).unwrap();
            let mut ctx = ctx_zeroed();
            ctx.r4 = 0x8000_0100;
            ctx.r5 = 0;
            ctx.r6 = 0x8000_0200;
            unsafe { osEPiReadIo_recomp(rdram.as_mut_ptr(), &mut ctx) };
        }
    }

    #[test]
    fn os_epi_start_dma_without_a_loaded_rom_is_a_loud_named_trap() {
        assert_subprocess_aborts("pi::tests::__os_epi_start_dma_no_rom_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __os_epi_start_dma_no_rom_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            // mb points at an all-zero rdram region -> ret_queue==0 (no
            // completion post attempted), dev_addr==0, len==0 -- the load-
            // bearing assertion here is that with_pi_dma panics because no
            // ROM was ever installed in this fresh subprocess, not that the
            // (deliberately trivial) transfer parameters are realistic.
            //
            // `mb` must be a real KSEG0 vram address with a buffer behind it:
            // a bare `r5 = 0` is NOT "rdram offset 0". `RdramAddr::from_gpr(0)`
            // computes `0 - 0xFFFFFFFF_80000000` = 0x80000000, so this shim's
            // `mb`-relative read of `retQueue` (+0x4) dereferenced ~2 GiB past
            // a 64-byte Vec and killed the child with SIGBUS *before* reaching
            // the `no ROM installed` panic -- the test still "passed" on
            // `!status.success()` while proving nothing about the trap.
            const MB_VRAM: u64 = 0xFFFF_FFFF_8000_0000;
            let mut ctx = ctx_zeroed();
            let mut rdram = rdram_for_vram(MB_VRAM + 0x40);
            unsafe {
                write_epi_handle(
                    rdram.as_mut_ptr(),
                    0x8000_0020,
                    DEVICE_TYPE_CART,
                    fn64_runtime::PiDomain::Domain1,
                    fn64_runtime::PiDomainTiming::default(),
                    0xb000_0000,
                )
            };
            ctx.r4 = 0x8000_0020;
            ctx.r5 = MB_VRAM;
            ctx.r6 = 0; // direction = ToRdram
            unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        }
    }
}
