//! Physical RDRAM coverage memory for the native raw-DPC backend.
//!
//! Public N64 Programming Manual section 15.5.6 defines RGBA16's visible
//! bit zero as coverage bit two; the remaining two bits live beside each
//! physical RDRAM halfword. A target key is therefore not the authority for
//! coverage: changing width or format does not move those physical bits.

use fn64_render_ir::PhysicalMemoryLayout;
use std::sync::Arc;

use super::{ColorTargetExtent, ColorTargetFormat, ColorTargetKey, TargetError};
use crate::Coverage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HiddenSample {
    visible: u16,
    bits: u8,
}
#[cfg(test)]
mod tests {
    use super::*;

    const RDRAM_BYTES: u32 = 0x80_0000;

    fn layout() -> PhysicalMemoryLayout {
        PhysicalMemoryLayout::try_new(RDRAM_BYTES).unwrap()
    }

    fn key(address: u32, width: u32, height: u32, format: ColorTargetFormat) -> ColorTargetKey {
        let layout = layout();
        ColorTargetKey::try_new(
            layout.address(address).unwrap(),
            ColorTargetExtent::try_new(width, height).unwrap(),
            format,
        )
        .unwrap()
    }

    fn rgba16_fixture(
        address: u32,
        width: u32,
        height: u32,
    ) -> (ColorTargetKey, Vec<u8>, ColorCoverageState) {
        let key = key(address, width, height, ColorTargetFormat::Rgba16);
        let pixels = key.extent().pixels() as usize;
        let mut bytes = Vec::with_capacity(pixels * 2);
        let mut cells = Vec::with_capacity(pixels);
        for index in 0..pixels {
            let count = (index % 8 + 1) as u8;
            let stored = Coverage::new(count).stored();
            let visible = ((index as u16) << 1) | u16::from((stored >> 2) & 1);
            bytes.extend_from_slice(&visible.to_be_bytes());
            cells.push(count);
        }
        (
            key,
            bytes,
            ColorCoverageState {
                cells: cells.into_boxed_slice(),
                unknown_cells: 0,
            },
        )
    }

    fn apply_scalar_rgba16(hidden: &mut RdramHiddenCoverage, first_address: u32, payload: &[u32]) {
        for (index, packed) in payload.iter().copied().enumerate() {
            let Some(sample) = RdramHiddenCoverage::decode(packed) else {
                continue;
            };
            hidden.insert(first_address + index as u32 * 2, sample);
        }
    }

    #[test]
    fn packed_rgba16_publication_is_page_local_and_matches_scalar_oracle() {
        let (key, bytes, coverage) = rgba16_fixture(0, 480, 240);
        let publication = HiddenCoveragePublication::try_new(key, &bytes, &coverage).unwrap();
        let (runs, payload) = publication.structural_counts();
        assert_eq!(runs, 29);
        assert_eq!(payload, 480 * 240);

        let scalar_payload = publication.payload.to_vec();
        let mut packed = RdramHiddenCoverage::new(layout());
        publication.apply(&mut packed);
        let mut scalar = RdramHiddenCoverage::new(layout());
        apply_scalar_rgba16(&mut scalar, 0, &scalar_payload);
        assert_eq!(packed, scalar);
    }

    #[test]
    fn packed_publication_preserves_unknown_last_write_and_cow_snapshot_semantics() {
        let target_key = key(0x1ffe, 3, 1, ColorTargetFormat::Rgba16);
        let initial_bytes = [0x12, 0x35, 0x22, 0x22, 0x32, 0x30];
        let initial = ColorCoverageState {
            cells: vec![8, 4, 2].into_boxed_slice(),
            unknown_cells: 0,
        };
        let mut hidden = RdramHiddenCoverage::new(layout());
        HiddenCoveragePublication::try_new(target_key, &initial_bytes, &initial)
            .unwrap()
            .apply(&mut hidden);
        let snapshot = hidden.clone();

        let later_bytes = [0x44, 0x45, 0x54, 0x52, 0x64, 0x60];
        let later_cells = [8, 4, 0];
        let final_bytes = [0x74, 0x75];
        let final_cells = [8];
        HiddenCoveragePublication::try_from_fragments(
            target_key,
            [
                (0, later_bytes.as_slice(), later_cells.as_slice()),
                (2, final_bytes.as_slice(), final_cells.as_slice()),
            ],
        )
        .unwrap()
        .apply(&mut hidden);

        assert_eq!(snapshot.rgba16_coverage(0x1ffe, 0x1235).count(), 8);
        assert_eq!(snapshot.rgba16_coverage(0x2000, 0x2222).count(), 4);
        assert_eq!(hidden.rgba16_coverage(0x1ffe, 0x4445).count(), 8);
        assert_eq!(hidden.rgba16_coverage(0x2000, 0x7475).count(), 8);
        assert_eq!(hidden.rgba16_coverage(0x2002, 0x3230).count(), 2);
        assert_eq!(snapshot.rgba16_coverage(0x2002, 0x3230).count(), 2);

        let unknown_key = key(0x3000, 1, 1, ColorTargetFormat::Rgba16);
        let unknown = ColorCoverageState::unknown(unknown_key.extent());
        let mut absent = RdramHiddenCoverage::new(layout());
        HiddenCoveragePublication::try_new(unknown_key, &[0, 0], &unknown)
            .unwrap()
            .apply(&mut absent);
        assert!(!absent.contains(0x3000));
        assert!(absent.pages.is_empty());
    }

    #[test]
    fn rgba32_refreshes_only_existing_visible_markers() {
        let mut hidden = RdramHiddenCoverage::new(layout());
        hidden.insert(
            0x4000,
            HiddenSample {
                visible: 0x1235,
                bits: 2,
            },
        );
        let snapshot = hidden.clone();
        let key = key(0x4000, 1, 1, ColorTargetFormat::Rgba32);
        let bytes = [0xab, 0xcd, 0xef, 0x01];
        let coverage = ColorCoverageState {
            cells: vec![8].into_boxed_slice(),
            unknown_cells: 0,
        };
        HiddenCoveragePublication::try_new(key, &bytes, &coverage)
            .unwrap()
            .apply(&mut hidden);

        assert_eq!(
            hidden.get(0x4000),
            Some(HiddenSample {
                visible: 0xabcd,
                bits: 2
            })
        );
        assert_eq!(hidden.get(0x4002), None);
        assert_eq!(
            snapshot.get(0x4000),
            Some(HiddenSample {
                visible: 0x1235,
                bits: 2
            })
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HiddenCoverageRunKind {
    Rgba16,
    Rgba32Visible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HiddenCoverageRun {
    first_slot: usize,
    payload_start: usize,
    payload_len: usize,
    kind: HiddenCoverageRunKind,
}

/// A fully checked, sealed physical hidden-memory update. Construction
/// performs every fallible invariant check before the guest commit boundary;
/// applying it only writes already validated operations.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HiddenCoveragePublication {
    runs: Arc<[HiddenCoverageRun]>,
    payload: Arc<[u32]>,
}

impl HiddenCoveragePublication {
    /// Revalidates a complete visible/coverage image without materializing a
    /// second hidden-memory payload. A caller which already owns a sealed
    /// sparse publication uses this to retain the full-image validation bar
    /// while reusing that publication's packed operations.
    pub(crate) fn validate_full_image(
        key: ColorTargetKey,
        bytes: &[u8],
        coverage: &ColorCoverageState,
    ) -> Result<(), TargetError> {
        let expected_bytes = key.range().len() as usize;
        if bytes.len() != expected_bytes {
            return Err(TargetError::HiddenCoverageByteLengthMismatch {
                key,
                expected: expected_bytes,
                actual: bytes.len(),
            });
        }
        let expected_cells = key.extent().pixels() as usize;
        if coverage.cells.len() != expected_cells {
            return Err(TargetError::HiddenCoverageCellCountMismatch {
                key,
                expected: expected_cells,
                actual: coverage.cells.len(),
            });
        }
        match key.format() {
            ColorTargetFormat::Rgba16 => {
                for (index, (pixel, count)) in bytes
                    .chunks_exact(2)
                    .zip(coverage.cells.iter().copied())
                    .enumerate()
                {
                    if count == 0 {
                        continue;
                    }
                    if count > 8 {
                        return Err(TargetError::HiddenCoverageCountInvalid { key, index, count });
                    }
                    let visible = u16::from_be_bytes([pixel[0], pixel[1]]);
                    let expected_visible = (Coverage::new(count).stored() >> 2) & 1;
                    let actual_visible = (visible & 1) as u8;
                    if actual_visible != expected_visible {
                        return Err(TargetError::HiddenCoverageVisibleBitMismatch {
                            key,
                            index,
                            count,
                            expected: expected_visible,
                            actual: actual_visible,
                        });
                    }
                }
            }
            ColorTargetFormat::Rgba32 => {}
        }
        Ok(())
    }

    pub(crate) fn try_new(
        key: ColorTargetKey,
        bytes: &[u8],
        coverage: &ColorCoverageState,
    ) -> Result<Self, TargetError> {
        let expected_bytes = key.range().len() as usize;
        if bytes.len() != expected_bytes {
            return Err(TargetError::HiddenCoverageByteLengthMismatch {
                key,
                expected: expected_bytes,
                actual: bytes.len(),
            });
        }
        let expected_cells = key.extent().pixels() as usize;
        if coverage.cells.len() != expected_cells {
            return Err(TargetError::HiddenCoverageCellCountMismatch {
                key,
                expected: expected_cells,
                actual: coverage.cells.len(),
            });
        }

        Self::try_from_fragments(key, [(0, bytes, coverage.cells.as_ref())])
    }

    /// Seals hidden-memory effects for only the journal-declared byte runs of
    /// a sparse color checkpoint. Hidden-only CLR_ON_CVG effects still belong
    /// to those runs; pixels outside them were never executed by the member.
    pub(crate) fn try_from_fragments<'a>(
        key: ColorTargetKey,
        fragments: impl IntoIterator<Item = (usize, &'a [u8], &'a [u8])>,
    ) -> Result<Self, TargetError> {
        let target_bytes = key.range().len() as usize;
        let bytes_per_pixel = key.format().bytes_per_pixel() as usize;
        let mut runs = Vec::new();
        let mut payload = Vec::new();
        for (byte_start, bytes, coverage) in fragments {
            let byte_end = byte_start.checked_add(bytes.len()).ok_or(
                TargetError::HiddenCoverageFragmentOutsideTarget {
                    key,
                    start: byte_start,
                    len: bytes.len(),
                },
            )?;
            if !byte_start.is_multiple_of(bytes_per_pixel)
                || !bytes.len().is_multiple_of(bytes_per_pixel)
                || byte_end > target_bytes
            {
                return Err(TargetError::HiddenCoverageFragmentOutsideTarget {
                    key,
                    start: byte_start,
                    len: bytes.len(),
                });
            }
            let expected_cells = bytes.len() / bytes_per_pixel;
            if coverage.len() != expected_cells {
                return Err(TargetError::HiddenCoverageCellCountMismatch {
                    key,
                    expected: expected_cells,
                    actual: coverage.len(),
                });
            }
            let first_pixel = byte_start / bytes_per_pixel;
            payload.reserve(match key.format() {
                ColorTargetFormat::Rgba16 => expected_cells,
                ColorTargetFormat::Rgba32 => bytes.len() / 2,
            });
            let fragment_payload_start = payload.len();
            match key.format() {
                ColorTargetFormat::Rgba16 => {
                    for (index, (pixel, count)) in bytes
                        .chunks_exact(2)
                        .zip(coverage.iter().copied())
                        .enumerate()
                    {
                        let target_index = first_pixel + index;
                        if count == 0 {
                            payload.push(RdramHiddenCoverage::EMPTY);
                            continue;
                        }
                        if count > 8 {
                            return Err(TargetError::HiddenCoverageCountInvalid {
                                key,
                                index: target_index,
                                count,
                            });
                        }
                        let visible = u16::from_be_bytes([pixel[0], pixel[1]]);
                        let stored = Coverage::new(count).stored();
                        let expected_visible = (stored >> 2) & 1;
                        let actual_visible = (visible & 1) as u8;
                        if actual_visible != expected_visible {
                            return Err(TargetError::HiddenCoverageVisibleBitMismatch {
                                key,
                                index: target_index,
                                count,
                                expected: expected_visible,
                                actual: actual_visible,
                            });
                        }
                        payload.push(RdramHiddenCoverage::encode(HiddenSample {
                            visible,
                            bits: stored & 3,
                        }));
                    }
                }
                ColorTargetFormat::Rgba32 => {
                    for bytes in bytes.chunks_exact(2) {
                        payload.push(u32::from(u16::from_be_bytes([bytes[0], bytes[1]])));
                    }
                }
            }
            let first_slot = (key.address().get() as usize + byte_start) >> 1;
            let fragment_payload_len = payload.len() - fragment_payload_start;
            let kind = match key.format() {
                ColorTargetFormat::Rgba16 => HiddenCoverageRunKind::Rgba16,
                ColorTargetFormat::Rgba32 => HiddenCoverageRunKind::Rgba32Visible,
            };
            let mut offset = 0;
            while offset < fragment_payload_len {
                let slot = first_slot + offset;
                let page_remaining =
                    RdramHiddenCoverage::PAGE_SLOTS - slot % RdramHiddenCoverage::PAGE_SLOTS;
                let len = page_remaining.min(fragment_payload_len - offset);
                runs.push(HiddenCoverageRun {
                    first_slot: slot,
                    payload_start: fragment_payload_start + offset,
                    payload_len: len,
                    kind,
                });
                offset += len;
            }
        }
        Ok(Self {
            runs: runs.into(),
            payload: payload.into(),
        })
    }

    pub(crate) fn shared(&self) -> Self {
        Self {
            runs: Arc::clone(&self.runs),
            payload: Arc::clone(&self.payload),
        }
    }

    pub(crate) fn apply_ref(&self, hidden: &mut RdramHiddenCoverage) {
        for run in self.runs.iter() {
            let source = &self.payload[run.payload_start..run.payload_start + run.payload_len];
            match run.kind {
                HiddenCoverageRunKind::Rgba16 => hidden.apply_rgba16_run(run.first_slot, source),
                HiddenCoverageRunKind::Rgba32Visible => {
                    hidden.apply_rgba32_visible_run(run.first_slot, source)
                }
            }
        }
    }

    pub(crate) fn apply(self, hidden: &mut RdramHiddenCoverage) {
        self.apply_ref(hidden);
    }

    #[cfg(test)]
    pub(crate) fn structural_counts(&self) -> (usize, usize) {
        (self.runs.len(), self.payload.len())
    }
}

/// Lazy dense storage indexed by physical RDRAM halfword.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RdramHiddenCoverage {
    layout: PhysicalMemoryLayout,
    pages: Vec<Option<Arc<[u32; Self::PAGE_SLOTS]>>>,
}

impl RdramHiddenCoverage {
    const EMPTY: u32 = u32::MAX;
    // One page is 16 KiB. A task-private registry snapshot clones only the
    // Arc table, then copies physical coverage pages it actually writes.
    // A single dense allocation made the first sparse publication of every
    // task copy all 16 MiB of hidden state for an 8 MiB RDRAM layout.
    const PAGE_SLOTS: usize = 4096;

    pub(crate) fn new(layout: PhysicalMemoryLayout) -> Self {
        Self {
            layout,
            pages: Vec::new(),
        }
    }

    fn slot(&self, address: u32) -> Option<usize> {
        if address & 1 != 0 || address >= self.layout.bytes() {
            return None;
        }
        Some(address as usize >> 1)
    }

    fn decode(packed: u32) -> Option<HiddenSample> {
        (packed != Self::EMPTY).then_some(HiddenSample {
            visible: packed as u16,
            bits: ((packed >> 16) & 3) as u8,
        })
    }

    fn encode(sample: HiddenSample) -> u32 {
        u32::from(sample.visible) | (u32::from(sample.bits & 3) << 16)
    }

    fn ensure_storage(&mut self) {
        if self.pages.is_empty() {
            let halfwords = self.layout.bytes() as usize / 2;
            self.pages
                .resize_with(halfwords.div_ceil(Self::PAGE_SLOTS), || None);
        }
    }

    fn get(&self, address: u32) -> Option<HiddenSample> {
        let slot = self.slot(address)?;
        let page = self.pages.get(slot / Self::PAGE_SLOTS)?.as_ref()?;
        Self::decode(page[slot % Self::PAGE_SLOTS])
    }

    fn insert(&mut self, address: u32, sample: HiddenSample) {
        let slot = self.slot(address).unwrap_or_else(|| {
            panic!("hidden coverage address must be an in-range halfword: {address:#010x}")
        });
        self.ensure_storage();
        let page = self.pages[slot / Self::PAGE_SLOTS]
            .get_or_insert_with(|| Arc::new([Self::EMPTY; Self::PAGE_SLOTS]));
        Arc::make_mut(page)[slot % Self::PAGE_SLOTS] = Self::encode(sample);
    }

    fn apply_rgba16_run(&mut self, first_slot: usize, source: &[u32]) {
        debug_assert!(!source.is_empty());
        debug_assert_eq!(
            first_slot / Self::PAGE_SLOTS,
            (first_slot + source.len() - 1) / Self::PAGE_SLOTS
        );
        if source.iter().all(|packed| *packed == Self::EMPTY) {
            return;
        }
        self.ensure_storage();
        let page_index = first_slot / Self::PAGE_SLOTS;
        let page_offset = first_slot % Self::PAGE_SLOTS;
        let page =
            self.pages[page_index].get_or_insert_with(|| Arc::new([Self::EMPTY; Self::PAGE_SLOTS]));
        let destination = &mut Arc::make_mut(page)[page_offset..page_offset + source.len()];
        if source.iter().all(|packed| *packed != Self::EMPTY) {
            destination.copy_from_slice(source);
        } else {
            for (destination, packed) in destination.iter_mut().zip(source.iter().copied()) {
                if packed != Self::EMPTY {
                    *destination = packed;
                }
            }
        }
    }

    fn apply_rgba32_visible_run(&mut self, first_slot: usize, source: &[u32]) {
        debug_assert!(!source.is_empty());
        debug_assert_eq!(
            first_slot / Self::PAGE_SLOTS,
            (first_slot + source.len() - 1) / Self::PAGE_SLOTS
        );
        let page_index = first_slot / Self::PAGE_SLOTS;
        let page_offset = first_slot % Self::PAGE_SLOTS;
        let Some(page) = self.pages.get(page_index).and_then(Option::as_ref) else {
            return;
        };
        let range = page_offset..page_offset + source.len();
        if page[range.clone()]
            .iter()
            .all(|packed| *packed == Self::EMPTY)
        {
            return;
        }
        let page = self.pages[page_index]
            .as_mut()
            .expect("the immutable preflight observed this hidden page");
        for (destination, visible) in Arc::make_mut(page)[range]
            .iter_mut()
            .zip(source.iter().copied())
        {
            if *destination != Self::EMPTY {
                *destination = (*destination & 0x0003_0000) | visible;
            }
        }
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        self.get(address).is_some()
    }

    /// Records the Programming Manual's non-RDP 16-bit write rule, including
    /// same-value writes which a visible-byte comparison cannot detect.
    pub(crate) fn record_non_rdp_write16(
        &mut self,
        address: u32,
        visible: u16,
        owned: bool,
    ) -> bool {
        if !owned || self.slot(address).is_none() {
            return false;
        }
        self.insert(
            address,
            HiddenSample {
                visible,
                bits: if visible & 1 == 0 { 0 } else { 3 },
            },
        );
        true
    }

    pub(crate) fn rgba16_coverage(&self, address: u32, visible: u16) -> Coverage {
        let hidden = self
            .get(address)
            .filter(|sample| sample.visible == visible)
            .map(|sample| sample.bits)
            .unwrap_or_else(|| if visible & 1 == 0 { 0 } else { 3 });
        Coverage::from_stored((((visible & 1) as u8) << 2) | hidden)
    }

    pub(crate) fn project(&self, key: ColorTargetKey, bytes: &[u8]) -> ColorCoverageState {
        assert_eq!(bytes.len(), key.range().len() as usize);
        let cells: Vec<u8> = match key.format() {
            ColorTargetFormat::Rgba16 => bytes
                .chunks_exact(2)
                .enumerate()
                .map(|(index, pixel)| {
                    let address = key.address().get() + (index as u32) * 2;
                    self.rgba16_coverage(address, u16::from_be_bytes([pixel[0], pixel[1]]))
                        .count()
                })
                .collect(),
            ColorTargetFormat::Rgba32 => bytes
                .chunks_exact(4)
                .map(|pixel| Coverage::from_stored(pixel[3] >> 5).count())
                .collect(),
        };
        ColorCoverageState {
            cells: cells.into_boxed_slice(),
            unknown_cells: 0,
        }
    }

    /// Publishes every known target-local coverage cell into physical hidden
    /// memory. Unknown cells are deliberately skipped: a partial new target
    /// cannot manufacture hidden bits for bytes it did not write.
    #[cfg(test)]
    pub(crate) fn publish(
        &mut self,
        key: ColorTargetKey,
        bytes: &[u8],
        coverage: &ColorCoverageState,
    ) -> Result<(), TargetError> {
        HiddenCoveragePublication::try_new(key, bytes, coverage)?.apply(self);
        Ok(())
    }
}

/// Transaction-local projection of physical coverage for one target.
/// Zero is the explicit unknown sentinel; exact sample populations are 1..=8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ColorCoverageState {
    pub(super) cells: Box<[u8]>,
    unknown_cells: usize,
}

/// A copied subrange whose source coverage state proved every cell exact.
/// Keeping that fact in the type lets spatial joins avoid cloning and then
/// overwriting a full coverage plane without rescanning every count.
pub(in crate::targets) struct ExactCoverageFragment {
    cells: Box<[u8]>,
}

impl ColorCoverageState {
    pub(super) fn len(&self) -> usize {
        self.cells.len()
    }

    pub(crate) fn unknown(extent: ColorTargetExtent) -> Self {
        let unknown_cells = extent.pixels() as usize;
        Self {
            cells: vec![0; unknown_cells].into_boxed_slice(),
            unknown_cells,
        }
    }

    /// Transfers the exact plane into one color command, leaving a
    /// deliberately unusable placeholder until that command's completion is
    /// threaded back into the ordered accumulator. A failed command aborts
    /// the task, so there is no rollback path which may observe the empty
    /// state.
    pub(crate) fn take_for_command(&mut self) -> Self {
        core::mem::replace(
            self,
            Self {
                cells: Box::default(),
                unknown_cells: 0,
            },
        )
    }

    pub(crate) fn exact(&self, pixel: usize) -> Option<Coverage> {
        let count = *self.cells.get(pixel)?;
        (count != 0).then(|| Coverage::new(count))
    }

    pub(crate) fn is_all_unknown(&self) -> bool {
        self.unknown_cells == self.cells.len()
    }

    #[cfg(test)]
    pub(crate) const fn unknown_cells(&self) -> usize {
        self.unknown_cells
    }

    pub(crate) fn set_exact(&mut self, pixel: usize, coverage: Coverage) {
        assert!(
            coverage.count() != 0,
            "zero fragment coverage is never stored"
        );
        if self.cells[pixel] == 0 {
            self.unknown_cells -= 1;
        }
        self.cells[pixel] = coverage.count();
    }

    pub(crate) fn cells_mut(&mut self) -> &mut [u8] {
        assert_eq!(
            self.unknown_cells, 0,
            "raster mutation requires a fully reconciled coverage plane"
        );
        &mut self.cells
    }

    pub(in crate::targets) fn exact_fragment(
        &self,
        range: std::ops::Range<usize>,
    ) -> ExactCoverageFragment {
        assert_eq!(
            self.unknown_cells, 0,
            "an exact coverage fragment cannot retain unknown cells"
        );
        ExactCoverageFragment {
            cells: self
                .cells
                .get(range)
                .expect("exact coverage fragment range must be in bounds")
                .to_vec()
                .into_boxed_slice(),
        }
    }

    pub(in crate::targets) fn from_exact_fragments<'a>(
        extent: ColorTargetExtent,
        fragments: impl IntoIterator<Item = &'a ExactCoverageFragment>,
    ) -> Self {
        let expected = extent.pixels() as usize;
        let mut cells = Vec::with_capacity(expected);
        for fragment in fragments {
            cells.extend_from_slice(&fragment.cells);
        }
        assert_eq!(
            cells.len(),
            expected,
            "exact coverage fragments must cover the complete target"
        );
        Self {
            cells: cells.into_boxed_slice(),
            unknown_cells: 0,
        }
    }

    /// Preserves the standalone executor's pre-sidecar behavior while making
    /// the uncertainty explicit. Production supplies a physical projection;
    /// only cells still unknown here are reconstructed from visible storage.
    pub(crate) fn reconcile_unknown_visible(&mut self, key: ColorTargetKey, bytes: &[u8]) {
        assert_eq!(bytes.len(), key.range().len() as usize);
        if self.unknown_cells == 0 {
            return;
        }
        for (index, cell) in self.cells.iter_mut().enumerate() {
            if *cell != 0 {
                continue;
            }
            *cell = match key.format() {
                ColorTargetFormat::Rgba16 => {
                    let visible = bytes[index * 2 + 1] & 1;
                    if visible == 0 {
                        1
                    } else {
                        8
                    }
                }
                ColorTargetFormat::Rgba32 => {
                    Coverage::from_stored(bytes[index * 4 + 3] >> 5).count()
                }
            };
        }
        self.unknown_cells = 0;
    }

    #[cfg(test)]
    pub(crate) fn reconcile_unknown_visible_scan_oracle(
        &mut self,
        key: ColorTargetKey,
        bytes: &[u8],
    ) {
        assert_eq!(bytes.len(), key.range().len() as usize);
        for (index, cell) in self.cells.iter_mut().enumerate() {
            if *cell != 0 {
                continue;
            }
            *cell = match key.format() {
                ColorTargetFormat::Rgba16 => {
                    if bytes[index * 2 + 1] & 1 == 0 {
                        1
                    } else {
                        8
                    }
                }
                ColorTargetFormat::Rgba32 => {
                    Coverage::from_stored(bytes[index * 4 + 3] >> 5).count()
                }
            };
        }
        self.unknown_cells = 0;
    }

    pub(super) fn copy_patch(&mut self, pixel_start: usize, patch: &[u8]) {
        let pixel_end = pixel_start + patch.len();
        let destination = &mut self.cells[pixel_start..pixel_end];
        for (cell, exact) in destination.iter_mut().zip(patch.iter().copied()) {
            match (*cell == 0, exact == 0) {
                (true, false) => self.unknown_cells -= 1,
                (false, true) => self.unknown_cells += 1,
                _ => {}
            }
            *cell = exact;
        }
    }

    pub(super) fn patch_for_byte_range(
        &self,
        key: ColorTargetKey,
        byte_start: usize,
        byte_len: usize,
    ) -> Box<[u8]> {
        let bpp = key.format().bytes_per_pixel() as usize;
        assert!(byte_start.is_multiple_of(bpp));
        assert!(byte_len.is_multiple_of(bpp));
        let start = byte_start / bpp;
        let end = start + byte_len / bpp;
        self.cells[start..end].to_vec().into_boxed_slice()
    }
}
