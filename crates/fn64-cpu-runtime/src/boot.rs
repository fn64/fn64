//! Typed CPU state at the IPL3-to-ROM-header handoff.
//!
//! A block program begins at an architectural PC, not at a C ABI function
//! whose prologue happens to initialize enough state.  Its initial register
//! file must therefore come from the machine that executed IPL3.  This wire
//! retains that black-box observation together with the ROM, IPL3, and TV
//! identities needed to decide whether it belongs to the current boot.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const BOOT_CONTEXT_SCHEMA_V1: &str = "fn64.boot-context.v1";

/// One canonical lowercase SHA-256 identity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }

    pub fn parse(value: &str) -> Result<Self, BootContextError> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(BootContextError::InvalidSha256(value.to_string()));
        }
        let mut bytes = [0u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let hex = std::str::from_utf8(pair).expect("ASCII was checked above");
            bytes[index] = u8::from_str_radix(hex, 16)
                .map_err(|_| BootContextError::InvalidSha256(value.to_string()))?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Concrete video clock selected for this boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootTvStandard {
    Ntsc,
    Pal,
    Mpal,
}

/// Header-derived region evidence plus the concrete clock used by the
/// black-box producer. Region-free software still needs an explicit clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootRegion {
    pub destination_code: u8,
    pub tv_standard: BootTvStandard,
}

/// Exact CIC-facing boot-code identity without guessing a marketing label
/// from a weak address or checksum match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootCicIdentity {
    pub ipl3_sha256: Sha256Digest,
}

/// Raw CP0 register image observed by the debugger boundary.
///
/// Slots use architectural register numbers. The 64-bit element width retains
/// BadVAddr, EntryHi, and XContext without forcing a future producer through
/// a lossy 32-bit wire; producers which expose only 32-bit CP0 values
/// zero-extend them explicitly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootCop0Context {
    pub registers: [u64; 32],
}

/// Canonical IPL3-to-header-entry state consumed by block-lane thread 0.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootContext {
    pub schema: String,
    pub producer: String,
    pub normalized_rom_sha256: Sha256Digest,
    pub cic: BootCicIdentity,
    pub region: BootRegion,
    pub entry_pc: u32,
    pub gprs: [u64; 32],
    pub hi: u64,
    pub lo: u64,
    pub cp0: BootCop0Context,
}

impl BootContext {
    /// Validate the identity-independent architectural invariants.
    pub fn validate(&self) -> Result<(), BootContextError> {
        if self.schema != BOOT_CONTEXT_SCHEMA_V1 {
            return Err(BootContextError::SchemaMismatch(self.schema.clone()));
        }
        if self.producer.trim().is_empty() {
            return Err(BootContextError::EmptyProducer);
        }
        if self.entry_pc & 3 != 0 {
            return Err(BootContextError::UnalignedEntryPc(self.entry_pc));
        }
        if self.gprs[0] != 0 {
            return Err(BootContextError::NonzeroZeroRegister(self.gprs[0]));
        }
        let wired = self.cp0.registers[6];
        let random = self.cp0.registers[1];
        if wired > 31 {
            return Err(BootContextError::InvalidWired(wired));
        }
        if random < wired || random > 31 {
            return Err(BootContextError::InvalidRandom { random, wired });
        }
        for &register in &[0usize, 2, 3, 4, 5, 6, 9, 11, 12, 13, 14, 18, 19, 30] {
            let value = self.cp0.registers[register];
            if value > u64::from(u32::MAX) {
                return Err(BootContextError::WideCop0Register { register, value });
            }
        }
        Ok(())
    }

    pub fn validate_for_entry(&self, entry_pc: u32) -> Result<(), BootContextError> {
        self.validate()?;
        if self.entry_pc != entry_pc {
            return Err(BootContextError::EntryPcMismatch {
                context: self.entry_pc,
                requested: entry_pc,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootContextError {
    SchemaMismatch(String),
    EmptyProducer,
    InvalidSha256(String),
    UnalignedEntryPc(u32),
    EntryPcMismatch { context: u32, requested: u32 },
    NonzeroZeroRegister(u64),
    InvalidWired(u64),
    InvalidRandom { random: u64, wired: u64 },
    WideCop0Register { register: usize, value: u64 },
}

/// Architectural field compared at the first generated-bank boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootContextStateField {
    Gpr(u8),
    Hi,
    Lo,
    Cop0(u8),
}

/// One exact difference between a captured handoff and live CPU state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootContextStateMismatch {
    pub field: BootContextStateField,
    pub expected: u64,
    pub actual: u64,
}

impl fmt::Display for BootContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SchemaMismatch(schema) => {
                write!(f, "unsupported boot-context schema {schema:?}")
            }
            Self::EmptyProducer => write!(f, "boot-context producer must not be empty"),
            Self::InvalidSha256(value) => {
                write!(f, "SHA-256 must be exactly 64 lowercase hexadecimal digits: {value:?}")
            }
            Self::UnalignedEntryPc(pc) => {
                write!(f, "boot-context entry PC 0x{pc:08x} is not four-byte aligned")
            }
            Self::EntryPcMismatch { context, requested } => write!(
                f,
                "boot-context entry PC 0x{context:08x} does not match requested block entry 0x{requested:08x}"
            ),
            Self::NonzeroZeroRegister(value) => {
                write!(f, "boot-context $zero is nonzero: 0x{value:016x}")
            }
            Self::InvalidWired(value) => {
                write!(f, "boot-context COP0 Wired {value} exceeds 31")
            }
            Self::InvalidRandom { random, wired } => write!(
                f,
                "boot-context COP0 Random {random} is outside Wired..=31 ({wired}..=31)"
            ),
            Self::WideCop0Register { register, value } => write!(
                f,
                "boot-context 32-bit COP0 register {register} contains 0x{value:016x}"
            ),
        }
    }
}

impl std::error::Error for BootContextError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> BootContext {
        let mut cp0 = [0u64; 32];
        cp0[1] = 31;
        BootContext {
            schema: BOOT_CONTEXT_SCHEMA_V1.to_string(),
            producer: "synthetic boot debugger".to_string(),
            normalized_rom_sha256: Sha256Digest::from_bytes([0x11; 32]),
            cic: BootCicIdentity {
                ipl3_sha256: Sha256Digest::from_bytes([0x22; 32]),
            },
            region: BootRegion {
                destination_code: b'E',
                tv_standard: BootTvStandard::Ntsc,
            },
            entry_pc: 0x8000_0400,
            gprs: [0; 32],
            hi: 0,
            lo: 0,
            cp0: BootCop0Context { registers: cp0 },
        }
    }

    #[test]
    fn canonical_json_roundtrip_retains_identities_and_registers() {
        let mut original = context();
        original.gprs[29] = 0xffff_ffff_a400_1ff0;
        original.cp0.registers[12] = 0x3400_0000;
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains(&"11".repeat(32)));
        assert_eq!(
            serde_json::from_str::<BootContext>(&json).unwrap(),
            original
        );
    }

    #[test]
    fn validation_rejects_wrong_entry_and_impossible_cp0_random() {
        let original = context();
        assert!(matches!(
            original.validate_for_entry(0x8000_0404),
            Err(BootContextError::EntryPcMismatch { .. })
        ));

        let mut invalid = original;
        invalid.cp0.registers[6] = 10;
        invalid.cp0.registers[1] = 9;
        assert!(matches!(
            invalid.validate(),
            Err(BootContextError::InvalidRandom {
                random: 9,
                wired: 10
            })
        ));
    }

    #[test]
    fn digest_wire_rejects_noncanonical_text() {
        assert!(Sha256Digest::parse(&"A".repeat(64)).is_err());
        assert!(Sha256Digest::parse(&"0".repeat(63)).is_err());
        assert_eq!(
            Sha256Digest::parse(&"ab".repeat(32)).unwrap().to_string(),
            "ab".repeat(32)
        );
    }
}
