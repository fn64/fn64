use crate::native_contract::DeviceRgba16Bytes;

use super::{
    CandidateColorTarget, ColorTargetFormat, ColorTargetKey, TargetError, TargetGeneration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba8 {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl Rgba8 {
    pub const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeviceColorBytes {
    pub(super) key: ColorTargetKey,
    pub(super) generation: TargetGeneration,
    pub(super) format: ColorTargetFormat,
    pub(super) bytes: Box<[u8]>,
}

impl DeviceColorBytes {
    pub const fn key(&self) -> ColorTargetKey {
        self.key
    }

    pub const fn generation(&self) -> TargetGeneration {
        self.generation
    }

    pub const fn format(&self) -> ColorTargetFormat {
        self.format
    }

    pub fn device_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_device_bytes(self) -> Box<[u8]> {
        self.bytes
    }

    /// Constructs a full-extent device-byte buffer from an already fill-
    /// executed row patch. The M4.3.4 fill executor's own production
    /// constructor -- `bytes` must already be the target's complete,
    /// full-extent byte content (patched with the newly-filled sub-
    /// rectangle where applicable), matching `pack_device_pixels`'s
    /// invariant for every other producer of this type.
    pub(crate) fn new_for_fill(
        key: ColorTargetKey,
        generation: TargetGeneration,
        format: ColorTargetFormat,
        bytes: Vec<u8>,
    ) -> Result<Self, TargetError> {
        let expected = (key.extent().pixels() as usize)
            .checked_mul(format.bytes_per_pixel() as usize)
            .ok_or(TargetError::PixelBufferLengthOverflow {
                pixels: key.extent().pixels() as usize,
                bytes_per_pixel: format.bytes_per_pixel(),
            })?;
        if bytes.len() != expected {
            return Err(TargetError::CompletedByteLengthMismatch {
                key,
                generation,
                expected,
                actual: bytes.len(),
            });
        }
        Ok(Self {
            key,
            generation,
            format,
            bytes: bytes.into_boxed_slice(),
        })
    }

    /// M3.3a's logical/device-order domain is narrower than this module's
    /// RGBA16/32 oracle. Keep the narrowing explicit at the integration seam.
    #[allow(dead_code)] // Reserved for the M3.3a native-output integration seam.
    pub(crate) fn into_m3_3a_rgba16(self) -> Result<DeviceRgba16Bytes, TargetError> {
        if self.format != ColorTargetFormat::Rgba16 {
            return Err(TargetError::DeviceDomainMismatch {
                expected: ColorTargetFormat::Rgba16,
                actual: self.format,
            });
        }
        Ok(DeviceRgba16Bytes::from_device_bytes(self.bytes.into_vec()))
    }
}

/// Packs the oracle's color-only pixels with full destination coverage.
/// RGBA16 bit 0 is stored coverage bit 2 (Programming Manual §§15.5.3,
/// 15.5.6, 15.7), so primitive alpha never selects it. This generic oracle
/// has no render-mode or sample-mask input and therefore makes only the
/// explicit full-coverage fixture claim, not a geometric-coverage claim.
pub fn pack_device_pixels(
    candidate: &CandidateColorTarget,
    pixels: &[Rgba8],
) -> Result<DeviceColorBytes, TargetError> {
    let key = candidate.key();
    let format = key.format();
    let expected_pixels = key.extent().pixels() as usize;
    if pixels.len() != expected_pixels {
        return Err(TargetError::PixelCountMismatch {
            key,
            expected: expected_pixels,
            actual: pixels.len(),
        });
    }
    let byte_count = pixels
        .len()
        .checked_mul(format.bytes_per_pixel() as usize)
        .ok_or(TargetError::PixelBufferLengthOverflow {
            pixels: pixels.len(),
            bytes_per_pixel: format.bytes_per_pixel(),
        })?;
    let mut bytes = Vec::with_capacity(byte_count);
    match format {
        ColorTargetFormat::Rgba16 => {
            for pixel in pixels {
                let packed = (u16::from(pixel.red >> 3) << 11)
                    | (u16::from(pixel.green >> 3) << 6)
                    | (u16::from(pixel.blue >> 3) << 1)
                    | 1;
                bytes.extend_from_slice(&packed.to_be_bytes());
            }
        }
        ColorTargetFormat::Rgba32 => {
            for pixel in pixels {
                bytes.extend_from_slice(&[pixel.red, pixel.green, pixel.blue, pixel.alpha]);
            }
        }
    }
    Ok(DeviceColorBytes {
        key,
        generation: candidate.generation(),
        format,
        bytes: bytes.into_boxed_slice(),
    })
}

pub fn unpack_device_pixels(
    format: ColorTargetFormat,
    bytes: &[u8],
) -> Result<Box<[Rgba8]>, TargetError> {
    let bytes_per_pixel = format.bytes_per_pixel() as usize;
    if !bytes.len().is_multiple_of(bytes_per_pixel) {
        return Err(TargetError::PixelByteLength {
            format,
            actual: bytes.len(),
            required_multiple: bytes_per_pixel,
        });
    }

    let mut pixels = Vec::with_capacity(bytes.len() / bytes_per_pixel);
    match format {
        ColorTargetFormat::Rgba16 => {
            for bytes in bytes.chunks_exact(2) {
                let packed = u16::from_be_bytes([bytes[0], bytes[1]]);
                pixels.push(Rgba8 {
                    red: expand_five(((packed >> 11) & 0x1f) as u8),
                    green: expand_five(((packed >> 6) & 0x1f) as u8),
                    blue: expand_five(((packed >> 1) & 0x1f) as u8),
                    alpha: if packed & 1 == 0 { 0 } else { u8::MAX },
                });
            }
        }
        ColorTargetFormat::Rgba32 => {
            for bytes in bytes.chunks_exact(4) {
                pixels.push(Rgba8::new(bytes[0], bytes[1], bytes[2], bytes[3]));
            }
        }
    }
    Ok(pixels.into_boxed_slice())
}

const fn expand_five(value: u8) -> u8 {
    (value << 3) | (value >> 2)
}
