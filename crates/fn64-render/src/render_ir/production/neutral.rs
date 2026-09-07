use super::*;

// ------------------------------------------------------------------
// Neutral DTO vocabulary
// ------------------------------------------------------------------

/// Neutral mirror of the public `G_IM_FMT` texel-format field (SGI *RDP
/// Command Summary* Table 6). Concrete and `Copy`; carries no
/// `fn64-render-wgpu`-private type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NeutralImageFormat {
    Rgba,
    Yuv,
    ColorIndex,
    IntensityAlpha,
    Intensity,
}

/// Neutral mirror of the public `G_IM_SIZ` texel-size field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NeutralPixelSize {
    Bits4,
    Bits8,
    Bits16,
    Bits32,
}

/// Neutral mirror of one tile's public S/T address-mode bits (mirror,
/// clamp) from `SetTile`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NeutralTileAddressMode {
    pub mirror: bool,
    pub clamp: bool,
}

/// Neutral, complete mirror of one `SetTile` command's staged fields:
/// format/size, TMEM word address, line stride, palette, and both axes'
/// address mode/mask/shift. T3 needs every field here to execute a load
/// against the tile state a real decoder already staged; none of it can
/// be recovered from `ResourceAccess` identity alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralTileDescriptor {
    pub format: NeutralImageFormat,
    pub size: NeutralPixelSize,
    pub line_words: u16,
    pub tmem_word_address: u16,
    pub palette: u8,
    pub s_mode: NeutralTileAddressMode,
    pub mask_s: u8,
    pub shift_s: u8,
    pub t_mode: NeutralTileAddressMode,
    pub mask_t: u8,
    pub shift_t: u8,
}

/// Neutral mirror of one `SetTileSize` command's S/T bounds, in the
/// public 10.2 fixed-point raw field encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralTileSize {
    pub low_s: u16,
    pub low_t: u16,
    pub high_s: u16,
    pub high_t: u16,
}

/// Neutral mirror of `SetTextureImage`'s staged source description.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralTextureImage {
    pub format: NeutralImageFormat,
    pub size: NeutralPixelSize,
    pub width: u16,
    pub address: fn64_render_ir::PhysicalAddress,
}

/// Opaque monotonic TMEM load-sync epoch. Mirrors
/// `fn64-render-wgpu`'s private `TmemLoadEpoch`: staged by `SyncLoad`,
/// bound to every load that follows it, so T3 can reject a load whose
/// epoch predates the physical state's own generation without rereading
/// command bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TmemLoadEpoch(core::num::NonZeroU64);

impl TmemLoadEpoch {
    pub const fn new(epoch: core::num::NonZeroU64) -> Self {
        Self(epoch)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Which public opcode produced one [`TmemLoadSemantics`] value, together
/// with that opcode's exact addressing geometry. Distinct geometry per
/// kind (LoadBlock's DXT accumulator vs. LoadTile/LoadTLUT's S/T bounds)
/// cannot be recovered from a shared bare `TileSize`, so each variant
/// carries its own real fields instead of a lossy common shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TmemLoadKind {
    Block {
        source_s: u16,
        source_t: u16,
        high_s: u16,
        dxt: u16,
    },
    Tile {
        bounds: NeutralTileSize,
    },
    /// Reserved for M4.3.2; a plan admits this only once that
    /// prerequisite lands (card v10 section 1/8).
    Tlut {
        bounds: NeutralTileSize,
        entries: core::num::NonZeroU16,
    },
}

/// Neutral mirror of `fn64-render-wgpu`'s private `TmemTransferLayout`:
/// which physical addressing rule this load's transfer words follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TmemTransferLayout {
    Linear,
    OddRowBankSwap,
}

/// One physical TMEM destination for a transfer word: either a single
/// linear range, or the split low/high-bank pair the public odd-row
/// exchange rule produces. Concrete, not a downstream type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NeutralTmemTransferPhysicalWord {
    Linear(TmemRange),
    SplitBanks { low: TmemRange, high: TmemRange },
}

/// One complete, already-computed 64-bit TMEM transfer word: exactly the
/// materialized fact set a physical executor (T3) needs per word, so it
/// never rereads raw command bytes or recomputes tile geometry from a
/// bare resource access. Mirrors `fn64-render-wgpu`'s private
/// `TmemTransferWord` field-for-field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralTmemTransferWord {
    pub index: u16,
    pub logical_source_offset: u32,
    pub source_access_index: u32,
    pub source_access_byte_offset: u32,
    pub defined_source_byte_mask: u8,
    pub defined_destination_byte_mask: u8,
    pub destination_word: u16,
    pub row_advance: u16,
    pub odd_row_exchange: bool,
    pub physical: NeutralTmemTransferPhysicalWord,
}

/// Exact location of the raw command word(s) one semantic command was
/// decoded from: its ordinal position within the plan
/// (`command_index`), which stream/chunk it came from, the exact
/// physical/DMEM source address the command bytes were read from
/// (`source_address` -- distinct from `source_byte_offset`, which is
/// relative to the owning chunk, not the address space), the source's
/// byte offset/length within that chunk, and the wire opcode byte. T3
/// needs every field here to bind a physical effect back to the exact
/// command that caused it, in its exact decode-order position, without
/// rereading the owning `ExactValidatedRawDpcPlan`'s source bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RawDpcCommandLocation {
    pub command_index: u32,
    pub stream_index: u32,
    pub chunk_index: u32,
    pub source_address: fn64_render_ir::PhysicalAddress,
    pub source_byte_offset: u32,
    pub source_byte_len: u32,
    pub wire_opcode: u8,
}

/// Complete neutral semantics for one TMEM load command (`LoadBlock`,
/// `LoadTile`, or `LoadTLUT`): staged tile/source-image state, the exact
/// opcode-specific addressing geometry, the load-sync epoch it was bound
/// under, the command's own raw wire words, exact source-byte
/// accounting (the complete ordered source-access run, which a
/// partial-width `LoadTile` splits one-per-row), an explicit index into
/// the owning plan's access list for the destination access, transfer
/// layout, and the full ordered
/// transfer-word set a real decoder (T1) already computed. This is the
/// complete neutral semantic/load representation the execution seam
/// needs -- every field T3's physical executor reads is here,
/// materialized, so crossing into `fn64-render`'s neutral plan cannot
/// force a redecode or a weakened generation/epoch/state check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmemLoadSemantics {
    location: RawDpcCommandLocation,
    raw_words: Box<[u32]>,
    epoch: TmemLoadEpoch,
    kind: TmemLoadKind,
    tile_index: u8,
    source_image: NeutralTextureImage,
    tile_descriptor: NeutralTileDescriptor,
    sources: Box<[ResourceAccess]>,
    source_access_index: u32,
    destination: ResourceAccess,
    destination_access_index: u32,
    logical_source_bytes: u32,
    undefined_padding_bytes: u32,
    words_per_row: u16,
    row_count: u16,
    layout: TmemTransferLayout,
    transfer_words: Box<[NeutralTmemTransferWord]>,
}

impl TmemLoadSemantics {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        location: RawDpcCommandLocation,
        raw_words: Vec<u32>,
        epoch: TmemLoadEpoch,
        kind: TmemLoadKind,
        tile_index: u8,
        source_image: NeutralTextureImage,
        tile_descriptor: NeutralTileDescriptor,
        sources: Vec<ResourceAccess>,
        source_access_index: u32,
        destination: ResourceAccess,
        destination_access_index: u32,
        logical_source_bytes: u32,
        undefined_padding_bytes: u32,
        words_per_row: u16,
        row_count: u16,
        layout: TmemTransferLayout,
        transfer_words: Vec<NeutralTmemTransferWord>,
    ) -> Self {
        assert!(
            !sources.is_empty(),
            "a TMEM load always reads at least one source access"
        );
        Self {
            location,
            raw_words: raw_words.into_boxed_slice(),
            epoch,
            kind,
            tile_index,
            source_image,
            tile_descriptor,
            sources: sources.into_boxed_slice(),
            source_access_index,
            destination,
            destination_access_index,
            logical_source_bytes,
            undefined_padding_bytes,
            words_per_row,
            row_count,
            layout,
            transfer_words: transfer_words.into_boxed_slice(),
        }
    }

    pub const fn location(&self) -> RawDpcCommandLocation {
        self.location
    }

    pub fn raw_words(&self) -> &[u32] {
        &self.raw_words
    }

    pub const fn epoch(&self) -> TmemLoadEpoch {
        self.epoch
    }

    pub const fn kind(&self) -> TmemLoadKind {
        self.kind
    }

    pub const fn shape(&self) -> TmemLoadShape {
        match self.kind {
            TmemLoadKind::Block { .. } => TmemLoadShape::Block,
            TmemLoadKind::Tile { .. } => TmemLoadShape::Tile,
            TmemLoadKind::Tlut { .. } => TmemLoadShape::Tlut,
        }
    }

    pub const fn tile_index(&self) -> u8 {
        self.tile_index
    }

    pub const fn source_image(&self) -> NeutralTextureImage {
        self.source_image
    }

    pub const fn tile_descriptor(&self) -> NeutralTileDescriptor {
        self.tile_descriptor
    }

    /// Every [`ResourceAccess`] this load reads from, in the exact
    /// journal order the decoder produced them, occupying the owning
    /// plan's access list contiguously starting at
    /// [`Self::source_access_index`].
    ///
    /// This is a slice, not a single access, precisely because a
    /// partial-width `LoadTile` declares **one source read per row**
    /// (`tmem::wire::decode_load_tile`'s `(low_t..=high_t)` arm): a
    /// 49-row sub-rectangle of a wider texture is 49 disjoint RDRAM
    /// reads, not one contiguous span. Only a load whose source columns
    /// cover the full texture-image width collapses to a single range.
    /// There is no "collapse to one access" path and there must never
    /// be one -- a collapsed range would claim the untouched
    /// inter-row bytes as read, and `transfer_words[].source_access_index`
    /// already binds each transfer word to the exact row it came from.
    pub fn sources(&self) -> &[ResourceAccess] {
        &self.sources
    }

    /// The load's **first** source access -- the one at
    /// [`Self::source_access_index`]. Callers that must account for
    /// every byte the load reads want [`Self::sources`]; this names
    /// only the first fragment, exactly as [`Self::destination`] names
    /// only the first destination fragment.
    pub fn source(&self) -> ResourceAccess {
        self.sources[0]
    }

    /// Index of [`Self::sources`]`[0]` within the owning plan's exact
    /// ordered access list -- lets T3 correlate this load's source run
    /// without re-deriving which journal entries it came from. The run
    /// is contiguous, so fragment `i` sits at
    /// `source_access_index() + i`.
    pub const fn source_access_index(&self) -> u32 {
        self.source_access_index
    }

    pub const fn destination(&self) -> ResourceAccess {
        self.destination
    }

    /// Index of [`Self::destination`] within the owning plan's exact
    /// ordered access list -- the explicit destination access index T3
    /// needs to bind a physical write back to the plan without
    /// re-deriving it.
    pub const fn destination_access_index(&self) -> u32 {
        self.destination_access_index
    }

    pub const fn logical_source_bytes(&self) -> u32 {
        self.logical_source_bytes
    }

    pub const fn undefined_padding_bytes(&self) -> u32 {
        self.undefined_padding_bytes
    }

    pub const fn words_per_row(&self) -> u16 {
        self.words_per_row
    }

    pub const fn row_count(&self) -> u16 {
        self.row_count
    }

    pub const fn layout(&self) -> TmemTransferLayout {
        self.layout
    }

    pub fn transfer_words(&self) -> &[NeutralTmemTransferWord] {
        &self.transfer_words
    }
}

/// Which public opcode produced one [`TmemLoadSemantics`] value. Kept as
/// a cheap discriminant alongside [`TmemLoadKind`] (which carries the
/// exact geometry) so callers that only need the opcode class do not
/// have to match the full geometry enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TmemLoadShape {
    Block,
    Tile,
    Tlut,
}

/// Content identity for one neutral tile/texture-image/epoch/RDP-state
/// value, so a state transition can name what it superseded and what it
/// established without T3 having to reread or re-derive either snapshot
/// from raw bytes. Distinct hash domains per state kind keep a
/// `SetTile` identity from ever colliding with a `SetTextureImage`
/// identity for coincidentally identical bit patterns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RdpStateIdentity(fn64_render_ir::ContentDigest);

impl RdpStateIdentity {
    pub fn of_tile_descriptor(tile_index: u8, descriptor: NeutralTileDescriptor) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.tmem-state-tile-descriptor.v1\0",
            &[&[tile_index], &descriptor_bytes(descriptor)],
        ))
    }

    pub fn of_tile_size(tile_index: u8, size: NeutralTileSize) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.tmem-state-tile-size.v1\0",
            &[
                &[tile_index],
                &size.low_s.to_be_bytes(),
                &size.low_t.to_be_bytes(),
                &size.high_s.to_be_bytes(),
                &size.high_t.to_be_bytes(),
            ],
        ))
    }

    pub fn of_texture_image(image: NeutralTextureImage) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.tmem-state-texture-image.v1\0",
            &[&texture_image_bytes(image)],
        ))
    }

    pub fn of_other_mode(value: NeutralOtherMode) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.rdp-state-other-mode.v1\0",
            &[&other_mode_bytes(value)],
        ))
    }

    pub fn of_color_image(value: NeutralColorImage) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.rdp-state-color-image.v1\0",
            &[&color_image_bytes(value)],
        ))
    }

    pub fn of_fill_color(value: NeutralFillColor) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.rdp-state-fill-color.v1\0",
            &[&fill_color_bytes(value)],
        ))
    }

    pub fn of_env_color(value: NeutralColor4) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.rdp-state-env-color.v1\0",
            &[&color4_bytes(value)],
        ))
    }

    pub fn of_prim_color(value: NeutralPrimColor) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.rdp-state-prim-color.v1\0",
            &[&prim_color_bytes(value)],
        ))
    }

    pub fn of_blend_color(value: NeutralColor4) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.rdp-state-blend-color.v1\0",
            &[&color4_bytes(value)],
        ))
    }

    pub fn of_fog_color(value: NeutralColor4) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.rdp-state-fog-color.v1\0",
            &[&color4_bytes(value)],
        ))
    }

    pub fn of_prim_depth(value: NeutralPrimDepth) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.rdp-state-prim-depth.v1\0",
            &[&prim_depth_bytes(value)],
        ))
    }

    pub fn of_combine(value: NeutralCombineParams) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.rdp-state-combine.v1\0",
            &[&combine_bytes(value)],
        ))
    }

    /// Identity of one `SetScissor`'s tracked rect. Its own domain tag
    /// (`rdp-state-scissor.v1`) keeps it disjoint from every other
    /// state slot's identity space, exactly as [`Self::of_fog_color`]
    /// and its siblings do.
    pub fn of_scissor(value: NeutralScissor) -> Self {
        Self(fn64_render_ir::ContentDigest::hash(
            b"fn64.render.rdp-state-scissor.v1\0",
            &[&scissor_bytes(value)],
        ))
    }

    pub const fn digest(self) -> fn64_render_ir::ContentDigest {
        self.0
    }
}

fn descriptor_bytes(descriptor: NeutralTileDescriptor) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16);
    bytes.push(descriptor.format as u8);
    bytes.push(descriptor.size as u8);
    bytes.extend_from_slice(&descriptor.line_words.to_be_bytes());
    bytes.extend_from_slice(&descriptor.tmem_word_address.to_be_bytes());
    bytes.push(descriptor.palette);
    bytes.push(descriptor.s_mode.mirror as u8);
    bytes.push(descriptor.s_mode.clamp as u8);
    bytes.push(descriptor.mask_s);
    bytes.push(descriptor.shift_s);
    bytes.push(descriptor.t_mode.mirror as u8);
    bytes.push(descriptor.t_mode.clamp as u8);
    bytes.push(descriptor.mask_t);
    bytes.push(descriptor.shift_t);
    bytes
}

fn texture_image_bytes(image: NeutralTextureImage) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8);
    bytes.push(image.format as u8);
    bytes.push(image.size as u8);
    bytes.extend_from_slice(&image.width.to_be_bytes());
    bytes.extend_from_slice(&image.address.get().to_be_bytes());
    bytes
}

/// Neutral mirror of `SetOtherMode`'s staged pure-state value.
///
/// Kept as the raw `high`/`low` wire pair, matching
/// `crate::state::OtherMode`'s own internal representation, rather than
/// decomposed into its ~20 derived fields: every one of those fields is
/// already a cheap computed accessor on `OtherMode` (`cycle_type`,
/// `texture_lut_mode`, `blender_cycle_1`, etc.), so decomposing here
/// would duplicate that bit-math in a second place for no reader this
/// admission-only card serves. A future consumer needing named fields can
/// call `OtherMode`'s own accessors after reconstructing it from
/// `high`/`low`. Open, non-blocking per this card's Section 2d.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralOtherMode {
    pub high: u32,
    pub low: u32,
}

fn other_mode_bytes(value: NeutralOtherMode) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8);
    bytes.extend_from_slice(&value.high.to_be_bytes());
    bytes.extend_from_slice(&value.low.to_be_bytes());
    bytes
}

/// Neutral mirror of `SetColorImage`'s staged pure-state value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralColorImage {
    pub format: NeutralImageFormat,
    pub size: NeutralPixelSize,
    pub width: u32,
    pub address: fn64_render_ir::PhysicalAddress,
}

fn color_image_bytes(value: NeutralColorImage) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(10);
    bytes.push(value.format as u8);
    bytes.push(value.size as u8);
    bytes.extend_from_slice(&value.width.to_be_bytes());
    bytes.extend_from_slice(&value.address.get().to_be_bytes());
    bytes
}

/// Neutral mirror of `SetFillColor`'s staged raw 32-bit wire value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralFillColor {
    pub value: u32,
}

fn fill_color_bytes(value: NeutralFillColor) -> Vec<u8> {
    value.value.to_be_bytes().to_vec()
}

/// Neutral mirror of one fragment constant-register RGBA color, shared by
/// `SetEnvColor`/`SetBlendColor`/`SetFogColor` -- all three decode via
/// the identical `Color4::from_wire(w1)` (card Section 2d).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralColor4 {
    pub value: u32,
}

fn color4_bytes(value: NeutralColor4) -> Vec<u8> {
    value.value.to_be_bytes().to_vec()
}

/// Neutral mirror of `SetPrimColor`'s staged LOD-fraction/LOD-min/color
/// fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralPrimColor {
    pub lod_frac: u8,
    pub lod_min: u8,
    pub color: u32,
}

fn prim_color_bytes(value: NeutralPrimColor) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(6);
    bytes.push(value.lod_frac);
    bytes.push(value.lod_min);
    bytes.extend_from_slice(&value.color.to_be_bytes());
    bytes
}

/// Neutral mirror of `SetPrimDepth`'s staged masked depth/delta-Z fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralPrimDepth {
    pub z: u16,
    pub dz: u16,
}

fn prim_depth_bytes(value: NeutralPrimDepth) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4);
    bytes.extend_from_slice(&value.z.to_be_bytes());
    bytes.extend_from_slice(&value.dz.to_be_bytes());
    bytes
}

/// Neutral mirror of `SetCombine`'s staged raw low/high wire words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralCombineParams {
    pub low: u32,
    pub high: u32,
}

fn combine_bytes(value: NeutralCombineParams) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8);
    bytes.extend_from_slice(&value.low.to_be_bytes());
    bytes.extend_from_slice(&value.high.to_be_bytes());
    bytes
}

/// Neutral mirror of `SetScissor`'s decoded operands (RDP opcode `0x2d`),
/// field-for-field as RT64's `setScissor` decode reads them: a 2-bit
/// `mode` plus four 12-bit fixed-point coordinates (10 integer bits, 2
/// fractional -- the same `<< 2` scale `FillRectangle`/`TexRect` use), all
/// zero-extended and therefore never negative.
///
/// **Tracked state only.** This carrier exists so a stream containing
/// `SetScissor` is admitted rather than rejected; nothing in the raster
/// path reads it. It is deliberately *not* mirrored into
/// `RdpState`/`RdpStateDelta` the way the nine applied pure-state
/// commands are, because that is precisely the channel a draw would use
/// to consult it. Actually clipping to this rect is separate, later work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeutralScissor {
    pub mode: u8,
    pub upper_left_x: u16,
    pub upper_left_y: u16,
    pub lower_right_x: u16,
    pub lower_right_y: u16,
}

fn scissor_bytes(value: NeutralScissor) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(9);
    bytes.push(value.mode);
    bytes.extend_from_slice(&value.upper_left_x.to_be_bytes());
    bytes.extend_from_slice(&value.upper_left_y.to_be_bytes());
    bytes.extend_from_slice(&value.lower_right_x.to_be_bytes());
    bytes.extend_from_slice(&value.lower_right_y.to_be_bytes());
    bytes
}
