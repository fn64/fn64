use crate::raster::Framebuffer;
use crate::{
    depth, gbi, png_dump, raster, render_unsupported_error, s2dex, vi, GeometryWireFamily,
    S2dexWireFamily,
};
use fn64_render::{
    F3dex2UcodeCatalog, FrameStatus, MicrocodeDataImageIdentity, MicrocodePairCatalog,
    NonRdpWrite16, NonRdpWrite16Disposition, OsTask, PresentMemory, PresentRequest, RenderBackend,
    RenderConfig, RenderError, S2dexUcodeCatalog, UcodeId, ViPixelType, ViPresentation,
    ViScanoutRegisters,
};

use super::*;
use super::vi_source::*;
use super::validate::*;
use super::framebuffer_io::*;
use super::imp::*;
use super::render_backend::*;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct RdramHiddenSample {
    pub(super) visible: u16,
    pub(super) bits: u8,
}

/// Dense physical storage for RDRAM's two hidden bits and their visible-word
/// coherence marker. Hidden state is indexed by physical halfword, so a hash
/// table paid hashing/allocation costs for a naturally dense bounded address
/// space. `u32::MAX` is outside the 18-bit packed sample domain and represents
/// an untouched halfword.
#[derive(Clone, Debug)]
pub(super) struct RdramHiddenBits {
    samples: Vec<u32>,
}

impl RdramHiddenBits {
    const EMPTY: u32 = u32::MAX;
    const HALFWORDS: usize = fn64_runtime::rdram::DEFAULT_RDRAM_SIZE / 2;

    pub(super) fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }

    pub(super) fn slot(address: u32) -> Option<usize> {
        if address & 1 != 0 || address >= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u32 {
            return None;
        }
        Some(address as usize >> 1)
    }

    pub(super) fn decode(packed: u32) -> Option<RdramHiddenSample> {
        (packed != Self::EMPTY).then_some(RdramHiddenSample {
            visible: packed as u16,
            bits: ((packed >> 16) & 3) as u8,
        })
    }

    pub(super) fn encode(sample: RdramHiddenSample) -> u32 {
        u32::from(sample.visible) | (u32::from(sample.bits & 3) << 16)
    }

    pub(super) fn ensure_storage(&mut self) {
        if self.samples.is_empty() {
            self.samples.resize(Self::HALFWORDS, Self::EMPTY);
        }
    }

    pub(super) fn get(&self, address: &u32) -> Option<RdramHiddenSample> {
        let slot = Self::slot(*address)?;
        self.samples.get(slot).copied().and_then(Self::decode)
    }

    pub(super) fn insert(&mut self, address: u32, sample: RdramHiddenSample) {
        let slot = Self::slot(address).unwrap_or_else(|| {
            panic!("hidden-RDRAM address must be an in-range halfword: {address:#010x}")
        });
        self.ensure_storage();
        self.samples[slot] = Self::encode(sample);
    }

    pub(super) fn insert_pair(&mut self, address: u32, first: RdramHiddenSample, second: RdramHiddenSample) {
        assert!(
            address.is_multiple_of(4),
            "hidden-RDRAM pair must begin at a word boundary: {address:#010x}"
        );
        let slot = Self::slot(address).unwrap_or_else(|| {
            panic!("hidden-RDRAM address must be an in-range halfword: {address:#010x}")
        });
        self.ensure_storage();
        let pair = self
            .samples
            .get_mut(slot..slot + 2)
            .expect("hidden-RDRAM word extends outside dense storage");
        pair[0] = Self::encode(first);
        pair[1] = Self::encode(second);
    }

    pub(super) fn update_visible(&mut self, address: u32, visible: u16) {
        let Some(slot) = Self::slot(address) else {
            return;
        };
        let Some(mut sample) = self.samples.get(slot).copied().and_then(Self::decode) else {
            return;
        };
        sample.visible = visible;
        self.samples[slot] = Self::encode(sample);
    }

    pub(super) fn contains_key(&self, address: &u32) -> bool {
        self.get(address).is_some()
    }

    pub(super) fn extend(&mut self, updates: impl IntoIterator<Item = (u32, RdramHiddenSample)>) {
        for (address, sample) in updates {
            self.insert(address, sample);
        }
    }

    pub(super) fn clear(&mut self) {
        self.samples.fill(Self::EMPTY);
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.samples.iter().all(|sample| *sample == Self::EMPTY)
    }
}

impl<const N: usize> From<[(u32, RdramHiddenSample); N]> for RdramHiddenBits {
    fn from(entries: [(u32, RdramHiddenSample); N]) -> Self {
        let mut hidden = Self::new();
        hidden.extend(entries);
        hidden
    }
}

pub(super) fn read_rdram_hidden_bits(hidden: &mut RdramHiddenBits, address: u32, visible: u16) -> u8 {
    if let Some(sample) = hidden.get(&address) {
        if sample.visible == visible {
            return sample.bits & 3;
        }
    }
    // Programming Manual 15.5.6: a non-RDP 16-bit write replicates the
    // visible LSB into both physical hidden bits. A changed visible word is
    // therefore observable evidence that another RDRAM master wrote it.
    record_non_rdp_16bit_write(hidden, address, visible)
}


/// Record a known non-RDP 16-bit write to one physical RDRAM halfword.
///
/// Programming Manual 15.5.6 defines this mutation even when the visible
/// value is unchanged: both hidden bits receive the visible LSB. The renderer
/// calls this from its changed-visible-word fallback. A same-value external
/// store requires the host to provide a write event because `&mut [u8]`
/// alone cannot distinguish that store from no mutation.
pub(super) fn record_non_rdp_16bit_write(hidden: &mut RdramHiddenBits, address: u32, visible: u16) -> u8 {
    let bits = if visible & 1 == 0 { 0 } else { 3 };
    hidden.insert(address, RdramHiddenSample { visible, bits });
    bits
}

pub(super) fn write_rdram_hidden_bits(hidden: &mut RdramHiddenBits, address: u32, visible: u16, bits: u8) {
    hidden.insert(
        address,
        RdramHiddenSample {
            visible,
            bits: bits & 3,
        },
    );
}

/// Refresh the CPU-visible halfword paired with already-owned physical hidden
/// bits after an RDP write through a layout that does not consume those bits.
/// I8 and RGBA32 preserve hidden storage, but failing to update this coherence
/// marker would make a later RGBA16 import misclassify the known RDP write as
/// an external non-RDP store and replace the preserved bits from the LSB.
pub(super) fn refresh_rdp_visible_halfwords_preserving_hidden(
    rdram: &[u8],
    hidden: &mut RdramHiddenBits,
    start: u32,
    byte_len: usize,
) {
    debug_assert!(start.is_multiple_of(2));
    let view = fn64_runtime::RdramView::from_storage(rdram);
    for byte_offset in (0..byte_len).step_by(2) {
        let Ok(byte_offset) = u32::try_from(byte_offset) else {
            break;
        };
        let Some(address) = start.checked_add(byte_offset) else {
            break;
        };
        if address as usize + 2 > view.len() {
            break;
        }
        hidden.update_visible(
            address,
            view.read_u16(fn64_runtime::RdramAddr::from_offset(address)),
        );
    }
}
