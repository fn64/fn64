//! Typed RDP state staged by one raw-DPC transaction.

use fn64_render_ir::{PhysicalAddress, QueueIdentity};

use crate::tmem::TmemState;

/// Texture lookup-table interpretation selected by `SetOtherModes` high bits
/// 15:14 (`G_MDSFT_TEXTLUT`).
///
/// The encodings follow the permitted MIT RT64 source pinned by
/// `docs/RT64-PORT-AUTHORITY.md` (`shared/rt64_f3d_defines.h` and
/// `shared/rt64_other_mode.h`): zero disables the TLUT, two selects RGBA16,
/// and three selects IA16. Encoding one is reserved and is rejected rather
/// than treated as a disabled table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureLutMode {
    Disabled,
    Rgba16,
    Ia16,
}

/// Why `SetOtherModes`' texture-LUT field could not be decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureLutModeError {
    ReservedEncoding { encoding: u8 },
}

impl core::fmt::Display for TextureLutModeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ReservedEncoding { encoding } => write!(
                formatter,
                "SetOtherModes texture-LUT field uses reserved encoding {encoding}"
            ),
        }
    }
}

impl std::error::Error for TextureLutModeError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleType {
    OneCycle,
    TwoCycle,
    Copy,
    Fill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtherMode {
    high: u32,
    low: u32,
}

impl OtherMode {
    pub(crate) const fn from_wire(high: u32, low: u32) -> Self {
        Self { high, low }
    }

    pub const fn high(self) -> u32 {
        self.high
    }

    pub const fn low(self) -> u32 {
        self.low
    }

    pub const fn cycle_type(self) -> CycleType {
        match (self.high >> 20) & 0x3 {
            0 => CycleType::OneCycle,
            1 => CycleType::TwoCycle,
            2 => CycleType::Copy,
            _ => CycleType::Fill,
        }
    }

    /// Decodes the two-bit texture-LUT selector without normalizing its
    /// reserved encoding into a supported mode.
    pub const fn texture_lut_mode(self) -> Result<TextureLutMode, TextureLutModeError> {
        let encoding = ((self.high >> 14) & 0x3) as u8;
        match encoding {
            0 => Ok(TextureLutMode::Disabled),
            2 => Ok(TextureLutMode::Rgba16),
            3 => Ok(TextureLutMode::Ia16),
            _ => Err(TextureLutModeError::ReservedEncoding { encoding }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Rgba,
    Yuv,
    ColorIndex,
    IntensityAlpha,
    Intensity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelSize {
    Bits4,
    Bits8,
    Bits16,
    Bits32,
}

impl PixelSize {
    pub const fn bytes_per_pixel(self) -> Option<u32> {
        match self {
            Self::Bits4 => None,
            Self::Bits8 => Some(1),
            Self::Bits16 => Some(2),
            Self::Bits32 => Some(4),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorImage {
    format: ImageFormat,
    size: PixelSize,
    width: u32,
    address: PhysicalAddress,
}

impl ColorImage {
    pub(crate) const fn from_wire(
        format: ImageFormat,
        size: PixelSize,
        width: u32,
        address: PhysicalAddress,
    ) -> Self {
        Self {
            format,
            size,
            width,
            address,
        }
    }

    pub const fn format(self) -> ImageFormat {
        self.format
    }

    pub const fn size(self) -> PixelSize {
        self.size
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn address(self) -> PhysicalAddress {
        self.address
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillColor(u32);

impl FillColor {
    pub(crate) const fn from_wire(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }

    pub const fn rgba32(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }
}

/// Durable renderer state is immutable to the decoder. A caller can publish a
/// staged result only at a later owner-controlled commit boundary.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RdpState {
    other_mode: Option<OtherMode>,
    color_image: Option<ColorImage>,
    fill_color: Option<FillColor>,
    tmem: TmemState,
}

impl RdpState {
    pub const fn other_mode(&self) -> Option<OtherMode> {
        self.other_mode
    }

    pub const fn color_image(&self) -> Option<ColorImage> {
        self.color_image
    }

    pub const fn fill_color(&self) -> Option<FillColor> {
        self.fill_color
    }

    pub const fn tmem(&self) -> &TmemState {
        &self.tmem
    }

    pub(crate) fn tmem_mut(&mut self) -> &mut TmemState {
        &mut self.tmem
    }

    pub(crate) fn fork_for_decode(&self) -> Self {
        Self {
            other_mode: self.other_mode,
            color_image: self.color_image,
            fill_color: self.fill_color,
            tmem: self.tmem.clone(),
        }
    }

    pub(crate) fn apply(&mut self, delta: &RdpStateDelta) {
        if let Some(value) = delta.other_mode {
            self.other_mode = Some(value);
        }
        if let Some(value) = delta.color_image {
            self.color_image = Some(value);
        }
        if let Some(value) = delta.fill_color {
            self.fill_color = Some(value);
        }
        if let Some(value) = &delta.tmem {
            self.tmem = value.clone();
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RdpStateDelta {
    other_mode: Option<OtherMode>,
    color_image: Option<ColorImage>,
    fill_color: Option<FillColor>,
    tmem: Option<TmemState>,
}

impl RdpStateDelta {
    pub const fn other_mode(&self) -> Option<OtherMode> {
        self.other_mode
    }

    pub const fn color_image(&self) -> Option<ColorImage> {
        self.color_image
    }

    pub const fn fill_color(&self) -> Option<FillColor> {
        self.fill_color
    }

    pub const fn tmem(&self) -> Option<&TmemState> {
        self.tmem.as_ref()
    }

    pub(crate) fn set_other_mode(&mut self, value: OtherMode) {
        self.other_mode = Some(value);
    }

    pub(crate) fn set_color_image(&mut self, value: ColorImage) {
        self.color_image = Some(value);
    }

    pub(crate) fn set_fill_color(&mut self, value: FillColor) {
        self.fill_color = Some(value);
    }

    pub(crate) fn set_tmem(&mut self, value: TmemState) {
        self.tmem = Some(value);
    }
}

/// Transaction-local state. Its distinct type makes cross-packet chaining an
/// explicit choice and prevents decode from masquerading as durable commit.
#[derive(Debug, PartialEq, Eq)]
pub struct StagedRdpState {
    state: RdpState,
    queue: QueueIdentity,
    submission_ordinal: u64,
    transaction_sequence: u64,
}

impl StagedRdpState {
    pub const fn other_mode(&self) -> Option<OtherMode> {
        self.state.other_mode()
    }

    pub const fn color_image(&self) -> Option<ColorImage> {
        self.state.color_image()
    }

    pub const fn fill_color(&self) -> Option<FillColor> {
        self.state.fill_color()
    }

    pub const fn tmem(&self) -> &TmemState {
        self.state.tmem()
    }

    pub const fn queue(&self) -> QueueIdentity {
        self.queue
    }

    pub const fn transaction_sequence(&self) -> u64 {
        self.transaction_sequence
    }

    pub const fn submission_ordinal(&self) -> u64 {
        self.submission_ordinal
    }

    pub(crate) fn from_transaction(
        state: RdpState,
        queue: QueueIdentity,
        submission_ordinal: u64,
        transaction_sequence: u64,
    ) -> Self {
        Self {
            state,
            queue,
            submission_ordinal,
            transaction_sequence,
        }
    }

    pub(crate) fn into_parts(self) -> (RdpState, QueueIdentity, u64, u64) {
        (
            self.state,
            self.queue,
            self.submission_ordinal,
            self.transaction_sequence,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_lut_mode_decodes_all_four_wire_encodings_without_normalization() {
        assert_eq!(
            OtherMode::from_wire(0 << 14, u32::MAX).texture_lut_mode(),
            Ok(TextureLutMode::Disabled)
        );
        assert_eq!(
            OtherMode::from_wire(1 << 14, 0).texture_lut_mode(),
            Err(TextureLutModeError::ReservedEncoding { encoding: 1 })
        );
        assert_eq!(
            OtherMode::from_wire(2 << 14, 0).texture_lut_mode(),
            Ok(TextureLutMode::Rgba16)
        );
        assert_eq!(
            OtherMode::from_wire(3 << 14, 0).texture_lut_mode(),
            Ok(TextureLutMode::Ia16)
        );
    }

    #[test]
    fn texture_lut_mode_ignores_unrelated_other_mode_bits() {
        let high = 0x00ff_ffff & !(0x3 << 14);
        assert_eq!(
            OtherMode::from_wire(high | (2 << 14), u32::MAX).texture_lut_mode(),
            Ok(TextureLutMode::Rgba16)
        );
    }

    #[test]
    fn reserved_texture_lut_encoding_is_a_public_typed_error() {
        let error = OtherMode::from_wire(1 << 14, 0)
            .texture_lut_mode()
            .unwrap_err();
        assert_eq!(error, TextureLutModeError::ReservedEncoding { encoding: 1 });
        assert!(!error.to_string().is_empty());
        let _: &dyn std::error::Error = &error;
    }
}
