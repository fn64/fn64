//! Pure CPU-side byte projection of committed physical TMEM into the exact
//! layout the new fragment-callable WGSL sampling chain
//! (`shaders/tmem_sample.wgsl`) reads as a read-only storage buffer.
//!
//! This is a copy-and-encode adapter, not a resolver: it performs no
//! addressing, filtering, or format-decode arithmetic of any kind -- that
//! arithmetic is the WGSL port itself (published textured-triangle-draw card
//! §3, Option B, mandatory). [`PhysicalTmemState::valid_byte`]
//! (`tmem/physical.rs:160`) is the only public byte-level readout this
//! module uses; `PhysicalTmemState`'s private `bytes`/`valid` fields stay
//! untouched.
//!
//! **GPU-side validity-sentinel encoding (card §6, a new named choice this
//! slice must document):** a parallel bitmap, one bit per TMEM byte address,
//! packed 32 bits per `u32` (`128` words for `TMEM_BYTES = 4096` addresses),
//! set exactly when [`PhysicalTmemState::byte_is_valid`] is true for that
//! address. Chosen over a reserved-sentinel-value scheme (e.g. widening each
//! byte to a `u32` and reserving one bit pattern as "invalid") because a
//! sentinel value would either forbid a real byte value from ever equalling
//! it or require widening every stored byte to a larger type; a same-width
//! parallel bitmap instead mirrors this crate's own CPU-side `valid: Box<[bool;
//! TMEM_LEN]>` shadow array one-for-one, at 1/32 the size a per-byte `u32`
//! flag array would cost, and keeps the byte buffer itself exactly
//! `TMEM_BYTES` bytes wide (matching `PhysicalTmemState.bytes`'s own width,
//! so no repacking is needed on the Rust side beyond a direct copy).

use fn64_render_ir::TMEM_BYTES;

use crate::state::{ImageFormat, PixelSize};

use super::{PhysicalTmemState, TileDescriptor, TileSize};

const TMEM_LEN: usize = TMEM_BYTES as usize;
/// `TMEM_LEN` bits, packed 32 bits per word: `4096 / 32 == 128`.
pub const TMEM_VALIDITY_WORDS: usize = TMEM_LEN / 32;

/// `tmem_sample.wgsl`'s `TMEM_IMAGE_FORMAT_RGBA`/`TMEM_PIXEL_SIZE_BITS16`
/// wire codes, matching `shader_manifest.rs`'s existing test-side
/// `format_code`/`size_code` convention exactly (`ImageFormat::Rgba` = 0,
/// `PixelSize::Bits16` = 2) -- this module's own host-side encoder for the
/// same codes, since `format_code`/`size_code` are `#[cfg(test)]`-only and
/// this is production code.
const fn format_code(format: ImageFormat) -> u32 {
    match format {
        ImageFormat::Rgba => 0,
        ImageFormat::Yuv => 1,
        ImageFormat::ColorIndex => 2,
        ImageFormat::IntensityAlpha => 3,
        ImageFormat::Intensity => 4,
    }
}

const fn size_code(size: PixelSize) -> u32 {
    match size {
        PixelSize::Bits4 => 0,
        PixelSize::Bits8 => 1,
        PixelSize::Bits16 => 2,
        PixelSize::Bits32 => 3,
    }
}

/// Same wire codes as [`format_code`]/[`size_code`], for
/// `fn64_render`'s neutral wire-mirror enums (`NeutralImageFormat`/
/// `NeutralPixelSize`) instead of this crate's own `ImageFormat`/
/// `PixelSize` -- the two enums are 1:1 in variant meaning but distinct
/// Rust types, so each needs its own match arm.
const fn neutral_format_code(format: fn64_render::NeutralImageFormat) -> u32 {
    match format {
        fn64_render::NeutralImageFormat::Rgba => 0,
        fn64_render::NeutralImageFormat::Yuv => 1,
        fn64_render::NeutralImageFormat::ColorIndex => 2,
        fn64_render::NeutralImageFormat::IntensityAlpha => 3,
        fn64_render::NeutralImageFormat::Intensity => 4,
    }
}

const fn neutral_size_code(size: fn64_render::NeutralPixelSize) -> u32 {
    match size {
        fn64_render::NeutralPixelSize::Bits4 => 0,
        fn64_render::NeutralPixelSize::Bits8 => 1,
        fn64_render::NeutralPixelSize::Bits16 => 2,
        fn64_render::NeutralPixelSize::Bits32 => 3,
    }
}

/// Host-side mirror of `tmem_sample.wgsl`'s `TileBindingParams` uniform
/// struct, field-for-field, in the exact same declaration order (WGSL
/// uniform-buffer layout packs same-size scalar fields back-to-back with no
/// implicit padding, so declaration order here must match the shader's
/// struct exactly -- verified mechanically by
/// [`TileBindingParams::to_bytes`]'s own byte-offset test below). `bound`
/// is `false` when this triangle's tile index had no snapshotted
/// [`TileDescriptor`]/[`TileSize`] pair at its own stream position (card §6:
/// "missing `TileDescriptor`" is a named condition, propagated to WGSL as
/// `bound = 0`, never silently defaulted to some other tile's data) --
/// [`TileBindingParams::unbound`] is the only way to construct that case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileBindingParams {
    pub tmem_word_address: u32,
    pub line_words: u32,
    pub mask_s: u32,
    pub shift_s: u32,
    pub mode_s_mirror: u32,
    pub mode_s_clamp: u32,
    pub mask_t: u32,
    pub shift_t: u32,
    pub mode_t_mirror: u32,
    pub mode_t_clamp: u32,
    pub low_s: u32,
    pub low_t: u32,
    pub high_s: u32,
    pub high_t: u32,
    pub bound: u32,
    pub format: u32,
    pub pixel_size: u32,
    pub reserved_zero: u32,
}

/// Field count in `TileBindingParams` (and its WGSL twin) -- `4 *
/// TILE_BINDING_PARAMS_FIELDS` is the exact uniform-buffer byte size
/// `to_bytes` produces and `triangle_pipeline.rs`'s bind-group-layout entry
/// must request.
pub const TILE_BINDING_PARAMS_FIELDS: usize = 18;
pub const TILE_BINDING_PARAMS_BYTES: u64 = TILE_BINDING_PARAMS_FIELDS as u64 * 4;

impl TileBindingParams {
    /// The named "no binding was snapshotted" case (card §6): every
    /// arithmetic field is zero, `bound = 0`. `tmem_sample.wgsl`'s
    /// `sample_committed_rgba16_three_nearest` checks `bound` before any of
    /// the other fields are read, so their zero values are never
    /// interpreted as real tile-zero state.
    pub const fn unbound() -> Self {
        Self {
            tmem_word_address: 0,
            line_words: 0,
            mask_s: 0,
            shift_s: 0,
            mode_s_mirror: 0,
            mode_s_clamp: 0,
            mask_t: 0,
            shift_t: 0,
            mode_t_mirror: 0,
            mode_t_clamp: 0,
            low_s: 0,
            low_t: 0,
            high_s: 0,
            high_t: 0,
            bound: 0,
            format: 0,
            pixel_size: 0,
            reserved_zero: 0,
        }
    }

    /// Projects one real, snapshotted `(TileDescriptor, TileSize)` pair
    /// (card §2: "extend `PlanCollector`... to snapshot the current
    /// `TmemState` tile bindings onto each triangle") into this upload
    /// shape -- a pure field-by-field readout through each type's own
    /// public accessors, no addressing/filtering arithmetic (that stays in
    /// WGSL per §3).
    pub fn bound(descriptor: TileDescriptor, size: TileSize) -> Self {
        Self {
            tmem_word_address: u32::from(descriptor.tmem().get()),
            line_words: u32::from(descriptor.line_words()),
            mask_s: u32::from(descriptor.mask_s()),
            shift_s: u32::from(descriptor.shift_s()),
            mode_s_mirror: u32::from(descriptor.s_mode().mirror()),
            mode_s_clamp: u32::from(descriptor.s_mode().clamp()),
            mask_t: u32::from(descriptor.mask_t()),
            shift_t: u32::from(descriptor.shift_t()),
            mode_t_mirror: u32::from(descriptor.t_mode().mirror()),
            mode_t_clamp: u32::from(descriptor.t_mode().clamp()),
            low_s: u32::from(size.low_s().raw()),
            low_t: u32::from(size.low_t().raw()),
            high_s: u32::from(size.high_s().raw()),
            high_t: u32::from(size.high_t().raw()),
            bound: 1,
            format: format_code(descriptor.format()),
            pixel_size: size_code(descriptor.size()),
            reserved_zero: 0,
        }
    }

    /// Projects one `(NeutralTileDescriptor, NeutralTileSize)` pair --
    /// `fn64_render`'s neutral wire mirrors, the only shape
    /// `RawDpcSemanticCommandRef::State(RdpStateCommand::SetTile{..}/
    /// SetTileSize{..})` exposes to a plan-walking visitor -- into this
    /// upload shape. Field-for-field with [`Self::bound`]; kept as a
    /// separate constructor rather than routing through the typed
    /// `TileDescriptor`/`TileSize` (which would need a new neutral->typed
    /// converter this slice has no other use for) since every field this
    /// struct needs is already present on the neutral mirrors directly.
    pub fn from_neutral(
        descriptor: fn64_render::NeutralTileDescriptor,
        size: fn64_render::NeutralTileSize,
    ) -> Self {
        Self {
            tmem_word_address: u32::from(descriptor.tmem_word_address),
            line_words: u32::from(descriptor.line_words),
            mask_s: u32::from(descriptor.mask_s),
            shift_s: u32::from(descriptor.shift_s),
            mode_s_mirror: u32::from(descriptor.s_mode.mirror),
            mode_s_clamp: u32::from(descriptor.s_mode.clamp),
            mask_t: u32::from(descriptor.mask_t),
            shift_t: u32::from(descriptor.shift_t),
            mode_t_mirror: u32::from(descriptor.t_mode.mirror),
            mode_t_clamp: u32::from(descriptor.t_mode.clamp),
            low_s: u32::from(size.low_s),
            low_t: u32::from(size.low_t),
            high_s: u32::from(size.high_s),
            high_t: u32::from(size.high_t),
            bound: 1,
            format: neutral_format_code(descriptor.format),
            pixel_size: neutral_size_code(descriptor.size),
            reserved_zero: 0,
        }
    }

    /// Little-endian `u32`-per-field encoding, in the struct's own
    /// declaration order -- the exact byte layout `wgpu::Queue::write_buffer`
    /// uploads into the `TileBindingParams` uniform binding.
    pub fn to_bytes(self) -> [u8; TILE_BINDING_PARAMS_BYTES as usize] {
        let fields = [
            self.tmem_word_address,
            self.line_words,
            self.mask_s,
            self.shift_s,
            self.mode_s_mirror,
            self.mode_s_clamp,
            self.mask_t,
            self.shift_t,
            self.mode_t_mirror,
            self.mode_t_clamp,
            self.low_s,
            self.low_t,
            self.high_s,
            self.high_t,
            self.bound,
            self.format,
            self.pixel_size,
            self.reserved_zero,
        ];
        let mut bytes = [0u8; TILE_BINDING_PARAMS_BYTES as usize];
        for (chunk, field) in bytes.chunks_exact_mut(4).zip(fields) {
            chunk.copy_from_slice(&field.to_le_bytes());
        }
        bytes
    }
}

/// One committed physical-TMEM snapshot, byte-projected for GPU upload: the
/// full `TMEM_BYTES`-byte image (invalid addresses copied as-is -- their
/// content is never observable through [`PhysicalTmemState::valid_byte`], so
/// the projection's own validity bitmap is the only defined way a WGSL
/// caller may treat one as invalid, exactly the same rule
/// [`PhysicalTmemState::valid_byte`] already enforces on the CPU side) plus
/// the validity bitmap described in this module's own doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TmemGpuProjection {
    pub bytes: [u8; TMEM_LEN],
    pub validity_words: [u32; TMEM_VALIDITY_WORDS],
}

/// Projects `state`'s committed bytes and per-byte validity into the exact
/// layout [`TmemGpuProjection`] documents, reading every address through
/// [`PhysicalTmemState::valid_byte`] -- the same public readout
/// `read_committed_texel` itself is built on (`tmem/read.rs`'s
/// `read_valid_byte`) -- so an invalid address is never promoted into a
/// defined byte value by this projection either.
pub fn project_committed_tmem(state: &PhysicalTmemState) -> TmemGpuProjection {
    let mut bytes = [0u8; TMEM_LEN];
    let mut validity_words = [0u32; TMEM_VALIDITY_WORDS];
    for address in 0..TMEM_LEN {
        if let Some(byte) = state.valid_byte(address as u16) {
            bytes[address] = byte;
            validity_words[address / 32] |= 1 << (address % 32);
        }
    }
    TmemGpuProjection {
        bytes,
        validity_words,
    }
}

/// `TMEM_LEN` bytes packed 4-per-`u32`-word, little-endian (byte 0 in the
/// low 8 bits) -- `tmem_sample.wgsl`'s `tmem_read_byte` doc states this
/// exact packing convention as its own upload-layout decision; this
/// function is that decision's sole Rust-side producer. `4096 / 4 == 1024`
/// words, matching the shader's `array<u32, 1024>` binding.
pub const TMEM_BYTE_WORDS: usize = TMEM_LEN / 4;

impl TmemGpuProjection {
    pub fn byte_words(&self) -> [u32; TMEM_BYTE_WORDS] {
        let mut words = [0u32; TMEM_BYTE_WORDS];
        for (word, chunk) in words.iter_mut().zip(self.bytes.chunks_exact(4)) {
            *word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        words
    }

    /// Little-endian byte encoding of [`Self::byte_words`], the exact
    /// upload payload for the `tmem_bytes` storage-buffer binding.
    pub fn byte_words_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(TMEM_BYTE_WORDS * 4);
        for word in self.byte_words() {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    /// Little-endian byte encoding of `validity_words`, the exact upload
    /// payload for the `tmem_validity_words` storage-buffer binding.
    pub fn validity_words_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(TMEM_VALIDITY_WORDS * 4);
        for word in self.validity_words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_projects_to_all_zero_bytes_and_all_zero_validity() {
        let state = PhysicalTmemState::try_new().unwrap();
        let projection = project_committed_tmem(&state);
        assert_eq!(projection.bytes, [0u8; TMEM_LEN]);
        assert_eq!(projection.validity_words, [0u32; TMEM_VALIDITY_WORDS]);
    }

    #[test]
    fn validity_words_count_matches_tmem_bytes_over_thirty_two() {
        assert_eq!(TMEM_VALIDITY_WORDS * 32, TMEM_LEN);
        assert_eq!(TMEM_LEN, 4096);
        assert_eq!(TMEM_VALIDITY_WORDS, 128);
    }

    /// Mechanical proof (card audit repair) of `TmemGpuProjection`'s own
    /// packing convention -- built directly (its fields are public) rather
    /// than through a real `PhysicalTmemState` commit, since packing
    /// correctness is a property of `byte_words`/`byte_words_bytes`
    /// themselves, independent of how the bytes were committed. Four
    /// distinct, mechanically-checkable byte values at addresses 0..4,
    /// landing in word 0, prove the exact bit-shift convention
    /// `tmem_sample.wgsl`'s `tmem_read_byte` documents: "byte 0 in the low
    /// 8 bits".
    #[test]
    fn byte_words_packs_four_bytes_little_endian_per_word_matching_wgsl_tmem_read_byte() {
        let mut bytes = [0u8; TMEM_LEN];
        bytes[0] = 0x11;
        bytes[1] = 0x22;
        bytes[2] = 0x33;
        bytes[3] = 0x44;
        let projection = TmemGpuProjection {
            bytes,
            validity_words: [0u32; TMEM_VALIDITY_WORDS],
        };
        let words = projection.byte_words();
        assert_eq!(words[0], 0x4433_2211);
        let encoded = projection.byte_words_bytes();
        assert_eq!(encoded.len(), TMEM_BYTE_WORDS * 4);
        assert_eq!(&encoded[0..4], &[0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn validity_words_bytes_matches_validity_words_little_endian() {
        let mut validity_words = [0u32; TMEM_VALIDITY_WORDS];
        validity_words[0] = 1;
        validity_words[1] = 2;
        let projection = TmemGpuProjection {
            bytes: [0u8; TMEM_LEN],
            validity_words,
        };
        let encoded = projection.validity_words_bytes();
        assert_eq!(encoded.len(), TMEM_VALIDITY_WORDS * 4);
        assert_eq!(
            u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]),
            projection.validity_words[0]
        );
        assert_eq!(
            u32::from_le_bytes([encoded[4], encoded[5], encoded[6], encoded[7]]),
            projection.validity_words[1]
        );
    }

    fn test_tile_descriptor() -> TileDescriptor {
        TileDescriptor::from_wire(
            ImageFormat::Rgba,
            PixelSize::Bits16,
            1,
            crate::TmemWordAddress::try_new(0).unwrap(),
            0,
            crate::TileAddressMode::default(),
            0,
            0,
            crate::TileAddressMode::default(),
            0,
            0,
        )
    }

    fn test_tile_size() -> TileSize {
        TileSize::from_wire(
            crate::TileCoordinate::try_new(0).unwrap(),
            crate::TileCoordinate::try_new(0).unwrap(),
            crate::TileCoordinate::try_new(4).unwrap(),
            crate::TileCoordinate::try_new(4).unwrap(),
        )
    }

    /// Mechanical proof (card audit repair: "mechanically prove host
    /// byte-to-u32 packing and `TileBindingParams` layout") that
    /// `TileBindingParams::to_bytes`'s field order and byte offsets match
    /// `tmem_sample.wgsl`'s `TileBindingParams` WGSL struct exactly: each
    /// field is set to a distinct, unmistakable value, and this test reads
    /// back the exact byte offset the WGSL struct's declaration order
    /// implies for each `u32` field (uniform address space packs same-size
    /// scalars back-to-back, 4 bytes apart, no padding).
    #[test]
    fn to_bytes_offsets_match_the_wgsl_tile_binding_params_declaration_order() {
        let params = TileBindingParams::bound(test_tile_descriptor(), test_tile_size());
        let bytes = params.to_bytes();
        assert_eq!(bytes.len(), TILE_BINDING_PARAMS_BYTES as usize);
        assert_eq!(TILE_BINDING_PARAMS_FIELDS, 18);

        let word_at = |index: usize| -> u32 {
            let start = index * 4;
            u32::from_le_bytes([
                bytes[start],
                bytes[start + 1],
                bytes[start + 2],
                bytes[start + 3],
            ])
        };
        // Declaration order, matching `tmem_sample.wgsl`'s `TileBindingParams`
        // field-for-field: tmem_word_address, line_words, mask_s, shift_s,
        // mode_s_mirror, mode_s_clamp, mask_t, shift_t, mode_t_mirror,
        // mode_t_clamp, low_s, low_t, high_s, high_t, bound, format,
        // pixel_size, reserved_zero.
        assert_eq!(word_at(0), params.tmem_word_address);
        assert_eq!(word_at(1), params.line_words);
        assert_eq!(word_at(2), params.mask_s);
        assert_eq!(word_at(3), params.shift_s);
        assert_eq!(word_at(4), params.mode_s_mirror);
        assert_eq!(word_at(5), params.mode_s_clamp);
        assert_eq!(word_at(6), params.mask_t);
        assert_eq!(word_at(7), params.shift_t);
        assert_eq!(word_at(8), params.mode_t_mirror);
        assert_eq!(word_at(9), params.mode_t_clamp);
        assert_eq!(word_at(10), params.low_s);
        assert_eq!(word_at(11), params.low_t);
        assert_eq!(word_at(12), params.high_s);
        assert_eq!(word_at(13), params.high_t);
        assert_eq!(word_at(14), params.bound);
        assert_eq!(word_at(15), params.format);
        assert_eq!(word_at(16), params.pixel_size);
        assert_eq!(word_at(17), params.reserved_zero);

        assert_eq!(params.bound, 1);
        assert_eq!(params.format, 0, "ImageFormat::Rgba must encode to 0");
        assert_eq!(params.pixel_size, 2, "PixelSize::Bits16 must encode to 2");
    }

    #[test]
    fn unbound_is_all_zero_including_bound_flag() {
        let params = TileBindingParams::unbound();
        assert_eq!(params.to_bytes(), [0u8; TILE_BINDING_PARAMS_BYTES as usize]);
        assert_eq!(params.bound, 0);
    }
}
