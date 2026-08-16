//! Physical TMEM load executors over exact captured guest reads.

mod load_block;
mod load_tile;
mod load_tlut;
mod packet;

pub(crate) use load_block::map_physical_lanes as map_physical_lanes_block;
pub use load_block::{
    prepare_load_block, ExecutedLoadBlock, LoadBlockExecutionError, PreparedLoadBlock,
};
pub use load_tile::{
    prepare_load_tile, ExecutedLoadTile, LoadTileExecutionError, PreparedLoadTile,
};
// LoadTile shares LoadBlock's exact byte-to-physical-lane mapping (both are
// linear/split-bank fragment placement with no quadrication); T3 Phase B's
// neutral executor reuses the one already-tested implementation rather than
// re-deriving an identical private copy under `load_tile`'s own name.
pub(crate) use load_tlut::map_physical_lanes as map_physical_lanes_tlut;
pub use load_tlut::{
    prepare_load_tlut, ExecutedLoadTlut, LoadTlutExecutionError, PreparedLoadTlut,
};
pub use packet::{execute_ordered_tmem_loads, TmemPacketExecutionError};
