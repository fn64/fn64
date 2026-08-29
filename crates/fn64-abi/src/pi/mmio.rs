use super::*;

pub(crate) const VI_MMIO_BASE: u32 = 0xA440_0000;
pub(crate) const VI_MMIO_END: u32 =
    VI_MMIO_BASE + fn64_render::ViScanoutRegisters::WORD_COUNT as u32 * 4;

pub(crate) fn notify_committed_dma_write(
    channel: fn64_runtime::DmaWriterChannel,
    offset: usize,
    len: usize,
) {
    #[cfg(feature = "recomp-rs")]
    {
        let notify = match channel {
            fn64_runtime::DmaWriterChannel::Pi => fn64_cpu_runtime::notify_pi_dma_write,
            fn64_runtime::DmaWriterChannel::Si => fn64_cpu_runtime::notify_si_dma_write,
            fn64_runtime::DmaWriterChannel::Sp => fn64_cpu_runtime::notify_sp_dma_write,
        };
        notify(
            u32::try_from(offset).expect("DMA write offset exceeds u32"),
            u32::try_from(len).expect("DMA write length exceeds u32"),
        );
    }
    #[cfg(not(feature = "recomp-rs"))]
    let _ = (channel, offset, len);
}

/// Normalize a cartridge image to canonical big-endian order.
///
/// `.z64` is already big-endian, `.n64` is word-reversed and `.v64` is
/// byte-pair-swapped. Returns `None` when the header magic is unrecognized --
/// the caller then publishes nothing and a shard's later recovery attempt
/// fails loudly with its own span in the message, which is a better error
/// than a silently mis-ordered image would produce.
///
/// This mirrors `fn64_discover::normalize`'s ordering half. `fn64-abi` does
/// not depend on `fn64-discover` (a build-time analysis crate), so the
/// transform is restated here rather than importing the pipeline.
#[cfg(feature = "recomp-rs")]
fn normalize_rom_to_big_endian(bytes: &[u8]) -> Option<Vec<u8>> {
    const MAGIC_Z64: u32 = 0x8037_1240;
    const MAGIC_N64: u32 = 0x4012_3780;
    const MAGIC_V64: u32 = 0x3780_4012;
    if bytes.len() < 4 || !bytes.len().is_multiple_of(4) {
        return None;
    }
    match u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) {
        MAGIC_Z64 => Some(bytes.to_vec()),
        MAGIC_N64 => Some(
            bytes
                .chunks_exact(4)
                .flat_map(|word| [word[3], word[2], word[1], word[0]])
                .collect(),
        ),
        MAGIC_V64 => Some(
            bytes
                .chunks_exact(2)
                .flat_map(|pair| [pair[1], pair[0]])
                .collect(),
        ),
        _ => None,
    }
}

/// Publish the normalized image to `fn64-cpu-runtime` so generated shard crates
/// can recover their instruction words instead of embedding them.
///
/// A no-op without the `recomp-rs` feature, and a no-op for the many tests
/// that install small synthetic ROMs with no valid header magic.
fn publish_normalized_rom_image_for_shards(bytes: &[u8]) {
    #[cfg(feature = "recomp-rs")]
    if let Some(normalized) = normalize_rom_to_big_endian(bytes) {
        fn64_cpu_runtime::publish_normalized_rom_image(normalized);
    }
    #[cfg(not(feature = "recomp-rs"))]
    let _ = bytes;
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
    // Publish the normalized image for generated shard crates to recover their
    // instruction words from, so no verbatim ROM words ship in the artifact.
    // Every entry point already routes the user's ROM through here, which is
    // why this is the seam rather than each shell's boot path.
    publish_normalized_rom_image_for_shards(&bytes);
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
        host.rsp_rdp_observation_count = 0;
        host.rsp_rdp_observation_retention = crate::RspRdpObservationRetention::CompleteEvidence;
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

// Public os_pi.h defines four device types: CART=0, BULK=1 ("ROM bulk", used
// by osDriveRomInit's N64DD Drive-ROM handle), 64DD=2, SRAM=3 -- N64
// Programming Manual Chapter 27's os_pi.h listing documents "values 0 through
// 3" as the supported range. BULK has no named constant or constructor here:
// like the 64DD register window traced in `abi.pi.absent-domain1-device`
// (pi/timing.rs), Drive-ROM access is 64DD accessory territory this crate
// does not model, so no code path ever builds a handle carrying it. The
// `<= DEVICE_TYPE_SRAM` bound below is deliberately inclusive of the BULK
// value for that reason: a handle claiming it still decodes and then falls
// through `resolve_epi_device_address`'s final match to the same loud
// "no backing device" trap an unbacked 64DD handle already hits, not a
// silent gap.
pub(crate) const DEVICE_TYPE_CART: u8 = 0;
pub(crate) const DEVICE_TYPE_64DD: u8 = 2;
pub(crate) const DEVICE_TYPE_SRAM: u8 = 3;
pub(crate) const PI_DOMAIN1: u8 = 0;
pub(crate) const PI_DOMAIN2: u8 = 1;
pub(crate) const KSEG1_BASE: u32 = 0xa000_0000;
pub(crate) const KSEG1_END: u32 = 0xc000_0000;
pub(crate) const PI_DOM1_ADDR1: std::ops::RangeInclusive<u32> = 0x0600_0000..=0x07ff_ffff;
pub(crate) const PI_DOM1_ADDR2: std::ops::RangeInclusive<u32> = 0x1000_0000..=0x1fbf_ffff;
pub(crate) const PI_DOM1_ADDR3: std::ops::RangeInclusive<u32> = 0x1fd0_0000..=0x7fff_ffff;
pub(crate) const PI_DOM2_ADDR1: std::ops::RangeInclusive<u32> = 0x0500_0000..=0x05ff_ffff;
pub(crate) const PI_DOM2_ADDR2: std::ops::RangeInclusive<u32> = 0x0800_0000..=0x0fff_ffff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EpiHandle {
    pub(crate) device_type: u8,
    pub(crate) domain: fn64_runtime::PiDomain,
    pub(crate) timing: fn64_runtime::PiDomainTiming,
    pub(crate) base_address: u32,
}

pub(crate) fn trap_epi_handle(shim: &str, detail: impl std::fmt::Display) -> ! {
    let message = format!("{shim}: {detail}");
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Abi,
        "abi.pi.epi-handle",
        &message,
        Some(Cycles::new(with_host(|host| host.device_fabric.now().get()))),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!("{message}")
}

pub(crate) fn epi_domain_for_address(shim: &str, address: u32) -> fn64_runtime::PiDomain {
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

pub(crate) fn epi_physical_base(shim: &str, base_address: u32) -> u32 {
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
pub(crate) unsafe fn resolve_epi_device_address(
    rdram: *mut u8,
    handle_gpr: u64,
    dev_addr: u32,
    shim: &str,
) -> fn64_runtime::PiDeviceAddress {
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
        fn64_runtime::PiDeviceAddress::RomOffset(physical - 0x1000_0000)
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
        fn64_runtime::PiDeviceAddress::SramOffset(physical - 0x0800_0000)
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

pub(crate) fn start_timed_pi_dma(
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
            "[fn64-abi/pi] thread={thread} {shim} {:?} device={:?} dram={:#010x} len={:#x} \
             ret_queue={ret_queue:?}",
            request.direction,
            request.device,
            request.dram_addr.offset(),
            request.len
        );
    }
    let result = with_host(|host| {
        if matches!(
            (request.direction, request.device),
            (
                DmaDirection::FromRdram,
                fn64_runtime::PiDeviceAddress::RomOffset(_)
            )
        ) {
            return Err(DeviceFault::PiTransfer(PiDmaError::ReadOnlyDevice {
                device: request.device,
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
pub(crate) fn start_raw_pi_dma(
    rdram: *mut u8,
    direction: DmaDirection,
    dram_addr: RdramAddr,
    device: fn64_runtime::PiDeviceAddress,
    len: u32,
    shim: &str,
) -> bool {
    assert!(
        device.offset().is_multiple_of(2),
        "{shim}: PI device address {device:?} is not 2-byte aligned"
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
            device,
            len,
        },
        None,
        0,
        shim,
    )
    .is_ok()
}

pub(crate) fn live_device_mmio_addr(vaddr: u64, write: bool) -> Option<MmioAddr> {
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
    let is_dpc_read = !write
        && matches!(
            addr,
            0xA410_0000
                | 0xA410_0004
                | 0xA410_0008
                | 0xA410_000C
                // Performance counters (clock/cmd/pipe/tmem) -- readable so the
                // STATUS counter-clear commands are observable over raw MMIO.
                | 0xA410_0010
                | 0xA410_0014
                | 0xA410_0018
                | 0xA410_001C
        );
    let is_dpc_write = write && matches!(addr, 0xA410_0000 | 0xA410_0004 | 0xA410_000C);
    let is_vi = (VI_MMIO_BASE..VI_MMIO_END).contains(&addr);
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
        if crate::boot_probe_enabled() {
            let snapshot = fabric.snapshot();
            eprintln!(
                "[boot-probe] MI mask set requested={mask:#04x} latched={:#04x} pending={:#04x}",
                snapshot.mi_mask, snapshot.mi_pending
            );
        }
    });
}

pub(crate) fn raise_device_interrupt(source: fn64_runtime::InterruptSource) {
    with_host(|host| host.device_fabric.raise_interrupt(source));
}

pub(crate) fn clear_device_interrupt(source: fn64_runtime::InterruptSource) {
    with_host(|host| host.device_fabric.clear_interrupt(source));
}

pub(crate) fn apply_live_ai_write_effect(
    rdram: *mut u8,
    effect: fn64_runtime::DeviceMmioWriteEffect,
) {
    match effect {
        fn64_runtime::DeviceMmioWriteEffect::None => {}
        fn64_runtime::DeviceMmioWriteEffect::AiFrequencyChanged {
            sample_rate_hz,
            affected_dma_ids,
        } => {
            let sample_period = with_host(|host| host.device_fabric.ai_sample_period())
                .unwrap_or_else(|error| {
                    panic!("accepted AI frequency has no exact period: {error}")
                });
            assert_eq!(
                sample_period.floor_hz(),
                sample_rate_hz,
                "AI frequency effect and exact device period disagree"
            );
            crate::task_dispatch::notify_audio_sample_period(sample_period);
            for id in affected_dma_ids.into_iter().flatten() {
                crate::task_dispatch::notify_audio_dma_retimed(id);
            }
        }
        fn64_runtime::DeviceMmioWriteEffect::AiDmaAccepted(admission) => {
            let request = admission.request;
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
                        Some(admission.id),
                    )
                };
            }
            if let Some(start) = admission.start {
                crate::task_dispatch::notify_audio_dma_started(start);
            }
        }
        fn64_runtime::DeviceMmioWriteEffect::AiDmaStarted(start) => {
            crate::task_dispatch::notify_audio_dma_started(start);
        }
        fn64_runtime::DeviceMmioWriteEffect::DpcSubmissionRequested { submission, .. } => {
            panic!(
                "AI register write unexpectedly produced DPC transaction token {}",
                submission.token
            )
        }
        fn64_runtime::DeviceMmioWriteEffect::RspStartRequested { pc } => {
            panic!("AI register write unexpectedly started the RSP at {pc:#06x}")
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
    let mut committed = notify_committed_dma_write;
    let view = unsafe {
        fn64_runtime::ProcessDmaMemory::from_raw_parts(rdram, required_len, &mut committed)
    };
    with_host(|host| {
        host.device_fabric
            .admit_sp_task_with_boot_image(&view, task_addr, header, boot)
    })
}

pub(crate) fn arm_live_vi(interval: u64) -> Result<(), DeviceFault> {
    with_host(|host| host.device_fabric.arm_vi(Cycles::new(interval)))
}

pub(crate) fn queue_live_vi_mode(registers: [u32; 14], fields: [[u32; 5]; 2]) {
    if crate::boot_probe_enabled() {
        eprintln!("[boot-probe] queued VI mode common={registers:08x?} fields={fields:08x?}");
    }
    let pending_vi_framebuffer =
        crate::with_executor(|exec| exec.vi().next_framebuffer.map(RdramAddr::offset));
    with_host(|host| {
        host.pending_vi_mode = Some(PendingViMode { registers, fields });
        // The public VI manager resets prior scale/special-feature overrides
        // when a new mode is queued. Later calls may mutate this mode image.
        host.pending_vi_control = None;
        host.pending_vi_x_scale = None;
        host.pending_vi_y_scale = None;
        if host.device_fabric.vi_field_interval().is_none() {
            super::timing::latch_pending_vi_mode_initial(host, pending_vi_framebuffer);
        }
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

pub(crate) fn scaled_vi_register(base: u32, scale: f32) -> u32 {
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
                    .read_mmio(MmioAddr::new(VI_MMIO_BASE))
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
        if addr.get() == 0xA460_0008
            && (0x1000_0000..0x1FC0_0000).contains(&fabric.snapshot().pi_cart_addr)
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
        | fn64_runtime::DeviceMmioWriteEffect::AiDmaAccepted(_)
        | fn64_runtime::DeviceMmioWriteEffect::AiDmaStarted(_)) => {
            apply_live_ai_write_effect(rdram, effect);
        }
        fn64_runtime::DeviceMmioWriteEffect::DpcSubmissionRequested {
            submission,
            retained_tail,
        } => {
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
            unsafe {
                crate::task_dispatch::dispatch_dpc_submission(rdram, submission, retained_tail)
            };
        }
        fn64_runtime::DeviceMmioWriteEffect::RspStartRequested { pc } => {
            // A guest kicked the RSP through raw MMIO rather than through the
            // libultra task shim. Run it on the same LLE interpreter the task
            // lane uses; ownership is a raw kick rather than a task lineage.
            unsafe { crate::task_dispatch::dispatch_raw_rsp_start(rdram, pc) };
        }
    }
    true
}
