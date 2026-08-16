//! Public RDP texture-command wire fields.
//!
//! Opcode and field masks are transcribed from the public SGI *Nintendo 64
//! RDP Command Summary*, Tables 1, 3, and 6–10. LoadTLUT's restricted shape
//! and ten-bit `count - 1` field follow public libultra `gbi.h`'s
//! `gDPLoadTLUTCmd` macro. Fn64's M4.0/M4.1 design documents define ownership
//! and transactional staging; RT64 is not a hardware authority here.

use fn64_render_ir::{
    AccessMode, AccessPurpose, OperationId, PhysicalMemoryLayout, PhysicalRange, RdramResource,
    ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion, MAX_RESOURCE_ACCESSES,
};

use crate::{ImageFormat, PixelSize};

use super::{
    TextureImage, TileAddressMode, TileCoordinate, TileDescriptor, TileIndex, TileSize,
    TlutEntryCount, TmemDxt, TmemLoad, TmemLoadKind, TmemLoadSourceIdentity, TmemLoadSourcePlan,
    TmemState, TmemWordAddress,
};

pub(crate) const LOAD_SYNC: u8 = 0x26;
pub(crate) const LOAD_TLUT: u8 = 0x30;
pub(crate) const SET_TILE_SIZE: u8 = 0x32;
pub(crate) const LOAD_BLOCK: u8 = 0x33;
pub(crate) const LOAD_TILE: u8 = 0x34;
pub(crate) const SET_TILE: u8 = 0x35;
pub(crate) const SET_TEXTURE_IMAGE: u8 = 0x3d;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TmemSourcePlanStart {
    identity: TmemLoadSourceIdentity,
    access_index: u32,
    operation: OperationId,
}

impl TmemSourcePlanStart {
    pub(crate) const fn new(
        identity: TmemLoadSourceIdentity,
        access_index: u32,
        operation: OperationId,
    ) -> Self {
        Self {
            identity,
            access_index,
            operation,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TmemCommand {
    SetTextureImage(TextureImage),
    SetTile {
        tile: TileIndex,
        descriptor: TileDescriptor,
    },
    SetTileSize {
        tile: TileIndex,
        size: TileSize,
    },
    LoadSync(super::TmemLoadEpoch),
    LoadBlock(TmemLoad),
    LoadTile(TmemLoad),
    LoadTlut(TmemLoad),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TmemWireError {
    reason: &'static str,
}

impl TmemWireError {
    const fn new(reason: &'static str) -> Self {
        Self { reason }
    }

    pub(crate) const fn reason(self) -> &'static str {
        self.reason
    }
}

pub(crate) fn decode_tmem_command(
    opcode: u8,
    w0: u32,
    w1: u32,
    layout: PhysicalMemoryLayout,
    state: &mut TmemState,
    source_start: TmemSourcePlanStart,
) -> Result<(TmemCommand, Vec<ResourceAccess>), TmemWireError> {
    match opcode {
        SET_TEXTURE_IMAGE => {
            let image = decode_texture_image(w0, w1, layout)?;
            state.set_texture_image(image);
            Ok((TmemCommand::SetTextureImage(image), Vec::new()))
        }
        SET_TILE => {
            let tile = tile_index(w1)?;
            let descriptor = decode_tile_descriptor(w0, w1)?;
            state.set_tile(tile, descriptor);
            Ok((TmemCommand::SetTile { tile, descriptor }, Vec::new()))
        }
        SET_TILE_SIZE => {
            let tile = tile_index(w1)?;
            let size = tile_size(w0, w1)?;
            state.set_tile_size(tile, size);
            Ok((TmemCommand::SetTileSize { tile, size }, Vec::new()))
        }
        LOAD_SYNC => {
            let epoch = state.load_sync().map_err(TmemWireError::new)?;
            Ok((TmemCommand::LoadSync(epoch), Vec::new()))
        }
        LOAD_BLOCK => decode_load_block(
            w0,
            w1,
            layout,
            state,
            source_start.identity,
            source_start.access_index,
            source_start.operation,
        ),
        LOAD_TILE => decode_load_tile(
            w0,
            w1,
            layout,
            state,
            source_start.identity,
            source_start.access_index,
            source_start.operation,
        ),
        LOAD_TLUT => decode_load_tlut(
            w0,
            w1,
            layout,
            state,
            source_start.identity,
            source_start.access_index,
            source_start.operation,
        ),
        _ => Err(TmemWireError::new("opcode is not a TMEM command")),
    }
}

fn decode_texture_image(
    w0: u32,
    w1: u32,
    layout: PhysicalMemoryLayout,
) -> Result<TextureImage, TmemWireError> {
    let format = image_format((w0 >> 21) & 7)?;
    let size = pixel_size((w0 >> 19) & 3);
    let width = u16::try_from((w0 & 0x0fff) + 1)
        .map_err(|_| TmemWireError::new("SetTextureImage width overflows u16"))?;
    // Table 3 assigns the low 26 bits to the public image-address payload.
    // Bits 31..26 are outside that payload and remain only in the owned raw
    // command bytes; semantic decode intentionally does not reinterpret them.
    let address = layout
        .address(w1 & 0x03ff_ffff)
        .map_err(|_| TmemWireError::new("SetTextureImage address is outside installed RDRAM"))?;
    Ok(TextureImage::from_wire(format, size, width, address))
}

fn decode_tile_descriptor(w0: u32, w1: u32) -> Result<TileDescriptor, TmemWireError> {
    Ok(TileDescriptor::from_wire(
        image_format((w0 >> 21) & 7)?,
        pixel_size((w0 >> 19) & 3),
        ((w0 >> 9) & 0x01ff) as u16,
        TmemWordAddress::try_new((w0 & 0x01ff) as u16).map_err(TmemWireError::new)?,
        ((w1 >> 20) & 0x0f) as u8,
        TileAddressMode::from_wire(((w1 >> 18) & 3) as u8),
        ((w1 >> 14) & 0x0f) as u8,
        ((w1 >> 10) & 0x0f) as u8,
        TileAddressMode::from_wire(((w1 >> 8) & 3) as u8),
        ((w1 >> 4) & 0x0f) as u8,
        (w1 & 0x0f) as u8,
    ))
}

fn decode_load_block(
    w0: u32,
    w1: u32,
    layout: PhysicalMemoryLayout,
    state: &mut TmemState,
    source_identity: TmemLoadSourceIdentity,
    first_access_index: u32,
    first_operation: OperationId,
) -> Result<(TmemCommand, Vec<ResourceAccess>), TmemWireError> {
    let tile = tile_index(w1)?;
    let (epoch, image, descriptor) = state
        .load_inputs(tile, layout)
        .map_err(TmemWireError::new)?;
    let source_s = coordinate((w0 >> 12) & 0x0fff)?;
    let source_t = coordinate(w0 & 0x0fff)?;
    let high_s = coordinate((w1 >> 12) & 0x0fff)?;
    let dxt = TmemDxt::try_new((w1 & 0x0fff) as u16).map_err(TmemWireError::new)?;
    if high_s.raw() < source_s.raw() {
        return Err(TmemWireError::new("LoadBlock source span is inverted"));
    }
    if source_t.raw() > 1023 {
        return Err(TmemWireError::new(
            "LoadBlock TL exceeds the public ten-bit limit",
        ));
    }
    let texels = u32::from(high_s.raw() - source_s.raw()) + 1;
    if texels > 2048 {
        return Err(TmemWireError::new(
            "LoadBlock inclusive texel count exceeds 2048",
        ));
    }
    let start_texel = u32::from(source_t.raw())
        .checked_mul(u32::from(image.width()))
        .and_then(|value| value.checked_add(u32::from(source_s.raw())))
        .ok_or(TmemWireError::new(
            "LoadBlock source texel offset overflows",
        ))?;
    let range = source_range(layout, image, start_texel, texels, "LoadBlock source range")?;
    let (source_plan, accesses) = source_accesses(
        source_identity,
        first_access_index,
        first_operation,
        vec![range],
    )?;
    let size = TileSize::from_wire(
        source_s,
        source_t,
        high_s,
        coordinate(u32::from(dxt.get()))?,
    );
    let load = TmemLoad::new(
        epoch,
        tile,
        image,
        descriptor,
        TmemLoadKind::Block {
            source_s,
            source_t,
            high_s,
            dxt,
        },
        source_plan,
    );
    state.commit_load(load, size);
    Ok((TmemCommand::LoadBlock(load), accesses))
}

fn decode_load_tile(
    w0: u32,
    w1: u32,
    layout: PhysicalMemoryLayout,
    state: &mut TmemState,
    source_identity: TmemLoadSourceIdentity,
    first_access_index: u32,
    first_operation: OperationId,
) -> Result<(TmemCommand, Vec<ResourceAccess>), TmemWireError> {
    let tile = tile_index(w1)?;
    let (epoch, image, descriptor) = state
        .load_inputs(tile, layout)
        .map_err(TmemWireError::new)?;
    let bounds = tile_size(w0, w1)?;
    let low_s = u32::from(bounds.low_s().integer());
    let low_t = u32::from(bounds.low_t().integer());
    let high_s = u32::from(bounds.high_s().integer());
    let high_t = u32::from(bounds.high_t().integer());
    if high_s < low_s || high_t < low_t {
        return Err(TmemWireError::new("LoadTile source bounds are inverted"));
    }
    if high_s >= u32::from(image.width()) {
        return Err(TmemWireError::new(
            "LoadTile source bounds exceed the texture-image width",
        ));
    }
    let width = high_s - low_s + 1;
    let ranges = if low_s == 0 && width == u32::from(image.width()) {
        let first = low_t
            .checked_mul(u32::from(image.width()))
            .ok_or(TmemWireError::new("LoadTile source texel offset overflows"))?;
        let rows = high_t - low_t + 1;
        let texels = rows
            .checked_mul(width)
            .ok_or(TmemWireError::new("LoadTile source texel count overflows"))?;
        vec![source_range(
            layout,
            image,
            first,
            texels,
            "LoadTile source range",
        )?]
    } else {
        let rows = usize::try_from(high_t - low_t + 1)
            .map_err(|_| TmemWireError::new("LoadTile row count overflows usize"))?;
        if rows > MAX_RESOURCE_ACCESSES {
            return Err(TmemWireError::new(
                "LoadTile exceeds the bounded resource-plan access count",
            ));
        }
        (low_t..=high_t)
            .map(|row| {
                let first = row
                    .checked_mul(u32::from(image.width()))
                    .and_then(|value| value.checked_add(low_s))
                    .ok_or(TmemWireError::new("LoadTile source texel offset overflows"))?;
                source_range(layout, image, first, width, "LoadTile source row")
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let (source_plan, accesses) =
        source_accesses(source_identity, first_access_index, first_operation, ranges)?;
    let load = TmemLoad::new(
        epoch,
        tile,
        image,
        descriptor,
        TmemLoadKind::Tile { bounds },
        source_plan,
    );
    state.commit_load(load, bounds);
    Ok((TmemCommand::LoadTile(load), accesses))
}

fn decode_load_tlut(
    w0: u32,
    w1: u32,
    layout: PhysicalMemoryLayout,
    state: &mut TmemState,
    source_identity: TmemLoadSourceIdentity,
    first_access_index: u32,
    first_operation: OperationId,
) -> Result<(TmemCommand, Vec<ResourceAccess>), TmemWireError> {
    let tile = tile_index(w1)?;
    let (epoch, image, descriptor) = state
        .load_inputs(tile, layout)
        .map_err(TmemWireError::new)?;
    if descriptor.tmem().get() < 256 {
        return Err(TmemWireError::new(
            "LoadTLUT destination tile is outside high TMEM",
        ));
    }
    let low_s = (w0 >> 12) & 0x0fff;
    let low_t = w0 & 0x0fff;
    let count_fraction = (w1 >> 12) & 3;
    let high_t = w1 & 0x0fff;
    if low_s >> 2 != 0 {
        return Err(TmemWireError::new(
            "LoadTLUT public macro requires a zero SL origin",
        ));
    }
    if low_t >> 2 != 0 {
        return Err(TmemWireError::new(
            "LoadTLUT public macro requires a zero TL origin",
        ));
    }
    if low_s & 3 != 0 || low_t & 3 != 0 || count_fraction != 0 {
        return Err(TmemWireError::new(
            "LoadTLUT public macro requires zero fractional coordinate fields",
        ));
    }
    if high_t != 0 {
        return Err(TmemWireError::new(
            "LoadTLUT public macro requires a zero TH field",
        ));
    }
    let entries =
        TlutEntryCount::try_new((((w1 >> 14) & 0x03ff) + 1) as u16).map_err(TmemWireError::new)?;
    let bounds = tile_size(w0, w1)?;
    let bytes = u32::from(entries.get())
        .checked_mul(2)
        .ok_or(TmemWireError::new("LoadTLUT source byte count overflows"))?;
    let start = image.address().get();
    let end = start
        .checked_add(bytes)
        .ok_or(TmemWireError::new("LoadTLUT source range overflows"))?;
    let range = layout
        .range(start, end)
        .map_err(|_| TmemWireError::new("LoadTLUT source range is outside installed RDRAM"))?;
    let (source_plan, accesses) = source_accesses(
        source_identity,
        first_access_index,
        first_operation,
        vec![range],
    )?;
    let load = TmemLoad::new(
        epoch,
        tile,
        image,
        descriptor,
        TmemLoadKind::Tlut { bounds, entries },
        source_plan,
    );
    state.commit_load(load, bounds);
    Ok((TmemCommand::LoadTlut(load), accesses))
}

fn source_accesses(
    identity: TmemLoadSourceIdentity,
    first_access_index: u32,
    first_operation: OperationId,
    ranges: Vec<PhysicalRange>,
) -> Result<(TmemLoadSourcePlan, Vec<ResourceAccess>), TmemWireError> {
    if ranges.is_empty() || ranges.len() > MAX_RESOURCE_ACCESSES {
        return Err(TmemWireError::new(
            "TMEM load source plan has an invalid access count",
        ));
    }
    let access_count = u16::try_from(ranges.len())
        .map_err(|_| TmemWireError::new("TMEM load source access count exceeds u16"))?;
    let total_bytes = ranges.iter().try_fold(0_u32, |total, range| {
        total
            .checked_add(range.len())
            .ok_or(TmemWireError::new("TMEM load source byte count overflows"))
    })?;
    let mut accesses = Vec::with_capacity(ranges.len());
    for (offset, range) in ranges.into_iter().enumerate() {
        let operation = first_operation
            .get()
            .checked_add(
                u32::try_from(offset)
                    .map_err(|_| TmemWireError::new("TMEM load operation offset overflows"))?,
            )
            .ok_or(TmemWireError::new("TMEM load operation identity overflows"))?;
        accesses.push(
            ResourceAccess::try_new(
                OperationId::new(operation),
                AccessMode::Read,
                AccessPurpose::TmemLoadSource,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range,
                },
            )
            .expect("TMEM source access uses the IR-admitted mode and resource"),
        );
    }
    let source_access_identity = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(accesses.len(), total_bytes)
            .expect("TMEM source plan is nonempty and already bounded"),
        accesses.clone(),
    )
    .expect("TMEM source accesses already satisfy the IR journal contract")
    .identity();
    Ok((
        TmemLoadSourcePlan::new(
            identity,
            source_access_identity,
            first_access_index,
            first_operation,
            access_count,
            total_bytes,
        ),
        accesses,
    ))
}

fn source_range(
    layout: PhysicalMemoryLayout,
    image: TextureImage,
    first_texel: u32,
    texel_count: u32,
    context: &'static str,
) -> Result<PhysicalRange, TmemWireError> {
    let (first_byte, end_byte) = match image.size() {
        PixelSize::Bits4 => {
            let last_texel = first_texel
                .checked_add(texel_count)
                .and_then(|value| value.checked_sub(1))
                .ok_or(TmemWireError::new("TMEM load source texel span overflows"))?;
            (first_texel / 2, last_texel / 2 + 1)
        }
        size => {
            let bytes_per_texel = size
                .bytes_per_pixel()
                .expect("only four-bit pixels are sub-byte");
            let first = first_texel
                .checked_mul(bytes_per_texel)
                .ok_or(TmemWireError::new("TMEM load source byte offset overflows"))?;
            let bytes = texel_count
                .checked_mul(bytes_per_texel)
                .ok_or(TmemWireError::new("TMEM load source byte count overflows"))?;
            let end = first
                .checked_add(bytes)
                .ok_or(TmemWireError::new("TMEM load source byte end overflows"))?;
            (first, end)
        }
    };
    let start = image
        .address()
        .get()
        .checked_add(first_byte)
        .ok_or(TmemWireError::new(context))?;
    let end = image
        .address()
        .get()
        .checked_add(end_byte)
        .ok_or(TmemWireError::new(context))?;
    layout
        .range(start, end)
        .map_err(|_| TmemWireError::new(context))
}

fn tile_size(w0: u32, w1: u32) -> Result<TileSize, TmemWireError> {
    Ok(TileSize::from_wire(
        coordinate((w0 >> 12) & 0x0fff)?,
        coordinate(w0 & 0x0fff)?,
        coordinate((w1 >> 12) & 0x0fff)?,
        coordinate(w1 & 0x0fff)?,
    ))
}

fn tile_index(w1: u32) -> Result<TileIndex, TmemWireError> {
    TileIndex::try_new(((w1 >> 24) & 7) as u8).map_err(TmemWireError::new)
}

fn coordinate(raw: u32) -> Result<TileCoordinate, TmemWireError> {
    TileCoordinate::try_new(raw as u16).map_err(TmemWireError::new)
}

fn image_format(raw: u32) -> Result<ImageFormat, TmemWireError> {
    match raw {
        0 => Ok(ImageFormat::Rgba),
        1 => Ok(ImageFormat::Yuv),
        2 => Ok(ImageFormat::ColorIndex),
        3 => Ok(ImageFormat::IntensityAlpha),
        4 => Ok(ImageFormat::Intensity),
        _ => Err(TmemWireError::new("image format is reserved")),
    }
}

const fn pixel_size(raw: u32) -> PixelSize {
    match raw {
        0 => PixelSize::Bits4,
        1 => PixelSize::Bits8,
        2 => PixelSize::Bits16,
        _ => PixelSize::Bits32,
    }
}
