use super::*;

pub(crate) fn require_no_mmio_write_effect(
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
pub(crate) fn pif_ram_window_offset(vaddr: u64) -> Option<usize> {
    let upper = vaddr >> 32;
    let low = vaddr as u32;
    let physical = low & 0x1FFF_FFFF;
    ((upper == 0 || upper == u32::MAX as u64)
        && (0x8000_0000..0xC000_0000).contains(&low)
        && (0x1FC0_07C0..0x1FC0_0800).contains(&physical))
    .then(|| (physical - 0x1FC0_07C0) as usize)
}

/// Direct cached/uncached CPU views of PI domain 1 address 2, whose physical
/// base `0x1000_0000` is byte zero of the installed Game Pak ROM. The public
/// libultra PI/Cartridge Domain contract uses the same KSEG translation for
/// programmed CPU reads and PI DMA device addresses.
pub(crate) fn cartridge_rom_window_offset(vaddr: u64) -> Option<u32> {
    let upper = vaddr >> 32;
    let low = vaddr as u32;
    let physical = low & 0x1FFF_FFFF;
    ((upper == 0 || upper == u32::MAX as u64)
        && (0x8000_0000..0xC000_0000).contains(&low)
        && PI_DOM1_ADDR2.contains(&physical))
    .then(|| physical - 0x1000_0000)
}

/// Name an access to PI domain-1 address 1 instead of letting it fall through
/// as an anonymous unbacked-memory fault.
///
/// `epi_domain_for_address` accepts `PI_DOM1_ADDR1` (0x0600_0000..=0x07ff_ffff,
/// the N64DD window) for `osPiHandle` operations, but
/// `cartridge_rom_window_offset` accepts only `PI_DOM1_ADDR2`, the cartridge
/// ROM. So a direct CPU read there matched no window, was not backed RDRAM,
/// and surfaced as `MemoryFault { addr: 0xffffffffa6000000 }` -- a message
/// that says nothing about which device is missing.
///
/// WM2000 hits this: it configures the BSD domain-1 registers at
/// 0xA4600018/0x20 and then probes the device at 0x8002_2620.
///
/// This deliberately traps rather than returning a value. Real hardware
/// returns open-bus for an absent device, but the exact value is a
/// MEASUREMENT question, and inventing one would fabricate hardware behaviour
/// -- the same reason the U5 frontier leaves device timing open rather than
/// guessing. Naming the device costs nothing and turns an anonymous fault into
/// an actionable one.
/// Model an ABSENT N64DD ASIC, opt-in via `FN64_ABSENT_N64DD=1`.
///
/// Disassembling WM2000's probe settles what this read is for, which the
/// mupen-vs-Ares disagreement about open-bus never could:
///
/// ```text
/// 0x80022550  lui  $14,0x8004 ; lw $14,0x7f10($14)   load guard
/// 0x80022560  bne  $14,$0,+6                          guard!=0 -> 0x8002257c
/// 0x8002257c  sw   $0,0x7f10($1)                      CLEAR the guard
/// 0x80022584  lui  $24,0xa600                         DD base as a VALUE
/// 0x80022590  sw   $24,0xc($16)                       store it in a struct
/// 0x80022614  lw   $14,0xc($16)                       load it back
/// 0x80022618  lui  $1,0xa000 ; or $15,$14,$1          force uncached
/// 0x80022620  lw   $2,0x0($15)                        <-- THE TRAPPING READ
/// 0x80022624  srl  $25,$2,16  ; andi $12,$25,0xf      version nibble
/// 0x80022628  srl  $13,$2,20  ; andi $14,$13,0xf      version nibble
/// 0x80022634  srl  $24,$2,8
/// 0x80022638  sb   $12,0x6($16) .. sb $2,0x5($16)     store 4 derived bytes
/// ```
///
/// The word is consumed ONLY as packed BCD version nibbles written into a
/// device descriptor. No branch anywhere in this routine tests them, so no
/// control-flow decision depends on which value an absent device returns --
/// which is exactly why mupen (`(addr & 0xFFFF) | ((addr & 0xFFFF) << 16)`)
/// and Ares (freeze, 0) can disagree and both boot cartridge games.
///
/// Zero is the honest encoding of "no ASIC answered": every version nibble
/// reads back 0, i.e. no drive revision. It is not a measurement of open-bus
/// on real silicon and does not claim to be. It takes effect when the loaded
/// configuration is cartridge-only (see [`absent_n64dd_enabled`]); when no ROM
/// is installed the config is unknown and `trap_absent_pi_domain1_device`
/// still fires.
fn read_absent_n64dd_asic(vaddr: u64) -> Option<u32> {
    let physical = (vaddr as u32) & 0x1FFF_FFFF;
    if !crate::pi::mmio::PI_DOM1_ADDR1.contains(&physical) {
        return None;
    }
    absent_n64dd_enabled().then_some(0)
}

/// Whether to model the N64DD as absent (return the no-drive value) rather than
/// trap on a domain-1 probe.
///
/// Default-on WHEN A CARTRIDGE ROM IS LOADED AND NO 64DD DISK IS CONFIGURED.
/// A loaded cartridge (`rom_installed`) with no `leo_disk` (the `osLeoDiskInit`
/// handle) is a configuration that provably has no drive attached, so "no ASIC
/// answered" is a FACT about what is loaded, not a guess about open-bus silicon
/// — it does not fabricate hardware behaviour the way inventing a value for an
/// unknown device would. When no ROM is installed the config is unknown, and if
/// a disk WAS configured the drive is present-but-unmodelled; the trap still
/// fires in both cases. `FN64_ABSENT_N64DD=1` forces it on regardless (e.g. a
/// probe that runs before `load_rom`).
fn absent_n64dd_enabled() -> bool {
    if std::env::var_os("FN64_ABSENT_N64DD").is_some_and(|value| value == "1") {
        return true;
    }
    with_host(|host| host.rom_installed && host.leo_disk.is_none())
}

fn trap_absent_pi_domain1_device(vaddr: u64) {
    let low = vaddr as u32;
    let physical = low & 0x1FFF_FFFF;
    if !crate::pi::mmio::PI_DOM1_ADDR1.contains(&physical) {
        return;
    }
    let message = format!(
        "CPU read of PI domain-1 address 1 at {vaddr:#018x} (physical          {physical:#010x}): this window is the N64DD, which no cartridge-only          ROM provides. Reads here have no modelled device; open-bus behaviour          is unmeasured and is not invented here."
    );
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Abi,
        "abi.pi.absent-domain1-device",
        &message,
        Some(with_host(|host| host.device_fabric.now())),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!("{message}");
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
    if let Some(offset) = cartridge_rom_window_offset(vaddr) {
        return Some(with_host(|host| {
            assert!(
                host.rom_installed,
                "direct cartridge ROM read at {vaddr:#018x} requires load_rom first"
            );
            let mut bytes = [0u8; 4];
            host.device_fabric
                .pi_dma()
                .read_rom_bytes(offset, &mut bytes);
            u32::from_be_bytes(bytes)
        }));
    }
    if let Some(value) = read_live_device_mmio(vaddr) {
        return Some(value);
    }
    if let Some(value) = read_live_rcp_interrupt_mmio(vaddr) {
        return Some(value);
    }
    if let Some(value) = read_absent_n64dd_asic(vaddr) {
        return Some(value);
    }
    trap_absent_pi_domain1_device(vaddr);
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
        if (VI_MMIO_BASE..VI_MMIO_END).contains(&low) {
            eprintln!("[boot-probe] raw VI write {low:#010x} = {value:#010x}");
        } else if (0xA480_0000..0xA480_0020).contains(&low)
            || (0xA404_0000..0xA404_0020).contains(&low)
        {
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

// Commit due device work before any executor resume is possible.
//
// Plain comment, not `///`: rustdoc does not generate documentation for macro
// invocations, so a doc comment here is silently dropped and warns. The
// per-case doc comment inside the macro body does attach to the static.
thread_local! {
    /// [clock_lags+due, clock_lags+nothing_due, current+due, current+nothing_due]
    pub(crate) static DEVICE_ADVANCE_CENSUS: std::cell::RefCell<[u64; 4]> =
        const { std::cell::RefCell::new([0; 4]) };
}

/// Report the device-advance case census gathered under
/// `FN64_DEVICE_ADVANCE_CENSUS`.
pub fn print_device_advance_census() {
    DEVICE_ADVANCE_CENSUS.with(|c| {
        let c = c.borrow();
        let total: u64 = c.iter().sum();
        if total == 0 {
            println!("[device-census] no samples");
            return;
        }
        println!(
            "[device-census] total={total} lag+due={} ({:.1}%) lag+nothing={} ({:.1}%) current+due={} ({:.1}%) current+nothing={} ({:.1}%)",
            c[0], c[0] as f64 / total as f64 * 100.0,
            c[1], c[1] as f64 / total as f64 * 100.0,
            c[2], c[2] as f64 / total as f64 * 100.0,
            c[3], c[3] as f64 / total as f64 * 100.0,
        );
    });
}

pub(crate) fn advance_device_time(now: u64) -> u32 {
    // Fast path: nothing to commit.
    //
    // This runs after EVERY guest step, and the loop below always performs at
    // least one `advance_device_time_step`, which takes three separate
    // host/executor borrows and collects pending PI/SI/SP state before
    // discovering there is nothing due. It profiled as 38% of the certified
    // lane's self time.
    //
    // When device time already equals `now` and no deadline is due, that step
    // has no work: no event can fire, and the fabric clock needs no advance.
    // Checking that in one borrow skips the rest.
    // Fast path: no deadline is due, so only the fabric CLOCK needs to move.
    //
    // A census over 60,000 steps found this case is 100% of calls: the clock
    // always lags `now` and nothing is ever due at this point, because due
    // work is committed when it is scheduled rather than discovered here. The
    // loop below nonetheless ran a full `advance_device_time_step` every time,
    // taking three host/executor borrows and collecting pending PI/SI/SP state
    // before finding no event to fire. That was 38% of the lane's self time.
    //
    // With nothing due, no event handler can run and no PIF work can be
    // produced, so advancing the clock is the entire operation. VI retrace
    // ticks are likewise only produced by a firing VI event, hence zero.
    // Advance the clock without a memory view when nothing is due.
    // `advance_clock_if_idle` re-checks the deadline itself and refuses if any
    // event IS due, so this cannot skip real device work -- which is the exact
    // failure mode of the earlier empty-view version.
    let advanced = with_host(|host| {
        host.device_fabric
            .advance_clock_if_idle(fn64_runtime::Cycles::new(now))
    });
    if advanced {
        return 0;
    }
    // NOTE: an earlier version of this fast path advanced the fabric with an
    // EMPTY `RdramViewMut::from_storage(&mut [])`, on the reasoning that with
    // no deadline due no device work can touch memory. That was wrong, and it
    // is what zeroed the executable baseline: the view is passed as
    // `DmaMemory`, so anything the fabric does commit goes into a zero-length
    // buffer, and the mutation journal then reads zeros where published ROM
    // bytes belong.
    //
    // Measured: with the fast path enabled the route dies at step 421,717 with
    // "unjournaled executable mutation changed physical RDRAM
    // [0x0009b0b3,0x0009b0b4)"; with it disabled the same build reaches the
    // same step cleanly. The baseline probe confirms the byte is seeded
    // correctly at construction (`expected[0x9b0b3]=0x10`, matching ROM) and
    // only becomes zero later.
    //
    // The win was recovered by `advance_clock_if_idle` above, which advances
    // the clock with no memory view at all and re-checks the deadline itself,
    // so it cannot be used to skip real device work. Below this point some
    // event IS due, and the loop advances with the real memory.
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
pub(crate) fn advance_device_time_step(now: u64) -> u32 {
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
            let mut committed = notify_committed_dma_write;
            let mut view = unsafe {
                fn64_runtime::ProcessDmaMemory::from_raw_parts(rdram, rdram_len, &mut committed)
            };
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
                            device: completion.device,
                            len: completion.len,
                        },
                        "PI completion does not match the sole in-flight request"
                    );

                    if let (
                        DmaDirection::ToRdram,
                        fn64_runtime::PiDeviceAddress::RomOffset(rom_offset),
                    ) = (completion.direction, completion.device)
                    {
                        if let Some(static_off) =
                            host.sections.plan_static_mirror(rom_offset, completion.len)
                        {
                            let mut bytes = vec![0u8; completion.len as usize];
                            host.device_fabric
                                .pi_dma_mut()
                                .read_rom_bytes(rom_offset, &mut bytes);
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
                            fn64_cpu_runtime::notify_pi_dma_write(mirror.offset(), completion.len);
                        }
                        overlays.push((
                            rom_offset,
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
                    let had_pending_mode = host.pending_vi_mode.is_some();
                    if let Some(mode) = host.pending_vi_mode.take() {
                        const FIELD_REGISTER_INDICES: [usize; 5] = [1, 13, 10, 11, 3];
                        for (index, value) in mode.registers.into_iter().enumerate() {
                            if index == 4 || FIELD_REGISTER_INDICES.contains(&index) {
                                continue;
                            }
                            let addr = MmioAddr::new(
                                VI_MMIO_BASE
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
                                .write_mmio(MmioAddr::new(VI_MMIO_BASE), control),
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
                                VI_MMIO_BASE
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
                        let address = VI_MMIO_BASE
                            + u32::try_from(index).expect("VI register index exceeds u32") * 4;
                        *word = host
                            .device_fabric
                            .read_mmio(MmioAddr::new(address))
                            .expect("complete VI register image is not mapped");
                    }
                    if crate::boot_probe_enabled() {
                        let device = host.device_fabric.snapshot();
                        eprintln!(
                            "[boot-probe] VI retrace at={} pending_mode={} active_mode={} pending_framebuffer={pending_vi_framebuffer:?} mi_pending={:#04x} mi_mask={:#04x} words={words:08x?}",
                            at.get(),
                            had_pending_mode,
                            host.active_vi_mode.is_some(),
                            device.mi_pending,
                            device.mi_mask,
                        );
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

pub(crate) unsafe fn epi_start_dma_impl(rdram: *mut u8, ctx: *mut RecompContext, use_handle: bool) {
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
    let dev_addr = read_offset_word(rdram, mb_addr.offset(), 0xC);
    let len = read_offset_word(rdram, mb_addr.offset(), 0x10);
    let device = if use_handle {
        unsafe { resolve_epi_device_address(rdram, ctx.r4, dev_addr, "osEPiStartDma_recomp") }
    } else {
        fn64_runtime::PiDeviceAddress::RomOffset(dev_addr)
    };
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
            device,
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
    let device = unsafe {
        resolve_epi_device_address(rdram, ctx.r4, ctx.r6 as u32, "osEPiRawStartDma_recomp")
    };
    ctx.r2 = if start_raw_pi_dma(
        rdram,
        direction,
        dram_addr,
        device,
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

pub(crate) unsafe fn epi_read_io_impl(rdram: *mut u8, ctx: *mut RecompContext, use_handle: bool) {
    let ctx = unsafe { &mut *ctx };
    let device = if use_handle {
        unsafe { resolve_epi_device_address(rdram, ctx.r4, ctx.r5 as u32, "osEPiReadIo_recomp") }
    } else {
        fn64_runtime::PiDeviceAddress::RomOffset(ctx.r5 as u32)
    };
    let dram_addr = RdramAddr::from_gpr(ctx.r6).offset() as usize;
    let record_sram = with_pi_dma("osEPiReadIo_recomp", |dma| {
        let mut buf = [0u8; 4];
        let record_sram = match device {
            fn64_runtime::PiDeviceAddress::SramOffset(offset) => {
                dma.sram_read_into(offset, &mut buf);
                dma.save_len() == Some(fn64_runtime::SaveType::SramBanked.byte_len())
            }
            fn64_runtime::PiDeviceAddress::RomOffset(offset) => {
                dma.read_rom_bytes(offset, &mut buf);
                false
            }
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
    if let (true, fn64_runtime::PiDeviceAddress::SramOffset(offset)) = (record_sram, device) {
        crate::record_save_operation(
            fn64_runtime::SaveType::SramBanked,
            fn64_runtime::SaveOperationKind::Read,
            offset as usize,
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

pub(crate) unsafe fn write_io_mesg_word(rdram: *mut u8, mb: RdramAddr, offset: u32, value: u32) {
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
    let device =
        unsafe { resolve_epi_device_address(rdram, ctx.r4, ctx.r5 as u32, "osEPiWriteIo_recomp") };
    if let fn64_runtime::PiDeviceAddress::SramOffset(offset) = device {
        let record_sram = with_pi_dma("osEPiWriteIo_recomp", |dma| {
            dma.sram_write_from(offset, &(ctx.r6 as u32).to_be_bytes());
            dma.save_len() == Some(fn64_runtime::SaveType::SramBanked.byte_len())
        });
        if record_sram {
            crate::record_save_operation(
                fn64_runtime::SaveType::SramBanked,
                fn64_runtime::SaveOperationKind::Write,
                offset as usize,
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
mod absent_n64dd_tests {
    use super::*;

    // A KSEG1 view of PI domain-1 address-1 (the N64DD ASIC window), physical
    // 0x0600_0000 -- the exact address OoT/WM2000 probe and that trapped.
    const DOMAIN1_PROBE_VADDR: u64 = 0xffff_ffff_a600_0000;

    #[test]
    fn cartridge_loaded_no_disk_reads_absent_drive() {
        with_host(|host| {
            host.rom_installed = true;
            host.leo_disk = None;
        });
        // A cartridge with no 64DD disk provably has no drive: return the
        // honest no-drive value (0) instead of trapping.
        assert_eq!(read_absent_n64dd_asic(DOMAIN1_PROBE_VADDR), Some(0));
    }

    #[test]
    fn no_rom_installed_still_traps() {
        with_host(|host| {
            host.rom_installed = false;
            host.leo_disk = None;
        });
        // Unknown configuration -> do not fabricate a value; the caller falls
        // through to trap_absent_pi_domain1_device.
        assert_eq!(read_absent_n64dd_asic(DOMAIN1_PROBE_VADDR), None);
    }

    // The `FN64_ABSENT_N64DD=1` override path is intentionally NOT unit-tested
    // here: it mutates process-global env, which races the other tests under
    // the default parallel runner. It is a thin `var_os == "1"` check; the
    // rom_installed inference below is the behaviour worth pinning.

    #[test]
    fn disk_configured_still_traps() {
        with_host(|host| {
            host.rom_installed = true;
            host.leo_disk = Some(crate::pi::LeoDiskConfig {
                handle_vram: 0x8010_0000,
                latency: 0,
                page_size: 0,
                release: 0,
                pulse_width: 0,
            });
        });
        // A 64DD disk WAS configured: the drive is present-but-unmodelled, so
        // absence is not a fact here -> fall through to the trap.
        let got = read_absent_n64dd_asic(DOMAIN1_PROBE_VADDR);
        with_host(|host| host.leo_disk = None);
        assert_eq!(got, None);
    }

    #[test]
    fn non_domain1_address_is_not_this_devices_concern() {
        with_host(|host| host.rom_installed = true);
        // A cartridge-ROM address (domain-1 address-2) is not the N64DD window.
        assert_eq!(read_absent_n64dd_asic(0xffff_ffff_b000_0000), None);
    }
}
