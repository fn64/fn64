//! T1: private raw-DPC decoder -> T0's sealed production plan writer.
//!
//! This module performs no TMEM math and no decode logic of its own. It
//! walks one already-`decode_raw_dpc`'d [`DecodedRawDpc`] command stream and
//! translates each command's already-computed private geometry
//! (`crate::tmem` types) into `fn64_render::production`'s neutral DTOs,
//! pushing each into an [`fn64_render::ExactRawDpcPlanWriter`]. This seam
//! admits the TMEM/load subset (`SetTile`/`SetTileSize`/`SetTextureImage`/
//! `LoadSync`/`LoadBlock`/`LoadTile`/`LoadTlut`) plus the nine pure-RDP-state
//! commands (`SetOtherMode`, `SetColorImage`, `SetFillColor`, `SetEnvColor`,
//! `SetPrimColor`, `SetBlendColor`, `SetFogColor`, `SetPrimDepth`,
//! `SetCombine`); every other decoded command kind (`NoOp`, `FillRectangle`,
//! `FullSync`, `RawTriangle`) remains outside the admitted zero-guest-write
//! subset and is rejected loudly -- never silently dropped -- the instant
//! one is encountered.

use fn64_render::{
    ExactRawDpcPlanWriter, NeutralColor4, NeutralColorImage, NeutralCombineParams,
    NeutralFillColor, NeutralImageFormat, NeutralOtherMode, NeutralPixelSize, NeutralPrimColor,
    NeutralPrimDepth, NeutralTextureImage, NeutralTileAddressMode, NeutralTileDescriptor,
    NeutralTileSize, NeutralTmemTransferPhysicalWord, NeutralTmemTransferWord,
    RawDpcCommandLocation as NeutralRawDpcCommandLocation, RdpStateCommand, RdpStateIdentity,
    TmemLoadEpoch, TmemLoadKind as NeutralTmemLoadKind, TmemLoadSemantics,
    TmemTransferLayout as NeutralTmemTransferLayout,
};
use fn64_render_ir::PhysicalMemoryLayout;

use crate::{
    DecodedRawDpc, ImageFormat, PixelSize, RawDpcCommandKind, RawDpcResourcePlan, TextureImage,
    TileAddressMode, TileDescriptor, TileIndex, TileSize, TmemLoad, TmemLoadKind,
    TmemTransferLayout, TmemTransferPhysicalWord, TmemTransferWord,
};

/// A decoded raw-DPC command this production seam does not admit. Every
/// command kind carried by [`RawDpcCommandKind`] outside `SetTextureImage`/
/// `SetTile`/`SetTileSize`/`LoadSync`/`LoadBlock`/`LoadTile`/`LoadTlut` and
/// the nine pure-RDP-state commands (`SetOtherMode`/`SetColorImage`/
/// `SetFillColor`/`SetEnvColor`/`SetPrimColor`/`SetBlendColor`/
/// `SetFogColor`/`SetPrimDepth`/`SetCombine`) is rejected here, loudly, at
/// the exact command index/location it was decoded at -- never silently
/// dropped or aliased to a no-op push. Remaining rejected kinds: `NoOp`,
/// `FillRectangle`, `FullSync`, `RawTriangle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnadmittedRawDpcCommand {
    pub command_index: u32,
    pub location: crate::RawDpcCommandLocation,
    pub opcode_name: &'static str,
}

impl core::fmt::Display for UnadmittedRawDpcCommand {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "raw-DPC command #{} ({}) at {} is outside v11's admitted zero-guest-write \
             TMEM/state subset and cannot enter the production plan",
            self.command_index, self.opcode_name, self.location
        )
    }
}

impl std::error::Error for UnadmittedRawDpcCommand {}

fn opcode_name(kind: &RawDpcCommandKind) -> &'static str {
    match kind {
        RawDpcCommandKind::NoOp { .. } => "NoOp",
        RawDpcCommandKind::FillRectangle(_) => "FillRectangle",
        RawDpcCommandKind::FullSync(_) => "FullSync",
        RawDpcCommandKind::RawTriangle(_) => "RawTriangle",
        RawDpcCommandKind::SetOtherMode(_)
        | RawDpcCommandKind::SetColorImage(_)
        | RawDpcCommandKind::SetFillColor(_)
        | RawDpcCommandKind::SetEnvColor(_)
        | RawDpcCommandKind::SetPrimColor(_)
        | RawDpcCommandKind::SetBlendColor(_)
        | RawDpcCommandKind::SetFogColor(_)
        | RawDpcCommandKind::SetPrimDepth(_)
        | RawDpcCommandKind::SetCombine(_)
        | RawDpcCommandKind::SetTextureImage(_)
        | RawDpcCommandKind::SetTile { .. }
        | RawDpcCommandKind::SetTileSize { .. }
        | RawDpcCommandKind::LoadSync(_)
        | RawDpcCommandKind::LoadBlock(_)
        | RawDpcCommandKind::LoadTile(_)
        | RawDpcCommandKind::LoadTlut(_) => {
            unreachable!("admitted kinds are pushed, never rejected")
        }
    }
}

fn neutral_image_format(format: ImageFormat) -> NeutralImageFormat {
    match format {
        ImageFormat::Rgba => NeutralImageFormat::Rgba,
        ImageFormat::Yuv => NeutralImageFormat::Yuv,
        ImageFormat::ColorIndex => NeutralImageFormat::ColorIndex,
        ImageFormat::IntensityAlpha => NeutralImageFormat::IntensityAlpha,
        ImageFormat::Intensity => NeutralImageFormat::Intensity,
    }
}

fn neutral_pixel_size(size: PixelSize) -> NeutralPixelSize {
    match size {
        PixelSize::Bits4 => NeutralPixelSize::Bits4,
        PixelSize::Bits8 => NeutralPixelSize::Bits8,
        PixelSize::Bits16 => NeutralPixelSize::Bits16,
        PixelSize::Bits32 => NeutralPixelSize::Bits32,
    }
}

fn neutral_address_mode(mode: TileAddressMode) -> NeutralTileAddressMode {
    NeutralTileAddressMode {
        mirror: mode.mirror(),
        clamp: mode.clamp(),
    }
}

fn neutral_tile_descriptor(descriptor: TileDescriptor) -> NeutralTileDescriptor {
    NeutralTileDescriptor {
        format: neutral_image_format(descriptor.format()),
        size: neutral_pixel_size(descriptor.size()),
        line_words: descriptor.line_words(),
        tmem_word_address: descriptor.tmem().get(),
        palette: descriptor.palette(),
        s_mode: neutral_address_mode(descriptor.s_mode()),
        mask_s: descriptor.mask_s(),
        shift_s: descriptor.shift_s(),
        t_mode: neutral_address_mode(descriptor.t_mode()),
        mask_t: descriptor.mask_t(),
        shift_t: descriptor.shift_t(),
    }
}

fn neutral_tile_size(size: TileSize) -> NeutralTileSize {
    NeutralTileSize {
        low_s: size.low_s().raw(),
        low_t: size.low_t().raw(),
        high_s: size.high_s().raw(),
        high_t: size.high_t().raw(),
    }
}

fn neutral_texture_image(image: TextureImage, layout: PhysicalMemoryLayout) -> NeutralTextureImage {
    NeutralTextureImage {
        format: neutral_image_format(image.format()),
        size: neutral_pixel_size(image.size()),
        width: image.width(),
        address: layout
            .address(image.address().get())
            .expect("decoder-staged texture image address already fits the capture's own layout"),
    }
}

fn neutral_other_mode(value: crate::OtherMode) -> NeutralOtherMode {
    NeutralOtherMode {
        high: value.high(),
        low: value.low(),
    }
}

fn neutral_color_image(
    image: crate::ColorImage,
    layout: PhysicalMemoryLayout,
) -> NeutralColorImage {
    NeutralColorImage {
        format: neutral_image_format(image.format()),
        size: neutral_pixel_size(image.size()),
        width: image.width(),
        address: layout
            .address(image.address().get())
            .expect("decoder-staged color image address already fits the capture's own layout"),
    }
}

fn neutral_fill_color(value: crate::FillColor) -> NeutralFillColor {
    NeutralFillColor {
        value: value.value(),
    }
}

fn neutral_color4(value: crate::Color4) -> NeutralColor4 {
    NeutralColor4 {
        value: value.value(),
    }
}

fn neutral_prim_color(value: crate::PrimColor) -> NeutralPrimColor {
    NeutralPrimColor {
        lod_frac: value.lod().lod_frac(),
        lod_min: value.lod().lod_min(),
        color: value.color().value(),
    }
}

fn neutral_prim_depth(value: crate::PrimDepth) -> NeutralPrimDepth {
    NeutralPrimDepth {
        z: value.z(),
        dz: value.dz(),
    }
}

fn neutral_combine(value: crate::CombineParams) -> NeutralCombineParams {
    NeutralCombineParams {
        low: value.low(),
        high: value.high(),
    }
}

fn neutral_load_kind(kind: TmemLoadKind) -> NeutralTmemLoadKind {
    match kind {
        TmemLoadKind::Block {
            source_s,
            source_t,
            high_s,
            dxt,
        } => NeutralTmemLoadKind::Block {
            source_s: source_s.raw(),
            source_t: source_t.raw(),
            high_s: high_s.raw(),
            dxt: dxt.get(),
        },
        TmemLoadKind::Tile { bounds } => NeutralTmemLoadKind::Tile {
            bounds: neutral_tile_size(bounds),
        },
        TmemLoadKind::Tlut { bounds, entries } => NeutralTmemLoadKind::Tlut {
            bounds: neutral_tile_size(bounds),
            entries: core::num::NonZeroU16::new(entries.get())
                .expect("TlutEntryCount is already nonzero"),
        },
    }
}

fn neutral_transfer_layout(layout: TmemTransferLayout) -> NeutralTmemTransferLayout {
    match layout {
        TmemTransferLayout::Linear64 => NeutralTmemTransferLayout::Linear,
        TmemTransferLayout::SplitBanks64 => NeutralTmemTransferLayout::OddRowBankSwap,
    }
}

fn neutral_transfer_physical(range: TmemTransferPhysicalWord) -> NeutralTmemTransferPhysicalWord {
    match range {
        TmemTransferPhysicalWord::Linear(range) => NeutralTmemTransferPhysicalWord::Linear(range),
        TmemTransferPhysicalWord::SplitBanks { low, high } => {
            NeutralTmemTransferPhysicalWord::SplitBanks { low, high }
        }
    }
}

fn neutral_transfer_word(word: TmemTransferWord) -> NeutralTmemTransferWord {
    NeutralTmemTransferWord {
        index: word.index(),
        logical_source_offset: word.logical_source_offset(),
        source_access_index: word.source_access_index(),
        source_access_byte_offset: word.source_access_byte_offset(),
        defined_source_byte_mask: word.defined_source_byte_mask(),
        defined_destination_byte_mask: word.defined_destination_byte_mask(),
        destination_word: word.destination_word(),
        row_advance: word.row_advance(),
        odd_row_exchange: word.odd_row_exchange(),
        physical: neutral_transfer_physical(word.physical()),
    }
}

/// Maps the old private-shape [`crate::RawDpcCommandLocation`] (a decoder
/// implementation detail: `workload`/`stream`/`stream_byte_offset`, no
/// `source_byte_len`, no ordinal `command_index`) onto T0's frozen neutral
/// [`NeutralRawDpcCommandLocation`], field by field. `command_index` is this
/// push loop's own ordinal counter, not a field the old type carries.
/// `source_byte_len` is the exact wire width `raw_rdp_command_width` already
/// used to slice `raw_words` for this same command -- the two must always
/// agree, since both come from one decoded command's own wire opcode.
fn neutral_location(
    command_index: u32,
    old: crate::RawDpcCommandLocation,
    layout: PhysicalMemoryLayout,
    source_byte_len: u32,
) -> NeutralRawDpcCommandLocation {
    // The neutral field is documented as chunk-relative (`render_ir.rs`:
    // "relative to the owning chunk, not the address space"), but the old
    // decoder type has no chunk-relative accessor -- only
    // `stream_byte_offset` (relative to the whole flattened stream) and
    // `source_byte_offset` (absolute address-space offset, used for
    // `source_address` below). `stream_byte_offset` only equals the
    // chunk-relative offset when this command's chunk is the stream's
    // first chunk (its own `stream_start` is 0), which is always true for
    // every capture this seam's production entry point
    // (`OwnedRawDpcCapture`/`preflight_raw_dpc_capture`) can build: they
    // construct exactly one chunk per stream. Assert that invariant here
    // rather than silently relying on it, so a future multi-chunk capture
    // fails loudly instead of mis-populating this field.
    assert_eq!(
        old.chunk_index(),
        0,
        "T1's production entry point only ever builds single-chunk streams; \
         stream_byte_offset is chunk-relative only for chunk 0"
    );
    NeutralRawDpcCommandLocation {
        command_index,
        stream_index: old.stream_index(),
        chunk_index: old.chunk_index(),
        source_address: layout
            .address(old.source_byte_offset())
            .expect("decoded command's source byte offset already fits the capture's own layout"),
        source_byte_offset: old.stream_byte_offset(),
        source_byte_len,
        wire_opcode: old.wire_opcode(),
    }
}

/// Slice this command's own raw wire words out of the capture's full word
/// stream. Every TMEM/state opcode this seam admits (`SetTextureImage`,
/// `SetTile`, `SetTileSize`, `LoadSync`, `LoadBlock`, `LoadTile`,
/// `LoadTlut`) is a fixed 8-byte/2-word command --
/// `crate::raw_dpc::decode_stream` always reads exactly `w0`/`w1` for every
/// one of them via `decode_tmem_command` -- so this never needs
/// `raw_rdp_command_width` to size a variable-width read; it only asserts
/// that fixed shape holds.
fn tmem_command_raw_words(
    capture_words: &[u32],
    submission_start: u32,
    old: crate::RawDpcCommandLocation,
) -> Vec<u32> {
    let start = ((old.source_byte_offset() - submission_start) / 4) as usize;
    let words = capture_words
        .get(start..start + 2)
        .expect("every admitted TMEM/state command is a checked-in-bounds 2-word command");
    words.to_vec()
}

/// Per-plan `before`/`after` tile-state tracking this push loop must thread
/// itself: [`RdpStateIdentity::of_tile_descriptor`]/`of_tile_size` need the
/// prior identity for the *same* tile slot, and `of_texture_image` and each
/// pure-RDP-state kind need the prior identity for their own single global
/// slot, neither of which [`crate::DecodedRawDpcCommand`] carries on its
/// own. `before` stays `None` until this plan's own first state command
/// touching that slot/image runs; this tracker is scoped to one
/// `push_decoded_raw_dpc` call and does not persist across submissions (T0's
/// writer is itself one-shot per submission).
#[derive(Default)]
struct StateIdentityTracker {
    tile_descriptor: [Option<RdpStateIdentity>; 8],
    tile_size: [Option<RdpStateIdentity>; 8],
    texture_image: Option<RdpStateIdentity>,
    load_epoch: Option<TmemLoadEpoch>,
    other_mode: Option<RdpStateIdentity>,
    color_image: Option<RdpStateIdentity>,
    fill_color: Option<RdpStateIdentity>,
    env_color: Option<RdpStateIdentity>,
    prim_color: Option<RdpStateIdentity>,
    blend_color: Option<RdpStateIdentity>,
    fog_color: Option<RdpStateIdentity>,
    prim_depth: Option<RdpStateIdentity>,
    combine: Option<RdpStateIdentity>,
}

fn tile_slot(index: TileIndex) -> usize {
    usize::from(index.get())
}

/// Push every command in one already-decoded raw-DPC stream into `writer`,
/// translating each into T0's neutral DTOs. `capture_words` is the exact
/// flat word image of the submission `decoded` was decoded from
/// (`writer.capture().submission().command_words()`); `layout` is that same
/// capture's memory layout. Returns the first unadmitted command
/// encountered, if any -- v11's frozen scope is TMEM-only,
/// no-FullSync, no-guest-write, so any other decoded command kind is a loud
/// rejection, not a silent omission. The writer retains every command
/// pushed before the rejection; the caller must not call `finish` on a
/// writer this function rejected against, since the resulting plan would
/// silently omit the unadmitted command's semantics.
pub fn push_decoded_raw_dpc(
    writer: &mut ExactRawDpcPlanWriter,
    decoded: &DecodedRawDpc,
    capture_words: &[u32],
    layout: PhysicalMemoryLayout,
    submission_start: u32,
) -> Result<(), UnadmittedRawDpcCommand> {
    let resource_plan: &RawDpcResourcePlan = decoded.resource_plan();
    let mut tracker = StateIdentityTracker::default();

    // The journal's ordered access list opens with one `CommandDecode` read
    // access per source stream (`decode_from_state` pushes these before it
    // ever walks a command), *before* any TMEM source/destination pair. T1's
    // capture is always the single-stream shape `OwnedRawDpcCapture`/
    // `preflight_raw_dpc_capture` produce, so there is exactly one such
    // access; push it first so `finish`'s access-count/order check against
    // the real journal it hands to preflight lines up access-for-access.
    for access in resource_plan
        .accesses()
        .iter()
        .take_while(|access| access.purpose() == fn64_render_ir::AccessPurpose::CommandDecode)
    {
        writer.push_command_decode_access(*access);
    }

    for (index, command) in decoded.commands().iter().enumerate() {
        let command_index = u32::try_from(index).expect("bounded command stream fits u32");
        let old_location = command.location();
        let raw_words = tmem_command_raw_words(capture_words, submission_start, old_location);
        let location = neutral_location(
            command_index,
            old_location,
            layout,
            u32::try_from(raw_words.len() * 4).expect("2-word commands fit u32 bytes"),
        );

        match command.kind() {
            RawDpcCommandKind::SetTextureImage(image) => {
                let neutral_image = neutral_texture_image(image, layout);
                let after = RdpStateIdentity::of_texture_image(neutral_image);
                let before = tracker.texture_image;
                tracker.texture_image = Some(after);
                writer.push_state(RdpStateCommand::SetTextureImage {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    image: neutral_image,
                    before,
                    after,
                });
            }
            RawDpcCommandKind::SetTile { tile, descriptor } => {
                let neutral_descriptor = neutral_tile_descriptor(descriptor);
                let after = RdpStateIdentity::of_tile_descriptor(tile.get(), neutral_descriptor);
                let slot = tile_slot(tile);
                let before = tracker.tile_descriptor[slot];
                tracker.tile_descriptor[slot] = Some(after);
                writer.push_state(RdpStateCommand::SetTile {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    tile_index: tile.get(),
                    descriptor: neutral_descriptor,
                    before,
                    after,
                });
            }
            RawDpcCommandKind::SetTileSize { tile, size } => {
                let neutral_size = neutral_tile_size(size);
                let after = RdpStateIdentity::of_tile_size(tile.get(), neutral_size);
                let slot = tile_slot(tile);
                let before = tracker.tile_size[slot];
                tracker.tile_size[slot] = Some(after);
                writer.push_state(RdpStateCommand::SetTileSize {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    tile_index: tile.get(),
                    size: neutral_size,
                    before,
                    after,
                });
            }
            RawDpcCommandKind::LoadSync(epoch) => {
                let output_epoch = TmemLoadEpoch::new(
                    core::num::NonZeroU64::new(epoch.get())
                        .expect("decoder-minted TmemLoadEpoch is already nonzero"),
                );
                let input_epoch = tracker.load_epoch;
                tracker.load_epoch = Some(output_epoch);
                writer.push_state(RdpStateCommand::SyncLoad {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    input_epoch,
                    output_epoch,
                });
            }
            RawDpcCommandKind::LoadBlock(load) | RawDpcCommandKind::LoadTile(load) => {
                push_tmem_load(writer, resource_plan, location, raw_words, load);
            }
            RawDpcCommandKind::LoadTlut(load) => {
                push_tmem_load(writer, resource_plan, location, raw_words, load);
            }
            RawDpcCommandKind::SetOtherMode(value) => {
                let neutral = neutral_other_mode(value);
                let after = RdpStateIdentity::of_other_mode(neutral);
                let before = tracker.other_mode;
                tracker.other_mode = Some(after);
                writer.push_state(RdpStateCommand::SetOtherMode {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    other_mode: neutral,
                    before,
                    after,
                });
            }
            RawDpcCommandKind::SetColorImage(image) => {
                let neutral = neutral_color_image(image, layout);
                let after = RdpStateIdentity::of_color_image(neutral);
                let before = tracker.color_image;
                tracker.color_image = Some(after);
                writer.push_state(RdpStateCommand::SetColorImage {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    image: neutral,
                    before,
                    after,
                });
            }
            RawDpcCommandKind::SetFillColor(value) => {
                let neutral = neutral_fill_color(value);
                let after = RdpStateIdentity::of_fill_color(neutral);
                let before = tracker.fill_color;
                tracker.fill_color = Some(after);
                writer.push_state(RdpStateCommand::SetFillColor {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    color: neutral,
                    before,
                    after,
                });
            }
            RawDpcCommandKind::SetEnvColor(value) => {
                let neutral = neutral_color4(value);
                let after = RdpStateIdentity::of_env_color(neutral);
                let before = tracker.env_color;
                tracker.env_color = Some(after);
                writer.push_state(RdpStateCommand::SetEnvColor {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    color: neutral,
                    before,
                    after,
                });
            }
            RawDpcCommandKind::SetPrimColor(value) => {
                let neutral = neutral_prim_color(value);
                let after = RdpStateIdentity::of_prim_color(neutral);
                let before = tracker.prim_color;
                tracker.prim_color = Some(after);
                writer.push_state(RdpStateCommand::SetPrimColor {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    color: neutral,
                    before,
                    after,
                });
            }
            RawDpcCommandKind::SetBlendColor(value) => {
                let neutral = neutral_color4(value);
                let after = RdpStateIdentity::of_blend_color(neutral);
                let before = tracker.blend_color;
                tracker.blend_color = Some(after);
                writer.push_state(RdpStateCommand::SetBlendColor {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    color: neutral,
                    before,
                    after,
                });
            }
            RawDpcCommandKind::SetFogColor(value) => {
                let neutral = neutral_color4(value);
                let after = RdpStateIdentity::of_fog_color(neutral);
                let before = tracker.fog_color;
                tracker.fog_color = Some(after);
                writer.push_state(RdpStateCommand::SetFogColor {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    color: neutral,
                    before,
                    after,
                });
            }
            RawDpcCommandKind::SetPrimDepth(value) => {
                let neutral = neutral_prim_depth(value);
                let after = RdpStateIdentity::of_prim_depth(neutral);
                let before = tracker.prim_depth;
                tracker.prim_depth = Some(after);
                writer.push_state(RdpStateCommand::SetPrimDepth {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    depth: neutral,
                    before,
                    after,
                });
            }
            RawDpcCommandKind::SetCombine(value) => {
                let neutral = neutral_combine(value);
                let after = RdpStateIdentity::of_combine(neutral);
                let before = tracker.combine;
                tracker.combine = Some(after);
                writer.push_state(RdpStateCommand::SetCombine {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    combine: neutral,
                    before,
                    after,
                });
            }
            other @ (RawDpcCommandKind::NoOp { .. }
            | RawDpcCommandKind::FillRectangle(_)
            | RawDpcCommandKind::FullSync(_)
            | RawDpcCommandKind::RawTriangle(_)) => {
                return Err(UnadmittedRawDpcCommand {
                    command_index,
                    location: old_location,
                    opcode_name: opcode_name(&other),
                });
            }
        }
    }
    Ok(())
}

fn push_tmem_load(
    writer: &mut ExactRawDpcPlanWriter,
    resource_plan: &RawDpcResourcePlan,
    location: NeutralRawDpcCommandLocation,
    raw_words: Vec<u32>,
    load: TmemLoad,
) {
    let bound = resource_plan
        .bind_tmem_transfer(load)
        .expect("decoder-admitted TMEM load already binds its own resource plan");
    let transfer_plan = load.transfer_plan().expect(
        "LoadBlock/LoadTile/LoadTlut always carry a Transfer contract in this admitted subset",
    );

    let source_accesses = bound.source_accesses();
    let destination_accesses = bound.destination_accesses();
    assert_eq!(
        source_accesses.len(),
        1,
        "v11's admitted TMEM source plan is exactly one journal access wide"
    );
    assert!(
        !destination_accesses.is_empty(),
        "a TMEM load always writes at least one destination access"
    );
    let source = source_accesses[0];
    // The destination union can span more than one journal access (the
    // canonical sorted/disjoint fragment set `destination_ranges` computes,
    // e.g. the low/high split-bank halves an odd-row exchange produces).
    // `TmemLoadSemantics::destination`/`destination_access_index` name only
    // the *first* fragment -- exactly what its doc comment promises T3
    // ("correlate... without re-deriving which journal entry it came
    // from"), not an exhaustive destination list; `transfer_words[].physical`
    // already carries the complete per-word physical placement, including
    // `SplitBanks { low, high }`, so no per-fragment fact is lost. Every
    // fragment still enters the plan's own access list, in the journal's
    // exact order, via `push_tmem_load` (first fragment) followed by
    // `push_command_decode_access` for the rest -- `finish`'s access-count/
    // order check requires nothing less.
    let destination = destination_accesses[0];
    let extra_destination_accesses = &destination_accesses[1..];
    let source_access_index = transfer_plan.source().first_access_index();
    let destination_access_index = transfer_plan.destination().first_access_index();

    let transfer_words: Vec<NeutralTmemTransferWord> = bound
        .words()
        .iter()
        .copied()
        .map(neutral_transfer_word)
        .collect();

    let epoch = TmemLoadEpoch::new(
        core::num::NonZeroU64::new(load.epoch().get())
            .expect("decoder-minted TmemLoadEpoch is already nonzero"),
    );

    let semantics = TmemLoadSemantics::new(
        location,
        raw_words,
        epoch,
        neutral_load_kind(load.kind()),
        load.tile().get(),
        neutral_texture_image(load.source_image(), location.source_address.layout()),
        neutral_tile_descriptor(load.tile_descriptor()),
        source,
        source_access_index,
        destination,
        destination_access_index,
        transfer_plan.logical_source_bytes(),
        transfer_plan.undefined_padding_bytes(),
        transfer_plan.words_per_row(),
        transfer_plan.row_count(),
        neutral_transfer_layout(transfer_plan.layout()),
        transfer_words,
    );
    writer.push_tmem_load(semantics);
    for extra in extra_destination_accesses {
        writer.push_command_decode_access(*extra);
    }
}

#[cfg(test)]
mod tests {
    use fn64_render::{
        new_raw_dpc_roles, ExactRawDpcPlanVisitor, OwnedRawDpcCapture, OwnedRawDpcSubmission,
        RawDpcSemanticCommandRef, TmemLoadShape,
    };
    use fn64_render_ir::{ResourceJournal, ResourceJournalLimits, TemporalBoundary};

    use crate::{decode_raw_dpc, RawDpcDecodeError, RdpState};

    use super::*;

    const LAYOUT_BYTES: u32 = 0x4000;
    const COMMAND_START: u32 = 0x1000;

    const SET_TEXTURE_IMAGE: u8 = 0x3d;
    const SET_TILE: u8 = 0x35;
    const SET_TILE_SIZE: u8 = 0x32;
    const LOAD_SYNC: u8 = 0x26;
    const LOAD_BLOCK: u8 = 0x33;
    const LOAD_TILE: u8 = 0x34;
    const LOAD_TLUT: u8 = 0x30;

    fn word(opcode: u8, payload: u32) -> u32 {
        u32::from(opcode) << 24 | payload
    }

    fn set_texture_image(format: u32, size: u32, width: u32, address: u32) -> [u32; 2] {
        [
            word(SET_TEXTURE_IMAGE, format << 21 | size << 19 | (width - 1)),
            address,
        ]
    }

    fn set_tile(tile: u32, line: u32, tmem: u32) -> [u32; 2] {
        [word(SET_TILE, 2 << 19 | line << 9 | tmem), tile << 24]
    }

    fn set_tile_size(tile: u32, high_s: u32, high_t: u32) -> [u32; 2] {
        [word(SET_TILE_SIZE, high_s << 12 | high_t), tile << 24]
    }

    fn load_sync() -> [u32; 2] {
        [word(LOAD_SYNC, 0), 0]
    }

    const SET_OTHER_MODE: u8 = 0x2f;
    const SET_COLOR_IMAGE: u8 = 0x3f;
    const SET_FILL_COLOR: u8 = 0x37;
    const SET_ENV_COLOR: u8 = 0x3b;
    const SET_PRIM_COLOR: u8 = 0x3a;
    const SET_BLEND_COLOR: u8 = 0x39;
    const SET_FOG_COLOR: u8 = 0x38;
    const SET_PRIM_DEPTH: u8 = 0x2e;
    const SET_COMBINE: u8 = 0x3c;

    fn set_other_mode(cycle_type: u32, low: u32) -> [u32; 2] {
        [word(SET_OTHER_MODE, cycle_type << 20), low]
    }

    fn set_color_image(format: u32, size: u32, width: u32, address: u32) -> [u32; 2] {
        [
            word(SET_COLOR_IMAGE, format << 21 | size << 19 | (width - 1)),
            address,
        ]
    }

    fn set_fill_color(color: u32) -> [u32; 2] {
        [word(SET_FILL_COLOR, 0), color]
    }

    fn set_env_color(color: u32) -> [u32; 2] {
        [word(SET_ENV_COLOR, 0), color]
    }

    fn set_prim_color(lod_frac: u32, lod_min: u32, color: u32) -> [u32; 2] {
        [word(SET_PRIM_COLOR, lod_min << 8 | lod_frac), color]
    }

    fn set_blend_color(color: u32) -> [u32; 2] {
        [word(SET_BLEND_COLOR, 0), color]
    }

    fn set_fog_color(color: u32) -> [u32; 2] {
        [word(SET_FOG_COLOR, 0), color]
    }

    fn set_prim_depth(z: u32, dz: u32) -> [u32; 2] {
        [word(SET_PRIM_DEPTH, 0), z << 16 | dz]
    }

    /// `CombineParams::from_wire(w0, w1)` stores `w0` unmasked -- the opcode
    /// byte `word()` bakes into the top 8 bits stays part of `low`, matching
    /// RT64's `combineL = combine & 0xFFFFFFFF` (`combiner.rs` module doc).
    /// `payload` is only the low 24 bits (the command's real 24-bit
    /// payload field); the opcode byte occupies bits 24:31 of the wire word
    /// itself, exactly like every other command this fixture module builds.
    fn set_combine(payload: u32, high: u32) -> [u32; 2] {
        [word(SET_COMBINE, payload & 0x00ff_ffff), high]
    }

    /// Build one owned, admitted, `SubmittedTicket`-decoded raw-DPC capture
    /// out of `words` plus one TMEM source range, exactly the same
    /// "probe, then finalize" journal-derivation `raw_dpc::mod::tests`'s own
    /// `packet_with_tmem_sources` performs -- reimplemented locally against
    /// `OwnedRawDpcCapture`, T1's actual production entry point, rather than
    /// the legacy multi-stream `WorkloadPacket` constructor those tests use.
    const FULL_SYNC: u8 = 0x29;

    /// Every full-sync boundary this fixture's `words` observes, derived the
    /// same way `raw_dpc::mod::tests`'s own `packet()` helper does: scan
    /// each 2-word command slot for the `FULL_SYNC` opcode byte and record
    /// its exact stream/source position. `preflight_raw_dpc_capture` has no
    /// auto-derivation of its own -- a caller must supply this list.
    fn full_sync_boundaries(words: &[u32]) -> Vec<fn64_render_ir::FullSyncBoundary> {
        words
            .chunks_exact(2)
            .enumerate()
            .filter(|(_, command)| ((command[0] >> 24) as u8 & 0x3f) == FULL_SYNC)
            .map(|(ordinal, _)| {
                fn64_render_ir::FullSyncBoundary::new(
                    2 + ordinal as u64 * 2,
                    3 + ordinal as u64 * 2,
                    fn64_render_ir::DpInterruptState::Clear,
                    fn64_render_ir::DpInterruptState::Asserted,
                )
            })
            .collect()
    }

    fn decode_admitted_capture(
        words: Vec<u32>,
        source_range: (u32, u32),
    ) -> (DecodedRawDpc, OwnedRawDpcCapture, ResourceJournal) {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let end = COMMAND_START + u32::try_from(words.len() * 4).unwrap();
        let full_syncs = full_sync_boundaries(&words);
        let submission =
            OwnedRawDpcSubmission::from_rdram_words(COMMAND_START, end, words.clone()).unwrap();
        let capture = OwnedRawDpcCapture::new(
            submission,
            layout,
            7,
            TemporalBoundary::new(1, fn64_render_ir::DpInterruptState::Clear),
        );

        // The real journal (which includes every TMEM destination access
        // the decoder itself computes -- possibly split across the
        // odd-row-bank-swap layout) cannot be hand-derived without running
        // the decoder once. Probe with a command/source-only journal, let
        // `decode_raw_dpc` report the exact access list it actually wanted
        // via `JournalMismatch::expected`, then finalize for real against
        // that. Same two-pass shape as `raw_dpc::mod::tests`'s own
        // `packet_with_tmem_sources`.
        let probe_journal = journal_for(&capture, source_range, layout);
        let probe_ticket = finalize_ticket(&capture, layout, probe_journal, full_syncs.clone());
        let (mut probe_queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let probe_submitted = probe_queue.submit(probe_ticket).unwrap();
        let journal = match decode_raw_dpc(probe_submitted, &RdpState::default()) {
            Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => {
                let accesses = expected.into_vec();
                let declared = accesses
                    .iter()
                    .map(|access| access.region().declared_bytes())
                    .sum::<u32>();
                ResourceJournal::try_new(
                    ResourceJournalLimits::try_new(64, declared.max(1)).unwrap(),
                    accesses,
                )
                .unwrap()
            }
            Ok(_) => journal_for(&capture, source_range, layout),
            Err(error) => panic!("TMEM fixture probe failed before journal comparison: {error}"),
        };

        let ticket = finalize_ticket(&capture, layout, journal.clone(), full_syncs);
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let submitted = queue.submit(ticket).unwrap();
        let decoded =
            decode_raw_dpc(submitted, &RdpState::default()).expect("fixture decodes cleanly");
        (decoded, capture, journal)
    }

    fn finalize_ticket(
        capture: &OwnedRawDpcCapture,
        layout: PhysicalMemoryLayout,
        journal: ResourceJournal,
        full_syncs: Vec<fn64_render_ir::FullSyncBoundary>,
    ) -> fn64_render_ir::DecodedTicket {
        let preflight = fn64_render::preflight_raw_dpc_capture(
            layout,
            7,
            capture.submission().clone(),
            capture.cmd_end(),
            full_syncs,
            journal,
        )
        .expect("fixture journal has valid limits for this capture's own command bytes");
        let guest_capture = fn64_render_ir::DeferredGuestReadCapture::new(
            preflight
                .guest_read_plan()
                .reads()
                .iter()
                .map(|read| {
                    fn64_render_ir::CapturedGuestRead::try_new(
                        *read,
                        vec![0; read.range().len() as usize],
                    )
                    .unwrap()
                })
                .collect(),
        );
        preflight
            .finalize(guest_capture)
            .expect("captured reads match the plan's own guest-read plan exactly")
    }

    fn journal_for(
        capture: &OwnedRawDpcCapture,
        source_range: (u32, u32),
        layout: PhysicalMemoryLayout,
    ) -> ResourceJournal {
        use fn64_render_ir::{
            AccessMode, AccessPurpose, OperationId, RdramResource, ResourceAccess, ResourceRegion,
        };
        let bytes = u32::try_from(capture.submission().command_words().len() * 4).unwrap();
        let command_access = ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: layout.range(COMMAND_START, COMMAND_START + bytes).unwrap(),
            },
        )
        .unwrap();
        let source_access = ResourceAccess::try_new(
            OperationId::new(1),
            AccessMode::Read,
            AccessPurpose::TmemLoadSource,
            ResourceRegion::Rdram {
                resource: RdramResource::Buffer,
                range: layout.range(source_range.0, source_range.1).unwrap(),
            },
        )
        .unwrap();
        let accesses = vec![command_access, source_access];
        let declared = accesses
            .iter()
            .map(|access| access.region().declared_bytes())
            .sum::<u32>();
        ResourceJournal::try_new(
            ResourceJournalLimits::try_new(64, declared.max(1)).unwrap(),
            accesses,
        )
        .unwrap()
    }

    #[derive(Default)]
    struct RecordingVisitor {
        loads: Vec<TmemLoadSemantics>,
        states: Vec<RdpStateCommand>,
        accesses: Vec<fn64_render_ir::ResourceAccess>,
    }

    impl ExactRawDpcPlanVisitor for RecordingVisitor {
        fn command(&mut self, command: RawDpcSemanticCommandRef<'_>) {
            match command {
                RawDpcSemanticCommandRef::TmemLoad(load) => self.loads.push(load.clone()),
                RawDpcSemanticCommandRef::State(state) => self.states.push(state.clone()),
                other => unreachable!(
                    "RawDpcSemanticCommandRef gained a variant this test doesn't know about: \
                     {other:?}"
                ),
            }
        }

        fn access(&mut self, access: fn64_render_ir::ResourceAccess) {
            self.accesses.push(access);
        }
    }

    struct NoopExecutionView;
    impl fn64_render::RawDpcExecutionView<RecordingVisitor> for NoopExecutionView {
        fn plan_visited(&mut self, _plan_visitor: &mut RecordingVisitor) {}
        fn captured_reads(&mut self, _reads: &[fn64_render_ir::CapturedGuestRead]) {}
        fn submitted_packet(&mut self, _packet: &fn64_render_ir::WorkloadPacket) {}
    }

    /// Drive one real submission through T0's sealed writer/session lifecycle
    /// with T1's push loop, then hand back every neutral command/access
    /// [`fn64_render::ExactValidatedRawDpcPlan::visit`] lent through
    /// [`fn64_render::BoundSubmittedRawDpc::execution_view`] -- the one
    /// public, nonextracting route to a plan's contents once it is sealed.
    /// This exercises the real `new_raw_dpc_roles` -> `begin_plan` ->
    /// (T1's push loop) -> `finish` -> `finalize_and_submit` ->
    /// `execution_view` chain end to end, not a shortcut around any of it.
    fn push_and_visit(
        decoded: &DecodedRawDpc,
        capture: OwnedRawDpcCapture,
        journal: ResourceJournal,
    ) -> RecordingVisitor {
        let layout = capture.memory_layout();
        let submission_start = capture.submission().start();
        let capture_words = capture.submission().command_words();

        let (mut session, authority) = new_raw_dpc_roles().unwrap();
        let request = session.plan_request(capture);
        let mut writer = authority.begin_plan(request);

        push_decoded_raw_dpc(
            &mut writer,
            decoded,
            &capture_words,
            layout,
            submission_start,
        )
        .expect("fixture stays inside v11's admitted TMEM/state subset");

        let planned = writer
            .finish(journal)
            .expect("pushed accesses match the journal exactly");
        // v11's admitted subset is zero *guest-write*
        // (`RawDpcAbiSession::commit_zero_guest_writes`), not zero guest
        // read: every TMEM load's source bytes are an RDRAM read the ABI
        // owner must capture and hand back here before the submission can
        // finalize.
        let reads = fn64_render_ir::DeferredGuestReadCapture::new(
            planned
                .guest_read_plan()
                .reads()
                .iter()
                .map(|read| {
                    fn64_render_ir::CapturedGuestRead::try_new(
                        *read,
                        vec![0; read.range().len() as usize],
                    )
                    .unwrap()
                })
                .collect(),
        );
        let bound = session
            .finalize_and_submit(planned, reads)
            .expect("captured reads match the plan's own guest-read plan exactly");

        let mut plan_visitor = RecordingVisitor::default();
        let mut view = NoopExecutionView;
        bound.execution_view(&authority, &mut plan_visitor, &mut view);
        plan_visitor
    }

    #[test]
    fn load_block_differential_matches_the_decoded_command() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(set_tile(7, 2, 0));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));

        let RawDpcCommandKind::LoadBlock(source_load) = decoded.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        let plan = push_and_visit(&decoded, capture, journal);

        assert_eq!(plan.states.len(), 3, "SetTextureImage, SetTile, LoadSync");
        assert_eq!(plan.loads.len(), 1);
        let load = &plan.loads[0];
        assert_eq!(load.shape(), TmemLoadShape::Block);
        assert_eq!(load.tile_index(), source_load.tile().get());
        assert_eq!(load.epoch().get(), source_load.epoch().get());
        assert_eq!(
            load.raw_words(),
            [word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]
        );
        assert_eq!(
            neutral_tile_descriptor(source_load.tile_descriptor()),
            load.tile_descriptor()
        );
        let transfer_plan = source_load.transfer_plan().unwrap();
        assert_eq!(
            load.logical_source_bytes(),
            transfer_plan.logical_source_bytes()
        );
        assert_eq!(load.words_per_row(), transfer_plan.words_per_row());
        assert_eq!(load.row_count(), transfer_plan.row_count());
        assert_eq!(
            load.transfer_words().len(),
            transfer_plan.transfer_words() as usize
        );

        let bound = decoded
            .resource_plan()
            .bind_tmem_transfer(source_load)
            .unwrap();
        for (neutral_word, source_word) in load.transfer_words().iter().zip(bound.words()) {
            assert_eq!(*neutral_word, neutral_transfer_word(*source_word));
        }
    }

    /// A `SplitBanks64`-layout LoadBlock (RGBA32 source image) produces more
    /// than one TMEM destination journal access -- the exact shape
    /// `push_tmem_load`'s `extra_destination_accesses` /
    /// `push_command_decode_access` loop exists to push in journal order.
    /// Reuses the same RGBA32/16-bit-tile-descriptor split-bank fixture as
    /// `raw_dpc::mod::tests::rgba32_uses_texture_image_size_and_split_banks_despite_tile_size`.
    #[test]
    fn load_block_split_bank_pushes_every_destination_access_in_journal_order() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 3, 2, 0x200));
        words.extend(set_tile(7, 0, 255));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 0), 7 << 24 | 1 << 12]);
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x200, 0x208));

        let RawDpcCommandKind::LoadBlock(source_load) = decoded.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        let bound = decoded
            .resource_plan()
            .bind_tmem_transfer(source_load)
            .unwrap();
        assert_eq!(
            source_load.transfer_plan().unwrap().layout(),
            crate::TmemTransferLayout::SplitBanks64,
            "fixture must actually exercise the split-bank destination shape"
        );
        assert!(
            bound.destination_accesses().len() > 1,
            "fixture must produce more than one TMEM destination journal access"
        );
        let expected_destination_accesses = bound.destination_accesses().to_vec();

        let plan = push_and_visit(&decoded, capture, journal);

        assert_eq!(plan.loads.len(), 1);
        let load = &plan.loads[0];
        assert_eq!(
            neutral_transfer_layout(source_load.transfer_plan().unwrap().layout()),
            NeutralTmemTransferLayout::OddRowBankSwap
        );
        // `destination`/`destination_access_index` name only the first
        // fragment (see `push_tmem_load`'s doc comment) -- confirm it
        // matches the real first destination access.
        assert_eq!(load.destination(), expected_destination_accesses[0]);

        // Every destination fragment -- not just the first -- must appear in
        // the plan's own access list, in the journal's exact order, so a
        // physical executor can bind every physical write this load
        // produces. The plan's access list is `[CommandDecode, source,
        // destination_0, destination_1, ...]` for this single-load fixture.
        let plan_destination_accesses = &plan.accesses[2..];
        assert_eq!(
            plan_destination_accesses,
            expected_destination_accesses.as_slice(),
            "every split-bank destination fragment must be pushed, in order, not just the first"
        );
    }

    #[test]
    fn load_tile_differential_matches_the_decoded_command() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, 5, 0x200));
        words.extend(set_tile(7, 3, 0));
        words.extend(set_tile_size(7, 16, 8));
        words.extend(load_sync());
        words.extend([word(LOAD_TILE, 4), 7 << 24 | 16 << 12 | 8]);
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x20a, 0x21e));

        let RawDpcCommandKind::LoadTile(source_load) = decoded.commands()[4].kind() else {
            panic!("expected LoadTile");
        };
        let plan = push_and_visit(&decoded, capture, journal);

        assert_eq!(
            plan.states.len(),
            4,
            "SetTextureImage, SetTile, SetTileSize, LoadSync"
        );
        assert_eq!(plan.loads.len(), 1);
        let load = &plan.loads[0];
        assert_eq!(load.shape(), TmemLoadShape::Tile);
        assert_eq!(neutral_load_kind(source_load.kind()), load.kind());
        let bound = decoded
            .resource_plan()
            .bind_tmem_transfer(source_load)
            .unwrap();
        assert_eq!(load.transfer_words().len(), bound.words().len());
        for (neutral_word, source_word) in load.transfer_words().iter().zip(bound.words()) {
            assert_eq!(*neutral_word, neutral_transfer_word(*source_word));
        }
    }

    #[test]
    fn load_tlut_differential_matches_the_decoded_command() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, 1, 0x300));
        words.extend(set_tile(7, 0, 256));
        words.extend(load_sync());
        words.extend([word(LOAD_TLUT, 0), 7 << 24 | 255 << 14]);
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x300, 0x500));

        let RawDpcCommandKind::LoadTlut(source_load) = decoded.commands()[3].kind() else {
            panic!("expected LoadTLUT");
        };
        let plan = push_and_visit(&decoded, capture, journal);

        assert_eq!(plan.loads.len(), 1);
        let load = &plan.loads[0];
        assert_eq!(load.shape(), TmemLoadShape::Tlut);
        let NeutralTmemLoadKind::Tlut { entries, .. } = load.kind() else {
            panic!("expected TLUT load kind");
        };
        assert_eq!(entries.get(), 256);
        let TmemLoadKind::Tlut {
            entries: source_entries,
            ..
        } = source_load.kind()
        else {
            panic!("expected source TLUT load kind");
        };
        assert_eq!(entries.get(), source_entries.get());
    }

    #[test]
    fn set_state_commands_thread_before_after_identity_across_the_plan() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(set_tile(7, 2, 0));
        words.extend(set_tile_size(7, 4, 8));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let plan = push_and_visit(&decoded, capture, journal);

        assert_eq!(plan.states.len(), 4);
        let RdpStateCommand::SetTextureImage { before, .. } = &plan.states[0] else {
            panic!("expected SetTextureImage first");
        };
        assert!(
            before.is_none(),
            "first state command touching this slot has no prior identity"
        );
        let RdpStateCommand::SetTile { before, .. } = &plan.states[1] else {
            panic!("expected SetTile second");
        };
        assert!(before.is_none());
        let RdpStateCommand::SetTileSize { before, .. } = &plan.states[2] else {
            panic!("expected SetTileSize third");
        };
        assert!(before.is_none());
        let RdpStateCommand::SyncLoad { input_epoch, .. } = &plan.states[3] else {
            panic!("expected SyncLoad fourth");
        };
        assert!(
            input_epoch.is_none(),
            "first LoadSync in a plan has no prior epoch"
        );
    }

    /// New coverage for this card: each of the nine pure-RDP-state commands'
    /// two-occurrence `before`/`after` identity chaining. This is NOT an
    /// extension of already-proven behavior -- the independent review that
    /// froze this card found the "second occurrence, `before == Some(first's
    /// after)`" chaining shape untested for any existing single-slot field
    /// (including `SetTextureImage`) prior to this test. The mechanism
    /// itself is real (it falls out of `StateIdentityTracker`'s ordinary
    /// mutate-then-store pattern, identical in shape for every field), but
    /// this is the first test that actually exercises two occurrences of the
    /// same single-slot state command in one plan and asserts the second's
    /// `before` equals the first's `after`.
    #[test]
    fn new_pure_state_commands_thread_before_after_identity_across_two_occurrences() {
        let mut words = Vec::new();
        words.extend(set_other_mode(3, 0)); // Fill
        words.extend(set_other_mode(0, 0)); // OneCycle
        words.extend(set_color_image(0, 2, 8, 0x200));
        words.extend(set_color_image(0, 2, 4, 0x400));
        words.extend(set_fill_color(0xf801_f801));
        words.extend(set_fill_color(0x0000_0000));
        words.extend(set_env_color(0x11223344));
        words.extend(set_env_color(0x55667788));
        words.extend(set_prim_color(10, 5, 0x11223344));
        words.extend(set_prim_color(20, 10, 0x55667788));
        words.extend(set_blend_color(0x11223344));
        words.extend(set_blend_color(0x55667788));
        words.extend(set_fog_color(0x11223344));
        words.extend(set_fog_color(0x55667788));
        words.extend(set_prim_depth(100, 200));
        words.extend(set_prim_depth(300, 400));
        words.extend(set_combine(0x1234_5678, 0x9abc_def0));
        words.extend(set_combine(0x0000_0001, 0x0000_0002));
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let plan = push_and_visit(&decoded, capture, journal);

        assert_eq!(
            plan.states.len(),
            18,
            "two occurrences of each of nine commands"
        );

        fn assert_chains<T: std::fmt::Debug>(
            label: &str,
            first: (Option<RdpStateIdentity>, RdpStateIdentity),
            second: (Option<RdpStateIdentity>, RdpStateIdentity),
            distinguishing_first: T,
            distinguishing_second: T,
        ) {
            assert!(
                first.0.is_none(),
                "{label}: first occurrence in this plan has no prior identity"
            );
            assert_eq!(
                second.0,
                Some(first.1),
                "{label}: second occurrence's before must equal the first's after"
            );
            assert_ne!(
                first.1, second.1,
                "{label}: distinct values ({distinguishing_first:?} vs \
                 {distinguishing_second:?}) must produce distinct identities"
            );
        }

        let RdpStateCommand::SetOtherMode {
            before: b0,
            after: a0,
            other_mode: v0,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetOtherMode first");
        };
        let RdpStateCommand::SetOtherMode {
            before: b1,
            after: a1,
            other_mode: v1,
            ..
        } = &plan.states[1]
        else {
            panic!("expected SetOtherMode second");
        };
        assert_chains("SetOtherMode", (*b0, *a0), (*b1, *a1), *v0, *v1);

        let RdpStateCommand::SetColorImage {
            before: b0,
            after: a0,
            image: v0,
            ..
        } = &plan.states[2]
        else {
            panic!("expected SetColorImage first");
        };
        let RdpStateCommand::SetColorImage {
            before: b1,
            after: a1,
            image: v1,
            ..
        } = &plan.states[3]
        else {
            panic!("expected SetColorImage second");
        };
        assert_chains("SetColorImage", (*b0, *a0), (*b1, *a1), *v0, *v1);

        let RdpStateCommand::SetFillColor {
            before: b0,
            after: a0,
            color: v0,
            ..
        } = &plan.states[4]
        else {
            panic!("expected SetFillColor first");
        };
        let RdpStateCommand::SetFillColor {
            before: b1,
            after: a1,
            color: v1,
            ..
        } = &plan.states[5]
        else {
            panic!("expected SetFillColor second");
        };
        assert_chains("SetFillColor", (*b0, *a0), (*b1, *a1), *v0, *v1);

        let RdpStateCommand::SetEnvColor {
            before: b0,
            after: a0,
            color: v0,
            ..
        } = &plan.states[6]
        else {
            panic!("expected SetEnvColor first");
        };
        let RdpStateCommand::SetEnvColor {
            before: b1,
            after: a1,
            color: v1,
            ..
        } = &plan.states[7]
        else {
            panic!("expected SetEnvColor second");
        };
        assert_chains("SetEnvColor", (*b0, *a0), (*b1, *a1), *v0, *v1);

        let RdpStateCommand::SetPrimColor {
            before: b0,
            after: a0,
            color: v0,
            ..
        } = &plan.states[8]
        else {
            panic!("expected SetPrimColor first");
        };
        let RdpStateCommand::SetPrimColor {
            before: b1,
            after: a1,
            color: v1,
            ..
        } = &plan.states[9]
        else {
            panic!("expected SetPrimColor second");
        };
        assert_chains("SetPrimColor", (*b0, *a0), (*b1, *a1), *v0, *v1);

        let RdpStateCommand::SetBlendColor {
            before: b0,
            after: a0,
            color: v0,
            ..
        } = &plan.states[10]
        else {
            panic!("expected SetBlendColor first");
        };
        let RdpStateCommand::SetBlendColor {
            before: b1,
            after: a1,
            color: v1,
            ..
        } = &plan.states[11]
        else {
            panic!("expected SetBlendColor second");
        };
        assert_chains("SetBlendColor", (*b0, *a0), (*b1, *a1), *v0, *v1);

        let RdpStateCommand::SetFogColor {
            before: b0,
            after: a0,
            color: v0,
            ..
        } = &plan.states[12]
        else {
            panic!("expected SetFogColor first");
        };
        let RdpStateCommand::SetFogColor {
            before: b1,
            after: a1,
            color: v1,
            ..
        } = &plan.states[13]
        else {
            panic!("expected SetFogColor second");
        };
        assert_chains("SetFogColor", (*b0, *a0), (*b1, *a1), *v0, *v1);

        let RdpStateCommand::SetPrimDepth {
            before: b0,
            after: a0,
            depth: v0,
            ..
        } = &plan.states[14]
        else {
            panic!("expected SetPrimDepth first");
        };
        let RdpStateCommand::SetPrimDepth {
            before: b1,
            after: a1,
            depth: v1,
            ..
        } = &plan.states[15]
        else {
            panic!("expected SetPrimDepth second");
        };
        assert_chains("SetPrimDepth", (*b0, *a0), (*b1, *a1), *v0, *v1);

        let RdpStateCommand::SetCombine {
            before: b0,
            after: a0,
            combine: v0,
            ..
        } = &plan.states[16]
        else {
            panic!("expected SetCombine first");
        };
        let RdpStateCommand::SetCombine {
            before: b1,
            after: a1,
            combine: v1,
            ..
        } = &plan.states[17]
        else {
            panic!("expected SetCombine second");
        };
        assert_chains("SetCombine", (*b0, *a0), (*b1, *a1), *v0, *v1);
    }

    /// One test per newly admitted command, decoding a fixture stream
    /// containing that command, pushing it through `push_decoded_raw_dpc`,
    /// and asserting the pushed `RdpStateCommand` variant's fields match the
    /// decoded source exactly (wire words, decoded value, location) -- same
    /// shape as `load_block_differential_matches_the_decoded_command`.
    #[test]
    fn set_other_mode_is_admitted_and_matches_the_decoded_command() {
        let words = set_other_mode(3, 0x00c0_0000).to_vec();
        let (decoded, capture, journal) = decode_admitted_capture(words.clone(), (0x214, 0x224));
        let RawDpcCommandKind::SetOtherMode(source) = decoded.commands()[0].kind() else {
            panic!("expected SetOtherMode");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 1);
        let RdpStateCommand::SetOtherMode {
            raw_words,
            other_mode,
            before,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetOtherMode");
        };
        assert_eq!(raw_words.as_ref(), words.as_slice());
        assert_eq!(*other_mode, neutral_other_mode(source));
        assert!(before.is_none());
    }

    #[test]
    fn set_color_image_is_admitted_and_matches_the_decoded_command() {
        let words = set_color_image(0, 2, 8, 0x200).to_vec();
        let (decoded, capture, journal) = decode_admitted_capture(words.clone(), (0x214, 0x224));
        let RawDpcCommandKind::SetColorImage(source) = decoded.commands()[0].kind() else {
            panic!("expected SetColorImage");
        };
        let layout = capture.memory_layout();
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 1);
        let RdpStateCommand::SetColorImage {
            raw_words,
            image,
            before,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetColorImage");
        };
        assert_eq!(raw_words.as_ref(), words.as_slice());
        assert_eq!(*image, neutral_color_image(source, layout));
        assert!(before.is_none());
    }

    #[test]
    fn set_fill_color_is_admitted_and_matches_the_decoded_command() {
        let words = set_fill_color(0xf801_f801).to_vec();
        let (decoded, capture, journal) = decode_admitted_capture(words.clone(), (0x214, 0x224));
        let RawDpcCommandKind::SetFillColor(source) = decoded.commands()[0].kind() else {
            panic!("expected SetFillColor");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 1);
        let RdpStateCommand::SetFillColor {
            raw_words,
            color,
            before,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetFillColor");
        };
        assert_eq!(raw_words.as_ref(), words.as_slice());
        assert_eq!(*color, neutral_fill_color(source));
        assert!(before.is_none());
    }

    #[test]
    fn set_env_color_is_admitted_and_matches_the_decoded_command() {
        let words = set_env_color(0x11223344).to_vec();
        let (decoded, capture, journal) = decode_admitted_capture(words.clone(), (0x214, 0x224));
        let RawDpcCommandKind::SetEnvColor(source) = decoded.commands()[0].kind() else {
            panic!("expected SetEnvColor");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 1);
        let RdpStateCommand::SetEnvColor {
            raw_words,
            color,
            before,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetEnvColor");
        };
        assert_eq!(raw_words.as_ref(), words.as_slice());
        assert_eq!(*color, neutral_color4(source));
        assert!(before.is_none());
    }

    #[test]
    fn set_prim_color_is_admitted_and_matches_the_decoded_command() {
        let words = set_prim_color(10, 5, 0x11223344).to_vec();
        let (decoded, capture, journal) = decode_admitted_capture(words.clone(), (0x214, 0x224));
        let RawDpcCommandKind::SetPrimColor(source) = decoded.commands()[0].kind() else {
            panic!("expected SetPrimColor");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 1);
        let RdpStateCommand::SetPrimColor {
            raw_words,
            color,
            before,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetPrimColor");
        };
        assert_eq!(raw_words.as_ref(), words.as_slice());
        assert_eq!(*color, neutral_prim_color(source));
        assert!(before.is_none());
    }

    #[test]
    fn set_blend_color_is_admitted_and_matches_the_decoded_command() {
        let words = set_blend_color(0x11223344).to_vec();
        let (decoded, capture, journal) = decode_admitted_capture(words.clone(), (0x214, 0x224));
        let RawDpcCommandKind::SetBlendColor(source) = decoded.commands()[0].kind() else {
            panic!("expected SetBlendColor");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 1);
        let RdpStateCommand::SetBlendColor {
            raw_words,
            color,
            before,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetBlendColor");
        };
        assert_eq!(raw_words.as_ref(), words.as_slice());
        assert_eq!(*color, neutral_color4(source));
        assert!(before.is_none());
    }

    #[test]
    fn set_fog_color_is_admitted_and_matches_the_decoded_command() {
        let words = set_fog_color(0x11223344).to_vec();
        let (decoded, capture, journal) = decode_admitted_capture(words.clone(), (0x214, 0x224));
        let RawDpcCommandKind::SetFogColor(source) = decoded.commands()[0].kind() else {
            panic!("expected SetFogColor");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 1);
        let RdpStateCommand::SetFogColor {
            raw_words,
            color,
            before,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetFogColor");
        };
        assert_eq!(raw_words.as_ref(), words.as_slice());
        assert_eq!(*color, neutral_color4(source));
        assert!(before.is_none());
    }

    #[test]
    fn set_prim_depth_is_admitted_and_matches_the_decoded_command() {
        let words = set_prim_depth(100, 200).to_vec();
        let (decoded, capture, journal) = decode_admitted_capture(words.clone(), (0x214, 0x224));
        let RawDpcCommandKind::SetPrimDepth(source) = decoded.commands()[0].kind() else {
            panic!("expected SetPrimDepth");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 1);
        let RdpStateCommand::SetPrimDepth {
            raw_words,
            depth,
            before,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetPrimDepth");
        };
        assert_eq!(raw_words.as_ref(), words.as_slice());
        assert_eq!(*depth, neutral_prim_depth(source));
        assert!(before.is_none());
    }

    #[test]
    fn set_combine_is_admitted_and_matches_the_decoded_command() {
        let words = set_combine(0x1234_5678, 0x9abc_def0).to_vec();
        let (decoded, capture, journal) = decode_admitted_capture(words.clone(), (0x214, 0x224));
        let RawDpcCommandKind::SetCombine(source) = decoded.commands()[0].kind() else {
            panic!("expected SetCombine");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 1);
        let RdpStateCommand::SetCombine {
            raw_words,
            combine,
            before,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetCombine");
        };
        assert_eq!(raw_words.as_ref(), words.as_slice());
        assert_eq!(*combine, neutral_combine(source));
        assert!(before.is_none());
    }

    #[test]
    fn journal_mismatch_is_a_loud_rejection_not_a_silent_plan() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(set_tile(7, 2, 0));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
        let (decoded, capture, _correct_journal) = decode_admitted_capture(words, (0x214, 0x224));

        let layout = capture.memory_layout();
        let submission_start = capture.submission().start();
        let capture_words = capture.submission().command_words();
        let (session, authority) = new_raw_dpc_roles().unwrap();
        let request = session.plan_request(capture);
        let mut writer = authority.begin_plan(request);
        push_decoded_raw_dpc(
            &mut writer,
            &decoded,
            &capture_words,
            layout,
            submission_start,
        )
        .unwrap();

        // A journal from an unrelated source range: same shape, different
        // declared bytes, so it can never equal what the writer actually
        // accumulated for this fixture's real TMEM source access.
        let wrong_journal = journal_for(
            &OwnedRawDpcCapture::new(
                OwnedRawDpcSubmission::from_rdram_words(
                    COMMAND_START,
                    COMMAND_START + 4 * 4 * 2,
                    vec![0; 8],
                )
                .unwrap(),
                layout,
                7,
                TemporalBoundary::new(1, fn64_render_ir::DpInterruptState::Clear),
            ),
            (0x300, 0x310),
            layout,
        );

        let result = writer.finish(wrong_journal);
        assert!(
            result.is_err(),
            "a journal whose access list disagrees with what T1 pushed must be a loud Err, \
             never a silently-accepted plan"
        );
    }

    #[test]
    fn full_sync_is_rejected_loudly_not_silently_omitted() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(set_tile(7, 2, 0));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
        words.extend([word(0x29, 0), 0]); // FULL_SYNC
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));

        let layout = capture.memory_layout();
        let submission_start = capture.submission().start();
        let capture_words = capture.submission().command_words();
        let (session, authority) = new_raw_dpc_roles().unwrap();
        let request = session.plan_request(capture);
        let mut writer = authority.begin_plan(request);
        let outcome = push_decoded_raw_dpc(
            &mut writer,
            &decoded,
            &capture_words,
            layout,
            submission_start,
        );
        let Err(rejection) = outcome else {
            panic!("FullSync must be rejected, not admitted into the plan");
        };
        assert_eq!(rejection.opcode_name, "FullSync");
        assert_eq!(rejection.command_index, 4);
        let _ = (session, journal);
    }
}
