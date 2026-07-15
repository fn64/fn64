//! Phase 1 (docs/DISCOVER-DESIGN.md): ROM normalization and identification.
//!
//! Every downstream artifact binds to `NormalizedRom::digest`, so this
//! module has exactly one job: turn arbitrary input bytes into a canonical
//! big-endian byte buffer plus its cryptographic identity, or reject the
//! input outright. No heuristics here -- byte order and the header layout
//! are hard N64 hardware facts, not guesses.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Standard N64 ROM header magic words, one per byte-order convention.
/// A `.z64` (big-endian, native) always starts with `80 37 12 40`.
const MAGIC_Z64: u32 = 0x8037_1240;
/// `.n64` (little-endian, byte-swapped) is the big-endian magic with every
/// 4-byte word byte-reversed.
const MAGIC_N64: u32 = 0x4012_3780;
/// `.v64` (byte-swapped within each 16-bit halfword) swaps adjacent bytes.
const MAGIC_V64: u32 = 0x3780_4012;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RomByteOrder {
    /// Native big-endian, no conversion needed.
    Z64,
    /// Little-endian words; convert by reversing each 4-byte word.
    N64,
    /// Byte-swapped halfwords; convert by swapping each adjacent byte pair.
    V64,
}

impl fmt::Display for RomByteOrder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RomByteOrder::Z64 => "z64",
            RomByteOrder::N64 => "n64",
            RomByteOrder::V64 => "v64",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RomRejectReason {
    /// Fewer bytes than a header needs; cannot even inspect the magic word.
    TooSmall { len: usize },
    /// The first 4 bytes did not match any known byte-order magic.
    UnknownMagic { first_word: u32 },
    /// Byte length is not a multiple of 4 -- normalization would be lossy.
    NotWordAligned { len: usize },
}

impl fmt::Display for RomRejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RomRejectReason::TooSmall { len } => {
                write!(
                    f,
                    "input is {len} bytes, smaller than a valid N64 ROM header"
                )
            }
            RomRejectReason::UnknownMagic { first_word } => {
                write!(
                    f,
                    "first word 0x{first_word:08x} matches no known z64/n64/v64 magic"
                )
            }
            RomRejectReason::NotWordAligned { len } => {
                write!(
                    f,
                    "input is {len} bytes, not a multiple of 4 -- cannot normalize byte order"
                )
            }
        }
    }
}

impl std::error::Error for RomRejectReason {}

/// Parsed fixed-layout fields of the 0x40-byte N64 ROM header. Field
/// offsets and widths are fixed hardware/bootrom facts (libultra's
/// `bootcode.s` / any N64 hardware reference), not discovered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RomHeader {
    /// PI BSD domain 1 config word at header offset 0x00.
    pub pi_bsd_dom1_config: u32,
    /// Clock rate override at 0x04.
    pub clock_rate: u32,
    /// Entrypoint VA the boot process jumps to after the IPL3 stage copies
    /// the first 0x100000 ROM bytes to this VA's physical RDRAM alias, at
    /// header offset 0x08.
    pub entry_point: u32,
    /// Libultra revision at 0x0c.
    pub libultra_version: u32,
    /// CRC1/CRC2 at 0x10/0x14, used by IPL3 to select a CIC-compatible boot
    /// path; not re-derived here, just recorded.
    pub crc1: u32,
    pub crc2: u32,
    /// ASCII cartridge name, 0x20..0x34, NUL/space padded.
    pub name: String,
    /// Cartridge ID / region bytes, 0x3b..0x3f (media format, id, region,
    /// version).
    pub cartridge_id: [u8; 4],
}

/// A ROM normalized to canonical big-endian bytes plus its identities.
/// Every fact, bank, and function record downstream binds to `digest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedRom {
    pub source_byte_order: RomByteOrder,
    pub header: RomHeader,
    /// SHA-256 of the normalized (big-endian) bytes, hex-encoded. Primary
    /// cache/identity key.
    pub sha256: String,
    pub sha1: String,
    pub md5: String,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

impl NormalizedRom {
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Read a big-endian u32 at a normalized-ROM byte offset. Returns
    /// `None` rather than panicking on out-of-range reads -- callers in a
    /// discovery pipeline routinely probe speculative offsets, and an
    /// out-of-range probe is an ordinary "no evidence here," not a bug.
    pub fn read_u32(&self, offset: usize) -> Option<u32> {
        let bytes = self.bytes.get(offset..offset + 4)?;
        Some(u32::from_be_bytes(bytes.try_into().ok()?))
    }
}

fn detect_byte_order(first_word: u32) -> Option<RomByteOrder> {
    match first_word {
        MAGIC_Z64 => Some(RomByteOrder::Z64),
        MAGIC_N64 => Some(RomByteOrder::N64),
        MAGIC_V64 => Some(RomByteOrder::V64),
        _ => None,
    }
}

/// Convert arbitrary input bytes to canonical big-endian order in place,
/// given the byte order that was detected from the header magic.
fn to_big_endian(input: &[u8], order: RomByteOrder) -> Vec<u8> {
    match order {
        RomByteOrder::Z64 => input.to_vec(),
        RomByteOrder::N64 => {
            let mut out = Vec::with_capacity(input.len());
            for word in input.chunks_exact(4) {
                out.extend_from_slice(&[word[3], word[2], word[1], word[0]]);
            }
            out
        }
        RomByteOrder::V64 => {
            let mut out = Vec::with_capacity(input.len());
            for pair in input.chunks_exact(2) {
                out.extend_from_slice(&[pair[1], pair[0]]);
            }
            out
        }
    }
}

fn parse_header(be: &[u8]) -> RomHeader {
    let read_u32 = |off: usize| u32::from_be_bytes(be[off..off + 4].try_into().unwrap());
    let name_bytes = &be[0x20..0x34];
    let name = String::from_utf8_lossy(name_bytes)
        .trim_end_matches(['\0', ' '])
        .to_string();
    let cartridge_id = [be[0x3b], be[0x3c], be[0x3d], be[0x3e]];
    RomHeader {
        pi_bsd_dom1_config: read_u32(0x00),
        clock_rate: read_u32(0x04),
        entry_point: read_u32(0x08),
        libultra_version: read_u32(0x0c),
        crc1: read_u32(0x10),
        crc2: read_u32(0x14),
        name,
        cartridge_id,
    }
}

/// Phase 1 entry point: detect byte order, convert to big-endian, parse the
/// header, and compute all three digests. Rejects malformed input before
/// any analysis begins, per docs/DISCOVER-DESIGN.md's "reject malformed or
/// unexpected inputs before analysis."
pub fn normalize(input: &[u8]) -> Result<NormalizedRom, RomRejectReason> {
    const HEADER_LEN: usize = 0x40;
    if input.len() < HEADER_LEN {
        return Err(RomRejectReason::TooSmall { len: input.len() });
    }
    if !input.len().is_multiple_of(4) {
        return Err(RomRejectReason::NotWordAligned { len: input.len() });
    }
    let first_word = u32::from_be_bytes(input[0..4].try_into().unwrap());
    let order =
        detect_byte_order(first_word).ok_or(RomRejectReason::UnknownMagic { first_word })?;

    let bytes = to_big_endian(input, order);
    let header = parse_header(&bytes[..HEADER_LEN]);

    use sha2::Digest as _;
    let sha256 = {
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        hex_encode(&hasher.finalize())
    };
    let sha1 = {
        use sha1::Digest as _;
        let mut hasher = sha1::Sha1::new();
        hasher.update(&bytes);
        hex_encode(&hasher.finalize())
    };
    let md5 = {
        use md5::Digest as _;
        let mut hasher = md5::Md5::new();
        hasher.update(&bytes);
        hex_encode(&hasher.finalize())
    };

    Ok(NormalizedRom {
        source_byte_order: order,
        header,
        sha256,
        sha1,
        md5,
        bytes,
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    use fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").unwrap();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_z64_header(entry: u32, name: &str, cart_id: [u8; 4]) -> Vec<u8> {
        let mut buf = vec![0u8; 0x1000];
        buf[0..4].copy_from_slice(&MAGIC_Z64.to_be_bytes());
        buf[4..8].copy_from_slice(&0x0000_000fu32.to_be_bytes());
        buf[8..12].copy_from_slice(&entry.to_be_bytes());
        buf[12..16].copy_from_slice(&0u32.to_be_bytes());
        buf[0x10..0x14].copy_from_slice(&0xdead_beefu32.to_be_bytes());
        buf[0x14..0x18].copy_from_slice(&0xcafe_babeu32.to_be_bytes());
        let name_bytes = name.as_bytes();
        buf[0x20..0x20 + name_bytes.len()].copy_from_slice(name_bytes);
        buf[0x3b..0x3f].copy_from_slice(&cart_id);
        buf
    }

    #[test]
    fn normalizes_z64_without_byte_swap() {
        let raw = make_z64_header(0x8000_0400, "TEST GAME", *b"CTSE");
        let rom = normalize(&raw).expect("valid z64");
        assert_eq!(rom.source_byte_order, RomByteOrder::Z64);
        assert_eq!(rom.header.entry_point, 0x8000_0400);
        assert_eq!(rom.header.name, "TEST GAME");
        assert_eq!(rom.header.cartridge_id, *b"CTSE");
        assert_eq!(rom.bytes, raw);
    }

    #[test]
    fn normalizes_n64_byte_swapped_words() {
        let z64 = make_z64_header(0x8000_0400, "TEST GAME", *b"CTSE");
        let mut n64 = Vec::with_capacity(z64.len());
        for word in z64.chunks_exact(4) {
            n64.extend_from_slice(&[word[3], word[2], word[1], word[0]]);
        }
        let rom = normalize(&n64).expect("valid n64");
        assert_eq!(rom.source_byte_order, RomByteOrder::N64);
        assert_eq!(
            rom.bytes, z64,
            "round-trips back to canonical big-endian bytes"
        );
        assert_eq!(rom.header.entry_point, 0x8000_0400);
    }

    #[test]
    fn normalizes_v64_halfword_swapped() {
        let z64 = make_z64_header(0x8000_0400, "TEST GAME", *b"CTSE");
        let mut v64 = Vec::with_capacity(z64.len());
        for pair in z64.chunks_exact(2) {
            v64.extend_from_slice(&[pair[1], pair[0]]);
        }
        let rom = normalize(&v64).expect("valid v64");
        assert_eq!(rom.source_byte_order, RomByteOrder::V64);
        assert_eq!(rom.bytes, z64);
    }

    #[test]
    fn rejects_too_small() {
        let err = normalize(&[0u8; 8]).unwrap_err();
        assert_eq!(err, RomRejectReason::TooSmall { len: 8 });
    }

    #[test]
    fn rejects_unaligned_length() {
        let mut raw = make_z64_header(0x8000_0400, "X", *b"CTSE");
        raw.push(0);
        let err = normalize(&raw).unwrap_err();
        assert!(matches!(err, RomRejectReason::NotWordAligned { .. }));
    }

    #[test]
    fn rejects_unknown_magic() {
        let raw = vec![0u8; 0x1000];
        let err = normalize(&raw).unwrap_err();
        assert!(matches!(
            err,
            RomRejectReason::UnknownMagic { first_word: 0 }
        ));
    }

    #[test]
    fn digests_are_deterministic_and_distinct_per_algorithm() {
        let raw = make_z64_header(0x8000_0400, "TEST GAME", *b"CTSE");
        let rom_a = normalize(&raw).unwrap();
        let rom_b = normalize(&raw).unwrap();
        assert_eq!(rom_a.sha256, rom_b.sha256, "sha256 is deterministic");
        assert_eq!(rom_a.sha1, rom_b.sha1, "sha1 is deterministic");
        assert_eq!(rom_a.md5, rom_b.md5, "md5 is deterministic");
        assert_ne!(rom_a.sha256, rom_a.sha1);
        assert_ne!(rom_a.sha1, rom_a.md5);
    }
}
