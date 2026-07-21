//! Shared boot seam for the headless examples and `fn64-shell`.
//!
//! The generated-C lane presents one process-global section table through
//! `bridge/section_bridge.c`. This crate owns that bridge's Rust callback,
//! batches its per-function records into `fn64-abi` sections, exposes the
//! generated entry point, and allocates the one ABI-visible RDRAM buffer.
//! Game policy remains with each harness: which sections are resident,
//! controller input, save type, rendering, audio, and executor driving.

#[cfg(feature = "c-bridge")]
use std::collections::HashMap;

pub use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;

pub use fn64_runtime::TvType;

const OS_TV_TYPE: fn64_runtime::RdramAddr = fn64_runtime::RdramAddr::from_offset(0x300);

/// Length required by generated `MEM_*` accesses during boot.
pub const fn rdram_len() -> usize {
    let mmio_end = fn64_runtime::RDRAM_MMIO_WINDOW_END as usize;
    if DEFAULT_RDRAM_SIZE > mmio_end {
        DEFAULT_RDRAM_SIZE
    } else {
        mmio_end
    }
}

/// Allocate the process's single RDRAM buffer, including the raw MMIO/KSEG1
/// window generated code can address directly, and seed the IPL-owned boot
/// globals that libultra/game initialization reads before any shim runs.
pub fn new_rdram(tv_type: TvType) -> Vec<u8> {
    fn64_abi::configure_tv_type(tv_type);
    let mut rdram = vec![0; rdram_len()];
    fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u32(OS_TV_TYPE, tv_type as u32);
    rdram
}

#[cfg(feature = "c-bridge")]
#[allow(improper_ctypes)]
extern "C" {
    fn fn64_bridge_register_all_sections();
    fn fn64_bridge_num_sections() -> usize;
    fn recomp_entrypoint(rdram: *mut u8, ctx: *mut fn64_abi::RecompContext);
}

/// One section registered from the generated `section_table[]`.
#[cfg(feature = "c-bridge")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisteredSection {
    pub source_index: usize,
    pub registry_index: fn64_runtime::SectionIndex,
    pub rom_addr: u32,
    pub ram_addr: u32,
    pub size: u32,
    pub function_count: usize,
}

/// Result of walking and registering one linked generated section table.
#[cfg(feature = "c-bridge")]
#[derive(Debug)]
pub struct SectionRegistration {
    reported_count: usize,
    sections: Vec<RegisteredSection>,
}

#[cfg(feature = "c-bridge")]
impl SectionRegistration {
    /// Number of entries reported by the generated `section_table[]`.
    pub fn reported_count(&self) -> usize {
        self.reported_count
    }

    /// Registered sections, ordered by their generated table index.
    pub fn sections(&self) -> &[RegisteredSection] {
        &self.sections
    }

    /// Runtime registry index corresponding to a generated table index.
    pub fn registry_index(&self, source_index: usize) -> Option<fn64_runtime::SectionIndex> {
        self.sections
            .iter()
            .find(|section| section.source_index == source_index)
            .map(|section| section.registry_index)
    }
}

#[cfg(feature = "c-bridge")]
type SectionEntry = (u32, u32, u32, Vec<(u32, u32, fn64_abi::RecompFunc)>);

#[cfg(feature = "c-bridge")]
#[derive(Default)]
struct SectionBuilder {
    sections: HashMap<usize, SectionEntry>,
}

#[cfg(feature = "c-bridge")]
thread_local! {
    static SECTION_BUILDER: std::cell::RefCell<SectionBuilder> =
        std::cell::RefCell::new(SectionBuilder::default());
}

/// Receive one `(section, function)` pair from `bridge/section_bridge.c`.
///
/// The C bridge emits one callback per `FuncEntry`; `fn64-abi` accepts a
/// complete function list per section, so this process-global accumulator is
/// the single adapter between those contracts.
#[cfg(feature = "c-bridge")]
#[no_mangle]
extern "C" fn fn64_register_func(
    section_index: usize,
    rom_addr: u32,
    ram_addr: u32,
    size: u32,
    offset: u32,
    rom_size: u32,
    func: fn64_abi::RecompFunc,
) {
    SECTION_BUILDER.with(|cell| {
        let mut builder = cell.borrow_mut();
        let entry = builder
            .sections
            .entry(section_index)
            .or_insert_with(|| (rom_addr, ram_addr, size, Vec::new()));
        entry.3.push((offset, rom_size, func));
    });
}

/// Walk the linked generated section table and register every section with
/// `fn64-abi` in generated-index order.
///
/// This is safe for harness callers because the bundled C bridge obtains all
/// function pointers from file-scope generated `FuncEntry` definitions,
/// satisfying `fn64_abi::register_section`'s process-lifetime requirement.
#[cfg(feature = "c-bridge")]
pub fn register_linked_sections() -> SectionRegistration {
    SECTION_BUILDER.with(|cell| cell.borrow_mut().sections.clear());

    // SAFETY: these two symbols are defined by the bundled bridge compiled
    // against the linked generated table. Its walk invokes the callback above
    // synchronously and its count is a plain read of generated `num_sections`.
    unsafe { fn64_bridge_register_all_sections() };
    let reported_count = unsafe { fn64_bridge_num_sections() };

    let sections = SECTION_BUILDER.with(|cell| {
        let builder = cell.borrow();
        let mut keys: Vec<_> = builder.sections.keys().copied().collect();
        keys.sort_unstable();
        keys.into_iter()
            .map(|source_index| {
                let (rom_addr, ram_addr, size, funcs) = &builder.sections[&source_index];
                // SAFETY: every pointer came directly from a file-scope
                // generated FuncEntry and remains valid for the process.
                let registry_index =
                    unsafe { fn64_abi::register_section(*rom_addr, *ram_addr, *size, funcs) };
                RegisteredSection {
                    source_index,
                    registry_index,
                    rom_addr: *rom_addr,
                    ram_addr: *ram_addr,
                    size: *size,
                    function_count: funcs.len(),
                }
            })
            .collect()
    });

    SectionRegistration {
        reported_count,
        sections,
    }
}

/// The linked generated C boot entry point.
#[cfg(feature = "c-bridge")]
pub fn c_recomp_entrypoint() -> fn64_abi::RecompFunc {
    recomp_entrypoint
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdram_length_covers_physical_memory_and_raw_mmio_window() {
        assert!(rdram_len() >= DEFAULT_RDRAM_SIZE);
        assert!(rdram_len() >= fn64_runtime::RDRAM_MMIO_WINDOW_END as usize);
    }

    #[test]
    fn television_standard_is_explicit_boot_state_not_zero_fill_accident() {
        for (tv_type, expected) in [(TvType::Pal, 0), (TvType::Ntsc, 1), (TvType::Mpal, 2)] {
            let rdram = new_rdram(tv_type);
            assert_eq!(
                fn64_runtime::RdramView::from_storage(&rdram).read_u32(OS_TV_TYPE),
                expected
            );
            assert_eq!(fn64_abi::configured_tv_type(), tv_type);
            assert_eq!(
                fn64_abi::vi_field_interval(),
                Some(tv_type.nominal_field_cycles())
            );
        }
    }
}
