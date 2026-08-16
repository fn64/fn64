use core::num::{NonZeroU16, NonZeroU64};

use fn64_render_ir::{
    JournalIdentity, OperationId, PhysicalAddress, PhysicalMemoryLayout, SubmissionIdentity,
    WorkloadIdentity,
};

use crate::{ImageFormat, PixelSize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileIndex(u8);

impl TileIndex {
    pub fn try_new(index: u8) -> Result<Self, &'static str> {
        (index < 8)
            .then_some(Self(index))
            .ok_or("tile index is outside the eight RDP tile slots")
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub(crate) const fn array_index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TmemWordAddress(u16);

impl TmemWordAddress {
    pub fn try_new(word: u16) -> Result<Self, &'static str> {
        (word < 512)
            .then_some(Self(word))
            .ok_or("TMEM word address exceeds the public nine-bit field")
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    pub const fn byte_address(self) -> u16 {
        self.0 * 8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileCoordinate(u16);

impl TileCoordinate {
    pub fn try_new(raw: u16) -> Result<Self, &'static str> {
        (raw < 4096)
            .then_some(Self(raw))
            .ok_or("tile coordinate exceeds the public 12-bit field")
    }

    pub const fn raw(self) -> u16 {
        self.0
    }

    pub const fn integer(self) -> u16 {
        self.0 >> 2
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TmemDxt(u16);

impl TmemDxt {
    pub fn try_new(raw: u16) -> Result<Self, &'static str> {
        (raw < 4096)
            .then_some(Self(raw))
            .ok_or("LoadBlock DXT exceeds the public 12-bit field")
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TlutEntryCount(NonZeroU16);

impl TlutEntryCount {
    pub fn try_new(entries: u16) -> Result<Self, &'static str> {
        let entries = NonZeroU16::new(entries).ok_or("LoadTLUT entry count is zero")?;
        (entries.get() <= 256)
            .then_some(Self(entries))
            .ok_or("LoadTLUT entry count exceeds the 256-entry TLUT")
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureImage {
    format: ImageFormat,
    size: PixelSize,
    width: u16,
    address: PhysicalAddress,
}

impl TextureImage {
    pub(crate) const fn from_wire(
        format: ImageFormat,
        size: PixelSize,
        width: u16,
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

    pub const fn width(self) -> u16 {
        self.width
    }

    pub const fn address(self) -> PhysicalAddress {
        self.address
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TileAddressMode {
    mirror: bool,
    clamp: bool,
}

impl TileAddressMode {
    pub(crate) const fn from_wire(value: u8) -> Self {
        Self {
            mirror: value & 1 != 0,
            clamp: value & 2 != 0,
        }
    }

    pub const fn mirror(self) -> bool {
        self.mirror
    }

    pub const fn clamp(self) -> bool {
        self.clamp
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileDescriptor {
    format: ImageFormat,
    size: PixelSize,
    line_words: u16,
    tmem: TmemWordAddress,
    palette: u8,
    t: TileAddressMode,
    mask_t: u8,
    shift_t: u8,
    s: TileAddressMode,
    mask_s: u8,
    shift_s: u8,
}

impl TileDescriptor {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn from_wire(
        format: ImageFormat,
        size: PixelSize,
        line_words: u16,
        tmem: TmemWordAddress,
        palette: u8,
        t: TileAddressMode,
        mask_t: u8,
        shift_t: u8,
        s: TileAddressMode,
        mask_s: u8,
        shift_s: u8,
    ) -> Self {
        Self {
            format,
            size,
            line_words,
            tmem,
            palette,
            t,
            mask_t,
            shift_t,
            s,
            mask_s,
            shift_s,
        }
    }

    pub const fn format(self) -> ImageFormat {
        self.format
    }

    pub const fn size(self) -> PixelSize {
        self.size
    }

    pub const fn line_words(self) -> u16 {
        self.line_words
    }

    pub const fn tmem(self) -> TmemWordAddress {
        self.tmem
    }

    pub const fn palette(self) -> u8 {
        self.palette
    }

    pub const fn s_mode(self) -> TileAddressMode {
        self.s
    }

    pub const fn t_mode(self) -> TileAddressMode {
        self.t
    }

    pub const fn mask_s(self) -> u8 {
        self.mask_s
    }

    pub const fn mask_t(self) -> u8 {
        self.mask_t
    }

    pub const fn shift_s(self) -> u8 {
        self.shift_s
    }

    pub const fn shift_t(self) -> u8 {
        self.shift_t
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileSize {
    low_s: TileCoordinate,
    low_t: TileCoordinate,
    high_s: TileCoordinate,
    high_t: TileCoordinate,
}

impl TileSize {
    pub(crate) const fn from_wire(
        low_s: TileCoordinate,
        low_t: TileCoordinate,
        high_s: TileCoordinate,
        high_t: TileCoordinate,
    ) -> Self {
        Self {
            low_s,
            low_t,
            high_s,
            high_t,
        }
    }

    pub const fn low_s(self) -> TileCoordinate {
        self.low_s
    }

    pub const fn low_t(self) -> TileCoordinate {
        self.low_t
    }

    pub const fn high_s(self) -> TileCoordinate {
        self.high_s
    }

    pub const fn high_t(self) -> TileCoordinate {
        self.high_t
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TmemLoadEpoch(NonZeroU64);

impl TmemLoadEpoch {
    pub(crate) const fn new(epoch: NonZeroU64) -> Self {
        Self(epoch)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TmemLoadSourceIdentity {
    workload: WorkloadIdentity,
    journal: JournalIdentity,
    submission: SubmissionIdentity,
    memory_layout: PhysicalMemoryLayout,
}

impl TmemLoadSourceIdentity {
    pub(crate) const fn new(
        workload: WorkloadIdentity,
        journal: JournalIdentity,
        submission: SubmissionIdentity,
        memory_layout: PhysicalMemoryLayout,
    ) -> Self {
        Self {
            workload,
            journal,
            submission,
            memory_layout,
        }
    }

    pub const fn workload(self) -> WorkloadIdentity {
        self.workload
    }

    pub const fn journal(self) -> JournalIdentity {
        self.journal
    }

    pub const fn submission(self) -> SubmissionIdentity {
        self.submission
    }

    pub const fn memory_layout(self) -> PhysicalMemoryLayout {
        self.memory_layout
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TmemLoadSourcePlan {
    identity: TmemLoadSourceIdentity,
    source_access_identity: JournalIdentity,
    first_access_index: u32,
    first_operation: OperationId,
    access_count: u16,
    total_bytes: u32,
}

impl TmemLoadSourcePlan {
    pub(crate) const fn new(
        identity: TmemLoadSourceIdentity,
        source_access_identity: JournalIdentity,
        first_access_index: u32,
        first_operation: OperationId,
        access_count: u16,
        total_bytes: u32,
    ) -> Self {
        Self {
            identity,
            source_access_identity,
            first_access_index,
            first_operation,
            access_count,
            total_bytes,
        }
    }

    pub const fn identity(self) -> TmemLoadSourceIdentity {
        self.identity
    }

    pub const fn source_access_identity(self) -> JournalIdentity {
        self.source_access_identity
    }

    pub const fn first_access_index(self) -> u32 {
        self.first_access_index
    }

    pub const fn first_operation(self) -> OperationId {
        self.first_operation
    }

    pub const fn access_count(self) -> u16 {
        self.access_count
    }

    pub const fn total_bytes(self) -> u32 {
        self.total_bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TmemLoadKind {
    Block {
        source_s: TileCoordinate,
        source_t: TileCoordinate,
        high_s: TileCoordinate,
        dxt: TmemDxt,
    },
    Tile {
        bounds: TileSize,
    },
    Tlut {
        bounds: TileSize,
        entries: TlutEntryCount,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TmemLoad {
    epoch: TmemLoadEpoch,
    tile: TileIndex,
    source_image: TextureImage,
    tile_descriptor: TileDescriptor,
    kind: TmemLoadKind,
    source_plan: TmemLoadSourcePlan,
}

impl TmemLoad {
    pub(crate) const fn new(
        epoch: TmemLoadEpoch,
        tile: TileIndex,
        source_image: TextureImage,
        tile_descriptor: TileDescriptor,
        kind: TmemLoadKind,
        source_plan: TmemLoadSourcePlan,
    ) -> Self {
        Self {
            epoch,
            tile,
            source_image,
            tile_descriptor,
            kind,
            source_plan,
        }
    }

    pub const fn epoch(self) -> TmemLoadEpoch {
        self.epoch
    }

    pub const fn tile(self) -> TileIndex {
        self.tile
    }

    pub const fn source_image(self) -> TextureImage {
        self.source_image
    }

    pub const fn tile_descriptor(self) -> TileDescriptor {
        self.tile_descriptor
    }

    pub const fn kind(self) -> TmemLoadKind {
        self.kind
    }

    pub const fn source_plan(self) -> TmemLoadSourcePlan {
        self.source_plan
    }
}
