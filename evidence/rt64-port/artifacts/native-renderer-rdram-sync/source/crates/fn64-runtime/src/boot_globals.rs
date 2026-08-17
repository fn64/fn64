//! Typed IPL-owned globals present before the ROM header entry runs.
//!
//! Provenance: the public libultra Functions Reference, “N64 Global Variables,”
//! defines the numeric `osTvType` and `osResetType` values. The public
//! `os_system.h` header reference identifies `osRomBase` as the game-image ROM
//! base. The public N64 memory map places cartridge domain 1 address 2 at
//! physical `0x1000_0000`; the MIPS KSEG1 mapping therefore gives its canonical
//! unmapped alias `0xb000_0000`.

use crate::{RdramAddr, RdramViewMut, TvType};

pub const OS_TV_TYPE_ADDR: RdramAddr = RdramAddr::from_offset(0x300);
pub const OS_ROM_BASE_ADDR: RdramAddr = RdramAddr::from_offset(0x308);
pub const OS_RESET_TYPE_ADDR: RdramAddr = RdramAddr::from_offset(0x30c);
pub const CART_ROM_KSEG1_BASE: u32 = 0xb000_0000;
const IPL_BOOT_GLOBAL_STORAGE_END: usize = 0x310;

/// IPL-selected reset cause stored in the public `osResetType` global.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum IplResetType {
    Cold = 0,
    Nmi = 1,
}

/// Complete typed value set for the IPL-owned globals read during early boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IplBootGlobals {
    pub tv_type: TvType,
    pub reset_type: IplResetType,
}

impl IplBootGlobals {
    pub const fn cold(tv_type: TvType) -> Self {
        Self {
            tv_type,
            reset_type: IplResetType::Cold,
        }
    }

    /// Exact address/value authority installed by [`Self::install`].
    pub const fn words(self) -> [(RdramAddr, u32); 3] {
        [
            (OS_TV_TYPE_ADDR, self.tv_type as u32),
            (OS_ROM_BASE_ADDR, CART_ROM_KSEG1_BASE),
            (OS_RESET_TYPE_ADDR, self.reset_type as u32),
        ]
    }

    pub fn install(self, storage: &mut [u8]) {
        assert!(
            storage.len() >= IPL_BOOT_GLOBAL_STORAGE_END,
            "IplBootGlobals::install requires RDRAM storage through 0x{IPL_BOOT_GLOBAL_STORAGE_END:x}, got 0x{:x} bytes",
            storage.len()
        );
        let mut view = RdramViewMut::from_storage(storage);
        for (address, value) in self.words() {
            view.write_u32(address, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RdramView;

    #[test]
    fn installs_exact_public_ipl_global_values() {
        for (tv_type, expected_tv) in [(TvType::Pal, 0), (TvType::Ntsc, 1), (TvType::Mpal, 2)] {
            for (reset_type, expected_reset) in [(IplResetType::Cold, 0), (IplResetType::Nmi, 1)] {
                let mut storage = [0xa5; 0x310];
                IplBootGlobals {
                    tv_type,
                    reset_type,
                }
                .install(&mut storage);
                let view = RdramView::from_storage(&storage);
                assert_eq!(view.read_u32(OS_TV_TYPE_ADDR), expected_tv);
                assert_eq!(view.read_u32(OS_ROM_BASE_ADDR), CART_ROM_KSEG1_BASE);
                assert_eq!(view.read_u32(OS_RESET_TYPE_ADDR), expected_reset);
                for (index, &byte) in storage.iter().enumerate() {
                    let written =
                        (0x300..0x304).contains(&index) || (0x308..0x310).contains(&index);
                    if !written {
                        assert_eq!(byte, 0xa5, "unexpected mutation at storage byte {index:#x}");
                    }
                }
            }
        }
    }

    #[test]
    fn short_storage_rejection_is_atomic() {
        let mut storage = [0xa5; IPL_BOOT_GLOBAL_STORAGE_END - 1];
        let before = storage;
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            IplBootGlobals::cold(TvType::Ntsc).install(&mut storage);
        }));
        assert!(panic.is_err());
        assert_eq!(storage, before);
    }
}
