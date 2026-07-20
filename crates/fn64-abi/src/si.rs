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
                pif_ram[rx_off..next]
                    .copy_from_slice(&executor.pif().read_data_response(port));
                if matches!(
                    executor.pif().port_state(port),
                    fn64_runtime::PortState::StandardControllerNoPak
                        | fn64_runtime::PortState::StandardControllerControllerPak
                        | fn64_runtime::PortState::StandardControllerRumblePak
                        | fn64_runtime::PortState::StandardControllerTransferPak
                ) {
                    observations.record_controller(
                        now,
                        port,
                        fn64_runtime::ControllerOperationDevice::StandardController,
                        fn64_runtime::ControllerOperationKind::Read,
                    );
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
                    fn64_runtime::PortState::VoiceRecognitionUnit => panic!(
                        "SI PIF accessory read command 0x02 on Voice channel {port} is not implemented"
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
                    fn64_runtime::PortState::VoiceRecognitionUnit => panic!(
                        "SI PIF accessory write command 0x03 on Voice channel {port} is not implemented"
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
            _ => panic!(
                "SI PIF command {command:#04x} on channel {port} with tx={tx_size} rx={rx_size} is not implemented"
            ),
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
/// Pak block I/O uses the same physical
/// data pages as the high-level PFS model; management-page inode/directory
/// decoding remains open. Transfer Pak power, status/mode, bank, and Game Boy
/// bus windows use typed cartridge ROM/RAM plus common mapper state. MBC3 RTC
/// oscillator, immutable latch, halt, 9-bit day, and sticky carry state advance
/// on the same guest clock through raw and high-level paths. A separate host
/// API imports/exports the exact-ROM-bound RTC sidecar with explicit wall-time
/// samples. Raw Voice result reads, status, captured five-write initialization,
/// and initialization/clear/start/stop controls share the high-level
/// VoiceUnit lifecycle. Region-dependent `0x0A` dictionary staging, `0x0D`
/// power/gain writes, result reads without a host result, and unestablished
/// `0x0C` forms record a command-specific unsupported event before trapping.
/// High-level shims do not silently authorize fabricated raw-protocol success.
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
            let base = dram_addr.offset() as usize;
            let bytes: Vec<u8> = (0..64)
                .map(|i| unsafe { rdram.add((base + i) ^ 3).read() })
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
        rdram,
        rdram_len,
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
/// Fills the whole `OSContStatus[MAXCONTROLLERS]` array, one entry per port
/// (`__osContGetInitData`, `oot-decomp/src/libultra/io/controller.c:58`,
/// iterates all `__osMaxControllers` and advances `data++` each). Each
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
    let pif = with_executor(|exec| *exec.pif());
    for port in 0..MAXCONTROLLERS {
        let resp = pif.query_response(port);
        let absent = (resp[2] & fn64_runtime::si::CONT_ABSENT) != 0;
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

/// `MAXCONTROLLERS` (`oot-decomp/include/ultra64/controller.h:9`): the N64
/// has four controller ports; `osContGetQuery` fills one `OSContStatus` per
/// port.
const MAXCONTROLLERS: usize = 4;

/// `osContGetReadData(OSContPad *pad) -> void` -- `a0`=`ctx->r4`, the base of
/// an `OSContPad[MAXCONTROLLERS]` array (`padMgr->pads`, decomp
/// `oot-decomp/src/code/padmgr.c:364` `osContGetReadData(padMgr->pads)`).
/// This is the INPUT SEAM's game-facing half: the per-port button/stick state
/// a host harness fed via `PifModel::set_input` lands in the pad array the
/// game reads each retrace to drive Link.
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
    let pif = with_executor(|exec| *exec.pif());
    // Diagnostic (opt-in via FN64_TRACE_CONT): proves PadMgr actually polls
    // input, and echoes what port-0 state the game is about to see -- the
    // observable evidence a scripted press reaches the game.
    if std::env::var_os("FN64_TRACE_CONT").is_some() {
        let p0 = pif.read_data_response(0);
        eprintln!(
            "[fn64-abi] osContGetReadData(pad@{base_addr:#x}): port0 button={:#06x} stick=({},{})",
            u16::from_be_bytes([p0[0], p0[1]]),
            p0[2] as i8,
            p0[3] as i8,
        );
    }
    for port in 0..MAXCONTROLLERS {
        // A present standard controller reports errno == 0 and its live input;
        // an absent port reports no-response, matching the decomp's
        // `errno = CHNL_ERR(read)` branch (button/stick left zero here).
        let absent = (pif.query_response(port)[2] & fn64_runtime::si::CONT_ABSENT) != 0;
        // `read_data_response` is the `[button_hi, button_lo, stick_x, stick_y]`
        // PIF wire shape filled from the fed input (idle default).
        let resp = pif.read_data_response(port);
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
        if matches!(
            pif.port_state(port),
            fn64_runtime::PortState::StandardControllerNoPak
                | fn64_runtime::PortState::StandardControllerControllerPak
                | fn64_runtime::PortState::StandardControllerRumblePak
                | fn64_runtime::PortState::StandardControllerTransferPak
        ) {
            crate::record_controller_operation(
                port,
                fn64_runtime::ControllerOperationDevice::StandardController,
                fn64_runtime::ControllerOperationKind::Read,
            );
        }
    }
    // osContGetReadData returns void; leave $v0 as the decomp does (unset).
}

/// `osContInit(OSMesgQueue *mq, u8 *bitpattern, OSContStatus *data) -> s32`
/// -- `a0`=mq (`ctx->r4`), `a1`=bitpattern (`ctx->r5`), `a2`=data
/// (`ctx->r6`). Public libultra manual's documented one-time controller-
/// manager bring-up: probes all 4 ports and sets one bit per populated
/// port in `*bitpattern`. Function-table slot only
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
    let bitpattern_addr = RdramAddr::from_gpr(ctx.r5).offset() as usize;
    let data_addr = RdramAddr::from_gpr(ctx.r6).offset() as usize;
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let mut mask: u8 = 0;
    with_executor(|exec| {
        let pif = *exec.pif();
        for port in 0..4usize {
            let resp = pif.query_response(port);
            let absent = (resp[2] & fn64_runtime::si::CONT_ABSENT) != 0;
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
                            u32::try_from(base + o)
                                .expect("OSContStatus RDRAM address exceeds u32"),
                        ),
                        b,
                    );
                }
            }
        }
    });
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
/// restricts subsequent controller-manager polling to the first `ch`
/// channels. This crate's `PifModel` always reports the same fixed 4-port
/// state regardless of channel count (`si.rs`'s module doc: "one standard
/// controller on port 0... ports 1-3 absent" is not parameterized by a
/// runtime channel-count setting) -- stored as plain host state for
/// fidelity/logging, with no other behavioral effect, matching
/// `osAiSetFrequency_recomp`'s existing "store it, no consumer needs it
/// yet" pattern for an unconsumed configuration value. Function-table slot
/// only (`recomp_overlays.inl:2958`), reached from `PadMgr_Init`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContSetCh_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    CONT_CHANNELS.with(|cell| cell.set((ctx.r4 & 0xFF) as u8));
    ctx.r2 = 0;
}

thread_local! {
    static CONT_CHANNELS: Cell<u8> = const { Cell::new(4) };
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
    const OS_EVENT_SI: u32 = 5;
    with_executor(|exec| exec.set_event_mesg(OS_EVENT_SI, mq_addr, 0));
    ctx.r2 = match crate::pi::start_live_si_dma(
        fn64_runtime::SiDmaRequest {
            kind: fn64_runtime::SiDmaKind::ControllerQuery,
            dram_addr: RdramAddr::from_offset(0),
        },
        std::ptr::null_mut(),
        0,
    ) {
        Ok(()) => 0,
        Err(DeviceFault::SiBusy) => u64::MAX,
        Err(error) => panic!("osContStartQuery_recomp: {error}"),
    };
}

/// `osContStartReadData(OSMesgQueue *mq) -> s32` -- same shape/reasoning as
/// `osContStartQuery_recomp` (Public libultra manual's paired async
/// button/stick-read DMA kickoff). Function-table slot only
/// (`recomp_overlays.inl:2919`), reached from PadMgr's polling thread body.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContStartReadData_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    const OS_EVENT_SI: u32 = 5;
    with_executor(|exec| exec.set_event_mesg(OS_EVENT_SI, mq_addr, 0));
    ctx.r2 = match crate::pi::start_live_si_dma(
        fn64_runtime::SiDmaRequest {
            kind: fn64_runtime::SiDmaKind::ControllerRead,
            dram_addr: RdramAddr::from_offset(0),
        },
        std::ptr::null_mut(),
        0,
    ) {
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
mod tests {
    use super::*;
    use crate::pi::{load_rom, set_save};
    use crate::test_support::*;

    fn install_eeprom(kind: fn64_runtime::SaveType) {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            kind,
        )));
    }

    fn write_logical_bytes(rdram: &mut [u8], offset: u32, bytes: &[u8]) {
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        for (index, byte) in bytes.iter().copied().enumerate() {
            view.write_u8(
                RdramAddr::from_offset(offset + u32::try_from(index).unwrap()),
                byte,
            );
        }
    }

    fn read_logical_bytes(rdram: &[u8], offset: u32, len: usize) -> Vec<u8> {
        let view = fn64_runtime::RdramView::from_storage(rdram);
        (0..len)
            .map(|index| {
                view.read_u8(RdramAddr::from_offset(
                    offset + u32::try_from(index).unwrap(),
                ))
            })
            .collect()
    }

    fn raw_si_round_trip(rdram: &mut [u8]) {
        let write_deadline = crate::sim_time().saturating_add(1);
        let mut ctx = ctx_zeroed();
        ctx.r4 = 1;
        ctx.r5 = 0x8000_0000;
        unsafe { __osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        crate::advance_virtual_time(write_deadline);

        ctx.r4 = 0;
        unsafe { __osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        crate::advance_virtual_time(write_deadline + 1);
    }

    /// Regression for the OoT-boot `PadSetup_Init` EXC_BAD_ACCESS: the real
    /// `osContGetQuery(OSContStatus* data)` takes its ONLY argument (the
    /// array pointer) in `$a0`/`ctx.r4`; the buggy prior signature read it
    /// from `$a1`/`ctx.r5`, which the real call site (`funcs_55.c:2193`)
    /// leaves as stale garbage, so the shim dereferenced a wild pointer.
    ///
    /// This test wires `r4` and `r5` to two DIFFERENT, both-valid rdram
    /// addresses and asserts the OSContStatus array lands at `r4`'s address
    /// (and that `r5`'s address is untouched) -- so reintroducing the bug
    /// (reading the pointer from `r5`) makes it fail rather than pass. It
    /// also checks all four ports are filled with the exact byte-swizzled
    /// values the game's own MEM_HU/MEM_BU reads recover: port 0 a present
    /// standard controller (`type == 0x0005 == CONT_TYPE_NORMAL`, `errno ==
    /// 0`), ports 1-3 absent (`errno == CONT_NO_RESPONSE_ERROR == 0x08`).
    #[test]
    fn os_cont_get_query_reads_array_pointer_from_a0_and_fills_all_ports() {
        // Fresh PIF state (default: port 0 standard, 1-3 absent).
        with_executor(|exec| *exec = fn64_runtime::Executor::new());

        let mut buf = vec![0u8; fn64_runtime::RDRAM_MMIO_WINDOW_END as usize];

        // Two distinct, both-valid vram addresses. r4 = the REAL data
        // pointer the game passes; r5 = a decoy the buggy shim would have
        // used. Kept 0x40 apart so the 0x10-byte (4 * OSContStatus) write
        // regions can't overlap.
        let data_vram: u64 = 0xFFFF_FFFF_8020_0000;
        let decoy_vram: u64 = 0xFFFF_FFFF_8020_0040;
        let data_off = RdramAddr::from_gpr(data_vram).offset() as usize;
        let decoy_off = RdramAddr::from_gpr(decoy_vram).offset() as usize;

        // Pre-poison the decoy region with a sentinel so "untouched" is a
        // real, checkable statement, not "happened to already be zero".
        for i in 0..0x10 {
            buf[decoy_off + i] = 0xAB;
        }

        let mut ctx = ctx_zeroed();
        ctx.r4 = data_vram;
        ctx.r5 = decoy_vram;
        unsafe { osContGetQuery_recomp(buf.as_mut_ptr(), &mut ctx as *mut _) };

        // Read each OSContStatus exactly as the generated game code does:
        // MEM_HU(base, 0) = *(u16*)(rdram + (base ^ 2)); MEM_BU(base, 3) =
        // *(u8*)(rdram + ((base + 3) ^ 3)) (recomp.h). Reading through the
        // same swizzle the reader uses is what makes this a faithful check
        // rather than an encoding of whatever byte order the writer chose.
        let read_type = |base: usize| -> u16 {
            let a = base ^ 2;
            u16::from_ne_bytes([buf[a], buf[a + 1]])
        };
        let read_errno = |base: usize| -> u8 { buf[(base + 3) ^ 3] };

        // Port 0: present standard controller.
        let p0 = data_off;
        assert_eq!(
            read_type(p0),
            0x0005,
            "port 0 type must read as CONT_TYPE_NORMAL (0x0005) via the game's MEM_HU"
        );
        assert_eq!(read_errno(p0), 0, "port 0 (present) has no channel error");

        // Ports 1-3: absent -> non-zero errno so PadSetup_Init skips them.
        for port in 1..4usize {
            let base = data_off + port * 4;
            assert_eq!(
                read_errno(base),
                0x08,
                "absent port {port} must report CONT_NO_RESPONSE_ERROR (0x08)"
            );
        }

        // The decoy region (r5's address) must be completely untouched --
        // proves the pointer came from r4, not r5. Under the old bug this
        // region would have been written (and r4's region left as zeros).
        for i in 0..0x10 {
            assert_eq!(
                buf[decoy_off + i],
                0xAB,
                "byte {i} at the r5/decoy address was written -- the shim read \
                 its pointer from the wrong register (the reintroduced bug)"
            );
        }
    }

    /// The INPUT-SEAM contract: a host harness feeds controller state via
    /// `set_controller_state`, and `osContGetReadData_recomp` writes it into
    /// the game's `OSContPad[MAXCONTROLLERS]` array at `$a0`/`ctx.r4`, in the
    /// exact byte-swizzled layout the game's own MEM_HU/MEM_BU reads recover.
    ///
    /// Fail-against-the-bug: it reads every field back through the SAME
    /// swizzle the recompiled game uses (`button` via MEM_HU `^2`, `stick`/
    /// `errno` via MEM_BU `^3`, recomp.h:104-108). A flat/unswizzled copy (the
    /// prior WIP) or a wrong button bit lands the bytes at the wrong lanes and
    /// this fails. It also checks the button HIGH byte carries `BTN_START`
    /// (0x1000) -- the scripted-boot press -- so an endianness flip fails too.
    #[test]
    fn os_cont_get_read_data_writes_swizzled_input_into_pad_array() {
        // Fresh state, then feed a distinctive input on port 0: Start+A held,
        // stick pushed. (BTN_A = 0x8000, BTN_START = 0x1000 -> 0x9000.)
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        set_controller_state(0, 0x9000, -50, 70);

        let mut buf = vec![0u8; fn64_runtime::RDRAM_MMIO_WINDOW_END as usize];
        let pad_vram: u64 = 0xFFFF_FFFF_8020_0000;
        let pad_off = RdramAddr::from_gpr(pad_vram).offset() as usize;

        let mut ctx = ctx_zeroed();
        ctx.r4 = pad_vram;
        unsafe { osContGetReadData_recomp(buf.as_mut_ptr(), &mut ctx as *mut _) };

        // Read each OSContPad field EXACTLY as the recompiled game does:
        // button via MEM_HU (`^2` halfword), the s8/u8 fields via MEM_BU
        // (`^3` byte). OSContPad size = 0x06 (controller.h:132).
        let read_button = |base: usize| -> u16 {
            let a = base ^ 2;
            u16::from_ne_bytes([buf[a], buf[a + 1]])
        };
        let read_i8 = |base: usize, o: usize| -> i8 { buf[(base + o) ^ 3] as i8 };
        let read_u8 = |base: usize, o: usize| -> u8 { buf[(base + o) ^ 3] };

        // Port 0: present -> errno 0 and the exact fed input.
        let p0 = pad_off;
        assert_eq!(
            read_button(p0),
            0x9000,
            "port 0 button must read back BTN_A|BTN_START (0x9000) via the game's MEM_HU"
        );
        assert_ne!(
            read_button(p0) & 0x1000,
            0,
            "BTN_START (0x1000) must be set -- the scripted press must reach the game"
        );
        assert_eq!(read_i8(p0, 2), -50, "stick_x");
        assert_eq!(read_i8(p0, 3), 70, "stick_y");
        assert_eq!(read_u8(p0, 4), 0, "port 0 (present) errno == 0");

        // Ports 1-3: absent -> nonzero errno so the game ignores them.
        for port in 1..MAXCONTROLLERS {
            let base = pad_off + port * 6;
            assert_eq!(
                read_u8(base, 4),
                CONT_NO_RESPONSE_ERROR,
                "absent port {port} errno must be CONT_NO_RESPONSE_ERROR (0x08)"
            );
            assert_eq!(read_button(base), 0, "absent port {port} button zeroed");
        }
    }

    #[test]
    fn high_level_input_evidence_excludes_voice_ports() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);
        set_controller_port_state(1, fn64_runtime::PortState::StandardControllerNoPak);

        let mut rdram = vec![0u8; 0x40];
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000;
        unsafe { osContGetReadData_recomp(rdram.as_mut_ptr(), &mut ctx) };

        assert_eq!(
            crate::copy_controller_operations(),
            vec![fn64_runtime::ControllerOperationEvent {
                at: Cycles::ZERO,
                port: 1,
                device: fn64_runtime::ControllerOperationDevice::StandardController,
                operation: fn64_runtime::ControllerOperationKind::Read,
            }]
        );
    }

    /// `__osSiRawStartDma_recomp` is real this wave (replacing the prior
    /// loud trap) -- verifies a port-0 status-query channel (tx_size=1,
    /// rx_size=3) gets `PifModel::query_response(0)`'s real bytes written
    /// back, and that an absent port (1) gets `CONT_ABSENT` set.
    #[test]
    fn os_si_raw_start_dma_fills_real_pif_query_responses() {
        let mut rdram = vec![0u8; 64];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            // Channel 0: tx_size=1, rx_size=3, cmd=0xFF (query), 1 tx byte,
            // then 3 response bytes to be filled at offset 3..6.
            view.write_u8(RdramAddr::from_offset(0), 1);
            view.write_u8(RdramAddr::from_offset(1), 3);
            view.write_u8(RdramAddr::from_offset(2), 0xFF);
            view.write_u8(RdramAddr::from_offset(3), 0);
            // Channel 1 starts after channel 0's three response bytes.
            view.write_u8(RdramAddr::from_offset(6), 1);
            view.write_u8(RdramAddr::from_offset(7), 3);
            view.write_u8(RdramAddr::from_offset(8), 0xFF);
            view.write_u8(RdramAddr::from_offset(9), 0);
            view.write_u8(RdramAddr::from_offset(12), 0xFE);
        }

        let mut ctx = ctx_zeroed();
        ctx.r4 = 1; // OS_WRITE: DRAM -> PIF, then execute the command block.
        ctx.r5 = 0x8000_0000; // dramAddr vram -> rdram offset 0
        unsafe { __osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0);
        crate::advance_virtual_time(1);

        ctx.r4 = 0; // OS_READ: PIF -> DRAM response copy.
        unsafe { __osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        crate::advance_virtual_time(2);

        // Port 0: standard controller, no pak, not absent.
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            (3..6)
                .map(|offset| view.read_u8(RdramAddr::from_offset(offset)))
                .collect::<Vec<_>>(),
            vec![0x05, 0x00, 0x00]
        );
        // Port 1: absent bit set.
        assert_eq!(
            view.read_u8(RdramAddr::from_offset(11)) & fn64_runtime::CONT_ABSENT,
            fn64_runtime::CONT_ABSENT
        );
    }

    #[test]
    fn raw_pif_records_operations_but_not_accessory_probes() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerNoPak);

        let mut input = [0u8; 64];
        input[0] = 1;
        input[1] = 4;
        input[2] = 0x01;
        input[7] = 0xfe;
        let input_observations = crate::pi::with_pi_dma("raw controller input", |pi_dma| {
            execute_controller_pif(Cycles::new(11), &mut input, pi_dma)
        });
        assert_eq!(
            input_observations.controller_operations,
            vec![fn64_runtime::ControllerOperationEvent {
                at: Cycles::new(11),
                port: 0,
                device: fn64_runtime::ControllerOperationDevice::StandardController,
                operation: fn64_runtime::ControllerOperationKind::Read,
            }]
        );

        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerRumblePak);
        let encoded_probe = ACCESSORY_ADDR_RUMBLE_PROBE
            | u16::from(accessory_address_crc(ACCESSORY_ADDR_RUMBLE_PROBE));
        let mut probe = [0u8; 64];
        probe[0] = 3;
        probe[1] = 33;
        probe[2] = 0x02;
        probe[3..5].copy_from_slice(&encoded_probe.to_be_bytes());
        probe[38] = 0xfe;
        let probe_observations = crate::pi::with_pi_dma("raw Rumble Pak probe", |pi_dma| {
            execute_controller_pif(Cycles::new(12), &mut probe, pi_dma)
        });
        assert!(probe_observations.controller_operations.is_empty());

        let encoded_motor = ACCESSORY_ADDR_RUMBLE_MOTOR
            | u16::from(accessory_address_crc(ACCESSORY_ADDR_RUMBLE_MOTOR));
        let mut motor = [0u8; 64];
        motor[0] = 35;
        motor[1] = 1;
        motor[2] = 0x03;
        motor[3..5].copy_from_slice(&encoded_motor.to_be_bytes());
        motor[5..37].fill(1);
        motor[38] = 0xfe;
        let motor_observations = crate::pi::with_pi_dma("raw Rumble Pak motor", |pi_dma| {
            execute_controller_pif(Cycles::new(13), &mut motor, pi_dma)
        });
        assert_eq!(
            motor_observations.controller_operations,
            vec![fn64_runtime::ControllerOperationEvent {
                at: Cycles::new(13),
                port: 0,
                device: fn64_runtime::ControllerOperationDevice::RumblePak,
                operation: fn64_runtime::ControllerOperationKind::Control,
            }]
        );

        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
        let mut gb_rom = vec![0xff; 0x8000];
        gb_rom[0x147] = 0;
        insert_transfer_pak_cartridge(0, gb_rom, None).unwrap();
        let mut transfer = [0u8; 64];
        transfer[0] = 3;
        transfer[1] = 33;
        transfer[2] = 0x02;
        transfer[3..5].copy_from_slice(&encoded_probe.to_be_bytes());
        transfer[38] = 0xfe;
        let transfer_observations = crate::pi::with_pi_dma("raw Transfer Pak read", |pi_dma| {
            execute_controller_pif(Cycles::new(14), &mut transfer, pi_dma)
        });
        assert_eq!(
            transfer_observations.controller_operations,
            vec![fn64_runtime::ControllerOperationEvent {
                at: Cycles::new(14),
                port: 0,
                device: fn64_runtime::ControllerOperationDevice::TransferPak,
                operation: fn64_runtime::ControllerOperationKind::Read,
            }]
        );
    }

    #[test]
    fn raw_voice_info_and_high_level_init_share_readiness_state() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);

        let query = || {
            let mut packet = [0u8; 64];
            packet[0] = 1;
            packet[1] = 3;
            packet[2] = 0x00;
            packet[6] = 0xFE;
            packet
        };

        let mut before = query();
        crate::pi::with_pi_dma("raw Voice pre-init Info", |pi_dma| {
            execute_controller_pif(Cycles::ZERO, &mut before, pi_dma)
        });
        assert_eq!(&before[3..6], &[0x00, 0x01, 0x00]);

        let mut rdram = vec![0u8; 0x100];
        let mut init = ctx_zeroed();
        init.r4 = 0x8000_0020;
        init.r5 = 0x8000_0040;
        init.r6 = 0;
        unsafe { crate::voice::osVoiceInit_recomp(rdram.as_mut_ptr(), &mut init) };
        assert_eq!(init.r2, 0);

        let mut after = query();
        crate::pi::with_pi_dma("raw Voice post-init Info", |pi_dma| {
            execute_controller_pif(Cycles::ZERO, &mut after, pi_dma)
        });
        assert_eq!(&after[3..6], &[0x00, 0x01, 0x01]);
    }

    #[test]
    fn raw_voice_captured_initialization_sequence_reaches_shared_readiness() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);

        let run = |packet: &mut [u8; 64]| {
            crate::pi::with_pi_dma("raw Voice initialization", |pi_dma| {
                execute_controller_pif(Cycles::new(17), packet, pi_dma)
            });
        };
        for encoded_address in [0x1E0Cu16, 0x6E07, 0x080E, 0x5618, 0x030F] {
            let mut packet = [0u8; 64];
            packet[0] = 3;
            packet[1] = 1;
            packet[2] = 0x0D;
            packet[3..5].copy_from_slice(&encoded_address.to_be_bytes());
            packet[6] = 0xFE;
            run(&mut packet);
            assert_eq!(packet[5], 0);
        }
        assert_eq!(
            with_executor(|exec| exec
                .voice_unit(0)
                .unwrap()
                .evidence_snapshot()
                .raw_init_step),
            5
        );

        let mut finish = [0u8; 64];
        finish[0] = 7;
        finish[1] = 1;
        finish[2] = 0x0C;
        finish[5..9].copy_from_slice(&[0x00, 0x00, 0x01, 0x00]);
        finish[10] = 0xFE;
        run(&mut finish);
        assert_eq!(finish[9], 0x97);
        assert!(with_executor(|exec| exec
            .voice_unit(0)
            .unwrap()
            .initialized()));
    }

    #[test]
    fn joybus_crc_matches_public_voice_capture_vectors() {
        assert_eq!(joybus_data_crc(&[0x00, 0x00, 0x01, 0x00]), 0x97);
        assert_eq!(joybus_data_crc(&[0x02, 0x00, 0x3B, 0x00]), 0xF9);
        assert_eq!(joybus_data_crc(&[0x05, 0x00, 0x00, 0x00]), 0x4E);
        assert_eq!(joybus_data_crc(&[0x00, 0x00, 0x06, 0x00]), 0x78);
        assert_eq!(joybus_data_crc(&[0x01, 0x00]), 0x97);
        assert_eq!(joybus_data_crc(&[0x05, 0x00]), 0x44);
    }

    #[test]
    fn raw_voice_status_clear_and_start_share_high_level_state() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);

        let run = |packet: &mut [u8; 64]| {
            crate::pi::with_pi_dma("raw Voice state convergence", |pi_dma| {
                execute_controller_pif(Cycles::new(23), packet, pi_dma)
            });
        };
        let status_packet = || {
            let mut packet = [0u8; 64];
            packet[0] = 3;
            packet[1] = 3;
            packet[2] = 0x0B;
            packet[8] = 0xFE;
            packet
        };

        let mut pre_init = status_packet();
        run(&mut pre_init);
        assert_eq!(&pre_init[5..8], &[0x01, 0x00, 0x97]);

        let mut rdram = vec![0u8; 0x100];
        let mut init = ctx_zeroed();
        init.r4 = 0x8000_0020;
        init.r5 = 0x8000_0040;
        init.r6 = 0;
        unsafe { crate::voice::osVoiceInit_recomp(rdram.as_mut_ptr(), &mut init) };
        assert_eq!(init.r2, 0);

        let mut ready = status_packet();
        run(&mut ready);
        assert_eq!(&ready[5..8], &[0x00, 0x00, 0x00]);

        let mut clear = [0u8; 64];
        clear[0] = 7;
        clear[1] = 1;
        clear[2] = 0x0C;
        clear[5..9].copy_from_slice(&[0x02, 0x00, 0x01, 0x00]);
        clear[10] = 0xFE;
        run(&mut clear);
        assert_eq!(clear[9], joybus_data_crc(&[0x02, 0x00, 0x01, 0x00]));
        assert_eq!(
            with_executor(|exec| exec
                .voice_unit(0)
                .unwrap()
                .evidence_snapshot()
                .expected_words),
            Some(1)
        );

        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
        for (index, byte) in b"voice\0".iter().copied().enumerate() {
            unsafe {
                storage.write_u8(RdramAddr::from_offset(0x80 + index as u32), byte);
            }
        }
        let mut word = ctx_zeroed();
        word.r4 = 0x8000_0040;
        word.r5 = 0x8000_0080;
        unsafe { crate::voice::osVoiceSetWord_recomp(rdram.as_mut_ptr(), &mut word) };
        assert_eq!(word.r2, 0);

        let mut start = [0u8; 64];
        start[0] = 7;
        start[1] = 1;
        start[2] = 0x0C;
        start[5..9].copy_from_slice(&[0x00, 0x00, 0x06, 0x00]);
        start[10] = 0xFE;
        run(&mut start);
        assert_eq!(start[9], 0x78);

        let mut get = ctx_zeroed();
        get.r4 = 0x8000_0040;
        get.r5 = 0x8000_00C0;
        unsafe { crate::voice::osVoiceGetReadData_recomp(rdram.as_mut_ptr(), &mut get) };
        assert_ne!(get.r2, 0);
        assert_eq!(
            unsafe { storage.read_u8(RdramAddr::from_offset(0x4C)) },
            fn64_runtime::voice::VOICE_STATUS_START
        );

        let mut started = status_packet();
        run(&mut started);
        assert_eq!(&started[5..8], &[0x01, 0x00, 0x97]);

        crate::voice::mark_voice_detected(0);
        let mut busy = status_packet();
        run(&mut busy);
        assert_eq!(&busy[5..8], &[0x05, 0x00, 0x44]);

        crate::voice::inject_voice_result(0, fn64_runtime::VoiceData::default());
        let mut ended = status_packet();
        run(&mut ended);
        assert_eq!(&ended[5..8], &[0x07, 0x00, joybus_data_crc(&[0x07, 0x00])]);

        let mut stop = [0u8; 64];
        stop[0] = 7;
        stop[1] = 1;
        stop[2] = 0x0C;
        stop[5..9].copy_from_slice(&[0x05, 0x00, 0x00, 0x00]);
        stop[10] = 0xFE;
        run(&mut stop);
        assert_eq!(stop[9], 0x4E);
        let mut canceled = status_packet();
        run(&mut canceled);
        assert_eq!(
            &canceled[5..8],
            &[0x03, 0x00, joybus_data_crc(&[0x03, 0x00])]
        );
    }

    #[test]
    fn raw_voice_result_matches_public_capture_layout_and_consumes_shared_result() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);
        with_executor(|exec| {
            let voice = exec.voice_unit_mut(0).unwrap();
            voice.initialize();
            voice.clear_dictionary(1).unwrap();
            voice.set_word(b"capture").unwrap();
            voice.start().unwrap();
            voice
                .inject_result(fn64_runtime::VoiceData {
                    warning: 0,
                    answer_num: 2,
                    voice_level: 0x059D,
                    voice_sn: 0x077C,
                    voice_time: 0x04B0,
                    answer: [0, 4, 0x10, 0x14, 5],
                    distance: [0x0477, 0x04CC, 0x04F9, 0x0503, 0x0512],
                })
                .unwrap();
        });

        let mut packet = [0u8; 64];
        packet[0] = 3;
        packet[1] = 37;
        packet[2] = 0x09;
        packet[42] = 0xFE;
        let observations = crate::pi::with_pi_dma("raw Voice result", |pi_dma| {
            execute_controller_pif(Cycles::new(29), &mut packet, pi_dma)
        });
        assert_eq!(
            observations.controller_operations,
            vec![fn64_runtime::ControllerOperationEvent {
                at: Cycles::new(29),
                port: 0,
                device: fn64_runtime::ControllerOperationDevice::VoiceRecognitionUnit,
                operation: fn64_runtime::ControllerOperationKind::Read,
            }]
        );
        assert_eq!(
            &packet[5..42],
            &[
                0x80, 0x00, 0x0F, 0x00, 0x00, 0x00, 0x02, 0x00, 0x9D, 0x05, 0x7C, 0x07, 0xB0, 0x04,
                0x00, 0x00, 0x77, 0x04, 0x04, 0x00, 0xCC, 0x04, 0x10, 0x00, 0xF9, 0x04, 0x14, 0x00,
                0x03, 0x05, 0x05, 0x00, 0x12, 0x05, 0x40, 0x00, 0x97,
            ]
        );
        assert_eq!(
            with_executor(|exec| exec.voice_unit(0).unwrap().status()),
            fn64_runtime::voice::VOICE_STATUS_READY
        );
    }

    #[test]
    fn unestablished_raw_voice_payload_records_a_typed_loud_trap() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::VoiceRecognitionUnit);
        fn64_runtime::arm_unsupported_events(None).unwrap();

        let mut packet = [0u8; 64];
        packet[0] = 7;
        packet[1] = 1;
        packet[2] = 0x0C;
        packet[5..9].copy_from_slice(&[0x00, 0x00, 0x07, 0x00]);
        packet[10] = 0xFE;
        let trapped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::pi::with_pi_dma("unsupported raw Voice payload", |pi_dma| {
                execute_controller_pif(Cycles::new(41), &mut packet, pi_dma)
            });
        }));
        assert!(trapped.is_err());

        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].subsystem, fn64_runtime::UnsupportedSubsystem::Abi);
        assert_eq!(events[0].operation, "abi.si.voice-command-0c");
        assert_eq!(events[0].guest_cycle, Some(Cycles::new(41)));
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        assert!(events[0].context.contains("00 00 07 00"));
        fn64_runtime::complete_unsupported_observation(Cycles::new(41), &"0".repeat(64));
    }

    #[test]
    fn raw_eeprom_and_high_level_shims_share_one_backing_store() {
        install_eeprom(fn64_runtime::SaveType::Eeprom4k);
        let mut rdram = vec![0u8; 0x200];
        let raw_payload = [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87];

        let mut raw_write = vec![0; 18];
        raw_write[4] = 10;
        raw_write[5] = 1;
        raw_write[6] = 0x05;
        raw_write[7] = 7;
        raw_write[8..16].copy_from_slice(&raw_payload);
        raw_write[17] = 0xFE;
        write_logical_bytes(&mut rdram, 0, &raw_write);
        raw_si_round_trip(&mut rdram);
        assert_eq!(read_logical_bytes(&rdram, 16, 1), vec![0]);

        let mut high_read = ctx_zeroed();
        high_read.r5 = 7;
        high_read.r6 = 0x8000_0080;
        unsafe { crate::save::osEepromRead_recomp(rdram.as_mut_ptr(), &mut high_read) };
        assert_eq!(high_read.r2, 0);
        assert_eq!(read_logical_bytes(&rdram, 0x80, 8), raw_payload);

        let shim_payload = [0xF8, 0xE7, 0xD6, 0xC5, 0xB4, 0xA3, 0x92, 0x81];
        write_logical_bytes(&mut rdram, 0xA0, &shim_payload);
        let mut high_write = ctx_zeroed();
        high_write.r5 = 7;
        high_write.r6 = 0x8000_00A0;
        unsafe { crate::save::osEepromWrite_recomp(rdram.as_mut_ptr(), &mut high_write) };
        assert_eq!(high_write.r2, 0);
        crate::advance_virtual_time(
            crate::sim_time().saturating_add(fn64_runtime::EEPROM_WRITE_CYCLES.get()),
        );

        let mut raw_read = vec![0; 17];
        raw_read[4] = 2;
        raw_read[5] = 8;
        raw_read[6] = 0x04;
        raw_read[7] = 7;
        raw_read[16] = 0xFE;
        write_logical_bytes(&mut rdram, 0, &raw_read);
        raw_si_round_trip(&mut rdram);
        assert_eq!(read_logical_bytes(&rdram, 8, 8), shim_payload);
        let operations = crate::copy_save_operations();
        assert_eq!(operations.len(), 4);
        assert_eq!(
            operations
                .iter()
                .map(|event| event.operation)
                .collect::<Vec<_>>(),
            vec![
                fn64_runtime::SaveOperationKind::Write,
                fn64_runtime::SaveOperationKind::Read,
                fn64_runtime::SaveOperationKind::Write,
                fn64_runtime::SaveOperationKind::Read,
            ]
        );
        assert!(operations.iter().all(|event| {
            event.device == fn64_runtime::SaveType::Eeprom4k
                && event.offset == 7 * EEPROM_BLOCK_BYTES as u32
                && event.len == EEPROM_BLOCK_BYTES as u32
        }));
    }

    #[test]
    fn same_cycle_eeprom_maturity_pfs_and_eeprom_read_keep_wire_order() {
        install_eeprom(fn64_runtime::SaveType::Eeprom4k);
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerControllerPak);
        let deadline = crate::pi::with_pi_dma("same-cycle raw save ordering", |pi_dma| {
            pi_dma
                .start_eeprom_write(Cycles::ZERO, 1, [0x5a; EEPROM_BLOCK_BYTES])
                .unwrap()
        });
        crate::advance_virtual_time(deadline.get() - 1);

        let mut packet = [0u8; 64];
        let pak_address = 0u16;
        let encoded = pak_address | u16::from(accessory_address_crc(pak_address));
        packet[0] = 3;
        packet[1] = 33;
        packet[2] = 0x02;
        packet[3..5].copy_from_slice(&encoded.to_be_bytes());
        packet[38..41].fill(0);
        packet[41] = 2;
        packet[42] = 8;
        packet[43] = 0x04;
        packet[44] = 1;
        packet[53] = 0xfe;

        let mut rdram = vec![0u8; 64];
        write_logical_bytes(&mut rdram, 0, &packet);
        raw_si_round_trip(&mut rdram);

        assert_eq!(
            crate::copy_save_operations(),
            vec![
                fn64_runtime::SaveOperationEvent {
                    at: deadline,
                    device: fn64_runtime::SaveType::Eeprom4k,
                    operation: fn64_runtime::SaveOperationKind::Write,
                    offset: EEPROM_BLOCK_BYTES as u32,
                    len: EEPROM_BLOCK_BYTES as u32,
                },
                fn64_runtime::SaveOperationEvent {
                    at: deadline,
                    device: fn64_runtime::SaveType::ControllerPak,
                    operation: fn64_runtime::SaveOperationKind::Read,
                    offset: 0,
                    len: ACCESSORY_BLOCK_BYTES as u32,
                },
                fn64_runtime::SaveOperationEvent {
                    at: deadline,
                    device: fn64_runtime::SaveType::Eeprom4k,
                    operation: fn64_runtime::SaveOperationKind::Read,
                    offset: EEPROM_BLOCK_BYTES as u32,
                    len: EEPROM_BLOCK_BYTES as u32,
                },
            ]
        );
    }

    #[test]
    fn raw_eeprom_query_distinguishes_devices_and_reports_no_response() {
        fn query(kind: fn64_runtime::SaveType) -> ([u8; 3], u8) {
            install_eeprom(kind);
            let mut packet = [0u8; 64];
            packet[4] = 1;
            packet[5] = 3;
            packet[6] = 0;
            packet[10] = 0xFE;
            crate::pi::with_pi_dma("raw EEPROM query test", |pi_dma| {
                execute_controller_pif(Cycles::ZERO, &mut packet, pi_dma)
            });
            (packet[7..10].try_into().unwrap(), packet[5])
        }

        assert_eq!(
            query(fn64_runtime::SaveType::Eeprom4k),
            ([0x00, 0x80, 0x00], 3)
        );
        assert_eq!(
            query(fn64_runtime::SaveType::Eeprom16k),
            ([0x00, 0xC0, 0x00], 3)
        );
        assert_eq!(
            query(fn64_runtime::SaveType::SramBanked),
            ([0x00, 0x00, 0x00], 3 | PIF_CHANNEL_NO_RESPONSE)
        );
    }

    #[test]
    fn raw_eeprom_busy_status_rejects_overlap_and_clears_at_deadline() {
        install_eeprom(fn64_runtime::SaveType::Eeprom4k);
        let first = [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87];
        let second = [0xA5; EEPROM_BLOCK_BYTES];

        let write_packet = |payload: [u8; EEPROM_BLOCK_BYTES]| {
            let mut packet = [0u8; 64];
            packet[4] = 10;
            packet[5] = 1;
            packet[6] = 0x05;
            packet[7] = 0xC9;
            packet[8..16].copy_from_slice(&payload);
            packet[17] = 0xFE;
            packet
        };
        let query_packet = || {
            let mut packet = [0u8; 64];
            packet[4] = 1;
            packet[5] = 3;
            packet[6] = 0;
            packet[10] = 0xFE;
            packet
        };

        let start = Cycles::new(50);
        let deadline = start
            .checked_add(fn64_runtime::EEPROM_WRITE_CYCLES)
            .unwrap();
        let mut write = write_packet(first);
        crate::pi::with_pi_dma("raw EEPROM timed write", |pi_dma| {
            execute_controller_pif(start, &mut write, pi_dma)
        });
        assert_eq!(write[16], 0);

        let mut busy_query = query_packet();
        crate::pi::with_pi_dma("raw EEPROM busy query", |pi_dma| {
            execute_controller_pif(start, &mut busy_query, pi_dma)
        });
        assert_eq!(&busy_query[7..10], &[0x00, 0x80, 0x80]);

        let mut overlap = write_packet(second);
        crate::pi::with_pi_dma("raw EEPROM overlapping write", |pi_dma| {
            execute_controller_pif(Cycles::new(deadline.get() - 1), &mut overlap, pi_dma)
        });
        assert_eq!(overlap[16], 0x80);

        let mut ready_query = query_packet();
        crate::pi::with_pi_dma("raw EEPROM deadline query", |pi_dma| {
            execute_controller_pif(deadline, &mut ready_query, pi_dma);
            let mut stored = [0; EEPROM_BLOCK_BYTES];
            pi_dma.save_read_into(9 * EEPROM_BLOCK_BYTES, &mut stored);
            assert_eq!(stored, first);
        });
        assert_eq!(&ready_query[7..10], &[0x00, 0x80, 0x00]);
    }

    #[test]
    fn malformed_raw_eeprom_packet_traps_with_protocol_context() {
        install_eeprom(fn64_runtime::SaveType::Eeprom4k);
        let mut packet = [0u8; 64];
        packet[4] = 1;
        packet[5] = 8;
        packet[6] = 0x04;
        packet[15] = 0xFE;
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::pi::with_pi_dma("malformed raw EEPROM test", |pi_dma| {
                execute_controller_pif(Cycles::ZERO, &mut packet, pi_dma)
            });
        }))
        .expect_err("wrong EEPROM packet shape must trap");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("panic must carry protocol context");
        assert!(message.contains("command 0x04 on channel 4"), "{message}");
        assert!(message.contains("expected tx=2 rx=8"), "{message}");
    }

    #[test]
    fn public_accessory_crc_vectors_match_rumble_and_block_addresses() {
        assert_eq!(accessory_address_crc(0x0020), 0x15);
        assert_eq!(accessory_address_crc(ACCESSORY_ADDR_RUMBLE_PROBE), 0x01);
        assert_eq!(accessory_address_crc(ACCESSORY_ADDR_RUMBLE_MOTOR), 0x1B);
        assert_eq!(accessory_data_crc(&[0; ACCESSORY_BLOCK_BYTES]), 0x00);
        assert_eq!(accessory_data_crc(&[1; ACCESSORY_BLOCK_BYTES]), 0xEB);
        assert_eq!(accessory_data_crc(&[0x80; ACCESSORY_BLOCK_BYTES]), 0xB8);
    }

    #[test]
    fn raw_rumble_probe_and_write_share_the_high_level_motor_latch() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerRumblePak);
        let mut rdram = vec![0u8; 0x200];

        let mut init = ctx_zeroed();
        init.r5 = 0x8000_0100;
        init.r6 = 0;
        unsafe { osMotorInit_recomp(rdram.as_mut_ptr(), &mut init) };
        assert_eq!(init.r2, 0);
        assert!(!rumble_active(0));

        let mut raw_write = vec![0; 39];
        raw_write[0] = 35;
        raw_write[1] = 1;
        raw_write[2] = 0x03;
        raw_write[3..5].copy_from_slice(&0xC01Bu16.to_be_bytes());
        raw_write[5..37].fill(1);
        raw_write[38] = 0xFE;
        write_logical_bytes(&mut rdram, 0, &raw_write);
        raw_si_round_trip(&mut rdram);
        assert_eq!(read_logical_bytes(&rdram, 37, 1), vec![0xEB]);
        assert!(rumble_active(0));

        let mut stop = ctx_zeroed();
        stop.r4 = 0x8000_0100;
        unsafe { osMotorStop_recomp(rdram.as_mut_ptr(), &mut stop) };
        assert_eq!(stop.r2, 0);
        assert!(!rumble_active(0));

        let mut raw_probe = vec![0; 39];
        raw_probe[0] = 3;
        raw_probe[1] = 33;
        raw_probe[2] = 0x02;
        raw_probe[3..5].copy_from_slice(&0x8001u16.to_be_bytes());
        raw_probe[38] = 0xFE;
        write_logical_bytes(&mut rdram, 0, &raw_probe);
        raw_si_round_trip(&mut rdram);
        assert_eq!(read_logical_bytes(&rdram, 5, 32), vec![0x80; 32]);
        assert_eq!(read_logical_bytes(&rdram, 37, 1), vec![0xB8]);
        assert!(!rumble_active(0), "probe reads must not energize the motor");
        assert_eq!(
            crate::copy_controller_operations(),
            vec![
                fn64_runtime::ControllerOperationEvent {
                    at: Cycles::new(1),
                    port: 0,
                    device: fn64_runtime::ControllerOperationDevice::RumblePak,
                    operation: fn64_runtime::ControllerOperationKind::Control,
                },
                fn64_runtime::ControllerOperationEvent {
                    at: Cycles::new(2),
                    port: 0,
                    device: fn64_runtime::ControllerOperationDevice::RumblePak,
                    operation: fn64_runtime::ControllerOperationKind::Control,
                },
            ]
        );
    }

    #[test]
    fn raw_controller_pak_blocks_and_high_level_files_share_data_pages() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerControllerPak);
        let key = fn64_runtime::PfsKey {
            company_code: 0x1234,
            game_code: 0x4142_4344,
            game_name: [0x21; 16],
            ext_name: [0x11; 4],
        };
        with_executor(|executor| {
            assert_eq!(
                executor
                    .controller_pak_mut(0)
                    .expect("configured Controller Pak")
                    .allocate(key, fn64_runtime::pfs::PFS_PAGE_SIZE),
                Ok(0)
            );
        });
        let mut rdram = vec![0u8; 0x200];
        let raw_payload = [0x5A; ACCESSORY_BLOCK_BYTES];
        let first_data_address =
            (fn64_runtime::pfs::PFS_MANAGEMENT_PAGES * fn64_runtime::pfs::PFS_PAGE_SIZE) as u16;
        let encoded_address =
            first_data_address | u16::from(accessory_address_crc(first_data_address));
        let mut raw_write = vec![0; 39];
        raw_write[0] = 35;
        raw_write[1] = 1;
        raw_write[2] = 0x03;
        raw_write[3..5].copy_from_slice(&encoded_address.to_be_bytes());
        raw_write[5..37].copy_from_slice(&raw_payload);
        raw_write[38] = 0xFE;
        write_logical_bytes(&mut rdram, 0, &raw_write);
        raw_si_round_trip(&mut rdram);
        assert_eq!(
            read_logical_bytes(&rdram, 37, 1),
            vec![accessory_data_crc(&raw_payload)]
        );
        with_executor(|executor| {
            let mut semantic = [0; ACCESSORY_BLOCK_BYTES];
            executor
                .controller_pak(0)
                .expect("configured Controller Pak")
                .read(0, 0, &mut semantic)
                .unwrap();
            assert_eq!(semantic, raw_payload);
        });

        let semantic_payload = [0xA5; ACCESSORY_BLOCK_BYTES];
        with_executor(|executor| {
            executor
                .controller_pak_mut(0)
                .expect("configured Controller Pak")
                .write(0, ACCESSORY_BLOCK_BYTES, &semantic_payload)
                .unwrap();
        });
        let second_block = first_data_address + ACCESSORY_BLOCK_BYTES as u16;
        let encoded_second = second_block | u16::from(accessory_address_crc(second_block));
        let mut raw_read = vec![0; 39];
        raw_read[0] = 3;
        raw_read[1] = 33;
        raw_read[2] = 0x02;
        raw_read[3..5].copy_from_slice(&encoded_second.to_be_bytes());
        raw_read[38] = 0xFE;
        write_logical_bytes(&mut rdram, 0, &raw_read);
        raw_si_round_trip(&mut rdram);
        assert_eq!(read_logical_bytes(&rdram, 5, 32), semantic_payload);
        assert_eq!(
            read_logical_bytes(&rdram, 37, 1),
            vec![accessory_data_crc(&semantic_payload)]
        );
        assert_eq!(
            crate::copy_save_operations(),
            vec![
                fn64_runtime::SaveOperationEvent {
                    at: Cycles::new(1),
                    device: fn64_runtime::SaveType::ControllerPak,
                    operation: fn64_runtime::SaveOperationKind::Write,
                    offset: u32::from(first_data_address),
                    len: ACCESSORY_BLOCK_BYTES as u32,
                },
                fn64_runtime::SaveOperationEvent {
                    at: Cycles::new(3),
                    device: fn64_runtime::SaveType::ControllerPak,
                    operation: fn64_runtime::SaveOperationKind::Read,
                    offset: u32::from(second_block),
                    len: ACCESSORY_BLOCK_BYTES as u32,
                },
            ]
        );
    }

    #[test]
    fn raw_controller_pak_bank_select_reaches_high_level_cross_bank_data() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_pak_bank_count(0, fn64_runtime::ControllerPakBankCount::new(2).unwrap());
        let key = fn64_runtime::PfsKey {
            company_code: 0x1234,
            game_code: 0x4241_4e4b,
            game_name: [0x22; 16],
            ext_name: [0x12; 4],
        };
        let payload = [0x6c; ACCESSORY_BLOCK_BYTES];
        with_executor(|executor| {
            let pak = executor.controller_pak_mut(0).unwrap();
            let file = pak
                .allocate(key, 122 * fn64_runtime::pfs::PFS_PAGE_SIZE)
                .unwrap();
            pak.write(file, 121 * fn64_runtime::pfs::PFS_PAGE_SIZE, &payload)
                .unwrap();
        });

        let encoded_select = ACCESSORY_ADDR_RUMBLE_PROBE | u16::from(accessory_address_crc(0x8000));
        let mut select = [0u8; 64];
        select[0] = 35;
        select[1] = 1;
        select[2] = 0x03;
        select[3..5].copy_from_slice(&encoded_select.to_be_bytes());
        select[5..37].fill(1);
        select[38] = 0xfe;
        let select_operations =
            crate::pi::with_pi_dma("raw Controller Pak bank select", |pi_dma| {
                execute_controller_pif(Cycles::ZERO, &mut select, pi_dma)
            });
        assert!(select_operations.save_operations.is_empty());
        assert!(select_operations.controller_operations.is_empty());
        assert_eq!(select[37], accessory_data_crc(&[1; ACCESSORY_BLOCK_BYTES]));

        let address = fn64_runtime::pfs::PFS_PAGE_SIZE as u16;
        let encoded = address | u16::from(accessory_address_crc(address));
        let mut read = [0u8; 64];
        read[0] = 3;
        read[1] = 33;
        read[2] = 0x02;
        read[3..5].copy_from_slice(&encoded.to_be_bytes());
        read[38] = 0xfe;
        let read_operations = crate::pi::with_pi_dma("raw Controller Pak banked read", |pi_dma| {
            execute_controller_pif(Cycles::ZERO, &mut read, pi_dma)
        });
        assert_eq!(&read[5..37], &payload);
        assert_eq!(read[37], accessory_data_crc(&payload));
        with_executor(|executor| {
            assert_eq!(executor.controller_pak(0).unwrap().active_bank(), 1);
        });
        assert_eq!(
            read_operations.save_operations,
            vec![fn64_runtime::SaveOperationEvent {
                at: Cycles::ZERO,
                device: fn64_runtime::SaveType::ControllerPak,
                operation: fn64_runtime::SaveOperationKind::Read,
                offset: (fn64_runtime::pfs::PFS_BANK_CAPACITY + fn64_runtime::pfs::PFS_PAGE_SIZE)
                    as u32,
                len: ACCESSORY_BLOCK_BYTES as u32,
            }]
        );
        assert!(read_operations.controller_operations.is_empty());
    }

    #[test]
    fn raw_transfer_pak_reaches_banked_game_boy_rom_and_persistent_ram() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
        let mut gb_rom = vec![0xff; 64 * 0x4000];
        gb_rom[0x147] = 0x03; // MBC1 + RAM + battery
        gb_rom[0x149] = 0x03; // 32 KiB RAM
        for bank in 0..64 {
            gb_rom[bank * 0x4000] = bank as u8;
        }
        insert_transfer_pak_cartridge(0, gb_rom, None).unwrap();

        let write = |address: u16, value: u8| {
            let encoded = address | u16::from(accessory_address_crc(address));
            let mut packet = [0u8; 64];
            packet[0] = 35;
            packet[1] = 1;
            packet[2] = 0x03;
            packet[3..5].copy_from_slice(&encoded.to_be_bytes());
            packet[5..37].fill(value);
            packet[38] = 0xfe;
            crate::pi::with_pi_dma("raw Transfer Pak write", |pi_dma| {
                execute_controller_pif(Cycles::ZERO, &mut packet, pi_dma)
            });
            assert_eq!(packet[37], accessory_data_crc(&[value; 32]));
        };
        let read = |address: u16| {
            let encoded = address | u16::from(accessory_address_crc(address));
            let mut packet = [0u8; 64];
            packet[0] = 3;
            packet[1] = 33;
            packet[2] = 0x02;
            packet[3..5].copy_from_slice(&encoded.to_be_bytes());
            packet[38] = 0xfe;
            crate::pi::with_pi_dma("raw Transfer Pak read", |pi_dma| {
                execute_controller_pif(Cycles::ZERO, &mut packet, pi_dma)
            });
            let data: [u8; 32] = packet[5..37].try_into().unwrap();
            assert_eq!(packet[37], accessory_data_crc(&data));
            data
        };

        assert_eq!(read(0x8000), [0; 32]);
        write(0x8000, 0x84);
        assert_eq!(read(0x8000), [0x84; 32]);
        assert_eq!(read(0xb000), [0x84; 32]);

        // Transfer bank zero exposes GB 0x2000 at accessory 0xe000; select
        // MBC1 ROM bank two, then Transfer bank one exposes GB 0x4000.
        write(0xe000, 2);
        write(0xa000, 1);
        assert_eq!(read(0xc000)[0], 2);

        // Select MBC1 RAM banking mode and RAM bank two, then write GB
        // 0xa000 through Transfer bank two. The host-visible cartridge RAM
        // must observe the same byte, proving raw Joybus and persistence use
        // one backing store.
        write(0xa000, 0);
        write(0xc000, 0x0a);
        write(0xa000, 1);
        write(0xc000, 2);
        write(0xe000, 1);
        write(0xa000, 2);
        write(0xe000, 0x5a);
        assert_eq!(read(0xe000), [0x5a; 32]);
        with_executor(|executor| {
            assert_eq!(
                executor
                    .transfer_pak(0)
                    .expect("configured Transfer Pak")
                    .cartridge_ram()
                    .expect("MBC1 cartridge RAM")[2 * 0x2000],
                0x5a
            );
        });
    }

    #[test]
    fn raw_and_high_level_transfer_pak_paths_share_one_mbc3_guest_clock() {
        fn high_transfer(rdram: &mut [u8], write: bool, address: u16, buffer_offset: u32) {
            let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
            let mut ctx = ctx_zeroed();
            ctx.r4 = 0x8000_0200;
            ctx.r5 = u64::from(write);
            ctx.r6 = u64::from(address);
            ctx.r7 = u64::from(0x8000_0000 | buffer_offset);
            ctx.r29 = 0x8000_0080;
            unsafe { storage.write_u32(RdramAddr::from_offset(0x90), 32) };
            unsafe { crate::gbpak::osGbpakReadWrite_recomp(rdram.as_mut_ptr(), &mut ctx) };
            assert_eq!(ctx.r2, 0);
        }

        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
        let mut gb_rom = vec![0xff; 4 * 0x4000];
        gb_rom[0x147] = 0x10; // MBC3 + timer + RAM + battery
        gb_rom[0x149] = 0x03; // 32 KiB RAM
        insert_transfer_pak_cartridge(0, gb_rom, None).unwrap();

        let raw_write = |address: u16, value: u8, now: Cycles| {
            let encoded = address | u16::from(accessory_address_crc(address));
            let mut packet = [0u8; 64];
            packet[0] = 35;
            packet[1] = 1;
            packet[2] = 0x03;
            packet[3..5].copy_from_slice(&encoded.to_be_bytes());
            packet[5..37].fill(value);
            packet[38] = 0xfe;
            crate::pi::with_pi_dma("raw timed Transfer Pak write", |pi_dma| {
                execute_controller_pif(now, &mut packet, pi_dma)
            });
            assert_eq!(packet[37], accessory_data_crc(&[value; 32]));
        };
        let raw_read = |address: u16, now: Cycles| {
            let encoded = address | u16::from(accessory_address_crc(address));
            let mut packet = [0u8; 64];
            packet[0] = 3;
            packet[1] = 33;
            packet[2] = 0x02;
            packet[3..5].copy_from_slice(&encoded.to_be_bytes());
            packet[38] = 0xfe;
            crate::pi::with_pi_dma("raw timed Transfer Pak read", |pi_dma| {
                execute_controller_pif(now, &mut packet, pi_dma)
            });
            let data: [u8; 32] = packet[5..37].try_into().unwrap();
            assert_eq!(packet[37], accessory_data_crc(&data));
            data
        };

        // Raw Joybus powers the Pak, enables MBC3 RAM/RTC, halts the timer,
        // initializes seconds to zero, then resumes it at guest cycle zero.
        raw_write(0x8000, 0x84, Cycles::ZERO);
        raw_write(0xa000, 0, Cycles::ZERO);
        raw_write(0xc000, 0x0a, Cycles::ZERO);
        raw_write(0xa000, 1, Cycles::ZERO);
        raw_write(0xc000, 0x0c, Cycles::ZERO);
        raw_write(0xa000, 2, Cycles::ZERO);
        raw_write(0xe000, 0x40, Cycles::ZERO);
        raw_write(0xa000, 1, Cycles::ZERO);
        raw_write(0xc000, 0x08, Cycles::ZERO);
        raw_write(0xa000, 2, Cycles::ZERO);
        raw_write(0xe000, 0, Cycles::ZERO);
        raw_write(0xa000, 1, Cycles::ZERO);
        raw_write(0xc000, 0x0c, Cycles::ZERO);
        raw_write(0xa000, 2, Cycles::ZERO);
        raw_write(0xe000, 0, Cycles::ZERO);

        let mut rdram = vec![0; 0x800];
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram.as_mut_ptr()) };
        unsafe {
            storage.write_u32(RdramAddr::from_offset(0x200), 0x10);
            storage.write_u32(RdramAddr::from_offset(0x208), 0);
            for offset in 0..32 {
                storage.write_u8(RdramAddr::from_offset(0x300 + offset), 0);
                storage.write_u8(RdramAddr::from_offset(0x340 + offset), 1);
                storage.write_u8(RdramAddr::from_offset(0x380 + offset), 0x08);
            }
        }

        crate::advance_virtual_time(fn64_runtime::CPU_CLOCK_HZ - 1);
        high_transfer(&mut rdram, true, 0x6000, 0x300);
        high_transfer(&mut rdram, true, 0x6000, 0x340);
        high_transfer(&mut rdram, true, 0x4000, 0x380);
        high_transfer(&mut rdram, false, 0xa000, 0x400);
        assert_eq!(unsafe { storage.read_u8(RdramAddr::from_offset(0x400)) }, 0);

        crate::advance_virtual_time(fn64_runtime::CPU_CLOCK_HZ);
        high_transfer(&mut rdram, false, 0xa000, 0x440);
        assert_eq!(
            unsafe { storage.read_u8(RdramAddr::from_offset(0x440)) },
            0,
            "high-level read must retain the prior RTC latch"
        );
        high_transfer(&mut rdram, true, 0x6000, 0x300);
        high_transfer(&mut rdram, true, 0x6000, 0x340);
        high_transfer(&mut rdram, false, 0xa000, 0x480);
        assert_eq!(unsafe { storage.read_u8(RdramAddr::from_offset(0x480)) }, 1);
        assert_eq!(
            raw_read(0xe000, Cycles::new(fn64_runtime::CPU_CLOCK_HZ)),
            [1; 32]
        );
    }

    #[test]
    fn host_battery_forwarding_materializes_before_guest_access() {
        let mut gb_rom = vec![0xff; 4 * 0x4000];
        gb_rom[0x147] = 0x10;
        gb_rom[0x149] = 0x03;
        let mut source = fn64_runtime::TransferPak::new();
        source.insert_cartridge(gb_rom.clone(), None).unwrap();
        let metadata = source
            .checkpoint_mbc3_battery(
                Cycles::new(fn64_runtime::CPU_CLOCK_HZ / 2),
                fn64_runtime::HostUnixNanos::new(1_000_000_000),
            )
            .unwrap()
            .unwrap();

        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
        insert_transfer_pak_cartridge_with_battery(
            0,
            gb_rom,
            None,
            Some(fn64_runtime::Mbc3BatteryRestore::new(
                metadata,
                fn64_runtime::HostUnixNanos::new(2_500_000_000),
            )),
        )
        .unwrap();
        let checkpoint =
            checkpoint_transfer_pak_battery(0, fn64_runtime::HostUnixNanos::new(3_000_000_000))
                .unwrap()
                .unwrap();
        assert_eq!(checkpoint.rtc()[0], 2);
        assert_eq!(checkpoint.subsecond_cycles(), 0);
    }

    #[test]
    fn transfer_pak_removal_changes_status_and_data_access_traps_by_name() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerTransferPak);
        let mut gb_rom = vec![0xff; 2 * 0x4000];
        gb_rom[0x147] = 0x00;
        gb_rom[0x149] = 0;
        insert_transfer_pak_cartridge(0, gb_rom, None).unwrap();
        with_executor(|executor| {
            let pak = executor.transfer_pak_mut(0).unwrap();
            pak.write_block(0x8000, &[0x84; 32]);
            assert!(pak.remove_cartridge().is_some());
            let mut status = [0xff; 32];
            pak.read_block(0xb000, &mut status);
            assert_eq!(status, [0xc0; 32]);
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pak.read_block(0xc000, &mut status)
            }))
            .expect_err("powered Transfer Pak without a cartridge must not fabricate data");
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .expect("panic must carry Transfer Pak context");
            assert!(message.contains("no Game Boy cartridge"), "{message}");
        });
    }

    #[test]
    fn malformed_raw_accessory_address_crc_traps_with_address_context() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        load_rom(vec![0; 0x100]);
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerRumblePak);
        let mut packet = [0u8; 64];
        packet[0] = 3;
        packet[1] = 33;
        packet[2] = 0x02;
        packet[3..5].copy_from_slice(&0xC000u16.to_be_bytes());
        packet[38] = 0xFE;
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::pi::with_pi_dma("malformed raw accessory test", |pi_dma| {
                execute_controller_pif(Cycles::ZERO, &mut packet, pi_dma)
            });
        }))
        .expect_err("wrong accessory address CRC must trap");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("panic must carry protocol context");
        assert!(message.contains("command 0x02 on channel 0"), "{message}");
        assert!(message.contains("for 0xc000; expected 0x1b"), "{message}");
    }

    /// osContInit: (1) OSContStatus entries must be written SWIZZLED (^3) like
    /// osContGetQuery, and (2) ctlBitfield is a `u8*` -- a SINGLE swizzled
    /// byte, no +1 store. Fails against the bug (flat status stores + two
    /// bitfield bytes at flat +0/+1).
    #[test]
    fn os_cont_init_swizzles_status_and_writes_single_bitfield_byte() {
        // data at offset 0x40 (16 bytes = 4 OSContStatus), bitfield at 0x80.
        let mut rdram = vec![0xEEu8; 256]; // 0xEE sentinel: catch stray writes.
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0; // mq (unused for the byte layout under test)
        ctx.r5 = 0x8000_0080; // ctlBitfield
        ctx.r6 = 0x8000_0040; // data (OSContStatus[4])
        unsafe { osContInit_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // Port 0 is a standard controller (type 0x0005). The swizzled entry
        // [type_hi=0x00, type_lo=0x05, status=0x00, pad=0x00] lands at
        // (0x40+o)^3, so logical byte 1 (0x05) is at host 0x40+ (1^3)=0x40+2.
        let logical = |base: usize, o: usize| rdram[(base + o) ^ 3];
        assert_eq!(logical(0x40, 0), 0x00, "port0 type_hi");
        assert_eq!(logical(0x40, 1), 0x05, "port0 type_lo (CONT_TYPE_STANDARD)");
        assert_eq!(logical(0x40, 2), 0x00, "port0 status");
        assert_eq!(logical(0x40, 3), 0x00, "port0 pad");
        // Port 1 absent -> [0,0,0,CONT_NO_RESPONSE_ERROR] swizzled.
        assert_eq!(logical(0x44, 3), CONT_NO_RESPONSE_ERROR, "port1 errno");

        // ctlBitfield: a SINGLE swizzled byte = mask (0x01, only port 0). The
        // flat address 0x80 must stay the 0xEE sentinel (the buggy flat store
        // would overwrite it), and 0x81 must stay 0xEE (the buggy +1 store
        // would clobber this adjacent byte).
        assert_eq!(
            rdram[0x80 ^ 3],
            0x01,
            "bitfield: single swizzled byte, port0 set"
        );
        assert_eq!(
            rdram[0x80], 0xEE,
            "flat bitfield addr untouched (no flat store)"
        );
        assert_eq!(rdram[0x81], 0xEE, "adjacent byte untouched (no +1 store)");
    }

    #[test]
    fn motor_init_start_and_stop_share_the_configured_accessory_state() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerRumblePak);

        let mut rdram = vec![0u8; 0x100];
        let pfs_vram = 0x8000_0040u64;
        let queue_vram = 0x8000_0080u64;
        let mut init = ctx_with(queue_vram, pfs_vram, 0);
        unsafe { osMotorInit_recomp(rdram.as_mut_ptr(), &mut init) };
        assert_eq!(init.r2, 0);
        assert_eq!(u32::from_ne_bytes(rdram[0x40..0x44].try_into().unwrap()), 8);
        assert_eq!(
            u32::from_ne_bytes(rdram[0x44..0x48].try_into().unwrap()),
            queue_vram as u32
        );
        assert_eq!(u32::from_ne_bytes(rdram[0x48..0x4c].try_into().unwrap()), 0);
        assert_eq!(
            with_executor(|exec| exec.pif().query_response(0)),
            [0x05, 0x00, fn64_runtime::CONT_CARD_ON]
        );

        let mut access = ctx_zeroed();
        access.r4 = pfs_vram;
        unsafe { osMotorStart_recomp(rdram.as_mut_ptr(), &mut access) };
        assert_eq!(access.r2, 0);
        assert!(rumble_active(0));

        unsafe { osMotorStop_recomp(rdram.as_mut_ptr(), &mut access) };
        assert_eq!(access.r2, 0);
        assert!(!rumble_active(0));
    }

    #[test]
    fn motor_init_returns_documented_no_pak_and_wrong_device_errors() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        let mut rdram = vec![0u8; 0x100];
        let mut ctx = ctx_with(0x8000_0080, 0x8000_0040, 0);

        unsafe { osMotorInit_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, PFS_ERR_NOPACK as u64);

        set_controller_port_state(0, fn64_runtime::PortState::StandardControllerControllerPak);
        unsafe { osMotorInit_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, PFS_ERR_DEVICE as u64);
    }

    #[test]
    fn controller_dma_completion_raises_the_shared_mi_si_source() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        crate::pi::set_mi_interrupt_mask(fn64_runtime::InterruptSource::Si.bit());
        let queue = RdramAddr::from_offset(0x40);
        with_executor(|exec| exec.create_mesg_queue(queue, 1));
        let mut ctx = ctx_zeroed();
        ctx.r4 = queue.to_kseg0() as u64;
        unsafe { osContStartQuery_recomp(std::ptr::null_mut(), &mut ctx) };

        let before = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
        assert_eq!(before & fn64_runtime::InterruptSource::Si.bit(), 0);
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, queue, false)),
            fn64_runtime::RecvMesgOutcome::WouldBlock
        );
        crate::advance_virtual_time(1);

        let pending = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
        assert_ne!(pending & fn64_runtime::InterruptSource::Si.bit(), 0);
        assert!(crate::pi::cpu_interrupt_pending());
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, queue, false)),
            fn64_runtime::RecvMesgOutcome::Delivered(0)
        );
    }
}
