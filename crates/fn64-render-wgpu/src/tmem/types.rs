use core::num::{NonZeroU16, NonZeroU64};

use fn64_render_ir::{
    JournalIdentity, OperationId, PhysicalAddress, PhysicalMemoryLayout, SubmissionIdentity,
    TmemRange, WorkloadIdentity,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TmemLoadDestinationPlan {
    identity: TmemLoadSourceIdentity,
    source_access_identity: JournalIdentity,
    destination_access_identity: JournalIdentity,
    first_access_index: u32,
    first_operation: OperationId,
    access_count: u16,
    total_bytes: u32,
}

impl TmemLoadDestinationPlan {
    pub(crate) const fn new(
        identity: TmemLoadSourceIdentity,
        source_access_identity: JournalIdentity,
        destination_access_identity: JournalIdentity,
        first_access_index: u32,
        first_operation: OperationId,
        access_count: u16,
        total_bytes: u32,
    ) -> Self {
        Self {
            identity,
            source_access_identity,
            destination_access_identity,
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

    pub const fn destination_access_identity(self) -> JournalIdentity {
        self.destination_access_identity
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
pub enum TmemTransferLayout {
    Linear64,
    SplitBanks64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TmemTransferGeometry {
    destination_word: u16,
    row_advance: u16,
    odd_row_exchange: bool,
    physical: TmemTransferPhysicalWord,
}

impl TmemTransferGeometry {
    pub(crate) const fn destination_word(self) -> u16 {
        self.destination_word
    }

    pub(crate) const fn row_advance(self) -> u16 {
        self.row_advance
    }

    pub(crate) const fn odd_row_exchange(self) -> bool {
        self.odd_row_exchange
    }

    pub(crate) const fn physical(self) -> TmemTransferPhysicalWord {
        self.physical
    }
}

/// Projects a LoadTLUT destination word into the high TMEM bank (words
/// 256-511), never wrapping into the low half. SGI RDP Command Summary
/// Table 9 / libultra `gbi.h` `gDPLoadTLUTCmd` require the palette
/// destination tile's `tmem` field to already be `>= 256` (enforced by
/// `decode_load_tlut`'s pre-existing gate before this arm runs); once
/// admitted, every subsequent entry word must stay inside that same
/// 256-word high bank -- this is public hardware/project projection
/// authority (SGI Table 9 / `gDPLoadTLUTCmd`), not a claim this
/// repository's `write_tlut` (`fn64-render-reference/src/gbi/state.rs`)
/// establishes. `write_tlut` is cited elsewhere for the quadrication/
/// stride fact only (one entry per 8-byte word, value quadricated into
/// it); it is HLE F3DEX2 authority for that fact and is explicitly
/// **not** authority for high-bank wrap: its own `physical_byte` masks
/// against the *full* 4 KiB TMEM byte domain
/// (`fn64-render-reference/src/gbi/state.rs`'s `TMEM_BYTES - 1` mask),
/// so a `write_tlut(511, 1, _)` call would wrap into low-half TMEM --
/// the same noncanonical behavior this function exists to avoid, not a
/// precedent for avoiding it. Wrapping through the shared 512-word
/// `& 0x01ff` mask Block/Tile use would likewise let `base=511, entry=1`
/// land on low-half word 0 -- a destination LoadTLUT can never target on
/// real hardware.
const fn project_tlut_high_bank_word(base_word: u16, entry_index: u64) -> u16 {
    let projected = (base_word as u64 + entry_index) & 0xff;
    0x100 | projected as u16
}

pub(crate) fn project_tmem_transfer_word(
    descriptor: TileDescriptor,
    kind: TmemLoadKind,
    layout: TmemTransferLayout,
    transfer_words: u16,
    words_per_row: u16,
    word: u16,
) -> Result<TmemTransferGeometry, &'static str> {
    if word >= transfer_words {
        return Err("TMEM transfer word index is outside the declared plan");
    }
    let word = u64::from(word);
    let line = u64::from(descriptor.line_words());
    let (destination_word, row_advance, odd_row_exchange) = match kind {
        TmemLoadKind::Block { source_t, dxt, .. } => {
            let advance = word
                .checked_mul(u64::from(dxt.get()))
                .ok_or("TMEM LoadBlock DXT product overflows")?
                >> 11;
            let destination = u64::from(descriptor.tmem().get())
                .checked_add(word)
                .and_then(|value| value.checked_add(advance.checked_mul(line)?))
                .ok_or("TMEM LoadBlock destination word overflows")?;
            let mask = match layout {
                TmemTransferLayout::Linear64 => 0x01ff,
                TmemTransferLayout::SplitBanks64 => 0x00ff,
            };
            (
                (destination & mask) as u16,
                advance,
                (u64::from(source_t.raw()) + advance) & 1 != 0,
            )
        }
        TmemLoadKind::Tile { bounds } => {
            if words_per_row == 0 {
                return Err("TMEM LoadTile row word count is zero");
            }
            let row = word / u64::from(words_per_row);
            let within = word % u64::from(words_per_row);
            let destination = u64::from(descriptor.tmem().get())
                .checked_add(
                    row.checked_mul(line)
                        .ok_or("TMEM LoadTile row offset overflows")?,
                )
                .and_then(|value| value.checked_add(within))
                .ok_or("TMEM LoadTile destination word overflows")?;
            let mask = match layout {
                TmemTransferLayout::Linear64 => 0x01ff,
                TmemTransferLayout::SplitBanks64 => 0x00ff,
            };
            (
                (destination & mask) as u16,
                row,
                (u64::from(bounds.low_t().integer()) + row) & 1 != 0,
            )
        }
        TmemLoadKind::Tlut { .. } => {
            // SGI RDP Command Summary Table 9 / libultra `gbi.h`
            // `gDPLoadTLUTCmd`: LoadTLUT's destination is `entries`
            // consecutive TMEM word slots starting at the tile descriptor's
            // `tmem` word, one 8-byte word per palette entry (the entry's
            // 16-bit value is quadricated within that word; see
            // `destination_ranges`/`transfer_shape` below and this
            // repository's own already-shipped `write_tlut`
            // (`fn64-render-reference/src/gbi/state.rs`), whose
            // `base_word + index` addressing and regression fixture
            // `ci4_samples_quadricated_tlut_at_palette_bank_address`
            // establish this exact stride as fn64's own cited behavioral
            // spec -- for quadrication/stride only. `write_tlut` is not
            // cited for destination wrap: see `project_tlut_high_bank_word`'s
            // doc comment for why its own full-4-KiB `physical_byte` mask
            // makes it the opposite of high-bank-wrap authority. There is
            // no row/DXT accumulation, so word advance is a direct index and
            // there is no odd-row bank exchange. The destination projects
            // through `project_tlut_high_bank_word`, never the shared
            // 512-word linear mask below -- a TLUT destination must stay in
            // the high bank even when `base + entry` would otherwise wrap
            // into low TMEM.
            let destination_word = project_tlut_high_bank_word(descriptor.tmem().get(), word);
            (destination_word, word, false)
        }
    };
    let row_advance =
        u16::try_from(row_advance).map_err(|_| "TMEM transfer row advance exceeds u16")?;
    let destination = u32::from(destination_word);
    let physical = match layout {
        TmemTransferLayout::Linear64 => TmemTransferPhysicalWord::Linear(
            TmemRange::try_new(destination * 8, destination * 8 + 8)
                .map_err(|_| "linear TMEM transfer word is outside TMEM")?,
        ),
        TmemTransferLayout::SplitBanks64 => {
            let exchange = if odd_row_exchange { 4 } else { 0 };
            let low = destination * 8 + exchange;
            TmemTransferPhysicalWord::SplitBanks {
                low: TmemRange::try_new(low, low + 4)
                    .map_err(|_| "low split-bank TMEM fragment is outside TMEM")?,
                high: TmemRange::try_new(low + 2048, low + 2052)
                    .map_err(|_| "high split-bank TMEM fragment is outside TMEM")?,
            }
        }
    };
    Ok(TmemTransferGeometry {
        destination_word,
        row_advance,
        odd_row_exchange,
        physical,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TmemTransferPlan {
    source: TmemLoadSourcePlan,
    destination: TmemLoadDestinationPlan,
    logical_source_bytes: u32,
    transfer_words: u16,
    undefined_padding_bytes: u32,
    words_per_row: u16,
    row_count: u16,
    layout: TmemTransferLayout,
    kind: TmemLoadKind,
    epoch: TmemLoadEpoch,
    tile: TileIndex,
    source_image: TextureImage,
    tile_descriptor: TileDescriptor,
}

impl TmemTransferPlan {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        source: TmemLoadSourcePlan,
        destination: TmemLoadDestinationPlan,
        logical_source_bytes: u32,
        transfer_words: u16,
        undefined_padding_bytes: u32,
        words_per_row: u16,
        row_count: u16,
        layout: TmemTransferLayout,
        kind: TmemLoadKind,
        epoch: TmemLoadEpoch,
        tile: TileIndex,
        source_image: TextureImage,
        tile_descriptor: TileDescriptor,
    ) -> Self {
        Self {
            source,
            destination,
            logical_source_bytes,
            transfer_words,
            undefined_padding_bytes,
            words_per_row,
            row_count,
            layout,
            kind,
            epoch,
            tile,
            source_image,
            tile_descriptor,
        }
    }

    pub const fn source(self) -> TmemLoadSourcePlan {
        self.source
    }

    pub const fn destination(self) -> TmemLoadDestinationPlan {
        self.destination
    }

    pub const fn logical_source_bytes(self) -> u32 {
        self.logical_source_bytes
    }

    pub const fn transfer_words(self) -> u16 {
        self.transfer_words
    }

    pub const fn written_bytes(self) -> u32 {
        self.transfer_words as u32 * 8
    }

    /// Bytes touched by complete 64-bit transfer words whose values are not
    /// defined by the logical source span. This is effect coverage, never a
    /// promise that padding contains zeroes or valid texture content.
    pub const fn undefined_padding_bytes(self) -> u32 {
        self.undefined_padding_bytes
    }

    pub const fn words_per_row(self) -> u16 {
        self.words_per_row
    }

    pub const fn row_count(self) -> u16 {
        self.row_count
    }

    pub const fn layout(self) -> TmemTransferLayout {
        self.layout
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

    pub(crate) fn geometry_for_word(self, word: u16) -> Result<TmemTransferGeometry, &'static str> {
        project_tmem_transfer_word(
            self.tile_descriptor,
            self.kind,
            self.layout,
            self.transfer_words,
            self.words_per_row,
            word,
        )
    }

    pub fn destination_word(self, word: u16) -> Result<u16, &'static str> {
        Ok(self.geometry_for_word(word)?.destination_word())
    }

    pub fn logical_source_offset(self, word: u16) -> Result<u32, &'static str> {
        if word >= self.transfer_words {
            return Err("TMEM transfer word index is outside the declared plan");
        }
        match self.kind {
            TmemLoadKind::Block { .. } => Ok(u32::from(word) * 8),
            TmemLoadKind::Tile { .. } => {
                let row_bytes = self.logical_source_bytes / u32::from(self.row_count);
                let row = u32::from(word) / u32::from(self.words_per_row);
                let within = u32::from(word) % u32::from(self.words_per_row);
                row.checked_mul(row_bytes)
                    .and_then(|offset| offset.checked_add(within * 8))
                    .ok_or("TMEM LoadTile logical source offset overflows")
            }
            // One TLUT entry is two source bytes, addressed sequentially
            // (word index == entry index); there is no 8-bytes-per-word
            // source stride here, unlike Block/Tile.
            TmemLoadKind::Tlut { .. } => Ok(u32::from(word) * 2),
        }
    }

    pub fn row_advance_for_word(self, word: u16) -> Result<u16, &'static str> {
        Ok(self.geometry_for_word(word)?.row_advance())
    }

    pub fn physical_word(self, word: u16) -> Result<TmemTransferPhysicalWord, &'static str> {
        Ok(self.geometry_for_word(word)?.physical())
    }

    pub fn defined_source_byte_mask(self, word: u16) -> Result<u8, &'static str> {
        if word >= self.transfer_words {
            return Err("TMEM transfer word index is outside the declared plan");
        }
        // TLUT's mask names which of the destination word's 8 bytes are
        // *captured source* bytes (the entry's 2 real bytes at offsets 0-1,
        // per `logical_source_offset`'s `word * 2` addressing) -- not which
        // bytes hold defined destination content. The other 6 bytes are real,
        // defined TMEM content (quadricated copies, not undefined padding:
        // `undefined_padding_bytes` stays 0 for Tlut), but the hardware
        // derives them from the same 2 bytes this mask reports rather than
        // capturing them independently from the source.
        if matches!(self.kind, TmemLoadKind::Tlut { .. }) {
            return Ok(0x03);
        }
        let defined = match self.kind {
            TmemLoadKind::Block { .. } => self
                .logical_source_bytes
                .saturating_sub(u32::from(word) * 8)
                .min(8),
            TmemLoadKind::Tile { .. } => {
                let row_bytes = self.logical_source_bytes / u32::from(self.row_count);
                let within = u32::from(word) % u32::from(self.words_per_row);
                row_bytes.saturating_sub(within * 8).min(8)
            }
            TmemLoadKind::Tlut { .. } => 0,
        };
        Ok(if defined == 8 {
            u8::MAX
        } else {
            (1_u16.checked_shl(defined).unwrap_or(0) - 1) as u8
        })
    }

    /// Returns the public odd-row exchange selector for one complete 64-bit
    /// transfer word. LoadBlock's DXT accumulator starts at the command's TL;
    /// rebasing every block to row zero loses that first-row parity.
    pub fn word_uses_odd_row_exchange(self, word: u16) -> Result<bool, &'static str> {
        Ok(self.geometry_for_word(word)?.odd_row_exchange())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
// Command-state snapshots remain pointer-free and Copy; only
// `BoundTmemTransfer` is an immutable checked plan view. Introducing shared
// heap ownership here would blur the command-state boundary before M4.2 owns
// a consuming execution capability and durable TMEM.
#[allow(clippy::large_enum_variant)]
pub enum TmemLoadContract {
    Transfer(TmemTransferPlan),
    DeferredYuv { source: TmemLoadSourcePlan },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TmemTransferPhysicalWord {
    Linear(TmemRange),
    SplitBanks { low: TmemRange, high: TmemRange },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TmemTransferWord {
    index: u16,
    logical_source_offset: u32,
    source_access_index: u32,
    source_access_byte_offset: u32,
    defined_source_byte_mask: u8,
    destination_word: u16,
    row_advance: u16,
    odd_row_exchange: bool,
    physical: TmemTransferPhysicalWord,
}

impl TmemTransferWord {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        index: u16,
        logical_source_offset: u32,
        source_access_index: u32,
        source_access_byte_offset: u32,
        defined_source_byte_mask: u8,
        destination_word: u16,
        row_advance: u16,
        odd_row_exchange: bool,
        physical: TmemTransferPhysicalWord,
    ) -> Self {
        Self {
            index,
            logical_source_offset,
            source_access_index,
            source_access_byte_offset,
            defined_source_byte_mask,
            destination_word,
            row_advance,
            odd_row_exchange,
            physical,
        }
    }

    pub const fn index(self) -> u16 {
        self.index
    }

    pub const fn logical_source_offset(self) -> u32 {
        self.logical_source_offset
    }

    pub const fn source_access_index(self) -> u32 {
        self.source_access_index
    }

    pub const fn source_access_byte_offset(self) -> u32 {
        self.source_access_byte_offset
    }

    pub const fn defined_source_byte_mask(self) -> u8 {
        self.defined_source_byte_mask
    }

    pub const fn destination_word(self) -> u16 {
        self.destination_word
    }

    pub const fn row_advance(self) -> u16 {
        self.row_advance
    }

    pub const fn odd_row_exchange(self) -> bool {
        self.odd_row_exchange
    }

    pub const fn physical(self) -> TmemTransferPhysicalWord {
        self.physical
    }
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
    contract: TmemLoadContract,
}

impl TmemLoad {
    pub(crate) const fn new(
        epoch: TmemLoadEpoch,
        tile: TileIndex,
        source_image: TextureImage,
        tile_descriptor: TileDescriptor,
        kind: TmemLoadKind,
        transfer_plan: TmemTransferPlan,
    ) -> Self {
        Self {
            epoch,
            tile,
            source_image,
            tile_descriptor,
            kind,
            contract: TmemLoadContract::Transfer(transfer_plan),
        }
    }

    pub(crate) const fn new_deferred_yuv(
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
            contract: TmemLoadContract::DeferredYuv {
                source: source_plan,
            },
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
        match self.contract {
            TmemLoadContract::Transfer(plan) => plan.source(),
            TmemLoadContract::DeferredYuv { source } => source,
        }
    }

    pub const fn contract(self) -> TmemLoadContract {
        self.contract
    }

    pub const fn transfer_plan(self) -> Result<TmemTransferPlan, &'static str> {
        match self.contract {
            TmemLoadContract::Transfer(plan) => Ok(plan),
            TmemLoadContract::DeferredYuv { .. } => {
                Err("YUV destination execution is deferred pending a public pairing contract")
            }
        }
    }
}
