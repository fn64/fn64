//! Bounded decoding for the first admitted raw-DPC command subset.

mod production_adapter;
mod texture_rectangle;
mod triangle;
#[cfg(test)]
mod triangle_composition;
mod triangle_draw_data;
pub(crate) mod triangle_span;
mod triangle_vertices;

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
    neutral_vertex_to_raster_vertex, retrieve_triangle_draws, MissingTriangleDrawState,
    RetrievedTriangleDraw, TriangleDrawStateCollector,
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
    PrimDepth, RdpState, RdpStateDelta, StagedRdpState,
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
const SET_FILL_COLOR: u8 = 0x37;
const FILL_RECTANGLE: u8 = 0x36;
const FULL_SYNC: u8 = 0x29;
/// `G_RDPPIPESYNC` (`0xe7`, `crates/fn64-render-reference/src/gbi/wire.rs`'s
/// `G_RDPPIPESYNC`); public SGI *RDP Command Summary* "Sync Pipe". WM2000's
/// most-issued rejected command: 20,800 occurrences, 14.6% of its whole
/// stream (`docs/RT64-WM2000-CENSUS.md` §3).
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
    /// clipped. `execute_texture_rectangle` now clips against this rect the
    /// way the RDP does (angrylion `rasterizer.c:2349-2363` for X,
    /// `:2284-2305` for Y), so the value has to survive into durable state:
    /// a display list commonly sets the scissor once per frame and then
    /// submits several packets under it.
    SetScissor(crate::rt64_gbi_rdp_decode::SetScissorDecoded),
    SetColorImage(ColorImage),
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillAccessSpanError {
    FillNotDeclared { command_index: u32 },
    AccessSliceOutOfBounds { command_index: u32 },
    AccessDescriptorsDiffer { command_index: u32 },
}

impl fmt::Display for FillAccessSpanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FillNotDeclared { command_index } => write!(
                formatter,
                "no FillRectangle access span was declared for command #{command_index}"
            ),
            Self::AccessSliceOutOfBounds { command_index } => write!(
                formatter,
                "FillRectangle command #{command_index}'s access span is out of bounds"
            ),
            Self::AccessDescriptorsDiffer { command_index } => write!(
                formatter,
                "FillRectangle command #{command_index}'s access span is not a run of \
                 RenderTarget color-framebuffer writes"
            ),
        }
    }
}

impl std::error::Error for FillAccessSpanError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TmemLoadSourcePlanError {
    DecodeIdentityMismatch,
    AccessSliceOutOfBounds,
    AccessDescriptorsDiffer,
    TransferNotDeclared,
    DestinationDecodeIdentityMismatch,
    DestinationAccessSliceOutOfBounds,
    DestinationAccessDescriptorsDiffer,
    YuvExecutionDeferred,
}

impl fmt::Display for TmemLoadSourcePlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DecodeIdentityMismatch => {
                formatter.write_str("TMEM source plan belongs to a different decode")
            }
            Self::AccessSliceOutOfBounds => {
                formatter.write_str("TMEM source plan access slice is out of bounds")
            }
            Self::AccessDescriptorsDiffer => formatter
                .write_str("TMEM source plan differs from the exact ordered source descriptors"),
            Self::TransferNotDeclared => {
                formatter.write_str("TMEM transfer plan is not declared by this decode")
            }
            Self::DestinationDecodeIdentityMismatch => formatter
                .write_str("TMEM destination plan belongs to a different source/decode identity"),
            Self::DestinationAccessSliceOutOfBounds => {
                formatter.write_str("TMEM destination plan access slice is out of bounds")
            }
            Self::DestinationAccessDescriptorsDiffer => formatter.write_str(
                "TMEM destination plan differs from the canonical sorted destination union",
            ),
            Self::YuvExecutionDeferred => formatter.write_str(
                "YUV destination execution is deferred pending a public pairing contract",
            ),
        }
    }
}

impl std::error::Error for TmemLoadSourcePlanError {}

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

#[derive(Debug, PartialEq, Eq)]
pub enum RawDpcDecodeError {
    UnsupportedAdmission {
        workload: WorkloadIdentity,
    },
    StagedStateMismatch {
        workload: WorkloadIdentity,
        reason: &'static str,
    },
    UnknownCommandWidth {
        location: RawDpcCommandLocation,
    },
    TruncatedCommand {
        location: RawDpcCommandLocation,
        width: u32,
        available: u32,
    },
    UnsupportedCommand {
        location: RawDpcCommandLocation,
        decoded_opcode: u8,
        width: u32,
    },
    InvalidCommand {
        location: RawDpcCommandLocation,
        reason: &'static str,
    },
    ResourcePlanOverflow {
        workload: WorkloadIdentity,
    },
    JournalMismatch {
        workload: WorkloadIdentity,
        expected: Box<[ResourceAccess]>,
        actual: Box<[ResourceAccess]>,
    },
    Ir(ValidationError),
}

impl fmt::Display for RawDpcDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedAdmission { workload } => {
                write!(formatter, "workload {workload} is not admitted as raw DPC")
            }
            Self::StagedStateMismatch { workload, reason } => {
                write!(
                    formatter,
                    "workload {workload} cannot chain staged RDP state: {reason}"
                )
            }
            Self::UnknownCommandWidth { location } => {
                write!(
                    formatter,
                    "{location}: command has no admitted public width"
                )
            }
            Self::TruncatedCommand {
                location,
                width,
                available,
            } => write!(
                formatter,
                "{location}: {width}-byte command is truncated with {available} bytes available"
            ),
            Self::UnsupportedCommand {
                location,
                decoded_opcode,
                width,
            } => write!(
                formatter,
                "{location}: decoded opcode {decoded_opcode:#04x} has public width {width} but is outside the M3.2 subset"
            ),
            Self::InvalidCommand { location, reason } => {
                write!(formatter, "{location}: state-invalid command: {reason}")
            }
            Self::ResourcePlanOverflow { workload } => {
                write!(
                    formatter,
                    "workload {workload} resource-plan operation IDs overflow u32"
                )
            }
            Self::JournalMismatch {
                workload,
                expected,
                actual,
            } => write!(
                formatter,
                "workload {workload} resource journal differs from exact decoder plan: expected {} accesses, found {}",
                expected.len(),
                actual.len()
            ),
            Self::Ir(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RawDpcDecodeError {}

impl From<ValidationError> for RawDpcDecodeError {
    fn from(error: ValidationError) -> Self {
        Self::Ir(error)
    }
}

pub fn decode_raw_dpc(
    submitted: SubmittedTicket,
    durable_state: &RdpState,
) -> Result<DecodedRawDpc, RawDpcDecodeError> {
    decode_from_state(
        submitted,
        durable_state.fork_for_decode(),
        RawDpcDecodeOrigin::Durable,
    )
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
    decode_from_state(submitted, state, RawDpcDecodeOrigin::SpeculativeStaged)
}

fn decode_from_state(
    submitted: SubmittedTicket,
    mut state: RdpState,
    origin: RawDpcDecodeOrigin,
) -> Result<DecodedRawDpc, RawDpcDecodeError> {
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

    let actual = packet.journal().accesses();
    if actual != planned {
        return Err(RawDpcDecodeError::JournalMismatch {
            workload,
            expected: planned.into_boxed_slice(),
            actual: actual.to_vec().into_boxed_slice(),
        });
    }
    let resource_accesses = actual.to_vec().into_boxed_slice();
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

    Ok(DecodedRawDpc {
        submitted,
        base_state,
        commands: commands.into_boxed_slice(),
        state_delta: delta,
        staged_state,
        resource_plan: RawDpcResourcePlan {
            tmem_source_identity,
            accesses: resource_accesses,
            tmem_transfers,
            fill_spans: fill_spans.into_boxed_slice(),
            fill_seeds: fill_seeds.into_boxed_slice(),
        },
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
            // (`docs/RT64-WM2000-CENSUS.md` §3). Each rejection aborted the
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
                // `targets::clip_texrect_extent` and angrylion
                // `rasterizer.c:2349-2363`), so keeping it out of `RdpState`
                // would silently unscissor every packet that inherits the
                // rect from an earlier one.
                //
                // Latched in the decoder's own quarter-pixel wire units,
                // matching `rdp_set_scissor` (angrylion
                // `rasterizer.c:2779-2784`), which stores the four
                // twelve-bit fields with no rescale.
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
    // **A non-Fill cycle type declares no write; it does not fail the
    // decode.** This is the same "decode succeeds, journal stays silent"
    // shape `plan_raw_triangle` (Fill cycle) and `plan_texture_rectangle`
    // (every case its executor cannot produce content for) already use, and
    // it is applied here for the identical reason.
    //
    // The cycle type selects how a rectangle's *content* is produced, not
    // whether the command is legal. Three independent authorities agree:
    //
    // - **fn64's own reference lane.** `fn64-render-reference`'s
    //   `raster/draw.rs:113-128` dispatches `G_FILLRECT` on cycle type and
    //   sends one/two-cycle rectangles to `draw_combined_fill_rectangle`
    //   (`draw.rs:223`), which runs them through the colour combiner with
    //   `shade`/`texel0`/`texel1` all zero. Only the `CycleType::Fill` arm
    //   uses the fill colour. This module's own doc already said so:
    //   `targets/fill.rs`'s module header states one- and two-cycle
    //   `G_FILLRECT` "need the combiner ... not ported here".
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
    // Declaring nothing rather than erroring is not "skipping the command":
    // the command is still decoded, still pushed, and still admitted. What
    // is absent is only the journal's write entry -- which must be absent,
    // because this backend's sole fill executor
    // (`targets/fill.rs`'s `execute_fill_rectangle`) implements the
    // fill-cycle branch alone and refuses any other by name
    // (`FillExecutionError::NotFillCycle`). Declaring a write it will not
    // fill is the specific hazard `raw_triangle_is_executable`'s doc names:
    // `fill_completed_writes` slices the full-extent buffer for every
    // declared range without checking the raster touched it, so a
    // declared-but-undrawn row publishes a real digest of STALE bytes that
    // passes `validate_effects` and reaches guest RDRAM. Convincing garbage
    // beats a loud error nowhere.
    //
    // Honest nonclaim, and the rung that is still missing: declaring
    // nothing means WM2000's one-cycle rectangles are admitted but produce
    // no pixels on this backend yet. That is a *narrower* gap than refusing
    // the whole packet -- the other 15 commands in the measured packet now
    // decode and execute -- but it is not parity. Closing it needs the
    // combiner-driven rectangle executor `targets/fill.rs` names as absent,
    // and by this crate's own rule the executor widens first and this
    // planner second, never the other way round.
    //
    // **Scope: a STAGED non-Fill cycle type only.** A `FillRectangle`
    // before any `SetOtherMode` at all is a different case and keeps its
    // existing loud refusal below -- the cycle type is not merely "not
    // Fill", it is unknown, so there is no wire fact saying which content
    // producer the guest asked for. `state_order_table_rejects_each_missing_precondition`
    // pins that arm, and it is deliberately untouched here.
    let Some(other_mode) = state.other_mode() else {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle requires staged fill-cycle OtherMode",
        });
    };
    if other_mode.cycle_type() != CycleType::Fill {
        return Ok(());
    }
    let Some(image) = state.color_image() else {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle requires staged SetColorImage",
        });
    };
    if state.fill_color().is_none() {
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
    let x1 = u32::from(rectangle.lower_right_x) >> 2;
    let y1 = u32::from(rectangle.lower_right_y) >> 2;
    if x0 > x1 || y0 > y1 {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle coordinates are reversed",
        });
    }
    if x1 >= image.width() {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle exceeds the staged color-image width",
        });
    }
    // **The scissor narrows what this command WRITES, so it must narrow what
    // the journal DECLARES.**
    //
    // angrylion clips every span against the `clip` rect latched by
    // `rdp_set_scissor` (`rasterizer.c:2779-2784`) inside the edgewalker --
    // X at `:2349-2363`, Y at `:2284-2305` -- so a scissored fill never
    // touches the pixels outside the clip, and the executor
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
    // `docs/RT64-FILL-PARTIAL-SEED.md` records the seam.
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

impl FillSeedRead {
    pub const fn command_index(self) -> u32 {
        self.command_index
    }

    pub const fn access_index(self) -> Option<u32> {
        self.access_index
    }
}

/// Intersects one fill's whole-pixel destination rectangle with the staged
/// scissor, in the scissor's own quarter-pixel domain, returning `None` when
/// nothing survives.
///
/// Uses [`crate::targets::RdpScissorRect`]'s own pixel accessors rather than
/// re-deriving the quarter-pixel rounding, so the decoder and the executor
/// round the scissor identically by construction instead of by two
/// agreeing-looking copies of `div_ceil`. That rounding is angrylion's, and
/// `RdpScissorRect::quarter_to_pixel_ceil` carries its derivation.
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
/// - **`TextureRectangleFlip`**: flip's destination footprint is the same,
///   but this slice does not execute it, and declaring a write no executor
///   fills would promise content that never arrives.
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
    if rectangle.flip() {
        return Ok(());
    }
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
        if tick % 100_000 == 0 && std::env::var_os("FN64_TRI_DROP_STATS").is_some() {
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
    /// `docs/RT64-WM2000-INMATCH-GAPS.md` once admission was shown not to be
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
mod tests {
    use fn64_render_ir::{
        AccessMode, AccessPurpose, CapturedGuestRead, DecodedTicket, DeferredGuestReadCapture,
        DmemRange, DpInterruptState, DramCommandChunk, DramCommandStream, FullSyncBoundary,
        PhysicalMemoryLayout, RawCommandStream, RdramResource, ResourceAccess, ResourceJournal,
        ResourceJournalLimits, ResourceRegion, TemporalBoundary, WorkloadAdmission, WorkloadPacket,
        WorkloadPacketPreflight, XbusCommandChunk, XbusCommandStream,
    };

    use super::*;
    use crate::TmemLoadKind;

    const LAYOUT_BYTES: u32 = 0x4000;
    const COMMAND_START: u32 = 0x1000;

    /// `note_textured` must route each arm to its OWN counter.
    ///
    /// Written against DELTAS rather than absolute values on purpose: the two
    /// counters are process-global `AtomicU64`s that every other test in this
    /// binary can also advance, so an absolute assertion would pass or fail
    /// depending on test order. Deltas are order-independent.
    ///
    /// The fixture presses on the mutants that matter. Both arms are driven,
    /// with a DIFFERENT number of calls each (2 textured, 3 untextured), so:
    ///
    /// - swapping the two arms is caught (2 and 3 would trade places, which
    ///   equal counts would have hidden -- the exact fixture mistake
    ///   `RT64-WM2000-HARNESS-TRAPS.md` names, where correct and incorrect
    ///   answers coincide at the sampled point),
    /// - routing both arms to one counter is caught (one delta goes to 0),
    /// - dropping the `fetch_add` entirely is caught (both go to 0).
    ///
    /// Mutation-verified: each of those three mutants was applied and each
    /// fails this test.
    #[test]
    fn admitted_texture_bit_counts_route_to_their_own_buckets() {
        let (textured_before, untextured_before) = raw_triangle_drop_stats::textured_snapshot();

        raw_triangle_drop_stats::note_textured(true);
        raw_triangle_drop_stats::note_textured(true);
        raw_triangle_drop_stats::note_textured(false);
        raw_triangle_drop_stats::note_textured(false);
        raw_triangle_drop_stats::note_textured(false);

        let (textured_after, untextured_after) = raw_triangle_drop_stats::textured_snapshot();
        assert_eq!(
            textured_after - textured_before,
            2,
            "two textured notes must land in the textured bucket"
        );
        assert_eq!(
            untextured_after - untextured_before,
            3,
            "three untextured notes must land in the untextured bucket"
        );
    }

    /// The texture-bit counters must NOT be folded into the drop-reason
    /// `total`, which partitions the decision stream. Adding them would make
    /// `total` double-count every admitted triangle and corrupt the
    /// percentage the whole report exists to state.
    #[test]
    fn texture_bit_counts_are_not_part_of_the_drop_reason_total() {
        let before: u64 = raw_triangle_drop_stats::snapshot().iter().sum();
        raw_triangle_drop_stats::note_textured(true);
        raw_triangle_drop_stats::note_textured(false);
        let after: u64 = raw_triangle_drop_stats::snapshot().iter().sum();
        assert_eq!(
            before, after,
            "note_textured must not advance any REASONS counter"
        );
    }

    use crate::wire_words::word_with_prefix as word;

    fn state_words(prefix: u8) -> Vec<u32> {
        vec![
            word(prefix, SET_OTHER_MODE, 3 << 20),
            0,
            word(prefix, SET_COLOR_IMAGE, 3 << 19 | 1),
            0,
            word(prefix, SET_FILL_COLOR, 0),
            0x213c_4d59,
        ]
    }

    /// `state_words` with a color image wide and tall enough to contain
    /// `texrect_words`' own destination rectangle (pixels x 16..=64,
    /// y 0..=48).
    ///
    /// The narrow 2-pixel image `state_words` stages is enough for the fill
    /// fixtures, but a texrect now declares real `ColorFramebuffer` writes
    /// (`plan_texture_rectangle`), so its fixture must stage an image that
    /// actually contains them. Widening the fixture is the correct repair:
    /// the alternative -- relaxing the planner's width bound -- would let a
    /// rectangle declare writes outside the staged image.
    fn texrect_state_words(prefix: u8) -> Vec<u32> {
        vec![
            // Copy cycle (`cycle_type` 2 in bits 21:20), the mode a raw-DPC
            // texrect runs in. `state_words`' own `3 << 20` is Fill.
            word(prefix, SET_OTHER_MODE, 2 << 20),
            0,
            // RGBA16, width 65 (the wire field is width-1).
            word(prefix, SET_COLOR_IMAGE, 3 << 19 | 64),
            0,
            word(prefix, SET_FILL_COLOR, 0),
            0x213c_4d59,
        ]
    }

    fn fixture_words(prefix: u8) -> Vec<u32> {
        let mut words = state_words(prefix);
        words.extend([
            word(prefix, FILL_RECTANGLE, 4 << 12 | 4),
            0,
            word(prefix, FULL_SYNC, 0),
            0,
        ]);
        words
    }

    fn command_access(layout: PhysicalMemoryLayout, bytes: u32, operation: u32) -> ResourceAccess {
        ResourceAccess::try_new(
            OperationId::new(operation),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: layout.range(COMMAND_START, COMMAND_START + bytes).unwrap(),
            },
        )
        .unwrap()
    }

    fn effect_access(
        layout: PhysicalMemoryLayout,
        operation: u32,
        start: u32,
        end: u32,
    ) -> ResourceAccess {
        ResourceAccess::try_new(
            OperationId::new(operation),
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range: layout.range(start, end).unwrap(),
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

    fn packet(
        transaction_sequence: u64,
        words: Vec<u32>,
        effect_ranges: &[(u32, u32)],
    ) -> WorkloadPacket {
        packet_with_seed(transaction_sequence, words, effect_ranges, None)
    }

    /// `packet`, plus the colour-image SEED read a partial `FillRectangle`
    /// declares (`raw_dpc::plan_fill`).
    ///
    /// Ordered before the write accesses because `plan_fill` pushes the
    /// seed before `plan_render_target_rows` pushes the span, and the
    /// journal comparison is access-for-access.
    fn packet_with_seed(
        transaction_sequence: u64,
        words: Vec<u32>,
        effect_ranges: &[(u32, u32)],
        seed_range: Option<(u32, u32)>,
    ) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let bytes = u32::try_from(words.len() * 4).unwrap();
        let command_range = layout.range(COMMAND_START, COMMAND_START + bytes).unwrap();
        let sync_count = words
            .chunks_exact(2)
            .filter(|command| ((command[0] >> 24) as u8 & 0x3f) == FULL_SYNC)
            .count();
        let full_syncs = (0..sync_count)
            .map(|ordinal| {
                FullSyncBoundary::new(
                    2 + ordinal as u64 * 2,
                    3 + ordinal as u64 * 2,
                    DpInterruptState::Clear,
                    DpInterruptState::Asserted,
                )
            })
            .collect();
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                command_range,
                words,
                TemporalBoundary::new(1, DpInterruptState::Clear),
                full_syncs,
            )
            .unwrap()])
            .unwrap(),
        );
        let mut accesses = vec![command_access(layout, bytes, 0)];
        if let Some((start, end)) = seed_range {
            accesses.push(
                ResourceAccess::try_new(
                    OperationId::new(1),
                    AccessMode::Read,
                    AccessPurpose::TmemLoadSource,
                    ResourceRegion::Rdram {
                        resource: RdramResource::ColorFramebuffer,
                        range: layout.range(start, end).unwrap(),
                    },
                )
                .unwrap(),
            );
        }
        let first_effect = accesses.len() as u32;
        accesses.extend(
            effect_ranges
                .iter()
                .enumerate()
                .map(|(index, &(start, end))| {
            effect_access(layout, index as u32 + first_effect, start, end)
                }),
        );
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(64, LAYOUT_BYTES).unwrap(),
            accesses,
        )
        .unwrap();
        // Through the preflight + capture pair rather than
        // `WorkloadPacket::try_new`, because a journal that declares any
        // guest read must be finalized against captured bytes for it --
        // the same two-step `packet_with_tmem_sources` uses, and for the
        // identical reason. With no seed the plan declares no reads and the
        // capture is empty, so the packets this builds are unchanged.
        let preflight = WorkloadPacketPreflight::try_new(
            layout,
            WorkloadAdmission::RawDpc {
                transaction_sequence,
            },
            vec![stream],
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
    }

    fn submit(packet: WorkloadPacket) -> SubmittedTicket {
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        queue.submit(DecodedTicket::new(packet)).unwrap()
    }

    fn packet_with_tmem_sources(
        transaction_sequence: u64,
        words: Vec<u32>,
        source_ranges: &[(u32, u32)],
    ) -> WorkloadPacket {
        packet_with_tmem_sources_in_layout(LAYOUT_BYTES, transaction_sequence, words, source_ranges)
    }

    fn packet_with_tmem_sources_in_layout(
        layout_bytes: u32,
        transaction_sequence: u64,
        words: Vec<u32>,
        source_ranges: &[(u32, u32)],
    ) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(layout_bytes).unwrap();
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
                    transaction_sequence,
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

    fn packet_with_unplanned_tmem_accesses_in_layout(
        layout_bytes: u32,
        transaction_sequence: u64,
        words: Vec<u32>,
        source_ranges: &[(u32, u32)],
        destination_ranges: &[(u32, u32)],
    ) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(layout_bytes).unwrap();
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
        let destination_operation = u32::try_from(accesses.len()).unwrap();
        accesses.extend(
            destination_ranges
                .iter()
                .enumerate()
                .map(|(index, &(start, end))| {
                    ResourceAccess::try_new(
                        OperationId::new(destination_operation + index as u32),
                        AccessMode::Write,
                        AccessPurpose::TmemLoadDestination,
                        ResourceRegion::Tmem(
                            fn64_render_ir::TmemRange::try_new(start, end).unwrap(),
                        ),
                    )
                    .unwrap()
                }),
        );
        let declared = accesses
            .iter()
            .map(|access| access.region().declared_bytes())
            .sum::<u32>();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(MAX_RESOURCE_ACCESSES, declared).unwrap(),
            accesses,
        )
        .unwrap();
        let preflight = WorkloadPacketPreflight::try_new(
            layout,
            WorkloadAdmission::RawDpc {
                transaction_sequence,
            },
            vec![stream],
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
    }

    fn assert_transfer_geometry_matches_destination_union(decoded: &DecodedRawDpc, load: TmemLoad) {
        let transfer = decoded.resource_plan().bind_tmem_transfer(load).unwrap();
        let mut projected = transfer
            .words()
            .iter()
            .flat_map(|word| match word.physical() {
                crate::TmemTransferPhysicalWord::Linear(range) => vec![range],
                crate::TmemTransferPhysicalWord::SplitBanks { low, high } => vec![low, high],
            })
            .collect::<Vec<_>>();
        projected.sort_unstable_by_key(|range| (range.start(), range.end()));
        let mut canonical: Vec<fn64_render_ir::TmemRange> = Vec::new();
        for range in projected {
            if let Some(last) = canonical.last_mut() {
                if range.start() <= last.end() {
                    *last = fn64_render_ir::TmemRange::try_new(
                        last.start(),
                        last.end().max(range.end()),
                    )
                    .unwrap();
                    continue;
                }
            }
            canonical.push(range);
        }
        assert_eq!(
            canonical,
            transfer
                .destination_accesses()
                .iter()
                .map(|access| match access.region() {
                    ResourceRegion::Tmem(range) => range,
                    _ => panic!("TMEM destination access has a non-TMEM region"),
                })
                .collect::<Vec<_>>()
        );
    }

    fn set_texture_image(prefix: u8, format: u32, size: u32, width: u32, address: u32) -> [u32; 2] {
        [
            word(
                prefix,
                SET_TEXTURE_IMAGE,
                format << 21 | size << 19 | (width - 1),
            ),
            address,
        ]
    }

    fn set_tile(prefix: u8, tile: u32, line: u32, tmem: u32) -> [u32; 2] {
        [
            word(prefix, SET_TILE, 2 << 19 | line << 9 | tmem),
            tile << 24,
        ]
    }

    fn load_sync(prefix: u8) -> [u32; 2] {
        [word(prefix, LOAD_SYNC, 0), 0]
    }

    fn decode(words: Vec<u32>) -> Result<DecodedRawDpc, RawDpcDecodeError> {
        let submitted = submit(packet(7, words, &[]));
        decode_raw_dpc(submitted, &RdpState::default())
    }

    #[test]
    fn every_public_width_is_used_before_subset_rejection() {
        // 0x2a is an unassigned single-word opcode slot (known width via
        // `raw_rdp_command_width`, no `decode_stream` dispatch arm) -- a
        // stand-in for "any unsupported opcode", not a texture-rectangle
        // fixture. 0x24/0x25 moved to the "decodes successfully" test below
        // now that they are admitted opcodes.
        for (opcode, width) in [(0x2a, 8), (0x3e, 8)] {
            let mut words = vec![0; width / 4];
            words[0] = word(0x80, opcode, 0);
            let error = decode(words).unwrap_err();
            assert!(matches!(
                error,
                RawDpcDecodeError::UnsupportedCommand {
                    location,
                    decoded_opcode,
                    width: actual,
                } if location.wire_opcode() == 0x80 | opcode
                    && decoded_opcode == opcode
                    && actual == width as u32
            ));
        }
    }

    /// The eight triangle opcodes are no longer part of the "still
    /// unsupported" table above -- they decode. This proves each one's full
    /// public width is consumed successfully rather than rejected.
    #[test]
    fn every_triangle_width_decodes_successfully_before_subset_rejection_table() {
        for (opcode, width) in [
            (0x08, 32),
            (0x09, 48),
            (0x0a, 96),
            (0x0b, 112),
            (0x0c, 96),
            (0x0d, 112),
            (0x0e, 160),
            (0x0f, 176),
        ] {
            let mut words = vec![0; width / 4];
            words[0] = word(0x80, opcode, 0);
            let decoded = decode(words).unwrap_or_else(|error| {
                panic!("opcode {opcode:#04x} width {width} must decode: {error}")
            });
            assert_eq!(decoded.commands().len(), 1);
            assert!(matches!(
                decoded.commands()[0].kind(),
                RawDpcCommandKind::RawTriangle(_)
            ));
        }
    }

    /// `0x24`/`0x25` are no longer part of the "still unsupported" table
    /// above -- they decode. This proves each one's full 16-byte public
    /// width is consumed successfully rather than rejected, mirroring the
    /// triangle-width precedent immediately above.
    #[test]
    fn every_texture_rectangle_opcode_decodes_successfully_before_subset_rejection_table() {
        for opcode in [0x24u8, 0x25] {
            let mut words = vec![0u32; 4];
            words[0] = word(0x80, opcode, 0);
            let decoded = decode(words)
                .unwrap_or_else(|error| panic!("opcode {opcode:#04x} must decode: {error}"));
            assert_eq!(decoded.commands().len(), 1);
            assert!(matches!(
                decoded.commands()[0].kind(),
                RawDpcCommandKind::TextureRectangle(_)
            ));
        }
    }

    #[test]
    fn truncation_table_reports_exact_context_for_every_width_class() {
        let submitted = submit(packet(7, vec![0, 0], &[]));
        let source_identity = TmemLoadSourceIdentity::new(
            submitted.packet().identity(),
            submitted.packet().journal().identity(),
            submitted.identity(),
            submitted.packet().memory_layout(),
        );
        let packet = submitted.packet();
        let mut stream = FlattenedStream::new(packet.identity(), 0, &packet.streams()[0]);
        for (opcode, width) in [(0x00, 8), (0x24, 16), (0x08, 32), (0x0f, 176)] {
            stream.bytes = vec![0; width - 1];
            stream.bytes[0] = 0xc0 | opcode;
            let mut state = RdpState::default();
            let mut delta = RdpStateDelta::default();
            let mut planned = Vec::new();
            let mut commands = Vec::new();
            let error = decode_stream(
                &stream,
                packet.memory_layout(),
                &mut state,
                source_identity,
                &mut delta,
                &mut planned,
                &mut commands,
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .unwrap_err();
            assert!(matches!(
                error,
                RawDpcDecodeError::TruncatedCommand {
                    location,
                    width: actual_width,
                    available,
                } if location.workload() == packet.identity()
                    && location.stream() == packet.streams()[0].identity()
                    && location.chunk_index() == 0
                    && location.source_byte_offset() == COMMAND_START
                    && location.wire_opcode() == 0xc0 | opcode
                    && actual_width == width as u32
                    && available == (width - 1) as u32
            ));
        }
    }

    #[test]
    fn unknown_width_is_loud_with_complete_location() {
        let submitted = submit(packet(7, vec![0, 0], &[]));
        let source_identity = TmemLoadSourceIdentity::new(
            submitted.packet().identity(),
            submitted.packet().journal().identity(),
            submitted.identity(),
            submitted.packet().memory_layout(),
        );
        let packet = submitted.packet();
        let mut stream = FlattenedStream::new(packet.identity(), 0, &packet.streams()[0]);
        stream.bytes[0] = 0x50;
        let error = decode_stream(
            &stream,
            packet.memory_layout(),
            &mut RdpState::default(),
            source_identity,
            &mut RdpStateDelta::default(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap_err();
        let RawDpcDecodeError::UnknownCommandWidth { location } = error else {
            panic!("expected unknown command width");
        };
        assert_eq!(location.workload(), packet.identity());
        assert_eq!(location.stream(), packet.streams()[0].identity());
        assert_eq!(location.chunk_index(), 0);
        assert_eq!(location.source_byte_offset(), COMMAND_START);
        assert_eq!(location.wire_opcode(), 0x50);
    }

    #[test]
    fn unsupported_offset_table_is_byte_exact() {
        // 0x2a is an unassigned single-word opcode slot (known width via
        // `raw_rdp_command_width`'s `RDP_SYNC_LOAD..=0x3f` catch-all, but no
        // `decode_stream` dispatch arm) -- a stand-in for "any unsupported
        // opcode", not a texture-rectangle-specific fixture. 0x24/0x25 are
        // no longer valid stand-ins for this now that they decode.
        for noops in [0usize, 1, 3, 7] {
            let mut words = vec![0; noops * 2];
            words.extend([word(0x40, 0x2a, 0), 0]);
            let submitted = submit(packet(7, words, &[]));
            let error = decode_raw_dpc(submitted, &RdpState::default()).unwrap_err();
            let rendered = error.to_string();
            let RawDpcDecodeError::UnsupportedCommand { location, .. } = error else {
                panic!("expected unsupported command");
            };
            let offset = u32::try_from(noops * 8).unwrap();
            assert_eq!(location.stream_byte_offset(), offset);
            assert_eq!(location.source_byte_offset(), COMMAND_START + offset);
            assert_eq!(location.wire_opcode(), 0x6a);
            assert!(rendered.contains("chunk 0 source byte offset"));
        }
    }

    #[test]
    fn source_offset_selects_the_exact_chunk() {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let first_range = layout.range(COMMAND_START, COMMAND_START + 8).unwrap();
        let second_range = layout.range(COMMAND_START + 8, COMMAND_START + 40).unwrap();
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![
                DramCommandChunk::try_new(
                    first_range,
                    vec![0, 0],
                    TemporalBoundary::new(1, DpInterruptState::Clear),
                    Vec::new(),
                )
                .unwrap(),
                DramCommandChunk::try_new(
                    second_range,
                    {
                        // 0x2a is an unassigned single-word opcode slot, not
                        // a texture-rectangle-specific fixture -- 0x24/0x25
                        // are no longer valid stand-ins for "unsupported"
                        // now that they decode.
                        let mut words = vec![0; 8];
                        words[0] = word(0x80, 0x2a, 0);
                        words
                    },
                    TemporalBoundary::new(2, DpInterruptState::Clear),
                    Vec::new(),
                )
                .unwrap(),
            ])
            .unwrap(),
        );
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(4, 64).unwrap(),
            vec![ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: layout.range(COMMAND_START, COMMAND_START + 40).unwrap(),
                },
            )
            .unwrap()],
        )
        .unwrap();
        let submitted = submit(
            WorkloadPacket::try_new(
                layout,
                WorkloadAdmission::RawDpc {
                    transaction_sequence: 7,
                },
                vec![stream],
                journal,
            )
            .unwrap(),
        );
        let RawDpcDecodeError::UnsupportedCommand { location, .. } =
            decode_raw_dpc(submitted, &RdpState::default()).unwrap_err()
        else {
            panic!("expected unsupported command");
        };
        assert_eq!(location.chunk_index(), 1);
        assert_eq!(location.stream_byte_offset(), 8);
        assert_eq!(location.source_byte_offset(), COMMAND_START + 8);
        assert_eq!(location.wire_opcode(), 0xaa);
    }

    #[test]
    fn xbus_noop_has_an_exact_rsp_dmem_command_plan() {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let range = DmemRange::try_new(0x100, 0x108).unwrap();
        let stream = RawCommandStream::Xbus(
            XbusCommandStream::try_new(vec![XbusCommandChunk::try_new(
                range,
                vec![0xc0, 0, 0, 0, 0, 0, 0, 0],
                TemporalBoundary::new(1, DpInterruptState::Clear),
                Vec::new(),
            )
            .unwrap()])
            .unwrap(),
        );
        let access = ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::RspDmem(range),
        )
        .unwrap();
        let journal =
            ResourceJournal::try_new(ResourceJournalLimits::try_new(1, 8).unwrap(), vec![access])
                .unwrap();
        let submitted = submit(
            WorkloadPacket::try_new(
                layout,
                WorkloadAdmission::RawDpc {
                    transaction_sequence: 7,
                },
                vec![stream],
                journal,
            )
            .unwrap(),
        );
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        assert_eq!(decoded.resource_plan().accesses(), [access]);
        assert_eq!(decoded.commands()[0].location().source_byte_offset(), 0x100);
    }

    #[test]
    fn all_low_noop_variants_and_four_prefixes_are_admitted() {
        let mut words = Vec::new();
        for prefix in [0x00, 0x40, 0x80, 0xc0] {
            for variant in 0..=7 {
                words.extend([word(prefix, variant, 0x005a_5a5a), 0xa5a5_a5a5]);
            }
        }
        let submitted = submit(packet(7, words, &[]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        assert_eq!(decoded.commands().len(), 32);
        for (index, command) in decoded.commands().iter().enumerate() {
            assert_eq!(
                command.kind(),
                RawDpcCommandKind::NoOp {
                    variant: (index % 8) as u8
                }
            );
        }
        assert_eq!(decoded.state_delta(), &RdpStateDelta::default());
    }

    // ------------------------------------------------------------------
    // WM2000's three blocking opcodes: SyncPipe (0x27), SyncTile (0x28), and
    // the 0x1f stream terminator.
    //
    // Measured motivation, not speculative coverage: the census
    // (`docs/RT64-WM2000-CENSUS.md` §3) counted 20,800 + 1,808 + 219
    // occurrences across 218/218 frames of a real WM2000 run. Each was a
    // whole-stream abort, so these three ids alone held every frame at zero
    // decoded commands.
    // ------------------------------------------------------------------

    /// All three newly-admitted ids decode as `NoOp` under all four wire
    /// prefixes, and none of them stages any RDP state.
    ///
    /// The `state_delta` assertion is the load-bearing half. "It decodes" is
    /// necessary but not sufficient: admitting an opcode that quietly staged
    /// state would move pixels, and the whole argument for admitting these
    /// three without designing a semantic is that `NoOp` cannot.
    #[test]
    fn the_three_wm2000_blocking_ids_decode_as_state_free_noops() {
        for prefix in [0x00u8, 0x40, 0x80, 0xc0] {
            for id in [SYNC_PIPE, SYNC_TILE, STREAM_TERMINATOR] {
                // Nonzero payloads: the RDP assigns no field to any of these
                // three, so a decoder that read one would be inventing it.
                let words = vec![word(prefix, id, 0x005a_5a5a), 0xa5a5_a5a5];
                let submitted = submit(packet(7, words, &[]));
                let decoded =
                    decode_raw_dpc(submitted, &RdpState::default()).unwrap_or_else(|error| {
                        panic!("id {id:#04x} under prefix {prefix:#04x} must decode: {error}")
                    });
                assert_eq!(decoded.commands().len(), 1);
                assert_eq!(
                    decoded.commands()[0].kind(),
                    RawDpcCommandKind::NoOp { variant: id },
                    "id {id:#04x} must decode as a NoOp carrying its own variant"
                );
                assert_eq!(
                    decoded.state_delta(),
                    &RdpStateDelta::default(),
                    "id {id:#04x} must stage no RDP state; admitting it must not be able to \
                     move a pixel"
                );
            }
        }
    }

    /// **The measurement this slice exists for.** A WM2000-shaped command
    /// sequence -- fill state, a fill, the sync ops it really interleaves,
    /// and its `0x1f` terminator -- decodes end to end instead of aborting.
    ///
    /// Shaped from the census's own per-frame profile (§3): every frame
    /// issues `SyncPipe` many times, `SyncTile` occasionally, a
    /// `FillRectangle`, and exactly one terminator last. Deliberately NOT a
    /// mixed fill+TMEM packet -- this test is about DECODE, and keeping the
    /// two concerns apart is what makes a failure here point at the decoder.
    /// The fill+TMEM composition the census recorded as a hard refusal at
    /// 218/218 frames is now admitted at execution
    /// (`production::StagedOutcome::MixedFillAndTmemLoads`), and is proven
    /// by that module's own tests plus `fn64-abi`'s
    /// `raw_dpc_session_integration` end-to-end pair.
    ///
    /// The command count is asserted exactly, and the terminator's position
    /// with it: a decoder that treated `0x1f` as a `break` would return 9
    /// commands rather than 10 and would silently drop everything after the
    /// first terminator in a coalesced stream.
    #[test]
    fn a_wm2000_shaped_stream_decodes_end_to_end_rather_than_aborting() {
        let mut words = Vec::new();
        // Frame setup, then the syncs WM2000 actually interleaves.
        words.extend([word(0, SYNC_PIPE, 0), 0]);
        words.extend(state_words(0));
        words.extend([word(0, SYNC_TILE, 0), 0]);
        words.extend([word(0, SYNC_PIPE, 0), 0]);
        words.extend([word(0, FILL_RECTANGLE, 4 << 12 | 4), 0]);
        words.extend([word(0, SYNC_PIPE, 0), 0]);
        words.extend([word(0, FULL_SYNC, 0), 0]);
        // The terminator the game writes to end every submission.
        words.extend([word(0, STREAM_TERMINATOR, 0), 0]);

        let submitted = submit(packet(7, words, &[(0, 16)]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default())
            .expect("a WM2000-shaped stream must decode end to end");

        // 4 syncs + 3 state + 1 fill + 1 FullSync + 1 terminator.
        assert_eq!(
            decoded.commands().len(),
            10,
            "every command must be decoded, including the ones after the terminator's \
             predecessors -- a `break` on 0x1f would come up short"
        );
        assert_eq!(
            decoded.commands().last().unwrap().kind(),
            RawDpcCommandKind::NoOp {
                variant: STREAM_TERMINATOR
            },
            "the terminator must be the last decoded command, not the end of decoding"
        );
        assert!(
            decoded
                .commands()
                .iter()
                .any(|command| matches!(command.kind(), RawDpcCommandKind::FillRectangle(_))),
            "the fill must survive the syncs around it"
        );
        // The fill's own state really was staged, so "it decoded" is not
        // passing on an empty stream.
        assert_eq!(
            decoded.staged_state().fill_color().unwrap().rgba32(),
            [0x21, 0x3c, 0x4d, 0x59],
            "the fill color from state_words must be staged"
        );
    }

    /// A terminator mid-stream does not truncate the commands after it.
    ///
    /// This is the specific defect a `break`-based reading of `0x1f` would
    /// introduce, and it is invisible to a fixture whose terminator is last.
    /// Coalesced submissions really do carry more than one.
    #[test]
    fn a_mid_stream_terminator_does_not_truncate_what_follows() {
        let mut words = Vec::new();
        words.extend([word(0, STREAM_TERMINATOR, 0), 0]);
        words.extend(state_words(0));
        words.extend([word(0, STREAM_TERMINATOR, 0), 0]);
        words.extend([word(0, FILL_RECTANGLE, 4 << 12 | 4), 0]);
        words.extend([word(0, STREAM_TERMINATOR, 0), 0]);

        let submitted = submit(packet(7, words, &[(0, 16)]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default())
            .expect("terminators must not abort the stream");
        assert_eq!(
            decoded.commands().len(),
            7,
            "three terminators plus three state commands plus the fill -- a decoder that \
             stopped at the first terminator would report 1"
        );
        assert!(
            decoded
                .commands()
                .iter()
                .any(|command| matches!(command.kind(), RawDpcCommandKind::FillRectangle(_))),
            "the fill sits after two terminators and must still be decoded"
        );
    }

    /// Admitting three ids must not have widened the door for a fourth.
    ///
    /// The two neighbour classes are refused at *different layers*, and the
    /// test asserts each where it actually happens rather than forcing both
    /// through one path:
    ///
    /// - `0x1e`/`0x20` bracket the terminator inside the still-rejected
    ///   `0x10..=0x23` block. They never reach this decoder at all --
    ///   `WorkloadPacket` construction validates command widths through
    ///   `fn64-render-ir`'s own copy of the table and refuses first, with
    ///   `ValidationError::UnknownRdpOpcode`. Discovered by writing this
    ///   test against `decode_raw_dpc` and watching the packet builder panic
    ///   instead; asserting at the decoder would have been asserting against
    ///   a layer that never sees the input.
    /// - `0x2a`/`0x2b`/`0x2c` (`SetKeyGB`, `SetKeyR`, `SetConvert`) are real
    ///   RDP commands with an admitted width but no decode arm, which the
    ///   census measured at zero occurrences and says explicitly not to
    ///   build. They reach the match's catch-all and name themselves.
    #[test]
    fn the_neighbouring_ids_are_still_refused() {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let one_command_stream = |id: u8| {
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                layout.range(COMMAND_START, COMMAND_START + 8).unwrap(),
                vec![word(0, id, 0), 0],
                TemporalBoundary::new(1, DpInterruptState::Clear),
                Vec::new(),
            )
            .expect("chunk construction validates length, not opcodes")])
        };
        for id in [0x1eu8, 0x20] {
            let error = one_command_stream(id)
                .expect_err("an id with no admitted width must be refused before decode");
            assert!(
                matches!(error, ValidationError::UnknownRdpOpcode { .. }),
                "id {id:#04x} must still have no admitted width; the carve-out was one id \
                 wide. Got {error:?}"
            );
        }
        // The same construction with the carved-out id succeeds, so the two
        // assertions above are discriminating rather than rejecting
        // everything put in front of them.
        one_command_stream(STREAM_TERMINATOR)
            .expect("the measured terminator must be accepted where its neighbours are not");

        for id in [0x2au8, 0x2b, 0x2c] {
            let submitted = submit(packet(7, vec![word(0, id, 0), 0], &[]));
            let error = decode_raw_dpc(submitted, &RdpState::default())
                .expect_err("an unadmitted command must be refused");
            let RawDpcDecodeError::UnsupportedCommand { decoded_opcode, .. } = error else {
                panic!("id {id:#04x} must be refused as UnsupportedCommand, got {error:?}");
            };
            assert_eq!(
                decoded_opcode, id,
                "the refusal must name the offending opcode"
            );
        }
    }

    #[test]
    fn every_admitted_state_command_accepts_all_four_wire_prefixes() {
        for prefix in [0x00, 0x40, 0x80, 0xc0] {
            let submitted = submit(packet(7, fixture_words(prefix), &[(0, 16)]));
            let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
            assert_eq!(decoded.commands().len(), 5);
            assert_eq!(
                decoded.staged_state().fill_color().unwrap().rgba32(),
                [0x21, 0x3c, 0x4d, 0x59]
            );
            for command in decoded.commands() {
                assert_eq!(command.location().wire_opcode() & 0xc0, prefix);
            }
        }
    }

    /// M4.3.4 end-to-end: decode a real raw-DPC command stream containing a
    /// `FillRectangle` and execute the decoded command against a real color
    /// target, proving `execute_fill_rectangle` is a genuine production
    /// consumer of `decode_raw_dpc`'s output -- not just of an independently
    /// hand-built `FillRectangle` value.
    #[test]
    fn decoded_fill_rectangle_executes_against_a_real_color_target() {
        use crate::targets::{
            execute_fill_rectangle, ColorTargetExtent, ColorTargetFormat, ColorTargetKey,
            ColorTargetRegistry,
        };

        let submitted = submit(packet(7, fixture_words(0), &[(0, 16)]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        let fill_rectangle_command = decoded
            .commands()
            .iter()
            .find_map(|command| match command.kind() {
                RawDpcCommandKind::FillRectangle(rectangle) => Some(rectangle),
                _ => None,
            })
            .expect("fixture_words contains exactly one FillRectangle");

        let color_image = decoded.staged_state().color_image().unwrap();
        let other_mode = decoded.staged_state().other_mode().unwrap();
        let fill_color = decoded.staged_state().fill_color().unwrap();
        assert_eq!(color_image.width(), 2);

        let registry = ColorTargetRegistry::try_new(color_image.address().layout(), 1).unwrap();
        let key = ColorTargetKey::try_new(
            color_image.address(),
            ColorTargetExtent::try_new(color_image.width(), 2).unwrap(),
            ColorTargetFormat::try_from_rdp(color_image.format(), color_image.size()).unwrap(),
        )
        .unwrap();
        let candidate = registry.begin_candidate(key).unwrap();

        let completed = execute_fill_rectangle(
            &candidate,
            other_mode,
            fill_color,
            fill_rectangle_command,
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 0xffff, 0xffff),
            None,
        )
        .unwrap();
        // RGBA32, period 1: every pixel is the fill color's RGB + expanded
        // low-5-bit alpha-coverage byte. 0x59 & 0x1f = 0x19; expand_five(0x19)
        // = (0x19<<3)|(0x19>>2) = 0xc8|0x06 = 0xce.
        assert_eq!(
            completed.device_bytes().device_bytes(),
            [0x21, 0x3c, 0x4d, 0xce].repeat(4)
        );
        assert_eq!(completed.rectangle().width(), 2);
        assert_eq!(completed.rectangle().height(), 2);
    }

    #[test]
    fn state_order_table_rejects_each_missing_precondition() {
        let cases = [
            ("OtherMode", vec![word(0, FILL_RECTANGLE, 0), 0]),
            (
                "SetColorImage",
                vec![
                    word(0, SET_OTHER_MODE, 3 << 20),
                    256,
                    word(0, FILL_RECTANGLE, 0),
                    0,
                ],
            ),
            (
                "SetFillColor",
                vec![
                    word(0, SET_OTHER_MODE, 3 << 20),
                    0,
                    word(0, SET_COLOR_IMAGE, 3 << 19 | 1),
                    0,
                    word(0, FILL_RECTANGLE, 0),
                    0,
                ],
            ),
        ];
        for (expected, words) in cases {
            let error = decode(words).unwrap_err();
            let RawDpcDecodeError::InvalidCommand { location, reason } = error else {
                panic!("expected state-invalid command");
            };
            assert!(reason.contains(expected));
            assert_eq!(location.wire_opcode() & 0x3f, FILL_RECTANGLE);
        }
    }

    #[test]
    fn hostile_state_mutation_table_is_loud_and_located() {
        let mut cases = vec![
            (
                "reserved image format",
                vec![word(0, SET_COLOR_IMAGE, 7 << 21 | 3 << 19 | 1), 0],
            ),
            (
                "unaligned image",
                vec![word(0, SET_COLOR_IMAGE, 3 << 19 | 1), 4],
            ),
            (
                "image outside RDRAM",
                vec![word(0, SET_COLOR_IMAGE, 3 << 19 | 1), LAYOUT_BYTES],
            ),
        ];
        let mut unsupported_image = vec![
            word(0, SET_OTHER_MODE, 3 << 20),
            0,
            word(0, SET_COLOR_IMAGE, 1),
            0,
            word(0, SET_FILL_COLOR, 0),
            0,
        ];
        unsupported_image.extend([word(0, FILL_RECTANGLE, 0), 0]);
        cases.push(("unsupported image size", unsupported_image));
        let mut fractional = state_words(0);
        fractional.extend([word(0, FILL_RECTANGLE, 1), 0]);
        cases.push(("fractional rectangle", fractional));
        let mut reversed = state_words(0);
        reversed.extend([word(0, FILL_RECTANGLE, 0), 4 << 12]);
        cases.push(("reversed rectangle", reversed));
        let mut too_wide = state_words(0);
        too_wide.extend([word(0, FILL_RECTANGLE, 8 << 12), 0]);
        cases.push(("wide rectangle", too_wide));
        for (name, words) in cases {
            let submitted = submit(packet(7, words, &[]));
            let workload = submitted.packet().identity();
            let stream = submitted.packet().streams()[0].identity();
            let error = decode_raw_dpc(submitted, &RdpState::default()).unwrap_err();
            let RawDpcDecodeError::InvalidCommand { location, .. } = &error else {
                panic!("{name}: expected state-invalid command, got {error:?}");
            };
            assert_eq!(location.workload(), workload, "{name}");
            assert_eq!(location.stream(), stream, "{name}");
            let rendered = error.to_string();
            assert!(
                rendered.contains("source byte offset"),
                "{name}: {rendered}"
            );
            assert!(rendered.contains("wire opcode"), "{name}: {rendered}");
        }
    }

    /// This card removes `SET_OTHER_MODE`'s decode-time cycle-type gate
    /// (`raw_dpc::mod.rs`'s old `if value.cycle_type() != CycleType::Fill`
    /// check): every cycle type RT64's own `RDP::setOtherMode` accepts
    /// (`OneCycle`/`TwoCycle`/`Copy`/`Fill` -- RT64 never gates this setter
    /// on cycle type) must now decode successfully here too, not just
    /// `Fill`.
    #[test]
    fn set_other_mode_decodes_successfully_for_every_cycle_type() {
        for (name, cycle_bits) in [
            ("OneCycle", 0u32),
            ("TwoCycle", 1u32),
            ("Copy", 2u32),
            ("Fill", 3u32),
        ] {
            let words = vec![word(0, SET_OTHER_MODE, cycle_bits << 20), 0];
            let decoded = decode(words).unwrap_or_else(|error| {
                panic!("{name}: SetOtherMode must decode for every cycle type, got {error:?}")
            });
            assert!(
                matches!(
                    decoded.commands()[0].kind(),
                    RawDpcCommandKind::SetOtherMode(_)
                ),
                "{name}"
            );
        }
    }

    /// **Replaces the card that asserted a non-Fill cycle type is
    /// *rejected*.** That assertion pinned scaffolding, not hardware: the
    /// RDP defines `G_FILLRECT` in every cycle type, and cycle type selects
    /// how the rectangle's content is produced, not whether the command is
    /// legal. See `plan_fill`'s own doc for the three authorities
    /// (fn64's reference lane `raster/draw.rs:113-128`, RT64
    /// `rt64_rdp.cpp:1033-1050`, and the measured WM2000 packet in
    /// `docs/WM2000-FILLRECT-EVIDENCE.txt`).
    ///
    /// FAILS BEFORE this change (`decode` returned `Err(InvalidCommand {
    /// reason: "FillRectangle requires staged fill-cycle OtherMode" })`),
    /// PASSES AFTER (the whole stream decodes and the fill is admitted).
    ///
    /// Both halves are asserted, and the second is the one that matters:
    /// it is not enough that the decode stops failing, the `FillRectangle`
    /// must actually be present as a decoded command. A change that
    /// silently dropped the command would satisfy the first assertion
    /// alone.
    #[test]
    fn a_non_fill_cycle_fill_rectangle_decodes_and_is_admitted() {
        for (name, cycle_bits) in [("OneCycle", 0u32), ("TwoCycle", 1), ("Copy", 2)] {
            let words = vec![
                word(0, SET_OTHER_MODE, cycle_bits << 20),
                0,
                word(0, SET_COLOR_IMAGE, 3 << 19 | 1),
                0,
                word(0, SET_FILL_COLOR, 0),
                0x213c_4d59,
                word(0, FILL_RECTANGLE, 4 << 12 | 4),
                0,
            ];
            let decoded = decode(words).unwrap_or_else(|error| {
                panic!("{name}: a non-Fill cycle FillRectangle must decode, got {error:?}")
            });
            assert!(
                decoded
                    .commands()
                    .iter()
                    .any(|command| matches!(
                        command.kind(),
                        RawDpcCommandKind::FillRectangle(_)
                    )),
                "{name}: the FillRectangle must be admitted as a decoded command,                  not silently dropped"
            );
        }
    }

    /// The other half of the same contract, and the arm a mutation that
    /// merely deleted the cycle-type check would break: a non-Fill cycle
    /// fill declares **no** journal write.
    ///
    /// This is load-bearing, not bookkeeping. `targets/fill.rs`'s
    /// `execute_fill_rectangle` implements only the fill-cycle branch and
    /// refuses any other by name (`FillExecutionError::NotFillCycle`), so a
    /// declared write here would be a range nothing fills --
    /// `fill_completed_writes` would slice the full-extent buffer for it
    /// and publish a real digest of stale bytes into guest RDRAM.
    ///
    /// FAILS BEFORE (the stream did not decode at all, so there was no plan
    /// to inspect), PASSES AFTER.
    ///
    /// Derived by hand from the wire, not from the code under test: the
    /// Fill-cycle control below stages an RGBA16 (`3 << 19 | 1`) colour
    /// image and a 4x4-quarter-pixel rectangle (`4 << 12 | 4` = lrx 4,
    /// lry 4, ulx/uly 0), i.e. exactly one whole pixel at (0, 0) with
    /// `x0 == 0` and `x1 + 1 == 1 == image.width()` -- the whole-width case
    /// `plan_render_target_rows` collapses to a single access. So the
    /// Fill-cycle expectation is 1 declared write and the non-Fill
    /// expectation is 0.
    #[test]
    fn a_non_fill_cycle_fill_rectangle_declares_no_write_but_a_fill_cycle_one_does() {
        let stream = |cycle_bits: u32| {
            vec![
                word(0, SET_OTHER_MODE, cycle_bits << 20),
                0,
                word(0, SET_COLOR_IMAGE, 3 << 19 | 1),
                0,
                word(0, SET_FILL_COLOR, 0),
                0x213c_4d59,
                word(0, FILL_RECTANGLE, 4 << 12 | 4),
                0,
            ]
        };
        // The plan's OWN access list, read the way `plan_raw_dpc_inner`
        // reads it in production: decode against a journal the packet does
        // not carry, and take the planner's list off the resulting
        // `JournalMismatch::expected`. Going through `decode`'s success
        // path instead would require guessing the journal in advance --
        // which is the very number under test.
        let render_target_writes = |cycle_bits: u32| {
            let submitted = submit(packet(7, stream(cycle_bits), &[]));
            let accesses = match decode_raw_dpc(submitted, &RdpState::default()) {
                Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
                Ok(decoded) => decoded.resource_plan().accesses().to_vec(),
                Err(error) => panic!("every cycle type must decode or mismatch, got {error:?}"),
            };
            accesses
                .iter()
                .filter(|access| {
                    access.mode() == AccessMode::Write
                        && access.purpose() == AccessPurpose::RenderTarget
                })
                .count()
        };

        // The control arm. Fill cycle (bits 3) still declares its write --
        // this is what makes the fixture non-degenerate: without it, a
        // planner that declared nothing for EVERY cycle type would pass the
        // zero-write assertions below.
        assert_eq!(
            render_target_writes(3),
            1,
            "a Fill-cycle FillRectangle must still declare its one whole-width write"
        );
        for (name, cycle_bits) in [("OneCycle", 0u32), ("TwoCycle", 1), ("Copy", 2)] {
            assert_eq!(
                render_target_writes(cycle_bits),
                0,
                "{name}: a FillRectangle this backend has no executor for must declare                  no write -- a declared-but-unfilled range publishes stale bytes"
            );
        }
    }

    /// The real WM2000 packet, replayed from the measured wire bytes.
    ///
    /// Not a synthesized approximation: these are the first commands of the
    /// 600-byte stream captured at VI swap 2522 on the real ROM and
    /// recorded verbatim in `docs/WM2000-FILLRECT-EVIDENCE.txt`. The
    /// `SetOtherMode` words are the guest's own
    /// (`0xef08acff 0x00504240`, cycle bits 0 = one cycle), and the
    /// `FillRectangle` is the guest's own first band
    /// (`0xf677c010 0x00000000`).
    ///
    /// Hand-decoded from the wire, independently of any fn64 code: word 0
    /// is `0xf677c010`, so `lrx = (0x77c010 >> 12) & 0xfff = 0x77c = 1916`
    /// and `lry = 0x010 = 16`; word 1 is zero, so `ulx = uly = 0`. Divided
    /// by 4 that is x 0..=478 (inclusive edges) and y 0..=4 -- a full-width
    /// four-scanline band, one of the 60 that tile a 480x240 screen.
    ///
    /// FAILS BEFORE with the exact production error string, PASSES AFTER.
    #[test]
    fn the_measured_wm2000_swap_2522_packet_decodes() {
        // The guest's own SetOtherMode: 0xef08acff / 0x00504240.
        // Cycle bits are (0x08acff >> 20) & 3 == 0 -- one cycle.
        let other_mode_high = 0x0008_acffu32;
        assert_eq!(
            (other_mode_high >> 20) & 3,
            0,
            "hand-check of the measured wire word: WM2000 stages ONE-cycle here"
        );
        let words = vec![
            0xef00_0000 | other_mode_high,
            0x0050_4240,
            word(0, SET_COLOR_IMAGE, 3 << 19 | 479),
            0,
            // The guest's own first band, verbatim.
            0xf677_c010,
            0x0000_0000,
        ];
        let decoded = decode(words).expect(
            "the measured WM2000 packet must decode; before this change it failed with              \"FillRectangle requires staged fill-cycle OtherMode\"",
        );
        let fill = decoded
            .commands()
            .iter()
            .find_map(|command| match command.kind() {
                RawDpcCommandKind::FillRectangle(rectangle) => Some(rectangle),
                _ => None,
            })
            .expect("the guest's FillRectangle must be admitted");
        // Hand-derived above from 0xf677c010, never read back from the
        // decoder's own extraction.
        assert_eq!(fill.lower_right_x(), 1916);
        assert_eq!(fill.lower_right_y(), 16);
        assert_eq!(fill.upper_left_x(), 0);
        assert_eq!(fill.upper_left_y(), 0);
    }

    #[test]
    fn full_sync_preserves_unassigned_nonzero_payload_bits() {
        let words = vec![word(0x80, FULL_SYNC, 0x005a_a55a), 0xa5a5_5a5a];
        let decoded = decode_raw_dpc(submit(packet(7, words, &[])), &RdpState::default()).unwrap();
        assert!(matches!(
            decoded.commands()[0].kind(),
            RawDpcCommandKind::FullSync(_)
        ));
    }

    #[test]
    fn resource_plan_growth_is_bounded_at_the_command_location() {
        let mut words = vec![
            word(0, SET_OTHER_MODE, 3 << 20),
            0,
            word(0, SET_COLOR_IMAGE, 3 << 19 | 2),
            0,
            word(0, SET_FILL_COLOR, 0),
            0,
        ];
        for _ in 0..17 {
            words.extend([word(0, FILL_RECTANGLE, 4 << 12 | (1023 * 4)), 0]);
        }
        let submitted = submit(packet(7, words, &[]));
        let error = decode_raw_dpc(submitted, &RdpState::default()).unwrap_err();
        assert!(matches!(
            error,
            RawDpcDecodeError::InvalidCommand { location, reason }
                if location.wire_opcode() & 0x3f == FILL_RECTANGLE
                    && reason.contains("bounded resource-plan")
        ));
    }

    #[test]
    fn exact_journal_equality_includes_order_identity_and_cardinality() {
        let valid = packet(7, fixture_words(0), &[(0, 16)]);
        let expected = valid.journal().accesses().to_vec();
        for accesses in [
            vec![expected[0]],
            vec![
                ResourceAccess::try_new(
                    OperationId::new(0),
                    expected[1].mode(),
                    expected[1].purpose(),
                    expected[1].region(),
                )
                .unwrap(),
                ResourceAccess::try_new(
                    OperationId::new(1),
                    expected[0].mode(),
                    expected[0].purpose(),
                    expected[0].region(),
                )
                .unwrap(),
            ],
            vec![
                expected[0],
                ResourceAccess::try_new(
                    OperationId::new(9),
                    expected[1].mode(),
                    expected[1].purpose(),
                    expected[1].region(),
                )
                .unwrap(),
            ],
            vec![
                expected[0],
                expected[1],
                ResourceAccess::try_new(
                    OperationId::new(2),
                    AccessMode::Read,
                    AccessPurpose::UploadSource,
                    ResourceRegion::Rdram {
                        resource: RdramResource::Buffer,
                        range: valid.memory_layout().range(0x200, 0x208).unwrap(),
                    },
                )
                .unwrap(),
            ],
        ] {
            let journal = ResourceJournal::try_new(
                ResourceJournalLimits::try_new(64, LAYOUT_BYTES).unwrap(),
                accesses.clone(),
            )
            .unwrap();
            let altered = WorkloadPacket::try_new(
                valid.memory_layout(),
                valid.admission(),
                valid.streams().to_vec(),
                journal,
            )
            .unwrap();
            let submitted = submit(altered);
            let RawDpcDecodeError::JournalMismatch {
                expected: planned,
                actual,
                ..
            } = decode_raw_dpc(submitted, &RdpState::default()).unwrap_err()
            else {
                panic!("expected exact journal mismatch");
            };
            assert_eq!(&*planned, expected);
            assert_eq!(&*actual, accesses);
        }
    }

    #[test]
    fn non_contiguous_rectangle_rows_have_one_exact_access_each() {
        let mut words = vec![
            word(0, SET_OTHER_MODE, 3 << 20),
            0,
            word(0, SET_COLOR_IMAGE, 3 << 19 | 3),
            0,
            word(0, SET_FILL_COLOR, 0),
            0x0102_0304,
        ];
        words.extend([word(0, FILL_RECTANGLE, 8 << 12 | 4), 4 << 12]);
        // The fill covers x 1..=2 of rows 0..=1 in a 4-wide image, so it is
        // partial and declares a colour-image seed over the whole target.
        // Hand-derived: `SET_COLOR_IMAGE 3 << 19 | 3` gives width 4, RGBA16
        // (2 bytes/pixel), address 0; no height is configured, so the seed
        // spans the fill's own last row -- 4 x 2 pixels x 2 bytes = 32.
        let submitted = submit(packet_with_seed(
            7,
            words,
            &[(4, 12), (20, 28)],
            Some((0, 32)),
        ));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        assert_eq!(
            decoded.resource_plan().accesses(),
            decoded.submitted().packet().journal().accesses()
        );
    }

    #[test]
    fn two_packet_chaining_is_explicit_move_only_and_does_not_mutate_baseline() {
        let durable = RdpState::default();
        let first_packet = packet(7, state_words(0), &[]);
        let second_packet = packet(
            8,
            vec![
                word(0, FILL_RECTANGLE, 4 << 12 | 4),
                0,
                word(0, FULL_SYNC, 0),
                0,
            ],
            &[(0, 16)],
        );
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let first = queue.submit(DecodedTicket::new(first_packet)).unwrap();
        let second = queue.submit(DecodedTicket::new(second_packet)).unwrap();
        let first = decode_raw_dpc(first, &durable).unwrap();
        assert_eq!(durable, RdpState::default());
        assert_eq!(first.staged_state().transaction_sequence(), 7);
        let second = decode_raw_dpc_after(second, first.into_staged_state()).unwrap();
        assert_eq!(second.state_delta(), &RdpStateDelta::default());
        assert_eq!(
            second.staged_state().fill_color().unwrap().value(),
            0x213c_4d59
        );
        assert_eq!(durable, RdpState::default());
    }

    #[test]
    fn stale_or_cross_queue_staged_state_is_rejected_before_decode() {
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let first = queue
            .submit(DecodedTicket::new(packet(7, state_words(0), &[])))
            .unwrap();
        let stale = queue
            .submit(DecodedTicket::new(packet(7, vec![0, 0], &[])))
            .unwrap();
        let staged = decode_raw_dpc(first, &RdpState::default())
            .unwrap()
            .into_staged_state();
        assert!(matches!(
            decode_raw_dpc_after(stale, staged),
            Err(RawDpcDecodeError::StagedStateMismatch { reason, .. })
                if reason.contains("transaction sequence")
        ));

        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let first = queue
            .submit(DecodedTicket::new(packet(7, state_words(0), &[])))
            .unwrap();
        let gap = queue
            .submit(DecodedTicket::new(packet(9, vec![0, 0], &[])))
            .unwrap();
        let staged = decode_raw_dpc(first, &RdpState::default())
            .unwrap()
            .into_staged_state();
        assert!(matches!(
            decode_raw_dpc_after(gap, staged),
            Err(RawDpcDecodeError::StagedStateMismatch { reason, .. })
                if reason.contains("transaction sequence")
        ));

        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let maximum = queue
            .submit(DecodedTicket::new(packet(u64::MAX, state_words(0), &[])))
            .unwrap();
        let wrapped = queue
            .submit(DecodedTicket::new(packet(0, vec![0, 0], &[])))
            .unwrap();
        let staged = decode_raw_dpc(maximum, &RdpState::default())
            .unwrap()
            .into_staged_state();
        assert!(matches!(
            decode_raw_dpc_after(wrapped, staged),
            Err(RawDpcDecodeError::StagedStateMismatch { reason, .. })
                if reason.contains("transaction sequence")
        ));

        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let first = queue
            .submit(DecodedTicket::new(packet(7, state_words(0), &[])))
            .unwrap();
        let _intervening = queue
            .submit(DecodedTicket::new(packet(8, vec![0, 0], &[])))
            .unwrap();
        let skipped = queue
            .submit(DecodedTicket::new(packet(9, vec![0, 0], &[])))
            .unwrap();
        let staged = decode_raw_dpc(first, &RdpState::default())
            .unwrap()
            .into_staged_state();
        assert!(matches!(
            decode_raw_dpc_after(skipped, staged),
            Err(RawDpcDecodeError::StagedStateMismatch { reason, .. })
                if reason.contains("semantic submission")
        ));

        let (mut first_queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let source = first_queue
            .submit(DecodedTicket::new(packet(7, state_words(0), &[])))
            .unwrap();
        let staged = decode_raw_dpc(source, &RdpState::default())
            .unwrap()
            .into_staged_state();
        let (mut other_queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let other = other_queue
            .submit(DecodedTicket::new(packet(8, vec![0, 0], &[])))
            .unwrap();
        assert!(matches!(
            decode_raw_dpc_after(other, staged),
            Err(RawDpcDecodeError::StagedStateMismatch { reason, .. })
                if reason.contains("queue")
        ));
    }

    #[test]
    fn tmem_state_commands_decode_every_public_field_width_for_all_prefixes() {
        for prefix in [0x00, 0x40, 0x80, 0xc0] {
            let words = vec![
                word(prefix, SET_TEXTURE_IMAGE, 3 << 21 | 3 << 19 | 0x0abc),
                0xfc00_0200,
                word(prefix, SET_TILE, 4 << 21 | 3 << 19 | 0x01ab << 9 | 0x01fe),
                7 << 24
                    | 0x0f << 20
                    | 3 << 18
                    | 0x0a << 14
                    | 0x0b << 10
                    | 1 << 8
                    | 0x0c << 4
                    | 0x0d,
                word(prefix, SET_TILE_SIZE, 0x0fed << 12 | 0x0cba),
                7 << 24 | 0x0abc << 12 | 0x0789,
            ];
            let decoded =
                decode_raw_dpc(submit(packet(7, words, &[])), &RdpState::default()).unwrap();
            assert_eq!(decoded.commands().len(), 3);
            let image = decoded.staged_state().tmem().texture_image().unwrap();
            assert_eq!(image.format(), ImageFormat::IntensityAlpha);
            assert_eq!(image.size(), PixelSize::Bits32);
            assert_eq!(image.width(), 0x0abd);
            assert_eq!(image.address().get(), 0x200);
            let tile = decoded
                .staged_state()
                .tmem()
                .tile(crate::TileIndex::try_new(7).unwrap());
            let descriptor = tile.descriptor().unwrap();
            assert_eq!(descriptor.format(), ImageFormat::Intensity);
            assert_eq!(descriptor.size(), PixelSize::Bits32);
            assert_eq!(descriptor.line_words(), 0x01ab);
            assert_eq!(descriptor.tmem().get(), 0x01fe);
            assert_eq!(descriptor.palette(), 0x0f);
            assert!(descriptor.t_mode().mirror());
            assert!(descriptor.t_mode().clamp());
            assert_eq!(descriptor.mask_t(), 0x0a);
            assert_eq!(descriptor.shift_t(), 0x0b);
            assert!(descriptor.s_mode().mirror());
            assert!(!descriptor.s_mode().clamp());
            assert_eq!(descriptor.mask_s(), 0x0c);
            assert_eq!(descriptor.shift_s(), 0x0d);
            let size = tile.size().unwrap();
            assert_eq!(size.low_s().raw(), 0x0fed);
            assert_eq!(size.low_t().raw(), 0x0cba);
            assert_eq!(size.high_s().raw(), 0x0abc);
            assert_eq!(size.high_t().raw(), 0x0789);
            assert!(decoded
                .commands()
                .iter()
                .all(|command| command.location().wire_opcode() & 0xc0 == prefix));
        }
    }

    #[test]
    fn set_texture_image_decodes_all_26_public_address_bits() {
        let legal = vec![word(0, SET_TEXTURE_IMAGE, 2 << 19), 0xfc00_0200];
        let decoded = decode_raw_dpc(submit(packet(7, legal, &[])), &RdpState::default()).unwrap();
        assert_eq!(
            decoded
                .staged_state()
                .tmem()
                .texture_image()
                .unwrap()
                .address()
                .get(),
            0x200
        );

        for public_address_bit in [1 << 24, 1 << 25] {
            let words = vec![
                word(0, SET_TEXTURE_IMAGE, 2 << 19),
                public_address_bit | 0x200,
            ];
            assert!(matches!(
                decode_raw_dpc(submit(packet(7, words, &[])), &RdpState::default()),
                Err(RawDpcDecodeError::InvalidCommand { reason, .. })
                    if reason.contains("outside installed RDRAM")
            ));
        }
    }

    #[test]
    fn set_tile_preserves_an_earlier_tile_size() {
        let words = vec![
            word(0, SET_TILE_SIZE, 4 << 12 | 8),
            3 << 24 | 20 << 12 | 24,
            word(0, SET_TILE, 2 << 19 | 7 << 9 | 9),
            3 << 24,
        ];
        let decoded = decode_raw_dpc(submit(packet(7, words, &[])), &RdpState::default()).unwrap();
        let tile = decoded
            .staged_state()
            .tmem()
            .tile(crate::TileIndex::try_new(3).unwrap());

        assert!(tile.descriptor().is_some());
        assert_eq!(
            tile.size().unwrap(),
            crate::TileSize::from_wire(
                crate::TileCoordinate::try_new(4).unwrap(),
                crate::TileCoordinate::try_new(8).unwrap(),
                crate::TileCoordinate::try_new(20).unwrap(),
                crate::TileCoordinate::try_new(24).unwrap(),
            )
        );
    }

    #[test]
    fn staged_tmem_state_chains_across_immediate_packets() {
        let mut first_words = Vec::new();
        first_words.extend(set_texture_image(0, 0, 2, 8, 0x200));
        first_words.extend(set_tile(0, 7, 2, 0));
        first_words.extend(load_sync(0));
        let second_words = vec![word(0, LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800];
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let first = queue
            .submit(DecodedTicket::new(packet(7, first_words, &[])))
            .unwrap();
        let second = queue
            .submit(DecodedTicket::new(
                packet_with_unplanned_tmem_accesses_in_layout(
                    LAYOUT_BYTES,
                    8,
                    second_words,
                    &[(0x214, 0x224)],
                    &[(0, 8), (24, 32)],
                ),
            ))
            .unwrap();

        let staged = decode_raw_dpc(first, &RdpState::default())
            .unwrap()
            .into_staged_state();
        let decoded = decode_raw_dpc_after(second, staged).unwrap();
        let RawDpcCommandKind::LoadBlock(load) = decoded.commands()[0].kind() else {
            panic!("expected LoadBlock");
        };
        assert_eq!(load.epoch().get(), 1);
        assert_eq!(load.source_image().address().get(), 0x200);
        assert_eq!(load.tile().get(), 7);
        assert_eq!(decoded.staged_state().tmem().last_load(), Some(load));
    }

    #[test]
    fn latched_texture_image_rejects_staged_and_durable_cross_layout_loads() {
        let mut state_words = Vec::new();
        state_words.extend(set_texture_image(0, 0, 2, 8, 0x200));
        state_words.extend(set_tile(0, 7, 2, 0));
        state_words.extend(load_sync(0));
        let load_words = vec![word(0, LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800];

        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let first = queue
            .submit(DecodedTicket::new(packet_with_tmem_sources_in_layout(
                0x4000,
                7,
                state_words.clone(),
                &[],
            )))
            .unwrap();
        let second = queue
            .submit(DecodedTicket::new(
                packet_with_unplanned_tmem_accesses_in_layout(
                    0x5000,
                    8,
                    load_words.clone(),
                    &[(0x214, 0x224)],
                    &[(0, 16)],
                ),
            ))
            .unwrap();
        let staged = decode_raw_dpc(first, &RdpState::default())
            .unwrap()
            .into_staged_state();
        assert!(matches!(
            decode_raw_dpc_after(second, staged),
            Err(RawDpcDecodeError::InvalidCommand { reason, .. })
                if reason.contains("layout differs")
        ));

        let first = decode_raw_dpc(
            submit(packet_with_tmem_sources_in_layout(
                0x4000,
                7,
                state_words,
                &[],
            )),
            &RdpState::default(),
        )
        .unwrap();
        let mut durable = RdpState::default();
        durable.apply(first.state_delta());
        assert!(matches!(
            decode_raw_dpc(
                submit(packet_with_unplanned_tmem_accesses_in_layout(
                    0x5000,
                    8,
                    load_words,
                    &[(0x214, 0x224)],
                    &[(0, 16)],
                )),
                &durable,
            ),
            Err(RawDpcDecodeError::InvalidCommand { reason, .. })
                if reason.contains("layout differs")
        ));
    }

    #[test]
    fn load_block_binds_exact_source_access_owned_bytes_and_one_sync_epoch() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 0, 2, 8, 0x200));
        words.extend(set_tile(0, 7, 2, 0));
        words.extend(load_sync(0));
        words.extend([word(0, LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, words, &[(0x214, 0x224)])),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadBlock(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        assert_eq!(load.epoch().get(), 1);
        assert_eq!(load.source_plan().first_operation(), OperationId::new(1));
        assert_eq!(load.source_plan().access_count(), 1);
        assert_eq!(load.source_plan().total_bytes(), 16);
        assert_eq!(
            decoded
                .resource_plan()
                .tmem_load_source_accesses(load.source_plan())
                .unwrap(),
            [tmem_source_access(
                PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap(),
                1,
                0x214,
                0x224,
            )]
        );
        assert_eq!(decoded.submitted().packet().owned_guest_read_bytes(), 16);
        assert_eq!(
            decoded.submitted().packet().guest_reads().reads()[0].bytes(),
            [1; 16]
        );
        assert_eq!(decoded.staged_state().tmem().last_load(), Some(load));
        assert_eq!(decoded.staged_state().tmem().armed_load_sync(), None);

        let mut missing_second_sync = Vec::new();
        missing_second_sync.extend(set_texture_image(0, 0, 2, 8, 0x200));
        missing_second_sync.extend(set_tile(0, 7, 2, 0));
        missing_second_sync.extend(load_sync(0));
        missing_second_sync.extend([
            word(0, LOAD_BLOCK, 2 << 12 | 1),
            7 << 24 | 9 << 12 | 0x0800,
            word(0, LOAD_BLOCK, 2 << 12 | 1),
            7 << 24 | 9 << 12 | 0x0800,
        ]);
        assert!(matches!(
            decode_raw_dpc(
                submit(packet_with_unplanned_tmem_accesses_in_layout(
                    LAYOUT_BYTES,
                    8,
                    missing_second_sync,
                    &[(0x214, 0x224), (0x214, 0x224)],
                    &[],
                )),
                &RdpState::default(),
            ),
            Err(RawDpcDecodeError::InvalidCommand { reason, .. })
                if reason.contains("LoadSync")
        ));
    }

    #[test]
    fn load_block_enforces_tl_and_inclusive_count_boundaries() {
        let block_words = |width: u32, low_t: u32, high_s: u32| {
            let mut words = Vec::new();
            words.extend(set_texture_image(0, 0, 1, width, 0x200));
            words.extend(set_tile(0, 7, 1, 0));
            words.extend(load_sync(0));
            words.extend([word(0, LOAD_BLOCK, low_t), 7 << 24 | high_s << 12 | 0x0800]);
            words
        };

        decode_raw_dpc(
            submit(packet_with_tmem_sources(
                7,
                block_words(1, 1023, 0),
                &[(0x5ff, 0x600)],
            )),
            &RdpState::default(),
        )
        .unwrap();
        assert!(matches!(
            decode_raw_dpc(
                submit(packet(7, block_words(1, 1024, 0), &[])),
                &RdpState::default(),
            ),
            Err(RawDpcDecodeError::InvalidCommand { reason, .. })
                if reason.contains("ten-bit")
        ));

        decode_raw_dpc(
            submit(packet_with_tmem_sources(
                7,
                block_words(4096, 0, 2047),
                &[(0x200, 0xa00)],
            )),
            &RdpState::default(),
        )
        .unwrap();
        assert!(matches!(
            decode_raw_dpc(
                submit(packet(7, block_words(4096, 0, 2048), &[])),
                &RdpState::default(),
            ),
            Err(RawDpcDecodeError::InvalidCommand { reason, .. })
                if reason.contains("exceeds 2048")
        ));
    }

    #[test]
    fn load_block_plans_exact_32_bit_source_bytes() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 0, 3, 4, 0x200));
        words.extend(set_tile(0, 7, 1, 0));
        words.extend(load_sync(0));
        words.extend([word(0, LOAD_BLOCK, 1 << 12 | 1), 7 << 24 | 2 << 12]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, words, &[(0x214, 0x21c)])),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadBlock(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        assert_eq!(load.source_plan().total_bytes(), 8);
    }

    /// The final logical tail still records six padding bytes, while RT64
    /// `rt64_rdp.cpp:369-397` (`Copy the entire word`) makes every copied lane
    /// defined and keeps destination wrap order independent of the union.
    #[test]
    fn transfer_words_preserve_wrap_order_and_padded_tail_separately_from_union() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 0, 2, 8, 0x200));
        words.extend(set_tile(0, 7, 0, 511));
        words.extend(load_sync(0));
        words.extend([word(0, LOAD_BLOCK, 1), 7 << 24 | 4 << 12]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, words, &[(0x210, 0x220)])),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadBlock(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        let transfer = decoded.resource_plan().bind_tmem_transfer(load).unwrap();
        assert_eq!(load.transfer_plan().unwrap().logical_source_bytes(), 10);
        assert_eq!(load.transfer_plan().unwrap().written_bytes(), 16);
        assert_eq!(load.transfer_plan().unwrap().undefined_padding_bytes(), 6);
        assert_eq!(transfer.words().len(), 2);
        assert_eq!(transfer.words()[0].destination_word(), 511);
        assert_eq!(transfer.words()[1].destination_word(), 0);
        assert_eq!(transfer.words()[0].defined_source_byte_mask(), 0xff);
        assert_eq!(transfer.words()[1].defined_source_byte_mask(), 0xff);
        // **Neither word exchanges: both are tile-relative row 0.** This
        // LoadBlock carries `DXT = 0`, so no word advances a row, and the
        // block's own TL does not enter the exchange. This used to assert
        // that ALL words exchanged, which was the writer's removed
        // `source_t` term; see `tmem/read.rs::odd_row_exchange`.
        assert!(transfer.words().iter().all(|word| !word.odd_row_exchange()));
        assert_eq!(
            transfer
                .destination_accesses()
                .iter()
                .map(|access| access.region())
                .collect::<Vec<_>>(),
            vec![
                ResourceRegion::Tmem(fn64_render_ir::TmemRange::try_new(0, 8).unwrap()),
                ResourceRegion::Tmem(fn64_render_ir::TmemRange::try_new(4088, 4096).unwrap()),
            ]
        );
        assert_transfer_geometry_matches_destination_union(&decoded, load);
    }

    /// DXT still selects each destination row while every source word is fully
    /// defined by RT64 `rt64_rdp.cpp:369-397` (`Copy the entire word`).
    #[test]
    fn load_block_starting_tl_and_dxt_carry_select_each_word_row() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 0, 2, 16, 0x200));
        words.extend(set_tile(0, 7, 2, 10));
        words.extend(load_sync(0));
        words.extend([word(0, LOAD_BLOCK, 1), 7 << 24 | 8 << 12 | 0x0400]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, words, &[(0x220, 0x238)])),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadBlock(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        let transfer = decoded.resource_plan().bind_tmem_transfer(load).unwrap();
        assert_eq!(
            transfer
                .words()
                .iter()
                .map(|word| (
                    word.row_advance(),
                    word.destination_word(),
                    word.odd_row_exchange()
                ))
                .collect::<Vec<_>>(),
            // The exchange column is the TILE-RELATIVE row's parity alone --
            // words 0-1 are row 0, word 2 is row 1. This used to read
            // `true, true, false`, folding in the writer's removed
            // `source_t` term (TL is 1 here). See
            // `tmem/read.rs::odd_row_exchange` for the angrylion citation.
            vec![(0, 10, false), (0, 11, false), (1, 14, true)]
        );
        assert_eq!(transfer.words()[2].defined_source_byte_mask(), 0xff);
        assert_transfer_geometry_matches_destination_union(&decoded, load);
    }

    #[test]
    fn canonical_union_cannot_alias_distinct_transfer_order_or_detached_decode() {
        let decode = |source_t: u32, line: u32, base: u32, dxt: u32| {
            let mut words = Vec::new();
            words.extend(set_texture_image(0, 0, 2, 8, 0x200));
            words.extend(set_tile(0, 7, line, base));
            words.extend(load_sync(0));
            words.extend([word(0, LOAD_BLOCK, source_t), 7 << 24 | 4 << 12 | dxt]);
            let start = 0x200 + source_t * 16;
            decode_raw_dpc(
                submit(packet_with_tmem_sources(7, words, &[(start, start + 10)])),
                &RdpState::default(),
            )
            .unwrap()
        };
        let forward = decode(0, 0, 0, 0);
        let reverse = decode(1, 510, 1, 0x0800);
        let RawDpcCommandKind::LoadBlock(forward_load) = forward.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        let RawDpcCommandKind::LoadBlock(reverse_load) = reverse.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        let forward_bound = forward
            .resource_plan()
            .bind_tmem_transfer(forward_load)
            .unwrap();
        let reverse_bound = reverse
            .resource_plan()
            .bind_tmem_transfer(reverse_load)
            .unwrap();
        assert_eq!(
            forward_bound.destination_accesses(),
            reverse_bound.destination_accesses()
        );
        assert_eq!(
            forward_bound
                .words()
                .iter()
                .map(|word| word.destination_word())
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            reverse_bound
                .words()
                .iter()
                .map(|word| word.destination_word())
                .collect::<Vec<_>>(),
            vec![1, 0]
        );
        assert_ne!(forward_load.transfer_plan(), reverse_load.transfer_plan());

        let mut repeated_words = Vec::new();
        repeated_words.extend(set_texture_image(0, 0, 2, 16, 0x200));
        repeated_words.extend(set_tile(0, 7, 511, 0));
        repeated_words.extend(load_sync(0));
        repeated_words.extend([word(0, LOAD_BLOCK, 0), 7 << 24 | 8 << 12 | 0x0800]);
        let repeated = decode_raw_dpc(
            submit(packet_with_tmem_sources(
                7,
                repeated_words,
                &[(0x200, 0x212)],
            )),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadBlock(repeated_load) = repeated.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        let repeated_bound = repeated
            .resource_plan()
            .bind_tmem_transfer(repeated_load)
            .unwrap();
        assert_eq!(
            repeated_bound
                .words()
                .iter()
                .map(|word| word.destination_word())
                .collect::<Vec<_>>(),
            vec![0, 0, 0]
        );
        assert_eq!(repeated_load.transfer_plan().unwrap().written_bytes(), 24);
        assert_eq!(
            repeated_load
                .transfer_plan()
                .unwrap()
                .destination()
                .total_bytes(),
            8
        );
        assert_eq!(repeated_bound.destination_accesses().len(), 1);
        for (decoded, load) in [
            (&forward, forward_load),
            (&reverse, reverse_load),
            (&repeated, repeated_load),
        ] {
            assert_transfer_geometry_matches_destination_union(decoded, load);
        }
        assert_eq!(
            reverse
                .resource_plan()
                .bind_tmem_transfer(forward_load)
                .unwrap_err(),
            TmemLoadSourcePlanError::TransferNotDeclared
        );
    }

    #[test]
    fn rgba32_uses_texture_image_size_and_split_banks_despite_tile_size() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 0, 3, 2, 0x200));
        words.extend(set_tile(0, 7, 0, 255));
        words.extend(load_sync(0));
        words.extend([word(0, LOAD_BLOCK, 0), 7 << 24 | 1 << 12]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, words, &[(0x200, 0x208)])),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadBlock(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        assert_eq!(load.source_image().size(), PixelSize::Bits32);
        assert_eq!(load.tile_descriptor().size(), PixelSize::Bits16);
        let transfer = decoded.resource_plan().bind_tmem_transfer(load).unwrap();
        assert_eq!(
            load.transfer_plan().unwrap().layout(),
            crate::TmemTransferLayout::SplitBanks64
        );
        assert_eq!(
            transfer.words()[0].physical(),
            crate::TmemTransferPhysicalWord::SplitBanks {
                low: fn64_render_ir::TmemRange::try_new(2040, 2044).unwrap(),
                high: fn64_render_ir::TmemRange::try_new(4088, 4092).unwrap(),
            }
        );

        let mut odd_wrapped = Vec::new();
        odd_wrapped.extend(set_texture_image(0, 0, 3, 4, 0x200));
        odd_wrapped.extend(set_tile(0, 7, 0, 255));
        odd_wrapped.extend(load_sync(0));
        odd_wrapped.extend([word(0, LOAD_BLOCK, 1), 7 << 24 | 3 << 12]);
        let odd_wrapped = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, odd_wrapped, &[(0x210, 0x220)])),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadBlock(odd_load) = odd_wrapped.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        let odd_transfer = odd_wrapped
            .resource_plan()
            .bind_tmem_transfer(odd_load)
            .unwrap();
        assert_eq!(
            odd_transfer
                .words()
                .iter()
                .map(|word| (word.destination_word(), word.physical()))
                .collect::<Vec<_>>(),
            // **Both words are tile-relative row 0, so neither exchanges.**
            // This LoadBlock carries `DXT = 0`, and a word lands on row
            // `(word * dxt) >> 11`, so no word here advances a row -- the
            // block's own TL does not enter the exchange at all. The ranges
            // used to be the exchanged ones (`2044`/`4`), which was the
            // writer's removed `source_t` term; see
            // `tmem/read.rs::odd_row_exchange`.
            vec![
                (
                    255,
                    crate::TmemTransferPhysicalWord::SplitBanks {
                        low: fn64_render_ir::TmemRange::try_new(2040, 2044).unwrap(),
                        high: fn64_render_ir::TmemRange::try_new(4088, 4092).unwrap(),
                    },
                ),
                (
                    0,
                    crate::TmemTransferPhysicalWord::SplitBanks {
                        low: fn64_render_ir::TmemRange::try_new(0, 4).unwrap(),
                        high: fn64_render_ir::TmemRange::try_new(2048, 2052).unwrap(),
                    },
                ),
            ]
        );
        assert_eq!(
            odd_transfer
                .destination_accesses()
                .iter()
                .map(|access| access.region())
                .collect::<Vec<_>>(),
            // The union of the two unexchanged split-bank words above, in
            // ascending order. Shifted down four bytes with them.
            vec![
                ResourceRegion::Tmem(fn64_render_ir::TmemRange::try_new(0, 4).unwrap()),
                ResourceRegion::Tmem(fn64_render_ir::TmemRange::try_new(2040, 2044).unwrap()),
                ResourceRegion::Tmem(fn64_render_ir::TmemRange::try_new(2048, 2052).unwrap()),
                ResourceRegion::Tmem(fn64_render_ir::TmemRange::try_new(4088, 4092).unwrap()),
            ]
        );
        assert_transfer_geometry_matches_destination_union(&odd_wrapped, odd_load);

        let mut rejected = Vec::new();
        rejected.extend(set_texture_image(0, 0, 3, 2, 0x200));
        rejected.extend(set_tile(0, 7, 0, 256));
        rejected.extend(load_sync(0));
        rejected.extend([word(0, LOAD_BLOCK, 0), 7 << 24 | 1 << 12]);
        assert!(matches!(
            decode_raw_dpc(
                submit(packet_with_unplanned_tmem_accesses_in_layout(
                    LAYOUT_BYTES,
                    7,
                    rejected,
                    &[(0x200, 0x208)],
                    &[],
                )),
                &RdpState::default(),
            ),
            Err(RawDpcDecodeError::InvalidCommand { reason, .. })
                if reason.contains("outside low TMEM")
        ));
    }

    /// Tile rows bind independently and every transfer word is fully defined, as
    /// RT64 `rt64_rdp.cpp:369-397` (`Copy the entire word`) requires; row parity
    /// and logical offsets remain independently asserted.
    #[test]
    fn load_tile_retains_row_local_padded_words_and_source_row_parity() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 0, 2, 5, 0x200));
        words.extend(set_tile(0, 7, 3, 0));
        words.extend(load_sync(0));
        words.extend([word(0, LOAD_TILE, 4), 7 << 24 | 16 << 12 | 8]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(
                7,
                words,
                &[(0x20a, 0x21a), (0x214, 0x224)],
            )),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadTile(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadTile");
        };
        let transfer = decoded.resource_plan().bind_tmem_transfer(load).unwrap();
        assert_eq!(
            transfer
                .words()
                .iter()
                .map(|word| {
                    (
                        word.logical_source_offset(),
                        word.defined_source_byte_mask(),
                        word.destination_word(),
                        word.odd_row_exchange(),
                    )
                })
                .collect::<Vec<_>>(),
            // Words 0-1 are tile-relative row 0 and words 2-3 are row 1;
            // the parity column used to be inverted by the writer's removed
            // `low_t.integer()` term (1 for this fixture's S10.2 `low_t = 4`).
            vec![
                (0, 0xff, 0, false),
                (8, 0xff, 1, false),
                (10, 0xff, 3, true),
                (18, 0xff, 4, true)
            ]
        );
        assert_transfer_geometry_matches_destination_union(&decoded, load);
    }

    #[test]
    fn four_bit_yuv_and_tlut_destination_claims_stay_loudly_out_of_scope() {
        let mut four_bit = Vec::new();
        four_bit.extend(set_texture_image(0, 0, 0, 8, 0x200));
        four_bit.extend(set_tile(0, 7, 1, 0));
        four_bit.extend(load_sync(0));
        four_bit.extend([word(0, LOAD_BLOCK, 0), 7 << 24 | 7 << 12]);
        assert!(matches!(
            decode_raw_dpc(
                submit(packet_with_unplanned_tmem_accesses_in_layout(
                    LAYOUT_BYTES,
                    7,
                    four_bit,
                    &[(0x200, 0x204)],
                    &[],
                )),
                &RdpState::default(),
            ),
            Err(RawDpcDecodeError::InvalidCommand { reason, .. })
                if reason.contains("direct four-bit")
        ));

        let mut four_bit_tile = Vec::new();
        four_bit_tile.extend(set_texture_image(0, 0, 0, 8, 0x200));
        four_bit_tile.extend(set_tile(0, 7, 1, 0));
        four_bit_tile.extend(load_sync(0));
        four_bit_tile.extend([word(0, LOAD_TILE, 0), 7 << 24 | 28 << 12]);
        assert!(matches!(
            decode_raw_dpc(
                submit(packet_with_unplanned_tmem_accesses_in_layout(
                    LAYOUT_BYTES,
                    7,
                    four_bit_tile,
                    &[(0x200, 0x204)],
                    &[],
                )),
                &RdpState::default(),
            ),
            Err(RawDpcDecodeError::InvalidCommand { reason, .. })
                if reason.contains("direct four-bit")
        ));

        let mut unpaired_yuv = Vec::new();
        unpaired_yuv.extend(set_texture_image(0, 1, 2, 4, 0x200));
        unpaired_yuv.extend(set_tile(0, 7, 1, 0));
        unpaired_yuv.extend(load_sync(0));
        unpaired_yuv.extend([word(0, LOAD_BLOCK, 1 << 12), 7 << 24 | 2 << 12]);
        let yuv = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, unpaired_yuv, &[(0x202, 0x20a)])),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadBlock(yuv_load) = yuv.commands()[3].kind() else {
            panic!("expected YUV LoadBlock");
        };
        assert!(yuv_load.transfer_plan().is_err());
        assert_eq!(
            yuv.resource_plan()
                .bind_tmem_transfer(yuv_load)
                .unwrap_err(),
            TmemLoadSourcePlanError::YuvExecutionDeferred
        );
        assert!(yuv
            .resource_plan()
            .accesses()
            .iter()
            .all(|access| access.purpose() != AccessPurpose::TmemLoadDestination));

        let mut yuv_tile = Vec::new();
        yuv_tile.extend(set_texture_image(0, 1, 2, 4, 0x200));
        yuv_tile.extend(set_tile(0, 7, 1, 0));
        yuv_tile.extend(load_sync(0));
        yuv_tile.extend([word(0, LOAD_TILE, 4 << 12), 7 << 24 | 8 << 12 | 4]);
        let yuv_tile = decode_raw_dpc(
            submit(packet_with_tmem_sources(
                7,
                yuv_tile,
                &[(0x202, 0x20a), (0x20a, 0x212)],
            )),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadTile(yuv_tile_load) = yuv_tile.commands()[3].kind() else {
            panic!("expected YUV LoadTile");
        };
        let yuv_tile_sources = yuv_tile
            .resource_plan()
            .tmem_load_source_accesses(yuv_tile_load.source_plan())
            .unwrap();
        assert_eq!(yuv_tile_load.source_plan().access_count(), 2);
        assert_eq!(yuv_tile_load.source_plan().total_bytes(), 16);
        assert_eq!(
            yuv_tile_sources
                .iter()
                .map(|access| access.region())
                .collect::<Vec<_>>(),
            vec![
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range: PhysicalMemoryLayout::try_new(LAYOUT_BYTES)
                        .unwrap()
                        .range(0x202, 0x20a)
                        .unwrap(),
                },
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range: PhysicalMemoryLayout::try_new(LAYOUT_BYTES)
                        .unwrap()
                        .range(0x20a, 0x212)
                        .unwrap(),
                },
            ]
        );
        assert!(yuv_tile_load.transfer_plan().is_err());
        assert_eq!(
            yuv_tile
                .resource_plan()
                .bind_tmem_transfer(yuv_tile_load)
                .unwrap_err(),
            TmemLoadSourcePlanError::YuvExecutionDeferred
        );
        assert!(yuv_tile
            .resource_plan()
            .accesses()
            .iter()
            .all(|access| access.purpose() != AccessPurpose::TmemLoadDestination));

        let mut tlut = Vec::new();
        tlut.extend(set_texture_image(0, 0, 2, 1, 0x300));
        tlut.extend(set_tile(0, 7, 0, 256));
        tlut.extend(load_sync(0));
        tlut.extend([word(0, LOAD_TLUT, 0), 7 << 24 | 15 << 14]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, tlut, &[(0x300, 0x320)])),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadTlut(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadTLUT");
        };
        // M4.3.1 closes LoadTLUT's transfer plan: decode always produces a
        // real `Transfer`, so binding now succeeds and journals the exact
        // source accesses plus the high-half `TmemLoadDestination` word
        // slices -- no physical TMEM byte is claimed as written here, only
        // the wire-layer transfer geometry (M4.3.2 executes it).
        let plan = load
            .transfer_plan()
            .expect("M4.3.1 closes LoadTLUT's transfer plan");
        assert_eq!(plan.transfer_words(), 16);
        assert_eq!(plan.destination_word(0).unwrap(), 256);
        assert_eq!(plan.destination_word(15).unwrap(), 271);
        let transfer = decoded.resource_plan().bind_tmem_transfer(load).unwrap();
        assert_eq!(
            transfer
                .source_accesses()
                .iter()
                .map(|access| access.region())
                .collect::<Vec<_>>(),
            vec![ResourceRegion::Rdram {
                resource: RdramResource::Buffer,
                range: PhysicalMemoryLayout::try_new(LAYOUT_BYTES)
                    .unwrap()
                    .range(0x300, 0x320)
                    .unwrap(),
            }]
        );
        assert!(transfer
            .destination_accesses()
            .iter()
            .all(|access| access.purpose() == AccessPurpose::TmemLoadDestination));
        assert_transfer_geometry_matches_destination_union(&decoded, load);
    }

    #[test]
    fn tmem_source_plans_reject_cross_decode_range_and_layout_aliases() {
        let decode_block = |layout_bytes: u32, address: u32, range: (u32, u32)| {
            let mut words = Vec::new();
            words.extend(set_texture_image(0, 0, 2, 8, address));
            words.extend(set_tile(0, 7, 2, 0));
            words.extend(load_sync(0));
            words.extend([word(0, LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
            decode_raw_dpc(
                submit(packet_with_tmem_sources_in_layout(
                    layout_bytes,
                    7,
                    words,
                    &[range],
                )),
                &RdpState::default(),
            )
            .unwrap()
        };

        let first = decode_block(0x4000, 0x200, (0x214, 0x224));
        let different_range = decode_block(0x4000, 0x300, (0x314, 0x324));
        let RawDpcCommandKind::LoadBlock(first_load) = first.commands()[3].kind() else {
            panic!("expected LoadBlock");
        };
        let RawDpcCommandKind::LoadBlock(different_range_load) =
            different_range.commands()[3].kind()
        else {
            panic!("expected LoadBlock");
        };
        assert_ne!(
            first_load.source_plan().source_access_identity(),
            different_range_load.source_plan().source_access_identity()
        );
        assert_eq!(
            first
                .resource_plan()
                .tmem_load_source_accesses(different_range_load.source_plan()),
            Err(TmemLoadSourcePlanError::DecodeIdentityMismatch)
        );

        let different_layout = decode_block(0x5000, 0x200, (0x214, 0x224));
        let RawDpcCommandKind::LoadBlock(different_layout_load) =
            different_layout.commands()[3].kind()
        else {
            panic!("expected LoadBlock");
        };
        assert_ne!(
            first_load.source_plan().source_access_identity(),
            different_layout_load.source_plan().source_access_identity()
        );
        assert_eq!(
            first
                .resource_plan()
                .tmem_load_source_accesses(different_layout_load.source_plan()),
            Err(TmemLoadSourcePlanError::DecodeIdentityMismatch)
        );
    }

    /// Every tile row has one padded whole-word access, including full-width rows,
    /// matching RT64 `rt64_rdp.cpp:369-397` (`Copy the entire word`).
    #[test]
    fn load_tile_plans_one_padded_access_per_row_for_fractional_and_full_rows() {
        let mut subrows = Vec::new();
        subrows.extend(set_texture_image(0, 2, 1, 9, 0x200));
        subrows.extend(set_tile(0, 3, 1, 0));
        subrows.extend(load_sync(0));
        subrows.extend([word(0, LOAD_TILE, 5 << 12 | 8), 3 << 24 | 15 << 12 | 15]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(
                7,
                subrows,
                &[(0x213, 0x21b), (0x21c, 0x224)],
            )),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadTile(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadTile");
        };
        assert_eq!(load.source_plan().access_count(), 2);
        assert_eq!(load.source_plan().total_bytes(), 16);
        let TmemLoadKind::Tile { bounds } = load.kind() else {
            panic!("expected tile bounds");
        };
        assert_eq!(bounds.low_s().raw(), 5);
        assert_eq!(bounds.low_t().raw(), 8);
        assert_eq!(bounds.high_s().raw(), 15);
        assert_eq!(bounds.high_t().raw(), 15);

        let mut full_rows = Vec::new();
        full_rows.extend(set_texture_image(0, 3, 1, 4, 0x300));
        full_rows.extend(set_tile(0, 2, 1, 0));
        full_rows.extend(load_sync(0));
        full_rows.extend([word(0, LOAD_TILE, 0), 2 << 24 | 12 << 12 | 4]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(
                7,
                full_rows,
                &[(0x300, 0x308), (0x304, 0x30c)],
            )),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadTile(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadTile");
        };
        assert_eq!(load.source_plan().access_count(), 2);
        assert_eq!(load.source_plan().total_bytes(), 16);
    }

    #[test]
    fn load_tlut_uses_all_ten_count_bits_and_rejects_the_257th_entry() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 0, 2, 1, 0x300));
        words.extend(set_tile(0, 7, 0, 256));
        words.extend(load_sync(0));
        words.extend([word(0, LOAD_TLUT, 0), 7 << 24 | 255 << 14]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, words, &[(0x300, 0x500)])),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadTlut(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadTLUT");
        };
        let TmemLoadKind::Tlut { entries, .. } = load.kind() else {
            panic!("expected TLUT load kind");
        };
        assert_eq!(entries.get(), 256);
        assert_eq!(load.source_plan().total_bytes(), 512);

        let mut too_many = Vec::new();
        too_many.extend(set_texture_image(0, 0, 2, 1, 0x300));
        too_many.extend(set_tile(0, 7, 0, 256));
        too_many.extend(load_sync(0));
        too_many.extend([word(0, LOAD_TLUT, 0), 7 << 24 | 256 << 14]);
        assert!(matches!(
            decode_raw_dpc(
                submit(packet(7, too_many, &[])),
                &RdpState::default()
            ),
            Err(RawDpcDecodeError::InvalidCommand { reason, .. })
                if reason.contains("256-entry")
        ));
    }

    /// The canonical libultra `gDPLoadTLUT_pal16` destination tile is
    /// `siz == G_IM_SIZ_4b`, and must decode.
    ///
    /// Public libultra `gbi.h` (identical in four independent SDK copies on
    /// this machine: `sm64-decomp/include/PR/gbi.h:4229`,
    /// `mm-decomp/include/PR/gbi.h:4655`,
    /// `kirby64-decomp/include/PR/gbi.h:4239`,
    /// `oot-decomp/include/ultra64/gbi.h:4657`) expands
    /// `gDPLoadTLUT_pal16(pkt, pal, dram)` to
    /// `gDPSetTextureImage(pkt, G_IM_FMT_RGBA, G_IM_SIZ_16b, 1, dram)`
    /// followed by
    /// `gDPSetTile(pkt, 0, 0, 0, (256+((pal&0xf)*16)), G_TX_LOADTILE, ...)`.
    /// `gDPSetTile`'s parameter order is `(pkt, fmt, siz, line, tmem, tile,
    /// ...)` (`sm64-decomp/include/PR/gbi.h:3401`), so that second `0` is
    /// `siz`, and `G_IM_SIZ_4b == 0` (`:410`). `gDPLoadTLUT_pal256` (`:4283`)
    /// and the generic `gDPLoadTLUT` (`:4331`) program the same `siz == 0`.
    ///
    /// Fail-against-bug: an earlier revision required the DESTINATION
    /// descriptor to be `Bits16` and refused every canonical macro emission --
    /// including all seven `LoadTLUT`s in WWF WrestleMania 2000's captured
    /// frame-0 packet. The sibling SOURCE-image check is the correct one and
    /// is asserted here too, so deleting either check fails this test.
    ///
    /// The `set_tile` helper hardcodes `siz == 2`, which is why no existing
    /// fixture could reach this shape; these cases build the word directly.
    #[test]
    fn load_tlut_accepts_the_canonical_macro_four_bit_destination_descriptor() {
        // `gDPSetTile(pkt, fmt=0, siz, line=0, tmem=256, tile=7, ...)`.
        let set_tile_siz = |siz: u32| [word(0, SET_TILE, siz << 19 | 256), 7 << 24];

        // Every destination `siz` decodes: the field describes a TMEM region
        // for a quadricated palette write, not the palette's pixel format,
        // and nothing downstream consumes it for this load kind.
        for siz in 0..=3 {
            let mut words = Vec::new();
            words.extend(set_texture_image(0, 0, 2, 1, 0x300));
            words.extend(set_tile_siz(siz));
            words.extend(load_sync(0));
            words.extend([word(0, LOAD_TLUT, 0), 7 << 24 | 15 << 14]);
            let decoded = decode_raw_dpc(
                submit(packet_with_tmem_sources(7, words, &[(0x300, 0x320)])),
                &RdpState::default(),
            )
            .unwrap_or_else(|error| panic!("siz={siz} must decode: {error}"));
            let RawDpcCommandKind::LoadTlut(load) = decoded.commands()[3].kind() else {
                panic!("siz={siz}: expected LoadTLUT");
            };
            let TmemLoadKind::Tlut { entries, .. } = load.kind() else {
                panic!("siz={siz}: expected TLUT load kind");
            };
            // The transfer is sized from the SOURCE image and the entry
            // count, never from the destination descriptor -- so the shape
            // is identical across all four destination sizes.
            assert_eq!(entries.get(), 16, "siz={siz}");
            assert_eq!(load.source_plan().total_bytes(), 32, "siz={siz}");
        }

        // The sibling SOURCE check remains, and is the one the macro
        // actually constrains: a non-16-bit `SetTextureImage` is refused
        // even with the canonical 4-bit destination descriptor.
        for source_siz in [0, 1, 3] {
            let mut words = Vec::new();
            words.extend(set_texture_image(0, 0, source_siz, 1, 0x300));
            words.extend(set_tile_siz(0));
            words.extend(load_sync(0));
            words.extend([word(0, LOAD_TLUT, 0), 7 << 24 | 15 << 14]);
            let error =
                decode_raw_dpc(submit(packet(7, words, &[])), &RdpState::default()).unwrap_err();
            assert!(
                matches!(
                    error,
                    RawDpcDecodeError::InvalidCommand { reason: ref actual, .. }
                        if actual.contains("16-bit SetTextureImage source")
                ),
                "source siz={source_siz}: {error}"
            );
        }
    }

    #[test]
    fn load_tlut_rejects_every_non_macro_coordinate_field() {
        for (name, w0_payload, w1_payload, reason) in [
            ("SL integer", 4 << 12, 0, "zero SL origin"),
            ("TL integer", 4, 0, "zero TL origin"),
            ("SL fraction", 1 << 12, 0, "fractional"),
            ("TL fraction", 1, 0, "fractional"),
            ("count fraction", 0, 1 << 12, "fractional"),
            ("TH", 0, 1, "zero TH"),
        ] {
            let mut words = Vec::new();
            words.extend(set_texture_image(0, 0, 2, 1, 0x300));
            words.extend(set_tile(0, 7, 0, 256));
            words.extend(load_sync(0));
            words.extend([word(0, LOAD_TLUT, w0_payload), 7 << 24 | w1_payload]);
            let error =
                decode_raw_dpc(submit(packet(7, words, &[])), &RdpState::default()).unwrap_err();
            assert!(
                matches!(error, RawDpcDecodeError::InvalidCommand { reason: actual, .. } if actual.contains(reason)),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn every_missing_tmem_load_precondition_is_loud_without_durable_publication() {
        let durable = RdpState::default();
        let cases = [
            vec![word(0, LOAD_BLOCK, 0), 0],
            {
                let mut words = Vec::new();
                words.extend(load_sync(0));
                words.extend([word(0, LOAD_BLOCK, 0), 0]);
                words
            },
            {
                let mut words = Vec::new();
                words.extend(set_texture_image(0, 0, 2, 1, 0x200));
                words.extend(load_sync(0));
                words.extend([word(0, LOAD_BLOCK, 0), 0]);
                words
            },
        ];
        for words in cases {
            assert!(matches!(
                decode_raw_dpc(submit(packet(7, words, &[])), &durable),
                Err(RawDpcDecodeError::InvalidCommand { .. })
            ));
            assert_eq!(durable, RdpState::default());
        }
    }

    #[test]
    fn wrong_tmem_source_range_is_an_exact_journal_rejection() {
        let durable = RdpState::default();
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 0, 2, 8, 0x200));
        words.extend(set_tile(0, 7, 2, 0));
        words.extend(load_sync(0));
        words.extend([word(0, LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12]);
        let error = decode_raw_dpc(
            submit(packet_with_unplanned_tmem_accesses_in_layout(
                LAYOUT_BYTES,
                7,
                words,
                &[(0x214, 0x222)],
                &[],
            )),
            &durable,
        )
        .unwrap_err();
        let RawDpcDecodeError::JournalMismatch {
            expected, actual, ..
        } = error
        else {
            panic!("expected exact journal mismatch");
        };
        assert_eq!(
            expected[1].region(),
            ResourceRegion::Rdram {
                resource: RdramResource::Buffer,
                range: PhysicalMemoryLayout::try_new(LAYOUT_BYTES)
                    .unwrap()
                    .range(0x214, 0x224)
                    .unwrap(),
            }
        );
        assert_eq!(
            actual[1].region(),
            ResourceRegion::Rdram {
                resource: RdramResource::Buffer,
                range: PhysicalMemoryLayout::try_new(LAYOUT_BYTES)
                    .unwrap()
                    .range(0x214, 0x222)
                    .unwrap(),
            }
        );
        assert_eq!(durable, RdpState::default());
    }

    // --- triangle decode wired into the real command stream ---

    fn triangle_base_word0(prefix: u8, opcode: u8, tile: u32, level: u32, yl: u16) -> u32 {
        crate::wire_words::EdgeWords {
            tile,
            level,
            yl: yl as i16,
            ..crate::wire_words::EdgeWords::zeroed()
        }
        .word0(prefix, opcode)
    }

    /// A base-edge (0x08) triangle decoded from a real command stream, with
    /// a `FullSync` immediately following, proves the decoder proves the
    /// exact 32-byte boundary rather than guessing a stride: if triangle
    /// decode consumed the wrong width, the following command would either
    /// re-read triangle payload as an opcode (desync) or fail to reach
    /// `FullSync` at all.
    #[test]
    fn base_edge_triangle_frames_exactly_against_a_following_full_sync() {
        let prefix = 0;
        let mut words = state_words(prefix);
        words.extend([
            triangle_base_word0(prefix, 0x08, 3, 2, 0x1234),
            (0x5678u32) << 16 | 0x9abc,
            0x0011_2233,
            0xffbb_ccdd,
            0x0044_5566,
            0xff99_8877,
            0x0077_8899,
            0xff11_2233,
        ]);
        words.extend([word(prefix, FULL_SYNC, 0), 0]);

        let submitted = submit(packet(9, words, &[]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();

        assert_eq!(decoded.commands().len(), 5);
        let RawDpcCommandKind::RawTriangle(triangle) = decoded.commands()[3].kind() else {
            panic!("fourth command must decode as RawTriangle");
        };
        assert_eq!(triangle.tile().get(), 3);
        assert_eq!(triangle.level(), 2);
        assert_eq!(triangle.yl(), 0x1234);
        assert_eq!(triangle.ym(), 0x5678u16 as i16);
        assert_eq!(triangle.yh(), 0x9abcu16 as i16);
        assert!(triangle.shade().is_none());
        assert!(triangle.texture().is_none());
        assert!(triangle.depth().is_none());
        assert!(matches!(
            decoded.commands()[4].kind(),
            RawDpcCommandKind::FullSync(_)
        ));
        // The next command's decoded source offset must be exactly 32 bytes
        // (4 words) past the triangle's own offset -- the base-edge width,
        // proving no coefficient byte was misread as part of the boundary.
        assert_eq!(
            decoded.commands()[4].location().stream_byte_offset()
                - decoded.commands()[3].location().stream_byte_offset(),
            32
        );
    }

    /// A fully-populated (0x0f) triangle decoded from a real command stream
    /// consumes exactly 176 bytes before a following `FullSync` is reached,
    /// exercising the full base+shade+texture+depth block order end to end.
    #[test]
    fn fully_populated_triangle_frames_exactly_against_a_following_full_sync() {
        let prefix = 0;
        let mut words = state_words(prefix);
        let mut triangle_words = vec![
            triangle_base_word0(prefix, 0x0f, 1, 0, 10),
            (20u32) << 16 | 30,
            1,
            2,
            3,
            4,
            5,
            6,
        ];
        // 8 shade words + 8 texture words + 2 depth words = 18 more 64-bit
        // words (36 more u32 halves), for 22 words / 44 halves total.
        triangle_words.extend((0..36u32).map(|index| 0x1000_0000 + index));
        assert_eq!(triangle_words.len(), 44);
        words.extend(triangle_words);
        words.extend([word(prefix, FULL_SYNC, 0), 0]);

        let submitted = submit(packet(9, words, &[]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();

        assert_eq!(decoded.commands().len(), 5);
        let RawDpcCommandKind::RawTriangle(triangle) = decoded.commands()[3].kind() else {
            panic!("fourth command must decode as RawTriangle");
        };
        assert!(triangle.shade().is_some());
        assert!(triangle.texture().is_some());
        assert!(triangle.depth().is_some());
        assert_eq!(
            decoded.commands()[4].location().stream_byte_offset()
                - decoded.commands()[3].location().stream_byte_offset(),
            176
        );
        assert!(matches!(
            decoded.commands()[4].kind(),
            RawDpcCommandKind::FullSync(_)
        ));
    }

    // -- Texture rectangle decode wired into the real command stream --

    /// One `TextureRectangle`/`TextureRectangleFlip` command's 4-word wire
    /// payload, following `texture_rectangle.rs`'s own bit layout exactly:
    /// word 0 = `(lrx << 12) | lry`, word 1 = `(tile << 24) | (ulx << 12) |
    /// uly`, word 2 = `(uls << 16) | ult`, word 3 = `(dsdx << 16) | dtdy`.
    /// The frozen card fixture: `ulx=0x0040` (16.0px), `uly=0`,
    /// `lrx=0x0100` (64.0px), `lry=0x00C0` (48.0px) -- a 48x48 rectangle,
    /// upper-left `(16, 0)`, lower-right `(64, 48)`.
    fn texrect_words(prefix: u8, opcode: u8, tile: u32) -> [u32; 4] {
        let ulx: u32 = 0x0040;
        let uly: u32 = 0;
        let lrx: u32 = 0x0100;
        let lry: u32 = 0x00c0;
        let uls: u32 = 0;
        let ult: u32 = 0;
        let dsdx: u32 = 0x0100;
        let dtdy: u32 = 0x0100;
        [
            word(prefix, opcode, (lrx << 12) | lry),
            (tile & 0x7) << 24 | (ulx << 12) | uly,
            (uls << 16) | ult,
            (dsdx << 16) | dtdy,
        ]
    }

    /// A `TextureRectangle` (`0x24`) decoded from a real command stream
    /// frames exactly against a following `FullSync` at the fixed 16-byte
    /// boundary `raw_rdp_command_width` declares for both texrect opcodes --
    /// proving the dispatch arm added at `decode_stream`'s `TEXRECT |
    /// TEXRECT_FLIP` match consumes exactly 4 words, never truncating to
    /// the 2-word tmem/state stride or over-reading into the next command.
    #[test]
    fn texture_rectangle_frames_exactly_against_a_following_full_sync() {
        let prefix = 0;
        let mut words = state_words(prefix);
        words.extend(texrect_words(prefix, TEXRECT, 0));
        words.extend([word(prefix, FULL_SYNC, 0), 0]);

        let submitted = submit(packet(9, words, &[]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();

        assert_eq!(decoded.commands().len(), 5);
        let RawDpcCommandKind::TextureRectangle(rectangle) = decoded.commands()[3].kind() else {
            panic!("fourth command must decode as TextureRectangle");
        };
        assert_eq!(rectangle.ulx(), 0x0040);
        assert_eq!(rectangle.uly(), 0);
        assert_eq!(rectangle.lrx(), 0x0100);
        assert_eq!(rectangle.lry(), 0x00c0);
        assert_eq!(rectangle.tile(), 0);
        assert_eq!(rectangle.uls(), 0);
        assert_eq!(rectangle.ult(), 0);
        assert_eq!(rectangle.dsdx(), 0x0100);
        assert_eq!(rectangle.dtdy(), 0x0100);
        assert!(!rectangle.flip(), "opcode 0x24 must decode flip=false");
        assert!(matches!(
            decoded.commands()[4].kind(),
            RawDpcCommandKind::FullSync(_)
        ));
        // Exactly 16 bytes (4 words) between this command and the next --
        // proving no coefficient byte was misread as part of the boundary
        // and no truncation to the fixed-2-word tmem/state stride occurred.
        assert_eq!(
            decoded.commands()[4].location().stream_byte_offset()
                - decoded.commands()[3].location().stream_byte_offset(),
            16
        );
    }

    /// `TextureRectangleFlip` (`0x25`) decodes with `flip=true` and the same
    /// exact 16-byte framing as `0x24` -- the two opcodes share one wire
    /// shape and differ only in the derived `flip` field (mod.rs's
    /// `TEXRECT | TEXRECT_FLIP` dispatch arm, `RawTextureRectangle::decode`'s
    /// own opcode match), never a wire bit.
    #[test]
    fn texture_rectangle_flip_decodes_flip_true_with_identical_framing() {
        let prefix = 0;
        let mut words = state_words(prefix);
        words.extend(texrect_words(prefix, TEXRECT_FLIP, 0));
        words.extend([word(prefix, FULL_SYNC, 0), 0]);

        let submitted = submit(packet(9, words, &[]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();

        assert_eq!(decoded.commands().len(), 5);
        let RawDpcCommandKind::TextureRectangle(rectangle) = decoded.commands()[3].kind() else {
            panic!("fourth command must decode as TextureRectangle");
        };
        assert!(rectangle.flip(), "opcode 0x25 must decode flip=true");
        assert_eq!(
            decoded.commands()[4].location().stream_byte_offset()
                - decoded.commands()[3].location().stream_byte_offset(),
            16,
            "TextureRectangleFlip must frame exactly like TextureRectangle"
        );
    }

    // -- Fragment constant registers (SetEnvColor/SetPrimColor/
    // SetBlendColor/SetFogColor/SetPrimDepth) --------------------------

    #[test]
    fn set_env_color_decodes_exact_rgba_byte_order_from_w1_independent_of_w0() {
        let prefix = 0x80;
        // w0 carries only the opcode byte for this command (its low 24 bits
        // are unused by SetEnvColor); set them to a hostile nonzero pattern
        // to prove they are never consulted.
        let words = vec![word(prefix, SET_ENV_COLOR, 0x00ab_cdef), 0x11223344];
        let decoded = decode(words).unwrap();
        let RawDpcCommandKind::SetEnvColor(color) = decoded.commands()[0].kind() else {
            panic!("expected SetEnvColor");
        };
        assert_eq!(color.rgba8(), [0x11, 0x22, 0x33, 0x44]);
        assert_eq!(decoded.staged_state().env_color(), color);
    }

    #[test]
    fn set_env_color_zero_and_max_boundaries() {
        let prefix = 0x40;
        let decoded_zero = decode(vec![word(prefix, SET_ENV_COLOR, 0), 0]).unwrap();
        let RawDpcCommandKind::SetEnvColor(zero) = decoded_zero.commands()[0].kind() else {
            panic!("expected SetEnvColor");
        };
        assert_eq!(zero.rgba8(), [0, 0, 0, 0]);

        let decoded_max = decode(vec![word(prefix, SET_ENV_COLOR, 0), u32::MAX]).unwrap();
        let RawDpcCommandKind::SetEnvColor(max) = decoded_max.commands()[0].kind() else {
            panic!("expected SetEnvColor");
        };
        assert_eq!(max.rgba8(), [0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn set_blend_color_decodes_exact_rgba_byte_order() {
        let prefix = 0;
        let words = vec![word(prefix, SET_BLEND_COLOR, 0), 0xAABBCCDD];
        let decoded = decode(words).unwrap();
        let RawDpcCommandKind::SetBlendColor(color) = decoded.commands()[0].kind() else {
            panic!("expected SetBlendColor");
        };
        assert_eq!(color.rgba8(), [0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(decoded.staged_state().blend_color(), color);
    }

    #[test]
    fn set_fog_color_decodes_exact_rgba_byte_order() {
        let prefix = 0xc0;
        let words = vec![word(prefix, SET_FOG_COLOR, 0), 0x01020304];
        let decoded = decode(words).unwrap();
        let RawDpcCommandKind::SetFogColor(color) = decoded.commands()[0].kind() else {
            panic!("expected SetFogColor");
        };
        assert_eq!(color.rgba8(), [0x01, 0x02, 0x03, 0x04]);
        assert_eq!(decoded.staged_state().fog_color(), color);
    }

    // -- SetScissor (tracked state only) --------------------------------

    /// Hand-derived wire words for one `SetScissor`. `w0` payload packs
    /// `ulx << 12 | uly`; `w1` packs `mode << 24 | lrx << 12 | lry`.
    fn set_scissor_words(
        prefix: u8,
        mode: u32,
        ulx: u32,
        uly: u32,
        lrx: u32,
        lry: u32,
    ) -> Vec<u32> {
        vec![
            word(prefix, SET_SCISSOR, ulx << 12 | uly),
            mode << 24 | lrx << 12 | lry,
        ]
    }

    /// **The DECLARED write rows follow the scissor, not just the painted
    /// ones.**
    ///
    /// Mutation-driven: making `plan_fill` declare the unclipped extent --
    /// while the executor still clips -- survived the whole unit suite and
    /// the differential sweep. It is nonetheless the hazard `plan_fill`'s
    /// own doc names: `fill_completed_writes` slices the full-extent buffer
    /// for every declared range WITHOUT checking the raster touched it, so a
    /// declared-but-unpainted row publishes a real digest of whatever the
    /// buffer held and carries it into guest RDRAM.
    ///
    /// Pinned on `clip_fill_rows_to_scissor` directly rather than through a
    /// decoded packet: `plan_fill` is reached by two decode passes and the
    /// retained plan is not the one a journal probe reports, so a
    /// packet-level fixture asserts the wrong pass. This is the function
    /// whose result `plan_render_target_rows` is handed.
    ///
    /// Every expectation is hand-derived from the wire. The scissor latches
    /// quarter-pixels and each edge becomes `ceil(q / 4)`
    /// (`RdpScissorRect::quarter_to_pixel_ceil`, derived from angrylion
    /// `rasterizer.c:2349-2363` for X and `:2284-2305` for Y).
    #[test]
    fn a_scissored_fill_declares_only_the_rows_that_survive_the_clip() {
        let rect = |x0, y0, x1, y1| RenderTargetRectangle { x0, y0, x1, y1 };
        // Rows 0..=1 requested; scissor `lry = 4` admits `ceil(4/4) = 1`
        // row, so only row 0 survives. Full width either way.
        assert_eq!(
            clip_fill_rows_to_scissor(
                rect(0, 0, 3, 1),
                Some(crate::targets::RdpScissorRect::from_wire_quarter_pixels(
                    0, 0, 0, 16, 4
                )),
            ),
            Some(rect(0, 0, 3, 0)),
            "the high row edge must clip y1 from 1 to 0"
        );
        // The X counterpart, asserted separately: a clip correct in Y and
        // absent in X still narrows the rectangle, so one combined case
        // would pass with either axis broken.
        assert_eq!(
            clip_fill_rows_to_scissor(
                rect(0, 0, 3, 1),
                Some(crate::targets::RdpScissorRect::from_wire_quarter_pixels(
                    0, 0, 0, 8, 16
                )),
            ),
            Some(rect(0, 0, 1, 1)),
            "the high column edge must clip x1 from 3 to 1"
        );
        // Low edges, which a backend clamping only lrx/lry would miss.
        assert_eq!(
            clip_fill_rows_to_scissor(
                rect(0, 0, 3, 3),
                Some(crate::targets::RdpScissorRect::from_wire_quarter_pixels(
                    0, 4, 8, 16, 16
                )),
            ),
            Some(rect(1, 2, 3, 3)),
            "ulx = 4 -> first column 1; uly = 8 -> first row 2"
        );
        // Nothing survives: the rectangle sits entirely right of the
        // scissor, which must be `None` rather than a silently empty rect.
        assert_eq!(
            clip_fill_rows_to_scissor(
                rect(2, 0, 3, 1),
                Some(crate::targets::RdpScissorRect::from_wire_quarter_pixels(
                    0, 0, 0, 4, 16
                )),
            ),
            None,
            "an empty intersection declares nothing"
        );
        // No scissor staged: the rectangle passes through untouched, so an
        // unscissored stream declares exactly what it always did.
        assert_eq!(
            clip_fill_rows_to_scissor(rect(0, 0, 3, 1), None),
            Some(rect(0, 0, 3, 1)),
            "absent scissor is not an empty scissor"
        );
    }

    #[test]
    fn set_scissor_is_admitted_rather_than_rejected_as_unsupported() {
        // Before admission this exact stream returned
        // `UnsupportedCommand { decoded_opcode: 0x2d, width: 8 }`.
        let decoded = decode(set_scissor_words(0xc0, 0, 0, 0, 0, 0))
            .expect("SetScissor must decode, not reject as UnsupportedCommand");
        assert_eq!(decoded.commands().len(), 1);
        assert!(matches!(
            decoded.commands()[0].kind(),
            RawDpcCommandKind::SetScissor(_)
        ));
    }

    #[test]
    fn set_scissor_opcode_constant_is_the_low_six_bits_of_g_setscissor() {
        // `G_SETSCISSOR` is 0xED (fn64-render-reference's `gbi::wire`);
        // the raw-DPC decoder keys on `opcode & 0x3f`.
        assert_eq!(SET_SCISSOR, 0x2d);
        assert_eq!(SET_SCISSOR, 0xedu8 & 0x3f);
    }

    /// **`SetScissor` now STAGES, where it used to be tracked-only.**
    ///
    /// The tracked-only shape is exactly what made a texrect overhanging
    /// the framebuffer a refusal rather than a clip: with no latched rect,
    /// `execute_texture_rectangle` had nothing to clip against. angrylion
    /// latches the four fields into `wstate->clip` in `rdp_set_scissor`
    /// (`rasterizer.c:2779-2784`) and the edgewalker clips every span
    /// against them (`:2349-2363` for X, `:2284-2305` for Y).
    ///
    /// Asserts the staged rect field by field, in the quarter-pixel wire
    /// units the command carries, using four DISTINCT values so a staging
    /// path that transposed two of them cannot pass.
    #[test]
    fn set_scissor_stages_its_rect_into_durable_rdp_state() {
        let decoded = decode(set_scissor_words(0x80, 2, 0x123, 0x456, 0x789, 0xABC)).unwrap();
        let staged = decoded
            .staged_state()
            .scissor()
            .expect("SetScissor stages a rect");
        assert_eq!(staged.mode(), 2);
        assert_eq!(staged.upper_left_x(), 0x123);
        assert_eq!(staged.upper_left_y(), 0x456);
        assert_eq!(staged.lower_right_x(), 0x789);
        assert_eq!(staged.lower_right_y(), 0xABC);
    }

    /// A stream with no `SetScissor` stages none -- the consumer's own
    /// fallback (the colour target's extent) applies, rather than a
    /// fabricated rect latched here.
    /// **A scissor latched by an earlier packet survives into a later
    /// one.** `decode_raw_dpc` decodes against
    /// `durable_state.fork_for_decode()`, so a fork that dropped the rect
    /// would unscissor every packet after the one that set it -- and a
    /// display list commonly sets the scissor once per frame and then
    /// submits several packets under it.
    ///
    /// The second packet deliberately issues a DIFFERENT state command
    /// (`SetEnvColor`) so its own decode really does run and really does
    /// stage something, ruling out a vacuous pass.
    #[test]
    fn a_scissor_from_an_earlier_packet_survives_the_fork_into_a_later_one() {
        let first = decode(set_scissor_words(0x80, 2, 0x123, 0x456, 0x789, 0xABC)).unwrap();
        let latched = first
            .staged_state()
            .scissor()
            .expect("the first packet stages a rect");

        // Feed the first packet's result forward as the second's durable
        // state, which is what the backend does between packets.
        let mut durable = RdpState::default();
        let mut delta = RdpStateDelta::default();
        delta.set_scissor(latched);
        durable.apply(&delta);

        let submitted = submit(packet(7, vec![word(0, SET_ENV_COLOR, 0), 0x1122_3344], &[]));
        let second = decode_raw_dpc(submitted, &durable).unwrap();
        assert_eq!(
            second.staged_state().scissor(),
            Some(latched),
            "the second packet issued no SetScissor and must inherit the first packet's rect"
        );
        // The second packet's own command staged too, so the decode ran.
        assert_eq!(
            second.staged_state().env_color().rgba8(),
            [0x11, 0x22, 0x33, 0x44]
        );
    }

    #[test]
    fn a_stream_with_no_set_scissor_stages_no_rect() {
        let words = vec![word(0, SET_BLEND_COLOR, 0), 0xAABBCCDD];
        let decoded = decode(words).unwrap();
        assert_eq!(decoded.staged_state().scissor(), None);
    }

    #[test]
    fn set_scissor_decodes_each_field_from_its_own_wire_position() {
        // ulx = 0x123 (291), uly = 0x456 (1110) from w0's low 24 bits;
        // mode = 2, lrx = 0x789 (1929), lry = 0xABC (2748) from w1.
        let decoded = decode(set_scissor_words(0x80, 2, 0x123, 0x456, 0x789, 0xABC)).unwrap();
        let RawDpcCommandKind::SetScissor(scissor) = decoded.commands()[0].kind() else {
            panic!("expected SetScissor");
        };
        assert_eq!(scissor.mode, 2);
        assert_eq!(scissor.ulx, 0x123);
        assert_eq!(scissor.uly, 0x456);
        assert_eq!(scissor.lrx, 0x789);
        assert_eq!(scissor.lry, 0xABC);
    }

    #[test]
    fn set_scissor_fields_saturate_at_their_own_widths_and_never_go_negative() {
        // Every wire bit set: the opcode byte still selects SetScissor
        // (0xff & 0x3f == 0x3f is SetColorImage, so build the word
        // explicitly rather than flooding w0's top byte). mode is 2 bits
        // (max 3), each coordinate is 12 bits (max 0xFFF = 4095).
        let words = vec![word(0xc0, SET_SCISSOR, 0x00ff_ffff), 0xffff_ffff];
        let decoded = decode(words).unwrap();
        let RawDpcCommandKind::SetScissor(scissor) = decoded.commands()[0].kind() else {
            panic!("expected SetScissor");
        };
        assert_eq!(scissor.mode, 3);
        assert_eq!(scissor.ulx, 0xFFF);
        assert_eq!(scissor.uly, 0xFFF);
        assert_eq!(scissor.lrx, 0xFFF);
        assert_eq!(scissor.lry, 0xFFF);
        // The decoder's `int32_t` typing mirrors RT64's locals; the fields
        // are zero-extended and can never be negative.
        for value in [scissor.ulx, scissor.uly, scissor.lrx, scissor.lry] {
            assert!(value >= 0, "scissor coordinates are zero-extended");
        }
    }

    #[test]
    fn set_scissor_matches_the_pinned_rt64_decoder_bit_for_bit() {
        // Proves this arm reuses `decode_set_scissor` rather than
        // re-deriving the layout: a hostile pattern that fills every
        // unrelated bit must agree with the ported decoder exactly.
        let w0 = word(0x40, SET_SCISSOR, 0x00ab_cdef);
        let w1 = 0x3579_2468u32;
        let decoded = decode(vec![w0, w1]).unwrap();
        let RawDpcCommandKind::SetScissor(scissor) = decoded.commands()[0].kind() else {
            panic!("expected SetScissor");
        };
        assert_eq!(
            scissor,
            crate::rt64_gbi_rdp_decode::decode_set_scissor(w0, w1)
        );
    }

    /// Assert two decodes staged byte-identical RDP state.
    ///
    /// Compares every public [`StagedRdpState`] accessor rather than the
    /// struct itself: `StagedRdpState` also carries the per-decode
    /// `QueueIdentity`/`transaction_sequence` bookkeeping, which the
    /// fixture's `decode` helper advances on each call and which has
    /// nothing to do with the commands in the stream.
    fn assert_same_staged_state(left: &StagedRdpState, right: &StagedRdpState) {
        assert_eq!(left.other_mode(), right.other_mode());
        assert_eq!(left.color_image(), right.color_image());
        assert_eq!(left.fill_color(), right.fill_color());
        assert_eq!(left.env_color(), right.env_color());
        assert_eq!(left.prim_color(), right.prim_color());
        assert_eq!(left.blend_color(), right.blend_color());
        assert_eq!(left.fog_color(), right.fog_color());
        assert_eq!(left.prim_depth(), right.prim_depth());
        assert_eq!(left.combine(), right.combine());
        assert_eq!(left.tmem(), right.tmem());
    }

    #[test]
    fn set_scissor_stages_nothing_into_the_rdp_state() {
        // The core no-pixel-change guarantee at the decode layer: a stream
        // whose only command is SetScissor must leave every staged slot
        // exactly as a stream carrying only a no-op leaves it. Nothing in
        // `RdpState` can name a scissor, so nothing downstream can read one.
        let noop_only = decode(vec![word(0, 0x00, 0), 0]).unwrap();
        let scissored = decode(set_scissor_words(0, 1, 0x0a0, 0x0b0, 0x0c0, 0x0d0)).unwrap();
        assert_same_staged_state(scissored.staged_state(), noop_only.staged_state());
    }

    #[test]
    fn set_scissor_interleaved_between_state_commands_changes_no_staged_value() {
        // Same guarantee against a realistic stream: dropping SetScissor
        // commands in between real state commands must leave every staged
        // slot exactly as the scissor-free stream produced it.
        let prefix = 0xc0;
        let without = decode(state_words(prefix)).unwrap();

        let mut with_words = Vec::new();
        with_words.extend(set_scissor_words(prefix, 0, 0, 0, 320 * 4, 240 * 4));
        with_words.extend(state_words(prefix));
        with_words.extend(set_scissor_words(prefix, 1, 16, 16, 300 * 4, 220 * 4));
        let with = decode(with_words).unwrap();

        assert_same_staged_state(with.staged_state(), without.staged_state());
        // ...and the scissor commands are genuinely present, so the
        // equality above is not vacuous.
        assert_eq!(
            with.commands().len(),
            without.commands().len() + 2,
            "both SetScissor commands must have been admitted"
        );
        // The non-scissor commands must also be unchanged, in order.
        let with_kinds: Vec<_> = with
            .commands()
            .iter()
            .map(|command| command.kind())
            .filter(|kind| !matches!(kind, RawDpcCommandKind::SetScissor(_)))
            .collect();
        let without_kinds: Vec<_> = without
            .commands()
            .iter()
            .map(|command| command.kind())
            .collect();
        assert_eq!(with_kinds, without_kinds);
    }

    #[test]
    fn set_prim_color_decodes_lod_bytes_from_w0_and_color_from_w1() {
        let prefix = 0x80;
        // w0 low byte = lodFrac (0x3c), next 5 bits = lodMin (0x0a).
        let w0_payload = (0x0a << 8) | 0x3c;
        let words = vec![word(prefix, SET_PRIM_COLOR, w0_payload), 0x11223344];
        let decoded = decode(words).unwrap();
        let RawDpcCommandKind::SetPrimColor(prim) = decoded.commands()[0].kind() else {
            panic!("expected SetPrimColor");
        };
        assert_eq!(prim.lod().lod_frac(), 0x3c);
        assert_eq!(prim.lod().lod_min(), 0x0a);
        assert_eq!(prim.color().rgba8(), [0x11, 0x22, 0x33, 0x44]);
        assert_eq!(decoded.staged_state().prim_color(), prim);
    }

    #[test]
    fn set_prim_color_unrelated_w0_bits_above_lod_min_are_isolated() {
        // Set every w0 bit above the 5-bit lodMin field (bits 13:23 -- w0's
        // top byte carries the opcode and is excluded from this pattern) to
        // prove they cannot leak into lod_frac/lod_min.
        let prefix = 0;
        let hostile_payload = 0x00ff_e000u32; // bits 13:23 set, bits 0:12 clear
        let words = vec![word(prefix, SET_PRIM_COLOR, hostile_payload), 0];
        let decoded = decode(words).unwrap();
        let RawDpcCommandKind::SetPrimColor(prim) = decoded.commands()[0].kind() else {
            panic!("expected SetPrimColor");
        };
        assert_eq!(prim.lod().lod_frac(), 0);
        assert_eq!(prim.lod().lod_min(), 0);
    }

    #[test]
    fn set_prim_color_lod_min_masks_to_five_bits_on_the_wire() {
        // lodMin's public field is 8 bits (w0 bits 8:15) but the RDP only
        // consults 5 of them (w0 bits 8:12). Set the full 8-bit field to
        // 0xff and confirm lod_min() reports only 0x1f.
        let prefix = 0;
        let words = vec![word(prefix, SET_PRIM_COLOR, 0xff << 8), 0];
        let decoded = decode(words).unwrap();
        let RawDpcCommandKind::SetPrimColor(prim) = decoded.commands()[0].kind() else {
            panic!("expected SetPrimColor");
        };
        assert_eq!(prim.lod().lod_min(), 0x1f);
    }

    #[test]
    fn set_prim_color_zero_and_max_boundaries() {
        let prefix = 0;
        let decoded_zero = decode(vec![word(prefix, SET_PRIM_COLOR, 0), 0]).unwrap();
        let RawDpcCommandKind::SetPrimColor(zero) = decoded_zero.commands()[0].kind() else {
            panic!("expected SetPrimColor");
        };
        assert_eq!(zero.lod().lod_frac(), 0);
        assert_eq!(zero.lod().lod_min(), 0);
        assert_eq!(zero.color().rgba8(), [0, 0, 0, 0]);

        let max_w0 = (0x1f << 8) | 0xff;
        let decoded_max = decode(vec![word(prefix, SET_PRIM_COLOR, max_w0), u32::MAX]).unwrap();
        let RawDpcCommandKind::SetPrimColor(max) = decoded_max.commands()[0].kind() else {
            panic!("expected SetPrimColor");
        };
        assert_eq!(max.lod().lod_frac(), 0xff);
        assert_eq!(max.lod().lod_min(), 0x1f);
        assert_eq!(max.color().rgba8(), [0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn set_prim_depth_decodes_z_from_high_half_and_dz_from_low_half_of_w1() {
        let prefix = 0;
        let w1 = (0x1234u32 << 16) | 0x5678;
        let words = vec![word(prefix, SET_PRIM_DEPTH, 0), w1];
        let decoded = decode(words).unwrap();
        let RawDpcCommandKind::SetPrimDepth(depth) = decoded.commands()[0].kind() else {
            panic!("expected SetPrimDepth");
        };
        assert_eq!(depth.z(), 0x1234);
        assert_eq!(depth.dz(), 0x5678);
        assert_eq!(decoded.staged_state().prim_depth(), Some(depth));
    }

    #[test]
    fn set_prim_depth_z_mask_discards_only_the_top_bit_masked_high_bit_hostile() {
        // z is masked to 0x7FFF (15 bits): the wire field is 16 bits
        // (p1(16,16)) but only the low 15 are consulted. A hostile z value
        // with only the top bit set must decode to zero.
        let prefix = 0;
        let hostile_w1 = 0x8000_0000u32;
        let words = vec![word(prefix, SET_PRIM_DEPTH, 0), hostile_w1];
        let decoded = decode(words).unwrap();
        let RawDpcCommandKind::SetPrimDepth(depth) = decoded.commands()[0].kind() else {
            panic!("expected SetPrimDepth");
        };
        assert_eq!(depth.z(), 0);
        assert_eq!(depth.dz(), 0);
    }

    #[test]
    fn set_prim_depth_zero_and_max_boundaries_normalize_to_unity() {
        let prefix = 0;
        let decoded_zero = decode(vec![word(prefix, SET_PRIM_DEPTH, 0), 0]).unwrap();
        let RawDpcCommandKind::SetPrimDepth(zero) = decoded_zero.commands()[0].kind() else {
            panic!("expected SetPrimDepth");
        };
        assert_eq!(zero.z(), 0);
        assert_eq!(zero.dz(), 0);

        let decoded_max = decode(vec![word(prefix, SET_PRIM_DEPTH, 0), u32::MAX]).unwrap();
        let RawDpcCommandKind::SetPrimDepth(max) = decoded_max.commands()[0].kind() else {
            panic!("expected SetPrimDepth");
        };
        assert_eq!(max.z(), 0x7fff);
        assert_eq!(max.dz(), 0xffff);
        assert_eq!(max.z_normalized(), 1.0);
        assert_eq!(max.dz_normalized(), 1.0);
    }

    #[test]
    fn fragment_registers_sequential_overwrite_last_command_wins_in_one_packet() {
        let prefix = 0;
        let words = vec![
            word(prefix, SET_ENV_COLOR, 0),
            0x1111_1111,
            word(prefix, SET_ENV_COLOR, 0),
            0x2222_2222,
            word(prefix, SET_PRIM_COLOR, 0x0a3c),
            0x3333_3333,
            word(prefix, SET_PRIM_COLOR, 0x0b4d),
            0x4444_4444,
            word(prefix, SET_BLEND_COLOR, 0),
            0x5555_5555,
            word(prefix, SET_BLEND_COLOR, 0),
            0x6666_6666,
            word(prefix, SET_FOG_COLOR, 0),
            0x7777_7777,
            word(prefix, SET_FOG_COLOR, 0),
            0x8888_8888,
            word(prefix, SET_PRIM_DEPTH, 0),
            (100u32 << 16) | 200,
            word(prefix, SET_PRIM_DEPTH, 0),
            (300u32 << 16) | 400,
        ];
        let decoded = decode(words).unwrap();
        assert_eq!(decoded.commands().len(), 10);
        let staged = decoded.staged_state();
        assert_eq!(
            staged.env_color().value(),
            0x2222_2222,
            "last SetEnvColor in the packet must win"
        );
        let prim = staged.prim_color();
        assert_eq!(prim.color().value(), 0x4444_4444);
        assert_eq!(prim.lod().lod_min(), 0x0b);
        assert_eq!(prim.lod().lod_frac(), 0x4d);
        assert_eq!(staged.blend_color().value(), 0x6666_6666);
        assert_eq!(staged.fog_color().value(), 0x8888_8888);
        let depth = staged.prim_depth().unwrap();
        assert_eq!(depth.z(), 300);
        assert_eq!(depth.dz(), 400);
    }

    #[test]
    fn fragment_register_state_delta_records_only_the_decoded_command() {
        let words = vec![word(0, SET_ENV_COLOR, 0), 0xDEAD_BEEF];
        let decoded = decode(words).unwrap();
        assert_eq!(
            decoded.state_delta().env_color().unwrap().value(),
            0xDEAD_BEEF
        );
        assert!(decoded.state_delta().prim_color().is_none());
        assert!(decoded.state_delta().blend_color().is_none());
        assert!(decoded.state_delta().fog_color().is_none());
        assert!(decoded.state_delta().prim_depth().is_none());
    }

    #[test]
    fn failed_fragment_register_packet_leaves_durable_state_unchanged() {
        // A successful SetEnvColor followed by a truncated SetPrimDepth
        // (only its opcode byte present, matching the module's own
        // `truncation_table_reports_exact_context_for_every_width_class`
        // hand-built-stream pattern) must fail the whole decode without
        // ever producing a `DecodedRawDpc`/`StagedRdpState` value the caller
        // could go on to publish: decode operates on
        // `durable_state.fork_for_decode()`, a plain owned copy, and a
        // failed decode returns only `Err`, never a partially-applied `Ok`.
        let durable = RdpState::default();
        let submitted = submit(packet(7, vec![word(0, SET_ENV_COLOR, 0), 0x1122_3344], &[]));
        let packet = submitted.packet();
        let source_identity = TmemLoadSourceIdentity::new(
            packet.identity(),
            packet.journal().identity(),
            submitted.identity(),
            packet.memory_layout(),
        );
        let mut stream = FlattenedStream::new(packet.identity(), 0, &packet.streams()[0]);
        stream.bytes = vec![0xc0 | SET_PRIM_DEPTH];
        let mut state = durable.fork_for_decode();
        let mut delta = RdpStateDelta::default();
        let mut planned = Vec::new();
        let mut commands = Vec::new();
        let error = decode_stream(
            &stream,
            packet.memory_layout(),
            &mut state,
            source_identity,
            &mut delta,
            &mut planned,
            &mut commands,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(error, RawDpcDecodeError::TruncatedCommand { .. }));
        // The type system already guarantees `decode_raw_dpc`/`decode_stream`
        // cannot mutate `durable` (it is borrowed `&RdpState`, never `&mut`);
        // this assertion documents that invariant rather than proving new
        // behavior a compile error would already catch.
        assert_eq!(durable, RdpState::default());
        // The four constant-color registers are not `Option`: untouched
        // means still holding their power-on zero, not "absent".
        assert_eq!(durable.env_color(), Color4::from_wire(0));
        assert_eq!(durable.prim_color(), PrimColor::from_wire(0, 0));
        assert_eq!(durable.blend_color(), Color4::from_wire(0));
        assert_eq!(durable.fog_color(), Color4::from_wire(0));
        assert!(durable.prim_depth().is_none());
    }

    #[test]
    fn fragment_register_decode_preserves_every_pre_existing_state_field() {
        // Seed staged state with OtherMode/ColorImage/FillColor/TMEM (via a
        // real prior decode of `state_words`), then chain one SetEnvColor
        // decode onto that staged state (`decode_raw_dpc_after`, the same
        // move-only chaining `two_packet_chaining_is_explicit_move_only_and_
        // does_not_mutate_baseline` exercises) and confirm every unrelated
        // field survives fork/delta/apply unchanged.
        let prefix = 0;
        let seed_packet = packet(7, state_words(prefix), &[]);
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let seed_submitted = queue.submit(DecodedTicket::new(seed_packet)).unwrap();
        let seed = decode_raw_dpc(seed_submitted, &RdpState::default()).unwrap();
        let expected_other_mode = seed.staged_state().other_mode();
        let expected_color_image = seed.staged_state().color_image();
        let expected_fill_color = seed.staged_state().fill_color();

        let next_packet = packet(8, vec![word(prefix, SET_ENV_COLOR, 0), 0x0102_0304], &[]);
        let next_submitted = queue.submit(DecodedTicket::new(next_packet)).unwrap();
        let decoded = decode_raw_dpc_after(next_submitted, seed.into_staged_state()).unwrap();
        let staged = decoded.staged_state();
        assert_eq!(staged.other_mode(), expected_other_mode);
        assert_eq!(staged.color_image(), expected_color_image);
        assert_eq!(staged.fill_color(), expected_fill_color);
        assert_eq!(staged.env_color().value(), 0x0102_0304);
    }

    #[test]
    fn fragment_register_commands_report_exact_location_and_opcode_width_agreement() {
        for (opcode_fn, opcode) in [
            (SET_ENV_COLOR, 0xfb),
            (SET_PRIM_COLOR, 0xfa),
            (SET_BLEND_COLOR, 0xf9),
            (SET_FOG_COLOR, 0xf8),
            (SET_PRIM_DEPTH, 0xee),
        ] {
            assert_eq!(
                opcode_fn,
                opcode & 0x3f,
                "opcode constant must equal its public spelling masked to 6 bits"
            );
            assert_eq!(
                raw_rdp_command_width(opcode_fn),
                Some(8),
                "every fragment constant register is exactly one 64-bit command word"
            );
            let words = vec![word(0xc0, opcode_fn, 0), 0];
            let decoded = decode(words).unwrap();
            let location = decoded.commands()[0].location();
            assert_eq!(location.wire_opcode(), 0xc0 | opcode_fn);
            assert_eq!(location.source_byte_offset(), COMMAND_START);
            assert_eq!(location.stream_byte_offset(), 0);
        }
    }

    // -- SetCombine (0xfc & 0x3f) ----------------------------------------

    #[test]
    fn set_combine_opcode_constant_matches_public_spelling_masked_to_six_bits() {
        assert_eq!(SET_COMBINE, 0xfc & 0x3f);
        assert_eq!(
            raw_rdp_command_width(SET_COMBINE),
            Some(8),
            "SetCombine is exactly one 64-bit command word"
        );
    }

    #[test]
    fn set_combine_accepts_all_four_wire_prefixes() {
        for prefix in [0x00, 0x40, 0x80, 0xc0] {
            let words = vec![word(prefix, SET_COMBINE, 0x00ab_cdef), 0x1122_3344];
            let decoded = decode(words).unwrap();
            let RawDpcCommandKind::SetCombine(params) = decoded.commands()[0].kind() else {
                panic!("expected SetCombine");
            };
            // `w0`'s top byte carries `prefix | SET_COMBINE` here (from the
            // `word()` helper), not a hostile hand-built payload -- the
            // full-word-passthrough property is proven separately below
            // with an exact captured-style fixture.
            assert_eq!(params.low(), word(prefix, SET_COMBINE, 0x00ab_cdef));
            assert_eq!(params.high(), 0x1122_3344);
        }
    }

    #[test]
    fn set_combine_zero_and_max_high_word_boundaries() {
        let prefix = 0x00;
        let decoded_zero = decode(vec![word(prefix, SET_COMBINE, 0), 0]).unwrap();
        let RawDpcCommandKind::SetCombine(zero) = decoded_zero.commands()[0].kind() else {
            panic!("expected SetCombine");
        };
        assert_eq!(zero.low(), word(prefix, SET_COMBINE, 0));
        assert_eq!(zero.high(), 0);

        let decoded_max = decode(vec![word(prefix, SET_COMBINE, 0), u32::MAX]).unwrap();
        let RawDpcCommandKind::SetCombine(max) = decoded_max.commands()[0].kind() else {
            panic!("expected SetCombine");
        };
        assert_eq!(max.low(), word(prefix, SET_COMBINE, 0));
        assert_eq!(max.high(), u32::MAX);
    }

    /// Proves the wire-fidelity property the task calls out explicitly:
    /// `CombineParams::from_wire` receives `w0` completely unmasked, top
    /// opcode byte included -- `low()` must report the command's actual
    /// captured first wire word verbatim, not a decoder-"cleaned" value with
    /// its top byte stripped. `w0 = 0xfc8f_ff1f` here is not built through
    /// the `word()` helper (which would force the top byte to
    /// `prefix | SET_COMBINE`); it is a standalone literal whose own top
    /// byte (`0xfc`) already happens to mask to `SET_COMBINE` (`0xfc & 0x3f
    /// == 0x3c == SET_COMBINE`), so it is simultaneously a wire-legal
    /// `SetCombine` command word and an exact fixture for this assertion.
    #[test]
    fn set_combine_w0_is_passed_through_completely_unmasked() {
        let w0: u32 = 0xfc8f_ff1f;
        let w1: u32 = 0x88fc_f279;
        assert_eq!(
            w0 & 0x3f000000,
            0x3c00_0000,
            "fixture's top byte must mask to SET_COMBINE"
        );
        let decoded = decode(vec![w0, w1]).unwrap();
        let RawDpcCommandKind::SetCombine(params) = decoded.commands()[0].kind() else {
            panic!("expected SetCombine");
        };
        assert_eq!(
            params.low(),
            0xfc8f_ff1f,
            "w0 must reach CombineParams::low() byte-for-byte"
        );
        assert_eq!(
            params.high(),
            0x88fc_f279,
            "w1 must reach CombineParams::high() byte-for-byte"
        );

        // Independently hand-derived selector decode for this exact fixture
        // (bit positions verified against `combiner.rs`'s cited
        // `parseColorInputA/B/C/D`/`parseAlphaInputA/B/C/D` bit offsets, not
        // against this crate's own `decode_color`/`decode_alpha` output):
        //
        // low  = 0xfc8fff1f, high = 0x88fcf279
        // cycle 0 (second_cycle=false): colorA=(low>>20)&0xF=8, colorB=
        //   (high>>28)&0xF=8, colorC=(low>>15)&0x1F=31, colorD=(high>>15)&0x7=1,
        //   alphaA=(low>>12)&0x7=7, alphaB=(high>>12)&0x7=7,
        //   alphaC=(low>>9)&0x7=7, alphaD=(high>>9)&0x7=1
        // cycle 1 (second_cycle=true): colorA=(low>>5)&0xF=8, colorB=
        //   (high>>24)&0xF=8, colorC=low&0x1F=31, colorD=(high>>6)&0x7=1,
        //   alphaA=(high>>21)&0x7=7, alphaB=(high>>3)&0x7=7,
        //   alphaC=(high>>18)&0x7=7, alphaD=high&0x7=1
        //
        // index 8 collapses to Zero in both color-A (table has 0-7) and
        // color-B (table has 0-7); index 31 collapses to Zero in color-C
        // (table has 0-15); index 1 is Texel0 in the common/ABD tables;
        // index 7 collapses to Zero in alpha-ABD (table has 0-6) and in
        // alpha-C (table has 0-6, distinct mapping).
        use crate::{AlphaInput, AlphaInputSlot, ColorInput, ColorInputSlot};
        for second_cycle in [false, true] {
            assert_eq!(
                params.decode_color(ColorInputSlot::A, second_cycle),
                ColorInput::Zero
            );
            assert_eq!(
                params.decode_color(ColorInputSlot::B, second_cycle),
                ColorInput::Zero
            );
            assert_eq!(
                params.decode_color(ColorInputSlot::C, second_cycle),
                ColorInput::Zero
            );
            assert_eq!(
                params.decode_color(ColorInputSlot::D, second_cycle),
                ColorInput::Texel0
            );
            assert_eq!(
                params.decode_alpha(AlphaInputSlot::A, second_cycle),
                AlphaInput::Zero
            );
            assert_eq!(
                params.decode_alpha(AlphaInputSlot::B, second_cycle),
                AlphaInput::Zero
            );
            assert_eq!(
                params.decode_alpha(AlphaInputSlot::C, second_cycle),
                AlphaInput::Zero
            );
            assert_eq!(
                params.decode_alpha(AlphaInputSlot::D, second_cycle),
                AlphaInput::Texel0
            );
        }
    }

    #[test]
    fn set_combine_state_delta_records_only_the_decoded_command() {
        let words = vec![word(0, SET_COMBINE, 0), 0xDEAD_BEEF];
        let decoded = decode(words).unwrap();
        assert_eq!(decoded.state_delta().combine().unwrap().high(), 0xDEAD_BEEF);
        assert!(decoded.state_delta().env_color().is_none());
        assert!(decoded.state_delta().prim_color().is_none());
    }

    #[test]
    fn set_combine_sequential_overwrite_last_command_wins_in_one_packet() {
        let prefix = 0;
        let words = vec![
            word(prefix, SET_COMBINE, 0),
            0x1111_1111,
            word(prefix, SET_COMBINE, 0),
            0x2222_2222,
        ];
        let decoded = decode(words).unwrap();
        assert_eq!(decoded.commands().len(), 2);
        assert_eq!(
            decoded.staged_state().combine().unwrap().high(),
            0x2222_2222,
            "last SetCombine in the packet must win"
        );
    }

    /// Retention across packet boundaries: a `SetCombine` decoded in packet
    /// N must still be the active combine state read from `RdpState` in
    /// packet N+1, with no intervening `SetCombine`, exactly matching
    /// `fragment_register_decode_preserves_every_pre_existing_state_field`'s
    /// cross-packet chaining shape for the other fragment registers.
    #[test]
    fn set_combine_is_retained_as_durable_state_across_a_packet_boundary_with_no_intervening_set_combine(
    ) {
        let prefix = 0;
        let seed_packet = packet(7, vec![word(prefix, SET_COMBINE, 0), 0xAABB_CCDD], &[]);
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let seed_submitted = queue.submit(DecodedTicket::new(seed_packet)).unwrap();
        let seed = decode_raw_dpc(seed_submitted, &RdpState::default()).unwrap();
        let expected_combine = seed.staged_state().combine();
        assert!(expected_combine.is_some());

        // Packet N+1 decodes an unrelated command (SetEnvColor) with no
        // SetCombine of its own.
        let next_packet = packet(8, vec![word(prefix, SET_ENV_COLOR, 0), 0x0102_0304], &[]);
        let next_submitted = queue.submit(DecodedTicket::new(next_packet)).unwrap();
        let decoded = decode_raw_dpc_after(next_submitted, seed.into_staged_state()).unwrap();
        let staged = decoded.staged_state();
        assert_eq!(
            staged.combine(),
            expected_combine,
            "SetCombine from packet N must still be the active combine state in packet N+1"
        );
        assert!(
            decoded.state_delta().combine().is_none(),
            "packet N+1's own delta must not record a SetCombine it never decoded"
        );
    }

    // -----------------------------------------------------------------
    // Raw triangle -> declared per-row ColorFramebuffer writes
    // -----------------------------------------------------------------

    /// One-cycle `OtherMode` (cycle_type 0) and an RGBA16 colour image 8
    /// pixels wide at address 0 -- the narrowest staging a flat raw
    /// triangle needs to declare a write.
    ///
    /// One cycle, not Fill: `plan_raw_triangle` refuses Fill cycle, where
    /// the RDP consults no combiner at all.
    /// `RdpState` with a host-configured colour-target height, which every
    /// raw-triangle declaration test needs.
    ///
    /// `plan_raw_triangle` bounds its per-scanline row walk by this height --
    /// the same value `create_inner` gives `configured_target_extent` -- and
    /// declares NOTHING when it is absent, rather than guessing one. So a
    /// fixture using bare `RdpState::default()` measures the missing-height
    /// refusal instead of the row walk.
    ///
    /// Eight rows: taller than the three-row fixture triangle, so the height
    /// bound is not what limits it and these tests keep measuring the edge
    /// coefficients.
    fn triangle_state() -> RdpState {
        let mut state = RdpState::default();
        state.set_color_target_height(8);
        state
    }

    fn flat_triangle_state_words(prefix: u8) -> Vec<u32> {
        vec![
            word(prefix, SET_OTHER_MODE, 0),
            0,
            // RGBA16: format 0 (RGBA) and size 2 (`PixelSize::Bits16`) in
            // bits 21:19; width 8 (the wire field is width-1); address 0.
            word(prefix, SET_COLOR_IMAGE, 2 << 19 | 7),
            0,
        ]
    }

    /// The eight wire words of one flat (opcode 0x08) left-major triangle
    /// whose two edges are vertical at x = 2 and x = 6, spanning scanlines
    /// 0..3.
    ///
    /// Hand-derived footprint, from the wire fields and nothing else:
    ///   yh = 0, yl = 3<<2 = 12 (S11.2) -> yh_eighth 0, yl_eighth 24
    ///   min_y = ceil((0-7)/8) = 0, max_y = ceil((24-1)/8) = 3  -> rows 0,1,2
    ///   xh = 2<<16, dxhdy = 0 -> left edge parked at 2.0
    ///   xm = xl = 6<<16, slopes 0 -> right edge parked at 6.0
    ///   x0 = ceil((2*65536 - 7*65536/8)/65536) = ceil(1.125) = 2
    ///   x1 = ceil((6*65536 - 65536/8)/65536)   = ceil(5.875) = 6
    /// So each row covers pixels 2..6 = 4 pixels = 8 bytes at RGBA16, and
    /// row y starts at byte (y*8 + 2)*2 = 16y + 4: bytes 4, 20, 36.
    fn flat_triangle_words(prefix: u8) -> Vec<u32> {
        crate::wire_words::EdgeWords {
            lft: true,
            yl: crate::wire_words::line(3),
            ym: crate::wire_words::line(3),
            yh: 0,
            xl: crate::wire_words::px(6),
            xh: crate::wire_words::px(2),
            xm: crate::wire_words::px(6),
            ..crate::wire_words::EdgeWords::zeroed()
        }
        .words(prefix, 0x08)
        .to_vec()
    }

    #[test]
    fn a_flat_raw_triangle_declares_one_exact_access_per_covered_row() {
        let mut words = flat_triangle_state_words(0);
        words.extend(flat_triangle_words(0));
        // The three hand-derived per-row ranges, in row order. A collapsed
        // single span would be (4, 44) and this would fail.
        let submitted = submit(packet(7, words, &[(4, 12), (20, 28), (36, 44)]));
        let decoded = decode_raw_dpc(submitted, &triangle_state()).unwrap();
        assert_eq!(
            decoded.resource_plan().accesses(),
            decoded.submitted().packet().journal().accesses(),
            "the declared run must equal the journal's, access for access"
        );
        // The command is still decoded as a triangle, not swallowed.
        assert!(matches!(
            decoded.commands().last().map(|command| command.kind()),
            Some(RawDpcCommandKind::RawTriangle(_))
        ));
    }

    #[test]
    fn a_flat_raw_triangles_declared_rows_bind_back_as_its_own_span() {
        let mut words = flat_triangle_state_words(0);
        words.extend(flat_triangle_words(0));
        let submitted = submit(packet(7, words, &[(4, 12), (20, 28), (36, 44)]));
        let decoded = decode_raw_dpc(submitted, &triangle_state()).unwrap();
        // The triangle is command index 2 (two state commands precede it).
        let accesses = decoded
            .resource_plan()
            .bind_texture_rectangle(2)
            .expect("the triangle's own span binds");
        assert_eq!(accesses.len(), 3, "one access per covered scanline");
        for access in accesses {
            assert_eq!(access.mode(), AccessMode::Write);
            assert_eq!(access.purpose(), AccessPurpose::RenderTarget);
        }
    }

    /// One triangle of `opcode`, with the right number of zeroed
    /// coefficient words for its own flag bits, decoded through the real
    /// planner -- returning how many accesses the plan declared.
    ///
    /// A plan with no triangle declares exactly ONE access (the state
    /// commands' own), so "declared nothing" is `1` and "declared its rows"
    /// is more.
    fn declared_access_count_for_opcode(opcode: u8) -> usize {
        let mut words = flat_triangle_state_words(0);
        let mut triangle = flat_triangle_words(0);
        triangle[0] = word(0, opcode, 1 << 23 | 12);
        let extra_words = 8 * u32::from(opcode & 0x4 != 0)
            + 8 * u32::from(opcode & 0x2 != 0)
            + 2 * u32::from(opcode & 0x1 != 0);
        triangle.extend(core::iter::repeat_n(0u32, (extra_words * 2) as usize));
        words.extend(triangle);
        // The journal the packet is submitted with must be the one the
        // decoder produces, or `decode_raw_dpc` refuses with
        // `JournalMismatch` before this helper can count anything. The three
        // ranges are `flat_triangle_words`' own footprint, hand-derived the
        // same way `a_flat_raw_triangle_declares_one_exact_access_per_
        // covered_row` derives them -- and they are shared by every opcode
        // here BECAUSE the footprint is a function of the edges alone, which
        // is the very claim this helper's callers assert.
        //
        // A depth-bearing opcode declares none of them, so it is submitted
        // with an empty journal instead.
        let expected: &[(u32, u32)] = if opcode & 0x1 != 0 {
            &[]
        } else {
            &[(4, 12), (20, 28), (36, 44)]
        };
        let submitted = submit(packet(7, words, expected));
        let decoded = decode_raw_dpc(submitted, &triangle_state()).unwrap();
        decoded.resource_plan().accesses().len()
    }

    #[test]
    fn a_depth_bearing_triangle_declares_no_write() {
        // Depth is still outside the executor's subset -- there is no depth
        // image, no depth journal declaration and no Z encoding -- so a
        // depth-bearing triangle must declare nothing. Declaring a row the
        // executor cannot fill would digest stale resident bytes into guest
        // RDRAM.
        //
        // Opcodes: bit 0 = depth, bit 1 = textured, bit 2 = shaded. The
        // refused set is now exactly the four with bit 0 set.
        for opcode in [0x09u8, 0x0b, 0x0d, 0x0f] {
            assert_eq!(
                declared_access_count_for_opcode(opcode),
                1,
                "opcode {opcode:#04x} carries a depth plane and declared a write it cannot fill"
            );
        }
    }

    /// **The texture rung's decoder half.** Every depth-free opcode --
    /// including 0x0a and 0x0e, the textured ones -- now declares its own
    /// per-row run.
    ///
    /// This is the change WM2000's geometry needs: all 1,314,648 raw
    /// triangles measured on the real ROM are opcode 0x0e, and this
    /// predicate previously refused every one of them, so the ROM's entire
    /// 3D scene declared nothing and drew nothing.
    ///
    /// Asserted as an EQUALITY against the flat opcode's own count, not as
    /// "more than one": the footprint is a function of the edge
    /// coefficients alone, and every opcode here carries identical edges, so
    /// a textured triangle declaring a DIFFERENT number of rows than a flat
    /// one would mean the coefficient-block length changed the geometry.
    #[test]
    fn every_depth_free_opcode_including_the_textured_ones_declares_its_rows() {
        let flat = declared_access_count_for_opcode(0x08);
        assert!(
            flat > 1,
            "the flat opcode must declare rows for this comparison to mean anything"
        );
        for opcode in [0x0au8, 0x0c, 0x0e] {
            assert_eq!(
                declared_access_count_for_opcode(opcode),
                flat,
                "opcode {opcode:#04x} is depth-free and must declare the same per-row run \
                 the identically-edged flat triangle does"
            );
        }
    }

    #[test]
    fn a_shaded_triangle_declares_the_same_per_row_run_a_flat_one_does() {
        // Opcode 0x0c is flat's shaded sibling: identical edge coefficients
        // plus eight shade words. The footprint is a function of the EDGE
        // coefficients alone, so the declared run must be byte-identical to
        // `a_flat_raw_triangle_declares_one_exact_access_per_covered_row`'s.
        //
        // This is what the executor's shade interpolation bought: 100% of
        // the 826,056 raw triangles WM2000 issues are shaded (and textured),
        // so a decoder that refuses shaded declares nothing for the entire
        // ROM.
        let mut words = flat_triangle_state_words(0);
        let mut triangle = flat_triangle_words(0);
        triangle[0] = word(0, 0x0c, 1 << 23 | 12);
        // Eight shade coefficient words = sixteen u32 halves.
        triangle.extend(core::iter::repeat_n(0u32, 16));
        words.extend(triangle);
        let submitted = submit(packet(7, words, &[(4, 12), (20, 28), (36, 44)]));
        let decoded = decode_raw_dpc(submitted, &triangle_state()).unwrap();
        assert_eq!(
            decoded.resource_plan().accesses(),
            decoded.submitted().packet().journal().accesses()
        );
    }

    #[test]
    fn a_flat_triangle_in_fill_cycle_or_without_a_colour_image_declares_no_write() {
        // Fill cycle: the RDP consults no combiner, a path this executor
        // does not implement for triangles.
        let mut words = vec![word(0, SET_OTHER_MODE, 3 << 20), 0];
        words.extend([word(0, SET_COLOR_IMAGE, 2 << 19 | 7), 0]);
        words.extend(flat_triangle_words(0));
        let submitted = submit(packet(7, words, &[]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        assert_eq!(decoded.resource_plan().accesses().len(), 1);

        // No staged colour image at all: nothing to write into.
        let mut words = vec![word(0, SET_OTHER_MODE, 0), 0];
        words.extend(flat_triangle_words(0));
        let submitted = submit(packet(7, words, &[]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        assert_eq!(decoded.resource_plan().accesses().len(), 1);
    }

    #[test]
    fn a_flat_triangle_reaching_outside_installed_rdram_declares_nothing_at_all() {
        // Not a truncated prefix: a partially-declarable triangle must
        // declare NO row, because the rasterizer walks every row the span
        // claims and a prefix would leave it writing rows nobody declared.
        //
        // Same vertical edges as `flat_triangle_words`, but 200 scanlines
        // tall (YL = 200<<2) against an RGBA16 image 8 pixels wide parked at
        // 0x3fc0, the last 64-byte-aligned address in the 0x4000 layout. Row
        // 0 fits (bytes 0x3fc4..0x3fcc); row 4 already needs 0x4000, which is
        // past installed RDRAM. So SOME rows are placeable and the whole
        // triangle must still declare nothing.
        let base = LAYOUT_BYTES - 64;
        let mut words = vec![word(0, SET_OTHER_MODE, 0), 0];
        words.extend([word(0, SET_COLOR_IMAGE, 2 << 19 | 7), base]);
        words.extend([
            word(0, 0x08, 1 << 23 | (200 << 2)),
            (200u32 << 2) << 16,
            6 << 16,
            0,
            2 << 16,
            0,
            6 << 16,
            0,
        ]);
        let submitted = submit(packet(7, words, &[]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        assert_eq!(
            decoded.resource_plan().accesses().len(),
            1,
            "one command-decode read and no render-target write"
        );
    }
}
