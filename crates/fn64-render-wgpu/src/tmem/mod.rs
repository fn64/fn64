//! Typed texture-memory register state and raw RDP wire decoding.
//!
//! Field widths, opcodes, and command ordering come from the public SGI
//! *Nintendo 64 RDP Command Summary*, Tables 1, 3, and 6–10. Strict TLUT
//! command admission additionally follows the public libultra `gbi.h`
//! `gDPLoadTLUTCmd` macro shape. The owned-source and transactional boundaries
//! are fn64's [`docs/DESIGN.md`](../../../../docs/DESIGN.md) M4.0 mechanism and
//! [`docs/RENDER-WGPU-PORT-PLAN.md`](../../../../docs/RENDER-WGPU-PORT-PLAN.md)
//! M4.1 scope. M4.2.0's transfer-word/effect-union split follows public
//! Programming Manual section 13.9; undefined word padding is never promoted
//! to defined texture content. YUV retains a source-only deferred contract
//! until its destination rules are frozen. TLUT's destination transfer-plan
//! geometry (M4.3.1) and physical-lane mask (M4.3.1b) are frozen. Each
//! transfer word's defined destination-byte mask is a distinct, checked fact
//! from its defined source-byte mask -- equal by construction for Block/Tile,
//! but not for TLUT, whose 2 captured source bytes quadricate into 8 defined
//! destination bytes. M4.3.2's LoadTLUT executor (`execute::load_tlut`) maps
//! that quadrication into M4.2a's physical TMEM lanes the same way M4.2b/c's
//! LoadTile/LoadBlock executors already do, and `execute::packet`'s
//! packet-level outer loop now dispatches `LoadTlut` alongside
//! `LoadTile`/`LoadBlock` in decode order; a YUV-deferred Tile/Block contract
//! remains the only refused load kind. M4.2a consumes those exact plans into
//! one packet-local physical-state transaction, retaining intermediate load
//! effects across overlaps and publishing durable state only after exact GPU
//! and guest lifecycle evidence. M4.3.3a's [`texel`] module adds a
//! format-neutral raw texel carrier reusable by later CI/TLUT and YUV
//! layers, plus a pure, allocation-free decoder for the seven direct-format
//! texel pairs (RGBA16, RGBA32, IA4, IA8, IA16, I4, I8); it makes no TMEM,
//! CI/TLUT, YUV, or GPU claim. M4.3.3b adds pure CI4/CI8 index normalization,
//! typed disabled/RGBA16/IA16 TLUT resolution, canonical lookup values, and
//! caller-supplied 16-bit entry decode; it deliberately performs no physical
//! TMEM read or validity/generation check. M4.3.3c's [`read`] module binds
//! those pure decoders to durable physical TMEM, explicit caller-owned
//! first-row parity, complete validity footprints, one state/generation
//! snapshot, and the conservative canonical all-eight-valid/equal TLUT
//! subset. Partial/unequal sample-lane behavior remains deferred to hardware
//! measurement. It performs no coordinate normalization, sampling, filtering,
//! cache, GPU, or production work. M4.3.3d's [`sample`] module adds the
//! integer-only point path from already-quantized signed S10.5 coordinates to
//! that reader. It preserves explicit caller-owned first-row parity and does
//! not select filter lanes, relax unequal TLUT banks, or enter a raster/GPU
//! path. RT64 is not hardware authority for this module.

mod execute;
mod physical;
mod read;
mod sample;
mod state;
mod texel;
mod types;
mod wire;

pub use execute::{
    execute_ordered_tmem_loads, prepare_load_block, prepare_load_tile, prepare_load_tlut,
    ExecutedLoadBlock, ExecutedLoadTile, ExecutedLoadTlut, LoadBlockExecutionError,
    LoadTileExecutionError, LoadTlutExecutionError, PreparedLoadBlock, PreparedLoadTile,
    PreparedLoadTlut, TmemPacketExecutionError,
};
pub use physical::{
    CommittedTmemTransaction, DefinedPhysicalTmemWordBytes, GpuBoundTmemTransaction,
    PendingTmemTransaction, PhysicalTmemBinding, PhysicalTmemError, PhysicalTmemPacketTransaction,
    PhysicalTmemPublicationAuthority, PhysicalTmemState, PhysicalTmemStateIdentity,
    PhysicalTmemTransactionIdentity, StagedTmemTransaction,
};
pub use read::{
    read_committed_texel, AddressedTmemTexel, DecodedPhysicalTexel, PhysicalTexelReadError,
    PhysicalTmemSnapshotIdentity, TmemFirstRowParity,
};
pub use sample::{
    address_point_texel, sample_committed_point, PointAddressError, PointSampleCoordinates,
    PointSampleError, PointSampleRequest, TextureAxis, TextureCoordinateS10_5,
};
pub use state::{TileState, TmemState};
pub use texel::{
    decode_direct_texel, decode_tlut_entry, resolve_indexed_texel, unpack_ci4_texel, Ci4Palette,
    Ci4PaletteError, Ci4UnpackError, DecodedTexel, DirectTexelDecodeError,
    IndexedTexelResolveError, RawTexel, RawTexelError, ResolvedIndexedTexel, TexelColumnParity,
    TlutEntryDecodeError, TlutLookup,
};
pub use types::{
    TextureImage, TileAddressMode, TileCoordinate, TileDescriptor, TileIndex, TileSize,
    TlutEntryCount, TmemDxt, TmemLoad, TmemLoadContract, TmemLoadDestinationPlan, TmemLoadEpoch,
    TmemLoadKind, TmemLoadSourceIdentity, TmemLoadSourcePlan, TmemTransferLayout,
    TmemTransferPhysicalWord, TmemTransferPlan, TmemTransferWord, TmemWordAddress,
};

pub(crate) use wire::{
    decode_tmem_command, TmemCommand, TmemSourcePlanStart, LOAD_BLOCK, LOAD_SYNC, LOAD_TILE,
    LOAD_TLUT, SET_TEXTURE_IMAGE, SET_TILE, SET_TILE_SIZE,
};
