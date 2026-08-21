//! Public RDP texture-command wire fields.
//!
//! Opcode and field masks are transcribed from the public SGI *Nintendo 64
//! RDP Command Summary*, Tables 1, 3, and 6–10. LoadTLUT's restricted shape
//! and ten-bit `count - 1` field follow public libultra `gbi.h`'s
//! `gDPLoadTLUTCmd` macro. Fn64's M4.0/M4.1 design documents define ownership
//! and transactional staging; RT64 is not a hardware authority here.

use fn64_render_ir::{
    AccessMode, AccessPurpose, OperationId, PhysicalMemoryLayout, PhysicalRange, RdramResource,
    ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion, TmemRange,
    MAX_RESOURCE_ACCESSES,
};

use crate::{ImageFormat, PixelSize};

use super::types::project_tmem_transfer_word;
use super::{
    TextureImage, TileAddressMode, TileCoordinate, TileDescriptor, TileIndex, TileSize,
    TlutEntryCount, TmemDxt, TmemLoad, TmemLoadDestinationPlan, TmemLoadKind,
    TmemLoadSourceIdentity, TmemLoadSourcePlan, TmemState, TmemTransferLayout,
    TmemTransferPhysicalWord, TmemTransferPlan, TmemWordAddress,
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
    // Padded to whole 64-bit words: hardware copies whole words, so the last
    // word of the block reads its remaining bytes from adjacent RDRAM. See
    // `source_row_range`.
    let block_bytes = texel_bytes(image.size(), texels)?
        .div_ceil(8)
        .checked_mul(8)
        .ok_or(TmemWireError::new("LoadBlock padded byte count overflows"))?;
    let range = source_row_range(
        layout,
        image,
        start_texel,
        block_bytes,
        "LoadBlock source range",
    )?;
    let (source_plan, mut accesses) = source_accesses(
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
    let kind = TmemLoadKind::Block {
        source_s,
        source_t,
        high_s,
        dxt,
    };
    // M4.1 retains the exact source plan, but M4.2.0 does not guess at YUV
    // pairing or descriptor constraints without a frozen public contract.
    let load = if image.format() == ImageFormat::Yuv {
        TmemLoad::new_deferred_yuv(epoch, tile, image, descriptor, kind, source_plan)
    } else {
        let transfer_plan = transfer_plan(
            source_plan,
            TransferInputs {
                epoch,
                tile,
                image,
                descriptor,
                kind,
            },
            first_access_index,
            first_operation,
            &mut accesses,
        )?;
        TmemLoad::new(epoch, tile, image, descriptor, kind, transfer_plan)
    };
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
    // **One padded access PER ROW, including the full-width case.**
    //
    // The collapsed single-range form this used for full-width loads cannot
    // express row-local padding: each row's last 64-bit word reads bytes
    // beyond that row's logical texels, so the reads are per-row spans with
    // gaps, not one contiguous run. Hardware drives `wordsPerRow` whole-word
    // copies per row (`rt64_rdp.cpp:459-468`), which is exactly this shape.
    let row_bytes = texel_bytes(image.size(), width)?;
    let padded_row_bytes = row_bytes
        .div_ceil(8)
        .checked_mul(8)
        .ok_or(TmemWireError::new(
            "LoadTile padded row byte count overflows",
        ))?;
    let ranges = {
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
                source_row_range(
                    layout,
                    image,
                    first,
                    padded_row_bytes,
                    "LoadTile source row",
                )
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let (source_plan, mut accesses) =
        source_accesses(source_identity, first_access_index, first_operation, ranges)?;
    let kind = TmemLoadKind::Tile { bounds };
    // M4.1 retains the exact source plan, but M4.2.0 does not guess at YUV
    // pairing or descriptor constraints without a frozen public contract.
    let load = if image.format() == ImageFormat::Yuv {
        TmemLoad::new_deferred_yuv(epoch, tile, image, descriptor, kind, source_plan)
    } else {
        let transfer_plan = transfer_plan(
            source_plan,
            TransferInputs {
                epoch,
                tile,
                image,
                descriptor,
                kind,
            },
            first_access_index,
            first_operation,
            &mut accesses,
        )?;
        TmemLoad::new(epoch, tile, image, descriptor, kind, transfer_plan)
    };
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
    // Public libultra `gbi.h`, the `gDPLoadTLUT_pal16` / `gDPLoadTLUT_pal256`
    // / `gDPLoadTLUT` macro bodies: every one emits
    // `gDPSetTextureImage(..., G_IM_FMT_RGBA, G_IM_SIZ_16b, 1, dram)`, so the
    // SOURCE image is 16-bit per palette entry and this module's
    // transfer-shape (`entries.get() * 2` source bytes) is sized for exactly
    // that. Reject any other `SetTextureImage` size explicitly rather than
    // silently mis-sizing the transfer -- `AGENTS.md`'s "loud traps, no
    // silent shrugs".
    if image.size() != PixelSize::Bits16 {
        return Err(TmemWireError::new(
            "LoadTLUT public macro requires a 16-bit SetTextureImage source",
        ));
    }
    // The DESTINATION tile descriptor's `siz` is deliberately NOT constrained
    // here. The same macro bodies emit `gDPSetTile(pkt, 0, 0, 0, tmem,
    // G_TX_LOADTILE, ...)`, whose second positional argument is `siz`
    // (`gDPSetTile(pkt, fmt, siz, line, tmem, tile, ...)`), so the canonical
    // destination `siz` is `0` == `G_IM_SIZ_4b` -- never 16-bit. A previous
    // revision required `Bits16` here and refused every canonical
    // `gDPLoadTLUT_pal16`; the load tile describes a TMEM region for a
    // quadricated palette write, not the palette's own pixel format, and no
    // code consumes the field for this kind: `transfer_shape`'s `Tlut` arm
    // sizes from `entries` and `image.size()`, and
    // `project_tmem_transfer_word`'s `Tlut` arm reads only `descriptor.tmem()`
    // and `line_words()`.
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
    let (source_plan, mut accesses) = source_accesses(
        source_identity,
        first_access_index,
        first_operation,
        vec![range],
    )?;
    let kind = TmemLoadKind::Tlut { bounds, entries };
    // M4.3.1 closes LoadTLUT's destination/quadrication claim using the same
    // M4.2.0 transfer-plan mechanism Block/Tile already use (see
    // `transfer_shape`'s and `project_tmem_transfer_word`'s `Tlut` arms).
    let transfer_plan = transfer_plan(
        source_plan,
        TransferInputs {
            epoch,
            tile,
            image,
            descriptor,
            kind,
        },
        first_access_index,
        first_operation,
        &mut accesses,
    )?;
    let load = TmemLoad::new(epoch, tile, image, descriptor, kind, transfer_plan);
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

#[derive(Clone, Copy)]
struct TransferShape {
    layout: TmemTransferLayout,
    logical_source_bytes: u32,
    transfer_words: u16,
    undefined_padding_bytes: u32,
    words_per_row: u16,
    row_count: u16,
}

#[derive(Clone, Copy)]
struct TransferInputs {
    epoch: super::TmemLoadEpoch,
    tile: TileIndex,
    image: TextureImage,
    descriptor: TileDescriptor,
    kind: TmemLoadKind,
}

fn transfer_plan(
    source: TmemLoadSourcePlan,
    inputs: TransferInputs,
    first_access_index: u32,
    first_operation: OperationId,
    accesses: &mut Vec<ResourceAccess>,
) -> Result<TmemTransferPlan, TmemWireError> {
    let shape = transfer_shape(source, inputs.image, inputs.descriptor, inputs.kind)?;
    let destination_ranges = destination_ranges(inputs.descriptor, inputs.kind, shape)?;
    let first_destination_access = first_access_index
        .checked_add(u32::from(source.access_count()))
        .ok_or(TmemWireError::new(
            "TMEM destination access identity overflows",
        ))?;
    let first_destination_operation = first_operation
        .get()
        .checked_add(u32::from(source.access_count()))
        .ok_or(TmemWireError::new(
            "TMEM destination operation identity overflows",
        ))?;
    if accesses
        .len()
        .checked_add(destination_ranges.len())
        .is_none_or(|count| count > MAX_RESOURCE_ACCESSES)
    {
        return Err(TmemWireError::new(
            "TMEM transfer exceeds the bounded resource-plan access count",
        ));
    }
    let total_bytes = destination_ranges.iter().try_fold(0_u32, |total, range| {
        total
            .checked_add(range.len())
            .ok_or(TmemWireError::new("TMEM destination byte count overflows"))
    })?;
    let mut destination_accesses = Vec::with_capacity(destination_ranges.len());
    for (offset, range) in destination_ranges.into_iter().enumerate() {
        let offset = u32::try_from(offset)
            .map_err(|_| TmemWireError::new("TMEM destination operation offset overflows"))?;
        let operation =
            first_destination_operation
                .checked_add(offset)
                .ok_or(TmemWireError::new(
                    "TMEM destination operation identity overflows",
                ))?;
        destination_accesses.push(
            ResourceAccess::try_new(
                OperationId::new(operation),
                AccessMode::Write,
                AccessPurpose::TmemLoadDestination,
                ResourceRegion::Tmem(range),
            )
            .expect("TMEM destination uses the IR-admitted mode and resource"),
        );
    }
    let access_count = u16::try_from(destination_accesses.len())
        .map_err(|_| TmemWireError::new("TMEM destination access count exceeds u16"))?;
    let destination_access_identity = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(destination_accesses.len(), total_bytes)
            .expect("TMEM destination plan is nonempty and bounded"),
        destination_accesses.clone(),
    )
    .expect("TMEM destination accesses satisfy the IR journal contract")
    .identity();
    accesses.extend(destination_accesses);
    let destination = TmemLoadDestinationPlan::new(
        source.identity(),
        source.source_access_identity(),
        destination_access_identity,
        first_destination_access,
        OperationId::new(first_destination_operation),
        access_count,
        total_bytes,
    );
    Ok(TmemTransferPlan::new(
        source,
        destination,
        shape.logical_source_bytes,
        shape.transfer_words,
        shape.undefined_padding_bytes,
        shape.words_per_row,
        shape.row_count,
        shape.layout,
        inputs.kind,
        inputs.epoch,
        inputs.tile,
        inputs.image,
        inputs.descriptor,
    ))
}

fn transfer_shape(
    source: TmemLoadSourcePlan,
    image: TextureImage,
    descriptor: TileDescriptor,
    kind: TmemLoadKind,
) -> Result<TransferShape, TmemWireError> {
    if image.format() == ImageFormat::Yuv {
        return Err(TmemWireError::new(
            "YUV destination execution is deferred pending a public pairing contract",
        ));
    }
    let layout = if image.size() == PixelSize::Bits32 {
        if descriptor.tmem().get() >= 256 {
            return Err(TmemWireError::new(
                "split-bank TMEM load base is outside low TMEM",
            ));
        }
        TmemTransferLayout::SplitBanks64
    } else {
        TmemTransferLayout::Linear64
    };
    let (logical_source_bytes, transfer_words, undefined_padding_bytes, words_per_row, row_count) =
        match kind {
            TmemLoadKind::Block {
                source_s, high_s, ..
            } => {
                // **Derived from the COMMAND, not from `source.total_bytes()`.**
                //
                // The two are equal today, and the invariant below still
                // asserts it. But the source plan is about to carry PADDED DMA
                // bytes rather than logical texel bytes -- hardware copies
                // whole 64-bit words (`rt64_rdp.cpp`'s `loadWord`: "Copy the
                // entire word", `for i in 0..8`) -- and a logical length read
                // back out of the padded plan would be circular. The Tile arm
                // below already derives its own geometry this way; this makes
                // Block match, with no behaviour change.
                //
                // Same inclusive span the decoder computes for the source
                // range (`decode_load_block`: `high_s - source_s + 1`).
                let texels = u32::from(
                    high_s
                        .raw()
                        .checked_sub(source_s.raw())
                        .ok_or(TmemWireError::new("LoadBlock source span is inverted"))?,
                ) + 1;
                let logical = texel_bytes(image.size(), texels)?;
                let words = logical.div_ceil(8);
                let written = words.checked_mul(8).ok_or(TmemWireError::new(
                    "LoadBlock transfer byte count overflows",
                ))?;
                let padding = written.checked_sub(logical).ok_or(TmemWireError::new(
                    "LoadBlock padding byte count underflows",
                ))?;
                (logical, words, padding, words, 1)
            }
            TmemLoadKind::Tile { bounds } => {
                let width = u32::from(bounds.high_s().integer())
                    .checked_sub(u32::from(bounds.low_s().integer()))
                    .and_then(|span| span.checked_add(1))
                    .ok_or(TmemWireError::new("LoadTile transfer width underflows"))?;
                let rows = u32::from(bounds.high_t().integer())
                    .checked_sub(u32::from(bounds.low_t().integer()))
                    .and_then(|span| span.checked_add(1))
                    .ok_or(TmemWireError::new("LoadTile transfer height underflows"))?;
                let row_bytes = texel_bytes(image.size(), width)?;
                let words_per_row = row_bytes.div_ceil(8);
                let words = words_per_row
                    .checked_mul(rows)
                    .ok_or(TmemWireError::new("LoadTile transfer word count overflows"))?;
                let logical = row_bytes
                    .checked_mul(rows)
                    .ok_or(TmemWireError::new("LoadTile logical byte count overflows"))?;
                let written = words
                    .checked_mul(8)
                    .ok_or(TmemWireError::new("LoadTile transfer byte count overflows"))?;
                let padding = written
                    .checked_sub(logical)
                    .ok_or(TmemWireError::new("LoadTile padding byte count underflows"))?;
                (logical, words, padding, words_per_row, rows)
            }
            TmemLoadKind::Tlut { entries, .. } => {
                // SGI RDP Command Summary Table 9 / libultra gbi.h
                // gDPLoadTLUTCmd: the source is `entries` sequential 16-bit
                // palette values (2 bytes/entry, already validated by
                // `decode_load_tlut`'s `entries.get() * 2` source-byte
                // computation). Each entry occupies exactly one 8-byte TMEM
                // destination word (quadricated -- see
                // `project_tmem_transfer_word`'s `Tlut` arm and this
                // repository's own `write_tlut` precedent), so the transfer
                // is one word per entry with no row grouping.
                let logical = source.total_bytes();
                let words = u32::from(entries.get());
                // Every transfer-word byte is defined destination content
                // (the entry's 2 real source bytes plus their 3 quadricated
                // copies) -- none are `undefined_padding_bytes` in the sense
                // that field documents (a value the logical source span does
                // not define). `defined_source_byte_mask`'s `Tlut` arm
                // reports only the 2 *captured* source bytes (`0x03`), a
                // distinct, narrower fact from this padding count.
                (logical, words, 0, words, 1)
            }
        };
    // **The source plan carries what the DMA READS, and that is kind-specific.**
    //
    // Block and Tile read whole 64-bit words -- `logical` plus
    // `undefined_padding_bytes` -- because hardware copies whole words and a
    // short row tail still reads the adjacent RDRAM
    // (`rt64_rdp.cpp:369-397`'s "Copy the entire word"). TLUT is the
    // exception: it captures exactly two source bytes per entry and derives
    // the other six by quadrication, so its plan stays logical.
    let expected_source_bytes = match kind {
        TmemLoadKind::Tlut { .. } => logical_source_bytes,
        TmemLoadKind::Block { .. } | TmemLoadKind::Tile { .. } => logical_source_bytes
            .checked_add(undefined_padding_bytes)
            .ok_or(TmemWireError::new("TMEM padded source bytes overflow"))?,
    };
    if expected_source_bytes != source.total_bytes() {
        return Err(TmemWireError::new(
            "TMEM transfer bytes differ from the exact source plan",
        ));
    }
    let transfer_words = u16::try_from(transfer_words)
        .map_err(|_| TmemWireError::new("TMEM transfer word count exceeds u16"))?;
    let words_per_row = u16::try_from(words_per_row)
        .map_err(|_| TmemWireError::new("TMEM transfer row word count exceeds u16"))?;
    let row_count = u16::try_from(row_count)
        .map_err(|_| TmemWireError::new("TMEM transfer row count exceeds u16"))?;
    Ok(TransferShape {
        layout,
        logical_source_bytes,
        transfer_words,
        undefined_padding_bytes,
        words_per_row,
        row_count,
    })
}

fn destination_ranges(
    descriptor: TileDescriptor,
    kind: TmemLoadKind,
    shape: TransferShape,
) -> Result<Vec<TmemRange>, TmemWireError> {
    let mut ranges = Vec::new();
    for word in 0..shape.transfer_words {
        let geometry = project_tmem_transfer_word(
            descriptor,
            kind,
            shape.layout,
            shape.transfer_words,
            shape.words_per_row,
            word,
        )
        .map_err(TmemWireError::new)?;
        match geometry.physical() {
            TmemTransferPhysicalWord::Linear(range) => ranges.push(range),
            TmemTransferPhysicalWord::SplitBanks { low, high } => {
                ranges.push(low);
                ranges.push(high);
            }
        }
    }
    canonical_destination_union(ranges)
}

fn canonical_destination_union(
    mut ranges: Vec<TmemRange>,
) -> Result<Vec<TmemRange>, TmemWireError> {
    ranges.sort_unstable_by_key(|range| (range.start(), range.end()));
    let mut union: Vec<TmemRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = union.last_mut() {
            if range.start() <= last.end() {
                let end = last.end().max(range.end());
                *last = TmemRange::try_new(last.start(), end).map_err(|_| {
                    TmemWireError::new("canonical TMEM destination union is outside TMEM")
                })?;
                continue;
            }
        }
        union.push(range);
    }
    Ok(union)
}

fn texel_bytes(size: PixelSize, texels: u32) -> Result<u32, TmemWireError> {
    match size {
        PixelSize::Bits4 => Err(TmemWireError::new(
            "direct four-bit TMEM loads are unsupported; load through a public 16-bit form",
        )),
        size => texels
            .checked_mul(size.bytes_per_pixel().expect("non-four-bit size has bytes"))
            .ok_or(TmemWireError::new("TMEM logical byte count overflows")),
    }
}

/// The DMA source range for one transfer row, in BYTES.
///
/// **Hardware copies whole 64-bit words**, so a row whose logical texels do
/// not fill its last word still reads that word's remaining bytes from the
/// adjacent RDRAM -- verified in the pinned RT64 oracle's live loader, where
/// `loadWord` is commented "Copy the entire word" and loops `i < 8`
/// (`src/hle/rt64_rdp.cpp:369-397`), driven `wordsPerRow` times per row
/// (`:459-468`). There is no partial-tail concept and no clamp.
///
/// The byte count is therefore `words * 8`, never the logical texel span.
/// A padded word that runs past installed RDRAM must FAIL rather than clamp:
/// clamping would hand the executor fewer than eight bytes and recreate the
/// partial-word ambiguity this exists to remove. `PhysicalMemoryLayout::range`
/// supplies that rejection.
fn source_row_range(
    layout: PhysicalMemoryLayout,
    image: TextureImage,
    first_texel: u32,
    byte_count: u32,
    context: &'static str,
) -> Result<PhysicalRange, TmemWireError> {
    let bytes_per_texel = match image.size() {
        PixelSize::Bits4 => {
            return Err(TmemWireError::new(
                "direct four-bit TMEM loads are unsupported; load through a public 16-bit form",
            ));
        }
        size => size
            .bytes_per_pixel()
            .expect("only four-bit pixels are sub-byte"),
    };
    let first_byte = first_texel
        .checked_mul(bytes_per_texel)
        .ok_or(TmemWireError::new("TMEM load source byte offset overflows"))?;
    let end_byte = first_byte
        .checked_add(byte_count)
        .ok_or(TmemWireError::new("TMEM load source byte end overflows"))?;
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

fn source_range(
    layout: PhysicalMemoryLayout,
    image: TextureImage,
    first_texel: u32,
    texel_count: u32,
    context: &'static str,
) -> Result<PhysicalRange, TmemWireError> {
    let (first_byte, end_byte) = match image.size() {
        PixelSize::Bits4 => {
            return Err(TmemWireError::new(
                "direct four-bit TMEM loads are unsupported; load through a public 16-bit form",
            ));
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

#[cfg(test)]
mod tlut_transfer_plan_tests {
    //! M4.3.1: LoadTLUT destination transfer-plan tests. These decode
    //! through the crate's public `decode_raw_dpc` entry point (the same
    //! depth `raw_dpc::tests` and `tmem::physical::tests` already exercise)
    //! rather than hand-assembling `TmemSourcePlanStart`'s identity/journal
    //! plumbing, which is production-wired only inside `raw_dpc/mod.rs`
    //! (out of this task's writable scope). Only wire-layer geometry is
    //! asserted here; palette-byte decode is M4.3.4/M4.3.5 and physical
    //! execution is M4.3.2 -- neither is claimed by these tests.

    use fn64_render_ir::{
        AccessPurpose, CapturedGuestRead, DecodedTicket, DeferredGuestReadCapture,
        DpInterruptState, DramCommandChunk, DramCommandStream, PhysicalMemoryLayout,
        RawCommandStream, ResourceAccess, ResourceJournal, ResourceJournalLimits, SubmittedTicket,
        TemporalBoundary, TicketAuthoritySet, WorkloadAdmission, WorkloadPacket,
        WorkloadPacketPreflight, MAX_RESOURCE_ACCESSES,
    };

    use super::*;
    use crate::{decode_raw_dpc, RawDpcCommandKind, RawDpcDecodeError, RdpState};

    const LAYOUT_BYTES: u32 = 0x4000;
    const COMMAND_START: u32 = 0x1000;

    fn word(opcode: u8, payload: u32) -> u32 {
        u32::from(opcode) << 24 | payload
    }

    /// RGBA16 SetTextureImage: TLUT sources are always a 16-bit-per-entry
    /// palette image, which `decode_load_tlut` now explicitly requires (see
    /// `source_size_is_rejected_unless_bits16` below); `transfer_shape`
    /// always resolves `Linear64` for LoadTLUT as a result.
    fn set_texture_image(width: u32, address: u32) -> [u32; 2] {
        // format=RGBA (0), size=16-bit (2).
        [word(SET_TEXTURE_IMAGE, 2 << 19 | (width - 1)), address]
    }

    /// Like `set_texture_image`, but with an explicit wire size field (0-3,
    /// `pixel_size`'s raw encoding) for the Bits4/Bits8/Bits32 hostiles.
    fn set_texture_image_sized(size: u32, width: u32, address: u32) -> [u32; 2] {
        [word(SET_TEXTURE_IMAGE, size << 19 | (width - 1)), address]
    }

    /// Like `set_tile`, but with an explicit wire size field for the
    /// Bits4/Bits8/Bits32 destination-descriptor hostiles.
    fn set_tile_sized(size: u32, tile: u32, tmem: u32) -> [u32; 2] {
        [word(SET_TILE, size << 19 | tmem), tile << 24]
    }

    fn set_tile(tile: u32, tmem: u32) -> [u32; 2] {
        [word(SET_TILE, 2 << 19 | tmem), tile << 24]
    }

    fn load_sync() -> [u32; 2] {
        [word(LOAD_SYNC, 0), 0]
    }

    fn load_tlut(tile: u32, entries_minus_one: u32) -> [u32; 2] {
        [word(LOAD_TLUT, 0), tile << 24 | entries_minus_one << 14]
    }

    fn command_access(
        layout: PhysicalMemoryLayout,
        byte_count: u32,
        operation: u32,
    ) -> ResourceAccess {
        ResourceAccess::try_new(
            OperationId::new(operation),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: layout
                    .range(COMMAND_START, COMMAND_START + byte_count)
                    .unwrap(),
            },
        )
        .unwrap()
    }

    fn tmem_source_access(
        layout: PhysicalMemoryLayout,
        operation: u32,
        start: u32,
        end: u32,
    ) -> ResourceAccess {
        ResourceAccess::try_new(
            OperationId::new(operation),
            AccessMode::Read,
            AccessPurpose::TmemLoadSource,
            ResourceRegion::Rdram {
                resource: RdramResource::Buffer,
                range: layout.range(start, end).unwrap(),
            },
        )
        .unwrap()
    }

    fn submit(packet: WorkloadPacket) -> SubmittedTicket {
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        queue.submit(DecodedTicket::new(packet)).unwrap()
    }

    /// A command-only packet with no declared TMEM source access, for
    /// hostile fixtures whose decode is expected to reject the command
    /// before it ever reaches source-range construction (mirrors
    /// `raw_dpc::tests::packet`, used the same way there).
    fn hostile_packet(words: Vec<u32>) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let bytes = u32::try_from(words.len() * 4).unwrap();
        let command_range = layout.range(COMMAND_START, COMMAND_START + bytes).unwrap();
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                command_range,
                words,
                TemporalBoundary::new(1, DpInterruptState::Clear),
                Vec::new(),
            )
            .unwrap()])
            .unwrap(),
        );
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(MAX_RESOURCE_ACCESSES, LAYOUT_BYTES).unwrap(),
            vec![command_access(layout, bytes, 0)],
        )
        .unwrap();
        WorkloadPacket::try_new(
            layout,
            WorkloadAdmission::RawDpc {
                transaction_sequence: 7,
            },
            vec![stream],
            journal,
        )
        .unwrap()
    }

    /// Mirrors `raw_dpc::tests::packet_with_tmem_sources`: this task's
    /// destination accesses are only known once decode runs, so the packet
    /// is built with a source-only journal guess, probed once, and rebuilt
    /// from the exact expected journal `decode_raw_dpc` reports back on
    /// mismatch.
    fn packet_with_tmem_sources(words: Vec<u32>, source_ranges: &[(u32, u32)]) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let bytes = u32::try_from(words.len() * 4).unwrap();
        let command_range = layout.range(COMMAND_START, COMMAND_START + bytes).unwrap();
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                command_range,
                words,
                TemporalBoundary::new(1, DpInterruptState::Clear),
                Vec::new(),
            )
            .unwrap()])
            .unwrap(),
        );
        let mut accesses = vec![command_access(layout, bytes, 0)];
        accesses.extend(
            source_ranges
                .iter()
                .enumerate()
                .map(|(index, &(start, end))| {
                    tmem_source_access(layout, index as u32 + 1, start, end)
                }),
        );
        let finalize = |accesses: Vec<ResourceAccess>| {
            let declared = accesses
                .iter()
                .map(|access| access.region().declared_bytes())
                .sum::<u32>();
            let journal = ResourceJournal::try_new(
                ResourceJournalLimits::try_new(MAX_RESOURCE_ACCESSES, declared.max(1)).unwrap(),
                accesses,
            )
            .unwrap();
            let preflight = WorkloadPacketPreflight::try_new(
                layout,
                WorkloadAdmission::RawDpc {
                    transaction_sequence: 7,
                },
                vec![stream.clone()],
                journal,
            )
            .unwrap();
            let capture = DeferredGuestReadCapture::new(
                preflight
                    .guest_read_plan()
                    .reads()
                    .iter()
                    .map(|read| {
                        CapturedGuestRead::try_new(
                            *read,
                            vec![read.operation().get() as u8; read.range().len() as usize],
                        )
                        .unwrap()
                    })
                    .collect(),
            );
            preflight.finalize(capture).unwrap()
        };
        let probe = finalize(accesses.clone());
        let final_accesses = match decode_raw_dpc(submit(probe), &RdpState::default()) {
            Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
            Ok(_) => accesses,
            Err(error) => {
                panic!("TMEM packet planning probe failed before journal comparison: {error}")
            }
        };
        finalize(final_accesses)
    }

    fn decode_tlut(
        tmem_base: u32,
        entries: u16,
        source_range: (u32, u32),
    ) -> Result<TmemLoad, RawDpcDecodeError> {
        let mut words = Vec::new();
        words.extend(set_texture_image(1, 0x300));
        words.extend(set_tile(7, tmem_base));
        words.extend(load_sync());
        words.extend(load_tlut(7, u32::from(entries) - 1));
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(words, &[source_range])),
            &RdpState::default(),
        )?;
        let RawDpcCommandKind::LoadTlut(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadTLUT");
        };
        Ok(load)
    }

    #[test]
    fn minimum_single_entry_produces_one_destination_word() {
        let load = decode_tlut(256, 1, (0x300, 0x302)).unwrap();
        let plan = load
            .transfer_plan()
            .expect("M4.3.1 closes LoadTLUT's transfer plan");
        assert_eq!(plan.transfer_words(), 1);
        assert_eq!(plan.logical_source_bytes(), 2);
        assert_eq!(plan.destination_word(0).unwrap(), 256);
        assert_eq!(plan.undefined_padding_bytes(), 0);
        // 8 destination bytes are defined content (quadrication), but only
        // the entry's 2 real bytes are *captured* source bytes.
        assert_eq!(plan.defined_source_byte_mask(0).unwrap(), 0x03);
    }

    #[test]
    fn maximum_256_entries_fills_the_high_bank_without_overflow() {
        let load = decode_tlut(256, 256, (0x300, 0x500)).unwrap();
        let plan = load
            .transfer_plan()
            .expect("256-entry TLUT closes a transfer plan");
        assert_eq!(plan.transfer_words(), 256);
        assert_eq!(plan.logical_source_bytes(), 512);
        assert_eq!(plan.destination_word(0).unwrap(), 256);
        assert_eq!(plan.destination_word(255).unwrap(), 511);
        for word in 0..256 {
            assert_eq!(plan.defined_source_byte_mask(word).unwrap(), 0x03);
        }
    }

    #[test]
    fn destination_base_at_the_exact_256_boundary_is_accepted() {
        let load = decode_tlut(256, 4, (0x300, 0x308)).unwrap();
        assert!(load.transfer_plan().is_ok());
    }

    #[test]
    fn destination_word_near_tmem_top_wraps_to_word_zero() {
        // tmem=511, entries=2: 511 + 1 exceeds the 512-word space.
        // M4.3.1c: the destination wraps across the full 512-word TMEM
        // domain (`project_tlut_full_domain_word`'s `& 0x01ff` mask,
        // matching this repository's own `write_tlut` full-4-KiB-byte-mask
        // precedent and the pinned RT64 reference), landing at word 0 --
        // not word 256, which M4.3.1b's now-superseded high-bank-only
        // projection produced. See `project_tlut_full_domain_word`'s doc
        // comment: this is RT64/reference parity policy, not a proven
        // silicon fact.
        let load = decode_tlut(511, 2, (0x300, 0x304)).unwrap();
        let plan = load.transfer_plan().unwrap();
        assert_eq!(plan.destination_word(0).unwrap(), 511);
        assert_eq!(plan.destination_word(1).unwrap(), 0);
    }

    #[test]
    fn destination_base_at_literal_256_stays_at_256() {
        let load = decode_tlut(256, 1, (0x300, 0x302)).unwrap();
        let plan = load.transfer_plan().unwrap();
        assert_eq!(plan.destination_word(0).unwrap(), 256);
    }

    #[test]
    fn destination_base_511_plus_one_entry_wraps_to_word_zero() {
        // M4.3.1c: base 511 followed by one more entry wraps to word 0
        // (full 512-word domain wrap), not word 256 (M4.3.1b's superseded
        // high-bank-only wrap). See `project_tlut_full_domain_word`.
        let load = decode_tlut(511, 1, (0x300, 0x302)).unwrap();
        let plan = load.transfer_plan().unwrap();
        assert_eq!(plan.destination_word(0).unwrap(), 511);

        let load = decode_tlut(511, 2, (0x300, 0x304)).unwrap();
        let plan = load.transfer_plan().unwrap();
        assert_eq!(plan.destination_word(0).unwrap(), 511);
        assert_eq!(plan.destination_word(1).unwrap(), 0);
    }

    #[test]
    fn full_256_entry_high_bank_coverage_never_wraps_within_capacity() {
        let load = decode_tlut(256, 256, (0x300, 0x500)).unwrap();
        let plan = load.transfer_plan().unwrap();
        for word in 0..256u16 {
            let destination = plan.destination_word(word).unwrap();
            assert!(
                (256..512).contains(&destination),
                "TLUT destination word {destination} at entry {word} escaped the high bank"
            );
        }
        // Base 256 with 256 entries exactly fills 256..512: entry 255 (the
        // last real entry in this load) lands at word 511, the top of TMEM,
        // with no wrap -- a 256th entry beyond this load's declared count
        // would be the one that wraps, back to word 0 under the full-domain
        // projection (see `destination_base_511_plus_one_entry_wraps_to_word_zero`).
        assert_eq!(plan.destination_word(0).unwrap(), 256);
        assert_eq!(plan.destination_word(255).unwrap(), 511);
    }

    #[test]
    fn base_511_word_511_then_word_0_boundary_is_literal() {
        // Regression for the exact bug this task repairs: base=511 with 2
        // entries must produce destination word 511 for entry 0 and word 0
        // for entry 1 -- the literal 511-then-0 boundary crossing, per the
        // RT64/`write_tlut` full-4096-byte-domain wrap policy this task
        // adopts (see `project_tlut_full_domain_word`'s doc comment).
        let load = decode_tlut(511, 2, (0x300, 0x304)).unwrap();
        let plan = load.transfer_plan().unwrap();
        assert_eq!(plan.destination_word(0).unwrap(), 511);
        assert_eq!(plan.destination_word(1).unwrap(), 0);
    }

    #[test]
    fn destination_ranges_are_disjoint_sorted_and_word_aligned() {
        let load = decode_tlut(256, 16, (0x300, 0x320)).unwrap();
        let plan = load.transfer_plan().unwrap();
        for word in 0..16u16 {
            match plan.physical_word(word).unwrap() {
                TmemTransferPhysicalWord::Linear(range) => {
                    let expected_start = (256 + u32::from(word)) * 8;
                    assert_eq!(range.start(), expected_start);
                    assert_eq!(range.end(), expected_start + 8);
                }
                TmemTransferPhysicalWord::SplitBanks { .. } => {
                    panic!("LoadTLUT destination is always linear, never split-banks")
                }
            }
        }
    }

    #[test]
    fn destination_accesses_are_purpose_tagged_and_disjoint_from_source() {
        let load = decode_tlut(256, 4, (0x300, 0x308)).unwrap();
        let plan = load.transfer_plan().unwrap();
        assert_eq!(plan.destination().access_count(), 1);
        assert_eq!(plan.destination().total_bytes(), 32);
    }

    #[test]
    fn source_plan_and_transfer_plan_agree_on_total_bytes() {
        let load = decode_tlut(256, 16, (0x300, 0x320)).unwrap();
        let plan = load.transfer_plan().unwrap();
        assert_eq!(plan.source().total_bytes(), plan.logical_source_bytes());
        assert_eq!(plan.source().total_bytes(), 32);
    }

    #[test]
    fn no_source_tail_spill_every_destination_byte_is_defined_content() {
        // Hostile per the task: a naive Block/Tile-style `div_ceil(8)`
        // padding calculation would treat 6 of every 8 destination bytes as
        // "undefined tail" since TLUT entries are 2 source bytes per 8-byte
        // destination word. Quadrication means all 8 destination bytes are
        // real, defined TMEM content -- `undefined_padding_bytes` regresses
        // that. `defined_source_byte_mask` is a distinct, narrower fact: only
        // the entry's 2 real *captured* source bytes (`0x03`), not the 6
        // quadricated copies -- this regresses that distinction too, so a
        // regression to either the old wrong padding count or a wrongly
        // widened mask is caught here.
        let load = decode_tlut(256, 3, (0x300, 0x306)).unwrap();
        let plan = load.transfer_plan().unwrap();
        assert_eq!(plan.transfer_words(), 3);
        assert_eq!(plan.written_bytes(), 24);
        assert_eq!(plan.logical_source_bytes(), 6);
        assert_eq!(plan.undefined_padding_bytes(), 0);
        for word in 0..3u16 {
            assert_eq!(plan.defined_source_byte_mask(word).unwrap(), 0x03);
        }
    }

    #[test]
    fn destination_mask_is_0xff_while_source_mask_stays_0x03() {
        // M4.3.1b: the plan-level source/destination mask split this task
        // adds. `defined_source_byte_mask` names captured source bytes only
        // (0x03, unchanged by this task); `defined_destination_byte_mask` is
        // the new, distinct fact that every one of the 8 destination bytes is
        // defined quadricated content (0xff) -- not the 2-bit source prefix a
        // naive "destination follows source" assumption would report.
        let load = decode_tlut(256, 3, (0x300, 0x306)).unwrap();
        let plan = load.transfer_plan().unwrap();
        for word in 0..3u16 {
            assert_eq!(plan.defined_source_byte_mask(word).unwrap(), 0x03);
            assert_eq!(plan.defined_destination_byte_mask(word).unwrap(), 0xff);
        }
    }

    #[test]
    fn logical_source_offset_is_sequential_two_bytes_per_entry() {
        let load = decode_tlut(256, 4, (0x300, 0x308)).unwrap();
        let plan = load.transfer_plan().unwrap();
        assert_eq!(plan.logical_source_offset(0).unwrap(), 0);
        assert_eq!(plan.logical_source_offset(1).unwrap(), 2);
        assert_eq!(plan.logical_source_offset(2).unwrap(), 4);
        assert_eq!(plan.logical_source_offset(3).unwrap(), 6);
    }

    #[test]
    fn no_odd_row_bank_exchange_for_tlut() {
        let load = decode_tlut(256, 4, (0x300, 0x308)).unwrap();
        let plan = load.transfer_plan().unwrap();
        for word in 0..4u16 {
            assert!(!plan.word_uses_odd_row_exchange(word).unwrap());
            assert_eq!(plan.row_advance_for_word(word).unwrap(), word);
        }
    }

    #[test]
    fn low_s_low_t_count_fraction_and_high_t_stay_rejected_before_this_tasks_code_runs() {
        // Regression fixtures: M4.3.1 must not loosen the pre-existing
        // public-macro-shape gate (`decode_load_tlut`, already present
        // before this task). Each nonzero field is independently hostile.
        let base = || {
            let mut words = Vec::new();
            words.extend(set_texture_image(1, 0x300));
            words.extend(set_tile(7, 256));
            words.extend(load_sync());
            words
        };

        let mut low_s = base();
        low_s.extend([word(LOAD_TLUT, 1 << 12), 7 << 24]);
        assert!(decode_raw_dpc(submit(hostile_packet(low_s)), &RdpState::default()).is_err());

        let mut low_t = base();
        low_t.extend([word(LOAD_TLUT, 1), 7 << 24]);
        assert!(decode_raw_dpc(submit(hostile_packet(low_t)), &RdpState::default()).is_err());

        let mut count_fraction = base();
        count_fraction.extend([word(LOAD_TLUT, 0), 7 << 24 | 1 << 12]);
        assert!(
            decode_raw_dpc(submit(hostile_packet(count_fraction)), &RdpState::default()).is_err()
        );

        let mut high_t = base();
        high_t.extend([word(LOAD_TLUT, 0), 7 << 24 | 1]);
        assert!(decode_raw_dpc(submit(hostile_packet(high_t)), &RdpState::default()).is_err());
    }

    #[test]
    fn a_source_descriptor_whose_declared_entry_count_reads_past_installed_rdram_is_rejected() {
        // `decode_load_tlut` derives its source range purely from
        // `image.address() .. image.address() + entries * 2` -- it never
        // cross-checks `entries` against `SetTextureImage`'s own declared
        // `width` (unlike `decode_load_tile`'s explicit `high_s >=
        // image.width()` check). The only remaining backstop against a
        // source-descriptor/entry-count combination that reads past what
        // was actually captured is `PhysicalMemoryLayout::range`'s installed-
        // RDRAM bounds check. This proves that backstop actually holds: the
        // maximum *valid* entry count (256, `TlutEntryCount`'s own cap) at
        // an address near the installed-layout boundary must still be
        // rejected on its source range, rather than silently producing an
        // out-of-bounds captured-range claim.
        let mut words = Vec::new();
        words.extend(set_texture_image(1, LAYOUT_BYTES - 0x100));
        words.extend(set_tile(7, 256));
        words.extend(load_sync());
        words.extend(load_tlut(7, 255));
        assert!(decode_raw_dpc(submit(hostile_packet(words)), &RdpState::default()).is_err());
    }

    #[test]
    fn destination_below_high_tmem_stays_rejected_before_this_tasks_code_runs() {
        let mut words = Vec::new();
        words.extend(set_texture_image(1, 0x300));
        words.extend(set_tile(7, 255));
        words.extend(load_sync());
        words.extend(load_tlut(7, 0));
        assert!(decode_raw_dpc(submit(hostile_packet(words)), &RdpState::default()).is_err());
    }

    #[test]
    fn source_size_is_rejected_unless_bits16() {
        // Public libultra `gbi.h` `gDPLoadTLUTCmd` always programs a 16-bit
        // SetTextureImage; every other wire size (4/8/32-bit, encodings
        // 0/1/3) must be rejected explicitly rather than silently mis-sized
        // by `transfer_shape`'s `entries.get() * 2` byte math.
        for size in [0_u32, 1, 3] {
            let mut words = Vec::new();
            words.extend(set_texture_image_sized(size, 1, 0x300));
            words.extend(set_tile(7, 256));
            words.extend(load_sync());
            words.extend(load_tlut(7, 0));
            assert!(
                decode_raw_dpc(submit(hostile_packet(words)), &RdpState::default()).is_err(),
                "SetTextureImage size encoding {size} must be rejected for LoadTLUT"
            );
        }
    }

    #[test]
    fn descriptor_size_is_rejected_unless_bits16() {
        // The admitted TLUT load descriptor (`SetTile`'s size field) must
        // independently match the public macro's 16-bit assumption -- a
        // 16-bit source image paired with a differently sized destination
        // tile descriptor is rejected too, not just a mismatched source.
        for size in [0_u32, 1, 3] {
            let mut words = Vec::new();
            words.extend(set_texture_image(1, 0x300));
            words.extend(set_tile_sized(size, 7, 256));
            words.extend(load_sync());
            words.extend(load_tlut(7, 0));
            assert!(
                decode_raw_dpc(submit(hostile_packet(words)), &RdpState::default()).is_err(),
                "SetTile size encoding {size} must be rejected for LoadTLUT"
            );
        }
    }

    #[test]
    fn back_to_back_loads_to_overlapping_destinations_each_produce_independent_plans() {
        // Two LoadTLUTs to overlapping destination word ranges in one
        // packet: each command's own transfer plan is independently exact
        // (physical-TMEM overlap resolution is M4.3.2's transaction
        // machinery, not this task's).
        let mut words = Vec::new();
        words.extend(set_texture_image(1, 0x300));
        words.extend(set_tile(7, 256));
        words.extend(load_sync());
        words.extend(load_tlut(7, 3));
        words.extend(load_sync());
        words.extend(load_tlut(7, 1));
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(
                words,
                &[(0x300, 0x308), (0x300, 0x304)],
            )),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadTlut(first) = decoded.commands()[3].kind() else {
            panic!("expected first LoadTLUT");
        };
        let RawDpcCommandKind::LoadTlut(second) = decoded.commands()[5].kind() else {
            panic!("expected second LoadTLUT");
        };
        let first_plan = first.transfer_plan().unwrap();
        let second_plan = second.transfer_plan().unwrap();
        assert_eq!(first_plan.transfer_words(), 4);
        assert_eq!(second_plan.transfer_words(), 2);
        assert_eq!(first_plan.destination_word(0).unwrap(), 256);
        assert_eq!(second_plan.destination_word(0).unwrap(), 256);
    }
}
