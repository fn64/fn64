use super::*;

/// An index into `PlanCollector::triangles` (and, index-parallel with it,
/// `PlanCollector::triangle_commands`). Distinguished from
/// [`CommandIndex`] because the two spaces disagree: a triangle index
/// counts admitted triangles, a command index counts wire commands, and a
/// texture rectangle's two triangles share one command index but take two
/// consecutive triangle indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct TriangleIndex(u32);

impl TriangleIndex {
    pub(super) fn new(index: usize) -> Self {
        Self(u32::try_from(index).expect("triangle index fits in u32"))
    }

    pub(super) fn get(self) -> usize {
        self.0 as usize
    }
}

/// An index into `PlanCollector::raw_triangle_commands`. See
/// [`TriangleIndex`] for why this is not the same space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct CommandIndex(u32);

impl CommandIndex {
    pub(super) fn new(index: usize) -> Self {
        Self(u32::try_from(index).expect("command index fits in u32"))
    }

    pub(super) fn get(self) -> usize {
        self.0 as usize
    }
}

/// One admitted `Triangle` command's retrieved draw state, folded with its
/// index-parallel neutral tile table.
///
/// Previously two separate `Vec`s on `PlanCollector` (`triangles` and
/// `triangle_neutral_tiles`), pushed at the same site and read at the same
/// index at every call site; folding them into one `Vec<PlannedTriangle>`
/// makes that parallelism a type-level invariant instead of a convention
/// documented on each field.
pub(super) struct PlannedTriangle {
    pub(super) draw: Result<RetrievedTriangleDraw, MissingTriangleDrawState>,
    pub(super) neutral_tiles: [(
        Option<fn64_render::NeutralTileDescriptor>,
        Option<fn64_render::NeutralTileSize>,
    ); 8],
}

impl std::ops::Index<TriangleIndex> for Vec<PlannedTriangle> {
    type Output = PlannedTriangle;

    fn index(&self, index: TriangleIndex) -> &Self::Output {
        &self[index.get()]
    }
}

impl std::ops::Index<CommandIndex> for Vec<ScheduledRawTriangle> {
    type Output = ScheduledRawTriangle;

    fn index(&self, index: CommandIndex) -> &Self::Output {
        &self[index.get()]
    }
}

/// Collects every TMEM load in the complete neutral plan, in plan order
/// (`command_index` records each load's position among *every* plan
/// command, matching T1's own `push_decoded_raw_dpc` numbering, even though
/// `State` commands are not retained here), plus every access, plus every
/// admitted `Triangle` command's own vertices/command-time `OtherMode`/
/// `CombineParams` snapshot, exactly as
/// [`fn64_render::ExactValidatedRawDpcPlan::visit`] lends them through
/// [`BoundSubmittedRawDpc::execution_view`]/
/// [`RawDpcCoordinator::execution_view`] -- nonextracting, borrowed for the
/// duration of one `execution_view` call only. This is the sole route
/// `execute_raw_dpc` uses to reach plan contents; it never widens access to
/// a bare ticket. `State` commands other than `SetOtherMode`/`SetCombine`
/// (`SetTile`/`SetTileSize`/`SetTextureImage`/`SyncLoad`, etc.) carry no
/// resource access of their own and no field this executor reads --
/// `TmemLoadSemantics` already carries its own staged
/// `source_image`/`tile_descriptor`/`epoch` directly -- so they are counted
/// for `command_index` continuity but not stored.
///
/// The `Triangle`/`SetOtherMode`/`SetCombine` handling below deliberately
/// duplicates `raw_dpc::triangle_draw_data::TriangleDrawStateCollector`'s
/// exact per-command logic (the walk-local [`RdpDrawState`]'s
/// `other_mode`/`combine`, snapshotted onto each triangle at its own stream
/// position, never a single whole-plan-final value) rather than reusing
/// that type directly: `RawDpcExecutionView::plan_visited` is generic over
/// exactly one visitor type, fixed at this file's own `execute_raw_dpc_
/// inner` call site, so there is no route to lend one sealed plan to two
/// independent visitors in the same `execution_view` call. This is a
/// duplication of behavior, not of trust -- if `TriangleDrawStateCollector`
/// changes, this file's own copy must be updated to match.
pub(super) struct PlanCollector {
    pub(super) loads: Vec<(u32, TmemLoadSemantics)>,
    pub(super) accesses: Vec<ResourceAccess>,
    pub(super) next_command_index: u32,
    /// Every durable RDP draw register current at the walk's current stream
    /// position -- seeded from `WgpuBackend.rdp_state`'s durable values at
    /// construction time (`Self::seeded`), then advanced by
    /// [`RdpDrawState::apply`] on every state command in plan order.
    ///
    /// Each snapshot taken onto a `PlannedTriangle` or a fill is taken at
    /// that command's own stream position -- never a single
    /// whole-plan-final value.
    pub(super) draw: RdpDrawState,
    /// One entry per admitted `Triangle` command, in plan order. `draw`'s
    /// `Err` names exactly which state (`OtherMode` or `CombineParams`) was
    /// still unset at that triangle's own stream position -- never a
    /// silent default, matching `TriangleDrawStateCollector`'s own
    /// documented absence handling. `neutral_tiles` is the **neutral**
    /// `SetTile`/`SetTileSize` table current at that same stream position
    /// (see [`PlannedTriangle`] for why this is folded rather than a
    /// second parallel `Vec`).
    pub(super) triangles: Vec<PlannedTriangle>,
    /// One entry per admitted `FillRectangle`, in plan order: its
    /// decode-order command index, command, and render state current at
    /// **its own** stream position. Fill cycle consumes the command's fill
    /// color; one-/two-cycle consumes the snapshotted combiner and color
    /// registers through the existing texture-rectangle pixel stages.
    ///
    /// The `OtherMode` snapshot is not redundant with
    /// `draw.other_mode`. That field is the walk's running value, which
    /// by the end of the plan holds whatever the *last* `SetOtherMode` set
    /// -- and a real stream sets Fill cycle for the fill and then Copy
    /// cycle for a following texture rectangle, so reading the running
    /// value at execute time rejects the fill with `NotFillCycle` for a
    /// mode it never ran under. Snapshotting per command is the same rule
    /// `triangles` already follows and states in its own doc: "never a
    /// single whole-plan-final value".
    ///
    /// The `RdpScissorRect` snapshot is taken at the same stream position
    /// and for the same reason. Pinned RT64 intersects its current scissor
    /// with each draw rectangle (`src/hle/rt64_rdp.cpp:1214-1223`, commit
    /// `f0728a2`), so fn64 snapshots the scissor current where THIS fill
    /// sits, not the one a later `SetScissor` installs for a following
    /// primitive. The exact relatch sequencing is fn64's own reading and is
    /// not independently confirmed against an allowed hardware reference.
    /// `None` here means
    /// the plan issued no `SetScissor` before this fill, and the consumer
    /// (`fill_scissor_or_full_target`) supplies the whole-target fallback,
    /// exactly as the texrect path already does.
    pub(super) fills: Vec<(
        u32,
        fn64_render::RdpFillRectangleCommand,
        Option<OtherMode>,
        Option<crate::targets::RdpScissorRect>,
        Option<CombineParams>,
        Color4,
        PrimColor,
        Color4,
        Color4,
    )>,
    /// One entry per admitted `FullSync` site, in plan order, paired with
    /// its own decode-order command index.
    ///
    /// Collected for accounting only -- this backend performs no GPU work
    /// for a sync and schedules no DP completion (the device fabric does
    /// that, from the ABI seam). Retaining the site keeps the executed plan
    /// able to account for every command it carried instead of silently
    /// losing one.
    pub(super) full_sync_sites: Vec<(u32, fn64_render::RdpFullSyncSite)>,
    /// The wire command index each admitted triangle was produced at,
    /// parallel to `triangles` and pushed at the same site each
    /// `PlannedTriangle` is.
    ///
    /// A texture rectangle contributes two entries carrying the *same*
    /// index: both halves come from one wire command and both must sample
    /// TMEM as of that one stream position. Splitting them would let a
    /// rectangle's two triangles disagree about which load they saw.
    ///
    /// Needed because the GPU raster path selects a TMEM projection per
    /// triangle for exactly the reason the CPU texel reader selects a
    /// prefix per texrect: within one packet, TMEM is not one image.
    pub(super) triangle_commands: Vec<u32>,
    /// One entry per admitted `TextureRectangle` **wire command**, in plan
    /// order: the declared `RenderTarget` write-access span the decoder
    /// recorded for it (`None` when it declared no write), and the index
    /// into `triangles` of the first of the two triangles it was admitted
    /// as.
    ///
    /// A texrect is admitted as two `TriangleSource::TextureRectangle`
    /// triangles (`production_adapter`'s own split) and **both halves carry
    /// the identical span**, so counting texrects means counting distinct
    /// originating commands, not triangles -- adjacent pairs are collapsed
    /// here. Counting triangles instead would double every texrect and
    /// reject a single legal one as two.
    pub(super) texrect_commands: Vec<(
        Option<fn64_render::TriangleAccessSpan>,
        usize,
        u8,
        u32,
        bool,
    )>,
    /// One entry per admitted **flat raw triangle** that declared a
    /// destination write: its declared access span, its index into
    /// `triangles`, its own stream command index, and its exact decoded
    /// wire payload.
    ///
    /// Separate from `texrect_commands` because the two use different
    /// executors, and the pairing rule that collapses a texrect's two halves
    /// has no analogue here: a raw triangle is exactly one triangle. Merging
    /// them would mean re-deriving "which kind is this" at execute time from
    /// a field the collector already knows at push time.
    ///
    /// A raw triangle that declared NO write is absent from this list
    /// entirely -- `None` and "not present" would be the same value, and
    /// this list's only consumer is the schedule, which must not schedule
    /// an undeclared triangle.
    pub(super) raw_triangle_commands: Vec<ScheduledRawTriangle>,
}

pub(super) struct ScheduledRawTriangle {
    pub(super) span: fn64_render::TriangleAccessSpan,
    pub(super) triangle_index: TriangleIndex,
    pub(super) command_index: u32,
    pub(super) decoded: Result<crate::raw_dpc::RawTriangle, ScheduledRawTriangleDecodeError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScheduledRawTriangleDecodeError {
    MissingOpcode,
    Decode(crate::raw_dpc::TriangleDecodeError),
}

impl PlanCollector {
    /// Seeds the whole [`RdpDrawState`] from `WgpuBackend`'s own durable
    /// `rdp_state` instead of `None` -- a real constructor parameter, never
    /// a synthetic plan-stream entry. This is the draw-state-*retrieval*
    /// half of durable cross-submission carry-in; the admission-time half
    /// (`push_decoded_raw_dpc`'s own `TriangleBeforeAnyOtherMode` gate)
    /// already seeds identically from the same `rdp_state`, via
    /// `decode_raw_dpc`'s existing `durable_state` parameter -- see this
    /// card's own design notes for why neither half needed a signature
    /// change to close this gap.
    pub(super) fn seeded(carry_in: RawDpcCarryIn) -> Self {
        Self {
            loads: Vec::new(),
            accesses: Vec::new(),
            next_command_index: 0,
            draw: carry_in.draw,
            triangles: Vec::new(),
            fills: Vec::new(),
            full_sync_sites: Vec::new(),
            triangle_commands: Vec::new(),
            texrect_commands: Vec::new(),
            raw_triangle_commands: Vec::new(),
        }
    }
}

impl ExactRawDpcPlanVisitor for PlanCollector {
    fn command(&mut self, command: RawDpcSemanticCommandRef<'_>) {
        let command_index = self.next_command_index;
        self.next_command_index += 1;
        match command {
            RawDpcSemanticCommandRef::TmemLoad(load) => {
                self.loads.push((command_index, load.clone()));
            }
            // Every state command's update rule lives in
            // `RdpDrawState::apply` -- the plan walk and the carry-in value
            // advance by the same code.
            RawDpcSemanticCommandRef::State(state) => self.draw.apply(state),
            RawDpcSemanticCommandRef::Triangle(RdpTriangleCommand {
                vertices,
                source,
                viewport,
                texrect_accesses,
                raw_words,
                ..
            }) => {
                let triangle_index = self.triangles.len();
                // **The tile this draw actually names, not tile 0.**
                //
                // A `TextureRectangle` selects its tile in wire word 1 bits
                // 26:24 -- the same field the `texrect_commands` push below
                // already reads, from the same retained `raw_words`, so this
                // is one field read at one more site, not a second decode
                // path. Binding tile 0 unconditionally was measured to
                // report `TMEM_SAMPLE_STATUS_NO_TILE_BINDING` for every
                // texrect naming any other tile, and this crate's own
                // composed fixtures name tile 7; two of them had to be moved
                // to tile 0 to exercise the GPU path at all before this.
                //
                // A `RawTriangle` names its tile in wire word 0 bits
                // 18:16 -- the same field `RawTriangle::decode` reads as
                // `tile` and `execute_scheduled_raw_triangle` (the CPU
                // reader) already binds from. This arm previously froze the
                // index to 0, with a comment claiming the triangle "carries
                // no tile field of its own to read". That claim was false,
                // and the consequence was silent: a triangle naming any
                // other tile had the GPU uniform sample tile 0's descriptor
                // instead of its own. Reading the field here from the
                // command's own retained `raw_words` is the same one-field
                // read the texrect arm above already performs, so the two
                // paths resolve the SAME tile for the same triangle.
                //
                // `draw.tiles` is the whole 8-entry table as of this
                // command's stream position (the same table
                // `triangle_neutral_tiles` snapshots for the CPU reader), so
                // the two paths now resolve the SAME tile for the same
                // draw -- texrect or raw triangle alike -- instead of
                // disagreeing whenever tile != 0.
                // The shared implementation, not a second copy: this file
                // and `TriangleDrawStateCollector` must resolve the same
                // tile for the same draw, and when each carried its own
                // copy of this arithmetic they drifted.
                let bound_tile_index = crate::raw_dpc::bound_tile_index(*source, raw_words);
                let tile_binding = match self
                    .draw
                    .tiles
                    .get(bound_tile_index)
                    .copied()
                    .unwrap_or((None, None))
                {
                    (Some(descriptor), Some(size)) => {
                        TileBindingParams::from_neutral(descriptor, size)
                    }
                    _ => TileBindingParams::unbound(),
                };
                let snapshot = (|| {
                    let other_mode = self
                        .draw
                        .other_mode
                        .ok_or(MissingTriangleDrawState::NoOtherMode { triangle_index })?;
                    let combine_params = self
                        .draw
                        .combine
                        .ok_or(MissingTriangleDrawState::NoCombine { triangle_index })?;
                    // Retrieval-time admission gate (card §4a), duplicated
                    // from `TriangleDrawStateCollector` per this struct's
                    // own module doc: `Dither` never reaches
                    // `submit_admitted_triangle` -- a loud, named panic here,
                    // not a silent None/Threshold coercion. Wire encoding 2
                    // is not reserved. Pinned RT64's shader branches only
                    // for `G_AC_DITHER` and `G_AC_THRESHOLD`, so wire
                    // encoding 2 falls through to no compare
                    // (`src/shaders/RasterPS.hlsl:203-213`, commit
                    // `f0728a2`).
                    match other_mode.alpha_compare() {
                        AlphaCompare::Dither => panic!(
                            "triangle #{triangle_index} (plan order) selected G_AC_DITHER \
                             alpha-compare, which has no fragment-callable RT64 PRNG binding in \
                             this pipeline (no frame-count uniform exists to seed it honestly; \
                             see fn64-alpha-compare-production-card.md \u{a7}2)"
                        ),
                        // `Threshold` compares the fragment alpha against
                        // `G_SETBLENDCOLOR.a`. That register always holds a
                        // value -- zero until the guest writes one -- so
                        // there is nothing to refuse here: a plan with no
                        // `SetBlendColor` compares against 0, and
                        // `alpha >= 0` passes, which is what the reference
                        // lane and RT64 both do. See `RdpState`'s
                        // constant-color field doc for the citations.
                        AlphaCompare::Threshold | AlphaCompare::None => {}
                    };
                    Ok(RetrievedTriangleDraw {
                        vertices: *vertices,
                        source: *source,
                        viewport: *viewport,
                        other_mode,
                        combine_params,
                        tile_binding,
                        blend_color: self.draw.blend_color,
                        env_color: self.draw.env_color,
                        prim_color: self.draw.prim_color,
                        fog_color: self.draw.fog_color,
                        scissor: self.draw.scissor,
                        prim_depth: self.draw.prim_depth,
                    })
                })();
                // The whole tile table as of this command's own stream
                // position, not just tile 0's entry: a texture rectangle
                // names its own tile in its wire word and this file cannot
                // know which until it reads that word at execute time.
                self.triangles.push(PlannedTriangle {
                    draw: snapshot,
                    neutral_tiles: self.draw.tiles,
                });
                // **A texture rectangle's two triangles take the FIRST
                // half's command index, not their own.**
                //
                // The adapter assigns each half its own index -- measured
                // on the sprite-strip fixture, the halves are (11, 12),
                // (20, 21), ... -- so pushing `command_index` unchanged
                // would let the two halves select prefixes independently.
                // In that fixture no load falls between 11 and 12, so it
                // happens to be harmless; that is an accident of the
                // spacing, not a guarantee. A rectangle whose halves
                // straddled a load would tear along its own diagonal, one
                // triangle carrying texels the other never saw.
                //
                // The pairing rule is `texrect_commands`' own
                // `previous_was_first_half`, applied here rather than
                // re-derived, so one fact about "which triangles are one
                // rectangle" has one implementation.
                let second_half = *source == TriangleSource::TextureRectangle
                    && self
                        .texrect_commands
                        .last()
                        .is_some_and(|(_, first, _, _, _)| *first + 1 == triangle_index);
                self.triangle_commands.push(if second_half {
                    *self
                        .triangle_commands
                        .last()
                        .expect("a second half always follows a first half that pushed its own")
                } else {
                    command_index
                });
                // One texture rectangle is admitted as TWO
                // `TriangleSource::TextureRectangle` triangles sharing one
                // originating wire command, and the adapter pushes them
                // back to back with identical `location`. Recording only
                // the first of each adjacent pair recovers the count of
                // *commands*, which is what the declared-write span is
                // keyed on -- counting triangles would double every
                // texrect and reject a single legal one as "two".
                // A raw triangle carries its own declared span in the same
                // field a texrect uses, because the adapter pushes both
                // through one `RdpTriangleCommand`. `None` means it declared
                // no write -- outside the flat-opaque subset, no staged
                // colour image, Fill cycle, or a row outside installed RDRAM
                // -- and it is simply absent here, so the schedule cannot
                // reach it.
                if *source == TriangleSource::RawTriangle {
                    if let Some(span) = *texrect_accesses {
                        // Decode the triangle's authoritative wire words
                        // directly into the exact scheduled carrier. The
                        // neutral plan still owns those words for provenance;
                        // the executor must not reconstruct coefficients from
                        // the lossy projected `NeutralTriangleVertex` triple.
                        let decoded = raw_words.first().map_or(
                            Err(ScheduledRawTriangleDecodeError::MissingOpcode),
                            |word| {
                                let opcode = ((word >> 24) & 0x3f) as u8;
                                crate::raw_dpc::RawTriangle::decode_u32_words(opcode, raw_words)
                                    .map_err(ScheduledRawTriangleDecodeError::Decode)
                            },
                        );
                        self.raw_triangle_commands.push(ScheduledRawTriangle {
                            span,
                            triangle_index: TriangleIndex::new(triangle_index),
                            command_index,
                            decoded,
                        });
                    }
                }
                if *source == TriangleSource::TextureRectangle {
                    let previous_was_first_half = self
                        .texrect_commands
                        .last()
                        .is_some_and(|(_, first, _, _, _)| *first + 1 == triangle_index);
                    if !previous_was_first_half {
                        // The tile index is wire word 1 bits 26:24, the
                        // same field `texrect_words_in_target` writes and
                        // `RawTextureRectangle` decodes. Read from the
                        // command's own retained `raw_words` rather than
                        // re-decoded from a second source.
                        let tile_index = raw_words
                            .get(1)
                            .map(|word| ((word >> 24) & 0x7) as u8)
                            .unwrap_or(0);
                        self.texrect_commands.push((
                            *texrect_accesses,
                            triangle_index,
                            tile_index,
                            command_index,
                            raw_words
                                .first()
                                .is_some_and(|word| ((word >> 24) & 0x3f) == 0x25),
                        ));
                    }
                }
            }
            // Mandatory alongside `push_fill_rectangle`'s admission: the
            // enum is `#[non_exhaustive]`, so a produced variant with no arm
            // here falls into the catch-all below and panics at execute time
            // rather than failing to compile.
            RawDpcSemanticCommandRef::FillRectangle(fill) => {
                self.fills.push((
                    command_index,
                    fill.clone(),
                    self.draw.other_mode,
                    self.draw.scissor,
                    self.draw.combine,
                    self.draw.env_color,
                    self.draw.prim_color,
                    self.draw.blend_color,
                    self.draw.fog_color,
                ));
            }
            // Mandatory alongside `push_full_sync_site`'s admission, for the
            // same `#[non_exhaustive]` reason as the arm above.
            //
            // Collected, not executed. A `SYNC_FULL` site has no GPU work:
            // its whole effect is on the RDP pipeline and the DP interrupt
            // line, and the DP completion is scheduled by the device fabric
            // (`start_dp_full_sync`, driven from the ABI seam), never by this
            // backend. Dropping it silently would be wrong in the other
            // direction, though -- the site is retained so the executed plan
            // still accounts for every command the plan carried.
            //
            // Nonclaim: retaining a site here is not an observation of a DP
            // interrupt. `site.boundary.interrupt_after()` is the only field
            // that could carry one, and this backend never writes it.
            RawDpcSemanticCommandRef::FullSyncSite(site) => {
                self.full_sync_sites.push((command_index, site.clone()));
            }
            other => unreachable!(
                "RawDpcSemanticCommandRef gained a variant WgpuBackend does not know about: \
                 {other:?}"
            ),
        }
    }

    fn access(&mut self, access: ResourceAccess) {
        self.accesses.push(access);
    }
}
/// `plan_raw_dpc`'s body: decode `request`'s capture once through T1's typed
/// planning decoder, push every admitted command through T0's sealed writer,
/// and seal the decoder-derived journal. The seed journal exists only to
/// satisfy capture preflight; [`crate::raw_dpc::PlanningDecodedRawDpc`]
/// cannot enter ordinary execution and carries the authoritative access plan
/// derived during that same decode.
///
/// **The decode observes `durable_state`, and that is load-bearing.**
/// The journal a capture declares is not a function of its bytes alone: a
/// `FillRectangle`/`TextureRectangle` reads its destination back off
/// `RdpState::color_image()`, which an *earlier* submission's
/// `SetColorImage` may have staged. `plan_texture_rectangle` treats a
/// missing color image as "declares no write" (`return Ok(())`) rather than
/// as an error, so deriving against `RdpState::default()` silently returns a
/// shorter access list. The derivation therefore observes the same durable
/// predecessor as command planning.
///
/// Decoding against durable state is side-effect-free:
/// `decode_raw_dpc` takes `&RdpState` and forks it (`fork_for_decode`), so
/// neither pass can mutate the caller's state, and only the real pass's
/// `state_delta` is ever applied. Every `SubmittedTicket`
/// minted here is through a throwaway, locally owned `TicketAuthoritySet` --
/// `crate::decode_raw_dpc` only needs one that is internally consistent
/// with the capture it decodes, never the "real" production queue (that
/// queue identity is proven separately, by `RawDpcBackendAuthority::
/// begin_plan`'s own paired-queue assertion against `request`).
pub(super) fn plan_raw_dpc_inner(
    coordinator: &RawDpcCoordinator<PhysicalTmemState>,
    durable_state: &RdpState,
    request: RawDpcPlanRequest,
) -> Result<
    (
        PlannedRawDpcSubmission,
        crate::RdpStateDelta,
        PlannedTaskExecution,
    ),
    String,
> {
    let capture = request.capture();
    let layout = capture.memory_layout();
    let submission = capture.submission().clone();
    let submission_start = submission.start();
    let capture_words = submission.command_words();

    let ticket = raw_dpc_plan_census::timed(raw_dpc_plan_census::Phase::Prepare, || {
        let seed_journal = planning_seed_journal(&submission, layout)
            .map_err(|error| format!("raw-DPC plan seed journal failed: {error}"))?;
        let decoded_ticket = finalize_with_zero_reads(
            layout,
            capture.transaction_sequence(),
            submission,
            capture.cmd_end(),
            capture.full_sync_boundaries().to_vec(),
            seed_journal,
        )
        .map_err(|error| format!("raw-DPC plan preflight failed: {error}"))?;
        submit_locally(decoded_ticket)
            .map_err(|error| format!("raw-DPC plan submission failed: {error}"))
    })?;

    let decoded = raw_dpc_plan_census::timed(raw_dpc_plan_census::Phase::DecodeAndDerive, || {
        crate::raw_dpc::decode_raw_dpc_for_planning(ticket, durable_state)
    })
    .map_err(|error| format!("raw-DPC plan decode failed: {error}"))?;
    let accesses = decoded.resource_plan().accesses().to_vec();
    let declared = accesses
        .iter()
        .map(|access| access.region().declared_bytes())
        .sum::<u32>();
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(fn64_render_ir::MAX_RESOURCE_ACCESSES, declared.max(1))
            .map_err(|error| format!("raw-DPC plan journal limits failed: {error}"))?,
        accesses,
    )
    .map_err(|error| format!("raw-DPC plan journal failed: {error}"))?;
    let delta = decoded.state_delta().clone();
    let execution = classify_task_execution(decoded.commands(), durable_state);

    let planned = raw_dpc_plan_census::timed(raw_dpc_plan_census::Phase::AdmitAndSeal, || {
        let mut writer = coordinator.begin_plan(request);
        push_planning_decoded_raw_dpc(
            &mut writer,
            &decoded,
            &capture_words,
            layout,
            submission_start,
        )
        .map_err(|error| format!("raw-DPC plan admission failed: {error}"))?;
        writer
            .finish(journal)
            .map_err(|error| format!("raw-DPC plan seal failed: {error}"))
    })?;
    Ok((planned, delta, execution))
}

pub(super) fn classify_task_execution(
    commands: &[crate::DecodedRawDpcCommand],
    durable_state: &RdpState,
) -> PlannedTaskExecution {
    let has_raw_triangle = commands
        .iter()
        .any(|command| matches!(command.kind(), crate::RawDpcCommandKind::RawTriangle(_)));
    if !has_raw_triangle {
        return PlannedTaskExecution::Cpu(PlannedTaskCpuReason::NoRawTriangle(
            classify_no_raw_triangle(commands),
        ));
    }
    if commands.iter().any(|command| {
        matches!(
            command.kind(),
            crate::RawDpcCommandKind::FillRectangle(_)
                | crate::RawDpcCommandKind::TextureRectangle(_)
        )
    }) {
        return PlannedTaskExecution::Cpu(PlannedTaskCpuReason::MixedFillOrTexrect);
    }

    let mut other_mode = durable_state.other_mode();
    let mut combine = durable_state.combine();
    for command in commands {
        match command.kind() {
            crate::RawDpcCommandKind::SetOtherMode(value) => other_mode = Some(value),
            crate::RawDpcCommandKind::SetCombine(value) => combine = Some(value),
            crate::RawDpcCommandKind::RawTriangle(triangle) => {
                let (Some(combine), Some(other_mode)) = (combine, other_mode) else {
                    // Missing state remains a runtime admission concern so its
                    // existing loud typed refusal is not converted to CPU.
                    continue;
                };
                if let Err(reason) = ComputeRasterProgramKey::try_admit_program(
                    combine,
                    other_mode,
                    triangle.flags().textured(),
                ) {
                    return PlannedTaskExecution::Cpu(PlannedTaskCpuReason::DefinitelyCpu(
                        reason.into(),
                    ));
                }
            }
            _ => {}
        }
    }
    PlannedTaskExecution::ComputeCandidate
}

pub(super) fn classify_no_raw_triangle(
    commands: &[crate::DecodedRawDpcCommand],
) -> PlannedNoRawTriangleReason {
    let mut fill = false;
    let mut texrect = false;
    let mut tmem_load = false;
    let mut sync_or_state = false;
    for command in commands {
        match command.kind() {
            crate::RawDpcCommandKind::FillRectangle(_) => fill = true,
            crate::RawDpcCommandKind::TextureRectangle(_) => texrect = true,
            crate::RawDpcCommandKind::LoadBlock(_)
            | crate::RawDpcCommandKind::LoadTile(_)
            | crate::RawDpcCommandKind::LoadTlut(_) => tmem_load = true,
            crate::RawDpcCommandKind::NoOp { .. } => {}
            crate::RawDpcCommandKind::RawTriangle(_) => {
                unreachable!("no-triangle classification received a raw triangle")
            }
            _ => sync_or_state = true,
        }
    }
    classify_no_raw_triangle_flags(fill, texrect, tmem_load, sync_or_state)
}

pub(super) fn classify_no_raw_triangle_flags(
    fill: bool,
    texrect: bool,
    tmem_load: bool,
    sync_or_state: bool,
) -> PlannedNoRawTriangleReason {
    match (fill, texrect, tmem_load) {
        (true, false, false) => PlannedNoRawTriangleReason::FillOnly,
        (false, true, false) => PlannedNoRawTriangleReason::TexrectOnly,
        (true, true, false) => PlannedNoRawTriangleReason::FillAndTexrect,
        (false, false, true) => PlannedNoRawTriangleReason::TmemLoadOnly,
        (true, false, true) => PlannedNoRawTriangleReason::FillAndTmemLoad,
        (false, true, true) => PlannedNoRawTriangleReason::TexrectAndTmemLoad,
        (true, true, true) => PlannedNoRawTriangleReason::FillTexrectAndTmemLoad,
        (false, false, false) if sync_or_state => PlannedNoRawTriangleReason::SyncStateOnly,
        (false, false, false) => PlannedNoRawTriangleReason::NoOpOnly,
    }
}

pub(super) fn submit_locally(decoded: DecodedTicket) -> Result<SubmittedTicket, ValidationError> {
    let (mut queue, _, _) = TicketAuthoritySet::try_new()?.into_roles();
    queue.submit(decoded)
}

/// `fn64_render::decode_raw_dpc_capture` hard-codes
/// `DeferredGuestReadCapture::empty()`, which only satisfies a plan whose
/// guest-read plan is itself empty -- never true here, since every admitted
/// TMEM load declares at least one `TmemLoadSource` read. `plan_raw_dpc`'s
/// internal planning decode exists purely to learn the command
/// structure/journal shape and drive T1's push loop -- it is not the
/// production submission the ABI session's `finalize_and_submit` performs
/// later with the real captured bytes -- so a correctly *sized*, zero-filled
/// capture is exactly as valid here as any other byte content: `finish`'s
/// own access-count/order check never inspects read content, only shape.
///
/// `full_sync_boundaries` is NOT zero-filled the way the read bytes are, and
/// must be the originating capture's own list. Stream derivation requires one
/// boundary per decoded `SYNC_FULL` opcode, so an empty list here would fail
/// the planning decode with `MissingFullSyncObservation` for any
/// capture containing a FullSync -- making the site unplannable no matter
/// what its producer supplied. Shape, unlike content, is load-bearing here.
pub(super) fn finalize_with_zero_reads(
    layout: fn64_render_ir::PhysicalMemoryLayout,
    transaction_sequence: u64,
    submission: fn64_render::OwnedRawDpcSubmission,
    cmd_end: fn64_render_ir::TemporalBoundary,
    full_sync_boundaries: Vec<fn64_render_ir::FullSyncBoundary>,
    journal: ResourceJournal,
) -> Result<DecodedTicket, ValidationError> {
    let preflight = fn64_render::preflight_raw_dpc_capture(
        layout,
        transaction_sequence,
        submission,
        cmd_end,
        full_sync_boundaries,
        journal,
    )?;
    let capture = fn64_render_ir::DeferredGuestReadCapture::new(
        preflight
            .guest_read_plan()
            .reads()
            .iter()
            .map(|read| {
                fn64_render_ir::CapturedGuestRead::try_new(
                    *read,
                    vec![0; read.range().len() as usize],
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    preflight.finalize(capture)
}

/// A minimal, self-consistent command-decode seed journal sufficient to pass
/// capture preflight.
/// The typed planning decoder derives the authoritative access list instead
/// of admitting this seed as execution authority.
///
/// The command-decode access's region kind must match `submission.source()`
/// exactly (`fn64_render_ir::workload::validate_one_to_one_command_reads`
/// keys a stream's expected read by `RawStreamKind`, not by byte range
/// alone): `RawDpcSource::Rdram` needs `ResourceRegion::Rdram { resource:
/// RdramResource::RawCommands, .. }`; `RawDpcSource::XbusDmem` needs
/// `ResourceRegion::RspDmem(DmemRange)`, the same 4 KiB DMEM-relative
/// address space `submission.start()`/`end()` are already expressed in for
/// an XBUS submission (`OwnedRawDpcSubmission::validate_range` bounds XBUS
/// ranges to `RSP_DMEM_BYTES`, never the RDP's 24-bit physical space).
///
/// It contains no speculative TMEM source: the typed planning decode derives
/// those exact ranges, and preflight need not allocate or zero bytes for a
/// placeholder read that execution can never consume.
pub(super) fn planning_seed_journal(
    submission: &fn64_render::OwnedRawDpcSubmission,
    layout: fn64_render_ir::PhysicalMemoryLayout,
) -> Result<ResourceJournal, ValidationError> {
    use fn64_render_ir::{DmemRange, OperationId, RdramResource, ResourceRegion};
    let start = submission.start();
    let command_bytes = u32::try_from(submission.command_words().len() * 4)
        .expect("bounded command stream fits u32 bytes");
    let command_access = ResourceAccess::try_new(
        OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        match submission.source() {
            fn64_render::RawDpcSource::Rdram => ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: layout.range(start, start + command_bytes)?,
            },
            fn64_render::RawDpcSource::XbusDmem => {
                ResourceRegion::RspDmem(DmemRange::try_new(start, start + command_bytes)?)
            }
        },
    )?;
    let accesses = vec![command_access];
    let declared = accesses
        .iter()
        .map(|access| access.region().declared_bytes())
        .sum::<u32>();
    ResourceJournal::try_new(
        ResourceJournalLimits::try_new(64, declared.max(1))?,
        accesses,
    )
}

#[cfg(test)]
pub(super) fn single_source_probe_journal(
    submission: &fn64_render::OwnedRawDpcSubmission,
    layout: fn64_render_ir::PhysicalMemoryLayout,
) -> Result<ResourceJournal, ValidationError> {
    planning_seed_journal(submission, layout)
}

/// `plan_raw_dpc` always constructs raw-DPC admission (via
/// `fn64_render::decode_raw_dpc_capture`, which hard-codes
/// `WorkloadAdmission::RawDpc`), and `RawDpcCoordinator::execution_view`
/// only ever lends a plan T1's raw-DPC push loop admitted -- a graphics-task
/// packet can never reach this executor. Traps rather than silently
/// defaulting a sequence number, matching AGENTS.md's loud-trap rule.
pub(super) fn transaction_sequence(packet: &WorkloadPacket) -> u64 {
    match packet.admission() {
        WorkloadAdmission::RawDpc {
            transaction_sequence,
        } => transaction_sequence,
        WorkloadAdmission::GraphicsTask(_) => unreachable!(
            "WgpuBackend's raw-DPC execution seam only ever receives RawDpc-admitted packets"
        ),
    }
}
