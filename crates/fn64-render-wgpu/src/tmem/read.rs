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
pub trait TmemByteSource: Sync {
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
                write!(
                    formatter,
                    "physical TMEM texel byte {address:#05x} is invalid"
                )
            }
            Self::Rgba32BaseOutsideLowHalf { byte_address } => write!(
                formatter,
                "RGBA32 tile base {byte_address:#05x} is outside low-half TMEM"
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
            Self::InvalidTexelByte { .. } | Self::Rgba32BaseOutsideLowHalf { .. } => None,
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
    validate_address_scope(tile, kind)?;
    let scope = AddressScope::of(tile, kind);
    let snapshot = state.snapshot();
    let texel = match kind {
        ReadKind::Direct => {
            let raw = read_raw_texel(state, tile, addressed, scope)?;
            decode_direct_texel(tile.format(), raw)?
        }
        ReadKind::Indexed { palette } => {
            let raw_index = read_raw_texel(state, tile, addressed, scope)?;
            match resolve_indexed_texel(tile.format(), raw_index, palette, lut_mode)? {
                ResolvedIndexedTexel::Direct(texel) => texel,
                ResolvedIndexedTexel::Tlut(lookup) => {
                    let entry = read_tlut_entry(state, lookup.byte_address())?;
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

/// The one address-scope preflight, now carrying only the refusal that
/// survives hardware comparison.
///
/// The enabled-TLUT low-half rule has NOT been dropped -- it moved from a
/// refusal to a mask, in [`AddressScope`] below, which is what RT64 does.
/// See that type's doc for the citation.
fn validate_address_scope(
    tile: TileDescriptor,
    kind: ReadKind,
) -> Result<(), PhysicalTexelReadError> {
    let base = tile.tmem().get() * 8;
    if matches!(kind, ReadKind::Direct) && tile.size() == PixelSize::Bits32 && base >= 0x0800 {
        return Err(PhysicalTexelReadError::Rgba32BaseOutsideLowHalf { byte_address: base });
    }
    Ok(())
}

/// How far a read may address before it wraps.
///
/// The RDP masks a texel's TMEM address rather than trapping it, and the
/// mask depends on whether the read scopes to one 2 KiB half. RT64's
/// `src/shaders/TextureDecoder.hlsli:162-163` states the whole rule in one
/// line:
///
/// ```text
/// // Determine the TMEM address mask. When using RGBA32 or TLUT, each
/// // sample only addresses half of TMEM.
/// const uint addressMask =
///     select_uint(or(isRgba32, usesTlut), RDP_TMEM_MASK16, RDP_TMEM_MASK8);
/// ```
///
/// with `RDP_TMEM_MASK16 = 0x7FF` and `RDP_TMEM_MASK8 = 0xFFF` (`:14-15`),
/// applied inside `implLoadTMEM` (`:17-25`) as
/// `TMEM.Load(((finalAddress & maskAddress) | orAddress) & RDP_TMEM_MASK8)`.
///
/// So an enabled-TLUT index source IS confined to the low half -- that
/// constraint is real, not invented here -- but hardware confines it by
/// wrapping. Wrapping keeps the property the old refusal existed to
/// protect (a TLUT-enabled index read can never reach the palette's own
/// half and paint palette bytes as image data) without refusing a frame
/// the RDP would have drawn. RGBA32 in this same file has always been
/// handled this way, by [`rgba32_low_address`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AddressScope {
    /// The full 4 KiB, mask `0x0fff`.
    FullTmem,
    /// One 2 KiB half, mask `0x07ff`.
    LowHalf,
}

impl AddressScope {
    const fn mask(self) -> u64 {
        match self {
            Self::FullTmem => TMEM_ADDRESS_MASK,
            Self::LowHalf => TMEM_LOW_HALF_MASK,
        }
    }

    /// RT64's `or(isRgba32, usesTlut)`, in this reader's own terms. The
    /// `ReadKind::Indexed` arm is reached only under an enabled TLUT
    /// (`preflight` classifies a TLUT-disabled CI tile through
    /// `resolve_indexed_texel`, which refuses it long before here), and
    /// `PixelSize::Bits32` is the RGBA32 half of the same disjunction.
    const fn of(tile: TileDescriptor, kind: ReadKind) -> Self {
        if matches!(kind, ReadKind::Indexed { .. }) {
            return Self::LowHalf;
        }
        match tile.size() {
            PixelSize::Bits32 => Self::LowHalf,
            _ => Self::FullTmem,
        }
    }
}

fn read_raw_texel<S: TmemByteSource + ?Sized>(
    state: &S,
    tile: TileDescriptor,
    addressed: AddressedTmemTexel,
    scope: AddressScope,
) -> Result<RawTexel, PhysicalTexelReadError> {
    match tile.size() {
        PixelSize::Bits4 => {
            let packed = RawTexel::try_new(
                PixelSize::Bits8,
                u32::from(read_valid_byte(
                    state,
                    first_physical_byte(tile, addressed, scope),
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
                first_physical_byte(tile, addressed, scope),
            )?),
        )
        .map_err(Into::into),
        PixelSize::Bits16 => {
            let bytes = read_linear_bytes::<2, S>(state, tile, addressed, scope)?;
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
    scope: AddressScope,
) -> Result<[u8; N], PhysicalTexelReadError> {
    let linear = linear_byte_address(tile, addressed);
    let exchange = odd_row_exchange(addressed);
    let mut bytes = [0; N];
    for (offset, byte) in bytes.iter_mut().enumerate() {
        let address = ((linear + offset as u64) & scope.mask()) as u16;
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

/// Reads one TLUT entry: the FIRST quadricated lane, and only that lane.
///
/// The RDP quadricates on `LoadTLUT` -- one 16-bit palette entry into all
/// four 16-bit lanes of the 64-bit word -- and this crate's loader does
/// too (`tmem/execute/load_tlut.rs`'s `map_physical_lanes`), matching
/// RT64's `src/hle/rt64_rdp.cpp:368-397` (`loadWord<_, TLUT = true>`,
/// which masks the RDRAM offset to `& 0x1` and stores all eight bytes).
///
/// That is a WRITE-side convention. It used to be enforced here as a
/// READ-side precondition -- all eight bytes valid
/// (`IncompleteTlutEntry`) and all four lanes equal
/// (`NonCanonicalTlutEntry`) -- and neither half survives comparison
/// with the hardware model this crate ports. RT64's palette read is one
/// line, `src/shaders/TextureDecoder.hlsli:179`:
///
/// ```text
/// const uint paletteValue =
///     loadTLUT(paletteAddress + 1) | (loadTLUT(paletteAddress) << 8);
/// ```
///
/// over `#define loadTLUT(a) TMEM.Load(uint2((a) & RDP_TMEM_MASK8, 0))`
/// (`:28`). Lanes 1..3 are never addressed, so RT64 can neither observe
/// nor refuse a word whose lanes disagree or whose tail is unwritten.
/// The in-tree reference lane reads the same two bytes and stops
/// (`fn64-render-reference` `src/gbi/state.rs:853-869`, `read_tlut`).
///
/// The refusal was also self-inconsistent: this crate's own
/// `load_tlut.rs` deliberately supports wrapping TLUT bases (base 511
/// across the bank), which writes exactly the unequal lanes this
/// function then refused to read.
///
/// What is KEPT is the rule that governs every other read in this file:
/// a byte the reader actually addresses must have been written. Lane 0's
/// two bytes are still required, and a missing one still raises
/// [`PhysicalTexelReadError::InvalidTexelByte`] naming the exact address,
/// rather than being invented as zero.
fn read_tlut_entry<S: TmemByteSource + ?Sized>(
    state: &S,
    byte_address: u16,
) -> Result<RawTexel, PhysicalTexelReadError> {
    let high = read_valid_byte(state, byte_address)?;
    let low = read_valid_byte(state, byte_address + 1)?;
    RawTexel::try_new(
        PixelSize::Bits16,
        u32::from(u16::from_be_bytes([high, low])),
    )
    .map_err(Into::into)
}

fn first_physical_byte(
    tile: TileDescriptor,
    addressed: AddressedTmemTexel,
    scope: AddressScope,
) -> u16 {
    if tile.size() == PixelSize::Bits32 {
        return rgba32_low_address(tile, addressed);
    }
    let address = (linear_byte_address(tile, addressed) & scope.mask()) as u16;
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

/// The odd-row XOR4 bank exchange: the TILE-RELATIVE row's parity, and
/// nothing else.
///
/// Pinned RT64 IMPLEMENTS this exchange, and derives its parity from a
/// TILE-RELATIVE coordinate with no T-origin term. That is implementation
/// evidence from an allowed MIT source, not a prose statement of the
/// hardware rule -- no allowed source found so far states it directly, so
/// the rule below is fn64's own reading of what these two agree on:
///
/// - **The exchange.** `implLoadTMEM` (`shaders/TextureDecoder.hlsli:17-25`)
///   computes `wordIndex = (relativeAddress - rowStart) / 4`, then on
///   `oddRow` addresses `swapWordIndex = wordIndex ^ 1` while preserving
///   `relativeAddress & 0x3`. Swapping the 32-bit word index within a row
///   and keeping the byte offset is exactly a 4-byte address XOR -- fn64
///   spells the same operation `addr ^ 4`.
/// - **The parity.** `sampleTMEM` (`:149-150`) derives it as
///   `oddRow = (texelInt.y & 1)`, from the tile-relative texel coordinate.
///   No tile origin (`tl`/`low_t`) participates.
///
///
/// **This used to XOR in a `first_row_parity` derived from the tile's
/// `low_t`.** That term is not on hardware. It was self-cancelling for
/// LoadTile, whose writer carried the identical term, but the LoadBlock
/// writer derived its parity from `source_t.raw()` instead -- a different
/// field in a different unit (`.raw()` versus `.integer()` = `raw >> 2`). The
/// two disagreed in 256 of 512 enumerated cases, and a disagreeing row
/// fetched every texel from the wrong 4-byte half of its 64-bit word. See
/// `docs/RT64-WM2000-TEXEL-LOCALISATION.md`.
fn odd_row_exchange(addressed: AddressedTmemTexel) -> bool {
    addressed.row() & 1 != 0
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

    /// The RGBA32 base refusal still preempts validity, and the
    /// enabled-TLUT CI tile at the same base no longer refuses at all --
    /// it wraps, and then reports the WRAPPED address as the unwritten
    /// byte. The two arms are deliberately kept side by side: they used
    /// to give the same answer, and now they must not.
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
        // 0x800 & 0x7ff == 0x000: the wrap target, hand-computed.
        assert_eq!(
            read_committed_texel(
                &state,
                tile(ImageFormat::ColorIndex, PixelSize::Bits8, 256, 0),
                addressed,
                TextureLutMode::Rgba16,
            ),
            Err(PhysicalTexelReadError::InvalidTexelByte { address: 0x000 })
        );
    }

    /// An empty state names the exact byte the read would have needed.
    ///
    /// Row 0 does not exchange, so a tile based at TMEM word 0 addresses byte
    /// 0 -- and it does so for BOTH first-row parity values, because the
    /// caller-supplied parity no longer participates in the exchange (see
    /// `odd_row_exchange`). Asserting both arms is what makes a reintroduced
    /// origin term fail here: it would send the `Odd` arm to byte 4.
    ///
    /// This previously asserted `address: 4` for the `Odd` arm alone, which
    /// was the removed term.
    #[test]
    fn supported_empty_state_read_reports_the_exact_first_physical_byte() {
        let state = PhysicalTmemState::try_new().unwrap();
        for parity in [TmemFirstRowParity::Even, TmemFirstRowParity::Odd] {
            assert!(
                matches!(
                    read_committed_texel(
                        &state,
                        tile(ImageFormat::Intensity, PixelSize::Bits8, 0, 0),
                        AddressedTmemTexel::new(0, 0, parity),
                        TextureLutMode::Disabled,
                    ),
                    Err(PhysicalTexelReadError::InvalidTexelByte { address: 0 })
                ),
                "row 0 never exchanges, whatever first-row parity the caller \
                 supplies: {parity:?}"
            );
        }
    }

    // --- WM2000's failing texrect: reader/writer odd-row parity ---------
    //
    // Measured on the real ROM through the all-Rust stack
    // (`fn64-cpu-runtime` + `fn64-render-wgpu`), the texrect that aborted
    // execution carries exactly these wire fields:
    //
    //     fmt=IntensityAlpha siz=Bits4 line_words=5 tmem_word=0 palette=0
    //     mask_s=0 shift_s=0 mask_t=0 shift_t=0  (both axes clamp)
    //     SetTileSize raw 10.2: sl=252 tl=188 sh=512 th=384
    //     draw 64x48 at (144,48); S10.5 s_at(0)=2048 s_at(63)=4064
    //                             t_at(0)=1536 t_at(47)=3040
    //     lut=Rgba16
    //
    // `low_t.integer() == 188 >> 2 == 47`, which is **odd**, and that is why
    // this tile is the one every origin-term defect in this crate has
    // surfaced through.
    //
    // Historically the WRITER (`tmem/types.rs`'s
    // `project_tmem_transfer_word`, `Tile` arm) derived its XOR4 exchange as
    // `(bounds.low_t().integer() + row) & 1`, so an odd `low_t` exchanged the
    // EVEN tile rows, while the READER's `odd_row_exchange` derived it as
    // `first_is_odd ^ (row & 1)` -- invertible by the caller. Every texel
    // whose row landed in the 6-byte per-row tail gap then read an address
    // the load never wrote.
    //
    // Both sides now use the tile-relative row alone, which is what
    // pinned RT64 does (see `odd_row_exchange`), so the origin cannot perturb
    // the pairing at all. These tests are retained as the regression guard
    // for that, on real measured content.

    /// WM2000's own wire fields, named once so the two tests below cannot
    /// drift apart from each other or from the measured packet.
    const WM2000_LINE_WORDS: u16 = 5;
    const WM2000_LOW_S_RAW: u16 = 252;
    const WM2000_LOW_T_RAW: u16 = 188;
    const WM2000_HIGH_S_RAW: u16 = 512;
    const WM2000_HIGH_T_RAW: u16 = 384;
    /// `ceil(row_bytes / 8)` for this load: 5 destination words per row, of
    /// which the last carries only 2 defined source bytes (34 bytes/row).
    const WM2000_WORDS_PER_ROW: u16 = 5;
    const WM2000_DEFINED_TAIL_BYTES: u16 = 2;
    const WM2000_ROWS: u16 = 50;

    /// Rebuilds the exact TMEM byte set WM2000's `cmd 39` `LoadTile`
    /// validates, from the WRITER's own two rules rather than from a
    /// capture:
    ///
    /// - `project_tmem_transfer_word`'s `Tile` arm places transfer word
    ///   `w` at destination word `tmem + (w / words_per_row) * line_words
    ///   + (w % words_per_row)`, so rows advance by `line_words` while only
    ///   `words_per_row` words per row are written -- a gap when
    ///   `words_per_row < line_words`, and a 6-byte tail gap here because
    ///   the row's last word is only partly defined.
    /// - `tmem/execute/load_tile.rs`'s `map_physical_lanes` writes lane
    ///   `source_lane ^ (4 * odd_row_exchange)` for the `Linear64` layout.
    ///
    /// Returns a source whose valid set is precisely that, so a read of any
    /// byte the load did not write fails exactly as production did.
    fn wm2000_load_tile_source() -> SparseSource {
        let mut source = SparseSource::new();
        let low_t_integer = WM2000_LOW_T_RAW >> 2;
        for word in 0..WM2000_WORDS_PER_ROW * WM2000_ROWS {
            let row = word / WM2000_WORDS_PER_ROW;
            let within = word % WM2000_WORDS_PER_ROW;
            let destination_word = row * WM2000_LINE_WORDS + within;
            // The writer's own parity, not the reader's -- and the writer's
            // rule is the TILE-RELATIVE row alone, with no T-origin term.
            // See `odd_row_exchange` above for the RT64 citation. This
            // line used to read `(low_t_integer + row) & 1`, mirroring a
            // writer term that has since been removed from
            // `tmem/types.rs` as not being on hardware.
            let exchange = if row & 1 == 1 { 4 } else { 0 };
            let defined = if within + 1 < WM2000_WORDS_PER_ROW {
                8
            } else {
                WM2000_DEFINED_TAIL_BYTES
            };
            for lane in 0..defined {
                // A nonzero payload: the CI4 index must resolve to a TLUT
                // entry this fixture actually writes.
                source.write(destination_word * 8 + (lane ^ exchange), 0x00);
            }
        }
        // The palette the enabled TLUT resolves through. Index 0 is the only
        // index this fixture's all-zero payload produces.
        source.write_quadricated_entry(0, 0x0001);
        source
    }

    fn wm2000_tile() -> TileDescriptor {
        TileDescriptor::from_wire(
            ImageFormat::IntensityAlpha,
            PixelSize::Bits4,
            WM2000_LINE_WORDS,
            TmemWordAddress::try_new(0).unwrap(),
            0,
            TileAddressMode::from_wire(0b10),
            0,
            0,
            TileAddressMode::from_wire(0b10),
            0,
            0,
        )
    }

    /// The reader's addressing for WM2000's texrect pixel `(column, row)`,
    /// hand-derived from the wire fields above rather than routed through
    /// the texrect executor (which this crate's unit tests cannot drive
    /// without a GPU): the rectangle steps exactly one texel per pixel on
    /// both axes, so `s = 2048 + 32 * column` and `t = 1536 + 32 * row` in
    /// S10.5, and `mask == 0` forces the clamp arm of `address_axis_texel`.
    fn wm2000_addressed(column: u32, row: u32, parity: TmemFirstRowParity) -> AddressedTmemTexel {
        let dimension_s = i64::from((WM2000_HIGH_S_RAW >> 2) - (WM2000_LOW_S_RAW >> 2) + 1);
        let dimension_t = i64::from((WM2000_HIGH_T_RAW >> 2) - (WM2000_LOW_T_RAW >> 2) + 1);
        let s = 2048_i64 + 32 * i64::from(column) - i64::from(WM2000_LOW_S_RAW) * 8;
        let t = 1536_i64 + 32 * i64::from(row) - i64::from(WM2000_LOW_T_RAW) * 8;
        let texel_column = (s >> 5).clamp(0, dimension_s - 1) as u16;
        let texel_row = (t >> 5).clamp(0, dimension_t - 1) as u16;
        AddressedTmemTexel::new(texel_column, texel_row, parity)
    }

    /// **Positive control.** Before asserting anything about parity, prove
    /// this fixture really reaches WM2000's failing pixel and really
    /// reproduces its byte -- otherwise the test below could pass against a
    /// rectangle that never leaves column 0.
    ///
    /// Pixel `(63, 0)` addresses texel column 64, row 1. Row 1's own
    /// un-exchanged address is `0 * 8 + 1 * 5 * 8 + 64 / 2 == 0x048`, and the
    /// row's exchange sends it to `0x04c`, which
    /// the load did NOT. `0x04c` is the exact address the production abort
    /// named.
    #[test]
    fn wm2000_texrect_pixel_sixty_three_reads_the_byte_its_own_load_wrote() {
        let addressed = wm2000_addressed(63, 0, TmemFirstRowParity::Even);
        assert_eq!(
            (addressed.column(), addressed.row()),
            (64, 1),
            "the fixture must reach texel column 64 of tile row 1, not column 0"
        );
        let source = wm2000_load_tile_source();
        // Tile-relative row 1 takes the XOR4 exchange, on BOTH sides. The
        // writer put this row's bytes at the exchanged addresses, so the
        // exchanged address is the one inside the load and the un-exchanged
        // one is outside it. Both halves are asserted so the claim is
        // falsifiable either way.
        //
        // These two used to be the other way round, and the read below used
        // to be an `InvalidTexelByte { address: 0x04c }` -- the production
        // abort this fixture was written to reproduce. That abort was the
        // origin-term defect: the writer exchanged row 1 by
        // `(low_t.integer() + row) & 1` while the reader un-exchanged it by
        // `first_is_odd ^ (row & 1)`, so the reader addressed a byte the
        // load had never written. With both sides on the tile-relative row
        // (`odd_row_exchange` above) the read lands on the byte the load
        // wrote and SUCCEEDS.
        assert!(
            source.valid_byte(0x04c).is_some(),
            "row 1 is exchanged, so its exchanged byte is the one the load wrote"
        );
        assert!(
            source.valid_byte(0x048).is_none(),
            "row 1's un-exchanged partner must be a byte the load never wrote"
        );
        assert!(
            read_texel(&source, wm2000_tile(), addressed, TextureLutMode::Rgba16).is_ok(),
            "the pixel that used to abort production must now read a byte its \
             own load wrote"
        );
    }

    /// **Every one of the 3,072 pixels WM2000's texrect draws samples a byte
    /// its own `LoadTile` wrote.** Real measured content: this tile's
    /// `low_t.integer()` is 47, an ODD T origin, which is the case every
    /// origin-term defect in this crate has surfaced through.
    ///
    /// The reader derives the exchange from the tile-relative row alone
    /// (`odd_row_exchange` above), so the caller-supplied
    /// `TmemFirstRowParity` no longer participates. The sweep therefore runs
    /// both parity values and requires BOTH to read clean -- which is the
    /// real property now: the origin must not perturb the read at all. A
    /// reader that reintroduced an origin term would fail one of the two
    /// arms, because they differ by exactly that term.
    ///
    /// Swept over the whole rectangle rather than one pixel so a fix that
    /// merely moves the failure along the strip cannot pass.
    #[test]
    fn wm2000_texrect_reads_only_loaded_bytes_under_the_writers_own_row_parity() {
        let source = wm2000_load_tile_source();
        let tile = wm2000_tile();
        assert_eq!(
            (WM2000_LOW_T_RAW >> 2) & 1,
            1,
            "this test is only meaningful while WM2000's low_t really is odd"
        );

        // **The footprint is load-bearing, so pin it.** The sweep below
        // only addresses tile rows 1..=48, so a fixture whose row count
        // drifted by one would still sweep clean and prove nothing about
        // the load's real extent. These two assertions make the declared
        // footprint falsifiable on its own terms: the load writes exactly
        // `words_per_row * rows` transfer words, of which every row's last
        // is only partly defined, so the valid-byte total is exact.
        let expected_valid_bytes = u32::from(WM2000_ROWS)
            * (u32::from(WM2000_WORDS_PER_ROW - 1) * 8 + u32::from(WM2000_DEFINED_TAIL_BYTES));
        let loaded_bytes = (0..0x0800_u16)
            .filter(|address| source.valid_byte(*address).is_some())
            .count() as u32;
        assert_eq!(
            loaded_bytes, expected_valid_bytes,
            "the fixture's own valid set must equal the footprint its constants declare"
        );
        assert_eq!(
            loaded_bytes, 1_700,
            "WM2000's cmd-39 LoadTile validates exactly 1,700 low-TMEM bytes: 50 rows of 34"
        );
        // The load's last written row must be the last row the tile can
        // address, so the sweep cannot be silently reading a shortened
        // fixture that happens to cover the rows it touches.
        assert_eq!(
            u32::from(WM2000_ROWS),
            u32::from((WM2000_HIGH_T_RAW >> 2) - (WM2000_LOW_T_RAW >> 2) + 1),
            "the load's row count must equal the tile's own addressable T extent"
        );

        let mut even_failures = 0_u32;
        let mut odd_failures = 0_u32;
        for row in 0..48 {
            for column in 0..64 {
                for (parity, failures) in [
                    (TmemFirstRowParity::Even, &mut even_failures),
                    (TmemFirstRowParity::Odd, &mut odd_failures),
                ] {
                    let addressed = wm2000_addressed(column, row, parity);
                    if read_texel(&source, tile, addressed, TextureLutMode::Rgba16).is_err() {
                        *failures += 1;
                    }
                }
            }
        }

        assert_eq!(
            odd_failures, 0,
            "every sampled byte must be one the load wrote"
        );
        // **Both arms must be clean, and that is the assertion.** The two
        // differ only in the caller-supplied first-row parity, which the
        // reader no longer consults; an origin term reintroduced on the read
        // side would make exactly one of them fail. Before that term was
        // removed this read `even_failures == 48` -- one pixel per rectangle
        // row -- which was the defect, not a property worth preserving.
        assert_eq!(
            even_failures, 0,
            "the tile's T origin must not perturb the read: both parity values \
             address the same bytes, so neither may leave the load's footprint"
        );
    }

    // ---------------------------------------------------------------
    // WM2000 walls 3/4/5: what the RDP does when a TLUT read is not the
    // textbook shape.
    //
    // AUTHORITY, checked against the pinned RT64 port source
    // `5473732a822a4423b5696e7cb18fecc425a59875`:
    //
    // * WALL 3 (`EnabledCiSourceOutsideLowHalf`). RT64
    //   `src/shaders/TextureDecoder.hlsli:162-163` DOES restrict a
    //   TLUT-enabled index source to the low half --
    //   `addressMask = select_uint(or(isRgba32, usesTlut),
    //   RDP_TMEM_MASK16, RDP_TMEM_MASK8)` with `RDP_TMEM_MASK16 = 0x7FF`
    //   (`:15`). So the constraint is REAL and is NOT invented here. But
    //   RT64 applies it as a MASK inside `implLoadTMEM` (`:17-25`:
    //   `TMEM.Load(((finalAddress & maskAddress) | orAddress) & ...)`),
    //   never as a refusal. Masking preserves the whole point of the
    //   rule -- a TLUT-enabled index read can never reach the palette's
    //   own half -- while matching hardware's wrap. This mirrors what
    //   `rgba32_low_address` in this very file already does for the
    //   other genuinely half-scoped format.
    //
    // * WALLS 4 and 5 (`NonCanonicalTlutEntry` / `IncompleteTlutEntry`).
    //   RT64 reads exactly ONE lane: `TextureDecoder.hlsli:179` is
    //   `loadTLUT(paletteAddress + 1) | (loadTLUT(paletteAddress) << 8)`
    //   over `#define loadTLUT(a) TMEM.Load(uint2((a) & RDP_TMEM_MASK8,
    //   0))` (`:28`). Lanes 1..3 are never addressed, so RT64 can
    //   neither observe nor refuse a non-quadricated word. The in-tree
    //   reference lane agrees: `fn64-render-reference`
    //   `src/gbi/state.rs:853-869` (`read_tlut`) reads two bytes at
    //   `TMEM_HALF_BYTES + index * 8` and `+ 1` and stops.
    //
    //   The quadrication CONVENTION is still real on the write side --
    //   RT64 `src/hle/rt64_rdp.cpp:368-397` (`loadWord<_, TLUT=true>`)
    //   masks the RDRAM offset to `& 0x1` and stores all eight TMEM
    //   bytes -- and this crate's `tmem/execute/load_tlut.rs` does the
    //   same. What was wrong was promoting a WRITE convention into a
    //   READ precondition, which this crate's own loader can then
    //   violate: `load_tlut.rs`'s wrapping-base support (base 511
    //   wrapping across the bank) writes exactly the unequal lanes the
    //   reader refused.
    //
    //   The validity requirement is NOT dropped, only narrowed to the
    //   two bytes actually read. Inventing an unloaded palette byte
    //   stays refused, by the same `InvalidTexelByte` rule every other
    //   read in this file obeys -- see
    //   `an_unloaded_lane_zero_is_still_refused_by_name` below.

    /// WALL 3, before/after pin. A CI8 tile under an enabled TLUT whose
    /// index source walks to exactly `0x800` must WRAP into the low half
    /// (RT64 `TextureDecoder.hlsli:162-163`), not refuse.
    ///
    /// The expected value is hand-derived, never read back from the
    /// constant under test: `0x800 & 0x7ff == 0x000`, so the read must
    /// land on byte `0x000`. That byte is written here as index `0x42`,
    /// and entry `0x42` is written as `0xf801`, which the RGBA16 5/5/5/1
    /// layout sends to `r=0b11111 g=0 b=0 a=1` -> `[0xff, 0, 0, 0xff]`.
    /// A distinct index `0x11` is placed at `0x800` itself, so a reader
    /// that failed to wrap and read the palette half as image data would
    /// produce a *different, detectably wrong* color rather than
    /// silently agreeing.
    #[test]
    fn an_enabled_tlut_ci_source_at_the_low_half_boundary_wraps_like_rt64() {
        let subject = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 256, 0);
        let addressed = AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Even);

        // tmem word 256 * 8 == 0x800 exactly: the first byte past the
        // low half, which is the address the WM2000 run aborted on.
        assert_eq!(u64::from(subject.tmem().get()) * 8, 0x800);

        let mut source = SparseSource::new();
        // Where the wrap must land.
        source.write(0x000, 0x42);
        source.write_quadricated_entry(0x42, 0xf801);
        // A decoy at the unwrapped address, with its own distinct entry.
        source.write(0x800, 0x11);
        source.write_quadricated_entry(0x11, 0x07c1);

        let decoded = read_texel(&source, subject, addressed, TextureLutMode::Rgba16)
            .expect("a TLUT-enabled CI source at 0x800 wraps into the low half");
        assert_eq!(
            decoded.texel().rgba8888(),
            [0xff, 0x00, 0x00, 0xff],
            "the wrapped read must resolve index 0x42 at byte 0x000, not \
             the decoy index 0x11 sitting at 0x800"
        );
    }

    /// WALL 3's kept half. Masking must not turn into "no rule at all":
    /// an RGBA32 tile based in the high half is a genuinely different
    /// refusal (`Rgba32BaseOutsideLowHalf`) that RT64 also does not
    /// share, and it stays exactly as it was. This is the mutation
    /// guard on the arm that is KEPT.
    #[test]
    fn the_rgba32_high_half_base_refusal_is_untouched_by_the_ci_wrap() {
        let state = PhysicalTmemState::try_new().unwrap();
        assert_eq!(
            read_committed_texel(
                &state,
                tile(ImageFormat::Rgba, PixelSize::Bits32, 256, 0),
                AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Even),
                TextureLutMode::Disabled,
            ),
            Err(PhysicalTexelReadError::Rgba32BaseOutsideLowHalf {
                byte_address: 0x800,
            })
        );
    }

    /// WALL 4, before/after pin. A TLUT word whose lanes disagree must
    /// resolve from LANE 0, exactly as RT64
    /// `TextureDecoder.hlsli:179` and the reference's
    /// `read_tlut` do.
    ///
    /// The lane pattern is WM2000's own measured one in shape --
    /// three agreeing lanes and one foreign -- but the values are
    /// chosen here so the expected color is hand-derivable and so that
    /// reading ANY lane other than 0 gives a different answer.
    /// Lane 0 is `0xf801` -> opaque red. Lane 3 is `0x07c1`, which is
    /// `r=0 g=0b11111 b=0 a=1` -> opaque green, so a reader that took
    /// the last lane would be caught.
    #[test]
    fn an_unequal_tlut_word_resolves_from_lane_zero_like_rt64() {
        let subject = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 0, 0);
        let addressed = AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Even);

        let mut source = SparseSource::new();
        source.write(0x000, 0x00);
        // Entry 0 lives at 0x800. Hand-write lanes 0..2 as 0xf801 and
        // lane 3 as 0x07c1, the three-quarters-consistent shape the
        // WM2000 run reported (`[0100, 0100, 0100, 8f94]`).
        for lane in 0..3_u16 {
            source.write(0x800 + lane * 2, 0xf8);
            source.write(0x800 + lane * 2 + 1, 0x01);
        }
        source.write(0x806, 0x07);
        source.write(0x807, 0xc1);

        let decoded = read_texel(&source, subject, addressed, TextureLutMode::Rgba16)
            .expect("an unequal TLUT word resolves from lane 0");
        assert_eq!(
            decoded.texel().rgba8888(),
            [0xff, 0x00, 0x00, 0xff],
            "lane 0 (0xf801, opaque red) is the entry; lane 3's 0x07c1 \
             (opaque green) must never be read"
        );
    }

    /// WALL 5, before/after pin. A TLUT word with only its low four
    /// bytes valid (`mask 0x0f`, the exact mask the WM2000 run
    /// reported) has a VALID lane 0, so the read succeeds. RT64 and
    /// the reference never address lanes 1..3, so their validity
    /// cannot gate the read.
    ///
    /// Expected color hand-derived from `0xf801` as above.
    #[test]
    fn a_tlut_word_valid_only_in_its_low_four_bytes_still_reads() {
        let subject = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 0, 0);
        let addressed = AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Even);

        let mut source = SparseSource::new();
        source.write(0x000, 0x00);
        // mask 0x0f: bytes 0..3 valid, 4..7 never written.
        source.write(0x800, 0xf8);
        source.write(0x801, 0x01);
        source.write(0x802, 0xf8);
        source.write(0x803, 0x01);

        let decoded = read_texel(&source, subject, addressed, TextureLutMode::Rgba16)
            .expect("lanes 1..3's validity is not addressed by the RDP");
        assert_eq!(decoded.texel().rgba8888(), [0xff, 0x00, 0x00, 0xff]);
    }

    /// WALLS 4/5's kept half, and the mutation guard on the arm that
    /// SURVIVES. Narrowing validity to lane 0 must not become "invent
    /// missing palette bytes". A word whose lane 0 is itself unwritten
    /// is still refused, and by the SAME `InvalidTexelByte` name every
    /// other unloaded read in this file uses -- so the refusal cannot
    /// have been quietly downgraded to a zero.
    #[test]
    fn an_unloaded_lane_zero_is_still_refused_by_name() {
        let subject = tile(ImageFormat::ColorIndex, PixelSize::Bits8, 0, 0);
        let addressed = AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Even);

        // Nothing at all in the palette.
        let mut nothing = SparseSource::new();
        nothing.write(0x000, 0x00);
        assert_eq!(
            read_texel(&nothing, subject, addressed, TextureLutMode::Rgba16),
            Err(PhysicalTexelReadError::InvalidTexelByte { address: 0x800 })
        );

        // Lanes 1..3 fully written, lane 0 missing: the inverse of the
        // wall-5 fixture. If validity had merely been widened to
        // "any lane will do", this would wrongly succeed.
        let mut tail_only = SparseSource::new();
        tail_only.write(0x000, 0x00);
        for lane in 1..4_u16 {
            tail_only.write(0x800 + lane * 2, 0xf8);
            tail_only.write(0x800 + lane * 2 + 1, 0x01);
        }
        assert_eq!(
            read_texel(&tail_only, subject, addressed, TextureLutMode::Rgba16),
            Err(PhysicalTexelReadError::InvalidTexelByte { address: 0x800 })
        );

        // Lane 0's HIGH byte written but its low byte missing: the
        // half-lane case, which must refuse at 0x801 specifically.
        let mut half_lane = SparseSource::new();
        half_lane.write(0x000, 0x00);
        half_lane.write(0x800, 0xf8);
        assert_eq!(
            read_texel(&half_lane, subject, addressed, TextureLutMode::Rgba16),
            Err(PhysicalTexelReadError::InvalidTexelByte { address: 0x801 })
        );
    }
}
