//! Task 6.2 Step 1: count render-join overlaps against the next task's inputs.
//!
//! **Instrumentation only.** Nothing here changes a join decision; it observes
//! the decision `osSpTaskStartGo_recomp` has already made and classifies it.
//!
//! # The question
//!
//! `osSpTaskStartGo_recomp` joins the in-flight raw-DPC batch whenever the
//! incoming SP task is a later graphics task (`LaterGraphics`) or a live
//! rspboot is waiting on the shared DMEM command buffer (`DmemDependency`).
//! The `LaterGraphics` half exists because the next task *may* read RDRAM the
//! batch has not written yet. "May" is the load-bearing word: if the next
//! task's declared inputs and the batch's declared writes/reads share no
//! byte, the join is buying nothing and could be skipped (Step 2).
//!
//! This module measures how often that is actually true, so Step 2 is
//! dispatched on a number rather than on the hypothesis.
//!
//! # The comparison rule
//!
//! Both sides are reduced to half-open byte ranges `[start, end)` in the
//! **RDP 24-bit physical address space**, and two ranges OVERLAP when
//! `a.start < b.end && b.start < a.end` -- i.e. they share at least one byte.
//! Adjacent ranges (`a.end == b.start`) are DISJOINT: the RDP's last written
//! byte is at `end - 1`. An empty range (`start >= end`) overlaps nothing,
//! including itself.
//!
//! A join is classified OVERLAP if **any** next-task input range shares a byte
//! with **any** in-flight batch range, and DISJOINT only when every pair is
//! disjoint. A join whose either side yields no range at all is counted
//! separately as `indeterminate` and is NEVER counted as disjoint -- an
//! unknown range is not a proven-safe range, and Step 2 must not be justified
//! by a range we failed to decode.
//!
//! # Joins are counted per cause
//!
//! Only `LaterGraphics` asks an RDRAM question. A `DmemDependency` join is
//! about the shared DMEM command buffer, so an RDRAM-disjoint verdict on one
//! is **not** evidence that it is skippable, and pooling the causes would hand
//! Step 2 a total it must not act on. The summary therefore prints one line
//! per observed cause, each carrying an explicit `rdram_only=` flag, alongside
//! the pooled total.
//!
//! ## Measured on the WM2000 lane: the RDRAM bucket is empty
//!
//! Splitting by cause changed the conclusion. Over 3,000 pumps every one of
//! the 1,144 joins was `DmemDependency` and **none** was `LaterGraphics`: the
//! joining task is `task_type == M_AUDTASK` (2) on every occurrence, so
//! `is_gfx` -- and therefore `later_graphics` -- is false throughout. The
//! population is an audio task arriving while a render batch is in flight,
//! whose rspboot waits on the shared DMEM command buffer.
//!
//! So the 100%-disjoint RDRAM verdict, while correctly computed, describes
//! joins whose dependency is **not** the one these ranges model. Step 2 must
//! not read it as "1,144 skippable joins". Before pooling the causes this was
//! invisible, which is exactly why the breakdown belongs in Step 1.
//!
//! ## Masking
//!
//! Guest pointers in an `OSTask` header are KSEG0/KSEG1 virtual addresses, so
//! each is masked with `& 0x1fff_ffff` (the same mask
//! `osSpTaskLoad_recomp` applies to `ucode_boot` in `lifecycle.rs`) and then
//! bounded to installed RDRAM. RDP image addresses are already physical: this
//! module reproduces the production decoder's own masks exactly rather than
//! inventing one --
//! `SetColorImage` uses `w1 & 0x00ff_ffff`
//! (`fn64-render-wgpu/src/raw_dpc/mod.rs:1256`) and `SetTextureImage` uses
//! `w1 & 0x03ff_ffff` (`fn64-render-wgpu/src/tmem/wire.rs:152`). Those two
//! masks genuinely differ in hardware and are deliberately not unified.
//!
//! Because both sides land in the same 24-bit physical space after masking, a
//! KSEG0 pointer and a KSEG1 pointer to the same RDRAM byte compare equal --
//! which is the point: an alias must not read as disjoint.
//!
//! # Cost when off
//!
//! Every entry point begins with `enabled()`, a `OnceLock<bool>` read: one
//! cached branch. No decode, no allocation, no clock read happens with
//! `FN64_RENDER_JOIN_CENSUS` unset.

use std::sync::{
    atomic::{AtomicU64, Ordering::Relaxed},
    OnceLock,
};

/// KSEG0/KSEG1 -> physical, as `osSpTaskLoad_recomp` masks `ucode_boot`.
const GUEST_POINTER_MASK: u32 = 0x1fff_ffff;

/// `SetColorImage`'s wire address mask, mirroring
/// `fn64-render-wgpu/src/raw_dpc/mod.rs:1256`.
const SET_COLOR_IMAGE_ADDRESS_MASK: u32 = 0x00ff_ffff;

/// `SetTextureImage`'s wire address mask, mirroring
/// `fn64-render-wgpu/src/tmem/wire.rs:152`. Wider than the color-image mask
/// on real hardware; see the module docs.
const SET_TEXTURE_IMAGE_ADDRESS_MASK: u32 = 0x03ff_ffff;

/// The RDP's hard 24-bit physical address ceiling, mirroring
/// `fn64_render_ir::RDP_PHYSICAL_ADDRESS_BYTES`
/// (`crates/fn64-render-ir/src/address.rs:6`). The runtime's own allocation is
/// much larger than RDRAM -- it backs more than the console's memory -- so the
/// height-less color-image extent below must be bounded by the RDP's
/// addressable space clamped to installed RDRAM, never by the raw allocation
/// length. Measured on the WM2000 lane: `runtime_rdram_len` reads
/// `0x2900_0000`, 82x the console's 8 MB, and using it directly inflated every
/// render-target extent by the same factor.
const RDP_PHYSICAL_ADDRESS_BYTES: u32 = 0x0100_0000;

const RDP_SET_COLOR_IMAGE: u8 = 0x3f;
/// `SetMaskImage`/`SetZImage` -- the depth buffer. Decoded with the same
/// 24-bit address mask as `SetColorImage` and with no alignment gate, per
/// `fn64-render-wgpu/src/raw_dpc/mod.rs:1274-1296`.
const RDP_SET_MASK_IMAGE: u8 = 0x3e;
const RDP_SET_TEXTURE_IMAGE: u8 = 0x3d;
const RDP_LOAD_BLOCK: u8 = 0x33;
const RDP_LOAD_TILE: u8 = 0x34;
/// `LoadTLUT` -- a palette read from the staged texture image,
/// `fn64-render-wgpu/src/tmem/wire.rs:366-446`.
const RDP_LOAD_TLUT: u8 = 0x30;

/// A half-open physical byte range `[start, end)`.
///
/// `end` is exclusive because that is what the RDP's own range type means
/// (`fn64_render_ir::PhysicalRange`): the last byte touched is `end - 1`. Two
/// buffers laid end to end therefore do not overlap, which is exactly the
/// adjacency case Step 2 depends on getting right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PhysRange {
    start: u32,
    end: u32,
}

impl PhysRange {
    /// A range from an already-physical start and a byte length.
    ///
    /// Returns `None` for a zero length or an end that overflows the 32-bit
    /// physical space -- both are "no range", never a silently clamped one.
    pub(crate) fn from_start_len(start: u32, len: u32) -> Option<Self> {
        let end = start.checked_add(len)?;
        (len > 0).then_some(Self { start, end })
    }

    /// A range from a **guest** (KSEG0/KSEG1) pointer and a byte length.
    pub(crate) fn from_guest_ptr_len(ptr: u32, len: u32) -> Option<Self> {
        Self::from_start_len(ptr & GUEST_POINTER_MASK, len)
    }

    /// Whether these two ranges share at least one byte.
    ///
    /// Half-open comparison: `a.end == b.start` is adjacency, not overlap.
    /// The `<` on both sides is the whole predicate -- widening either to
    /// `<=` would report touching-but-disjoint buffers as overlapping and
    /// silently erase the very population Step 2 is looking for.
    pub(crate) fn overlaps(self, other: Self) -> bool {
        if self.is_empty() || other.is_empty() {
            return false;
        }
        self.start < other.end && other.start < self.end
    }

    /// An empty range touches nothing, including itself.
    pub(crate) fn is_empty(self) -> bool {
        self.start >= self.end
    }

    pub(crate) fn len(self) -> u64 {
        u64::from(self.end.saturating_sub(self.start))
    }
}

/// How one join classified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JoinClass {
    /// At least one next-task input byte is also an in-flight batch byte.
    Overlap,
    /// Every pair of ranges is disjoint. Only reachable when BOTH sides
    /// produced at least one range.
    Disjoint,
    /// One side produced no range at all, so nothing was proven. Never
    /// counted as disjoint.
    Indeterminate,
}

/// Classify one join from the two already-collected range sets.
///
/// Pure: the entire decision the census reports lives here, so a test can
/// drive it directly instead of parsing a printed number.
pub(crate) fn classify(next_task_inputs: &[PhysRange], batch_ranges: &[PhysRange]) -> JoinClass {
    let next: Vec<PhysRange> = next_task_inputs
        .iter()
        .copied()
        .filter(|range| !range.is_empty())
        .collect();
    let batch: Vec<PhysRange> = batch_ranges
        .iter()
        .copied()
        .filter(|range| !range.is_empty())
        .collect();
    if next.is_empty() || batch.is_empty() {
        return JoinClass::Indeterminate;
    }
    for a in &next {
        for b in &batch {
            if a.overlaps(*b) {
                return JoinClass::Overlap;
            }
        }
    }
    JoinClass::Disjoint
}

/// The next SP task's declared RDRAM inputs, from its `OSTask` header.
///
/// Source of truth for the field shape: `fn64_runtime::OsTaskHeader`
/// (`crates/fn64-runtime/src/rsp.rs:266`), the public libultra 64-byte
/// `OSTask_t`. Every pointer/size pair the task declares is an input the
/// microcode may read, so all of them are included:
///
/// - `ucode` / `ucode_size` -- the microcode text image.
/// - `ucode_data` / `ucode_data_size` -- the microcode's data segment.
/// - `data_ptr` / `data_size` -- the display/audio command list itself. This
///   is the one that matters for `LaterGraphics`: a gfx task's `data_ptr` is
///   the DL the prior batch may still be writing into.
/// - `dram_stack` / `dram_stack_size` -- read-write scratch.
/// - `output_buff` / `output_buff_size` -- written, and for a chained audio
///   task also read.
/// - `yield_data_ptr` / `yield_data_size` -- read back on resume.
///
/// `ucode_boot` is deliberately excluded: it is copied by the *host* at
/// admission (`osSpTaskLoad_recomp`), before this join, so it is not an
/// input the joined-against batch could race.
pub(crate) fn next_task_input_ranges(header: &fn64_runtime::OsTaskHeader) -> Vec<PhysRange> {
    [
        (header.ucode, header.ucode_size),
        (header.ucode_data, header.ucode_data_size),
        (header.data_ptr, header.data_size),
        (header.dram_stack, header.dram_stack_size),
        (header.output_buff, header.output_buff_size),
        (header.yield_data_ptr, header.yield_data_size),
    ]
    .into_iter()
    .filter_map(|(ptr, size)| PhysRange::from_guest_ptr_len(ptr, size))
    .collect()
}

/// Decode one raw-DPC command word stream into the physical ranges the batch
/// touches: the `SetColorImage` render-target extent and every
/// `LoadBlock`/`LoadTile` source read.
///
/// The stream is walked with `fn64_render::raw_rdp_command_width`
/// (`crates/fn64-render/src/rdp_completion.rs:421`), the same width table the
/// production decoder uses, so an unknown opcode stops the walk rather than
/// desynchronising it into garbage ranges.
///
/// Field arithmetic mirrors `fn64-render-wgpu`'s decoders, whose private
/// `pub(crate)` visibility is why it is restated rather than called:
///
/// - `SetColorImage` (`raw_dpc/mod.rs:1236-1273`): size `(w0 >> 19) & 3`,
///   width `(w0 & 0x0fff) + 1`, address `w1 & 0x00ff_ffff`.
/// - `SetTextureImage` (`tmem/wire.rs:141-158`): same size/width fields,
///   address `w1 & 0x03ff_ffff`.
/// - `LoadBlock` (`tmem/wire.rs:176-265`): S/T taken **raw** (no `>> 2`),
///   `texels = high_s - source_s + 1`, byte count padded to whole 64-bit
///   words (`div_ceil(8) * 8`) because the RDP copies whole words.
/// - `LoadTile` (`tmem/wire.rs:267-360`): S/T are fixed-point, so `>> 2`;
///   one padded row span per row, strided by the image width.
/// - `SetMaskImage`/`SetZImage` (`raw_dpc/mod.rs:1274-1296`): same 24-bit
///   address mask as the color image, no alignment gate. The depth buffer is
///   read AND written by the RDP, so it is a genuine race surface.
/// - `LoadTLUT` (`tmem/wire.rs:366-446`): `((w1 >> 14) & 0x03ff) + 1` entries
///   of two bytes each, read from the staged texture image base with no S/T
///   offset (the public macros require zero SL/TL).
///
/// A *missing* range is the unsafe direction -- it turns a true overlap into a
/// reported disjoint -- which is why the depth buffer and the palette load are
/// modelled here rather than left to the `_ => {}` arm.
///
/// # Deliberate conservatism
///
/// A `SetColorImage` extent needs a height, and the RDP never declares one --
/// height emerges from the primitives drawn into it. Rather than guess, the
/// target's extent is taken as the **whole remainder of installed RDRAM from
/// its base**, which can only ever turn a true disjoint into a reported
/// overlap. The count this produces is therefore a *lower bound* on the
/// disjoint population: it never claims a join is skippable when it is not.
fn batch_ranges_from_words(words: &[u32], rdram_bytes: u32) -> Vec<PhysRange> {
    // The RDP cannot address past 24 bits, and cannot address past installed
    // RDRAM either; the tighter of the two is the only honest ceiling for the
    // height-less color-image extent below.
    let ceiling = rdram_bytes.min(RDP_PHYSICAL_ADDRESS_BYTES);
    let mut ranges = Vec::new();
    // Staged `SetTextureImage`, which LoadBlock/LoadTile read relative to.
    let mut texture_image: Option<(u32, u32, u32)> = None; // (address, width, bytes_per_texel)
    let mut index = 0usize;
    while index + 1 < words.len() {
        let w0 = words[index];
        let w1 = words[index + 1];
        let wire_opcode = (w0 >> 24) as u8;
        let opcode = wire_opcode & 0x3f;
        let Some(width_bytes) = fn64_render::raw_rdp_command_width(wire_opcode) else {
            // An opcode this table does not know desynchronises the walk;
            // stopping yields fewer ranges, which the classifier reports as
            // indeterminate rather than as a false disjoint.
            break;
        };
        let advance = (width_bytes as usize) / std::mem::size_of::<u32>();
        if advance == 0 {
            break;
        }
        match opcode {
            RDP_SET_COLOR_IMAGE => {
                let address = w1 & SET_COLOR_IMAGE_ADDRESS_MASK;
                // No declared height: take the rest of RDRAM (see above).
                if let Some(len) = ceiling.checked_sub(address) {
                    if let Some(range) = PhysRange::from_start_len(address, len) {
                        ranges.push(range);
                    }
                }
            }
            RDP_SET_MASK_IMAGE => {
                // The Z buffer is read AND written by the RDP, so a next task
                // sharing bytes with it is a genuine race. Omitting it was the
                // unsafe direction: a missing batch range turns a true overlap
                // into a reported disjoint. Same 24-bit mask as the color
                // image, no alignment gate (raw_dpc/mod.rs:1291).
                let address = w1 & SET_COLOR_IMAGE_ADDRESS_MASK;
                // Like the color image, the depth buffer declares no height.
                if let Some(len) = ceiling.checked_sub(address) {
                    if let Some(range) = PhysRange::from_start_len(address, len) {
                        ranges.push(range);
                    }
                }
            }
            RDP_SET_TEXTURE_IMAGE => {
                let address = w1 & SET_TEXTURE_IMAGE_ADDRESS_MASK;
                let width = (w0 & 0x0fff) + 1;
                let bytes_per_texel = match (w0 >> 19) & 3 {
                    0 => 0, // four-bit: sub-byte, and the production decoder
                    // rejects direct 4-bit TMEM loads outright.
                    1 => 1,
                    2 => 2,
                    _ => 4,
                };
                texture_image = Some((address, width, bytes_per_texel));
            }
            RDP_LOAD_BLOCK => {
                if let Some((address, _width, bytes_per_texel)) = texture_image {
                    if bytes_per_texel > 0 {
                        let source_s = (w0 >> 12) & 0x0fff;
                        let source_t = w0 & 0x0fff;
                        let high_s = (w1 >> 12) & 0x0fff;
                        if high_s >= source_s {
                            let texels = high_s - source_s + 1;
                            let width = texture_image.map(|image| image.1).unwrap_or(1);
                            let start_texel = source_t
                                .checked_mul(width)
                                .and_then(|value| value.checked_add(source_s));
                            let block_bytes = texels
                                .checked_mul(bytes_per_texel)
                                .map(|bytes| bytes.div_ceil(8) * 8);
                            if let (Some(start_texel), Some(block_bytes)) =
                                (start_texel, block_bytes)
                            {
                                if let Some(first_byte) = start_texel.checked_mul(bytes_per_texel) {
                                    if let Some(start) = address.checked_add(first_byte) {
                                        if let Some(range) =
                                            PhysRange::from_start_len(start, block_bytes)
                                        {
                                            ranges.push(range);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            RDP_LOAD_TILE => {
                if let Some((address, image_width, bytes_per_texel)) = texture_image {
                    if bytes_per_texel > 0 {
                        // Fixed-point S/T: the integer texel is the raw field
                        // shifted right by two.
                        let low_s = ((w0 >> 12) & 0x0fff) >> 2;
                        let low_t = (w0 & 0x0fff) >> 2;
                        let high_s = ((w1 >> 12) & 0x0fff) >> 2;
                        let high_t = (w1 & 0x0fff) >> 2;
                        if high_s >= low_s && high_t >= low_t {
                            let row_texels = high_s - low_s + 1;
                            let padded_row_bytes = row_texels
                                .checked_mul(bytes_per_texel)
                                .map(|bytes| bytes.div_ceil(8) * 8);
                            if let Some(padded_row_bytes) = padded_row_bytes {
                                for row in low_t..=high_t {
                                    let first_texel = row
                                        .checked_mul(image_width)
                                        .and_then(|value| value.checked_add(low_s));
                                    let Some(first_texel) = first_texel else {
                                        break;
                                    };
                                    let Some(first_byte) = first_texel.checked_mul(bytes_per_texel)
                                    else {
                                        break;
                                    };
                                    let Some(start) = address.checked_add(first_byte) else {
                                        break;
                                    };
                                    if let Some(range) =
                                        PhysRange::from_start_len(start, padded_row_bytes)
                                    {
                                        ranges.push(range);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            RDP_LOAD_TLUT => {
                // A palette read from the staged texture image base. Entry
                // count is `((w1 >> 14) & 0x03ff) + 1` and each entry is two
                // bytes, because the public `gDPLoadTLUT*` macros always stage
                // a 16-bit source image (tmem/wire.rs:433-441). Unlike the
                // tile loads this reads from the image base with no S/T
                // offset, which the macros enforce by requiring zero SL/TL.
                if let Some((address, _width, _bytes_per_texel)) = texture_image {
                    let entries = ((w1 >> 14) & 0x03ff) + 1;
                    if let Some(bytes) = entries.checked_mul(2) {
                        if let Some(range) = PhysRange::from_start_len(address, bytes) {
                            ranges.push(range);
                        }
                    }
                }
            }
            _ => {}
        }
        index += advance;
    }
    ranges
}

/// Whether the census is armed. One cached bool; this is the only cost the
/// census imposes on a run that did not ask for it.
fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        crate::diag_env::diag_env("FN64_RENDER_JOIN_CENSUS").is_some_and(|value| value == "1")
    })
}

/// Whether the census is armed, for the one call site that must decide
/// *before* a join whether to retain the batch's command words.
///
/// Same cached bool as [`enabled`]; exposed because
/// `dispatch_raw_dpc_task_batch_via_session` has to pay (or not pay) the
/// retention cost at batch construction, long before [`note_join`] runs.
pub(crate) fn armed() -> bool {
    enabled()
}

/// The join causes counted separately.
///
/// Only `LaterGraphics` is an RDRAM question. A `DmemDependency` join exists
/// because a live rspboot is waiting on the shared DMEM command buffer, which
/// these RDRAM ranges say nothing about -- so an RDRAM-DISJOINT verdict on
/// such a join is NOT evidence that it is skippable, and pooling the two would
/// hand Step 2 a number it must not act on. `LaterGraphicsAndDmemDependency`
/// carries both dependencies at once and is likewise not skippable on RDRAM
/// grounds alone.
const CAUSE_COUNT: usize = 4;

const fn cause_index(cause: crate::RenderBatchJoinCause) -> usize {
    match cause {
        crate::RenderBatchJoinCause::ViVisibility => 0,
        crate::RenderBatchJoinCause::LaterGraphics => 1,
        crate::RenderBatchJoinCause::DmemDependency => 2,
        crate::RenderBatchJoinCause::LaterGraphicsAndDmemDependency => 3,
    }
}

const CAUSE_LABELS: [&str; CAUSE_COUNT] = [
    "vi_visibility",
    "later_graphics",
    "dmem_dependency",
    "later_graphics_and_dmem",
];

/// Every cause, in slot order, so the reporter can walk causes rather than
/// bare indices and `cause_index`/`cause_is_rdram_only` stay the single
/// source of truth for both.
const CAUSES: [crate::RenderBatchJoinCause; CAUSE_COUNT] = [
    crate::RenderBatchJoinCause::ViVisibility,
    crate::RenderBatchJoinCause::LaterGraphics,
    crate::RenderBatchJoinCause::DmemDependency,
    crate::RenderBatchJoinCause::LaterGraphicsAndDmemDependency,
];

/// Whether an RDRAM-disjoint verdict is even meaningful for this cause.
///
/// `LaterGraphics` alone is the only cause whose dependency is exactly "the
/// next task may read RDRAM this batch has not written". Step 2 may act on
/// that bucket; the others are reported for completeness and must not be
/// summed into a skippable total.
const fn cause_is_rdram_only(cause: crate::RenderBatchJoinCause) -> bool {
    matches!(cause, crate::RenderBatchJoinCause::LaterGraphics)
}

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU64 = AtomicU64::new(0);

static JOINS: [AtomicU64; CAUSE_COUNT] = [ZERO; CAUSE_COUNT];
static OVERLAPS: [AtomicU64; CAUSE_COUNT] = [ZERO; CAUSE_COUNT];
static DISJOINT: [AtomicU64; CAUSE_COUNT] = [ZERO; CAUSE_COUNT];
static INDETERMINATE: [AtomicU64; CAUSE_COUNT] = [ZERO; CAUSE_COUNT];
static OVERLAP_NEXT_BYTES: [AtomicU64; CAUSE_COUNT] = [ZERO; CAUSE_COUNT];
static OVERLAP_BATCH_BYTES: [AtomicU64; CAUSE_COUNT] = [ZERO; CAUSE_COUNT];
static DISJOINT_NEXT_BYTES: [AtomicU64; CAUSE_COUNT] = [ZERO; CAUSE_COUNT];
static DISJOINT_BATCH_BYTES: [AtomicU64; CAUSE_COUNT] = [ZERO; CAUSE_COUNT];

/// Observe one join that `osSpTaskStartGo_recomp` is about to perform.
///
/// Called immediately before `advance_async_lle_render_task`, while the batch
/// is still pending, so `words` is the in-flight command stream and `header`
/// is the task that is about to run after it. `cause` is the same value the
/// join itself is given, so the census cannot disagree with the join about why
/// it happened.
pub(crate) fn note_join(
    header: &fn64_runtime::OsTaskHeader,
    batch_words: &[u32],
    rdram_bytes: u32,
    cause: crate::RenderBatchJoinCause,
) {
    if !enabled() {
        return;
    }
    let next = next_task_input_ranges(header);
    let batch = batch_ranges_from_words(batch_words, rdram_bytes);
    let next_bytes: u64 = next.iter().map(|range| range.len()).sum();
    let batch_bytes: u64 = batch.iter().map(|range| range.len()).sum();
    let slot = cause_index(cause);
    JOINS[slot].fetch_add(1, Relaxed);
    match classify(&next, &batch) {
        JoinClass::Overlap => {
            OVERLAPS[slot].fetch_add(1, Relaxed);
            OVERLAP_NEXT_BYTES[slot].fetch_add(next_bytes, Relaxed);
            OVERLAP_BATCH_BYTES[slot].fetch_add(batch_bytes, Relaxed);
        }
        JoinClass::Disjoint => {
            DISJOINT[slot].fetch_add(1, Relaxed);
            DISJOINT_NEXT_BYTES[slot].fetch_add(next_bytes, Relaxed);
            DISJOINT_BATCH_BYTES[slot].fetch_add(batch_bytes, Relaxed);
        }
        JoinClass::Indeterminate => {
            INDETERMINATE[slot].fetch_add(1, Relaxed);
        }
    }
}

/// Emit the summary at process exit: one total line plus one line per observed
/// cause. A no-op when the census never armed, and when it armed but saw no
/// join (nothing to report is not a finding).
///
/// The per-cause lines are the point. A reader must be able to see how many of
/// the disjoint joins are `later_graphics` -- the only bucket Step 2 may act on
/// -- rather than a pooled total that silently includes DMEM dependencies.
pub fn report_render_join_census() {
    if !enabled() {
        return;
    }
    let total = |counters: &[AtomicU64; CAUSE_COUNT]| -> u64 {
        counters.iter().map(|counter| counter.load(Relaxed)).sum()
    };
    let joins = total(&JOINS);
    if joins == 0 {
        return;
    }
    eprintln!(
        "[render-join-census] joins={joins} overlap={} disjoint={} indeterminate={} \
         overlap_next_bytes={} overlap_batch_bytes={} \
         disjoint_next_bytes={} disjoint_batch_bytes={}",
        total(&OVERLAPS),
        total(&DISJOINT),
        total(&INDETERMINATE),
        total(&OVERLAP_NEXT_BYTES),
        total(&OVERLAP_BATCH_BYTES),
        total(&DISJOINT_NEXT_BYTES),
        total(&DISJOINT_BATCH_BYTES),
    );
    for cause in CAUSES {
        let slot = cause_index(cause);
        let joins = JOINS[slot].load(Relaxed);
        if joins == 0 {
            continue;
        }
        eprintln!(
            "[render-join-census] cause={} rdram_only={} joins={joins} overlap={} \
             disjoint={} indeterminate={} disjoint_next_bytes={} disjoint_batch_bytes={}",
            CAUSE_LABELS[slot],
            // Spelled per line so a reader of one line alone still knows
            // whether its disjoint count is actionable.
            cause_is_rdram_only(cause),
            OVERLAPS[slot].load(Relaxed),
            DISJOINT[slot].load(Relaxed),
            INDETERMINATE[slot].load(Relaxed),
            DISJOINT_NEXT_BYTES[slot].load(Relaxed),
            DISJOINT_BATCH_BYTES[slot].load(Relaxed),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The predicate's core: sharing one byte is an overlap.
    #[test]
    fn ranges_sharing_a_byte_overlap() {
        let a = PhysRange::from_start_len(0x1000, 0x100).expect("nonempty");
        let b = PhysRange::from_start_len(0x10ff, 0x100).expect("nonempty");
        assert!(a.overlaps(b));
        assert!(b.overlaps(a));
    }

    /// Fully separated ranges do not overlap.
    #[test]
    fn separated_ranges_are_disjoint() {
        let a = PhysRange::from_start_len(0x1000, 0x100).expect("nonempty");
        let b = PhysRange::from_start_len(0x2000, 0x100).expect("nonempty");
        assert!(!a.overlaps(b));
        assert!(!b.overlaps(a));
    }

    /// **The mutation target.** `[0x1000, 0x1100)` and `[0x1100, 0x1200)`
    /// touch but share no byte. Widening either `<` in `overlaps` to `<=`
    /// makes this assertion fail, which is the point: adjacency is the exact
    /// boundary Step 2's disjoint population sits on, and a predicate that
    /// calls adjacent buffers "overlapping" would report zero disjoint joins
    /// and silently kill Step 2 for the wrong reason.
    #[test]
    fn adjacent_ranges_are_disjoint_not_overlapping() {
        let a = PhysRange::from_start_len(0x1000, 0x100).expect("nonempty");
        let b = PhysRange::from_start_len(0x1100, 0x100).expect("nonempty");
        assert_eq!(a.end, b.start, "the ranges must actually be adjacent");
        assert!(!a.overlaps(b));
        assert!(!b.overlaps(a));
        assert_eq!(
            classify(&[a], &[b]),
            JoinClass::Disjoint,
            "adjacency must classify as disjoint end to end, not just in the predicate"
        );
    }

    /// A zero-length range is not constructible, so it can never be silently
    /// compared as a point.
    #[test]
    fn an_empty_range_does_not_exist() {
        assert_eq!(PhysRange::from_start_len(0x1000, 0), None);
        assert_eq!(PhysRange::from_guest_ptr_len(0x8000_1000, 0), None);
    }

    /// An end that overflows the physical space yields no range rather than a
    /// wrapped one.
    #[test]
    fn an_overflowing_range_yields_nothing() {
        assert_eq!(PhysRange::from_start_len(u32::MAX, 2), None);
    }

    /// KSEG0 and KSEG1 pointers to the same RDRAM byte must compare equal.
    /// An alias that read as disjoint would be a false skip in Step 2.
    #[test]
    fn kseg0_and_kseg1_aliases_of_one_buffer_overlap() {
        let cached = PhysRange::from_guest_ptr_len(0x8010_0000, 0x1000).expect("nonempty");
        let uncached = PhysRange::from_guest_ptr_len(0xa010_0000, 0x1000).expect("nonempty");
        assert_eq!(
            cached, uncached,
            "the two aliases mask to one physical range"
        );
        assert!(cached.overlaps(uncached));
        assert_eq!(classify(&[cached], &[uncached]), JoinClass::Overlap);
    }

    /// One overlapping pair anywhere in the two sets makes the whole join an
    /// overlap, even when every other pair is disjoint.
    #[test]
    fn one_overlapping_pair_decides_the_whole_join() {
        let next = [
            PhysRange::from_start_len(0x1000, 0x100).expect("nonempty"),
            PhysRange::from_start_len(0x5000, 0x100).expect("nonempty"),
        ];
        let batch = [
            PhysRange::from_start_len(0x9000, 0x100).expect("nonempty"),
            PhysRange::from_start_len(0x5080, 0x100).expect("nonempty"),
        ];
        assert_eq!(classify(&next, &batch), JoinClass::Overlap);
    }

    /// An empty side is indeterminate, never disjoint: a range we failed to
    /// decode must not be counted as proof that a join is skippable.
    #[test]
    fn an_undecodable_side_is_indeterminate_not_disjoint() {
        let range = PhysRange::from_start_len(0x1000, 0x100).expect("nonempty");
        assert_eq!(classify(&[], &[range]), JoinClass::Indeterminate);
        assert_eq!(classify(&[range], &[]), JoinClass::Indeterminate);
        assert_eq!(classify(&[], &[]), JoinClass::Indeterminate);
    }

    /// The task header's six declared pointer/size pairs all become ranges,
    /// and a zero-size pair contributes none.
    #[test]
    fn task_header_inputs_cover_every_declared_pair() {
        let header = fn64_runtime::OsTaskHeader {
            ucode: 0x8010_0000,
            ucode_size: 0x1000,
            ucode_data: 0x8020_0000,
            ucode_data_size: 0x800,
            data_ptr: 0x8030_0000,
            data_size: 0x2000,
            dram_stack: 0x8040_0000,
            dram_stack_size: 0x400,
            output_buff: 0x8050_0000,
            output_buff_size: 0x100,
            // Not a yielded task: contributes no range.
            yield_data_ptr: 0x8060_0000,
            yield_data_size: 0,
            ..Default::default()
        };
        let ranges = next_task_input_ranges(&header);
        assert_eq!(ranges.len(), 5, "five nonzero pairs, one zero-size pair");
        assert!(ranges.contains(&PhysRange::from_start_len(0x0030_0000, 0x2000).expect("nonempty")));
        assert!(
            !ranges.iter().any(|range| range.start == 0x0060_0000),
            "a zero-size yield buffer is not an input range"
        );
    }

    /// `SetColorImage` contributes its target base, masked to 24 bits.
    #[test]
    fn set_color_image_contributes_its_target_base() {
        // 0xff = wire prefix 0xc0 | 0x3f. RGBA/16-bit, width 320.
        let w0 = (0xffu32 << 24) | (0 << 21) | (2 << 19) | (320 - 1);
        let w1 = 0x0030_0000;
        let ranges = batch_ranges_from_words(&[w0, w1], 0x0080_0000);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0x0030_0000);
        assert_eq!(
            ranges[0].end, 0x0080_0000,
            "with no declared height the target runs to the end of RDRAM"
        );
    }

    /// The color-image extent is bounded by the RDP's 24-bit ceiling, never by
    /// the runtime's raw allocation length.
    ///
    /// Regression: the first version of this census passed
    /// `host.runtime_rdram_len` straight through. On the WM2000 lane that
    /// reads `0x2900_0000` -- the runtime allocation backs far more than
    /// console RDRAM -- so every render-target extent ran to 688 MB. The
    /// direction of that bug was safe (an oversized write extent can only
    /// manufacture false OVERLAPs, never false DISJOINTs) but the number it
    /// produced was not one to report.
    #[test]
    fn the_color_image_extent_is_bounded_by_the_rdp_address_ceiling() {
        let w0 = (0xffu32 << 24) | (2 << 19) | (320 - 1);
        let w1 = 0x0030_0000;
        // A runtime allocation far larger than both RDRAM and the RDP's reach.
        let ranges = batch_ranges_from_words(&[w0, w1], 0x2900_0000);
        assert_eq!(ranges.len(), 1);
        assert_eq!(
            ranges[0].end, RDP_PHYSICAL_ADDRESS_BYTES,
            "the extent must stop at the RDP's 24-bit ceiling, not at the allocation length"
        );
        // And when installed RDRAM is the tighter bound, RDRAM wins.
        let ranges = batch_ranges_from_words(&[w0, w1], 0x0080_0000);
        assert_eq!(
            ranges[0].end, 0x0080_0000,
            "installed RDRAM is the tighter bound"
        );
    }

    /// A `LoadBlock` reads from the staged `SetTextureImage`, padded to whole
    /// 64-bit words.
    #[test]
    fn load_block_reads_the_staged_texture_image() {
        // SetTextureImage: RGBA/16-bit, width 64, address 0x0010_0000.
        let timg0 = (0xfdu32 << 24) | (2 << 19) | (64 - 1);
        let timg1 = 0x0010_0000;
        // LoadBlock: source (0,0) .. high_s 3 -> 4 texels * 2 bytes = 8.
        let lb0 = 0x33u32 << 24;
        let lb1 = 3u32 << 12;
        let ranges = batch_ranges_from_words(&[timg0, timg1, lb0, lb1], 0x0080_0000);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0x0010_0000);
        assert_eq!(
            ranges[0].end, 0x0010_0008,
            "4 RGBA16 texels pad to one word"
        );
    }

    /// `SetMaskImage` (the Z buffer) contributes a range.
    ///
    /// Omitting it was the **unsafe** direction: the depth buffer is real RDRAM
    /// traffic, and a missing batch range turns a true overlap into a reported
    /// disjoint -- which would let Step 2 skip a join it must not skip.
    #[test]
    fn set_mask_image_contributes_the_depth_buffer() {
        // 0xfe = wire prefix 0xc0 | 0x3e.
        let w0 = 0xfeu32 << 24;
        let w1 = 0x0020_0000;
        let ranges = batch_ranges_from_words(&[w0, w1], 0x0080_0000);
        assert_eq!(ranges.len(), 1, "the depth buffer must produce a range");
        assert_eq!(ranges[0].start, 0x0020_0000);
        assert_eq!(ranges[0].end, 0x0080_0000);
    }

    /// A next task reading the depth buffer classifies OVERLAP.
    ///
    /// This is the assertion the missing-`SetMaskImage` mutant breaks: drop the
    /// arm and this join silently reports DISJOINT.
    #[test]
    fn a_task_reading_the_depth_buffer_overlaps() {
        let w0 = 0xfeu32 << 24;
        let w1 = 0x0020_0000;
        let batch = batch_ranges_from_words(&[w0, w1], 0x0080_0000);
        let header = fn64_runtime::OsTaskHeader {
            // KSEG0 pointer into the depth buffer the batch is writing.
            data_ptr: 0x8020_1000,
            data_size: 0x100,
            ..Default::default()
        };
        let next = next_task_input_ranges(&header);
        assert_eq!(
            classify(&next, &batch),
            JoinClass::Overlap,
            "a task reading the Z buffer must not be classified disjoint"
        );
    }

    /// `LoadTLUT` reads `entries * 2` bytes from the staged texture image base.
    #[test]
    fn load_tlut_reads_the_palette_from_the_staged_image() {
        // SetTextureImage: 16-bit (the public macros' required size), addr.
        let timg0 = (0xfdu32 << 24) | (2 << 19);
        let timg1 = 0x0010_0000;
        // LoadTLUT with 256 entries: ((w1 >> 14) & 0x3ff) + 1 == 256.
        let tlut0 = 0xf0u32 << 24;
        let tlut1 = (255u32) << 14;
        let ranges = batch_ranges_from_words(&[timg0, timg1, tlut0, tlut1], 0x0080_0000);
        assert_eq!(ranges.len(), 1, "the palette load must produce a range");
        assert_eq!(ranges[0].start, 0x0010_0000);
        assert_eq!(
            ranges[0].end,
            0x0010_0000 + 256 * 2,
            "256 entries of two bytes each"
        );
    }

    /// A next task reading the palette classifies OVERLAP.
    ///
    /// The assertion the missing-`LoadTLUT` mutant breaks.
    #[test]
    fn a_task_reading_the_palette_overlaps() {
        let timg0 = (0xfdu32 << 24) | (2 << 19);
        let timg1 = 0x0010_0000;
        let tlut0 = 0xf0u32 << 24;
        let tlut1 = (255u32) << 14;
        let batch = batch_ranges_from_words(&[timg0, timg1, tlut0, tlut1], 0x0080_0000);
        let header = fn64_runtime::OsTaskHeader {
            data_ptr: 0x8010_0010,
            data_size: 0x10,
            ..Default::default()
        };
        let next = next_task_input_ranges(&header);
        assert_eq!(
            classify(&next, &batch),
            JoinClass::Overlap,
            "a task reading the palette must not be classified disjoint"
        );
    }

    /// A `LoadTLUT` with no staged texture image contributes nothing.
    #[test]
    fn load_tlut_without_a_staged_image_contributes_nothing() {
        let tlut0 = 0xf0u32 << 24;
        let tlut1 = (255u32) << 14;
        assert!(batch_ranges_from_words(&[tlut0, tlut1], 0x0080_0000).is_empty());
    }

    /// Every `RenderBatchJoinCause` maps to its own counter slot, and only
    /// `LaterGraphics` is flagged as an RDRAM-only question.
    ///
    /// The `rdram_only` flag is what stops Step 2 from summing a
    /// `DmemDependency` join into a skippable total: that join is about the
    /// shared DMEM command buffer, which these RDRAM ranges do not model.
    #[test]
    fn each_join_cause_has_its_own_slot_and_only_later_graphics_is_rdram_only() {
        let slots: Vec<usize> = CAUSES.iter().copied().map(cause_index).collect();
        let mut sorted = slots.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            CAUSE_COUNT,
            "every cause needs a distinct slot"
        );
        assert_eq!(slots, (0..CAUSE_COUNT).collect::<Vec<_>>());
        assert!(cause_is_rdram_only(
            crate::RenderBatchJoinCause::LaterGraphics
        ));
        for cause in [
            crate::RenderBatchJoinCause::ViVisibility,
            crate::RenderBatchJoinCause::DmemDependency,
            crate::RenderBatchJoinCause::LaterGraphicsAndDmemDependency,
        ] {
            assert!(
                !cause_is_rdram_only(cause),
                "{cause:?} carries a dependency these RDRAM ranges do not model"
            );
        }
    }

    /// A `LoadBlock` with no staged texture image contributes nothing rather
    /// than reading from address zero.
    #[test]
    fn load_block_without_a_staged_image_contributes_nothing() {
        let lb0 = 0x33u32 << 24;
        let lb1 = 3u32 << 12;
        assert!(batch_ranges_from_words(&[lb0, lb1], 0x0080_0000).is_empty());
    }

    /// A stream whose opcode the width table rejects stops the walk, so the
    /// classifier sees an empty side and reports indeterminate -- it must not
    /// desynchronise into invented ranges.
    #[test]
    fn an_unknown_opcode_stops_the_walk_rather_than_inventing_ranges() {
        // 0x10 is inside the width table's rejected block.
        let bad = 0x10u32 << 24;
        assert!(batch_ranges_from_words(&[bad, 0], 0x0080_0000).is_empty());
    }

    /// **Negative control, committed.** A decoded batch compared against
    /// *itself* must report OVERLAP on every range kind.
    ///
    /// The headline lane result is zero overlaps, so the pipeline's ability to
    /// report a non-degenerate OVERLAP at all is worth pinning rather than
    /// leaving to a one-off run. The live version of this control (comparing
    /// each real batch to itself over the 3,000-pump lane) reported
    /// 1140 joins / 1140 overlap / 0 disjoint -- the exact inverse of the real
    /// run -- which is what established that the zero-overlap headline is a
    /// property of the data and not a broken comparison. This test is that
    /// control over a stream exercising all five decoded opcodes.
    #[test]
    fn a_batch_compared_against_itself_always_overlaps() {
        let stream = [
            // SetColorImage: RGBA/16, width 320, target 0x0030_0000.
            (0xffu32 << 24) | (2 << 19) | (320 - 1),
            0x0030_0000,
            // SetMaskImage: depth buffer at 0x0020_0000.
            0xfeu32 << 24,
            0x0020_0000,
            // SetTextureImage: 16-bit, width 64, source 0x0010_0000.
            (0xfdu32 << 24) | (2 << 19) | (64 - 1),
            0x0010_0000,
            // LoadBlock: 4 texels.
            0x33u32 << 24,
            3u32 << 12,
            // LoadTLUT: 256 entries.
            0xf0u32 << 24,
            255u32 << 14,
        ];
        let batch = batch_ranges_from_words(&stream, 0x0080_0000);
        assert!(
            batch.len() >= 4,
            "all five opcodes should contribute ranges, got {}",
            batch.len()
        );
        assert_eq!(
            classify(&batch, &batch),
            JoinClass::Overlap,
            "a batch must overlap itself; a Disjoint here means the pipeline \
             cannot report an overlap at all"
        );
    }

    /// The census is off unless explicitly armed, so an unarmed process pays
    /// one cached branch and records nothing.
    #[test]
    fn the_census_is_off_unless_armed() {
        assert!(
            !enabled(),
            "FN64_RENDER_JOIN_CENSUS must not be set in the test environment"
        );
        let slot = cause_index(crate::RenderBatchJoinCause::LaterGraphics);
        let before = JOINS[slot].load(Relaxed);
        note_join(
            &fn64_runtime::OsTaskHeader::default(),
            &[],
            0x0080_0000,
            crate::RenderBatchJoinCause::LaterGraphics,
        );
        assert_eq!(
            JOINS[slot].load(Relaxed),
            before,
            "an unarmed census records nothing"
        );
    }
}
