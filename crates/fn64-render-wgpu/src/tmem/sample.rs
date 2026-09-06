//! Typed point addressing over committed physical TMEM.
//!
//! The public RDP tile sequence shifts an S10.5 coordinate, subtracts the
//! S10.2 tile origin, and applies clamp before mirror/mask addressing. This
//! module performs that integer-only point path and delegates every physical
//! byte, validity, format, and TLUT decision to [`super::read_committed_texel`].
//! First-row parity remains caller-owned: the available authorities do not
//! settle its derivation when load and render tiles differ.

use core::{fmt, num::NonZeroU64};

use crate::{TextureFilter, TextureLutMode};

use super::{
    read_texel, AddressedTmemTexel, DecodedPhysicalTexel, PhysicalTexelReadError,
    PhysicalTmemState, PreparedTexelReader, TileAddressMode, TileCoordinate, TileDescriptor,
    TileSize, TmemByteSource, TmemFirstRowParity,
};

const TEXEL_FRACTION_BITS: u32 = 5;
const TEXEL_FRACTION_SCALE: i64 = 1 << TEXEL_FRACTION_BITS;
const TILE_TO_TEXEL_FRACTION_SCALE: i64 = TEXEL_FRACTION_SCALE / 4;
// Each small triangle, and each parallel row band, binds a fresh sampler.
// Keep that short-lived zeroed state cache-local. The packed key is injective
// over the complete addressed texel, while the value is only the RGBA result
// this private sampler returns; collision can only reread, never alias.
const PREPARED_TEXEL_CACHE_AXIS_BITS: usize = 3;
const PREPARED_TEXEL_CACHE_LEN: usize = 1 << (PREPARED_TEXEL_CACHE_AXIS_BITS * 2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreparedTexelCacheKey(NonZeroU64);

impl PreparedTexelCacheKey {
    fn new(addressed: AddressedTmemTexel) -> Self {
        let parity = match addressed.first_row_parity() {
            TmemFirstRowParity::Even => 0,
            TmemFirstRowParity::Odd => 1,
        };
        let packed =
            u64::from(addressed.column()) | (u64::from(addressed.row()) << 16) | (parity << 32);
        Self(NonZeroU64::new(packed + 1).expect("a 33-bit cache key plus one is nonzero"))
    }

    fn index(self) -> usize {
        let packed = self.0.get() - 1;
        let axis_mask = (1 << PREPARED_TEXEL_CACHE_AXIS_BITS) - 1;
        (((packed as usize >> 16) & axis_mask) << PREPARED_TEXEL_CACHE_AXIS_BITS)
            | (packed as usize & axis_mask)
    }
}

#[derive(Clone, Copy)]
struct PreparedTexelCacheEntry {
    key: PreparedTexelCacheKey,
    rgba8888: [u8; 4],
}

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

/// Five-bit position of one sample inside its post-tile-shift texture cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureCellFractions {
    s: u8,
    t: u8,
}

impl TextureCellFractions {
    pub const fn s_five_bit(self) -> u8 {
        self.s
    }

    pub const fn t_five_bit(self) -> u8 {
        self.t
    }
}

/// One semantic corner of the integer texture cell containing a sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureCellCorner {
    UpperLeft,
    LowerLeft,
    UpperRight,
    LowerRight,
}

impl TextureCellCorner {
    const fn index(self) -> usize {
        match self {
            Self::UpperLeft => 0,
            Self::LowerLeft => 1,
            Self::UpperRight => 2,
            Self::LowerRight => 3,
        }
    }
}

impl fmt::Display for TextureCellCorner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UpperLeft => formatter.write_str("upper-left"),
            Self::LowerLeft => formatter.write_str("lower-left"),
            Self::UpperRight => formatter.write_str("upper-right"),
            Self::LowerRight => formatter.write_str("lower-right"),
        }
    }
}

/// Four independently addressed corners around one point coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressedTextureCell {
    fractions: TextureCellFractions,
    corners: [AddressedTmemTexel; 4],
}

impl AddressedTextureCell {
    pub const fn fractions(self) -> TextureCellFractions {
        self.fractions
    }

    pub const fn corner(self, corner: TextureCellCorner) -> AddressedTmemTexel {
        self.corners[corner.index()]
    }

    pub const fn corners(self) -> [AddressedTmemTexel; 4] {
        self.corners
    }
}

/// Four decoded corners bound to their committed physical-TMEM snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommittedTextureCell {
    addressed: AddressedTextureCell,
    texels: [DecodedPhysicalTexel; 4],
}

impl CommittedTextureCell {
    pub const fn addressed(self) -> AddressedTextureCell {
        self.addressed
    }

    pub const fn corner(self, corner: TextureCellCorner) -> DecodedPhysicalTexel {
        self.texels[corner.index()]
    }

    pub const fn texels(self) -> [DecodedPhysicalTexel; 4] {
        self.texels
    }
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

/// Why a committed four-corner texture-cell gather could not complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureCellSampleError {
    Address(PointAddressError),
    Read {
        corner: TextureCellCorner,
        source: PhysicalTexelReadError,
    },
}

/// Why the filter selected by OtherMode could not produce one texture sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureSampleError {
    Point(PointSampleError),
    Cell(TextureCellSampleError),
    ReservedFilter,
}

impl fmt::Display for TextureSampleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Point(error) => error.fmt(formatter),
            Self::Cell(error) => error.fmt(formatter),
            Self::ReservedFilter => formatter
                .write_str("reserved RDP texture-filter encoding reached the production sampler"),
        }
    }
}

impl std::error::Error for TextureSampleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Point(error) => Some(error),
            Self::Cell(error) => Some(error),
            Self::ReservedFilter => None,
        }
    }
}

impl From<PointSampleError> for TextureSampleError {
    fn from(error: PointSampleError) -> Self {
        Self::Point(error)
    }
}

impl From<TextureCellSampleError> for TextureSampleError {
    fn from(error: TextureCellSampleError) -> Self {
        Self::Cell(error)
    }
}

impl fmt::Display for TextureCellSampleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Address(error) => error.fmt(formatter),
            Self::Read { corner, source } => {
                write!(formatter, "{corner} texture-cell read failed: {source}")
            }
        }
    }
}

impl std::error::Error for TextureCellSampleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Address(error) => Some(error),
            Self::Read { source, .. } => Some(source),
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
    let s = relative_axis_coordinate(coordinates.s(), tile.shift_s(), size.low_s());
    let column = address_axis_texel(
        TextureAxis::S,
        s.base_texel,
        size.low_s(),
        size.high_s(),
        tile.s_mode(),
        tile.mask_s(),
    )?;
    let t = relative_axis_coordinate(coordinates.t(), tile.shift_t(), size.low_t());
    let row = address_axis_texel(
        TextureAxis::T,
        t.base_texel,
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

/// Addresses all four corners of the integer cell containing one S10.5 point.
///
/// This exposes the exact post-shift, post-origin five-bit fractions but does
/// not select a texture filter or decide which corners a filter consumes.
pub fn address_texture_cell(
    tile: TileDescriptor,
    size: TileSize,
    request: PointSampleRequest,
) -> Result<AddressedTextureCell, PointAddressError> {
    let coordinates = request.coordinates();
    let s = relative_axis_coordinate(coordinates.s(), tile.shift_s(), size.low_s());
    let t = relative_axis_coordinate(coordinates.t(), tile.shift_t(), size.low_t());
    let s0 = address_axis_texel(
        TextureAxis::S,
        s.base_texel,
        size.low_s(),
        size.high_s(),
        tile.s_mode(),
        tile.mask_s(),
    )?;
    let s1 = address_axis_texel(
        TextureAxis::S,
        s.base_texel + 1,
        size.low_s(),
        size.high_s(),
        tile.s_mode(),
        tile.mask_s(),
    )?;
    let t0 = address_axis_texel(
        TextureAxis::T,
        t.base_texel,
        size.low_t(),
        size.high_t(),
        tile.t_mode(),
        tile.mask_t(),
    )?;
    let t1 = address_axis_texel(
        TextureAxis::T,
        t.base_texel + 1,
        size.low_t(),
        size.high_t(),
        tile.t_mode(),
        tile.mask_t(),
    )?;
    let parity = request.first_row_parity();
    Ok(AddressedTextureCell {
        fractions: TextureCellFractions {
            s: s.fraction_five_bit,
            t: t.fraction_five_bit,
        },
        corners: [
            AddressedTmemTexel::new(s0, t0, parity),
            AddressedTmemTexel::new(s0, t1, parity),
            AddressedTmemTexel::new(s1, t0, parity),
            AddressedTmemTexel::new(s1, t1, parity),
        ],
    })
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
    sample_point(state, tile, size, request, lut_mode)
}

/// Point-samples one texel from any physical-TMEM image -- durable state or
/// a sealed-but-unpublished [`super::PendingTmemTransaction`] post-image
/// (via [`super::PendingTmemTransaction::pending_image`]).
///
/// [`sample_committed_point`] above is this function at
/// `S = PhysicalTmemState`; there is one addressing path and one reader,
/// not two. The returned texel's `snapshot()` distinguishes which image was
/// observed -- `TmemSnapshotIdentity::Committed` for durable state,
/// `Proposed` for a post-image -- so a caller requiring durability rejects
/// the proposal case by name instead of being handed a fabricated durable
/// receipt.
pub fn sample_point<S: TmemByteSource + ?Sized>(
    state: &S,
    tile: TileDescriptor,
    size: TileSize,
    request: PointSampleRequest,
    lut_mode: TextureLutMode,
) -> Result<DecodedPhysicalTexel, PointSampleError> {
    let addressed = address_point_texel(tile, size, request)?;
    read_texel(state, tile, addressed, lut_mode).map_err(Into::into)
}

/// Samples one texel through the RDP filter selected by OtherMode.
///
/// Point mode performs one physical read. Bilinear mode uses the RDP's
/// three-nearest triangular interpolation, and Average uses all four cell
/// corners. The same typed address/read path serves durable and pending TMEM
/// images, so filter selection cannot change snapshot authority.
pub fn sample_texture<S: TmemByteSource + ?Sized>(
    state: &S,
    tile: TileDescriptor,
    size: TileSize,
    request: PointSampleRequest,
    lut_mode: TextureLutMode,
    filter: TextureFilter,
) -> Result<[u8; 4], TextureSampleError> {
    match filter {
        TextureFilter::Point => sample_point(state, tile, size, request, lut_mode)
            .map(|sample| sample.texel().rgba8888())
            .map_err(Into::into),
        TextureFilter::Reserved => Err(TextureSampleError::ReservedFilter),
        TextureFilter::Average => gather_texture_cell(state, tile, size, request, lut_mode)
            .map(average_texture_cell)
            .map_err(Into::into),
        TextureFilter::Bilinear => {
            let addressed = address_texture_cell(tile, size, request)
                .map_err(TextureCellSampleError::Address)?;
            let read = |corner| {
                read_texel(state, tile, addressed.corner(corner), lut_mode)
                    .map(|sample| sample.texel().rgba8888())
                    .map_err(|source| TextureCellSampleError::Read { corner, source })
            };
            let fractions = addressed.fractions();
            let sf = i64::from(fractions.s_five_bit());
            let tf = i64::from(fractions.t_five_bit());
            if sf + tf <= TEXEL_FRACTION_SCALE {
                Ok(filter_three_nearest_lower(
                    read(TextureCellCorner::UpperLeft)?,
                    read(TextureCellCorner::UpperRight)?,
                    read(TextureCellCorner::LowerLeft)?,
                    sf,
                    tf,
                ))
            } else {
                Ok(filter_three_nearest_upper(
                    read(TextureCellCorner::LowerRight)?,
                    read(TextureCellCorner::LowerLeft)?,
                    read(TextureCellCorner::UpperRight)?,
                    sf,
                    tf,
                ))
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct PreparedTextureSampler {
    tile: TileDescriptor,
    size: TileSize,
    filter: TextureFilter,
    reader: PreparedTexelReader,
}

impl PreparedTextureSampler {
    pub fn try_new(
        tile: TileDescriptor,
        size: TileSize,
        lut_mode: TextureLutMode,
        filter: TextureFilter,
    ) -> Result<Self, TextureSampleError> {
        if filter == TextureFilter::Reserved {
            return Err(TextureSampleError::ReservedFilter);
        }
        Ok(Self {
            tile,
            size,
            filter,
            reader: PreparedTexelReader::try_new(tile, lut_mode).map_err(PointSampleError::from)?,
        })
    }

    pub(crate) fn bind<S: TmemByteSource + ?Sized>(
        self,
        state: &S,
    ) -> BoundPreparedTextureSampler<'_, S> {
        BoundPreparedTextureSampler {
            tile: self.tile,
            size: self.size,
            filter: self.filter,
            reader: self.reader.bind(state),
            texel_cache: [None; PREPARED_TEXEL_CACHE_LEN],
        }
    }
}

pub(crate) struct BoundPreparedTextureSampler<'a, S: TmemByteSource + ?Sized> {
    tile: TileDescriptor,
    size: TileSize,
    filter: TextureFilter,
    reader: super::BoundPreparedTexelReader<'a, S>,
    texel_cache: [Option<PreparedTexelCacheEntry>; PREPARED_TEXEL_CACHE_LEN],
}

impl<S: TmemByteSource + ?Sized> BoundPreparedTextureSampler<'_, S> {
    fn read(&mut self, addressed: AddressedTmemTexel) -> Result<[u8; 4], PhysicalTexelReadError> {
        let key = PreparedTexelCacheKey::new(addressed);
        let index = key.index();
        if let Some(cached) = self.texel_cache[index] {
            if cached.key == key {
                return Ok(cached.rgba8888);
            }
        }
        let rgba8888 = self.reader.read(addressed)?.texel().rgba8888();
        self.texel_cache[index] = Some(PreparedTexelCacheEntry { key, rgba8888 });
        Ok(rgba8888)
    }

    pub fn sample(&mut self, request: PointSampleRequest) -> Result<[u8; 4], TextureSampleError> {
        if self.filter == TextureFilter::Point {
            let addressed = address_point_texel(self.tile, self.size, request)
                .map_err(PointSampleError::from)?;
            return self
                .read(addressed)
                .map_err(PointSampleError::from)
                .map_err(Into::into);
        }

        let addressed = address_texture_cell(self.tile, self.size, request)
            .map_err(TextureCellSampleError::Address)?;
        let filter = self.filter;
        let mut read = |corner| {
            self.read(addressed.corner(corner))
                .map_err(|source| TextureCellSampleError::Read { corner, source })
        };
        if filter == TextureFilter::Bilinear {
            let fractions = addressed.fractions();
            let sf = i64::from(fractions.s_five_bit());
            let tf = i64::from(fractions.t_five_bit());
            return if sf + tf <= TEXEL_FRACTION_SCALE {
                let c00 = read(TextureCellCorner::UpperLeft)?;
                let c10 = read(TextureCellCorner::UpperRight)?;
                let c01 = read(TextureCellCorner::LowerLeft)?;
                Ok(filter_three_nearest_lower(c00, c10, c01, sf, tf))
            } else {
                let c11 = read(TextureCellCorner::LowerRight)?;
                let c01 = read(TextureCellCorner::LowerLeft)?;
                let c10 = read(TextureCellCorner::UpperRight)?;
                Ok(filter_three_nearest_upper(c11, c01, c10, sf, tf))
            };
        }
        let texels = [
            read(TextureCellCorner::UpperLeft)?,
            read(TextureCellCorner::LowerLeft)?,
            read(TextureCellCorner::UpperRight)?,
            read(TextureCellCorner::LowerRight)?,
        ];
        debug_assert_eq!(filter, TextureFilter::Average);
        Ok(average_rgba_texture_cell(texels))
    }
}

/// Reads all four semantic corners around one point from committed TMEM.
///
/// All addressing completes before the first physical read. Production
/// three-nearest sampling uses [`PreparedTextureSampler`] instead so the
/// unused fourth corner cannot create a false validity failure.
pub fn gather_committed_texture_cell(
    state: &PhysicalTmemState,
    tile: TileDescriptor,
    size: TileSize,
    request: PointSampleRequest,
    lut_mode: TextureLutMode,
) -> Result<CommittedTextureCell, TextureCellSampleError> {
    gather_texture_cell(state, tile, size, request, lut_mode)
}

/// Reads all four semantic corners from any physical-TMEM image.
pub fn gather_texture_cell<S: TmemByteSource + ?Sized>(
    state: &S,
    tile: TileDescriptor,
    size: TileSize,
    request: PointSampleRequest,
    lut_mode: TextureLutMode,
) -> Result<CommittedTextureCell, TextureCellSampleError> {
    let addressed =
        address_texture_cell(tile, size, request).map_err(TextureCellSampleError::Address)?;
    let read = |corner| {
        read_texel(state, tile, addressed.corner(corner), lut_mode)
            .map_err(|source| TextureCellSampleError::Read { corner, source })
    };
    let texels = [
        read(TextureCellCorner::UpperLeft)?,
        read(TextureCellCorner::LowerLeft)?,
        read(TextureCellCorner::UpperRight)?,
        read(TextureCellCorner::LowerRight)?,
    ];
    debug_assert!(texels
        .iter()
        .all(|texel| texel.snapshot() == texels[0].snapshot()));
    Ok(CommittedTextureCell { addressed, texels })
}

/// Four-corner box average selected by `TextureFilter::Average`.
pub fn average_texture_cell(cell: CommittedTextureCell) -> [u8; 4] {
    let texels = cell.texels().map(|sample| sample.texel().rgba8888());
    average_rgba_texture_cell(texels)
}

fn average_rgba_texture_cell(texels: [[u8; 4]; 4]) -> [u8; 4] {
    std::array::from_fn(|channel| {
        let sum = texels
            .iter()
            .map(|sample| u16::from(sample[channel]))
            .sum::<u16>();
        ((sum + 2) / 4) as u8
    })
}

const TEXEL_FRACTION_HALF_SCALE: i64 = TEXEL_FRACTION_SCALE / 2;

/// The RDP's "three nearest" triangular interpolation of a committed 2x2
/// texture cell, selected by which half of the cell's diagonal the sample
/// falls in. Not a 4-tap/box average — [`average_texture_cell`] implements
/// that separately selected mode.
///
/// Nintendo's Programming Manual, "TF: Texture Filter" and "Sampling
/// Overview," define this triangular selection; the fixed-point formula
/// below ports `fn64-render-reference`'s already-tested
/// `filter_three_nearest_s10_5`
/// (`crates/fn64-render-reference/src/gbi/types.rs:954-972`). That
/// function's corner order is `[c00, c10, c01, c11]` = `[UpperLeft,
/// UpperRight, LowerLeft, LowerRight]`; `CommittedTextureCell`'s stored order
/// is `[UpperLeft, LowerLeft, UpperRight, LowerRight]`, so this remaps by
/// named corner rather than reusing the reference array literally.
///
/// The round-to-nearest, clamp-to-`u8` output policy matches the reference
/// lane exactly. Public documentation does not establish the silicon filter
/// accumulator width or its tie-break rule; this is a preserved convention,
/// not a verified hardware fact.
pub fn filter_three_nearest_committed_cell(cell: CommittedTextureCell) -> [u8; 4] {
    let fractions = cell.addressed().fractions();
    let c00 = cell.corner(TextureCellCorner::UpperLeft).texel().rgba8888();
    let c10 = cell
        .corner(TextureCellCorner::UpperRight)
        .texel()
        .rgba8888();
    let c01 = cell.corner(TextureCellCorner::LowerLeft).texel().rgba8888();
    let c11 = cell
        .corner(TextureCellCorner::LowerRight)
        .texel()
        .rgba8888();
    filter_three_nearest(
        [c00, c10, c01, c11],
        i64::from(fractions.s_five_bit()),
        i64::from(fractions.t_five_bit()),
    )
}

/// Pure fixed-point arithmetic behind [`filter_three_nearest_committed_cell`],
/// taking the four corners pre-remapped to `[c00, c10, c01, c11]` = `[UpperLeft,
/// UpperRight, LowerLeft, LowerRight]` (`fn64-render-reference`'s
/// `filter_three_nearest_s10_5` order) and 5-bit `sf`/`tf` fractions in
/// `0..32`. Split out so the exhaustive differential below can drive it
/// directly against synthetic corner values, without a physical TMEM commit
/// per case.
fn filter_three_nearest(corners: [[u8; 4]; 4], sf: i64, tf: i64) -> [u8; 4] {
    debug_assert!((0..TEXEL_FRACTION_SCALE).contains(&sf));
    debug_assert!((0..TEXEL_FRACTION_SCALE).contains(&tf));
    let [c00, c10, c01, c11] = corners;
    if sf + tf <= TEXEL_FRACTION_SCALE {
        filter_three_nearest_lower(c00, c10, c01, sf, tf)
    } else {
        filter_three_nearest_upper(c11, c01, c10, sf, tf)
    }
}

fn filter_three_nearest_lower(
    c00: [u8; 4],
    c10: [u8; 4],
    c01: [u8; 4],
    sf: i64,
    tf: i64,
) -> [u8; 4] {
    std::array::from_fn(|channel| {
        let c00 = i64::from(c00[channel]);
        let value = c00 * TEXEL_FRACTION_SCALE
            + sf * (i64::from(c10[channel]) - c00)
            + tf * (i64::from(c01[channel]) - c00);
        ((value + TEXEL_FRACTION_HALF_SCALE) / TEXEL_FRACTION_SCALE).clamp(0, 255) as u8
    })
}

fn filter_three_nearest_upper(
    c11: [u8; 4],
    c01: [u8; 4],
    c10: [u8; 4],
    sf: i64,
    tf: i64,
) -> [u8; 4] {
    std::array::from_fn(|channel| {
        let c11 = i64::from(c11[channel]);
        let value = c11 * TEXEL_FRACTION_SCALE
            + (TEXEL_FRACTION_SCALE - sf) * (i64::from(c01[channel]) - c11)
            + (TEXEL_FRACTION_SCALE - tf) * (i64::from(c10[channel]) - c11);
        ((value + TEXEL_FRACTION_HALF_SCALE) / TEXEL_FRACTION_SCALE).clamp(0, 255) as u8
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RelativeAxisCoordinate {
    base_texel: i64,
    fraction_five_bit: u8,
}

fn relative_axis_coordinate(
    coordinate: TextureCoordinateS10_5,
    shift: u8,
    low: TileCoordinate,
) -> RelativeAxisCoordinate {
    debug_assert!(shift <= 15);

    let raw = i64::from(coordinate.raw());
    let shifted = match shift {
        0 => raw,
        1..=10 => raw >> shift,
        11..=15 => raw * (1_i64 << (16 - shift)),
        _ => unreachable!("G_SETTILE shift is a four-bit field"),
    };
    let origin = i64::from(low.raw()) * TILE_TO_TEXEL_FRACTION_SCALE;
    let relative = shifted - origin;
    RelativeAxisCoordinate {
        base_texel: relative.div_euclid(TEXEL_FRACTION_SCALE),
        fraction_five_bit: relative.rem_euclid(TEXEL_FRACTION_SCALE) as u8,
    }
}

fn address_axis_texel(
    axis: TextureAxis,
    coordinate: i64,
    low: TileCoordinate,
    high: TileCoordinate,
    mode: TileAddressMode,
    mask: u8,
) -> Result<u16, PointAddressError> {
    debug_assert!(mask <= 15);

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
    use crate::{ImageFormat, PixelSize, TmemSnapshotIdentity, TmemWordAddress};

    struct FilterFixture(PhysicalTmemState);

    impl TmemByteSource for FilterFixture {
        fn snapshot(&self) -> TmemSnapshotIdentity {
            self.0.snapshot()
        }

        fn valid_byte(&self, address: u16) -> Option<u8> {
            Some(match address {
                // I8, one TMEM word per row. Odd-row word exchange maps
                // row-one columns zero and one to physical bytes 12 and 13.
                0 => 0,
                1 => 64,
                12 => 128,
                13 => 255,
                _ => 0,
            })
        }
    }

    struct InvalidCornerFixture {
        source: FilterFixture,
        invalid_address: u16,
    }

    impl TmemByteSource for InvalidCornerFixture {
        fn snapshot(&self) -> TmemSnapshotIdentity {
            self.source.snapshot()
        }

        fn valid_byte(&self, address: u16) -> Option<u8> {
            (address != self.invalid_address).then(|| {
                self.source
                    .valid_byte(address)
                    .expect("fixture byte is valid")
            })
        }
    }

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
    fn production_sampler_selects_point_three_nearest_average_and_reserved_lanes() {
        let source = FilterFixture(PhysicalTmemState::try_new().unwrap());
        let tile = tile(mode(false, false), 1, 0, mode(false, false), 1, 0);
        let size = size(0, 0, 4, 4);
        let center_request = request(8, 8, TmemFirstRowParity::Even);
        let sample = |filter| {
            sample_texture(
                &source,
                tile,
                size,
                center_request,
                TextureLutMode::Disabled,
                filter,
            )
        };
        let prepared_sample = |filter| {
            PreparedTextureSampler::try_new(tile, size, TextureLutMode::Disabled, filter)
                .and_then(|sampler| sampler.bind(&source).sample(center_request))
        };

        assert_eq!(sample(TextureFilter::Point), Ok([0, 0, 0, 0]));
        // sf=tf=8/32: 0 + 1/4*(64-0) + 1/4*(128-0) = 48.
        assert_eq!(sample(TextureFilter::Bilinear), Ok([48, 48, 48, 48]));
        assert_eq!(sample(TextureFilter::Average), Ok([112, 112, 112, 112]));
        assert_eq!(
            sample(TextureFilter::Reserved),
            Err(TextureSampleError::ReservedFilter)
        );
        for filter in [
            TextureFilter::Point,
            TextureFilter::Bilinear,
            TextureFilter::Average,
            TextureFilter::Reserved,
        ] {
            assert_eq!(prepared_sample(filter), sample(filter), "filter {filter:?}");
        }
        for s in 0..32 {
            for t in 0..32 {
                let request = request(s, t, TmemFirstRowParity::Even);
                for filter in [
                    TextureFilter::Point,
                    TextureFilter::Bilinear,
                    TextureFilter::Average,
                ] {
                    let generic = sample_texture(
                        &source,
                        tile,
                        size,
                        request,
                        TextureLutMode::Disabled,
                        filter,
                    );
                    let prepared = PreparedTextureSampler::try_new(
                        tile,
                        size,
                        TextureLutMode::Disabled,
                        filter,
                    )
                    .and_then(|sampler| sampler.bind(&source).sample(request));
                    assert_eq!(prepared, generic, "s={s} t={t} filter={filter:?}");
                }
            }
        }
    }

    #[test]
    fn three_nearest_reads_only_the_selected_triangle_corners() {
        let tile = tile(mode(false, false), 1, 0, mode(false, false), 1, 0);
        let size = size(0, 0, 4, 4);
        for (request, invalid_address) in [
            (request(8, 8, TmemFirstRowParity::Even), 13),
            (request(24, 24, TmemFirstRowParity::Even), 0),
        ] {
            let source = InvalidCornerFixture {
                source: FilterFixture(PhysicalTmemState::try_new().unwrap()),
                invalid_address,
            };
            let generic = sample_texture(
                &source,
                tile,
                size,
                request,
                TextureLutMode::Disabled,
                TextureFilter::Bilinear,
            );
            let prepared = PreparedTextureSampler::try_new(
                tile,
                size,
                TextureLutMode::Disabled,
                TextureFilter::Bilinear,
            )
            .and_then(|sampler| sampler.bind(&source).sample(request));
            assert_eq!(prepared, generic);
            assert!(generic.is_ok(), "unused address {invalid_address} was read");
            assert!(sample_texture(
                &source,
                tile,
                size,
                request,
                TextureLutMode::Disabled,
                TextureFilter::Average,
            )
            .is_err());
        }
    }

    #[test]
    fn prepared_sampler_cache_collisions_reread_the_named_texel() {
        struct AddressFixture(PhysicalTmemState);

        impl TmemByteSource for AddressFixture {
            fn snapshot(&self) -> TmemSnapshotIdentity {
                self.0.snapshot()
            }

            fn valid_byte(&self, address: u16) -> Option<u8> {
                Some(address as u8)
            }
        }

        let source = AddressFixture(PhysicalTmemState::try_new().unwrap());
        let tile = tile(mode(false, false), 4, 0, mode(false, false), 4, 0);
        let size = size(0, 0, 60, 60);
        let requests = [
            request(8, 8, TmemFirstRowParity::Even),
            request(264, 264, TmemFirstRowParity::Even),
            request(8, 8, TmemFirstRowParity::Even),
        ];
        for filter in [
            TextureFilter::Point,
            TextureFilter::Bilinear,
            TextureFilter::Average,
        ] {
            let sampler = PreparedTextureSampler::try_new(
                tile,
                size,
                TextureLutMode::Disabled,
                filter,
            )
            .unwrap();
            let mut bound = sampler.bind(&source);
            for request in requests {
                assert_eq!(
                    bound.sample(request),
                    sample_texture(
                        &source,
                        tile,
                        size,
                        request,
                        TextureLutMode::Disabled,
                        filter,
                    )
                );
            }
        }
    }

    #[test]
    fn prepared_sampler_cache_key_preserves_every_address_field() {
        let base = AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Even);
        let column = AddressedTmemTexel::new(u16::MAX, 0, TmemFirstRowParity::Even);
        let row = AddressedTmemTexel::new(0, u16::MAX, TmemFirstRowParity::Even);
        let parity = AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Odd);

        let base_key = PreparedTexelCacheKey::new(base);
        assert_ne!(base_key, PreparedTexelCacheKey::new(column));
        assert_ne!(base_key, PreparedTexelCacheKey::new(row));
        assert_ne!(base_key, PreparedTexelCacheKey::new(parity));

        let colliding = AddressedTmemTexel::new(8, 8, TmemFirstRowParity::Even);
        assert_eq!(
            base_key.index(),
            PreparedTexelCacheKey::new(colliding).index()
        );
        assert_ne!(base_key, PreparedTexelCacheKey::new(colliding));
    }

    fn corner_coordinates(cell: AddressedTextureCell) -> [(u16, u16); 4] {
        cell.corners().map(|corner| (corner.column(), corner.row()))
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

    /// Pins the addressing of a texrect's RIGHTMOST column when it lands ONE
    /// texel past the tile's loaded S extent -- the shape task 35 inferred
    /// and task 36 built into the parity corpus
    /// (`gen-texrect-right-edge-overread-{clamp,wrap}`). The corpus proved
    /// wgpu == angrylion == RT64 on that column for both addressing modes;
    /// this test freezes the underlying texel address so a revert of the
    /// addressing math re-breaks it.
    ///
    /// A four-texel row (`low_s = 0`, `high_s = 3` texels, i.e. `3 << 2` in
    /// S10.2) sampled at S10.5 coordinate `4 << 5 = 128` -- integer texel 4,
    /// one past the loaded `[0, 3]` extent:
    /// - CLAMP (`mask_s == 0`) must clamp to the last loaded texel, column 3.
    /// - WRAP (`mask_s == 2`, non-clamp mode) must wrap to column 0.
    ///
    /// Neither is a read past the loaded extent: the address never lands on
    /// texel 4.
    #[test]
    fn right_edge_one_past_extent_addresses_within_the_loaded_row() {
        // high_s = 3 texels << 2 = 12 (S10.2); a four-texel row [0, 3].
        let four_texel_row = size(0, 0, 3 << 2, 0);
        // S10.5 coordinate at the over-wide rightmost column: texel index 4.
        let past_extent = 4i16 << 5;
        let t_center = 0i16;

        // CLAMP: mask_s == 0 forces the clamp arm regardless of mode.
        let clamp_tile = tile(mode(false, false), 0, 0, mode(false, false), 0, 0);
        let clamped = address_point_texel(
            clamp_tile,
            four_texel_row,
            request(past_extent, t_center, TmemFirstRowParity::Even),
        )
        .unwrap();
        assert_eq!(
            clamped.column(),
            3,
            "clamp must pin the rightmost over-wide column to the last loaded texel, not texel 4"
        );

        // WRAP: mask_s == 2 addresses [0, 3]; texel 4 wraps to column 0.
        let wrap_tile = tile(mode(false, false), 2, 0, mode(false, false), 2, 0);
        let wrapped = address_point_texel(
            wrap_tile,
            four_texel_row,
            request(past_extent, t_center, TmemFirstRowParity::Even),
        )
        .unwrap();
        assert_eq!(
            wrapped.column(),
            0,
            "wrap must fold the rightmost over-wide column back to texel 0, not read texel 4"
        );
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
    fn centered_cell_exposes_literal_fractions_corners_and_parity() {
        let cell = address_texture_cell(
            tile(mode(false, false), 0, 0, mode(false, false), 0, 0),
            size(0, 0, 4, 4),
            request(16, 16, TmemFirstRowParity::Odd),
        )
        .unwrap();
        assert_eq!(cell.fractions().s_five_bit(), 16);
        assert_eq!(cell.fractions().t_five_bit(), 16);
        assert_eq!(corner_coordinates(cell), [(0, 0), (0, 1), (1, 0), (1, 1)]);
        for corner in [
            TextureCellCorner::UpperLeft,
            TextureCellCorner::LowerLeft,
            TextureCellCorner::UpperRight,
            TextureCellCorner::LowerRight,
        ] {
            assert_eq!(
                cell.corner(corner).first_row_parity(),
                TmemFirstRowParity::Odd
            );
        }
        assert_eq!(TextureCellCorner::UpperLeft.to_string(), "upper-left");
        assert_eq!(TextureCellCorner::LowerRight.to_string(), "lower-right");
    }

    #[test]
    fn cell_shift_precedes_fractional_origin_with_literal_result() {
        let cell = address_texture_cell(
            tile(mode(false, false), 0, 1, mode(false, false), 0, 15),
            size(2, 1, 20, 20),
            request(208, 40, TmemFirstRowParity::Even),
        )
        .unwrap();
        assert_eq!(
            (cell.fractions().s_five_bit(), cell.fractions().t_five_bit()),
            (24, 8)
        );
        assert_eq!(corner_coordinates(cell), [(2, 2), (2, 3), (3, 2), (3, 3)]);
    }

    #[test]
    fn negative_cell_uses_euclidean_fraction_then_wraps_each_corner() {
        let cell = address_texture_cell(
            tile(mode(false, false), 2, 0, mode(false, false), 2, 0),
            size(0, 0, 0, 0),
            request(-1, -33, TmemFirstRowParity::Even),
        )
        .unwrap();
        assert_eq!(
            (cell.fractions().s_five_bit(), cell.fractions().t_five_bit()),
            (31, 31)
        );
        assert_eq!(corner_coordinates(cell), [(3, 2), (3, 3), (0, 2), (0, 3)]);
    }

    #[test]
    fn mirror_and_fractional_clamp_preserve_duplicate_semantic_corners() {
        let mirrored = address_texture_cell(
            tile(mode(true, false), 2, 0, mode(true, false), 2, 0),
            size(0, 0, 0, 0),
            request(127, 128, TmemFirstRowParity::Even),
        )
        .unwrap();
        assert_eq!(
            corner_coordinates(mirrored),
            [(3, 3), (3, 2), (3, 3), (3, 2)]
        );

        let clamped = address_texture_cell(
            tile(mode(false, true), 2, 0, mode(false, false), 0, 0),
            size(3, 5, 12, 18),
            request(151, 167, TmemFirstRowParity::Even),
        )
        .unwrap();
        assert_eq!(
            (
                clamped.fractions().s_five_bit(),
                clamped.fractions().t_five_bit()
            ),
            (31, 31)
        );
        assert_eq!(corner_coordinates(clamped), [(3, 3); 4]);
    }

    #[test]
    fn mask_fifteen_cell_boundaries_have_literal_axis_and_joint_rows() {
        let axis_cases = [
            (-32_768, 4, [(32_767, 0), (32_767, 32_767)]),
            (-1, 0, [(32_767, 0), (0, 0)]),
        ];
        for (raw, low_raw, expected_pairs) in axis_cases {
            for (mirror_index, mirror) in [false, true].into_iter().enumerate() {
                let s_cell = address_texture_cell(
                    tile(mode(mirror, false), 15, 11, mode(false, false), 1, 0),
                    size(low_raw, 0, 0, 0),
                    request(raw, 0, TmemFirstRowParity::Even),
                )
                .unwrap();
                assert_eq!(
                    (
                        s_cell.corner(TextureCellCorner::UpperLeft).column(),
                        s_cell.corner(TextureCellCorner::UpperRight).column()
                    ),
                    expected_pairs[mirror_index]
                );

                let t_cell = address_texture_cell(
                    tile(mode(false, false), 1, 0, mode(mirror, false), 15, 11),
                    size(0, low_raw, 0, 0),
                    request(0, raw, TmemFirstRowParity::Even),
                )
                .unwrap();
                assert_eq!(
                    (
                        t_cell.corner(TextureCellCorner::UpperLeft).row(),
                        t_cell.corner(TextureCellCorner::LowerLeft).row()
                    ),
                    expected_pairs[mirror_index]
                );
            }
        }

        let wrapped_joint = address_texture_cell(
            tile(mode(false, false), 15, 11, mode(false, false), 15, 11),
            size(4, 0, 0, 0),
            request(-32_768, -1, TmemFirstRowParity::Odd),
        )
        .unwrap();
        assert_eq!(
            corner_coordinates(wrapped_joint),
            [(32_767, 32_767), (32_767, 0), (0, 32_767), (0, 0)]
        );

        let mirrored_joint = address_texture_cell(
            tile(mode(true, false), 15, 11, mode(true, false), 15, 11),
            size(4, 0, 0, 0),
            request(-32_768, -1, TmemFirstRowParity::Odd),
        )
        .unwrap();
        assert_eq!(corner_coordinates(mirrored_joint), [(32_767, 0); 4]);
    }

    #[test]
    fn point_address_is_the_cell_upper_left_for_literal_modes_shifts_and_parities() {
        for shift in 0..=15 {
            for parity in [TmemFirstRowParity::Even, TmemFirstRowParity::Odd] {
                let tile = tile(mode(true, false), 15, shift, mode(false, true), 2, shift);
                let size = size(3, 5, 12, 18);
                let request = request(-65, 167, parity);
                let point = address_point_texel(tile, size, request).unwrap();
                let cell = address_texture_cell(tile, size, request).unwrap();
                assert_eq!(point, cell.corner(TextureCellCorner::UpperLeft));
            }
        }

        let literal_cases = [
            (
                tile(mode(false, false), 1, 0, mode(false, false), 1, 0),
                size(0, 0, 0, 0),
                request(-1, 95, TmemFirstRowParity::Odd),
                AddressedTmemTexel::new(1, 0, TmemFirstRowParity::Odd),
            ),
            (
                tile(mode(true, false), 0, 0, mode(true, false), 0, 0),
                size(4, 8, 12, 20),
                request(-1024, 1024, TmemFirstRowParity::Even),
                AddressedTmemTexel::new(0, 3, TmemFirstRowParity::Even),
            ),
        ];
        for (tile, size, request, expected) in literal_cases {
            let point = address_point_texel(tile, size, request).unwrap();
            let cell = address_texture_cell(tile, size, request).unwrap();
            assert_eq!(point, expected);
            assert_eq!(cell.corner(TextureCellCorner::UpperLeft), expected);
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

        let cell = address_texture_cell(
            tile(mode(false, false), 2, 0, mode(false, false), 3, 0),
            size(12, 12, 4, 4),
            request(143, 79, TmemFirstRowParity::Odd),
        )
        .unwrap();
        assert_eq!(
            (cell.fractions().s_five_bit(), cell.fractions().t_five_bit()),
            (15, 15)
        );
        assert_eq!(corner_coordinates(cell), [(1, 7), (1, 0), (2, 7), (2, 0)]);
        assert_eq!(
            cell.corner(TextureCellCorner::LowerRight)
                .first_row_parity(),
            TmemFirstRowParity::Odd
        );
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

        let cell_error = gather_committed_texture_cell(
            &state,
            tile(mode(false, false), 0, 0, mode(false, false), 1, 0),
            size(12, 0, 4, 0),
            request(0, 0, TmemFirstRowParity::Even),
            TextureLutMode::Disabled,
        )
        .unwrap_err();
        assert_eq!(
            cell_error,
            TextureCellSampleError::Address(PointAddressError::ReversedClampExtent {
                axis: TextureAxis::S,
                low_raw: 12,
                high_raw: 4,
            })
        );
        assert!(!cell_error.to_string().is_empty());
        assert!(std::error::Error::source(&cell_error).is_some());

        assert_eq!(
            address_texture_cell(
                tile(mode(false, false), 1, 0, mode(false, false), 0, 0),
                size(12, 12, 4, 4),
                request(0, 0, TmemFirstRowParity::Even),
            ),
            Err(PointAddressError::ReversedClampExtent {
                axis: TextureAxis::T,
                low_raw: 12,
                high_raw: 4,
            })
        );
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
                state.identity(),
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
            // Row 0 does not exchange, whatever first-row parity the caller
            // supplies, so the addressed byte is 0 -- see
            // `tmem/read.rs::odd_row_exchange`. This asserted 4 while the
            // reader still folded the caller's parity into the exchange.
            Err(PointSampleError::Read(
                PhysicalTexelReadError::InvalidTexelByte { address: 0 }
            ))
        );
        let cell_error = gather_committed_texture_cell(
            &state,
            tile(mode(false, false), 0, 0, mode(false, false), 0, 0),
            size(0, 0, 4, 4),
            request(0, 0, TmemFirstRowParity::Odd),
            TextureLutMode::Disabled,
        )
        .unwrap_err();
        assert_eq!(
            cell_error,
            TextureCellSampleError::Read {
                corner: TextureCellCorner::UpperLeft,
                // Row 0 does not exchange; see the sibling assertion above.
                source: PhysicalTexelReadError::InvalidTexelByte { address: 0 },
            }
        );
        assert_eq!(
            std::error::Error::source(&cell_error).unwrap().to_string(),
            "physical TMEM texel byte 0x000 is invalid"
        );
        assert_eq!(observe(&state), before);
    }

    #[test]
    fn three_nearest_filter_selects_lower_left_and_upper_right_triangles() {
        let corners = |value: u8| [value; 4];
        let c_ul = corners(100);
        let c_ur = corners(150);
        let c_ll = corners(50);
        let c_lr = corners(200);

        // sf+tf=16 <= 32 selects the lower-left triangle:
        // value = 100*32 + 8*(150-100) + 8*(50-100) = 3200 + 400 - 400 = 3200
        // result = round(3200/32) = 100
        assert_eq!(
            filter_three_nearest([c_ul, c_ur, c_ll, c_lr], 8, 8),
            corners(100)
        );

        // sf+tf=48 > 32 selects the upper-right triangle:
        // value = 200*32 + 8*(50-200) + 8*(150-200) = 6400 - 1200 - 400 = 4800
        // result = round(4800/32) = 150
        assert_eq!(
            filter_three_nearest([c_ul, c_ur, c_ll, c_lr], 24, 24),
            corners(150)
        );
    }

    #[test]
    fn three_nearest_filter_matches_the_reference_lane_across_every_fraction_and_seed() {
        let mut lower_half = 0usize;
        let mut upper_half = 0usize;
        for seed in 0..=255u16 {
            let values = [
                seed as u8,
                seed.wrapping_mul(73).wrapping_add(19) as u8,
                seed.wrapping_mul(151).wrapping_add(41) as u8,
                seed.wrapping_mul(211).wrapping_add(97) as u8,
            ];
            // fn64-render-reference's `[c00, c10, c01, c11]` order: this
            // sweep drives `filter_three_nearest` directly in that already-
            // remapped order, matching filter_three_nearest_committed_cell's
            // own corner remap under test elsewhere.
            let [c00, c10, c01, c11] = values.map(|value| [value; 4]);
            for sf in 0..32i64 {
                for tf in 0..32i64 {
                    let [c00f, c10f, c01f, c11f] = values.map(f32::from);
                    let sf_float = sf as f32 / 32.0;
                    let tf_float = tf as f32 / 32.0;
                    let expected = if sf + tf <= 32 {
                        lower_half += 1;
                        c00f + sf_float * (c10f - c00f) + tf_float * (c01f - c00f)
                    } else {
                        upper_half += 1;
                        c11f + (1.0 - sf_float) * (c01f - c11f) + (1.0 - tf_float) * (c10f - c11f)
                    }
                    .round()
                    .clamp(0.0, 255.0) as u8;
                    assert_eq!(
                        filter_three_nearest([c00, c10, c01, c11], sf, tf),
                        [expected; 4],
                        "seed={seed} sf={sf}/32 tf={tf}/32"
                    );
                }
            }
        }
        assert_eq!((lower_half, upper_half), (143_104, 119_040));
    }
}
