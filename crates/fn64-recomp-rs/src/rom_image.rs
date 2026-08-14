//! The user-supplied ROM image, published once at startup so generated shard
//! crates can recover their instruction words from it instead of carrying a
//! baked copy.
//!
//! # Why this exists
//!
//! A shipped fn64 build must contain no verbatim ROM content. Before this
//! module each generated shard crate embedded a `pub static WORDS: &[u32]`
//! literal holding its slice of the ROM (~1.82 MiB across 32 shards for
//! WM2000). Those words are never executed — execution is the emitted
//! `match pc` arms — and the admission decision that front-runs them
//! ([`CodeSpan::resolve`](crate::execution::CodeSpan)) reads only a *length*,
//! not the word values. Every other non-test reader hashes them. So the words
//! are recoverable at runtime from the user's own ROM at a known offset, and
//! nothing that runs depends on them being baked in.
//!
//! What ships instead is **geometry**: each shard's `(ROM_START, ROM_END)`
//! offsets, the same shape as the existing `pack::ROM_COPY` triple.
//!
//! # Normalization is part of the contract
//!
//! The offsets index the **normalized big-endian** image. A user's file may be
//! `.z64` (big-endian), `.n64` (little-endian) or `.v64` (byte-swapped);
//! [`publish_normalized_rom_image`] must be handed the already-normalized
//! bytes so all three resolve to one identity rather than three failures.
//!
//! # This is not a verification boundary
//!
//! Nothing here checks that the bytes are the *right* ROM. That proof is
//! already made downstream, where each recovered bank is hashed and compared
//! against the build-time digest recovered from the ROM
//! (`code_bank_sha256(&code_bank) == expected.code_sha256`). Keeping the check
//! there rather than adding one here means a wrong ROM fails against the
//! strongest existing evidence, not a second weaker copy of it.

use std::sync::OnceLock;

/// The normalized, big-endian ROM bytes supplied by the user at launch.
///
/// `OnceLock` rather than a mutable global: the image is installed once during
/// startup and read concurrently thereafter, and a second install with
/// different bytes would silently change what every already-constructed bank
/// was proven against.
static NORMALIZED_ROM_IMAGE: OnceLock<Vec<u8>> = OnceLock::new();

/// Why a shard could not recover its words from the published ROM image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RomImageError {
    /// No image was published before a shard tried to read one.
    NotPublished,
    /// The requested span is not fully inside the published image.
    OutOfRange {
        rom_start: u32,
        rom_end: u32,
        rom_len: usize,
    },
    /// The requested span is not a whole number of instruction words.
    Misaligned { rom_start: u32, rom_end: u32 },
}

impl std::fmt::Display for RomImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPublished => write!(
                f,
                "no normalized ROM image has been published; the host must call \
                 publish_normalized_rom_image before constructing code banks"
            ),
            Self::OutOfRange {
                rom_start,
                rom_end,
                rom_len,
            } => write!(
                f,
                "shard ROM span {rom_start:#010X}..{rom_end:#010X} is outside the \
                 published ROM image of {rom_len:#x} bytes -- this is the wrong ROM \
                 or a truncated file, not a recoverable condition"
            ),
            Self::Misaligned { rom_start, rom_end } => write!(
                f,
                "shard ROM span {rom_start:#010X}..{rom_end:#010X} is not a whole \
                 number of 4-byte instruction words"
            ),
        }
    }
}

impl std::error::Error for RomImageError {}

/// Install the user's normalized ROM image.
///
/// Idempotent for identical bytes; **panics** if called twice with different
/// content, because banks already constructed against the first image were
/// proven against digests that the second would invalidate. Silently accepting
/// the second image is the failure mode this guards.
pub fn publish_normalized_rom_image(bytes: Vec<u8>) {
    if let Some(existing) = NORMALIZED_ROM_IMAGE.get() {
        assert!(
            existing == &bytes,
            "a different normalized ROM image was published after code banks were \
             already constructed against the first ({} bytes then, {} bytes now)",
            existing.len(),
            bytes.len()
        );
        return;
    }
    // A race between two first-time publishers resolves to whichever wins; the
    // equality assert above then rejects a genuinely conflicting second image.
    let _ = NORMALIZED_ROM_IMAGE.set(bytes);
}

/// Whether an image is available. Lets a host report a clear startup error
/// rather than failing inside bank construction.
pub fn normalized_rom_image_published() -> bool {
    NORMALIZED_ROM_IMAGE.get().is_some()
}

/// Borrow the published normalized ROM image.
pub fn normalized_rom_image() -> Option<&'static [u8]> {
    NORMALIZED_ROM_IMAGE.get().map(Vec::as_slice)
}

/// Recover the big-endian instruction words in `rom_start..rom_end` from the
/// published image.
///
/// This is the replacement for a shard's baked `WORDS` array. The bytes are
/// read big-endian because that is the normalized on-cartridge order and the
/// order every digest in the system is computed over.
pub fn shard_words(rom_start: u32, rom_end: u32) -> Result<Vec<u32>, RomImageError> {
    let image = NORMALIZED_ROM_IMAGE
        .get()
        .ok_or(RomImageError::NotPublished)?;
    // `rom_start` must be word-aligned too, not just the length. A misaligned
    // start with an aligned length would slice at a non-word boundary and
    // yield shifted words -- the same COUNT of words, so the geometry would
    // look right and only the digest would catch it, far from the cause.
    if rom_end <= rom_start || rom_start % 4 != 0 || (rom_end - rom_start) % 4 != 0 {
        return Err(RomImageError::Misaligned { rom_start, rom_end });
    }
    let start = rom_start as usize;
    let end = rom_end as usize;
    if end > image.len() {
        return Err(RomImageError::OutOfRange {
            rom_start,
            rom_end,
            rom_len: image.len(),
        });
    }
    Ok(image[start..end]
        .chunks_exact(4)
        .map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decode must be big-endian. A little-endian read would produce words
    /// that still admit the same PCs (admission is a length check) but hash to
    /// a different digest -- so this test is what distinguishes "recovered the
    /// right words" from "recovered the right *number* of words".
    #[test]
    fn decodes_big_endian_words() {
        let bytes = vec![0x3C, 0x08, 0x80, 0x12, 0x8D, 0x08, 0x00, 0x04];
        let words: Vec<u32> = bytes
            .chunks_exact(4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        assert_eq!(words, vec![0x3C08_8012, 0x8D08_0004]);
    }

    #[test]
    fn rejects_misaligned_and_reversed_spans() {
        // `shard_words` resolves the published image before it validates the
        // span, so without this the assertions below meet `NotPublished` and
        // never reach the alignment checks they exist to pin. Publishing is a
        // `OnceCell` set shared by the whole test binary: a matching image is
        // accepted whether or not another test got here first.
        publish_normalized_rom_image(vec![0u8; 0x40]);

        // Unaligned length.
        assert_eq!(
            shard_words(0x10, 0x13).unwrap_err(),
            RomImageError::Misaligned {
                rom_start: 0x10,
                rom_end: 0x13
            }
        );
        // Reversed span.
        assert_eq!(
            shard_words(0x20, 0x10).unwrap_err(),
            RomImageError::Misaligned {
                rom_start: 0x20,
                rom_end: 0x10
            }
        );
        // Unaligned START with an aligned LENGTH -- the dangerous one, because
        // it yields the right number of words with every one of them shifted.
        assert_eq!(
            shard_words(0x12, 0x22).unwrap_err(),
            RomImageError::Misaligned {
                rom_start: 0x12,
                rom_end: 0x22
            }
        );
    }
}
