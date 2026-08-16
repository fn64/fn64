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
//! geometry (M4.3.1) and physical-lane mask (M4.3.1b) are frozen; TLUT's own
//! destination execution is still deferred. Each transfer word's defined
//! destination-byte mask is a distinct, checked fact from its defined
//! source-byte mask -- equal by construction for Block/Tile, but not for
//! TLUT, whose 2 captured source bytes quadricate into 8 defined destination
//! bytes. M4.2a consumes those exact plans into one packet-local
//! physical-state transaction, retaining intermediate load effects across
//! overlaps and publishing durable state only after exact GPU and guest
//! lifecycle evidence. RT64 is not hardware authority for this module.

mod execute;
mod physical;
mod state;
mod types;
mod wire;

pub use execute::{
    execute_ordered_tmem_loads, prepare_load_block, prepare_load_tile, ExecutedLoadBlock,
    ExecutedLoadTile, LoadBlockExecutionError, LoadTileExecutionError, PreparedLoadBlock,
    PreparedLoadTile, TmemPacketExecutionError,
};
pub use physical::{
    CommittedTmemTransaction, DefinedPhysicalTmemWordBytes, GpuBoundTmemTransaction,
    PendingTmemTransaction, PhysicalTmemBinding, PhysicalTmemError, PhysicalTmemPacketTransaction,
    PhysicalTmemPublicationAuthority, PhysicalTmemState, PhysicalTmemStateIdentity,
    PhysicalTmemTransactionIdentity, StagedTmemTransaction,
};
pub use state::{TileState, TmemState};
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
