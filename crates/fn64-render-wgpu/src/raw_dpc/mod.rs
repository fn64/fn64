//! Bounded decoding for the first admitted raw-DPC command subset.

mod production_adapter;
#[cfg(test)]
mod s10_5_properties;
mod texture_rectangle;
mod triangle;
#[cfg(test)]
mod triangle_composition;
mod triangle_draw_data;
pub(crate) mod triangle_span;
mod triangle_vertices;

pub(crate) use production_adapter::push_planning_decoded_raw_dpc;
pub use production_adapter::{
    push_decoded_raw_dpc, DegenerateTextureRectangle, PushDecodedRawDpcError,
    TextureRectangleBeforeAnyOtherMode, TriangleBeforeAnyOtherMode, UnadmittedRawDpcCommand,
};
pub use texture_rectangle::{
    texture_rectangle_vertices, RawTextureRectangle, RawTextureRectangleError,
    TextureRectangleVertex, TextureRectangleVertices, TEXTURE_RECTANGLE_COMMAND_BYTES,
};
pub use triangle::{
    triangle_word_count, CoefficientWords, DepthWords, RawTriangle, RawWord, TriangleDecodeError,
    TriangleFlags,
};
pub use triangle_draw_data::{
    bound_tile_index, neutral_vertex_to_raster_vertex, retrieve_triangle_draws,
    MissingTriangleDrawState, RetrievedTriangleDraw, TriangleDrawStateCollector,
};
pub use triangle_vertices::{decode_triangle_vertices, TriangleVertex, TriangleVertices};

use core::fmt;

use fn64_render::raw_rdp_command_width;
use fn64_render_ir::{
    AccessMode, AccessPurpose, DmemRange, FullSyncOccurrence, OperationId, RawCommandStream,
    RawStreamIdentity, RdramResource, ResourceAccess, ResourceJournal, ResourceJournalLimits,
    ResourceRegion, SubmittedTicket, ValidationError, WorkloadAdmission, WorkloadIdentity,
    MAX_RESOURCE_ACCESSES,
};

use crate::combiner::CombineParams;
use crate::state::{
    Color4, ColorImage, CycleType, FillColor, ImageFormat, OtherMode, PixelSize, PrimColor,
    PrimDepth, RdpState, RdpStateDelta, StagedRdpState, ZImage,
};
use crate::tmem::{
    decode_tmem_command, TmemCommand, TmemLoad, TmemLoadContract, TmemLoadEpoch,
    TmemLoadSourceIdentity, TmemLoadSourcePlan, TmemSourcePlanStart, TmemTransferPlan,
    TmemTransferWord, LOAD_BLOCK, LOAD_SYNC, LOAD_TILE, LOAD_TLUT, SET_TEXTURE_IMAGE, SET_TILE,
    SET_TILE_SIZE,
};

const SET_OTHER_MODE: u8 = 0x2f;
/// `G_SETSCISSOR` (`0xED`, as
/// `crates/fn64-render-reference/src/gbi/wire.rs`'s `G_SETSCISSOR` already
/// spells it); public SGI *RDP Command Summary* "Set Scissor". Admitted as
/// tracked state only -- see [`RawDpcCommandKind::SetScissor`].
const SET_SCISSOR: u8 = 0xed & 0x3f;
const SET_COLOR_IMAGE: u8 = 0x3f;
/// The depth-buffer binder. **Two wire opcodes decode to this same masked
/// value**: `G_SETZIMG` (`0xfe`, public SGI *RDP Command Summary* "Set Z
/// Image") and `G_SETMASKIMG` (`0x3e`, "Set Mask Image"). Both mask to
/// `0x3e` under `& 0x3f`, and angrylion binds the z-buffer base address on
/// either (`rdp_set_depth_image` / `rdp_set_mask_image` both do
/// `zb_address = args[1] & 0x00ffffff`), so a single dispatch arm honours
/// both -- exactly what `gen-zbuffer-setmaskimage-binds-z-image` probes.
const SET_Z_IMAGE: u8 = 0xfe & 0x3f;
const SET_FILL_COLOR: u8 = 0x37;
const FILL_RECTANGLE: u8 = 0x36;
const FULL_SYNC: u8 = 0x29;
/// `G_RDPPIPESYNC` (`0xe7`, `crates/fn64-render-reference/src/gbi/wire.rs`'s
/// `G_RDPPIPESYNC`); public SGI *RDP Command Summary* "Sync Pipe". WM2000's
/// most-issued rejected command: 20,800 occurrences, 14.6% of its whole
/// stream (`docs/rt64/RT64-WM2000-CENSUS.md` §3).
const SYNC_PIPE: u8 = 0xe7 & 0x3f;
/// `G_RDPTILESYNC` (`0xe8`); public SGI *RDP Command Summary* "Sync Tile".
/// 1,808 occurrences in the same census.
const SYNC_TILE: u8 = 0xe8 & 0x3f;
/// The id WM2000 writes to end a submission (`0xdf`, the GBI's `G_ENDDL`,
/// masked to its command bits). Not an assigned RDP command: it is carved
/// out of the otherwise-rejected `0x10..=0x23` block by
/// `fn64_render::raw_rdp_command_width`'s `RDP_STREAM_TERMINATOR_NOOP`,
/// whose doc carries the measurement and the narrow-widening argument.
///
/// This decoder is length-delimited, so it needs to tolerate a terminator,
/// never to act on one -- hence `NoOp` rather than an early `break`. A
/// `break` here would silently discard every command after the first
/// terminator in a coalesced stream.
const STREAM_TERMINATOR: u8 = 0xdf & 0x3f;
/// `G_TEXRECT` (`texrectLLE`, opcode `0x24`); public SGI *RDP Command
/// Summary* "Texture Rectangle".
const TEXRECT: u8 = 0x24;
/// `G_TEXRECTFLIP` (`texrectFlipLLE`, opcode `0x25`); public SGI *RDP
/// Command Summary* "Texture Rectangle Flip".
const TEXRECT_FLIP: u8 = 0x25;
/// `G_SETPRIMDEPTH` (`src/shared/rt64_f3d_defines.h:157`, pinned commit
/// `5473732a822a4423b5696e7cb18fecc425a59875`); public SGI *RDP Command
/// Summary* "Set Primitive Depth".
const SET_PRIM_DEPTH: u8 = 0xee & 0x3f;
/// `G_SETFOGCOLOR` (`src/shared/rt64_f3d_defines.h:148`); public SGI *RDP
/// Command Summary* "Set Fog Color".
const SET_FOG_COLOR: u8 = 0xf8 & 0x3f;
/// `G_SETBLENDCOLOR` (`src/shared/rt64_f3d_defines.h:147`); public SGI *RDP
/// Command Summary* "Set Blend Color".
const SET_BLEND_COLOR: u8 = 0xf9 & 0x3f;
/// `G_SETPRIMCOLOR` (`src/shared/rt64_f3d_defines.h:146`); public SGI *RDP
/// Command Summary* "Set Primitive Color".
const SET_PRIM_COLOR: u8 = 0xfa & 0x3f;
/// `G_SETENVCOLOR` (`src/shared/rt64_f3d_defines.h:145`); public SGI *RDP
/// Command Summary* "Set Environment Color".
const SET_ENV_COLOR: u8 = 0xfb & 0x3f;
/// `G_SETCOMBINE` (`src/shared/rt64_f3d_defines.h:144`); public SGI *RDP
/// Command Summary* "Set Combine Mode" / libultra `gDPSetCombineMode` /
/// `gsDPSetCombineLERP`.
const SET_COMBINE: u8 = 0xfc & 0x3f;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawDpcCommandLocation {
    workload: WorkloadIdentity,
    stream: RawStreamIdentity,
    stream_index: u32,
    chunk_index: u32,
    stream_byte_offset: u32,
    source_byte_offset: u32,
    wire_opcode: u8,
}

impl RawDpcCommandLocation {
    pub const fn workload(self) -> WorkloadIdentity {
        self.workload
    }

    pub const fn stream(self) -> RawStreamIdentity {
        self.stream
    }

    pub const fn stream_index(self) -> u32 {
        self.stream_index
    }

    pub const fn chunk_index(self) -> u32 {
        self.chunk_index
    }

    pub const fn stream_byte_offset(self) -> u32 {
        self.stream_byte_offset
    }

    pub const fn source_byte_offset(self) -> u32 {
        self.source_byte_offset
    }

    pub const fn wire_opcode(self) -> u8 {
        self.wire_opcode
    }
}

impl fmt::Display for RawDpcCommandLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "workload {} stream {} (index {}) chunk {} source byte offset {:#x} stream byte offset {:#x} wire opcode {:#04x}",
            self.workload,
            self.stream,
            self.stream_index,
            self.chunk_index,
            self.source_byte_offset,
            self.stream_byte_offset,
            self.wire_opcode,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillRectangle {
    upper_left_x: u16,
    upper_left_y: u16,
    lower_right_x: u16,
    lower_right_y: u16,
}

impl FillRectangle {
    /// Reconstructs the decode payload from the four raw 12-bit wire
    /// fields. No longer test-only: `production`'s fill executor
    /// reconstructs one from the neutral `RdpFillRectangleCommand`, which
    /// carries the identical undivided wire fields precisely so the
    /// execution-time rectangle is the decoder's own, not a re-derivation.
    pub(crate) const fn from_wire_fields(
        upper_left_x: u16,
        upper_left_y: u16,
        lower_right_x: u16,
        lower_right_y: u16,
    ) -> Self {
        Self {
            upper_left_x,
            upper_left_y,
            lower_right_x,
            lower_right_y,
        }
    }

    pub const fn upper_left_x(self) -> u16 {
        self.upper_left_x
    }

    pub const fn upper_left_y(self) -> u16 {
        self.upper_left_y
    }

    pub const fn lower_right_x(self) -> u16 {
        self.lower_right_x
    }

    pub const fn lower_right_y(self) -> u16 {
        self.lower_right_y
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawDpcCommandKind {
    NoOp {
        variant: u8,
    },
    SetOtherMode(OtherMode),
    /// `G_SETSCISSOR` (`0x2d`), **staged RDP state**.
    ///
    /// Carries the rect exactly as
    /// [`crate::rt64_gbi_rdp_decode::decode_set_scissor`] -- the pinned
    /// RT64 port, reused verbatim rather than re-derived -- reads it, and
    /// stages into [`RdpState`]/[`RdpStateDelta`] like every other `Set*`
    /// kind here, as a [`crate::targets::RdpScissorRect`] in the same
    /// quarter-pixel units the wire carries.
    ///
    /// It was previously tracked-only, and this doc previously claimed
    /// "no draw, clip, or bounds computation in this crate reads a scissor
    /// rect today". That was accurate when written, and is precisely why a
    /// texrect overhanging the framebuffer was refused outright instead of
    /// clipped. `execute_texture_rectangle` now clips against this rect;
    /// pinned RT64 likewise intersects its current scissor with the draw
    /// rectangle (`src/hle/rt64_rdp.cpp:1214-1223`, commit `f0728a2`), so the
    /// value has to survive into durable state:
    /// a display list commonly sets the scissor once per frame and then
    /// submits several packets under it.
    SetScissor(crate::rt64_gbi_rdp_decode::SetScissorDecoded),
    SetColorImage(ColorImage),
    /// `G_SETZIMG` (`0xfe`) or its `G_SETMASKIMG` (`0x3e`) alias -- binds
    /// the depth buffer. Admitted as **tracked-only** at the neutral-IR
    /// seam (like [`RawDpcCommandKind::SetScissor`]): its presence is what
    /// makes a z-compared/z-updated draw legal, but the depth test in the
    /// wgpu CPU raster path reads the per-draw `OtherMode` z bits and the
    /// staged `SetPrimDepth`, not this address, because the test corpus's
    /// z-buffer is the zeroed RDRAM region and is never read back through a
    /// second guest image. See `stage_color_commands`' depth accumulator.
    SetZImage(ZImage),
    SetFillColor(FillColor),
    SetEnvColor(Color4),
    SetPrimColor(PrimColor),
    SetBlendColor(Color4),
    SetFogColor(Color4),
    SetPrimDepth(PrimDepth),
    SetCombine(CombineParams),
    FillRectangle(FillRectangle),
    SetTextureImage(crate::TextureImage),
    SetTile {
        tile: crate::TileIndex,
        descriptor: crate::TileDescriptor,
    },
    SetTileSize {
        tile: crate::TileIndex,
        size: crate::TileSize,
    },
    LoadSync(TmemLoadEpoch),
    LoadBlock(TmemLoad),
    LoadTile(TmemLoad),
    LoadTlut(TmemLoad),
    FullSync(FullSyncOccurrence),
    RawTriangle(RawTriangle),
    TextureRectangle(RawTextureRectangle),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodedRawDpcCommand {
    location: RawDpcCommandLocation,
    kind: RawDpcCommandKind,
}

impl DecodedRawDpcCommand {
    pub const fn location(self) -> RawDpcCommandLocation {
        self.location
    }

    pub const fn kind(self) -> RawDpcCommandKind {
        self.kind
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RawDpcResourcePlan {
    tmem_source_identity: TmemLoadSourceIdentity,
    accesses: Box<[ResourceAccess]>,
    tmem_transfers: Box<[TmemTransferRecord]>,
    fill_spans: Box<[FillAccessSpan]>,
    /// One entry per admitted `FillRectangle`, naming the declared guest
    /// read that carries its colour-image seed, or `None` when the fill is
    /// full-extent and needs none. See [`FillSeedRead`].
    fill_seeds: Box<[FillSeedRead]>,
}

/// The exact ordered `ResourceAccess` run one admitted `FillRectangle`
/// pushed into the plan's own access list, recorded by `plan_fill` at the
/// moment it pushed them.
///
/// Exists so the production adapter can hand
/// `ExactRawDpcPlanWriter::push_fill_rectangle` the *decoder's* access slice
/// rather than re-deriving it: two independent derivations of the same
/// access list is exactly the divergence `ExactRawDpcPlanWriter::finish`'s
/// access-for-access check exists to catch, and re-deriving would turn that
/// sealed guarantee into a runtime coin flip.
///
/// `count` is `1` for a full-image-width fill and the rectangle's pixel
/// height otherwise -- a partial-width rectangle's rows occupy disjoint,
/// width-strided RDRAM ranges, so collapsing them would declare untouched
/// inter-row bytes as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillAccessSpan {
    command_index: u32,
    first_access_index: u32,
    count: u32,
}

impl FillAccessSpan {
    pub const fn command_index(self) -> u32 {
        self.command_index
    }

    pub const fn first_access_index(self) -> u32 {
        self.first_access_index
    }

    pub const fn count(self) -> u32 {
        self.count
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TmemTransferRecord {
    plan: TmemTransferPlan,
    words: Box<[TmemTransferWord]>,
}

#[derive(Debug)]
/// An immutable view whose load, ordered words, and exact journal slices have
/// been checked against one decoded resource plan. This is not a consuming
/// execution capability and may be reconstructed while that plan is borrowed.
pub struct BoundTmemTransfer<'a> {
    load: TmemLoad,
    source_accesses: &'a [ResourceAccess],
    destination_accesses: &'a [ResourceAccess],
    words: &'a [TmemTransferWord],
}

impl BoundTmemTransfer<'_> {
    pub const fn load(&self) -> TmemLoad {
        self.load
    }

    pub const fn source_accesses(&self) -> &[ResourceAccess] {
        self.source_accesses
    }

    pub const fn destination_accesses(&self) -> &[ResourceAccess] {
        self.destination_accesses
    }

    pub const fn words(&self) -> &[TmemTransferWord] {
        self.words
    }
}

/// A recorded `FillRectangle` access span could not be bound back to the
/// plan's own ordered access list. Every variant means the decoder and the
/// resource plan disagree about what an admitted fill writes -- a loud
/// rejection, never a silently substituted access slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FillAccessSpanError {
    #[error("no FillRectangle access span was declared for command #{command_index}")]
    FillNotDeclared { command_index: u32 },
    #[error("FillRectangle command #{command_index}'s access span is out of bounds")]
    AccessSliceOutOfBounds { command_index: u32 },
    #[error(
        "FillRectangle command #{command_index}'s access span is not a run of \
         RenderTarget color-framebuffer writes"
    )]
    AccessDescriptorsDiffer { command_index: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TmemLoadSourcePlanError {
    #[error("TMEM source plan belongs to a different decode")]
    DecodeIdentityMismatch,
    #[error("TMEM source plan access slice is out of bounds")]
    AccessSliceOutOfBounds,
    #[error("TMEM source plan differs from the exact ordered source descriptors")]
    AccessDescriptorsDiffer,
    #[error("TMEM transfer plan is not declared by this decode")]
    TransferNotDeclared,
    #[error("TMEM destination plan belongs to a different source/decode identity")]
    DestinationDecodeIdentityMismatch,
    #[error("TMEM destination plan access slice is out of bounds")]
    DestinationAccessSliceOutOfBounds,
    #[error("TMEM destination plan differs from the canonical sorted destination union")]
    DestinationAccessDescriptorsDiffer,
    #[error("YUV destination execution is deferred pending a public pairing contract")]
    YuvExecutionDeferred,
}

impl RawDpcResourcePlan {
    pub fn accesses(&self) -> &[ResourceAccess] {
        &self.accesses
    }

    /// The exact ordered access slice `plan_fill` pushed for the admitted
    /// `FillRectangle` at decode-order position `command_index`, together
    /// with the span record itself.
    ///
    /// Every returned access is re-checked here to be an
    /// `AccessMode::Write`/`AccessPurpose::RenderTarget` RDRAM
    /// `ColorFramebuffer` region: the span was recorded by the decoder, but
    /// a span that no longer describes render-target writes means the plan
    /// and the decoder disagree, which is a loud rejection rather than a
    /// slice handed on unchecked.
    pub fn bind_fill_rectangle(
        &self,
        command_index: u32,
    ) -> Result<(FillAccessSpan, &[ResourceAccess]), FillAccessSpanError> {
        let span = self
            .fill_spans
            .iter()
            .copied()
            .find(|span| span.command_index == command_index)
            .ok_or(FillAccessSpanError::FillNotDeclared { command_index })?;
        let start = usize::try_from(span.first_access_index)
            .map_err(|_| FillAccessSpanError::AccessSliceOutOfBounds { command_index })?;
        let end = start
            .checked_add(span.count as usize)
            .ok_or(FillAccessSpanError::AccessSliceOutOfBounds { command_index })?;
        let accesses = self
            .accesses
            .get(start..end)
            .ok_or(FillAccessSpanError::AccessSliceOutOfBounds { command_index })?;
        if accesses.is_empty() {
            return Err(FillAccessSpanError::AccessSliceOutOfBounds { command_index });
        }
        let exact = accesses.iter().all(|access| {
            access.mode() == AccessMode::Write
                && access.purpose() == AccessPurpose::RenderTarget
                && matches!(
                    access.region(),
                    ResourceRegion::Rdram {
                        resource: RdramResource::ColorFramebuffer,
                        ..
                    }
                )
        });
        if !exact {
            return Err(FillAccessSpanError::AccessDescriptorsDiffer { command_index });
        }
        Ok((span, accesses))
    }

    /// The declared guest read carrying the colour-image seed for the fill
    /// at decode-order position `command_index`.
    ///
    /// `Ok(None)` means that fill covers the whole target and legitimately
    /// declared no seed -- every byte comes from the command itself.
    /// `Err(FillNotDeclared)` means no seed record exists for the command at
    /// all, which is a decoder bug rather than a full-extent fill, and is
    /// kept distinguishable for exactly that reason.
    pub fn bind_fill_seed(&self, command_index: u32) -> Result<Option<u32>, FillAccessSpanError> {
        self.fill_seeds
            .iter()
            .copied()
            .find(|seed| seed.command_index == command_index)
            .map(|seed| seed.access_index)
            .ok_or(FillAccessSpanError::FillNotDeclared { command_index })
    }

    /// The exact ordered access slice `plan_texture_rectangle` pushed for the
    /// admitted `TextureRectangle` at decode-order position `command_index`,
    /// or an empty slice when that texrect declared no destination write.
    ///
    /// Mirrors [`Self::bind_fill_rectangle`] -- same span table, same
    /// `Write`/`RenderTarget`/`ColorFramebuffer` re-check on every access, so
    /// a span that no longer describes render-target writes is a loud
    /// rejection rather than a slice handed on unchecked.
    ///
    /// Differs from the fill binder in exactly one way, deliberately: a
    /// texrect with **no** recorded span is `Ok(&[])`, not
    /// `FillNotDeclared`. A texrect legitimately declares no write when its
    /// destination is not provable at decode time (see
    /// `plan_texture_rectangle`'s contract), whereas an admitted fill that
    /// declared none is a decoder bug.
    pub fn bind_texture_rectangle(
        &self,
        command_index: u32,
    ) -> Result<&[ResourceAccess], FillAccessSpanError> {
        let Some(span) = self
            .fill_spans
            .iter()
            .copied()
            .find(|span| span.command_index == command_index)
        else {
            return Ok(&[]);
        };
        let start = usize::try_from(span.first_access_index)
            .map_err(|_| FillAccessSpanError::AccessSliceOutOfBounds { command_index })?;
        let end = start
            .checked_add(span.count as usize)
            .ok_or(FillAccessSpanError::AccessSliceOutOfBounds { command_index })?;
        let accesses = self
            .accesses
            .get(start..end)
            .ok_or(FillAccessSpanError::AccessSliceOutOfBounds { command_index })?;
        if accesses.is_empty() {
            return Err(FillAccessSpanError::AccessSliceOutOfBounds { command_index });
        }
        let exact = accesses.iter().all(|access| {
            access.mode() == AccessMode::Write
                && access.purpose() == AccessPurpose::RenderTarget
                && matches!(
                    access.region(),
                    ResourceRegion::Rdram {
                        resource: RdramResource::ColorFramebuffer,
                        ..
                    }
                )
        });
        if !exact {
            return Err(FillAccessSpanError::AccessDescriptorsDiffer { command_index });
        }
        Ok(accesses)
    }

    pub fn tmem_load_source_accesses(
        &self,
        plan: TmemLoadSourcePlan,
    ) -> Result<&[ResourceAccess], TmemLoadSourcePlanError> {
        if plan.identity() != self.tmem_source_identity {
            return Err(TmemLoadSourcePlanError::DecodeIdentityMismatch);
        }
        let start = usize::try_from(plan.first_access_index())
            .map_err(|_| TmemLoadSourcePlanError::AccessSliceOutOfBounds)?;
        let end = start
            .checked_add(usize::from(plan.access_count()))
            .ok_or(TmemLoadSourcePlanError::AccessSliceOutOfBounds)?;
        let accesses = self
            .accesses
            .get(start..end)
            .ok_or(TmemLoadSourcePlanError::AccessSliceOutOfBounds)?;
        let exact_descriptors = accesses.iter().enumerate().all(|(offset, access)| {
            access.operation().get() == plan.first_operation().get() + offset as u32
                && access.mode() == AccessMode::Read
                && access.purpose() == AccessPurpose::TmemLoadSource
                && matches!(
                    access.region(),
                    ResourceRegion::Rdram {
                        resource: RdramResource::Buffer,
                        range,
                    } if range.layout() == plan.identity().memory_layout()
                )
        }) && accesses.iter().try_fold(0_u32, |total, access| {
            total.checked_add(access.region().declared_bytes())
        }) == Some(plan.total_bytes())
            && access_identity(accesses) == plan.source_access_identity();
        if !exact_descriptors {
            return Err(TmemLoadSourcePlanError::AccessDescriptorsDiffer);
        }
        Ok(accesses)
    }

    pub fn bind_tmem_transfer(
        &self,
        load: TmemLoad,
    ) -> Result<BoundTmemTransfer<'_>, TmemLoadSourcePlanError> {
        let plan = match load.contract() {
            TmemLoadContract::Transfer(plan) => plan,
            TmemLoadContract::DeferredYuv { .. } => {
                return Err(TmemLoadSourcePlanError::YuvExecutionDeferred);
            }
        };
        let record = self
            .tmem_transfers
            .iter()
            .find(|record| record.plan == plan)
            .ok_or(TmemLoadSourcePlanError::TransferNotDeclared)?;
        let source_accesses = self.tmem_load_source_accesses(plan.source())?;
        let destination = plan.destination();
        if destination.identity() != self.tmem_source_identity
            || destination.identity() != plan.source().identity()
            || destination.source_access_identity() != plan.source().source_access_identity()
        {
            return Err(TmemLoadSourcePlanError::DestinationDecodeIdentityMismatch);
        }
        let start = usize::try_from(destination.first_access_index())
            .map_err(|_| TmemLoadSourcePlanError::DestinationAccessSliceOutOfBounds)?;
        let end = start
            .checked_add(usize::from(destination.access_count()))
            .ok_or(TmemLoadSourcePlanError::DestinationAccessSliceOutOfBounds)?;
        let destination_accesses = self
            .accesses
            .get(start..end)
            .ok_or(TmemLoadSourcePlanError::DestinationAccessSliceOutOfBounds)?;
        let exact_descriptors = destination_accesses
            .iter()
            .enumerate()
            .all(|(offset, access)| {
                access.operation().get() == destination.first_operation().get() + offset as u32
                    && access.mode() == AccessMode::Write
                    && access.purpose() == AccessPurpose::TmemLoadDestination
                    && matches!(access.region(), ResourceRegion::Tmem(_))
            })
            && destination_accesses
                .iter()
                .try_fold(0_u32, |total, access| {
                    total.checked_add(access.region().declared_bytes())
                })
                == Some(destination.total_bytes())
            && access_identity(destination_accesses) == destination.destination_access_identity();
        if !exact_descriptors {
            return Err(TmemLoadSourcePlanError::DestinationAccessDescriptorsDiffer);
        }
        Ok(BoundTmemTransfer {
            load,
            source_accesses,
            destination_accesses,
            words: &record.words,
        })
    }
}

fn access_identity(accesses: &[ResourceAccess]) -> fn64_render_ir::JournalIdentity {
    let total_bytes = accesses
        .iter()
        .try_fold(0_u32, |total, access| {
            total.checked_add(access.region().declared_bytes())
        })
        .expect("decoder-admitted resource bytes fit u32");
    ResourceJournal::try_new(
        ResourceJournalLimits::try_new(accesses.len(), total_bytes)
            .expect("TMEM source slice is nonempty and bounded"),
        accesses.to_vec(),
    )
    .expect("TMEM source slice came from an admitted resource journal")
    .identity()
}

fn transfer_record(
    plan: TmemTransferPlan,
    accesses: &[ResourceAccess],
) -> Result<TmemTransferRecord, TmemLoadSourcePlanError> {
    let source_start = usize::try_from(plan.source().first_access_index())
        .map_err(|_| TmemLoadSourcePlanError::AccessSliceOutOfBounds)?;
    let source_end = source_start
        .checked_add(usize::from(plan.source().access_count()))
        .ok_or(TmemLoadSourcePlanError::AccessSliceOutOfBounds)?;
    let sources = accesses
        .get(source_start..source_end)
        .ok_or(TmemLoadSourcePlanError::AccessSliceOutOfBounds)?;
    let mut words = Vec::with_capacity(usize::from(plan.transfer_words()));
    for index in 0..plan.transfer_words() {
        let geometry = plan
            .geometry_for_word(index)
            .map_err(|_| TmemLoadSourcePlanError::AccessDescriptorsDiffer)?;
        let logical_offset = plan
            .logical_source_offset(index)
            .map_err(|_| TmemLoadSourcePlanError::AccessDescriptorsDiffer)?;
        // **Bound ROW-LOCALLY, not by walking concatenated lengths.**
        //
        // The declared source reads are PADDED to whole 64-bit words (each
        // row reads `words_per_row * 8` bytes, because hardware copies whole
        // words), so consecutive accesses are no longer contiguous in logical
        // texel space. Walking their lengths against a LOGICAL offset -- what
        // this did -- binds later rows into the wrong access as soon as a row
        // carries any padding.
        //
        // Each transfer word's row and position within it are already known
        // from the plan's own geometry, so the binding is direct: access
        // `first + row`, offset `within_row * 8`. A single-access plan
        // (LoadBlock, LoadTLUT) has `row == 0` and reduces to the previous
        // behaviour.
        let words_per_row = u32::from(plan.words_per_row().max(1));
        let row = u32::from(index) / words_per_row;
        let within_row = u32::from(index) % words_per_row;
        let source_ordinal = if sources.len() == 1 { 0 } else { row };
        let source_access_index = plan
            .source()
            .first_access_index()
            .checked_add(source_ordinal)
            .ok_or(TmemLoadSourcePlanError::AccessSliceOutOfBounds)?;
        let source_access_byte_offset = if sources.len() == 1 {
            logical_offset
        } else {
            within_row
                .checked_mul(8)
                .ok_or(TmemLoadSourcePlanError::AccessSliceOutOfBounds)?
        };
        if usize::try_from(source_ordinal).unwrap_or(usize::MAX) >= sources.len() {
            return Err(TmemLoadSourcePlanError::AccessSliceOutOfBounds);
        }
        words.push(TmemTransferWord::new(
            index,
            logical_offset,
            source_access_index,
            source_access_byte_offset,
            plan.defined_source_byte_mask(index)
                .map_err(|_| TmemLoadSourcePlanError::AccessDescriptorsDiffer)?,
            plan.defined_destination_byte_mask(index)
                .map_err(|_| TmemLoadSourcePlanError::AccessDescriptorsDiffer)?,
            geometry.destination_word(),
            geometry.row_advance(),
            geometry.odd_row_exchange(),
            geometry.physical(),
        ));
    }
    Ok(TmemTransferRecord {
        plan,
        words: words.into_boxed_slice(),
    })
}

#[derive(Debug)]
pub struct DecodedRawDpc {
    submitted: SubmittedTicket,
    base_state: RdpState,
    commands: Box<[DecodedRawDpcCommand]>,
    state_delta: RdpStateDelta,
    staged_state: StagedRdpState,
    resource_plan: RawDpcResourcePlan,
    origin: RawDpcDecodeOrigin,
}

/// A decoder result whose resource plan is authoritative even though the
/// submitted ticket carried only a preflight seed journal. This type cannot
/// enter ordinary decoded execution; the planning adapter is its sole
/// consumer and seals the derived accesses into the real plan journal.
pub(crate) struct PlanningDecodedRawDpc {
    base_state: RdpState,
    commands: Box<[DecodedRawDpcCommand]>,
    state_delta: RdpStateDelta,
    resource_plan: RawDpcResourcePlan,
}

impl PlanningDecodedRawDpc {
    pub(crate) fn commands(&self) -> &[DecodedRawDpcCommand] {
        &self.commands
    }

    pub(crate) const fn state_delta(&self) -> &RdpStateDelta {
        &self.state_delta
    }

    pub(crate) const fn resource_plan(&self) -> &RawDpcResourcePlan {
        &self.resource_plan
    }
}

impl DecodedRawDpc {
    pub const fn submitted(&self) -> &SubmittedTicket {
        &self.submitted
    }

    pub fn commands(&self) -> &[DecodedRawDpcCommand] {
        &self.commands
    }

    pub const fn state_delta(&self) -> &RdpStateDelta {
        &self.state_delta
    }

    pub const fn staged_state(&self) -> &StagedRdpState {
        &self.staged_state
    }

    pub const fn resource_plan(&self) -> &RawDpcResourcePlan {
        &self.resource_plan
    }

    pub fn into_staged_state(self) -> StagedRdpState {
        self.staged_state
    }

    pub(crate) fn into_contract_parts(self) -> DecodedRawDpcParts {
        DecodedRawDpcParts {
            submitted: self.submitted,
            base_state: self.base_state,
            commands: self.commands,
            state_delta: self.state_delta,
            staged_state: self.staged_state,
            resource_plan: self.resource_plan,
            origin: self.origin,
        }
    }
}

pub(crate) struct DecodedRawDpcParts {
    pub(crate) submitted: SubmittedTicket,
    pub(crate) base_state: RdpState,
    pub(crate) commands: Box<[DecodedRawDpcCommand]>,
    pub(crate) state_delta: RdpStateDelta,
    pub(crate) staged_state: StagedRdpState,
    pub(crate) resource_plan: RawDpcResourcePlan,
    pub(crate) origin: RawDpcDecodeOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawDpcDecodeOrigin {
    Durable,
    SpeculativeStaged,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RawDpcDecodeError {
    #[error("workload {workload} is not admitted as raw DPC")]
    UnsupportedAdmission {
        workload: WorkloadIdentity,
    },
    #[error("workload {workload} cannot chain staged RDP state: {reason}")]
    StagedStateMismatch {
        workload: WorkloadIdentity,
        reason: &'static str,
    },
    #[error("{location}: command has no admitted public width")]
    UnknownCommandWidth {
        location: RawDpcCommandLocation,
    },
    #[error("{location}: {width}-byte command is truncated with {available} bytes available")]
    TruncatedCommand {
        location: RawDpcCommandLocation,
        width: u32,
        available: u32,
    },
    #[error(
        "{location}: decoded opcode {decoded_opcode:#04x} has public width {width} but is outside the M3.2 subset"
    )]
    UnsupportedCommand {
        location: RawDpcCommandLocation,
        decoded_opcode: u8,
        width: u32,
    },
    #[error("{location}: state-invalid command: {reason}")]
    InvalidCommand {
        location: RawDpcCommandLocation,
        reason: &'static str,
    },
    #[error("workload {workload} resource-plan operation IDs overflow u32")]
    ResourcePlanOverflow {
        workload: WorkloadIdentity,
    },
    #[error(
        "workload {workload} resource journal differs from exact decoder plan: expected {} accesses, found {}",
        expected.len(),
        actual.len()
    )]
    JournalMismatch {
        workload: WorkloadIdentity,
        expected: Box<[ResourceAccess]>,
        actual: Box<[ResourceAccess]>,
    },
    #[error("{0}")]
    Ir(ValidationError),
}

impl From<ValidationError> for RawDpcDecodeError {
    fn from(error: ValidationError) -> Self {
        Self::Ir(error)
    }
}

pub fn decode_raw_dpc(
    submitted: SubmittedTicket,
    durable_state: &RdpState,
) -> Result<DecodedRawDpc, RawDpcDecodeError> {
    decode_exact_from_state(
        submitted,
        durable_state.fork_for_decode(),
        RawDpcDecodeOrigin::Durable,
    )
}

pub(crate) fn decode_raw_dpc_for_planning(
    submitted: SubmittedTicket,
    durable_state: &RdpState,
) -> Result<PlanningDecodedRawDpc, RawDpcDecodeError> {
    decode_derivation(
        submitted,
        durable_state.fork_for_decode(),
        RawDpcDecodeOrigin::Durable,
    )
    .map(RawDpcDecodeDerivation::into_planning)
}

pub fn decode_raw_dpc_after(
    submitted: SubmittedTicket,
    staged_state: StagedRdpState,
) -> Result<DecodedRawDpc, RawDpcDecodeError> {
    let workload = submitted.packet().identity();
    let (state, queue, prior_ordinal, prior_sequence) = staged_state.into_parts();
    if submitted.queue() != queue {
        return Err(RawDpcDecodeError::StagedStateMismatch {
            workload,
            reason: "submission queue differs from the staged-state owner",
        });
    }
    let WorkloadAdmission::RawDpc {
        transaction_sequence,
    } = submitted.packet().admission()
    else {
        return Err(RawDpcDecodeError::UnsupportedAdmission { workload });
    };
    if prior_ordinal.checked_add(1) != Some(submitted.ordinal()) {
        return Err(RawDpcDecodeError::StagedStateMismatch {
            workload,
            reason: "semantic submission is not the immediate successor",
        });
    }
    if prior_sequence.checked_add(1) != Some(transaction_sequence) {
        return Err(RawDpcDecodeError::StagedStateMismatch {
            workload,
            reason: "raw-DPC transaction sequence is not the immediate successor",
        });
    }
    decode_exact_from_state(submitted, state, RawDpcDecodeOrigin::SpeculativeStaged)
}

struct RawDpcDecodeDerivation {
    submitted: SubmittedTicket,
    base_state: RdpState,
    commands: Box<[DecodedRawDpcCommand]>,
    state_delta: RdpStateDelta,
    staged_state: StagedRdpState,
    resource_plan: RawDpcResourcePlan,
    origin: RawDpcDecodeOrigin,
}

impl RawDpcDecodeDerivation {
    fn into_exact(self) -> DecodedRawDpc {
        DecodedRawDpc {
            submitted: self.submitted,
            base_state: self.base_state,
            commands: self.commands,
            state_delta: self.state_delta,
            staged_state: self.staged_state,
            resource_plan: self.resource_plan,
            origin: self.origin,
        }
    }

    fn into_planning(self) -> PlanningDecodedRawDpc {
        PlanningDecodedRawDpc {
            base_state: self.base_state,
            commands: self.commands,
            state_delta: self.state_delta,
            resource_plan: self.resource_plan,
        }
    }
}

fn decode_exact_from_state(
    submitted: SubmittedTicket,
    state: RdpState,
    origin: RawDpcDecodeOrigin,
) -> Result<DecodedRawDpc, RawDpcDecodeError> {
    let derived = decode_derivation(submitted, state, origin)?;
    let actual = derived.submitted.packet().journal().accesses();
    let expected = derived.resource_plan.accesses();
    if actual != expected {
        return Err(RawDpcDecodeError::JournalMismatch {
            workload: derived.submitted.packet().identity(),
            expected: expected.to_vec().into_boxed_slice(),
            actual: actual.to_vec().into_boxed_slice(),
        });
    }
    Ok(derived.into_exact())
}

fn decode_derivation(
    submitted: SubmittedTicket,
    mut state: RdpState,
    origin: RawDpcDecodeOrigin,
) -> Result<RawDpcDecodeDerivation, RawDpcDecodeError> {
    // The contract layer compares this immutable predecessor with its
    // exclusively borrowed durable state before it admits GPU work. Keeping
    // the proof inside the move-only decoded value closes the otherwise-safe
    // but incorrect possibility of preparing a decode against another state.
    let base_state = state.fork_for_decode();
    let submission = submitted.identity();
    let packet = submitted.packet();
    let workload = packet.identity();
    let WorkloadAdmission::RawDpc {
        transaction_sequence,
    } = packet.admission()
    else {
        return Err(RawDpcDecodeError::UnsupportedAdmission { workload });
    };
    let queue = submitted.queue();
    let submission_ordinal = submitted.ordinal();
    let tmem_source_identity = TmemLoadSourceIdentity::new(
        workload,
        packet.journal().identity(),
        submission,
        packet.memory_layout(),
    );

    let mut planned = Vec::new();
    for stream in packet.streams() {
        let (start, end) = stream.source_bounds();
        let region = match stream {
            RawCommandStream::Dram(_) => ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: packet.memory_layout().range(start, end)?,
            },
            RawCommandStream::Xbus(_) => ResourceRegion::RspDmem(DmemRange::try_new(start, end)?),
        };
        push_access(
            workload,
            &mut planned,
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            region,
        )?;
    }

    let mut delta = RdpStateDelta::default();
    let mut commands = Vec::new();
    let mut fill_spans: Vec<FillAccessSpan> = Vec::new();
    let mut fill_seeds: Vec<FillSeedRead> = Vec::new();
    for (stream_index, stream) in packet.streams().iter().enumerate() {
        let flattened = FlattenedStream::new(workload, stream_index, stream);
        decode_stream(
            &flattened,
            packet.memory_layout(),
            &mut state,
            tmem_source_identity,
            &mut delta,
            &mut planned,
            &mut commands,
            &mut fill_spans,
            &mut fill_seeds,
        )?;
    }

    let resource_accesses = planned.into_boxed_slice();
    let tmem_transfers = commands
        .iter()
        .filter_map(|command| match command.kind {
            RawDpcCommandKind::LoadBlock(load)
            | RawDpcCommandKind::LoadTile(load)
            | RawDpcCommandKind::LoadTlut(load) => load.transfer_plan().ok(),
            _ => None,
        })
        .map(|plan| {
            transfer_record(plan, &resource_accesses)
                .expect("decoder-created TMEM transfer must bind its exact admitted journal")
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let staged_state =
        StagedRdpState::from_transaction(state, queue, submission_ordinal, transaction_sequence);

    let commands = commands.into_boxed_slice();
    let resource_plan = RawDpcResourcePlan {
        tmem_source_identity,
        accesses: resource_accesses,
        tmem_transfers,
        fill_spans: fill_spans.into_boxed_slice(),
        fill_seeds: fill_seeds.into_boxed_slice(),
    };
    Ok(RawDpcDecodeDerivation {
        submitted,
        base_state,
        commands,
        state_delta: delta,
        staged_state,
        resource_plan,
        origin,
    })
}

#[derive(Clone, Copy)]
struct ChunkSpan {
    index: u32,
    stream_start: u32,
    stream_end: u32,
    source_start: u32,
}

struct FlattenedStream {
    workload: WorkloadIdentity,
    stream: RawStreamIdentity,
    stream_index: u32,
    bytes: Vec<u8>,
    chunks: Vec<ChunkSpan>,
    full_syncs: Vec<FullSyncOccurrence>,
}

impl FlattenedStream {
    fn new(workload: WorkloadIdentity, stream_index: usize, stream: &RawCommandStream) -> Self {
        let mut bytes = Vec::with_capacity(stream.byte_len() as usize);
        let mut chunks = Vec::with_capacity(stream.chunk_count());
        let mut stream_start = 0;
        match stream {
            RawCommandStream::Dram(stream) => {
                for (index, chunk) in stream.chunks().iter().enumerate() {
                    bytes.extend(chunk.words().iter().flat_map(|word| word.to_be_bytes()));
                    let stream_end = stream_start + chunk.range().len();
                    chunks.push(ChunkSpan {
                        index: index as u32,
                        stream_start,
                        stream_end,
                        source_start: chunk.range().start().get(),
                    });
                    stream_start = stream_end;
                }
            }
            RawCommandStream::Xbus(stream) => {
                for (index, chunk) in stream.chunks().iter().enumerate() {
                    bytes.extend_from_slice(chunk.bytes());
                    let stream_end = stream_start + chunk.range().len();
                    chunks.push(ChunkSpan {
                        index: index as u32,
                        stream_start,
                        stream_end,
                        source_start: chunk.range().start(),
                    });
                    stream_start = stream_end;
                }
            }
        }
        Self {
            workload,
            stream: stream.identity(),
            stream_index: stream_index as u32,
            bytes,
            chunks,
            full_syncs: stream.full_sync_occurrences().to_vec(),
        }
    }

    fn location(&self, stream_byte_offset: u32, wire_opcode: u8) -> RawDpcCommandLocation {
        let chunk = self
            .chunks
            .iter()
            .find(|chunk| stream_byte_offset < chunk.stream_end)
            .or_else(|| self.chunks.last())
            .expect("IR command streams are nonempty");
        let within = stream_byte_offset.saturating_sub(chunk.stream_start);
        RawDpcCommandLocation {
            workload: self.workload,
            stream: self.stream,
            stream_index: self.stream_index,
            chunk_index: chunk.index,
            stream_byte_offset,
            source_byte_offset: chunk.source_start + within,
            wire_opcode,
        }
    }
}

fn decode_stream(
    stream: &FlattenedStream,
    layout: fn64_render_ir::PhysicalMemoryLayout,
    state: &mut RdpState,
    tmem_source_identity: TmemLoadSourceIdentity,
    delta: &mut RdpStateDelta,
    planned: &mut Vec<ResourceAccess>,
    commands: &mut Vec<DecodedRawDpcCommand>,
    fill_spans: &mut Vec<FillAccessSpan>,
    fill_seeds: &mut Vec<FillSeedRead>,
) -> Result<(), RawDpcDecodeError> {
    let mut offset = 0usize;
    while offset < stream.bytes.len() {
        let wire_opcode = stream.bytes[offset];
        let location = stream.location(offset as u32, wire_opcode);
        let Some(width) = raw_rdp_command_width(wire_opcode) else {
            return Err(RawDpcDecodeError::UnknownCommandWidth { location });
        };
        let available = stream.bytes.len() - offset;
        if available < width as usize {
            return Err(RawDpcDecodeError::TruncatedCommand {
                location,
                width,
                available: available as u32,
            });
        }
        let command = &stream.bytes[offset..offset + width as usize];
        let w0 = u32::from_be_bytes(command[..4].try_into().expect("width is at least 8"));
        let w1 = u32::from_be_bytes(command[4..8].try_into().expect("width is at least 8"));
        let opcode = wire_opcode & 0x3f;
        let kind = match opcode {
            0x00..=0x07 => RawDpcCommandKind::NoOp { variant: opcode },
            // The three ids WM2000 issues that this decoder used to abort on.
            // All three are admitted as `NoOp`, which is admitted and
            // discarded (`production_adapter`'s `NoOp` arm) and stages no
            // `RdpState`/`RdpStateDelta` -- so admitting them cannot move a
            // pixel, exactly the property that makes this safe to do without
            // designing a semantic.
            //
            // Measured, not assumed: 20,800 `SyncPipe` + 1,808 `SyncTile` +
            // 219 terminators, across 218/218 frames
            // (`docs/rt64/RT64-WM2000-CENSUS.md` §3). Each rejection aborted the
            // WHOLE stream rather than one command, so these three ids alone
            // held 0% of frames at zero decoded.
            //
            // `SyncPipe`/`SyncTile` are real RDP commands whose effect is
            // pipeline sequencing, not rasterization; `ReferenceBackend`
            // already groups them with `SyncLoad` as a no-op arm
            // (`fn64-render-reference`'s `gbi/stream.rs`), and this follows
            // that precedent rather than inventing a second reading.
            //
            // Nonclaim, and the reason these are `NoOp` rather than named
            // kinds: discarding a sync is correct only because this backend
            // has no pipeline to sequence. It is not a claim that RDP
            // synchronization is semantically empty. A backend that later
            // models pipeline hazards must revisit this arm, not inherit it.
            SYNC_PIPE | SYNC_TILE | STREAM_TERMINATOR => {
                RawDpcCommandKind::NoOp { variant: opcode }
            }
            SET_OTHER_MODE => {
                let value = OtherMode::from_wire(w0 & 0x00ff_ffff, w1);
                delta.set_other_mode(value);
                state.apply(delta);
                RawDpcCommandKind::SetOtherMode(value)
            }
            SET_COLOR_IMAGE => {
                let format = match (w0 >> 21) & 0x7 {
                    0 => ImageFormat::Rgba,
                    1 => ImageFormat::Yuv,
                    2 => ImageFormat::ColorIndex,
                    3 => ImageFormat::IntensityAlpha,
                    4 => ImageFormat::Intensity,
                    _ => {
                        return Err(RawDpcDecodeError::InvalidCommand {
                            location,
                            reason: "SetColorImage format is reserved",
                        });
                    }
                };
                let size = match (w0 >> 19) & 0x3 {
                    0 => PixelSize::Bits4,
                    1 => PixelSize::Bits8,
                    2 => PixelSize::Bits16,
                    _ => PixelSize::Bits32,
                };
                let address = w1 & 0x00ff_ffff;
                if !address.is_multiple_of(64) {
                    return Err(RawDpcDecodeError::InvalidCommand {
                        location,
                        reason: "SetColorImage address is not 64-byte aligned",
                    });
                }
                let address =
                    layout
                        .address(address)
                        .map_err(|_| RawDpcDecodeError::InvalidCommand {
                            location,
                            reason: "SetColorImage address is outside installed RDRAM",
                        })?;
                let value = ColorImage::from_wire(format, size, (w0 & 0x0fff) + 1, address);
                delta.set_color_image(value);
                state.apply(delta);
                RawDpcCommandKind::SetColorImage(value)
            }
            SET_Z_IMAGE => {
                // Both `G_SETZIMG` (`0xfe`) and `G_SETMASKIMG` (`0x3e`)
                // funnel here (both mask to `0x3e`), byte-identically:
                // angrylion's `zb_address = args[1] & 0x00ffffff`. The Z
                // image is not staged into `RdpState` (there is no neutral
                // field for it and no wgpu consumer reads the address); this
                // arm exists so a z-buffered stream is ADMITTED rather than
                // refused as `UnsupportedCommand`, which is the entire fn64
                // gap the six `gen-zbuffer-*` parity cases probe. The
                // address is masked and range-checked exactly as
                // `SetColorImage` does, so a malformed binding is a named
                // refusal rather than a silently swallowed one.
                let address = w1 & 0x00ff_ffff;
                let address =
                    layout
                        .address(address)
                        .map_err(|_| RawDpcDecodeError::InvalidCommand {
                            location,
                            reason: "SetZImage address is outside installed RDRAM",
                        })?;
                RawDpcCommandKind::SetZImage(ZImage::from_wire(address))
            }
            SET_FILL_COLOR => {
                let value = FillColor::from_wire(w1);
                delta.set_fill_color(value);
                state.apply(delta);
                RawDpcCommandKind::SetFillColor(value)
            }
            SET_ENV_COLOR => {
                let value = Color4::from_wire(w1);
                delta.set_env_color(value);
                state.apply(delta);
                RawDpcCommandKind::SetEnvColor(value)
            }
            SET_PRIM_COLOR => {
                let value = PrimColor::from_wire(w0, w1);
                delta.set_prim_color(value);
                state.apply(delta);
                RawDpcCommandKind::SetPrimColor(value)
            }
            SET_BLEND_COLOR => {
                let value = Color4::from_wire(w1);
                delta.set_blend_color(value);
                state.apply(delta);
                RawDpcCommandKind::SetBlendColor(value)
            }
            SET_FOG_COLOR => {
                let value = Color4::from_wire(w1);
                delta.set_fog_color(value);
                state.apply(delta);
                RawDpcCommandKind::SetFogColor(value)
            }
            SET_SCISSOR => {
                // Reuses the pinned RT64 port verbatim. `ulx`/`uly` come
                // from `p0(w0, 12, 12)`/`p0(w0, 0, 12)` -- both inside w0's
                // low 24-bit payload -- so passing the full wire word here
                // (opcode byte still in bits 24:31) reads exactly the same
                // fields the RT64 decoder would; no masking is needed and
                // none is applied, matching how `SET_COMBINE` below passes
                // `w0` unmasked.
                //
                // **Staged now, not merely tracked.** It used to be
                // tracked-only, on the reasoning that nothing in this crate
                // read a scissor rect -- true when written, and no longer
                // true: `execute_texture_rectangle` clips against it (see
                // `targets::clip_texrect_extent`); pinned RT64 also
                // intersects its scissor and draw rectangles
                // (`src/hle/rt64_rdp.cpp:1214-1223`, commit `f0728a2`), so
                // keeping it out of `RdpState`
                // would silently unscissor every packet that inherits the
                // rect from an earlier one.
                //
                // Latched in the decoder's own quarter-pixel wire units.
                // Public libultra `include/ultra64/gbi.h:4794-4837` encodes
                // the four coordinates as twelve-bit fields scaled by four,
                // or accepts the fractional wire values directly.
                let decoded = crate::rt64_gbi_rdp_decode::decode_set_scissor(w0, w1);
                // `p0`/`p1` mask to twelve bits and never sign-extend, so
                // every coordinate is in `0..=4095` and fits a `u16`. The
                // `expect`s are loud traps on that invariant rather than
                // silent `as` truncations, matching `neutral_scissor`'s own
                // treatment of the identical decode.
                let quarter = |value: i32, field: &str| {
                    u16::try_from(value)
                        .unwrap_or_else(|_| panic!("SetScissor {field} is a 12-bit field: {value}"))
                };
                delta.set_scissor(crate::targets::RdpScissorRect::from_wire_quarter_pixels(
                    decoded.mode,
                    quarter(decoded.ulx, "ulx"),
                    quarter(decoded.uly, "uly"),
                    quarter(decoded.lrx, "lrx"),
                    quarter(decoded.lry, "lry"),
                ));
                state.apply(delta);
                RawDpcCommandKind::SetScissor(decoded)
            }
            SET_PRIM_DEPTH => {
                let value = PrimDepth::from_wire(w1);
                delta.set_prim_depth(value);
                state.apply(delta);
                RawDpcCommandKind::SetPrimDepth(value)
            }
            SET_COMBINE => {
                // `RDP::setCombine` stores `combineL = combine & 0xFFFFFFFF`
                // (exactly `w0`, unmasked -- RT64 never strips its top
                // opcode byte) and `combineH = combine >> 32` (exactly
                // `w1`). `w0` here is the full 32-bit wire word this
                // decoder already extracted above, before the `& 0x3f`
                // opcode mask below it was ever applied to `opcode`.
                let value = CombineParams::from_wire(w0, w1);
                delta.set_combine(value);
                state.apply(delta);
                RawDpcCommandKind::SetCombine(value)
            }
            FILL_RECTANGLE => {
                let rectangle = FillRectangle {
                    upper_left_x: ((w1 >> 12) & 0x0fff) as u16,
                    upper_left_y: (w1 & 0x0fff) as u16,
                    lower_right_x: ((w0 >> 12) & 0x0fff) as u16,
                    lower_right_y: (w0 & 0x0fff) as u16,
                };
                // `commands.len()` is this command's own decode-order
                // index: `commands` accumulates across every stream in the
                // packet and this command is pushed at the bottom of this
                // same loop iteration, so the value read here is exactly
                // the index it will occupy. The span is recorded by
                // `plan_fill` itself, at the moment it pushes the accesses,
                // so the recorded run can never drift from the pushed one.
                let command_index = u32::try_from(commands.len()).map_err(|_| {
                    RawDpcDecodeError::ResourcePlanOverflow {
                        workload: location.workload,
                    }
                })?;
                plan_fill(
                    location,
                    command_index,
                    rectangle,
                    layout,
                    state,
                    planned,
                    fill_spans,
                    fill_seeds,
                )?;
                RawDpcCommandKind::FillRectangle(rectangle)
            }
            LOAD_SYNC | LOAD_TLUT | SET_TILE_SIZE | LOAD_BLOCK | LOAD_TILE | SET_TILE
            | SET_TEXTURE_IMAGE => {
                let first_access_index = u32::try_from(planned.len()).map_err(|_| {
                    RawDpcDecodeError::ResourcePlanOverflow {
                        workload: location.workload,
                    }
                })?;
                let first_operation = match planned.last() {
                    Some(access) => access.operation().get().checked_add(1).ok_or(
                        RawDpcDecodeError::ResourcePlanOverflow {
                            workload: location.workload,
                        },
                    )?,
                    None => 0,
                };
                let (command, accesses) = decode_tmem_command(
                    opcode,
                    w0,
                    w1,
                    layout,
                    state.tmem_mut(),
                    TmemSourcePlanStart::new(
                        tmem_source_identity,
                        first_access_index,
                        OperationId::new(first_operation),
                    ),
                )
                .map_err(|error| RawDpcDecodeError::InvalidCommand {
                    location,
                    reason: error.reason(),
                })?;
                if planned
                    .len()
                    .checked_add(accesses.len())
                    .is_none_or(|count| count > MAX_RESOURCE_ACCESSES)
                {
                    return Err(RawDpcDecodeError::InvalidCommand {
                        location,
                        reason: "TMEM load exceeds the bounded resource-plan access count",
                    });
                }
                planned.extend(accesses);
                delta.set_tmem(state.tmem().clone());
                match command {
                    TmemCommand::SetTextureImage(image) => {
                        RawDpcCommandKind::SetTextureImage(image)
                    }
                    TmemCommand::SetTile { tile, descriptor } => {
                        RawDpcCommandKind::SetTile { tile, descriptor }
                    }
                    TmemCommand::SetTileSize { tile, size } => {
                        RawDpcCommandKind::SetTileSize { tile, size }
                    }
                    TmemCommand::LoadSync(epoch) => RawDpcCommandKind::LoadSync(epoch),
                    TmemCommand::LoadBlock(load) => RawDpcCommandKind::LoadBlock(load),
                    TmemCommand::LoadTile(load) => RawDpcCommandKind::LoadTile(load),
                    TmemCommand::LoadTlut(load) => RawDpcCommandKind::LoadTlut(load),
                }
            }
            FULL_SYNC => {
                let Some(sync) = stream
                    .full_syncs
                    .iter()
                    .find(|sync| sync.stream_byte_offset == offset as u32)
                    .copied()
                else {
                    return Err(RawDpcDecodeError::InvalidCommand {
                        location,
                        reason: "FullSync lacks the matching capture observation",
                    });
                };
                if sync.chunk_index != location.chunk_index
                    || sync.source_address != location.source_byte_offset
                {
                    return Err(RawDpcDecodeError::InvalidCommand {
                        location,
                        reason: "FullSync capture observation has different chunk/source identity",
                    });
                }
                RawDpcCommandKind::FullSync(sync)
            }
            0x08..=0x0f => {
                // `command` is already sliced to exactly `width` bytes above,
                // and `width` came from `raw_rdp_command_width(opcode)`,
                // which the module-level test
                // `word_counts_match_raw_rdp_command_width_table` proves
                // equals `triangle::triangle_word_count` for every triangle
                // opcode -- so this length can never actually mismatch here.
                let triangle = triangle::RawTriangle::decode(opcode, command)
                    .expect("command slice length was already proven exact above");
                // Same span-recording contract as `FILL_RECTANGLE` and
                // `TEXTURE_RECTANGLE` above: `commands.len()` is this
                // command's own decode-order index, and the access run is
                // recorded by `plan_raw_triangle` at the moment it pushes,
                // so the span can never drift from the pushed accesses.
                let command_index = u32::try_from(commands.len()).map_err(|_| {
                    RawDpcDecodeError::ResourcePlanOverflow {
                        workload: location.workload,
                    }
                })?;
                plan_raw_triangle(
                    location,
                    command_index,
                    &triangle,
                    layout,
                    state,
                    planned,
                    fill_spans,
                )?;
                RawDpcCommandKind::RawTriangle(triangle)
            }
            TEXRECT | TEXRECT_FLIP => {
                // `command` is already sliced to exactly `width` bytes above,
                // and `width` came from `raw_rdp_command_width(opcode)`,
                // which returns 16 for both `TEXRECT`/`TEXRECT_FLIP`
                // (`fn64-render/src/rdp_completion.rs`'s
                // `RDP_TEXRECT | RDP_TEXRECTFLIP => 16` arm) -- exactly
                // `TEXTURE_RECTANGLE_COMMAND_BYTES`, so this length can
                // never actually mismatch here.
                let rectangle = texture_rectangle::RawTextureRectangle::decode(opcode, command)
                    .expect("command slice length was already proven exact above");
                // Same span-recording contract as `FILL_RECTANGLE` above:
                // `commands.len()` is this command's own decode-order index,
                // and the access run is recorded by `plan_render_target_rows`
                // at the moment it pushes, so the span can never drift from
                // the pushed accesses.
                let command_index = u32::try_from(commands.len()).map_err(|_| {
                    RawDpcDecodeError::ResourcePlanOverflow {
                        workload: location.workload,
                    }
                })?;
                plan_texture_rectangle(
                    location,
                    command_index,
                    rectangle,
                    layout,
                    state,
                    planned,
                    fill_spans,
                )?;
                RawDpcCommandKind::TextureRectangle(rectangle)
            }
            _ => {
                return Err(RawDpcDecodeError::UnsupportedCommand {
                    location,
                    decoded_opcode: opcode,
                    width,
                });
            }
        };
        commands.push(DecodedRawDpcCommand { location, kind });
        offset += width as usize;
    }
    Ok(())
}

fn plan_fill(
    location: RawDpcCommandLocation,
    command_index: u32,
    rectangle: FillRectangle,
    layout: fn64_render_ir::PhysicalMemoryLayout,
    state: &RdpState,
    planned: &mut Vec<ResourceAccess>,
    fill_spans: &mut Vec<FillAccessSpan>,
    fill_seeds: &mut Vec<FillSeedRead>,
) -> Result<(), RawDpcDecodeError> {
    // The cycle type selects how a rectangle's *content* is produced, not
    // whether the command is legal. Three independent authorities agree:
    //
    // - **fn64's own reference lane.** `fn64-render-reference`'s
    //   `raster/draw.rs:113-128` dispatches `G_FILLRECT` on cycle type and
    //   sends one/two-cycle rectangles to `draw_combined_fill_rectangle`
    //   (`draw.rs:223`), which runs them through the colour combiner with
    //   `shade`/`texel0`/`texel1` all zero. Only the `CycleType::Fill` arm
    //   uses the fill colour.
    //
    // - **RT64.** `RDP::fillRect` (`rt64_rdp.cpp:1033-1050`, pinned
    //   `5473732a`) reads `otherMode` only to decide the `lrx |= 3`/
    //   `lry |= 3` COPY/FILL rounding, then unconditionally calls
    //   `drawRect(ulx, uly, lrx, lry, 0, 0, 0, 0, false, extAlignment)`.
    //   The fill-colour *clear* is selected far downstream, at
    //   `rt64_framebuffer_renderer.cpp:1518-1519`, and only when
    //   `cycleType == G_CYC_FILL`; every other cycle type falls through to
    //   the ordinary raster shader. RT64 never refuses the command.
    //
    // - **The guest.** Measured on the real ROM at VI swap 2522
    //   (`docs/WM2000-FILLRECT-EVIDENCE.txt`): WM2000 stages four
    //   `SetOtherMode`s, all one-cycle, then issues 60 full-width
    //   `FillRectangle`s and **no `SetFillColor` at all**. It stages
    //   `SetCombine`/`SetPrimColor`/`SetEnvColor` instead. There is no
    //   fill colour because none is wanted; the rectangle's colour comes
    //   from the combiner.
    //
    // One- and two-cycle rectangles now run through the same combiner and
    // blender stages as texture rectangles, so they declare the bytes that
    // executor writes. The lower/right edge is exclusive in those modes;
    // applying that adjustment here as well as in the executor is required
    // because `fill_completed_writes` publishes every declared byte without
    // independently asking which pixels the raster touched.
    //
    // A `FillRectangle` before any `SetOtherMode` keeps its loud refusal:
    // there is no wire fact saying which content producer the guest asked
    // for. Copy cycle is also refused because it has no guaranteed public
    // result.
    let Some(other_mode) = state.other_mode() else {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle requires staged OtherMode",
        });
    };
    match other_mode.cycle_type() {
        CycleType::Copy => {
            return Err(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "G_FILLRECT in copy cycle has no guaranteed public result; use G_TEXRECT",
            })
        }
        CycleType::Fill | CycleType::OneCycle | CycleType::TwoCycle => {}
    }
    let Some(image) = state.color_image() else {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle requires staged SetColorImage",
        });
    };
    if other_mode.cycle_type() == CycleType::Fill && state.fill_color().is_none() {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle requires staged SetFillColor",
        });
    }
    if image.format() != ImageFormat::Rgba
        || !matches!(image.size(), PixelSize::Bits16 | PixelSize::Bits32)
    {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle supports only RGBA16 or RGBA32 color images",
        });
    }
    if [
        rectangle.upper_left_x,
        rectangle.upper_left_y,
        rectangle.lower_right_x,
        rectangle.lower_right_y,
    ]
    .iter()
    .any(|coordinate| coordinate & 0x3 != 0)
    {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle coordinates must be whole pixels in the bounded slice",
        });
    }
    let x0 = u32::from(rectangle.upper_left_x) >> 2;
    let y0 = u32::from(rectangle.upper_left_y) >> 2;
    let encoded_x1 = u32::from(rectangle.lower_right_x) >> 2;
    let encoded_y1 = u32::from(rectangle.lower_right_y) >> 2;
    if x0 > encoded_x1 || y0 > encoded_y1 {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle coordinates are reversed",
        });
    }
    let (x1, y1) = match other_mode.cycle_type() {
        CycleType::Fill => (encoded_x1, encoded_y1),
        CycleType::OneCycle | CycleType::TwoCycle => {
            let Some(x1) = encoded_x1.checked_sub(1) else {
                return Ok(());
            };
            let Some(y1) = encoded_y1.checked_sub(1) else {
                return Ok(());
            };
            if x0 > x1 || y0 > y1 {
                return Ok(());
            }
            (x1, y1)
        }
        CycleType::Copy => unreachable!("copy cycle was refused above"),
    };
    if x1 >= image.width() {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle exceeds the staged color-image width",
        });
    }
    // **The scissor narrows what this command WRITES, so it must narrow what
    // the journal DECLARES.**
    //
    // Pinned RT64 intersects its current scissor with the draw rectangle
    // (`src/hle/rt64_rdp.cpp:1214-1223`, commit `f0728a2`), so a scissored
    // fill never touches pixels outside that intersection, and the executor
    // (`targets::execute_fill_rectangle`, via `clip_fill_rectangle`) now
    // does the same.
    //
    // Declaring the UNCLIPPED extent while executing the clipped one is
    // precisely the hazard this function's own doc names sixty lines above:
    // `fill_completed_writes` slices the full-extent buffer for every
    // declared range *without checking the raster touched it*, so a
    // declared-but-unpainted row would publish a real digest of whatever the
    // buffer held there and carry it into guest RDRAM. Declaration and
    // execution must agree on the geometry, and the scissor is part of that
    // geometry.
    //
    // `None` means no `SetScissor` has been staged, in which case the
    // consumer's honest widest bound is the whole target -- the same
    // fallback `texrect_scissor_or_full_target` supplies on the execute
    // side. The clip below is then a no-op, so an unscissored stream
    // declares exactly what it declared before this change.
    let full_extent = RenderTargetRectangle { x0, y0, x1, y1 };
    let Some(rectangle) = clip_fill_rows_to_scissor(full_extent, state.scissor()) else {
        // Nothing survives the clip, so this command writes no guest byte
        // and the journal must declare none. Not an error: a fully
        // scissored-away rectangle is ordinary content (a sprite walked off
        // the clip window), and the executor names the same case loudly as
        // `FillExecutionError::ScissoredAway` only when it is asked to
        // execute one, which it now never is.
        return Ok(());
    };
    // **A partial fill declares a READ of the colour image it is patching
    // into, so the pixels it does not paint have a real value.**
    //
    // A fill that covers the whole target needs no seed: every byte comes
    // from the command. A partial one does, and the alternative is the
    // fabricated zeros `targets/mod.rs` used to refuse the draw over --
    // measured, not assumed: admitting partial fills without this read made
    // the differential's `top-left-quadrant` report `wgpu: 0x0000` where
    // both the reference and the hand-derived key say `0xffff`.
    //
    // The oracle does exactly this. `fn64-render-reference` seeds its
    // target from guest RDRAM before rendering every raw-RDP task
    // (`backend/imp.rs:440-447` calling `framebuffer_io.rs:12-44`), fills
    // strictly inside the clipped rect, and writes the whole extent back.
    // Its untouched pixels are the pre-existing guest bytes, which is what
    // hardware gives: an N64 colour image is RDRAM, and the bytes outside a
    // fill are whatever was already there.
    //
    // `TmemLoadSource` is the purpose because it is the one the deferred
    // guest-read plan selects on -- `DeferredGuestReadPlan::try_from_journal`
    // (`fn64-render-ir/src/guest_read.rs`) keys purely on
    // `purpose == TmemLoadSource` and is otherwise resource-agnostic, and
    // `ResourceAccess::try_new` explicitly admits
    // `RdramResource::ColorFramebuffer` for it. Naming it `UploadSource`,
    // which reads more honestly, would make the plan skip the read and hand
    // the executor nothing. The narrower name is the load-bearing one here;
    // `docs/rt64/RT64-FILL-PARTIAL-SEED.md` records the seam.
    //
    // **Pushed BEFORE the write span, never inside it.** `fill_accesses`
    // re-checks that every access in a fill's recorded span is
    // `Write`/`RenderTarget`, and `plan_render_target_rows` records the span
    // starting at the next index, so a read pushed here stays outside it.
    // **Full extent of the TARGET, not "unchanged by the scissor clip".**
    //
    // The question a seed answers is "are there pixels of this colour image
    // that this command will not write", and that is a comparison against
    // the image, not against the pre-clip rectangle. Comparing to
    // `full_extent` instead was measured wrong: the differential's
    // `top-left-quadrant`, `single-pixel` and `last-column-last-row` cases
    // are unscissored, so the clip is a no-op and `rectangle == full_extent`
    // held for every one of them -- while each covers a small corner of an
    // 8x4 target and needs a seed badly.
    //
    // **The height is the host-configured one, and its absence is not an
    // error here.** `SetColorImage` carries no height (see
    // `RdpState::color_target_height`), and plenty of decode-only paths
    // never configure one. A fill that spans the full image width and
    // starts at row 0 with no known height is treated as covering the
    // target -- the same thing this planner assumed before seeds existed,
    // so those paths decode exactly as they did. Making the height
    // mandatory instead broke twenty-odd decode tests that legitimately
    // have none, which is how this arm got written.
    //
    // **A zero height declares no seed.** A degenerate target holds no
    // pixels to seed from, and the honest refusal for a fill against one is
    // the named downstream rejection the executor already produces
    // (`resize_to_zero_is_recorded_and_rejected_by_name_at_the_fill`), not
    // "color image lies outside installed RDRAM" raised while sizing a
    // zero-pixel read. Preempting it here replaced a specific diagnosis
    // with a misleading one, which is how this arm was found.
    let covers_target = state
        .color_target_height()
        .is_some_and(|height| height == 0)
        || (rectangle.x0 == 0
            && rectangle.y0 == 0
            && rectangle.x1 + 1 == image.width()
            && state
                .color_target_height()
                .is_none_or(|height| rectangle.y1 + 1 == height));
    let seed = if covers_target {
        None
    } else {
        // A seed is only reachable when the fill does NOT cover the target,
        // which above requires a known height whenever the rectangle is
        // otherwise full-image -- but a partial-width fill can reach here
        // with none, and the seed must still be sized. The colour image's
        // own last covered row is the honest bound in that case.
        let height = state.color_target_height().unwrap_or(rectangle.y1 + 1);
        let pixels =
            image
                .width()
                .checked_mul(height)
                .ok_or(RawDpcDecodeError::InvalidCommand {
                    location,
                    reason: "color image pixel count overflows",
                })?;
        let bytes_per_pixel =
            image
                .size()
                .bytes_per_pixel()
                .ok_or(RawDpcDecodeError::InvalidCommand {
                    location,
                    reason: "color image size has no byte width",
                })?;
        let start = image.address().get();
        let end = pixels
            .checked_mul(bytes_per_pixel)
            .and_then(|bytes| start.checked_add(bytes))
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "color image byte range overflows",
            })?;
        let range = layout
            .range(start, end)
            .map_err(|_| RawDpcDecodeError::InvalidCommand {
                location,
                reason: "color image lies outside installed RDRAM",
            })?;
        let index =
            u32::try_from(planned.len()).map_err(|_| RawDpcDecodeError::ResourcePlanOverflow {
                workload: location.workload,
            })?;
        push_access(
            location.workload,
            planned,
            AccessMode::Read,
            AccessPurpose::TmemLoadSource,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range,
            },
        )?;
        Some(index)
    };
    fill_seeds.push(FillSeedRead {
        command_index,
        access_index: seed,
    });
    plan_render_target_rows(
        location,
        command_index,
        rectangle,
        image,
        layout,
        planned,
        fill_spans,
    )
}

/// Which declared guest read carries the colour-image seed for one admitted
/// `FillRectangle`, or `None` when the fill covers the whole target and
/// needs no seed.
///
/// Recorded per command rather than inferred at execute time: the executor
/// must be able to tell "this fill declared no seed because it is
/// full-extent" from "this fill declared a seed that failed to thread",
/// and only the decoder knows which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillSeedRead {
    command_index: u32,
    access_index: Option<u32>,
}

/// Intersects one fill's whole-pixel destination rectangle with the staged
/// scissor, in the scissor's own quarter-pixel domain, returning `None` when
/// nothing survives.
///
/// Uses [`crate::targets::RdpScissorRect`]'s own pixel accessors rather than
/// re-deriving the quarter-pixel rounding, so the decoder and the executor
/// round the scissor identically by construction instead of by two
/// agreeing-looking copies of `div_ceil`. The `ceil(q / 4)` rule is fn64's
/// own reading and is not independently confirmed against an allowed
/// hardware reference.
///
/// The target-extent bound is deliberately NOT applied here: `plan_fill`
/// has already rejected `x1 >= image.width()` loudly, and the row planner
/// derives its own ranges from the image, so adding a second extent clamp
/// would silently admit a rectangle the width check exists to refuse.
fn clip_fill_rows_to_scissor(
    rectangle: RenderTargetRectangle,
    scissor: Option<crate::targets::RdpScissorRect>,
) -> Option<RenderTargetRectangle> {
    let Some(scissor) = scissor else {
        return Some(rectangle);
    };
    let RenderTargetRectangle { x0, y0, x1, y1 } = rectangle;
    // Half-open intersection, then back to this struct's inclusive edges.
    let first_x = x0.max(scissor.first_column());
    let limit_x = (x1 + 1).min(scissor.column_limit());
    let first_y = y0.max(scissor.first_row());
    let limit_y = (y1 + 1).min(scissor.row_limit());
    if first_x >= limit_x || first_y >= limit_y {
        return None;
    }
    Some(RenderTargetRectangle {
        x0: first_x,
        y0: first_y,
        x1: limit_x - 1,
        y1: limit_y - 1,
    })
}

/// Declares the exact ordered `ColorFramebuffer` write accesses one admitted
/// `TextureRectangle` covers, so the journal carries a write for the
/// composition layer to order a texrect against -- the gap that previously
/// made a texrect indistinguishable from a triangle (which declares no write
/// at all) and forced `MixedFillAndTrianglePacket`.
///
/// **Destination geometry only.** Nothing here reads TMEM, samples a texel,
/// or claims what the rectangle's *content* will be. The access run is
/// derived from the rasterized pixel extent and the staged `SetColorImage`
/// exactly as `plan_fill`'s is, through the same [`plan_render_target_rows`].
///
/// The extent is read off [`texture_rectangle::texture_rectangle_vertices`]
/// -- this crate's ported RT64 `drawTexRect`/`drawRect` -- rather than
/// re-derived from the wire fields, because copy-mode `lrx |= 3`/`lry |= 3`
/// mutation and fill/copy UL rounding live inside that function. A second
/// derivation of the same rectangle is exactly the drift
/// `ExactRawDpcPlanWriter::finish`'s access-for-access check exists to catch.
///
/// # Declaring nothing is not a silent no-op
///
/// Several conditions below return without pushing an access. That is a
/// declaration that this command writes *no* `ColorFramebuffer` range the
/// journal can name -- not a suppressed error, and not a skipped draw. The
/// command is still decoded, still admitted, and still rasters through the
/// triangle path exactly as it did before this planner existed; only the
/// journal entry is absent, which is precisely the pre-existing behavior for
/// every texrect. The conditions:
///
/// - **No staged `SetColorImage`**, or one outside the RGBA16/RGBA32 subset
///   `plan_fill` admits: there is no destination image, so there is no range
///   to name. This crate's own `tmem_then_texrect_words` fixture is such a
///   stream, and it decoded before this planner existed.
/// - **No staged `OtherMode`**: `texture_rectangle_vertices` reads
///   `cycle_type` to decide copy-mode mutation and UL rounding, so the
///   geometry is undefined. The production adapter already refuses this case
///   by name (`TextureRectangleBeforeAnyOtherMode`) -- refusing it a second
///   time here, at a different layer and with a different error, would just
///   move which message a reader sees.
/// - **Fractional edges, a degenerate/empty extent, a negative origin, or a
///   rectangle wider than the staged image**: the exact covered range is not
///   provable here, and a clamped or rounded guess would declare bytes the
///   RDP never covers.
///
/// A refusal that *must* be loud belongs at the executor, which knows it was
/// asked to produce content and cannot -- not at the journal, which would be
/// refusing streams that decode correctly today.
fn plan_texture_rectangle(
    location: RawDpcCommandLocation,
    command_index: u32,
    rectangle: texture_rectangle::RawTextureRectangle,
    layout: fn64_render_ir::PhysicalMemoryLayout,
    state: &RdpState,
    planned: &mut Vec<ResourceAccess>,
    fill_spans: &mut Vec<FillAccessSpan>,
) -> Result<(), RawDpcDecodeError> {
    let Some(other_mode) = state.other_mode() else {
        return Ok(());
    };
    // Deliberately NOT gated on cycle type. The destination footprint this
    // planner declares is a function of the wire coordinates and the staged
    // `SetColorImage` only; the cycle type selects how the *content* is
    // produced (Copy blits the texel, one/two-cycle runs it through the color
    // combiner), which is the executor's concern, not the journal's. Gating
    // here on Copy/Fill was tried and reverted: it would have refused the
    // one-cycle texrects this crate's own fixtures carry, and no measurement
    // in this repo establishes which mode WM2000's title-screen texrects use.
    //
    // `other_mode` is still required to be staged, because
    // `texture_rectangle_vertices` reads `cycle_type` to decide copy-mode
    // `dsdx`/`lrx`/`lry` mutation and fill/copy UL rounding -- a texrect
    // before any `SetOtherMode` has no defined geometry, and the production
    // adapter already refuses it by name (`TextureRectangleBeforeAnyOtherMode`).
    let cycle_type = other_mode.cycle_type();
    // No staged `SetColorImage` means this texrect has no destination image
    // to write into, so it declares no write access and the stream still
    // decodes. This is NOT a silent no-op: the command is still admitted and
    // still rasters through the triangle path exactly as before this planner
    // existed; there is simply no `ColorFramebuffer` range to declare. A
    // decode refusal here would regress every already-decodable TMEM-then-
    // texrect stream that never staged a color image (this crate's own
    // `tmem_then_texrect_words` fixture is one).
    let Some(image) = state.color_image() else {
        return Ok(());
    };
    // Same narrow color-image subset `plan_fill` admits. An unsupported
    // format declares no write rather than failing the decode, for the same
    // reason as above -- the executor, not the journal, is where an
    // unexecutable texrect must be refused by name.
    if image.format() != ImageFormat::Rgba
        || !matches!(image.size(), PixelSize::Bits16 | PixelSize::Bits32)
    {
        return Ok(());
    }
    if [
        rectangle.ulx(),
        rectangle.uly(),
        rectangle.lrx(),
        rectangle.lry(),
    ]
    .iter()
    .any(|coordinate| coordinate & 0x3 != 0)
    {
        return Ok(());
    }
    // The destination footprint is read off the SAME geometry the executor
    // will raster -- `texture_rectangle_vertices`, which is this crate's
    // ported RT64 `drawTexRect`/`drawRect` -- never re-derived here. A second
    // independent derivation of the rectangle is exactly the drift
    // `ExactRawDpcPlanWriter::finish`'s access-for-access check exists to
    // catch, and the copy-mode `lrx |= 3`/`lry |= 3` mutation and fill/copy UL
    // rounding live inside that function, not on the wire fields.
    //
    // `None` is RT64's own `FixedRect::isEmpty()` early return (a reversed or
    // zero-area rectangle draws nothing); it is a named refusal here rather
    // than a silently-declared empty write run.
    let Some(vertices) = texture_rectangle::texture_rectangle_vertices(rectangle, cycle_type)
    else {
        return Ok(());
    };
    let viewport = vertices.viewport;
    // `RectViewportPixels` is RT64's own `left`/`top`/`right`/`bottom` pixel
    // extent, half-open at right/bottom, in signed pixels. A negative origin
    // would be a scissored rectangle this slice does not clip, so it is
    // refused by name rather than saturated to zero -- clamping would declare
    // a write run for pixels the RDP never covers.
    if viewport.left < 0 || viewport.top < 0 {
        return Ok(());
    }
    if viewport.right <= viewport.left || viewport.bottom <= viewport.top {
        return Ok(());
    }
    let x0 = viewport.left as u32;
    let y0 = viewport.top as u32;
    let x1 = viewport.right as u32 - 1;
    let y1 = viewport.bottom as u32 - 1;
    if x1 >= image.width() {
        return Ok(());
    }
    plan_render_target_rows(
        location,
        command_index,
        RenderTargetRectangle { x0, y0, x1, y1 },
        image,
        layout,
        planned,
        fill_spans,
    )
}

/// The raw triangles this backend can produce guest bytes for, and
/// therefore the only ones it declares a write for: **opaque, with no depth
/// plane** -- shaded or not, textured or not.
///
/// Shaded was admitted after the executor gained per-pixel shade plane
/// interpolation; TEXTURED after it gained per-pixel S/T/W plane
/// interpolation, the perspective divide and the TMEM fetch -- each time in
/// that order. Widening this predicate first would declare rows the executor
/// cannot fill.
///
/// Admitting the texture bit is what makes WM2000's geometry reachable at
/// all: every one of the 1,314,648 raw triangles measured on the real ROM is
/// opcode 0x0e, shaded AND textured, and this predicate refused all of them.
///
/// Depth (bit 0) stays out. It is not a rung this predicate can widen alone:
/// it needs a depth image, its own journal declaration and the RDP's Z
/// encoding, none of which exist here.
///
/// This is an *admission*, not an approximation. A triangle outside the
/// subset declares nothing and behaves exactly as it did before this
/// planner existed -- it still decodes, still pushes its command, still
/// reaches the GPU triangle path. Declaring a write the CPU executor cannot
/// fill would be strictly worse than declaring none: `fill_completed_writes`
/// slices the full-extent buffer for every declared range without checking
/// the raster touched it, so a declared-but-undrawn row yields a real digest
/// of STALE bytes that passes `validate_effects` and reaches guest RDRAM.
/// Convincing garbage beats a loud error nowhere.
///
/// The subset widens by widening the executor first, then this predicate --
/// never the other way round.
fn raw_triangle_is_executable(triangle: &triangle::RawTriangle) -> bool {
    !triangle.flags().depth()
}

/// **Diagnostic-only.** Counts, per named reason, every raw triangle
/// `plan_raw_triangle` declines to declare a write for -- the eight
/// `return Ok(())` arms that are silent by design. Nothing in the render
/// path reads these; they exist so a real ROM run can say WHICH silent
/// arm a frozen frame is falling into, instead of inferring it.
///
/// Dumped to stderr at process exit when `FN64_TRI_DROP_STATS` is set.
pub mod raw_triangle_drop_stats {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The named arms of `plan_raw_triangle`, in source order.
    pub const REASONS: [&str; 9] = [
        "depth_bit_set",
        "no_other_mode",
        "fill_cycle",
        "no_color_image",
        "color_image_format",
        "no_target_height",
        "no_covered_rows",
        "row_outside_rdram",
        "ADMITTED",
    ];

    static COUNTS: [AtomicU64; 9] = [
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ];

    static TICKS: AtomicU64 = AtomicU64::new(0);

    pub(super) fn bump(index: usize) {
        COUNTS[index].fetch_add(1, Ordering::Relaxed);
        // Periodic self-report: the harness is a separate crate that does
        // not call into this module, so the dump has to come from here.
        // Every 100k decisions is ~1 line per few VI swaps on the real ROM.
        let tick = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
        if tick % 100_000 == 0 && crate::diag_env::diag_env_present("FN64_TRI_DROP_STATS") {
            report(&format!("tick={tick}"));
        }
    }

    /// Current counts, in `REASONS` order.
    pub fn snapshot() -> [u64; 9] {
        let mut out = [0u64; 9];
        for (slot, counter) in out.iter_mut().zip(COUNTS.iter()) {
            *slot = counter.load(Ordering::Relaxed);
        }
        out
    }

    /// Admitted-triangle destination addresses: the `SetColorImage` address
    /// each admitted raw triangle declares its rows against, with a count.
    /// This is the measurement that distinguishes "triangles are drawn but
    /// into a buffer nobody scans out" from "triangles are drawn into the
    /// scanned-out buffer and something later discards them".
    static ADDRESSES: std::sync::Mutex<Option<std::collections::BTreeMap<u32, u64>>> =
        std::sync::Mutex::new(None);

    /// Whether each ADMITTED triangle carried the wire opcode's texture bit.
    ///
    /// Split out from `COUNTS` rather than added as two more `REASONS`
    /// because these are not drop reasons: every triangle counted here was
    /// admitted, and the two buckets sum to the `ADMITTED` count rather than
    /// partitioning the same total the drop reasons partition. Folding them
    /// into `REASONS` would make `total` double-count every admitted
    /// triangle and silently corrupt the percentage the whole report exists
    /// to state.
    ///
    /// This is the measurement named as the cheap next step in
    /// `docs/rt64/RT64-WM2000-INMATCH-GAPS.md` once admission was shown not to be
    /// the cause of flat-shaded models: it distinguishes "textured triangles
    /// are drawn but sample wrongly" from "the game issues untextured
    /// triangles here", which are different investigations and were not
    /// separable from the drop counters alone.
    static ADMITTED_TEXTURED: AtomicU64 = AtomicU64::new(0);
    static ADMITTED_UNTEXTURED: AtomicU64 = AtomicU64::new(0);

    pub(super) fn note_textured(textured: bool) {
        if textured {
            &ADMITTED_TEXTURED
        } else {
            &ADMITTED_UNTEXTURED
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    /// The two admitted-triangle texture-bit counts, `(textured, untextured)`.
    pub fn textured_snapshot() -> (u64, u64) {
        (
            ADMITTED_TEXTURED.load(Ordering::Relaxed),
            ADMITTED_UNTEXTURED.load(Ordering::Relaxed),
        )
    }

    pub(super) fn note_address(address: u32) {
        let mut guard = ADDRESSES.lock().expect("address histogram poisoned");
        guard
            .get_or_insert_with(std::collections::BTreeMap::new)
            .entry(address)
            .and_modify(|count| *count += 1)
            .or_insert(1);
    }

    /// Prints the snapshot to stderr. Called by the harness, or by a test.
    pub fn report(tag: &str) {
        let counts = snapshot();
        let total: u64 = counts.iter().sum();
        eprintln!("[fn64-tri-drop] {tag} total={total}");
        for (name, count) in REASONS.iter().zip(counts.iter()) {
            if *count > 0 {
                eprintln!("[fn64-tri-drop]   {name} = {count}");
            }
        }
        // Reported separately and NEVER summed into `total`: these two
        // partition the ADMITTED bucket, not the whole decision stream.
        let (textured, untextured) = textured_snapshot();
        if textured > 0 || untextured > 0 {
            eprintln!(
                "[fn64-tri-drop]   of ADMITTED: textured = {textured}, untextured = {untextured}"
            );
        }
        if let Some(map) = ADDRESSES
            .lock()
            .expect("address histogram poisoned")
            .as_ref()
        {
            for (address, count) in map.iter() {
                eprintln!("[fn64-tri-drop]   admitted_target {address:#010x} = {count}");
            }
        }
    }
}

/// Declares the exact ordered `ColorFramebuffer` write accesses one admitted
/// raw triangle covers -- **one access per covered scanline** -- and records
/// the run as a [`FillAccessSpan`].
///
/// Not routed through [`plan_render_target_rows`], which takes a single
/// rectangle: a triangle's covered X range differs per scanline, so its rows
/// are N genuinely different ranges rather than N copies of one. Feeding it
/// the triangle's bounding box instead would declare, for every scanline,
/// every pixel between the leftmost and rightmost the triangle reaches
/// anywhere -- the exact over-declaration `plan_render_target_rows`' own
/// per-row rule exists to prevent, one level up.
///
/// **The row list is `triangle_span::covered_rows`, and so is the
/// rasterizer's.** That shared call is what makes the declaration honest:
/// there is no row here that the raster will skip and no row the raster
/// visits that is not here.
///
/// Declares nothing (and does not fail) when the triangle is outside the
/// admitted subset, when no compatible `SetColorImage` is staged, or when
/// any covered row would fall outside installed RDRAM -- the same
/// "decode succeeds, journal stays silent" shape `plan_texture_rectangle`
/// uses, because a triangle that declares no write is exactly today's
/// behaviour and must stay decodable.
fn plan_raw_triangle(
    location: RawDpcCommandLocation,
    command_index: u32,
    triangle: &triangle::RawTriangle,
    layout: fn64_render_ir::PhysicalMemoryLayout,
    state: &RdpState,
    planned: &mut Vec<ResourceAccess>,
    fill_spans: &mut Vec<FillAccessSpan>,
) -> Result<(), RawDpcDecodeError> {
    if !raw_triangle_is_executable(triangle) {
        raw_triangle_drop_stats::bump(0);
        return Ok(());
    }
    // A triangle before any `SetOtherMode` has no defined blend or cycle
    // state and the production adapter already refuses it by name
    // (`TriangleBeforeAnyOtherMode`); declaring a write for one here would
    // declare a write for a command that never executes.
    let Some(other_mode) = state.other_mode() else {
        raw_triangle_drop_stats::bump(1);
        return Ok(());
    };
    // Fill cycle consults no combiner and no blender at all -- the RDP fills
    // with the fill colour through a path this executor does not implement
    // for triangles. Refused by declaring nothing rather than drawn as an
    // approximation.
    if matches!(other_mode.cycle_type(), CycleType::Fill) {
        raw_triangle_drop_stats::bump(2);
        return Ok(());
    }
    let Some(image) = state.color_image() else {
        raw_triangle_drop_stats::bump(3);
        return Ok(());
    };
    // The same narrow colour-image subset `plan_fill` and
    // `plan_texture_rectangle` admit.
    if image.format() != ImageFormat::Rgba
        || !matches!(image.size(), PixelSize::Bits16 | PixelSize::Bits32)
    {
        raw_triangle_drop_stats::bump(4);
        return Ok(());
    }
    let bytes_per_pixel = image
        .size()
        .bytes_per_pixel()
        .expect("RGBA16/32 are byte-addressed");
    // **Height is bounded by installed RDRAM, not by a target extent.**
    // `SetColorImage` carries a width and no height, and the executor's own
    // extent (`configured_target_extent`) does not exist at decode time. So
    // the honest decode-time bound is "every row whose bytes are inside
    // installed RDRAM", derived below by `layout.range` refusing the first
    // row that is not. `MAX_RAW_TRIANGLE_ROWS` caps the walk itself so a
    // wildly out-of-range YL cannot make this loop unbounded.
    // **The row walk is bounded by the HOST-CONFIGURED target height**, the
    // same value `configured_target_extent` gives the executor. Without it
    // the only bound is installed RDRAM -- 4MB, hundreds of times the real
    // target -- and a triangle whose YL reaches past the last row declares
    // byte ranges outside the target, which `verify_accesses_inside` refuses
    // for the WHOLE PACKET rather than for the one triangle.
    //
    // Measured on the real ROM: WM2000 aborted after 280 VI swaps naming
    // "FillRectangle access #59". The defect predates the texture rung; it
    // was unreachable only because the decoder refused every triangle the
    // ROM emits.
    //
    // `None` (no `create` yet) declares nothing rather than guessing, the
    // same shape every other missing precondition above takes.
    let Some(height) = state.color_target_height() else {
        raw_triangle_drop_stats::bump(5);
        return Ok(());
    };
    let rows = triangle_span::covered_rows(triangle, image.width(), height);
    if rows.is_empty() {
        raw_triangle_drop_stats::bump(6);
        return Ok(());
    }
    if planned
        .len()
        .checked_add(rows.len())
        .is_none_or(|accesses| accesses > MAX_RESOURCE_ACCESSES)
    {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "raw triangle exceeds the bounded resource-plan access count",
        });
    }
    // Every row's range is derived and validated BEFORE any is pushed, so a
    // triangle whose last row leaves installed RDRAM declares nothing at all
    // rather than a truncated prefix that the rasterizer would then overrun.
    let mut ranges = Vec::with_capacity(rows.len());
    for row in &rows {
        let Some(range) = row
            .y
            .checked_mul(image.width())
            .and_then(|pixel| pixel.checked_add(row.x0))
            .and_then(|pixel| pixel.checked_mul(bytes_per_pixel))
            .and_then(|offset| image.address().get().checked_add(offset))
            .and_then(|start| {
                let bytes = (row.x1 - row.x0).checked_mul(bytes_per_pixel)?;
                let end = start.checked_add(bytes)?;
                layout.range(start, end).ok()
            })
        else {
            raw_triangle_drop_stats::bump(7);
            return Ok(());
        };
        ranges.push(range);
    }
    let first_access_index =
        u32::try_from(planned.len()).map_err(|_| RawDpcDecodeError::ResourcePlanOverflow {
            workload: location.workload,
        })?;
    for range in ranges {
        push_access(
            location.workload,
            planned,
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range,
            },
        )?;
    }
    let count = u32::try_from(planned.len() - first_access_index as usize).map_err(|_| {
        RawDpcDecodeError::ResourcePlanOverflow {
            workload: location.workload,
        }
    })?;
    debug_assert_eq!(
        count as usize,
        rows.len(),
        "plan_raw_triangle pushed a different number of accesses than it planned"
    );
    fill_spans.push(FillAccessSpan {
        command_index,
        first_access_index,
        count,
    });
    raw_triangle_drop_stats::note_address(image.address().get());
    // The wire opcode's own texture bit, read from the same `flags()` the
    // executor's binding equality check reads (`raw_triangle.rs:158`), at the
    // moment of admission -- so the count is over exactly the triangles that
    // went on to draw.
    raw_triangle_drop_stats::note_textured(triangle.flags().textured());
    raw_triangle_drop_stats::bump(8);
    Ok(())
}

/// One admitted command's whole-pixel destination rectangle in the staged
/// color image, already validated as forward-ordered and inside the image
/// width by its own caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RenderTargetRectangle {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

/// Declares the exact ordered `ColorFramebuffer` write accesses one admitted
/// destination rectangle covers, and records the run as a [`FillAccessSpan`].
///
/// Shared verbatim by `plan_fill` and `plan_texture_rectangle`: both commands
/// write the same resource through the same geometry, so a second copy of
/// this arithmetic would be a second, independent model of one fact -- the
/// divergence `ExactRawDpcPlanWriter::finish`'s access-for-access check
/// exists to catch. `reason` strings stay generic across both callers for
/// that reason; the command that failed is already named by `location`.
///
/// A full-image-width rectangle collapses to a single contiguous access; a
/// partial-width one declares N disjoint per-row ranges strided by the image
/// width, because collapsing those would declare untouched inter-row bytes as
/// written.
fn plan_render_target_rows(
    location: RawDpcCommandLocation,
    command_index: u32,
    rectangle: RenderTargetRectangle,
    image: ColorImage,
    layout: fn64_render_ir::PhysicalMemoryLayout,
    planned: &mut Vec<ResourceAccess>,
    fill_spans: &mut Vec<FillAccessSpan>,
) -> Result<(), RawDpcDecodeError> {
    let RenderTargetRectangle { x0, y0, x1, y1 } = rectangle;
    let bytes_per_pixel = image
        .size()
        .bytes_per_pixel()
        .expect("RGBA16/32 are byte-addressed");
    let row_bytes =
        (x1 - x0 + 1)
            .checked_mul(bytes_per_pixel)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "destination rectangle row byte count overflows",
            })?;
    let planned_rows = if x0 == 0 && x1 + 1 == image.width() {
        1
    } else {
        usize::try_from(y1 - y0 + 1).expect("12-bit coordinates fit usize")
    };
    if planned
        .len()
        .checked_add(planned_rows)
        .is_none_or(|accesses| accesses > MAX_RESOURCE_ACCESSES)
    {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "destination rectangle exceeds the bounded resource-plan access count",
        });
    }
    // Recorded before the push loop below and paired with `planned_rows`
    // after it, so the span this decoder publishes is derived from the same
    // loop that pushes the accesses -- never from a second, independent
    // derivation of the same math.
    let first_access_index =
        u32::try_from(planned.len()).map_err(|_| RawDpcDecodeError::ResourcePlanOverflow {
            workload: location.workload,
        })?;
    let rows: Box<dyn Iterator<Item = (u32, u32)>> = if planned_rows == 1 {
        Box::new(core::iter::once((y0, y1)))
    } else {
        Box::new((y0..=y1).map(|y| (y, y)))
    };
    for (first_y, last_y) in rows {
        let pixel = first_y
            .checked_mul(image.width())
            .and_then(|value| value.checked_add(x0))
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "destination rectangle pixel offset overflows",
            })?;
        let start = image
            .address()
            .get()
            .checked_add(pixel.checked_mul(bytes_per_pixel).ok_or(
                RawDpcDecodeError::InvalidCommand {
                    location,
                    reason: "destination rectangle byte offset overflows",
                },
            )?)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "destination rectangle address overflows",
            })?;
        let rows = last_y - first_y + 1;
        let bytes = row_bytes
            .checked_mul(rows)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "destination rectangle byte count overflows",
            })?;
        let end = start
            .checked_add(bytes)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "destination rectangle range end overflows",
            })?;
        let range = layout
            .range(start, end)
            .map_err(|_| RawDpcDecodeError::InvalidCommand {
                location,
                reason: "destination rectangle writes outside installed RDRAM",
            })?;
        push_access(
            location.workload,
            planned,
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range,
            },
        )?;
    }
    let count = u32::try_from(planned.len() - first_access_index as usize).map_err(|_| {
        RawDpcDecodeError::ResourcePlanOverflow {
            workload: location.workload,
        }
    })?;
    debug_assert_eq!(
        count as usize, planned_rows,
        "plan_render_target_rows pushed a different number of accesses than it planned"
    );
    fill_spans.push(FillAccessSpan {
        command_index,
        first_access_index,
        count,
    });
    Ok(())
}

fn push_access(
    workload: WorkloadIdentity,
    accesses: &mut Vec<ResourceAccess>,
    mode: AccessMode,
    purpose: AccessPurpose,
    region: ResourceRegion,
) -> Result<(), RawDpcDecodeError> {
    let id = u32::try_from(accesses.len())
        .map_err(|_| RawDpcDecodeError::ResourcePlanOverflow { workload })?;
    accesses.push(ResourceAccess::try_new(
        OperationId::new(id),
        mode,
        purpose,
        region,
    )?);
    Ok(())
}

#[cfg(test)]
mod tests;
