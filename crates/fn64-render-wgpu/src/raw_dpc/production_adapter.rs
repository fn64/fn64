//! T1: private raw-DPC decoder -> T0's sealed production plan writer.
//!
//! This module performs no TMEM math and no decode logic of its own. It
//! walks one already-`decode_raw_dpc`'d [`DecodedRawDpc`] command stream and
//! translates each command's already-computed private geometry
//! (`crate::tmem` types) into `fn64_render::production`'s neutral DTOs,
//! pushing each into an [`fn64_render::ExactRawDpcPlanWriter`]. This seam
//! admits the TMEM/load subset (`SetTile`/`SetTileSize`/`SetTextureImage`/
//! `LoadSync`/`LoadBlock`/`LoadTile`/`LoadTlut`), the nine pure-RDP-state
//! commands (`SetOtherMode`, `SetColorImage`, `SetFillColor`, `SetEnvColor`,
//! `SetPrimColor`, `SetBlendColor`, `SetFogColor`, `SetPrimDepth`,
//! `SetCombine`), `RawTriangle`, and `TextureRectangle`/
//! `TextureRectangleFlip` (admitted as two `RdpTriangleCommand` pushes each,
//! reusing the same triangle draw path -- see `push_decoded_raw_dpc`'s
//! `TextureRectangle` arm). `NoOp` is admitted and discarded: it carries no
//! resource identity or state delta, so it is pushed nowhere and simply
//! skipped.
//!
//! `FillRectangle` is admitted too, as the one command here that declares
//! guest-visible `RenderTarget` write accesses -- N of them, one per row for
//! a partial-width fill (see `RdpFillRectangleCommand`). Its access slice
//! comes from the decoder's own recorded span, never re-derived here.
//!
//! `FullSync` is admitted as a *site*: the opcode is walked, bound to the
//! capture's own `FullSyncBoundary`, and pushed as an `RdpFullSyncSite` that
//! declares zero resource accesses. Admitting the site claims the opcode was
//! reached and the sole DP completion slot was proved free -- it claims
//! nothing about a DP interrupt being raised or observed. A capture that
//! carries no boundary for the site is still rejected loudly, never silently
//! dropped.

use fn64_render::{
    ExactRawDpcPlanWriter, NeutralColor4, NeutralColorImage, NeutralCombineParams,
    NeutralFillColor, NeutralImageFormat, NeutralOtherMode, NeutralPixelSize, NeutralPrimColor,
    NeutralPrimDepth, NeutralScissor, NeutralTextureImage, NeutralTileAddressMode,
    NeutralTileDescriptor, NeutralTileSize, NeutralTmemTransferPhysicalWord,
    NeutralTmemTransferWord, NeutralTriangleVertex,
    RawDpcCommandLocation as NeutralRawDpcCommandLocation, RdpFillRectangleCommand,
    RdpStateCommand, RdpStateIdentity, RdpTriangleCommand, TmemLoadEpoch,
    TmemLoadKind as NeutralTmemLoadKind, TmemLoadSemantics,
    TmemTransferLayout as NeutralTmemTransferLayout, TriangleSource,
};
use fn64_render_ir::PhysicalMemoryLayout;

use crate::raw_dpc::{decode_triangle_vertices, texture_rectangle_vertices};
use crate::state::OtherMode;
use crate::{
    DecodedRawDpc, ImageFormat, PixelSize, RawDpcCommandKind, RawDpcResourcePlan, TextureImage,
    TileAddressMode, TileDescriptor, TileIndex, TileSize, TmemLoad, TmemLoadKind,
    TmemTransferLayout, TmemTransferPhysicalWord, TmemTransferWord,
};

/// A decoded raw-DPC command this production seam does not admit. Every
/// command kind carried by [`RawDpcCommandKind`] outside `SetTextureImage`/
/// `SetTile`/`SetTileSize`/`LoadSync`/`LoadBlock`/`LoadTile`/`LoadTlut`, the
/// nine pure-RDP-state commands (`SetOtherMode`/`SetColorImage`/
/// `SetFillColor`/`SetEnvColor`/`SetPrimColor`/`SetBlendColor`/
/// `SetFogColor`/`SetPrimDepth`/`SetCombine`), `RawTriangle`,
/// `TextureRectangle`, `FillRectangle`, and `FullSync` is rejected here,
/// loudly, at the exact command index/location it was decoded at -- never
/// silently dropped or aliased to a no-op push. No command kind is blanket-
/// rejected any more. `NoOp` is not rejected: it is admitted and discarded
/// (see `push_decoded_raw_dpc`'s `NoOp` arm), never producing this error.
///
/// Two admitted kinds can still reach this error through their own narrowed
/// rejections: a `FillRectangle` whose staged `SetColorImage`/`SetFillColor`
/// this walk never observed is reported here rather than executed against
/// invented state, and a `FullSync` whose capture carries no boundary record
/// is reported here rather than admitted as a site the producer never
/// reserved.
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

/// A `RawTriangle` command was walked with no `OtherMode` established yet
/// either by an earlier command in this same plan's own command order, or
/// by durable state carried in from before this capture
/// (`decoded.base_state`, itself set from `WgpuBackend`'s own durable
/// `rdp_state` -- see `production_adapter.rs`'s `current_other_mode` doc).
/// `current_other_mode`'s `texture_perspective()` bit feeds
/// `decode_triangle_vertices` directly (RT64:
/// `state->rdp->otherMode.textPersp()`) and changes decoded triangle
/// geometry, so there is no safe silent default here (AGENTS.md "loud
/// traps, no silent shrugs") -- unlike `StateIdentityTracker`'s `before`
/// fields, which stay `None` for a plan's first occurrence of a state
/// command and never feed a decode computation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriangleBeforeAnyOtherMode {
    pub command_index: u32,
    pub location: crate::RawDpcCommandLocation,
}

impl core::fmt::Display for TriangleBeforeAnyOtherMode {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "raw-DPC triangle command #{} at {} has no SetOtherMode established yet -- neither \
             earlier in this plan nor carried in from durable state before this capture; \
             texture_perspective() cannot be decoded from an unstated OtherMode",
            self.command_index, self.location
        )
    }
}

impl std::error::Error for TriangleBeforeAnyOtherMode {}

/// A `TextureRectangle`/`TextureRectangleFlip` command was walked with no
/// `OtherMode` established yet, either by an earlier command in this same
/// plan's own command order or by durable state carried in from before this
/// capture (mirrors [`TriangleBeforeAnyOtherMode`]'s rationale exactly:
/// `texture_rectangle_vertices`'s `cycle_type` parameter feeds its copy/fill
/// rounding branches directly and changes decoded rectangle geometry, so
/// there is no safe silent default here either).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureRectangleBeforeAnyOtherMode {
    pub command_index: u32,
    pub location: crate::RawDpcCommandLocation,
}

impl core::fmt::Display for TextureRectangleBeforeAnyOtherMode {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "raw-DPC texture-rectangle command #{} at {} has no SetOtherMode established yet -- \
             neither earlier in this plan nor carried in from durable state before this capture; \
             cycle_type cannot be decoded from an unstated OtherMode",
            self.command_index, self.location
        )
    }
}

impl std::error::Error for TextureRectangleBeforeAnyOtherMode {}

/// An admitted `TextureRectangle`/`TextureRectangleFlip` command whose wire
/// bounds are reversed or empty after copy/fill-mode rounding --
/// `texture_rectangle_vertices` returning `None`, RT64's own exact
/// `FixedRect::isEmpty()` early return (see that function's doc). This must
/// surface as a loud, named rejection, never a silently-skipped command and
/// never a vacuously-successful zero-area draw (AGENTS.md "loud traps, no
/// silent shrugs").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DegenerateTextureRectangle {
    pub command_index: u32,
    pub location: crate::RawDpcCommandLocation,
}

impl core::fmt::Display for DegenerateTextureRectangle {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "raw-DPC texture-rectangle command #{} at {} is empty or reversed after copy/fill-mode \
             rounding -- texture_rectangle_vertices returned None, matching RT64's own \
             FixedRect::isEmpty() early return",
            self.command_index, self.location
        )
    }
}

impl std::error::Error for DegenerateTextureRectangle {}

/// [`push_decoded_raw_dpc`]'s complete rejection set: either an unadmitted
/// command kind, an admitted `RawTriangle` that arrived before this plan's
/// first `SetOtherMode`, an admitted `TextureRectangle` that arrived before
/// this plan's first `SetOtherMode`, or an admitted `TextureRectangle` whose
/// bounds are degenerate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushDecodedRawDpcError {
    Unadmitted(UnadmittedRawDpcCommand),
    TriangleBeforeAnyOtherMode(TriangleBeforeAnyOtherMode),
    TextureRectangleBeforeAnyOtherMode(TextureRectangleBeforeAnyOtherMode),
    DegenerateTextureRectangle(DegenerateTextureRectangle),
    /// An admitted `FillRectangle`'s recorded access span could not be
    /// bound back to the plan's own ordered access list -- the decoder and
    /// the resource plan disagree about what this fill writes.
    FillAccessSpan(crate::raw_dpc::FillAccessSpanError),
}

impl core::fmt::Display for PushDecodedRawDpcError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unadmitted(error) => core::fmt::Display::fmt(error, formatter),
            Self::TriangleBeforeAnyOtherMode(error) => core::fmt::Display::fmt(error, formatter),
            Self::TextureRectangleBeforeAnyOtherMode(error) => {
                core::fmt::Display::fmt(error, formatter)
            }
            Self::DegenerateTextureRectangle(error) => core::fmt::Display::fmt(error, formatter),
            Self::FillAccessSpan(error) => core::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for PushDecodedRawDpcError {}

impl From<UnadmittedRawDpcCommand> for PushDecodedRawDpcError {
    fn from(error: UnadmittedRawDpcCommand) -> Self {
        Self::Unadmitted(error)
    }
}

impl From<TriangleBeforeAnyOtherMode> for PushDecodedRawDpcError {
    fn from(error: TriangleBeforeAnyOtherMode) -> Self {
        Self::TriangleBeforeAnyOtherMode(error)
    }
}

impl From<TextureRectangleBeforeAnyOtherMode> for PushDecodedRawDpcError {
    fn from(error: TextureRectangleBeforeAnyOtherMode) -> Self {
        Self::TextureRectangleBeforeAnyOtherMode(error)
    }
}

impl From<DegenerateTextureRectangle> for PushDecodedRawDpcError {
    fn from(error: DegenerateTextureRectangle) -> Self {
        Self::DegenerateTextureRectangle(error)
    }
}

fn opcode_name(kind: &RawDpcCommandKind) -> &'static str {
    match kind {
        RawDpcCommandKind::NoOp { .. } => "NoOp",
        // Still named, deliberately, even though `FillRectangle` is now
        // admitted rather than blanket-rejected: this arm remains reachable
        // from the narrowed fill rejections in `push_decoded_raw_dpc`'s own
        // `FillRectangle` arm (a fill whose staged `SetColorImage`/
        // `SetFillColor` the walk never saw). Deleting it would make those
        // rejections hit the `unreachable!` below and panic instead of
        // naming the opcode. `NoOp` above is already in the same defensive
        // state -- it is admitted and discarded, so its arm is likewise
        // unreachable in practice.
        RawDpcCommandKind::FillRectangle(_) => "FillRectangle",
        // Likewise still named after FullSync became a site admission: the
        // `FullSync` arm's own narrowed rejection (a capture carrying no
        // boundary record for the site) routes through here.
        RawDpcCommandKind::FullSync(_) => "FullSync",
        RawDpcCommandKind::RawTriangle(_) => "RawTriangle",
        RawDpcCommandKind::SetOtherMode(_)
        | RawDpcCommandKind::SetScissor(_)
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
        | RawDpcCommandKind::LoadTlut(_)
        | RawDpcCommandKind::TextureRectangle(_) => {
            unreachable!("admitted kinds are pushed, never rejected")
        }
    }
}

/// Widen one `SetScissor` decode into its neutral mirror.
///
/// [`crate::rt64_gbi_rdp_decode::decode_set_scissor`] types its four
/// coordinates `i32` to mirror RT64's `int32_t` locals, but every one is a
/// zero-extended 12-bit extraction (`p0`/`p1` mask to `bits` width and never
/// sign-extend), so the true domain is `0..=4095` -- comfortably inside
/// `u16`. The `expect`s below are loud traps on that invariant rather than
/// silent `as` truncations: if the decoder is ever changed to sign-extend,
/// this panics naming the field instead of quietly wrapping a negative
/// coordinate into a huge positive one.
fn neutral_scissor(value: crate::rt64_gbi_rdp_decode::SetScissorDecoded) -> NeutralScissor {
    fn coordinate(field: &str, raw: i32) -> u16 {
        u16::try_from(raw).unwrap_or_else(|_| {
            panic!(
                "SetScissor {field} decoded to {raw}, outside the zero-extended 12-bit \
                 domain 0..=4095 that decode_set_scissor is proven to produce"
            )
        })
    }
    NeutralScissor {
        mode: value.mode,
        upper_left_x: coordinate("ulx", value.ulx),
        upper_left_y: coordinate("uly", value.uly),
        lower_right_x: coordinate("lrx", value.lrx),
        lower_right_y: coordinate("lry", value.lry),
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

fn neutral_triangle_vertex(vertex: crate::raw_dpc::TriangleVertex) -> NeutralTriangleVertex {
    NeutralTriangleVertex {
        x: vertex.x(),
        y: vertex.y(),
        z: vertex.z(),
        w: vertex.w(),
        color: vertex.color(),
        texcoord: vertex.texcoord(),
    }
}

/// Maps one [`crate::raw_dpc::TextureRectangleVertex`] (RT64's
/// `triPosFloats`/`triColorFloats`/`triTcFloats` push, four-component
/// clip-space-ready position, always-zero color, one texcoord pair) onto
/// [`NeutralTriangleVertex`]'s field shape so a texture rectangle's six
/// converted vertices can reuse `RdpTriangleCommand`/`push_triangle` exactly
/// like a `RawTriangle`'s three (§3b Option A: no new `fn64-render` DTO).
/// `position()` is `[x, y, z, w]`; `w` is always `1.0` per
/// `texture_rectangle.rs`'s `RECT_POS_FLOATS`, matching `NeutralTriangleVertex`'s
/// separate `x`/`y`/`z`/`w` fields one-for-one.
fn neutral_texture_rectangle_vertex(
    vertex: crate::raw_dpc::TextureRectangleVertex,
) -> NeutralTriangleVertex {
    let [x, y, z, w] = vertex.position();
    NeutralTriangleVertex {
        x,
        y,
        z,
        w,
        color: vertex.color(),
        texcoord: vertex.texcoord(),
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

/// Slices one command's own raw wire words out of the capture's full word
/// stream, sized by the command's own declared width rather than any fixed
/// assumption. Unlike [`tmem_command_raw_words`]'s fixed 2-word assumption,
/// every command routed through this function is wider than 2 words -- a
/// `RawTriangle` is variable-width (32..=176 bytes, per its opcode's
/// optional shade/texture/depth coefficient blocks) and a `TextureRectangle`/
/// `TextureRectangleFlip` is a fixed 16 bytes / 4 words
/// (`TEXTURE_RECTANGLE_COMMAND_BYTES`) -- so this keys the read off
/// [`fn64_render::raw_rdp_command_width`] -- the same function
/// `decode_stream` (`mod.rs:890`) already uses to size every wide command's
/// own read, rather than reimplementing that stride table here. This
/// width-driven design is deliberate: any future command kind wider than 2
/// words can route through this same slicer by width alone, with no new
/// per-kind branch and no risk of the kind of silent truncation
/// [`tmem_command_raw_words`]'s fixed-2-word reader would cause if misapplied
/// to a wider command (e.g. a texture rectangle's 4-word payload truncated to
/// 2 words).
fn width_keyed_command_raw_words(
    capture_words: &[u32],
    submission_start: u32,
    old: crate::RawDpcCommandLocation,
) -> Vec<u32> {
    let width_bytes = fn64_render::raw_rdp_command_width(old.wire_opcode())
        .expect("decode_stream already proved this opcode has a known width");
    let width_words = (width_bytes / 4) as usize;
    let start = ((old.source_byte_offset() - submission_start) / 4) as usize;
    let words = capture_words
        .get(start..start + width_words)
        .expect("decode_stream already proved this command's bytes are in-bounds");
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
    /// Tracked-only, exactly like the applied slots above -- `SetScissor`
    /// still owns one global slot whose `before`/`after` chain threads
    /// through this plan, even though no consumer reads the value.
    scissor: Option<RdpStateIdentity>,
}

fn tile_slot(index: TileIndex) -> usize {
    usize::from(index.get())
}

/// Whether the capture backing `writer` carries a boundary record for the
/// FullSync site at `ordinal`.
///
/// This is the seam's proof that the producer took the reserve half. A
/// boundary can only enter a capture through
/// `OwnedRawDpcCapture::with_full_sync_boundaries`, whose contract requires
/// `DeviceFabric::preflight_dp_full_sync` to have proved the sole DP
/// completion slot free first; `OwnedRawDpcCapture::new` installs an empty
/// list and no other constructor exists.
///
/// In practice this always returns `true` for a decoded site, because IR
/// stream derivation already refuses to build a stream whose `SYNC_FULL`
/// count exceeds its boundary count (`MissingFullSyncObservation`). It is
/// checked anyway rather than asserted: the alternative is admitting a site
/// on an assumption about a sibling crate's invariant, and a loud rejection
/// costs nothing on a path that is not hot.
fn full_sync_reserved_by_capture(writer: &ExactRawDpcPlanWriter, ordinal: u32) -> bool {
    writer.capture().full_sync_boundaries().len() > ordinal as usize
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
) -> Result<(), PushDecodedRawDpcError> {
    let resource_plan: &RawDpcResourcePlan = decoded.resource_plan();
    let mut tracker = StateIdentityTracker::default();
    // `decode_triangle_vertices`'s own `texture_perspective` parameter is
    // the live `G_TP_PERSP` OtherMode bit at the point a triangle command is
    // decoded (RT64: `state->rdp->otherMode.textPersp()`), not this plan's
    // final value -- mirrored locally here the same way `tracker` mirrors
    // per-slot identity, since `DecodedRawDpcCommand` carries no OtherMode
    // field of its own. Updated only inside the `SetOtherMode` arm below, so
    // a triangle at stream position N always sees the most recent
    // `SetOtherMode` at position < N in *this decode's own command order*,
    // never a later one and never this plan's final value.
    //
    // Seeded from `decoded.base_state` -- the durable RDP state the caller
    // held *before* this exact decode (`raw_dpc::mod`'s `decode_from_state`:
    // `let base_state = state.fork_for_decode();`, stored on `DecodedRawDpc`
    // itself). `production_adapter` is declared `mod production_adapter;`
    // inside `raw_dpc/mod.rs`, so `base_state`'s no-modifier (module-tree)
    // visibility already reaches this file with no new accessor. This is
    // NOT a fabricated default: it is the same real prior state
    // `WgpuBackend` (`production.rs`) already threads across submissions
    // via `rdp_state`/`rdp_state.apply`, read here rather than duplicated.
    //
    // Still `None` only when this plan's OWN first submission has never had
    // a `SetOtherMode` at all (a fresh renderer with no prior state) --
    // deliberately NOT defaulted to wire `(0, 0)` in that case (AGENTS.md
    // "loud traps, no silent shrugs"): this value feeds
    // `texture_perspective()` directly into `decode_triangle_vertices`,
    // changing decoded triangle geometry, so a silent default would be a
    // real correctness risk, not mere bookkeeping the way
    // `StateIdentityTracker`'s `before` fields are (they stay `None` for a
    // plan's first occurrence and never feed a decode computation).
    // Updated again inside the `SetOtherMode` arm below on every in-plan
    // occurrence, so a triangle at stream position N always sees the most
    // recent `SetOtherMode` at position < N -- either carried in from
    // `base_state` or set later in this same plan -- never a later one and
    // never this plan's final value.
    let mut current_other_mode: Option<OtherMode> = decoded.base_state.other_mode();

    // The staged `SetColorImage`/`SetFillColor` *values* (not merely their
    // `RdpStateIdentity`s, which `tracker` already carries) current at the
    // walk's position. An admitted `FillRectangle` copies both onto its own
    // neutral command so the execution-time color-target identity is derived
    // from the same values plan time used, rather than re-tracked
    // independently at the far end.
    //
    // Seeded from `decoded.base_state` for the same reason
    // `current_other_mode` is: a `FillRectangle` may legitimately depend on
    // a `SetColorImage` issued by an earlier submission. `plan_fill`'s own
    // admission gate reads the identical durable state, so a fill this loop
    // sees is one `plan_fill` already proved has both staged.
    let mut current_color_image: Option<crate::ColorImage> = decoded.base_state.color_image();
    let mut current_fill_color: Option<crate::FillColor> = decoded.base_state.fill_color();

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
        // Every admitted TMEM/state opcode is a fixed 2-word command; a
        // `RawTriangle` is variable-width and a `TextureRectangle` is a
        // fixed 4-word (16-byte) command -- both wider than 2 words, so
        // both must go through `width_keyed_command_raw_words`'s
        // `raw_rdp_command_width`-keyed reader (see its own doc), never the
        // fixed-2-word slicer, or a texture rectangle's payload would be
        // silently truncated from 4 words to 2. `NoOp` and the two rejected
        // kinds below (`FillRectangle`/`FullSync`) never reach a
        // `push_state`/`push_triangle` call, and `FullSync` is itself a
        // fixed 2-word command, so slicing all three with the fixed-2-word
        // reader (their own wire shape) is exact. `NoOp`'s
        // `raw_words`/`location` values are computed but discarded.
        let raw_words = if matches!(
            command.kind(),
            RawDpcCommandKind::RawTriangle(_) | RawDpcCommandKind::TextureRectangle(_)
        ) {
            width_keyed_command_raw_words(capture_words, submission_start, old_location)
        } else {
            tmem_command_raw_words(capture_words, submission_start, old_location)
        };
        let location = neutral_location(
            command_index,
            old_location,
            layout,
            u32::try_from(raw_words.len() * 4).expect("command word count fits u32 bytes"),
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
                current_other_mode = Some(value);
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
                current_color_image = Some(image);
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
                current_fill_color = Some(value);
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
            RawDpcCommandKind::SetScissor(value) => {
                let neutral = neutral_scissor(value);
                let after = RdpStateIdentity::of_scissor(neutral);
                let before = tracker.scissor;
                tracker.scissor = Some(after);
                writer.push_state(RdpStateCommand::SetScissor {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    scissor: neutral,
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
            RawDpcCommandKind::RawTriangle(triangle) => {
                let Some(other_mode) = current_other_mode else {
                    return Err(TriangleBeforeAnyOtherMode {
                        command_index,
                        location: old_location,
                    }
                    .into());
                };
                let decoded = decode_triangle_vertices(&triangle, other_mode.texture_perspective());
                let vertices =
                    core::array::from_fn(|index| neutral_triangle_vertex(decoded.vertex(index)));
                writer.push_triangle(RdpTriangleCommand {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    vertices,
                    source: TriangleSource::RawTriangle,
                    viewport: None,
                });
            }
            RawDpcCommandKind::TextureRectangle(rectangle) => {
                let Some(other_mode) = current_other_mode else {
                    return Err(TextureRectangleBeforeAnyOtherMode {
                        command_index,
                        location: old_location,
                    }
                    .into());
                };
                let Some(vertices) = texture_rectangle_vertices(rectangle, other_mode.cycle_type())
                else {
                    return Err(DegenerateTextureRectangle {
                        command_index,
                        location: old_location,
                    }
                    .into());
                };
                // `texture_rectangle_vertices` returns exactly six vertices
                // forming two triangles, RT64's own two-triangle push order
                // for one rectangle (`texture_rectangle.rs`'s module doc:
                // "two triangles... `flip` swapping only the texcoord
                // pairing"). §3b Option A (this card's recommended default,
                // taken here): no new `fn64-render` DTO -- split the six
                // vertices into the two three-vertex groups (0,1,2 and
                // 3,4,5) and push each through the same
                // `RdpTriangleCommand`/`push_triangle` path a `RawTriangle`
                // already uses, so a texture rectangle becomes, from the
                // collector's perspective, "two more triangles in the
                // stream" with no new command-kind branch anywhere
                // downstream.
                let first: [NeutralTriangleVertex; 3] = core::array::from_fn(|index| {
                    neutral_texture_rectangle_vertex(vertices.vertex(index))
                });
                let second: [NeutralTriangleVertex; 3] = core::array::from_fn(|index| {
                    neutral_texture_rectangle_vertex(vertices.vertex(index + 3))
                });
                // Both triangle halves come from the same origin command
                // (one texture rectangle = one wire command producing two
                // triangles, not two independent wire commands), so both
                // pushes deliberately reuse the identical `location`/
                // `raw_words` content -- `location` is `Copy`
                // (`NeutralRawDpcCommandLocation`), and `raw_words` is
                // cloned once here (`Vec<u32>::clone` then
                // `into_boxed_slice` on each half) rather than shared via
                // `Rc`/`Arc`, matching `RdpTriangleCommand::raw_words`'s
                // plain `Box<[u32]>` field shape -- a rectangle's wire words
                // are a handful of `u32`s, so this is not a hot-path
                // allocation concern.
                // The rectangle's destination writes come first, so
                // `writer.accesses` stays in the decoder's own order: the
                // decoder pushed these at the point it decoded this command,
                // and `ExactRawDpcPlanWriter::finish` compares the two lists
                // position by position. The slice is the decoder's own,
                // never re-derived here -- the same contract
                // `push_fill_rectangle`'s doc states.
                let texrect_accesses = resource_plan
                    .bind_texture_rectangle(command_index)
                    .map_err(PushDecodedRawDpcError::FillAccessSpan)?;
                writer.push_texture_rectangle_accesses(texrect_accesses);
                writer.push_triangle(RdpTriangleCommand {
                    location,
                    raw_words: raw_words.clone().into_boxed_slice(),
                    vertices: first,
                    source: TriangleSource::TextureRectangle,
                    viewport: Some(vertices.viewport),
                });
                writer.push_triangle(RdpTriangleCommand {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    vertices: second,
                    source: TriangleSource::TextureRectangle,
                    viewport: Some(vertices.viewport),
                });
            }
            RawDpcCommandKind::NoOp { .. } => {}
            RawDpcCommandKind::FillRectangle(rectangle) => {
                // Narrow admission. `plan_fill` has already refused every
                // fill this backend cannot execute -- non-Fill cycle,
                // missing `SetColorImage`/`SetFillColor`, a non-RGBA16/32
                // color image, fractional or reversed edges, a rectangle
                // wider than the staged image, or a write outside installed
                // RDRAM -- as a `RawDpcDecodeError::InvalidCommand` before
                // this loop ever runs. Reaching this arm therefore means the
                // decoder already proved the fill admissible; nothing is
                // silently downgraded to a zero-write no-op here.
                //
                // The access slice comes from the decoder's own recorded
                // span, never re-derived: `ExactRawDpcPlanWriter::finish`
                // proves the pushed accesses equal the journal's one for
                // one, and a second independent derivation of the same
                // geometry would turn that sealed guarantee into a runtime
                // coin flip.
                let (span, accesses) = resource_plan
                    .bind_fill_rectangle(command_index)
                    .map_err(PushDecodedRawDpcError::FillAccessSpan)?;
                let Some(color_image) = current_color_image else {
                    return Err(UnadmittedRawDpcCommand {
                        command_index,
                        location: old_location,
                        opcode_name: opcode_name(&command.kind()),
                    }
                    .into());
                };
                let Some(fill_color) = current_fill_color else {
                    return Err(UnadmittedRawDpcCommand {
                        command_index,
                        location: old_location,
                        opcode_name: opcode_name(&command.kind()),
                    }
                    .into());
                };
                let neutral_image = neutral_color_image(color_image, layout);
                let neutral_fill = neutral_fill_color(fill_color);
                let after = RdpStateIdentity::of_color_image(neutral_image);
                writer.push_fill_rectangle(
                    RdpFillRectangleCommand {
                        location,
                        raw_words: raw_words.into_boxed_slice(),
                        upper_left_x: rectangle.upper_left_x(),
                        upper_left_y: rectangle.upper_left_y(),
                        lower_right_x: rectangle.lower_right_x(),
                        lower_right_y: rectangle.lower_right_y(),
                        color_image: neutral_image,
                        fill_color: neutral_fill,
                        first_access_index: span.first_access_index(),
                        access_count: span.count(),
                        before: tracker.color_image,
                        after,
                    },
                    accesses,
                );
            }
            RawDpcCommandKind::FullSync(occurrence) => {
                // Site-only admission. Reaching this arm means the decoder
                // already found a capture-time `FullSyncBoundary` at exactly
                // this stream offset and proved its chunk/source identity
                // matches (`raw_dpc::mod`'s `FULL_SYNC` arm rejects both
                // failures as `InvalidCommand` before this loop runs), so the
                // boundary carried below is the decoder's own, not one
                // re-derived here.
                //
                // A capture can only carry a boundary through
                // `OwnedRawDpcCapture::with_full_sync_boundaries`, whose
                // contract requires the producer to have reserved the sole DP
                // completion slot via the nonmutating
                // `DeviceFabric::preflight_dp_full_sync` before building it.
                // `OwnedRawDpcCapture::new` installs an empty list, so a
                // producer that never reserved cannot reach this arm at all:
                // its stream derivation fails with
                // `MissingFullSyncObservation` before decode.
                //
                // NONCLAIM. Admitting the site claims the opcode was walked
                // and the slot was free. It claims nothing about a DP
                // interrupt being raised or observed. The only observation
                // claim in the whole path is
                // `occurrence.interrupt_after == Asserted`, carried verbatim
                // below and never synthesized here -- see `RdpFullSyncSite`.
                let dp_slot_reserved = full_sync_reserved_by_capture(writer, occurrence.ordinal);
                if !dp_slot_reserved {
                    return Err(UnadmittedRawDpcCommand {
                        command_index,
                        location: old_location,
                        opcode_name: opcode_name(&command.kind()),
                    }
                    .into());
                }
                writer.push_full_sync_site(fn64_render::RdpFullSyncSite {
                    location,
                    raw_words: raw_words.into_boxed_slice(),
                    ordinal: occurrence.ordinal,
                    boundary: fn64_render_ir::FullSyncBoundary::new(
                        occurrence.sequence,
                        occurrence.interrupt_sequence,
                        occurrence.interrupt_before,
                        occurrence.interrupt_after,
                    ),
                    dp_slot_reserved,
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

    use crate::{decode_raw_dpc, CycleType, RawDpcDecodeError, RdpState};

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
    /// Spelled independently of the decoder's own `SET_SCISSOR`, exactly
    /// like every sibling above -- so a typo in either one is caught by
    /// `set_scissor_is_admitted_and_matches_the_decoded_command` rather
    /// than cancelling out.
    const SET_SCISSOR: u8 = 0x2d;

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

    /// `SetScissor` wire words: `w0` payload packs `ulx << 12 | uly`, `w1`
    /// packs `mode << 24 | lrx << 12 | lry`.
    fn set_scissor(mode: u32, ulx: u32, uly: u32, lrx: u32, lry: u32) -> [u32; 2] {
        [
            word(SET_SCISSOR, ulx << 12 | uly),
            mode << 24 | lrx << 12 | lry,
        ]
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

    /// Every full-sync boundary this fixture's `words` requires, derived the
    /// same way `raw_dpc::mod::tests`'s own `packet()` helper does: scan
    /// each 2-word command slot for the `FULL_SYNC` opcode byte and record
    /// its exact stream/source position. `preflight_raw_dpc_capture` has no
    /// auto-derivation of its own -- a caller must supply this list.
    ///
    /// `interrupt_after` is `Clear`, matching what the real ABI producer
    /// (`rsp_commit.rs`'s `try_dispatch_raw_dpc_via_session`) supplies: it
    /// reserves the DP completion slot but cannot observe the interrupt,
    /// which the device fabric raises only on a later `advance_to`. A
    /// fixture claiming `Asserted` would be testing an observation the
    /// production path does not make.
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
                    fn64_render_ir::DpInterruptState::Clear,
                )
            })
            .collect()
    }

    fn decode_admitted_capture(
        words: Vec<u32>,
        source_range: (u32, u32),
    ) -> (DecodedRawDpc, OwnedRawDpcCapture, ResourceJournal) {
        decode_admitted_capture_with_state(words, source_range, RdpState::default())
    }

    /// Same as [`decode_admitted_capture`], but decodes against a
    /// caller-supplied `RdpState` rather than a fresh `default()` -- the
    /// "durable state a caller held before this capture" seam
    /// `decode_from_state`'s `state.fork_for_decode()` stores as
    /// `DecodedRawDpc::base_state`, exercised by the cross-submission
    /// admission test.
    fn decode_admitted_capture_with_state(
        words: Vec<u32>,
        source_range: (u32, u32),
        initial_state: RdpState,
    ) -> (DecodedRawDpc, OwnedRawDpcCapture, ResourceJournal) {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let end = COMMAND_START + u32::try_from(words.len() * 4).unwrap();
        let full_syncs = full_sync_boundaries(&words);
        let submission =
            OwnedRawDpcSubmission::from_rdram_words(COMMAND_START, end, words.clone()).unwrap();
        // Carry the boundaries on the capture itself, not only into
        // `finalize_ticket`. `push_decoded_raw_dpc` reads
        // `writer.capture().full_sync_boundaries()` to prove the producer
        // took the reserve half, so a fixture that supplied them to the
        // ticket alone would exercise the narrowed rejection instead of the
        // site admission it means to test.
        let capture = OwnedRawDpcCapture::with_full_sync_boundaries(
            submission,
            layout,
            7,
            TemporalBoundary::new(1, fn64_render_ir::DpInterruptState::Clear),
            full_syncs.clone(),
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
        let journal = match decode_raw_dpc(probe_submitted, &initial_state) {
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
        let decoded = decode_raw_dpc(submitted, &initial_state).expect("fixture decodes cleanly");
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
        triangles: Vec<fn64_render::RdpTriangleCommand>,
        full_sync_sites: Vec<fn64_render::RdpFullSyncSite>,
        accesses: Vec<fn64_render_ir::ResourceAccess>,
    }

    impl ExactRawDpcPlanVisitor for RecordingVisitor {
        fn command(&mut self, command: RawDpcSemanticCommandRef<'_>) {
            match command {
                RawDpcSemanticCommandRef::TmemLoad(load) => self.loads.push(load.clone()),
                RawDpcSemanticCommandRef::State(state) => self.states.push(state.clone()),
                RawDpcSemanticCommandRef::Triangle(triangle) => {
                    self.triangles.push(triangle.clone());
                }
                RawDpcSemanticCommandRef::FullSyncSite(site) => {
                    self.full_sync_sites.push(site.clone());
                }
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

    // -- SetScissor (tracked state only) --------------------------------

    #[test]
    fn set_scissor_is_admitted_and_matches_the_decoded_command() {
        // ulx = 0x0A0 (160), uly = 0x0B0 (176), lrx = 0x0C0 (192),
        // lry = 0x0D0 (208), mode = 1 -- hand-derived, each inside its own
        // 12-bit (coordinate) or 2-bit (mode) field.
        let words = set_scissor(1, 0x0A0, 0x0B0, 0x0C0, 0x0D0).to_vec();
        let (decoded, capture, journal) = decode_admitted_capture(words.clone(), (0x214, 0x224));
        let RawDpcCommandKind::SetScissor(source) = decoded.commands()[0].kind() else {
            panic!("expected SetScissor");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(
            plan.states.len(),
            1,
            "SetScissor must reach the plan as a state command, exactly as SetFogColor does"
        );
        let RdpStateCommand::SetScissor {
            raw_words,
            scissor,
            before,
            after,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetScissor");
        };
        assert_eq!(raw_words.as_ref(), words.as_slice());
        assert_eq!(*scissor, neutral_scissor(source));
        assert!(
            before.is_none(),
            "first SetScissor in the plan has no prior"
        );
        assert_eq!(*after, RdpStateIdentity::of_scissor(*scissor));

        // Hand-derived field values, independent of the decoder.
        assert_eq!(scissor.mode, 1);
        assert_eq!(scissor.upper_left_x, 0x0A0);
        assert_eq!(scissor.upper_left_y, 0x0B0);
        assert_eq!(scissor.lower_right_x, 0x0C0);
        assert_eq!(scissor.lower_right_y, 0x0D0);
    }

    #[test]
    fn set_scissor_pushes_zero_resource_accesses() {
        // A pure state command, like SetFogColor: it plans no reads and no
        // writes of its own, so it can neither touch RDRAM nor reorder
        // anything that does. Both fixtures are one two-word command, so
        // even the command-decode read spans match byte for byte.
        let words = set_scissor(0, 0, 0, 320 * 4, 240 * 4).to_vec();
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let plan = push_and_visit(&decoded, capture, journal);
        let fog_words = set_fog_color(0x11223344).to_vec();
        let (fog_decoded, fog_capture, fog_journal) =
            decode_admitted_capture(fog_words, (0x214, 0x224));
        let fog_plan = push_and_visit(&fog_decoded, fog_capture, fog_journal);
        assert_eq!(
            plan.accesses, fog_plan.accesses,
            "SetScissor must declare exactly the accesses a SetFogColor does -- none of its own"
        );
    }

    #[test]
    fn set_scissor_threads_before_after_identity_across_two_occurrences() {
        let mut words = Vec::new();
        words.extend(set_scissor(0, 0, 0, 320 * 4, 240 * 4));
        words.extend(set_scissor(1, 16, 16, 300 * 4, 220 * 4));
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 2);

        let RdpStateCommand::SetScissor {
            before: b0,
            after: a0,
            scissor: v0,
            ..
        } = &plan.states[0]
        else {
            panic!("expected SetScissor first");
        };
        let RdpStateCommand::SetScissor {
            before: b1,
            after: a1,
            scissor: v1,
            ..
        } = &plan.states[1]
        else {
            panic!("expected SetScissor second");
        };
        assert_ne!(v0, v1, "the fixture's two rects must differ");
        assert!(b0.is_none());
        assert_eq!(*b1, Some(*a0), "the second's before is the first's after");
        assert_ne!(a0, a1, "distinct rects must hash to distinct identities");
    }

    #[test]
    fn set_scissor_identity_is_disjoint_from_every_other_state_slot() {
        // The all-zero rect must not collide with any other slot's all-zero
        // value: each identity carries its own domain tag.
        let zero = NeutralScissor {
            mode: 0,
            upper_left_x: 0,
            upper_left_y: 0,
            lower_right_x: 0,
            lower_right_y: 0,
        };
        let scissor = RdpStateIdentity::of_scissor(zero);
        assert_ne!(
            scissor,
            RdpStateIdentity::of_fog_color(NeutralColor4 { value: 0 })
        );
        assert_ne!(
            scissor,
            RdpStateIdentity::of_fill_color(NeutralFillColor { value: 0 })
        );
        assert_ne!(
            scissor,
            RdpStateIdentity::of_combine(NeutralCombineParams { low: 0, high: 0 })
        );
        assert_ne!(
            scissor,
            RdpStateIdentity::of_prim_depth(NeutralPrimDepth { z: 0, dz: 0 })
        );
    }

    /// **The card's core evidence.** Adding `SetScissor` commands to a
    /// stream must leave the produced plan's rendered effect bit-identical:
    /// every non-scissor state command, every triangle, every resource
    /// access, and every full-sync site must be exactly what the
    /// scissor-free stream produced, in exactly the same order.
    ///
    /// Because `SetScissor` is tracked state only -- it stages nothing into
    /// `RdpState` and pushes no `ResourceAccess` -- the only differences
    /// between the two plans may be the tracked `RdpStateCommand::SetScissor`
    /// entries themselves and the length of the command-decode read that
    /// covers the longer display list.
    #[test]
    fn admitting_set_scissor_changes_no_rendered_output() {
        fn stream(with_scissor: bool) -> Vec<u32> {
            let mut words = Vec::new();
            if with_scissor {
                words.extend(set_scissor(0, 0, 0, 320 * 4, 240 * 4));
            }
            words.extend(set_other_mode(3, 0)); // Fill
            if with_scissor {
                words.extend(set_scissor(1, 16, 16, 300 * 4, 220 * 4));
            }
            words.extend(set_color_image(0, 2, 8, 0x200));
            words.extend(set_fill_color(0xf801_f801));
            if with_scissor {
                words.extend(set_scissor(2, 0, 0, 0xFFF, 0xFFF));
            }
            words.extend(set_env_color(0x11223344));
            words.extend(set_prim_color(10, 5, 0x11223344));
            words.extend(set_blend_color(0x55667788));
            words.extend(set_fog_color(0x11223344));
            words.extend(set_prim_depth(100, 200));
            words.extend(set_combine(0x0034_5678, 0x9abc_def0));
            if with_scissor {
                words.extend(set_scissor(3, 4095, 4095, 4095, 4095));
            }
            words
        }

        fn plan_of(words: Vec<u32>) -> RecordingVisitor {
            let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
            push_and_visit(&decoded, capture, journal)
        }

        let without = plan_of(stream(false));
        let with = plan_of(stream(true));

        // The scissor commands really are there -- otherwise every equality
        // below is vacuous.
        let scissor_count = with
            .states
            .iter()
            .filter(|state| matches!(state, RdpStateCommand::SetScissor { .. }))
            .count();
        assert_eq!(scissor_count, 4, "all four SetScissor commands admitted");
        assert!(!without
            .states
            .iter()
            .any(|state| matches!(state, RdpStateCommand::SetScissor { .. })));

        // Everything that can reach a pixel is identical. The only
        // difference permitted is the tracked SetScissor entries.
        let with_non_scissor: Vec<_> = with
            .states
            .iter()
            .filter(|state| !matches!(state, RdpStateCommand::SetScissor { .. }))
            .collect();
        let without_states: Vec<_> = without.states.iter().collect();
        assert_eq!(
            with_non_scissor.len(),
            without_states.len(),
            "no state command may be added or dropped"
        );
        for (index, (left, right)) in with_non_scissor
            .iter()
            .zip(without_states.iter())
            .enumerate()
        {
            // `location`/`raw_words` legitimately shift (the scissor words
            // move later commands' byte offsets), so compare the variant
            // and the staged value, which are what a consumer reads.
            assert_eq!(
                std::mem::discriminant(*left),
                std::mem::discriminant(*right),
                "state command {index} changed variant"
            );
            assert_eq!(
                state_command_value_debug(left),
                state_command_value_debug(right),
                "state command {index} changed its staged value"
            );
            // The `before`/`after` identity chain must be untouched too:
            // a SetScissor that wrote into some *other* slot's tracker
            // entry would leave every staged value intact while silently
            // corrupting that slot's differential history, which T3 uses
            // to reconstruct state without rereading command bytes.
            assert_eq!(
                state_command_identities(left),
                state_command_identities(right),
                "state command {index} changed its before/after identity chain"
            );
        }

        assert_eq!(
            with.triangles.len(),
            without.triangles.len(),
            "no triangle may appear or vanish"
        );

        // Accesses: the only entry either plan declares is the
        // `CommandDecode` read of the display list itself, and the
        // scissored stream's is longer by exactly the four SetScissor
        // commands' own eight bytes each -- reading more command words is
        // not a rendered effect. Every *other* access must match exactly,
        // which here means neither plan gained one.
        fn command_decode_span(accesses: &[fn64_render_ir::ResourceAccess]) -> u64 {
            let mut spans = accesses.iter().filter_map(|access| {
                matches!(
                    access.purpose(),
                    fn64_render_ir::AccessPurpose::CommandDecode
                )
                .then(|| match access.region() {
                    fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                        u64::from(range.end()) - u64::from(range.start().get())
                    }
                    other => panic!("CommandDecode read an unexpected region: {other:?}"),
                })
            });
            let span = spans
                .next()
                .expect("every plan reads its own command words");
            assert!(spans.next().is_none(), "exactly one CommandDecode access");
            span
        }

        let non_decode = |accesses: &[fn64_render_ir::ResourceAccess]| {
            accesses
                .iter()
                .filter(|access| {
                    !matches!(
                        access.purpose(),
                        fn64_render_ir::AccessPurpose::CommandDecode
                    )
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            non_decode(&with.accesses),
            non_decode(&without.accesses),
            "SetScissor must declare no resource access of its own beyond the command read"
        );
        assert_eq!(
            command_decode_span(&with.accesses),
            command_decode_span(&without.accesses) + 4 * 8,
            "the command-decode read grows by exactly the four SetScissor commands' own bytes"
        );

        assert_eq!(with.full_sync_sites, without.full_sync_sites);
        assert_eq!(with.loads, without.loads);
    }

    /// The `before`/`after` identity pair one state command threads through
    /// its own slot. `SyncLoad` has no identity pair (it threads TMEM load
    /// epochs instead), so it reports `None`.
    ///
    /// Deliberately an **exhaustive** match with no `_` arm, for the same
    /// reason [`state_command_value_debug`] is.
    fn state_command_identities(
        command: &RdpStateCommand,
    ) -> Option<(Option<RdpStateIdentity>, RdpStateIdentity)> {
        match command {
            RdpStateCommand::SetOtherMode { before, after, .. }
            | RdpStateCommand::SetColorImage { before, after, .. }
            | RdpStateCommand::SetFillColor { before, after, .. }
            | RdpStateCommand::SetEnvColor { before, after, .. }
            | RdpStateCommand::SetPrimColor { before, after, .. }
            | RdpStateCommand::SetBlendColor { before, after, .. }
            | RdpStateCommand::SetFogColor { before, after, .. }
            | RdpStateCommand::SetPrimDepth { before, after, .. }
            | RdpStateCommand::SetCombine { before, after, .. }
            | RdpStateCommand::SetScissor { before, after, .. }
            | RdpStateCommand::SetTile { before, after, .. }
            | RdpStateCommand::SetTileSize { before, after, .. }
            | RdpStateCommand::SetTextureImage { before, after, .. } => Some((*before, *after)),
            RdpStateCommand::SyncLoad { .. } => None,
        }
    }

    /// The staged value of one state command, excluding `location` and
    /// `raw_words` (both of which legitimately shift when other commands
    /// are inserted ahead of them in the stream).
    ///
    /// Deliberately an **exhaustive** match with no `_` arm: a future
    /// `RdpStateCommand` variant must be classified here explicitly rather
    /// than silently escaping the no-pixel-change comparison above.
    fn state_command_value_debug(command: &RdpStateCommand) -> String {
        match command {
            RdpStateCommand::SetOtherMode { other_mode, .. } => format!("{other_mode:?}"),
            RdpStateCommand::SetColorImage { image, .. } => format!("{image:?}"),
            RdpStateCommand::SetFillColor { color, .. } => format!("{color:?}"),
            RdpStateCommand::SetEnvColor { color, .. } => format!("{color:?}"),
            RdpStateCommand::SetPrimColor { color, .. } => format!("{color:?}"),
            RdpStateCommand::SetBlendColor { color, .. } => format!("{color:?}"),
            RdpStateCommand::SetFogColor { color, .. } => format!("{color:?}"),
            RdpStateCommand::SetPrimDepth { depth, .. } => format!("{depth:?}"),
            RdpStateCommand::SetCombine { combine, .. } => format!("{combine:?}"),
            RdpStateCommand::SetScissor { scissor, .. } => format!("{scissor:?}"),
            RdpStateCommand::SetTile {
                tile_index,
                descriptor,
                ..
            } => format!("{tile_index:?}{descriptor:?}"),
            RdpStateCommand::SetTileSize {
                tile_index, size, ..
            } => format!("{tile_index:?}{size:?}"),
            RdpStateCommand::SetTextureImage { image, .. } => format!("{image:?}"),
            RdpStateCommand::SyncLoad {
                input_epoch,
                output_epoch,
                ..
            } => format!("{input_epoch:?}{output_epoch:?}"),
        }
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

    /// A FullSync whose capture carries the matching boundary record is
    /// admitted as a *site*: pushed into the plan, declaring zero resource
    /// accesses, carrying the decoder's own boundary verbatim.
    ///
    /// The nonclaim is asserted, not merely documented: the admitted site's
    /// `interrupt_after` is `Clear`, so nothing on this path can be read as
    /// "the guest observed a DP interrupt". Admission records that the
    /// opcode was reached and the DP slot was free -- no more.
    #[test]
    fn full_sync_with_a_capture_boundary_is_admitted_as_a_site_claiming_no_observation() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(set_tile(7, 2, 0));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
        words.extend([word(0x29, 0), 0]); // FULL_SYNC
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let accesses_before = journal.accesses().len();

        let plan = push_and_visit(&decoded, capture, journal);

        assert_eq!(
            plan.full_sync_sites.len(),
            1,
            "the site is pushed, not dropped"
        );
        let site = &plan.full_sync_sites[0];
        assert_eq!(site.ordinal, 0);
        assert!(site.dp_slot_reserved);
        // THE NONCLAIM. A reservation is not an observation.
        assert_eq!(
            site.boundary.interrupt_after(),
            fn64_render_ir::DpInterruptState::Clear,
            "admitting a FullSync site must not report an observed DP interrupt"
        );
        // A sync journals no resource region, so admitting it added no
        // access -- the plan's access list still matches the journal exactly
        // (which `finish` inside `push_and_visit` already proved by not
        // erroring).
        assert_eq!(plan.accesses.len(), accesses_before);
        // The surrounding TMEM stream is unaffected.
        assert_eq!(plan.loads.len(), 1);
        assert_eq!(plan.states.len(), 3);
    }

    /// A FullSync whose capture carries NO boundary record is still rejected
    /// loudly, never silently omitted and never admitted as a site the
    /// producer did not reserve. This is the narrowed rejection that
    /// replaced the old blanket one.
    ///
    /// The fixture reaches this state by building the capture through
    /// `OwnedRawDpcCapture::new` (the no-FullSync constructor) while the
    /// ticket is finalized with the boundary the IR requires -- i.e. a
    /// producer that satisfied stream derivation but never took the reserve
    /// half.
    #[test]
    fn full_sync_without_a_capture_boundary_is_rejected_loudly_not_admitted() {
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
        // Rebuild the capture WITHOUT its boundary list, modelling a producer
        // that never reserved the DP slot.
        let unreserved = OwnedRawDpcCapture::new(
            capture.submission().clone(),
            layout,
            capture.transaction_sequence(),
            capture.cmd_end(),
        );
        assert!(unreserved.full_sync_boundaries().is_empty());

        let (session, authority) = new_raw_dpc_roles().unwrap();
        let request = session.plan_request(unreserved);
        let mut writer = authority.begin_plan(request);
        let outcome = push_decoded_raw_dpc(
            &mut writer,
            &decoded,
            &capture_words,
            layout,
            submission_start,
        );
        let Err(PushDecodedRawDpcError::Unadmitted(rejection)) = outcome else {
            panic!(
                "a FullSync with no capture boundary must be rejected via \
                 UnadmittedRawDpcCommand, not admitted or rejected for a different reason: \
                 {outcome:?}"
            );
        };
        assert_eq!(rejection.opcode_name, "FullSync");
        assert_eq!(rejection.command_index, 4);
        let _ = (session, journal);
    }

    /// All 32 `NoOp` wire encodings (four `0x00`/`0x40`/`0x80`/`0xc0`
    /// high-bit prefixes x eight `0x00..=0x07` variants -- the same
    /// coverage `raw_dpc::mod::tests::
    /// all_low_noop_variants_and_four_prefixes_are_admitted` exercises at
    /// the decode layer) are admitted and discarded here, not merely
    /// decoded: the finished plan is empty.
    #[test]
    fn no_op_is_admitted_and_produces_an_empty_plan() {
        let mut words = Vec::new();
        for prefix in [0x00, 0x40, 0x80, 0xc0] {
            for variant in 0..=7u8 {
                words.extend([word(prefix | variant, 0x005a_5a5a), 0xa5a5_a5a5]);
            }
        }
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x200, 0x208));
        assert_eq!(decoded.commands().len(), 32);

        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 0);
        assert_eq!(plan.loads.len(), 0);
        assert_eq!(plan.triangles.len(), 0);
    }

    /// `NoOp`s interleaved before, between, and after real admitted
    /// commands are dropped, not double-counted or miscounted: the
    /// finished plan's semantic command sequence matches the identical
    /// stream with every `NoOp` physically deleted.
    #[test]
    fn no_op_interleaved_with_admitted_commands_is_invisible_downstream() {
        let no_op = || [word(0x00, 0), 0];
        let mut words = Vec::new();
        words.extend(no_op());
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(no_op());
        words.extend(set_tile(7, 2, 0));
        words.extend(no_op());
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
        words.extend(no_op());
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        assert_eq!(decoded.commands().len(), 8, "4 admitted + 4 NoOp");

        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 3, "SetTextureImage, SetTile, LoadSync");
        assert_eq!(plan.loads.len(), 1, "LoadBlock");
        assert_eq!(plan.triangles.len(), 0);
    }

    /// `NoOp`s present in a stream that also contains a trailing rejected
    /// command must not weaken or widen rejection: the rejection still
    /// fires, names the actual offending opcode (never `"NoOp"`), and
    /// `command_index` still counts every `NoOp` as an ordinary indexed
    /// command.
    #[test]
    fn no_op_does_not_widen_admission_past_a_trailing_rejected_command() {
        let mut words = Vec::new();
        words.extend([word(0x00, 0), 0]); // NoOp, index 0
        words.extend(set_texture_image(0, 2, 8, 0x200)); // index 1
        words.extend([word(0x01, 0), 0]); // NoOp, index 2
        words.extend(set_tile(7, 2, 0)); // index 3
        words.extend([word(0x02, 0), 0]); // NoOp, index 4
        words.extend([word(0x29, 0), 0]); // FULL_SYNC, index 5
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));

        let layout = capture.memory_layout();
        let submission_start = capture.submission().start();
        let capture_words = capture.submission().command_words();
        // Strip the boundary so the trailing FullSync takes its narrowed
        // rejection -- the rejection whose `command_index` this test is
        // about. With the boundary present the site is admitted instead,
        // which is a different test
        // (`full_sync_with_a_capture_boundary_is_admitted_as_a_site_...`).
        let unreserved = OwnedRawDpcCapture::new(
            capture.submission().clone(),
            layout,
            capture.transaction_sequence(),
            capture.cmd_end(),
        );
        let (session, authority) = new_raw_dpc_roles().unwrap();
        let request = session.plan_request(unreserved);
        let mut writer = authority.begin_plan(request);
        let outcome = push_decoded_raw_dpc(
            &mut writer,
            &decoded,
            &capture_words,
            layout,
            submission_start,
        );
        let Err(PushDecodedRawDpcError::Unadmitted(rejection)) = outcome else {
            panic!(
                "an unreserved FullSync must still be rejected via UnadmittedRawDpcCommand \
                 even with NoOps present in the stream: {outcome:?}"
            );
        };
        assert_eq!(rejection.opcode_name, "FullSync");
        assert_eq!(
            rejection.command_index, 5,
            "every NoOp still counts as an ordinary indexed command"
        );
        let _ = (session, journal);
    }

    /// The same NoOp-interleaved stream, this time WITH its capture boundary:
    /// the trailing FullSync is admitted as a site at the same command index,
    /// proving NoOps do not shift the admitted site's position either.
    #[test]
    fn no_op_interleaving_does_not_shift_an_admitted_full_sync_site() {
        let mut words = Vec::new();
        words.extend([word(0x00, 0), 0]); // NoOp, index 0
        words.extend(set_texture_image(0, 2, 8, 0x200)); // index 1
        words.extend([word(0x01, 0), 0]); // NoOp, index 2
        words.extend(set_tile(7, 2, 0)); // index 3
        words.extend([word(0x02, 0), 0]); // NoOp, index 4
        words.extend([word(0x29, 0), 0]); // FULL_SYNC, index 5
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));

        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.full_sync_sites.len(), 1);
        assert_eq!(
            plan.full_sync_sites[0].ordinal, 0,
            "first site in the stream"
        );
        assert_eq!(
            plan.full_sync_sites[0].location.command_index, 5,
            "NoOps still count as ordinary indexed commands for an admitted site"
        );
        assert_eq!(
            plan.full_sync_sites[0].boundary.interrupt_after(),
            fn64_render_ir::DpInterruptState::Clear
        );
    }

    #[test]
    fn opcode_name_still_names_no_op_after_the_match_arm_split() {
        for variant in 0..=7u8 {
            assert_eq!(opcode_name(&RawDpcCommandKind::NoOp { variant }), "NoOp");
        }
    }

    const RAW_TRIANGLE_BASE_EDGE: u8 = 0x08;

    /// One base-edge (non-shaded, non-textured, non-Z) triangle command's
    /// eight raw wire words -- the simplest admitted triangle opcode, same
    /// shape `raw_dpc::mod::tests`' own `triangle_base_word0` builds.
    fn triangle_base_edge_words(tile: u32, level: u32, yl: u16) -> [u32; 8] {
        let w0 = word(
            RAW_TRIANGLE_BASE_EDGE,
            (tile & 0x7) << 16 | (level & 0x7) << 19 | u32::from(yl),
        );
        [
            w0,
            0,
            0x0010_0000,
            0,
            0x0020_0000,
            0x0000_8000,
            0x0005_0000,
            0,
        ]
    }

    fn push_and_expect_error(words: Vec<u32>, source_range: (u32, u32)) -> PushDecodedRawDpcError {
        let (decoded, capture, journal) = decode_admitted_capture(words, source_range);
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
        let _ = (session, journal);
        outcome.expect_err("expected push_decoded_raw_dpc to reject this fixture")
    }

    #[test]
    fn raw_triangle_before_any_set_other_mode_is_rejected_loudly_not_defaulted() {
        // No SetOtherMode anywhere in this fixture: the triangle at index 0
        // must be rejected, not silently decoded against a fabricated
        // OtherMode(0, 0).
        let words = triangle_base_edge_words(3, 2, 0x1234).to_vec();
        let error = push_and_expect_error(words, (0x214, 0x224));
        let PushDecodedRawDpcError::TriangleBeforeAnyOtherMode(rejection) = error else {
            panic!("expected TriangleBeforeAnyOtherMode, got {error:?}");
        };
        assert_eq!(rejection.command_index, 0);
    }

    #[test]
    fn raw_triangle_is_admitted_after_a_preceding_set_other_mode() {
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0));
        words.extend(triangle_base_edge_words(3, 2, 0x1234));
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let RawDpcCommandKind::RawTriangle(source_triangle) = decoded.commands()[1].kind() else {
            panic!("expected RawTriangle as the second command");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 1, "one SetOtherMode");
        assert_eq!(plan.triangles.len(), 1, "one admitted triangle");

        let expected_decoded = decode_triangle_vertices(&source_triangle, false);
        let expected_vertices: [NeutralTriangleVertex; 3] =
            core::array::from_fn(|index| neutral_triangle_vertex(expected_decoded.vertex(index)));
        assert_eq!(plan.triangles[0].vertices, expected_vertices);
        assert!(
            plan.accesses.is_empty() || plan.accesses.len() == 1,
            "no TMEM access from a triangle push"
        );
    }

    #[test]
    fn a_set_other_mode_after_a_triangle_does_not_retroactively_change_its_already_decoded_vertices(
    ) {
        // Interleaved order: SetOtherMode(perspective off) -> triangle A ->
        // SetOtherMode(perspective on) -> triangle B. Triangle A must decode
        // with perspective off (the value current at ITS stream position),
        // never with the later SetOtherMode's value.
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0)); // perspective off (bit 19 low)
        words.extend(triangle_base_edge_words(0, 0, 0x0100));
        words.extend(set_other_mode(0, 1 << 19)); // perspective on
        words.extend(triangle_base_edge_words(1, 0, 0x0200));

        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let RawDpcCommandKind::RawTriangle(triangle_a) = decoded.commands()[1].kind() else {
            panic!("expected RawTriangle at index 1");
        };
        let RawDpcCommandKind::RawTriangle(triangle_b) = decoded.commands()[3].kind() else {
            panic!("expected RawTriangle at index 3");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.triangles.len(), 2);

        let expected_a = decode_triangle_vertices(&triangle_a, false);
        let expected_a_vertices: [NeutralTriangleVertex; 3] =
            core::array::from_fn(|index| neutral_triangle_vertex(expected_a.vertex(index)));
        assert_eq!(
            plan.triangles[0].vertices, expected_a_vertices,
            "triangle A must decode with perspective OFF (its own stream-position OtherMode)"
        );

        let expected_b = decode_triangle_vertices(&triangle_b, true);
        let expected_b_vertices: [NeutralTriangleVertex; 3] =
            core::array::from_fn(|index| neutral_triangle_vertex(expected_b.vertex(index)));
        assert_eq!(
            plan.triangles[1].vertices, expected_b_vertices,
            "triangle B must decode with perspective ON"
        );
        assert_ne!(
            expected_a_vertices, expected_b_vertices,
            "the two OtherMode values must actually produce different decoded vertices, or this \
             test cannot distinguish correct from incorrect ordering"
        );
    }

    /// Real two-triangle A/B wire stream, decoded, pushed, sealed, and
    /// visited exclusively through `RawDpcCoordinator::execution_view` (the
    /// same coordinator-owned route `WgpuBackend`/`production.rs` uses in
    /// production, never bare-authority `execution_view`) with
    /// `TriangleDrawStateCollector` as the plan visitor -- closes the gap
    /// `push_and_visit`'s bare-authority route and
    /// `triangle_draw_data.rs`'s hand-fed-collector test each leave open on
    /// their own: neither combines "real wire words" + "sealed plan" +
    /// "coordinator-routed visit" + "two triangles with an intervening
    /// state change" in one proof.
    #[test]
    fn two_triangles_with_an_intervening_state_change_retrieve_ordered_command_time_snapshots_through_the_coordinator(
    ) {
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0)); // OtherMode A
        words.extend(set_combine(0x0000_0004, 0x0000_0000)); // Combine A
        words.extend(triangle_base_edge_words(0, 0, 0x0100)); // Triangle A
        words.extend(set_other_mode(1, 1 << 19)); // OtherMode B (2Cycle, perspective on)
        words.extend(set_combine(0x0000_0005, 0x0080_0000)); // Combine B
        words.extend(triangle_base_edge_words(1, 0, 0x0200)); // Triangle B

        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let RawDpcCommandKind::SetOtherMode(other_mode_a) = decoded.commands()[0].kind() else {
            panic!("expected SetOtherMode as the first command");
        };
        let RawDpcCommandKind::SetCombine(combine_a) = decoded.commands()[1].kind() else {
            panic!("expected SetCombine as the second command");
        };
        let RawDpcCommandKind::RawTriangle(triangle_a) = decoded.commands()[2].kind() else {
            panic!("expected RawTriangle as the third command");
        };
        let RawDpcCommandKind::SetOtherMode(other_mode_b) = decoded.commands()[3].kind() else {
            panic!("expected SetOtherMode as the fourth command");
        };
        let RawDpcCommandKind::SetCombine(combine_b) = decoded.commands()[4].kind() else {
            panic!("expected SetCombine as the fifth command");
        };
        let RawDpcCommandKind::RawTriangle(triangle_b) = decoded.commands()[5].kind() else {
            panic!("expected RawTriangle as the sixth command");
        };

        let layout = capture.memory_layout();
        let submission_start = capture.submission().start();
        let capture_words = capture.submission().command_words();
        let (mut session, authority) = new_raw_dpc_roles().unwrap();
        // Coordinator-owned route, not a bare-authority call -- unit `()`
        // stands in for the coordinator's physical-state slot `P`: this
        // test only needs the plan-writing/plan-visiting surface, not real
        // TMEM state, matching `targets/triangle_pipeline/tests.rs`'s own
        // coordinator-route precedent.
        let coordinator = authority.into_coordinator(());
        let request = session.plan_request(capture);
        let mut writer = coordinator.begin_plan(request);
        push_decoded_raw_dpc(
            &mut writer,
            &decoded,
            &capture_words,
            layout,
            submission_start,
        )
        .expect("fixture stays inside the admitted state+triangle subset");
        let planned = writer
            .finish(journal)
            .expect("pushed accesses match the journal exactly");
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

        // `execution_view` is the sealed API's sole route to plan contents
        // once bound; never `decoded.commands()` from here on.
        struct NoopExecutionView;
        impl fn64_render::RawDpcExecutionView<crate::raw_dpc::TriangleDrawStateCollector>
            for NoopExecutionView
        {
            fn plan_visited(
                &mut self,
                _plan_visitor: &mut crate::raw_dpc::TriangleDrawStateCollector,
            ) {
            }
            fn captured_reads(&mut self, _reads: &[fn64_render_ir::CapturedGuestRead]) {}
            fn submitted_packet(&mut self, _packet: &fn64_render_ir::WorkloadPacket) {}
        }
        let mut collector = crate::raw_dpc::TriangleDrawStateCollector::default();
        let mut view = NoopExecutionView;
        coordinator.execution_view(&bound, &mut collector, &mut view);
        let retrieved = collector
            .finish()
            .expect("plan has two triangles with real state at each one's own stream position");
        assert_eq!(retrieved.len(), 2);

        assert_eq!(
            retrieved[0].other_mode, other_mode_a,
            "triangle A must retrieve the OtherMode current at its own stream position"
        );
        assert_eq!(
            retrieved[0].combine_params, combine_a,
            "triangle A must retrieve the CombineParams current at its own stream position"
        );
        assert_eq!(
            retrieved[1].other_mode, other_mode_b,
            "triangle B must retrieve the OtherMode current at its own stream position"
        );
        assert_eq!(
            retrieved[1].combine_params, combine_b,
            "triangle B must retrieve the CombineParams current at its own stream position"
        );
        assert_ne!(
            retrieved[0].other_mode, retrieved[1].other_mode,
            "the two OtherMode values must actually differ, or this test cannot distinguish \
             correct per-triangle snapshots from a collapsed-to-one-value bug"
        );
        assert_ne!(
            retrieved[0].combine_params, retrieved[1].combine_params,
            "the two CombineParams values must actually differ, or this test cannot distinguish \
             correct per-triangle snapshots from a collapsed-to-one-value bug"
        );

        let expected_a = decode_triangle_vertices(&triangle_a, false);
        let expected_a_vertices: [NeutralTriangleVertex; 3] =
            core::array::from_fn(|index| neutral_triangle_vertex(expected_a.vertex(index)));
        let expected_b = decode_triangle_vertices(&triangle_b, true);
        let expected_b_vertices: [NeutralTriangleVertex; 3] =
            core::array::from_fn(|index| neutral_triangle_vertex(expected_b.vertex(index)));
        assert_eq!(
            retrieved[0].vertices, expected_a_vertices,
            "triangle A's retrieved vertices must match its own stream-position decode"
        );
        assert_eq!(
            retrieved[1].vertices, expected_b_vertices,
            "triangle B's retrieved vertices must match its own stream-position decode"
        );
        assert_ne!(
            retrieved[0].vertices, retrieved[1].vertices,
            "the two triangles' vertices must actually differ, confirming ordering, not just \
             independent correctness"
        );
    }

    /// Builds one full-width triangle command's wire words for `opcode`
    /// (`0x08..=0x0f`), filling every present optional coefficient block
    /// (shade/texture/depth, per the opcode's own low three bits) with a
    /// deterministic, distinct, nonzero pattern -- `raw_rdp_command_width`
    /// determines the total byte count (32..=176), same source
    /// `width_keyed_command_raw_words` itself keys off.
    fn full_width_triangle_words(opcode: u8) -> Vec<u32> {
        let width_bytes = fn64_render::raw_rdp_command_width(opcode)
            .expect("every triangle opcode 0x08..=0x0f has a known width");
        let width_words = (width_bytes / 4) as usize;
        // Base edge block: reuse the same non-degenerate geometry as
        // `triangle_base_edge_words` (tile/level/yl in w0, zero edges) --
        // this test only cares about wire-word/location provenance, not
        // decoded vertex values.
        let w0 = word(opcode, (3u32 & 0x7) << 16 | (2u32 & 0x7) << 19 | 0x1234u32);
        let mut words = vec![
            w0,
            0,
            0x0010_0000,
            0,
            0x0020_0000,
            0x0000_8000,
            0x0005_0000,
            0,
        ];
        // Fill every remaining word (the optional coefficient blocks) with a
        // distinct, deterministic, nonzero pattern so a truncated or
        // misaligned slice would show up as a mismatch, not an accidental
        // zero-vs-zero pass.
        let mut fill = 0x1000_0000u32;
        while words.len() < width_words {
            words.push(fill);
            fill = fill.wrapping_add(0x0101_0101);
        }
        assert_eq!(words.len(), width_words);
        words
    }

    /// Cross-submission admission test: a durable `OtherMode` established
    /// by a prior submission (carried in as `RdpState`, matching how
    /// `WgpuBackend`'s own `rdp_state`/`rdp_state.apply` threads durable
    /// state across submissions in production) admits a triangle that has
    /// no local `SetOtherMode` of its own -- proving `current_other_mode`
    /// really does seed from `decoded.base_state`, not just from in-plan
    /// commands.
    #[test]
    fn raw_triangle_is_admitted_using_durable_other_mode_carried_from_a_prior_submission() {
        let mut durable_delta = crate::state::RdpStateDelta::default();
        durable_delta.set_other_mode(crate::state::OtherMode::from_wire(0, 0));
        let mut durable_state = RdpState::default();
        durable_state.apply(&durable_delta);
        assert_eq!(
            durable_state.other_mode(),
            Some(crate::state::OtherMode::from_wire(0, 0)),
            "durable state fixture must actually carry a real OtherMode, or this test proves \
             nothing"
        );

        // No SetOtherMode anywhere in THIS submission's own words -- only
        // the carried-in durable state establishes it.
        let words = triangle_base_edge_words(3, 2, 0x1234).to_vec();
        let (decoded, capture, journal) =
            decode_admitted_capture_with_state(words, (0x214, 0x224), durable_state);
        let RawDpcCommandKind::RawTriangle(source_triangle) = decoded.commands()[0].kind() else {
            panic!("expected RawTriangle as the only command");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(
            plan.triangles.len(),
            1,
            "triangle must be admitted using the carried-in durable OtherMode, not rejected"
        );

        let expected_decoded = decode_triangle_vertices(&source_triangle, false);
        let expected_vertices: [NeutralTriangleVertex; 3] =
            core::array::from_fn(|index| neutral_triangle_vertex(expected_decoded.vertex(index)));
        assert_eq!(
            plan.triangles[0].vertices, expected_vertices,
            "triangle must decode using the carried-in durable OtherMode's texture_perspective()"
        );
    }

    /// Wire-provenance sweep (independent-review finding): for every one of
    /// the eight triangle opcodes (`0x08..=0x0f`, spanning the full 32..176
    /// byte width range `raw_rdp_command_width`/`triangle_word_count`
    /// admit), the admitted `RdpTriangleCommand`'s `raw_words`,
    /// `location.source_byte_len`, `location.source_address`, and
    /// `location.wire_opcode` must exactly match the source wire bytes --
    /// proving `width_keyed_command_raw_words`'s variable-width slicer
    /// handles the full opcode range correctly, not just the simplest
    /// (0x08, 32-byte) case the other triangle tests exercise.
    #[test]
    fn triangle_raw_words_and_location_match_the_source_wire_bytes_across_every_opcode_width() {
        for opcode in 0x08u8..=0x0f {
            let mut words = Vec::new();
            words.extend(set_other_mode(0, 0));
            let triangle_words = full_width_triangle_words(opcode);
            let triangle_start_word = words.len();
            words.extend(triangle_words.clone());

            let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
            let expected_source_address = capture
                .memory_layout()
                .address(
                    capture.submission().start() + u32::try_from(triangle_start_word * 4).unwrap(),
                )
                .unwrap();
            let plan = push_and_visit(&decoded, capture, journal);
            assert_eq!(plan.triangles.len(), 1, "opcode {opcode:#04x}");

            let triangle = &plan.triangles[0];
            assert_eq!(
                triangle.raw_words.as_ref(),
                triangle_words.as_slice(),
                "opcode {opcode:#04x}: raw_words must exactly match the source wire words"
            );
            assert_eq!(
                triangle.location.source_byte_len,
                u32::try_from(triangle_words.len() * 4).unwrap(),
                "opcode {opcode:#04x}: source_byte_len must match this opcode's real wire width"
            );
            assert_eq!(
                triangle.location.source_address, expected_source_address,
                "opcode {opcode:#04x}: source_address must point at the triangle's own wire bytes, \
                 not the preceding SetOtherMode"
            );
            assert_eq!(
                triangle.location.wire_opcode, opcode,
                "opcode {opcode:#04x}: wire_opcode must be preserved exactly"
            );
        }
    }

    // --- TextureRectangle/TextureRectangleFlip admission ---

    const TEXRECT: u8 = 0x24;
    const TEXRECT_FLIP: u8 = 0x25;

    /// One `TextureRectangle`/`TextureRectangleFlip` command's 4-word wire
    /// payload, following `texture_rectangle.rs`'s own bit layout: word 0 =
    /// `(lrx << 12) | lry` (with the opcode in the top byte via `word()`),
    /// word 1 = `(tile << 24) | (ulx << 12) | uly`, word 2 = `(uls << 16) |
    /// ult`, word 3 = `(dsdx << 16) | dtdy`.
    ///
    /// The frozen card fixture: `ulx=0x0040` (16.0px), `uly=0`,
    /// `lrx=0x0100` (64.0px), `lry=0x00C0` (48.0px) -- a 48x48 rectangle,
    /// upper-left `(16, 0)`, lower-right `(64, 48)`; `uls=0`, `ult=0`,
    /// `dsdx=0x0100` (`1.0` in `s10.5`, one texel per pixel), `dtdy`
    /// identical.
    fn texrect_words(opcode: u8, tile: u32) -> [u32; 4] {
        let ulx: u32 = 0x0040;
        let uly: u32 = 0;
        let lrx: u32 = 0x0100;
        let lry: u32 = 0x00c0;
        let uls: u32 = 0;
        let ult: u32 = 0;
        let dsdx: u32 = 0x0100;
        let dtdy: u32 = 0x0100;
        [
            word(opcode, (lrx << 12) | lry),
            (tile & 0x7) << 24 | (ulx << 12) | uly,
            (uls << 16) | ult,
            (dsdx << 16) | dtdy,
        ]
    }

    /// A wire fixture that decodes as `is_null()` (`ulx > lrx`), matching
    /// RT64's own `FixedRect::isEmpty()` early return -- `ulx=0x0100`
    /// (64.0px) with `lrx=0x0040` (16.0px), reversed.
    fn reversed_texrect_words(opcode: u8) -> [u32; 4] {
        let ulx: u32 = 0x0100;
        let uly: u32 = 0;
        let lrx: u32 = 0x0040;
        let lry: u32 = 0x00c0;
        [
            word(opcode, (lrx << 12) | lry),
            (ulx << 12) | uly,
            0,
            (0x0100u32 << 16) | 0x0100,
        ]
    }

    #[test]
    fn texture_rectangle_before_any_set_other_mode_is_rejected_loudly_not_defaulted() {
        let words = texrect_words(TEXRECT, 0).to_vec();
        let error = push_and_expect_error(words, (0x214, 0x224));
        let PushDecodedRawDpcError::TextureRectangleBeforeAnyOtherMode(rejection) = error else {
            panic!("expected TextureRectangleBeforeAnyOtherMode, got {error:?}");
        };
        assert_eq!(rejection.command_index, 0);
    }

    #[test]
    fn texture_rectangle_is_admitted_after_a_preceding_set_other_mode() {
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0)); // OneCycle
        words.extend(texrect_words(TEXRECT, 0));
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let RawDpcCommandKind::TextureRectangle(source_rectangle) = decoded.commands()[1].kind()
        else {
            panic!("expected TextureRectangle as the second command");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.states.len(), 1, "one SetOtherMode");
        assert_eq!(
            plan.triangles.len(),
            2,
            "one texture rectangle admits as exactly two RdpTriangleCommand pushes (§3b Option A)"
        );

        let expected_vertices = texture_rectangle_vertices(source_rectangle, CycleType::OneCycle)
            .expect("this fixture's rectangle is non-degenerate");
        let expected_first: [NeutralTriangleVertex; 3] = core::array::from_fn(|index| {
            neutral_texture_rectangle_vertex(expected_vertices.vertex(index))
        });
        let expected_second: [NeutralTriangleVertex; 3] = core::array::from_fn(|index| {
            neutral_texture_rectangle_vertex(expected_vertices.vertex(index + 3))
        });
        assert_eq!(plan.triangles[0].vertices, expected_first);
        assert_eq!(plan.triangles[1].vertices, expected_second);
    }

    /// Texture-rectangle placement card §2 invariant 2: both triangle
    /// halves of one rectangle carry the identical `RectViewportPixels` --
    /// RT64 computes `viewportRect` once per `DrawCall`, not once per
    /// triangle.
    #[test]
    fn both_triangle_halves_of_one_texture_rectangle_share_the_same_viewport() {
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0));
        words.extend(texrect_words(TEXRECT, 0));
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.triangles.len(), 2);
        assert_eq!(plan.triangles[0].source, TriangleSource::TextureRectangle);
        assert_eq!(plan.triangles[1].source, TriangleSource::TextureRectangle);
        assert!(plan.triangles[0].viewport.is_some());
        assert_eq!(plan.triangles[0].viewport, plan.triangles[1].viewport);
    }

    /// Texture-rectangle placement card §2 invariant 1: a `RawTriangle`
    /// admits with `source == RawTriangle` and `viewport == None`.
    #[test]
    fn a_raw_triangle_has_no_viewport_override() {
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0));
        words.extend(triangle_base_edge_words(3, 2, 0x1234));
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.triangles.len(), 1);
        assert_eq!(plan.triangles[0].source, TriangleSource::RawTriangle);
        assert_eq!(plan.triangles[0].viewport, None);
    }

    #[test]
    fn texture_rectangle_flip_is_admitted_with_flip_swapped_texcoords() {
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0));
        words.extend(texrect_words(TEXRECT_FLIP, 0));
        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let RawDpcCommandKind::TextureRectangle(source_rectangle) = decoded.commands()[1].kind()
        else {
            panic!("expected TextureRectangle as the second command");
        };
        assert!(source_rectangle.flip(), "0x25 must decode flip=true");
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.triangles.len(), 2);

        let expected_vertices = texture_rectangle_vertices(source_rectangle, CycleType::OneCycle)
            .expect("this fixture's rectangle is non-degenerate");
        let expected_first: [NeutralTriangleVertex; 3] = core::array::from_fn(|index| {
            neutral_texture_rectangle_vertex(expected_vertices.vertex(index))
        });
        let expected_second: [NeutralTriangleVertex; 3] = core::array::from_fn(|index| {
            neutral_texture_rectangle_vertex(expected_vertices.vertex(index + 3))
        });
        assert_eq!(plan.triangles[0].vertices, expected_first);
        assert_eq!(plan.triangles[1].vertices, expected_second);

        // Independent geometry check: flip must actually change the
        // texcoord pairing versus the non-flip fixture, or this test cannot
        // distinguish correct flip handling from a no-op.
        let nonflip_words = texrect_words(TEXRECT, 0);
        let nonflip_rectangle =
            crate::RawTextureRectangle::decode(TEXRECT, &texrect_command_bytes(nonflip_words))
                .unwrap();
        let nonflip_vertices = texture_rectangle_vertices(nonflip_rectangle, CycleType::OneCycle)
            .expect("non-degenerate");
        assert_ne!(
            expected_vertices.vertex(1).texcoord(),
            nonflip_vertices.vertex(1).texcoord(),
            "flip must change vertex 1's texcoord pairing versus the non-flip fixture"
        );
    }

    /// Converts a 4-word fixture (as built by [`texrect_words`]) into the
    /// big-endian byte slice `RawTextureRectangle::decode` expects, mirroring
    /// how `decode_stream` slices `stream.bytes` before calling it.
    fn texrect_command_bytes(words: [u32; 4]) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        for (index, word) in words.iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        bytes
    }

    #[test]
    fn degenerate_texture_rectangle_is_rejected_loudly_not_a_vacuous_zero_area_draw() {
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0));
        words.extend(reversed_texrect_words(TEXRECT));
        let error = push_and_expect_error(words, (0x214, 0x224));
        let PushDecodedRawDpcError::DegenerateTextureRectangle(rejection) = error else {
            panic!("expected DegenerateTextureRectangle, got {error:?}");
        };
        assert_eq!(
            rejection.command_index, 1,
            "the texrect is the second command (index 1)"
        );
    }

    #[test]
    fn a_set_other_mode_after_a_texture_rectangle_does_not_retroactively_change_its_geometry() {
        // Interleaved order: SetOtherMode(OneCycle) -> texrect A ->
        // SetOtherMode(Copy) -> texrect B. Texrect A must decode using
        // OneCycle (its own stream-position OtherMode), never the later
        // SetOtherMode's Copy-mode dsdx/lrx/lry mutation.
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0)); // OneCycle
        words.extend(texrect_words(TEXRECT, 0));
        words.extend(set_other_mode(2, 0)); // Copy (bits 20:21 = 2)
        words.extend(texrect_words(TEXRECT, 1));

        let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
        let RawDpcCommandKind::TextureRectangle(rectangle_a) = decoded.commands()[1].kind() else {
            panic!("expected TextureRectangle at index 1");
        };
        let RawDpcCommandKind::TextureRectangle(rectangle_b) = decoded.commands()[3].kind() else {
            panic!("expected TextureRectangle at index 3");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(plan.triangles.len(), 4, "two texrects, two triangles each");

        let expected_a = texture_rectangle_vertices(rectangle_a, CycleType::OneCycle)
            .expect("OneCycle fixture is non-degenerate");
        let expected_a_first: [NeutralTriangleVertex; 3] = core::array::from_fn(|index| {
            neutral_texture_rectangle_vertex(expected_a.vertex(index))
        });
        assert_eq!(
            plan.triangles[0].vertices, expected_a_first,
            "texrect A must decode with OneCycle (its own stream-position OtherMode)"
        );

        let expected_b = texture_rectangle_vertices(rectangle_b, CycleType::Copy)
            .expect("Copy-mode fixture is non-degenerate");
        let expected_b_first: [NeutralTriangleVertex; 3] = core::array::from_fn(|index| {
            neutral_texture_rectangle_vertex(expected_b.vertex(index))
        });
        assert_eq!(
            plan.triangles[2].vertices, expected_b_first,
            "texrect B must decode with Copy (its own stream-position OtherMode)"
        );

        // Vertex index 3 carries `[u2, v2]`, derived from `lrs`/`lrt`, which
        // depend on `uv_width`/`uv_height` -- and those depend on Copy
        // mode's `dsdx >>= 2; lrx |= 3; lry |= 3;` mutation (vertex 0's
        // `[u1, v1]` does not, since it comes straight from `uls`/`ult`
        // with no copy-mode-dependent term), so index 3 is the vertex that
        // actually distinguishes the two modes for this fixture.
        let expected_a_under_copy = texture_rectangle_vertices(rectangle_a, CycleType::Copy)
            .expect("Copy-mode reinterpretation is also non-degenerate for this fixture");
        assert_ne!(
            expected_a.vertex(3).texcoord(),
            expected_a_under_copy.vertex(3).texcoord(),
            "OneCycle vs Copy must actually produce different geometry for rectangle A's wire \
             bytes, or this test cannot distinguish correct from incorrect OtherMode timing"
        );
    }

    #[test]
    fn texture_rectangle_is_admitted_using_durable_other_mode_carried_from_a_prior_submission() {
        let mut durable_delta = crate::state::RdpStateDelta::default();
        durable_delta.set_other_mode(crate::state::OtherMode::from_wire(0, 0));
        let mut durable_state = RdpState::default();
        durable_state.apply(&durable_delta);

        // No SetOtherMode anywhere in THIS submission's own words -- only
        // the carried-in durable state establishes it.
        let words = texrect_words(TEXRECT, 0).to_vec();
        let (decoded, capture, journal) =
            decode_admitted_capture_with_state(words, (0x214, 0x224), durable_state);
        let RawDpcCommandKind::TextureRectangle(source_rectangle) = decoded.commands()[0].kind()
        else {
            panic!("expected TextureRectangle as the only command");
        };
        let plan = push_and_visit(&decoded, capture, journal);
        assert_eq!(
            plan.triangles.len(),
            2,
            "texture rectangle must be admitted using the carried-in durable OtherMode, not \
             rejected"
        );

        let expected_vertices = texture_rectangle_vertices(source_rectangle, CycleType::OneCycle)
            .expect("non-degenerate");
        let expected_first: [NeutralTriangleVertex; 3] = core::array::from_fn(|index| {
            neutral_texture_rectangle_vertex(expected_vertices.vertex(index))
        });
        assert_eq!(
            plan.triangles[0].vertices, expected_first,
            "texture rectangle must decode using the carried-in durable OtherMode's cycle_type()"
        );
    }

    #[test]
    fn texture_rectangle_raw_words_and_location_match_the_source_wire_bytes_for_both_opcodes() {
        for opcode in [TEXRECT, TEXRECT_FLIP] {
            let mut words = Vec::new();
            words.extend(set_other_mode(0, 0));
            let rectangle_words = texrect_words(opcode, 0);
            let rectangle_start_word = words.len();
            words.extend(rectangle_words);

            let (decoded, capture, journal) = decode_admitted_capture(words, (0x214, 0x224));
            let expected_source_address = capture
                .memory_layout()
                .address(
                    capture.submission().start() + u32::try_from(rectangle_start_word * 4).unwrap(),
                )
                .unwrap();
            let plan = push_and_visit(&decoded, capture, journal);
            assert_eq!(plan.triangles.len(), 2, "opcode {opcode:#04x}");

            // Both triangle halves originate from the same wire command, so
            // both must carry identical raw_words/location provenance.
            for (half_index, triangle) in plan.triangles.iter().enumerate() {
                assert_eq!(
                    triangle.raw_words.as_ref(),
                    rectangle_words.as_slice(),
                    "opcode {opcode:#04x} half {half_index}: raw_words must exactly match the \
                     source wire words"
                );
                assert_eq!(
                    triangle.location.source_byte_len,
                    u32::try_from(rectangle_words.len() * 4).unwrap(),
                    "opcode {opcode:#04x} half {half_index}: source_byte_len must be exactly 16"
                );
                assert_eq!(
                    triangle.location.source_address, expected_source_address,
                    "opcode {opcode:#04x} half {half_index}: source_address must point at the \
                     rectangle's own wire bytes, not the preceding SetOtherMode"
                );
                assert_eq!(
                    triangle.location.wire_opcode, opcode,
                    "opcode {opcode:#04x} half {half_index}: wire_opcode must be preserved \
                     exactly"
                );
            }
            assert_eq!(
                plan.triangles[0].location, plan.triangles[1].location,
                "opcode {opcode:#04x}: both halves share one origin command, so location must be \
                 identical, not independently derived"
            );
        }
    }

    /// Mutation-hostile check: flipping a single bit in each of the four
    /// wire words changes at least one decoded vertex -- proving the
    /// admission path does not silently ignore any word (e.g. an accidental
    /// fixed-2-word read that only ever sees the first half).
    #[test]
    fn mutating_any_of_the_four_wire_words_changes_the_admitted_vertices() {
        let baseline = texrect_words(TEXRECT, 0);
        for bit_index in 0..4usize {
            let mut mutated = baseline;
            mutated[bit_index] ^= 1;

            let mut baseline_words = Vec::new();
            baseline_words.extend(set_other_mode(0, 0));
            baseline_words.extend(baseline);
            let (decoded, capture, journal) =
                decode_admitted_capture(baseline_words, (0x214, 0x224));
            let baseline_plan = push_and_visit(&decoded, capture, journal);

            let mut mutated_words = Vec::new();
            mutated_words.extend(set_other_mode(0, 0));
            mutated_words.extend(mutated);
            let (decoded, capture, journal) =
                decode_admitted_capture(mutated_words, (0x214, 0x224));
            let mutated_plan = push_and_visit(&decoded, capture, journal);

            assert_ne!(
                baseline_plan.triangles[0].raw_words, mutated_plan.triangles[0].raw_words,
                "flipping bit 0 of word {bit_index} must change the admitted raw_words"
            );
        }
    }
}
