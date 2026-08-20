//! M4.3.4: Fill-cycle `FillRectangle` execution against a color target.
//!
//! Scope is exactly the `CycleType::Fill` branch of a decoded RDP
//! `FILL_RECTANGLE` (opcode `0x36`) against an RGBA16 or RGBA32 color image:
//! no combiner, blend, coverage, depth, texture, or triangle behavior. One-
//! and two-cycle `G_FILLRECT` need the combiner (`evaluate_cycle`,
//! `fn64-render-reference/src/raster/combiner.rs`, `pub(super)` and not
//! ported here); Copy-cycle `G_FILLRECT` is out of scope per
//! `fn64-render-reference/src/raster/draw.rs:120-124` ("no guaranteed public
//! result; use `G_TEXRECT`"). Neither is decoded or executed by this module.
//!
//! ## Coordinate provenance
//!
//! `FillRectangle`'s four wire fields (`raw_dpc/mod.rs:92-114`) are extracted
//! as `(word >> 12) & 0x0fff` from the command's two words -- the same 12-bit
//! field the reference lane extracts at
//! `fn64-render-reference/src/gbi/stream.rs:1087,1313,1337` before dividing
//! by 4.0 to a float pixel coordinate. Both lanes read one identical wire
//! field: this is the public RDP rectangle-coordinate encoding, 10 integer
//! bits + 2 fractional bits (quarter-pixel resolution), matching the
//! Programming Manual's rectangle-command coordinate format (SGI/Nintendo
//! *N64 Programming Manual*, RDP command summary for `FILL_RECTANGLE`;
//! section number not independently reconfirmed here -- this claim rests on
//! the two allowed in-repo sources agreeing on the identical bit-field
//! extraction and its `/4` scale, not on a freshly read Programming Manual
//! page. Loud nonclaim, not a silent shrug: if a future slice needs the
//! literal manual section number, get it from a fresh, cited read of the
//! manual, not from this comment.)
//!
//! The apparent `raw_dpc/mod.rs:1059` `>>2` vs.
//! `fn64-render-reference/src/raster/draw.rs:198-205` `.ceil()/.floor()`
//! discrepancy the M4.3.3x audit flagged is **reconciled, not chosen-over**:
//! they are not competing rounding rules on the same representation.
//! `plan_fill` (`raw_dpc/mod.rs:1045-1058`) first *rejects* any
//! `FillRectangle` whose low two fixed-point bits are nonzero ("coordinates
//! must be whole pixels"), then `>>2` on an already-whole-pixel value is an
//! exact, lossless integer division by 4 -- there is no fractional part left
//! to round. The reference lane's `.ceil()`/`.floor()` handle the *general*
//! case (including genuinely fractional quarter-pixel edges, which real RDP
//! hardware and this decoder both permit on the wire but wgpu's decoder
//! chooses not to admit past `plan_fill`). This module inherits `plan_fill`'s
//! same whole-pixel restriction (see [`FillCoordinateError::FractionalEdge`])
//! for the same reason `plan_fill` already established it, and for a fill
//! cycle specifically the reference lane's own rounding at
//! `draw.rs:198,203` (`rect.ulx.ceil()`, `rect.lrx.floor()`, an **inclusive**
//! lower/right edge -- see `draw.rs:111`) reduces to plain integer edges
//! whenever the input is already whole-pixel, so both lanes compute the same
//! integer pixel range in the domain this slice admits. Fractional-edge Fill
//! rectangles (genuinely sub-pixel, needing `.ceil()`/`.floor()` to do real
//! rounding work) are out of scope for this slice; `FillCoordinateError`
//! names that gap loudly rather than silently truncating.
//!
//! ## Z/framebuffer bypass hazard
//!
//! `docs/BASE-RENDERER-BEHAVIOR-MATRIX.md`'s `rdp-command-state-order` row
//! grades this `exact_public`: fill-cycle `G_FILLRECT` "rejects every
//! nonempty Z_CMP/Z_UPD/IM_RD bypass-hazard combination before framebuffer or
//! depth mutation". [`require_safe_fill_cycle_bypass`] ports
//! `fn64-render-reference/src/raster/blend.rs:13-21`'s check (itself citing
//! *Nintendo 64 Functions Reference*, `gDPFillRectangle` "Note" and
//! `gDPSetCycleType` "Notes") onto `crate::state::OtherMode`'s existing
//! `low`/`high` wire words -- same bit positions (`low & 0x0010/0x0020/0x0040`
//! for Z_CMP/Z_UPD/IM_RD, `fn64-render-reference/src/gbi/types.rs:445-455`).
//! This slice adds the check rather than deferring it, since the matrix
//! already asserts it as exact-public fill-cycle behavior, not a gap.

use crate::state::{CycleType, FillColor, OtherMode};

use super::{
    CandidateColorTarget, ColorTargetFormat, ColorTargetKey, CompletedColorTargetWrite,
    DeviceColorBytes, RdpScissorRect, Rgba8, TargetError, TargetRectangle,
};

/// One RDP fill-cycle bypass hazard bit, decoded from [`OtherMode`]'s wire
/// words. Provenance: `fn64-render-reference/src/gbi/types.rs:445-455`
/// (`depth_compare_enabled`/`depth_update_enabled`/`image_read_enabled`),
/// itself citing *Nintendo 64 Functions Reference* `gDPSetRenderMode`/
/// `gDPSetOtherMode` bit tables for `Z_CMP`(0x0010)/`Z_UPD`(0x0020)/
/// `IM_RD`(0x0040) on the low `SetOtherModes` word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FillCycleBypassHazards {
    depth_compare: bool,
    depth_update: bool,
    image_read: bool,
}

impl FillCycleBypassHazards {
    const fn is_empty(self) -> bool {
        !self.depth_compare && !self.depth_update && !self.image_read
    }
}

impl core::fmt::Display for FillCycleBypassHazards {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut separator = "";
        for (enabled, name) in [
            (self.depth_compare, "Z_CMP"),
            (self.depth_update, "Z_UPD"),
            (self.image_read, "IM_RD"),
        ] {
            if enabled {
                formatter.write_str(separator)?;
                formatter.write_str(name)?;
                separator = "+";
            }
        }
        Ok(())
    }
}

fn other_mode_fill_cycle_bypass_hazards(other_mode: OtherMode) -> FillCycleBypassHazards {
    FillCycleBypassHazards {
        depth_compare: other_mode.low() & 0x0010 != 0,
        depth_update: other_mode.low() & 0x0020 != 0,
        image_read: other_mode.low() & 0x0040 != 0,
    }
}

/// Rejects an `OtherMode` that retains Z/framebuffer-consumer state while
/// bypassing the RDP pixel pipeline in Fill cycle. See module doc "Z/
/// framebuffer bypass hazard".
fn require_safe_fill_cycle_bypass(other_mode: OtherMode) -> Result<(), FillExecutionError> {
    let hazards = other_mode_fill_cycle_bypass_hazards(other_mode);
    if hazards.is_empty() {
        Ok(())
    } else {
        Err(FillExecutionError::UnsafeFillCycleBypass { hazards })
    }
}

/// A `FillRectangle`'s four wire coordinates could not be resolved to a
/// whole-pixel range. See module doc "Coordinate provenance".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillCoordinateError {
    FractionalEdge { field: &'static str, raw: u16 },
    ReversedRectangle { x0: u32, y0: u32, x1: u32, y1: u32 },
}

impl core::fmt::Display for FillCoordinateError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::FractionalEdge { field, raw } => write!(
                formatter,
                "FillRectangle {field}={raw:#x} has a nonzero quarter-pixel fraction; \
                 this slice only executes whole-pixel fill-cycle rectangles"
            ),
            Self::ReversedRectangle { x0, y0, x1, y1 } => write!(
                formatter,
                "FillRectangle upper-left ({x0}, {y0}) is not upper-left of \
                 lower-right ({x1}, {y1})"
            ),
        }
    }
}

impl std::error::Error for FillCoordinateError {}

/// The whole-pixel, inclusive-edge rectangle a decoded `FillRectangle`
/// resolves to. Inclusive lower/right edge matches
/// `fn64-render-reference/src/raster/draw.rs:111` ("Fill cycle ... includes
/// the lower/right edge").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillPixelRectangle {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl FillPixelRectangle {
    pub const fn x0(self) -> u32 {
        self.x0
    }
    pub const fn y0(self) -> u32 {
        self.y0
    }
    pub const fn x1(self) -> u32 {
        self.x1
    }
    pub const fn y1(self) -> u32 {
        self.y1
    }
    pub const fn width(self) -> u32 {
        self.x1 - self.x0 + 1
    }
    pub const fn height(self) -> u32 {
        self.y1 - self.y0 + 1
    }
}

fn whole_pixel(field: &'static str, raw: u16) -> Result<u32, FillCoordinateError> {
    if raw & 0x3 != 0 {
        return Err(FillCoordinateError::FractionalEdge { field, raw });
    }
    Ok(u32::from(raw) >> 2)
}

/// Resolves a decoded `FillRectangle`'s four wire coordinates to a whole-
/// pixel, inclusive-edge rectangle. Independent re-derivation of
/// `raw_dpc::plan_fill`'s coordinate math (`raw_dpc/mod.rs:1045-1068`) at the
/// same wire-field granularity, not a call into it -- `plan_fill` is a
/// private resource-journal step with its own error type.
pub fn resolve_fill_pixel_rectangle(
    upper_left_x: u16,
    upper_left_y: u16,
    lower_right_x: u16,
    lower_right_y: u16,
) -> Result<FillPixelRectangle, FillCoordinateError> {
    let x0 = whole_pixel("upper_left_x", upper_left_x)?;
    let y0 = whole_pixel("upper_left_y", upper_left_y)?;
    let x1 = whole_pixel("lower_right_x", lower_right_x)?;
    let y1 = whole_pixel("lower_right_y", lower_right_y)?;
    if x0 > x1 || y0 > y1 {
        return Err(FillCoordinateError::ReversedRectangle { x0, y0, x1, y1 });
    }
    Ok(FillPixelRectangle { x0, y0, x1, y1 })
}

/// Clips one resolved fill rectangle against the scissor and the colour
/// target extent, returning the surviving whole-pixel rectangle.
///
/// ## The scissor clips; it does not reject and it is not ignored
///
/// angrylion applies the `clip` rect latched by `rdp_set_scissor`
/// (`rasterizer.c:2779-2784`) inside the edgewalker, which every primitive
/// the rasterizer walks shares -- there is no fill-specific bypass. The X
/// clamp at `:2349-2363` drives a span endpoint out to `clipxhshift` on the
/// low side and `clipxlshift` on the high side rather than discarding the
/// span, and the Y limits at `:2284-2305` (`yllimit = yllimit ? yl :
/// wstate->clip.yl;`) bound the walked scanline range the same way. So a
/// fill whose rectangle overhangs the scissor paints the intersection and
/// leaves the remainder holding whatever the framebuffer already held.
///
/// fn64's own reference renderer computes the identical intersection at
/// `fn64-render-reference/src/raster/draw.rs:191-208`, clamping with
/// `.max(clip_min_x)` / `.min(clip_max_x - 1).min(self.width - 1)` and
/// returning early only when the result is empty.
///
/// ## Precedence: scissor AND target extent, neither substituting
///
/// Both bounds are applied, exactly as [`super::clip_texrect_extent`]
/// applies them. A scissor tighter than the framebuffer really does suppress
/// pixels the framebuffer could hold; separately, no span may name memory
/// outside this executor's sized target. The intersection is strictly
/// narrower than either.
///
/// ## What this refuses
///
/// An empty result, as [`FillExecutionError::ScissoredAway`] -- never a
/// silent no-op. See that variant's own doc.
fn clip_fill_rectangle(
    rectangle: FillPixelRectangle,
    scissor: RdpScissorRect,
    key: ColorTargetKey,
) -> Result<FillPixelRectangle, FillExecutionError> {
    let extent = key.extent();
    // Half-open intersection of three spans -- the rectangle's own
    // (inclusive `x1`, hence the `+ 1`), the scissor's, and the target's --
    // then converted back to this module's inclusive-edge representation.
    let first_x = rectangle.x0().max(scissor.first_column());
    let limit_x = (rectangle.x1() + 1)
        .min(scissor.column_limit())
        .min(extent.width());
    let first_y = rectangle.y0().max(scissor.first_row());
    let limit_y = (rectangle.y1() + 1)
        .min(scissor.row_limit())
        .min(extent.height());
    if first_x >= limit_x || first_y >= limit_y {
        return Err(FillExecutionError::ScissoredAway {
            key,
            rectangle,
            scissor,
        });
    }
    Ok(FillPixelRectangle {
        x0: first_x,
        y0: first_y,
        x1: limit_x - 1,
        y1: limit_y - 1,
    })
}

/// 5-bit-per-channel expansion, matching
/// `fn64-render-reference/src/raster/draw.rs:132-134` bit-for-bit:
/// `(value << 3) | (value >> 2)`.
const fn expand_five(value: u8) -> u8 {
    (value << 3) | (value >> 2)
}

/// Decodes one RDP fill-cycle `FillRectangle` pixel color for the pixel at
/// column `x` (target-relative, not rectangle-relative), matching
/// `fn64-render-reference/src/raster/draw.rs:130-190`'s `CycleType::Fill`
/// branch bit-for-bit for RGBA16 (period 2) and RGBA32 (period 1).
/// `Index8`/`ColorIndex` is out of scope (`draw.rs:144-151`, `plan_fill`
/// already rejects non-RGBA formats before this is reachable).
pub fn decode_fill_cycle_pixel(fill_color: FillColor, format: ColorTargetFormat, x: u32) -> Rgba8 {
    match format {
        ColorTargetFormat::Rgba16 => {
            let halfword = if x.is_multiple_of(2) {
                (fill_color.value() >> 16) as u16
            } else {
                fill_color.value() as u16
            };
            Rgba8::new(
                expand_five(((halfword >> 11) & 0x1f) as u8),
                expand_five(((halfword >> 6) & 0x1f) as u8),
                expand_five(((halfword >> 1) & 0x1f) as u8),
                if halfword & 1 != 0 { 255 } else { 0 },
            )
        }
        ColorTargetFormat::Rgba32 => {
            let [red, green, blue, alpha_coverage] = fill_color.rgba32();
            let alpha = expand_five(alpha_coverage & 0x1f);
            Rgba8::new(red, green, blue, alpha)
        }
    }
}

/// Why an RDP fill-cycle `FillRectangle` could not be executed against a
/// color target. Every variant is a loud rejection, per AGENTS.md "loud
/// traps, no silent shrugs" -- none of them mutate the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FillExecutionError {
    NotFillCycle,
    UnsafeFillCycleBypass { hazards: FillCycleBypassHazards },
    Coordinate(FillCoordinateError),
    Target(TargetError),
    MissingResidentBytes { key: ColorTargetKey },
    /// The rectangle survived coordinate resolution but nothing of it
    /// survived the intersection with the scissor and the target extent.
    ///
    /// A loud refusal rather than a silent no-op, for the same reason
    /// [`super::TexrectExecutionError::ScissoredAway`] is one: an empty
    /// result is either a genuinely off-screen primitive or a reversed or
    /// degenerate scissor, and the two are worth telling apart. Both the
    /// rectangle and the scissor are carried so a reader can.
    ScissoredAway {
        key: ColorTargetKey,
        rectangle: FillPixelRectangle,
        scissor: RdpScissorRect,
    },
}

impl core::fmt::Display for FillExecutionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotFillCycle => {
                write!(formatter, "execute_fill_rectangle requires CycleType::Fill")
            }
            Self::UnsafeFillCycleBypass { hazards } => write!(
                formatter,
                "FillRectangle in Fill cycle retains unsafe {hazards} state; the public fill \
                 contract requires G_RM_NOOP/G_RM_NOOP2 (a depth read can hang the RDP)"
            ),
            Self::Coordinate(error) => write!(formatter, "{error}"),
            Self::Target(error) => write!(formatter, "{error}"),
            Self::MissingResidentBytes { key } => write!(
                formatter,
                "execute_fill_rectangle requires resident_bytes for already-resident target \
                 {key:?}; treating a resident candidate as if it had no prior content would \
                 silently discard everything outside the claimed rectangle"
            ),
            Self::ScissoredAway {
                key,
                rectangle,
                scissor,
            } => write!(
                formatter,
                "FillRectangle {rectangle:?} against target {key:?} is empty after clipping to \
                 scissor {scissor:?} and the target extent"
            ),
        }
    }
}

impl std::error::Error for FillExecutionError {}

impl From<FillCoordinateError> for FillExecutionError {
    fn from(error: FillCoordinateError) -> Self {
        Self::Coordinate(error)
    }
}

impl From<TargetError> for FillExecutionError {
    fn from(error: TargetError) -> Self {
        Self::Target(error)
    }
}

/// Executes one decoded fill-cycle `FillRectangle` against `candidate`,
/// producing a [`CompletedColorTargetWrite`] the caller admits via
/// [`CandidateColorTarget::admit_completed_initialization`]. `resident_bytes`
/// is the target's current full-extent device bytes for a resident
/// (`predecessor.is_some()`) candidate -- required in that case
/// ([`FillExecutionError::MissingResidentBytes`] rejects `None` rather than
/// assuming the untouched rows are zero, which would silently discard real
/// content). `None` is only valid for a brand-new target, which this
/// executor still only accepts a full-extent rectangle for (the admission
/// function enforces that; see `targets/mod.rs`
/// `PartialNewTargetInitialization`).
///
/// No combiner, triangle, texture, or depth behavior; Fill cycle only. Not a
/// GPU/wgpu call: this is a CPU-side executor writing the same
/// [`DeviceColorBytes`] domain the M3.3c GPU raster path also produces, and
/// they compose at the identical `CompletedColorTargetWrite`/
/// `admit_completed_initialization` seam by construction. No performance or
/// pixel-shader parity is claimed.
///
/// `rectangle` is the real `crate::FillRectangle` decode payload (as
/// produced by `crate::raw_dpc::decode_raw_dpc`) -- this is the first
/// production (non-test) caller of that decode payload; `raw_dpc::plan_fill`
/// only ever produced a resource-journal entry for it, never executed it
/// against a target (see module doc).
pub fn execute_fill_rectangle(
    candidate: &CandidateColorTarget,
    other_mode: OtherMode,
    fill_color: FillColor,
    rectangle: crate::FillRectangle,
    scissor: RdpScissorRect,
    resident_bytes: Option<&[u8]>,
) -> Result<CompletedColorTargetWrite, FillExecutionError> {
    if !matches!(other_mode.cycle_type(), CycleType::Fill) {
        return Err(FillExecutionError::NotFillCycle);
    }
    require_safe_fill_cycle_bypass(other_mode)?;

    let pixel_rect = resolve_fill_pixel_rectangle(
        rectangle.upper_left_x(),
        rectangle.upper_left_y(),
        rectangle.lower_right_x(),
        rectangle.lower_right_y(),
    )?;
    let key = candidate.key();
    let clipped = clip_fill_rectangle(pixel_rect, scissor, key)?;
    let rectangle = TargetRectangle::try_new(
        clipped.x0(),
        clipped.y0(),
        clipped.width(),
        clipped.height(),
    )?;
    let plan = candidate.plan_rows(rectangle)?;

    let format = key.format();
    let extent = key.extent();
    let bytes_per_pixel = format.bytes_per_pixel() as usize;
    let full_len = (extent.pixels() as usize)
        .checked_mul(bytes_per_pixel)
        .ok_or(TargetError::PixelBufferLengthOverflow {
            pixels: extent.pixels() as usize,
            bytes_per_pixel: format.bytes_per_pixel(),
        })?;

    let mut bytes = match resident_bytes {
        Some(existing) => {
            if existing.len() != full_len {
                return Err(TargetError::CompletedByteLengthMismatch {
                    key,
                    generation: candidate.generation(),
                    expected: full_len,
                    actual: existing.len(),
                }
                .into());
            }
            existing.to_vec()
        }
        None if candidate.predecessor().is_some() => {
            // A resident candidate's untouched rows must come from its prior
            // generation's real content, never from an assumed-zero buffer:
            // treating "no resident_bytes given" as "the rest of the target
            // is zero" would silently discard every row outside this fill's
            // claimed rectangle the instant a caller forgets to thread the
            // prior generation's bytes through. Loud trap, not a fallback.
            return Err(FillExecutionError::MissingResidentBytes { key });
        }
        None => vec![0u8; full_len],
    };

    for row in plan.rows() {
        let target_x0 = row.first_pixel() % extent.width();
        for column in 0..row.pixel_count() {
            let target_x = target_x0 + column;
            let pixel = decode_fill_cycle_pixel(fill_color, format, target_x);
            let byte_offset = (row.first_pixel() as usize + column as usize) * bytes_per_pixel;
            write_pixel(
                format,
                &mut bytes[byte_offset..byte_offset + bytes_per_pixel],
                pixel,
            );
        }
    }

    let device_bytes = DeviceColorBytes::new_for_fill(key, candidate.generation(), format, bytes)?;

    Ok(CompletedColorTargetWrite::new_for_fill(
        key,
        candidate.generation(),
        key.range(),
        rectangle,
        device_bytes,
    ))
}

fn write_pixel(format: ColorTargetFormat, dest: &mut [u8], pixel: Rgba8) {
    match format {
        ColorTargetFormat::Rgba16 => {
            let packed = (u16::from(pixel.red >> 3) << 11)
                | (u16::from(pixel.green >> 3) << 6)
                | (u16::from(pixel.blue >> 3) << 1)
                | u16::from(pixel.alpha >> 7);
            dest.copy_from_slice(&packed.to_be_bytes());
        }
        ColorTargetFormat::Rgba32 => {
            dest.copy_from_slice(&[pixel.red, pixel.green, pixel.blue, pixel.alpha]);
        }
    }
}

#[cfg(test)]
mod tests;
