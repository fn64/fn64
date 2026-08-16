//! Physical TMEM load executors over exact captured guest reads.

mod load_block;
mod load_tile;

pub use load_block::{
    prepare_load_block, ExecutedLoadBlock, LoadBlockExecutionError, PreparedLoadBlock,
};
pub use load_tile::{
    prepare_load_tile, ExecutedLoadTile, LoadTileExecutionError, PreparedLoadTile,
};
