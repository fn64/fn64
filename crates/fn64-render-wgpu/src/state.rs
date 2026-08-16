//! Typed RDP state staged by one raw-DPC transaction.

use fn64_render_ir::{PhysicalAddress, QueueIdentity};

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Rgba,
    Yuv,
    ColorIndex,
    IntensityAlpha,
    Intensity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    pub(crate) const fn fork_for_decode(&self) -> Self {
        Self {
            other_mode: self.other_mode,
            color_image: self.color_image,
            fill_color: self.fill_color,
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
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RdpStateDelta {
    other_mode: Option<OtherMode>,
    color_image: Option<ColorImage>,
    fill_color: Option<FillColor>,
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

    pub(crate) fn set_other_mode(&mut self, value: OtherMode) {
        self.other_mode = Some(value);
    }

    pub(crate) fn set_color_image(&mut self, value: ColorImage) {
        self.color_image = Some(value);
    }

    pub(crate) fn set_fill_color(&mut self, value: FillColor) {
        self.fill_color = Some(value);
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
