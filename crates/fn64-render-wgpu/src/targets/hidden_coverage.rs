use super::{ColorTargetExtent, ColorTargetFormat, ColorTargetKey};
use crate::Coverage;

/// Transaction-local projection of physical coverage for one target.
/// Zero is the explicit unknown sentinel; exact sample populations are 1..=8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ColorCoverageState {
    pub(super) cells: Box<[u8]>,
    unknown_cells: usize,
}

impl ColorCoverageState {
    pub(crate) fn unknown(extent: ColorTargetExtent) -> Self {
        let unknown_cells = extent.pixels() as usize;
        Self {
            cells: vec![0; unknown_cells].into_boxed_slice(),
            unknown_cells,
        }
    }

    pub(crate) fn exact(&self, pixel: usize) -> Option<Coverage> {
        let count = *self.cells.get(pixel)?;
        (count != 0).then(|| Coverage::new(count))
    }

    pub(crate) fn set_exact(&mut self, pixel: usize, coverage: Coverage) {
        assert_ne!(
            coverage.count(),
            0,
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
}

#[cfg(test)]
mod tests {
    use fn64_render_ir::PhysicalMemoryLayout;

    use super::*;

    fn key(format: ColorTargetFormat) -> ColorTargetKey {
        let layout = PhysicalMemoryLayout::try_new(4096).unwrap();
        ColorTargetKey::try_new(
            layout.address(0).unwrap(),
            ColorTargetExtent::try_new(2, 1).unwrap(),
            format,
        )
        .unwrap()
    }

    #[test]
    fn unknown_cells_reconcile_from_the_formats_visible_coverage_bits() {
        let rgba16 = key(ColorTargetFormat::Rgba16);
        let mut coverage = ColorCoverageState::unknown(rgba16.extent());
        assert_eq!(coverage.exact(0), None);
        coverage.reconcile_unknown_visible(rgba16, &[0, 0, 0, 1]);
        assert_eq!(coverage.exact(0), Some(Coverage::new(1)));
        assert_eq!(coverage.exact(1), Some(Coverage::FULL));

        let rgba32 = key(ColorTargetFormat::Rgba32);
        let mut coverage = ColorCoverageState::unknown(rgba32.extent());
        coverage.reconcile_unknown_visible(rgba32, &[0, 0, 0, 3 << 5, 0, 0, 0, 7 << 5]);
        assert_eq!(coverage.exact(0), Some(Coverage::new(4)));
        assert_eq!(coverage.exact(1), Some(Coverage::FULL));
    }

    #[test]
    fn exact_cells_survive_reconciliation_and_unlock_mutation_only_when_complete() {
        let key = key(ColorTargetFormat::Rgba16);
        let mut coverage = ColorCoverageState::unknown(key.extent());
        coverage.set_exact(0, Coverage::new(3));
        coverage.reconcile_unknown_visible(key, &[0, 0, 0, 1]);
        assert_eq!(coverage.cells_mut(), &[3, 8]);
    }
}
