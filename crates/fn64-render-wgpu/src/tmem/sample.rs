//! Typed point addressing over committed physical TMEM.
//!
//! The public RDP tile sequence shifts an S10.5 coordinate, subtracts the
//! S10.2 tile origin, and applies clamp before mirror/mask addressing. This
//! module performs that integer-only point path and delegates every physical
//! byte, validity, format, and TLUT decision to [`super::read_committed_texel`].
//! First-row parity remains caller-owned: the available authorities do not
//! settle its derivation when load and render tiles differ.

use core::fmt;

use crate::TextureLutMode;

use super::{
    read_committed_texel, AddressedTmemTexel, DecodedPhysicalTexel, PhysicalTexelReadError,
    PhysicalTmemState, TileAddressMode, TileCoordinate, TileDescriptor, TileSize,
    TmemFirstRowParity,
};

const TEXEL_FRACTION_BITS: u32 = 5;
const TEXEL_FRACTION_SCALE: i64 = 1 << TEXEL_FRACTION_BITS;
const TILE_TO_TEXEL_FRACTION_SCALE: i64 = TEXEL_FRACTION_SCALE / 4;

/// One signed RDP texture coordinate with five fractional bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureCoordinateS10_5(i16);

impl TextureCoordinateS10_5 {
    pub const fn from_raw(raw: i16) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> i16 {
        self.0
    }
}

/// One point sampler's already-quantized S/T coordinate pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PointSampleCoordinates {
    s: TextureCoordinateS10_5,
    t: TextureCoordinateS10_5,
}

impl PointSampleCoordinates {
    pub const fn new(s: TextureCoordinateS10_5, t: TextureCoordinateS10_5) -> Self {
        Self { s, t }
    }

    pub const fn s(self) -> TextureCoordinateS10_5 {
        self.s
    }

    pub const fn t(self) -> TextureCoordinateS10_5 {
        self.t
    }
}

/// Point coordinates plus the unresolved physical first-row parity input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PointSampleRequest {
    coordinates: PointSampleCoordinates,
    first_row_parity: TmemFirstRowParity,
}

impl PointSampleRequest {
    pub const fn new(
        coordinates: PointSampleCoordinates,
        first_row_parity: TmemFirstRowParity,
    ) -> Self {
        Self {
            coordinates,
            first_row_parity,
        }
    }

    pub const fn coordinates(self) -> PointSampleCoordinates {
        self.coordinates
    }

    pub const fn first_row_parity(self) -> TmemFirstRowParity {
        self.first_row_parity
    }
}

/// Texture axis named by an addressing diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureAxis {
    S,
    T,
}

impl fmt::Display for TextureAxis {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::S => formatter.write_str("S"),
            Self::T => formatter.write_str("T"),
        }
    }
}

/// Why a point coordinate could not be reduced to one addressed texel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointAddressError {
    ReversedClampExtent {
        axis: TextureAxis,
        low_raw: u16,
        high_raw: u16,
    },
}

impl fmt::Display for PointAddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReversedClampExtent {
                axis,
                low_raw,
                high_raw,
            } => write!(
                formatter,
                "point-sampled {axis} clamp extent is reversed: low {low_raw:#05x}, high {high_raw:#05x}"
            ),
        }
    }
}

impl std::error::Error for PointAddressError {}

/// Why a committed point sample could not be produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointSampleError {
    Address(PointAddressError),
    Read(PhysicalTexelReadError),
}

impl fmt::Display for PointSampleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Address(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PointSampleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Address(error) => Some(error),
            Self::Read(error) => Some(error),
        }
    }
}

impl From<PointAddressError> for PointSampleError {
    fn from(error: PointAddressError) -> Self {
        Self::Address(error)
    }
}

impl From<PhysicalTexelReadError> for PointSampleError {
    fn from(error: PhysicalTexelReadError) -> Self {
        Self::Read(error)
    }
}

/// Resolves one point-sampled S10.5 coordinate pair to integer tile indices.
///
/// This does not infer first-row parity, select a texture filter, convert a
/// floating-point or perspective coordinate, or read TMEM.
pub fn address_point_texel(
    tile: TileDescriptor,
    size: TileSize,
    request: PointSampleRequest,
) -> Result<AddressedTmemTexel, PointAddressError> {
    let coordinates = request.coordinates();
    let column = address_axis(
        TextureAxis::S,
        coordinates.s(),
        tile.shift_s(),
        size.low_s(),
        size.high_s(),
        tile.s_mode(),
        tile.mask_s(),
    )?;
    let row = address_axis(
        TextureAxis::T,
        coordinates.t(),
        tile.shift_t(),
        size.low_t(),
        size.high_t(),
        tile.t_mode(),
        tile.mask_t(),
    )?;
    Ok(AddressedTmemTexel::new(
        column,
        row,
        request.first_row_parity(),
    ))
}

/// Point-samples one texel from an immutable committed physical-TMEM state.
///
/// Addressing is completed before [`read_committed_texel`] observes physical
/// state. The returned color retains that reader's exact state/generation
/// snapshot identity.
pub fn sample_committed_point(
    state: &PhysicalTmemState,
    tile: TileDescriptor,
    size: TileSize,
    request: PointSampleRequest,
    lut_mode: TextureLutMode,
) -> Result<DecodedPhysicalTexel, PointSampleError> {
    let addressed = address_point_texel(tile, size, request)?;
    read_committed_texel(state, tile, addressed, lut_mode).map_err(Into::into)
}

fn address_axis(
    axis: TextureAxis,
    coordinate: TextureCoordinateS10_5,
    shift: u8,
    low: TileCoordinate,
    high: TileCoordinate,
    mode: TileAddressMode,
    mask: u8,
) -> Result<u16, PointAddressError> {
    debug_assert!(shift <= 15);
    debug_assert!(mask <= 15);

    let raw = i64::from(coordinate.raw());
    let shifted = match shift {
        0 => raw,
        1..=10 => raw >> shift,
        11..=15 => raw * (1_i64 << (16 - shift)),
        _ => unreachable!("G_SETTILE shift is a four-bit field"),
    };
    let origin = i64::from(low.raw()) * TILE_TO_TEXEL_FRACTION_SCALE;
    let coordinate = (shifted - origin).div_euclid(TEXEL_FRACTION_SCALE);

    let clamps = mask == 0 || mode.clamp();
    let coordinate = if clamps {
        if high.integer() < low.integer() {
            return Err(PointAddressError::ReversedClampExtent {
                axis,
                low_raw: low.raw(),
                high_raw: high.raw(),
            });
        }
        let dimension = i64::from(high.integer() - low.integer() + 1);
        coordinate.clamp(0, dimension - 1)
    } else {
        coordinate
    };

    if mask == 0 {
        return Ok(coordinate as u16);
    }

    let low_mask = (1_i64 << mask) - 1;
    let addressed = if mode.mirror() && coordinate & (1_i64 << mask) != 0 {
        (!coordinate) & low_mask
    } else {
        coordinate & low_mask
    };
    Ok(addressed as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImageFormat, PixelSize, TmemWordAddress};

    fn coordinate(raw: u16) -> TileCoordinate {
        TileCoordinate::try_new(raw).unwrap()
    }

    fn mode(mirror: bool, clamp: bool) -> TileAddressMode {
        TileAddressMode::from_wire(u8::from(mirror) | (u8::from(clamp) << 1))
    }

    #[allow(clippy::too_many_arguments)]
    fn tile(
        s_mode: TileAddressMode,
        mask_s: u8,
        shift_s: u8,
        t_mode: TileAddressMode,
        mask_t: u8,
        shift_t: u8,
    ) -> TileDescriptor {
        TileDescriptor::from_wire(
            ImageFormat::Intensity,
            PixelSize::Bits8,
            1,
            TmemWordAddress::try_new(0).unwrap(),
            0,
            t_mode,
            mask_t,
            shift_t,
            s_mode,
            mask_s,
            shift_s,
        )
    }

    fn size(low_s: u16, low_t: u16, high_s: u16, high_t: u16) -> TileSize {
        TileSize::from_wire(
            coordinate(low_s),
            coordinate(low_t),
            coordinate(high_s),
            coordinate(high_t),
        )
    }

    fn request(s: i16, t: i16, parity: TmemFirstRowParity) -> PointSampleRequest {
        PointSampleRequest::new(
            PointSampleCoordinates::new(
                TextureCoordinateS10_5::from_raw(s),
                TextureCoordinateS10_5::from_raw(t),
            ),
            parity,
        )
    }

    #[test]
    fn coordinate_and_request_preserve_raw_values_and_explicit_parity() {
        for raw in [i16::MIN, -33, -32, -31, -1, 0, 1, 31, 32, 33, i16::MAX] {
            assert_eq!(TextureCoordinateS10_5::from_raw(raw).raw(), raw);
        }
        let coordinates = PointSampleCoordinates::new(
            TextureCoordinateS10_5::from_raw(-33),
            TextureCoordinateS10_5::from_raw(65),
        );
        let request = PointSampleRequest::new(coordinates, TmemFirstRowParity::Odd);
        assert_eq!(request.coordinates(), coordinates);
        assert_eq!(request.coordinates().s().raw(), -33);
        assert_eq!(request.coordinates().t().raw(), 65);
        assert_eq!(request.first_row_parity(), TmemFirstRowParity::Odd);
    }

    #[test]
    fn all_shift_encodings_apply_before_origin_subtraction() {
        let positive = [
            (0, 0),
            (1, 32_767),
            (2, 32_766),
            (3, 32_766),
            (4, 32_766),
            (5, 32_766),
            (6, 32_766),
            (7, 32_766),
            (8, 32_766),
            (9, 32_766),
            (10, 32_766),
            (11, 62),
            (12, 30),
            (13, 14),
            (14, 6),
            (15, 2),
        ];
        for (shift, expected) in positive {
            let addressed = address_point_texel(
                tile(mode(false, false), 15, shift, mode(false, false), 1, 0),
                size(8, 0, 0, 0),
                request(64, 0, TmemFirstRowParity::Even),
            )
            .unwrap();
            assert_eq!(addressed.column(), expected, "shift {shift}");
        }

        let negative = [
            (0, 32_765),
            (1, 32_766),
            (2, 32_767),
            (3, 32_767),
            (4, 32_767),
            (5, 32_767),
            (6, 32_767),
            (7, 32_767),
            (8, 32_767),
            (9, 32_767),
            (10, 32_767),
            (11, 32_703),
            (12, 32_735),
            (13, 32_751),
            (14, 32_759),
            (15, 32_763),
        ];
        for (shift, expected) in negative {
            let addressed = address_point_texel(
                tile(mode(false, false), 15, shift, mode(false, false), 1, 0),
                size(0, 0, 0, 0),
                request(-65, 0, TmemFirstRowParity::Even),
            )
            .unwrap();
            assert_eq!(addressed.column(), expected, "negative shift {shift}");
        }
    }

    #[test]
    fn origin_and_negative_fraction_use_exact_integer_floor() {
        let tile = tile(mode(false, false), 4, 0, mode(false, false), 4, 0);
        let size = size(3, 5, 0, 0);
        let cases = [
            ((23, 39), (15, 15)),
            ((24, 40), (0, 0)),
            ((55, 71), (0, 0)),
            ((56, 72), (1, 1)),
        ];
        for ((s, t), expected) in cases {
            let addressed =
                address_point_texel(tile, size, request(s, t, TmemFirstRowParity::Even)).unwrap();
            assert_eq!((addressed.column(), addressed.row()), expected);
        }
    }

    #[test]
    fn simultaneous_fractional_clamp_extents_select_both_exact_edges() {
        let clamped = tile(mode(false, true), 2, 0, mode(false, false), 0, 0);
        let fractional = size(3, 5, 12, 18);
        let cases = [
            ((23, 39), (0, 0)),
            ((24, 40), (0, 0)),
            ((56, 72), (1, 1)),
            ((120, 136), (3, 3)),
            ((151, 167), (3, 3)),
            ((152, 168), (3, 3)),
        ];
        for ((s, t), expected) in cases {
            let addressed =
                address_point_texel(clamped, fractional, request(s, t, TmemFirstRowParity::Even))
                    .unwrap();
            assert_eq!(
                (addressed.column(), addressed.row()),
                expected,
                "raw S10.5=({s}, {t})"
            );
        }
    }

    #[test]
    fn implicit_and_explicit_clamp_precede_mirror_and_mask() {
        let implicit = tile(mode(true, false), 0, 0, mode(false, false), 1, 0);
        let extent = size(4, 0, 12, 0);
        assert_eq!(
            address_point_texel(
                implicit,
                extent,
                request(-1024, 0, TmemFirstRowParity::Even)
            )
            .unwrap()
            .column(),
            0
        );
        assert_eq!(
            address_point_texel(implicit, extent, request(1024, 0, TmemFirstRowParity::Even))
                .unwrap()
                .column(),
            2
        );

        let explicit = tile(mode(true, true), 2, 0, mode(false, false), 1, 0);
        assert_eq!(
            address_point_texel(
                explicit,
                size(0, 0, 44, 0),
                request(20 * 32, 0, TmemFirstRowParity::Even),
            )
            .unwrap()
            .column(),
            3,
            "clamp to texel 11 must occur before mask-2 mirror resolves it"
        );
    }

    #[test]
    fn literal_mask_one_and_two_boundaries_cover_s_t_and_joint_addressing() {
        let cases = [
            (1, -2, 0, 1),
            (1, -1, 1, 0),
            (1, 0, 0, 0),
            (1, 1, 1, 1),
            (1, 2, 0, 1),
            (1, 3, 1, 0),
            (1, 4, 0, 0),
            (1, 5, 1, 1),
            (2, -4, 0, 3),
            (2, -1, 3, 0),
            (2, 0, 0, 0),
            (2, 3, 3, 3),
            (2, 4, 0, 3),
            (2, 5, 1, 2),
            (2, 7, 3, 0),
            (2, 8, 0, 0),
            (2, 9, 1, 1),
        ];
        for (mask, texel, wrapped, mirrored) in cases {
            for (mirror, expected) in [(false, wrapped), (true, mirrored)] {
                let s_addressed = address_point_texel(
                    tile(mode(mirror, false), mask, 0, mode(false, false), 1, 0),
                    size(0, 0, 0, 0),
                    request(texel * 32, 0, TmemFirstRowParity::Even),
                )
                .unwrap();
                assert_eq!(
                    s_addressed.column(),
                    expected,
                    "S mask={mask} texel={texel} mirror={mirror}"
                );

                let t_addressed = address_point_texel(
                    tile(mode(false, false), 1, 0, mode(mirror, false), mask, 0),
                    size(0, 0, 0, 0),
                    request(0, texel * 32, TmemFirstRowParity::Even),
                )
                .unwrap();
                assert_eq!(
                    t_addressed.row(),
                    expected,
                    "T mask={mask} texel={texel} mirror={mirror}"
                );
            }
        }

        let joint_cases = [((-1, 4), (0, 3)), ((2, 5), (1, 2)), ((5, 9), (1, 1))];
        for ((s, t), expected) in joint_cases {
            let joint = address_point_texel(
                tile(mode(true, false), 1, 0, mode(true, false), 2, 0),
                size(0, 0, 0, 0),
                request(s * 32, t * 32, TmemFirstRowParity::Odd),
            )
            .unwrap();
            assert_eq!((joint.column(), joint.row()), expected);
            assert_eq!(joint.first_row_parity(), TmemFirstRowParity::Odd);
        }
    }

    #[test]
    fn mask_fifteen_literal_boundaries_cover_s_t_and_joint_addressing() {
        // Signed S10.5 cannot spell +32768. With shift 11, these literal
        // negative coordinates are the adjacent representatives around the
        // 32768 and 65536 boundaries in mask-15's 65536-texel period.
        let cases = [
            (-32_768, 4, 32_767, 32_767),
            (-32_768, 0, 0, 32_767),
            (-32_767, 0, 1, 32_766),
            (-1, 0, 32_767, 0),
            (0, 0, 0, 0),
            (1, 0, 1, 1),
        ];
        for (raw, low_raw, wrapped, mirrored) in cases {
            for (mirror, expected) in [(false, wrapped), (true, mirrored)] {
                let s_addressed = address_point_texel(
                    tile(mode(mirror, false), 15, 11, mode(false, false), 1, 0),
                    size(low_raw, 0, 0, 0),
                    request(raw, 0, TmemFirstRowParity::Even),
                )
                .unwrap();
                assert_eq!(
                    s_addressed.column(),
                    expected,
                    "S raw={raw} low={low_raw} mirror={mirror}"
                );

                let t_addressed = address_point_texel(
                    tile(mode(false, false), 1, 0, mode(mirror, false), 15, 11),
                    size(0, low_raw, 0, 0),
                    request(0, raw, TmemFirstRowParity::Even),
                )
                .unwrap();
                assert_eq!(
                    t_addressed.row(),
                    expected,
                    "T raw={raw} low={low_raw} mirror={mirror}"
                );
            }
        }

        let joint = address_point_texel(
            tile(mode(true, false), 15, 11, mode(true, false), 15, 11),
            size(0, 0, 0, 0),
            request(-32_767, -1, TmemFirstRowParity::Odd),
        )
        .unwrap();
        assert_eq!((joint.column(), joint.row()), (32_766, 0));
        assert_eq!(joint.first_row_parity(), TmemFirstRowParity::Odd);
    }

    #[test]
    fn reversed_extent_is_required_only_for_a_clamped_axis() {
        let reversed = size(12, 0, 4, 0);
        assert_eq!(
            address_point_texel(
                tile(mode(false, false), 0, 0, mode(false, false), 1, 0),
                reversed,
                request(0, 0, TmemFirstRowParity::Even),
            ),
            Err(PointAddressError::ReversedClampExtent {
                axis: TextureAxis::S,
                low_raw: 12,
                high_raw: 4,
            })
        );

        let addressed = address_point_texel(
            tile(mode(false, false), 2, 0, mode(false, false), 1, 0),
            reversed,
            request(3 * 32, 0, TmemFirstRowParity::Odd),
        )
        .unwrap();
        assert_eq!(addressed.column(), 0);
        assert_eq!(addressed.first_row_parity(), TmemFirstRowParity::Odd);
    }

    #[test]
    fn addressing_error_precedes_an_empty_state_read() {
        let state = PhysicalTmemState::try_new().unwrap();
        let error = sample_committed_point(
            &state,
            tile(mode(false, false), 0, 0, mode(false, false), 1, 0),
            size(12, 0, 4, 0),
            request(0, 0, TmemFirstRowParity::Even),
            TextureLutMode::Disabled,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            PointSampleError::Address(PointAddressError::ReversedClampExtent {
                axis: TextureAxis::S,
                ..
            })
        ));
        assert!(!error.to_string().is_empty());
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn invalid_direct_byte_is_preserved_as_an_exact_nested_read_error() {
        let state = PhysicalTmemState::try_new().unwrap();
        let observe = |state: &PhysicalTmemState| {
            (
                (0_u16..4096)
                    .map(|address| state.valid_byte(address))
                    .collect::<Vec<_>>(),
                (0_u16..4096)
                    .map(|address| state.byte_is_valid(address))
                    .collect::<Vec<_>>(),
                (0_u16..4096)
                    .map(|address| state.last_touched_generation(address))
                    .collect::<Vec<_>>(),
                state.generation(),
                state.last_load_epoch(),
            )
        };
        let before = observe(&state);
        assert_eq!(
            sample_committed_point(
                &state,
                tile(mode(false, false), 0, 0, mode(false, false), 0, 0),
                size(0, 0, 0, 0),
                request(0, 0, TmemFirstRowParity::Odd),
                TextureLutMode::Disabled,
            ),
            Err(PointSampleError::Read(
                PhysicalTexelReadError::InvalidTexelByte { address: 4 }
            ))
        );
        assert_eq!(observe(&state), before);
    }
}
