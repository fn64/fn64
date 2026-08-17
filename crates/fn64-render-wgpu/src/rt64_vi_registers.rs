//! RT64's Video Interface register field layout and its register-read masks.
//!
//! Ported from the Rust-port authority pin
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/rt64-port-authority.json`'s `port_source.commit`). Both cited files
//! are `port_delta: unchanged` with identical `oracle.sha256`, so the citation
//! is unambiguous against either pin.
//!
//! ## Cited sources and their digests
//!
//! | file | whole-file SHA-256 | lines | ported |
//! |---|---|---:|---|
//! | `src/hle/rt64_vi.h` | `483f62fa9f771adbb7e1631cbe6f5a61b185277b6c21823d217961593a3c6dce` | 172 | partial (~90/172) |
//! | `src/hle/rt64_application.cpp` | `5bbb4960a262c91b1ccc383ed0027b25c60be839f7d38ed0f205a9fcd0341f84` | 756 | partial (~16/756) |
//!
//! Each digest was recomputed with `shasum -a 256` against the pinned
//! checkout and cross-checked against `docs/rt64-port-inventory.json`'s
//! `sources.port.sha256` for the same path; `sources.oracle.sha256` matches
//! byte for byte in both cases.
//!
//! ## Inventory drift, per file
//!
//! Both entries read `not-started` before this module and will read `ported`
//! after it, because `port_state` is derived at whole-file granularity from a
//! verbatim digest match. **Neither file is fully ported.**
//!
//! - `rt64_vi.h` -- **partial, ~90 of 172 lines.** Ported: the four
//!   `VI_STATUS_TYPE_*` and four `VI_STATUS_AA_MODE_*` constants
//!   (`:13-21`) and the eight register bitfield unions `Status`, `Burst`,
//!   `HSync`, `Leap`, `HRegion`, `VRegion`, `XTransform`, `YTransform`
//!   (`:25-124`). Not ported: the `VI` struct's member declarations
//!   (`:126-139`), the thirteen accessor *declarations* whose bodies live in
//!   `rt64_vi.cpp` (`:141-153`), and all of `VIHistory` (`:156-172`).
//! - `rt64_application.cpp` -- **partial, ~16 of 756 lines.** Ported:
//!   `Application::Core::decodeVI`'s register-read masks (`:45-61`). Nothing
//!   else; see the refusal boundary below.
//!
//! The inventory is deliberately **not** regenerated here (the standing brief
//! §8): a concurrent lane owns `docs/rt64-port-inventory.json`, and a separate
//! `docs: regenerate inventory for ...` commit is the only writer.
//!
//! ## Ported / refused boundary, and the criterion
//!
//! The standing criterion: *a construct is ported when its behavior is fully
//! determined by values and control flow present in the cited file -- no GPU,
//! no ImGui context, no type from an uncited file.*
//!
//! Ported, because a register word plus a bit range fully determines the
//! answer: the field extents, the constant values, and `decodeVI`'s masks.
//!
//! Refused:
//!
//! - **`VI`'s accessor bodies** -- `fbSiz`, `fbAddress`, `fbSize`,
//!   `xScaleFloat`, `xOffsetFloat`, `yScaleFloat`, `yOffsetFloat`,
//!   `viewRectangle`, `cropRectangle`, `gamma`, `compatibleWith`, `visible`,
//!   `operator!=` are declared in `rt64_vi.h` but *defined* in
//!   `src/hle/rt64_vi.cpp`, which `docs/rt64-port-inventory.json` records as
//!   **`authority-gated`** (it carries the `vi-retrace-cadence:v1` source
//!   overlay). An authority-gated file is not this card's to port. Its bodies
//!   are quoted below only as the evidence for a reported disagreement, and
//!   `rt64_vi.cpp` is deliberately **not** cited by digest anywhere in this
//!   module.
//! - **`VIHistory`** -- a three-entry ring buffer plus a `logicalRateFromFactors`
//!   whose `FullRate = 60` carries an in-source `// TODO: PAL support.`
//!   (`rt64_vi.cpp:166`). Same authority-gated file.
//! - **`rt64_application.cpp`'s other ~740 lines** -- see below.
//!
//! ## Verbatim key structure
//!
//! `src/hle/rt64_vi.h:25-46`, the `Status` union, which fixes every other
//! decode in this module:
//!
//! ```text
//! union Status {
//!     struct {
//!         // Refer to VI_STATUS_TYPE_ values.
//!         unsigned type : 2;
//!         unsigned gammaDitherEnable : 1;
//!         // Use linear color space if disabled.
//!         unsigned gammaEnable : 1;
//!         unsigned divotEnable : 1;
//!         unsigned vbusClockEnable : 1;
//!         // Always on if interlaced.
//!         unsigned serrate : 1;
//!         unsigned testMode : 1;
//!         unsigned aaMode : 2;
//!         unsigned reserved : 1;
//!         unsigned diagnostics : 1;
//!         unsigned pixelAdvance : 4;
//!         unsigned ditherFilter : 1;
//!         unsigned padding : 15;
//!     };
//!
//!     unsigned word;
//! };
//! ```
//!
//! and `src/hle/rt64_application.cpp:45-61`:
//!
//! ```text
//! VI Application::Core::decodeVI() const {
//!     VI vi;
//!     vi.status.word = *VI_STATUS_REG;
//!     vi.origin = (*VI_ORIGIN_REG) & 0xFFFFFFU;
//!     vi.width = (*VI_WIDTH_REG) & 0xFFFU;
//!     vi.intr = (*VI_INTR_REG) & 0x3FF;
//!     vi.vCurrentLine = (*VI_V_CURRENT_LINE_REG) & 0x3FF;
//!     vi.burst.word = *VI_TIMING_REG;
//!     vi.vSync = (*VI_V_SYNC_REG) & 0x3FF;
//!     ...
//! }
//! ```
//!
//! ## DEVIATION: bit positions are derived, not transcribed from layout
//!
//! C++ bitfield allocation order within a storage unit is
//! implementation-defined (the standing brief §3.8). This module therefore
//! does **not** claim that RT64's `Status` struct *occupies* these bit
//! positions on any given compiler. It claims the LSB-first reading -- which
//! is what every mainstream N64 VI reference and fn64's own public-document
//! derivation use -- and pins the resulting positions as constants. The
//! agreement documented under "Overlap" below is evidence that the LSB-first
//! reading is the intended one, not a proof about C++ layout.
//!
//! ## Overlap with fn64's own types
//!
//! **fn64 already owns this decode**, from public documentation rather than
//! from RT64, in `crates/fn64-render/src/lib.rs`. This module is therefore a
//! **comparison**, not a substitute: nothing here is wired into any pipeline,
//! and `fn64_render::ViFilterControl` remains the production decoder.
//!
//! Every overlapping fact was checked and **agrees**:
//!
//! | fact | RT64 | fn64 | |
//! |---|---|---|---|
//! | pixel type | `type : 2` @0, `VI_STATUS_TYPE_*` 0..3 | `status & 3`, `ViPixelType` 0..3 | agree |
//! | gamma dither | `gammaDitherEnable` @2 | `status & (1 << 2)` | agree |
//! | gamma | `gammaEnable` @3 | `status & (1 << 3)` | agree |
//! | divot | `divotEnable` @4 | `status & (1 << 4)` | agree |
//! | serrate | `serrate` @6 | `status & (1 << 6)` | agree |
//! | AA mode | `aaMode : 2` @8, `VI_STATUS_AA_MODE_*` 0..3 | `(status >> 8) & 3`, `ViAaMode` 0..3 | agree |
//! | dither filter | `ditherFilter` @16 | `status & (1 << 16)` | agree |
//! | origin mask | `& 0xFFFFFFU` | `words[1] & 0x00ff_ffff` | agree |
//! | width mask | `& 0xFFFU` | `words[2] & 0x0fff` | agree |
//! | H/V region fields | `hEnd:10 / pad 6 / hStart:10` | `FIELD_MASK = 0x03ff`, `>> 16` | agree |
//! | X/Y transform fields | `xScale:12 / pad 4 / xOffset:12` | `FIELD_MASK = 0x0fff`, `>> 16` | agree |
//!
//! The four constant families line up name for name:
//! `VI_STATUS_TYPE_BLANK/RESERVED/16_BIT/32_BIT` onto
//! `ViPixelType::Blank/Reserved/Rgba16/Rgba32`, and
//! `VI_STATUS_AA_MODE_RESAMP_ALWAYS_FETCH/FETCH_IF_NEEDED/RESAMP_ONLY/NONE`
//! onto `ViAaMode::AaResampleAlways/AaResampleWhenNeeded/ResampleOnly/Replicate`.
//!
//! ## Open questions
//!
//! Two behavioral differences were found between RT64 and fn64's existing
//! owner. Both are **reported, not resolved**, and neither changes any fn64
//! behavior here. Both live in `rt64_vi.cpp`, which is authority-gated, so
//! settling them is out of this card's scope.
//!
//! 1. **Scale is a reciprocal in RT64 and a direct step in fn64, and the two
//!    round differently.** `rt64_vi.cpp:127-129` is
//!    `xScaleFloat() { return (1024.0f / xTransform.xScale); }` -- a
//!    *reciprocal* -- and `fbSize` then **divides** by it:
//!    `lround(float(vEnd - vStart) / (2.0f * yScaleFloat() * ...))`
//!    (`rt64_vi.cpp:110`). fn64 instead keeps the raw U2.10 field as a
//!    **step** and multiplies:
//!    `row * ViScaleAxis::step_u2_10() >> ViScaleAxis::FRACTION_BITS`
//!    (`crates/fn64-render/src/vi_source.rs:86-90`). The two are
//!    algebraically identical -- both compute `(vEnd - vStart) * step / 2048`
//!    -- but RT64 rounds `1024/step` to f32 *first*, so the composed result
//!    differs. Over `step` in `1..4096` and even `vEnd - vStart` in
//!    `2..1024`, the two disagree **after `lround`** on 1,036 pairs; within a
//!    realistic range (`step` 0x100..0x800, span 400..540) 101 pairs still
//!    disagree, e.g. `step = 0x1e0`, `span = 480` gives RT64 **112** rows and
//!    the direct form **113** -- a whole-row footprint difference. All
//!    power-of-two steps (0x200, 0x400, 0x800) agree at every span tested,
//!    which is why the common 1:1 and 2:1 cases never show it. This is the
//!    same shape as the already-recorded `setPrimDepth` finding (RT64
//!    multiplies by an f32 reciprocal where fn64 divides).
//! 2. **`visible()` and `try_from_registers` disagree on a half-programmed
//!    H_START.** RT64's `visible()` is
//!    `(status.type != VI_STATUS_TYPE_BLANK) && (hRegion.hStart > 0)`
//!    (`rt64_vi.cpp:44-46`); fn64's `ViActiveWindow::try_from_registers`
//!    returns `Some` whenever *either* 10-bit subfield of *each* of H and V
//!    is nonzero (`crates/fn64-render/src/lib.rs:379-387`). For
//!    `H_START = 0x2d0` (hStart 0, hEnd 720) RT64 reports not-visible and
//!    fn64 reports programmed. fn64's own doc comment
//!    (`crates/fn64-render/src/lib.rs:376-381`) explains the choice --
//!    register initialization is not atomic -- so this is a documented,
//!    deliberate difference in predicate purpose rather than a defect in
//!    either. Recorded so a future card does not "harmonize" them.
//!
//! ## Reuse, not new type
//!
//! No new vector type: this module is plain `u32` register words and `u8` bit
//! positions, per `AGENTS.md`'s one-vector-type-per-port rule. It deliberately
//! does **not** define a `VI` struct -- `fn64_render::ViScanoutRegisters`
//! already owns the fourteen-word image, and a second one would be exactly the
//! competing duplicate the standing brief §1 warns about.
//!
//! ## Admitted domain
//!
//! Any `u32` register word. Every accessor here is a mask-and-shift over the
//! full 32-bit domain and cannot panic.
//!
//! ## Scope status
//!
//! DONE. The accessor bodies in `rt64_vi.cpp` and all of `VIHistory` are
//! deliberately not ported -- a scope boundary this card chose because that
//! file is `authority-gated`, not work this module is waiting on.
//!
//! ## Nonclaims
//!
//! - Unwired: declared `mod`, not `pub mod`; no production admission.
//! - No behavior change: fn64's existing VI decode in
//!   `crates/fn64-render/src/lib.rs` is untouched and remains authoritative.
//! - No `repr(C)`, size, alignment or ABI claim; in particular no claim about
//!   where a C++ compiler places these bitfields (see DEVIATION above).
//! - No claim that either side of the two reported disagreements is correct
//!   against hardware. Neither was tested against silicon.
//! - No field-declaration-order pin (the standing brief §3.7).

/// `VI_STATUS_TYPE_*` -- `src/hle/rt64_vi.h:13-16`, the two-bit `type` field.
pub(crate) const VI_STATUS_TYPE_BLANK: u32 = 0;
pub(crate) const VI_STATUS_TYPE_RESERVED: u32 = 1;
pub(crate) const VI_STATUS_TYPE_16_BIT: u32 = 2;
pub(crate) const VI_STATUS_TYPE_32_BIT: u32 = 3;

/// `VI_STATUS_AA_MODE_*` -- `src/hle/rt64_vi.h:18-21`, the two-bit `aaMode`
/// field. `RESAMP_ONLY` resamples without fetching extra coverage;
/// `NONE` replicates.
pub(crate) const VI_STATUS_AA_MODE_RESAMP_ALWAYS_FETCH: u32 = 0;
pub(crate) const VI_STATUS_AA_MODE_RESAMP_FETCH_IF_NEEDED: u32 = 1;
pub(crate) const VI_STATUS_AA_MODE_RESAMP_ONLY: u32 = 2;
pub(crate) const VI_STATUS_AA_MODE_NONE: u32 = 3;

/// One contiguous bit range inside a 32-bit VI register word.
///
/// This is the single transcription of RT64's `unsigned name : width`
/// declarations, read LSB-first (see the module DEVIATION). Every field
/// below is one of these, so a wrong offset or width is a one-place error.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ViField {
    /// Bit offset of the field's least significant bit.
    pub(crate) offset: u32,
    /// Field width in bits.
    pub(crate) width: u32,
}

impl ViField {
    const fn new(offset: u32, width: u32) -> Self {
        Self { offset, width }
    }

    /// The field's mask *in place*, i.e. already shifted to `offset`.
    ///
    /// Note the return convention: this is the masked-in-place form, and
    /// [`Self::get`] is the shifted-down form. The two are deliberately
    /// separate methods rather than one ambiguous accessor.
    pub(crate) const fn mask_in_place(self) -> u32 {
        self.low_mask() << self.offset
    }

    /// The field's mask at bit zero, before shifting into place.
    pub(crate) const fn low_mask(self) -> u32 {
        // `width` is never 32 for any VI field, so the shift cannot overflow;
        // written as a checked expression anyway rather than `(1 << w) - 1`.
        ((1u64 << self.width) - 1) as u32
    }

    /// Extract the field, **shifted down** to bit zero.
    pub(crate) const fn get(self, word: u32) -> u32 {
        (word >> self.offset) & self.low_mask()
    }

    /// Extract a one-bit field as a `bool`.
    pub(crate) const fn get_bool(self, word: u32) -> bool {
        self.get(word) != 0
    }

    /// One past the field's most significant bit.
    pub(crate) const fn end(self) -> u32 {
        self.offset + self.width
    }
}

/// `VI::Status` -- `src/hle/rt64_vi.h:25-46`.
pub(crate) mod status {
    use super::ViField;

    pub(crate) const TYPE: ViField = ViField::new(0, 2);
    pub(crate) const GAMMA_DITHER_ENABLE: ViField = ViField::new(2, 1);
    pub(crate) const GAMMA_ENABLE: ViField = ViField::new(3, 1);
    pub(crate) const DIVOT_ENABLE: ViField = ViField::new(4, 1);
    pub(crate) const VBUS_CLOCK_ENABLE: ViField = ViField::new(5, 1);
    /// "Always on if interlaced." -- `rt64_vi.h:34`.
    pub(crate) const SERRATE: ViField = ViField::new(6, 1);
    pub(crate) const TEST_MODE: ViField = ViField::new(7, 1);
    pub(crate) const AA_MODE: ViField = ViField::new(8, 2);
    pub(crate) const RESERVED: ViField = ViField::new(10, 1);
    pub(crate) const DIAGNOSTICS: ViField = ViField::new(11, 1);
    pub(crate) const PIXEL_ADVANCE: ViField = ViField::new(12, 4);
    pub(crate) const DITHER_FILTER: ViField = ViField::new(16, 1);
    pub(crate) const PADDING: ViField = ViField::new(17, 15);

    /// Declaration order, which for `Status` is also numeric order.
    pub(crate) const DECLARATION_ORDER: [ViField; 13] = [
        TYPE,
        GAMMA_DITHER_ENABLE,
        GAMMA_ENABLE,
        DIVOT_ENABLE,
        VBUS_CLOCK_ENABLE,
        SERRATE,
        TEST_MODE,
        AA_MODE,
        RESERVED,
        DIAGNOSTICS,
        PIXEL_ADVANCE,
        DITHER_FILTER,
        PADDING,
    ];
}

/// `VI::Burst` -- `src/hle/rt64_vi.h:48-58`. Used for both `burst` and
/// `vBurst` (`rt64_vi.h:131,137`).
pub(crate) mod burst {
    use super::ViField;

    pub(crate) const H_SYNC_WIDTH: ViField = ViField::new(0, 8);
    pub(crate) const COLOR_WIDTH: ViField = ViField::new(8, 8);
    pub(crate) const V_SYNC_WIDTH: ViField = ViField::new(16, 4);
    pub(crate) const COLOR_START: ViField = ViField::new(20, 10);
    pub(crate) const PADDING: ViField = ViField::new(30, 2);

    pub(crate) const DECLARATION_ORDER: [ViField; 5] = [
        H_SYNC_WIDTH,
        COLOR_WIDTH,
        V_SYNC_WIDTH,
        COLOR_START,
        PADDING,
    ];
}

/// `VI::HSync` -- `src/hle/rt64_vi.h:60-69`. Note the 4-bit gap between
/// `hSync` and `leap`, which is a declared `padding0` and not an accident.
pub(crate) mod h_sync {
    use super::ViField;

    pub(crate) const H_SYNC: ViField = ViField::new(0, 12);
    pub(crate) const PADDING0: ViField = ViField::new(12, 4);
    pub(crate) const LEAP: ViField = ViField::new(16, 5);
    pub(crate) const PADDING1: ViField = ViField::new(21, 11);

    pub(crate) const DECLARATION_ORDER: [ViField; 4] = [H_SYNC, PADDING0, LEAP, PADDING1];
}

/// `VI::Leap` -- `src/hle/rt64_vi.h:71-80`. `leapB` occupies the *low* half
/// and `leapA` the high half, which is the opposite of the alphabetical
/// reading; pinned deliberately.
pub(crate) mod leap {
    use super::ViField;

    pub(crate) const LEAP_B: ViField = ViField::new(0, 12);
    pub(crate) const PADDING0: ViField = ViField::new(12, 4);
    pub(crate) const LEAP_A: ViField = ViField::new(16, 12);
    pub(crate) const PADDING1: ViField = ViField::new(28, 4);

    pub(crate) const DECLARATION_ORDER: [ViField; 4] = [LEAP_B, PADDING0, LEAP_A, PADDING1];
}

/// `VI::HRegion` -- `src/hle/rt64_vi.h:82-91`. `hEnd` is the **low** half and
/// `hStart` the high half.
pub(crate) mod h_region {
    use super::ViField;

    pub(crate) const H_END: ViField = ViField::new(0, 10);
    pub(crate) const PADDING0: ViField = ViField::new(10, 6);
    pub(crate) const H_START: ViField = ViField::new(16, 10);
    pub(crate) const PADDING1: ViField = ViField::new(26, 6);

    pub(crate) const DECLARATION_ORDER: [ViField; 4] = [H_END, PADDING0, H_START, PADDING1];
}

/// `VI::VRegion` -- `src/hle/rt64_vi.h:93-102`, the same shape as
/// [`h_region`] with `vEnd` low and `vStart` high. Vertical values are
/// half-lines.
pub(crate) mod v_region {
    use super::ViField;

    pub(crate) const V_END: ViField = ViField::new(0, 10);
    pub(crate) const PADDING0: ViField = ViField::new(10, 6);
    pub(crate) const V_START: ViField = ViField::new(16, 10);
    pub(crate) const PADDING1: ViField = ViField::new(26, 6);

    pub(crate) const DECLARATION_ORDER: [ViField; 4] = [V_END, PADDING0, V_START, PADDING1];
}

/// `VI::XTransform` -- `src/hle/rt64_vi.h:104-113`. Twelve-bit scale low,
/// twelve-bit offset high. The scale field is U2.10 fixed point.
pub(crate) mod x_transform {
    use super::ViField;

    pub(crate) const X_SCALE: ViField = ViField::new(0, 12);
    pub(crate) const PADDING0: ViField = ViField::new(12, 4);
    pub(crate) const X_OFFSET: ViField = ViField::new(16, 12);
    pub(crate) const PADDING1: ViField = ViField::new(28, 4);

    pub(crate) const DECLARATION_ORDER: [ViField; 4] = [X_SCALE, PADDING0, X_OFFSET, PADDING1];
}

/// `VI::YTransform` -- `src/hle/rt64_vi.h:115-124`, identical shape to
/// [`x_transform`].
pub(crate) mod y_transform {
    use super::ViField;

    pub(crate) const Y_SCALE: ViField = ViField::new(0, 12);
    pub(crate) const PADDING0: ViField = ViField::new(12, 4);
    pub(crate) const Y_OFFSET: ViField = ViField::new(16, 12);
    pub(crate) const PADDING1: ViField = ViField::new(28, 4);

    pub(crate) const DECLARATION_ORDER: [ViField; 4] = [Y_SCALE, PADDING0, Y_OFFSET, PADDING1];
}

/// `Application::Core::decodeVI`'s masks --
/// `src/hle/rt64_application.cpp:45-61`.
///
/// `decodeVI` reads each MMIO register and masks four of them; the other ten
/// are stored as whole words because their bitfield unions already describe
/// every occupied bit. These are the four that are masked on read.
pub(crate) mod decode_masks {
    /// `vi.origin = (*VI_ORIGIN_REG) & 0xFFFFFFU;` -- 24-bit RDRAM address.
    pub(crate) const ORIGIN: u32 = 0xFF_FFFF;
    /// `vi.width = (*VI_WIDTH_REG) & 0xFFFU;` -- 12-bit stride in pixels.
    pub(crate) const WIDTH: u32 = 0xFFF;
    /// `vi.intr = (*VI_INTR_REG) & 0x3FF;` -- 10-bit interrupt half-line.
    pub(crate) const INTR: u32 = 0x3FF;
    /// `vi.vCurrentLine = (*VI_V_CURRENT_LINE_REG) & 0x3FF;`
    pub(crate) const V_CURRENT_LINE: u32 = 0x3FF;
    /// `vi.vSync = (*VI_V_SYNC_REG) & 0x3FF;`
    pub(crate) const V_SYNC: u32 = 0x3FF;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every union in `rt64_vi.h` declares fields that tile a 32-bit word
    /// with no gap and no overlap -- the `padding` members exist exactly so
    /// that this holds. Checking it catches a wrong width or offset in any
    /// single field, because a one-field error breaks the tiling.
    fn assert_tiles_word(fields: &[ViField], name: &str) {
        let mut next = 0u32;
        let mut union_mask = 0u32;
        for (index, field) in fields.iter().enumerate() {
            assert_eq!(
                field.offset, next,
                "{name} field {index} starts at {} but the previous field ended at {next}",
                field.offset
            );
            assert!(field.width > 0, "{name} field {index} has zero width");
            assert_eq!(
                union_mask & field.mask_in_place(),
                0,
                "{name} field {index} overlaps an earlier field"
            );
            union_mask |= field.mask_in_place();
            next = field.end();
        }
        assert_eq!(next, 32, "{name} tiles {next} bits, not 32");
        // Second, independent derivation of the same fact: the union of every
        // in-place mask must be exactly the full word.
        assert_eq!(
            union_mask,
            u32::MAX,
            "{name} masks do not union to the full word"
        );
    }

    #[test]
    fn every_vi_union_tiles_thirty_two_bits_exactly() {
        assert_tiles_word(&status::DECLARATION_ORDER, "Status");
        assert_tiles_word(&burst::DECLARATION_ORDER, "Burst");
        assert_tiles_word(&h_sync::DECLARATION_ORDER, "HSync");
        assert_tiles_word(&leap::DECLARATION_ORDER, "Leap");
        assert_tiles_word(&h_region::DECLARATION_ORDER, "HRegion");
        assert_tiles_word(&v_region::DECLARATION_ORDER, "VRegion");
        assert_tiles_word(&x_transform::DECLARATION_ORDER, "XTransform");
        assert_tiles_word(&y_transform::DECLARATION_ORDER, "YTransform");
    }

    /// The widths, transcribed as a flat list straight from the C++ source
    /// text. This is the second independent statement of the same layout: the
    /// tiling test above proves the offsets are consistent with *some* widths,
    /// and this proves the widths are the ones RT64 wrote.
    #[test]
    fn declared_widths_match_the_source_text() {
        assert_eq!(
            status::DECLARATION_ORDER.map(|f| f.width),
            [2, 1, 1, 1, 1, 1, 1, 2, 1, 1, 4, 1, 15]
        );
        assert_eq!(burst::DECLARATION_ORDER.map(|f| f.width), [8, 8, 4, 10, 2]);
        assert_eq!(h_sync::DECLARATION_ORDER.map(|f| f.width), [12, 4, 5, 11]);
        assert_eq!(leap::DECLARATION_ORDER.map(|f| f.width), [12, 4, 12, 4]);
        assert_eq!(h_region::DECLARATION_ORDER.map(|f| f.width), [10, 6, 10, 6]);
        assert_eq!(v_region::DECLARATION_ORDER.map(|f| f.width), [10, 6, 10, 6]);
        assert_eq!(
            x_transform::DECLARATION_ORDER.map(|f| f.width),
            [12, 4, 12, 4]
        );
        assert_eq!(
            y_transform::DECLARATION_ORDER.map(|f| f.width),
            [12, 4, 12, 4]
        );
    }

    /// The two mask conventions are different values and must not be
    /// substituted for one another. This is the `rgbDither` trap: an
    /// accessor's return convention is part of its contract.
    #[test]
    fn masked_in_place_and_shifted_down_are_distinct_conventions() {
        let aa = status::AA_MODE;
        assert_eq!(aa.low_mask(), 0b11);
        assert_eq!(aa.mask_in_place(), 0b11 << 8);
        assert_ne!(aa.low_mask(), aa.mask_in_place());

        // A word with aaMode = 3 and nothing else set.
        let word = VI_STATUS_AA_MODE_NONE << 8;
        assert_eq!(aa.get(word), VI_STATUS_AA_MODE_NONE);
        assert_eq!(word & aa.mask_in_place(), 0b11 << 8);
        // Substituting the in-place mask for the accessor would give 768, not 3.
        assert_ne!(word & aa.mask_in_place(), aa.get(word));

        // A field already at offset zero is the one case where the two agree,
        // which is exactly why the trap is easy to miss.
        let ty = status::TYPE;
        assert_eq!(ty.low_mask(), ty.mask_in_place());
    }

    /// `get_bool` is the one-bit convenience over [`ViField::get`]; exercise
    /// it directly so it cannot drift from the accessor it wraps.
    #[test]
    fn get_bool_reports_a_one_bit_field_set_and_clear() {
        for field in [
            status::GAMMA_DITHER_ENABLE,
            status::GAMMA_ENABLE,
            status::DIVOT_ENABLE,
            status::SERRATE,
            status::DITHER_FILTER,
        ] {
            assert_eq!(field.width, 1);
            assert!(field.get_bool(field.mask_in_place()));
            assert!(!field.get_bool(!field.mask_in_place()));
            assert!(!field.get_bool(0));
            assert!(field.get_bool(u32::MAX));
            // Agrees with the accessor it wraps, both ways round.
            assert_eq!(field.get_bool(u32::MAX), field.get(u32::MAX) != 0);
            assert_eq!(field.get_bool(0), field.get(0) != 0);
        }

        // A multi-bit field is "set" when any of its bits is, not only when
        // all of them are.
        let aa = status::AA_MODE;
        assert!(aa.get_bool(VI_STATUS_AA_MODE_RESAMP_FETCH_IF_NEEDED << aa.offset));
        assert!(!aa.get_bool(VI_STATUS_AA_MODE_RESAMP_ALWAYS_FETCH << aa.offset));
    }

    #[test]
    fn status_constant_families_are_exhaustive_and_ordered() {
        // Both families are complete 0..=3 runs over a two-bit field.
        let types = [
            VI_STATUS_TYPE_BLANK,
            VI_STATUS_TYPE_RESERVED,
            VI_STATUS_TYPE_16_BIT,
            VI_STATUS_TYPE_32_BIT,
        ];
        assert_eq!(types, [0, 1, 2, 3]);
        let aa_modes = [
            VI_STATUS_AA_MODE_RESAMP_ALWAYS_FETCH,
            VI_STATUS_AA_MODE_RESAMP_FETCH_IF_NEEDED,
            VI_STATUS_AA_MODE_RESAMP_ONLY,
            VI_STATUS_AA_MODE_NONE,
        ];
        assert_eq!(aa_modes, [0, 1, 2, 3]);
        // Derived a second way: each family must exactly exhaust its field.
        assert_eq!(types.len() as u32, status::TYPE.low_mask() + 1);
        assert_eq!(aa_modes.len() as u32, status::AA_MODE.low_mask() + 1);
    }

    /// Pins the ordering irregularity the standing brief §3.5 warns about:
    /// in `HRegion`, `VRegion`, `Leap`, `XTransform` and `YTransform` the
    /// *end* / *B* / *scale* member is declared first and occupies the LOW
    /// half, while the *start* / *A* / *offset* member occupies the HIGH
    /// half. Reading the names alphabetically gives the wrong answer.
    #[test]
    fn end_and_scale_members_occupy_the_low_half() {
        assert_eq!(h_region::H_END.offset, 0);
        assert_eq!(h_region::H_START.offset, 16);
        assert_eq!(v_region::V_END.offset, 0);
        assert_eq!(v_region::V_START.offset, 16);
        assert_eq!(leap::LEAP_B.offset, 0);
        assert_eq!(leap::LEAP_A.offset, 16);
        assert_eq!(x_transform::X_SCALE.offset, 0);
        assert_eq!(x_transform::X_OFFSET.offset, 16);
        assert_eq!(y_transform::Y_SCALE.offset, 0);
        assert_eq!(y_transform::Y_OFFSET.offset, 16);
    }

    /// `HSync` is the one union whose high member is NOT 12 bits at offset 16:
    /// `leap` is five bits at 16, leaving an 11-bit tail. Pinned so it cannot
    /// be "smoothed" into the shape of its four neighbours.
    #[test]
    fn h_sync_leap_is_five_bits_not_twelve() {
        assert_eq!(h_sync::LEAP.width, 5);
        assert_eq!(h_sync::LEAP.offset, 16);
        assert_ne!(h_sync::LEAP.width, leap::LEAP_A.width);
        assert_eq!(h_sync::PADDING1.width, 11);
    }

    /// `Burst`'s fields are the only ones that are not 10/12-bit halves:
    /// two bytes, then a nibble, then ten bits. Pinned for the same reason.
    #[test]
    fn burst_is_byte_byte_nibble_then_ten_bits() {
        assert_eq!(burst::H_SYNC_WIDTH.width, 8);
        assert_eq!(burst::COLOR_WIDTH.width, 8);
        assert_eq!(burst::V_SYNC_WIDTH.width, 4);
        assert_eq!(burst::COLOR_START.width, 10);
        assert_eq!(burst::COLOR_START.offset, 20);
    }

    #[test]
    fn decode_masks_agree_with_their_field_widths() {
        // Literal, then derivation, then reconcile -- the standing brief §3.2.
        assert_eq!(decode_masks::ORIGIN, 0xFF_FFFF);
        assert_eq!(decode_masks::ORIGIN, (1u32 << 24) - 1);
        assert_eq!(decode_masks::WIDTH, 0xFFF);
        assert_eq!(decode_masks::WIDTH, (1u32 << 12) - 1);
        assert_eq!(decode_masks::INTR, 0x3FF);
        assert_eq!(decode_masks::INTR, (1u32 << 10) - 1);
        assert_eq!(decode_masks::V_CURRENT_LINE, decode_masks::INTR);
        assert_eq!(decode_masks::V_SYNC, decode_masks::INTR);

        // The 10-bit masks are the same width as the H/V region subfields,
        // and the 12-bit width mask matches the transform scale field.
        assert_eq!(decode_masks::INTR, h_region::H_END.low_mask());
        assert_eq!(decode_masks::INTR, v_region::V_START.low_mask());
        assert_eq!(decode_masks::WIDTH, x_transform::X_SCALE.low_mask());
    }

    /// `decodeVI` masks exactly five registers and stores the rest whole.
    /// Recorded as a set so a future edit that masks a sixth is visible.
    #[test]
    fn decode_vi_masks_exactly_the_five_unbitfielded_registers() {
        let masked = [
            decode_masks::ORIGIN,
            decode_masks::WIDTH,
            decode_masks::INTR,
            decode_masks::V_CURRENT_LINE,
            decode_masks::V_SYNC,
        ];
        assert_eq!(masked.len(), 5);
        // None of the five is a full word: each genuinely discards bits.
        for mask in masked {
            assert_ne!(mask, u32::MAX);
        }
    }

    /// Cross-check against fn64's independently-derived owner in
    /// `crates/fn64-render`. This is the comparison this card exists to make;
    /// a disagreement here is a real finding, not a test bug.
    #[test]
    fn rt64_status_layout_agrees_with_fn64_render_vi_filter_control() {
        use fn64_render::{ViAaMode, ViFilterControl, ViPixelType};

        // Pixel type: RT64's constants against fn64's enum, all four values.
        for (raw, expected) in [
            (VI_STATUS_TYPE_BLANK, ViPixelType::Blank),
            (VI_STATUS_TYPE_RESERVED, ViPixelType::Reserved),
            (VI_STATUS_TYPE_16_BIT, ViPixelType::Rgba16),
            (VI_STATUS_TYPE_32_BIT, ViPixelType::Rgba32),
        ] {
            let word = raw << status::TYPE.offset;
            assert_eq!(ViFilterControl::from_status(word).pixel_type, expected);
            assert_eq!(status::TYPE.get(word), raw);
        }

        // AA mode: RT64's four constants against fn64's four variants.
        for (raw, expected) in [
            (
                VI_STATUS_AA_MODE_RESAMP_ALWAYS_FETCH,
                ViAaMode::AaResampleAlways,
            ),
            (
                VI_STATUS_AA_MODE_RESAMP_FETCH_IF_NEEDED,
                ViAaMode::AaResampleWhenNeeded,
            ),
            (VI_STATUS_AA_MODE_RESAMP_ONLY, ViAaMode::ResampleOnly),
            (VI_STATUS_AA_MODE_NONE, ViAaMode::Replicate),
        ] {
            let word = raw << status::AA_MODE.offset;
            assert_eq!(ViFilterControl::from_status(word).antialias_mode, expected);
            assert_eq!(status::AA_MODE.get(word), raw);
        }

        // The four boolean filter bits, each proved to be read from exactly
        // the bit RT64 declares it at: setting only that bit turns exactly
        // that flag on.
        let cases: [(ViField, fn(&ViFilterControl) -> bool); 4] = [
            (status::GAMMA_DITHER_ENABLE, |f| f.gamma_dither),
            (status::GAMMA_ENABLE, |f| f.gamma),
            (status::DIVOT_ENABLE, |f| f.divot),
            (status::DITHER_FILTER, |f| f.dither_filter),
        ];
        for (field, read) in cases {
            let set = ViFilterControl::from_status(field.mask_in_place());
            assert!(
                read(&set),
                "fn64 did not observe the bit at {}",
                field.offset
            );
            let clear = ViFilterControl::from_status(!field.mask_in_place());
            assert!(
                !read(&clear),
                "fn64 still observed the bit at {} when it was the only one clear",
                field.offset
            );
        }
    }

    /// RT64's `serrate` bit and fn64's interlace test are the same bit.
    #[test]
    fn rt64_serrate_is_the_bit_fn64_reads_for_interlace() {
        use fn64_render::{ViResampleControl, ViScanoutField};

        assert_eq!(status::SERRATE.mask_in_place(), 1 << 6);

        let progressive = ViResampleControl::from_registers(0, 0, 0, 0);
        assert_eq!(progressive.field, ViScanoutField::Progressive);

        let serrate = status::SERRATE.mask_in_place();
        assert_eq!(
            ViResampleControl::from_registers(0, 0, serrate, 0).field,
            ViScanoutField::InterlacedEven
        );
        assert_eq!(
            ViResampleControl::from_registers(0, 0, serrate, 1).field,
            ViScanoutField::InterlacedOdd
        );
    }

    /// RT64's `XTransform`/`YTransform` field extents against fn64's
    /// `ViScaleAxis::from_register`, over values that occupy the full 12 bits
    /// and would alias if either mask were wrong.
    #[test]
    fn rt64_transform_fields_agree_with_fn64_vi_scale_axis() {
        use fn64_render::ViScaleAxis;

        for (scale, offset) in [(0u32, 0u32), (0xFFF, 0xFFF), (0x400, 0x123), (0x001, 0xABC)] {
            let word = scale | (offset << 16);
            let axis = ViScaleAxis::from_register(word);
            assert_eq!(u32::from(axis.step_u2_10()), x_transform::X_SCALE.get(word));
            assert_eq!(
                u32::from(axis.offset_u2_10()),
                x_transform::X_OFFSET.get(word)
            );
            assert_eq!(u32::from(axis.step_u2_10()), y_transform::Y_SCALE.get(word));
            assert_eq!(
                u32::from(axis.offset_u2_10()),
                y_transform::Y_OFFSET.get(word)
            );
        }

        // The padding nibbles are genuinely discarded by both sides: setting
        // every padding bit changes neither decode.
        let padding = x_transform::PADDING0.mask_in_place() | x_transform::PADDING1.mask_in_place();
        let axis = ViScaleAxis::from_register(padding);
        assert_eq!(axis.step_u2_10(), 0);
        assert_eq!(axis.offset_u2_10(), 0);

        // fn64's 1:1 scale is RT64's xScale = 1024, which is the value whose
        // reciprocal `xScaleFloat()` returns as exactly 1.0.
        assert_eq!(u32::from(ViScaleAxis::ONE), 1 << 10);
        assert!(u32::from(ViScaleAxis::ONE) <= x_transform::X_SCALE.low_mask());
    }

    /// RT64's `HRegion`/`VRegion` extents against fn64's `ViActiveWindow`.
    #[test]
    fn rt64_region_fields_agree_with_fn64_active_window() {
        use fn64_render::ViActiveWindow;

        // hStart 108, hEnd 748; vStart 34, vEnd 514 -- a plausible NTSC image
        // with both subfields occupying more than eight bits.
        let horizontal = 748 | (108 << 16);
        let vertical = 514 | (34 << 16);
        let window = ViActiveWindow::from_registers(horizontal, vertical);

        // fn64 keeps the subfields private, so compare through the derived
        // extents: RT64's field extraction must reproduce both of them.
        assert_eq!(
            window.output_width(),
            h_region::H_END.get(horizontal) - h_region::H_START.get(horizontal)
        );
        assert_eq!(
            window.output_height(),
            (v_region::V_END.get(vertical) - v_region::V_START.get(vertical)) / 2
        );

        // Round-tripping through fn64's register accessors preserves exactly
        // the bits RT64's unions declare, and nothing in the padding.
        let h_used = h_region::H_END.mask_in_place() | h_region::H_START.mask_in_place();
        let v_used = v_region::V_END.mask_in_place() | v_region::V_START.mask_in_place();
        assert_eq!(window.horizontal_register(), horizontal & h_used);
        assert_eq!(window.vertical_register(), vertical & v_used);

        // The padding sextets are discarded by both sides: setting every
        // padding bit leaves the extents unchanged.
        let padding = h_region::PADDING0.mask_in_place() | h_region::PADDING1.mask_in_place();
        let padded = ViActiveWindow::from_registers(horizontal | padding, vertical | padding);
        assert_eq!(padded.output_width(), window.output_width());
        assert_eq!(padded.output_height(), window.output_height());
    }

    /// The reported disagreement, pinned as a test so it cannot be silently
    /// "fixed" by tidying one form into the other.
    ///
    /// RT64 computes the framebuffer height as
    /// `lround(span / (2 * (1024 / scale)))`, taking the f32 reciprocal
    /// first; fn64's source-footprint walk multiplies by the raw U2.10 step
    /// instead. The two are algebraically equal and **not** equal after
    /// rounding.
    ///
    /// This test claims only that the two forms differ on this witness. It
    /// makes no claim about which matches hardware.
    #[test]
    fn rt64_reciprocal_scale_and_direct_step_disagree_after_rounding() {
        fn rt64_form(span: u32, scale: u32) -> i64 {
            // rt64_vi.cpp:127-129 then :110, in f32 throughout.
            let y_scale_float = 1024.0f32 / (scale as f32);
            let height = (span as f32) / (2.0f32 * y_scale_float);
            height.round() as i64
        }
        fn direct_form(span: u32, scale: u32) -> i64 {
            // The same quantity as span * scale / 2048, in f32.
            let height = ((span as f32) * (scale as f32)) / 2048.0f32;
            height.round() as i64
        }

        // The witness found by exhaustive search over scale 0x100..=0x800 and
        // even spans 400..=540: a whole-row difference.
        assert_eq!(rt64_form(480, 0x1e0), 112);
        assert_eq!(direct_form(480, 0x1e0), 113);
        assert_ne!(rt64_form(480, 0x1e0), direct_form(480, 0x1e0));

        // Power-of-two scales agree, which is why the common cases hide it.
        for scale in [0x200u32, 0x400, 0x800] {
            for span in [474u32, 476, 478, 480, 482] {
                assert_eq!(
                    rt64_form(span, scale),
                    direct_form(span, scale),
                    "power-of-two scale {scale:#x} span {span} unexpectedly disagreed"
                );
            }
        }

        // And the 1:1 case is exact on both.
        assert_eq!(rt64_form(480, 0x400), 240);
        assert_eq!(direct_form(480, 0x400), 240);
    }

    /// RT64's `visible()` predicate and fn64's `try_from_registers` disagree
    /// on a half-programmed H_START. Pinned as a documented difference.
    #[test]
    fn rt64_visible_and_fn64_try_from_registers_disagree_on_zero_h_start() {
        use fn64_render::ViActiveWindow;

        fn rt64_visible(status: u32, h_region_word: u32) -> bool {
            // rt64_vi.cpp:44-46.
            status::TYPE.get(status) != VI_STATUS_TYPE_BLANK
                && h_region::H_START.get(h_region_word) > 0
        }

        let vertical = 514 | (34 << 16);
        let non_blank = VI_STATUS_TYPE_16_BIT << status::TYPE.offset;

        // hStart = 0, hEnd = 720: RT64 says not visible, fn64 says programmed.
        let half_programmed = 720;
        assert!(!rt64_visible(non_blank, half_programmed));
        assert!(ViActiveWindow::try_from_registers(half_programmed, vertical).is_some());

        // Fully programmed: both agree it is a real image.
        let programmed = 748 | (108 << 16);
        assert!(rt64_visible(non_blank, programmed));
        assert!(ViActiveWindow::try_from_registers(programmed, vertical).is_some());

        // Entirely unprogrammed: both agree it is not.
        assert!(!rt64_visible(non_blank, 0));
        assert!(ViActiveWindow::try_from_registers(0, vertical).is_none());

        // A blank pixel type makes RT64 say not-visible regardless of H_START,
        // which fn64's window decode does not consider at all.
        let blank = VI_STATUS_TYPE_BLANK << status::TYPE.offset;
        assert!(!rt64_visible(blank, programmed));
        assert!(ViActiveWindow::try_from_registers(programmed, vertical).is_some());
    }
}
