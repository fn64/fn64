//! Typed N64 Transfer Pak and Game Boy cartridge-bus model.
//!
//! Provenance: the public libultra SI-device manual documents Transfer Pak
//! power, status, cartridge read/write, removal, and error behavior. Public
//! Joybus hardware documentation describes the accessory register windows:
//! power/probe at `0x8000`, Transfer Pak bank at `0xa000`, mode/status at
//! `0xb000`, and a 16 KiB Game Boy bus window at `0xc000..=0xffff`. Game Boy
//! cartridge header and MBC register behavior follows public Pan Docs
//! (`https://gbdev.io/pandocs/MBC3.html` for RTC latch/register/halt/day-carry
//! semantics). Pan Docs also establishes that timer-bearing MBC3 cartridges
//! use an external oscillator and battery so the clock can continue while the
//! Game Boy is off. The versioned sidecar and caller-injected Unix timestamp
//! below are fn64 host policy, not a hardware file format. No GPL runtime
//! implementation was consulted.

use crate::device::Cycles;
use crate::tv::CPU_CLOCK_HZ;
use sha2::{Digest, Sha256};

pub const TRANSFER_PAK_BLOCK_SIZE: usize = 32;
const GB_ROM_BANK_SIZE: usize = 0x4000;
const GB_RAM_BANK_SIZE: usize = 0x2000;
const MBC3_BATTERY_MAGIC: [u8; 8] = *b"FN64M3RT";
const MBC3_BATTERY_VERSION: u16 = 1;
const MBC3_BATTERY_CHECKSUM_OFFSET: usize = 72;
pub const MBC3_BATTERY_METADATA_LEN: usize = MBC3_BATTERY_CHECKSUM_OFFSET + 32;

/// Caller-sampled host time used only at the battery-persistence boundary.
///
/// The runtime never reads `SystemTime`. A host explicitly supplies this
/// value while importing or exporting an MBC3 sidecar; import immediately
/// materializes elapsed time into the modeled RTC and discards the timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct HostUnixNanos(u64);

impl HostUnixNanos {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Canonical battery-backed MBC3 RTC sidecar payload.
///
/// Only the live RTC and its subsecond phase are hardware-state payload. The
/// ROM identity prevents attaching a valid sidecar to a different cartridge;
/// the checkpoint is host metadata used to materialize time spent powered
/// off. Mapper banks, enable/latch state, the latched RTC copy, and Transfer
/// Pak state are intentionally absent because they are powered-session state.
///
/// V1 is 104 bytes, all integers big-endian: magic `[0..8)`, version
/// `[8..10)`, total length `[10..12)`, ROM SHA-256 `[12..44)`, cartridge type
/// `[44]`, RTC registers `[45..50)`, six zero reserved bytes `[50..56)`,
/// subsecond guest cycles `[56..64)`, checkpoint Unix nanoseconds `[64..72)`,
/// and SHA-256 of the preceding 72 bytes `[72..104)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mbc3BatteryMetadata {
    rom_sha256: [u8; 32],
    cartridge_type: u8,
    rtc: [u8; 5],
    subsecond_cycles: u64,
    checkpoint: HostUnixNanos,
}

impl Mbc3BatteryMetadata {
    pub fn decode(bytes: &[u8]) -> Result<Self, Mbc3BatteryMetadataError> {
        if bytes.len() != MBC3_BATTERY_METADATA_LEN {
            return Err(Mbc3BatteryMetadataError::InvalidLength {
                expected: MBC3_BATTERY_METADATA_LEN,
                actual: bytes.len(),
            });
        }
        if bytes[..8] != MBC3_BATTERY_MAGIC {
            return Err(Mbc3BatteryMetadataError::InvalidMagic);
        }
        let version = u16::from_be_bytes([bytes[8], bytes[9]]);
        if version != MBC3_BATTERY_VERSION {
            crate::record_unsupported_event(
                crate::UnsupportedSubsystem::Runtime,
                "runtime.transfer-pak.battery-metadata-version",
                format!(
                    "MBC3 battery metadata version {version} is unsupported; expected {MBC3_BATTERY_VERSION}"
                ),
                None,
                crate::UnsupportedDisposition::ReturnedError,
            );
            return Err(Mbc3BatteryMetadataError::UnsupportedVersion(version));
        }
        let declared_len = usize::from(u16::from_be_bytes([bytes[10], bytes[11]]));
        if declared_len != MBC3_BATTERY_METADATA_LEN {
            return Err(Mbc3BatteryMetadataError::InvalidDeclaredLength {
                expected: MBC3_BATTERY_METADATA_LEN,
                actual: declared_len,
            });
        }
        let expected_checksum = Sha256::digest(&bytes[..MBC3_BATTERY_CHECKSUM_OFFSET]);
        if bytes[MBC3_BATTERY_CHECKSUM_OFFSET..] != expected_checksum[..] {
            return Err(Mbc3BatteryMetadataError::ChecksumMismatch);
        }
        if bytes[50..56].iter().any(|byte| *byte != 0) {
            return Err(Mbc3BatteryMetadataError::NonzeroReservedBytes);
        }

        let mut rom_sha256 = [0; 32];
        rom_sha256.copy_from_slice(&bytes[12..44]);
        let cartridge_type = bytes[44];
        if !is_timer_battery_mbc3(cartridge_type) {
            return Err(Mbc3BatteryMetadataError::NotTimerBatteryMbc3(
                cartridge_type,
            ));
        }
        let mut rtc = [0; 5];
        rtc.copy_from_slice(&bytes[45..50]);
        validate_rtc(rtc)?;
        let subsecond_cycles = u64::from_be_bytes(bytes[56..64].try_into().unwrap());
        if subsecond_cycles >= CPU_CLOCK_HZ {
            return Err(Mbc3BatteryMetadataError::InvalidSubsecondCycles(
                subsecond_cycles,
            ));
        }
        let checkpoint = HostUnixNanos::new(u64::from_be_bytes(bytes[64..72].try_into().unwrap()));
        Ok(Self {
            rom_sha256,
            cartridge_type,
            rtc,
            subsecond_cycles,
            checkpoint,
        })
    }

    pub fn encode(&self) -> [u8; MBC3_BATTERY_METADATA_LEN] {
        let mut bytes = [0; MBC3_BATTERY_METADATA_LEN];
        bytes[..8].copy_from_slice(&MBC3_BATTERY_MAGIC);
        bytes[8..10].copy_from_slice(&MBC3_BATTERY_VERSION.to_be_bytes());
        bytes[10..12].copy_from_slice(&(MBC3_BATTERY_METADATA_LEN as u16).to_be_bytes());
        bytes[12..44].copy_from_slice(&self.rom_sha256);
        bytes[44] = self.cartridge_type;
        bytes[45..50].copy_from_slice(&self.rtc);
        bytes[56..64].copy_from_slice(&self.subsecond_cycles.to_be_bytes());
        bytes[64..72].copy_from_slice(&self.checkpoint.get().to_be_bytes());
        let checksum = Sha256::digest(&bytes[..MBC3_BATTERY_CHECKSUM_OFFSET]);
        bytes[MBC3_BATTERY_CHECKSUM_OFFSET..].copy_from_slice(&checksum);
        bytes
    }

    pub const fn checkpoint(&self) -> HostUnixNanos {
        self.checkpoint
    }

    pub const fn rtc(&self) -> [u8; 5] {
        self.rtc
    }

    pub const fn subsecond_cycles(&self) -> u64 {
        self.subsecond_cycles
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mbc3BatteryRestore {
    pub metadata: Mbc3BatteryMetadata,
    pub resumed_at: HostUnixNanos,
}

impl Mbc3BatteryRestore {
    pub const fn new(metadata: Mbc3BatteryMetadata, resumed_at: HostUnixNanos) -> Self {
        Self {
            metadata,
            resumed_at,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mbc3BatteryMetadataError {
    InvalidLength { expected: usize, actual: usize },
    InvalidMagic,
    UnsupportedVersion(u16),
    InvalidDeclaredLength { expected: usize, actual: usize },
    ChecksumMismatch,
    NonzeroReservedBytes,
    NotTimerBatteryMbc3(u8),
    InvalidRtcRegister { register: u8, value: u8 },
    InvalidSubsecondCycles(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferPakError {
    RomTooSmall,
    UnsupportedCartridgeType(u8),
    RamSize {
        expected: usize,
        actual: usize,
    },
    BatteryMetadataForNonTimerCartridge(u8),
    BatteryMetadataCartridgeType {
        expected: u8,
        actual: u8,
    },
    BatteryMetadataRomMismatch,
    BatteryClockMovedBackward {
        checkpoint: HostUnixNanos,
        resumed_at: HostUnixNanos,
    },
}

#[derive(Clone, Debug, Default)]
pub struct TransferPak {
    now: Cycles,
    enabled: bool,
    transfer_bank: u8,
    access_mode: u8,
    cartridge: Option<GameBoyCartridge>,
    cartridge_pulled: bool,
    reset_detected: bool,
}

/// Complete retained Transfer Pak state. Cartridge bytes are included because
/// an inserted Game Boy ROM is an independent peripheral input and is not
/// implied by the N64 cartridge hash in a release report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferPakEvidenceSnapshot {
    pub now: Cycles,
    pub enabled: bool,
    pub transfer_bank: u8,
    pub access_mode: u8,
    pub cartridge: Option<GameBoyCartridgeEvidenceSnapshot>,
    pub cartridge_pulled: bool,
    pub reset_detected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameBoyCartridgeEvidenceSnapshot {
    pub rom: Vec<u8>,
    pub ram: Vec<u8>,
    pub mapper: GameBoyMapperEvidenceSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameBoyMapperEvidenceSnapshot {
    RomOnly,
    Mbc1 {
        ram_enabled: bool,
        rom_low5: u8,
        upper2: u8,
        ram_mode: bool,
    },
    Mbc2 {
        ram_enabled: bool,
        rom_bank: u8,
    },
    Mbc3 {
        timer_present: bool,
        ram_enabled: bool,
        rom_bank: u8,
        select: u8,
        latch_armed: bool,
        rtc: [u8; 5],
        latched_rtc: [u8; 5],
        subsecond_cycles: u64,
    },
    Mbc5 {
        ram_enabled: bool,
        rom_bank: u16,
        ram_bank: u8,
        rumble_variant: bool,
    },
}

/// Software-visible Transfer Pak state returned by libultra's status API.
/// The raw accessory register uses a different bit layout, so callers cannot
/// accidentally confuse that wire byte with this typed snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferPakStatus {
    pub cartridge_present: bool,
    pub cartridge_pulled: bool,
    pub powered: bool,
    pub reset_detected: bool,
}

impl TransferPak {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn evidence_snapshot(&self) -> TransferPakEvidenceSnapshot {
        TransferPakEvidenceSnapshot {
            now: self.now,
            enabled: self.enabled,
            transfer_bank: self.transfer_bank,
            access_mode: self.access_mode,
            cartridge: self
                .cartridge
                .as_ref()
                .map(GameBoyCartridge::evidence_snapshot),
            cartridge_pulled: self.cartridge_pulled,
            reset_detected: self.reset_detected,
        }
    }

    pub fn insert_cartridge(
        &mut self,
        rom: Vec<u8>,
        ram: Option<Vec<u8>>,
    ) -> Result<(), TransferPakError> {
        self.insert_cartridge_with_battery(rom, ram, None)
    }

    /// Insert a cartridge, optionally restoring its battery-backed RTC.
    ///
    /// `restore` is the only wall-time ingress. Its elapsed duration is
    /// materialized before the cartridge becomes visible, after which the
    /// ordinary guest-cycle clock is authoritative again.
    pub fn insert_cartridge_with_battery(
        &mut self,
        rom: Vec<u8>,
        ram: Option<Vec<u8>>,
        restore: Option<Mbc3BatteryRestore>,
    ) -> Result<(), TransferPakError> {
        let mut cartridge = GameBoyCartridge::new(rom, ram)?;
        if let Some(restore) = restore {
            cartridge.restore_mbc3_battery(restore)?;
        }
        if self.cartridge.is_some() {
            self.cartridge_pulled = true;
        }
        self.cartridge = Some(cartridge);
        Ok(())
    }

    /// Advance the battery-backed cartridge clock on the runtime's one guest
    /// timebase. Transfer Pak power does not gate MBC3's external oscillator;
    /// the cartridge timer therefore advances whenever it is inserted unless
    /// its own RTC halt bit is set.
    pub fn advance_to(&mut self, now: Cycles) {
        let elapsed = now.get().checked_sub(self.now.get()).unwrap_or_else(|| {
            panic!(
                "Transfer Pak clock cannot move backward from {} to {} guest cycles",
                self.now.get(),
                now.get()
            )
        });
        self.now = now;
        if let Some(cartridge) = self.cartridge.as_mut() {
            cartridge.advance_cycles(elapsed);
        }
    }

    pub const fn device_time(&self) -> Cycles {
        self.now
    }

    pub fn remove_cartridge(&mut self) -> Option<(Vec<u8>, Vec<u8>)> {
        let removed = self
            .cartridge
            .take()
            .map(|cartridge| (cartridge.rom, cartridge.ram));
        if removed.is_some() {
            self.cartridge_pulled = true;
        }
        removed
    }

    pub fn cartridge_ram(&self) -> Option<&[u8]> {
        self.cartridge
            .as_ref()
            .map(|cartridge| cartridge.ram.as_slice())
    }

    pub fn cartridge_type(&self) -> Option<u8> {
        self.cartridge
            .as_ref()
            .map(|cartridge| cartridge.rom[0x147])
    }

    pub fn cartridge_ram_len(&self) -> Option<usize> {
        self.cartridge.as_ref().map(|cartridge| cartridge.ram.len())
    }

    /// Freeze the battery-backed RTC at the supplied guest and host edges.
    ///
    /// The guest time is applied first. The caller-supplied host timestamp is
    /// serialized for the next process but is never retained by `TransferPak`
    /// and therefore cannot perturb fixed-cycle evidence in this process.
    pub fn checkpoint_mbc3_battery(
        &mut self,
        now: Cycles,
        checkpoint: HostUnixNanos,
    ) -> Result<Option<Mbc3BatteryMetadata>, TransferPakError> {
        self.advance_to(now);
        match self.cartridge.as_ref() {
            Some(cartridge) => cartridge.mbc3_battery_metadata(checkpoint),
            None => Ok(None),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn has_cartridge(&self) -> bool {
        self.cartridge.is_some()
    }

    /// Observe and acknowledge the sticky cartridge-removal/reset signals.
    /// Hardware keeps removal sticky across an N64 reset; libultra clears
    /// both signals only after observing a currently inserted cartridge.
    pub fn observe_status(&mut self) -> TransferPakStatus {
        let status = TransferPakStatus {
            cartridge_present: self.cartridge.is_some(),
            cartridge_pulled: self.cartridge_pulled,
            powered: self.enabled,
            reset_detected: self.reset_detected,
        };
        if status.cartridge_present {
            self.cartridge_pulled = false;
            self.reset_detected = false;
        }
        status
    }

    pub fn set_power(&mut self, enabled: bool) {
        if enabled {
            if !self.enabled {
                self.reset_detected = true;
            }
            self.enabled = true;
        } else {
            self.enabled = false;
            self.transfer_bank = 0;
            self.access_mode = 0;
        }
    }

    pub fn transfer_bank(&self) -> u8 {
        self.transfer_bank
    }

    pub fn access_mode(&self) -> u8 {
        self.access_mode
    }

    /// Read one aligned Game Boy bus block through the same bank register and
    /// 16 KiB window used by raw Joybus traffic.
    pub fn read_game_boy_block(&mut self, address: u16, data: &mut [u8; TRANSFER_PAK_BLOCK_SIZE]) {
        assert!(
            address <= 0xbfe0 && address.is_multiple_of(TRANSFER_PAK_BLOCK_SIZE as u16),
            "Transfer Pak Game Boy read uses invalid block address {address:#06x}"
        );
        let bank = (address / 0x4000) as u8;
        self.write_block(0xa000, &[bank; TRANSFER_PAK_BLOCK_SIZE]);
        self.read_block(0xc000 + address % 0x4000, data);
    }

    /// Write one aligned Game Boy bus block through the same raw register
    /// path as an accessory command.
    pub fn write_game_boy_block(&mut self, address: u16, data: &[u8; TRANSFER_PAK_BLOCK_SIZE]) {
        assert!(
            address <= 0xbfe0 && address.is_multiple_of(TRANSFER_PAK_BLOCK_SIZE as u16),
            "Transfer Pak Game Boy write uses invalid block address {address:#06x}"
        );
        let bank = (address / 0x4000) as u8;
        self.write_block(0xa000, &[bank; TRANSFER_PAK_BLOCK_SIZE]);
        self.write_block(0xc000 + address % 0x4000, data);
    }

    pub fn read_block(&mut self, address: u16, data: &mut [u8; TRANSFER_PAK_BLOCK_SIZE]) {
        assert!(
            address.is_multiple_of(TRANSFER_PAK_BLOCK_SIZE as u16),
            "Transfer Pak read address {address:#06x} is not 32-byte aligned"
        );
        match address {
            0x8000..=0x9fe0 => data.fill(if self.enabled { 0x84 } else { 0x00 }),
            0xa000..=0xafe0 => data.fill(self.transfer_bank),
            0xb000..=0xbfe0 => {
                let status = if self.cartridge.is_none() {
                    (u8::from(self.enabled) * 0x80) | (u8::from(self.cartridge_pulled) * 0x40)
                } else if !self.enabled {
                    u8::from(self.cartridge_pulled) * 0x40
                } else if self.access_mode == 0 {
                    0x84 | (u8::from(self.cartridge_pulled) * 0x40)
                } else {
                    0x89 | (u8::from(self.cartridge_pulled) * 0x40)
                };
                data.fill(status);
            }
            0xc000..=0xffe0 => {
                if !self.enabled {
                    data.fill(0);
                    return;
                }
                let cartridge = self.cartridge.as_mut().unwrap_or_else(|| {
                    panic!("Transfer Pak data read at {address:#06x} has no Game Boy cartridge")
                });
                let base = u16::from(self.transfer_bank) * 0x4000 + (address - 0xc000);
                for (offset, byte) in data.iter_mut().enumerate() {
                    *byte = cartridge.read(base + offset as u16);
                }
            }
            _ => {
                let context =
                    format!("Transfer Pak read uses unsupported accessory address {address:#06x}");
                crate::record_unsupported_event(
                    crate::UnsupportedSubsystem::Runtime,
                    "runtime.transfer-pak.read-address",
                    &context,
                    None,
                    crate::UnsupportedDisposition::LoudTrap,
                );
                panic!("{context}");
            }
        }
    }

    pub fn write_block(&mut self, address: u16, data: &[u8; TRANSFER_PAK_BLOCK_SIZE]) {
        assert!(
            address.is_multiple_of(TRANSFER_PAK_BLOCK_SIZE as u16),
            "Transfer Pak write address {address:#06x} is not 32-byte aligned"
        );
        match address {
            0x8000..=0x9fe0 => match data[0] {
                0x84 => self.set_power(true),
                0x00 | 0xfe => {
                    self.set_power(false);
                }
                value => {
                    let context = format!(
                        "Transfer Pak power/probe write at {address:#06x} uses unsupported value {value:#04x}"
                    );
                    crate::record_unsupported_event(
                        crate::UnsupportedSubsystem::Runtime,
                        "runtime.transfer-pak.power-value",
                        &context,
                        None,
                        crate::UnsupportedDisposition::LoudTrap,
                    );
                    panic!("{context}");
                }
            },
            0xa000..=0xafe0 => {
                assert!(
                    data[0] <= 3,
                    "Transfer Pak bank write at {address:#06x} selects invalid bank {}",
                    data[0]
                );
                self.transfer_bank = data[0];
            }
            0xb000..=0xbfe0 => {
                assert!(
                    data[0] <= 1,
                    "Transfer Pak mode write at {address:#06x} selects invalid mode {}",
                    data[0]
                );
                self.access_mode = data[0];
            }
            0xc000..=0xffe0 => {
                assert!(
                    self.enabled,
                    "Transfer Pak data write at {address:#06x} while power is disabled"
                );
                let cartridge = self.cartridge.as_mut().unwrap_or_else(|| {
                    panic!("Transfer Pak data write at {address:#06x} has no Game Boy cartridge")
                });
                let base = u16::from(self.transfer_bank) * 0x4000 + (address - 0xc000);
                for (offset, byte) in data.iter().copied().enumerate() {
                    cartridge.write(base + offset as u16, byte);
                }
            }
            _ => {
                let context =
                    format!("Transfer Pak write uses unsupported accessory address {address:#06x}");
                crate::record_unsupported_event(
                    crate::UnsupportedSubsystem::Runtime,
                    "runtime.transfer-pak.write-address",
                    &context,
                    None,
                    crate::UnsupportedDisposition::LoudTrap,
                );
                panic!("{context}");
            }
        }
    }
}

#[derive(Clone, Debug)]
struct GameBoyCartridge {
    rom: Vec<u8>,
    ram: Vec<u8>,
    mapper: Mapper,
}

#[derive(Clone, Debug)]
enum Mapper {
    RomOnly,
    Mbc1 {
        ram_enabled: bool,
        rom_low5: u8,
        upper2: u8,
        ram_mode: bool,
    },
    Mbc2 {
        ram_enabled: bool,
        rom_bank: u8,
    },
    Mbc3 {
        timer_present: bool,
        ram_enabled: bool,
        rom_bank: u8,
        select: u8,
        latch_armed: bool,
        rtc: [u8; 5],
        latched_rtc: [u8; 5],
        subsecond_cycles: u64,
    },
    Mbc5 {
        ram_enabled: bool,
        rom_bank: u16,
        ram_bank: u8,
        rumble_variant: bool,
    },
}

impl GameBoyCartridge {
    fn restore_mbc3_battery(
        &mut self,
        restore: Mbc3BatteryRestore,
    ) -> Result<(), TransferPakError> {
        let cartridge_type = self.rom[0x147];
        let Mapper::Mbc3 {
            timer_present: true,
            rtc,
            subsecond_cycles,
            ..
        } = &mut self.mapper
        else {
            return Err(TransferPakError::BatteryMetadataForNonTimerCartridge(
                cartridge_type,
            ));
        };
        if restore.metadata.cartridge_type != cartridge_type {
            return Err(TransferPakError::BatteryMetadataCartridgeType {
                expected: cartridge_type,
                actual: restore.metadata.cartridge_type,
            });
        }
        if restore.metadata.rom_sha256 != rom_sha256(&self.rom) {
            return Err(TransferPakError::BatteryMetadataRomMismatch);
        }
        if restore.resumed_at < restore.metadata.checkpoint {
            return Err(TransferPakError::BatteryClockMovedBackward {
                checkpoint: restore.metadata.checkpoint,
                resumed_at: restore.resumed_at,
            });
        }

        *rtc = restore.metadata.rtc;
        *subsecond_cycles = restore.metadata.subsecond_cycles;
        let elapsed_nanos = restore
            .resumed_at
            .get()
            .checked_sub(restore.metadata.checkpoint.get())
            .expect("MBC3 restore checked host-clock ordering");
        advance_rtc_host_nanos(rtc, subsecond_cycles, elapsed_nanos);
        Ok(())
    }

    fn mbc3_battery_metadata(
        &self,
        checkpoint: HostUnixNanos,
    ) -> Result<Option<Mbc3BatteryMetadata>, TransferPakError> {
        let Mapper::Mbc3 {
            timer_present,
            rtc,
            subsecond_cycles,
            ..
        } = &self.mapper
        else {
            return Ok(None);
        };
        if !timer_present {
            return Ok(None);
        }
        let cartridge_type = self.rom[0x147];
        if !is_timer_battery_mbc3(cartridge_type) {
            return Err(TransferPakError::BatteryMetadataForNonTimerCartridge(
                cartridge_type,
            ));
        }
        Ok(Some(Mbc3BatteryMetadata {
            rom_sha256: rom_sha256(&self.rom),
            cartridge_type,
            rtc: *rtc,
            subsecond_cycles: *subsecond_cycles,
            checkpoint,
        }))
    }

    fn evidence_snapshot(&self) -> GameBoyCartridgeEvidenceSnapshot {
        let mapper = match &self.mapper {
            Mapper::RomOnly => GameBoyMapperEvidenceSnapshot::RomOnly,
            Mapper::Mbc1 {
                ram_enabled,
                rom_low5,
                upper2,
                ram_mode,
            } => GameBoyMapperEvidenceSnapshot::Mbc1 {
                ram_enabled: *ram_enabled,
                rom_low5: *rom_low5,
                upper2: *upper2,
                ram_mode: *ram_mode,
            },
            Mapper::Mbc2 {
                ram_enabled,
                rom_bank,
            } => GameBoyMapperEvidenceSnapshot::Mbc2 {
                ram_enabled: *ram_enabled,
                rom_bank: *rom_bank,
            },
            Mapper::Mbc3 {
                timer_present,
                ram_enabled,
                rom_bank,
                select,
                latch_armed,
                rtc,
                latched_rtc,
                subsecond_cycles,
            } => GameBoyMapperEvidenceSnapshot::Mbc3 {
                timer_present: *timer_present,
                ram_enabled: *ram_enabled,
                rom_bank: *rom_bank,
                select: *select,
                latch_armed: *latch_armed,
                rtc: *rtc,
                latched_rtc: *latched_rtc,
                subsecond_cycles: *subsecond_cycles,
            },
            Mapper::Mbc5 {
                ram_enabled,
                rom_bank,
                ram_bank,
                rumble_variant,
            } => GameBoyMapperEvidenceSnapshot::Mbc5 {
                ram_enabled: *ram_enabled,
                rom_bank: *rom_bank,
                ram_bank: *ram_bank,
                rumble_variant: *rumble_variant,
            },
        };
        GameBoyCartridgeEvidenceSnapshot {
            rom: self.rom.clone(),
            ram: self.ram.clone(),
            mapper,
        }
    }

    fn new(rom: Vec<u8>, ram: Option<Vec<u8>>) -> Result<Self, TransferPakError> {
        if rom.len() < 0x150 {
            return Err(TransferPakError::RomTooSmall);
        }
        let cartridge_type = rom[0x147];
        let mapper = match cartridge_type {
            0x00 | 0x08 | 0x09 => Mapper::RomOnly,
            0x01..=0x03 => Mapper::Mbc1 {
                ram_enabled: false,
                rom_low5: 1,
                upper2: 0,
                ram_mode: false,
            },
            0x05 | 0x06 => Mapper::Mbc2 {
                ram_enabled: false,
                rom_bank: 1,
            },
            0x0f..=0x13 => Mapper::Mbc3 {
                timer_present: matches!(cartridge_type, 0x0f | 0x10),
                ram_enabled: false,
                rom_bank: 1,
                select: 0,
                latch_armed: false,
                rtc: [0; 5],
                latched_rtc: [0; 5],
                subsecond_cycles: 0,
            },
            0x19..=0x1e => Mapper::Mbc5 {
                ram_enabled: false,
                rom_bank: 1,
                ram_bank: 0,
                rumble_variant: matches!(cartridge_type, 0x1c..=0x1e),
            },
            other => {
                crate::record_unsupported_event(
                    crate::UnsupportedSubsystem::Runtime,
                    "runtime.transfer-pak.cartridge-type",
                    format!("Game Boy cartridge type {other:#04x} is unsupported"),
                    None,
                    crate::UnsupportedDisposition::ReturnedError,
                );
                return Err(TransferPakError::UnsupportedCartridgeType(other));
            }
        };
        let expected_ram = if matches!(cartridge_type, 0x05 | 0x06) {
            512
        } else {
            ram_size_from_header(rom[0x149])
        };
        let ram = ram.unwrap_or_else(|| vec![0xff; expected_ram]);
        if ram.len() != expected_ram {
            return Err(TransferPakError::RamSize {
                expected: expected_ram,
                actual: ram.len(),
            });
        }
        Ok(Self { rom, ram, mapper })
    }

    fn advance_cycles(&mut self, elapsed: u64) {
        let Mapper::Mbc3 {
            timer_present: true,
            rtc,
            subsecond_cycles,
            ..
        } = &mut self.mapper
        else {
            return;
        };
        if rtc[4] & 0x40 != 0 {
            return;
        }
        let total = u128::from(*subsecond_cycles) + u128::from(elapsed);
        *subsecond_cycles = (total % u128::from(CPU_CLOCK_HZ)) as u64;
        let seconds = total / u128::from(CPU_CLOCK_HZ);
        if seconds == 0 {
            return;
        }
        advance_rtc_seconds(rtc, seconds);
    }

    fn read(&mut self, address: u16) -> u8 {
        match address {
            0x0000..=0x3fff => {
                let bank = match self.mapper {
                    Mapper::Mbc1 {
                        upper2,
                        ram_mode: true,
                        ..
                    } => usize::from(upper2) << 5,
                    _ => 0,
                };
                self.rom_byte(bank, usize::from(address))
            }
            0x4000..=0x7fff => {
                let bank = match self.mapper {
                    Mapper::RomOnly => 1,
                    Mapper::Mbc1 {
                        rom_low5, upper2, ..
                    } => usize::from((upper2 << 5) | rom_low5),
                    Mapper::Mbc2 { rom_bank, .. } => usize::from(rom_bank),
                    Mapper::Mbc3 { rom_bank, .. } => usize::from(rom_bank),
                    Mapper::Mbc5 { rom_bank, .. } => usize::from(rom_bank),
                };
                self.rom_byte(bank, usize::from(address - 0x4000))
            }
            0xa000..=0xbfff => self.read_ram_or_rtc(address),
            _ => 0xff,
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        match address {
            0x0000..=0x1fff => match &mut self.mapper {
                Mapper::Mbc1 { ram_enabled, .. }
                | Mapper::Mbc3 { ram_enabled, .. }
                | Mapper::Mbc5 { ram_enabled, .. } => *ram_enabled = value & 0x0f == 0x0a,
                Mapper::Mbc2 { ram_enabled, .. } if address & 0x0100 == 0 => {
                    *ram_enabled = value & 0x0f == 0x0a
                }
                _ => {}
            },
            0x2000..=0x3fff => match &mut self.mapper {
                Mapper::Mbc1 { rom_low5, .. } => *rom_low5 = (value & 0x1f).max(1),
                Mapper::Mbc2 { rom_bank, .. } if address & 0x0100 != 0 => {
                    *rom_bank = (value & 0x0f).max(1)
                }
                Mapper::Mbc3 { rom_bank, .. } => *rom_bank = (value & 0x7f).max(1),
                Mapper::Mbc5 { rom_bank, .. } if address <= 0x2fff => {
                    *rom_bank = (*rom_bank & 0x100) | u16::from(value)
                }
                Mapper::Mbc5 { rom_bank, .. } => {
                    *rom_bank = (*rom_bank & 0x0ff) | (u16::from(value & 1) << 8)
                }
                _ => {}
            },
            0x4000..=0x5fff => match &mut self.mapper {
                Mapper::Mbc1 { upper2, .. } => *upper2 = value & 0x03,
                Mapper::Mbc3 { select, .. } => *select = value,
                Mapper::Mbc5 {
                    ram_bank,
                    rumble_variant,
                    ..
                } => *ram_bank = value & if *rumble_variant { 0x07 } else { 0x0f },
                _ => {}
            },
            0x6000..=0x7fff => match &mut self.mapper {
                Mapper::Mbc1 { ram_mode, .. } => *ram_mode = value & 1 != 0,
                Mapper::Mbc3 {
                    latch_armed,
                    rtc,
                    latched_rtc,
                    ..
                } => {
                    if *latch_armed && value == 1 {
                        *latched_rtc = *rtc;
                    }
                    *latch_armed = value == 0;
                }
                _ => {}
            },
            0xa000..=0xbfff => self.write_ram_or_rtc(address, value),
            _ => {}
        }
    }

    fn rom_byte(&self, bank: usize, offset: usize) -> u8 {
        self.rom
            .get(bank * GB_ROM_BANK_SIZE + offset)
            .copied()
            .unwrap_or(0xff)
    }

    fn read_ram_or_rtc(&self, address: u16) -> u8 {
        let offset = usize::from(address - 0xa000);
        match &self.mapper {
            Mapper::RomOnly => self.ram.get(offset).copied().unwrap_or(0xff),
            Mapper::Mbc1 {
                ram_enabled,
                upper2,
                ram_mode,
                ..
            } if *ram_enabled => {
                let bank = if *ram_mode { usize::from(*upper2) } else { 0 };
                self.ram_byte(bank, offset)
            }
            Mapper::Mbc2 {
                ram_enabled: true, ..
            } => self
                .ram
                .get(offset & 0x01ff)
                .map(|value| 0xf0 | (value & 0x0f))
                .unwrap_or(0xff),
            Mapper::Mbc3 {
                ram_enabled: true,
                timer_present,
                select,
                latched_rtc,
                ..
            } => match *select {
                0x00..=0x03 => self.ram_byte(usize::from(*select), offset),
                0x08..=0x0c if *timer_present => latched_rtc[usize::from(*select - 0x08)],
                _ => 0xff,
            },
            Mapper::Mbc5 {
                ram_enabled: true,
                ram_bank,
                ..
            } => self.ram_byte(usize::from(*ram_bank), offset),
            _ => 0xff,
        }
    }

    fn write_ram_or_rtc(&mut self, address: u16, value: u8) {
        let offset = usize::from(address - 0xa000);
        let target = match &mut self.mapper {
            Mapper::RomOnly => Some(offset),
            Mapper::Mbc1 {
                ram_enabled,
                upper2,
                ram_mode,
                ..
            } if *ram_enabled => {
                let bank = if *ram_mode { usize::from(*upper2) } else { 0 };
                Some(bank * GB_RAM_BANK_SIZE + offset)
            }
            Mapper::Mbc2 {
                ram_enabled: true, ..
            } => {
                if let Some(byte) = self.ram.get_mut(offset & 0x01ff) {
                    *byte = value & 0x0f;
                }
                None
            }
            Mapper::Mbc3 {
                ram_enabled: true,
                timer_present,
                select,
                rtc,
                ..
            } => match *select {
                0x00..=0x03 => Some(usize::from(*select) * GB_RAM_BANK_SIZE + offset),
                0x08..=0x0c if *timer_present => {
                    write_rtc_register(rtc, *select, value);
                    None
                }
                _ => None,
            },
            Mapper::Mbc5 {
                ram_enabled: true,
                ram_bank,
                ..
            } => Some(usize::from(*ram_bank) * GB_RAM_BANK_SIZE + offset),
            _ => None,
        };
        if let Some(target) = target.and_then(|target| self.ram.get_mut(target)) {
            *target = value;
        }
    }

    fn ram_byte(&self, bank: usize, offset: usize) -> u8 {
        self.ram
            .get(bank * GB_RAM_BANK_SIZE + offset)
            .copied()
            .unwrap_or(0xff)
    }
}

fn advance_rtc_seconds(rtc: &mut [u8; 5], elapsed_seconds: u128) {
    const SECONDS_PER_MINUTE: u128 = 60;
    const SECONDS_PER_HOUR: u128 = 60 * SECONDS_PER_MINUTE;
    const SECONDS_PER_DAY: u128 = 24 * SECONDS_PER_HOUR;
    const RTC_DAYS: u128 = 512;

    let day = u128::from(rtc[3]) | (u128::from(rtc[4] & 1) << 8);
    let current = u128::from(rtc[0])
        + u128::from(rtc[1]) * SECONDS_PER_MINUTE
        + u128::from(rtc[2]) * SECONDS_PER_HOUR
        + day * SECONDS_PER_DAY;
    let advanced = current + elapsed_seconds;
    let total_days = advanced / SECONDS_PER_DAY;
    let day = total_days % RTC_DAYS;
    let within_day = advanced % SECONDS_PER_DAY;
    rtc[0] = (within_day % SECONDS_PER_MINUTE) as u8;
    rtc[1] = ((within_day / SECONDS_PER_MINUTE) % 60) as u8;
    rtc[2] = (within_day / SECONDS_PER_HOUR) as u8;
    rtc[3] = day as u8;
    let halt = rtc[4] & 0x40;
    let carry = (rtc[4] & 0x80) | (u8::from(total_days >= RTC_DAYS) << 7);
    rtc[4] = ((day >> 8) as u8) | halt | carry;
}

/// Convert caller-injected wall time once at restore. Fractional nanoseconds
/// are deterministically rounded down to the runtime's guest-cycle phase; at
/// most one guest cycle is discarded per restore boundary.
fn advance_rtc_host_nanos(rtc: &mut [u8; 5], subsecond_cycles: &mut u64, elapsed_nanos: u64) {
    if rtc[4] & 0x40 != 0 {
        return;
    }
    const NANOS_PER_SECOND: u128 = 1_000_000_000;
    let elapsed_nanos = u128::from(elapsed_nanos);
    let whole_seconds = elapsed_nanos / NANOS_PER_SECOND;
    let fractional_nanos = elapsed_nanos % NANOS_PER_SECOND;
    let fractional_cycles = fractional_nanos * u128::from(CPU_CLOCK_HZ) / NANOS_PER_SECOND;
    let total_cycles = u128::from(*subsecond_cycles) + fractional_cycles;
    *subsecond_cycles = (total_cycles % u128::from(CPU_CLOCK_HZ)) as u64;
    advance_rtc_seconds(rtc, whole_seconds + total_cycles / u128::from(CPU_CLOCK_HZ));
}

fn is_timer_battery_mbc3(cartridge_type: u8) -> bool {
    matches!(cartridge_type, 0x0f | 0x10)
}

fn rom_sha256(rom: &[u8]) -> [u8; 32] {
    Sha256::digest(rom).into()
}

fn validate_rtc(rtc: [u8; 5]) -> Result<(), Mbc3BatteryMetadataError> {
    let invalid = if rtc[0] >= 60 {
        Some((0x08, rtc[0]))
    } else if rtc[1] >= 60 {
        Some((0x09, rtc[1]))
    } else if rtc[2] >= 24 {
        Some((0x0a, rtc[2]))
    } else if rtc[4] & 0x3e != 0 {
        Some((0x0c, rtc[4]))
    } else {
        None
    };
    if let Some((register, value)) = invalid {
        return Err(Mbc3BatteryMetadataError::InvalidRtcRegister { register, value });
    }
    Ok(())
}

fn write_rtc_register(rtc: &mut [u8; 5], select: u8, value: u8) {
    let index = usize::from(select - 0x08);
    match select {
        0x08 => assert!(value < 60, "MBC3 RTC seconds write {value} exceeds 59"),
        0x09 => assert!(value < 60, "MBC3 RTC minutes write {value} exceeds 59"),
        0x0a => assert!(value < 24, "MBC3 RTC hours write {value} exceeds 23"),
        0x0b => {}
        0x0c => assert!(
            value & 0x3e == 0,
            "MBC3 RTC day-high write {value:#04x} sets undefined bits 1..5"
        ),
        _ => unreachable!("RTC register selection was validated by caller"),
    }
    rtc[index] = value;
}

fn ram_size_from_header(code: u8) -> usize {
    match code {
        0x00 => 0,
        0x01 => 2 * 1024,
        0x02 => 8 * 1024,
        0x03 => 32 * 1024,
        0x04 => 128 * 1024,
        0x05 => 64 * 1024,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom(cartridge_type: u8, banks: usize, ram_size: u8) -> Vec<u8> {
        let mut rom = vec![0xff; banks * GB_ROM_BANK_SIZE];
        rom[0x147] = cartridge_type;
        rom[0x149] = ram_size;
        for bank in 0..banks {
            rom[bank * GB_ROM_BANK_SIZE] = bank as u8;
        }
        rom
    }

    fn filled(value: u8) -> [u8; TRANSFER_PAK_BLOCK_SIZE] {
        [value; TRANSFER_PAK_BLOCK_SIZE]
    }

    fn rtc_write(cartridge: &mut GameBoyCartridge, register: u8, value: u8) {
        cartridge.write(0x4000, register);
        cartridge.write(0xa000, value);
    }

    fn rtc_latch(cartridge: &mut GameBoyCartridge) {
        cartridge.write(0x6000, 0);
        cartridge.write(0x6000, 1);
    }

    fn rtc_read(cartridge: &mut GameBoyCartridge, register: u8) -> u8 {
        cartridge.write(0x4000, register);
        cartridge.read(0xa000)
    }

    fn mbc3_evidence(pak: &TransferPak) -> GameBoyMapperEvidenceSnapshot {
        pak.evidence_snapshot().cartridge.unwrap().mapper
    }

    #[test]
    fn power_probe_bank_mode_and_window_are_distinct_registers() {
        let mut pak = TransferPak::new();
        pak.insert_cartridge(rom(0x00, 2, 0), None).unwrap();
        let mut data = [0; TRANSFER_PAK_BLOCK_SIZE];
        pak.read_block(0x8000, &mut data);
        assert_eq!(data, filled(0));
        pak.write_block(0x8000, &filled(0x84));
        pak.read_block(0x8000, &mut data);
        assert_eq!(data, filled(0x84));
        pak.read_block(0xb000, &mut data);
        assert_eq!(data, filled(0x84));
        pak.write_block(0xb000, &filled(1));
        pak.read_block(0xb000, &mut data);
        assert_eq!(data, filled(0x89));
        pak.write_block(0xa000, &filled(1));
        pak.read_block(0xc000, &mut data);
        assert_eq!(data[0], 1);
    }

    #[test]
    fn cartridge_removal_and_reset_are_sticky_until_typed_status_observes_them() {
        let mut pak = TransferPak::new();
        pak.insert_cartridge(rom(0x00, 2, 0), None).unwrap();
        pak.set_power(true);
        assert!(pak.remove_cartridge().is_some());
        let mut raw = [0; TRANSFER_PAK_BLOCK_SIZE];
        pak.read_block(0xb000, &mut raw);
        assert_eq!(raw, filled(0xc0));

        pak.insert_cartridge(rom(0x00, 2, 0), None).unwrap();
        let first = pak.observe_status();
        assert!(first.cartridge_present);
        assert!(first.cartridge_pulled);
        assert!(first.reset_detected);
        let second = pak.observe_status();
        assert!(!second.cartridge_pulled);
        assert!(!second.reset_detected);
    }

    #[test]
    fn mbc1_register_writes_rebank_rom_and_ram_through_the_same_window() {
        let mut pak = TransferPak::new();
        pak.insert_cartridge(rom(0x03, 64, 3), None).unwrap();
        pak.write_block(0x8000, &filled(0x84));

        // Transfer bank zero exposes GB 0x0000..0x3fff. Writing 2 to GB
        // 0x2000 selects ROM bank 2, then Transfer bank one reads it.
        pak.write_block(0xe000, &filled(2));
        pak.write_block(0xa000, &filled(1));
        let mut data = [0; TRANSFER_PAK_BLOCK_SIZE];
        pak.read_block(0xc000, &mut data);
        assert_eq!(data[0], 2);

        // Enable RAM at GB 0x0000, select RAM banking mode/upper bank 2,
        // then write/read GB 0xa000 through Transfer bank two.
        pak.write_block(0xa000, &filled(0));
        pak.write_block(0xc000, &filled(0x0a));
        pak.write_block(0xa000, &filled(1));
        pak.write_block(0xc000, &filled(2));
        pak.write_block(0xe000, &filled(1));
        pak.write_block(0xa000, &filled(2));
        pak.write_block(0xe000, &filled(0x5a));
        pak.read_block(0xe000, &mut data);
        assert_eq!(data, filled(0x5a));
        assert_eq!(pak.cartridge_ram().unwrap()[2 * GB_RAM_BANK_SIZE], 0x5a);
    }

    #[test]
    fn mbc3_rtc_latch_is_deterministic_and_mbc5_uses_nine_rom_bank_bits() {
        let mut mbc3 = GameBoyCartridge::new(rom(0x10, 4, 3), None).unwrap();
        mbc3.write(0x0000, 0x0a);
        mbc3.write(0x4000, 0x08);
        mbc3.write(0xa000, 37);
        mbc3.write(0x6000, 0);
        mbc3.write(0x6000, 1);
        assert_eq!(mbc3.read(0xa000), 37);

        let mut mbc5_rom = rom(0x1b, 258, 3);
        mbc5_rom[257 * GB_ROM_BANK_SIZE] = 0xa5;
        let mut mbc5 = GameBoyCartridge::new(mbc5_rom, None).unwrap();
        mbc5.write(0x2000, 1);
        mbc5.write(0x3000, 1);
        assert_eq!(mbc5.read(0x4000), 0xa5);
    }

    #[test]
    fn mbc3_rtc_ticks_at_exact_guest_second_and_latch_stays_immutable() {
        let mut cartridge = GameBoyCartridge::new(rom(0x10, 4, 3), None).unwrap();
        cartridge.write(0x0000, 0x0a);
        rtc_latch(&mut cartridge);
        assert_eq!(rtc_read(&mut cartridge, 0x08), 0);

        cartridge.advance_cycles(CPU_CLOCK_HZ - 1);
        rtc_latch(&mut cartridge);
        assert_eq!(rtc_read(&mut cartridge, 0x08), 0);
        cartridge.advance_cycles(1);
        assert_eq!(
            rtc_read(&mut cartridge, 0x08),
            0,
            "latched snapshot must not track the live counter"
        );
        rtc_latch(&mut cartridge);
        assert_eq!(rtc_read(&mut cartridge, 0x08), 1);
    }

    #[test]
    fn transfer_pak_power_does_not_gate_battery_backed_mbc3_clock() {
        let mut pak = TransferPak::new();
        pak.insert_cartridge(rom(0x10, 4, 3), None).unwrap();
        assert!(!pak.enabled());
        pak.advance_to(Cycles::new(CPU_CLOCK_HZ));
        pak.set_power(true);
        pak.write_game_boy_block(0x0000, &filled(0x0a));
        pak.write_game_boy_block(0x6000, &filled(0));
        pak.write_game_boy_block(0x6000, &filled(1));
        pak.write_game_boy_block(0x4000, &filled(0x08));
        let mut seconds = [0; TRANSFER_PAK_BLOCK_SIZE];
        pak.read_game_boy_block(0xa000, &mut seconds);
        assert_eq!(seconds, filled(1));
    }

    #[test]
    fn mbc3_rtc_halt_freezes_elapsed_time_and_resume_keeps_fraction() {
        let mut cartridge = GameBoyCartridge::new(rom(0x10, 4, 3), None).unwrap();
        cartridge.write(0x0000, 0x0a);
        cartridge.advance_cycles(CPU_CLOCK_HZ / 2);
        rtc_write(&mut cartridge, 0x0c, 0x40);
        cartridge.advance_cycles(CPU_CLOCK_HZ * 2);
        rtc_latch(&mut cartridge);
        assert_eq!(rtc_read(&mut cartridge, 0x08), 0);
        assert_eq!(rtc_read(&mut cartridge, 0x0c) & 0x40, 0x40);

        rtc_write(&mut cartridge, 0x0c, 0x00);
        cartridge.advance_cycles(CPU_CLOCK_HZ / 2 - 1);
        rtc_latch(&mut cartridge);
        assert_eq!(rtc_read(&mut cartridge, 0x08), 0);
        cartridge.advance_cycles(1);
        rtc_latch(&mut cartridge);
        assert_eq!(rtc_read(&mut cartridge, 0x08), 1);
    }

    #[test]
    fn mbc3_rtc_day_overflow_sets_sticky_carry_until_software_clears_it() {
        let mut cartridge = GameBoyCartridge::new(rom(0x10, 4, 3), None).unwrap();
        cartridge.write(0x0000, 0x0a);
        rtc_write(&mut cartridge, 0x0c, 0x41);
        rtc_write(&mut cartridge, 0x08, 59);
        rtc_write(&mut cartridge, 0x09, 59);
        rtc_write(&mut cartridge, 0x0a, 23);
        rtc_write(&mut cartridge, 0x0b, 0xff);
        rtc_write(&mut cartridge, 0x0c, 0x01);

        cartridge.advance_cycles(CPU_CLOCK_HZ);
        rtc_latch(&mut cartridge);
        assert_eq!(rtc_read(&mut cartridge, 0x08), 0);
        assert_eq!(rtc_read(&mut cartridge, 0x09), 0);
        assert_eq!(rtc_read(&mut cartridge, 0x0a), 0);
        assert_eq!(rtc_read(&mut cartridge, 0x0b), 0);
        assert_eq!(rtc_read(&mut cartridge, 0x0c), 0x80);

        cartridge.advance_cycles(CPU_CLOCK_HZ);
        rtc_latch(&mut cartridge);
        assert_eq!(rtc_read(&mut cartridge, 0x08), 1);
        assert_eq!(rtc_read(&mut cartridge, 0x0c), 0x80);
        rtc_write(&mut cartridge, 0x0c, 0);
        rtc_latch(&mut cartridge);
        assert_eq!(rtc_read(&mut cartridge, 0x0c), 0);
    }

    #[test]
    fn plain_mbc3_without_timer_does_not_fabricate_rtc_registers() {
        let mut cartridge = GameBoyCartridge::new(rom(0x13, 4, 3), None).unwrap();
        cartridge.write(0x0000, 0x0a);
        rtc_write(&mut cartridge, 0x08, 37);
        cartridge.advance_cycles(CPU_CLOCK_HZ * 10);
        rtc_latch(&mut cartridge);
        assert_eq!(rtc_read(&mut cartridge, 0x08), 0xff);
    }

    #[test]
    fn mbc3_battery_sidecar_roundtrips_and_materializes_offline_time_once() {
        let image = rom(0x10, 4, 3);
        let mut first = TransferPak::new();
        first.insert_cartridge(image.clone(), None).unwrap();
        let metadata = first
            .checkpoint_mbc3_battery(
                Cycles::new(CPU_CLOCK_HZ / 2),
                HostUnixNanos::new(10_000_000_000),
            )
            .unwrap()
            .unwrap();
        let decoded = Mbc3BatteryMetadata::decode(&metadata.encode()).unwrap();
        assert_eq!(decoded, metadata);

        let mut restored = TransferPak::new();
        restored
            .insert_cartridge_with_battery(
                image,
                None,
                Some(Mbc3BatteryRestore::new(
                    decoded,
                    HostUnixNanos::new(11_500_000_000),
                )),
            )
            .unwrap();
        match mbc3_evidence(&restored) {
            GameBoyMapperEvidenceSnapshot::Mbc3 {
                rtc,
                subsecond_cycles,
                ..
            } => {
                assert_eq!(rtc[0], 2);
                assert_eq!(subsecond_cycles, 0);
            }
            other => panic!("expected MBC3 evidence, got {other:?}"),
        }
    }

    #[test]
    fn host_timestamps_are_discarded_after_equal_elapsed_restore() {
        let image = rom(0x10, 4, 3);
        let mut source = TransferPak::new();
        source.insert_cartridge(image.clone(), None).unwrap();
        let first = source
            .checkpoint_mbc3_battery(Cycles::new(0), HostUnixNanos::new(1_000_000_000))
            .unwrap()
            .unwrap();
        let mut second_bytes = first.encode();
        second_bytes[64..72].copy_from_slice(&21_000_000_000_u64.to_be_bytes());
        let checksum = Sha256::digest(&second_bytes[..MBC3_BATTERY_CHECKSUM_OFFSET]);
        second_bytes[MBC3_BATTERY_CHECKSUM_OFFSET..].copy_from_slice(&checksum);
        let second = Mbc3BatteryMetadata::decode(&second_bytes).unwrap();

        let mut a = TransferPak::new();
        a.insert_cartridge_with_battery(
            image.clone(),
            None,
            Some(Mbc3BatteryRestore::new(
                first,
                HostUnixNanos::new(4_250_000_000),
            )),
        )
        .unwrap();
        let mut b = TransferPak::new();
        b.insert_cartridge_with_battery(
            image,
            None,
            Some(Mbc3BatteryRestore::new(
                second,
                HostUnixNanos::new(24_250_000_000),
            )),
        )
        .unwrap();
        assert_eq!(a.evidence_snapshot(), b.evidence_snapshot());
    }

    #[test]
    fn battery_restore_rejects_clock_rollback_and_wrong_cartridge() {
        let image = rom(0x10, 4, 3);
        let mut source = TransferPak::new();
        source.insert_cartridge(image.clone(), None).unwrap();
        let metadata = source
            .checkpoint_mbc3_battery(Cycles::new(0), HostUnixNanos::new(20))
            .unwrap()
            .unwrap();

        let mut rollback = TransferPak::new();
        assert_eq!(
            rollback
                .insert_cartridge_with_battery(
                    image.clone(),
                    None,
                    Some(Mbc3BatteryRestore::new(
                        metadata.clone(),
                        HostUnixNanos::new(19),
                    )),
                )
                .unwrap_err(),
            TransferPakError::BatteryClockMovedBackward {
                checkpoint: HostUnixNanos::new(20),
                resumed_at: HostUnixNanos::new(19),
            }
        );

        let mut other_image = image;
        other_image[0x200] ^= 1;
        let mut mismatch = TransferPak::new();
        assert_eq!(
            mismatch
                .insert_cartridge_with_battery(
                    other_image,
                    None,
                    Some(Mbc3BatteryRestore::new(metadata, HostUnixNanos::new(20),)),
                )
                .unwrap_err(),
            TransferPakError::BatteryMetadataRomMismatch
        );
    }

    #[test]
    fn battery_sidecar_rejects_corruption_unknown_version_and_invalid_payload() {
        let image = rom(0x10, 4, 3);
        let mut pak = TransferPak::new();
        pak.insert_cartridge(image, None).unwrap();
        let metadata = pak
            .checkpoint_mbc3_battery(Cycles::new(0), HostUnixNanos::new(0))
            .unwrap()
            .unwrap();

        let mut corrupt = metadata.encode();
        corrupt[45] ^= 1;
        assert_eq!(
            Mbc3BatteryMetadata::decode(&corrupt).unwrap_err(),
            Mbc3BatteryMetadataError::ChecksumMismatch
        );

        let mut future = metadata.encode();
        future[8..10].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(
            Mbc3BatteryMetadata::decode(&future).unwrap_err(),
            Mbc3BatteryMetadataError::UnsupportedVersion(2)
        );

        let mut invalid = metadata.encode();
        invalid[45] = 60;
        let checksum = Sha256::digest(&invalid[..MBC3_BATTERY_CHECKSUM_OFFSET]);
        invalid[MBC3_BATTERY_CHECKSUM_OFFSET..].copy_from_slice(&checksum);
        assert_eq!(
            Mbc3BatteryMetadata::decode(&invalid).unwrap_err(),
            Mbc3BatteryMetadataError::InvalidRtcRegister {
                register: 0x08,
                value: 60,
            }
        );

        assert_eq!(
            Mbc3BatteryMetadata::decode(&metadata.encode()[..103]).unwrap_err(),
            Mbc3BatteryMetadataError::InvalidLength {
                expected: MBC3_BATTERY_METADATA_LEN,
                actual: 103,
            }
        );
        let mut invalid = metadata.encode();
        invalid[50] = 1;
        let checksum = Sha256::digest(&invalid[..MBC3_BATTERY_CHECKSUM_OFFSET]);
        invalid[MBC3_BATTERY_CHECKSUM_OFFSET..].copy_from_slice(&checksum);
        assert_eq!(
            Mbc3BatteryMetadata::decode(&invalid).unwrap_err(),
            Mbc3BatteryMetadataError::NonzeroReservedBytes
        );
        let mut invalid = metadata.encode();
        invalid[56..64].copy_from_slice(&CPU_CLOCK_HZ.to_be_bytes());
        let checksum = Sha256::digest(&invalid[..MBC3_BATTERY_CHECKSUM_OFFSET]);
        invalid[MBC3_BATTERY_CHECKSUM_OFFSET..].copy_from_slice(&checksum);
        assert_eq!(
            Mbc3BatteryMetadata::decode(&invalid).unwrap_err(),
            Mbc3BatteryMetadataError::InvalidSubsecondCycles(CPU_CLOCK_HZ)
        );
    }

    #[test]
    fn battery_restore_rejects_type_mismatch_and_non_timer_mbc3() {
        let timer_image = rom(0x10, 4, 3);
        let mut source = TransferPak::new();
        source.insert_cartridge(timer_image.clone(), None).unwrap();
        let metadata = source
            .checkpoint_mbc3_battery(Cycles::ZERO, HostUnixNanos::new(0))
            .unwrap()
            .unwrap();

        let mut other_type_bytes = metadata.encode();
        other_type_bytes[44] = 0x0f;
        let checksum = Sha256::digest(&other_type_bytes[..MBC3_BATTERY_CHECKSUM_OFFSET]);
        other_type_bytes[MBC3_BATTERY_CHECKSUM_OFFSET..].copy_from_slice(&checksum);
        let other_type = Mbc3BatteryMetadata::decode(&other_type_bytes).unwrap();
        let mut mismatch = TransferPak::new();
        assert_eq!(
            mismatch
                .insert_cartridge_with_battery(
                    timer_image,
                    None,
                    Some(Mbc3BatteryRestore::new(other_type, HostUnixNanos::new(0),)),
                )
                .unwrap_err(),
            TransferPakError::BatteryMetadataCartridgeType {
                expected: 0x10,
                actual: 0x0f,
            }
        );

        let mut no_timer = TransferPak::new();
        assert_eq!(
            no_timer
                .insert_cartridge_with_battery(
                    rom(0x13, 4, 3),
                    None,
                    Some(Mbc3BatteryRestore::new(metadata, HostUnixNanos::new(0),)),
                )
                .unwrap_err(),
            TransferPakError::BatteryMetadataForNonTimerCartridge(0x13)
        );
    }

    #[test]
    fn battery_restore_preserves_halt_but_resets_powered_session_state() {
        let image = rom(0x10, 4, 3);
        let mut source = TransferPak::new();
        source.insert_cartridge(image.clone(), None).unwrap();
        let cartridge = source.cartridge.as_mut().unwrap();
        cartridge.write(0x0000, 0x0a);
        cartridge.write(0x2000, 7);
        rtc_write(cartridge, 0x08, 12);
        rtc_write(cartridge, 0x0c, 0x40);
        rtc_latch(cartridge);
        let metadata = source
            .checkpoint_mbc3_battery(Cycles::new(0), HostUnixNanos::new(1))
            .unwrap()
            .unwrap();

        let mut restored = TransferPak::new();
        restored
            .insert_cartridge_with_battery(
                image,
                None,
                Some(Mbc3BatteryRestore::new(
                    metadata,
                    HostUnixNanos::new(10_000_000_001),
                )),
            )
            .unwrap();
        match mbc3_evidence(&restored) {
            GameBoyMapperEvidenceSnapshot::Mbc3 {
                ram_enabled,
                rom_bank,
                select,
                latch_armed,
                rtc,
                latched_rtc,
                ..
            } => {
                assert!(!ram_enabled);
                assert_eq!(rom_bank, 1);
                assert_eq!(select, 0);
                assert!(!latch_armed);
                assert_eq!(rtc[0], 12);
                assert_eq!(rtc[4] & 0x40, 0x40);
                assert_eq!(latched_rtc, [0; 5]);
            }
            other => panic!("expected MBC3 evidence, got {other:?}"),
        }
    }

    #[test]
    fn mbc2_uses_address_bit_eight_for_control_and_four_bit_ram() {
        let mut cartridge = GameBoyCartridge::new(rom(0x06, 16, 0), None).unwrap();
        cartridge.write(0x0000, 0x0a);
        cartridge.write(0x2100, 3);
        assert_eq!(cartridge.read(0x4000), 3);
        cartridge.write(0xa123, 0xab);
        assert_eq!(cartridge.read(0xa123), 0xfb);
        assert_eq!(cartridge.read(0xa323), 0xfb);
    }

    #[test]
    fn mapper_and_ram_shape_fail_loudly_at_attachment() {
        assert_eq!(
            GameBoyCartridge::new(rom(0xfc, 2, 0), None).unwrap_err(),
            TransferPakError::UnsupportedCartridgeType(0xfc)
        );
        assert_eq!(
            GameBoyCartridge::new(rom(0x03, 2, 3), Some(vec![0; 8])).unwrap_err(),
            TransferPakError::RamSize {
                expected: 32 * 1024,
                actual: 8
            }
        );
    }

    #[test]
    fn typed_unsupported_input_errors_enter_the_armed_event_source() {
        crate::arm_unsupported_events(None).unwrap();

        let mut future_metadata = [0; MBC3_BATTERY_METADATA_LEN];
        future_metadata[..8].copy_from_slice(&MBC3_BATTERY_MAGIC);
        future_metadata[8..10].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(
            Mbc3BatteryMetadata::decode(&future_metadata).unwrap_err(),
            Mbc3BatteryMetadataError::UnsupportedVersion(2)
        );
        assert_eq!(
            GameBoyCartridge::new(rom(0xfc, 2, 0), None).unwrap_err(),
            TransferPakError::UnsupportedCartridgeType(0xfc)
        );

        let events = crate::copy_unsupported_events();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].operation,
            "runtime.transfer-pak.battery-metadata-version"
        );
        assert_eq!(events[0].guest_cycle, None);
        assert_eq!(
            events[0].disposition,
            crate::UnsupportedDisposition::ReturnedError
        );
        assert_eq!(events[1].operation, "runtime.transfer-pak.cartridge-type");
        assert_eq!(events[1].guest_cycle, None);
        assert_eq!(
            events[1].disposition,
            crate::UnsupportedDisposition::ReturnedError
        );
        crate::complete_unsupported_observation(Cycles::ZERO, &"0".repeat(64));
    }
}
