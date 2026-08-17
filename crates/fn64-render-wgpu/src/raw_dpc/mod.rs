//! Bounded decoding for the first admitted raw-DPC command subset.

mod production_adapter;
mod texture_rectangle;
mod triangle;
#[cfg(test)]
mod triangle_composition;
mod triangle_draw_data;
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
const SET_COLOR_IMAGE: u8 = 0x3f;
const SET_FILL_COLOR: u8 = 0x37;
const FILL_RECTANGLE: u8 = 0x36;
const FULL_SYNC: u8 = 0x29;
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
        let mut preceding = 0_u32;
        let mut source_binding = None;
        for (source_ordinal, access) in sources.iter().enumerate() {
            let bytes = access.region().declared_bytes();
            let end = preceding
                .checked_add(bytes)
                .ok_or(TmemLoadSourcePlanError::AccessDescriptorsDiffer)?;
            if logical_offset < end {
                let ordinal = u32::try_from(source_ordinal)
                    .map_err(|_| TmemLoadSourcePlanError::AccessSliceOutOfBounds)?;
                source_binding = Some((
                    plan.source()
                        .first_access_index()
                        .checked_add(ordinal)
                        .ok_or(TmemLoadSourcePlanError::AccessSliceOutOfBounds)?,
                    logical_offset - preceding,
                ));
                break;
            }
            preceding = end;
        }
        let (source_access_index, source_access_byte_offset) =
            source_binding.ok_or(TmemLoadSourcePlanError::AccessDescriptorsDiffer)?;
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
) -> Result<(), RawDpcDecodeError> {
    if state.other_mode().map(OtherMode::cycle_type) != Some(CycleType::Fill) {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle requires staged fill-cycle OtherMode",
        });
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
    let bytes_per_pixel = image
        .size()
        .bytes_per_pixel()
        .expect("RGBA16/32 are byte-addressed");
    let row_bytes =
        (x1 - x0 + 1)
            .checked_mul(bytes_per_pixel)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "FillRectangle row byte count overflows",
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
            reason: "FillRectangle exceeds the bounded resource-plan access count",
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
                reason: "FillRectangle pixel offset overflows",
            })?;
        let start = image
            .address()
            .get()
            .checked_add(pixel.checked_mul(bytes_per_pixel).ok_or(
                RawDpcDecodeError::InvalidCommand {
                    location,
                    reason: "FillRectangle byte offset overflows",
                },
            )?)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "FillRectangle address overflows",
            })?;
        let rows = last_y - first_y + 1;
        let bytes = row_bytes
            .checked_mul(rows)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "FillRectangle byte count overflows",
            })?;
        let end = start
            .checked_add(bytes)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "FillRectangle range end overflows",
            })?;
        let range = layout
            .range(start, end)
            .map_err(|_| RawDpcDecodeError::InvalidCommand {
                location,
                reason: "FillRectangle writes outside installed RDRAM",
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
        "plan_fill pushed a different number of accesses than it planned"
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

    fn word(prefix: u8, opcode: u8, payload: u32) -> u32 {
        u32::from(prefix | opcode) << 24 | payload
    }

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
        accesses.extend(
            effect_ranges
                .iter()
                .enumerate()
                .map(|(index, &(start, end))| effect_access(layout, index as u32 + 1, start, end)),
        );
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(64, LAYOUT_BYTES).unwrap(),
            accesses,
        )
        .unwrap();
        WorkloadPacket::try_new(
            layout,
            WorkloadAdmission::RawDpc {
                transaction_sequence,
            },
            vec![stream],
            journal,
        )
        .unwrap()
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

    /// The decode-time gate this card removes was strictly redundant with
    /// `plan_fill`'s own independent fill-cycle check (card Section 1/5):
    /// a `FillRectangle` preceded by a non-`Fill` `SetOtherMode` must still
    /// be rejected -- just later, at `plan_fill`, with its existing
    /// `"FillRectangle requires staged fill-cycle OtherMode"` reason, not at
    /// decode.
    #[test]
    fn non_fill_cycle_other_mode_is_rejected_at_plan_fill_not_at_decode() {
        let words = vec![
            word(0, SET_OTHER_MODE, 2 << 20), // TwoCycle, not Fill
            0,
            word(0, SET_COLOR_IMAGE, 3 << 19 | 1),
            0,
            word(0, SET_FILL_COLOR, 0),
            0x213c_4d59,
            word(0, FILL_RECTANGLE, 4 << 12 | 4),
            0,
        ];
        let error = decode(words).unwrap_err();
        let RawDpcDecodeError::InvalidCommand { location, reason } = error else {
            panic!("expected FillRectangle to be rejected by plan_fill's own check");
        };
        assert_eq!(
            reason, "FillRectangle requires staged fill-cycle OtherMode",
            "rejection must carry plan_fill's own reason, not a decode-time gate's"
        );
        assert_eq!(location.wire_opcode() & 0x3f, FILL_RECTANGLE);
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
        let submitted = submit(packet(7, words, &[(4, 12), (20, 28)]));
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

    #[test]
    fn transfer_words_preserve_wrap_order_and_undefined_tail_separately_from_union() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 0, 2, 8, 0x200));
        words.extend(set_tile(0, 7, 0, 511));
        words.extend(load_sync(0));
        words.extend([word(0, LOAD_BLOCK, 1), 7 << 24 | 4 << 12]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, words, &[(0x210, 0x21a)])),
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
        assert_eq!(transfer.words()[1].defined_source_byte_mask(), 0x03);
        assert!(transfer.words().iter().all(|word| word.odd_row_exchange()));
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

    #[test]
    fn load_block_starting_tl_and_dxt_carry_select_each_word_row() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 0, 2, 16, 0x200));
        words.extend(set_tile(0, 7, 2, 10));
        words.extend(load_sync(0));
        words.extend([word(0, LOAD_BLOCK, 1), 7 << 24 | 8 << 12 | 0x0400]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, words, &[(0x220, 0x232)])),
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
            vec![(0, 10, true), (0, 11, true), (1, 14, false)]
        );
        assert_eq!(transfer.words()[2].defined_source_byte_mask(), 0x03);
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
            vec![
                (
                    255,
                    crate::TmemTransferPhysicalWord::SplitBanks {
                        low: fn64_render_ir::TmemRange::try_new(2044, 2048).unwrap(),
                        high: fn64_render_ir::TmemRange::try_new(4092, 4096).unwrap(),
                    },
                ),
                (
                    0,
                    crate::TmemTransferPhysicalWord::SplitBanks {
                        low: fn64_render_ir::TmemRange::try_new(4, 8).unwrap(),
                        high: fn64_render_ir::TmemRange::try_new(2052, 2056).unwrap(),
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
            vec![
                ResourceRegion::Tmem(fn64_render_ir::TmemRange::try_new(4, 8).unwrap()),
                ResourceRegion::Tmem(fn64_render_ir::TmemRange::try_new(2044, 2048).unwrap()),
                ResourceRegion::Tmem(fn64_render_ir::TmemRange::try_new(2052, 2056).unwrap()),
                ResourceRegion::Tmem(fn64_render_ir::TmemRange::try_new(4092, 4096).unwrap()),
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

    #[test]
    fn load_tile_retains_row_local_defined_masks_and_source_row_parity() {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 0, 2, 5, 0x200));
        words.extend(set_tile(0, 7, 3, 0));
        words.extend(load_sync(0));
        words.extend([word(0, LOAD_TILE, 4), 7 << 24 | 16 << 12 | 8]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(7, words, &[(0x20a, 0x21e)])),
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
            vec![
                (0, 0xff, 0, true),
                (8, 0x03, 1, true),
                (10, 0xff, 3, false),
                (18, 0x03, 4, false)
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
            submit(packet_with_tmem_sources(7, unpaired_yuv, &[(0x202, 0x206)])),
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
                &[(0x202, 0x206), (0x20a, 0x20e)],
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
        assert_eq!(yuv_tile_load.source_plan().total_bytes(), 8);
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
                        .range(0x202, 0x206)
                        .unwrap(),
                },
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range: PhysicalMemoryLayout::try_new(LAYOUT_BYTES)
                        .unwrap()
                        .range(0x20a, 0x20e)
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

    #[test]
    fn load_tile_plans_exact_fractional_subrows_and_collapses_full_rows() {
        let mut subrows = Vec::new();
        subrows.extend(set_texture_image(0, 2, 1, 9, 0x200));
        subrows.extend(set_tile(0, 3, 1, 0));
        subrows.extend(load_sync(0));
        subrows.extend([word(0, LOAD_TILE, 5 << 12 | 8), 3 << 24 | 15 << 12 | 15]);
        let decoded = decode_raw_dpc(
            submit(packet_with_tmem_sources(
                7,
                subrows,
                &[(0x213, 0x216), (0x21c, 0x21f)],
            )),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadTile(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadTile");
        };
        assert_eq!(load.source_plan().access_count(), 2);
        assert_eq!(load.source_plan().total_bytes(), 6);
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
            submit(packet_with_tmem_sources(7, full_rows, &[(0x300, 0x308)])),
            &RdpState::default(),
        )
        .unwrap();
        let RawDpcCommandKind::LoadTile(load) = decoded.commands()[3].kind() else {
            panic!("expected LoadTile");
        };
        assert_eq!(load.source_plan().access_count(), 1);
        assert_eq!(load.source_plan().total_bytes(), 8);
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
        word(
            prefix,
            opcode,
            (tile & 0x7) << 16 | (level & 0x7) << 19 | u32::from(yl),
        )
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
        assert_eq!(decoded.staged_state().env_color(), Some(color));
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
        assert_eq!(decoded.staged_state().blend_color(), Some(color));
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
        assert_eq!(decoded.staged_state().fog_color(), Some(color));
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
        assert_eq!(decoded.staged_state().prim_color(), Some(prim));
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
            staged.env_color().unwrap().value(),
            0x2222_2222,
            "last SetEnvColor in the packet must win"
        );
        let prim = staged.prim_color().unwrap();
        assert_eq!(prim.color().value(), 0x4444_4444);
        assert_eq!(prim.lod().lod_min(), 0x0b);
        assert_eq!(prim.lod().lod_frac(), 0x4d);
        assert_eq!(staged.blend_color().unwrap().value(), 0x6666_6666);
        assert_eq!(staged.fog_color().unwrap().value(), 0x8888_8888);
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
        )
        .unwrap_err();
        assert!(matches!(error, RawDpcDecodeError::TruncatedCommand { .. }));
        // The type system already guarantees `decode_raw_dpc`/`decode_stream`
        // cannot mutate `durable` (it is borrowed `&RdpState`, never `&mut`);
        // this assertion documents that invariant rather than proving new
        // behavior a compile error would already catch.
        assert_eq!(durable, RdpState::default());
        assert!(durable.env_color().is_none());
        assert!(durable.prim_color().is_none());
        assert!(durable.blend_color().is_none());
        assert!(durable.fog_color().is_none());
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
        assert_eq!(staged.env_color().unwrap().value(), 0x0102_0304);
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
}
