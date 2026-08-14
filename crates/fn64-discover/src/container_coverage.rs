//! Which compression containers a ROM uses, and how much of it they cover.
//!
//! `ledger.rs` already scans for `Yaz0`, `MIO0` and gzip magics, but
//! `has_container_magic` collapses the answer to a bool -- which scheme, and
//! how much of the ROM sits behind it, are both discarded. Only Yaz0
//! (`banks/mod.rs`) and headered raw deflate (`headered_raw_deflate.rs`)
//! actually decode.
//!
//! That gap matters for the same reason code-span locality did: it separates
//! "nothing is there" from "we cannot read what is there". A ROM that is
//! largely MIO0, with no MIO0 decoder in tree, is a KNOWN limitation rather
//! than a mystery -- and knowing that stops it being re-investigated as if the
//! search were at fault.
//!
//! Deliberately a scan for stream STARTS, not a decode: it counts headers and
//! the bytes they declare, never inflating anything. So this is cheap, and its
//! byte totals are what the containers CLAIM rather than verified payloads.

use serde::{Deserialize, Serialize};

/// Containers fn64 recognizes, and whether this build can decode each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerScheme {
    /// Nintendo Yaz0 RLE. Decoded by fn64 (`banks::decode_yaz0`).
    Yaz0,
    /// Nintendo MIO0. NOT decoded by fn64; recognized only.
    Mio0,
    /// gzip/DEFLATE stream. Headered raw deflate is decoded
    /// (`headered_raw_deflate`); a bare gzip member is not.
    Gzip,
}

impl ContainerScheme {
    pub fn magic(self) -> &'static [u8] {
        match self {
            Self::Yaz0 => b"Yaz0",
            Self::Mio0 => b"MIO0",
            Self::Gzip => &[0x1f, 0x8b],
        }
    }

    /// Whether this build can turn such a stream back into bytes. A stream
    /// this returns `false` for is a recognized, bounded limitation.
    pub fn is_decodable_here(self) -> bool {
        match self {
            Self::Yaz0 => true,
            // No MIO0 decoder exists in this workspace.
            Self::Mio0 => false,
            // Only the headered raw-deflate variant decodes, not bare gzip.
            Self::Gzip => false,
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::Yaz0, Self::Mio0, Self::Gzip]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerStreamCounts {
    pub scheme: ContainerScheme,
    /// Streams whose magic appears at a 4-byte-aligned offset.
    pub stream_count: usize,
    /// Sum of DECLARED decompressed sizes, for schemes that carry one in a
    /// fixed header field (Yaz0 and MIO0 both do, at offset 4). Zero for gzip,
    /// whose size field is at the END of a member and cannot be read without
    /// walking the stream.
    pub declared_output_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerCoverage {
    pub streams: Vec<ContainerStreamCounts>,
    /// Total streams across every scheme.
    pub total_streams: usize,
    /// Streams whose scheme this build cannot decode.
    pub undecodable_streams: usize,
    pub rom_bytes: usize,
}

impl ContainerCoverage {
    /// Whether an unreadable container plausibly explains a thin discovery
    /// result. True when this build cannot decode any stream it found.
    ///
    /// Like code-span locality, this is a signal rather than a proof: a ROM
    /// can carry undecodable assets and still expose all its code plainly.
    pub fn has_undecodable_content(&self) -> bool {
        self.undecodable_streams > 0
    }

    pub fn scheme(&self, scheme: ContainerScheme) -> Option<&ContainerStreamCounts> {
        self.streams.iter().find(|entry| entry.scheme == scheme)
    }
}

/// Reject a magic match that cannot be a real stream header.
///
/// gzip's magic is only two bytes, which fires constantly on texture and audio
/// data: measured on WM2000, all 77 aligned `1f 8b` hits had a compression
/// method other than deflate, i.e. every one was a false positive. RFC 1952
/// fixes CM=8 for every gzip member in practice, so requiring it removes them
/// without excluding any real stream.
///
/// Yaz0 and MIO0 carry four-byte ASCII magics and a declared output size; a
/// zero or absurd size is the analogous tell.
fn plausible_stream(scheme: ContainerScheme, rom_bytes: &[u8], offset: usize) -> bool {
    match scheme {
        ContainerScheme::Gzip => rom_bytes.get(offset + 2) == Some(&8),
        ContainerScheme::Yaz0 | ContainerScheme::Mio0 => {
            let declared = u32::from_be_bytes([
                rom_bytes[offset + 4],
                rom_bytes[offset + 5],
                rom_bytes[offset + 6],
                rom_bytes[offset + 7],
            ]);
            // A stream that declares nothing, or more than 64 MiB (larger than
            // any N64 ROM), is not a stream.
            declared > 0 && declared <= 64 * 1024 * 1024
        }
    }
}

/// Scan normalized (big-endian) ROM bytes for container stream headers.
///
/// Only 4-byte-aligned offsets are considered. N64 tooling places these
/// containers at aligned offsets, and scanning every byte would turn ordinary
/// data that happens to contain `MIO0` into a stream.
pub fn measure_container_coverage(rom_bytes: &[u8]) -> ContainerCoverage {
    let mut streams = Vec::new();
    let mut total_streams = 0usize;
    let mut undecodable_streams = 0usize;

    for scheme in ContainerScheme::all() {
        let magic = scheme.magic();
        let mut stream_count = 0usize;
        let mut declared_output_bytes = 0u64;
        let mut offset = 0usize;
        while offset + 8 <= rom_bytes.len() {
            if rom_bytes[offset..].starts_with(magic) && plausible_stream(scheme, rom_bytes, offset)
            {
                stream_count += 1;
                // Yaz0 and MIO0 both declare the decompressed size as a
                // big-endian u32 at +4. gzip does not, so it is left at zero
                // rather than reading an unrelated word.
                if matches!(scheme, ContainerScheme::Yaz0 | ContainerScheme::Mio0) {
                    let declared = u32::from_be_bytes([
                        rom_bytes[offset + 4],
                        rom_bytes[offset + 5],
                        rom_bytes[offset + 6],
                        rom_bytes[offset + 7],
                    ]);
                    declared_output_bytes += u64::from(declared);
                }
            }
            offset += 4;
        }
        if stream_count > 0 {
            total_streams += stream_count;
            if !scheme.is_decodable_here() {
                undecodable_streams += stream_count;
            }
            streams.push(ContainerStreamCounts {
                scheme,
                stream_count,
                declared_output_bytes,
            });
        }
    }

    ContainerCoverage {
        streams,
        total_streams,
        undecodable_streams,
        rom_bytes: rom_bytes.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom_with(streams: &[(ContainerScheme, usize, u32)], len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        for &(scheme, offset, declared) in streams {
            let magic = scheme.magic();
            bytes[offset..offset + magic.len()].copy_from_slice(magic);
            bytes[offset + 4..offset + 8].copy_from_slice(&declared.to_be_bytes());
        }
        bytes
    }

    #[test]
    fn a_rom_with_no_containers_reports_nothing_undecodable() {
        let coverage = measure_container_coverage(&vec![0u8; 0x1000]);
        assert_eq!(coverage.total_streams, 0);
        assert!(!coverage.has_undecodable_content());
    }

    #[test]
    fn yaz0_is_counted_and_is_decodable() {
        let rom = rom_with(&[(ContainerScheme::Yaz0, 0x100, 0x2000)], 0x1000000);
        let coverage = measure_container_coverage(&rom);
        let yaz0 = coverage.scheme(ContainerScheme::Yaz0).unwrap();
        assert_eq!(yaz0.stream_count, 1);
        assert_eq!(yaz0.declared_output_bytes, 0x2000);
        // Yaz0 decodes here, so it is not a limitation.
        assert!(!coverage.has_undecodable_content());
    }

    #[test]
    fn mio0_is_recognized_but_flagged_undecodable() {
        // The case this exists for: content is present and unreadable, which
        // is a KNOWN bound rather than a discovery failure.
        let rom = rom_with(
            &[
                (ContainerScheme::Mio0, 0x100, 0x800),
                (ContainerScheme::Mio0, 0x200, 0x800),
            ],
            0x1000000,
        );
        let coverage = measure_container_coverage(&rom);
        let mio0 = coverage.scheme(ContainerScheme::Mio0).unwrap();
        assert_eq!(mio0.stream_count, 2);
        assert_eq!(mio0.declared_output_bytes, 0x1000);
        assert_eq!(coverage.undecodable_streams, 2);
        assert!(coverage.has_undecodable_content());
    }

    #[test]
    fn unaligned_magic_is_not_counted() {
        let mut rom = vec![0u8; 0x10000];
        rom[0x102..0x106].copy_from_slice(b"MIO0");
        let coverage = measure_container_coverage(&rom);
        assert_eq!(
            coverage.total_streams, 0,
            "an unaligned magic must not be read as a stream"
        );
    }

    #[test]
    fn a_gzip_magic_without_deflate_is_rejected() {
        // Measured on WM2000: all 77 aligned `1f 8b` hits carried a
        // compression method other than 8, i.e. every one was texture data,
        // not a stream. Without this check the probe reported 77 gzip streams.
        let mut rom = vec![0u8; 0x10000];
        rom[0x100] = 0x1f;
        rom[0x101] = 0x8b;
        rom[0x102] = 31; // not deflate
        let coverage = measure_container_coverage(&rom);
        assert_eq!(coverage.total_streams, 0);
    }

    #[test]
    fn a_declared_size_of_zero_is_rejected() {
        // rom_with writes declared=0, which no real stream carries.
        let rom = rom_with(&[(ContainerScheme::Mio0, 0x100, 0)], 0x10000);
        assert_eq!(measure_container_coverage(&rom).total_streams, 0);
    }

    #[test]
    fn gzip_declares_no_output_size() {
        let mut rom = vec![0u8; 0x10000];
        rom[0x100] = 0x1f;
        rom[0x101] = 0x8b;
        rom[0x102] = 8; // deflate, the only method that appears in practice
        let coverage = measure_container_coverage(&rom);
        let gzip = coverage.scheme(ContainerScheme::Gzip).unwrap();
        assert_eq!(gzip.stream_count, 1);
        // gzip's size lives at the END of the member; reading +4 would report
        // an unrelated word as a size.
        assert_eq!(gzip.declared_output_bytes, 0);
    }
}
