//! Durable physical-TMEM texel reads.
//!
//! This layer binds M4.3.3a/b's pure direct and indexed decoders to one
//! immutable `(PhysicalTmemStateIdentity, generation)` snapshot. Addressing
//! follows the public SGI *Nintendo 64 RDP Command Summary* tile/TMEM fields
//! and this crate's already-frozen LoadTile placement: 64-bit line strides,
//! caller-supplied first-row parity, odd-row XOR4, 12-bit linear wrapping,
//! and RGBA32's split low/high 2 KiB banks. Enabled CI lookup is restricted to
//! the canonical low-half source and `0x800 + index * 8` TLUT placement frozen
//! by M4.3.3b. Requiring every quadricated lane to be valid and equal is a
//! conservative admitted subset; partial/unequal sample-lane behavior remains
//! deferred to hardware measurement. RT64 is not physical-memory authority
//! for this reader.
//!
//! ## Committed and proposed byte sources
//!
//! Every read here is expressed against [`TmemByteSource`] -- the exact
//! three-method surface the reader ever touched on `PhysicalTmemState`
//! (`identity`/`generation`, collapsed into one `snapshot()`, plus
//! `valid_byte`). Two sources implement it and they are **not**
//! interchangeable identities:
//!
//! - [`PhysicalTmemState`] answers with a
//!   [`TmemSnapshotIdentity::Committed`] naming its own durable
//!   `(state, generation)` pair.
//! - [`super::PendingTmemImage`] -- the post-image of a sealed but
//!   unpublished [`super::PendingTmemTransaction`] -- answers with a
//!   [`TmemSnapshotIdentity::Proposed`] naming that transaction's
//!   `proposal_identity` and its binding.
//!
//! That split is the whole reason a pending read is admissible at all. A
//! pending post-image has no durable `(state, generation)` of its own:
//! `binding.state` still names the *base* state and
//! `binding.next_generation` names a generation that does not exist yet and
//! never will if publication is rejected. Answering a pending read with
//! that pair would mint a `PhysicalTmemSnapshotIdentity` for a snapshot
//! nothing ever published -- a forged receipt, indistinguishable downstream
//! from a real one. `TmemSnapshotIdentity` makes the two cases distinct
//! *types* of answer instead, so no consumer can silently accept a proposal
//! where it required a publication.
//!
//! The decode, addressing, validity, XOR4, RGBA32-bank, and TLUT logic is
//! shared verbatim between the two: there is exactly one
//! [`read_texel`] and the committed entry point
//! [`read_committed_texel`] is a thin monomorphization of it.

use core::fmt;

use crate::{ImageFormat, PixelSize, TextureLutMode};

use super::{
    decode_direct_texel, decode_tlut_entry, resolve_indexed_texel, unpack_ci4_texel, Ci4Palette,
    Ci4PaletteError, Ci4UnpackError, DecodedTexel, DirectTexelDecodeError,
    IndexedTexelResolveError, PhysicalTmemState, PhysicalTmemStateIdentity,
    PhysicalTmemTransactionIdentity, RawTexel, RawTexelError, ResolvedIndexedTexel,
    TexelColumnParity, TileDescriptor, TlutEntryDecodeError,
};
use fn64_render_ir::ContentDigest;

const TMEM_ADDRESS_MASK: u64 = 0x0fff;
const TMEM_LOW_HALF_MASK: u64 = 0x07ff;
const TMEM_HIGH_HALF_BASE: u16 = 0x0800;

/// Parity of the first addressed row relative to TMEM's XOR4 exchange.
///
/// This is explicit caller input. The reader never infers it from a tile
/// coordinate or a preceding load command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TmemFirstRowParity {
    Even,
    Odd,
}

/// One already-addressed integer texel coordinate and its first-row parity.
///
/// Coordinate normalization (shift, mask, mirror, clamp), sampling,
/// filtering, and LOD are deliberately outside this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AddressedTmemTexel {
    column: u16,
    row: u16,
    first_row_parity: TmemFirstRowParity,
}

impl AddressedTmemTexel {
    pub const fn new(column: u16, row: u16, first_row_parity: TmemFirstRowParity) -> Self {
        Self {
            column,
            row,
            first_row_parity,
        }
    }

    pub const fn column(self) -> u16 {
        self.column
    }

    pub const fn row(self) -> u16 {
        self.row
    }

    pub const fn first_row_parity(self) -> TmemFirstRowParity {
        self.first_row_parity
    }
}

/// Identity of the exact durable physical-TMEM snapshot used by one read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhysicalTmemSnapshotIdentity {
    state: PhysicalTmemStateIdentity,
    generation: u64,
}

impl PhysicalTmemSnapshotIdentity {
    const fn new(state: PhysicalTmemStateIdentity, generation: u64) -> Self {
        Self { state, generation }
    }

    pub const fn state(self) -> PhysicalTmemStateIdentity {
        self.state
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }
}

/// Identity of a proposed, not-yet-published physical-TMEM post-image.
///
/// Deliberately **not** a `PhysicalTmemSnapshotIdentity`. A pending
/// transaction's post-image is not a durable state: it carries the *base*
/// state's identity and a `next_generation` that no `PhysicalTmemState`
/// holds yet. Naming it with the durable snapshot type would let a
/// proposal's receipt compare equal to a publication's, which is exactly
/// the confusion the committed/pending split exists to prevent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProposedTmemImageIdentity {
    proposal: ContentDigest,
    base_state: PhysicalTmemStateIdentity,
    transaction: PhysicalTmemTransactionIdentity,
    next_generation: u64,
}

impl ProposedTmemImageIdentity {
    pub(super) const fn new(
        proposal: ContentDigest,
        base_state: PhysicalTmemStateIdentity,
        transaction: PhysicalTmemTransactionIdentity,
        next_generation: u64,
    ) -> Self {
        Self {
            proposal,
            base_state,
            transaction,
            next_generation,
        }
    }

    /// The sealed transaction's own `proposal_identity` content digest --
    /// the same value `validate_proposal` recomputes and `publish`
    /// re-checks, so a proposal receipt cannot outlive a mutation of the
    /// proposal it names.
    pub const fn proposal(self) -> ContentDigest {
        self.proposal
    }

    /// The state this transaction staged *from*. Not the state a
    /// publication would produce -- that state does not exist yet.
    pub const fn base_state(self) -> PhysicalTmemStateIdentity {
        self.base_state
    }

    pub const fn transaction(self) -> PhysicalTmemTransactionIdentity {
        self.transaction
    }

    /// The generation a successful publication *would* reach. Carried for
    /// diagnosis only; it names nothing observable until publication.
    pub const fn next_generation(self) -> u64 {
        self.next_generation
    }
}

/// Which kind of physical-TMEM image one read observed.
///
/// The two variants are the whole committed/pending distinction, reified.
/// A consumer that requires durable state matches `Committed` and rejects
/// `Proposed` by name; a consumer executing inside the staging window
/// accepts `Proposed` and records the proposal it read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TmemSnapshotIdentity {
    Committed(PhysicalTmemSnapshotIdentity),
    Proposed(ProposedTmemImageIdentity),
}

impl TmemSnapshotIdentity {
    /// The durable snapshot this read observed, or `None` when it observed
    /// a proposal instead. Deliberately an `Option` rather than a
    /// synthesized snapshot: there is no durable snapshot to return for a
    /// proposal, and manufacturing one is the forgery this type prevents.
    pub const fn committed(self) -> Option<PhysicalTmemSnapshotIdentity> {
        match self {
            Self::Committed(snapshot) => Some(snapshot),
            Self::Proposed(_) => None,
        }
    }

    pub const fn proposed(self) -> Option<ProposedTmemImageIdentity> {
        match self {
            Self::Proposed(identity) => Some(identity),
            Self::Committed(_) => None,
        }
    }

    pub const fn is_committed(self) -> bool {
        matches!(self, Self::Committed(_))
    }
}

/// A real `Proposed` snapshot identity, for a test that must hand a
/// [`TmemByteSource`] an identity it could not otherwise construct.
///
/// Every constructor on the way to one is module-private -- deliberately,
/// so a proposal's receipt cannot be forged from outside `tmem`. That
/// privacy is also what makes `production.rs`'s
/// `CommittedTmemImageClaimedProposed` refusal untestable from there, which
/// is what this exists for. `#[cfg(test)]`, so no non-test caller can reach
/// it and the privacy holds where it matters.
///
/// Built from this module's own real types, not a stand-in: a test using it
/// exercises the same `is_committed()` discrimination a live proposal does.
#[cfg(test)]
pub(crate) fn proposed_identity_for_test() -> TmemSnapshotIdentity {
    TmemSnapshotIdentity::Proposed(ProposedTmemImageIdentity {
        proposal: ContentDigest::from_bytes([0x5a; 32]),
        base_state: super::PhysicalTmemState::try_new()
            .expect("a fresh physical state allocates")
            .identity(),
        transaction: super::physical::next_transaction_identity_for_test(),
        next_generation: 1,
    })
}

/// The complete surface [`read_texel`] observes on a physical-TMEM image.
///
/// Exactly the three things the reader ever asked `PhysicalTmemState` for,
/// no more: its snapshot identity, and one byte's validity-gated value.
/// Keeping the surface this narrow is what makes a pending post-image a
/// legal source without relaxing anything -- a pending image answers
/// `valid_byte` from the same `bytes`/`valid` arrays a publication would
/// install, so the two agree byte-for-byte by construction, and differ only
/// in what they claim about *durability*.
pub trait TmemByteSource {
    /// This image's identity. Committed for durable state, proposed for a
    /// sealed-but-unpublished transaction post-image.
    fn snapshot(&self) -> TmemSnapshotIdentity;

    /// Returns a byte only when its latest complete-word touch defined it.
    /// Out-of-range and invalid storage are both unobservable, identically
    /// for both sources.
    fn valid_byte(&self, address: u16) -> Option<u8>;
}

impl TmemByteSource for PhysicalTmemState {
    fn snapshot(&self) -> TmemSnapshotIdentity {
        TmemSnapshotIdentity::Committed(PhysicalTmemSnapshotIdentity::new(
            self.identity(),
            self.generation(),
        ))
    }

    fn valid_byte(&self, address: u16) -> Option<u8> {
        PhysicalTmemState::valid_byte(self, address)
    }
}

/// One decoded color bound to the physical-TMEM image it read.
///
/// Named `DecodedPhysicalTexel` unchanged, but its `snapshot` is now a
/// [`TmemSnapshotIdentity`]: a caller that previously read
/// `texel.snapshot()` and required a durable pair now reads
/// `texel.snapshot().committed()` and handles the proposal case, rather
/// than being handed a fabricated durable pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodedPhysicalTexel {
    snapshot: TmemSnapshotIdentity,
    texel: DecodedTexel,
}

impl DecodedPhysicalTexel {
    pub const fn snapshot(self) -> TmemSnapshotIdentity {
        self.snapshot
    }

    pub const fn texel(self) -> DecodedTexel {
        self.texel
    }
}

/// Why a committed physical texel could not be read and decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalTexelReadError {
    Direct(DirectTexelDecodeError),
    Indexed(IndexedTexelResolveError),
    Ci4Palette(Ci4PaletteError),
    Ci4Unpack(Ci4UnpackError),
    Raw(RawTexelError),
    TlutEntry(TlutEntryDecodeError),
    InvalidTexelByte { address: u16 },
    Rgba32BaseOutsideLowHalf { byte_address: u16 },
    EnabledCiSourceOutsideLowHalf { byte_address: u16 },
    IncompleteTlutEntry { byte_address: u16, valid_mask: u8 },
    NonCanonicalTlutEntry { byte_address: u16, lanes: [u16; 4] },
}

impl fmt::Display for PhysicalTexelReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Direct(error) => error.fmt(formatter),
            Self::Indexed(error) => error.fmt(formatter),
            Self::Ci4Palette(error) => error.fmt(formatter),
            Self::Ci4Unpack(error) => error.fmt(formatter),
            Self::Raw(error) => error.fmt(formatter),
            Self::TlutEntry(error) => error.fmt(formatter),
            Self::InvalidTexelByte { address } => {
                write!(formatter, "physical TMEM texel byte {address:#05x} is invalid")
            }
            Self::Rgba32BaseOutsideLowHalf { byte_address } => write!(
                formatter,
                "RGBA32 tile base {byte_address:#05x} is outside low-half TMEM"
            ),
            Self::EnabledCiSourceOutsideLowHalf { byte_address } => write!(
                formatter,
                "enabled-TLUT CI source byte {byte_address:#05x} is outside canonical low-half TMEM"
            ),
            Self::IncompleteTlutEntry {
                byte_address,
                valid_mask,
            } => write!(
                formatter,
                "TLUT entry at {byte_address:#05x} requires all eight valid bytes, found mask {valid_mask:#04x}"
            ),
            Self::NonCanonicalTlutEntry {
                byte_address,
                lanes,
            } => write!(
                formatter,
                "TLUT entry at {byte_address:#05x} is not four equal big-endian 16-bit lanes: {lanes:04x?}"
            ),
        }
    }
}

impl std::error::Error for PhysicalTexelReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Direct(error) => Some(error),
            Self::Indexed(error) => Some(error),
            Self::Ci4Palette(error) => Some(error),
            Self::Ci4Unpack(error) => Some(error),
            Self::Raw(error) => Some(error),
            Self::TlutEntry(error) => Some(error),
            Self::InvalidTexelByte { .. }
            | Self::Rgba32BaseOutsideLowHalf { .. }
            | Self::EnabledCiSourceOutsideLowHalf { .. }
            | Self::IncompleteTlutEntry { .. }
            | Self::NonCanonicalTlutEntry { .. } => None,
        }
    }
}

impl From<RawTexelError> for PhysicalTexelReadError {
    fn from(error: RawTexelError) -> Self {
        Self::Raw(error)
    }
}

impl From<DirectTexelDecodeError> for PhysicalTexelReadError {
    fn from(error: DirectTexelDecodeError) -> Self {
        Self::Direct(error)
    }
}

impl From<IndexedTexelResolveError> for PhysicalTexelReadError {
    fn from(error: IndexedTexelResolveError) -> Self {
        Self::Indexed(error)
    }
}

impl From<Ci4PaletteError> for PhysicalTexelReadError {
    fn from(error: Ci4PaletteError) -> Self {
        Self::Ci4Palette(error)
    }
}

impl From<Ci4UnpackError> for PhysicalTexelReadError {
    fn from(error: Ci4UnpackError) -> Self {
        Self::Ci4Unpack(error)
    }
}

impl From<TlutEntryDecodeError> for PhysicalTexelReadError {
    fn from(error: TlutEntryDecodeError) -> Self {
        Self::TlutEntry(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadKind {
    Direct,
    Indexed { palette: Ci4Palette },
}

/// Reads and decodes one texel from durable, already-published physical TMEM.
///
/// The format/size/TLUT combination is preflighted through the existing pure
/// decoders before any physical byte is observed. Every byte in the selected
/// footprint must be valid; its touch generation may precede the durable
/// state's current generation. Enabled CI additionally requires all eight
/// bytes of the conservative canonical quadricated TLUT subset to be valid and
/// its four big-endian 16-bit lanes to agree.
pub fn read_committed_texel(
    state: &PhysicalTmemState,
    tile: TileDescriptor,
    addressed: AddressedTmemTexel,
    lut_mode: TextureLutMode,
) -> Result<DecodedPhysicalTexel, PhysicalTexelReadError> {
    read_texel(state, tile, addressed, lut_mode)
}

/// The one texel reader, over any [`TmemByteSource`].
///
/// [`read_committed_texel`] above is this function at
/// `S = PhysicalTmemState`, and a pending post-image read is this function
/// at `S = super::PendingTmemImage`. There is deliberately no second copy
/// of the addressing, validity, XOR4, RGBA32-bank, or TLUT logic: a
/// pending read that disagreed with a committed read of the same bytes
/// would be a defect no test comparing the two could distinguish from a
/// deliberate difference, so the two cases are made incapable of
/// disagreeing rather than checked for agreement.
pub fn read_texel<S: TmemByteSource + ?Sized>(
    state: &S,
    tile: TileDescriptor,
    addressed: AddressedTmemTexel,
    lut_mode: TextureLutMode,
) -> Result<DecodedPhysicalTexel, PhysicalTexelReadError> {
    let kind = preflight(tile, lut_mode)?;
    validate_address_scope(tile, addressed, lut_mode, kind)?;
    let snapshot = state.snapshot();
    let texel = match kind {
        ReadKind::Direct => {
            let raw = read_raw_texel(state, tile, addressed)?;
            decode_direct_texel(tile.format(), raw)?
        }
        ReadKind::Indexed { palette } => {
            let raw_index = read_raw_texel(state, tile, addressed)?;
            match resolve_indexed_texel(tile.format(), raw_index, palette, lut_mode)? {
                ResolvedIndexedTexel::Direct(texel) => texel,
                ResolvedIndexedTexel::Tlut(lookup) => {
                    let entry = read_canonical_tlut_entry(state, lookup.byte_address())?;
                    decode_tlut_entry(lookup, entry)?
                }
            }
        }
    };
    Ok(DecodedPhysicalTexel { snapshot, texel })
}

fn preflight(
    tile: TileDescriptor,
    lut_mode: TextureLutMode,
) -> Result<ReadKind, PhysicalTexelReadError> {
    let zero = RawTexel::try_new(tile.size(), 0)?;
    if lut_mode == TextureLutMode::Disabled && tile.format() != ImageFormat::ColorIndex {
        decode_direct_texel(tile.format(), zero)?;
        return Ok(ReadKind::Direct);
    }
    let palette = Ci4Palette::try_new(tile.palette())?;
    resolve_indexed_texel(tile.format(), zero, palette, lut_mode)?;
    Ok(ReadKind::Indexed { palette })
}

fn validate_address_scope(
    tile: TileDescriptor,
    addressed: AddressedTmemTexel,
    lut_mode: TextureLutMode,
    kind: ReadKind,
) -> Result<(), PhysicalTexelReadError> {
    let base = tile.tmem().get() * 8;
    if matches!(kind, ReadKind::Direct) && tile.size() == PixelSize::Bits32 && base >= 0x0800 {
        return Err(PhysicalTexelReadError::Rgba32BaseOutsideLowHalf { byte_address: base });
    }
    if matches!(kind, ReadKind::Indexed { .. }) && lut_mode != TextureLutMode::Disabled {
        let address = first_physical_byte(tile, addressed);
        if address >= TMEM_HIGH_HALF_BASE {
            return Err(PhysicalTexelReadError::EnabledCiSourceOutsideLowHalf {
                byte_address: address,
            });
        }
    }
    Ok(())
}

fn read_raw_texel<S: TmemByteSource + ?Sized>(
    state: &S,
    tile: TileDescriptor,
    addressed: AddressedTmemTexel,
) -> Result<RawTexel, PhysicalTexelReadError> {
    match tile.size() {
        PixelSize::Bits4 => {
            let packed = RawTexel::try_new(
                PixelSize::Bits8,
                u32::from(read_valid_byte(
                    state,
                    first_physical_byte(tile, addressed),
                )?),
            )?;
            let parity = if addressed.column() & 1 == 0 {
                TexelColumnParity::Even
            } else {
                TexelColumnParity::Odd
            };
            Ok(unpack_ci4_texel(packed, parity)?)
        }
        PixelSize::Bits8 => RawTexel::try_new(
            PixelSize::Bits8,
            u32::from(read_valid_byte(
                state,
                first_physical_byte(tile, addressed),
            )?),
        )
        .map_err(Into::into),
        PixelSize::Bits16 => {
            let bytes = read_linear_bytes::<2, S>(state, tile, addressed)?;
            RawTexel::try_new(PixelSize::Bits16, u32::from(u16::from_be_bytes(bytes)))
                .map_err(Into::into)
        }
        PixelSize::Bits32 => {
            let low = rgba32_low_address(tile, addressed);
            let bytes = [
                read_valid_byte(state, low)?,
                read_valid_byte(state, low.wrapping_add(1))?,
                read_valid_byte(state, low + TMEM_HIGH_HALF_BASE)?,
                read_valid_byte(state, low + TMEM_HIGH_HALF_BASE + 1)?,
            ];
            RawTexel::try_new(PixelSize::Bits32, u32::from_be_bytes(bytes)).map_err(Into::into)
        }
    }
}

fn read_linear_bytes<const N: usize, S: TmemByteSource + ?Sized>(
    state: &S,
    tile: TileDescriptor,
    addressed: AddressedTmemTexel,
) -> Result<[u8; N], PhysicalTexelReadError> {
    let linear = linear_byte_address(tile, addressed);
    let exchange = odd_row_exchange(addressed);
    let mut bytes = [0; N];
    for (offset, byte) in bytes.iter_mut().enumerate() {
        let address = ((linear + offset as u64) & TMEM_ADDRESS_MASK) as u16;
        let address = if exchange { address ^ 4 } else { address };
        *byte = read_valid_byte(state, address)?;
    }
    Ok(bytes)
}

fn read_valid_byte<S: TmemByteSource + ?Sized>(
    state: &S,
    address: u16,
) -> Result<u8, PhysicalTexelReadError> {
    state
        .valid_byte(address)
        .ok_or(PhysicalTexelReadError::InvalidTexelByte { address })
}

fn read_canonical_tlut_entry<S: TmemByteSource + ?Sized>(
    state: &S,
    byte_address: u16,
) -> Result<RawTexel, PhysicalTexelReadError> {
    let mut bytes = [0; 8];
    let mut valid_mask = 0_u8;
    for (lane, byte) in bytes.iter_mut().enumerate() {
        let address = byte_address + lane as u16;
        if let Some(value) = state.valid_byte(address) {
            *byte = value;
            valid_mask |= 1 << lane;
        }
    }
    if valid_mask != u8::MAX {
        return Err(PhysicalTexelReadError::IncompleteTlutEntry {
            byte_address,
            valid_mask,
        });
    }
    let lanes = [
        u16::from_be_bytes([bytes[0], bytes[1]]),
        u16::from_be_bytes([bytes[2], bytes[3]]),
        u16::from_be_bytes([bytes[4], bytes[5]]),
        u16::from_be_bytes([bytes[6], bytes[7]]),
    ];
    if lanes[1..].iter().any(|lane| *lane != lanes[0]) {
        return Err(PhysicalTexelReadError::NonCanonicalTlutEntry {
            byte_address,
            lanes,
        });
    }
    RawTexel::try_new(PixelSize::Bits16, u32::from(lanes[0])).map_err(Into::into)
}

fn first_physical_byte(tile: TileDescriptor, addressed: AddressedTmemTexel) -> u16 {
    if tile.size() == PixelSize::Bits32 {
        return rgba32_low_address(tile, addressed);
    }
    let address = (linear_byte_address(tile, addressed) & TMEM_ADDRESS_MASK) as u16;
    if odd_row_exchange(addressed) {
        address ^ 4
    } else {
        address
    }
}

fn rgba32_low_address(tile: TileDescriptor, addressed: AddressedTmemTexel) -> u16 {
    let address = (linear_byte_address(tile, addressed) & TMEM_LOW_HALF_MASK) as u16;
    if odd_row_exchange(addressed) {
        address ^ 4
    } else {
        address
    }
}

fn linear_byte_address(tile: TileDescriptor, addressed: AddressedTmemTexel) -> u64 {
    let bytes_per_texel = match tile.size() {
        PixelSize::Bits4 => 0,
        PixelSize::Bits8 => 1,
        PixelSize::Bits16 | PixelSize::Bits32 => 2,
    };
    let column_offset = if tile.size() == PixelSize::Bits4 {
        u64::from(addressed.column() / 2)
    } else {
        u64::from(addressed.column()) * bytes_per_texel
    };
    u64::from(tile.tmem().get()) * 8
        + u64::from(addressed.row()) * u64::from(tile.line_words()) * 8
        + column_offset
}

fn odd_row_exchange(addressed: AddressedTmemTexel) -> bool {
    let first_is_odd = addressed.first_row_parity() == TmemFirstRowParity::Odd;
    first_is_odd ^ (addressed.row() & 1 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TileAddressMode, TmemWordAddress};

    /// A sparse byte source for end-to-end reader tests. Only the bytes
    /// explicitly written are valid; every other address reads as undefined,
    /// exactly like untouched physical TMEM.
    struct SparseSource(std::collections::BTreeMap<u16, u8>);

    impl SparseSource {
        fn new() -> Self {
            Self(std::collections::BTreeMap::new())
        }

        fn write(&mut self, address: u16, value: u8) {
            self.0.insert(address, value);
        }

        /// Writes all four big-endian lanes of one canonical quadricated
        /// TLUT entry, which the reader requires to be valid and to agree.
        fn write_quadricated_entry(&mut self, index: u8, value: u16) {
            let base = 0x0800 + u16::from(index) * 8;
            let [high, low] = value.to_be_bytes();
            for lane in 0..4 {
                self.write(base + lane * 2, high);
                self.write(base + lane * 2 + 1, low);
            }
        }
    }

    impl TmemByteSource for SparseSource {
        fn snapshot(&self) -> TmemSnapshotIdentity {
            PhysicalTmemState::try_new().unwrap().snapshot()
        }

        fn valid_byte(&self, address: u16) -> Option<u8> {
            self.0.get(&address).copied()
        }
    }

    /// POSITIVE CONTROL for WM2000's failing texrect, end to end through the
    /// physical reader rather than the pure value layer.
    ///
    /// The tile below is genuinely `IntensityAlpha`/`Bits8` -- asserted, not
    /// assumed -- and the mode is genuinely an enabled TLUT. Before the
    /// tlut_en fix this exact call returned
    /// `FormatMustBeColorIndex { format: IntensityAlpha }`, which is the
    /// error that aborted the all-Rust stack.
    ///
    /// The expected color is hand-derived from the RGBA16 (5/5/5/1) bit
    /// layout, never captured: entry `0xF801` is
    /// `r=0b11111, g=0b00000, b=0b00000, a=1`, and 5-bit replication
    /// (`v << 3 | v >> 2`) sends `0b11111` to `0xFF` and `0b00000` to `0x00`,
    /// giving opaque red.
    #[test]
    fn enabled_tlut_over_a_non_ci_tile_reaches_the_tlut_lookup() {
        let subject = tile(ImageFormat::IntensityAlpha, PixelSize::Bits8, 0, 0);
        assert_eq!(
            subject.format(),
            ImageFormat::IntensityAlpha,
            "positive control must bind a genuinely IntensityAlpha tile"
        );
        assert_eq!(subject.size(), PixelSize::Bits8);

        let addressed = AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Even);
        let mut source = SparseSource::new();
        source.write(0, 0x42);
        source.write_quadricated_entry(0x42, 0xf801);

        let decoded = read_texel(&source, subject, addressed, TextureLutMode::Rgba16)
            .expect("enabled TLUT ignores the tile format");
        assert_eq!(decoded.texel().rgba8888(), [0xff, 0x00, 0x00, 0xff]);
    }

    /// The companion refutation: the SAME tile and bytes with the TLUT
    /// DISABLED must still refuse by format, so the test above cannot pass
    /// by the format check having been deleted outright.
    #[test]
    fn disabled_tlut_over_the_same_non_ci_tile_still_refuses_by_format() {
        let subject = tile(ImageFormat::IntensityAlpha, PixelSize::Bits8, 0, 0);
        let addressed = AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Even);
        let mut source = SparseSource::new();
        source.write(0, 0x42);

        // IA8 is a legal direct pair, so a disabled TLUT decodes it
        // directly rather than refusing -- the refusal below is the CI
        // alias's, reached only by a format with no direct decode.
        assert!(read_texel(&source, subject, addressed, TextureLutMode::Disabled).is_ok());

        let ci16 = tile(ImageFormat::ColorIndex, PixelSize::Bits16, 0, 0);
        assert!(matches!(
            read_texel(&source, ci16, addressed, TextureLutMode::Disabled),
            Err(PhysicalTexelReadError::Indexed(
                IndexedTexelResolveError::UnsupportedIndexSize { .. }
            ))
        ));
    }

    fn tile(format: ImageFormat, size: PixelSize, tmem: u16, palette: u8) -> TileDescriptor {
        TileDescriptor::from_wire(
            format,
            size,
            1,
            TmemWordAddress::try_new(tmem).unwrap(),
            palette,
            TileAddressMode::default(),
            0,
            0,
            TileAddressMode::default(),
            0,
            0,
        )
    }

    #[test]
    fn addressed_texel_and_snapshot_fields_are_typed_and_observable() {
        let addressed = AddressedTmemTexel::new(7, 9, TmemFirstRowParity::Odd);
        assert_eq!(addressed.column(), 7);
        assert_eq!(addressed.row(), 9);
        assert_eq!(addressed.first_row_parity(), TmemFirstRowParity::Odd);

        let state = PhysicalTmemState::try_new().unwrap();
        let snapshot = PhysicalTmemSnapshotIdentity::new(state.identity(), state.generation());
        assert_eq!(snapshot.state(), state.identity());
        assert_eq!(snapshot.generation(), 0);
    }

    #[test]
    fn unsupported_pairs_are_rejected_before_empty_state_validity() {
        let state = PhysicalTmemState::try_new().unwrap();
        let addressed = AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Even);

        assert!(matches!(
            read_committed_texel(
                &state,
                tile(ImageFormat::Yuv, PixelSize::Bits16, 0, 0),
                addressed,
                TextureLutMode::Disabled,
            ),
            Err(PhysicalTexelReadError::Direct(
                DirectTexelDecodeError::YuvConversionDeferred { .. }
            ))
        ));
        assert!(matches!(
            read_committed_texel(
                &state,
                tile(ImageFormat::Rgba, PixelSize::Bits8, 0, 0),
                addressed,
                TextureLutMode::Disabled,
            ),
            Err(PhysicalTexelReadError::Direct(
                DirectTexelDecodeError::UnsupportedPair { .. }
            ))
        ));
        assert!(matches!(
            read_committed_texel(
                &state,
                tile(ImageFormat::ColorIndex, PixelSize::Bits16, 0, 0),
                addressed,
                TextureLutMode::Disabled,
            ),
            Err(PhysicalTexelReadError::Indexed(
                IndexedTexelResolveError::UnsupportedIndexSize { .. }
            ))
        ));
        // A non-CI tile under a DISABLED TLUT still refuses by format: the
        // CI-to-I8 alias is genuinely format-specific. Under an ENABLED
        // TLUT the format is ignored instead, covered by
        // `enabled_tlut_over_a_non_ci_tile_reaches_the_tlut_lookup` below.
        assert!(matches!(
            read_committed_texel(
                &state,
                tile(ImageFormat::ColorIndex, PixelSize::Bits32, 0, 0),
                addressed,
                TextureLutMode::Rgba16,
            ),
            Err(PhysicalTexelReadError::Indexed(
                IndexedTexelResolveError::UnsupportedIndexSize { .. }
            ))
        ));
    }

    #[test]
    fn address_scope_is_rejected_before_empty_state_validity() {
        let state = PhysicalTmemState::try_new().unwrap();
        let addressed = AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Even);

        assert_eq!(
            read_committed_texel(
                &state,
                tile(ImageFormat::Rgba, PixelSize::Bits32, 256, 0),
                addressed,
                TextureLutMode::Disabled,
            ),
            Err(PhysicalTexelReadError::Rgba32BaseOutsideLowHalf {
                byte_address: 0x800,
            })
        );
        assert_eq!(
            read_committed_texel(
                &state,
                tile(ImageFormat::ColorIndex, PixelSize::Bits8, 256, 0),
                addressed,
                TextureLutMode::Rgba16,
            ),
            Err(PhysicalTexelReadError::EnabledCiSourceOutsideLowHalf {
                byte_address: 0x800,
            })
        );
    }

    #[test]
    fn supported_empty_state_read_reports_the_exact_first_physical_byte() {
        let state = PhysicalTmemState::try_new().unwrap();
        assert!(matches!(
            read_committed_texel(
                &state,
                tile(ImageFormat::Intensity, PixelSize::Bits8, 0, 0),
                AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Odd),
                TextureLutMode::Disabled,
            ),
            Err(PhysicalTexelReadError::InvalidTexelByte { address: 4 })
        ));
    }
}
