use super::*;

const EEPROM_CHANNEL: usize = 4;
const EEPROM_BLOCK_BYTES: usize = 8;
const PIF_CHANNEL_NO_RESPONSE: u8 = 0x80;
const ACCESSORY_BLOCK_BYTES: usize = 32;
const ACCESSORY_ADDR_MASK: u16 = 0xFFE0;
const ACCESSORY_ADDR_RUMBLE_PROBE: u16 = 0x8000;
const ACCESSORY_ADDR_RUMBLE_MOTOR: u16 = 0xC000;

/// Public Joybus accessory addressing sends eleven aligned address bits plus
/// the five-bit polynomial checksum. The public hardware vectors include
/// 0x0020 -> 0x15 and the Rumble motor address 0xC000 -> 0x1B.
fn accessory_address_crc(address: u16) -> u8 {
    let mut block_address = address >> 5;
    let mut crc = 0u8;
    for _ in 0..16 {
        crc <<= 1;
        if block_address & 0x400 != 0 {
            crc = if crc & 0x20 != 0 { crc ^ 0x14 } else { crc | 1 };
        } else if crc & 0x20 != 0 {
            crc ^= 0x15;
        }
        block_address <<= 1;
    }
    crc & 0x1F
}

/// Public Joybus data checksum: CRC-8 seed 0, polynomial 0x85, followed by
/// eight zero augmentation bits. Accessories apply it to 32-byte blocks;
/// public VRU captures apply the same checksum to their shorter payloads.
fn joybus_data_crc(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for byte in data.iter().copied().chain(std::iter::once(0)) {
        for bit in (0..8).rev() {
            let feedback = crc & 0x80 != 0;
            crc <<= 1;
            if byte & (1 << bit) != 0 {
                crc |= 1;
            }
            if feedback {
                crc ^= 0x85;
            }
        }
    }
    crc
}

#[cfg(test)]
fn accessory_data_crc(data: &[u8; ACCESSORY_BLOCK_BYTES]) -> u8 {
    joybus_data_crc(data)
}

fn trap_raw_voice(command: u8, port: usize, now: Cycles, detail: impl AsRef<str>) -> ! {
    let context = format!(
        "SI PIF Voice command {command:#04x} on channel {port} is unsupported: {}",
        detail.as_ref()
    );
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Abi,
        format!("abi.si.voice-command-{command:02x}"),
        &context,
        Some(now),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!("{context}")
}

fn trap_raw_pif(command: u8, port: usize, tx_size: u8, rx_size: u8, now: Cycles) -> ! {
    let context = format!(
        "SI PIF command {command:#04x} on channel {port} with tx={tx_size} rx={rx_size} is not implemented"
    );
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Abi,
        format!("abi.si.pif-command-{command:02x}"),
        &context,
        Some(now),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!("{context}")
}

fn accessory_address(pif_ram: &[u8; 64], cursor: usize, command: u8, port: usize) -> u16 {
    let encoded = u16::from_be_bytes([pif_ram[cursor + 3], pif_ram[cursor + 4]]);
    let address = encoded & ACCESSORY_ADDR_MASK;
    let supplied_crc = (encoded & !ACCESSORY_ADDR_MASK) as u8;
    let expected_crc = accessory_address_crc(address);
    assert_eq!(
        supplied_crc, expected_crc,
        "SI PIF accessory command {command:#04x} on channel {port} has address CRC {supplied_crc:#04x} for {address:#06x}; expected {expected_crc:#04x}"
    );
    address
}

fn voice_zero_address(
    pif_ram: &[u8; 64],
    cursor: usize,
    command: u8,
    port: usize,
    now: Cycles,
) -> u16 {
    let address = accessory_address(pif_ram, cursor, command, port);
    if address != 0 {
        trap_raw_voice(
            command,
            port,
            now,
            format!("address {address:#06x}; public Voice captures establish only address 0x0000"),
        );
    }
    address
}

fn voice_result_payload(result: fn64_runtime::VoiceData) -> [u8; 36] {
    let mut payload = [0u8; 36];
    payload[..4].copy_from_slice(&[0x80, 0x00, 0x0F, 0x00]);
    let scalar_fields = [
        result.warning,
        result.answer_num,
        result.voice_level,
        result.voice_sn,
        result.voice_time,
    ];
    for (index, value) in scalar_fields.into_iter().enumerate() {
        let offset = 4 + index * 2;
        payload[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    for index in 0..5 {
        let offset = 14 + index * 4;
        payload[offset..offset + 2].copy_from_slice(&result.answer[index].to_le_bytes());
        payload[offset + 2..offset + 4].copy_from_slice(&result.distance[index].to_le_bytes());
    }
    payload[34..].copy_from_slice(&[0x40, 0x00]);
    payload
}

fn eeprom_query_response<R: fn64_runtime::RomStorage>(
    pi_dma: &mut fn64_runtime::PiDma<R>,
    now: Cycles,
) -> Option<[u8; 3]> {
    let status = pi_dma.eeprom_status(now)?;
    let identifier = status.kind.joybus_identifier().to_be_bytes();
    Some([identifier[0], identifier[1], u8::from(status.busy) * 0x80])
}

fn require_pif_shape(
    command: u8,
    port: usize,
    tx_size: u8,
    rx_size: u8,
    expected_tx: u8,
    expected_rx: u8,
) {
    assert!(
        tx_size == expected_tx && rx_size == expected_rx,
        "SI PIF command {command:#04x} on channel {port} has tx={tx_size} rx={rx_size}; expected tx={expected_tx} rx={expected_rx}"
    );
}

fn mark_no_response(pif_ram: &mut [u8; 64], cursor: usize) {
    pif_ram[cursor + 1] |= PIF_CHANNEL_NO_RESPONSE;
}

#[derive(Default)]
pub(crate) struct PifExecutionObservations {
    pub(crate) save_operations: Vec<fn64_runtime::SaveOperationEvent>,
    pub(crate) controller_operations: Vec<fn64_runtime::ControllerOperationEvent>,
}

impl PifExecutionObservations {
    fn record_controller(
        &mut self,
        now: Cycles,
        port: usize,
        device: fn64_runtime::ControllerOperationDevice,
        operation: fn64_runtime::ControllerOperationKind,
    ) {
        self.controller_operations
            .push(fn64_runtime::ControllerOperationEvent {
                at: now,
                port: u8::try_from(port).expect("PIF controller port exceeds u8"),
                device,
                operation,
            });
    }
}

fn execute_pif_commands<R: fn64_runtime::RomStorage>(
    now: Cycles,
    pif_ram: &mut [u8; 64],
    executor: &mut fn64_runtime::Executor,
    pi_dma: &mut fn64_runtime::PiDma<R>,
    observations: &mut PifExecutionObservations,
) {
    executor.advance_transfer_paks_to(now);
    let mut port = 0usize;
    let mut cursor = 0usize;
    while cursor < pif_ram.len() {
        let tx_size = pif_ram[cursor];
        match tx_size {
            0x00 => {
                port += 1;
                cursor += 1;
                continue;
            }
            0xFD => {
                port += 1;
                cursor += 1;
                continue;
            }
            0xFE => break,
            0xFF => {
                cursor += 1;
                continue;
            }
            _ => {}
        }
        assert!(
            cursor + 2 < pif_ram.len(),
            "SI PIF command header at {cursor:#x} exceeds 64-byte PIF RAM"
        );
        let tx_size = tx_size & 0x3F;
        let rx_size = pif_ram[cursor + 1] & 0x3F;
        pif_ram[cursor + 1] = rx_size;
        let command = pif_ram[cursor + 2];
        let rx_off = cursor + 2 + tx_size as usize;
        let next = rx_off + rx_size as usize;
        assert!(
            next <= pif_ram.len(),
            "SI PIF command at {cursor:#x} overruns PIF RAM: tx={tx_size} rx={rx_size}"
        );
        match command {
            0x00 | 0xFF if port < EEPROM_CHANNEL => {
                require_pif_shape(command, port, tx_size, rx_size, 1, 3);
                let mut response = executor.pif().query_response(port);
                if matches!(
                    executor.pif().port_state(port),
                    fn64_runtime::PortState::VoiceRecognitionUnit
                ) {
                    response[2] = u8::from(
                        executor
                            .voice_unit(port)
                            .expect("VRU identity exists without VoiceUnit state")
                            .initialized(),
                    );
                }
                pif_ram[rx_off..next].copy_from_slice(&response);
            }
            0x00 | 0xFF if port == EEPROM_CHANNEL => {
                require_pif_shape(command, port, tx_size, rx_size, 1, 3);
                if let Some(response) = eeprom_query_response(pi_dma, now) {
                    pif_ram[rx_off..next].copy_from_slice(&response);
                } else {
                    mark_no_response(pif_ram, cursor);
                }
            }
            0x01 if port < EEPROM_CHANNEL => {
                require_pif_shape(command, port, tx_size, rx_size, 1, 4);
                match executor.pif().port_state(port) {
                    fn64_runtime::PortState::StandardControllerNoPak
                    | fn64_runtime::PortState::StandardControllerControllerPak
                    | fn64_runtime::PortState::StandardControllerRumblePak
                    | fn64_runtime::PortState::StandardControllerTransferPak => {
                        pif_ram[rx_off..next]
                            .copy_from_slice(&executor.pif().read_data_response(port));
                        observations.record_controller(
                            now,
                            port,
                            fn64_runtime::ControllerOperationDevice::StandardController,
                            fn64_runtime::ControllerOperationKind::Read,
                        );
                    }
                    fn64_runtime::PortState::VoiceRecognitionUnit
                    | fn64_runtime::PortState::Absent => mark_no_response(pif_ram, cursor),
                }
            }
            0x02 if port < EEPROM_CHANNEL => {
                require_pif_shape(command, port, tx_size, rx_size, 3, 33);
                let address = accessory_address(pif_ram, cursor, command, port);
                let mut data = [0u8; ACCESSORY_BLOCK_BYTES];
                match executor.pif().port_state(port) {
                    fn64_runtime::PortState::StandardControllerRumblePak => {
                        if address == ACCESSORY_ADDR_RUMBLE_PROBE {
                            data.fill(0x80);
                        }
                    }
                    fn64_runtime::PortState::StandardControllerNoPak => {
                        pif_ram[rx_off..rx_off + ACCESSORY_BLOCK_BYTES].copy_from_slice(&data);
                        pif_ram[next - 1] = joybus_data_crc(&data) ^ 0xFF;
                        cursor = next;
                        port += 1;
                        continue;
                    }
                    fn64_runtime::PortState::Absent => {
                        mark_no_response(pif_ram, cursor);
                        cursor = next;
                        port += 1;
                        continue;
                    }
                    fn64_runtime::PortState::StandardControllerControllerPak => {
                        let pak = executor
                            .controller_pak(port)
                            .expect("typed Controller Pak identity has no storage");
                        let physical_offset = usize::from(pak.active_bank())
                            * fn64_runtime::pfs::PFS_BANK_CAPACITY
                            + usize::from(address);
                        pak.raw_read_block(address as usize, &mut data)
                            .expect("validated Controller Pak block address was rejected");
                        if usize::from(address) < fn64_runtime::pfs::PFS_BANK_CAPACITY {
                            observations
                                .save_operations
                                .push(fn64_runtime::SaveOperationEvent {
                                at: now,
                                device: fn64_runtime::SaveType::ControllerPak,
                                operation: fn64_runtime::SaveOperationKind::Read,
                                offset: u32::try_from(physical_offset)
                                    .expect("Controller Pak physical offset exceeds u32"),
                                len: fn64_runtime::pfs::PFS_BLOCK_SIZE as u32,
                                });
                        }
                    }
                    fn64_runtime::PortState::StandardControllerTransferPak => {
                        executor
                            .transfer_pak_mut(port)
                            .expect("typed Transfer Pak identity has no storage")
                            .read_block(address, &mut data);
                        observations.record_controller(
                            now,
                            port,
                            fn64_runtime::ControllerOperationDevice::TransferPak,
                            fn64_runtime::ControllerOperationKind::Read,
                        );
                    }
                    fn64_runtime::PortState::VoiceRecognitionUnit => trap_raw_voice(
                        command,
                        port,
                        now,
                        "the standard accessory-read packet has no established Voice semantics",
                    ),
                }
                pif_ram[rx_off..rx_off + ACCESSORY_BLOCK_BYTES].copy_from_slice(&data);
                pif_ram[next - 1] = joybus_data_crc(&data);
            }
            0x03 if port < EEPROM_CHANNEL => {
                require_pif_shape(command, port, tx_size, rx_size, 35, 1);
                let address = accessory_address(pif_ram, cursor, command, port);
                let data: [u8; ACCESSORY_BLOCK_BYTES] = pif_ram
                    [cursor + 5..cursor + 5 + ACCESSORY_BLOCK_BYTES]
                    .try_into()
                    .expect("fixed accessory write payload must be 32 bytes");
                let data_crc = joybus_data_crc(&data);
                match executor.pif().port_state(port) {
                    fn64_runtime::PortState::StandardControllerRumblePak => {
                        if address == ACCESSORY_ADDR_RUMBLE_MOTOR {
                            executor
                                .set_rumble(port, data[ACCESSORY_BLOCK_BYTES - 1] & 1 != 0)
                                .expect("typed Rumble Pak identity changed during one PIF command");
                            observations.record_controller(
                                now,
                                port,
                                fn64_runtime::ControllerOperationDevice::RumblePak,
                                fn64_runtime::ControllerOperationKind::Control,
                            );
                        }
                        pif_ram[rx_off] = data_crc;
                    }
                    fn64_runtime::PortState::StandardControllerNoPak => {
                        pif_ram[rx_off] = data_crc ^ 0xFF;
                    }
                    fn64_runtime::PortState::Absent => mark_no_response(pif_ram, cursor),
                    fn64_runtime::PortState::StandardControllerControllerPak => {
                        let pak = executor
                            .controller_pak_mut(port)
                            .expect("typed Controller Pak identity has no storage");
                        let physical_offset = usize::from(pak.active_bank())
                            * fn64_runtime::pfs::PFS_BANK_CAPACITY
                            + usize::from(address);
                        pak.raw_write_block(address as usize, &data)
                            .expect("validated Controller Pak block address was rejected");
                        if usize::from(address) < fn64_runtime::pfs::PFS_BANK_CAPACITY {
                            observations
                                .save_operations
                                .push(fn64_runtime::SaveOperationEvent {
                                at: now,
                                device: fn64_runtime::SaveType::ControllerPak,
                                operation: fn64_runtime::SaveOperationKind::Write,
                                offset: u32::try_from(physical_offset)
                                    .expect("Controller Pak physical offset exceeds u32"),
                                len: fn64_runtime::pfs::PFS_BLOCK_SIZE as u32,
                                });
                        }
                        pif_ram[rx_off] = data_crc;
                    }
                    fn64_runtime::PortState::StandardControllerTransferPak => {
                        executor
                            .transfer_pak_mut(port)
                            .expect("typed Transfer Pak identity has no storage")
                            .write_block(address, &data);
                        observations.record_controller(
                            now,
                            port,
                            fn64_runtime::ControllerOperationDevice::TransferPak,
                            fn64_runtime::ControllerOperationKind::Write,
                        );
                        pif_ram[rx_off] = data_crc;
                    }
                    fn64_runtime::PortState::VoiceRecognitionUnit => trap_raw_voice(
                        command,
                        port,
                        now,
                        "the standard accessory-write packet has no established Voice semantics",
                    ),
                }
            }
            0x04 if port == EEPROM_CHANNEL => {
                require_pif_shape(command, port, tx_size, rx_size, 2, 8);
                let block = pif_ram[cursor + 3];
                match pi_dma.eeprom_read_block(now, block) {
                    Ok(data) => pif_ram[rx_off..next].copy_from_slice(&data),
                    Err(fn64_runtime::EepromError::NoDevice) => mark_no_response(pif_ram, cursor),
                    Err(fn64_runtime::EepromError::Busy { ready_at }) => panic!(
                        "SI PIF EEPROM read command 0x04 on channel {port} arrived while a write is busy until guest cycle {} -- poll EEPROM Info status bit 0x80 before reading",
                        ready_at.get()
                    ),
                }
            }
            0x05 if port == EEPROM_CHANNEL => {
                require_pif_shape(command, port, tx_size, rx_size, 10, 1);
                let block = pif_ram[cursor + 3];
                let data: [u8; EEPROM_BLOCK_BYTES] = pif_ram
                    [cursor + 4..cursor + 4 + EEPROM_BLOCK_BYTES]
                    .try_into()
                    .expect("fixed EEPROM write payload must be eight bytes");
                match pi_dma.start_eeprom_write(now, block, data) {
                    Ok(_) => pif_ram[rx_off] = 0,
                    Err(fn64_runtime::EepromError::Busy { .. }) => pif_ram[rx_off] = 0x80,
                    Err(fn64_runtime::EepromError::NoDevice) => mark_no_response(pif_ram, cursor),
                }
            }
            0x0B
                if port < EEPROM_CHANNEL
                    && matches!(
                        executor.pif().port_state(port),
                        fn64_runtime::PortState::VoiceRecognitionUnit
                    ) =>
            {
                require_pif_shape(command, port, tx_size, rx_size, 3, 3);
                let arguments = [pif_ram[cursor + 3], pif_ram[cursor + 4]];
                if arguments != [0, 0] {
                    trap_raw_voice(
                        command,
                        port,
                        now,
                        format!(
                            "status-query arguments {:02x} {:02x}; public captures establish only 00 00",
                            arguments[0], arguments[1]
                        ),
                    );
                }
                let status = executor
                    .voice_unit(port)
                    .expect("VRU identity exists without VoiceUnit state")
                    .wire_status();
                let response = [status, 0];
                pif_ram[rx_off..rx_off + response.len()].copy_from_slice(&response);
                pif_ram[rx_off + response.len()] = joybus_data_crc(&response);
            }
            0x09
                if port < EEPROM_CHANNEL
                    && matches!(
                        executor.pif().port_state(port),
                        fn64_runtime::PortState::VoiceRecognitionUnit
                    ) =>
            {
                require_pif_shape(command, port, tx_size, rx_size, 3, 37);
                voice_zero_address(pif_ram, cursor, command, port, now);
                let result = executor
                    .voice_unit_mut(port)
                    .expect("VRU identity exists without VoiceUnit state")
                    .take_result()
                    .unwrap_or_else(|error| {
                        trap_raw_voice(
                            command,
                            port,
                            now,
                            format!(
                                "result read without a captured host-injected result: {error:?}"
                            ),
                        )
                    });
                let payload = voice_result_payload(result);
                pif_ram[rx_off..rx_off + payload.len()].copy_from_slice(&payload);
                pif_ram[rx_off + payload.len()] = joybus_data_crc(&payload);
                observations.record_controller(
                    now,
                    port,
                    fn64_runtime::ControllerOperationDevice::VoiceRecognitionUnit,
                    fn64_runtime::ControllerOperationKind::Read,
                );
            }
            0x0C
                if port < EEPROM_CHANNEL
                    && matches!(
                        executor.pif().port_state(port),
                        fn64_runtime::PortState::VoiceRecognitionUnit
                    ) =>
            {
                require_pif_shape(command, port, tx_size, rx_size, 7, 1);
                voice_zero_address(pif_ram, cursor, command, port, now);
                let data: [u8; 4] = pif_ram[cursor + 5..cursor + 9]
                    .try_into()
                    .expect("fixed Voice control payload must be four bytes");
                let voice = executor
                    .voice_unit_mut(port)
                    .expect("VRU identity exists without VoiceUnit state");
                match data {
                    [0x02, 0, words, 0] => voice.clear_dictionary(words).unwrap_or_else(|error| {
                        panic!(
                            "SI PIF Voice clear-dictionary command 0x0c on channel {port} failed for {words} words: {error:?}"
                        )
                    }),
                    [0x00, 0, 0x01, 0] => voice.finish_raw_initialize().unwrap_or_else(|error| {
                        panic!(
                            "SI PIF Voice raw initialization-finalize command 0x0c on channel {port} failed: {error:?}"
                        )
                    }),
                    [0x00, 0, 0x06, 0] => voice.start().unwrap_or_else(|error| {
                        panic!(
                            "SI PIF Voice start command 0x0c on channel {port} failed: {error:?}"
                        )
                    }),
                    [0x05, 0, 0, 0] => voice.stop(),
                    _ => trap_raw_voice(
                        command,
                        port,
                        now,
                        format!(
                            "control payload {:02x} {:02x} {:02x} {:02x}; only captured initialization-finalize 00 00 01 00, clear-dictionary 02 00 nn 00, start 00 00 06 00, and stop 05 00 00 00 forms are modeled",
                            data[0], data[1], data[2], data[3]
                        ),
                    ),
                }
                pif_ram[rx_off] = joybus_data_crc(&data);
                observations.record_controller(
                    now,
                    port,
                    fn64_runtime::ControllerOperationDevice::VoiceRecognitionUnit,
                    fn64_runtime::ControllerOperationKind::Control,
                );
            }
            0x0D
                if port < EEPROM_CHANNEL
                    && matches!(
                        executor.pif().port_state(port),
                        fn64_runtime::PortState::VoiceRecognitionUnit
                    ) =>
            {
                require_pif_shape(command, port, tx_size, rx_size, 3, 1);
                let address = accessory_address(pif_ram, cursor, command, port);
                executor
                    .voice_unit_mut(port)
                    .expect("VRU identity exists without VoiceUnit state")
                    .raw_initialize_step(address)
                    .unwrap_or_else(|error| {
                        trap_raw_voice(
                            command,
                            port,
                            now,
                            format!(
                                "address {address:#06x} is not the next captured raw initialization write: {error:?}; power and gain writes remain unestablished"
                            ),
                        )
                    });
                pif_ram[rx_off] = 0;
                observations.record_controller(
                    now,
                    port,
                    fn64_runtime::ControllerOperationDevice::VoiceRecognitionUnit,
                    fn64_runtime::ControllerOperationKind::Control,
                );
            }
            0x0A
                if port < EEPROM_CHANNEL
                    && matches!(
                        executor.pif().port_state(port),
                        fn64_runtime::PortState::VoiceRecognitionUnit
                    ) =>
            {
                trap_raw_voice(
                    command,
                    port,
                    now,
                    "public captures do not establish the complete packet, sequencing, and error semantics",
                )
            }
            _ => trap_raw_pif(command, port, tx_size, rx_size, now),
        }
        cursor = next;
        port += 1;
    }
    // Real PIF hardware clears its control/format byte (0x1FC007FF) once the
    // command block has been processed; hand-rolled joybus code (AKI titles)
    // can poll that byte to confirm completion, and leaving the guest's 0x01
    // in place reads as "PIF still busy" forever.
    pif_ram[63] = 0;
}

pub(crate) fn execute_controller_pif<R: fn64_runtime::RomStorage>(
    now: Cycles,
    pif_ram: &mut [u8; 64],
    pi_dma: &mut fn64_runtime::PiDma<R>,
) -> PifExecutionObservations {
    let mut observations = PifExecutionObservations::default();
    with_executor(|executor| {
        execute_pif_commands(now, pif_ram, executor, pi_dma, &mut observations)
    });
    observations
}

/// `__osSiDeviceBusy(void) -> s32` reads `SI_STATUS_REG` and reports whether
/// either public SI busy bit (`DMA_BUSY = 1`, `IO_BUSY = 2`) is set.
///
/// The exact six-instruction NWXE body captured in the materialized boot bank
/// is `lui/ori/lw/andi/jr/sltu`: it reads `0xA4800018`, masks with `3`, and
/// canonicalizes the result to zero or one. The shim uses the same live device
/// fabric as arbitrary-PC MMIO, so a host binding cannot disagree with the
/// guest-AOT implementation about an in-flight SI transfer.
///
/// # Safety
/// `ctx` must point to a live recompiler context. The function does not access
/// `rdram`, matching the no-argument guest ABI.
#[no_mangle]
pub unsafe extern "C" fn __osSiDeviceBusy_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let status = crate::pi::read_raw_mmio_word(0xffff_ffff_a480_0018)
        .expect("__osSiDeviceBusy_recomp: SI_STATUS_REG is not mapped");
    ctx.r2 = u64::from(status & 3 != 0);
}

/// `__osSiRawStartDma(s32 direction, u8* dramAddr)` -- `a0`=direction
/// (`ctx->r4`; `OS_READ=0` transfers PIF RAM to RDRAM and `OS_WRITE=1`
/// transfers RDRAM to PIF RAM), `a1`=dramAddr
/// (`ctx->r5`, the PIF command-block buffer's rdram address).
///
/// ## What this really does (this wave, replacing the prior loud trap)
///
/// A real call site (`funcs_15.c` asm 0x80036040-0x80036064, the function
/// this milestone's evidence shows builds a controller-probe PIF block)
/// writes a standard libultra PIF-RAM command block into the buffer before
/// calling this: byte 0 = tx-size (`0xFF` = end-of-block marker observed at
/// offsets 0x26/final), each channel's header is
/// `[tx_size, rx_size, cmd, ...tx_bytes]` followed by `rx_size` response
/// bytes to fill in -- the public PIF-RAM format
/// protocol (`osContStartQuery`'s `0x01,0x03` 3-byte-tx-then-3-byte-rx
/// status-query shape, `osContStartReadData`'s 1-byte-tx/4-byte-rx
/// read-data shape). This function walks channels 0-3 in that documented
/// format when the DRAM-to-PIF DMA reaches its deadline. `0x00`, `0xFD`,
/// `0xFE`, and `0xFF` retain their documented skip, reset, format-end, and
/// dummy meanings. Controller query/read responses come from `PifModel`;
/// unsupported command bytes trap with channel context.
///
/// Each direction is a separate scheduled 64-byte transfer. SI BUSY stays set
/// until the fabric deadline; only then are bytes committed, MI SI raised,
/// and completion posted through `OS_EVENT_SI` (5, per the public libultra
/// manual's event-code table) via the SAME `Executor::inject_event` path
/// every other completion source uses -- matching `docs/DESIGN.md`
/// section 2's "closing the asymmetry" design point. If no
/// `osSetEventMesg(5, ...)` registration exists yet (this call happening
/// before the game registers its SI event), the post is silently absent
/// (mirrors `advance_time`'s VI-retrace handling of the same
/// not-yet-registered case) rather than panicking -- the DMA itself still
/// completes and the response bytes are still written, matching real
/// hardware where the SI interrupt fires regardless of whether software
/// has hooked it yet.
///
/// Raw EEPROM probe/read/write commands on external channel 4 use the same
/// save backing store as the high-level EEPROM shims. Raw Rumble Pak
/// probe/read/write uses the same typed port identity and motor latch as the
/// high-level motor shims, including the public address and data CRCs. A
/// non-EEPROM save device reports the PIF no-response bit; malformed packet
/// shapes and address checksums trap with command/channel context, while a
/// physical 4-Kbit EEPROM ignores its upper two block-address bits. Controller
/// Pak block I/O uses the same physical image as the high-level PFS model;
/// raw FAT and directory writes therefore determine later high-level note
/// discovery, chains, and free space. The layout and checksum rules follow the
/// public Controller Pak hardware map and filesystem geometry documented by
/// n64brew. Transfer Pak power, status/mode, bank, and Game Boy
/// bus windows use typed cartridge ROM/RAM plus common mapper state. MBC3 RTC
/// oscillator, immutable latch, halt, 9-bit day, and sticky carry state advance
/// on the same guest clock through raw and high-level paths. A separate host
/// API imports/exports the exact-ROM-bound RTC sidecar with explicit wall-time
/// samples. Raw Voice result reads, status, captured five-write initialization,
/// and initialization/clear/start/stop controls share the high-level
/// VoiceUnit lifecycle. Standard accessory read/write packets on a Voice
/// channel, region-dependent `0x0A` dictionary staging, `0x0D` power/gain
/// writes, result reads without a host result, and unestablished `0x0C` forms
/// record a command-specific unsupported event before trapping. Every other
/// unimplemented raw PIF command records its command byte and packet shape at
/// the same loud boundary. High-level shims do not silently authorize
/// fabricated raw-protocol success.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSiRawStartDma_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let dram_addr = RdramAddr::from_gpr(ctx.r5);
    if crate::boot_probe_enabled() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let n = CALLS.fetch_add(1, Ordering::Relaxed);
        if n < 24 || n.is_multiple_of(512) {
            eprintln!(
                "[boot-probe] __osSiRawStartDma #{n} dir={} dram={:#010x}",
                ctx.r4 as u32,
                dram_addr.offset() + 0x8000_0000
            );
            let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
            let bytes: Vec<u8> = (0..64)
                .map(|offset| {
                    let address = dram_addr
                        .checked_add(offset)
                        .expect("SI boot-probe block address overflow");
                    unsafe { storage.read_u8(address) }
                })
                .collect();
            eprintln!("[boot-probe]   block: {:02x?}", bytes);
        }
    }
    let kind = match ctx.r4 as u32 {
        0 => fn64_runtime::SiDmaKind::PifToDram,
        1 => fn64_runtime::SiDmaKind::DramToPif,
        direction => panic!("__osSiRawStartDma_recomp: invalid direction {direction}"),
    };
    let logical_end = dram_addr
        .offset()
        .checked_add(64)
        .expect("SI DMA RDRAM range overflow") as usize;
    let rdram_len = logical_end
        .checked_add(3)
        .expect("SI DMA RDRAM storage extent overflow")
        & !3;
    ctx.r2 = match crate::pi::start_live_si_dma(
        fn64_runtime::SiDmaRequest { kind, dram_addr },
        PendingSiCompletionOwner::ProcessRdram { rdram, rdram_len },
    ) {
        Ok(()) => 0,
        Err(DeviceFault::SiBusy) => u64::MAX,
        Err(error) => panic!("__osSiRawStartDma_recomp: {error}"),
    };
}

/// Host-facing input seam: feed controller `port`'s live button/stick state
/// so the game's next `osContGetReadData` reflects it. `buttons` is the N64
/// `OSContPad.button` bitmask (`oot-decomp/include/controller.h:4-17`:
/// `BTN_A = 0x8000`, `BTN_B = 0x4000`, `BTN_Z = 0x2000`, `BTN_START = 0x1000`,
/// d-pad `0x0800..0x0100`, `BTN_L = 0x0020`, `BTN_R = 0x0010`, C-buttons
/// `0x0008..0x0001`); `stick_x`/`stick_y` are the signed analog values
/// (`OSContPad.stick_x`/`stick_y`, centered at 0). A scripted-input harness
/// (`examples/oot-boot`) calls this to drive OoT headlessly. Idle by default,
/// so an un-driven boot sees an honest neutral pad.
pub fn set_controller_state(port: usize, buttons: u16, stick_x: i8, stick_y: i8) {
    let input = fn64_runtime::si::ContInput {
        button: buttons,
        stick_x,
        stick_y,
    };
    with_executor(|exec| exec.set_controller_input(port, input));
}

/// Host-facing accessory seam. The configured identity is the single source
/// used by controller queries, Controller Pak calls, and Rumble Pak calls.
pub fn set_controller_port_state(port: usize, state: fn64_runtime::PortState) {
    with_executor(|exec| exec.set_controller_port_state(port, state));
}

/// Host-facing runtime configuration for a linear bank-switched Controller
/// Pak. This replaces the retained Pak on `port` and selects it as the active
/// accessory; games do not need to be recompiled for a different capacity.
pub fn set_controller_pak_bank_count(
    port: usize,
    bank_count: fn64_runtime::ControllerPakBankCount,
) {
    with_executor(|exec| {
        exec.attach_controller_pak(
            port,
            fn64_runtime::ControllerPak::with_bank_count(bank_count),
        );
    });
}

/// Attach a Game Boy cartridge image and optional persistent RAM image to a
/// configured Transfer Pak. The cartridge header selects the typed mapper;
/// unsupported cartridge types and wrong RAM sizes are returned explicitly.
pub fn insert_transfer_pak_cartridge(
    port: usize,
    rom: Vec<u8>,
    ram: Option<Vec<u8>>,
) -> Result<(), fn64_runtime::TransferPakError> {
    with_executor(|exec| exec.insert_transfer_pak_cartridge(port, rom, ram))
}

/// Attach a Game Boy cartridge while restoring caller-loaded MBC3 battery
/// metadata. The host supplies the wall-clock sample explicitly; neither the
/// ABI nor runtime reads `SystemTime`.
pub fn insert_transfer_pak_cartridge_with_battery(
    port: usize,
    rom: Vec<u8>,
    ram: Option<Vec<u8>>,
    restore: Option<fn64_runtime::Mbc3BatteryRestore>,
) -> Result<(), fn64_runtime::TransferPakError> {
    with_executor(|exec| exec.insert_transfer_pak_cartridge_with_battery(port, rom, ram, restore))
}

/// Export the current cartridge's battery-backed RTC at the executor's exact
/// guest cycle and a caller-supplied host checkpoint.
pub fn checkpoint_transfer_pak_battery(
    port: usize,
    checkpoint: fn64_runtime::HostUnixNanos,
) -> Result<Option<fn64_runtime::Mbc3BatteryMetadata>, fn64_runtime::TransferPakError> {
    with_executor(|exec| exec.checkpoint_transfer_pak_battery(port, checkpoint))
}

/// Read back the physical Rumble Pak output for a host haptics backend.
pub fn rumble_active(port: usize) -> bool {
    with_executor(|exec| exec.pif().rumble_active(port))
}

/// Public Controller Manager maximum: the four physical controller ports.
const MAXCONTROLLERS: usize = 4;
const OS_EVENT_SI: u32 = 5;
const PIF_CONTROLLER_CONTROL: usize = 63;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControllerPollKind {
    Query,
    Read,
}

impl ControllerPollKind {
    const fn request(self) -> fn64_runtime::SiDmaKind {
        match self {
            Self::Query => fn64_runtime::SiDmaKind::ControllerQuery,
            Self::Read => fn64_runtime::SiDmaKind::ControllerRead,
        }
    }

    const fn command(self) -> u8 {
        match self {
            Self::Query => 0xFF,
            Self::Read => 0x01,
        }
    }

    const fn response_len(self) -> usize {
        match self {
            Self::Query => 3,
            Self::Read => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CompletedControllerChannel {
    error: u8,
    response: [u8; 4],
}

fn controller_pif_command(kind: ControllerPollKind, channels: usize) -> [u8; 64] {
    assert!(
        channels <= MAXCONTROLLERS,
        "Controller Manager channel prefix {channels} exceeds MAXCONTROLLERS (4)"
    );
    let mut packet = [0u8; 64];
    let mut cursor = 0usize;
    for _ in 0..channels {
        packet[cursor] = 1;
        packet[cursor + 1] = kind.response_len() as u8;
        packet[cursor + 2] = kind.command();
        cursor += 3 + kind.response_len();
    }
    packet[cursor] = 0xFE;
    // The public PIF command-byte protocol clears this control byte only
    // after command execution. Getters use it to reject a still-live poll.
    packet[PIF_CONTROLLER_CONTROL] = 1;
    packet
}

fn completed_controller_channels(
    packet: &[u8; 64],
    kind: ControllerPollKind,
) -> Vec<CompletedControllerChannel> {
    assert_eq!(
        packet[PIF_CONTROLLER_CONTROL], 0,
        "Controller Manager {:?} result requested before the SI/PIF transaction completed",
        kind
    );
    let mut channels = Vec::new();
    let mut cursor = 0usize;
    while packet[cursor] != 0xFE {
        assert!(
            channels.len() < MAXCONTROLLERS,
            "completed Controller Manager packet exceeds MAXCONTROLLERS (4)"
        );
        let tx = packet[cursor] & 0x3F;
        let rx = packet[cursor + 1] & 0x3F;
        assert_eq!(tx, 1, "completed Controller Manager packet has tx={tx}");
        assert_eq!(
            usize::from(rx),
            kind.response_len(),
            "completed Controller Manager {:?} packet has rx={rx}",
            kind
        );
        assert_eq!(
            packet[cursor + 2],
            kind.command(),
            "completed PIF packet is not the paired Controller Manager {:?} transaction",
            kind
        );
        let response_offset = cursor + 3;
        let mut response = [0u8; 4];
        response[..kind.response_len()]
            .copy_from_slice(&packet[response_offset..response_offset + kind.response_len()]);
        channels.push(CompletedControllerChannel {
            error: (packet[cursor + 1] & 0xC0) >> 4,
            response,
        });
        cursor = response_offset + kind.response_len();
        assert!(
            cursor < PIF_CONTROLLER_CONTROL,
            "completed Controller Manager packet lacks a format terminator"
        );
    }
    channels
}

fn completed_controller_packet(kind: ControllerPollKind) -> Vec<CompletedControllerChannel> {
    let packet = with_host(|host| *host.device_fabric.pif_ram());
    completed_controller_channels(&packet, kind)
}

fn controller_completion_message(mq_addr: RdramAddr, exclusive: bool, shim: &str) -> Mesg {
    with_executor(|exec| {
        let (registered, message) = exec.event_registration(OS_EVENT_SI).unwrap_or_else(|| {
            panic!("{shim}: OS_EVENT_SI has no registered completion message queue")
        });
        assert_eq!(
            registered,
            mq_addr,
            "{shim}: queue {:#010x} is not the OS_EVENT_SI target {:#010x}",
            mq_addr.offset(),
            registered.offset()
        );
        let activity = exec.queue_activity(mq_addr).unwrap_or_else(|| {
            panic!(
                "{shim}: queue {:#010x} was not initialized by osCreateMesgQueue",
                mq_addr.offset()
            )
        });
        if exclusive {
            assert!(
                activity.is_exclusively_idle(),
                "{shim}: initialization queue {:#010x} is shared or not idle: {activity:?}",
                mq_addr.offset()
            );
        }
        message
    })
}

fn start_controller_poll(
    mq_addr: RdramAddr,
    kind: ControllerPollKind,
    channels: usize,
) -> Result<(), DeviceFault> {
    let _ = controller_completion_message(
        mq_addr,
        false,
        match kind {
            ControllerPollKind::Query => "osContStartQuery_recomp",
            ControllerPollKind::Read => "osContStartReadData_recomp",
        },
    );
    crate::pi::start_live_controller_si_dma(
        fn64_runtime::SiDmaRequest {
            kind: kind.request(),
            dram_addr: RdramAddr::from_offset(0),
        },
        PendingSiCompletionOwner::OsEvent,
        controller_pif_command(kind, channels),
    )
}

/// `osContGetQuery(OSContStatus *data)` -- ONE argument, `a0`=data
/// (`ctx->r4`), returns void. Byte-verified against the OoT decomp
/// (`oot-decomp/src/libultra/io/contquery.c:31`,
/// `void osContGetQuery(OSContStatus* data)`) and its real call site
/// `PadSetup_Init` (`oot-decomp/src/libu64/padsetup.c:19`,
/// `osContGetQuery(status)` where `status = padMgr->padStatus`, an
/// `OSContStatus[MAXCONTROLLERS]` array). The generated call site confirms
/// the shape: `funcs_55.c:2193` sets only `$a0` (`ctx->r4 = ctx->r16`, the
/// `padStatus` pointer) and leaves `$a1` UNSET -- a prior wave's
/// `(int channel, OSContStatus* data)` signature read the data pointer from
/// the stale `$a1`/`ctx->r5` (garbage left by the preceding `osRecvMesg`
/// whose asm 0x800CD438 sets `$a1 = 0`), then dereferenced it: a real
/// EXC_BAD_ACCESS deep in `Main -> PadMgr_Init -> PadSetup_Init` on OoT's
/// first controller-status probe, which is why boot never yielded again
/// after DmaMgr delivered the code-segment DMA (thread 3 died mid-C, before
/// the next shim/yield).
///
/// Fills one entry per channel in the Controller Manager's active prefix.
/// The public `osContSetCh` manual specifies the default as
/// `MAXCONTROLLERS` and permits callers to allocate a smaller array after
/// reducing the count. Each
/// 4-byte entry is `{type: u16 @0, status: u8 @2, errno: u8 @3}`
/// (`oot-decomp/include/ultra64/controller.h:121`). The game reads these
/// back with `MEM_HU`/`MEM_BU` (`funcs_55.c:2205/2214`:
/// `MEM_BU(reg,3)`=errno, `MEM_HU(reg,0)`=type, compared `== 0x0005`
/// = `CONT_TYPE_NORMAL`), whose `^2`/`^3` sub-word swizzle
/// (`refs/N64RecompSource/include/recomp.h:104-108`) requires each logical
/// N64 struct byte at struct-offset `o` to live in the host buffer at
/// `(base + o) ^ 3` -- so a present port-0 standard controller must read
/// `type == 0x0005, errno == 0`, and absent ports 1-3 must read a non-zero
/// `errno` (`CONT_NO_RESPONSE_ERROR = 0x08`,
/// `oot-decomp/include/ultra64/controller.h:66`, the value
/// `CHNL_ERR(no-response) = (0x80 >> 4)` yields) so `PadSetup_Init`'s
/// `switch (status[i].errno)` skips them.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContGetQuery_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let data_addr = RdramAddr::from_gpr(ctx.r4).offset() as usize;
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    for (port, channel) in completed_controller_packet(ControllerPollKind::Query)
        .into_iter()
        .enumerate()
    {
        let resp = channel.response;
        let absent = channel.error != 0 || (resp[2] & fn64_runtime::si::CONT_ABSENT) != 0;
        // `query_response` returns the PIF wire bytes `[typeh, typel,
        // status]`. `__osContGetInitData` assembles the game-visible
        // `OSContStatus.type` u16 as `typel << 8 | typeh`
        // (`controller.c:72`), i.e. `(resp[1] << 8) | resp[0]` -- so a
        // standard controller (`[0x05, 0x00, ..]`) becomes `0x0005 =
        // CONT_TYPE_NORMAL`, which is what `PadSetup_Init` compares against.
        // The 4 logical N64 struct bytes are `type` (u16, big-endian: hi @0,
        // lo @1), `status` @2, `errno` @3. An absent port reports no-response
        // in `errno` with type/status left zero (matching the decomp's
        // `if (data->errno) continue;`, which never writes them on error).
        let type_u16: u16 = ((resp[1] as u16) << 8) | resp[0] as u16;
        let entry: [u8; 4] = if absent {
            [0, 0, 0, CONT_NO_RESPONSE_ERROR]
        } else {
            [(type_u16 >> 8) as u8, (type_u16 & 0xFF) as u8, resp[2], 0]
        };
        let base = data_addr + port * 4;
        // Store each logical byte at its `^3`-swizzled host position so the
        // game's MEM_HU/MEM_BU reads (recomp.h) recover the right values --
        // see the doc comment above for the byte-order derivation.
        for (o, &b) in entry.iter().enumerate() {
            unsafe {
                storage.write_u8(
                    RdramAddr::from_offset(
                        u32::try_from(base + o).expect("OSContStatus RDRAM address exceeds u32"),
                    ),
                    b,
                );
            }
        }
    }
}

/// `CONT_NO_RESPONSE_ERROR` (`oot-decomp/include/ultra64/controller.h:66`):
/// the `OSContStatus.errno` value an absent/non-responding controller port
/// reports (`CHNL_ERR` of a PIF no-response = `(CHNL_ERR_NORESP=0x80) >> 4`).
const CONT_NO_RESPONSE_ERROR: u8 = 0x08;

/// `osContGetReadData(OSContPad *pad) -> void` -- `a0`=`ctx->r4`, the base of
/// an `OSContPad[MAXCONTROLLERS]` array (`padMgr->pads`, decomp
/// `oot-decomp/src/code/padmgr.c:364` `osContGetReadData(padMgr->pads)`).
/// This is the INPUT SEAM's game-facing half: the per-port button/stick state
/// a host harness fed via `PifModel::set_input` is sampled when the paired
/// timed SI/PIF transaction completes, and this getter copies that immutable
/// completed packet into the pad array the game reads each retrace.
///
/// ## OSContPad layout + swizzle (byte-cited)
///
/// `oot-decomp/include/ultra64/controller.h:127-132`:
/// `{ button: u16 @0x00, stick_x: s8 @0x02, stick_y: s8 @0x03, errno: u8 @0x04 }`,
/// `size = 0x06`. The decomp `osContGetReadData`
/// (`oot-decomp/src/libultra/io/contreaddata.c:22`) iterates all
/// `__osMaxControllers`, sets `errno = CHNL_ERR(read)` for each, and ONLY
/// fills `button`/`stick_x`/`stick_y` when `errno == 0` -- so a present
/// controller reports `errno == 0` + live input, an absent port reports a
/// nonzero `errno` (`CONT_NO_RESPONSE_ERROR = 0x08`) with the game leaving the
/// stale button/stick (padmgr then `bzero`s pads[1]/pads[3] anyway).
///
/// The game reads these fields back through the recomp memory macros
/// (`refs/N64RecompSource/include/recomp.h:104-108`): `button` via `MEM_HU`
/// (`^2` halfword swizzle), `stick_x`/`stick_y`/`errno` via `MEM_B`/`MEM_BU`
/// (`^3` byte swizzle). Storing each LOGICAL struct byte at host offset
/// `(base + o) ^ 3` satisfies both: the two bytes of the big-endian `button`
/// u16 land at `0^3 = 3` (hi) and `1^3 = 2` (lo), so a native `MEM_HU` read at
/// `0^2 = 2` recovers `hi<<8 | lo` -- identical to the `^3` per-byte store
/// `osContGetQuery_recomp` already uses for `OSContStatus`. A flat
/// (unswizzled) copy, which a prior WIP did, put every field at the wrong lane
/// and the game saw garbage/no input -- the exact fail this shim's test pins.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContGetReadData_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let base_addr = RdramAddr::from_gpr(ctx.r4).offset() as usize;
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let channels = completed_controller_packet(ControllerPollKind::Read);
    // Diagnostic (opt-in via FN64_TRACE_CONT): proves PadMgr actually polls
    // input, and echoes what port-0 state the game is about to see -- the
    // observable evidence a scripted press reaches the game.
    if controller_trace_enabled() {
        if let Some(p0) = channels.first() {
            eprintln!(
                "[fn64-abi] osContGetReadData(pad@{base_addr:#x}): port0 button={:#06x} stick=({},{})",
                u16::from_be_bytes([p0.response[0], p0.response[1]]),
                p0.response[2] as i8,
                p0.response[3] as i8,
            );
        }
    }
    for (port, channel) in channels.into_iter().enumerate() {
        // A present standard controller reports errno == 0 and its live input;
        // an absent port reports no-response, matching the decomp's
        // `errno = CHNL_ERR(read)` branch (button/stick left zero here).
        let absent = channel.error != 0;
        // `read_data_response` is the `[button_hi, button_lo, stick_x, stick_y]`
        // PIF wire shape filled from the fed input (idle default).
        let resp = channel.response;
        // Assemble the 6-byte OSContPad in LOGICAL struct-offset order:
        // button hi@0, button lo@1, stick_x@2, stick_y@3, errno@4, pad@5.
        let (button_hi, button_lo, stick_x, stick_y, errno) = if absent {
            (0, 0, 0, 0, CONT_NO_RESPONSE_ERROR)
        } else {
            (resp[0], resp[1], resp[2], resp[3], 0)
        };
        let pad: [u8; 6] = [button_hi, button_lo, stick_x, stick_y, errno, 0];
        let base = base_addr + port * 6;
        // Store each logical byte at its `^3`-swizzled host position so the
        // game's MEM_HU(button)/MEM_BU(stick,errno) reads recover the right
        // values -- see the doc comment for the byte-order derivation.
        for (o, &b) in pad.iter().enumerate() {
            unsafe {
                storage.write_u8(
                    RdramAddr::from_offset(
                        u32::try_from(base + o).expect("OSContPad RDRAM address exceeds u32"),
                    ),
                    b,
                );
            }
        }
    }
    // osContGetReadData returns void; leave $v0 as the decomp does (unset).
}

fn controller_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("FN64_TRACE_CONT").is_some())
}

/// `osContInit(OSMesgQueue *mq, u8 *bitpattern, OSContStatus *data) -> s32`
/// -- `a0`=mq (`ctx->r4`), `a1`=bitpattern (`ctx->r5`), `a2`=data
/// (`ctx->r6`). Public libultra manual's documented one-time controller-
/// manager bring-up: probes all 4 ports and sets one bit per populated
/// port in `*bitpattern`. The supplied queue must be the initialized,
/// unshared `OS_EVENT_SI` target. The call blocks internally and publishes
/// both outputs only after the fabric executes the four-channel PIF query,
/// clears SI busy, raises MI SI, and posts completion. Function-table slot only
/// (`recomp_overlays.inl:2918`), reached from `PadMgr_Init`
/// (BOOT-PLAN.md rung 15's forcing-function call) -- implemented for real
/// against `PifModel`'s "port 0 populated, 1-3 absent" model
/// (`si.rs`'s module doc, this task's explicit scope).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContInit_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    if with_host(|host| host.controller_manager.initialized) {
        // The public Controller Manager manual specifies that only the first
        // call initializes the manager; later calls do not rewrite caller
        // buffers or reset an `osContSetCh` selection.
        ctx.r2 = 0;
        return;
    }
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let completion_message = controller_completion_message(mq_addr, true, "osContInit_recomp");
    let bitpattern_addr = RdramAddr::from_gpr(ctx.r5).offset() as usize;
    let data_addr = RdramAddr::from_gpr(ctx.r6).offset() as usize;
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    assert!(
        with_host(|host| host.controller_manager.initialize()),
        "osContInit_recomp: Controller Manager initialization raced after queue validation"
    );
    match start_controller_poll(mq_addr, ControllerPollKind::Query, MAXCONTROLLERS) {
        Ok(()) => {}
        Err(DeviceFault::SiBusy) => {
            with_host(|host| host.controller_manager = ControllerManagerState::default());
            ctx.r2 = u64::MAX;
            return;
        }
        Err(error) => panic!("osContInit_recomp: SI request failed: {error}"),
    }

    let delivered = match suspend_active_coroutine(Yield::BlockOnRecv {
        mq_addr,
        may_block: true,
    }) {
        Resume::Delivered(message) => message,
        other => panic!("osContInit_recomp: synchronous SI wait resumed with unexpected {other:?}"),
    };
    assert_eq!(
        delivered, completion_message,
        "osContInit_recomp: private queue delivered a non-SI-transaction message"
    );

    let channels = completed_controller_packet(ControllerPollKind::Query);
    assert_eq!(
        channels.len(),
        MAXCONTROLLERS,
        "osContInit_recomp: completed initialization did not probe all four ports"
    );
    let mut mask: u8 = 0;
    for (port, channel) in channels.into_iter().enumerate() {
        let resp = channel.response;
        let absent = channel.error != 0 || (resp[2] & fn64_runtime::si::CONT_ABSENT) != 0;
        if !absent {
            mask |= 1 << port;
        }
        // Write each OSContStatus entry SWIZZLED (`^3`), exactly like
        // osContGetQuery_recomp -- the game reads type/status/errno back
        // via MEM_HU/MEM_BU (recomp.h), so flat stores would transpose
        // them. type u16 = (resp[1]<<8)|resp[0] (controller.c:72);
        // absent ports report no-response in errno with type/status 0.
        let type_u16: u16 = ((resp[1] as u16) << 8) | resp[0] as u16;
        let entry: [u8; 4] = if absent {
            [0, 0, 0, CONT_NO_RESPONSE_ERROR]
        } else {
            [(type_u16 >> 8) as u8, (type_u16 & 0xFF) as u8, resp[2], 0]
        };
        let base = data_addr + port * 4;
        for (o, &b) in entry.iter().enumerate() {
            unsafe {
                storage.write_u8(
                    RdramAddr::from_offset(
                        u32::try_from(base + o).expect("OSContStatus RDRAM address exceeds u32"),
                    ),
                    b,
                );
            }
        }
    }
    unsafe {
        // ctlBitfield is a `u8*`: the decomp writes a SINGLE byte
        // `*ctlBitfield = bits` (controller.c:96), bits<=0x0F. Write one
        // swizzled byte (^3). A second byte at +1 would (a) be always 0 for a
        // u16 hi-byte and (b) clobber the adjacent variable -- and the flat
        // +0 store misses the swizzled sentinel address PadSetup_Init checks
        // (funcs_55.c 0x800CD414 `bnel $t7,0xFF`), so it would bail and skip
        // all controller-present stores.
        storage.write_u8(
            RdramAddr::from_offset(
                u32::try_from(bitpattern_addr)
                    .expect("controller bitpattern RDRAM address exceeds u32"),
            ),
            mask,
        );
    }
    ctx.r2 = 0;
}

/// `osContSetCh(u8 ch) -> s32` -- `a0`=`ctx->r4`. Public libultra manual:
/// restricts subsequent controller-manager polling to ports `0..ch`. The
/// manual also requires `osContInit` first: a premature call leaves the
/// manager at its default `MAXCONTROLLERS`, and `ch` may not exceed four.
/// This manager policy is separate from `PifModel`'s physical port state, so
/// an explicit raw PIF packet can still address a channel outside the
/// high-level polling prefix. The manual's approximate per-channel polling
/// savings are not an exact cycle formula; `DeviceFabric` retains its explicit
/// fixed SI compatibility latency, so timing parity for this selector remains
/// unverified. Function-table slot only
/// (`recomp_overlays.inl:2958`), reached from `PadMgr_Init`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContSetCh_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let channels = (ctx.r4 & 0xFF) as u8;
    with_host(|host| host.controller_manager.set_channels(channels));
    ctx.r2 = 0;
}

/// `osContStartQuery(OSMesgQueue *mq) -> s32` -- `a0`=`ctx->r4`. Public
/// libultra manual: kicks off an async PIF status-query DMA, posting
/// completion to `mq`. It enters the same timed SI fabric as raw register and
/// `__osSiRawStartDma` starts; no queue message or MI bit appears before the
/// deadline. Function-table slot only
/// (`recomp_overlays.inl:2933`), reached from `PadMgr_Init`/its polling
/// thread body (BOOT-PLAN.md rung 15).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContStartQuery_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let channels = with_host(|host| {
        assert!(
            host.controller_manager.initialized,
            "osContStartQuery_recomp: osContInit must initialize the Controller Manager first"
        );
        host.controller_manager.channels()
    });
    ctx.r2 = match start_controller_poll(mq_addr, ControllerPollKind::Query, channels) {
        Ok(()) => 0,
        Err(DeviceFault::SiBusy) => u64::MAX,
        Err(error) => panic!("osContStartQuery_recomp: {error}"),
    };
}

/// `osContStartReadData(OSMesgQueue *mq) -> s32` -- same shape/reasoning as
/// `osContStartQuery_recomp` (Public libultra manual's paired async
/// button/stick-read DMA kickoff). The active channel prefix is encoded into
/// device-owned PIF RAM at acceptance, and the response is sampled only at
/// the SI deadline; `osContGetReadData` decodes that completed image rather
/// than current host input. Function-table slot only
/// (`recomp_overlays.inl:2919`), reached from PadMgr's polling thread body.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContStartReadData_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let channels = with_host(|host| {
        assert!(
            host.controller_manager.initialized,
            "osContStartReadData_recomp: osContInit must initialize the Controller Manager first"
        );
        host.controller_manager.channels()
    });
    ctx.r2 = match start_controller_poll(mq_addr, ControllerPollKind::Read, channels) {
        Ok(()) => 0,
        Err(DeviceFault::SiBusy) => u64::MAX,
        Err(error) => panic!("osContStartReadData_recomp: {error}"),
    };
}

const PFS_ERR_NOPACK: u32 = 1;
const PFS_ERR_DEVICE: u32 = 11;
const MOTOR_INITIALIZED: u32 = 8;

fn rumble_error_code(error: fn64_runtime::RumbleError) -> u32 {
    match error {
        fn64_runtime::RumbleError::NoPak => PFS_ERR_NOPACK,
        fn64_runtime::RumbleError::WrongDevice => PFS_ERR_DEVICE,
    }
}

/// `osMotorInit(OSMesgQueue *mq, OSPfs *pfs, int channel) -> s32`.
/// Public `os_motor.h` defines the signature and `MOTOR_INITIALIZED`; the
/// function manual defines `PFS_ERR_NOPACK` and `PFS_ERR_DEVICE`. Successful
/// initialization records the documented OSPfs status/queue/channel prefix.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osMotorInit_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let queue = ctx.r4 as u32;
    let pfs = RdramAddr::from_gpr(ctx.r5);
    let channel = ctx.r6 as usize;
    let result = with_executor(|exec| exec.set_rumble(channel, false));
    match result {
        Ok(()) => {
            let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
            unsafe {
                storage.write_u32(pfs, MOTOR_INITIALIZED);
                storage.write_u32(
                    pfs.checked_add(4)
                        .expect("OSPfs queue field address overflow"),
                    queue,
                );
                storage.write_u32(
                    pfs.checked_add(8)
                        .expect("OSPfs channel field address overflow"),
                    channel as u32,
                );
            }
            ctx.r2 = 0;
        }
        Err(error) => ctx.r2 = rumble_error_code(error) as u64,
    }
}

fn motor_access(rdram: *mut u8, ctx: &mut RecompContext) {
    let pfs = RdramAddr::from_gpr(ctx.r4);
    let active = match ctx.r5 as u32 {
        0 => false,
        1 => true,
        access => panic!("__osMotorAccess_recomp: invalid MOTOR access value {access}"),
    };
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let status = unsafe { storage.read_u32(pfs) };
    assert!(
        status & MOTOR_INITIALIZED != 0,
        "__osMotorAccess_recomp: OSPfs was not initialized by osMotorInit"
    );
    let channel = unsafe {
        storage.read_u32(
            pfs.checked_add(8)
                .expect("OSPfs channel field address overflow"),
        )
    } as usize;
    ctx.r2 = match with_executor(|exec| exec.set_rumble(channel, active)) {
        Ok(()) => {
            crate::record_controller_operation(
                channel,
                fn64_runtime::ControllerOperationDevice::RumblePak,
                fn64_runtime::ControllerOperationKind::Control,
            );
            0
        }
        Err(error) => rumble_error_code(error) as u64,
    };
}

/// `__osMotorAccess(OSPfs *pfs, s32 accesslib) -> s32`. Public `os_motor.h`
/// defines `MOTOR_START = 1`, `MOTOR_STOP = 0`, and exposes `osMotorStart`/
/// `osMotorStop` as macros over this entry point.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osMotorAccess_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    motor_access(rdram, ctx);
}

/// Callable form of the public `osMotorStart(pfs)` macro.
///
/// # Safety
/// `rdram` and `ctx` must satisfy the recompiler ABI contract and name a valid `OSPfs`.
#[no_mangle]
pub unsafe extern "C" fn osMotorStart_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    ctx.r5 = 1;
    motor_access(rdram, ctx);
}

/// Callable form of the public `osMotorStop(pfs)` macro.
///
/// # Safety
/// `rdram` and `ctx` must satisfy the recompiler ABI contract and name a valid `OSPfs`.
#[no_mangle]
pub unsafe extern "C" fn osMotorStop_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    ctx.r5 = 0;
    motor_access(rdram, ctx);
}

#[cfg(test)]
mod tests;
