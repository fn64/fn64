use super::*;

/// Neutral payload for one staged, resource-access-free state command
/// (`SetTile`, `SetTileSize`, `SetTextureImage`, `SyncLoad`, plus the
/// nine pure-RDP-state commands admitted alongside them: `SetOtherMode`,
/// `SetColorImage`, `SetFillColor`, `SetEnvColor`, `SetPrimColor`,
/// `SetBlendColor`, `SetFogColor`, `SetPrimDepth`, `SetCombine`, plus
/// the tracked-only `SetScissor`) that a following load or draw command
/// depends on. T3 needs these fields to reconstruct tile/RDP state
/// without rereading command bytes.
///
/// Every variant except `SyncLoad` carries `raw_words` (the command's own
/// wire words) and an ordered `before`/`after` [`RdpStateIdentity`] pair:
/// `before` is `None` only for the first state command touching that
/// slot in a plan (there is no prior state to identify); `after` is
/// always the identity of the value this command just staged. `SyncLoad`
/// instead carries `input_epoch`/`output_epoch` -- the epoch this
/// command superseded (`None` only for a plan's first `SyncLoad`) and
/// the new epoch it established -- since a load-sync boundary has no
/// tile/image value of its own to hash. The nine pure-state commands
/// each occupy one single global slot in `RdpState`/`RdpStateDelta`
/// (`Option<T>`, not a per-tile array), so `before` threads exactly the
/// way `SetTextureImage`'s single-slot `texture_image` field already
/// does, not the 8-slot tile arrays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RdpStateCommand {
    SetTile {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        tile_index: u8,
        descriptor: NeutralTileDescriptor,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    SetTileSize {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        tile_index: u8,
        size: NeutralTileSize,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    SetTextureImage {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        image: NeutralTextureImage,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    SyncLoad {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        input_epoch: Option<TmemLoadEpoch>,
        output_epoch: TmemLoadEpoch,
    },
    SetOtherMode {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        other_mode: NeutralOtherMode,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    SetColorImage {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        image: NeutralColorImage,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    SetFillColor {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        color: NeutralFillColor,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    SetEnvColor {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        color: NeutralColor4,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    SetPrimColor {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        color: NeutralPrimColor,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    SetBlendColor {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        color: NeutralColor4,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    SetFogColor {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        color: NeutralColor4,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    SetPrimDepth {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        depth: NeutralPrimDepth,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    SetCombine {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        combine: NeutralCombineParams,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
    /// RDP opcode `0x2d`, admitted as **tracked state only**: the rect
    /// is carried here so a stream containing `SetScissor` parses and
    /// plans instead of dying at `UnsupportedCommand`, but no draw,
    /// clip, or bounds computation reads it. Unlike the nine applied
    /// pure-state commands above, this one's value is deliberately
    /// absent from `RdpState`/`RdpStateDelta`, so there is no channel
    /// through which the raster path could consult it even by accident.
    /// It still threads `before`/`after` over its own single global
    /// slot exactly like its siblings, so admitting it later (as
    /// applied state) is an additive change rather than a reshape.
    SetScissor {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        scissor: NeutralScissor,
        before: Option<RdpStateIdentity>,
        after: RdpStateIdentity,
    },
}

/// Neutral mirror of one decoded triangle vertex (RT64's
/// `posWorkBuffer`/`colorWorkBuffer`/`texcoordWorkBuffer` write for one
/// triangle vertex), field-for-field identical to T1's private
/// `TriangleVertex` shape -- this crate cannot name that wgpu-crate type
/// directly (`fn64-render-wgpu` depends on `fn64-render`, not the
/// reverse), so this is a plain data mirror, not a reinterpretation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NeutralTriangleVertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
    pub color: [f32; 4],
    pub texcoord: [f32; 2],
}

/// Neutral carrier for one admitted `RawTriangle` draw command: the
/// command's own raw wire words (variable-width, per
/// `raw_rdp_command_width`/`triangle_word_count` -- unlike every
/// `RdpStateCommand` variant's fixed 2-word shape) and the three already-
/// decoded triangle vertices, in RT64's exact `workBufferIndex + 0/1/2`
/// order. Deliberately has **no** `before`/`after` [`RdpStateIdentity`]
/// fields: a triangle is a draw event, not a value that persists in one
/// global slot and gets overwritten the way the nine pure-state
/// commands do, and it pushes zero [`ResourceAccess`] entries into the
/// owning plan, so [`ExactRawDpcPlanWriter::finish`]'s access-ordering
/// contract (which has zero coupling to the writer's pushed commands) is
/// trivially satisfied without one. `raw_words` is kept anyway, matching
/// this crate's characterization-first convention of never discarding
/// the raw bytes a command carried.
#[derive(Clone, Debug, PartialEq)]
pub struct RdpTriangleCommand {
    pub location: RawDpcCommandLocation,
    pub raw_words: Box<[u32]>,
    pub vertices: [NeutralTriangleVertex; 3],
    pub source: TriangleSource,
    pub viewport: Option<RectViewportPixels>,
    /// The exact ordered `RenderTarget` write-access span the decoder
    /// declared for the originating `TextureRectangle` command, or
    /// `None`.
    ///
    /// `None` for every `TriangleSource::RawTriangle` -- a raw triangle
    /// pushes zero accesses, as this type's own doc states -- and also
    /// `None` for a `TextureRectangle` whose destination was not
    /// provable at decode time (no staged `SetColorImage`, an
    /// unsupported color format, or a fractional or reversed rectangle;
    /// see the wgpu decoder's `plan_texture_rectangle`). A texrect
    /// that declared no write is not a silent no-op: it still rasters
    /// through the triangle path, it simply has no `ColorFramebuffer`
    /// range for a CPU-side executor to compose into.
    ///
    /// Carried for the same reason [`RdpFillRectangleCommand`] carries
    /// its own pair: so a visitor can locate the accesses this command
    /// declared **without re-deriving them** from the rectangle's
    /// geometry, which is exactly the second-independent-derivation
    /// drift `ExactRawDpcPlanWriter::finish`'s access-for-access check
    /// exists to catch.
    ///
    /// One texture rectangle is admitted as two triangles, and **both
    /// halves carry the identical span** -- it describes the
    /// originating wire command, not either half's own share of it.
    /// A consumer counting declared writes must therefore attribute the
    /// span once per originating command, never once per triangle.
    pub texrect_accesses: Option<TriangleAccessSpan>,
}

/// One `TextureRectangle`-sourced triangle's originating command's
/// declared `RenderTarget` write-access span, in the owning plan's
/// ordered access list.
///
/// Field-for-field the same pair [`RdpFillRectangleCommand`] carries
/// (`first_access_index`/`access_count`), named as a struct here because
/// the whole pair is optional on a triangle where it is mandatory on a
/// fill -- `Option<TriangleAccessSpan>` makes "declared nothing"
/// unrepresentable as "declared zero accesses starting at zero".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TriangleAccessSpan {
    /// Index into the owning plan's ordered access list of the
    /// originating command's first `RenderTarget` write access.
    pub first_access_index: u32,
    /// How many consecutive accesses starting at `first_access_index`
    /// belong to it. `1` for a full-image-width rectangle, the
    /// rectangle's covered pixel-row count otherwise.
    pub access_count: u32,
}

/// Neutral carrier for one admitted fill-cycle `FillRectangle` (RDP
/// opcode 0x36). Carries the decoded wire rectangle plus the exact
/// ordered [`ResourceAccess`] span the decoder declared for it -- one
/// access for a full-image-width fill, one **per row** otherwise,
/// because a partial-width rectangle's rows occupy disjoint,
/// width-strided RDRAM ranges and one collapsed range would declare
/// untouched bytes as written.
///
/// Unlike [`RdpTriangleCommand`] (which pushes zero accesses) this
/// command pushes N, so it carries `first_access_index`/`access_count`
/// exactly the way [`TmemLoadSemantics`] carries its own
/// destination-access span -- letting a visitor locate this fill's
/// accesses in the owning plan without re-deriving them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RdpFillRectangleCommand {
    pub location: RawDpcCommandLocation,
    pub raw_words: Box<[u32]>,
    /// Raw 12-bit fixed-point wire fields, exactly as the decoder read
    /// them -- 10 integer bits plus 2 fractional bits. Deliberately
    /// **not** pre-divided by 4: the fill executor performs that
    /// conversion itself and rejects a nonzero fraction rather than
    /// truncating, so passing whole pixels here would silently discard
    /// the evidence that rejection is built on.
    pub upper_left_x: u16,
    pub upper_left_y: u16,
    pub lower_right_x: u16,
    pub lower_right_y: u16,
    /// The color image this fill targets, as staged by the preceding
    /// `SetColorImage`. Duplicated onto the command (rather than left
    /// for the visitor to track) so the execution-time color-target
    /// identity is derived from the same value plan time used.
    pub color_image: NeutralColorImage,
    /// The staged `SetFillColor` wire value. Required and present in
    /// Fill cycle; irrelevant to one-/two-cycle rectangles, whose color
    /// comes from the combiner, so those commands preserve its absence.
    pub fill_color: Option<NeutralFillColor>,
    /// Index into the owning plan's ordered access list of this
    /// command's first `RenderTarget` write access.
    pub first_access_index: u32,
    /// How many consecutive accesses starting at `first_access_index`
    /// belong to this fill. `1` for a full-width fill, the rectangle's
    /// pixel height otherwise.
    pub access_count: u32,
    /// Index into the owning plan's ordered access list of this fill's
    /// colour-image SEED read, or `None` when the fill covers the whole
    /// target and needs no seed.
    ///
    /// A partial fill patches into pixels it does not itself write, and
    /// those pixels must carry their real guest value rather than a
    /// fabricated zero -- the same thing `fn64-render-reference` gets by
    /// seeding its target from RDRAM before every raw-RDP task
    /// (`backend/imp.rs:440-447`). The declaring backend records which
    /// declared read carries those bytes; `None` is a positive statement
    /// that none is needed, not an absence of information.
    pub seed_access_index: Option<u32>,
    pub before: Option<RdpStateIdentity>,
    pub after: RdpStateIdentity,
}

/// Neutral carrier for one decoded `SYNC_FULL` **site** (RDP opcode 0x29).
///
/// # This is a site, not a boundary observation
///
/// The name is deliberate. A `RdpFullSyncSite` records that the backend
/// walked a `SYNC_FULL` opcode at a known stream position and that the
/// sole DP completion slot was proved free before anything was touched.
/// It records nothing about whether a DP interrupt was subsequently
/// raised or observed.
///
/// The observation, when a producer can honestly make one, lives in the
/// capture's own [`fn64_render_ir::FullSyncBoundary`] -- reachable from
/// [`Self::boundary`] -- and specifically in its
/// `interrupt_after == Asserted`. Nothing on this struct duplicates or
/// summarizes that bit, because a second copy is a second thing to get
/// out of sync with the first.
///
/// Pushes zero [`ResourceAccess`] entries: a sync reads and writes no
/// resource. That is not a simplification -- `SYNC_FULL`'s effect is on
/// the RDP pipeline and the DP interrupt line, neither of which is a
/// journaled resource region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RdpFullSyncSite {
    pub location: RawDpcCommandLocation,
    pub raw_words: Box<[u32]>,
    /// Zero-based index of this site among the decoded stream's
    /// `SYNC_FULL` occurrences, matching
    /// [`fn64_render_ir::FullSyncOccurrence::ordinal`].
    pub ordinal: u32,
    /// The capture-time boundary record this site was bound to during
    /// decode, carried verbatim.
    ///
    /// `interrupt_after` is the *only* place an observation claim can
    /// live. A backend that merely reserved the DP slot supplies
    /// [`fn64_render_ir::DpInterruptState::Clear`] here; reading
    /// `Asserted` off this field is the sole way a consumer may conclude
    /// the interrupt was observed.
    pub boundary: fn64_render_ir::FullSyncBoundary,
    /// Whether the sole DP completion slot was proved free for this site
    /// before the backend touched anything.
    ///
    /// Nonclaim: `true` means a nonmutating reserve succeeded. It does
    /// **not** mean a DP event was scheduled, an interrupt was raised, or
    /// the guest observed one.
    pub dp_slot_reserved: bool,
}

/// Which wire command admitted this triangle: a genuine `RawTriangle`
/// (0xC8-0xCF family) versus one synthesized from a `TextureRectangle`/
/// `TextureRectangleFlip` (0x24/0x25) two-triangle expansion. Constructed
/// only at the two admission sites in `production_adapter.rs` -- see
/// that file's `RawTriangle`/`TextureRectangle` match arms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriangleSource {
    RawTriangle,
    TextureRectangle,
}

/// Pixel-space `left`/`top`/`right`/`bottom` bounds of a `TextureRectangle`
/// draw, RT64's `FixedRect`-equivalent (`rt64_rdp.cpp:1232`). `None` on
/// [`RdpTriangleCommand`] for `TriangleSource::RawTriangle`; `Some` only
/// for `TriangleSource::TextureRectangle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectViewportPixels {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// One neutral, borrowed semantic view of a decoded raw-DPC command.
/// `#[non_exhaustive]` because T1's private wgpu decoder is the sole
/// producer of the owning [`ExactValidatedRawDpcPlan`] and may need to
/// widen this set (still bounded to the frozen TMEM-only scope) without
/// an unrelated crate boundary break.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum RawDpcSemanticCommandRef<'plan> {
    /// `LoadBlock`/`LoadTile`/`LoadTLUT`: the complete materialized load
    /// semantics a physical executor needs, borrowed from the owning
    /// plan. [`TmemLoadSemantics::shape`] distinguishes which opcode
    /// produced it.
    TmemLoad(&'plan TmemLoadSemantics),
    /// A supported state/sync command carrying its own staged fields but
    /// no resource access -- required context for the load commands
    /// above.
    State(&'plan RdpStateCommand),
    /// One admitted `RawTriangle` draw command -- geometry only, no
    /// resource access and no before/after identity (see
    /// [`RdpTriangleCommand`]'s own doc for why).
    Triangle(&'plan RdpTriangleCommand),
    /// One admitted fill-cycle `FillRectangle` -- unlike every sibling
    /// here, this command declares N guest-visible `RenderTarget` write
    /// accesses (see [`RdpFillRectangleCommand`]).
    FillRectangle(&'plan RdpFillRectangleCommand),
    /// One decoded `SYNC_FULL` site. Declares zero resource accesses and,
    /// on its own, no DP-interrupt observation -- see
    /// [`RdpFullSyncSite`].
    FullSyncSite(&'plan RdpFullSyncSite),
}

/// Borrowed, nonextracting visitor over one validated plan's semantic
/// commands and resource accesses. Implementors receive read-only views;
/// nothing here can move a field out of the plan or reconstruct a
/// constructor for it.
pub trait ExactRawDpcPlanVisitor {
    fn command(&mut self, command: RawDpcSemanticCommandRef<'_>);
    fn access(&mut self, access: ResourceAccess);
}

/// Genuine `fn64-render`-owned neutral concrete representation of one
/// validated raw-DPC plan. This is not type erasure: every field is a
/// concrete fn64-render-ir/fn64-render value, never `Any`, `TypeId`, a
/// downstream private type, or a downcast hook.
///
/// Public access is nonextracting: the identity/count getters return
/// `Copy` facts, and [`Self::visit`] lends command/access views through
/// [`ExactRawDpcPlanVisitor`] without moving a field or exposing a
/// constructor. There is no public constructor: the only route to one is
/// [`ExactRawDpcPlanWriter`], reachable only through
/// [`RawDpcBackendAuthority::begin_plan`] with the exact paired queue
/// identity already checked.
#[derive(Debug)]
pub struct ExactValidatedRawDpcPlan {
    pub(super) source_identity: RawDpcSubmissionIdentity,
    pub(super) journal_identity: JournalIdentity,
    pub(super) commands: Box<[OwnedSemanticCommand]>,
    pub(super) accesses: Box<[ResourceAccess]>,
}

/// Owned storage backing one borrowed [`RawDpcSemanticCommandRef`]. Kept
/// private: only [`ExactValidatedRawDpcPlan::visit`] ever turns this back
/// into a borrowed view.
#[derive(Clone, Debug)]
pub(super) enum OwnedSemanticCommand {
    TmemLoad(TmemLoadSemantics),
    State(RdpStateCommand),
    Triangle(RdpTriangleCommand),
    FillRectangle(RdpFillRectangleCommand),
    FullSyncSite(RdpFullSyncSite),
}

impl OwnedSemanticCommand {
    fn as_ref(&self) -> RawDpcSemanticCommandRef<'_> {
        match self {
            Self::TmemLoad(semantics) => RawDpcSemanticCommandRef::TmemLoad(semantics),
            Self::State(state) => RawDpcSemanticCommandRef::State(state),
            Self::Triangle(triangle) => RawDpcSemanticCommandRef::Triangle(triangle),
            Self::FillRectangle(fill) => RawDpcSemanticCommandRef::FillRectangle(fill),
            Self::FullSyncSite(site) => RawDpcSemanticCommandRef::FullSyncSite(site),
        }
    }
}

impl ExactValidatedRawDpcPlan {
    pub const fn source_identity(&self) -> RawDpcSubmissionIdentity {
        self.source_identity
    }

    pub fn command_count(&self) -> usize {
        self.commands.len()
    }

    pub const fn journal_identity(&self) -> JournalIdentity {
        self.journal_identity
    }

    /// Lend every semantic command, then every resource access, to
    /// `visitor`. Order matches construction order; no field is moved.
    /// Generic over `V: ExactRawDpcPlanVisitor`, monomorphized per
    /// concrete visitor type at every call site -- no `dyn` here, so
    /// this call is never itself a vtable dispatch (only the trait-
    /// object entry into `RenderBackend`'s own methods is; see the sole-
    /// dynamic-dispatch documentation on that trait).
    pub fn visit<V: ExactRawDpcPlanVisitor>(&self, visitor: &mut V) {
        for command in &self.commands {
            visitor.command(command.as_ref());
        }
        for access in self.accesses.iter().copied() {
            visitor.access(access);
        }
    }
}
