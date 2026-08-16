//! Loader and identity checks for an out-of-tree boot-context observation.

use std::fmt;
use std::path::Path;

use fn64_cpu_runtime::{BootContext, BootTvStandard, Sha256Digest};
use sha2::{Digest, Sha256};

use crate::TvType;

/// Load a canonical boot context and bind it to the exact normalized ROM
/// bytes and configured device clock that will consume it.
pub fn load_boot_context(
    path: &Path,
    normalized_rom: &[u8],
    tv_type: TvType,
) -> Result<BootContext, BootContextLoadError> {
    let bytes = std::fs::read(path).map_err(|source| BootContextLoadError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse_boot_context(&bytes, normalized_rom, tv_type)
}

pub fn parse_boot_context(
    bytes: &[u8],
    normalized_rom: &[u8],
    tv_type: TvType,
) -> Result<BootContext, BootContextLoadError> {
    let context: BootContext =
        serde_json::from_slice(bytes).map_err(BootContextLoadError::Parse)?;
    context.validate().map_err(BootContextLoadError::Invalid)?;

    let expected_rom = Sha256Digest::from_bytes(Sha256::digest(normalized_rom).into());
    if context.normalized_rom_sha256 != expected_rom {
        return Err(BootContextLoadError::RomIdentityMismatch {
            context: context.normalized_rom_sha256,
            supplied: expected_rom,
        });
    }
    let context_tv = match context.region.tv_standard {
        BootTvStandard::Ntsc => TvType::Ntsc,
        BootTvStandard::Pal => TvType::Pal,
        BootTvStandard::Mpal => TvType::Mpal,
    };
    if context_tv != tv_type {
        return Err(BootContextLoadError::TvStandardMismatch {
            context: context.region.tv_standard,
            supplied: tv_type,
        });
    }
    Ok(context)
}

#[derive(Debug)]
pub enum BootContextLoadError {
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    Parse(serde_json::Error),
    Invalid(fn64_cpu_runtime::BootContextError),
    RomIdentityMismatch {
        context: Sha256Digest,
        supplied: Sha256Digest,
    },
    TvStandardMismatch {
        context: BootTvStandard,
        supplied: TvType,
    },
}

impl fmt::Display for BootContextLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "reading boot context {}: {source}", path.display())
            }
            Self::Parse(error) => write!(f, "parsing boot context JSON: {error}"),
            Self::Invalid(error) => write!(f, "invalid boot context: {error}"),
            Self::RomIdentityMismatch { context, supplied } => write!(
                f,
                "boot-context ROM identity {context} does not match supplied normalized ROM {supplied}"
            ),
            Self::TvStandardMismatch { context, supplied } => write!(
                f,
                "boot-context TV standard {context:?} does not match configured {supplied:?}"
            ),
        }
    }
}

impl std::error::Error for BootContextLoadError {}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_cpu_runtime::{BootCicIdentity, BootCop0Context, BootRegion, BOOT_CONTEXT_SCHEMA_V1};

    fn context(rom: &[u8]) -> BootContext {
        let mut cp0 = [0u64; 32];
        cp0[1] = 31;
        BootContext {
            schema: BOOT_CONTEXT_SCHEMA_V1.to_string(),
            producer: "synthetic black-box debugger".to_string(),
            normalized_rom_sha256: Sha256Digest::from_bytes(Sha256::digest(rom).into()),
            cic: BootCicIdentity {
                ipl3_sha256: Sha256Digest::from_bytes([0x55; 32]),
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
    fn parser_binds_exact_rom_and_tv_identity() {
        let rom = b"normalized synthetic ROM";
        let bytes = serde_json::to_vec(&context(rom)).unwrap();
        assert_eq!(
            parse_boot_context(&bytes, rom, TvType::Ntsc)
                .unwrap()
                .entry_pc,
            0x8000_0400
        );
        assert!(matches!(
            parse_boot_context(&bytes, b"other ROM", TvType::Ntsc),
            Err(BootContextLoadError::RomIdentityMismatch { .. })
        ));
        assert!(matches!(
            parse_boot_context(&bytes, rom, TvType::Pal),
            Err(BootContextLoadError::TvStandardMismatch { .. })
        ));
    }
}
