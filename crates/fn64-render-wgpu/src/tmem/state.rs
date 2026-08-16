//! Transaction-local TMEM register ordering.
//!
//! Register roles and load ordering follow the public SGI *Nintendo 64 RDP
//! Command Summary*, Tables 3 and 6–10. Publication follows fn64's M4.0 owned
//! guest-read boundary in `docs/DESIGN.md` and the M4.1 transactional scope in
//! `docs/RENDER-WGPU-PORT-PLAN.md`; no RT64 behavior is used as authority.

use core::num::NonZeroU64;

use fn64_render_ir::PhysicalMemoryLayout;

use super::{TextureImage, TileDescriptor, TileIndex, TileSize, TmemLoad, TmemLoadEpoch};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileState {
    descriptor: Option<TileDescriptor>,
    size: Option<TileSize>,
    last_load_epoch: Option<TmemLoadEpoch>,
}

impl TileState {
    pub const fn descriptor(self) -> Option<TileDescriptor> {
        self.descriptor
    }

    pub const fn size(self) -> Option<TileSize> {
        self.size
    }

    pub const fn last_load_epoch(self) -> Option<TmemLoadEpoch> {
        self.last_load_epoch
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TmemState {
    texture_image: Option<TextureImage>,
    tiles: [TileState; 8],
    next_load_epoch: u64,
    armed_load_sync: Option<TmemLoadEpoch>,
    last_load: Option<TmemLoad>,
}

impl TmemState {
    pub const fn texture_image(&self) -> Option<TextureImage> {
        self.texture_image
    }

    pub fn tile(&self, index: TileIndex) -> TileState {
        self.tiles[index.array_index()]
    }

    pub const fn armed_load_sync(&self) -> Option<TmemLoadEpoch> {
        self.armed_load_sync
    }

    pub const fn last_load(&self) -> Option<TmemLoad> {
        self.last_load
    }

    pub(crate) fn set_texture_image(&mut self, image: TextureImage) {
        self.texture_image = Some(image);
    }

    pub(crate) fn set_tile(&mut self, index: TileIndex, descriptor: TileDescriptor) {
        self.tiles[index.array_index()].descriptor = Some(descriptor);
    }

    pub(crate) fn set_tile_size(&mut self, index: TileIndex, size: TileSize) {
        self.tiles[index.array_index()].size = Some(size);
    }

    pub(crate) fn load_sync(&mut self) -> Result<TmemLoadEpoch, &'static str> {
        let next = self
            .next_load_epoch
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or("LoadSync epoch overflowed")?;
        let epoch = TmemLoadEpoch::new(next);
        self.next_load_epoch = next.get();
        self.armed_load_sync = Some(epoch);
        Ok(epoch)
    }

    pub(crate) fn load_inputs(
        &self,
        tile: TileIndex,
        memory_layout: PhysicalMemoryLayout,
    ) -> Result<(TmemLoadEpoch, TextureImage, TileDescriptor), &'static str> {
        let epoch = self
            .armed_load_sync
            .ok_or("TMEM load requires a preceding unconsumed LoadSync")?;
        let image = self
            .texture_image
            .ok_or("TMEM load requires staged SetTextureImage state")?;
        if image.address().layout() != memory_layout {
            return Err("TMEM load texture-image layout differs from the current packet layout");
        }
        let descriptor = self.tiles[tile.array_index()]
            .descriptor
            .ok_or("TMEM load requires staged SetTile state for its tile")?;
        Ok((epoch, image, descriptor))
    }

    pub(crate) fn commit_load(&mut self, load: TmemLoad, size: TileSize) {
        let tile = load.tile().array_index();
        self.tiles[tile].size = Some(size);
        self.tiles[tile].last_load_epoch = Some(load.epoch());
        self.last_load = Some(load);
        self.armed_load_sync = None;
    }
}
