use super::geometry::clip_texrect_extent;
use super::shading::RankOneCi4Rgba16;
use super::*;

/// Executes one admitted `TextureRectangle` against `candidate`, sampling
/// every texel from `tmem` -- any [`TmemByteSource`], which in practice is
/// one of exactly two images the caller chooses between by a rule this
/// function does not apply:
///
/// - a [`PendingTmemImage`](crate::tmem::PendingTmemImage), the sealed
///   post-image of the **same packet's**
///   own TMEM loads, for a packet that carries at least one load; or
/// - the durable [`PhysicalTmemState`] the coordinator holds, for a packet
///   that carries **no** load at all and therefore samples what an earlier
///   packet already published.
///
/// Generic rather than two overloads for the same reason
/// [`sample_point`] is: one addressing/validity/XOR4/TLUT path, so the two
/// images cannot disagree about a texel. The distinction survives in the
/// data, not the signature -- a sampled texel's `snapshot()` answers
/// `Proposed` for the post-image and `Committed` for durable state, and the
/// caller checks that crossing rather than trusting it.
///
/// Produces the same [`CompletedColorTargetWrite`] the fill executor
/// produces, so the two compose at the identical
/// `admit_completed_initialization` seam. `resident_bytes` carries the
/// target's current full-extent device bytes and is **required**: a texrect
/// always writes a sub-rectangle, so every pixel outside it must come from
/// real prior content. In the composed fill+texrect shape those prior bytes
/// are the fill's own output, which is why this executor runs after the fill
/// and takes its bytes as input rather than writing into a separate buffer.
///
/// `already_initialized` is the region `resident_bytes` was itself proven
/// to cover -- the fill's own claimed rectangle in the composed shape,
/// `None` when the bytes come from a resident whose coverage this executor
/// does not re-establish. It only widens the claimed output rectangle; it
/// never changes a pixel.
///
/// Ordering is therefore load-bearing and observable: this reads `tmem`'s
/// post-image, so a `LoadBlock` staged before this call is visible and one
/// staged after is not.
#[allow(clippy::too_many_arguments)]
pub fn execute_texture_rectangle<'a, S: crate::TmemByteSource + ?Sized>(
    candidate: &CandidateColorTarget,
    other_mode: OtherMode,
    draw: TexrectDraw,
    tile: TexrectTileBinding,
    tmem: &S,
    lut_mode: TextureLutMode,
    shading: TexrectShading,
    blend_registers: TexrectBlendRegisters,
    scissor: RdpScissorRect,
    resident_bytes: impl Into<Cow<'a, [u8]>>,
    already_initialized: Option<TargetRectangle>,
) -> Result<CompletedColorTargetWrite, TexrectExecutionError> {
    let timing = texrect_timing_census::StartedDraw::if_enabled(
        shading.combine(),
        other_mode,
        candidate.key().format(),
        lut_mode,
        tile,
        draw,
        u64::from(draw.width()) * u64::from(draw.height()),
    );
    let mut bytes = resident_bytes.into().into_owned();
    // Copy cycle blits the texel to the destination with no combiner, which
    // is what the RDP itself does in that mode. One-cycle runs the texel
    // through the color combiner once per fragment; two-cycle runs it twice
    // with the cross-cycle carry, both through `crate::combiner`'s own
    // evaluators. Fill cycle samples no texture at all and is refused by
    // name rather than drawn as an approximation.
    let evaluation = admitted_cycle_evaluation(other_mode.cycle_type())?;
    // Selector admission runs before any pixel is produced, so an
    // unevaluatable program refuses with an untouched target rather than a
    // half-drawn one. Skipped in Copy cycle, where the RDP consults no
    // combiner program at all and gating on one would refuse a rectangle
    // the hardware draws.
    let base_inputs = match evaluation.validated_cycles() {
        Some(cycles) => Some(shading.validate_combiner_program(cycles)?.base_inputs()),
        None => None,
    };
    // The blender's own admission, run at the same point and for the same
    // reason as the combiner's: before any pixel is produced, so a mode
    // this executor cannot evaluate exactly refuses with an untouched
    // target rather than a half-drawn one. Copy cycle passes through with
    // `cycle_count() == 0`, which is the RDP's own blender bypass.
    let blend_state = blend_registers.mode_state(other_mode);
    require_blendable_mode(blend_state)?;
    // The other three post-combiner stages, admitted at the same point and
    // for the same reason: a mode this executor cannot evaluate exactly
    // refuses with an untouched target rather than a half-drawn one.
    let stages = TexrectFragmentStages::try_new(other_mode, blend_registers.blend_color)?;

    let key = candidate.key();
    let format = key.format();
    let texture_filter = match evaluation {
        TexrectCombinerEvaluation::BlitsTheTexel => TextureFilter::Point,
        TexrectCombinerEvaluation::OneCycle | TexrectCombinerEvaluation::TwoCycle => {
            other_mode.texture_filter()
        }
    };
    let rank_one = (texture_filter == TextureFilter::Point)
        .then(|| {
            RankOneCi4Rgba16::admit(shading.combine(), other_mode, format, lut_mode, tile, draw)
        })
        .flatten();
    let mut prepared_sampler = rank_one
        .is_none()
        .then(|| {
            PreparedTextureSampler::try_new(
                tile.descriptor(),
                tile.size(),
                lut_mode,
                texture_filter,
            )
        })
        .transpose()
        .map_err(|source| TexrectExecutionError::Sample {
            column: 0,
            row: 0,
            source,
        })?
        .map(|sampler| sampler.bind(tmem));
    let extent = key.extent();
    let rectangle = TargetRectangle::try_new(draw.left(), draw.top(), draw.width(), draw.height())?;
    // **Clipped, not refused.** Pinned RT64 intersects the scissor and draw
    // rectangles and keeps a non-empty intersection rather than rejecting
    // an overhanging primitive
    // (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/hle/rt64_rdp.cpp:1214-1223`).
    // A rectangle that
    // overhangs the framebuffer is routine content, and the previous
    // `OutsideTarget` refusal here dropped all of it. See
    // [`clip_texrect_extent`] for the precedence between the scissor and
    // the target extent, and for what still refuses.
    let clipped = clip_texrect_extent(
        draw,
        scissor,
        extent.width(),
        extent.height(),
        key,
        rectangle,
    )?;
    // The rectangle actually written, after clipping -- narrower than
    // `rectangle` whenever the scissor or the target bit into it. This is
    // what the journal is told about, because it is what the pixel loop
    // below touches; claiming the unclipped rectangle would declare rows
    // this call never writes.
    let drawn = TargetRectangle::try_new(
        draw.left() + clipped.first_column,
        draw.top() + clipped.first_row,
        clipped.column_limit - clipped.first_column,
        clipped.row_limit - clipped.first_row,
    )?;
    // Planned, not just bounds-checked: `plan_rows` is the target's own
    // row-planning authority and rejects the same out-of-bounds cases with
    // its own named error. Calling it keeps this executor and the fill
    // executor on one row planner. Handed the CLIPPED rectangle, which is
    // the one whose rows are written.
    let _plan = candidate.plan_rows(drawn)?;

    let bytes_per_pixel = format.bytes_per_pixel() as usize;
    let full_len = (extent.pixels() as usize)
        .checked_mul(bytes_per_pixel)
        .ok_or(TargetError::PixelBufferLengthOverflow {
            pixels: extent.pixels() as usize,
            bytes_per_pixel: format.bytes_per_pixel(),
        })?;
    if bytes.len() != full_len {
        return Err(TargetError::CompletedByteLengthMismatch {
            key,
            generation: candidate.generation(),
            expected: full_len,
            actual: bytes.len(),
        }
        .into());
    }
    // **First-row parity comes from the tile's own T origin, not a
    // constant.** [`crate::TmemFirstRowParity`] is explicit caller input by
    // design -- the reader never infers it -- so this executor owes the
    // reader the same parity the *writer* used, or the two disagree about
    // which TMEM rows carry the XOR4 bank exchange.
    //
    // The writer's rule is `tmem/types.rs`'s `project_tmem_transfer_word`,
    // `TmemLoadKind::Tile` arm: `odd_row_exchange = (bounds.low_t().integer()
    // + row) & 1`, applied to the physical lanes by
    // `tmem/execute/load_tile.rs`'s `map_physical_lanes`. The reader's rule
    // is `tmem/read.rs`'s `odd_row_exchange`: `first_is_odd ^ (row & 1)`.
    // The two agree exactly when `first_is_odd == low_t.integer() & 1`, and
    // this line is that equality.
    //
    // A frozen `Even` was previously passed here. That is correct only for
    // a tile whose T origin is even -- and it is invisible for `LoadBlock`,
    // whose `transfer_shape` `Block` arm always reports `row_count = 1` so
    // its own `odd_row_exchange` never fires on the write side. Measured on
    // the real ROM, WM2000's sprite-strip tile has `low_t.integer() == 47`,
    // an ODD origin, so the frozen constant inverted the exchange for every
    // row and each rectangle row's last pixel read a byte the load never
    // wrote (`tmem::read::tests`'s two `wm2000_texrect_*` tests pin exactly
    // that, including the production abort's own byte `0x04c`).
    let first_row_parity = if tile.size().low_t().integer() & 1 == 1 {
        TmemFirstRowParity::Odd
    } else {
        TmemFirstRowParity::Even
    };

    // The loop walks the CLIPPED offsets, but `t_at`/`s_at` are still
    // indexed by the offset from the rectangle's own unclipped origin --
    // that is the whole reason `clip_texrect_extent` returns offsets rather
    // than a narrowed `TexrectDraw`. `rdp_tex_rect` loads the S/T origin and
    // steps once from the unclipped command (`rasterizer.c:2657-2677`) and
    // the edgewalker's clip touches only `majorx`/`minorx` (`:2349-2363`),
    // so a clipped rectangle samples the same texel at a given screen pixel
    // that an unclipped one would. Rebasing the ramp onto the clipped left
    // edge would slide the texture sideways by the clipped amount.
    for row in clipped.rows() {
        for column in clipped.columns() {
            let (s, t) = draw.coordinates_at(column, row);
            // The one texel fetch. `sample_point` is `tmem/sample.rs`'s
            // existing sampler, monomorphized over the pending post-image
            // rather than over durable state -- the same shift/mask/mirror/
            // clamp addressing, the same validity-gated physical read, the
            // same format and TLUT decode. There is no second sampler.
            let request = PointSampleRequest::new(
                PointSampleCoordinates::new(
                    TextureCoordinateS10_5::from_raw(s),
                    TextureCoordinateS10_5::from_raw(t),
                ),
                first_row_parity,
            );
            let sampled_rgba = match rank_one {
                Some(specialized) => specialized.sample(tmem, s, t).map_err(Into::into),
                None => prepared_sampler
                    .as_mut()
                    .expect("the generic texrect path prepared one sampler")
                    .sample(request),
            }
            .map_err(|source| TexrectExecutionError::Sample {
                column,
                row,
                source,
            })?;
            let rgba = match base_inputs {
                // Copy cycle: the sampled texel's own RGBA8888, unchanged.
                None => sampled_rgba,
                // One cycle: `(A-B)*C+D` for color and alpha independently,
                // then RT64's final `wrapClamp` -- all inside
                // `run_one_cycle`, which is the triangle pipeline's own
                // evaluator, not a second copy of the arithmetic. The
                // texel enters as `tex_val0` normalized by `/ 255.0`,
                // matching `RasterPS.hlsl`'s already-normalized sample, and
                // the `[0.0, 1.0]` result is returned to bytes by
                // `* 255.0` then round-half-away-from-zero (`f32::round`),
                // the same quantization `production.rs`'s existing
                // WGSL-agreement test uses. Rounding happens strictly
                // after `wrap_clamp`: clamping a rounded value and
                // rounding a clamped one differ at the boundary, and RT64
                // clamps in float before any quantization.
                Some(base) => combine_one_texel(shading.combine(), base, sampled_rgba, evaluation),
            };
            let pixel_x = draw.left() + column;
            let pixel_y = draw.top() + row;
            let offset =
                (pixel_y as usize * extent.width() as usize + pixel_x as usize) * bytes_per_pixel;
            // **The blender, the stage this executor previously declared it
            // did not run**, composed with the write in one named function
            // so a mutation that drops it is reachable from a unit test --
            // the same reason `combine_one_texel` is a function rather than
            // an inline block (see its own doc: while that arithmetic was
            // inline, replacing `round()` with a truncating cast left the
            // entire suite green).
            blend_and_write_pixel(
                format,
                &mut bytes[offset..offset + bytes_per_pixel],
                rgba,
                blend_state,
                stages,
                column,
                row,
            )?;
        }
    }

    let device_bytes = DeviceColorBytes::new_for_fill(key, candidate.generation(), format, bytes)?;
    // The claimed rectangle is the union of what this texrect covered and
    // what `already_initialized` says the incoming `resident_bytes` already
    // proved -- not the texrect's own rectangle alone.
    //
    // `admit_completed_initialization` reads this rectangle to decide
    // whether a brand-new target is fully initialized, and it is right to:
    // every byte of a new target must come from a proven write. In the
    // composed fill+texrect shape those bytes DO all come from proven
    // writes, just from two of them -- the fill initialized the whole
    // extent and this executor patched a sub-rectangle of that same buffer.
    // Reporting only the sub-rectangle would understate what the buffer
    // proves and be rejected; reporting the full extent unconditionally
    // would overstate it for a texrect with no fill under it. The union is
    // the honest answer, and the caller supplies the other half rather than
    // this executor assuming one.
    //
    // `drawn`, not `rectangle`: the claim must be what this call actually
    // wrote. A clipped texrect covers less than its command asked for, and
    // claiming the unclipped rect would assert proof over pixels the
    // scissor kept it away from.
    let claimed = union_rectangle(drawn, already_initialized);
    let completed = CompletedColorTargetWrite::new_for_fill(
        key,
        candidate.generation(),
        key.range(),
        claimed,
        device_bytes,
    );
    if let Some(timing) = timing {
        timing.finish(u64::from(drawn.width()) * u64::from(drawn.height()));
    }
    Ok(completed)
}

/// The smallest rectangle containing both, or `covered` alone when there is
/// no prior proven region.
fn union_rectangle(
    covered: TargetRectangle,
    already_initialized: Option<TargetRectangle>,
) -> TargetRectangle {
    let Some(prior) = already_initialized else {
        return covered;
    };
    let x = covered.x().min(prior.x());
    let y = covered.y().min(prior.y());
    let right = (covered.x() + covered.width()).max(prior.x() + prior.width());
    let bottom = (covered.y() + covered.height()).max(prior.y() + prior.height());
    TargetRectangle::try_new(x, y, right - x, bottom - y)
        .expect("a union of two in-bounds rectangles is in bounds")
}

/// Default-off timing for successful CPU texrect execution, keyed by every
/// state field needed to choose a closed exact specialization.
///
/// There is deliberately no task identifier here: the target executor owns
/// no production scheduling context. Rows are cumulative rankings. Joining a
/// row to one drawn-frame tail requires the production caller to supply its
/// task/member identity at the `execute_scheduled_texrect` seam rather than
/// introducing an ambient thread-local identity in this reusable executor.
mod texrect_timing_census {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const REPORT_EVERY_CALLS: u64 = 5_000;

    const fn target_format_code(format: ColorTargetFormat) -> u8 {
        match format {
            ColorTargetFormat::Rgba16 => 16,
            ColorTargetFormat::Rgba32 => 32,
        }
    }

    /// Public RDP `G_TT_*` encodings: disabled=0, RGBA16=2, IA16=3.
    const fn lut_mode_code(mode: TextureLutMode) -> u8 {
        match mode {
            TextureLutMode::Disabled => 0,
            TextureLutMode::Rgba16 => 2,
            TextureLutMode::Ia16 => 3,
        }
    }

    /// Public RDP image-format encodings (`G_IM_FMT_*`).
    const fn tile_format_code(format: ImageFormat) -> u8 {
        match format {
            ImageFormat::Rgba => 0,
            ImageFormat::Yuv => 1,
            ImageFormat::ColorIndex => 2,
            ImageFormat::IntensityAlpha => 3,
            ImageFormat::Intensity => 4,
        }
    }

    const fn tile_size_bits(size: PixelSize) -> u8 {
        match size {
            PixelSize::Bits4 => 4,
            PixelSize::Bits8 => 8,
            PixelSize::Bits16 => 16,
            PixelSize::Bits32 => 32,
        }
    }

    const fn address_mode_bits(mode: TileAddressMode) -> u8 {
        let mirror = if mode.mirror() { 1 } else { 0 };
        let clamp = if mode.clamp() { 1 } else { 0 };
        mirror | (clamp << 1)
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Key {
        combine_low: u32,
        combine_high: u32,
        other_mode_high: u32,
        other_mode_low: u32,
        target_format: u8,
        lut_mode: u8,
        tile_format: u8,
        tile_size: u8,
        tile_line_words: u16,
        tile_tmem_word: u16,
        tile_palette: u8,
        tile_s_mode: u8,
        tile_mask_s: u8,
        tile_shift_s: u8,
        tile_t_mode: u8,
        tile_mask_t: u8,
        tile_shift_t: u8,
        tile_low_s: u16,
        tile_low_t: u16,
        tile_high_s: u16,
        tile_high_t: u16,
        tile_low_t_parity: u8,
        flipped_axes: bool,
    }

    impl Key {
        fn new(
            combine: CombineParams,
            other_mode: OtherMode,
            target_format: ColorTargetFormat,
            lut_mode: TextureLutMode,
            tile: TexrectTileBinding,
            draw: TexrectDraw,
        ) -> Self {
            let descriptor = tile.descriptor();
            let size = tile.size();
            Self {
                combine_low: combine.low(),
                combine_high: combine.high(),
                other_mode_high: other_mode.high(),
                other_mode_low: other_mode.low(),
                target_format: target_format_code(target_format),
                lut_mode: lut_mode_code(lut_mode),
                tile_format: tile_format_code(descriptor.format()),
                tile_size: tile_size_bits(descriptor.size()),
                tile_line_words: descriptor.line_words(),
                tile_tmem_word: descriptor.tmem().get(),
                tile_palette: descriptor.palette(),
                tile_s_mode: address_mode_bits(descriptor.s_mode()),
                tile_mask_s: descriptor.mask_s(),
                tile_shift_s: descriptor.shift_s(),
                tile_t_mode: address_mode_bits(descriptor.t_mode()),
                tile_mask_t: descriptor.mask_t(),
                tile_shift_t: descriptor.shift_t(),
                tile_low_s: size.low_s().raw(),
                tile_low_t: size.low_t().raw(),
                tile_high_s: size.high_s().raw(),
                tile_high_t: size.high_t().raw(),
                tile_low_t_parity: (size.low_t().integer() & 1) as u8,
                flipped_axes: draw.flipped_axes,
            }
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct Stats {
        calls: u64,
        requested_pixels: u64,
        clipped_pixels: u64,
        elapsed_ns: u128,
        max_call_ns: u128,
    }

    impl Stats {
        fn note(&mut self, requested_pixels: u64, clipped_pixels: u64, elapsed: Duration) {
            self.calls += 1;
            self.requested_pixels += requested_pixels;
            self.clipped_pixels += clipped_pixels;
            self.elapsed_ns += elapsed.as_nanos();
            self.max_call_ns = self.max_call_ns.max(elapsed.as_nanos());
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct Census {
        calls: u64,
        keys: BTreeMap<Key, Stats>,
    }

    static CENSUS: Mutex<Option<Census>> = Mutex::new(None);
    static LAST_EMITTED_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct ThreadExitReporter;

    impl Drop for ThreadExitReporter {
        fn drop(&mut self) {
            flush("final");
        }
    }

    thread_local! {
        static THREAD_EXIT_REPORTER: ThreadExitReporter = const { ThreadExitReporter };
    }

    /// Takes `Option<&str>` since task 2.2b (was `Option<&OsStr>`): the
    /// crate's single permitted read site returns `Option<String>`. "Set to
    /// anything but `0`" is the same predicate either way.
    fn env_value_enables(value: Option<&str>) -> bool {
        value.is_some_and(|value| value != "0")
    }

    fn enabled() -> bool {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let enabled = *ENABLED.get_or_init(|| {
            env_value_enables(crate::diag_env::diag_env("FN64_TEXRECT_TIMING_CENSUS").as_deref())
        });
        if enabled {
            THREAD_EXIT_REPORTER.with(|_| {});
        }
        enabled
    }

    pub(super) struct StartedDraw {
        key: Key,
        requested_pixels: u64,
        started: Instant,
    }

    impl StartedDraw {
        pub(super) fn if_enabled(
            combine: CombineParams,
            other_mode: OtherMode,
            target_format: ColorTargetFormat,
            lut_mode: TextureLutMode,
            tile: TexrectTileBinding,
            draw: TexrectDraw,
            requested_pixels: u64,
        ) -> Option<Self> {
            enabled().then(|| Self {
                key: Key::new(combine, other_mode, target_format, lut_mode, tile, draw),
                requested_pixels,
                started: Instant::now(),
            })
        }

        pub(super) fn finish(self, clipped_pixels: u64) {
            note(
                self.key,
                self.requested_pixels,
                clipped_pixels,
                self.started.elapsed(),
            );
        }
    }

    fn note(key: Key, requested_pixels: u64, clipped_pixels: u64, elapsed: Duration) {
        let snapshot = {
            let mut guard = CENSUS.lock().expect("texrect timing census mutex poisoned");
            let census = guard.get_or_insert_with(Census::default);
            census.calls += 1;
            census
                .keys
                .entry(key)
                .or_default()
                .note(requested_pixels, clipped_pixels, elapsed);
            (census.calls % REPORT_EVERY_CALLS == 0).then(|| census.clone())
        };
        if let Some(snapshot) = snapshot {
            emit_snapshot("periodic", &snapshot);
        }
    }

    pub(super) fn flush(tag: &str) {
        let snapshot = CENSUS
            .lock()
            .expect("texrect timing census mutex poisoned")
            .as_ref()
            .cloned();
        if let Some(snapshot) = snapshot {
            emit_snapshot(tag, &snapshot);
        }
    }

    fn emit_snapshot(tag: &str, census: &Census) {
        use std::sync::atomic::Ordering;

        let prior = LAST_EMITTED_CALLS.fetch_max(census.calls, Ordering::Relaxed);
        if census.calls <= prior {
            return;
        }
        for row in format_report(tag, census) {
            eprintln!("{row}");
        }
    }

    fn format_report(tag: &str, census: &Census) -> Vec<String> {
        let mut ranked = census.keys.iter().collect::<Vec<_>>();
        ranked.sort_by(|(key_a, stats_a), (key_b, stats_b)| {
            stats_b
                .elapsed_ns
                .cmp(&stats_a.elapsed_ns)
                .then_with(|| key_a.cmp(key_b))
        });
        let total_requested = census
            .keys
            .values()
            .map(|stats| stats.requested_pixels)
            .sum::<u64>();
        let total_clipped = census
            .keys
            .values()
            .map(|stats| stats.clipped_pixels)
            .sum::<u64>();
        let total_ns = census
            .keys
            .values()
            .map(|stats| stats.elapsed_ns)
            .sum::<u128>();
        let mut rows = vec![format!(
            "[fn64-texrect-census] snapshot={tag} calls={} keys={} requested_pixels={} clipped_pixels={} elapsed_ns={} elapsed_ms={:.3}",
            census.calls,
            census.keys.len(),
            total_requested,
            total_clipped,
            total_ns,
            total_ns as f64 / 1_000_000.0,
        )];
        for (rank, (key, stats)) in ranked.into_iter().take(16).enumerate() {
            rows.push(format_row(tag, rank + 1, key, stats));
        }
        rows
    }

    fn format_row(tag: &str, rank: usize, key: &Key, stats: &Stats) -> String {
        let ns_per_clipped_pixel = if stats.clipped_pixels == 0 {
            0.0
        } else {
            stats.elapsed_ns as f64 / stats.clipped_pixels as f64
        };
        format!(
            "[fn64-texrect-census] snapshot={tag} rank={rank} combine={:#010x}/{:#010x} other={:#010x}/{:#010x} target_fmt={} lut={} tile_fmt={} tile_size={} line_words={} tmem_word={} palette={} s_mode={} mask_s={} shift_s={} t_mode={} mask_t={} shift_t={} low_s={} low_t={} high_s={} high_t={} low_t_parity={} flipped={} calls={} requested_pixels={} clipped_pixels={} elapsed_ns={} max_call_ns={} elapsed_ms={:.3} max_call_ms={:.3} ns_per_clipped_pixel={:.2}",
            key.combine_low,
            key.combine_high,
            key.other_mode_high,
            key.other_mode_low,
            key.target_format,
            key.lut_mode,
            key.tile_format,
            key.tile_size,
            key.tile_line_words,
            key.tile_tmem_word,
            key.tile_palette,
            key.tile_s_mode,
            key.tile_mask_s,
            key.tile_shift_s,
            key.tile_t_mode,
            key.tile_mask_t,
            key.tile_shift_t,
            key.tile_low_s,
            key.tile_low_t,
            key.tile_high_s,
            key.tile_high_t,
            key.tile_low_t_parity,
            u8::from(key.flipped_axes),
            stats.calls,
            stats.requested_pixels,
            stats.clipped_pixels,
            stats.elapsed_ns,
            stats.max_call_ns,
            stats.elapsed_ns as f64 / 1_000_000.0,
            stats.max_call_ns as f64 / 1_000_000.0,
            ns_per_clipped_pixel,
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        use fn64_render::{
            NeutralImageFormat, NeutralPixelSize, NeutralTileAddressMode, NeutralTileDescriptor,
            NeutralTileSize,
        };

        #[derive(Clone, Copy)]
        struct Inputs {
            combine: CombineParams,
            other_mode: OtherMode,
            target_format: ColorTargetFormat,
            lut_mode: TextureLutMode,
            descriptor: NeutralTileDescriptor,
            size: NeutralTileSize,
            draw: TexrectDraw,
        }

        impl Inputs {
            fn key(self) -> Key {
                Key::new(
                    self.combine,
                    self.other_mode,
                    self.target_format,
                    self.lut_mode,
                    TexrectTileBinding::try_from_neutral(self.descriptor, self.size).unwrap(),
                    self.draw,
                )
            }
        }

        fn base_inputs() -> Inputs {
            Inputs {
                combine: CombineParams::from_wire(1, 2),
                other_mode: OtherMode::from_wire(3, 4),
                target_format: ColorTargetFormat::Rgba16,
                lut_mode: TextureLutMode::Disabled,
                descriptor: NeutralTileDescriptor {
                    format: NeutralImageFormat::Rgba,
                    size: NeutralPixelSize::Bits4,
                    line_words: 4,
                    tmem_word_address: 7,
                    palette: 3,
                    s_mode: NeutralTileAddressMode::default(),
                    mask_s: 2,
                    shift_s: 1,
                    t_mode: NeutralTileAddressMode::default(),
                    mask_t: 3,
                    shift_t: 2,
                },
                size: NeutralTileSize {
                    low_s: 0,
                    low_t: 0,
                    high_s: 60,
                    high_t: 56,
                },
                draw: TexrectDraw {
                    left: 0,
                    top: 0,
                    right: 8,
                    bottom: 8,
                    s_start: 0,
                    t_start: 0,
                    s_end: 256,
                    t_end: 256,
                    flipped_axes: false,
                },
            }
        }

        fn base_key() -> Key {
            base_inputs().key()
        }

        #[test]
        fn default_off_requires_the_environment_variable_to_exist() {
            assert!(!env_value_enables(None));
            assert!(!env_value_enables(Some("0")));
            assert!(env_value_enables(Some("")));
            assert!(env_value_enables(Some("1")));
        }

        #[test]
        fn exact_key_denominator_contains_every_requested_field() {
            let base = base_inputs();
            let mut variants = Vec::new();

            let mut input = base;
            input.combine = CombineParams::from_wire(11, 2);
            variants.push(input.key());
            let mut input = base;
            input.combine = CombineParams::from_wire(1, 12);
            variants.push(input.key());
            let mut input = base;
            input.other_mode = OtherMode::from_wire(13, 4);
            variants.push(input.key());
            let mut input = base;
            input.other_mode = OtherMode::from_wire(3, 14);
            variants.push(input.key());
            let mut input = base;
            input.target_format = ColorTargetFormat::Rgba32;
            variants.push(input.key());
            let mut input = base;
            input.lut_mode = TextureLutMode::Rgba16;
            variants.push(input.key());

            let mut input = base;
            input.descriptor.format = NeutralImageFormat::ColorIndex;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.size = NeutralPixelSize::Bits16;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.line_words = 5;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.tmem_word_address = 8;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.palette = 4;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.s_mode.mirror = true;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.s_mode.clamp = true;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.mask_s = 4;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.shift_s = 4;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.t_mode.mirror = true;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.t_mode.clamp = true;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.mask_t = 5;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.shift_t = 5;
            variants.push(input.key());

            let mut input = base;
            input.size.low_s = 4;
            variants.push(input.key());
            let mut input = base;
            input.size.low_t = 4;
            let odd_low_t = input.key();
            assert_eq!(odd_low_t.tile_low_t_parity, 1);
            variants.push(odd_low_t);
            let mut input = base;
            input.size.high_s = 64;
            variants.push(input.key());
            let mut input = base;
            input.size.high_t = 64;
            variants.push(input.key());
            let mut input = base;
            input.draw = input.draw.with_flipped_axes();
            variants.push(input.key());

            let key = base.key();
            assert_eq!(key.tile_line_words, 4);
            assert_eq!(key.tile_tmem_word, 7);
            assert_eq!(key.tile_palette, 3);
            assert_eq!(key.tile_low_s, 0);
            assert_eq!(key.tile_low_t, 0);
            assert_eq!(key.tile_high_s, 60);
            assert_eq!(key.tile_high_t, 56);
            assert_eq!(key.tile_low_t_parity, 0);
            assert!(!key.flipped_axes);

            let mut denominator = std::collections::BTreeSet::from([key]);
            denominator.extend(variants);
            assert_eq!(denominator.len(), 25);
        }

        #[test]
        fn aggregation_tracks_calls_both_pixel_denominators_total_and_maximum() {
            let mut stats = Stats::default();
            stats.note(80, 60, Duration::from_micros(10));
            stats.note(120, 40, Duration::from_micros(40));
            assert_eq!(
                stats,
                Stats {
                    calls: 2,
                    requested_pixels: 200,
                    clipped_pixels: 100,
                    elapsed_ns: 50_000,
                    max_call_ns: 40_000,
                }
            );
        }

        #[test]
        fn final_snapshot_reports_a_partial_interval_with_a_checkable_denominator() {
            let mut census = Census {
                calls: 1,
                keys: BTreeMap::new(),
            };
            census.keys.insert(
                base_key(),
                Stats {
                    calls: 2,
                    requested_pixels: 200,
                    clipped_pixels: 100,
                    elapsed_ns: 50_000,
                    max_call_ns: 40_000,
                },
            );
            let rows = format_report("final", &census);
            assert_eq!(rows.len(), 2);
            assert_eq!(
                rows[0],
                "[fn64-texrect-census] snapshot=final calls=1 keys=1 requested_pixels=200 clipped_pixels=100 elapsed_ns=50000 elapsed_ms=0.050"
            );
            for field in [
                "snapshot=final",
                "rank=1",
                "combine=0x00000001/0x00000002",
                "other=0x00000003/0x00000004",
                "target_fmt=16",
                "lut=0",
                "tile_fmt=0",
                "tile_size=4",
                "line_words=4",
                "tmem_word=7",
                "palette=3",
                "s_mode=0",
                "mask_s=2",
                "shift_s=1",
                "t_mode=0",
                "mask_t=3",
                "shift_t=2",
                "low_s=0",
                "low_t=0",
                "high_s=60",
                "high_t=56",
                "low_t_parity=0",
                "flipped=0",
                "calls=2",
                "requested_pixels=200",
                "clipped_pixels=100",
                "elapsed_ns=50000",
                "max_call_ns=40000",
                "elapsed_ms=0.050",
                "max_call_ms=0.040",
                "ns_per_clipped_pixel=500.00",
            ] {
                assert!(rows[1].split_ascii_whitespace().any(|token| token == field));
            }
        }
    }
}
