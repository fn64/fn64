//! A from-scratch, dependency-free PNG encoder for dumping a rendered
//! `Framebuffer` to disk. Deliberately minimal: 8-bit RGBA, no filtering
//! (filter type 0 per scanline, valid per the PNG spec), and "stored"
//! (uncompressed) DEFLATE blocks per RFC 1951 section 3.2.4 -- a
//! byte-for-byte-legal zlib stream that just doesn't bother compressing.
//! This avoids adding an external PNG/deflate crate dependency for what is,
//! for this seam-proof deliverable, a small one-off image.
use std::io::Write;

fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// Wrap `raw` in a minimal valid zlib stream using uncompressed ("stored")
/// DEFLATE blocks (RFC 1951 3.2.4). Each block carries up to 65535 bytes.
fn zlib_store(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 8);
    out.push(0x78); // CMF: deflate, 32K window
    out.push(0x01); // FLG: no dict, fastest (checksum bits valid for CMF/FLG pair)

    let mut i = 0;
    if raw.is_empty() {
        out.push(0x01); // final empty stored block
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    }
    while i < raw.len() {
        let remaining = raw.len() - i;
        let chunk_len = remaining.min(65535);
        let is_final = i + chunk_len >= raw.len();
        out.push(if is_final { 0x01 } else { 0x00 });
        let len = chunk_len as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(&raw[i..i + chunk_len]);
        i += chunk_len;
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    // Standard PNG/zlib CRC-32 (polynomial 0xEDB88320), textbook table-free
    // bit-at-a-time implementation -- fine for the small one-off images
    // this module produces.
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn write_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(tag);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Encode an RGBA8888 `pixels` buffer (row-major, `width*height*4` bytes,
/// as produced by `Framebuffer`) as a PNG file's bytes.
pub fn encode_rgba8(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    assert_eq!(pixels.len(), (width as usize) * (height as usize) * 4);

    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // color type 6 = RGBA
    ihdr.push(0); // compression method
    ihdr.push(0); // filter method
    ihdr.push(0); // interlace method
    write_chunk(&mut out, b"IHDR", &ihdr);

    // Raw scanlines, each prefixed with filter-type byte 0 (none).
    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0u8);
        raw.extend_from_slice(&pixels[row * stride..(row + 1) * stride]);
    }
    let compressed = zlib_store(&raw);
    write_chunk(&mut out, b"IDAT", &compressed);
    write_chunk(&mut out, b"IEND", &[]);
    out
}

/// Encode + write a PNG file to `path`.
pub fn write_png(
    path: &std::path::Path,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> std::io::Result<()> {
    let bytes = encode_rgba8(width, height, pixels);
    let mut f = std::fs::File::create(path)?;
    f.write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_png_has_valid_signature_and_ihdr() {
        let pixels = vec![255u8; 2 * 2 * 4];
        let bytes = encode_rgba8(2, 2, &pixels);
        assert_eq!(
            &bytes[0..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );
        // IHDR chunk follows immediately: length(4) + "IHDR"(4) + 13 bytes data + crc(4)
        assert_eq!(&bytes[12..16], b"IHDR");
        let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        assert_eq!((w, h), (2, 2));
    }

    #[test]
    fn round_trips_through_a_real_png_decoder_expectation_crc_is_self_consistent() {
        // We don't have a decoder crate available (deliberately dependency-
        // free), so verify internal self-consistency instead: every chunk's
        // stored CRC matches a freshly recomputed CRC over its tag+data.
        let pixels: Vec<u8> = (0..(4 * 3 * 4)).map(|i| (i % 256) as u8).collect();
        let bytes = encode_rgba8(4, 3, &pixels);
        let mut pos = 8usize;
        while pos + 8 <= bytes.len() {
            let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
            let tag = &bytes[pos + 4..pos + 8];
            let data_start = pos + 8;
            let data_end = data_start + len;
            let stored_crc = u32::from_be_bytes(bytes[data_end..data_end + 4].try_into().unwrap());
            let recomputed = crc32(&bytes[pos + 4..data_end]);
            assert_eq!(stored_crc, recomputed, "chunk {:?} CRC mismatch", tag);
            if tag == b"IEND" {
                break;
            }
            pos = data_end + 4;
        }
    }
}
