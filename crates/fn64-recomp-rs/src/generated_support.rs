//! Shared cold paths used by generated arbitrary-PC runners.
//!
//! Code generators should reuse these helpers instead of spelling exception
//! construction and checked-memory retirement into every instruction arm.
//! Keeping the fault site typed preserves the architectural distinction
//! between a straight instruction and a branch delay slot while keeping the
//! successful generated path inline.

use crate::execution::{
    finalize_executable_write_exit, BankId, BlockExit, BlockRun, CpuException, CpuFault,
    CpuFaultKind, ExecutionKey, GuestPc,
};
use crate::runtime::{DataAccessError, DataAccessKind};

/// Architectural location of a synchronous fault in generated code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchitecturalFaultSite {
    at: ExecutionKey,
    epc: GuestPc,
    branch_delay: bool,
}

impl ArchitecturalFaultSite {
    /// A fault raised by an ordinary instruction.
    pub const fn straight(bank: BankId, pc: u32) -> Self {
        let pc = GuestPc::new(pc);
        Self {
            at: ExecutionKey::new(bank, pc),
            epc: pc,
            branch_delay: false,
        }
    }

    /// A fault raised by `at` while executing the delay slot of `epc`.
    pub const fn delay(bank: BankId, at: u32, epc: u32) -> Self {
        Self {
            at: ExecutionKey::new(bank, GuestPc::new(at)),
            epc: GuestPc::new(epc),
            branch_delay: true,
        }
    }

    const fn fault(self, kind: CpuFaultKind) -> CpuFault {
        CpuFault { at: self.at, kind }
    }
}

/// Construct an aligned-data address exception without duplicating its full
/// representation in every generated load and store arm.
#[cold]
#[inline(never)]
pub fn address_error(
    site: ArchitecturalFaultSite,
    access: DataAccessKind,
    bad_vaddr: u64,
) -> BlockExit {
    let exception = match access {
        DataAccessKind::Load => CpuException::AddressErrorLoad,
        DataAccessKind::Store => CpuException::AddressErrorStore,
    };
    BlockExit::Fault(site.fault(CpuFaultKind::Exception {
        exception,
        epc: site.epc,
        branch_delay: site.branch_delay,
        instruction_code: 0,
        bad_vaddr: Some(bad_vaddr),
        coprocessor: None,
    }))
}

/// Finish a checked-memory failure with the exact retirement rule used by a
/// generated instruction arm.
///
/// Architectural exceptions retire the faulting straight instruction for
/// clock accounting; a delay-slot fault has already counted its indivisible
/// branch pair. Host admission failures retain the caller-supplied retirement
/// count because they are not guest exceptions.
#[cold]
#[inline(never)]
pub fn finish_data_access_error(
    error: DataAccessError,
    site: ArchitecturalFaultSite,
    executed: u32,
    nonarchitectural_retired: u32,
) -> BlockRun {
    let architectural = error.is_architectural_exception();
    let kind = error.into_cpu_fault_kind(site.epc, site.branch_delay);
    let instructions = if architectural {
        if site.branch_delay {
            executed
        } else {
            executed + 1
        }
    } else {
        nonarchitectural_retired
    };
    BlockRun::new(
        finalize_executable_write_exit(site.at.bank, BlockExit::Fault(site.fault(kind))),
        instructions,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fault_sites_preserve_straight_and_delay_epc_rules() {
        let bank = BankId::new(7);
        let straight = address_error(
            ArchitecturalFaultSite::straight(bank, 0x8000_1000),
            DataAccessKind::Load,
            0x8000_1001,
        );
        let delay = address_error(
            ArchitecturalFaultSite::delay(bank, 0x8000_1004, 0x8000_1000),
            DataAccessKind::Store,
            0x8000_1002,
        );
        assert!(matches!(straight, BlockExit::Fault(CpuFault {
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorLoad,
                epc,
                branch_delay: false,
                bad_vaddr: Some(0x8000_1001),
                ..
            }, ..
        }) if epc == GuestPc::new(0x8000_1000)));
        assert!(matches!(delay, BlockExit::Fault(CpuFault {
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorStore,
                epc,
                branch_delay: true,
                bad_vaddr: Some(0x8000_1002),
                ..
            }, ..
        }) if epc == GuestPc::new(0x8000_1000)));
    }

    #[test]
    fn checked_memory_retirement_distinguishes_guest_and_host_faults() {
        let bank = BankId::new(9);
        let straight = finish_data_access_error(
            DataAccessError::AddressError {
                vaddr: 3,
                access: DataAccessKind::Load,
            },
            ArchitecturalFaultSite::straight(bank, 0x8000_2000),
            4,
            1,
        );
        let delay = finish_data_access_error(
            DataAccessError::AddressError {
                vaddr: 3,
                access: DataAccessKind::Store,
            },
            ArchitecturalFaultSite::delay(bank, 0x8000_2004, 0x8000_2000),
            6,
            2,
        );
        let host = finish_data_access_error(
            DataAccessError::Unbacked { vaddr: 3 },
            ArchitecturalFaultSite::straight(bank, 0x8000_2000),
            4,
            1,
        );
        assert_eq!(straight.instructions, 5);
        assert_eq!(delay.instructions, 6);
        assert_eq!(host.instructions, 1);
    }
}
