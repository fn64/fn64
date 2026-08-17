use core::num::NonZeroU32;

use crate::ValidationError;

/// The RDP exposes a 24-bit physical address field.
pub const RDP_PHYSICAL_ADDRESS_BYTES: u32 = 0x0100_0000;
pub const RSP_DMEM_BYTES: u32 = 0x1000;
pub const TMEM_BYTES: u32 = 0x1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhysicalMemoryLayout(NonZeroU32);

impl PhysicalMemoryLayout {
    pub fn try_new(bytes: u32) -> Result<Self, ValidationError> {
        let bytes = NonZeroU32::new(bytes).ok_or(ValidationError::ZeroMemoryLayout)?;
        if bytes.get() > RDP_PHYSICAL_ADDRESS_BYTES {
            return Err(ValidationError::MemoryLayoutTooLarge {
                bytes: bytes.get(),
                maximum: RDP_PHYSICAL_ADDRESS_BYTES,
            });
        }
        Ok(Self(bytes))
    }

    pub const fn bytes(self) -> u32 {
        self.0.get()
    }

    pub fn address(self, address: u32) -> Result<PhysicalAddress, ValidationError> {
        PhysicalAddress::with_layout(address, self)
    }

    pub fn range(self, start: u32, end: u32) -> Result<PhysicalRange, ValidationError> {
        PhysicalRange::with_layout(start, end, self)
    }
}

/// An RDP-visible physical byte address. Construction proves the hard 24-bit
/// hardware bound; a [`PhysicalMemoryLayout`] proves a narrower installed
/// RDRAM bound when one is available.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhysicalAddress {
    address: u32,
    layout: PhysicalMemoryLayout,
}

impl PhysicalAddress {
    pub fn try_new(address: u32) -> Result<Self, ValidationError> {
        Self::with_layout(
            address,
            PhysicalMemoryLayout::try_new(RDP_PHYSICAL_ADDRESS_BYTES)
                .expect("RDP physical address width is nonzero and self-bounded"),
        )
    }

    pub(crate) fn with_layout(
        address: u32,
        layout: PhysicalMemoryLayout,
    ) -> Result<Self, ValidationError> {
        let upper_bound = layout.bytes();
        if address >= upper_bound {
            return Err(ValidationError::AddressOutOfBounds {
                address,
                upper_bound,
            });
        }
        Ok(Self { address, layout })
    }

    pub const fn get(self) -> u32 {
        self.address
    }

    /// The exact installed-memory bound used to construct this address.
    pub const fn layout(self) -> PhysicalMemoryLayout {
        self.layout
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhysicalRange {
    start: PhysicalAddress,
    end: u32,
}

impl PhysicalRange {
    pub fn try_new(start: u32, end: u32) -> Result<Self, ValidationError> {
        Self::with_layout(
            start,
            end,
            PhysicalMemoryLayout::try_new(RDP_PHYSICAL_ADDRESS_BYTES)
                .expect("RDP physical address width is nonzero and self-bounded"),
        )
    }

    pub(crate) fn with_layout(
        start: u32,
        end: u32,
        layout: PhysicalMemoryLayout,
    ) -> Result<Self, ValidationError> {
        let upper_bound = layout.bytes();
        if start >= end {
            return Err(ValidationError::EmptyOrReversedRange { start, end });
        }
        if end > upper_bound {
            return Err(ValidationError::RangeOutOfBounds {
                start,
                end,
                upper_bound,
            });
        }
        Ok(Self {
            start: PhysicalAddress::with_layout(start, layout)?,
            end,
        })
    }

    pub const fn start(self) -> PhysicalAddress {
        self.start
    }

    pub const fn end(self) -> u32 {
        self.end
    }

    pub const fn len(self) -> u32 {
        self.end - self.start.address
    }

    pub const fn is_empty(self) -> bool {
        false
    }

    pub const fn contains(self, address: PhysicalAddress) -> bool {
        address.layout.bytes() == self.layout().bytes()
            && address.address >= self.start.address
            && address.address < self.end
    }

    /// The exact installed-memory bound retained by this range proof.
    pub const fn layout(self) -> PhysicalMemoryLayout {
        self.start.layout
    }

    pub fn require_alignment(self, alignment: u32) -> Result<Self, ValidationError> {
        assert!(
            alignment.is_power_of_two(),
            "alignment must be a nonzero power of two"
        );
        if !self.start.address.is_multiple_of(alignment) || !self.end.is_multiple_of(alignment) {
            return Err(ValidationError::UnalignedRange {
                start: self.start.address,
                end: self.end,
                alignment,
            });
        }
        Ok(self)
    }
}

macro_rules! fixed_range {
    ($name:ident, $bound:expr) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name {
            start: u16,
            end: u16,
        }

        impl $name {
            pub fn try_new(start: u32, end: u32) -> Result<Self, ValidationError> {
                if start >= end {
                    return Err(ValidationError::EmptyOrReversedRange { start, end });
                }
                if end > $bound {
                    return Err(ValidationError::RangeOutOfBounds {
                        start,
                        end,
                        upper_bound: $bound,
                    });
                }
                Ok(Self {
                    start: start as u16,
                    end: end as u16,
                })
            }

            pub const fn start(self) -> u32 {
                self.start as u32
            }

            pub const fn end(self) -> u32 {
                self.end as u32
            }

            pub const fn len(self) -> u32 {
                self.end() - self.start()
            }

            pub const fn is_empty(self) -> bool {
                false
            }

            pub fn require_alignment(self, alignment: u32) -> Result<Self, ValidationError> {
                assert!(
                    alignment.is_power_of_two(),
                    "alignment must be a nonzero power of two"
                );
                if !self.start().is_multiple_of(alignment) || !self.end().is_multiple_of(alignment)
                {
                    return Err(ValidationError::UnalignedRange {
                        start: self.start(),
                        end: self.end(),
                        alignment,
                    });
                }
                Ok(self)
            }
        }
    };
}

fixed_range!(DmemRange, RSP_DMEM_BYTES);
fixed_range!(TmemRange, TMEM_BYTES);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_ranges_reject_every_boundary_escape() {
        assert_eq!(
            PhysicalMemoryLayout::try_new(0).unwrap_err(),
            ValidationError::ZeroMemoryLayout
        );
        assert!(matches!(
            PhysicalMemoryLayout::try_new(RDP_PHYSICAL_ADDRESS_BYTES + 1),
            Err(ValidationError::MemoryLayoutTooLarge { .. })
        ));
        let layout = PhysicalMemoryLayout::try_new(8 * 1024 * 1024).unwrap();
        assert_eq!(
            layout.range(0, layout.bytes()).unwrap().len(),
            layout.bytes()
        );
        assert!(matches!(
            layout.address(layout.bytes()),
            Err(ValidationError::AddressOutOfBounds { .. })
        ));
        assert_eq!(layout.range(0, 8).unwrap().layout(), layout);
        assert_ne!(
            layout.range(0, 8).unwrap(),
            PhysicalMemoryLayout::try_new(16 * 1024 * 1024)
                .unwrap()
                .range(0, 8)
                .unwrap()
        );
        assert!(matches!(
            layout.range(layout.bytes() - 1, layout.bytes() + 1),
            Err(ValidationError::RangeOutOfBounds { .. })
        ));
    }

    #[test]
    fn device_local_ranges_are_bounded_and_alignment_is_explicit() {
        assert_eq!(DmemRange::try_new(0, RSP_DMEM_BYTES).unwrap().len(), 0x1000);
        assert!(DmemRange::try_new(0, RSP_DMEM_BYTES + 1).is_err());
        assert!(TmemRange::try_new(4, 12)
            .unwrap()
            .require_alignment(8)
            .is_err());
    }
}
