//! A ROM-independent, adapterless harness for the RDP wire -> guest bytes seam.
//!
//! The layer under test takes **wire words plus staged RDP state and produces
//! guest bytes**. Neither end needs a ROM. What a ROM tells you is *which*
//! wire words are realistic -- a question answered once, with a census, not a
//! dependency of the inner loop.
//!
//! This replaces a ~40-line plan/execute/commit/seal/publish preamble that
//! every end-to-end triangle test previously repeated by hand:
//!
//! ```ignore
//! let frame = Rdp::new()
//!     .one_cycle()
//!     .combine_prim_passthrough()
//!     .prim_color(0x80FF_4080)
//!     .triangle(Tri::flat().left_major().edges(2.0, 6.0).rows(0..3))
//!     .run();
//! frame.assert_pixel(3, 1, 0x87D1);
//! ```
//!
//! **It runs in the default suite with no GPU adapter.** That is the whole
//! reason the CPU seam was chosen for guest-visible correctness; a harness
//! needing Metal would reintroduce the problem it exists to solve.
//!
//! **It builds WIRE WORDS and goes through the real decoder.** It never
//! constructs `RawTriangle` or `ResourceAccess` directly -- the moment it
//! shortcuts past the decoder it stops testing the thing that breaks.

use crate::production::{WgpuBackend, WgpuCreateError};
use crate::wire_words::{
    line, set_combine, set_other_mode, set_prim_color, word, EdgeWords, D_SLOT_PRIMITIVE,
    D_SLOT_SHADE, RAW_TRIANGLE_BASE_EDGE,
};
use fn64_render::{OwnedRawDpcSubmission, RawDpcAbiSession, RenderBackend};
use fn64_render_ir::{
    CompletedWrite, DeferredGuestReadCapture, DpInterruptState, TemporalBoundary,
};

const LAYOUT_BYTES: u32 = 0x4000;
const COMMAND_START: u32 = 0x1000;

/// The RGBA16 colour image every harness frame targets. 64-byte aligned, as
/// `SetColorImage`'s own decode requires.
pub(crate) const TARGET_ADDRESS: u32 = 0x2000;

const SET_COLOR_IMAGE: u8 = 0x3f;
const SET_FILL_COLOR: u8 = 0x37;
const FILL_RECTANGLE: u8 = 0x36;

/// What an untouched pixel reads back as: a colour with every 5/5/5/1 channel
/// distinct from black, from white, and from the primitive colours the tests
/// pick, so a pixel the raster skipped is never confusable with one it wrote.
pub(crate) const CLEAR_COLOR_RGBA16: u16 = 0x1085;
/// `SetFillColor` in an RGBA16 image carries TWO packed pixels, and a fill
/// writes the high half to the even column and the low half to the odd one.
/// Both halves are `CLEAR_COLOR_RGBA16` so an untouched pixel reads the same
/// value regardless of its column parity -- a fill colour whose halves differ
/// would make every "untouched" assertion depend on x % 2.
pub(crate) const CLEAR_COLOR_WIRE: u32 =
    ((CLEAR_COLOR_RGBA16 as u32) << 16) | CLEAR_COLOR_RGBA16 as u32;

/// The RDP cycle type staged into `SetOtherMode`'s bits 21..20.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "all four RDP cycle types are named; TwoCycle has no \
    harness fixture yet and is exactly the case a future test must be able to state"
)]
pub(crate) enum CycleType {
    One = 0,
    Two = 1,
    Copy = 2,
    Fill = 3,
}

/// A triangle stated in PIXEL and SUBPIXEL coordinates, so an edge case can be
/// *stated* ("put the edge at 0.75 px") rather than found by search.
///
/// Two mutation survivors in this lane (M5, M10) existed only because a
/// fixture happened to sample a point where the correct and incorrect answers
/// coincide. Naming the edge position directly is what prevents that class of
/// gap.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tri {
    opcode: u8,
    lft: bool,
    tile: u32,
    level: u32,
    /// Left edge X in pixels; fractional values are exact in Q16.16.
    left_x: f64,
    /// Right edge X in pixels.
    right_x: f64,
    /// dX/dy for the left (XH) edge, in pixels per scanline.
    left_slope: f64,
    /// dX/dy for the right (XL/XM) edges, in pixels per scanline.
    right_slope: f64,
    /// The upper minor (XM) edge, when it differs from XL. `None` parks XM on
    /// XL, which is what a two-edge triangle wants; `Some` splits them so the
    /// YM crossover between them is observable.
    upper_right_x: Option<f64>,
    /// First and last scanline, in whole pixels.
    y_start: i16,
    y_end: i16,
    /// The scanline at which the minor edge switches from XM to XL. `None`
    /// puts it at `y_end`, so XM governs the whole triangle.
    ym_row: Option<i16>,
    /// Optional per-component shade planes.
    shade: Option<ShadePlanes>,
}

/// One shaded triangle's four coefficient groups: (value, d/dx, d/de, d/dy),
/// each carrying the four RGBA components.
pub(crate) type ShadePlanes = ([i32; 4], [i32; 4], [i32; 4], [i32; 4]);

/// Q16.16 from a pixel coordinate that may carry a fraction.
///
/// Rounds to nearest so a coordinate like 0.75 lands on its exact Q16.16
/// representation (0.75 * 65536 = 49152) rather than one ULP below it.
pub(crate) fn px_frac(pixels: f64) -> i32 {
    (pixels * 65536.0).round() as i32
}

#[allow(
    dead_code,
    reason = "the builder states the whole wire surface; not every \
    field has a caller yet, and a builder that can only express today's fixtures \
    is the gap this harness exists to close"
)]
impl Tri {
    /// A flat (non-shaded, non-textured, non-depth) triangle, opcode 0x08.
    pub(crate) fn flat() -> Self {
        Self {
            opcode: RAW_TRIANGLE_BASE_EDGE,
            lft: false,
            tile: 0,
            level: 0,
            left_x: 0.0,
            right_x: 0.0,
            left_slope: 0.0,
            right_slope: 0.0,
            upper_right_x: None,
            y_start: 0,
            y_end: 0,
            ym_row: None,
            shade: None,
        }
    }

    /// Sets wire bit 23. A left-major triangle's XH edge is its LEFT edge.
    pub(crate) fn left_major(mut self) -> Self {
        self.lft = true;
        self
    }

    pub(crate) fn tile(mut self, tile: u32) -> Self {
        self.tile = tile;
        self
    }

    pub(crate) fn level(mut self, level: u32) -> Self {
        self.level = level;
        self
    }

    /// Parks both edges at fixed X positions, in pixels, with zero slope.
    pub(crate) fn edges(mut self, left_x: f64, right_x: f64) -> Self {
        self.left_x = left_x;
        self.right_x = right_x;
        self
    }

    /// Parks the upper minor (XM) edge at its own X, distinct from XL.
    ///
    /// The RDP switches the minor edge from XM to XL at YM, so a triangle
    /// whose XM and XL differ makes that crossover visible; one where they
    /// coincide cannot see it at all.
    pub(crate) fn upper_right(mut self, x: f64) -> Self {
        self.upper_right_x = Some(x);
        self
    }

    /// Gives the edges per-scanline slopes, in pixels per scanline.
    pub(crate) fn slopes(mut self, left: f64, right: f64) -> Self {
        self.left_slope = left;
        self.right_slope = right;
        self
    }

    /// The scanline where the minor edge switches from XM to XL.
    pub(crate) fn ym_row(mut self, row: i16) -> Self {
        self.ym_row = Some(row);
        self
    }

    /// The half-open scanline range the triangle spans, in whole pixels.
    pub(crate) fn rows(mut self, rows: std::ops::Range<i16>) -> Self {
        self.y_start = rows.start;
        self.y_end = rows.end;
        self
    }

    /// Promotes the triangle to opcode 0x0c and attaches four shade planes.
    pub(crate) fn shade(
        mut self,
        value: [i32; 4],
        dx: [i32; 4],
        de: [i32; 4],
        dy: [i32; 4],
    ) -> Self {
        self.opcode = crate::wire_words::RAW_TRIANGLE_SHADE;
        self.shade = Some((value, dx, de, dy));
        self
    }

    /// The triangle's own wire words: four base-edge words, plus eight
    /// coefficient words when shaded.
    pub(crate) fn words(&self) -> Vec<u32> {
        let edges = EdgeWords {
            lft: self.lft,
            tile: self.tile,
            level: self.level,
            // YL is the LAST covered scanline's lower bound: a `rows(0..3)`
            // triangle spans scanlines 0, 1 and 2, so YL is line 3 in S11.2
            // and the raster's `y < yl` bound stops after row 2.
            yl: line(self.y_end),
            ym: line(self.ym_row.unwrap_or(self.y_end)),
            yh: line(self.y_start),
            xl: px_frac(self.right_x),
            dxldy: px_frac(self.right_slope),
            xh: px_frac(self.left_x),
            dxhdy: px_frac(self.left_slope),
            xm: px_frac(self.upper_right_x.unwrap_or(self.right_x)),
            dxmdy: px_frac(self.right_slope),
        };
        let mut words = edges.words(0, self.opcode).to_vec();
        if let Some((value, dx, de, dy)) = self.shade {
            words.extend(crate::wire_words::coefficient_halves(value, dx, de, dy));
        }
        words
    }
}

/// One staged frame: colour-image extent, RDP state, and the triangles to
/// raster into it.
pub(crate) struct Rdp {
    width: u32,
    height: u32,
    cycle_type: CycleType,
    other_mode_low: u32,
    combine: Option<(u32, u32)>,
    prim_color: Option<u32>,
    triangles: Vec<Tri>,
}

#[allow(
    dead_code,
    reason = "see Tri's own allow: the staged-state surface is \
    complete by design, ahead of its callers"
)]
impl Rdp {
    /// A frame targeting a `width` x `height` RGBA16 colour image.
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            cycle_type: CycleType::One,
            other_mode_low: 0,
            combine: None,
            prim_color: None,
            triangles: Vec::new(),
        }
    }

    pub(crate) fn cycle(mut self, cycle_type: CycleType) -> Self {
        self.cycle_type = cycle_type;
        self
    }

    pub(crate) fn other_mode_low(mut self, low: u32) -> Self {
        self.other_mode_low = low;
        self
    }

    /// Stages a combiner program from its two packed wire slices.
    pub(crate) fn combine(mut self, wire: (u32, u32)) -> Self {
        self.combine = Some(wire);
        self
    }

    /// `(Zero - Zero) * Zero + Primitive`.
    pub(crate) fn combine_prim_passthrough(self) -> Self {
        let wire = crate::wire_words::passthrough_combine(D_SLOT_PRIMITIVE);
        self.combine(wire)
    }

    /// `(Zero - Zero) * Zero + Shade`.
    pub(crate) fn combine_shade_passthrough(self) -> Self {
        let wire = crate::wire_words::passthrough_combine(D_SLOT_SHADE);
        self.combine(wire)
    }

    pub(crate) fn prim_color(mut self, color: u32) -> Self {
        self.prim_color = Some(color);
        self
    }

    pub(crate) fn triangle(mut self, triangle: Tri) -> Self {
        self.triangles.push(triangle);
        self
    }

    fn set_color_image(&self) -> [u32; 2] {
        // Wire `format` 0 (Rgba), `size` 2 (Bits16), and a width field of
        // width-1 which the decoder adds one back to.
        [
            word(SET_COLOR_IMAGE, 2 << 19 | (self.width - 1)),
            TARGET_ADDRESS,
        ]
    }

    /// The whole-target fill that establishes generation 1 honestly.
    ///
    /// `admit_completed_initialization` rejects a partial rectangle against a
    /// target with no predecessor, because a brand-new target has no prior
    /// device bytes for the untouched rows and admitting one would publish
    /// fabricated zeros as if they were real content. This is also the real
    /// order: a title clears its framebuffer before drawing into it.
    fn clear_words(&self) -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(set_other_mode(CycleType::Fill as u32, 0));
        words.extend(self.set_color_image());
        words.extend([word(SET_FILL_COLOR, 0), CLEAR_COLOR_WIRE]);
        let (x1, y1) = (self.width - 1, self.height - 1);
        // FillRectangle's coordinates are 10.2 fixed point.
        words.extend([word(FILL_RECTANGLE, ((x1 << 2) << 12) | (y1 << 2)), 0]);
        words
    }

    /// The staged-state + triangle packet's wire words.
    fn draw_words(&self) -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(set_other_mode(self.cycle_type as u32, self.other_mode_low));
        if let Some((low, high)) = self.combine {
            words.extend(set_combine(low, high));
        }
        if let Some(color) = self.prim_color {
            words.extend(set_prim_color(0, 0, color));
        }
        words.extend(self.set_color_image());
        for triangle in &self.triangles {
            words.extend(triangle.words());
        }
        words
    }

    /// plan -> execute -> commit -> seal -> publish, with no ROM and no GPU
    /// adapter, returning the guest-visible bytes.
    ///
    /// Panics if any stage refuses. Use [`Rdp::try_run`] to inspect a refusal.
    pub(crate) fn run(self) -> Frame {
        self.try_run().expect("harness frame completes")
    }

    /// As [`Rdp::run`], but returns the refusal instead of panicking, so a
    /// test can name which guard fired.
    pub(crate) fn try_run(self) -> Result<Frame, HarnessRefusal> {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_extent(&mut backend, self.width, self.height);

        // The clear is its own packet: a fill in the SAME packet as a raw
        // triangle is refused by `MixedFillAndTrianglePacket`.
        publish_packet(&mut backend, &mut session, self.clear_words())
            .map_err(HarnessRefusal::Clear)?;

        let staged = publish_packet(&mut backend, &mut session, self.draw_words())
            .map_err(HarnessRefusal::Draw)?;

        let bytes = backend
            .color_targets()
            .and_then(|registry| {
                registry
                    .residents()
                    .iter()
                    .find(|resident| resident.key().address().get() == TARGET_ADDRESS)
                    .map(|resident| resident.device_bytes().device_bytes().to_vec())
            })
            .ok_or(HarnessRefusal::NoResident)?;

        Ok(Frame {
            width: self.width,
            height: self.height,
            bytes,
            writes: staged,
        })
    }
}

/// Which stage of the harness refused, keyed so a test can assert on the
/// stage rather than on a formatted string.
#[derive(Debug)]
pub(crate) enum HarnessRefusal {
    /// The target-establishing clear did not complete.
    Clear(String),
    /// The staged-state + triangle packet did not complete.
    Draw(String),
    /// Nothing published a resident at the target address.
    NoResident,
}

impl HarnessRefusal {
    /// The refusing stage's message, for tests that assert a named guard.
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Clear(message) | Self::Draw(message) => message,
            Self::NoResident => "no resident published at the target address",
        }
    }
}

/// The guest-visible result of one harness frame.
pub(crate) struct Frame {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
    writes: Vec<CompletedWrite>,
}

/// Summarizes rather than dumping the whole buffer: a failed `expect_err`
/// should print what the frame covered, not several hundred bytes.
impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("write_ranges", &self.write_ranges())
            .finish()
    }
}

impl Frame {
    /// The RGBA16 pixel at `(x, y)`, read from the published resident's own
    /// device bytes.
    pub(crate) fn pixel(&self, x: u32, y: u32) -> u16 {
        let offset = ((y * self.width + x) * 2) as usize;
        u16::from_be_bytes([self.bytes[offset], self.bytes[offset + 1]])
    }

    pub(crate) fn assert_pixel(&self, x: u32, y: u32, expected: u16) {
        assert_eq!(
            self.pixel(x, y),
            expected,
            "pixel ({x},{y}): expected {expected:#06x}, got {:#06x}",
            self.pixel(x, y)
        );
    }

    /// Every pixel outside `columns` x `rows` still holds the clear colour.
    pub(crate) fn assert_outside_untouched(
        &self,
        columns: std::ops::Range<u32>,
        rows: std::ops::Range<u32>,
    ) {
        for y in 0..self.height {
            for x in 0..self.width {
                if columns.contains(&x) && rows.contains(&y) {
                    continue;
                }
                assert_eq!(
                    self.pixel(x, y),
                    CLEAR_COLOR_RGBA16,
                    "pixel ({x},{y}) is outside the drawn region and must keep the clear colour"
                );
            }
        }
    }

    /// The draw packet's committed guest writes, as `(start_address, bytes)`
    /// in declaration order -- the same list `copy_committed_guest_writes`
    /// re-derives before writing a byte into guest RDRAM.
    ///
    /// Reports the write's STORED `byte_count` rather than its declared
    /// range's length. For a well-formed write the two agree (pinned by
    /// `each_committed_writes_stored_byte_count_matches_its_declared_range_length`),
    /// so swapping them here is an equivalent mutation and no test can tell
    /// them apart -- deliberately so: the stored count is what the guest copy
    /// actually trusts, which makes it the honest one to report.
    pub(crate) fn write_ranges(&self) -> Vec<(u32, u32)> {
        self.writes
            .iter()
            .map(|write| match write.access().region() {
                fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                    (range.start().get(), write.byte_count())
                }
                other => panic!("a render-target write must name an RDRAM range, got {other:?}"),
            })
            .collect()
    }

    pub(crate) fn writes(&self) -> &[CompletedWrite] {
        &self.writes
    }
}

/// Records the host-configured framebuffer extent without requiring a GPU
/// adapter.
///
/// `create_inner` stores `configured_target_extent` *before* it requests a
/// device, precisely so admitted CPU-side executors can run on an adapterless
/// host. A `NoAdapter` result is therefore expected and ignored; any other
/// create failure still panics, because that would mean the extent was not
/// recorded for the reason this helper assumes.
fn configure_extent(backend: &mut WgpuBackend, width: u32, height: u32) {
    match backend.create_inner(&fn64_render::RenderConfig {
        width,
        height,
        tv_type: fn64_runtime::TvType::default(),
    }) {
        Ok(()) | Err(WgpuCreateError::NoAdapter(_)) => {}
        Err(other) => panic!("create_inner failed for an unexpected reason: {other}"),
    }
    assert!(
        backend.has_configured_target_extent(),
        "create_inner must record the host-configured extent even with no GPU adapter"
    );
}

fn capture(words: Vec<u32>) -> fn64_render::OwnedRawDpcCapture {
    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
    let end = COMMAND_START + u32::try_from(words.len() * 4).unwrap();
    let submission =
        OwnedRawDpcSubmission::from_rdram_words(COMMAND_START, end, words.clone()).unwrap();
    fn64_render::OwnedRawDpcCapture::new(
        submission,
        layout,
        7,
        TemporalBoundary::new(1, DpInterruptState::Clear),
    )
}

fn admitted_fabric(
) -> fn64_runtime::DeviceFabric<fn64_runtime::rom::InMemoryRom, fn64_runtime::FixedPiTiming> {
    let mut fabric = fn64_runtime::DeviceFabric::new(
        fn64_runtime::rom::PiDma::new(fn64_runtime::rom::InMemoryRom::new(Vec::new())),
        fn64_runtime::FixedPiTiming(fn64_runtime::Cycles::new(0)),
    );
    fabric
        .request_dpc_submission(fn64_runtime::DpcSubmissionSource::Rdram, 0x100, 0x108)
        .unwrap()
        .expect("fresh fabric is never frozen");
    fabric
}

/// One packet all the way through plan -> execute -> commit -> seal ->
/// publish, returning the guest writes it committed.
///
/// Every refusal is returned as its stage's message rather than unwrapped, so
/// the harness can report which guard fired instead of panicking inside a
/// builder chain.
fn publish_packet(
    backend: &mut WgpuBackend,
    session: &mut RawDpcAbiSession,
    words: Vec<u32>,
) -> Result<Vec<CompletedWrite>, String> {
    let request = session.plan_request(capture(words));
    let planned = backend.plan_raw_dpc(request).map_err(|e| e.to_string())?;
    assert!(
        planned.guest_read_plan().reads().is_empty(),
        "a fill/triangle-only plan must declare zero TmemLoadSource reads"
    );
    let bound = session
        .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
        .map_err(|e| e.to_string())?;
    let submission = bound.submission();
    let prepared = backend.execute_raw_dpc(bound).map_err(|e| e.to_string())?;
    let staged = backend.staged_guest_render_target_writes(submission);
    let committed = session
        .commit_guest_render_target_writes(prepared, staged.clone())
        .map_err(|e| e.to_string())?;
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session
        .seal_publication(committed, ready)
        .map_err(|e| e.to_string())?;
    backend.publish_raw_dpc(capsule);
    Ok(staged)
}

#[cfg(test)]
mod tests;
